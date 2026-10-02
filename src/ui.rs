use crate::{
  layers::{Layer, Layers},
  layout::DocumentLayout,
  model::{render_size, visible_tiles, Cache, Region, RenderKey, Zoom},
  pdf::Raster,
  printing::{self, wide},
  worker::{Command, Event, Worker, READY},
};
use std::{
  cell::RefCell,
  path::PathBuf,
  ptr,
  sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
  },
};
use windows_sys::Win32::{
  Foundation::*,
  Graphics::{Dwm::*, Gdi::*},
  System::LibraryLoader::*,
  UI::{
    Controls::Dialogs::*, Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*,
    WindowsAndMessaging::*,
  },
};

const OPEN: usize = 10;
mod editor;
const PRINT: usize = 11;
const PREV: usize = 12;
const NEXT: usize = 13;
const MINUS: usize = 14;
const PLUS: usize = 15;
const FIT: usize = 16;
const WIDTH: usize = 17;
const ROTATE: usize = 18;
const PAGE_EDIT: usize = 19;
const PAGE_LIST: usize = 20;
const LAYER_LIST: usize = 21;
const RESET: usize = 22;
const CANCEL: usize = 23;
const TAB_PAGES: usize = 24;
const TAB_LAYERS: usize = 25;
const LAYER_NOTE: usize = 26;
const CONTINUOUS: usize = 27;
const VIEW_MODE: usize = 28;

type LayoutKey = (u64, i32, i32, Zoom, u32, i32, bool);
type ThumbContext = (u64, u32, i32, Vec<bool>);

#[derive(Clone, Copy)]
struct ViewAnchor {
  page: usize,
  fraction: (f64, f64),
  point: (i32, i32),
}

thread_local! { static APP: RefCell<Option<App>> = const { RefCell::new(None) }; }

struct App {
  editor: editor::Editor,
  hwnd: HWND,
  canvas: HWND,
  controls: Vec<(usize, HWND)>,
  font: HFONT,
  big_font: HFONT,
  dpi: u32,
  toolbar_height: i32,
  worker: Worker,
  path: Option<PathBuf>,
  sizes: Vec<(f64, f64)>,
  layers: Vec<Layer>,
  groups: Vec<Vec<lopdf::ObjectId>>,
  states: Vec<bool>,
  page: usize,
  zoom: Zoom,
  scale: f64,
  rotation: i32,
  generation: u64,
  ticket: u64,
  image: Option<Raster>,
  wanted: (i32, i32),
  scroll: (i32, i32),
  layer_tab: bool,
  layer_warning: bool,
  status: String,
  rendering: bool,
  printing: bool,
  closing: bool,
  cancel: Arc<AtomicBool>,
  smoke: Option<PathBuf>,
  smoke_stage: u8,
  probe: Option<PathBuf>,
  opened_at: std::time::Instant,
  first_frame_ms: Option<u128>,
  last_error: bool,
  print_result: Option<bool>,
  dialog_open: bool,
  pending_error: Option<String>,
  continuous: bool,
  document_layout: DocumentLayout,
  layout_key: Option<LayoutKey>,
  frames: Cache,
  details: Cache,
  detail_mode: bool,
  thumbnails: Cache,
  render_plan: Vec<RenderKey>,
  thumb_plan: Vec<RenderKey>,
  thumb_context: Option<ThumbContext>,
  thumb_ticket: u64,
  anchor: Option<ViewAnchor>,
}

fn with_app<T>(f: impl FnOnce(&mut App) -> T) -> Option<T> {
  APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

unsafe fn text(dc: HDC, rect: RECT, value: &str, color: u32, font: HFONT, flags: u32) {
  let old = SelectObject(dc, font);
  SetBkMode(dc, TRANSPARENT as i32);
  SetTextColor(dc, color);
  let value = wide(value);
  let mut r = rect;
  DrawTextW(dc, value.as_ptr(), (value.len() - 1) as i32, &mut r, flags);
  SelectObject(dc, old);
}

unsafe fn fill(dc: HDC, r: &RECT, color: u32) {
  let brush = CreateSolidBrush(color);
  FillRect(dc, r, brush);
  DeleteObject(brush);
}

impl App {
  fn unit(&self, n: i32) -> i32 {
    (n as f64 * self.dpi as f64 / 96.).round() as i32
  }
  fn control(&self, id: usize) -> HWND {
    self
      .controls
      .iter()
      .find(|(i, _)| *i == id)
      .map(|(_, h)| *h)
      .unwrap_or(ptr::null_mut())
  }
  unsafe fn create(&mut self, id: usize, class: &str, label: &str, style: u32) -> HWND {
    let hwnd = CreateWindowExW(
      if id == PAGE_EDIT { WS_EX_CLIENTEDGE } else { 0 },
      wide(class).as_ptr(),
      wide(label).as_ptr(),
      WS_CHILD
        | WS_VISIBLE
        | WS_TABSTOP
        | if id == PAGE_EDIT {
          style & !WS_BORDER
        } else {
          style
        },
      0,
      0,
      0,
      0,
      self.hwnd,
      id as _,
      GetModuleHandleW(ptr::null()),
      ptr::null(),
    );
    SendMessageW(hwnd, WM_SETFONT, self.font as usize, 1);
    self.controls.push((id, hwnd));
    hwnd
  }

  unsafe fn setup(&mut self) {
    self.set_fonts();
    for (id, label) in [
      (OPEN, "Открыть"),
      (PRINT, "Печать"),
      (PREV, "‹"),
      (NEXT, "›"),
      (MINUS, "−"),
      (PLUS, "+"),
      (FIT, "Страница"),
      (WIDTH, "По ширине"),
      (ROTATE, "Повернуть"),
      (RESET, "Сбросить слои"),
      (CANCEL, "Отмена печати"),
      (TAB_PAGES, "Страницы"),
      (TAB_LAYERS, "Слои"),
    ] {
      let style = if id == TAB_PAGES || id == TAB_LAYERS {
        BS_RADIOBUTTON | BS_PUSHLIKE
      } else {
        BS_PUSHBUTTON
      };
      self.create(id, "BUTTON", label, style as u32);
    }
    let continuous = self.create(
      CONTINUOUS,
      "BUTTON",
      "Прокрутка",
      (BS_AUTOCHECKBOX | BS_PUSHLIKE) as u32,
    );
    SendMessageW(continuous, BM_SETCHECK, BST_CHECKED as usize, 0);
    let mode = self.create(
      VIEW_MODE,
      "COMBOBOX",
      "Режим просмотра",
      CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
    );
    for label in ["Быстрый просмотр", "Чертёж — точные детали"] {
      SendMessageW(mode, CB_ADDSTRING, 0, wide(label).as_ptr() as isize);
    }
    SendMessageW(mode, CB_SETCURSEL, 0, 0);
    self.create(
      PAGE_EDIT,
      "EDIT",
      "1",
      (ES_CENTER | ES_NUMBER | ES_MULTILINE) as u32 | WS_BORDER,
    );
    self.create(
      PAGE_LIST,
      "LISTBOX",
      "Страницы документа",
      (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | LBS_OWNERDRAWFIXED | LBS_HASSTRINGS) as u32 | WS_VSCROLL,
    );
    self.create(
      LAYER_NOTE,
      "STATIC",
      "Откройте документ, чтобы увидеть его слои.",
      0,
    );
    let layers = self.create(
      LAYER_LIST,
      "SysListView32",
      "Слои документа",
      LVS_REPORT | LVS_NOCOLUMNHEADER | LVS_SINGLESEL | LVS_SHOWSELALWAYS,
    );
    SendMessageW(
      layers,
      LVM_SETEXTENDEDLISTVIEWSTYLE,
      0,
      (LVS_EX_CHECKBOXES | LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize,
    );
    let mut column: LVCOLUMNW = std::mem::zeroed();
    column.mask = LVCF_WIDTH;
    column.cx = self.unit(215);
    SendMessageW(layers, LVM_INSERTCOLUMNW, 0, &column as *const _ as isize);
    self.canvas = CreateWindowExW(
      0,
      wide("AstraPdfCanvas").as_ptr(),
      wide("Просмотр страницы").as_ptr(),
      WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_VSCROLL | WS_HSCROLL,
      0,
      0,
      1,
      1,
      self.hwnd,
      ptr::null_mut(),
      GetModuleHandleW(ptr::null()),
      ptr::null(),
    );
    DragAcceptFiles(self.hwnd, 1);
    self.layout();
    self.enable_controls();
    SetTimer(self.hwnd, 3, 150, None);
  }

  unsafe fn set_fonts(&mut self) {
    let font = CreateFontW(
      -self.unit(14),
      0,
      0,
      0,
      400,
      0,
      0,
      0,
      DEFAULT_CHARSET as u32,
      0,
      0,
      CLEARTYPE_QUALITY as u32,
      0,
      wide("Segoe UI").as_ptr(),
    );
    let big = CreateFontW(
      -self.unit(30),
      0,
      0,
      0,
      600,
      0,
      0,
      0,
      DEFAULT_CHARSET as u32,
      0,
      0,
      CLEARTYPE_QUALITY as u32,
      0,
      wide("Segoe UI").as_ptr(),
    );
    for (_, control) in &self.controls {
      SendMessageW(*control, WM_SETFONT, font as usize, 1);
    }
    if !self.font.is_null() {
      DeleteObject(self.font);
    }
    if !self.big_font.is_null() {
      DeleteObject(self.big_font);
    }
    self.font = font;
    self.big_font = big;
  }

  unsafe fn place(&self, id: usize, x: i32, y: i32, w: i32, h: i32) {
    MoveWindow(
      self.control(id),
      self.unit(x),
      self.unit(y),
      self.unit(w),
      self.unit(h),
      1,
    );
  }

  unsafe fn layout(&mut self) {
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(self.hwnd, &mut r);
    let width = (r.right as f64 * 96. / self.dpi as f64) as i32;
    let height = (r.bottom as f64 * 96. / self.dpi as f64) as i32;
    let wrapped = width < if self.printing { 1120 } else { 980 };
    let compact_print = wrapped && width < 700 && self.printing;
    let top = if compact_print {
      152
    } else if wrapped {
      108
    } else {
      64
    };
    self.toolbar_height = top;
    for (id, x, w) in [
      (OPEN, 16, 90),
      (PRINT, 114, 82),
      (PREV, 221, 32),
      (PAGE_EDIT, 259, 52),
      (NEXT, 317, 32),
      (MINUS, 381, 32),
      (PLUS, 419, 32),
      (FIT, 476, 94),
      (WIDTH, 578, 112),
      (ROTATE, 698, 112),
    ] {
      self.place(id, x, 14, w, 32);
    }
    self.place(CONTINUOUS, 824, 14, 134, 32);
    self.place(CANCEL, 970, 14, 138, 32);
    if wrapped {
      self.place(FIT, 16, 58, 94, 32);
      self.place(WIDTH, 118, 58, 112, 32);
      self.place(ROTATE, 238, 58, 112, 32);
      self.place(CONTINUOUS, 358, 58, 142, 32);
      self.place(
        CANCEL,
        if compact_print { 16 } else { 508 },
        if compact_print { 102 } else { 58 },
        138,
        32,
      );
    }
    self.place(TAB_PAGES, 12, top + 13, 107, 32);
    self.place(TAB_LAYERS, 124, top + 13, 104, 32);
    self.place(VIEW_MODE, 12, top + 54, 216, 120);
    self.place(PAGE_LIST, 12, top + 96, 216, (height - top - 136).max(30));
    SendMessageW(
      self.control(PAGE_LIST),
      LB_SETITEMHEIGHT,
      0,
      self.unit(184) as isize,
    );
    self.place(LAYER_LIST, 12, top + 96, 216, (height - top - 186).max(30));
    self.place(LAYER_NOTE, 20, top + 104, 200, 100);
    self.place(RESET, 12, height - 78, 216, 32);
    MoveWindow(
      self.canvas,
      self.unit(240),
      self.unit(top),
      self.unit((width - 240).max(1)),
      self.unit((height - top - 32).max(1)),
      1,
    );
    ShowWindow(
      self.control(PAGE_LIST),
      if self.layer_tab { SW_HIDE } else { SW_SHOW },
    );
    ShowWindow(
      self.control(LAYER_LIST),
      if self.layer_tab && !self.layers.is_empty() {
        SW_SHOW
      } else {
        SW_HIDE
      },
    );
    ShowWindow(
      self.control(LAYER_NOTE),
      if self.layer_tab && self.layers.is_empty() {
        SW_SHOW
      } else {
        SW_HIDE
      },
    );
    ShowWindow(
      self.control(RESET),
      if self.layer_tab { SW_SHOW } else { SW_HIDE },
    );
    SendMessageW(
      self.control(TAB_PAGES),
      BM_SETCHECK,
      if self.layer_tab {
        BST_UNCHECKED
      } else {
        BST_CHECKED
      } as usize,
      0,
    );
    SendMessageW(
      self.control(TAB_LAYERS),
      BM_SETCHECK,
      if self.layer_tab {
        BST_CHECKED
      } else {
        BST_UNCHECKED
      } as usize,
      0,
    );
    ShowWindow(
      self.control(CANCEL),
      if self.printing { SW_SHOW } else { SW_HIDE },
    );
    self.center_page_number();
    self.update_scroll();
    InvalidateRect(self.hwnd, ptr::null(), 0);
    SetTimer(self.hwnd, 1, 100, None);
  }

  unsafe fn center_page_number(&self) {
    let edit = self.control(PAGE_EDIT);
    let mut rect: RECT = std::mem::zeroed();
    GetClientRect(edit, &mut rect);
    let dc = GetDC(edit);
    let old = SelectObject(dc, self.font);
    let mut metrics: TEXTMETRICW = std::mem::zeroed();
    GetTextMetricsW(dc, &mut metrics);
    SelectObject(dc, old);
    ReleaseDC(edit, dc);
    rect.top = ((rect.bottom - metrics.tmHeight) / 2).max(0);
    rect.left = self.unit(2);
    rect.right -= self.unit(2);
    rect.bottom = rect.top + metrics.tmHeight;
    SendMessageW(edit, EM_SETRECTNP, 0, &rect as *const _ as isize);
    InvalidateRect(edit, ptr::null(), 1);
  }

  unsafe fn enable_controls(&self) {
    let loaded = !self.sizes.is_empty();
    for (id, h) in &self.controls {
      let enabled = match *id {
        OPEN => !self.printing,
        TAB_PAGES | TAB_LAYERS | LAYER_NOTE => true,
        CANCEL => self.printing || self.editor.busy,
        PREV => loaded && self.page > 0,
        NEXT => loaded && self.page + 1 < self.sizes.len(),
        PRINT => loaded && !self.printing,
        RESET | LAYER_LIST => !self.layers.is_empty() && !self.printing,
        _ => loaded,
      };
      EnableWindow(*h, (enabled && (!self.editor.busy || *id == CANCEL)) as i32);
    }
  }

  unsafe fn choose_file(hwnd: HWND) -> Result<Option<PathBuf>, String> {
    let mut path = vec![0u16; 32768];
    let filter = wide("Документы PDF\0*.pdf\0Все файлы\0*.*\0");
    let mut ofn: OPENFILENAMEW = std::mem::zeroed();
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = hwnd;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = path.as_mut_ptr();
    ofn.nMaxFile = path.len() as u32;
    ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_EXPLORER | OFN_NOCHANGEDIR;
    if GetOpenFileNameW(&mut ofn) != 0 {
      use std::os::windows::ffi::OsStringExt;
      let end = path.iter().position(|v| *v == 0).unwrap_or(path.len());
      Ok(Some(PathBuf::from(std::ffi::OsString::from_wide(
        &path[..end],
      ))))
    } else {
      let error = CommDlgExtendedError();
      if error != 0 {
        Err(format!("Ошибка диалога открытия: {error}"))
      } else {
        Ok(None)
      }
    }
  }

  unsafe fn open(&mut self, path: PathBuf) {
    self.editor.reset();
    self.opened_at = std::time::Instant::now();
    self.first_frame_ms = None;
    self.last_error = false;
    self.generation += 1;
    self.ticket += 1;
    self.worker.latest.store(self.ticket, Ordering::Relaxed);
    self.path = Some(path.clone());
    self.sizes.clear();
    self.layers.clear();
    self.states.clear();
    self.groups.clear();
    self.layer_warning = false;
    SetWindowTextW(
      self.control(LAYER_NOTE),
      wide("Читаю слои документа…").as_ptr(),
    );
    self.page = 0;
    self.rotation = 0;
    self.zoom = Zoom::FitPage;
    self.scroll = (0, 0);
    self.image = None;
    self.frames = Cache::with_count(96 * 1024 * 1024, 64);
    self.details = Cache::with_count(64 * 1024 * 1024, 64);
    self.thumbnails = Cache::with_count(16 * 1024 * 1024, 48);
    self.document_layout = DocumentLayout::default();
    self.layout_key = None;
    self.render_plan.clear();
    self.thumb_plan.clear();
    self.thumb_context = None;
    self.anchor = None;
    self.status = "Открываю документ…".into();
    self.rendering = true;
    SendMessageW(self.control(PAGE_LIST), LB_RESETCONTENT, 0, 0);
    SendMessageW(self.control(LAYER_LIST), LVM_DELETEALLITEMS, 0, 0);
    let title = format!(
      "{} — {}",
      path.file_name().unwrap_or_default().to_string_lossy(),
      crate::version::title()
    );
    SetWindowTextW(self.hwnd, wide(&title).as_ptr());
    if self
      .worker
      .sender
      .send(Command::Open {
        path,
        generation: self.generation,
      })
      .is_err()
    {
      self.error("Движок PDF недоступен. Проверьте наличие pdfium.dll рядом с программой.".into());
    }
    self.enable_controls();
    self.layout();
    InvalidateRect(self.hwnd, ptr::null(), 0);
    InvalidateRect(self.canvas, ptr::null(), 0);
  }

  unsafe fn viewport(&self) -> (i32, i32) {
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(self.canvas, &mut r);
    (r.right, r.bottom)
  }

  unsafe fn page_origin(&self, page: usize) -> (i32, i32) {
    let view = self.viewport();
    let p = &self.document_layout.pages[page];
    if self.continuous {
      (self.document_layout.x(page, view.0), p.top)
    } else {
      (
        ((view.0 - p.width) / 2).max(self.document_layout.margin),
        ((view.1 - p.height) / 2).max(self.document_layout.margin),
      )
    }
  }

  fn page_key(&self, page: usize) -> RenderKey {
    let p = &self.document_layout.pages[page];
    // Ограничение одного изображения оставляет память для соседних страниц.
    let pixels = p.width as f64 * p.height as f64;
    let budget = if self.detail_mode && pixels > 8_000_000. {
      2_000_000.
    } else {
      8_000_000.
    };
    let downscale = (budget / pixels).sqrt().min(1.);
    RenderKey {
      page,
      width: (p.width as f64 * downscale).round().max(1.) as i32,
      height: (p.height as f64 * downscale).round().max(1.) as i32,
      rotation: self.rotation,
      states: self.states.clone(),
      region: None,
    }
  }

  fn cached(&self, key: &RenderKey) -> Option<&Raster> {
    if key.region.is_some() {
      self.details.peek(key)
    } else {
      self.frames.peek(key)
    }
  }

  unsafe fn detail_keys(&self, page: usize) -> Vec<RenderKey> {
    let p = &self.document_layout.pages[page];
    if !self.detail_mode || i64::from(p.width) * i64::from(p.height) <= 8_000_000 {
      return vec![];
    }
    let origin = self.page_origin(page);
    let view = self.viewport();
    visible_tiles(
      (p.width, p.height),
      Region {
        x: self.scroll.0 - origin.0,
        y: self.scroll.1 - origin.1,
        width: view.0,
        height: view.1,
      },
    )
    .into_iter()
    .map(|region| RenderKey {
      page,
      width: p.width,
      height: p.height,
      rotation: self.rotation,
      states: self.states.clone(),
      region: Some(region),
    })
    .collect()
  }

  unsafe fn remember_anchor(&mut self, point: Option<(i32, i32)>) {
    if self.document_layout.pages.is_empty() {
      return;
    }
    let view = self.viewport();
    let point = point.unwrap_or((view.0 / 2, view.1 / 2));
    let page = if self.continuous {
      self.document_layout.active(self.scroll.1 + point.1, 1)
    } else {
      self.page
    };
    let p = &self.document_layout.pages[page];
    let origin = self.page_origin(page);
    self.anchor = Some(ViewAnchor {
      page,
      fraction: (
        (self.scroll.0 + point.0 - origin.0) as f64 / p.width as f64,
        (self.scroll.1 + point.1 - origin.1) as f64 / p.height as f64,
      ),
      point,
    });
  }

  unsafe fn reflow(&mut self) {
    if self.sizes.is_empty() {
      return;
    }
    let view = self.viewport();
    let key = (
      self.generation,
      view.0,
      view.1,
      self.zoom,
      self.dpi,
      self.rotation,
      self.detail_mode,
    );
    if self.layout_key != Some(key) {
      if self.anchor.is_none() && !self.document_layout.pages.is_empty() {
        self.remember_anchor(None);
      }
      self.document_layout = if self.detail_mode {
        DocumentLayout::with_detail(&self.sizes, view, self.zoom, self.dpi, self.rotation, true)
      } else {
        DocumentLayout::new(&self.sizes, view, self.zoom, self.dpi, self.rotation)
      };
      self.layout_key = Some(key);
    }
    if let Some(anchor) = self.anchor.take().filter(|a| a.page < self.sizes.len()) {
      self.page = anchor.page;
      let origin = self.page_origin(anchor.page);
      let p = &self.document_layout.pages[anchor.page];
      self.scroll = (
        origin.0 + (anchor.fraction.0 * p.width as f64).round() as i32 - anchor.point.0,
        origin.1 + (anchor.fraction.1 * p.height as f64).round() as i32 - anchor.point.1,
      );
    }
    let p = &self.document_layout.pages[self.page];
    self.wanted = (p.width, p.height);
    self.scale = p.scale;
    self.update_scroll();
  }

  unsafe fn visible_pages(&self) -> Vec<usize> {
    if self.document_layout.pages.is_empty() {
      return vec![];
    }
    if self.continuous {
      self
        .document_layout
        .visible(self.scroll.1, self.viewport().1)
        .collect()
    } else {
      vec![self.page]
    }
  }

  unsafe fn sync_page_controls(&self) {
    if GetFocus() != self.control(PAGE_EDIT) {
      SetWindowTextW(
        self.control(PAGE_EDIT),
        wide(&(self.page + 1).to_string()).as_ptr(),
      );
    }
    let list = self.control(PAGE_LIST);
    if SendMessageW(list, LB_GETCURSEL, 0, 0) != self.page as isize {
      SendMessageW(list, LB_SETCURSEL, self.page, 0);
      // Синхронное рисование пропускается при занятом состоянии окна; перерисуем после сообщения.
      InvalidateRect(list, ptr::null(), 0);
    }
    self.enable_controls();
  }

  unsafe fn request(&mut self) {
    if self.sizes.is_empty() {
      return;
    }
    self.reflow();
    let current_key = self.page_key(self.page);
    self.image = self
      .frames
      .peek(&current_key)
      .or_else(|| {
        self
          .detail_mode
          .then(|| self.frames.preview(&current_key))
          .flatten()
      })
      .cloned();
    let mut pages = self.visible_pages();
    pages.sort_by_key(|p| p.abs_diff(self.page));
    let mut plan = vec![];
    for page in pages {
      let details = self.detail_keys(page);
      let key = self.page_key(page);
      // Готовый обзор уже сохраняет изображение: сначала уточняем видимые детали.
      if details.is_empty() || self.frames.preview(&key).is_none() {
        plan.push(key);
      }
      plan.extend(details);
    }
    self.rendering = plan.iter().any(|key| self.cached(key).is_none());
    if plan != self.render_plan {
      self.ticket += 1;
      self.worker.latest.store(self.ticket, Ordering::Relaxed);
      let missing = plan
        .iter()
        .filter(|key| self.cached(key).is_none())
        .cloned()
        .collect();
      self.render_plan = plan;
      if self
        .worker
        .sender
        .render_batch(self.generation, self.ticket, missing, false)
        .is_err()
      {
        self.error("Рабочий поток PDF остановлен.".into());
      }
    }
    self.sync_page_controls();
    self.refresh_status();
    self.request_thumbnails();
    self.request_mosaics();
    InvalidateRect(self.canvas, ptr::null(), 0);
  }

  unsafe fn refresh_status(&mut self) {
    if self.sizes.is_empty() || self.printing || self.last_error {
      return;
    }
    self.status = format!(
      "Страница {} из {}   ·   {:.0}%   ·   {}",
      self.page + 1,
      self.sizes.len(),
      self.scale * 100.,
      if self.continuous {
        "Непрерывная прокрутка"
      } else {
        "Одна страница"
      }
    );
    if self.detail_mode {
      self.status.push_str(if self.rendering {
        "   ·   Уточняю детали…"
      } else {
        "   ·   Чертёж"
      });
    }
    if self.layer_warning {
      self.status.push_str("   ·   Слои недоступны");
    }
    InvalidateRect(self.hwnd, ptr::null(), 0);
  }

  unsafe fn request_thumbnails(&mut self) {
    if self.sizes.is_empty() || self.layer_tab {
      return;
    }
    let context = (
      self.generation,
      self.dpi,
      self.rotation,
      self.states.clone(),
    );
    if self.thumb_context.as_ref() != Some(&context) {
      self.thumb_context = Some(context);
      self.thumbnails = Cache::with_count(16 * 1024 * 1024, 48);
      self.thumb_plan.clear();
    }
    let list = self.control(PAGE_LIST);
    let top = SendMessageW(list, LB_GETTOPINDEX, 0, 0).max(0) as usize;
    let mut rect: RECT = std::mem::zeroed();
    GetClientRect(list, &mut rect);
    let count = (rect.bottom / self.unit(184).max(1) + 2).max(1) as usize;
    let plan: Vec<_> = (top..(top + count).min(self.sizes.len()))
      .map(|page| self.thumbnail_key(page))
      .collect();
    if plan != self.thumb_plan {
      self.thumb_ticket += 1;
      self
        .worker
        .latest_thumbnail
        .store(self.thumb_ticket, Ordering::Relaxed);
      let missing = plan
        .iter()
        .filter(|key| self.thumbnails.peek(key).is_none())
        .cloned()
        .collect();
      self.thumb_plan = plan;
      let _ = self
        .worker
        .sender
        .render_batch(self.generation, self.thumb_ticket, missing, true);
    }
  }

  fn thumbnail_key(&self, page: usize) -> RenderKey {
    let (width, height, _) = render_size(
      self.sizes[page],
      (self.unit(168) + 40, self.unit(142) + 40),
      Zoom::FitPage,
      self.dpi as f64,
      self.rotation,
    );
    RenderKey {
      page,
      width,
      height,
      rotation: self.rotation,
      states: self.states.clone(),
      region: None,
    }
  }

  unsafe fn update_scroll(&mut self) {
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(self.canvas, &mut r);
    let margin = self.document_layout.margin.max(self.unit(20));
    let size = if self.continuous && !self.document_layout.pages.is_empty() {
      (self.document_layout.width, self.document_layout.height)
    } else {
      (self.wanted.0 + 2 * margin, self.wanted.1 + 2 * margin)
    };
    self.scroll.0 = self.scroll.0.clamp(0, (size.0 - r.right).max(0));
    self.scroll.1 = self.scroll.1.clamp(0, (size.1 - r.bottom).max(0));
    for (bar, size, page, pos) in [
      (SB_HORZ, size.0, r.right, self.scroll.0),
      (SB_VERT, size.1, r.bottom, self.scroll.1),
    ] {
      let info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
        nMin: 0,
        nMax: size.max(1) - 1,
        nPage: page.max(1) as u32,
        nPos: pos,
        nTrackPos: 0,
      };
      SetScrollInfo(self.canvas, bar, &info, 1);
    }
  }

  unsafe fn navigate(&mut self, index: usize) {
    if index < self.sizes.len() {
      self.page = index;
      self.reflow();
      self.scroll = (
        0,
        if self.continuous {
          self.document_layout.pages[index].top - self.document_layout.margin
        } else {
          0
        },
      );
      self.request();
    }
  }
  unsafe fn zoom_by(&mut self, factor: f64) {
    self.zoom_at(factor, None);
  }

  unsafe fn zoom_at(&mut self, factor: f64, point: Option<(i32, i32)>) {
    self.remember_anchor(point);
    self.zoom =
      Zoom::Scale((self.scale * factor).clamp(0.1, if self.detail_mode { 64. } else { 8. }));
    self.reflow();
    self.rendering = true;
    self.refresh_status();
    InvalidateRect(self.canvas, ptr::null(), 0);
    SetTimer(self.hwnd, 1, 65, None);
  }
  unsafe fn refresh_layers(&self) {
    let hwnd = self.control(LAYER_LIST);
    SendMessageW(hwnd, WM_SETREDRAW, 0, 0);
    SendMessageW(hwnd, LVM_DELETEALLITEMS, 0, 0);
    for (i, layer) in self.layers.iter().enumerate() {
      let label = if layer.locked {
        format!("{} (закреплён)", layer.name)
      } else {
        layer.name.clone()
      };
      let mut label = wide(&label);
      let mut item: LVITEMW = std::mem::zeroed();
      item.mask = LVIF_TEXT | LVIF_STATE;
      item.iItem = i as i32;
      item.pszText = label.as_mut_ptr();
      item.stateMask = LVIS_STATEIMAGEMASK;
      item.state = if self.states[i] { 2 << 12 } else { 1 << 12 };
      SendMessageW(hwnd, LVM_INSERTITEMW, 0, &item as *const _ as isize);
    }
    SendMessageW(hwnd, WM_SETREDRAW, 1, 0);
    self.sync_layer_checks();
    InvalidateRect(hwnd, ptr::null(), 1);
  }

  unsafe fn sync_layer_checks(&self) {
    for (i, state) in self.states.iter().enumerate() {
      let mut item: LVITEMW = std::mem::zeroed();
      item.stateMask = LVIS_STATEIMAGEMASK;
      item.state = if *state { 2 << 12 } else { 1 << 12 };
      SendMessageW(
        self.control(LAYER_LIST),
        LVM_SETITEMSTATE,
        i,
        &item as *const _ as isize,
      );
    }
  }

  unsafe fn action(&mut self, id: usize) {
    if self.editor.busy && id != CANCEL {
      return;
    }
    if (editor::PAGES..=editor::PIXEL_LARGE).contains(&id) {
      PostMessageW(self.hwnd, WM_APP + 21, id, 0);
      return;
    }
    if id == PRINT && !self.editor.masks.is_empty() {
      self.editor.notice = Some("Есть несохранённые области скрытия. Сначала сохраните очищенный PDF и откройте его для печати.".into());
      PostMessageW(self.hwnd, WM_APP + 23, 0, 0);
      return;
    }
    match id {
      OPEN if !self.dialog_open && !self.printing => {
        PostMessageW(self.hwnd, WM_APP + 12, 0, 0);
      }
      PREV => self.navigate(self.page.saturating_sub(1)),
      NEXT => self.navigate(self.page + 1),
      MINUS => self.zoom_by(1. / 1.1),
      PLUS => self.zoom_by(1.1),
      VIEW_MODE => {
        self.remember_anchor(None);
        self.detail_mode = SendMessageW(self.control(VIEW_MODE), CB_GETCURSEL, 0, 0) == 1;
        if let Zoom::Scale(s) = self.zoom {
          self.zoom = Zoom::Scale(s.min(if self.detail_mode { 64. } else { 8. }));
        }
        self.render_plan.clear();
        self.request();
      }
      FIT => {
        self.zoom = Zoom::FitPage;
        self.anchor = Some(ViewAnchor {
          page: self.page,
          fraction: (0., 0.),
          point: (self.unit(20), self.unit(20)),
        });
        self.request();
      }
      WIDTH => {
        self.zoom = Zoom::FitWidth;
        self.anchor = Some(ViewAnchor {
          page: self.page,
          fraction: (0., 0.),
          point: (self.unit(20), self.unit(20)),
        });
        self.request();
      }
      ROTATE => {
        self.remember_anchor(None);
        self.rotation = (self.rotation + 1) % 4;
        self.request();
      }
      CONTINUOUS => {
        self.continuous = !self.continuous;
        SendMessageW(
          self.control(CONTINUOUS),
          BM_SETCHECK,
          if self.continuous {
            BST_CHECKED
          } else {
            BST_UNCHECKED
          } as usize,
          0,
        );
        self.navigate(self.page);
      }
      TAB_PAGES | TAB_LAYERS => {
        self.layer_tab = id == TAB_LAYERS;
        self.layout();
        self.request_thumbnails();
        self.request_mosaics();
      }
      RESET if !self.printing => {
        self.states = self.layers.iter().map(|l| l.visible).collect();
        self.sync_layer_checks();
        self.request();
      }
      CANCEL => {
        self.cancel.store(true, Ordering::Relaxed);
        self.status = "Отменяю операцию…".into();
      }
      PRINT if !self.sizes.is_empty() && !self.printing && !self.dialog_open => {
        PostMessageW(self.hwnd, WM_APP + 13, 0, 0);
      }
      _ => (),
    }
    InvalidateRect(self.hwnd, ptr::null(), 0);
    PostMessageW(self.hwnd, READY, 0, 0);
  }

  unsafe fn error(&mut self, message: String) {
    self.last_error = true;
    self.rendering = false;
    self.render_plan.clear();
    self.status = message.clone();
    if self.smoke.is_none() {
      self.pending_error = Some(message);
      PostMessageW(self.hwnd, WM_APP + 14, 0, 0);
    } else if let Some(path) = &self.smoke {
      let _ = std::fs::write(path.with_extension("error.txt"), message);
      PostQuitMessage(1);
    }
  }

  unsafe fn events(&mut self) {
    while let Ok(event) = self.worker.receiver.try_recv() {
      match event {
        Event::Mosaic {
          generation,
          key,
          result,
        } if generation == self.generation => {
          match result {
            Ok(image) => {
              self
                .editor
                .grids
                .get_or_insert_with(|| Cache::with_count(16 * 1024 * 1024, 32))
                .put(key, image);
            }
            Err(message) => {
              self.editor.notice = Some(format!("Предпросмотр пикселизации: {message}"));
              PostMessageW(self.hwnd, WM_APP + 23, 0, 0);
            }
          }
          InvalidateRect(self.canvas, ptr::null(), 0);
        }
        Event::Loaded {
          fingerprint,
          generation,
          sizes,
          millis,
        } if generation == self.generation => {
          self.editor.fingerprint = Some(fingerprint);
          self.sizes = sizes;
          self.status = format!("Открыто за {millis} мс · анализ слоёв…");
          let list = self.control(PAGE_LIST);
          SendMessageW(list, WM_SETREDRAW, 0, 0);
          for i in 0..self.sizes.len() {
            SendMessageW(
              list,
              LB_ADDSTRING,
              0,
              wide(&format!("    Страница {}", i + 1)).as_ptr() as isize,
            );
          }
          SendMessageW(list, WM_SETREDRAW, 1, 0);
          self.request();
        }
        Event::Layers {
          generation,
          items,
          groups,
          warning,
        } if generation == self.generation => {
          self.states = items.iter().map(|l| l.visible).collect();
          self.layers = items;
          self.groups = groups;
          self.refresh_layers();
          self.layer_warning = warning.is_some();
          let note =
            warning.unwrap_or_else(|| "В этом документе нет управляемых слоёв PDF.".into());
          SetWindowTextW(self.control(LAYER_NOTE), wide(&note).as_ptr());
          self.layout();
          self.request();
        }
        Event::Rendered {
          generation,
          ticket,
          key,
          image,
        } if generation == self.generation && ticket == self.ticket => {
          self
            .first_frame_ms
            .get_or_insert(self.opened_at.elapsed().as_millis());
          if key.region.is_some() {
            self.details.put(key, image);
          } else {
            self.frames.put(key, image);
          }
          let current_key = self.page_key(self.page);
          self.image = self
            .frames
            .peek(&current_key)
            .or_else(|| {
              self
                .detail_mode
                .then(|| self.frames.preview(&current_key))
                .flatten()
            })
            .cloned();
          self.rendering = self
            .render_plan
            .iter()
            .any(|key| self.cached(key).is_none());
          self.refresh_status();
          InvalidateRect(self.canvas, ptr::null(), 0);
          if self.smoke.is_some() {
            SetTimer(self.hwnd, 2, 350, None);
          }
        }
        Event::Thumbnail {
          generation,
          ticket,
          key,
          image,
        } if generation == self.generation && ticket == self.thumb_ticket => {
          if let Some(image) = image {
            self.thumbnails.put(key, image);
          }
          InvalidateRect(self.control(PAGE_LIST), ptr::null(), 0);
        }
        Event::Error {
          generation,
          message,
        } if generation == self.generation || generation == 0 => self.error(message),
        Event::PrintProgress { page, total } => {
          self.status = format!("Печать: страница {page} из {total}…");
        }
        Event::Printed(result) => {
          self.print_result = Some(result.is_ok());
          self.printing = false;
          if self.smoke.is_some() && self.smoke_stage == 6 {
            if let Err(e) = &result {
              self.error(e.clone());
              return;
            }
            SetTimer(self.hwnd, 2, 200, None);
          }
          self.status = match result {
            Ok(()) => "Документ передан в очередь печати.".into(),
            Err(e) => e,
          };
          self.layout();
          self.enable_controls();
          if self.closing {
            PostQuitMessage(0);
          }
        }
        _ => (),
      }
    }
    InvalidateRect(self.hwnd, ptr::null(), 0);
  }

  unsafe fn paint(&self, target: Option<HDC>) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let dc = target.unwrap_or_else(|| BeginPaint(self.hwnd, &mut ps));
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(self.hwnd, &mut r);
    fill(dc, &r, 0x00fafafa);
    fill(
      dc,
      &RECT {
        left: 0,
        top: self.unit(self.toolbar_height - 1),
        right: r.right,
        bottom: self.unit(self.toolbar_height),
      },
      0x00dedede,
    );
    fill(
      dc,
      &RECT {
        left: self.unit(239),
        top: self.unit(self.toolbar_height),
        right: self.unit(240),
        bottom: r.bottom - self.unit(32),
      },
      0x00dedede,
    );
    text(
      dc,
      RECT {
        left: self.unit(16),
        top: r.bottom - self.unit(30),
        right: r.right - self.unit(12),
        bottom: r.bottom,
      },
      &self.status,
      0x00585858,
      self.font,
      DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
    );
    if target.is_none() {
      EndPaint(self.hwnd, &ps);
    }
  }

  unsafe fn paint_canvas(&self, target: Option<HDC>) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let dc = target.unwrap_or_else(|| BeginPaint(self.canvas, &mut ps));
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(self.canvas, &mut r);
    let mem = CreateCompatibleDC(dc);
    let bitmap = CreateCompatibleBitmap(dc, r.right.max(1), r.bottom.max(1));
    let old = SelectObject(mem, bitmap);
    fill(mem, &r, 0x00ece9e6);
    if !self.document_layout.pages.is_empty() {
      for page in self.visible_pages() {
        let p = &self.document_layout.pages[page];
        let origin = self.page_origin(page);
        let (x, y) = (origin.0 - self.scroll.0, origin.1 - self.scroll.1);
        fill(
          mem,
          &RECT {
            left: x + 3,
            top: y + 4,
            right: x + p.width + 3,
            bottom: y + p.height + 4,
          },
          0x00d3ceca,
        );
        let key = self.page_key(page);
        let preview = self
          .frames
          .peek(&key)
          .or_else(|| self.frames.preview(&key))
          .or_else(|| self.thumbnails.peek(&self.thumbnail_key(page)));
        if let Some(image) = preview {
          printing::draw_raster(mem, image, x, y, p.width, p.height);
        } else {
          fill(
            mem,
            &RECT {
              left: x,
              top: y,
              right: x + p.width,
              bottom: y + p.height,
            },
            0x00ffffff,
          );
          text(
            mem,
            RECT {
              left: x,
              top: y + p.height / 2 - self.unit(16),
              right: x + p.width,
              bottom: y + p.height / 2 + self.unit(16),
            },
            &format!("Страница {}", page + 1),
            0x00808080,
            self.font,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
          );
        }
        for key in self.detail_keys(page) {
          if let Some(image) = self.details.peek(&key) {
            let region = key.region.unwrap();
            printing::draw_raster(
              mem,
              image,
              x + region.x,
              y + region.y,
              region.width,
              region.height,
            );
          }
        }
      }
    } else {
      let middle = r.bottom / 2;
      text(
        mem,
        RECT {
          left: 30,
          top: middle - self.unit(60),
          right: r.right - 30,
          bottom: middle,
        },
        if self.rendering {
          "Загрузка…"
        } else {
          "Astra PDF"
        },
        0x00483a30,
        self.big_font,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
      );
      text(
        mem,
        RECT {
          left: 30,
          top: middle + self.unit(5),
          right: r.right - 30,
          bottom: middle + self.unit(75),
        },
        if self.rendering {
          "Подготавливаю страницу"
        } else {
          "Откройте PDF или перетащите его в окно\nCtrl+O — открыть документ"
        },
        0x00786858,
        self.font,
        DT_CENTER,
      );
    }
    self.editor_paint(mem);
    BitBlt(dc, 0, 0, r.right, r.bottom, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old);
    DeleteObject(bitmap);
    DeleteDC(mem);
    if target.is_none() {
      EndPaint(self.canvas, &ps);
    }
  }

  unsafe fn paint_thumbnail(&self, item: &DRAWITEMSTRUCT) {
    if item.itemID == u32::MAX || item.itemID as usize >= self.sizes.len() {
      return;
    }
    let page = item.itemID as usize;
    let selected = item.itemState & ODS_SELECTED != 0;
    let rect = item.rcItem;
    fill(
      item.hDC,
      &rect,
      if selected { 0x00f4efe3 } else { 0x00fafafa },
    );
    let key = self.thumbnail_key(page);
    let x = rect.left + (rect.right - rect.left - key.width) / 2;
    let y = rect.top + self.unit(10) + (self.unit(142) - key.height) / 2;
    let border = self.unit(if selected { 2 } else { 1 });
    fill(
      item.hDC,
      &RECT {
        left: x - border,
        top: y - border,
        right: x + key.width + border,
        bottom: y + key.height + border,
      },
      if selected { 0x0095711c } else { 0x00d8d3cc },
    );
    if let Some(image) = self.thumbnails.peek(&key) {
      printing::draw_raster(item.hDC, image, x, y, key.width, key.height);
    } else {
      fill(
        item.hDC,
        &RECT {
          left: x,
          top: y,
          right: x + key.width,
          bottom: y + key.height,
        },
        0x00ffffff,
      );
    }
    text(
      item.hDC,
      RECT {
        left: rect.left,
        top: rect.top + self.unit(156),
        right: rect.right,
        bottom: rect.bottom - self.unit(6),
      },
      &format!("{}", page + 1),
      if selected { 0x00604c12 } else { 0x00606060 },
      self.font,
      DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );
    if item.itemState & ODS_FOCUS != 0 {
      DrawFocusRect(item.hDC, &rect);
    }
  }

  unsafe fn scrolled(&mut self) {
    self.update_scroll();
    if self.continuous && !self.document_layout.pages.is_empty() {
      self.page = self
        .document_layout
        .active(self.scroll.1, self.viewport().1);
    }
    self.request();
    InvalidateRect(self.canvas, ptr::null(), 0);
  }

  unsafe fn scroll_message(&mut self, horizontal: bool, code: i32) {
    let bar = if horizontal { SB_HORZ } else { SB_VERT };
    let mut info: SCROLLINFO = std::mem::zeroed();
    info.cbSize = std::mem::size_of::<SCROLLINFO>() as u32;
    info.fMask = SIF_ALL;
    GetScrollInfo(self.canvas, bar, &mut info);
    let pos = match code {
      SB_LINEUP => info.nPos - self.unit(40),
      SB_LINEDOWN => info.nPos + self.unit(40),
      SB_PAGEUP => info.nPos - info.nPage as i32,
      SB_PAGEDOWN => info.nPos + info.nPage as i32,
      SB_THUMBTRACK | SB_THUMBPOSITION => info.nTrackPos,
      SB_TOP => 0,
      SB_BOTTOM => info.nMax,
      _ => info.nPos,
    };
    if horizontal {
      self.scroll.0 = pos;
    } else {
      self.scroll.1 = pos;
    }
    self.scrolled();
  }

  unsafe fn smoke_step(&mut self) {
    if self.rendering || self.printing {
      return;
    }
    KillTimer(self.hwnd, 2);
    let path = self.smoke.clone().unwrap();
    match self.smoke_stage {
      0 => {
        self.layer_tab = true;
        self.layout();
        self.smoke_stage = 1;
        SetTimer(self.hwnd, 2, 500, None);
      }
      1 => {
        for (i, state) in self.states.iter().enumerate() {
          let actual = SendMessageW(
            self.control(LAYER_LIST),
            LVM_GETITEMSTATE,
            i,
            LVIS_STATEIMAGEMASK as isize,
          );
          if (actual == 2 << 12) != *state {
            self.error("Состояние флажка слоя не совпадает с документом.".into());
            return;
          }
        }
        // Отложенный снимок выполняется без удержания заимствования состояния окна.
        PostMessageW(self.hwnd, WM_APP + 3, 0, 0);
        self.smoke_stage = 2;
      }
      2 => {
        if !self.layers.is_empty() {
          PostMessageW(self.hwnd, WM_APP + 5, 0, 0);
        } else {
          self.request();
        }
        self.smoke_stage = 3;
      }
      3 => {
        self.navigate(1);
        self.smoke_stage = 4;
      }
      4 => {
        self.action(ROTATE);
        self.smoke_stage = 5;
      }
      5 if std::env::var_os("ASTRA_SMOKE_PRINT").is_some() => {
        if self.page != 1 || self.rotation != 1 || (!self.states.is_empty() && self.states[0]) {
          self.error("Проверка перехода, поворота или выключения слоя не прошла.".into());
          return;
        }
        let dc = CreateDCW(
          wide("WINSPOOL").as_ptr(),
          wide("Microsoft Print to PDF").as_ptr(),
          ptr::null(),
          ptr::null(),
        );
        if dc.is_null() {
          self.error("Недоступен Microsoft Print to PDF.".into());
          return;
        }
        let job = printing::PrintJob {
          dc: dc as usize,
          first: 1,
          last: (self.sizes.len() - 1).min(2),
          title: "Astra PDF — проверка слоёв и диапазона".into(),
          output: Some(
            path
              .with_extension("print.pdf")
              .to_string_lossy()
              .into_owned(),
          ),
          cancel: self.cancel.clone(),
        };
        self.printing = true;
        self.smoke_stage = 6;
        if self
          .worker
          .sender
          .send(Command::Print {
            generation: self.generation,
            states: self.states.clone(),
            rotation: self.rotation,
            job,
          })
          .is_err()
        {
          self.error("Не удалось запустить проверку печати.".into());
        }
      }
      _ => {
        let _ = std::fs::write(
          path.with_extension("ok.txt"),
          "Окно открыто; слои переключены; страница изменена; поворот отрисован.",
        );
        PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
      }
    }
  }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
  if msg == WM_APP + 21 {
    editor::command(hwnd, wp);
    return 0;
  }
  if msg == WM_APP + 22 {
    with_app(|a| a.poll_job());
    return 0;
  }
  if msg == WM_APP + 23 {
    let message = with_app(|a| {
      if a.dialog_open {
        None
      } else {
        let text = a.editor.notice.take()?;
        a.dialog_open = true;
        Some(text)
      }
    })
    .flatten();
    if let Some(message) = message {
      crate::dialogs::prompt(hwnd, "Astra PDF", "Результат операции", &message, true);
      with_app(|a| a.dialog_open = false);
    }
    return 0;
  }
  if msg == WM_CLOSE {
    let pending = with_app(|a| !a.editor.masks.is_empty() && !a.editor.busy).unwrap_or(false);
    if pending
      && MessageBoxW(
        hwnd,
        wide("Метки скрытия не сохраняются при закрытии. Закрыть окно?").as_ptr(),
        wide("Закрыть документ").as_ptr(),
        MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
      ) != IDYES
    {
      return 0;
    }
  }
  if msg == WM_APP + 14 {
    let message = with_app(|a| {
      if a.dialog_open {
        return None;
      }
      let message = a.pending_error.take()?;
      a.dialog_open = true;
      Some(message)
    })
    .flatten();
    if let Some(message) = message {
      MessageBoxW(
        hwnd,
        wide(&message).as_ptr(),
        wide("Astra PDF").as_ptr(),
        MB_OK | MB_ICONERROR,
      );
      with_app(|a| {
        a.dialog_open = false;
      });
      PostMessageW(hwnd, WM_APP + 14, 0, 0);
    }
    return 0;
  }
  // Системные диалоги обслуживают сообщения окна и не должны удерживать его состояние.
  if msg == WM_APP + 12 {
    let allowed = with_app(|a| {
      if a.dialog_open || a.printing || a.editor.busy {
        false
      } else {
        a.dialog_open = true;
        true
      }
    })
    .unwrap_or(false);
    if allowed {
      let result = App::choose_file(hwnd);
      let discard = result.as_ref().is_ok_and(|p| p.is_some())
        && with_app(|a| !a.editor.masks.is_empty()).unwrap_or(false);
      if discard
        && MessageBoxW(
          hwnd,
          wide("Метки скрытия текущего документа не сохранятся. Открыть другой документ?").as_ptr(),
          wide("Открыть документ").as_ptr(),
          MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
        ) != IDYES
      {
        with_app(|a| a.dialog_open = false);
        return 0;
      }
      with_app(|a| {
        a.dialog_open = false;
        match result {
          Ok(Some(path)) => a.open(path),
          Err(e) => a.error(e),
          _ => (),
        }
      });
    }
    PostMessageW(hwnd, WM_APP + 14, 0, 0);
    return 0;
  }
  if msg == WM_APP + 13 {
    let info = with_app(|a| {
      if a.dialog_open
        || a.printing
        || a.editor.busy
        || !a.editor.masks.is_empty()
        || a.sizes.is_empty()
      {
        return None;
      }
      a.dialog_open = true;
      a.cancel = Arc::new(AtomicBool::new(false));
      Some((
        a.generation,
        a.sizes.len(),
        a.page,
        a.path
          .as_ref()
          .and_then(|p| p.file_name())
          .unwrap_or_default()
          .to_string_lossy()
          .into_owned(),
        a.cancel.clone(),
      ))
    })
    .flatten();
    if let Some((generation, pages, page, name, cancel)) = info {
      let result = printing::dialog(hwnd, pages, page, &name, cancel);
      with_app(|a| {
        a.dialog_open = false;
        match result {
          Ok(Some(job)) if generation == a.generation => {
            a.printing = true;
            a.print_result = None;
            a.status = "Подготовка к печати…".into();
            if a
              .worker
              .sender
              .send(Command::Print {
                generation,
                states: a.states.clone(),
                rotation: a.rotation,
                job,
              })
              .is_err()
            {
              a.printing = false;
              a.error("Рабочий поток PDF остановлен.".into());
            }
          }
          Ok(Some(_)) => a.error("Документ изменился до начала печати.".into()),
          Err(e) => a.error(e),
          _ => (),
        }
        a.layout();
        a.enable_controls();
      });
    }
    PostMessageW(hwnd, WM_APP + 14, 0, 0);
    return 0;
  }
  if msg == WM_APP + 8 {
    with_app(|a| {
      if let Some(path) = &a.probe {
        use sha2::{Digest, Sha256};
        let digest = a
          .image
          .as_ref()
          .map(|image| {
            Sha256::digest(image.pixels.as_ref())
              .iter()
              .map(|b| format!("{b:02x}"))
              .collect::<String>()
          })
          .unwrap_or_default();
        let states: Vec<u8> = a.states.iter().map(|v| u8::from(*v)).collect();
        let checks: Vec<u8> = a
          .states
          .iter()
          .enumerate()
          .map(|(i, _)| {
            u8::from(
              SendMessageW(
                a.control(LAYER_LIST),
                LVM_GETITEMSTATE,
                i,
                LVIS_STATEIMAGEMASK as isize,
              ) == 2 << 12,
            )
          })
          .collect();
        let report = format!("{{\"page\":{},\"pages\":{},\"layers\":{},\"states\":{:?},\"checks\":{:?},\"rotation\":{},\"rendering\":{},\"printing\":{},\"generation\":{},\"ticket\":{},\"error\":{},\"first_frame_ms\":{},\"scale\":{},\"dpi\":{},\"scroll\":[{},{}],\"image_size\":[{},{}],\"image_hash\":\"{}\",\"cancelled\":{},\"print_result\":{}}}", a.page, a.sizes.len(), a.layers.len(), states, checks, a.rotation, a.rendering, a.printing, a.generation, a.ticket, a.last_error, a.first_frame_ms.map_or("null".into(), |v| v.to_string()), a.scale, a.dpi, a.scroll.0, a.scroll.1, a.image.as_ref().map_or(0, |i| i.width), a.image.as_ref().map_or(0, |i| i.height), digest, a.cancel.load(Ordering::Relaxed), a.print_result.map_or("null".into(), |v| v.to_string()));
        let report = format!(
          "{},\"dialog_open\":{},\"continuous\":{},\"visible_pages\":{:?},\"thumbnail_count\":{},\"thumbnail_bytes\":{},\"frame_bytes\":{},\"page_boxes\":{:?},\"preview_available\":{}}}",
          &report[..report.len() - 1],
          a.dialog_open,
          a.continuous,
          a.visible_pages(),
          a.thumbnails.len(),
          a.thumbnails.bytes,
          a.frames.bytes,
          a.visible_pages().iter().map(|&p| { let b = &a.document_layout.pages[p]; [p as i32, b.top, b.width, b.height] }).collect::<Vec<_>>(),
          !a.document_layout.pages.is_empty() && a.frames.preview(&a.page_key(a.page)).is_some()
        );
        let tiles: Vec<_> = a
          .visible_pages()
          .into_iter()
          .flat_map(|p| a.detail_keys(p))
          .collect();
        let report = format!("{},\"detail_mode\":{},\"tiles_visible\":{},\"tiles_ready\":{},\"tile_bytes\":{},\"viewport\":{:?}}}",
          &report[..report.len()-1], a.detail_mode, tiles.len(), tiles.iter().filter(|k| a.details.peek(k).is_some()).count(), a.details.bytes, [a.viewport().0,a.viewport().1]);
        let mut report: serde_json::Value = serde_json::from_str(&report).unwrap();
        report["editor"] = a.editor.probe();
        report["page_origins"] = serde_json::json!(a
          .visible_pages()
          .iter()
          .map(|&p| {
            let o = a.page_origin(p);
            [p as i32, o.0 - a.scroll.0, o.1 - a.scroll.1]
          })
          .collect::<Vec<_>>());
        let _ = std::fs::write(path, report.to_string());
      }
    });
    return 0;
  }
  if msg == WM_APP + 9 {
    let path = APP.with(|a| a.borrow().as_ref().and_then(|s| s.probe.clone()));
    if let Some(path) = path {
      capture_window(hwnd, &path.with_extension("png"));
    }
    return 0;
  }
  if msg == WM_APP + 10 {
    with_app(|a| {
      if let Some(path) = &a.probe {
        if a.printing || a.sizes.is_empty() {
          return;
        }
        let dc = CreateDCW(
          wide("WINSPOOL").as_ptr(),
          wide("Microsoft Print to PDF").as_ptr(),
          ptr::null(),
          ptr::null(),
        );
        if dc.is_null() {
          a.error("Недоступен Microsoft Print to PDF.".into());
          return;
        }
        a.cancel = Arc::new(AtomicBool::new(false));
        a.print_result = None;
        let job = printing::PrintJob {
          dc: dc as usize,
          first: 0,
          last: a.sizes.len() - 1,
          title: "Astra PDF — проверка отмены".into(),
          output: Some(
            path
              .with_extension("print.pdf")
              .to_string_lossy()
              .into_owned(),
          ),
          cancel: a.cancel.clone(),
        };
        a.printing = true;
        if a
          .worker
          .sender
          .send(Command::Print {
            generation: a.generation,
            states: a.states.clone(),
            rotation: a.rotation,
            job,
          })
          .is_err()
        {
          a.printing = false;
          a.error("Не удалось запустить проверку печати.".into());
        }
        a.layout();
        a.enable_controls();
      }
    });
    return 0;
  }
  if msg == WM_APP + 5 {
    let info = APP.with(|a| {
      a.borrow()
        .as_ref()
        .map(|s| (s.control(LAYER_LIST), !s.states[0]))
    });
    if let Some((control, next)) = info {
      let mut item: LVITEMW = std::mem::zeroed();
      item.stateMask = LVIS_STATEIMAGEMASK;
      item.state = if next { 2 << 12 } else { 1 << 12 };
      SendMessageW(control, LVM_SETITEMSTATE, 0, &item as *const _ as isize);
    }
    return 0;
  }
  if msg == WM_APP + 3 {
    // Снимок синхронно вызывает отрисовку; заимствование состояния заранее освобождается.
    let info = APP.with(|a| a.borrow().as_ref().map(|s| (s.hwnd, s.smoke.clone())));
    if let Some((_, Some(path))) = info {
      capture_window(hwnd, &path);
      with_app(|a| {
        SetTimer(a.hwnd, 2, 100, None);
      });
    }
    return 0;
  }
  let result = with_app(|app| match msg {
    WM_DRAWITEM => {
      let item = &*(lp as *const DRAWITEMSTRUCT);
      if item.CtlID == PAGE_LIST as u32 {
        app.paint_thumbnail(item);
        Some(1)
      } else {
        None
      }
    }
    WM_MEASUREITEM => {
      let item = &mut *(lp as *mut MEASUREITEMSTRUCT);
      if item.CtlID == PAGE_LIST as u32 {
        item.itemHeight = app.unit(184) as u32;
        Some(1)
      } else {
        None
      }
    }
    WM_PAINT => {
      app.paint(None);
      Some(0)
    }
    WM_ERASEBKGND => Some(1),
    WM_PRINTCLIENT => {
      app.paint(Some(wp as HDC));
      Some(0)
    }
    WM_SIZE => {
      app.layout();
      Some(0)
    }
    WM_GETMINMAXINFO => {
      let m = &mut *(lp as *mut MINMAXINFO);
      m.ptMinTrackSize = POINT {
        x: app.unit(560),
        y: app.unit(520),
      };
      Some(0)
    }
    WM_DPICHANGED => {
      app.dpi = wp as u32 & 0xffff;
      app.set_fonts();
      let r = &*(lp as *const RECT);
      SetWindowPos(
        hwnd,
        ptr::null_mut(),
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
        SWP_NOZORDER | SWP_NOACTIVATE,
      );
      app.layout();
      Some(0)
    }
    WM_COMMAND => {
      let id = wp & 0xffff;
      let notification = (wp >> 16) as u32;
      if id == PAGE_LIST && notification == LBN_SELCHANGE {
        let index = SendMessageW(app.control(PAGE_LIST), LB_GETCURSEL, 0, 0);
        if index >= 0 {
          app.navigate(index as usize);
        }
      } else if (id == VIEW_MODE && notification == CBN_SELCHANGE) || notification == BN_CLICKED {
        app.action(id);
      }
      Some(0)
    }
    WM_NOTIFY => {
      let header = &*(lp as *const NMHDR);
      if header.idFrom == LAYER_LIST && header.code == LVN_ITEMCHANGED {
        let change = &*(lp as *const NMLISTVIEW);
        if change.iItem >= 0 && (change.uOldState ^ change.uNewState) & LVIS_STATEIMAGEMASK != 0 {
          let i = change.iItem as usize;
          if i < app.states.len() && !app.printing {
            let desired = change.uNewState & LVIS_STATEIMAGEMASK == 2 << 12;
            if desired != app.states[i] {
              PostMessageW(hwnd, WM_APP + 4, i, desired as isize);
            }
          }
        }
      }
      Some(0)
    }
    WM_DROPFILES => {
      let drop = wp as HDROP;
      let size = DragQueryFileW(drop, 0, ptr::null_mut(), 0);
      let mut value = vec![0u16; size as usize + 1];
      DragQueryFileW(drop, 0, value.as_mut_ptr(), value.len() as u32);
      DragFinish(drop);
      if app.printing || app.editor.busy || !app.editor.masks.is_empty() {
        return Some(0);
      }
      use std::os::windows::ffi::OsStringExt;
      app.open(PathBuf::from(std::ffi::OsString::from_wide(
        &value[..size as usize],
      )));
      Some(0)
    }
    READY => {
      app.events();
      Some(0)
    }
    msg if msg == WM_APP + 4 => {
      if wp < app.states.len() && (lp != 0) != app.states[wp] && !app.printing {
        Layers::apply_toggle(&app.layers, &app.groups, &mut app.states, wp);
        app.sync_layer_checks();
        app.request();
      }
      Some(0)
    }
    WM_TIMER if wp == 1 => {
      KillTimer(hwnd, 1);
      app.request();
      Some(0)
    }
    WM_TIMER if wp == 2 => {
      app.smoke_step();
      Some(0)
    }
    WM_TIMER if wp == 3 => {
      app.request_thumbnails();
      Some(0)
    }
    WM_TIMER if wp == 4 => {
      app.poll_job();
      Some(0)
    }
    WM_CLOSE => {
      app.cancel.store(true, Ordering::Relaxed);
      if app.printing || app.editor.busy {
        app.closing = true;
        app.status = "Завершаю отмену печати…".into();
        InvalidateRect(hwnd, ptr::null(), 0);
      } else {
        PostQuitMessage(0);
      }
      Some(0)
    }
    _ => None,
  })
  .flatten();
  result.unwrap_or_else(|| DefWindowProcW(hwnd, msg, wp, lp))
}

unsafe extern "system" fn canvas_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
  let result = with_app(|app| match msg {
    WM_PAINT => {
      app.paint_canvas(None);
      Some(0)
    }
    WM_ERASEBKGND => Some(1),
    WM_PRINTCLIENT => {
      app.paint_canvas(Some(wp as HDC));
      Some(0)
    }
    WM_LBUTTONDOWN => {
      SetFocus(hwnd);
      app.editor_mouse(msg, lp);
      Some(0)
    }
    WM_MOUSEMOVE | WM_LBUTTONUP | WM_CAPTURECHANGED => {
      app.editor_mouse(msg, lp);
      Some(0)
    }
    WM_VSCROLL | WM_HSCROLL => {
      app.scroll_message(msg == WM_HSCROLL, (wp & 0xffff) as i32);
      Some(0)
    }
    WM_MOUSEWHEEL => {
      let delta = ((wp >> 16) as u16 as i16) as i32;
      if GetKeyState(VK_CONTROL as i32) < 0 {
        let mut point = POINT {
          x: (lp as u16 as i16) as i32,
          y: ((lp >> 16) as u16 as i16) as i32,
        };
        ScreenToClient(hwnd, &mut point);
        app.zoom_at(1.08_f64.powf(delta as f64 / 120.), Some((point.x, point.y)));
      } else {
        let step = delta * app.unit(48) / 120;
        if GetKeyState(VK_SHIFT as i32) < 0 {
          app.scroll.0 -= step;
        } else {
          app.scroll.1 -= step;
        }
        app.scrolled();
      }
      Some(0)
    }
    WM_MOUSEHWHEEL => {
      app.scroll.0 += ((wp >> 16) as u16 as i16) as i32 * app.unit(48) / 120;
      app.scrolled();
      Some(0)
    }
    _ => None,
  })
  .flatten();
  result.unwrap_or_else(|| DefWindowProcW(hwnd, msg, wp, lp))
}

unsafe fn capture_window(hwnd: HWND, path: &std::path::Path) {
  let mut r: RECT = std::mem::zeroed();
  GetWindowRect(hwnd, &mut r);
  let (w, h) = (r.right - r.left, r.bottom - r.top);
  let dc = GetDC(hwnd);
  let mem = CreateCompatibleDC(dc);
  let bmp = CreateCompatibleBitmap(dc, w, h);
  let old = SelectObject(mem, bmp);
  SendMessageW(
    hwnd,
    WM_PRINT,
    mem as usize,
    (PRF_CLIENT | PRF_NONCLIENT | PRF_CHILDREN | PRF_ERASEBKGND) as isize,
  );
  SelectObject(mem, old);
  let mut image = Raster {
    width: w,
    height: h,
    pixels: Arc::new(vec![0; w as usize * h as usize * 4]),
  };
  let mut info = printing::bitmap_info(&image);
  GetDIBits(
    mem,
    bmp,
    0,
    h as u32,
    Arc::get_mut(&mut image.pixels).unwrap().as_mut_ptr().cast(),
    &mut info,
    DIB_RGB_COLORS,
  );
  DeleteObject(bmp);
  DeleteDC(mem);
  ReleaseDC(hwnd, dc);
  let _ = image.save_png(path);
}

unsafe fn hotkey(msg: &MSG) -> bool {
  if msg.message != WM_KEYDOWN {
    return false;
  }
  with_app(|a| {
    let key = msg.wParam as u16;
    let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
    let edit = GetFocus() == a.control(PAGE_EDIT);
    let canvas_focus = GetFocus() == a.canvas || GetFocus() == a.hwnd;
    if !edit && canvas_focus && !ctrl {
      let code = match key {
        VK_PRIOR => Some(SB_PAGEUP),
        VK_NEXT | VK_SPACE => Some(SB_PAGEDOWN),
        VK_UP => Some(SB_LINEUP),
        VK_DOWN => Some(SB_LINEDOWN),
        VK_HOME => Some(SB_TOP),
        VK_END => Some(SB_BOTTOM),
        _ => None,
      };
      if let Some(code) = code.filter(|_| a.continuous) {
        a.scroll_message(false, code);
        return true;
      }
    }
    let id = if ctrl {
      match key {
        0x4f => Some(OPEN),
        0x50 => Some(PRINT),
        VK_OEM_PLUS | VK_ADD => Some(PLUS),
        VK_OEM_MINUS | VK_SUBTRACT => Some(MINUS),
        0x30 => Some(FIT),
        0x32 => Some(WIDTH),
        0x52 => Some(ROTATE),
        _ => None,
      }
    } else if !edit {
      match key {
        VK_PRIOR => Some(PREV),
        VK_NEXT => Some(NEXT),
        _ => None,
      }
    } else {
      None
    };
    if let Some(id) = id {
      a.action(id);
      return true;
    }
    if edit && key == VK_RETURN {
      let mut value = [0u16; 32];
      let len = GetWindowTextW(a.control(PAGE_EDIT), value.as_mut_ptr(), 32);
      if let Ok(page) = String::from_utf16_lossy(&value[..len.max(0) as usize]).parse::<usize>() {
        a.navigate(page.saturating_sub(1));
      }
      SetWindowTextW(
        a.control(PAGE_EDIT),
        wide(&(a.page + 1).to_string()).as_ptr(),
      );
      SetFocus(a.canvas);
      return true;
    }
    if key == VK_ESCAPE && (a.printing || a.editor.busy) {
      a.action(CANCEL);
      return true;
    }
    if ctrl && key == 0x31 {
      a.remember_anchor(None);
      a.zoom = Zoom::Scale(1.);
      a.request();
      return true;
    }
    false
  })
  .unwrap_or(false)
}

pub fn run(
  dll: PathBuf,
  path: Option<PathBuf>,
  smoke: Option<PathBuf>,
  probe: Option<PathBuf>,
) -> Result<(), String> {
  unsafe {
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    let controls = INITCOMMONCONTROLSEX {
      dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
      dwICC: ICC_LISTVIEW_CLASSES | ICC_STANDARD_CLASSES,
    };
    InitCommonControlsEx(&controls);
    let instance = GetModuleHandleW(ptr::null());
    let classes: [(&str, WNDPROC); 2] = [
      ("AstraPdfWindow", Some(window_proc)),
      ("AstraPdfCanvas", Some(canvas_proc)),
    ];
    for (name, proc) in classes {
      let mut class: WNDCLASSW = std::mem::zeroed();
      let name = wide(name);
      class.lpszClassName = name.as_ptr();
      class.hInstance = instance;
      class.lpfnWndProc = proc;
      class.hCursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);
      class.hIcon = LoadIconW(instance, 101usize as _);
      if RegisterClassW(&class) == 0 {
        return Err("Не удалось зарегистрировать окно.".into());
      }
    }
    let hwnd = CreateWindowExW(
      WS_EX_ACCEPTFILES,
      wide("AstraPdfWindow").as_ptr(),
      wide(&crate::version::title()).as_ptr(),
      WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
      CW_USEDEFAULT,
      CW_USEDEFAULT,
      1200,
      850,
      ptr::null_mut(),
      ptr::null_mut(),
      instance,
      ptr::null(),
    );
    if hwnd.is_null() {
      return Err("Не удалось создать окно Windows.".into());
    }
    let corner: u32 = 2;
    DwmSetWindowAttribute(hwnd, 33, &corner as *const _ as _, 4);
    let worker = Worker::start(hwnd as usize, dll);
    let mut app = App {
      editor: editor::Editor::default(),
      hwnd,
      canvas: ptr::null_mut(),
      controls: vec![],
      font: ptr::null_mut(),
      big_font: ptr::null_mut(),
      dpi: GetDpiForWindow(hwnd),
      toolbar_height: 64,
      worker,
      path: None,
      sizes: vec![],
      layers: vec![],
      groups: vec![],
      states: vec![],
      page: 0,
      zoom: Zoom::FitPage,
      scale: 1.,
      rotation: 0,
      generation: 0,
      ticket: 0,
      image: None,
      wanted: (0, 0),
      scroll: (0, 0),
      layer_tab: false,
      layer_warning: false,
      status: "Готово   ·   Ctrl+O — открыть   ·   Ctrl+P — печать".into(),
      rendering: false,
      printing: false,
      closing: false,
      cancel: Arc::new(AtomicBool::new(false)),
      smoke,
      smoke_stage: 0,
      probe,
      opened_at: std::time::Instant::now(),
      first_frame_ms: None,
      last_error: false,
      print_result: None,
      dialog_open: false,
      pending_error: None,
      continuous: true,
      document_layout: DocumentLayout::default(),
      layout_key: None,
      frames: Cache::with_count(96 * 1024 * 1024, 64),
      thumbnails: Cache::with_count(16 * 1024 * 1024, 48),
      render_plan: vec![],
      detail_mode: false,
      details: Cache::with_count(64 * 1024 * 1024, 64),
      thumb_plan: vec![],
      thumb_context: None,
      thumb_ticket: 0,
      anchor: None,
    };
    app.setup();
    editor::menu(hwnd);
    let mut monitor: MONITORINFO = std::mem::zeroed();
    monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(
      MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
      &mut monitor,
    );
    let area = monitor.rcWork;
    let width = app
      .unit(1100)
      .min((area.right - area.left - app.unit(24)).max(600));
    let height = app
      .unit(760)
      .min((area.bottom - area.top - app.unit(24)).max(400));
    let x = area.left + (area.right - area.left - width) / 2;
    let y = area.top + (area.bottom - area.top - height) / 2;
    APP.with(|a| *a.borrow_mut() = Some(app));
    SetWindowPos(
      hwnd,
      ptr::null_mut(),
      x,
      y,
      width,
      height,
      SWP_NOZORDER | SWP_NOACTIVATE,
    );
    ShowWindow(hwnd, SW_SHOW);
    UpdateWindow(hwnd);
    if let Some(path) = path {
      with_app(|a| a.open(path));
    }
    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
      if !hotkey(&msg) && IsDialogMessageW(hwnd, &msg) == 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
      }
    }
    DestroyWindow(hwnd);
    APP.with(|a| {
      if let Some(a) = a.borrow_mut().take() {
        DeleteObject(a.font);
        DeleteObject(a.big_font);
      }
    });
    if msg.wParam != 0 {
      return Err("Проверка интерфейса завершилась ошибкой.".into());
    }
  }
  Ok(())
}

use super::*;
use crate::{
  editing::Operation,
  engine::Client,
  preferences,
  reading::{self as data, Bookmark, PageText, Search, Target},
};
use std::{collections::VecDeque, sync::mpsc};
use windows_sys::Win32::System::{DataExchange::*, Memory::*};

pub(super) const FIND: usize = 70;
pub(super) const TEXT: usize = 71;
pub(super) const BOOKMARKS: usize = 72;
pub(super) const BOOKMARK_LIST: usize = 73;
pub(super) const QUERY: usize = 74;
pub(super) const FIND_PREV: usize = 75;
pub(super) const FIND_NEXT: usize = 76;
pub(super) const FIND_CLOSE: usize = 77;
pub(super) const FIND_STATUS: usize = 78;
pub(super) const HISTORY: usize = 79;
pub(super) const COPY: usize = 80;
pub(super) const RECENT: usize = 300;

enum Answer {
  Page(usize, PageText),
  Search(Search),
  Bookmarks(Vec<Bookmark>),
}
#[derive(Default)]
pub(super) struct Reader {
  pub text_mode: bool,
  pub search_open: bool,
  pub bookmarks_open: bool,
  pub query: String,
  pages: VecDeque<(usize, PageText)>,
  failed_pages: Vec<usize>,
  bookmarks: Option<Vec<Bookmark>>,
  pub results: Vec<data::Hit>,
  current: Option<usize>,
  selection: Option<(usize, usize, usize)>,
  dragging: bool,
  receiver: Option<mpsc::Receiver<Result<Answer, String>>>,
  cancel: Arc<AtomicBool>,
  requested: Option<usize>,
  search_pending: bool,
}

impl Drop for Reader {
  fn drop(&mut self) {
    self.cancel.store(true, Ordering::Relaxed);
  }
}

impl Reader {
  pub(super) fn invalidate_page(&mut self, page: usize) {
    self.cancel.store(true, Ordering::Relaxed);
    self.receiver = None;
    self.requested = None;
    self.search_pending = false;
    self.pages.retain(|(p, _)| *p != page);
    self.failed_pages.retain(|p| *p != page);
    self.selection = None;
    self.dragging = false;
    // После изменения текста прежние координаты поиска уже недействительны.
    self.results.clear();
    self.current = None;
    self.query.clear();
  }
  pub fn note_at(
    &self,
    page: usize,
    point: (f64, f64),
    masks: &[crate::editing::Mask],
  ) -> Option<crate::reading::Comment> {
    self
      .pages
      .iter()
      .find(|(p, _)| *p == page)?
      .1
      .notes
      .iter()
      .rev()
      .find(|n| {
        point.0 >= n.bounds[0]
          && point.0 <= n.bounds[2]
          && point.1 >= n.bounds[1]
          && point.1 <= n.bounds[3]
          && !data::hidden(page, n.bounds, masks)
      })
      .cloned()
  }
  pub fn selected_text(&self, masks: &[crate::editing::Mask]) -> String {
    self
      .selection
      .and_then(|(page, start, end)| {
        self
          .pages
          .iter()
          .find(|(p, _)| *p == page)
          .map(|(_, text)| data::selection(page, text, start, end, masks))
      })
      .unwrap_or_default()
  }
  pub fn probe(&self) -> serde_json::Value {
    serde_json::json!({"text_mode":self.text_mode,"search_open":self.search_open,"query":self.query,"hits":self.results.len(),"current":self.current,"selection":self.selection,"loaded_pages":self.pages.iter().map(|p|p.0).collect::<Vec<_>>(),"busy":self.receiver.is_some(),"bookmarks":self.bookmarks.as_ref().map(Vec::len)})
  }
  pub fn stop(&mut self) {
    self.text_mode = false;
    self.selection = None;
    self.dragging = false;
  }
}

impl App {
  pub(super) unsafe fn setup_reader(&mut self) {
    for (id, label) in [
      (FIND, "Поиск · Ctrl+F"),
      (TEXT, "Выделить текст"),
      (BOOKMARKS, "Закладки"),
      (FIND_PREV, "Предыдущее совпадение"),
      (FIND_NEXT, "Следующее совпадение"),
      (FIND_CLOSE, "Закрыть поиск"),
      (HISTORY, "История действий"),
    ] {
      self.create(id, "BUTTON", label, BS_OWNERDRAW as u32);
    }
    self.create(QUERY, "EDIT", "", WS_BORDER | ES_AUTOHSCROLL as u32);
    SendMessageW(self.control(QUERY), EM_SETLIMITTEXT, 1024, 0);
    SendMessageW(
      self.control(QUERY),
      EM_SETCUEBANNER,
      1,
      wide("Найти в документе").as_ptr() as isize,
    );
    self.create(FIND_STATUS, "STATIC", "Enter — найти", 0x200);
    self.create(
      BOOKMARK_LIST,
      "LISTBOX",
      "Закладки документа",
      WS_VSCROLL | LBS_NOTIFY as u32 | LBS_NOINTEGRALHEIGHT as u32,
    );
    SetTimer(self.hwnd, 6, 150, None);
  }

  pub(super) unsafe fn reader_layout(
    &self,
    width: i32,
    height: i32,
    top: i32,
    tools_x: i32,
    tools_y: i32,
  ) {
    for (i, id) in [FIND, TEXT, HISTORY].iter().enumerate() {
      self.place(*id, tools_x + (7 + i as i32) * 36, tools_y, 32, 32);
    }
    self.place(BOOKMARKS, 164, top + 10, 32, 32);
    self.place(
      BOOKMARK_LIST,
      12,
      top + 96,
      216,
      (height - top - 136).max(30),
    );
    ShowWindow(
      self.control(BOOKMARK_LIST),
      if self.reader.bookmarks_open {
        SW_SHOW
      } else {
        SW_HIDE
      },
    );
    if self.reader.bookmarks_open {
      for id in [PAGE_LIST, LAYER_LIST, LAYER_NOTE, RESET] {
        ShowWindow(self.control(id), SW_HIDE);
      }
    }
    let x = 252;
    let available = (width - x - 12).max(100);
    let query_width = (available - 210).max(100);
    self.place(QUERY, x, top + 6, query_width, 28);
    self.place(FIND_PREV, x + query_width + 6, top + 4, 28, 32);
    self.place(FIND_NEXT, x + query_width + 36, top + 4, 28, 32);
    self.place(
      FIND_STATUS,
      x + query_width + 70,
      top + 6,
      (available - query_width - 105).max(30),
      28,
    );
    self.place(FIND_CLOSE, width - 44, top + 4, 32, 32);
    for id in [QUERY, FIND_PREV, FIND_NEXT, FIND_STATUS, FIND_CLOSE] {
      ShowWindow(
        self.control(id),
        if self.reader.search_open {
          SW_SHOW
        } else {
          SW_HIDE
        },
      );
    }
  }

  unsafe fn reader_job(&mut self, operation: Operation) {
    let Some(path) = self.document_path() else {
      return;
    };
    let Some(hash) = self.editor.fingerprint else {
      return;
    };
    self.reader.cancel.store(true, Ordering::Relaxed);
    let cancel = Arc::new(AtomicBool::new(false));
    self.reader.cancel = cancel.clone();
    let revision = self.editor.revision();
    let (tx, rx) = mpsc::channel();
    self.reader.receiver = Some(rx);
    std::thread::spawn(move || {
      let _revision = revision;
      let result = (|| {
        let mut client = Client::spawn(&crate::pdfium_path())?;
        client.open_checked(&path, hash, || cancel.load(Ordering::Relaxed))?;
        let bytes = client.edit(operation.clone(), || cancel.load(Ordering::Relaxed))?;
        match operation {
          Operation::ReadPage { page } => Ok(Answer::Page(
            page,
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?,
          )),
          Operation::Find { .. } => Ok(Answer::Search(
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?,
          )),
          Operation::Bookmarks => Ok(Answer::Bookmarks(
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?,
          )),
          _ => Err("Неизвестная операция чтения.".into()),
        }
      })();
      let _ = tx.send(result);
    });
  }

  pub(super) unsafe fn poll_reader(&mut self) {
    if self.editor.busy || self.printing || self.dialog_open {
      return;
    }
    if let Some(result) = self
      .reader
      .receiver
      .as_ref()
      .and_then(|r| r.try_recv().ok())
    {
      self.reader.receiver = None;
      let requested = self.reader.requested.take();
      self.reader.search_pending = false;
      match result {
        Ok(Answer::Page(page, text)) => {
          self.reader.pages.push_front((page, text));
          self.reader.pages.truncate(4);
        }
        Ok(Answer::Search(result)) => {
          self.reader.results = result.hits;
          self.reader.current = None;
          if self.reader.results.is_empty() {
            SetWindowTextW(self.control(FIND_STATUS), wide("Не найдено").as_ptr());
          } else {
            self.find_next(false);
          }
          if result.truncated {
            self.status = "Показаны первые 1000 совпадений. Уточните запрос.".into();
          }
        }
        Ok(Answer::Bookmarks(items)) => {
          let list = self.control(BOOKMARK_LIST);
          SendMessageW(list, LB_RESETCONTENT, 0, 0);
          if items.is_empty() {
            SendMessageW(
              list,
              LB_ADDSTRING,
              0,
              wide("В PDF нет закладок").as_ptr() as isize,
            );
          }
          for item in &items {
            SendMessageW(
              list,
              LB_ADDSTRING,
              0,
              wide(&format!(
                "{}{}",
                "  ".repeat(item.level.min(12)),
                item.title.replace(['\r', '\n'], " ")
              ))
              .as_ptr() as isize,
            );
          }
          self.reader.bookmarks = Some(items);
        }
        Err(error) => {
          self.reader.query.clear();
          if let Some(page) = requested {
            self.reader.failed_pages.push(page);
          }
          self.status = error;
          if self.reader.search_open {
            SetWindowTextW(self.control(FIND_STATUS), wide("Ошибка поиска").as_ptr());
          }
        }
      }
      InvalidateRect(self.canvas, ptr::null(), 0);
      InvalidateRect(self.hwnd, ptr::null(), 0);
    }
    if self.sizes.is_empty()
      || self.rendering
      || self.reader.receiver.is_some()
      || self.editor.fingerprint.is_none()
    {
      return;
    }
    if self.reader.bookmarks_open && self.reader.bookmarks.is_none() {
      self.reader_job(Operation::Bookmarks);
    } else if self.reader.text_mode || self.editor.viewing() {
      if let Some(page) = self.visible_pages().into_iter().take(2).find(|p| {
        !self.reader.pages.iter().any(|(i, _)| i == p) && !self.reader.failed_pages.contains(p)
      }) {
        self.reader.requested = Some(page);
        self.reader_job(Operation::ReadPage { page });
      }
    }
  }

  pub(super) unsafe fn reader_action(&mut self, id: usize) {
    match id {
      FIND => {
        self.reader.search_open = true;
        self.layout();
        SetFocus(self.control(QUERY));
        SendMessageW(self.control(QUERY), EM_SETSEL, 0, -1);
      }
      TEXT => {
        let active = self.reader.text_mode;
        self.cancel_editor();
        self.reader.text_mode = !active;
        self.reader.selection = None;
        self.status =
          "Выделите текст мышью · Ctrl+C — копировать · Esc — выключить инструмент".into();
      }
      BOOKMARKS => {
        self.reader.bookmarks_open = !self.reader.bookmarks_open;
        self.layout();
      }
      BOOKMARK_LIST => {
        let i = SendMessageW(self.control(BOOKMARK_LIST), LB_GETCURSEL, 0, 0);
        if let Some(page) = self
          .reader
          .bookmarks
          .as_ref()
          .and_then(|b| b.get(i as usize))
          .and_then(|b| b.page)
        {
          self.navigate(page);
        }
      }
      FIND_CLOSE => {
        self.reader.search_open = false;
        self.reader.cancel.store(true, Ordering::Relaxed);
        self.reader.receiver = None;
        self.reader.search_pending = false;
        self.reader.requested = None;
        self.reader.results.clear();
        self.reader.current = None;
        self.layout();
        SetFocus(self.canvas);
      }
      FIND_NEXT | FIND_PREV => {
        let mut value = vec![0u16; 1025];
        let len = GetWindowTextW(self.control(QUERY), value.as_mut_ptr(), value.len() as i32);
        let query = String::from_utf16_lossy(&value[..len.max(0) as usize]);
        if !query.trim().is_empty() && query != self.reader.query {
          self.reader.query = query.clone();
          self.reader.results.clear();
          self.reader.current = None;
          self.reader.search_pending = true;
          self.reader.requested = None;
          SetWindowTextW(self.control(FIND_STATUS), wide("Поиск…").as_ptr());
          self.reader_job(Operation::Find {
            query,
            masks: self.editor.masks.clone(),
          });
        } else {
          self.find_next(id == FIND_PREV);
        }
      }
      COPY => {
        if let Some((page, start, end)) = self.reader.selection {
          if let Some((_, text)) = self.reader.pages.iter().find(|(p, _)| *p == page) {
            let value = data::selection(page, text, start, end, &self.editor.masks);
            if !value.is_empty() {
              if let Err(error) = copy(self.hwnd, &value) {
                self.status = error;
              } else {
                self.status = "Выделенный текст скопирован".into();
              }
            }
          }
        }
      }
      HISTORY => {
        self.editor.notice = Some(self.editor.history_description());
        PostMessageW(self.hwnd, WM_APP + 23, 0, 0);
      }
      _ => (),
    }
    self.enable_controls();
    InvalidateRect(self.hwnd, ptr::null(), 0);
    InvalidateRect(self.control(TEXT), ptr::null(), 1);
    InvalidateRect(self.control(FIND), ptr::null(), 1);
    InvalidateRect(self.canvas, ptr::null(), 0);
  }

  unsafe fn find_next(&mut self, previous: bool) {
    let n = self.reader.results.len();
    if n == 0 {
      return;
    }
    let i = self
      .reader
      .current
      .map_or(if previous { n - 1 } else { 0 }, |i| {
        if previous {
          (i + n - 1) % n
        } else {
          (i + 1) % n
        }
      });
    self.reader.current = Some(i);
    let hit = self.reader.results[i].clone();
    self.navigate(hit.page);
    if let Some(bounds) = hit.bounds.first() {
      let p = &self.document_layout.pages[hit.page];
      let (x, y) = crate::export::rotate_point(
        ((bounds[0] + bounds[2]) * 0.5, (bounds[1] + bounds[3]) * 0.5),
        self.rotation,
      );
      let origin = self.page_origin(hit.page);
      let view = self.viewport();
      self.scroll = (
        origin.0 + (x * p.width as f64) as i32 - view.0 / 2,
        origin.1 + (y * p.height as f64) as i32 - view.1 / 2,
      );
      self.update_scroll();
      self.request();
    }
    SetWindowTextW(
      self.control(FIND_STATUS),
      wide(&format!("{} / {n}", i + 1)).as_ptr(),
    );
  }

  pub(super) unsafe fn reader_mouse(&mut self, msg: u32, lp: LPARAM) -> bool {
    if self.editor.busy || self.printing || self.dialog_open {
      return false;
    }
    let point = ((lp as u16 as i16) as i32, ((lp >> 16) as u16 as i16) as i32);
    if msg == WM_LBUTTONDOWN {
      let Some((page, p)) = self.page_point(point, None) else {
        return false;
      };
      if self.editor.masks.iter().any(|m| {
        m.page == page
          && p.0 >= m.bounds[0]
          && p.0 <= m.bounds[2]
          && p.1 >= m.bounds[1]
          && p.1 <= m.bounds[3]
      }) {
        return false;
      }
      let Some((_, text)) = self.reader.pages.iter().find(|(i, _)| *i == page) else {
        return false;
      };
      if self.reader.text_mode {
        if let Some(index) = data::nearest(text, p) {
          self.reader.selection = Some((page, index, index));
          self.reader.dragging = true;
          SetCapture(self.canvas);
        }
        InvalidateRect(self.canvas, ptr::null(), 0);
        return true;
      }
      if self.editor.viewing() {
        if let Some(link) = text.links.iter().find(|l| {
          p.0 >= l.bounds[0] && p.0 <= l.bounds[2] && p.1 >= l.bounds[1] && p.1 <= l.bounds[3]
        }) {
          match link.target.clone() {
            Target::Page(page) => self.navigate(page),
            Target::Url(url) => {
              ShellExecuteW(
                self.hwnd,
                wide("open").as_ptr(),
                wide(&url).as_ptr(),
                ptr::null(),
                ptr::null(),
                SW_SHOWNORMAL,
              );
            }
          }
          return true;
        }
      }
    } else if self.reader.dragging {
      if let Some((page, start, _)) = self.reader.selection {
        if let Some((_, p)) = self.page_point(point, Some(page)) {
          if let Some((_, text)) = self.reader.pages.iter().find(|(i, _)| *i == page) {
            if let Some(end) = data::nearest(text, p) {
              self.reader.selection = Some((page, start, end));
            }
          }
        }
      }
      if msg == WM_LBUTTONUP || msg == WM_CAPTURECHANGED {
        self.reader.dragging = false;
        ReleaseCapture();
      }
      InvalidateRect(self.canvas, ptr::null(), 0);
      return true;
    }
    false
  }

  pub(super) unsafe fn reader_paint(&self, dc: HDC) {
    let mut rectangles = Vec::new();
    for (i, hit) in self.reader.results.iter().enumerate() {
      for b in &hit.bounds {
        if !data::hidden(hit.page, *b, &self.editor.masks) {
          if let Some(r) = self.editor_rect(hit.page, *b) {
            rectangles.push((r, Some(i) == self.reader.current));
          }
        }
      }
    }
    if let Some((page, start, end)) = self.reader.selection {
      if let Some((_, text)) = self.reader.pages.iter().find(|(p, _)| *p == page) {
        let bounds = data::merge_bounds(
          text
            .glyphs
            .iter()
            .skip(start.min(end))
            .take(start.abs_diff(end) + 1)
            .filter(|g| !data::hidden(page, g.bounds, &self.editor.masks))
            .map(|g| g.bounds),
        );
        for bounds in bounds {
          if !data::hidden(page, bounds, &self.editor.masks) {
            if let Some(r) = self.editor_rect(page, bounds) {
              rectangles.push((r, false));
            }
          }
        }
      }
    }
    // Полупрозрачная заливка сохраняет читаемость текста под выделением.
    let source = CreateCompatibleDC(dc);
    let pixel = CreateCompatibleBitmap(dc, 1, 1);
    let old = SelectObject(source, pixel);
    for (r, active) in rectangles {
      if r.right <= r.left || r.bottom <= r.top {
        continue;
      }
      SetPixel(source, 0, 0, if active { 0x0000aaee } else { 0x00efac42 });
      AlphaBlend(
        dc,
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
        source,
        0,
        0,
        1,
        1,
        BLENDFUNCTION {
          BlendOp: AC_SRC_OVER as u8,
          BlendFlags: 0,
          SourceConstantAlpha: 90,
          AlphaFormat: 0,
        },
      );
    }
    SelectObject(source, old);
    DeleteObject(pixel);
    DeleteDC(source);
  }

  pub(super) unsafe fn remember_reading(&mut self) {
    if (self.probe.is_some() && std::env::var_os("ASTRA_TEST_RECENT").is_none())
      || self.smoke.is_some()
      || self.sizes.is_empty()
      || self.editor.busy
      || self.editor.dirty()
    {
      return;
    }
    let (Some(path), Some(hash)) = (self.path.clone(), self.editor.fingerprint) else {
      return;
    };
    let recent = preferences::Recent {
      path,
      hash,
      page: self.page,
      zoom: self.zoom,
      rotation: self.rotation,
      continuous: self.continuous,
      detail: self.detail_mode,
    };
    if preferences::remember(&mut self.recent, recent) {
      let _ = preferences::save(&self.recent);
    }
  }

  pub(super) unsafe fn resume_reading(&mut self, hash: [u8; 32]) {
    if (self.probe.is_some() && std::env::var_os("ASTRA_TEST_RECENT").is_none())
      || self.smoke.is_some()
    {
      return;
    }
    if let Some(item) = self
      .recent
      .iter()
      .find(|r| Some(&r.path) == self.path.as_ref() && r.hash == hash)
    {
      self.page = item.page.min(self.sizes.len().saturating_sub(1));
      self.zoom = item.zoom;
      self.rotation = item.rotation;
      self.continuous = item.continuous;
      self.detail_mode = item.detail;
      SendMessageW(
        self.control(VIEW_MODE),
        CB_SETCURSEL,
        item.detail as usize,
        0,
      );
      self.anchor = Some(ViewAnchor {
        page: self.page,
        fraction: (0., 0.),
        point: (self.unit(20), self.unit(20)),
      });
    }
  }

  pub(super) unsafe fn recent_menu(&mut self) {
    let menu = GetSubMenu(GetMenu(self.hwnd), 0);
    for i in 0..12 {
      DeleteMenu(menu, (RECENT + i) as u32, MF_BYCOMMAND);
    }
    for (i, item) in self.recent.iter().enumerate() {
      let label = format!(
        "{}  {}",
        i + 1,
        item.path.file_name().unwrap_or_default().to_string_lossy()
      )
      .replace('&', "&&");
      AppendMenuW(menu, MF_STRING, RECENT + i, wide(&label).as_ptr());
    }
  }
}

unsafe fn copy(owner: HWND, value: &str) -> Result<(), String> {
  if OpenClipboard(owner) == 0 {
    return Err("Буфер обмена занят другой программой.".into());
  }
  let result = (|| {
    let value = wide(&value.replace('\n', "\r\n"));
    let handle = GlobalAlloc(GMEM_MOVEABLE, value.len() * 2);
    if handle.is_null() {
      return Err("Недостаточно памяти для копирования.".into());
    }
    let memory = GlobalLock(handle);
    if memory.is_null() {
      GlobalFree(handle);
      return Err("Не удалось скопировать текст.".into());
    }
    ptr::copy_nonoverlapping(value.as_ptr(), memory.cast(), value.len());
    GlobalUnlock(handle);
    if EmptyClipboard() == 0 || SetClipboardData(13, handle).is_null() {
      GlobalFree(handle);
      return Err("Не удалось скопировать текст.".into());
    }
    Ok(())
  })();
  CloseClipboard();
  result
}

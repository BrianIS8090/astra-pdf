use super::*;
use crate::{
  dialogs,
  editing::{Mask, MaskKind, Operation, PageObject},
  engine::Client,
  export, vault,
};
use std::{path::Path, sync::mpsc};

pub const PAGES: usize = 40;
const SELECT: usize = 41;
const CHANGE_TEXT: usize = 42;
const DELETE: usize = 43;
const MASK: usize = 44;
const SAFE_SAVE: usize = 45;
const RESTORE: usize = 46;
const UNDO: usize = 47;
const HISTORY: usize = 48;
const ABOUT: usize = 49;
const VIEW: usize = 50;
const HELP: usize = 51;
const COVER: usize = 52;
const PIXEL_SMALL: usize = 53;
const PIXEL_MEDIUM: usize = 54;
pub const PIXEL_LARGE: usize = 55;

#[derive(Default, PartialEq)]
enum Mode {
  #[default]
  View,
  Select,
  Mask,
  Pixelate,
}
enum ResultItem {
  Objects {
    page: usize,
    point: (f64, f64),
    objects: Vec<PageObject>,
  },
  Copy {
    path: PathBuf,
    previous: PathBuf,
  },
  Info(String),
}
enum JobMessage {
  Done(Result<ResultItem, String>),
  Progress(usize, usize),
}
#[derive(Default)]
pub(super) struct Editor {
  pub fingerprint: Option<[u8; 32]>,
  pub busy: bool,
  mode: Mode,
  pub masks: Vec<Mask>,
  drag: Option<Mask>,
  start: Option<(f64, f64)>,
  selected: Option<(usize, PageObject)>,
  undo: Vec<PathBuf>,
  receiver: Option<mpsc::Receiver<JobMessage>>,
  pub notice: Option<String>,
  pixel_mm: u8,
  pub grids: Option<Cache>,
  mosaic_plan: Vec<RenderKey>,
}

impl Editor {
  pub(super) fn probe(&self) -> serde_json::Value {
    serde_json::json!({
      "masks": self.masks,
      "grids": self.mosaic_plan.iter().map(|k| [k.page as i32, k.width, k.height]).collect::<Vec<_>>(),
      "ready": self.mosaic_plan.iter().filter(|k| self.grids.as_ref().and_then(|c| c.peek(k)).is_some()).count(),
      "bytes": self.grids.as_ref().map_or(0, |c| c.bytes)
    })
  }

  pub(super) fn reset(&mut self) {
    *self = Self {
      pixel_mm: self.pixel_mm,
      ..Self::default()
    };
  }
}

pub(super) unsafe fn menu(hwnd: HWND) {
  let menu = CreateMenu();
  for (name, items) in [
    (
      "Файл",
      vec![
        (OPEN, "Открыть…\tCtrl+O"),
        (PAGES, "Сохранить отдельные страницы…"),
        (PRINT, "Печать…\tCtrl+P"),
      ],
    ),
    (
      "Редактирование",
      vec![
        (VIEW, "Просмотр"),
        (SELECT, "Выбрать объект мышью"),
        (CHANGE_TEXT, "Изменить выбранный текст…"),
        (DELETE, "Удалить выбранный объект…"),
        (UNDO, "Отменить последнее действие"),
      ],
    ),
    (
      "Скрытие информации",
      vec![
        (MASK, "Пикселизация содержимого — выделить область"),
        (COVER, "Полное скрытие — выделить область"),
        (PIXEL_SMALL, "Мелкие блоки · 3 мм"),
        (PIXEL_MEDIUM, "Средние блоки · 6 мм"),
        (PIXEL_LARGE, "Крупные блоки · 12 мм"),
        (SAFE_SAVE, "Сохранить очищенный PDF…"),
        (RESTORE, "Восстановить оригинал по ключу…"),
        (CANCEL, "Отменить текущую операцию"),
      ],
    ),
    (
      "Справка",
      vec![
        (HELP, "Как редактировать и скрывать"),
        (HISTORY, "Что нового"),
        (ABOUT, "О программе"),
      ],
    ),
  ] {
    let popup = CreatePopupMenu();
    for (id, label) in items {
      AppendMenuW(popup, MF_STRING, id, wide(label).as_ptr());
    }
    AppendMenuW(menu, MF_POPUP, popup as usize, wide(name).as_ptr());
  }
  SetMenu(hwnd, menu);
  CheckMenuRadioItem(
    menu,
    PIXEL_SMALL as u32,
    PIXEL_LARGE as u32,
    PIXEL_MEDIUM as u32,
    MF_BYCOMMAND,
  );
}

impl App {
  pub(super) unsafe fn request_mosaics(&mut self) {
    let mut plan = Vec::new();
    for page in self.visible_pages() {
      let Some(&size) = self.sizes.get(page) else {
        continue;
      };
      let mut blocks: Vec<u8> = self
        .editor
        .masks
        .iter()
        .chain(self.editor.drag.iter())
        .filter_map(|m| match m.kind {
          MaskKind::Pixelate { block_mm } if m.page == page => Some(block_mm),
          _ => None,
        })
        .collect();
      if self.editor.mode == Mode::Pixelate {
        blocks.push(if self.editor.pixel_mm == 0 {
          6
        } else {
          self.editor.pixel_mm
        });
      }
      blocks.sort_unstable();
      blocks.dedup();
      for block in blocks {
        if let Ok(key) = crate::pixelate::key(page, size, block, &self.states) {
          plan.push(key);
        }
      }
    }
    if plan == self.editor.mosaic_plan {
      return;
    }
    let cache = self
      .editor
      .grids
      .get_or_insert_with(|| Cache::with_count(16 * 1024 * 1024, 32));
    let missing = plan
      .iter()
      .filter(|key| cache.peek(key).is_none())
      .cloned()
      .collect();
    self.editor.mosaic_plan = plan;
    let _ = self.worker.sender.mosaic_batch(self.generation, missing);
  }

  unsafe fn notice(&mut self, value: String) {
    self.editor.notice = Some(value);
    PostMessageW(self.hwnd, WM_APP + 23, 0, 0);
  }

  unsafe fn job(
    &mut self,
    run: impl FnOnce(Arc<AtomicBool>, mpsc::Sender<JobMessage>) -> Result<ResultItem, String>
      + Send
      + 'static,
  ) {
    if self.editor.busy || self.printing {
      return;
    }
    self.cancel = Arc::new(AtomicBool::new(false));
    let cancel = self.cancel.clone();
    let (sender, receiver) = mpsc::channel();
    self.editor.receiver = Some(receiver);
    self.editor.busy = true;
    self.status = "Выполняю операцию…  ·  Esc — отмена".into();
    self.enable_controls();
    let hwnd = self.hwnd as usize;
    std::thread::spawn(move || {
      let result = run(cancel, sender.clone());
      let _ = sender.send(JobMessage::Done(result));
      unsafe {
        PostMessageW(hwnd as HWND, WM_APP + 22, 0, 0);
      }
    });
    SetTimer(self.hwnd, 4, 200, None);
    InvalidateRect(self.hwnd, ptr::null(), 0);
  }

  pub(super) unsafe fn poll_job(&mut self) {
    while let Some(message) = self
      .editor
      .receiver
      .as_ref()
      .and_then(|r| r.try_recv().ok())
    {
      match message {
        JobMessage::Progress(done, total) => {
          self.status = format!("Сохраняю очищенную копию: {done} / {total}  ·  Esc — отмена");
        }
        JobMessage::Done(result) => {
          self.editor.busy = false;
          self.editor.receiver = None;
          KillTimer(self.hwnd, 4);
          self.enable_controls();
          match result {
            Ok(ResultItem::Info(message)) => {
              self.status = "Сохранено".into();
              self.notice(message);
            }
            Ok(ResultItem::Copy { path, previous }) => {
              let mut undo = std::mem::take(&mut self.editor.undo);
              undo.push(previous);
              let page = self.page;
              self.open(path);
              self.page = page;
              self.editor.undo = undo;
            }
            Ok(ResultItem::Objects {
              page,
              point,
              objects,
            }) => {
              let old = self
                .editor
                .selected
                .as_ref()
                .filter(|(p, _)| *p == page)
                .map(|(_, o)| o.index);
              let mut matches: Vec<_> = objects
                .into_iter()
                .filter(|o| {
                  point.0 >= o.bounds[0]
                    && point.0 <= o.bounds[2]
                    && point.1 >= o.bounds[1]
                    && point.1 <= o.bounds[3]
                })
                .collect();
              matches.sort_by(|a, b| {
                ((a.bounds[2] - a.bounds[0]) * (a.bounds[3] - a.bounds[1]))
                  .total_cmp(&((b.bounds[2] - b.bounds[0]) * (b.bounds[3] - b.bounds[1])))
              });
              let next = old
                .and_then(|id| matches.iter().position(|o| o.index == id))
                .map_or(0, |i| (i + 1) % matches.len().max(1));
              self.editor.selected = matches.get(next).cloned().map(|o| (page, o));
              self.status = if let Some((_, o)) = &self.editor.selected {
                format!(
                  "{} · объект {} · Редактирование → изменить или удалить",
                  kind_name(o.kind),
                  o.index + 1
                )
              } else {
                "Объект не найден. Текст скана является частью изображения.".into()
              };
            }
            Err(error) => {
              self.status = error.clone();
              self.notice(error);
            }
          }
          if self.closing {
            PostQuitMessage(0);
          }
        }
      }
      InvalidateRect(self.hwnd, ptr::null(), 0);
      InvalidateRect(self.canvas, ptr::null(), 0);
    }
  }

  unsafe fn page_point(
    &self,
    point: (i32, i32),
    only: Option<usize>,
  ) -> Option<(usize, (f64, f64))> {
    for page in only.map_or_else(|| self.visible_pages(), |p| vec![p]) {
      let p = self.document_layout.pages.get(page)?;
      let origin = self.page_origin(page);
      let x = (point.0 + self.scroll.0 - origin.0) as f64 / p.width as f64;
      let y = (point.1 + self.scroll.1 - origin.1) as f64 / p.height as f64;
      if only.is_some() || ((0. ..=1.).contains(&x) && (0. ..=1.).contains(&y)) {
        return Some((
          page,
          export::rotate_point((x.clamp(0., 1.), y.clamp(0., 1.)), -self.rotation),
        ));
      }
    }
    None
  }

  pub(super) unsafe fn editor_mouse(&mut self, msg: u32, lp: LPARAM) {
    if self.editor.busy || self.printing || self.dialog_open {
      return;
    }
    let point = ((lp as u16 as i16) as i32, ((lp >> 16) as u16 as i16) as i32);
    if msg == WM_LBUTTONDOWN {
      let Some((page, p)) = self.page_point(point, None) else {
        return;
      };
      if matches!(self.editor.mode, Mode::Mask | Mode::Pixelate) {
        if self.editor.mode == Mode::Pixelate {
          let block = if self.editor.pixel_mm == 0 {
            6
          } else {
            self.editor.pixel_mm
          };
          if let Err(error) = crate::pixelate::key(page, self.sizes[page], block, &self.states) {
            self.notice(error);
            return;
          }
        }
        self.editor.start = Some(p);
        self.editor.drag = Some(Mask {
          kind: if self.editor.mode == Mode::Pixelate {
            MaskKind::Pixelate {
              block_mm: if self.editor.pixel_mm == 0 {
                6
              } else {
                self.editor.pixel_mm
              },
            }
          } else {
            MaskKind::Cover
          },
          page,
          bounds: [p.0, p.1, p.0, p.1],
        });
        SetCapture(self.canvas);
      } else if self.editor.mode == Mode::Select {
        let Some(fingerprint) = self.editor.fingerprint else {
          return;
        };
        let Some(path) = self.path.clone() else {
          return;
        };
        self.job(move |cancel, _| {
          let mut client = Client::spawn(&crate::pdfium_path())?;
          client.open_checked(&path, fingerprint, || cancel.load(Ordering::Relaxed))?;
          let bytes = client.edit(Operation::Objects { page }, || {
            cancel.load(Ordering::Relaxed)
          })?;
          Ok(ResultItem::Objects {
            page,
            point: p,
            objects: serde_json::from_slice(&bytes).map_err(|e| e.to_string())?,
          })
        });
      }
    } else if [WM_MOUSEMOVE, WM_LBUTTONUP].contains(&msg) {
      if let (Some(drag), Some(start)) = (&self.editor.drag, self.editor.start) {
        if let Some((page, p)) = self.page_point(point, Some(drag.page)) {
          let mask = Mask {
            kind: drag.kind,
            page,
            bounds: [
              p.0.min(start.0),
              p.1.min(start.1),
              p.0.max(start.0),
              p.1.max(start.1),
            ],
          };
          self.editor.drag = Some(mask.clone());
          if msg == WM_LBUTTONUP {
            if mask.valid(self.sizes.len()) {
              self.editor.masks.push(mask);
              self.status = format!("Областей скрытия: {}. Выберите «Сохранить очищенный PDF». Метки пока не сохранены.",self.editor.masks.len());
            }
            self.editor.drag = None;
            self.editor.start = None;
            ReleaseCapture();
          }
        }
      }
    } else if msg == WM_CAPTURECHANGED {
      self.editor.drag = None;
      self.editor.start = None;
    }
    InvalidateRect(self.canvas, ptr::null(), 0);
    self.request_mosaics();
    InvalidateRect(self.hwnd, ptr::null(), 0);
  }

  pub(super) unsafe fn editor_paint(&self, dc: HDC) {
    let masks: Vec<_> = self
      .editor
      .masks
      .iter()
      .chain(self.editor.drag.iter())
      .cloned()
      .collect();
    let mut viewport: RECT = std::mem::zeroed();
    GetClientRect(self.canvas, &mut viewport);
    for mask in masks
      .iter()
      .filter(|m| matches!(m.kind, MaskKind::Pixelate { .. }))
      .chain(masks.iter().filter(|m| m.kind == MaskKind::Cover))
    {
      let Some(rect) = self.editor_rect(mask.page, mask.bounds) else {
        continue;
      };
      if let MaskKind::Pixelate { block_mm } = mask.kind {
        let grid = crate::pixelate::key(mask.page, self.sizes[mask.page], block_mm, &self.states)
          .ok()
          .and_then(|key| self.editor.grids.as_ref()?.peek(&key));
        if let Some(grid) = grid {
          let page = self.editor_rect(mask.page, [0., 0., 1., 1.]).unwrap();
          let clip = RECT {
            left: rect.left.max(viewport.left),
            top: rect.top.max(viewport.top),
            right: rect.right.min(viewport.right),
            bottom: rect.bottom.min(viewport.bottom),
          };
          if clip.right <= clip.left || clip.bottom <= clip.top {
            continue;
          }
          let width = f64::from(page.right - page.left);
          let height = f64::from(page.bottom - page.top);
          let visible = export::rotate_bounds(
            [
              f64::from(clip.left - page.left) / width,
              f64::from(clip.top - page.top) / height,
              f64::from(clip.right - page.left) / width,
              f64::from(clip.bottom - page.top) / height,
            ],
            -self.rotation,
          );
          let x0 = (visible[0] * grid.width as f64).floor().max(0.) as i32;
          let y0 = (visible[1] * grid.height as f64).floor().max(0.) as i32;
          let x1 = (visible[2] * grid.width as f64)
            .ceil()
            .min(grid.width as f64) as i32;
          let y1 = (visible[3] * grid.height as f64)
            .ceil()
            .min(grid.height as f64) as i32;
          for y in y0..y1 {
            for x in x0..x1 {
              let cell =
                export::rotate_bounds(crate::pixelate::cell_bounds(grid, x, y), self.rotation);
              let r = RECT {
                left: (page.left + (cell[0] * width).floor() as i32).max(clip.left),
                top: (page.top + (cell[1] * height).floor() as i32).max(clip.top),
                right: (page.left + (cell[2] * width).ceil() as i32).min(clip.right),
                bottom: (page.top + (cell[3] * height).ceil() as i32).min(clip.bottom),
              };
              let c = crate::pixelate::color(grid, x, y, &masks, mask.page);
              fill(
                dc,
                &r,
                u32::from(c[2]) | u32::from(c[1]) << 8 | u32::from(c[0]) << 16,
              );
            }
          }
          continue;
        }
        fill(dc, &rect, 0x00ece9e6);
        text(
          dc,
          rect,
          "Готовлю пикселизацию…",
          0x00786858,
          self.font,
          DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        continue;
      }
      fill(dc, &rect, 0x00505050);
      text(
        dc,
        rect,
        "СКРЫТО",
        0x00ffffff,
        self.font,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
      );
    }
    if let Some((page, object)) = &self.editor.selected {
      if let Some(rect) = self.editor_rect(*page, object.bounds) {
        let pen = CreatePen(PS_SOLID, self.unit(2), 0x00c87818);
        let old_pen = SelectObject(dc, pen);
        let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
        Rectangle(dc, rect.left, rect.top, rect.right, rect.bottom);
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        DeleteObject(pen);
      }
    }
  }

  unsafe fn editor_rect(&self, page: usize, bounds: [f64; 4]) -> Option<RECT> {
    let p = self.document_layout.pages.get(page)?;
    if !self.visible_pages().contains(&page) {
      return None;
    }
    let b = export::rotate_bounds(bounds, self.rotation);
    let origin = self.page_origin(page);
    let (x, y) = (origin.0 - self.scroll.0, origin.1 - self.scroll.1);
    Some(RECT {
      left: x + (b[0] * p.width as f64).floor() as i32,
      top: y + (b[1] * p.height as f64).floor() as i32,
      right: x + (b[2] * p.width as f64).ceil() as i32,
      bottom: y + (b[3] * p.height as f64).ceil() as i32,
    })
  }
}

fn kind_name(kind: i32) -> &'static str {
  match kind {
    1 => "Текст",
    2 => "Контур",
    3 => "Изображение",
    4 => "Заливка",
    5 => "Группа объектов",
    _ => "Объект",
  }
}

pub(super) unsafe fn command(hwnd: HWND, id: usize) {
  let ready = with_app(|a| {
    if a.dialog_open || a.editor.busy || a.printing {
      false
    } else {
      a.dialog_open = true;
      true
    }
  })
  .unwrap_or(false);
  if !ready {
    return;
  }
  let result = command_inner(hwnd, id);
  with_app(|a| {
    a.dialog_open = false;
    if let Err(e) = result {
      a.notice(e);
    }
  });
  PostMessageW(hwnd, WM_APP + 14, 0, 0);
}

unsafe fn command_inner(hwnd: HWND, id: usize) -> Result<(), String> {
  match id {
    HISTORY => {
      dialogs::prompt(
        hwnd,
        &crate::version::title(),
        "История изменений. Все выпуски rc — предварительные.",
        crate::version::CHANGES,
        true,
      );
      return Ok(());
    }
    ABOUT => {
      dialogs::prompt(hwnd,"О программе",&crate::version::title(),"Нативный просмотрщик PDF для Windows 11.\nСтатус: предварительный выпуск.\nhttps://github.com/BrianIS8090/astra-pdf\n\nВекторный просмотр, слои, страницы и печать.\nБезопасное скрытие сохраняется отдельной очищенной копией.",true);
      return Ok(());
    }
    HELP => {
      dialogs::prompt(
        hwnd,
        "Редактирование и скрытие",
        "Исходный файл остаётся неизменным.",
        include_str!("../../docs/EDITING.txt"),
        true,
      );
      return Ok(());
    }
    RESTORE => {
      restore(hwnd);
      return Ok(());
    }
    _ => (),
  }
  let (path, page, count, selected, masks, states, fingerprint) = with_app(|a| {
    (
      a.path.clone(),
      a.page,
      a.sizes.len(),
      a.editor.selected.clone(),
      a.editor.masks.clone(),
      a.states.clone(),
      a.editor.fingerprint,
    )
  })
  .ok_or("Окно закрыто.")?;
  let path = path.ok_or("Сначала откройте PDF.")?;
  let fingerprint = fingerprint.ok_or("Дождитесь загрузки документа.")?;
  if count == 0 {
    return Err("Дождитесь загрузки документа.".into());
  }
  if matches!(id, PAGES | CHANGE_TEXT | DELETE) && !masks.is_empty() {
    return Err("Есть метки скрытия. Сохраните очищенный PDF через меню «Скрытие информации» или отмените метки. Обычная копия их не применяет.".into());
  }
  match id {
    SELECT | MASK | COVER | VIEW => {
      with_app(|a| {
        a.editor.mode = match id {
          SELECT => Mode::Select,
          MASK => Mode::Pixelate,
          COVER => Mode::Mask,
          _ => Mode::View,
        };
        a.editor.selected = None;
        a.status = match id {
          SELECT => "Нажмите на объект. Повторный щелчок выбирает следующий объект под курсором.",
          MASK => "Выделите область для пикселизации. Размер блоков — в меню. Пикселизация не гарантирует секретность.",
          COVER => "Выделите область полного скрытия. Затем сохраните очищенный PDF.",
          _ => "Просмотр",
        }
        .into();
        InvalidateRect(a.hwnd, ptr::null(), 0);
        InvalidateRect(a.canvas, ptr::null(), 0);
        a.request_mosaics();
      });
    }
    PIXEL_SMALL | PIXEL_MEDIUM | PIXEL_LARGE => {
      let block_mm = match id {
        PIXEL_SMALL => 3,
        PIXEL_LARGE => 12,
        _ => 6,
      };
      let valid = with_app(|a| {
        for m in &a.editor.masks {
          if matches!(m.kind, MaskKind::Pixelate { .. }) {
            crate::pixelate::key(m.page, a.sizes[m.page], block_mm, &a.states)?;
          }
        }
        Ok::<_, String>(())
      });
      if let Some(result) = valid {
        result?;
      }
      with_app(|a| {
        a.editor.pixel_mm = block_mm;
        for mask in &mut a.editor.masks {
          if matches!(mask.kind, MaskKind::Pixelate { .. }) {
            mask.kind = MaskKind::Pixelate { block_mm };
          }
        }
        CheckMenuRadioItem(
          GetMenu(hwnd),
          PIXEL_SMALL as u32,
          PIXEL_LARGE as u32,
          id as u32,
          MF_BYCOMMAND,
        );
        a.status = format!("Размер блоков пикселизации: {block_mm} мм на странице. Настройка применена к отмеченным областям.");
        a.request_mosaics();
        InvalidateRect(a.canvas, ptr::null(), 0);
        InvalidateRect(a.hwnd, ptr::null(), 0);
      });
    }
    UNDO => {
      with_app(|a| {
        if a.editor.masks.pop().is_some() {
          a.status = "Последняя метка скрытия отменена.".into();
          InvalidateRect(a.canvas, ptr::null(), 0);
        } else if let Some(previous) = a.editor.undo.pop() {
          let undo = std::mem::take(&mut a.editor.undo);
          a.open(previous);
          a.editor.undo = undo;
        } else {
          a.status = "Нет действий для отмены.".into();
        }
      });
    }
    PAGES => {
      let Some(range) = dialogs::prompt(hwnd,"Сохранить страницы",&format!("Всего страниц: {count}. Введите номера или диапазоны, например: 1, 3-5.\nПорядок как в исходнике; без повторов. Сохраняются исходные слои."),&(page+1).to_string(),false) else { return Ok(()); };
      let pages = crate::editing::pages(&range, count)?;
      let Some(output) = output_pdf(hwnd, &path, "Сохранить выбранные страницы", "страницы")?
      else {
        return Ok(());
      };
      with_app(|a| {
        a.job(move |cancel,_| {
        let mut client=Client::spawn(&crate::pdfium_path())?;
        client.open_checked(&path,fingerprint,||cancel.load(Ordering::Relaxed))?;
        let bytes=client.edit(Operation::Extract{pages},||cancel.load(Ordering::Relaxed))?;
        if cancel.load(Ordering::Relaxed) { return Err("Сохранение отменено.".into()); }
        export::write(&output,&bytes)?;
        Ok(ResultItem::Info(format!("Страницы сохранены:\n{}\n\nЭто обычная PDF-копия с векторной графикой. Экспорт страниц не является очисткой конфиденциальных данных.",output.display())))
      })
      });
    }
    CHANGE_TEXT | DELETE => {
      let (page, object) = selected
        .ok_or("Сначала выберите «Редактирование → Выбрать объект мышью» и нажмите на объект.")?;
      let operation = if id == CHANGE_TEXT {
        if object.kind != 1 {
          return Err("Выбран не текстовый объект. В сканах текст является частью изображения; вложенные группы пока редактируются целиком.".into());
        }
        let Some(text)=dialogs::prompt(hwnd,"Изменить текст","Замена одной строки шрифтом Arial. Размер, положение и цвет сохраняются.\nПеренос строк и исходное начертание не сохраняются. Проверьте результат перед отправкой.",&object.text,false) else { return Ok(()); };
        Operation::Text {
          page,
          object: object.index,
          text,
        }
      } else {
        if MessageBoxW(hwnd,wide(&format!("Удалить: {}?\nБудет сохранена новая копия. Это обычное редактирование; для конфиденциальных данных используйте защищённое скрытие.",kind_name(object.kind))).as_ptr(),wide("Удаление объекта").as_ptr(),MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2)!=IDYES { return Ok(()); }
        Operation::Delete {
          page,
          object: object.index,
        }
      };
      let Some(output) = output_pdf(hwnd, &path, "Сохранить изменённую копию", "правка")?
      else {
        return Ok(());
      };
      with_app(|a| {
        a.job(move |cancel, _| {
          let mut client = Client::spawn(&crate::pdfium_path())?;
          client.open_checked(&path, fingerprint, || cancel.load(Ordering::Relaxed))?;
          let bytes = client.edit(operation, || cancel.load(Ordering::Relaxed))?;
          if cancel.load(Ordering::Relaxed) {
            return Err("Изменение отменено.".into());
          }
          export::write(&output, &bytes)?;
          Ok(ResultItem::Copy {
            path: output,
            previous: path,
          })
        })
      });
    }
    SAFE_SAVE => {
      if masks.is_empty() {
        return Err("Сначала выделите области через меню «Скрытие информации».".into());
      }
      let warning = if masks
        .iter()
        .any(|m| matches!(m.kind, MaskKind::Pixelate { .. }))
      {
        "Пикселизация сохраняет средние цвета содержимого. Некоторые детали и текст могут быть угаданы или распознаны. Для секретных данных используйте «Полное скрытие».\n\n"
      } else {
        ""
      };
      let message = format!("{warning}Копия будет состоять из изображений страниц, 300 dpi. Исходные текстовые объекты, слои, вложения и история в неё не попадут.\n\nОтдельный зашифрованный оригинал и ключ будут сохранены у вас. Не отправляйте их заказчику.\n\nПродолжить?");
      if MessageBoxW(
        hwnd,
        wide(&message).as_ptr(),
        wide("Сохранить PDF с обработанными областями").as_ptr(),
        MB_YESNO | MB_ICONINFORMATION | MB_DEFBUTTON2,
      ) != IDYES
      {
        return Ok(());
      }
      let Some(output) = output_pdf(
        hwnd,
        &path,
        "PDF для заказчика — очищенная копия",
        "очищено",
      )?
      else {
        return Ok(());
      };
      with_app(|a| {
        a.job(move |cancel,progress| {
        use sha2::{Digest,Sha256};
        let mut client=Client::spawn(&crate::pdfium_path())?;
        let meta=client.open_checked(&path,fingerprint,||cancel.load(Ordering::Relaxed))?;
        let original=zeroize::Zeroizing::new(export::read(&path,vault::LIMIT)?);
        if <[u8;32]>::from(Sha256::digest(&*original)) != meta.fingerprint { return Err("Исходный файл изменился. Откройте его повторно.".into()); }
        let key=vault::key_file()?;
        let encrypted=vault::encrypt(&original,&key)?;
        let private=private_dir()?;
        let token=vault::random::<16>()?.iter().map(|b|format!("{b:02x}")).collect::<String>();
        let key_path=private.join("Keys").join(format!("{token}.astrakey"));
        let vault_path=private.join("Originals").join(format!("{token}.astravault"));
        std::fs::create_dir_all(key_path.parent().unwrap()).map_err(|e|e.to_string())?;
        std::fs::create_dir_all(vault_path.parent().unwrap()).map_err(|e|e.to_string())?;
        export::write(&key_path,&key)?;
        export::write(&vault_path,&encrypted)?;
        export::clean_pdf(|key|client.render(key,false,||cancel.load(Ordering::Relaxed)),&meta.sizes,&states,&masks,&output,||cancel.load(Ordering::Relaxed),|done,total| { let _=progress.send(JobMessage::Progress(done,total)); })?;
        Ok(ResultItem::Info(format!("Для заказчика:\n{}\n\nЗащищённый оригинал (сохраните у себя):\n{}\n\nКлюч восстановления (не отправляйте заказчику):\n{}\n\nБез ключа восстановление невозможно. Сделайте отдельную резервную копию ключа. Меню «Восстановить оригинал по ключу» вернёт исходный PDF.",output.display(),vault_path.display(),key_path.display())))
      })
      });
    }
    _ => (),
  }
  Ok(())
}

fn private_dir() -> Result<PathBuf, String> {
  Ok(
    PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("Не найден личный каталог Windows.")?)
      .join("AstraPDF")
      .join("Private"),
  )
}

unsafe fn output_pdf(
  hwnd: HWND,
  source: &Path,
  title: &str,
  suffix: &str,
) -> Result<Option<PathBuf>, String> {
  let name = format!(
    "{}-{suffix}.pdf",
    source.file_stem().unwrap_or_default().to_string_lossy()
  );
  let result = dialogs::file(hwnd, title, "pdf", &source.with_file_name(name), true);
  if result
    .as_ref()
    .is_some_and(|p| export::same_file(p, source))
  {
    return Err("Выберите другое имя: исходный документ не перезаписывается.".into());
  }
  Ok(result)
}

unsafe fn restore(hwnd: HWND) {
  let private = private_dir().unwrap_or_default();
  let Some(source) = dialogs::file(
    hwnd,
    "Выберите защищённый оригинал",
    "astravault",
    &private.join("Originals").join("оригинал.astravault"),
    false,
  ) else {
    return;
  };
  let Some(key) = dialogs::file(
    hwnd,
    "Выберите ключ восстановления",
    "astrakey",
    &private.join("Keys").join("ключ.astrakey"),
    false,
  ) else {
    return;
  };
  let Some(output) = dialogs::file(
    hwnd,
    "Сохранить восстановленный PDF",
    "pdf",
    Path::new("восстановленный.pdf"),
    true,
  ) else {
    return;
  };
  if export::same_file(&output, &source) || export::same_file(&output, &key) {
    with_app(|a| a.notice("Выберите новое имя PDF.".into()));
    return;
  }
  with_app(|a| {
    a.job(move |cancel, _| {
      let key = zeroize::Zeroizing::new(export::read(&key, 40)?);
      let data = export::read(&source, vault::LIMIT + 48)?;
      let original = vault::decrypt(&data, &key)?;
      if cancel.load(Ordering::Relaxed) {
        return Err("Восстановление отменено.".into());
      }
      export::write(&output, &original)?;
      Ok(ResultItem::Info(format!(
        "Оригинал восстановлен:\n{}",
        output.display()
      )))
    })
  });
}

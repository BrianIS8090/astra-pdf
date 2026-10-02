use super::*;
use crate::{
  dialogs,
  editing::{Mask, MaskKind, Operation, PageObject},
  engine::Client,
  export,
  session::{self, Revision, Session},
  vault,
};
use std::{path::Path, sync::mpsc};

pub const PAGES: usize = 40;
pub(super) const SELECT: usize = 41;
pub(super) const CHANGE_TEXT: usize = 42;
pub(super) const DELETE: usize = 43;
pub(super) const MASK: usize = 44;
pub(super) const SAFE_SAVE: usize = 45;
const RESTORE: usize = 46;
pub(super) const UNDO: usize = 47;
const HISTORY: usize = 48;
const ABOUT: usize = 49;
const VIEW: usize = 50;
const HELP: usize = 51;
pub(super) const COVER: usize = 52;
const PIXEL_SMALL: usize = 53;
const PIXEL_MEDIUM: usize = 54;
pub const PIXEL_LARGE: usize = 55;
pub(super) const MASK_SELECT: usize = 56;
pub(super) const MASK_DELETE: usize = 57;
pub(super) const SAVE: usize = 58;
pub(super) const SAVE_AS: usize = 59;

pub(super) enum AfterSave {
  Close,
  Open(PathBuf),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Mode {
  #[default]
  View,
  Select,
  Mask,
  Pixelate,
  Masks,
}
enum MaskUndo {
  Added(usize),
  Removed(usize, Mask),
  Document(Arc<Revision>),
  Masks(Vec<Mask>),
}
enum ResultItem {
  Objects {
    page: usize,
    point: (f64, f64),
    objects: Vec<PageObject>,
  },
  Edited {
    before: Arc<Revision>,
    after: Arc<Revision>,
  },
  Saved {
    path: PathBuf,
    hash: [u8; 32],
  },
  Cleaned(String),
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
  selected_mask: Option<usize>,
  mask_undo: Vec<MaskUndo>,
  drag: Option<Mask>,
  start: Option<(f64, f64)>,
  selected: Option<(usize, PageObject)>,
  session: Option<Session>,
  pub after_save: Option<AfterSave>,
  pub reload_states: Option<Vec<bool>>,
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
      "dirty": self.dirty(),
      "document_dirty": self.session.as_ref().is_some_and(Session::dirty),
      "undo_count": self.mask_undo.len(),
      "mode": format!("{:?}", self.mode),
      "selected_mask": self.selected_mask,
      "selected_object": self.selected.as_ref().map(|(p, o)| serde_json::json!({"page":p,"index":o.index,"kind":o.kind,"bounds":o.bounds})),
      "busy": self.busy,
      "dragging": self.drag.is_some(),
      "grids": self.mosaic_plan.iter().map(|k| [k.page as i32, k.width, k.height]).collect::<Vec<_>>(),
      "ready": self.mosaic_plan.iter().filter(|k| self.grids.as_ref().and_then(|c| c.peek(k)).is_some()).count(),
      "bytes": self.grids.as_ref().map_or(0, |c| c.bytes)
    })
  }

  fn add_mask(&mut self, mask: Mask) {
    self.mask_undo.push(MaskUndo::Added(self.masks.len()));
    self.masks.push(mask);
    self.selected_mask = None;
  }

  fn remove_mask(&mut self) {
    if let Some(index) = self.selected_mask.take().filter(|i| *i < self.masks.len()) {
      self
        .mask_undo
        .push(MaskUndo::Removed(index, self.masks.remove(index)));
    }
  }

  fn undo_mask(&mut self) -> bool {
    if matches!(self.mask_undo.last(), Some(MaskUndo::Document(_))) {
      return false;
    }
    let Some(change) = self.mask_undo.pop() else {
      return false;
    };
    match change {
      MaskUndo::Added(index) => {
        self.masks.remove(index);
      }
      MaskUndo::Removed(index, mask) => self.masks.insert(index, mask),
      MaskUndo::Masks(masks) => self.masks = masks,
      MaskUndo::Document(_) => unreachable!(),
    }
    self.selected_mask = None;
    true
  }

  fn stop_tool(&mut self) {
    self.mode = Mode::View;
    self.drag = None;
    self.start = None;
    self.selected = None;
    self.selected_mask = None;
  }

  pub(super) fn active(&self, id: usize) -> bool {
    matches!(
      (id, self.mode),
      (SELECT, Mode::Select)
        | (MASK, Mode::Pixelate)
        | (COVER, Mode::Mask)
        | (MASK_SELECT, Mode::Masks)
    )
  }

  pub(super) fn can_cancel(&self) -> bool {
    self.busy
      || self.mode != Mode::View
      || self.drag.is_some()
      || self.selected_mask.is_some()
      || self.selected.is_some()
  }

  pub(super) fn can_undo(&self) -> bool {
    !self.mask_undo.is_empty()
  }

  pub(super) fn dirty(&self) -> bool {
    !self.masks.is_empty() || self.session.as_ref().is_some_and(Session::dirty)
  }

  fn trim_history(&mut self) {
    let mut bytes = 0;
    let mut keep = self.mask_undo.len();
    for item in self.mask_undo.iter().rev().take(100) {
      if let MaskUndo::Document(r) = item {
        bytes += r.size;
      }
      if bytes > 256 * 1024 * 1024 {
        break;
      }
      keep -= 1;
    }
    self.mask_undo.drain(..keep);
  }

  pub(super) fn selection_is_mask(&self) -> bool {
    self.selected_mask.is_some()
  }

  pub(super) fn drawing(&self) -> bool {
    matches!(self.mode, Mode::Mask | Mode::Pixelate)
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
        (SAVE, "Сохранить\tCtrl+S"),
        (SAVE_AS, "Сохранить как…\tCtrl+Shift+S"),
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
        (MASK_SELECT, "Выбрать скрывающий блок"),
        (MASK_DELETE, "Убрать выбранный блок\tDelete"),
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
  pub(super) fn document_path(&self) -> Option<PathBuf> {
    self
      .editor
      .session
      .as_ref()
      .map(|s| s.current.path.clone())
      .or_else(|| self.path.clone())
  }

  pub(super) unsafe fn update_title(&self) {
    if let Some(path) = &self.path {
      let title = format!(
        "{}{} — {}",
        if self.editor.dirty() { "* " } else { "" },
        path.file_name().unwrap_or_default().to_string_lossy(),
        crate::version::title()
      );
      SetWindowTextW(self.hwnd, wide(&title).as_ptr());
    }
  }

  unsafe fn reload_revision(&mut self) {
    let Some(path) = self.document_path() else {
      return;
    };
    let logical = self.path.clone();
    let mut editor = std::mem::take(&mut self.editor);
    editor.selected = None;
    editor.grids = None;
    editor.mosaic_plan.clear();
    editor.reload_states = Some(self.states.clone());
    let (page, zoom, rotation, scroll) = (self.page, self.zoom, self.rotation, self.scroll);
    self.open(path);
    self.path = logical;
    self.editor = editor;
    self.page = page;
    self.zoom = zoom;
    self.rotation = rotation;
    self.scroll = scroll;
    self.update_title();
  }

  pub(super) unsafe fn continue_after_save(&mut self) {
    match self.editor.after_save.take() {
      Some(AfterSave::Close) => PostQuitMessage(0),
      Some(AfterSave::Open(path)) => self.open(path),
      None => (),
    }
  }
  pub(super) unsafe fn cancel_editor(&mut self) {
    self.editor.stop_tool();
    ReleaseCapture();
    if self.printing || self.editor.busy {
      self.cancel.store(true, Ordering::Relaxed);
      self.status = "Отменяю операцию…".into();
    } else {
      self.status =
        "Просмотр · инструмент выключен. Созданные блоки сохранены в этом сеансе.".into();
    }
    self.request_mosaics();
    self.sync_editor_controls();
    SetFocus(self.canvas);
    InvalidateRect(self.canvas, ptr::null(), 0);
  }

  pub(super) unsafe fn sync_editor_controls(&self) {
    self.update_title();
    self.enable_controls();
    for id in [SELECT, MASK, COVER, MASK_SELECT, UNDO, SAVE, CANCEL] {
      CheckMenuItem(
        GetMenu(self.hwnd),
        id as u32,
        MF_BYCOMMAND
          | if self.editor.active(id) {
            MF_CHECKED
          } else {
            MF_UNCHECKED
          },
      );
      InvalidateRect(self.control(id), ptr::null(), 1);
    }
    self.position_editor_actions();
  }

  pub(super) unsafe fn position_editor_actions(&self) {
    let target = if self.editor.busy || self.printing || self.dialog_open {
      None
    } else if let Some(mask) = self
      .editor
      .selected_mask
      .and_then(|i| self.editor.masks.get(i))
    {
      self
        .editor_rect(mask.page, mask.bounds)
        .map(|r| (r, true, false))
    } else {
      self.editor.selected.as_ref().and_then(|(p, o)| {
        self
          .editor_rect(*p, o.bounds)
          .map(|r| (r, false, o.kind == 1))
      })
    };
    let view = self.viewport();
    let target =
      target.filter(|(r, _, _)| r.right > 0 && r.left < view.0 && r.bottom > 0 && r.top < view.1);
    let Some((r, mask, is_text)) = target else {
      for id in [CHANGE_TEXT, DELETE, MASK_DELETE] {
        ShowWindow(self.control(id), SW_HIDE);
      }
      return;
    };
    let button_width = self.unit(if mask { 142 } else { 112 });
    let gap = self.unit(4);
    let count = if mask { 1 } else { 2 };
    let width = count * button_width + (count - 1) * gap;
    let height = self.unit(36);
    let x = (r.left.max(0) + (r.right.min(view.0) - r.left.max(0) - width) / 2).clamp(
      self.unit(4),
      (view.0 - width - self.unit(4)).max(self.unit(4)),
    );
    let y = if r.top >= height + gap {
      r.top - height - gap
    } else {
      (r.bottom + gap).min(view.1 - height - gap).max(gap)
    };
    let ids: &[usize] = if mask {
      &[MASK_DELETE]
    } else {
      &[CHANGE_TEXT, DELETE]
    };
    for id in [CHANGE_TEXT, DELETE, MASK_DELETE] {
      if !ids.contains(&id) {
        ShowWindow(self.control(id), SW_HIDE);
      }
    }
    for (i, id) in ids.iter().enumerate() {
      let h = self.control(*id);
      MoveWindow(
        h,
        x + i as i32 * (button_width + gap),
        y,
        button_width,
        height,
        1,
      );
      EnableWindow(h, (*id != CHANGE_TEXT || is_text) as i32);
      SetWindowPos(
        h,
        HWND_TOP,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
      );
    }
  }
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
    self.position_editor_actions();
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
            Ok(ResultItem::Edited { before, after }) => {
              let saved_hash = self
                .editor
                .session
                .as_ref()
                .map_or(before.hash, |s| s.saved_hash);
              self.editor.mask_undo.push(MaskUndo::Document(before));
              self.editor.trim_history();
              self.editor.session = Some(Session {
                current: after,
                saved_hash,
              });
              self.reload_revision();
              self.status = "Изменено · Ctrl+Z — отменить · Ctrl+S — сохранить".into();
            }
            Ok(ResultItem::Saved { path, hash }) => {
              self.path = Some(path);
              if let Some(session) = &mut self.editor.session {
                session.saved_hash = hash;
              }
              self.status = "Сохранено".into();
              self.continue_after_save();
            }
            Ok(ResultItem::Cleaned(message)) => {
              self.status = "Очищенный PDF сохранён".into();
              self.notice(message);
            }
            Ok(ResultItem::Objects {
              page,
              point,
              objects,
            }) => {
              if self.cancel.load(Ordering::Relaxed) || self.editor.mode != Mode::Select {
                self.status = "Выбор объекта отменён.".into();
                self.sync_editor_controls();
                if self.closing {
                  self.closing = false;
                  PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
                }
                continue;
              }
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
              self.editor.after_save = None;
              self.status = error.clone();
              if !self.cancel.load(Ordering::Relaxed) {
                self.notice(error);
              }
            }
          }
          if self.closing {
            self.closing = false;
            PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
          }
          self.sync_editor_controls();
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
    // Перестановка кнопок порождает новые сообщения мыши. Без активной рамки
    // они не меняют редактор; повторная перерисовка может вытеснить таймер масштаба.
    if msg != WM_LBUTTONDOWN && self.editor.drag.is_none() {
      return;
    }
    let point = ((lp as u16 as i16) as i32, ((lp >> 16) as u16 as i16) as i32);
    if msg == WM_LBUTTONDOWN {
      self.editor.selected_mask = None;
      let Some((page, p)) = self.page_point(point, None) else {
        self.editor.selected = None;
        self.sync_editor_controls();
        InvalidateRect(self.canvas, ptr::null(), 0);
        return;
      };
      if matches!(self.editor.mode, Mode::View | Mode::Masks) {
        self.editor.selected = None;
        self.editor.selected_mask = self
          .editor
          .masks
          .iter()
          .enumerate()
          .rev()
          .filter(|(_, m)| {
            m.page == page
              && p.0 >= m.bounds[0]
              && p.0 <= m.bounds[2]
              && p.1 >= m.bounds[1]
              && p.1 <= m.bounds[3]
          })
          .max_by_key(|(i, m)| (m.kind == MaskKind::Cover, *i))
          .map(|(i, _)| i);
        self.status = if self.editor.selected_mask.is_some() {
          "Выбран скрывающий блок · Delete или «Убрать блок» · Esc — снять выделение"
        } else {
          "Нажмите на скрывающий блок, чтобы убрать его."
        }
        .into();
      }
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
        let Some(path) = self.document_path() else {
          return;
        };
        self.editor.selected_mask = None;
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
              self.editor.add_mask(mask);
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
    self.sync_editor_controls();
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
    let selection = self
      .editor
      .selected_mask
      .and_then(|i| self.editor.masks.get(i))
      .map(|m| (m.page, m.bounds))
      .or_else(|| self.editor.selected.as_ref().map(|(p, o)| (*p, o.bounds)));
    if let Some((page, bounds)) = selection {
      if let Some(rect) = self.editor_rect(page, bounds) {
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
    a.sync_editor_controls();
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
        "Правки сохраняются по Ctrl+S. Ctrl+Z — отмена.",
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
      a.document_path(),
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
  let logical = with_app(|a| a.path.clone())
    .flatten()
    .ok_or("Сначала откройте PDF.")?;
  if matches!(id, SAVE | SAVE_AS) {
    return save_document(hwnd, id == SAVE_AS);
  }
  let fingerprint = fingerprint.ok_or("Дождитесь загрузки документа.")?;
  if count == 0 {
    return Err("Дождитесь загрузки документа.".into());
  }
  if matches!(id, PAGES | CHANGE_TEXT | DELETE) && !masks.is_empty() {
    return Err("Есть метки скрытия. Сохраните очищенный PDF через меню «Скрытие информации» или отмените метки. Обычная копия их не применяет.".into());
  }
  match id {
    SELECT | MASK | COVER | MASK_SELECT | VIEW => {
      with_app(|a| {
        let next = match id {
          SELECT => Mode::Select,
          MASK => Mode::Pixelate,
          COVER => Mode::Mask,
          MASK_SELECT => Mode::Masks,
          _ => Mode::View,
        };
        let active = a.editor.mode == next;
        a.editor.stop_tool();
        ReleaseCapture();
        a.editor.mode = if active { Mode::View } else { next };
        a.status = match if active { VIEW } else { id } {
          SELECT => "Нажмите на объект. Повторный щелчок выбирает следующий объект под курсором.",
          MASK => "Выделите область для пикселизации. Размер блоков — в меню. Пикселизация не гарантирует секретность.",
          COVER => "Выделите область полного скрытия. Затем сохраните очищенный PDF.",
          MASK_SELECT => "Нажмите на блок. Delete или кнопка над выделением убирает только выбранный блок. Esc — просмотр.",
          _ => "Просмотр",
        }
        .into();
        InvalidateRect(a.hwnd, ptr::null(), 0);
        InvalidateRect(a.canvas, ptr::null(), 0);
        a.request_mosaics();
        SetFocus(a.canvas);
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
        if a
          .editor
          .masks
          .iter()
          .any(|m| matches!(m.kind, MaskKind::Pixelate { block_mm: old } if old != block_mm))
        {
          a.editor
            .mask_undo
            .push(MaskUndo::Masks(a.editor.masks.clone()));
        }
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
        if a.editor.undo_mask() {
          a.status = "Последнее изменение скрывающих блоков отменено.".into();
          InvalidateRect(a.canvas, ptr::null(), 0);
        } else if let Some(MaskUndo::Document(previous)) = a.editor.mask_undo.pop() {
          if let Some(session) = &mut a.editor.session {
            session.current = previous;
          }
          a.reload_revision();
        } else {
          a.status = "Нет действий для отмены.".into();
        }
        a.request_mosaics();
      });
    }
    MASK_DELETE => {
      with_app(|a| {
        a.editor.remove_mask();
        SetFocus(a.canvas);
        a.status = "Блок убран. Ctrl+Z — вернуть его. Остальные блоки сохранены.".into();
        a.request_mosaics();
        InvalidateRect(a.canvas, ptr::null(), 0);
      });
    }
    PAGES => {
      let Some(range) = dialogs::prompt(hwnd,"Сохранить страницы",&format!("Всего страниц: {count}. Введите номера или диапазоны, например: 1, 3-5.\nПорядок как в исходнике; без повторов. Сохраняются исходные слои."),&(page+1).to_string(),false) else { return Ok(()); };
      let pages = crate::editing::pages(&range, count)?;
      let Some(output) = output_pdf(hwnd, &logical, "Сохранить выбранные страницы", "страницы")?
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
        if text == object.text {
          return Ok(());
        }
        Operation::Text {
          page,
          object: object.index,
          text,
        }
      } else {
        Operation::Delete {
          page,
          object: object.index,
        }
      };
      let previous = with_app(|a| a.editor.session.as_ref().map(|s| s.current.clone())).flatten();
      with_app(|a| {
        a.job(move |cancel, _| {
          let before = if let Some(previous) = previous {
            previous
          } else {
            let original = export::read(&path, vault::LIMIT)?;
            let revision = Revision::new(&original)?;
            if revision.hash != fingerprint {
              return Err("Исходный файл изменился. Откройте его повторно.".into());
            }
            revision
          };
          let mut client = Client::spawn(&crate::pdfium_path())?;
          client.open_checked(&path, fingerprint, || cancel.load(Ordering::Relaxed))?;
          let bytes = client.edit(operation, || cancel.load(Ordering::Relaxed))?;
          if cancel.load(Ordering::Relaxed) {
            return Err("Изменение отменено.".into());
          }
          let after = Revision::new(&bytes)?;
          if cancel.load(Ordering::Relaxed) {
            return Err("Изменение отменено.".into());
          }
          Ok(ResultItem::Edited { before, after })
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
        &logical,
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
        Ok(ResultItem::Cleaned(format!("Для заказчика:\n{}\n\nЗащищённый оригинал (сохраните у себя):\n{}\n\nКлюч восстановления (не отправляйте заказчику):\n{}\n\nБез ключа восстановление невозможно. Сделайте отдельную резервную копию ключа. Меню «Восстановить оригинал по ключу» вернёт исходный PDF.",output.display(),vault_path.display(),key_path.display())))
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

unsafe fn save_document(hwnd: HWND, save_as: bool) -> Result<(), String> {
  let (logical, source, expected, masks) = with_app(|a| {
    (
      a.path.clone(),
      a.document_path(),
      a.editor
        .session
        .as_ref()
        .map(|s| s.saved_hash)
        .or(a.editor.fingerprint),
      !a.editor.masks.is_empty(),
    )
  })
  .ok_or("Окно закрыто.")?;
  if masks {
    return command_inner(hwnd, SAFE_SAVE);
  }
  let logical = logical.ok_or("Сначала откройте PDF.")?;
  let source = source.ok_or("Сначала откройте PDF.")?;
  let expected = expected.ok_or("Дождитесь загрузки документа.")?;
  let destination = if save_as {
    let Some(path) = dialogs::file(hwnd, "Сохранить как", "pdf", &logical, true) else {
      return Ok(());
    };
    path
  } else {
    logical.clone()
  };
  let expected = if export::same_file(&destination, &logical) {
    Some(expected)
  } else if destination.exists() {
    Some(session::hash_file(&destination)?)
  } else {
    None
  };
  with_app(|a| {
    a.job(move |cancel, _| {
      let hash = session::save(&source, &destination, expected, || {
        cancel.load(Ordering::Relaxed)
      })?;
      Ok(ResultItem::Saved {
        path: destination,
        hash,
      })
    })
  });
  Ok(())
}

pub(super) unsafe fn leave_document(hwnd: HWND, next: AfterSave) {
  let allowed = with_app(|a| {
    if a.dialog_open {
      return false;
    }
    if a.editor.busy || a.printing {
      if matches!(next, AfterSave::Close) {
        a.closing = true;
        a.cancel.store(true, Ordering::Relaxed);
      }
      return false;
    }
    a.dialog_open = true;
    true
  })
  .unwrap_or(false);
  if !allowed {
    return;
  }
  let dirty = with_app(|a| a.editor.dirty()).unwrap_or(false);
  let answer = if dirty {
    let masks = with_app(|a| !a.editor.masks.is_empty()).unwrap_or(false);
    MessageBoxW(hwnd, wide(if masks {
      "Сохранить изменения перед закрытием документа?\nОбласти скрытия будут сохранены отдельным очищенным PDF.\n\nДа — сохранить; Нет — не сохранять; Отмена — продолжить работу."
    } else {
      "Сохранить изменения перед закрытием документа?\n\nДа — сохранить; Нет — не сохранять; Отмена — продолжить работу."
    }).as_ptr(), wide("Несохранённые изменения").as_ptr(), MB_YESNOCANCEL | MB_ICONQUESTION)
  } else {
    IDNO
  };
  if answer == IDYES {
    with_app(|a| a.editor.after_save = Some(next));
    let result = save_document(hwnd, false);
    with_app(|a| {
      if let Err(e) = result {
        a.notice(e);
      }
      if !a.editor.busy {
        a.editor.after_save = None;
      }
    });
  } else if answer == IDNO {
    with_app(|a| {
      a.editor.after_save = Some(next);
      a.continue_after_save();
    });
  }
  with_app(|a| {
    a.dialog_open = false;
    a.sync_editor_controls();
  });
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

#[cfg(test)]
mod tests {
  use super::*;

  fn mask(left: f64) -> Mask {
    Mask {
      page: 0,
      bounds: [left, 0.1, left + 0.1, 0.5],
      kind: MaskKind::Cover,
    }
  }

  #[test]
  fn removing_any_block_and_undo_preserves_order_and_other_blocks() {
    let mut editor = Editor::default();
    editor.add_mask(mask(0.1));
    editor.add_mask(mask(0.3));
    editor.add_mask(mask(0.5));
    editor.selected_mask = Some(1);
    editor.remove_mask();
    assert_eq!(
      editor.masks.iter().map(|m| m.bounds[0]).collect::<Vec<_>>(),
      vec![0.1, 0.5]
    );
    assert!(editor.undo_mask());
    assert_eq!(
      editor.masks.iter().map(|m| m.bounds[0]).collect::<Vec<_>>(),
      vec![0.1, 0.3, 0.5]
    );
    assert!(editor.undo_mask());
    assert!(editor.undo_mask());
    assert_eq!(editor.masks.len(), 1);
    assert!(editor.undo_mask());
    assert!(editor.masks.is_empty());
    assert!(!editor.undo_mask());
  }

  #[test]
  fn escape_discards_unfinished_drag_but_preserves_committed_masks_and_history() {
    let mut editor = Editor::default();
    editor.add_mask(mask(0.1));
    editor.mode = Mode::Pixelate;
    editor.drag = Some(mask(0.3));
    editor.start = Some((0.3, 0.1));
    editor.selected_mask = Some(0);
    editor.stop_tool();
    assert_eq!(editor.mode, Mode::View);
    assert!(editor.drag.is_none() && editor.start.is_none() && editor.selected_mask.is_none());
    assert!(!editor.can_cancel());
    assert_eq!(editor.masks.len(), 1);
    assert!(editor.undo_mask());
    assert!(editor.masks.is_empty());
  }

  #[test]
  fn document_and_mask_undo_follow_actual_order() {
    let revision = Revision::new(b"test revision").unwrap();
    let mut editor = Editor::default();
    editor.add_mask(mask(0.1));
    editor.selected_mask = Some(0);
    editor.remove_mask();
    editor.mask_undo.push(MaskUndo::Document(revision));
    assert!(!editor.undo_mask());
    assert!(matches!(
      editor.mask_undo.pop(),
      Some(MaskUndo::Document(_))
    ));
    assert!(editor.undo_mask());
    assert_eq!(editor.masks.len(), 1);
    assert!(editor.undo_mask());
    assert!(editor.masks.is_empty());
  }
}

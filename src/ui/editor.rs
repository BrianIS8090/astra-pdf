use super::*;
use crate::{
  dialogs,
  editing::{Mask, MaskKind, Operation, PageObject},
  engine::Client,
  export, recovery,
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
pub(super) const REDO: usize = 60;
pub(super) const RECOVER: usize = 61;

pub(super) enum AfterSave {
  Close,
  Open(PathBuf),
  Recover(Box<recovery::Entry>),
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Change {
  Overlays,
  Page(usize),
  Document,
}
enum MaskUndo {
  Added(usize),
  Removed(usize, Mask),
  Document(Arc<Revision>, Vec<Mask>, &'static str, Change),
  Masks(Vec<Mask>),
}
struct ObjectDrag {
  page: usize,
  index: usize,
  original: [f64; 4],
  bounds: [f64; 4],
  start: (f64, f64),
  resize: bool,
}
enum ResultItem {
  Recovered(Box<recovery::Recovered>),
  Objects {
    page: usize,
    point: (f64, f64),
    objects: Vec<PageObject>,
  },
  Edited {
    label: &'static str,
    before: Arc<Revision>,
    after: Arc<Revision>,
    page_change: Option<(usize, Vec<Mask>)>,
    change: Change,
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
  mask_redo: Vec<MaskUndo>,
  drag: Option<Mask>,
  start: Option<(f64, f64)>,
  selected: Option<(usize, PageObject)>,
  object_drag: Option<ObjectDrag>,
  session: Option<Session>,
  recovery: Option<recovery::Writer>,
  recovery_error: Option<String>,
  pub after_save: Option<AfterSave>,
  pub reload_states: Option<Vec<bool>>,
  receiver: Option<mpsc::Receiver<JobMessage>>,
  pub notice: Option<String>,
  pixel_mm: u8,
  pub grids: Option<Cache>,
  mosaic_plan: Vec<RenderKey>,
}

impl Editor {
  pub(super) fn revision(&self) -> Option<Arc<Revision>> {
    self.session.as_ref().map(|s| s.current.clone())
  }
  pub(super) fn viewing(&self) -> bool {
    self.mode == Mode::View
  }
  pub(super) fn history_description(&self) -> String {
    let describe = |item: &MaskUndo| match item {
      MaskUndo::Added(_) => "Добавление скрывающего блока",
      MaskUndo::Removed(..) => "Удаление скрывающего блока",
      MaskUndo::Masks(_) => "Настройка пикселизации",
      MaskUndo::Document(_, _, label, _) => *label,
    };
    let mut text =
      String::from("История текущего сеанса\n\nВыполненные действия (сначала последнее):\n");
    for (i, item) in self.mask_undo.iter().rev().enumerate() {
      text.push_str(&format!("{}. {}\n", i + 1, describe(item)));
    }
    if self.mask_undo.is_empty() {
      text.push_str("Нет действий.\n");
    }
    text.push_str("\nМожно повторить:\n");
    for (i, item) in self.mask_redo.iter().rev().enumerate() {
      text.push_str(&format!("{}. {}\n", i + 1, describe(item)));
    }
    text.push_str("\nCtrl+Z — отменить, Ctrl+Y — повторить.\nХранятся последние 100 действий, не более 256 МиБ копий PDF.\nПосле восстановления сеанса прежняя история правок PDF недоступна.");
    text
  }
  pub(super) fn probe(&self) -> serde_json::Value {
    serde_json::json!({
      "masks": self.masks,
      "dirty": self.dirty(),
      "document_dirty": self.session.as_ref().is_some_and(Session::dirty),
      "undo_count": self.mask_undo.len(),
      "redo_count": self.mask_redo.len(),
      "recovery_saved": self.recovery.as_ref().is_some_and(|r| r.status().0),
      "recovery_error": self.recovery_error,
      "mode": format!("{:?}", self.mode),
      "selected_mask": self.selected_mask,
      "selected_object": self.selected.as_ref().map(|(p, o)| serde_json::json!({"page":p,"index":o.index,"kind":o.kind,"bounds":o.bounds})),
      "busy": self.busy,
      "dragging": self.drag.is_some(),
      "object_dragging":self.object_drag.is_some(),
      "grids": self.mosaic_plan.iter().map(|k| [k.page as i32, k.width, k.height]).collect::<Vec<_>>(),
      "ready": self.mosaic_plan.iter().filter(|k| self.grids.as_ref().and_then(|c| c.peek(k)).is_some()).count(),
      "bytes": self.grids.as_ref().map_or(0, |c| c.bytes)
    })
  }

  fn record(&mut self, change: MaskUndo) {
    self.mask_redo.clear();
    self.mask_undo.push(change);
    self.trim_history();
  }

  fn add_mask(&mut self, mask: Mask) {
    self.record(MaskUndo::Added(self.masks.len()));
    self.masks.push(mask);
    self.selected_mask = None;
  }

  fn remove_mask(&mut self) {
    if let Some(index) = self.selected_mask.take().filter(|i| *i < self.masks.len()) {
      let mask = self.masks.remove(index);
      self.record(MaskUndo::Removed(index, mask));
    }
  }

  fn history_step(&mut self, redo: bool) -> Option<Change> {
    let change = if redo {
      self.mask_redo.pop()?
    } else {
      self.mask_undo.pop()?
    };
    let scope = match &change {
      MaskUndo::Document(_, _, _, scope) => *scope,
      _ => Change::Overlays,
    };
    let inverse = match change {
      MaskUndo::Added(index) => MaskUndo::Removed(index, self.masks.remove(index)),
      MaskUndo::Removed(index, mask) => {
        self.masks.insert(index, mask);
        MaskUndo::Added(index)
      }
      MaskUndo::Masks(masks) => MaskUndo::Masks(std::mem::replace(&mut self.masks, masks)),
      MaskUndo::Document(previous, masks, label, scope) => {
        let session = self.session.as_mut()?;
        MaskUndo::Document(
          std::mem::replace(&mut session.current, previous),
          std::mem::replace(&mut self.masks, masks),
          label,
          scope,
        )
      }
    };
    if redo {
      self.mask_undo.push(inverse);
    } else {
      self.mask_redo.push(inverse);
    }
    self.trim_history();
    self.selected_mask = None;
    self.selected = None;
    self.drag = None;
    self.start = None;
    self.object_drag = None;
    Some(scope)
  }

  #[cfg(test)]
  fn undo_mask(&mut self) -> bool {
    if matches!(self.mask_undo.last(), Some(MaskUndo::Document(..))) {
      return false;
    }
    self.history_step(false).is_some()
  }

  fn stop_tool(&mut self) {
    self.mode = Mode::View;
    self.drag = None;
    self.start = None;
    self.selected = None;
    self.selected_mask = None;
    self.object_drag = None;
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

  pub(super) fn can_redo(&self) -> bool {
    !self.mask_redo.is_empty()
  }

  pub(super) fn dirty(&self) -> bool {
    !self.masks.is_empty() || self.session.as_ref().is_some_and(Session::dirty)
  }

  fn trim_history(&mut self) {
    let size = |item: &MaskUndo| match item {
      MaskUndo::Document(r, _, _, _) => r.size,
      _ => 0,
    };
    let mut bytes = self
      .mask_undo
      .iter()
      .chain(&self.mask_redo)
      .map(size)
      .sum::<usize>();
    while self.mask_undo.len() + self.mask_redo.len() > 100 || bytes > 256 * 1024 * 1024 {
      // Удаляем наиболее далёкое действие, сохраняя непрерывность обеих цепочек.
      let history = if self.mask_undo.len() >= self.mask_redo.len() {
        &mut self.mask_undo
      } else {
        &mut self.mask_redo
      };
      bytes -= size(&history.remove(0));
    }
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
        (RECOVER, "Восстановить сеанс после сбоя…"),
        (PRINT, "Печать…\tCtrl+P"),
      ],
    ),
    (
      "Редактирование",
      vec![
        (VIEW, "Просмотр"),
        (reading::FIND, "Найти в документе…\tCtrl+F"),
        (reading::TEXT, "Выделить текст"),
        (reading::COPY, "Копировать выделенный текст\tCtrl+C"),
        (reading::BOOKMARKS, "Закладки PDF"),
        (reading::HISTORY, "История действий"),
        (SELECT, "Выбрать объект мышью"),
        (CHANGE_TEXT, "Изменить выбранный текст…"),
        (DELETE, "Удалить выбранный объект…"),
        (UNDO, "Отменить действие\tCtrl+Z"),
        (REDO, "Повторить действие\tCtrl+Y"),
      ],
    ),
    (
      "Страницы",
      vec![
        (PAGES, "Сохранить выбранные страницы в отдельный PDF…"),
        (pages::DUPLICATE, "Дублировать текущую страницу"),
        (pages::REMOVE, "Удалить текущую страницу"),
        (pages::INSERT, "Вставить страницы из PDF…"),
        (pages::MERGE, "Объединить с PDF…"),
        (pages::UP, "Переместить выше"),
        (pages::DOWN, "Переместить ниже"),
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
      "Замечания",
      vec![
        (review::TOOLS, "Панель замечаний и измерений"),
        (review::COMMENT, "Комментарий"),
        (review::RECTANGLE, "Прямоугольник"),
        (review::ARROW, "Стрелка"),
        (review::MARKER, "Маркер"),
        (review::INK, "Карандаш — рисовать от руки"),
        (review::TEXT, "Добавить цветную текстовую заметку"),
        (review::DISTANCE, "Измерить расстояние"),
        (review::AREA, "Измерить площадь"),
        (review::SCALE, "Масштаб чертежа…"),
      ],
    ),
    (
      "Справка",
      vec![
        (HELP, "Как редактировать и скрывать"),
        (HISTORY, "Что нового"),
        (updates::CHECK, "Проверить обновления…"),
        (ABOUT, "О программе"),
      ],
    ),
    ("Вид", vec![(THEME, "Тёмная тема")]),
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
  pub(super) unsafe fn commit_revision(
    &mut self,
    before: Arc<Revision>,
    after: Arc<Revision>,
    page: usize,
  ) {
    if before.hash == after.hash {
      return;
    }
    let saved_hash = self
      .editor
      .session
      .as_ref()
      .map_or(before.hash, |s| s.saved_hash);
    self.editor.record(MaskUndo::Document(
      before,
      self.editor.masks.clone(),
      "Изменение текста",
      Change::Page(page),
    ));
    self.editor.session = Some(Session {
      current: after,
      saved_hash,
    });
    self.reload_revision(Change::Page(page));
    self.status = "Текст изменён · Ctrl+Z — отменить · Ctrl+S — сохранить".into();
  }
  pub(super) unsafe fn apply_operation(
    &mut self,
    operation: Operation,
    page_change: Option<(usize, Vec<Mask>)>,
  ) {
    let Some(path) = self.document_path() else {
      return;
    };
    let Some(fingerprint) = self.editor.fingerprint else {
      return;
    };
    let previous = self.editor.revision();
    let masks = self.editor.masks.clone();
    let pages = self.sizes.len();
    let label = match &operation {
      Operation::Text { .. } | Operation::FontText { .. } => "Изменение текста",
      Operation::Delete { .. } => "Удаление объекта",
      Operation::Transform { .. } => "Перемещение или размер объекта",
      Operation::Annotate { .. } => "Замечание или измерение",
      Operation::NoteDelete { .. } => "Удаление замечания",
      Operation::NoteText { .. } => "Изменение комментария",
      Operation::PageOrder { order } if order.len() < pages => "Удаление страницы",
      Operation::PageOrder { order } if order.len() > pages => "Дублирование страницы",
      Operation::PageOrder { .. } => "Перестановка страниц",
      Operation::InsertPages { .. } => "Вставка страниц PDF",
      _ => "Изменение PDF",
    };
    let inserted = match &operation {
      Operation::InsertPages { at, .. } => Some(*at),
      _ => None,
    };
    let change = match &operation {
      Operation::Text { page, .. }
      | Operation::FontText { page, .. }
      | Operation::Delete { page, .. }
      | Operation::Transform { page, .. }
      | Operation::NoteText { page, .. }
      | Operation::NoteDelete { page, .. } => Change::Page(*page),
      Operation::Annotate { note } => Change::Page(note.page),
      _ => Change::Document,
    };
    self.job(move |cancel, _| {
      let before = if let Some(previous) = previous {
        previous
      } else {
        let revision = Revision::new(&export::read(&path, vault::LIMIT)?)?;
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
      let page_change = if let Some(at) = inserted {
        let meta =
          client.open_checked(&after.path, after.hash, || cancel.load(Ordering::Relaxed))?;
        let count = meta
          .sizes
          .len()
          .checked_sub(pages)
          .ok_or("Вставка не добавила страницы.")?;
        let masks = masks
          .into_iter()
          .map(|mut m| {
            if m.page >= at {
              m.page += count;
            }
            m
          })
          .collect();
        Some((at, masks))
      } else {
        page_change
      };
      if cancel.load(Ordering::Relaxed) {
        return Err("Изменение отменено.".into());
      }
      Ok(ResultItem::Edited {
        label,
        before,
        after,
        page_change,
        change,
      })
    });
  }

  pub(super) fn document_path(&self) -> Option<PathBuf> {
    self
      .editor
      .session
      .as_ref()
      .map(|s| s.current.path.clone())
      .or_else(|| self.path.clone())
  }

  pub(super) unsafe fn checkpoint(&mut self) {
    if self.editor.busy || self.sizes.is_empty() {
      return;
    }
    if !self.editor.dirty() {
      self.editor.recovery = None;
      self.editor.recovery_error = None;
      return;
    }
    let (Some(path), Some(source), Some(current_hash)) = (
      self.path.clone(),
      self.document_path(),
      self.editor.fingerprint,
    ) else {
      return;
    };
    let revision = self.editor.session.as_ref().map(|s| s.current.clone());
    if revision.as_ref().is_some_and(|r| r.hash != current_hash) {
      return;
    }
    let result = (|| {
      if self.editor.recovery.is_none() {
        self.editor.recovery = Some(recovery::Writer::new()?);
      }
      let writer = self.editor.recovery.as_ref().unwrap();
      writer.submit(recovery::Snapshot {
        source,
        revision,
        metadata: recovery::Metadata {
          path,
          current_hash,
          saved_hash: self
            .editor
            .session
            .as_ref()
            .map_or(current_hash, |s| s.saved_hash),
          masks: self.editor.masks.clone(),
          states: self.states.clone(),
          page: self.page,
          pages: self.sizes.len(),
          zoom: self.zoom,
          rotation: self.rotation,
          continuous: self.continuous,
          detail_mode: self.detail_mode,
          pixel_mm: self.editor.pixel_mm,
        },
      });
      if let Some(error) = writer.status().1 {
        return Err(error);
      }
      Ok::<_, String>(())
    })();
    if let Err(error) = result {
      if self.editor.recovery_error.as_ref() != Some(&error) {
        self.editor.recovery_error = Some(error.clone());
        self.notice(format!("Автовосстановление временно недоступно: {error}\nСохраните свои правки вручную по Ctrl+S."));
      }
    } else {
      self.editor.recovery_error = None;
    }
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

  unsafe fn reload_revision(&mut self, change: Change) {
    let Some(path) = self.document_path() else {
      return;
    };
    if let Change::Page(page) = change {
      self.editor.selected = None;
      self.editor.fingerprint = self.editor.revision().map(|r| r.hash);
      self.editor.mosaic_plan.clear();
      if let Some(grids) = &mut self.editor.grids {
        grids.invalidate_page(page);
        grids.finish_page(page);
      }
      self.reader.invalidate_page(page);
      if self.reader.search_open {
        SetWindowTextW(
          self.control(reading::FIND_STATUS),
          wide("Enter — найти").as_ptr(),
        );
      }
      let review_mode = self.review.mode;
      self.review.stop();
      self.review.mode = review_mode;
      self.position_review_actions();
      self.frames.invalidate_page(page);
      self.details.invalidate_page(page);
      self.thumbnails.invalidate_page(page);
      self.generation += 1;
      self.refreshing_revision = true;
      self.last_error = false;
      if let Some(key) = &mut self.layout_key {
        key.0 = self.generation;
      }
      if let Some(context) = &mut self.thumb_context {
        context.0 = self.generation;
      }
      self.render_plan.clear();
      self.thumb_plan.clear();
      if self
        .worker
        .sender
        .send(Command::Open {
          path,
          generation: self.generation,
        })
        .is_err()
      {
        self.error("Рабочий поток PDF остановлен.".into());
      } else {
        self.request();
      }
      self.update_title();
      return;
    }
    let logical = self.path.clone();
    let review_mode = self.review.mode;
    let mut editor = std::mem::take(&mut self.editor);
    editor.selected = None;
    editor.grids = None;
    editor.mosaic_plan.clear();
    editor.reload_states = Some(self.states.clone());
    let (page, zoom, rotation, scroll) = (self.page, self.zoom, self.rotation, self.scroll);
    self.open(path);
    self.path = logical;
    self.editor = editor;
    self.review.mode = review_mode;
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
      Some(AfterSave::Recover(entry)) => {
        self.job(move |_, _| Ok(ResultItem::Recovered(Box::new(entry.recover()?))))
      }
      None => (),
    }
  }
  pub(super) unsafe fn cancel_editor(&mut self) {
    self.editor.stop_tool();
    self.reader.stop();
    self.review.stop();
    for (id, _) in review::BUTTONS {
      InvalidateRect(self.control(id), ptr::null(), 1);
    }
    self.position_review_actions();
    InvalidateRect(self.control(reading::TEXT), ptr::null(), 1);
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
    for id in [SELECT, MASK, COVER, MASK_SELECT, UNDO, REDO, SAVE, CANCEL] {
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
    if self.draft.is_some() {
      for id in [CHANGE_TEXT, DELETE, MASK_DELETE] {
        ShowWindow(self.control(id), SW_HIDE);
      }
      self.inline_layout();
      return;
    }
    self.position_review_actions();
    let target =
      if self.editor.busy || self.printing || self.dialog_open || self.editor.object_drag.is_some()
      {
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
            Ok(ResultItem::Recovered(recovered)) => {
              let meta = recovered.entry.metadata().clone();
              self.open(recovered.revision.path.clone());
              self.path = Some(meta.path);
              self.page = meta.page;
              self.zoom = meta.zoom;
              self.rotation = meta.rotation;
              self.continuous = meta.continuous;
              self.detail_mode = meta.detail_mode;
              self.editor.pixel_mm = meta.pixel_mm;
              self.editor.masks = meta.masks;
              self.editor.mask_undo = (0..self.editor.masks.len()).map(MaskUndo::Added).collect();
              self.editor.reload_states = Some(meta.states);
              self.editor.session = Some(Session {
                current: recovered.revision,
                saved_hash: meta.saved_hash,
              });
              self.editor.recovery = Some(recovery::Writer::resume(recovered.entry));
              self.status =
                "Сеанс восстановлен. Исходный PDF не изменён; Ctrl+S — сохранить правки.".into();
            }
            Ok(ResultItem::Info(message)) => {
              self.status = "Сохранено".into();
              self.notice(message);
            }
            Ok(ResultItem::Edited {
              label,
              before,
              after,
              page_change,
              change,
            }) => {
              let saved_hash = self
                .editor
                .session
                .as_ref()
                .map_or(before.hash, |s| s.saved_hash);
              self.editor.record(MaskUndo::Document(
                before,
                self.editor.masks.clone(),
                label,
                change,
              ));
              if let Some((page, masks)) = &page_change {
                self.page = *page;
                self.editor.masks = masks.clone();
              }
              self.editor.session = Some(Session {
                current: after,
                saved_hash,
              });
              self.reload_revision(change);
              if let Some((page, _)) = page_change {
                self.anchor = Some(ViewAnchor {
                  page,
                  fraction: (0., 0.),
                  point: (self.unit(20), self.unit(20)),
                });
              }
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
                  "{} · объект {} · перетащите рамку; угол — размер; Shift+щелчок — следующий объект",
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

  pub(super) unsafe fn page_point(
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
    let cursor = ((lp as u16 as i16) as i32, ((lp >> 16) as u16 as i16) as i32);
    if let Some(drag) = &self.editor.object_drag {
      let page = drag.page;
      if msg == WM_CAPTURECHANGED {
        self.editor.object_drag = None;
        return;
      }
      if let Some((_, p)) = self.page_point(cursor, Some(page)) {
        let drag = self.editor.object_drag.as_mut().unwrap();
        let b = drag.original;
        drag.bounds = if drag.resize {
          let mut rotated = export::rotate_bounds(b, self.rotation);
          let p = export::rotate_point(p, self.rotation);
          rotated[2] = p.0.max(rotated[0] + 0.002);
          rotated[3] = p.1.max(rotated[1] + 0.002);
          export::rotate_bounds(rotated, -self.rotation)
        } else {
          let (dx, dy) = (p.0 - drag.start.0, p.1 - drag.start.1);
          [b[0] + dx, b[1] + dy, b[2] + dx, b[3] + dy]
        };
      }
      if msg == WM_LBUTTONUP {
        let drag = self.editor.object_drag.take().unwrap();
        ReleaseCapture();
        if drag
          .bounds
          .iter()
          .zip(drag.original)
          .any(|(a, b)| (a - b).abs() > 0.0005)
        {
          self.apply_operation(
            Operation::Transform {
              page: drag.page,
              object: drag.index,
              bounds: drag.bounds,
            },
            None,
          );
        }
      }
      InvalidateRect(self.canvas, ptr::null(), 0);
      return;
    }
    if msg == WM_LBUTTONDOWN
      && self.editor.mode == Mode::Select
      && GetKeyState(VK_SHIFT as i32) >= 0
    {
      if let Some((page, object)) = &self.editor.selected {
        if let Some(r) = self.editor_rect(*page, object.bounds) {
          let margin = self.unit(8);
          if cursor.0 >= r.left - margin
            && cursor.0 <= r.right + margin
            && cursor.1 >= r.top - margin
            && cursor.1 <= r.bottom + margin
          {
            if let Some((_, p)) = self.page_point(cursor, Some(*page)) {
              let resize =
                (cursor.0 - r.right).abs() <= margin && (cursor.1 - r.bottom).abs() <= margin;
              self.editor.object_drag = Some(ObjectDrag {
                page: *page,
                index: object.index,
                original: object.bounds,
                bounds: object.bounds,
                start: p,
                resize,
              });
              SetCapture(self.canvas);
              return;
            }
          }
        }
      }
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
    if self.draft.is_some() {
      return;
    }
    if let Some(drag) = &self.editor.object_drag {
      if let Some(r) = self.editor_rect(drag.page, drag.bounds) {
        DrawFocusRect(dc, &r);
      }
    }
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
        if self.editor.selected_mask.is_none() && self.editor.mode == Mode::Select {
          let radius = self.unit(4);
          fill(
            dc,
            &RECT {
              left: rect.right - radius,
              top: rect.bottom - radius,
              right: rect.right + radius,
              bottom: rect.bottom + radius,
            },
            0x00c87818,
          );
        }
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        DeleteObject(pen);
      }
    }
  }

  pub(super) unsafe fn editor_rect(&self, page: usize, bounds: [f64; 4]) -> Option<RECT> {
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
  if id == RECOVER {
    with_app(|a| a.dialog_open = false);
    offer_recovery(hwnd, false);
    return Ok(());
  }
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
        a.reader.stop();
        a.review.stop();
        for (id, _) in review::BUTTONS {
          InvalidateRect(a.control(id), ptr::null(), 1);
        }
        InvalidateRect(a.control(reading::TEXT), ptr::null(), 1);
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
          a.editor.record(MaskUndo::Masks(a.editor.masks.clone()));
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
    UNDO | REDO => {
      with_app(|a| {
        ReleaseCapture();
        if let Some(change) = a.editor.history_step(id == REDO) {
          if change != Change::Overlays {
            a.reload_revision(change);
          }
          a.status = if id == REDO {
            "Действие повторено · Ctrl+Z — отменить"
          } else {
            "Действие отменено · Ctrl+Y — повторить"
          }
          .into();
        } else {
          a.status = "Нет доступных действий.".into();
        }
        a.request_mosaics();
        SetFocus(a.canvas);
        InvalidateRect(a.canvas, ptr::null(), 0);
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
      if !masks.is_empty() {
        return Err("Есть области скрытия. Сначала сохраните очищенный PDF и откройте его для сохранения отдельных страниц.".into());
      }
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
      if id == CHANGE_TEXT {
        if object.kind != 1 {
          return Err(
            "Выбран не текстовый объект. В сканах текст является частью изображения.".into(),
          );
        }
        with_app(|a| a.begin_inline(page, object));
        return Ok(());
      }
      let operation = Operation::Delete {
        page,
        object: object.index,
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
          Ok(ResultItem::Edited {
            label: "Удаление объекта",
            before,
            after,
            page_change: None,
            change: Change::Page(page),
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
  if with_app(|a| {
    a.remember_reading();
    if a.draft.is_some() {
      a.status = "Сначала примените текст (Ctrl+Enter) или отмените правку (Esc).".into();
      true
    } else {
      false
    }
  })
  .unwrap_or(false)
  {
    return;
  }
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

pub(super) unsafe fn offer_recovery(hwnd: HWND, startup: bool) {
  let eligible = with_app(|a| !a.dialog_open && !a.editor.busy && !a.printing).unwrap_or(false);
  if !eligible {
    return;
  }
  let result = recovery::pending();
  let mut entries = match result {
    Ok(entries) => entries,
    Err(error) => {
      with_app(|a| a.notice(error));
      return;
    }
  };
  if startup {
    if let Some(path) = with_app(|a| a.path.clone()).flatten() {
      entries.retain(|e| export::same_file(&e.metadata().path, &path));
    }
  }
  let Some(entry) = entries.into_iter().next() else {
    if !startup {
      with_app(|a| {
        a.notice("Незавершённых сеансов не найдено. Сеансы других открытых окон не предлагаются для восстановления.".into())
      });
    }
    return;
  };
  let label = entry
    .metadata()
    .path
    .file_name()
    .unwrap_or_default()
    .to_string_lossy();
  let message = format!("Найден незавершённый сеанс: {label}\nВосстановить правки PDF и области скрытия?\n\nДа — восстановить; Нет — удалить эту рабочую копию; Отмена — оставить на потом.\nИсходный PDF не изменится. История действий до сбоя не восстанавливается.");
  with_app(|a| a.dialog_open = true);
  let answer = MessageBoxW(
    hwnd,
    wide(&message).as_ptr(),
    wide("Восстановление сеанса").as_ptr(),
    MB_YESNOCANCEL | MB_ICONQUESTION,
  );
  with_app(|a| a.dialog_open = false);
  if answer == IDYES {
    leave_document(hwnd, AfterSave::Recover(Box::new(entry)));
  } else if answer == IDNO {
    entry.discard();
  }
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
    editor.mask_undo.push(MaskUndo::Document(
      revision,
      vec![],
      "Изменение PDF",
      Change::Document,
    ));
    assert!(!editor.undo_mask());
    assert!(matches!(
      editor.mask_undo.pop(),
      Some(MaskUndo::Document(..))
    ));
    assert!(editor.undo_mask());
    assert_eq!(editor.masks.len(), 1);
    assert!(editor.undo_mask());
    assert!(editor.masks.is_empty());
  }
  #[test]
  fn redo_masks_preserves_order_and_new_edits_clear_redo() {
    let mut editor = Editor::default();
    editor.add_mask(mask(0.1));
    editor.add_mask(mask(0.3));
    editor.selected_mask = Some(0);
    editor.remove_mask();
    assert_eq!(editor.history_step(false), Some(Change::Overlays));
    assert_eq!(editor.masks[0].bounds[0], 0.1);
    assert_eq!(editor.history_step(true), Some(Change::Overlays));
    assert_eq!(editor.masks.len(), 1);
    assert_eq!(editor.masks[0].bounds[0], 0.3);
    editor.history_step(false);
    editor.add_mask(mask(0.5));
    assert!(!editor.can_redo());
    assert_eq!(editor.masks.len(), 3);
  }

  #[test]
  fn document_redo_and_save_keep_dirty_state_correct() {
    let before = Revision::new(b"before").unwrap();
    let after = Revision::new(b"after").unwrap();
    let mut editor = Editor {
      session: Some(Session {
        current: after.clone(),
        saved_hash: after.hash,
      }),
      ..Editor::default()
    };
    editor.record(MaskUndo::Document(
      before.clone(),
      vec![],
      "Изменение PDF",
      Change::Document,
    ));
    assert!(!editor.dirty());
    assert_eq!(editor.history_step(false), Some(Change::Document));
    assert!(editor.dirty());
    assert_eq!(editor.session.as_ref().unwrap().current.hash, before.hash);
    assert_eq!(editor.history_step(true), Some(Change::Document));
    assert!(!editor.dirty());
    assert_eq!(editor.session.as_ref().unwrap().current.hash, after.hash);
  }

  #[test]
  fn pixel_size_change_redo_restores_the_actual_masks() {
    let mut editor = Editor::default();
    editor.add_mask(mask(0.1));
    editor.record(MaskUndo::Masks(editor.masks.clone()));
    editor.masks[0].kind = MaskKind::Pixelate { block_mm: 12 };
    editor.history_step(false);
    assert_eq!(editor.masks[0].kind, MaskKind::Cover);
    editor.history_step(true);
    assert_eq!(editor.masks[0].kind, MaskKind::Pixelate { block_mm: 12 });
  }

  #[test]
  fn mask_history_has_the_same_bound_as_document_history() {
    let mut editor = Editor::default();
    for _ in 0..150 {
      editor.add_mask(mask(0.1));
    }
    assert_eq!(editor.mask_undo.len(), 100);
    for _ in 0..100 {
      assert_eq!(editor.history_step(false), Some(Change::Overlays));
    }
    assert_eq!(editor.masks.len(), 50);
    for _ in 0..100 {
      assert_eq!(editor.history_step(true), Some(Change::Overlays));
    }
    assert_eq!(editor.masks.len(), 150);
  }

  #[test]
  fn undo_and_redo_keep_the_affected_page() {
    let before = Revision::new(b"before").unwrap();
    let after = Revision::new(b"after").unwrap();
    let mut editor = Editor {
      session: Some(Session {
        current: after.clone(),
        saved_hash: before.hash,
      }),
      ..Editor::default()
    };
    editor.record(MaskUndo::Document(
      before.clone(),
      vec![],
      "Замечание",
      Change::Page(7),
    ));
    assert_eq!(editor.history_step(false), Some(Change::Page(7)));
    assert_eq!(editor.session.as_ref().unwrap().current.hash, before.hash);
    assert_eq!(editor.history_step(true), Some(Change::Page(7)));
    assert_eq!(editor.session.as_ref().unwrap().current.hash, after.hash);
  }

  #[test]
  fn undo_rechecks_budget_when_large_current_revision_moves_to_redo() {
    let before = Revision::new(b"before").unwrap();
    let mut after = Revision::new(b"after").unwrap();
    Arc::get_mut(&mut after).unwrap().size = 257 * 1024 * 1024;
    let mut editor = Editor {
      session: Some(Session {
        saved_hash: after.hash,
        current: after,
      }),
      ..Editor::default()
    };
    editor.record(MaskUndo::Document(
      before.clone(),
      vec![],
      "Изменение PDF",
      Change::Document,
    ));
    assert_eq!(editor.history_step(false), Some(Change::Document));
    assert_eq!(editor.session.as_ref().unwrap().current.hash, before.hash);
    assert!(!editor.can_redo());
  }
}

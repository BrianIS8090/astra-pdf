use super::*;
use crate::editing::Operation;

pub(super) const REMOVE: usize = 90;
pub(super) const DUPLICATE: usize = 91;
pub(super) const INSERT: usize = 92;
pub(super) const MERGE: usize = 93;
pub(super) const UP: usize = 94;
pub(super) const DOWN: usize = 95;
pub(super) const BUTTONS: [(usize, &str); 7] = [
  (DUPLICATE, "Дублировать страницу"),
  (REMOVE, "Удалить страницу"),
  (INSERT, "Вставить страницы из PDF"),
  (MERGE, "Объединить с PDF"),
  (UP, "Переместить страницу выше"),
  (DOWN, "Переместить страницу ниже"),
  (
    editor::PAGES,
    "Сохранить выбранные страницы в отдельный PDF",
  ),
];

impl App {
  pub(super) unsafe fn setup_pages(&mut self) {
    for (id, label) in BUTTONS {
      self.create(id, "BUTTON", label, BS_OWNERDRAW as u32);
    }
    MakeDragList(self.control(PAGE_LIST));
  }

  pub(super) unsafe fn pages_layout(&self, top: i32) {
    for (i, (id, _)) in BUTTONS.iter().enumerate() {
      if *id == editor::PAGES {
        self.place(*id, 200, top + 10, 28, 32);
      } else {
        self.place(*id, 12 + i as i32 * 36, top + 86, 32, 32);
      }
      ShowWindow(
        self.control(*id),
        if !self.layer_tab && !self.reader.bookmarks_open {
          SW_SHOW
        } else {
          SW_HIDE
        },
      );
    }
  }

  pub(super) unsafe fn page_action(&mut self, id: usize) {
    if self.sizes.is_empty() || self.printing || self.editor.busy {
      return;
    }
    if id == INSERT || id == MERGE {
      PostMessageW(self.hwnd, WM_APP + 27, id, 0);
      return;
    }
    let mut order: Vec<_> = (0..self.sizes.len()).collect();
    let mut target = self.page;
    match id {
      REMOVE if order.len() > 1 => {
        order.remove(self.page);
        target = target.min(order.len() - 1);
      }
      DUPLICATE => {
        order.insert(self.page, self.page);
        target += 1;
      }
      UP if self.page > 0 => {
        order.swap(self.page, self.page - 1);
        target -= 1;
      }
      DOWN if self.page + 1 < order.len() => {
        order.swap(self.page, self.page + 1);
        target += 1;
      }
      _ => return,
    }
    self.edit_page_order(order, target);
  }

  unsafe fn edit_page_order(&mut self, order: Vec<usize>, target: usize) {
    let masks = order
      .iter()
      .enumerate()
      .flat_map(|(page, old)| {
        self
          .editor
          .masks
          .iter()
          .filter(move |m| m.page == *old)
          .cloned()
          .map(move |mut m| {
            m.page = page;
            m
          })
      })
      .collect();
    self.apply_operation(Operation::PageOrder { order }, Some((target, masks)));
  }

  pub(super) unsafe fn page_drag(&mut self, data: &DRAGLISTINFO) -> LRESULT {
    if self.editor.busy || self.printing || self.dialog_open || self.draft.is_some() {
      return 0;
    }
    let hit = LBItemFromPt(data.hWnd, data.ptCursor, 1);
    match data.uNotification {
      DL_BEGINDRAG => {
        if hit < 0 {
          return 0;
        }
        self.drag_page = Some(hit as usize);
        1
      }
      DL_DRAGGING => {
        DrawInsert(self.hwnd, data.hWnd, hit);
        DL_MOVECURSOR as isize
      }
      DL_DROPPED => {
        DrawInsert(self.hwnd, data.hWnd, -1);
        if let Some(from) = self.drag_page.take() {
          let count = self.sizes.len();
          let to = if hit < 0 { count } else { hit as usize };
          if from < count && to <= count {
            let mut order: Vec<_> = (0..count).collect();
            let item = order.remove(from);
            let target = if to > from { to - 1 } else { to };
            order.insert(target, item);
            if from != target {
              self.edit_page_order(order, target);
            }
          }
        }
        0
      }
      DL_CANCELDRAG => {
        self.drag_page = None;
        DrawInsert(self.hwnd, data.hWnd, -1);
        0
      }
      _ => 0,
    }
  }
}

pub(super) unsafe fn insert_dialog(hwnd: HWND, append: bool) {
  let allowed = with_app(|a| {
    if a.editor.busy || a.printing || a.dialog_open || a.sizes.is_empty() {
      false
    } else {
      a.dialog_open = true;
      true
    }
  })
  .unwrap_or(false);
  if !allowed {
    return;
  }
  let result = (|| {
    let Some(source) = App::choose_file(hwnd)? else {
      return Ok::<_, String>(());
    };
    let range = if append {
      String::new()
    } else {
      let Some(value)=crate::dialogs::prompt(hwnd,"Вставить страницы","Номера из выбранного PDF: 1, 3-5. Пустое поле — все страницы.\nВставка после текущей страницы; Ctrl+Z отменяет её.","",false) else{return Ok(())};
      value
    };
    with_app(|a| {
      let at = if append { a.sizes.len() } else { a.page + 1 };
      a.pending_insert = Some((source, range, at));
    });
    Ok(())
  })();
  with_app(|a| {
    a.dialog_open = false;
    if let Err(error) = result {
      a.editor.notice = Some(error);
      PostMessageW(hwnd, WM_APP + 23, 0, 0);
    }
    if let Some((source, range, at)) = a.pending_insert.take() {
      a.apply_operation(Operation::InsertPages { source, range, at }, None);
    }
  });
}

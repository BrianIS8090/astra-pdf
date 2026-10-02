use super::*;
use crate::{
  annotation::{Kind, Note},
  editing::Operation,
  reading::Comment,
};

pub(super) const TOOLS: usize = 120;
pub(super) const COMMENT: usize = 121;
pub(super) const RECTANGLE: usize = 122;
pub(super) const ARROW: usize = 123;
pub(super) const MARKER: usize = 124;
pub(super) const DISTANCE: usize = 125;
pub(super) const AREA: usize = 126;
pub(super) const SCALE: usize = 127;
pub(super) const EDIT: usize = 128;
pub(super) const REMOVE: usize = 129;
pub(super) const LABEL: usize = 130;
pub(super) const BUTTONS: [(usize, &str); 10] = [
  (TOOLS, "Замечания и измерения"),
  (COMMENT, "Комментарий"),
  (RECTANGLE, "Прямоугольник"),
  (ARROW, "Стрелка"),
  (MARKER, "Маркер"),
  (DISTANCE, "Измерить расстояние"),
  (AREA, "Измерить площадь · Enter — завершить"),
  (SCALE, "Масштаб чертежа"),
  (EDIT, "Изменить комментарий"),
  (REMOVE, "Удалить замечание"),
];

pub(super) struct Review {
  pub open: bool,
  pub mode: Option<usize>,
  pub scale: f64,
  page: usize,
  points: Vec<[f64; 2]>,
  cursor: Option<[f64; 2]>,
  pub pending: Option<Note>,
  selected: Option<(usize, Comment)>,
}
impl Default for Review {
  fn default() -> Self {
    Self {
      open: false,
      mode: None,
      scale: 1.,
      page: 0,
      points: vec![],
      cursor: None,
      pending: None,
      selected: None,
    }
  }
}
impl Review {
  pub fn has_selection(&self) -> bool {
    self.selected.is_some()
  }
  pub fn stop(&mut self) {
    self.mode = None;
    self.points.clear();
    self.cursor = None;
    self.selected = None;
    self.pending = None;
  }
  pub fn active(&self, id: usize) -> bool {
    if id == TOOLS {
      self.open
    } else {
      self.mode == Some(id)
    }
  }
  pub fn can_cancel(&self) -> bool {
    self.mode.is_some() || self.selected.is_some()
  }
  pub fn probe(&self) -> serde_json::Value {
    serde_json::json!({"open":self.open,"mode":self.mode,"scale":self.scale,"points":self.points,"selected":self.selected.as_ref().map(|(p,n)|serde_json::json!({"page":p,"index":n.index,"text":n.text}))})
  }
}

impl App {
  pub(super) unsafe fn setup_review(&mut self) {
    for (id, label) in BUTTONS {
      let h = self.create(id, "BUTTON", label, BS_OWNERDRAW as u32);
      if id == EDIT || id == REMOVE {
        SetParent(h, self.canvas);
        ShowWindow(h, SW_HIDE);
      }
    }
    self.create(
      LABEL,
      "STATIC",
      "Масштаб 1:1 · измерения по геометрии PDF",
      0x200,
    );
  }

  pub(super) unsafe fn review_layout(&self, base: i32, tools_x: i32, tools_y: i32) {
    self.place(TOOLS, tools_x + 10 * 36, tools_y, 32, 32);
    for (i, id) in [COMMENT, RECTANGLE, ARROW, MARKER, DISTANCE, AREA, SCALE]
      .iter()
      .enumerate()
    {
      self.place(*id, 12 + i as i32 * 36, base + 4, 32, 32);
      ShowWindow(
        self.control(*id),
        if self.review.open { SW_SHOW } else { SW_HIDE },
      );
    }
    self.place(LABEL, 280, base + 6, 390, 28);
    ShowWindow(
      self.control(LABEL),
      if self.review.open { SW_SHOW } else { SW_HIDE },
    );
    SetWindowTextW(
      self.control(LABEL),
      wide(&format!(
        "Масштаб 1:{} · измерения по геометрии PDF",
        self.review.scale
      ))
      .as_ptr(),
    );
    self.position_review_actions();
  }

  pub(super) unsafe fn review_action(&mut self, id: usize) {
    match id {
      TOOLS => {
        self.review.open = !self.review.open;
        if !self.review.open {
          self.review.stop();
        }
        self.layout();
      }
      COMMENT | RECTANGLE | ARROW | MARKER | DISTANCE | AREA => {
        let was = self.review.mode;
        self.cancel_editor();
        if was != Some(id) {
          self.review.mode = Some(id);
        }
        self.status=match id {
          COMMENT=>"Нажмите на страницу, чтобы оставить комментарий.",
          AREA=>"Отметьте вершины контура · Enter — завершить · Backspace — убрать последнюю · Esc — отменить",
          DISTANCE=>"Протяните линию между двумя точками. Сначала установите масштаб чертежа.",
          _=>"Протяните область на странице · Esc — выключить инструмент",
        }.into();
        SetFocus(self.canvas);
      }
      SCALE | EDIT => {
        PostMessageW(self.hwnd, WM_APP + 29, id, 0);
      }
      REMOVE => {
        if let Some((page, note)) = self.review.selected.take() {
          self.apply_operation(
            Operation::NoteDelete {
              page,
              index: note.index,
            },
            None,
          );
        }
      }
      _ => (),
    }
    self.sync_editor_controls();
    for (id, _) in BUTTONS {
      InvalidateRect(self.control(id), ptr::null(), 1);
    }
    self.position_review_actions();
    InvalidateRect(self.canvas, ptr::null(), 0);
  }

  pub(super) unsafe fn review_finish(&mut self) {
    if self.review.mode != Some(AREA) || self.review.points.len() < 3 {
      return;
    }
    let note = Note {
      page: self.review.page,
      points: std::mem::take(&mut self.review.points),
      kind: Kind::Area {
        scale: self.review.scale,
      },
    };
    self.review.cursor = None;
    self.apply_operation(Operation::Annotate { note }, None);
  }

  pub(super) unsafe fn review_backspace(&mut self) {
    self.review.points.pop();
    InvalidateRect(self.canvas, ptr::null(), 0);
  }

  pub(super) unsafe fn review_mouse(&mut self, msg: u32, lp: LPARAM) -> bool {
    if self.editor.busy || self.printing || self.dialog_open {
      return false;
    }
    let xy = ((lp as u16 as i16) as i32, ((lp >> 16) as u16 as i16) as i32);
    if msg == WM_CAPTURECHANGED {
      self.review.cursor = None;
      if self.review.mode != Some(AREA) {
        self.review.points.clear();
      }
      return false;
    }
    let only = if self.review.points.is_empty() {
      None
    } else {
      Some(self.review.page)
    };
    let Some((page, p)) = self.page_point(xy, only) else {
      return false;
    };
    let p = [p.0, p.1];
    if let Some(mode) = self.review.mode {
      if msg == WM_LBUTTONDOWN {
        self.review.page = page;
        if mode == COMMENT {
          self.review.pending = Some(Note {
            page,
            points: vec![p],
            kind: Kind::Comment(String::new()),
          });
          PostMessageW(self.hwnd, WM_APP + 29, COMMENT, 0);
        } else if mode == AREA {
          if self.review.points.len() < 256 {
            self.review.points.push(p);
          }
        } else {
          self.review.points = vec![p];
          self.review.cursor = Some(p);
          SetCapture(self.canvas);
        }
      } else if msg == WM_MOUSEMOVE {
        self.review.cursor = Some(p);
      } else if msg == WM_LBUTTONUP && mode != AREA && self.review.points.len() == 1 {
        let a = self.review.points[0];
        self.review.points.clear();
        self.review.cursor = None;
        ReleaseCapture();
        if (p[0] - a[0]).hypot(p[1] - a[1]) > 0.002 {
          let kind = match mode {
            RECTANGLE => Kind::Rectangle,
            ARROW => Kind::Arrow,
            MARKER => Kind::Highlight,
            DISTANCE => Kind::Distance {
              scale: self.review.scale,
            },
            _ => return true,
          };
          self.apply_operation(
            Operation::Annotate {
              note: Note {
                page,
                points: vec![a, p],
                kind,
              },
            },
            None,
          );
        }
      }
      InvalidateRect(self.canvas, ptr::null(), 0);
      return true;
    }
    if msg == WM_LBUTTONDOWN && self.editor.viewing() && !self.reader.text_mode {
      if let Some(note) = self.reader.note_at(page, (p[0], p[1]), &self.editor.masks) {
        self.review.selected = Some((page, note));
        self.position_review_actions();
        InvalidateRect(self.canvas, ptr::null(), 0);
        return true;
      }
      self.review.selected = None;
      self.position_review_actions();
    }
    false
  }

  pub(super) unsafe fn position_review_actions(&self) {
    let target = if self.editor.busy || self.dialog_open || self.printing {
      None
    } else {
      self
        .review
        .selected
        .as_ref()
        .and_then(|(p, n)| self.editor_rect(*p, n.bounds).map(|r| (r, n.kind)))
    };
    for id in [EDIT, REMOVE] {
      ShowWindow(self.control(id), SW_HIDE);
    }
    let Some((r, kind)) = target else { return };
    let view = self.viewport();
    if r.right < 0 || r.left > view.0 || r.bottom < 0 || r.top > view.1 {
      return;
    }
    let size = self.unit(32);
    let x = r
      .left
      .clamp(self.unit(4), (view.0 - size * 2 - self.unit(4)).max(4));
    let y = (r.top - size - self.unit(4)).max(self.unit(4));
    for (i, id) in [EDIT, REMOVE].iter().enumerate() {
      let h = self.control(*id);
      MoveWindow(h, x + i as i32 * (size + 4), y, size, size, 1);
      EnableWindow(h, (*id != EDIT || kind == 1) as i32);
      ShowWindow(h, SW_SHOW);
    }
  }

  pub(super) unsafe fn review_paint(&self, dc: HDC) {
    if let Some((page, note)) = &self.review.selected {
      if let Some(r) = self.editor_rect(*page, note.bounds) {
        let brush = CreateSolidBrush(0x00c87818);
        FrameRect(dc, &r, brush);
        DeleteObject(brush);
      }
    }
    if self.review.points.is_empty() {
      return;
    }
    let Some(layout) = self.document_layout.pages.get(self.review.page) else {
      return;
    };
    let origin = self.page_origin(self.review.page);
    let convert = |p: [f64; 2]| {
      let p = crate::export::rotate_point((p[0], p[1]), self.rotation);
      POINT {
        x: origin.0 - self.scroll.0 + (p.0 * layout.width as f64).round() as i32,
        y: origin.1 - self.scroll.1 + (p.1 * layout.height as f64).round() as i32,
      }
    };
    let points: Vec<_> = self
      .review
      .points
      .iter()
      .copied()
      .chain(self.review.cursor)
      .map(convert)
      .collect();
    if points.len() < 2 {
      return;
    }
    let pen = CreatePen(PS_SOLID, self.unit(2), 0x00c87818);
    let old = SelectObject(dc, pen);
    let brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
    if matches!(self.review.mode, Some(RECTANGLE | MARKER)) {
      Rectangle(
        dc,
        points[0].x.min(points[1].x),
        points[0].y.min(points[1].y),
        points[0].x.max(points[1].x),
        points[0].y.max(points[1].y),
      );
    } else {
      Polyline(dc, points.as_ptr(), points.len() as i32);
    }
    SelectObject(dc, old);
    SelectObject(dc, brush);
    DeleteObject(pen);
  }
}

pub(super) unsafe fn dialog(hwnd: HWND, id: usize) {
  if !with_app(|a| {
    if a.editor.busy || a.printing || a.dialog_open {
      false
    } else {
      a.dialog_open = true;
      true
    }
  })
  .unwrap_or(false)
  {
    return;
  }
  let result = (|| {
    if id == SCALE {
      let value = with_app(|a| a.review.scale.to_string()).unwrap_or_default();
      if let Some(text)=crate::dialogs::prompt(hwnd,"Масштаб чертежа","Введите знаменатель масштаба: 100 для 1:100, 50 для 1:50.\nРазмеры вычисляются по физическому размеру страницы PDF. Проверяйте масштаб по известному размеру.",&value,false) {
        let scale=text.trim().replace(',',".").parse::<f64>().map_err(|_|"Введите число, например 100.")?;
        if !scale.is_finite() || !(0.001..=100_000.).contains(&scale) {return Err("Масштаб вне допустимого диапазона.".into());}
        with_app(|a|a.review.scale=scale);
      }
    } else if id == EDIT {
      if let Some((page, note)) = with_app(|a| a.review.selected.clone()).flatten() {
        if let Some(text) = crate::dialogs::prompt(
          hwnd,
          "Комментарий",
          "Измените текст комментария.",
          &note.text,
          false,
        ) {
          with_app(|a| {
            a.apply_operation(
              Operation::NoteText {
                page,
                index: note.index,
                text,
              },
              None,
            )
          });
        }
      }
    } else if let Some(mut note) = with_app(|a| a.review.pending.take()).flatten() {
      if let Some(text) = crate::dialogs::prompt(
        hwnd,
        "Новый комментарий",
        "Комментарий будет сохранён в PDF и доступен в других просмотрщиках.",
        "",
        false,
      ) {
        note.kind = Kind::Comment(text);
        with_app(|a| a.apply_operation(Operation::Annotate { note }, None));
      }
    }
    Ok::<_, String>(())
  })();
  with_app(|a| {
    a.dialog_open = false;
    if let Err(error) = result {
      a.editor.notice = Some(error);
      PostMessageW(hwnd, WM_APP + 23, 0, 0);
    }
    a.layout();
    a.sync_editor_controls();
  });
}

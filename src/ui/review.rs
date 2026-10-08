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
pub(super) const INK: usize = 131;
pub(super) const TEXT: usize = 132;
pub(super) const COLOR: usize = 133;
const COLORS: [(&str, [u8; 3]); 7] = [
  ("Синий", [26, 107, 194]),
  ("Красный", [211, 47, 47]),
  ("Зелёный", [36, 128, 68]),
  ("Оранжевый", [213, 110, 0]),
  ("Фиолетовый", [123, 65, 181]),
  ("Чёрный", [25, 25, 25]),
  ("Белый", [255, 255, 255]),
];
pub(super) const BUTTONS: [(usize, &str); 12] = [
  (TOOLS, "Замечания и измерения"),
  (COMMENT, "Комментарий"),
  (RECTANGLE, "Прямоугольник"),
  (ARROW, "Стрелка"),
  (MARKER, "Маркер"),
  (INK, "Карандаш · рисовать от руки"),
  (
    TEXT,
    "Текстовая заметка · выделите область или нажмите на страницу",
  ),
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
  color: [u8; 3],
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
      color: COLORS[0].1,
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
    serde_json::json!({"open":self.open,"mode":self.mode,"scale":self.scale,"color":self.color,"points":self.points,"selected":self.selected.as_ref().map(|(p,n)|serde_json::json!({"page":p,"index":n.index,"text":n.text,"kind":n.kind}))})
  }
  fn stroke_point(&mut self, point: [f64; 2]) {
    if self
      .points
      .last()
      .is_some_and(|last| (last[0] - point[0]).hypot(last[1] - point[1]) < 0.0005)
    {
      return;
    }
    // Длинный жест остаётся в пределах памяти; начало и последний участок сохраняются.
    if self.points.len() >= 4096 {
      self.points = self.points.iter().step_by(2).copied().collect();
    }
    self.points.push(point);
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
    let color = self.create(
      COLOR,
      "COMBOBOX",
      "Цвет карандаша и текста",
      CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
    );
    for (label, _) in COLORS {
      SendMessageW(color, CB_ADDSTRING, 0, wide(label).as_ptr() as isize);
    }
    SendMessageW(color, CB_SETCURSEL, 0, 0);
  }

  pub(super) unsafe fn review_layout(&self, base: i32, tools_x: i32, tools_y: i32) {
    self.place(TOOLS, tools_x + 10 * 36, tools_y, 32, 32);
    for (i, id) in [
      COMMENT, RECTANGLE, ARROW, MARKER, INK, TEXT, DISTANCE, AREA, SCALE,
    ]
    .iter()
    .enumerate()
    {
      self.place(*id, 12 + i as i32 * 36, base + 4, 32, 32);
      ShowWindow(
        self.control(*id),
        if self.review.open { SW_SHOW } else { SW_HIDE },
      );
    }
    self.place(COLOR, 340, base + 4, 128, 200);
    ShowWindow(
      self.control(COLOR),
      if self.review.open { SW_SHOW } else { SW_HIDE },
    );
    let mut bounds: RECT = std::mem::zeroed();
    GetClientRect(self.hwnd, &mut bounds);
    let label_width = (bounds.right as f64 * 96. / self.dpi as f64) as i32 - 492;
    self.place(LABEL, 480, base + 6, label_width.max(1), 28);
    ShowWindow(
      self.control(LABEL),
      if self.review.open && label_width >= 160 {
        SW_SHOW
      } else {
        SW_HIDE
      },
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
      COMMENT | RECTANGLE | ARROW | MARKER | DISTANCE | AREA | INK | TEXT => {
        let was = self.review.mode;
        self.cancel_editor();
        if was != Some(id) {
          self.review.mode = Some(id);
        }
        self.status=match id {
          COMMENT=>"Нажмите на страницу, чтобы оставить комментарий.",
          INK=>"Рисуйте левой кнопкой мыши. Цвет — в списке на панели. Ctrl+Z отменяет штрих, Ctrl+S сохраняет в PDF.",
          TEXT=>"Выделите область для текста или нажмите на страницу. Цвет — в списке на панели. Esc — выключить инструмент.",
          AREA=>"Отметьте вершины контура · Enter — завершить · Backspace — убрать последнюю · Esc — отменить",
          DISTANCE=>"Протяните линию между двумя точками. Сначала установите масштаб чертежа.",
          _=>"Протяните область на странице · Esc — выключить инструмент",
        }.into();
        SetFocus(self.canvas);
      }
      SCALE | EDIT => {
        PostMessageW(self.hwnd, WM_APP + 29, id, 0);
      }
      COLOR => {
        let index = SendMessageW(self.control(COLOR), CB_GETCURSEL, 0, 0);
        if let Some((label, color)) = COLORS.get(index.max(0) as usize) {
          self.review.color = *color;
          self.status = format!("Цвет нового рисунка и текста: {label}");
        }
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
        if mode == INK && !self.review.points.is_empty() {
          self.review.stroke_point(p);
        }
      } else if msg == WM_LBUTTONUP && mode == INK && !self.review.points.is_empty() {
        self.review.stroke_point(p);
        let points = std::mem::take(&mut self.review.points);
        self.review.cursor = None;
        ReleaseCapture();
        if points.len() >= 2 {
          self.apply_operation(
            Operation::Annotate {
              note: Note {
                page,
                points,
                kind: Kind::Ink {
                  color: self.review.color,
                  width: 2.,
                },
              },
            },
            None,
          );
        }
      } else if msg == WM_LBUTTONUP && mode == TEXT && self.review.points.len() == 1 {
        let a = self.review.points[0];
        let points = if (p[0] - a[0]).hypot(p[1] - a[1]) > 0.002 {
          vec![a, p]
        } else {
          let a = [a[0].min(0.60), a[1].min(0.82)];
          vec![a, [a[0] + 0.4, a[1] + 0.16]]
        };
        self.review.points.clear();
        self.review.cursor = None;
        ReleaseCapture();
        self.review.pending = Some(Note {
          page,
          points,
          kind: Kind::Text {
            text: String::new(),
            color: self.review.color,
            size: 14.,
          },
        });
        PostMessageW(self.hwnd, WM_APP + 29, TEXT, 0);
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
      EnableWindow(h, (*id != EDIT || matches!(kind, 1 | 3)) as i32);
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
    let color = if matches!(self.review.mode, Some(INK | TEXT)) {
      let [r, g, b] = self.review.color;
      r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
    } else {
      0x00c87818
    };
    let width = if self.review.mode == Some(INK) {
      let size = self.sizes[self.review.page];
      let physical_width = if self.rotation % 2 == 0 {
        size.0
      } else {
        size.1
      };
      (2. * layout.width as f64 / physical_width).round().max(1.) as i32
    } else {
      self.unit(2)
    };
    let pen = CreatePen(PS_SOLID, width, color);
    let old = SelectObject(dc, pen);
    let brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
    if matches!(self.review.mode, Some(RECTANGLE | MARKER | TEXT)) {
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
        if matches!(note.kind, Kind::Text { .. }) {
          "Текстовая заметка"
        } else {
          "Новый комментарий"
        },
        "Заметка будет сохранена в PDF. Ctrl+S сохраняет документ; Ctrl+Z отменяет добавление.",
        "",
        false,
      ) {
        match &mut note.kind {
          Kind::Text { text: value, .. } => *value = text,
          _ => note.kind = Kind::Comment(text),
        }
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

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn long_stroke_is_bounded_and_escape_clears_only_the_gesture() {
    let mut review = Review {
      mode: Some(INK),
      color: [211, 47, 47],
      ..Review::default()
    };
    for i in 0..12000 {
      review.stroke_point([i as f64 / 12000., if i % 2 == 0 { 0.2 } else { 0.4 }]);
    }
    assert!(review.points.len() <= 4096);
    assert_eq!(review.points[0], [0., 0.2]);
    assert_eq!(review.points.last().unwrap()[0], 11999. / 12000.);
    review.stop();
    assert!(review.points.is_empty() && review.mode.is_none());
    assert_eq!(review.color, [211, 47, 47]);
  }
}

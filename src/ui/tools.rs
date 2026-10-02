use super::*;
use editor::{CHANGE_TEXT, COVER, DELETE, MASK, MASK_DELETE, MASK_SELECT, SAFE_SAVE, SELECT, UNDO};

pub(super) const BAR: [(usize, &str); 7] = [
  (SELECT, "Объект"),
  (MASK, "Пиксели"),
  (COVER, "Скрыть"),
  (MASK_SELECT, "Блоки"),
  (UNDO, "Назад"),
  (SAFE_SAVE, "Сохранить"),
  (CANCEL, "Esc"),
];
pub(super) const ACTIONS: [(usize, &str); 3] = [
  (CHANGE_TEXT, "Текст"),
  (DELETE, "Удалить"),
  (MASK_DELETE, "Убрать блок"),
];

pub(super) fn is_button(id: usize) -> bool {
  BAR.iter().chain(ACTIONS.iter()).any(|(i, _)| *i == id)
}

impl App {
  pub(super) unsafe fn setup_editor_controls(&mut self) {
    for (id, label) in BAR.into_iter().chain(ACTIONS) {
      let h = self.create(id, "BUTTON", label, BS_OWNERDRAW as u32);
      if ACTIONS.iter().any(|(i, _)| *i == id) {
        SetParent(h, self.canvas);
        ShowWindow(h, SW_HIDE);
      }
    }
    let tips = CreateWindowExW(
      WS_EX_TOPMOST,
      wide("tooltips_class32").as_ptr(),
      ptr::null(),
      WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
      0,
      0,
      0,
      0,
      self.hwnd,
      ptr::null_mut(),
      GetModuleHandleW(ptr::null()),
      ptr::null(),
    );
    for (id, label) in [
      (SELECT, "Выбрать объект PDF: текст, изображение или контур"),
      (MASK, "Пикселизация: выделить область мышью"),
      (COVER, "Полное скрытие: выделить область мышью"),
      (MASK_SELECT, "Выбрать скрывающий блок и убрать его"),
      (UNDO, "Отменить последнее действие · Ctrl+Z"),
      (SAFE_SAVE, "Сохранить PDF с отмеченными областями"),
      (CANCEL, "Выключить инструмент / отменить операцию · Esc"),
      (CHANGE_TEXT, "Изменить выбранный текст"),
      (DELETE, "Удалить выбранный объект"),
      (MASK_DELETE, "Убрать выбранный блок · Delete"),
    ] {
      self.tool_labels.push(wide(label));
      let h = self.control(id);
      let info = TTTOOLINFOW {
        cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
        uFlags: TTF_IDISHWND | TTF_SUBCLASS,
        hwnd: GetParent(h),
        uId: h as usize,
        lpszText: self.tool_labels.last_mut().unwrap().as_mut_ptr(),
        ..std::mem::zeroed()
      };
      SendMessageW(tips, TTM_ADDTOOLW, 0, &info as *const _ as isize);
    }
  }

  pub(super) unsafe fn layout_editor_bar(&self, width: i32, top: i32) -> i32 {
    let button_width = if width >= 880 { 112 } else { 44 };
    for (i, (id, _)) in BAR.iter().enumerate() {
      self.place(
        *id,
        12 + i as i32 * (button_width + 8),
        top + 2,
        button_width,
        36,
      );
    }
    top + 46
  }

  pub(super) unsafe fn paint_editor_button(&self, item: &DRAWITEMSTRUCT) {
    let id = item.CtlID as usize;
    let active = self.editor.active(id);
    let disabled = item.itemState & ODS_DISABLED != 0;
    let pressed = item.itemState & ODS_SELECTED != 0;
    let background = if active {
      0x00f2decb
    } else if pressed {
      0x00e9e3da
    } else {
      0x00ffffff
    };
    let foreground = if disabled {
      0x00b5b1ab
    } else if active {
      0x00905a16
    } else {
      0x00534535
    };
    let dc = item.hDC;
    let r = item.rcItem;
    fill(dc, &r, background);
    let pen = CreatePen(
      PS_SOLID,
      self.unit(if active { 2 } else { 1 }),
      if active { 0x00c87818 } else { 0x00ddd7d0 },
    );
    let old_pen = SelectObject(dc, pen);
    let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
    RoundRect(
      dc,
      r.left,
      r.top,
      r.right,
      r.bottom,
      self.unit(8),
      self.unit(8),
    );
    SelectObject(dc, old_pen);
    DeleteObject(pen);
    let with_label = r.right - r.left >= self.unit(80);
    let x = if with_label {
      r.left + self.unit(10)
    } else {
      (r.left + r.right - self.unit(20)) / 2
    };
    let y = r.top + self.unit(7);
    let pen = CreatePen(PS_SOLID, self.unit(2).max(1), foreground);
    let old_pen = SelectObject(dc, pen);
    let line = |points: &[(i32, i32)]| {
      let points: Vec<_> = points
        .iter()
        .map(|(a, b)| POINT {
          x: x + self.unit(*a),
          y: y + self.unit(*b),
        })
        .collect();
      Polyline(dc, points.as_ptr(), points.len() as i32);
    };
    match id {
      SELECT => {
        line(&[
          (2, 1),
          (2, 18),
          (7, 13),
          (11, 20),
          (14, 18),
          (10, 11),
          (17, 11),
          (2, 1),
        ]);
      }
      MASK => {
        for row in 0..3 {
          for col in 0..3 {
            let r = RECT {
              left: x + self.unit(col * 7),
              top: y + self.unit(row * 7),
              right: x + self.unit(col * 7 + 5),
              bottom: y + self.unit(row * 7 + 5),
            };
            fill(
              dc,
              &r,
              if (row + col) % 2 == 0 {
                foreground
              } else {
                0x00c8bbaa
              },
            );
          }
        }
      }
      COVER => {
        fill(
          dc,
          &RECT {
            left: x,
            top: y + self.unit(3),
            right: x + self.unit(20),
            bottom: y + self.unit(17),
          },
          foreground,
        );
      }
      MASK_SELECT => {
        line(&[(0, 7), (0, 0), (7, 0)]);
        line(&[(13, 0), (20, 0), (20, 7)]);
        line(&[(20, 13), (20, 20), (13, 20)]);
        line(&[(7, 20), (0, 20), (0, 13)]);
        line(&[(7, 7), (7, 16), (16, 7), (7, 7)]);
      }
      UNDO => {
        line(&[(8, 2), (1, 8), (8, 14)]);
        line(&[(2, 8), (14, 8), (19, 12), (19, 18), (11, 18)]);
      }
      SAFE_SAVE => {
        line(&[(1, 1), (16, 1), (20, 5), (20, 20), (1, 20), (1, 1)]);
        line(&[(6, 1), (6, 7), (15, 7), (15, 1)]);
        line(&[(6, 20), (6, 13), (15, 13), (15, 20)]);
      }
      CHANGE_TEXT => {
        line(&[(3, 3), (17, 3)]);
        line(&[(10, 3), (10, 18)]);
        line(&[(5, 18), (15, 18)]);
      }
      DELETE | MASK_DELETE => {
        line(&[(2, 5), (18, 5)]);
        line(&[(6, 5), (6, 1), (14, 1), (14, 5)]);
        line(&[(4, 5), (5, 20), (15, 20), (16, 5)]);
        line(&[(8, 9), (8, 16)]);
        line(&[(12, 9), (12, 16)]);
      }
      CANCEL => {
        line(&[(3, 3), (17, 17)]);
        line(&[(17, 3), (3, 17)]);
      }
      _ => (),
    }
    SelectObject(dc, old_pen);
    SelectObject(dc, old_brush);
    DeleteObject(pen);
    let label = BAR
      .iter()
      .chain(ACTIONS.iter())
      .find(|(i, _)| *i == id)
      .map_or("", |(_, s)| *s);
    let label = if id == CANCEL && (self.printing || self.editor.busy) {
      "Стоп"
    } else {
      label
    };
    if with_label {
      let label_rect = RECT {
        left: x + self.unit(27),
        top: r.top,
        right: r.right - self.unit(4),
        bottom: r.bottom,
      };
      text(
        dc,
        label_rect,
        label,
        foreground,
        self.font,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
      );
    }
    if item.itemState & ODS_FOCUS != 0 {
      let focus = RECT {
        left: r.left + self.unit(3),
        top: r.top + self.unit(3),
        right: r.right - self.unit(3),
        bottom: r.bottom - self.unit(3),
      };
      DrawFocusRect(dc, &focus);
    }
  }
}

use super::*;
use editor::{
  CHANGE_TEXT, COVER, DELETE, MASK, MASK_DELETE, MASK_SELECT, REDO, SAVE, SELECT, UNDO,
};

pub(super) const BAR: [(usize, &str); 8] = [
  (SELECT, "Объект"),
  (MASK, "Пиксели"),
  (COVER, "Скрыть"),
  (MASK_SELECT, "Блоки"),
  (UNDO, "Назад"),
  (REDO, "Повторить"),
  (SAVE, "Сохранить"),
  (CANCEL, "Esc"),
];
pub(super) const ACTIONS: [(usize, &str); 3] = [
  (CHANGE_TEXT, "Текст"),
  (DELETE, "Удалить"),
  (MASK_DELETE, "Убрать блок"),
];

pub(super) const NAV: [(usize, &str); 21] = [
  (THEME, "Светлая / тёмная тема · выбор сохраняется"),
  (reading::FIND, "Поиск · Ctrl+F"),
  (reading::TEXT, "Выделить текст · Ctrl+C — копировать"),
  (reading::BOOKMARKS, "Закладки PDF"),
  (reading::FIND_PREV, "Предыдущее совпадение · Shift+F3"),
  (reading::FIND_NEXT, "Следующее совпадение · F3"),
  (reading::FIND_CLOSE, "Закрыть поиск · Esc"),
  (reading::HISTORY, "История действий"),
  (OPEN, "Открыть PDF · Ctrl+O"),
  (PRINT, "Печать · Ctrl+P"),
  (PREV, "Предыдущая страница"),
  (NEXT, "Следующая страница"),
  (MINUS, "Уменьшить · Ctrl+−"),
  (PLUS, "Увеличить · Ctrl++"),
  (FIT, "Страница целиком · Ctrl+0"),
  (WIDTH, "По ширине · Ctrl+2"),
  (ROTATE, "Повернуть · Ctrl+R"),
  (CONTINUOUS, "Непрерывная прокрутка"),
  (TAB_PAGES, "Страницы"),
  (TAB_LAYERS, "Слои"),
  (RESET, "Сбросить слои"),
];

pub(super) fn is_button(id: usize) -> bool {
  all_buttons().any(|(i, _)| *i == id)
}

fn all_buttons() -> impl Iterator<Item = &'static (usize, &'static str)> {
  BAR
    .iter()
    .chain(ACTIONS.iter())
    .chain(NAV.iter())
    .chain(pages::BUTTONS.iter())
    .chain(review::BUTTONS.iter())
    .chain(inline::BUTTONS.iter())
}

unsafe extern "system" fn button_proc(
  hwnd: HWND,
  msg: u32,
  wp: WPARAM,
  lp: LPARAM,
  _: usize,
  _: usize,
) -> LRESULT {
  let name = wide("AstraHover");
  if msg == WM_MOUSEMOVE && GetPropW(hwnd, name.as_ptr()).is_null() {
    SetPropW(hwnd, name.as_ptr(), 1usize as _);
    let mut tracking = TRACKMOUSEEVENT {
      cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
      dwFlags: TME_LEAVE,
      hwndTrack: hwnd,
      dwHoverTime: 0,
    };
    TrackMouseEvent(&mut tracking);
    InvalidateRect(hwnd, ptr::null(), 0);
  } else if msg == WM_MOUSELEAVE || msg == WM_NCDESTROY {
    RemovePropW(hwnd, name.as_ptr());
    InvalidateRect(hwnd, ptr::null(), 0);
  }
  DefSubclassProc(hwnd, msg, wp, lp)
}

fn icon(id: usize) -> &'static [u8] {
  match id {
    THEME => include_bytes!("../../assets/lucide/moon.png"),
    1001 => include_bytes!("../../assets/lucide/sun.png"),
    141 => include_bytes!("../../assets/lucide/save.png"),
    142 => include_bytes!("../../assets/lucide/x.png"),
    143 => include_bytes!("../../assets/lucide/scan.png"),
    144 => include_bytes!("../../assets/lucide/text-cursor-input.png"),
    120 | 121 => include_bytes!("../../assets/lucide/message-square.png"),
    122 => include_bytes!("../../assets/lucide/square.png"),
    123 => include_bytes!("../../assets/lucide/arrow-up-right.png"),
    124 => include_bytes!("../../assets/lucide/highlighter.png"),
    125 => include_bytes!("../../assets/lucide/ruler.png"),
    126 => include_bytes!("../../assets/lucide/pentagon.png"),
    127 => include_bytes!("../../assets/lucide/scan.png"),
    128 => include_bytes!("../../assets/lucide/text-cursor-input.png"),
    129 => include_bytes!("../../assets/lucide/trash.png"),
    90 => include_bytes!("../../assets/lucide/trash.png"),
    91 => include_bytes!("../../assets/lucide/copy-plus.png"),
    92 => include_bytes!("../../assets/lucide/file-plus.png"),
    93 => include_bytes!("../../assets/lucide/files.png"),
    94 => include_bytes!("../../assets/lucide/arrow-up.png"),
    95 => include_bytes!("../../assets/lucide/arrow-down.png"),
    70 => include_bytes!("../../assets/lucide/search.png"),
    71 => include_bytes!("../../assets/lucide/text-cursor.png"),
    72 => include_bytes!("../../assets/lucide/bookmark.png"),
    75 => include_bytes!("../../assets/lucide/chevron-left.png"),
    76 => include_bytes!("../../assets/lucide/chevron-right.png"),
    77 => include_bytes!("../../assets/lucide/x.png"),
    79 => include_bytes!("../../assets/lucide/rotate-ccw-clock.png"),
    10 => include_bytes!("../../assets/lucide/folder-open.png"),
    11 => include_bytes!("../../assets/lucide/printer.png"),
    12 => include_bytes!("../../assets/lucide/chevron-left.png"),
    13 => include_bytes!("../../assets/lucide/chevron-right.png"),
    14 => include_bytes!("../../assets/lucide/minus.png"),
    15 => include_bytes!("../../assets/lucide/plus.png"),
    16 => include_bytes!("../../assets/lucide/scan.png"),
    17 => include_bytes!("../../assets/lucide/move-horizontal.png"),
    18 => include_bytes!("../../assets/lucide/rotate-cw.png"),
    27 => include_bytes!("../../assets/lucide/scroll.png"),
    41 => include_bytes!("../../assets/lucide/mouse-pointer-2.png"),
    44 => include_bytes!("../../assets/lucide/grid-3x3.png"),
    52 => include_bytes!("../../assets/lucide/shield.png"),
    56 => include_bytes!("../../assets/lucide/scan-line.png"),
    47 => include_bytes!("../../assets/lucide/undo-2.png"),
    60 => include_bytes!("../../assets/lucide/redo-2.png"),
    58 => include_bytes!("../../assets/lucide/save.png"),
    23 => include_bytes!("../../assets/lucide/x.png"),
    42 => include_bytes!("../../assets/lucide/text-cursor-input.png"),
    43 => include_bytes!("../../assets/lucide/trash.png"),
    57 => include_bytes!("../../assets/lucide/trash.png"),
    24 => include_bytes!("../../assets/lucide/files.png"),
    25 => include_bytes!("../../assets/lucide/layers.png"),
    22 => include_bytes!("../../assets/lucide/rotate-ccw.png"),
    _ => &[],
  }
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
    self.tooltip = tips;
    for (id, label) in NAV
      .into_iter()
      .chain(pages::BUTTONS)
      .chain(review::BUTTONS)
      .chain(inline::BUTTONS)
      .chain([
        (SELECT, "Выбрать объект PDF"),
        (MASK, "Пикселизация области"),
        (COVER, "Полное скрытие области"),
        (MASK_SELECT, "Выбрать скрывающий блок"),
        (UNDO, "Отменить действие · Ctrl+Z"),
        (REDO, "Повторить действие · Ctrl+Y"),
        (SAVE, "Сохранить · Ctrl+S"),
        (CANCEL, "Выключить инструмент / отменить операцию · Esc"),
        (CHANGE_TEXT, "Изменить текст"),
        (DELETE, "Удалить объект · Delete"),
        (MASK_DELETE, "Убрать блок · Delete"),
      ])
    {
      self.tool_labels.push(wide(label));
      let h = self.control(id);
      SetWindowSubclass(h, Some(button_proc), 1, 0);
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

  pub(super) unsafe fn paint_editor_button(&self, item: &DRAWITEMSTRUCT) {
    let palette = theme::palette();
    let id = item.CtlID as usize;
    let active = self.editor.active(id)
      || self.review.active(id)
      || (id == CONTINUOUS && self.continuous)
      || (id == reading::TEXT && self.reader.text_mode)
      || (id == reading::FIND && self.reader.search_open)
      || (id == reading::BOOKMARKS && self.reader.bookmarks_open)
      || (id == TAB_PAGES && !self.layer_tab && !self.reader.bookmarks_open)
      || (id == TAB_LAYERS && self.layer_tab && !self.reader.bookmarks_open);
    let disabled = item.itemState & ODS_DISABLED != 0;
    let pressed = item.itemState & ODS_SELECTED != 0;
    let hover = !GetPropW(item.hwndItem, wide("AstraHover").as_ptr()).is_null();
    let background = if active {
      palette.selected
    } else if !disabled && pressed {
      palette.pressed
    } else if !disabled && hover {
      palette.hover
    } else {
      palette.panel
    };
    let foreground = if disabled {
      palette.disabled
    } else if active {
      palette.accent
    } else {
      palette.text
    };
    let dc = item.hDC;
    let r = item.rcItem;
    fill(dc, &r, palette.panel);
    let brush = CreateSolidBrush(background);
    let old_brush = SelectObject(dc, brush);
    let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
    RoundRect(
      dc,
      r.left,
      r.top,
      r.right,
      r.bottom,
      self.unit(8),
      self.unit(8),
    );
    SelectObject(dc, old_brush);
    SelectObject(dc, old_pen);
    DeleteObject(brush);
    let labelled = matches!(id, TAB_PAGES | RESET | CHANGE_TEXT | DELETE | MASK_DELETE);
    let size = self.unit(20);
    let x = if labelled {
      r.left + self.unit(10)
    } else {
      (r.left + r.right - size) / 2
    };
    let y = (r.top + r.bottom - size) / 2;
    // PNG содержат только альфа-канал официальной геометрии Lucide. Цвет задаётся темой кнопки.
    static ICONS: std::sync::OnceLock<std::collections::HashMap<usize, Vec<u8>>> =
      std::sync::OnceLock::new();
    let icons = ICONS.get_or_init(|| {
      all_buttons()
        .map(|(id, _)| *id)
        .chain([1001])
        .map(|id| {
          let mut reader = png::Decoder::new(std::io::Cursor::new(icon(id)))
            .read_info()
            .expect("Встроенная иконка");
          let mut pixels = vec![0; reader.output_buffer_size()];
          let info = reader.next_frame(&mut pixels).expect("Встроенная иконка");
          assert_eq!(info.color_type, png::ColorType::Rgba);
          (id, pixels.as_chunks::<4>().0.iter().map(|p| p[3]).collect())
        })
        .collect()
    });
    let icon_id = if id == THEME && theme::current() == theme::Theme::Dark {
      1001
    } else {
      id
    };
    if let Some(alpha) = icons.get(&icon_id) {
      let pixels: Vec<u8> = alpha
        .iter()
        .flat_map(|a| {
          let mut p = [0u8; 4];
          for (i, shift) in [16, 8, 0].iter().enumerate() {
            p[i] = (((foreground >> shift) & 255) * u32::from(*a)
              + ((background >> shift) & 255) * (255 - u32::from(*a)))
            .div_ceil(255) as u8;
          }
          p[3] = 255;
          p
        })
        .collect();
      let raster = Raster {
        width: 80,
        height: 80,
        pixels: Arc::new(pixels),
      };
      printing::draw_raster(dc, &raster, x, y, size, size);
    }
    if labelled {
      let label = NAV
        .iter()
        .chain(ACTIONS.iter())
        .find(|(i, _)| *i == id)
        .map_or("", |(_, label)| *label);
      text(
        dc,
        RECT {
          left: x + size + self.unit(7),
          right: r.right - self.unit(6),
          ..r
        },
        label,
        foreground,
        self.font,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE,
      );
    }
    if item.itemState & ODS_FOCUS != 0 {
      DrawFocusRect(
        dc,
        &RECT {
          left: r.left + 3,
          top: r.top + 3,
          right: r.right - 3,
          bottom: r.bottom - 3,
        },
      );
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn every_toolbar_button_has_a_valid_embedded_lucide_icon() {
    let mut ids = std::collections::HashSet::new();
    for (id, _) in all_buttons() {
      assert!(ids.insert(*id));
      let reader = png::Decoder::new(std::io::Cursor::new(icon(*id)))
        .read_info()
        .unwrap();
      assert_eq!((reader.info().width, reader.info().height), (80, 80));
    }
    assert_eq!(ids.len(), 52);
    let reader = png::Decoder::new(std::io::Cursor::new(icon(1001)))
      .read_info()
      .unwrap();
    assert_eq!((reader.info().width, reader.info().height), (80, 80));
  }
}

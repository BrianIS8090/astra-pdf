use super::*;
use editor::{CHANGE_TEXT, COVER, DELETE, MASK, MASK_DELETE, MASK_SELECT, SAVE, SELECT, UNDO};

pub(super) const BAR: [(usize, &str); 7] = [
  (SELECT, "Объект"),
  (MASK, "Пиксели"),
  (COVER, "Скрыть"),
  (MASK_SELECT, "Блоки"),
  (UNDO, "Назад"),
  (SAVE, "Сохранить"),
  (CANCEL, "Esc"),
];
pub(super) const ACTIONS: [(usize, &str); 3] = [
  (CHANGE_TEXT, "Текст"),
  (DELETE, "Удалить"),
  (MASK_DELETE, "Убрать блок"),
];

pub(super) const NAV: [(usize, &str); 13] = [
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
  BAR
    .iter()
    .chain(ACTIONS.iter())
    .chain(NAV.iter())
    .any(|(i, _)| *i == id)
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
    for (id, label) in NAV.into_iter().chain([
      (SELECT, "Выбрать объект PDF"),
      (MASK, "Пикселизация области"),
      (COVER, "Полное скрытие области"),
      (MASK_SELECT, "Выбрать скрывающий блок"),
      (UNDO, "Отменить действие · Ctrl+Z"),
      (SAVE, "Сохранить · Ctrl+S"),
      (CANCEL, "Выключить инструмент / отменить операцию · Esc"),
      (CHANGE_TEXT, "Изменить текст"),
      (DELETE, "Удалить объект · Delete"),
      (MASK_DELETE, "Убрать блок · Delete"),
    ]) {
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
    let id = item.CtlID as usize;
    let active = self.editor.active(id)
      || (id == CONTINUOUS && self.continuous)
      || (id == TAB_PAGES && !self.layer_tab)
      || (id == TAB_LAYERS && self.layer_tab);
    let disabled = item.itemState & ODS_DISABLED != 0;
    let pressed = item.itemState & ODS_SELECTED != 0;
    let hover = !GetPropW(item.hwndItem, wide("AstraHover").as_ptr()).is_null();
    let background = if active {
      0x00f2decb
    } else if !disabled && pressed {
      0x00e5d9ca
    } else if !disabled && hover {
      0x00eeebe7
    } else {
      0x00faf9f8
    };
    let foreground = if disabled {
      0x00b5aea7
    } else if active {
      0x00a36519
    } else {
      0x005a4b3b
    };
    let dc = item.hDC;
    let r = item.rcItem;
    fill(dc, &r, 0x00faf9f8);
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
    let labelled = matches!(
      id,
      TAB_PAGES | TAB_LAYERS | RESET | CHANGE_TEXT | DELETE | MASK_DELETE
    );
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
      NAV
        .iter()
        .chain(BAR.iter())
        .chain(ACTIONS.iter())
        .map(|(id, _)| {
          let mut reader = png::Decoder::new(std::io::Cursor::new(icon(*id)))
            .read_info()
            .expect("Встроенная иконка");
          let mut pixels = vec![0; reader.output_buffer_size()];
          let info = reader.next_frame(&mut pixels).expect("Встроенная иконка");
          assert_eq!(info.color_type, png::ColorType::Rgba);
          (
            *id,
            pixels.as_chunks::<4>().0.iter().map(|p| p[3]).collect(),
          )
        })
        .collect()
    });
    if let Some(alpha) = icons.get(&id) {
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

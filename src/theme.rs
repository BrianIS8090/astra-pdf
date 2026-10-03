use crate::printing::wide;
use std::{
  cell::{Cell, RefCell},
  collections::HashMap,
  path::Path,
  ptr,
};
use windows_sys::Win32::{
  Foundation::*,
  Graphics::{Dwm::*, Gdi::*},
  UI::{
    Controls::*,
    HiDpi::GetDpiForWindow,
    Input::KeyboardAndMouse::{GetFocus, IsWindowEnabled},
    Shell::{DefSubclassProc, SetWindowSubclass},
    WindowsAndMessaging::*,
  },
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
  #[default]
  Light,
  Dark,
}

impl Theme {
  pub fn name(self) -> &'static str {
    if self == Self::Dark {
      "dark"
    } else {
      "light"
    }
  }
  pub fn toggled(self) -> Self {
    if self == Self::Dark {
      Self::Light
    } else {
      Self::Dark
    }
  }
}

thread_local! {
  static CURRENT: Cell<Theme> = const { Cell::new(Theme::Light) };
  static MENUS: RefCell<HashMap<usize, (String, bool)>> = RefCell::new(HashMap::new());
}

pub fn current() -> Theme {
  CURRENT.with(Cell::get)
}
pub fn set(theme: Theme) {
  CURRENT.with(|c| c.set(theme));
}
pub fn load() -> Theme {
  std::env::var_os("LOCALAPPDATA")
    .map(|root| read_at(&Path::new(&root).join("AstraPDF/theme.txt")))
    .unwrap_or_default()
}
fn read_at(path: &Path) -> Theme {
  if std::fs::metadata(path).is_ok_and(|m| m.len() <= 32)
    && std::fs::read_to_string(path).is_ok_and(|s| s.trim() == "dark")
  {
    Theme::Dark
  } else {
    Theme::Light
  }
}
pub fn save(theme: Theme) -> Result<(), String> {
  let root = std::env::var_os("LOCALAPPDATA").ok_or("Не найден каталог настроек Windows.")?;
  save_at(&Path::new(&root).join("AstraPDF/theme.txt"), theme)
}
fn save_at(path: &Path, theme: Theme) -> Result<(), String> {
  std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
  crate::export::write(path, theme.name().as_bytes())
}

#[derive(Clone, Copy)]
pub struct Palette {
  pub panel: u32,
  pub canvas: u32,
  pub field: u32,
  pub text: u32,
  pub muted: u32,
  pub disabled: u32,
  pub border: u32,
  pub hover: u32,
  pub pressed: u32,
  pub selected: u32,
  pub accent: u32,
  pub shadow: u32,
}
const fn rgb(r: u32, g: u32, b: u32) -> u32 {
  r | (g << 8) | (b << 16)
}
pub fn palette() -> Palette {
  colors(current())
}
pub fn colors(theme: Theme) -> Palette {
  if theme == Theme::Dark {
    Palette {
      panel: rgb(33, 40, 50),
      canvas: rgb(24, 29, 37),
      field: rgb(40, 49, 61),
      text: rgb(231, 236, 244),
      muted: rgb(171, 184, 201),
      disabled: rgb(111, 125, 143),
      border: rgb(68, 82, 101),
      hover: rgb(49, 62, 78),
      pressed: rgb(55, 76, 99),
      selected: rgb(40, 63, 89),
      accent: rgb(123, 182, 250),
      shadow: rgb(12, 16, 22),
    }
  } else {
    Palette {
      panel: 0x00faf9f8,
      canvas: 0x00ece9e6,
      field: 0x00ffffff,
      text: 0x005a4b3b,
      muted: 0x00786858,
      disabled: 0x00b5aea7,
      border: 0x00dedede,
      hover: 0x00eeebe7,
      pressed: 0x00e5d9ca,
      selected: 0x00f2decb,
      accent: rgb(18, 86, 149),
      shadow: 0x00d3ceca,
    }
  }
}

pub unsafe fn fill(dc: HDC, rect: &RECT, color: u32) {
  SetDCBrushColor(dc, color);
  FillRect(dc, rect, GetStockObject(DC_BRUSH) as HBRUSH);
}

pub unsafe fn title(hwnd: HWND) {
  let dark = i32::from(current() == Theme::Dark);
  DwmSetWindowAttribute(
    hwnd,
    DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
    (&dark as *const i32).cast(),
    4,
  );
  let p = palette();
  for (attr, color) in [(DWMWA_CAPTION_COLOR, p.panel), (DWMWA_TEXT_COLOR, p.text)] {
    let color = if dark != 0 {
      color
    } else {
      DWMWA_COLOR_DEFAULT
    };
    DwmSetWindowAttribute(hwnd, attr as u32, (&color as *const u32).cast(), 4);
  }
}

// Цвета запрашиваются также вложенными полями редактора и модальными окнами.
pub unsafe fn control_color(msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
  if !matches!(
    msg,
    WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN
  ) {
    return None;
  }
  let dc = wp as HDC;
  let mut class = [0u16; 32];
  let n = GetClassNameW(lp as HWND, class.as_mut_ptr(), class.len() as i32);
  let edit = String::from_utf16_lossy(&class[..n.max(0) as usize]) == "Edit";
  let p = palette();
  let bg = if edit || matches!(msg, WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX) {
    p.field
  } else {
    p.panel
  };
  SetTextColor(
    dc,
    if IsWindowEnabled(lp as HWND) != 0 {
      p.text
    } else {
      p.disabled
    },
  );
  SetBkColor(dc, bg);
  // EDIT перерисовывает часть строки при вводе: прозрачный фон оставляет старые буквы.
  SetBkMode(dc, if edit { OPAQUE } else { TRANSPARENT } as i32);
  SetDCBrushColor(dc, bg);
  Some(GetStockObject(DC_BRUSH) as isize)
}

pub unsafe fn control(hwnd: HWND) {
  // Отключение системной заливки позволяет полям использовать выбранную палитру.
  let empty = wide("");
  SetWindowTheme(
    hwnd,
    if current() == Theme::Dark {
      empty.as_ptr()
    } else {
      ptr::null()
    },
    ptr::null(),
  );
  let mut class = [0u16; 32];
  let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
  match String::from_utf16_lossy(&class[..n.max(0) as usize]).as_str() {
    "SysListView32" => {
      let p = palette();
      SendMessageW(hwnd, LVM_SETBKCOLOR, 0, p.field as isize);
      SendMessageW(hwnd, LVM_SETTEXTBKCOLOR, 0, p.field as isize);
      SendMessageW(hwnd, LVM_SETTEXTCOLOR, 0, p.text as isize);
    }
    "ComboBox" => {
      SetWindowSubclass(hwnd, Some(combo_proc), 201, 0);
    }
    _ => (),
  }
  InvalidateRect(hwnd, ptr::null(), 1);
}

unsafe extern "system" fn combo_proc(
  hwnd: HWND,
  msg: u32,
  wp: WPARAM,
  lp: LPARAM,
  _: usize,
  _: usize,
) -> LRESULT {
  if current() == Theme::Dark && matches!(msg, WM_PAINT | WM_PRINTCLIENT) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let dc = if msg == WM_PAINT {
      BeginPaint(hwnd, &mut ps)
    } else {
      wp as HDC
    };
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut r);
    let p = palette();
    fill(dc, &r, p.border);
    InflateRect(&mut r, -1, -1);
    fill(dc, &r, p.field);
    let u = GetDpiForWindow(hwnd).max(96) as i32;
    let arrow = 24 * u / 96;
    r.left += 7 * u / 96;
    r.right -= arrow;
    let mut label = vec![0u16; GetWindowTextLengthW(hwnd).max(0) as usize + 1];
    GetWindowTextW(hwnd, label.as_mut_ptr(), label.len() as i32);
    let old = SelectObject(dc, SendMessageW(hwnd, WM_GETFONT, 0, 0) as HGDIOBJ);
    SetBkMode(dc, TRANSPARENT as i32);
    SetTextColor(
      dc,
      if IsWindowEnabled(hwnd) != 0 {
        p.text
      } else {
        p.disabled
      },
    );
    DrawTextW(
      dc,
      label.as_ptr(),
      -1,
      &mut r,
      DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
    r.left = r.right;
    r.right += arrow;
    DrawTextW(
      dc,
      wide("⌄").as_ptr(),
      -1,
      &mut r,
      DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );
    SelectObject(dc, old);
    if GetFocus() == hwnd {
      let mut focus: RECT = std::mem::zeroed();
      GetClientRect(hwnd, &mut focus);
      InflateRect(&mut focus, -3, -3);
      DrawFocusRect(dc, &focus);
    }
    if msg == WM_PAINT {
      EndPaint(hwnd, &ps);
    }
    return 0;
  }
  DefSubclassProc(hwnd, msg, wp, lp)
}

// Подписи остаются в MENUITEMINFO: клавиатурная навигация и доступные имена сохраняются.
pub unsafe fn menu(menu: HMENU, top: bool) {
  if menu.is_null() {
    return;
  }
  static LIGHT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
  static DARK: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
  let brush = if current() == Theme::Dark {
    &DARK
  } else {
    &LIGHT
  };
  let brush = *brush.get_or_init(|| CreateSolidBrush(palette().panel) as usize);
  let info = MENUINFO {
    cbSize: std::mem::size_of::<MENUINFO>() as u32,
    fMask: MIM_BACKGROUND,
    hbrBack: brush as HBRUSH,
    ..std::mem::zeroed()
  };
  SetMenuInfo(menu, &info);
  for index in 0..GetMenuItemCount(menu).max(0) as u32 {
    let mut label = vec![0u16; 1024];
    let mut item = MENUITEMINFOW {
      cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
      fMask: MIIM_STRING | MIIM_FTYPE | MIIM_ID | MIIM_SUBMENU,
      dwTypeData: label.as_mut_ptr(),
      cch: 1023,
      ..std::mem::zeroed()
    };
    if GetMenuItemInfoW(menu, index, 1, &mut item) == 0 {
      continue;
    }
    let key = if item.hSubMenu.is_null() {
      item.wID as usize
    } else {
      item.hSubMenu as usize
    };
    if item.fType & MFT_SEPARATOR == 0 {
      MENUS.with(|m| {
        m.borrow_mut().insert(
          key,
          (String::from_utf16_lossy(&label[..item.cch as usize]), top),
        );
      });
      item.fMask = MIIM_FTYPE | MIIM_DATA;
      item.fType |= MFT_OWNERDRAW;
      item.dwItemData = key;
      SetMenuItemInfoW(menu, index, 1, &item);
    }
    if !item.hSubMenu.is_null() {
      self::menu(item.hSubMenu, false);
    }
  }
}

unsafe fn menu_font(hwnd: HWND) -> HFONT {
  CreateFontW(
    -14 * GetDpiForWindow(hwnd).max(96) as i32 / 96,
    0,
    0,
    0,
    400,
    0,
    0,
    0,
    DEFAULT_CHARSET as u32,
    0,
    0,
    CLEARTYPE_QUALITY as u32,
    0,
    wide("Segoe UI").as_ptr(),
  )
}
pub unsafe fn menu_message(hwnd: HWND, msg: u32, lp: LPARAM) -> Option<LRESULT> {
  if msg == WM_MEASUREITEM {
    let item = &mut *(lp as *mut MEASUREITEMSTRUCT);
    if item.CtlType != ODT_MENU {
      return None;
    }
    let label = MENUS.with(|m| m.borrow().get(&item.itemData).cloned())?;
    let dc = GetDC(hwnd);
    let font = menu_font(hwnd);
    let old = SelectObject(dc, font);
    let value = wide(&label.0.replace('\t', "    "));
    let mut size: SIZE = std::mem::zeroed();
    GetTextExtentPoint32W(dc, value.as_ptr(), value.len() as i32 - 1, &mut size);
    let dpi = GetDpiForWindow(hwnd).max(96);
    item.itemWidth = size.cx as u32 + (if label.1 { 16 } else { 64 }) * dpi / 96;
    item.itemHeight = (if label.1 { 22 } else { 28 }) * dpi / 96;
    SelectObject(dc, old);
    DeleteObject(font);
    ReleaseDC(hwnd, dc);
    return Some(1);
  }
  if msg == WM_DRAWITEM {
    let item = &*(lp as *const DRAWITEMSTRUCT);
    if item.CtlType != ODT_MENU {
      return None;
    }
    let (label, top) = MENUS.with(|m| m.borrow().get(&item.itemData).cloned())?;
    let p = palette();
    fill(
      item.hDC,
      &item.rcItem,
      if item.itemState & (ODS_SELECTED | ODS_HOTLIGHT) != 0 {
        p.selected
      } else {
        p.panel
      },
    );
    let font = menu_font(hwnd);
    let old = SelectObject(item.hDC, font);
    SetBkMode(item.hDC, TRANSPARENT as i32);
    SetTextColor(
      item.hDC,
      if item.itemState & (ODS_DISABLED | ODS_GRAYED) != 0 {
        p.disabled
      } else {
        p.text
      },
    );
    let u = GetDpiForWindow(hwnd).max(96) as i32;
    let mut rect = item.rcItem;
    if !top && item.itemState & ODS_CHECKED != 0 {
      rect.right = rect.left + 24 * u / 96;
      DrawTextW(
        item.hDC,
        wide("✓").as_ptr(),
        -1,
        &mut rect,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
      );
    }
    rect = item.rcItem;
    rect.left += (if top { 8 } else { 28 }) * u / 96;
    rect.right -= (if top { 8 } else { 20 }) * u / 96;
    let (title, shortcut) = label.split_once('\t').unwrap_or((&label, ""));
    let flags = DT_SINGLELINE
      | DT_VCENTER
      | if item.itemState & ODS_NOACCEL != 0 {
        DT_HIDEPREFIX
      } else {
        0
      };
    DrawTextW(item.hDC, wide(title).as_ptr(), -1, &mut rect, flags);
    if !shortcut.is_empty() {
      DrawTextW(
        item.hDC,
        wide(shortcut).as_ptr(),
        -1,
        &mut rect,
        flags | DT_RIGHT,
      );
    }
    SelectObject(item.hDC, old);
    DeleteObject(font);
    return Some(1);
  }
  None
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn edit_fields_erase_previous_glyphs_in_both_themes() {
    unsafe {
      let edit = CreateWindowExW(
        0,
        wide("EDIT").as_ptr(),
        wide("Старый текст").as_ptr(),
        WS_POPUP | ES_MULTILINE as u32,
        0,
        0,
        200,
        80,
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null(),
      );
      assert!(!edit.is_null());
      let dc = CreateCompatibleDC(ptr::null_mut());
      assert!(!dc.is_null());
      let mut results = Vec::new();
      for theme in [Theme::Light, Theme::Dark] {
        set(theme);
        for readonly in [false, true] {
          SendMessageW(edit, EM_SETREADONLY, readonly as usize, 0);
          let message = if readonly {
            WM_CTLCOLORSTATIC
          } else {
            WM_CTLCOLOREDIT
          };
          control_color(message, dc as usize, edit as isize).unwrap();
          // При частичной перерисовке EDIT должен закрашивать прежние буквы.
          results.push((theme, readonly, GetBkMode(dc), GetBkColor(dc)));
        }
      }
      set(Theme::Light);
      DeleteDC(dc);
      DestroyWindow(edit);
      for (theme, readonly, mode, background) in results {
        assert_eq!(mode, OPAQUE as i32, "{theme:?}, только чтение: {readonly}");
        assert_eq!(background, colors(theme).field);
      }
    }
  }
  #[test]
  fn theme_preference_round_trips_and_recovers_from_invalid_values() {
    let dir = std::env::temp_dir().join(format!("astra-theme-{}", std::process::id()));
    let path = dir.join("theme.txt");
    assert_eq!(read_at(&path), Theme::Light);
    for value in [Theme::Dark, Theme::Light] {
      save_at(&path, value).unwrap();
      assert_eq!(read_at(&path), value);
    }
    for bytes in [b"unknown".as_slice(), &[255], &[b'x'; 40]] {
      std::fs::write(&path, bytes).unwrap();
      assert_eq!(read_at(&path), Theme::Light);
    }
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
  }
  #[test]
  fn both_themes_keep_readable_text_and_selected_tools() {
    fn luminance(color: u32) -> f64 {
      let c = |shift: u32| {
        let v = ((color >> shift) & 255u32) as f64 / 255.;
        if v <= 0.04045 {
          v / 12.92
        } else {
          ((v + 0.055) / 1.055).powf(2.4)
        }
      };
      0.2126 * c(0) + 0.7152 * c(8) + 0.0722 * c(16)
    }
    fn contrast(a: u32, b: u32) -> f64 {
      let (a, b) = (luminance(a), luminance(b));
      (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    for theme in [Theme::Light, Theme::Dark] {
      let p = colors(theme);
      for bg in [p.panel, p.canvas, p.field] {
        assert!(contrast(p.text, bg) >= 4.5);
        assert!(contrast(p.muted, bg) >= 4.5);
      }
      assert!(contrast(p.accent, p.selected) >= 4.5);
      assert_ne!(p.selected, p.panel);
      assert_eq!(theme.toggled().toggled(), theme);
    }
  }
}

use crate::printing::wide;
use std::{
  path::{Path, PathBuf},
  ptr,
};
use windows_sys::Win32::{
  Foundation::*,
  Graphics::Gdi::*,
  System::LibraryLoader::GetModuleHandleW,
  UI::{
    Controls::Dialogs::*, Controls::*, HiDpi::GetDpiForWindow, Input::KeyboardAndMouse::*,
    WindowsAndMessaging::*,
  },
};

struct State {
  done: bool,
  accepted: bool,
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
  if msg == WM_NCCREATE {
    SetWindowLongPtrW(
      hwnd,
      GWLP_USERDATA,
      (*(lp as *const CREATESTRUCTW)).lpCreateParams as isize,
    );
  }
  let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
  if !state.is_null() && (msg == WM_CLOSE || (msg == WM_COMMAND && [1, 2].contains(&(wp & 65535))))
  {
    (*state).done = true;
    (*state).accepted = msg == WM_COMMAND && wp & 65535 == 1;
    return 0;
  }
  DefWindowProcW(hwnd, msg, wp, lp)
}

pub unsafe fn prompt(
  owner: HWND,
  title: &str,
  label: &str,
  value: &str,
  readonly: bool,
) -> Option<String> {
  let instance = GetModuleHandleW(ptr::null());
  let class = wide("AstraPdfDialog");
  let wc = WNDCLASSW {
    lpfnWndProc: Some(proc),
    hInstance: instance,
    lpszClassName: class.as_ptr(),
    hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
    hbrBackground: (COLOR_WINDOW + 1) as HBRUSH,
    ..std::mem::zeroed()
  };
  RegisterClassW(&wc);
  let dpi = GetDpiForWindow(owner).max(96);
  let u = |n: i32| n * dpi as i32 / 96;
  let mut area: RECT = std::mem::zeroed();
  GetWindowRect(owner, &mut area);
  let height = if readonly { 530 } else { 360 };
  let mut state = State {
    done: false,
    accepted: false,
  };
  let hwnd = CreateWindowExW(
    WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
    class.as_ptr(),
    wide(title).as_ptr(),
    WS_CAPTION | WS_SYSMENU | WS_POPUP,
    area.left + ((area.right - area.left - u(620)) / 2).max(0),
    area.top + ((area.bottom - area.top - u(height)) / 2).max(0),
    u(620),
    u(height),
    owner,
    ptr::null_mut(),
    instance,
    (&mut state as *mut State).cast(),
  );
  if hwnd.is_null() {
    return None;
  }
  let font = CreateFontW(
    -u(15),
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
  );
  let child = |class: &str, text: &str, style, id, x, y, w, h| {
    let control = CreateWindowExW(
      0,
      wide(class).as_ptr(),
      wide(text).as_ptr(),
      WS_CHILD | WS_VISIBLE | style,
      u(x),
      u(y),
      u(w),
      u(h),
      hwnd,
      id as _,
      instance,
      ptr::null(),
    );
    SendMessageW(control, WM_SETFONT, font as usize, 1);
    control
  };
  child("STATIC", label, 0, 100, 16, 12, 570, 84);
  let edit = child(
    "EDIT",
    &value.replace('\n', "\r\n"),
    WS_TABSTOP
      | WS_BORDER
      | WS_VSCROLL
      | ES_MULTILINE as u32
      | ES_AUTOVSCROLL as u32
      | if readonly { ES_READONLY as u32 } else { 0 },
    101,
    16,
    100,
    570,
    height - 200,
  );
  SendMessageW(
    edit,
    EM_SETLIMITTEXT,
    if readonly { 1_000_000 } else { 16_384 },
    0,
  );
  child(
    "BUTTON",
    if readonly {
      "Закрыть"
    } else {
      "Продолжить"
    },
    WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
    1,
    440,
    height - 85,
    145,
    34,
  );
  if !readonly {
    child("BUTTON", "Отмена", WS_TABSTOP, 2, 285, height - 85, 140, 34);
  }
  EnableWindow(owner, 0);
  ShowWindow(hwnd, SW_SHOW);
  SetFocus(edit);
  if !readonly {
    SendMessageW(edit, EM_SETSEL, 0, -1);
  }
  let mut msg: MSG = std::mem::zeroed();
  loop {
    if state.done {
      break;
    }
    let result = GetMessageW(&mut msg, ptr::null_mut(), 0, 0);
    if result <= 0 {
      if result == 0 {
        PostQuitMessage(msg.wParam as i32);
      }
      break;
    }
    if msg.message == WM_KEYDOWN && msg.wParam == VK_ESCAPE as usize {
      break;
    }
    if IsDialogMessageW(hwnd, &msg) == 0 {
      TranslateMessage(&msg);
      DispatchMessageW(&msg);
    }
  }
  let result = if state.accepted {
    let mut buffer = vec![0u16; GetWindowTextLengthW(edit) as usize + 1];
    let n = GetWindowTextW(edit, buffer.as_mut_ptr(), buffer.len() as i32);
    Some(String::from_utf16_lossy(&buffer[..n as usize]).replace("\r\n", "\n"))
  } else {
    None
  };
  EnableWindow(owner, 1);
  DestroyWindow(hwnd);
  SetActiveWindow(owner);
  DeleteObject(font);
  result
}

pub unsafe fn file(
  owner: HWND,
  title: &str,
  extension: &str,
  suggested: &Path,
  save: bool,
) -> Option<PathBuf> {
  use std::os::windows::ffi::OsStringExt;
  let mut name = vec![0u16; 32768];
  let initial = dialog_path(suggested);
  if initial.len() >= name.len() {
    return None;
  }
  name[..initial.len()].copy_from_slice(&initial);
  let filter = wide(&format!("Файлы {extension}\0*.{extension}\0\0"));
  let title = wide(title);
  let ext = wide(extension);
  let mut info: OPENFILENAMEW = std::mem::zeroed();
  info.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
  info.hwndOwner = owner;
  info.lpstrFilter = filter.as_ptr();
  info.lpstrTitle = title.as_ptr();
  info.lpstrDefExt = ext.as_ptr();
  info.lpstrFile = name.as_mut_ptr();
  info.nMaxFile = name.len() as u32;
  info.Flags = OFN_EXPLORER
    | OFN_NOCHANGEDIR
    | OFN_PATHMUSTEXIST
    | if save {
      OFN_OVERWRITEPROMPT
    } else {
      OFN_FILEMUSTEXIST
    };
  let result = if save {
    GetSaveFileNameW(&mut info)
  } else {
    GetOpenFileNameW(&mut info)
  };
  if result == 0 {
    let error = CommDlgExtendedError();
    if error != 0 {
      MessageBoxW(
        owner,
        wide(&format!(
          "Не удалось открыть окно выбора файла (код Windows: {error:#x})."
        ))
        .as_ptr(),
        wide("Astra PDF").as_ptr(),
        MB_OK | MB_ICONERROR,
      );
    }
    return None;
  }
  let n = name.iter().position(|&c| c == 0)?;
  Some(std::ffi::OsString::from_wide(&name[..n]).into())
}

fn dialog_path(path: &Path) -> Vec<u16> {
  use std::os::windows::ffi::OsStrExt;
  // Общий диалог Windows не принимает смешанные разделители в начальном имени.
  path
    .as_os_str()
    .encode_wide()
    .map(|c| if c == b'/' as u16 { b'\\' as u16 } else { c })
    .collect()
}

#[cfg(test)]
mod tests {
  #[test]
  fn native_file_dialog_paths_normalize_slashes_and_keep_unicode() {
    let value = super::dialog_path(std::path::Path::new(
      "C:/Users/Test/AstraPDF/Private\\Originals/оригинал.astravault",
    ));
    assert_eq!(
      String::from_utf16(&value).unwrap(),
      "C:\\Users\\Test\\AstraPDF\\Private\\Originals\\оригинал.astravault"
    );
  }
}

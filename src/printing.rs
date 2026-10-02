use crate::{
  pdf::Raster,
  print_pipeline::{self, Capabilities, Device, Settings},
};
use std::{
  ptr,
  sync::{atomic::AtomicBool, Arc},
};
use windows_sys::Win32::{
  Foundation::*, Graphics::Gdi::*, Storage::Xps::*, UI::Controls::Dialogs::*,
};

pub fn wide(s: &str) -> Vec<u16> {
  s.encode_utf16().chain(Some(0)).collect()
}

pub struct PrintJob {
  pub dc: usize,
  pub first: usize,
  pub last: usize,
  pub title: String,
  pub output: Option<String>,
  pub cancel: Arc<AtomicBool>,
}

impl Drop for PrintJob {
  fn drop(&mut self) {
    unsafe {
      DeleteDC(self.dc as HDC);
    }
  }
}

pub unsafe fn dialog(
  hwnd: HWND,
  pages: usize,
  current: usize,
  title: &str,
  cancel: Arc<AtomicBool>,
) -> Result<Option<PrintJob>, String> {
  use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
  if pages == 0 || current >= pages || pages > u32::MAX as usize {
    return Err("Некорректный документ для печати.".into());
  }
  let com = CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32);
  if com < 0 {
    return Err(format!(
      "Не удалось подготовить диалог печати: 0x{:08X}",
      com as u32
    ));
  }
  let mut range = PRINTPAGERANGE {
    nFromPage: 1,
    nToPage: pages as u32,
  };
  let mut pd: PRINTDLGEXW = std::mem::zeroed();
  pd.lStructSize = std::mem::size_of::<PRINTDLGEXW>() as u32;
  pd.hwndOwner = hwnd;
  pd.Flags = PD_RETURNDC | PD_NOSELECTION | PD_USEDEVMODECOPIESANDCOLLATE | PD_HIDEPRINTTOFILE;
  pd.nMinPage = 1;
  pd.nMaxPage = pages as u32;
  pd.nPageRanges = 0;
  pd.ExclusionFlags = DM_COPIES | DM_COLLATE;
  pd.nMaxPageRanges = 1;
  pd.lpPageRanges = &mut range;
  pd.nStartPage = START_PAGE_GENERAL;
  pd.nCopies = 1;
  let result = PrintDlgExW(&mut pd);
  CoUninitialize();
  let accepted = result >= 0 && pd.dwResultAction == PD_RESULT_PRINT;
  if !pd.hDevMode.is_null() {
    GlobalFree(pd.hDevMode);
  }
  if !pd.hDevNames.is_null() {
    GlobalFree(pd.hDevNames);
  }
  if !accepted {
    if !pd.hDC.is_null() {
      DeleteDC(pd.hDC);
    }
    return if result >= 0 {
      Ok(None)
    } else {
      Err(format!("Ошибка диалога печати: 0x{:08X}", result as u32))
    };
  }
  if pd.hDC.is_null() {
    return Err("Принтер не предоставил устройство печати.".into());
  }
  let (first, last) = match selected_range(pages, current, pd.Flags, range.nFromPage, range.nToPage)
  {
    Ok(range) => range,
    Err(e) => {
      DeleteDC(pd.hDC);
      return Err(e);
    }
  };
  Ok(Some(PrintJob {
    dc: pd.hDC as usize,
    first,
    last,
    title: title.into(),
    output: None,
    cancel,
  }))
}

fn selected_range(
  pages: usize,
  current: usize,
  flags: u32,
  first: u32,
  last: u32,
) -> Result<(usize, usize), String> {
  if pages == 0 || current >= pages {
    return Err("Некорректный документ для печати.".into());
  }
  let range = if flags & PD_CURRENTPAGE != 0 {
    (current, current)
  } else if flags & PD_PAGENUMS != 0 {
    if first == 0 || last == 0 {
      return Err("Номер страницы должен начинаться с 1.".into());
    }
    ((first - 1) as usize, (last - 1) as usize)
  } else {
    (0, pages - 1)
  };
  if range.0 > range.1 || range.1 >= pages {
    return Err("Некорректный диапазон печати.".into());
  }
  Ok(range)
}

pub unsafe fn bitmap_info(image: &Raster) -> BITMAPINFO {
  let mut info: BITMAPINFO = std::mem::zeroed();
  info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
  info.bmiHeader.biWidth = image.width;
  info.bmiHeader.biHeight = -image.height;
  info.bmiHeader.biPlanes = 1;
  info.bmiHeader.biBitCount = 32;
  info.bmiHeader.biCompression = BI_RGB;
  info
}

pub unsafe fn draw_raster(dc: HDC, image: &Raster, x: i32, y: i32, w: i32, h: i32) -> bool {
  let info = bitmap_info(image);
  SetStretchBltMode(dc, HALFTONE);
  SetBrushOrgEx(dc, 0, 0, ptr::null_mut());
  let result = StretchDIBits(
    dc,
    x,
    y,
    w,
    h,
    0,
    0,
    image.width,
    image.height,
    image.pixels.as_ptr().cast(),
    &info,
    DIB_RGB_COLORS,
    SRCCOPY,
  );
  result != 0 && result != -1
}

pub fn print(
  sizes: &[(f64, f64)],
  job: PrintJob,
  rotation: i32,
  render: impl FnMut(usize, i32, i32, i32) -> Result<Raster, String>,
  progress: impl Fn(usize, usize),
) -> Result<(), String> {
  let settings = Settings {
    first: job.first,
    last: job.last,
    rotation,
    cancel: &job.cancel,
  };
  print_pipeline::run(&mut NativePrinter(&job), sizes, settings, render, progress)
}

struct NativePrinter<'a>(&'a PrintJob);

impl Device for NativePrinter<'_> {
  fn begin(&mut self) -> Result<(), String> {
    unsafe {
      let title = wide(&self.0.title);
      let output = self.0.output.as_ref().map(|s| wide(s));
      let mut info: DOCINFOW = std::mem::zeroed();
      info.cbSize = std::mem::size_of::<DOCINFOW>() as i32;
      info.lpszDocName = title.as_ptr();
      info.lpszOutput = output.as_ref().map_or(ptr::null(), |v| v.as_ptr());
      if StartDocW(self.0.dc as HDC, &info) <= 0 {
        Err("Принтер не принял задание.".into())
      } else {
        Ok(())
      }
    }
  }

  fn capabilities(&mut self) -> Result<Capabilities, String> {
    unsafe {
      let dc = self.0.dc as HDC;
      Ok(Capabilities {
        dpi: (
          GetDeviceCaps(dc, LOGPIXELSX as i32),
          GetDeviceCaps(dc, LOGPIXELSY as i32),
        ),
        area: (
          GetDeviceCaps(dc, HORZRES as i32),
          GetDeviceCaps(dc, VERTRES as i32),
        ),
      })
    }
  }

  fn begin_page(&mut self) -> Result<(), String> {
    if unsafe { StartPage(self.0.dc as HDC) } <= 0 {
      Err("Не удалось начать печать страницы.".into())
    } else {
      Ok(())
    }
  }

  fn draw(&mut self, image: &Raster, rect: (i32, i32, i32, i32)) -> Result<(), String> {
    if unsafe { draw_raster(self.0.dc as HDC, image, rect.0, rect.1, rect.2, rect.3) } {
      Ok(())
    } else {
      Err("Драйвер отклонил изображение страницы.".into())
    }
  }

  fn end_page(&mut self) -> Result<(), String> {
    if unsafe { EndPage(self.0.dc as HDC) } <= 0 {
      Err("Не удалось завершить печать страницы.".into())
    } else {
      Ok(())
    }
  }

  fn commit(&mut self) -> Result<(), String> {
    if unsafe { EndDoc(self.0.dc as HDC) } <= 0 {
      Err("Не удалось передать документ в очередь печати.".into())
    } else {
      Ok(())
    }
  }

  fn abort(&mut self) {
    unsafe {
      AbortDoc(self.0.dc as HDC);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn print_dialog_preserves_large_ranges_and_current_page() {
    assert_eq!(
      selected_range(100_000, 70_000, PD_CURRENTPAGE, 1, 1).unwrap(),
      (70_000, 70_000)
    );
    assert_eq!(
      selected_range(100_000, 0, PD_PAGENUMS, 70_001, 80_000).unwrap(),
      (70_000, 79_999)
    );
    assert_eq!(selected_range(100_000, 0, 0, 1, 1).unwrap(), (0, 99_999));
    for (first, last) in [(0, 1), (3, 2), (1, 100_001)] {
      assert!(selected_range(100_000, 0, PD_PAGENUMS, first, last).is_err());
    }
  }
}

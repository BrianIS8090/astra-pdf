use crate::model::Region;
use libloading::Library;
use std::{cell::RefCell, collections::VecDeque, ffi::c_void, path::Path, ptr, rc::Rc, sync::Arc};

type Handle = *mut c_void;
type Init = unsafe extern "C" fn();
type Load = unsafe extern "C" fn(*const c_void, usize, *const i8) -> Handle;
type Close = unsafe extern "C" fn(Handle);
type Count = unsafe extern "C" fn(Handle) -> i32;
type Size = unsafe extern "C" fn(Handle, i32, *mut f64, *mut f64) -> i32;
type Page = unsafe extern "C" fn(Handle, i32) -> Handle;
type Bitmap = unsafe extern "C" fn(i32, i32, i32, *mut c_void, i32) -> Handle;
type Fill = unsafe extern "C" fn(Handle, i32, i32, i32, i32, u32) -> i32;
type Render = unsafe extern "C" fn(Handle, Handle, i32, i32, i32, i32, i32, i32);
type Error = unsafe extern "C" fn() -> u32;

pub struct Api {
  _library: Library,
  destroy: Init,
  load: Load,
  close: Close,
  count: Count,
  size: Size,
  page: Page,
  close_page: Close,
  bitmap: Bitmap,
  close_bitmap: Close,
  fill: Fill,
  render: Render,
  error: Error,
}

impl Api {
  // PDFium вызывается только из одного рабочего потока; Rc запрещает перенос между потоками.
  pub fn new(path: &Path) -> Result<Rc<Self>, String> {
    unsafe {
      let lib = Library::new(path).map_err(|e| format!("Не удалось загрузить PDFium: {e}"))?;
      macro_rules! sym {
        ($name:literal, $t:ty) => {
          *lib
            .get::<$t>(concat!($name, "\0").as_bytes())
            .map_err(|e| e.to_string())?
        };
      }
      let init = sym!("FPDF_InitLibrary", Init);
      let api = Self {
        destroy: sym!("FPDF_DestroyLibrary", Init),
        load: sym!("FPDF_LoadMemDocument64", Load),
        close: sym!("FPDF_CloseDocument", Close),
        count: sym!("FPDF_GetPageCount", Count),
        size: sym!("FPDF_GetPageSizeByIndex", Size),
        page: sym!("FPDF_LoadPage", Page),
        close_page: sym!("FPDF_ClosePage", Close),
        bitmap: sym!("FPDFBitmap_CreateEx", Bitmap),
        close_bitmap: sym!("FPDFBitmap_Destroy", Close),
        fill: sym!("FPDFBitmap_FillRect", Fill),
        render: sym!("FPDF_RenderPageBitmap", Render),
        error: sym!("FPDF_GetLastError", Error),
        _library: lib,
      };
      init();
      Ok(Rc::new(api))
    }
  }
}

impl Drop for Api {
  fn drop(&mut self) {
    unsafe { (self.destroy)() };
  }
}

pub struct Pdf {
  api: Rc<Api>,
  handle: Handle,
  _bytes: Arc<Vec<u8>>,
  pub sizes: Vec<(f64, f64)>,
  pages: RefCell<VecDeque<(usize, Handle)>>,
}

#[derive(Clone)]
pub struct Raster {
  pub width: i32,
  pub height: i32,
  pub pixels: Arc<Vec<u8>>,
}

impl Pdf {
  pub fn open(api: Rc<Api>, bytes: Arc<Vec<u8>>) -> Result<Self, String> {
    unsafe {
      let handle = (api.load)(bytes.as_ptr().cast(), bytes.len(), ptr::null());
      if handle.is_null() {
        return Err(match (api.error)() {
          4 => "PDF защищён паролем. В этой версии защищённые документы не поддерживаются.".into(),
          3 => "Файл повреждён или имеет неподдерживаемый формат PDF.".into(),
          n => format!("Не удалось открыть PDF (код {n})."),
        });
      }
      let mut pdf = Self {
        api,
        handle,
        _bytes: bytes,
        sizes: vec![],
        pages: RefCell::new(VecDeque::new()),
      };
      let count = (pdf.api.count)(handle);
      if count <= 0 {
        return Err("В документе нет страниц.".into());
      }
      for i in 0..count {
        let (mut w, mut h) = (0., 0.);
        if (pdf.api.size)(handle, i, &mut w, &mut h) == 0
          || !w.is_finite()
          || !h.is_finite()
          || w <= 0.
          || h <= 0.
        {
          return Err(format!("Некорректный размер страницы {}.", i + 1));
        }
        pdf.sizes.push((w, h));
      }
      Ok(pdf)
    }
  }

  #[cfg(test)]
  pub fn render(
    &self,
    index: usize,
    width: i32,
    height: i32,
    rotation: i32,
    printing: bool,
  ) -> Result<Raster, String> {
    self.render_region(index, (width, height), rotation, printing, None)
  }

  pub fn render_region(
    &self,
    index: usize,
    size: (i32, i32),
    rotation: i32,
    printing: bool,
    region: Option<Region>,
  ) -> Result<Raster, String> {
    let area = region.unwrap_or(Region {
      x: 0,
      y: 0,
      width: size.0,
      height: size.1,
    });
    let (width, height) = (area.width, area.height);
    if index >= self.sizes.len()
      || size.0 <= 0
      || size.1 <= 0
      || size.0 > 1_000_000
      || size.1 > 1_000_000
      || !area.valid(size)
    {
      return Err("Размер страницы превышает предел отрисовки (24 млн пикселей).".into());
    }
    let mut pixels = vec![255u8; width as usize * height as usize * 4];
    unsafe {
      // Повторные участки используют уже разобранную страницу; память удерживают только две страницы.
      let mut pages = self.pages.borrow_mut();
      let page = if let Some(pos) = pages.iter().position(|(i, _)| *i == index) {
        let item = pages.remove(pos).unwrap();
        pages.push_front(item);
        item.1
      } else {
        if pages.len() >= 2 {
          (self.api.close_page)(pages.pop_back().unwrap().1);
        }
        let handle = (self.api.page)(self.handle, index as i32);
        if !handle.is_null() {
          pages.push_front((index, handle));
        }
        handle
      };
      if page.is_null() {
        return Err("Не удалось прочитать страницу.".into());
      }
      let bitmap = (self.api.bitmap)(width, height, 4, pixels.as_mut_ptr().cast(), width * 4);
      if bitmap.is_null() {
        return Err("Не удалось выделить память для страницы.".into());
      }
      (self.api.fill)(bitmap, 0, 0, width, height, 0xffffffff);
      (self.api.render)(
        bitmap,
        page,
        -area.x,
        -area.y,
        size.0,
        size.1,
        rotation.rem_euclid(4),
        1 | 0x200 | if printing { 0x800 } else { 0 },
      );
      (self.api.close_bitmap)(bitmap);
    }
    Ok(Raster {
      width,
      height,
      pixels: Arc::new(pixels),
    })
  }
}

impl Drop for Pdf {
  fn drop(&mut self) {
    for (_, page) in self.pages.get_mut().drain(..) {
      unsafe { (self.api.close_page)(page) };
    }
    unsafe { (self.api.close)(self.handle) };
  }
}

impl Raster {
  pub fn save_png(&self, path: &Path) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(file, self.width as u32, self.height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut rgba = self.pixels.as_ref().clone();
    for p in rgba.as_chunks_mut::<4>().0 {
      p.swap(0, 2);
      p[3] = 255;
    }
    encoder
      .write_header()
      .map_err(|e| e.to_string())?
      .write_image_data(&rgba)
      .map_err(|e| e.to_string())
  }
}

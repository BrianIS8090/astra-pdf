use super::*;
use crate::editing::{Operation, PageObject};
use skrifa::MetadataProvider;

#[repr(C)]
struct Matrix {
  a: f32,
  b: f32,
  c: f32,
  d: f32,
  e: f32,
  f: f32,
}
#[repr(C)]
struct Output {
  version: i32,
  write: unsafe extern "C" fn(*mut Output, *const c_void, u32) -> i32,
  bytes: Vec<u8>,
}
unsafe extern "C" fn write(out: *mut Output, data: *const c_void, size: u32) -> i32 {
  let out = &mut *out;
  if size as usize > 96_000_000usize.saturating_sub(out.bytes.len())
    || out.bytes.try_reserve(size as usize).is_err()
  {
    return 0;
  }
  out
    .bytes
    .extend_from_slice(std::slice::from_raw_parts(data.cast::<u8>(), size as usize));
  1
}

impl Pdf {
  pub fn edit(&self, operation: &Operation) -> Result<Vec<u8>, String> {
    if let Operation::Extract { pages } = operation {
      return crate::editing::extract(&self._bytes, pages);
    }
    let index = match operation {
      Operation::Objects { page }
      | Operation::Delete { page, .. }
      | Operation::Text { page, .. } => *page,
      _ => unreachable!(),
    };
    if index >= self.sizes.len() {
      return Err("Страница отсутствует.".into());
    }
    unsafe {
      macro_rules! sym {
        ($name:literal,$t:ty) => {
          *self
            .api
            ._library
            .get::<$t>(concat!($name, "\0").as_bytes())
            .map_err(|e| e.to_string())?
        };
      }
      let count = sym!("FPDFPage_CountObjects", unsafe extern "C" fn(Handle) -> i32);
      let get = sym!(
        "FPDFPage_GetObject",
        unsafe extern "C" fn(Handle, i32) -> Handle
      );
      let kind = sym!("FPDFPageObj_GetType", unsafe extern "C" fn(Handle) -> i32);
      let bounds = sym!(
        "FPDFPageObj_GetBounds",
        unsafe extern "C" fn(Handle, *mut f32, *mut f32, *mut f32, *mut f32) -> i32
      );
      let device = sym!(
        "FPDF_PageToDevice",
        unsafe extern "C" fn(Handle, i32, i32, i32, i32, i32, f64, f64, *mut i32, *mut i32) -> i32
      );
      let load_text = sym!("FPDFText_LoadPage", unsafe extern "C" fn(Handle) -> Handle);
      let close_text = sym!("FPDFText_ClosePage", Close);
      let text = sym!(
        "FPDFTextObj_GetText",
        unsafe extern "C" fn(Handle, Handle, *mut u16, u32) -> u32
      );
      let remove = sym!(
        "FPDFPage_RemoveObject",
        unsafe extern "C" fn(Handle, Handle) -> i32
      );
      let destroy = sym!("FPDFPageObj_Destroy", Close);
      let generate = sym!(
        "FPDFPage_GenerateContent",
        unsafe extern "C" fn(Handle) -> i32
      );
      let save = sym!(
        "FPDF_SaveAsCopy",
        unsafe extern "C" fn(Handle, *mut Output, u32) -> i32
      );
      let page = (self.api.page)(self.handle, index as i32);
      if page.is_null() {
        return Err("Не удалось загрузить страницу для редактирования.".into());
      }
      let result = (|| {
        let n = count(page);
        if !(0..=100_000).contains(&n) {
          return Err("Слишком много объектов на странице.".into());
        }
        if matches!(operation, Operation::Objects { .. }) {
          let text_page = load_text(page);
          if text_page.is_null() {
            return Err("Не удалось прочитать объекты страницы.".into());
          }
          let mut objects = Vec::new();
          for i in 0..n {
            let object = get(page, i);
            let (mut l, mut b, mut r, mut t) = (0., 0., 0., 0.);
            if bounds(object, &mut l, &mut b, &mut r, &mut t) == 0 {
              continue;
            }
            let (mut x1, mut y1, mut x2, mut y2) = (0, 0, 0, 0);
            if device(
              page, 0, 0, 1_000_000, 1_000_000, 0, l as f64, b as f64, &mut x1, &mut y1,
            ) == 0
              || device(
                page, 0, 0, 1_000_000, 1_000_000, 0, r as f64, t as f64, &mut x2, &mut y2,
              ) == 0
            {
              continue;
            }
            let object_kind = kind(object);
            let mut value = String::new();
            if object_kind == 1 {
              let bytes = text(object, text_page, ptr::null_mut(), 0);
              if bytes > 0 && bytes <= 32768 && bytes % 2 == 0 {
                let mut buffer = vec![0u16; bytes as usize / 2];
                if text(object, text_page, buffer.as_mut_ptr(), bytes) == bytes {
                  value = String::from_utf16_lossy(&buffer[..buffer.len() - 1]);
                }
              }
            }
            objects.push(PageObject {
              index: i as usize,
              kind: object_kind,
              bounds: [
                x1.min(x2) as f64 / 1_000_000.,
                y1.min(y2) as f64 / 1_000_000.,
                x1.max(x2) as f64 / 1_000_000.,
                y1.max(y2) as f64 / 1_000_000.,
              ],
              text: value,
            });
          }
          close_text(text_page);
          return serde_json::to_vec(&objects).map_err(|e| e.to_string());
        }
        let object_index = match operation {
          Operation::Delete { object, .. } | Operation::Text { object, .. } => *object,
          _ => unreachable!(),
        };
        if object_index >= n as usize {
          return Err("Объект отсутствует. Выберите его повторно.".into());
        }
        let object = get(page, object_index as i32);
        if let Operation::Text { text, .. } = operation {
          if kind(object) != 1
            || text.is_empty()
            || text.len() > 16_384
            || text.chars().any(char::is_control)
          {
            return Err(
              "Выберите отдельный текстовый объект и введите одну непустую строку.".into(),
            );
          }
          let marks = sym!(
            "FPDFPageObj_CountMarks",
            unsafe extern "C" fn(Handle) -> i32
          );
          if marks(object) != 0 {
            return Err("Текст находится в слое или размеченной группе. Замена такого текста пока отключена, чтобы сохранить структуру PDF.".into());
          }
          let clip = sym!(
            "FPDFPageObj_GetClipPath",
            unsafe extern "C" fn(Handle) -> Handle
          );
          let clip_count = sym!(
            "FPDFClipPath_CountPaths",
            unsafe extern "C" fn(Handle) -> i32
          );
          let clip_path = clip(object);
          if !clip_path.is_null() && clip_count(clip_path) > 0 {
            return Err(
              "Текст ограничен обтравочным контуром. Замена пока не поддерживается.".into(),
            );
          }
          let get_size = sym!(
            "FPDFTextObj_GetFontSize",
            unsafe extern "C" fn(Handle, *mut f32) -> i32
          );
          let get_matrix = sym!(
            "FPDFPageObj_GetMatrix",
            unsafe extern "C" fn(Handle, *mut Matrix) -> i32
          );
          let set_matrix = sym!(
            "FPDFPageObj_SetMatrix",
            unsafe extern "C" fn(Handle, *const Matrix) -> i32
          );
          let get_color = sym!(
            "FPDFPageObj_GetFillColor",
            unsafe extern "C" fn(Handle, *mut u32, *mut u32, *mut u32, *mut u32) -> i32
          );
          let set_color = sym!(
            "FPDFPageObj_SetFillColor",
            unsafe extern "C" fn(Handle, u32, u32, u32, u32) -> i32
          );
          let load_font = sym!(
            "FPDFText_LoadFont",
            unsafe extern "C" fn(Handle, *const u8, u32, i32, i32) -> Handle
          );
          let close_font = sym!("FPDFFont_Close", Close);
          let new_text = sym!(
            "FPDFPageObj_CreateTextObj",
            unsafe extern "C" fn(Handle, Handle, f32) -> Handle
          );
          let set_text = sym!(
            "FPDFText_SetText",
            unsafe extern "C" fn(Handle, *const u16) -> i32
          );
          let insert = sym!(
            "FPDFPage_InsertObjectAtIndex",
            unsafe extern "C" fn(Handle, Handle, usize) -> i32
          );
          let mode = sym!(
            "FPDFTextObj_GetTextRenderMode",
            unsafe extern "C" fn(Handle) -> i32
          );
          if mode(object) != 0 {
            return Err(
              "Этот текст использует контур или обтравку. Его замена пока не поддерживается."
                .into(),
            );
          }
          let mut matrix: Matrix = std::mem::zeroed();
          let mut size = 0.;
          let (mut r, mut g, mut b, mut a) = (0, 0, 0, 255);
          if get_size(object, &mut size) == 0
            || get_matrix(object, &mut matrix) == 0
            || get_color(object, &mut r, &mut g, &mut b, &mut a) == 0
          {
            return Err("Не удалось прочитать оформление текста.".into());
          }
          let font_path = std::path::PathBuf::from(
            std::env::var_os("WINDIR").ok_or("Не найден каталог Windows.")?,
          )
          .join("Fonts/arial.ttf");
          let bytes = crate::export::read(&font_path, 16 * 1024 * 1024)?;
          let face =
            skrifa::FontRef::new(&bytes).map_err(|_| "Не удалось прочитать шрифт Arial.")?;
          if text.chars().any(|c| face.charmap().map(c).is_none()) {
            return Err("В Arial нет части введённых символов. Замена отменена.".into());
          }
          let font = load_font(self.handle, bytes.as_ptr(), bytes.len() as u32, 2, 1);
          if font.is_null() {
            return Err("Не удалось встроить шрифт Arial.".into());
          }
          let replacement = new_text(self.handle, font, size);
          if replacement.is_null() {
            close_font(font);
            return Err("Не удалось создать текст.".into());
          }
          let utf16: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
          let ok = set_text(replacement, utf16.as_ptr()) != 0
            && set_matrix(replacement, &matrix) != 0
            && set_color(replacement, r, g, b, a) != 0;
          if !ok || remove(page, object) == 0 {
            destroy(replacement);
            close_font(font);
            return Err("Замена текста не удалась.".into());
          }
          destroy(object);
          if insert(page, replacement, object_index) == 0 {
            destroy(replacement);
            close_font(font);
            return Err("Не удалось вставить новый текст.".into());
          }
          close_font(font);
        } else {
          if remove(page, object) == 0 {
            return Err("Не удалось удалить объект.".into());
          }
          destroy(object);
        }
        if generate(page) == 0 {
          return Err("Не удалось обновить содержимое страницы.".into());
        }
        let mut output = Output {
          version: 1,
          write,
          bytes: Vec::new(),
        };
        if save(self.handle, &mut output, 2 | 8) == 0 {
          return Err("Не удалось сохранить PDF (предел копии 96 МБ).".into());
        }
        Ok(output.bytes)
      })();
      (self.api.close_page)(page);
      result
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn object_removal_and_cyrillic_text_survive_reopen() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let pdf = Pdf::open(api.clone(), Arc::new(crate::fixture::demo())).unwrap();
    let objects: Vec<PageObject> =
      serde_json::from_slice(&pdf.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    let text = objects
      .iter()
      .find(|o| o.kind == 1 && !o.text.is_empty())
      .unwrap();
    let bytes = pdf
      .edit(&Operation::Text {
        page: 0,
        object: text.index,
        text: "Проверка 123".into(),
      })
      .unwrap();
    drop(pdf);
    let changed = Pdf::open(api.clone(), Arc::new(bytes)).unwrap();
    let list: Vec<PageObject> =
      serde_json::from_slice(&changed.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    assert!(list
      .iter()
      .any(|o| o.text.replace('\u{a0}', " ") == "Проверка 123"));
    assert_eq!(objects.len(), list.len());
    let bytes = changed
      .edit(&Operation::Delete {
        page: 0,
        object: text.index,
      })
      .unwrap();
    drop(changed);
    let deleted = Pdf::open(api, Arc::new(bytes)).unwrap();
    let list: Vec<PageObject> =
      serde_json::from_slice(&deleted.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    assert_eq!(objects.len() - 1, list.len());
    assert!(!list
      .iter()
      .any(|o| o.text.replace('\u{a0}', " ") == "Проверка 123"));
    deleted.render(0, 300, 400, 0, false).unwrap();
  }
}

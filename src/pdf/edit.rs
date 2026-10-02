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
    if let Operation::Text { page, .. } | Operation::Delete { page, .. } = operation {
      let bytes = crate::editing::separate_streams(&self._bytes, *page)?;
      let prepared = Pdf::open(self.api.clone(), Arc::new(bytes))?;
      return crate::editing::retain_state_fonts(
        &self._bytes,
        prepared.edit_inner(operation)?,
        *page,
      );
    }
    self.edit_inner(operation)
  }
  fn edit_inner(&self, operation: &Operation) -> Result<Vec<u8>, String> {
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
        let mode = sym!(
          "FPDFTextObj_GetTextRenderMode",
          unsafe extern "C" fn(Handle) -> i32
        );
        if kind(object) == 1 && mode(object) >= 4 {
          return Err(
            "Текст задаёт обтравку других объектов. Его правка и удаление пока не поддерживаются."
              .into(),
          );
        }
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
          if !ok {
            destroy(replacement);
            close_font(font);
            return Err("Замена текста не удалась.".into());
          }
          // Вставка наследует поток соседа. Пока исходный объект на месте,
          // замена попадает в его поток и сохраняет порядок и окружение.
          if insert(page, replacement, object_index) == 0 {
            close_font(font);
            return Err("Не удалось вставить новый текст.".into());
          }
          if remove(page, object) == 0 {
            close_font(font);
            return Err("Не удалось заменить исходный текст.".into());
          }
          destroy(object);
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
  fn with_content(content: &[u8]) -> Vec<u8> {
    let mut doc = lopdf::Document::load_mem(&crate::fixture::demo()).unwrap();
    let page = doc.get_pages()[&1];
    let stream = doc.add_object(lopdf::Stream::new(lopdf::dictionary! {}, content.to_vec()));
    doc
      .get_object_mut(page)
      .unwrap()
      .as_dict_mut()
      .unwrap()
      .set("Contents", stream);
    let mut result = Vec::new();
    doc.save_to(&mut result).unwrap();
    result
  }

  #[test]
  fn editing_keeps_fonts_referenced_by_empty_text_state_blocks() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let pdf = Pdf::open(
      api,
      Arc::new(with_content(
        b"BT /F1 12 Tf ET BT /F1 18 Tf 40 700 Td (OLD) Tj ET",
      )),
    )
    .unwrap();
    for operation in [
      Operation::Text {
        page: 0,
        object: 0,
        text: "NEW".into(),
      },
      Operation::Delete { page: 0, object: 0 },
    ] {
      let changed = pdf.edit(&operation).unwrap();
      let doc = lopdf::Document::load_mem(&changed).unwrap();
      assert!(
        doc
          .get_page_fonts(doc.get_pages()[&1])
          .unwrap()
          .contains_key(b"F1".as_slice()),
        "В команде Tf осталась ссылка на удалённый шрифт"
      );
    }
  }

  #[test]
  fn deletion_before_later_transform_keeps_neighbor_rendering() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let suffix = "BT /F1 12 Tf 40 660 Td (KEEP) Tj ET q 1 0 0 1 250 300 cm 0 0 60 40 re f Q";
    let source = with_content(format!("BT /F1 18 Tf 40 700 Td (OLD) Tj ET {suffix}").as_bytes());
    let pdf = Pdf::open(api.clone(), Arc::new(source)).unwrap();
    let changed = pdf.edit(&Operation::Delete { page: 0, object: 0 }).unwrap();
    let after = Pdf::open(api.clone(), Arc::new(changed)).unwrap();
    let expected = Pdf::open(api, Arc::new(with_content(suffix.as_bytes()))).unwrap();
    assert!(
      after.render(0, 600, 800, 0, false).unwrap().pixels
        == expected.render(0, 600, 800, 0, false).unwrap().pixels,
      "Удаление изменило координаты соседей"
    );
  }

  #[test]
  fn replacement_before_later_transform_keeps_text_and_neighbors_in_place() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let source = with_content(b"BT /F1 18 Tf 40 700 Td (OLD) Tj ET BT /F1 12 Tf 40 660 Td (KEEP) Tj ET q 1 0 0 1 250 300 cm 0 0 60 40 re f Q");
    let pdf = Pdf::open(api.clone(), Arc::new(source)).unwrap();
    let changed = pdf
      .edit(&Operation::Text {
        page: 0,
        object: 0,
        text: "NEW".into(),
      })
      .unwrap();
    let after = Pdf::open(api, Arc::new(changed)).unwrap();
    let objects: Vec<PageObject> =
      serde_json::from_slice(&after.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    let original: Vec<PageObject> =
      serde_json::from_slice(&pdf.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    let keep = objects.iter().find(|o| o.text == "KEEP").unwrap();
    assert_eq!(
      keep.bounds, original[1].bounds,
      "Соседний текст сместился после замены"
    );
    let replacement = objects.iter().find(|o| o.text == "NEW").unwrap();
    assert!((replacement.bounds[0] - original[0].bounds[0]).abs() < 0.01);
    assert!((replacement.bounds[1] - original[0].bounds[1]).abs() < 0.01);
    let before_image = pdf.render(0, 600, 800, 0, false).unwrap();
    let after_image = after.render(0, 600, 800, 0, false).unwrap();
    assert_eq!(
      &before_image.pixels[600 * 125 * 4..],
      &after_image.pixels[600 * 125 * 4..],
      "Правка изменила соседние элементы страницы"
    );
  }

  #[test]
  fn replacement_preserves_overlap_order_and_shared_graphics_state() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let bytes = with_content(b"q 1 0 0 rg 30 0 0 30 0 0 cm BT /F1 1 Tf 2 20 Td (OLD) Tj ET 0 0 1 rg 1 19 12 3 re f Q 0 1 0 RG 3 w 150 260 m 430 260 l 430 520 l 150 520 l S");
    let pdf = Pdf::open(api.clone(), Arc::new(bytes)).unwrap();
    let changed = pdf
      .edit(&Operation::Text {
        page: 0,
        object: 0,
        text: "NEW".into(),
      })
      .unwrap();
    let edited = Pdf::open(api.clone(), Arc::new(changed)).unwrap();
    for rotation in 0..4 {
      let before = pdf.render(0, 600, 800, rotation, false).unwrap();
      let after = edited.render(0, 600, 800, rotation, false).unwrap();
      assert!(
        before.pixels == after.pixels,
        "Изменился порядок перекрытия или состояние графики при повороте {rotation}"
      );
    }
  }

  #[test]
  fn deletion_preserves_layers_and_open_paths() {
    use crate::layers::Layers;
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let source = crate::fixture::demo();
    let pdf = Pdf::open(api.clone(), Arc::new(source.clone())).unwrap();
    let objects: Vec<PageObject> =
      serde_json::from_slice(&pdf.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    let target = objects
      .iter()
      .find(|o| o.text.starts_with("ASTRA PDF"))
      .unwrap();
    let result = pdf
      .edit(&Operation::Delete {
        page: 0,
        object: target.index,
      })
      .unwrap();
    for states in [
      [true, true, true],
      [false, true, false],
      [true, false, true],
    ] {
      let before = Pdf::open(
        api.clone(),
        Arc::new(Layers::read(&source).unwrap().with_states(&states).unwrap()),
      )
      .unwrap();
      let after = Pdf::open(
        api.clone(),
        Arc::new(Layers::read(&result).unwrap().with_states(&states).unwrap()),
      )
      .unwrap();
      let a = before.render(0, 600, 800, 0, false).unwrap();
      let b = after.render(0, 600, 800, 0, false).unwrap();
      assert!(
        a.pixels[600 * 100 * 4..] == b.pixels[600 * 100 * 4..],
        "Удаление затронуло соседние объекты или слои"
      );
    }
  }

  #[test]
  fn deleting_layered_path_preserves_following_objects() {
    use crate::layers::Layers;
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let prefix = "q /OC /Electric BDC 0.89 0.36 0.16 RG 4 w ";
    let target = "150 260 m 430 260 l 430 520 l 150 520 l S ";
    let suffix = "100 100 m 500 100 l S EMC /OC /Structure BDC 0 1 0 rg 200 600 50 50 re f EMC Q";
    let source = with_content(format!("{prefix}{target}{suffix}").as_bytes());
    let expected = with_content(format!("{prefix}{suffix}").as_bytes());
    let pdf = Pdf::open(api.clone(), Arc::new(source)).unwrap();
    let changed = pdf.edit(&Operation::Delete { page: 0, object: 0 }).unwrap();
    for states in [
      [true, true, true],
      [false, true, false],
      [true, false, true],
    ] {
      let a = Pdf::open(
        api.clone(),
        Arc::new(
          Layers::read(&changed)
            .unwrap()
            .with_states(&states)
            .unwrap(),
        ),
      )
      .unwrap();
      let b = Pdf::open(
        api.clone(),
        Arc::new(
          Layers::read(&expected)
            .unwrap()
            .with_states(&states)
            .unwrap(),
        ),
      )
      .unwrap();
      assert!(
        a.render(0, 600, 800, 0, false).unwrap().pixels
          == b.render(0, 600, 800, 0, false).unwrap().pixels,
        "Удаление контура повредило соседний объект или слой"
      );
    }
  }

  #[test]
  fn deletion_preserves_inherited_text_font_color_and_spacing() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let source = with_content(b"BT /F1 18 Tf 0.8 0.1 0.2 rg 2 Tc 3 Tw 40 700 Td (REMOVE) Tj ET BT 40 600 Td (KEEP THIS) Tj ET 40 100 50 50 re f");
    let expected = with_content(b"BT /F1 18 Tf 0.8 0.1 0.2 rg 2 Tc 3 Tw 40 700 Td ET BT 40 600 Td (KEEP THIS) Tj ET 40 100 50 50 re f");
    let pdf = Pdf::open(api.clone(), Arc::new(source)).unwrap();
    let changed = pdf.edit(&Operation::Delete { page: 0, object: 0 }).unwrap();
    let a = Pdf::open(api.clone(), Arc::new(changed)).unwrap();
    let b = Pdf::open(api, Arc::new(expected)).unwrap();
    assert!(
      a.render(0, 600, 800, 0, false).unwrap().pixels
        == b.render(0, 600, 800, 0, false).unwrap().pixels
    );
  }

  #[test]
  fn deleting_image_preserves_unrelated_vector_paths() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let suffix = "0 1 0 RG 3 w 150 260 m 430 260 l 430 520 l 150 520 l S";
    let mut doc = lopdf::Document::load_mem(&with_content(
      format!("q 80 0 0 80 20 700 cm /Im0 Do Q {suffix}").as_bytes(),
    ))
    .unwrap();
    let image = doc.add_object(lopdf::Stream::new(lopdf::dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 1, "Height" => 1, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8 }, vec![255, 0, 0]));
    let page = doc.get_pages()[&1];
    doc
      .get_object_mut(page)
      .unwrap()
      .as_dict_mut()
      .unwrap()
      .get_mut(b"Resources")
      .unwrap()
      .as_dict_mut()
      .unwrap()
      .set("XObject", lopdf::dictionary! { "Im0" => image });
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    let pdf = Pdf::open(api.clone(), Arc::new(bytes)).unwrap();
    let objects: Vec<PageObject> =
      serde_json::from_slice(&pdf.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    assert_eq!(objects[0].kind, 3);
    let changed = pdf.edit(&Operation::Delete { page: 0, object: 0 }).unwrap();
    let a = Pdf::open(api.clone(), Arc::new(changed)).unwrap();
    let b = Pdf::open(api, Arc::new(with_content(suffix.as_bytes()))).unwrap();
    assert!(
      a.render(0, 600, 800, 0, false).unwrap().pixels
        == b.render(0, 600, 800, 0, false).unwrap().pixels
    );
  }

  #[test]
  fn deleting_text_used_as_a_clip_is_rejected() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let pdf = Pdf::open(
      api,
      Arc::new(with_content(
        b"BT /F1 18 Tf 7 Tr 40 700 Td (CLIP) Tj ET 0 0 600 800 re f",
      )),
    )
    .unwrap();
    assert!(pdf.edit(&Operation::Delete { page: 0, object: 0 }).is_err());
  }

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
    let before = pdf.render(0, 600, 800, 0, false).unwrap();
    drop(pdf);
    let changed = Pdf::open(api.clone(), Arc::new(bytes)).unwrap();
    let after = changed.render(0, 600, 800, 0, false).unwrap();
    assert_eq!(
      &before.pixels[600 * 100 * 4..],
      &after.pixels[600 * 100 * 4..],
      "Правка текста изменила нетронутую часть страницы"
    );
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

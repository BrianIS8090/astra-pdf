use super::*;
use crate::editing::{Operation, PageObject};

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

pub(super) fn save_document(pdf: &Pdf) -> Result<Vec<u8>, String> {
  let mut output = Output {
    version: 1,
    write,
    bytes: Vec::new(),
  };
  unsafe {
    let save = *pdf
      .api
      ._library
      .get::<unsafe extern "C" fn(Handle, *mut Output, u32) -> i32>(b"FPDF_SaveAsCopy\0")
      .map_err(|e| e.to_string())?;
    if save(pdf.handle, &mut output, 2 | 8) == 0 {
      return Err("Не удалось сохранить PDF (предел копии 96 МБ).".into());
    }
  }
  Ok(output.bytes)
}

impl Pdf {
  pub fn edit(&self, operation: &Operation) -> Result<Vec<u8>, String> {
    if let Operation::Text { page, object, text } = operation {
      return self.replace_text(*page, *object, text);
    }
    if let Operation::Delete { page, .. } = operation {
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
      let char_count = sym!("FPDFText_CountChars", unsafe extern "C" fn(Handle) -> i32);
      let char_object = sym!(
        "FPDFText_GetTextObject",
        unsafe extern "C" fn(Handle, i32) -> Handle
      );
      let generated = sym!(
        "FPDFText_IsGenerated",
        unsafe extern "C" fn(Handle, i32) -> i32
      );
      let unicode = sym!(
        "FPDFText_GetUnicode",
        unsafe extern "C" fn(Handle, i32) -> u32
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
          let chars = char_count(text_page);
          if !(0..=1_000_000).contains(&chars) {
            close_text(text_page);
            return Err("Слишком много символов на странице.".into());
          }
          let mut texts = std::collections::HashMap::<usize, String>::new();
          for i in 0..chars {
            if generated(text_page, i) == 0 {
              if let Some(c) = char::from_u32(unicode(text_page, i)) {
                let value = texts.entry(char_object(text_page, i) as usize).or_default();
                if value.len() < 32_768 {
                  value.push(c);
                }
              }
            }
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
            let value = texts.remove(&(object as usize)).unwrap_or_default();
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
        if remove(page, object) == 0 {
          return Err("Не удалось удалить объект.".into());
        }
        destroy(object);
        if generate(page) == 0 {
          return Err("Не удалось обновить содержимое страницы.".into());
        }
        save_document(self)
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
  fn text_edit_preserves_font_style_spacing_transform_and_neighbors() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    for font in ["Helvetica-Bold", "Times-Italic", "Courier-BoldOblique"] {
      for show in [
        "(OLD WORD) Tj",
        "[(OL) 75 (D WO) -20 (RD)] TJ",
        "(OLD WORD) '",
        "4 2 (OLD WORD) \"",
      ] {
        let prefix = "q 0.8 0.2 0.1 rg 0.1 0.3 0.8 RG 0.5 w 0.98 0.1 -0.1 0.98 15 10 cm BT /F1 18 Tf 2 Tc 4 Tw 85 Tz 5 Ts 2 Tr 22 TL 40 600 Td";
        let suffix = "0 Tc 0 Tw (KEEP) Tj ET Q BT /F1 12 Tf 40 300 Td (UNCHANGED) Tj ET";
        let source = with_content(format!("{prefix} {show} {suffix}").as_bytes());
        let mut doc = lopdf::Document::load_mem(&source).unwrap();
        for obj in doc.objects.values_mut() {
          if let Ok(d) = obj.as_dict_mut() {
            if d.get(b"Type").and_then(lopdf::Object::as_name).ok() == Some(b"Font".as_slice()) {
              d.set("BaseFont", lopdf::Object::Name(font.as_bytes().to_vec()));
            }
          }
        }
        let mut source = Vec::new();
        doc.save_to(&mut source).unwrap();
        let pdf = Pdf::open(api.clone(), Arc::new(source)).unwrap();
        let before: Vec<PageObject> =
          serde_json::from_slice(&pdf.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
        let bytes = pdf
          .edit(&Operation::Text {
            page: 0,
            object: 0,
            text: "OLD WIDER WORD".into(),
          })
          .unwrap();
        let after = Pdf::open(api.clone(), Arc::new(bytes.clone())).unwrap();
        let list: Vec<PageObject> =
          serde_json::from_slice(&after.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
        assert_eq!(list.len(), before.len());
        for (a, b) in list.iter().skip(1).zip(before.iter().skip(1)) {
          assert_eq!(a.text, b.text);
          for (x, y) in a.bounds.iter().zip(b.bounds) {
            assert!((x - y).abs() < 0.000005, "Смещён сосед: {font} / {show}");
          }
        }
        let updated = lopdf::Document::load_mem(&bytes).unwrap();
        let fonts = updated.get_page_fonts(updated.get_pages()[&1]).unwrap();
        assert_eq!(fonts.len(), 1);
        assert_eq!(
          fonts
            .values()
            .next()
            .unwrap()
            .get(b"BaseFont")
            .unwrap()
            .as_name()
            .unwrap(),
          font.as_bytes()
        );
        let old_image = pdf.render(0, 1200, 1600, 0, false).unwrap();
        let new_image = after.render(0, 1200, 1600, 0, false).unwrap();
        assert_eq!(
          &old_image.pixels[1200 * 800 * 4..],
          &new_image.pixels[1200 * 800 * 4..]
        );
      }
    }
  }

  #[test]
  fn editing_text_with_repeated_spaces_keeps_neighbor_position() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let pdf = Pdf::open(api.clone(), Arc::new(crate::fixture::demo())).unwrap();
    let bytes = pdf
      .edit(&Operation::Text {
        page: 0,
        object: 1,
        text: "Edited text 123".into(),
      })
      .unwrap();
    let after = Pdf::open(api, Arc::new(bytes)).unwrap();
    let list: Vec<PageObject> =
      serde_json::from_slice(&after.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    assert_eq!(list[1].text, "Edited text 123");
  }

  #[test]
  fn text_edit_rejects_missing_glyph_without_changing_original() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let source = with_content(b"BT /F1 18 Tf 40 700 Td (ORIGINAL) Tj ET");
    let pdf = Pdf::open(api, Arc::new(source.clone())).unwrap();
    assert!(pdf
      .edit(&Operation::Text {
        page: 0,
        object: 0,
        text: "Кириллица".into()
      })
      .unwrap_err()
      .contains("исходном шрифте"));
    assert_eq!(pdf.source_bytes().as_slice(), source);
    let list: Vec<PageObject> =
      serde_json::from_slice(&pdf.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    assert_eq!(list[0].text, "ORIGINAL");
  }

  #[test]
  fn editing_one_repeated_line_does_not_change_other_occurrences() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let pdf = Pdf::open(
      api.clone(),
      Arc::new(with_content(
        b"BT /F1 18 Tf 40 700 Td (OLD) Tj 0 -40 Td (OLD) Tj 0 -40 Td (OLD) Tj ET",
      )),
    )
    .unwrap();
    let bytes = pdf
      .edit(&Operation::Text {
        page: 0,
        object: 1,
        text: "NEW".into(),
      })
      .unwrap();
    let after = Pdf::open(api, Arc::new(bytes)).unwrap();
    let list: Vec<PageObject> =
      serde_json::from_slice(&after.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    assert_eq!(
      list.iter().map(|o| o.text.as_str()).collect::<Vec<_>>(),
      ["OLD", "NEW", "OLD"]
    );
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
    let fixture = super::super::text::font_fixture("arialbd.ttf", "Проверка исходного текста 123");
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let pdf = Pdf::open(api.clone(), Arc::new(fixture)).unwrap();
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

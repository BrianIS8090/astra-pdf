use super::*;
use crate::editing::{Operation, PageObject};

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Matrix(pub [f32; 6]);
impl Matrix {
  fn point(self, x: f32, y: f32) -> (f32, f32) {
    let [a, b, c, d, e, f] = self.0;
    (a * x + c * y + e, b * x + d * y + f)
  }
  pub(super) fn compose(self, child: Self) -> Self {
    let [a, b, c, d, e, f] = child.0;
    let (x, y) = self.point(e, f);
    let [p, q, r, s, _, _] = self.0;
    Self([
      p * a + r * b,
      q * a + s * b,
      p * c + r * d,
      q * c + s * d,
      x,
      y,
    ])
  }
  pub(super) fn inverse(self) -> Result<Self, String> {
    let [a, b, c, d, e, f] = self.0;
    let determinant = a * d - b * c;
    if !determinant.is_finite() || determinant.abs() < 1e-12 {
      return Err("Вырожденное преобразование PDF.".into());
    }
    Ok(Self([
      d / determinant,
      -b / determinant,
      -c / determinant,
      a / determinant,
      (c * f - d * e) / determinant,
      (b * e - a * f) / determinant,
    ]))
  }
}

impl Pdf {
  pub(super) unsafe fn object_at(&self, page: Handle, path: &[usize]) -> Result<Handle, String> {
    let get = *self
      .api
      ._library
      .get::<unsafe extern "C" fn(Handle, i32) -> Handle>(b"FPDFPage_GetObject\0")
      .map_err(|e| e.to_string())?;
    let child = *self
      .api
      ._library
      .get::<unsafe extern "C" fn(Handle, u32) -> Handle>(b"FPDFFormObj_GetObject\0")
      .map_err(|e| e.to_string())?;
    let mut object = get(page, *path.first().ok_or("Объект отсутствует.")? as i32);
    for index in path.iter().skip(1) {
      object = child(object, *index as u32);
    }
    if object.is_null() {
      Err("Объект отсутствует.".into())
    } else {
      Ok(object)
    }
  }
  pub(super) unsafe fn object_tree(
    &self,
    page: Handle,
  ) -> Result<Vec<(Handle, Vec<usize>, Matrix)>, String> {
    let count = *self
      .api
      ._library
      .get::<unsafe extern "C" fn(Handle) -> i32>(b"FPDFPage_CountObjects\0")
      .map_err(|e| e.to_string())?;
    let children = *self
      .api
      ._library
      .get::<unsafe extern "C" fn(Handle) -> i32>(b"FPDFFormObj_CountObjects\0")
      .map_err(|e| e.to_string())?;
    let kind = *self
      .api
      ._library
      .get::<unsafe extern "C" fn(Handle) -> i32>(b"FPDFPageObj_GetType\0")
      .map_err(|e| e.to_string())?;
    let matrix = *self
      .api
      ._library
      .get::<unsafe extern "C" fn(Handle, *mut Matrix) -> i32>(b"FPDFPageObj_GetMatrix\0")
      .map_err(|e| e.to_string())?;
    let n = count(page);
    if !(0..=100_000).contains(&n) {
      return Err("Слишком много объектов.".into());
    }
    let identity = Matrix([1., 0., 0., 1., 0., 0.]);
    let mut out = Vec::new();
    for i in 0..n {
      out.push((
        self.object_at(page, &[i as usize])?,
        vec![i as usize],
        identity,
      ));
    }
    let mut at = 0;
    while at < out.len() {
      let (object, path, parent) = out[at].clone();
      if kind(object) == 5 {
        let n = children(object);
        if n < 0 || out.len() + n as usize > 100_000 || path.len() > 32 {
          return Err("Слишком сложная группа PDF.".into());
        }
        let mut local = identity;
        if matrix(object, &mut local) == 0 {
          return Err("Не удалось прочитать координаты группы.".into());
        }
        for i in 0..n {
          let mut path = path.clone();
          path.push(i as usize);
          out.push((self.object_at(page, &path)?, path, parent.compose(local)));
        }
      }
      at += 1;
    }
    Ok(out)
  }
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
    match operation {
      Operation::FontText {
        page,
        object,
        text,
        font,
      } => return self.replace_text_font(*page, *object, text, Some(font)),
      Operation::NoteDelete { page, index } => return self.edit_note(*page, *index, None),
      Operation::NoteText { page, index, text } => {
        return self.edit_note(*page, *index, Some(text))
      }
      Operation::Annotate { note } => return self.annotate(note),
      Operation::PageOrder { order } => return crate::pages::reorder(&self._bytes, order),
      Operation::InsertPages { source, range, at } => {
        return crate::pages::insert(
          &self._bytes,
          &crate::export::read(source, crate::vault::LIMIT)?,
          range,
          *at,
        )
      }
      Operation::ReadPage { page } => {
        return serde_json::to_vec(&self.read_page(*page)?).map_err(|e| e.to_string())
      }
      Operation::Find { query, masks } => {
        return serde_json::to_vec(&self.search(query, masks)?).map_err(|e| e.to_string())
      }
      Operation::Bookmarks => {
        return serde_json::to_vec(&self.bookmarks()?).map_err(|e| e.to_string())
      }
      _ => (),
    }
    if let Operation::Text { page, object, text } = operation {
      return self.replace_text(*page, *object, text);
    }
    if let Operation::Transform {
      page,
      object,
      bounds,
    } = operation
    {
      let objects: Vec<PageObject> =
        serde_json::from_slice(&self.edit(&Operation::Objects { page: *page })?)
          .map_err(|e| e.to_string())?;
      if let Some(item) = objects.iter().find(|o| o.index == *object && o.kind == 1) {
        return self.transform_text(*page, item, bounds);
      }
    }
    if let Operation::Delete { page, object } = operation {
      let objects: Vec<PageObject> =
        serde_json::from_slice(&self.edit(&Operation::Objects { page: *page })?)
          .map_err(|e| e.to_string())?;
      if objects
        .iter()
        .find(|o| o.index == *object)
        .is_some_and(|o| o.path.len() > 1 && o.kind == 1)
      {
        return self.replace_text(*page, *object, "");
      }
    }
    if let Operation::Delete { page, .. } | Operation::Transform { page, .. } = operation {
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
      | Operation::Transform { page, .. }
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
          for (i, (object, path, parent)) in self.object_tree(page)?.into_iter().enumerate() {
            let (mut l, mut b, mut r, mut t) = (0., 0., 0., 0.);
            if bounds(object, &mut l, &mut b, &mut r, &mut t) == 0 {
              continue;
            }
            let corners = [
              parent.point(l, b),
              parent.point(l, t),
              parent.point(r, b),
              parent.point(r, t),
            ];
            l = corners.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
            r = corners
              .iter()
              .map(|p| p.0)
              .fold(f32::NEG_INFINITY, f32::max);
            b = corners.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
            t = corners
              .iter()
              .map(|p| p.1)
              .fold(f32::NEG_INFINITY, f32::max);
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
            let mut font = String::new();
            let mut font_size = 0.;
            if object_kind == 1 {
              let handle = sym!(
                "FPDFTextObj_GetFont",
                unsafe extern "C" fn(Handle) -> Handle
              )(object);
              let mut name = [0u8; 256];
              let length = sym!(
                "FPDFFont_GetBaseFontName",
                unsafe extern "C" fn(Handle, *mut u8, usize) -> usize
              )(handle, name.as_mut_ptr(), name.len());
              if length > 0 && length <= name.len() {
                font = String::from_utf8_lossy(&name[..length - 1]).into_owned();
              }
              sym!(
                "FPDFTextObj_GetFontSize",
                unsafe extern "C" fn(Handle, *mut f32) -> i32
              )(object, &mut font_size);
            }
            objects.push(PageObject {
              index: i,
              kind: object_kind,
              bounds: [
                x1.min(x2) as f64 / 1_000_000.,
                y1.min(y2) as f64 / 1_000_000.,
                x1.max(x2) as f64 / 1_000_000.,
                y1.max(y2) as f64 / 1_000_000.,
              ],
              text: value,
              path,
              font,
              font_size,
            });
          }
          close_text(text_page);
          return serde_json::to_vec(&objects).map_err(|e| e.to_string());
        }
        let object_index = match operation {
          Operation::Delete { object, .. }
          | Operation::Text { object, .. }
          | Operation::Transform { object, .. } => *object,
          _ => unreachable!(),
        };
        if object_index >= n as usize {
          return Err("Отдельный нетекстовый элемент внутри группы пока не редактируется. Выберите всю группу повторным щелчком.".into());
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
        if let Operation::Transform { bounds: target, .. } = operation {
          if target
            .iter()
            .any(|v| !v.is_finite() || !(-5. ..=6.).contains(v))
            || target[2] - target[0] < 0.000001
            || target[3] - target[1] < 0.000001
          {
            return Err("Недопустимый размер или положение объекта.".into());
          }
          let (mut l, mut b, mut r, mut t) = (0., 0., 0., 0.);
          if bounds(object, &mut l, &mut b, &mut r, &mut t) == 0 || r - l < 0.001 || t - b < 0.001 {
            return Err("Невозможно изменить размер вырожденного объекта.".into());
          }
          let to_page = sym!(
            "FPDF_DeviceToPage",
            unsafe extern "C" fn(
              Handle,
              i32,
              i32,
              i32,
              i32,
              i32,
              i32,
              i32,
              *mut f64,
              *mut f64,
            ) -> i32
          );
          let (mut x1, mut y1, mut x2, mut y2) = (0., 0., 0., 0.);
          if to_page(
            page,
            0,
            0,
            1_000_000,
            1_000_000,
            0,
            (target[0] * 1_000_000.).round() as i32,
            (target[1] * 1_000_000.).round() as i32,
            &mut x1,
            &mut y1,
          ) == 0
            || to_page(
              page,
              0,
              0,
              1_000_000,
              1_000_000,
              0,
              (target[2] * 1_000_000.).round() as i32,
              (target[3] * 1_000_000.).round() as i32,
              &mut x2,
              &mut y2,
            ) == 0
          {
            return Err("Не удалось преобразовать координаты объекта.".into());
          }
          let sx = (x2 - x1).abs() / (r - l) as f64;
          let sy = (y2 - y1).abs() / (t - b) as f64;
          if !(0.02..=50.).contains(&sx) || !(0.02..=50.).contains(&sy) {
            return Err("Изменяйте размер в пределах от 2% до 5000% за один шаг.".into());
          }
          let dx = x1.min(x2) - l as f64 * sx;
          let dy = y1.min(y2) - b as f64 * sy;
          sym!(
            "FPDFPageObj_Transform",
            unsafe extern "C" fn(Handle, f64, f64, f64, f64, f64, f64)
          )(object, sx, 0., 0., sy, dx, dy);
          sym!(
            "FPDFPageObj_TransformClipPath",
            unsafe extern "C" fn(Handle, f64, f64, f64, f64, f64, f64)
          )(object, sx, 0., 0., sy, dx, dy);
        } else {
          if remove(page, object) == 0 {
            return Err("Не удалось удалить объект.".into());
          }
          destroy(object);
        }
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
  #[test]
  fn nested_repeated_group_edits_only_selected_instance() {
    use lopdf::{dictionary, Stream};
    let mut doc = lopdf::Document::load_mem(&with_content(
      b"q 1 0 0 1 30 400 cm /Group Do Q q 1 0 0 1 30 200 cm /Group Do Q",
    ))
    .unwrap();
    let page = doc.get_pages()[&1];
    let mut resources = super::super::groups::resources(&doc, page).unwrap();
    let form=doc.add_object(Stream::new(dictionary!{"Type"=>"XObject","Subtype"=>"Form","BBox"=>vec![0.into(),0.into(),300.into(),100.into()],"Resources"=>resources.clone(),"Group"=>dictionary!{"S"=>"Transparency","I"=>true}},b"BT /F1 18 Tf 10 30 Td (OLD WORD) Tj ET".to_vec()));
    resources.set("XObject", dictionary! {"Group"=>form});
    doc
      .get_object_mut(page)
      .unwrap()
      .as_dict_mut()
      .unwrap()
      .set("Resources", resources);
    let mut source = Vec::new();
    doc.save_to(&mut source).unwrap();
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let pdf = Pdf::open(api.clone(), Arc::new(source)).unwrap();
    let items: Vec<PageObject> =
      serde_json::from_slice(&pdf.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    let children: Vec<_> = items.iter().filter(|o| o.kind == 1).collect();
    assert_eq!(children.len(), 2);
    assert!(children[0].bounds[1] < children[1].bounds[1]);
    assert_eq!(children[0].path, vec![0, 0]);
    let bytes = pdf
      .edit(&Operation::Text {
        page: 0,
        object: children[0].index,
        text: "NEW WORD\nMORE".into(),
      })
      .unwrap();
    let pdf = Pdf::open(api.clone(), Arc::new(bytes.clone())).unwrap();
    let result: Vec<PageObject> =
      serde_json::from_slice(&pdf.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    let text: Vec<_> = result
      .iter()
      .filter(|o| o.kind == 1)
      .map(|o| o.text.as_str())
      .collect();
    assert_eq!(text, vec!["NEW WORD", "MORE", "OLD WORD"]);
    let doc = lopdf::Document::load_mem(&bytes).unwrap();
    assert!(doc
      .objects
      .values()
      .filter_map(|o| o.as_stream().ok())
      .any(|s| s.dict.has(b"Group")));
    assert_eq!(result.last().unwrap().bounds, children[1].bounds);
    let removed = pdf
      .edit(&Operation::Delete {
        page: 0,
        object: result.iter().find(|o| o.text == "MORE").unwrap().index,
      })
      .unwrap();
    let check = Pdf::open(api, Arc::new(removed)).unwrap();
    let items: Vec<PageObject> =
      serde_json::from_slice(&check.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    assert_eq!(
      items
        .iter()
        .filter(|o| o.kind == 1)
        .map(|o| o.text.as_str())
        .collect::<Vec<_>>(),
      vec!["NEW WORD", "OLD WORD"]
    );
  }
  #[test]
  fn explicit_font_choice_embeds_cyrillic_and_preserves_neighbors() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let pdf = Pdf::open(
      api.clone(),
      Arc::new(with_content(
        b"BT /F1 18 Tf 2 Tc 3 Tw 50 650 Td (OLD WORD) Tj (KEEP) Tj ET",
      )),
    )
    .unwrap();
    let before: Vec<PageObject> =
      serde_json::from_slice(&pdf.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    let font =
      std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap()).join("Fonts/arial.ttf");
    let bytes = pdf
      .edit(&Operation::FontText {
        page: 0,
        object: 0,
        text: "Новый текст\nВторая строка".into(),
        font,
      })
      .unwrap();
    let edited = Pdf::open(api.clone(), Arc::new(bytes)).unwrap();
    let after: Vec<PageObject> =
      serde_json::from_slice(&edited.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    assert_eq!(after[0].text, "Новый текст");
    assert_eq!(after[1].text, "Вторая строка");
    assert_eq!(after[2].text, "KEEP");
    assert!(after[0].font.contains("Arial"));
    for (a, b) in after[2].bounds.iter().zip(before[1].bounds) {
      assert!((a - b).abs() < 0.00001)
    }
    let b = after[0].bounds;
    let target = [b[0] + 0.03, b[1] + 0.02, b[2] + 0.03, b[3] + 0.02];
    let moved = edited
      .edit(&Operation::Transform {
        page: 0,
        object: 0,
        bounds: target,
      })
      .unwrap();
    let check = Pdf::open(api, Arc::new(moved)).unwrap();
    let items: Vec<PageObject> =
      serde_json::from_slice(&check.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    for (a, b) in items[0].bounds.iter().zip(target) {
      assert!(
        (a - b).abs() < 0.00001,
        "actual {:?}, target {:?}",
        items[0].bounds,
        target
      )
    }
    for (a, b) in items.iter().skip(1).zip(after.iter().skip(1)) {
      assert_eq!(a.text, b.text);
      for (x, y) in a.bounds.iter().zip(b.bounds) {
        assert!((x - y).abs() < 0.00001)
      }
    }
  }
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

  #[test]
  fn multiline_text_in_layer_preserves_neighbors_and_layer_visibility() {
    let mut d = lopdf::Document::load_mem(&crate::fixture::demo()).unwrap();
    let page = d.get_pages()[&1];
    let contents = d.get_page_content(page);
    let mut changed=b"BT /F1 10 Tf 30 80 Td (Neighbor before) Tj ET\n/OC /Electric BDC BT /F1 18 Tf 50 400 Td (First) Tj 0 -100 Td (Neighbor after) Tj ET EMC\n".to_vec();
    changed.extend_from_slice(&contents);
    let stream = d.add_object(lopdf::Stream::new(lopdf::dictionary! {}, changed));
    d.get_object_mut(page)
      .unwrap()
      .as_dict_mut()
      .unwrap()
      .set("Contents", stream);
    let mut bytes = Vec::new();
    d.save_to(&mut bytes).unwrap();
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let pdf = Pdf::open(api.clone(), Arc::new(bytes)).unwrap();
    let original: Vec<PageObject> =
      serde_json::from_slice(&pdf.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    let selected = original.iter().find(|o| o.text == "First").unwrap();
    let edited = pdf
      .edit(&Operation::Text {
        page: 0,
        object: selected.index,
        text: "First line\nSecond line\nThird".into(),
      })
      .unwrap();
    let pdf = Pdf::open(api.clone(), Arc::new(edited.clone())).unwrap();
    let items: Vec<PageObject> =
      serde_json::from_slice(&pdf.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
    assert!(items.iter().any(|o| o.text == "Second line"));
    for old in original.iter().filter(|o| o.text.starts_with("Neighbor")) {
      let new = items.iter().find(|o| o.text == old.text).unwrap();
      assert_eq!(old.bounds, new.bounds);
    }
    let layers = crate::layers::Layers::read(&edited).unwrap();
    assert_eq!(layers.items.len(), 3);
    let hidden = Pdf::open(
      api,
      Arc::new(layers.with_states(&[true, false, true]).unwrap()),
    )
    .unwrap();
    assert_ne!(
      hidden.render(0, 300, 400, 0, false).unwrap().pixels,
      pdf.render(0, 300, 400, 0, false).unwrap().pixels
    );
  }

  #[test]
  fn object_move_resize_preserves_neighbors_at_intrinsic_rotations() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    for rotation in [0, 90, 180, 270] {
      let mut d = lopdf::Document::load_mem(&with_content(
        b"0 0 1 rg 40 80 100 90 re f BT /F1 12 Tf 300 600 Td (Neighbor) Tj ET",
      ))
      .unwrap();
      let id = d.get_pages()[&1];
      d.get_object_mut(id)
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .set("Rotate", rotation);
      let mut bytes = Vec::new();
      d.save_to(&mut bytes).unwrap();
      let pdf = Pdf::open(api.clone(), Arc::new(bytes)).unwrap();
      let items: Vec<PageObject> =
        serde_json::from_slice(&pdf.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
      let rect = &items[0];
      let b = rect.bounds;
      let target = [
        b[0] + 0.05,
        b[1] + 0.03,
        b[0] + 0.05 + (b[2] - b[0]) * 1.2,
        b[1] + 0.03 + (b[3] - b[1]) * 0.8,
      ];
      let bytes = pdf
        .edit(&Operation::Transform {
          page: 0,
          object: rect.index,
          bounds: target,
        })
        .unwrap();
      let after = Pdf::open(api.clone(), Arc::new(bytes)).unwrap();
      let changed: Vec<PageObject> =
        serde_json::from_slice(&after.edit(&Operation::Objects { page: 0 }).unwrap()).unwrap();
      assert_eq!(changed[1].bounds, items[1].bounds);
      for (x, y) in changed[0].bounds.iter().zip(target) {
        assert!((x - y).abs() < 0.00001, "{rotation}: {x} != {y}");
      }
    }
  }
}

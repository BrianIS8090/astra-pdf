use super::*;
use crate::editing::Operation;
use lopdf::{
  content::{Content, Operation as Op},
  Object, Stream, StringFormat,
};
use std::collections::BTreeMap;
type Codes = BTreeMap<char, (Vec<u8>, f32)>;

const UNSUPPORTED: &str =
  "Эту строку пока нельзя изменить с сохранением оформления. Документ не изменён.";

fn error(e: impl std::fmt::Display) -> String {
  e.to_string()
}
fn extracted_text(value: &str) -> String {
  // При извлечении PDFium сворачивает повторные пробелы, хотя в PDF они остаются.
  value
    .split([' ', '\u{a0}'])
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" ")
}
fn string(bytes: Vec<u8>) -> Object {
  Object::String(bytes, StringFormat::Hexadecimal)
}

fn write_content(
  doc: &mut lopdf::Document,
  page: lopdf::ObjectId,
  ops: Vec<Op>,
) -> Result<Vec<u8>, String> {
  let bytes = Content { operations: ops }.encode().map_err(error)?;
  let mut stream = Stream::new(lopdf::dictionary! {}, bytes);
  stream.compress().map_err(error)?;
  let stream = doc.add_object(stream);
  doc
    .get_object_mut(page)
    .and_then(Object::as_dict_mut)
    .map_err(error)?
    .set("Contents", stream);
  let mut output = Vec::new();
  doc.save_to(&mut output).map_err(error)?;
  if output.len() > 96_000_000 {
    return Err("Документ превышает предел редактирования 96 МБ.".into());
  }
  Ok(output)
}

#[derive(Clone, Copy, Default)]
struct Spacing {
  character: f32,
  word: f32,
}

fn spacing(ops: &[Op], index: usize) -> Result<Spacing, String> {
  let mut state = Spacing::default();
  let mut stack = Vec::new();
  for op in &ops[..=index] {
    let number = |i: usize| {
      op.operands
        .get(i)
        .ok_or(UNSUPPORTED)?
        .as_float()
        .map_err(error)
    };
    match op.operator.as_str() {
      "q" => {
        if stack.len() > 1024 {
          return Err(UNSUPPORTED.into());
        }
        stack.push(state);
      }
      "Q" => state = stack.pop().ok_or(UNSUPPORTED)?,
      "Tc" => state.character = number(0)?,
      "Tw" => state.word = number(0)?,
      "\"" => {
        state.word = number(0)?;
        state.character = number(1)?;
      }
      _ => {}
    }
  }
  if !state.character.is_finite() || !state.word.is_finite() {
    return Err(UNSUPPORTED.into());
  }
  Ok(state)
}

fn text_parts(op: &Op) -> Result<Vec<Object>, String> {
  Ok(match op.operator.as_str() {
    "Tj" | "'" => vec![op.operands.first().ok_or(UNSUPPORTED)?.clone()],
    "\"" => vec![op.operands.get(2).ok_or(UNSUPPORTED)?.clone()],
    "TJ" => op
      .operands
      .first()
      .ok_or(UNSUPPORTED)?
      .as_array()
      .map_err(error)?
      .clone(),
    _ => return Err(UNSUPPORTED.into()),
  })
}

// Сохраняем интервалы неизменённых пар букв, а вставленный текст получает
// исходные Tc/Tw. Поправка в конце оставляет следующие объекты на своих местах.
fn replacement(
  op: &Op,
  new: &str,
  codes: &Codes,
  size: f32,
  spaces: Spacing,
) -> Result<Vec<Op>, String> {
  let mut old = Vec::new();
  let new: Vec<char> = new.chars().collect();
  let mut kern = vec![0f32];
  for part in text_parts(op)? {
    match part {
      Object::String(bytes, _) => {
        let mut offset = 0;
        while offset < bytes.len() {
          let mut matching = codes
            .iter()
            .filter(|(_, (code, _))| !code.is_empty() && bytes[offset..].starts_with(code));
          let (c, (code, _)) = matching.next().ok_or(UNSUPPORTED)?;
          if matching.any(|(_, (other, _))| other != code) || old.len() >= 32_768 {
            return Err(UNSUPPORTED.into());
          }
          old.push(*c);
          kern.push(0.);
          offset += code.len();
        }
      }
      Object::Integer(n) => *kern.last_mut().unwrap() += n as f32,
      Object::Real(n) => *kern.last_mut().unwrap() += n,
      _ => return Err(UNSUPPORTED.into()),
    }
  }
  if old.is_empty() {
    return Err(UNSUPPORTED.into());
  }
  let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
  let suffix = old[prefix..]
    .iter()
    .rev()
    .zip(new[prefix..].iter().rev())
    .take_while(|(a, b)| a == b)
    .count();
  let mut gaps = vec![0.; new.len() + 1];
  let mut pair_gaps = BTreeMap::new();
  for (i, pair) in old.windows(2).enumerate() {
    pair_gaps
      .entry((pair[0], pair[1]))
      .and_modify(|value: &mut Option<f32>| {
        if *value != Some(kern[i + 1]) {
          *value = None;
        }
      })
      .or_insert(Some(kern[i + 1]));
  }
  gaps[0] = kern[0];
  for i in 1..new.len() {
    if i < prefix {
      gaps[i] = kern[i];
    } else if i > new.len() - suffix {
      gaps[i] = kern[old.len() - (new.len() - i)];
    } else if let Some(Some(value)) = pair_gaps.get(&(new[i - 1], new[i])) {
      gaps[i] = *value;
    }
  }
  let advance = |chars: &[char]| -> f32 {
    chars
      .iter()
      .map(|c| {
        let (code, width) = &codes[c];
        width * size
          + spaces.character
          + if code.as_slice() == [32] {
            spaces.word
          } else {
            0.
          }
      })
      .sum()
  };
  gaps[new.len()] = (advance(&new) - advance(&old)) * 1000. / size + kern.iter().sum::<f32>()
    - gaps[..new.len()].iter().sum::<f32>();
  if gaps.iter().any(|v| !v.is_finite() || v.abs() > 1e9) {
    return Err(UNSUPPORTED.into());
  }
  let mut array = Vec::new();
  if gaps[0] != 0. {
    array.push(Object::Real(gaps[0]));
  }
  for (i, c) in new.iter().enumerate() {
    array.push(string(codes[c].0.clone()));
    if gaps[i + 1] != 0. {
      array.push(Object::Real(gaps[i + 1]));
    }
  }
  let mut result = Vec::new();
  if op.operator == "\"" {
    result.push(Op::new("Tw", vec![op.operands[0].clone()]));
    result.push(Op::new("Tc", vec![op.operands[1].clone()]));
  }
  if op.operator == "'" || op.operator == "\"" {
    result.push(Op::new("T*", vec![]));
  }
  result.push(Op::new("TJ", vec![Object::Array(array)]));
  Ok(result)
}

impl Pdf {
  pub(super) fn replace_text(
    &self,
    index: usize,
    selected: usize,
    new: &str,
  ) -> Result<Vec<u8>, String> {
    if new.is_empty()
      || new.len() > 16_384
      || new.chars().any(|c| c.is_control() || c as u32 > 0xffff)
    {
      return Err(
        "Введите одну непустую строку. Некоторые сложные символы пока не поддерживаются.".into(),
      );
    }
    let originals: Vec<crate::editing::PageObject> =
      serde_json::from_slice(&self.edit(&Operation::Objects { page: index })?).map_err(error)?;
    let original = originals
      .iter()
      .find(|v| v.index == selected && v.kind == 1)
      .ok_or("Выберите текстовый объект.")?;
    if new == original.text {
      return Ok(self._bytes.as_ref().clone());
    }
    let mut doc = lopdf::Document::load_mem(&self._bytes).map_err(error)?;
    let page_id = *doc
      .get_pages()
      .get(&(index as u32 + 1))
      .ok_or("Страница отсутствует.")?;
    let ops = Content::decode(&doc.get_page_content(page_id))
      .map_err(error)?
      .operations;
    // Метки существуют только во временном документе и точно связывают
    // выбранный PDFium объект с исходной командой, даже при повторяющемся тексте.
    let mut prefix = "AstraTextEdit".to_string();
    while ops.iter().any(|o| {
      o.operands
        .iter()
        .any(|v| matches!(v, Object::Name(n) if n.starts_with(prefix.as_bytes())))
    }) {
      prefix.push('X');
    }
    let mut marked = Vec::new();
    for (i, op) in ops.iter().enumerate() {
      if matches!(op.operator.as_str(), "Tj" | "TJ" | "'" | "\"") {
        marked.push(Op::new(
          "BMC",
          vec![Object::Name(format!("{prefix}{i}").into_bytes())],
        ));
        marked.push(op.clone());
        marked.push(Op::new("EMC", vec![]));
      } else {
        marked.push(op.clone());
      }
    }
    let mut tagged_doc = doc.clone();
    let tagged = Pdf::open(
      self.api.clone(),
      Arc::new(write_content(&mut tagged_doc, page_id, marked)?),
    )?;
    let tagged_objects: Vec<crate::editing::PageObject> =
      serde_json::from_slice(&tagged.edit(&Operation::Objects { page: index })?).map_err(error)?;
    if tagged_objects.len() != originals.len()
      || tagged_objects
        .iter()
        .zip(&originals)
        .any(|(a, b)| a.kind != b.kind || a.text != b.text || a.bounds != b.bounds)
    {
      return Err(UNSUPPORTED.into());
    }
    let (position, codes, size) =
      unsafe { tagged.encode_text(index, selected, &prefix, &original.text, new)? };
    let mut changed = ops.clone();
    let updates = replacement(&ops[position], new, &codes, size, spacing(&ops, position)?)?;
    changed.splice(position..=position, updates);
    // Возвращаем исходные ресурсы, графические команды и шрифты без пересоздания.
    let bytes = write_content(&mut doc, page_id, changed)?;
    let check = Pdf::open(self.api.clone(), Arc::new(bytes))?;
    let objects: Vec<crate::editing::PageObject> =
      serde_json::from_slice(&check.edit(&Operation::Objects { page: index })?).map_err(error)?;
    let edited = objects
      .iter()
      .find(|o| o.index == selected)
      .ok_or(UNSUPPORTED)?;
    if objects.len() != originals.len()
      || extracted_text(&edited.text) != extracted_text(new)
      || objects.iter().zip(&originals).any(|(a, b)| {
        a.index != selected
          && (a.index != b.index
            || a.kind != b.kind
            || a.text != b.text
            || a
              .bounds
              .iter()
              .zip(b.bounds)
              .any(|(x, y)| (x - y).abs() > 0.00001))
      })
    {
      return Err("Проверка правки обнаружила потерю символов или смещение соседнего объекта. Документ не изменён.".into());
    }
    Ok(check._bytes.as_ref().clone())
  }

  unsafe fn encode_text(
    &self,
    index: usize,
    selected: usize,
    prefix: &str,
    old: &str,
    new: &str,
  ) -> Result<(usize, Codes, f32), String> {
    macro_rules! sym {
      ($name:literal,$t:ty) => {
        *self
          .api
          ._library
          .get::<$t>(concat!($name, "\0").as_bytes())
          .map_err(error)?
      };
    }
    let get = sym!(
      "FPDFPage_GetObject",
      unsafe extern "C" fn(Handle, i32) -> Handle
    );
    let marks = sym!(
      "FPDFPageObj_CountMarks",
      unsafe extern "C" fn(Handle) -> i32
    );
    let mark = sym!(
      "FPDFPageObj_GetMark",
      unsafe extern "C" fn(Handle, u32) -> Handle
    );
    let name = sym!(
      "FPDFPageObjMark_GetName",
      unsafe extern "C" fn(Handle, *mut c_void, u32, *mut u32) -> i32
    );
    let get_font = sym!(
      "FPDFTextObj_GetFont",
      unsafe extern "C" fn(Handle) -> Handle
    );
    let get_size = sym!(
      "FPDFTextObj_GetFontSize",
      unsafe extern "C" fn(Handle, *mut f32) -> i32
    );
    let mode = sym!(
      "FPDFTextObj_GetTextRenderMode",
      unsafe extern "C" fn(Handle) -> i32
    );
    let create_page = sym!(
      "FPDFPage_New",
      unsafe extern "C" fn(Handle, i32, f64, f64) -> Handle
    );
    let create_text = sym!(
      "FPDFPageObj_CreateTextObj",
      unsafe extern "C" fn(Handle, Handle, f32) -> Handle
    );
    let set_text = sym!(
      "FPDFText_SetText",
      unsafe extern "C" fn(Handle, *const u16) -> i32
    );
    let insert = sym!(
      "FPDFPage_InsertObject",
      unsafe extern "C" fn(Handle, Handle)
    );
    let transform = sym!(
      "FPDFPageObj_Transform",
      unsafe extern "C" fn(Handle, f64, f64, f64, f64, f64, f64)
    );
    let destroy = sym!("FPDFPageObj_Destroy", Close);
    let generate = sym!(
      "FPDFPage_GenerateContent",
      unsafe extern "C" fn(Handle) -> i32
    );
    let load_text = sym!("FPDFText_LoadPage", unsafe extern "C" fn(Handle) -> Handle);
    let close_text = sym!("FPDFText_ClosePage", Close);
    let read_text = sym!(
      "FPDFTextObj_GetText",
      unsafe extern "C" fn(Handle, Handle, *mut u16, u32) -> u32
    );
    let width = sym!(
      "FPDFFont_GetGlyphWidth",
      unsafe extern "C" fn(Handle, u32, f32, *mut f32) -> i32
    );
    let glyph = sym!(
      "FPDFFont_GetGlyphPath",
      unsafe extern "C" fn(Handle, u32, f32) -> Handle
    );
    let page = (self.api.page)(self.handle, index as i32);
    if page.is_null() {
      return Err(UNSUPPORTED.into());
    }
    let result = (|| {
      let object = get(page, selected as i32);
      if marks(object) != 1 || mode(object) >= 3 {
        return Err(
          "Текст в размеченной группе, невидимый или обтравочный текст пока не редактируется."
            .into(),
        );
      }
      let mut buffer = [0u16; 256];
      let mut length = 0;
      if name(
        mark(object, 0),
        buffer.as_mut_ptr().cast(),
        512,
        &mut length,
      ) == 0
        || !(2..=512).contains(&length)
      {
        return Err(UNSUPPORTED.into());
      }
      let tag = String::from_utf16_lossy(&buffer[..length as usize / 2 - 1]);
      let position = tag
        .strip_prefix(prefix)
        .ok_or(UNSUPPORTED)?
        .parse::<usize>()
        .map_err(error)?;
      let font = get_font(object);
      let mut size = 0.;
      if font.is_null() || get_size(object, &mut size) == 0 || !size.is_finite() || size <= 0. {
        return Err(UNSUPPORTED.into());
      }
      let mut chars: Vec<char> = old.chars().chain(new.chars()).collect();
      chars.sort_unstable();
      chars.dedup();
      let anchor = old
        .chars()
        .find(|c| !c.is_whitespace())
        .ok_or(UNSUPPORTED)?;
      if chars.len() > 4096 {
        return Err(UNSUPPORTED.into());
      }
      let scratch = create_page(
        self.handle,
        self.sizes.len() as i32,
        100.,
        chars.len() as f64 * 3. + 10.,
      );
      if scratch.is_null() {
        return Err(UNSUPPORTED.into());
      }
      let encoded = (|| {
        let mut handles = Vec::new();
        let mut widths = Vec::new();
        for c in &chars {
          let item = create_text(self.handle, font, 1.);
          if item.is_null() {
            return Err(UNSUPPORTED.into());
          }
          let encoded_char = if *c == ' ' && old.contains('\u{a0}') {
            '\u{a0}'
          } else {
            *c
          };
          let utf16: Vec<u16> = format!("{anchor}{encoded_char}{anchor}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
          let mut w = 0.;
          if set_text(item, utf16.as_ptr()) == 0
            || width(font, encoded_char as u32, 1., &mut w) == 0
            || !w.is_finite()
            || (!c.is_whitespace() && glyph(font, *c as u32, 1.).is_null())
          {
            destroy(item);
            return Err(format!("В исходном шрифте нет символа «{c}». Текст не изменён; другой шрифт автоматически не подставляется."));
          }
          transform(item, 1., 0., 0., 1., 5., handles.len() as f64 * 3. + 5.);
          insert(scratch, item);
          handles.push(item);
          widths.push(w);
        }
        let text_page = load_text(scratch);
        if text_page.is_null() {
          return Err(UNSUPPORTED.into());
        }
        let mut missing = None;
        for ((c, item), glyph_width) in chars.iter().zip(&handles).zip(&widths) {
          let mut buf = [0u16; 8];
          let len = read_text(*item, text_page, buf.as_mut_ptr(), 16);
          let decoded = if (2..=16).contains(&len) {
            String::from_utf16_lossy(&buf[..len as usize / 2 - 1]).replace('\u{a0}', " ")
          } else {
            String::new()
          };
          // PDFium может пропускать пробелы CID-шрифта при извлечении текста.
          let omitted_space = matches!(c, ' ' | '\u{a0}')
            && *glyph_width > 0.
            && (old.contains(*c) || old.contains('\u{a0}'))
            && decoded == format!("{anchor}{anchor}");
          if decoded != format!("{anchor}{c}{anchor}").replace('\u{a0}', " ") && !omitted_space {
            missing = Some(*c);
            break;
          }
        }
        close_text(text_page);
        if let Some(c) = missing {
          return Err(format!("В исходном шрифте нет кодировки символа «{c}». Текст не изменён; другой шрифт автоматически не подставляется."));
        }
        if generate(scratch) == 0 {
          return Err(UNSUPPORTED.into());
        }
        let bytes = super::edit::save_document(self)?;
        let doc = lopdf::Document::load_mem(&bytes).map_err(error)?;
        let id = doc.get_pages()[&(self.sizes.len() as u32 + 1)];
        for font in doc.get_page_fonts(id).map_err(error)?.values() {
          if font
            .get(b"Subtype")
            .and_then(Object::as_name)
            .map_err(error)?
            == b"Type0"
          {
            let encoding = font
              .get(b"Encoding")
              .and_then(Object::as_name)
              .map_err(|_| UNSUPPORTED)?;
            if !encoding.ends_with(b"-H") {
              return Err(
                "Вертикальный текст и нестандартные карты символов пока не редактируются.".into(),
              );
            }
          }
        }
        let content = Content::decode(&doc.get_page_content(id)).map_err(error)?;
        let mut strings = Vec::new();
        for op in content
          .operations
          .iter()
          .filter(|o| matches!(o.operator.as_str(), "Tj" | "TJ"))
        {
          let mut code = Vec::new();
          for part in text_parts(op)? {
            match part {
              Object::String(v, _) => code.extend(v),
              _ => return Err(UNSUPPORTED.into()),
            }
          }
          strings.push(code);
        }
        if strings.len() != chars.len() {
          return Err(UNSUPPORTED.into());
        }
        let anchor_bytes = &strings[chars.binary_search(&anchor).map_err(|_| UNSUPPORTED)?];
        if anchor_bytes.is_empty() || anchor_bytes.len() % 3 != 0 {
          return Err(UNSUPPORTED.into());
        }
        let unit = anchor_bytes[..anchor_bytes.len() / 3].to_vec();
        for value in &mut strings {
          if value.len() <= 2 * unit.len() || !value.starts_with(&unit) || !value.ends_with(&unit) {
            return Err(UNSUPPORTED.into());
          }
          *value = value[unit.len()..value.len() - unit.len()].to_vec();
        }
        Ok(
          chars
            .into_iter()
            .zip(strings.into_iter().zip(widths))
            .collect(),
        )
      })();
      (self.api.close_page)(scratch);
      Ok((position, encoded?, size))
    })();
    (self.api.close_page)(page);
    result
  }
}

#[cfg(test)]
pub(super) fn font_fixture(name: &str, text: &str) -> Vec<u8> {
  let api = Api::new(&crate::pdfium_path()).unwrap();
  let pdf = Pdf::open(api, Arc::new(crate::fixture::demo())).unwrap();
  unsafe {
    macro_rules! sym {
      ($n:literal,$t:ty) => {
        *pdf
          .api
          ._library
          .get::<$t>(concat!($n, "\0").as_bytes())
          .unwrap()
      };
    }
    let load = sym!(
      "FPDFText_LoadFont",
      unsafe extern "C" fn(Handle, *const u8, u32, i32, i32) -> Handle
    );
    let close = sym!("FPDFFont_Close", Close);
    let page_new = sym!(
      "FPDFPage_New",
      unsafe extern "C" fn(Handle, i32, f64, f64) -> Handle
    );
    let new_text = sym!(
      "FPDFPageObj_CreateTextObj",
      unsafe extern "C" fn(Handle, Handle, f32) -> Handle
    );
    let set = sym!(
      "FPDFText_SetText",
      unsafe extern "C" fn(Handle, *const u16) -> i32
    );
    let transform = sym!(
      "FPDFPageObj_Transform",
      unsafe extern "C" fn(Handle, f64, f64, f64, f64, f64, f64)
    );
    let insert = sym!(
      "FPDFPage_InsertObject",
      unsafe extern "C" fn(Handle, Handle)
    );
    let generate = sym!(
      "FPDFPage_GenerateContent",
      unsafe extern "C" fn(Handle) -> i32
    );
    let data = std::fs::read(
      std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap())
        .join("Fonts")
        .join(name),
    )
    .unwrap();
    let font = load(pdf.handle, data.as_ptr(), data.len() as u32, 2, 1);
    assert!(!font.is_null());
    let page = page_new(pdf.handle, 3, 600., 800.);
    let item = new_text(pdf.handle, font, 18.);
    let units: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    assert_ne!(set(item, units.as_ptr()), 0);
    transform(item, 1., 0., 0., 1., 40., 730.);
    insert(page, item);
    assert_ne!(generate(page), 0);
    let result = super::edit::save_document(&pdf).unwrap();
    (pdf.api.close_page)(page);
    close(font);
    crate::editing::extract(&result, &[3]).unwrap()
  }
}

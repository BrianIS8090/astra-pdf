use super::*;
use crate::editing::Operation;
use lopdf::{
  content::{Content, Operation as Op},
  Object, StringFormat,
};
use std::collections::BTreeMap;
pub(super) type Codes = BTreeMap<char, (Vec<u8>, f32)>;

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

#[derive(Clone, Copy, Default)]
struct Spacing {
  character: f32,
  word: f32,
  rise: f32,
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
      "Ts" => state.rise = number(0)?,
      "\"" => {
        state.word = number(0)?;
        state.character = number(1)?;
      }
      _ => {}
    }
  }
  if !state.character.is_finite() || !state.word.is_finite() || !state.rise.is_finite() {
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

fn multiline_replacement(
  op: &Op,
  new: &str,
  codes: &Codes,
  size: f32,
  spaces: Spacing,
) -> Result<Vec<Op>, String> {
  if !new.contains('\n') {
    return replacement(op, new, codes, size, spaces);
  }
  let lines: Vec<_> = new.split('\n').collect();
  if lines.len() > 128 {
    return Err("Не больше 128 строк в одном текстовом блоке.".into());
  }
  let parts = text_parts(op)?;
  let mut old_advance = 0.;
  for part in &parts {
    match part {
      Object::String(bytes, _) => {
        let mut offset = 0;
        while offset < bytes.len() {
          let (_, (code, width)) = codes
            .iter()
            .find(|(_, (code, _))| !code.is_empty() && bytes[offset..].starts_with(code))
            .ok_or(UNSUPPORTED)?;
          old_advance += width * size
            + spaces.character
            + if code.as_slice() == [32] {
              spaces.word
            } else {
              0.
            };
          offset += code.len();
        }
      }
      Object::Real(v) => old_advance -= v * size / 1000.,
      Object::Integer(v) => old_advance -= *v as f32 * size / 1000.,
      _ => return Err(UNSUPPORTED.into()),
    }
  }
  let normalized = Op::new("TJ", vec![Object::Array(parts)]);
  let mut result = vec![Op::new("q", vec![])];
  result.extend(replacement(op, lines[0], codes, size, spaces)?);
  for (line, text) in lines.iter().enumerate().skip(1) {
    result.push(Op::new(
      "TJ",
      vec![Object::Array(vec![Object::Real(
        old_advance * 1000. / size,
      )])],
    ));
    result.push(Op::new(
      "Ts",
      vec![Object::Real(spaces.rise - line as f32 * size * 1.2)],
    ));
    result.extend(replacement(&normalized, text, codes, size, spaces)?);
  }
  result.push(Op::new("Q", vec![]));
  Ok(result)
}

impl Pdf {
  pub(super) fn replace_text(
    &self,
    index: usize,
    selected: usize,
    new: &str,
  ) -> Result<Vec<u8>, String> {
    self.replace_text_font(index, selected, new, None)
  }
  pub(super) fn replace_text_font(
    &self,
    index: usize,
    selected: usize,
    new: &str,
    font: Option<&std::path::Path>,
  ) -> Result<Vec<u8>, String> {
    self.change_text(index, selected, new, font, None)
  }
  pub(super) fn transform_text(
    &self,
    page: usize,
    object: &crate::editing::PageObject,
    bounds: &[f64; 4],
  ) -> Result<Vec<u8>, String> {
    self.change_text(page, object.index, &object.text, None, Some(*bounds))
  }
  fn change_text(
    &self,
    index: usize,
    selected: usize,
    new: &str,
    font: Option<&std::path::Path>,
    target: Option<[f64; 4]>,
  ) -> Result<Vec<u8>, String> {
    let new = &new.replace("\r\n", "\n");
    if new.len() > 16_384
      || new
        .chars()
        .any(|c| (c.is_control() && c != '\n') || c as u32 > 0xffff)
    {
      return Err(
        "Введите непустой текст. Некоторые сложные символы пока не поддерживаются.".into(),
      );
    }
    let originals: Vec<crate::editing::PageObject> =
      serde_json::from_slice(&self.edit(&Operation::Objects { page: index })?).map_err(error)?;
    let original = originals
      .iter()
      .find(|v| v.index == selected && v.kind == 1)
      .ok_or("Выберите текстовый объект.")?;
    if new == &original.text && font.is_none() && target.is_none() {
      return Ok(self._bytes.as_ref().clone());
    }
    let mut doc = lopdf::Document::load_mem(&self._bytes).map_err(error)?;
    let page_id = *doc
      .get_pages()
      .get(&(index as u32 + 1))
      .ok_or("Страница отсутствует.")?;
    let streams = super::groups::detach(&mut doc, page_id)?;
    let contents: Vec<_> = streams
      .iter()
      .map(|(id, page)| super::groups::content(&doc, *id, *page))
      .collect::<Result<_, _>>()?;
    // Метки существуют только во временном документе и точно связывают
    // выбранный PDFium объект с исходной командой, даже при повторяющемся тексте.
    let mut prefix = "AstraTextEdit".to_string();
    while contents.iter().flatten().any(|o| {
      o.operands
        .iter()
        .any(|v| matches!(v, Object::Name(n) if n.starts_with(prefix.as_bytes())))
    }) {
      prefix.push('X');
    }
    let mut tagged_doc = doc.clone();
    let mut positions = Vec::new();
    for ((id, page), ops) in streams.iter().zip(&contents) {
      let mut marked = Vec::new();
      for (i, op) in ops.iter().enumerate() {
        if matches!(op.operator.as_str(), "Tj" | "TJ" | "'" | "\"") {
          marked.push(Op::new(
            "BMC",
            vec![Object::Name(
              format!("{prefix}{}", positions.len()).into_bytes(),
            )],
          ));
          positions.push((*id, *page, i));
          marked.push(op.clone());
          marked.push(Op::new("EMC", vec![]));
        } else {
          marked.push(op.clone());
        }
      }
      super::groups::write(&mut tagged_doc, *id, *page, marked)?;
    }
    let mut tagged_bytes = Vec::new();
    tagged_doc.save_to(&mut tagged_bytes).map_err(error)?;
    let tagged = Pdf::open(self.api.clone(), Arc::new(tagged_bytes))?;
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
    let (position, codes, size) = unsafe {
      tagged.encode_text(
        index,
        &original.path,
        &prefix,
        &original.text,
        if font.is_some() { &original.text } else { new },
      )?
    };
    let (id, is_page, position) = *positions.get(position).ok_or(UNSUPPORTED)?;
    let ops = super::groups::content(&doc, id, is_page)?;
    let mut changed = ops.clone();
    let spaces = spacing(&ops, position)?;
    let updates = if let Some(target) = target {
      let delta = unsafe { self.text_delta(index, original, target, &ops[..position])? };
      vec![
        Op::new("q", vec![]),
        Op::new("cm", delta.0.into_iter().map(Object::Real).collect()),
        ops[position].clone(),
        Op::new("Q", vec![]),
      ]
    } else if let Some(font) = font {
      let (name, new_codes) = super::font::embed(&mut doc, id, is_page, font, new)?;
      let mut advance = replacement(&ops[position], "", &codes, size, spaces)?;
      let last = advance.pop().ok_or(UNSUPPORTED)?;
      let mut result = advance;
      result.push(Op::new("q", vec![]));
      result.push(Op::new("Tf", vec![Object::Name(name), Object::Real(size)]));
      result.push(Op::new("Tw", vec![0.into()]));
      let lines: Vec<_> = new.split('\n').collect();
      if lines.len() > 128 {
        return Err("Не больше 128 строк.".into());
      }
      for (line, value) in lines.iter().enumerate() {
        result.push(Op::new(
          "Ts",
          vec![Object::Real(spaces.rise - line as f32 * size * 1.2)],
        ));
        let mut array = Vec::new();
        let mut width = 0.;
        for c in value.chars() {
          let (code, w) = &new_codes[&c];
          array.push(string(code.clone()));
          width += w * size + spaces.character;
          if c == ' ' || c == '\u{a0}' {
            array.push(Object::Real(-spaces.word * 1000. / size));
            width += spaces.word;
          }
        }
        array.push(Object::Real(width * 1000. / size));
        result.push(Op::new("TJ", vec![Object::Array(array)]));
      }
      result.push(Op::new("Q", vec![]));
      result.push(last);
      result
    } else {
      multiline_replacement(&ops[position], new, &codes, size, spaces)?
    };
    changed.splice(position..=position, updates);
    // Возвращаем исходные ресурсы, графические команды и шрифты без пересоздания.
    super::groups::write(&mut doc, id, is_page, changed)?;
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(error)?;
    if bytes.len() > 96_000_000 {
      return Err("Документ превышает предел редактирования 96 МБ.".into());
    }
    let check = Pdf::open(self.api.clone(), Arc::new(bytes))?;
    let objects: Vec<crate::editing::PageObject> =
      serde_json::from_slice(&check.edit(&Operation::Objects { page: index })?).map_err(error)?;
    let lines: Vec<_> = new.split('\n').filter(|s| !s.is_empty()).collect();
    let offset = originals
      .iter()
      .position(|o| o.index == selected)
      .ok_or(UNSUPPORTED)?;
    let count = lines.len();
    let edited = objects.get(offset..offset + count).ok_or(UNSUPPORTED)?;
    if objects.len() != originals.len() + count - 1
      || edited
        .iter()
        .zip(&lines)
        .any(|(o, line)| extracted_text(&o.text) != extracted_text(line))
      || originals
        .iter()
        .enumerate()
        .filter(|(_, o)| o.index != selected)
        .any(|(i, b)| {
          let a = &objects[if i < offset { i } else { i + count - 1 }];
          a.kind != b.kind
            || a.text != b.text
            || (b.kind != 5
              && a
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

  unsafe fn text_delta(
    &self,
    index: usize,
    object: &crate::editing::PageObject,
    target: [f64; 4],
    ops: &[Op],
  ) -> Result<super::edit::Matrix, String> {
    use super::edit::Matrix;
    if target
      .iter()
      .any(|v| !v.is_finite() || !(-5. ..=6.).contains(v))
      || target[2] <= target[0]
      || target[3] <= target[1]
    {
      return Err("Недопустимый размер объекта.".into());
    }
    let page = (self.api.page)(self.handle, index as i32);
    if page.is_null() {
      return Err(UNSUPPORTED.into());
    }
    let result = (|| {
      let (_, _, parent) = self
        .object_tree(page)?
        .into_iter()
        .find(|(_, p, _)| *p == object.path)
        .ok_or(UNSUPPORTED)?;
      let device=*self.api._library.get::<unsafe extern "C" fn(Handle,i32,i32,i32,i32,i32,i32,i32,*mut f64,*mut f64)->i32>(b"FPDF_DeviceToPage\0").map_err(error)?;
      let convert = |bounds: [f64; 4]| -> Result<[f64; 4], String> {
        let (mut x1, mut y1, mut x2, mut y2) = (0., 0., 0., 0.);
        if device(
          page,
          0,
          0,
          1_000_000,
          1_000_000,
          0,
          (bounds[0] * 1_000_000.).round() as i32,
          (bounds[1] * 1_000_000.).round() as i32,
          &mut x1,
          &mut y1,
        ) == 0
          || device(
            page,
            0,
            0,
            1_000_000,
            1_000_000,
            0,
            (bounds[2] * 1_000_000.).round() as i32,
            (bounds[3] * 1_000_000.).round() as i32,
            &mut x2,
            &mut y2,
          ) == 0
        {
          return Err(UNSUPPORTED.into());
        }
        Ok([x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2)])
      };
      let from = convert(object.bounds)?;
      let to = convert(target)?;
      let sx = (to[2] - to[0]) / (from[2] - from[0]);
      let sy = (to[3] - to[1]) / (from[3] - from[1]);
      if !(0.02..=50.).contains(&sx) || !(0.02..=50.).contains(&sy) {
        return Err("Изменяйте размер в пределах от 2% до 5000% за один шаг.".into());
      }
      let change = Matrix([
        sx as f32,
        0.,
        0.,
        sy as f32,
        (to[0] - from[0] * sx) as f32,
        (to[1] - from[1] * sy) as f32,
      ]);
      let mut matrix = parent;
      let mut stack = Vec::new();
      for op in ops {
        match op.operator.as_str() {
          "q" => {
            if stack.len() > 1024 {
              return Err(UNSUPPORTED.into());
            }
            stack.push(matrix)
          }
          "Q" => matrix = stack.pop().ok_or(UNSUPPORTED)?,
          "cm" => {
            let values: Vec<_> = op
              .operands
              .iter()
              .map(|o| o.as_float().map_err(error))
              .collect::<Result<_, _>>()?;
            let values: [f32; 6] = values.try_into().map_err(|_| UNSUPPORTED)?;
            matrix = matrix.compose(Matrix(values));
          }
          _ => (),
        }
      }
      Ok(matrix.inverse()?.compose(change).compose(matrix))
    })();
    (self.api.close_page)(page);
    result
  }

  unsafe fn encode_text(
    &self,
    index: usize,
    selected: &[usize],
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
      let object = self.object_at(page, selected)?;
      if mode(object) >= 3 {
        return Err("Невидимый или обтравочный текст пока не редактируется.".into());
      }
      let mut positions = Vec::new();
      if !(1..=128).contains(&marks(object)) {
        return Err(UNSUPPORTED.into());
      }
      for i in 0..marks(object) {
        let mut buffer = [0u16; 256];
        let mut length = 0;
        if name(
          mark(object, i as u32),
          buffer.as_mut_ptr().cast(),
          512,
          &mut length,
        ) == 0
          || !(2..=512).contains(&length)
        {
          continue;
        }
        let tag = String::from_utf16_lossy(&buffer[..length as usize / 2 - 1]);
        if let Some(value) = tag.strip_prefix(prefix) {
          positions.push(value.parse::<usize>().map_err(error)?);
        }
      }
      if positions.len() != 1 {
        return Err(UNSUPPORTED.into());
      }
      let position = positions[0];
      let font = get_font(object);
      let mut size = 0.;
      if font.is_null() || get_size(object, &mut size) == 0 || !size.is_finite() || size <= 0. {
        return Err(UNSUPPORTED.into());
      }
      let mut chars: Vec<char> = old
        .chars()
        .chain(new.chars())
        .filter(|c| *c != '\n')
        .collect();
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

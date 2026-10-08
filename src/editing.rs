use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Serialize, Deserialize)]
pub enum Operation {
  FontText {
    page: usize,
    object: usize,
    text: String,
    font: std::path::PathBuf,
  },
  NoteDelete {
    page: usize,
    index: usize,
  },
  NoteText {
    page: usize,
    index: usize,
    text: String,
  },
  Annotate {
    note: crate::annotation::Note,
  },
  Transform {
    page: usize,
    object: usize,
    bounds: [f64; 4],
  },
  PageOrder {
    order: Vec<usize>,
  },
  InsertPages {
    source: std::path::PathBuf,
    range: String,
    at: usize,
  },
  ReadPage {
    page: usize,
  },
  Find {
    query: String,
    masks: Vec<Mask>,
  },
  Bookmarks,
  Objects {
    page: usize,
  },
  Delete {
    page: usize,
    object: usize,
  },
  Text {
    page: usize,
    object: usize,
    text: String,
  },
  Extract {
    pages: Vec<usize>,
  },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PageObject {
  pub index: usize,
  pub kind: i32,
  pub bounds: [f64; 4],
  pub text: String,
  #[serde(default)]
  pub path: Vec<usize>,
  #[serde(default)]
  pub font: String,
  #[serde(default)]
  pub font_size: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mask {
  pub page: usize,
  pub bounds: [f64; 4],
  #[serde(default)]
  pub kind: MaskKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaskKind {
  #[default]
  Cover,
  Pixelate {
    block_mm: u8,
  },
}

impl Mask {
  pub fn valid(&self, pages: usize) -> bool {
    self.page < pages
      && match self.kind {
        MaskKind::Cover => true,
        MaskKind::Pixelate { block_mm } => [3, 6, 12].contains(&block_mm),
      }
      && self
        .bounds
        .iter()
        .all(|v| v.is_finite() && (0. ..=1.).contains(v))
      && self.bounds[0] < self.bounds[2]
      && self.bounds[1] < self.bounds[3]
  }
}

pub fn pages(input: &str, count: usize) -> Result<Vec<usize>, String> {
  if input.len() > 65536 || count == 0 || count > 100_000 {
    return Err("Недопустимый список страниц.".into());
  }
  let mut result = BTreeSet::new();
  let number = |s: &str| -> Result<usize, String> {
    let n = s
      .trim()
      .parse::<usize>()
      .map_err(|_| "Введите номера, например: 1, 3-5.")?;
    if n == 0 || n > count {
      return Err(format!("Номера страниц должны быть от 1 до {count}."));
    }
    Ok(n - 1)
  };
  for part in input.split([',', ';']) {
    let pair: Vec<_> = part.trim().split(['-', '–', '—']).collect();
    let first = number(pair[0])?;
    let last = match pair.len() {
      1 => first,
      2 => number(pair[1])?,
      _ => return Err("Неверный диапазон страниц.".into()),
    };
    if last < first {
      return Err("Начало диапазона должно быть не больше конца.".into());
    }
    result.extend(first..=last);
  }
  Ok(result.into_iter().collect())
}

pub fn extract(bytes: &[u8], pages: &[usize]) -> Result<Vec<u8>, String> {
  let mut doc = lopdf::Document::load_mem(bytes).map_err(|e| e.to_string())?;
  let count = doc.get_pages().len();
  if pages.is_empty()
    || pages.len() > count
    || pages.iter().any(|&p| p >= count)
    || pages.windows(2).any(|p| p[0] >= p[1])
  {
    return Err("Недопустимый список страниц.".into());
  }
  let remove: Vec<_> = (0..count)
    .filter(|p| pages.binary_search(p).is_err())
    .map(|p| p as u32 + 1)
    .collect();
  doc.delete_pages(&remove);
  // Оглавление может удерживать ссылки на удалённые страницы; экспорт не переносит его.
  let root = doc
    .trailer
    .get(b"Root")
    .and_then(lopdf::Object::as_reference)
    .map_err(|e| e.to_string())?;
  let catalog = doc
    .get_object_mut(root)
    .and_then(lopdf::Object::as_dict_mut)
    .map_err(|e| e.to_string())?;
  for key in [
    b"Outlines".as_slice(),
    b"PageLabels",
    b"OpenAction",
    b"AA",
    b"Names",
    b"AcroForm",
    b"StructTreeRoot",
  ] {
    catalog.remove(key);
  }
  doc.prune_objects();
  let mut output = Vec::new();
  doc.save_to(&mut output).map_err(|e| e.to_string())?;
  Ok(output)
}

pub fn separate_streams(bytes: &[u8], page: usize) -> Result<Vec<u8>, String> {
  use lopdf::{
    content::{Content, Operation as Op},
    dictionary, Object, Stream,
  };
  fn flush(
    doc: &mut lopdf::Document,
    streams: &mut Vec<Object>,
    chunk: &mut Vec<Op>,
  ) -> Result<(), String> {
    if chunk.is_empty() {
      return Ok(());
    }
    if streams.len() >= 100_000 {
      return Err("Слишком много объектов для редактирования.".into());
    }
    // PDFium хранит преобразования только у потоков с командой cm. Без явной
    // записи он может взять матрицу следующего потока и сместить правку страницы.
    // Единичная матрица не меняет рисунок и фиксирует состояние каждой границы.
    chunk.push(Op::new(
      "cm",
      vec![1.into(), 0.into(), 0.into(), 1.into(), 0.into(), 0.into()],
    ));
    let bytes = Content {
      operations: std::mem::take(chunk),
    }
    .encode()
    .map_err(|e| e.to_string())?;
    streams.push(Object::Reference(
      doc.add_object(Stream::new(dictionary! {}, bytes)),
    ));
    Ok(())
  }
  let mut doc = lopdf::Document::load_mem(bytes).map_err(|e| e.to_string())?;
  let id = *doc
    .get_pages()
    .get(&(page as u32 + 1))
    .ok_or("Страница отсутствует.")?;
  let content = doc.get_page_content(id);
  let parsed =
    Content::decode(&content).map_err(|_| "Этот поток PDF пока нельзя безопасно редактировать.")?;
  let mut chunk = Vec::new();
  let mut streams = Vec::new();
  let mut in_text = false;
  let mut in_path = false;
  let mut text_state = Vec::new();
  let mut text_stack = Vec::new();
  for operation in parsed.operations {
    let name = operation.operator.as_str();
    if in_text {
      match name {
        "ET" => {
          if !text_stack.is_empty() {
            return Err("Незавершённое состояние внутри текста PDF.".into());
          }
          chunk.push(operation);
          flush(&mut doc, &mut streams, &mut chunk)?;
          // Удаление текста не должно менять унаследованный шрифт и цвет соседей.
          if !text_state.is_empty() {
            chunk.push(Op::new("BT", vec![]));
            chunk.append(&mut text_state);
            chunk.push(Op::new("ET", vec![]));
          }
          in_text = false;
          continue;
        }
        "Tf" | "Tc" | "Tw" | "Tz" | "TL" | "Tr" | "Ts" | "rg" | "RG" | "g" | "G" | "k" | "K"
        | "cs" | "CS" | "sc" | "SC" | "scn" | "SCN" | "gs" => text_state.push(operation.clone()),
        "TD" if operation.operands.len() == 2 => {
          let leading = operation.operands[1]
            .as_float()
            .map_err(|e| e.to_string())?;
          text_state.push(Op::new("TL", vec![Object::Real(-leading)]));
        }
        "\"" if operation.operands.len() == 3 => {
          text_state.push(Op::new("Tw", vec![operation.operands[0].clone()]));
          text_state.push(Op::new("Tc", vec![operation.operands[1].clone()]));
        }
        "Tj" | "TJ" | "'" | "Td" | "Tm" | "T*" => {}
        "BMC" | "BDC" | "EMC" => {}
        "q" => {
          if text_stack.len() > 1024 {
            return Err("Слишком глубокое состояние текста.".into());
          }
          text_stack.push(text_state.clone());
        }
        "Q" => {
          text_state = text_stack
            .pop()
            .ok_or("Несогласованное состояние текста PDF.")?;
        }
        _ => {
          return Err(
            "Сложная структура текстового блока пока не поддерживает безопасное редактирование."
              .into(),
          )
        }
      }
      chunk.push(operation);
      continue;
    }
    if in_path {
      let end = matches!(
        name,
        "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n"
      );
      if !end && !matches!(name, "m" | "l" | "c" | "v" | "y" | "h" | "re" | "W" | "W*") {
        return Err(
          "Сложная структура контура пока не поддерживает безопасное редактирование.".into(),
        );
      }
      chunk.push(operation);
      if end {
        if chunk.last().unwrap().operator != "n"
          && chunk
            .iter()
            .any(|o| matches!(o.operator.as_str(), "W" | "W*"))
        {
          return Err("Контур одновременно рисует и обрезает страницу. Его редактирование пока не поддерживается.".into());
        }
        flush(&mut doc, &mut streams, &mut chunk)?;
        in_path = false;
      }
      continue;
    }
    if matches!(name, "BT" | "m" | "re" | "Do" | "sh") {
      flush(&mut doc, &mut streams, &mut chunk)?;
      in_text = name == "BT";
      in_path = matches!(name, "m" | "re");
      let standalone = matches!(name, "Do" | "sh");
      chunk.push(operation);
      if standalone {
        flush(&mut doc, &mut streams, &mut chunk)?;
      }
    } else {
      chunk.push(operation);
    }
  }
  if in_text || in_path {
    return Err("Незавершённый объект PDF: редактирование отменено.".into());
  }
  flush(&mut doc, &mut streams, &mut chunk)?;
  // Потоки страницы выполняются последовательно с общим графическим состоянием.
  // PDFium перегенерирует только поток изменённого объекта, сохраняя остальные команды.
  doc
    .get_object_mut(id)
    .and_then(Object::as_dict_mut)
    .map_err(|e| e.to_string())?
    .set("Contents", Object::Array(streams));
  doc.prune_objects();
  let mut result = Vec::new();
  doc.save_to(&mut result).map_err(|e| e.to_string())?;
  Ok(result)
}

pub fn retain_state_fonts(
  original: &[u8],
  edited: Vec<u8>,
  page: usize,
) -> Result<Vec<u8>, String> {
  use lopdf::{Dictionary, Document, Object, ObjectId};
  use std::collections::BTreeMap;

  fn import(
    value: &Object,
    source: &Document,
    target: &mut Document,
    ids: &mut BTreeMap<ObjectId, ObjectId>,
    depth: usize,
  ) -> Result<Object, String> {
    if depth > 64 || ids.len() > 100_000 {
      return Err("Слишком сложная структура шрифта PDF.".into());
    }
    Ok(match value {
      Object::Reference(id) => {
        if let Some(id) = ids.get(id) {
          return Ok(Object::Reference(*id));
        }
        let new_id = target.add_object(Object::Null);
        ids.insert(*id, new_id);
        let object = source.get_object(*id).map_err(|e| e.to_string())?;
        let copy = import(object, source, target, ids, depth + 1)?;
        target.objects.insert(new_id, copy);
        Object::Reference(new_id)
      }
      Object::Array(values) => Object::Array(
        values
          .iter()
          .map(|v| import(v, source, target, ids, depth + 1))
          .collect::<Result<_, _>>()?,
      ),
      Object::Dictionary(dict) => {
        let mut copy = Dictionary::new();
        for (key, value) in dict.iter() {
          copy.set(key.clone(), import(value, source, target, ids, depth + 1)?);
        }
        Object::Dictionary(copy)
      }
      Object::Stream(stream) => {
        let mut copy = stream.clone();
        copy.dict = import(
          &Object::Dictionary(stream.dict.clone()),
          source,
          target,
          ids,
          depth + 1,
        )?
        .as_dict()
        .map_err(|e| e.to_string())?
        .clone();
        Object::Stream(copy)
      }
      value => value.clone(),
    })
  }

  let source = Document::load_mem(original).map_err(|e| e.to_string())?;
  let mut target = Document::load_mem(&edited).map_err(|e| e.to_string())?;
  let page_id = target.get_pages()[&(page as u32 + 1)];
  let source_id = source.get_pages()[&(page as u32 + 1)];
  let fonts = source
    .get_page_fonts(source_id)
    .map_err(|e| e.to_string())?;
  let existing = target.get_page_fonts(page_id).map_err(|e| e.to_string())?;
  let missing: Vec<_> = fonts
    .into_iter()
    .filter(|(name, _)| !existing.contains_key(name))
    .collect();
  if missing.is_empty() {
    return Ok(edited);
  }
  // Генератор PDFium удаляет шрифты без рисуемых символов, хотя команды Tf
  // могут остаться в неизменённых потоках. Возвращаем только их ресурсы.
  let (direct, inherited) = target
    .get_page_resources(page_id)
    .map_err(|e| e.to_string())?;
  let mut resources = direct
    .cloned()
    .or_else(|| {
      inherited
        .first()
        .and_then(|id| target.get_dictionary(*id).ok())
        .cloned()
    })
    .unwrap_or_default();
  let mut font_resources = resources
    .get(b"Font")
    .ok()
    .and_then(|o| target.dereference(o).ok())
    .and_then(|(_, o)| o.as_dict().ok())
    .cloned()
    .unwrap_or_default();
  let mut ids = BTreeMap::new();
  for (name, font) in missing {
    font_resources.set(
      name,
      import(
        &Object::Dictionary(font.clone()),
        &source,
        &mut target,
        &mut ids,
        0,
      )?,
    );
  }
  resources.set("Font", font_resources);
  target
    .get_object_mut(page_id)
    .and_then(Object::as_dict_mut)
    .map_err(|e| e.to_string())?
    .set("Resources", resources);
  let mut result = Vec::new();
  target.save_to(&mut result).map_err(|e| e.to_string())?;
  if result.len() > 96_000_000 {
    return Err("Изменённая копия превышает 96 МБ.".into());
  }
  Ok(result)
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn ranges_are_validated_and_deduplicated() {
    assert_eq!(pages("3, 1; 2-3", 3).unwrap(), vec![0, 1, 2]);
    assert_eq!(pages("1, 2–3", 3).unwrap(), vec![0, 1, 2]);
    for s in [
      "",
      "0",
      "4",
      "2-1",
      "1-2-3",
      "1,",
      "-1",
      "1.5",
      "999999999999999999999999999999",
    ] {
      assert!(pages(s, 3).is_err(), "{s}");
    }
  }
  #[test]
  fn extraction_keeps_requested_pages_and_layers() {
    let source = crate::fixture::demo();
    let result = extract(&source, &[0, 2]).unwrap();
    let doc = lopdf::Document::load_mem(&result).unwrap();
    assert_eq!(doc.get_pages().len(), 2);
    assert_eq!(crate::layers::Layers::read(&result).unwrap().items.len(), 3);
    assert!(extract(&source, &[3]).is_err());
    assert!(extract(&source, &[]).is_err());
  }
}

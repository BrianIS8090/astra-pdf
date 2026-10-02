use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Serialize, Deserialize)]
pub enum Operation {
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mask {
  pub page: usize,
  pub bounds: [f64; 4],
}

impl Mask {
  pub fn valid(&self, pages: usize) -> bool {
    self.page < pages
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
    let pair: Vec<_> = part.trim().split('-').collect();
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

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn ranges_are_validated_and_deduplicated() {
    assert_eq!(pages("3, 1; 2-3", 3).unwrap(), vec![0, 1, 2]);
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

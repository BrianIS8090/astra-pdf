use lopdf::{dictionary, Dictionary, Document, Object, ObjectId};
use std::collections::{BTreeSet, HashSet};

fn load(bytes: &[u8]) -> Result<Document, String> {
  let doc = Document::load_mem(bytes).map_err(|e| e.to_string())?;
  if doc.get_pages().is_empty() {
    return Err("Документ не содержит страниц.".into());
  }
  Ok(doc)
}

fn direct<'a>(doc: &'a Document, value: &'a Object) -> Result<&'a Object, String> {
  doc
    .dereference(value)
    .map(|(_, o)| o)
    .map_err(|e| e.to_string())
}

fn page_dictionary(doc: &Document, id: ObjectId) -> Result<Dictionary, String> {
  let mut page = doc.get_dictionary(id).map_err(|e| e.to_string())?.clone();
  let mut current = id;
  let mut visited = HashSet::new();
  for _ in 0..128 {
    if !visited.insert(current) {
      return Err("Циклическая структура страниц.".into());
    }
    let node = doc.get_dictionary(current).map_err(|e| e.to_string())?;
    for key in [b"Resources".as_slice(), b"MediaBox", b"CropBox", b"Rotate"] {
      if !page.has(key) {
        if let Ok(value) = node.get(key) {
          page.set(key, value.clone());
        }
      }
    }
    match node.get(b"Parent").and_then(Object::as_reference) {
      Ok(parent) => current = parent,
      Err(_) => break,
    }
  }
  if !page.has(b"MediaBox") {
    return Err("У страницы не задан размер.".into());
  }
  Ok(page)
}

fn has_forms(doc: &Document) -> bool {
  doc.catalog().ok().is_some_and(|c| c.has(b"AcroForm"))
}

fn clone_annotations(
  doc: &mut Document,
  page: &mut Dictionary,
  new_id: ObjectId,
) -> Result<(), String> {
  let Some(items) = page
    .get(b"Annots")
    .ok()
    .and_then(|o| direct(doc, o).ok())
    .and_then(|o| o.as_array().ok())
    .cloned()
  else {
    return Ok(());
  };
  let mut cloned = Vec::new();
  for item in items {
    let mut annotation = direct(doc, &item)?
      .as_dict()
      .map_err(|e| e.to_string())?
      .clone();
    annotation.set("P", new_id);
    // Связанный всплывающий комментарий не должен ссылаться на исходную страницу.
    annotation.remove(b"Popup");
    cloned.push(Object::Reference(doc.add_object(annotation)));
  }
  page.set("Annots", cloned);
  Ok(())
}

fn write(mut doc: Document) -> Result<Vec<u8>, String> {
  doc.prune_objects();
  let mut bytes = Vec::new();
  doc.save_to(&mut bytes).map_err(|e| e.to_string())?;
  if bytes.len() > 96_000_000 {
    return Err("Рабочая копия превышает 96 МБ.".into());
  }
  Ok(bytes)
}

pub fn reorder(bytes: &[u8], order: &[usize]) -> Result<Vec<u8>, String> {
  let mut doc = load(bytes)?;
  let pages: Vec<_> = doc.get_pages().into_values().collect();
  if order.is_empty() || order.len() > 100_000 || order.iter().any(|p| *p >= pages.len()) {
    return Err("Неверный порядок страниц. В PDF должна остаться хотя бы одна страница.".into());
  }
  let unique: BTreeSet<_> = order.iter().copied().collect();
  if has_forms(&doc) && (unique.len() != pages.len() || unique.len() != order.len()) {
    return Err("Удаление и дублирование страниц с интерактивными полями пока недоступно. Перестановка сохраняет поля.".into());
  }
  let root = doc
    .catalog()
    .map_err(|e| e.to_string())?
    .get(b"Pages")
    .and_then(Object::as_reference)
    .map_err(|e| e.to_string())?;
  let mut seen = HashSet::new();
  let mut kids = Vec::new();
  for &index in order {
    let original = pages[index];
    let mut page = page_dictionary(&doc, original)?;
    let id = if seen.insert(index) {
      original
    } else {
      doc.new_object_id()
    };
    page.set("Parent", root);
    if id != original {
      clone_annotations(&mut doc, &mut page, id)?;
    }
    doc.objects.insert(id, page.into());
    kids.push(Object::Reference(id));
  }
  doc.objects.insert(
    root,
    dictionary! {"Type"=>"Pages","Kids"=>kids,"Count"=>order.len() as i64}.into(),
  );
  doc
    .catalog_mut()
    .map_err(|e| e.to_string())?
    .remove(b"PageLabels");
  // Недействительные переходы удаляются вместе с удалёнными страницами.
  let removed: HashSet<_> = pages
    .iter()
    .enumerate()
    .filter(|(i, _)| !unique.contains(i))
    .map(|(_, id)| *id)
    .collect();
  if !removed.is_empty() {
    remove_destinations(&mut doc, &removed);
  }
  write(doc)
}

fn remove_destinations(doc: &mut Document, removed: &HashSet<ObjectId>) {
  let mut invalid = Vec::new();
  for (&id, object) in &doc.objects {
    if let Ok(d) = object.as_dict() {
      for key in [b"Dest".as_slice(), b"D"] {
        if let Ok(value) = d.get(key) {
          if direct(doc, value)
            .ok()
            .and_then(|v| v.as_array().ok())
            .and_then(|a| a.first())
            .and_then(|o| o.as_reference().ok())
            .is_some_and(|p| removed.contains(&p))
          {
            invalid.push((id, key.to_vec()));
          }
        }
      }
    }
  }
  for (id, key) in invalid {
    if let Ok(d) = doc.get_object_mut(id).and_then(Object::as_dict_mut) {
      d.remove(&key);
    }
  }
  if let Ok(c) = doc.catalog_mut() {
    c.remove(b"OpenAction");
    c.remove(b"StructTreeRoot");
  }
  fn unlink(value: &mut Object, removed: &HashSet<ObjectId>) {
    match value {
      Object::Reference(id) if removed.contains(id) => *value = Object::Null,
      Object::Array(items) => {
        for item in items {
          unlink(item, removed)
        }
      }
      Object::Dictionary(d) => {
        for (_, item) in d.iter_mut() {
          unlink(item, removed)
        }
      }
      Object::Stream(s) => {
        for (_, item) in s.dict.iter_mut() {
          unlink(item, removed)
        }
      }
      _ => (),
    }
  }
  for object in doc.objects.values_mut() {
    unlink(object, removed);
  }
}

pub fn insert(original: &[u8], other: &[u8], range: &str, at: usize) -> Result<Vec<u8>, String> {
  let mut doc = load(original)?;
  let mut incoming = load(other)?;
  if has_forms(&doc) || has_forms(&incoming) {
    return Err(
      "Объединение PDF с интерактивными полями пока недоступно: поля могут иметь одинаковые имена."
        .into(),
    );
  }
  let own: Vec<_> = doc.get_pages().into_values().collect();
  let count = incoming.get_pages().len();
  let selected = if range.trim().is_empty() {
    (0..count).collect()
  } else {
    crate::editing::pages(range, count)?
  };
  if at > own.len() || own.len() + selected.len() > 100_000 {
    return Err("Недопустимое положение вставки.".into());
  }
  incoming.renumber_objects_with(doc.max_id + 1);
  let added: Vec<_> = incoming.get_pages().into_values().collect();
  let root = doc
    .catalog()
    .map_err(|e| e.to_string())?
    .get(b"Pages")
    .and_then(Object::as_reference)
    .map_err(|e| e.to_string())?;
  let mut pages = Vec::new();
  for id in &own {
    pages.push((*id, page_dictionary(&doc, *id)?));
  }
  let inserted: Vec<_> = selected
    .iter()
    .map(|i| Ok((added[*i], page_dictionary(&incoming, added[*i])?)))
    .collect::<Result<_, String>>()?;
  let mut incoming_bytes = Vec::new();
  incoming
    .save_to(&mut incoming_bytes)
    .map_err(|e| e.to_string())?;
  let mut layers = crate::layers::Layers::read(original)?;
  let other_layers = crate::layers::Layers::read(&incoming_bytes)?;
  layers.items.extend(other_layers.items);
  layers.radio_groups.extend(other_layers.radio_groups);
  doc.max_id = incoming.max_id.max(doc.max_id);
  doc.objects.extend(incoming.objects);
  pages.splice(at..at, inserted);
  let mut kids = Vec::new();
  for (id, mut page) in pages {
    page.set("Parent", root);
    doc.objects.insert(id, page.into());
    kids.push(Object::Reference(id));
  }
  doc.objects.insert(
    root,
    dictionary! {"Type"=>"Pages","Count"=>kids.len() as i64,"Kids"=>kids}.into(),
  );
  let catalog = doc.catalog_mut().map_err(|e| e.to_string())?;
  catalog.remove(b"PageLabels");
  catalog.remove(b"StructTreeRoot");
  if !layers.items.is_empty() {
    let groups: Vec<Object> = layers
      .items
      .iter()
      .map(|l| Object::Reference(l.id))
      .collect();
    let on: Vec<Object> = layers
      .items
      .iter()
      .filter(|l| l.visible)
      .map(|l| Object::Reference(l.id))
      .collect();
    let off: Vec<Object> = layers
      .items
      .iter()
      .filter(|l| !l.visible)
      .map(|l| Object::Reference(l.id))
      .collect();
    let locked: Vec<Object> = layers
      .items
      .iter()
      .filter(|l| l.locked)
      .map(|l| Object::Reference(l.id))
      .collect();
    let radio: Vec<Object> = layers
      .radio_groups
      .iter()
      .map(|r| Object::Array(r.iter().map(|id| Object::Reference(*id)).collect()))
      .collect();
    catalog.set("OCProperties",dictionary! {"OCGs"=>groups.clone(),"D"=>dictionary! {"BaseState"=>"ON","ON"=>on,"OFF"=>off,"Order"=>groups,"Locked"=>locked,"RBGroups"=>radio}});
  }
  write(doc)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::pdf::{Api, Pdf};
  use std::sync::Arc;
  #[test]
  fn reorder_duplicate_delete_and_merge_preserve_content_and_layers() {
    let bytes = crate::fixture::demo();
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let original = Pdf::open(api.clone(), Arc::new(bytes.clone())).unwrap();
    let changed = reorder(&bytes, &[2, 0, 0]).unwrap();
    let pdf = Pdf::open(api.clone(), Arc::new(changed.clone())).unwrap();
    assert_eq!(pdf.sizes.len(), 3);
    for (new, old) in [(0, 2), (1, 0), (2, 0)] {
      assert_eq!(
        pdf.render(new, 300, 400, 0, false).unwrap().pixels,
        original.render(old, 300, 400, 0, false).unwrap().pixels
      );
    }
    assert_eq!(
      crate::layers::Layers::read(&changed).unwrap().items.len(),
      3
    );
    assert!(reorder(&bytes, &[]).is_err());
    assert!(reorder(&bytes, &[3]).is_err());
    let merged = insert(&bytes, &changed, "2-3", 1).unwrap();
    let pdf = Pdf::open(api, Arc::new(merged.clone())).unwrap();
    assert_eq!(pdf.sizes.len(), 5);
    assert_eq!(crate::layers::Layers::read(&merged).unwrap().items.len(), 6);
    for (new, old) in [(0, 0), (1, 0), (2, 0), (3, 1), (4, 2)] {
      assert_eq!(
        pdf.render(new, 300, 400, 0, false).unwrap().pixels,
        original.render(old, 300, 400, 0, false).unwrap().pixels
      );
    }
  }

  #[test]
  fn inherited_size_rotation_and_duplicate_annotations_survive_reorder() {
    let mut doc = load(&crate::fixture::demo()).unwrap();
    let pages: Vec<_> = doc.get_pages().into_values().collect();
    let parent = doc
      .catalog()
      .unwrap()
      .get(b"Pages")
      .unwrap()
      .as_reference()
      .unwrap();
    let media = doc
      .get_dictionary(pages[0])
      .unwrap()
      .get(b"MediaBox")
      .unwrap()
      .clone();
    doc
      .get_object_mut(parent)
      .unwrap()
      .as_dict_mut()
      .unwrap()
      .set("MediaBox", media);
    doc
      .get_object_mut(parent)
      .unwrap()
      .as_dict_mut()
      .unwrap()
      .set("Rotate", 90);
    for id in &pages {
      doc
        .get_object_mut(*id)
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .remove(b"MediaBox");
    }
    let annotation=doc.add_object(dictionary! {"Type"=>"Annot","Subtype"=>"Text","Rect"=>vec![20.into(),20.into(),50.into(),50.into()],"Contents"=>Object::string_literal("Comment"),"P"=>pages[0]});
    doc
      .get_object_mut(pages[0])
      .unwrap()
      .as_dict_mut()
      .unwrap()
      .set("Annots", vec![Object::Reference(annotation)]);
    let bytes = write(doc).unwrap();
    let changed = reorder(&bytes, &[0, 0, 2]).unwrap();
    let d = load(&changed).unwrap();
    let p: Vec<_> = d.get_pages().into_values().collect();
    assert_ne!(p[0], p[1]);
    assert_eq!(
      d.get_dictionary(p[1])
        .unwrap()
        .get(b"Rotate")
        .unwrap()
        .as_i64()
        .unwrap(),
      90
    );
    let a = d.get_page_annotations(p[1]).unwrap();
    assert_eq!(a[0].get(b"P").unwrap().as_reference().unwrap(), p[1]);
    let pdf = Pdf::open(Api::new(&crate::pdfium_path()).unwrap(), Arc::new(changed)).unwrap();
    assert_eq!(pdf.sizes[0], (800., 600.));
  }
}

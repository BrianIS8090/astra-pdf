use lopdf::{
  content::{Content, Operation},
  Dictionary, Document, Object, ObjectId, Stream,
};

fn err(e: impl std::fmt::Display) -> String {
  e.to_string()
}
fn dictionary(doc: &Document, value: &Object) -> Result<Dictionary, String> {
  doc
    .dereference(value)
    .map_err(err)?
    .1
    .as_dict()
    .cloned()
    .map_err(err)
}
pub(super) fn resources(doc: &Document, page: ObjectId) -> Result<Dictionary, String> {
  let mut at = page;
  for _ in 0..64 {
    let d = doc.get_object(at).and_then(Object::as_dict).map_err(err)?;
    if let Ok(r) = d.get(b"Resources") {
      return dictionary(doc, r);
    }
    at = d
      .get(b"Parent")
      .and_then(Object::as_reference)
      .map_err(err)?;
  }
  Err("Слишком глубокое дерево ресурсов PDF.".into())
}
pub(super) fn content(doc: &Document, id: ObjectId, page: bool) -> Result<Vec<Operation>, String> {
  let bytes = if page {
    doc.get_page_content(id)
  } else {
    let s = doc
      .get_object(id)
      .and_then(Object::as_stream)
      .map_err(err)?;
    s.decompressed_content()
      .unwrap_or_else(|_| s.content.clone())
  };
  Ok(Content::decode(&bytes).map_err(err)?.operations)
}
pub(super) fn write(
  doc: &mut Document,
  id: ObjectId,
  page: bool,
  ops: Vec<Operation>,
) -> Result<(), String> {
  let bytes = Content { operations: ops }.encode().map_err(err)?;
  if page {
    let stream = doc.add_object(Stream::new(Dictionary::new(), bytes));
    doc
      .get_object_mut(id)
      .and_then(Object::as_dict_mut)
      .map_err(err)?
      .set("Contents", stream);
  } else {
    let stream = doc
      .get_object_mut(id)
      .and_then(Object::as_stream_mut)
      .map_err(err)?;
    stream.set_plain_content(bytes);
  }
  Ok(())
}
// Каждый вызов вложенной группы получает свои команды и ресурсы. Правка одного
// экземпляра не меняет повторения той же группы на этой или других страницах.
pub(super) fn detach(doc: &mut Document, page: ObjectId) -> Result<Vec<(ObjectId, bool)>, String> {
  fn visit(
    doc: &mut Document,
    id: ObjectId,
    page: bool,
    mut res: Dictionary,
    depth: usize,
    out: &mut Vec<(ObjectId, bool)>,
  ) -> Result<(), String> {
    if depth > 32 || out.len() > 4096 {
      return Err("Слишком много вложенных групп PDF.".into());
    }
    out.push((id, page));
    let mut ops = content(doc, id, page)?;
    let mut xobjects = res
      .get(b"XObject")
      .ok()
      .map(|v| dictionary(doc, v))
      .transpose()?
      .unwrap_or_default();
    for (i, op) in ops.iter_mut().enumerate() {
      if op.operator != "Do" {
        continue;
      }
      let Some(name) = op.operands.first().and_then(|o| o.as_name().ok()) else {
        continue;
      };
      let Some(object) = xobjects.get(name).ok() else {
        continue;
      };
      let stream = doc
        .dereference(object)
        .ok()
        .and_then(|(_, o)| o.as_stream().ok())
        .cloned();
      let Some(stream) = stream else { continue };
      if stream.dict.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Form") {
        continue;
      }
      let child_res = stream
        .dict
        .get(b"Resources")
        .ok()
        .map(|v| dictionary(doc, v))
        .transpose()?
        .unwrap_or_else(|| res.clone());
      let child = doc.add_object(stream);
      let mut fresh = format!("AstraGroup{i}").into_bytes();
      while xobjects.has(&fresh) {
        fresh.push(b'X');
      }
      xobjects.set(fresh.clone(), child);
      op.operands[0] = Object::Name(fresh);
      visit(doc, child, false, child_res, depth + 1, out)?;
    }
    res.set("XObject", xobjects);
    if page {
      doc
        .get_object_mut(id)
        .and_then(Object::as_dict_mut)
        .map_err(err)?
        .set("Resources", res);
    } else {
      doc
        .get_object_mut(id)
        .and_then(Object::as_stream_mut)
        .map_err(err)?
        .dict
        .set("Resources", res);
    }
    write(doc, id, page, ops)
  }
  let res = resources(doc, page)?;
  let mut out = Vec::new();
  visit(doc, page, true, res, 0, &mut out)?;
  Ok(out)
}

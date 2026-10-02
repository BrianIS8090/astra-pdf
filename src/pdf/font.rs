use super::text::Codes;
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use skrifa::{
  instance::{LocationRef, Size},
  raw::TableProvider,
  string::StringId,
  FontRef, MetadataProvider, Tag,
};
use std::path::Path;

pub(super) fn embed(
  doc: &mut Document,
  id: ObjectId,
  page: bool,
  path: &Path,
  text: &str,
) -> Result<(Vec<u8>, Codes), String> {
  let data = crate::export::read(path, 16 * 1024 * 1024)?;
  let face = FontRef::new(&data).map_err(|e| format!("Не удалось прочитать шрифт: {e:?}"))?;
  let flags = face
    .os2()
    .map_err(|e| format!("Не удалось проверить разрешение встраивания: {e}"))?
    .fs_type();
  if flags & 0x202 != 0 || (flags & 4 != 0 && flags & 8 == 0) {
    return Err("Этот шрифт запрещает встраивание для редактирования. Выберите другой TTF.".into());
  }
  if face.data_for_tag(Tag::new(b"fvar")).is_some()
    || face.glyf().is_err()
    || data.starts_with(b"ttcf")
  {
    return Err(
      "Выберите обычный TrueType-файл .ttf. Переменные шрифты и коллекции пока не поддерживаются."
        .into(),
    );
  }
  let metrics = face.metrics(Size::new(1000.), LocationRef::default());
  if !(16..=16384).contains(&metrics.units_per_em) {
    return Err("Недопустимый масштаб единиц шрифта.".into());
  }
  let glyph_metrics = face.glyph_metrics(Size::new(1000.), LocationRef::default());
  let charmap = face.charmap();
  let name = face
    .localized_strings(StringId::POSTSCRIPT_NAME)
    .next()
    .map(|n| n.to_string())
    .unwrap_or_else(|| "AstraEmbedded".into());
  let name: Vec<u8> = name
    .bytes()
    .filter(|c| c.is_ascii_alphanumeric() || *c == b'-')
    .collect();
  let mut codes = Codes::new();
  let mut chars: Vec<_> = text.chars().filter(|c| *c != '\n').collect();
  chars.sort_unstable();
  chars.dedup();
  if chars.len() > 4096 {
    return Err("Слишком много различных символов.".into());
  }
  let mut widths = Vec::new();
  let mut mapping = vec![0u8; 2];
  let mut unicode = Vec::new();
  for (i, c) in chars.iter().enumerate() {
    let glyph = charmap
      .map(*c)
      .filter(|g| g.to_u32() != 0 && g.to_u32() <= u16::MAX as u32)
      .ok_or_else(|| format!("В выбранном шрифте нет символа «{c}»."))?;
    let width = glyph_metrics
      .advance_width(glyph)
      .ok_or("В шрифте нет ширины символа.")?
      / 1000.;
    let width = (width * 1000.).round() / 1000.;
    let cid = (i + 1) as u16;
    codes.insert(*c, (cid.to_be_bytes().to_vec(), width));
    widths.push(Object::Real(width * 1000.));
    mapping.extend((glyph.to_u32() as u16).to_be_bytes());
    unicode.push(format!("<{cid:04X}> <{:04X}>", *c as u32));
  }
  let mut cmap=String::from("/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /AstraUnicode def /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange\n");
  for group in unicode.chunks(100) {
    cmap.push_str(&format!(
      "{} beginbfchar\n{}\nendbfchar\n",
      group.len(),
      group.join("\n")
    ));
  }
  cmap.push_str("endcmap CMapName currentdict /CMap defineresource pop end end");
  let bbox = metrics.bounds.ok_or("В шрифте нет габаритов символов.")?;
  let bbox: Vec<Object> = [bbox.x_min, bbox.y_min, bbox.x_max, bbox.y_max]
    .into_iter()
    .map(Object::Real)
    .collect();
  let mut file = Stream::new(dictionary! {"Length1"=>data.len() as i64}, data.clone());
  file.compress().map_err(|e| e.to_string())?;
  let file = doc.add_object(file);
  let descriptor=doc.add_object(dictionary!{"Type"=>"FontDescriptor","FontName"=>Object::Name(name.clone()),"Flags"=>32i64,"FontBBox"=>bbox,"ItalicAngle"=>metrics.italic_angle,"Ascent"=>metrics.ascent,"Descent"=>metrics.descent,"CapHeight"=>metrics.cap_height.unwrap_or(metrics.ascent),"StemV"=>80,"FontFile2"=>file});
  let map = doc.add_object(Stream::new(Dictionary::new(), mapping));
  let unicode = doc.add_object(Stream::new(Dictionary::new(), cmap.into_bytes()));
  let descendant=doc.add_object(dictionary!{"Type"=>"Font","Subtype"=>"CIDFontType2","BaseFont"=>Object::Name(name.clone()),"CIDSystemInfo"=>dictionary!{"Registry"=>Object::string_literal("Adobe"),"Ordering"=>Object::string_literal("Identity"),"Supplement"=>0},"FontDescriptor"=>descriptor,"CIDToGIDMap"=>map,"W"=>vec![Object::Integer(1),Object::Array(widths)]});
  let font=doc.add_object(dictionary!{"Type"=>"Font","Subtype"=>"Type0","BaseFont"=>Object::Name(name),"Encoding"=>"Identity-H","DescendantFonts"=>vec![Object::Reference(descendant)],"ToUnicode"=>unicode});
  let res = if page {
    doc.get_object(id).and_then(Object::as_dict)
  } else {
    doc
      .get_object(id)
      .and_then(Object::as_stream)
      .map(|s| &s.dict)
  }
  .map_err(|e| e.to_string())?
  .get(b"Resources")
  .map_err(|e| e.to_string())?;
  let mut resources = doc
    .dereference(res)
    .and_then(|(_, o)| o.as_dict())
    .map_err(|e| e.to_string())?
    .clone();
  let mut fonts = resources
    .get(b"Font")
    .ok()
    .and_then(|f| doc.dereference(f).ok())
    .and_then(|(_, o)| o.as_dict().ok())
    .cloned()
    .unwrap_or_default();
  let mut key = b"AstraFont".to_vec();
  while fonts.has(&key) {
    key.push(b'X')
  }
  fonts.set(key.clone(), font);
  resources.set("Font", fonts);
  if page {
    doc
      .get_object_mut(id)
      .and_then(Object::as_dict_mut)
      .map_err(|e| e.to_string())?
      .set("Resources", resources)
  } else {
    doc
      .get_object_mut(id)
      .and_then(Object::as_stream_mut)
      .map_err(|e| e.to_string())?
      .dict
      .set("Resources", resources)
  }
  Ok((key, codes))
}

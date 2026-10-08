use lopdf::{dictionary, Document, Object, ObjectId, Stream};

pub(super) fn appearance(
  doc: &mut Document,
  width: f64,
  height: f64,
  text: &str,
  color: [u8; 3],
  size: f64,
) -> Result<(ObjectId, String), String> {
  if !width.is_finite() || !height.is_finite() || width <= 0. || height <= 0. {
    return Err("Недопустимые границы текстовой заметки.".into());
  }
  let text = text.replace("\r\n", "\n").replace('\r', "\n");
  let id = doc.add_object(Stream::new(
    dictionary! {
      "Type"=>"XObject", "Subtype"=>"Form",
      "BBox"=>vec![0.into(), 0.into(), Object::Real(width as f32), Object::Real(height as f32)],
      "Resources"=>dictionary!{},
    },
    Vec::new(),
  ));
  let font = std::path::PathBuf::from(
    std::env::var_os("WINDIR").ok_or("Не найден каталог шрифтов Windows.")?,
  )
  .join("Fonts/arial.ttf");
  let (name, codes) = super::font::embed(doc, id, false, &font, &text)?;
  let mut lines = Vec::new();
  for paragraph in text.split('\n') {
    let mut line = Vec::new();
    let mut advance = 0.;
    for ch in paragraph.chars() {
      let (code, glyph_width) = codes
        .get(&ch)
        .ok_or("В шрифте отсутствует символ заметки.")?;
      let next = *glyph_width as f64 * size;
      if next > width - 6. {
        return Err("Область текста слишком узкая. Выделите область шире.".into());
      }
      if advance + next > width - 6. && !line.is_empty() {
        lines.push(std::mem::take(&mut line));
        advance = 0.;
      }
      line.extend_from_slice(code);
      advance += next;
    }
    lines.push(line);
  }
  let leading = size * 1.25;
  if size + (lines.len().saturating_sub(1) as f64) * leading > height - 6. {
    return Err(
      "Текст не помещается по высоте. Выделите область выше или сократите заметку.".into(),
    );
  }
  let rgb = color.map(|v| v as f64 / 255.);
  let font_name = String::from_utf8(name).map_err(|e| e.to_string())?;
  let da = format!("/{font_name} {size} Tf {} {} {} rg", rgb[0], rgb[1], rgb[2]);
  let mut commands = format!(
    "q 0 0 {width} {height} re W n BT {da} {leading} TL 3 {} Td\n",
    height - 3. - size
  );
  for (i, line) in lines.iter().enumerate() {
    if i > 0 {
      commands.push_str("T*\n");
    }
    let hex: String = line.iter().map(|b| format!("{b:02X}")).collect();
    commands.push_str(&format!("<{hex}> Tj\n"));
  }
  commands.push_str("ET Q\n");
  let stream = doc
    .get_object_mut(id)
    .and_then(Object::as_stream_mut)
    .map_err(|e| e.to_string())?;
  stream.set_content(commands.into_bytes());
  stream.compress().map_err(|e| e.to_string())?;
  Ok((id, da))
}

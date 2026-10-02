use super::*;
use crate::annotation::{Kind, Note};
use lopdf::{dictionary, Object, Stream};

fn unicode(text: &str) -> Object {
  let mut bytes = vec![0xfe, 0xff];
  for c in text.encode_utf16() {
    bytes.extend_from_slice(&c.to_be_bytes());
  }
  Object::String(bytes, lopdf::StringFormat::Hexadecimal)
}

impl Pdf {
  pub fn edit_note(
    &self,
    page: usize,
    index: usize,
    text: Option<&str>,
  ) -> Result<Vec<u8>, String> {
    let mut doc = lopdf::Document::load_mem(&self._bytes).map_err(|e| e.to_string())?;
    let id = *doc
      .get_pages()
      .get(&(page as u32 + 1))
      .ok_or("Страница отсутствует.")?;
    let page = doc.get_dictionary(id).map_err(|e| e.to_string())?;
    let mut list = doc
      .dereference(page.get(b"Annots").map_err(|e| e.to_string())?)
      .map_err(|e| e.to_string())?
      .1
      .as_array()
      .map_err(|e| e.to_string())?
      .clone();
    let selected = list.get(index).ok_or("Замечание отсутствует.")?;
    if let Some(text) = text {
      if text.trim().is_empty() || text.len() > 16_384 {
        return Err("Введите непустой комментарий не длиннее 16384 байт.".into());
      }
      let mut annotation = doc
        .dereference(selected)
        .map_err(|e| e.to_string())?
        .1
        .as_dict()
        .map_err(|e| e.to_string())?
        .clone();
      if annotation.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Text") {
        return Err("Изменение текста доступно у заметки с иконкой комментария.".into());
      }
      annotation.set("Contents", unicode(text));
      if let Ok(id) = selected.as_reference() {
        doc.objects.insert(id, annotation.into());
      } else {
        list[index] = Object::Reference(doc.add_object(annotation));
      }
    } else {
      list.remove(index);
    }
    doc
      .get_object_mut(id)
      .and_then(Object::as_dict_mut)
      .map_err(|e| e.to_string())?
      .set("Annots", list);
    doc.prune_objects();
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
  }

  pub fn annotate(&self, note: &Note) -> Result<Vec<u8>, String> {
    if !note.valid(self.sizes.len()) {
      return Err("Недопустимое замечание или масштаб измерения.".into());
    }
    let mut doc = lopdf::Document::load_mem(&self._bytes).map_err(|e| e.to_string())?;
    let page_id = doc.get_pages()[&(note.page as u32 + 1)];
    let mut points = Vec::new();
    unsafe {
      let convert=*self.api._library.get::<unsafe extern "C" fn(Handle,i32,i32,i32,i32,i32,i32,i32,*mut f64,*mut f64)->i32>(b"FPDF_DeviceToPage\0").map_err(|e|e.to_string())?;
      let page = (self.api.page)(self.handle, note.page as i32);
      if page.is_null() {
        return Err("Не удалось открыть страницу.".into());
      }
      let mut failed = false;
      for p in &note.points {
        let (mut x, mut y) = (0., 0.);
        if convert(
          page,
          0,
          0,
          1_000_000,
          1_000_000,
          0,
          (p[0] * 1_000_000.).round() as i32,
          (p[1] * 1_000_000.).round() as i32,
          &mut x,
          &mut y,
        ) == 0
        {
          failed = true;
          break;
        }
        points.push((x, y));
      }
      (self.api.close_page)(page);
      if failed {
        return Err("Не удалось определить координаты замечания.".into());
      }
    }
    let mut left = points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let mut right = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let mut bottom = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let mut top = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    if matches!(note.kind, Kind::Comment(_)) {
      right = left + 18.;
      bottom = top - 18.;
    }
    let label = note.measure(self.sizes[note.page]);
    if label.is_some() {
      right = right.max(left + 110.);
      top += 18.;
    }
    if !matches!(note.kind, Kind::Highlight) {
      left -= 4.;
      right += 4.;
      bottom -= 4.;
      top += 4.;
    }
    let width = (right - left).max(0.01);
    let height = (top - bottom).max(0.01);
    let xy: Vec<_> = points.iter().map(|p| (p.0 - left, p.1 - bottom)).collect();
    let mut commands = String::from("q /GS gs 1.5 w 0.10 0.42 0.76 RG 0.10 0.42 0.76 rg\n");
    let mut annotation = dictionary! {"Type"=>"Annot","P"=>page_id,"F"=>4,"Rect"=>vec![Object::Real(left as f32),Object::Real(bottom as f32),Object::Real(right as f32),Object::Real(top as f32)],"T"=>unicode("Astra PDF"),"NM"=>Object::string_literal(crate::vault::random::<16>()?.iter().map(|b|format!("{b:02x}")).collect::<String>()),"C"=>vec![Object::Real(0.10),Object::Real(0.42),Object::Real(0.76)],"Border"=>vec![0.into(),0.into(),1.into()]};
    let mut alpha = 1.;
    match &note.kind {
      Kind::Comment(value) => {
        annotation.set("Subtype", "Text");
        annotation.set("Name", "Comment");
        annotation.set("Contents", unicode(value));
        commands.push_str("1 0.82 0.20 rg 4 4 18 18 re f 0.35 0.26 0.08 RG 7 17 m 19 17 l S 7 13 m 19 13 l S 7 9 m 16 9 l S\n");
      }
      Kind::Rectangle => {
        annotation.set("Subtype", "Square");
        commands.push_str(&format!(
          "4 4 {} {} re S\n",
          (width - 8.).max(0.1),
          (height - 8.).max(0.1)
        ));
      }
      Kind::Highlight => {
        annotation.set("Subtype", "Highlight");
        annotation.set("C", vec![1.into(), Object::Real(0.85), Object::Real(0.05)]);
        annotation.set(
          "QuadPoints",
          vec![
            Object::Real(left as f32),
            Object::Real(top as f32),
            Object::Real(right as f32),
            Object::Real(top as f32),
            Object::Real(left as f32),
            Object::Real(bottom as f32),
            Object::Real(right as f32),
            Object::Real(bottom as f32),
          ],
        );
        commands.push_str(&format!("1 0.85 0.05 rg 0 0 {width} {height} re f\n"));
        alpha = 0.35;
      }
      Kind::Arrow | Kind::Distance { .. } => {
        annotation.set("Subtype", "Line");
        annotation.set(
          "L",
          vec![
            Object::Real(points[0].0 as f32),
            Object::Real(points[0].1 as f32),
            Object::Real(points[1].0 as f32),
            Object::Real(points[1].1 as f32),
          ],
        );
        let (a, b) = (xy[0], xy[1]);
        commands.push_str(&format!("{} {} m {} {} l S\n", a.0, a.1, b.0, b.1));
        let angle = (b.1 - a.1).atan2(b.0 - a.0);
        let length = 8.;
        commands.push_str(&format!(
          "{} {} m {} {} l {} {} l h f\n",
          b.0,
          b.1,
          b.0 - length * (angle - 0.45).cos(),
          b.1 - length * (angle - 0.45).sin(),
          b.0 - length * (angle + 0.45).cos(),
          b.1 - length * (angle + 0.45).sin()
        ));
        annotation.set(
          "LE",
          vec![
            Object::Name(b"None".to_vec()),
            Object::Name(b"ClosedArrow".to_vec()),
          ],
        );
      }
      Kind::Area { .. } => {
        annotation.set("Subtype", "Polygon");
        annotation.set(
          "Vertices",
          Object::Array(
            points
              .iter()
              .flat_map(|p| [Object::Real(p.0 as f32), Object::Real(p.1 as f32)])
              .collect(),
          ),
        );
        commands.push_str(&format!("{} {} m\n", xy[0].0, xy[0].1));
        for p in xy.iter().skip(1) {
          commands.push_str(&format!("{} {} l\n", p.0, p.1));
        }
        commands.push_str("h S\n");
      }
    }
    if let Some(label) = label {
      let scale = match note.kind {
        Kind::Distance { scale } | Kind::Area { scale } => scale,
        _ => 1.,
      };
      annotation.set("Contents", unicode(&format!("{label}\nМасштаб 1:{scale}")));
      annotation.set("AstraScale", Object::Real(scale as f32));
      commands.push_str(&format!(
        "BT /F1 10 Tf 4 {} Td ({label}) Tj ET\n",
        height - 14.
      ));
    }
    commands.push_str("Q\n");
    annotation.set("CA", Object::Real(alpha));
    let font =
      doc.add_object(dictionary! {"Type"=>"Font","Subtype"=>"Type1","BaseFont"=>"Helvetica"});
    let state = dictionary! {"Type"=>"ExtGState","CA"=>Object::Real(alpha),"ca"=>Object::Real(alpha),"BM"=>if matches!(note.kind,Kind::Highlight){"Multiply"}else{"Normal"}};
    let resources =
      dictionary! {"Font"=>dictionary!{"F1"=>font},"ExtGState"=>dictionary!{"GS"=>state}};
    let appearance=doc.add_object(Stream::new(dictionary!{"Type"=>"XObject","Subtype"=>"Form","BBox"=>vec![0.into(),0.into(),Object::Real(width as f32),Object::Real(height as f32)],"Resources"=>resources},commands.into_bytes()));
    annotation.set("AP", dictionary! {"N"=>appearance});
    let annotation = doc.add_object(annotation);
    let mut list = doc
      .get_dictionary(page_id)
      .ok()
      .and_then(|p| p.get(b"Annots").ok())
      .and_then(|o| doc.dereference(o).ok())
      .and_then(|(_, o)| o.as_array().ok())
      .cloned()
      .unwrap_or_default();
    list.push(Object::Reference(annotation));
    doc
      .get_object_mut(page_id)
      .and_then(Object::as_dict_mut)
      .map_err(|e| e.to_string())?
      .set("Annots", list);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 96_000_000 {
      return Err("Документ превышает 96 МБ.".into());
    }
    Ok(bytes)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn annotations_survive_reopen_and_render_for_screen_and_print() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let original = crate::fixture::demo();
    let pdf = Pdf::open(api.clone(), Arc::new(original.clone())).unwrap();
    let before = pdf.render(0, 600, 800, 0, false).unwrap();
    for kind in [
      Kind::Comment("Проверить размер".into()),
      Kind::Rectangle,
      Kind::Arrow,
      Kind::Highlight,
      Kind::Distance { scale: 100. },
      Kind::Area { scale: 100. },
    ] {
      let points = match kind {
        Kind::Comment(_) => vec![[0.2, 0.3]],
        Kind::Area { .. } => vec![[0.2, 0.3], [0.5, 0.3], [0.4, 0.6]],
        _ => vec![[0.2, 0.3], [0.5, 0.5]],
      };
      let bytes = pdf
        .annotate(&Note {
          page: 0,
          points,
          kind,
        })
        .unwrap();
      let doc = lopdf::Document::load_mem(&bytes).unwrap();
      let id = doc.get_pages()[&1];
      assert_eq!(doc.get_page_annotations(id).unwrap().len(), 1);
      let after = Pdf::open(api.clone(), Arc::new(bytes)).unwrap();
      assert_ne!(
        after.render(0, 600, 800, 0, false).unwrap().pixels,
        before.pixels
      );
      assert_ne!(
        after.render(0, 600, 800, 0, true).unwrap().pixels,
        before.pixels
      );
      assert_eq!(
        after.read_page(0).unwrap().glyphs.len(),
        pdf.read_page(0).unwrap().glyphs.len()
      );
    }
  }
}

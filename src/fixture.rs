use lopdf::{dictionary, Document, Object, Stream, StringFormat};

fn unicode(value: &str) -> Object {
  let mut bytes = vec![0xfe, 0xff];
  for unit in value.encode_utf16() {
    bytes.extend_from_slice(&unit.to_be_bytes());
  }
  Object::String(bytes, StringFormat::Hexadecimal)
}

pub fn demo() -> Vec<u8> {
  let mut doc = Document::with_version("1.7");
  let pages = doc.new_object_id();
  let font = doc
    .add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
  let structure = doc.add_object(dictionary! { "Type" => "OCG", "Name" => unicode("Конструкция") });
  let electric = doc.add_object(dictionary! { "Type" => "OCG", "Name" => unicode("Электрика") });
  let dimensions = doc.add_object(dictionary! { "Type" => "OCG", "Name" => unicode("Размеры") });
  let resources = dictionary! { "Font" => dictionary! { "F1" => font }, "Properties" => dictionary! { "Structure" => structure, "Electric" => electric, "Dimensions" => dimensions } };
  let mut kids = vec![];
  for i in 0..3 {
    let page_title = [
      "LIGHT / STRUCTURE",
      "ASSEMBLY / DETAILS",
      "CIRCUIT / CONNECTIONS",
    ][i];
    let mut content = format!("q\n0.08 0.13 0.20 rg 0 720 600 80 re f\nBT /F1 10 Tf 1 1 1 rg 40 767 Td (ASTRA PDF    /    LAYER DEMONSTRATION) Tj ET\nBT /F1 22 Tf 1 1 1 rg 40 736 Td ({page_title}) Tj ET\n0.22 0.27 0.32 rg BT /F1 11 Tf 40 685 Td (Three independent layers. Toggle them in the left panel.) Tj ET\nBT /F1 10 Tf 40 38 Td (A-0{}    /    SCALE 1:10    /    600 x 800 pt) Tj ET\n0.82 0.85 0.87 RG 40 58 m 560 58 l S\n", i + 1);
    content.push_str("/OC /Structure BDC\n0.86 0.92 0.94 rg 105 195 390 390 re f\n0.14 0.28 0.35 RG 5 w 105 195 390 390 re S\n2 w 125 215 350 350 re S\n105 325 m 495 325 l S\n105 455 m 495 455 l S\n235 195 m 235 585 l S\n365 195 m 365 585 l S\nEMC\n");
    content.push_str("/OC /Electric BDC\n0.89 0.36 0.16 RG 4 w 150 260 m 430 260 l 430 520 l 150 520 l S\n0.89 0.36 0.16 rg 140 250 20 20 re f 140 510 20 20 re f\nBT /F1 12 Tf 160 610 Td (24V DC / LED BUS) Tj ET\nEMC\n");
    content.push_str("/OC /Dimensions BDC\n0.19 0.40 0.78 RG 1 w 105 155 m 495 155 l S\n105 145 m 105 180 l S 495 145 m 495 180 l S\n65 195 m 65 585 l S 55 195 m 90 195 l S 55 585 m 90 585 l S\n0.19 0.40 0.78 rg BT /F1 12 Tf 283 132 Td (1200) Tj ET\nBT /F1 12 Tf 42 384 Td (1200) Tj ET\nEMC\nQ\n");
    let stream = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
    let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![0.into(), 0.into(), 600.into(), 800.into()], "Resources" => resources.clone(), "Contents" => stream });
    kids.push(Object::Reference(page));
  }
  doc.objects.insert(
    pages,
    dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 3 }.into(),
  );
  let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages, "OCProperties" => dictionary! {
    "OCGs" => vec![structure.into(), electric.into(), dimensions.into()],
    "D" => dictionary! { "Name" => Object::string_literal("Default"), "BaseState" => "ON", "Order" => vec![structure.into(), electric.into(), dimensions.into()] }
  } });
  doc.trailer.set("Root", catalog);
  let mut bytes = vec![];
  doc.save_to(&mut bytes).unwrap();
  bytes
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{
    layers::{Layer, Layers},
    pdf::{Api, Pdf},
  };
  use std::sync::Arc;

  #[test]
  fn detailed_regions_match_full_page_at_every_rotation_and_layer_state() {
    use crate::model::Region;
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let layers = Layers::read(&demo()).unwrap();
    for states in [[true, true, true], [false, true, false]] {
      let pdf = Pdf::open(api.clone(), Arc::new(layers.with_states(&states).unwrap())).unwrap();
      for rotation in 0..4 {
        let full = pdf.render(0, 1200, 1600, rotation, false).unwrap();
        for (x, y) in [(0, 0), (410, 580), (900, 1300)] {
          let r = Region {
            x,
            y,
            width: 300,
            height: 300,
          };
          let tile = pdf
            .render_region(0, (1200, 1600), rotation, false, Some(r))
            .unwrap();
          let mut difference = 0;
          for row in 0..r.height as usize {
            let start = ((r.y as usize + row) * 1200 + r.x as usize) * 4;
            difference += full.pixels[start..start + 1200]
              .iter()
              .zip(&tile.pixels[row * 1200..(row + 1) * 1200])
              .filter(|(a, b)| a.abs_diff(**b) > 3)
              .count();
          }
          assert!(
            difference < 500,
            "Участок не совпал с полной страницей: {rotation}, {x}, {y}, {difference}"
          );
        }
      }
      let tile = pdf
        .render_region(
          0,
          (500_000, 700_000),
          0,
          false,
          Some(Region {
            x: 100_000,
            y: 150_000,
            width: 256,
            height: 256,
          }),
        )
        .unwrap();
      assert_eq!(tile.pixels.len(), 256 * 256 * 4);
      assert!(pdf
        .render_region(0, (500_000, 700_000), 0, false, None)
        .is_err());
    }
  }

  #[test]
  fn reads_unicode_layers_and_never_changes_original() {
    let input = demo();
    let original = input.clone();
    let layers = Layers::read(&input).unwrap();
    assert_eq!(layers.items.len(), 3);
    assert_eq!(layers.items[0].name, "Конструкция");
    assert!(layers.items.iter().all(|l| l.visible));
    let bytes = layers.with_states(&[false, true, false]).unwrap();
    let changed = Layers::read(&bytes).unwrap();
    assert_eq!(
      changed.items.iter().map(|l| l.visible).collect::<Vec<_>>(),
      [false, true, false]
    );
    assert_eq!(input, original);
    assert!(layers.with_states(&[true]).is_err());
  }

  #[test]
  fn locked_and_mutually_exclusive_groups() {
    let items = vec![
      Layer {
        id: (1, 0),
        name: "a".into(),
        visible: true,
        locked: false,
      },
      Layer {
        id: (2, 0),
        name: "b".into(),
        visible: false,
        locked: false,
      },
    ];
    let groups = vec![vec![(1, 0), (2, 0)]];
    let mut states = vec![true, false];
    Layers::apply_toggle(&items, &groups, &mut states, 1);
    assert_eq!(states, [false, true]);
    let mut items = items;
    items[1].locked = true;
    Layers::apply_toggle(&items, &groups, &mut states, 0);
    assert_eq!(states, [false, true]);
    Layers::apply_toggle(&items, &groups, &mut states, 1);
    assert_eq!(states, [false, true]);
  }

  #[test]
  fn default_configuration_and_usage_precedence() {
    let mut doc = Document::load_mem(&demo()).unwrap();
    let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
    let props = doc
      .get_object_mut(root)
      .unwrap()
      .as_dict_mut()
      .unwrap()
      .get_mut(b"OCProperties")
      .unwrap()
      .as_dict_mut()
      .unwrap();
    let groups = props.get(b"OCGs").unwrap().as_array().unwrap().clone();
    let first = groups[0].as_reference().unwrap();
    props.set("D", dictionary! { "BaseState" => "OFF" });
    props.set("Configs", vec![Object::Dictionary(dictionary! { "Intent" => "View", "BaseState" => "ON", "OFF" => vec![first.into()], "Locked" => vec![first.into()] })]);
    doc
      .get_object_mut(first)
      .unwrap()
      .as_dict_mut()
      .unwrap()
      .set(
        "Usage",
        dictionary! { "View" => dictionary! { "ViewState" => "ON" } },
      );
    let mut bytes = vec![];
    doc.save_to(&mut bytes).unwrap();
    let layers = Layers::read(&bytes).unwrap();
    assert!(layers.items.iter().all(|l| l.visible));
    assert!(layers.items[0].locked);
    let changed = Layers::read(&layers.with_states(&[false, false, false]).unwrap()).unwrap();
    assert!(changed.items.iter().all(|l| !l.visible));
  }

  #[test]
  fn plain_and_malformed_documents() {
    assert!(Layers::read(b"broken").is_err());
    let mut doc = Document::load_mem(&demo()).unwrap();
    let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
    doc
      .get_object_mut(root)
      .unwrap()
      .as_dict_mut()
      .unwrap()
      .remove(b"OCProperties");
    let mut bytes = vec![];
    doc.save_to(&mut bytes).unwrap();
    assert!(Layers::read(&bytes).unwrap().items.is_empty());
  }

  #[test]
  fn actual_pdfium_renders_layers_pages_rotation_and_print_state() {
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let original = demo();
    let layers = Layers::read(&original).unwrap();
    let pdf = Pdf::open(api.clone(), Arc::new(original)).unwrap();
    assert_eq!(pdf.sizes, vec![(600., 800.); 3]);
    let full = pdf.render(0, 600, 800, 0, false).unwrap();
    let empty = Pdf::open(
      api.clone(),
      Arc::new(layers.with_states(&[false, false, false]).unwrap()),
    )
    .unwrap();
    let hidden = empty.render(0, 600, 800, 0, false).unwrap();
    assert_ne!(full.pixels, hidden.pixels);
    let pixel = (300 * 600 + 300) * 4;
    assert_ne!(&full.pixels[pixel..pixel + 3], &[255, 255, 255]);
    assert_eq!(&hidden.pixels[pixel..pixel + 3], &[255, 255, 255]);
    for states in [
      [true, false, false],
      [false, true, false],
      [false, false, true],
    ] {
      let pdf = Pdf::open(api.clone(), Arc::new(layers.with_states(&states).unwrap())).unwrap();
      let image = pdf.render(0, 600, 800, 0, false).unwrap();
      assert_ne!(image.pixels, hidden.pixels);
      assert_eq!(
        image.pixels,
        pdf.render(0, 600, 800, 0, true).unwrap().pixels
      );
    }
    assert_ne!(
      pdf.render(1, 600, 800, 0, false).unwrap().pixels,
      full.pixels
    );
    let rotated = pdf.render(0, 800, 600, 1, false).unwrap();
    assert_eq!((rotated.width, rotated.height), (800, 600));
    assert!(pdf.render(3, 600, 800, 0, false).is_err());
    assert!(pdf.render(0, 10_000, 10_000, 0, false).is_err());
    assert!(Pdf::open(api, Arc::new(b"not a pdf".to_vec())).is_err());
  }
}

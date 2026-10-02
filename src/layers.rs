use lopdf::{dictionary, Dictionary, Document, Object, ObjectId};

#[derive(Clone, Debug)]
pub struct Layer {
  pub id: ObjectId,
  pub name: String,
  pub visible: bool,
  pub locked: bool,
}

pub struct Layers {
  document: Document,
  pub items: Vec<Layer>,
  pub radio_groups: Vec<Vec<ObjectId>>,
}

fn direct<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a Object> {
  doc.dereference(object).ok().map(|(_, value)| value)
}

fn dict<'a>(doc: &'a Document, parent: &'a Dictionary, key: &[u8]) -> Option<&'a Dictionary> {
  direct(doc, parent.get(key).ok()?)?.as_dict().ok()
}

fn refs(doc: &Document, parent: &Dictionary, key: &[u8]) -> Vec<ObjectId> {
  parent
    .get(key)
    .ok()
    .and_then(|o| direct(doc, o))
    .and_then(|o| o.as_array().ok())
    .map(|a| a.iter().filter_map(|v| v.as_reference().ok()).collect())
    .unwrap_or_default()
}

fn has_view_intent(doc: &Document, config: &Dictionary) -> bool {
  let Some(intent) = config.get(b"Intent").ok().and_then(|o| direct(doc, o)) else {
    return false;
  };
  match intent {
    Object::Name(n) => n == b"View" || n == b"All",
    Object::Array(a) => a
      .iter()
      .any(|o| matches!(o, Object::Name(n) if n == b"View" || n == b"All")),
    _ => false,
  }
}

impl Layers {
  pub fn read(bytes: &[u8]) -> Result<Self, String> {
    let document =
      Document::load_mem(bytes).map_err(|e| format!("Не удалось прочитать слои: {e}"))?;
    let mut items = vec![];
    let mut radio_groups = vec![];
    let root = document.catalog().map_err(|e| e.to_string())?;
    if let Some(props) = dict(&document, root, b"OCProperties") {
      let configs = props
        .get(b"Configs")
        .ok()
        .and_then(|o| direct(&document, o))
        .and_then(|o| o.as_array().ok());
      let config = configs
        .and_then(|a| {
          a.iter()
            .filter_map(|o| direct(&document, o)?.as_dict().ok())
            .find(|d| has_view_intent(&document, d))
        })
        .or_else(|| dict(&document, props, b"D"));
      let empty = Dictionary::new();
      let config = config.unwrap_or(&empty);
      let on = refs(&document, config, b"ON");
      let off = refs(&document, config, b"OFF");
      let locked = refs(&document, config, b"Locked");
      let base_on = config
        .get(b"BaseState")
        .and_then(Object::as_name)
        .unwrap_or(b"ON")
        != b"OFF";
      if let Some(groups) = config
        .get(b"RBGroups")
        .ok()
        .and_then(|o| direct(&document, o))
        .and_then(|o| o.as_array().ok())
      {
        for group in groups {
          if let Some(a) = direct(&document, group).and_then(|o| o.as_array().ok()) {
            radio_groups.push(a.iter().filter_map(|o| o.as_reference().ok()).collect());
          }
        }
      }
      for id in refs(&document, props, b"OCGs") {
        if items.iter().any(|l: &Layer| l.id == id) {
          continue;
        }
        let d = document.get_dictionary(id).map_err(|e| e.to_string())?;
        let name = d
          .get(b"Name")
          .ok()
          .and_then(|o| direct(&document, o))
          .and_then(|o| o.as_str().ok())
          .map(decode_pdf_string)
          .unwrap_or_else(|| format!("Слой {}", items.len() + 1));
        let mut visible = (base_on || on.contains(&id)) && !off.contains(&id);
        if let Some(auto) = config
          .get(b"AS")
          .ok()
          .and_then(|o| direct(&document, o))
          .and_then(|o| o.as_array().ok())
        {
          for entry in auto {
            let Some(usage) = direct(&document, entry).and_then(|o| o.as_dict().ok()) else {
              continue;
            };
            if usage
              .get(b"Event")
              .and_then(Object::as_name)
              .unwrap_or(b"View")
              == b"View"
              && refs(&document, usage, b"OCGs").contains(&id)
            {
              if let Some(view) = dict(&document, usage, b"View") {
                visible = view
                  .get(b"ViewState")
                  .and_then(Object::as_name)
                  .unwrap_or(b"")
                  != b"OFF";
              }
            }
          }
        }
        if let Some(state) = dict(&document, d, b"Usage").and_then(|u| dict(&document, u, b"View"))
        {
          if let Ok(n) = state.get(b"ViewState").and_then(Object::as_name) {
            visible = n != b"OFF";
          }
        }
        if d.has(b"Intent") && !has_view_intent(&document, d) {
          visible = true;
        }
        items.push(Layer {
          id,
          name,
          visible,
          locked: locked.contains(&id),
        });
      }
    }
    Ok(Self {
      document,
      items,
      radio_groups,
    })
  }

  pub fn apply_toggle(
    items: &[Layer],
    groups: &[Vec<ObjectId>],
    states: &mut [bool],
    index: usize,
  ) {
    if index >= items.len() || states.len() != items.len() || items[index].locked {
      return;
    }
    let next = !states[index];
    if next {
      for group in groups.iter().filter(|g| g.contains(&items[index].id)) {
        if items
          .iter()
          .enumerate()
          .any(|(i, l)| i != index && group.contains(&l.id) && l.locked && states[i])
        {
          return;
        }
      }
      for group in groups.iter().filter(|g| g.contains(&items[index].id)) {
        for (i, l) in items.iter().enumerate() {
          if i != index && group.contains(&l.id) {
            states[i] = false;
          }
        }
      }
    }
    states[index] = next;
  }

  pub fn with_states(&self, states: &[bool]) -> Result<Vec<u8>, String> {
    if states.len() != self.items.len() {
      return Err("Неверное количество состояний слоёв.".into());
    }
    let mut doc = self.document.clone();
    let root_id = doc
      .trailer
      .get(b"Root")
      .and_then(Object::as_reference)
      .map_err(|e| e.to_string())?;
    let mut props = dict(
      &doc,
      doc.catalog().map_err(|e| e.to_string())?,
      b"OCProperties",
    )
    .cloned()
    .unwrap_or_default();
    let on: Vec<Object> = self
      .items
      .iter()
      .zip(states)
      .filter(|(_, b)| **b)
      .map(|(l, _)| Object::Reference(l.id))
      .collect();
    let off: Vec<Object> = self
      .items
      .iter()
      .zip(states)
      .filter(|(_, b)| !**b)
      .map(|(l, _)| Object::Reference(l.id))
      .collect();
    props.remove(b"Configs");
    props.set(
      "D",
      dictionary! { "BaseState" => "ON", "ON" => on, "OFF" => off, "Intent" => "View" },
    );
    doc
      .get_object_mut(root_id)
      .and_then(Object::as_dict_mut)
      .map_err(|e| e.to_string())?
      .set("OCProperties", props);
    for (layer, state) in self.items.iter().zip(states) {
      let value = if *state { "ON" } else { "OFF" };
      let d = doc
        .get_object_mut(layer.id)
        .and_then(Object::as_dict_mut)
        .map_err(|e| e.to_string())?;
      d.set("Intent", "View");
      // Явный выбор пользователя одинаков для экрана и печати; исходный файл не меняется.
      d.set(
        "Usage",
        dictionary! {
          "View" => dictionary! { "ViewState" => value },
          "Print" => dictionary! { "PrintState" => value },
          "Export" => dictionary! { "ExportState" => value }
        },
      );
    }
    let mut bytes = vec![];
    doc.save_to(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
  }
}

fn decode_pdf_string(bytes: &[u8]) -> String {
  if bytes.starts_with(&[0xfe, 0xff]) {
    let units: Vec<u16> = bytes[2..]
      .as_chunks::<2>()
      .0
      .iter()
      .map(|c| u16::from_be_bytes([c[0], c[1]]))
      .collect();
    String::from_utf16_lossy(&units)
  } else if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
    String::from_utf8_lossy(&bytes[3..]).into_owned()
  } else {
    lopdf::decode_text_string(&Object::String(
      bytes.to_vec(),
      lopdf::StringFormat::Literal,
    ))
    .unwrap_or_else(|_| String::from_utf8_lossy(bytes).into_owned())
  }
}

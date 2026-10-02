use crate::{
  editing::{Mask, Operation, PageObject},
  engine::Client,
  export, vault,
};
use std::path::Path;

pub fn run(directory: &Path) -> Result<(), String> {
  std::fs::create_dir(directory).map_err(|e| e.to_string())?;
  let source = directory.join("source.pdf");
  let original = crate::fixture::demo();
  export::write(&source, &original)?;
  let mut client = Client::spawn(&crate::pdfium_path())?;
  let meta = client.open(&source, || false)?;
  let objects: Vec<PageObject> =
    serde_json::from_slice(&client.edit(Operation::Objects { page: 0 }, || false)?)
      .map_err(|e| e.to_string())?;
  let selected = objects
    .iter()
    .find(|o| o.kind == 1 && !o.text.is_empty())
    .ok_or("Не найден текст образца.")?;
  export::write(
    &directory.join("objects.json"),
    &serde_json::to_vec_pretty(&objects).map_err(|e| e.to_string())?,
  )?;
  export::write(
    &directory.join("pages-1-3.pdf"),
    &client.edit(Operation::Extract { pages: vec![0, 2] }, || false)?,
  )?;
  export::write(
    &directory.join("edited.pdf"),
    &client.edit(
      Operation::Text {
        page: 0,
        object: selected.index,
        text: "Проверка текста 123".into(),
      },
      || false,
    )?,
  )?;
  export::write(
    &directory.join("deleted.pdf"),
    &client.edit(
      Operation::Delete {
        page: 0,
        object: selected.index,
      },
      || false,
    )?,
  )?;
  let bounds = selected.bounds;
  let masks = vec![Mask {
    page: 0,
    bounds: [
      (bounds[0] - 0.01).max(0.),
      (bounds[1] - 0.01).max(0.),
      (bounds[2] + 0.01).min(1.),
      (bounds[3] + 0.01).min(1.),
    ],
  }];
  let states: Vec<_> = meta.layers.iter().map(|l| l.visible).collect();
  export::clean_pdf(
    |key| client.render(key, false, || false),
    &meta.sizes,
    &states,
    &masks,
    &directory.join("clean.pdf"),
    || false,
    |_, _| {},
  )?;
  let key = vault::key_file()?;
  let encrypted = vault::encrypt(&original, &key)?;
  let restored = vault::decrypt(&encrypted, &key)?;
  if *restored != original {
    return Err("Восстановленный оригинал отличается.".into());
  }
  export::write(&directory.join("fixture.astravault"), &encrypted)?;
  export::write(&directory.join("fixture.astrakey"), &key)?;
  export::write(&directory.join("restored.pdf"), &restored)?;
  if export::read(&source, vault::LIMIT)? != original {
    return Err("Исходник изменился.".into());
  }
  export::write(&directory.join("result.json"), b"{\"ok\":true}")
}

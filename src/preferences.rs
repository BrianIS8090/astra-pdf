use crate::{export, model::Zoom, recovery};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Recent {
  pub path: PathBuf,
  pub hash: [u8; 32],
  pub page: usize,
  pub zoom: Zoom,
  pub rotation: i32,
  pub continuous: bool,
  pub detail: bool,
}

fn path() -> Option<PathBuf> {
  Some(
    PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
      .join("AstraPDF")
      .join("reading.dat"),
  )
}

pub fn read() -> Vec<Recent> {
  path().and_then(|p| read_at(&p).ok()).unwrap_or_default()
}

fn read_at(path: &Path) -> Result<Vec<Recent>, String> {
  if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
    return Err("Недопустимый размер списка документов.".into());
  }
  let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
  let items: Vec<Recent> =
    serde_json::from_slice(&recovery::protect(&bytes, true)?).map_err(|e| e.to_string())?;
  Ok(
    items
      .into_iter()
      .filter(|r| {
        r.path.is_absolute()
          && r.page < 100_000
          && (0..4).contains(&r.rotation)
          && match r.zoom {
            Zoom::Scale(s) => s.is_finite() && (0.01..=64.).contains(&s),
            _ => true,
          }
      })
      .take(12)
      .collect(),
  )
}

pub fn remember(items: &mut Vec<Recent>, recent: Recent) -> bool {
  if items.first() == Some(&recent) {
    return false;
  }
  items.retain(|r| r.path != recent.path);
  items.insert(0, recent);
  items.truncate(12);
  true
}

pub fn save(items: &[Recent]) -> Result<(), String> {
  let path = path().ok_or("Не найден личный каталог Windows.")?;
  std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
  save_at(&path, items)
}

fn save_at(path: &Path, items: &[Recent]) -> Result<(), String> {
  let bytes = serde_json::to_vec(items).map_err(|e| e.to_string())?;
  export::write(path, &recovery::protect(&bytes, false)?)
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn positions_are_private_bounded_and_latest_first() {
    let path = std::env::temp_dir().join(format!("astra-preferences-{}.dat", std::process::id()));
    let mut items = Vec::new();
    for i in 0..20 {
      remember(
        &mut items,
        Recent {
          path: path.with_extension(format!("{i}.pdf")),
          hash: [i as u8; 32],
          page: i,
          zoom: Zoom::Scale(2.),
          rotation: 1,
          continuous: true,
          detail: true,
        },
      );
    }
    assert_eq!(items.len(), 12);
    let mut changed = items[2].clone();
    changed.page = 400;
    assert!(remember(&mut items, changed.clone()));
    assert!(!remember(&mut items, changed));
    save_at(&path, &items).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("pdf"));
    let restored = read_at(&path).unwrap();
    assert!(restored == items);
    std::fs::remove_file(path).unwrap();
  }
}

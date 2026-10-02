use crate::{export, vault};
use sha2::{Digest, Sha256};
use std::{
  path::{Path, PathBuf},
  sync::Arc,
};

// Снимки принадлежат сеансу: пользовательские PDF создаются только при сохранении.
pub struct Revision {
  pub path: PathBuf,
  pub hash: [u8; 32],
  pub size: usize,
}
impl Revision {
  pub fn new(bytes: &[u8]) -> Result<Arc<Self>, String> {
    let root =
      PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("Не найден каталог пользователя.")?)
        .join("AstraPDF")
        .join("Sessions");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let token = vault::random::<16>()?.map(|b| format!("{b:02x}")).join("");
    let path = root.join(format!("{token}.pdf"));
    export::write(&path, bytes)?;
    Ok(Arc::new(Self {
      path,
      hash: Sha256::digest(bytes).into(),
      size: bytes.len(),
    }))
  }
}
impl Drop for Revision {
  fn drop(&mut self) {
    let _ = std::fs::remove_file(&self.path);
  }
}

pub struct Session {
  pub current: Arc<Revision>,
  pub saved_hash: [u8; 32],
}
impl Session {
  pub fn dirty(&self) -> bool {
    self.current.hash != self.saved_hash
  }
}

pub fn hash_file(path: &Path) -> Result<[u8; 32], String> {
  use std::io::Read;
  let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
  let mut hash = Sha256::new();
  let mut buf = [0u8; 65536];
  loop {
    let len = file.read(&mut buf).map_err(|e| e.to_string())?;
    if len == 0 {
      return Ok(hash.finalize().into());
    }
    hash.update(&buf[..len]);
  }
}

pub fn save(
  source: &Path,
  destination: &Path,
  expected: Option<[u8; 32]>,
  cancelled: impl Fn() -> bool,
) -> Result<[u8; 32], String> {
  use std::io::{Read, Write};
  let mut input = std::fs::File::open(source).map_err(|e| e.to_string())?;
  let mut output = export::AtomicFile::new(destination)?;
  let mut hash = Sha256::new();
  let mut buf = [0u8; 65536];
  loop {
    if cancelled() {
      return Err("Сохранение отменено.".into());
    }
    let len = input.read(&mut buf).map_err(|e| e.to_string())?;
    if len == 0 {
      break;
    }
    output
      .file
      .write_all(&buf[..len])
      .map_err(|e| e.to_string())?;
    hash.update(&buf[..len]);
  }
  drop(input);
  if destination.exists() {
    if expected != Some(hash_file(destination)?) {
      return Err("Файл изменился вне программы. Используйте «Сохранить как», чтобы сохранить свои правки отдельно.".into());
    }
  } else if expected.is_some() {
    return Err("Исходный файл перемещён или удалён. Используйте «Сохранить как».".into());
  }
  if cancelled() {
    return Err("Сохранение отменено.".into());
  }
  output.finish()?;
  Ok(hash.finalize().into())
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn revisions_live_until_last_owner_and_save_preserves_undo() {
    let before = Revision::new(b"before").unwrap();
    let after = Revision::new(b"after").unwrap();
    let target = Revision::new(b"before").unwrap();
    let mut session = Session {
      current: after.clone(),
      saved_hash: before.hash,
    };
    assert!(session.dirty());
    session.saved_hash = save(&after.path, &target.path, Some(before.hash), || false).unwrap();
    assert!(!session.dirty());
    session.current = before.clone();
    assert!(session.dirty());
    assert_eq!(std::fs::read(&target.path).unwrap(), b"after");
    let path = after.path.clone();
    drop(after);
    assert!(!path.exists());
  }
  #[test]
  fn conflict_cancel_and_missing_source_do_not_replace_destination() {
    let source = Revision::new(b"new").unwrap();
    let target = Revision::new(b"old").unwrap();
    assert!(save(&source.path, &target.path, Some([0; 32]), || false).is_err());
    assert!(save(&source.path, &target.path, Some(target.hash), || true).is_err());
    assert!(save(
      Path::new("missing-astra-source.pdf"),
      &target.path,
      Some(target.hash),
      || false
    )
    .is_err());
    assert_eq!(std::fs::read(&target.path).unwrap(), b"old");
  }
}

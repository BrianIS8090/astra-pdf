use crate::{editing::Mask, export, model::Zoom, session::Revision, vault};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
  fs::{File, OpenOptions},
  os::windows::fs::OpenOptionsExt,
  path::{Path, PathBuf},
  sync::{Arc, Condvar, Mutex},
  thread::JoinHandle,
};
use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::*};
use zeroize::{Zeroize, Zeroizing};

const META_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
  pub path: PathBuf,
  pub saved_hash: [u8; 32],
  pub current_hash: [u8; 32],
  pub masks: Vec<Mask>,
  pub states: Vec<bool>,
  pub page: usize,
  pub pages: usize,
  pub zoom: Zoom,
  pub rotation: i32,
  pub continuous: bool,
  pub detail_mode: bool,
  pub pixel_mm: u8,
}

#[derive(Serialize, Deserialize)]
struct Stored {
  version: u32,
  metadata: Metadata,
  key: Vec<u8>,
}
impl Drop for Stored {
  fn drop(&mut self) {
    self.key.zeroize();
  }
}

pub struct Snapshot {
  pub metadata: Metadata,
  pub source: PathBuf,
  pub revision: Option<Arc<Revision>>,
}

pub struct Entry {
  directory: PathBuf,
  lock: File,
  stored: Stored,
}

pub struct Recovered {
  pub entry: Entry,
  pub revision: Arc<Revision>,
}

#[derive(Default)]
struct State {
  pending: Option<(u64, Snapshot)>,
  last: Option<Metadata>,
  scheduled: u64,
  saved: u64,
  error: Option<String>,
  stop: bool,
}

#[derive(Default)]
struct Shared {
  state: Mutex<State>,
  ready: Condvar,
}

pub struct Writer {
  shared: Arc<Shared>,
  thread: Option<JoinHandle<()>>,
}

fn root() -> Result<PathBuf, String> {
  Ok(
    PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("Не найден личный каталог Windows.")?)
      .join("AstraPDF")
      .join("Recovery"),
  )
}

fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn data_path(directory: &Path, hash: &[u8; 32]) -> PathBuf {
  directory.join(format!("{}.astravault", hex(hash)))
}

// DPAPI связывает служебные данные и ключ с текущей учётной записью Windows.
pub fn protect(bytes: &[u8], decrypt: bool) -> Result<Zeroizing<Vec<u8>>, String> {
  if bytes.is_empty() || bytes.len() > META_LIMIT {
    return Err("Недопустимый размер данных восстановления.".into());
  }
  let input = CRYPT_INTEGER_BLOB {
    cbData: bytes.len() as u32,
    pbData: bytes.as_ptr() as _,
  };
  let mut output: CRYPT_INTEGER_BLOB = unsafe { std::mem::zeroed() };
  let ok = unsafe {
    if decrypt {
      CryptUnprotectData(
        &input,
        std::ptr::null_mut(),
        std::ptr::null(),
        std::ptr::null(),
        std::ptr::null(),
        CRYPTPROTECT_UI_FORBIDDEN,
        &mut output,
      )
    } else {
      CryptProtectData(
        &input,
        std::ptr::null(),
        std::ptr::null(),
        std::ptr::null(),
        std::ptr::null(),
        CRYPTPROTECT_UI_FORBIDDEN,
        &mut output,
      )
    }
  };
  if ok == 0 {
    return Err(format!(
      "Windows не смогла {} данные восстановления: {}",
      if decrypt {
        "расшифровать"
      } else {
        "защитить"
      },
      std::io::Error::last_os_error()
    ));
  }
  let result = unsafe {
    Zeroizing::new(std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec())
  };
  unsafe {
    std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
    LocalFree(output.pbData.cast());
  }
  Ok(result)
}

fn lock(directory: &Path) -> Result<File, String> {
  OpenOptions::new()
    .read(true)
    .write(true)
    .create(true)
    .truncate(false)
    .share_mode(0)
    .open(directory.join("session.lock"))
    .map_err(|e| e.to_string())
}

fn valid(metadata: &Metadata) -> bool {
  !metadata.path.as_os_str().is_empty()
    && metadata.path.is_absolute()
    && (1..=100_000).contains(&metadata.pages)
    && metadata.page < metadata.pages
    && metadata.states.len() <= 100_000
    && metadata.masks.len() <= 10_000
    && metadata.masks.iter().all(|m| m.valid(metadata.pages))
    && (0..4).contains(&metadata.rotation)
    && [0, 3, 6, 12].contains(&metadata.pixel_mm)
    && match metadata.zoom {
      Zoom::Scale(s) => s.is_finite() && (0.1..=64.).contains(&s),
      _ => true,
    }
}

// Удаляем только файлы принадлежащего нам каталога; по вложенным каталогам не переходим.
fn clean(directory: &Path) {
  if let Ok(entries) = std::fs::read_dir(directory) {
    for file in entries.flatten() {
      if file.file_type().is_ok_and(|t| t.is_file()) {
        let _ = std::fs::remove_file(file.path());
      }
    }
  }
  let _ = std::fs::remove_dir(directory);
}

impl Entry {
  pub fn metadata(&self) -> &Metadata {
    &self.stored.metadata
  }

  pub fn recover(self) -> Result<Recovered, String> {
    let encrypted = export::read(
      &data_path(&self.directory, &self.stored.metadata.current_hash),
      vault::LIMIT + 48,
    )?;
    let bytes = vault::decrypt(&encrypted, &self.stored.key)?;
    let revision = Revision::new(&bytes)?;
    if revision.hash != self.stored.metadata.current_hash {
      return Err("Рабочая копия повреждена. Исходный документ не изменён.".into());
    }
    Ok(Recovered {
      entry: self,
      revision,
    })
  }

  pub fn discard(self) {
    let Self {
      directory, lock, ..
    } = self;
    drop(lock);
    clean(&directory);
  }
}

pub fn pending() -> Result<Vec<Entry>, String> {
  pending_in(&root()?)
}

fn pending_in(root: &Path) -> Result<Vec<Entry>, String> {
  if !root.exists() {
    return Ok(Vec::new());
  }
  let mut directories = std::fs::read_dir(root)
    .map_err(|e| e.to_string())?
    .flatten()
    .collect::<Vec<_>>();
  directories
    .sort_by_key(|entry| std::cmp::Reverse(entry.metadata().and_then(|m| m.modified()).ok()));
  let mut result = Vec::new();
  for directory in directories.into_iter().take(100) {
    let name = directory.file_name();
    let name = name.to_string_lossy();
    if name.len() != 32
      || !name.bytes().all(|b| b.is_ascii_hexdigit())
      || !directory.file_type().is_ok_and(|t| t.is_dir())
    {
      continue;
    }
    let directory = directory.path();
    let Ok(lock) = lock(&directory) else {
      continue;
    };
    let manifest = directory.join("state.dat");
    if !manifest.exists() {
      continue;
    }
    let Ok(bytes) = export::read(&manifest, META_LIMIT) else {
      continue;
    };
    let Ok(decoded) = protect(&bytes, true) else {
      continue;
    };
    let Ok(stored) = serde_json::from_slice::<Stored>(&decoded) else {
      continue;
    };
    if stored.version != 1 || stored.key.len() != 40 || !valid(&stored.metadata) {
      continue;
    }
    result.push(Entry {
      directory,
      lock,
      stored,
    });
  }
  Ok(result)
}

impl Writer {
  pub fn new() -> Result<Self, String> {
    Self::new_in(&root()?)
  }

  fn new_in(root: &Path) -> Result<Self, String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let directory = root.join(hex(&vault::random::<16>()?));
    std::fs::create_dir(&directory).map_err(|e| e.to_string())?;
    let lock = lock(&directory)?;
    Ok(Self::start(directory, lock, vault::key_file()?, None))
  }

  pub fn resume(entry: Entry) -> Self {
    let Entry {
      directory,
      lock,
      mut stored,
    } = entry;
    Self::start(
      directory,
      lock,
      Zeroizing::new(std::mem::take(&mut stored.key)),
      Some(stored.metadata.current_hash),
    )
  }

  fn start(
    directory: PathBuf,
    lock: File,
    key: Zeroizing<Vec<u8>>,
    mut cached: Option<[u8; 32]>,
  ) -> Self {
    let shared = Arc::new(Shared::default());
    let worker = shared.clone();
    let thread = std::thread::spawn(move || {
      loop {
        let (serial, snapshot) = {
          let mut state = worker.state.lock().unwrap();
          while state.pending.is_none() && !state.stop {
            state = worker.ready.wait(state).unwrap();
          }
          if state.stop {
            break;
          }
          state.pending.take().unwrap()
        };
        let result = write_checkpoint(&directory, &key, &snapshot, cached);
        let mut state = worker.state.lock().unwrap();
        match result {
          Ok(()) => {
            cached = Some(snapshot.metadata.current_hash);
            state.saved = serial;
            state.error = None;
          }
          Err(error) => {
            state.error = Some(error);
            state.last = None;
          }
        }
      }
      drop(lock);
      clean(&directory);
    });
    Self {
      shared,
      thread: Some(thread),
    }
  }

  pub fn submit(&self, snapshot: Snapshot) {
    let mut state = self.shared.state.lock().unwrap();
    if state.last.as_ref() == Some(&snapshot.metadata) {
      return;
    }
    state.last = Some(snapshot.metadata.clone());
    state.scheduled += 1;
    state.pending = Some((state.scheduled, snapshot));
    self.shared.ready.notify_one();
  }

  pub fn status(&self) -> (bool, Option<String>) {
    let state = self.shared.state.lock().unwrap();
    (
      state.saved > 0 && state.saved == state.scheduled,
      state.error.clone(),
    )
  }
}

impl Drop for Writer {
  fn drop(&mut self) {
    self.shared.state.lock().unwrap().stop = true;
    self.shared.ready.notify_one();
    if let Some(thread) = self.thread.take() {
      let _ = thread.join();
    }
  }
}

fn write_checkpoint(
  directory: &Path,
  key: &[u8],
  snapshot: &Snapshot,
  cached: Option<[u8; 32]>,
) -> Result<(), String> {
  if !valid(&snapshot.metadata) {
    return Err("Недопустимые данные сеанса восстановления.".into());
  }
  // Сильная ссылка удерживает снимок PDF, пока фоновая запись не закончена.
  let _revision = &snapshot.revision;
  let target = data_path(directory, &snapshot.metadata.current_hash);
  if cached != Some(snapshot.metadata.current_hash) {
    let bytes = Zeroizing::new(export::read(&snapshot.source, vault::LIMIT)?);
    if <[u8; 32]>::from(Sha256::digest(&*bytes)) != snapshot.metadata.current_hash {
      return Err("PDF изменился на диске; рабочая копия восстановления не обновлена.".into());
    }
    let encrypted = vault::encrypt(&bytes, key)?;
    export::write(&target, &encrypted)?;
  }
  let stored = Stored {
    version: 1,
    metadata: snapshot.metadata.clone(),
    key: key.to_vec(),
  };
  let bytes = Zeroizing::new(serde_json::to_vec(&stored).map_err(|e| e.to_string())?);
  let encrypted = protect(&bytes, false)?;
  export::write(&directory.join("state.dat"), &encrypted)?;
  if let Some(old) = cached.filter(|h| *h != snapshot.metadata.current_hash) {
    let _ = std::fs::remove_file(data_path(directory, &old));
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn windows_protection_roundtrip_and_tamper() {
    let secret = b"path and private recovery key";
    let mut encrypted = protect(secret, false).unwrap();
    assert!(!encrypted.windows(secret.len()).any(|s| s == secret));
    assert_eq!(&**protect(&encrypted, true).unwrap(), secret);
    let index = encrypted.len() - 1;
    encrypted[index] ^= 0x80;
    assert!(protect(&encrypted, true).is_err());
  }

  #[test]
  fn checkpoint_is_encrypted_locked_and_recovers_masks_and_document() {
    let root = std::env::temp_dir().join(format!(
      "astra-recovery-test-{}",
      hex(&vault::random::<16>().unwrap())
    ));
    let revision = Revision::new(b"%PDF-1.7\nprivate edited content").unwrap();
    let metadata = Metadata {
      path: revision.path.clone(),
      saved_hash: [0; 32],
      current_hash: revision.hash,
      masks: vec![Mask {
        page: 0,
        bounds: [0.1, 0.2, 0.3, 0.4],
        kind: crate::editing::MaskKind::Cover,
      }],
      states: vec![true, false],
      page: 0,
      pages: 1,
      zoom: Zoom::Scale(2.),
      rotation: 1,
      continuous: true,
      detail_mode: true,
      pixel_mm: 6,
    };
    let directory = root.join(hex(&vault::random::<16>().unwrap()));
    std::fs::create_dir_all(&directory).unwrap();
    let lock = lock(&directory).unwrap();
    let snapshot = Snapshot {
      metadata: metadata.clone(),
      source: revision.path.clone(),
      revision: Some(revision.clone()),
    };
    write_checkpoint(&directory, &vault::key_file().unwrap(), &snapshot, None).unwrap();
    assert!(pending_in(&root).unwrap().is_empty());
    drop(lock);
    let corrupt = root.join("00000000000000000000000000000000");
    std::fs::create_dir(&corrupt).unwrap();
    std::fs::write(corrupt.join("state.dat"), b"damaged checkpoint").unwrap();
    let mut entries = pending_in(&root).unwrap();
    assert_eq!(entries.len(), 1);
    let restored = entries.pop().unwrap().recover().unwrap();
    assert_eq!(restored.entry.metadata().masks, metadata.masks);
    assert_eq!(restored.revision.hash, revision.hash);
    let writer = Writer::resume(restored.entry);
    writer.submit(snapshot);
    for _ in 0..100 {
      if writer.status().0 {
        break;
      }
      std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(writer.status().0);
    assert!(pending_in(&root).unwrap().is_empty());
    drop(writer);
    assert!(!directory.exists());
    assert!(corrupt.join("state.dat").exists());
    std::fs::remove_dir_all(corrupt).unwrap();
    std::fs::remove_dir(root).unwrap();
  }
}

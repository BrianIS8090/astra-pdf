use chacha20poly1305::{
  aead::{rand_core::RngCore, AeadInPlace, KeyInit, OsRng},
  XChaCha20Poly1305, XNonce,
};
use zeroize::Zeroizing;

const MAGIC: &[u8; 8] = b"ASTRAV01";
const KEY_MAGIC: &[u8; 8] = b"ASTRAK01";
pub const LIMIT: usize = 512 * 1024 * 1024;

pub fn random<const N: usize>() -> Result<[u8; N], String> {
  let mut bytes = [0; N];
  OsRng
    .try_fill_bytes(&mut bytes)
    .map_err(|_| "Система не смогла создать случайный ключ.")?;
  Ok(bytes)
}

pub fn key_file() -> Result<Zeroizing<Vec<u8>>, String> {
  let mut file = Zeroizing::new(KEY_MAGIC.to_vec());
  file.extend_from_slice(&*Zeroizing::new(random::<32>()?));
  Ok(file)
}

fn cipher(key: &[u8]) -> Result<XChaCha20Poly1305, String> {
  if key.len() != 40 || &key[..8] != KEY_MAGIC {
    return Err("Неверный формат файла ключа.".into());
  }
  XChaCha20Poly1305::new_from_slice(&key[8..]).map_err(|_| "Неверная длина ключа.".into())
}

pub fn encrypt(pdf: &[u8], key: &[u8]) -> Result<Vec<u8>, String> {
  if pdf.len() > LIMIT || !pdf.starts_with(b"%PDF-") {
    return Err("Неверный оригинал PDF или превышен предел 512 МиБ.".into());
  }
  let nonce = random::<24>()?;
  let mut data = Zeroizing::new(pdf.to_vec());
  cipher(key)?
    .encrypt_in_place(XNonce::from_slice(&nonce), MAGIC, &mut *data)
    .map_err(|_| "Не удалось зашифровать оригинал.")?;
  let mut result = MAGIC.to_vec();
  result.extend_from_slice(&nonce);
  result.extend_from_slice(&data);
  Ok(result)
}

pub fn decrypt(file: &[u8], key: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
  if !(48..=LIMIT + 48).contains(&file.len()) || &file[..8] != MAGIC {
    return Err("Неверный формат защищённого оригинала.".into());
  }
  let mut data = Zeroizing::new(file[32..].to_vec());
  cipher(key)?
    .decrypt_in_place(XNonce::from_slice(&file[8..32]), MAGIC, &mut *data)
    .map_err(|_| "Ключ не подходит или защищённый файл повреждён. Ничего не восстановлено.")?;
  if !data.starts_with(b"%PDF-") {
    return Err("Защищённый файл не содержит PDF.".into());
  }
  Ok(data)
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn authenticated_roundtrip_wrong_key_tamper_and_unique_nonce() {
    let key = key_file().unwrap();
    let pdf = b"%PDF-1.7\nPrivate secret";
    let encrypted = encrypt(pdf, &key).unwrap();
    assert_eq!(&**decrypt(&encrypted, &key).unwrap(), pdf);
    assert_ne!(encrypted, encrypt(pdf, &key).unwrap());
    assert!(decrypt(&encrypted, &key_file().unwrap()).is_err());
    for index in [0, 8, 31, 32, encrypted.len() - 1] {
      let mut changed = encrypted.clone();
      changed[index] ^= 1;
      assert!(decrypt(&changed, &key).is_err());
    }
    for n in 0..encrypted.len() {
      assert!(decrypt(&encrypted[..n], &key).is_err());
    }
    assert!(encrypt(pdf, b"hardcoded password").is_err());
  }
}

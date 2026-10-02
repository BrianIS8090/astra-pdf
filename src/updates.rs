use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{ffi::c_void, path::PathBuf, ptr};
use windows_sys::Win32::Networking::WinHttp::*;

const REPOSITORY: &str = "https://github.com/BrianIS8090/astra-pdf";
#[derive(Clone, Deserialize)]
pub struct Release {
  pub tag_name: String,
  pub html_url: String,
  pub body: Option<String>,
  pub draft: bool,
  pub prerelease: bool,
  pub published_at: Option<String>,
  pub assets: Vec<Asset>,
}
#[derive(Clone, Deserialize)]
pub struct Asset {
  pub name: String,
  pub browser_download_url: String,
  pub digest: Option<String>,
  pub size: u64,
}
struct Internet(*mut c_void);
impl Drop for Internet {
  fn drop(&mut self) {
    unsafe {
      WinHttpCloseHandle(self.0);
    }
  }
}
impl Internet {
  fn new(handle: *mut c_void) -> Result<Self, String> {
    if handle.is_null() {
      Err("Сеть недоступна. Проверьте подключение и настройки прокси Windows.".into())
    } else {
      Ok(Self(handle))
    }
  }
}
pub fn get(url: &str, limit: usize) -> Result<Vec<u8>, String> {
  let rest = url
    .strip_prefix("https://")
    .ok_or("Для обновлений требуется HTTPS.")?;
  let (host, path) = rest.split_once('/').ok_or("Неверный адрес обновления.")?;
  if !["api.github.com", "github.com"].contains(&host) || url.contains(['\r', '\n', '\\']) {
    return Err("Недопустимый источник обновления.".into());
  }
  unsafe {
    let wide = crate::printing::wide;
    let session = Internet::new(WinHttpOpen(
      wide("AstraPDF-Updater").as_ptr(),
      WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
      ptr::null(),
      ptr::null(),
      0,
    ))?;
    WinHttpSetTimeouts(session.0, 10000, 10000, 15000, 15000);
    let connection = Internet::new(WinHttpConnect(session.0, wide(host).as_ptr(), 443, 0))?;
    let request = Internet::new(WinHttpOpenRequest(
      connection.0,
      wide("GET").as_ptr(),
      wide(&format!("/{path}")).as_ptr(),
      ptr::null(),
      ptr::null(),
      ptr::null(),
      WINHTTP_FLAG_SECURE,
    ))?;
    let headers =
      wide("Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28\r\n");
    if WinHttpSendRequest(
      request.0,
      headers.as_ptr(),
      (headers.len() - 1) as u32,
      ptr::null(),
      0,
      0,
      0,
    ) == 0
      || WinHttpReceiveResponse(request.0, ptr::null_mut()) == 0
    {
      return Err("Не удалось получить обновление по защищённому соединению.".into());
    }
    let mut status = 0u32;
    let mut size = 4;
    if WinHttpQueryHeaders(
      request.0,
      WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
      ptr::null(),
      (&mut status as *mut u32).cast(),
      &mut size,
      ptr::null_mut(),
    ) == 0
      || status != 200
    {
      return Err(format!(
        "Сервер обновлений ответил HTTP {status}. Повторите проверку позже."
      ));
    }
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 65536];
    loop {
      let mut count = 0;
      if WinHttpReadData(
        request.0,
        buffer.as_mut_ptr().cast(),
        buffer.len() as u32,
        &mut count,
      ) == 0
      {
        return Err("Загрузка прервана.".into());
      }
      if count == 0 {
        break;
      }
      if count as usize > limit.saturating_sub(bytes.len()) {
        return Err("Превышен допустимый размер обновления.".into());
      }
      bytes.extend_from_slice(&buffer[..count as usize]);
    }
    Ok(bytes)
  }
}
fn version(value: &str) -> Option<([u32; 3], u32, u32)> {
  let value = value.strip_prefix('v').unwrap_or(value);
  let (core, pre) = value
    .split_once('-')
    .map_or((value, None), |(a, b)| (a, Some(b)));
  let parts: Vec<_> = core
    .split('.')
    .map(str::parse::<u32>)
    .collect::<Result<_, _>>()
    .ok()?;
  let core: [u32; 3] = parts.try_into().ok()?;
  let (stage, number) = if let Some(pre) = pre {
    let (stage, n) = pre.split_once('.')?;
    (
      match stage {
        "alpha" => 1,
        "beta" => 2,
        "rc" => 3,
        _ => return None,
      },
      n.parse().ok()?,
    )
  } else {
    (4, 0)
  };
  Some((core, stage, number))
}
pub fn choose(bytes: &[u8], current: &str) -> Result<Option<Release>, String> {
  let releases: Vec<Release> =
    serde_json::from_slice(bytes).map_err(|_| "Некорректный ответ сервера обновлений.")?;
  let current = version(current).ok_or("Неверная версия программы.")?;
  // Ошибочно названная ранняя серия 1.0.0-rc предшествует новой нумерации 0.x.
  let mut candidates: Vec<_> = releases
    .into_iter()
    .filter(|r| {
      !r.draft
        && r.published_at.is_some()
        && r
          .html_url
          .starts_with(&format!("{REPOSITORY}/releases/tag/"))
        && version(&r.tag_name).is_some_and(|v| {
          let historical = v.0 == [1, 0, 0] && v.1 == 3 && (1..=9).contains(&v.2);
          v > current && !(historical && current.0[0] == 0)
        })
    })
    .collect();
  candidates.sort_by_key(|r| version(&r.tag_name));
  Ok(candidates.pop())
}
pub fn check() -> Result<Option<Release>, String> {
  choose(
    &get(
      "https://api.github.com/repos/BrianIS8090/astra-pdf/releases?per_page=100",
      4 * 1024 * 1024,
    )?,
    crate::version::NUMBER,
  )
}
pub fn download(release: &Release) -> Result<PathBuf, String> {
  let version = release
    .tag_name
    .strip_prefix('v')
    .ok_or("Некорректный номер выпуска.")?;
  if self::version(version).is_none() {
    return Err("Некорректный номер выпуска.".into());
  }
  let name = format!("AstraPDF-{version}-setup-x64.exe");
  let asset = release
    .assets
    .iter()
    .find(|a| a.name == name)
    .ok_or("В выпуске ещё нет установщика Windows.")?;
  let expected_url = format!("{REPOSITORY}/releases/download/{}/{name}", release.tag_name);
  if asset.browser_download_url != expected_url || asset.size > 100 * 1024 * 1024 {
    return Err("Недопустимый установщик в выпуске.".into());
  }
  let hash=asset.digest.as_deref().and_then(|d|d.strip_prefix("sha256:")).filter(|s|s.len()==64 && s.bytes().all(|b|b.is_ascii_hexdigit())).ok_or("Сервер не предоставил контрольную сумму установщика. Используйте страницу выпуска на GitHub.")?;
  let bytes = get(&asset.browser_download_url, 100 * 1024 * 1024)?;
  let actual = Sha256::digest(&bytes)
    .iter()
    .map(|b| format!("{b:02x}"))
    .collect::<String>();
  if bytes.len() as u64 != asset.size || actual != hash.to_ascii_lowercase() {
    return Err("Контрольная сумма не совпала. Установщик не будет запущен.".into());
  }
  let directory =
    PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("Не найден личный каталог Windows.")?)
      .join("AstraPDF/Updates");
  std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
  let path = directory.join(name);
  crate::export::write(&path, &bytes)?;
  Ok(path)
}
#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  #[ignore = "Проверка требует соединения с GitHub"]
  fn live_github_tls_and_release_metadata() {
    let data = get(
      "https://api.github.com/repos/BrianIS8090/astra-pdf/releases?per_page=5",
      4 * 1024 * 1024,
    )
    .unwrap();
    let releases: Vec<Release> = serde_json::from_slice(&data).unwrap();
    assert!(!releases.is_empty());
    assert!(releases.iter().any(
      |r| r.assets.iter().any(|a| a.name.ends_with("setup-x64.exe")
        && a
          .digest
          .as_deref()
          .is_some_and(|h| h.starts_with("sha256:")))
    ));
  }
  fn release(v: &str) -> serde_json::Value {
    serde_json::json!({"tag_name":v,"html_url":format!("{REPOSITORY}/releases/tag/{v}"),"body":"Changes","draft":false,"prerelease":true,"published_at":"2026-10-02","assets":[]})
  }
  #[test]
  fn version_comparison_migration_drafts_and_foreign_urls() {
    let mut draft = release("v9.0.0");
    draft["draft"] = true.into();
    let mut foreign = release("v8.0.0");
    foreign["html_url"] = "https://example.org/evil".into();
    let data = serde_json::to_vec(&vec![
      release("v1.0.0-rc.9"),
      release("v0.10.0-alpha.2"),
      release("v0.10.0-alpha.10"),
      draft,
      foreign,
    ])
    .unwrap();
    assert_eq!(
      choose(&data, "0.10.0-alpha.1").unwrap().unwrap().tag_name,
      "v0.10.0-alpha.10"
    );
    assert!(choose(&data, "0.10.0-alpha.10").unwrap().is_none());
    let future = serde_json::to_vec(&vec![release("v1.0.0-rc.10")]).unwrap();
    assert_eq!(
      choose(&future, "0.10.0-alpha.1").unwrap().unwrap().tag_name,
      "v1.0.0-rc.10"
    );
    assert!(get("https://github.com.evil.test/x", 100).is_err());
  }
}

pub fn windows_version(version: &str) -> Result<[u16; 4], String> {
  let (base, preview) = version
    .split_once('-')
    .map_or((version, None), |(a, b)| (a, Some(b)));
  let parts: Vec<_> = base.split('.').collect();
  if parts.len() != 3 {
    return Err("Нужна версия major.minor.patch".into());
  }
  let mut output = [0, 0, 0, u16::MAX];
  for i in 0..3 {
    output[i] = parts[i]
      .parse()
      .map_err(|_| "Компонент версии должен быть от 0 до 65535")?;
  }
  if let Some(preview) = preview {
    let (kind, index) = preview
      .split_once('.')
      .ok_or("Ожидается alpha.N, beta.N или rc.N")?;
    let index: u16 = index
      .parse()
      .map_err(|_| "Неверный номер предварительного выпуска")?;
    if index == 0 || index >= 10_000 {
      return Err("Номер предварительного выпуска: 1–9999".into());
    }
    output[3] = match kind {
      "alpha" => 10_000,
      "beta" => 20_000,
      "rc" => 30_000,
      _ => return Err("Неизвестный тип предварительного выпуска".into()),
    } + index;
  }
  Ok(output)
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn windows_versions_increase_through_release_stages() {
    let versions = [
      "1.0.0-alpha.1",
      "1.0.0-beta.1",
      "1.0.0-rc.3",
      "1.0.0-rc.4",
      "1.0.0",
      "1.0.1-alpha.1",
    ];
    let numeric: Vec<_> = versions
      .into_iter()
      .map(|v| windows_version(v).unwrap())
      .collect();
    assert!(numeric.windows(2).all(|p| p[0] < p[1]));
    for invalid in [
      "1.0",
      "1.0.0-rc.0",
      "1.0.0-preview.1",
      "1.0.0-rc.10000",
      "65536.0.0",
    ] {
      assert!(windows_version(invalid).is_err());
    }
  }
}

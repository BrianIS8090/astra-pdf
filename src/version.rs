pub const NUMBER: &str = env!("CARGO_PKG_VERSION");
pub const CHANGES: &str = include_str!("../CHANGELOG.md");

pub fn title() -> String {
  if NUMBER.contains('-') {
    format!("Astra PDF {NUMBER} · предварительная версия")
  } else {
    format!("Astra PDF {NUMBER}")
  }
}

#[cfg(test)]
mod tests {
  #[test]
  fn release_identity_matches_embedded_history() {
    assert!(super::CHANGES.contains(&format!("## {}", super::NUMBER)));
    assert!(super::title().contains(super::NUMBER));
    assert!(super::title().contains("предварительная"));
  }
}

use std::{env, path::PathBuf, process::Command};
#[path = "src/version_number.rs"]
mod version_number;

fn main() {
  println!("cargo:rerun-if-changed=assets/app.rc");
  println!("cargo:rerun-if-changed=assets/app.manifest");
  println!("cargo:rerun-if-changed=assets/app.ico");
  println!("cargo:rerun-if-changed=src/version_number.rs");
  let version = env::var("CARGO_PKG_VERSION").unwrap();
  let numeric = version_number::windows_version(&version).expect("Некорректная версия выпуска");
  let dotted = numeric.map(|n| n.to_string()).join(".");
  let comma = numeric.map(|n| n.to_string()).join(",");
  let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
  let out = out_dir.join("app-res.o");
  let manifest = out_dir.join("app.manifest");
  std::fs::write(
    &manifest,
    std::fs::read_to_string("assets/app.manifest")
      .unwrap()
      .replace("@WINDOWS_VERSION@", &dotted),
  )
  .unwrap();
  let resource = out_dir.join("app.rc");
  let icon = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets/app.ico");
  let resources = std::fs::read_to_string("assets/app.rc")
    .unwrap()
    .replace("@VERSION@", &version)
    .replace("@WINDOWS_COMMA@", &comma)
    .replace("@MANIFEST@", &manifest.to_string_lossy().replace('\\', "/"))
    .replace("@ICON@", &icon.to_string_lossy().replace('\\', "/"));
  std::fs::write(&resource, resources).unwrap();
  if env::var("CARGO_CFG_TARGET_ENV").unwrap() == "gnu" {
    let status = Command::new("windres")
      .arg("-i")
      .arg(&resource)
      .args(["-O", "coff", "-o"])
      .arg(&out)
      .status()
      .expect("Не найден windres. Запустите scripts/build.ps1");
    assert!(status.success(), "Не удалось собрать ресурсы Windows");
    println!("cargo:rustc-link-arg={}", out.display());
  } else {
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
  }
}

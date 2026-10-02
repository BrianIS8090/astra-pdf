use std::{env, path::PathBuf, process::Command};

fn main() {
  println!("cargo:rerun-if-changed=assets/app.rc");
  println!("cargo:rerun-if-changed=assets/app.manifest");
  println!("cargo:rerun-if-changed=assets/app.ico");
  let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("app-res.o");
  if env::var("CARGO_CFG_TARGET_ENV").unwrap() == "gnu" {
    let status = Command::new("windres")
      .args(["-i", "assets/app.rc", "-O", "coff", "-o"])
      .arg(&out)
      .status()
      .expect("Не найден windres. Запустите scripts/build.ps1");
    assert!(status.success(), "Не удалось собрать ресурсы Windows");
    println!("cargo:rustc-link-arg={}", out.display());
  } else {
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:assets/app.manifest");
  }
}

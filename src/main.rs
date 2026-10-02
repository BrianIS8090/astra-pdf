#![cfg_attr(not(test), windows_subsystem = "windows")]

mod engine;
mod fixture;
mod layers;
mod layout;
mod model;
mod pdf;
mod print_pipeline;
mod printing;
mod ui;
mod worker;

use std::{
  path::PathBuf,
  sync::{atomic::AtomicBool, Arc},
};

fn pdfium_path() -> PathBuf {
  if let Some(path) = std::env::var_os("ASTRA_PDFIUM_PATH") {
    return PathBuf::from(path);
  }
  std::env::current_exe()
    .unwrap_or_default()
    .with_file_name("pdfium.dll")
}

fn main() {
  if let Err(error) = start() {
    let log = std::env::temp_dir().join("astra-pdf-error.log");
    let _ = std::fs::write(log, &error);
    if !std::env::args_os()
      .skip(1)
      .any(|v| v.to_string_lossy().starts_with("--"))
    {
      unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
          std::ptr::null_mut(),
          printing::wide(&error).as_ptr(),
          printing::wide("Astra PDF").as_ptr(),
          0x10,
        );
      }
    }
    std::process::exit(1);
  }
}

fn start() -> Result<(), String> {
  let args: Vec<_> = std::env::args_os().skip(1).collect();
  match args.first().and_then(|s| s.to_str()) {
    Some("--engine") => engine::serve(false),
    Some("--engine-diagnostics") => engine::serve(true),
    Some("--engine-selftest") if args.len() == 3 => engine::self_test(&PathBuf::from(&args[1]), &PathBuf::from(&args[2])),
    Some("--demo") if args.len() == 2 => std::fs::write(&args[1], fixture::demo()).map_err(|e| e.to_string()),
    Some("--render") if args.len() == 3 => {
      let mut engine = engine::Client::spawn(&pdfium_path())?;
      let meta = engine.open(&PathBuf::from(&args[1]), || false)?;
      let (w, h, _) = model::render_size(meta.sizes[0], (1000, 1200), model::Zoom::FitPage, 96., 0);
      let key = model::RenderKey { page: 0, width: w, height: h, rotation: 0, states: meta.layers.iter().map(|l| l.visible).collect(), region: None };
      engine.render(&key, false, || false)?.save_png(&PathBuf::from(&args[2]))
    }
    Some("--print-to-pdf") if args.len() == 3 => {
      let mut engine = engine::Client::spawn(&pdfium_path())?;
      let meta = engine.open(&PathBuf::from(&args[1]), || false)?;
      unsafe {
        use windows_sys::Win32::Graphics::Gdi::CreateDCW;
        let dc = CreateDCW(printing::wide("WINSPOOL").as_ptr(), printing::wide("Microsoft Print to PDF").as_ptr(), std::ptr::null(), std::ptr::null());
        if dc.is_null() { return Err("Не установлен принтер Microsoft Print to PDF.".into()); }
        let job = printing::PrintJob { dc: dc as usize, first: 0, last: meta.sizes.len() - 1, title: "Astra PDF — проверка печати".into(), output: Some(args[2].to_string_lossy().into_owned()), cancel: Arc::new(AtomicBool::new(false)) };
        let states: Vec<bool> = meta.layers.iter().map(|l| l.visible).collect();
        printing::print(&meta.sizes, job, 0, |page, width, height, rotation| engine.render(&model::RenderKey { page, width, height, rotation, states: states.clone(), region: None }, true, || false), |_, _| {})
      }
    }
    Some("--ui-smoke") if args.len() == 3 => ui::run(pdfium_path(), Some(PathBuf::from(&args[1])), Some(PathBuf::from(&args[2])), None),
    Some("--ui-test") if args.len() == 3 => ui::run(pdfium_path(), Some(PathBuf::from(&args[1])), None, Some(PathBuf::from(&args[2]))),
    Some(arg) if arg.starts_with("--") => Err("Аргументы: [документ.pdf], --demo файл.pdf, --render файл.pdf снимок.png, --print-to-pdf файл.pdf результат.pdf".into()),
    _ => ui::run(pdfium_path(), args.first().map(PathBuf::from), None, None),
  }
}

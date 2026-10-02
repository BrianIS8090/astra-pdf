use crate::{model::print_rect, pdf::Raster};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Settings<'a> {
  pub first: usize,
  pub last: usize,
  pub rotation: i32,
  pub cancel: &'a AtomicBool,
}
pub struct Capabilities {
  pub dpi: (i32, i32),
  pub area: (i32, i32),
}

pub trait Device {
  fn begin(&mut self) -> Result<(), String>;
  fn capabilities(&mut self) -> Result<Capabilities, String>;
  fn begin_page(&mut self) -> Result<(), String>;
  fn draw(&mut self, image: &Raster, rect: (i32, i32, i32, i32)) -> Result<(), String>;
  fn end_page(&mut self) -> Result<(), String>;
  fn commit(&mut self) -> Result<(), String>;
  fn abort(&mut self);
}

pub fn run(
  device: &mut impl Device,
  sizes: &[(f64, f64)],
  settings: Settings<'_>,
  mut render: impl FnMut(usize, i32, i32, i32) -> Result<Raster, String>,
  progress: impl Fn(usize, usize),
) -> Result<(), String> {
  let Settings {
    first,
    last,
    rotation,
    cancel,
  } = settings;
  if first > last || last >= sizes.len() {
    return Err("Некорректный диапазон печати.".into());
  }
  let check_cancel = || {
    if cancel.load(Ordering::Relaxed) {
      Err("Печать отменена.".to_string())
    } else {
      Ok(())
    }
  };
  check_cancel()?;
  device.begin()?;
  let result = (|| {
    let Capabilities { dpi, area } = device.capabilities()?;
    if dpi.0 <= 0 || dpi.1 <= 0 || area.0 <= 0 || area.1 <= 0 {
      return Err("Неверные параметры принтера.".into());
    }
    for (index, &(w, h)) in sizes.iter().enumerate().take(last + 1).skip(first) {
      check_cancel()?;
      if !w.is_finite() || !h.is_finite() || w <= 0. || h <= 0. {
        return Err("Неверный размер страницы для печати.".into());
      }
      progress(index - first + 1, last - first + 1);
      check_cancel()?;
      let size = if rotation.rem_euclid(2) == 0 {
        (w, h)
      } else {
        (h, w)
      };
      let rect = print_rect(size, area, dpi);
      if rect.2 <= 0 || rect.3 <= 0 {
        return Err("Страница не помещается в область печати.".into());
      }
      let rw = rect.2 as f64 * 200. / dpi.0 as f64;
      let rh = rect.3 as f64 * 200. / dpi.1 as f64;
      let limit = (23_990_000. / (rw * rh)).sqrt().min(1.);
      let image = render(
        index,
        (rw * limit).round().max(1.) as i32,
        (rh * limit).round().max(1.) as i32,
        rotation,
      )?;
      check_cancel()?;
      device.begin_page()?;
      device.draw(&image, rect)?;
      device.end_page()?;
    }
    check_cancel()?;
    device.commit()
  })();
  if result.is_err() {
    device.abort();
  }
  result
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::{cell::RefCell, sync::Arc};
  #[derive(Default)]
  struct Fake {
    calls: Vec<&'static str>,
    fail: Option<&'static str>,
    invalid_caps: bool,
  }
  impl Fake {
    fn call(&mut self, name: &'static str) -> Result<(), String> {
      self.calls.push(name);
      if self.fail == Some(name) {
        Err(format!("Ошибка {name}"))
      } else {
        Ok(())
      }
    }
  }
  impl Device for Fake {
    fn begin(&mut self) -> Result<(), String> {
      self.call("begin")
    }
    fn capabilities(&mut self) -> Result<Capabilities, String> {
      self.call("caps")?;
      Ok(Capabilities {
        dpi: (600, 300),
        area: if self.invalid_caps {
          (0, 0)
        } else {
          (2400, 3200)
        },
      })
    }
    fn begin_page(&mut self) -> Result<(), String> {
      self.call("page")
    }
    fn draw(&mut self, _: &Raster, rect: (i32, i32, i32, i32)) -> Result<(), String> {
      assert!(rect.0 >= 0 && rect.1 >= 0 && rect.2 <= 2400 && rect.3 <= 3200);
      self.call("draw")
    }
    fn end_page(&mut self) -> Result<(), String> {
      self.call("end_page")
    }
    fn commit(&mut self) -> Result<(), String> {
      self.call("commit")
    }
    fn abort(&mut self) {
      self.calls.push("abort");
    }
  }
  fn raster(_: usize, _: i32, _: i32, _: i32) -> Result<Raster, String> {
    Ok(Raster {
      width: 1,
      height: 1,
      pixels: Arc::new(vec![255; 4]),
    })
  }
  fn settings(cancel: &AtomicBool) -> Settings<'_> {
    Settings {
      first: 0,
      last: 0,
      rotation: 0,
      cancel,
    }
  }
  #[test]
  fn every_driver_failure_aborts_without_reporting_success() {
    for failure in ["begin", "caps", "page", "draw", "end_page", "commit"] {
      let mut device = Fake {
        fail: Some(failure),
        ..Default::default()
      };
      assert!(run(
        &mut device,
        &[(600., 800.)],
        settings(&AtomicBool::new(false)),
        raster,
        |_, _| {}
      )
      .is_err());
      if failure == "begin" {
        assert_eq!(device.calls, ["begin"]);
      } else {
        assert_eq!(device.calls.last(), Some(&"abort"));
        assert_eq!(device.calls.iter().filter(|s| **s == "abort").count(), 1);
      }
      if failure != "commit" {
        assert!(!device.calls.contains(&"commit"));
      }
    }
  }
  #[test]
  fn cancellation_before_and_during_render_stops_spooling() {
    let cancel = AtomicBool::new(true);
    let mut device = Fake::default();
    assert!(run(
      &mut device,
      &[(600., 800.)],
      settings(&cancel),
      raster,
      |_, _| {}
    )
    .is_err());
    assert!(device.calls.is_empty());
    cancel.store(false, Ordering::Relaxed);
    let result = run(
      &mut device,
      &[(600., 800.)],
      settings(&cancel),
      |a, b, c, d| {
        cancel.store(true, Ordering::Relaxed);
        raster(a, b, c, d)
      },
      |_, _| {},
    );
    assert!(result.unwrap_err().contains("отменена"));
    assert_eq!(device.calls, ["begin", "caps", "abort"]);
  }
  #[test]
  fn rendering_error_does_not_start_a_blank_page() {
    let mut device = Fake::default();
    let result = run(
      &mut device,
      &[(600., 800.)],
      settings(&AtomicBool::new(false)),
      |_, _, _, _| Err("Сбой PDF".into()),
      |_, _| {},
    );
    assert_eq!(result.unwrap_err(), "Сбой PDF");
    assert_eq!(device.calls, ["begin", "caps", "abort"]);
  }
  #[test]
  fn exact_selected_pages_rotation_and_completion() {
    let mut device = Fake::default();
    let cancel = AtomicBool::new(false);
    let mut pages = vec![];
    let progress = RefCell::new(vec![]);
    run(
      &mut device,
      &[(600., 800.); 4],
      Settings {
        first: 1,
        last: 2,
        rotation: 1,
        cancel: &cancel,
      },
      |page, w, h, rot| {
        assert!(w > h);
        assert_eq!(rot, 1);
        pages.push(page);
        raster(page, w, h, rot)
      },
      |p, t| progress.borrow_mut().push((p, t)),
    )
    .unwrap();
    assert_eq!(pages, [1, 2]);
    assert_eq!(*progress.borrow(), [(1, 2), (2, 2)]);
    assert_eq!(device.calls.iter().filter(|s| **s == "page").count(), 2);
    assert_eq!(device.calls.last(), Some(&"commit"));
    assert!(!device.calls.contains(&"abort"));
  }
  #[test]
  fn invalid_ranges_and_printer_geometry_fail_safely() {
    let cancel = AtomicBool::new(false);
    let mut device = Fake::default();
    assert!(run(&mut device, &[], settings(&cancel), raster, |_, _| {}).is_err());
    assert!(device.calls.is_empty());
    device.invalid_caps = true;
    assert!(run(
      &mut device,
      &[(600., 800.)],
      settings(&cancel),
      raster,
      |_, _| {}
    )
    .is_err());
    assert_eq!(device.calls, ["begin", "caps", "abort"]);
  }
}

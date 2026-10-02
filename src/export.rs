use crate::{
  editing::Mask,
  model::{Region, RenderKey},
  vault,
};
use std::{
  fs::{File, OpenOptions},
  io::{Read, Seek, Write},
  path::{Path, PathBuf},
};

pub fn read(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
  let mut bytes = Vec::new();
  File::open(path)
    .map_err(|e| e.to_string())?
    .take(limit as u64 + 1)
    .read_to_end(&mut bytes)
    .map_err(|e| e.to_string())?;
  if bytes.len() > limit {
    return Err("Файл превышает допустимый размер.".into());
  }
  Ok(bytes)
}

pub fn same_file(a: &Path, b: &Path) -> bool {
  if let (Ok(a), Ok(b)) = (a.canonicalize(), b.canonicalize()) {
    return a.as_os_str().eq_ignore_ascii_case(b.as_os_str());
  }
  a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
}

pub struct AtomicFile {
  pub file: File,
  temp: PathBuf,
  destination: PathBuf,
}
impl AtomicFile {
  pub fn new(destination: &Path) -> Result<Self, String> {
    let suffix = vault::random::<16>()?
      .iter()
      .map(|b| format!("{b:02x}"))
      .collect::<String>();
    let temp = destination.with_file_name(format!(".astra-{suffix}.partial"));
    let file = OpenOptions::new()
      .read(true)
      .write(true)
      .create_new(true)
      .open(&temp)
      .map_err(|e| e.to_string())?;
    Ok(Self {
      file,
      temp,
      destination: destination.into(),
    })
  }
  pub fn finish(self) -> Result<(), String> {
    self.file.sync_all().map_err(|e| e.to_string())?;
    use std::os::windows::ffi::OsStrExt;
    let from: Vec<_> = self.temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<_> = self
      .destination
      .as_os_str()
      .encode_wide()
      .chain(Some(0))
      .collect();
    // Открытый файл допускает переименование в Windows; стандартный File разрешает FILE_SHARE_DELETE.
    let ok = unsafe {
      windows_sys::Win32::Storage::FileSystem::MoveFileExW(
        from.as_ptr(),
        to.as_ptr(),
        windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
          | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
      )
    };
    if ok == 0 {
      return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
  }
}
impl Drop for AtomicFile {
  fn drop(&mut self) {
    let _ = std::fs::remove_file(&self.temp);
  }
}
pub fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
  let mut file = AtomicFile::new(path)?;
  file.file.write_all(bytes).map_err(|e| e.to_string())?;
  file.finish()
}

// Преобразования выполняются над нормализованными координатами исходной страницы.
pub fn rotate_point((x, y): (f64, f64), rotation: i32) -> (f64, f64) {
  match rotation.rem_euclid(4) {
    1 => (1. - y, x),
    2 => (1. - x, 1. - y),
    3 => (y, 1. - x),
    _ => (x, y),
  }
}
pub fn rotate_bounds(bounds: [f64; 4], rotation: i32) -> [f64; 4] {
  let a = rotate_point((bounds[0], bounds[1]), rotation);
  let b = rotate_point((bounds[2], bounds[3]), rotation);
  [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
}

pub fn mask_pixels(
  pixels: &mut [u8],
  size: (i32, i32),
  region: Region,
  masks: &[Mask],
  page: usize,
) {
  for mask in masks.iter().filter(|m| m.page == page) {
    let [l, t, r, b] = mask.bounds;
    let left = ((l * size.0 as f64).floor() as i32 - 2 - region.x).clamp(0, region.width);
    let top = ((t * size.1 as f64).floor() as i32 - 2 - region.y).clamp(0, region.height);
    let right = ((r * size.0 as f64).ceil() as i32 + 2 - region.x).clamp(0, region.width);
    let bottom = ((b * size.1 as f64).ceil() as i32 + 2 - region.y).clamp(0, region.height);
    for y in top..bottom {
      for x in left..right {
        // Мозаика не зависит от исходных пикселей и поэтому не сохраняет их значения.
        let shade = 72 + (((x + region.x) / 16 + (y + region.y) / 16).rem_euclid(2) * 24) as u8;
        let i = ((y * region.width + x) * 4) as usize;
        pixels[i..i + 4].copy_from_slice(&[shade, shade, shade, 255]);
      }
    }
  }
}

struct Writer<'a> {
  file: &'a mut File,
  offsets: Vec<u64>,
}
impl Writer<'_> {
  fn object(&mut self, body: &[u8]) -> Result<usize, String> {
    let id = self.offsets.len();
    self.offsets.push(0);
    self.object_at(id, body)?;
    Ok(id)
  }
  fn object_at(&mut self, id: usize, body: &[u8]) -> Result<(), String> {
    let offset = self.file.stream_position().map_err(|e| e.to_string())?;
    if offset > 4_000_000_000 {
      return Err("Очищенная копия превышает 4 ГБ. Сохраните меньше страниц.".into());
    }
    self.offsets[id] = offset;
    writeln!(self.file, "{id} 0 obj")
      .and_then(|()| self.file.write_all(body))
      .and_then(|()| self.file.write_all(b"\nendobj\n"))
      .map_err(|e| e.to_string())?;
    Ok(())
  }
  fn stream(&mut self, attributes: &str, bytes: &[u8]) -> Result<usize, String> {
    let mut compressed = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    compressed.write_all(bytes).map_err(|e| e.to_string())?;
    let compressed = compressed.finish().map_err(|e| e.to_string())?;
    let mut body = format!(
      "<< {attributes} /Filter /FlateDecode /Length {} >>\nstream\n",
      compressed.len()
    )
    .into_bytes();
    body.extend_from_slice(&compressed);
    body.extend_from_slice(b"\nendstream");
    self.object(&body)
  }
}

pub fn clean_pdf(
  mut render: impl FnMut(&RenderKey) -> Result<crate::pdf::Raster, String>,
  sizes: &[(f64, f64)],
  states: &[bool],
  masks: &[Mask],
  destination: &Path,
  cancelled: impl Fn() -> bool,
  progress: impl Fn(usize, usize),
) -> Result<(), String> {
  if masks.is_empty() || masks.iter().any(|m| !m.valid(sizes.len())) {
    return Err("Сначала выделите области для скрытия.".into());
  }
  let mut output = AtomicFile::new(destination)?;
  output
    .file
    .write_all(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    .map_err(|e| e.to_string())?;
  let mut writer = Writer {
    file: &mut output.file,
    offsets: vec![0, 0, 0],
  };
  let mut page_ids = Vec::new();
  for (page, &(w, h)) in sizes.iter().enumerate() {
    if cancelled() {
      return Err("Сохранение отменено.".into());
    }
    let size = (
      (w * 300. / 72.).ceil() as i32,
      (h * 300. / 72.).ceil() as i32,
    );
    if size.0 <= 0
      || size.1 <= 0
      || size.0 > 200_000
      || size.1 > 200_000
      || i64::from(size.0) * i64::from(size.1) > 1_000_000_000
    {
      return Err("Страница слишком велика для защищённого экспорта при 300 dpi.".into());
    }
    let mut resources = String::new();
    let mut commands = String::new();
    let mut n = 0;
    for y in (0..size.1).step_by(1024) {
      for x in (0..size.0).step_by(2048) {
        if cancelled() {
          return Err("Сохранение отменено.".into());
        }
        let region = Region {
          x,
          y,
          width: (size.0 - x).min(2048),
          height: (size.1 - y).min(1024),
        };
        let key = RenderKey {
          page,
          width: size.0,
          height: size.1,
          rotation: 0,
          states: states.to_vec(),
          region: Some(region),
        };
        let image = render(&key)?;
        let mut pixels = (*image.pixels).clone();
        mask_pixels(&mut pixels, size, region, masks, page);
        let rgb: Vec<_> = pixels
          .as_chunks::<4>()
          .0
          .iter()
          .flat_map(|p| [p[2], p[1], p[0]])
          .collect();
        let id = writer.stream(&format!("/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Interpolate false",region.width,region.height),&rgb)?;
        resources.push_str(&format!(" /I{n} {id} 0 R"));
        commands.push_str(&format!(
          "q {:.9} 0 0 {:.9} {:.9} {:.9} cm /I{n} Do Q\n",
          region.width as f64 * w / size.0 as f64,
          region.height as f64 * h / size.1 as f64,
          x as f64 * w / size.0 as f64,
          (size.1 - y - region.height) as f64 * h / size.1 as f64
        ));
        n += 1;
      }
    }
    let content = writer.stream("", commands.as_bytes())?;
    let id = writer.object(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Resources << /XObject <<{resources} >> >> /Contents {content} 0 R >>").as_bytes())?;
    page_ids.push(id);
    progress(page + 1, sizes.len());
  }
  let kids = page_ids
    .iter()
    .map(|id| format!("{id} 0 R"))
    .collect::<Vec<_>>()
    .join(" ");
  writer.object_at(
    2,
    format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", sizes.len()).as_bytes(),
  )?;
  writer.object_at(1, b"<< /Type /Catalog /Pages 2 0 R >>")?;
  let root = 1;
  let xref = writer.file.stream_position().map_err(|e| e.to_string())?;
  write!(
    writer.file,
    "xref\n0 {}\n0000000000 65535 f \n",
    writer.offsets.len()
  )
  .map_err(|e| e.to_string())?;
  for offset in writer.offsets.iter().skip(1) {
    writeln!(writer.file, "{offset:010} 00000 n ").map_err(|e| e.to_string())?;
  }
  write!(
    writer.file,
    "trailer\n<< /Size {} /Root {root} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
    writer.offsets.len()
  )
  .map_err(|e| e.to_string())?;
  if cancelled() {
    return Err("Сохранение отменено.".into());
  }
  output.finish()
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn clean_copy_has_only_new_images_and_cancel_preserves_existing_file() {
    use crate::pdf::{Api, Pdf};
    use std::sync::Arc;
    let api = Api::new(&crate::pdfium_path()).unwrap();
    let pdf = Pdf::open(api.clone(), Arc::new(crate::fixture::demo())).unwrap();
    let token = vault::random::<16>()
      .unwrap()
      .map(|b| format!("{b:02x}"))
      .join("");
    let path = std::env::temp_dir().join(format!("astra-safe-test-{token}.pdf"));
    let masks = vec![Mask {
      page: 0,
      bounds: [0., 0., 1., 1.],
    }];
    clean_pdf(
      |key| pdf.render_region(key.page, (key.width, key.height), 0, false, key.region),
      &pdf.sizes[..1],
      &[],
      &masks,
      &path,
      || false,
      |_, _| {},
    )
    .unwrap();
    let bytes = read(&path, vault::LIMIT).unwrap();
    let doc = lopdf::Document::load_mem(&bytes).unwrap();
    assert_eq!(doc.get_pages().len(), 1);
    assert_eq!(doc.extract_text(&[1]).unwrap(), "");
    let catalog = doc.catalog().unwrap();
    for name in [
      b"OCProperties".as_slice(),
      b"Metadata",
      b"Names",
      b"AcroForm",
      b"OpenAction",
    ] {
      assert!(!catalog.has(name));
    }
    let reopened = Pdf::open(api, Arc::new(bytes.clone())).unwrap();
    let raster = reopened.render(0, 300, 400, 0, false).unwrap();
    assert!(raster
      .pixels
      .as_chunks::<4>()
      .0
      .iter()
      .all(|p| p[0] == p[1] && p[1] == p[2] && (70..=98).contains(&p[0])));
    assert!(clean_pdf(
      |_| panic!("Отменённая операция не должна рисовать"),
      &pdf.sizes,
      &[],
      &masks,
      &path,
      || true,
      |_, _| {}
    )
    .is_err());
    assert_eq!(read(&path, vault::LIMIT).unwrap(), bytes);
    std::fs::remove_file(path).unwrap();
  }
  #[test]
  fn masks_erase_pixels_across_tile_boundaries_and_all_rotations_roundtrip() {
    let mask = Mask {
      page: 0,
      bounds: [0.45, 0.25, 0.55, 0.75],
    };
    for rotation in 0..4 {
      assert_eq!(
        rotate_bounds(rotate_bounds(mask.bounds, rotation), -rotation)
          .map(|v| (v * 1000.).round() as i32),
        mask.bounds.map(|v| (v * 1000.).round() as i32)
      );
    }
    for x in [0, 50] {
      let mut a = vec![7; 50 * 100 * 4];
      let mut b = vec![123; 50 * 100 * 4];
      let region = Region {
        x,
        y: 0,
        width: 50,
        height: 100,
      };
      mask_pixels(&mut a, (100, 100), region, std::slice::from_ref(&mask), 0);
      mask_pixels(&mut b, (100, 100), region, std::slice::from_ref(&mask), 0);
      let column = if x == 0 { 49 } else { 0 };
      for y in 23..77 {
        let i = (y * 50 + column) * 4;
        assert_eq!(&a[i..i + 4], &b[i..i + 4]);
      }
      assert_ne!(a[0], b[0]);
    }
  }
}

use crate::{
  editing::{Mask, MaskKind},
  model::{Region, RenderKey},
  pdf::Raster,
};
use std::sync::Arc;

pub fn key(
  page: usize,
  size: (f64, f64),
  block_mm: u8,
  states: &[bool],
) -> Result<RenderKey, String> {
  if ![3, 6, 12].contains(&block_mm)
    || !size.0.is_finite()
    || !size.1.is_finite()
    || size.0 <= 0.
    || size.1 <= 0.
  {
    return Err("Неверный размер страницы или блока пикселизации.".into());
  }
  let points = f64::from(block_mm) * 72. / 25.4;
  let width = (size.0 / points).ceil() as i32;
  let height = (size.1 / points).ceil() as i32;
  if width > 4096 || height > 4096 || i64::from(width) * i64::from(height) > 1_000_000 {
    return Err("Для этого листа выберите более крупные блоки пикселизации.".into());
  }
  Ok(RenderKey {
    page,
    width,
    height,
    rotation: 0,
    states: states.to_vec(),
    region: None,
  })
}

pub fn render_grid(
  mut render: impl FnMut(&RenderKey) -> Result<Raster, String>,
  key: &RenderKey,
) -> Result<Raster, String> {
  if key.width <= 0
    || key.height <= 0
    || key.width > 4096
    || key.height > 4096
    || i64::from(key.width) * i64::from(key.height) > 1_000_000
  {
    return Err("Слишком большая сетка пикселизации.".into());
  }
  let mut pixels = vec![0; key.width as usize * key.height as usize * 4];
  // Четыре отсчёта по каждой оси сохраняют средние цвета, не рисуя огромную страницу.
  for y in (0..key.height).step_by(256) {
    for x in (0..key.width).step_by(256) {
      let width = (key.width - x).min(256);
      let height = (key.height - y).min(256);
      let region = Region {
        x: x * 4,
        y: y * 4,
        width: width * 4,
        height: height * 4,
      };
      let sample = render(&RenderKey {
        width: key.width * 4,
        height: key.height * 4,
        region: Some(region),
        ..key.clone()
      })?;
      if sample.width != region.width
        || sample.height != region.height
        || sample.pixels.len() != region.width as usize * region.height as usize * 4
      {
        return Err("Движок вернул неверный фрагмент пикселизации.".into());
      }
      for row in 0..height {
        for col in 0..width {
          let mut sum = [0u32; 3];
          for sy in 0..4 {
            for sx in 0..4 {
              let i = (((row * 4 + sy) * region.width + col * 4 + sx) * 4) as usize;
              for (channel, total) in sum.iter_mut().enumerate() {
                *total += u32::from(sample.pixels[i + channel]);
              }
            }
          }
          let out = (((y + row) * key.width + x + col) * 4) as usize;
          for channel in 0..3 {
            pixels[out + channel] = ((sum[channel] + 8) / 16) as u8;
          }
          pixels[out + 3] = 255;
        }
      }
    }
  }
  Ok(Raster {
    width: key.width,
    height: key.height,
    pixels: Arc::new(pixels),
  })
}

pub fn cell_bounds(grid: &Raster, x: i32, y: i32) -> [f64; 4] {
  [
    x as f64 / grid.width as f64,
    y as f64 / grid.height as f64,
    (x + 1) as f64 / grid.width as f64,
    (y + 1) as f64 / grid.height as f64,
  ]
}

pub fn color(grid: &Raster, x: i32, y: i32, covers: &[Mask], page: usize) -> [u8; 4] {
  let cell = cell_bounds(grid, x, y);
  let guard = (1. / grid.width as f64, 1. / grid.height as f64);
  // Полное скрытие имеет приоритет: соседняя мозаика не получает даже средний цвет секрета.
  if covers.iter().any(|m| {
    m.page == page
      && m.kind == MaskKind::Cover
      && cell[0] - guard.0 < m.bounds[2]
      && cell[2] + guard.0 > m.bounds[0]
      && cell[1] - guard.1 < m.bounds[3]
      && cell[3] + guard.1 > m.bounds[1]
  }) {
    return [80, 80, 80, 255];
  }
  let i = ((y * grid.width + x) * 4) as usize;
  grid.pixels[i..i + 4].try_into().unwrap()
}

pub fn apply(
  pixels: &mut [u8],
  size: (i32, i32),
  region: Region,
  mask: &Mask,
  grid: &Raster,
  covers: &[Mask],
) {
  let [l, t, r, b] = mask.bounds;
  let left = ((l * size.0 as f64).floor() as i32 - 2 - region.x).clamp(0, region.width);
  let top = ((t * size.1 as f64).floor() as i32 - 2 - region.y).clamp(0, region.height);
  let right = ((r * size.0 as f64).ceil() as i32 + 2 - region.x).clamp(0, region.width);
  let bottom = ((b * size.1 as f64).ceil() as i32 + 2 - region.y).clamp(0, region.height);
  for y in top..bottom {
    let mut x = left;
    while x < right {
      let gx = (i64::from(x + region.x) * i64::from(grid.width) / i64::from(size.0)) as i32;
      let gy = (i64::from(y + region.y) * i64::from(grid.height) / i64::from(size.1)) as i32;
      let c = color(grid, gx, gy, covers, mask.page);
      let end = (((i64::from(gx + 1) * i64::from(size.0) + i64::from(grid.width) - 1)
        / i64::from(grid.width)) as i32
        - region.x)
        .min(right)
        .max(x + 1);
      let start = ((y * region.width + x) * 4) as usize;
      let stop = ((y * region.width + end) * 4) as usize;
      for pixel in pixels[start..stop].as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&c);
      }
      x = end;
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn grid_averages_content_colors_and_has_bounded_tiles() {
    let key = RenderKey {
      page: 0,
      width: 300,
      height: 270,
      rotation: 0,
      states: vec![],
      region: None,
    };
    let mut calls = 0;
    let grid = render_grid(
      |sample| {
        calls += 1;
        let r = sample.region.unwrap();
        assert!(r.width <= 1024 && r.height <= 1024);
        let mut pixels = Vec::new();
        for y in 0..r.height {
          for x in 0..r.width {
            pixels.extend_from_slice(&[
              ((r.x + x) % 4 * 40) as u8,
              ((r.y + y) % 4 * 60) as u8,
              210,
              255,
            ]);
          }
        }
        Ok(Raster {
          width: r.width,
          height: r.height,
          pixels: Arc::new(pixels),
        })
      },
      &key,
    )
    .unwrap();
    assert_eq!(calls, 4);
    assert!(grid
      .pixels
      .as_chunks::<4>()
      .0
      .iter()
      .all(|p| *p == [60, 90, 210, 255]));
  }

  #[test]
  fn blocks_remain_aligned_across_export_tiles_and_sizes() {
    let grid = Raster {
      width: 7,
      height: 5,
      pixels: Arc::new(
        (0..35)
          .flat_map(|i| [i as u8, (i * 3) as u8, (i * 7) as u8, 255])
          .collect(),
      ),
    };
    let mask = Mask {
      page: 0,
      bounds: [0.05, 0.07, 0.93, 0.89],
      kind: MaskKind::Pixelate { block_mm: 6 },
    };
    for size in [(83, 61), (3, 2), (1027, 521)] {
      let full = Region {
        x: 0,
        y: 0,
        width: size.0,
        height: size.1,
      };
      let mut expected = vec![241; size.0 as usize * size.1 as usize * 4];
      apply(&mut expected, size, full, &mask, &grid, &[]);
      for y in (0..size.1).step_by(37) {
        for x in (0..size.0).step_by(41) {
          let tile = Region {
            x,
            y,
            width: (size.0 - x).min(41),
            height: (size.1 - y).min(37),
          };
          let mut actual = vec![241; tile.width as usize * tile.height as usize * 4];
          apply(&mut actual, size, tile, &mask, &grid, &[]);
          for row in 0..tile.height as usize {
            let a = row * tile.width as usize * 4;
            let b = ((row + y as usize) * size.0 as usize + x as usize) * 4;
            assert_eq!(
              &actual[a..a + tile.width as usize * 4],
              &expected[b..b + tile.width as usize * 4]
            );
          }
        }
      }
    }
  }

  #[test]
  fn full_hiding_does_not_leak_through_neighboring_pixelation_colors() {
    let a = Raster {
      width: 10,
      height: 10,
      pixels: Arc::new(vec![11; 400]),
    };
    let b = Raster {
      width: 10,
      height: 10,
      pixels: Arc::new(vec![220; 400]),
    };
    let covers = [Mask {
      page: 0,
      bounds: [0.44, 0.44, 0.46, 0.46],
      kind: MaskKind::Cover,
    }];
    for y in 3..=5 {
      for x in 3..=5 {
        assert_eq!(color(&a, x, y, &covers, 0), color(&b, x, y, &covers, 0));
      }
    }
    assert_ne!(color(&a, 0, 0, &covers, 0), color(&b, 0, 0, &covers, 0));
  }

  #[test]
  fn grid_size_uses_physical_page_dimensions_and_rejects_excessive_work() {
    let small = key(0, (600., 800.), 3, &[true]).unwrap();
    let large = key(0, (600., 800.), 12, &[true]).unwrap();
    assert!(small.width > large.width * 3);
    assert!(key(0, (f64::NAN, 800.), 6, &[]).is_err());
    assert!(key(0, (1e12, 1e12), 3, &[]).is_err());
    assert!(key(0, (600., 800.), 0, &[]).is_err());
  }
}

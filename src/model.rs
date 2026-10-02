use crate::pdf::Raster;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Zoom {
  FitPage,
  FitWidth,
  Scale(f64),
}

pub fn render_size(
  page: (f64, f64),
  viewport: (i32, i32),
  zoom: Zoom,
  dpi: f64,
  rotation: i32,
) -> (i32, i32, f64) {
  page_size(page, viewport, zoom, dpi, rotation, None)
}

pub fn page_size(
  page: (f64, f64),
  viewport: (i32, i32),
  zoom: Zoom,
  dpi: f64,
  rotation: i32,
  detail_dimension: Option<i32>,
) -> (i32, i32, f64) {
  let (w, h) = if rotation.rem_euclid(2) == 0 {
    page
  } else {
    (page.1, page.0)
  };
  let vw = (viewport.0 - 40).max(40) as f64;
  let vh = (viewport.1 - 40).max(40) as f64;
  let scale = match zoom {
    Zoom::FitPage => (vw / w).min(vh / h),
    Zoom::FitWidth => vw / w,
    Zoom::Scale(s) => s.clamp(0.1, if detail_dimension.is_some() { 64. } else { 8. }) * dpi / 72.,
  };
  let scale = if let Some(limit) = detail_dimension {
    scale.min(limit as f64 / w.max(h))
  } else {
    scale
      .min((23_990_000. / (w * h)).sqrt())
      .min(16_000. / w.max(h))
  };
  (
    (w * scale).round().max(1.) as i32,
    (h * scale).round().max(1.) as i32,
    scale * 72. / dpi,
  )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderKey {
  pub page: usize,
  pub width: i32,
  pub height: i32,
  pub rotation: i32,
  pub states: Vec<bool>,
  pub region: Option<Region>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
  pub x: i32,
  pub y: i32,
  pub width: i32,
  pub height: i32,
}

impl Region {
  pub fn valid(self, page: (i32, i32)) -> bool {
    self.x >= 0
      && self.y >= 0
      && self.width > 0
      && self.height > 0
      && i64::from(self.x) + i64::from(self.width) <= i64::from(page.0)
      && i64::from(self.y) + i64::from(self.height) <= i64::from(page.1)
      && i64::from(self.width) * i64::from(self.height) <= 24_000_000
  }
}

impl RenderKey {
  pub fn raster_size(&self) -> (i32, i32) {
    self
      .region
      .map_or((self.width, self.height), |r| (r.width, r.height))
  }
}

pub fn visible_tiles(page: (i32, i32), visible: Region) -> Vec<Region> {
  const TILE: i32 = 768;
  let left = visible.x.max(0);
  let top = visible.y.max(0);
  let right = visible.x.saturating_add(visible.width).min(page.0);
  let bottom = visible.y.saturating_add(visible.height).min(page.1);
  if right <= left || bottom <= top {
    return vec![];
  }
  let mut tiles = vec![];
  for y in (top / TILE..=(bottom - 1) / TILE).take(16) {
    for x in (left / TILE..=(right - 1) / TILE).take(16) {
      // Два перекрывающихся пикселя сохраняют сглаживание на границах участков.
      let px = (x * TILE - 2).max(0);
      let py = (y * TILE - 2).max(0);
      tiles.push(Region {
        x: px,
        y: py,
        width: ((x + 1) * TILE + 2).min(page.0) - px,
        height: ((y + 1) * TILE + 2).min(page.1) - py,
      });
    }
  }
  let center = (i64::from(left + right), i64::from(top + bottom));
  tiles.sort_by_key(|r| {
    let dx = i64::from(2 * r.x + r.width) - center.0;
    let dy = i64::from(2 * r.y + r.height) - center.1;
    dx * dx + dy * dy
  });
  tiles
}

pub struct Cache {
  entries: VecDeque<(RenderKey, Raster)>,
  pub bytes: usize,
  limit: usize,
  count: usize,
}

impl Cache {
  pub fn new(limit: usize) -> Self {
    Self {
      entries: VecDeque::new(),
      bytes: 0,
      limit,
      count: 4,
    }
  }
  pub fn with_count(limit: usize, count: usize) -> Self {
    Self {
      count,
      ..Self::new(limit)
    }
  }
  pub fn peek(&self, key: &RenderKey) -> Option<&Raster> {
    self
      .entries
      .iter()
      .find(|(k, _)| k == key)
      .map(|(_, image)| image)
  }
  pub fn preview(&self, key: &RenderKey) -> Option<&Raster> {
    self
      .entries
      .iter()
      .find(|(k, _)| {
        k.region.is_none()
          && key.region.is_none()
          && k.page == key.page
          && k.rotation == key.rotation
          && k.states == key.states
      })
      .map(|(_, image)| image)
  }
  pub fn len(&self) -> usize {
    self.entries.len()
  }
  pub fn get(&mut self, key: &RenderKey) -> Option<Raster> {
    let i = self.entries.iter().position(|(k, _)| k == key)?;
    let item = self.entries.remove(i)?;
    let image = item.1.clone();
    self.entries.push_front(item);
    Some(image)
  }
  pub fn put(&mut self, key: RenderKey, image: Raster) {
    let size = image.pixels.len();
    if size > self.limit {
      return;
    }
    if let Some(i) = self.entries.iter().position(|(k, _)| k == &key) {
      self.bytes -= self.entries.remove(i).unwrap().1.pixels.len();
    }
    while self.bytes + size > self.limit || self.entries.len() >= self.count {
      if let Some((_, old)) = self.entries.pop_back() {
        self.bytes -= old.pixels.len();
      } else {
        break;
      }
    }
    self.bytes += size;
    self.entries.push_front((key, image));
  }
}

pub fn print_rect(page: (f64, f64), area: (i32, i32), dpi: (i32, i32)) -> (i32, i32, i32, i32) {
  let scale = (area.0 as f64 / dpi.0 as f64 / page.0).min(area.1 as f64 / dpi.1 as f64 / page.1);
  let w = (page.0 * scale * dpi.0 as f64).round() as i32;
  let h = (page.1 * scale * dpi.1 as f64).round() as i32;
  ((area.0 - w) / 2, (area.1 - h) / 2, w, h)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Arc;

  #[test]
  fn large_drawing_zoom_is_independent_of_raster_budget() {
    let (w, h, scale) = page_size(
      (2384., 1684.),
      (1600, 1200),
      Zoom::Scale(32.),
      192.,
      0,
      Some(1_000_000),
    );
    assert!((scale - 32.).abs() < 0.0001);
    assert!(i64::from(w) * i64::from(h) > 20_000_000_000);
    let tiles = visible_tiles(
      (w, h),
      Region {
        x: w / 2,
        y: h / 2,
        width: 1600,
        height: 1200,
      },
    );
    assert!(tiles.len() <= 12);
    assert!(tiles
      .iter()
      .all(|r| r.valid((w, h)) && r.width <= 772 && r.height <= 772));
    assert!(
      tiles
        .iter()
        .map(|r| i64::from(r.width) * i64::from(r.height) * 4)
        .sum::<i64>()
        < 32 * 1024 * 1024
    );
  }

  #[test]
  fn tiles_cover_viewport_edges_without_gaps_and_reuse_grid() {
    for (x, y) in [(-40, -20), (760, 760), (1800, 1450)] {
      let visible = Region {
        x,
        y,
        width: 500,
        height: 400,
      };
      let tiles = visible_tiles((2000, 1600), visible);
      for py in y.max(0)..(y + 400).min(1600) {
        for px in x.max(0)..(x + 500).min(2000) {
          assert!(tiles
            .iter()
            .any(|r| px >= r.x && py >= r.y && px < r.x + r.width && py < r.y + r.height));
        }
      }
    }
    let a = visible_tiles(
      (100_000, 100_000),
      Region {
        x: 2000,
        y: 2000,
        width: 600,
        height: 600,
      },
    );
    let b = visible_tiles(
      (100_000, 100_000),
      Region {
        x: 2010,
        y: 2010,
        width: 600,
        height: 600,
      },
    );
    assert_eq!(a.len(), b.len());
    assert!(a.iter().all(|r| b.contains(r)));
    assert!(!Region {
      x: i32::MAX,
      y: 0,
      width: 2,
      height: 2
    }
    .valid((1000, 1000)));
  }

  #[test]
  fn fit_rotation_and_memory_limit() {
    let (w, h, _) = render_size((600., 800.), (1000, 800), Zoom::FitPage, 96., 0);
    assert_eq!((w, h), (570, 760));
    let (w, h, _) = render_size((600., 800.), (1000, 800), Zoom::FitPage, 96., 1);
    assert_eq!((w, h), (960, 720));
    let (w, h, _) = render_size((100_000., 100_000.), (1000, 800), Zoom::Scale(8.), 192., 0);
    assert!(i64::from(w) * i64::from(h) <= 24_000_000);
  }

  #[test]
  fn cache_evicts_within_budget_and_updates_recency() {
    let mut c = Cache::new(32);
    let key = |page| RenderKey {
      page,
      width: 2,
      height: 2,
      rotation: 0,
      states: vec![],
      region: None,
    };
    let img = Raster {
      width: 2,
      height: 2,
      pixels: Arc::new(vec![0; 16]),
    };
    c.put(key(0), img.clone());
    c.put(key(1), img.clone());
    assert!(c.get(&key(0)).is_some());
    c.put(key(2), img);
    assert!(c.get(&key(1)).is_none());
    assert!(c.bytes <= 32);
  }

  #[test]
  fn zoom_preview_preserves_page_but_never_old_layer_state() {
    let mut cache = Cache::with_count(1024, 8);
    let original = RenderKey {
      page: 2,
      width: 3,
      height: 4,
      rotation: 0,
      states: vec![true],
      region: None,
    };
    let image = Raster {
      width: 3,
      height: 4,
      pixels: Arc::new(vec![255; 48]),
    };
    cache.put(original.clone(), image);
    let mut enlarged = original.clone();
    enlarged.width = 30;
    enlarged.height = 40;
    assert!(cache.peek(&enlarged).is_none());
    assert!(cache.preview(&enlarged).is_some());
    enlarged.states[0] = false;
    assert!(cache.preview(&enlarged).is_none());
    enlarged.states[0] = true;
    enlarged.rotation = 1;
    assert!(cache.preview(&enlarged).is_none());
    enlarged.rotation = 0;
    enlarged.page = 3;
    assert!(cache.preview(&enlarged).is_none());
  }

  #[test]
  fn print_preserves_physical_aspect_ratio() {
    let (_, _, w, h) = print_rect((600., 800.), (2400, 3200), (600, 300));
    assert!(((w as f64 / 600.) / (h as f64 / 300.) - 0.75).abs() < 0.001);
  }
}

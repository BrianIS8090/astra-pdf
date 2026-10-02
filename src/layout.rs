use crate::model::{render_size, Zoom};
use std::ops::Range;

#[derive(Clone, Debug)]
pub struct PageBox {
  pub top: i32,
  pub width: i32,
  pub height: i32,
  pub scale: f64,
}

#[derive(Default)]
pub struct DocumentLayout {
  pub pages: Vec<PageBox>,
  pub width: i32,
  pub height: i32,
  pub margin: i32,
}

impl DocumentLayout {
  pub fn new(
    sizes: &[(f64, f64)],
    viewport: (i32, i32),
    zoom: Zoom,
    dpi: u32,
    rotation: i32,
  ) -> Self {
    let margin = (20. * dpi as f64 / 96.).round() as i32;
    let gap = (18. * dpi as f64 / 96.).round() as i32;
    let mut top = margin;
    let mut width = 0;
    let pages = sizes
      .iter()
      .map(|&size| {
        // render_size резервирует 40 пикселей; здесь поля зависят от масштаба Windows.
        let view = (viewport.0 - 2 * margin + 40, viewport.1 - 2 * margin + 40);
        let (w, h, scale) = render_size(size, view, zoom, dpi as f64, rotation);
        let page = PageBox {
          top,
          width: w,
          height: h,
          scale,
        };
        top = top.saturating_add(h).saturating_add(gap);
        width = width.max(w);
        page
      })
      .collect();
    Self {
      pages,
      width: width + 2 * margin,
      height: top.saturating_sub(gap) + margin,
      margin,
    }
  }

  pub fn visible(&self, scroll: i32, height: i32) -> Range<usize> {
    let first = self.pages.partition_point(|p| p.top + p.height <= scroll);
    let end = self
      .pages
      .partition_point(|p| p.top < scroll.saturating_add(height));
    first..end.max(first)
  }

  pub fn active(&self, scroll: i32, height: i32) -> usize {
    self
      .visible(scroll, height)
      .max_by_key(|&i| {
        let p = &self.pages[i];
        (p.top + p.height).min(scroll.saturating_add(height)) - p.top.max(scroll)
      })
      .unwrap_or_else(|| self.pages.len().saturating_sub(1))
  }

  pub fn x(&self, page: usize, viewport: i32) -> i32 {
    (self.width.max(viewport) - self.pages[page].width) / 2
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn continuous_pages_share_viewport_at_boundary() {
    let layout = DocumentLayout::new(&[(600., 800.); 3], (900, 700), Zoom::FitPage, 96, 0);
    let second = layout.pages[1].top;
    assert_eq!(layout.visible(second - 100, 300), 0..2);
    assert_eq!(layout.active(second - 100, 300), 1);
    assert!(layout.pages[0].top + layout.pages[0].height < second);
  }

  #[test]
  fn mixed_pages_rotation_and_large_document_have_bounded_geometry() {
    let sizes = [(600., 800.), (1800., 200.), (300., 1200.)];
    let layout = DocumentLayout::new(&sizes, (900, 700), Zoom::FitWidth, 192, 1);
    assert_eq!(layout.pages[0].width, layout.pages[1].width);
    assert!(layout.pages[1].height > layout.pages[0].height);
    let huge = DocumentLayout::new(
      &vec![(100_000., 100_000.); 100_000],
      (1000, 800),
      Zoom::Scale(8.),
      192,
      0,
    );
    assert!(huge.height > 0 && huge.height < i32::MAX);
    assert_eq!(huge.visible(huge.pages[99_999].top, 800), 99_999..100_000);
  }
}

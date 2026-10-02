use crate::editing::Mask;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Glyph {
  pub value: char,
  pub bounds: [f64; 4],
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Target {
  Page(usize),
  Url(String),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Link {
  pub bounds: [f64; 4],
  pub target: Target,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PageText {
  pub glyphs: Vec<Glyph>,
  pub links: Vec<Link>,
  pub notes: Vec<Comment>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Comment {
  pub index: usize,
  pub kind: i32,
  pub bounds: [f64; 4],
  pub text: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Bookmark {
  pub title: String,
  pub page: Option<usize>,
  pub level: usize,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Hit {
  pub page: usize,
  pub bounds: Vec<[f64; 4]>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Search {
  pub hits: Vec<Hit>,
  pub truncated: bool,
}

pub fn intersects(a: [f64; 4], b: [f64; 4]) -> bool {
  a[0] < b[2] && a[2] > b[0] && a[1] < b[3] && a[3] > b[1]
}

pub fn hidden(page: usize, bounds: [f64; 4], masks: &[Mask]) -> bool {
  masks
    .iter()
    .any(|m| m.page == page && intersects(bounds, m.bounds))
}

pub fn selection(page: usize, text: &PageText, start: usize, end: usize, masks: &[Mask]) -> String {
  let range = start.min(end)..=start.max(end);
  let mut result = String::new();
  for (i, glyph) in text.glyphs.iter().enumerate() {
    if range.contains(&i) && !hidden(page, glyph.bounds, masks) && glyph.value != '\0' {
      result.push(glyph.value);
    }
  }
  result.replace("\r\n", "\n")
}

pub fn nearest(text: &PageText, point: (f64, f64)) -> Option<usize> {
  text
    .glyphs
    .iter()
    .enumerate()
    .filter(|(_, g)| g.bounds[2] > g.bounds[0] && g.bounds[3] > g.bounds[1])
    .min_by(|(_, a), (_, b)| {
      let distance = |g: &Glyph| {
        let x = point.0.clamp(g.bounds[0], g.bounds[2]);
        let y = point.1.clamp(g.bounds[1], g.bounds[3]);
        (point.0 - x).powi(2) + (point.1 - y).powi(2)
      };
      distance(a).total_cmp(&distance(b))
    })
    .map(|(i, _)| i)
}

pub fn merge_bounds(bounds: impl IntoIterator<Item = [f64; 4]>) -> Vec<[f64; 4]> {
  let mut result: Vec<[f64; 4]> = Vec::new();
  for b in bounds {
    if b[0] >= b[2] || b[1] >= b[3] {
      continue;
    }
    if let Some(last) = result.last_mut() {
      let height = (b[3] - b[1]).max(last[3] - last[1]);
      if b[1].max(last[1]) < b[3].min(last[3])
        && (b[1] - last[1]).abs() < height * 0.5
        && b[0] >= last[0]
        && b[0] - last[2] < height * 0.8
      {
        last[1] = last[1].min(b[1]);
        last[2] = last[2].max(b[2]);
        last[3] = last[3].max(b[3]);
        continue;
      }
    }
    result.push(b);
  }
  result
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn selection_is_unicode_bidirectional_and_excludes_hidden_glyphs() {
    let text = PageText {
      glyphs: vec![
        Glyph {
          value: 'Я',
          bounds: [0.1, 0.1, 0.2, 0.2],
        },
        Glyph {
          value: 'Б',
          bounds: [0.3, 0.1, 0.4, 0.2],
        },
      ],
      links: vec![],
      notes: vec![],
    };
    assert_eq!(selection(0, &text, 1, 0, &[]), "ЯБ");
    let mask = Mask {
      page: 0,
      bounds: [0.29, 0.09, 0.41, 0.21],
      kind: crate::editing::MaskKind::Cover,
    };
    assert_eq!(selection(0, &text, 0, 1, &[mask]), "Я");
    assert_eq!(nearest(&text, (0.35, 0.15)), Some(1));
  }
}

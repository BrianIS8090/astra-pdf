use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Kind {
  Comment(String),
  Rectangle,
  Arrow,
  Highlight,
  Ink {
    color: [u8; 3],
    width: f64,
  },
  Text {
    text: String,
    color: [u8; 3],
    size: f64,
  },
  Distance {
    scale: f64,
  },
  Area {
    scale: f64,
  },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Note {
  pub page: usize,
  pub points: Vec<[f64; 2]>,
  pub kind: Kind,
}
impl Note {
  pub fn valid(&self, pages: usize) -> bool {
    if self.page >= pages
      || self.points.is_empty()
      || self.points.len()
        > if matches!(self.kind, Kind::Ink { .. }) {
          4096
        } else {
          256
        }
      || self
        .points
        .iter()
        .flatten()
        .any(|v| !v.is_finite() || !(0. ..=1.).contains(v))
    {
      return false;
    }
    match &self.kind {
      Kind::Comment(text) => {
        self.points.len() == 1 && !text.trim().is_empty() && text.len() <= 16_384
      }
      Kind::Ink { width, .. } => {
        self.points.len() >= 2 && width.is_finite() && (0.5..=12.).contains(width)
      }
      Kind::Text { text, size, .. } => {
        self.points.len() == 2
          && !text.trim().is_empty()
          && text.len() <= 16_384
          && size.is_finite()
          && (8. ..=72.).contains(size)
          && (self.points[0][0] - self.points[1][0]).abs() > 0.001
          && (self.points[0][1] - self.points[1][1]).abs() > 0.001
      }
      Kind::Distance { scale } => {
        self.points.len() == 2 && scale.is_finite() && (0.001..=100_000.).contains(scale)
      }
      Kind::Area { scale } => {
        self.points.len() >= 3 && scale.is_finite() && (0.001..=100_000.).contains(scale)
      }
      _ => self.points.len() == 2,
    }
  }

  pub fn measure(&self, size: (f64, f64)) -> Option<String> {
    let scale = match self.kind {
      Kind::Distance { scale } | Kind::Area { scale } => scale,
      _ => return None,
    };
    if !self.valid(self.page + 1)
      || !size.0.is_finite()
      || !size.1.is_finite()
      || size.0 <= 0.
      || size.1 <= 0.
    {
      return None;
    }
    let points: Vec<_> = self
      .points
      .iter()
      .map(|p| {
        (
          p[0] * size.0 * 25.4 / 72. * scale,
          p[1] * size.1 * 25.4 / 72. * scale,
        )
      })
      .collect();
    Some(match self.kind {
      Kind::Distance { .. } => {
        let d = (points[1].0 - points[0].0).hypot(points[1].1 - points[0].1);
        if d >= 1000. {
          format!("{:.3} m", d / 1000.)
        } else {
          format!("{d:.2} mm")
        }
      }
      Kind::Area { .. } => {
        let area = points
          .iter()
          .zip(points.iter().cycle().skip(1))
          .map(|(a, b)| a.0 * b.1 - b.0 * a.1)
          .sum::<f64>()
          .abs()
          * 0.5;
        format!("{:.4} m2", area / 1_000_000.)
      }
      _ => unreachable!(),
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn freehand_and_text_validate_geometry_and_limits() {
    let mut note = Note {
      page: 0,
      points: vec![[0.1, 0.2]; 1000],
      kind: Kind::Ink {
        color: [220, 40, 40],
        width: 2.,
      },
    };
    assert!(note.valid(1));
    note.points.resize(4097, [0.3, 0.4]);
    assert!(!note.valid(1));
    note.points = vec![[0.1, 0.2], [0.5, 0.4]];
    note.kind = Kind::Text {
      text: "Цветная заметка".into(),
      color: [20, 100, 220],
      size: 14.,
    };
    assert!(note.valid(1));
    note.points[1] = note.points[0];
    assert!(!note.valid(1));
    note.points[1] = [0.5, 0.4];
    note.kind = Kind::Text {
      text: " ".into(),
      color: [0, 0, 0],
      size: 14.,
    };
    assert!(!note.valid(1));
  }
  #[test]
  fn measurements_use_physical_page_size_and_scale_once_per_axis() {
    let mut note = Note {
      page: 0,
      points: vec![[0., 0.], [1., 0.]],
      kind: Kind::Distance { scale: 100. },
    };
    assert_eq!(note.measure((72., 72.)).unwrap(), "2.540 m");
    note.points = vec![[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
    note.kind = Kind::Area { scale: 100. };
    assert_eq!(note.measure((72., 72.)).unwrap(), "6.4516 m2");
    note.points.reverse();
    assert_eq!(note.measure((72., 72.)).unwrap(), "6.4516 m2");
    note.kind = Kind::Area { scale: f64::NAN };
    assert!(!note.valid(1));
    note.kind = Kind::Comment("Test".into());
    assert!(!note.valid(1));
  }
}

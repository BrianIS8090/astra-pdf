use super::*;
use crate::{
  editing::Mask,
  reading::{Bookmark, Glyph, Hit, Link, PageText, Search, Target},
};

macro_rules! sym {
  ($pdf:expr,$name:literal,$t:ty) => {
    *$pdf
      .api
      ._library
      .get::<$t>(concat!($name, "\0").as_bytes())
      .map_err(|e| e.to_string())?
  };
}

#[repr(C)]
#[derive(Default)]
struct Rect {
  left: f32,
  top: f32,
  right: f32,
  bottom: f32,
}

impl Pdf {
  unsafe fn normalized_bounds(&self, page: Handle, b: [f64; 4]) -> Result<[f64; 4], String> {
    let convert = sym!(
      self,
      "FPDF_PageToDevice",
      unsafe extern "C" fn(Handle, i32, i32, i32, i32, i32, f64, f64, *mut i32, *mut i32) -> i32
    );
    let (mut x1, mut y1, mut x2, mut y2) = (0, 0, 0, 0);
    if convert(
      page, 0, 0, 1_000_000, 1_000_000, 0, b[0], b[1], &mut x1, &mut y1,
    ) == 0
      || convert(
        page, 0, 0, 1_000_000, 1_000_000, 0, b[2], b[3], &mut x2, &mut y2,
      ) == 0
    {
      return Err("Не удалось определить положение текста.".into());
    }
    Ok([
      x1.min(x2) as f64 / 1_000_000.,
      y1.min(y2) as f64 / 1_000_000.,
      x1.max(x2) as f64 / 1_000_000.,
      y1.max(y2) as f64 / 1_000_000.,
    ])
  }

  unsafe fn destination(&self, mut dest: Handle, action: Handle) -> Result<Option<Target>, String> {
    if dest.is_null() && !action.is_null() {
      let kind = sym!(
        self,
        "FPDFAction_GetType",
        unsafe extern "C" fn(Handle) -> u32
      );
      if kind(action) == 1 {
        dest = sym!(
          self,
          "FPDFAction_GetDest",
          unsafe extern "C" fn(Handle, Handle) -> Handle
        )(self.handle, action);
      } else if kind(action) == 3 {
        let uri = sym!(
          self,
          "FPDFAction_GetURIPath",
          unsafe extern "C" fn(Handle, Handle, *mut c_void, u32) -> u32
        );
        let size = uri(self.handle, action, ptr::null_mut(), 0);
        if (2..=8192).contains(&size) {
          let mut bytes = vec![0; size as usize];
          uri(self.handle, action, bytes.as_mut_ptr().cast(), size);
          bytes.pop();
          if let Ok(url) = String::from_utf8(bytes) {
            let lower = url.to_ascii_lowercase();
            if (lower.starts_with("https://") || lower.starts_with("http://"))
              && !url.chars().any(char::is_control)
            {
              return Ok(Some(Target::Url(url)));
            }
          }
        }
      }
    }
    if !dest.is_null() {
      let index = sym!(
        self,
        "FPDFDest_GetDestPageIndex",
        unsafe extern "C" fn(Handle, Handle) -> i32
      )(self.handle, dest);
      if index >= 0 && (index as usize) < self.sizes.len() {
        return Ok(Some(Target::Page(index as usize)));
      }
    }
    Ok(None)
  }

  pub fn read_page(&self, index: usize) -> Result<PageText, String> {
    if index >= self.sizes.len() {
      return Err("Страница отсутствует.".into());
    }
    unsafe {
      let page = (self.api.page)(self.handle, index as i32);
      if page.is_null() {
        return Err("Не удалось прочитать страницу.".into());
      }
      let result = self.read_page_inner(page);
      (self.api.close_page)(page);
      result
    }
  }

  unsafe fn read_page_inner(&self, page: Handle) -> Result<PageText, String> {
    let load = sym!(
      self,
      "FPDFText_LoadPage",
      unsafe extern "C" fn(Handle) -> Handle
    );
    let close = sym!(self, "FPDFText_ClosePage", Close);
    let count = sym!(
      self,
      "FPDFText_CountChars",
      unsafe extern "C" fn(Handle) -> i32
    );
    let unicode = sym!(
      self,
      "FPDFText_GetUnicode",
      unsafe extern "C" fn(Handle, i32) -> u32
    );
    let box_at = sym!(
      self,
      "FPDFText_GetCharBox",
      unsafe extern "C" fn(Handle, i32, *mut f64, *mut f64, *mut f64, *mut f64) -> i32
    );
    let text = load(page);
    if text.is_null() {
      return Err("Не удалось прочитать текст страницы.".into());
    }
    let result = (|| {
      let n = count(text);
      if !(0..=300_000).contains(&n) {
        return Err("На странице слишком много символов для выделения.".into());
      }
      let mut glyphs = Vec::with_capacity(n as usize);
      for i in 0..n {
        let (mut l, mut r, mut b, mut t) = (0., 0., 0., 0.);
        let bounds = if box_at(text, i, &mut l, &mut r, &mut b, &mut t) != 0 {
          self.normalized_bounds(page, [l, b, r, t])?
        } else {
          [0.; 4]
        };
        glyphs.push(Glyph {
          value: char::from_u32(unicode(text, i)).unwrap_or('\u{fffd}'),
          bounds,
        });
      }
      Ok::<_, String>(glyphs)
    })();
    close(text);
    let glyphs = result?;
    let enumerate = sym!(
      self,
      "FPDFLink_Enumerate",
      unsafe extern "C" fn(Handle, *mut i32, *mut Handle) -> i32
    );
    let rect = sym!(
      self,
      "FPDFLink_GetAnnotRect",
      unsafe extern "C" fn(Handle, *mut Rect) -> i32
    );
    let dest = sym!(
      self,
      "FPDFLink_GetDest",
      unsafe extern "C" fn(Handle, Handle) -> Handle
    );
    let action = sym!(
      self,
      "FPDFLink_GetAction",
      unsafe extern "C" fn(Handle) -> Handle
    );
    let mut links = Vec::new();
    let (mut position, mut link) = (0, ptr::null_mut());
    while links.len() < 4096 && enumerate(page, &mut position, &mut link) != 0 {
      let mut r = Rect::default();
      if rect(link, &mut r) == 0 {
        continue;
      }
      if let Some(target) = self.destination(dest(self.handle, link), action(link))? {
        links.push(Link {
          bounds: self.normalized_bounds(
            page,
            [r.left as f64, r.bottom as f64, r.right as f64, r.top as f64],
          )?,
          target,
        });
      }
      if position > 100_000 {
        break;
      }
    }
    let count = sym!(
      self,
      "FPDFPage_GetAnnotCount",
      unsafe extern "C" fn(Handle) -> i32
    )(page);
    let get = sym!(
      self,
      "FPDFPage_GetAnnot",
      unsafe extern "C" fn(Handle, i32) -> Handle
    );
    let close = sym!(self, "FPDFPage_CloseAnnot", Close);
    let subtype = sym!(
      self,
      "FPDFAnnot_GetSubtype",
      unsafe extern "C" fn(Handle) -> i32
    );
    let rect = sym!(
      self,
      "FPDFAnnot_GetRect",
      unsafe extern "C" fn(Handle, *mut Rect) -> i32
    );
    let value = sym!(
      self,
      "FPDFAnnot_GetStringValue",
      unsafe extern "C" fn(Handle, *const u8, *mut u16, u32) -> u32
    );
    let mut notes = Vec::new();
    for i in 0..count.min(10_000) {
      let annot = get(page, i);
      if annot.is_null() {
        continue;
      }
      let kind = subtype(annot);
      let mut r = Rect::default();
      if [1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 15].contains(&kind) && rect(annot, &mut r) != 0 {
        let size = value(annot, c"Contents".as_ptr().cast(), ptr::null_mut(), 0);
        let mut buffer = vec![
          0u16;
          if size <= 32768 && size % 2 == 0 {
            (size / 2) as usize
          } else {
            0
          }
        ];
        if size > 0 && size <= 32768 && size % 2 == 0 {
          value(
            annot,
            c"Contents".as_ptr().cast(),
            buffer.as_mut_ptr(),
            size,
          );
          buffer.pop();
        }
        let bounds = self.normalized_bounds(
          page,
          [r.left as f64, r.bottom as f64, r.right as f64, r.top as f64],
        );
        close(annot);
        notes.push(crate::reading::Comment {
          index: i as usize,
          kind,
          bounds: bounds?,
          text: String::from_utf16_lossy(&buffer),
        });
      } else {
        close(annot);
      }
    }
    Ok(PageText {
      glyphs,
      links,
      notes,
    })
  }

  pub fn bookmarks(&self) -> Result<Vec<Bookmark>, String> {
    unsafe {
      let child = sym!(
        self,
        "FPDFBookmark_GetFirstChild",
        unsafe extern "C" fn(Handle, Handle) -> Handle
      );
      let sibling = sym!(
        self,
        "FPDFBookmark_GetNextSibling",
        unsafe extern "C" fn(Handle, Handle) -> Handle
      );
      let title = sym!(
        self,
        "FPDFBookmark_GetTitle",
        unsafe extern "C" fn(Handle, *mut c_void, u32) -> u32
      );
      let dest = sym!(
        self,
        "FPDFBookmark_GetDest",
        unsafe extern "C" fn(Handle, Handle) -> Handle
      );
      let action = sym!(
        self,
        "FPDFBookmark_GetAction",
        unsafe extern "C" fn(Handle) -> Handle
      );
      let mut pending = vec![(child(self.handle, ptr::null_mut()), 0)];
      let mut visited = std::collections::HashSet::new();
      let mut result = Vec::new();
      while let Some((bookmark, level)) = pending.pop() {
        if bookmark.is_null() || !visited.insert(bookmark as usize) {
          continue;
        }
        if result.len() >= 4096 {
          break;
        }
        let size = title(bookmark, ptr::null_mut(), 0);
        if size > 65536 || size % 2 != 0 {
          return Err("Недопустимое название закладки.".into());
        }
        let mut text = vec![0u16; (size / 2) as usize];
        if size > 0 {
          title(bookmark, text.as_mut_ptr().cast(), size);
          text.pop();
        }
        let page = match self.destination(dest(self.handle, bookmark), action(bookmark))? {
          Some(Target::Page(p)) => Some(p),
          _ => None,
        };
        result.push(Bookmark {
          title: String::from_utf16_lossy(&text),
          page,
          level,
        });
        pending.push((sibling(self.handle, bookmark), level));
        if level < 32 {
          pending.push((child(self.handle, bookmark), level + 1));
        }
      }
      Ok(result)
    }
  }

  pub fn search(&self, query: &str, masks: &[Mask]) -> Result<Search, String> {
    if query.trim().is_empty() || query.len() > 2048 || masks.len() > 10_000 {
      return Err("Введите текст для поиска (не больше 2048 байт).".into());
    }
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let mut hits = Vec::new();
    for page in 0..self.sizes.len() {
      let text = self.read_page(page)?;
      let mut folded = Vec::new();
      let mut positions = Vec::new();
      for (i, g) in text.glyphs.iter().enumerate() {
        for ch in g.value.to_lowercase() {
          folded.push(if ch == '\u{a0}' { ' ' } else { ch });
          positions.push(i);
        }
      }
      for (i, window) in folded.windows(query.len()).enumerate() {
        if window != query {
          continue;
        }
        let glyphs = &text.glyphs[positions[i]..=positions[i + query.len() - 1]];
        if glyphs
          .iter()
          .any(|g| crate::reading::hidden(page, g.bounds, masks))
        {
          continue;
        }
        let bounds = glyphs
          .iter()
          .filter(|g| g.bounds[2] > g.bounds[0] && g.bounds[3] > g.bounds[1])
          .map(|g| g.bounds)
          .collect::<Vec<_>>();
        if bounds.is_empty() {
          continue;
        }
        hits.push(Hit {
          page,
          bounds: crate::reading::merge_bounds(bounds),
        });
        if hits.len() == 1000 {
          return Ok(Search {
            hits,
            truncated: true,
          });
        }
      }
    }
    Ok(Search {
      hits,
      truncated: false,
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use lopdf::{dictionary, Object};

  #[test]
  fn search_links_bookmarks_and_copy_use_real_pdf_geometry() {
    let mut doc = lopdf::Document::load_mem(&crate::fixture::demo()).unwrap();
    let pages: Vec<_> = doc.get_pages().into_values().collect();
    let link=doc.add_object(dictionary! {"Type"=>"Annot","Subtype"=>"Link","Rect"=>vec![40.into(),730.into(),500.into(),760.into()],"Dest"=>vec![Object::Reference(pages[1]),Object::Name(b"Fit".to_vec())]});
    let web=doc.add_object(dictionary! {"Type"=>"Annot","Subtype"=>"Link","Rect"=>vec![40.into(),50.into(),100.into(),70.into()],"A"=>dictionary! {"S"=>"URI","URI"=>Object::string_literal("https://example.com/")}});
    let bad=doc.add_object(dictionary! {"Type"=>"Annot","Subtype"=>"Link","Rect"=>vec![40.into(),20.into(),100.into(),40.into()],"A"=>dictionary! {"S"=>"URI","URI"=>Object::string_literal("file:///C:/example.exe")}});
    doc
      .get_object_mut(pages[0])
      .unwrap()
      .as_dict_mut()
      .unwrap()
      .set(
        "Annots",
        vec![
          Object::Reference(link),
          Object::Reference(web),
          Object::Reference(bad),
        ],
      );
    let outline = doc.new_object_id();
    let child=doc.add_object(dictionary! {"Title"=>Object::string_literal("Second"),"Parent"=>outline,"Dest"=>vec![Object::Reference(pages[1]),Object::Name(b"Fit".to_vec())]});
    doc.objects.insert(
      outline,
      dictionary! {"Type"=>"Outlines","First"=>child,"Last"=>child,"Count"=>1}.into(),
    );
    doc.catalog_mut().unwrap().set("Outlines", outline);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    let pdf = Pdf::open(Api::new(&crate::pdfium_path()).unwrap(), Arc::new(bytes)).unwrap();
    let text = pdf.read_page(0).unwrap();
    assert!(text
      .glyphs
      .iter()
      .any(|g| g.value == 'L' && g.bounds[0] > 0. && g.bounds[3] < 1.));
    assert_eq!(text.links.len(), 2);
    assert!(text
      .links
      .iter()
      .any(|l| matches!(l.target, Target::Page(1))));
    assert!(text
      .links
      .iter()
      .any(|l| matches!(&l.target,Target::Url(s) if s=="https://example.com/")));
    let bookmarks = pdf.bookmarks().unwrap();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].page, Some(1));
    let hits = pdf.search("aStRa", &[]).unwrap();
    assert_eq!(hits.hits.len(), 3);
    let mask = Mask {
      page: 0,
      bounds: [0., 0., 1., 1.],
      kind: crate::editing::MaskKind::Cover,
    };
    assert_eq!(
      pdf
        .search("ASTRA", std::slice::from_ref(&mask))
        .unwrap()
        .hits
        .len(),
      2
    );
    assert_eq!(
      crate::reading::selection(0, &text, 0, text.glyphs.len() - 1, &[mask]),
      ""
    );
    assert!(pdf.search("", &[]).is_err());
    assert!(pdf.read_page(3).is_err());
  }
}

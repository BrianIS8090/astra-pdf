use super::*;
use crate::{
  editing::{Operation, PageObject},
  engine::Client,
  session::Revision,
};
use std::{sync::mpsc, time::Instant};

pub(super) const INPUT: usize = 140;
pub(super) const APPLY: usize = 141;
pub(super) const DISCARD: usize = 142;
pub(super) const PREVIEW: usize = 143;
pub(super) const FONT: usize = 144;
pub(super) const CHOICE: usize = 145;
const LABEL: usize = 146;
pub(super) const BUTTONS: [(usize, &str); 4] = [
  (APPLY, "Применить · Ctrl+Enter"),
  (DISCARD, "Отменить правку · Esc"),
  (PREVIEW, "Предпросмотр PDF / вернуться к тексту"),
  (FONT, "Выбрать файл шрифта TTF"),
];
struct Preview {
  before: Arc<Revision>,
  after: Arc<Revision>,
  image: Raster,
}
pub(super) struct Draft {
  page: usize,
  object: PageObject,
  text: String,
  font: Option<PathBuf>,
  custom_fonts: Vec<PathBuf>,
  revision: Option<Arc<Revision>>,
  source: PathBuf,
  hash: [u8; 32],
  cancel: Arc<AtomicBool>,
  receiver: Option<mpsc::Receiver<Result<Preview, String>>>,
  preview: Option<Preview>,
  changed: Instant,
  requested: bool,
  show_preview: bool,
  apply: bool,
  pub save: bool,
  error: Option<String>,
}
impl Drop for Draft {
  fn drop(&mut self) {
    self.cancel.store(true, Ordering::Relaxed);
  }
}
impl Draft {
  pub fn probe(&self) -> serde_json::Value {
    serde_json::json!({"page":self.page,"text":self.text,"font":self.font,"busy":self.receiver.is_some(),"preview":self.preview.is_some(),"show_preview":self.show_preview,"error":self.error})
  }
}
impl App {
  pub(super) unsafe fn setup_inline(&mut self) {
    for (id, label) in BUTTONS {
      let h = self.create(id, "BUTTON", label, BS_OWNERDRAW as u32);
      SetParent(h, self.canvas);
      ShowWindow(h, SW_HIDE);
    }
    for (id, class, label, style) in [
      (
        INPUT,
        "EDIT",
        "Редактирование текста",
        WS_BORDER | WS_VSCROLL | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | ES_WANTRETURN as u32,
      ),
      (
        CHOICE,
        "COMBOBOX",
        "Шрифт",
        CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
      ),
      (LABEL, "STATIC", "", 0),
    ] {
      let h = self.create(id, class, label, style);
      SetParent(h, self.canvas);
      ShowWindow(h, SW_HIDE);
    }
    SendMessageW(self.control(INPUT), EM_SETLIMITTEXT, 16384, 0);
    SetTimer(self.hwnd, 7, 100, None);
  }
  pub(super) unsafe fn begin_inline(&mut self, page: usize, object: PageObject) {
    let (Some(source), Some(hash)) = (self.document_path(), self.editor.fingerprint) else {
      return;
    };
    self.reader.stop();
    self.review.stop();
    let text = object.text.clone();
    let choice = self.control(CHOICE);
    SendMessageW(choice, CB_RESETCONTENT, 0, 0);
    for label in [
      format!("Исходный: {} · {} pt", object.font, object.font_size),
      "Arial".into(),
      "Arial Bold".into(),
      "Segoe UI".into(),
      "Times New Roman".into(),
      "Courier New".into(),
    ] {
      SendMessageW(choice, CB_ADDSTRING, 0, wide(&label).as_ptr() as isize);
    }
    SendMessageW(choice, CB_SETCURSEL, 0, 0);
    self.draft = Some(Draft {
      page,
      object,
      text: text.clone(),
      font: None,
      custom_fonts: Vec::new(),
      revision: self.editor.revision(),
      source,
      hash,
      cancel: Arc::new(AtomicBool::new(false)),
      receiver: None,
      preview: None,
      changed: Instant::now(),
      requested: false,
      show_preview: false,
      apply: false,
      save: false,
      error: None,
    });
    SetWindowTextW(
      self.control(INPUT),
      wide(&text.replace('\n', "\r\n")).as_ptr(),
    );
    self.position_editor_actions();
    self.inline_layout();
    SetFocus(self.control(INPUT));
    SendMessageW(self.control(INPUT), EM_SETSEL, 0, -1);
    self.status =
      "Текст на странице · Enter — новая строка · Ctrl+Enter — применить · Esc — отменить".into();
  }
  pub(super) unsafe fn inline_layout(&self) {
    let ids = [INPUT, APPLY, DISCARD, PREVIEW, FONT, CHOICE, LABEL];
    let Some(d) = &self.draft else {
      for id in ids {
        ShowWindow(self.control(id), SW_HIDE);
      }
      return;
    };
    let Some(r) = self.editor_rect(d.page, d.object.bounds) else {
      return;
    };
    let view = self.viewport();
    let gap = self.unit(4);
    let bar = self.unit(32);
    let width = (r.right - r.left)
      .max(self.unit(330))
      .min((view.0 - 2 * gap).max(self.unit(100)));
    let height = self.unit(100);
    let x = r.left.clamp(gap, (view.0 - width - gap).max(gap));
    let y = r.top.clamp(
      bar + gap,
      (view.1 - height - self.unit(25) - gap).max(bar + gap),
    );
    let button_start = (width - 4 * (bar + gap)).max(0);
    for (i, (id, _)) in BUTTONS.iter().enumerate() {
      MoveWindow(
        self.control(*id),
        x + button_start + i as i32 * (bar + gap),
        y - bar - gap,
        bar,
        bar,
        1,
      );
    }
    MoveWindow(
      self.control(CHOICE),
      x,
      y - bar - gap,
      (button_start - gap).max(self.unit(80)),
      self.unit(220),
      1,
    );
    MoveWindow(self.control(INPUT), x, y, width, height, 1);
    let label = if let Some(e) = &d.error {
      e.as_str()
    } else if d.receiver.is_some() {
      "Готовлю точный предпросмотр…"
    } else if d.preview.is_some() {
      "Предпросмотр готов · Ctrl+Enter — применить"
    } else {
      "Исходный шрифт сохраняется; другой выбирается явно"
    };
    SetWindowTextW(self.control(LABEL), wide(label).as_ptr());
    MoveWindow(
      self.control(LABEL),
      x,
      y + height + gap,
      width,
      self.unit(40),
      1,
    );
    for id in ids {
      ShowWindow(
        self.control(id),
        if id == LABEL || (id == INPUT && d.show_preview) {
          SW_HIDE
        } else {
          SW_SHOW
        },
      );
    }
  }
  pub(super) unsafe fn cancel_inline(&mut self) {
    self.draft = None;
    self.inline_layout();
    self.position_editor_actions();
    SetFocus(self.canvas);
    InvalidateRect(self.canvas, ptr::null(), 0);
  }
  pub(super) unsafe fn inline_change(&mut self) {
    let h = self.control(INPUT);
    let mut text = vec![0u16; GetWindowTextLengthW(h) as usize + 1];
    let n = GetWindowTextW(h, text.as_mut_ptr(), text.len() as i32);
    let value = String::from_utf16_lossy(&text[..n as usize]).replace("\r\n", "\n");
    if let Some(d) = &mut self.draft {
      if d.text != value {
        d.text = value;
        d.preview = None;
        d.cancel.store(true, Ordering::Relaxed);
        d.receiver = None;
        d.requested = false;
        d.changed = Instant::now();
        d.error = None;
        d.apply = false;
      }
    }
  }
  pub(super) unsafe fn inline_font(&mut self, font: Option<PathBuf>) {
    if let Some(d) = &mut self.draft {
      d.font = font;
      d.cancel.store(true, Ordering::Relaxed);
      d.receiver = None;
      d.preview = None;
      d.requested = false;
      d.changed = Instant::now();
      d.error = None;
      d.apply = false;
    }
  }
  pub(super) unsafe fn inline_action(&mut self, id: usize) {
    if self.draft.is_none() {
      return;
    }
    self.inline_change();
    match id {
      DISCARD => {
        self.cancel_inline();
        return;
      }
      PREVIEW => {
        let d = self.draft.as_mut().unwrap();
        d.show_preview = !d.show_preview;
        if d.show_preview {
          d.changed = Instant::now() - std::time::Duration::from_secs(1);
        } else {
          SetFocus(self.control(INPUT));
        }
      }
      APPLY => {
        let d = self.draft.as_mut().unwrap();
        d.apply = true;
        d.changed = Instant::now() - std::time::Duration::from_secs(1);
        if d.error.is_some() {
          d.requested = false;
          d.error = None;
        }
      }
      CHOICE => {
        let i = SendMessageW(self.control(CHOICE), CB_GETCURSEL, 0, 0);
        if i >= 6 {
          let font = self
            .draft
            .as_ref()
            .and_then(|d| d.custom_fonts.get(i as usize - 6))
            .cloned();
          if let Some(font) = font {
            self.inline_font(Some(font));
          }
          return;
        }
        let names = [
          "",
          "arial.ttf",
          "arialbd.ttf",
          "segoeui.ttf",
          "times.ttf",
          "cour.ttf",
        ];
        let font = names
          .get(i.max(0) as usize)
          .filter(|s| !s.is_empty())
          .map(|s| {
            PathBuf::from(std::env::var_os("WINDIR").unwrap_or_default())
              .join("Fonts")
              .join(s)
          });
        self.inline_font(font);
      }
      FONT => {
        PostMessageW(self.hwnd, WM_APP + 30, 0, 0);
      }
      _ => (),
    }
    self.poll_inline();
    self.inline_layout();
    InvalidateRect(self.canvas, ptr::null(), 0);
  }
  pub(super) unsafe fn poll_inline(&mut self) {
    if self.draft.is_some() {
      self.inline_change();
    }
    let Some(d) = &mut self.draft else { return };
    if let Some(result) = d.receiver.as_ref().and_then(|r| r.try_recv().ok()) {
      d.receiver = None;
      match result {
        Ok(p) => d.preview = Some(p),
        Err(e) => {
          d.error = Some(e);
          d.apply = false;
        }
      }
      self.status = d
        .error
        .clone()
        .unwrap_or_else(|| "Предпросмотр готов · Ctrl+Enter — применить · Esc — отменить".into());
      self.inline_layout();
      InvalidateRect(self.canvas, ptr::null(), 0);
      InvalidateRect(self.hwnd, ptr::null(), 0);
    }
    let Some(d) = &mut self.draft else { return };
    if d.apply && d.preview.is_some() {
      let preview = d.preview.take().unwrap();
      let save = d.save;
      self.cancel_inline();
      self.commit_revision(preview.before, preview.after);
      if save {
        PostMessageW(self.hwnd, WM_APP + 21, editor::SAVE, 0);
      }
      return;
    }
    if d.requested || d.changed.elapsed().as_millis() < 650 {
      return;
    }
    d.requested = true;
    d.error = None;
    let operation = if let Some(font) = &d.font {
      Operation::FontText {
        page: d.page,
        object: d.object.index,
        text: d.text.clone(),
        font: font.clone(),
      }
    } else {
      Operation::Text {
        page: d.page,
        object: d.object.index,
        text: d.text.clone(),
      }
    };
    let (source, hash, revision) = (d.source.clone(), d.hash, d.revision.clone());
    let page = d.page;
    let (w, h) = self.sizes[page];
    let (w, h) = if self.rotation % 2 == 0 {
      (w, h)
    } else {
      (h, w)
    };
    let factor = (1600. / w.max(h)).min(2.);
    let key = RenderKey {
      page,
      width: (w * factor).round() as i32,
      height: (h * factor).round() as i32,
      rotation: self.rotation,
      states: self.states.clone(),
      region: None,
    };
    let (sender, receiver) = mpsc::channel();
    d.receiver = Some(receiver);
    d.cancel = Arc::new(AtomicBool::new(false));
    let cancel = d.cancel.clone();
    std::thread::spawn(move || {
      let result = (|| {
        let before = if let Some(revision) = revision {
          revision
        } else {
          Revision::new(&crate::export::read(&source, crate::vault::LIMIT)?)?
        };
        if before.hash != hash {
          return Err("Документ изменился на диске. Откройте его повторно.".into());
        }
        let mut client = Client::spawn(&crate::pdfium_path())?;
        client.open_checked(&source, hash, || cancel.load(Ordering::Relaxed))?;
        let bytes = client.edit(operation, || cancel.load(Ordering::Relaxed))?;
        let after = Revision::new(&bytes)?;
        client.open_checked(&after.path, after.hash, || cancel.load(Ordering::Relaxed))?;
        let image = client.render(&key, false, || cancel.load(Ordering::Relaxed))?;
        Ok(Preview {
          before,
          after,
          image,
        })
      })();
      let _ = sender.send(result);
    });
    self.inline_layout();
  }
  pub(super) unsafe fn inline_paint(&self, dc: HDC) {
    let Some(d) = &self.draft else { return };
    if d.show_preview {
      if let Some(p) = &d.preview {
        if let Some(page) = self.document_layout.pages.get(d.page) {
          let (x, y) = self.page_origin(d.page);
          printing::draw_raster(
            dc,
            &p.image,
            x - self.scroll.0,
            y - self.scroll.1,
            page.width,
            page.height,
          );
        }
      }
    }
  }
}
pub(super) unsafe fn choose_font(hwnd: HWND) {
  with_app(|a| a.dialog_open = true);
  let directory = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_default()).join("Fonts");
  let path = crate::dialogs::file(
    hwnd,
    "Выберите шрифт для встраивания в PDF",
    "ttf",
    &directory.join("шрифт.ttf"),
    false,
  );
  with_app(|a| {
    a.dialog_open = false;
    if let Some(path) = path {
      let Some(draft) = a.draft.as_mut() else {
        return;
      };
      draft.custom_fonts.push(path.clone());
      let label = path.file_stem().unwrap_or_default().to_string_lossy();
      let at = SendMessageW(
        a.control(CHOICE),
        CB_ADDSTRING,
        0,
        wide(&format!("Файл: {label}")).as_ptr() as isize,
      );
      SendMessageW(a.control(CHOICE), CB_SETCURSEL, at as usize, 0);
      a.inline_font(Some(path));
    }
  });
}

use super::*;
use crate::updates::Release;
use std::sync::mpsc;
pub(super) const CHECK: usize = 160;
enum ResultItem {
  Checked(Option<Release>),
  Downloaded(PathBuf),
}
#[derive(Default)]
pub(super) struct Updates {
  receiver: Option<mpsc::Receiver<Result<ResultItem, String>>>,
  result: Option<Result<ResultItem, String>>,
}
impl App {
  pub(super) unsafe fn check_updates(&mut self) {
    if self.updates.receiver.is_some() {
      return;
    }
    let (sender, receiver) = mpsc::channel();
    self.updates.receiver = Some(receiver);
    self.status = "Проверяю предварительные выпуски на GitHub…".into();
    std::thread::spawn(move || {
      let _ = sender.send(crate::updates::check().map(ResultItem::Checked));
    });
    SetTimer(self.hwnd, 8, 200, None);
  }
  pub(super) unsafe fn poll_updates(&mut self) {
    if let Some(result) = self
      .updates
      .receiver
      .as_ref()
      .and_then(|r| r.try_recv().ok())
    {
      self.updates.receiver = None;
      self.updates.result = Some(result);
    }
    if self.updates.result.is_some()
      && !self.dialog_open
      && !self.editor.busy
      && !self.printing
      && self.draft.is_none()
    {
      KillTimer(self.hwnd, 8);
      PostMessageW(self.hwnd, WM_APP + 31, 0, 0);
    }
  }
}
pub(super) unsafe fn show(hwnd: HWND) {
  let result = with_app(|a| a.updates.result.take()).flatten();
  let Some(result) = result else { return };
  with_app(|a| a.dialog_open = true);
  match result {
    Err(e) => {
      crate::dialogs::prompt(hwnd, "Обновления", "Проверка обновлений", &e, true);
    }
    Ok(ResultItem::Checked(None)) => {
      crate::dialogs::prompt(
        hwnd,
        "Обновления",
        &crate::version::title(),
        "Более новых опубликованных выпусков не найдено.",
        true,
      );
    }
    Ok(ResultItem::Checked(Some(release))) => {
      let title = format!(
        "Доступна {}{}",
        release.tag_name,
        if release.prerelease {
          " · предварительная версия"
        } else {
          ""
        }
      );
      crate::dialogs::prompt(
        hwnd,
        "Что нового перед обновлением",
        &title,
        release
          .body
          .as_deref()
          .unwrap_or("Описание изменений не опубликовано."),
        true,
      );
      if MessageBoxW(hwnd,wide("Скачать установщик этого выпуска? Перед запуском будет проверена контрольная сумма SHA-256 с GitHub.").as_ptr(),wide(&title).as_ptr(),MB_YESNO|MB_ICONQUESTION)==IDYES {
        with_app(|a|{
          let (sender,receiver)=mpsc::channel();a.updates.receiver=Some(receiver);a.status="Загружаю и проверяю установщик…".into();
          std::thread::spawn(move||{let _=sender.send(crate::updates::download(&release).map(ResultItem::Downloaded));});SetTimer(hwnd,8,200,None);
        });
      }
    }
    Ok(ResultItem::Downloaded(path)) => {
      let dirty = with_app(|a| a.editor.dirty()).unwrap_or(true);
      if dirty {
        crate::dialogs::prompt(hwnd,"Обновление загружено","Сначала сохраните текущий документ.",&format!("Проверенный установщик сохранён:\n{}\n\nПосле сохранения и закрытия документов запустите этот файл.",path.display()),true);
      }else if MessageBoxW(hwnd,wide("Установщик загружен и проверен. Запустить его?\nПеред обновлением сохраните документы в других окнах Astra PDF.").as_ptr(),wide("Установка обновления").as_ptr(),MB_YESNO|MB_ICONQUESTION)==IDYES {
        let result=ShellExecuteW(hwnd,wide("open").as_ptr(),wide(&path.to_string_lossy()).as_ptr(),ptr::null(),ptr::null(),SW_SHOWNORMAL) as isize;
        if result>32{PostMessageW(hwnd,WM_CLOSE,0,0);}else{crate::dialogs::prompt(hwnd,"Обновление","Не удалось запустить установщик.",&path.display().to_string(),true);}
      }
    }
  }
  with_app(|a| a.dialog_open = false);
}

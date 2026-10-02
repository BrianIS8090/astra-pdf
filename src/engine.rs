use crate::{
  layers::{Layer, Layers},
  model::{Region, RenderKey},
  pdf::{Api, Pdf, Raster},
};
use std::{
  io::{Read, Write},
  os::windows::{
    ffi::{OsStrExt, OsStringExt},
    io::AsRawHandle,
    process::CommandExt,
  },
  path::{Path, PathBuf},
  process::{Child, Command, Stdio},
  ptr,
  sync::{mpsc, Arc},
  thread,
  time::{Duration, Instant},
};
use windows_sys::Win32::{
  Foundation::*,
  System::{JobObjects::*, Threading::CREATE_NO_WINDOW},
};

const MAX_RESPONSE: usize = 100 * 1024 * 1024;
const MAX_REQUEST: usize = 1024 * 1024;
const MAX_ITEMS: usize = 100_000;
const MAX_FILE: u64 = 512 * 1024 * 1024;
pub const MEMORY_LIMIT: usize = 1024 * 1024 * 1024;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Metadata {
  pub fingerprint: [u8; 32],
  pub sizes: Vec<(f64, f64)>,
  pub layers: Vec<Layer>,
  pub groups: Vec<Vec<lopdf::ObjectId>>,
  pub warning: Option<String>,
}

enum Request {
  Open(PathBuf),
  Render(RenderKey, bool),
  Probe(u8),
}

enum Reply {
  Opened(Metadata),
  Image(Raster),
  Error(String),
  Ready,
}

struct Packet(Vec<u8>);
impl Packet {
  fn new(kind: u8) -> Self {
    Self(vec![b'A', b'P', b'D', b'F', 2, kind])
  }
  fn byte(&mut self, value: u8) {
    self.0.push(value);
  }
  fn u32(&mut self, value: u32) {
    self.0.extend_from_slice(&value.to_le_bytes());
  }
  fn f64(&mut self, value: f64) {
    self.0.extend_from_slice(&value.to_le_bytes());
  }
  fn bytes(&mut self, value: &[u8]) {
    self.u32(value.len() as u32);
    self.0.extend_from_slice(value);
  }
  fn text(&mut self, value: &str) {
    self.bytes(value.as_bytes());
  }
  fn id(&mut self, id: lopdf::ObjectId) {
    self.u32(id.0);
    self.u32(id.1 as u32);
  }
}

struct Cursor<'a> {
  bytes: &'a [u8],
  offset: usize,
}
impl<'a> Cursor<'a> {
  fn new(bytes: &'a [u8]) -> Result<(Self, u8), String> {
    if bytes.len() < 6 || bytes[..5] != [b'A', b'P', b'D', b'F', 2] {
      return Err("Несовместимый протокол движка PDF.".into());
    }
    Ok((Self { bytes, offset: 6 }, bytes[5]))
  }
  fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
    let end = self
      .offset
      .checked_add(len)
      .ok_or("Переполнение длины ответа PDF.")?;
    let value = self
      .bytes
      .get(self.offset..end)
      .ok_or("Неполный ответ движка PDF.")?;
    self.offset = end;
    Ok(value)
  }
  fn byte(&mut self) -> Result<u8, String> {
    Ok(self.take(1)?[0])
  }
  fn boolean(&mut self) -> Result<bool, String> {
    match self.byte()? {
      0 => Ok(false),
      1 => Ok(true),
      _ => Err("Неверный флаг ответа PDF.".into()),
    }
  }
  fn u32(&mut self) -> Result<u32, String> {
    Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
  }
  fn f64(&mut self) -> Result<f64, String> {
    Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
  }
  fn count(&mut self, max: usize) -> Result<usize, String> {
    let value = self.u32()? as usize;
    if value > max {
      return Err("Превышен предел данных движка PDF.".into());
    }
    Ok(value)
  }
  fn text(&mut self) -> Result<String, String> {
    let len = self.count(65536)?;
    String::from_utf8(self.take(len)?.to_vec()).map_err(|_| "Некорректный текст ответа PDF.".into())
  }
  fn id(&mut self) -> Result<lopdf::ObjectId, String> {
    Ok((self.u32()?, self.count(u16::MAX as usize)? as u16))
  }
  fn end(&self) -> Result<(), String> {
    if self.offset == self.bytes.len() {
      Ok(())
    } else {
      Err("Лишние данные в ответе движка PDF.".into())
    }
  }
}

fn write_frame(out: &mut impl Write, bytes: &[u8], max: usize) -> Result<(), String> {
  if bytes.len() > max {
    return Err("Превышен размер сообщения движка PDF.".into());
  }
  out
    .write_all(&(bytes.len() as u32).to_le_bytes())
    .and_then(|()| out.write_all(bytes))
    .and_then(|()| out.flush())
    .map_err(|e| format!("Канал движка PDF закрыт: {e}"))
}

fn read_frame(input: &mut impl Read, max: usize) -> Result<Vec<u8>, String> {
  let mut header = [0; 4];
  input
    .read_exact(&mut header)
    .map_err(|_| "Движок PDF завершился или потерял соединение.".to_string())?;
  let len = u32::from_le_bytes(header) as usize;
  if !(6..=max).contains(&len) {
    return Err("Недопустимая длина сообщения движка PDF.".into());
  }
  let mut bytes = Vec::new();
  bytes
    .try_reserve_exact(len)
    .map_err(|_| "Недостаточно памяти для ответа PDF.")?;
  bytes.resize(len, 0);
  input
    .read_exact(&mut bytes)
    .map_err(|_| "Движок PDF прервал передачу данных.".to_string())?;
  Ok(bytes)
}

impl Request {
  fn encode(&self) -> Vec<u8> {
    let mut p = match self {
      Self::Open(_) => Packet::new(1),
      Self::Render(_, _) => Packet::new(2),
      Self::Probe(_) => Packet::new(3),
    };
    match self {
      Self::Open(path) => {
        let path: Vec<u16> = path.as_os_str().encode_wide().collect();
        p.u32(path.len() as u32);
        for c in path {
          p.0.extend_from_slice(&c.to_le_bytes());
        }
      }
      Self::Render(key, printing) => {
        p.u32(key.page as u32);
        p.u32(key.width as u32);
        p.u32(key.height as u32);
        p.u32(key.rotation as u32);
        p.byte(*printing as u8);
        p.byte(key.region.is_some() as u8);
        if let Some(r) = key.region {
          for value in [r.x, r.y, r.width, r.height] {
            p.u32(value as u32);
          }
        }
        p.u32(key.states.len() as u32);
        for s in &key.states {
          p.byte(*s as u8);
        }
      }
      Self::Probe(mode) => p.byte(*mode),
    }
    p.0
  }
  fn decode(bytes: &[u8]) -> Result<Self, String> {
    let (mut c, kind) = Cursor::new(bytes)?;
    let request = match kind {
      1 => {
        let len = c.count(32768)?;
        let mut units = Vec::with_capacity(len);
        for _ in 0..len {
          units.push(u16::from_le_bytes(c.take(2)?.try_into().unwrap()));
        }
        Self::Open(std::ffi::OsString::from_wide(&units).into())
      }
      2 => {
        let page = c.count(MAX_ITEMS)?;
        let width = c.count(1_000_000)? as i32;
        let height = c.count(1_000_000)? as i32;
        let rotation = c.count(3)? as i32;
        let printing = c.boolean()?;
        let region = if c.boolean()? {
          Some(Region {
            x: c.count(1_000_000)? as i32,
            y: c.count(1_000_000)? as i32,
            width: c.count(1_000_000)? as i32,
            height: c.count(1_000_000)? as i32,
          })
        } else {
          None
        };
        let count = c.count(MAX_ITEMS)?;
        let mut states = Vec::with_capacity(count);
        for _ in 0..count {
          states.push(c.boolean()?);
        }
        if width == 0
          || height == 0
          || !region
            .unwrap_or(Region {
              x: 0,
              y: 0,
              width,
              height,
            })
            .valid((width, height))
          || (printing && region.is_some())
        {
          return Err("Недопустимый размер страницы.".into());
        }
        Self::Render(
          RenderKey {
            page,
            width,
            height,
            rotation,
            states,
            region,
          },
          printing,
        )
      }
      3 => Self::Probe(c.byte()?),
      _ => return Err("Неизвестная команда движка PDF.".into()),
    };
    c.end()?;
    Ok(request)
  }
}

impl Reply {
  fn encode(&self) -> Vec<u8> {
    let mut p = match self {
      Self::Error(_) => Packet::new(0),
      Self::Opened(_) => Packet::new(1),
      Self::Image(_) => Packet::new(2),
      Self::Ready => Packet::new(3),
    };
    match self {
      Self::Error(e) => p.text(e),
      Self::Opened(m) => {
        p.0.extend_from_slice(&m.fingerprint);
        p.u32(m.sizes.len() as u32);
        for (w, h) in &m.sizes {
          p.f64(*w);
          p.f64(*h);
        }
        p.u32(m.layers.len() as u32);
        for l in &m.layers {
          p.id(l.id);
          p.text(&l.name);
          p.byte(l.visible as u8);
          p.byte(l.locked as u8);
        }
        p.u32(m.groups.len() as u32);
        for g in &m.groups {
          p.u32(g.len() as u32);
          for id in g {
            p.id(*id);
          }
        }
        p.byte(m.warning.is_some() as u8);
        if let Some(w) = &m.warning {
          p.text(w);
        }
      }
      Self::Image(image) => {
        p.u32(image.width as u32);
        p.u32(image.height as u32);
        p.bytes(&image.pixels);
      }
      Self::Ready => (),
    }
    p.0
  }
  fn decode(bytes: &[u8]) -> Result<Self, String> {
    let (mut c, kind) = Cursor::new(bytes)?;
    let response = match kind {
      0 => Self::Error(c.text()?),
      1 => {
        let fingerprint = c.take(32)?.try_into().unwrap();
        let n = c.count(MAX_ITEMS)?;
        if n == 0 {
          return Err("Движок вернул документ без страниц.".into());
        }
        let mut sizes = Vec::with_capacity(n);
        for _ in 0..n {
          let (w, h) = (c.f64()?, c.f64()?);
          if !w.is_finite() || !h.is_finite() || w <= 0. || h <= 0. {
            return Err("Неверный размер страницы в ответе PDF.".into());
          }
          sizes.push((w, h));
        }
        let n = c.count(MAX_ITEMS)?;
        let mut layers = Vec::with_capacity(n);
        for _ in 0..n {
          layers.push(Layer {
            id: c.id()?,
            name: c.text()?,
            visible: c.boolean()?,
            locked: c.boolean()?,
          });
        }
        let n = c.count(MAX_ITEMS)?;
        let mut groups = Vec::with_capacity(n);
        let mut total = 0usize;
        for _ in 0..n {
          let len = c.count(MAX_ITEMS)?;
          total += len;
          if total > MAX_ITEMS {
            return Err("Слишком много связей слоёв.".into());
          }
          let mut group = Vec::with_capacity(len);
          for _ in 0..len {
            group.push(c.id()?);
          }
          groups.push(group);
        }
        let warning = if c.boolean()? { Some(c.text()?) } else { None };
        Self::Opened(Metadata {
          fingerprint,
          sizes,
          layers,
          groups,
          warning,
        })
      }
      2 => {
        let width = c.count(24_000_000)? as i32;
        let height = c.count(24_000_000)? as i32;
        let len = c.count(96_000_000)?;
        if width == 0 || height == 0 || i64::from(width) * i64::from(height) * 4 != len as i64 {
          return Err("Размер изображения не совпадает с данными PDF.".into());
        }
        Self::Image(Raster {
          width,
          height,
          pixels: Arc::new(c.take(len)?.to_vec()),
        })
      }
      3 => Self::Ready,
      _ => return Err("Неизвестный ответ движка PDF.".into()),
    };
    c.end()?;
    Ok(response)
  }
}

struct Job(HANDLE);
impl Drop for Job {
  fn drop(&mut self) {
    unsafe {
      CloseHandle(self.0);
    }
  }
}
impl Job {
  fn peak_memory(&self) -> Result<usize, String> {
    unsafe {
      let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
      if QueryInformationJobObject(
        self.0,
        JobObjectExtendedLimitInformation,
        &mut info as *mut _ as _,
        std::mem::size_of_val(&info) as u32,
        ptr::null_mut(),
      ) == 0
      {
        return Err("Не удалось проверить ограничение памяти PDF.".into());
      }
      if info.ProcessMemoryLimit != MEMORY_LIMIT
        || info.BasicLimitInformation.ActiveProcessLimit != 1
      {
        return Err("Ограничения процесса PDF не совпадают с заданными.".into());
      }
      Ok(info.PeakProcessMemoryUsed)
    }
  }
  fn attach(child: &Child) -> Result<Self, String> {
    unsafe {
      let handle = CreateJobObjectW(ptr::null(), ptr::null());
      if handle.is_null() {
        return Err("Не удалось создать ограничение ресурсов PDF.".into());
      }
      let job = Self(handle);
      let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
      limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
      limits.BasicLimitInformation.ActiveProcessLimit = 1;
      limits.ProcessMemoryLimit = MEMORY_LIMIT;
      if SetInformationJobObject(
        handle,
        JobObjectExtendedLimitInformation,
        &limits as *const _ as _,
        std::mem::size_of_val(&limits) as u32,
      ) == 0
        || AssignProcessToJobObject(handle, child.as_raw_handle() as _) == 0
      {
        return Err(format!(
          "Windows не применила ограничение ресурсов PDF: {}",
          std::io::Error::last_os_error()
        ));
      }
      Ok(job)
    }
  }
}

pub struct Client {
  dll: PathBuf,
  opened: Option<(PathBuf, [u8; 32])>,
  child: Child,
  _job: Job,
  requests: Option<mpsc::SyncSender<Vec<u8>>>,
  replies: mpsc::Receiver<Result<Vec<u8>, String>>,
  io: Option<thread::JoinHandle<()>>,
  healthy: bool,
}

impl Client {
  pub fn spawn(dll: &Path) -> Result<Self, String> {
    Self::spawn_inner(dll, false)
  }
  fn spawn_inner(dll: &Path, diagnostics: bool) -> Result<Self, String> {
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
      .arg(if diagnostics {
        "--engine-diagnostics"
      } else {
        "--engine"
      })
      .env("ASTRA_PDFIUM_PATH", dll)
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::null())
      .creation_flags(CREATE_NO_WINDOW);
    let mut child = command
      .spawn()
      .map_err(|e| format!("Не удалось запустить движок PDF: {e}"))?;
    let job = match Job::attach(&child) {
      Ok(j) => j,
      Err(e) => {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
      }
    };
    let mut input = child.stdin.take().ok_or("Недоступен вход движка PDF.")?;
    let mut output = child.stdout.take().ok_or("Недоступен выход движка PDF.")?;
    let (requests, incoming) = mpsc::sync_channel::<Vec<u8>>(1);
    let (replies, receiver) = mpsc::sync_channel(1);
    let io = thread::spawn(move || {
      while let Ok(request) = incoming.recv() {
        let result = write_frame(&mut input, &request, MAX_REQUEST)
          .and_then(|()| read_frame(&mut output, MAX_RESPONSE));
        let failed = result.is_err();
        if replies.send(result).is_err() || failed {
          break;
        }
      }
    });
    Ok(Self {
      dll: dll.into(),
      opened: None,
      child,
      _job: job,
      requests: Some(requests),
      replies: receiver,
      io: Some(io),
      healthy: true,
    })
  }
  pub fn healthy(&self) -> bool {
    self.healthy
  }
  fn stop(&mut self) {
    self.healthy = false;
    self.requests.take();
    let _ = self.child.kill();
    let _ = self.child.wait();
  }
  fn call(
    &mut self,
    request: Request,
    timeout: Duration,
    cancelled: impl Fn() -> bool,
  ) -> Result<Reply, String> {
    if !self.healthy {
      return Err("Движок PDF остановлен. Откройте документ повторно.".into());
    }
    if cancelled() {
      self.stop();
      return Err("Операция PDF отменена.".into());
    }
    self
      .requests
      .as_ref()
      .ok_or("Движок PDF остановлен.")?
      .try_send(request.encode())
      .map_err(|_| "Канал движка PDF занят или закрыт.".to_string())?;
    let started = Instant::now();
    let response = loop {
      if cancelled() {
        self.stop();
        return Err("Операция PDF отменена.".into());
      }
      if started.elapsed() >= timeout {
        self.stop();
        return Err(
          "Движок PDF не ответил за отведённое время. Можно открыть другой документ.".into(),
        );
      }
      match self.replies.recv_timeout(Duration::from_millis(20)) {
        Ok(result) => break result,
        Err(mpsc::RecvTimeoutError::Timeout) => (),
        Err(_) => break Err("Соединение с движком PDF потеряно.".into()),
      }
    };
    let decoded = response.and_then(|bytes| Reply::decode(&bytes));
    match decoded {
      Ok(Reply::Error(e)) => Err(e),
      Ok(reply) => Ok(reply),
      Err(e) => {
        self.stop();
        Err(format!("{e} Можно открыть документ повторно."))
      }
    }
  }
  pub fn open(&mut self, path: &Path, cancelled: impl Fn() -> bool) -> Result<Metadata, String> {
    self.opened = None;
    match self.call(Request::Open(path.into()), REQUEST_TIMEOUT, cancelled)? {
      Reply::Opened(m) => {
        self.opened = Some((path.into(), m.fingerprint));
        Ok(m)
      }
      _ => {
        self.stop();
        Err("Движок вернул неверный ответ открытия.".into())
      }
    }
  }
  pub fn render(
    &mut self,
    key: &RenderKey,
    printing: bool,
    cancelled: impl Fn() -> bool,
  ) -> Result<Raster, String> {
    if !self.healthy {
      let (path, fingerprint) = self.opened.clone().ok_or("Откройте документ повторно.")?;
      let mut replacement = Self::spawn(&self.dll)?;
      let metadata = replacement.open(&path, &cancelled)?;
      if metadata.fingerprint != fingerprint {
        return Err(
          "Файл изменён другой программой. Откройте его повторно перед продолжением.".into(),
        );
      }
      *self = replacement;
    }
    match self.call(
      Request::Render(key.clone(), printing),
      REQUEST_TIMEOUT,
      cancelled,
    )? {
      Reply::Image(image) if (image.width, image.height) == key.raster_size() => Ok(image),
      _ => {
        self.stop();
        Err("Движок вернул неверный ответ отрисовки.".into())
      }
    }
  }
}
impl Drop for Client {
  fn drop(&mut self) {
    self.stop();
    // После уничтожения процесса чтение и запись в его каналы завершаются.
    while self.replies.try_recv().is_ok() {}
    if let Some(io) = self.io.take() {
      let _ = io.join();
    }
  }
}

pub fn serve(diagnostics: bool) -> Result<(), String> {
  use windows_sys::Win32::System::Diagnostics::Debug::{
    SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX,
  };
  unsafe {
    SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
  }
  let mut input = std::io::stdin().lock();
  let mut output = std::io::stdout().lock();
  let mut api = None;
  let mut current: Option<Pdf> = None;
  let mut layers: Option<Layers> = None;
  let mut applied: Option<Vec<bool>> = None;
  while let Ok(frame) = read_frame(&mut input, MAX_REQUEST) {
    let request = match Request::decode(&frame) {
      Ok(r) => r,
      Err(e) => {
        write_frame(&mut output, &Reply::Error(e).encode(), MAX_RESPONSE)?;
        continue;
      }
    };
    let result: Result<Reply, String> = (|| match request {
      Request::Open(path) => {
        current = None;
        layers = None;
        applied = None;
        let file =
          std::fs::File::open(path).map_err(|e| format!("Не удалось открыть файл: {e}"))?;
        if file.metadata().map_err(|e| e.to_string())?.len() > MAX_FILE {
          return Err("Документ превышает предел 512 МиБ.".into());
        }
        let mut bytes = Vec::new();
        file
          .take(MAX_FILE + 1)
          .read_to_end(&mut bytes)
          .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_FILE {
          return Err("Документ превышает предел 512 МиБ.".into());
        }
        let api = match &api {
          Some(a) => a,
          None => {
            api = Some(Api::new(&crate::pdfium_path())?);
            api.as_ref().unwrap()
          }
        };
        let bytes = Arc::new(bytes);
        use sha2::{Digest, Sha256};
        let fingerprint: [u8; 32] = Sha256::digest(bytes.as_ref()).into();
        let pdf = Pdf::open(api.clone(), bytes.clone())?;
        if pdf.sizes.len() > MAX_ITEMS {
          return Err("В документе слишком много страниц.".into());
        }
        let (items, groups, warning) = match Layers::read(&bytes) {
          Ok(value) if value.items.len() <= MAX_ITEMS => {
            let items = value.items.clone();
            let groups = value.radio_groups.clone();
            if !items.is_empty() {
              layers = Some(value);
            }
            (items, groups, None)
          }
          Ok(_) => (
            vec![],
            vec![],
            Some("Слишком много слоёв для отображения.".into()),
          ),
          Err(e) => (vec![], vec![], Some(e)),
        };
        let sizes = pdf.sizes.clone();
        current = Some(pdf);
        Ok(Reply::Opened(Metadata {
          fingerprint,
          sizes,
          layers: items,
          groups,
          warning,
        }))
      }
      Request::Render(key, printing) => {
        if !key.states.is_empty() && applied.as_deref() != Some(&key.states) {
          let source = layers.as_ref().ok_or("В документе нет доступных слоёв.")?;
          let is_default = source
            .items
            .iter()
            .map(|l| l.visible)
            .eq(key.states.iter().copied());
          if applied.is_some() || !is_default || printing {
            let bytes = source.with_states(&key.states)?;
            current = Some(Pdf::open(
              api.as_ref().ok_or("Движок не загружен.")?.clone(),
              Arc::new(bytes),
            )?);
            applied = Some(key.states.clone());
          }
        }
        Ok(Reply::Image(
          current
            .as_ref()
            .ok_or("Документ не открыт.")?
            .render_region(
              key.page,
              (key.width, key.height),
              key.rotation,
              printing,
              key.region,
            )?,
        ))
      }
      Request::Probe(mode) if diagnostics => {
        match mode {
          1 => std::process::exit(73),
          2 => thread::sleep(Duration::from_secs(60)),
          3 => {
            let mut chunks = Vec::new();
            loop {
              let chunk = vec![42u8; 16 * 1024 * 1024];
              std::hint::black_box(&chunk);
              chunks.push(chunk);
            }
          }
          _ => (),
        }
        Ok(Reply::Ready)
      }
      Request::Probe(_) => Err("Диагностические команды отключены.".into()),
    })();
    let reply = match result {
      Ok(reply) => reply,
      Err(e) => Reply::Error(e),
    };
    write_frame(&mut output, &reply.encode(), MAX_RESPONSE)?;
  }
  Ok(())
}

pub fn self_test(path: &Path, report: &Path) -> Result<(), String> {
  let dll = crate::pdfium_path();
  let mut client = Client::spawn(&dll)?;
  let meta = client.open(path, || false)?;
  let key = RenderKey {
    page: 0,
    width: 600,
    height: 800,
    rotation: 0,
    states: meta.layers.iter().map(|l| l.visible).collect(),
    region: None,
  };
  let image = client.render(&key, false, || false)?;
  let mut tile_key = key.clone();
  tile_key.region = Some(Region {
    x: 130,
    y: 190,
    width: 180,
    height: 210,
  });
  let tile = client.render(&tile_key, false, || false)?;
  for row in 0..210 {
    let start = ((190 + row) * 600 + 130) * 4;
    if tile.pixels[row * 720..(row + 1) * 720] != image.pixels[start..start + 720] {
      return Err("Участок через канал движка не совпал с полной страницей.".into());
    }
  }
  if image.pixels.iter().all(|v| *v == 255) {
    return Err("Тестовая страница оказалась пустой.".into());
  }
  drop(client);
  let mut peak_memory = 0;
  for mode in [1, 2, 3] {
    let mut failed = Client::spawn_inner(&dll, true)?;
    failed.open(path, || false)?;
    let timeout = if mode == 2 {
      Duration::from_millis(150)
    } else {
      Duration::from_secs(10)
    };
    let error = failed
      .call(Request::Probe(mode), timeout, || false)
      .err()
      .unwrap_or_default();
    if error.is_empty() || failed.healthy() {
      return Err(format!("Не обнаружен сбой движка {mode}."));
    }
    if mode == 1
      && failed
        .child
        .try_wait()
        .map_err(|e| e.to_string())?
        .and_then(|s| s.code())
        != Some(73)
    {
      return Err("Проверка не достигла принудительного сбоя движка.".into());
    }
    if mode == 2 && !error.contains("не ответил") {
      return Err("Не подтверждено ограничение времени ожидания.".into());
    }
    if mode == 3 {
      peak_memory = failed._job.peak_memory()?;
      if !(MEMORY_LIMIT - 32 * 1024 * 1024..=MEMORY_LIMIT).contains(&peak_memory) {
        return Err(format!(
          "Не подтверждено достижение лимита памяти: {peak_memory} байт."
        ));
      }
    }
    let recovered = failed.render(&key, false, || false)?;
    if recovered.pixels != image.pixels {
      return Err("После сбоя изменилось изображение документа.".into());
    }
  }
  let mut cancelled = Client::spawn_inner(&dll, true)?;
  cancelled.open(path, || false)?;
  let start = Instant::now();
  if cancelled
    .call(Request::Probe(2), Duration::from_secs(5), || {
      start.elapsed() > Duration::from_millis(150)
    })
    .is_ok()
    || cancelled.healthy()
  {
    return Err("Не сработала отмена движка.".into());
  }
  if cancelled.render(&key, false, || false)?.pixels != image.pixels {
    return Err("Просмотр не восстановился после отмены.".into());
  }
  let temporary = report.with_extension(format!("changed-{}.pdf", std::process::id()));
  let mut copy = std::fs::OpenOptions::new()
    .write(true)
    .create_new(true)
    .open(&temporary)
    .map_err(|e| e.to_string())?;
  copy
    .write_all(&std::fs::read(path).map_err(|e| e.to_string())?)
    .map_err(|e| e.to_string())?;
  drop(copy);
  let check = (|| {
    let mut client = Client::spawn(&dll)?;
    client.open(&temporary, || false)?;
    client.stop();
    let mut file = std::fs::OpenOptions::new()
      .append(true)
      .open(&temporary)
      .map_err(|e| e.to_string())?;
    file
      .write_all("\n% изменено другой программой\n".as_bytes())
      .map_err(|e| e.to_string())?;
    drop(file);
    if !client
      .render(&key, false, || false)
      .err()
      .unwrap_or_default()
      .contains("изменён другой программой")
    {
      return Err("Не обнаружена подмена документа при восстановлении.".to_string());
    }
    Ok(())
  })();
  let _ = std::fs::remove_file(&temporary);
  check?;
  std::fs::write(report, format!("{{\"open_render\":true,\"crash_recovery\":true,\"timeout_recovery\":true,\"memory_limit_recovery\":true,\"peak_worker_bytes\":{peak_memory},\"cancellation\":true,\"changed_source_rejected\":true}}")).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn protocol_rejects_truncation_and_oversized_frames() {
    assert!(read_frame(&mut &u32::MAX.to_le_bytes()[..], MAX_RESPONSE).is_err());
    assert!(Reply::decode(b"APDF\x01\x02").is_err());
    let mut p = Packet::new(2);
    p.u32(600);
    p.u32(800);
    p.bytes(&[0; 12]);
    assert!(Reply::decode(&p.0).is_err());
    assert!(Request::decode(b"APDF\x02\x01").is_err());
  }
  #[test]
  fn protocol_round_trips_unicode_and_layer_states() {
    let path = PathBuf::from("C:\\Документы\\схема 1.pdf");
    match Request::decode(&Request::Open(path.clone()).encode()).unwrap() {
      Request::Open(actual) => assert_eq!(actual, path),
      _ => panic!("Неверная команда"),
    }
    let m = Metadata {
      fingerprint: [17; 32],
      sizes: vec![(600., 800.)],
      layers: vec![Layer {
        id: (3, 0),
        name: "Размеры".into(),
        visible: false,
        locked: true,
      }],
      groups: vec![vec![(3, 0)]],
      warning: None,
    };
    match Reply::decode(&Reply::Opened(m).encode()).unwrap() {
      Reply::Opened(m) => {
        assert_eq!(m.layers[0].name, "Размеры");
        assert!(!m.layers[0].visible);
        assert!(m.layers[0].locked);
      }
      _ => panic!("Неверный ответ"),
    }
  }
  #[test]
  fn protocol_accepts_bounded_tiles_and_rejects_invalid_regions() {
    let mut key = RenderKey {
      page: 0,
      width: 500_000,
      height: 700_000,
      rotation: 3,
      states: vec![true, false],
      region: Some(Region {
        x: 499_000,
        y: 699_000,
        width: 768,
        height: 768,
      }),
    };
    match Request::decode(&Request::Render(key.clone(), false).encode()).unwrap() {
      Request::Render(actual, false) => assert_eq!(actual, key),
      _ => panic!("Неверный ответ"),
    }
    assert!(Request::decode(&Request::Render(key.clone(), true).encode()).is_err());
    for region in [
      None,
      Some(Region {
        x: 499_999,
        y: 0,
        width: 2,
        height: 2,
      }),
      Some(Region {
        x: -1,
        y: 0,
        width: 2,
        height: 2,
      }),
      Some(Region {
        x: 0,
        y: 0,
        width: 10000,
        height: 10000,
      }),
    ] {
      key.region = region;
      assert!(Request::decode(&Request::Render(key.clone(), false).encode()).is_err());
    }
  }

  #[test]
  fn protocol_rejects_nonfinite_sizes_and_bad_flags() {
    let mut p = Packet::new(1);
    p.0.extend_from_slice(&[0; 32]);
    p.u32(1);
    p.f64(f64::NAN);
    p.f64(800.);
    assert!(Reply::decode(&p.0).is_err());
    let mut p = Request::Render(
      RenderKey {
        page: 0,
        width: 600,
        height: 800,
        rotation: 0,
        states: vec![true],
        region: None,
      },
      false,
    )
    .encode();
    *p.last_mut().unwrap() = 9;
    assert!(Request::decode(&p).is_err());
  }
}

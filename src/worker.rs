use crate::{
  engine::Client,
  layers::Layer,
  model::{Cache, RenderKey},
  pdf::Raster,
  printing::{self, PrintJob},
};
use std::{
  collections::VecDeque,
  path::PathBuf,
  sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc, Condvar, Mutex,
  },
  thread,
  time::Instant,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const READY: u32 = WM_APP + 1;
pub enum Command {
  Open {
    path: PathBuf,
    generation: u64,
  },
  Render {
    generation: u64,
    ticket: u64,
    key: RenderKey,
  },
  Thumbnail {
    generation: u64,
    ticket: u64,
    key: RenderKey,
  },
  Print {
    generation: u64,
    states: Vec<bool>,
    rotation: i32,
    job: PrintJob,
  },
  Quit,
}
pub enum Event {
  Loaded {
    fingerprint: [u8; 32],
    generation: u64,
    sizes: Vec<(f64, f64)>,
    millis: u128,
  },
  Layers {
    generation: u64,
    items: Vec<Layer>,
    groups: Vec<Vec<lopdf::ObjectId>>,
    warning: Option<String>,
  },
  Rendered {
    generation: u64,
    ticket: u64,
    key: RenderKey,
    image: Raster,
  },
  Thumbnail {
    generation: u64,
    ticket: u64,
    key: RenderKey,
    image: Option<Raster>,
  },
  Error {
    generation: u64,
    message: String,
  },
  PrintProgress {
    page: usize,
    total: usize,
  },
  Printed(Result<(), String>),
}

#[derive(Default)]
struct Pending {
  open: Option<Command>,
  render: VecDeque<Command>,
  thumbnails: VecDeque<Command>,
  print: Option<Command>,
  closed: bool,
}
struct Queue {
  pending: Mutex<Pending>,
  changed: Condvar,
  generation: AtomicU64,
}

pub struct CommandSender(Arc<Queue>);
impl CommandSender {
  pub fn render_batch(
    &self,
    generation: u64,
    ticket: u64,
    keys: Vec<RenderKey>,
    thumbnails: bool,
  ) -> Result<(), ()> {
    let mut pending = self.0.pending.lock().unwrap_or_else(|e| e.into_inner());
    if pending.closed {
      return Err(());
    }
    let queue = if thumbnails {
      &mut pending.thumbnails
    } else {
      &mut pending.render
    };
    queue.clear();
    queue.extend(keys.into_iter().take(128).map(|key| {
      if thumbnails {
        Command::Thumbnail {
          generation,
          ticket,
          key,
        }
      } else {
        Command::Render {
          generation,
          ticket,
          key,
        }
      }
    }));
    self.0.changed.notify_one();
    Ok(())
  }
  pub fn send(&self, command: Command) -> Result<(), ()> {
    let mut pending = self.0.pending.lock().unwrap_or_else(|e| e.into_inner());
    if pending.closed {
      return Err(());
    }
    match command {
      Command::Open { generation, .. } => {
        self.0.generation.store(generation, Ordering::Relaxed);
        pending.open = Some(command);
        pending.render.clear();
        pending.thumbnails.clear();
      }
      Command::Render { .. } => {
        pending.render.clear();
        pending.render.push_back(command);
      }
      Command::Thumbnail { .. } => {
        pending.thumbnails.clear();
        pending.thumbnails.push_back(command);
      }
      Command::Print { .. } => {
        if pending.print.is_some() {
          return Err(());
        }
        pending.print = Some(command);
      }
      Command::Quit => {
        pending.closed = true;
        pending.open = None;
        pending.render.clear();
        pending.thumbnails.clear();
        pending.print = None;
      }
    }
    self.0.changed.notify_one();
    Ok(())
  }
}
impl Queue {
  fn next(&self) -> Option<Command> {
    let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
    loop {
      if pending.closed {
        return None;
      }
      if let Some(command) = pending
        .print
        .take()
        .or_else(|| pending.open.take())
        .or_else(|| pending.render.pop_front())
        .or_else(|| pending.thumbnails.pop_front())
      {
        return Some(command);
      }
      pending = self
        .changed
        .wait(pending)
        .unwrap_or_else(|e| e.into_inner());
    }
  }
  fn stopped(&self) -> bool {
    self
      .pending
      .lock()
      .unwrap_or_else(|e| e.into_inner())
      .closed
  }
}

pub struct Worker {
  pub sender: CommandSender,
  pub receiver: mpsc::Receiver<Event>,
  pub latest: Arc<AtomicU64>,
  pub latest_thumbnail: Arc<AtomicU64>,
}
impl Worker {
  pub fn start(hwnd: usize, dll: PathBuf) -> Self {
    let queue = Arc::new(Queue {
      pending: Mutex::new(Pending::default()),
      changed: Condvar::new(),
      generation: AtomicU64::new(0),
    });
    let sender = CommandSender(queue.clone());
    let (results, receiver) = mpsc::sync_channel(8);
    let latest = Arc::new(AtomicU64::new(0));
    let active = latest.clone();
    let latest_thumbnail = Arc::new(AtomicU64::new(0));
    let thumbnail_active = latest_thumbnail.clone();
    thread::spawn(move || {
      let send = |event| {
        let sent = if matches!(&event, Event::PrintProgress { .. }) {
          results.try_send(event).is_ok()
        } else {
          results.send(event).is_ok()
        };
        if sent {
          unsafe {
            PostMessageW(hwnd as _, READY, 0, 0);
          }
        }
      };
      let mut client: Option<Client> = None;
      let mut sizes = vec![];
      let mut generation = 0;
      let mut cache = Cache::new(64 * 1024 * 1024);
      while let Some(command) = queue.next() {
        match command {
          Command::Quit => break,
          Command::Open {
            path,
            generation: id,
          } => {
            generation = id;
            sizes.clear();
            client = None;
            cache = Cache::new(64 * 1024 * 1024);
            let start = Instant::now();
            let result = Client::spawn(&dll).and_then(|mut engine| {
              let meta = engine.open(&path, || {
                queue.stopped() || queue.generation.load(Ordering::Relaxed) != id
              })?;
              client = Some(engine);
              Ok(meta)
            });
            if queue.stopped() {
              break;
            }
            if queue.generation.load(Ordering::Relaxed) != id {
              continue;
            }
            match result {
              Ok(meta) => {
                sizes = meta.sizes;
                send(Event::Loaded {
                  fingerprint: meta.fingerprint,
                  generation,
                  sizes: sizes.clone(),
                  millis: start.elapsed().as_millis(),
                });
                send(Event::Layers {
                  generation,
                  items: meta.layers,
                  groups: meta.groups,
                  warning: meta.warning,
                });
              }
              Err(message) => send(Event::Error {
                generation,
                message,
              }),
            }
          }
          Command::Render {
            generation: id,
            ticket,
            key,
          } => {
            if id != generation || ticket != active.load(Ordering::Relaxed) {
              continue;
            }
            let result = if let Some(image) = cache.get(&key) {
              Ok(image)
            } else {
              client
                .as_mut()
                .ok_or("Документ не открыт.".into())
                .and_then(|engine| {
                  engine.render(&key, false, || {
                    queue.stopped() || queue.generation.load(Ordering::Relaxed) != id
                  })
                })
            };
            if queue.stopped() {
              break;
            }
            match result {
              Ok(image) => {
                cache.put(key.clone(), image.clone());
                if ticket != active.load(Ordering::Relaxed) {
                  continue;
                }
                send(Event::Rendered {
                  generation,
                  ticket,
                  key,
                  image,
                });
              }
              Err(message) if ticket == active.load(Ordering::Relaxed) => send(Event::Error {
                generation,
                message,
              }),
              _ => (),
            }
          }
          Command::Thumbnail {
            generation: id,
            ticket,
            key,
          } => {
            if id != generation || ticket != thumbnail_active.load(Ordering::Relaxed) {
              continue;
            }
            let result = client.as_mut().and_then(|engine| {
              engine
                .render(&key, false, || {
                  queue.stopped() || queue.generation.load(Ordering::Relaxed) != id
                })
                .ok()
            });
            if !queue.stopped()
              && id == queue.generation.load(Ordering::Relaxed)
              && ticket == thumbnail_active.load(Ordering::Relaxed)
            {
              send(Event::Thumbnail {
                generation: id,
                ticket,
                key,
                image: result,
              });
            }
          }
          Command::Print {
            generation: id,
            states,
            rotation,
            job,
          } => {
            if id != generation {
              send(Event::Printed(Err(
                "Документ изменился до начала печати.".into(),
              )));
              continue;
            }
            let cancel = job.cancel.clone();
            let result = if let Some(engine) = client.as_mut() {
              printing::print(
                &sizes,
                job,
                rotation,
                |page, width, height, rotate| {
                  let key = RenderKey {
                    page,
                    width,
                    height,
                    rotation: rotate,
                    states: states.clone(),
                    region: None,
                  };
                  engine.render(&key, true, || {
                    queue.stopped() || cancel.load(Ordering::Relaxed)
                  })
                },
                |page, total| send(Event::PrintProgress { page, total }),
              )
            } else {
              Err("Документ не открыт.".into())
            };
            send(Event::Printed(result));
          }
        }
      }
    });
    Self {
      sender,
      receiver,
      latest,
      latest_thumbnail,
    }
  }
}
impl Drop for Worker {
  fn drop(&mut self) {
    self.latest.fetch_add(1, Ordering::Relaxed);
    let _ = self.sender.send(Command::Quit);
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn viewport_preempts_thumbnails_and_new_batch_replaces_old_work() {
    let queue = Arc::new(Queue {
      pending: Mutex::new(Pending::default()),
      changed: Condvar::new(),
      generation: AtomicU64::new(1),
    });
    let sender = CommandSender(queue.clone());
    let key = |page| RenderKey {
      page,
      width: 10,
      height: 10,
      rotation: 0,
      states: vec![],
      region: None,
    };
    sender
      .render_batch(1, 10, vec![key(0), key(1)], true)
      .unwrap();
    sender
      .render_batch(1, 20, vec![key(0), key(1)], false)
      .unwrap();
    sender
      .render_batch(1, 21, vec![key(5), key(6)], false)
      .unwrap();
    assert!(matches!(
      queue.next(),
      Some(Command::Render {
        ticket: 21,
        key: RenderKey { page: 5, .. },
        ..
      })
    ));
    assert!(matches!(
      queue.next(),
      Some(Command::Render {
        ticket: 21,
        key: RenderKey { page: 6, .. },
        ..
      })
    ));
    assert!(matches!(
      queue.next(),
      Some(Command::Thumbnail { ticket: 10, .. })
    ));
    sender
      .send(Command::Open {
        generation: 2,
        path: "next.pdf".into(),
      })
      .unwrap();
    assert!(queue.pending.lock().unwrap().thumbnails.is_empty());
    sender.send(Command::Quit).unwrap();
  }

  #[test]
  fn queue_keeps_only_latest_open_and_render() {
    let queue = Arc::new(Queue {
      pending: Mutex::new(Pending::default()),
      changed: Condvar::new(),
      generation: AtomicU64::new(0),
    });
    let sender = CommandSender(queue.clone());
    for generation in 1..=10_000 {
      sender
        .send(Command::Open {
          path: format!("{generation}.pdf").into(),
          generation,
        })
        .unwrap();
      sender
        .send(Command::Render {
          generation,
          ticket: generation,
          key: RenderKey {
            page: 0,
            width: 1,
            height: 1,
            rotation: 0,
            states: vec![],
            region: None,
          },
        })
        .unwrap();
    }
    assert!(matches!(
      queue.next(),
      Some(Command::Open {
        generation: 10_000,
        ..
      })
    ));
    assert!(matches!(
      queue.next(),
      Some(Command::Render { ticket: 10_000, .. })
    ));
    sender.send(Command::Quit).unwrap();
    assert!(queue.next().is_none());
    assert!(sender
      .send(Command::Open {
        path: "later.pdf".into(),
        generation: 10_001
      })
      .is_err());
  }
}

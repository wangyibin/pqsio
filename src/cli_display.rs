use anyhow::Result;
use pqsio::metadata::Value;
use std::ffi::c_void;
use std::io::{self, IsTerminal, Write};
use std::sync::mpsc::{self, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub fn table(name: &str, value: &Value) -> Result<()> {
    let mut out = io::stdout().lock();
    writeln!(
        out,
        "{}",
        if name == "info" {
            "PQS summary"
        } else {
            "PQS quality statistics"
        }
    )?;
    if let Some(fields) = value.object() {
        for (key, value) in fields {
            if key == "mapq_histogram" {
                if let Value::List(h) = value {
                    for (label, lo, hi) in [
                        ("MAPQ 0", 0, 1),
                        ("MAPQ 1–9", 1, 10),
                        ("MAPQ 10–19", 10, 20),
                        ("MAPQ 20–29", 20, 30),
                        ("MAPQ ≥30", 30, 256),
                    ] {
                        let n: u64 = h[lo..hi].iter().filter_map(Value::u64).sum();
                        writeln!(out, "{label:<30} {n}")?;
                    }
                }
            } else {
                let text = match value {
                    Value::String(s) => s
                        .chars()
                        .map(|c| {
                            if c.is_control() {
                                c.escape_default().to_string()
                            } else {
                                c.to_string()
                            }
                        })
                        .collect::<String>(),
                    Value::Null => "N/A".into(),
                    _ => value.json(),
                };
                writeln!(out, "{:<30} {}", key.replace('_', " "), text)?;
            }
        }
    }
    Ok(())
}

enum Event {
    Stage(String, u64, u64),
    Finish(Option<bool>),
}
pub struct Progress {
    sender: Option<Box<SyncSender<Event>>>,
    worker: Option<JoinHandle<()>>,
}
unsafe extern "C" fn callback(data: *const u8, len: usize, n: u64, total: u64, user: *mut c_void) {
    let sender = &*(user as *const SyncSender<Event>);
    let stage = String::from_utf8_lossy(std::slice::from_raw_parts(data, len)).into_owned();
    let _ = sender.try_send(Event::Stage(stage, n, total));
}
impl Progress {
    pub fn new(enabled: Option<bool>, name: &str) -> Self {
        let tty = io::stderr().is_terminal();
        if !enabled.unwrap_or(tty) {
            return Self {
                sender: None,
                worker: None,
            };
        }
        let (tx, rx) = mpsc::sync_channel(64);
        let mut sender = Box::new(tx);
        let name = name.to_owned();
        let worker = std::thread::spawn(move || {
            let start = Instant::now();
            let mut since = start;
            let mut stage = name;
            let mut completed = 0;
            let mut total = 0;
            let mut durations = Vec::new();
            let mut out = io::stderr();
            loop {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(Event::Stage(next, n, t)) => {
                        if next != stage {
                            durations
                                .push(format!("{stage}: {:.2}s", since.elapsed().as_secs_f64()));
                            since = Instant::now();
                            stage = next;
                            if !tty {
                                let _ = writeln!(out, "{stage}");
                            }
                        }
                        completed = n;
                        total = t;
                    }
                    Ok(Event::Finish(ok)) => {
                        durations.push(format!("{stage}: {:.2}s", since.elapsed().as_secs_f64()));
                        if tty {
                            let _ = write!(out, "\r\x1b[2K");
                        }
                        let Some(ok) = ok else { break };
                        let _ = writeln!(
                            out,
                            "{} in {:.2}s — {}",
                            if ok { "Done" } else { "Stopped" },
                            start.elapsed().as_secs_f64(),
                            durations.join("; ")
                        );
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => (),
                }
                if tty {
                    let counter = if total > 0 {
                        format!("{completed}/{total}")
                    } else {
                        completed.to_string()
                    };
                    let _ = write!(
                        out,
                        "\r\x1b[2K{stage}  {counter}  {:.1}s",
                        start.elapsed().as_secs_f64()
                    );
                    let _ = out.flush();
                }
            }
        });
        // Box keeps the sender address stable until this guard clears registration.
        unsafe {
            pqsio::progress::set(
                Some(callback),
                (&mut *sender as *mut SyncSender<Event>).cast(),
            );
        }
        Self {
            sender: Some(sender),
            worker: Some(worker),
        }
    }
    pub fn finish(mut self, ok: Option<bool>) {
        self.close(ok);
    }
    fn close(&mut self, ok: Option<bool>) {
        if let Some(sender) = self.sender.take() {
            unsafe {
                pqsio::progress::set(None, std::ptr::null_mut());
            }
            let _ = sender.send(Event::Finish(ok));
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        self.close(Some(false));
    }
}

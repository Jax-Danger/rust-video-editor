//! Background proxy transcode. The UI thread only starts the job and attaches paths.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use editor_media::{generate_proxy, ProxyRequest};

#[derive(Clone, Debug)]
pub struct ProxyItem {
    pub media_id: u64,
    pub name: String,
    pub source: PathBuf,
    pub output: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ProxySnapshot {
    pub finished: bool,
    pub cancelled: bool,
    pub done: usize,
    pub total: usize,
    pub message: String,
    pub attached: Vec<(u64, String)>,
    pub failures: Vec<String>,
}

pub struct ProxyJob {
    shared: Arc<Mutex<ProxySnapshot>>,
    cancel: Arc<AtomicBool>,
}

impl ProxyJob {
    pub fn snapshot(&self) -> ProxySnapshot {
        self.shared
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn spawn_proxies(items: Vec<ProxyItem>) -> ProxyJob {
    let total = items.len();
    let shared = Arc::new(Mutex::new(ProxySnapshot {
        finished: total == 0,
        cancelled: false,
        done: 0,
        total,
        message: if total == 0 {
            "No video media to proxy.".into()
        } else {
            format!("Proxy 0/{total}")
        },
        attached: Vec::new(),
        failures: Vec::new(),
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    let job = ProxyJob {
        shared: shared.clone(),
        cancel: cancel.clone(),
    };
    std::thread::spawn(move || run(items, shared, cancel));
    job
}

fn run(items: Vec<ProxyItem>, shared: Arc<Mutex<ProxySnapshot>>, cancel: Arc<AtomicBool>) {
    let total = items.len();
    for (index, item) in items.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            finish(&shared, true);
            return;
        }
        {
            let mut slot = shared.lock().unwrap_or_else(|poison| poison.into_inner());
            slot.message = format!("Proxy {}/{} — {}", index + 1, total, item.name);
        }
        let request = ProxyRequest::standard(&item.source, &item.output);
        match generate_proxy(&request) {
            Ok(()) => {
                let mut slot = shared.lock().unwrap_or_else(|poison| poison.into_inner());
                slot.done += 1;
                slot.attached
                    .push((item.media_id, item.output.to_string_lossy().into_owned()));
            }
            Err(err) => {
                let mut slot = shared.lock().unwrap_or_else(|poison| poison.into_inner());
                slot.done += 1;
                slot.failures.push(format!("{}: {err}", item.name));
            }
        }
    }
    finish(&shared, cancel.load(Ordering::Relaxed));
}

fn finish(shared: &Mutex<ProxySnapshot>, cancelled: bool) {
    let mut slot = shared.lock().unwrap_or_else(|poison| poison.into_inner());
    slot.finished = true;
    slot.cancelled = cancelled;
    let made = slot.attached.len();
    let failed = slot.failures.len();
    let total = slot.total;
    slot.message = if cancelled {
        format!("Proxy cancelled after {made} of {total}.")
    } else if failed == 0 {
        format!("Proxies ready ({made}). Preview is using them.")
    } else if made == 0 {
        slot.failures
            .first()
            .cloned()
            .unwrap_or_else(|| "Proxy generation failed.".into())
    } else {
        format!("Proxies ready ({made}), {failed} failed.")
    };
}

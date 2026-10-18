//! The background watcher: finds live sessions for every enabled harness and streams their
//! events to the UI. Plain polling keeps it identical on Linux, macOS and Windows (network
//! drives and WSL paths included); a stat per file every few hundred ms costs next to nothing.

use crate::config::Config;
use crate::harness::{self, Adapter, SourceReader};
use crate::model::Event;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

pub enum Update {
    Events(Vec<Event>),
    /// Which harnesses have data on this machine: (id, name, sessions found).
    Harnesses(Vec<(String, String, usize)>),
}

/// How long a quiet source keeps its read position; the UI forgets sessions idle this long too.
pub const IDLE_CLOSE: Duration = Duration::from_secs(6 * 3600);

struct Open {
    reader: Box<dyn SourceReader>,
    seen: Instant,
}

/// Runs until the receiver is dropped.
pub fn spawn(config: Arc<RwLock<Config>>) -> Receiver<Update> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("shipoffice-watch".into())
        .spawn(move || run(config, tx))
        .expect("watcher thread");
    rx
}

fn run(config: Arc<RwLock<Config>>, tx: Sender<Update>) {
    let adapters = harness::all();
    let mut open: HashMap<PathBuf, Open> = HashMap::new();
    let mut last_scan: Option<Instant> = None;
    loop {
        if last_scan.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            last_scan = Some(Instant::now());
            let cfg = config.read().map(|c| c.clone()).unwrap_or_default();
            let since = SystemTime::now() - Duration::from_secs(cfg.active_minutes.max(1) * 60);
            let mut found = Vec::new();
            for a in &adapters {
                let hc = cfg.harness(a.id());
                if !hc.enabled {
                    continue;
                }
                let specs = a.discover(&roots(a.as_ref(), &hc), since);
                let sessions: std::collections::HashSet<_> = specs.iter().map(|s| s.key.id.clone()).collect();
                found.push((a.id().to_owned(), a.name().to_owned(), sessions.len()));
                for s in specs {
                    let entry = open.entry(s.path.clone()).or_insert_with(|| Open { reader: a.open(&s), seen: Instant::now() });
                    entry.seen = Instant::now();
                }
            }
            // files that fell out of the active window stop being polled but keep their offset, so a
            // resumed session continues instead of replaying its transcript; long-idle ones are closed
            open.retain(|_, o| o.seen.elapsed() < IDLE_CLOSE);
            if tx.send(Update::Harnesses(found)).is_err() {
                return;
            }
        }
        let mut batch = Vec::new();
        for o in open.values_mut().filter(|o| o.seen.elapsed() < Duration::from_secs(30)) {
            batch.extend(o.reader.poll());
        }
        if !batch.is_empty() {
            batch.sort_by_key(|e| e.at);
            if tx.send(Update::Events(batch)).is_err() {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(350));
    }
}

fn roots(a: &dyn Adapter, hc: &crate::config::HarnessConfig) -> Vec<PathBuf> {
    let mut r = if hc.replace_defaults { Vec::new() } else { a.default_roots() };
    r.extend(hc.extra_paths.iter().cloned());
    r
}

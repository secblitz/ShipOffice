// release builds on Windows open no console window
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::time::{Duration, Instant};

fn main() -> iced::Result {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("shipoffice {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if std::env::args().any(|a| a == "--list") {
        list();
        return Ok(());
    }
    shipoffice::app::run()
}

/// `shipoffice --list`: what the watcher finds, without opening a window.
fn list() {
    let cfg = std::sync::Arc::new(std::sync::RwLock::new(shipoffice::config::Config::load()));
    let rx = shipoffice::sources::spawn(cfg);
    let mut sessions = std::collections::BTreeMap::new();
    let end = Instant::now() + Duration::from_secs(8);
    while Instant::now() < end {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(shipoffice::sources::Update::Events(evs)) => {
                for e in evs {
                    sessions.entry(e.session.clone()).or_insert_with(|| shipoffice::session::Session::new(e.session.clone(), e.session.harness, e.at)).apply(&e);
                }
            }
            Ok(shipoffice::sources::Update::Harnesses(h)) => {
                for (id, _, n) in h {
                    println!("{id:<14} {n} live");
                }
            }
            Err(_) => {}
        }
    }
    for s in sessions.values() {
        println!("{:<12} {:<24} {:?} agents={} tokens={} ${:.2}", s.key.harness, s.project(), s.status, s.live_agents().count(), s.usage.total_in() + s.usage.output, s.cost_estimate());
    }
}

fn main() {
    let cfg = std::sync::Arc::new(std::sync::RwLock::new(shipoffice::config::Config::load()));
    let rx = shipoffice::sources::spawn(cfg);
    let mut sessions = std::collections::BTreeMap::new();
    let end = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < end {
        match rx.recv_timeout(std::time::Duration::from_millis(500)) {
            Ok(shipoffice::sources::Update::Events(evs)) => {
                for e in evs {
                    sessions
                        .entry(e.session.clone())
                        .or_insert_with(|| shipoffice::session::Session::new(e.session.clone(), e.session.harness, e.at))
                        .apply(&e);
                }
            }
            Ok(shipoffice::sources::Update::Harnesses(h)) => println!("{h:?}"),
            Err(_) => {}
        }
    }
    for s in sessions.values() {
        println!("{} {} {:?} agents={} tokens={} ${:.2}", s.key.harness, s.project(), s.status, s.live_agents().count(), s.usage.total_in() + s.usage.output, s.cost_estimate());
    }
}

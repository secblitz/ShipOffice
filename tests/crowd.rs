//! A big crew in one office: every helper gets a desk, and a frame stays cheap.
//! `cargo test --release --test crowd -- --ignored --nocapture` prints the timings.

use shipoffice::model::{AgentRef, Event, EventKind, SessionKey, Usage};
use shipoffice::office::world::World;
use shipoffice::render::{scene, view::View};
use shipoffice::session::Session;
use std::time::{Instant, SystemTime};

fn session(n: usize) -> Session {
    let key = SessionKey { harness: "test", id: "crowd".into() };
    let mut s = Session::new(key.clone(), "Test", SystemTime::now());
    let ev = |agent: AgentRef, kind: EventKind| Event { session: key.clone(), agent, at: SystemTime::now(), kind };
    s.apply(&ev(AgentRef::Lead, EventKind::UserPrompt { text: "go".into() }));
    for i in 0..n {
        let id = format!("h{i}");
        s.apply(&ev(AgentRef::Lead, EventKind::SubagentStart { agent_id: id.clone(), description: format!("Task {i}"), kind: None }));
        let name = ["Read", "Edit", "Bash"][i % 3];
        s.apply(&ev(AgentRef::Sub(id.clone()), EventKind::ToolCall { call_id: format!("c{i}"), name: name.into(), summary: "x".into() }));
        s.apply(&ev(AgentRef::Sub(id), EventKind::Usage(Usage { input: 1000, output: 10, cache_read: 0, cache_write: 0 })));
    }
    s
}

fn run(n: usize, frames: usize) -> (World, f64, f64) {
    let s = session(n);
    let mut w = World::for_crew("crowd", 7, s.live_agents().count());
    let dt = 1.0 / 60.0;
    // everyone walks in first; then the frames are timed
    for _ in 0..60 * 120 {
        w.sync(&s);
        w.step(dt);
    }
    let (mut sim, mut draw) = (0.0, 0.0);
    for _ in 0..frames {
        let t = Instant::now();
        w.sync(&s);
        w.step(dt);
        sim += t.elapsed().as_secs_f64();
        let t = Instant::now();
        let v = View::new(&w.s.layout, shipoffice::render::view::DEFAULT_YAW, shipoffice::render::view::DEFAULT_PITCH);
        let sc = scene::build(&scene::Inputs { world: &w, view: &v, statics: None, board: None, hover: None, selected: None, anim_fps: 16.0, now: 1.0, city: None });
        std::hint::black_box(sc);
        draw += t.elapsed().as_secs_f64();
    }
    (w, sim * 1e3 / frames as f64, draw * 1e3 / frames as f64)
}

#[test]
fn a_big_crew_all_get_desks() {
    let (w, _, _) = run(300, 60);
    let seated = w.avatars.iter().filter(|a| a.is_seated_at_desk()).count();
    assert_eq!(w.avatars.len(), 301, "everyone is hired");
    assert!(w.s.layout.desks.len() > 300, "{} desks for 301 people", w.s.layout.desks.len());
    let mut doing: std::collections::BTreeMap<&str, usize> = Default::default();
    for a in w.avatars.iter().filter(|a| !a.is_seated_at_desk()) {
        *doing.entry(a.label.as_str()).or_default() += 1;
    }
    assert!(seated > 290, "only {seated} of 301 reached a desk; the rest: {doing:?}");
}

#[test]
#[ignore]
fn timings() {
    for n in [10, 60, 300] {
        let (w, sim, draw) = run(n, 600);
        println!("{n:>4} helpers, {}x{} room, {} desks: sim {sim:.3} ms, scene {draw:.3} ms per frame", w.s.layout.w, w.s.layout.h, w.s.layout.desks.len());
    }
}

//! Replay: the last hour of a session played back in its own office, at up to 64 times the
//! speed, with a scrubber. It folds the session's own events into a fresh session and office on
//! a clock of its own, so the live office carries on untouched beside it.

use crate::model::{Event, SessionKey};
use crate::office::world::World;
use crate::session::Session;
use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

/// How far back a replay reaches, and what each session keeps for it.
pub const SPAN: Duration = Duration::from_secs(60 * 60);
const KEEP: usize = 150_000;
/// Speeds a replay steps through.
pub const SPEEDS: [f32; 6] = [1.0, 2.0, 4.0, 8.0, 16.0, 64.0];
/// Office seconds simulated per step: the walkers stay smooth at any speed.
const STEP: f32 = 0.05;
/// What a seek simulates so the people are where the moment had them.
const SETTLE: f32 = 12.0;

/// One session's recent events, oldest first.
#[derive(Default)]
pub struct History(VecDeque<Event>);

impl History {
    pub fn push(&mut self, e: Event) {
        self.0.push_back(e);
        let newest = self.0.back().map(|e| e.at).unwrap_or(SystemTime::UNIX_EPOCH);
        while self.0.len() > KEEP || self.0.front().is_some_and(|f| newest.duration_since(f.at).unwrap_or_default() > SPAN + Duration::from_secs(300)) {
            self.0.pop_front();
        }
    }
}

pub struct Replay {
    pub key: SessionKey,
    pub from: SystemTime,
    pub to: SystemTime,
    pub at: SystemTime,
    pub speed: f32,
    pub playing: bool,
    pub session: Session,
    pub world: World,
    events: Vec<Event>,
    next: usize,
    /// what a fresh office and session are made from
    room: World,
    name: String,
    /// the session's clock last ticked here
    ticked: SystemTime,
}

impl Replay {
    /// A replay of `h` in `live`'s room, from an hour (or the start of what was seen) before its
    /// last event.
    pub fn new(key: SessionKey, h: &History, name: &str, live: &World) -> Option<Self> {
        let mut events: Vec<Event> = h.0.iter().cloned().collect();
        // a log read in bursts can be a little out of order
        events.sort_by_key(|e| e.at);
        let to = events.last()?.at;
        let from = events.iter().map(|e| e.at).find(|&t| to.duration_since(t).unwrap_or_default() <= SPAN)?;
        let mut r = Self {
            session: Session::new(key.clone(), name, from),
            world: live.same_room(),
            key,
            from,
            to,
            at: from,
            speed: 8.0,
            playing: true,
            events,
            next: 0,
            room: live.same_room(),
            name: name.to_owned(),
            ticked: from,
        };
        r.seek(from);
        Some(r)
    }

    /// How long the replay runs, and how far into it the cursor is, in seconds.
    pub fn length(&self) -> f32 {
        self.to.duration_since(self.from).unwrap_or_default().as_secs_f32().max(1.0)
    }

    pub fn offset(&self) -> f32 {
        self.at.duration_since(self.from).unwrap_or_default().as_secs_f32()
    }

    /// Jumps to `t`: everything before it folded in afresh, then a few seconds lived so the
    /// office looks like that moment rather than like everyone just walked in.
    pub fn seek(&mut self, t: SystemTime) {
        let t = t.clamp(self.from, self.to);
        let settle = Duration::from_secs_f32(SETTLE);
        let before = t.checked_sub(settle).unwrap_or(t).max(self.from);
        self.session = Session::new(self.key.clone(), &self.name, self.from);
        self.world = self.room.same_room();
        self.next = 0;
        while self.next < self.events.len() && self.events[self.next].at < before {
            self.session.apply(&self.events[self.next]);
            self.next += 1;
        }
        self.session.tick(before);
        self.ticked = before;
        self.at = before;
        let live = std::mem::replace(&mut self.playing, true);
        let speed = std::mem::replace(&mut self.speed, 1.0);
        while self.at < t {
            self.advance(STEP);
        }
        (self.playing, self.speed) = (live, speed);
    }

    /// Moves the replay on by `dt` seconds of wall time.
    pub fn tick(&mut self, dt: f32) {
        if !self.playing {
            // still: the office keeps breathing, the clock does not move
            self.world.step(dt);
            return;
        }
        let mut left = (dt * self.speed).min(STEP * 64.0);
        while left > 0.0 && self.playing {
            let step = left.min(STEP);
            self.advance(step);
            left -= step;
        }
    }

    /// `secs` of replay time: the events in it, then the office lives through it.
    fn advance(&mut self, secs: f32) {
        self.at = (self.at + Duration::from_secs_f32(secs)).min(self.to);
        while self.next < self.events.len() && self.events[self.next].at <= self.at {
            let e = &self.events[self.next];
            self.session.apply(e);
            self.world.cue(e, self.at);
            self.next += 1;
        }
        if self.at.duration_since(self.ticked).unwrap_or_default() >= Duration::from_secs(1) {
            self.session.tick(self.at);
            self.ticked = self.at;
        }
        self.world.sync(&self.session, self.at);
        self.world.step(secs);
        if self.at >= self.to {
            self.playing = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentRef, EventKind};

    #[test]
    fn a_replay_rebuilds_the_office_at_any_moment() {
        let key = SessionKey { harness: "test", id: "r".into() };
        let t0 = SystemTime::now() - Duration::from_secs(600);
        let ev = |s: u64, agent: AgentRef, kind: EventKind| Event { session: key.clone(), agent, at: t0 + Duration::from_secs(s), kind };
        let mut h = History::default();
        h.push(ev(0, AgentRef::Lead, EventKind::UserPrompt { text: "go".into() }));
        for i in 0..6 {
            h.push(ev(10 + i, AgentRef::Lead, EventKind::SubagentStart { agent_id: format!("h{i}"), description: format!("Task {i}"), kind: None }));
            h.push(ev(300 + i, AgentRef::Lead, EventKind::SubagentEnd { agent_id: format!("h{i}") }));
        }
        h.push(ev(500, AgentRef::Lead, EventKind::TurnEnd));
        let mut r = Replay::new(key, &h, "Test", &World::new("replay", 7)).expect("a replay");
        assert!((r.length() - 500.0).abs() < 1.0);
        // mid-run: the lead and the six helpers are in
        r.seek(t0 + Duration::from_secs(200));
        assert_eq!(r.session.live_agents().count(), 7);
        assert_eq!(r.world.avatars.len(), 7);
        // played on past the end of their work, the helpers are done
        r.speed = 64.0;
        for _ in 0..60 * 10 {
            r.tick(1.0 / 60.0);
        }
        assert!(!r.playing && r.at == r.to);
        assert_eq!(r.session.live_agents().count(), 1);
        // and back again
        r.seek(t0 + Duration::from_secs(100));
        assert_eq!(r.session.live_agents().count(), 7);
    }
}

//! The people in one office and what they are doing this frame.
//!
//! Each avatar runs a small queue of steps (walk somewhere, act for a while, wait until
//! something happens). The director in [`World::sync`] compares what the session says each
//! agent is doing with what its avatar is doing and replans when they differ, so the office
//! always follows the real work while the in-between moments (coffee, ping pong, the couch)
//! stay lively on their own.

use super::gen::{generate, Layout, SeatKind};
use super::{hash, tile, Face, Rng, Tile};
use crate::model::{Activity, AgentRef, Event, EventKind};
use crate::session::{Session, Status};
use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anim {
    Walk,
    Stand,
    Type,
    Read,
    Sit,
    PingPongFront,
    PingPongBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BubbleKind {
    Say,
    Think,
    Tool,
}

#[derive(Debug, Clone)]
pub struct Bubble {
    pub kind: BubbleKind,
    pub text: String,
    pub left: f32,
    pub dur: f32,
}

/// Outfit colours. Index 1 top, 2 hair, 3 pants, 4 skin (never recoloured), 5 accent.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub id: u32,
    pub cols: [Option<[u8; 3]>; 6],
}

const TOPS: [[u8; 3]; 12] = [[214, 104, 56], [58, 148, 138], [214, 164, 52], [72, 128, 82], [132, 82, 142], [62, 84, 146], [188, 62, 64], [112, 122, 138], [222, 132, 164], [226, 210, 178], [52, 54, 64], [96, 170, 206]];
const HAIR: [[u8; 3]; 8] = [[42, 40, 48], [96, 62, 40], [152, 72, 42], [218, 182, 108], [172, 172, 178], [72, 112, 192], [212, 112, 162], [60, 42, 36]];
const PANTS: [[u8; 3]; 6] = [[60, 62, 72], [62, 86, 132], [172, 150, 112], [92, 102, 72], [36, 36, 42], [120, 72, 60]];
const ACCENTS: [[u8; 3]; 8] = [[236, 232, 222], [200, 214, 232], [226, 196, 92], [184, 72, 68], [64, 96, 150], [88, 140, 104], [232, 170, 150], [66, 64, 74]];

/// A palette from a seed. Within one office nobody shares a shirt colour, and hair repeats only when it has to.
pub fn palette_for(seed: u32, avoid: &[&Palette]) -> Palette {
    let mut r = Rng::new(seed);
    for _ in 0..4 {
        r.next(); // similar seeds start out correlated
    }
    let tops: Vec<_> = TOPS.iter().filter(|t| !avoid.iter().any(|p| p.cols[1] == Some(**t))).copied().collect();
    let hair: Vec<_> = HAIR.iter().filter(|h| !avoid.iter().any(|p| p.cols[2] == Some(**h))).copied().collect();
    let top = *r.pick(if tops.is_empty() { &TOPS[..] } else { &tops });
    let hr = *r.pick(if hair.is_empty() { &HAIR[..] } else { &hair });
    Palette { id: seed, cols: [None, Some(top), Some(hr), Some(*r.pick(&PANTS)), None, Some(*r.pick(&ACCENTS))] }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeatRef {
    Desk(usize),
    Lounge(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    None,
    Bar,
    BarQueue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Until {
    /// until the director replans
    Replanned,
    BarFree,
    PartnerOrTimeout,
    GameOver,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Done {
    None,
    Cup,
    StandUp,
    CancelGame,
    EndGame,
    Reported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Brew,
    PlayFront,
    StartGame,
    LeadBreak,
}

#[derive(Debug, Clone)]
enum Step {
    Walk { to: Tile, label: String, path: Option<Vec<Tile>>, tag: Tag },
    Act { anim: Anim, sit: Option<SeatRef>, face: Option<Face>, left: f32, started: bool, bubble: Option<(BubbleKind, String)>, label: String, tag: Tag, done: Done },
    Wait { anim: Anim, sit: Option<SeatRef>, face: Option<Face>, label: String, until: Until, started: bool, bubble: Option<(BubbleKind, String)>, done: Done },
    Call(Call),
    Leave,
}

impl Step {
    fn tag(&self) -> Tag {
        match self {
            Step::Walk { tag, .. } | Step::Act { tag, .. } => *tag,
            Step::Wait { until: Until::BarFree, .. } | Step::Call(Call::Brew) => Tag::BarQueue,
            _ => Tag::None,
        }
    }
    fn target(&self) -> Option<Tile> {
        match self {
            Step::Walk { to, .. } => Some(*to),
            _ => None,
        }
    }
}

/// What the director has asked an avatar to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Work(Activity),
    WaitForUser,
    Report,
    Lounge,
}

#[derive(Debug, Clone)]
pub struct Avatar {
    pub id: u32,
    pub agent: AgentRef,
    pub name: String,
    pub role: Option<String>,
    pub lead: bool,
    pub char_id: u8,
    pub palette: Palette,
    pub x: f32,
    pub y: f32,
    pub face: Face,
    pub anim: Anim,
    pub t: f32,
    pub phase: f32,
    pub alpha: f32,
    pub bubble: Option<Bubble>,
    /// what the HUD shows: "Typing in src/main.rs", "Making a coffee"...
    pub label: String,
    pub last_said: Option<(BubbleKind, String)>,
    pub seated: Option<SeatRef>,
    pub desk: Option<usize>,
    q: VecDeque<Step>,
    mode: Mode,
    gone: bool,
    leaving: bool,
    idle_since: Option<f32>,
    reported: bool,
}

impl Avatar {
    pub fn say(&mut self, kind: BubbleKind, text: impl Into<String>, dur: f32) {
        let text = text.into();
        self.last_said = Some((kind, text.clone()));
        self.bubble = Some(Bubble { kind, text, left: dur, dur });
    }
    pub fn is_seated_at_desk(&self) -> bool {
        matches!(self.seated, Some(SeatRef::Desk(_)))
    }
    pub fn busy(&self) -> bool {
        matches!(self.mode, Mode::Work(_) | Mode::Report)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameState {
    Waiting,
    Joining,
    Playing,
}

#[derive(Debug, Clone)]
pub struct Game {
    pub state: GameState,
    pub a: u32,
    pub b: Option<u32>,
    pub t0: f32,
    pub end: f32,
    pub score: [u32; 2],
    next_point: f32,
    since: f32,
}

/// Everything except the avatars, so an avatar can update while reading shared state.
pub struct Shared {
    pub layout: Layout,
    pub elapsed: f32,
    pub cups: u32,
    pub game: Option<Game>,
    pub desk_owner: Vec<Option<u32>>,
    pub lounge_owner: Vec<Option<u32>>,
    rng: Rng,
    reporting: Vec<Tile>,
    /// tiles someone stands on or is walking to, refreshed every frame
    busy_tiles: Vec<(u32, Tile)>,
    bar_busy: Vec<(u32, Tag)>,
    chat_t: f32,
}

impl Shared {
    pub fn seat(&self, r: SeatRef) -> &super::gen::Seat {
        match r {
            SeatRef::Desk(i) => &self.layout.desks[i],
            SeatRef::Lounge(i) => &self.layout.lounge.seats[i],
        }
    }
}

impl Avatar {
    /// The colour that marks this agent in the HUD: their shirt.
    pub fn color(&self) -> [u8; 3] {
        self.palette.cols[1].unwrap_or([200, 200, 200])
    }
}

pub struct World {
    pub avatars: Vec<Avatar>,
    pub s: Shared,
    pub project: String,
    next_id: u32,
}

const SMALL_TALK: [&str; 10] = [
    "That rebase was something",
    "Cache hits are great today",
    "Who keeps breaking the linter?",
    "The new espresso is good",
    "Lead is in a planning mood",
    "Ship it on Friday?",
    "I read the whole router twice",
    "Did the tests ever go green?",
    "Context is getting full",
    "Ha, fair point",
];

impl World {
    /// The office for a project. `seed` comes from the project path, or from the config when pinned.
    pub fn new(project: &str, seed: u32) -> Self {
        let layout = generate(seed);
        let s = Shared {
            desk_owner: vec![None; layout.desks.len()],
            lounge_owner: vec![None; layout.lounge.seats.len()],
            rng: Rng::new(seed ^ 0x9e37_79b9),
            layout,
            elapsed: 0.0,
            cups: 0,
            game: None,
            reporting: Vec::new(),
            busy_tiles: Vec::new(),
            bar_busy: Vec::new(),
            chat_t: 2.0,
        };
        Self { avatars: Vec::new(), s, project: project.to_owned(), next_id: 1 }
    }

    /// Someone is at the coffee machine right now (the machine steams).
    pub fn brewing(&self) -> bool {
        self.avatars.iter().any(|a| a.q.front().is_some_and(|s| matches!(s, Step::Act { tag: Tag::Bar, started: true, .. })))
    }

    pub fn avatar(&self, id: u32) -> Option<&Avatar> {
        self.avatars.iter().find(|a| a.id == id)
    }

    fn hire(&mut self, agent: &AgentRef, name: &str, role: Option<String>) -> usize {
        let lead = *agent == AgentRef::Lead;
        let key = match agent {
            AgentRef::Lead => "lead".to_owned(),
            AgentRef::Sub(id) => id.clone(),
        };
        let h = hash(&format!("{}:{}", self.project, key));
        // a character nobody else here has yet; characters repeat (in new outfits) once all are taken
        let used: Vec<u8> = self.avatars.iter().filter(|a| !a.gone).map(|a| a.char_id).collect();
        let free: Vec<u8> = (0..7u8).filter(|c| !used.contains(c)).collect();
        let char_id = if free.is_empty() { (h % 7) as u8 } else { free[(h as usize) % free.len()] };
        let avoid: Vec<&Palette> = self.avatars.iter().map(|a| &a.palette).collect();
        let palette = palette_for(h, &avoid);
        let door = self.s.layout.door;
        let mut av = Avatar {
            id: self.next_id,
            agent: agent.clone(),
            name: name.to_owned(),
            role,
            lead,
            char_id,
            palette,
            x: door.x as f32,
            y: door.y as f32,
            face: Face::DR,
            anim: Anim::Stand,
            t: 0.0,
            phase: (h % 1000) as f32 / 333.0,
            alpha: 1.0,
            bubble: None,
            label: "Arriving".into(),
            last_said: None,
            seated: None,
            desk: None,
            q: VecDeque::new(),
            mode: Mode::Lounge,
            gone: false,
            leaving: false,
            idle_since: Some(0.0),
            reported: false,
        };
        self.next_id += 1;
        if lead && !self.s.layout.desks.is_empty() {
            // the lead owns the first desk and starts the day sitting at it
            self.s.desk_owner[0] = Some(av.id);
            av.desk = Some(0);
            let d = &self.s.layout.desks[0];
            av.x = d.ax as f32;
            av.y = d.ay as f32;
        }
        self.avatars.push(av);
        self.avatars.len() - 1
    }

    /// Reacts to one live event with a bubble over the right person.
    pub fn cue(&mut self, e: &Event) {
        if SystemTime::now().duration_since(e.at).unwrap_or_default() > Duration::from_secs(20) {
            return; // backlog from before the app started: no bubbles for old news
        }
        let Some(a) = self.avatars.iter_mut().find(|a| a.agent == e.agent && !a.gone) else { return };
        match &e.kind {
            EventKind::ToolCall { name, summary, .. } => {
                let text = if summary.is_empty() { name.clone() } else { format!("{name} · {summary}") };
                a.say(BubbleKind::Tool, text, 3.0);
            }
            EventKind::AssistantText { text } => a.say(BubbleKind::Say, text.clone(), 5.0),
            EventKind::Thinking { text } if !text.is_empty() => a.say(BubbleKind::Think, text.clone(), 3.5),
            EventKind::NeedsInput { text } => a.say(BubbleKind::Think, text.clone(), 6.0),
            _ => {}
        }
    }

    /// Brings the avatars in line with the session: hires new agents, sends finished helpers
    /// to report, and replans anyone whose job changed.
    pub fn sync(&mut self, session: &Session) {
        for st in session.agents.values() {
            if !self.avatars.iter().any(|a| a.agent == st.id && !a.gone) {
                let room = self.s.layout.desks.len() + 2;
                let live = self.avatars.iter().filter(|a| !a.gone && !a.leaving).count();
                if st.done || (st.id != AgentRef::Lead && live >= room) {
                    continue;
                }
                let i = self.hire(&st.id, &st.label, st.kind.clone());
                if st.id != AgentRef::Lead {
                    self.avatars[i].say(BubbleKind::Say, format!("On it: {}", st.label), 3.0);
                }
            }
        }
        for i in 0..self.avatars.len() {
            let Some(st) = session.agents.get(&self.avatars[i].agent) else { continue };
            let a = &mut self.avatars[i];
            if a.gone || a.leaving {
                continue;
            }
            a.name.clone_from(&st.label);
            let want = if a.lead {
                match session.status {
                    Status::Working => Mode::Work(st.activity),
                    Status::NeedsYou => Mode::WaitForUser,
                    Status::Idle => Mode::Lounge,
                }
            } else if !st.done {
                Mode::Work(st.activity)
            } else if !a.reported && a.mode != Mode::Lounge {
                Mode::Report
            } else {
                Mode::Lounge
            };
            if want == a.mode {
                continue;
            }
            // a new activity at the same desk only swaps the animation, no getting up
            if let (Mode::Work(_), Mode::Work(act)) = (a.mode, want) {
                a.mode = want;
                if let Some(Step::Wait { anim, label, until: Until::Replanned, .. }) = a.q.back_mut() {
                    *anim = anim_for(act);
                    *label = label_for(act, st.tool.as_ref()).into();
                    a.label.clone_from(label);
                    continue;
                }
            }
            let prev = a.mode;
            a.mode = want;
            interrupt(a, &mut self.s);
            match want {
                Mode::Work(act) => plan_work(a, &mut self.s, act, st.tool.as_ref()),
                Mode::WaitForUser => {
                    let desk = a.desk.map(SeatRef::Desk);
                    if prev == Mode::Lounge || desk.is_none() {
                        lead_break(a, &mut self.s);
                    } else {
                        // first a while at the desk, then a coffee on the couch
                        a.q.push_back(Step::Act { anim: Anim::Type, sit: desk, face: None, left: 25.0, started: false, bubble: Some((BubbleKind::Think, "Waiting for your input".into())), label: "Waiting for you".into(), tag: Tag::None, done: Done::None });
                        a.q.push_back(Step::Call(Call::LeadBreak));
                    }
                }
                Mode::Report => plan_report(a, &mut self.s, st.said.clone().unwrap_or_else(|| format!("Done: {}", st.label))),
                Mode::Lounge => {
                    a.idle_since = Some(self.s.elapsed);
                    release_desk(a, &mut self.s);
                }
            }
        }
        // the lead's desk shows their current tool even while the queue is busy
        if let Some(lead) = session.agents.get(&AgentRef::Lead) {
            if let Some(a) = self.avatars.iter_mut().find(|a| a.lead) {
                if let Mode::Work(act) = a.mode {
                    if a.is_seated_at_desk() {
                        a.label = label_for(act, lead.tool.as_ref()).into();
                    }
                }
            }
        }
    }

    pub fn step(&mut self, dt: f32) {
        let s = &mut self.s;
        s.elapsed += dt;
        // who stands where, and who is at the coffee machine, as of this frame
        s.busy_tiles = self
            .avatars
            .iter()
            .filter(|a| !a.leaving)
            .flat_map(|a| {
                let here = (a.id, tile(a.x.round() as i32, a.y.round() as i32));
                std::iter::once(here).chain(a.q.iter().filter_map(|st| st.target()).map(move |t| (a.id, t)))
            })
            .collect();
        s.bar_busy = self.avatars.iter().flat_map(|a| a.q.iter().map(move |st| (a.id, st.tag()))).filter(|(_, t)| *t != Tag::None).collect();

        // two people at the same table chat now and then
        s.chat_t -= dt;
        if s.chat_t <= 0.0 {
            s.chat_t = 3.5 + s.rng.next() * 2.0;
            for kind in ["cafe", "couch"] {
                let at: Vec<usize> = (0..s.lounge_owner.len())
                    .filter(|&i| s.layout.lounge.seats[i].table == Some(kind))
                    .filter_map(|i| s.lounge_owner[i].map(|id| (i, id)))
                    .filter_map(|(i, id)| self.avatars.iter().position(|a| a.id == id && a.seated == Some(SeatRef::Lounge(i)) && a.bubble.is_none() && !a.lead))
                    .collect();
                if at.len() == 2 {
                    let who = at[s.rng.below(2)];
                    let line = *s.rng.pick(&SMALL_TALK);
                    self.avatars[who].say(BubbleKind::Say, line, 3.0);
                    break;
                }
            }
        }
        // rallies end now and then; first to 11 wins
        if let Some(g) = s.game.as_mut() {
            if g.state == GameState::Playing {
                g.next_point -= dt;
                if g.next_point <= 0.0 {
                    let side = usize::from(s.rng.next() >= 0.5);
                    g.score[side] += 1;
                    g.next_point = 2.2 + s.rng.next() * 3.5;
                    if g.score[side] >= 11 {
                        g.end = s.elapsed;
                        let winner = if side == 0 { Some(g.a) } else { g.b };
                        let line = *s.rng.pick(&["Good game!", "Rematch later?", "That was close"]);
                        if let Some(w) = self.avatars.iter_mut().find(|a| Some(a.id) == winner) {
                            w.say(BubbleKind::Say, line, 2.6);
                        }
                    }
                }
            }
        }
        // the lead keeps a visible thought while it waits for you
        let think = s.rng.next() < dt * 0.25;
        for a in &mut self.avatars {
            if think && a.lead && a.mode == Mode::WaitForUser && a.seated.is_some() && a.bubble.is_none() {
                a.say(BubbleKind::Think, "Waiting for your input", 4.0);
            }
        }
        let idle_count = self.avatars.iter().filter(|a| !a.lead && a.mode == Mode::Lounge && !a.leaving).count();
        let first_idle = self.avatars.iter().find(|a| !a.lead && a.mode == Mode::Lounge && !a.leaving).map(|a| a.id);
        let game_waiting = self.s.game.as_ref().filter(|g| g.state == GameState::Waiting).map(|g| g.a);
        for i in 0..self.avatars.len() {
            let a = &mut self.avatars[i];
            update(a, &mut self.s, dt, idle_count, first_idle, game_waiting);
        }
        self.avatars.retain(|a| !a.gone);
    }
}

fn anim_for(act: Activity) -> Anim {
    match act {
        Activity::Reading | Activity::Waiting => Anim::Read,
        _ => Anim::Type,
    }
}

fn label_for(act: Activity, tool: Option<&(String, String)>) -> String {
    let base = match act {
        Activity::Reading => "Reading",
        Activity::Running => "Running",
        Activity::Thinking => "Thinking",
        Activity::Talking => "Writing a reply",
        Activity::Waiting => "Waiting on helpers",
        Activity::Typing => "Editing",
    };
    match tool {
        Some((name, summary)) if matches!(act, Activity::Reading | Activity::Running | Activity::Typing) => {
            if summary.is_empty() {
                format!("{base} · {name}")
            } else {
                format!("{base} · {summary}")
            }
        }
        _ => base.to_owned(),
    }
}

fn stand_up(a: &mut Avatar, s: &mut Shared) {
    if let Some(seat) = a.seated.take() {
        let st = match seat {
            SeatRef::Desk(i) => &s.layout.desks[i],
            SeatRef::Lounge(i) => {
                s.lounge_owner[i] = None;
                &s.layout.lounge.seats[i]
            }
        };
        a.x = st.ax as f32;
        a.y = st.ay as f32;
    }
}

fn sit_on(a: &mut Avatar, s: &Shared, seat: SeatRef) {
    let st = match seat {
        SeatRef::Desk(i) => &s.layout.desks[i],
        SeatRef::Lounge(i) => &s.layout.lounge.seats[i],
    };
    a.x = st.x as f32;
    a.y = st.y as f32;
    a.face = st.face;
    a.seated = Some(seat);
}

fn release_desk(a: &mut Avatar, s: &mut Shared) {
    if a.lead {
        return; // the lead's desk stays theirs
    }
    if let Some(d) = a.desk.take() {
        if a.seated == Some(SeatRef::Desk(d)) {
            stand_up(a, s);
        }
        s.desk_owner[d] = None;
    }
}

/// Stops whatever the avatar does (a break, a game, a seat) before giving it new work.
fn interrupt(a: &mut Avatar, s: &mut Shared) {
    if let Some(g) = &s.game {
        if g.a == a.id || g.b == Some(a.id) {
            s.game = None;
        }
    }
    for st in &a.q {
        if let Step::Act { done: Done::Reported, .. } = st {
            s.reporting.clear();
        }
    }
    a.q.clear();
    if !matches!(a.seated, Some(SeatRef::Desk(_))) {
        stand_up(a, s);
    }
}

fn free_desk(a: &mut Avatar, s: &mut Shared) -> Option<usize> {
    if a.desk.is_some() {
        return a.desk;
    }
    let i = s.desk_owner.iter().position(Option::is_none)?;
    s.desk_owner[i] = Some(a.id);
    a.desk = Some(i);
    a.desk
}

fn plan_work(a: &mut Avatar, s: &mut Shared, act: Activity, tool: Option<&(String, String)>) {
    a.idle_since = None;
    a.reported = false;
    let label = label_for(act, tool);
    match free_desk(a, s) {
        Some(d) => {
            let seat = &s.layout.desks[d];
            if a.seated != Some(SeatRef::Desk(d)) {
                a.q.push_back(Step::Walk { to: seat.access(), label: "Walking to the desk".into(), path: None, tag: Tag::None });
            }
            a.q.push_back(Step::Wait { anim: anim_for(act), sit: Some(SeatRef::Desk(d)), face: None, label, until: Until::Replanned, started: false, bubble: None, done: Done::None });
        }
        None => a.q.push_back(Step::Wait { anim: Anim::Stand, sit: None, face: Some(Face::DR), label, until: Until::Replanned, started: false, bubble: None, done: Done::None }),
    }
}

/// Reports standing beside the lead's desk, facing the lead, then goes on a break.
fn plan_report(a: &mut Avatar, s: &mut Shared, said: String) {
    release_desk(a, s);
    let d = &s.layout.desks[0];
    let cands = [(d.x - 1, d.y, Face::DR), (d.x - 1, d.ay, Face::DR), (d.x + 1, d.ay, Face::DL), (d.x, d.y + 1, Face::UR)];
    let ok = |&(x, y, _): &(i32, i32, Face)| !s.layout.blocked(x, y) && !s.reporting.contains(&tile(x, y)) && !s.busy_tiles.iter().any(|(id, t)| *id != a.id && *t == tile(x, y));
    let (x, y, face) = cands.iter().copied().find(|c| ok(c)).unwrap_or(cands[0]);
    s.reporting.push(tile(x, y));
    a.q.push_back(Step::Walk { to: tile(x, y), label: "Reporting to the lead".into(), path: None, tag: Tag::None });
    a.q.push_back(Step::Act { anim: Anim::Stand, sit: None, face: Some(face), left: 3.2, started: false, bubble: Some((BubbleKind::Say, said)), label: "Reporting back".into(), tag: Tag::None, done: Done::Reported });
}

fn lead_break(a: &mut Avatar, s: &mut Shared) {
    let seat = free_lounge_seat(a, s, Some("couch")).or_else(|| free_lounge_seat(a, s, None));
    match seat {
        Some(i) => coffee_break(a, s, i, Some("Waiting for your input")),
        None => a.q.push_back(Step::Wait { anim: Anim::Stand, sit: None, face: Some(Face::DR), label: "Waiting for you".into(), until: Until::Replanned, started: false, bubble: Some((BubbleKind::Think, "Waiting for your input".into())), done: Done::None }),
    }
}

fn free_lounge_seat(a: &Avatar, s: &mut Shared, kind: Option<&str>) -> Option<usize> {
    let c: Vec<usize> = (0..s.lounge_owner.len())
        .filter(|&i| s.lounge_owner[i].is_none() && kind.is_none_or(|k| s.layout.lounge.seats[i].table == Some(k)))
        .collect();
    if c.is_empty() {
        return None;
    }
    let i = c[s.rng.below(c.len())];
    s.lounge_owner[i] = Some(a.id);
    Some(i)
}

fn bar_busy(s: &Shared, except: u32, tag: Tag) -> bool {
    s.bar_busy.iter().any(|(id, t)| *id != except && *t == tag)
}

fn brew(s: &Shared) -> [Step; 2] {
    let bar = s.layout.lounge.bar;
    [
        Step::Walk { to: bar, label: "Getting a coffee".into(), path: None, tag: Tag::Bar },
        Step::Act { anim: Anim::Stand, sit: None, face: Some(Face::UR), left: 2.6, started: false, bubble: None, label: "Making a coffee".into(), tag: Tag::Bar, done: Done::Cup },
    ]
}

/// One person at the machine, at most one waiting behind them; anyone else skips the coffee.
fn coffee_break(a: &mut Avatar, s: &mut Shared, seat: usize, think: Option<&str>) {
    if !bar_busy(s, a.id, Tag::Bar) && !bar_busy(s, a.id, Tag::BarQueue) {
        a.q.extend(brew(s));
    } else if !bar_busy(s, a.id, Tag::BarQueue) {
        a.q.push_back(Step::Walk { to: s.layout.lounge.bar_queue, label: "Queueing for coffee".into(), path: None, tag: Tag::BarQueue });
        a.q.push_back(Step::Wait { anim: Anim::Stand, sit: None, face: Some(Face::UR), label: "Waiting for the coffee machine".into(), until: Until::BarFree, started: false, bubble: None, done: Done::None });
        a.q.push_back(Step::Call(Call::Brew));
    }
    let st = &s.layout.lounge.seats[seat];
    let couch = st.kind == SeatKind::Couch;
    a.q.push_back(Step::Walk { to: st.access(), label: if couch { "Heading to the couch" } else { "Finding a seat" }.into(), path: None, tag: Tag::None });
    let label = if couch { "On the couch" } else { "Coffee at the cafe table" };
    let dur = if think.is_some() { 1e9 } else { 14.0 + s.rng.next() * 12.0 };
    a.q.push_back(Step::Act {
        anim: Anim::Sit,
        sit: Some(SeatRef::Lounge(seat)),
        face: None,
        left: dur,
        started: false,
        bubble: think.map(|t| (BubbleKind::Think, t.to_owned())),
        label: label.into(),
        tag: Tag::None,
        done: Done::StandUp,
    });
}

/// Free time: join or start a ping pong game, take a coffee break, or wander; idle helpers
/// clock out once there are plenty of them.
fn idle(a: &mut Avatar, s: &mut Shared, idle_count: usize, first_idle: Option<u32>, game_waiting: Option<u32>) {
    if a.busy() {
        return;
    }
    if a.mode == Mode::WaitForUser {
        lead_break(a, s);
        return;
    }
    let idle_for = a.idle_since.map_or(0.0, |t| s.elapsed - t);
    if !a.lead && ((idle_count > 3 && first_idle == Some(a.id) && s.rng.next() < 0.3) || idle_for > 300.0) {
        a.leaving = true;
        stand_up(a, s);
        release_desk(a, s);
        a.q.push_back(Step::Walk { to: s.layout.door, label: "Clocking out".into(), path: None, tag: Tag::None });
        a.q.push_back(Step::Leave);
        return;
    }
    if !a.lead {
        // join a colleague who is waiting at the ping pong table
        if game_waiting.is_some_and(|w| w != a.id) && s.rng.next() < 0.85 {
            if let Some(g) = s.game.as_mut() {
                if g.state == GameState::Waiting {
                    g.b = Some(a.id);
                    g.state = GameState::Joining;
                    a.q.push_back(Step::Walk { to: s.layout.lounge.pp_b, label: "Joining the ping pong game".into(), path: None, tag: Tag::None });
                    a.q.push_back(Step::Call(Call::StartGame));
                    return;
                }
            }
        }
        // start a game only when someone else is free to play too
        if s.game.is_none() && idle_count >= 2 && s.rng.next() < 0.55 {
            s.game = Some(Game { state: GameState::Waiting, a: a.id, b: None, t0: 0.0, end: 0.0, score: [0, 0], next_point: 0.0, since: s.elapsed });
            a.q.push_back(Step::Walk { to: s.layout.lounge.pp_a, label: "Off to the ping pong table".into(), path: None, tag: Tag::None });
            a.q.push_back(Step::Wait { anim: Anim::Stand, sit: None, face: Some(Face::DR), label: "Waiting for a ping pong partner".into(), until: Until::PartnerOrTimeout, started: false, bubble: Some((BubbleKind::Say, "Ping pong, anyone?".into())), done: Done::CancelGame });
            a.q.push_back(Step::Call(Call::PlayFront));
            return;
        }
    }
    if let Some(seat) = free_lounge_seat(a, s, None) {
        coffee_break(a, s, seat, None);
        return;
    }
    let lz = &s.layout.lounge;
    let keep: Vec<Tile> = [lz.bar, lz.bar_queue, lz.pp_a, lz.pp_b].into_iter().chain(lz.seats.iter().map(|q| q.access())).collect();
    for _ in 0..12 {
        let x = s.layout.l0 + s.rng.below(4) as i32;
        let y = 1 + s.rng.below((s.layout.h - 2) as usize) as i32;
        let t = tile(x, y);
        if !s.layout.blocked(x, y) && !s.busy_tiles.iter().any(|(id, b)| *id != a.id && *b == t) && !keep.contains(&t) {
            a.q.push_back(Step::Walk { to: t, label: "Stretching legs".into(), path: None, tag: Tag::None });
            break;
        }
    }
    let face = if s.rng.next() < 0.5 { Face::DR } else { Face::DL };
    a.q.push_back(Step::Act { anim: Anim::Stand, sit: None, face: Some(face), left: 3.0 + s.rng.next() * 3.0, started: false, bubble: None, label: "Hanging around".into(), tag: Tag::None, done: Done::None });
}

fn finish(a: &mut Avatar, s: &mut Shared, done: Done) {
    match done {
        Done::None => {}
        Done::Cup => s.cups += 1,
        Done::StandUp => stand_up(a, s),
        Done::CancelGame => {
            if s.game.as_ref().is_some_and(|g| g.a == a.id && g.state == GameState::Waiting) {
                s.game = None;
            }
        }
        Done::EndGame => {
            if s.game.as_ref().is_some_and(|g| g.a == a.id || g.b == Some(a.id)) {
                s.game = None;
            }
        }
        Done::Reported => {
            a.reported = true;
            s.reporting.clear();
            a.mode = Mode::Lounge;
            a.idle_since = Some(s.elapsed);
        }
    }
}

fn update(a: &mut Avatar, s: &mut Shared, dt: f32, idle_count: usize, first_idle: Option<u32>, game_waiting: Option<u32>) {
    a.t += dt;
    if let Some(b) = a.bubble.as_mut() {
        b.left -= dt;
        if b.left <= 0.0 {
            a.bubble = None;
        }
    }
    if a.q.is_empty() {
        idle(a, s, idle_count, first_idle, game_waiting);
    }
    let Some(step) = a.q.front_mut() else {
        a.anim = if a.seated.is_some() { seated_anim(a) } else { Anim::Stand };
        return;
    };
    match step {
        Step::Walk { to, label, path, .. } => {
            let (to, label, planned) = (*to, label.clone(), path.take());
            let mut path = match planned {
                Some(p) => p,
                None => {
                    if let Some(seat) = a.seated {
                        // already sitting where this walk leads: stay seated, no stand-up flicker
                        let here = match seat {
                            SeatRef::Desk(i) => s.layout.desks[i].access(),
                            SeatRef::Lounge(i) => s.layout.lounge.seats[i].access(),
                        };
                        let next_sits_here = a.q.get(1).is_some_and(|n| matches!(n, Step::Act { sit: Some(st), .. } | Step::Wait { sit: Some(st), .. } if *st == seat));
                        if here == to && next_sits_here {
                            a.q.pop_front();
                            return;
                        }
                        stand_up(a, s);
                    }
                    a.label = label;
                    s.layout.path((a.x, a.y), to).unwrap_or_default()
                }
            };
            a.anim = Anim::Walk;
            walk_along(a, &mut path, dt * 2.1);
            if path.is_empty() {
                a.x = a.x.round();
                a.y = a.y.round();
                a.q.pop_front();
            } else if let Some(Step::Walk { path: slot, .. }) = a.q.front_mut() {
                *slot = Some(path);
            }
        }
        Step::Act { sit, face, left, started, bubble, label, .. } => {
            if !*started {
                *started = true;
                let (sit, face, bubble, label, total) = (*sit, *face, bubble.take(), label.clone(), *left);
                start_step(a, s, sit, face, bubble, label, total.min(4.2));
            }
            let Some(Step::Act { anim, left, done, .. }) = a.q.front_mut() else { return };
            *left -= dt;
            let (anim, over, d) = (*anim, *left <= 0.0, *done);
            a.anim = anim;
            if over {
                a.q.pop_front();
                finish(a, s, d);
            }
        }
        Step::Wait { started, sit, face, bubble, label, .. } => {
            if !*started {
                *started = true;
                let (sit, face, bubble, label) = (*sit, *face, bubble.take(), label.clone());
                start_step(a, s, sit, face, bubble, label, 4.2);
            }
            let Some(Step::Wait { anim, until, done, label, .. }) = a.q.front() else { return };
            a.anim = *anim;
            a.label.clone_from(label);
            let over = match until {
                Until::Replanned => false,
                Until::BarFree => !bar_busy(s, a.id, Tag::Bar),
                Until::PartnerOrTimeout => match &s.game {
                    Some(g) if g.a == a.id => g.state == GameState::Playing || (g.state == GameState::Waiting && s.elapsed - g.since > 16.0),
                    _ => true,
                },
                Until::GameOver => s.game.as_ref().is_none_or(|g| g.state != GameState::Playing || s.elapsed > g.end),
            };
            if over {
                let d = *done;
                a.q.pop_front();
                finish(a, s, d);
            }
        }
        Step::Call(c) => {
            let c = *c;
            a.q.pop_front();
            match c {
                Call::Brew => {
                    let [w, act] = brew(s);
                    a.q.push_front(act);
                    a.q.push_front(w);
                }
                Call::PlayFront => {
                    if s.game.as_ref().is_some_and(|g| g.a == a.id && g.state == GameState::Playing) {
                        a.q.push_back(Step::Wait { anim: Anim::PingPongFront, sit: None, face: Some(Face::DR), label: "Ping pong".into(), until: Until::GameOver, started: false, bubble: None, done: Done::EndGame });
                    }
                }
                Call::StartGame => {
                    let ready = s.game.as_ref().is_some_and(|g| g.b == Some(a.id) && g.state == GameState::Joining);
                    if ready {
                        let (end, next) = (40.0 + s.rng.next() * 30.0, 2.0 + s.rng.next() * 3.0);
                        if let Some(g) = s.game.as_mut() {
                            g.state = GameState::Playing;
                            g.t0 = s.elapsed;
                            g.end = s.elapsed + end;
                            g.score = [0, 0];
                            g.next_point = next;
                        }
                        a.q.push_back(Step::Wait { anim: Anim::PingPongBack, sit: None, face: Some(Face::UL), label: "Ping pong".into(), until: Until::GameOver, started: false, bubble: None, done: Done::EndGame });
                    }
                }
                Call::LeadBreak => lead_break(a, s),
            }
        }
        Step::Leave => {
            a.anim = Anim::Stand;
            a.alpha -= dt * 1.5;
            if a.alpha <= 0.0 {
                a.gone = true;
                a.q.pop_front();
            }
        }
    }
}

/// Moves `budget` tiles along the path, turning to face each step.
fn walk_along(a: &mut Avatar, path: &mut Vec<Tile>, mut budget: f32) {
    while budget > 0.0 {
        let Some(n) = path.first().copied() else { break };
        let (dx, dy) = (n.x as f32 - a.x, n.y as f32 - a.y);
        let d = (dx * dx + dy * dy).sqrt();
        if dx.abs() > 0.01 {
            a.face = if dx > 0.0 { Face::DR } else { Face::UL };
        } else if dy.abs() > 0.01 {
            a.face = if dy > 0.0 { Face::DL } else { Face::UR };
        }
        if d <= budget {
            a.x = n.x as f32;
            a.y = n.y as f32;
            path.remove(0);
            budget -= d;
        } else {
            a.x += dx / d * budget;
            a.y += dy / d * budget;
            budget = 0.0;
        }
    }
}

fn seated_anim(a: &Avatar) -> Anim {
    match a.seated {
        Some(SeatRef::Desk(_)) => Anim::Type,
        _ => Anim::Sit,
    }
}

fn start_step(a: &mut Avatar, s: &mut Shared, sit: Option<SeatRef>, face: Option<Face>, bubble: Option<(BubbleKind, String)>, label: String, bubble_dur: f32) {
    match sit {
        Some(seat) if a.seated != Some(seat) => {
            stand_up(a, s);
            sit_on(a, s, seat);
        }
        Some(_) => {}
        None => stand_up(a, s),
    }
    if let Some(f) = face {
        a.face = f;
    }
    if let Some((k, t)) = bubble {
        a.say(k, t, bubble_dur);
    }
    a.label = label;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Event, SessionKey};

    fn ev(agent: AgentRef, kind: EventKind) -> Event {
        Event { session: SessionKey { harness: "t", id: "s".into() }, agent, at: SystemTime::now(), kind }
    }

    /// Runs the office for a while and checks the rules the prototype was tuned for.
    #[test]
    fn office_follows_the_session_and_keeps_its_manners() {
        let key = SessionKey { harness: "t", id: "s".into() };
        let mut session = Session::new(key, "Test", SystemTime::now());
        let mut w = World::new("proj", 7);
        let lead = AgentRef::Lead;
        session.apply(&ev(lead.clone(), EventKind::UserPrompt { text: "go".into() }));
        session.apply(&ev(lead.clone(), EventKind::ToolCall { call_id: "1".into(), name: "Read".into(), summary: "a.rs".into() }));
        for i in 0..4 {
            session.apply(&ev(lead.clone(), EventKind::SubagentStart { agent_id: format!("s{i}"), description: format!("task {i}"), kind: None }));
        }
        let mut two_at_bar = 0;
        for frame in 0..(60 * 120) {
            if frame == 60 * 20 {
                for i in 0..4 {
                    session.apply(&ev(lead.clone(), EventKind::SubagentEnd { agent_id: format!("s{i}") }));
                }
                session.apply(&ev(lead.clone(), EventKind::TurnEnd));
            }
            w.sync(&session);
            w.step(1.0 / 60.0);
            let at_bar = w.avatars.iter().filter(|a| a.q.front().is_some_and(|s| matches!(s, Step::Act { tag: Tag::Bar, started: true, .. }))).count();
            if at_bar > 1 {
                two_at_bar += 1;
            }
            if frame == 60 * 10 {
                assert_eq!(w.avatars.len(), 5);
                let lead_av = w.avatars.iter().find(|a| a.lead).unwrap();
                assert!(lead_av.is_seated_at_desk() && lead_av.anim == Anim::Read, "the lead reads at its desk");
                let seated = w.avatars.iter().filter(|a| a.is_seated_at_desk()).count();
                assert_eq!(seated, 5, "every working agent sits at a desk");
            }
            // nobody plays ping pong alone
            if let Some(g) = &w.s.game {
                if g.state == GameState::Playing {
                    assert!(g.b.is_some());
                }
            }
        }
        assert_eq!(two_at_bar, 0, "one person at the coffee machine at a time");
        let lead_av = w.avatars.iter().find(|a| a.lead).unwrap();
        assert_eq!(lead_av.mode, Mode::WaitForUser);
    }

    #[test]
    fn palettes_differ_inside_an_office() {
        let a = palette_for(1, &[]);
        let b = palette_for(1, &[&a]);
        assert_ne!(a.cols[1], b.cols[1]);
    }
}

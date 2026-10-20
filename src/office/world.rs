//! The people in one office and what they are doing this frame.
//!
//! Each avatar runs a small queue of steps (walk somewhere, act for a while, wait until
//! something happens). The director in [`World::sync`] compares what the session says each
//! agent is doing with what its avatar is doing and replans when they differ, so the office
//! always follows the real work while the in-between moments (coffee, ping pong, the couch)
//! stay lively on their own.

use super::gen::{generate, generate_for, Layout, Play, SeatKind, MAX_CREW};
use super::{hash, tile, Face, Rng, Tile};
use crate::model::{Activity, AgentRef, Event, EventKind};
use crate::session::{Session, Status};
use std::collections::{HashMap, HashSet, VecDeque};
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
    /// at one of the lounge games
    Play(Play),
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

/// The colours that mark agents in the HUD and on the floor.
const COLOURS: [[u8; 3]; 12] = [[214, 104, 56], [58, 148, 138], [214, 164, 52], [72, 128, 82], [132, 82, 142], [62, 84, 146], [188, 62, 64], [112, 122, 138], [222, 132, 164], [226, 210, 178], [52, 54, 64], [96, 170, 206]];

/// An agent's colour from a seed: within one office nobody shares one until all are taken.
pub fn colour_for(seed: u32, avoid: &[[u8; 3]]) -> [u8; 3] {
    let mut r = Rng::new(seed);
    for _ in 0..4 {
        r.next(); // similar seeds start out correlated
    }
    let free: Vec<_> = COLOURS.iter().filter(|c| !avoid.contains(c)).copied().collect();
    *r.pick(if free.is_empty() { &COLOURS[..] } else { &free })
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
    /// playing lounge game i
    Game(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Until {
    /// until the director replans
    Replanned,
    BarFree,
    PartnerOrTimeout,
    GameOver,
    /// until someone joins lounge game i, or nobody does for a while
    Partner(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Done {
    None,
    Cup,
    StandUp,
    CancelGame,
    EndGame,
    Reported,
    /// leaves the lounge game it played
    Unplay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Brew,
    PlayFront,
    StartGame,
    LeadBreak,
    /// starts lounge game i if the partner came
    Play(u8),
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
    pub colour: [u8; 3],
    pub x: f32,
    pub y: f32,
    pub face: Face,
    /// the way the body points, in the room (radians, 0 along +x); turns smoothly toward `face`
    /// or the way it walks
    pub heading: f32,
    /// tiles walked since this walk began, which drives the walk cycle
    pub stride: f32,
    speed: f32,
    walk_dir: f32,
    /// the pose this avatar just left, drawn over the new one as it fades out
    pub fade: Option<Fade>,
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
    /// seconds before this avatar walks in; arrivals come through the door one by one
    arrive_in: f32,
    /// what the agent is asking you, while it waits on you: it raises its hand
    pub needs: Option<String>,
    /// it should be working but has done nothing for a while: it slumps and dozes
    pub stuck: bool,
    /// when (in office time) a tool call last failed, and when the context was last compacted
    pub failed_at: f32,
    pub tidied_at: f32,
    /// how full its context is (0..1), and whether that is near the point it compacts at
    pub gauge: f32,
    pub gauge_near: bool,
    /// a custom character's own animation and how much longer it plays
    pub extra: Option<(&'static str, f32)>,
    /// tool calls of its that failed
    pub errors: u32,
    pub team: Option<String>,
    /// the session's error and compaction counts this avatar last saw
    seen: Option<(u32, u32)>,
}

/// Everything the renderer needs to draw one avatar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub anim: Anim,
    pub phase: f32,
    pub face: Face,
    pub heading: f32,
    pub stride: f32,
    pub t: f32,
    pub x: f32,
    pub y: f32,
    pub seated: Option<SeatRef>,
    /// slumped over, dozing
    pub slump: bool,
    /// one of a custom character's own animations, played while it stands
    pub extra: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fade {
    pub pose: Pose,
    pub left: f32,
    pub dur: f32,
}

/// Walking pace in tiles per second, how quickly it is reached, and how fast a body turns.
const WALK_SPEED: f32 = 2.1;
const ACCEL: f32 = 3.6;
const TURN: f32 = 8.0;

/// The room angle of a facing.
pub fn face_angle(f: Face) -> f32 {
    use std::f32::consts::{FRAC_PI_2, PI};
    match f {
        Face::DR => 0.0,
        Face::DL => FRAC_PI_2,
        Face::UL => PI,
        Face::UR => -FRAC_PI_2,
    }
}

/// An angle wrapped into (-π, π].
fn wrap(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let a = (a + PI).rem_euclid(TAU) - PI;
    if a <= -PI {
        a + TAU
    } else {
        a
    }
}

impl Avatar {
    pub fn pose(&self) -> Pose {
        Pose { anim: self.anim, phase: self.phase, face: self.face, heading: self.heading, stride: self.stride, t: self.t, x: self.x, y: self.y, seated: self.seated, slump: self.stuck, extra: self.extra.map(|e| e.0).filter(|_| self.anim == Anim::Stand && self.seated.is_none()) }
    }
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

/// Ping pong timing: one return trip of the ball, and where in each player's swing the paddle
/// meets it.
pub const PP_PERIOD: f32 = 1.05;
pub const PP_HIT_A: f32 = 9.0 / 15.0;
pub const PP_HIT_B: f32 = 10.0 / 16.0;
/// The pause after a point before the next serve.
const PP_REST: f32 = 1.4;

#[derive(Debug, Clone)]
pub struct Game {
    pub state: GameState,
    /// player 0 at the near end, player 1 at the far end
    pub a: u32,
    pub b: Option<u32>,
    /// the rally clock: swings and the ball are timed from here
    pub t0: f32,
    pub end: f32,
    pub score: [u32; 2],
    /// who serves this rally, and how many times the ball crosses before the miss
    pub serve: usize,
    pub flights: u32,
    /// no swinging before this: the pause between points
    pub rest_until: f32,
    /// when the last point went in, and to whom
    pub point_at: f32,
    pub scorer: usize,
    /// the ball that was missed: who hit it and when it passed the other player
    pub out: Option<(usize, f32)>,
    since: f32,
}

/// Where the ball is: crossing from one player to the other (`k` from 0 at the hit to 1 at the
/// other paddle), or flying on past the player who missed it (`k` beyond 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ball {
    Flight { from: usize, k: f32 },
    Out { from: usize, k: f32 },
}

impl Game {
    /// When the server's paddle first meets the ball.
    pub fn first_hit(&self) -> f32 {
        self.t0 + (PP_HIT_A + 0.5 * self.serve as f32) * PP_PERIOD
    }

    /// When the last ball of this rally reaches the player who will miss it.
    fn miss_at(&self) -> f32 {
        self.first_hit() + self.flights as f32 * 0.5 * PP_PERIOD
    }

    /// Players swing while a rally is on.
    pub fn swinging(&self, now: f32) -> bool {
        self.state == GameState::Playing && now >= self.rest_until
    }

    pub fn ball(&self, now: f32) -> Option<Ball> {
        if self.state != GameState::Playing {
            return None;
        }
        if let Some((from, at)) = self.out {
            let k = 1.0 + (now - at) / (0.5 * PP_PERIOD);
            if k < 1.7 {
                return Some(Ball::Out { from, k });
            }
        }
        if now < self.rest_until {
            return None;
        }
        let f = (now - self.first_hit()) / (0.5 * PP_PERIOD);
        if f < 0.0 || f >= self.flights as f32 {
            return None;
        }
        Some(Ball::Flight { from: (self.serve + f as usize) % 2, k: f.fract() })
    }

    /// Lines the next rally up: `serve` serves after the pause that starts now.
    fn next_rally(&mut self, now: f32, serve: usize, flights: u32) {
        self.serve = serve;
        self.flights = flights;
        self.rest_until = now + PP_REST;
        self.t0 = self.rest_until + 0.35 * PP_PERIOD - (PP_HIT_A + 0.5 * serve as f32) * PP_PERIOD;
    }
}

/// Everything except the avatars, so an avatar can update while reading shared state.
pub struct Shared {
    pub layout: Layout,
    pub elapsed: f32,
    pub cups: u32,
    pub game: Option<Game>,
    /// the score the board keeps showing after a game
    pub pp_last: [u32; 2],
    pub desk_owner: Vec<Option<u32>>,
    pub lounge_owner: Vec<Option<u32>>,
    rng: Rng,
    reporting: Vec<Tile>,
    /// tiles someone stands on or is walking to, refreshed every frame
    busy_tiles: Claims,
    /// tiles nobody stands on: seats, the floor a workstation's drawing covers, the paths to
    /// them, the bar and the door
    keep_clear: HashSet<Tile>,
    bar_busy: Vec<(u32, Tag)>,
    chat_t: f32,
    /// who plays each lounge game: the first player, then the partner
    pub play: Vec<[Option<u32>; 2]>,
    play_since: Vec<f32>,
    /// avatars playing a lounge game right now
    playing: Vec<u32>,
    /// each avatar's team, so a team sits together
    teams: HashMap<u32, String>,
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
    /// The colour that marks this agent in the HUD and on the floor.
    pub fn color(&self) -> [u8; 3] {
        self.colour
    }
}

pub struct World {
    pub avatars: Vec<Avatar>,
    pub s: Shared,
    pub project: String,
    next_id: u32,
    seed: u32,
    /// the crew the room was last built for
    built_for: usize,
    /// counts up each time the room is rebuilt bigger, so its painted floor and walls are redone
    pub generation: u32,
    /// how long an agent that should be working may do nothing before it counts as stuck
    pub stuck_after: Duration,
}

/// Who has claimed which floor tile; a tile claimed by two is nobody's to take.
#[derive(Default)]
struct Claims(HashMap<Tile, (u32, bool)>);

impl Claims {
    fn claim(&mut self, id: u32, t: Tile) {
        self.0.entry(t).and_modify(|e| e.1 |= e.0 != id).or_insert((id, false));
    }

    fn taken(&self, id: u32, t: Tile) -> bool {
        self.0.get(&t).is_some_and(|&(who, shared)| shared || who != id)
    }
}

/// Minutes of nothing before an agent that should be working counts as stuck, by default.
pub const STUCK_SECS: u64 = 8 * 60;

/// How many more people than desks a room takes in before it grows; they wait at the whiteboard.
const SPARE: usize = 2;

/// Rooms are built for crews in steps, so a crew growing by one does not rebuild the room each time.
fn crew_step(crew: usize) -> usize {
    [24, 48, 96, 192, 320, MAX_CREW].into_iter().find(|&t| t >= crew).unwrap_or(MAX_CREW)
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
        Self::for_crew(project, seed, 0)
    }

    /// The office for a project with `crew` people in it at once: the seed's own room, grown when
    /// they would not fit at its desks.
    pub fn for_crew(project: &str, seed: u32, crew: usize) -> Self {
        let base = generate(seed);
        let built_for = if base.desks.len() + SPARE >= crew { 0 } else { crew_step(crew) };
        let layout = if built_for == 0 { base } else { generate_for(seed, built_for) };
        Self { avatars: Vec::new(), s: Shared::new(layout, seed), project: project.to_owned(), next_id: 1, seed, built_for, generation: 0, stuck_after: Duration::from_secs(STUCK_SECS) }
    }

    /// An empty office in this one's room, which never grows: a replay plays out in it, so the
    /// floor and walls already painted for this one serve it too.
    pub fn same_room(&self) -> Self {
        let mut s = Shared::new(self.s.layout.clone(), self.seed);
        s.rng = Rng::new(self.seed ^ 0x51ed_270b);
        Self { avatars: Vec::new(), s, project: self.project.clone(), next_id: 1, seed: self.seed, built_for: MAX_CREW, generation: self.generation, stuck_after: self.stuck_after }
    }

    /// Rebuilds the room bigger for `crew` people, keeping its look and everyone in it: they stand
    /// where they were (or the nearest free floor) and go about their work from there.
    fn grow(&mut self, crew: usize) {
        let target = crew_step(crew);
        if target <= self.built_for {
            return;
        }
        self.built_for = target;
        let mut layout = generate_for(self.seed, target);
        if layout.desks.len() <= self.s.layout.desks.len() {
            return;
        }
        layout.theme = self.s.layout.theme;
        let old = std::mem::replace(&mut self.s, Shared::new(layout, self.seed));
        (self.s.elapsed, self.s.cups, self.s.pp_last, self.s.rng) = (old.elapsed, old.cups, old.pp_last, old.rng);
        self.generation += 1;
        let s = &mut self.s;
        for a in &mut self.avatars {
            a.q.clear();
            (a.seated, a.desk, a.fade, a.speed, a.anim) = (None, None, None, 0.0, Anim::Stand);
            if a.lead && !s.layout.desks.is_empty() {
                s.desk_owner[0] = Some(a.id);
                a.desk = Some(0);
                let d = s.layout.desks[0].access();
                (a.x, a.y) = (d.x as f32, d.y as f32);
            } else {
                let here = tile((a.x.round() as i32).clamp(0, s.layout.w - 1), (a.y.round() as i32).clamp(0, s.layout.h - 1));
                let t = free_spot(a, s, here).unwrap_or(s.layout.door);
                (a.x, a.y) = (t.x as f32, t.y as f32);
            }
            // working again from here: the next sync plans everyone afresh
            a.mode = Mode::Lounge;
            a.idle_since = Some(s.elapsed);
            if a.leaving {
                a.q.push_back(Step::Walk { to: s.layout.door, label: "Clocking out".into(), path: None, tag: Tag::None });
                a.q.push_back(Step::Leave);
            }
        }
    }
}

impl Shared {
    fn new(layout: Layout, seed: u32) -> Self {
        let clear = keep_clear(&layout);
        let layout_games = layout.lounge.games.len();
        Shared {
            desk_owner: vec![None; layout.desks.len()],
            lounge_owner: vec![None; layout.lounge.seats.len()],
            rng: Rng::new(seed ^ 0x9e37_79b9),
            layout,
            elapsed: 0.0,
            cups: 0,
            game: None,
            pp_last: [0, 0],
            reporting: Vec::new(),
            busy_tiles: Claims::default(),
            keep_clear: clear,
            bar_busy: Vec::new(),
            chat_t: 2.0,
            play: vec![[None, None]; layout_games],
            play_since: vec![0.0; layout_games],
            playing: Vec::new(),
            teams: HashMap::new(),
        }
    }
}

impl World {

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
        // one of the characters fewest people here are already: a crowd repeats them evenly
        let pool = crate::custom::pool();
        let used = |c: u8| self.avatars.iter().filter(|a| !a.gone && a.char_id == c).count();
        let least = pool.ids.iter().map(|&c| used(c)).min().unwrap_or(0);
        let free: Vec<u8> = pool.ids.iter().copied().filter(|&c| used(c) == least).collect();
        let char_id = match pool.lead.filter(|_| lead) {
            Some(c) => c,
            None => free.get((h as usize) % free.len().max(1)).copied().unwrap_or(0),
        };
        let avoid: Vec<[u8; 3]> = self.avatars.iter().filter(|a| !a.gone).map(|a| a.colour).collect();
        let colour = colour_for(h, &avoid);
        let door = self.s.layout.door;
        let mut av = Avatar {
            id: self.next_id,
            agent: agent.clone(),
            name: name.to_owned(),
            role,
            lead,
            char_id,
            colour,
            x: door.x as f32,
            y: door.y as f32,
            face: Face::DR,
            heading: 0.0,
            stride: 0.0,
            speed: 0.0,
            walk_dir: 0.0,
            fade: None,
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
            arrive_in: 0.0,
            needs: None,
            stuck: false,
            failed_at: -100.0,
            tidied_at: -100.0,
            gauge: 0.0,
            gauge_near: false,
            extra: None,
            errors: 0,
            team: None,
            seen: None,
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

    /// Reacts to one live event with a bubble over the right person. `now` is the session's clock
    /// (a replay's runs behind the wall's).
    pub fn cue(&mut self, e: &Event, now: SystemTime) {
        if now.duration_since(e.at).unwrap_or_default() > Duration::from_secs(20) {
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
    pub fn sync(&mut self, session: &Session, now: SystemTime) {
        // a crew that has outgrown the desks gets a bigger room
        let crew = session.live_agents().count();
        if crew > self.s.layout.desks.len() + SPARE && self.built_for < MAX_CREW {
            self.grow(crew);
        }
        let mut arrivals = 0;
        let here: HashSet<&AgentRef> = self.avatars.iter().filter(|a| !a.gone).map(|a| &a.agent).collect();
        let hire: Vec<&crate::session::AgentState> = session.agents.values().filter(|st| !st.done && !here.contains(&st.id)).collect();
        let room = self.s.layout.desks.len() + SPARE;
        let mut live = self.avatars.iter().filter(|a| !a.gone && !a.leaving).count();
        // newcomers come in one by one, closer together when a crowd arrives at once
        let waiting = hire.len() + self.avatars.iter().filter(|a| a.arrive_in > 0.0).count();
        let gap = (6.6 / waiting.max(1) as f32).clamp(0.1, 1.1);
        for st in hire {
            {
                if st.id != AgentRef::Lead && live >= room {
                    continue;
                }
                live += 1;
                let i = self.hire(&st.id, &st.label, st.kind.clone());
                if st.id != AgentRef::Lead {
                    let queue = self.avatars.iter().filter(|a| a.arrive_in > 0.0).map(|a| a.arrive_in).fold(0.0, f32::max);
                    let a = &mut self.avatars[i];
                    a.arrive_in = if arrivals == 0 && queue == 0.0 { 0.01 } else { queue + gap };
                    a.alpha = 0.0;
                    arrivals += 1;
                }
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
            // asked something: it says what, and keeps saying it until you answer
            if let Some(q) = st.needs.as_ref().filter(|q| a.needs.as_ref() != Some(*q) || a.bubble.is_none()) {
                a.say(BubbleKind::Say, q.clone(), 30.0);
            }
            a.needs.clone_from(&st.needs);
            a.stuck = session.stuck(st, now, self.stuck_after);
            if a.team != st.team {
                a.team.clone_from(&st.team);
                match &st.team {
                    Some(t) => self.s.teams.insert(a.id, t.clone()),
                    None => self.s.teams.remove(&a.id),
                };
            }
            // a failed tool call puffs and a compaction tidies the desk, but only for what happens
            // while you watch, not for what the log already held
            let counts = (st.errors, st.compactions);
            if let Some((errs, comps)) = a.seen {
                let fresh = |t: Option<SystemTime>| t.is_some_and(|t| now.duration_since(t).unwrap_or_default() < Duration::from_secs(20));
                if counts.0 > errs && fresh(st.error_at) {
                    a.failed_at = self.s.elapsed;
                }
                if counts.1 > comps && fresh(st.compacted_at) {
                    a.tidied_at = self.s.elapsed;
                    a.say(BubbleKind::Think, "Tidying up my notes", 2.5);
                }
            }
            a.seen = Some(counts);
            a.errors = st.errors;
            let want = if a.lead {
                match session.status {
                    // asked something: it stays at its desk with its hand up
                    _ if st.needs.is_some() => Mode::Work(Activity::Waiting),
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
            let deskless = matches!(want, Mode::Work(_)) && a.desk.is_none() && self.s.desk_owner.iter().any(|o| o.is_none());
            if want == a.mode && !deskless {
                continue;
            }
            // a new activity at the same desk only swaps the animation, no getting up
            if let (Mode::Work(_), Mode::Work(act), false) = (a.mode, want, deskless) {
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
                Mode::Report => {
                    if let Some(k) = crate::custom::extras(a.char_id, crate::custom::Extra::Finish).next() {
                        a.extra = Some((k, 3.5));
                    }
                    // a team that worked together meets at the whiteboard between phases
                    let mates = st.team.as_ref().map_or(0, |t| session.agents.values().filter(|o| o.team.as_ref() == Some(t)).count());
                    match st.team.as_ref().filter(|_| mates >= 2) {
                        Some(team) => plan_huddle(a, &mut self.s, team),
                        None => plan_report(a, &mut self.s, st.said.clone().unwrap_or_else(|| format!("Done: {}", st.label))),
                    }
                }
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
        s.busy_tiles.0.clear();
        for a in self.avatars.iter().filter(|a| !a.leaving) {
            s.busy_tiles.claim(a.id, tile(a.x.round() as i32, a.y.round() as i32));
            for t in a.q.iter().filter_map(|st| st.target()) {
                s.busy_tiles.claim(a.id, t);
            }
        }
        s.bar_busy = self.avatars.iter().flat_map(|a| a.q.iter().map(move |st| (a.id, st.tag()))).filter(|(_, t)| matches!(t, Tag::Bar | Tag::BarQueue)).collect();
        s.playing = self.avatars.iter().filter(|a| a.q.front().is_some_and(|st| matches!(st, Step::Act { tag: Tag::Game(_), started: true, .. }))).map(|a| a.id).collect();

        // a character of your own plays its extras: once its work is done, and now and then
        // while it stands about
        for a in &mut self.avatars {
            if let Some((_, left)) = a.extra.as_mut() {
                *left -= dt;
                if *left <= 0.0 {
                    a.extra = None;
                }
            } else if a.anim == Anim::Stand && a.seated.is_none() && a.q.front().is_some_and(|st| matches!(st, Step::Act { .. })) && s.rng.next() < dt / 20.0 {
                let mut idle = crate::custom::extras(a.char_id, crate::custom::Extra::Idle);
                if let Some(k) = idle.next() {
                    a.extra = Some((k, 3.0));
                }
            }
        }
        // two people at the same table chat now and then
        s.chat_t -= dt;
        if s.chat_t <= 0.0 {
            s.chat_t = 3.5 + s.rng.next() * 2.0;
            for kind in ["cafe", "couch", "chess", "boardgame"] {
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
        // a rally ends when the ball gets past someone: the point goes in as it passes, then a
        // short pause and the scorer serves; first to 11 wins
        if let Some(g) = s.game.as_mut() {
            if g.state == GameState::Playing && g.score[0] < 11 && g.score[1] < 11 && s.elapsed >= g.miss_at() && s.elapsed >= g.rest_until {
                let side = (g.serve + g.flights as usize + 1) % 2;
                g.score[side] += 1;
                s.pp_last = g.score;
                g.out = Some((side, g.miss_at()));
                g.point_at = s.elapsed;
                g.scorer = side;
                let flights = 3 + s.rng.below(8) as u32;
                g.next_rally(s.elapsed, side, flights);
                let who = if side == 0 { Some(g.a) } else { g.b };
                let line = if g.score[side] >= 11 {
                    // a moment to celebrate before everyone heads off
                    g.end = s.elapsed + 1.8;
                    Some(*s.rng.pick(&["Good game!", "Rematch later?", "That was close"]))
                } else if s.rng.next() < 0.35 {
                    Some("Point!")
                } else {
                    None
                };
                if let (Some(line), Some(w)) = (line, self.avatars.iter_mut().find(|a| Some(a.id) == who)) {
                    w.say(BubbleKind::Say, line, 1.6);
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
        let here: HashSet<u32> = self.avatars.iter().map(|a| a.id).collect();
        self.s.teams.retain(|id, _| here.contains(id));
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
    release_play(a, s);
    if !matches!(a.seated, Some(SeatRef::Desk(_))) {
        stand_up(a, s);
    }
}

fn free_desk(a: &mut Avatar, s: &mut Shared) -> Option<usize> {
    if a.desk.is_some() {
        return a.desk;
    }
    let free: Vec<usize> = (0..s.desk_owner.len()).filter(|&i| s.desk_owner[i].is_none()).collect();
    let at = |i: usize| (s.layout.desks[i].x, s.layout.desks[i].y);
    let gap = |p: (i32, i32), q: (i32, i32)| (p.0 - q.0).abs() + (p.1 - q.1).abs();
    let team = s.teams.get(&a.id);
    // the desks of teammates already seated
    let mates: Vec<(i32, i32)> = match team {
        Some(t) => (0..s.desk_owner.len()).filter(|&i| s.desk_owner[i].is_some_and(|o| o != a.id && s.teams.get(&o) == Some(t))).map(at).collect(),
        None => Vec::new(),
    };
    let i = if !mates.is_empty() {
        // next to the team
        *free.iter().min_by_key(|&&i| (mates.iter().map(|&m| gap(at(i), m)).min().unwrap_or(0), i))?
    } else if team.is_some() {
        // the first of a team takes a desk with room around it for the rest
        *free.iter().max_by_key(|&&i| (free.iter().filter(|&&j| gap(at(i), at(j)) <= 4).count(), std::cmp::Reverse(i)))?
    } else {
        *free.first()?
    };
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
        None => {
            // every desk is taken: gather at the whiteboard, each on a tile of their own
            let board = tile((s.layout.bx0 + s.layout.bx1) / 2, 1);
            if let Some(t) = free_spot(a, s, board) {
                a.q.push_back(Step::Walk { to: t, label: "Over to the whiteboard".into(), path: None, tag: Tag::None });
            }
            a.q.push_back(Step::Wait { anim: Anim::Stand, sit: None, face: Some(Face::UR), label, until: Until::Replanned, started: false, bubble: None, done: Done::None });
        }
    }
}

/// Floor nobody should stand on, worked out once per layout.
fn keep_clear(o: &Layout) -> HashSet<Tile> {
    let mut k = HashSet::new();
    for d in &o.desks {
        // the seat, the way in behind the chair, and the floor the desk's drawing covers
        k.extend([tile(d.x, d.y), d.access(), tile(d.x - 1, d.y)]);
    }
    let lz = &o.lounge;
    for q in &lz.seats {
        k.extend([tile(q.x, q.y), q.access()]);
    }
    k.extend([lz.bar, lz.bar_queue, lz.pp_a, lz.pp_b, o.door]);
    k.extend(lz.games.iter().flat_map(|g| g.spots.iter().map(|s| s.0)));
    k
}

/// Whether `t` is free to stand on for `a`: open floor nobody else has claimed.
fn standable(a: &Avatar, s: &Shared, t: Tile) -> bool {
    !s.layout.blocked(t.x, t.y) && !s.keep_clear.contains(&t) && !s.reporting.contains(&t) && !s.busy_tiles.taken(a.id, t)
}

/// The nearest tile to `from` (walking distance) that `a` can stand on alone, claimed for `a`
/// at once so the next person planned in the same frame looks elsewhere.
fn free_spot(a: &Avatar, s: &mut Shared, from: Tile) -> Option<Tile> {
    let t = nearest_free(a, s, from)?;
    s.busy_tiles.claim(a.id, t);
    Some(t)
}

fn nearest_free(a: &Avatar, s: &Shared, from: Tile) -> Option<Tile> {
    let mut seen = HashSet::from([from]);
    let mut q = VecDeque::from([from]);
    while let Some(t) = q.pop_front() {
        if standable(a, s, t) {
            return Some(t);
        }
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let n = tile(t.x + dx, t.y + dy);
            if !s.layout.blocked(n.x, n.y) && seen.insert(n) {
                q.push_back(n);
            }
        }
    }
    None
}

/// Reports standing across the lead's desk, facing the lead, then goes on a break.
fn plan_report(a: &mut Avatar, s: &mut Shared, said: String) {
    release_desk(a, s);
    let (dx, dy) = (s.layout.desks[0].x, s.layout.desks[0].y);
    let cands = [(dx + 1, dy + 1, Face::UL), (dx, dy + 1, Face::UR), (dx - 1, dy + 1, Face::UR)];
    let pick = cands.iter().copied().find(|&(x, y, _)| standable(a, s, tile(x, y)));
    let (x, y, face) = match pick {
        Some(c) => {
            s.busy_tiles.claim(a.id, tile(c.0, c.1));
            c
        }
        None => match free_spot(a, s, tile(dx, dy + 1)) {
            Some(t) => (t.x, t.y, if t.y > dy { Face::UR } else { Face::DL }),
            None => (a.x.round() as i32, a.y.round() as i32, Face::UR),
        },
    };
    s.reporting.push(tile(x, y));
    a.q.push_back(Step::Walk { to: tile(x, y), label: "Reporting to the lead".into(), path: None, tag: Tag::None });
    a.q.push_back(Step::Act { anim: Anim::Stand, sit: None, face: Some(face), left: 3.2, started: false, bubble: Some((BubbleKind::Say, said)), label: "Reporting back".into(), tag: Tag::None, done: Done::Reported });
}

/// A phase is done: the team stands at the whiteboard a moment, then goes on a break.
fn plan_huddle(a: &mut Avatar, s: &mut Shared, team: &str) {
    release_desk(a, s);
    let board = tile((s.layout.bx0 + s.layout.bx1) / 2, 1);
    if let Some(t) = free_spot(a, s, board) {
        a.q.push_back(Step::Walk { to: t, label: "To the whiteboard".into(), path: None, tag: Tag::None });
    }
    let phase = team.rsplit(" · ").next().filter(|p| !p.starts_with("batch ")).map_or_else(|| "Done, all of us".to_owned(), |p| format!("{p}: done"));
    let wait = 5.0 + s.rng.next() * 3.0;
    a.q.push_back(Step::Act { anim: Anim::Stand, sit: None, face: Some(Face::UR), left: wait, started: false, bubble: Some((BubbleKind::Say, phase)), label: "At the whiteboard with the team".into(), tag: Tag::None, done: Done::Reported });
}

fn lead_break(a: &mut Avatar, s: &mut Shared) {
    let seat = free_lounge_seat(a, s, Some("couch")).or_else(|| free_lounge_seat(a, s, None));
    match seat {
        Some(i) => coffee_break(a, s, i, Some("Waiting for your input")),
        None => {
            let lz = &s.layout.lounge;
            if let Some(t) = free_spot(a, s, lz.pp_a) {
                a.q.push_back(Step::Walk { to: t, label: "Stretching legs".into(), path: None, tag: Tag::None });
            }
            a.q.push_back(Step::Wait { anim: Anim::Stand, sit: None, face: Some(Face::DR), label: "Waiting for you".into(), until: Until::Replanned, started: false, bubble: Some((BubbleKind::Think, "Waiting for your input".into())), done: Done::None });
        }
    }
}

fn free_lounge_seat(a: &Avatar, s: &mut Shared, kind: Option<&str>) -> Option<usize> {
    let c: Vec<usize> = (0..s.lounge_owner.len())
        .filter(|&i| s.lounge_owner[i].is_none() && kind.map_or(matches!(s.layout.lounge.seats[i].table, Some("cafe" | "couch")), |k| s.layout.lounge.seats[i].table == Some(k)))
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
    // claimed at once, so someone else planning in the same frame sees it taken
    if !bar_busy(s, a.id, Tag::Bar) && !bar_busy(s, a.id, Tag::BarQueue) {
        a.q.extend(brew(s));
        s.bar_busy.push((a.id, Tag::Bar));
    } else if !bar_busy(s, a.id, Tag::BarQueue) {
        s.bar_busy.push((a.id, Tag::BarQueue));
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
        release_play(a, s);
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
            s.game = Some(Game { state: GameState::Waiting, a: a.id, b: None, t0: 0.0, end: 0.0, score: [0, 0], serve: 0, flights: 0, rest_until: 0.0, point_at: -10.0, scorer: 0, out: None, since: s.elapsed });
            a.q.push_back(Step::Walk { to: s.layout.lounge.pp_a, label: "Off to the ping pong table".into(), path: None, tag: Tag::None });
            a.q.push_back(Step::Wait { anim: Anim::Stand, sit: None, face: Some(Face::DR), label: "Waiting for a ping pong partner".into(), until: Until::PartnerOrTimeout, started: false, bubble: Some((BubbleKind::Say, "Ping pong, anyone?".into())), done: Done::CancelGame });
            a.q.push_back(Step::Call(Call::PlayFront));
            return;
        }
    }
    if !a.lead && try_play(a, s, idle_count) {
        return;
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
        if standable(a, s, t) && !keep.contains(&t) {
            s.busy_tiles.claim(a.id, t);
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
        Done::Unplay => {
            release_play(a, s);
            stand_up(a, s);
        }
    }
}

/// Finishes a step; whoever it stood up shows standing from this frame on, not the seat's pose.
fn finish_standing(a: &mut Avatar, s: &mut Shared, done: Done) {
    let was = a.seated.is_some();
    finish(a, s, done);
    if was && a.seated.is_none() {
        a.anim = Anim::Stand;
    }
}

fn release_play(a: &Avatar, s: &mut Shared) {
    for p in &mut s.play {
        for slot in p.iter_mut() {
            if *slot == Some(a.id) {
                *slot = None;
            }
        }
    }
}

/// Free time at one of the lounge games: join someone waiting for a partner, or start a game
/// nobody is at (the two-player ones only when someone else is free to come).
fn try_play(a: &mut Avatar, s: &mut Shared, idle_count: usize) -> bool {
    let n = s.layout.lounge.games.len();
    for gi in 0..n {
        let pair = s.layout.lounge.games[gi].kind.pair();
        if pair && s.play[gi][0].is_some_and(|p| p != a.id) && s.play[gi][1].is_none() && s.rng.next() < 0.75 {
            s.play[gi][1] = Some(a.id);
            plan_play(a, s, gi, 1);
            return true;
        }
    }
    if s.rng.next() > 0.55 {
        return false;
    }
    let free: Vec<usize> = (0..n).filter(|&i| s.play[i] == [None, None] && (!s.layout.lounge.games[i].kind.pair() || idle_count >= 2)).collect();
    if free.is_empty() {
        return false;
    }
    let gi = free[s.rng.below(free.len())];
    s.play[gi][0] = Some(a.id);
    s.play_since[gi] = s.elapsed;
    plan_play(a, s, gi, 0);
    true
}

fn plan_play(a: &mut Avatar, s: &mut Shared, gi: usize, slot: usize) {
    let g = s.layout.lounge.games[gi].clone();
    let (to, sit, face) = match g.seats.get(slot).or(g.seats.first()) {
        Some(&seat) => {
            s.lounge_owner[seat] = Some(a.id);
            (s.layout.lounge.seats[seat].access(), Some(SeatRef::Lounge(seat)), None)
        }
        None => {
            let (t, f) = g.spots[slot.min(g.spots.len() - 1)];
            (t, None, Some(f))
        }
    };
    s.busy_tiles.claim(a.id, to);
    let label = g.kind.label();
    a.q.push_back(Step::Walk { to, label: "Off to play".into(), path: None, tag: Tag::None });
    if g.kind.pair() && slot == 0 {
        let wait = if sit.is_some() { Anim::Sit } else { Anim::Stand };
        a.q.push_back(Step::Wait { anim: wait, sit, face, label: "Waiting for someone to play with".into(), until: Until::Partner(gi as u8), started: false, bubble: Some((BubbleKind::Say, "Anyone up for a game?".into())), done: Done::None });
        a.q.push_back(Step::Call(Call::Play(gi as u8)));
    } else {
        let dur = 18.0 + s.rng.next() * 18.0;
        a.q.push_back(Step::Act { anim: Anim::Play(g.kind), sit, face, left: dur, started: false, bubble: None, label: label.into(), tag: Tag::Game(gi as u8), done: Done::Unplay });
    }
}

fn update(a: &mut Avatar, s: &mut Shared, dt: f32, idle_count: usize, first_idle: Option<u32>, game_waiting: Option<u32>) {
    if a.arrive_in > 0.0 {
        a.arrive_in -= dt;
        return;
    }
    let before = a.pose();
    act(a, s, dt, idle_count, first_idle, game_waiting);
    // the body turns toward where it is going or what it faces, never snapping round
    let target = if a.anim == Anim::Walk { a.walk_dir } else { face_angle(a.face) };
    if a.seated.is_some() {
        a.heading = target;
    } else {
        let d = wrap(target - a.heading);
        a.heading = wrap(a.heading + d.clamp(-TURN * dt, TURN * dt));
    }
    if let Some(f) = a.fade.as_mut() {
        f.left -= dt;
        if f.left <= 0.0 {
            a.fade = None;
        }
    }
    // a change of pose cross-fades instead of popping
    if (before.anim != a.anim || before.seated != a.seated) && a.alpha > 0.0 {
        let dur = if before.seated.is_some() || a.seated.is_some() { 0.26 } else { 0.2 };
        a.fade = Some(Fade { pose: before, left: dur, dur });
    }
}

fn act(a: &mut Avatar, s: &mut Shared, dt: f32, idle_count: usize, first_idle: Option<u32>, game_waiting: Option<u32>) {
    if a.alpha < 1.0 && !a.leaving {
        a.alpha = (a.alpha + dt * 3.0).min(1.0);
    }
    // a stuck agent at its desk stops moving
    if !(a.stuck && a.seated.is_some()) {
        a.t += dt;
    }
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
            if a.anim != Anim::Walk {
                // a walk starts from the stride's first frame, at a slow first step
                a.stride = 0.0;
                a.speed = 0.35;
            }
            a.anim = Anim::Walk;
            // ease in, and slow down over the last stretch so the stop is not abrupt
            let mut left = 0.0;
            let mut at = (a.x, a.y);
            for t in &path {
                left += ((t.x as f32 - at.0).powi(2) + (t.y as f32 - at.1).powi(2)).sqrt();
                at = (t.x as f32, t.y as f32);
            }
            // smoothstep-like: gentle at both ends of the walk; across a big office, a brisker pace
            let top = WALK_SPEED * (1.0 + ((left - 10.0) / 25.0).clamp(0.0, 0.6));
            a.speed = (a.speed + ACCEL * dt * (1.0 - a.speed / top).max(0.25)).min(top).min((1.6 * ACCEL * left).sqrt().max(0.45));
            a.stride += walk_along(a, &mut path, a.speed * dt);
            if path.is_empty() {
                a.x = a.x.round();
                a.y = a.y.round();
                a.speed = 0.0;
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
                finish_standing(a, s, d);
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
                Until::Partner(gi) => {
                    let gi = *gi as usize;
                    let waited = s.elapsed - s.play_since[gi];
                    match s.play[gi][1] {
                        Some(p) => s.playing.contains(&p) || waited > 40.0,
                        None => waited > 16.0,
                    }
                }
            };
            if over {
                let d = *done;
                a.q.pop_front();
                finish_standing(a, s, d);
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
                        let (end, flights, serve) = (150.0 + s.rng.next() * 60.0, 3 + s.rng.below(8) as u32, s.rng.below(2));
                        if let Some(g) = s.game.as_mut() {
                            g.state = GameState::Playing;
                            g.end = s.elapsed + end;
                            g.score = [0, 0];
                            g.out = None;
                            g.point_at = -10.0;
                            g.next_rally(s.elapsed - PP_REST + 0.4, serve, flights);
                        }
                        a.q.push_back(Step::Wait { anim: Anim::PingPongBack, sit: None, face: Some(Face::UL), label: "Ping pong".into(), until: Until::GameOver, started: false, bubble: None, done: Done::EndGame });
                    }
                }
                Call::LeadBreak => lead_break(a, s),
                Call::Play(gi) => {
                    let g = gi as usize;
                    let kind = s.layout.lounge.games[g].kind;
                    if s.play[g][1].is_some() {
                        let dur = 22.0 + s.rng.next() * 16.0;
                        let (sit, face) = (a.seated, if a.seated.is_some() { None } else { Some(a.face) });
                        a.q.push_back(Step::Act { anim: Anim::Play(kind), sit, face, left: dur, started: false, bubble: None, label: kind.label().into(), tag: Tag::Game(gi), done: Done::Unplay });
                    } else {
                        // nobody came
                        release_play(a, s);
                        stand_up(a, s);
                    }
                }
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

/// How far ahead along the path a walker aims, in tiles: corners become short curves instead
/// of sharp turns on a tile's centre.
const LOOK: f32 = 0.6;

/// The point `dist` tiles along the path from where the avatar stands.
fn ahead(a: &Avatar, path: &[Tile], dist: f32) -> (f32, f32) {
    let (mut x, mut y, mut left) = (a.x, a.y, dist);
    for t in path {
        let (dx, dy) = (t.x as f32 - x, t.y as f32 - y);
        let d = (dx * dx + dy * dy).sqrt();
        if d >= left && d > 0.0 {
            return (x + dx / d * left, y + dy / d * left);
        }
        (x, y, left) = (t.x as f32, t.y as f32, left - d);
    }
    (x, y)
}

/// Moves up to `budget` tiles along the path, facing the way it goes, and says how far it went.
/// It steers at a point a little ahead, so it rounds corners, and ends exactly on the last tile.
fn walk_along(a: &mut Avatar, path: &mut Vec<Tile>, budget: f32) -> f32 {
    let mut left = budget;
    while left > 1e-5 {
        let Some(&n) = path.first() else { break };
        let (nx, ny) = (n.x as f32 - a.x, n.y as f32 - a.y);
        let near = (nx * nx + ny * ny).sqrt();
        if path.len() > 1 && near < LOOK * 0.5 {
            // close enough to this corner: the curve takes it from here
            path.remove(0);
            continue;
        }
        let (tx, ty) = if path.len() == 1 { (n.x as f32, n.y as f32) } else { ahead(a, path, LOOK) };
        let (dx, dy) = (tx - a.x, ty - a.y);
        let d = (dx * dx + dy * dy).sqrt();
        if d > 0.01 {
            a.walk_dir = dy.atan2(dx);
            a.face = if dx.abs() >= dy.abs() {
                if dx > 0.0 { Face::DR } else { Face::UL }
            } else if dy > 0.0 {
                Face::DL
            } else {
                Face::UR
            };
        }
        if path.len() == 1 && d <= left {
            a.x = n.x as f32;
            a.y = n.y as f32;
            path.clear();
            left -= d;
            break;
        }
        // small steps, so the curve stays a curve at any speed
        let step = left.min(0.05).min(d.max(1e-3));
        a.x += dx / d.max(1e-3) * step;
        a.y += dy / d.max(1e-3) * step;
        left -= step;
    }
    budget - left
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
            w.sync(&session, SystemTime::now());
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

    /// A team sits together and meets at the whiteboard when its phase is done; whoever is
    /// asked something stays put, and a quiet worker slumps.
    #[test]
    fn teams_sit_together_and_signals_show() {
        let key = SessionKey { harness: "t", id: "s".into() };
        let mut session = Session::new(key, "Test", SystemTime::now());
        let mut w = World::new("proj", 11);
        let lead = AgentRef::Lead;
        session.apply(&ev(lead.clone(), EventKind::UserPrompt { text: "go".into() }));
        session.apply(&ev(lead.clone(), EventKind::ToolCall { call_id: "1".into(), name: "Read".into(), summary: "a.rs".into() }));
        for i in 0..4 {
            session.apply(&ev(lead.clone(), EventKind::SubagentStart { agent_id: format!("a{i}"), description: format!("task {i}"), kind: None }));
            session.apply(&ev(AgentRef::Sub(format!("a{i}")), EventKind::Team { name: "wf · Build".into() }));
        }
        let mut huddled = 0;
        for frame in 0..(60 * 60) {
            if frame == 60 * 15 {
                // the team sits within a few tiles of each other
                let desks: Vec<(i32, i32)> = w.avatars.iter().filter(|a| a.team.is_some()).filter_map(|a| a.desk).map(|d| (w.s.layout.desks[d].x, w.s.layout.desks[d].y)).collect();
                assert_eq!(desks.len(), 4, "the whole team got desks");
                for a in &desks {
                    assert!(desks.iter().all(|b| (a.0 - b.0).abs() + (a.1 - b.1).abs() <= 6), "team desks are close: {desks:?}");
                }
                session.apply(&ev(lead.clone(), EventKind::NeedsInput { text: "Allow?".into() }));
                for i in 0..4 {
                    session.apply(&ev(lead.clone(), EventKind::SubagentEnd { agent_id: format!("a{i}") }));
                }
            }
            w.sync(&session, SystemTime::now());
            w.step(1.0 / 60.0);
            if frame > 60 * 15 {
                huddled = huddled.max(w.avatars.iter().filter(|a| a.team.is_some() && a.q.iter().any(|s| matches!(s, Step::Act { label, .. } if label.contains("whiteboard")))).count());
                if frame > 60 * 18 {
                    let l = w.avatars.iter().find(|a| a.lead).unwrap();
                    assert!(l.needs.is_some() && l.is_seated_at_desk(), "the lead waits at its desk with its hand up");
                }
            }
        }
        assert!(huddled >= 2, "the finished team met at the whiteboard ({huddled})");
        // nothing for longer than the limit: the lead is stuck
        session.apply(&ev(lead.clone(), EventKind::ToolCall { call_id: "2".into(), name: "Bash".into(), summary: "ls".into() }));
        w.stuck_after = Duration::from_secs(0);
        w.sync(&session, SystemTime::now() + Duration::from_secs(5));
        assert!(w.avatars.iter().find(|a| a.lead).unwrap().stuck);
    }

    /// More helpers than desks: nobody shares a tile or stands inside a workstation.
    #[test]
    fn everyone_gets_their_own_spot() {
        for seed in [3u32, 7, 11, 19] {
            let key = SessionKey { harness: "t", id: "s".into() };
            let mut session = Session::new(key, "Test", SystemTime::now());
            let mut w = World::new("proj", seed);
            let lead = AgentRef::Lead;
            session.apply(&ev(lead.clone(), EventKind::UserPrompt { text: "go".into() }));
            session.apply(&ev(lead.clone(), EventKind::ToolCall { call_id: "1".into(), name: "Read".into(), summary: "a.rs".into() }));
            let n = w.s.layout.desks.len() + 4;
            for i in 0..n {
                session.apply(&ev(lead.clone(), EventKind::SubagentStart { agent_id: format!("s{i}"), description: format!("task {i}"), kind: None }));
            }
            for frame in 0..(60 * 90) {
                if frame == 60 * 40 {
                    for i in 0..3 {
                        session.apply(&ev(lead.clone(), EventKind::SubagentEnd { agent_id: format!("s{i}") }));
                    }
                }
                w.sync(&session, SystemTime::now());
                w.step(1.0 / 60.0);
                if frame < 60 * 25 || frame % 30 != 0 {
                    continue;
                }
                let mut spots = HashSet::new();
                // someone with nothing queued is between steps and plans the next one this frame
                for a in w.avatars.iter().filter(|a| a.anim != Anim::Walk && a.seated.is_none() && a.alpha >= 1.0 && !a.leaving && !a.q.is_empty()) {
                    let t = tile(a.x.round() as i32, a.y.round() as i32);
                    assert!(spots.insert(t), "seed {seed} frame {frame}: two people on {t:?}");
                    let lz = &w.s.layout.lounge;
                    let allowed = [lz.pp_a, lz.pp_b, lz.bar, lz.bar_queue].contains(&t) || lz.games.iter().any(|g| g.spots.iter().any(|s| s.0 == t));
                    assert!(!w.s.keep_clear.contains(&t) || allowed, "seed {seed} frame {frame}: {} stands inside a workstation at {t:?}", a.name);
                }
            }
        }
    }

    #[test]
    fn colours_differ_inside_an_office() {
        let a = colour_for(1, &[]);
        assert_ne!(a, colour_for(1, &[a]));
    }
}

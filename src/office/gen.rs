//! Floor plans. Every project gets its own office from a seed: room size, theme, desk rows,
//! the lounge's arrangement and games, the partition gap and the decorations all come from it.

use super::{tile, Face, Rng, Tile};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub work: &'static str,
    pub lounge: &'static str,
    pub back: u32,
    pub left: u32,
    pub accent: u32,
    pub trim: u32,
}

pub const THEMES: [Theme; 6] = [
    Theme { name: "Warm loft", work: "oak", lounge: "tealrug", back: 0xefe2cc, left: 0xe2d2b8, accent: 0xb9583a, trim: 0x8a5a3c },
    Theme { name: "Night lab", work: "concrete", lounge: "checker", back: 0x3b465c, left: 0x323c51, accent: 0x2f7f78, trim: 0x1f2738 },
    Theme { name: "Garden studio", work: "walnut", lounge: "terracotta", back: 0xdfe6d3, left: 0xd0dac2, accent: 0x6c8f5a, trim: 0x5d4a36 },
    Theme { name: "Marble HQ", work: "marble", lounge: "carpet", back: 0xe8edf2, left: 0xdae2ea, accent: 0x3d5a8a, trim: 0x6b7a90 },
    Theme { name: "Retro den", work: "carpet", lounge: "checker", back: 0xf0d8b2, left: 0xe5c99d, accent: 0xc4443f, trim: 0x6d3b2a },
    Theme { name: "Brick works", work: "walnut", lounge: "tealrug", back: 0xc9a68e, left: 0xbf9a80, accent: 0x3e6d6a, trim: 0x4f3324 },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Atlas {
    Props,
    Cafe,
    /// the lounge games and the scoreboard
    Games,
    /// an empty workstation: the shared desk render
    Desk,
}

#[derive(Debug, Clone)]
pub struct Prop {
    pub name: &'static str,
    pub x: i32,
    pub y: i32,
    pub fx: i32,
    pub fy: i32,
    pub atlas: Atlas,
    pub flip: bool,
    /// draw order override; props normally sort by their front corner
    pub depth: Option<f32>,
}

impl Prop {
    pub fn depth(&self) -> f32 {
        self.depth.unwrap_or((self.x + self.fx - 1 + self.y + self.fy - 1) as f32 + 0.6)
    }
}

/// Something flat on the floor (a rug), drawn with the floor.
#[derive(Debug, Clone)]
pub struct Flat {
    pub name: &'static str,
    pub x: f32,
    pub y: f32,
    pub fx: f32,
    pub fy: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeatKind {
    Desk,
    Chair,
    Couch,
}

#[derive(Debug, Clone)]
pub struct Seat {
    pub kind: SeatKind,
    /// where the avatar sits
    pub x: i32,
    pub y: i32,
    /// the free tile it walks to before sitting down
    pub ax: i32,
    pub ay: i32,
    pub face: Face,
    pub table: Option<&'static str>,
    /// px the sprite is lowered by when seated (chairs and the couch are low)
    pub lift: f32,
    pub dx: f32,
    pub depth: Option<f32>,
}

impl Seat {
    pub fn access(&self) -> Tile {
        tile(self.ax, self.ay)
    }
    pub fn depth(&self) -> f32 {
        self.depth.unwrap_or((self.x + self.y) as f32 + 0.65)
    }
}

#[derive(Debug, Clone)]
pub struct Lounge {
    pub seats: Vec<Seat>,
    pub pp_a: Tile,
    pub pp_b: Tile,
    pub pp_table: usize,
    pub bar: Tile,
    pub bar_queue: Tile,
    /// the ping pong scoreboard prop
    pub scoreboard: usize,
    pub games: Vec<GameSpot>,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub seed: u32,
    pub w: i32,
    pub h: i32,
    /// x of the glass partition between the desks and the lounge
    pub px: i32,
    /// first lounge column
    pub l0: i32,
    pub theme: Theme,
    pub blocked: Vec<bool>,
    pub props: Vec<Prop>,
    pub flats: Vec<Flat>,
    /// rows where the partition has a glass pane
    pub glass: Vec<i32>,
    pub door: Tile,
    pub desks: Vec<Seat>,
    pub lounge: Lounge,
    /// whiteboard span on the back wall, in tiles
    pub bx0: i32,
    pub bx1: i32,
    pub windows: Vec<f32>,
    pub gap_y: i32,
    pub sky: [f32; 24],
}

impl Layout {
    pub fn blocked(&self, x: i32, y: i32) -> bool {
        x < 0 || y < 0 || x >= self.w || y >= self.h || self.blocked[(y * self.w + x) as usize]
    }

    /// Tiles reachable from `from` without crossing furniture.
    pub fn flood(&self, from: Tile) -> HashSet<Tile> {
        let mut seen = HashSet::from([from]);
        let mut q = VecDeque::from([from]);
        while let Some(c) = q.pop_front() {
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let n = tile(c.x + dx, c.y + dy);
                if !self.blocked(n.x, n.y) && seen.insert(n) {
                    q.push_back(n);
                }
            }
        }
        seen
    }

    /// Shortest path in eight directions, excluding the start tile. A diagonal step needs both
    /// tiles beside it free, so nobody cuts a corner through furniture. The goal may itself be
    /// blocked (a seat); it is entered straight.
    pub fn path(&self, from: (f32, f32), to: Tile) -> Option<Vec<Tile>> {
        let (w, h) = (self.w, self.h);
        let sx = (from.0.round() as i32).clamp(0, w - 1);
        let sy = (from.1.round() as i32).clamp(0, h - 1);
        let idx = |x: i32, y: i32| (y * w + x) as usize;
        let (start, goal) = (idx(sx, sy), idx(to.x.clamp(0, w - 1), to.y.clamp(0, h - 1)));
        let open = |x: i32, y: i32| !self.blocked(x, y);
        let mut dist = vec![u32::MAX; (w * h) as usize];
        let mut prev = vec![usize::MAX; (w * h) as usize];
        dist[start] = 0;
        prev[start] = start;
        // costs in tenths: 10 straight, 14 diagonal
        let mut heap = BinaryHeap::from([Reverse((0u32, start))]);
        while let Some(Reverse((d, c))) = heap.pop() {
            if c == goal {
                break;
            }
            if d > dist[c] {
                continue;
            }
            let (cx, cy) = ((c as i32) % w, (c as i32) / w);
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                let (x, y) = (cx + dx, cy + dy);
                if x < 0 || y < 0 || x >= w || y >= h {
                    continue;
                }
                let k = idx(x, y);
                let diagonal = dx != 0 && dy != 0;
                if self.blocked[k] && (k != goal || diagonal) {
                    continue;
                }
                if diagonal && !(open(cx + dx, cy) && open(cx, cy + dy)) {
                    continue;
                }
                let nd = d + if diagonal { 14 } else { 10 };
                if nd < dist[k] {
                    dist[k] = nd;
                    prev[k] = c;
                    heap.push(Reverse((nd, k)));
                }
            }
        }
        if prev[goal] == usize::MAX {
            return None;
        }
        let mut path = Vec::new();
        let mut c = goal;
        while c != start {
            path.push(tile((c as i32) % w, (c as i32) / w));
            c = prev[c];
        }
        path.reverse();
        Some(path)
    }
}

/// The games a lounge can have. Each office gets two to four of them, by seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Play {
    Foosball,
    Pinball,
    AirHockey,
    Chess,
    Console,
    Vr,
    Claw,
    BoardGame,
    Guitar,
    Hoops,
    Pool,
}

impl Play {
    pub const ALL: [Play; 11] = [Play::Foosball, Play::Pinball, Play::AirHockey, Play::Chess, Play::Console, Play::Vr, Play::Claw, Play::BoardGame, Play::Guitar, Play::Hoops, Play::Pool];

    /// What the HUD says while someone plays.
    pub fn label(self) -> &'static str {
        match self {
            Play::Foosball => "Playing foosball",
            Play::Pinball => "Playing pinball",
            Play::AirHockey => "Playing air hockey",
            Play::Chess => "Playing chess",
            Play::Console => "Gaming on the console",
            Play::Vr => "In VR",
            Play::Claw => "Trying the claw machine",
            Play::BoardGame => "Playing a board game",
            Play::Guitar => "Playing the guitar",
            Play::Hoops => "Shooting hoops",
            Play::Pool => "Playing pool",
        }
    }

    /// Games that wait for a second player before starting.
    pub fn pair(self) -> bool {
        matches!(self, Play::Foosball | Play::AirHockey | Play::Chess | Play::BoardGame)
    }
}

/// A game in the lounge: its prop, where its players stand (tile and facing), and for the games
/// played sitting down, the lounge seats they use instead.
#[derive(Debug, Clone)]
pub struct GameSpot {
    pub kind: Play,
    pub prop: usize,
    pub spots: Vec<(Tile, Face)>,
    pub seats: Vec<usize>,
}

/// How a game sits in the room: its prop and footprint, whether its back must touch the back
/// wall, where players stand (relative to the prop, with the way they face) and for seated
/// games the chairs or beanbag around it.
struct GameDef {
    kind: Play,
    prop: &'static str,
    fx: i32,
    fy: i32,
    flip: bool,
    back_wall: bool,
    spots: &'static [(i32, i32, Face)],
    seats: &'static [Chair],
}

/// A seat that comes with a game: the prop drawn under it, where it is, where you get on, which way you face.
struct Chair {
    prop: &'static str,
    atlas: Atlas,
    at: (i32, i32),
    access: (i32, i32),
    face: Face,
    lift: f32,
}

const CAFE_L: Chair = Chair { prop: "chairL", atlas: Atlas::Cafe, at: (0, -1), access: (1, -1), face: Face::DL, lift: 4.0 };
const CAFE_R: Chair = Chair { prop: "chairR", atlas: Atlas::Cafe, at: (-1, 0), access: (-2, 0), face: Face::DR, lift: 4.0 };

const GAMES: [GameDef; 11] = [
    // the table art runs along y, so it is mirrored onto its 2x1 footprint like the ping pong table
    GameDef { kind: Play::Foosball, prop: "foosball", fx: 2, fy: 1, flip: true, back_wall: false, spots: &[(0, -1, Face::DL), (1, 1, Face::UR)], seats: &[] },
    GameDef { kind: Play::Pinball, prop: "pinball", fx: 1, fy: 1, flip: false, back_wall: true, spots: &[(0, 1, Face::UR)], seats: &[] },
    GameDef { kind: Play::AirHockey, prop: "airhockey", fx: 2, fy: 1, flip: true, back_wall: false, spots: &[(-1, 0, Face::DR), (2, 0, Face::UL)], seats: &[] },
    GameDef { kind: Play::Chess, prop: "chess", fx: 1, fy: 1, flip: false, back_wall: false, spots: &[], seats: &[CAFE_L, CAFE_R] },
    // played standing a step back from the screen, facing it
    GameDef { kind: Play::Console, prop: "tv", fx: 1, fy: 1, flip: false, back_wall: true, spots: &[(0, 2, Face::UR)], seats: &[] },
    // the mat lies on the floor and the player stands on it
    GameDef { kind: Play::Vr, prop: "vr", fx: 1, fy: 1, flip: false, back_wall: false, spots: &[(0, 0, Face::DR)], seats: &[] },
    GameDef { kind: Play::Claw, prop: "claw", fx: 1, fy: 1, flip: false, back_wall: true, spots: &[(0, 1, Face::UR)], seats: &[] },
    GameDef { kind: Play::BoardGame, prop: "boardgame", fx: 1, fy: 1, flip: false, back_wall: false, spots: &[], seats: &[CAFE_L, CAFE_R] },
    GameDef { kind: Play::Guitar, prop: "amp", fx: 1, fy: 1, flip: false, back_wall: false, spots: &[(1, 0, Face::DR)], seats: &[] },
    GameDef { kind: Play::Hoops, prop: "hoops", fx: 2, fy: 1, flip: false, back_wall: false, spots: &[(2, 0, Face::UL)], seats: &[] },
    GameDef { kind: Play::Pool, prop: "pool", fx: 2, fy: 1, flip: false, back_wall: false, spots: &[(-1, 0, Face::DR)], seats: &[] },
];

/// Builds the office for a seed: the desks on the left, a glass partition, and a lounge of six
/// columns laid out in zones (the coffee bar along the back wall, ping pong with its
/// scoreboard, a cafe table, a sofa on its rug, and the games), with a clear path to everything.
pub fn generate(seed: u32) -> Layout {
    build(seed, 0, 0)
}

/// The most people one office seats; a bigger crew gathers at the whiteboard.
pub const MAX_CREW: usize = 512;

/// The office for a crew of `crew` at once: the seed's own room while they fit at its desks, else
/// the same room grown wider and deeper (rows and columns of desks added, the lounge running
/// along) until they do.
pub fn generate_for(seed: u32, crew: usize) -> Layout {
    let base = build(seed, 0, 0);
    let want = crew.min(MAX_CREW);
    if base.desks.len() >= want {
        return base;
    }
    let (mut gw, mut gh) = (0, 0);
    loop {
        // keep the desk floor about as deep as it is wide, as far as either side may go
        let (wide, deep) = (base.w + gw + 2 <= MAX_SIDE, base.h + gh + 3 <= MAX_SIDE);
        match (wide, deep) {
            (false, false) => return build(seed, gw, gh),
            (true, false) => gw += 2,
            (false, true) => gh += 3,
            _ if base.w + gw - 9 <= base.h + gh => gw += 2,
            _ => gh += 3,
        }
        // about one desk in six tiles of desk floor: only build rooms that can be big enough
        let (dw, dh) = (base.px + gw - 3, base.h + gh - 4);
        if ((dw / 2) * (dh / 3)) as usize >= want {
            let o = build(seed, gw, gh);
            if o.desks.len() >= want {
                return o;
            }
        }
    }
}

/// The longest side a grown room gets, in tiles.
const MAX_SIDE: i32 = 80;

fn build(seed: u32, gw: i32, gh: i32) -> Layout {
    let mut r = Rng::new(seed);
    let w = 15 + (r.next() * 4.0) as i32 + gw;
    let h = 10 + (r.next() * 3.0) as i32 + gh;
    let px = w - 7;
    let l0 = px + 1;
    let theme = *r.pick(&THEMES);
    let mut b = Builder { w, h, blocked: vec![false; (w * h) as usize], props: Vec::new(), reserved: HashSet::new(), aisle: HashSet::new(), keep: HashSet::new() };

    // lounge plan first so the partition gap can open onto a free tile
    // ping pong at the back, the cafe table in the middle, the sofa at the front, with a row of
    // floor between each so the tables never touch
    let (pp_y, cafe_y) = (3, 6);
    let gaps: Vec<i32> = [1, 4, h - 4]
        .into_iter()
        .filter(|&y| y != pp_y && y + 1 != pp_y && y != cafe_y - 1 && y != cafe_y && y + 1 < h - 3)
        .collect();
    let gap_y = *r.pick(&gaps);
    let mut glass = Vec::new();
    for y in 0..h {
        if y != gap_y && y != gap_y + 1 {
            b.block(px, y);
            glass.push(y);
        }
    }
    for y in gap_y..=gap_y + 1 {
        b.reserve(px, y);
        b.reserve(px - 1, y);
        b.reserve(l0, y);
        b.reserve(l0 + 1, y);
    }
    let door = tile(0, h - 2);
    for (x, y) in [(0, h - 2), (1, h - 2), (0, h - 3), (1, h - 3), (2, h - 2)] {
        b.reserve(x, y);
    }
    // archive shelves on the left wall
    let shelf_start = *r.pick(&[2, 3, 4]);
    let shelf_n = 1 + (r.next() * 2.0) as i32;
    for i in 0..shelf_n {
        let y = shelf_start + i;
        b.place("bookshelf", 0, y, 1, 1, Atlas::Props);
        b.reserve(1, y);
    }
    let bx0 = 2;
    let bx1 = (px - 1).min(bx0 + 6);
    // desks: the avatar sits on the seat tile and walks to the tile behind the chair (y - 1)
    let mut desks = Vec::new();
    let start_x = 2 + if r.next() < 0.5 { 0 } else { 1 };
    let mut y = 3;
    while y <= h - 2 {
        let mut x = start_x;
        while x <= px - 2 {
            let skip = desks.len() > 5 && r.next() < 0.18;
            if !skip && b.ok(x, y) && b.free(x, y - 1, 1, 1) {
                b.place("ws", x, y, 1, 1, Atlas::Desk);
                desks.push(Seat { kind: SeatKind::Desk, x, y, ax: x, ay: y - 1, face: Face::DL, table: None, lift: 12.0, dx: 0.0, depth: None });
                b.reserve(x, y - 1);
                b.reserve(x - 1, y);
            }
            x += 2;
        }
        y += 3;
    }
    let mut windows = Vec::new();
    let mut y = 1;
    while y < h - 3 {
        if (y + 2 <= shelf_start || y >= shelf_start + shelf_n) && r.next() < 0.8 {
            windows.push(y as f32 + 0.2);
        }
        y += 3;
    }
    // the desk side keeps its decor to the corners and the walls between windows
    if b.free(px - 1, 0, 1, 1) && px - 1 > bx1 {
        b.place("server", px - 1, 0, 1, 1, Atlas::Props);
    } else if b.ok(px - 1, h - 1) {
        b.place("server", px - 1, h - 1, 1, 1, Atlas::Props);
    }
    if b.ok(0, 0) {
        b.place("plant", 0, 0, 1, 1, Atlas::Props);
    }
    if b.ok(1, 0) {
        let n = *r.pick(&["plant", "cooler"]);
        b.place(n, 1, 0, 1, 1, Atlas::Props);
    }
    if b.ok(px - 1, h - 1) {
        b.place("plant", px - 1, h - 1, 1, 1, Atlas::Props);
    }
    // a plant beside the first window that has room under it, never in front of one
    let in_window = |y: i32| windows.iter().any(|&wy| (y as f32) >= wy - 0.2 && (y as f32) <= wy + 1.8);
    if let Some(y) = windows.iter().map(|&wy| wy.round() as i32 + 2).find(|&y| !in_window(y) && y < h - 3 && b.ok(0, y)) {
        b.place("plant", 0, y, 1, 1, Atlas::Props);
    }

    // the coffee bar: lamp, machine and fridge side by side on the back wall
    let lamp = *r.pick(&["lamp", "plant"]);
    b.place(lamp, l0, 0, 1, 1, Atlas::Props);
    b.place("coffee", l0 + 1, 0, 1, 1, Atlas::Props);
    b.place("fridge", l0 + 2, 0, 1, 1, Atlas::Props);
    let (bar, bar_queue) = (tile(l0 + 1, 1), tile(l0 + 1, 2));
    b.reserve(bar.x, bar.y);
    b.reserve(bar_queue.x, bar_queue.y);
    b.reserve(l0 + 2, 1);
    // ping pong, with the scoreboard against the right wall beside the far player
    let pp_table = b.place("pingpong", l0 + 2, pp_y, 2, 1, Atlas::Props);
    b.props[pp_table].flip = true;
    let (pp_a, pp_b) = (tile(l0 + 1, pp_y), tile(l0 + 4, pp_y));
    b.reserve(pp_a.x, pp_a.y);
    b.reserve(pp_b.x, pp_b.y);
    b.keep.extend([(pp_a.x, pp_a.y), (pp_b.x, pp_b.y)]);
    let scoreboard = b.place("scoreboard", l0 + 5, pp_y, 1, 1, Atlas::Games);
    // a cafe table with two chairs
    let cx = l0 + 4;
    b.place("table", cx, cafe_y, 1, 1, Atlas::Cafe);
    b.place("chairL", cx, cafe_y - 1, 1, 1, Atlas::Cafe);
    b.place("chairR", cx - 1, cafe_y, 1, 1, Atlas::Cafe);
    let mut seats = vec![
        Seat { kind: SeatKind::Chair, x: cx, y: cafe_y - 1, ax: cx + 1, ay: cafe_y - 1, face: Face::DL, table: Some("cafe"), lift: 4.0, dx: 0.0, depth: None },
        Seat { kind: SeatKind::Chair, x: cx - 1, y: cafe_y, ax: cx - 2, ay: cafe_y, face: Face::DR, table: Some("cafe"), lift: 4.0, dx: 0.0, depth: None },
    ];
    // a grown room's long lounge gets more cafe tables down the middle, two rows of floor
    // between each, and stops short of the sofa
    if gh > 0 {
        let mut y = cafe_y + 5;
        while y + 6 <= h - 3 {
            b.place("table", cx, y, 1, 1, Atlas::Cafe);
            b.place("chairL", cx, y - 1, 1, 1, Atlas::Cafe);
            b.place("chairR", cx - 1, y, 1, 1, Atlas::Cafe);
            seats.push(Seat { kind: SeatKind::Chair, x: cx, y: y - 1, ax: cx + 1, ay: y - 1, face: Face::DL, table: Some("cafe"), lift: 4.0, dx: 0.0, depth: None });
            seats.push(Seat { kind: SeatKind::Chair, x: cx - 1, y, ax: cx - 2, ay: y, face: Face::DR, table: Some("cafe"), lift: 4.0, dx: 0.0, depth: None });
            y += 5;
        }
    }
    for s in &seats {
        b.reserve(s.ax, s.ay);
    }
    // the sofa against the partition, on its rug; the art runs along y and faces down-right
    let sofa = b.place("sofa", l0, h - 3, 1, 2, Atlas::Props);
    let sb = (l0 + h - 3) as f32;
    b.props[sofa].depth = Some(sb + 0.9);
    for i in 0..2 {
        seats.push(Seat { kind: SeatKind::Couch, x: l0, y: h - 3 + i, ax: l0 + 1, ay: h - 3 + i, face: Face::DR, table: Some("couch"), lift: 10.0, dx: 2.0, depth: Some(sb + 0.93 + i as f32 * 0.02) });
        b.reserve(l0 + 1, h - 3 + i);
    }
    let flats = vec![Flat { name: "rug", x: l0 as f32 + 0.7, y: h as f32 - 3.3, fx: 2.4, fy: 2.2 }];
    if b.ok(l0, h - 1) {
        b.place("plant", l0, h - 1, 1, 1, Atlas::Props);
    }
    // nothing stands on the rug or between it and the sofa
    for y in h - 4..h {
        for x in l0 + 1..=l0 + 3 {
            b.aisle.insert((x, y));
        }
    }
    // walking room: the row under the bar stays free of furniture (people may stand in it)
    for x in l0..w {
        b.aisle.insert((x, 1));
    }

    // the games besides ping pong: one or two, so each office has its own few and room to play
    // them; they go against a wall where they can, never out in the middle of the floor
    let mut order: Vec<usize> = (0..GAMES.len()).collect();
    for i in (1..order.len()).rev() {
        order.swap(i, r.below(i + 1));
    }
    let want = 1 + r.below(2);
    let mut targets: Vec<Tile> = desks.iter().chain(&seats).map(Seat::access).collect();
    targets.extend([pp_a, pp_b, bar, bar_queue]);
    targets.extend((0..shelf_n).map(|i| tile(1, shelf_start + i)));
    let mut games = Vec::new();
    let mut cells: Vec<(i32, i32)> = (0..h).flat_map(|y| (l0..w).map(move |x| (x, y))).collect();
    for gi in order {
        if games.len() >= want {
            break;
        }
        let g = &GAMES[gi];
        for i in (1..cells.len()).rev() {
            cells.swap(i, r.below(i + 1));
        }
        // a table two tiles long may also stand turned the other way
        let turns: &[bool] = if g.fx != g.fy { &[false, true] } else { &[false] };
        let mut tries: Vec<(i32, i32, bool)> = cells.iter().flat_map(|&(x, y)| turns.iter().map(move |&t| (x, y, t))).collect();
        tries.sort_by_key(|&(x, y, t)| {
            let (fx, fy) = if t { (g.fy, g.fx) } else { (g.fx, g.fy) };
            !(y == 0 || x == l0 || x + fx == w || y + fy == h)
        });
        if let Some((nb, prop, spots, new_seats)) = tries.iter().find_map(|&(x, y, t)| try_game(&b, g, x, y, t, l0, door, &targets)) {
            b = nb;
            targets.extend(spots.iter().map(|s| s.0));
            targets.extend(new_seats.iter().map(Seat::access));
            let first = seats.len();
            seats.extend(new_seats);
            games.push(GameSpot { kind: g.kind, prop, spots, seats: (first..seats.len()).collect() });
        }
    }
    // corner plants where nothing else went, and nothing stands right beside
    for (x, y) in [(w - 1, 0), (w - 1, h - 1)] {
        let alone = [(x - 1, y), (x, y + 1), (x, y - 1)].iter().all(|&(nx, ny)| !b.inb(nx, ny) || !b.blocked[(ny * w + nx) as usize]);
        if b.ok(x, y) && alone {
            b.place("plant", x, y, 1, 1, Atlas::Props);
        }
    }

    let mut sky = [0.0; 24];
    for s in &mut sky {
        *s = r.next();
    }

    let lounge = Lounge { seats, pp_a, pp_b, pp_table, bar, bar_queue, scoreboard, games };
    let mut o = Layout { seed, w, h, px, l0, theme, blocked: b.blocked, props: b.props, flats, glass, door, desks, lounge, bx0, bx1, windows, gap_y, sky };

    // every place an avatar has to reach must be reachable from the door; decorations make way
    for _ in 0..8 {
        let reach = o.flood(o.door);
        if targets.iter().all(|t| reach.contains(t)) {
            break;
        }
        let Some(victim) = o.props.iter().position(|p| ["plant", "lamp", "cooler", "server"].contains(&p.name)) else { break };
        let v = o.props.remove(victim);
        o.blocked[(v.y * o.w + v.x) as usize] = false;
        let lz = &mut o.lounge;
        for i in std::iter::once(&mut lz.pp_table).chain(std::iter::once(&mut lz.scoreboard)).chain(lz.games.iter_mut().map(|g| &mut g.prop)) {
            if victim < *i {
                *i -= 1;
            }
        }
    }
    o
}

/// The builder with game `g` placed at (x, y), its prop's index, its players' spots and its
/// seats, or None when it does not fit there or would cut a place off from the door.
type Placed = (Builder, usize, Vec<(Tile, Face)>, Vec<Seat>);

/// Turned, its footprint, flip and players' spots swap x for y.
#[allow(clippy::too_many_arguments)]
fn try_game(b: &Builder, g: &GameDef, x: i32, y: i32, turned: bool, l0: i32, door: Tile, targets: &[Tile]) -> Option<Placed> {
    let inside = |cx: i32, cy: i32| cx >= l0 && cx < b.w && cy >= 0 && cy < b.h;
    if g.back_wall != (y == 0) {
        return None;
    }
    let (fx, fy) = if turned { (g.fy, g.fx) } else { (g.fx, g.fy) };
    let foot: Vec<(i32, i32)> = (0..fy).flat_map(|j| (0..fx).map(move |i| (x + i, y + j))).collect();
    if !foot.iter().all(|&(cx, cy)| inside(cx, cy) && b.ok(cx, cy)) {
        return None;
    }
    // a ring of floor all round it and its chairs, corners too, as the art overhangs its tiles;
    // a big one is drawn tall enough to cover what stands just behind it (and to be covered by
    // what stands just in front), so it keeps two tiles clear both ways; machines on the back
    // wall may stand side by side
    let mut group: Vec<((i32, i32), i32)> = foot.iter().map(|&t| (t, if fx * fy > 1 { 2 } else { 1 })).collect();
    group.extend(g.seats.iter().map(|c| ((x + c.at.0, y + c.at.1), 1)));
    let roomy = group.iter().all(|&((cx, cy), r)| {
        (-r..=r).all(|dy| {
            (-r..=r).all(|dx| {
                let (nx, ny) = (cx + dx, cy + dy);
                group.iter().any(|&(t, _)| t == (nx, ny)) || !inside(nx, ny) || (g.back_wall && ny == 0) || !(b.furniture(nx, ny) || b.keep.contains(&(nx, ny)))
            })
        })
    });
    if !roomy {
        return None;
    }
    let spots: Vec<(Tile, Face)> = g
        .spots
        .iter()
        .map(|&(dx, dy, f)| {
            if !turned {
                return (tile(x + dx, y + dy), f);
            }
            let f = match f {
                Face::DR => Face::DL,
                Face::DL => Face::DR,
                Face::UR => Face::UL,
                Face::UL => Face::UR,
            };
            (tile(x + dy, y + dx), f)
        })
        .collect();
    // the VR player stands on the mat itself
    let on_prop = g.kind == Play::Vr;
    if !spots.iter().all(|(t, _)| inside(t.x, t.y) && (on_prop || b.standable(t.x, t.y))) {
        return None;
    }
    // a chair is got onto from beside it, or else from behind it (away from the table)
    let reach = |ax: i32, ay: i32| inside(ax, ay) && b.standable(ax, ay);
    let mut chairs: Vec<(i32, i32, i32, i32)> = Vec::new();
    for c in g.seats {
        let (sx, sy) = (x + c.at.0, y + c.at.1);
        if !(inside(sx, sy) && b.ok(sx, sy)) {
            return None;
        }
        let (ax, ay) = [(x + c.access.0, y + c.access.1), (x + 2 * c.at.0, y + 2 * c.at.1)].into_iter().find(|&(ax, ay)| reach(ax, ay))?;
        chairs.push((sx, sy, ax, ay));
    }
    let mut nb = b.clone();
    let prop = nb.place(g.prop, x, y, fx, fy, Atlas::Games);
    nb.props[prop].flip = g.flip != turned;
    if on_prop {
        // drawn under whoever stands on it
        nb.blocked[(y * nb.w + x) as usize] = false;
        nb.props[prop].depth = Some((x + y) as f32 - 0.4);
        nb.reserve(x, y);
    }
    let table = if g.kind == Play::Chess { "chess" } else { "boardgame" };
    let mut seats = Vec::new();
    for (c, &(sx, sy, ax, ay)) in g.seats.iter().zip(&chairs) {
        nb.place(c.prop, sx, sy, 1, 1, c.atlas);
        nb.reserve(ax, ay);
        seats.push(Seat { kind: SeatKind::Chair, x: sx, y: sy, ax, ay, face: c.face, table: Some(table), lift: c.lift, dx: 0.0, depth: None });
    }
    for (t, _) in &spots {
        nb.reserve(t.x, t.y);
    }
    // nothing may be cut off from the door
    let reach = flood(&nb, door);
    let mut need: Vec<Tile> = targets.to_vec();
    need.extend(spots.iter().map(|s| s.0));
    need.extend(seats.iter().map(Seat::access));
    if !need.iter().all(|t| reach.contains(t)) {
        return None;
    }
    Some((nb, prop, spots, seats))
}

fn flood(b: &Builder, from: Tile) -> HashSet<Tile> {
    let mut seen = HashSet::from([from]);
    let mut q = VecDeque::from([from]);
    while let Some(c) = q.pop_front() {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let n = tile(c.x + dx, c.y + dy);
            if b.inb(n.x, n.y) && !b.blocked[(n.y * b.w + n.x) as usize] && seen.insert(n) {
                q.push_back(n);
            }
        }
    }
    seen
}

#[derive(Clone)]
struct Builder {
    w: i32,
    h: i32,
    blocked: Vec<bool>,
    props: Vec<Prop>,
    reserved: HashSet<(i32, i32)>,
    /// floor kept free of furniture that people may still stand on
    aisle: HashSet<(i32, i32)>,
    /// floor the games keep their distance from, like furniture (where the ping pong players stand)
    keep: HashSet<(i32, i32)>,
}

impl Builder {
    fn inb(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < self.w && y < self.h
    }
    fn block(&mut self, x: i32, y: i32) {
        let w = self.w;
        self.blocked[(y * w + x) as usize] = true;
    }
    fn free(&self, x: i32, y: i32, fx: i32, fy: i32) -> bool {
        (0..fy).all(|j| (0..fx).all(|i| self.inb(x + i, y + j) && !self.blocked[((y + j) * self.w + x + i) as usize]))
    }
    /// A prop stands on this tile (the VR mat too, which people may step on).
    fn furniture(&self, x: i32, y: i32) -> bool {
        self.props.iter().any(|p| x >= p.x && x < p.x + p.fx && y >= p.y && y < p.y + p.fy)
    }
    fn reserve(&mut self, x: i32, y: i32) {
        self.reserved.insert((x, y));
    }
    fn ok(&self, x: i32, y: i32) -> bool {
        self.standable(x, y) && !self.aisle.contains(&(x, y))
    }
    fn standable(&self, x: i32, y: i32) -> bool {
        self.free(x, y, 1, 1) && !self.reserved.contains(&(x, y))
    }
    fn place(&mut self, name: &'static str, x: i32, y: i32, fx: i32, fy: i32, atlas: Atlas) -> usize {
        self.props.push(Prop { name, x, y, fx, fy, atlas, flip: false, depth: None });
        for j in 0..fy {
            for i in 0..fx {
                self.block(x + i, y + j);
            }
        }
        self.props.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_seed_gives_a_walkable_office() {
        for seed in 0..300u32 {
            let o = generate(seed.wrapping_mul(2654435761));
            let reach = o.flood(o.door);
            assert!(!o.desks.is_empty(), "seed {seed} has desks");
            for s in o.desks.iter().chain(&o.lounge.seats) {
                assert!(reach.contains(&s.access()), "seed {seed}: seat access unreachable");
            }
            assert!(reach.contains(&o.lounge.pp_a) && reach.contains(&o.lounge.pp_b) && reach.contains(&o.lounge.bar));
            assert!(o.path((o.door.x as f32, o.door.y as f32), o.lounge.bar).is_some());
            let lz = &o.lounge;
            assert!((1..=2).contains(&lz.games.len()), "seed {seed}: {} games", lz.games.len());
            assert_eq!(o.props[lz.scoreboard].name, "scoreboard");
            for g in &lz.games {
                assert_eq!(o.props[g.prop].atlas, Atlas::Games, "seed {seed}: {:?} points at its prop", g.kind);
                assert!(g.spots.iter().all(|s| reach.contains(&s.0)), "seed {seed}: {:?} can be reached", g.kind);
            }
        }
        // across offices every game turns up
        let mut seen = HashSet::new();
        for seed in 0..300u32 {
            seen.extend(generate(seed.wrapping_mul(2654435761)).lounge.games.iter().map(|g| g.kind));
        }
        assert_eq!(seen.len(), Play::ALL.len(), "{seen:?}");
    }

    #[test]
    fn a_grown_room_seats_its_crew_and_every_seat_can_be_reached() {
        for seed in 0..12u32 {
            let seed = seed.wrapping_mul(2654435761);
            let base = generate(seed);
            assert_eq!(generate_for(seed, base.desks.len()).w, base.w, "a crew that fits keeps the seed's room");
            for crew in [24, 96, 320, MAX_CREW] {
                let o = generate_for(seed, crew);
                assert!(o.desks.len() >= crew, "seed {seed}: {} desks for {crew}", o.desks.len());
                assert_eq!(o.theme.name, base.theme.name);
                let reach = o.flood(o.door);
                for s in o.desks.iter().chain(&o.lounge.seats) {
                    assert!(reach.contains(&s.access()), "seed {seed}, crew {crew}: seat access unreachable");
                }
                assert!((1..=2).contains(&o.lounge.games.len()));
            }
        }
    }

    #[test]
    fn same_seed_same_room() {
        let a = generate(42);
        let b = generate(42);
        assert_eq!((a.w, a.h, a.desks.len(), a.gap_y, a.theme.name), (b.w, b.h, b.desks.len(), b.gap_y, b.theme.name));
    }
}

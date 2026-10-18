//! Floor plans. Every project gets its own office from a seed: room size, theme, desk rows,
//! where the lounge sits, the partition gap and the wall decorations all come from it.

use super::{tile, Face, Rng, Tile};
use std::collections::{HashSet, VecDeque};

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

    /// Shortest 4-way path, excluding the start tile. The goal may itself be blocked (a seat).
    pub fn path(&self, from: (f32, f32), to: Tile) -> Option<Vec<Tile>> {
        let (w, h) = (self.w, self.h);
        let sx = (from.0.round() as i32).clamp(0, w - 1);
        let sy = (from.1.round() as i32).clamp(0, h - 1);
        let idx = |x: i32, y: i32| (y * w + x) as usize;
        let (start, goal) = (idx(sx, sy), idx(to.x.clamp(0, w - 1), to.y.clamp(0, h - 1)));
        let mut prev = vec![usize::MAX; (w * h) as usize];
        prev[start] = start;
        let mut q = VecDeque::from([start]);
        while let Some(c) = q.pop_front() {
            if c == goal {
                break;
            }
            let (cx, cy) = ((c as i32) % w, (c as i32) / w);
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (x, y) = (cx + dx, cy + dy);
                if x < 0 || y < 0 || x >= w || y >= h {
                    continue;
                }
                let k = idx(x, y);
                if prev[k] != usize::MAX || (self.blocked[k] && k != goal) {
                    continue;
                }
                prev[k] = c;
                q.push_back(k);
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

/// Builds the office for a seed. Mirrors `genOffice` in art/preview/office.js.
pub fn generate(seed: u32) -> Layout {
    let mut r = Rng::new(seed);
    let w = 13 + (r.next() * 3.0) as i32;
    let h = 10 + (r.next() * 2.0) as i32;
    let px = w - 5;
    let l0 = px + 1;
    let theme = *r.pick(&THEMES);
    let mut b = Builder { w, h, blocked: vec![false; (w * h) as usize], props: Vec::new(), reserved: HashSet::new() };

    // lounge plan first so the partition gap can open onto a free tile
    let flip_lounge = r.next() < 0.5;
    let (pp_y, cafe_y) = if flip_lounge { (6, 3) } else { (3, 6) };
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
    }
    let door = tile(0, h - 2);
    for (x, y) in [(0, h - 2), (1, h - 2), (0, h - 3), (1, h - 3)] {
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
    if b.free(px - 1, 0, 1, 1) && px - 1 > bx1 {
        b.place("server", px - 1, 0, 1, 1, Atlas::Props);
    } else if b.ok(px - 1, h - 1) {
        b.place("server", px - 1, h - 1, 1, 1, Atlas::Props);
    }
    if b.free(1, 0, 1, 1) {
        let n = *r.pick(&["plant", "cooler"]);
        b.place(n, 1, 0, 1, 1, Atlas::Props);
    }
    if b.free(0, 0, 1, 1) {
        b.place("plant", 0, 0, 1, 1, Atlas::Props);
    }
    for k in 0..3 {
        let x = 1 + (r.next() * (px - 2) as f32) as i32;
        let y = h - 1;
        if b.ok(x, y) && b.free(x - 1, y, 1, 1) && b.free(x + 1, y, 1, 1) {
            b.place(if k == 1 { "cooler" } else { "plant" }, x, y, 1, 1, Atlas::Props);
        }
    }

    // lounge: coffee bar on the back wall, ping pong, a cafe table with two chairs, a couch
    let lamp = *r.pick(&["lamp", "plant"]);
    b.place(lamp, l0, 0, 1, 1, Atlas::Props);
    b.place("coffee", l0 + 1, 0, 1, 1, Atlas::Props);
    b.place("fridge", l0 + 2, 0, 1, 1, Atlas::Props);
    if b.free(l0 + 3, 0, 1, 1) {
        b.place("arcade", l0 + 3, 0, 1, 1, Atlas::Props);
    }
    // the table art runs along the other diagonal, so it is mirrored to match its 2x1 footprint
    let pp_table = b.place("pingpong", l0 + 1, pp_y, 2, 1, Atlas::Props);
    b.props[pp_table].flip = true;
    let cx = l0 + 2;
    b.place("table", cx, cafe_y, 1, 1, Atlas::Cafe);
    b.place("chairL", cx, cafe_y - 1, 1, 1, Atlas::Cafe);
    b.place("chairR", cx - 1, cafe_y, 1, 1, Atlas::Cafe);
    let mut seats = vec![
        Seat { kind: SeatKind::Chair, x: cx, y: cafe_y - 1, ax: cx + 1, ay: cafe_y - 1, face: Face::DL, table: Some("cafe"), lift: 4.0, dx: 0.0, depth: None },
        Seat { kind: SeatKind::Chair, x: cx - 1, y: cafe_y, ax: cx - 2, ay: cafe_y, face: Face::DR, table: Some("cafe"), lift: 4.0, dx: 0.0, depth: None },
    ];
    // the sofa art runs along y and faces down-right, so its footprint is 1x2 and both seats face dr
    let sofa = b.place("sofa", l0, h - 3, 1, 2, Atlas::Props);
    let sb = (l0 + h - 3) as f32;
    b.props[sofa].depth = Some(sb + 0.9);
    for i in 0..2 {
        seats.push(Seat { kind: SeatKind::Couch, x: l0, y: h - 3 + i, ax: l0 + 1, ay: h - 3 + i, face: Face::DR, table: Some("couch"), lift: 10.0, dx: 2.0, depth: Some(sb + 0.93 + i as f32 * 0.02) });
    }
    let flats = vec![Flat { name: "rug", x: l0 as f32 + 0.7, y: h as f32 - 3.3, fx: 2.4, fy: 2.2 }];
    if b.free(l0 + 3, h - 2, 1, 1) {
        b.place("beanbag", l0 + 3, h - 2, 1, 1, Atlas::Props);
    }
    if b.free(l0 + 3, h - 1, 1, 1) {
        b.place("plant", l0 + 3, h - 1, 1, 1, Atlas::Props);
    }

    let mut windows = Vec::new();
    let mut y = 1;
    while y < h - 3 {
        if (y + 2 <= shelf_start || y >= shelf_start + shelf_n) && r.next() < 0.8 {
            windows.push(y as f32 + 0.2);
        }
        y += 3;
    }
    let mut sky = [0.0; 24];
    for s in &mut sky {
        *s = r.next();
    }

    let lounge = Lounge { seats, pp_a: tile(l0, pp_y), pp_b: tile(l0 + 3, pp_y), pp_table, bar: tile(l0 + 1, 1), bar_queue: tile(l0 + 1, 2) };
    let mut o = Layout { seed, w, h, px, l0, theme, blocked: b.blocked, props: b.props, flats, glass, door, desks, lounge, bx0, bx1, windows, gap_y, sky };

    // every place an avatar has to reach must be reachable from the door; decorations make way
    let mut targets: Vec<Tile> = o.desks.iter().map(Seat::access).collect();
    targets.extend(o.lounge.seats.iter().map(Seat::access));
    targets.extend([o.lounge.pp_a, o.lounge.pp_b, o.lounge.bar, o.lounge.bar_queue]);
    targets.extend((0..shelf_n).map(|i| tile(1, shelf_start + i)));
    for _ in 0..8 {
        let reach = o.flood(o.door);
        if targets.iter().all(|t| reach.contains(t)) {
            break;
        }
        let Some(victim) = o.props.iter().position(|p| ["plant", "lamp", "cooler", "beanbag", "server"].contains(&p.name)) else { break };
        let v = o.props.remove(victim);
        if victim < o.lounge.pp_table {
            o.lounge.pp_table -= 1;
        }
        o.blocked[(v.y * o.w + v.x) as usize] = false;
    }
    o
}

struct Builder {
    w: i32,
    h: i32,
    blocked: Vec<bool>,
    props: Vec<Prop>,
    reserved: HashSet<(i32, i32)>,
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
    fn reserve(&mut self, x: i32, y: i32) {
        self.reserved.insert((x, y));
    }
    fn ok(&self, x: i32, y: i32) -> bool {
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
        }
    }

    #[test]
    fn same_seed_same_room() {
        let a = generate(42);
        let b = generate(42);
        assert_eq!((a.w, a.h, a.desks.len(), a.gap_y, a.theme.name), (b.w, b.h, b.desks.len(), b.gap_y, b.theme.name));
    }
}

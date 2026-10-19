//! Turns one office into a depth-sorted list of quads, plus the anchors the text overlay and
//! the pointer need. Mirrors `render()` in the browser prototype.

use super::assets::{meta, sheet_key};
use super::paint::CUTAWAY;
use super::city::{self, City, Sky};
use super::view::{wall_height, View, Wall};
use super::{DrawList, Fx, Instance, Layer, TexKey};
use crate::office::gen::{Atlas, Layout, Play, Prop, SeatKind};
use crate::office::world::{Anim, Avatar, Ball, GameState, Pose, SeatRef, World, PP_HIT_A, PP_HIT_B, PP_PERIOD};
use crate::office::{Face, TH, TW};

/// Sprite scale and how far below the tile centre the feet sit, per sheet family.
fn sheet_scale(base: &str) -> (f32, f32) {
    match base {
        "type" | "read" => (0.54, 12.0),
        "sit" => (0.47, 0.0),
        _ => (0.47, 6.0),
    }
}

fn prop_height(name: &str) -> Option<f32> {
    Some(match name {
        "whiteboard" => 74.0,
        "plant" => 58.0,
        "coffee" => 54.0,
        "cooler" => 62.0,
        "fridge" => 54.0,
        "bookshelf" => 84.0,
        "server" => 82.0,
        "lamp" => 66.0,
        "beanbag" => 36.0,
        "arcade" => 78.0,
        "table" => 38.0,
        "chairL" | "chairR" => 50.0,
        "lowtable" => 30.0,
        "pinball" => 88.0,
        "claw" => 96.0,
        "hoops" => 104.0,
        "amp" => 52.0,
        "scoreboard" => 50.0,
        _ => return None,
    })
}

fn prop_width(name: &str) -> Option<f32> {
    match name {
        "pingpong" => Some(108.0),
        "sofa" => Some(100.0),
        "rug" => Some(124.0),
        "foosball" => Some(104.0),
        "airhockey" => Some(104.0),
        "pool" => Some(118.0),
        "chess" => Some(58.0),
        "boardgame" => Some(62.0),
        "tv" => Some(70.0),
        "vr" => Some(92.0),
        _ => None,
    }
}

/// The sheets made from video: 8 directions of walking and standing, and the games. They are
/// drawn at HiDPI resolution, so they scale down further than the hand-made sheets.
const VIDEO_SCALE: f32 = 0.324;
/// Tiles walked per walk cycle (two steps), so the feet keep up with the floor.
const STRIDE: f32 = 1.7;
/// The five filmed directions, front to back; the other three are these mirrored.
const DIRS: [&str; 5] = ["s", "se", "e", "ne", "n"];
/// Within this much of the angle between two filmed directions, both are shown, blended.
const BLEND: f32 = 0.22;

/// Which filmed directions show a room heading from this camera: up to two (index into DIRS,
/// weight), and whether the art is mirrored.
fn dir8(v: &View, heading: f32) -> ([(usize, f32); 2], bool) {
    let (right, toward) = v.ground(heading.cos(), heading.sin());
    let flip = right < 0.0;
    // 0 straight at you, 2 to the side, 4 straight away
    let pos = (std::f32::consts::FRAC_PI_2 - toward.atan2(right.abs())) / std::f32::consts::FRAC_PI_4;
    let lo = pos.floor().clamp(0.0, 4.0);
    let frac = pos - lo;
    let (i, j) = (lo as usize, (lo as usize + 1).min(4));
    let w = ((frac - 0.5 + BLEND) / (2.0 * BLEND)).clamp(0.0, 1.0);
    ([(i, 1.0 - w), (j, w)], flip)
}

/// One sprite of an avatar: which sheet and frame, mirrored or not, its scale, its feet in world
/// px, how opaque, and a vertical stretch for breathing.
struct Part {
    key: String,
    frame: u32,
    flip: bool,
    scale: f32,
    cx: f32,
    by: f32,
    alpha: f32,
    stretch: f32,
}

/// The sprites that draw a pose: usually one, two while it turns between filmed directions.
fn parts(p: &Pose, char_id: u8, w: &World, v: &View, anim_fps: f32) -> Vec<Part> {
    let m = meta();
    let seat = p.seated.map(|r| w.s.seat(r));
    let (px0, cy) = v.floor(p.x + 0.5, p.y + 0.5);
    let cx = px0 + seat.map_or(0.0, |s| if v.mirrored() { -s.dx } else { s.dx });
    let (toward, right) = v.facing(p.face);
    let mirrored = v.mirrored();
    let old = |base: &'static str, flip: bool, fixed: Option<u32>| {
        let key = sheet_key(base, char_id);
        let an = m.anims.get(&key)?;
        let (s, lift0) = sheet_scale(base);
        let lift = match seat {
            Some(st) if st.kind != SeatKind::Desk => st.lift,
            _ => lift0,
        };
        let frame = match fixed {
            Some(f) => f,
            None => {
                let game = w.s.game.as_ref().filter(|g| g.state == GameState::Playing && base.starts_with("pp"));
                match game {
                    Some(g) if g.swinging(w.s.elapsed) => {
                        let u = (w.s.elapsed - g.t0) / PP_PERIOD;
                        let shift = if base == "ppfront" { 0.0 } else { PP_HIT_B - PP_HIT_A - 0.5 };
                        (((u + shift).rem_euclid(1.0)) * an.frames as f32) as u32 % an.frames
                    }
                    Some(_) => 0,
                    None => ((p.t + p.phase) * anim_fps) as u32 % an.frames,
                }
            }
        };
        Some(Part { key, frame, flip, scale: s, cx, by: cy + lift, alpha: 1.0, stretch: 1.0 })
    };
    // a sheet filmed in 8 directions, or None when this character has none
    let eight = |base: &str, heading: f32, frame: &dyn Fn(u32, usize) -> u32, stretch: f32| -> Option<Vec<Part>> {
        let (dirs, flip) = dir8(v, heading);
        let mut out = Vec::new();
        for (d, wgt) in dirs {
            if wgt <= 0.0 {
                continue;
            }
            let key = if base == "stand8" { sheet_key(base, char_id) } else { sheet_key(&format!("{base}_{}", DIRS[d]), char_id) };
            let an = m.anims.get(&key)?;
            out.push(Part { key, frame: frame(an.frames, d) % an.frames, flip, scale: VIDEO_SCALE, cx, by: cy + 6.0, alpha: wgt, stretch });
        }
        // the stronger direction is drawn whole, the other over it as much as it shows
        out.sort_by(|a, b| b.alpha.total_cmp(&a.alpha));
        if let Some(f) = out.first_mut() {
            f.alpha = 1.0;
        }
        Some(out)
    };
    let breathe = 1.0 + 0.012 * (p.t * 2.3 + p.phase * 2.0).sin();
    let stand = |heading: f32| eight("stand8", heading, &|_, d| d as u32, breathe);
    // a sheet filmed once (front or back diagonal), looping at the rate it was filmed at
    let filmed = |base: &str, flip: bool, by: f32| {
        let key = sheet_key(base, char_id);
        let an = m.anims.get(&key)?;
        let frame = ((p.t + p.phase) * an.fps.unwrap_or(24.0)) as u32 % an.frames;
        Some(vec![Part { key, frame, flip, scale: VIDEO_SCALE, cx, by, alpha: 1.0, stretch: 1.0 }])
    };
    // a ping pong stroke starts at the hit, so it lines up with this player's turn in the rally
    // (`hit`, as a fraction of the period); between rallies they wait halfway back to ready
    let swing = |hit: f32| {
        let key = sheet_key(if toward { "swing_front" } else { "swing_back" }, char_id);
        let an = m.anims.get(&key)?;
        let n = an.frames as f32;
        let frame = match w.s.game.as_ref().filter(|g| g.state == GameState::Playing) {
            Some(g) if g.swinging(w.s.elapsed) => ((w.s.elapsed - g.t0) / PP_PERIOD - hit).rem_euclid(1.0) * n,
            _ => n / 2.0,
        };
        Some(vec![Part { key, frame: frame as u32 % an.frames, flip: !right, scale: VIDEO_SCALE, cx, by: cy + 6.0, alpha: 1.0, stretch: 1.0 }])
    };
    // seat lifts are for sitting on a sofa or a stool; at a desk the sheet holds the desk too
    let lift = seat.filter(|st| st.kind != SeatKind::Desk).map(|st| st.lift);
    let one = |x: Option<Part>| x.into_iter().collect::<Vec<_>>();
    let got = match p.anim {
        Anim::Walk => {
            let start = |frames: u32| (p.stride / STRIDE).fract() * frames as f32;
            eight("walk8", p.heading, &|frames, _| start(frames) as u32, 1.0)
        }
        Anim::Stand => stand(p.heading),
        Anim::Type => filmed("typing", mirrored, cy + lift.unwrap_or(12.0)),
        Anim::Read => filmed("reading", mirrored, cy + lift.unwrap_or(12.0)),
        Anim::Sit | Anim::Play(_) if p.seated.is_some() || p.anim == Anim::Sit => filmed("sipping", !right, cy + lift.unwrap_or(0.0)),
        Anim::Play(Play::Guitar) => filmed("guitar", !right, cy + 6.0),
        Anim::Play(Play::Pool) => filmed("pool", !right, cy + 6.0),
        // VR turns slowly all the way round
        Anim::Play(Play::Vr) => stand(p.heading + (p.t * 0.55).sin() * 2.6),
        // the rest are played standing at a table (seen from the front) or a machine (from behind)
        Anim::Play(_) => filmed(if toward { "play_se" } else { "play_ne" }, !right, cy + 6.0),
        Anim::PingPongFront => swing(PP_HIT_A),
        Anim::PingPongBack => swing(PP_HIT_A + 0.5),
        _ => None,
    };
    if let Some(parts) = got.filter(|g| !g.is_empty()) {
        return parts;
    }
    one(match p.anim {
        Anim::Walk => old(if toward { "walk" } else { "walkback" }, !right, None),
        Anim::Stand | Anim::Play(_) if toward => old("idle", !right, None),
        Anim::Stand | Anim::Play(_) => old("walkback", !right, Some(0)),
        Anim::Type => old("type", mirrored, None),
        Anim::Read => old("read", mirrored, None),
        Anim::Sit => old("sit", !right, None),
        Anim::PingPongFront => old("ppfront", mirrored, None),
        Anim::PingPongBack => old("ppback", !mirrored, None),
    })
}

/// A part as a quad, recoloured with the avatar's outfit.
fn part_quad(pt: &Part, a: &Avatar, alpha: f32, lift: f32) -> Option<(TexKey, Instance, f32, f32)> {
    let an = meta().anims.get(&pt.key)?;
    let i = pt.frame;
    let sheet_w = an.fw * an.cols;
    let rows = an.frames.div_ceil(an.cols);
    let uv = uv_rect((i % an.cols) * an.fw, (i / an.cols) * an.fh, an.fw, an.fh, (sheet_w, an.fh * rows));
    let (dw, dh) = (an.fw as f32 * pt.scale, an.fh as f32 * pt.scale * pt.stretch);
    let by = pt.by - lift;
    let mut inst = Instance::rect(pt.cx - dw / 2.0, by - dh, dw, dh, uv, pt.flip).alpha(alpha * pt.alpha);
    for (k, region) in [1usize, 2, 3, 5].into_iter().enumerate() {
        if let Some(c) = a.palette.cols[region] {
            inst.cols[k] = [c[0], c[1], c[2], 255];
        }
    }
    Some((TexKey::Sheet(pt.key.clone()), inst, dw, dh))
}

/// How high a ping pong player hops after scoring, and the winner keeps hopping.
fn hop(w: &World, id: u32) -> f32 {
    let Some(g) = w.s.game.as_ref().filter(|g| g.state == GameState::Playing) else { return 0.0 };
    let who = if g.scorer == 0 { Some(g.a) } else { g.b };
    if who != Some(id) {
        return 0.0;
    }
    let age = w.s.elapsed - g.point_at;
    let won = g.score[g.scorer] >= 11;
    let (len, n) = if won { (0.42, 4.0) } else { (0.42, 1.0) };
    if age < 0.0 || age > len * n {
        return 0.0;
    }
    let k = (age / len).fract();
    (if won { 10.0 } else { 7.0 }) * (std::f32::consts::PI * k).sin()
}

/// Where text and the pointer attach to an avatar, in world px.
#[derive(Debug, Clone, Copy)]
pub struct Head {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub top: f32,
    pub bottom: f32,
    paddle: Option<(f32, f32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Agent(u32),
    Prop(usize),
    Board,
}

#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub list: DrawList,
    pub heads: Vec<Head>,
    /// front-most first: (what, world rect x, y, w, h)
    pub hits: Vec<(Hit, [f32; 4])>,
    /// the ping pong scoreboard's flip cards in world px: the near player's, then the far one's
    pub cards: Option<[[f32; 4]; 2]>,
}

impl Scene {
    pub fn pick(&self, wx: f32, wy: f32) -> Option<Hit> {
        self.hits.iter().find(|(_, r)| wx >= r[0] && wx <= r[0] + r[2] && wy >= r[1] && wy <= r[1] + r[3]).map(|(h, _)| *h)
    }
}

pub struct Inputs<'a> {
    pub world: &'a World,
    pub view: &'a View,
    /// floor, then the back, left, front and right walls
    pub statics: Option<&'a [Layer; 5]>,
    /// the whiteboard texture and the world rect it covers before the wall skew
    pub board: Option<&'a Layer>,
    pub hover: Option<Hit>,
    pub selected: Option<u32>,
    pub anim_fps: f32,
    /// wall-clock seconds, for ambient motion
    pub now: f32,
    /// the city around the tower, and its camera
    pub city: Option<(City, Sky)>,
}

fn uv_rect(x: u32, y: u32, w: u32, h: u32, size: (u32, u32)) -> [f32; 4] {
    let (sw, sh) = (size.0.max(1) as f32, size.1.max(1) as f32);
    [x as f32 / sw, y as f32 / sh, (x + w) as f32 / sw, (y + h) as f32 / sh]
}

const FULL: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// The prop's art rect, scale, horizontal centre and baseline, like `propBox`. The art's base is
/// drawn for the classic angle, where it reaches half a diamond below the footprint's centre;
/// anchoring there, rather than at whichever corner is lowest, keeps it on the floor at any angle.
fn prop_box(p: &Prop, v: &View) -> Option<(&'static str, [u32; 4], f32, f32, f32)> {
    let &(file, rect) = meta().items.get(p.name)?;
    let (w, h) = (rect[2] as f32, rect[3] as f32);
    let s = prop_width(p.name).map(|pw| pw / w).unwrap_or_else(|| prop_height(p.name).unwrap_or(50.0) / h);
    let (x, y, fx, fy) = (p.x as f32, p.y as f32, p.fx as f32, p.fy as f32);
    let (cx, cy) = v.floor(x + fx / 2.0, y + fy / 2.0);
    let by = cy + (fx + fy) * TH / 4.0 - if p.fx > 1 || p.fy > 1 { 6.0 } else { 5.0 };
    Some((file, rect, s, cx, by))
}

/// Draw order for a prop: its nearest tile, plus the plan's hint for how it overlaps.
fn prop_depth(p: &Prop, v: &View) -> f32 {
    let tiles = (0..p.fx).flat_map(|i| (0..p.fy).map(move |j| (p.x + i, p.y + j)));
    let near = tiles.map(|(x, y)| v.depth(x as f32 + 0.5, y as f32 + 0.5)).fold(f32::MIN, f32::max);
    // the plan's depth hints are written for the default angle, relative to the front tile
    let hint = p.depth.map_or(0.6, |d| d - (p.x + p.fx - 1 + p.y + p.fy - 1) as f32);
    near + hint
}

/// Props with a front that stand against a wall: which way their front art faces on screen
/// unmirrored, and which way their back art's back panel faces (true is down-right).
fn sides(name: &str) -> Option<(bool, bool)> {
    match name {
        "coffee" | "cooler" | "server" => Some((false, true)),
        "fridge" | "arcade" => Some((true, true)),
        "bookshelf" => Some((false, false)),
        _ => None,
    }
}

/// Away from the wall a prop stands against, into the room.
fn wall_front(p: &Prop, o: &Layout) -> Option<Face> {
    if p.y == 0 {
        Some(Face::DL)
    } else if p.x == 0 {
        Some(Face::DR)
    } else if p.y + p.fy == o.h {
        Some(Face::UR)
    } else if p.x + p.fx == o.w {
        Some(Face::UL)
    } else {
        None
    }
}

/// The prop as a billboard quad and its screen rect. Wall props turn to face the room from any
/// side, and show their back when their front points away from you.
fn prop_quad(p: &Prop, v: &View, o: &Layout) -> Option<(TexKey, Instance, [f32; 4])> {
    let m = meta();
    if p.atlas == Atlas::Desk {
        let (cx, cy) = v.floor(p.x as f32 + 0.5, p.y as f32 + 0.5);
        let (s, lift) = sheet_scale("type");
        let (dw, dh) = (m.desk_empty.0 as f32 * s, m.desk_empty.1 as f32 * s);
        let inst = Instance::rect(cx - dw / 2.0, cy + lift - dh, dw, dh, FULL, v.mirrored());
        return Some((TexKey::Atlas("desk_empty.webp"), inst, [cx - dw / 2.0 + 12.0, cy + lift - dh * 0.55, dw - 24.0, dh * 0.55]));
    }
    let (mut file, mut r, s, cx, by) = prop_box(p, v)?;
    let mut flip = p.flip != v.mirrored();
    if let (Some((front_right, back_right)), Some(f)) = (sides(p.name), wall_front(p, o)) {
        let (toward, right) = v.facing(f);
        if toward {
            flip = right != front_right;
        } else if let Some(&(bf, br)) = m.items.get(&format!("{}_back", p.name)) {
            // the back panel faces the other way from the front
            (file, r, flip) = (bf, br, !right != back_right);
        }
    }
    let (w, h) = (r[2] as f32 * s, r[3] as f32 * s);
    let uv = uv_rect(r[0], r[1], r[2], r[3], m.atlas_size[file]);
    let inst = Instance::rect(cx - w / 2.0, by - h, w, h, uv, flip);
    Some((TexKey::Atlas(file), inst, [cx - w / 2.0, by - h, w, h]))
}

pub fn build(inp: &Inputs) -> Scene {
    let w = inp.world;
    let v = inp.view;
    let o = &w.s.layout;
    let mut sc = Scene::default();
    sc.list.sky = inp.city.map(|(_, s)| s);
    // our floor starts here; the floors below are copied from it
    let office_start = sc.list.items.len();
    let (ww, hh, zt) = (o.w as f32, o.h as f32, wall_height());
    let walls = [
        (Wall::Back, [0.0, 0.0, zt], [ww, 0.0, 0.0]),
        (Wall::Left, [0.0, 0.0, zt], [0.0, hh, 0.0]),
        (Wall::Front, [0.0, hh, zt], [ww, 0.0, 0.0]),
        (Wall::Right, [ww, 0.0, zt], [0.0, hh, 0.0]),
    ];
    // the floor, then the walls behind the room at full height
    if let Some(st) = inp.statics {
        sc.list.layers.extend(st.iter().cloned());
        let (mm, t) = v.plane([0.0, 0.0, 0.0], [ww, 0.0, 0.0], [0.0, hh, 0.0]);
        sc.list.items.push((TexKey::Layer(st[0].id), Instance::affine(mm, t, FULL)));
        for (i, (wall, o3, u)) in walls.iter().enumerate() {
            if v.behind(*wall) {
                let (mm, t) = v.plane(*o3, *u, [0.0, 0.0, -zt]);
                sc.list.items.push((TexKey::Layer(st[i + 1].id), Instance::affine(mm, t, FULL)));
            }
        }
    }
    let back = v.behind(Wall::Back);
    // the whiteboard hangs on the back wall
    let span = (o.bx1 - o.bx0) as f32 * TW / 2.0;
    if let (Some(l), true) = (inp.board, back) {
        sc.list.layers.push(l.clone());
        let (mm, t) = v.wall_rect(true, o.bx0 as f32, -3.0, -110.0, span + 6.0, 76.0);
        sc.list.items.push((TexKey::Layer(l.id), Instance::affine(mm, t, FULL)));
        if matches!(inp.hover, Some(Hit::Board)) {
            let c = [240, 181, 74, 230];
            for (x, y, w2, h2) in [(-5.0, -112.0, span + 10.0, 2.0), (-5.0, -34.0, span + 10.0, 2.0), (-5.0, -112.0, 2.0, 80.0), (span + 3.0, -112.0, 2.0, 80.0)] {
                let (mm, t) = v.wall_rect(true, o.bx0 as f32, x, y, w2, h2);
                sc.list.items.push((TexKey::Fx(Fx::White), Instance::affine(mm, t, FULL).tint(c)));
            }
        }
        let pts = [v.wall_rect(true, o.bx0 as f32, -3.0, -110.0, span + 6.0, 76.0)];
        let (mm, t) = pts[0];
        let xs = [t[0], t[0] + mm[0], t[0] + mm[2], t[0] + mm[0] + mm[2]];
        let ys = [t[1], t[1] + mm[1], t[1] + mm[3], t[1] + mm[1] + mm[3]];
        let (x0, x1) = (xs.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max));
        let (y0, y1) = (ys.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MIN, f32::max));
        sc.hits.push((Hit::Board, [x0, y0, x1 - x0, y1 - y0]));
    }
    // everything that stands on the floor, sorted back to front
    let mut items: Vec<(f32, TexKey, Instance)> = Vec::new();
    let mut hits: Vec<(f32, Hit, [f32; 4])> = Vec::new();
    let occupied: Vec<usize> = w.avatars.iter().filter_map(|a| if let Some(SeatRef::Desk(i)) = a.seated { Some(i) } else { None }).collect();
    let brewing = w.brewing();
    // the machines light up while someone plays them
    let lit: Vec<usize> = w
        .avatars
        .iter()
        .filter(|a| matches!(a.anim, Anim::Play(Play::Pinball | Play::Claw | Play::Console)))
        .filter_map(|a| match a.seated {
            Some(SeatRef::Lounge(i)) => o.lounge.games.iter().find(|g| g.seats.contains(&i)).map(|g| g.prop),
            _ => game_at(w, a).map(|g| o.lounge.games[g].prop),
        })
        .collect();
    for (pi, p) in o.props.iter().enumerate() {
        if p.atlas == Atlas::Desk && o.desks.iter().position(|d| d.x == p.x && d.y == p.y).is_some_and(|i| occupied.contains(&i)) {
            continue;
        }
        let Some((key, mut inst, rect)) = prop_quad(p, v, o) else { continue };
        let d = prop_depth(p, v);
        if lit.contains(&pi) {
            inst.glow = 0.05 + 0.05 * (inp.now * 5.0).sin();
        }
        if inp.hover == Some(Hit::Prop(pi)) {
            inst.glow = 0.16;
        }
        if pi == o.lounge.scoreboard {
            sc.cards = score_cards(p, rect, inst.m[0] < 0.0, o, v);
        }
        items.push((d, key, inst));
        hits.push((d, Hit::Prop(pi), rect));
        if p.name == "coffee" && brewing {
            if let Some((_, r, s, cx, by)) = prop_box(p, v) {
                let (x, y) = (cx + 8.0, by - r[3] as f32 * s + 10.0);
                for k in 0..3 {
                    let u = (inp.now * 0.7 + k as f32 / 3.0).fract();
                    let px = x + (inp.now * 3.0 + k as f32).sin() * 2.5;
                    let r = 2.5 + u * 2.5;
                    let inst = Instance::rect(px - r, y - u * 22.0 - r, 2.0 * r, 2.0 * r, FULL, false).tint([255, 255, 255, (110.0 * (1.0 - u)) as u8]);
                    items.push((d + 0.001, TexKey::Fx(Fx::Dot), inst));
                }
            }
        }
    }
    let glass_h = 44.0 / crate::office::WH * zt;
    for &gy in &o.glass {
        let (mm, t) = v.plane([o.px as f32 + 0.5, gy as f32, glass_h], [0.0, 1.0, 0.0], [0.0, 0.0, -glass_h]);
        items.push((v.depth(o.px as f32 + 0.5, gy as f32 + 0.5) + 0.55, TexKey::Fx(Fx::Glass), Instance::affine(mm, t, FULL)));
    }
    for a in &w.avatars {
        let seat = a.seated.map(|r| w.s.seat(r));
        let d = match seat {
            // the plan's seat hints are written for the default angle, relative to the seat tile
            Some(s) => v.depth(s.x as f32 + 0.5, s.y as f32 + 0.5) + s.depth.map_or(0.65, |h| h - (s.x + s.y) as f32),
            None => v.depth(a.x + 0.5, a.y + 0.5) + 0.5,
        };
        let col = a.color();
        // the shadow and the selection ring lie on the floor, so they turn with it
        let feet = |r: f32| v.plane([a.x + 0.5 - r, a.y + 0.5 - r, 0.0], [2.0 * r, 0.0, 0.0], [0.0, 2.0 * r, 0.0]);
        if seat.is_none() {
            let (mm, t) = feet(0.3);
            items.push((d - 0.002, TexKey::Fx(Fx::Shadow), Instance::affine(mm, t, FULL).tint([10, 12, 20, (0.22 * 255.0 * a.alpha) as u8])));
        }
        let ring = if inp.selected == Some(a.id) { 1.0 } else if inp.hover == Some(Hit::Agent(a.id)) { 0.7 } else { 0.0 };
        if ring > 0.0 {
            let pulse = if inp.selected == Some(a.id) { 1.0 + 0.04 * (inp.now * 4.0).sin() } else { 1.0 };
            let (mm, t) = feet(0.4 * pulse);
            items.push((d - 0.001, TexKey::Fx(Fx::Ring), Instance::affine(mm, t, FULL).tint([col[0], col[1], col[2], (255.0 * ring) as u8])));
        }
        let lift = hop(w, a.id);
        let now_parts = parts(&a.pose(), a.char_id, w, v, inp.anim_fps);
        let Some(main) = now_parts.first() else { continue };
        let desk = main.key.starts_with("typ") || main.key.starts_with("read");
        let (cx, by) = (main.cx, main.by - lift);
        let mut dh = 0.0;
        for (k, pt) in now_parts.iter().enumerate() {
            if let Some((key, inst, _, h)) = part_quad(pt, a, a.alpha, lift) {
                if k == 0 {
                    dh = h;
                }
                items.push((d + k as f32 * 0.0002, key, inst));
            }
        }
        // the pose it just left fades out over the new one
        if let Some(f) = a.fade {
            for (k, pt) in parts(&f.pose, a.char_id, w, v, inp.anim_fps).iter().enumerate() {
                if let Some((key, inst, _, _)) = part_quad(pt, a, a.alpha * (f.left / f.dur).clamp(0.0, 1.0), 0.0) {
                    items.push((d + 0.0006 + k as f32 * 0.0001, key, inst));
                }
            }
        }
        let video = main.scale == VIDEO_SCALE;
        // a desk sheet's frame is the desk's, so the head sits at a fixed height over it
        let (head_y, top) = match (desk, video) {
            (true, _) => (by - 79.0, by - 83.0),
            (false, true) => (by - dh * 0.95 + 2.0, by - dh * 0.96),
            (false, false) => (by - dh + 2.0, by - dh),
        };
        let paddle = match main.key.split("_c").next() {
            Some("ppfront") => Some((cx + if main.flip { -16.0 } else { 16.0 }, by - 34.0)),
            Some("ppback") => Some((cx + if main.flip { -14.0 } else { 14.0 }, by - 40.0)),
            // a filmed stroke knows where its paddle is at the hit; if it was not found, about there
            Some("swing_front" | "swing_back") => {
                let [x, y] = meta().anims.get(&main.key).and_then(|an| an.paddle).unwrap_or([50.0, 90.0]);
                Some((cx + if main.flip { -x } else { x } * main.scale, by - y * main.scale))
            }
            _ => None,
        };
        sc.heads.push(Head { id: a.id, x: cx, y: head_y, top, bottom: by, paddle });
        // the clickable body: a narrow box around the figure, not the whole frame
        let bw = match (desk, video) {
            (true, _) => 32.0,
            (false, true) => 24.0,
            (false, false) => 0.6 * meta().anims.get(&main.key).map_or(40.0, |an| an.fw as f32 * main.scale),
        };
        hits.push((d + 0.01, Hit::Agent(a.id), [cx - bw / 2.0, top, bw, by - top]));
        games_fx(a, w, v, inp.now, cx, by, d, &mut items);
    }
    items.sort_by(|p, q| p.0.total_cmp(&q.0));
    sc.list.items.extend(items.into_iter().map(|(_, k, i)| (k, i)));
    // the tower goes on below: two dimmer floors with our layout, no people, no board
    if let Some((c, _)) = &inp.city {
        let board = inp.board.map(|l| l.id);
        city::tower(&mut sc.list.items, office_start, *c, v, o, |k| match k {
            TexKey::Sheet(_) | TexKey::Fx(Fx::Shadow | Fx::Ring | Fx::Dot) => false,
            TexKey::Layer(id) => Some(*id) != board,
            _ => true,
        });
    }
    // walls between you and the room are cut down to a skirting, drawn over everything
    if let Some(st) = inp.statics {
        for (i, (wall, o3, u)) in walls.iter().enumerate() {
            if !v.behind(*wall) {
                let hcut = zt * CUTAWAY;
                let (mm, t) = v.plane([o3[0], o3[1], hcut], *u, [0.0, 0.0, -hcut]);
                sc.list.items.push((TexKey::Layer(st[i + 1].id), Instance::affine(mm, t, [0.0, 1.0 - CUTAWAY, 1.0, 1.0])));
            }
        }
    }
    // people win over furniture when both are under the pointer
    hits.sort_by(|p, q| {
        let pa = matches!(p.1, Hit::Agent(_));
        let qa = matches!(q.1, Hit::Agent(_));
        qa.cmp(&pa).then(q.0.total_cmp(&p.0))
    });
    let board = sc.hits.pop();
    sc.hits.extend(hits.into_iter().map(|(_, h, r)| (h, r)));
    sc.hits.extend(board);

    ball(w, v, &sc.heads.clone(), &mut sc.list);
    sc
}

/// The ping pong ball flies paddle to paddle with a bounce on the table; the one that gets
/// past a player carries on and drops off the end.
fn ball(w: &World, v: &View, heads: &[Head], list: &mut DrawList) {
    let Some(g) = w.s.game.as_ref() else { return };
    let Some(b) = g.ball(w.s.elapsed) else { return };
    let pa = heads.iter().find(|h| h.id == g.a).and_then(|h| h.paddle);
    let pb = heads.iter().find(|h| Some(h.id) == g.b).and_then(|h| h.paddle);
    let (Some(pa), Some(pb)) = (pa, pb) else { return };
    let (from, k, out) = match b {
        Ball::Flight { from, k } => (from, k, false),
        Ball::Out { from, k } => (from, k, true),
    };
    let (from, to) = if from == 0 { (pa, pb) } else { (pb, pa) };
    let o = &w.s.layout;
    let t = &o.props[o.lounge.pp_table];
    let (_, tcy) = v.floor(t.x as f32 + 1.0, t.y as f32 + 0.5);
    let surf = tcy - 22.0;
    let bounce = 0.66;
    let bx = from.0 + (to.0 - from.0) * bounce;
    let (x, y) = if k < bounce {
        let q = k / bounce;
        (from.0 + (bx - from.0) * q, from.1 + (surf - from.1) * q - 18.0 * (std::f32::consts::PI * q).sin())
    } else if k <= 1.0 {
        let q = (k - bounce) / (1.0 - bounce);
        (bx + (to.0 - bx) * q, surf + (to.1 - surf) * q - 12.0 * (std::f32::consts::PI * q).sin())
    } else {
        // past the paddle: on along the same line, falling
        let q = k - 1.0;
        let dir = (to.0 - bx) / (1.0 - bounce);
        (to.0 + dir * q * 0.8, to.1 + 70.0 * q * q)
    };
    let fade = if out { (1.0 - (k - 1.0) / 0.7).clamp(0.0, 1.0) } else { 1.0 };
    let shadow_y = (y + 3.0).max(surf + 4.0 + (tcy - surf) * 0.2);
    if !out || k < 1.15 {
        list.items.push((TexKey::Fx(Fx::Shadow), Instance::rect(x - 3.0, shadow_y - 1.4, 6.0, 2.8, FULL, false).tint([10, 14, 22, 90])));
    }
    list.items.push((TexKey::Fx(Fx::Dot), Instance::rect(x - 2.6, y - 2.6, 5.2, 5.2, FULL, false).tint([255, 253, 246, (255.0 * fade) as u8])));
}

/// The scoreboard's two cards in world px, the near player's first: each card shows the score
/// of the player on its side of the screen.
fn score_cards(p: &Prop, rect: [f32; 4], flipped: bool, o: &Layout, v: &View) -> Option<[[f32; 4]; 2]> {
    let m = meta();
    let &(_, item) = m.items.get(p.name)?;
    let k = rect[2] / item[2] as f32;
    let mut cards = m.score_cards.map(|c| {
        let x = if flipped { rect[0] + rect[2] - (c[0] + c[2]) as f32 * k } else { rect[0] + c[0] as f32 * k };
        [x, rect[1] + c[1] as f32 * k, c[2] as f32 * k, c[3] as f32 * k]
    });
    cards.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let (ax, _) = v.floor(o.lounge.pp_a.x as f32 + 0.5, o.lounge.pp_a.y as f32 + 0.5);
    let (bx, _) = v.floor(o.lounge.pp_b.x as f32 + 0.5, o.lounge.pp_b.y as f32 + 0.5);
    if ax > bx {
        cards.swap(0, 1);
    }
    Some(cards)
}

/// The lounge game an avatar stands at, by the spot it is on.
fn game_at(w: &World, a: &Avatar) -> Option<usize> {
    let t = crate::office::tile(a.x.round() as i32, a.y.round() as i32);
    w.s.layout.lounge.games.iter().position(|g| g.spots.iter().any(|s| s.0 == t))
}

/// The little things a game does while someone plays: a puck or ball on the table, a shot at
/// the hoop, notes from the guitar, lights on the machines.
fn games_fx(a: &Avatar, w: &World, v: &View, now: f32, cx: f32, by: f32, d: f32, items: &mut Vec<(f32, TexKey, Instance)>) {
    let Anim::Play(kind) = a.anim else { return };
    let t = a.t + a.phase;
    let dot = |x: f32, y: f32, r: f32, c: [u8; 4]| Instance::rect(x - r, y - r, 2.0 * r, 2.0 * r, FULL, false).tint(c);
    let o = &w.s.layout;
    let prop = game_at(w, a).map(|g| &o.props[o.lounge.games[g].prop]);
    match (kind, prop) {
        (Play::AirHockey | Play::Foosball, Some(p)) => {
            // back and forth along the table, a touch above its top
            let u = 0.5 + 0.42 * (t * if kind == Play::AirHockey { 2.6 } else { 1.7 }).sin() * (t * 0.37).cos().signum();
            let (x, y) = v.floor(p.x as f32 + u * p.fx as f32, p.y as f32 + 0.5 + 0.18 * (t * 3.1).sin());
            let lift = if kind == Play::AirHockey { 26.0 } else { 30.0 };
            let c = if kind == Play::AirHockey { [210, 52, 48, 255] } else { [250, 250, 245, 255] };
            items.push((v.depth(p.x as f32 + 1.0, p.y as f32 + 0.5) + 0.7, TexKey::Fx(Fx::Dot), dot(x, y - lift, 2.6, c)));
        }
        (Play::Hoops, Some(p)) => {
            // a shot every 1.6 s: from the hands up to the hoop at the machine's far end, then down
            let u = (t / 1.6).fract();
            let (hx, hy) = v.floor(p.x as f32 + 0.3, p.y as f32 + 0.5);
            let (hx, hy) = (hx, hy - 92.0);
            let (sx, sy) = (cx, by - 48.0);
            let (x, y) = if u < 0.6 {
                let q = u / 0.6;
                (sx + (hx - sx) * q, sy + (hy - sy) * q - 46.0 * (std::f32::consts::PI * q).sin())
            } else {
                let q = (u - 0.6) / 0.4;
                (hx, hy + q * 40.0)
            };
            items.push((d + 0.02, TexKey::Fx(Fx::Dot), dot(x, y, 3.4, [232, 118, 40, 255])));
        }
        (Play::Guitar, _) => {
            // notes drift up from the guitar
            for k in 0..2 {
                let u = (t * 0.6 + k as f32 * 0.5).fract();
                let x = cx + 10.0 + u * 10.0 + (t * 2.0 + k as f32).sin() * 3.0;
                let inst = dot(x, by - 40.0 - u * 30.0, 1.8, [255, 236, 170, (220.0 * (1.0 - u)) as u8]);
                items.push((d + 0.02, TexKey::Fx(Fx::Dot), inst));
            }
        }
        _ => {}
    }
    let _ = now;
}

pub fn hsl(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8]
}

/// Screen size of a tile, for callers that place things relative to the grid.
pub const TILE: (f32, f32) = (TW, TH);

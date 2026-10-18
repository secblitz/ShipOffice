//! Turns one office into a depth-sorted list of quads, plus the anchors the text overlay and
//! the pointer need. Mirrors `render()` in the browser prototype.

use super::assets::{meta, sheet_key};
use super::paint::CUTAWAY;
use super::view::{wall_height, View, Wall};
use super::{DrawList, Fx, Instance, Layer, TexKey};
use crate::office::gen::{Atlas, Prop, SeatKind};
use crate::office::world::{Anim, Avatar, GameState, SeatRef, World};
use crate::office::{TH, TW};

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
        _ => return None,
    })
}

fn prop_width(name: &str) -> Option<f32> {
    match name {
        "pingpong" => Some(108.0),
        "sofa" => Some(100.0),
        "rug" => Some(124.0),
        _ => None,
    }
}

/// logical animation + facing -> (sheet family, mirrored, fixed frame). Facing is in the room;
/// the view decides how it reads on screen.
fn resolve(a: &Avatar, v: &View) -> (&'static str, bool, Option<u32>) {
    let (toward, right) = v.facing(a.face);
    let m = v.mirrored();
    match a.anim {
        Anim::Walk => (if toward { "walk" } else { "walkback" }, !right, None),
        Anim::Stand if toward => ("idle", !right, None),
        Anim::Stand => ("walkback", !right, Some(0)),
        Anim::Type => ("type", m, None),
        Anim::Read => ("read", m, None),
        Anim::Sit => ("sit", !right, None),
        Anim::PingPongFront => ("ppfront", m, None),
        Anim::PingPongBack => ("ppback", !m, None),
    }
}

// ping pong: both players run on the game clock so their swings alternate with the ball
const PP_PERIOD: f32 = 1.05;
const PP_HIT_A: f32 = 9.0 / 15.0;
const PP_HIT_B: f32 = 10.0 / 16.0;

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
}

fn uv_rect(x: u32, y: u32, w: u32, h: u32, size: (u32, u32)) -> [f32; 4] {
    let (sw, sh) = (size.0.max(1) as f32, size.1.max(1) as f32);
    [x as f32 / sw, y as f32 / sh, (x + w) as f32 / sw, (y + h) as f32 / sh]
}

const FULL: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// The prop's art rect, scale, horizontal centre and baseline, like `propBox`: centred on its
/// footprint, standing on the footprint's lowest corner on screen.
fn prop_box(p: &Prop, v: &View) -> Option<(&'static str, [u32; 4], f32, f32, f32)> {
    let &(file, rect) = meta().items.get(p.name)?;
    let (w, h) = (rect[2] as f32, rect[3] as f32);
    let s = prop_width(p.name).map(|pw| pw / w).unwrap_or_else(|| prop_height(p.name).unwrap_or(50.0) / h);
    let (x, y, fx, fy) = (p.x as f32, p.y as f32, p.fx as f32, p.fy as f32);
    let corners = [v.floor(x, y), v.floor(x + fx, y), v.floor(x, y + fy), v.floor(x + fx, y + fy)];
    let cx = v.floor(x + fx / 2.0, y + fy / 2.0).0;
    let by = corners.iter().map(|c| c.1).fold(f32::MIN, f32::max) - if p.fx > 1 || p.fy > 1 { 6.0 } else { 5.0 };
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

/// The prop as a billboard quad and its screen rect.
fn prop_quad(p: &Prop, v: &View) -> Option<(TexKey, Instance, [f32; 4])> {
    let m = meta();
    if p.atlas == Atlas::Desk {
        let (cx, cy) = v.floor(p.x as f32 + 0.5, p.y as f32 + 0.5);
        let (s, lift) = sheet_scale("type");
        let (dw, dh) = (m.desk_empty.0 as f32 * s, m.desk_empty.1 as f32 * s);
        let inst = Instance::rect(cx - dw / 2.0, cy + lift - dh, dw, dh, FULL, v.mirrored());
        return Some((TexKey::Atlas("desk_empty.webp"), inst, [cx - dw / 2.0 + 12.0, cy + lift - dh * 0.55, dw - 24.0, dh * 0.55]));
    }
    let (file, r, s, cx, by) = prop_box(p, v)?;
    let (w, h) = (r[2] as f32 * s, r[3] as f32 * s);
    let uv = uv_rect(r[0], r[1], r[2], r[3], m.atlas_size[file]);
    let inst = Instance::rect(cx - w / 2.0, by - h, w, h, uv, p.flip != v.mirrored());
    Some((TexKey::Atlas(file), inst, [cx - w / 2.0, by - h, w, h]))
}

pub fn build(inp: &Inputs) -> Scene {
    let w = inp.world;
    let v = inp.view;
    let o = &w.s.layout;
    let m = meta();
    let mut sc = Scene::default();
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
    // the projector over the lounge lights up when someone is on the couch
    if back {
        let lit = w.avatars.iter().any(|a| matches!(a.seated, Some(r) if w.s.seat(r).kind == SeatKind::Couch));
        let pspan = (o.w - o.l0) as f32 * TW / 2.0;
        let (mm, t) = v.wall_rect(true, o.l0 as f32, -2.0, -112.0, pspan + 4.0, 50.0);
        sc.list.items.push((TexKey::Fx(Fx::White), Instance::affine(mm, t, FULL).tint([0x20, 0x23, 0x2b, 255])));
        let screen = if lit { hsl((inp.now * 8.0) % 360.0, 0.5, 0.45) } else { [0x2b, 0x2f, 0x3a] };
        let (mm, t) = v.wall_rect(true, o.l0 as f32, 1.0, -109.0, pspan - 2.0, 44.0);
        sc.list.items.push((TexKey::Fx(Fx::White), Instance::affine(mm, t, FULL).tint([screen[0], screen[1], screen[2], 255])));
        if lit {
            let x = (inp.now * 14.0) % (pspan + 30.0) - 15.0;
            let (mm, t) = v.wall_rect(true, o.l0 as f32, x, -76.0, 9.0, 12.0);
            sc.list.items.push((TexKey::Fx(Fx::White), Instance::affine(mm, t, FULL).tint([20, 24, 34, 216])));
            let (mm, t) = v.wall_rect(true, o.l0 as f32, pspan * 0.72, -103.0 + (inp.now * 0.6).sin() * 3.0, 10.0, 10.0);
            sc.list.items.push((TexKey::Fx(Fx::Dot), Instance::affine(mm, t, FULL).tint([255, 240, 200, 230])));
        }
    }

    // everything that stands on the floor, sorted back to front
    let mut items: Vec<(f32, TexKey, Instance)> = Vec::new();
    let mut hits: Vec<(f32, Hit, [f32; 4])> = Vec::new();
    let occupied: Vec<usize> = w.avatars.iter().filter_map(|a| if let Some(SeatRef::Desk(i)) = a.seated { Some(i) } else { None }).collect();
    let brewing = w.brewing();
    for (pi, p) in o.props.iter().enumerate() {
        if p.atlas == Atlas::Desk && o.desks.iter().position(|d| d.x == p.x && d.y == p.y).is_some_and(|i| occupied.contains(&i)) {
            continue;
        }
        let Some((key, mut inst, rect)) = prop_quad(p, v) else { continue };
        let d = prop_depth(p, v);
        if inp.hover == Some(Hit::Prop(pi)) {
            inst.glow = 0.16;
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
        let (base, flip, fixed) = resolve(a, v);
        let key = sheet_key(base, a.char_id);
        let Some(an) = m.anims.get(&key) else { continue };
        let (s, lift0) = sheet_scale(base);
        let (px0, cy) = v.floor(a.x + 0.5, a.y + 0.5);
        let cx = px0 + seat.map_or(0.0, |s| if v.mirrored() { -s.dx } else { s.dx });
        let lift = match seat {
            Some(st) if st.kind != SeatKind::Desk => st.lift,
            _ => lift0,
        };
        let by = cy + lift;
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
        let i = match fixed {
            Some(f) => f,
            None => {
                let playing = w.s.game.as_ref().filter(|g| g.state == GameState::Playing && base.starts_with("pp"));
                match playing {
                    Some(g) => {
                        let u = (w.s.elapsed - g.t0) / PP_PERIOD;
                        let shift = if base == "ppfront" { 0.0 } else { PP_HIT_B - PP_HIT_A - 0.5 };
                        (((u + shift).rem_euclid(1.0)) * an.frames as f32) as u32 % an.frames
                    }
                    None => ((a.t + a.phase) * inp.anim_fps) as u32 % an.frames,
                }
            }
        };
        let sheet_w = an.fw * an.cols;
        let rows = an.frames.div_ceil(an.cols);
        let uv = uv_rect((i % an.cols) * an.fw, (i / an.cols) * an.fh, an.fw, an.fh, (sheet_w, an.fh * rows));
        let (dw, dh) = (an.fw as f32 * s, an.fh as f32 * s);
        let mut inst = Instance::rect(cx - dw / 2.0, by - dh, dw, dh, uv, flip).alpha(a.alpha);
        for (k, region) in [1usize, 2, 3, 5].into_iter().enumerate() {
            if let Some(c) = a.palette.cols[region] {
                inst.cols[k] = [c[0], c[1], c[2], 255];
            }
        }
        items.push((d, TexKey::Sheet(key), inst));
        let desk = base == "type" || base == "read";
        let head_y = by - dh + if desk { 17.0 } else { 2.0 };
        let top = by - dh + if desk { 13.0 } else { 0.0 };
        let paddle = match base {
            "ppfront" => Some((cx + if flip { -16.0 } else { 16.0 }, by - 34.0)),
            "ppback" => Some((cx + if flip { -14.0 } else { 14.0 }, by - 40.0)),
            _ => None,
        };
        sc.heads.push(Head { id: a.id, x: cx, y: head_y, top, bottom: by, paddle });
        // the clickable body: a narrow box around the figure, not the whole frame
        let bw = if desk { dw * 0.45 } else { dw * 0.6 };
        hits.push((d + 0.01, Hit::Agent(a.id), [cx - bw / 2.0, top, bw, by - top]));
    }
    items.sort_by(|p, q| p.0.total_cmp(&q.0));
    sc.list.items.extend(items.into_iter().map(|(_, k, i)| (k, i)));
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

/// The ping pong ball flies paddle to paddle with a bounce on the table.
fn ball(w: &World, v: &View, heads: &[Head], list: &mut DrawList) {
    let Some(g) = w.s.game.as_ref().filter(|g| g.state == GameState::Playing) else { return };
    let pa = heads.iter().find(|h| h.id == g.a).and_then(|h| h.paddle);
    let pb = heads.iter().find(|h| Some(h.id) == g.b).and_then(|h| h.paddle);
    let (Some(pa), Some(pb)) = (pa, pb) else { return };
    let u = ((w.s.elapsed - g.t0) / PP_PERIOD - PP_HIT_A).rem_euclid(1.0);
    let forward = u < 0.5;
    let s = if forward { u / 0.5 } else { (u - 0.5) / 0.5 };
    let (from, to) = if forward { (pa, pb) } else { (pb, pa) };
    let o = &w.s.layout;
    let t = &o.props[o.lounge.pp_table];
    let (_, tcy) = v.floor(t.x as f32 + 1.0, t.y as f32 + 0.5);
    let surf = tcy - 22.0;
    let bounce = 0.66;
    let bx = from.0 + (to.0 - from.0) * bounce;
    let (x, y) = if s < bounce {
        let k = s / bounce;
        (from.0 + (bx - from.0) * k, from.1 + (surf - from.1) * k - 18.0 * (std::f32::consts::PI * k).sin())
    } else {
        let k = (s - bounce) / (1.0 - bounce);
        (bx + (to.0 - bx) * k, surf + (to.1 - surf) * k - 12.0 * (std::f32::consts::PI * k).sin())
    };
    let shadow_y = (y + 3.0).max(surf + 4.0 + (tcy - surf) * 0.2);
    list.items.push((TexKey::Fx(Fx::Shadow), Instance::rect(x - 3.0, shadow_y - 1.4, 6.0, 2.8, FULL, false).tint([10, 14, 22, 90])));
    list.items.push((TexKey::Fx(Fx::Dot), Instance::rect(x - 2.6, y - 2.6, 5.2, 5.2, FULL, false).tint([255, 253, 246, 255])));
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

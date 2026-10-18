//! The office as a floor of a city tower. The city itself is ray cast on the GPU (sky.wgsl)
//! from a camera that turns with the room; here we aim that camera, and build the tower under
//! our floor: two dimmer floors you can see into, behind a curtain wall of glass, slabs and
//! mullions that runs on down into the haze.

use super::view::{wall_height, View, Wall, MAX_PITCH, MIN_PITCH};
use super::{Fx, Instance, TexKey};
use crate::office::gen::Layout;
use bytemuck::{Pod, Zeroable};

const FULL: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
/// Floor to floor, in tile units: the room's walls plus the slab under the next floor up.
pub fn storey() -> f32 {
    wall_height() * 1.25
}
/// Floors shown under ours: the furnished ones you can see into, and the whole run of facade.
const FURNISHED: usize = 2;
const STOREYS: usize = 7;
/// How high the camera floats above the street (tiles), and how far a pan moves it.
const HEIGHT: f32 = 130.0;
const PAN: f32 = 0.03;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct City {
    pub night: bool,
}

/// The sky pass's uniforms; see `Sky` in sky.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct Sky {
    pub right: [f32; 4],
    pub up: [f32; 4],
    pub fwd: [f32; 4],
    pub cam: [f32; 4],
    pub lens: [f32; 4],
}

/// The city camera's tilt for the room's: level-ish at the classic angle so the skyline shows,
/// straight down when the room is seen from above.
fn tilt(pitch: f32) -> f32 {
    let t = ((pitch - MIN_PITCH) / (MAX_PITCH - MIN_PITCH)).clamp(0.0, 1.0);
    std::f32::consts::FRAC_PI_2 * t.powf(1.6)
}

/// Aims the city camera: the room's turn, a gentler tilt, a slightly narrower lens as you zoom,
/// and a small drift as you pan (`pan` in world px from home), so the street feels far below.
pub fn sky(c: City, v: &View, aspect: f32, zoom: f32, pan: (f32, f32), now: f32) -> Sky {
    let (sin, cos) = v.yaw.sin_cos();
    let (sp, cp) = tilt(v.pitch).sin_cos();
    // the same axes the room projects with (see View::project), at the city's tilt
    let right = [cos, -sin, 0.0, 0.0];
    let up = [-sin * sp, -cos * sp, cp, 0.0];
    let fwd = [-sin * cp, -cos * cp, -sp, 0.0];
    let shift = (PAN * (pan.0 * cos + pan.1 * sin), PAN * (-pan.0 * sin + pan.1 * cos));
    let ty = 0.577 / zoom.max(0.2).powf(0.2);
    Sky { right, up, fwd, cam: [shift.0, shift.1, HEIGHT, now], lens: [ty * aspect.max(0.1), ty, if c.night { 1.0 } else { 0.0 }, 0.0] }
}

struct Look {
    shade: [f32; FURNISHED],
    glass: [u8; 4],
    sheen: [u8; 4],
    slab: [u8; 3],
    mullion: [u8; 4],
    haze: [u8; 3],
}

fn look(night: bool) -> Look {
    if night {
        Look { shade: [0.46, 0.34], glass: [22, 32, 58, 120], sheen: [120, 150, 220, 26], slab: [46, 52, 70], mullion: [24, 28, 40, 230], haze: [38, 46, 74] }
    } else {
        Look { shade: [0.70, 0.56], glass: [150, 192, 218, 110], sheen: [255, 255, 255, 46], slab: [214, 210, 202], mullion: [74, 84, 98, 235], haze: [214, 226, 230] }
    }
}

fn scale(c: [u8; 3], k: f32) -> [u8; 3] {
    [(c[0] as f32 * k) as u8, (c[1] as f32 * k) as u8, (c[2] as f32 * k) as u8]
}

fn mixc(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

/// The tower under our floor, inserted before it in `list` (from `from`, where our floor starts):
/// copies of our floor (furniture, no people) a storey apart, then the curtain wall in front.
pub fn tower(list: &mut Vec<(TexKey, Instance)>, from: usize, c: City, v: &View, o: &Layout, keep: impl Fn(&TexKey) -> bool) {
    let l = look(c.night);
    let ours: Vec<(TexKey, Instance)> = list[from..].iter().filter(|(k, _)| keep(k)).cloned().collect();
    let st = storey();
    let dy = v.project(0.0, 0.0, -st).1 - v.project(0.0, 0.0, 0.0).1;
    let mut below = Vec::with_capacity(ours.len() * FURNISHED + 256);
    for n in (1..=FURNISHED).rev() {
        let shade = l.shade[n - 1];
        for (key, inst) in &ours {
            let mut i = *inst;
            i.t[1] += dy * n as f32;
            for ch in 0..3 {
                i.tint[ch] = (i.tint[ch] as f32 * shade) as u8;
            }
            below.push((key.clone(), i));
        }
    }
    facade(&mut below, &l, v, o);
    list.splice(from..from, below);
}

/// Glass, slabs and mullions on the sides of the tower that face you. Deeper storeys sink into
/// the haze and fade out.
fn facade(out: &mut Vec<(TexKey, Instance)>, l: &Look, v: &View, o: &Layout) {
    let (w, h) = (o.w as f32, o.h as f32);
    let (st, wh) = (storey(), wall_height());
    let slab = st - wh;
    for wall in [Wall::Front, Wall::Right, Wall::Back, Wall::Left] {
        if v.behind(wall) {
            continue;
        }
        // the face's corner, its run along the floor, its length, and how it catches the light
        let (start, run, len, light) = match wall {
            Wall::Front => ([0.0, h], [1.0, 0.0], w, 0.86),
            Wall::Back => ([0.0, 0.0], [1.0, 0.0], w, 0.86),
            Wall::Right => ([w, 0.0], [0.0, 1.0], h, 1.0),
            Wall::Left => ([0.0, 0.0], [0.0, 1.0], h, 1.0),
        };
        let quad = |z0: f32, z1: f32, a: f32, b: f32| v.plane([start[0] + run[0] * a, start[1] + run[1] * a, z1], [run[0] * (b - a), run[1] * (b - a), 0.0], [0.0, 0.0, z0 - z1]);
        for n in 0..STOREYS {
            // n = 0 is the slab under our own floor
            let top = -(n as f32) * st;
            let depth = (n as f32 - 2.0).max(0.0) / (STOREYS as f32 - 2.0);
            let fade = 1.0 - depth.powf(1.4);
            let fog = depth * 0.85;
            let slab_c = mixc(scale(l.slab, light), l.haze, fog);
            let (m, t) = quad(top - slab, top, 0.0, len);
            out.push((TexKey::Fx(Fx::White), Instance::affine(m, t, FULL).tint([slab_c[0], slab_c[1], slab_c[2], 255]).alpha(fade)));
            // the edge of the slab catches the light
            let (m, t) = quad(top - 0.05, top, 0.0, len);
            let lip = mixc(scale(l.slab, (light * 1.12).min(1.0)), l.haze, fog);
            out.push((TexKey::Fx(Fx::White), Instance::affine(m, t, FULL).tint([lip[0], lip[1], lip[2], 255]).alpha(fade)));
            if n + 1 >= STOREYS {
                break;
            }
            let (g0, g1) = (top - st, top - slab);
            // the furnished floors show through; the ones under them are glass on the haze
            let furnished = n < FURNISHED;
            let mut glass = l.glass;
            if !furnished {
                let g = mixc(scale([glass[0], glass[1], glass[2]], 0.8), l.haze, fog);
                glass = [g[0], g[1], g[2], 235];
            }
            let gl = scale([glass[0], glass[1], glass[2]], light);
            let (m, t) = quad(g0, g1, 0.0, len);
            out.push((TexKey::Fx(Fx::White), Instance::affine(m, t, FULL).tint([gl[0], gl[1], gl[2], glass[3]]).alpha(fade)));
            // a soft sheen, brighter at the top of each pane
            let (m, t) = quad(g0, g1, 0.0, len);
            out.push((TexKey::Fx(Fx::Ramp), Instance::affine(m, t, [0.0, 1.0, 1.0, 0.0]).tint(l.sheen).alpha(fade)));
            // mullions every two tiles, and at the corners
            let panes = (len / 2.0).round().max(1.0) as i32;
            for k in 0..=panes {
                let a = k as f32 * len / panes as f32;
                let half = if k == 0 || k == panes { 0.09 } else { 0.05 };
                let (m, t) = quad(g0, g1, (a - half).max(0.0), (a + half).min(len));
                let mc = mixc([l.mullion[0], l.mullion[1], l.mullion[2]], l.haze, fog);
                out.push((TexKey::Fx(Fx::White), Instance::affine(m, t, FULL).tint([mc[0], mc[1], mc[2], l.mullion[3]]).alpha(fade)));
            }
        }
    }
}

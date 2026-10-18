//! The city around the office: a painted skyline, rooftops and a street far below, each a layer
//! that slides at its own depth as the view turns, with birds and traffic, and two dimmer floors
//! of the tower under ours. Laid out in screen space and handed to the GPU in world units.

use super::view::{wall_height, View};
use super::{Fx, Instance, TexKey};
use std::f32::consts::TAU;

/// Pixel size of every city layer (two mirrored copies of the painting side by side, so the strip
/// repeats without a seam).
const LAYER: (f32, f32) = (2600.0, 322.0);
const FULL: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
/// The avenue in the street painting, as a fraction of its width, once in each mirrored half.
const LANES: [f32; 2] = [0.248, 0.750];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct City {
    pub night: bool,
}

/// How world px land on the screen: `screen = world · scale + origin`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    pub scale: f32,
    pub origin: (f32, f32),
    pub w: f32,
    pub h: f32,
}

impl Screen {
    /// A screen rect as a world-space quad.
    fn rect(&self, x: f32, y: f32, w: f32, h: f32, uv: [f32; 4]) -> Instance {
        let s = self.scale;
        Instance::rect((x - self.origin.0) / s, (y - self.origin.1) / s, w / s, h / s, uv, false)
    }
}

struct Look {
    sky: [u8; 3],
    /// the skyline's own colour along its bottom edge, filling below it
    horizon: [u8; 3],
    ground: [u8; 3],
    haze: [u8; 4],
    bird: [u8; 4],
    shade: [f32; 2],
}

fn look(night: bool) -> Look {
    if night {
        Look { sky: [0x10, 0x17, 0x35], horizon: [0x2d, 0x35, 0x46], ground: [0x2d, 0x38, 0x56], haze: [36, 44, 74, 97], bird: [150, 160, 200, 115], shade: [0.40, 0.27] }
    } else {
        Look { sky: [0xbd, 0xe5, 0xf5], horizon: [0xbd, 0xc7, 0xc8], ground: [0xa1, 0x99, 0x93], haze: [225, 238, 240, 115], bird: [40, 52, 70, 140], shade: [0.66, 0.52] }
    }
}

/// `v` wrapped into one period, centred on zero.
fn wrap(v: f32, period: f32) -> f32 {
    let m = v.rem_euclid(period);
    if m > period / 2.0 {
        m - period
    } else {
        m
    }
}

/// The far-to-near layers behind the office.
pub fn backdrop(list: &mut Vec<(TexKey, Instance)>, c: City, s: &Screen, v: &View, now: f32) {
    let l = look(c.night);
    let (w, h) = (s.w, s.h);
    let k = h / 760.0;
    let turn = v.yaw / TAU;
    let tilt = v.pitch - super::view::DEFAULT_PITCH;
    let solid = |list: &mut Vec<(TexKey, Instance)>, y: f32, hh: f32, c: [u8; 4]| list.push((TexKey::Fx(Fx::White), s.rect(0.0, y, w, hh, FULL).tint(c)));
    solid(list, 0.0, h, [l.sky[0], l.sky[1], l.sky[2], 255]);

    // a strip of a layer tiled across the screen, shifted by `depth` panoramas per full turn
    let strip = |list: &mut Vec<(TexKey, Instance)>, file: &'static str, top: f32, hh: f32, depth: f32| -> (f32, f32) {
        let period = hh * LAYER.0 / LAYER.1;
        let x0 = wrap(-turn * period * depth, period) - period;
        let mut x = x0;
        while x < w {
            list.push((TexKey::Atlas(file), s.rect(x, top, period, hh, FULL)));
            x += period;
        }
        (x0, period)
    };
    let (night, day) = (c.night, !c.night);
    let pick = |n: &'static str, d: &'static str| if night { n } else { d };

    // the skyline: tilting down lifts the horizon
    let far_h = h * 0.52;
    let far_top = h * 0.02 - tilt * h * 0.45;
    // below the skyline, its own colour, so tilting never opens a gap above the rooftops
    solid(list, far_top + far_h - 1.0, h, [l.horizon[0], l.horizon[1], l.horizon[2], 255]);
    strip(list, pick("city/far-night.webp", "city/far-day.webp"), far_top, far_h, 1.0);

    // a small flock now and then, wings beating
    let (_, far_period) = (0.0, far_h * LAYER.0 / LAYER.1);
    let flock = wrap(-turn * far_period * 1.5, far_period);
    let span = w + 400.0 * k;
    for i in 0..7 {
        let fi = i as f32;
        let dur = 42.0 + (fi * 1.7) % 6.0;
        let p = ((now + fi * 0.6) / dur).fract();
        let x = -200.0 * k + p * span + flock + fi * 22.0 * k;
        let x = (x + 200.0 * k).rem_euclid(span) - 200.0 * k;
        let y = far_top + far_h * 0.2 + ((i % 3) as f32 * 13.0 + (fi * 7.3) % 10.0) * k;
        let up = ((now * 4.0 + fi * 0.37) as i32) % 2 == 0;
        let (bw, bh) = (16.0 * k, 10.0 * k);
        list.push((TexKey::Fx(Fx::Bird(up)), s.rect(x, y, bw, bh, FULL).tint(l.bird)));
    }

    // the city below: rooftops and the street, sliding twice as fast
    let near_top = h * 0.43 - tilt * h * 0.26;
    // reaches the bottom of the screen however far the view tilts
    let near_h = (h - near_top + 2.0).max(h * 0.57);
    solid(list, near_top + near_h * 0.5, h, [l.ground[0], l.ground[1], l.ground[2], 255]);
    let (x0, period) = strip(list, pick("city/near-night.webp", "city/near-day.webp"), near_top, near_h, 2.0);

    // traffic both ways down the avenue: headlights coming, tail lights going (cabs by day)
    let mut x = x0;
    while x < w {
        for lane in LANES {
            let lx = x + lane * period;
            for i in 0..4 {
                let fi = i as f32;
                for (dur, offset, toward) in [(9.0, -2.0, true), (11.0, 6.0, false)] {
                    let mut p = ((now + fi * dur / 4.0 + lane * 3.0) / dur).fract();
                    if !toward {
                        p = 1.0 - p;
                    }
                    let sc = (0.6 + 0.55 * p) * k;
                    let cy = near_top + near_h * (0.37 + 0.63 * p);
                    let cx = lx + offset * k * (0.6 + p);
                    if day {
                        list.push((TexKey::Fx(Fx::White), s.rect(cx, cy, 3.0 * sc, 5.0 * sc, FULL).tint([242, 201, 76, 255])));
                    } else {
                        let c = if toward { [255, 240, 210, 240] } else { [255, 80, 70, 230] };
                        let r = 5.0 * sc;
                        list.push((TexKey::Fx(Fx::Dot), s.rect(cx - r / 2.0, cy - r / 2.0, r, r, FULL).tint(c)));
                    }
                }
            }
        }
        x += period;
    }

    // haze where the rooftops meet the skyline keeps the city quiet
    let hz = near_top - h * 0.06;
    list.push((TexKey::Fx(Fx::Ramp), s.rect(0.0, hz, w, h * 0.12, FULL).tint(l.haze)));
    list.push((TexKey::Fx(Fx::Ramp), s.rect(0.0, hz + h * 0.12, w, h * 0.16, [0.0, 1.0, 1.0, 0.0]).tint(l.haze)));
}

/// Two dimmer copies of the floor under ours: its floor, walls and furniture, no people.
pub fn floors_below(list: &mut Vec<(TexKey, Instance)>, from: usize, c: City, v: &View, keep: impl Fn(&TexKey) -> bool) {
    let l = look(c.night);
    let ours: Vec<(TexKey, Instance)> = list[from..].iter().filter(|(k, _)| keep(k)).cloned().collect();
    // one storey: the walls plus a slab
    let storey = wall_height() * 1.35;
    let dy = v.project(0.0, 0.0, -storey).1 - v.project(0.0, 0.0, 0.0).1;
    let mut below = Vec::with_capacity(ours.len() * 2);
    for (n, shade) in [(2.0, l.shade[1]), (1.0, l.shade[0])] {
        for (key, inst) in &ours {
            let mut i = *inst;
            i.t[1] += dy * n;
            for ch in 0..3 {
                i.tint[ch] = (i.tint[ch] as f32 * shade) as u8;
            }
            below.push((key.clone(), i));
        }
    }
    list.splice(from..from, below);
}

/// The tower fades into the street haze towards the bottom of the screen.
pub fn ground_fade(list: &mut Vec<(TexKey, Instance)>, at: usize, c: City, s: &Screen) {
    let l = look(c.night);
    let g = l.ground;
    let quad = s.rect(0.0, s.h * 0.62, s.w, s.h * 0.38, FULL).tint([g[0], g[1], g[2], 210]);
    list.insert(at, (TexKey::Fx(Fx::Ramp), quad));
}

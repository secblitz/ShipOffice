//! An orbiting camera over the office. The floor and walls are real planes; people and
//! furniture are billboards standing on them, so the room turns and tilts freely while the
//! sprites keep their hand-drawn look. The default angle is exactly the classic isometric view.

use crate::office::gen::Layout;
use crate::office::{Face, TW, WH};

/// World px per tile along a tile's diagonal at the default angle (32·√2).
pub const K: f32 = TW / 2.0 * std::f32::consts::SQRT_2;
pub const DEFAULT_YAW: f32 = std::f32::consts::FRAC_PI_4;
/// 30°: the 2:1 isometric tilt.
pub const DEFAULT_PITCH: f32 = std::f32::consts::FRAC_PI_6;
pub const MIN_PITCH: f32 = 0.30;
pub const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.02;

/// Wall height in tile units: 118 px tall at the default angle.
pub fn wall_height() -> f32 {
    WH / (DEFAULT_PITCH.cos() * K)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub yaw: f32,
    pub pitch: f32,
    /// the point the room turns around (its centre), in tiles
    cx: f32,
    cy: f32,
    /// keeps the room centre where the isometric projection puts it
    off: (f32, f32),
    sin: f32,
    cos: f32,
    sp: f32,
    cp: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wall {
    /// y = 0, along x
    Back,
    /// x = 0, along y
    Left,
    /// y = H
    Front,
    /// x = W
    Right,
}

impl View {
    pub fn new(o: &Layout, yaw: f32, pitch: f32) -> Self {
        let (cx, cy) = (o.w as f32 / 2.0, o.h as f32 / 2.0);
        let off = crate::office::iso(cx, cy);
        let pitch = pitch.clamp(MIN_PITCH, MAX_PITCH);
        Self { yaw, pitch, cx, cy, off, sin: yaw.sin(), cos: yaw.cos(), sp: pitch.sin(), cp: pitch.cos() }
    }

    pub fn default_for(o: &Layout) -> Self {
        Self::new(o, DEFAULT_YAW, DEFAULT_PITCH)
    }

    /// A floor point (tiles) at height `z` (tile units) to view px.
    pub fn project(&self, gx: f32, gy: f32, z: f32) -> (f32, f32) {
        let (dx, dy) = (gx - self.cx, gy - self.cy);
        let sx = (dx * self.cos - dy * self.sin) * K;
        let d = dx * self.sin + dy * self.cos;
        (self.off.0 + sx, self.off.1 + d * self.sp * K - z * self.cp * K)
    }

    pub fn floor(&self, gx: f32, gy: f32) -> (f32, f32) {
        self.project(gx, gy, 0.0)
    }

    /// Draw order: larger is nearer. At the default angle this is `gx + gy - 1`, the same
    /// numbers the floor plan's depth hints use.
    pub fn depth(&self, gx: f32, gy: f32) -> f32 {
        std::f32::consts::SQRT_2 * ((gx - self.cx) * self.sin + (gy - self.cy) * self.cos) + self.cx + self.cy - 1.0
    }

    /// Seen from the side where the art reads mirrored.
    pub fn mirrored(&self) -> bool {
        self.sin * self.cos < 0.0
    }

    /// Whether a wall stands behind the room (drawn full) or between you and it (cut away).
    pub fn behind(&self, w: Wall) -> bool {
        match w {
            Wall::Back => self.cos > 0.0,
            Wall::Front => self.cos < 0.0,
            Wall::Left => self.sin > 0.0,
            Wall::Right => self.sin < 0.0,
        }
    }

    /// The unit square mapped onto a plane: `o` the corner, `u` and `v` its edges (3D, tiles).
    pub fn plane(&self, o: [f32; 3], u: [f32; 3], v: [f32; 3]) -> ([f32; 4], [f32; 2]) {
        let p0 = self.project(o[0], o[1], o[2]);
        let pu = self.project(o[0] + u[0], o[1] + u[1], o[2] + u[2]);
        let pv = self.project(o[0] + v[0], o[1] + v[1], o[2] + v[2]);
        ([pu.0 - p0.0, pu.1 - p0.1, pv.0 - p0.0, pv.1 - p0.1], [p0.0, p0.1])
    }

    /// A rect in a wall's own drawing space (the prototype's `wallTransform` space: 32 px per
    /// tile along the wall, screen px up from the floor) as a quad on that wall.
    pub fn wall_rect(&self, back: bool, start: f32, x: f32, y: f32, w: f32, h: f32) -> ([f32; 4], [f32; 2]) {
        let z = |ly: f32| -ly / WH * wall_height();
        let per = 1.0 / (TW / 2.0);
        if back {
            self.plane([start + x * per, 0.0, z(y)], [w * per, 0.0, 0.0], [0.0, 0.0, -h / WH * wall_height()])
        } else {
            self.plane([0.0, start - x * per, z(y)], [0.0, -w * per, 0.0], [0.0, 0.0, -h / WH * wall_height()])
        }
    }

    /// A direction in the room as it reads on screen: (toward the viewer, to the right).
    pub fn facing(&self, f: Face) -> (bool, bool) {
        let (vx, vy) = match f {
            Face::DR => (1.0, 0.0),
            Face::DL => (0.0, 1.0),
            Face::UR => (0.0, -1.0),
            Face::UL => (-1.0, 0.0),
        };
        let right = vx * self.cos - vy * self.sin;
        let toward = vx * self.sin + vy * self.cos;
        (toward >= 0.0, right >= 0.0)
    }

    /// The screen box of the whole room, walls included.
    pub fn bounds(&self, o: &Layout) -> super::Bounds {
        let (w, h, z) = (o.w as f32, o.h as f32, wall_height());
        let mut b = super::Bounds { min_x: f32::MAX, max_x: f32::MIN, min_y: f32::MAX, max_y: f32::MIN };
        for (x, y) in [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)] {
            for zz in [0.0, z] {
                let (px, py) = self.project(x, y, zz);
                b.min_x = b.min_x.min(px);
                b.max_x = b.max_x.max(px);
                b.min_y = b.min_y.min(py);
                b.max_y = b.max_y.max(py);
            }
        }
        b.min_x -= 10.0;
        b.max_x += 10.0;
        b.min_y -= 40.0;
        b.max_y += 24.0;
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::office::gen::generate;
    use crate::office::iso;

    #[test]
    fn default_angle_is_the_isometric_projection() {
        let o = generate(5);
        let v = View::default_for(&o);
        for (x, y) in [(0.0, 0.0), (3.0, 7.5), (o.w as f32, o.h as f32)] {
            let (a, b) = v.floor(x, y);
            let (c, d) = iso(x, y);
            assert!((a - c).abs() < 1e-3 && (b - d).abs() < 1e-3, "{x},{y}");
            assert!((v.depth(x, y) - (x + y - 1.0)).abs() < 1e-4);
        }
        let (_, top) = v.project(0.0, 0.0, wall_height());
        assert!((top - (iso(0.0, 0.0).1 - WH)).abs() < 1e-3);
        assert!(v.behind(Wall::Back) && v.behind(Wall::Left) && !v.mirrored());
        assert_eq!(v.facing(Face::DR), (true, true));
        assert_eq!(v.facing(Face::UL), (false, false));
        let b = v.bounds(&o);
        assert!((b.min_y - (iso(0.0, 0.0).1 - WH - 40.0)).abs() < 1e-3 && (b.max_x - (iso(o.w as f32, 0.0).0 + 10.0)).abs() < 1e-3);
    }
}

//! Drawing the office: a CPU-painted static layer (floor and walls), then every sprite in one
//! instanced GPU pass, then a canvas overlay for text.

pub mod assets;
pub mod gpu;
pub mod paint;
pub mod city;
pub mod scene;
pub mod view;

use crate::office::gen::Layout;
use bytemuck::{Pod, Zeroable};
use std::sync::Arc;

/// One textured quad: the unit square mapped by an affine transform into world pixels.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Instance {
    /// x' = m[0]·u + m[2]·v + t[0], y' = m[1]·u + m[3]·v + t[1]
    pub m: [f32; 4],
    pub t: [f32; 2],
    /// additive highlight (hover), 0 for none
    pub glow: f32,
    pub _pad: f32,
    pub uv: [f32; 4],
    /// multiplies the result; alpha fades the whole quad
    pub tint: [u8; 4],
}

impl Instance {
    /// An axis-aligned quad; `flip` mirrors it horizontally around its centre.
    pub fn rect(x: f32, y: f32, w: f32, h: f32, uv: [f32; 4], flip: bool) -> Self {
        let (m, t) = if flip { ([-w, 0.0, 0.0, h], [x + w, y]) } else { ([w, 0.0, 0.0, h], [x, y]) };
        Self { m, t, glow: 0.0, _pad: 0.0, uv, tint: [255; 4] }
    }
    pub fn affine(m: [f32; 4], t: [f32; 2], uv: [f32; 4]) -> Self {
        Self { m, t, glow: 0.0, _pad: 0.0, uv, tint: [255; 4] }
    }
    pub fn tint(mut self, rgba: [u8; 4]) -> Self {
        self.tint = rgba;
        self
    }
    pub fn alpha(mut self, a: f32) -> Self {
        self.tint[3] = (self.tint[3] as f32 * a.clamp(0.0, 1.0)).round() as u8;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fx {
    White,
    Shadow,
    Ring,
    Dot,
    Glass,
    /// transparent at the top, opaque at the bottom: haze bands and fades
    Ramp,
    /// the selection ring filled this many steps of ARC_STEPS round: how full a context is
    Arc(u8),
    /// a sheet of paper, for a desk being tidied
    Paper,
}

/// How finely the context ring fills.
pub const ARC_STEPS: u8 = 24;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TexKey {
    /// a character animation sheet, by its meta.json key
    Sheet(&'static str),
    /// a plain atlas file (props, cafe set, the empty desk)
    Atlas(&'static str),
    Fx(Fx),
    /// a texture the app paints itself (the static layer, the whiteboard), uploaded when `version` changes
    Layer(u64),
}

/// A CPU-painted texture with a version so the GPU side knows when to re-upload it.
#[derive(Debug, Clone)]
pub struct Layer {
    pub id: u64,
    pub version: u64,
    pub image: Arc<image::RgbaImage>,
}

#[derive(Debug, Clone, Default)]
pub struct DrawList {
    pub items: Vec<(TexKey, Instance)>,
    pub layers: Vec<Layer>,
    /// clear colour behind the office, linear-free sRGB bytes
    pub background: [u8; 3],
    /// the city drawn behind everything, when the backdrop is on
    pub sky: Option<city::Sky>,
}

/// What the office looks at and how close. Port of the prototype's `cam`.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub z: f32,
    pub cx: f32,
    pub cy: f32,
    pub fit: f32,
    pub home: (f32, f32),
    pub bounds: Bounds,
    /// stage size in logical px
    pub w: f32,
    pub h: f32,
    /// how far the HUD pushes the room's centre up, in world px at fit
    shift: f32,
    pads: (f32, f32),
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Bounds {
    pub min_x: f32,
    pub max_x: f32,
    pub min_y: f32,
    pub max_y: f32,
}

impl Bounds {
    /// The room's box at the default angle.
    pub fn of(o: &Layout) -> Self {
        view::View::default_for(o).bounds(o)
    }
    pub fn width(&self) -> f32 {
        self.max_x - self.min_x
    }
    pub fn height(&self) -> f32 {
        self.max_y - self.min_y
    }
}

/// The most stage px one px of a character's sheet may cover at the furthest zoom in: any more
/// and the sprite is stretched past what was drawn, and blurs. Stage px, not device px: on a
/// HiDPI screen a sheet px over two device px looks as sharp as over one on a plain screen.
const SHARP: f32 = 1.0;

impl Camera {
    pub fn new() -> Self {
        Self { z: 1.0, cx: 0.0, cy: 0.0, fit: 1.0, home: (0.0, 0.0), bounds: Bounds::default(), w: 1.0, h: 1.0, shift: 0.0, pads: (0.0, 0.0) }
    }

    /// Fits the office into the stage, leaving room for the HUD bars. `fresh` recentres.
    pub fn layout(&mut self, b: Bounds, w: f32, h: f32, pad_top: f32, pad_bot: f32, fresh: bool) {
        self.w = w.max(1.0);
        self.h = h.max(1.0);
        self.bounds = b;
        self.pads = (pad_top, pad_bot);
        self.fit = self.fit_for(b);
        self.shift = (pad_top - pad_bot) / 2.0 / self.fit;
        self.home = ((b.min_x + b.max_x) / 2.0, (b.min_y + b.max_y) / 2.0 - self.shift);
        if fresh {
            self.z = 1.0;
            (self.cx, self.cy) = self.home;
        }
        // a bigger window lowers the furthest zoom
        self.z = self.z.min(self.max_z());
        self.clamp();
    }

    fn fit_for(&self, b: Bounds) -> f32 {
        (self.w / b.width()).min((self.h - self.pads.0 - self.pads.1).max(80.0) / b.height())
    }

    /// The room's box changed because the view turned. Zoomed in, only the limits move; at fit,
    /// the fit eases (by `k`) to frame the room's new shape.
    pub fn set_bounds(&mut self, b: Bounds, k: f32) {
        if self.z <= 1.0001 {
            self.fit += (self.fit_for(b) - self.fit) * k;
            self.shift = (self.pads.0 - self.pads.1) / 2.0 / self.fit;
        }
        let home = ((b.min_x + b.max_x) / 2.0, (b.min_y + b.max_y) / 2.0 - self.shift);
        let moved = (home.0 - self.home.0, home.1 - self.home.1);
        self.bounds = b;
        self.home = home;
        // at fit, stay centred on the room as it turns
        if self.z <= 1.0001 {
            self.cx += moved.0;
            self.cy += moved.1;
        }
        self.clamp();
    }

    pub fn scale(&self) -> f32 {
        self.fit * self.z
    }

    /// The furthest zoom in: until a person is drawn at the size their sheet was made, however
    /// big the room or the window.
    pub fn max_z(&self) -> f32 {
        (SHARP / (scene::VIDEO_SCALE * self.fit)).max(1.0)
    }

    /// The office never slides off screen: at 1x it sits at home, zoomed in it pans to its edges.
    pub fn clamp(&mut self) {
        let s = self.scale();
        let (hx, hy) = (self.w / 2.0 / s, self.h / 2.0 / s);
        let b = self.bounds;
        let c = |v: f32, lo: f32, hi: f32, half: f32, home: f32| if hi - lo <= 2.0 * half { home } else { v.clamp(lo + half, hi - half) };
        self.cx = c(self.cx, b.min_x, b.max_x, hx, self.home.0);
        self.cy = c(self.cy, b.min_y, b.max_y, hy, self.home.1);
    }

    pub fn origin(&self) -> (f32, f32) {
        let s = self.scale();
        (self.w / 2.0 - self.cx * s, self.h / 2.0 - self.cy * s)
    }

    pub fn to_world(&self, sx: f32, sy: f32) -> (f32, f32) {
        let (ox, oy) = self.origin();
        ((sx - ox) / self.scale(), (sy - oy) / self.scale())
    }

    pub fn to_screen(&self, wx: f32, wy: f32) -> (f32, f32) {
        let (ox, oy) = self.origin();
        (wx * self.scale() + ox, wy * self.scale() + oy)
    }

    pub fn zoom_at(&mut self, sx: f32, sy: f32, factor: f32) {
        let (wx, wy) = self.to_world(sx, sy);
        self.z = (self.z * factor).clamp(1.0, self.max_z());
        let s = self.scale();
        self.cx = wx - (sx - self.w / 2.0) / s;
        self.cy = wy - (sy - self.h / 2.0) / s;
        self.clamp();
    }

    pub fn pan(&mut self, dx: f32, dy: f32) {
        let s = self.scale();
        self.cx -= dx / s;
        self.cy -= dy / s;
        self.clamp();
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_furthest_zoom_stays_sharp() {
        let o = crate::office::gen::generate(3);
        for (w, h) in [(1320.0, 840.0), (3840.0, 2160.0), (640.0, 400.0)] {
            let mut c = Camera::new();
            c.layout(Bounds::of(&o), w, h, 60.0, 60.0, true);
            c.zoom_at(w / 2.0, h / 2.0, 100.0);
            let px = scene::VIDEO_SCALE * c.scale();
            assert!(px <= SHARP + 1e-4 || c.z == 1.0, "{w}x{h}: a sheet px covers {px} stage px");
            // and a usual window still zooms in a good way
            assert!(w > 2000.0 || c.z > 2.0, "{w}x{h}: only {}x", c.z);
        }
    }
}

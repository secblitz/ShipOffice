//! The office itself: a procedurally generated floor plan per project, and the little people
//! who walk around it acting out what the agents are doing.

pub mod gen;
pub mod world;

/// Tile size in world pixels: a tile is a 64×32 diamond, walls are 118 px tall.
pub const TW: f32 = 64.0;
pub const TH: f32 = 32.0;
pub const WH: f32 = 118.0;

/// Grid position to world pixels (isometric projection).
pub fn iso(gx: f32, gy: f32) -> (f32, f32) {
    ((gx - gy) * TW / 2.0, (gx + gy) * TH / 2.0)
}

/// The same small PRNG the browser prototype uses, so one seed gives one layout everywhere.
#[derive(Clone)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(seed)
    }
    /// mulberry32: uniform in [0, 1).
    pub fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x6D2B79F5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(1 | t);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        ((t ^ (t >> 14)) as f64 / 4294967296.0) as f32
    }
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next() * n as f32) as usize).min(n.saturating_sub(1))
    }
    pub fn pick<'a, T>(&mut self, a: &'a [T]) -> &'a T {
        &a[self.below(a.len())]
    }
}

/// FNV-1a over the UTF-16 units, like the prototype's `hash()`.
pub fn hash(s: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for c in s.encode_utf16() {
        h = (h ^ c as u32).wrapping_mul(16777619);
    }
    h
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// toward the viewer, to the right (screen down-right)
    DR,
    DL,
    UR,
    UL,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tile {
    pub x: i32,
    pub y: i32,
}

pub const fn tile(x: i32, y: i32) -> Tile {
    Tile { x, y }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_matches_the_prototype() {
        // mulberry32(1) in the browser starts 0.6270739405881613, 0.002735721180215478
        let mut r = Rng::new(1);
        assert!((r.next() - 0.627_073_94).abs() < 1e-6);
        assert!((r.next() - 0.002_735_72).abs() < 1e-6);
        assert_eq!(hash("a"), 0xe40c292c);
    }
}

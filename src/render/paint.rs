//! Things painted once on the CPU and shown as a single texture: the floor, the walls with their
//! windows and door, and the whiteboard. Mirrors `buildStatic` and `drawBoard` in the prototype.

use super::assets::{decode, meta, premultiply, FONT_HAND};
use super::Bounds;
use crate::office::gen::{Flat, Layout};
use crate::office::{iso, Rng, TH, TW, WH};
use image::RgbaImage;
use std::sync::OnceLock;
use tiny_skia::{Color, FillRule, FilterQuality, GradientStop, LinearGradient, Paint, PathBuilder, Pattern, Pixmap, Point, Rect, SpreadMode, Stroke, Transform};

fn rgb(c: u32) -> Color {
    Color::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, 255)
}

fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::from_rgba8(r, g, b, (a * 255.0).round() as u8)
}

fn solid(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn pixmap_of(mut img: RgbaImage) -> Pixmap {
    premultiply(&mut img);
    let (w, h) = img.dimensions();
    Pixmap::from_vec(img.into_raw(), tiny_skia::IntSize::from_wh(w, h).unwrap()).unwrap()
}

fn props_atlas() -> &'static Pixmap {
    static P: OnceLock<Pixmap> = OnceLock::new();
    P.get_or_init(|| pixmap_of(decode("props.webp").unwrap_or_else(|| RgbaImage::new(1, 1))))
}

/// The top face of a tile's art, resampled to a square so the iso matrix lays it flat.
fn floor_texture(name: &str) -> Pixmap {
    static TILES: OnceLock<RgbaImage> = OnceLock::new();
    let tiles = TILES.get_or_init(|| decode("tiles.webp").unwrap_or_else(|| RgbaImage::new(1, 1)));
    let [tx, ty, tw, _] = meta().tiles.get(name).copied().unwrap_or([0, 0, 1, 1]);
    const N: u32 = 64;
    let inset = 0.16;
    let mut out = RgbaImage::new(N, N);
    for j in 0..N {
        for i in 0..N {
            let u = inset + (1.0 - 2.0 * inset) * (i as f32 + 0.5) / N as f32;
            let v = inset + (1.0 - 2.0 * inset) * (j as f32 + 0.5) / N as f32;
            let px = (((u - v) * tw as f32 / 2.0 + tw as f32 / 2.0).round() as i64).clamp(0, tw as i64 - 1) as u32;
            let py = (((u + v) * tw as f32 / 4.0).round() as i64).clamp(0, tw as i64 - 1) as u32;
            let mut p = *tiles.get_pixel((tx + px).min(tiles.width() - 1), (ty + py).min(tiles.height() - 1));
            p[3] = 255;
            out.put_pixel(i, j, p);
        }
    }
    pixmap_of(out)
}

fn poly(pm: &mut Pixmap, pts: &[(f32, f32)], paint: &Paint, t: Transform) {
    let mut pb = PathBuilder::new();
    for (i, &(x, y)) in pts.iter().enumerate() {
        if i == 0 {
            pb.move_to(x, y)
        } else {
            pb.line_to(x, y)
        }
    }
    pb.close();
    if let Some(path) = pb.finish() {
        pm.fill_path(&path, paint, FillRule::Winding, t, None);
    }
}

fn rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, paint: &Paint, t: Transform) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pm.fill_rect(r, paint, t, None);
    }
}

fn wall(back: bool, start: f32) -> Transform {
    if back {
        let (x, y) = iso(start, 0.0);
        Transform::from_row(1.0, 0.5, 0.0, 1.0, x, y)
    } else {
        let (x, y) = iso(0.0, start);
        Transform::from_row(1.0, -0.5, 0.0, 1.0, x, y)
    }
}

/// Pixels per world px for the static layer: sharp at the current zoom, within a memory budget.
pub fn static_resolution(b: Bounds, scale_px: f32) -> f32 {
    const BUDGET: f32 = 12e6;
    scale_px.min((BUDGET / (b.width() * b.height())).sqrt()).max(0.5)
}

/// Floor, walls, windows, door and rugs, at `res` pixels per world px.
pub fn static_layer(o: &Layout, res: f32) -> RgbaImage {
    let b = Bounds::of(o);
    let (w, h) = ((b.width() * res).ceil() as u32, (b.height() * res).ceil() as u32);
    let mut pm = Pixmap::new(w.max(1), h.max(1)).unwrap();
    let base = Transform::from_row(res, 0.0, 0.0, res, -b.min_x * res, -b.min_y * res);
    let th = o.theme;
    let up = |(x, y): (f32, f32), d: f32| (x, y - d);
    let (a, bb, c, m) = (iso(0.0, 0.0), iso(o.w as f32, 0.0), iso(0.0, o.h as f32), iso(o.px as f32, 0.0));
    poly(&mut pm, &[a, m, up(m, WH), up(a, WH)], &solid(rgb(th.back)), base);
    poly(&mut pm, &[m, bb, up(bb, WH), up(m, WH)], &solid(rgb(th.accent)), base);
    poly(&mut pm, &[a, c, up(c, WH), up(a, WH)], &solid(rgb(th.left)), base);
    if let Some(g) = LinearGradient::new(Point::from_xy(c.0, 0.0), Point::from_xy(a.0, 0.0), vec![GradientStop::new(0.0, rgba(0, 0, 0, 0.16)), GradientStop::new(1.0, rgba(0, 0, 0, 0.02))], SpreadMode::Pad, Transform::identity()) {
        let mut p = Paint::default();
        p.shader = g;
        poly(&mut pm, &[a, c, up(c, WH), up(a, WH)], &p, base);
    }
    let trim = solid(rgb(th.trim));
    poly(&mut pm, &[a, bb, up(bb, 7.0), up(a, 7.0)], &trim, base);
    poly(&mut pm, &[a, c, up(c, 7.0), up(a, 7.0)], &trim, base);
    {
        let mut pb = PathBuilder::new();
        let (p0, p1, p2) = (up(c, WH), up(a, WH), up(bb, WH));
        pb.move_to(p0.0, p0.1);
        pb.line_to(p1.0, p1.1);
        pb.line_to(p2.0, p2.1);
        if let Some(path) = pb.finish() {
            pm.stroke_path(&path, &solid(rgba(255, 255, 255, 0.18)), &Stroke { width: 3.0, ..Stroke::default() }, base, None);
        }
    }
    // windows on the left wall: a dusk sky over a little skyline
    for &wy in &o.windows {
        let t = base.pre_concat(wall(false, wy + 1.8));
        let ww = 1.6 * TW / 2.0;
        rect(&mut pm, -3.0, -103.0, ww + 6.0, 76.0, &trim, t);
        if let Some(g) = LinearGradient::new(
            Point::from_xy(0.0, -100.0),
            Point::from_xy(0.0, -30.0),
            vec![GradientStop::new(0.0, rgb(0x1d2a4a)), GradientStop::new(0.7, rgb(0x6b5a8a)), GradientStop::new(1.0, rgb(0xe58b5a))],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            let mut p = Paint::default();
            p.shader = g;
            rect(&mut pm, 0.0, -100.0, ww, 70.0, &p, t);
        }
        let k0 = wy.round() as usize;
        for k in 0..6 {
            let bh = 10.0 + o.sky[(k + k0) % 24] * 26.0;
            rect(&mut pm, k as f32 * ww / 6.0, -30.0 - bh, ww / 6.0 - 1.0, bh, &solid(rgb(0x141b2c)), t);
        }
        for k in 0..10 {
            let v = o.sky[(k * 3 + k0) % 24];
            rect(&mut pm, 4.0 + v * (ww - 8.0), -36.0 - o.sky[(k + 7) % 24] * 14.0, 1.6, 1.6, &solid(rgba(255, 214, 140, 0.85)), t);
        }
        rect(&mut pm, ww / 2.0 - 1.0, -100.0, 2.0, 70.0, &trim, t);
        rect(&mut pm, 0.0, -66.0, ww, 2.0, &trim, t);
    }
    // the door
    {
        let t = base.pre_concat(wall(false, o.door.y as f32 + 1.0));
        rect(&mut pm, 4.0, -70.0, 26.0, 70.0, &trim, t);
        rect(&mut pm, 8.0, -66.0, 18.0, 66.0, &solid(rgba(0, 0, 0, 0.25)), t);
        rect(&mut pm, 23.0, -36.0, 3.0, 3.0, &solid(rgb(0xe8c27a)), t);
    }
    // the floor is one flat plane: each tile's square texture is projected with the iso matrix,
    // so neighbours meet edge to edge with no bevels, rims or slab thickness
    let isom = base.pre_concat(Transform::from_row(TW / 2.0, TH / 2.0, -TW / 2.0, TH / 2.0, 0.0, 0.0));
    let (work, lounge) = (floor_texture(th.work), floor_texture(th.lounge));
    let mut fr = Rng::new(o.seed ^ 0x51ed);
    for y in 0..o.h {
        for x in 0..o.w {
            let tex = if x > o.px { &lounge } else { &work };
            let (fx, fy) = (x as f32 - 0.004, y as f32 - 0.004);
            let s = 1.008 / tex.width() as f32;
            let mut p = Paint::default();
            p.shader = Pattern::new(tex.as_ref(), SpreadMode::Pad, FilterQuality::Bilinear, 1.0, Transform::from_row(s, 0.0, 0.0, s, fx, fy));
            // tiles overlap a hair, so hard edges leave no seams where antialiased ones would
            p.anti_alias = false;
            rect(&mut pm, fx, fy, 1.008, 1.008, &p, isom);
            let j = fr.next();
            if j < 0.35 {
                let c = if j < 0.17 { rgba(255, 255, 255, 0.035) } else { rgba(0, 0, 0, 0.045) };
                let mut p = solid(c);
                p.anti_alias = false;
                rect(&mut pm, x as f32, y as f32, 1.0, 1.0, &p, isom);
            }
        }
    }
    // grout: hairlines between tiles, a touch darker than the floor
    {
        let mut pb = PathBuilder::new();
        for x in 1..o.w {
            pb.move_to(x as f32, 0.0);
            pb.line_to(x as f32, o.h as f32);
        }
        for y in 1..o.h {
            pb.move_to(0.0, y as f32);
            pb.line_to(o.w as f32, y as f32);
        }
        if let Some(path) = pb.finish() {
            pm.stroke_path(&path, &solid(rgba(0, 0, 0, 0.16)), &Stroke { width: 1.0 / TH, ..Stroke::default() }, isom, None);
        }
    }
    for f in &o.flats {
        flat(&mut pm, f, base);
    }
    let (w, h) = (pm.width(), pm.height());
    RgbaImage::from_raw(w, h, pm.take()).unwrap()
}

fn flat(pm: &mut Pixmap, f: &Flat, base: Transform) {
    let Some(&(_, r)) = meta().items.get(f.name) else { return };
    let s = 124.0 / r[2] as f32;
    let cx = (iso(f.x, f.y + f.fy).0 + iso(f.x + f.fx, f.y).0) / 2.0;
    let by = iso(f.x + f.fx, f.y + f.fy).1 - if f.fx > 1.0 { 6.0 } else { 5.0 };
    let (w, h) = (r[2] as f32 * s, r[3] as f32 * s);
    let (x, y) = (cx - w / 2.0, by - h + 4.0);
    let mut p = Paint::default();
    // the pattern maps atlas pixels onto the destination rect
    p.shader = Pattern::new(props_atlas().as_ref(), SpreadMode::Pad, FilterQuality::Bicubic, 0.95, Transform::from_row(s, 0.0, 0.0, s, x - r[0] as f32 * s, y - r[1] as f32 * s));
    rect(pm, x, y, w, h, &p, base);
}

/// What the whiteboard shows.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardStats {
    pub elapsed: u64,
    pub working: bool,
    pub waiting: u64,
    pub tok_in: u64,
    pub tok_out: u64,
    pub cache: f32,
    pub files: u32,
    pub ctx: f32,
    /// read, edit, run, task, other
    pub tools: [u32; 5],
}

pub fn fmt_k(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

pub fn fmt_t(s: u64) -> String {
    let (h, m, x) = (s / 3600, s % 3600 / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{x:02}")
    } else {
        format!("{m:02}:{x:02}")
    }
}

const BOARD_RES: f32 = 3.0;

/// The whiteboard as an upright texture; the renderer skews it onto the back wall.
pub fn board(span: f32, st: &BoardStats) -> RgbaImage {
    let (w, h) = (((span + 6.0) * BOARD_RES).ceil() as u32, (76.0 * BOARD_RES).ceil() as u32);
    let mut pm = Pixmap::new(w.max(1), h.max(1)).unwrap();
    // local coordinates match the prototype: the frame starts at (-3, -110)
    let t = Transform::from_row(BOARD_RES, 0.0, 0.0, BOARD_RES, 3.0 * BOARD_RES, 110.0 * BOARD_RES);
    let bh = 70.0;
    rect(&mut pm, -3.0, -110.0, span + 6.0, bh + 6.0, &solid(rgb(0x9aa3ad)), t);
    rect(&mut pm, 0.0, -107.0, span, bh, &solid(rgb(0xfbfaf6)), t);
    rect(&mut pm, 0.0, -107.0 + bh - 10.0, span, 10.0, &solid(rgba(0, 0, 0, 0.04)), t);
    rect(&mut pm, 10.0, -107.0 + bh, span - 20.0, 4.0, &solid(rgb(0xd7dadf)), t);
    let mut hand = |s: &str, x: f32, y: f32, size: f32, c: u32| text(&mut pm, s, x, y, size, c, t);
    hand(&format!("SESSION {}", fmt_t(st.elapsed)), 6.0, -94.0, 12.0, 0x2c4f9e);
    if st.working {
        hand("working", span - 58.0, -94.0, 11.0, 0x2d8a5f);
    } else {
        hand(&format!("waiting {}", fmt_t(st.waiting)), span - 58.0, -94.0, 11.0, 0xc0702a);
    }
    hand(&format!("tokens {} in / {} out", fmt_k(st.tok_in), fmt_k(st.tok_out)), 6.0, -80.0, 11.0, 0x2a2f38);
    hand(&format!("cache {}%  ·  files {}", (st.cache * 100.0).round(), st.files), 6.0, -68.0, 11.0, 0x2a2f38);
    hand("ctx", 6.0, -55.0, 11.0, 0xb23a3a);
    hand(&format!("{}%", (st.ctx * 100.0).round()), 85.0, -55.0, 11.0, 0xb23a3a);
    {
        let mut pb = PathBuilder::new();
        pb.push_rect(Rect::from_xywh(24.0, -62.0, 58.0, 7.0).unwrap());
        if let Some(path) = pb.finish() {
            pm.stroke_path(&path, &solid(rgb(0xb23a3a)), &Stroke { width: 1.2, ..Stroke::default() }, t, None);
        }
        rect(&mut pm, 25.0, -61.0, 56.0 * st.ctx.clamp(0.0, 1.0), 5.0, &solid(rgba(178, 58, 58, 0.55)), t);
    }
    let max = st.tools.iter().copied().max().unwrap_or(0).max(1) as f32;
    let bx = span - 78.0;
    let cols = [0x2c4f9e, 0x5b7bd0, 0x2d8a5f, 0xc0702a, 0xb23a3a];
    for (i, (v, l)) in st.tools.iter().zip(["R", "E", "B", "T", "·"]).enumerate() {
        let hgt = 4.0 + 22.0 * *v as f32 / max;
        rect(&mut pm, bx + i as f32 * 15.0, -50.0 - hgt, 9.0, hgt, &solid(rgb(cols[i])), t);
        text(&mut pm, l, bx + i as f32 * 15.0 + 1.0, -41.0, 10.0, 0x5b6170, t);
    }
    let (w, h) = (pm.width(), pm.height());
    RgbaImage::from_raw(w, h, pm.take()).unwrap()
}

/// Handwriting: rasterises with fontdue and blends the coverage in the target's pixel grid.
fn text(pm: &mut Pixmap, s: &str, x: f32, y: f32, size: f32, color: u32, t: Transform) {
    static FONT: OnceLock<fontdue::Font> = OnceLock::new();
    let font = FONT.get_or_init(|| fontdue::Font::from_bytes(FONT_HAND, fontdue::FontSettings::default()).expect("Caveat"));
    // Caveat runs small for its em size; the prototype's 12px Caveat looks like this
    let px = size * 1.18 * t.sx;
    let (mut pen, base) = (x * t.sx + t.tx, y * t.sy + t.ty);
    let (cr, cg, cb) = ((color >> 16) as u8 as f32, (color >> 8) as u8 as f32, color as u8 as f32);
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    for ch in s.chars() {
        let (m, cov) = font.rasterize(ch, px);
        let gx = (pen + m.xmin as f32).round() as i32;
        let gy = (base - m.ymin as f32 - m.height as f32).round() as i32;
        for j in 0..m.height as i32 {
            for i in 0..m.width as i32 {
                let (px_x, px_y) = (gx + i, gy + j);
                if px_x < 0 || px_y < 0 || px_x >= w || px_y >= h {
                    continue;
                }
                let a = cov[(j * m.width as i32 + i) as usize] as f32 / 255.0;
                if a <= 0.0 {
                    continue;
                }
                let k = ((px_y * w + px_x) * 4) as usize;
                // premultiplied source-over
                data[k] = (cr * a + data[k] as f32 * (1.0 - a)) as u8;
                data[k + 1] = (cg * a + data[k + 1] as f32 * (1.0 - a)) as u8;
                data[k + 2] = (cb * a + data[k + 2] as f32 * (1.0 - a)) as u8;
                data[k + 3] = (255.0 * a + data[k + 3] as f32 * (1.0 - a)) as u8;
            }
        }
        pen += m.advance_width;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::office::gen::generate;

    #[test]
    fn paints_a_floor_and_a_board() {
        let o = generate(3);
        let img = static_layer(&o, 1.0);
        let b = Bounds::of(&o);
        if let Ok(d) = std::env::var("SHIPOFFICE_DUMP") {
            img.save(format!("{d}/static.png")).unwrap();
        }
        // the middle of the floor is opaque, the corner above the walls is empty
        let (cx, cy) = iso(o.w as f32 / 2.0, o.h as f32 / 2.0);
        assert_eq!(img.get_pixel((cx - b.min_x) as u32, (cy - b.min_y) as u32)[3], 255);
        assert_eq!(img.get_pixel(0, 0)[3], 0);
        let st = BoardStats { elapsed: 75, working: true, waiting: 0, tok_in: 12000, tok_out: 3000, cache: 0.8, files: 4, ctx: 0.3, tools: [3, 2, 1, 1, 0] };
        let bd = board(190.0, &st);
        if let Ok(d) = std::env::var("SHIPOFFICE_DUMP") {
            bd.save(format!("{d}/board.png")).unwrap();
            img.save(format!("{d}/static.png")).unwrap();
        }
        let dark = bd.pixels().filter(|p| p[2] > 100 && p[0] < 90).count();
        assert!(dark > 50, "handwriting was drawn");
    }
}

//! "Locked in": a card for social media that says how hard your agents are working right now, with
//! the office itself as the picture. Drawn on the CPU at 1080x1350 (4:5, the feed format), so the
//! file looks the same wherever it is posted.

use crate::render::assets::{premultiply, FONT_MONO, FONT_UI_MEDIUM, FONT_UI_SEMIBOLD};
use crate::render::paint::fmt_k;
use crate::session::Session;
use image::{imageops, RgbaImage};
use std::sync::OnceLock;
use tiny_skia::{Color, FillRule, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Point, RadialGradient, SpreadMode, Stroke, Transform};

pub const W: u32 = 1080;
pub const H: u32 = 1350;
pub const REPO: &str = "https://github.com/secblitz/ShipOffice";

/// What the live sessions add up to.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Stats {
    pub sessions: usize,
    /// agents on the floor right now, leads and helpers
    pub agents: usize,
    pub tools: u64,
    pub tokens: u64,
    /// seconds of the longest stretch any session has run
    pub focus: u64,
}

impl Stats {
    pub fn gather<'a>(sessions: impl IntoIterator<Item = &'a Session>) -> Self {
        let mut s = Stats::default();
        for x in sessions {
            s.sessions += 1;
            s.agents += x.live_agents().count();
            s.tools += x.tool_counts.values().map(|&n| n as u64).sum::<u64>();
            s.tokens += x.usage.total_in() + x.usage.output;
            s.focus = s.focus.max(x.last.duration_since(x.started).unwrap_or_default().as_secs());
        }
        s
    }
}

/// How locked in, out of 100. Each stat counts for less the more there is of it, so no single one
/// maxes the score: it takes a long run, a crew, real work and real tokens together.
pub fn score(s: &Stats) -> u32 {
    let f = |x: f64, k: f64| 1.0 - (-x / k).exp();
    let v = 0.30 * f(s.focus as f64 / 60.0, 75.0) + 0.25 * f(s.agents as f64, 3.0) + 0.25 * f(s.tools as f64, 120.0) + 0.20 * f(s.tokens as f64, 1.5e6);
    (v * 100.0).round().clamp(0.0, 100.0) as u32
}

pub fn tier(score: u32) -> &'static str {
    match score {
        0..20 => "Warming up",
        20..40 => "Getting into it",
        40..60 => "In the zone",
        60..80 => "Locked in",
        80..92 => "Deep in it",
        _ => "Cracked",
    }
}

pub fn duration(secs: u64) -> String {
    let (h, m) = (secs / 3600, secs % 3600 / 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m")
    }
}

/// The text to post with the card.
pub fn caption(s: &Stats) -> String {
    let n = score(s);
    let agents = if s.agents == 1 { "1 agent".to_owned() } else { format!("{} agents", s.agents) };
    format!(
        "Locked in: {n}/100, {}. {agents} on it, {} tool calls and {} tokens in {}. My agents have an office now: {REPO}",
        tier(n).to_lowercase(),
        s.tools,
        fmt_k(s.tokens),
        duration(s.focus)
    )
}

/// One of the crew, for the row of faces.
pub struct Member {
    pub face: Option<RgbaImage>,
    pub ring: [u8; 3],
}

/// The theme in use, so the card looks like the app: the stage's gradient, its panels and text.
#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub bg: [u8; 3],
    pub bg2: [u8; 3],
    pub surface: [u8; 3],
    pub raised: [u8; 3],
    pub text: [u8; 3],
    pub muted: [u8; 3],
    pub faint: [u8; 3],
    pub accent: [u8; 3],
    pub light: bool,
}
const M: f32 = 56.0;
/// where the picture of the office goes: x, y, w, h
const OFFICE: (f32, f32, f32, f32) = (M, 462.0, W as f32 - 2.0 * M, 600.0);

/// The part of a capture to show: the room's box (x0, y0, x1, y1 in its pixels) with some air
/// around it, grown to the picture's shape and kept inside the capture.
pub fn around(img: &RgbaImage, room: [f32; 4]) -> RgbaImage {
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let [x0, y0, x1, y1] = room;
    let (pad_x, pad_y) = ((x1 - x0) * 0.05, (y1 - y0) * 0.05);
    // a little more above, for the bubbles over heads and the skyline
    let (mut x0, mut y0, mut x1, mut y1) = (x0 - pad_x, y0 - pad_y * 1.8, x1 + pad_x, y1 + pad_y);
    let aspect = OFFICE.2 / OFFICE.3;
    if (x1 - x0) / (y1 - y0) < aspect {
        let grow = (y1 - y0) * aspect - (x1 - x0);
        (x0, x1) = (x0 - grow / 2.0, x1 + grow / 2.0);
    } else {
        let grow = (x1 - x0) / aspect - (y1 - y0);
        (y0, y1) = (y0 - grow / 2.0, y1 + grow / 2.0);
    }
    // at most the whole capture, slid back inside it
    let (cw, ch) = ((x1 - x0).min(iw), (y1 - y0).min(ih));
    let (cx, cy) = (((x0 + x1) / 2.0).clamp(cw / 2.0, iw - cw / 2.0), ((y0 + y1) / 2.0).clamp(ch / 2.0, ih - ch / 2.0));
    let (left, top) = ((cx - cw / 2.0).max(0.0) as u32, (cy - ch / 2.0).max(0.0) as u32);
    imageops::crop_imm(img, left, top, (cw as u32).min(img.width() - left).max(1), (ch as u32).min(img.height() - top).max(1)).to_image()
}

/// The card: score, the office, the crew and the numbers.
pub fn card(office: &RgbaImage, s: &Stats, crew: &[Member], look: &Look, date: &str) -> RgbaImage {
    let mut pm = Pixmap::new(W, H).unwrap();
    let (text_c, muted) = (c(look.text, 1.0), c(look.muted, 1.0));
    let acc = c(look.accent, 1.0);
    let (w, h) = (W as f32, H as f32);
    fill(&mut pm, &rrect(0.0, 0.0, w, h, 0.0), linear((0.0, 0.0), (0.0, h), &[(0.0, c(look.bg, 1.0)), (1.0, c(look.bg2, 1.0))]));
    // the accent glows behind the score, softer on a light theme
    let glow = if look.light { 0.1 } else { 0.12 };
    if let Some(g) = RadialGradient::new(Point::from_xy(w * 0.82, 230.0), Point::from_xy(w * 0.82, 230.0), 620.0, vec![GradientStop::new(0.0, with_alpha(acc, glow)), GradientStop::new(1.0, with_alpha(acc, 0.0))], SpreadMode::Pad, Transform::identity()) {
        fill(&mut pm, &rrect(0.0, 0.0, w, h, 0.0), g);
    }

    // header: the wordmark and the day
    let x = text(&mut pm, Font::Semi, "Ship", M, 96.0, 34.0, text_c, 0.0);
    text(&mut pm, Font::Semi, "Office", x, 96.0, 34.0, acc, 0.0);
    let dw = width(Font::Mono, date, 24.0, 0.0);
    text(&mut pm, Font::Mono, date, w - M - dw, 94.0, 24.0, muted, 0.0);

    // the score
    let n = score(s);
    text(&mut pm, Font::Mono, "HOW LOCKED IN", M, 178.0, 24.0, acc, 4.0);
    let x = text(&mut pm, Font::Semi, &n.to_string(), M - 7.0, 392.0, 260.0, text_c, -5.0);
    text(&mut pm, Font::Medium, "/100", x + 12.0, 392.0, 54.0, muted, 0.0);
    let t = tier(n);
    let tw = width(Font::Semi, t, 60.0, 0.0);
    text(&mut pm, Font::Semi, t, w - M - tw, 306.0, 60.0, acc, 0.0);
    let sub = format!("{} on {}", plural(s.agents, "agent"), plural(s.sessions, "session"));
    let sw = width(Font::Medium, &sub, 30.0, 0.0);
    text(&mut pm, Font::Medium, &sub, w - M - sw, 358.0, 30.0, muted, 0.0);
    // a bar filled to the score
    let bar = w - 2.0 * M;
    fill(&mut pm, &rrect(M, 420.0, bar, 12.0, 6.0), solid(c(look.faint, 1.0)));
    if n > 0 {
        let lit = c(look.accent.map(|v| v.saturating_add(70)), 1.0);
        fill(&mut pm, &rrect(M, 420.0, (bar * n as f32 / 100.0).max(12.0), 12.0, 6.0), linear((M, 0.0), (M + bar, 0.0), &[(0.0, acc), (1.0, lit)]));
    }

    // the office, cut to fit
    let (ox, oy, ow, oh) = (OFFICE.0, OFFICE.1, OFFICE.2, OFFICE.3);
    let pic = cover(office, ow as u32, oh as u32);
    let frame = rrect(ox, oy, ow, oh, 34.0);
    draw_image(&mut pm, &pic, ox, oy, &frame);
    stroke(&mut pm, &frame, c(look.text, 0.12), 2.0);

    // the crew's faces sit on the picture's bottom edge
    let (d, gap) = (96.0, 18.0);
    let shown = crew.len().min(7);
    // a gap the colour of the card around each face
    let k = (OFFICE.1 + OFFICE.3) / h;
    let gap_c = c([0, 1, 2].map(|i| (look.bg[i] as f32 + (look.bg2[i] as f32 - look.bg[i] as f32) * k).round() as u8), 1.0);
    for (i, m) in crew.iter().take(shown).enumerate() {
        let (cx, cy) = (ox + 34.0 + d / 2.0 + i as f32 * (d + gap), oy + oh);
        fill(&mut pm, &circle(cx, cy, d / 2.0 + 7.0), solid(gap_c));
        fill(&mut pm, &circle(cx, cy, d / 2.0), solid(Color::from_rgba8(m.ring[0], m.ring[1], m.ring[2], 255)));
        fill(&mut pm, &circle(cx, cy, d / 2.0 - 5.0), solid(c(look.raised, 1.0)));
        if let Some(f) = &m.face {
            let inner = d - 10.0;
            let f = imageops::resize(f, inner as u32, inner as u32, imageops::FilterType::Lanczos3);
            draw_image(&mut pm, &f, cx - inner / 2.0, cy - inner / 2.0, &circle(cx, cy, inner / 2.0));
        }
    }
    if crew.len() > shown {
        let (cx, cy) = (ox + 34.0 + d / 2.0 + shown as f32 * (d + gap), oy + oh);
        fill(&mut pm, &circle(cx, cy, d / 2.0 + 7.0), solid(gap_c));
        fill(&mut pm, &circle(cx, cy, d / 2.0), solid(c(look.raised, 1.0)));
        let more = format!("+{}", crew.len() - shown);
        let mw = width(Font::Semi, &more, 34.0, 0.0);
        text(&mut pm, Font::Semi, &more, cx - mw / 2.0, cy + 12.0, 34.0, text_c, 0.0);
    }

    // the numbers
    let tiles = [(s.agents.to_string(), "AGENTS"), (fmt_k(s.tools), "TOOL CALLS"), (fmt_k(s.tokens), "TOKENS"), (duration(s.focus), "FOCUS")];
    let (ty, th, tg) = (1136.0, 124.0, 16.0);
    let tw = (w - 2.0 * M - 3.0 * tg) / 4.0;
    let pad = 22.0;
    for (i, (v, l)) in tiles.iter().enumerate() {
        let tx = M + i as f32 * (tw + tg);
        let tile = rrect(tx, ty, tw, th, 22.0);
        fill(&mut pm, &tile, solid(c(look.surface, 1.0)));
        stroke(&mut pm, &tile, c(look.text, 0.06), 1.5);
        // a short accent rule ties the tiles to the score
        fill(&mut pm, &rrect(tx + pad, ty + 20.0, 22.0, 4.0, 2.0), solid(acc));
        text(&mut pm, Font::Mono, l, tx + pad, ty + 52.0, 17.0, muted, 2.0);
        // the number shrinks to fit its tile, so 1.68B and 24h 23m never spill out
        let room = tw - 2.0 * pad;
        let px = (48.0 * room / width(Font::Semi, v, 48.0, -0.5)).min(48.0);
        text(&mut pm, Font::Semi, v, tx + pad, ty + th - 22.0, px, text_c, -0.5);
    }

    // footer
    text(&mut pm, Font::Medium, "My agents have an office.", M, 1310.0, 26.0, c(look.text, 0.92), 0.0);
    let repo = REPO.trim_start_matches("https://");
    let rw = width(Font::Mono, repo, 22.0, 0.0);
    text(&mut pm, Font::Mono, repo, w - M - rw, 1309.0, 22.0, muted, 0.0);

    // the card is opaque, so premultiplied is straight
    RgbaImage::from_raw(W, H, pm.take()).unwrap()
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

fn c(rgb: [u8; 3], a: f32) -> Color {
    Color::from_rgba8(rgb[0], rgb[1], rgb[2], (a * 255.0).round() as u8)
}

fn with_alpha(mut c: Color, a: f32) -> Color {
    c.set_alpha(a);
    c
}

fn solid(c: Color) -> tiny_skia::Shader<'static> {
    tiny_skia::Shader::SolidColor(c)
}

fn linear(a: (f32, f32), b: (f32, f32), stops: &[(f32, Color)]) -> tiny_skia::Shader<'static> {
    let stops = stops.iter().map(|&(p, c)| GradientStop::new(p, c)).collect();
    LinearGradient::new(Point::from_xy(a.0, a.1), Point::from_xy(b.0, b.1), stops, SpreadMode::Pad, Transform::identity()).unwrap_or(solid(Color::TRANSPARENT))
}

fn fill(pm: &mut Pixmap, path: &Path, shader: tiny_skia::Shader) {
    let paint = Paint { shader, anti_alias: true, ..Paint::default() };
    pm.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
}

fn stroke(pm: &mut Pixmap, path: &Path, c: Color, width: f32) {
    let paint = Paint { shader: solid(c), anti_alias: true, ..Paint::default() };
    pm.stroke_path(path, &paint, &Stroke { width, ..Stroke::default() }, Transform::identity(), None);
}

fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
    let r = r.min(w / 2.0).min(h / 2.0);
    // a quarter circle as one cubic
    let k = r * 0.5523;
    let mut b = PathBuilder::new();
    b.move_to(x + r, y);
    b.line_to(x + w - r, y);
    b.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    b.line_to(x + w, y + h - r);
    b.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    b.line_to(x + r, y + h);
    b.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    b.line_to(x, y + r);
    b.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    b.close();
    b.finish().unwrap()
}

fn circle(cx: f32, cy: f32, r: f32) -> Path {
    PathBuilder::from_circle(cx, cy, r).unwrap()
}

/// Scales and crops an image to fill `w`x`h` from its centre.
fn cover(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let k = (w as f32 / img.width() as f32).max(h as f32 / img.height() as f32);
    let (sw, sh) = (((w as f32 / k).round() as u32).min(img.width()), ((h as f32 / k).round() as u32).min(img.height()));
    let cut = imageops::crop_imm(img, (img.width() - sw) / 2, (img.height() - sh) / 2, sw, sh).to_image();
    imageops::resize(&cut, w, h, imageops::FilterType::Lanczos3)
}

/// Draws an image at (x, y), clipped to `clip`.
fn draw_image(pm: &mut Pixmap, img: &RgbaImage, x: f32, y: f32, clip: &Path) {
    let mut img = img.clone();
    premultiply(&mut img);
    let (iw, ih) = img.dimensions();
    let Some(src) = Pixmap::from_vec(img.into_raw(), tiny_skia::IntSize::from_wh(iw, ih).unwrap()) else { return };
    let Some(mut mask) = Mask::new(pm.width(), pm.height()) else { return };
    mask.fill_path(clip, FillRule::Winding, true, Transform::identity());
    let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() };
    pm.draw_pixmap(0, 0, src.as_ref(), &paint, Transform::from_translate(x, y), Some(&mask));
}

#[derive(Clone, Copy)]
enum Font {
    Semi,
    Medium,
    Mono,
}

fn font(f: Font) -> &'static fontdue::Font {
    static FONTS: OnceLock<[fontdue::Font; 3]> = OnceLock::new();
    let all = FONTS.get_or_init(|| [FONT_UI_SEMIBOLD, FONT_UI_MEDIUM, FONT_MONO].map(|b| fontdue::Font::from_bytes(b, fontdue::FontSettings::default()).expect("font")));
    &all[f as usize]
}

/// The pen advance of a line of text, with kerning and extra `track` between letters.
fn width(f: Font, s: &str, px: f32, track: f32) -> f32 {
    let font = font(f);
    let mut prev = None;
    let mut w = 0.0;
    for ch in s.chars() {
        if let Some(p) = prev {
            w += font.horizontal_kern(p, ch, px).unwrap_or(0.0) + track;
        }
        w += font.metrics(ch, px).advance_width;
        prev = Some(ch);
    }
    w
}

/// Draws a line of text with its baseline at `y`; returns where the pen ends.
fn text(pm: &mut Pixmap, f: Font, s: &str, x: f32, y: f32, px: f32, c: Color, track: f32) -> f32 {
    let font = font(f);
    let c = c.to_color_u8();
    let (cr, cg, cb, ca) = (c.red() as f32, c.green() as f32, c.blue() as f32, c.alpha() as f32 / 255.0);
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let (mut pen, mut prev) = (x, None);
    for ch in s.chars() {
        if let Some(p) = prev {
            pen += font.horizontal_kern(p, ch, px).unwrap_or(0.0) + track;
        }
        let (m, cov) = font.rasterize(ch, px);
        let gx = (pen + m.xmin as f32).round() as i32;
        let gy = (y - m.ymin as f32 - m.height as f32).round() as i32;
        for j in 0..m.height as i32 {
            for i in 0..m.width as i32 {
                let (px_x, px_y) = (gx + i, gy + j);
                if px_x < 0 || px_y < 0 || px_x >= w || px_y >= h {
                    continue;
                }
                let a = cov[(j * m.width as i32 + i) as usize] as f32 / 255.0 * ca;
                if a <= 0.0 {
                    continue;
                }
                let k = ((px_y * w + px_x) * 4) as usize;
                // premultiplied source-over
                for (n, v) in [cr, cg, cb, 255.0].into_iter().enumerate() {
                    data[k + n] = (v * a + data[k + n] as f32 * (1.0 - a)).round() as u8;
                }
            }
        }
        pen += m.advance_width;
        prev = Some(ch);
    }
    pen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_score_grows_with_the_work_and_tops_out() {
        let none = Stats::default();
        let busy = Stats { sessions: 2, agents: 5, tools: 300, tokens: 4_000_000, focus: 3 * 3600 };
        let huge = Stats { sessions: 9, agents: 40, tools: 100_000, tokens: 10_000_000_000, focus: 48 * 3600 };
        assert_eq!(score(&none), 0);
        assert!(score(&busy) > 80, "a long run with a crew scores high: {}", score(&busy));
        assert_eq!(score(&huge), 100);
        assert_eq!(tier(score(&huge)), "Cracked");
        // one stat alone cannot carry it
        assert!(score(&Stats { tokens: 10_000_000_000, ..none }) <= 20);
    }

    #[test]
    fn draws_a_card() {
        let office = RgbaImage::from_pixel(1600, 1000, image::Rgba([60, 90, 140, 255]));
        let s = Stats { sessions: 2, agents: 4, tools: 312, tokens: 1_240_000, focus: 2 * 3600 + 14 * 60 };
        let crew: Vec<Member> = (0..9).map(|i| Member { face: None, ring: [200, (40 * i % 256) as u8, 90] }).collect();
        let look = Look { bg: [16, 19, 26], bg2: [11, 13, 18], surface: [24, 28, 36], raised: [34, 39, 50], text: [236, 239, 244], muted: [139, 147, 167], faint: [52, 58, 72], accent: [59, 130, 246], light: false };
        let img = card(&office, &s, &crew, &look, "Sun 18 Oct 2026");
        assert_eq!(img.dimensions(), (W, H));
        let holes: Vec<_> = img.enumerate_pixels().filter(|p| p.2[3] != 255).map(|p| (p.0, p.1, p.2[3])).take(5).collect();
        assert!(holes.is_empty(), "the card is opaque: {holes:?}");
        // the office sits in the middle of the card
        assert_eq!(img.get_pixel(540, 700).0, [60, 90, 140, 255]);
        assert!(caption(&s).contains("312 tool calls") && caption(&s).ends_with(REPO));
        if let Ok(d) = std::env::var("SHIPOFFICE_DUMP") {
            img.save(format!("{d}/locked_in.png")).unwrap();
        }
    }
}

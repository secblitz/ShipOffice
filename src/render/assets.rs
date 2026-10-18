//! The sprite atlases baked into the binary, decoded on demand.
//!
//! Character sheets are split for the GPU so the outfit swap stays exact when the sheet is
//! filtered and mipmapped: one texture holds the pixels that are never recoloured (premultiplied),
//! the other holds, per recolourable region, how much of the region's colour a pixel shows. The
//! shader adds `colour × weight` back, which is linear in the pixels, so blending texels first
//! gives the same result as recolouring first.

use image::RgbaImage;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));
}

pub const FONT_UI: &[u8] = include_bytes!("../../assets/fonts/InstrumentSans-Regular.ttf");
pub const FONT_UI_MEDIUM: &[u8] = include_bytes!("../../assets/fonts/InstrumentSans-Medium.ttf");
pub const FONT_UI_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/InstrumentSans-SemiBold.ttf");
pub const FONT_MONO: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Medium.ttf");
pub const FONT_HAND: &[u8] = include_bytes!("../../assets/fonts/Caveat-Bold.ttf");

pub fn file(name: &str) -> Option<&'static [u8]> {
    embedded::FILES.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

#[derive(Debug, Clone)]
pub struct Anim {
    pub file: String,
    pub mask: Option<String>,
    pub fw: u32,
    pub fh: u32,
    pub frames: u32,
    pub cols: u32,
}

#[derive(Debug)]
pub struct Meta {
    pub anims: HashMap<String, Anim>,
    /// prop name -> (atlas file, x, y, w, h)
    pub items: HashMap<String, (&'static str, [u32; 4])>,
    pub tiles: HashMap<String, [u32; 4]>,
    pub desk_empty: (u32, u32),
    /// atlas file -> pixel size
    pub atlas_size: HashMap<&'static str, (u32, u32)>,
}

pub fn meta() -> &'static Meta {
    static META: OnceLock<Meta> = OnceLock::new();
    META.get_or_init(|| {
        let v: Value = serde_json::from_slice(file("meta.json").expect("embedded meta.json")).expect("valid meta.json");
        let num = |x: &Value| x.as_u64().unwrap_or(0) as u32;
        let rect = |x: &Value| [num(&x[0]), num(&x[1]), num(&x[2]), num(&x[3])];
        let mut anims = HashMap::new();
        for (k, a) in v["anims"].as_object().into_iter().flatten() {
            anims.insert(
                k.clone(),
                Anim {
                    file: a["file"].as_str().unwrap_or_default().to_owned(),
                    mask: a["mask"].as_str().map(str::to_owned),
                    fw: num(&a["fw"]),
                    fh: num(&a["fh"]),
                    frames: num(&a["frames"]).max(1),
                    cols: num(&a["cols"]).max(1),
                },
            );
        }
        let mut items = HashMap::new();
        for (atlas, file) in [("props", "props.webp"), ("cafe", "cafe.webp"), ("propsBack", "props_back.webp")] {
            for (k, r) in v[atlas]["items"].as_object().into_iter().flatten() {
                items.insert(k.clone(), (file, rect(r)));
            }
        }
        let tiles = v["tiles"]["items"].as_object().into_iter().flatten().map(|(k, r)| (k.clone(), rect(r))).collect();
        let desk_empty = (num(&v["deskEmpty"]["w"]), num(&v["deskEmpty"]["h"]));
        let atlas_size = HashMap::from([
            ("props.webp", (num(&v["props"]["w"]), num(&v["props"]["h"]))),
            ("cafe.webp", (num(&v["cafe"]["w"]), num(&v["cafe"]["h"]))),
            ("props_back.webp", (num(&v["propsBack"]["w"]), num(&v["propsBack"]["h"]))),
            ("desk_empty.webp", desk_empty),
        ]);
        Meta { anims, items, tiles, desk_empty, atlas_size }
    })
}

pub fn decode(name: &str) -> Option<RgbaImage> {
    let bytes = file(name)?;
    let fmt = if name.ends_with(".png") { image::ImageFormat::Png } else { image::ImageFormat::WebP };
    image::load_from_memory_with_format(bytes, fmt).ok().map(|i| i.to_rgba8())
}

pub fn premultiply(img: &mut RgbaImage) {
    for p in img.pixels_mut() {
        let a = p[3] as u32;
        for c in 0..3 {
            p[c] = ((p[c] as u32 * a + 127) / 255) as u8;
        }
    }
}

/// Mask value / 40 -> weight channel (top, hair, pants, accent). Skin (4) is never recoloured.
fn channel(region: u8) -> Option<usize> {
    match region {
        1 => Some(0),
        2 => Some(1),
        3 => Some(2),
        5 => Some(3),
        _ => None,
    }
}

/// The largest shading factor a recoloured pixel may get relative to its region's average.
pub const MAX_SHADE: f32 = 1.7;

/// Splits a character sheet into the fixed pixels and the per-region weights described above.
pub fn split_sheet(src: &RgbaImage, mask: &image::GrayImage) -> (RgbaImage, RgbaImage) {
    let lum = |p: &image::Rgba<u8>| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0;
    let region = |x: u32, y: u32| ((mask.get_pixel(x, y)[0] as f32 / 40.0).round()) as u8;
    let (mut sum, mut cnt) = ([0f32; 6], [0u32; 6]);
    for (x, y, p) in src.enumerate_pixels() {
        let r = region(x, y) as usize;
        if r == 0 || r > 5 || p[3] < 8 {
            continue;
        }
        sum[r] += lum(p);
        cnt[r] += 1;
    }
    let reference: Vec<f32> = (0..6).map(|i| if cnt[i] > 0 { (sum[i] / cnt[i] as f32).max(0.02) } else { 1.0 }).collect();
    let mut fixed = src.clone();
    let mut weights = RgbaImage::new(src.width(), src.height());
    for (x, y, p) in src.enumerate_pixels() {
        let r = region(x, y);
        let a = p[3] as f32 / 255.0;
        match channel(r) {
            Some(c) if p[3] >= 8 => {
                let f = (lum(p) / reference[r as usize]).min(MAX_SHADE) / MAX_SHADE;
                weights.get_pixel_mut(x, y)[c] = (a * f * 255.0).round() as u8;
                let q = fixed.get_pixel_mut(x, y);
                q[0] = 0;
                q[1] = 0;
                q[2] = 0;
            }
            _ => {
                let q = fixed.get_pixel_mut(x, y);
                for k in 0..3 {
                    q[k] = (q[k] as f32 * a).round() as u8;
                }
            }
        }
    }
    (fixed, weights)
}

/// Box-filtered mip chain, level 0 first. Inputs are premultiplied, so plain averaging is right.
pub fn mips(img: &RgbaImage, levels: u32) -> Vec<RgbaImage> {
    let mut out = vec![img.clone()];
    for _ in 1..levels {
        let p = out.last().unwrap();
        if p.width() < 2 || p.height() < 2 {
            break;
        }
        let (w, h) = (p.width() / 2, p.height() / 2);
        let mut n = RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0u32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let q = p.get_pixel(2 * x + dx, 2 * y + dy);
                    for c in 0..4 {
                        acc[c] += q[c] as u32;
                    }
                }
                *n.get_pixel_mut(x, y) = image::Rgba(acc.map(|v| ((v + 2) / 4) as u8));
            }
        }
        out.push(n);
    }
    out
}

/// A character's sheet key for an animation: the hero uses the bare name, the others `_c{k}`.
pub fn sheet_key(base: &str, char_id: u8) -> String {
    if char_id == 0 {
        base.to_owned()
    } else {
        format!("{base}_c{char_id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sheet_is_embedded_and_splits() {
        let m = meta();
        assert!(m.anims.len() >= 50);
        let a = &m.anims["walk_c3"];
        let src = decode(&a.file).unwrap();
        assert_eq!(src.width(), a.fw * a.cols);
        let mask = image::load_from_memory(file(a.mask.as_ref().unwrap()).unwrap()).unwrap().to_luma8();
        let (fixed, w) = split_sheet(&src, &mask);
        // some pixels were moved into each of top, hair and pants
        for c in 0..3 {
            assert!(w.pixels().filter(|p| p[c] > 0).count() > 100, "channel {c}");
        }
        assert_eq!(fixed.dimensions(), src.dimensions());
        assert!(m.items.contains_key("pingpong") && m.items.contains_key("chairL"));
    }
}

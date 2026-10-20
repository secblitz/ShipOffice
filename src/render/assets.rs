//! The sprite atlases baked into the binary, decoded on demand.

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
    pub fw: u32,
    pub fh: u32,
    pub frames: u32,
    pub cols: u32,
    /// frames per second it was filmed at (the video sheets; the drawn ones follow the app's rate)
    pub fps: Option<f32>,
    /// a ping pong stroke: where the paddle is at the hit (frame 0), in sheet px from the frame's
    /// bottom centre, x to the right and y up
    pub paddle: Option<[f32; 2]>,
    /// world px per sheet px, for a sheet of your own (the built-in ones know theirs)
    pub scale: Option<f32>,
}

#[derive(Debug)]
pub struct Meta {
    pub anims: HashMap<String, Anim>,
    /// prop name -> (atlas file, x, y, w, h)
    pub items: HashMap<String, (&'static str, [u32; 4])>,
    pub tiles: HashMap<String, [u32; 4]>,
    pub desk_empty: (u32, u32),
    /// the scoreboard's two flip cards, [x, y, w, h] in its item's px
    pub score_cards: [[u32; 4]; 2],
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
                    fw: num(&a["fw"]),
                    fh: num(&a["fh"]),
                    frames: num(&a["frames"]).max(1),
                    cols: num(&a["cols"]).max(1),
                    fps: a["fps"].as_f64().map(|f| f as f32),
                    paddle: a["paddle"].as_array().and_then(|p| Some([p.first()?.as_f64()? as f32, p.get(1)?.as_f64()? as f32])),
                    scale: None,
                },
            );
        }
        let mut items = HashMap::new();
        for (atlas, file) in [("props", "props.webp"), ("cafe", "cafe.webp"), ("propsBack", "props_back.webp"), ("games", "games.webp"), ("signals", "signals.webp")] {
            for (k, r) in v[atlas]["items"].as_object().into_iter().flatten() {
                items.insert(k.clone(), (file, rect(r)));
            }
        }
        let tiles = v["tiles"]["items"].as_object().into_iter().flatten().map(|(k, r)| (k.clone(), rect(r))).collect();
        let desk_empty = (num(&v["deskEmpty"]["w"]), num(&v["deskEmpty"]["h"]));
        let mut atlas_size = HashMap::from([
            ("props.webp", (num(&v["props"]["w"]), num(&v["props"]["h"]))),
            ("cafe.webp", (num(&v["cafe"]["w"]), num(&v["cafe"]["h"]))),
            ("props_back.webp", (num(&v["propsBack"]["w"]), num(&v["propsBack"]["h"]))),
            ("games.webp", (num(&v["games"]["w"]), num(&v["games"]["h"]))),
            ("signals.webp", (num(&v["signals"]["w"]), num(&v["signals"]["h"]))),
            ("desk_empty.webp", desk_empty),
            ("desk_front.webp", desk_empty),
        ]);
        // your own characters and props: a prop stands where the built-in one did, at its size
        let c = crate::custom::content();
        anims.extend(c.anims.iter().cloned());
        for p in &c.props {
            let Some(&(_, r)) = items.get(&p.item) else { continue };
            let file: &'static str = Box::leak(format!("{}{}", crate::custom::PREFIX, p.file.display()).into_boxed_str());
            items.insert(format!("custom/{}", p.item), (file, [0, 0, r[2], r[3]]));
            atlas_size.insert(file, (r[2], r[3]));
        }
        let cards = &v["games"]["panels"];
        let score_cards = [rect(&cards[0]), rect(&cards[1])];
        Meta { anims, items, tiles, desk_empty, score_cards, atlas_size }
    })
}

/// A prop's art: your own when you switched it on, else the built-in one.
pub fn item(name: &str) -> Option<&'static (&'static str, [u32; 4])> {
    let m = meta();
    if crate::custom::prop_on(name) {
        if let Some(it) = m.items.get(&format!("custom/{name}")) {
            return Some(it);
        }
    }
    m.items.get(name)
}

pub fn decode(name: &str) -> Option<RgbaImage> {
    if name.starts_with(crate::custom::PREFIX) {
        let bytes = crate::custom::read(name)?;
        return image::load_from_memory(&bytes).ok().map(|i| i.to_rgba8());
    }
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
    fn every_sheet_is_embedded() {
        let m = meta();
        assert!(m.anims.len() >= 50);
        for a in m.anims.values() {
            let src = decode(&a.file).unwrap_or_else(|| panic!("{} is embedded", a.file));
            assert_eq!(src.width(), a.fw * a.cols, "{}", a.file);
        }
        assert!(m.items.contains_key("pingpong") && m.items.contains_key("chairL"));
    }
}

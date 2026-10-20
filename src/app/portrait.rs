//! Face portraits for the HUD, cut from each agent's own sprite with their outfit colours, so
//! the cards show the person you see in the office.

use crate::office::world::Palette;
use crate::render::assets::{self, decode, meta, sheet_key, split_sheet, MAX_SHADE};
use iced::widget::image::Handle;
use image::RgbaImage;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Output size in px; the HUD shows it at 22–36 logical px and the Locked in card at 86, so
/// this stays sharp on HiDPI.
const SIZE: u32 = 128;

/// The portrait for a character in a palette, cached per look.
pub fn portrait(char_id: u8, palette: &Palette) -> Option<Handle> {
    static CACHE: OnceLock<Mutex<HashMap<(u8, [[u8; 3]; 4]), Option<Handle>>>> = OnceLock::new();
    let cols = [1usize, 2, 3, 5].map(|r| palette.cols[r].unwrap_or([128, 128, 128]));
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    cache.entry((char_id, cols)).or_insert_with(|| face(char_id, &cols).map(|img| Handle::from_rgba(img.width(), img.height(), img.into_raw()))).clone()
}

/// The same face as an image, for cards drawn off screen.
pub fn face_image(char_id: u8, palette: &Palette) -> Option<RgbaImage> {
    face(char_id, &[1usize, 2, 3, 5].map(|r| palette.cols[r].unwrap_or([128, 128, 128])))
}

/// A character's first standing frame and its recolour mask, decoded from the sheet once: a
/// crowd has many outfits but only a few characters.
fn still(char_id: u8) -> Option<std::sync::Arc<(RgbaImage, Option<image::GrayImage>)>> {
    type Stills = Mutex<HashMap<u8, Option<std::sync::Arc<(RgbaImage, Option<image::GrayImage>)>>>>;
    static CACHE: OnceLock<Stills> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    cache
        .entry(char_id)
        .or_insert_with(|| {
            let a = meta().anims.get(&sheet_key("stand8", char_id)).or_else(|| meta().anims.get(&sheet_key("idle", char_id)))?;
            let sheet = decode(&a.file)?;
            let frame = image::imageops::crop_imm(&sheet, 0, 0, a.fw, a.fh).to_image();
            let mask = a.mask.as_deref().and_then(assets::file).and_then(|b| image::load_from_memory(b).ok()).map(|m| m.to_luma8());
            let mask = mask.filter(|m| m.dimensions() == sheet.dimensions()).map(|m| image::imageops::crop_imm(&m, 0, 0, a.fw, a.fh).to_image());
            Some(std::sync::Arc::new((frame, mask)))
        })
        .clone()
}

/// The standing still that faces you (the hand-drawn idle sheet for a character without one),
/// recoloured, cut to a square around the head with room to spare, so the head sits whole
/// inside a round frame.
fn face(char_id: u8, cols: &[[u8; 3]; 4]) -> Option<RgbaImage> {
    let (frame, mask) = &*still(char_id)?;
    let (frame, mask) = (frame.clone(), mask.clone());
    let img = match mask {
        Some(mask) => {
            // the same split the GPU uses, then the shader's sum done here: rgb = fixed + shade·Σ w·col
            let (fixed, weights) = split_sheet(&frame, &mask);
            let (fw, fh) = frame.dimensions();
            let mut out = RgbaImage::new(fw, fh);
            for (x, y, p) in out.enumerate_pixels_mut() {
                let (f, w) = (fixed.get_pixel(x, y), weights.get_pixel(x, y));
                let alpha = f[3] as f32 / 255.0;
                for c in 0..3 {
                    let add: f32 = (0..4).map(|k| w[k] as f32 / 255.0 * cols[k][c] as f32).sum();
                    let premul = f[c] as f32 + MAX_SHADE * add;
                    // back to straight alpha for the image widget
                    p[c] = if alpha > 0.0 { (premul / alpha).min(255.0) as u8 } else { 0 };
                }
                p[3] = f[3];
            }
            out
        }
        None => frame,
    };
    // each row's opaque span, top to bottom
    let rows: Vec<Option<(u32, u32)>> = (0..img.height())
        .map(|y| {
            let xs: Vec<u32> = (0..img.width()).filter(|&x| img.get_pixel(x, y)[3] > 40).collect();
            Some((*xs.first()?, *xs.last()?))
        })
        .collect();
    let y0 = rows.iter().position(Option::is_some)? as u32;
    let y1 = rows.iter().rposition(Option::is_some)? as u32;
    let h = (y1 - y0) as f32;
    // the neck: the narrowest row between the chin and the shoulders of a chibi figure
    let width = |y: u32| rows[y as usize].map_or(0, |(l, r)| r - l + 1);
    let lo = y0 + (h * 0.28) as u32;
    let hi = y0 + (h * 0.55) as u32;
    let neck = (lo..=hi).min_by_key(|&y| width(y)).unwrap_or(y0 + (h * 0.42) as u32);
    let (mut hx0, mut hx1) = (u32::MAX, 0);
    for y in y0..neck {
        if let Some((l, r)) = rows[y as usize] {
            hx0 = hx0.min(l);
            hx1 = hx1.max(r);
        }
    }
    let (hw, hh) = ((hx1.saturating_sub(hx0) + 1) as f32, (neck - y0) as f32);
    // a round frame clips the corners, so the square is a good deal larger than the head, and a
    // little low so a hint of the shoulders shows
    let side = hw.max(hh) * 1.3;
    let (cx, cy) = ((hx0 + hx1) as f32 / 2.0, y0 as f32 + hh / 2.0 + side * 0.08);
    let mut out = RgbaImage::new(side.ceil() as u32, side.ceil() as u32);
    image::imageops::overlay(&mut out, &img, -(cx - side / 2.0).round() as i64, -(cy - side / 2.0).round() as i64);
    Some(image::imageops::resize(&out, SIZE, SIZE, image::imageops::FilterType::Lanczos3))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::office::world::palette_for;

    #[test]
    fn every_character_has_a_face() {
        for c in 0..7u8 {
            let p = palette_for(c as u32 + 1, &[]);
            let img = face(c, &[1usize, 2, 3, 5].map(|r| p.cols[r].unwrap())).expect("a face");
            assert_eq!(img.dimensions(), (SIZE, SIZE));
            let opaque = img.pixels().filter(|p| p[3] > 128).count();
            assert!(opaque > (SIZE * SIZE / 4) as usize, "character {c}: the crop holds a head, not empty space");
            if let Ok(d) = std::env::var("SHIPOFFICE_DUMP") {
                img.save(format!("{d}/face_{c}.png")).unwrap();
            }
        }
    }
}

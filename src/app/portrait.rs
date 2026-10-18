//! Face portraits for the HUD, cut from each agent's own sprite with their outfit colours, so
//! the cards show the person you see in the office.

use crate::office::world::Palette;
use crate::render::assets::{self, decode, meta, sheet_key, split_sheet, MAX_SHADE};
use iced::widget::image::Handle;
use image::RgbaImage;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Output size in px; the HUD shows it at 22–36 logical px, so this stays sharp on HiDPI.
const SIZE: u32 = 96;

/// The portrait for a character in a palette, cached per look.
pub fn portrait(char_id: u8, palette: &Palette) -> Option<Handle> {
    static CACHE: OnceLock<Mutex<HashMap<(u8, [[u8; 3]; 4]), Option<Handle>>>> = OnceLock::new();
    let cols = [1usize, 2, 3, 5].map(|r| palette.cols[r].unwrap_or([128, 128, 128]));
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    cache.entry((char_id, cols)).or_insert_with(|| face(char_id, &cols).map(|img| Handle::from_rgba(img.width(), img.height(), img.into_raw()))).clone()
}

/// Frame 0 of the idle sheet (facing you), recoloured, cropped to the head and shoulders.
fn face(char_id: u8, cols: &[[u8; 3]; 4]) -> Option<RgbaImage> {
    let a = meta().anims.get(&sheet_key("idle", char_id))?;
    let sheet = decode(&a.file)?;
    let frame = image::imageops::crop_imm(&sheet, 0, 0, a.fw, a.fh).to_image();
    // the same split the GPU uses, then the shader's sum done here: rgb = fixed + shade·Σ w·col
    let mask = a.mask.as_deref().and_then(assets::file).and_then(|b| image::load_from_memory(b).ok()).map(|m| m.to_luma8());
    let img = match mask.filter(|m| m.dimensions() == sheet.dimensions()) {
        Some(mask) => {
            let mask = image::imageops::crop_imm(&mask, 0, 0, a.fw, a.fh).to_image();
            let (fixed, weights) = split_sheet(&frame, &mask);
            let mut out = RgbaImage::new(a.fw, a.fh);
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
    // the head is the top of the figure: a square as wide as the figure, from its top edge
    let (mut x0, mut x1, mut y0) = (u32::MAX, 0, u32::MAX);
    for (x, y, p) in img.enumerate_pixels() {
        if p[3] > 40 {
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
        }
    }
    if x0 > x1 {
        return None;
    }
    let side = ((x1 - x0) as f32 * 0.92) as u32;
    let cx = (x0 + x1) / 2;
    let left = cx.saturating_sub(side / 2);
    let top = y0.saturating_sub(side / 14);
    let side = side.min(img.width() - left).min(img.height() - top);
    let head = image::imageops::crop_imm(&img, left, top, side, side).to_image();
    Some(image::imageops::resize(&head, SIZE, SIZE, image::imageops::FilterType::Lanczos3))
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

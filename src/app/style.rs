//! The HUD's look: a palette of colour roles picked from a preset and your overrides, and the
//! widget styles built from it. Surfaces are solid and borderless; contrast does the separating.

use crate::config::Appearance;
use iced::widget::{button, container, pick_list, scrollable, slider, text_input, toggler};
use iced::{border, Background, Border, Color, Font, Shadow, Theme, Vector};
use std::sync::RwLock;

pub const UI: Font = Font::with_name("Instrument Sans");
pub const UI_MEDIUM: Font = Font { weight: iced::font::Weight::Medium, ..UI };
pub const UI_BOLD: Font = Font { weight: iced::font::Weight::Semibold, ..UI };
pub const MONO: Font = Font { weight: iced::font::Weight::Medium, ..Font::with_name("JetBrains Mono") };

/// Every colour the HUD uses, by role.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pal {
    /// stage gradient, top and bottom
    pub bg: Color,
    pub bg2: Color,
    /// panels, bars and cards
    pub surface: Color,
    /// hovered and selected things inside a surface
    pub raised: Color,
    pub text: Color,
    pub muted: Color,
    /// tracks, dividers, empty swatches
    pub faint: Color,
    pub accent: Color,
    pub working: Color,
    pub waiting: Color,
    pub danger: Color,
    pub info: Color,
    pub light: bool,
}

/// The roles you can change in Settings, in display order.
pub const ROLES: [(&str, &str); 7] = [
    ("accent", "Accent"),
    ("bg", "Background"),
    ("surface", "Panels"),
    ("text", "Text"),
    ("muted", "Secondary text"),
    ("working", "Working"),
    ("waiting", "Needs you"),
];

pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub pal: Pal,
}

const fn hex(c: u32) -> Color {
    Color { r: ((c >> 16) & 0xff) as f32 / 255.0, g: ((c >> 8) & 0xff) as f32 / 255.0, b: (c & 0xff) as f32 / 255.0, a: 1.0 }
}

#[allow(clippy::too_many_arguments)]
const fn pal(bg: u32, bg2: u32, surface: u32, raised: u32, text: u32, muted: u32, faint: u32, accent: u32, working: u32, waiting: u32, danger: u32, info: u32, light: bool) -> Pal {
    Pal { bg: hex(bg), bg2: hex(bg2), surface: hex(surface), raised: hex(raised), text: hex(text), muted: hex(muted), faint: hex(faint), accent: hex(accent), working: hex(working), waiting: hex(waiting), danger: hex(danger), info: hex(info), light }
}

pub const PRESETS: [Preset; 8] = [
    Preset { id: "graphite", name: "Graphite", pal: pal(0x161618, 0x0c0c0d, 0x1e1e21, 0x2c2c31, 0xf5f5f6, 0xa7a7b0, 0x3d3d44, 0x5b8cff, 0x34d399, 0xfbbf24, 0xf87171, 0x60a5fa, false) },
    Preset { id: "midnight", name: "Midnight", pal: pal(0x0e1426, 0x070a14, 0x161d33, 0x222b48, 0xeef1ff, 0xa1abcf, 0x2f3a5e, 0x8b7bff, 0x4ade80, 0xfacc15, 0xfb7185, 0x7dd3fc, false) },
    Preset { id: "paper", name: "Paper", pal: pal(0xf1efea, 0xe2ded5, 0xffffff, 0xf0ede6, 0x17171a, 0x5c5c66, 0xdcd7cc, 0x2f6bff, 0x0f9f6e, 0xb7700a, 0xd14343, 0x2563eb, true) },
    Preset { id: "forest", name: "Forest", pal: pal(0x101a15, 0x08100c, 0x17231d, 0x22332a, 0xeef6f0, 0x9fb7a8, 0x2e4237, 0x3ddc84, 0x3ddc84, 0xf5c451, 0xff7a7a, 0x7cc4ff, false) },
    Preset { id: "ember", name: "Ember", pal: pal(0x19130f, 0x0e0a08, 0x231b16, 0x33271f, 0xfcf4ed, 0xbca898, 0x46372d, 0xff7a3d, 0x6ee7a8, 0xffc857, 0xff6b6b, 0x7fb7ff, false) },
    Preset { id: "rose", name: "Rose", pal: pal(0x1a1117, 0x0f090d, 0x24171f, 0x34222d, 0xfdf0f6, 0xc6a6b8, 0x48303e, 0xff5c9a, 0x5eead4, 0xfcd34d, 0xff6b6b, 0x93c5fd, false) },
    Preset { id: "snow", name: "Snow", pal: pal(0xedf1f6, 0xdde3ec, 0xffffff, 0xeef2f7, 0x0f172a, 0x4f5d75, 0xd3dae5, 0x7c3aed, 0x059669, 0xc2410c, 0xdc2626, 0x2563eb, true) },
    Preset { id: "contrast", name: "High contrast", pal: pal(0x000000, 0x000000, 0x0f0f0f, 0x2a2a2a, 0xffffff, 0xd8d8d8, 0x555555, 0xffd60a, 0x00e676, 0xffd60a, 0xff5252, 0x40c4ff, false) },
];

/// Quick accent picks shown as swatches.
pub const ACCENTS: [u32; 8] = [0x5b8cff, 0x8b7bff, 0x2dd4bf, 0x3ddc84, 0xffd60a, 0xff7a3d, 0xff5c9a, 0xf5f5f6];

static CURRENT: RwLock<Pal> = RwLock::new(PRESETS[0].pal);

pub fn p() -> Pal {
    CURRENT.read().map(|p| *p).unwrap_or(PRESETS[0].pal)
}

pub fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(s, 16).ok().map(hex)
}

pub fn to_hex(c: Color) -> String {
    let [r, g, b, _] = c.into_rgba8();
    format!("#{r:02x}{g:02x}{b:02x}")
}

pub fn preset(id: &str) -> &'static Preset {
    PRESETS.iter().find(|p| p.id == id).unwrap_or(&PRESETS[0])
}

/// Builds the palette from the settings and makes it current.
pub fn apply(a: &Appearance) {
    let mut pal = preset(&a.theme).pal;
    for (k, v) in &a.colors {
        let Some(c) = parse_hex(v) else { continue };
        match k.as_str() {
            "accent" => pal.accent = c,
            "bg" => {
                pal.bg = c;
                pal.bg2 = mix(c, Color::BLACK, if luminance(c) > 0.5 { 0.08 } else { 0.4 });
            }
            "surface" => {
                pal.surface = c;
                pal.raised = mix(c, if luminance(c) > 0.5 { Color::BLACK } else { Color::WHITE }, 0.08);
            }
            "text" => pal.text = c,
            "muted" => pal.muted = c,
            "working" => pal.working = c,
            "waiting" => pal.waiting = c,
            _ => {}
        }
    }
    pal.light = luminance(pal.surface) > 0.5;
    if let Ok(mut cur) = CURRENT.write() {
        *cur = pal;
    }
}

/// The value of a role in the current palette, for the settings swatches.
pub fn role(pal: &Pal, key: &str) -> Color {
    match key {
        "accent" => pal.accent,
        "bg" => pal.bg,
        "surface" => pal.surface,
        "text" => pal.text,
        "muted" => pal.muted,
        "working" => pal.working,
        "waiting" => pal.waiting,
        _ => pal.text,
    }
}

pub fn luminance(c: Color) -> f32 {
    let f = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b)
}

pub fn mix(a: Color, b: Color, t: f32) -> Color {
    Color { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }
}

pub fn alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

/// Text that reads on top of `c`: near-black on bright colours, white otherwise.
pub fn on(c: Color) -> Color {
    if luminance(c) > 0.42 {
        hex(0x111114)
    } else {
        Color::WHITE
    }
}

pub fn rgb8(c: [u8; 3]) -> Color {
    Color::from_rgb8(c[0], c[1], c[2])
}

pub fn theme() -> Theme {
    let p = p();
    Theme::custom("ShipOffice", iced::theme::Palette { background: p.bg, text: p.text, primary: p.accent, success: p.working, warning: p.waiting, danger: p.danger })
}

fn shadow(strength: f32) -> Shadow {
    Shadow { color: alpha(Color::BLACK, if p().light { 0.12 * strength } else { 0.4 * strength }), offset: Vector::new(0.0, 6.0 * strength), blur_radius: 24.0 * strength }
}

/// A floating bar or panel.
pub fn surface(radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        let p = p();
        container::Style { background: Some(Background::Color(p.surface)), border: border::rounded(radius), shadow: shadow(0.6), text_color: Some(p.text), ..Default::default() }
    }
}

/// A card that hangs over the office: the same surface with a deeper shadow.
pub fn card(_: &Theme) -> container::Style {
    let p = p();
    container::Style { background: Some(Background::Color(p.surface)), border: border::rounded(16), shadow: shadow(1.2), text_color: Some(p.text), ..Default::default() }
}

/// A quiet inset block inside a surface.
pub fn inset(_: &Theme) -> container::Style {
    let p = p();
    container::Style { background: Some(Background::Color(p.raised)), border: border::rounded(10), ..Default::default() }
}

pub fn fill(c: Color, radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style { background: Some(Background::Color(c)), border: border::rounded(radius), ..Default::default() }
}

pub fn dot(c: Color) -> impl Fn(&Theme) -> container::Style {
    fill(c, 999.0)
}

pub fn ring(c: Color, selected: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(c)),
        border: Border { color: if selected { p().text } else { Color::TRANSPARENT }, width: 2.0, radius: 999.0.into() },
        ..Default::default()
    }
}

pub fn tinted(c: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style { background: Some(Background::Color(alpha(c, 0.16))), border: border::rounded(999), text_color: Some(c), ..Default::default() }
}

fn hot(status: button::Status) -> bool {
    matches!(status, button::Status::Hovered | button::Status::Pressed)
}

/// Buttons inside a surface: flat until hovered, `raised` when on.
pub fn flat(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let p = p();
        let bg = if on {
            p.raised
        } else if hot(status) {
            alpha(p.raised, 0.7)
        } else {
            Color::TRANSPARENT
        };
        button::Style { background: Some(Background::Color(bg)), text_color: if on || hot(status) { p.text } else { p.muted }, border: border::rounded(9), ..Default::default() }
    }
}

/// The one strong button in a group.
pub fn primary(_: &Theme, status: button::Status) -> button::Style {
    let p = p();
    let bg = if hot(status) { mix(p.accent, on(p.accent), 0.1) } else { p.accent };
    button::Style { background: Some(Background::Color(bg)), text_color: on(p.accent), border: border::rounded(9), ..Default::default() }
}

/// Secondary actions: a filled chip in the raised colour.
pub fn secondary(_: &Theme, status: button::Status) -> button::Style {
    let p = p();
    let bg = if hot(status) { mix(p.raised, p.text, 0.08) } else { p.raised };
    button::Style { background: Some(Background::Color(bg)), text_color: p.text, border: border::rounded(9), ..Default::default() }
}

/// Window controls; close turns red on hover like every desktop expects.
pub fn window_button(close: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let p = p();
        let bg = match (hot(status), close) {
            (true, true) => hex(0xe5484d),
            (true, false) => p.raised,
            _ => Color::TRANSPARENT,
        };
        button::Style { background: Some(Background::Color(bg)), text_color: if hot(status) { p.text } else { p.muted }, border: border::rounded(8), ..Default::default() }
    }
}

/// A preset card in Settings; the chosen one gets an accent outline.
pub fn preset_card(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let p = p();
        button::Style {
            background: Some(Background::Color(if hot(status) && !selected { p.raised } else { Color::TRANSPARENT })),
            text_color: p.text,
            border: Border { color: if selected { p.accent } else { alpha(p.faint, 0.0) }, width: 2.0, radius: 12.0.into() },
            ..Default::default()
        }
    }
}

pub fn hidden_scroll(_: &Theme, _: scrollable::Status) -> scrollable::Style {
    let rail = scrollable::Rail { background: None, border: Border::default(), scroller: scrollable::Scroller { background: Background::Color(Color::TRANSPARENT), border: Border::default() } };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll { background: Background::Color(Color::TRANSPARENT), border: Border::default(), shadow: Shadow::default(), icon: Color::TRANSPARENT },
    }
}

pub fn thin_scroll(_: &Theme, status: scrollable::Status) -> scrollable::Style {
    let p = p();
    let active = !matches!(status, scrollable::Status::Active { .. });
    let rail = scrollable::Rail { background: None, border: Border::default(), scroller: scrollable::Scroller { background: Background::Color(alpha(p.muted, if active { 0.45 } else { 0.2 })), border: border::rounded(4) } };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll { background: Background::Color(p.surface), border: Border::default(), shadow: Shadow::default(), icon: p.text },
    }
}

pub fn toggle(_: &Theme, status: toggler::Status) -> toggler::Style {
    let p = p();
    let on_ = matches!(status, toggler::Status::Active { is_toggled: true } | toggler::Status::Hovered { is_toggled: true });
    toggler::Style {
        background: Background::Color(if on_ { p.accent } else { p.faint }),
        background_border_width: 0.0,
        background_border_color: Color::TRANSPARENT,
        foreground: Background::Color(if on_ { on(p.accent) } else { p.text }),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: Some(p.text),
        border_radius: None,
        padding_ratio: 0.16,
    }
}

pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let p = p();
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: Background::Color(p.raised),
        border: Border { color: if focused { p.accent } else { Color::TRANSPARENT }, width: 1.5, radius: 8.0.into() },
        icon: p.muted,
        placeholder: p.muted,
        value: p.text,
        selection: alpha(p.accent, 0.35),
    }
}

pub fn select(_: &Theme, status: pick_list::Status) -> pick_list::Style {
    let p = p();
    let hover = matches!(status, pick_list::Status::Hovered | pick_list::Status::Opened { .. });
    pick_list::Style {
        text_color: p.text,
        placeholder_color: p.muted,
        handle_color: p.muted,
        background: Background::Color(if hover { mix(p.raised, p.text, 0.06) } else { p.raised }),
        border: border::rounded(8),
    }
}

pub fn menu(_: &Theme) -> iced::overlay::menu::Style {
    let p = p();
    iced::overlay::menu::Style {
        background: Background::Color(p.surface),
        border: Border { color: p.faint, width: 1.0, radius: 8.0.into() },
        text_color: p.text,
        selected_text_color: on(p.accent),
        selected_background: Background::Color(p.accent),
        shadow: shadow(1.0),
    }
}

pub fn range(_: &Theme, status: slider::Status) -> slider::Style {
    let p = p();
    let hot = !matches!(status, slider::Status::Active);
    slider::Style {
        rail: slider::Rail { backgrounds: (Background::Color(p.accent), Background::Color(p.faint)), width: 4.0, border: border::rounded(2) },
        handle: slider::Handle { shape: slider::HandleShape::Circle { radius: if hot { 8.0 } else { 7.0 } }, background: Background::Color(p.text), border_width: 0.0, border_color: Color::TRANSPARENT },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text must read on every preset's panels (WCAG AA for body text, 3:1 for the secondary).
    #[test]
    fn presets_keep_contrast() {
        let ratio = |a: Color, b: Color| {
            let (x, y) = (luminance(a) + 0.05, luminance(b) + 0.05);
            x.max(y) / x.min(y)
        };
        for pr in &PRESETS {
            let p = pr.pal;
            assert!(ratio(p.text, p.surface) >= 12.0, "{} text", pr.id);
            assert!(ratio(p.muted, p.surface) >= 4.5, "{} muted", pr.id);
            assert!(ratio(on(p.accent), p.accent) >= 4.5, "{} accent label", pr.id);
        }
    }
}

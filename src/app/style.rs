//! Design tokens and widget styles for the HUD: dark glass panels over the office.

use iced::widget::{button, container, scrollable, slider, text_input, toggler};
use iced::{border, color, Background, Border, Color, Font, Shadow, Theme, Vector};

pub const INK: Color = color!(0x141a26);
pub const PANEL: Color = color!(0x1c2433);
pub const PANEL2: Color = color!(0x243044);
pub const LINE: Color = color!(0x313d55);
pub const CREAM: Color = color!(0xf2e8d5);
pub const MUTED: Color = color!(0x98a3b8);
pub const EMBER: Color = color!(0xe07a3f);
pub const TEAL: Color = color!(0x4fb3a6);
pub const AMBER: Color = color!(0xf0b54a);
pub const ROSE: Color = color!(0xe2626b);
pub const SKY: Color = color!(0x6f9bd8);

pub const UI: Font = Font::with_name("Instrument Sans");
pub const UI_MEDIUM: Font = Font { weight: iced::font::Weight::Medium, ..UI };
pub const UI_BOLD: Font = Font { weight: iced::font::Weight::Semibold, ..UI };
pub const MONO: Font = Font { weight: iced::font::Weight::Medium, ..Font::with_name("JetBrains Mono") };

pub fn alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

pub fn rgb8(c: [u8; 3]) -> Color {
    Color::from_rgb8(c[0], c[1], c[2])
}

pub fn theme() -> Theme {
    Theme::custom("ShipOffice", iced::theme::Palette { background: INK, text: CREAM, primary: TEAL, success: TEAL, warning: AMBER, danger: ROSE })
}

const GLASS: Color = Color { r: 18.0 / 255.0, g: 24.0 / 255.0, b: 36.0 / 255.0, a: 0.82 };
const EDGE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.08 };

/// The floating bar and panel look.
pub fn glass(radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(GLASS)),
        border: Border { color: EDGE, width: 1.0, radius: radius.into() },
        text_color: Some(CREAM),
        ..Default::default()
    }
}

/// Cards that float over the office, with a soft drop shadow.
pub fn card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color { a: 0.94, ..color!(0x101520) })),
        border: Border { color: alpha(Color::WHITE, 0.09), width: 1.0, radius: 14.0.into() },
        shadow: Shadow { color: alpha(Color::BLACK, 0.45), offset: Vector::new(0.0, 18.0), blur_radius: 40.0 },
        text_color: Some(CREAM),
        ..Default::default()
    }
}

pub fn pill(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color { a: 0.74, ..GLASS })),
        border: Border { color: alpha(Color::WHITE, 0.07), width: 1.0, radius: 999.0.into() },
        text_color: Some(CREAM),
        ..Default::default()
    }
}

pub fn tinted(c: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(alpha(c, 0.16))),
        border: border::rounded(999),
        text_color: Some(c),
        ..Default::default()
    }
}

pub fn dot(c: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style { background: Some(Background::Color(c)), border: border::rounded(999), ..Default::default() }
}

pub fn bar_track(_: &Theme) -> container::Style {
    container::Style { background: Some(Background::Color(LINE)), border: border::rounded(2), ..Default::default() }
}

pub fn well(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(alpha(Color::WHITE, 0.03))),
        border: Border { color: alpha(Color::WHITE, 0.05), width: 1.0, radius: 10.0.into() },
        ..Default::default()
    }
}

/// A tab: quiet until selected.
pub fn tab(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(Background::Color(if selected {
                PANEL2
            } else if hover {
                alpha(Color::WHITE, 0.04)
            } else {
                Color::TRANSPARENT
            })),
            text_color: if selected || hover { CREAM } else { MUTED },
            border: Border { color: if selected { LINE } else { Color::TRANSPARENT }, width: 1.0, radius: 10.0.into() },
            ..Default::default()
        }
    }
}

/// Small segmented or icon buttons inside a glass bar.
pub fn seg(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(Background::Color(if on {
                PANEL2
            } else if hover {
                alpha(Color::WHITE, 0.05)
            } else {
                Color::TRANSPARENT
            })),
            text_color: if on || hover { CREAM } else { MUTED },
            border: border::rounded(8),
            ..Default::default()
        }
    }
}

pub fn ghost(_: &Theme, status: button::Status) -> button::Style {
    let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(if hover { alpha(Color::WHITE, 0.06) } else { GLASS })),
        text_color: CREAM,
        border: Border { color: EDGE, width: 1.0, radius: 10.0.into() },
        ..Default::default()
    }
}

pub fn primary(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |t, status| {
        if on {
            button::Style { background: Some(Background::Color(TEAL)), text_color: color!(0x0d1b1a), border: border::rounded(10), ..Default::default() }
        } else {
            ghost(t, status)
        }
    }
}

/// Window controls; close turns red on hover like every desktop expects.
pub fn window_button(close: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(Background::Color(match (hover, close) {
                (true, true) => color!(0xc4443f),
                (true, false) => alpha(Color::WHITE, 0.08),
                _ => Color::TRANSPARENT,
            })),
            text_color: if hover { CREAM } else { MUTED },
            border: border::rounded(8),
            ..Default::default()
        }
    }
}

/// An agent chip in the crew dock.
pub fn chip(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(Background::Color(if selected {
                PANEL2
            } else if hover {
                alpha(Color::WHITE, 0.05)
            } else {
                Color::TRANSPARENT
            })),
            text_color: CREAM,
            border: Border { color: if selected { LINE } else { Color::TRANSPARENT }, width: 1.0, radius: 10.0.into() },
            ..Default::default()
        }
    }
}

pub fn hidden_scroll(_: &Theme, _: scrollable::Status) -> scrollable::Style {
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller { background: Background::Color(Color::TRANSPARENT), border: Border::default() },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: Background::Color(Color::TRANSPARENT),
            border: Border::default(),
            shadow: Shadow::default(),
            icon: Color::TRANSPARENT,
        },
    }
}

pub fn thin_scroll(_: &Theme, status: scrollable::Status) -> scrollable::Style {
    let active = !matches!(status, scrollable::Status::Active { .. });
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller { background: Background::Color(alpha(Color::WHITE, if active { 0.18 } else { 0.08 })), border: border::rounded(4) },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: Background::Color(PANEL),
            border: Border::default(),
            shadow: Shadow::default(),
            icon: CREAM,
        },
    }
}

pub fn toggle(_: &Theme, status: toggler::Status) -> toggler::Style {
    let on = matches!(status, toggler::Status::Active { is_toggled: true } | toggler::Status::Hovered { is_toggled: true });
    toggler::Style {
        background: Background::Color(if on { TEAL } else { LINE }),
        background_border_width: 0.0,
        background_border_color: Color::TRANSPARENT,
        foreground: Background::Color(if on { color!(0x0d1b1a) } else { MUTED }),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: Some(CREAM),
        border_radius: None,
        padding_ratio: 0.18,
    }
}

pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: Background::Color(alpha(Color::WHITE, 0.04)),
        border: Border { color: if focused { TEAL } else { LINE }, width: 1.0, radius: 8.0.into() },
        icon: MUTED,
        placeholder: alpha(MUTED, 0.7),
        value: CREAM,
        selection: alpha(TEAL, 0.35),
    }
}

pub fn range(_: &Theme, status: slider::Status) -> slider::Style {
    let hot = !matches!(status, slider::Status::Active);
    slider::Style {
        rail: slider::Rail { backgrounds: (Background::Color(TEAL), Background::Color(LINE)), width: 4.0, border: border::rounded(2) },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius: if hot { 8.0 } else { 7.0 } },
            background: Background::Color(CREAM),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
        },
    }
}

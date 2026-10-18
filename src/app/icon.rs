//! Line icons drawn with the canvas (crisp at any scale, no icon font), and the app icon.

use super::Message;
use iced::mouse::Cursor;
use iced::widget::canvas::{self, Frame, Geometry, LineCap, Path, Stroke};
use iced::{Color, Element, Point, Rectangle, Renderer, Size, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Minimize,
    Maximize,
    Close,
    Settings,
    Plus,
    Minus,
    Fit,
    Follow,
    Left,
    Right,
    Shuffle,
    Feed,
    TurnLeft,
    TurnRight,
    Reset,
    Dice,
    Check,
    Chevron(bool),
}

struct Draw {
    icon: Icon,
    color: Color,
    /// radians, for icons that spin as feedback
    turn: f32,
}

impl canvas::Program<Message> for Draw {
    type State = ();

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, _: Cursor) -> Vec<Geometry> {
        let mut f = Frame::new(renderer, bounds.size());
        let s = bounds.width.min(bounds.height);
        let p = |x: f32, y: f32| Point::new(x * s, y * s);
        let stroke = Stroke::default().with_width((s / 11.0).max(1.2)).with_color(self.color).with_line_cap(LineCap::Round);
        let line = |f: &mut Frame, a: Point, b: Point| f.stroke(&Path::line(a, b), stroke);
        if self.turn != 0.0 {
            f.translate(iced::Vector::new(s / 2.0, s / 2.0));
            f.rotate(self.turn);
            f.translate(iced::Vector::new(-s / 2.0, -s / 2.0));
        }
        match self.icon {
            Icon::Dice => {
                // a die showing five: reads as "roll a new one"
                f.stroke(&Path::rounded_rectangle(p(0.2, 0.2), Size::new(0.6 * s, 0.6 * s), (0.14 * s).into()), stroke);
                for (x, y) in [(0.36, 0.36), (0.64, 0.36), (0.5, 0.5), (0.36, 0.64), (0.64, 0.64)] {
                    f.fill(&Path::circle(p(x, y), s * 0.055), self.color);
                }
            }
            Icon::Check => {
                line(&mut f, p(0.26, 0.52), p(0.43, 0.68));
                line(&mut f, p(0.43, 0.68), p(0.75, 0.33));
            }
            Icon::Minimize => line(&mut f, p(0.25, 0.55), p(0.75, 0.55)),
            Icon::Maximize => f.stroke(&Path::rectangle(p(0.27, 0.27), Size::new(0.46 * s, 0.46 * s)), stroke),
            Icon::Close => {
                line(&mut f, p(0.28, 0.28), p(0.72, 0.72));
                line(&mut f, p(0.72, 0.28), p(0.28, 0.72));
            }
            Icon::Plus => {
                line(&mut f, p(0.25, 0.5), p(0.75, 0.5));
                line(&mut f, p(0.5, 0.25), p(0.5, 0.75));
            }
            Icon::Minus => line(&mut f, p(0.25, 0.5), p(0.75, 0.5)),
            Icon::Fit => {
                for (x, y, dx, dy) in [(0.22, 0.22, 1.0, 1.0), (0.78, 0.22, -1.0, 1.0), (0.22, 0.78, 1.0, -1.0), (0.78, 0.78, -1.0, -1.0)] {
                    line(&mut f, p(x, y), p(x + dx * 0.18, y));
                    line(&mut f, p(x, y), p(x, y + dy * 0.18));
                }
            }
            Icon::Settings => {
                // three sliders: reads as "settings" without a gear's fiddly teeth
                for (i, k) in [0.3f32, 0.68, 0.45].iter().enumerate() {
                    let y = 0.28 + i as f32 * 0.22;
                    line(&mut f, p(0.2, y), p(0.8, y));
                    f.fill(&Path::circle(p(*k, y), s * 0.075), self.color);
                }
            }
            Icon::Follow => {
                f.stroke(&Path::circle(p(0.5, 0.5), s * 0.24), stroke);
                f.fill(&Path::circle(p(0.5, 0.5), s * 0.08), self.color);
                for (a, b) in [((0.5, 0.08), (0.5, 0.2)), ((0.5, 0.8), (0.5, 0.92)), ((0.08, 0.5), (0.2, 0.5)), ((0.8, 0.5), (0.92, 0.5))] {
                    line(&mut f, p(a.0, a.1), p(b.0, b.1));
                }
            }
            Icon::Left | Icon::Right => {
                let d = if self.icon == Icon::Left { 1.0 } else { -1.0 };
                let x = 0.5 + 0.1 * d;
                line(&mut f, p(x, 0.28), p(x - 0.2 * d, 0.5));
                line(&mut f, p(x - 0.2 * d, 0.5), p(x, 0.72));
            }
            Icon::Feed => {
                for (i, w) in [0.6f32, 0.45, 0.52].iter().enumerate() {
                    let y = 0.3 + i as f32 * 0.2;
                    f.fill(&Path::circle(p(0.24, y), s * 0.045), self.color);
                    line(&mut f, p(0.36, y), p(0.36 + *w * 0.75, y));
                }
            }
            Icon::TurnLeft | Icon::TurnRight | Icon::Reset => {
                // an arc with an arrow head; reset is a full circle around a dot
                let dir = if self.icon == Icon::TurnRight { -1.0 } else { 1.0 };
                let (c, r) = (p(0.5, 0.52), s * 0.27);
                let (a0, a1) = if self.icon == Icon::Reset { (0.6f32, 5.6f32) } else { (0.9f32, 4.6f32) };
                let arc = Path::new(|b| {
                    let n = 24;
                    for i in 0..=n {
                        let a = a0 + (a1 - a0) * i as f32 / n as f32;
                        let q = Point::new(c.x + dir * r * a.cos(), c.y - r * a.sin());
                        if i == 0 {
                            b.move_to(q)
                        } else {
                            b.line_to(q)
                        }
                    }
                });
                f.stroke(&arc, stroke);
                let tip = Point::new(c.x + dir * r * a1.cos(), c.y - r * a1.sin());
                let tangent = (-dir * a1.sin(), -a1.cos());
                let n = (tangent.1, -tangent.0);
                let h = s * 0.13;
                for side in [1.0, -1.0] {
                    let q = Point::new(tip.x - tangent.0 * h + n.0 * h * side * dir, tip.y - tangent.1 * h + n.1 * h * side * dir);
                    line(&mut f, tip, q);
                }
                if self.icon == Icon::Reset {
                    f.fill(&Path::circle(c, s * 0.06), self.color);
                }
            }
            Icon::Chevron(open) => {
                if open {
                    line(&mut f, p(0.28, 0.4), p(0.5, 0.62));
                    line(&mut f, p(0.5, 0.62), p(0.72, 0.4));
                } else {
                    line(&mut f, p(0.28, 0.6), p(0.5, 0.38));
                    line(&mut f, p(0.5, 0.38), p(0.72, 0.6));
                }
            }
            Icon::Shuffle => {
                line(&mut f, p(0.2, 0.32), p(0.4, 0.32));
                line(&mut f, p(0.4, 0.32), p(0.62, 0.68));
                line(&mut f, p(0.62, 0.68), p(0.8, 0.68));
                line(&mut f, p(0.2, 0.68), p(0.4, 0.68));
                line(&mut f, p(0.4, 0.68), p(0.62, 0.32));
                line(&mut f, p(0.62, 0.32), p(0.8, 0.32));
                line(&mut f, p(0.72, 0.22), p(0.8, 0.32));
                line(&mut f, p(0.72, 0.78), p(0.8, 0.68));
            }
        }
        vec![f.into_geometry()]
    }
}

pub fn icon<'a>(icon: Icon, size: f32, color: Color) -> Element<'a, Message> {
    canvas::Canvas::new(Draw { icon, color, turn: 0.0 }).width(size).height(size).into()
}

/// An icon turned by `turn` radians around its centre.
pub fn icon_turned<'a>(icon: Icon, size: f32, color: Color, turn: f32) -> Element<'a, Message> {
    canvas::Canvas::new(Draw { icon, color, turn }).width(size).height(size).into()
}

/// The window icon, from the same artwork as the app bundles.
pub fn app_icon() -> Option<iced::window::Icon> {
    let img = image::load_from_memory_with_format(include_bytes!("../../assets/brand/icon-256.png"), image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    iced::window::icon::from_rgba(img.into_raw(), w, h).ok()
}

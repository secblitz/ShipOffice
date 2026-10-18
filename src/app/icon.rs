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
    Chevron(bool),
}

struct Draw {
    icon: Icon,
    color: Color,
}

impl canvas::Program<Message> for Draw {
    type State = ();

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, _: Cursor) -> Vec<Geometry> {
        let mut f = Frame::new(renderer, bounds.size());
        let s = bounds.width.min(bounds.height);
        let p = |x: f32, y: f32| Point::new(x * s, y * s);
        let stroke = Stroke::default().with_width((s / 11.0).max(1.2)).with_color(self.color).with_line_cap(LineCap::Round);
        let line = |f: &mut Frame, a: Point, b: Point| f.stroke(&Path::line(a, b), stroke);
        match self.icon {
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
    canvas::Canvas::new(Draw { icon, color }).width(size).height(size).into()
}

/// The window icon: a tiny isometric floor tile with a desk lamp glow.
pub fn app_icon() -> Option<iced::window::Icon> {
    const N: u32 = 64;
    let mut px = vec![0u8; (N * N * 4) as usize];
    for y in 0..N {
        for x in 0..N {
            let (u, v) = (x as f32 + 0.5 - 32.0, y as f32 + 0.5 - 32.0);
            // rounded square background
            let q = (u.abs() - 26.0).max(0.0).hypot((v.abs() - 26.0).max(0.0));
            if q > 4.0 {
                continue;
            }
            let mut c = [0x1c, 0x24, 0x33, 255];
            // the diamond floor
            let d = u.abs() / 22.0 + (v - 6.0).abs() / 11.0;
            if d <= 1.0 {
                c = if u < 0.0 { [0x4f, 0xb3, 0xa6, 255] } else { [0x3d, 0x93, 0x88, 255] };
            }
            // a back wall
            if v < 6.0 - u.abs() / 2.0 && v > -14.0 - u.abs() / 2.0 && u.abs() < 22.0 {
                c = if u < 0.0 { [0xf2, 0xe8, 0xd5, 255] } else { [0xe0, 0x7a, 0x3f, 255] };
            }
            let k = ((y * N + x) * 4) as usize;
            px[k..k + 4].copy_from_slice(&c);
        }
    }
    iced::window::icon::from_rgba(px, N, N).ok()
}

//! A small map of a big floor: the room as you see it, everyone as a dot of their colour (whoever
//! waits on you pulses), and the part the window shows. Click or drag on it to look there.

use super::style::{self, p};
use super::Message;
use crate::office::world::World;
use crate::render::view::View;
use crate::render::Camera;
use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Action, Event, Frame, Geometry, Path, Stroke};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme};

pub const W: f32 = 196.0;
pub const H: f32 = 132.0;

pub struct MiniMap<'a> {
    pub world: &'a World,
    pub view: View,
    pub cam: &'a Camera,
    pub now: f32,
}

#[derive(Default)]
pub struct State {
    dragging: bool,
}

impl MiniMap<'_> {
    /// World px of the room's four corners, as the camera sees them.
    fn corners(&self) -> [(f32, f32); 4] {
        let (w, h) = (self.world.s.layout.w as f32, self.world.s.layout.h as f32);
        [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].map(|(x, y)| self.view.floor(x, y))
    }

    /// World px -> map px, and back: the room's box fitted into the map with a margin.
    fn fit(&self) -> (f32, f32, f32) {
        let c = self.corners();
        let (x0, x1) = (c.iter().map(|p| p.0).fold(f32::MAX, f32::min), c.iter().map(|p| p.0).fold(f32::MIN, f32::max));
        let (y0, y1) = (c.iter().map(|p| p.1).fold(f32::MAX, f32::min), c.iter().map(|p| p.1).fold(f32::MIN, f32::max));
        let k = ((W - 16.0) / (x1 - x0).max(1.0)).min((H - 16.0) / (y1 - y0).max(1.0));
        (k, W / 2.0 - (x0 + x1) / 2.0 * k, H / 2.0 - (y0 + y1) / 2.0 * k)
    }

    fn to_world(&self, p: Point) -> (f32, f32) {
        let (k, ox, oy) = self.fit();
        ((p.x - ox) / k, (p.y - oy) / k)
    }
}

impl canvas::Program<Message> for MiniMap<'_> {
    type State = State;

    fn update(&self, st: &mut State, event: &Event, bounds: Rectangle, cursor: Cursor) -> Option<Action<Message>> {
        let pos = cursor.position_in(bounds);
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let p = pos?;
                st.dragging = true;
                let (x, y) = self.to_world(p);
                Some(Action::publish(Message::LookAt(x, y)).and_capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) if st.dragging => {
                let p = pos?;
                let (x, y) = self.to_world(p);
                Some(Action::publish(Message::LookAt(x, y)).and_capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if st.dragging => {
                st.dragging = false;
                Some(Action::capture())
            }
            _ => None,
        }
    }

    fn mouse_interaction(&self, st: &State, bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        if st.dragging {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }

    fn draw(&self, _st: &State, renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: Cursor) -> Vec<Geometry> {
        let mut f = Frame::new(renderer, bounds.size());
        let pal = p();
        let (k, ox, oy) = self.fit();
        let at = |(x, y): (f32, f32)| Point::new(x * k + ox, y * k + oy);
        let room = Path::new(|b| {
            let c = self.corners();
            b.move_to(at(c[0]));
            for q in &c[1..] {
                b.line_to(at(*q));
            }
            b.close();
        });
        f.fill(&room, style::alpha(pal.raised, 0.9));
        f.stroke(&room, Stroke::default().with_width(1.0).with_color(style::alpha(pal.text, 0.25)));
        // desks, faintly, so the rows read
        for d in &self.world.s.layout.desks {
            let q = at(self.view.floor(d.x as f32 + 0.5, d.y as f32 + 0.5));
            f.fill_rectangle(Point::new(q.x - 1.0, q.y - 1.0), Size::new(2.0, 2.0), style::alpha(pal.muted, 0.45));
        }
        for a in &self.world.avatars {
            let q = at(self.view.floor(a.x + 0.5, a.y + 0.5));
            let c = style::rgb8(a.color());
            if a.needs.is_some() {
                let pulse = 0.5 + 0.5 * (self.now * 5.0).sin();
                f.fill(&Path::circle(q, 4.5 + 2.0 * pulse), style::alpha(pal.waiting, 0.35 + 0.3 * pulse));
                f.fill(&Path::circle(q, 3.0), pal.waiting);
            } else {
                let c = if a.stuck { style::alpha(c, 0.45) } else { Color { a: c.a * a.alpha, ..c } };
                f.fill(&Path::circle(q, 2.2), c);
            }
        }
        // what the window shows
        let (sw, sh) = (self.cam.w, self.cam.h);
        let tl = self.cam.to_world(0.0, 0.0);
        let br = self.cam.to_world(sw, sh);
        let (p0, p1) = (at(tl), at(br));
        let r = Path::rectangle(Point::new(p0.x.max(1.0), p0.y.max(1.0)), Size::new((p1.x.min(W - 1.0) - p0.x.max(1.0)).max(4.0), (p1.y.min(H - 1.0) - p0.y.max(1.0)).max(4.0)));
        f.stroke(&r, Stroke::default().with_width(1.5).with_color(pal.accent));
        vec![f.into_geometry()]
    }
}

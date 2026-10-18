//! The layer over the office: speech bubbles, name tags and the ping pong score, plus the
//! pointer (hover, click, drag to pan, wheel to zoom). Text stays a readable size: it grows a
//! little with zoom, never with the room.

use super::style::{self, MONO, UI, UI_BOLD};
use super::Message;
use crate::office::iso;
use crate::office::world::{BubbleKind, GameState, World};
use crate::render::assets::{FONT_MONO, FONT_UI, FONT_UI_SEMIBOLD};
use crate::render::scene::{Hit, Scene};
use crate::render::Camera;
use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Action, Event, Frame, Geometry, Path, Stroke, Text};
use iced::{alignment, Color, Font, Point, Rectangle, Renderer, Size, Theme, Vector};
use std::sync::OnceLock;

pub struct Overlay<'a> {
    pub scene: &'a Scene,
    pub world: Option<&'a World>,
    pub cam: &'a Camera,
    pub hover: Option<Hit>,
    pub selected: Option<u32>,
    pub bubbles: bool,
    pub stage: Size,
}

#[derive(Default)]
pub struct State {
    press: Option<Point>,
    last: Option<Point>,
    dragged: bool,
    hover: Option<Hit>,
}

impl canvas::Program<Message> for Overlay<'_> {
    type State = State;

    fn update(&self, st: &mut State, event: &Event, bounds: Rectangle, cursor: Cursor) -> Option<Action<Message>> {
        if bounds.size() != self.stage {
            return Some(Action::publish(Message::Stage(bounds.size())));
        }
        let pos = cursor.position_in(bounds);
        match event {
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let (Some(last), Some(p)) = (st.last, cursor.position()) {
                    if st.press.is_some() {
                        let d = p - last;
                        st.last = Some(p);
                        if st.dragged || d.x.abs() + d.y.abs() > 0.0 && st.press.is_some_and(|q| q.distance(p) > 4.0) {
                            st.dragged = true;
                            return Some(Action::publish(Message::Pan(d)).and_capture());
                        }
                        return None;
                    }
                }
                let hit = pos.and_then(|p| {
                    let (wx, wy) = self.cam.to_world(p.x, p.y);
                    self.scene.pick(wx, wy)
                });
                if hit != st.hover {
                    st.hover = hit;
                    return Some(Action::publish(Message::Hover(hit)));
                }
                None
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let p = pos?;
                let _ = p;
                st.press = cursor.position();
                st.last = cursor.position();
                st.dragged = false;
                Some(Action::capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let was = st.press.take();
                st.last = None;
                if was.is_none() || st.dragged {
                    st.dragged = false;
                    return None;
                }
                let hit = pos.and_then(|p| {
                    let (wx, wy) = self.cam.to_world(p.x, p.y);
                    self.scene.pick(wx, wy)
                });
                Some(Action::publish(Message::Click(hit)).and_capture())
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let p = pos?;
                let y = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / 60.0,
                };
                Some(Action::publish(Message::ZoomAt(p, 1.12f32.powf(y))).and_capture())
            }
            Event::Mouse(mouse::Event::CursorLeft) => {
                st.press = None;
                if st.hover.take().is_some() {
                    return Some(Action::publish(Message::Hover(None)));
                }
                None
            }
            _ => None,
        }
    }

    fn mouse_interaction(&self, st: &State, bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        if st.dragged {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(bounds) && self.hover.is_some() {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }

    fn draw(&self, _st: &State, renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: Cursor) -> Vec<Geometry> {
        let mut f = Frame::new(renderer, bounds.size());
        let Some(world) = self.world else { return vec![f.into_geometry()] };
        let ub = self.cam.fit * self.cam.z.min(1.25);
        let tag = match self.hover {
            Some(Hit::Agent(id)) => Some(id),
            _ => None,
        };
        for h in &self.scene.heads {
            let Some(a) = world.avatar(h.id) else { continue };
            let (sx, sy) = self.cam.to_screen(h.x, h.y);
            if let Some(b) = a.bubble.as_ref().filter(|_| self.bubbles) {
                let fade = (b.left / 0.35).min((b.dur - b.left) / 0.2 + 0.2).clamp(0.0, 1.0);
                bubble(&mut f, b.kind, &b.text, style::rgb8(a.color()), sx, sy, ub, fade);
            } else if Some(a.id) == tag || Some(a.id) == self.selected {
                name_tag(&mut f, &a.name, style::rgb8(a.color()), sx, sy, ub);
            }
        }
        if let Some(g) = world.s.game.as_ref().filter(|g| g.state == GameState::Playing) {
            let t = &world.s.layout.props[world.s.layout.lounge.pp_table];
            let (wx, wy) = iso(t.x as f32 + 1.0, t.y as f32 + 0.5);
            let (sx, sy) = self.cam.to_screen(wx, wy);
            let ca = world.avatar(g.a).map(|a| style::rgb8(a.color())).unwrap_or(style::MUTED);
            let cb = g.b.and_then(|b| world.avatar(b)).map(|a| style::rgb8(a.color())).unwrap_or(style::MUTED);
            score(&mut f, g.score, ca, cb, sx, sy - 66.0 * self.cam.scale(), ub);
        }
        vec![f.into_geometry()]
    }
}

fn font(bytes: &'static [u8], cell: &'static OnceLock<fontdue::Font>) -> &'static fontdue::Font {
    cell.get_or_init(|| fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).expect("embedded font"))
}

/// Width of a line of text at `size` px, from the same font files iced draws with.
pub fn measure(s: &str, size: f32, which: Font) -> f32 {
    static UI_F: OnceLock<fontdue::Font> = OnceLock::new();
    static BOLD_F: OnceLock<fontdue::Font> = OnceLock::new();
    static MONO_F: OnceLock<fontdue::Font> = OnceLock::new();
    let f = if which == MONO {
        font(FONT_MONO, &MONO_F)
    } else if which == UI_BOLD {
        font(FONT_UI_SEMIBOLD, &BOLD_F)
    } else {
        font(FONT_UI, &UI_F)
    };
    s.chars().map(|c| f.metrics(c, size).advance_width).sum()
}

/// Greedy word wrap to `max` px, at most three lines.
fn wrap(text: &str, size: f32, max: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for w in text.split_whitespace() {
        let t = if cur.is_empty() { w.to_owned() } else { format!("{cur} {w}") };
        if measure(&t, size, UI) > max && !cur.is_empty() {
            lines.push(std::mem::replace(&mut cur, w.to_owned()));
        } else {
            cur = t;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.len() > 3 {
        lines.truncate(3);
        lines[2].push('…');
    }
    lines
}

fn label(f: &mut Frame, s: &str, x: f32, baseline: f32, size: f32, color: Color, font: Font) {
    f.fill_text(Text {
        content: s.to_owned(),
        position: Point::new(x, baseline - size * 0.36),
        color,
        size: size.into(),
        font,
        align_x: iced::widget::text::Alignment::Left,
        align_y: alignment::Vertical::Center,
        ..Text::default()
    });
}

fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
    Path::rounded_rectangle(Point::new(x, y), Size::new(w, h), r.into())
}

#[allow(clippy::too_many_arguments)]
fn bubble(f: &mut Frame, kind: BubbleKind, text: &str, accent: Color, x: f32, y: f32, k: f32, fade: f32) {
    let a = |c: Color, m: f32| Color { a: c.a * m * fade, ..c };
    if kind == BubbleKind::Tool {
        let size = 9.0 * k;
        let t: String = if text.chars().count() > 34 { text.chars().take(33).chain(['…']).collect() } else { text.to_owned() };
        let w = measure(&t, size, MONO) + 12.0 * k;
        f.fill(&rounded(x - w / 2.0, y - 16.0 * k, w, 15.0 * k, 7.0 * k), a(Color::from_rgba8(20, 26, 38, 0.92), 1.0));
        f.fill_rectangle(Point::new(x - w / 2.0 + 4.0 * k, y - 11.0 * k), Size::new(3.0 * k, 5.0 * k), a(accent, 1.0));
        label(f, &t, x - w / 2.0 + 9.0 * k, y - 5.5 * k, size, a(style::CREAM, 1.0), MONO);
        return;
    }
    let size = 10.5 * k;
    let lines = wrap(text, size, 128.0 * k);
    let w = lines.iter().map(|l| measure(l, size, UI)).fold(0.0, f32::max) + 16.0 * k;
    let h = (lines.len() as f32 * 13.0 + 9.0) * k;
    let (bx, by) = (x - w / 2.0, y - h - 10.0 * k);
    let think = kind == BubbleKind::Think;
    let fill = if think { Color::from_rgb8(0xe9, 0xee, 0xf7) } else { Color::from_rgb8(0xff, 0xfa, 0xf0) };
    let body = rounded(bx, by, w, h, if think { 11.0 } else { 7.0 } * k);
    f.fill(&body, a(fill, 1.0));
    f.stroke(&body, Stroke::default().with_width(1.0).with_color(a(Color::from_rgba8(20, 26, 38, 0.25), 1.0)));
    if think {
        f.fill(&Path::circle(Point::new(x - 3.0 * k, y - 6.0 * k), 3.0 * k), a(fill, 1.0));
        f.fill(&Path::circle(Point::new(x - 6.0 * k, y - 1.0 * k), 1.8 * k), a(fill, 1.0));
    } else {
        let tail = Path::new(|p| {
            p.move_to(Point::new(x - 5.0 * k, by + h - 1.0));
            p.line_to(Point::new(x, y - 3.0 * k));
            p.line_to(Point::new(x + 4.0 * k, by + h - 1.0));
            p.close();
        });
        f.fill(&tail, a(fill, 1.0));
    }
    for (i, l) in lines.iter().enumerate() {
        label(f, l, bx + 8.0 * k, by + (15.0 + i as f32 * 13.0) * k, size, a(Color::from_rgb8(0x1d, 0x24, 0x33), 1.0), UI);
    }
}

fn name_tag(f: &mut Frame, name: &str, accent: Color, x: f32, y: f32, k: f32) {
    let size = 9.5 * k;
    let name: String = if name.chars().count() > 30 { name.chars().take(29).chain(['…']).collect() } else { name.to_owned() };
    let w = measure(&name, size, UI_BOLD) + 14.0 * k;
    f.fill(&rounded(x - w / 2.0, y - 17.0 * k, w, 14.0 * k, 7.0 * k), Color::from_rgba8(20, 26, 38, 0.9));
    f.fill(&Path::circle(Point::new(x - w / 2.0 + 6.0 * k, y - 10.0 * k), 2.2 * k), accent);
    label(f, &name, x - w / 2.0 + 10.0 * k, y - 6.5 * k, size, style::CREAM, UI_BOLD);
}

fn score(f: &mut Frame, s: [u32; 2], ca: Color, cb: Color, x: f32, y: f32, k: f32) {
    let size = 9.0 * k;
    let t = format!("{} – {}", s[0], s[1]);
    let w = measure(&t, size, MONO) + 18.0 * k;
    f.fill(&rounded(x - w / 2.0, y, w, 14.0 * k, 7.0 * k), Color::from_rgba8(20, 26, 38, 0.85));
    f.fill_rectangle(Point::new(x - w / 2.0 + 4.0 * k, y + 5.0 * k), Size::new(3.0 * k, 4.0 * k), ca);
    f.fill_rectangle(Point::new(x + w / 2.0 - 7.0 * k, y + 5.0 * k), Size::new(3.0 * k, 4.0 * k), cb);
    label(f, &t, x - w / 2.0 + 9.0 * k, y + 10.0 * k, size, style::CREAM, MONO);
}

/// Screen offset helper for cards that hang off a world point.
pub fn screen_of(cam: &Camera, wx: f32, wy: f32) -> Vector {
    let (x, y) = cam.to_screen(wx, wy);
    Vector::new(x, y)
}

//! The layer over the office: speech bubbles, name tags and the ping pong score, plus the
//! pointer (hover, click, drag to pan, wheel to zoom). Text stays a readable size: it grows a
//! little with zoom, never with the room.

use super::style::{self, p, MONO, UI, UI_BOLD};
use super::Message;
use crate::office::iso;
use crate::office::world::{BubbleKind, GameState, World};
use crate::render::assets::{FONT_MONO, FONT_UI, FONT_UI_SEMIBOLD};
use crate::render::scene::{Hit, Scene};
use crate::render::Camera;
use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Action, Event, Frame, Geometry, Path, Stroke, Text};
use iced::{alignment, Color, Font, Point, Rectangle, Renderer, Size, Theme, Vector};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

pub struct Overlay<'a> {
    pub scene: &'a Scene,
    pub world: Option<&'a World>,
    pub cam: &'a Camera,
    pub hover: Option<Hit>,
    pub selected: Option<u32>,
    pub bubbles: bool,
    pub stage: Size,
    pub view: Option<crate::render::view::View>,
}

#[derive(Default)]
pub struct State {
    press: Option<Point>,
    last: Option<Point>,
    dragged: bool,
    /// this drag turns the view (right button, or shift + left) rather than panning (left, middle)
    orbit: bool,
    /// the press was the left button (only it clicks)
    left: bool,
    shift: bool,
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
                        if st.dragged || st.press.is_some_and(|q| q.distance(p) > 4.0) {
                            st.dragged = true;
                            let m = if st.orbit { Message::Orbit(d) } else { Message::Pan(d) };
                            return Some(Action::publish(m).and_capture());
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
            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(m)) => {
                st.shift = m.shift();
                None
            }
            Event::Mouse(mouse::Event::ButtonPressed(b @ (mouse::Button::Left | mouse::Button::Right | mouse::Button::Middle))) => {
                pos?;
                st.press = cursor.position();
                st.last = cursor.position();
                st.dragged = false;
                st.left = *b == mouse::Button::Left;
                st.orbit = *b == mouse::Button::Right || (st.left && st.shift);
                Some(Action::capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Right | mouse::Button::Middle)) => {
                st.press = None;
                st.last = None;
                let turned = std::mem::take(&mut st.dragged) && st.orbit;
                turned.then(|| Action::publish(Message::OrbitEnd).and_capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let was = st.press.take();
                st.last = None;
                if was.is_none() || !st.left {
                    return None;
                }
                if st.dragged {
                    st.dragged = false;
                    return st.orbit.then(|| Action::publish(Message::OrbitEnd).and_capture());
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
                if std::mem::take(&mut st.dragged) && st.orbit {
                    return Some(Action::publish(Message::OrbitEnd));
                }
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
        // bubbles nearest the viewer keep their place; ones behind them rise until they clear
        let mut items: Vec<(f32, f32, &crate::office::world::Avatar, &crate::office::world::Bubble)> = Vec::new();
        for h in &self.scene.heads {
            let Some(a) = world.avatar(h.id).filter(|a| a.alpha > 0.3) else { continue };
            let (sx, sy) = self.cam.to_screen(h.x, h.y);
            match a.bubble.as_ref().filter(|_| self.bubbles) {
                Some(b) => items.push((sx, sy, a, b)),
                None if Some(a.id) == tag || Some(a.id) == self.selected => name_tag(&mut f, &a.name, style::rgb8(a.color()), sx, sy, ub),
                None => {}
            }
        }
        items.sort_by(|p, q| q.1.total_cmp(&p.1));
        let mut placed: Vec<Rectangle> = Vec::new();
        for (sx, sy, a, b) in items {
            // bubbles pop in: a quick ease-out-back on their first fifth of a second
            let k = ub * pop(b.dur - b.left);
            let (w, h) = bubble_size(b.kind, &b.text, k);
            let mut lift = 0.0;
            for _ in 0..8 {
                let r = Rectangle::new(Point::new(sx - w / 2.0, sy - h - lift), Size::new(w, h));
                match placed.iter().find(|q| q.intersects(&r)) {
                    Some(q) => lift = sy - h - q.y + 3.0,
                    None => break,
                }
            }
            placed.push(Rectangle::new(Point::new(sx - w / 2.0, sy - h - lift), Size::new(w, h)));
            let fade = (b.left / 0.35).min((b.dur - b.left) / 0.2 + 0.2).clamp(0.0, 1.0);
            bubble(&mut f, b.kind, &b.text, style::rgb8(a.color()), sx, sy, lift, k, fade);
        }
        if let Some(g) = world.s.game.as_ref().filter(|g| g.state == GameState::Playing) {
            let t = &world.s.layout.props[world.s.layout.lounge.pp_table];
            let (wx, wy) = self.view.map_or_else(|| iso(t.x as f32 + 1.0, t.y as f32 + 0.5), |v| v.floor(t.x as f32 + 1.0, t.y as f32 + 0.5));
            let (sx, sy) = self.cam.to_screen(wx, wy);
            let ca = world.avatar(g.a).map(|a| style::rgb8(a.color())).unwrap_or(p().muted);
            let cb = g.b.and_then(|b| world.avatar(b)).map(|a| style::rgb8(a.color())).unwrap_or(p().muted);
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
    let mut cur_w = 0.0;
    let space = measure(" ", size, UI);
    // words wider than a line are cut into pieces that fit
    let pieces = text.split_whitespace().flat_map(|w| {
        let mut out: Vec<String> = Vec::new();
        let mut piece = String::new();
        for ch in w.chars() {
            piece.push(ch);
            if piece.chars().count() > 1 && measure(&piece, size, UI) > max {
                piece.pop();
                out.push(std::mem::replace(&mut piece, ch.to_string()));
            }
        }
        out.push(piece);
        out
    });
    for w in pieces {
        let w = w.as_str();
        let ww = measure(w, size, UI);
        if cur.is_empty() {
            cur = w.to_owned();
            cur_w = ww;
        } else if cur_w + space + ww > max {
            lines.push(std::mem::replace(&mut cur, w.to_owned()));
            cur_w = ww;
        } else {
            cur.push(' ');
            cur.push_str(w);
            cur_w += space + ww;
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

type Wrapped = Rc<(Vec<String>, f32)>;

/// The wrapped lines of a speech bubble and the widest one, remembered because the text and
/// size only change while a bubble pops in.
fn wrapped(text: &str, k: f32) -> Wrapped {
    thread_local! {
        static CACHE: RefCell<HashMap<(String, u32), Wrapped>> = RefCell::new(HashMap::new());
    }
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 256 {
            c.clear();
        }
        c.entry((text.to_owned(), k.to_bits()))
            .or_insert_with(|| {
                let size = 10.5 * k;
                let lines = wrap(text, size, 128.0 * k);
                let widest = lines.iter().map(|l| measure(l, size, UI)).fold(0.0, f32::max);
                Rc::new((lines, widest))
            })
            .clone()
    })
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

/// Scale for something appearing `age` seconds ago: overshoots a hair, then settles at 1.
fn pop(age: f32) -> f32 {
    let t = (age / 0.2).clamp(0.0, 1.0);
    let (c1, c3) = (1.70158, 2.70158);
    let back = 1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2);
    0.82 + 0.18 * back
}

fn tool_text(text: &str) -> String {
    if text.chars().count() > 34 {
        text.chars().take(33).chain(['…']).collect()
    } else {
        text.to_owned()
    }
}

/// Screen size of a bubble including its tail.
fn bubble_size(kind: BubbleKind, text: &str, k: f32) -> (f32, f32) {
    if kind == BubbleKind::Tool {
        return (measure(&tool_text(text), 9.0 * k, MONO) + 12.0 * k, 17.0 * k);
    }
    let (lines, widest) = &*wrapped(text, k);
    let w = widest + 16.0 * k;
    (w, (lines.len() as f32 * 13.0 + 9.0) * k + 10.0 * k)
}

/// `lift` raises the bubble body above its natural spot; the tail still points at the head.
#[allow(clippy::too_many_arguments)]
fn bubble(f: &mut Frame, kind: BubbleKind, text: &str, accent: Color, x: f32, y0: f32, lift: f32, k: f32, fade: f32) {
    let a = |c: Color, m: f32| Color { a: c.a * m * fade, ..c };
    let y = y0 - lift;
    if kind == BubbleKind::Tool {
        let size = 9.0 * k;
        let t = tool_text(text);
        let w = measure(&t, size, MONO) + 12.0 * k;
        let pal = p();
        f.fill(&rounded(x - w / 2.0, y - 16.0 * k, w, 15.0 * k, 7.0 * k), a(style::alpha(pal.surface, 0.95), 1.0));
        f.fill_rectangle(Point::new(x - w / 2.0 + 4.0 * k, y - 11.0 * k), Size::new(3.0 * k, 5.0 * k), a(accent, 1.0));
        label(f, &t, x - w / 2.0 + 9.0 * k, y - 5.5 * k, size, a(pal.text, 1.0), MONO);
        return;
    }
    let size = 10.5 * k;
    let (lines, widest) = &*wrapped(text, k);
    let w = widest + 16.0 * k;
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
            p.line_to(Point::new(x, y0 - 3.0 * k));
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
    let pal = p();
    f.fill(&rounded(x - w / 2.0, y - 17.0 * k, w, 14.0 * k, 7.0 * k), style::alpha(pal.surface, 0.95));
    f.fill(&Path::circle(Point::new(x - w / 2.0 + 6.0 * k, y - 10.0 * k), 2.2 * k), accent);
    label(f, &name, x - w / 2.0 + 10.0 * k, y - 6.5 * k, size, pal.text, UI_BOLD);
}

fn score(f: &mut Frame, s: [u32; 2], ca: Color, cb: Color, x: f32, y: f32, k: f32) {
    let size = 9.0 * k;
    let t = format!("{} – {}", s[0], s[1]);
    let w = measure(&t, size, MONO) + 18.0 * k;
    let pal = p();
    f.fill(&rounded(x - w / 2.0, y, w, 14.0 * k, 7.0 * k), style::alpha(pal.surface, 0.95));
    f.fill_rectangle(Point::new(x - w / 2.0 + 4.0 * k, y + 5.0 * k), Size::new(3.0 * k, 4.0 * k), ca);
    f.fill_rectangle(Point::new(x + w / 2.0 - 7.0 * k, y + 5.0 * k), Size::new(3.0 * k, 4.0 * k), cb);
    label(f, &t, x - w / 2.0 + 9.0 * k, y + 10.0 * k, size, pal.text, MONO);
}

/// Screen offset helper for cards that hang off a world point.
pub fn screen_of(cam: &Camera, wx: f32, wy: f32) -> Vector {
    let (x, y) = cam.to_screen(wx, wy);
    Vector::new(x, y)
}

//! The HUD, kept to the corners so the office stays the picture: tabs and one status line top
//! left, settings and window controls top right, a single toolbar bottom centre. The crew list,
//! the live feed and the cards open on demand.

use super::icon::{icon, Icon};
use super::overlay::Overlay;
use super::style::{self, alpha, on, p, rgb8, MONO, UI_BOLD, UI_MEDIUM};
use super::{App, HarnessChoice, Message, Setting, WindowAction};
use crate::office::gen::THEMES;
use crate::office::world::SeatRef;
use crate::render::paint::{fmt_k, fmt_t};
use crate::render::scene::Hit;
use crate::session::{Session, Status};
use iced::widget::{button, canvas, column, container, mouse_area, opaque, pick_list, pin, row, scrollable, slider, stack, text, text_input, toggler, tooltip, Space};
use iced::{mouse, window, Alignment, Color, Element, Fill, Length, Padding};
use std::time::SystemTime;

type El<'a> = Element<'a, Message>;

// Advanced shaping everywhere: basic shaping drops glyphs of text that changes every frame
fn t<'a>(s: impl text::IntoFragment<'a>, size: f32, color: Color) -> text::Text<'a> {
    text(s).size(size).color(color).shaping(text::Shaping::Advanced)
}

fn mono<'a>(s: impl text::IntoFragment<'a>, size: f32, color: Color) -> text::Text<'a> {
    text(s).size(size).color(color).font(MONO).shaping(text::Shaping::Advanced)
}

fn dot<'a>(c: Color, d: f32) -> El<'a> {
    container(Space::new()).width(d).height(d).style(style::dot(c)).into()
}

fn sep<'a>() -> El<'a> {
    container(Space::new()).width(1).height(18).style(style::fill(alpha(p().faint, 0.9), 0.0)).into()
}

/// A square icon button with a tooltip, flat until hovered.
fn tool<'a>(i: Icon, tip: &'a str, msg: Message, on_: bool) -> El<'a> {
    let pal = p();
    let b = button(container(icon(i, 15.0, if on_ { pal.text } else { pal.muted })).center(Fill)).width(32).height(32).padding(0).style(style::flat(on_)).on_press(msg);
    tooltip(b, container(t(tip, 11.5, pal.text)).padding([4, 8]).style(style::surface(7.0)), tooltip::Position::Top).gap(6).into()
}

fn small_x<'a>(msg: Message) -> El<'a> {
    button(container(icon(Icon::Close, 13.0, p().muted)).center(Fill)).width(26).height(26).padding(0).style(style::flat(false)).on_press(msg).into()
}

fn ago(t: SystemTime) -> u64 {
    SystemTime::now().duration_since(t).unwrap_or_default().as_secs()
}

pub fn status_color(s: Status) -> Color {
    let pal = p();
    match s {
        Status::Working => pal.working,
        Status::NeedsYou => pal.waiting,
        Status::Idle => pal.muted,
    }
}

fn status_label(s: Status) -> &'static str {
    match s {
        Status::Working => "Working",
        Status::NeedsYou => "Needs you",
        Status::Idle => "Idle",
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        s.chars().take(n.saturating_sub(1)).chain(['…']).collect()
    } else {
        s.to_owned()
    }
}

/// What an avatar's label says, as a colour for its badge.
fn badge_color(label: &str) -> Color {
    let pal = p();
    let l = label.to_ascii_lowercase();
    if l.starts_with("reading") || l.starts_with("thinking") {
        pal.info
    } else if l.starts_with("editing") || l.starts_with("running") || l.starts_with("writing") || l == "working" {
        pal.working
    } else if l.contains("report") || l.contains("waiting for you") || l.contains("needs you") {
        pal.waiting
    } else {
        pal.muted
    }
}

fn badge<'a>(label: &str) -> El<'a> {
    let c = badge_color(label);
    container(t(clip(label, 40), 11.5, c).font(UI_MEDIUM)).padding([3, 9]).style(style::tinted(c)).into()
}

fn no_scrollbar() -> scrollable::Direction {
    scrollable::Direction::Horizontal(scrollable::Scrollbar::new().width(0).scroller_width(0))
}

impl App {
    /// How far along a card's entrance is (eased 0..1).
    fn enter(&self, key: &str) -> f32 {
        self.opened.get(key).map_or(1.0, |t| style::entrance(*t, 0.22))
    }

    /// A status dot; a working one breathes.
    fn status_dot(&self, s: Status, d: f32) -> El<'static> {
        let c = status_color(s);
        let c = if s == Status::Working {
            let t = self.started_secs();
            alpha(c, c.a * (0.55 + 0.45 * (0.5 + 0.5 * (t * 3.2).sin())))
        } else {
            c
        };
        dot(c, d)
    }

    pub fn view(&self) -> El<'_> {
        let pal = p();
        let bg = container(Space::new().width(Fill).height(Fill)).style(move |_| container::Style {
            background: Some(iced::Background::Gradient(iced::Gradient::Linear(iced::gradient::Linear::new(0.0).add_stop(0.0, pal.bg).add_stop(1.0, pal.bg2)))),
            ..Default::default()
        });
        let cfg = self.config();
        let overlay = canvas(Overlay { scene: &self.scene, world: self.tab().map(|t| &t.world), cam: &self.cam, hover: self.hover, selected: self.selected, bubbles: cfg.bubbles, stage: self.stage, view: self.tab().map(|t| self.view_of(t)) }).width(Fill).height(Fill);
        let mut layers: Vec<El> = vec![bg.into()];
        if self.tab().is_some() {
            layers.push(self.view_office());
        }
        layers.push(overlay.into());
        layers.push(self.hud(cfg.feed_open));
        layers.extend(self.inspector());
        layers.extend(self.board_card());
        if self.tabs.is_empty() {
            layers.push(self.empty_state());
        }
        if self.settings_open {
            layers.push(self.settings());
        }
        if cfg!(not(target_os = "macos")) {
            layers.push(resize_grips());
        }
        stack(layers).width(Fill).height(Fill).into()
    }

    fn hud(&self, feed_open: bool) -> El<'_> {
        let mut left = column![self.tabs_bar()].spacing(8);
        if let Some(s) = self.session() {
            left = left.push(self.status_line(s));
        }
        let top = row![left, self.grab_area(), self.window_bar()].spacing(8).align_y(Alignment::Start);
        let mut middle = row![Space::new().width(Fill)].height(Fill);
        if feed_open && self.tab().is_some() {
            let e = self.enter("feed");
            middle = middle.push(opaque(style::faded(e, || container(self.feed()).padding(Padding { left: (1.0 - e) * 16.0, ..Padding::ZERO }))));
        }
        let mut col = column![top, middle].spacing(10).padding(Padding { top: 10.0, right: 12.0, bottom: 12.0, left: 12.0 });
        if self.tab().is_some() {
            let mut bottom = column![].spacing(8).align_x(Alignment::Center).width(Fill);
            if self.crew_open {
                let e = self.enter("crew");
                bottom = bottom.push(opaque(style::faded(e, || container(self.crew_list()).padding(Padding { top: (1.0 - e) * 10.0, ..Padding::ZERO }))));
            }
            bottom = bottom.push(opaque(self.toolbar(feed_open)));
            col = col.push(bottom);
        }
        col.width(Fill).height(Fill).into()
    }

    /// Empty space at the top moves the window; a double click maximises.
    fn grab_area(&self) -> El<'_> {
        mouse_area(Space::new().width(Fill).height(44)).on_press(Message::Window(WindowAction::Drag)).on_double_click(Message::Window(WindowAction::ToggleMaximize)).into()
    }

    fn tabs_bar(&self) -> El<'_> {
        let pal = p();
        let mut r = row![].spacing(2).align_y(Alignment::Center);
        if cfg!(target_os = "macos") {
            r = r.push(Space::new().width(64)); // the traffic lights sit here
        }
        // the wordmark: two weights of one word, no glyph
        r = r.push(container(row![t("Ship", 14.0, pal.text).font(UI_BOLD), t("Office", 14.0, pal.accent).font(UI_BOLD)].spacing(0).align_y(Alignment::Center)).padding([0, 10]));
        if !self.tabs.is_empty() {
            r = r.push(sep());
        }
        let tabs = row(self.tabs.iter().enumerate().map(|(i, tab)| -> El {
            let s = self.sessions.get(&tab.key);
            let status = s.map_or(Status::Idle, |s| s.status);
            let name = s.map_or_else(|| "…".into(), |s| s.project());
            let sel = i == self.active;
            let body = row![self.status_dot(status, 7.0), t(clip(&name, 22), 13.0, if sel { pal.text } else { pal.muted }).font(if sel { UI_BOLD } else { UI_MEDIUM })].spacing(8).align_y(Alignment::Center);
            button(body).padding([7, 12]).style(style::flat(sel)).on_press(Message::SelectTab(i)).into()
        }))
        .spacing(2);
        r = r.push(scrollable(tabs).direction(no_scrollbar()).style(style::hidden_scroll));
        opaque(container(r).padding(4).height(44).align_y(Alignment::Center).style(style::surface(12.0)).max_width(900))
    }

    /// One line with what matters now; the session card has the rest.
    fn status_line(&self, s: &Session) -> El<'_> {
        let pal = p();
        let ctx = (s.context as f32 / s.context_window() as f32).clamp(0.0, 1.0);
        let ctx_color = if ctx > 0.85 { pal.danger } else { pal.muted };
        let mut r = row![
            self.status_dot(s.status, 8.0),
            t(status_label(s.status), 12.5, pal.text).font(UI_BOLD),
            mono(fmt_t(ago(s.status_since)), 11.5, pal.muted),
            t("·", 12.0, pal.muted),
            mono(format!("${:.2}", s.cost_estimate()), 11.5, pal.text),
            t("·", 12.0, pal.muted),
            mono(format!("ctx {}%", (ctx * 100.0).round()), 11.5, ctx_color),
        ]
        .spacing(7)
        .align_y(Alignment::Center);
        if self.config().show_fps {
            r = r.push(t("·", 12.0, pal.muted)).push(mono(format!("{:.0} fps", self.fps), 11.5, pal.muted));
        }
        let b = button(r).padding([7, 12]).style(style::flat(self.board_open)).on_press(Message::Click(Some(Hit::Board)));
        opaque(container(b).padding(3).style(style::surface(12.0)))
    }

    fn window_bar(&self) -> El<'_> {
        let pal = p();
        let mut r = row![tool(Icon::Settings, "Settings", Message::ToggleSettings, self.settings_open)].spacing(2).align_y(Alignment::Center);
        if cfg!(not(target_os = "macos")) {
            let wb = |i: Icon, msg: WindowAction, close: bool| -> El { button(container(icon(i, 14.0, pal.muted)).center(Fill)).width(34).height(32).padding(0).style(style::window_button(close)).on_press(Message::Window(msg)).into() };
            r = r.push(sep()).push(wb(Icon::Minimize, WindowAction::Minimize, false)).push(wb(Icon::Maximize, WindowAction::ToggleMaximize, false)).push(wb(Icon::Close, WindowAction::Close, true));
        }
        opaque(container(r).padding(6).height(44).align_y(Alignment::Center).style(style::surface(12.0)))
    }

    /// The bottom toolbar: camera, crew, feed.
    fn toolbar(&self, feed_open: bool) -> El<'_> {
        let pal = p();
        let zoom = mono(format!("{:.0}%", self.cam.z * 100.0), 11.5, pal.muted).width(44).align_x(Alignment::Center);
        let crew: Vec<El> = self.tab().map(|t| t.world.avatars.iter().take(6).map(|a| dot(rgb8(a.color()), 9.0)).collect()).unwrap_or_default();
        let n = self.tab().map_or(0, |t| t.world.avatars.len());
        let crew_btn = button(row![row(crew).spacing(3), t(format!("{n} crew"), 12.0, if self.crew_open { pal.text } else { pal.muted }).font(UI_MEDIUM)].spacing(8).align_y(Alignment::Center))
            .padding([8, 11])
            .height(32)
            .style(style::flat(self.crew_open))
            .on_press(Message::ToggleCrew);
        let r = row![
            tool(Icon::Minus, "Zoom out  −", Message::Zoom(0.8), false),
            zoom,
            tool(Icon::Plus, "Zoom in  +", Message::Zoom(1.25), false),
            tool(Icon::Fit, "Fit the office  0", Message::Fit, false),
            sep(),
            crew_btn,
            tool(Icon::Follow, "Follow the selected agent  F", Message::Follow, self.follow),
            sep(),
            tool(Icon::TurnLeft, "Turn left  Q · right-drag to orbit", Message::Turn(std::f32::consts::FRAC_PI_2), false),
            tool(Icon::TurnRight, "Turn right  E", Message::Turn(-std::f32::consts::FRAC_PI_2), false),
        ]
        .spacing(2)
        .align_y(Alignment::Center);
        let r = if self.turned() { r.push(tool(Icon::Reset, "Back to the classic angle  R", Message::ResetView, false)) } else { r };
        let r = row![
            r,
            sep(),
            tool(Icon::Feed, "Live feed", Message::ToggleFeed, feed_open),
            tool(Icon::Shuffle, "New office layout", Message::NewLayout, false),
        ]
        .spacing(2)
        .align_y(Alignment::Center);
        container(r).padding(6).style(style::surface(14.0)).into()
    }

    fn crew_list(&self) -> El<'_> {
        let pal = p();
        let Some(tab) = self.tab() else { return Space::new().into() };
        let rows = column(tab.world.avatars.iter().map(|a| -> El {
            let sel = self.selected == Some(a.id);
            let body = row![dot(rgb8(a.color()), 9.0), t(clip(&a.name, 30), 12.5, pal.text).font(UI_MEDIUM).width(Fill), t(clip(&a.label, 30), 11.5, badge_color(&a.label))].spacing(10).align_y(Alignment::Center);
            button(body).padding([7, 10]).width(Fill).style(style::flat(sel)).on_press(Message::Select(if sel { None } else { Some(a.id) })).into()
        }))
        .spacing(1);
        container(scrollable(rows).height(Length::Shrink).style(style::thin_scroll)).padding(6).width(440).max_height(300).style(style::surface(14.0)).into()
    }

    fn agent_color(&self, who: &str) -> Color {
        if who == "You" {
            return p().text;
        }
        self.tab().and_then(|t| t.world.avatars.iter().find(|a| a.name == who)).map_or(p().muted, |a| rgb8(a.color()))
    }

    fn feed(&self) -> El<'_> {
        let pal = p();
        let head = row![t("Live feed", 13.0, pal.text).font(UI_BOLD), Space::new().width(Fill), small_x(Message::ToggleFeed)].align_y(Alignment::Center);
        let mut col = column![head].spacing(6).width(340).height(Fill);
        if let Some(s) = self.session() {
            if s.feed.is_empty() {
                col = col.push(t("Events show up here as they happen.", 12.0, pal.muted));
            } else {
                let items = column(s.feed.iter().take(60).map(|f| -> El {
                    let mut top = row![dot(self.agent_color(&f.who), 6.0), t(clip(&f.who, 24), 12.0, pal.text).font(UI_MEDIUM)].spacing(7).align_y(Alignment::Center);
                    if !f.what.is_empty() {
                        top = top.push(mono(clip(&f.what, 14), 11.0, pal.accent));
                    }
                    top = top.push(Space::new().width(Fill)).push(mono(fmt_ago(ago(f.at)), 10.5, pal.muted));
                    let mut c = column![top].spacing(2);
                    if !f.detail.is_empty() {
                        c = c.push(container(t(clip(&f.detail.replace('\n', " "), 120), 12.0, pal.muted)).padding(Padding { left: 13.0, ..Padding::ZERO }));
                    }
                    container(c).padding([6, 4]).into()
                }))
                .spacing(2);
                col = col.push(scrollable(items).height(Fill).style(style::thin_scroll));
            }
        }
        container(col).padding(Padding { top: 10.0, right: 10.0, bottom: 8.0, left: 14.0 }).style(style::surface(14.0)).into()
    }

    /// Hangs a card off a stage point, flipping sides so it stays on screen.
    fn hang<'a>(&self, card: El<'a>, sx: f32, sy: f32, w: f32, h: f32, e: f32) -> El<'a> {
        let (sw, sh) = (self.stage.width, self.stage.height);
        let mut x = sx + 28.0;
        if x + w > sw - 12.0 {
            x = sx - w - 28.0;
        }
        x = x.clamp(12.0, (sw - w - 12.0).max(12.0));
        let y = (sy - h * 0.35).clamp(112.0, (sh - h - 80.0).max(112.0)) + (1.0 - e) * 10.0;
        pin(opaque(card)).x(x).y(y).width(Fill).height(Fill).into()
    }

    fn inspector(&self) -> Option<El<'_>> {
        let e = self.enter("inspector");
        style::faded(e, || self.inspector_card(e))
    }

    fn inspector_card(&self, e: f32) -> Option<El<'_>> {
        let pal = p();
        let id = self.selected?;
        let tab = self.tab()?;
        let a = tab.world.avatar(id)?;
        let s = self.sessions.get(&tab.key)?;
        let st = s.agents.get(&a.agent);
        let h = self.scene.heads.iter().find(|h| h.id == id)?;
        let (sx, sy) = self.cam.to_screen(h.x, (h.top + h.bottom) / 2.0);
        let role = if a.lead { format!("Lead · {}", s.model.clone().unwrap_or_else(|| s.harness_name.clone())) } else { st.and_then(|st| st.kind.clone()).unwrap_or_else(|| "Helper".into()) };
        let c = rgb8(a.color());
        let initial = a.name.chars().next().unwrap_or('?').to_uppercase().to_string();
        let head = row![
            container(t(initial, 14.0, on(c)).font(UI_BOLD)).center(34).style(style::dot(c)),
            column![t(clip(&a.name, 30), 14.0, pal.text).font(UI_BOLD), t(clip(&role, 38), 11.5, pal.muted)].spacing(1).width(Fill),
            small_x(Message::CloseCards)
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        let mut col = column![head, row![badge(&a.label)]].spacing(10);
        if let Some((_, said)) = &a.last_said {
            col = col.push(container(t(clip(said, 180), 12.5, pal.text)).padding([8, 10]).width(Fill).style(style::inset()));
        }
        let seat = match a.seated {
            Some(SeatRef::Desk(i)) => format!("Desk {}", i + 1),
            Some(SeatRef::Lounge(_)) => "Lounge".into(),
            None => "Walking".into(),
        };
        let kv = |k: &'static str, v: String| -> El<'static> { column![t(k, 11.0, pal.muted), mono(v, 13.0, pal.text)].spacing(2).width(Fill).into() };
        let (tok, tools) = st.map_or((0, 0), |st| (st.tokens, st.tools_used));
        col = col.push(row![kv("Tokens", fmt_k(tok)), kv("Tool calls", tools.to_string()), kv("Seat", seat)].spacing(10));
        let hist: Vec<El> = s
            .feed
            .iter()
            .filter(|f| f.who == a.name && !f.what.is_empty())
            .take(4)
            .map(|f| -> El { row![mono(clip(&f.what, 10), 11.0, pal.accent).width(76), t(clip(&f.detail, 30), 11.5, pal.muted)].spacing(6).into() })
            .collect();
        if !hist.is_empty() {
            col = col.push(column(hist).spacing(4));
        }
        let fg = if self.follow { on(pal.accent) } else { pal.text };
        let follow_on = self.follow;
        let follow = button(row![icon(Icon::Follow, 14.0, fg), t(if self.follow { "Following" } else { "Follow" }, 12.5, fg).font(UI_MEDIUM)].spacing(7).align_y(Alignment::Center))
            .padding([6, 12])
            .style(move |t, st| if follow_on { style::primary()(t, st) } else { style::secondary()(t, st) })
            .on_press(Message::Follow);
        col = col.push(row![follow, Space::new().width(Fill), tool(Icon::Left, "Previous", Message::Step(-1), false), tool(Icon::Right, "Next  Tab", Message::Step(1), false)].spacing(2).align_y(Alignment::Center));
        let card = container(col).padding(14).width(300).style(style::card());
        Some(self.hang(card.into(), sx, sy, 300.0, 300.0, e))
    }

    fn board_card(&self) -> Option<El<'_>> {
        if !self.board_open {
            return None;
        }
        let e = self.enter("board");
        style::faded(e, || self.board_card_inner(e))
    }

    fn board_card_inner(&self, e: f32) -> Option<El<'_>> {
        let pal = p();
        let tab = self.tab()?;
        let s = self.sessions.get(&tab.key)?;
        let o = &tab.world.s.layout;
        let (wx, wy) = self.view_of(tab).project((o.bx0 + o.bx1) as f32 / 2.0, 0.0, crate::render::view::wall_height() * 0.6);
        let (sx, sy) = self.cam.to_screen(wx, wy);
        let u = &s.usage;
        let cache = u.cache_read as f32 / u.total_in().max(1) as f32;
        let tools: u32 = s.tool_counts.values().sum();
        let cells: Vec<(&str, String)> = vec![
            ("Session", fmt_t(ago(s.started))),
            ("Est. cost", format!("${:.2}", s.cost_estimate())),
            ("Tokens in", fmt_k(u.total_in())),
            ("Tokens out", fmt_k(u.output)),
            ("Cache hits", format!("{}%", (cache * 100.0).round())),
            ("Tool calls", tools.to_string()),
            ("Files touched", s.files_touched.to_string()),
            ("Crew", tab.world.avatars.len().to_string()),
        ];
        let grid = column(cells.chunks(2).map(|pair| -> El { row(pair.iter().map(|(k, v)| -> El { column![t(*k, 11.0, pal.muted), mono(v.clone(), 14.0, pal.text)].spacing(2).width(Fill).into() })).spacing(12).into() })).spacing(10);
        let ctx = (s.context as f32 / s.context_window() as f32).clamp(0.0, 1.0);
        let fillw = ((ctx * 1000.0) as u16).clamp(1, 1000);
        let bar = row![container(Space::new()).width(Length::FillPortion(fillw)).height(6).style(style::fill(if ctx > 0.85 { pal.danger } else { pal.accent }, 3.0)), Space::new().width(Length::FillPortion((1000 - fillw).max(1)))];
        let bar = container(bar).width(Fill).height(6).style(style::fill(pal.faint, 3.0));
        let sub = [Some(s.harness_name.clone()), s.branch.clone().filter(|b| b != "HEAD"), s.model.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        let col = column![
            row![column![t(clip(&s.project(), 30), 14.0, pal.text).font(UI_BOLD), t(clip(&sub, 46), 11.5, pal.muted)].spacing(1).width(Fill), small_x(Message::CloseCards)].align_y(Alignment::Center),
            row![badge(status_label(s.status)), mono(fmt_t(ago(s.status_since)), 11.5, pal.muted)].spacing(8).align_y(Alignment::Center),
            grid,
            column![row![t("Context window", 11.0, pal.muted), Space::new().width(Fill), mono(format!("{}% of {}", (ctx * 100.0).round(), fmt_k(s.context_window())), 11.0, pal.text)], bar].spacing(6),
        ]
        .spacing(12);
        let card = container(col).padding(14).width(310).style(style::card());
        Some(self.hang(card.into(), sx, sy, 310.0, 300.0, e))
    }

    fn empty_state(&self) -> El<'_> {
        let pal = p();
        let watched: Vec<El> = self
            .harnesses
            .iter()
            .map(|h| -> El {
                let root = h.roots.first().map(|p| tidy_path(p)).unwrap_or_default();
                row![dot(if h.found > 0 { pal.working } else { pal.faint }, 7.0), t(h.name, 12.5, pal.text).width(140), mono(clip(&root, 38), 11.0, pal.muted)].spacing(9).align_y(Alignment::Center).into()
            })
            .collect();
        let col = column![
            t("No live sessions", 20.0, pal.text).font(UI_BOLD),
            t("Start a coding agent in any project. Its office opens here within a few seconds.", 13.0, pal.muted),
            scrollable(column(watched).spacing(6)).height(Length::Shrink).style(style::thin_scroll),
            row![
                button(t("Show the demo crew", 12.5, on(pal.accent)).font(UI_MEDIUM)).padding([7, 14]).style(style::primary()).on_press(Message::Setting(Setting::Demo(true))),
                button(t("Settings", 12.5, pal.text).font(UI_MEDIUM)).padding([7, 14]).style(style::secondary()).on_press(Message::ToggleSettings)
            ]
            .spacing(8),
        ]
        .spacing(14)
        .max_width(540);
        container(opaque(container(col).padding(24).max_height(560).style(style::card()))).center(Fill).into()
    }

    fn settings(&self) -> El<'_> {
        let e = self.enter("settings");
        style::faded(e, || self.settings_sheet(e))
    }

    fn settings_sheet(&self, e: f32) -> El<'_> {
        let pal = p();
        let cfg = self.config();
        let section = |s: &'static str| -> El<'static> { container(t(s, 12.0, pal.muted).font(UI_BOLD)).padding(Padding { top: 8.0, ..Padding::ZERO }).into() };
        let mut c = column![row![t("Settings", 17.0, pal.text).font(UI_BOLD), Space::new().width(Fill), small_x(Message::ToggleSettings)].align_y(Alignment::Center)].spacing(12);

        // appearance: presets as cards, then accent swatches, then every colour by hand
        c = c.push(section("Appearance"));
        let cards: Vec<El> = style::PRESETS
            .iter()
            .map(|pr| -> El {
                let q = pr.pal;
                let sample = container(column![t("Aa", 15.0, q.text).font(UI_BOLD), row![dot(q.accent, 7.0), dot(q.working, 7.0), dot(q.waiting, 7.0)].spacing(3)].spacing(4))
                    .padding([8, 10])
                    .width(Fill)
                    .style(move |_| container::Style { background: Some(iced::Background::Color(q.surface)), border: iced::border::rounded(8), ..Default::default() });
                let sel = cfg.appearance.theme == pr.id;
                button(column![sample, t(pr.name, 11.5, if sel { pal.text } else { pal.muted }).font(UI_MEDIUM)].spacing(5).width(Fill))
                    .padding(4)
                    .width(Fill)
                    .style(style::preset_card(sel))
                    .on_press(Message::Setting(Setting::Theme(pr.id.into())))
                    .into()
            })
            .collect();
        let mut grid = column![].spacing(4);
        let mut it = cards.into_iter();
        loop {
            let r: Vec<El> = it.by_ref().take(4).collect();
            if r.is_empty() {
                break;
            }
            grid = grid.push(row(r).spacing(4));
        }
        c = c.push(grid);
        let accents = row(style::ACCENTS.iter().map(|&a| -> El {
            let col = Color::from_rgb8((a >> 16) as u8, (a >> 8) as u8, a as u8);
            let sel = style::to_hex(pal.accent) == style::to_hex(col);
            button(Space::new().width(22).height(22))
                .padding(0)
                .style(move |_, _| button::Style { background: Some(iced::Background::Color(col)), border: iced::Border { color: if sel { pal.text } else { Color::TRANSPARENT }, width: 2.0, radius: 999.0.into() }, ..Default::default() })
                .on_press(Message::Setting(Setting::Accent(a)))
                .into()
        }))
        .spacing(8);
        c = c.push(row![t("Accent", 12.5, pal.text).width(Fill), accents].align_y(Alignment::Center));
        let colors = column(style::ROLES.iter().map(|&(key, label)| -> El {
            let cur = style::role(&pal, key);
            let draft = self.color_drafts.get(key).cloned().unwrap_or_else(|| style::to_hex(cur));
            let k = key.to_owned();
            row![
                container(Space::new()).width(20).height(20).style(style::ring(cur, false)),
                t(label, 12.5, pal.text).width(Fill),
                text_input("#rrggbb", &draft).on_input(move |v| Message::Setting(Setting::Color(k.clone(), v))).font(MONO).size(12).padding([5, 8]).width(96).style(style::input),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
        }))
        .spacing(6);
        c = c.push(colors);
        let mut office_styles = vec!["Procedural".to_owned()];
        office_styles.extend(THEMES.iter().map(|th| th.name.to_owned()));
        let current = cfg.appearance.office_style.clone().unwrap_or_else(|| "Procedural".into());
        c = c.push(
            row![
                t("Office style", 12.5, pal.text).width(Fill),
                pick_list(office_styles, Some(current), |v: String| Message::Setting(Setting::OfficeStyle(if v == "Procedural" { None } else { Some(v) }))).text_size(12).padding([5, 10]).style(style::select).menu_style(style::menu),
            ]
            .align_y(Alignment::Center),
        );
        if !cfg.appearance.colors.is_empty() {
            c = c.push(button(t("Reset colours to the preset", 12.0, pal.text).font(UI_MEDIUM)).padding([6, 12]).style(style::secondary()).on_press(Message::Setting(Setting::ResetColors)));
        }

        c = c.push(section("Sources"));
        for h in &self.harnesses {
            let hc = cfg.harness(h.id);
            let id = h.id.to_owned();
            let found = if h.found > 0 { format!("{} live", h.found) } else { String::new() };
            let mut hcol = column![row![t(h.name, 12.5, pal.text).width(Fill), mono(found, 11.0, pal.working), toggler(hc.enabled).on_toggle(move |v| Message::Setting(Setting::Harness(id.clone(), v))).size(18).style(style::toggle)].spacing(10).align_y(Alignment::Center)].spacing(3);
            for (i, path) in hc.extra_paths.iter().enumerate() {
                let id = h.id.to_owned();
                hcol = hcol.push(row![mono(clip(&tidy_path(path), 44), 11.0, pal.muted).width(Fill), small_x(Message::Setting(Setting::RemovePath(id, i)))].align_y(Alignment::Center));
            }
            c = c.push(hcol);
        }
        let choices: Vec<HarnessChoice> = self.harnesses.iter().map(|h| HarnessChoice { id: h.id.into(), name: h.name.into() }).collect();
        c = c.push(
            column![
                t("Watch another folder", 12.5, pal.text),
                row![
                    pick_list(choices, self.path_harness.clone(), |h| Message::Setting(Setting::PathHarness(h))).text_size(12).padding([5, 8]).width(124).style(style::select).menu_style(style::menu),
                    text_input("~/path/to/sessions", &self.path_input).on_input(|s| Message::Setting(Setting::PathInput(s))).on_submit(Message::Setting(Setting::AddPath)).size(12).padding([5, 8]).style(style::input).font(MONO),
                    button(t("Add", 12.0, on(pal.accent)).font(UI_MEDIUM)).padding([5, 12]).style(style::primary()).on_press(Message::Setting(Setting::AddPath)),
                ]
                .spacing(6)
                .align_y(Alignment::Center)
            ]
            .spacing(6),
        );

        c = c.push(section("Office"));
        let switch = |label: &'static str, v: bool, f: fn(bool) -> Setting| -> El<'static> { row![t(label, 12.5, pal.text).width(Fill), toggler(v).on_toggle(move |x| Message::Setting(f(x))).size(18).style(style::toggle)].align_y(Alignment::Center).into() };
        c = c.push(switch("Speech bubbles", cfg.bubbles, Setting::Bubbles));
        c = c.push(switch("Demo crew when nothing is live", cfg.demo_when_empty, Setting::Demo));
        c = c.push(switch("Show frame rate", cfg.show_fps, Setting::ShowFps));
        let fps = row([12u32, 16, 24].into_iter().map(|v| -> El { button(mono(format!("{v}"), 11.5, if cfg.anim_fps == v { pal.text } else { pal.muted })).padding([5, 10]).style(style::flat(cfg.anim_fps == v)).on_press(Message::Setting(Setting::AnimFps(v))).into() })).spacing(2);
        c = c.push(row![t("Animation frames per second", 12.5, pal.text).width(Fill), container(fps).padding(2).style(style::inset())].align_y(Alignment::Center));
        c = c.push(
            column![
                row![t("Sessions active in the last", 12.5, pal.text).width(Fill), mono(format!("{} min", cfg.active_minutes), 12.0, pal.text)],
                slider(5.0..=240.0, cfg.active_minutes as f64, |v| Message::Setting(Setting::ActiveMinutes(v.round() as u64))).step(5.0).style(style::range)
            ]
            .spacing(8),
        );
        c = c.push(
            column![
                row![t("Most tabs at once", 12.5, pal.text).width(Fill), mono(format!("{}", cfg.max_tabs), 12.0, pal.text)],
                slider(1.0..=12.0, cfg.max_tabs as f64, |v| Message::Setting(Setting::MaxTabs(v.round() as usize))).step(1.0).style(style::range)
            ]
            .spacing(8),
        );

        c = c.push(section("Shortcuts"));
        let keys = [("1–9", "Switch tab"), ("Tab", "Next agent"), ("F", "Follow"), ("+  −  0", "Zoom, fit"), ("Esc", "Close")];
        c = c.push(column(keys.iter().map(|(k, v)| -> El { row![t(*v, 12.0, pal.muted).width(Fill), container(mono(*k, 11.0, pal.text)).padding([2, 7]).style(style::inset())].align_y(Alignment::Center).into() })).spacing(5));
        let path = crate::config::Config::path().map(|p| tidy_path(&p)).unwrap_or_default();
        c = c.push(row![mono(clip(&path, 38), 11.0, pal.muted).width(Fill), button(t("Open folder", 12.0, pal.text).font(UI_MEDIUM)).padding([5, 12]).style(style::secondary()).on_press(Message::Setting(Setting::OpenConfigFolder))].align_y(Alignment::Center));

        let panel = container(scrollable(container(c).padding(Padding { right: 12.0, ..Padding::ZERO })).style(style::thin_scroll)).padding(18).width(400).max_height((self.stage.height - 76.0).max(200.0)).style(style::card());
        let shade = mouse_area(container(Space::new()).width(Fill).height(Fill).style(move |_| container::Style { background: Some(iced::Background::Color(alpha(Color::BLACK, if pal.light { 0.12 } else { 0.3 }))), ..Default::default() })).on_press(Message::ToggleSettings);
        stack![shade, container(opaque(panel)).align_right(Fill).padding(Padding { top: 62.0, right: 12.0 - (1.0 - e) * 24.0, bottom: 12.0, left: 12.0 })].into()
    }
}

fn fmt_ago(s: u64) -> String {
    if s < 5 {
        "now".into()
    } else if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h", s / 3600)
    }
}

/// Paths in the UI use ~ for the home folder, so screenshots do not show user names.
pub fn tidy_path(p: &std::path::Path) -> String {
    let home = crate::harness::home();
    match p.strip_prefix(&home) {
        Ok(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        Err(_) => p.display().to_string(),
    }
}

/// Window edges that resize a frameless window.
fn resize_grips<'a>() -> El<'a> {
    use window::Direction as D;
    let g = Length::Fixed(5.0);
    let edge = |w: Length, h: Length, dir: D, i: mouse::Interaction| -> El<'a> { mouse_area(Space::new().width(w).height(h)).on_press(Message::Window(WindowAction::Resize(dir))).interaction(i).into() };
    let (ew, ns, nwse, nesw) = (mouse::Interaction::ResizingHorizontally, mouse::Interaction::ResizingVertically, mouse::Interaction::ResizingDiagonallyDown, mouse::Interaction::ResizingDiagonallyUp);
    column![
        row![edge(g, g, D::NorthWest, nwse), edge(Fill, g, D::North, ns), edge(g, g, D::NorthEast, nesw)],
        row![edge(g, Fill, D::West, ew), Space::new().width(Fill).height(Fill), edge(g, Fill, D::East, ew)].height(Fill),
        row![edge(g, g, D::SouthWest, nesw), edge(Fill, g, D::South, ns), edge(g, g, D::SouthEast, nwse)],
    ]
    .into()
}

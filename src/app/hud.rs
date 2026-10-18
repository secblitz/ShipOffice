//! The HUD, kept to the corners so the office stays the picture: tabs and one status line top
//! left, settings and window controls top right, a single toolbar bottom centre. The crew list,
//! the live feed and the cards open on demand.

use super::icon::{icon, icon_turned, Icon};
use super::popover::Popover;
use super::overlay::Overlay;
use super::style::{self, alpha, on, p, rgb8, MONO, UI_BOLD, UI_MEDIUM};
use super::{App, HarnessChoice, Message, Page, Setting, WindowAction};
use crate::office::gen::THEMES;
use crate::office::world::SeatRef;
use crate::render::paint::{fmt_k, fmt_t};
use crate::render::scene::Hit;
use crate::session::{Session, Status};
use iced::widget::{button, canvas, column, container, mouse_area, opaque, pin, row, scrollable, stack, text, text_input, toggler, tooltip, Space};
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

/// An agent's face from their own sprite, ringed in their shirt colour; their initial if the
/// sprite has none.
fn face<'a>(a: &crate::office::world::Avatar, d: f32) -> El<'a> {
    let c = rgb8(a.color());
    let ring = 2.0;
    let inner: El = match super::portrait::portrait(a.char_id, &a.palette) {
        Some(h) => iced::widget::image(h).width(d - 2.0 * ring).height(d - 2.0 * ring).border_radius((d - 2.0 * ring) / 2.0).into(),
        None => container(t(a.name.chars().next().unwrap_or('?').to_uppercase().to_string(), d * 0.4, on(c)).font(UI_BOLD)).center(d - 2.0 * ring).into(),
    };
    let pal = p();
    container(container(inner).center(d - 2.0 * ring).style(style::fill(pal.raised, 999.0))).padding(ring).style(style::fill(c, 999.0)).into()
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

    /// The dice icon; it spins for a moment after a roll.
    fn dice(&self, size: f32) -> El<'static> {
        let k = self.rolled.map_or(1.0, |t| style::entrance(t, 0.5));
        icon_turned(Icon::Dice, size, p().muted, k * std::f32::consts::PI)
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
            let mut body = row![self.status_dot(status, 7.0), t(clip(&name, 22), 13.0, if sel { pal.text } else { pal.muted }).font(if sel { UI_BOLD } else { UI_MEDIUM })].spacing(8).align_y(Alignment::Center);
            if sel {
                // the active office can be rolled again, right where its name is
                let dice = button(self.dice(13.0)).padding(3).style(style::flat(false)).on_press(Message::NewLayout);
                body = body.push(tooltip(dice, container(t("New office for this session  N", 12.0, pal.text)).padding([5, 9]).style(style::surface(8.0)), tooltip::Position::Bottom).gap(8));
            }
            button(body).padding(if sel { Padding { top: 4.0, bottom: 4.0, left: 12.0, right: 5.0 } } else { Padding::from([7, 12]) }).style(style::flat(sel)).on_press(Message::SelectTab(i)).into()
        }))
        .spacing(2);
        r = r.push(scrollable(tabs).direction(no_scrollbar()).style(style::hidden_scroll));
        opaque(container(r).padding(4).height(44).align_y(Alignment::Center).style(style::surface(12.0)).max_width(900))
    }

    /// One line with what matters now; the session card has the rest.
    fn status_line(&self, s: &Session) -> El<'_> {
        let pal = p();
        let mut r = row![
            self.status_dot(s.status, 8.0),
            t(status_label(s.status), 12.5, pal.text).font(UI_BOLD),
            mono(fmt_t(ago(s.status_since)), 11.5, pal.muted),
            t("·", 12.0, pal.muted),
            mono(format!("${:.2}", s.cost_estimate()), 11.5, pal.text),
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
            tool(Icon::TurnLeft, "Turn left  Q · drag to turn and tilt", Message::Turn(std::f32::consts::FRAC_PI_2), false),
            tool(Icon::TurnRight, "Turn right  E", Message::Turn(-std::f32::consts::FRAC_PI_2), false),
        ]
        .spacing(2)
        .align_y(Alignment::Center);
        let r = if self.turned() { r.push(tool(Icon::Reset, "Back to the classic angle  R", Message::ResetView, false)) } else { r };
        let r = row![
            r,
            sep(),
            tool(Icon::Feed, "Live feed", Message::ToggleFeed, feed_open),
            tooltip(button(container(self.dice(16.0)).center(Fill)).width(32).height(32).padding(0).style(style::flat(false)).on_press(Message::NewLayout), container(t("New office for this session  N", 12.0, pal.text)).padding([5, 9]).style(style::surface(8.0)), tooltip::Position::Top).gap(8),
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
            let body = row![face(a, 24.0), t(clip(&a.name, 30), 12.5, pal.text).font(UI_MEDIUM).width(Fill), t(clip(&a.label, 30), 11.5, badge_color(&a.label))].spacing(10).align_y(Alignment::Center);
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
        let head = row![
            face(a, 38.0),
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
        col = col.push(self.context_gauge(s, st));
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

    /// One agent's context: how full it is, where the user's compaction setting kicks in, and how
    /// often it has been compacted.
    fn context_gauge(&self, s: &Session, st: Option<&crate::session::AgentState>) -> El<'static> {
        use crate::harness::compaction::Rule;
        let pal = p();
        let window = s.context_window().max(1);
        let (ctx, times, last) = st.map_or((0, 0, None), |st| (st.context, st.compactions, st.compacted_at));
        let share = (ctx as f32 / window as f32).clamp(0.0, 1.0);
        let known = matches!(s.key.harness, "claude-code" | "codex" | "gemini-cli" | "qwen-code");
        let setting = if known { self.compaction_for(s.key.harness, s.cwd.as_deref()) } else { None };
        let mark = match setting.as_ref().map(|c| &c.rule) {
            Some(Rule::Share(x)) => Some(*x),
            Some(Rule::Tokens(n)) => Some((*n as f32 / window as f32).min(1.0)),
            _ => None,
        };
        let near = mark.map_or(share > 0.9, |m| share >= m - 0.05);
        let fill = ((share * 1000.0) as u16).clamp(1, 1000);
        let level = row![container(Space::new()).width(Length::FillPortion(fill)).height(6).style(style::fill(if near { pal.danger } else { pal.accent }, 3.0)), Space::new().width(Length::FillPortion((1000 - fill).max(1)))];
        let mut bar = stack![container(Space::new()).width(Fill).height(6).style(style::fill(pal.faint, 3.0)), level];
        if let Some(m) = mark {
            let at = ((m * 1000.0) as u16).clamp(1, 999);
            bar = bar.push(row![Space::new().width(Length::FillPortion(at)), container(Space::new()).width(2).height(10).style(style::fill(pal.text, 1.0)), Space::new().width(Length::FillPortion(1000 - at))].align_y(Alignment::Center).height(6));
        }
        let rule = match setting.as_ref().map(|c| &c.rule) {
            Some(Rule::Off) => "Auto-compact off".to_owned(),
            Some(Rule::Share(x)) => format!("Compacts at {:.0}%", x * 100.0),
            Some(Rule::Tokens(n)) => format!("Compacts at {}", fmt_k(*n)),
            None if known => "Compacts near the limit".to_owned(),
            None => String::new(),
        };
        let mut col = column![
            row![t("Context", 11.0, pal.muted), Space::new().width(Fill), mono(format!("{}% · {} of {}", (share * 100.0).round(), fmt_k(ctx), fmt_k(window)), 11.0, pal.text)],
            container(bar).height(10).align_y(Alignment::Center),
        ]
        .spacing(5);
        let mut notes: Vec<String> = Vec::new();
        if !rule.is_empty() {
            notes.push(match &setting {
                Some(c) => format!("{rule} · {}", c.from),
                None => format!("{rule} (default)"),
            });
        }
        if times > 0 {
            let ago = last.map(|t| format!(", last {} ago", fmt_ago(ago(t)))).unwrap_or_default();
            notes.push(format!("Compacted {times}×{ago}"));
        }
        for n in notes {
            col = col.push(t(clip(&n, 60), 11.0, pal.muted));
        }
        col.into()
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
        let sub = [Some(s.harness_name.clone()), s.branch.clone().filter(|b| b != "HEAD"), s.model.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        let col = column![
            row![column![t(clip(&s.project(), 30), 14.0, pal.text).font(UI_BOLD), t(clip(&sub, 46), 11.5, pal.muted)].spacing(1).width(Fill), small_x(Message::CloseCards)].align_y(Alignment::Center),
            row![badge(status_label(s.status)), mono(fmt_t(ago(s.status_since)), 11.5, pal.muted)].spacing(8).align_y(Alignment::Center),
            grid,
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
        let header = row![t("Settings", 17.0, pal.text).font(UI_BOLD), Space::new().width(Fill), small_x(Message::ToggleSettings)].align_y(Alignment::Center);
        let pages = row([(Page::Look, "Look"), (Page::Sources, "Sources"), (Page::Office, "Office")].into_iter().map(|(pg, label)| -> El {
            let sel = self.page == pg;
            button(t(label, 12.5, if sel { pal.text } else { pal.muted }).font(if sel { UI_BOLD } else { UI_MEDIUM }).width(Fill).align_x(Alignment::Center)).width(Fill).padding([6, 0]).style(style::flat(sel)).on_press(Message::SettingsPage(pg)).into()
        }))
        .spacing(2);
        let body = match self.page {
            Page::Look => self.page_look(&cfg),
            Page::Sources => self.page_sources(&cfg),
            Page::Office => self.page_office(&cfg),
        };
        let c = column![header, container(pages).padding(3).style(style::inset()), scrollable(container(body).padding(Padding { right: 10.0, bottom: 4.0, ..Padding::ZERO })).style(style::thin_scroll).height(Length::Shrink)].spacing(14);
        let panel = container(c).padding(18).width(392).max_height((self.stage.height - 76.0).max(200.0)).style(style::card());
        let shade = mouse_area(container(Space::new()).width(Fill).height(Fill).style(move |_| container::Style { background: Some(iced::Background::Color(alpha(Color::BLACK, if pal.light { 0.12 } else { 0.3 }))), ..Default::default() })).on_press(Message::ToggleSettings);
        stack![shade, container(opaque(panel)).align_right(Fill).padding(Padding { top: 62.0, right: 12.0 - (1.0 - e) * 24.0, bottom: 12.0, left: 12.0 })].into()
    }

    /// A dropdown: the trigger shows the current choice, the menu floats over everything.
    fn dropdown<'a>(&self, id: &'static str, current: El<'a>, options: Vec<(El<'a>, Message, bool)>, width: f32) -> El<'a> {
        let pal = p();
        let open = self.menu == Some(id);
        let trigger = button(row![current, Space::new().width(Fill), icon(Icon::Chevron(!open), 14.0, pal.muted)].spacing(8).align_y(Alignment::Center))
            .padding([6, 10])
            .width(width)
            .style(style::trigger(open))
            .on_press(Message::Menu(if open { None } else { Some(id) }));
        let menu = style::faded(self.enter("menu"), || {
            let items = column(options.into_iter().map(|(content, msg, sel)| -> El {
                let mark: El = if sel { icon(Icon::Check, 14.0, pal.accent) } else { Space::new().width(14).into() };
                button(row![content, Space::new().width(Fill), mark].spacing(8).align_y(Alignment::Center)).width(Fill).padding([7, 10]).style(style::menu_item(sel)).on_press(msg).into()
            }))
            .spacing(2);
            container(scrollable(items).style(style::thin_scroll)).padding(5).max_height(300).style(style::menu_panel())
        });
        Popover::new(trigger, menu, open, Message::Menu(None)).width(width.max(200.0)).into()
    }

    fn page_look(&self, cfg: &crate::config::Config) -> El<'_> {
        let pal = p();
        let preset = style::preset(&cfg.appearance.theme);
        let theme_opts = style::PRESETS.iter().map(|pr| (row![chip(pr.pal), t(pr.name, 12.5, pal.text)].spacing(10).align_y(Alignment::Center).into(), Message::Setting(Setting::Theme(pr.id.into())), pr.id == preset.id)).collect();
        let theme = self.dropdown("theme", row![chip(preset.pal), t(preset.name, 12.5, pal.text).font(UI_MEDIUM)].spacing(10).align_y(Alignment::Center).into(), theme_opts, 180.0);
        let accents = row(style::ACCENTS.iter().map(|&a| -> El {
            let col = Color::from_rgb8((a >> 16) as u8, (a >> 8) as u8, a as u8);
            let sel = style::to_hex(pal.accent) == style::to_hex(col);
            button(Space::new().width(18).height(18))
                .padding(0)
                .style(move |_, st| button::Style { background: Some(iced::Background::Color(col)), border: iced::Border { color: if sel { pal.text } else if matches!(st, button::Status::Hovered) { alpha(pal.text, 0.4) } else { Color::TRANSPARENT }, width: 2.0, radius: 999.0.into() }, ..Default::default() })
                .on_press(Message::Setting(Setting::Accent(a)))
                .into()
        }))
        .spacing(6);
        let style_name = cfg.appearance.office_style.clone();
        let mut office_opts: Vec<(El, Message, bool)> = vec![(column![t("Procedural", 12.5, pal.text), t("Every project its own", 11.0, pal.muted)].spacing(1).into(), Message::Setting(Setting::OfficeStyle(None)), style_name.is_none())];
        office_opts.extend(THEMES.iter().map(|th| -> (El, Message, bool) { (row![room_chip(th), t(th.name, 12.5, pal.text)].spacing(10).align_y(Alignment::Center).into(), Message::Setting(Setting::OfficeStyle(Some(th.name.into()))), style_name.as_deref() == Some(th.name)) }));
        let current: El = match style_name.as_deref().and_then(|n| THEMES.iter().find(|th| th.name == n)) {
            Some(th) => row![room_chip(th), t(th.name, 12.5, pal.text).font(UI_MEDIUM)].spacing(10).align_y(Alignment::Center).into(),
            None => t("Procedural", 12.5, pal.text).font(UI_MEDIUM).into(),
        };
        let office = self.dropdown("office", current, office_opts, 180.0);
        use crate::config::Backdrop;
        let names = [(Backdrop::City, "City", "Day or night with the theme"), (Backdrop::Day, "City by day", ""), (Backdrop::Night, "City by night", ""), (Backdrop::None, "None", "The office on its own")];
        let cur_b = cfg.appearance.backdrop;
        let backdrop_opts = names
            .iter()
            .map(|&(b, name, hint)| -> (El, Message, bool) {
                let label: El = if hint.is_empty() { t(name, 12.5, pal.text).into() } else { column![t(name, 12.5, pal.text), t(hint, 11.0, pal.muted)].spacing(1).into() };
                (label, Message::Setting(Setting::Backdrop(b)), b == cur_b)
            })
            .collect();
        let cur_name = names.iter().find(|n| n.0 == cur_b).map_or("City", |n| n.1);
        let backdrop = self.dropdown("backdrop", t(cur_name, 12.5, pal.text).font(UI_MEDIUM).into(), backdrop_opts, 180.0);
        let mut c = column![group("Theme", vec![item("Theme", None, theme), item("Accent", None, accents.into()), item("Office style", None, office), item("Backdrop", None, backdrop)])].spacing(18);

        // hand-picked colours stay folded away until asked for
        let n = cfg.appearance.colors.len();
        let summary = if n == 0 { "From the theme".to_owned() } else { format!("{n} changed") };
        let fold = button(row![t("Custom colours", 12.5, pal.text).width(Fill), t(summary, 11.5, pal.muted), icon(Icon::Chevron(!self.colors_open), 14.0, pal.muted)].spacing(8).align_y(Alignment::Center)).width(Fill).padding([10, 12]).style(style::flat(false)).on_press(Message::ToggleColors);
        let mut rows: Vec<El> = vec![fold.into()];
        if self.colors_open {
            for &(key, label) in style::ROLES.iter() {
                let cur = style::role(&pal, key);
                let draft = self.color_drafts.get(key).cloned().unwrap_or_else(|| style::to_hex(cur));
                let k = key.to_owned();
                rows.push(
                    row![
                        container(Space::new()).width(18).height(18).style(style::ring(cur, false)),
                        t(label, 12.5, pal.text).width(Fill),
                        text_input("#rrggbb", &draft).on_input(move |v| Message::Setting(Setting::Color(k.clone(), v))).font(MONO).size(12).padding([5, 8]).width(92).style(style::input),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .padding([6, 12])
                    .into(),
                );
            }
            if n > 0 {
                rows.push(container(button(t("Reset to the theme", 12.0, pal.text).font(UI_MEDIUM)).padding([6, 12]).style(style::secondary()).on_press(Message::Setting(Setting::ResetColors))).padding([8, 12]).into());
            }
        }
        c = c.push(group_raw("Colours", rows));
        c.into()
    }

    fn page_sources(&self, cfg: &crate::config::Config) -> El<'_> {
        let pal = p();
        // live harnesses first, then the ones you switched on, then the rest
        let mut hs: Vec<_> = self.harnesses.iter().collect();
        hs.sort_by_key(|h| (h.found == 0, !cfg.harness(h.id).enabled));
        let rows = hs
            .into_iter()
            .map(|h| -> El {
                let hc = cfg.harness(h.id);
                let id = h.id.to_owned();
                let (sub, sub_c) = if h.found > 0 { (format!("{} live", h.found), pal.working) } else if hc.enabled { ("Watching".to_owned(), pal.muted) } else { ("Off".to_owned(), pal.muted) };
                let mut col = column![row![column![t(h.name, 12.5, pal.text), t(sub, 11.0, sub_c)].spacing(1).width(Fill), toggler(hc.enabled).on_toggle(move |v| Message::Setting(Setting::Harness(id.clone(), v))).size(18).style(style::toggle)].align_y(Alignment::Center)].spacing(4);
                for (i, path) in hc.extra_paths.iter().enumerate() {
                    let id = h.id.to_owned();
                    col = col.push(row![mono(clip(&tidy_path(path), 40), 11.0, pal.muted).width(Fill), small_x(Message::Setting(Setting::RemovePath(id, i)))].align_y(Alignment::Center));
                }
                container(col).padding([9, 12]).into()
            })
            .collect();
        let pick = self.path_harness.clone();
        let opts = self.harnesses.iter().map(|h| -> (El, Message, bool) { (t(h.name, 12.5, pal.text).into(), Message::Setting(Setting::PathHarness(HarnessChoice { id: h.id.into(), name: h.name.into() })), pick.as_ref().is_some_and(|p| p.id == h.id)) }).collect();
        let current: El = match &pick {
            Some(h) => t(h.name.clone(), 12.5, pal.text).font(UI_MEDIUM).into(),
            None => t("Choose a harness", 12.5, pal.muted).into(),
        };
        let add = column![
            item("Harness", None, self.dropdown("harness", current, opts, 180.0)),
            row![
                text_input("~/path/to/sessions", &self.path_input).on_input(|s| Message::Setting(Setting::PathInput(s))).on_submit(Message::Setting(Setting::AddPath)).size(12).padding([6, 9]).style(style::input).font(MONO),
                button(t("Add", 12.0, on(pal.accent)).font(UI_MEDIUM)).padding([6, 14]).style(style::primary()).on_press(Message::Setting(Setting::AddPath)),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .padding([4, 12]),
        ]
        .spacing(4)
        .padding(Padding { bottom: 8.0, ..Padding::ZERO });
        column![group_raw("Harnesses", rows), group_raw("Watch another folder", vec![add.into()])].spacing(18).into()
    }

    fn page_office(&self, cfg: &crate::config::Config) -> El<'_> {
        let pal = p();
        let switch = |v: bool, f: fn(bool) -> Setting| -> El<'static> { toggler(v).on_toggle(move |x| Message::Setting(f(x))).size(18).style(style::toggle).into() };
        let label = |s: String| -> El<'static> { t(s, 12.5, pal.text).font(UI_MEDIUM).into() };
        let plain = |s: String| -> El<'static> { t(s, 12.5, pal.text).into() };

        let mins = |m: u64| if m % 60 == 0 { format!("{} h", m / 60) } else { format!("{m} min") };
        let mut steps = vec![15u64, 30, 45, 60, 120, 240];
        if !steps.contains(&cfg.active_minutes) {
            steps.push(cfg.active_minutes);
            steps.sort();
        }
        let window = self.dropdown("minutes", label(mins(cfg.active_minutes)), steps.iter().map(|&m| (plain(mins(m)), Message::Setting(Setting::ActiveMinutes(m)), m == cfg.active_minutes)).collect(), 110.0);
        let tabs = self.dropdown("tabs", label(cfg.max_tabs.to_string()), (1..=12usize).map(|n| (plain(n.to_string()), Message::Setting(Setting::MaxTabs(n)), n == cfg.max_tabs)).collect(), 110.0);
        let fps_name = |v: u32| match v {
            12 => "12 fps · lighter".to_owned(),
            24 => "24 fps · smoothest".to_owned(),
            v => format!("{v} fps"),
        };
        let fps = self.dropdown("fps", label(format!("{} fps", cfg.anim_fps)), [12u32, 16, 24].into_iter().map(|v| (plain(fps_name(v)), Message::Setting(Setting::AnimFps(v)), v == cfg.anim_fps)).collect(), 110.0);

        let keys = [("Drag", "Turn and tilt"), ("Right-drag", "Pan"), ("Scroll", "Zoom"), ("Q  E", "Turn a quarter"), ("R", "Classic angle"), ("N", "New office"), ("F", "Follow"), ("Tab", "Next agent"), ("1–9", "Switch tab"), ("Esc", "Close")];
        let key_rows = keys.iter().map(|(k, v)| -> El { row![t(*v, 12.0, pal.muted).width(Fill), container(mono(*k, 11.0, pal.text)).padding([2, 7]).style(style::inset())].align_y(Alignment::Center).padding([5, 12]).into() }).collect();
        let path = crate::config::Config::path().map(|p| tidy_path(&p)).unwrap_or_default();
        column![
            group("Scene", vec![
                item("Speech bubbles", None, switch(cfg.bubbles, Setting::Bubbles)),
                item("Snap to corners", Some("After a drag, settle where the art reads best"), switch(cfg.snap_view, Setting::SnapView)),
                item("Demo crew", Some("Pretend agents when nothing is live"), switch(cfg.demo_when_empty, Setting::Demo)),
            ]),
            group("Sessions", vec![item("Active within", None, window), item("Most tabs", None, tabs)]),
            group("Performance", vec![item("Animation", None, fps), item("Show frame rate", None, switch(cfg.show_fps, Setting::ShowFps))]),
            group_raw("Controls", key_rows),
            group_raw("Config file", vec![row![mono(clip(&path, 34), 11.0, pal.muted).width(Fill), button(t("Open folder", 12.0, pal.text).font(UI_MEDIUM)).padding([5, 12]).style(style::secondary()).on_press(Message::Setting(Setting::OpenConfigFolder))].align_y(Alignment::Center).padding([8, 12]).into()]),
        ]
        .spacing(18)
        .into()
    }
}

/// A settings row: label (with an optional hint under it) on the left, its control on the right.
fn item<'a>(label: &'a str, hint: Option<&'a str>, control: El<'a>) -> El<'a> {
    let pal = p();
    let mut l = column![t(label, 12.5, pal.text)].spacing(2).width(Fill);
    if let Some(h) = hint {
        l = l.push(t(h, 11.0, pal.muted));
    }
    row![l, control].spacing(12).align_y(Alignment::Center).padding([8, 12]).into()
}

/// A titled block of rows with hairlines between them.
fn group_raw<'a>(title: &'a str, rows: Vec<El<'a>>) -> El<'a> {
    let pal = p();
    let mut c = column![];
    let n = rows.len();
    for (i, r) in rows.into_iter().enumerate() {
        c = c.push(r);
        if i + 1 < n {
            c = c.push(container(container(Space::new()).width(Fill).height(1).style(style::fill(alpha(pal.text, 0.06), 0.0))).padding([0, 12]));
        }
    }
    column![t(title, 11.5, pal.muted).font(UI_BOLD), container(c).padding([3, 0]).style(style::group())].spacing(7).into()
}

fn group<'a>(title: &'a str, rows: Vec<El<'a>>) -> El<'a> {
    group_raw(title, rows)
}

/// A theme in miniature: its panel colour with its three signal colours on it.
fn chip<'a>(q: style::Pal) -> El<'a> {
    container(row![dot(q.accent, 6.0), dot(q.working, 6.0), dot(q.waiting, 6.0)].spacing(3))
        .padding([4, 6])
        .style(move |_| container::Style { background: Some(iced::Background::Color(q.surface)), border: iced::Border { color: alpha(q.text, 0.15), width: 1.0, radius: 6.0.into() }, ..Default::default() })
        .into()
}

/// An office style in miniature: its two wall colours.
fn room_chip<'a>(th: &crate::office::gen::Theme) -> El<'a> {
    let c = |v: u32| Color::from_rgb8((v >> 16) as u8, (v >> 8) as u8, v as u8);
    let (a, b) = (c(th.back), c(th.accent));
    row![container(Space::new()).width(10).height(16).style(style::fill(a, 3.0)), container(Space::new()).width(10).height(16).style(style::fill(b, 3.0))].spacing(2).into()
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

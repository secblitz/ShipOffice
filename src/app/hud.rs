//! The HUD: session tabs and window controls on top, the session's vital signs under them, the
//! crew and the live feed at the bottom, and cards that hang off whatever you clicked.

use super::icon::{icon, Icon};
use super::overlay::Overlay;
use super::style::{self, alpha, rgb8, AMBER, CREAM, EMBER, MONO, MUTED, ROSE, SKY, TEAL, UI_BOLD, UI_MEDIUM};
use super::{App, HarnessChoice, Message, Setting, WindowAction};
use crate::office::world::SeatRef;
use crate::office::{iso, TW};
use crate::render::paint::{fmt_k, fmt_t};
use crate::session::{Session, Status};
use iced::widget::{button, canvas, column, container, mouse_area, opaque, pick_list, pin, row, scrollable, slider, stack, text, text_input, toggler, Space};
use iced::{mouse, window, Alignment, Color, Element, Fill, Length, Padding};
use std::time::SystemTime;

type El<'a> = Element<'a, Message>;

fn t<'a>(s: impl text::IntoFragment<'a>, size: f32, color: Color) -> text::Text<'a> {
    text(s).size(size).color(color)
}

fn mono<'a>(s: impl text::IntoFragment<'a>, size: f32, color: Color) -> text::Text<'a> {
    text(s).size(size).color(color).font(MONO)
}

fn dot<'a>(c: Color, d: f32) -> El<'a> {
    container(Space::new()).width(d).height(d).style(style::dot(c)).into()
}

fn icon_button<'a>(i: Icon, msg: Message, on: bool) -> El<'a> {
    button(container(icon(i, 14.0, if on { CREAM } else { MUTED })).center(Fill)).width(30).height(28).padding(0).style(style::seg(on)).on_press(msg).into()
}

fn ago(t: SystemTime) -> u64 {
    SystemTime::now().duration_since(t).unwrap_or_default().as_secs()
}

pub fn status_color(s: Status) -> Color {
    match s {
        Status::Working => TEAL,
        Status::NeedsYou => AMBER,
        Status::Idle => MUTED,
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

/// What a label says about the avatar, as a colour family for its badge.
fn badge_color(label: &str) -> Color {
    let l = label.to_ascii_lowercase();
    if l.starts_with("reading") || l.starts_with("thinking") || l.contains("search") {
        SKY
    } else if l.starts_with("editing") || l.starts_with("running") || l.starts_with("writing") {
        TEAL
    } else if l.contains("coffee") || l.contains("couch") || l.contains("ping") || l.contains("hanging") || l.contains("seat") || l.contains("legs") {
        AMBER
    } else if l.contains("report") || l.contains("waiting for you") {
        EMBER
    } else {
        MUTED
    }
}

fn badge<'a>(label: &str) -> El<'a> {
    let c = badge_color(label);
    container(mono(clip(label, 34), 10.0, c)).padding([2, 7]).style(style::tinted(c)).into()
}

impl App {
    pub fn view(&self) -> El<'_> {
        let bg = container(Space::new().width(Fill).height(Fill)).style(|_| container::Style {
            background: Some(iced::Background::Gradient(iced::Gradient::Linear(
                iced::gradient::Linear::new(0.0).add_stop(0.0, iced::color!(0x253047)).add_stop(0.75, iced::color!(0x141a26)),
            ))),
            ..Default::default()
        });
        let cfg = self.config();
        let overlay = canvas(Overlay {
            scene: &self.scene,
            world: self.tab().map(|t| &t.world),
            cam: &self.cam,
            hover: self.hover,
            selected: self.selected,
            bubbles: cfg.bubbles,
            stage: self.stage,
        })
        .width(Fill)
        .height(Fill);
        let mut layers: Vec<El> = vec![bg.into()];
        if self.tab().is_some() {
            layers.push(self.view_office());
        }
        layers.push(overlay.into());
        layers.push(self.hud());
        if let Some(c) = self.inspector() {
            layers.push(c);
        }
        if let Some(c) = self.board_card() {
            layers.push(c);
        }
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

    fn hud(&self) -> El<'_> {
        let top = self.top_bar();
        let mut col = column![top].spacing(8).padding(Padding { top: 10.0, right: 12.0, bottom: 12.0, left: 12.0 });
        if let Some(s) = self.session() {
            col = col.push(self.pulse(s));
        }
        col = col.push(Space::new().height(Fill));
        if self.tab().is_some() {
            col = col.push(self.bottom());
        }
        col.width(Fill).height(Fill).into()
    }

    fn top_bar(&self) -> El<'_> {
        let mut bar = row![].spacing(8).align_y(Alignment::Center).height(40);
        if cfg!(target_os = "macos") {
            // the traffic lights live here
            bar = bar.push(Space::new().width(66));
        }
        let brand = row![dot(TEAL, 8.0), t("ShipOffice", 13.0, CREAM).font(UI_BOLD)].spacing(7).align_y(Alignment::Center);
        bar = bar.push(container(brand).padding([0, 6]));
        if !self.tabs.is_empty() {
            let tabs = row(self.tabs.iter().enumerate().map(|(i, tab)| {
                let s = self.sessions.get(&tab.key);
                let status = s.map_or(Status::Idle, |s| s.status);
                let name = s.map_or_else(|| "…".into(), |s| s.project());
                let sub = s.map_or_else(String::new, |s| format!("{} · {}", s.harness_name, s.branch.clone().unwrap_or_else(|| fmt_t(ago(s.started)))));
                let body = row![dot(status_color(status), 8.0), column![t(clip(&name, 22), 12.5, CREAM).font(UI_BOLD), mono(clip(&sub, 30), 10.0, MUTED)].spacing(0)].spacing(9).align_y(Alignment::Center);
                button(body).padding([5, 11]).style(style::tab(i == self.active)).on_press(Message::SelectTab(i)).into()
            }))
            .spacing(4);
            let strip = scrollable(tabs).direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::new().width(0).scroller_width(0))).style(style::hidden_scroll);
            bar = bar.push(opaque(container(strip).padding(3).style(style::glass(13.0)).max_width(820)));
        }
        // empty bar space moves the window; a double click maximises
        let grab = mouse_area(Space::new().width(Fill).height(40))
            .on_press(Message::Window(WindowAction::Drag))
            .on_double_click(Message::Window(WindowAction::ToggleMaximize));
        bar = bar.push(grab);
        if self.tab().is_some() {
            let zoom = row![
                icon_button(Icon::Minus, Message::Zoom(0.8), false),
                button(mono("Fit", 11.0, MUTED)).padding([6, 8]).style(style::seg(false)).on_press(Message::Fit),
                icon_button(Icon::Plus, Message::Zoom(1.25), false),
                icon_button(Icon::Shuffle, Message::NewLayout, false),
            ]
            .spacing(2)
            .align_y(Alignment::Center);
            bar = bar.push(opaque(container(zoom).padding(3).style(style::glass(11.0))));
        }
        bar = bar.push(opaque(container(icon_button(Icon::Settings, Message::ToggleSettings, self.settings_open)).padding(3).style(style::glass(11.0))));
        if cfg!(not(target_os = "macos")) {
            let w = row![
                button(container(icon(Icon::Minimize, 14.0, MUTED)).center(Fill)).width(34).height(28).padding(0).style(style::window_button(false)).on_press(Message::Window(WindowAction::Minimize)),
                button(container(icon(Icon::Maximize, 14.0, MUTED)).center(Fill)).width(34).height(28).padding(0).style(style::window_button(false)).on_press(Message::Window(WindowAction::ToggleMaximize)),
                button(container(icon(Icon::Close, 14.0, MUTED)).center(Fill)).width(34).height(28).padding(0).style(style::window_button(true)).on_press(Message::Window(WindowAction::Close)),
            ]
            .spacing(2);
            bar = bar.push(opaque(container(w).padding(3).style(style::glass(11.0))));
        }
        bar.into()
    }

    /// The session's vital signs as a row of pills.
    fn pulse(&self, s: &Session) -> El<'_> {
        let pill = |content: El<'static>| -> El<'static> { container(content).padding([4, 10]).style(style::pill).into() };
        let mut r = row![].spacing(6).align_y(Alignment::Center);
        let since = fmt_t(ago(s.status_since));
        r = r.push(pill(row![dot(status_color(s.status), 7.0), t(status_label(s.status), 11.5, CREAM).font(UI_MEDIUM), mono(since, 11.0, MUTED)].spacing(6).align_y(Alignment::Center).into()));
        r = r.push(pill(row![mono(fmt_t(ago(s.started)), 11.5, CREAM), t("session", 11.0, MUTED)].spacing(6).into()));
        let u = &s.usage;
        r = r.push(pill(row![mono(fmt_k(u.total_in()), 11.5, CREAM), t("in", 11.0, MUTED), mono(fmt_k(u.output), 11.5, CREAM), t("out", 11.0, MUTED)].spacing(5).into()));
        r = r.push(pill(row![mono(format!("${:.2}", s.cost_estimate()), 11.5, CREAM), t("est.", 11.0, MUTED)].spacing(5).into()));
        let ctx = (s.context as f32 / s.context_window() as f32).clamp(0.0, 1.0);
        let bar = container(container(Space::new()).width(Length::Fixed(46.0 * ctx)).height(4).style(style::dot(ROSE))).width(46).height(4).style(style::bar_track);
        r = r.push(pill(row![t("ctx", 11.0, MUTED), bar, mono(format!("{}%", (ctx * 100.0).round()), 11.5, CREAM)].spacing(6).align_y(Alignment::Center).into()));
        if let Some(m) = &s.model {
            r = r.push(pill(mono(clip(m, 26), 11.0, MUTED).into()));
        }
        if self.config().show_fps {
            r = r.push(pill(mono(format!("{:.0} fps", self.fps), 11.0, MUTED).into()));
        }
        let mut line = row![r].spacing(8).align_y(Alignment::Center);
        if let Some(tab) = self.tab() {
            let o = &tab.world.s.layout;
            line = line.push(Space::new().width(Fill));
            line = line.push(
                row![t(o.theme.name, 11.5, CREAM).font(UI_MEDIUM), mono(format!("seed {:06x} · {}×{} · {} desks", tab.seed & 0xffffff, o.w, o.h, o.desks.len()), 10.5, MUTED)]
                    .spacing(8)
                    .align_y(Alignment::Center),
            );
        }
        scrollable(line.width(Fill)).direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::new().width(0).scroller_width(0))).style(style::hidden_scroll).into()
    }

    fn bottom(&self) -> El<'_> {
        let Some(tab) = self.tab() else { return Space::new().into() };
        let w = &tab.world;
        let chips = row(w.avatars.iter().map(|a| {
            let sel = self.selected == Some(a.id);
            let body = row![dot(rgb8(a.color()), 8.0), column![t(clip(&a.name, 18), 11.5, CREAM).font(UI_MEDIUM), mono(clip(&a.label, 20), 9.5, MUTED)].spacing(0)].spacing(7).align_y(Alignment::Center);
            button(body).padding([5, 9]).style(style::chip(sel)).on_press(Message::Select(if sel { None } else { Some(a.id) })).into()
        }))
        .spacing(4);
        let dock = column![
            row![t("Crew", 10.5, MUTED).font(UI_MEDIUM), mono(format!("{}", w.avatars.len()), 10.5, CREAM)].spacing(6),
            scrollable(chips).direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::new().width(0).scroller_width(0))).style(style::hidden_scroll)
        ]
        .spacing(4);
        let dock = opaque(container(dock).padding([7, 10]).style(style::glass(14.0)).max_width(760));
        let feed = opaque(self.feed());
        row![dock, Space::new().width(Fill), feed].spacing(10).align_y(Alignment::End).into()
    }

    fn agent_color(&self, who: &str) -> Color {
        if who == "You" {
            return CREAM;
        }
        self.tab().and_then(|t| t.world.avatars.iter().find(|a| a.name == who)).map_or(MUTED, |a| rgb8(a.color()))
    }

    fn feed(&self) -> El<'_> {
        let head = button(
            row![t("Live feed", 12.0, CREAM).font(UI_BOLD), Space::new().width(Fill), mono("TRANSCRIPT", 9.5, MUTED), icon(Icon::Chevron(self.feed_open), 12.0, MUTED)]
                .spacing(8)
                .align_y(Alignment::Center),
        )
        .padding([8, 12])
        .width(Fill)
        .style(style::seg(false))
        .on_press(Message::ToggleFeed);
        let mut col = column![head].width(340);
        if self.feed_open {
            if let Some(s) = self.session() {
                let items = column(s.feed.iter().take(40).map(|f| {
                    let who = row![dot(self.agent_color(&f.who), 6.0), t(clip(&f.who, 22), 11.0, CREAM).font(UI_MEDIUM), mono(clip(&f.what, 14), 10.5, TEAL), Space::new().width(Fill), mono(fmt_ago(ago(f.at)), 9.5, MUTED)]
                        .spacing(6)
                        .align_y(Alignment::Center);
                    let mut c = column![who].spacing(1);
                    if !f.detail.is_empty() {
                        c = c.push(t(clip(&f.detail.replace('\n', " "), 90), 11.0, MUTED));
                    }
                    container(c).padding([5, 12]).into()
                }));
                let body: El = if s.feed.is_empty() { container(t("Nothing yet. Events show up here as they happen.", 11.5, MUTED)).padding([6, 12]).into() } else { items.into() };
                col = col.push(scrollable(body).height(Length::Fixed(220.0)).style(style::thin_scroll));
            }
        }
        container(col).padding(Padding { bottom: if self.feed_open { 6.0 } else { 0.0 }, ..Padding::ZERO }).style(style::glass(14.0)).into()
    }

    /// Hangs a card off a stage point, flipping sides so it stays on screen.
    fn hang<'a>(&self, card: El<'a>, sx: f32, sy: f32, w: f32, h: f32) -> El<'a> {
        let (sw, sh) = (self.stage.width, self.stage.height);
        let mut x = sx + 26.0;
        if x + w > sw - 10.0 {
            x = sx - w - 26.0;
        }
        x = x.clamp(10.0, (sw - w - 10.0).max(10.0));
        let y = (sy - h * 0.35).clamp(110.0, (sh - h - 96.0).max(110.0));
        pin(opaque(card)).x(x).y(y).width(Fill).height(Fill).into()
    }

    fn inspector(&self) -> Option<El<'_>> {
        let id = self.selected?;
        let tab = self.tab()?;
        let a = tab.world.avatar(id)?;
        let s = self.sessions.get(&tab.key)?;
        let st = s.agents.get(&a.agent);
        let h = self.scene.heads.iter().find(|h| h.id == id)?;
        let (sx, sy) = self.cam.to_screen(h.x, (h.top + h.bottom) / 2.0);
        let role = if a.lead { format!("Lead · {}", s.model.clone().unwrap_or_else(|| s.harness_name.clone())) } else { st.and_then(|st| st.kind.clone()).unwrap_or_else(|| "Helper".into()) };
        let head = row![
            container(t(a.name.chars().next().unwrap_or('?').to_uppercase().to_string(), 13.0, iced::color!(0x0d1b1a)).font(UI_BOLD)).center(30).style(style::dot(rgb8(a.color()))),
            column![t(clip(&a.name, 30), 13.5, CREAM).font(UI_BOLD), t(clip(&role, 36), 11.0, MUTED)].spacing(0),
            Space::new().width(Fill),
            icon_button(Icon::Close, Message::CloseCards, false)
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        let mut c = column![head, row![badge(&a.label)].spacing(6)].spacing(9);
        if let Some((_, said)) = &a.last_said {
            c = c.push(container(t(clip(said, 160), 12.0, CREAM)).padding([7, 9]).width(Fill).style(style::well));
        }
        let seat = match a.seated {
            Some(SeatRef::Desk(i)) => format!("Desk {}", i + 1),
            Some(SeatRef::Lounge(_)) => "Lounge".into(),
            None => "On the move".into(),
        };
        let kv = |k: &'static str, v: String| -> El<'static> { column![t(k, 10.5, MUTED), mono(v, 12.0, CREAM)].spacing(1).width(Fill).into() };
        let (tok, tools, since) = st.map_or((0, 0, 0), |st| (st.tokens, st.tools_used, ago(st.since)));
        c = c.push(row![kv("Tokens", fmt_k(tok)), kv("Tool calls", tools.to_string())].spacing(10));
        c = c.push(row![kv("Seat", seat), kv(if a.lead { "Session" } else { "Working for" }, fmt_t(if a.lead { ago(s.started) } else { since }))].spacing(10));
        let hist: Vec<El> = s.feed.iter().filter(|f| f.who == a.name && !f.what.is_empty()).take(5).map(|f| -> El { row![mono("›", 11.0, MUTED), mono(clip(&f.what, 12), 11.0, TEAL), t(clip(&f.detail, 34), 11.0, MUTED)].spacing(6).into() }).collect();
        if !hist.is_empty() {
            c = c.push(column(hist).spacing(3));
        }
        let follow = button(row![icon(Icon::Follow, 13.0, if self.follow { iced::color!(0x0d1b1a) } else { CREAM }), t("Follow", 12.0, if self.follow { iced::color!(0x0d1b1a) } else { CREAM })].spacing(6).align_y(Alignment::Center))
            .padding([5, 10])
            .style(style::primary(self.follow))
            .on_press(Message::Follow);
        c = c.push(row![follow, Space::new().width(Fill), icon_button(Icon::Left, Message::Step(-1), false), icon_button(Icon::Right, Message::Step(1), false)].spacing(4).align_y(Alignment::Center));
        let card = container(c).padding(12).width(280).style(style::card);
        Some(self.hang(card.into(), sx, sy, 280.0, 300.0))
    }

    fn board_card(&self) -> Option<El<'_>> {
        if !self.board_open {
            return None;
        }
        let tab = self.tab()?;
        let s = self.sessions.get(&tab.key)?;
        let o = &tab.world.s.layout;
        let (wx, wy) = iso((o.bx0 + o.bx1) as f32 / 2.0, 0.0);
        let (sx, sy) = self.cam.to_screen(wx + (o.bx1 - o.bx0) as f32 * TW / 4.0, wy - 70.0);
        let u = &s.usage;
        let cache = u.cache_read as f32 / u.total_in().max(1) as f32;
        let tools: u32 = s.tool_counts.values().sum();
        let cells: Vec<(&str, String)> = vec![
            ("Session", fmt_t(ago(s.started))),
            ("Status", status_label(s.status).into()),
            ("Tokens in", fmt_k(u.total_in())),
            ("Tokens out", fmt_k(u.output)),
            ("Cache hits", format!("{}%", (cache * 100.0).round())),
            ("Est. cost", format!("${:.2}", s.cost_estimate())),
            ("Tool calls", tools.to_string()),
            ("Files touched", s.files_touched.to_string()),
        ];
        let grid = column(cells.chunks(2).map(|pair| -> El { row(pair.iter().map(|(k, v)| -> El { column![t(*k, 10.5, MUTED), mono(v.clone(), 13.0, CREAM)].spacing(1).width(Fill).into() })).spacing(10).into() })).spacing(8);
        let ctx = (s.context as f32 / s.context_window() as f32).clamp(0.0, 1.0);
        let bar = container(container(Space::new()).width(Length::FillPortion(((ctx * 1000.0) as u16).max(1))).height(5).style(style::dot(ROSE))).width(Fill).height(5).style(style::bar_track);
        let sub = format!("{} · {}", s.project(), s.branch.clone().unwrap_or_else(|| s.harness_name.clone()));
        let c = column![
            row![column![t("Session board", 13.5, CREAM).font(UI_BOLD), t(clip(&sub, 40), 11.0, MUTED)], Space::new().width(Fill), icon_button(Icon::Close, Message::CloseCards, false)].align_y(Alignment::Center),
            grid,
            column![row![t("Context window", 10.5, MUTED), Space::new().width(Fill), mono(format!("{}% of {}", (ctx * 100.0).round(), fmt_k(s.context_window())), 10.5, CREAM)], bar].spacing(4),
            t(format!("{} · {}", s.harness_name, s.model.clone().unwrap_or_default()), 10.5, MUTED),
        ]
        .spacing(10);
        let card = container(c).padding(12).width(300).style(style::card);
        Some(self.hang(card.into(), sx, sy, 300.0, 260.0))
    }

    fn empty_state(&self) -> El<'_> {
        let watched: Vec<El> = self
            .harnesses
            .iter()
            .map(|h| -> El {
                let root = h.roots.first().map(|p| tidy_path(p)).unwrap_or_default();
                row![dot(if h.found > 0 { TEAL } else { style::LINE }, 7.0), t(h.name, 12.0, CREAM).width(150), mono(clip(&root, 40), 10.5, MUTED)].spacing(8).align_y(Alignment::Center).into()
            })
            .collect();
        let c = column![
            t("No live sessions yet", 18.0, CREAM).font(UI_BOLD),
            t("Start a coding agent in any project and its office opens here within a few seconds. ShipOffice watches these folders:", 12.5, MUTED),
            column(watched).spacing(5),
            row![button(t("Settings", 12.0, CREAM)).padding([6, 12]).style(style::ghost).on_press(Message::ToggleSettings), button(t("Show the demo crew", 12.0, iced::color!(0x0d1b1a))).padding([6, 12]).style(style::primary(true)).on_press(Message::Setting(Setting::Demo(true)))].spacing(8),
        ]
        .spacing(12)
        .max_width(520);
        container(opaque(container(c).padding(20).style(style::card))).center(Fill).into()
    }

    fn settings(&self) -> El<'_> {
        let cfg = self.config();
        let section = |s: &'static str| -> El<'static> { mono(s, 10.0, MUTED).into() };
        let mut c = column![row![t("Settings", 16.0, CREAM).font(UI_BOLD), Space::new().width(Fill), icon_button(Icon::Close, Message::ToggleSettings, false)].align_y(Alignment::Center)].spacing(12);
        c = c.push(section("HARNESSES"));
        for h in &self.harnesses {
            let hc = cfg.harness(h.id);
            let found = if h.found > 0 { format!("{} live", h.found) } else { "nothing live".into() };
            let id = h.id.to_owned();
            let top = row![
                toggler(hc.enabled).on_toggle(move |v| Message::Setting(Setting::Harness(id.clone(), v))).size(16).style(style::toggle),
                t(h.name, 12.5, CREAM).font(UI_MEDIUM),
                Space::new().width(Fill),
                mono(found, 10.0, if h.found > 0 { TEAL } else { MUTED })
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            let mut hcol = column![top].spacing(3);
            if !hc.replace_defaults {
                for r in h.roots.iter().take(2) {
                    hcol = hcol.push(container(mono(clip(&tidy_path(r), 46), 10.0, MUTED)).padding(Padding { left: 40.0, ..Padding::ZERO }));
                }
            }
            for (i, p) in hc.extra_paths.iter().enumerate() {
                let id = h.id.to_owned();
                hcol = hcol.push(
                    row![Space::new().width(40), mono(clip(&tidy_path(p), 40), 10.0, CREAM), Space::new().width(Fill), button(icon(Icon::Close, 11.0, MUTED)).padding(2).style(style::seg(false)).on_press(Message::Setting(Setting::RemovePath(id, i)))]
                        .align_y(Alignment::Center),
                );
            }
            c = c.push(hcol);
        }
        let choices: Vec<HarnessChoice> = self.harnesses.iter().map(|h| HarnessChoice { id: h.id.into(), name: h.name.into() }).collect();
        c = c.push(
            column![
                t("Watch another folder", 11.5, CREAM).font(UI_MEDIUM),
                row![
                    pick_list(choices, self.path_harness.clone(), |h| Message::Setting(Setting::PathHarness(h))).text_size(11.5).padding([5, 8]).width(130),
                    text_input("~/path/to/logs", &self.path_input).on_input(|s| Message::Setting(Setting::PathInput(s))).on_submit(Message::Setting(Setting::AddPath)).size(11.5).padding([5, 8]).style(style::input).font(MONO),
                    button(t("Add", 11.5, CREAM)).padding([5, 10]).style(style::ghost).on_press(Message::Setting(Setting::AddPath)),
                ]
                .spacing(6)
                .align_y(Alignment::Center)
            ]
            .spacing(5),
        );
        c = c.push(section("OFFICE"));
        let switch = |label: &'static str, on: bool, f: fn(bool) -> Setting| -> El<'static> {
            row![t(label, 12.0, CREAM), Space::new().width(Fill), toggler(on).on_toggle(move |v| Message::Setting(f(v))).size(16).style(style::toggle)].align_y(Alignment::Center).into()
        };
        c = c.push(switch("Speech bubbles", cfg.bubbles, Setting::Bubbles));
        c = c.push(switch("Demo crew when nothing is live", cfg.demo_when_empty, Setting::Demo));
        c = c.push(switch("Show frame rate", cfg.show_fps, Setting::ShowFps));
        let fps = row([12u32, 16, 24].into_iter().map(|v| -> El { button(mono(format!("{v}"), 11.0, if cfg.anim_fps == v { CREAM } else { MUTED })).padding([4, 9]).style(style::seg(cfg.anim_fps == v)).on_press(Message::Setting(Setting::AnimFps(v))).into() })).spacing(2);
        c = c.push(row![t("Animation frame rate", 12.0, CREAM), Space::new().width(Fill), container(fps).padding(2).style(style::glass(9.0))].align_y(Alignment::Center));
        c = c.push(
            column![
                row![t("Show sessions active in the last", 12.0, CREAM), Space::new().width(Fill), mono(format!("{} min", cfg.active_minutes), 11.5, CREAM)],
                slider(5.0..=240.0, cfg.active_minutes as f64, |v| Message::Setting(Setting::ActiveMinutes(v.round() as u64))).step(5.0).style(style::range)
            ]
            .spacing(6),
        );
        c = c.push(
            column![
                row![t("Most tabs at once", 12.0, CREAM), Space::new().width(Fill), mono(format!("{}", cfg.max_tabs), 11.5, CREAM)],
                slider(1.0..=12.0, cfg.max_tabs as f64, |v| Message::Setting(Setting::MaxTabs(v.round() as usize))).step(1.0).style(style::range)
            ]
            .spacing(6),
        );
        c = c.push(section("SHORTCUTS"));
        c = c.push(mono("1–9 tabs · Tab next agent · F follow · +/− zoom · 0 fit · Esc close", 10.5, MUTED));
        let path = crate::config::Config::path().map(|p| tidy_path(&p)).unwrap_or_default();
        c = c.push(row![mono(clip(&path, 40), 10.0, MUTED), Space::new().width(Fill), button(t("Open folder", 11.5, CREAM)).padding([5, 10]).style(style::ghost).on_press(Message::Setting(Setting::OpenConfigFolder))].align_y(Alignment::Center));
        let panel = container(scrollable(container(c).padding(Padding { right: 10.0, ..Padding::ZERO })).style(style::thin_scroll)).padding(16).width(380).max_height((self.stage.height - 80.0).max(200.0)).style(style::card);
        let shade = mouse_area(container(Space::new()).width(Fill).height(Fill).style(|_| container::Style { background: Some(iced::Background::Color(alpha(Color::BLACK, 0.25))), ..Default::default() })).on_press(Message::ToggleSettings);
        stack![shade, container(opaque(panel)).align_right(Fill).padding(Padding { top: 60.0, right: 12.0, bottom: 12.0, left: 12.0 })].into()
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
    let g = 5.0;
    let edge = |w: Length, h: Length, dir: D, i: mouse::Interaction| -> El<'a> { mouse_area(Space::new().width(w).height(h)).on_press(Message::Window(WindowAction::Resize(dir))).interaction(i).into() };
    let (ew, ns, nwse, nesw) = (mouse::Interaction::ResizingHorizontally, mouse::Interaction::ResizingVertically, mouse::Interaction::ResizingDiagonallyDown, mouse::Interaction::ResizingDiagonallyUp);
    let fx = Length::Fixed(g);
    column![
        row![edge(fx, fx, D::NorthWest, nwse), edge(Fill, fx, D::North, ns), edge(fx, fx, D::NorthEast, nesw)],
        row![edge(fx, Fill, D::West, ew), Space::new().width(Fill).height(Fill), edge(fx, Fill, D::East, ew)].height(Fill),
        row![edge(fx, fx, D::SouthWest, nesw), edge(Fill, fx, D::South, ns), edge(fx, fx, D::SouthEast, nwse)],
    ]
    .into()
}

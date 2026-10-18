//! The window: one office per live session in tabs, the HUD floating over it, and the loop
//! that feeds watcher events into sessions and sessions into the little people.

pub mod demo;
pub mod hud;
pub mod icon;
pub mod overlay;
pub mod style;

use crate::config::Config;
use crate::harness;
use crate::model::{Event, SessionKey};
use crate::office::gen::{Atlas, Layout};
use crate::office::world::{SeatRef, World};
use crate::office::{hash, TW};
use crate::render::paint::{self, BoardStats};
use crate::render::scene::{self, Hit, Scene};
use crate::render::{assets, gpu, Bounds, Camera, Layer};
use crate::session::{Session, Status};
use crate::sources::{self, Update};
use demo::Demo;
use iced::widget::shader;
use iced::{keyboard, mouse, window, Point, Rectangle, Size, Subscription, Task, Vector};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

/// Room the floating bars take at the top and bottom of the stage.
const PAD_TOP: f32 = 104.0;
const PAD_BOTTOM: f32 = 96.0;

#[derive(Debug, Clone)]
pub enum Message {
    Tick(Instant),
    Stage(Size),
    ScaleFactor(f32),
    SelectTab(usize),
    Hover(Option<Hit>),
    Click(Option<Hit>),
    Pan(Vector),
    ZoomAt(Point, f32),
    Zoom(f32),
    Fit,
    Select(Option<u32>),
    Step(i32),
    Follow,
    CloseCards,
    ToggleFeed,
    ToggleSettings,
    NewLayout,
    Key(keyboard::Event),
    Window(WindowAction),
    Setting(Setting),
    /// debug: `SHIPOFFICE_SCREENSHOT=path.png` saves the window after a few seconds and quits
    Shot(window::Screenshot),
}

#[derive(Debug, Clone)]
pub enum WindowAction {
    Drag,
    Minimize,
    ToggleMaximize,
    Close,
    Resize(window::Direction),
}

#[derive(Debug, Clone)]
pub enum Setting {
    Harness(String, bool),
    Bubbles(bool),
    Demo(bool),
    ShowFps(bool),
    AnimFps(u32),
    ActiveMinutes(u64),
    MaxTabs(usize),
    PathInput(String),
    PathHarness(HarnessChoice),
    AddPath,
    RemovePath(String, usize),
    OpenConfigFolder,
}

/// A harness as the settings list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessChoice {
    pub id: String,
    pub name: String,
}

impl std::fmt::Display for HarnessChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

pub struct HarnessInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub roots: Vec<PathBuf>,
    pub found: usize,
}

pub struct Tab {
    pub key: SessionKey,
    pub world: World,
    uid: u64,
    pub seed: u32,
    static_layer: Option<Layer>,
    static_res: f32,
    building: bool,
    board: Option<Layer>,
    board_stats: Option<BoardStats>,
}

pub struct App {
    cfg: Arc<RwLock<Config>>,
    rx: Receiver<Update>,
    pub sessions: BTreeMap<SessionKey, Session>,
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub cam: Camera,
    pub stage: Size,
    scale_factor: f32,
    pub scene: Scene,
    pub hover: Option<Hit>,
    pub selected: Option<u32>,
    pub follow: bool,
    pub board_open: bool,
    pub feed_open: bool,
    pub settings_open: bool,
    pub harnesses: Vec<HarnessInfo>,
    pub path_input: String,
    pub path_harness: Option<HarnessChoice>,
    demo: Option<Demo>,
    last_tick: Option<Instant>,
    last_second: Instant,
    zoomed_at: Instant,
    pub fps: f32,
    started: Instant,
    jobs_tx: Sender<(u64, f32, image::RgbaImage)>,
    jobs_rx: Receiver<(u64, f32, image::RgbaImage)>,
    next_uid: u64,
    shot_taken: bool,
}

pub fn run() -> iced::Result {
    let mut win = window::Settings { size: Size::new(1320.0, 840.0), min_size: Some(Size::new(760.0, 520.0)), icon: icon::app_icon(), ..Default::default() };
    // native chrome without a title bar: macOS keeps its traffic lights over the HUD,
    // elsewhere the HUD draws its own window controls and the edges resize the window
    #[cfg(target_os = "macos")]
    {
        win.platform_specific = window::settings::PlatformSpecific { title_hidden: true, titlebar_transparent: true, fullsize_content_view: true };
    }
    #[cfg(not(target_os = "macos"))]
    {
        win.decorations = false;
    }
    iced::application(App::boot, App::update, App::view)
        .title(|_: &App| String::from("ShipOffice"))
        .theme(|_: &App| style::theme())
        .subscription(App::subscription)
        .window(win)
        .font(assets::FONT_UI)
        .font(assets::FONT_UI_MEDIUM)
        .font(assets::FONT_UI_SEMIBOLD)
        .font(assets::FONT_MONO)
        .default_font(style::UI)
        .antialiasing(true)
        .run()
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let cfg = Arc::new(RwLock::new(Config::load()));
        let rx = sources::spawn(cfg.clone());
        let harnesses = harness::all().iter().map(|a| HarnessInfo { id: a.id(), name: a.name(), roots: a.default_roots(), found: 0 }).collect::<Vec<_>>();
        let path_harness = harnesses.first().map(|h| HarnessChoice { id: h.id.into(), name: h.name.into() });
        let (jobs_tx, jobs_rx) = std::sync::mpsc::channel();
        let app = Self {
            cfg,
            rx,
            sessions: BTreeMap::new(),
            tabs: Vec::new(),
            active: 0,
            cam: Camera::new(),
            stage: Size::new(1320.0, 840.0),
            scale_factor: 1.0,
            scene: Scene::default(),
            hover: None,
            selected: None,
            follow: false,
            board_open: false,
            feed_open: true,
            settings_open: false,
            harnesses,
            path_input: String::new(),
            path_harness,
            demo: None,
            last_tick: None,
            last_second: Instant::now(),
            zoomed_at: Instant::now(),
            fps: 0.0,
            started: Instant::now(),
            jobs_tx,
            jobs_rx,
            next_uid: 1,
            shot_taken: false,
        };
        (app, window::latest().and_then(window::scale_factor).map(Message::ScaleFactor))
    }

    pub fn config(&self) -> Config {
        self.cfg.read().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn session(&self) -> Option<&Session> {
        self.tab().and_then(|t| self.sessions.get(&t.key))
    }

    fn subscription(&self) -> Subscription<Message> {
        let fps = 30;
        Subscription::batch([
            iced::time::every(Duration::from_millis(1000 / fps)).map(Message::Tick),
            keyboard::listen().map(Message::Key),
        ])
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::Tick(now) => {
                self.tick(now);
                return self.debug_shot();
            }
            Message::Shot(shot) => {
                if let Ok(path) = std::env::var("SHIPOFFICE_SCREENSHOT") {
                    let (w, h) = (shot.size.width, shot.size.height);
                    if let Some(img) = image::RgbaImage::from_raw(w, h, shot.rgba.to_vec()) {
                        let _ = img.save(path);
                    }
                }
                return iced::exit();
            }
            Message::Stage(size) => {
                self.stage = size;
                self.relayout(false);
            }
            Message::ScaleFactor(s) => self.scale_factor = s.max(0.5),
            Message::SelectTab(i) => {
                if i < self.tabs.len() && i != self.active {
                    self.active = i;
                    self.selected = None;
                    self.hover = None;
                    self.board_open = false;
                    self.follow = false;
                    self.relayout(true);
                }
            }
            Message::Hover(h) => self.hover = h,
            Message::Click(hit) => self.click(hit),
            Message::Pan(d) => {
                self.follow = false;
                self.cam.pan(d.x, d.y);
                self.zoomed_at = Instant::now();
            }
            Message::ZoomAt(p, f) => {
                self.cam.zoom_at(p.x, p.y, f);
                self.zoomed_at = Instant::now();
            }
            Message::Zoom(f) => {
                self.cam.zoom_at(self.stage.width / 2.0, self.stage.height / 2.0, f);
                self.zoomed_at = Instant::now();
            }
            Message::Fit => {
                self.follow = false;
                self.relayout(true);
            }
            Message::Select(id) => {
                self.selected = id;
                self.board_open = false;
                if id.is_none() {
                    self.follow = false;
                }
            }
            Message::Step(d) => self.step_selection(d),
            Message::Follow => self.follow = !self.follow && self.selected.is_some(),
            Message::CloseCards => {
                self.selected = None;
                self.board_open = false;
                self.follow = false;
            }
            Message::ToggleFeed => self.feed_open = !self.feed_open,
            Message::ToggleSettings => self.settings_open = !self.settings_open,
            Message::NewLayout => self.new_layout(),
            Message::Key(k) => return self.key(k),
            Message::Window(a) => {
                return match a {
                    WindowAction::Drag => window::latest().and_then(window::drag),
                    WindowAction::Minimize => window::latest().and_then(|id| window::minimize(id, true)),
                    WindowAction::ToggleMaximize => window::latest().and_then(window::toggle_maximize),
                    WindowAction::Close => window::latest().and_then(window::close),
                    WindowAction::Resize(dir) => window::latest().and_then(move |id| window::drag_resize(id, dir)),
                }
            }
            Message::Setting(s) => self.setting(s),
        }
        Task::none()
    }

    fn debug_shot(&mut self) -> Task<Message> {
        let Ok(after) = std::env::var("SHIPOFFICE_SCREENSHOT_AFTER").map(|v| v.parse::<f32>().unwrap_or(6.0)).or_else(|_| std::env::var("SHIPOFFICE_SCREENSHOT").map(|_| 6.0)) else { return Task::none() };
        if self.shot_taken || self.started.elapsed().as_secs_f32() < after {
            return Task::none();
        }
        self.shot_taken = true;
        window::latest().and_then(window::screenshot).map(Message::Shot)
    }

    fn key(&mut self, k: keyboard::Event) -> Task<Message> {
        use keyboard::key::Named;
        let keyboard::Event::KeyPressed { key, modifiers, .. } = k else { return Task::none() };
        // typing in the settings field is not a shortcut
        if self.settings_open && !matches!(key, keyboard::Key::Named(Named::Escape)) && !modifiers.command() {
            return Task::none();
        }
        match key.as_ref() {
            keyboard::Key::Named(Named::Escape) => {
                if self.settings_open {
                    self.settings_open = false;
                } else {
                    return self.update(Message::CloseCards);
                }
            }
            keyboard::Key::Named(Named::Tab) => self.step_selection(if modifiers.shift() { -1 } else { 1 }),
            keyboard::Key::Named(Named::ArrowRight) if modifiers.command() => return self.update(Message::SelectTab((self.active + 1) % self.tabs.len().max(1))),
            keyboard::Key::Named(Named::ArrowLeft) if modifiers.command() => return self.update(Message::SelectTab((self.active + self.tabs.len().max(1) - 1) % self.tabs.len().max(1))),
            keyboard::Key::Character(",") if modifiers.command() => self.settings_open = !self.settings_open,
            keyboard::Key::Character("+" | "=") => return self.update(Message::Zoom(1.25)),
            keyboard::Key::Character("-") => return self.update(Message::Zoom(0.8)),
            keyboard::Key::Character("0") => return self.update(Message::Fit),
            keyboard::Key::Character("f") => return self.update(Message::Follow),
            keyboard::Key::Character(c) => {
                if let Some(n) = c.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) {
                    return self.update(Message::SelectTab(n - 1));
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn step_selection(&mut self, d: i32) {
        let Some(t) = self.tab() else { return };
        let ids: Vec<u32> = t.world.avatars.iter().map(|a| a.id).collect();
        if ids.is_empty() {
            return;
        }
        let i = self.selected.and_then(|s| ids.iter().position(|&x| x == s)).map_or(0, |i| (i as i32 + d).rem_euclid(ids.len() as i32) as usize);
        self.selected = Some(ids[i]);
        self.board_open = false;
    }

    fn click(&mut self, hit: Option<Hit>) {
        match hit {
            Some(Hit::Agent(id)) => {
                self.selected = if self.selected == Some(id) { None } else { Some(id) };
                self.board_open = false;
            }
            Some(Hit::Board) => {
                self.board_open = !self.board_open;
                self.selected = None;
            }
            Some(Hit::Prop(i)) => {
                // furniture points at whoever is using it
                let Some(t) = self.tab() else { return };
                let w = &t.world;
                let p = &w.s.layout.props[i];
                let user = match p.atlas {
                    Atlas::Desk => w.s.layout.desks.iter().position(|d| d.x == p.x && d.y == p.y).and_then(|d| w.s.desk_owner[d]),
                    _ if p.name == "pingpong" => w.s.game.as_ref().map(|g| g.a),
                    _ if matches!(p.name, "sofa" | "table" | "chairL" | "chairR") => w.avatars.iter().find(|a| matches!(a.seated, Some(SeatRef::Lounge(_)))).map(|a| a.id),
                    _ => None,
                };
                self.selected = user;
                self.board_open = false;
            }
            None => {
                self.selected = None;
                self.board_open = false;
                self.follow = false;
            }
        }
    }

    fn relayout(&mut self, fresh: bool) {
        let Some(t) = self.tabs.get(self.active) else { return };
        let b = Bounds::of(&t.world.s.layout);
        let narrow = self.stage.width < 900.0;
        let fresh = fresh || self.cam.bounds != b;
        self.cam.layout(b, self.stage.width, self.stage.height, if narrow { PAD_TOP + 40.0 } else { PAD_TOP }, PAD_BOTTOM, fresh);
    }

    fn harness_name(&self, id: &str) -> String {
        if id == "demo" {
            return "Demo".into();
        }
        self.harnesses.iter().find(|h| h.id == id).map_or_else(|| id.to_owned(), |h| h.name.to_owned())
    }

    fn apply(&mut self, events: Vec<Event>) {
        for e in events {
            let name = self.harness_name(e.session.harness);
            let s = self.sessions.entry(e.session.clone()).or_insert_with(|| Session::new(e.session.clone(), &name, e.at));
            s.apply(&e);
            if let Some(t) = self.tabs.iter_mut().find(|t| t.key == e.session) {
                t.world.cue(&e);
            }
        }
    }

    fn seed_for(&self, s: &Session) -> u32 {
        let project = s.project();
        let cfg = self.config();
        cfg.layouts.get(&project).copied().unwrap_or_else(|| hash(&s.cwd.as_ref().map_or_else(|| project.clone(), |p| p.to_string_lossy().into_owned())))
    }

    /// Which sessions get an office: the recently active ones, newest first, up to the tab limit.
    /// Tabs keep their order so nothing jumps while you watch.
    fn refresh_tabs(&mut self) {
        let cfg = self.config();
        let now = SystemTime::now();
        let window = Duration::from_secs(cfg.active_minutes.max(1) * 60);
        let fresh = |s: &Session| s.status == Status::Working || now.duration_since(s.last).unwrap_or_default() < window;
        let real_live = self.sessions.values().any(|s| !Demo::is_demo(&s.key) && fresh(s));
        // the demo crew only fills an empty building
        if !real_live && cfg.demo_when_empty && self.demo.is_none() {
            let d = Demo::new(hash("shipoffice-demo"));
            let intro = d.intro();
            self.demo = Some(d);
            self.apply(intro);
        } else if (real_live || !cfg.demo_when_empty) && self.demo.is_some() {
            self.demo = None;
            self.sessions.retain(|k, _| !Demo::is_demo(k));
        }
        let disabled: Vec<String> = cfg.harness.iter().filter(|(_, h)| !h.enabled).map(|(k, _)| k.clone()).collect();
        self.sessions.retain(|k, _| !disabled.iter().any(|d| d == k.harness));
        let mut live: Vec<&Session> = self.sessions.values().filter(|s| fresh(s)).collect();
        live.sort_by(|a, b| b.last.cmp(&a.last));
        live.truncate(cfg.max_tabs.max(1));
        let keep: Vec<SessionKey> = live.iter().map(|s| s.key.clone()).collect();
        let active_key = self.tab().map(|t| t.key.clone());
        self.tabs.retain(|t| keep.contains(&t.key));
        for s in live.iter().rev() {
            if !self.tabs.iter().any(|t| t.key == s.key) {
                let seed = self.seed_for(s);
                let uid = self.next_uid;
                self.next_uid += 1;
                let world = World::new(&s.project(), seed);
                self.tabs.push(Tab { key: s.key.clone(), world, uid, seed, static_layer: None, static_res: 0.0, building: false, board: None, board_stats: None });
            }
        }
        let before = self.active;
        self.active = active_key.and_then(|k| self.tabs.iter().position(|t| t.key == k)).unwrap_or(0);
        if before != self.active || self.cam.bounds == Bounds::default() {
            self.selected = None;
            self.relayout(true);
        }
    }

    fn new_layout(&mut self) {
        let Some(key) = self.tab().map(|t| t.key.clone()) else { return };
        let Some(s) = self.sessions.get(&key) else { return };
        let project = s.project();
        let seed = hash(&format!("{project}:{}", self.started.elapsed().as_nanos()));
        if let Ok(mut c) = self.cfg.write() {
            c.layouts.insert(project.clone(), seed);
            let _ = c.save();
        }
        let uid = self.next_uid;
        self.next_uid += 1;
        if let Some(t) = self.tabs.get_mut(self.active) {
            *t = Tab { key, world: World::new(&project, seed), uid, seed, static_layer: None, static_res: 0.0, building: false, board: None, board_stats: None };
        }
        self.selected = None;
        self.relayout(true);
    }

    fn tick(&mut self, now: Instant) {
        let dt = self.last_tick.map_or(0.0, |t| (now - t).as_secs_f32()).min(0.1);
        self.last_tick = Some(now);
        if dt > 0.0 {
            self.fps = if self.fps == 0.0 { 1.0 / dt } else { self.fps * 0.93 + 0.07 / dt };
        }
        let mut batch = Vec::new();
        while let Ok(u) = self.rx.try_recv() {
            match u {
                Update::Events(e) => batch.extend(e),
                Update::Harnesses(found) => {
                    for h in &mut self.harnesses {
                        h.found = found.iter().find(|f| f.0 == h.id).map_or(0, |f| f.2);
                    }
                }
            }
        }
        if let Some(d) = self.demo.as_mut() {
            batch.extend(d.step(dt));
        }
        let had_events = !batch.is_empty();
        self.apply(batch);
        if had_events && self.tabs.is_empty() || self.last_second.elapsed() >= Duration::from_secs(1) {
            self.last_second = Instant::now();
            let wall = SystemTime::now();
            for s in self.sessions.values_mut() {
                s.tick(wall);
            }
            self.refresh_tabs();
        }
        for t in &mut self.tabs {
            if let Some(s) = self.sessions.get(&t.key) {
                t.world.sync(s);
            }
            t.world.step(dt);
        }
        if self.selected.is_some_and(|id| self.tab().is_none_or(|t| t.world.avatar(id).is_none())) {
            self.selected = None;
            self.follow = false;
        }
        if self.follow {
            if let Some(h) = self.selected.and_then(|id| self.scene.heads.iter().find(|h| h.id == id)).copied() {
                let e = (dt * 3.5).min(1.0);
                self.cam.z += (1.9 - self.cam.z) * e;
                self.cam.cx += (h.x - self.cam.cx) * e;
                self.cam.cy += ((h.top + h.bottom) / 2.0 - self.cam.cy) * e;
                self.cam.clamp();
                self.zoomed_at = now;
            }
        }
        self.layers();
        self.build_scene();
    }

    /// Keeps the active office's painted layers current: the floor at a resolution that stays
    /// sharp at this zoom (repainted off the UI thread), and the whiteboard's numbers.
    fn layers(&mut self) {
        while let Ok((uid, res, img)) = self.jobs_rx.try_recv() {
            if let Some(t) = self.tabs.iter_mut().find(|t| t.uid == uid) {
                let version = t.static_layer.as_ref().map_or(1, |l| l.version + 1);
                t.static_layer = Some(Layer { id: uid * 2, version, image: Arc::new(img) });
                t.static_res = res;
                t.building = false;
            }
        }
        let scale = self.cam.scale() * self.scale_factor;
        let settled = self.zoomed_at.elapsed() > Duration::from_millis(160);
        let stats = self.session().map(board_stats);
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        let b = Bounds::of(&t.world.s.layout);
        let want = paint::static_resolution(b, scale);
        let stale = t.static_layer.is_none() || ((want - t.static_res).abs() / t.static_res.max(0.01) > 0.18 && settled);
        if stale && !t.building {
            t.building = true;
            let (layout, tx, uid): (Layout, _, _) = (t.world.s.layout.clone(), self.jobs_tx.clone(), t.uid);
            std::thread::spawn(move || {
                let img = paint::static_layer(&layout, want);
                let _ = tx.send((uid, want, img));
            });
        }
        if let Some(st) = stats {
            if t.board_stats.as_ref() != Some(&st) {
                let o = &t.world.s.layout;
                let span = (o.bx1 - o.bx0) as f32 * TW / 2.0;
                let img = paint::board(span, &st);
                let version = t.board.as_ref().map_or(1, |l| l.version + 1);
                t.board = Some(Layer { id: t.uid * 2 + 1, version, image: Arc::new(img) });
                t.board_stats = Some(st);
            }
        }
    }

    fn build_scene(&mut self) {
        let cfg_fps = self.config().anim_fps.clamp(4, 30) as f32;
        let Some(t) = self.tabs.get(self.active) else {
            self.scene = Scene::default();
            return;
        };
        self.scene = scene::build(&scene::Inputs {
            world: &t.world,
            static_layer: t.static_layer.as_ref(),
            board: t.board.as_ref(),
            hover: self.hover,
            selected: self.selected,
            anim_fps: cfg_fps,
            now: self.started.elapsed().as_secs_f32(),
        });
    }

    fn setting(&mut self, s: Setting) {
        let mut save = true;
        {
            let Ok(mut c) = self.cfg.write() else { return };
            match s {
                Setting::Harness(id, on) => c.harness.entry(id).or_default().enabled = on,
                Setting::Bubbles(v) => c.bubbles = v,
                Setting::Demo(v) => c.demo_when_empty = v,
                Setting::ShowFps(v) => c.show_fps = v,
                Setting::AnimFps(v) => c.anim_fps = v,
                Setting::ActiveMinutes(v) => c.active_minutes = v,
                Setting::MaxTabs(v) => c.max_tabs = v,
                Setting::PathInput(v) => {
                    self.path_input = v;
                    save = false;
                }
                Setting::PathHarness(h) => {
                    self.path_harness = Some(h);
                    save = false;
                }
                Setting::AddPath => {
                    let p = self.path_input.trim();
                    match self.path_harness.as_ref() {
                        Some(h) if !p.is_empty() => {
                            let path = expand_home(p);
                            let hc = c.harness.entry(h.id.clone()).or_default();
                            if !hc.extra_paths.contains(&path) {
                                hc.extra_paths.push(path);
                            }
                            self.path_input.clear();
                        }
                        _ => save = false,
                    }
                }
                Setting::RemovePath(id, i) => {
                    if let Some(hc) = c.harness.get_mut(&id) {
                        if i < hc.extra_paths.len() {
                            hc.extra_paths.remove(i);
                        }
                    }
                }
                Setting::OpenConfigFolder => {
                    save = false;
                    if let Some(dir) = Config::path().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
                        let _ = std::fs::create_dir_all(&dir);
                        open_folder(&dir);
                    }
                }
            }
            if save {
                let _ = c.save();
            }
        }
        self.last_second = Instant::now() - Duration::from_secs(2);
    }

    pub fn view_office(&self) -> iced::Element<'_, Message> {
        shader(OfficeShader { scene: &self.scene, cam: &self.cam }).width(iced::Fill).height(iced::Fill).into()
    }
}

/// The session numbers the whiteboard shows.
fn board_stats(s: &Session) -> BoardStats {
    let now = SystemTime::now();
    let secs = |t: SystemTime| now.duration_since(t).unwrap_or_default().as_secs();
    let mut tools = [0u32; 5];
    for (name, n) in &s.tool_counts {
        let l = name.to_ascii_lowercase();
        let i = if l.contains("task") || l.contains("agent") || l.contains("spawn") {
            3
        } else {
            match crate::model::activity_for_tool(name) {
                crate::model::Activity::Reading => 0,
                crate::model::Activity::Typing => 1,
                crate::model::Activity::Running => 2,
                _ => 4,
            }
        };
        tools[i] += n;
    }
    let u = &s.usage;
    let total_in = u.total_in().max(1);
    BoardStats {
        elapsed: secs(s.started),
        working: s.status == Status::Working,
        waiting: secs(s.status_since),
        tok_in: u.total_in(),
        tok_out: u.output,
        cache: u.cache_read as f32 / total_in as f32,
        files: s.files_touched,
        ctx: (s.context as f32 / s.context_window() as f32).min(1.0),
        tools,
    }
}

fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        Some(rest) => harness::home().join(rest),
        None => PathBuf::from(p),
    }
}

fn open_folder(dir: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(dir).spawn();
}

struct OfficeShader<'a> {
    scene: &'a Scene,
    cam: &'a Camera,
}

impl shader::Program<Message> for OfficeShader<'_> {
    type State = ();
    type Primitive = gpu::Primitive;

    fn draw(&self, _: &(), _: mouse::Cursor, _: Rectangle) -> gpu::Primitive {
        gpu::Primitive { list: self.scene.list.clone(), scale: self.cam.scale(), origin: self.cam.origin() }
    }
}

/// Labels the HUD uses for counts of agents in a world.
pub fn crew_count(w: &World) -> usize {
    w.avatars.len()
}

pub type Colors = HashMap<String, iced::Color>;

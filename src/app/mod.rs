//! The window: one office per live session in tabs, the HUD floating over it, and the loop
//! that feeds watcher events into sessions and sessions into the little people.

pub mod demo;
pub mod hud;
pub mod icon;
pub mod overlay;
pub mod popover;
pub mod portrait;
pub mod share;
pub mod style;

use crate::config::Config;
use crate::harness;
use crate::model::{Event, SessionKey};
use crate::office::gen::{Atlas, Layout};
use crate::office::world::{SeatRef, World};
use crate::office::{hash, TW};
use crate::render::paint::{self, BoardStats};
use crate::render::scene::{self, Hit, Scene};
use crate::render::view::{self, View};
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
/// (harness, project, model)
type SettingsKey = (String, Option<PathBuf>, Option<String>);

const PAD_TOP: f32 = 108.0;
const PAD_BOTTOM: f32 = 72.0;

#[derive(Debug, Clone)]
pub enum Message {
    Tick(Instant),
    /// the window is shown (true) or minimized (false)
    Visible(bool),
    Stage(Size),
    ScaleFactor(f32),
    SelectTab(usize),
    Hover(Option<Hit>),
    Click(Option<Hit>),
    Pan(Vector),
    /// drag to turn (x) and tilt (y) the view
    Orbit(Vector),
    /// the drag that turned the view let go
    OrbitEnd,
    /// turn by this many radians, eased
    Turn(f32),
    ResetView,
    ZoomAt(Point, f32),
    Zoom(f32),
    Fit,
    Select(Option<u32>),
    Step(i32),
    Follow,
    CloseCards,
    ToggleFeed,
    ToggleCrew,
    ToggleSettings,
    /// a fresh office for the active session
    NewLayout,
    /// open one settings dropdown, or close them
    Menu(Option<&'static str>),
    /// show one settings page
    SettingsPage(Page),
    /// show or hide the hand-picked colours
    ToggleColors,
    Key(keyboard::Event),
    Window(WindowAction),
    Setting(Setting),
    /// debug: `SHIPOFFICE_SCREENSHOT=path.png` saves the window after a few seconds and quits
    Shot(window::Screenshot),
    /// make the Locked in card: capture the office, then show the card
    LockedIn,
    LockedInShot(window::Screenshot),
    SaveCard,
    CopyCaption,
    ShowCardFolder,
    CloseCard,
}

/// The Locked in card, from the moment the office is captured to the preview.
pub enum Sharing {
    /// the HUD is hidden; the office is captured after this many more ticks, so a frame without
    /// it has been drawn
    Capturing(u8),
    Shooting,
    Ready(Card),
}

pub struct Card {
    pub image: image::RgbaImage,
    pub handle: iced::widget::image::Handle,
    pub caption: String,
    pub saved: Option<PathBuf>,
    pub copied: bool,
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
    Theme(String),
    Accent(u32),
    /// a colour role and the hex being typed for it
    Color(String, String),
    ResetColors,
    OfficeStyle(Option<String>),
    SnapView(bool),
    Backdrop(crate::config::Backdrop),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Look,
    Sources,
    Office,
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
    /// floor and the four walls, painted off the UI thread
    statics: Option<[Layer; 5]>,
    building: bool,
    board: Option<Layer>,
    board_stats: Option<BoardStats>,
    /// the room's `generation` its painted layers are for
    painted: u32,
}

pub struct App {
    cfg: Arc<RwLock<Config>>,
    rx: Receiver<Update>,
    pub sessions: BTreeMap<SessionKey, Session>,
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub cam: Camera,
    pub stage: Size,
    /// minimized windows tick slowly
    hidden: bool,
    scale_factor: f32,
    pub scene: Scene,
    pub hover: Option<Hit>,
    pub selected: Option<u32>,
    pub follow: bool,
    pub board_open: bool,
    pub settings_open: bool,
    pub crew_open: bool,
    pub sharing: Option<Sharing>,
    pub color_drafts: HashMap<String, String>,
    pub harnesses: Vec<HarnessInfo>,
    pub path_input: String,
    pub path_harness: Option<HarnessChoice>,
    demo: Option<Demo>,
    last_tick: Option<Instant>,
    last_second: Instant,
    zoomed_at: Instant,
    pub fps: f32,
    started: Instant,
    jobs_tx: Sender<(u64, paint::Statics)>,
    jobs_rx: Receiver<(u64, paint::Statics)>,
    /// the orbit: where the view is and where it is easing to
    pub yaw: f32,
    pub pitch: f32,
    yaw_to: f32,
    pitch_to: f32,
    /// eased zoom: target zoom and the stage point it zooms around
    zoom_to: Option<(f32, Point)>,
    /// eased return to the fitted view
    fitting: bool,
    /// when each card or panel opened, for its entrance
    pub opened: HashMap<&'static str, Instant>,
    pub tab_since: Instant,
    /// the dropdown that is open, if any
    pub menu: Option<&'static str>,
    /// each harness's compaction and window settings per project and model, read in the background
    compaction: Arc<std::sync::Mutex<HashMap<SettingsKey, (Instant, crate::harness::compaction::Settings)>>>,
    pub page: Page,
    pub colors_open: bool,
    /// debug: select the first agent once an office has one
    inspect_pending: bool,
    /// debug: make the Locked in card this many seconds in
    card_at: Option<f32>,
    /// when the active office was last regenerated (the dice spins)
    pub rolled: Option<Instant>,
    next_uid: u64,
    shot_taken: bool,
}

pub fn run() -> iced::Result {
    // the local offset can only be read safely while this is the only thread
    let _ = LOCAL_OFFSET.set(time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC));
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
    // the dock pairs the window with shipoffice.desktop by this id
    #[cfg(target_os = "linux")]
    {
        win.platform_specific.application_id = String::from("shipoffice");
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
        let loaded = Config::load();
        style::apply(&loaded.appearance);
        let cfg = Arc::new(RwLock::new(loaded));
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
            hidden: false,
            stage: Size::new(1320.0, 840.0),
            scale_factor: 1.0,
            scene: Scene::default(),
            hover: None,
            selected: None,
            follow: false,
            board_open: false,
            settings_open: false,
            crew_open: false,
            sharing: None,
            color_drafts: HashMap::new(),
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
            yaw: view::DEFAULT_YAW,
            pitch: view::DEFAULT_PITCH,
            yaw_to: view::DEFAULT_YAW,
            pitch_to: view::DEFAULT_PITCH,
            zoom_to: None,
            fitting: false,
            opened: HashMap::new(),
            tab_since: Instant::now(),
            menu: None,
            compaction: Default::default(),
            page: Page::default(),
            colors_open: false,
            inspect_pending: false,
            card_at: None,
            rolled: None,
            next_uid: 1,
            shot_taken: false,
        };
        let mut app = app;
        // debug: SHIPOFFICE_OPEN="settings,page=office,menu=fps,colors" starts with those open,
        // ("card=6" makes the Locked in card after 6 s)
        // for screenshots of the HUD
        if let Ok(v) = std::env::var("SHIPOFFICE_OPEN") {
            for part in v.split(',').map(str::trim) {
                match part.split_once('=') {
                    None if part == "settings" => app.settings_open = true,
                    None if part == "colors" => app.colors_open = true,
                    None if part == "inspect" => app.inspect_pending = true,
                    None if part == "card" => app.card_at = Some(5.0),
                    Some(("card", secs)) => app.card_at = secs.parse().ok(),
                    Some(("page", "look")) => app.page = Page::Look,
                    Some(("page", "sources")) => app.page = Page::Sources,
                    Some(("page", "office")) => app.page = Page::Office,
                    Some(("menu", m)) => app.menu = ["theme", "office", "backdrop", "harness", "minutes", "tabs", "fps"].into_iter().find(|k| *k == m),
                    _ => {}
                }
            }
        }
        // debug: SHIPOFFICE_VIEW="yaw,pitch" in degrees starts at another angle
        if let Some((y, p)) = std::env::var("SHIPOFFICE_VIEW").ok().and_then(|v| v.split_once(',').and_then(|(a, b)| Some((a.trim().parse::<f32>().ok()?, b.trim().parse::<f32>().ok()?)))) {
            app.yaw = y.to_radians();
            app.yaw_to = app.yaw;
            app.pitch = p.to_radians().clamp(view::MIN_PITCH, view::MAX_PITCH);
            app.pitch_to = app.pitch;
        }
        (app, window::latest().and_then(window::scale_factor).map(Message::ScaleFactor))
    }

    pub fn config(&self) -> Config {
        self.cfg.read().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    /// The camera is turning, tilting, zooming, fitting or following someone, or someone in the
    /// office is walking or changing pose: all of these glide at 60 Hz.
    fn moving(&self) -> bool {
        let people = self.tab().is_some_and(|t| t.world.avatars.iter().any(|a| a.anim == crate::office::world::Anim::Walk || a.fade.is_some()));
        (self.yaw - self.yaw_to).abs() > 1e-3 || (self.pitch - self.pitch_to).abs() > 1e-3 || self.zoom_to.is_some() || self.fitting || self.follow || people
    }

    pub fn session(&self) -> Option<&Session> {
        self.tab().and_then(|t| self.sessions.get(&t.key))
    }

    fn subscription(&self) -> Subscription<Message> {
        // 60 Hz while the view is moving, so turns, tilts and zooms glide; 30 Hz is plenty for a
        // still office, and a hidden window barely ticks
        let every = if self.hidden {
            Duration::from_millis(500)
        } else if self.moving() {
            Duration::from_micros(1_000_000 / 60)
        } else {
            Duration::from_millis(1000 / 30)
        };
        Subscription::batch([
            iced::time::every(every).map(Message::Tick),
            window::events().filter_map(|(_, e)| match e {
                window::Event::Resized(s) => Some(Message::Visible(s.width > 0.0 && s.height > 0.0)),
                window::Event::Focused => Some(Message::Visible(true)),
                _ => None,
            }),
            keyboard::listen().map(Message::Key),
        ])
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::Tick(now) => {
                self.tick(now);
                return Task::batch([self.debug_shot(), self.capture()]);
            }
            Message::Visible(v) => self.hidden = !v,
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
                    self.tab_since = Instant::now();
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
                let z = self.zoom_to.map_or(self.cam.z, |(z, _)| z);
                self.zoom_to = Some(((z * f).clamp(1.0, self.cam.max_z), p));
                self.fitting = false;
            }
            Message::Zoom(f) => return self.update(Message::ZoomAt(Point::new(self.stage.width / 2.0, self.stage.height / 2.0), f)),
            Message::Fit => {
                self.follow = false;
                self.zoom_to = None;
                self.fitting = true;
            }
            Message::Orbit(d) => {
                self.yaw_to -= d.x * 0.008;
                self.pitch_to = (self.pitch_to + d.y * 0.006).clamp(view::MIN_PITCH, view::MAX_PITCH);
                self.yaw = self.yaw_to;
                self.pitch = self.pitch_to;
            }
            Message::OrbitEnd => {
                if self.config().snap_view {
                    // the nearest corner: the four angles where the drawings sit on their tiles
                    let q = std::f32::consts::FRAC_PI_2;
                    self.yaw_to = view::DEFAULT_YAW + ((self.yaw - view::DEFAULT_YAW) / q).round() * q;
                }
            }
            Message::Turn(a) => self.yaw_to += a,
            Message::Menu(m) => {
                self.mark("menu", m.is_some());
                self.menu = m;
            }
            Message::SettingsPage(p) => {
                self.page = p;
                self.menu = None;
            }
            Message::ToggleColors => self.colors_open = !self.colors_open,
            Message::ResetView => {
                // the shortest way back to the classic angle
                let tau = std::f32::consts::TAU;
                let d = (self.yaw - view::DEFAULT_YAW).rem_euclid(tau);
                self.yaw = view::DEFAULT_YAW + if d > std::f32::consts::PI { d - tau } else { d };
                self.yaw_to = view::DEFAULT_YAW;
                self.pitch_to = view::DEFAULT_PITCH;
                self.fitting = true;
            }
            Message::Select(id) => {
                self.mark("inspector", id.is_some() && id != self.selected);
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
            Message::ToggleFeed => {
                let open = if let Ok(mut c) = self.cfg.write() {
                    c.feed_open = !c.feed_open;
                    let _ = c.save();
                    c.feed_open
                } else {
                    false
                };
                self.mark("feed", open);
            }
            Message::LockedIn => {
                if self.tab().is_some() && self.sharing.is_none() {
                    self.menu = None;
                    self.sharing = Some(Sharing::Capturing(2));
                }
            }
            Message::LockedInShot(shot) => {
                self.sharing = self.card(&shot).map(Sharing::Ready);
                self.mark("card", true);
            }
            Message::SaveCard => {
                if let Some(Sharing::Ready(c)) = self.sharing.as_mut() {
                    let dir = directories::UserDirs::new().and_then(|u| u.picture_dir().map(|d| d.to_path_buf())).unwrap_or_else(|| crate::harness::home().join("Pictures")).join("ShipOffice");
                    let name = local_now().format(time::macros::format_description!("locked-in-[year]-[month]-[day]-[hour][minute][second].png")).unwrap_or_else(|_| "locked-in.png".into());
                    let path = dir.join(name);
                    if std::fs::create_dir_all(&dir).is_ok() && c.image.save(&path).is_ok() {
                        c.saved = Some(path);
                    }
                }
            }
            Message::CopyCaption => {
                if let Some(Sharing::Ready(c)) = self.sharing.as_mut() {
                    c.copied = true;
                    return iced::clipboard::write(c.caption.clone());
                }
            }
            Message::ShowCardFolder => {
                if let Some(Sharing::Ready(Card { saved: Some(path), .. })) = &self.sharing {
                    if let Some(dir) = path.parent() {
                        open_folder(dir);
                    }
                }
            }
            Message::CloseCard => self.sharing = None,
            Message::ToggleCrew => {
                self.crew_open = !self.crew_open;
                self.mark("crew", self.crew_open);
            }
            Message::ToggleSettings => {
                self.settings_open = !self.settings_open;
                self.menu = None;
                self.mark("settings", self.settings_open);
            }
            Message::NewLayout => self.new_layout(),
            Message::Key(k) => return self.key(k),
            Message::Window(a) => {
                return match a {
                    WindowAction::Drag => window::latest().and_then(window::drag),
                    WindowAction::Minimize => {
                        self.hidden = true;
                        window::latest().and_then(|id| window::minimize(id, true))
                    }
                    WindowAction::ToggleMaximize => window::latest().and_then(window::toggle_maximize),
                    WindowAction::Close => window::latest().and_then(window::close),
                    WindowAction::Resize(dir) => window::latest().and_then(move |id| window::drag_resize(id, dir)),
                }
            }
            Message::Setting(s) => {
                // a pick from a dropdown closes it; typing does not
                if !matches!(s, Setting::PathInput(_) | Setting::Color(..)) {
                    self.menu = None;
                }
                self.setting(s)
            }
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

    /// Captures the office once the HUD has been off screen for a frame or two.
    fn capture(&mut self) -> Task<Message> {
        match self.sharing {
            Some(Sharing::Capturing(0)) => {
                self.sharing = Some(Sharing::Shooting);
                window::latest().and_then(window::screenshot).map(Message::LockedInShot)
            }
            Some(Sharing::Capturing(ref mut n)) => {
                *n -= 1;
                Task::none()
            }
            _ => Task::none(),
        }
    }

    /// The Locked in card for what every open session adds up to, the active office as its picture.
    fn card(&self, shot: &window::Screenshot) -> Option<Card> {
        let office = image::RgbaImage::from_raw(shot.size.width, shot.size.height, shot.rgba.to_vec())?;
        // close in on the room, wherever the camera has it
        let office = match self.tab() {
            Some(t) => {
                let b = self.view_of(t).bounds(&t.world.s.layout);
                let k = shot.size.width as f32 / self.stage.width.max(1.0);
                let ((x0, y0), (x1, y1)) = (self.cam.to_screen(b.min_x, b.min_y), self.cam.to_screen(b.max_x, b.max_y));
                share::around(&office, [x0 * k, y0 * k, x1 * k, y1 * k])
            }
            None => office,
        };
        let stats = share::Stats::gather(self.tabs.iter().filter_map(|t| self.sessions.get(&t.key)));
        let crew: Vec<share::Member> = self.tabs.iter().flat_map(|t| &t.world.avatars).map(|a| share::Member { face: portrait::face_image(a.char_id, &a.palette), ring: a.color() }).collect();
        let pal = style::p();
        let rgb = |c: iced::Color| [c.r, c.g, c.b].map(|v| (v * 255.0).round() as u8);
        let look = share::Look { bg: rgb(pal.bg), bg2: rgb(pal.bg2), surface: rgb(pal.surface), raised: rgb(pal.raised), text: rgb(pal.text), muted: rgb(pal.muted), faint: rgb(pal.faint), accent: rgb(pal.accent), light: pal.light };
        let date = local_now().format(time::macros::format_description!("[weekday repr:short] [day padding:none] [month repr:short] [year]")).unwrap_or_default();
        let image = share::card(&office, &stats, &crew, &look, &date);
        let handle = iced::widget::image::Handle::from_rgba(image.width(), image.height(), image.as_raw().clone());
        Some(Card { image, handle, caption: share::caption(&stats), saved: None, copied: false })
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
                if matches!(self.sharing, Some(Sharing::Ready(_))) {
                    self.sharing = None;
                } else if self.menu.is_some() {
                    self.menu = None;
                } else if self.settings_open {
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
            keyboard::Key::Character("q") => return self.update(Message::Turn(std::f32::consts::FRAC_PI_4)),
            keyboard::Key::Character("e") => return self.update(Message::Turn(-std::f32::consts::FRAC_PI_4)),
            keyboard::Key::Character("r") => return self.update(Message::ResetView),
            keyboard::Key::Character("n") => return self.update(Message::NewLayout),
            keyboard::Key::Character("l") => return self.update(Message::LockedIn),
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
        self.mark("inspector", true);
        self.board_open = false;
    }

    fn click(&mut self, hit: Option<Hit>) {
        match hit {
            Some(Hit::Agent(id)) => {
                self.selected = if self.selected == Some(id) { None } else { Some(id) };
                self.mark("inspector", self.selected.is_some());
                self.board_open = false;
            }
            Some(Hit::Board) => {
                self.board_open = !self.board_open;
                self.mark("board", self.board_open);
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
                if user.is_some() && user != self.selected {
                    self.mark("inspector", true);
                }
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
        self.cam.max_z = crate::render::max_zoom(&t.world.s.layout);
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
        cfg.layouts.get(&s.key.to_string()).or_else(|| cfg.layouts.get(&project)).copied().unwrap_or_else(|| hash(&s.cwd.as_ref().map_or_else(|| project.clone(), |p| p.to_string_lossy().into_owned())))
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
        // long-idle sessions go, as the watcher lets go of their files at the same point
        let tabs = &self.tabs;
        self.sessions.retain(|k, s| fresh(s) || tabs.iter().any(|t| t.key == *k) || now.duration_since(s.last).unwrap_or_default() < sources::IDLE_CLOSE);
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
                let world = self.world_for(&s.project(), seed, s.live_agents().count());
                self.tabs.push(Tab { key: s.key.clone(), world, uid, seed, statics: None, building: false, board: None, board_stats: None, painted: 0 });
            }
        }
        let before = self.active;
        self.active = active_key.and_then(|k| self.tabs.iter().position(|t| t.key == k)).unwrap_or(0);
        if before != self.active || self.cam.bounds == Bounds::default() {
            self.selected = None;
            self.relayout(true);
        }
    }

    /// The office for a project, sized for the `crew` in it now (it grows later if need be).
    fn world_for(&self, project: &str, seed: u32, crew: usize) -> World {
        Self::styled(World::for_crew(project, seed, crew), &self.config())
    }

    /// Applies a fixed office style from the settings, if one is picked.
    fn styled(mut w: World, cfg: &Config) -> World {
        if let Some(th) = cfg.appearance.office_style.as_ref().and_then(|n| crate::office::gen::THEMES.iter().find(|t| t.name == n.as_str())) {
            w.s.layout.theme = *th;
        }
        w
    }

    fn new_layout(&mut self) {
        let Some(key) = self.tab().map(|t| t.key.clone()) else { return };
        let Some(s) = self.sessions.get(&key) else { return };
        let (project, crew) = (s.project(), s.live_agents().count());
        let seed = hash(&format!("{key}:{}", self.started.elapsed().as_nanos()));
        // pinned to this session only; other sessions of the project keep theirs
        if let Ok(mut c) = self.cfg.write() {
            c.layouts.insert(key.to_string(), seed);
            let _ = c.save();
        }
        self.rolled = Some(Instant::now());
        self.tab_since = Instant::now();
        let uid = self.next_uid;
        self.next_uid += 1;
        let world = self.world_for(&project, seed, crew);
        if let Some(t) = self.tabs.get_mut(self.active) {
            *t = Tab { key, world, uid, seed, statics: None, building: false, board: None, board_stats: None, painted: 0 };
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
        let mut grew = false;
        for (i, t) in self.tabs.iter_mut().enumerate() {
            if let Some(s) = self.sessions.get(&t.key) {
                t.world.sync(s);
            }
            t.world.step(dt);
            // a crew that outgrew its room got a bigger one: paint it afresh
            if t.world.generation != t.painted {
                t.painted = t.world.generation;
                (t.statics, t.building, t.board, t.board_stats) = (None, false, None, None);
                t.uid = self.next_uid;
                self.next_uid += 1;
                grew |= i == self.active;
            }
        }
        if grew {
            // the same people in a bigger room: whoever was picked stays picked
            self.tab_since = Instant::now();
            self.relayout(false);
        }
        if self.selected.is_some_and(|id| self.tab().is_none_or(|t| t.world.avatar(id).is_none())) {
            self.selected = None;
            self.follow = false;
        }
        if self.follow {
            if let Some(h) = self.selected.and_then(|id| self.scene.heads.iter().find(|h| h.id == id)).copied() {
                let e = (dt * 3.5).min(1.0);
                // as close as following someone in a small office, however big this one is
                let z = 1.9 * self.cam.max_z / crate::render::MAX_ZOOM;
                self.cam.z += (z - self.cam.z) * e;
                self.cam.cx += (h.x - self.cam.cx) * e;
                self.cam.cy += ((h.top + h.bottom) / 2.0 - self.cam.cy) * e;
                self.cam.clamp();
                self.zoomed_at = now;
            }
        }
        self.motion(dt);
        if self.card_at.is_some_and(|t| self.started_secs() >= t) {
            self.card_at = None;
            let _ = self.update(Message::LockedIn);
        }
        if self.inspect_pending {
            if let Some(id) = self.tab().and_then(|t| t.world.avatars.first()).map(|a| a.id) {
                self.inspect_pending = false;
                self.selected = Some(id);
                self.mark("inspector", true);
            }
        }
        self.layers();
        self.build_scene();
    }

    /// Keeps the active office's painted layers current: the floor and walls (painted off the
    /// UI thread once per office) and the whiteboard's numbers.
    fn layers(&mut self) {
        while let Ok((uid, st)) = self.jobs_rx.try_recv() {
            if let Some(t) = self.tabs.iter_mut().find(|t| t.uid == uid) {
                let [w0, w1, w2, w3] = st.walls;
                let mk = |k: u64, img: image::RgbaImage| Layer { id: uid * 8 + k, version: 1, image: Arc::new(img) };
                t.statics = Some([mk(0, st.floor), mk(1, w0), mk(2, w1), mk(3, w2), mk(4, w3)]);
                t.building = false;
            }
        }
        // the board hangs on the back wall: nothing to repaint while that wall is out of sight
        let back = self.tab().is_some_and(|t| self.view_of(t).behind(view::Wall::Back));
        let stats = if back { self.session().map(|s| board_stats(s, s.window(&crate::model::AgentRef::Lead, &self.settings_of(s, &crate::model::AgentRef::Lead)).0)) } else { None };
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        if t.statics.is_none() && !t.building {
            t.building = true;
            let (layout, tx, uid): (Layout, _, _) = (t.world.s.layout.clone(), self.jobs_tx.clone(), t.uid);
            std::thread::spawn(move || {
                let _ = tx.send((uid, paint::statics(&layout)));
            });
        }
        if let Some(st) = stats {
            if t.board_stats.as_ref() != Some(&st) {
                let o = &t.world.s.layout;
                let span = (o.bx1 - o.bx0) as f32 * TW / 2.0;
                let img = paint::board(span, &st);
                let version = t.board.as_ref().map_or(1, |l| l.version + 1);
                t.board = Some(Layer { id: t.uid * 8 + 5, version, image: Arc::new(img) });
                t.board_stats = Some(st);
            }
        }
    }

    /// The user's compaction and window settings for a session's harness, project and model.
    /// Read on a worker the first time and every 30 s after, so the HUD never waits on a file.
    pub fn settings_for(&self, harness: &str, cwd: Option<&std::path::Path>, model: Option<&str>) -> crate::harness::compaction::Settings {
        let key = (harness.to_owned(), cwd.map(PathBuf::from), model.map(str::to_owned));
        let Ok(mut map) = self.compaction.lock() else { return Default::default() };
        let stale = map.get(&key).is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(30));
        if stale {
            let known = map.get(&key).map(|(_, c)| c.clone()).unwrap_or_default();
            map.insert(key.clone(), (Instant::now(), known));
            let (shared, job) = (self.compaction.clone(), key.clone());
            std::thread::spawn(move || {
                let found = crate::harness::compaction::read(&job.0, job.1.as_deref(), job.2.as_deref());
                if let Ok(mut map) = shared.lock() {
                    map.insert(job, (Instant::now(), found));
                }
            });
        }
        map.get(&key).map(|(_, c)| c.clone()).unwrap_or_default()
    }

    /// The settings that apply to one agent of a session (helpers may run another model).
    pub fn settings_of(&self, s: &Session, who: &crate::model::AgentRef) -> crate::harness::compaction::Settings {
        let model = s.agents.get(who).and_then(|a| a.model.as_deref()).or(s.model.as_deref());
        self.settings_for(s.key.harness, s.cwd.as_deref(), model)
    }

    pub fn started_secs(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    pub fn view_of(&self, t: &Tab) -> View {
        View::new(&t.world.s.layout, self.yaw, self.pitch)
    }

    /// The view has been turned or tilted away from the classic angle.
    pub fn turned(&self) -> bool {
        let d = (self.yaw_to - view::DEFAULT_YAW).rem_euclid(std::f32::consts::TAU);
        d.min(std::f32::consts::TAU - d) > 0.01 || (self.pitch_to - view::DEFAULT_PITCH).abs() > 0.01
    }

    /// Eases the orbit, zoom and fit toward their targets, ease-out style.
    fn motion(&mut self, dt: f32) {
        let k = (dt * 9.0).min(1.0);
        self.yaw += (self.yaw_to - self.yaw) * k;
        self.pitch += (self.pitch_to - self.pitch) * k;
        if let Some((z, at)) = self.zoom_to {
            let f = (z / self.cam.z).powf((dt * 12.0).min(1.0));
            self.cam.zoom_at(at.x, at.y, f);
            if (z / self.cam.z - 1.0).abs() < 0.002 {
                self.zoom_to = None;
            }
            self.zoomed_at = Instant::now();
        }
        if let Some(t) = self.tabs.get(self.active) {
            let b = self.view_of(t).bounds(&t.world.s.layout);
            self.cam.set_bounds(b, (dt * 8.0).min(1.0));
        }
        if self.fitting {
            let k = (dt * 7.0).min(1.0);
            self.cam.z += (1.0 - self.cam.z) * k;
            self.cam.cx += (self.cam.home.0 - self.cam.cx) * k;
            self.cam.cy += (self.cam.home.1 - self.cam.cy) * k;
            self.cam.clamp();
            if (self.cam.z - 1.0).abs() < 0.002 {
                self.cam.z = 1.0;
                self.fitting = false;
            }
        }
    }

    /// Marks a card or panel as just opened (for its entrance) or closed.
    fn mark(&mut self, key: &'static str, open: bool) {
        if open {
            self.opened.insert(key, Instant::now());
        } else {
            self.opened.remove(key);
        }
    }

    fn build_scene(&mut self) {
        let cfg_fps = self.config().anim_fps.clamp(4, 30) as f32;
        use crate::config::Backdrop;
        let night = match self.config().appearance.backdrop {
            Backdrop::None => None,
            Backdrop::Day => Some(false),
            Backdrop::Night => Some(true),
            Backdrop::City => Some(!style::p().light),
        };
        let Some(t) = self.tabs.get(self.active) else {
            self.scene = Scene::default();
            return;
        };
        let view = self.view_of(t);
        let pan = (self.cam.cx - self.cam.home.0, self.cam.cy - self.cam.home.1);
        let (aspect, now) = (self.stage.width / self.stage.height.max(1.0), self.started.elapsed().as_secs_f32());
        let city = night.map(|night| {
            let c = crate::render::city::City { night };
            (c, crate::render::city::sky(c, &view, aspect, self.cam.z, pan, now))
        });
        self.scene = scene::build(&scene::Inputs {
            world: &t.world,
            view: &view,
            statics: t.statics.as_ref(),
            board: t.board.as_ref(),
            hover: self.hover,
            selected: self.selected,
            anim_fps: cfg_fps,
            now: self.started.elapsed().as_secs_f32(),
            city,
        });
        // a new office fades in
        let fade = style::entrance(self.tab_since, 0.35);
        if fade < 1.0 {
            for (_, i) in &mut self.scene.list.items {
                i.tint[3] = (i.tint[3] as f32 * fade) as u8;
            }
        }
    }

    fn setting(&mut self, s: Setting) {
        let mut save = true;
        let mut restyle = false;
        {
            let Ok(mut c) = self.cfg.write() else { return };
            match s {
                Setting::Harness(id, on) => c.harness.entry(id).or_default().enabled = on,
                Setting::Bubbles(v) => c.bubbles = v,
                Setting::Demo(v) => c.demo_when_empty = v,
                Setting::ShowFps(v) => c.show_fps = v,
                Setting::SnapView(v) => c.snap_view = v,
                Setting::Backdrop(b) => c.appearance.backdrop = b,
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
                Setting::Theme(id) => {
                    c.appearance.theme = id;
                    c.appearance.colors.clear();
                    self.color_drafts.clear();
                }
                Setting::Accent(a) => {
                    c.appearance.colors.insert("accent".into(), format!("#{a:06x}"));
                    self.color_drafts.remove("accent");
                }
                Setting::Color(role, v) => {
                    // keep what is being typed; apply once it is a full colour
                    match style::parse_hex(&v) {
                        Some(col) => {
                            c.appearance.colors.insert(role.clone(), style::to_hex(col));
                            if v.trim().trim_start_matches('#').len() == 6 {
                                self.color_drafts.remove(&role);
                            }
                        }
                        None => {
                            self.color_drafts.insert(role, v);
                            save = false;
                        }
                    }
                }
                Setting::ResetColors => {
                    c.appearance.colors.clear();
                    self.color_drafts.clear();
                }
                Setting::OfficeStyle(v) => {
                    c.appearance.office_style = v;
                    restyle = true;
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
            style::apply(&c.appearance);
        }
        if restyle {
            // offices repaint in the new style; their crews arrive again from the sessions
            self.tabs.clear();
            self.relayout(true);
        }
        self.last_second = Instant::now() - Duration::from_secs(2);
    }

    pub fn view_office(&self) -> iced::Element<'_, Message> {
        shader(OfficeShader { scene: &self.scene, cam: &self.cam }).width(iced::Fill).height(iced::Fill).into()
    }
}

/// The session numbers the whiteboard shows.
fn board_stats(s: &Session, window: u64) -> BoardStats {
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
        // only shown while waiting
        waiting: if s.status == Status::Working { 0 } else { secs(s.status_since) },
        tok_in: u.total_in(),
        tok_out: u.output,
        cache: u.cache_read as f32 / total_in as f32,
        files: s.files_touched,
        ctx: (s.context as f32 / window.max(1) as f32).min(1.0),
        tools,
    }
}

fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        Some(rest) => harness::home().join(rest),
        None => PathBuf::from(p),
    }
}

static LOCAL_OFFSET: std::sync::OnceLock<time::UtcOffset> = std::sync::OnceLock::new();

fn local_now() -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc().to_offset(LOCAL_OFFSET.get().copied().unwrap_or(time::UtcOffset::UTC))
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

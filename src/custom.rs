//! Your own content, from the content folder beside the config file: characters (new people, or
//! new animations for the built-in crew) and props (your own look for any prop in the office).
//! Read once when the app starts; docs/custom.md says how to make them.

use crate::render::assets::Anim;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

/// The built-in crew: character ids 0 to 6.
pub const BUILTIN: u8 = 7;
/// How tall the built-in crew stands in its sheets, in sheet px.
pub const BODY: f32 = 214.0;
/// Files from the content folder are named with this in front, so they are read from disk.
pub const PREFIX: &str = "custom:";
pub const NAMES: [&str; BUILTIN as usize] = ["hoodie", "bun", "beanie", "buzz", "silver", "red", "curls"];
/// The animations a character can bring, and what stands in for a missing one.
const FALLBACK: &[(&str, &[&str])] = &[
    ("walk", &["idle"]),
    ("walkback", &["walk_back", "walk"]),
    ("type", &["read", "sit", "idle"]),
    ("read", &["type", "sit", "idle"]),
    ("sit", &["type", "idle"]),
    ("ppfront", &["walk", "idle"]),
    ("ppback", &["walkback", "walk_back", "walk", "idle"]),
];
/// Built-in sheets a character of your own can bring too, for the full filmed look.
const FILMED: &[&str] = &[
    "stand8", "walk8_s", "walk8_se", "walk8_e", "walk8_ne", "walk8_n", "typing", "reading", "sipping", "guitar", "pool", "play_se", "play_ne", "swing_front", "swing_back",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    name: Option<String>,
    /// px from the soles to the top of the head in the sheets
    height: Option<f32>,
    /// the sitting sheets draw a desk and chair already
    #[serde(default)]
    desk: bool,
    /// a built-in character (hoodie, bun, ...) this one gives new animations to
    replaces: Option<String>,
    /// one still picture, for a character with no animations
    image: Option<String>,
    #[serde(default)]
    anims: BTreeMap<String, Spec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    file: String,
    #[serde(default = "one")]
    frames: u32,
    cols: Option<u32>,
    fps: Option<f32>,
    /// this sheet's own height of the character, when it differs from the manifest's
    height: Option<f32>,
    /// "finish": played when its work is done; "idle": now and then while it stands about
    when: Option<String>,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone)]
pub struct Character {
    /// its folder's name, which settings refer to it by
    pub folder: String,
    pub name: String,
    pub id: u8,
    /// sits at a desk drawn by the app (not by its own sheets)
    pub bare_desk: bool,
    /// sheet keys of its extra animations, by when they play
    pub extras: Vec<(Extra, &'static str)>,
    pub replaces: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extra {
    Finish,
    Idle,
}

#[derive(Debug, Clone)]
pub struct Prop {
    pub item: String,
    pub file: PathBuf,
}

#[derive(Default)]
pub struct Content {
    pub dir: Option<PathBuf>,
    pub characters: Vec<Character>,
    pub props: Vec<Prop>,
    /// what could not be read, one line each, for the settings page
    pub errors: Vec<String>,
    pub anims: Vec<(String, Anim)>,
}

static DIR: OnceLock<PathBuf> = OnceLock::new();
static CONTENT: OnceLock<Content> = OnceLock::new();

/// Where content is read from; set once before the first frame. Without it there is none.
pub fn set_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}

pub fn content() -> &'static Content {
    CONTENT.get_or_init(|| DIR.get().map(|d| load(d)).unwrap_or_default())
}

/// Reads a file from the content folder, for names made with [`PREFIX`].
pub fn read(name: &str) -> Option<Vec<u8>> {
    std::fs::read(name.strip_prefix(PREFIX)?).ok()
}

fn key(path: &Path) -> &'static str {
    Box::leak(format!("{PREFIX}{}", path.display()).into_boxed_str())
}

pub fn load(dir: &Path) -> Content {
    let mut c = Content { dir: Some(dir.to_owned()), ..Default::default() };
    let mut folders: Vec<PathBuf> = std::fs::read_dir(dir.join("characters")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    folders.sort();
    for f in folders {
        let folder = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let id = BUILTIN as usize + c.characters.len();
        if id > u8::MAX as usize {
            c.errors.push(format!("{folder}: too many characters"));
            break;
        }
        match character(&f, &folder, id as u8) {
            Ok((ch, anims)) => {
                c.anims.extend(anims);
                c.characters.push(ch);
            }
            Err(e) => c.errors.push(format!("{folder}: {e}")),
        }
    }
    let mut props: Vec<PathBuf> = std::fs::read_dir(dir.join("props")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("png") || x.eq_ignore_ascii_case("webp"))).collect();
    props.sort();
    for p in props {
        let item = p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if image::image_dimensions(&p).is_err() {
            c.errors.push(format!("props/{}: not a picture that can be read", p.file_name().unwrap_or_default().to_string_lossy()));
            continue;
        }
        c.props.push(Prop { item, file: p });
    }
    c
}

/// One character's folder: its manifest, and every sheet checked and measured.
fn character(dir: &Path, folder: &str, id: u8) -> Result<(Character, Vec<(String, Anim)>), String> {
    let text = std::fs::read_to_string(dir.join("character.toml")).map_err(|_| "no character.toml".to_owned())?;
    let m: Manifest = toml::from_str(&text).map_err(|e| format!("character.toml: {}", e.message()))?;
    let replaces = match &m.replaces {
        Some(r) => Some(NAMES.iter().position(|n| n.eq_ignore_ascii_case(r)).ok_or_else(|| format!("replaces = \"{r}\": not one of {}", NAMES.join(", ")))? as u8),
        None => None,
    };
    let id = replaces.unwrap_or(id);
    let mut specs = m.anims;
    if let Some(img) = &m.image {
        specs.entry("idle".into()).or_insert(Spec { file: img.clone(), frames: 1, cols: None, fps: None, height: None, when: None });
    }
    if replaces.is_none() && !specs.contains_key("idle") {
        return Err("needs an idle animation, or an image".into());
    }
    let height = m.height.unwrap_or(BODY).max(8.0);
    let mut anims: BTreeMap<String, Anim> = BTreeMap::new();
    let mut extras = Vec::new();
    for (name, s) in &specs {
        let path = dir.join(&s.file);
        let (w, h) = image::image_dimensions(&path).map_err(|_| format!("{name}: cannot read {}", s.file))?;
        let frames = s.frames.max(1);
        let cols = s.cols.unwrap_or(frames).clamp(1, frames);
        let rows = frames.div_ceil(cols);
        if w % cols != 0 || h % rows != 0 {
            return Err(format!("{name}: {} is {w}×{h}, which does not split into {cols} columns and {rows} rows", s.file));
        }
        let scale = BODY * crate::render::scene::VIDEO_SCALE / s.height.unwrap_or(height).max(8.0);
        let anim = Anim { file: key(&path).to_owned(), fw: w / cols, fh: h / rows, frames, cols, fps: Some(s.fps.unwrap_or(12.0)), paddle: None, scale: Some(scale) };
        let known = FALLBACK.iter().any(|f| f.0 == name) || FILMED.contains(&name.as_str()) || ["idle", "walk_back"].contains(&name.as_str());
        match s.when.as_deref() {
            Some(w) => {
                let when = match w {
                    "finish" => Extra::Finish,
                    "idle" => Extra::Idle,
                    _ => return Err(format!("{name}: when = \"{w}\": use \"finish\" or \"idle\"")),
                };
                let k: &'static str = Box::leak(format!("extra_{name}_c{id}").into_boxed_str());
                extras.push((when, k));
                anims.insert(format!("extra_{name}"), anim);
            }
            None if known => {
                anims.insert(name.clone(), anim);
            }
            None => return Err(format!("{name}: not an animation the office plays; give it when = \"finish\" or \"idle\" to play it as an extra")),
        }
    }
    // a new character has every animation: what it lacks, another of its own stands in for
    if replaces.is_none() {
        for (name, stand_ins) in FALLBACK {
            if !anims.contains_key(*name) {
                if let Some(a) = stand_ins.iter().find_map(|s| anims.get(*s)).cloned() {
                    anims.insert((*name).to_owned(), a);
                }
            }
        }
    }
    anims.remove("walk_back").into_iter().for_each(|a| {
        anims.entry("walkback".into()).or_insert(a);
    });
    let sits = ["typing", "reading"].iter().any(|k| anims.contains_key(*k));
    let ch = Character { folder: folder.to_owned(), name: m.name.unwrap_or_else(|| folder.to_owned()), id, bare_desk: !m.desk && !sits, extras, replaces };
    let keyed = anims.into_iter().map(|(k, a)| (crate::render::assets::sheet_key(&k, id), a)).collect();
    Ok((ch, keyed))
}

/// Who the office hires from, as character ids, and which one the lead is.
#[derive(Debug, Clone, Default)]
pub struct Pool {
    pub ids: Vec<u8>,
    pub lead: Option<u8>,
}

static POOL: RwLock<Option<Pool>> = RwLock::new(None);
static PROPS_ON: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Your own art is in use for this prop.
pub fn prop_on(item: &str) -> bool {
    PROPS_ON.read().is_ok_and(|p| p.iter().any(|i| i == item))
}

/// Sets who is hired, from the settings: the built-in crew (unless switched off) and your own
/// characters that are on.
pub fn set_pool(cfg: &crate::config::Custom) {
    let c = content();
    let mut ids: Vec<u8> = if cfg.builtin_characters { (0..BUILTIN).collect() } else { Vec::new() };
    ids.extend(c.characters.iter().filter(|ch| ch.replaces.is_none() && !cfg.off.contains(&ch.folder)).map(|ch| ch.id));
    if ids.is_empty() {
        ids = (0..BUILTIN).collect();
    }
    let lead = cfg.lead.as_ref().and_then(|l| c.characters.iter().find(|ch| &ch.folder == l).map(|ch| ch.id).or_else(|| NAMES.iter().position(|n| n == l).map(|i| i as u8)));
    if let Ok(mut p) = POOL.write() {
        *p = Some(Pool { ids, lead });
    }
    if let Ok(mut p) = PROPS_ON.write() {
        *p = props_on(cfg).into_iter().map(|p| p.item.clone()).collect();
    }
}

pub fn pool() -> Pool {
    POOL.read().ok().and_then(|p| p.clone()).unwrap_or_else(|| Pool { ids: (0..BUILTIN).collect(), lead: None })
}

/// Your character with this id, when it is one.
pub fn character_of(id: u8) -> Option<&'static Character> {
    content().characters.iter().find(|c| c.id == id && c.replaces.is_none())
}

/// The extra animations a character plays at this moment, as sheet keys.
pub fn extras(id: u8, when: Extra) -> impl Iterator<Item = &'static str> {
    content().characters.iter().filter(move |c| c.id == id).flat_map(move |c| c.extras.iter().filter(move |e| e.0 == when).map(|e| e.1))
}

/// Props you switched on, by item name.
pub fn props_on(cfg: &crate::config::Custom) -> Vec<&'static Prop> {
    content().props.iter().filter(|p| !cfg.off.contains(&format!("props/{}", p.item))).collect()
}

/// A short README dropped into a fresh content folder.
pub const README: &str = "ShipOffice content folder

characters/<name>/character.toml   a character of your own, or new animations for a built-in one
props/<item>.png                   your own look for a prop: arcade, beanbag, bookshelf, coffee,
                                   cooler, fridge, lamp, pingpong, plant, server, sofa, whiteboard,
                                   airhockey, amp, boardgame, chess, claw, foosball, hoops, pinball,
                                   pool, tv, vr, table, chairL, chairR, lowtable (and arcade_back,
                                   coffee_back... for the side that faces away)

Changes are read when ShipOffice starts. The full guide:
https://github.com/secblitz/ShipOffice/blob/main/docs/custom.md
";

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(path: &Path, w: u32, h: u32) {
        image::RgbaImage::from_pixel(w, h, image::Rgba([200, 80, 80, 255])).save(path).unwrap();
    }

    #[test]
    fn a_character_folder_becomes_sheets() {
        let dir = std::env::temp_dir().join(format!("shipoffice-custom-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let robo = dir.join("characters/robo");
        std::fs::create_dir_all(&robo).unwrap();
        sheet(&robo.join("idle.png"), 400, 200);
        sheet(&robo.join("dance.png"), 300, 200);
        std::fs::write(
            robo.join("character.toml"),
            "name = \"Robo\"\nheight = 180\n[anims.idle]\nfile = \"idle.png\"\nframes = 4\n[anims.dance]\nfile = \"dance.png\"\nframes = 3\nwhen = \"finish\"\n",
        )
        .unwrap();
        let broken = dir.join("characters/zz-broken");
        std::fs::create_dir_all(&broken).unwrap();
        sheet(&broken.join("idle.png"), 301, 200);
        std::fs::write(broken.join("character.toml"), "[anims.idle]\nfile = \"idle.png\"\nframes = 4\n").unwrap();
        std::fs::create_dir_all(dir.join("props")).unwrap();
        sheet(&dir.join("props/plant.png"), 60, 120);

        let c = load(&dir);
        assert_eq!(c.characters.len(), 1, "{:?}", c.errors);
        assert_eq!(c.errors.len(), 1);
        assert!(c.errors[0].contains("does not split"), "{}", c.errors[0]);
        let r = &c.characters[0];
        assert_eq!((r.id, r.name.as_str(), r.bare_desk), (7, "Robo", true));
        assert_eq!(r.extras, vec![(Extra::Finish, "extra_dance_c7")]);
        let keys: Vec<&str> = c.anims.iter().map(|a| a.0.as_str()).collect();
        // every animation the office asks for is there, from the one it has
        for k in ["idle_c7", "walk_c7", "walkback_c7", "type_c7", "read_c7", "sit_c7", "ppfront_c7", "ppback_c7", "extra_dance_c7"] {
            assert!(keys.contains(&k), "{k} in {keys:?}");
        }
        let idle = &c.anims.iter().find(|a| a.0 == "idle_c7").unwrap().1;
        assert_eq!((idle.fw, idle.fh, idle.cols), (100, 200, 4));
        assert_eq!(c.props.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

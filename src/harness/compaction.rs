//! When each harness compacts an agent's context, and how big a window it gives the model, as the
//! user configured them. Only those settings are taken from each file; nothing else in them is
//! kept, and credential files are never opened.

use super::{env_path, home, read_capped};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    /// auto-compaction is switched off
    Off,
    /// compacts once the context passes this share of the window, 0..1
    Share(f32),
    /// compacts once the context passes this many tokens
    Tokens(u64),
}

/// What the user set, each with the file (or variable) it came from, for the inspector to cite.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    /// when it compacts
    pub rule: Option<(Rule, String)>,
    /// the context window it works with, in tokens, when that is not simply the model's
    pub window: Option<(u64, String)>,
}

/// Biggest settings file read; Claude's `~/.claude.json` grows with history, so it gets more room.
const SETTINGS: u64 = 1024 * 1024;
const CLAUDE_STATE: u64 = 32 * 1024 * 1024;

fn json(path: &Path, limit: u64) -> Option<Value> {
    read_capped(path, limit).and_then(|b| serde_json::from_slice(&b).ok())
}

fn tidy(p: &Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

/// The user's settings for `harness` running `model`, looking in the project (`cwd`) first.
/// Empty when they set nothing, or the harness has no such settings.
pub fn read(harness: &str, cwd: Option<&Path>, model: Option<&str>) -> Settings {
    match harness {
        "claude-code" => claude(cwd, model),
        "codex" => codex(),
        "gemini-cli" => gemini(".gemini", cwd),
        "qwen-code" => gemini(".qwen", cwd),
        "goose" => goose(),
        _ => Settings::default(),
    }
}

fn claude(cwd: Option<&Path>, model: Option<&str>) -> Settings {
    let root = env_path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home().join(".claude"));
    let state = env_path("CLAUDE_CONFIG_DIR").map_or_else(|| home().join(".claude.json"), |d| d.join(".claude.json"));
    let mut s = claude_in(&root, &state, cwd, model);
    if s.rule.is_none() {
        s.rule = std::env::var("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE").ok().and_then(|v| v.trim().parse::<f64>().ok()).map(|pct| (Rule::Share((pct / 100.0).clamp(0.0, 1.0) as f32), "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE".into()));
    }
    // the variable wins over the settings files, as in Claude Code
    if let Some(n) = std::env::var("CLAUDE_CODE_AUTO_COMPACT_WINDOW").ok().and_then(|v| v.trim().parse::<u64>().ok()).filter(|n| *n > 0) {
        s.window = Some((n, "CLAUDE_CODE_AUTO_COMPACT_WINDOW".into()));
    }
    s
}

/// A model id as settings may name it: no `[1m]`, no date.
fn bare(model: &str) -> String {
    let m = model.trim().to_ascii_lowercase().replace("[1m]", "");
    match m.rsplit_once('-') {
        Some((head, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => head.to_owned(),
        _ => m,
    }
}

fn claude_in(root: &Path, state: &Path, cwd: Option<&Path>, model: Option<&str>) -> Settings {
    let mut out = Settings::default();
    // switched off in /config, kept in the global state file
    if read_capped(state, CLAUDE_STATE).is_some_and(|b| memfind(&b, b"\"autoCompactEnabled\"")) {
        if json(state, CLAUDE_STATE).and_then(|v| v.get("autoCompactEnabled").and_then(Value::as_bool)) == Some(false) {
            out.rule = Some((Rule::Off, tidy(state)));
        }
    }
    // the threshold override, as an env entry in settings (project local, project, user), and the
    // window, for this model first and then for every model
    let mut files: Vec<PathBuf> = Vec::new();
    if let Some(c) = cwd {
        files.push(c.join(".claude/settings.local.json"));
        files.push(c.join(".claude/settings.json"));
    }
    files.push(root.join("settings.json"));
    let want = model.map(bare);
    for f in files {
        let Some(v) = json(&f, SETTINGS) else { continue };
        if out.rule.is_none() {
            if let Some(pct) = v.pointer("/env/CLAUDE_AUTOCOMPACT_PCT_OVERRIDE").and_then(number) {
                out.rule = Some((Rule::Share((pct / 100.0).clamp(0.0, 1.0) as f32), tidy(&f)));
            }
        }
        if out.window.is_none() {
            let per_model = want.as_ref().and_then(|m| {
                let all = v.get("modelSettings")?.as_object()?;
                all.iter().find(|(k, _)| bare(k) == *m).and_then(|(_, s)| s.get("autoCompactWindow")).and_then(number)
            });
            let n = per_model.or_else(|| v.get("autoCompactWindow").and_then(number)).or_else(|| v.pointer("/env/CLAUDE_CODE_AUTO_COMPACT_WINDOW").and_then(number));
            if let Some(n) = n.filter(|n| *n > 0.0) {
                out.window = Some((n as u64, tidy(&f)));
            }
        }
    }
    out
}

fn codex() -> Settings {
    let f = env_path("CODEX_HOME").unwrap_or_else(|| home().join(".codex")).join("config.toml");
    let Some(v) = read_capped(&f, SETTINGS).and_then(|b| String::from_utf8(b).ok()).and_then(|t| toml::from_str::<toml::Value>(&t).ok()) else { return Settings::default() };
    let rule = v.get("model_auto_compact_token_limit").and_then(toml::Value::as_integer).map(|n| (if n <= 0 { Rule::Off } else { Rule::Tokens(n as u64) }, tidy(&f)));
    let window = v.get("model_context_window").and_then(toml::Value::as_integer).filter(|n| *n > 0).map(|n| (n as u64, tidy(&f)));
    Settings { rule, window }
}

fn gemini(dir: &str, cwd: Option<&Path>) -> Settings {
    let mut files: Vec<PathBuf> = cwd.map(|c| c.join(dir).join("settings.json")).into_iter().collect();
    files.push(home().join(dir).join("settings.json"));
    for f in files {
        let Some(v) = json(&f, SETTINGS) else { continue };
        let t = ["/model/compressionThreshold", "/model/chatCompression/contextPercentageThreshold", "/chatCompression/contextPercentageThreshold"].iter().find_map(|p| v.pointer(p).and_then(number));
        if let Some(t) = t {
            return Settings { rule: Some((Rule::Share(t.clamp(0.0, 1.0) as f32), tidy(&f))), window: None };
        }
    }
    Settings::default()
}

/// Goose takes its settings from the environment first, then its `config.yaml`. Given both a
/// share and a token cap it compacts at whichever comes first; the cap is the one shown.
fn goose() -> Settings {
    let f = directories::BaseDirs::new().map(|b| b.config_dir().join("goose").join("config.yaml")).unwrap_or_default();
    let yaml = if f.as_os_str().is_empty() { Default::default() } else { super::copilot::yaml_fields(&f) };
    let get = |k: &str| -> Option<(f64, String)> {
        match std::env::var(k).ok().and_then(|v| v.trim().parse().ok()) {
            Some(n) => Some((n, k.to_owned())),
            None => yaml.get(k).and_then(|v| v.parse().ok()).map(|n| (n, tidy(&f))),
        }
    };
    let rule = match (get("GOOSE_AUTO_COMPACT_TOKEN_LIMIT"), get("GOOSE_AUTO_COMPACT_THRESHOLD")) {
        (Some((n, from)), _) if n > 0.0 => Some((Rule::Tokens(n as u64), from)),
        (_, Some((t, from))) if t <= 0.0 => Some((Rule::Off, from)),
        (_, Some((t, from))) => Some((Rule::Share(t.clamp(0.0, 1.0) as f32), from)),
        _ => None,
    };
    Settings { rule, window: None }
}

/// A number, or a number written as a string (env values are strings).
fn number(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn memfind(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_the_setting() {
        let dir = std::env::temp_dir().join(format!("shipoffice-compaction-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(dir.join(".claude/settings.json"), r#"{"env":{"CLAUDE_AUTOCOMPACT_PCT_OVERRIDE":"80","OTHER":"x"}}"#).unwrap();
        let c = claude_in(&dir.join("user"), &dir.join("state.json"), Some(&dir), None);
        let (rule, from) = c.rule.unwrap();
        assert_eq!(rule, Rule::Share(0.8));
        assert!(from.ends_with(".claude/settings.json"));
        assert_eq!(c.window, None);
        std::fs::write(dir.join("state.json"), r#"{"autoCompactEnabled":false}"#).unwrap();
        assert_eq!(claude_in(&dir.join("user"), &dir.join("state.json"), Some(&dir), None).rule.unwrap().0, Rule::Off);
        std::fs::create_dir_all(dir.join(".gemini")).unwrap();
        std::fs::write(dir.join(".gemini/settings.json"), r#"{"model":{"chatCompression":{"contextPercentageThreshold":0.6}}}"#).unwrap();
        // the project file is found before the user's own
        let f = dir.join(".gemini/settings.json");
        let v = json(&f, SETTINGS).unwrap();
        assert_eq!(v.pointer("/model/chatCompression/contextPercentageThreshold").and_then(number), Some(0.6));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_window_for_this_model_before_the_one_for_all() {
        let dir = std::env::temp_dir().join(format!("shipoffice-window-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), r#"{"autoCompactWindow":400000,"modelSettings":{"claude-opus-5-5":{"autoCompactWindow":500000}}}"#).unwrap();
        let w = |m: Option<&str>| claude_in(&dir, &dir.join("none.json"), None, m).window.map(|w| w.0);
        assert_eq!(w(Some("claude-opus-5-5")), Some(500_000));
        assert_eq!(w(Some("claude-opus-5-5[1m]")), Some(500_000));
        assert_eq!(w(Some("claude-sonnet-5-5")), Some(400_000));
        assert_eq!(w(None), Some(400_000));
        assert_eq!(bare("claude-haiku-4-5-20251001"), "claude-haiku-4-5");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

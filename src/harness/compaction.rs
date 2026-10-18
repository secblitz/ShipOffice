//! When each harness compacts an agent's context, as the user configured it. Only the one setting
//! is taken from each file; nothing else in them is kept, and credential files are never opened.

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

/// A compaction setting and the file (or variable) it came from, for the inspector to cite.
#[derive(Debug, Clone, PartialEq)]
pub struct Compaction {
    pub rule: Rule,
    pub from: String,
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

/// The user's compaction setting for `harness`, looking in the project (`cwd`) first. `None` when
/// they set nothing, or the harness has no such setting.
pub fn read(harness: &str, cwd: Option<&Path>) -> Option<Compaction> {
    match harness {
        "claude-code" => claude(cwd),
        "codex" => codex(),
        "gemini-cli" => gemini(".gemini", cwd),
        "qwen-code" => gemini(".qwen", cwd),
        _ => None,
    }
}

fn claude(cwd: Option<&Path>) -> Option<Compaction> {
    let root = env_path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home().join(".claude"));
    let state = env_path("CLAUDE_CONFIG_DIR").map_or_else(|| home().join(".claude.json"), |d| d.join(".claude.json"));
    claude_in(&root, &state, cwd).or_else(|| {
        let pct = std::env::var("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE").ok().and_then(|s| s.trim().parse::<f64>().ok())?;
        Some(Compaction { rule: Rule::Share((pct / 100.0).clamp(0.0, 1.0) as f32), from: "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE".into() })
    })
}

fn claude_in(root: &Path, state: &Path, cwd: Option<&Path>) -> Option<Compaction> {
    // switched off in /config, kept in the global state file
    if read_capped(state, CLAUDE_STATE).is_some_and(|b| memfind(&b, b"\"autoCompactEnabled\"")) {
        if json(state, CLAUDE_STATE).and_then(|v| v.get("autoCompactEnabled").and_then(Value::as_bool)) == Some(false) {
            return Some(Compaction { rule: Rule::Off, from: tidy(state) });
        }
    }
    // the threshold override, as an env entry in settings (project local, project, user) or the shell
    let mut files: Vec<PathBuf> = Vec::new();
    if let Some(c) = cwd {
        files.push(c.join(".claude/settings.local.json"));
        files.push(c.join(".claude/settings.json"));
    }
    files.push(root.join("settings.json"));
    for f in files {
        let Some(v) = json(&f, SETTINGS) else { continue };
        if let Some(pct) = v.pointer("/env/CLAUDE_AUTOCOMPACT_PCT_OVERRIDE").and_then(number) {
            return Some(Compaction { rule: Rule::Share((pct / 100.0).clamp(0.0, 1.0) as f32), from: tidy(&f) });
        }
    }
    None
}

fn codex() -> Option<Compaction> {
    let f = env_path("CODEX_HOME").unwrap_or_else(|| home().join(".codex")).join("config.toml");
    let text = String::from_utf8(read_capped(&f, SETTINGS)?).ok()?;
    let v: toml::Value = toml::from_str(&text).ok()?;
    let n = v.get("model_auto_compact_token_limit")?.as_integer()?;
    Some(Compaction { rule: if n <= 0 { Rule::Off } else { Rule::Tokens(n as u64) }, from: tidy(&f) })
}

fn gemini(dir: &str, cwd: Option<&Path>) -> Option<Compaction> {
    let mut files: Vec<PathBuf> = cwd.map(|c| c.join(dir).join("settings.json")).into_iter().collect();
    files.push(home().join(dir).join("settings.json"));
    for f in files {
        let Some(v) = json(&f, SETTINGS) else { continue };
        let t = v.pointer("/model/chatCompression/contextPercentageThreshold").or_else(|| v.pointer("/chatCompression/contextPercentageThreshold")).and_then(number);
        if let Some(t) = t {
            return Some(Compaction { rule: Rule::Share(t.clamp(0.0, 1.0) as f32), from: tidy(&f) });
        }
    }
    None
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
        let c = claude_in(&dir.join("user"), &dir.join("state.json"), Some(&dir)).unwrap();
        assert_eq!(c.rule, Rule::Share(0.8));
        assert!(c.from.ends_with(".claude/settings.json"));
        std::fs::write(dir.join("state.json"), r#"{"autoCompactEnabled":false}"#).unwrap();
        assert_eq!(claude_in(&dir.join("user"), &dir.join("state.json"), Some(&dir)).unwrap().rule, Rule::Off);
        std::fs::create_dir_all(dir.join(".gemini")).unwrap();
        std::fs::write(dir.join(".gemini/settings.json"), r#"{"model":{"chatCompression":{"contextPercentageThreshold":0.6}}}"#).unwrap();
        // the project file is found before the user's own
        let f = dir.join(".gemini/settings.json");
        let v = json(&f, SETTINGS).unwrap();
        assert_eq!(v.pointer("/model/chatCompression/contextPercentageThreshold").and_then(number), Some(0.6));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! Harness adapters: where each coding agent keeps its live sessions, and how to read them.
//!
//! Adapters are passive. They only read files the harness already writes, never talk to the
//! harness, and never write anywhere near its data.

pub mod claude;
pub mod cline;
pub mod codex;
pub mod copilot;
pub mod crush;
pub mod cursor;
pub mod droid;
pub mod gemini;
pub mod goose;
pub mod opencode;
pub mod qwen;

use crate::model::{AgentRef, Event, SessionKey};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One thing to watch: usually a transcript file, sometimes a database.
#[derive(Debug, Clone)]
pub struct SourceSpec {
    pub key: SessionKey,
    pub agent: AgentRef,
    pub path: PathBuf,
    pub modified: SystemTime,
}

pub trait Adapter: Send + Sync {
    /// Stable id used in the config file, e.g. `claude-code`.
    fn id(&self) -> &'static str;
    /// Display name, e.g. `Claude Code`.
    fn name(&self) -> &'static str;
    /// Where this harness keeps sessions by default on this OS (env overrides included).
    fn default_roots(&self) -> Vec<PathBuf>;
    /// Sources under `roots` that changed after `since`.
    fn discover(&self, roots: &[PathBuf], since: SystemTime) -> Vec<SourceSpec>;
    /// A reader that yields new events from one source each time it is polled.
    fn open(&self, spec: &SourceSpec) -> Box<dyn SourceReader>;
}

pub trait SourceReader: Send {
    fn poll(&mut self) -> Vec<Event>;
}

/// Every adapter this build knows about, in the order they show up in settings.
pub fn all() -> Vec<Box<dyn Adapter>> {
    vec![
        Box::new(claude::ClaudeCode),
        Box::new(codex::Codex::default()),
        Box::new(opencode::OpenCode::opencode()),
        Box::new(opencode::OpenCode::kilo()),
        Box::new(gemini::GeminiCli::default()),
        Box::new(qwen::QwenCode),
        Box::new(copilot::CopilotCli),
        Box::new(cline::Cline::cline()),
        Box::new(cline::Cline::roo()),
        Box::new(goose::Goose::default()),
        Box::new(crush::Crush::default()),
        Box::new(droid::Droid::default()),
        Box::new(cursor::Cursor::default()),
    ]
}

/// The user's home directory, or an empty path when there is none.
pub fn home() -> PathBuf {
    directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()).unwrap_or_default()
}

/// A path from an environment variable, when it is set and not empty.
pub fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// `$XDG_DATA_HOME`, else `~/.local/share`. Several harnesses use it on every OS, macOS and Windows included.
pub fn xdg_data() -> PathBuf {
    env_path("XDG_DATA_HOME").unwrap_or_else(|| home().join(".local").join("share"))
}

/// Unix milliseconds as a time.
pub fn from_ms(ms: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64)
}

/// A JSON timestamp in Unix seconds or milliseconds (harnesses mix them), or RFC 3339.
pub fn any_ts(v: &serde_json::Value) -> Option<SystemTime> {
    if let Some(n) = v.as_f64() {
        let ms = if n > 1e11 { n } else { n * 1000.0 };
        return Some(from_ms(ms as i64));
    }
    let s = v.as_str()?;
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok().map(SystemTime::from)
}

/// A live harness database opened read-only, shared by the readers of every session in it.
///
/// The connection is opened lazily and dropped on any error, so a database that is being
/// created, replaced or briefly locked is simply tried again on the next poll. Queries run in
/// autocommit mode, so no read snapshot outlives a call and the writer can still checkpoint.
pub struct Db {
    path: PathBuf,
    conn: Mutex<Option<Connection>>,
}

impl Db {
    pub fn new(path: PathBuf) -> Self {
        Self { path, conn: Mutex::new(None) }
    }

    /// Runs `f` on the connection; `None` when the database can't be read right now.
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Option<T> {
        let mut guard = self.conn.lock().ok()?;
        if guard.is_none() {
            if !self.path.is_file() {
                return None;
            }
            // never `immutable=1`: the harness is writing while we read
            let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
            let c = Connection::open_with_flags(&self.path, flags).ok()?;
            c.busy_timeout(Duration::from_millis(250)).ok()?;
            *guard = Some(c);
        }
        let r = f(guard.as_ref()?);
        if r.is_err() {
            *guard = None;
        }
        r.ok()
    }

    /// `PRAGMA data_version` changes whenever another connection commits, so readers only
    /// query again after something was written.
    pub fn version(&self) -> Option<i64> {
        self.with(|c| c.query_row("PRAGMA data_version", [], |r| r.get(0)))
    }

    pub fn has_table(&self, name: &str) -> bool {
        self.with(|c| c.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1", [name], |r| r.get::<_, i64>(0)))
            .is_some_and(|n| n > 0)
    }
}

/// One shared [`Db`] per database file, kept by the adapter across scans.
#[derive(Default)]
pub struct DbCache(Mutex<HashMap<PathBuf, Arc<Db>>>);

impl DbCache {
    pub fn get(&self, path: &Path) -> Arc<Db> {
        let mut m = self.0.lock().unwrap();
        m.entry(path.to_path_buf()).or_insert_with(|| Arc::new(Db::new(path.to_path_buf()))).clone()
    }
}

/// The newest change to a SQLite database: the main file or its write-ahead log.
pub fn db_modified(db: &Path) -> Option<SystemTime> {
    let mut wal = db.as_os_str().to_owned();
    wal.push("-wal");
    modified(db).max(modified(Path::new(&wal)))
}

/// A JSON file the harness rewrites whole. [`JsonFile::read`] returns the contents when the
/// file's size or mtime changed and it parses; a half-written or briefly missing file gives
/// `None` and is tried again next time.
pub struct JsonFile {
    path: PathBuf,
    seen: Option<(u64, SystemTime)>,
}

/// Whole-file JSON bigger than this is not parsed.
pub const MAX_JSON: u64 = 64 * 1024 * 1024;

impl JsonFile {
    pub fn new(path: PathBuf) -> Self {
        Self { path, seen: None }
    }

    pub fn read(&mut self) -> Option<serde_json::Value> {
        let m = std::fs::metadata(&self.path).ok()?;
        let stamp = (m.len(), m.modified().ok()?);
        if self.seen == Some(stamp) || m.len() > MAX_JSON {
            return None;
        }
        let v = serde_json::from_slice(&std::fs::read(&self.path).ok()?).ok()?;
        self.seen = Some(stamp);
        Some(v)
    }
}

/// Follows an append-only JSONL file by byte offset.
///
/// Partial last lines are kept until the writer finishes them, and a file that shrinks
/// (rotated or rewritten) is read again from the start. Lines over [`MAX_LINE`] (pasted
/// images, huge tool outputs) are skipped without being buffered, and the first read of a
/// very long transcript starts [`BACKLOG`] bytes before the end.
pub struct JsonlTail {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
    skipping: bool,
}

pub const MAX_LINE: usize = 512 * 1024;
pub const BACKLOG: u64 = 64 * 1024 * 1024;
const CHUNK: usize = 1024 * 1024;
/// Most bytes one poll reads, so one huge file can't hold up every other session.
const PER_POLL: u64 = 96 * 1024 * 1024;

impl JsonlTail {
    pub fn new(path: PathBuf) -> Self {
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let (offset, skipping) = if len > BACKLOG { (len - BACKLOG, true) } else { (0, false) };
        Self { path, offset, partial: Vec::new(), skipping }
    }

    /// Complete new lines since the last call, parsed as JSON; lines that fail to parse are skipped.
    pub fn read(&mut self) -> Vec<serde_json::Value> {
        let Ok(mut f) = File::open(&self.path) else { return Vec::new() };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            self.offset = 0;
            self.partial.clear();
            self.skipping = false;
        }
        if len == self.offset || f.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut chunk = vec![0u8; CHUNK];
        let mut budget = PER_POLL;
        while budget > 0 {
            let n = match f.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            self.offset += n as u64;
            budget = budget.saturating_sub(n as u64);
            let mut rest = &chunk[..n];
            while let Some(i) = rest.iter().position(|&b| b == b'\n') {
                if !self.skipping {
                    self.partial.extend_from_slice(&rest[..i]);
                    if !self.partial.is_empty() {
                        if let Ok(v) = serde_json::from_slice(&self.partial) {
                            out.push(v);
                        }
                    }
                }
                self.partial.clear();
                self.skipping = false;
                rest = &rest[i + 1..];
            }
            if !self.skipping {
                self.partial.extend_from_slice(rest);
                if self.partial.len() > MAX_LINE {
                    self.partial.clear();
                    self.skipping = true;
                }
            }
        }
        out
    }
}

/// RFC 3339 timestamp from a JSON string field, or now when it is missing.
pub fn ts(v: &serde_json::Value) -> SystemTime {
    v.as_str()
        .and_then(|s| time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
        .map(SystemTime::from)
        .unwrap_or_else(SystemTime::now)
}

/// Shortens text for bubbles and the feed: first line, at most `max` characters.
pub fn clip(s: &str, max: usize) -> String {
    let line = s.trim().lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() <= max {
        line.to_string()
    } else {
        let mut out: String = line.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// A one-line summary of a tool call's arguments: the most telling field, whatever the harness names it.
pub fn tool_summary(input: &serde_json::Value) -> String {
    const KEYS: [&str; 12] = ["description", "command", "cmd", "file_path", "path", "pattern", "query", "url", "prompt", "filePath", "absolute_path", "regex"];
    if let Some(s) = input.as_str() {
        return clip(s, 90);
    }
    for k in KEYS {
        if let Some(v) = input.get(k) {
            if let Some(s) = v.as_str() {
                return clip(s, 90);
            }
            if let Some(a) = v.as_array() {
                let parts: Vec<&str> = a.iter().filter_map(|x| x.as_str()).collect();
                if !parts.is_empty() {
                    return clip(&parts.join(" "), 90);
                }
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn tail_waits_for_complete_lines_and_survives_truncation() {
        let dir = std::env::temp_dir().join(format!("shipoffice-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.jsonl");
        std::fs::write(&p, b"{\"a\":1}\n{\"a\":").unwrap();
        let mut t = JsonlTail::new(p.clone());
        assert_eq!(t.read().len(), 1);
        std::fs::OpenOptions::new().append(true).open(&p).unwrap().write_all(b"2}\n").unwrap();
        let got = t.read();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["a"], 2);
        std::fs::write(&p, b"{\"a\":3}\n").unwrap();
        assert_eq!(t.read()[0]["a"], 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clip_and_summary() {
        assert_eq!(clip("  hello\nworld", 10), "hello");
        assert_eq!(clip("abcdefghijkl", 5), "abcd…");
        assert_eq!(tool_summary(&serde_json::json!({"command": "cargo test"})), "cargo test");
        assert_eq!(tool_summary(&serde_json::json!({"command": ["bash", "-lc", "ls"]})), "bash -lc ls");
    }
}

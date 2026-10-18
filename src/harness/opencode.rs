//! OpenCode: one SQLite database, `~/.local/share/opencode/opencode.db` on every OS. 2.x keeps a
//! session's messages as rows of `session_message`; 1.x split them into `message` and `part`.
//! Kilo Code 7 is an OpenCode fork with the same schema in `kilo.db`. Child sessions (`parent_id`)
//! are subagents. See docs/harnesses.md §3 and §8b.
//!
//! Only `session_v2`, `session`, `session_message`, `message` and `part` are ever selected from:
//! the same file holds account tokens and credentials.

use super::{clip, db_modified, env_path, from_ms, tool_summary, xdg_data, Adapter, Db, DbCache, SourceReader, SourceSpec};
use crate::model::{AgentRef, Event, EventKind, SessionKey, Usage};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Sessions per database a scan looks at, newest first.
const SCAN_LIMIT: i64 = 100;
/// How far back a newly opened session is replayed, in messages (2.x) or parts (1.x).
const BACKLOG_ROWS: i64 = 200;
/// Rows one poll reads.
const POLL_LIMIT: i64 = 300;

pub struct OpenCode {
    id: &'static str,
    name: &'static str,
    /// database file stem: `opencode` (also `opencode-<channel>`) or `kilo`
    stem: &'static str,
    env_db: &'static str,
    dbs: DbCache,
    /// db path -> (data_version at the last scan, (session, parent, time_updated) it found)
    scans: Mutex<HashMap<PathBuf, (i64, Vec<(String, Option<String>, i64)>)>>,
    /// session -> its root session, so a subagent lands in the office of the session that spawned it
    roots: Mutex<HashMap<String, String>>,
}

impl OpenCode {
    pub fn opencode() -> Self {
        Self::new("opencode", "OpenCode", "opencode", "OPENCODE_DB")
    }

    pub fn kilo() -> Self {
        Self::new("kilo-code", "Kilo Code", "kilo", "KILO_DB")
    }

    fn new(id: &'static str, name: &'static str, stem: &'static str, env_db: &'static str) -> Self {
        Self { id, name, stem, env_db, dbs: DbCache::default(), scans: Mutex::default(), roots: Mutex::default() }
    }

    /// The database files under a root: the root itself when it is a file, else `<stem>.db` and
    /// channel builds `<stem>-<channel>.db` inside it.
    fn dbs_in(&self, root: &Path) -> Vec<PathBuf> {
        if root.is_file() {
            return vec![root.to_path_buf()];
        }
        let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
        entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                n.strip_suffix(".db").is_some_and(|s| s == self.stem || s.starts_with(&format!("{}-", self.stem)))
            })
            .collect()
    }

    fn root_of(&self, db: &Db, id: &str, parent: Option<&str>) -> String {
        let Some(mut cur) = parent.map(str::to_owned) else { return id.to_owned() };
        let mut roots = self.roots.lock().unwrap();
        if let Some(r) = roots.get(id) {
            return r.clone();
        }
        for _ in 0..8 {
            let up = db
                .with(|c| {
                    let v2 = parent_of(c, "session_v2", &cur)?;
                    if v2.is_some() {
                        return Ok(v2);
                    }
                    parent_of(c, "session", &cur)
                })
                .flatten()
                .flatten();
            match up {
                Some(p) => cur = p,
                None => break,
            }
        }
        roots.insert(id.to_owned(), cur.clone());
        cur
    }
}

/// `Some(parent)` for a known session (`None` inside when it has none); errors when the table is missing.
fn parent_of(c: &rusqlite::Connection, table: &str, id: &str) -> rusqlite::Result<Option<Option<String>>> {
    c.query_row(&format!("SELECT parent_id FROM {table} WHERE id=?1"), [id], |r| r.get(0)).optional()
}

impl Adapter for OpenCode {
    fn id(&self) -> &'static str {
        self.id
    }
    fn name(&self) -> &'static str {
        self.name
    }
    fn default_roots(&self) -> Vec<PathBuf> {
        let data = xdg_data().join(self.stem);
        let mut roots = Vec::new();
        // an absolute path, or one relative to the data dir
        if let Some(p) = env_path(self.env_db) {
            roots.push(if p.is_absolute() { p } else { data.join(p) });
        }
        roots.push(data);
        roots
    }

    fn discover(&self, roots: &[PathBuf], since: SystemTime) -> Vec<SourceSpec> {
        let since_ms = since.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for path in roots.iter().flat_map(|r| self.dbs_in(r)) {
            if !seen.insert(path.clone()) || db_modified(&path).is_none_or(|m| m < since) {
                continue;
            }
            let db = self.dbs.get(&path);
            let Some(version) = db.version() else { continue };
            let cached = self.scans.lock().unwrap().get(&path).filter(|(v, _)| *v == version).map(|(_, rows)| rows.clone());
            let rows = match cached {
                Some(rows) => rows,
                None => {
                    let mut rows = Vec::new();
                    for table in ["session_v2", "session"] {
                        if !db.has_table(table) {
                            continue;
                        }
                        // a few hundred rows without an index on time_updated: a cheap scan
                        let sql = format!("SELECT id, parent_id, time_updated FROM {table} WHERE time_updated >= ?1 ORDER BY time_updated DESC LIMIT ?2");
                        let got = db.with(|c| {
                            let mut st = c.prepare(&sql)?;
                            let it = st.query_map(params![since_ms, SCAN_LIMIT], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                            it.collect::<rusqlite::Result<Vec<(String, Option<String>, i64)>>>()
                        });
                        rows.extend(got.unwrap_or_default());
                    }
                    self.scans.lock().unwrap().insert(path.clone(), (version, rows.clone()));
                    rows
                }
            };
            let mut ids = HashSet::new();
            for (id, parent, updated) in rows {
                if updated < since_ms || !ids.insert(id.clone()) {
                    continue;
                }
                let root = self.root_of(&db, &id, parent.as_deref());
                let agent = if root == id { AgentRef::Lead } else { AgentRef::Sub(id.clone()) };
                // one source per session, so the watcher keys it by `<db>/<session id>`
                out.push(SourceSpec {
                    key: SessionKey { harness: self.id, id: root },
                    agent,
                    path: path.join(&id),
                    modified: from_ms(updated),
                });
            }
        }
        out
    }

    fn open(&self, spec: &SourceSpec) -> Box<dyn SourceReader> {
        let db = spec.path.parent().map(|p| self.dbs.get(p)).unwrap_or_else(|| self.dbs.get(&spec.path));
        let sid = spec.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Box::new(Reader::new(db, sid, spec.key.clone(), spec.agent.clone()))
    }
}

#[derive(PartialEq)]
enum Mode {
    Unknown,
    /// 2.x `session_message`
    Messages,
    /// 1.x `message` + `part`
    Parts,
}

/// What was already emitted for one message (2.x) or part (1.x).
#[derive(Default)]
struct Seen {
    /// content items by index (text, reasoning)
    items: HashSet<usize>,
    called: HashSet<String>,
    finished: HashSet<String>,
    usage: bool,
    done: bool,
}

struct Reader {
    db: Arc<Db>,
    sid: String,
    key: SessionKey,
    agent: AgentRef,
    version: Option<i64>,
    mode: Mode,
    started: bool,
    title: Option<String>,
    model: Option<String>,
    /// 2.x: lowest seq that may still change, and each row's last time_updated
    cursor: i64,
    rows: HashMap<i64, (i64, Seen)>,
    /// 1.x: newest part time_updated seen, parts by id, and message id -> (role, model)
    part_cursor: i64,
    parts: HashMap<String, Seen>,
    messages: HashMap<String, (String, Option<String>)>,
}

impl Reader {
    fn new(db: Arc<Db>, sid: String, key: SessionKey, agent: AgentRef) -> Self {
        Self {
            db,
            sid,
            key,
            agent,
            version: None,
            mode: Mode::Unknown,
            started: false,
            title: None,
            model: None,
            cursor: 0,
            rows: HashMap::new(),
            part_cursor: 0,
            parts: HashMap::new(),
            messages: HashMap::new(),
        }
    }

    fn ev(&self, at: SystemTime, kind: EventKind) -> Event {
        Event { session: self.key.clone(), agent: self.agent.clone(), at, kind }
    }

    fn sub_id(&self) -> Option<&str> {
        match &self.agent {
            AgentRef::Sub(id) => Some(id),
            AgentRef::Lead => None,
        }
    }

    /// The session row: cwd and title, and for a child session the subagent it is.
    fn session(&mut self, out: &mut Vec<Event>) {
        let sid = self.sid.clone();
        let row = self.db.with(|c| {
            for table in ["session_v2", "session"] {
                let sql = format!("SELECT directory, title, agent, model, time_updated FROM {table} WHERE id=?1");
                // a missing table (or column, in old 1.x) is not fatal here
                if let Ok(Some(r)) = c
                    .query_row(&sql, [&sid], |r| {
                        Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, i64>(4)?))
                    })
                    .optional()
                {
                    return Ok(Some(r));
                }
            }
            Ok(None)
        });
        let Some(Some((dir, title, agent, model, updated))) = row else { return };
        let at = from_ms(updated);
        if !self.started {
            if let Some(id) = self.sub_id().map(str::to_owned) {
                let description = title.as_deref().map(|t| clip(t, 60)).unwrap_or_else(|| "Subagent".into());
                out.push(self.ev(at, EventKind::SubagentStart { agent_id: id, description, kind: agent.clone() }));
            }
        }
        if self.sub_id().is_some() {
            return;
        }
        if !self.started || title != self.title {
            let model = model.and_then(|m| serde_json::from_str::<Value>(&m).ok()).and_then(|m| m.get("id").and_then(Value::as_str).map(str::to_owned));
            if model.is_some() {
                self.model.clone_from(&model);
            }
            let title_ev = title.as_deref().filter(|t| !t.is_empty()).map(|t| clip(t, 60));
            out.push(self.ev(at, EventKind::Meta { cwd: dir.map(PathBuf::from), branch: None, model, title: title_ev }));
            self.title = title;
        }
    }

    fn pick_mode(&mut self) {
        if self.mode != Mode::Unknown {
            return;
        }
        let has = |table: &str, sql: &str| self.db.has_table(table) && self.db.with(|c| c.query_row(sql, [&self.sid], |_| Ok(())).optional()).flatten().is_some();
        // Kilo writes both generations side by side; the newer one wins
        if has("session_message", "SELECT 1 FROM session_message WHERE session_id=?1 LIMIT 1") {
            self.mode = Mode::Messages;
            let start = self.db.with(|c| {
                c.query_row("SELECT seq FROM session_message WHERE session_id=?1 ORDER BY seq DESC LIMIT 1 OFFSET ?2", params![self.sid, BACKLOG_ROWS - 1], |r| r.get::<_, i64>(0))
                    .optional()
            });
            self.cursor = start.flatten().unwrap_or(0);
        } else if has("part", "SELECT 1 FROM part WHERE session_id=?1 LIMIT 1") {
            self.mode = Mode::Parts;
            let start = self.db.with(|c| {
                c.query_row("SELECT time_updated FROM part WHERE session_id=?1 ORDER BY time_updated DESC LIMIT 1 OFFSET ?2", params![self.sid, BACKLOG_ROWS - 1], |r| r.get::<_, i64>(0))
                    .optional()
            });
            self.part_cursor = start.flatten().unwrap_or(0);
        }
    }

    /// 2.x: re-reads only the rows whose `time_updated` moved, from the first one still streaming.
    fn poll_messages(&mut self, out: &mut Vec<Event>) {
        let (sid, cursor) = (self.sid.clone(), self.cursor);
        let Some(heads) = self.db.with(|c| {
            let mut st = c.prepare_cached("SELECT seq, type, time_updated FROM session_message WHERE session_id=?1 AND seq >= ?2 ORDER BY seq LIMIT ?3")?;
            let it = st.query_map(params![sid, cursor, POLL_LIMIT], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?;
            it.collect::<rusqlite::Result<Vec<_>>>()
        }) else {
            return;
        };
        let mut open_from: Option<i64> = None;
        let idle_at = heads.iter().rposition(|(_, t, _)| t == "idle");
        for (i, (seq, kind, updated)) in heads.iter().enumerate() {
            let fresh = self.rows.get(seq).is_none_or(|(t, _)| t != updated);
            let mut done = self.rows.get(seq).is_some_and(|(_, s)| s.done);
            if fresh {
                let data = self.db.with(|c| {
                    c.query_row("SELECT data FROM session_message WHERE session_id=?1 AND seq=?2", params![sid, seq], |r| r.get::<_, String>(0))
                });
                let Some(v) = data.and_then(|d| serde_json::from_str::<Value>(&d).ok()) else { continue };
                let mut seen = self.rows.remove(seq).map(|(_, s)| s).unwrap_or_default();
                self.message(kind, &v, &mut seen, out);
                done = seen.done;
                self.rows.insert(*seq, (*updated, seen));
            }
            // an assistant row is still streaming until it completes or the turn went idle after it
            if !done && idle_at.is_none_or(|j| j < i) && open_from.is_none() {
                open_from = Some(*seq);
            }
        }
        let next = match (open_from, heads.last()) {
            (Some(s), _) if (heads.len() as i64) < POLL_LIMIT => s,
            (_, Some((last, _, _))) => last + 1,
            _ => cursor,
        };
        self.cursor = next;
        self.rows.retain(|s, _| *s >= next);
    }

    fn message(&mut self, kind: &str, v: &Value, seen: &mut Seen, out: &mut Vec<Event>) {
        let at = v.pointer("/time/created").and_then(Value::as_i64).map(from_ms).unwrap_or_else(SystemTime::now);
        let lead = self.sub_id().is_none();
        match kind {
            "user" => {
                seen.done = true;
                let t = v.get("text").and_then(Value::as_str).unwrap_or("");
                if lead && !t.trim().is_empty() && !seen.items.contains(&0) {
                    seen.items.insert(0);
                    out.push(self.ev(at, EventKind::UserPrompt { text: clip(t, 140) }));
                }
            }
            "assistant" => {
                let completed = v.pointer("/time/completed").is_some_and(|t| !t.is_null());
                if let Some(m) = v.pointer("/model/id").and_then(Value::as_str) {
                    self.model(m, at, out);
                }
                let content = v["content"].as_array().map(Vec::as_slice).unwrap_or_default();
                for (i, item) in content.iter().enumerate() {
                    let last = i + 1 == content.len() && !completed;
                    let text = item.get("text").and_then(Value::as_str).unwrap_or("");
                    match item.get("type").and_then(Value::as_str).unwrap_or("") {
                        // text streams in place: wait until something follows it
                        "text" if !last && !seen.items.contains(&i) => {
                            seen.items.insert(i);
                            if !text.trim().is_empty() {
                                out.push(self.ev(at, EventKind::AssistantText { text: clip(text, 160) }));
                            }
                        }
                        "reasoning" if !seen.items.contains(&i) => {
                            seen.items.insert(i);
                            out.push(self.ev(at, EventKind::Thinking { text: clip(text, 140) }));
                        }
                        "tool" => {
                            let id = item.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
                            let name = item.get("name").and_then(Value::as_str).unwrap_or("tool");
                            let status = item.pointer("/state/status").and_then(Value::as_str).unwrap_or("");
                            self.tool(&id, name, status, &item["state"]["input"], at, seen, out);
                        }
                        _ => {}
                    }
                }
                if completed && !seen.usage {
                    seen.usage = true;
                    if let Some(t) = v.get("tokens") {
                        out.push(self.ev(at, EventKind::Usage(usage(t))));
                    }
                }
                seen.done = completed;
            }
            "synthetic" => {
                seen.done = true;
                // the parent's note that a subagent finished, in case its own session isn't watched
                let m = &v["metadata"];
                if m.get("source").and_then(Value::as_str) == Some("subagent") && m.get("state").and_then(Value::as_str) != Some("running") {
                    if let Some(child) = m.get("childID").and_then(Value::as_str) {
                        out.push(self.ev(at, EventKind::SubagentEnd { agent_id: child.to_owned() }));
                    }
                }
            }
            "idle" => {
                seen.done = true;
                out.push(self.end(at));
            }
            _ => seen.done = true,
        }
    }

    /// One tool item from either generation; call and result are the same object.
    #[allow(clippy::too_many_arguments)]
    fn tool(&self, id: &str, name: &str, status: &str, input: &Value, at: SystemTime, seen: &mut Seen, out: &mut Vec<Event>) {
        // a pending call may not have its arguments yet
        if matches!(status, "running" | "completed" | "error") && seen.called.insert(id.to_owned()) {
            if name == "question" && status == "running" {
                out.push(self.ev(at, EventKind::NeedsInput { text: "Waiting for your answer".into() }));
            }
            out.push(self.ev(at, EventKind::ToolCall { call_id: id.to_owned(), name: name.to_owned(), summary: tool_summary(input) }));
        }
        if matches!(status, "completed" | "error") && seen.finished.insert(id.to_owned()) {
            out.push(self.ev(at, EventKind::ToolResult { call_id: id.to_owned(), ok: status == "completed" }));
        }
    }

    /// The lead's model, whenever it changes.
    fn model(&mut self, m: &str, at: SystemTime, out: &mut Vec<Event>) {
        if self.sub_id().is_none() && self.model.as_deref() != Some(m) {
            self.model = Some(m.to_owned());
            out.push(self.ev(at, EventKind::Meta { cwd: None, branch: None, model: Some(m.to_owned()), title: None }));
        }
    }

    fn end(&self, at: SystemTime) -> Event {
        match self.sub_id() {
            Some(id) => self.ev(at, EventKind::SubagentEnd { agent_id: id.to_owned() }),
            None => self.ev(at, EventKind::TurnEnd),
        }
    }

    /// 1.x: parts updated since the newest one seen (`>=`: several share a millisecond).
    fn poll_parts(&mut self, out: &mut Vec<Event>) {
        let (sid, cursor) = (self.sid.clone(), self.part_cursor);
        let Some(rows) = self.db.with(|c| {
            let mut st = c.prepare_cached("SELECT id, message_id, time_updated, data FROM part WHERE session_id=?1 AND time_updated >= ?2 ORDER BY time_updated, id LIMIT ?3")?;
            let it = st.query_map(params![sid, cursor, POLL_LIMIT], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?)))?;
            it.collect::<rusqlite::Result<Vec<_>>>()
        }) else {
            return;
        };
        for (id, message, updated, data) in rows {
            self.part_cursor = self.part_cursor.max(updated);
            let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
            let (role, model) = self.role(&message);
            let mut seen = self.parts.remove(&id).unwrap_or_default();
            self.part(&v, &role, model, from_ms(updated), &mut seen, out);
            self.parts.insert(id, seen);
        }
    }

    fn role(&mut self, message: &str) -> (String, Option<String>) {
        if let Some(r) = self.messages.get(message) {
            return r.clone();
        }
        let data = self.db.with(|c| c.query_row("SELECT data FROM message WHERE id=?1", [message], |r| r.get::<_, String>(0)).optional());
        let v = data.flatten().and_then(|d| serde_json::from_str::<Value>(&d).ok()).unwrap_or(Value::Null);
        let r = (v.get("role").and_then(Value::as_str).unwrap_or("").to_owned(), v.get("modelID").and_then(Value::as_str).map(str::to_owned));
        if !r.0.is_empty() {
            self.messages.insert(message.to_owned(), r.clone());
        }
        r
    }

    fn part(&mut self, v: &Value, role: &str, model: Option<String>, updated: SystemTime, seen: &mut Seen, out: &mut Vec<Event>) {
        let at = v.pointer("/time/start").and_then(Value::as_i64).map(from_ms).unwrap_or(updated);
        let lead = self.sub_id().is_none();
        let text = v.get("text").and_then(Value::as_str).unwrap_or("");
        if let Some(m) = model.filter(|_| role == "assistant") {
            self.model(&m, at, out);
        }
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" if !seen.done => {
                let injected = v.get("synthetic").and_then(Value::as_bool).unwrap_or(false) || v.get("ignored").and_then(Value::as_bool).unwrap_or(false);
                if role == "user" {
                    seen.done = true;
                    if lead && !injected && !text.trim().is_empty() {
                        out.push(self.ev(at, EventKind::UserPrompt { text: clip(text, 140) }));
                    }
                } else if role == "assistant" && (v.pointer("/time/end").is_some() || v.get("time").is_none()) {
                    // a streaming text part has a start but no end yet
                    seen.done = true;
                    if !text.trim().is_empty() {
                        out.push(self.ev(at, EventKind::AssistantText { text: clip(text, 160) }));
                    }
                }
            }
            "reasoning" if !seen.done => {
                seen.done = true;
                out.push(self.ev(at, EventKind::Thinking { text: clip(text, 140) }));
            }
            "tool" => {
                let id = v.get("callID").and_then(Value::as_str).unwrap_or("").to_owned();
                let name = v.get("tool").and_then(Value::as_str).unwrap_or("tool");
                let status = v.pointer("/state/status").and_then(Value::as_str).unwrap_or("");
                self.tool(&id, name, status, &v["state"]["input"], at, seen, out);
            }
            "step-finish" if !seen.done => {
                seen.done = true;
                if let Some(t) = v.get("tokens") {
                    out.push(self.ev(at, EventKind::Usage(usage(t))));
                }
                if v.get("reason").and_then(Value::as_str).is_some_and(|r| r != "tool-calls") {
                    out.push(self.end(at));
                }
            }
            _ => {}
        }
    }
}

/// OpenCode keeps cached tokens out of `input` and reasoning out of `output`.
fn usage(t: &Value) -> Usage {
    let n = |p: &str| t.pointer(p).and_then(Value::as_u64).unwrap_or(0);
    Usage { input: n("/input"), output: n("/output") + n("/reasoning"), cache_read: n("/cache/read"), cache_write: n("/cache/write") }
}

impl SourceReader for Reader {
    fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let Some(version) = self.db.version() else { return out };
        if self.version == Some(version) {
            return out;
        }
        self.version = Some(version);
        self.session(&mut out);
        self.started = true;
        self.pick_mode();
        match self.mode {
            Mode::Messages => self.poll_messages(&mut out),
            Mode::Parts => self.poll_parts(&mut out),
            Mode::Unknown => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    const SCHEMA: &str = "
        CREATE TABLE session_v2(id TEXT PRIMARY KEY, project_id TEXT, parent_id TEXT, slug TEXT, directory TEXT, title TEXT, version TEXT,
            agent TEXT, model TEXT, time_created INTEGER, time_updated INTEGER, time_idle INTEGER, idle_outcome TEXT);
        CREATE TABLE session_message(id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER, time_created INTEGER, time_updated INTEGER, data TEXT);
        CREATE UNIQUE INDEX session_message_session_seq_idx ON session_message(session_id, seq);
        CREATE TABLE session(id TEXT PRIMARY KEY, project_id TEXT, parent_id TEXT, directory TEXT, title TEXT, version TEXT, agent TEXT, model TEXT,
            time_created INTEGER, time_updated INTEGER);
        CREATE TABLE message(id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
        CREATE TABLE part(id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
        CREATE INDEX part_session_idx ON part(session_id);
        PRAGMA journal_mode=WAL;";

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shipoffice-oc-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn msg(w: &Connection, sid: &str, seq: i64, kind: &str, t: i64, data: &str) {
        w.execute(
            "INSERT OR REPLACE INTO session_message VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![format!("{sid}-{seq}"), sid, kind, seq, t, t, data],
        )
        .unwrap();
    }

    fn kinds(r: &mut Box<dyn SourceReader>) -> Vec<EventKind> {
        r.poll().into_iter().map(|e| e.kind).collect()
    }

    #[test]
    fn v2_turn_streams_once_and_child_is_a_subagent() {
        let dir = temp("v2");
        let w = Connection::open(dir.join("opencode.db")).unwrap();
        w.execute_batch(SCHEMA).unwrap();
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64;
        w.execute(
            "INSERT INTO session_v2(id, project_id, parent_id, slug, directory, title, version, agent, model, time_created, time_updated) VALUES
             ('ses_a', 'p', NULL, 'clever-wolf', '/home/user/proj', 'Improve recovery', '2.0.29', 'build', '{\"id\":\"gpt-x\",\"providerID\":\"openai\"}', ?1, ?1),
             ('ses_b', 'p', 'ses_a', 'calm-fox', '/home/user/proj', 'Review fixes (@general subagent)', '2.0.29', 'general', NULL, ?1, ?1)",
            [now],
        )
        .unwrap();
        msg(&w, "ses_a", 1, "user", now, r#"{"time":{"created":1792271656963},"text":"Final focused review"}"#);
        // still streaming: the text item is last and the row has no completed time
        msg(&w, "ses_a", 2, "assistant", now, r#"{"time":{"created":1792274004999},"model":{"id":"gpt-x"},"content":[
            {"type":"reasoning","text":"","state":{"reasoningEncryptedContent":"x"},"time":{"created":1}},
            {"type":"tool","id":"call_1","name":"shell","state":{"status":"running","input":{"command":"cargo test"}},"time":{"created":1}},
            {"type":"text","text":"Implem"}]}"#);

        let oc = OpenCode::opencode();
        let specs = oc.discover(&[dir.clone()], SystemTime::now() - std::time::Duration::from_secs(60));
        assert_eq!(specs.len(), 2);
        let lead = specs.iter().find(|s| s.agent == AgentRef::Lead).unwrap();
        let child = specs.iter().find(|s| s.agent != AgentRef::Lead).unwrap();
        assert_eq!(child.key.id, "ses_a");
        assert_eq!(child.agent, AgentRef::Sub("ses_b".into()));

        let mut r = oc.open(lead);
        let k = kinds(&mut r);
        assert!(matches!(&k[0], EventKind::Meta { cwd: Some(c), title: Some(t), model: Some(m), .. } if c == Path::new("/home/user/proj") && t == "Improve recovery" && m == "gpt-x"));
        assert!(k.iter().any(|e| matches!(e, EventKind::UserPrompt { text } if text == "Final focused review")));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolCall { name, summary, .. } if name == "shell" && summary == "cargo test")));
        assert!(!k.iter().any(|e| matches!(e, EventKind::AssistantText { .. } | EventKind::ToolResult { .. })));
        assert!(kinds(&mut r).is_empty(), "nothing written, nothing read");

        w.execute(
            "UPDATE session_message SET time_updated=?1, data=?2 WHERE session_id='ses_a' AND seq=2",
            params![now + 5, r#"{"time":{"created":1792274004999,"completed":1792274024303},"model":{"id":"gpt-x"},"finish":"stop",
            "tokens":{"input":965,"output":260,"reasoning":414,"cache":{"read":178304,"write":0}},"content":[
            {"type":"reasoning","text":"","time":{"created":1,"completed":2}},
            {"type":"tool","id":"call_1","name":"shell","state":{"status":"completed","input":{"command":"cargo test"},"metadata":{"exit":0}},"time":{"created":1,"completed":2}},
            {"type":"text","text":"Implemented and packaged","state":{"phase":"final_answer"}}]}"#],
        )
        .unwrap();
        msg(&w, "ses_a", 3, "idle", now + 6, r#"{"time":{"created":1792274024305},"outcome":"succeeded"}"#);
        let k = kinds(&mut r);
        assert_eq!(k.iter().filter(|e| matches!(e, EventKind::ToolCall { .. } | EventKind::Thinking { .. })).count(), 0);
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolResult { call_id, ok: true } if call_id == "call_1")));
        assert!(k.iter().any(|e| matches!(e, EventKind::AssistantText { text } if text == "Implemented and packaged")));
        assert!(k.iter().any(|e| matches!(e, EventKind::Usage(u) if u.input == 965 && u.output == 674 && u.cache_read == 178304)));
        assert!(matches!(k.last(), Some(EventKind::TurnEnd)));

        msg(&w, "ses_b", 1, "idle", now + 7, r#"{"time":{"created":1},"outcome":"succeeded"}"#);
        let k = kinds(&mut oc.open(child));
        assert!(matches!(&k[0], EventKind::SubagentStart { agent_id, kind: Some(a), .. } if agent_id == "ses_b" && a == "general"));
        assert!(matches!(k.last(), Some(EventKind::SubagentEnd { agent_id }) if agent_id == "ses_b"));
        drop(w);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn v1_parts() {
        let dir = temp("v1");
        let db = dir.join("kilo.db");
        let w = Connection::open(&db).unwrap();
        w.execute_batch(SCHEMA).unwrap();
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64;
        w.execute("INSERT INTO session VALUES('ses_1', 'p', NULL, '/home/user/proj', 'Fix it', '1.1', 'build', NULL, ?1, ?1)", [now]).unwrap();
        w.execute_batch(r#"
            INSERT INTO message VALUES('msg_u', 'ses_1', 1, 1, '{"role":"user","time":{"created":1}}');
            INSERT INTO message VALUES('msg_a', 'ses_1', 2, 2, '{"role":"assistant","modelID":"claude-x","time":{"created":2}}');
            INSERT INTO part VALUES('prt_1', 'msg_u', 'ses_1', 1, 1, '{"type":"text","text":"Run the checks"}');
            INSERT INTO part VALUES('prt_2', 'msg_a', 'ses_1', 2, 2, '{"type":"tool","tool":"bash","callID":"call_e","state":{"status":"completed","input":{"command":"git diff --check"},"output":"(no output)","metadata":{"exit":0}}}');
            INSERT INTO part VALUES('prt_3', 'msg_a', 'ses_1', 3, 3, '{"type":"step-finish","reason":"stop","tokens":{"total":37395,"input":166,"output":109,"reasoning":0,"cache":{"write":0,"read":37120}},"cost":0}');
        "#).unwrap();
        let k = OpenCode::kilo();
        assert!(k.default_roots().iter().any(|r| r.ends_with("kilo")));
        let specs = k.discover(&[db.clone()], SystemTime::UNIX_EPOCH);
        assert_eq!(specs.len(), 1);
        let mut r = k.open(&specs[0]);
        let got = kinds(&mut r);
        assert!(got.iter().any(|e| matches!(e, EventKind::UserPrompt { text } if text == "Run the checks")));
        assert!(got.iter().any(|e| matches!(e, EventKind::Meta { model: Some(m), .. } if m == "claude-x")));
        assert!(got.iter().any(|e| matches!(e, EventKind::ToolCall { summary, .. } if summary == "git diff --check")));
        assert!(got.iter().any(|e| matches!(e, EventKind::ToolResult { ok: true, .. })));
        assert!(got.iter().any(|e| matches!(e, EventKind::Usage(u) if u.input == 166 && u.cache_read == 37120)));
        assert!(matches!(got.last(), Some(EventKind::TurnEnd)));
        // an unrelated write re-polls without repeating anything
        w.execute("UPDATE session SET time_updated=time_updated+1", []).unwrap();
        assert!(kinds(&mut r).iter().all(|e| matches!(e, EventKind::Meta { .. })));
        drop(w);
        std::fs::remove_dir_all(&dir).ok();
    }
}

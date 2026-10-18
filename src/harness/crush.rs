//! Crush: one SQLite database per project, `<project>/.crush/crush.db`, listed in a global
//! `projects.json` (`~/.local/share/crush`, `%LOCALAPPDATA%\crush` on Windows). Assistant rows are
//! created empty and updated in place while they stream. Agent tools run in child sessions.
//! See docs/harnesses.md §11.
//!
//! The `files` table holds file contents and is never read.

use super::{any_ts, clip, db_modified, env_path, tool_summary, xdg_data, Adapter, Db, DbCache, JsonFile, SourceReader, SourceSpec};
use crate::model::{AgentRef, Event, EventKind, SessionKey, Usage};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

const ID: &str = "crush";
const SCAN_LIMIT: i64 = 100;
const BACKLOG_ROWS: i64 = 200;
const POLL_LIMIT: i64 = 300;

/// (session, parent, updated) rows from one scan of a database.
type Scan = Vec<(String, Option<String>, SystemTime)>;
/// (database, project folder)
type Project = (PathBuf, PathBuf);

#[derive(Default)]
pub struct Crush {
    dbs: DbCache,
    /// projects.json path -> the file and the (database, project folder) pairs it lists
    indexes: Mutex<HashMap<PathBuf, (JsonFile, Vec<Project>)>>,
    /// database -> project folder, for the readers
    projects: Mutex<HashMap<PathBuf, PathBuf>>,
    /// db path -> (data_version at the last scan, what it found)
    scans: Mutex<HashMap<PathBuf, (i64, Scan)>>,
}

impl Crush {
    /// Databases under a root: the projects a `projects.json` lists, or a project / `.crush` folder given directly.
    fn dbs_in(&self, root: &Path) -> Vec<Project> {
        let index = root.join("projects.json");
        if index.is_file() {
            let mut idx = self.indexes.lock().unwrap();
            let (file, list) = idx.entry(index.clone()).or_insert_with(|| (JsonFile::new(index), Vec::new()));
            if let Some(v) = file.read() {
                *list = v["projects"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| {
                        let path = PathBuf::from(p.get("path")?.as_str()?);
                        let data = p.get("data_dir").and_then(Value::as_str).map(PathBuf::from).unwrap_or_else(|| ".crush".into());
                        let data = if data.is_absolute() { data } else { path.join(data) };
                        Some((data.join("crush.db"), path))
                    })
                    .collect();
            }
            return list.clone();
        }
        if root.join("crush.db").is_file() {
            let project = if root.file_name().is_some_and(|n| n == ".crush") { root.parent().unwrap_or(root) } else { root };
            return vec![(root.join("crush.db"), project.to_path_buf())];
        }
        let dot = root.join(".crush").join("crush.db");
        if dot.is_file() {
            return vec![(dot, root.to_path_buf())];
        }
        Vec::new()
    }
}

impl Adapter for Crush {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        "Crush"
    }
    fn default_roots(&self) -> Vec<PathBuf> {
        if let Some(p) = env_path("CRUSH_GLOBAL_DATA") {
            return vec![p];
        }
        if cfg!(windows) {
            return directories::BaseDirs::new().map(|b| vec![b.data_local_dir().join("crush")]).unwrap_or_default();
        }
        vec![xdg_data().join("crush")]
    }

    fn discover(&self, roots: &[PathBuf], since: SystemTime) -> Vec<SourceSpec> {
        let mut out = Vec::new();
        let mut done = HashSet::new();
        for (path, project) in roots.iter().flat_map(|r| self.dbs_in(r)) {
            if !done.insert(path.clone()) || db_modified(&path).is_none_or(|m| m < since) {
                continue;
            }
            self.projects.lock().unwrap().insert(path.clone(), project);
            let db = self.dbs.get(&path);
            let Some(version) = db.version() else { continue };
            let cached = self.scans.lock().unwrap().get(&path).filter(|(v, _)| *v == version).map(|(_, r)| r.clone());
            let rows = cached.unwrap_or_else(|| {
                let got = db.with(|c| {
                    let mut st = c.prepare("SELECT id, parent_session_id, updated_at FROM sessions ORDER BY updated_at DESC LIMIT ?1")?;
                    let it = st.query_map([SCAN_LIMIT], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, rusqlite::types::Value>(2)?)))?;
                    it.collect::<rusqlite::Result<Vec<_>>>()
                });
                let rows: Vec<_> = got
                    .unwrap_or_default()
                    .into_iter()
                    // `title-<parent>` sessions only generate a title
                    .filter(|(id, _, _)| !id.starts_with("title-"))
                    .map(|(id, parent, t)| (id, parent.filter(|p| !p.is_empty()), sql_time(&t)))
                    .collect();
                self.scans.lock().unwrap().insert(path.clone(), (version, rows.clone()));
                rows
            });
            let parents: HashMap<&str, Option<&str>> = rows.iter().map(|(id, p, _)| (id.as_str(), p.as_deref())).collect();
            for (id, parent, updated) in &rows {
                if *updated < since {
                    continue;
                }
                let mut root = parent.as_deref().unwrap_or(id);
                for _ in 0..8 {
                    match parents.get(root).copied().flatten() {
                        Some(p) => root = p,
                        None => break,
                    }
                }
                let agent = if root == id { AgentRef::Lead } else { AgentRef::Sub(id.clone()) };
                out.push(SourceSpec { key: SessionKey { harness: ID, id: root.to_owned() }, agent, path: path.join(id), modified: *updated });
            }
        }
        out
    }

    fn open(&self, spec: &SourceSpec) -> Box<dyn SourceReader> {
        let path = spec.path.parent().unwrap_or(&spec.path).to_path_buf();
        let sid = spec.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Box::new(Reader {
            db: self.dbs.get(&path),
            cwd: self.projects.lock().unwrap().get(&path).cloned(),
            sid,
            key: spec.key.clone(),
            agent: spec.agent.clone(),
            version: None,
            cursor: None,
            seen: HashMap::new(),
            calls: HashMap::new(),
            started: false,
            title: None,
            model: None,
            tokens: (0, 0),
        })
    }
}

/// Crush stores Unix seconds; tolerate text timestamps too.
fn sql_time(v: &rusqlite::types::Value) -> SystemTime {
    use rusqlite::types::Value as V;
    let j = match v {
        V::Integer(n) => Value::from(*n),
        V::Real(f) => Value::from(*f),
        V::Text(s) => Value::from(s.clone()),
        _ => Value::Null,
    };
    any_ts(&j).unwrap_or(SystemTime::UNIX_EPOCH)
}

/// What was already emitted for one message row.
#[derive(Default)]
struct Seen {
    parts: HashSet<usize>,
    called: HashSet<String>,
    finished: bool,
}

struct Reader {
    db: Arc<Db>,
    cwd: Option<PathBuf>,
    sid: String,
    key: SessionKey,
    agent: AgentRef,
    version: Option<i64>,
    /// `updated_at` watermark; one-second resolution, so rows at it are read again (and diffed)
    cursor: Option<i64>,
    seen: HashMap<String, Seen>,
    /// tool call id -> the assistant message holding it: an agent tool's child session is `<msg>$$<call>`
    calls: HashMap<String, String>,
    started: bool,
    title: Option<String>,
    model: Option<String>,
    /// session token totals already reported
    tokens: (u64, u64),
}

impl Reader {
    fn ev(&self, at: SystemTime, kind: EventKind) -> Event {
        Event { session: self.key.clone(), agent: self.agent.clone(), at, kind }
    }

    /// The session row: title and the token totals, which Crush only keeps per session.
    fn session(&mut self, out: &mut Vec<Event>) {
        let sid = self.sid.clone();
        let row = self.db.with(|c| {
            c.query_row("SELECT title, prompt_tokens, completion_tokens FROM sessions WHERE id=?1", [&sid], |r| {
                Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?))
            })
            .optional()
        });
        let Some(Some((title, prompt, completion))) = row else { return };
        let now = SystemTime::now();
        match &self.agent {
            AgentRef::Sub(id) if !self.started => {
                let description = title.as_deref().filter(|t| !t.is_empty()).map(|t| clip(t, 60)).unwrap_or_else(|| "Subagent".into());
                out.push(self.ev(now, EventKind::SubagentStart { agent_id: id.clone(), description, kind: None }));
            }
            AgentRef::Lead if !self.started || title != self.title => {
                let t = title.as_deref().filter(|t| !t.is_empty()).map(|t| clip(t, 60));
                out.push(self.ev(now, EventKind::Meta { cwd: self.cwd.clone(), branch: None, model: None, title: t }));
            }
            _ => {}
        }
        self.title = title;
        let (p, c) = (prompt.unwrap_or(0).max(0) as u64, completion.unwrap_or(0).max(0) as u64);
        if (p, c) != self.tokens {
            let u = Usage { input: p.saturating_sub(self.tokens.0), output: c.saturating_sub(self.tokens.1), ..Usage::default() };
            self.tokens = (p, c);
            if u != Usage::default() {
                out.push(self.ev(now, EventKind::Usage(u)));
            }
        }
    }

    fn messages(&mut self, out: &mut Vec<Event>) {
        let sid = self.sid.clone();
        let cursor = match self.cursor {
            Some(c) => c,
            None => {
                let start = self.db.with(|c| {
                    c.query_row("SELECT updated_at FROM messages WHERE session_id=?1 ORDER BY updated_at DESC LIMIT 1 OFFSET ?2", params![sid, BACKLOG_ROWS - 1], |r| r.get::<_, i64>(0))
                        .optional()
                });
                let Some(start) = start else { return };
                start.unwrap_or(0)
            }
        };
        let Some(rows) = self.db.with(|c| {
            let mut st = c.prepare_cached(
                "SELECT id, role, parts, model, created_at, updated_at, finished_at FROM messages WHERE session_id=?1 AND updated_at >= ?2 ORDER BY created_at, id LIMIT ?3",
            )?;
            let it = st.query_map(params![sid, cursor, POLL_LIMIT], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                ))
            })?;
            it.collect::<rusqlite::Result<Vec<_>>>()
        }) else {
            return;
        };
        self.cursor = Some(rows.iter().map(|r| r.5).max().unwrap_or(cursor).max(cursor));
        for (id, role, parts, model, created, _, finished_at) in rows {
            let mut seen = self.seen.remove(&id).unwrap_or_default();
            if !seen.finished {
                let parts: Value = serde_json::from_str(&parts).unwrap_or(Value::Null);
                let at = any_ts(&Value::from(created)).unwrap_or_else(SystemTime::now);
                self.message(&id, &role, &parts, model, finished_at.is_some(), at, &mut seen, out);
            }
            self.seen.insert(id, seen);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn message(&mut self, id: &str, role: &str, parts: &Value, model: Option<String>, finished: bool, at: SystemTime, seen: &mut Seen, out: &mut Vec<Event>) {
        let lead = self.agent == AgentRef::Lead;
        if let Some(m) = model.filter(|m| lead && role == "assistant" && !m.is_empty() && self.model.as_deref() != Some(m.as_str())) {
            self.model = Some(m.clone());
            out.push(self.ev(at, EventKind::Meta { cwd: None, branch: None, model: Some(m), title: None }));
        }
        let parts = parts.as_array().map(Vec::as_slice).unwrap_or_default();
        let finish = parts.iter().find(|p| p.get("type").and_then(Value::as_str) == Some("finish"));
        let complete = finished || finish.is_some() || role != "assistant";
        for (i, p) in parts.iter().enumerate() {
            let d = &p["data"];
            let streaming = !complete && i + 1 == parts.len();
            let text = d.get("text").and_then(Value::as_str).unwrap_or("");
            match p.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" if !streaming && seen.parts.insert(i) && !text.trim().is_empty() => match role {
                    "user" if lead => out.push(self.ev(at, EventKind::UserPrompt { text: clip(text, 140) })),
                    "assistant" => out.push(self.ev(at, EventKind::AssistantText { text: clip(text, 160) })),
                    _ => {}
                },
                "reasoning" if seen.parts.insert(i) => {
                    let t = d.get("thinking").and_then(Value::as_str).unwrap_or("");
                    out.push(self.ev(at, EventKind::Thinking { text: clip(t, 140) }));
                }
                "tool_call" => {
                    let call_id = d.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
                    // `input` is a JSON string that streams; wait until the call is complete
                    let ready = d.get("finished").and_then(Value::as_bool).unwrap_or(false) || complete;
                    if ready && seen.called.insert(call_id.clone()) {
                        self.calls.insert(call_id.clone(), id.to_owned());
                        let name = d.get("name").and_then(Value::as_str).unwrap_or("tool").to_owned();
                        let input = d.get("input").and_then(Value::as_str).and_then(|s| serde_json::from_str::<Value>(s).ok()).unwrap_or(Value::Null);
                        out.push(self.ev(at, EventKind::ToolCall { call_id, name, summary: tool_summary(&input) }));
                    }
                }
                "tool_result" if seen.parts.insert(i) => {
                    let call_id = d.get("tool_call_id").and_then(Value::as_str).unwrap_or("").to_owned();
                    let ok = !d.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                    let name = d.get("name").and_then(Value::as_str).unwrap_or("");
                    if matches!(name, "agent" | "agentic_fetch") {
                        if let Some(msg) = self.calls.get(&call_id) {
                            out.push(self.ev(at, EventKind::SubagentEnd { agent_id: format!("{msg}$${call_id}") }));
                        }
                    }
                    out.push(self.ev(at, EventKind::ToolResult { call_id, ok }));
                }
                _ => {}
            }
        }
        if role == "assistant" {
            if let Some(reason) = finish.and_then(|f| f.pointer("/data/reason")).and_then(Value::as_str) {
                if reason != "tool_use" {
                    out.push(match &self.agent {
                        AgentRef::Lead => self.ev(at, EventKind::TurnEnd),
                        AgentRef::Sub(id) => self.ev(at, EventKind::SubagentEnd { agent_id: id.clone() }),
                    });
                }
            }
        }
        seen.finished = complete;
    }
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
        self.messages(&mut out);
        self.started = true;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn streams_in_place_and_ends_on_finish() {
        let dir = std::env::temp_dir().join(format!("shipoffice-crush-{}", std::process::id()));
        let data = dir.join("global");
        let proj = dir.join("proj");
        std::fs::create_dir_all(proj.join(".crush")).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("projects.json"), serde_json::json!({"projects":[{"path": proj, "data_dir": ".crush", "last_accessed": "2026-10-18T10:00:00Z"}]}).to_string()).unwrap();
        let w = Connection::open(proj.join(".crush/crush.db")).unwrap();
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() as i64;
        w.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE sessions(id TEXT PRIMARY KEY, parent_session_id TEXT, title TEXT, message_count INTEGER, prompt_tokens INTEGER, completion_tokens INTEGER,
                 cost REAL, created_at INTEGER, updated_at INTEGER, summary_message_id TEXT, todos TEXT);
             CREATE TABLE messages(id TEXT PRIMARY KEY, session_id TEXT, role TEXT, parts TEXT, model TEXT, provider TEXT, created_at INTEGER, updated_at INTEGER,
                 finished_at INTEGER, is_summary_message INTEGER);
             CREATE INDEX idx_messages_session_id ON messages(session_id);",
        )
        .unwrap();
        w.execute(
            "INSERT INTO sessions(id, parent_session_id, title, prompt_tokens, completion_tokens, created_at, updated_at) VALUES
             ('s1', NULL, 'Read main', 100, 10, ?1, ?1), ('title-s1', 's1', 't', 0, 0, ?1, ?1), ('m2$$toolu_02', 's1', 'Explore', 0, 0, ?1, ?1)",
            [now],
        )
        .unwrap();
        let msg = |n: i64, id: &str, role: &str, parts: &str, fin: Option<i64>| {
            w.execute(
                "INSERT OR REPLACE INTO messages(id, session_id, role, parts, model, created_at, updated_at, finished_at) VALUES(?1, 's1', ?2, ?3, 'claude-x', ?4, ?4, ?5)",
                params![id, role, parts, now + n, fin],
            )
            .unwrap();
        };
        msg(1, "u", "user", r#"[{"type":"text","data":{"text":"Read main.go"}}]"#, Some(now));
        msg(2, "m1", "assistant", r#"[{"type":"reasoning","data":{"thinking":"look"}},{"type":"text","data":{"text":"Let me"}}]"#, None);

        let c = Crush::default();
        let specs = c.discover(&[data.clone()], SystemTime::now() - std::time::Duration::from_secs(60));
        assert_eq!(specs.len(), 2, "title sessions are skipped");
        assert!(specs.iter().any(|s| s.agent == AgentRef::Sub("m2$$toolu_02".into()) && s.key.id == "s1"));
        let lead = specs.iter().find(|s| s.agent == AgentRef::Lead).unwrap();
        let mut r = c.open(lead);
        let k: Vec<_> = r.poll().into_iter().map(|e| e.kind).collect();
        assert!(matches!(&k[0], EventKind::Meta { cwd: Some(p), title: Some(t), .. } if *p == proj && t == "Read main"));
        assert!(k.iter().any(|e| matches!(e, EventKind::Usage(u) if u.input == 100 && u.output == 10)));
        assert!(k.iter().any(|e| matches!(e, EventKind::UserPrompt { .. })));
        assert!(k.iter().any(|e| matches!(e, EventKind::Thinking { .. })));
        assert!(!k.iter().any(|e| matches!(e, EventKind::AssistantText { .. })));

        msg(2, "m1", "assistant", r#"[{"type":"reasoning","data":{"thinking":"look"}},{"type":"text","data":{"text":"Let me read main.go."}},
            {"type":"tool_call","data":{"id":"toolu_01XY","name":"view","input":"{\"file_path\":\"/home/user/proj/main.go\"}","finished":true}},
            {"type":"finish","data":{"reason":"tool_use","time":1760781005}}]"#, Some(now));
        msg(3, "t1", "tool", r#"[{"type":"tool_result","data":{"tool_call_id":"toolu_01XY","name":"view","content":"x","is_error":false}},{"type":"finish","data":{"reason":"stop","time":0}}]"#, Some(now));
        msg(4, "m3", "assistant", r#"[{"type":"text","data":{"text":"It's a CLI."}},{"type":"finish","data":{"reason":"end_turn","time":1}}]"#, Some(now));
        let k: Vec<_> = r.poll().into_iter().map(|e| e.kind).collect();
        assert_eq!(k.iter().filter(|e| matches!(e, EventKind::Thinking { .. })).count(), 0);
        assert!(k.iter().any(|e| matches!(e, EventKind::AssistantText { text } if text == "Let me read main.go.")));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolCall { name, summary, .. } if name == "view" && summary == "/home/user/proj/main.go")));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolResult { ok: true, .. })));
        assert_eq!(k.iter().filter(|e| matches!(e, EventKind::TurnEnd)).count(), 1);
        assert!(matches!(k.last(), Some(EventKind::TurnEnd)));
        drop(w);
        std::fs::remove_dir_all(&dir).ok();
    }
}

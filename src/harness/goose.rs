//! Goose: one SQLite database, `~/.local/share/goose/sessions/sessions.db` on Linux and macOS
//! (`%APPDATA%\Block\goose\data\sessions\sessions.db` on Windows). Messages are append-only rows;
//! `sub_agent` sessions point at their parent. See docs/harnesses.md §9.

use super::{any_ts, clip, db_modified, env_path, tool_summary, xdg_data, Adapter, Db, DbCache, SourceReader, SourceSpec};
use crate::model::{AgentRef, Event, EventKind, SessionKey, Usage};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

const ID: &str = "goose";
const SCAN_LIMIT: i64 = 100;
const BACKLOG_ROWS: i64 = 200;
const POLL_LIMIT: i64 = 300;

/// (session, parent, updated) rows from one scan of a database.
type Scan = Vec<(String, Option<String>, SystemTime)>;

#[derive(Default)]
pub struct Goose {
    dbs: DbCache,
    /// db path -> (data_version at the last scan, what it found)
    scans: Mutex<HashMap<PathBuf, (i64, Scan)>>,
}

impl Adapter for Goose {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        "Goose"
    }
    fn default_roots(&self) -> Vec<PathBuf> {
        let data = if let Some(root) = env_path("GOOSE_PATH_ROOT") {
            root.join("data")
        } else if cfg!(windows) {
            directories::BaseDirs::new().map(|b| b.data_dir().join("Block").join("goose").join("data")).unwrap_or_default()
        } else {
            // etcetera's app strategy is XDG on macOS too
            xdg_data().join("goose")
        };
        vec![data.join("sessions")]
    }

    fn discover(&self, roots: &[PathBuf], since: SystemTime) -> Vec<SourceSpec> {
        let mut out = Vec::new();
        for root in roots {
            let path = if root.is_file() { root.clone() } else { root.join("sessions.db") };
            if db_modified(&path).is_none_or(|m| m < since) {
                continue;
            }
            let db = self.dbs.get(&path);
            let Some(version) = db.version() else { continue };
            let cached = self.scans.lock().unwrap().get(&path).filter(|(v, _)| *v == version).map(|(_, r)| r.clone());
            let rows = cached.unwrap_or_else(|| {
                // the sessions table is small; `updated_at` may be text or a number, so sort here
                let got = db.with(|c| {
                    let mut st = c.prepare("SELECT id, parent_session_id, session_type, updated_at FROM sessions ORDER BY updated_at DESC LIMIT ?1")?;
                    let it = st.query_map([SCAN_LIMIT], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, rusqlite::types::Value>(3)?))
                    })?;
                    it.collect::<rusqlite::Result<Vec<_>>>()
                });
                let rows: Vec<_> = got
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(_, _, kind, _)| kind.as_deref() != Some("hidden"))
                    .map(|(id, parent, _, updated)| (id, parent, sql_time(&updated)))
                    .collect();
                self.scans.lock().unwrap().insert(path.clone(), (version, rows.clone()));
                rows
            });
            let parents: HashMap<&str, Option<&str>> = rows.iter().map(|(id, p, _)| (id.as_str(), p.as_deref())).collect();
            for (id, parent, updated) in &rows {
                if *updated < since {
                    continue;
                }
                // walk up through parents found in the same scan; a delegate's parent is active too
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
        let db = self.dbs.get(spec.path.parent().unwrap_or(&spec.path));
        let sid = spec.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Box::new(Reader {
            db,
            sid,
            key: spec.key.clone(),
            agent: spec.agent.clone(),
            version: None,
            cursor: None,
            seen: HashSet::new(),
            started: false,
            name: None,
            model: None,
            asked_tools: true,
        })
    }
}

/// `CURRENT_TIMESTAMP` text (`YYYY-MM-DD HH:MM:SS`, UTC), RFC 3339, or Unix seconds/ms.
fn sql_time(v: &rusqlite::types::Value) -> SystemTime {
    use rusqlite::types::Value as V;
    let j = match v {
        V::Integer(n) => Value::from(*n),
        V::Real(f) => Value::from(*f),
        V::Text(s) if s.len() == 19 => Value::from(format!("{}Z", s.replacen(' ', "T", 1))),
        V::Text(s) => Value::from(s.clone()),
        _ => Value::Null,
    };
    any_ts(&j).unwrap_or(SystemTime::UNIX_EPOCH)
}

struct Reader {
    db: Arc<Db>,
    sid: String,
    key: SessionKey,
    agent: AgentRef,
    version: Option<i64>,
    /// highest `messages.id` handled
    cursor: Option<i64>,
    /// `message_id`s handled: `replace_conversation` re-inserts old messages under new row ids
    seen: HashSet<String>,
    started: bool,
    name: Option<String>,
    model: Option<String>,
    /// the newest assistant message asked for a tool, so the turn goes on
    asked_tools: bool,
}

impl Reader {
    fn ev(&self, at: SystemTime, kind: EventKind) -> Event {
        Event { session: self.key.clone(), agent: self.agent.clone(), at, kind }
    }

    fn lead(&self) -> bool {
        self.agent == AgentRef::Lead
    }

    fn session(&mut self, out: &mut Vec<Event>) {
        let sid = self.sid.clone();
        let row = self.db.with(|c| {
            c.query_row("SELECT name, working_dir, model_config_json, description FROM sessions WHERE id=?1", [&sid], |r| {
                Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?))
            })
            .optional()
        });
        let Some(Some((name, dir, model, description))) = row else { return };
        let name = name.filter(|n| !n.is_empty()).or(description);
        if !self.started {
            if let AgentRef::Sub(id) = &self.agent {
                let description = name.as_deref().map(|n| clip(n, 60)).unwrap_or_else(|| "Subagent".into());
                out.push(self.ev(SystemTime::now(), EventKind::SubagentStart { agent_id: id.clone(), description, kind: None }));
            }
        }
        if self.lead() && (!self.started || name != self.name) {
            let model = model.and_then(|m| serde_json::from_str::<Value>(&m).ok()).and_then(|m| m.get("model_name").and_then(Value::as_str).map(str::to_owned));
            if model.is_some() {
                self.model.clone_from(&model);
            }
            let title = name.as_deref().map(|n| clip(n, 60));
            out.push(self.ev(SystemTime::now(), EventKind::Meta { cwd: dir.map(PathBuf::from), branch: None, model, title }));
        }
        self.name = name;
    }

    fn messages(&mut self, out: &mut Vec<Event>) {
        let sid = self.sid.clone();
        let cursor = match self.cursor {
            Some(c) => c,
            None => {
                let start = self.db.with(|c| {
                    c.query_row("SELECT id FROM messages WHERE session_id=?1 ORDER BY id DESC LIMIT 1 OFFSET ?2", params![sid, BACKLOG_ROWS], |r| r.get::<_, i64>(0)).optional()
                });
                let Some(start) = start else { return };
                start.unwrap_or(0)
            }
        };
        let Some(rows) = self.db.with(|c| {
            let mut st = c.prepare_cached(
                "SELECT id, message_id, role, content_json, created_timestamp, metadata_json FROM messages WHERE session_id=?1 AND id > ?2 ORDER BY id LIMIT ?3",
            )?;
            let it = st.query_map(params![sid, cursor, POLL_LIMIT], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            })?;
            it.collect::<rusqlite::Result<Vec<_>>>()
        }) else {
            return;
        };
        self.cursor = Some(rows.last().map_or(cursor, |r| r.0));
        let mut last_role = None;
        for (id, message_id, role, content, created, meta) in rows {
            if !self.seen.insert(message_id.unwrap_or_else(|| id.to_string())) {
                continue;
            }
            let at = created.and_then(|t| any_ts(&Value::from(t))).unwrap_or_else(SystemTime::now);
            let content: Value = serde_json::from_str(&content).unwrap_or(Value::Null);
            let meta: Value = meta.and_then(|m| serde_json::from_str(&m).ok()).unwrap_or(Value::Null);
            self.message(&role, &content, &meta, at, out);
            last_role = Some((role, at));
        }
        // no turn marker: the newest message is the model's and it asked for no tool
        if let Some((role, at)) = last_role {
            if role == "assistant" && !self.asked_tools {
                self.asked_tools = true;
                out.push(match &self.agent {
                    AgentRef::Lead => self.ev(at, EventKind::TurnEnd),
                    AgentRef::Sub(id) => self.ev(at, EventKind::SubagentEnd { agent_id: id.clone() }),
                });
            }
        }
    }

    fn message(&mut self, role: &str, content: &Value, meta: &Value, at: SystemTime, out: &mut Vec<Event>) {
        let items = content.as_array().map(Vec::as_slice).unwrap_or_default();
        let lead = self.lead();
        if role == "assistant" {
            let m = meta.pointer("/inference/resolvedModel").or_else(|| meta.pointer("/inference/requestedModel")).and_then(Value::as_str);
            if let Some(m) = m.filter(|m| lead && self.model.as_deref() != Some(*m)) {
                self.model = Some(m.to_owned());
                out.push(self.ev(at, EventKind::Meta { cwd: None, branch: None, model: Some(m.to_owned()), title: None }));
            }
            self.asked_tools = items.iter().any(|i| i.get("type").and_then(Value::as_str) == Some("toolRequest"));
        }
        // messages goose adds for the model only
        let hidden = meta.get("userVisible").and_then(Value::as_bool) == Some(false);
        for item in items {
            let text = item.get("text").and_then(Value::as_str).unwrap_or("");
            match (role, item.get("type").and_then(Value::as_str).unwrap_or("")) {
                ("user", "text") if lead && !hidden && !text.trim().is_empty() => out.push(self.ev(at, EventKind::UserPrompt { text: clip(text, 140) })),
                ("assistant", "text") if !text.trim().is_empty() => out.push(self.ev(at, EventKind::AssistantText { text: clip(text, 160) })),
                ("assistant", "thinking") => {
                    let t = item.get("thinking").and_then(Value::as_str).unwrap_or("");
                    out.push(self.ev(at, EventKind::Thinking { text: clip(t, 140) }));
                }
                ("assistant", "redactedThinking") => out.push(self.ev(at, EventKind::Thinking { text: String::new() })),
                (_, "toolRequest") => {
                    let call_id = item.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
                    let call = item.pointer("/toolCall/value").unwrap_or(&Value::Null);
                    // `<extension>__<tool>`: the tool part is what the office understands
                    let full = call.get("name").and_then(Value::as_str).unwrap_or("tool");
                    let name = full.rsplit("__").next().unwrap_or(full).to_owned();
                    out.push(self.ev(at, EventKind::ToolCall { call_id, name, summary: tool_summary(&call["arguments"]) }));
                }
                (_, "toolResponse") => {
                    let call_id = item.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
                    let r = &item["toolResult"];
                    let ok = r.get("status").and_then(Value::as_str).is_none_or(|s| s == "success") && !r.pointer("/value/isError").and_then(Value::as_bool).unwrap_or(false);
                    out.push(self.ev(at, EventKind::ToolResult { call_id, ok }));
                    // a delegate's result arrives when its child session is done
                    if let Some(child) = find_key(r, "subagent_session_id", 6).and_then(Value::as_str) {
                        out.push(self.ev(at, EventKind::SubagentEnd { agent_id: child.to_owned() }));
                    }
                }
                (_, "actionRequired") if item.pointer("/data/actionType").and_then(Value::as_str) == Some("toolConfirmation") => {
                    let what = item.pointer("/data/toolName").and_then(Value::as_str).map(|t| format!("Allow {t}?")).unwrap_or_else(|| "Waiting for approval".into());
                    out.push(self.ev(at, EventKind::NeedsInput { text: what }));
                }
                _ => {}
            }
        }
        if let Some(u) = meta.get("usage").filter(|_| role == "assistant") {
            out.push(self.ev(at, EventKind::Usage(usage(u))));
        }
    }
}

/// Goose's input count already includes the cached tokens; split them out.
fn usage(u: &Value) -> Usage {
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let (read, write) = (n("cacheReadTokens"), n("cacheWriteTokens"));
    Usage { input: n("inputTokens").saturating_sub(read + write), output: n("outputTokens"), cache_read: read, cache_write: write }
}

fn find_key<'a>(v: &'a Value, key: &str, depth: u8) -> Option<&'a Value> {
    if depth == 0 {
        return None;
    }
    match v {
        Value::Object(m) => m.get(key).or_else(|| m.values().find_map(|x| find_key(x, key, depth - 1))),
        Value::Array(a) => a.iter().find_map(|x| find_key(x, key, depth - 1)),
        _ => None,
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
    fn reads_messages_and_a_delegate() {
        let dir = std::env::temp_dir().join(format!("shipoffice-goose-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let w = Connection::open(dir.join("sessions.db")).unwrap();
        w.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE sessions(id TEXT PRIMARY KEY, name TEXT, description TEXT, session_type TEXT, working_dir TEXT, parent_session_id TEXT,
                 created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP, updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP, model_config_json TEXT);
             CREATE TABLE messages(id INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT, session_id TEXT, role TEXT, content_json TEXT,
                 created_timestamp INTEGER, timestamp TEXT, tokens INTEGER, metadata_json TEXT);
             INSERT INTO sessions(id, name, session_type, working_dir, parent_session_id, model_config_json) VALUES
                 ('20261018_1', 'Fix the tests', 'user', '/home/user/proj', NULL, '{\"model_name\":\"claude-x\"}'),
                 ('20261018_2', 'Look at tests', 'sub_agent', '/home/user/proj', '20261018_1', NULL),
                 ('20261018_3', 'title gen', 'hidden', '/home/user/proj', NULL, NULL);",
        )
        .unwrap();
        let add = |mid: &str, sid: &str, role: &str, content: &str, meta: &str| {
            w.execute(
                "INSERT INTO messages(message_id, session_id, role, content_json, created_timestamp, metadata_json) VALUES(?1, ?2, ?3, ?4, 1760781000, ?5)",
                params![mid, sid, role, content, meta],
            )
            .unwrap();
        };
        add("m1", "20261018_1", "user", r#"[{"type":"text","text":"Make the tests pass"}]"#, "{}");
        add("m2", "20261018_1", "assistant",
            r#"[{"type":"thinking","thinking":"plan","signature":"s"},{"type":"text","text":"Let me look at the tests."},
               {"type":"toolRequest","id":"toolu_01AB","toolCall":{"status":"success","value":{"name":"developer__shell","arguments":{"command":"ls tests"}}}}]"#,
            r#"{"usage":{"inputTokens":1200,"outputTokens":40,"cacheReadTokens":1000},"inference":{"resolvedModel":"claude-x"}}"#);
        add("m3", "20261018_1", "user",
            r#"[{"type":"toolResponse","id":"toolu_01AB","toolResult":{"status":"success","value":{"content":[{"type":"text","text":"x"}],"isError":false,"_meta":{"subagent_session_id":"20261018_2"}}}}]"#, "{}");
        add("m4", "20261018_1", "assistant", r#"[{"type":"text","text":"All green."}]"#, "{}");

        let g = Goose::default();
        let specs = g.discover(&[dir.clone()], SystemTime::now() - std::time::Duration::from_secs(3600));
        assert_eq!(specs.len(), 2, "hidden sessions are skipped");
        let sub = specs.iter().find(|s| s.agent != AgentRef::Lead).unwrap();
        assert_eq!(sub.key.id, "20261018_1");
        let lead = specs.iter().find(|s| s.agent == AgentRef::Lead).unwrap();
        let mut r = g.open(lead);
        let k: Vec<_> = r.poll().into_iter().map(|e| e.kind).collect();
        assert!(matches!(&k[0], EventKind::Meta { cwd: Some(_), title: Some(t), model: Some(m), .. } if t == "Fix the tests" && m == "claude-x"));
        assert!(k.iter().any(|e| matches!(e, EventKind::UserPrompt { text } if text == "Make the tests pass")));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolCall { name, summary, .. } if name == "shell" && summary == "ls tests")));
        assert!(k.iter().any(|e| matches!(e, EventKind::Usage(u) if u.input == 200 && u.cache_read == 1000)));
        assert!(k.iter().any(|e| matches!(e, EventKind::SubagentEnd { agent_id } if agent_id == "20261018_2")));
        assert!(matches!(k.last(), Some(EventKind::TurnEnd)));
        assert_eq!(k.iter().filter(|e| matches!(e, EventKind::Meta { .. })).count(), 1);

        // replace_conversation: the same messages under new row ids are not repeated
        add("m4", "20261018_1", "assistant", r#"[{"type":"text","text":"All green."}]"#, "{}");
        add("m5", "20261018_1", "user", r#"[{"type":"text","text":"Thanks"}]"#, "{}");
        let k: Vec<_> = r.poll().into_iter().map(|e| e.kind).collect();
        assert!(matches!(&k[..], [EventKind::UserPrompt { text }] if text == "Thanks"), "{k:?}");
        drop(w);
        std::fs::remove_dir_all(&dir).ok();
    }
}

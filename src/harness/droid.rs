//! Factory Droid: `~/.factory/sessions/<encoded-cwd>/<session>.jsonl`, append-only, plus a
//! rewritten `<session>.settings.json` that holds the model and the running token totals. A
//! `Task` subagent writes its own session whose `session_start` names the calling session.
//! See docs/harnesses.md §14.
//!
//! `~/.factory/.env` and the `auth*` files hold secrets and are never opened.

use super::{clip, env_path, home, modified, tool_summary, ts, Adapter, JsonFile, JsonlTail, SourceReader, SourceSpec};
use crate::model::{AgentRef, Event, EventKind, SessionKey, Usage};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

const ID: &str = "factory-droid";

#[derive(Default)]
pub struct Droid {
    /// transcript path -> (root session id, Some(own id) when it is a subagent's)
    heads: Mutex<HashMap<PathBuf, (String, Option<String>)>>,
}

impl Adapter for Droid {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        "Factory Droid"
    }
    fn default_roots(&self) -> Vec<PathBuf> {
        vec![env_path("FACTORY_HOME_OVERRIDE").unwrap_or_else(home).join(".factory")]
    }

    fn discover(&self, roots: &[PathBuf], since: SystemTime) -> Vec<SourceSpec> {
        let mut files = Vec::new();
        for root in roots {
            // current layout, older flat files, and the `projects/` variant some builds use
            walk(&root.join("sessions"), since, 0, &mut files);
            walk(&root.join("projects"), since, 0, &mut files);
        }
        let mut heads = self.heads.lock().unwrap();
        let mut ids = std::collections::HashSet::new();
        files
            .into_iter()
            .filter_map(|(path, m)| {
                if !heads.contains_key(&path) {
                    let head = read_head(&path)?;
                    heads.insert(path.clone(), head);
                }
                let (root, own) = heads.get(&path)?.clone();
                // the same session can sit in two layouts
                if !ids.insert(own.clone().unwrap_or_else(|| root.clone())) {
                    return None;
                }
                let agent = own.map_or(AgentRef::Lead, AgentRef::Sub);
                Some(SourceSpec { key: SessionKey { harness: ID, id: root }, agent, path, modified: m })
            })
            .collect()
    }

    fn open(&self, spec: &SourceSpec) -> Box<dyn SourceReader> {
        let settings = spec.path.with_extension("settings.json");
        Box::new(Reader {
            key: spec.key.clone(),
            agent: spec.agent.clone(),
            tail: JsonlTail::new(spec.path.clone()),
            settings: (spec.agent == AgentRef::Lead).then(|| JsonFile::new(settings)),
            tokens: Usage::default(),
            model: None,
            ended: false,
        })
    }
}

fn walk(dir: &Path, since: SystemTime, depth: u8, out: &mut Vec<(PathBuf, SystemTime)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() && depth < 1 {
            walk(&p, since, depth + 1, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            if let Some(m) = modified(&p).filter(|m| *m >= since) {
                out.push((p, m));
            }
        }
    }
}

/// `session_start` names the session and, for a subagent, the session that called it.
fn read_head(path: &Path) -> Option<(String, Option<String>)> {
    use std::io::BufRead;
    let f = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    std::io::BufReader::new(f).read_line(&mut line).ok()?;
    let v: Value = serde_json::from_str(&line).ok()?;
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let own = v.get("id").and_then(Value::as_str).map(str::to_owned).unwrap_or(stem);
    match v.get("callingSessionId").and_then(Value::as_str) {
        Some(parent) if parent != own => Some((parent.to_owned(), Some(own))),
        _ => Some((own, None)),
    }
}

struct Reader {
    key: SessionKey,
    agent: AgentRef,
    tail: JsonlTail,
    settings: Option<JsonFile>,
    /// totals from settings.json already reported
    tokens: Usage,
    model: Option<String>,
    ended: bool,
}

impl Reader {
    fn ev(&self, at: SystemTime, kind: EventKind) -> Event {
        Event { session: self.key.clone(), agent: self.agent.clone(), at, kind }
    }

    fn line(&mut self, v: &Value, out: &mut Vec<Event>) {
        let at = ts(&v["timestamp"]);
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "session_start" => {
                let title = v.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()).map(|t| clip(t, 60));
                match &self.agent {
                    AgentRef::Sub(id) => {
                        let description = title.unwrap_or_else(|| "Subagent".into());
                        out.push(self.ev(at, EventKind::SubagentStart { agent_id: id.clone(), description, kind: None }));
                    }
                    AgentRef::Lead => {
                        let cwd = v.get("cwd").and_then(Value::as_str).map(PathBuf::from);
                        out.push(self.ev(at, EventKind::Meta { cwd, branch: None, model: None, title }));
                    }
                }
            }
            "message" => self.message(v, at, out),
            // one source mentions it; harmless when it never shows up
            "agent_turn_outcome" => self.end(at, out),
            _ => {}
        }
    }

    fn message(&mut self, v: &Value, at: SystemTime, out: &mut Vec<Event>) {
        let msg = &v["message"];
        // reminders and summaries only one side sees
        if msg.get("visibility").or_else(|| v.get("visibility")).and_then(Value::as_str).is_some_and(|s| s == "user_only" || s == "llm_only") {
            return;
        }
        let lead = self.agent == AgentRef::Lead;
        let blocks: Vec<Value> = match &msg["content"] {
            Value::String(s) => vec![serde_json::json!({"type": "text", "text": s})],
            Value::Array(a) => a.clone(),
            _ => Vec::new(),
        };
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("");
        let mut called = false;
        let mut said = false;
        for b in &blocks {
            let text = b.get("text").and_then(Value::as_str).unwrap_or("");
            match (role, b.get("type").and_then(Value::as_str).unwrap_or("")) {
                ("user", "text") if lead && !text.trim().is_empty() && !text.trim_start().starts_with('<') => {
                    self.ended = false;
                    out.push(self.ev(at, EventKind::UserPrompt { text: clip(text, 140) }));
                }
                ("user", "tool_result") => {
                    let call_id = b.get("tool_use_id").and_then(Value::as_str).unwrap_or("").to_owned();
                    let ok = !b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                    out.push(self.ev(at, EventKind::ToolResult { call_id, ok }));
                }
                ("assistant", "text") if !text.trim().is_empty() => {
                    said = true;
                    out.push(self.ev(at, EventKind::AssistantText { text: clip(text, 160) }));
                }
                ("assistant", "thinking") => {
                    let t = b.get("thinking").and_then(Value::as_str).unwrap_or("");
                    out.push(self.ev(at, EventKind::Thinking { text: clip(t, 140) }));
                }
                ("assistant", "tool_use") => {
                    called = true;
                    self.ended = false;
                    let call_id = b.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
                    let name = b.get("name").and_then(Value::as_str).unwrap_or("tool").to_owned();
                    out.push(self.ev(at, EventKind::ToolCall { call_id, name, summary: tool_summary(&b["input"]) }));
                }
                _ => {}
            }
        }
        // no turn marker on disk: an answer that asks for no tool ends the turn
        if role == "assistant" && said && !called {
            self.end(at, out);
        }
    }

    fn end(&mut self, at: SystemTime, out: &mut Vec<Event>) {
        if self.ended {
            return;
        }
        self.ended = true;
        out.push(match &self.agent {
            AgentRef::Lead => self.ev(at, EventKind::TurnEnd),
            AgentRef::Sub(id) => self.ev(at, EventKind::SubagentEnd { agent_id: id.clone() }),
        });
    }

    /// Tokens only exist as running totals (subagents included) in settings.json: report the growth.
    fn settings(&mut self, out: &mut Vec<Event>) {
        let Some(v) = self.settings.as_mut().and_then(JsonFile::read) else { return };
        if let Some(m) = v.get("model").and_then(Value::as_str).filter(|m| self.model.as_deref() != Some(*m)) {
            self.model = Some(m.to_owned());
            out.push(self.ev(SystemTime::now(), EventKind::Meta { cwd: None, branch: None, model: Some(m.to_owned()), title: None }));
        }
        let t = &v["tokenUsage"];
        let n = |k: &str| t.get(k).and_then(Value::as_u64).unwrap_or(0);
        let total = Usage {
            input: n("inputTokens"),
            output: n("outputTokens") + n("thinkingTokens"),
            cache_read: n("cacheReadTokens"),
            cache_write: n("cacheCreationTokens"),
        };
        let delta = Usage {
            input: total.input.saturating_sub(self.tokens.input),
            output: total.output.saturating_sub(self.tokens.output),
            cache_read: total.cache_read.saturating_sub(self.tokens.cache_read),
            cache_write: total.cache_write.saturating_sub(self.tokens.cache_write),
        };
        self.tokens = total;
        if delta != Usage::default() {
            out.push(self.ev(SystemTime::now(), EventKind::Usage(delta)));
        }
    }
}

impl SourceReader for Reader {
    fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        for v in self.tail.read() {
            self.line(&v, &mut out);
        }
        self.settings(&mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_session_and_a_task_child() {
        let dir = std::env::temp_dir().join(format!("shipoffice-droid-{}", std::process::id()));
        let s = dir.join("sessions/-home-user-proj");
        std::fs::create_dir_all(&s).unwrap();
        std::fs::write(s.join("0dce.jsonl"), [
            r#"{"type":"session_start","id":"0dce","title":"Fix x.rs","version":2,"cwd":"/home/user/proj","owner":"[REDACTED]"}"#,
            r#"{"type":"message","id":"m1","timestamp":"2026-10-13T07:40:00.000Z","message":{"role":"user","content":[{"type":"text","text":"Fix x.rs"},{"type":"text","text":"<system-reminder>x</system-reminder>"}]}}"#,
            r#"{"type":"message","id":"m1b","timestamp":"2026-10-13T07:40:00.000Z","message":{"role":"user","visibility":"llm_only","content":[{"type":"text","text":"summary"}]}}"#,
            r#"{"type":"message","id":"m2","parentId":"m1","timestamp":"2026-10-13T07:40:01Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"/home/user/proj/src/x.rs"}}]}}"#,
            r#"{"type":"message","id":"m3","parentId":"m2","timestamp":"2026-10-13T07:40:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"[REDACTED]"}]}}"#,
            r#"{"type":"message","id":"m4","parentId":"m3","timestamp":"2026-10-13T07:40:03Z","message":{"role":"assistant","content":[{"type":"text","text":"Fixed."}]}}"#,
        ].join("\n") + "\n").unwrap();
        std::fs::write(s.join("0dce.settings.json"), r#"{"model":"claude-x","tokenUsage":{"inputTokens":100,"outputTokens":20,"cacheReadTokens":500,"thinkingTokens":5}}"#).unwrap();
        std::fs::write(s.join("77ab.jsonl"), r#"{"type":"session_start","id":"77ab","title":"Explore the repo","callingSessionId":"0dce","cwd":"/home/user/proj"}"#.to_string() + "\n").unwrap();

        let d = Droid::default();
        let specs = d.discover(&[dir.clone()], SystemTime::UNIX_EPOCH);
        assert_eq!(specs.len(), 2);
        assert!(specs.iter().all(|s| s.key.id == "0dce"));
        let lead = specs.iter().find(|s| s.agent == AgentRef::Lead).unwrap();
        let k: Vec<_> = d.open(lead).poll().into_iter().map(|e| e.kind).collect();
        assert!(matches!(&k[0], EventKind::Meta { cwd: Some(_), title: Some(t), .. } if t == "Fix x.rs"));
        assert_eq!(k.iter().filter(|e| matches!(e, EventKind::UserPrompt { .. })).count(), 1);
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolCall { summary, .. } if summary == "/home/user/proj/src/x.rs")));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolResult { ok: true, .. })));
        assert!(k.iter().any(|e| matches!(e, EventKind::TurnEnd)));
        assert!(k.iter().any(|e| matches!(e, EventKind::Usage(u) if u.input == 100 && u.output == 25 && u.cache_read == 500)));
        let sub = specs.iter().find(|s| s.agent != AgentRef::Lead).unwrap();
        let k: Vec<_> = d.open(sub).poll().into_iter().map(|e| e.kind).collect();
        assert!(matches!(&k[..], [EventKind::SubagentStart { agent_id, description, .. }] if agent_id == "77ab" && description == "Explore the repo"));
        std::fs::remove_dir_all(&dir).ok();
    }
}

//! Claude Code: `~/.claude/projects/<encoded-cwd>/<session>.jsonl`, plus one transcript per
//! subagent under `<session>/subagents/` (and the two older layouts). See docs/harnesses.md §1.

use super::{clip, env_path, home, modified, read_capped, tool_summary, ts, Adapter, JsonlTail, MAX_LINE, SourceReader, SourceSpec};
use crate::model::{AgentRef, Event, EventKind, SessionKey, Usage};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

pub struct ClaudeCode;

const ID: &str = "claude-code";
/// Biggest `meta.json` or `sessions/<pid>.json` that is read.
const SIDE_FILE: u64 = 1024 * 1024;

impl Adapter for ClaudeCode {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        "Claude Code"
    }
    fn default_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![home().join(".claude")];
        if let Some(p) = env_path("CLAUDE_CONFIG_DIR") {
            roots.insert(0, p);
        }
        roots.dedup();
        roots
    }

    fn discover(&self, roots: &[PathBuf], since: SystemTime) -> Vec<SourceSpec> {
        let mut out = Vec::new();
        for root in roots {
            let Ok(projects) = std::fs::read_dir(root.join("projects")) else { continue };
            for project in projects.flatten() {
                let dir = project.path();
                let Ok(entries) = std::fs::read_dir(&dir) else { continue };
                for e in entries.flatten() {
                    let p = e.path();
                    let Some(stem) = p.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else { continue };
                    if p.extension().is_some_and(|x| x == "jsonl") {
                        let Some(m) = modified(&p).filter(|m| *m >= since) else { continue };
                        if let Some(agent) = stem.strip_prefix("agent-") {
                            // 2.0.x layout: sidechains next to the session file; the parent id is inside
                            if let Some(sid) = first_session_id(&p) {
                                out.push(spec(&sid, AgentRef::Sub(agent.to_owned()), p, m));
                            }
                        } else {
                            out.push(spec(&stem, AgentRef::Lead, p.clone(), m));
                            subagents(&dir.join(&stem).join("subagents"), &stem, since, &mut out);
                        }
                    }
                }
            }
        }
        out
    }

    fn open(&self, spec: &SourceSpec) -> Box<dyn SourceReader> {
        let root = spec.path.ancestors().find(|a| a.join("projects").is_dir()).map(Path::to_path_buf);
        let meta = match &spec.agent {
            AgentRef::Sub(_) => read_capped(&spec.path.with_extension("meta.json"), SIDE_FILE)
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok()),
            AgentRef::Lead => None,
        };
        let project_dir = spec
            .path
            .ancestors()
            .find(|a| a.parent().is_some_and(|p| p.file_name().is_some_and(|n| n == "projects")))
            .and_then(|a| a.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        Box::new(Reader {
            project_dir,
            key: spec.key.clone(),
            agent: spec.agent.clone(),
            tail: JsonlTail::new(spec.path.clone()),
            meta,
            started: false,
            seen_messages: HashSet::new(),
            sessions_dir: root.map(|r| r.join("sessions")),
            last_status: None,
            status_checked: None,
        })
    }
}

fn spec(session: &str, agent: AgentRef, path: PathBuf, modified: SystemTime) -> SourceSpec {
    SourceSpec { key: SessionKey { harness: ID, id: session.to_owned() }, agent, path, modified }
}

fn subagents(dir: &Path, session: &str, since: SystemTime, out: &mut Vec<SourceSpec>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            // workflows/wf_<id>/agent-*.jsonl
            subagents(&p, session, since, out);
            continue;
        }
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if let Some(id) = name.strip_prefix("agent-").and_then(|n| n.strip_suffix(".jsonl")) {
            if let Some(m) = modified(&p).filter(|m| *m >= since) {
                out.push(spec(session, AgentRef::Sub(id.to_owned()), p, m));
            }
        }
    }
}

/// Every session's `(status, waitingFor)` from `sessions/<pid>.json`. One scan serves all the
/// readers that ask within a second, whatever their number.
fn statuses(dir: &Path) -> HashMap<String, (String, Option<String>)> {
    type Scan = (PathBuf, Instant, HashMap<String, (String, Option<String>)>);
    static CACHE: Mutex<Option<Scan>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((d, at, m)) = cache.as_ref() {
        if d == dir && at.elapsed() < Duration::from_secs(1) {
            return m.clone();
        }
    }
    let mut found = HashMap::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        // only `<pid>.json`; the `.key` files beside them hold tokens and are never opened
        if !name.ends_with(".json") || name.matches('.').count() != 1 {
            continue;
        }
        let Some(v) = read_capped(&e.path(), SIDE_FILE).and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
        let Some(id) = v.get("sessionId").and_then(Value::as_str) else { continue };
        let status = v.get("status").and_then(Value::as_str).unwrap_or("").to_owned();
        found.insert(id.to_owned(), (status, v.get("waitingFor").and_then(Value::as_str).map(str::to_owned)));
    }
    *cache = Some((dir.to_path_buf(), Instant::now(), found.clone()));
    found
}

fn first_session_id(path: &Path) -> Option<String> {
    use std::io::{BufRead, Read};
    let f = std::fs::File::open(path).ok()?;
    // a first line past the cap is cut short, fails to parse and is skipped
    std::io::BufReader::new(f.take(MAX_LINE as u64)).lines().take(5).flatten().find_map(|l| {
        serde_json::from_str::<Value>(&l).ok()?.get("sessionId")?.as_str().map(str::to_owned)
    })
}

struct Reader {
    /// `projects/<this>`: the session's starting folder, encoded
    project_dir: Option<String>,
    key: SessionKey,
    agent: AgentRef,
    tail: JsonlTail,
    meta: Option<Value>,
    started: bool,
    seen_messages: HashSet<String>,
    sessions_dir: Option<PathBuf>,
    last_status: Option<String>,
    status_checked: Option<Instant>,
}

impl Reader {
    fn ev(&self, at: SystemTime, kind: EventKind) -> Event {
        Event { session: self.key.clone(), agent: self.agent.clone(), at, kind }
    }

    /// `sessions/<pid>.json` says whether the CLI is busy, idle or waiting on a dialog.
    fn live_status(&mut self, out: &mut Vec<Event>) {
        if self.status_checked.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
            return;
        }
        self.status_checked = Some(Instant::now());
        let Some(dir) = &self.sessions_dir else { return };
        let Some((status, waiting_for)) = statuses(dir).remove(self.key.id.as_str()) else { return };
        if self.last_status.as_deref() != Some(status.as_str()) {
            if status == "waiting" {
                let what = waiting_for.as_deref().unwrap_or("Waiting for your input");
                out.push(self.ev(SystemTime::now(), EventKind::NeedsInput { text: clip(what, 80) }));
            }
            self.last_status = Some(status);
        }
    }

    fn line(&mut self, v: &Value, out: &mut Vec<Event>) {
        let at = ts(&v["timestamp"]);
        let sub = matches!(self.agent, AgentRef::Sub(_));
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "user" if !v.get("isMeta").and_then(Value::as_bool).unwrap_or(false) => self.user(v, at, sub, out),
            "assistant" => self.assistant(v, at, sub, out),
            "system" if v.get("subtype").and_then(Value::as_str) == Some("turn_duration") && !sub => {
                out.push(self.ev(at, EventKind::TurnEnd));
            }
            "ai-title" | "custom-title" => {
                let t = v.get("customTitle").or_else(|| v.get("aiTitle")).and_then(Value::as_str);
                if let Some(t) = t {
                    out.push(self.ev(at, EventKind::Meta { cwd: None, branch: None, model: None, title: Some(clip(t, 60)) }));
                }
            }
            _ => {}
        }
    }

    fn user(&mut self, v: &Value, at: SystemTime, sub: bool, out: &mut Vec<Event>) {
        let content = &v["message"]["content"];
        if let Some(s) = content.as_str() {
            if let Some(n) = s.trim_start().strip_prefix("<task-notification>") {
                // a background subagent finished
                if let Some(id) = between(n, "<task-id>", "</task-id>") {
                    out.push(self.ev(at, EventKind::SubagentEnd { agent_id: id.to_owned() }));
                }
            } else if !sub && !s.trim_start().starts_with('<') {
                out.push(self.ev(at, EventKind::UserPrompt { text: clip(s, 140) }));
            }
            return;
        }
        for block in content.as_array().into_iter().flatten() {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_result") => {
                    let id = block.get("tool_use_id").and_then(Value::as_str).unwrap_or("").to_owned();
                    let ok = !block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                    out.push(self.ev(at, EventKind::ToolResult { call_id: id, ok }));
                }
                Some("text") if !sub => {
                    let t = block.get("text").and_then(Value::as_str).unwrap_or("");
                    if !t.trim_start().starts_with('<') {
                        out.push(self.ev(at, EventKind::UserPrompt { text: clip(t, 140) }));
                    }
                }
                _ => {}
            }
        }
    }

    fn assistant(&mut self, v: &Value, at: SystemTime, sub: bool, out: &mut Vec<Event>) {
        let msg = &v["message"];
        let model = msg.get("model").and_then(Value::as_str).filter(|m| !m.starts_with('<'));
        if !self.started || model.is_some() {
            let cwd = v.get("cwd").and_then(Value::as_str).map(|c| project_root(Path::new(c), self.project_dir.as_deref()));
            let branch = v.get("gitBranch").and_then(Value::as_str).map(str::to_owned);
            out.push(self.ev(at, EventKind::Meta { cwd, branch, model: model.map(str::to_owned), title: None }));
        }
        // usage repeats on every content-block line of one message: count it once
        let id = msg.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
        if !id.is_empty() && self.seen_messages.insert(id) {
            let u = &msg["usage"];
            let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            let usage = Usage {
                input: n("input_tokens"),
                output: n("output_tokens"),
                cache_read: n("cache_read_input_tokens"),
                cache_write: n("cache_creation_input_tokens"),
            };
            if usage != Usage::default() {
                out.push(self.ev(at, EventKind::Usage(usage)));
            }
        }
        for block in msg["content"].as_array().into_iter().flatten() {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let t = block.get("text").and_then(Value::as_str).unwrap_or("");
                    if !t.trim().is_empty() {
                        out.push(self.ev(at, EventKind::AssistantText { text: clip(t, 160) }));
                    }
                }
                Some("thinking") => {
                    let t = block.get("thinking").and_then(Value::as_str).unwrap_or("");
                    out.push(self.ev(at, EventKind::Thinking { text: clip(t, 140) }));
                }
                Some("tool_use") => {
                    let call_id = block.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("tool").to_owned();
                    let summary = tool_summary(&block["input"]);
                    out.push(self.ev(at, EventKind::ToolCall { call_id, name, summary }));
                }
                _ => {}
            }
        }
        if sub && msg.get("stop_reason").and_then(Value::as_str) == Some("end_turn") {
            if let AgentRef::Sub(id) = &self.agent {
                out.push(self.ev(at, EventKind::SubagentEnd { agent_id: id.clone() }));
            }
        }
    }
}

impl SourceReader for Reader {
    fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let lines = self.tail.read();
        if !self.started && !lines.is_empty() {
            if let AgentRef::Sub(id) = &self.agent {
                let m = self.meta.as_ref();
                let description = m
                    .and_then(|m| m.get("description").and_then(Value::as_str))
                    .map(str::to_owned)
                    .unwrap_or_else(|| "Subagent".into());
                let kind = m.and_then(|m| m.get("agentType").and_then(Value::as_str)).map(str::to_owned);
                // the start is when the helper's first line was written, not when we found it
                let at = lines[0].get("timestamp").map_or_else(SystemTime::now, ts);
                out.push(self.ev(at, EventKind::SubagentStart { agent_id: id.clone(), description, kind }));
            }
        }
        let first_read = !self.started;
        for v in &lines {
            self.line(v, &mut out);
            self.started = true;
        }
        // a helper whose transcript went quiet long ago finished before we started watching,
        // even if the lead's completion notice is outside the part of its log we read
        if first_read && self.started {
            if let (AgentRef::Sub(id), Some(m)) = (&self.agent, self.tail.modified()) {
                if SystemTime::now().duration_since(m).unwrap_or_default() > std::time::Duration::from_secs(600) {
                    out.push(self.ev(m, EventKind::SubagentEnd { agent_id: id.clone() }));
                }
            }
        }
        if matches!(self.agent, AgentRef::Lead) {
            self.live_status(&mut out);
        }
        out
    }
}

/// The folder name under `projects/` is the starting cwd with every non-alphanumeric byte
/// turned into `-`. The cwd on a record may be a subfolder after a `cd`, so walk up to the
/// ancestor whose encoding matches; that's the project.
fn project_root(cwd: &Path, encoded: Option<&str>) -> PathBuf {
    let enc = |p: &Path| -> String {
        p.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
    };
    if let Some(e) = encoded {
        if let Some(a) = cwd.ancestors().find(|a| enc(a) == e) {
            return a.to_path_buf();
        }
    }
    cwd.to_path_buf()
}

fn between<'a>(s: &'a str, a: &str, b: &str) -> Option<&'a str> {
    let start = s.find(a)? + a.len();
    let end = s[start..].find(b)? + start;
    Some(s[start..end].trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lines: &[&str], agent: AgentRef) -> Vec<EventKind> {
        let dir = std::env::temp_dir().join(format!("shipoffice-cc-{}-{}", std::process::id(), lines.len()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("s.jsonl");
        std::fs::write(&p, lines.join("\n") + "\n").unwrap();
        let spec = spec("s", agent, p, SystemTime::now());
        let mut r = ClaudeCode.open(&spec);
        let got = r.poll().into_iter().map(|e| e.kind).collect();
        std::fs::remove_dir_all(&dir).ok();
        got
    }

    #[test]
    fn parses_a_turn_and_counts_usage_once() {
        let k = run(
            &[
                r#"{"type":"user","timestamp":"2026-10-15T17:41:36.996Z","cwd":"/p","message":{"role":"user","content":"Fix the flaky test"}}"#,
                r#"{"type":"assistant","cwd":"/p","gitBranch":"main","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"text","text":"Looking at it"}],"usage":{"input_tokens":3,"output_tokens":10,"cache_read_input_tokens":100}}}"#,
                r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}],"usage":{"input_tokens":3,"output_tokens":10,"cache_read_input_tokens":100}}}"#,
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true}]}}"#,
                r#"{"type":"system","subtype":"turn_duration","durationMs":5}"#,
            ],
            AgentRef::Lead,
        );
        assert!(matches!(&k[0], EventKind::UserPrompt { text } if text == "Fix the flaky test"));
        assert_eq!(k.iter().filter(|e| matches!(e, EventKind::Usage(_))).count(), 1);
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolCall { name, summary, .. } if name == "Bash" && summary == "cargo test")));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolResult { ok: false, .. })));
        assert!(matches!(k.last(), Some(EventKind::TurnEnd)));
    }

    #[test]
    fn subagent_start_and_end() {
        let k = run(
            &[
                r#"{"type":"user","isSidechain":true,"message":{"content":"task prompt"}}"#,
                r#"{"type":"assistant","message":{"id":"m2","stop_reason":"end_turn","content":[{"type":"text","text":"Done"}]}}"#,
            ],
            AgentRef::Sub("abc".into()),
        );
        assert!(matches!(&k[0], EventKind::SubagentStart { agent_id, .. } if agent_id == "abc"));
        assert!(!k.iter().any(|e| matches!(e, EventKind::UserPrompt { .. })));
        assert!(matches!(k.last(), Some(EventKind::SubagentEnd { agent_id }) if agent_id == "abc"));
    }

    #[test]
    fn project_root_undoes_a_cd() {
        assert_eq!(project_root(Path::new("/home/u/a.b/c/raw"), Some("-home-u-a-b-c")), PathBuf::from("/home/u/a.b/c"));
        assert_eq!(project_root(Path::new("/x/y"), Some("-nope")), PathBuf::from("/x/y"));
    }

    #[test]
    fn task_notification_ends_a_background_agent() {
        let k = run(
            &[r#"{"type":"user","message":{"content":"<task-notification>\n<task-id>a78</task-id>\n<status>completed</status></task-notification>"}}"#],
            AgentRef::Lead,
        );
        assert!(matches!(&k[0], EventKind::SubagentEnd { agent_id } if agent_id == "a78"));
    }
}

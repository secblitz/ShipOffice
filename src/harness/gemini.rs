//! Gemini CLI: `~/.gemini/tmp/<project slug>/chats/session-*.jsonl`, an append-only log where a
//! message is appended again under the same `id` whenever it changes, plus `$set`/`$patch`/
//! `$rewindTo` lines. Subagents write `chats/<parent session id>/<agent id>.jsonl`. Older builds
//! rewrote one `session-*.json` whole. See docs/harnesses.md §4.

use super::{clip, env_path, home, modified, read_capped, tool_summary, ts, Adapter, JsonFile, JsonlTail, SourceReader, SourceSpec};
use crate::model::{AgentRef, Event, EventKind, SessionKey, Usage};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

const ID: &str = "gemini-cli";

/// The log has no turn-finished record: a final answer followed by this much silence counts as one.
const QUIET_END: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct GeminiCli {
    /// transcript path -> session id from its header (the file name only has the first 8 characters)
    heads: Mutex<HashMap<PathBuf, String>>,
}

impl Adapter for GeminiCli {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        "Gemini CLI"
    }
    fn default_roots(&self) -> Vec<PathBuf> {
        let base = env_path("GEMINI_CLI_HOME").unwrap_or_else(home);
        // the macOS seatbelt sandbox moves the runtime dir to ~/.cache/.gemini
        let mut roots = vec![base.join(".gemini"), home().join(".cache").join(".gemini")];
        roots.dedup();
        roots
    }

    fn discover(&self, roots: &[PathBuf], since: SystemTime) -> Vec<SourceSpec> {
        let mut out = Vec::new();
        let mut heads = self.heads.lock().unwrap();
        for root in roots {
            let Ok(projects) = std::fs::read_dir(root.join("tmp")) else { continue };
            for project in projects.flatten() {
                let Ok(entries) = std::fs::read_dir(project.path().join("chats")) else { continue };
                for e in entries.flatten() {
                    let p = e.path();
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_owned();
                    if p.is_dir() {
                        // chats/<parent session id>/<agent id>.jsonl
                        subagents(&p, &name, since, &mut out);
                        continue;
                    }
                    if !name.starts_with("session-") || !(name.ends_with(".jsonl") || name.ends_with(".json")) {
                        continue;
                    }
                    let Some(m) = modified(&p).filter(|m| *m >= since) else { continue };
                    if !heads.contains_key(&p) {
                        let Some(id) = session_id(&p) else { continue };
                        heads.insert(p.clone(), id);
                    }
                    let id = heads[&p].clone();
                    out.push(SourceSpec { key: SessionKey { harness: ID, id }, agent: AgentRef::Lead, path: p, modified: m });
                }
            }
        }
        out
    }

    fn open(&self, spec: &SourceSpec) -> Box<dyn SourceReader> {
        let legacy = spec.path.extension().is_some_and(|x| x == "json");
        let source = if legacy { Source::Whole(JsonFile::new(spec.path.clone())) } else { Source::Log(JsonlTail::new(spec.path.clone())) };
        // tmp/<slug>/chats/<file> (or chats/<parent>/<file>): `.project_root` names the folder
        let cwd = spec
            .path
            .ancestors()
            .find(|a| a.file_name().is_some_and(|n| n == "chats"))
            .and_then(|chats| read_capped(&chats.with_file_name(".project_root"), 4096))
            .map(|b| PathBuf::from(String::from_utf8_lossy(&b).trim()))
            .filter(|p| !p.as_os_str().is_empty());
        Box::new(Reader {
            key: spec.key.clone(),
            agent: spec.agent.clone(),
            source,
            cwd,
            model: None,
            started: false,
            messages: HashMap::new(),
            order: Vec::new(),
            last_line: None,
            ended: true,
        })
    }
}

fn subagents(dir: &Path, parent: &str, since: SystemTime, out: &mut Vec<SourceSpec>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "jsonl") {
            let Some(agent) = p.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else { continue };
            if let Some(m) = modified(&p).filter(|m| *m >= since) {
                out.push(SourceSpec { key: SessionKey { harness: ID, id: parent.to_owned() }, agent: AgentRef::Sub(agent), path: p, modified: m });
            }
        }
    }
}

/// The header's `sessionId`: the first line of a log, or an early key of a legacy whole-file session.
fn session_id(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut head = vec![0u8; 16 * 1024];
    let n = std::fs::File::open(path).ok()?.read(&mut head).ok()?;
    let head = String::from_utf8_lossy(&head[..n]);
    let rest = &head[head.find("\"sessionId\"")? + "\"sessionId\"".len()..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_owned()).filter(|s| !s.is_empty())
}

enum Source {
    Log(JsonlTail),
    Whole(JsonFile),
}

/// What was already emitted for one message id.
#[derive(Default)]
struct Seen {
    text: bool,
    thoughts: usize,
    usage: bool,
    called: HashSet<String>,
    finished: HashSet<String>,
    /// a final answer: text, no tool calls
    answer: bool,
}

struct Reader {
    key: SessionKey,
    agent: AgentRef,
    source: Source,
    cwd: Option<PathBuf>,
    model: Option<String>,
    started: bool,
    messages: HashMap<String, Seen>,
    /// message ids in log order, for `$rewindTo`
    order: Vec<String>,
    last_line: Option<Instant>,
    ended: bool,
}

impl Reader {
    fn ev(&self, at: SystemTime, kind: EventKind) -> Event {
        Event { session: self.key.clone(), agent: self.agent.clone(), at, kind }
    }

    fn line(&mut self, v: &Value, out: &mut Vec<Event>) {
        if let Some(id) = v.get("id").and_then(Value::as_str) {
            self.record(id, v, out);
        } else if let Some(set) = v.get("$set") {
            self.header(set, out);
        } else if let Some(p) = v.get("$patch") {
            self.patch(p, out);
        } else if let Some(to) = v.get("$rewindTo").and_then(Value::as_str) {
            if let Some(i) = self.order.iter().position(|m| m == to) {
                for gone in self.order.drain(i..) {
                    self.messages.remove(&gone);
                }
            }
        } else if v.get("sessionId").is_some() {
            self.header(v, out);
        }
    }

    fn header(&mut self, v: &Value, out: &mut Vec<Event>) {
        let title = v.get("summary").and_then(Value::as_str).filter(|s| !s.is_empty()).map(|s| clip(s, 60));
        if matches!(self.agent, AgentRef::Lead) && (title.is_some() || !self.started) {
            let at = v.get("lastUpdated").or_else(|| v.get("startTime")).map(ts).unwrap_or_else(SystemTime::now);
            out.push(self.ev(at, EventKind::Meta { cwd: self.cwd.clone(), branch: None, model: None, title }));
        }
    }

    /// Upserts one MessageRecord: only what is new since the last copy of the same id is emitted.
    fn record(&mut self, id: &str, v: &Value, out: &mut Vec<Event>) {
        let at = ts(&v["timestamp"]);
        let mut seen = self.messages.remove(id).unwrap_or_else(|| {
            self.order.push(id.to_owned());
            Seen::default()
        });
        let lead = matches!(self.agent, AgentRef::Lead);
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "user" if !seen.text => {
                seen.text = true;
                // tool results come back as user messages holding functionResponse parts
                let parts = v["content"].as_array().map(Vec::as_slice).unwrap_or_default();
                if lead && !parts.iter().any(|p| p.get("functionResponse").is_some()) {
                    let t = v.get("displayContent").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| text_of(&v["content"]));
                    if !t.trim().is_empty() {
                        out.push(self.ev(at, EventKind::UserPrompt { text: clip(&t, 140) }));
                    }
                }
            }
            "gemini" => {
                if let Some(m) = v.get("model").and_then(Value::as_str).filter(|m| lead && self.model.as_deref() != Some(*m)) {
                    self.model = Some(m.to_owned());
                    out.push(self.ev(at, EventKind::Meta { cwd: None, branch: None, model: Some(m.to_owned()), title: None }));
                }
                let thoughts = v["thoughts"].as_array().map(Vec::as_slice).unwrap_or_default();
                for t in thoughts.iter().skip(seen.thoughts) {
                    let s = t.get("subject").and_then(Value::as_str).or_else(|| t.get("description").and_then(Value::as_str)).unwrap_or("");
                    out.push(self.ev(ts(&t["timestamp"]), EventKind::Thinking { text: clip(s, 140) }));
                }
                seen.thoughts = seen.thoughts.max(thoughts.len());
                let text = text_of(&v["content"]);
                if !seen.text && !text.trim().is_empty() {
                    seen.text = true;
                    out.push(self.ev(at, EventKind::AssistantText { text: clip(&text, 160) }));
                }
                if let Some(t) = v.get("tokens").filter(|_| !seen.usage) {
                    seen.usage = true;
                    out.push(self.ev(at, EventKind::Usage(usage(t))));
                }
                let calls = v["toolCalls"].as_array().map(Vec::as_slice).unwrap_or_default();
                for c in calls {
                    self.call(c, &mut seen, out);
                }
                seen.answer = seen.text && calls.is_empty();
            }
            _ => {}
        }
        self.messages.insert(id.to_owned(), seen);
    }

    fn call(&self, c: &Value, seen: &mut Seen, out: &mut Vec<Event>) {
        let at = ts(&c["timestamp"]);
        let id = c.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
        let name = c.get("name").and_then(Value::as_str).unwrap_or("tool").to_owned();
        let status = c.get("status").and_then(Value::as_str).unwrap_or("");
        if seen.called.insert(id.clone()) {
            if status == "awaiting_approval" {
                out.push(self.ev(at, EventKind::NeedsInput { text: format!("Allow {name}?") }));
            }
            let mut summary = tool_summary(&c["args"]);
            if summary.is_empty() {
                summary = c.get("description").and_then(Value::as_str).map(|d| clip(d, 90)).unwrap_or_default();
            }
            out.push(self.ev(at, EventKind::ToolCall { call_id: id.clone(), name, summary }));
        }
        let done = matches!(status, "success" | "error" | "cancelled") || c.get("result").is_some_and(|r| !r.is_null());
        if done && seen.finished.insert(id.clone()) {
            out.push(self.ev(at, EventKind::ToolResult { call_id: id, ok: status != "error" && status != "cancelled" }));
            // a subagent's call is recorded once it is over
            if let Some(agent) = c.get("agentId").and_then(Value::as_str) {
                out.push(self.ev(at, EventKind::SubagentEnd { agent_id: agent.to_owned() }));
            }
        }
    }

    /// `$patch`: late results, rewritten content, and messages dropped by id.
    fn patch(&mut self, p: &Value, out: &mut Vec<Event>) {
        for gone in p["removeIds"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            self.messages.remove(gone);
            self.order.retain(|m| m != gone);
        }
        let Some(id) = p.get("id").and_then(Value::as_str) else { return };
        let Some(mut seen) = self.messages.remove(id) else { return };
        for c in p["toolCalls"].as_array().into_iter().flatten() {
            self.call(c, &mut seen, out);
        }
        self.messages.insert(id.to_owned(), seen);
    }

    /// No marker on disk: a final answer with nothing after it for [`QUIET_END`] ends the turn.
    fn quiet_end(&mut self, out: &mut Vec<Event>) {
        if self.ended || self.last_line.is_none_or(|t| t.elapsed() < QUIET_END) {
            return;
        }
        self.ended = true;
        let last = self.order.last().and_then(|id| self.messages.get(id));
        if last.is_some_and(|s| s.answer && s.finished.len() == s.called.len()) {
            out.push(match &self.agent {
                AgentRef::Lead => self.ev(SystemTime::now(), EventKind::TurnEnd),
                AgentRef::Sub(id) => self.ev(SystemTime::now(), EventKind::SubagentEnd { agent_id: id.clone() }),
            });
        }
    }
}

/// `content` is a string, or an array of parts after a `$patch`.
fn text_of(c: &Value) -> String {
    if let Some(s) = c.as_str() {
        return s.to_owned();
    }
    c.as_array().into_iter().flatten().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")
}

/// Gemini counts cached tokens inside `input` and thoughts outside `output`.
fn usage(t: &Value) -> Usage {
    let n = |k: &str| t.get(k).and_then(Value::as_u64).unwrap_or(0);
    Usage { input: n("input").saturating_sub(n("cached")), output: n("output") + n("thoughts"), cache_read: n("cached"), cache_write: 0 }
}

impl SourceReader for Reader {
    fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let lines = match &mut self.source {
            Source::Log(t) => t.read(),
            Source::Whole(f) => match f.read() {
                // the whole session again; the per-id diff keeps only what is new
                Some(mut v) => {
                    let msgs = match v.get_mut("messages").map(Value::take) {
                        Some(Value::Array(a)) => a,
                        _ => Vec::new(),
                    };
                    std::iter::once(v).chain(msgs).collect()
                }
                None => Vec::new(),
            },
        };
        if !self.started && !lines.is_empty() {
            if let AgentRef::Sub(id) = &self.agent {
                out.push(self.ev(SystemTime::now(), EventKind::SubagentStart { agent_id: id.clone(), description: "Subagent".into(), kind: None }));
            }
        }
        if !lines.is_empty() {
            self.last_line = Some(Instant::now());
            self.ended = false;
        }
        for v in &lines {
            self.line(v, &mut out);
            self.started = true;
        }
        self.quiet_end(&mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shipoffice-gemini-{name}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("tmp/proj/chats/3f9c1e2a-full")).unwrap();
        std::fs::write(dir.join("tmp/proj/.project_root"), "/home/user/proj\n").unwrap();
        dir
    }

    const LOG: [&str; 7] = [
        r#"{"sessionId":"3f9c1e2a-full","projectHash":"abc","startTime":"2026-10-18T09:12:03.120Z","lastUpdated":"2026-10-18T09:12:03.120Z"}"#,
        r#"{"id":"a1","timestamp":"2026-10-18T09:12:10.004Z","type":"user","content":[{"text":"Read the config"}]}"#,
        r#"{"$set":{"lastUpdated":"2026-10-18T09:12:10.005Z"}}"#,
        r#"{"id":"b2","timestamp":"2026-10-18T09:12:11Z","type":"gemini","content":"I'll read the config first.","thoughts":[{"subject":"Inspecting config","description":"x","timestamp":"2026-10-18T09:12:11Z"}],"tokens":{"input":8123,"output":64,"cached":4096,"thoughts":120,"tool":0,"total":8307},"model":"gemini-pro"}"#,
        r#"{"id":"b2","timestamp":"2026-10-18T09:12:11Z","type":"gemini","content":"I'll read the config first.","thoughts":[{"subject":"Inspecting config","description":"x","timestamp":"2026-10-18T09:12:11Z"}],"tokens":{"input":8123,"output":64,"cached":4096,"thoughts":120,"tool":0,"total":8307},"model":"gemini-pro","toolCalls":[{"id":"read_file_1","name":"read_file","args":{"absolute_path":"/home/user/proj/config.ts"},"result":[{"functionResponse":{"id":"read_file_1","name":"read_file","response":{"output":"x"}}}],"status":"success","timestamp":"2026-10-18T09:12:12Z","displayName":"ReadFile"}]}"#,
        r#"{"id":"c3","timestamp":"2026-10-18T09:12:13Z","type":"user","content":[{"functionResponse":{"id":"read_file_1","name":"read_file","response":{"output":"x"}}}]}"#,
        r#"{"id":"d4","timestamp":"2026-10-18T09:12:14Z","type":"gemini","content":"Done.","tokens":{"input":9000,"output":10,"cached":8000}}"#,
    ];

    #[test]
    fn log_upserts_by_id() {
        let dir = temp("log");
        let p = dir.join("tmp/proj/chats/session-2026-10-18T09-12-3f9c1e2a.jsonl");
        std::fs::write(&p, LOG.join("\n") + "\n").unwrap();
        std::fs::write(dir.join("tmp/proj/chats/3f9c1e2a-full/agent-x.jsonl"), r#"{"sessionId":"agent-x","kind":"subagent"}"#.to_string() + "\n").unwrap();

        let g = GeminiCli::default();
        let specs = g.discover(&[dir.clone()], SystemTime::UNIX_EPOCH);
        assert_eq!(specs.len(), 2);
        assert!(specs.iter().all(|s| s.key.id == "3f9c1e2a-full"));
        assert!(specs.iter().any(|s| s.agent == AgentRef::Sub("agent-x".into())));

        let lead = specs.iter().find(|s| s.agent == AgentRef::Lead).unwrap();
        let mut r = g.open(lead);
        let k: Vec<_> = r.poll().into_iter().map(|e| e.kind).collect();
        assert!(matches!(&k[0], EventKind::Meta { cwd: Some(c), .. } if c == Path::new("/home/user/proj")));
        assert_eq!(k.iter().filter(|e| matches!(e, EventKind::UserPrompt { .. })).count(), 1);
        assert_eq!(k.iter().filter(|e| matches!(e, EventKind::AssistantText { .. })).count(), 2);
        assert_eq!(k.iter().filter(|e| matches!(e, EventKind::Usage(_))).count(), 2);
        assert!(k.iter().any(|e| matches!(e, EventKind::Usage(u) if u.input == 4027 && u.cache_read == 4096 && u.output == 184)));
        assert!(k.iter().any(|e| matches!(e, EventKind::Thinking { text } if text == "Inspecting config")));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolCall { name, summary, .. } if name == "read_file" && summary == "/home/user/proj/config.ts")));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolResult { ok: true, .. })));
        assert!(!k.iter().any(|e| matches!(e, EventKind::TurnEnd)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn legacy_whole_file_diffs_by_id() {
        let dir = temp("json");
        let p = dir.join("tmp/proj/chats/session-2026-10-18T09-12-3f9c1e2a.json");
        let mut v = serde_json::json!({"sessionId":"3f9c1e2a-full","projectHash":"abc","messages":[
            serde_json::from_str::<Value>(LOG[1]).unwrap(),
        ]});
        std::fs::write(&p, v.to_string()).unwrap();
        let g = GeminiCli::default();
        let specs = g.discover(&[dir.clone()], SystemTime::UNIX_EPOCH);
        assert_eq!(specs[0].key.id, "3f9c1e2a-full");
        let mut r = g.open(&specs[0]);
        assert!(r.poll().iter().any(|e| matches!(e.kind, EventKind::UserPrompt { .. })));
        v["messages"].as_array_mut().unwrap().push(serde_json::from_str(LOG[6]).unwrap());
        std::fs::write(&p, v.to_string()).unwrap();
        let k: Vec<_> = r.poll().into_iter().map(|e| e.kind).collect();
        assert!(!k.iter().any(|e| matches!(e, EventKind::UserPrompt { .. })));
        assert!(k.iter().any(|e| matches!(e, EventKind::AssistantText { text } if text == "Done.")));
        std::fs::remove_dir_all(&dir).ok();
    }
}

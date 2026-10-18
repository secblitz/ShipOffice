//! Qwen Code: `~/.qwen/projects/<encoded-cwd>/chats/<session>.jsonl`, an append-only log much like
//! Claude Code's but with Gemini-style `parts`. Subagents get
//! `projects/<encoded-cwd>/subagents/<session>/agent-<id>.jsonl` and a `.meta.json` whose
//! `status` says when they are done. See docs/harnesses.md §5.

use super::{clip, env_path, home, modified, tool_summary, ts, Adapter, JsonFile, JsonlTail, SourceReader, SourceSpec};
use crate::model::{AgentRef, Event, EventKind, SessionKey, Usage};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct QwenCode;

const ID: &str = "qwen-code";

impl Adapter for QwenCode {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        "Qwen Code"
    }
    fn default_roots(&self) -> Vec<PathBuf> {
        let global = env_path("QWEN_HOME").unwrap_or_else(|| home().join(".qwen"));
        let mut roots = Vec::new();
        if let Some(rt) = env_path("QWEN_RUNTIME_DIR") {
            roots.push(rt);
        }
        roots.push(global);
        roots
    }

    fn discover(&self, roots: &[PathBuf], since: SystemTime) -> Vec<SourceSpec> {
        let mut out = Vec::new();
        for root in roots {
            let Ok(projects) = std::fs::read_dir(root.join("projects")) else { continue };
            for project in projects.flatten() {
                let dir = project.path();
                for (p, m) in files(&dir.join("chats"), since) {
                    let Some(stem) = p.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else { continue };
                    out.push(spec(&stem, AgentRef::Lead, p, m));
                }
                let Ok(sessions) = std::fs::read_dir(dir.join("subagents")) else { continue };
                for s in sessions.flatten() {
                    let sid = s.file_name().to_string_lossy().into_owned();
                    for (p, m) in files(&s.path(), since) {
                        let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                        if let Some(agent) = name.strip_prefix("agent-") {
                            out.push(spec(&sid, AgentRef::Sub(agent.to_owned()), p.clone(), m));
                        }
                    }
                }
            }
        }
        out
    }

    fn open(&self, spec: &SourceSpec) -> Box<dyn SourceReader> {
        let meta = match spec.agent {
            AgentRef::Sub(_) => Some(JsonFile::new(spec.path.with_extension("meta.json"))),
            AgentRef::Lead => None,
        };
        Box::new(Reader {
            key: spec.key.clone(),
            agent: spec.agent.clone(),
            tail: JsonlTail::new(spec.path.clone()),
            meta,
            started: false,
            model: None,
            answered: false,
            request_open: false,
            ended: true,
            info: Value::Null,
            announced: false,
            sub_done: false,
        })
    }
}

fn spec(session: &str, agent: AgentRef, path: PathBuf, modified: SystemTime) -> SourceSpec {
    SourceSpec { key: SessionKey { harness: ID, id: session.to_owned() }, agent, path, modified }
}

/// `*.jsonl` in one folder changed after `since` (not the `.jsonl.stream` scratch files).
fn files(dir: &Path, since: SystemTime) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|p| modified(&p).filter(|m| *m >= since).map(|m| (p, m)))
        .collect()
}

struct Reader {
    key: SessionKey,
    agent: AgentRef,
    tail: JsonlTail,
    meta: Option<JsonFile>,
    started: bool,
    model: Option<String>,
    /// the newest assistant message had no function call
    answered: bool,
    /// a model request is in flight (`request_lifecycle` started, not ended)
    request_open: bool,
    ended: bool,
    /// the subagent's latest meta.json
    info: Value,
    announced: bool,
    sub_done: bool,
}

impl Reader {
    fn ev(&self, at: SystemTime, kind: EventKind) -> Event {
        Event { session: self.key.clone(), agent: self.agent.clone(), at, kind }
    }

    fn line(&mut self, v: &Value, out: &mut Vec<Event>) {
        let at = ts(&v["timestamp"]);
        let lead = matches!(self.agent, AgentRef::Lead);
        if !self.started && lead {
            let cwd = v.get("cwd").and_then(Value::as_str).map(PathBuf::from);
            let branch = v.get("gitBranch").and_then(Value::as_str).map(str::to_owned);
            out.push(self.ev(at, EventKind::Meta { cwd, branch, model: None, title: None }));
        }
        let parts = v.pointer("/message/parts").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "user" => {
                // notifications, cron runs, agent messages... carry a subtype or another provenance
                let human = v.get("subtype").is_none() && v.get("provenance").and_then(Value::as_str).is_none_or(|p| p == "real_user");
                let text: Vec<&str> = parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect();
                if lead && human && !text.is_empty() {
                    self.answered = false;
                    self.ended = false;
                    out.push(self.ev(at, EventKind::UserPrompt { text: clip(&text.join("\n"), 140) }));
                }
            }
            "assistant" => self.assistant(v, parts, at, out),
            "tool_result" => {
                let r = &v["toolCallResult"];
                let call_id = r
                    .get("callId")
                    .and_then(Value::as_str)
                    .or_else(|| parts.iter().find_map(|p| p.pointer("/functionResponse/id").and_then(Value::as_str)))
                    .unwrap_or("")
                    .to_owned();
                let ok = r.get("status").and_then(Value::as_str).is_none_or(|s| s == "success") && r.get("error").is_none_or(Value::is_null);
                out.push(self.ev(at, EventKind::ToolResult { call_id, ok }));
            }
            "system" => {
                let p = &v["systemPayload"];
                match v.get("subtype").and_then(Value::as_str).unwrap_or("") {
                    "ui_telemetry" if p.pointer("/uiEvent/event.name").and_then(Value::as_str) == Some("request_lifecycle") => {
                        self.request_open = p.pointer("/uiEvent/phase").and_then(Value::as_str) == Some("started");
                    }
                    // only ACP / `qwen serve` sessions write this
                    "turn_result" => {
                        self.answered = true;
                        self.request_open = false;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        // the TUI writes no turn marker: a request that ended on an answer with no tool call
        if lead && self.answered && !self.request_open && !self.ended {
            self.ended = true;
            out.push(self.ev(at, EventKind::TurnEnd));
        }
    }

    fn assistant(&mut self, v: &Value, parts: &[Value], at: SystemTime, out: &mut Vec<Event>) {
        if let Some(m) = v.get("model").and_then(Value::as_str).filter(|m| self.model.as_deref() != Some(*m)) {
            self.model = Some(m.to_owned());
            if matches!(self.agent, AgentRef::Lead) {
                out.push(self.ev(at, EventKind::Meta { cwd: None, branch: None, model: Some(m.to_owned()), title: None }));
            }
        }
        if let Some(u) = v.get("usageMetadata") {
            out.push(self.ev(at, EventKind::Usage(usage(u))));
        }
        let mut called = false;
        for p in parts {
            if let Some(c) = p.get("functionCall") {
                called = true;
                let call_id = c.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
                let name = c.get("name").and_then(Value::as_str).unwrap_or("tool").to_owned();
                out.push(self.ev(at, EventKind::ToolCall { call_id, name, summary: tool_summary(&c["args"]) }));
            } else if let Some(t) = p.get("text").and_then(Value::as_str) {
                if p.get("thought").and_then(Value::as_bool).unwrap_or(false) {
                    out.push(self.ev(at, EventKind::Thinking { text: clip(t, 140) }));
                } else if !t.trim().is_empty() {
                    out.push(self.ev(at, EventKind::AssistantText { text: clip(t, 160) }));
                }
            }
        }
        self.answered = !called;
        if called {
            self.ended = false;
        }
    }

    /// Subagents announce themselves with what their `meta.json` says, and are done when its
    /// `status` turns terminal.
    fn sub_meta(&mut self, has_lines: bool, out: &mut Vec<Event>) {
        let AgentRef::Sub(id) = self.agent.clone() else { return };
        if let Some(m) = self.meta.as_mut().and_then(JsonFile::read) {
            self.info = m;
        }
        if !self.announced && (has_lines || !self.info.is_null()) {
            self.announced = true;
            let description = self.info.get("description").and_then(Value::as_str).map(|d| clip(d, 60)).unwrap_or_else(|| "Subagent".into());
            let kind = self.info.get("agentType").and_then(Value::as_str).map(str::to_owned);
            out.push(self.ev(SystemTime::now(), EventKind::SubagentStart { agent_id: id.clone(), description, kind }));
        }
    }

    fn sub_finished(&mut self, out: &mut Vec<Event>) {
        let AgentRef::Sub(id) = self.agent.clone() else { return };
        let status = self.info.get("status").and_then(Value::as_str).unwrap_or("");
        if self.announced && !self.sub_done && matches!(status, "completed" | "failed" | "cancelled") {
            self.sub_done = true;
            out.push(self.ev(SystemTime::now(), EventKind::SubagentEnd { agent_id: id }));
        }
    }
}

/// Qwen reports Gemini-style usage: cached tokens inside the prompt count, thoughts outside the output.
fn usage(u: &Value) -> Usage {
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let cached = n("cachedContentTokenCount");
    Usage {
        input: n("promptTokenCount").saturating_sub(cached),
        output: n("candidatesTokenCount") + n("thoughtsTokenCount"),
        cache_read: cached,
        cache_write: 0,
    }
}

impl SourceReader for Reader {
    fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let lines = self.tail.read();
        self.sub_meta(!lines.is_empty(), &mut out);
        for v in &lines {
            self.line(v, &mut out);
            self.started = true;
        }
        self.sub_finished(&mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_turn_with_lifecycle_and_subagent_meta() {
        let dir = std::env::temp_dir().join(format!("shipoffice-qwen-{}", std::process::id()));
        let proj = dir.join("projects/-home-user-proj");
        std::fs::create_dir_all(proj.join("chats")).unwrap();
        std::fs::create_dir_all(proj.join("subagents/9d1e")).unwrap();
        std::fs::write(proj.join("chats/9d1e.runtime.json"), "{}").unwrap();
        std::fs::write(proj.join("chats/9d1e.jsonl"), [
            r#"{"uuid":"u1","sessionId":"9d1e","timestamp":"2026-10-18T10:00:00.000Z","type":"user","provenance":"real_user","cwd":"/home/user/proj","gitBranch":"main","message":{"role":"user","parts":[{"text":"Check the config"}]}}"#,
            r#"{"uuid":"n1","type":"user","subtype":"notification","message":{"role":"user","parts":[{"text":"background job done"}]}}"#,
            r#"{"uuid":"s0","type":"system","subtype":"ui_telemetry","systemPayload":{"uiEvent":{"event.name":"request_lifecycle","phase":"started"}}}"#,
            r#"{"uuid":"a1","type":"assistant","model":"qwen3-coder","message":{"role":"model","parts":[{"text":"hmm","thought":true},{"text":"Let me check the config."},{"functionCall":{"id":"call_abc123","name":"read_file","args":{"absolute_path":"/home/user/proj/config.ts"}}}]},"usageMetadata":{"promptTokenCount":9012,"candidatesTokenCount":88,"cachedContentTokenCount":4096,"thoughtsTokenCount":40,"totalTokenCount":9140}}"#,
            r#"{"uuid":"t1","type":"tool_result","message":{"role":"user","parts":[{"functionResponse":{"id":"call_abc123","name":"read_file","response":{"output":"x"}}}]},"toolCallResult":{"callId":"call_abc123","status":"success"}}"#,
            r#"{"uuid":"a2","type":"assistant","model":"qwen3-coder","message":{"role":"model","parts":[{"text":"All good."}]}}"#,
            r#"{"uuid":"s1","type":"system","subtype":"ui_telemetry","systemPayload":{"uiEvent":{"event.name":"request_lifecycle","phase":"ended"}}}"#,
        ].join("\n") + "\n").unwrap();
        std::fs::write(proj.join("subagents/9d1e/agent-a7.jsonl"), r#"{"type":"user","isSidechain":true,"agentId":"a7","message":{"role":"user","parts":[{"text":"task"}]}}"#.to_string() + "\n").unwrap();
        std::fs::write(proj.join("subagents/9d1e/agent-a7.meta.json"), r#"{"agentId":"a7","agentType":"explore","description":"Map the repo","status":"running"}"#).unwrap();

        let specs = QwenCode.discover(&[dir.clone()], SystemTime::UNIX_EPOCH);
        assert_eq!(specs.len(), 2, "the runtime sidecar is not a transcript");
        let lead = specs.iter().find(|s| s.agent == AgentRef::Lead).unwrap();
        let k: Vec<_> = QwenCode.open(lead).poll().into_iter().map(|e| e.kind).collect();
        assert!(matches!(&k[0], EventKind::Meta { branch: Some(b), .. } if b == "main"));
        assert_eq!(k.iter().filter(|e| matches!(e, EventKind::UserPrompt { .. })).count(), 1);
        assert!(k.iter().any(|e| matches!(e, EventKind::Thinking { text } if text == "hmm")));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolCall { summary, .. } if summary == "/home/user/proj/config.ts")));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolResult { call_id, ok: true } if call_id == "call_abc123")));
        assert!(k.iter().any(|e| matches!(e, EventKind::Usage(u) if u.input == 4916 && u.cache_read == 4096 && u.output == 128)));
        assert_eq!(k.iter().filter(|e| matches!(e, EventKind::TurnEnd)).count(), 1);
        assert!(matches!(k.last(), Some(EventKind::TurnEnd)));

        let sub = specs.iter().find(|s| s.agent != AgentRef::Lead).unwrap();
        assert_eq!(sub.key.id, "9d1e");
        let mut r = QwenCode.open(sub);
        let k: Vec<_> = r.poll().into_iter().map(|e| e.kind).collect();
        assert!(matches!(&k[0], EventKind::SubagentStart { description, kind: Some(t), .. } if description == "Map the repo" && t == "explore"));
        assert!(!k.iter().any(|e| matches!(e, EventKind::UserPrompt { .. } | EventKind::SubagentEnd { .. })));
        std::fs::write(proj.join("subagents/9d1e/agent-a7.meta.json"), r#"{"agentId":"a7","agentType":"explore","description":"Map the repo","status":"completed"}"#).unwrap();
        let k: Vec<_> = r.poll().into_iter().map(|e| e.kind).collect();
        assert!(matches!(&k[..], [EventKind::SubagentEnd { agent_id }] if agent_id == "a7"));
        std::fs::remove_dir_all(&dir).ok();
    }
}

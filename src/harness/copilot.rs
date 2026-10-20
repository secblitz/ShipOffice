//! GitHub Copilot CLI: `~/.copilot/session-state/<session>/events.jsonl`, one event per line,
//! plus a small `workspace.yaml`. Idle and per-request usage events are never written to disk.
//! Subagent events carry an `agentId`. See docs/harnesses.md §6.
//!
//! `config.json` beside the sessions can hold plaintext tokens and is never opened.

use super::{clip, env_path, home, modified, read_capped, tool_summary, ts, Adapter, JsonlTail, SourceReader, SourceSpec};
use crate::model::{AgentRef, Event, EventKind, SessionKey, Usage};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct CopilotCli;

const ID: &str = "copilot-cli";

impl Adapter for CopilotCli {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        "Copilot CLI"
    }
    fn default_roots(&self) -> Vec<PathBuf> {
        vec![env_path("COPILOT_HOME").unwrap_or_else(|| home().join(".copilot"))]
    }

    fn discover(&self, roots: &[PathBuf], since: SystemTime) -> Vec<SourceSpec> {
        let mut out = Vec::new();
        for root in roots {
            let Ok(sessions) = std::fs::read_dir(root.join("session-state")) else { continue };
            for s in sessions.flatten() {
                let p = s.path().join("events.jsonl");
                if let Some(m) = modified(&p).filter(|m| *m >= since) {
                    let id = s.file_name().to_string_lossy().into_owned();
                    out.push(SourceSpec { key: SessionKey { harness: ID, id }, agent: AgentRef::Lead, path: p, modified: m });
                }
            }
        }
        out
    }

    fn open(&self, spec: &SourceSpec) -> Box<dyn SourceReader> {
        Box::new(Reader {
            key: spec.key.clone(),
            workspace: spec.path.with_file_name("workspace.yaml"),
            tail: JsonlTail::new(spec.path.clone()),
            started: false,
            called: HashSet::new(),
            agents: HashMap::new(),
            unbound: Vec::new(),
            asked_tools: true,
        })
    }
}

struct Reader {
    key: SessionKey,
    workspace: PathBuf,
    tail: JsonlTail,
    started: bool,
    called: HashSet<String>,
    /// event `agentId` -> the spawning tool call id that `subagent.started` announced
    agents: HashMap<String, String>,
    /// announced subagents whose events haven't shown up yet
    unbound: Vec<String>,
    /// the lead's last message asked for tools, so a `turn_end` is not the end of the turn
    asked_tools: bool,
}

impl Reader {
    fn ev(&self, agent: &AgentRef, at: SystemTime, kind: EventKind) -> Event {
        Event { session: self.key.clone(), agent: agent.clone(), at, kind }
    }

    /// Child events name their agent by an id the start event doesn't carry; tie each new id to
    /// the oldest announced subagent still unclaimed.
    fn agent_of(&mut self, v: &Value) -> AgentRef {
        let Some(id) = v.get("agentId").and_then(Value::as_str).filter(|s| !s.is_empty()) else { return AgentRef::Lead };
        if let Some(call) = self.agents.get(id) {
            return AgentRef::Sub(call.clone());
        }
        let call = if self.unbound.is_empty() { id.to_owned() } else { self.unbound.remove(0) };
        self.agents.insert(id.to_owned(), call.clone());
        AgentRef::Sub(call)
    }

    fn line(&mut self, v: &Value, out: &mut Vec<Event>) {
        let at = ts(&v["timestamp"]);
        let d = &v["data"];
        let s = |k: &str| d.get(k).and_then(Value::as_str);
        let agent = self.agent_of(v);
        let lead = agent == AgentRef::Lead;
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "session.start" | "session.context_changed" => {
                let c = d.get("context").unwrap_or(d);
                let cwd = c.get("cwd").and_then(Value::as_str).map(PathBuf::from);
                let branch = c.get("branch").and_then(Value::as_str).map(str::to_owned);
                let model = s("selectedModel").map(str::to_owned);
                out.push(self.ev(&agent, at, EventKind::Meta { cwd, branch, model, title: None }));
            }
            "session.model_change" | "assistant.turn_start" if lead => {
                if let Some(m) = s("newModel").or(s("model")) {
                    out.push(self.ev(&agent, at, EventKind::Meta { cwd: None, branch: None, model: Some(m.to_owned()), title: None }));
                }
            }
            "user.message" if lead => {
                // `source` marks messages the CLI injected rather than typed ones
                let injected = s("source").is_some_and(|src| ["system", "reminder", "skill", "hook", "agent", "notification"].iter().any(|k| src.contains(k)));
                let t = s("content").unwrap_or("");
                if !injected && !t.trim().is_empty() {
                    out.push(self.ev(&agent, at, EventKind::UserPrompt { text: clip(t, 140) }));
                }
            }
            "assistant.reasoning" => {
                out.push(self.ev(&agent, at, EventKind::Thinking { text: clip(s("content").unwrap_or(""), 140) }));
            }
            "assistant.message" => {
                if let Some(r) = s("reasoningText").filter(|r| !r.trim().is_empty()) {
                    out.push(self.ev(&agent, at, EventKind::Thinking { text: clip(r, 140) }));
                }
                if let Some(t) = s("content").filter(|t| !t.trim().is_empty()) {
                    out.push(self.ev(&agent, at, EventKind::AssistantText { text: clip(t, 160) }));
                }
                let requests = d["toolRequests"].as_array().map(Vec::as_slice).unwrap_or_default();
                for r in requests {
                    let call_id = r.get("toolCallId").and_then(Value::as_str).unwrap_or("");
                    let name = r.get("name").and_then(Value::as_str).unwrap_or("tool");
                    self.call(&agent, at, call_id, name, &r["arguments"], out);
                }
                if lead {
                    self.asked_tools = !requests.is_empty();
                }
                // per-request usage is ephemeral; output tokens are all that is written
                if let Some(n) = d.get("outputTokens").and_then(Value::as_u64).filter(|n| *n > 0) {
                    out.push(self.ev(&agent, at, EventKind::Usage(Usage { output: n, ..Usage::default() })));
                }
            }
            "tool.execution_start" => {
                let name = s("toolName").unwrap_or("tool").to_owned();
                self.call(&agent, at, s("toolCallId").unwrap_or(""), &name, &d["arguments"], out);
            }
            "tool.execution_complete" => {
                let call_id = s("toolCallId").unwrap_or("").to_owned();
                let ok = d.get("success").and_then(Value::as_bool).unwrap_or(true);
                out.push(self.ev(&agent, at, EventKind::ToolResult { call_id, ok }));
            }
            "subagent.started" => {
                if let Some(id) = s("toolCallId") {
                    self.unbound.push(id.to_owned());
                    let description = s("agentDisplayName").or(s("agentName")).unwrap_or("Subagent").to_owned();
                    let kind = s("agentType").or(s("agentName")).map(str::to_owned);
                    out.push(self.ev(&AgentRef::Lead, at, EventKind::SubagentStart { agent_id: id.to_owned(), description, kind }));
                }
            }
            "subagent.completed" | "subagent.failed" => {
                if let Some(id) = s("toolCallId") {
                    self.unbound.retain(|u| u != id);
                    out.push(self.ev(&AgentRef::Lead, at, EventKind::SubagentEnd { agent_id: id.to_owned() }));
                }
            }
            "permission.requested" | "user_input.requested" => {
                let text = s("message").or(s("question")).or(s("toolName")).map(|t| clip(t, 80)).unwrap_or_else(|| "Waiting for your answer".into());
                out.push(self.ev(&agent, at, EventKind::NeedsInput { text }));
            }
            // one agent-loop iteration ends; the turn is over when its message asked for no tools
            "assistant.turn_end" if lead && !self.asked_tools => {
                self.asked_tools = true;
                out.push(self.ev(&agent, at, EventKind::TurnEnd));
            }
            "session.task_complete" | "abort" if lead => {
                self.asked_tools = true;
                out.push(self.ev(&agent, at, EventKind::TurnEnd));
            }
            _ => {}
        }
    }

    fn call(&mut self, agent: &AgentRef, at: SystemTime, call_id: &str, name: &str, args: &Value, out: &mut Vec<Event>) {
        // a call shows up in the message's toolRequests and again when it starts
        if self.called.insert(call_id.to_owned()) {
            out.push(self.ev(agent, at, EventKind::ToolCall { call_id: call_id.to_owned(), name: name.to_owned(), summary: tool_summary(args) }));
        }
    }
}

/// The top-level `key: value` pairs of `workspace.yaml`, which is all this needs from it.
pub fn yaml_fields(path: &Path) -> HashMap<String, String> {
    let Some(text) = read_capped(path, 1024 * 1024).and_then(|b| String::from_utf8(b).ok()) else { return HashMap::new() };
    text.lines()
        .filter(|l| !l.starts_with([' ', '\t', '#', '-']))
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().trim_matches(['"', '\'']).to_owned()))
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

impl SourceReader for Reader {
    fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let lines = self.tail.read();
        if !self.started && !lines.is_empty() {
            let f = yaml_fields(&self.workspace);
            let title = f.get("name").or(f.get("summary")).map(|t| clip(t, 60));
            out.push(self.ev(
                &AgentRef::Lead,
                SystemTime::now(),
                EventKind::Meta { cwd: f.get("cwd").map(PathBuf::from), branch: f.get("branch").cloned(), model: None, title },
            ));
        }
        for v in &lines {
            self.line(v, &mut out);
            self.started = true;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_with_a_subagent() {
        let dir = std::env::temp_dir().join(format!("shipoffice-copilot-{}", std::process::id()));
        let s = dir.join("session-state/3f2a");
        std::fs::create_dir_all(&s).unwrap();
        std::fs::write(s.join("workspace.yaml"), "cwd: /home/user/proj\nsummary: Fix the failing test\nbranch: main\n").unwrap();
        std::fs::write(s.join("events.jsonl"), [
            r#"{"id":"e1","parentId":null,"timestamp":"2026-10-15T19:05:40.000Z","type":"session.start","data":{"sessionId":"3f2a","selectedModel":"gpt-x","context":{"cwd":"/home/user/proj","branch":"main"}}}"#,
            r#"{"id":"e2","parentId":"e1","timestamp":"2026-10-15T19:05:41Z","type":"user.message","data":{"content":"Fix the failing test","messageId":"m1"}}"#,
            r#"{"id":"e3","type":"assistant.turn_start","data":{"turnId":"0","model":"gpt-x"}}"#,
            r#"{"id":"e4","type":"assistant.message","data":{"messageId":"a1","content":"Running tests…","outputTokens":120,"toolRequests":[{"toolCallId":"call_1","name":"bash","arguments":{"command":"npm test"}}]}}"#,
            r#"{"id":"e5","type":"tool.execution_start","data":{"toolCallId":"call_1","toolName":"bash","arguments":{"command":"npm test"},"turnId":"0"}}"#,
            r#"{"id":"e6","type":"tool.execution_complete","data":{"toolCallId":"call_1","success":false}}"#,
            r#"{"id":"e7","type":"subagent.started","data":{"toolCallId":"call_2","agentName":"explore","agentDisplayName":"Explore","executionMode":"sync"}}"#,
            r#"{"id":"e7b","type":"tool.execution_start","agentId":"ag1","data":{"toolCallId":"call_3","toolName":"grep","arguments":{"pattern":"TODO"}}}"#,
            r#"{"id":"e8","type":"subagent.completed","data":{"toolCallId":"call_2","durationMs":8123}}"#,
            r#"{"id":"e9","type":"assistant.turn_end","data":{"turnId":"0"}}"#,
            r#"{"id":"e10","type":"assistant.message","data":{"messageId":"a2","content":"Fixed."}}"#,
            r#"{"id":"e11","type":"assistant.turn_end","data":{"turnId":"1"}}"#,
        ].join("\n") + "\n").unwrap();

        let specs = CopilotCli.discover(&[dir.clone()], SystemTime::UNIX_EPOCH);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].key.id, "3f2a");
        let ev = CopilotCli.open(&specs[0]).poll();
        let k: Vec<_> = ev.iter().map(|e| e.kind.clone()).collect();
        assert!(matches!(&k[0], EventKind::Meta { title: Some(t), cwd: Some(_), .. } if t == "Fix the failing test"));
        assert!(k.iter().any(|e| matches!(e, EventKind::UserPrompt { text } if text == "Fix the failing test")));
        assert_eq!(k.iter().filter(|e| matches!(e, EventKind::ToolCall { call_id, .. } if call_id == "call_1")).count(), 1);
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolResult { ok: false, .. })));
        assert!(k.iter().any(|e| matches!(e, EventKind::Usage(u) if u.output == 120)));
        assert!(ev.iter().any(|e| e.agent == AgentRef::Sub("call_2".into()) && matches!(&e.kind, EventKind::ToolCall { name, .. } if name == "grep")));
        assert!(k.iter().any(|e| matches!(e, EventKind::SubagentEnd { agent_id } if agent_id == "call_2")));
        assert_eq!(k.iter().filter(|e| matches!(e, EventKind::TurnEnd)).count(), 1);
        assert!(matches!(k.last(), Some(EventKind::TurnEnd)));
        std::fs::remove_dir_all(&dir).ok();
    }
}

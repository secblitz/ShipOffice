//! OpenAI Codex CLI: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`. Subagents get their own
//! rollout whose `session_meta` names the parent thread. See docs/harnesses.md §2.

use super::{clip, env_path, home, modified, tool_summary, ts, Adapter, JsonlTail, SourceReader, SourceSpec};
use crate::model::{AgentRef, Event, EventKind, SessionKey, Usage};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

const ID: &str = "codex";

#[derive(Default)]
pub struct Codex {
    /// rollout path -> (root thread id, Some(own id) when the rollout is a subagent's)
    heads: Mutex<HashMap<PathBuf, (String, Option<String>)>>,
}

impl Adapter for Codex {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        "Codex"
    }
    fn default_roots(&self) -> Vec<PathBuf> {
        vec![env_path("CODEX_HOME").unwrap_or_else(|| home().join(".codex"))]
    }

    fn discover(&self, roots: &[PathBuf], since: SystemTime) -> Vec<SourceSpec> {
        let mut files = Vec::new();
        for root in roots {
            walk(&root.join("sessions"), since, 0, &mut files);
        }
        let mut heads = self.heads.lock().unwrap();
        files
            .into_iter()
            .filter_map(|(path, m)| {
                if !heads.contains_key(&path) {
                    let head = read_head(&path)?;
                    heads.insert(path.clone(), head);
                }
                let (root, own) = heads.get(&path)?.clone();
                let agent = own.map_or(AgentRef::Lead, AgentRef::Sub);
                Some(SourceSpec { key: SessionKey { harness: ID, id: root }, agent, path, modified: m })
            })
            .collect()
    }

    fn open(&self, spec: &SourceSpec) -> Box<dyn SourceReader> {
        Box::new(Reader {
            key: spec.key.clone(),
            agent: spec.agent.clone(),
            tail: JsonlTail::new(spec.path.clone()),
            started: false,
            pending_input: HashSet::new(),
        })
    }
}

/// `sessions/` is split into year/month/day folders; only files touched after `since` matter.
fn walk(dir: &Path, since: SystemTime, depth: u8, out: &mut Vec<(PathBuf, SystemTime)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() && depth < 4 {
            walk(&p, since, depth + 1, out);
        } else if p.extension().is_some_and(|x| x == "jsonl")
            && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rollout-"))
        {
            if let Some(m) = modified(&p).filter(|m| *m >= since) {
                out.push((p, m));
            }
        }
    }
}

/// The first line says which thread this is and, for subagents, whose.
fn read_head(path: &Path) -> Option<(String, Option<String>)> {
    use std::io::BufRead;
    let f = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    std::io::BufReader::new(f).read_line(&mut line).ok()?;
    let v: Value = serde_json::from_str(&line).ok()?;
    let p = &v["payload"];
    let own = p.get("id").and_then(Value::as_str)?.to_owned();
    let parent = p
        .get("parent_thread_id")
        .or_else(|| p.pointer("/source/subagent/thread_spawn/parent_thread_id"))
        .and_then(Value::as_str);
    let root = p.get("session_id").and_then(Value::as_str).or(parent);
    match root {
        Some(r) if r != own => Some((r.to_owned(), Some(own))),
        _ => Some((own, None)),
    }
}

struct Reader {
    key: SessionKey,
    agent: AgentRef,
    tail: JsonlTail,
    started: bool,
    pending_input: HashSet<String>,
}

impl Reader {
    fn ev(&self, at: SystemTime, kind: EventKind) -> Event {
        Event { session: self.key.clone(), agent: self.agent.clone(), at, kind }
    }

    fn line(&mut self, v: &Value, out: &mut Vec<Event>) {
        let at = ts(&v["timestamp"]);
        let p = &v["payload"];
        let ptype = p.get("type").and_then(Value::as_str).unwrap_or("");
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "session_meta" => {
                if let AgentRef::Sub(id) = &self.agent {
                    let description = p
                        .pointer("/source/subagent/thread_spawn/agent_nickname")
                        .or_else(|| p.pointer("/source/subagent/thread_spawn/agent_path"))
                        .and_then(Value::as_str)
                        .unwrap_or("Subagent")
                        .to_owned();
                    out.push(self.ev(at, EventKind::SubagentStart { agent_id: id.clone(), description, kind: None }));
                }
                let cwd = p.get("cwd").and_then(Value::as_str).map(PathBuf::from);
                let branch = p.pointer("/git/branch").and_then(Value::as_str).map(str::to_owned);
                out.push(self.ev(at, EventKind::Meta { cwd, branch, model: None, title: None }));
            }
            "turn_context" => {
                let cwd = p.get("cwd").and_then(Value::as_str).map(PathBuf::from);
                let model = p.get("model").and_then(Value::as_str).map(str::to_owned);
                out.push(self.ev(at, EventKind::Meta { cwd, branch: None, model, title: None }));
            }
            "response_item" => self.item(p, ptype, at, out),
            "token_usage_record" => {
                if let Some(u) = p.get("usage") {
                    out.push(self.ev(at, EventKind::Usage(usage(u))));
                }
            }
            "event_msg" => match ptype {
                "task_complete" | "turn_complete" | "turn_aborted" => {
                    let kind = if matches!(self.agent, AgentRef::Sub(_)) {
                        let AgentRef::Sub(id) = &self.agent else { unreachable!() };
                        EventKind::SubagentEnd { agent_id: id.clone() }
                    } else {
                        EventKind::TurnEnd
                    };
                    out.push(self.ev(at, kind));
                }
                // pre-0.100 rollouts only carry these
                "user_message" if matches!(self.agent, AgentRef::Lead) => {
                    let t = p.get("message").and_then(Value::as_str).unwrap_or("");
                    out.push(self.ev(at, EventKind::UserPrompt { text: clip(t, 140) }));
                }
                "agent_message" => {
                    let t = p.get("message").and_then(Value::as_str).unwrap_or("");
                    out.push(self.ev(at, EventKind::AssistantText { text: clip(t, 160) }));
                }
                "agent_reasoning" => {
                    let t = p.get("text").and_then(Value::as_str).unwrap_or("");
                    out.push(self.ev(at, EventKind::Thinking { text: clip(t, 140) }));
                }
                "token_count" if !self.started => {
                    if let Some(u) = p.pointer("/info/last_token_usage") {
                        out.push(self.ev(at, EventKind::Usage(usage(u))));
                    }
                }
                "item_completed" => {
                    let item = &p["item"];
                    if item.get("type").and_then(Value::as_str) == Some("SubAgentActivity")
                        && item.get("kind").and_then(Value::as_str) == Some("completed")
                    {
                        if let Some(id) = item.get("agent_thread_id").and_then(Value::as_str) {
                            out.push(self.ev(at, EventKind::SubagentEnd { agent_id: id.to_owned() }));
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn item(&mut self, p: &Value, ptype: &str, at: SystemTime, out: &mut Vec<Event>) {
        match ptype {
            "message" => {
                let role = p.get("role").and_then(Value::as_str).unwrap_or("");
                let text: String = p["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n");
                if role == "user" && matches!(self.agent, AgentRef::Lead) && !text.trim_start().starts_with('<') && !text.starts_with("#") {
                    out.push(self.ev(at, EventKind::UserPrompt { text: clip(&text, 140) }));
                } else if role == "assistant" && !text.trim().is_empty() {
                    out.push(self.ev(at, EventKind::AssistantText { text: clip(&text, 160) }));
                }
            }
            "reasoning" => {
                let t: String = p["summary"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s.get("text").and_then(Value::as_str))
                    .collect();
                out.push(self.ev(at, EventKind::Thinking { text: clip(&t, 140) }));
            }
            "function_call" | "custom_tool_call" | "local_shell_call" | "web_search_call" => {
                let call_id = p.get("call_id").and_then(Value::as_str).unwrap_or("").to_owned();
                let mut name = p.get("name").and_then(Value::as_str).unwrap_or(match ptype {
                    "local_shell_call" => "shell",
                    "web_search_call" => "web_search",
                    _ => "tool",
                }).to_owned();
                let args = p
                    .get("arguments")
                    .and_then(Value::as_str)
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .or_else(|| p.get("input").cloned())
                    .or_else(|| p.pointer("/action/command").cloned())
                    .or_else(|| p.pointer("/action/query").cloned())
                    .unwrap_or(Value::Null);
                let mut summary = tool_summary(&args);
                if name == "exec" {
                    // the code-mode tool wraps a shell command in a small script
                    name = "shell".into();
                    if let Some(cmd) = summary.split("cmd:").nth(1) {
                        summary = clip(cmd.trim_start_matches(['"', '\\']), 90);
                    }
                }
                if name == "spawn_agent" {
                    let task = args.get("task_name").and_then(Value::as_str).unwrap_or("helper");
                    summary = format!("Spawn {task}");
                }
                if name.starts_with("request_user_input") {
                    self.pending_input.insert(call_id.clone());
                    out.push(self.ev(at, EventKind::NeedsInput { text: "Waiting for your answer".into() }));
                }
                out.push(self.ev(at, EventKind::ToolCall { call_id, name, summary }));
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = p.get("call_id").and_then(Value::as_str).unwrap_or("").to_owned();
                self.pending_input.remove(&call_id);
                out.push(self.ev(at, EventKind::ToolResult { call_id, ok: true }));
            }
            _ => {}
        }
    }
}

/// Codex counts cached tokens inside `input_tokens`; split them out like the other harnesses.
fn usage(u: &Value) -> Usage {
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let cached = n("cached_input_tokens");
    Usage {
        input: n("input_tokens").saturating_sub(cached),
        output: n("output_tokens") + n("reasoning_output_tokens"),
        cache_read: cached,
        cache_write: n("cache_write_input_tokens"),
    }
}

impl SourceReader for Reader {
    fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        for v in self.tail.read() {
            self.line(&v, &mut out);
            self.started = true;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_turn_and_subagent_head() {
        let dir = std::env::temp_dir().join(format!("shipoffice-codex-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lead = dir.join("rollout-a.jsonl");
        std::fs::write(&lead, [
            r#"{"timestamp":"2026-10-09T14:01:21.664Z","type":"session_meta","payload":{"id":"T1","cwd":"/p","git":{"branch":"main"}}}"#,
            r#"{"timestamp":"2026-10-09T14:01:22Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Install it"}]}}"#,
            r#"{"timestamp":"2026-10-09T14:01:23Z","type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"c1","arguments":"{\"command\":[\"bash\",\"-lc\",\"make\"]}"}}"#,
            r#"{"timestamp":"2026-10-09T14:01:24Z","type":"token_usage_record","payload":{"usage":{"input_tokens":100,"cached_input_tokens":60,"output_tokens":5}}}"#,
            r#"{"timestamp":"2026-10-09T14:01:25Z","type":"event_msg","payload":{"type":"task_complete"}}"#,
        ].join("\n") + "\n").unwrap();
        let sub = dir.join("rollout-b.jsonl");
        std::fs::write(&sub, r#"{"type":"session_meta","payload":{"id":"T2","session_id":"T1","source":{"subagent":{"thread_spawn":{"parent_thread_id":"T1","agent_nickname":"Ramanujan"}}}}}"#.to_string() + "\n").unwrap();

        let c = Codex::default();
        let mut specs = c.discover(&[], SystemTime::UNIX_EPOCH);
        assert!(specs.is_empty());
        walk(&dir, SystemTime::UNIX_EPOCH, 0, &mut Vec::new());
        assert_eq!(read_head(&sub), Some(("T1".into(), Some("T2".into()))));
        assert_eq!(read_head(&lead), Some(("T1".into(), None)));

        specs.push(SourceSpec { key: SessionKey { harness: ID, id: "T1".into() }, agent: AgentRef::Lead, path: lead, modified: SystemTime::now() });
        let k: Vec<_> = c.open(&specs[0]).poll().into_iter().map(|e| e.kind).collect();
        assert!(k.iter().any(|e| matches!(e, EventKind::UserPrompt { text } if text == "Install it")));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolCall { summary, .. } if summary == "bash -lc make")));
        assert!(k.iter().any(|e| matches!(e, EventKind::Usage(u) if u.input == 40 && u.cache_read == 60)));
        assert!(matches!(k.last(), Some(EventKind::TurnEnd)));
        std::fs::remove_dir_all(&dir).ok();
    }
}

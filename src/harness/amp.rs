//! Amp: one JSON file per thread, `~/.local/share/amp/threads/T-*.json` (some installs use
//! `~/.config/amp`), rewritten whole. Current Amp versions may keep threads on the server only
//! and leave nothing here, so this is best effort. See docs/harnesses.md §13.
//!
//! Only `threads/T-*.json` is opened; `secrets.json` beside it holds credentials.

use super::{any_ts, clip, home, modified, tool_summary, xdg_data, Adapter, JsonFile, SourceReader, SourceSpec};
use crate::model::{AgentRef, Event, EventKind, SessionKey, Usage};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

pub struct Amp;

const ID: &str = "amp";

impl Adapter for Amp {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        "Amp"
    }
    fn default_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![xdg_data().join("amp"), home().join(".config").join("amp")];
        roots.dedup();
        roots
    }

    fn discover(&self, roots: &[PathBuf], since: SystemTime) -> Vec<SourceSpec> {
        let mut out = Vec::new();
        for root in roots {
            let Ok(entries) = std::fs::read_dir(root.join("threads")) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let Some(id) = name.strip_suffix(".json").filter(|n| n.starts_with("T-")).map(str::to_owned) else { continue };
                if let Some(m) = modified(&p).filter(|m| *m >= since) {
                    out.push(SourceSpec { key: SessionKey { harness: ID, id }, agent: AgentRef::Lead, path: p, modified: m });
                }
            }
        }
        out
    }

    fn open(&self, spec: &SourceSpec) -> Box<dyn SourceReader> {
        Box::new(Reader { key: spec.key.clone(), file: JsonFile::new(spec.path.clone()), started: false, title: None, model: None, seen: HashMap::new() })
    }
}

/// What was already emitted for one message, by `messageId`.
#[derive(Default)]
struct Seen {
    blocks: HashSet<usize>,
    results: HashSet<String>,
    done: bool,
}

struct Reader {
    key: SessionKey,
    file: JsonFile,
    started: bool,
    title: Option<String>,
    model: Option<String>,
    seen: HashMap<i64, Seen>,
}

impl Reader {
    fn ev(&self, at: SystemTime, kind: EventKind) -> Event {
        Event { session: self.key.clone(), agent: AgentRef::Lead, at, kind }
    }

    fn thread(&mut self, t: &Value, out: &mut Vec<Event>) {
        let title = t.get("title").and_then(Value::as_str).filter(|s| !s.is_empty()).map(|s| clip(s, 60));
        if !self.started || title != self.title {
            let tree = t.pointer("/env/initial/trees/0");
            let cwd = tree.and_then(|x| x.get("uri")).and_then(Value::as_str).and_then(file_uri);
            let branch = tree.and_then(|x| x.pointer("/repository/ref")).and_then(Value::as_str).map(|r| r.trim_start_matches("refs/heads/").to_owned());
            let at = any_ts(&t["created"]).unwrap_or_else(SystemTime::now);
            out.push(self.ev(at, EventKind::Meta { cwd, branch, model: None, title: title.clone() }));
            self.title = title;
        }
        let msgs = t["messages"].as_array().map(Vec::as_slice).unwrap_or_default();
        for (i, m) in msgs.iter().enumerate() {
            let id = m.get("messageId").and_then(Value::as_i64).unwrap_or(i as i64);
            let mut seen = self.seen.remove(&id).unwrap_or_default();
            if !seen.done {
                self.message(m, i + 1 == msgs.len(), &mut seen, out);
            }
            self.seen.insert(id, seen);
        }
    }

    fn message(&mut self, m: &Value, last: bool, seen: &mut Seen, out: &mut Vec<Event>) {
        let at = m.pointer("/meta/sentAt").and_then(any_ts).unwrap_or_else(SystemTime::now);
        let role = m.get("role").and_then(Value::as_str).unwrap_or("");
        let state = m.pointer("/state/type").and_then(Value::as_str);
        // an assistant message streams until its state says complete
        let complete = role != "assistant" || state.is_some_and(|s| s != "streaming") || !last;
        let blocks = m["content"].as_array().map(Vec::as_slice).unwrap_or_default();
        for (i, b) in blocks.iter().enumerate() {
            let text = b.get("text").and_then(Value::as_str).unwrap_or("");
            let final_block = complete || i + 1 < blocks.len();
            match (role, b.get("type").and_then(Value::as_str).unwrap_or("")) {
                ("user", "text") if seen.blocks.insert(i) && !text.trim().is_empty() && !text.trim_start().starts_with('<') => {
                    out.push(self.ev(at, EventKind::UserPrompt { text: clip(text, 140) }));
                }
                ("user", "tool_result") => {
                    let call_id = b.get("toolUseID").and_then(Value::as_str).unwrap_or("").to_owned();
                    let status = b.pointer("/run/status").and_then(Value::as_str).unwrap_or("");
                    if matches!(status, "done" | "error" | "cancelled") && seen.results.insert(call_id.clone()) {
                        out.push(self.ev(at, EventKind::ToolResult { call_id, ok: status == "done" }));
                    }
                }
                ("assistant", "text") if final_block && seen.blocks.insert(i) && !text.trim().is_empty() => {
                    out.push(self.ev(at, EventKind::AssistantText { text: clip(text, 160) }));
                }
                ("assistant", "thinking") if seen.blocks.insert(i) => {
                    let t = b.get("thinking").and_then(Value::as_str).unwrap_or("");
                    out.push(self.ev(at, EventKind::Thinking { text: clip(t, 140) }));
                }
                ("assistant", "tool_use") if (final_block || b.get("complete").and_then(Value::as_bool) == Some(true)) && seen.blocks.insert(i) => {
                    let call_id = b.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
                    let name = b.get("name").and_then(Value::as_str).unwrap_or("tool").to_owned();
                    out.push(self.ev(at, EventKind::ToolCall { call_id, name, summary: tool_summary(&b["input"]) }));
                }
                _ => {}
            }
        }
        if role == "assistant" && complete {
            let u = &m["usage"];
            if let Some(model) = u.get("model").and_then(Value::as_str).filter(|x| self.model.as_deref() != Some(*x)) {
                self.model = Some(model.to_owned());
                out.push(self.ev(at, EventKind::Meta { cwd: None, branch: None, model: Some(model.to_owned()), title: None }));
            }
            let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            let usage = Usage { input: n("inputTokens"), output: n("outputTokens"), cache_read: n("cacheReadInputTokens"), cache_write: n("cacheCreationInputTokens") };
            if usage != Usage::default() {
                out.push(self.ev(at, EventKind::Usage(usage)));
            }
            if last && m.pointer("/state/stopReason").and_then(Value::as_str).is_some_and(|r| r != "tool_use") {
                out.push(self.ev(at, EventKind::TurnEnd));
            }
        }
        // user messages may still gain tool results; keep looking at them while they are newest
        seen.done = complete && (role == "assistant" || !last);
    }
}

/// `file:///home/user/proj` (or `file:///C:/…`) as a path.
fn file_uri(uri: &str) -> Option<PathBuf> {
    let p = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(p.len());
    let mut it = p.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let h: Vec<u8> = it.by_ref().take(2).collect();
            if let Some(v) = std::str::from_utf8(&h).ok().and_then(|s| u8::from_str_radix(s, 16).ok()) {
                bytes.push(v);
                continue;
            }
        }
        bytes.push(b);
    }
    let s = String::from_utf8(bytes).ok()?;
    // a drive letter comes after the third slash
    let s = if s.len() > 3 && s.as_bytes()[2] == b':' && s.starts_with('/') { s[1..].to_owned() } else { s };
    Some(PathBuf::from(s))
}

impl SourceReader for Reader {
    fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        if let Some(t) = self.file.read() {
            self.thread(&t, &mut out);
            self.started = true;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_thread_and_its_rewrites() {
        let dir = std::env::temp_dir().join(format!("shipoffice-amp-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("threads")).unwrap();
        std::fs::write(dir.join("secrets.json"), "{}").unwrap();
        let p = dir.join("threads/T-0199.json");
        let mut t: Value = serde_json::from_str(r#"{"v":1,"id":"T-0199","created":1760000000000,"title":"Speed up the build",
            "env":{"initial":{"trees":[{"displayName":"proj","uri":"file:///home/user/my%20proj","repository":{"type":"git","ref":"refs/heads/main"}}]}},
            "messages":[{"role":"user","messageId":0,"content":[{"type":"text","text":"Speed up the build"}],"meta":{"sentAt":1760000001000}},
             {"role":"assistant","messageId":1,"content":[{"type":"thinking","thinking":"hm"},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"cmd":"cargo build"},"complete":true}],
              "state":{"type":"complete","stopReason":"tool_use"},"usage":{"model":"claude-x","inputTokens":12,"outputTokens":340,"cacheReadInputTokens":18000}},
             {"role":"user","messageId":2,"content":[{"type":"tool_result","toolUseID":"toolu_1","run":{"status":"running"}}]}]}"#).unwrap();
        std::fs::write(&p, t.to_string()).unwrap();
        let specs = Amp.discover(&[dir.clone()], SystemTime::UNIX_EPOCH);
        assert_eq!(specs.len(), 1);
        let mut r = Amp.open(&specs[0]);
        let k: Vec<_> = r.poll().into_iter().map(|e| e.kind).collect();
        assert!(matches!(&k[0], EventKind::Meta { cwd: Some(c), branch: Some(b), .. } if c == &PathBuf::from("/home/user/my proj") && b == "main"));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolCall { summary, .. } if summary == "cargo build")));
        assert!(k.iter().any(|e| matches!(e, EventKind::Usage(u) if u.cache_read == 18000)));
        assert!(!k.iter().any(|e| matches!(e, EventKind::ToolResult { .. } | EventKind::TurnEnd)));

        t["messages"][2]["content"][0]["run"]["status"] = "done".into();
        t["messages"].as_array_mut().unwrap().push(serde_json::json!({"role":"assistant","messageId":3,"content":[{"type":"text","text":"Build is 2x faster."}],"state":{"type":"complete","stopReason":"end_turn"}}));
        std::fs::write(&p, t.to_string()).unwrap();
        let k: Vec<_> = r.poll().into_iter().map(|e| e.kind).collect();
        assert!(matches!(&k[..], [EventKind::ToolResult { ok: true, .. }, EventKind::AssistantText { .. }, EventKind::TurnEnd]), "{k:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

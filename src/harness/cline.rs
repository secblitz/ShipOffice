//! Cline and Roo Code. The VS Code extensions keep one folder per task under the editor's
//! globalStorage, `tasks/<id>/ui_messages.json`, rewritten whole on every change; Roo is a Cline
//! fork with the same message model and links subtasks to their parent in `history_item.json`.
//! The newer Cline runtime (CLI 3.x, JetBrains, the SDK bundle in VS Code) writes
//! `~/.cline/data/sessions/<id>/<id>.messages.json` instead, in place. See docs/harnesses.md §7,
//! §8a and the globalStorage section.

use super::{any_ts, clip, env_path, home, modified, tool_summary, Adapter, JsonFile, SourceReader, SourceSpec};
use crate::model::{AgentRef, Event, EventKind, SessionKey, Usage};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// A subagent whose last message asked for no tool and then stayed quiet this long is done.
const QUIET_END: Duration = Duration::from_secs(5);

pub struct Cline {
    id: &'static str,
    name: &'static str,
    /// the extension's globalStorage folder: lowercased `publisher.name`
    ext: &'static str,
    roo: bool,
    /// Roo task folder -> its root task, from `history_item.json`
    roots: Mutex<HashMap<PathBuf, String>>,
}

impl Cline {
    pub fn cline() -> Self {
        Self { id: "cline", name: "Cline", ext: "saoudrizwan.claude-dev", roo: false, roots: Mutex::default() }
    }

    pub fn roo() -> Self {
        Self { id: "roo-code", name: "Roo Code", ext: "rooveterinaryinc.roo-cline", roo: true, roots: Mutex::default() }
    }

    fn root_task(&self, dir: &Path, id: &str) -> String {
        if !self.roo {
            return id.to_owned();
        }
        let mut roots = self.roots.lock().unwrap();
        if let Some(r) = roots.get(dir) {
            return r.clone();
        }
        let Some(item) = std::fs::read(dir.join("history_item.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else {
            // written a moment after the task starts; look again next scan
            return id.to_owned();
        };
        let root = item.get("rootTaskId").or_else(|| item.get("parentTaskId")).and_then(Value::as_str).unwrap_or(id).to_owned();
        roots.insert(dir.to_path_buf(), root.clone());
        root
    }
}

/// Where VS Code and its forks keep an extension's globalStorage on this OS, plus the remote
/// server folders for when this runs on an SSH/WSL host.
pub fn vscode_global_storage(ext: &str) -> Vec<PathBuf> {
    const EDITORS: [&str; 6] = ["Code", "Code - Insiders", "VSCodium", "Code - OSS", "Cursor", "Windsurf"];
    const SERVERS: [&str; 4] = [".vscode-server", ".cursor-server", ".windsurf-server", ".vscodium-server"];
    // Linux ~/.config (XDG), macOS ~/Library/Application Support, Windows %APPDATA%
    let config = directories::BaseDirs::new().map(|b| b.config_dir().to_path_buf()).unwrap_or_default();
    let h = home();
    let mut users: Vec<PathBuf> = EDITORS.iter().map(|e| config.join(e).join("User")).collect();
    users.push(h.join(".var/app/com.visualstudio.code/config/Code/User"));
    users.extend(SERVERS.iter().map(|s| h.join(s).join("data").join("User")));
    if let Some(p) = env_path("VSCODE_PORTABLE") {
        users.push(p.join("user-data").join("User"));
    }
    users.into_iter().map(|u| u.join("globalStorage").join(ext)).collect()
}

impl Adapter for Cline {
    fn id(&self) -> &'static str {
        self.id
    }
    fn name(&self) -> &'static str {
        self.name
    }
    fn default_roots(&self) -> Vec<PathBuf> {
        let mut roots = vscode_global_storage(self.ext);
        if self.roo {
            // the Roo CLI's stand-in for globalStorage
            roots.push(home().join(".vscode-mock").join("global-storage"));
        } else {
            // the legacy CLI and JetBrains (tasks/) and the SDK runtime (sessions/) share one folder
            let dir = env_path("CLINE_DIR").unwrap_or_else(|| home().join(".cline"));
            roots.push(env_path("CLINE_DATA_DIR").unwrap_or_else(|| dir.join("data")));
            if let Some(s) = env_path("CLINE_SESSION_DATA_DIR") {
                roots.push(s);
            }
        }
        roots
    }

    fn discover(&self, roots: &[PathBuf], since: SystemTime) -> Vec<SourceSpec> {
        let mut out = Vec::new();
        for root in roots {
            if let Ok(tasks) = std::fs::read_dir(root.join("tasks")) {
                for t in tasks.flatten() {
                    let dir = t.path();
                    let p = dir.join("ui_messages.json");
                    let Some(m) = modified(&p).filter(|m| *m >= since) else { continue };
                    let id = t.file_name().to_string_lossy().into_owned();
                    let root_id = self.root_task(&dir, &id);
                    let agent = if root_id == id { AgentRef::Lead } else { AgentRef::Sub(id) };
                    out.push(SourceSpec { key: SessionKey { harness: self.id, id: root_id }, agent, path: p, modified: m });
                }
            }
            if !self.roo {
                let sessions = if root.file_name().is_some_and(|n| n == "sessions") { root.clone() } else { root.join("sessions") };
                sdk_sessions(&sessions, self.id, since, &mut out);
            }
        }
        out
    }

    fn open(&self, spec: &SourceSpec) -> Box<dyn SourceReader> {
        let base = Base { key: spec.key.clone(), agent: spec.agent.clone(), started: false, model: None };
        if spec.path.file_name().is_some_and(|n| n == "ui_messages.json") {
            let dir = spec.path.parent().map(Path::to_path_buf).unwrap_or_default();
            Box::new(Legacy {
                base,
                file: JsonFile::new(spec.path.clone()),
                item: std::fs::read(dir.join("history_item.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()),
                history: dir.parent().and_then(Path::parent).map(|r| r.join("state").join("taskHistory.json")),
                task_id: dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                seen: HashMap::new(),
                last_call: None,
                waited_on: None,
                helpers: HashSet::new(),
                cwd_known: false,
            })
        } else {
            Box::new(Sdk::new(base, &spec.path))
        }
    }
}

/// `sessions/<sid>/<sid>.messages.json` is the lead; `<agentId>.messages.json` beside it a subagent.
fn sdk_sessions(dir: &Path, harness: &'static str, since: SystemTime, out: &mut Vec<SourceSpec>) {
    let Ok(sessions) = std::fs::read_dir(dir) else { return };
    for s in sessions.flatten() {
        let sid = s.file_name().to_string_lossy().into_owned();
        let Ok(files) = std::fs::read_dir(s.path()) else { continue };
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".messages.json") else { continue };
            let Some(m) = modified(&f.path()).filter(|m| *m >= since) else { continue };
            let agent = if stem == sid { AgentRef::Lead } else { AgentRef::Sub(stem.to_owned()) };
            out.push(SourceSpec { key: SessionKey { harness, id: sid.clone() }, agent, path: f.path(), modified: m });
        }
    }
}

fn manifest_path(messages: &Path) -> PathBuf {
    let name = messages.file_name().map(|n| n.to_string_lossy().replace(".messages.json", ".json")).unwrap_or_default();
    messages.with_file_name(name)
}

/// What both readers share.
struct Base {
    key: SessionKey,
    agent: AgentRef,
    started: bool,
    model: Option<String>,
}

impl Base {
    fn ev(&self, at: SystemTime, kind: EventKind) -> Event {
        Event { session: self.key.clone(), agent: self.agent.clone(), at, kind }
    }

    fn lead(&self) -> bool {
        self.agent == AgentRef::Lead
    }

    fn model(&mut self, m: Option<&str>, at: SystemTime, out: &mut Vec<Event>) {
        if let Some(m) = m.filter(|m| self.lead() && !m.is_empty() && self.model.as_deref() != Some(*m)) {
            self.model = Some(m.to_owned());
            out.push(self.ev(at, EventKind::Meta { cwd: None, branch: None, model: Some(m.to_owned()), title: None }));
        }
    }

    /// The task is waiting for the user: the turn is over (or a subtask is done).
    fn end(&self, at: SystemTime) -> Event {
        match &self.agent {
            AgentRef::Lead => self.ev(at, EventKind::TurnEnd),
            AgentRef::Sub(id) => self.ev(at, EventKind::SubagentEnd { agent_id: id.clone() }),
        }
    }
}

/// What was already emitted for one `ui_messages.json` entry, keyed by its `ts`.
#[derive(Default)]
struct Seen {
    shown: bool,
    usage: bool,
}

struct Legacy {
    base: Base,
    file: JsonFile,
    item: Option<Value>,
    history: Option<PathBuf>,
    task_id: String,
    seen: HashMap<i64, Seen>,
    /// the command or MCP call whose output comes next
    last_call: Option<String>,
    /// the trailing ask already reported
    waited_on: Option<i64>,
    /// in-process subagents (`say:"subagent"` items) already announced
    helpers: HashSet<String>,
    cwd_known: bool,
}

/// Asks that mean the task is finished and idle, rather than blocked on an approval.
const IDLE_ASKS: [&str; 5] = ["completion_result", "resume_task", "resume_completed_task", "api_req_failed", "mistake_limit_reached"];

impl Legacy {
    /// Cline keeps the task's starting folder in the global history index; Roo in the task's own item.
    fn meta(&mut self, msgs: &[Value], out: &mut Vec<Event>) {
        if !self.base.lead() {
            return;
        }
        let item_cwd = self.item.as_ref().and_then(|i| i.get("workspace").and_then(Value::as_str)).map(PathBuf::from);
        let cwd = item_cwd
            .or_else(|| {
                if self.base.started {
                    return None;
                }
                let h = std::fs::read(self.history.as_ref()?).ok().filter(|b| b.len() < 32 * 1024 * 1024)?;
                let v: Value = serde_json::from_slice(&h).ok()?;
                v.as_array()?
                    .iter()
                    .find(|t| t.get("id").and_then(Value::as_str) == Some(self.task_id.as_str()))?
                    .get("cwdOnTaskInitialization")?
                    .as_str()
                    .map(PathBuf::from)
            })
            // fallback: the environment details the first request carried
            .or_else(|| msgs.iter().find_map(|m| between(m.get("text")?.as_str()?, "Current Working Directory (", ")").map(PathBuf::from)));
        let title = msgs
            .iter()
            .find(|m| m.get("say").and_then(Value::as_str) == Some("task"))
            .and_then(|m| m.get("text").and_then(Value::as_str))
            .map(|t| clip(t, 60));
        if !self.base.started || (cwd.is_some() && !self.cwd_known) {
            self.cwd_known |= cwd.is_some();
            out.push(self.base.ev(SystemTime::now(), EventKind::Meta { cwd, branch: None, model: None, title }));
        }
    }

    fn entry(&mut self, m: &Value, out: &mut Vec<Event>) {
        let Some(ts) = m.get("ts").and_then(Value::as_i64) else { return };
        if m.get("partial").and_then(Value::as_bool).unwrap_or(false) {
            return;
        }
        let at = any_ts(&m["ts"]).unwrap_or_else(SystemTime::now);
        let mut seen = self.seen.remove(&ts).unwrap_or_default();
        let text = m.get("text").and_then(Value::as_str).unwrap_or("");
        let kind = m.get("say").or_else(|| m.get("ask")).and_then(Value::as_str).unwrap_or("");
        let lead = self.base.lead();
        let id = ts.to_string();
        if kind == "api_req_started" {
            // tokens and cost are filled in when the request finishes
            let info: Value = serde_json::from_str(text).unwrap_or(Value::Null);
            self.base.model(m.pointer("/modelInfo/modelId").and_then(Value::as_str), at, out);
            let finished = info.get("cost").is_some_and(|c| !c.is_null()) || info.get("cancelReason").is_some_and(|c| !c.is_null());
            if finished && !seen.usage {
                seen.usage = true;
                let n = |k: &str| info.get(k).and_then(Value::as_u64).unwrap_or(0);
                out.push(self.base.ev(at, EventKind::Usage(Usage { input: n("tokensIn"), output: n("tokensOut"), cache_read: n("cacheReads"), cache_write: n("cacheWrites") })));
            }
        } else if !seen.shown {
            seen.shown = true;
            match kind {
                "task" | "user_feedback" if lead && !text.trim().is_empty() => out.push(self.base.ev(at, EventKind::UserPrompt { text: clip(text, 140) })),
                "text" | "completion_result" if m.get("type").and_then(Value::as_str) == Some("say") && !text.trim().is_empty() => {
                    out.push(self.base.ev(at, EventKind::AssistantText { text: clip(text, 160) }));
                }
                "reasoning" => out.push(self.base.ev(at, EventKind::Thinking { text: clip(text, 140) })),
                "tool" => {
                    let t: Value = serde_json::from_str(text).unwrap_or(Value::Null);
                    let name = t.get("tool").and_then(Value::as_str).unwrap_or("tool").to_owned();
                    let summary = match tool_summary(&t) {
                        s if s.is_empty() => t.get("content").and_then(Value::as_str).map(|c| clip(c, 90)).unwrap_or_default(),
                        s => s,
                    };
                    out.push(self.base.ev(at, EventKind::ToolCall { call_id: id, name, summary }));
                }
                "command" => {
                    self.last_call = Some(id.clone());
                    out.push(self.base.ev(at, EventKind::ToolCall { call_id: id, name: "execute_command".into(), summary: clip(text, 90) }));
                }
                "use_mcp_server" => {
                    let t: Value = serde_json::from_str(text).unwrap_or(Value::Null);
                    let name = t.get("toolName").or_else(|| t.get("uri")).and_then(Value::as_str).unwrap_or("use_mcp_server").to_owned();
                    self.last_call = Some(id.clone());
                    out.push(self.base.ev(at, EventKind::ToolCall { call_id: id, name, summary: t.get("serverName").and_then(Value::as_str).unwrap_or("").to_owned() }));
                }
                "command_output" | "mcp_server_response" => {
                    if let Some(call_id) = self.last_call.take() {
                        out.push(self.base.ev(at, EventKind::ToolResult { call_id, ok: true }));
                    }
                }
                _ => {}
            }
        }
        if kind == "subagent" {
            self.helpers(text, at, out);
        }
        self.seen.insert(ts, seen);
    }

    /// Cline's in-process subagents report progress in one entry that is rewritten as they run.
    fn helpers(&mut self, text: &str, at: SystemTime, out: &mut Vec<Event>) {
        let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        for (i, item) in v["items"].as_array().into_iter().flatten().enumerate() {
            let id = format!("{}-{i}", self.task_id);
            let status = item.get("status").and_then(Value::as_str).unwrap_or("");
            if self.helpers.insert(id.clone()) {
                let description = item.get("prompt").and_then(Value::as_str).map(|p| clip(p, 60)).unwrap_or_else(|| "Subagent".into());
                out.push(self.base.ev(at, EventKind::SubagentStart { agent_id: id.clone(), description, kind: None }));
            }
            if matches!(status, "completed" | "failed" | "cancelled") && self.helpers.insert(format!("{id}/done")) {
                out.push(self.base.ev(at, EventKind::SubagentEnd { agent_id: id }));
            }
        }
    }

    /// A trailing, finished `ask` means the task waits on the user.
    fn trailing_ask(&mut self, msgs: &[Value], out: &mut Vec<Event>) {
        let Some(last) = msgs.last() else { return };
        let ts = last.get("ts").and_then(Value::as_i64);
        if last.get("type").and_then(Value::as_str) != Some("ask") || last.get("partial").and_then(Value::as_bool).unwrap_or(false) || self.waited_on == ts {
            return;
        }
        self.waited_on = ts;
        let at = any_ts(&last["ts"]).unwrap_or_else(SystemTime::now);
        let ask = last.get("ask").and_then(Value::as_str).unwrap_or("");
        let text = last.get("text").and_then(Value::as_str).unwrap_or("");
        if IDLE_ASKS.contains(&ask) {
            out.push(self.base.end(at));
            return;
        }
        let what = match ask {
            "followup" | "plan_mode_respond" => {
                let q: Value = serde_json::from_str(text).unwrap_or(Value::Null);
                clip(q.get("question").or_else(|| q.get("response")).and_then(Value::as_str).unwrap_or(text), 80)
            }
            "command" => format!("Run {}?", clip(text, 60)),
            "tool" | "use_mcp_server" | "browser_action_launch" => "Waiting for approval".into(),
            _ => return,
        };
        let what = if what.is_empty() { "Waiting for your answer".into() } else { what };
        out.push(self.base.ev(at, EventKind::NeedsInput { text: what }));
    }
}

impl SourceReader for Legacy {
    fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let Some(v) = self.file.read() else { return out };
        let msgs = v.as_array().map(Vec::as_slice).unwrap_or_default();
        if !self.base.started {
            if let AgentRef::Sub(id) = &self.base.agent {
                let i = self.item.as_ref();
                let description = i.and_then(|i| i.get("task").and_then(Value::as_str)).map(|t| clip(t, 60)).unwrap_or_else(|| "Subtask".into());
                let kind = i.and_then(|i| i.get("mode").and_then(Value::as_str)).map(str::to_owned);
                out.push(self.base.ev(SystemTime::now(), EventKind::SubagentStart { agent_id: id.clone(), description, kind }));
            }
        }
        self.meta(msgs, &mut out);
        self.base.started = true;
        for m in msgs {
            self.entry(m, &mut out);
        }
        self.trailing_ask(msgs, &mut out);
        out
    }
}

/// The SDK runtime's `<id>.messages.json`: Anthropic-style messages, rewritten in place.
struct Sdk {
    base: Base,
    file: JsonFile,
    manifest: Option<JsonFile>,
    msgs: Vec<Value>,
    /// messages fully handled
    done: usize,
    /// blocks of message `done` already handled while it was still streaming
    partial_blocks: usize,
    called: HashSet<String>,
    status: Option<String>,
    /// the newest message was an assistant one that asked for no tool
    answered: bool,
    changed: Option<Instant>,
    settled: bool,
}

impl Sdk {
    fn new(base: Base, path: &Path) -> Self {
        Self {
            manifest: (base.agent == AgentRef::Lead).then(|| JsonFile::new(manifest_path(path))),
            base,
            file: JsonFile::new(path.to_path_buf()),
            msgs: Vec::new(),
            done: 0,
            partial_blocks: 0,
            called: HashSet::new(),
            status: None,
            answered: false,
            changed: None,
            settled: false,
        }
    }

    /// Handles blocks `from..` of one message and returns how many are done. While the message
    /// is still `streaming`, its last text block may grow and waits.
    fn message(&mut self, i: usize, from: usize, streaming: bool, out: &mut Vec<Event>) -> usize {
        let m = self.msgs[i].clone();
        let at = m.get("ts").and_then(any_ts).unwrap_or_else(SystemTime::now);
        let role = m.get("role").and_then(Value::as_str).unwrap_or("");
        let lead = self.base.lead();
        if role == "assistant" && from == 0 {
            self.base.model(m.pointer("/modelInfo/id").and_then(Value::as_str), at, out);
        }
        let blocks: Vec<Value> = match &m["content"] {
            Value::String(s) => vec![serde_json::json!({"type": "text", "text": s})],
            Value::Array(a) => a.clone(),
            _ => Vec::new(),
        };
        let mut handled = from;
        for (j, b) in blocks.iter().enumerate().skip(from) {
            let kind = b.get("type").and_then(Value::as_str).unwrap_or("");
            if streaming && j + 1 == blocks.len() && kind != "tool_use" && kind != "tool_result" {
                break;
            }
            handled = j + 1;
            let text = b.get("text").and_then(Value::as_str).unwrap_or("");
            match (role, kind) {
                ("user", "text") if lead => {
                    let t = text.trim_start();
                    // the first prompt arrives wrapped in <task>; other tags are injected context
                    let t = t.strip_prefix("<task>").map(|r| r.split("</task>").next().unwrap_or(r)).unwrap_or(t);
                    if !t.trim().is_empty() && !t.starts_with('<') {
                        out.push(self.base.ev(at, EventKind::UserPrompt { text: clip(t, 140) }));
                    }
                }
                ("user", "tool_result") => {
                    let call_id = b.get("tool_use_id").and_then(Value::as_str).unwrap_or("").to_owned();
                    let ok = !b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                    out.push(self.base.ev(at, EventKind::ToolResult { call_id, ok }));
                }
                ("assistant", "text") if !text.trim().is_empty() => out.push(self.base.ev(at, EventKind::AssistantText { text: clip(text, 160) })),
                ("assistant", "thinking") => {
                    let t = b.get("thinking").and_then(Value::as_str).unwrap_or("");
                    out.push(self.base.ev(at, EventKind::Thinking { text: clip(t, 140) }));
                }
                ("assistant", "tool_use") => {
                    let call_id = b.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
                    if self.called.insert(call_id.clone()) {
                        let name = b.get("name").and_then(Value::as_str).unwrap_or("tool").to_owned();
                        if name == "ask_question" {
                            out.push(self.base.ev(at, EventKind::NeedsInput { text: "Waiting for your answer".into() }));
                        }
                        out.push(self.base.ev(at, EventKind::ToolCall { call_id, name, summary: tool_summary(&b["input"]) }));
                    }
                }
                _ => {}
            }
        }
        self.answered = role == "assistant" && !blocks.iter().any(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"));
        if role == "assistant" && !streaming {
            if let Some(u) = m.get("metrics") {
                let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                out.push(self.base.ev(at, EventKind::Usage(Usage {
                    input: n("inputTokens"),
                    output: n("outputTokens"),
                    cache_read: n("cacheReadTokens"),
                    cache_write: n("cacheWriteTokens"),
                })));
            }
        }
        handled
    }

    /// The lead's manifest says whether it is running; a subagent has none, so a quiet file
    /// counts as settled.
    fn check_settled(&mut self, out: &mut Vec<Event>) -> bool {
        if let Some(v) = self.manifest.as_mut().and_then(JsonFile::read) {
            if !self.base.started {
                let cwd = v.get("cwd").or_else(|| v.get("workspace_root")).and_then(Value::as_str).map(PathBuf::from);
                let title = v.get("title").or_else(|| v.get("prompt")).and_then(Value::as_str).map(|t| clip(t, 60));
                let model = v.get("model").and_then(Value::as_str).map(str::to_owned);
                self.base.model.clone_from(&model);
                out.push(self.base.ev(SystemTime::now(), EventKind::Meta { cwd, branch: None, model, title }));
            }
            self.status = v.get("status").and_then(Value::as_str).map(str::to_owned);
        }
        match (&self.manifest, self.status.as_deref()) {
            (Some(_), Some(s)) => !matches!(s, "running" | "pending"),
            _ => self.changed.is_some_and(|t| t.elapsed() >= QUIET_END),
        }
    }
}

impl SourceReader for Sdk {
    fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let fresh = self.file.read();
        let settled = self.check_settled(&mut out);
        if fresh.is_none() && settled == self.settled {
            return out;
        }
        if let Some(v) = fresh {
            if !self.base.started {
                if let AgentRef::Sub(id) = &self.base.agent {
                    out.push(self.base.ev(SystemTime::now(), EventKind::SubagentStart { agent_id: id.clone(), description: "Subagent".into(), kind: None }));
                }
            }
            self.msgs = v["messages"].as_array().cloned().unwrap_or_default();
            self.changed = Some(Instant::now());
            if self.msgs.len() < self.done {
                // compacted or rewound: carry on from the new end
                self.done = self.msgs.len();
                self.partial_blocks = 0;
            }
        }
        self.base.started = true;
        let was_settled = self.settled;
        self.settled = settled;
        for i in self.done..self.msgs.len() {
            let streaming = i + 1 == self.msgs.len() && !settled;
            let from = if i == self.done { self.partial_blocks } else { 0 };
            let handled = self.message(i, from, streaming, &mut out);
            if streaming {
                self.partial_blocks = handled;
            } else {
                self.done = i + 1;
                self.partial_blocks = 0;
            }
        }
        if settled && !was_settled && (self.answered || self.manifest.is_some()) {
            out.push(self.base.end(SystemTime::now()));
        }
        out
    }
}

fn between<'a>(s: &'a str, a: &str, b: &str) -> Option<&'a str> {
    let start = s.find(a)? + a.len();
    let end = s[start..].find(b)? + start;
    Some(s[start..end].trim()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shipoffice-cline-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn kinds(r: &mut Box<dyn SourceReader>) -> Vec<EventKind> {
        r.poll().into_iter().map(|e| e.kind).collect()
    }

    const UI: [&str; 6] = [
        r#"{"ts":1760812345678,"type":"say","say":"task","text":"Make the tests pass"}"#,
        r#"{"ts":1760812345702,"type":"say","say":"api_req_started","text":"{\"request\":\"<task>…# Current Working Directory (/home/user/proj) Files\",\"tokensIn\":12034,\"tokensOut\":412,\"cacheWrites\":11800,\"cacheReads\":0,\"cost\":0.0521}","modelInfo":{"modelId":"claude-x","providerId":"anthropic","mode":"act"}}"#,
        r#"{"ts":1760812347001,"type":"say","say":"reasoning","text":"Looking around","partial":false}"#,
        r#"{"ts":1760812348010,"type":"say","say":"tool","text":"{\"tool\":\"readFile\",\"path\":\"src/main.rs\"}"}"#,
        r#"{"ts":1760812349100,"type":"ask","ask":"command","text":"cargo test"}"#,
        r#"{"ts":1760812355000,"type":"say","say":"command_output","text":"ok"}"#,
    ];

    #[test]
    fn legacy_task_diffs_rewrites_and_waits() {
        let dir = temp("legacy");
        let task = dir.join("tasks/1760812345678");
        std::fs::create_dir_all(&task).unwrap();
        let ui = task.join("ui_messages.json");
        std::fs::write(&ui, format!("[{}]", UI[..5].join(","))).unwrap();
        let c = Cline::cline();
        assert!(c.default_roots().iter().any(|r| r.ends_with("globalStorage/saoudrizwan.claude-dev")));
        let specs = c.discover(&[dir.clone()], SystemTime::UNIX_EPOCH);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].key.id, "1760812345678");
        let mut r = c.open(&specs[0]);
        let k = kinds(&mut r);
        assert!(matches!(&k[0], EventKind::Meta { cwd: Some(p), title: Some(t), .. } if p == Path::new("/home/user/proj") && t == "Make the tests pass"));
        assert!(k.iter().any(|e| matches!(e, EventKind::UserPrompt { .. })));
        assert!(k.iter().any(|e| matches!(e, EventKind::Usage(u) if u.input == 12034 && u.cache_write == 11800)));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolCall { name, summary, .. } if name == "readFile" && summary == "src/main.rs")));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolCall { name, .. } if name == "execute_command")));
        assert!(matches!(k.last(), Some(EventKind::NeedsInput { text }) if text == "Run cargo test?"));

        // rewritten whole with two more entries: only those come out
        let mut all = UI.to_vec();
        all.push(r#"{"ts":1760812360000,"type":"ask","ask":"completion_result","text":"","partial":false}"#);
        std::fs::write(&ui, format!("[{}]", all.join(","))).unwrap();
        let k = kinds(&mut r);
        assert!(matches!(&k[..], [EventKind::ToolResult { ok: true, .. }, EventKind::TurnEnd]), "{k:?}");
        assert!(kinds(&mut r).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn roo_subtask_belongs_to_its_root() {
        let dir = temp("roo");
        let child = dir.join("tasks/0199f0a2");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join("history_item.json"), r#"{"id":"0199f0a2","rootTaskId":"0199f09f","parentTaskId":"0199f09f","task":"Write the parser","workspace":"/home/user/proj","mode":"code","status":"active"}"#).unwrap();
        std::fs::write(child.join("ui_messages.json"), r#"[{"ts":1,"type":"say","say":"text","text":"On it"},{"ts":2,"type":"ask","ask":"completion_result","text":""}]"#).unwrap();
        let roo = Cline::roo();
        let specs = roo.discover(&[dir.clone()], SystemTime::UNIX_EPOCH);
        assert_eq!(specs[0].key.id, "0199f09f");
        assert_eq!(specs[0].agent, AgentRef::Sub("0199f0a2".into()));
        let k = kinds(&mut roo.open(&specs[0]));
        assert!(matches!(&k[0], EventKind::SubagentStart { description, kind: Some(m), .. } if description == "Write the parser" && m == "code"));
        assert!(matches!(k.last(), Some(EventKind::SubagentEnd { agent_id }) if agent_id == "0199f0a2"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sdk_session_messages() {
        let dir = temp("sdk");
        let s = dir.join("sessions/1760812345678_k3j9x");
        std::fs::create_dir_all(&s).unwrap();
        std::fs::write(s.join("1760812345678_k3j9x.json"), r#"{"version":1,"session_id":"1760812345678_k3j9x","status":"running","model":"claude-x","cwd":"/home/user/proj"}"#).unwrap();
        let msgs = |n: usize| {
            let all = [
                r#"{"role":"user","content":[{"type":"text","text":"<task>\nAdd a flag\n</task>"},{"type":"text","text":"<environment_details>x</environment_details>"}]}"#,
                r#"{"role":"assistant","content":[{"type":"thinking","thinking":"plan"},{"type":"tool_use","id":"toolu_1","name":"run_commands","input":{"command":"ls"}}],"metrics":{"inputTokens":10,"outputTokens":5,"cacheReadTokens":100}}"#,
                r#"{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"x","is_error":false}]}"#,
                r#"{"role":"assistant","content":[{"type":"text","text":"Added the flag."}]}"#,
            ];
            format!(r#"{{"version":1,"agent":"lead","sessionId":"1760812345678_k3j9x","messages":[{}]}}"#, all[..n].join(","))
        };
        let file = s.join("1760812345678_k3j9x.messages.json");
        std::fs::write(&file, msgs(4)).unwrap();
        let specs = Cline::cline().discover(&[dir.clone()], SystemTime::UNIX_EPOCH);
        assert_eq!(specs.len(), 1);
        let mut r = Cline::cline().open(&specs[0]);
        let k = kinds(&mut r);
        assert!(matches!(&k[0], EventKind::Meta { cwd: Some(_), model: Some(m), .. } if m == "claude-x"));
        assert!(k.iter().any(|e| matches!(e, EventKind::UserPrompt { text } if text == "Add a flag")));
        assert_eq!(k.iter().filter(|e| matches!(e, EventKind::UserPrompt { .. })).count(), 1);
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolCall { summary, .. } if summary == "ls")));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolResult { ok: true, .. })));
        assert!(k.iter().any(|e| matches!(e, EventKind::Usage(u) if u.cache_read == 100)));
        assert!(!k.iter().any(|e| matches!(e, EventKind::AssistantText { .. })), "the newest text may still stream");

        std::fs::write(s.join("1760812345678_k3j9x.json"), r#"{"version":1,"status":"idle","cwd":"/home/user/proj"}"#).unwrap();
        let k = kinds(&mut r);
        assert!(matches!(&k[..], [EventKind::AssistantText { text }, EventKind::TurnEnd] if text == "Added the flag."), "{k:?}");
        std::fs::write(&file, msgs(4) + " ").unwrap();
        assert!(kinds(&mut r).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}

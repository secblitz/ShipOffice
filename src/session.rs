//! Folds a stream of [`Event`]s into what the office shows: who is in the session, what each
//! agent is doing right now, and the running totals for the HUD.

use crate::model::{activity_for_tool, Activity, AgentRef, Event, EventKind, SessionKey, Usage};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The model is producing output or running tools.
    Working,
    /// The harness is waiting on the user: a question, a permission prompt, or the next prompt.
    NeedsYou,
    /// Turn finished; nothing is happening.
    Idle,
}

#[derive(Debug, Clone)]
pub struct AgentState {
    pub id: AgentRef,
    pub label: String,
    pub kind: Option<String>,
    pub activity: Activity,
    pub tool: Option<(String, String)>,
    pub said: Option<String>,
    pub thought: Option<String>,
    pub tools_used: u32,
    pub tokens: u64,
    /// tokens the agent's last request carried: how full its own context is
    pub context: u64,
    pub compactions: u32,
    pub compacted_at: Option<SystemTime>,
    pub done: bool,
    pub last: SystemTime,
    pub since: SystemTime,
}

impl AgentState {
    fn new(id: AgentRef, label: String, at: SystemTime) -> Self {
        Self {
            id,
            label,
            kind: None,
            activity: Activity::Waiting,
            tool: None,
            said: None,
            thought: None,
            tools_used: 0,
            context: 0,
            compactions: 0,
            compacted_at: None,
            tokens: 0,
            done: false,
            last: at,
            since: at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FeedItem {
    pub at: SystemTime,
    pub who: String,
    pub what: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub key: SessionKey,
    pub harness_name: String,
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub title: Option<String>,
    pub status: Status,
    pub waiting_for: Option<String>,
    pub started: SystemTime,
    pub last: SystemTime,
    pub status_since: SystemTime,
    pub usage: Usage,
    /// Tokens the last request sent: how full the context window is.
    pub context: u64,
    /// The context window the log reported, if it did.
    pub window: Option<u64>,
    pub tool_counts: BTreeMap<String, u32>,
    pub agents: BTreeMap<AgentRef, AgentState>,
    pub feed: VecDeque<FeedItem>,
    pub files_touched: u32,
}

const FEED_MAX: usize = 200;

impl Session {
    pub fn new(key: SessionKey, harness_name: &str, at: SystemTime) -> Self {
        let mut agents = BTreeMap::new();
        agents.insert(AgentRef::Lead, AgentState::new(AgentRef::Lead, "Lead".into(), at));
        Self {
            key,
            harness_name: harness_name.to_owned(),
            cwd: None,
            branch: None,
            model: None,
            title: None,
            status: Status::Idle,
            waiting_for: None,
            started: at,
            last: at,
            status_since: at,
            usage: Usage::default(),
            window: None,
            context: 0,
            tool_counts: BTreeMap::new(),
            agents,
            feed: VecDeque::new(),
            files_touched: 0,
        }
    }

    /// The project folder's name, which is what tabs show.
    pub fn project(&self) -> String {
        self.cwd
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.title.clone().unwrap_or_else(|| self.key.id.chars().take(8).collect()))
    }

    pub fn live_agents(&self) -> impl Iterator<Item = &AgentState> {
        self.agents.values().filter(|a| !a.done)
    }

    fn set_status(&mut self, s: Status, at: SystemTime) {
        if self.status != s {
            self.status = s;
            self.status_since = at;
        }
    }

    fn agent(&mut self, r: &AgentRef, at: SystemTime) -> &mut AgentState {
        let n = self.agents.len();
        self.agents
            .entry(r.clone())
            .or_insert_with(|| AgentState::new(r.clone(), format!("Helper {n}"), at))
    }

    fn feed(&mut self, at: SystemTime, who: &AgentRef, what: &str, detail: &str) {
        let who = self.agents.get(who).map(|a| a.label.clone()).unwrap_or_default();
        self.feed.push_front(FeedItem { at, who, what: what.to_owned(), detail: detail.to_owned() });
        self.feed.truncate(FEED_MAX);
    }

    pub fn apply(&mut self, e: &Event) {
        let at = e.at;
        if at > self.last {
            self.last = at;
        }
        if at < self.started {
            self.started = at;
        }
        let who = e.agent.clone();
        match &e.kind {
            EventKind::Meta { cwd, branch, model, title } => {
                if who == AgentRef::Lead && cwd.is_some() {
                    self.cwd.clone_from(cwd);
                }
                if branch.is_some() {
                    self.branch.clone_from(branch);
                }
                if model.is_some() && who == AgentRef::Lead {
                    self.model.clone_from(model);
                }
                if title.is_some() {
                    self.title.clone_from(title);
                }
            }
            EventKind::UserPrompt { text } => {
                self.set_status(Status::Working, at);
                self.waiting_for = None;
                let a = self.agent(&who, at);
                a.activity = Activity::Thinking;
                a.tool = None;
                a.last = at;
                self.feed(at, &AgentRef::Lead, "You", text);
                if let Some(f) = self.feed.front_mut() {
                    f.who = "You".into();
                    f.what = String::new();
                }
            }
            EventKind::AssistantText { text } => {
                let a = self.agent(&who, at);
                a.said = Some(text.clone());
                a.activity = Activity::Talking;
                a.last = at;
                self.feed(at, &who, "", text);
            }
            EventKind::Thinking { text } => {
                if self.status == Status::Idle && who == AgentRef::Lead {
                    self.set_status(Status::Working, at);
                }
                let a = self.agent(&who, at);
                a.activity = Activity::Thinking;
                if !text.is_empty() {
                    a.thought = Some(text.clone());
                }
                a.last = at;
            }
            EventKind::ToolCall { name, summary, .. } => {
                if who == AgentRef::Lead {
                    self.set_status(Status::Working, at);
                }
                *self.tool_counts.entry(name.clone()).or_default() += 1;
                if activity_for_tool(name) == Activity::Typing {
                    self.files_touched += 1;
                }
                let a = self.agent(&who, at);
                a.activity = activity_for_tool(name);
                a.tool = Some((name.clone(), summary.clone()));
                a.tools_used += 1;
                a.last = at;
                self.feed(at, &who, name, summary);
            }
            EventKind::ToolResult { ok, .. } => {
                let a = self.agent(&who, at);
                a.last = at;
                if !ok {
                    a.thought = Some("That didn't work".into());
                }
            }
            EventKind::SubagentStart { agent_id, description, kind } => {
                let r = AgentRef::Sub(agent_id.clone());
                let a = self.agent(&r, at);
                a.label = description.clone();
                a.kind.clone_from(kind);
                a.done = false;
                a.activity = Activity::Thinking;
                a.since = at;
                a.last = at;
                self.feed(at, &AgentRef::Lead, "Spawned", description);
            }
            EventKind::SubagentEnd { agent_id } => {
                let r = AgentRef::Sub(agent_id.clone());
                if let Some(a) = self.agents.get_mut(&r) {
                    if !a.done {
                        a.done = true;
                        a.last = at;
                        let label = a.label.clone();
                        self.feed(at, &AgentRef::Lead, "Finished", &label);
                    }
                }
            }
            EventKind::Usage(u) => {
                self.usage.add(u);
                if who == AgentRef::Lead {
                    self.context = u.total_in() + u.output;
                }
                let a = self.agent(&who, at);
                a.tokens += u.input + u.output + u.cache_write;
                a.context = u.total_in() + u.output;
            }
            EventKind::Compacted { .. } => {
                let a = self.agent(&who, at);
                a.compactions += 1;
                a.compacted_at = Some(at);
                a.context = 0;
                if who == AgentRef::Lead {
                    self.context = 0;
                }
            }
            EventKind::ContextWindow(w) => self.window = Some(*w),
            EventKind::TurnEnd => {
                self.set_status(Status::NeedsYou, at);
                self.waiting_for = Some("Your turn".into());
                let a = self.agent(&AgentRef::Lead, at);
                a.activity = Activity::Waiting;
                a.tool = None;
            }
            EventKind::NeedsInput { text } => {
                self.set_status(Status::NeedsYou, at);
                self.waiting_for = Some(text.clone());
                let a = self.agent(&who, at);
                a.activity = Activity::Waiting;
                self.feed(at, &who, "Needs you", text);
            }
        }
    }

    /// Ages the session with the clock: quiet subagents leave, and a session that has said
    /// nothing for a while is shown as idle even when the harness never wrote a turn end.
    pub fn tick(&mut self, now: SystemTime) {
        let quiet = |t: SystemTime| now.duration_since(t).unwrap_or_default();
        for a in self.agents.values_mut() {
            if let AgentRef::Sub(_) = a.id {
                if !a.done && quiet(a.last) > Duration::from_secs(600) {
                    a.done = true;
                }
            }
        }
        if self.status == Status::Working && quiet(self.last) > Duration::from_secs(180) {
            self.set_status(Status::Idle, now);
        }
    }

    /// API list price estimate in USD. Real billing depends on plan and model.
    pub fn cost_estimate(&self) -> f64 {
        let (inp, out) = price_per_mtok(self.model.as_deref().unwrap_or(""));
        let u = &self.usage;
        (u.input as f64 * inp + u.cache_write as f64 * inp * 1.25 + u.cache_read as f64 * inp * 0.1 + u.output as f64 * out) / 1e6
    }

    /// Context window size for the model, for the "context" gauge.
    pub fn context_window(&self) -> u64 {
        if let Some(w) = self.window.filter(|w| *w > 0) {
            return w;
        }
        let m = self.model.as_deref().unwrap_or("").to_ascii_lowercase();
        if m.contains("1m") {
            1_000_000
        } else if m.contains("gpt") || m.contains("codex") {
            272_000
        } else if m.contains("gemini") {
            1_000_000
        } else {
            200_000
        }
    }
}

/// (input, output) USD per million tokens, rough list prices by model family.
fn price_per_mtok(model: &str) -> (f64, f64) {
    let m = model.to_ascii_lowercase();
    if m.contains("opus") {
        (5.0, 25.0)
    } else if m.contains("haiku") {
        (1.0, 5.0)
    } else if m.contains("sonnet") || m.contains("claude") {
        (3.0, 15.0)
    } else if m.contains("mini") || m.contains("flash") {
        (0.3, 2.5)
    } else if m.contains("gpt") || m.contains("codex") || m.contains("gemini") {
        (1.25, 10.0)
    } else {
        (3.0, 15.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(agent: AgentRef, kind: EventKind) -> Event {
        Event { session: SessionKey { harness: "t", id: "s".into() }, agent, at: SystemTime::now(), kind }
    }

    #[test]
    fn a_turn_moves_through_the_statuses() {
        let mut s = Session::new(SessionKey { harness: "t", id: "s".into() }, "Test", SystemTime::now());
        s.apply(&ev(AgentRef::Lead, EventKind::UserPrompt { text: "go".into() }));
        assert_eq!(s.status, Status::Working);
        s.apply(&ev(AgentRef::Lead, EventKind::ToolCall { call_id: "1".into(), name: "Read".into(), summary: "a.rs".into() }));
        assert_eq!(s.agents[&AgentRef::Lead].activity, Activity::Reading);
        s.apply(&ev(AgentRef::Lead, EventKind::SubagentStart { agent_id: "x".into(), description: "Write tests".into(), kind: None }));
        assert_eq!(s.live_agents().count(), 2);
        s.apply(&ev(AgentRef::Lead, EventKind::SubagentEnd { agent_id: "x".into() }));
        assert_eq!(s.live_agents().count(), 1);
        s.apply(&ev(AgentRef::Lead, EventKind::TurnEnd));
        assert_eq!(s.status, Status::NeedsYou);
    }
}

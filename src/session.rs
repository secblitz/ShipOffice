//! Folds a stream of [`Event`]s into what the office shows: who is in the session, what each
//! agent is doing right now, and the running totals for the HUD.

use crate::harness::compaction::{Rule, Settings};
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
    /// the most it has carried, which the window can be no smaller than
    pub peak: u64,
    /// the model this agent runs on, when its log says (helpers may run a different one)
    pub model: Option<String>,
    /// how big the context was when the harness last compacted it on its own
    pub auto_at: Option<u64>,
    pub compactions: u32,
    pub compacted_at: Option<SystemTime>,
    /// what it is asking you, while it waits on you (a permission prompt, a question)
    pub needs: Option<String>,
    pub needs_since: Option<SystemTime>,
    /// tool calls that failed, and when the last one did
    pub errors: u32,
    pub error_at: Option<SystemTime>,
    /// the helpers started with it: one phase of a workflow, or a batch spawned at once
    pub team: Option<String>,
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
            peak: 0,
            model: None,
            auto_at: None,
            compactions: 0,
            compacted_at: None,
            needs: None,
            needs_since: None,
            errors: 0,
            error_at: None,
            team: None,
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
    /// the log has marked a compaction, so a drop in context is not taken for one
    marked: bool,
    /// the last helper started and its batch, so helpers spawned together form a team
    last_spawn: Option<(SystemTime, String)>,
    batches: u32,
}

const FEED_MAX: usize = 200;
/// Helpers started this close together are one batch.
const BATCH: Duration = Duration::from_secs(4);
/// Finished helpers a session remembers.
const KEEP_DONE: usize = 64;

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
            marked: false,
            last_spawn: None,
            batches: 0,
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
        // anything it does after asking means it got its answer
        if matches!(e.kind, EventKind::UserPrompt { .. } | EventKind::AssistantText { .. } | EventKind::Thinking { .. } | EventKind::ToolCall { .. } | EventKind::ToolResult { .. }) {
            if let Some(a) = self.agents.get_mut(&who).filter(|a| a.needs_since.is_some_and(|t| at >= t)) {
                a.needs = None;
                a.needs_since = None;
            }
        }
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
                if model.is_some() {
                    self.agent(&who, at).model.clone_from(model);
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
                    a.errors += 1;
                    a.error_at = Some(at);
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
                // helpers spawned within a few seconds of each other are a batch working together
                let batch = match &self.last_spawn {
                    Some((t, b)) if at.duration_since(*t).unwrap_or_default() < BATCH && t.duration_since(at).unwrap_or_default() < BATCH => b.clone(),
                    _ => {
                        self.batches += 1;
                        format!("batch {}", self.batches)
                    }
                };
                self.last_spawn = Some((at, batch.clone()));
                let a = self.agent(&r, at);
                if a.team.as_ref().is_none_or(|t| t.starts_with("batch ")) {
                    a.team = Some(batch);
                }
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
                let now = u.total_in() + u.output;
                if who == AgentRef::Lead {
                    self.context = now;
                }
                // a harness that never writes down its compactions still shows them: the context
                // it sends falls to well under half in one step
                let infer = !self.marked && !crate::harness::marks_compactions(self.key.harness);
                let a = self.agent(&who, at);
                if infer && a.context >= 20_000 && now * 2 < a.context {
                    a.compactions += 1;
                    a.compacted_at = Some(at);
                }
                a.tokens += u.input + u.output + u.cache_write;
                a.context = now;
                a.peak = a.peak.max(now);
            }
            EventKind::Compacted { pre_tokens, auto } => {
                self.marked = true;
                let a = self.agent(&who, at);
                a.compactions += 1;
                a.compacted_at = Some(at);
                if let Some(p) = pre_tokens {
                    a.peak = a.peak.max(*p);
                    if *auto == Some(true) {
                        a.auto_at = Some(*p);
                    }
                }
                a.context = 0;
                if who == AgentRef::Lead {
                    self.context = 0;
                }
            }
            EventKind::ContextWindow(w) => self.window = Some(*w),
            EventKind::Team { name } => self.agent(&who, at).team = Some(name.clone()),
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
                a.needs = Some(text.clone());
                a.needs_since = Some(at);
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
        // a long run spawns helpers by the thousand: only the latest finished ones are kept
        let done = self.agents.values().filter(|a| a.done).count();
        if done > KEEP_DONE {
            let mut ends: Vec<SystemTime> = self.agents.values().filter(|a| a.done).map(|a| a.last).collect();
            ends.sort_unstable();
            let cut = ends[done - KEEP_DONE];
            self.agents.retain(|_, a| !a.done || a.last >= cut);
        }
    }

    /// Whether an agent that should be working has done nothing for `limit`: no line in its log,
    /// no tool call, no reply. One waiting on you or on its helpers is not stuck.
    pub fn stuck(&self, a: &AgentState, now: SystemTime, limit: Duration) -> bool {
        let working = if a.id == AgentRef::Lead { self.status == Status::Working } else { !a.done };
        working && a.needs.is_none() && a.activity != Activity::Waiting && now.duration_since(a.last).unwrap_or_default() > limit
    }

    /// API list price estimate in USD. Real billing depends on plan and model.
    pub fn cost_estimate(&self) -> f64 {
        let (inp, out) = price_per_mtok(self.model.as_deref().unwrap_or(""));
        let u = &self.usage;
        (u.input as f64 * inp + u.cache_write as f64 * inp * 1.25 + u.cache_read as f64 * inp * 0.1 + u.output as f64 * out) / 1e6
    }

    /// The context window of one agent (the lead when `who` is not in the session) and where
    /// that number came from. `set` is the user's own compaction setting for this harness.
    pub fn window(&self, who: &AgentRef, set: &Settings) -> (u64, WindowFrom) {
        let a = self.agents.get(who).or_else(|| self.agents.get(&AgentRef::Lead));
        let model = a.and_then(|a| a.model.as_deref()).or(self.model.as_deref()).unwrap_or("");
        let (w, from) = if let Some(w) = self.window.filter(|w| *w > 0) {
            (w, WindowFrom::Log)
        } else if let Some(w) = set.window.as_ref().map(|w| w.0).filter(|w| *w > 0) {
            (w, WindowFrom::Setting)
        } else if let Some(at) = a.and_then(|a| a.auto_at) {
            // it compacted itself at `at`: near the limit, or at the share the user set
            let share = match set.rule.as_ref().map(|r| &r.0) {
                Some(Rule::Share(x)) if *x > 0.05 => *x as f64,
                _ => 1.0,
            };
            (round_up((at as f64 / share) as u64), WindowFrom::Compaction)
        } else {
            model_window(model).map_or((200_000, WindowFrom::Default), |w| (w, WindowFrom::Model))
        };
        // nothing carries more than its window: when this one did, the window is bigger
        match a.map_or(0, |a| a.peak) {
            p if p > w => (round_up(p), WindowFrom::Seen),
            _ => (w, from),
        }
    }

    /// The lead's window, with no settings known.
    pub fn context_window(&self) -> u64 {
        self.window(&AgentRef::Lead, &Settings::default()).0
    }
}

/// Where a context window's size came from, most trusted first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowFrom {
    /// the harness wrote it in the log
    Log,
    /// the user set it (Claude Code's `autoCompactWindow`, Codex's `model_context_window`)
    Setting,
    /// what the agent's last automatic compaction implies
    Compaction,
    /// what the model takes
    Model,
    /// the agent carried more than any of the above allow
    Seen,
    /// an unknown model
    Default,
}

/// The window sizes models come in, for rounding a size seen in a log up to a real one.
const WINDOWS: [u64; 10] = [128_000, 200_000, 256_000, 272_000, 400_000, 500_000, 1_000_000, 2_000_000, 4_000_000, 10_000_000];

fn round_up(n: u64) -> u64 {
    WINDOWS.iter().copied().find(|&w| w >= n).unwrap_or(n)
}

/// The context window a model takes, by its name as harnesses log it.
pub fn model_window(model: &str) -> Option<u64> {
    let m = model.to_ascii_lowercase();
    if m.contains("[1m]") || m.ends_with("-1m") {
        return Some(1_000_000);
    }
    let has = |keys: &[&str]| keys.iter().any(|k| m.contains(k));
    if has(&["claude", "opus", "sonnet", "haiku", "fable", "mythos"]) {
        if has(&["fable", "mythos"]) {
            return Some(1_000_000);
        }
        // the version after the family name: claude-opus-4-6, claude-sonnet-5, claude-3-7-sonnet
        let v: Vec<u32> = m.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty() && p.len() < 3).filter_map(|p| p.parse().ok()).collect();
        let (major, minor) = (v.first().copied().unwrap_or(0), v.get(1).copied().unwrap_or(0));
        return Some(if major >= 5 || (major == 4 && minor >= 6) { 1_000_000 } else { 200_000 });
    }
    if has(&["gemini"]) {
        return Some(1_000_000);
    }
    if has(&["gpt-4.1"]) {
        return Some(1_000_000);
    }
    if has(&["gpt-5", "codex"]) {
        return Some(272_000);
    }
    if has(&["gpt-4o", "deepseek", "devstral"]) {
        return Some(128_000);
    }
    if has(&["o3", "o4-mini", "o1"]) && m.starts_with('o') {
        return Some(200_000);
    }
    if has(&["qwen", "kimi", "grok"]) {
        return Some(256_000);
    }
    if has(&["glm"]) {
        return Some(200_000);
    }
    None
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
    fn questions_errors_and_silence_are_signals() {
        let mut s = Session::new(SessionKey { harness: "t", id: "s".into() }, "Test", SystemTime::now());
        let lead = AgentRef::Lead;
        s.apply(&ev(lead.clone(), EventKind::UserPrompt { text: "go".into() }));
        s.apply(&ev(lead.clone(), EventKind::NeedsInput { text: "Allow Bash?".into() }));
        assert_eq!(s.agents[&lead].needs.as_deref(), Some("Allow Bash?"));
        // asking is not being stuck
        let later = SystemTime::now() + Duration::from_secs(3600);
        assert!(!s.stuck(&s.agents[&lead], later, Duration::from_secs(480)));
        // an answer clears the question; a failed call counts
        s.apply(&ev(lead.clone(), EventKind::ToolCall { call_id: "1".into(), name: "Bash".into(), summary: "ls".into() }));
        assert!(s.agents[&lead].needs.is_none());
        s.apply(&ev(lead.clone(), EventKind::ToolResult { call_id: "1".into(), ok: false }));
        assert_eq!(s.agents[&lead].errors, 1);
        assert!(s.agents[&lead].error_at.is_some());
        // working, then nothing for longer than the limit
        assert!(!s.stuck(&s.agents[&lead], SystemTime::now() + Duration::from_secs(60), Duration::from_secs(480)));
        assert!(s.stuck(&s.agents[&lead], later, Duration::from_secs(480)));
        // helpers started together are one team
        for i in 0..3 {
            s.apply(&ev(lead.clone(), EventKind::SubagentStart { agent_id: format!("h{i}"), description: format!("t{i}"), kind: None }));
        }
        let teams: std::collections::HashSet<_> = (0..3).map(|i| s.agents[&AgentRef::Sub(format!("h{i}"))].team.clone()).collect();
        assert_eq!(teams.len(), 1);
        assert!(teams.iter().next().unwrap().is_some());
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

    #[test]
    fn the_window_comes_from_the_most_trusted_place() {
        let key = |h: &'static str| SessionKey { harness: h, id: "s".into() };
        let at = |k: &SessionKey, agent: AgentRef, kind: EventKind| Event { session: k.clone(), agent, at: SystemTime::now(), kind };
        let k = key("claude-code");
        let mut s = Session::new(k.clone(), "Claude Code", SystemTime::now());
        s.apply(&at(&k, AgentRef::Lead, EventKind::Meta { cwd: None, branch: None, model: Some("claude-opus-5-5".into()), title: None }));
        let none = Settings::default();
        assert_eq!(s.window(&AgentRef::Lead, &none), (1_000_000, WindowFrom::Model));
        let set = Settings { rule: None, window: Some((500_000, "~/.claude/settings.json".into())) };
        assert_eq!(s.window(&AgentRef::Lead, &set), (500_000, WindowFrom::Setting));
        // a helper on an older model has the smaller window
        s.apply(&at(&k, AgentRef::Lead, EventKind::SubagentStart { agent_id: "h".into(), description: "x".into(), kind: None }));
        s.apply(&at(&k, AgentRef::Sub("h".into()), EventKind::Meta { cwd: None, branch: None, model: Some("claude-haiku-4-5-20251001".into()), title: None }));
        assert_eq!(s.window(&AgentRef::Sub("h".into()), &none), (200_000, WindowFrom::Model));
        // compacting on its own at 467k says the window is 500k
        s.apply(&at(&k, AgentRef::Lead, EventKind::Compacted { pre_tokens: Some(467_276), auto: Some(true) }));
        assert_eq!(s.window(&AgentRef::Lead, &none), (500_000, WindowFrom::Compaction));
        // never smaller than what it carried
        let u = Usage { input: 10, output: 10, cache_read: 690_000, cache_write: 0 };
        s.apply(&at(&k, AgentRef::Lead, EventKind::Usage(u)));
        assert_eq!(s.window(&AgentRef::Lead, &set), (1_000_000, WindowFrom::Seen));
        assert_eq!(s.agents[&AgentRef::Lead].compactions, 1);
    }

    #[test]
    fn a_big_drop_in_context_counts_as_a_compaction_where_the_log_says_nothing() {
        let k = SessionKey { harness: "gemini-cli", id: "s".into() };
        let mut s = Session::new(k.clone(), "Gemini CLI", SystemTime::now());
        let usage = |n: u64| Event { session: k.clone(), agent: AgentRef::Lead, at: SystemTime::now(), kind: EventKind::Usage(Usage { input: n, ..Usage::default() }) };
        for n in [40_000, 90_000, 150_000, 30_000, 45_000] {
            s.apply(&usage(n));
        }
        assert_eq!(s.agents[&AgentRef::Lead].compactions, 1);
        assert_eq!(s.agents[&AgentRef::Lead].peak, 150_000);
    }

    #[test]
    fn model_windows() {
        for (m, w) in [
            ("claude-opus-5-5", 1_000_000),
            ("claude-sonnet-4-6", 1_000_000),
            ("claude-fable-5-1", 1_000_000),
            ("claude-haiku-5-5", 1_000_000),
            ("claude-haiku-4-5-20251001", 200_000),
            ("claude-sonnet-4-5-20250929", 200_000),
            ("claude-sonnet-4-5[1m]", 1_000_000),
            ("claude-opus-4-20250514", 200_000),
            ("claude-3-7-sonnet-20250219", 200_000),
            ("us.anthropic.claude-opus-4-1-20250805-v1:0", 200_000),
            ("gpt-5.3-codex", 272_000),
            ("gemini-3-pro", 1_000_000),
        ] {
            assert_eq!(model_window(m), Some(w), "{m}");
        }
        assert_eq!(model_window("some-local-model"), None);
    }

    #[test]
    fn finished_helpers_do_not_pile_up() {
        let k = SessionKey { harness: "t", id: "s".into() };
        let mut s = Session::new(k.clone(), "Test", SystemTime::now());
        for n in 0..500 {
            let id = format!("h{n}");
            s.apply(&ev(AgentRef::Lead, EventKind::SubagentStart { agent_id: id.clone(), description: "x".into(), kind: None }));
            s.apply(&ev(AgentRef::Lead, EventKind::SubagentEnd { agent_id: id }));
        }
        s.apply(&ev(AgentRef::Lead, EventKind::SubagentStart { agent_id: "live".into(), description: "x".into(), kind: None }));
        s.tick(SystemTime::now());
        assert!(s.agents.len() <= KEEP_DONE + 2, "{}", s.agents.len());
        assert_eq!(s.live_agents().count(), 2);
    }
}

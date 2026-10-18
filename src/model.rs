//! The harness-neutral picture of what agents are doing.
//!
//! Every harness adapter turns its own log format into [`Event`]s. The rest of the app only
//! ever sees these, so adding a harness never touches the office or the HUD.

use std::path::PathBuf;
use std::time::SystemTime;

/// Identifies one session across reloads: the harness plus its own session id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionKey {
    pub harness: &'static str,
    pub id: String,
}

impl std::fmt::Display for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.harness, self.id)
    }
}

/// Which agent inside a session an event belongs to. The lead is the agent the user talks to;
/// subagents are the helpers it spawns (Claude Code's Task tool, Codex sub-tasks and so on).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AgentRef {
    Lead,
    Sub(String),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    pub fn add(&mut self, o: &Usage) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
    }
    pub fn total_in(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventKind {
    /// Session facts that can arrive at any time and only fill in what is known.
    Meta { cwd: Option<PathBuf>, branch: Option<String>, model: Option<String>, title: Option<String> },
    UserPrompt { text: String },
    AssistantText { text: String },
    Thinking { text: String },
    ToolCall { call_id: String, name: String, summary: String },
    ToolResult { call_id: String, ok: bool },
    SubagentStart { agent_id: String, description: String, kind: Option<String> },
    SubagentEnd { agent_id: String },
    Usage(Usage),
    /// The model finished its turn and the harness is waiting for the user.
    TurnEnd,
    /// The harness asks the user something (a permission prompt, a plan approval...).
    NeedsInput { text: String },
    /// The agent's context was compacted (summarised to make room); `pre_tokens` is how big it was.
    Compacted { pre_tokens: Option<u64> },
    /// The model's context window in tokens, when the log says so.
    ContextWindow(u64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub session: SessionKey,
    pub agent: AgentRef,
    pub at: SystemTime,
    pub kind: EventKind,
}

/// What a tool call looks like in the office: the animation and where the agent goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Activity {
    Typing,
    Reading,
    Running,
    Thinking,
    Talking,
    Waiting,
}

/// Maps a tool name from any harness to the activity it looks like.
///
/// Names differ per harness (Claude Code `Read`, Codex `shell`, Gemini `read_file`, Cline
/// `read_file`/`execute_command`...), so this matches on lowercase substrings.
pub fn activity_for_tool(name: &str) -> Activity {
    let n = name.to_ascii_lowercase();
    let has = |keys: &[&str]| keys.iter().any(|k| n.contains(k));
    if has(&["read", "grep", "glob", "search", "find", "list", "ls", "fetch", "web", "view", "cat", "open"]) {
        Activity::Reading
    } else if has(&["bash", "shell", "exec", "command", "run", "terminal", "test", "build"]) {
        Activity::Running
    } else if has(&["todo", "plan", "think"]) {
        Activity::Thinking
    } else {
        Activity::Typing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_from_several_harnesses_map_to_activities() {
        assert_eq!(activity_for_tool("Read"), Activity::Reading);
        assert_eq!(activity_for_tool("read_file"), Activity::Reading);
        assert_eq!(activity_for_tool("Grep"), Activity::Reading);
        assert_eq!(activity_for_tool("WebSearch"), Activity::Reading);
        assert_eq!(activity_for_tool("Bash"), Activity::Running);
        assert_eq!(activity_for_tool("shell"), Activity::Running);
        assert_eq!(activity_for_tool("execute_command"), Activity::Running);
        assert_eq!(activity_for_tool("Edit"), Activity::Typing);
        assert_eq!(activity_for_tool("apply_patch"), Activity::Typing);
        assert_eq!(activity_for_tool("write_to_file"), Activity::Typing);
        assert_eq!(activity_for_tool("TodoWrite"), Activity::Thinking);
    }
}

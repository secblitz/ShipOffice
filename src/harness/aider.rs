//! Aider: an append-only Markdown log, `<repo>/.aider.chat.history.md`, with one
//! `# aider chat started at …` header per session. There is no global index, so this only finds
//! repos it is pointed at: `AIDER_CHAT_HISTORY_FILE`, or folders added in the settings (each
//! folder and its direct subfolders are checked). See docs/harnesses.md §10.
//!
//! The format has prompts, answers, applied edits and token lines, but no times or tool calls.

use super::{clip, env_path, modified, Adapter, SourceReader, SourceSpec};
use crate::model::{AgentRef, Event, EventKind, SessionKey, Usage};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct Aider;

const ID: &str = "aider";
const FILE: &str = ".aider.chat.history.md";
/// A history holds every session ever run in the repo; the first read starts this far from the end.
const BACKLOG: u64 = 1024 * 1024;
const PER_POLL: u64 = 8 * 1024 * 1024;
/// Subfolders of a configured folder that are looked into, at most.
const MAX_CHILDREN: usize = 500;

impl Adapter for Aider {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        "Aider"
    }
    fn default_roots(&self) -> Vec<PathBuf> {
        env_path("AIDER_CHAT_HISTORY_FILE").into_iter().collect()
    }

    fn discover(&self, roots: &[PathBuf], since: SystemTime) -> Vec<SourceSpec> {
        let mut files = Vec::new();
        for root in roots {
            if root.is_file() {
                files.push(root.clone());
                continue;
            }
            files.push(root.join(FILE));
            let Ok(entries) = std::fs::read_dir(root) else { continue };
            for e in entries.flatten().take(MAX_CHILDREN) {
                if !e.file_name().to_string_lossy().starts_with('.') && e.file_type().is_ok_and(|t| t.is_dir()) {
                    files.push(e.path().join(FILE));
                }
            }
        }
        files
            .into_iter()
            .filter_map(|p| {
                let m = modified(&p).filter(|m| *m >= since)?;
                // one reader per file; it switches sessions at each header
                Some(SourceSpec { key: SessionKey { harness: ID, id: file_key(&p) }, agent: AgentRef::Lead, path: p, modified: m })
            })
            .collect()
    }

    fn open(&self, spec: &SourceSpec) -> Box<dyn SourceReader> {
        let len = std::fs::metadata(&spec.path).map(|m| m.len()).unwrap_or(0);
        Box::new(Reader {
            path: spec.path.clone(),
            cwd: spec.path.parent().map(Path::to_path_buf),
            base: file_key(&spec.path),
            key: spec.key.clone(),
            offset: len.saturating_sub(BACKLOG),
            // mid-file: drop the cut line, and wait for a session header before trusting anything
            skip_first: len > BACKLOG,
            partial: Vec::new(),
            first_at: modified(&spec.path).unwrap_or_else(SystemTime::now),
            started: false,
            in_session: len <= BACKLOG,
            prompting: false,
            answered: true,
            thinking: false,
            fence: false,
            calls: 0,
        })
    }
}

/// A short stable id for a history file, so session ids don't carry the path.
fn file_key(p: &Path) -> String {
    let h = p.to_string_lossy().bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    format!("{:08x}", h as u32)
}

struct Reader {
    path: PathBuf,
    cwd: Option<PathBuf>,
    base: String,
    key: SessionKey,
    offset: u64,
    skip_first: bool,
    partial: Vec<u8>,
    first_at: SystemTime,
    started: bool,
    in_session: bool,
    /// inside a run of `#### ` prompt lines
    prompting: bool,
    /// the current turn's answer was shown already
    answered: bool,
    thinking: bool,
    fence: bool,
    calls: u64,
}

impl Reader {
    fn ev(&self, at: SystemTime, kind: EventKind) -> Event {
        Event { session: self.key.clone(), agent: AgentRef::Lead, at, kind }
    }

    /// Complete new lines since the last call.
    fn lines(&mut self) -> Vec<String> {
        let Ok(mut f) = File::open(&self.path) else { return Vec::new() };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            self.offset = 0;
            self.partial.clear();
        }
        if len == self.offset || f.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut buf = Vec::new();
        let n = f.take(PER_POLL).read_to_end(&mut buf).unwrap_or(0);
        self.offset += n as u64;
        self.partial.extend_from_slice(&buf[..n]);
        let Some(end) = self.partial.iter().rposition(|&b| b == b'\n') else { return Vec::new() };
        let done: Vec<u8> = self.partial.drain(..=end).collect();
        let mut out: Vec<String> = String::from_utf8_lossy(&done).lines().map(str::to_owned).collect();
        if std::mem::take(&mut self.skip_first) && !out.is_empty() {
            out.remove(0);
        }
        out
    }

    fn line(&mut self, l: &str, at: SystemTime, out: &mut Vec<Event>) {
        if let Some(when) = l.strip_prefix("# aider chat started at ") {
            self.key = SessionKey { harness: ID, id: format!("{}-{}", self.base, when.trim().replace([' ', ':'], "")) };
            self.in_session = true;
            self.answered = true;
            out.push(self.ev(at, EventKind::Meta { cwd: self.cwd.clone(), branch: None, model: None, title: None }));
            return;
        }
        if !self.in_session {
            return;
        }
        if let Some(p) = l.strip_prefix("#### ") {
            if !self.prompting && !p.trim().is_empty() {
                out.push(self.ev(at, EventKind::UserPrompt { text: clip(p, 140) }));
            }
            self.prompting = true;
            self.answered = false;
            return;
        }
        self.prompting = false;
        if let Some(note) = l.strip_prefix("> ") {
            self.note(note.trim(), at, out);
            return;
        }
        let t = l.trim();
        if t.starts_with("► **THINKING**") {
            self.thinking = true;
            out.push(self.ev(at, EventKind::Thinking { text: String::new() }));
        } else if t.starts_with("► **ANSWER**") {
            self.thinking = false;
        } else if t.starts_with("```") {
            self.fence = !self.fence;
        } else if !t.is_empty() && !self.thinking && !self.fence && !self.answered && !t.starts_with('#') {
            self.answered = true;
            out.push(self.ev(at, EventKind::AssistantText { text: clip(t, 160) }));
        }
    }

    /// `> ` lines are aider's own notes: the model, applied edits, commits and token counts.
    fn note(&mut self, n: &str, at: SystemTime, out: &mut Vec<Event>) {
        if let Some(m) = n.strip_prefix("Main model: ").or_else(|| n.strip_prefix("Model: ")) {
            let model = m.split(" with ").next().unwrap_or(m).trim().to_owned();
            out.push(self.ev(at, EventKind::Meta { cwd: None, branch: None, model: Some(model), title: None }));
        } else if let Some(rest) = n.strip_prefix("Tokens: ") {
            out.push(self.ev(at, EventKind::Usage(tokens(rest))));
            out.push(self.ev(at, EventKind::TurnEnd));
        } else if let Some(path) = n.strip_prefix("Applied edit to ") {
            self.tool("edit", path, at, out);
        } else if let Some(c) = n.strip_prefix("Commit ") {
            let msg = c.split_once(' ').map_or(c, |(_, m)| m);
            self.tool("git commit", msg, at, out);
        } else if let Some(cmd) = n.strip_prefix("Running ") {
            self.tool("run", cmd, at, out);
        }
    }

    fn tool(&mut self, name: &str, summary: &str, at: SystemTime, out: &mut Vec<Event>) {
        self.calls += 1;
        let call_id = format!("{}-{}", self.key.id, self.calls);
        out.push(self.ev(at, EventKind::ToolCall { call_id: call_id.clone(), name: name.into(), summary: clip(summary, 90) }));
        out.push(self.ev(at, EventKind::ToolResult { call_id, ok: true }));
    }
}

/// `5.2k sent, 1.1k cache hit, 310 received. Cost: …`
fn tokens(s: &str) -> Usage {
    let s = s.split(". Cost").next().unwrap_or(s);
    let mut u = Usage::default();
    let mut sent = 0;
    for part in s.split(',') {
        let part = part.trim().trim_end_matches('.');
        let Some((num, what)) = part.split_once(' ') else { continue };
        let n = parse_count(num);
        match what {
            "sent" => sent = n,
            "received" => u.output = n,
            "cache hit" => u.cache_read = n,
            "cache write" => u.cache_write = n,
            _ => {}
        }
    }
    // `sent` counts the cached part too
    u.input = sent.saturating_sub(u.cache_read + u.cache_write);
    u
}

fn parse_count(s: &str) -> u64 {
    let (num, mul) = match s.chars().last() {
        Some('k') => (&s[..s.len() - 1], 1e3),
        Some('M') => (&s[..s.len() - 1], 1e6),
        _ => (s, 1.0),
    };
    num.replace(',', "").parse::<f64>().map(|n| (n * mul).round() as u64).unwrap_or(0)
}

impl SourceReader for Reader {
    fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let mut lines = self.lines();
        // no times in the log: the backlog is dated by the file, later lines by the clock
        let at = if self.started { SystemTime::now() } else { self.first_at };
        if !self.started {
            // older sessions in the backlog are over; replay only the newest
            match lines.iter().rposition(|l| l.starts_with("# aider chat started at ")) {
                Some(i) => {
                    lines.drain(..i);
                }
                None if !lines.is_empty() => {
                    self.in_session = true;
                    self.key.id = format!("{}-current", self.base);
                    out.push(self.ev(at, EventKind::Meta { cwd: self.cwd.clone(), branch: None, model: None, title: None }));
                }
                None => {}
            }
        }
        for l in &lines {
            self.line(l, at, &mut out);
        }
        self.started |= !lines.is_empty();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sessions_prompts_edits_and_tokens() {
        let dir = std::env::temp_dir().join(format!("shipoffice-aider-{}", std::process::id()));
        let repo = dir.join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        let p = repo.join(FILE);
        std::fs::write(&p, "\n# aider chat started at 2026-10-17 09:00:00\n\n#### old\n\nok\n\n> Tokens: 1k sent, 10 received.\n\
            \n# aider chat started at 2026-10-18 10:12:03\n\n> Main model: claude-x with diff edit format\n\
            #### add a --verbose flag to cli.py\n#### and test it\n\n► **THINKING**\n\nhmm\n\n► **ANSWER**\n\nI'll add the flag.\n```python\nx\n```\n\n\
            > Tokens: 5.2k sent, 1.1k cache hit, 310 received. Cost: $0.02 message, $0.02 session.\n> Applied edit to cli.py\n> Commit 3a4b5c6 feat: add --verbose\n\n#### next").unwrap();
        let specs = Aider.discover(&[dir.clone()], SystemTime::UNIX_EPOCH);
        assert_eq!(specs.len(), 1, "a folder's direct subfolders are checked");
        let ev = Aider.open(&specs[0]).poll();
        assert!(ev.iter().all(|e| e.session.id.ends_with("2026-10-18101203")), "only the newest session is replayed");
        let last: Vec<_> = ev.iter().map(|e| e.kind.clone()).collect();
        assert!(matches!(&last[0], EventKind::Meta { cwd: Some(c), .. } if *c == repo));
        assert!(last.iter().any(|e| matches!(e, EventKind::Meta { model: Some(m), .. } if m == "claude-x")));
        assert_eq!(last.iter().filter(|e| matches!(e, EventKind::UserPrompt { .. })).count(), 1, "the unfinished last line waits");
        assert!(last.iter().any(|e| matches!(e, EventKind::Thinking { .. })));
        assert!(last.iter().any(|e| matches!(e, EventKind::AssistantText { text } if text == "I'll add the flag.")));
        assert!(last.iter().any(|e| matches!(e, EventKind::Usage(u) if u.input == 4100 && u.cache_read == 1100 && u.output == 310)));
        assert!(last.iter().any(|e| matches!(e, EventKind::ToolCall { name, summary, .. } if name == "edit" && summary == "cli.py")));
        assert!(last.iter().any(|e| matches!(e, EventKind::ToolCall { name, summary, .. } if name == "git commit" && summary == "feat: add --verbose")));
        assert!(last.iter().any(|e| matches!(e, EventKind::TurnEnd)));
        std::fs::remove_dir_all(&dir).ok();
    }
}

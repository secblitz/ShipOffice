//! Cursor agent transcripts: `~/.cursor/projects/<slug>/agent-transcripts/`, append-only JSONL
//! (`<id>.jsonl` from the CLI, `<id>/<id>.jsonl` from the IDE, subagents in
//! `<id>/subagents/<child>.jsonl`). They hold prompts, answers and tool inputs, but no tool
//! results, tokens or per-line times. See docs/harnesses.md §12.
//!
//! The IDE's `state.vscdb` is deliberately not read: it grows to tens of GB and also holds
//! login state.

use super::{clip, home, modified, tool_summary, Adapter, JsonlTail, SourceReader, SourceSpec};
use crate::model::{AgentRef, Event, EventKind, SessionKey};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ID: &str = "cursor";

#[derive(Default)]
pub struct Cursor {
    /// project slug -> the folder it was made from, when the filesystem still has it
    slugs: Mutex<HashMap<String, Option<PathBuf>>>,
}

impl Adapter for Cursor {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        "Cursor"
    }
    fn default_roots(&self) -> Vec<PathBuf> {
        vec![home().join(".cursor")]
    }

    fn discover(&self, roots: &[PathBuf], since: SystemTime) -> Vec<SourceSpec> {
        let mut out = Vec::new();
        for root in roots {
            let Ok(projects) = std::fs::read_dir(root.join("projects")) else { continue };
            for project in projects.flatten() {
                let Ok(entries) = std::fs::read_dir(project.path().join("agent-transcripts")) else { continue };
                for e in entries.flatten() {
                    let p = e.path();
                    let Some(id) = p.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else { continue };
                    if p.is_dir() {
                        let main = p.join(format!("{id}.jsonl"));
                        if let Some(m) = modified(&main).filter(|m| *m >= since) {
                            out.push(spec(&id, AgentRef::Lead, main, m));
                        }
                        let Ok(subs) = std::fs::read_dir(p.join("subagents")) else { continue };
                        for s in subs.flatten() {
                            let sp = s.path();
                            let Some(child) = sp.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else { continue };
                            if sp.extension().is_none_or(|x| x != "jsonl") {
                                continue;
                            }
                            if let Some(m) = modified(&sp).filter(|m| *m >= since) {
                                out.push(spec(&id, AgentRef::Sub(child), sp, m));
                            }
                        }
                    } else if p.extension().is_some_and(|x| x == "jsonl") {
                        if let Some(m) = modified(&p).filter(|m| *m >= since) {
                            out.push(spec(&id, AgentRef::Lead, p, m));
                        }
                    }
                }
            }
        }
        out
    }

    fn open(&self, spec: &SourceSpec) -> Box<dyn SourceReader> {
        let slug = spec
            .path
            .ancestors()
            .find(|a| a.file_name().is_some_and(|n| n == "agent-transcripts"))
            .and_then(Path::parent)
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let cwd = self.slugs.lock().unwrap().entry(slug.clone()).or_insert_with(|| resolve_slug(&slug)).clone();
        Box::new(Reader {
            key: spec.key.clone(),
            agent: spec.agent.clone(),
            tail: JsonlTail::new(spec.path.clone()),
            first_at: modified(&spec.path).unwrap_or_else(SystemTime::now),
            cwd,
            slug,
            started: false,
            calls: 0,
        })
    }
}

fn spec(session: &str, agent: AgentRef, path: PathBuf, modified: SystemTime) -> SourceSpec {
    SourceSpec { key: SessionKey { harness: ID, id: session.to_owned() }, agent, path, modified }
}

/// The slug is the absolute path with its separators turned into `-` (`home-user-my-proj`), which
/// is lossy. Walk the filesystem and take, at each level, the longest run of parts that names an
/// existing folder.
fn resolve_slug(slug: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = slug.split('-').collect();
    let (mut dir, mut i) = if cfg!(windows) {
        // `c-Users-me-proj`
        (PathBuf::from(format!("{}:\\", parts.first()?.to_ascii_uppercase())), 1)
    } else {
        (PathBuf::from("/"), 0)
    };
    while i < parts.len() {
        let next = (i + 1..=parts.len().min(i + 8)).rev().find_map(|j| {
            let name = parts[i..j].join("-");
            // a leading dot is lost too: `-config` was `.config`
            [name.clone(), format!(".{name}")].into_iter().map(|n| dir.join(n)).find(|p| p.is_dir()).map(|p| (p, j))
        });
        let (p, j) = next?;
        dir = p;
        i = j;
    }
    (dir.components().count() > 1).then_some(dir)
}

struct Reader {
    key: SessionKey,
    agent: AgentRef,
    tail: JsonlTail,
    /// lines carry no time: the backlog read at open is dated by the file
    first_at: SystemTime,
    cwd: Option<PathBuf>,
    slug: String,
    started: bool,
    /// tool_use ids are always null; number the calls instead
    calls: u64,
}

impl Reader {
    fn ev(&self, at: SystemTime, kind: EventKind) -> Event {
        Event { session: self.key.clone(), agent: self.agent.clone(), at, kind }
    }

    fn line(&mut self, v: &Value, at: SystemTime, out: &mut Vec<Event>) {
        if v.get("type").and_then(Value::as_str) == Some("turn_ended") {
            out.push(match &self.agent {
                AgentRef::Lead => self.ev(at, EventKind::TurnEnd),
                AgentRef::Sub(id) => self.ev(at, EventKind::SubagentEnd { agent_id: id.clone() }),
            });
            return;
        }
        let blocks = v.pointer("/message/content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        let lead = self.agent == AgentRef::Lead;
        for b in blocks {
            let text = b.get("text").and_then(Value::as_str).unwrap_or("");
            match (v.get("role").and_then(Value::as_str).unwrap_or(""), b.get("type").and_then(Value::as_str).unwrap_or("")) {
                ("user", "text") if lead => {
                    let at = between(text, "<timestamp>", "</timestamp>").and_then(parse_stamp).unwrap_or(at);
                    // the typed prompt sits in <user_query>; the rest is injected context
                    let q = between(text, "<user_query>", "</user_query>").unwrap_or(text);
                    if !q.trim().is_empty() && !q.trim_start().starts_with('<') {
                        out.push(self.ev(at, EventKind::UserPrompt { text: clip(q, 140) }));
                    }
                }
                ("assistant", "text") if !text.trim().is_empty() && text != "[REDACTED]" => out.push(self.ev(at, EventKind::AssistantText { text: clip(text, 160) })),
                ("assistant", "thinking") => {
                    let t = b.get("thinking").or_else(|| b.get("text")).and_then(Value::as_str).unwrap_or("");
                    out.push(self.ev(at, EventKind::Thinking { text: if t == "[REDACTED]" { String::new() } else { clip(t, 140) } }));
                }
                ("assistant", "tool_use") => {
                    self.calls += 1;
                    let name = b.get("name").and_then(Value::as_str).unwrap_or("tool").to_owned();
                    let call_id = b.get("id").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("call-{}", self.calls));
                    out.push(self.ev(at, EventKind::ToolCall { call_id, name, summary: tool_summary(&b["input"]) }));
                }
                _ => {}
            }
        }
    }
}

/// `Monday, Oct 13, 2026, 9:41 AM (UTC+2)`, the stamp Cursor puts in front of each prompt.
fn parse_stamp(s: &str) -> Option<SystemTime> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let f: Vec<&str> = s.split(',').map(str::trim).collect();
    let [_, md, year, rest] = f.as_slice() else { return None };
    let (mon, day) = md.split_once(' ')?;
    let month = MONTHS.iter().position(|m| mon.starts_with(m))? as u8 + 1;
    let (clock, zone) = rest.split_once('(').map(|(c, z)| (c.trim(), z.trim_end_matches(')'))).unwrap_or((rest, "UTC"));
    let (hm, ampm) = clock.split_once(' ').unwrap_or((clock, ""));
    let (h, m) = hm.split_once(':')?;
    let mut hour: i64 = h.parse().ok()?;
    match ampm {
        "PM" if hour < 12 => hour += 12,
        "AM" if hour == 12 => hour = 0,
        _ => {}
    }
    let offset_min = zone.strip_prefix("UTC").or_else(|| zone.strip_prefix("GMT")).map_or(Some(0), |o| {
        if o.is_empty() {
            return Some(0);
        }
        let sign = if o.starts_with('-') { -1 } else { 1 };
        let (oh, om) = o[1..].split_once(':').unwrap_or((&o[1..], "0"));
        Some(sign * (oh.parse::<i64>().ok()? * 60 + om.parse::<i64>().ok()?))
    })?;
    let date = time::Date::from_calendar_date(year.parse().ok()?, time::Month::try_from(month).ok()?, day.parse().ok()?).ok()?;
    let days = (date - time::Date::from_calendar_date(1970, time::Month::January, 1).ok()?).whole_days();
    let secs = days * 86400 + hour * 3600 + m.parse::<i64>().ok()? * 60 - offset_min * 60;
    (secs > 0).then(|| UNIX_EPOCH + Duration::from_secs(secs as u64))
}

fn between<'a>(s: &'a str, a: &str, b: &str) -> Option<&'a str> {
    let start = s.find(a)? + a.len();
    let end = s[start..].find(b)? + start;
    Some(s[start..end].trim())
}

impl SourceReader for Reader {
    fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let lines = self.tail.read();
        if lines.is_empty() {
            return out;
        }
        let at = if self.started { SystemTime::now() } else { self.first_at };
        if !self.started {
            match &self.agent {
                AgentRef::Sub(id) => {
                    out.push(self.ev(at, EventKind::SubagentStart { agent_id: id.clone(), description: "Subagent".into(), kind: None }));
                }
                AgentRef::Lead => {
                    // without a folder to show, the slug's last part still names the project
                    let title = self.cwd.is_none().then(|| self.slug.rsplit('-').next().unwrap_or("").to_owned()).filter(|t| !t.is_empty());
                    out.push(self.ev(at, EventKind::Meta { cwd: self.cwd.clone(), branch: None, model: None, title }));
                }
            }
            self.started = true;
        }
        for v in &lines {
            self.line(v, at, &mut out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_transcripts_and_subagents() {
        let dir = std::env::temp_dir().join(format!("shipoffice-cursor-{}", std::process::id()));
        let t = dir.join("projects/home-user-proj/agent-transcripts");
        std::fs::create_dir_all(t.join("c1/subagents")).unwrap();
        std::fs::write(t.join("c1/c1.jsonl"), [
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<timestamp>Monday, Oct 13, 2026, 9:41 AM (UTC+2)</timestamp>\n<user_query>Explain the parser</user_query>"}]}}"#,
            r#"{"role":"assistant","message":{"content":[{"type":"text","text":"Looking."},{"type":"tool_use","id":null,"name":"Subagent","input":{"description":"Map the code","prompt":"…","subagent_type":"explore"}}]}}"#,
            r#"{"type":"turn_ended","status":"completed"}"#,
        ].join("\n") + "\n").unwrap();
        std::fs::write(t.join("c1/subagents/c2.jsonl"), r#"{"type":"turn_ended","status":"completed"}"#.to_string() + "\n").unwrap();
        std::fs::write(t.join("c3.jsonl"), "").unwrap();

        let c = Cursor::default();
        let specs = c.discover(&[dir.clone()], SystemTime::UNIX_EPOCH);
        assert_eq!(specs.len(), 3);
        let lead = specs.iter().find(|s| s.key.id == "c1" && s.agent == AgentRef::Lead).unwrap();
        let ev = c.open(lead).poll();
        let k: Vec<_> = ev.iter().map(|e| e.kind.clone()).collect();
        assert!(matches!(&k[0], EventKind::Meta { .. }));
        let prompt = ev.iter().find(|e| matches!(&e.kind, EventKind::UserPrompt { text } if text == "Explain the parser")).unwrap();
        assert_eq!(prompt.at, UNIX_EPOCH + Duration::from_secs(1_791_877_260));
        assert!(k.iter().any(|e| matches!(e, EventKind::ToolCall { call_id, name, summary } if call_id == "call-1" && name == "Subagent" && summary == "Map the code")));
        assert!(matches!(k.last(), Some(EventKind::TurnEnd)));
        let sub = specs.iter().find(|s| s.agent != AgentRef::Lead).unwrap();
        assert_eq!(sub.key.id, "c1");
        let k: Vec<_> = c.open(sub).poll().into_iter().map(|e| e.kind).collect();
        assert!(matches!(&k[..], [EventKind::SubagentStart { .. }, EventKind::SubagentEnd { agent_id }] if agent_id == "c2"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stamps_and_slugs() {
        let a = parse_stamp("Monday, Oct 13, 2026, 9:41 AM (UTC+2)").unwrap();
        let b = parse_stamp("Monday, Oct 13, 2026, 7:41 AM (UTC)").unwrap();
        assert_eq!(a, b);
        assert_eq!(parse_stamp("Tuesday, Oct 14, 2026, 12:05 AM (UTC-5:30)").unwrap(), parse_stamp("Tuesday, Oct 14, 2026, 5:35 AM").unwrap());
        let dir = std::env::temp_dir().join(format!("shipoffice-slug-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("my-proj/src")).unwrap();
        let slug = format!("{}-my-proj", dir.to_string_lossy().trim_start_matches('/').replace(['/', '\\'], "-"));
        if !cfg!(windows) {
            assert_eq!(resolve_slug(&slug), Some(dir.join("my-proj")));
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}

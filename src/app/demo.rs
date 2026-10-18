//! Pretend sessions for when nothing is running yet, so a fresh install shows a living office
//! (and so the README video does not need anyone's real work on screen).

use crate::model::{AgentRef, Event, EventKind, SessionKey, Usage};
use crate::office::Rng;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

struct Project {
    path: &'static str,
    branch: &'static str,
    model: &'static str,
    files: &'static [&'static str],
    tasks: &'static [&'static str],
    prompts: &'static [&'static str],
    replies: &'static [&'static str],
}

const PROJECTS: [Project; 3] = [
    Project {
        path: "~/code/shipoffice",
        branch: "feat/procedural-offices",
        model: "claude-opus-5-5",
        files: &["src/office/gen.rs", "src/render/gpu.rs", "src/app/hud.rs", "src/session.rs", "Cargo.toml", "README.md"],
        tasks: &["Write tests for the floor plan", "Audit the shader for sRGB", "Profile the sprite pass", "Port the coffee queue"],
        prompts: &["Make the lounge feel busier", "Why does the couch flicker?", "Add a whiteboard with session stats"],
        replies: &["Done. Two helpers covered tests and docs.", "Fixed: seats now stay claimed while walking.", "The board now shows tokens, cache and context."],
    },
    Project {
        path: "~/code/atlas-api",
        branch: "main",
        model: "gpt-5.3-codex",
        files: &["api/routes/users.ts", "api/db/schema.sql", "api/auth/session.ts", "package.json", "tests/users.spec.ts"],
        tasks: &["Find every caller of getUser", "Draft the migration", "Check the rate limiter"],
        prompts: &["Add pagination to /users", "The login test is flaky", "Rename accounts to workspaces"],
        replies: &["Pagination is in, with cursor tests.", "Race in the session cache, now awaited.", "Renamed across 14 files; migration drafted."],
    },
    Project {
        path: "~/code/garden-notes",
        branch: "docs/onboarding",
        model: "gemini-3-pro",
        files: &["docs/getting-started.md", "site/config.yaml", "site/theme.css"],
        tasks: &["Proofread the guide"],
        prompts: &["Rewrite the onboarding guide"],
        replies: &["The guide is shorter and every step is tested."],
    },
];

const TOOLS: [(&str, usize); 7] = [("Read", 4), ("Grep", 2), ("Edit", 3), ("Bash", 2), ("Glob", 1), ("Write", 1), ("WebSearch", 1)];

#[derive(Clone)]
enum Phase {
    Idle(f32),
    Working { left: f32, spawned: Vec<(String, f32)>, next_tool: f32 },
}

struct Live {
    p: usize,
    id: String,
    phase: Phase,
    turn: usize,
    sub_n: u32,
}

pub struct Demo {
    rng: Rng,
    live: Vec<Live>,
}

impl Demo {
    pub fn new(seed: u32) -> Self {
        let live = (0..PROJECTS.len()).map(|p| Live { p, id: format!("demo-{p}"), phase: Phase::Idle(1.0 + p as f32 * 6.0), turn: 0, sub_n: 0 }).collect();
        Self { rng: Rng::new(seed), live }
    }

    pub fn is_demo(key: &SessionKey) -> bool {
        key.harness == "demo"
    }

    /// The session's opening lines: where it runs, on which branch and model.
    pub fn intro(&self) -> Vec<Event> {
        let now = SystemTime::now();
        self.live
            .iter()
            .map(|l| {
                let p = &PROJECTS[l.p];
                ev(&l.id, AgentRef::Lead, now - Duration::from_secs(60 * (3 + l.p as u64 * 7)), EventKind::Meta { cwd: Some(PathBuf::from(p.path)), branch: Some(p.branch.into()), model: Some(p.model.into()), title: None })
            })
            .collect()
    }

    pub fn step(&mut self, dt: f32) -> Vec<Event> {
        let mut out = Vec::new();
        let now = SystemTime::now();
        for l in &mut self.live {
            let p = &PROJECTS[l.p];
            let r = &mut self.rng;
            match &mut l.phase {
                Phase::Idle(left) => {
                    *left -= dt;
                    if *left <= 0.0 {
                        let prompt = p.prompts[l.turn % p.prompts.len()];
                        out.push(ev(&l.id, AgentRef::Lead, now, EventKind::UserPrompt { text: prompt.into() }));
                        out.push(ev(&l.id, AgentRef::Lead, now, EventKind::Thinking { text: "Planning the change".into() }));
                        // bigger projects bring helpers along
                        let n = if p.tasks.len() > 1 { 1 + r.below(p.tasks.len().min(3)) } else { r.below(2) };
                        let mut spawned = Vec::new();
                        for k in 0..n {
                            l.sub_n += 1;
                            let id = format!("{}-sub{}", l.id, l.sub_n);
                            let desc = p.tasks[(l.turn + k) % p.tasks.len()];
                            out.push(ev(&l.id, AgentRef::Lead, now, EventKind::SubagentStart { agent_id: id.clone(), description: desc.into(), kind: Some("general-purpose".into()) }));
                            spawned.push((id, 14.0 + r.next() * 22.0));
                        }
                        l.phase = Phase::Working { left: 34.0 + r.next() * 26.0, spawned, next_tool: 2.0 };
                    }
                }
                Phase::Working { left, spawned, next_tool } => {
                    *left -= dt;
                    *next_tool -= dt;
                    if *next_tool <= 0.0 {
                        *next_tool = 1.6 + r.next() * 2.4;
                        // the lead and each live helper take turns using tools
                        let who: Vec<AgentRef> = std::iter::once(AgentRef::Lead).chain(spawned.iter().map(|(id, _)| AgentRef::Sub(id.clone()))).collect();
                        let a = who[r.below(who.len())].clone();
                        let total: usize = TOOLS.iter().map(|t| t.1).sum();
                        let mut k = r.below(total);
                        let tool = TOOLS.iter().find(|t| {
                            if k < t.1 {
                                true
                            } else {
                                k -= t.1;
                                false
                            }
                        });
                        let tool = tool.map_or("Read", |t| t.0);
                        let file = p.files[r.below(p.files.len())];
                        let summary = match tool {
                            "Bash" => (*r.pick(&["cargo test", "npm run lint", "git status", "cargo clippy"])).to_string(),
                            "Grep" => format!("\"{}\"", r.pick(&["fn generate", "TODO", "getUser", "session"])),
                            "WebSearch" => "wgpu premultiplied alpha".into(),
                            _ => file.into(),
                        };
                        out.push(ev(&l.id, a.clone(), now, EventKind::ToolCall { call_id: format!("{}", r.below(1 << 30)), name: tool.into(), summary }));
                        let usage = Usage { input: 800 + r.below(2400) as u64, output: 120 + r.below(900) as u64, cache_read: 9000 + r.below(30000) as u64, cache_write: r.below(3000) as u64 };
                        out.push(ev(&l.id, a.clone(), now, EventKind::Usage(usage)));
                        if r.next() < 0.18 {
                            out.push(ev(&l.id, a, now, EventKind::Thinking { text: (*r.pick(&["Checking the edge cases", "That path looks unused", "Tests first, then the fix"])).into() }));
                        }
                    }
                    for (id, t) in spawned.iter_mut() {
                        *t -= dt;
                        if *t <= 0.0 {
                            out.push(ev(&l.id, AgentRef::Sub(id.clone()), now, EventKind::AssistantText { text: (*r.pick(&["Done, all green", "Found three callers", "Draft is ready", "Looks good to me"])).into() }));
                            out.push(ev(&l.id, AgentRef::Lead, now, EventKind::SubagentEnd { agent_id: id.clone() }));
                        }
                    }
                    spawned.retain(|(_, t)| *t > 0.0);
                    if *left <= 0.0 && spawned.is_empty() {
                        out.push(ev(&l.id, AgentRef::Lead, now, EventKind::AssistantText { text: p.replies[l.turn % p.replies.len()].into() }));
                        out.push(ev(&l.id, AgentRef::Lead, now, EventKind::TurnEnd));
                        l.turn += 1;
                        l.phase = Phase::Idle(30.0 + r.next() * 40.0);
                    }
                }
            }
        }
        out
    }
}

fn ev(id: &str, agent: AgentRef, at: SystemTime, kind: EventKind) -> Event {
    Event { session: SessionKey { harness: "demo", id: id.to_owned() }, agent, at, kind }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_runs_whole_turns() {
        let mut d = Demo::new(1);
        let mut kinds = std::collections::HashSet::new();
        for _ in 0..(60 * 180) {
            for e in d.step(1.0 / 60.0) {
                kinds.insert(std::mem::discriminant(&e.kind));
            }
        }
        assert!(kinds.len() >= 6, "prompts, tools, usage, spawns, ends and replies all happen");
    }
}

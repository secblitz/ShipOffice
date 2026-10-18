# Agent harness session storage: reference for a passive visualizer

Researched 2026-10-18. Sections marked **[local]** were checked against real data on this machine (Linux, read-only; SQLite opened with `?mode=ro`). Sections marked **[web]** come from upstream source code and docs, with links. Examples are redacted: long strings are cut off with `…`, home directories are shown as `/home/user`, and account ids, emails, keys and file contents are removed.

## Summary

| # | Harness | Paths (Linux / macOS / Windows) | Format | Tail strategy | Confidence |
|---|---|---|---|---|---|
| 1 | **Claude Code** | `~/.claude/projects/<enc-cwd>/<sid>.jsonl` and `<sid>/subagents/agent-<id>.jsonl` on all three (`%USERPROFILE%\.claude`). `CLAUDE_CONFIG_DIR` | Append-only JSONL (plus `sessions/<pid>.json` live status) | Byte offset. Dedupe usage by `message.id`. Turn end = `system/turn_duration` | **High** (local) |
| 2 | **Codex CLI** | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` (`%USERPROFILE%\.codex`). `CODEX_HOME`, `CODEX_SQLITE_HOME` | Append-only JSONL, plus a SQLite WAL index (`state_*.sqlite`) | Byte offset. Turn end = `event_msg task_complete` (≥ v0.100) | **High** (local) |
| 3 | **OpenCode** | `~/.local/share/opencode/opencode.db` on **all** OSes (xdg-basedir). `XDG_DATA_HOME`, `OPENCODE_DB` | SQLite WAL (v2: `session_v2`/`session_message`; v1: `session`/`message`/`part`). Legacy ≤1.1: JSON tree | `mode=ro`. Watch `-wal`, re-query rows by `time_updated`/`seq`. Turn end = `idle` row | **High** (local) |
| 4 | **Gemini CLI** | `~/.gemini/tmp/<slug>/chats/session-*.jsonl` (all OSes). `GEMINI_CLI_HOME` | JSONL event log with `$set`/`$patch`/`$rewindTo`, messages re-appended by `id`. Legacy: rewritten `.json` | Byte offset plus upsert by `id`. No turn-end marker | High (source) |
| 5 | **Qwen Code** | `~/.qwen/projects/<enc-cwd>/chats/<sid>.jsonl`, `subagents/<sid>/agent-*.jsonl`. `QWEN_HOME`, `QWEN_RUNTIME_DIR` | Append-only JSONL (Claude-Code-like) plus `.runtime.json` | Byte offset. `tool_lifecycle` events. `turn_result` only in ACP/serve | High (source) |
| 6 | **GitHub Copilot CLI** | `~/.copilot/session-state/<sid>/events.jsonl` (all OSes). `COPILOT_HOME` | Append-only JSONL events plus `workspace.yaml` | Byte offset. `assistant.turn_end`. idle/usage events are ephemeral (not on disk) | Medium-high (SDK schema) |
| 7 | **Cline** | legacy: `<globalStorage>/saoudrizwan.claude-dev/tasks/<id>/*.json`. SDK: `~/.cline/data/sessions/<sid>/*.json` plus `db/sessions.db`. `CLINE_DIR`, `CLINE_DATA_DIR` | Whole-file JSON (legacy: tmp+rename; SDK: in place) plus SQLite WAL | Re-read on change, diff by `ts`. Waiting = trailing non-partial `ask` | High (source) |
| 8a | **Roo Code** (discontinued) | `<globalStorage>/rooveterinaryinc.roo-cline/tasks/<id>/*.json`. `roo-cline.customStoragePath` | Whole-file JSON (backup+rename, lockdir) | Re-read on change, tolerate ENOENT | High (source) |
| 8b | **Kilo Code 7.x** | `~/.local/share/kilo/kilo.db` (all OSes). `KILO_DB`. Pre-7: `<globalStorage>/kilocode.kilo-code/tasks/` | SQLite WAL (OpenCode schema) | Same as OpenCode | High (source) |
| 9 | **Goose** | `~/.local/share/goose/sessions/sessions.db` (Linux **and** macOS); `%APPDATA%\Block\goose\data\sessions\sessions.db`. `GOOSE_PATH_ROOT` | SQLite WAL (legacy <1.10: JSONL) | `messages.id > last`, dedupe on `message_id` | High (source) |
| 10 | **Aider** | `<repo>/.aider.chat.history.md` (no global index). `AIDER_CHAT_HISTORY_FILE` | Append-only Markdown | Byte offset, but the repos have to be found first. Little structure | High (format) / low (data richness) |
| 11 | **Crush** | `<project>/.crush/crush.db`. Index `~/.local/share/crush/projects.json` / `%LOCALAPPDATA%\crush\projects.json`. `CRUSH_GLOBAL_DATA` | SQLite WAL, rows updated in place | Poll `updated_at >= wm`, diff parts. Turn end = `finish` part `end_turn` | High (source) |
| 12 | **Cursor** | transcripts `~/.cursor/projects/<slug>/agent-transcripts/*.jsonl` (all OSes). IDE `…/Cursor/User/globalStorage/state.vscdb` | JSONL (no tool outputs, no timestamps, no tokens), plus a huge SQLite KV | Tail transcripts (`turn_ended`). KV lookups by key only | Medium / medium-low |
| 13 | **Amp** | `~/.local/share/amp/threads/T-*.json` (also `~/.config/amp`) | Whole-file JSON, **often server-side only** | Re-read on change, best effort | Low-medium |
| 14 | **Factory Droid** | `~/.factory/sessions/<enc-cwd>/<sid>.jsonl` plus `.settings.json` (all OSes). `FACTORY_HOME_OVERRIDE` | Append-only JSONL plus a rewritten JSON sidecar (tokens) | Byte offset. No turn-end marker | Medium |

**Feasible to tail passively and richly:** Claude Code, Codex, OpenCode, Kilo 7, Qwen Code, Gemini CLI, Copilot CLI, Cline, Goose, Crush, Factory Droid, Cursor transcripts.
**Limited:** Aider (Markdown, no tool structure or timing), Cursor IDE KV (reverse-engineered and huge), Roo Code (works, but discontinued).
**Not reliably feasible:** Amp (current versions keep threads server-side) and cursor-agent `store.db` (undocumented protobuf).

**Explicit turn-finished signal on disk:** Claude Code (`turn_duration`), Codex ≥0.100 (`task_complete`), OpenCode v2 (`idle`), Copilot (`assistant.turn_end`), Crush (`finish.reason`), Cursor transcripts (`turn_ended`), Cline (trailing `ask`), Qwen (ACP/serve only). Gemini, Goose, Droid and Aider need heuristics, or the optional hooks.

## Cross-cutting implementation notes

1. **JSONL tailing.** Keep `(path, inode/file-id, byte_offset)` per file. Read from the offset to EOF and split on `\n`. If the last fragment has no trailing newline, keep it for the next read. If the file shrinks or the inode changes, re-read from 0, because tools do rotate and rewrite files. Single lines can be **several MB** (Claude Code `api-request-blob`, Codex `compacted.replacement_history`), so don't use a fixed-size line buffer.
2. **Rewritten JSON files** (Cline/Roo/Kilo, Gemini CLI, Copilot's older format, Claude Code's `sessions/<pid>.json`): debounce filesystem events (about 150–300 ms) and re-parse the whole file. Tolerate a parse error mid-write and retry on the next event. Diff against the previous parse by message index or id, so only new items are animated.
3. **SQLite in WAL mode** (OpenCode, Codex state, Goose, Crush, Cursor): open with `file:<path>?mode=ro` (rusqlite: `OpenFlags::SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_URI`). **Never** use `immutable=1` on a live database, because you would read stale or torn pages. WAL lets readers and the writer run at the same time. Keep read transactions short, because a long-lived read snapshot blocks checkpoints and lets the `-wal` file grow without limit. Detect changes by watching the `-wal` file's mtime/size, or by polling `PRAGMA data_version` (cheap) on a persistent connection, then re-query by `rowid`/`seq`/`time_updated` cursor. Opening read-only still needs the `-shm` file to be readable and writable by your user. That holds when the visualizer runs as the same user.
4. **Secrets live next to the logs.** `~/.claude/.credentials.json`, `~/.codex/auth.json`, `~/.local/share/opencode/auth.json` / `mcp-auth.json`, and **inside `opencode.db`** the `account`, `control_account`, `credential` and `session_share.secret` columns. Whitelist the paths and tables you read rather than blacklisting the dangerous ones.
5. **Unknown record types are normal.** Every harness adds record types often. The local Claude Code 2.1.30x data has more than 30 `type` values. Dispatch on a small known set and ignore the rest.

---

## 1. Claude Code (Anthropic) [local, v2.1.300–2.1.302]

### Paths

| OS | Default |
|---|---|
| Linux | `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl` |
| macOS | `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl` |
| Windows | `%USERPROFILE%\.claude\projects\<encoded-cwd>\<sessionId>.jsonl` (for example `C--Users-me-src-app`) |

Override: **`CLAUDE_CONFIG_DIR`** replaces `~/.claude` as the root (the string is present in the binary; docs: https://docs.anthropic.com/en/docs/claude-code/settings#environment-variables). Watch both the default root and `$CLAUDE_CONFIG_DIR` if it is set. There is no XDG handling.

Other files under the root that are useful:

| Path | Shape | Use |
|---|---|---|
| `projects/<enc>/<sessionId>.jsonl` | append-only JSONL | main transcript |
| `projects/<enc>/<sessionId>/subagents/agent-<agentId>.jsonl` | append-only JSONL | subagent (sidechain) transcript, current layout |
| `projects/<enc>/<sessionId>/subagents/agent-<agentId>.meta.json` | small JSON | links the subagent to the parent `toolUseId`, `agentType`, `description`, `model`, team `name`, worktree |
| `projects/<enc>/<sessionId>/subagents/workflows/wf_<id>/agent-*.jsonl` | JSONL | agents spawned by the Workflow tool |
| `projects/<enc>/<sessionId>/tool-results/*` | raw files | large tool outputs moved out of the transcript |
| `projects/<enc>/agent-<agentId>.jsonl` | JSONL | **older layout** (2.0.x): sidechain files sat next to the session file. Before that, sidechain records were inline in the main file with `isSidechain: true`. Handle all three. |
| `sessions/<pid>.json` | small JSON, rewritten | **live session registry**: `{pid, sessionId, cwd, startedAt, version, kind:"interactive", entrypoint:"cli", status:"busy"\|"idle"\|"waiting", waitingFor:null\|"dialog open"…, updatedAt, statusUpdatedAt, name}`. This is the best passive "is it running / waiting for me" signal. Check that the pid is alive. The `sessions/<pid>.*.key` files beside it hold peer tokens: never read them. |
| `history.jsonl` | append-only JSONL | prompt history `{display, pastedContents, project, sessionId, timestamp}`. `project` is the real cwd. |
| `teams/<team>/config.json`, `teams/<team>/inboxes/` | JSON | agent-team (teammate) metadata |

### Project mapping

The folder name is the cwd with every non-alphanumeric character replaced by `-`. For example `/home/u/a.b/c` becomes `-home-u-a-b-c`, and `/.tools` becomes `--tools`. The encoding is **lossy**, so don't decode it. Every transcript record has a `cwd` field, so use the `cwd` of the first `user`/`assistant` record. `cwd` can change during a session (for example after `cd` into a subfolder), so treat the first value as the project and later values as the current directory. `gitBranch` is also on every record.

### Storage shape and tail strategy

Append-only JSONL, one JSON object per line. Tail by byte offset. Old sessions are deleted by cleanup (`cleanupPeriodDays`, default 30). New subagent files appear while a session runs, so watch `projects/**` recursively.

**Important:** one API response is written as **one line per content block**. Each line carries the same `message.id` and `requestId`, and the **same `usage` object is repeated on every line**. Dedupe usage by `message.id`, or you will over-count 2–5×.

### Record types that matter (all others can be ignored)

Common envelope on transcript records: `type, uuid, parentUuid, timestamp (ISO), sessionId, cwd, gitBranch, version, isSidechain, userType, entrypoint ("cli"|"sdk-ts"|…), slug`. Subagent files add `agentId`.

| Need | Where |
|---|---|
| session id | `sessionId` (equals the file stem; subagent files carry the **parent's** sessionId plus `agentId`) |
| timestamp | `timestamp` |
| cwd | `cwd` |
| user prompt | `type:"user"` whose `message.content` is a **string**, or an array of `{type:"text"}`/`{type:"image"}` blocks, and **not** `isMeta:true`. A real human prompt has `origin.kind:"human"`, `promptSource:"typed"`, `turnOrigin:"human"`. Strings that start with `<task-notification>`, `<command-name>` or `<local-command-stdout>` are system-injected. |
| assistant text | `type:"assistant"`, `message.content[].type=="text"` → `.text` |
| thinking | `message.content[].type=="thinking"` → `.thinking`. It is often `""` with only a `signature` when thinking display is off. `redacted_thinking` also exists. `thinkingDurationMs` sits on the envelope. |
| tool call | `message.content[].type=="tool_use"` → `{id:"toolu_…", name, input}` |
| tool result | `type:"user"`, `message.content[].type=="tool_result"` → `{tool_use_id, content, is_error}`. **Link: `tool_use_id == tool_use.id`.** The envelope also has `sourceToolAssistantUUID` (the uuid of the assistant line with the call) and `toolUseResult` (structured result, for example Bash `{stdout,stderr,interrupted}`). |
| subagent spawn | `tool_use` with `name` `"Agent"` (named `"Task"` in older versions) and `input {description, prompt, subagent_type, name?, model?}`. Its `tool_result.toolUseResult` gives `{status:"async_launched", agentId, outputFile, description, resolvedModel}` or `{status:"teammate_spawned", agentId, name, team_name, …}`. The transcript is `<sessionId>/subagents/agent-<agentId>.jsonl` and its `.meta.json.toolUseId` equals the spawning `tool_use.id`. |
| subagent completion | **async (current default):** a later `type:"user"` record with `origin.kind:"task-notification"` and `turnOrigin:"task_notification"`. Its string content is `<task-notification><task-id>{agentId}</task-id><tool-use-id>toolu_…</tool-use-id><output-file>…</output-file><status>completed\|failed\|killed</status><summary>Agent "…" finished</summary><result>…</result><usage>…</usage></task-notification>`. The same notification also shows up as `queue-operation` (`enqueue`/`remove`) and `attachment.type:"queued_command"` records. Dedupe on `task-id` + status. An agent can notify more than once if it is resumed. **sync (older):** the Agent/Task `tool_result` holds the final report, and `toolUseResult` has `{status:"completed", content, totalDurationMs, totalTokens, totalToolUseCount, usage}`. Fallback: the subagent file's last `assistant` line has `stop_reason:"end_turn"`. |
| tokens | `message.usage {input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens, cache_creation{ephemeral_5m_input_tokens, ephemeral_1h_input_tokens}, output_tokens_details{thinking_tokens}, service_tier}`. Dedupe by `message.id`. Session totals are in `type:"cost-state"` → `modelUsage[model]{inputTokens, outputTokens, thinkingTokens, cacheReadInputTokens, cacheCreationInputTokens, costUSD}`, `totalCostUSD`. |
| model | `message.model` (`requestedModel` and `advisorModel` also sit on the envelope) |
| turn finished | (a) last assistant line of the turn has `message.stop_reason:"end_turn"` (`tool_use` means more is coming; also `refusal`, `stop_sequence`, `max_tokens`). (b) **`type:"system", subtype:"turn_duration"` `{durationMs, messageCount}` is written when the turn ends**, which makes it the cleanest "turn done" marker. `subtype:"stop_hook_summary"` precedes it. |
| waiting for user | `sessions/<pid>.json` `status:"waiting"` (permission dialog or question). In the transcript, a pending `tool_use` named `AskUserQuestion` (or `ExitPlanMode`) with no `tool_result` yet. `status:"idle"` with no live subagents means waiting for the next prompt. |
| compaction | `type:"system", subtype:"compact_boundary"` `{compactMetadata{trigger, preTokens, postTokens}}`. `parentUuid` restarts at null. |
| titles | `type:"ai-title"` `{aiTitle}`, `type:"custom-title"` `{customTitle}`, `type:"agent-name"` |

### Examples (redacted)

```jsonc
// user prompt
{"type":"user","uuid":"2ee1…","parentUuid":"56c5…","isSidechain":false,"sessionId":"1822…","timestamp":"2026-10-15T17:41:36.996Z",
 "cwd":"/home/user/proj","gitBranch":"HEAD","version":"2.1.300","entrypoint":"cli",
 "message":{"role":"user","content":"Resume proper work from…"},
 "origin":{"kind":"human"},"promptSource":"typed","turnOrigin":"human","turnPosition":{"promptIndex":1,"turnIndex":1},"permissionMode":"auto"}

// assistant thinking / text / tool_use: three lines, same message.id and same usage
{"type":"assistant","uuid":"282e…","parentUuid":"6663…","timestamp":"…","requestId":"req_011C…","thinkingDurationMs":695,
 "message":{"id":"msg_011Cg4SA…","model":"claude-opus-5-5","role":"assistant","stop_reason":"tool_use",
  "content":[{"type":"thinking","thinking":"","signature":"<sig>"}],
  "usage":{"input_tokens":2,"cache_creation_input_tokens":26144,"cache_read_input_tokens":22794,"output_tokens":365,
           "output_tokens_details":{"thinking_tokens":77},"cache_creation":{"ephemeral_1h_input_tokens":26144,"ephemeral_5m_input_tokens":0}}}, "...":"…"}
{"type":"assistant","message":{"id":"msg_011Cg4SA…","content":[{"type":"text","text":"I'll start by reading…"}],"usage":{"…":"same"}}, "...":"…"}
{"type":"assistant","uuid":"7dd4…","message":{"id":"msg_011Cg4SA…","stop_reason":"tool_use",
  "content":[{"type":"tool_use","id":"toolu_01Ld2d…","name":"Bash","input":{"command":"ls && …","description":"List project files"}}]}, "...":"…"}

// tool result
{"type":"user","uuid":"24ca…","parentUuid":"7dd4…","sourceToolAssistantUUID":"7dd4…",
 "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_01Ld2d…","content":"Art\nDocs\n…","is_error":false}]},
 "toolUseResult":{"stdout":"…","stderr":"","interrupted":false,"isImage":false}, "...":"…"}

// subagent spawn: Agent tool_use, then its result
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_01AvgC…","name":"Agent",
  "input":{"name":"w7-gameplay","description":"Wave 7 gameplay implementer","subagent_type":"general-purpose","prompt":"You are…"}}]}, "...":"…"}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_01AvgC…","content":[{"type":"text","text":"Spawned successfully…"}]}]},
 "toolUseResult":{"status":"async_launched","agentId":"a84f8…","description":"…","outputFile":"…/tasks/a84f….output","resolvedModel":"claude-opus-5-5","isAsync":true}}

// subagents/agent-a84f8….meta.json
{"agentType":"general-purpose","description":"Submit and download sprite batches","toolUseId":"toolu_01X1pQ…","spawnDepth":1,"requestShape":"background","requestNonInteractive":true}

// first line of subagents/agent-<id>.jsonl
{"type":"user","isSidechain":true,"agentId":"a84f8…","parentUuid":null,"sessionId":"<parent session id>","cwd":"/home/user/proj/raw","promptId":"…","message":{"role":"user","content":"…"}}

// subagent completion in the parent
{"type":"user","origin":{"kind":"task-notification","producer":"session-task"},"promptSource":"system","turnOrigin":"task_notification",
 "message":{"role":"user","content":"<task-notification>\n<task-id>a7832…</task-id>\n<tool-use-id>toolu_0191…</tool-use-id>\n<output-file>…</output-file>\n<status>completed</status>\n<summary>Agent \"…\" finished</summary>\n<result>…</result>…"}}

// turn end
{"type":"system","subtype":"turn_duration","durationMs":1137809,"messageCount":315,"timestamp":"2026-10-15T18:00:34.785Z","sessionId":"1822…","isSidechain":false}

// sessions/<pid>.json (rewritten in place)
{"pid":2502416,"sessionId":"a6b9…","cwd":"/home/user/proj","version":"2.1.302","kind":"interactive","entrypoint":"cli","status":"busy","waitingFor":null,"updatedAt":1792288098821,"name":"…"}
```

### Hooks (optional fast channel)

Configured in `settings.json` under `hooks`. Docs: https://docs.anthropic.com/en/docs/claude-code/hooks. Event names found in the 2.1.302 binary: `SessionStart, SessionEnd, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, PermissionRequest, PermissionDenied, Notification, Stop, StopFailure, SubagentStart, SubagentStop, PreCompact, PostCompact, TaskCreated, TaskCompleted, TeammateIdle, CwdChanged, FileChanged, ConfigChange, InstructionsLoaded, WorktreeCreate, Elicitation`. Each hook gets JSON on stdin with `session_id`, `transcript_path`, `cwd` and `hook_event_name`, so a tiny hook can `echo` that into a socket for the visualizer. The visualizer must not depend on it.

**Confidence: high** (local data plus binary strings). The record set changes quickly between versions. Expect `Task`→`Agent` and all three sidechain layouts in the wild.

---

## 2. OpenAI Codex CLI [local, v0.162; legacy notes from source]

### Paths

| OS | Default |
|---|---|
| Linux / macOS | `~/.codex/sessions/YYYY/MM/DD/rollout-YYYY-MM-DDTHH-MM-SS-<threadId>.jsonl` |
| Windows | `%USERPROFILE%\.codex\sessions\YYYY\MM\DD\rollout-…jsonl` |

Overrides: **`CODEX_HOME`** replaces `~/.codex`. **`CODEX_SQLITE_HOME`** or config `sqlite_home` moves the SQLite state databases (both strings are in the binary). Archived threads move to `$CODEX_HOME/archived_sessions/`. The date folder is the thread's **creation** date: a resumed old thread keeps appending to an old folder, so watch `sessions/**` recursively, or discover files via `state_*.sqlite threads.rollout_path`. Thread ids are UUIDv7 (time-ordered).

Other files in `$CODEX_HOME`:

| File | Shape | Use |
|---|---|---|
| `state_<N>.sqlite` (here `state_5`) | SQLite, **WAL** | `threads` table: `id, rollout_path, cwd, title, model, reasoning_effort, source, thread_source ("user"\|"subagent"), agent_nickname, agent_path, tokens_used, first_user_message, created_at_ms, updated_at_ms, archived, git_branch, git_sha, cli_version`. `thread_spawn_edges(parent_thread_id, child_thread_id, status)`. This is the best discovery index. **The `<N>` suffix changes with migrations, so glob `state_*.sqlite` and pick the highest N.** |
| `thread_history_<N>.sqlite` | SQLite, WAL | projection of the rollouts: `thread_turns(thread_id, turn_id, status, started_at, completed_at, duration_ms, rollout_byte_offset, rollout_end_byte_offset)` and `thread_items(thread_id, turn_id, item_id, item_type, item_json, created_at_ms, completed_at_ms)`. `item_type` values: `userMessage, agentMessage, reasoning, commandExecution, fileChange, mcpToolCall, webSearch, imageView, subAgentActivity, collabAgentToolCall, contextCompaction, sleep`. |
| `logs_<N>.sqlite` | SQLite, WAL | tracing logs `(ts, level, target, thread_id, feedback_log_body)`. Noisy; skip. |
| `session_index.jsonl` | append JSONL | `{id, thread_name, updated_at}` |
| `history.jsonl` | append JSONL | prompt history `{session_id, ts, text}` |
| `auth.json` | **secret** | never read |

### Project mapping

`session_meta.payload.cwd` (first line) and `turn_context.payload.cwd` (one per turn, can change). The SQLite `threads.cwd` holds the same value. File names carry no project information.

### Storage shape and tail strategy

Append-only JSONL. Every line is `{timestamp, ordinal, type, payload}` (`ordinal` is new; older files lack it). Tail by byte offset, starting with the newest rollout files and the `updated_at_ms` from `threads`.

Top-level `type` values seen locally: `session_meta, turn_context, response_item, event_msg, token_usage_record, compacted, world_state, inter_agent_communication_metadata`. The legacy `RolloutItem` enum in `codex-rs/protocol/src/protocol.rs` has `session_meta, response_item, compacted, turn_context, event_msg`.

| Need | Where (current 0.16x) | Legacy (≤0.5x, still in older files) |
|---|---|---|
| session id | `session_meta.payload.id` (equals the file-name uuid). `payload.session_id` is the root session for subagents. | `session_meta.payload.id` |
| timestamp | line `timestamp` (ISO) | same |
| cwd | `session_meta.payload.cwd`, `turn_context.payload.cwd` | same |
| user prompt | `response_item` `payload.type:"message", role:"user", content[{type:"input_text", text}]`, or `event_msg item_completed` with `item.type:"UserMessage"`. `role:"developer"` and `user` messages starting with `<environment_context>` / `<user_instructions>` are injected context. | `event_msg` `payload.type:"user_message"` `{message}` |
| assistant text | `response_item` `message role:"assistant"` `content[{type:"output_text", text}]` with `phase:"commentary"\|"final_answer"`, or `item_completed AgentMessage` | `event_msg agent_message {message}` |
| reasoning | `response_item type:"reasoning"` `{summary:[{type:"summary_text",text}], encrypted_content}`. **Usually encrypted with an empty summary**: show a "thinking…" state without text. `item_completed Reasoning {summary_text, raw_content}`. | `event_msg agent_reasoning {text}` / `agent_reasoning_raw_content` |
| tool call | `response_item type:"function_call"` `{name, namespace?, arguments (JSON string), call_id}`. `type:"custom_tool_call"` `{name:"exec", input (string), call_id, status}`. Older types: `local_shell_call {action{command[]}, call_id}`, `web_search_call`. Richer view: `event_msg item_completed` with `item.type` `CommandExecution {command[], cwd, exit_code, aggregated_output, duration}`, `FileChange {changes{path:{type, unified_diff}}}`, `McpToolCall {server, tool, arguments, result, status}`, `Extension` (web search), `ImageView`. | `event_msg exec_command_begin/exec_command_end`, `patch_apply_begin/end`, `mcp_tool_call_begin/end` |
| tool result | `response_item type:"function_call_output"` / `"custom_tool_call_output"` `{call_id, output}`. **Link by `call_id`.** | same |
| subagent spawn | `function_call` `namespace:"collaboration", name:"spawn_agent"` `{arguments:{task_name, fork_turns, message(encrypted)}}`. Its output is `{"task_name":"/root/<name>"}`. The child thread gets its own rollout file whose `session_meta.payload.source.subagent.thread_spawn = {parent_thread_id, depth, agent_path, agent_nickname}` and `thread_source:"subagent"`. Also `state_*.sqlite thread_spawn_edges`. Other collaboration tools: `send_message, followup_task, list_agents, wait_agent`. Parent-side activity: `item_completed SubAgentActivity {kind:"started"\|"interacted"\|"completed", agent_thread_id, agent_path}`. | (no multi-agent) |
| subagent completion | `SubAgentActivity kind:"completed"` in the parent, and/or `task_complete` in the child rollout | n/a |
| tokens | `type:"token_usage_record"` `{usage, turn_token_usage, thread_token_usage}` each holding `{input_tokens, cached_input_tokens, cache_write_input_tokens, output_tokens, reasoning_output_tokens, total_tokens}`. Also `event_msg token_count {info{total_token_usage, last_token_usage, model_context_window}, rate_limits}`. | `event_msg token_count` (same `info` shape) |
| model | `turn_context.payload.model`, `.effort`. `event_msg thread_settings_applied.thread_settings.model` | `turn_context.model` |
| turn start | `event_msg task_started {turn_id, started_at, model_context_window}` | same |
| **turn finished** | **`event_msg task_complete {turn_id, last_agent_message, duration_ms, completed_at}`** | `task_complete`; `turn_aborted {reason}` on interrupt |
| waiting for user | `function_call name:"request_user_input_async"` with no output yet. Approval prompts are only visible through `notify` (`approval-requested`) or hooks. | `exec_approval_request` event_msg |

### Examples (redacted)

```jsonc
{"timestamp":"2026-10-09T14:01:21.664Z","ordinal":0,"type":"session_meta","payload":{"id":"01a120f7-…","session_id":"01a1207f-…","parent_thread_id":"01a1207f-…",
  "cwd":"/home/user/proj","originator":"codex-tui","cli_version":"0.162.0","model_provider":"openai","thread_source":"subagent",
  "source":{"subagent":{"thread_spawn":{"parent_thread_id":"01a1207f-…","depth":1,"agent_path":"/root/performance_ui","agent_nickname":"Ramanujan"}}},
  "git":{"commit_hash":"3476…","branch":"main"},"base_instructions":{"text":"You are Codex…"},"creator_user_id":"<redacted>"}}
{"timestamp":"…","ordinal":4,"type":"turn_context","payload":{"turn_id":"01a120b0-…","cwd":"/home/user/proj","model":"gpt-6-astra","effort":"xhigh","approval_policy":"never","sandbox_policy":{"type":"danger-full-access"}}}
{"timestamp":"…","ordinal":7,"type":"event_msg","payload":{"type":"task_started","turn_id":"01a120f7-…","started_at":1791554443,"model_context_window":258400}}
{"timestamp":"…","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Install it locally here"}]}}
{"timestamp":"…","type":"response_item","payload":{"type":"reasoning","id":"rs_…","summary":[],"encrypted_content":"gAAAA…"}}
{"timestamp":"…","ordinal":21,"type":"response_item","payload":{"type":"custom_tool_call","call_id":"call_1c87…","name":"exec","status":"completed","input":"text(await tools.exec_command({cmd:\"pwd && rg --files…"}}
{"timestamp":"…","ordinal":23,"type":"event_msg","payload":{"type":"item_completed","turn_id":"…","item":{"type":"CommandExecution","id":"exec-fa8d…","command":["/usr/bin/zsh","-lc","pwd && rg …"],
  "cwd":"file:///home/user/proj","status":"completed","exit_code":0,"aggregated_output":"…"},"started_at_ms":1791554488213,"completed_at_ms":1791554488214}}
{"timestamp":"…","ordinal":24,"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call_1c87…","output":[{"type":"input_text","text":"Script completed\n…"}]}}
{"timestamp":"…","type":"response_item","payload":{"type":"function_call","namespace":"collaboration","name":"spawn_agent","call_id":"call_53e5…","arguments":"{\"task_name\":\"research_benchmark\",\"fork_turns\":\"all\",\"message\":\"gAAAA…\"}"}}
{"timestamp":"…","type":"response_item","payload":{"type":"function_call_output","call_id":"call_53e5…","output":"{\"task_name\":\"/root/research_benchmark\"}"}}
{"timestamp":"…","ordinal":22,"type":"token_usage_record","payload":{"thread_id":"01a120f7-…","turn_id":"…","usage":{"input_tokens":24498,"cached_input_tokens":23936,"cache_write_input_tokens":0,"output_tokens":86,"reasoning_output_tokens":0,"total_tokens":24584},"turn_token_usage":{"…":"…"},"thread_token_usage":{"…":"…"}}}
{"timestamp":"…","type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"**BenchME 0.2.0 is installed…"}]}}
{"timestamp":"…","ordinal":6,"type":"event_msg","payload":{"type":"task_complete","turn_id":"01a120b0-…","last_agent_message":"**BenchME…","started_at":1791549782,"completed_at":1791552101,"duration_ms":2319064}}
```

### Legacy rollouts (older files you will meet)

* The envelope is `RolloutLine {timestamp, type, payload}`, with no `ordinal`. `RolloutItem` is `session_meta | response_item | compacted | turn_context | event_msg` ([protocol.rs @ rust-v0.50.0](https://github.com/openai/codex/blob/rust-v0.50.0/codex-rs/protocol/src/protocol.rs)). Very early (mid-2025) files used a bare first line (low confidence).
* Persisted `event_msg` types at v0.50 ([policy.rs](https://github.com/openai/codex/blob/rust-v0.50.0/codex-rs/core/src/rollout/policy.rs)): `user_message {message, images}`, `agent_message {message}`, `agent_reasoning {text}`, `agent_reasoning_raw_content`, `token_count {info{total_token_usage, last_token_usage, model_context_window}}`, `turn_aborted {reason}`, `entered/exited_review_mode`. **Not persisted:** `task_started`, `task_complete`, `exec_command_*`, `mcp_tool_call_*`, `patch_apply_*`.
* **Before rust-v0.100.0 (2026-02-11) there is no turn-finished marker on disk.** v0.100 started persisting `task_started`/`task_complete` (aliases `turn_started`/`turn_complete`) ([policy.rs @ rust-v0.100.0](https://github.com/openai/codex/blob/rust-v0.100.0/codex-rs/core/src/rollout/policy.rs)). For older files, infer the end of a turn: a final assistant `message`, then `token_count`, with no `function_call` waiting for its `function_call_output`.
* `response_item` variants ([models.rs @ rust-v0.100.0](https://github.com/openai/codex/blob/rust-v0.100.0/codex-rs/protocol/src/models.rs)): `message {role, content, end_turn?, phase?}`, `reasoning {summary, content?, encrypted_content?}`, `local_shell_call {call_id, status, action}`, `function_call {name, arguments (JSON string), call_id}`, `function_call_output`, `custom_tool_call(_output)`, `web_search_call {action{query}}`, `ghost_snapshot`, `compaction`.
* `codex-rs/protocol/src/legacy_events.rs` maps new `item_completed` TurnItems back to the legacy `exec_command_end`/`patch_apply_end`/… events. It is a good reference for normalizing old and new formats in one parser.

### Hooks / notify (optional)

* `notify = ["/path/to/program", "args"]` in `config.toml`. Codex appends the JSON as the **last argv argument**, not on stdin: `{"type":"agent-turn-complete","thread-id","turn-id","cwd","client"?,"input-messages":[…],"last-assistant-message"}` ([legacy_notify.rs](https://github.com/openai/codex/blob/main/codex-rs/hooks/src/legacy_notify.rs), which is marked for removal). The binary also contains `approval-requested`.
* **Hooks** live in `~/.codex/hooks.json`, `[hooks]` in `config.toml`, or `<repo>/.codex/hooks.json` (on by default; `[features] hooks=false` disables them). Events: `SessionStart, SessionEnd, UserPromptSubmit, PreToolUse, PermissionRequest, PostToolUse, Stop, Interrupt, PreCompact, PostCompact, SubagentStart, SubagentStop`. The Stop input carries `session_id, transcript_path, cwd, turn_id, model, last_assistant_message` (https://learn.chatgpt.com/docs/hooks; schemas in [codex-rs/hooks/schema/generated](https://github.com/openai/codex/tree/main/codex-rs/hooks/schema/generated)).
* `codex exec --json` prints JSONL events on stdout, which only helps runs you launch yourself.

**Confidence: high** for the current format (local data). Medium for legacy field names, which come from the source.

---

## 3. OpenCode (sst/opencode, now anomalyco/opencode) [local, v2.0.29]

### Paths

OpenCode uses `xdg-basedir` **on every OS**, so the data dir is `$XDG_DATA_HOME/opencode`, falling back to `~/.local/share/opencode` even on macOS and Windows (`%USERPROFILE%\.local\share\opencode`). Source: `packages/opencode/src/global/index.ts` (https://github.com/sst/opencode/blob/dev/packages/opencode/src/global/index.ts).

| What | Path |
|---|---|
| DB (1.x+, SQLite via drizzle) | `<data>/opencode.db`, plus `-wal` and `-shm` |
| Channel builds | `<data>/opencode-<channel>.db` for channels other than `latest/dev/beta/next/prod` |
| Override | **`OPENCODE_DB`** (absolute path, or relative to the data dir). `OPENCODE_DISABLE_CHANNEL_DB=1` forces `opencode.db`. Logic verified in the v2.0.29 binary: `process.env.OPENCODE_DB ?? (prodChannels.includes(ch) \|\| DISABLE ? "opencode.db" : \`opencode-${ch}.db\`)` |
| Legacy (<1.x) JSON storage | `<data>/storage/…` (see below) |
| Other | `<data>/tool-output/` (large tool outputs), `<data>/snapshot/` (git snapshot repos), `<data>/log/`. **`auth.json`, `mcp-auth.json` hold secrets.** |

The local DB is **23 GB** with a 10 MB WAL. Queries must use indexes. Never `count(*)` the big tables, and never scan `part` or `session_message` without a `session_id` / `rowid` bound.

### Storage shape

SQLite with `PRAGMA journal_mode = WAL` (the string is in the binary; confirmed via `pragma journal_mode` → `wal`). Safe to open read-only. **The database also contains credentials** (`account.access_token/refresh_token`, `control_account`, `credential.value`, `session_share.secret`). Only select from `session_v2`, `session_message`, `project`, and the v1 `session`/`message`/`part` tables.

There are two generations of tables in the same file:

* **v2 (current, 2.x)**: `session_v2` plus `session_message` (one row per message; content inline as JSON). Locally all activity since 2026-10-12 is here.
* **v1 (1.x)**: `session` plus `message` (metadata JSON) plus `part` (one row per content part). It stopped receiving rows locally after the 2.x upgrade but still holds history. Support both: someone on 1.x only has v1.

Core v2 schema (abridged):

```sql
session_v2(id TEXT PK /* ses_… */, project_id, parent_id /* subagent parent */, fork_session_id, slug, directory /* cwd */, title, version,
           agent, model /* JSON {id,providerID,variant} */, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write,
           time_created, time_updated, time_compacting, time_archived, time_suspended, time_idle, idle_outcome /* succeeded|failed|interrupted */)
session_message(id PK, session_id, type, seq /* unique per session */, time_created, time_updated, data JSON)
  -- idx (session_id, seq), (session_id, type, seq), (time_created)
project(id /* sha of the repo's first commit, or "global" */, worktree /* repo root path */, vcs, name, time_*)
-- v1:
session(id, project_id, parent_id, directory, title, version, tokens_*, cost, time_*)
message(id /* msg_… */, session_id, time_created, time_updated, data JSON)
part(id /* prt_… */, message_id, session_id, time_created, time_updated, data JSON)
```

`session_message.type` values seen: `user, assistant, synthetic, system, idle, compaction, agent-switched, model-switched, shell`.

### Tail strategy

Rows are **updated in place while they stream**: every sampled assistant row had `time_updated != time_created`. Recommended approach:
1. Watch `opencode.db-wal` (mtime/size) or poll `PRAGMA data_version` every 250–500 ms.
2. On change, run `SELECT id,directory,parent_id,title,time_updated,time_idle,idle_outcome FROM session_v2 WHERE time_updated > :last` (775–1000 rows, cheap).
3. For each active session, run `SELECT seq,type,time_updated,data FROM session_message WHERE session_id=:s AND seq >= :lowest_unfinished_seq ORDER BY seq`. An assistant row is finished when `data.time.completed` is set. Keep re-reading from the first unfinished row.
4. v1: `SELECT … FROM part WHERE session_id=:s AND time_updated > :t` (`part_session_idx`).

### Project mapping

`session_v2.directory` (or v1 `session.directory`) is the cwd. `project.worktree` is the repo root. The project id is derived from git (the root commit hash), or `"global"` for non-git dirs (`worktree:"/"`).

### Record fields (v2 `session_message.data`)

| Need | Where |
|---|---|
| session id | `session_message.session_id` / `session_v2.id` |
| timestamp | `data.time.created` (ms), row `time_created`/`time_updated` |
| user prompt | `type:"user"` → `data.text`, plus `data.files`, `data.agents` |
| assistant text | `type:"assistant"` → `data.content[]` items with `type:"text"` → `.text` (`state.phase:"commentary"\|"final_answer"` for OpenAI) |
| reasoning | `content[]` items with `type:"reasoning"` → `.text` (empty when the provider encrypts it: `state.reasoningEncryptedContent`), `time{created, completed}` |
| tool call and result | `content[]` items with `type:"tool"` `{id:"call_…", name, state{status:"pending"\|"running"\|"completed"\|"error", input{…}, content[{type:"text",text}] /* result */, metadata{exit, truncated}}, time{created, ran, completed}}`. **The call and its result are the same object**: the result shows up when `state.status` becomes `completed`/`error`. Tool names seen: `read, shell, grep, patch, glob, execute, subagent, webfetch, websearch, skill, question`. |
| subagent spawn | tool `name:"subagent"`, `state.input{agent, description, prompt, sessionID}`. The child is a `session_v2` row with `parent_id` = the parent session. |
| subagent completion | tool `state.status:"completed"` with `state.metadata{sessionID, status}`. The parent also gets `type:"synthetic"` `{metadata{source:"subagent", childID, agent, state:"completed"}, text:"<subagent sessionID=… state=…>"}`. The child's `session_v2.idle_outcome` is set. |
| tokens | assistant `data.tokens{input, output, reasoning, cache{read, write}}`, `data.cost`. Session totals are columns on `session_v2`. |
| model | assistant `data.model{id, providerID, variant}`, `data.agent` (`build`/`plan`/`general`…) |
| turn finished | **`type:"idle"` row `{outcome:"succeeded"\|"failed"\|"interrupted", time}`**, and `session_v2.time_idle`/`idle_outcome`. Per step: assistant `data.finish` (`"tool-calls"` means continuing; `"stop"` means final; also `"error"`). |
| waiting for user | tool `name:"question"` in `pending`/`running` state, or permission asks (not persisted as messages) |
| compaction | `type:"compaction"` `{status, reason:"auto", summary, tokens}` |

v1 `part.data.type` values: `text, reasoning, tool {tool, callID, state{status, input, output, metadata, title, time{start,end}}}, step-start, step-finish {reason, tokens, cost}, patch {hash, files}, file, agent, subtask, compaction`. v1 `message.data` (assistant): `{role, parentID /* the user msg */, modelID, providerID, mode, agent, path{cwd, root}, tokens{input, output, reasoning, cache{read, write}, total}, cost, time{created, completed}, finish}`. A v1 turn is finished when the assistant `time.completed` is set and `finish != "tool-calls"`.

### Examples (redacted)

```jsonc
// session_v2 row (child)
{"id":"ses_eb45…","project_id":"e63f09…","parent_id":"ses_eb48…","slug":"clever-wolf","directory":"/home/user/proj","title":"Improve learned joint recovery",
 "version":"2.0.29","agent":"general","model":"{\"id\":\"gpt-6-astra\",\"providerID\":\"openai\",\"variant\":\"xhigh\"}",
 "tokens_input":91588,"tokens_output":19249,"tokens_reasoning":12786,"tokens_cache_read":2827392,"time_idle":1792273821574,"idle_outcome":"succeeded"}
// session_message type=user
{"time":{"created":1792271656963},"text":"Final focused review of…"}
// session_message type=assistant
{"time":{"created":1792274004999,"streamed":1792274024301,"completed":1792274024303},"agent":"build",
 "model":{"id":"gpt-6-astra","providerID":"openai","variant":"xhigh"},"finish":"stop","cost":0,
 "tokens":{"input":965,"output":260,"reasoning":414,"cache":{"read":178304,"write":0}},
 "content":[{"type":"reasoning","text":"","state":{"reasoningEncryptedContent":"gAAAA…"},"time":{"created":…,"completed":…}},
            {"type":"tool","id":"call_703c…","name":"shell","state":{"status":"completed","input":{"command":"…","timeout":10000},
              "content":[{"type":"text","text":"{…}"}],"metadata":{"exit":0,"truncated":false}},"time":{"created":…,"ran":…,"completed":…}},
            {"type":"text","text":"Implemented and packaged…","state":{"phase":"final_answer"}}]}
// subagent tool item
{"type":"tool","id":"call_065b…","name":"subagent","state":{"status":"completed","input":{"agent":"general","sessionID":"ses_eb46…","description":"Review final integration fixes","prompt":"…"},
 "content":[{"type":"text","text":"<subagent sessionID=\"ses_eb46…\" state=\"completed\">\n…"}],"metadata":{"sessionID":"ses_eb46…","status":"completed"}}}
// session_message type=synthetic (subagent finished)
{"metadata":{"source":"subagent","childID":"ses_eb45…","agent":"General","state":"completed"},"description":"Finish held-out reader evaluation","text":"<subagent sessionID=…"}
// session_message type=idle
{"time":{"created":1792274024305},"outcome":"succeeded"}
// v1 part tool
{"type":"tool","tool":"bash","callID":"call_eXc8…","state":{"status":"completed","input":{"command":"git diff --check"},"output":"(no output)","metadata":{"exit":0},"title":"git diff --check","time":{"start":…,"end":…}}}
// v1 part step-finish
{"type":"step-finish","reason":"stop","tokens":{"total":37395,"input":166,"output":109,"reasoning":0,"cache":{"write":0,"read":37120}},"cost":0}
```

### Legacy JSON storage (≤ v1.1.x; SQLite arrived in v1.2.0, 2026-02-14)

The v1.2.0 [`storage/db.ts`](https://github.com/anomalyco/opencode/blob/v1.2.0/packages/opencode/src/storage/db.ts) opens `<data>/opencode.db` with WAL and `busy_timeout=5000`. [`json-migration.ts`](https://github.com/anomalyco/opencode/blob/v1.2.0/packages/opencode/src/storage/json-migration.ts) imports the old tree once. The old layout under `<data>/storage/` was:

* `project/<projectID>.json`
* `session/<projectID>/<sessionID>.json`
* `message/<sessionID>/<messageID>.json`
* `part/<messageID>/<partID>.json`
* `todo/<sessionID>.json`

Files were rewritten **in place with `Bun.write` (no temp+rename)**, so expect half-written files: watch the directories, re-read on change, and retry on parse errors ([storage.ts @ v1.1.60](https://github.com/anomalyco/opencode/blob/v1.1.60/packages/opencode/src/storage/storage.ts)). Message and part JSON matches the v1 `message.data`/`part.data` above. Part types: `text, reasoning, file, agent, compaction, subtask, retry, step-start, step-finish, snapshot, patch, tool` ([message-v2.ts](https://github.com/anomalyco/opencode/blob/v1.1.60/packages/opencode/src/session/message-v2.ts)).

### Optional live channels

* Plugins (`packages/plugin/src/index.ts`): the `event` hook receives every bus event (`message.updated`, `message.part.updated {part, delta}`, `session.status {idle|busy|retry}`, `session.idle`). There are also `tool.execute.before/after`, `chat.message`, `permission.ask`.
* `opencode serve` exposes an SSE stream at `GET /event` (and `/global/event`). It only helps with a server instance the user runs.

**Confidence: high** for v2/v1 (local schema and data). The v2 tables come from a fast-moving 2.x line, so check `migration` and `version` at startup.

---

## 4. Gemini CLI (google-gemini/gemini-cli) [web, main @ v0.64.0]

Not installed here (`~/.gemini` doesn't exist). Source root: https://github.com/google-gemini/gemini-cli/blob/main/. The examples below are built from the TypeScript types and writer code.

### Paths

| What | Path | Source |
|---|---|---|
| Global dir | `<home>/.gemini`, where `<home>` = `$GEMINI_CLI_HOME`, else `os.homedir()`. Linux `~/.gemini`, macOS `~/.gemini`, Windows `%USERPROFILE%\.gemini` | `packages/core/src/utils/paths.ts` |
| Runtime dir (sessions) | the global dir, except under the macOS seatbelt sandbox (`SANDBOX=sandbox-exec`): `~/.cache/.gemini` | `packages/core/src/config/storage.ts` `getGlobalRuntimeDir()` |
| Project registry | `<runtime>/projects.json` = `{"projects":{"<abs cwd>":"<slug>"}}`. Written atomically (tmp + rename) | `packages/core/src/config/projectRegistry.ts` |
| Main transcript | `<runtime>/tmp/<slug>/chats/session-<YYYY-MM-DDTHH-MM>-<sessionId[0:8]>.jsonl` | `packages/core/src/services/chatRecordingService.ts` |
| Subagent transcript | `<runtime>/tmp/<slug>/chats/<parentSessionId>/<agentId>.jsonl` (header `kind:"subagent"`) | same; `packages/core/src/agents/local-executor.ts` |
| Legacy transcript | `.../chats/session-*.json`, one JSON object **rewritten whole** (before Apr 2026, #23749). Converted to `.jsonl` when resumed. | same |
| Prompt log | `<runtime>/tmp/<slug>/logs.json` (JSON array, rewritten) | `packages/core/src/core/logger.ts` |
| Other | `tmp/<slug>/checkpoint-<tag>.json`, `tool-outputs/`, `checkpoints/`, `logs/session-<id>.jsonl` | `storage.ts` |

Never read `oauth_creds.json`, `google_accounts.json`, `mcp-oauth-tokens.json` or `a2a-oauth-tokens.json`. Docker/Podman sandbox runs write into a container-mounted `.gemini` that may not be visible on the host. Sessions are deleted after 30 days by default (`general.sessionRetention.maxAge`).

### Storage shape and tail strategy

**JSONL written as an append-only event log**: `fs.appendFileSync(JSON.stringify(record)+"\n")`. There are several kinds of line:
1. **Header**: has `sessionId` and `projectHash`, no `id`: `{sessionId, projectHash, startTime, lastUpdated, kind?, directories?, summary?}`.
2. **MessageRecord**: has `id`. **The same `id` is appended again whenever the message changes** (tokens arrive, tool calls complete). Upsert by `id`; the last write wins.
3. `{"$set":{…}}`: metadata merge (`lastUpdated`, `summary`, `directories`).
4. `{"$patch":{id?, content?, toolCalls?:[{id,result}], updates?:[…], removeIds?:[], orderIds?:[]}}`.
5. `{"$rewindTo":"<messageId>"}`: drop that message and everything after it.

Tail by byte offset, keep an insertion-ordered `Map<id, MessageRecord>`, and apply lines 3–5 as they arrive. The file is rewritten only when a resumed file was unreadable: the old one is renamed `*.unreadable-<ts>` and a new file is written. Detect that through an inode change. Sessions with no content are deleted on exit. Legacy `.json`: debounce, re-read and diff by `id`.

### Project mapping

The folder slug is the **cwd basename**, lowercased, with non-`[a-z0-9]` characters turned into `-`, plus `-1`/`-2` on collision (`projectRegistry.ts` `slugify`). Recover the full path from **`tmp/<slug>/.project_root`** (plain text, the absolute root) or by inverting `projects.json`. `projectHash` = `sha256(projectRoot)` hex. Installs before Feb 2026 (#17901) used `tmp/<sha256(cwd)>/` folders, which are migrated at startup. Records carry no per-record `cwd`.

### Fields (`packages/core/src/services/chatRecordingTypes.ts`)

| Need | Where |
|---|---|
| session id | header `sessionId`. Subagent: its file stem (`agentId`) |
| timestamp | `timestamp` (ISO) on messages, tool calls and thoughts |
| user prompt | `type:"user"`, `content:[{text}]` (`displayContent` optional). **Exclude** user messages whose parts contain `functionResponse`: those are tool results. |
| assistant text | `type:"gemini"`, `content` (string; a parts array after `$patch`) |
| thinking | `type:"gemini"` `thoughts:[{subject, description, timestamp}]` |
| tool call | `type:"gemini"` `toolCalls[]:{id, name, args, status, timestamp, displayName, description, resultDisplay?, agentId?}`. `status` is one of `validating, scheduled, executing, awaiting_approval, success, error, cancelled`. **Only completed calls are recorded**, so there is no tool-start event in the file. |
| tool result | `toolCalls[].result` (`[{functionResponse:{id,name,response}}]`), linked by `toolCalls[].id`. It is also repeated as a synthetic user message, and can be patched later via `$patch.updates`. |
| subagent | a tool call named after the agent definition, carrying `agentId`. Its transcript is `chats/<parentSessionId>/<agentId>.jsonl`. It completes when that tool call's `status` turns terminal. |
| tokens | `type:"gemini"` `tokens:{input, output, cached, thoughts, tool, total}` |
| model | `type:"gemini"` `model` |
| other | `type:"info"\|"error"\|"warning"` |
| turn finished | **No explicit marker.** Heuristic: the last record is a `gemini` message with text and no tool call or `functionResponse` after it, followed by silence. The `AfterAgent` hook is the definite signal. |

### Examples (redacted, built from the types)

```jsonl
{"sessionId":"3f9c1e2a-…","projectHash":"<sha256-hex>","startTime":"2026-10-18T09:12:03.120Z","lastUpdated":"2026-10-18T09:12:03.120Z"}
{"id":"a1b2…","timestamp":"2026-10-18T09:12:10.004Z","type":"user","content":[{"text":"<user prompt>"}]}
{"$set":{"lastUpdated":"2026-10-18T09:12:10.005Z"}}
{"id":"b2c3…","timestamp":"…","type":"gemini","content":"I'll read the config first.","thoughts":[{"subject":"Inspecting config","description":"<redacted>","timestamp":"…"}],"tokens":{"input":8123,"output":64,"cached":4096,"thoughts":120,"tool":0,"total":8307},"model":"gemini-<model>"}
{"id":"b2c3…","timestamp":"…","type":"gemini","content":"I'll read the config first.","thoughts":["…"],"tokens":{"…":"…"},"model":"gemini-<model>","toolCalls":[{"id":"read_file_1760778735000_0","name":"read_file","args":{"absolute_path":"/home/user/proj/config.ts"},"result":[{"functionResponse":{"id":"read_file_1760778735000_0","name":"read_file","response":{"output":"<redacted>"}}}],"status":"success","timestamp":"…","displayName":"ReadFile"}]}
{"id":"c3d4…","timestamp":"…","type":"user","content":[{"functionResponse":{"id":"read_file_1760778735000_0","name":"read_file","response":{"output":"<redacted>"}}}]}
{"$rewindTo":"a1b2…"}
```

### Hooks

`settings.json` `hooks.<Event>[]`. Events: `BeforeTool, AfterTool, BeforeAgent, AfterAgent` (once per turn, after the final response), `BeforeModel, AfterModel, BeforeToolSelection, SessionStart, SessionEnd, Notification, PreCompress`. stdin carries `{session_id, transcript_path, cwd, hook_event_name, timestamp}` (`docs/hooks/reference.md`). OTEL telemetry can go to a file (`telemetry.outfile`, `GEMINI_TELEMETRY_OUTFILE`), but it is opt-in (`docs/cli/telemetry.md`).

**Confidence: high** (source-verified, not seen locally). Handle both `.json` and `.jsonl`, and both hash and slug folders.

---

## 5. Qwen Code (QwenLM/qwen-code) [web, main @ fbc698a, v0.25.2]

Not installed here. Qwen Code began as a Gemini CLI fork, but its **session storage has been rewritten into a Claude-Code-like design**. Source root: https://github.com/QwenLM/qwen-code/blob/main/

### Paths

| What | Path | Source |
|---|---|---|
| Global dir | `$QWEN_HOME`, else `~/.qwen` (Windows `%USERPROFILE%\.qwen`) | `packages/core/src/config/storage.ts` `getGlobalQwenDir()` |
| Runtime base | pinned runtime context, then `$QWEN_RUNTIME_DIR`, then settings `advanced.runtimeOutputDir`, then the global dir | `storage.ts` `getRuntimeBaseDir()` |
| Transcript | `<runtime>/projects/<sanitizeCwd(root)>/chats/<sessionId>.jsonl` | `packages/core/src/services/chatRecordingService.ts` |
| Liveness sidecar | `.../chats/<sessionId>.runtime.json` = `{schema_version, pid, session_id, work_dir, hostname, started_at, qwen_version}`. Written atomically and **deleted on exit**. | `packages/core/src/utils/runtimeStatus.ts` |
| Subagents | `<runtime>/projects/<enc>/subagents/<sessionId>/agent-<agentId>.jsonl` plus `agent-<id>.meta.json` (and a temporary `.jsonl.stream`) | `packages/core/src/agents/agent-transcript.ts` |
| Temp | `<runtime>/tmp/<sha256(root)>/{checkpoints,tool-results}` | `storage.ts` |

Never read `oauth_creds.json`, `mcp-oauth-tokens.json` or `google_accounts.json`.

### Storage shape and tail strategy

Append-only JSONL, one self-contained `ChatRecord` per line, never rewritten (`O_APPEND` writer lease, `packages/core/src/services/session-writer-lease.ts`). Rewind and branching re-parent via `parentUuid` (the same idea as Claude Code). Tail by byte offset. Re-read `*.runtime.json` and `*.meta.json` on change. A session is **live** while `runtime.json` exists and its `pid` is alive on this `hostname`.

### Project mapping

`sanitizeCwd` replaces non-`[A-Za-z0-9]` characters with `-` (Windows lowercases first), the same lossy scheme as Claude Code. Use the **`cwd` field on every record**, or `work_dir` from `runtime.json`.

### Fields (`ChatRecord` in `chatRecordingService.ts`)

| Need | Where |
|---|---|
| session id | `sessionId` (file stem) |
| timestamp | `timestamp` ISO. Lifecycle events add `startedAt`/`endedAt` (epoch ms) |
| cwd | `cwd`, `gitBranch` on every record. Chain: `uuid`, `parentUuid`, `promptId` |
| user prompt | `type:"user"`, `provenance:"real_user"`, `message:{role:"user", parts:[{text}]}`. Subtypes `notification, cron, mid_turn_user_message, agent_message, background_task_completed` are not human input. |
| assistant text | `type:"assistant"`, `message:{role:"model", parts:[{text}]}` |
| thinking | parts with `thought:true` |
| tool call | assistant `message.parts[].functionCall{id, name, args}`. **Explicit start and end**: `type:"system", subtype:"ui_telemetry"`, `systemPayload.uiEvent{"event.name":"tool_lifecycle", phase:"started"\|"ended", callId, toolName, outcome, executionDurationMs, subagentId?}` (`packages/core/src/telemetry/tool-lifecycle.ts`) |
| tool result | `type:"tool_result"`, `message.parts[].functionResponse{id,name,response}` plus `toolCallResult{callId, status, resultDisplay, error?}`. Link by `callId`. |
| model requests | `ui_telemetry` `event.name:"request_lifecycle"` (phase, outcome, model) and `api_response` (`input_token_count, output_token_count, cached_content_token_count, thoughts_token_count, total_token_count`) |
| subagent | tool named `agent` (formerly `task`). Sidechain records carry `isSidechain:true, agentId, agentName, agentColor`. `meta.json` = `{agentId, agentType, description, parentSessionId, toolUseId, parentAgentId, createdAt, status, isBackgrounded, model}`. **Completion: `meta.json.status` becomes `completed`/`failed`/`cancelled`**, plus the parent's `tool_result` (foreground) or a `background_task_completed` record (background). |
| tokens | assistant `usageMetadata{promptTokenCount, candidatesTokenCount, cachedContentTokenCount, thoughtsTokenCount, totalTokenCount}`, `contextWindowSize` |
| model | assistant `model`; `system/session_model` records |
| turn finished | `type:"system", subtype:"turn_result"` `{promptId, state:"completed"\|"cancelled"\|"error", stopReason, endedAt}`, but **only in ACP / `qwen serve` sessions**. Interactive TUI heuristic: the last `request_lifecycle` has `phase:"ended"`, and the last assistant message has no `functionCall`. |

### Examples (redacted, built from the types)

```jsonl
{"uuid":"u1","parentUuid":null,"sessionId":"9d1e…","timestamp":"2026-10-18T10:00:00.000Z","type":"user","provenance":"real_user","cwd":"/home/user/proj","version":"0.25.2","gitBranch":"main","message":{"role":"user","parts":[{"text":"<user prompt>"}]},"promptId":"9d1e…########1"}
{"uuid":"a1","parentUuid":"s1","type":"assistant","model":"<model>","message":{"role":"model","parts":[{"text":"<redacted>","thought":true},{"text":"Let me check the config."},{"functionCall":{"id":"call_abc123","name":"read_file","args":{"absolute_path":"/home/user/proj/config.ts"}}}]},"usageMetadata":{"promptTokenCount":9012,"candidatesTokenCount":88,"cachedContentTokenCount":4096,"thoughtsTokenCount":40,"totalTokenCount":9140}, "...":"base fields"}
{"uuid":"s2","parentUuid":"a1","type":"system","subtype":"ui_telemetry","systemPayload":{"uiEvent":{"event.name":"tool_lifecycle","callId":"call_abc123","toolName":"read_file","phase":"started","startedAt":1760781603000}}, "...":"base fields"}
{"uuid":"t1","parentUuid":"s2","type":"tool_result","message":{"role":"user","parts":[{"functionResponse":{"id":"call_abc123","name":"read_file","response":{"output":"<redacted>"}}}]},"toolCallResult":{"callId":"call_abc123","status":"success"}, "...":"base fields"}
{"uuid":"r1","parentUuid":"t1","type":"system","subtype":"turn_result","systemPayload":{"promptId":"…","state":"completed","endedAt":1760781612000}, "...":"ACP/serve only"}
```

### Hooks

Claude-Code-style (`docs/users/features/hooks.md`): `PreToolUse, PostToolUse, PostToolUseFailure, PostToolBatch, UserPromptSubmit, SessionStart, SessionEnd, Stop, StopFailure, SubagentStart, SubagentStop` (with `agent_transcript_path`), `PreCompact, PostCompact, Notification` (`permission_prompt`, `idle_prompt`), `PermissionRequest`. There is also "Dual Output" `--json-fd/--json-file`, which must be set at launch.

**Confidence: high** (source-verified).

---

## 6. GitHub Copilot CLI (github/copilot-cli, npm @github/copilot) [web; local dir has no sessions]

On this machine `~/.copilot` only contains `config.json` and `logs/process-*.log`, so there were no sessions to inspect. The schema below comes from the official SDK's generated types: https://github.com/github/copilot-sdk/blob/main/nodejs/src/generated/session-events.ts (generated from `session-events.schema.json`).

### Paths

| What | Path |
|---|---|
| Root | `~/.copilot` on Linux, macOS and Windows (`%USERPROFILE%\.copilot`). **`COPILOT_HOME`** replaces it ([config dir reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)). |
| Session dir | `session-state/<session-uuid>/`: `events.jsonl` (main), `workspace.yaml` (`cwd, created_at, updated_at, summary, name, branch, repository`), `plan.md`, `checkpoints/`, `files/` |
| Legacy | `history-session-state/*.json` (older builds; one JSON per session, rewritten) |
| Cross-session | `session-store.db` (SQLite, `sessions` table) |
| Cache | `COPILOT_CACHE_HOME`, else `~/Library/Caches/copilot`, `$XDG_CACHE_HOME/copilot`, or `%LOCALAPPDATA%\copilot` |
| **Secrets** | `config.json` can hold plaintext tokens when no keychain exists. Also skip `mcp-oauth-config/` and `mcp-secrets/`. |

### Storage shape and tail strategy

Append-only JSONL, one event per line, so tail by byte offset (`ShutdownData.eventsFileSizeBytes` refers to "the session's persisted events.jsonl"). Watch `session-state/` for new directories, and re-read `workspace.yaml` when it changes. **Caveat:** events marked `ephemeral: true` are not persisted. That covers `session.idle`, `assistant.idle`, **`assistant.usage`**, and all streaming deltas, so a file reader sees neither idle nor per-call usage events.

### Project mapping

`session.start.data.context {cwd, gitRoot, branch, repository, headCommit}`, `workspace.yaml cwd`, and `session.context_changed` for later changes. The directory name is a UUID.

### Envelope and fields

Every line is `{id, timestamp (ISO), parentId (previous event id), type, data, ephemeral?, agentId?}`. `agentId` is set on sub-agent events.

| Need | Where |
|---|---|
| session id | `session.start.data.sessionId` (= directory name) |
| model | `session.start.data.selectedModel`, `assistant.turn_start.data.model`, `session.model_change` |
| user prompt | `user.message.data.content` (`source` marks injected messages; `transformedContent` is what the model saw) |
| assistant text | `assistant.message.data.content` (`messageId`, `phase`, `outputTokens`) |
| thinking | `assistant.reasoning.data.content`, `assistant.message.data.reasoningText` |
| tool call | `assistant.message.data.toolRequests[]{toolCallId, name, arguments}`, `tool.execution_start {toolCallId, toolName, arguments, turnId}` |
| tool result | `tool.execution_complete {toolCallId, success, result{content, detailedContent}, error?}`. **Link by `toolCallId`.** |
| subagent | `subagent.started {toolCallId, agentName, agentDisplayName, agentType, executionMode, model}`, then `subagent.completed {toolCallId, durationMs, totalTokens, totalToolCalls}` or `subagent.failed`. Child events carry `agentId`. |
| tokens | per call only through ephemeral `assistant.usage`, which is **not on disk**. Persisted: `assistant.message.data.outputTokens`, and `session.shutdown.data.modelMetrics` at the end |
| turn boundaries | `assistant.turn_start` / **`assistant.turn_end {turnId}`** (one agent-loop iteration), `session.task_complete {success, summary}`, `abort {reason}`, `session.shutdown` |
| waiting for user | no persisted idle event. Heuristic: the last event is `assistant.turn_end` with no `turn_start` or `tool.execution_start` after it. `permission.requested` / `user_input.requested` mean blocked on the user (check whether they are persisted). |

### Examples (built from the schema and redacted)

```jsonl
{"id":"e1","parentId":null,"timestamp":"2026-10-15T19:05:40.000Z","type":"session.start","data":{"sessionId":"3f2a…","producer":"copilot-agent","copilotVersion":"1.x","selectedModel":"<model>","context":{"cwd":"/home/user/proj","gitRoot":"/home/user/proj","branch":"main","repository":"owner/repo"}}}
{"id":"e2","parentId":"e1","timestamp":"…","type":"user.message","data":{"content":"Fix the failing test","messageId":"m1"}}
{"id":"e3","parentId":"e2","timestamp":"…","type":"assistant.turn_start","data":{"turnId":"0","model":"<model>"}}
{"id":"e4","parentId":"e3","timestamp":"…","type":"assistant.message","data":{"messageId":"a1","content":"Running tests…","outputTokens":120,"toolRequests":[{"toolCallId":"call_1","name":"bash","arguments":{"command":"npm test"}}]}}
{"id":"e5","parentId":"e4","timestamp":"…","type":"tool.execution_start","data":{"toolCallId":"call_1","toolName":"bash","arguments":{"command":"npm test"},"turnId":"0"}}
{"id":"e6","parentId":"e5","timestamp":"…","type":"tool.execution_complete","data":{"toolCallId":"call_1","success":true,"result":{"content":"<redacted>"}}}
{"id":"e7","parentId":"e6","timestamp":"…","type":"subagent.started","data":{"toolCallId":"call_2","agentName":"explore","agentDisplayName":"Explore","executionMode":"sync"}}
{"id":"e8","parentId":"e7","timestamp":"…","type":"subagent.completed","data":{"toolCallId":"call_2","durationMs":8123,"totalTokens":20411,"totalToolCalls":6}}
{"id":"e9","parentId":"e8","timestamp":"…","type":"assistant.turn_end","data":{"turnId":"0"}}
```

### Hooks

Hooks live in `.github/hooks/*.json`, `~/.copilot/hooks/*.json`, or the `hooks` key of `settings.json`, in the format `{"version":1,"hooks":{"<event>":[{"type":"command","bash":"…"}]}}`. Events: `sessionStart, sessionEnd, userPromptSubmitted, preToolUse, postToolUse, postToolUseFailure, agentStop` (adds `transcriptPath, stopReason`, and is the clean turn-done signal the file lacks), `subagentStart, subagentStop, errorOccurred, preCompact, permissionRequest, notification` ([hooks reference](https://docs.github.com/en/copilot/reference/hooks-configuration)).

**Confidence: medium-high** (official SDK schema, but no local sample).

---

## VS Code globalStorage locations (Cline legacy, Roo Code, Kilo Code before 7.x)

Extension folder = lowercased `publisher.name`: **`saoudrizwan.claude-dev`** (Cline), **`rooveterinaryinc.roo-cline`** (Roo Code), **`kilocode.kilo-code`** (Kilo Code). Each extension's `package.json`: [cline](https://github.com/cline/cline/blob/main/apps/vscode/package.json), [roo](https://github.com/RooCodeInc/Roo-Code/blob/main/src/package.json), [kilo](https://github.com/Kilo-Org/kilocode/blob/main/packages/kilo-vscode/package.json).

| Variant | Linux | macOS | Windows |
|---|---|---|---|
| VS Code | `~/.config/Code/User/globalStorage/<ext>/` | `~/Library/Application Support/Code/User/globalStorage/<ext>/` | `%APPDATA%\Code\User\globalStorage\<ext>\` |
| Insiders | `~/.config/Code - Insiders/User/globalStorage/` | `~/Library/Application Support/Code - Insiders/…` | `%APPDATA%\Code - Insiders\…` |
| VSCodium | `~/.config/VSCodium/…` | `~/Library/Application Support/VSCodium/…` | `%APPDATA%\VSCodium\…` |
| Code - OSS | `~/.config/Code - OSS/…` | `~/Library/Application Support/Code - OSS/…` | `%APPDATA%\Code - OSS\…` |
| Cursor | `~/.config/Cursor/…` | `~/Library/Application Support/Cursor/…` | `%APPDATA%\Cursor\…` |
| Windsurf | `~/.config/Windsurf/…` | `~/Library/Application Support/Windsurf/…` | `%APPDATA%\Windsurf\…` |
| Flatpak VS Code | `~/.var/app/com.visualstudio.code/config/Code/User/globalStorage/` | – | – |
| `--user-data-dir X` / portable | `X/User/globalStorage/` (portable: `<VSCODE_PORTABLE>/user-data/User/globalStorage/`) | same | same |
| Remote (SSH/WSL/containers) | on the remote host: `~/.vscode-server/data/User/globalStorage/` (also `~/.cursor-server`, `~/.windsurf-server`, `~/.vscodium-server`) | | |

Discovery glob: `{~/.config,~/Library/Application Support,%APPDATA%}/*/User/globalStorage/{saoudrizwan.claude-dev,rooveterinaryinc.roo-cline,kilocode.kilo-code}`. Very old versions kept task history in VS Code's `state.vscdb` (globalState). Use the files instead.

---

## 7. Cline (cline/cline) [web, main @ ext 4.1.25 / CLI 3.0.73; legacy-extension @ 4.0.12]

No local data on this machine.

### Two runtimes, chosen per window

Since 4.1.0, the VSIX ships a **legacy** bundle (the 4.0.x code) and an **SDK "next"** bundle. A remote rollout flag (`ext-sdk-bundle-rollout`, which started at 1%) picks one per window. "Tasks created on the SDK bundle are stored as SDK sessions the legacy bundle doesn't list" ([apps/vscode-rollout/README.md](https://github.com/cline/cline/blob/main/apps/vscode-rollout/README.md)). CLI 3.x and JetBrains use the SDK. **Watch both layouts.**

### Paths

* **Legacy (VS Code):** `<globalStorage>/saoudrizwan.claude-dev/tasks/<taskId>/{api_conversation_history.json, ui_messages.json, task_metadata.json, context_history.json, settings.json}` plus the index `state/taskHistory.json` ([disk.ts](https://github.com/cline/cline/blob/legacy-extension/apps/vscode/src/core/storage/disk.ts)). `taskId = Date.now()`. The legacy CLI and JetBrains use the same layout under `~/.cline/data/`. There is no custom-path setting.
* **SDK / CLI 3.x / JetBrains** ([paths.ts](https://github.com/cline/cline/blob/main/sdk/packages/shared/src/storage/paths.ts)): `CLINE_DIR` (default `~/.cline` on all OSes) → `CLINE_DATA_DIR` (`$CLINE_DIR/data`) → `CLINE_SESSION_DATA_DIR` (`…/sessions/<sid>/{<sid>.messages.json, <sid>.json manifest, <sid>.compaction.json, <agentId>.messages.json}`), plus `CLINE_DB_DATA_DIR` (`…/db/sessions.db`) and `CLINE_HOOKS_LOG_PATH` (`…/logs/hooks.jsonl`). `cline --data-dir X` redirects all of these. Session id = `${Date.now()}_${nanoid5}`.

### Storage shape and tail strategy

* **Legacy:** `ui_messages.json` and `api_conversation_history.json` are JSON arrays **rewritten whole** via `${file}.tmp.<ts>.<rand>.json` + rename (`atomicWriteFile`). Watch `tasks/` recursively, ignore `*.tmp.*.json`, debounce about 100 ms, re-read, and diff by `ts`. Entries change in place: `partial` flips to false, and `api_req_started` gains tokens and cost later.
* **SDK:** `<sid>.messages.json` = `{version:1, updated_at, agent:"lead"|"subagent"|"teammate", sessionId, origin, messages:[…]}`, written with **non-atomic `writeFileSync`** ([session-data.ts](https://github.com/cline/cline/blob/main/sdk/packages/core/src/services/session-data.ts)), so retry on a parse error. `sessions.db` is **SQLite WAL** (busy_timeout 5000, [sqlite-db.ts](https://github.com/cline/cline/blob/main/sdk/packages/shared/src/db/sqlite-db.ts)). Its `sessions` table has `session_id, source, pid, started_at, ended_at, status, provider, model, cwd, workspace_root, parent_session_id, parent_agent_id, agent_id, is_subagent, prompt, metadata_json (title, totalCost, tokensIn/Out, cacheReads/Writes), messages_path, updated_at`. Poll it read-only by `updated_at` to find active sessions, then re-read their `messages_path`.
* `hooks.jsonl` (SDK audit log, append-only: `agent_start, prompt_submit, tool_call, tool_result, agent_end, agent_abort, agent_error`) is written only when no runtime hooks are configured. The VS Code "next" bundle always configures them, so don't rely on the log.

### Project mapping

Legacy: `HistoryItem.cwdOnTaskInitialization` in `state/taskHistory.json` ([HistoryItem.ts](https://github.com/cline/cline/blob/legacy-extension/apps/vscode/src/shared/HistoryItem.ts)). Fallback: `# Current Working Directory (/abs/path)` inside `<environment_details>`. SDK: `sessions.cwd`, `workspace_root`.

### Fields

| Need | Legacy (`ui_messages.json` = `ClineMessage[]`; `api_conversation_history.json` = Anthropic `MessageParam[]` plus `ts`, `modelInfo`, `metrics`) | SDK |
|---|---|---|
| session / time | folder name; `ts` (ms) | `sessionId`; message `ts` |
| user prompt | `say:"task"`, `say:"user_feedback"` | `role:"user"` text; `sessions.prompt` |
| assistant text | `say:"text"` | assistant `text` blocks |
| thinking | `say:"reasoning"`; api `thinking`/`redacted_thinking` | `thinking` blocks |
| tool call | `ask`/`say:"tool"` with JSON text `{tool, path, diff, content}`, `ask:"command"`, `ask:"use_mcp_server"`. Api: `tool_use{id,name,input}`, or **XML tags in text** (`<execute_command>…`) for non-native models | `tool_use{id,name,input}` (`read_files, search_codebase, run_commands, apply_patch, editor, spawn_agent, ask_question, …`) |
| tool result | `say:"command_output"`, `say:"mcp_server_response"`; api `tool_result{tool_use_id}` | `tool_result{tool_use_id, name, content, is_error}` |
| subagents | `ask:"use_subagents"` `{prompts}`. Progress in `say:"subagent"` JSON `{status, total, completed, items[{prompt, status, toolCalls, inputTokens, outputTokens, result}]}` (in-process, no separate files). `ask:"new_task"` is a hand-off with no parent link. | `spawn_agent` tool. The child session id is `<root>__<agentId>`, with `is_subagent=1, parent_session_id`. Messages go in `<root>/<agentId>.messages.json`. Done when the child status is terminal and the parent `tool_result` appears. |
| tokens | `say:"api_req_started"` whose `text` is JSON `{request, tokensIn, tokensOut, cacheWrites, cacheReads, cost, cancelReason?}`; api `metrics{tokens{prompt,completion,cached},cost}` | assistant `metrics{inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens, cost}` |
| model | `modelInfo{modelId, providerId, mode}`; `task_metadata.json model_usage[]` | `modelInfo{id, provider}`; `sessions.model` |
| **waiting / finished** | last message is `type:"ask"` and not `partial`. **Idle asks:** `completion_result, followup, plan_mode_respond, resume_task, resume_completed_task, api_req_failed, mistake_limit_reached`. **Approval asks:** `tool, command, use_mcp_server, browser_action_launch`. A request is in flight while the last `api_req_started` has no `cost` and no `cancelReason`. | `sessions.status`: `idle` (waiting), `running`, `pending`, `completed`, `failed`, `cancelled` ([records.ts](https://github.com/cline/cline/blob/main/sdk/packages/shared/src/session/records.ts)) |

Sources: [ExtensionMessage.ts](https://github.com/cline/cline/blob/legacy-extension/apps/vscode/src/shared/ExtensionMessage.ts), [tools.ts](https://github.com/cline/cline/blob/legacy-extension/apps/vscode/src/shared/tools.ts), [sdk messages.ts](https://github.com/cline/cline/blob/main/sdk/packages/shared/src/llms/messages.ts), [spawn-agent-tool.ts](https://github.com/cline/cline/blob/main/sdk/packages/core/src/extensions/tools/team/spawn-agent-tool.ts).

### Examples (redacted, built from the types)

```jsonc
// legacy ui_messages.json entries
{"ts":1760812345678,"type":"say","say":"task","text":"<redacted prompt>"}
{"ts":1760812345702,"type":"say","say":"api_req_started","text":"{\"request\":\"<task>…\",\"tokensIn\":12034,\"tokensOut\":412,\"cacheWrites\":11800,\"cacheReads\":0,\"cost\":0.0521}","modelInfo":{"modelId":"claude-…","providerId":"anthropic","mode":"act"}}
{"ts":1760812347001,"type":"say","say":"reasoning","text":"<redacted>","partial":false}
{"ts":1760812348010,"type":"say","say":"tool","text":"{\"tool\":\"readFile\",\"path\":\"src/main.rs\"}"}
{"ts":1760812349100,"type":"ask","ask":"command","text":"cargo test"}
{"ts":1760812355000,"type":"say","say":"command_output","text":"<redacted>"}
{"ts":1760812360000,"type":"ask","ask":"completion_result","text":"","partial":false}
// legacy api_conversation_history.json entry
{"role":"assistant","ts":1760812349000,"content":[{"type":"thinking","thinking":"<redacted>","signature":"<sig>"},{"type":"tool_use","id":"toolu_01…","name":"execute_command","input":{"command":"cargo test","requires_approval":false}}],
 "modelInfo":{"modelId":"…","providerId":"…","mode":"act"},"metrics":{"tokens":{"prompt":12034,"completion":412,"cached":0},"cost":0.0521}}
// legacy state/taskHistory.json entry
{"id":"1760812345678","ts":1760812360000,"task":"<redacted>","tokensIn":24068,"tokensOut":980,"cacheWrites":11800,"cacheReads":11800,"totalCost":0.08,"cwdOnTaskInitialization":"/home/user/proj","modelId":"…"}
// SDK <sid>.json manifest
{"version":1,"session_id":"1760812345678_k3j9x","source":"vscode","pid":12345,"status":"idle","provider":"cline","model":"…","cwd":"/home/user/proj","messages_path":"/home/user/.cline/data/sessions/1760812345678_k3j9x/1760812345678_k3j9x.messages.json"}
```

**Hooks:** executable files named after the event (`TaskStart, TaskResume, TaskCancel, TaskComplete, TaskError, PreToolUse, PostToolUse, UserPromptSubmit, PreCompact, SessionShutdown`) in `~/Documents/Cline/Hooks/`, `~/.cline/hooks/`, `<ws>/.clinerules/hooks/` or `<ws>/.cline/hooks/`. Each gets JSON on stdin ([hook-file-config.ts](https://github.com/cline/cline/blob/main/sdk/packages/core/src/hooks/hook-file-config.ts)).

**Confidence: high** (source). The dual runtime is the main pitfall.

---

## 8. Roo Code and Kilo Code [web]

### Roo Code (RooCodeInc/Roo-Code), shut down 2026-05-15, repo archived at 3.53.0

"The Roo Code Extension was shut down on May 15th" ([README](https://github.com/RooCodeInc/Roo-Code/blob/main/README.md)). Support it only for existing installs. The community fork ZooCode probably keeps this layout under its own extension id (unverified).

* **Paths:** `<globalStorage>/rooveterinaryinc.roo-cline/tasks/<uuidv7>/{api_conversation_history.json, ui_messages.json, task_metadata.json, history_item.json}`, plus `tasks/_index.json` `{version, updatedAt, entries: HistoryItem[]}` ([globalFileNames.ts](https://github.com/RooCodeInc/Roo-Code/blob/main/src/shared/globalFileNames.ts), [TaskHistoryStore.ts](https://github.com/RooCodeInc/Roo-Code/blob/main/src/core/task-persistence/TaskHistoryStore.ts)). Override: the **`roo-cline.customStoragePath`** setting in the editor's `User/settings.json` ([storage.ts](https://github.com/RooCodeInc/Roo-Code/blob/main/src/utils/storage.ts)). Roo CLI: `~/.vscode-mock/global-storage/tasks/…`.
* **Shape:** whole-file JSON written by `safeWriteJson`: a proper-lockfile `<file>.lock` **directory**, write `.<name>.new_*.tmp`, rename the target to `.bak_*.tmp`, rename `.new` into place ([safeWriteJson.ts](https://github.com/RooCodeInc/Roo-Code/blob/main/src/utils/safeWriteJson.ts)). The file is **briefly missing**, so ignore `.*.tmp`/`*.lock` and retry on ENOENT.
* **Project:** `history_item.workspace`.
* **Fields:** the same `ClineMessage` model as Cline legacy ([message.ts](https://github.com/RooCodeInc/Roo-Code/blob/main/packages/types/src/message.ts)): `say:"text"|"reasoning"|"command_output"|"api_req_started"` (JSON `{apiProtocol, tokensIn, tokensOut, cacheWrites, cacheReads, cost}`) and `ask:"tool"|"command"|"followup"|"completion_result"|"resume_task"`. Api history uses native `tool_use`/`tool_result`. OpenAI reasoning is stored as `{type:"reasoning", encrypted_content}` entries. **The model is not stored per message**; only `history_item.apiConfigName` and `mode` are kept.
* **Subtasks:** the `new_task` tool `{mode, message}` creates a child task with `parentTaskId` and `rootTaskId`. The parent gets `status:"delegated", delegatedToId, awaitingChildId, childIds[]`. On completion the parent gets `completedByChildId`, `completionResultSummary` and a `say:"subtask_result"` message ([history.ts](https://github.com/RooCodeInc/Roo-Code/blob/main/packages/types/src/history.ts)).
* **Waiting / finished:** the last message is a non-partial `ask` of `completion_result | api_req_failed | resume_completed_task | mistake_limit_reached` (idle) or `followup | command | tool | use_mcp_server` (needs the user). `history_item.status` is `active | completed | delegated`.
* **Hooks:** none. The IPC socket (`ROO_CODE_IPC_SOCKET_PATH`) must be set when the editor launches.

```jsonc
{"ts":1760812346000,"type":"ask","ask":"tool","text":"{\"tool\":\"newTask\",\"mode\":\"code\",\"content\":\"<redacted>\"}","partial":false}
{"ts":1760812400000,"type":"say","say":"subtask_result","text":"<redacted>"}
// history_item.json
{"id":"0199f0a2-…","rootTaskId":"0199f09f-…","parentTaskId":"0199f09f-…","number":3,"ts":1760812400000,"task":"<redacted>","tokensIn":8100,"tokensOut":220,"totalCost":0.004,"workspace":"/home/user/proj","mode":"code","apiConfigName":"default","status":"completed"}
```

**Confidence: high** (source), but the product is discontinued.

### Kilo Code (Kilo-Org/kilocode) 7.x: an OpenCode fork

**Surprise:** Kilo 7.x (extension and `@kilocode/cli` 7.8.14) is **rebased on OpenCode** (`.opencode-version`, `packages/opencode`). The VS Code extension spawns a bundled `kilo serve`, and **CLI, VS Code and JetBrains sessions all go to one SQLite DB.**

* **Path:** `$XDG_DATA_HOME/kilo/kilo.db`, default **`~/.local/share/kilo/kilo.db` on every OS** (Windows: `%USERPROFILE%\.local\share\kilo\kilo.db`). Channel builds: `kilo-<channel>.db` (or the older `opencode-<channel>.db`). Overrides: `KILO_DB`, `KILO_DISABLE_CHANNEL_DB`, `XDG_DATA_HOME`. `kilo db path` prints the active path. Under Remote SSH the DB is on the remote host. ([global.ts](https://github.com/Kilo-Org/kilocode/blob/main/packages/core/src/global.ts), [db.ts](https://github.com/Kilo-Org/kilocode/blob/main/packages/opencode/src/storage/db.ts), [flag.ts](https://github.com/Kilo-Org/kilocode/blob/main/packages/core/src/flag/flag.ts), [troubleshooting doc](https://github.com/Kilo-Org/kilocode/blob/main/packages/kilo-docs/pages/getting-started/troubleshooting/troubleshooting-extension.md))
* **Shape:** SQLite, **WAL**, busy_timeout 5000. Tables `session` (`directory, parent_id, agent, model, tokens_*, time_*`), `message`, `part`, `todo`, plus the newer `session_message`. Per migration `20260714141136_session-message-legacy-writer-compat`, the v1 `message`/`part` tables and `session_message` are written side by side ([sql.ts](https://github.com/Kilo-Org/kilocode/blob/main/packages/core/src/session/sql.ts)). **Reuse the OpenCode parser (§3)** with the same tail strategy and the same caution about credential tables.
* **Project:** `session.directory` (forward slashes on Windows). Agent Manager sessions point into `.kilo` worktrees.
* **Legacy (before 7.x, a Roo fork):** `<globalStorage>/kilocode.kilo-code/tasks/<id>/…` in Roo's format. 7.x imports these ([migrate.ts](https://github.com/Kilo-Org/kilocode/blob/main/packages/kilo-vscode/src/legacy-migration/sessions/migrate.ts)).
* **Hooks:** OpenCode-style plugins ([plugins.md](https://github.com/Kilo-Org/kilocode/blob/main/packages/kilo-docs/pages/automate/extending/plugins.md)). The `kilo serve` SSE stream runs on a random port with a password, so it is not usable passively.

**Confidence: high** (source and docs).

---

## 9. Goose (Block, now aaif-goose/goose) [web]

The `block/goose` repo now redirects to `aaif-goose/goose`; the on-disk names still say "Block". Not installed here.

### Paths

Paths come from `etcetera::choose_app_strategy` (author "Block", app "goose"), which means **XDG on Linux and on macOS** ([crates/goose/src/config/paths.rs](https://github.com/aaif-goose/goose/blob/main/crates/goose/src/config/paths.rs), [etcetera app_strategy.rs](https://github.com/lunacookies/etcetera/blob/master/src/app_strategy.rs)).

| OS | Session DB |
|---|---|
| Linux / macOS | `${XDG_DATA_HOME:-~/.local/share}/goose/sessions/sessions.db` |
| Windows | `%APPDATA%\Block\goose\data\sessions\sessions.db` |
| Override | `GOOSE_PATH_ROOT=<abs>` → `<root>/data/sessions/sessions.db` (also `<root>/config`, `<root>/state`) |
| Legacy (< v1.10) | `<data>/sessions/<id>.jsonl` (line 1 = metadata, then one message per line). Imported into the DB once and left on disk. ([session/legacy.rs](https://github.com/aaif-goose/goose/blob/main/crates/goose/src/session/legacy.rs)) |

Config is in `~/.config/goose/config.yaml` and logs in `~/.local/state/goose/logs/`. The desktop app (goosed) writes the same DB.

### Storage shape

SQLite via sqlx: **WAL**, `busy_timeout=30s`, `BEGIN IMMEDIATE` writes, `schema_version` 17 ([session_manager.rs](https://github.com/aaif-goose/goose/blob/main/crates/goose/src/session/session_manager.rs)). Safe to open with `mode=ro`.

* `sessions(id /* "YYYYMMDD_N" */, name, description, session_type /* user|scheduled|sub_agent|hidden|terminal|gateway|acp */, working_dir, project_id, parent_session_id, thread_id, created_at, updated_at, archived_at, total_tokens, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, accumulated_*, accumulated_cost, provider_name, model_config_json, recipe_json, …)`
* `messages(id INTEGER AUTOINCREMENT, message_id, session_id, role /* user|assistant */, content_json, created_timestamp /* unix s */, timestamp, tokens, metadata_json)`
* `usage_ledger(session_id, created_timestamp, model, input/output/total/cache_read/cache_write tokens, cost, is_compaction)`

**Tail:** watch `sessions.db-wal`, then `SELECT … FROM messages WHERE id > :last ORDER BY id`. **`replace_conversation` deletes and re-inserts the whole session** (with new autoincrement ids), so dedupe on `message_id`. When a session's row count drops, reload it.

### Fields

| Need | Where |
|---|---|
| session / cwd | `sessions.id`, **`sessions.working_dir`** |
| timestamp | `messages.created_timestamp` (unix s; values > 1e10 are ms) |
| user prompt | role `user`, content item `{"type":"text","text"}` |
| assistant text | role `assistant`, `{"type":"text"}` |
| thinking | `{"type":"thinking","thinking","signature"}` / `{"type":"redactedThinking"}` |
| tool call | `{"type":"toolRequest","id","toolCall":{"status":"success","value":{"name":"<ext>__<tool>","arguments":{…}}}}` ([tool_request.rs](https://github.com/aaif-goose/goose/blob/main/crates/goose-provider-types/src/conversation/tool_request.rs)) |
| tool result | **role `user`** message: `{"type":"toolResponse","id":<same>,"toolResult":{"status":"success","value":{"content":[…],"isError":false}}}`, linked by `id` |
| permission | `{"type":"actionRequired","data":{"actionType":"toolConfirmation"}}` means waiting for the user |
| subagent | the `delegate` tool creates a child session with `session_type='sub_agent'` and `parent_session_id` ([summon.rs](https://github.com/aaif-goose/goose/blob/main/crates/goose/src/agents/platform_extensions/summon.rs)). The parent's toolResponse `_meta.subagent_session_id` holds the link. The subagent is done when the parent toolResponse arrives. |
| tokens / model | `metadata_json.usage{inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens, cost}`, `metadata_json.inference{provider, requestedModel, resolvedModel}`. Also `usage_ledger` |
| turn finished | no marker. The newest assistant message has no `toolRequest`. |

```jsonc
// messages.content_json (assistant)
[{"type":"thinking","thinking":"<redacted>","signature":"<sig>"},{"type":"text","text":"Let me look at the tests."},
 {"type":"toolRequest","id":"toolu_01AB","toolCall":{"status":"success","value":{"name":"developer__shell","arguments":{"command":"ls tests"}}}}]
// messages.content_json (role=user, tool result)
[{"type":"toolResponse","id":"toolu_01AB","toolResult":{"status":"success","value":{"content":[{"type":"text","text":"<redacted>"}],"isError":false}}}]
```

**Hooks:** Open Plugins hooks (`~/.agents/plugins/<p>/hooks/hooks.json`). Events: `SessionStart, SessionEnd, Stop, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, AfterFileEdit, BeforeShellExecution, AfterShellExecution, …` ([hooks.md](https://github.com/aaif-goose/goose/blob/main/documentation/docs/guides/context-engineering/hooks.md)).

**Confidence: high** (source).

---

## 10. Aider (Aider-AI/aider) [web]

### Paths

Same on all OSes ([aider/args.py](https://github.com/Aider-AI/aider/blob/main/aider/args.py)):
* `<git root>/.aider.chat.history.md` (`--chat-history-file`, `AIDER_CHAT_HISTORY_FILE`, `.aider.conf.yml`)
* `<git root>/.aider.input.history` (prompt_toolkit format)
* optional `--llm-history-file` (raw prompts and responses)
* optional `--analytics-log FILE` (JSONL events)

**There is no global index.** The visualizer has to scan configured folders for `.aider.chat.history.md`. Don't read `~/.aider/` (caches, possibly keys).

### Storage shape

Append-only Markdown ([aider/io.py](https://github.com/Aider-AI/aider/blob/main/aider/io.py) `append_chat_history`). Tail by byte offset.

| Need | Where |
|---|---|
| session start / id | `# aider chat started at YYYY-MM-DD HH:MM:SS` (local time, no timezone). Session id = path + header time |
| project | the folder containing the file |
| user prompt | lines starting `#### ` |
| assistant text | a plain Markdown block, **written only after the response finishes** (no streaming) |
| reasoning | between `► **THINKING**` and `► **ANSWER**` ([reasoning_tags.py](https://github.com/Aider-AI/aider/blob/main/aider/reasoning_tags.py)) |
| tool calls | none. SEARCH/REPLACE edit blocks; `> Applied edit to <path>`, `> Commit <hash> <msg>` |
| tokens / cost | `> Tokens: 5.2k sent, 1.1k cache hit, 310 received. Cost: $0.02 message, $0.15 session.` Exact numbers only appear in the analytics `message_send` event |
| model | `> Main model: <model> with <format> edit format` |
| subagent | none (architect and editor replies go to the same file) |
| turn finished | the `> Tokens:` line after an assistant block. A `#### ` line with nothing after it means a turn is running |

````markdown
# aider chat started at 2026-10-18 10:12:03

> Main model: <model> with diff edit format
#### add a --verbose flag to cli.py

I'll add the flag.
…SEARCH/REPLACE block…

> Tokens: 5.2k sent, 310 received. Cost: $0.02 message, $0.02 session.
> Applied edit to cli.py
````

**Hooks:** none. `--notifications-command CMD` runs when a response is ready.

**Confidence: high** for the format, but there is **little structure**: you get prompt, answer, edits and tokens, without timing.

---

## 11. Crush (charmbracelet/crush) [web]

### Paths

* **One DB per project**: `<data_dir>/crush.db`. `data_dir` comes from `options.data_directory` (default `.crush`: the nearest existing `.crush` above the cwd, else `<cwd>/.crush`). `--data-dir`/`-D` overrides it. ([internal/config/load.go](https://github.com/charmbracelet/crush/blob/main/internal/config/load.go), [internal/db/connect.go](https://github.com/charmbracelet/crush/blob/main/internal/db/connect.go))
* **Global project index** `projects.json` (rewritten whole): `$CRUSH_GLOBAL_DATA`, else `$XDG_DATA_HOME/crush`, else `~/.local/share/crush/projects.json` (Linux and macOS); `%LOCALAPPDATA%\crush\projects.json` (Windows). Shape: `{"projects":[{"path","data_dir","last_accessed"}]}` ([internal/projects/projects.go](https://github.com/charmbracelet/crush/blob/main/internal/projects/projects.go))

### Storage shape

SQLite with **WAL**, `busy_timeout=30000` ([migrations](https://github.com/charmbracelet/crush/tree/main/internal/db/migrations)).
* `sessions(id, parent_session_id, title, message_count, prompt_tokens, completion_tokens, cost, created_at, updated_at, summary_message_id, todos)`
* `messages(id, session_id, role /* user|assistant|system|tool */, parts JSON, model, provider, created_at, updated_at, finished_at /* unix s */, is_summary_message)`
* `files`: snapshots holding file **contents**. Skip it.

Assistant rows are **created empty and UPDATEd in place while streaming** ([internal/message/message.go](https://github.com/charmbracelet/crush/blob/main/internal/message/message.go)). **Tail:** read `projects.json`, open each `crush.db` read-only, watch its `-wal`, poll `messages WHERE updated_at >= :wm` (1-second resolution, so use `>=`), and diff by `(id, hash(parts))`.

### Fields

`parts` = `[{"type", "data"}]` ([internal/message/content.go](https://github.com/charmbracelet/crush/blob/main/internal/message/content.go)).

| Need | Where |
|---|---|
| project | `projects.json` `path` for that `data_dir`. No cwd column. |
| user / assistant text | part `text {text}` |
| thinking | part `reasoning {thinking, signature, started_at, finished_at}` |
| tool call | assistant part `tool_call {id, name, input (JSON string), finished}` |
| tool result | separate **role `tool`** message, part `tool_result {tool_call_id, name, content, is_error}`. Link by `tool_call_id`. |
| subagent | `agent` / `agentic_fetch` tools create a child session `id = "<assistantMsgId>$$<toolCallId>"`, `parent_session_id` ([coordinator.go](https://github.com/charmbracelet/crush/blob/main/internal/agent/coordinator.go)). It is done when the parent's `tool_result` arrives. Hide `title-<parent>` sessions (title generation). |
| tokens | **session level only**: `sessions.prompt_tokens`, `completion_tokens`, `cost` |
| model | `messages.model`, `messages.provider` |
| turn finished | part **`finish {reason, time}`**: `end_turn` means waiting for the user; `tool_use` means continuing; others are `max_tokens, canceled, error`. No `finish` part and `finished_at` NULL means still streaming. |

```jsonc
// messages.parts (assistant)
[{"type":"reasoning","data":{"thinking":"<redacted>","started_at":1760781000,"finished_at":1760781003}},
 {"type":"text","data":{"text":"Let me read main.go."}},
 {"type":"tool_call","data":{"id":"toolu_01XY","name":"view","input":"{\"file_path\":\"/home/user/proj/main.go\"}","finished":true}},
 {"type":"finish","data":{"reason":"tool_use","time":1760781005}}]
// role=tool
[{"type":"tool_result","data":{"tool_call_id":"toolu_01XY","name":"view","content":"<redacted>","is_error":false}},{"type":"finish","data":{"reason":"stop","time":0}}]
```

**Hooks:** only `PreToolUse` (`hooks` in crush.json; env `CRUSH_EVENT, CRUSH_TOOL_NAME, CRUSH_SESSION_ID, CRUSH_CWD`; [docs/hooks/README.md](https://github.com/charmbracelet/crush/blob/main/docs/hooks/README.md)).

**Confidence: high** (source).

---

## 12. Cursor (IDE agent and cursor-agent CLI) [web]

Not installed here. Cursor publishes no schema, so this section is pieced together from Cursor staff forum posts, the official hooks docs, and third-party parsers. A useful reference for many harnesses is agentsview's inventory: https://github.com/kenn-io/agentsview/blob/main/docs/internal/session-format-sources.md. Cursor staff say: "Transcripts are stored locally in `%USERPROFILE%\.cursor\projects\<project>\agent-transcripts\`, and chat metadata is in `state.vscdb`. There is no server copy." (https://forum.cursor.com/t/chat-history-gone-after-pc-restart-agent-transcripts-files-emptied-how-to-recover/158251/5)

### Paths

| Store | Linux | macOS | Windows |
|---|---|---|---|
| **Agent transcripts** (IDE and CLI) | `~/.cursor/projects/<slug>/agent-transcripts/` | same | `%USERPROFILE%\.cursor\projects\<slug>\agent-transcripts\` |
| IDE global DB (composer chats) | `~/.config/Cursor/User/globalStorage/state.vscdb` | `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb` | `%APPDATA%\Cursor\User\globalStorage\state.vscdb` |
| IDE workspace DB | `…/Cursor/User/workspaceStorage/<hash>/state.vscdb` and `workspace.json` (`{"folder":"file:///…"}`) | same | same |
| cursor-agent CLI store | `~/.cursor/chats/<workspace-hash>/<agent-id>/store.db` (and `-wal`) | same | `%USERPROFILE%\.cursor\chats\…` |
| Hooks config | `~/.cursor/hooks.json`, `<project>/.cursor/hooks.json`, `/etc/cursor/hooks.json` | `/Library/Application Support/Cursor/hooks.json` | `C:\ProgramData\Cursor\hooks.json` |

Cursor documents no environment variable that moves `~/.cursor`. As a VS Code fork, the IDE's `--user-data-dir` and portable mode move `User/` (VS Code behaviour; not documented by Cursor).

### Storage shape and tail strategy

* **Transcripts (preferred)**: append-only JSONL with Anthropic-style messages. **Tail by byte offset.** Layouts: IDE `agent-transcripts/<id>/<id>.jsonl`, CLI `agent-transcripts/<id>.jsonl`, subagent `agent-transcripts/<parent-id>/subagents/<child-id>.jsonl`. Very old builds wrote `.txt`. Per Cursor staff (https://forum.cursor.com/t/accessing-the-full-agent-transcript-in-cursor/157311), transcripts contain user messages, assistant text and tool call inputs, but **intentionally not tool outputs**. Users can turn transcripts off. Sources disagree on whether the IDE writes them, so watch both stores.
* **IDE `state.vscdb`**: SQLite. The `cursorDiskKV(key, value)` table holds `composerData:<composerId>` (chat) and `bubbleId:<composerId>:<bubbleId>` (message). Cursor 3.x adds a `composerHeaders` table (`composerId, workspaceId, createdAt, lastUpdatedAt, isSubagent, …`). The DB **can grow to tens of GB** (forum: 1.3M `bubbleId` rows), and a single message can exceed 13 MB, so always look rows up by key and never scan. Journal mode isn't documented, so check for `-wal` at runtime. Open with `mode=ro` and a busy_timeout, and expect `SQLITE_BUSY`. The DB also holds login and extension state, so select only `composerData:%` and `bubbleId:%`. Tail strategy: poll `composerData:<id>` for active chats (`lastUpdatedAt`) and diff `fullConversationHeadersOnly[]` to find new bubble ids.
* **CLI `store.db`**: tables `meta(key,value)` and `blobs(id,data)`, a content-addressed **protobuf** graph with no published `.proto`, optionally encrypted. **Don't tail it**; use the transcripts.

### Project mapping

The transcript `<slug>` is the absolute path with the leading separator dropped and the other separators turned into `-` (`Users-alice-dev-myproj`), which is lossy. Recover the path by matching it against the filesystem (agentsview `internal/parser/cursor_paths.go`), or use hook `workspace_roots` / IDE `composerData.workspaceIdentifier.uri.fsPath`.

### Fields

**Transcript JSONL**

| Need | Where |
|---|---|
| session id | file stem |
| timestamp | none per line. The user text starts with `<timestamp>Monday, Oct 13, 2026, 9:41 AM (UTC+2)</timestamp>`. Otherwise use file mtime. |
| user prompt | `role:"user"`, text inside `<user_query>…</user_query>` |
| assistant text / thinking | `role:"assistant"` `content[]` `text` / `thinking` blocks (often `[REDACTED]`) |
| tool call | `tool_use {name, input}`. **`id` is always null.** |
| tool result | **not stored** |
| subagent | `tool_use name:"Subagent"` `input{description, prompt, subagent_type, run_in_background}`. The child file is in `<parent>/subagents/`. |
| tokens / model | none |
| **turn finished** | line `{"type":"turn_ended","status":"completed"\|"error"}` |

**IDE bubble JSON** (`bubbleId:` values)

| Need | Where |
|---|---|
| order | `composerData.fullConversationHeadersOnly[] {bubbleId, type}` |
| timestamp | bubble `createdAt` (ISO; written when the step **finishes**) |
| user / assistant | `type:1` user `text`; `type:2` assistant `text` |
| thinking | `thinking.text`, `thinkingDurationMs` |
| tool call and result | `type:2`, `capabilityType:15`, `toolFormerData{tool /* numeric id */, name, toolCallId, status:"completed"\|"loading"\|"error"\|"cancelled", rawArgs, params, result}`. Call and result live in the same bubble. Ignore bubbles whose `toolFormerData` lacks `tool`/`name`. Stable numeric ids: 15 run_terminal_cmd, 38 edit, 40 read_file, 41 grep, 48 task_v2 (subagent), 51 ask_question. |
| subagent | `composerData.subagentComposerIds[]`; the child has `subagentInfo{toolCallId, forkedFromComposerId}` |
| tokens | `tokenCount{inputTokens, outputTokens}`, which is non-zero only on the last bubble of a turn |
| model | `modelInfo.modelName` / `composerData.modelConfig.modelName` |
| turn finished | `composerData.status` (`"completed"`…) and empty `generatingBubbleIds[]` (inferred; check on a real install) |

```jsonc
// agent-transcripts/<id>.jsonl
{"role":"user","message":{"content":[{"type":"text","text":"<timestamp>Monday, Oct 13, 2026, 9:41 AM (UTC+2)</timestamp>\n<user_query>[REDACTED]</user_query>"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"[REDACTED]"},{"type":"tool_use","id":null,"name":"Subagent","input":{"description":"…","prompt":"…","subagent_type":"explore"}}]}}
{"type":"turn_ended","status":"completed"}
// cursorDiskKV bubbleId:<composerId>:<bubbleId>
{"_v":3,"type":2,"bubbleId":"…","createdAt":"2026-10-13T07:42:12.281Z","capabilityType":15,"modelInfo":{"modelName":"…"},"tokenCount":{"inputTokens":0,"outputTokens":0},
 "toolFormerData":{"tool":40,"name":"read_file","toolCallId":"toolu_…","status":"completed","rawArgs":"{\"path\":\"/home/user/proj/src/x.rs\"}","result":"{…}"}}
```

### Hooks

Official (https://cursor.com/docs/agent/hooks). Events: `sessionStart, sessionEnd, beforeSubmitPrompt, preToolUse, postToolUse, postToolUseFailure, subagentStart, subagentStop, beforeShellExecution, afterShellExecution, beforeMCPExecution, afterMCPExecution, beforeReadFile, afterFileEdit, preCompact, stop` (`status: completed|aborted|error`), `afterAgentResponse, afterAgentThought`. Every hook receives `conversation_id, generation_id, model, workspace_roots, transcript_path, user_email, hook_event_name`. A `postToolUse` hook is the only way to capture tool outputs.

**Confidence: medium** for transcripts. **Medium-low** for `state.vscdb` (reverse-engineered, versioned with `_v`). **Low** for the CLI `store.db`.

---

## 13. Amp (Sourcegraph / ampcode) [web]

### Paths

`~/.local/share/amp/threads/T-<uuid>.json` (one file per thread), `~/.local/share/amp/session.json` (`{lastThreadId}`), `~/.local/share/amp/history.jsonl` (prompts). Some macOS installs put these under `~/.config/amp/`, so watch both. **`secrets.json` holds credentials: never open it.** Settings live in `~/.config/amp/settings.json(c)` (https://ampcode.com/docs/cli/settings). The Windows data path is unverified. There is no documented override. Sources: `illegalstudio/lazyagent` `internal/amp/process.go`, agentsview `internal/parser/amp.go`.

### Shape and feasibility

Each thread is one JSON file **rewritten whole**, so re-read it on change and diff `messages[]` by `messageId`. **Big caveat:** threads are stored on Amp's servers ("A thread is not tied to the device you started it on", https://ampcode.com/docs/threads). agentsview marks Amp as deprecated because "current Amp releases may keep full threads server-side and leave only local stubs". lazyagent's workaround calls `amp threads export` over the network, which a passive visualizer shouldn't do. **Passive tailing is unreliable for current Amp.** Use `history.jsonl` and `session.json` as activity hints.

### Fields (thread JSON)

| Need | Where |
|---|---|
| session id / title | `id` (`T-…`), `title`, `created` (ms) |
| cwd | `env.initial.trees[0].uri` (`file://` URI), `.repository{url, ref, sha}` |
| timestamp | `messages[].meta.sentAt` (ms), `usage.timestamp` |
| user prompt | `role:"user"`, `content[]` with `type:"text"` |
| assistant text / thinking | `role:"assistant"`, `text` / `thinking` blocks |
| tool call | `{type:"tool_use", id, name, input, complete}` |
| tool result | in a **user** message: `{type:"tool_result", toolUseID, run:{status:"done"\|"error"\|"cancelled"\|"running"\|"pending", result}}`. Link via **`toolUseID`**. |
| tokens / model | assistant `usage{model, inputTokens, outputTokens, cacheCreationInputTokens, cacheReadInputTokens, totalInputTokens, maxInputTokens}` |
| turn finished | assistant `state{type, stopReason}`: `stopReason:"tool_use"` means still working; anything else means idle |
| subagents | oracle, librarian and Task subagents are **not** saved as separate threads |

```jsonc
{"v":1,"id":"T-0199…","created":1760000000000,"title":"[title]",
 "env":{"initial":{"trees":[{"displayName":"proj","uri":"file:///home/user/proj","repository":{"type":"git","ref":"refs/heads/main"}}]}},
 "messages":[{"role":"user","messageId":0,"content":[{"type":"text","text":"[REDACTED]"}],"meta":{"sentAt":1760000001000}},
  {"role":"assistant","messageId":1,"content":[{"type":"thinking","thinking":"[REDACTED]"},{"type":"tool_use","id":"toolu_…","name":"Bash","input":{"cmd":"…"},"complete":true}],
   "state":{"type":"complete","stopReason":"tool_use"},"usage":{"model":"claude-…","inputTokens":12,"outputTokens":340,"cacheReadInputTokens":18000}},
  {"role":"user","messageId":2,"content":[{"type":"tool_result","toolUseID":"toolu_…","run":{"status":"done","result":{"output":"[REDACTED]","exitCode":0}}}]}]}
```

**Hooks:** the plugin API (`~/.config/amp/plugins/`, Bun): `amp.on("session.start"|"agent.start"|"agent.end"|"tool.call"|"tool.result")` (https://ampcode.com/manual/plugin-api). For current Amp this is the most reliable live channel, but the user has to install it. `amp -x --stream-json` only covers runs you launch yourself.

**Confidence: low to medium.** The local format is known, but whether the files exist depends on the Amp version.

---

## 14. Factory Droid [web]

### Paths

`~/.factory/sessions/<encoded-cwd>/<sessionId>.jsonl` plus `<sessionId>.settings.json`, the same on all OSes (`%USERPROFILE%\.factory` on Windows). Older sessions may sit directly in `~/.factory/sessions/*.jsonl`, and some builds write `~/.factory/projects/<slug>/<id>.jsonl`, so dedupe by session id. Indexes: `~/.factory/cache/session-discovery-index.json` (`entries{id:{title, cwd, createdTimeMs, modifiedTimeMs, messageCount}}`) and the older `~/.factory/sessions-index.json`. Override: **`FACTORY_HOME_OVERRIDE`** → `$FACTORY_HOME_OVERRIDE/.factory` (yetone/magpie `internal/sessions/droid.go`). **`~/.factory/.env` and `auth*` files hold secrets.** Sources: cli-continues `src/parsers/droid.ts`, Factory's session-navigation skill (https://skills.cat/skills/factory-ai/factory-plugins/session-navigation). Factory publishes no official schema.

### Shape

The JSONL is **append-only**, so tail it by byte offset. Line `type`s: `session_start` (first line), `message`, `todo_state`, `compaction_state`, and possibly `agent_turn_outcome` (one source, unverified). `.settings.json` is rewritten, so re-read it on change.

### Project mapping

Use `session_start.cwd`. The folder name is the cwd with `/` turned into `-`, which is lossy.

### Fields

| Need | Where |
|---|---|
| session id | `session_start.id` (file stem); `title` |
| timestamp | `message.timestamp` (ISO); `id`, `parentId` |
| user prompt | `type:"message"`, `message.role:"user"`, `text` blocks. **Skip `message.visibility:"user_only"\|"llm_only"`**, which are injected reminders and summaries. |
| assistant text / thinking | `role:"assistant"` `text` / `thinking` blocks (OpenAI models: `openaiEncryptedContent`, which is opaque) |
| tool call | `{type:"tool_use", id, name, input}`. Names include `Read, LS, Glob, Grep, Execute, Edit, MultiEdit, Create, ApplyPatch, TodoWrite, Task, WebSearch, FetchUrl` |
| tool result | **user** message `{type:"tool_result", tool_use_id, content}`; link by `tool_use_id` |
| subagent | `Task` tool call with `input.subagent_type`. The child session's `session_start.callingSessionId` = the parent id. It is done when the parent's `tool_result` arrives. |
| tokens / model | **only in `.settings.json`**: `tokenUsage{inputTokens, outputTokens, cacheCreationTokens, cacheReadTokens, thinkingTokens}` (running session total, including subagents), plus `model`, `reasoningEffort`, `autonomyMode` |
| turn finished | no documented marker. Heuristic: the last assistant message has no `tool_use`. Possibly `agent_turn_outcome`. |

```jsonl
{"type":"session_start","id":"0dce4686-…","title":"[title]","version":2,"cwd":"/home/user/proj","owner":"[REDACTED]"}
{"type":"message","id":"m1","timestamp":"2026-10-13T07:40:00.000Z","message":{"role":"user","content":[{"type":"text","text":"[REDACTED]"}]}}
{"type":"message","id":"m2","parentId":"m1","timestamp":"…","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_…","name":"Read","input":{"file_path":"/home/user/proj/src/x.rs"}}]}}
{"type":"message","id":"m3","parentId":"m2","timestamp":"…","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_…","content":"[REDACTED]"}]}}
```

**Hooks:** Claude-Code-compatible (https://docs.factory.ai/reference/hooks-reference): `PreToolUse, PostToolUse, UserPromptSubmit, Notification, Stop, SubagentStop, PreCompact, SessionStart, SessionEnd`. Each hook gets `session_id, transcript_path, cwd, hook_event_name`. Config lives in `~/.factory/hooks.json` or `.factory/settings.json`.

**Confidence: medium** (several third-party parsers agree; no official schema).

---

## Harnesses skipped or partially covered

* **Windsurf (Cascade)** was not requested. It is mentioned only as a VS Code globalStorage variant for the Cline/Roo/Kilo extensions. Its own Cascade history is not covered.
* **Amp**: covered, but current versions may keep no local thread data.
* **cursor-agent CLI `store.db`**: protobuf with no published schema. Use the transcripts instead.

# Changelog

## Unreleased

### Fixed
- Builds with the newest stable Rust when warnings are errors.

## 0.4.0 (2026-10-21)

### Added
- **Needs you.** An agent waiting on a permission prompt or your answer raises its hand and shows what it is asking. The crew bar counts who is waiting, and one click goes to the next. A desktop notification reaches you while ShipOffice is in the background.
- **Stuck and failing agents.** An agent that should be working but has done nothing for a while slumps with z's over its head (after 8 minutes, adjustable). A failed tool call puffs red.
- **Context you can see.** The ring under each agent fills as its context fills and turns red near compaction. When the context is compacted, the agent tidies its desk.
- **Teams.** Helpers started together, such as one phase of a workflow, sit together and meet at the whiteboard when the phase is done.
- **Find anyone.** The crew list searches by name, task or team, filters to who needs you, is stuck, failed or is near compaction, and jumps to whoever you pick. Big floors get a minimap.
- **Replay.** Play back the last hour of a session at 1× to 64×, with a scrubber. Esc goes back to live.
- **Your own content.** Add characters with their own animations, give the built-in crew new animations, and swap props for your own art, each with a switch in settings. See [docs/custom.md](docs/custom.md) and the example in `examples/content`.
- `shipoffice --version`.

### Changed
- Tool bubbles are larger and solid, grow as you zoom, are cut between words and never overlap. In a crew of more than 24, only the bubbles that matter show.
- Characters wear the clothes they were drawn in. Agents are told apart by the colour of their ring, and characters are spread evenly through the crew.

### Fixed
- Outfit colours no longer leak into beards and into other characters' clothes.
- Sprites stay sharp: zoom stops at the art's full resolution.

## 0.3.0 (2026-10-20)

### Added
- **The office grows with the crew.** A crew that outgrows its desks gets the same room wider and deeper, in steps up to 512 desks, with a longer lounge and everyone carried over. The camera zooms in further on big floors, and newcomers arrive faster in a crowd.

### Changed
- Each agent's context is measured against the window it really has: the one the harness logs, the one you set (Claude Code's `autoCompactWindow`, per model or for all; Codex's `model_context_window`), the one its last automatic compaction implies, or the model's own (1M for current Claude models). It is never less than what the agent has already carried, and each helper is measured on its own model.
- Compactions are counted for every harness: from the log where it records them, otherwise from the context suddenly halving. Goose's compaction settings are read.

### Performance
- Large crews run faster: quicker lookups for hiring and floor space, fewer allocations per frame, at most 18 bubbles on screen, portraits decoded once, and only the latest 64 finished helpers kept per session.

## 0.2.0 (2026-10-19)

### Changed
- **Filmed animation.** Walking, typing, reading, sipping coffee, ping pong and lounge games come from video of each character, with turns blended between five filmed directions and sharper sheets on HiDPI screens.
- **Tidier lounges.** Ping pong at the back, the cafe in the middle, the sofa on its rug at the front. One or two games per office besides ping pong, from 11, each with clear floor around it. A flip scoreboard stands by the ping pong table, and the ball meets the paddle on every hit.
- The coffee machine, water cooler and server face into the room. Rooms are 10 to 12 tiles deep.
- The README leads with a still of the office; the 60 fps video is one click away.

## 0.1.0 (2026-10-18)

First release.

- A procedural isometric office for every live agent session. Subagents arrive, work, take breaks and leave.
- Reads the logs of 15 harnesses: Claude Code, Codex, OpenCode, Kilo Code, Gemini CLI, Qwen Code, Copilot CLI, Cline, Roo Code, Goose, Crush, Factory Droid, Cursor, Amp and Aider. Read-only, and nothing is sent over the network.
- Pan, turn, tilt and zoom from any side, with optional snapping to the classic corners.
- A city backdrop by day or by night, with the skyline, the street below and the floors of the tower under yours.
- An inspector for each agent: tokens, tool calls, and context against your compaction setting.
- Tabs per session, a whiteboard of live stats, a live feed, and a demo crew when nothing is running.
- **Locked in:** a card of what your agents are doing, saved as a PNG with a caption to post.
- Themes, accent and custom colours, office styles, and a borderless HUD.
- Builds for Linux and Windows.

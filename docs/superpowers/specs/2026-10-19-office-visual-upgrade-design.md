# Office visual upgrade

The crew used to move like paper dolls: a few hand-made frames per pose, stiff turns, one lounge
layout with a fixed set of toys. This upgrade makes the people move like they were filmed, makes
every office's lounge its own, and keeps the whole thing sharp on HiDPI screens.

## Decisions (the owner's picks)

| Topic | Pick | What it means |
|---|---|---|
| Walking | video | Walk cycles come from short generated videos of each character walking on the spot, not hand-drawn frames. |
| Turning | blend | Five directions are filmed (front, front diagonal, side, back diagonal, back) and the other three mirrored; near the angle between two of them both are drawn, cross-faded. |
| Resolution | hidpi | Video sheets are stored at about 1.5x the old sheets (214 px standing height) and drawn scaled down. |
| Ping pong | board | A flip scoreboard next to the table shows the score, its cards flipping on each point. |
| Layout | zones | The lounge is laid out in zones: coffee bar on the back wall, ping pong with its scoreboard, a cafe table, a sofa on its rug, and the games, with a clear path to everything. |
| Size | 6 columns | The lounge is six tiles wide. |
| Games | all 11, 2-4 per office | Foosball, pinball, air hockey, chess, console, VR, claw machine, board game, guitar, hoops and pool all exist; each office rolls 2 to 4 of them from its seed. |

A later request from the owner: everything else the crew does (typing, reading, sipping coffee,
ping pong, playing games) is filmed the same way as walking, and walking eases in and out more
smoothly.

## What was built

- **Clips.** Each of the seven characters was filmed against magenta: walking in five directions,
  guitar on a stool, pool strokes, typing and reading at a desk, sipping a coffee on an unseen
  seat, a ping pong stroke from each end of the table, and playing a game standing at a table
  (front diagonal) and at a machine (back diagonal). Pose stills were made first and each clip
  starts and ends on its still, so it loops.
- **`art/pipeline/video.py`** turns clips into sheets: finds the stretch where the motion repeats,
  cross-fades the seam, keys the magenta, removes drift and stray specks (including the ping pong
  ball the model kept adding), scales every figure to one standing height and packs a sheet with a
  recolour mask, so outfits keep their per-agent colours.
  - Desk clips include the desk and chair; the empty desk is found in the clip by silhouette fit
    and the sheet is framed so it lands exactly on the desk it replaces. The desk itself is left
    out of the recolour mask.
  - Stroke clips hold exactly one stroke, rotated to start at the hit, and record where the paddle
    is at that moment.
- **`art/pipeline/games.py`** cuts the generated game props into one atlas, and finds the
  scoreboard's two blank flip cards so the app can draw the score on them.
- **Engine.** `scene.rs` draws the filmed sheets (falling back to the old ones if a sheet is
  missing), plays desk and game loops at 12 fps and walks and strokes at 24 fps, and lines up each
  stroke's hit frame with the player's turn in the rally; the ball flies to where the paddle
  actually is in that character's clip. Sheets upload to the GPU lazily under a memory budget.

## Non-goals

- No new behaviour for the crew beyond the new games; who goes where is unchanged.
- No real people or real work in any clip, still or the README video.

## Left open

- The back player's paddle is at table height behind the table, so it is mostly hidden by the
  table top; the ball still visibly reaches the far edge. Drawing that player's arm over the table
  would need splitting the sheet and was not worth it.
- VR uses the standing pose, turning slowly as if looking around; it was not filmed.

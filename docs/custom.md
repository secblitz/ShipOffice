# Your own characters, animations and props

ShipOffice reads extra content from the folder its config file is in. That is `~/.config/shipoffice/` on Linux, `~/Library/Application Support/dev.shipoffice.ShipOffice/` on macOS and `%APPDATA%\shipoffice\ShipOffice\config\` on Windows. **Settings > Office > Open content folder** creates the folder and opens it.

```
characters/
  robot/
    character.toml
    robot.png
    walk.png
props/
  plant.png
```

Content is read when ShipOffice starts. Each character and prop then gets a switch under **Settings > Office > Your content**, and anything that could not be read is listed there with the reason.

[`examples/content`](../examples/content) has a complete example: a robot with a walk and a dance, and a plum-leaved plant. To try it, copy `characters` and `props` into your content folder and restart ShipOffice.

## A character

A character is a folder under `characters/` with a `character.toml` and its pictures. PNG and WebP work, with transparency.

```toml
name = "Robot"
# px from the soles to the top of the head in your pictures: sets its size beside the crew
height = 240

# a still picture, for anything the animations below do not cover
image = "robot.png"

[anims.walk]
file = "walk.png"   # one sheet: frames left to right, then the next row
frames = 6
cols = 6            # frames per row (default: all in one row)
fps = 10            # default 12
```

Each frame of a sheet is the same size, with the character's feet at the bottom centre. A sheet can give its own `height` when the character is drawn at another size in it.

### The animations

| Name | When it plays | If it is missing |
|---|---|---|
| `idle` | standing, facing you | `image` (one of the two is required) |
| `walk` | walking toward you | `idle` |
| `walkback` | walking away | `walk` |
| `type` | writing at a desk | `read`, then `sit`, then `idle` |
| `read` | reading at a desk | `type`, then `sit`, then `idle` |
| `sit` | sitting in the lounge or at a table | `type`, then `idle` |
| `ppfront`, `ppback` | ping pong, near and far side of the table | `walk`, `walkback` |

All pictures face front-left, the way the built-in crew stands at the classic angle. When the view turns, they are mirrored.

At a desk, the office draws the desk and chair around your character: the chair behind it and the desk in front. If your `type` and `read` sheets draw their own desk, set `desk = true`.

For the full filmed look, a character can also bring the sheets the built-in crew uses: `stand8` (one frame per direction: front, front-right, right, back-right, back) and `walk8_s`, `walk8_se`, `walk8_e`, `walk8_ne` and `walk8_n` for walking in five directions; `typing` and `reading` at a desk, desk included; `sipping` on a seat; `guitar`, `pool`, `play_se` and `play_ne` for the lounge games; and `swing_front` and `swing_back` for ping pong strokes.

### Extra animations

Any other animation plays as an extra when you say when:

```toml
[anims.dance]
file = "dance.png"
frames = 8
cols = 4
fps = 10
when = "finish"   # when its work is done; "idle" plays it now and then while it stands about
```

### New animations for the built-in crew

To change how one of the built-in crew moves, add a folder with `replaces` and only the animations you want to change; the rest stay as they are. The built-in crew is filmed, so give the filmed sheets (`stand8`, `walk8_s`, `typing` and the others above) to change what it already does, or extras to give it something new.

```toml
replaces = "hoodie"   # hoodie, bun, beanie, buzz, silver, red or curls
height = 214

[anims.cheer]
file = "cheer.png"
frames = 12
cols = 6
when = "finish"
```

### Who is hired

New agents are given the character fewest people in the office already have, from the built-in crew and yours. To have only your own, switch **Built-in crew** off. To make one character always the lead, set it in `config.toml`:

```toml
[custom]
lead = "robot"   # a folder name, or one of the built-in names
```

## A prop

Put a picture named after a prop in `props/`, and it takes that prop's place in every office, at the same size. Draw it at the angle the built-in one has; any resolution works as long as the proportions match.

The props: `arcade`, `beanbag`, `bookshelf`, `coffee`, `cooler`, `fridge`, `lamp`, `pingpong`, `plant`, `server`, `sofa` and `whiteboard`; the lounge's `table`, `chairL`, `chairR` and `lowtable`; and the games: `airhockey`, `amp`, `boardgame`, `chess`, `claw`, `foosball`, `hoops`, `pinball`, `pool`, `tv` and `vr`. Props that stand against a wall also have a back, seen when the room is turned: `arcade_back`, `bookshelf_back`, `coffee_back`, `cooler_back`, `fridge_back` and `server_back`.

Each prop has a switch, so you can go back to the built-in one without deleting the file.

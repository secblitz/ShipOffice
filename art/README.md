# Art pipeline

Sprites are generated with Nano Banana 2 (via fal.ai) as 4×4 sheets on a magenta background,
then cleaned up by the scripts here. The generated sources live in `art/raw/` and are not in
git; the cleaned atlases in `/assets` are, so the app builds without them.

| Script | What it does |
| --- | --- |
| `pipeline/process.py raw_dir out_dir` | Keys the magenta, slices the grids, aligns frames, locks feet and desks in place, packs the atlases and writes `meta.json`. |
| `pipeline/charseg.py raw_dir` | Turns each character's AI-painted clothing map into exact per-pixel labels (`*_lab.png`). |
| `pipeline/masks.py out_dir` | Writes the recolor mask for every animation: 40 top, 80 hair, 120 pants, 160 skin, 200 accent. The app no longer repaints outfits; only the browser prototype in `art/preview/` uses the masks. |

Typical run from the repository root:

```sh
python3 -I art/pipeline/charseg.py art/raw
python3 -I art/pipeline/process.py art/raw assets
python3 -I art/pipeline/masks.py assets
```

`art/preview/` is the browser prototype the app was designed from. Serve it with any static
server (`python3 -m http.server -d art/preview`) and open `index.html`.

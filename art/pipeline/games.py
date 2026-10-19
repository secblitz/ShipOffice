"""Game props: cuts the generated prop sheets into one atlas, keyed and trimmed.

Each sheet is a grid on magenta with one prop per cell; a cell can hold a few pieces (a TV and its
controllers), so every blob of a fair size is kept, while specks and the grid lines are not. The
scoreboard's two blank flip cards are found and recorded, so the app can draw the score on them.

Usage: python3 -I games.py <sheets_dir> <out_dir>
  sheets_dir/propsA.png, propsB.png (2x2) and propsC.png (4x4)
"""
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image
from scipy import ndimage

SHEETS, OUT = Path(sys.argv[1]), Path(sys.argv[2])
# sheet, grid, row, col, name
CELLS = [
    ("propsA.png", 2, 0, 0, "foosball"), ("propsA.png", 2, 0, 1, "pinball"),
    ("propsA.png", 2, 1, 0, "airhockey"), ("propsA.png", 2, 1, 1, "claw"),
    ("propsB.png", 2, 0, 0, "pool"), ("propsB.png", 2, 0, 1, "hoops"),
    ("propsB.png", 2, 1, 0, "chess"), ("propsB.png", 2, 1, 1, "boardgame"),
    ("propsC.png", 4, 0, 0, "tv"), ("propsC.png", 4, 0, 1, "vr"),
    ("propsC.png", 4, 3, 0, "amp"), ("propsC.png", 4, 3, 3, "scoreboard"),
]
MAX_SIDE = 280  # stored at about 2.5x the size they are drawn, for HiDPI screens
WIDTH = 2048


def key_magenta(rgb):
    """The same key as process.py: soft edge, despill, strongly red or blue pixels kept."""
    a = rgb.astype(np.float32)
    r, g, b = a[..., 0], a[..., 1], a[..., 2]
    m = np.minimum(r, b) - g
    alpha = np.clip(1.0 - (m - 60.0) / 90.0, 0.0, 1.0)
    alpha = np.where(np.abs(r - b) > 110, 1.0, alpha)
    spill = np.clip(m, 0, None) * (alpha < 0.999) + np.clip(m - 25, 0, None) * (alpha >= 0.999)
    out = np.dstack([np.clip(r - spill * 0.85, 0, 255), g, np.clip(b - spill * 0.85, 0, 255), alpha * 255]).astype(np.uint8)
    out[..., 3][out[..., 3] < 24] = 0
    return out


def cut(sheet, grid, row, col):
    h, w = sheet.shape[:2]
    ch, cw = h // grid, w // grid
    inset = max(6, ch // 60)
    cell = sheet[row * ch + inset:(row + 1) * ch - inset, col * cw + inset:(col + 1) * cw - inset].copy()
    solid = cell[..., 3] > 40
    lab, n = ndimage.label(solid)
    if n == 0:
        return None
    sizes = ndimage.sum(solid, lab, range(1, n + 1))
    keep = np.isin(lab, 1 + np.nonzero(sizes >= sizes.max() * 0.015)[0])
    # thin strips along the cell's edge are what is left of the grid lines
    for side in (keep[:4], keep[-4:], keep[:, :4], keep[:, -4:]):
        side[:] = False
    keep = ndimage.binary_dilation(keep, iterations=2)
    cell[..., 3] = np.where(keep, cell[..., 3], 0)
    ys, xs = np.nonzero(cell[..., 3] > 40)
    im = Image.fromarray(cell).crop((xs.min(), ys.min(), xs.max() + 1, ys.max() + 1))
    s = min(1.0, MAX_SIDE / max(im.size))
    return im.resize((round(im.width * s), round(im.height * s)), Image.LANCZOS)


def panels(im):
    """The two blank cards of the scoreboard, left to right, as [x, y, w, h] in item px."""
    a = np.asarray(im).astype(int)
    light = (a[..., :3].min(-1) > 200) & (a[..., :3].max(-1) - a[..., :3].min(-1) < 40) & (a[..., 3] > 200)
    lab, n = ndimage.label(ndimage.binary_opening(light, iterations=2))
    sizes = ndimage.sum(light, lab, range(1, n + 1))
    best = sorted(np.argsort(sizes)[::-1][:2] + 1)
    out = []
    for k in best:
        sl = ndimage.find_objects((lab == k).astype(int))[0]
        out.append([sl[1].start, sl[0].start, sl[1].stop - sl[1].start, sl[0].stop - sl[0].start])
    return sorted(out)


def main():
    sheets = {}
    items = []
    for src, grid, row, col, name in CELLS:
        if src not in sheets:
            sheets[src] = key_magenta(np.asarray(Image.open(SHEETS / src).convert("RGB")))
        items.append((name, cut(sheets[src], grid, row, col)))
    x = y = shelf = 0
    rects = {}
    for name, im in items:
        if x + im.width > WIDTH:
            x, y, shelf = 0, y + shelf + 2, 0
        rects[name] = [x, y, im.width, im.height]
        x += im.width + 2
        shelf = max(shelf, im.height)
    atlas = Image.new("RGBA", (WIDTH, y + shelf), (0, 0, 0, 0))
    for name, im in items:
        atlas.paste(im, tuple(rects[name][:2]))
    atlas.save(OUT / "games.webp", "WEBP", quality=90, alpha_quality=100, method=6)
    meta = json.loads((OUT / "meta.json").read_text())
    meta["games"] = {"file": "games.webp", "w": atlas.width, "h": atlas.height, "items": rects,
                     "panels": panels(dict(items)["scoreboard"])}
    (OUT / "meta.json").write_text(json.dumps(meta, indent=1))
    print(json.dumps(meta["games"]))


if __name__ == "__main__":
    main()

"""Region masks for palette swapping (same idea the Godot shader will use).

Each animation atlas gets a grayscale PNG of the same size where the value encodes
the clothing region: 0 keep, 40 top, 80 hair, 120 pants, 160 skin, 200 accent (beanie, shirt).
The hero's masks come from colour rules; the other characters' come from their _lab.png labels.
Usage: python3 -I masks.py <out_dir>
"""
import json
import sys
from pathlib import Path

import numpy as np
from scipy import ndimage
from PIL import Image, ImageFilter

OUT = Path(sys.argv[1])
meta = json.loads((OUT / "meta.json").read_text())

TOP, HAIR, PANTS, SKIN, ACCENT = 40, 80, 120, 160, 200
LAB = {(0, 255, 0): TOP, (0, 0, 255): HAIR, (255, 255, 0): PANTS, (255, 0, 0): SKIN, (0, 255, 255): ACCENT}
# per animation: (hair band end, pants band start) as fractions of the frame's opaque height
BANDS = {"walk": (0.36, 0.60), "walkback": (0.40, 0.62), "sit": (0.36, 0.62), "type": (0.30, 0.62), "read": (0.30, 0.62), "search": (0.36, 0.60),
         "idle": (0.36, 0.60), "ppfront": (0.36, 0.60), "ppback": (0.40, 0.62)}


def largest_component(mask):
    from collections import deque
    h, w = mask.shape; seen = np.zeros_like(mask); best = []
    ys, xs = np.nonzero(mask)
    for y0, x0 in zip(ys, xs):
        if seen[y0, x0]:
            continue
        q = deque([(y0, x0)]); seen[y0, x0] = True; pts = []
        while q:
            y, x = q.popleft(); pts.append((y, x))
            for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                ny, nx = y + dy, x + dx
                if 0 <= ny < h and 0 <= nx < w and mask[ny, nx] and not seen[ny, nx]:
                    seen[ny, nx] = True; q.append((ny, nx))
        if len(pts) > len(best):
            best = pts
    out = np.zeros_like(mask)
    if best:
        yy, xx = zip(*best); out[list(yy), list(xx)] = True
    return out


def hsv(px):
    r, g, b = px[..., 0], px[..., 1], px[..., 2]
    mx, mn = px.max(-1), px.min(-1)
    d = mx - mn + 1e-6
    h = np.where(mx == r, ((g - b) / d) % 6, np.where(mx == g, (b - r) / d + 2, (r - g) / d + 4)) * 60
    return h, d / (mx + 1e-6), mx


def big_components(mask, min_px):
    from collections import deque
    h, w = mask.shape; seen = np.zeros_like(mask); out = np.zeros_like(mask)
    ys, xs = np.nonzero(mask)
    for y0, x0 in zip(ys, xs):
        if seen[y0, x0]:
            continue
        q = deque([(y0, x0)]); seen[y0, x0] = True; pts = []
        while q:
            y, x = q.popleft(); pts.append((y, x))
            for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                ny, nx = y + dy, x + dx
                if 0 <= ny < h and 0 <= nx < w and mask[ny, nx] and not seen[ny, nx]:
                    seen[ny, nx] = True; q.append((ny, nx))
        if len(pts) >= min_px:
            yy, xx = zip(*pts); out[list(yy), list(xx)] = True
    return out


for key, a in meta["anims"].items():
    img = np.asarray(Image.open(OUT / a["file"]).convert("RGBA")).astype(np.float32) / 255
    if a.get("char"):
        lab_path = OUT / a["file"].replace(".webp", "_lab.png")
        if not lab_path.exists():
            continue
        lab = np.asarray(Image.open(lab_path).convert("RGBA"))
        mask = np.zeros(img.shape[:2], np.uint8)
        for c, v in LAB.items():
            mask[(lab[..., :3] == c).all(-1) & (lab[..., 3] > 0) & (img[..., 3] > 0.5)] = v
        fn = a["file"].replace(".webp", "_mask.png")
        Image.fromarray(mask, "L").save(OUT / fn, optimize=True)
        a["mask"] = fn
        print(key, fn, {r: int((mask == r).sum()) for r in (TOP, HAIR, PANTS, ACCENT)})
        continue
    comp_path = OUT / a["file"].replace(".webp", "_companion.png")
    comp = np.asarray(Image.open(comp_path).convert("RGBA")).astype(np.float32) / 255 if comp_path.exists() else None
    mask = np.zeros(img.shape[:2], np.uint8)
    fw, fh = a["fw"], a["fh"]
    hair_end, pants_start = BANDS[key]
    for i in range(a["frames"]):
        x0, y0 = (i % a["cols"]) * fw, (i // a["cols"]) * fh
        cell = img[y0:y0 + fh, x0:x0 + fw]
        opaque = cell[..., 3] > 0.5
        ys = np.where(opaque.any(1))[0]
        if len(ys) == 0:
            continue
        top, bot = ys.min(), ys.max()
        rel = (np.arange(fh)[:, None] - top) / max(1, bot - top)
        rel = np.broadcast_to(rel, (fh, fw))
        h, s, v = hsv(cell[..., :3])
        m = np.zeros((fh, fw), np.uint8)
        hue_hi = 24 if key == "type" else 17  # typing: furniture is removed by the empty-desk reference below
        hue_lo = 3 if key.startswith("pp") else 0  # red paddles sit at hue 0-3 and must keep their colour
        hoodie = opaque & (h >= hue_lo) & (h <= hue_hi) & (s > 0.38) & (v > 0.2)
        skin = opaque & (h > 12) & (h < 40) & (s > 0.12) & (s < 0.45) & (v > 0.72)
        dark = opaque & (s < 0.4) & (v > 0.13) & (v < 0.45)
        m[big_components(hoodie, 30)] = TOP
        m[skin] = SKIN
        if comp is not None:
            # hair from the companion sheet, where the chair is orange and can't pass for hair
            cc = comp[y0:y0 + fh, x0:x0 + fw]; ch_, cs_, cv_ = hsv(cc[..., :3])
            cdark = (cc[..., 3] > 0.5) & (cs_ < 0.4) & (cv_ > 0.13) & (cv_ < 0.45)
            m[largest_component(cdark & (rel < hair_end)) & opaque] = HAIR
        else:
            m[largest_component(dark & (rel < hair_end))] = HAIR
        m[dark & (rel > pants_start)] = PANTS
        # close speckle holes: an unassigned opaque pixel joins the region most of its neighbours belong to
        for _ in range(3):
            for reg in (TOP, HAIR, PANTS, SKIN):
                r_ = (m == reg).astype(np.int32)
                n = sum(np.roll(np.roll(r_, dy, 0), dx, 1) for dy in (-1, 0, 1) for dx in (-1, 0, 1) if dy or dx)
                fill = (m == 0) & opaque & (n >= 5) & (v > 0.12)
                m[fill] = reg
        mask[y0:y0 + fh, x0:x0 + fw] = m
    if key in ("type", "read"):
        # typing has a desk and chair in the frame, so the colour rules get three extra checks:
        #  - each region must lie within a few px of its colour in the AI-painted segmentation map (coarse gate)
        #  - hair uses the sheet's own dark pixels near the companion's hair (exact edges, no fringe)
        #  - the chair is cut out exactly: it is the only thing that differs from the orange-chair companion
        seg = np.asarray(Image.open(OUT / a["file"].replace(".webp", "_seg.png")).convert("RGBA")).astype(np.float32) / 255
        ref = np.asarray(Image.open(OUT / "desk_ai.png").convert("RGBA")).astype(np.float32) / 255
        def near(x, px):
            return np.asarray(Image.fromarray((x * 255).astype(np.uint8)).filter(ImageFilter.MaxFilter(px))) > 0
        for i in range(a["frames"]):
            x0, y0 = (i % a["cols"]) * fw, (i // a["cols"]) * fh
            f = img[y0:y0 + fh, x0:x0 + fw]; sg = seg[y0:y0 + fh, x0:x0 + fw]
            cc = comp[y0:y0 + fh, x0:x0 + fw] if comp is not None else None
            op = f[..., 3] > 0.5
            is_c = lambda c: (np.abs(sg[..., :3] - np.array(c)).sum(-1) < 0.9) & (sg[..., 3] > 0.5)
            g_top, g_hair, g_pants = near(is_c((0, 1, 0)), 9), near(is_c((0, 0, 1)), 7), near(is_c((1, 1, 0)), 9)
            chair = near((np.abs(f[..., :3] - cc[..., :3]).mean(-1) > 0.12) & (f[..., 3] > 0.3), 3) if cc is not None else None
            ys_ = np.where(op.any(1))[0]; top_, bot_ = ys_.min(), ys_.max()
            rel_ = ((np.arange(fh) - top_) / max(1, bot_ - top_))[:, None].repeat(fw, 1)
            bestd = np.full((fh, fw), 9.0, np.float32)
            for dy in range(-2, 3):
                for dx in range(-2, 3):
                    rs = np.roll(np.roll(ref, dy, 0), dx, 1)
                    bestd = np.minimum(bestd, np.where(rs[..., 3] > 0.5, np.abs(f[..., :3] - rs[..., :3]).mean(-1), 9.0))
            desk = (bestd < 0.09) & (rel_ > 0.42)
            if chair is None:
                # no orange-chair companion: the chair is whatever matches the empty desk render, at any height
                chair = near((bestd < 0.09) & ~is_c((0, 0, 1)), 3) & ~is_c((0, 0, 1))
            h_, s_, v_ = hsv(f[..., :3])
            dark_ = op & (s_ < 0.4) & (v_ > 0.13) & (v_ < 0.45)
            if cc is not None:
                ch_, cs_, cv_ = hsv(cc[..., :3])
            comp_hair = largest_component((cc[..., 3] > 0.5) & (cs_ < 0.4) & (cv_ > 0.13) & (cv_ < 0.45) & (rel_ < hair_end)) if cc is not None else is_c((0, 0, 1))
            cell = mask[y0:y0 + fh, x0:x0 + fw]
            cell[(cell == TOP) & ~g_top] = 0
            cell[(cell == PANTS) & ~g_pants] = 0
            cell[cell == HAIR] = 0
            cell[largest_component(dark_ & near(comp_hair, 5) & g_hair & ~chair)] = HAIR
            cell[chair | desk] = 0
    # small holes inside the hair (highlight streaks, specular dots) belong to the hair too
    for i in range(a["frames"]):
        x0, y0 = (i % a["cols"]) * fw, (i // a["cols"]) * fh
        cell = mask[y0:y0 + fh, x0:x0 + fw]; op = img[y0:y0 + fh, x0:x0 + fw, 3] > 0.5
        hair = cell == HAIR
        if not hair.any():
            continue
        lab, n = ndimage.label(~hair)
        sizes = ndimage.sum(np.ones_like(lab), lab, range(1, n + 1))
        border = set(np.unique(np.concatenate([lab[0], lab[-1], lab[:, 0], lab[:, -1]])))
        for k in range(1, n + 1):
            if k not in border and sizes[k - 1] <= 40:
                cell[(lab == k) & op & (cell != SKIN)] = HAIR
    fn = a["file"].replace(".webp", "_mask.png")
    Image.fromarray(mask, "L").save(OUT / fn, optimize=True)
    a["mask"] = fn
    print(key, fn, {r: int((mask == r).sum()) for r in (TOP, HAIR, PANTS, SKIN)})

(OUT / "meta.json").write_text(json.dumps(meta, indent=1))

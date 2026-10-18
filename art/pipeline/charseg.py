"""Clothing labels for the extra characters, so every character can be recoloured like the hero.

Each raw sheet raw/chars/c{k}_{anim}.png has an AI-painted flat colour map (_seg.png) with the
same poses. The map is close but not pixel exact, so it only says roughly where each garment
is; every sprite pixel then picks, among the garments painted near it, the one whose real
colours it matches best. The result (_lab.png) has the sheet's exact silhouette and goes
through process.py as a companion, so it gets the same crop, alignment and scaling.

Usage: python3 -I charseg.py <raw_dir>
"""
import sys
from pathlib import Path

import numpy as np
from PIL import Image
from scipy import ndimage
from scipy.spatial import cKDTree

RAW = Path(sys.argv[1])

# label id -> colour in the AI map (and in the _lab.png output)
LABELS = {1: (0, 255, 0), 2: (0, 0, 255), 3: (255, 255, 0), 5: (0, 255, 255), 4: (255, 0, 0), 6: (255, 255, 255)}
RECOLOR = (1, 2, 3, 5)           # top, hair, pants, accent; skin (4) and the rest (6) keep their colours
KEEP = (128, 128, 128)           # outlines and anything unlabelled
GRID, RADIUS = 4, 10             # px search radius around the map's regions, at 1024 px sheet size


def key_alpha(rgb):
    r, g, b = rgb[..., 0], rgb[..., 1], rgb[..., 2]
    m = np.minimum(r, b) - g
    alpha = np.clip(1.0 - (m - 60.0) / 90.0, 0, 1)
    alpha = np.where(np.abs(r - b) > 110, 1.0, alpha)
    return alpha > 0.16


def seg_labels(seg):
    """Nearest pure colour, or 0 when the pixel is background (magenta, or the purple a few maps came back with)."""
    d = np.stack([np.abs(seg - np.array(c)).sum(-1) for c in LABELS.values()], -1)
    lab = np.array(list(LABELS))[d.argmin(-1)]
    return np.where(d.min(-1) < 150, lab, 0)


def best_shift(fg, sfg, r=10):
    best = (-1, 0, 0)
    for dy in range(-r, r + 1, 2):
        for dx in range(-r, r + 1, 2):
            s = np.roll(np.roll(sfg, dy, 0), dx, 1); v = (fg & s).sum() / max(1, (fg | s).sum())
            if v > best[0]:
                best = (v, dy, dx)
    return best


def label_sheet(raw_path, seg_path):
    rgb = np.asarray(Image.open(raw_path).convert("RGB")).astype(np.float32)
    seg_im = Image.open(seg_path).convert("RGB")
    if seg_im.size != (rgb.shape[1], rgb.shape[0]):
        seg_im = seg_im.resize((rgb.shape[1], rgb.shape[0]), Image.NEAREST)
    slab = seg_labels(np.asarray(seg_im).astype(np.float32))
    fg = key_alpha(rgb)
    h, w = fg.shape; ch, cw = h // GRID, w // GRID
    aligned = np.zeros_like(slab); ious = []
    for gy in range(GRID):
        for gx in range(GRID):
            sl = (slice(gy * ch, (gy + 1) * ch), slice(gx * cw, (gx + 1) * cw))
            iou, dy, dx = best_shift(fg[sl], slab[sl] > 0)
            aligned[sl] = np.roll(np.roll(slab[sl], dy, 0), dx, 1); ious.append(iou)
    lum = rgb @ np.array([0.299, 0.587, 0.114]) / 255
    outline = lum < 0.10
    # colour samples per garment: the map's region shrunk away from its edges, so the samples are clean
    trees, dists = {}, {}
    for lid in LABELS:
        reg = aligned == lid
        if reg.sum() < 50:
            continue
        core = ndimage.binary_erosion(reg, iterations=3) & fg & ~outline
        pts = rgb[core]
        if len(pts) < 30:
            pts = rgb[reg & fg & ~outline]
        if len(pts) == 0:
            continue
        rng = np.random.default_rng(lid); pts = pts[rng.choice(len(pts), min(len(pts), 4000), replace=False)]
        trees[lid] = cKDTree(pts)
        dists[lid] = ndimage.distance_transform_edt(~reg)
    ys, xs = np.nonzero(fg & ~outline); px = rgb[ys, xs]
    score = np.full((len(ys), len(trees)), np.inf, np.float32); ids = list(trees)
    for j, lid in enumerate(ids):
        sd = dists[lid][ys, xs]; ok = sd <= RADIUS
        cd, _ = trees[lid].query(px[ok], k=1)
        score[ok, j] = cd + sd[ok] * 6.0   # colour distance first, closeness to the painted region breaks ties
    lab = np.zeros((h, w), np.int32)
    has = np.isfinite(score).any(1)
    lab[ys[has], xs[has]] = np.array(ids)[score[has].argmin(1)]
    # majority filter: a pixel surrounded by another garment joins it (kills speckles on plaid and denim)
    for _ in range(2):
        counts = np.stack([ndimage.uniform_filter((lab == lid).astype(np.float32), 3) for lid in ids], -1)
        win = np.array(ids)[counts.argmax(-1)]
        flip = (lab > 0) & (counts.max(-1) > 0.6) & (win != lab)
        lab[flip] = win[flip]
    out = np.zeros((h, w, 3), np.uint8); out[:] = (255, 0, 255)
    out[fg] = KEEP
    for lid, c in LABELS.items():
        out[(lab == lid) & fg] = c
    return out, float(np.mean(ious)), {lid: int((lab == lid).sum()) for lid in ids}


if __name__ == "__main__":
    for seg_path in sorted((RAW / "chars").glob("c*_*_seg.png")):
        raw_path = seg_path.with_name(seg_path.name.replace("_seg.png", ".png"))
        out, iou, counts = label_sheet(raw_path, seg_path)
        Image.fromarray(out).save(seg_path.with_name(seg_path.name.replace("_seg.png", "_lab.png")))
        print(raw_path.stem, round(iou, 3), counts)

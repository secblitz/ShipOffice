"""Sprite pipeline: magenta key -> slice 4x4 grids -> stabilize -> pack lightweight atlases.

Usage: python3 -I process.py <raw_dir> <out_dir>
"""
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image

RAW, OUT = Path(sys.argv[1]), Path(sys.argv[2])
OUT.mkdir(parents=True, exist_ok=True)

GRID = 4
INSET = 5          # trims the dark grid lines the model draws between cells
ANIM_SCALE = 0.62  # character frames are stored at ~display size, not generation size
PROP_SCALE = 0.55


def key_magenta(img):
    """Magenta chroma key with soft edge + despill. Returns RGBA uint8 array."""
    a = np.asarray(img.convert("RGB")).astype(np.float32)
    r, g, b = a[..., 0], a[..., 1], a[..., 2]
    # "magentaness": how much red AND blue dominate green
    m = np.minimum(r, b) - g
    balance = np.abs(r - b)
    alpha = np.clip(1.0 - (m - 60.0) / (150.0 - 60.0), 0.0, 1.0)
    alpha = np.where(balance > 110, 1.0, alpha)  # strongly red or blue pixels are content
    # despill: pull r/b down toward g on semi-transparent and near-edge pixels
    spill = np.clip(m, 0, None) * (alpha < 0.999) + np.clip(m - 25, 0, None) * (alpha >= 0.999)
    r2 = r - spill * 0.85
    b2 = b - spill * 0.85
    out = np.dstack([np.clip(r2, 0, 255), g, np.clip(b2, 0, 255), alpha * 255]).astype(np.uint8)
    # kill isolated low-alpha noise
    out[..., 3][out[..., 3] < 24] = 0
    return out


def cells(arr, grid=GRID):
    h, w = arr.shape[:2]
    ch, cw = h // grid, w // grid
    for row in range(grid):
        for col in range(grid):
            y0, x0 = row * ch + INSET, col * cw + INSET
            yield arr[y0:(row + 1) * ch - INSET, x0:(col + 1) * cw - INSET].copy()


def bbox(alpha, thresh=40):
    ys, xs = np.where(alpha > thresh)
    if len(xs) == 0:
        return None
    return xs.min(), ys.min(), xs.max() + 1, ys.max() + 1


def largest_blob_mask(alpha):
    """Drop stray specks: keep pixels connected to the biggest opaque region (cheap flood fill)."""
    from collections import deque
    solid = alpha > 40
    h, w = solid.shape
    seen = np.zeros_like(solid)
    best = None
    for y0 in range(0, h, 3):
        for x0 in range(0, w, 3):
            if solid[y0, x0] and not seen[y0, x0]:
                q = deque([(y0, x0)]); seen[y0, x0] = True; pts = []
                while q:
                    y, x = q.popleft(); pts.append((y, x))
                    for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                        ny, nx = y + dy, x + dx
                        if 0 <= ny < h and 0 <= nx < w and solid[ny, nx] and not seen[ny, nx]:
                            seen[ny, nx] = True; q.append((ny, nx))
                if best is None or len(pts) > len(best):
                    best = pts
    keep = np.zeros_like(solid)
    if best:
        ys, xs = zip(*best); keep[list(ys), list(xs)] = True
    # grow by 2px so antialiased edges survive
    from PIL import ImageFilter
    grown = Image.fromarray((keep * 255).astype(np.uint8)).filter(ImageFilter.MaxFilter(5))
    return np.asarray(grown) > 0


def align_offsets(frames, region="head"):
    """Find per-frame (dx, dy) so the chosen region overlaps frame 0 best (removes AI jitter)."""
    def mask(f):
        a = f[..., 3] > 100
        bb = bbox(f[..., 3])
        if bb is None:
            return a
        x0, y0, x1, y1 = bb
        if region == "head":
            cut = y0 + int((y1 - y0) * 0.42)
            a = a.copy(); a[cut:] = False
        elif region == "base":  # fixed furniture at the bottom (typing desk)
            cut = y0 + int((y1 - y0) * 0.55)
            a = a.copy(); a[:cut] = False
        return a

    ref = mask(frames[0])
    ref_bb = bbox(ref.astype(np.uint8) * 255)
    offs = []
    for f in frames:
        m = mask(f)
        bb = bbox(m.astype(np.uint8) * 255)
        # coarse init from bbox bottoms/centers, then local IoU search
        cx = ((ref_bb[0] + ref_bb[2]) - (bb[0] + bb[2])) // 2
        cy = (ref_bb[3] - bb[3]) if region == "base" else (ref_bb[1] - bb[1])
        best, best_s = (cx, cy), -1
        for dy in range(cy - 8, cy + 9):
            for dx in range(cx - 10, cx + 11):
                sh = np.roll(np.roll(m, dy, 0), dx, 1)
                s = (sh & ref).sum() / max(1, (sh | ref).sum())
                if s > best_s:
                    best_s, best = s, (dx, dy)
        offs.append(best)
    return offs


DESK = None  # canonical desk: typing frame 0 canvas, empty-desk composite and the shared crop


def snap_to_desk(canvas_frames):
    """Move a desk sheet onto the canonical desk and repaint its furniture with the canonical pixels,
    so every character sits at the very same desk and switching sheets never moves the furniture."""
    ref = DESK["frame0"][..., 3] > 100
    ys = np.where(ref.any(1))[0]; cut = ys.min() + int((ys.max() - ys.min()) * 0.55)
    ref_b = ref.copy(); ref_b[:cut] = False
    m0 = canvas_frames[0][..., 3] > 100; m0[:cut] = False
    best, best_s = (0, 0), -1
    for dy in range(-14, 15):
        for dx in range(-14, 15):
            sh = np.roll(np.roll(m0, dy, 0), dx, 1)
            sc = (sh & ref_b).sum() / max(1, (sh | ref_b).sum())
            if sc > best_s:
                best_s, best = sc, (dx, dy)
    C = DESK["comp"].astype(np.float32); A = C[..., 3] > 128
    out = []
    for cf in canvas_frames:
        f = np.roll(np.roll(cf, best[1], 0), best[0], 1).astype(np.float32)
        d = np.full(f.shape[:2], 999.0, np.float32)
        for oy in (-1, 0, 1):
            for ox in (-1, 0, 1):
                d = np.minimum(d, np.abs(np.roll(np.roll(f[..., :3], oy, 0), ox, 1) - C[..., :3]).mean(-1))
        use = A & ((d < 20) | (f[..., 3] < 60))
        f[use] = C[use]
        out.append(f.clip(0, 255).astype(np.uint8))
    return out, best, round(float(best_s), 3)


def build_anim(name, region, drop=(), feet_lock=False, lock_static=False, companion=None, src=None, out=None, desk=False, crop=None, raw_sheet=True, quality=90):
    out = out or name
    raw = Image.open(RAW / (src or f"{name}.png"))
    keyed = key_magenta(raw)
    frames = []
    for c in cells(keyed):
        keep = largest_blob_mask(c[..., 3])
        c[..., 3] = np.where(keep, c[..., 3], 0)
        frames.append(c)
    raw_frames = [f.copy() for f in frames]
    frames = [f for i, f in enumerate(frames) if i not in drop]

    offs = align_offsets(frames, region)
    pad = 24
    global LAST_TYPE
    h, w = frames[0].shape[:2]
    canvas_frames = []
    for f, (dx, dy) in zip(frames, offs):
        big = np.zeros((h + pad * 2, w + pad * 2, 4), np.uint8)
        big[pad + dy:pad + dy + h, pad + dx:pad + dx + w] = f
        canvas_frames.append(big)
    static, rolls = None, None
    if lock_static:
        # pixels that barely change across frames (desk, laptop, chair) are copied from frame 0,
        # so the furniture is perfectly still and only the character moves
        stack = np.stack(canvas_frames).astype(np.float32)
        std = stack[..., :3].std(0).mean(-1) + stack[..., 3].std(0) * 0.5
        static = (std < 14) & (stack[0][..., 3] > 0)
        for cf in canvas_frames[1:]:
            cf[static] = canvas_frames[0][static]
    if feet_lock:
        # keep the head alignment horizontally, but put every frame's feet on one baseline
        bottoms = [bbox(cf[..., 3])[3] for cf in canvas_frames]
        rolls = [max(bottoms) - b for b in bottoms]
        canvas_frames = [np.roll(cf, r, 0) for cf, r in zip(canvas_frames, rolls)]

    snap = None
    if desk and DESK is not None:
        canvas_frames, shift, iou = snap_to_desk(canvas_frames); snap = {"shift": shift, "iou": iou}
        crop = DESK["crop"]
    if crop is None:
        # union bbox -> one tight frame size for the whole animation
        boxes = [bbox(cf[..., 3]) for cf in canvas_frames]
        x0 = min(b[0] for b in boxes) - 2; y0 = min(b[1] for b in boxes) - 2
        x1 = max(b[2] for b in boxes) + 2; y1 = max(b[3] for b in boxes) + 2
    else:
        x0, y0, x1, y1 = crop
    fw, fh = int((x1 - x0) * ANIM_SCALE), int((y1 - y0) * ANIM_SCALE)

    def pack(frame_list, crop):
        n = len(frame_list); cols = 4; rows = (n + cols - 1) // cols
        sheet = Image.new("RGBA", (fw * cols, fh * rows), (0, 0, 0, 0))
        for i, cf in enumerate(frame_list):
            im = Image.fromarray(cf)
            if crop:
                im = im.crop((x0, y0, x1, y1))
            im = im.resize((fw, fh), Image.LANCZOS)
            sheet.paste(im, ((i % cols) * fw, (i // cols) * fh))
        return sheet

    if name == "anim_type":
        LAST_TYPE = {"frame0": canvas_frames[0], "crop": (x0, y0, x1, y1), "pad": pad, "off0": offs[0], "fw": fw, "fh": fh}
    stable = pack(canvas_frames, True)
    stable.save(OUT / f"{out}.webp", "WEBP", quality=quality, method=6)
    for suffix, src in (companion or {}).items():
        ck = key_magenta(Image.open(RAW / f"{src}.png"))
        cfr = []
        for c in cells(ck):
            keep = largest_blob_mask(c[..., 3]); c[..., 3] = np.where(keep, c[..., 3], 0); cfr.append(c)
        cfr = [f for i, f in enumerate(cfr) if i not in drop]
        comp_frames = []
        for f, (dx, dy) in zip(cfr, offs):
            big = np.zeros((h + pad * 2, w + pad * 2, 4), np.uint8)
            big[pad + dy:pad + dy + h, pad + dx:pad + dx + w] = f
            comp_frames.append(big)
        # companions follow every per-frame step the sheet took, so they stay pixel aligned
        if static is not None:
            for cf in comp_frames[1:]:
                cf[static] = comp_frames[0][static]
        if rolls is not None:
            comp_frames = [np.roll(cf, r, 0) for cf, r in zip(comp_frames, rolls)]
        if snap:
            comp_frames = [np.roll(np.roll(cf, snap["shift"][1], 0), snap["shift"][0], 1) for cf in comp_frames]
        sheet = pack(comp_frames, True)
        if suffix == "lab":
            sheet = quantize_labels(sheet)
        sheet.save(OUT / f"{out}_{suffix}.png")

    jitter = float(np.mean([abs(dx) + abs(dy) for dx, dy in offs]))
    info = {"file": f"{out}.webp", "fw": fw, "fh": fh, "frames": len(canvas_frames), "cols": 4,
            "anchor": [0.5, 0.97], "dropped": list(drop), "jitterPx": round(jitter * ANIM_SCALE, 2)}
    if snap:
        info["snap"] = snap
    if not raw_sheet:
        return info
    # raw comparison: keyed but unaligned, cropped to the same window around each cell
    raw_canvas = []
    for f in raw_frames:
        big = np.zeros((h + pad * 2, w + pad * 2, 4), np.uint8)
        big[pad:pad + h, pad:pad + w] = f
        raw_canvas.append(big)
    rb = [bbox(cf[..., 3]) for cf in raw_canvas]
    rx0 = min(b[0] for b in rb) - 2; ry0 = min(b[1] for b in rb) - 2
    rx1 = max(b[2] for b in rb) + 2; ry1 = max(b[3] for b in rb) + 2
    rfw, rfh = int((rx1 - rx0) * ANIM_SCALE), int((ry1 - ry0) * ANIM_SCALE)
    rsheet = Image.new("RGBA", (rfw * 4, rfh * 4), (0, 0, 0, 0))
    for i, cf in enumerate(raw_canvas):
        im = Image.fromarray(cf).crop((rx0, ry0, rx1, ry1)).resize((rfw, rfh), Image.LANCZOS)
        rsheet.paste(im, ((i % 4) * rfw, (i // 4) * rfh))
    rsheet.save(OUT / f"{out}_raw.webp", "WEBP", quality=88, method=6)
    info.update({"raw": f"{out}_raw.webp", "rawFw": rfw, "rawFh": rfh, "rawFrames": 16})
    return info


LAB_COLORS = np.array([(0, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0), (255, 0, 0), (0, 255, 255), (255, 255, 255), (128, 128, 128)], np.float32)


def quantize_labels(sheet):
    """Resizing blends neighbouring labels; snap every pixel back to the nearest pure label colour."""
    a = np.asarray(sheet).astype(np.float32)
    d = np.stack([np.abs(a[..., :3] - c).sum(-1) for c in LAB_COLORS], -1)
    q = LAB_COLORS[d.argmin(-1)].astype(np.uint8)
    alpha = np.where(a[..., 3] > 100, 255, 0).astype(np.uint8)
    return Image.fromarray(np.dstack([q, alpha]), "RGBA")


def build_atlas(src, name, grid_rows, grid_cols, scale, names):
    keyed = key_magenta(Image.open(RAW / src))
    h, w = keyed.shape[:2]
    ch, cw = h // grid_rows, w // grid_cols
    items = []
    for r in range(grid_rows):
        for c in range(grid_cols):
            cell = keyed[r * ch + INSET:(r + 1) * ch - INSET, c * cw + INSET:(c + 1) * cw - INSET].copy()
            keep = largest_blob_mask(cell[..., 3])
            cell[..., 3] = np.where(keep, cell[..., 3], 0)
            bb = bbox(cell[..., 3])
            im = Image.fromarray(cell).crop(bb)
            im = im.resize((max(1, int(im.width * scale)), max(1, int(im.height * scale))), Image.LANCZOS)
            items.append(im)
    # shelf packing
    width = 1024
    x = y = shelf = 0
    rects = []
    for im in items:
        if x + im.width > width:
            x = 0; y += shelf + 2; shelf = 0
        rects.append((x, y, im.width, im.height))
        x += im.width + 2; shelf = max(shelf, im.height)
    atlas = Image.new("RGBA", (width, y + shelf), (0, 0, 0, 0))
    for im, (rx, ry, _, _) in zip(items, rects):
        atlas.paste(im, (rx, ry))
    atlas.save(OUT / f"{name}.webp", "WEBP", quality=90, method=6)
    return {"file": f"{name}.webp", "w": atlas.width, "h": atlas.height,
            "items": {n: list(rc) for n, rc in zip(names, rects)}}


def build_lineup():
    keyed = key_magenta(Image.open(RAW / "lineup.png"))
    alpha = keyed[..., 3] > 40
    colfill = alpha.any(0)
    # split on empty column runs
    spans, start = [], None
    for x, v in enumerate(colfill):
        if v and start is None: start = x
        if not v and start is not None:
            if x - start > 30: spans.append((start, x))
            start = None
    if start is not None: spans.append((start, len(colfill)))
    out = []
    for i, (a, b) in enumerate(spans):
        sub = keyed[:, a:b]
        bb = bbox(sub[..., 3])
        im = Image.fromarray(sub).crop(bb)
        im = im.resize((int(im.width * 0.8), int(im.height * 0.8)), Image.LANCZOS)
        fn = f"base_{i}.webp"; im.save(OUT / fn, "WEBP", quality=90, method=6)
        out.append({"file": fn, "w": im.width, "h": im.height})
    return out


def build_empty_desk():
    """Register the AI-made empty desk to typing frame 0 so sitting down causes no pop."""
    T = LAST_TYPE
    big = key_magenta(Image.open(RAW / "desk_empty_big.png"))
    # the edit input was cell 0 (inset 4) scaled 3x and pasted at (140, 140) on a 1024 canvas
    im = Image.fromarray(big).crop((140, 140, 140 + 744, 140 + 744)).resize((248, 248), Image.LANCZOS)
    cell = np.asarray(im).copy()
    keep = largest_blob_mask(cell[..., 3]); cell[..., 3] = np.where(keep, cell[..., 3], 0)
    # place it on the same padded canvas as frame 0 (cells were sliced with INSET=5, input used 4)
    h, w = T["frame0"].shape[:2]; pad = T["pad"]; dx0, dy0 = T["off0"]
    ref = T["frame0"][..., 3] > 100
    ys = np.where(ref.any(1))[0]; cut = ys.min() + int((ys.max() - ys.min()) * 0.55)
    ref_base = ref.copy(); ref_base[:cut] = False
    best, best_s = (0, 0), -1
    for dy in range(-10, 11):
        for dx in range(-10, 11):
            canvas = np.zeros((h, w, 4), np.uint8)
            y0, x0 = pad + dy0 - 1 + dy, pad + dx0 - 1 + dx
            ys0, xs0 = max(0, y0), max(0, x0)
            sub = cell[ys0 - y0:min(248, h - y0), xs0 - x0:min(248, w - x0)]
            canvas[ys0:ys0 + sub.shape[0], xs0:xs0 + sub.shape[1]] = sub
            m = canvas[..., 3] > 100; m[:cut] = False
            sc = (m & ref_base).sum() / max(1, (m | ref_base).sum())
            if sc > best_s:
                best_s, best = sc, (dx, dy); best_canvas = canvas
    # keep frame 0's pixels only where they clearly show furniture; everything the character covered comes from the empty render
    f0 = T["frame0"].astype(np.float32); em = best_canvas.astype(np.float32)
    from PIL import ImageFilter
    # the empty render came back with a lighter laptop; match its grey to the laptop in the typing frames
    hh, ww = em.shape[:2]
    mx_e, mn_e = em[..., :3].max(-1), em[..., :3].min(-1); sat_e = (mx_e - mn_e) / (mx_e + 1e-6)
    left = (np.arange(ww)[None, :] < ww * 0.55).repeat(hh, 0)
    lap_e = (em[..., 3] > 200) & (sat_e < 0.15) & (mx_e > 115) & (mx_e < 245) & left
    mx_f, mn_f = f0[..., :3].max(-1), f0[..., :3].min(-1); sat_f = (mx_f - mn_f) / (mx_f + 1e-6)
    lap_f = lap_e & (f0[..., 3] > 200) & (sat_f < 0.15) & (mx_f > 50)
    if lap_e.sum() > 50 and lap_f.sum() > 50:
        gain = f0[..., :3][lap_f].mean(0) / em[..., :3][lap_e].mean(0)
        em[..., :3][lap_e] = (em[..., :3][lap_e] * gain).clip(0, 255)
    furn = (em[..., 3] > 128) & (f0[..., 3] > 128) & (np.abs(f0[..., :3] - em[..., :3]).mean(-1) < 15)
    furn_img = Image.fromarray((furn * 255).astype(np.uint8)).filter(ImageFilter.MinFilter(3)).filter(ImageFilter.GaussianBlur(0.8))
    mk = (np.asarray(furn_img).astype(np.float32) / 255.0)[..., None]
    comp = (f0 * mk + em * (1 - mk)).clip(0, 255).astype(np.uint8)
    global DESK
    DESK = {"frame0": T["frame0"], "comp": comp, "crop": DESK_CROP}
    x0, y0, x1, y1 = DESK_CROP; fw, fh = int((x1 - x0) * ANIM_SCALE), int((y1 - y0) * ANIM_SCALE)
    out = Image.fromarray(comp).crop((x0, y0, x1, y1)).resize((fw, fh), Image.LANCZOS)
    out.save(OUT / "desk_empty.webp", "WEBP", quality=90, method=6)
    # the pure AI furniture render (no character pixels) is kept as the reference for the recolor mask
    Image.fromarray(em.clip(0, 255).astype(np.uint8)).crop((x0, y0, x1, y1)).resize((fw, fh), Image.LANCZOS).save(OUT / "desk_ai.png")
    return {"file": "desk_empty.webp", "w": fw, "h": fh, "iou": round(float(best_s), 3), "shift": best}


meta = {"anims": {}}
CHARS = ["Hoodie", "Bun + glasses", "Beanie + beard", "Buzz cut", "Silver + vest", "Long red hair", "Curls + bomber"]
# per animation: (raw name for the hero, alignment region, dropped frames, feet lock)
ANIMS = {
    "walk": ("anim_walk", "head", (), True),
    "walkback": ("anim_walkback", "head", (), True),
    "idle": ("anim_idle", "head", (), True),
    "sit": ("anim_sit", "head", (10, 11, 12, 13), False),   # the mug leaves the hands on frames 11-14
    "ppfront": ("anim_ppfront", "head", (15,), True),       # the paddle vanishes on the last frame
    "ppback": ("anim_ppback", "head", (), True),
}
# frames a single character's sheet gets wrong on top of the shared drops
EXTRA_DROP = {(4, "sit"): (8,)}   # sudden fist pump in the middle of sipping
# the typing sheet defines the desk every other desk sheet snaps to
build_anim("anim_type", "base", lock_static=True, raw_sheet=False)
x0, y0, x1, y1 = LAST_TYPE["crop"]
DESK_CROP = (x0 - 8, y0 - 40, x1 + 8, y1)   # room for taller hair (buns, curls) above the desk sheet
meta["deskEmpty"] = build_empty_desk()
meta["anims"]["type"] = build_anim("anim_type", "base", lock_static=True, crop=DESK_CROP, companion={"companion": "anim_type_orange", "seg": "anim_type_seg"})
meta["anims"]["read"] = build_anim("anim_read", "base", lock_static=True, desk=True, companion={"seg": "anim_read_seg"})
for key, (raw, region, drop, feet) in ANIMS.items():
    meta["anims"][key] = build_anim(raw, region, drop=drop, feet_lock=feet)
for k in range(1, len(CHARS)):
    for key, (raw, region, drop, feet) in list(ANIMS.items()) + [("type", ("", "base", (), False)), ("read", ("", "base", (), False))]:
        src = RAW / "chars" / f"c{k}_{key}.png"
        if not src.exists():
            print("missing", src.name); continue
        desk = key in ("type", "read")
        drop = tuple(sorted(set(drop) | set(EXTRA_DROP.get((k, key), ()))))
        a = build_anim(raw, region, drop=drop, feet_lock=feet, lock_static=desk, desk=desk,
                       src=f"chars/c{k}_{key}.png", out=f"anim_{key}_c{k}", raw_sheet=False, quality=86,
                       companion={"lab": f"chars/c{k}_{key}_lab"} if (RAW / "chars" / f"c{k}_{key}_lab.png").exists() else None)
        a["char"] = k
        meta["anims"][f"{key}_c{k}"] = a
        im = Image.open(src).convert("RGB"); im.thumbnail((384, 384)); im.save(OUT / f"src_anim_{key}_c{k}.webp", "WEBP", quality=72, method=6)
meta["chars"] = [{"id": i, "name": n, "recolor": True} for i, n in enumerate(CHARS)]

meta["props"] = build_atlas("props_sheet.png", "props", 4, 4, PROP_SCALE, [
    "desk", "chair", "whiteboard", "plant", "coffee", "cooler", "pingpong", "sofa",
    "screen", "fridge", "bookshelf", "server", "lamp", "beanbag", "arcade", "rug"])
meta["tiles"] = build_atlas("tiles_sheet.png", "tiles", 4, 4, 0.5, [
    "oak", "carpet", "marble", "tealrug", "walnut", "terracotta", "concrete", "checker",
    "wallA", "wallWindowA", "brickA", "posterA", "wallB", "wallWindowB", "slatB", "glassB"])
meta["bases"] = build_lineup()
meta["cafe"] = build_atlas("cafe_sheet.png", "cafe", 2, 2, 0.32, ["table", "chairL", "chairR", "lowtable"])
for name, src, size in (("concept", "office_scene.png", 1376), ("ref", "character_ref.png", 900)):
    im = Image.open(RAW / src).convert("RGB")
    im.thumbnail((size, size)); im.save(OUT / f"{name}.webp", "WEBP", quality=86, method=6)
for n in ("anim_walk", "anim_walkback", "anim_idle", "anim_type", "anim_read", "anim_sit", "anim_ppfront", "anim_ppback", "props_sheet", "tiles_sheet", "cafe_sheet"):
    im = Image.open(RAW / f"{n}.png").convert("RGB"); im.thumbnail((512, 512))
    im.save(OUT / f"src_{n}.webp", "WEBP", quality=80, method=6)

def clean_pingpong(meta):
    """The table prop came with two paddles and balls on it; players hold their own, so inpaint them out."""
    from PIL import ImageFilter
    atlas = Image.open(OUT / meta["props"]["file"]).convert("RGBA")
    x, y, w, h = meta["props"]["items"]["pingpong"]
    a = np.asarray(atlas.crop((x, y, x + w, y + h))).astype(np.float32)
    r, g, b = a[..., 0], a[..., 1], a[..., 2]
    red = (r > 140) & (r > g * 1.6) & (r > b * 1.5)
    handle = (r > 150) & (g > 90) & (b < 110) & (r > b * 1.6) & (r - g < 90)
    ball = (r > 200) & (g > 190) & (b > 120) & (np.abs(r - g) < 40) & (b < 235)
    near = np.asarray(Image.fromarray((red * 255).astype(np.uint8)).filter(ImageFilter.MaxFilter(15))) > 0
    dark = a[..., :3].max(-1) < 90
    seed = Image.fromarray((((red | handle | ball | dark) & near) * 255).astype(np.uint8))
    hole = (np.asarray(seed.filter(ImageFilter.MaxFilter(3))) > 0) & near & (a[..., 3] > 200)
    known = ~hole
    img = a[..., :3].copy()
    for _ in range(60):  # diffusion fill from the surrounding table surface
        acc = np.zeros_like(img); cnt = np.zeros(img.shape[:2], np.float32)
        for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            sh = np.roll(np.roll(img, dy, 0), dx, 1); kn = np.roll(np.roll(known, dy, 0), dx, 1)
            acc += sh * kn[..., None]; cnt += kn
        upd = hole & (cnt > 0)
        img[upd] = acc[upd] / cnt[upd][:, None]
        known = known | upd
    a[..., :3] = img
    atlas.paste(Image.fromarray(a.clip(0, 255).astype(np.uint8)), (x, y))
    atlas.save(OUT / meta["props"]["file"], "WEBP", quality=90, method=6)
    return int(hole.sum())


meta["pingpongCleaned"] = clean_pingpong(meta)
(OUT / "meta.json").write_text(json.dumps(meta, indent=1))
print(json.dumps({k: (v["fw"], v["fh"], v["frames"], v.get("snap", {}).get("iou")) for k, v in meta["anims"].items()}))
print(meta["deskEmpty"], meta["cafe"]["items"])

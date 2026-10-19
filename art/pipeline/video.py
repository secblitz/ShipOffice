"""Walk cycles from video, and the standing turnaround they start from.

Each character was filmed walking on the spot in five directions (front, front diagonal, side,
back diagonal, back; the other three are mirror images), and playing two games (the guitar on a
stool, practice strokes with a pool cue), all facing the front diagonal for the games. For every clip this cuts the stretch
where the motion repeats into a seamless loop, keys the magenta, takes out any drift, scales the
figure to one standing height and packs a sheet. The five stills the clips started from become a
standing sheet. Recolour masks are learned per character from the labelled sheets it already
has, so outfits keep their per-agent colours.

The rest of what they do was filmed the same way: typing and reading at the desk (the desk and
chair are in the shot, found and fitted onto the empty desk so the sheet replaces it exactly),
sipping a coffee on an unseen seat, a ping pong stroke from both ends of the table (one stroke
per loop, starting at the hit, with where the paddle is at that moment), and two ways of
playing a game standing: at a table (front diagonal) and at a machine (back diagonal).

Usage: python3 -I video.py <clips_dir> <stills_dir> <out_dir> [only]
  clips_dir/c{k}_{dir}.mp4 and stills_dir/c{k}_{dir}.png, dir in s se e ne n;
  clips_dir/c{k}_{game}.mp4 for game in guitar pool;
  clips_dir/c{k}_{act}.mp4 for act in type read sit ppfront ppback play_se play_ne;
  only: a comma list (say "guitar,pool" or "stand,s,ne") to redo just those sheets
"""
import json
import subprocess
import sys
from multiprocessing import Pool
from pathlib import Path

import numpy as np
from PIL import Image
from scipy import ndimage
from scipy.spatial import cKDTree

CLIPS, STILLS, OUT = (Path(a) for a in sys.argv[1:4]) if len(sys.argv) > 3 else (None, None, None)
DIRS = ["s", "se", "e", "ne", "n"]
GAMES = ["guitar", "pool"]
# what else they were filmed doing, and the sheet each becomes
ACTS = {"type": "typing", "read": "reading", "sit": "sipping", "ppfront": "swing_front", "ppback": "swing_back",
        "play_se": "play_se", "play_ne": "play_ne"}
DESK = ("type", "read")
SWINGS = ("ppfront", "ppback")
# the desk sheets share the empty desk's frame (130x177 drawn at 0.54), with room above for the
# head and at the sides, at the video sheets' resolution (drawn at 0.324)
DESK_RES = 0.54 / 0.324
DESK_PAD = (10, 30)  # empty-desk px added left and right, and above
ONLY = set(sys.argv[4].split(",")) if len(sys.argv) > 4 else None
BODY = 214          # standing figure height in sheet px (HiDPI: the old sheets were ~144)
ANCHOR = 0.97       # feet sit this far down the frame, like every other sheet
PMIN, PMAX = 14, 40  # loop length in frames at 24 fps
COLS = 8


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


def clip_frames(path):
    w, h = map(int, subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v", "-show_entries", "stream=width,height",
                                    "-of", "csv=p=0", str(path)], capture_output=True, text=True, check=True).stdout.split(","))
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", str(path), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], capture_output=True, check=True).stdout
    return np.frombuffer(raw, np.uint8).reshape(-1, h, w, 3)


def feet(k):
    ys, xs = np.nonzero(k[..., 3] > 128)
    if len(xs) == 0:
        return 0.0, 0.0
    low = ys > ys.max() - 40
    return float(xs[low].mean()), float(ys.max())


def find_loop(F, K, pmin=PMIN, pmax=PMAX):
    """Start and length of the most seamless loop, matching motion as well as pose at the seam."""
    n = len(F)
    edge = np.concatenate([F[:, :40].reshape(n, -1, 3), F[:, -40:].reshape(n, -1, 3)], axis=1).astype(int)
    bg_ok = ((edge[..., 0] > 200) & (edge[..., 2] > 200) & (edge[..., 1] < 90)).mean(axis=1)
    small = np.stack([np.asarray(Image.fromarray(k).resize((k.shape[1] // 8, k.shape[0] // 8), Image.BILINEAR)) for k in K]).astype(np.float32)
    sig = small[..., :3] * (small[..., 3:] / 255.0)
    D = np.abs(sig[:, None] - sig[None, :]).mean(axis=(2, 3, 4))
    step = np.diag(D, 1)
    best = None
    for s in range(5, n - pmin - 6):
        for p in range(pmin, min(pmax, n - s - 4) + 1):
            if bg_ok[s:s + p + 3].min() < 0.97:
                continue
            motion = step[s:s + p].mean()
            score = (D[s, s + p] + 0.5 * D[s + 1, s + p + 1] + 0.5 * D[s + 2, s + p + 2]) / 2.0 / max(motion, 1e-3)
            if best is None or score < best[0]:
                best = (score, s, p, D[s, s + p] / np.median(step))
    return best


def normalise(frames, body_h, baseline):
    """Scale RGBA frames so the figure stands BODY px tall, and frame them around the feet."""
    sc = BODY / body_h
    A = np.stack([f[..., 3] > 16 for f in frames])
    ys, xs = np.nonzero(A.any(axis=0))
    cx = float(np.mean([feet(f)[0] for f in frames]))
    half = max(cx - xs.min(), xs.max() - cx) + 4
    top = ys.min() - 4
    above = (baseline - top) * sc
    fh = int(np.ceil(max(above / ANCHOR, BODY / ANCHOR + 6)))
    fw = int(np.ceil(2 * half * sc)) + 2
    out = []
    for f in frames:
        im = Image.fromarray(f).crop((int(cx - half), int(top), int(cx + half), int(baseline + 6)))
        im = im.resize((max(1, round(im.width * sc)), max(1, round(im.height * sc))), Image.LANCZOS)
        c = Image.new("RGBA", (fw, fh), (0, 0, 0, 0))
        c.paste(im, ((fw - im.width) // 2, round(fh * ANCHOR - (baseline - top) * sc)), im)
        out.append(np.asarray(c))
    return out


def body_height(f):
    ys = np.nonzero((f[..., 3] > 128).any(axis=1))[0]
    return ys.max() - ys.min() + 1


def standing_height(k, clip_h):
    """How tall the character stands in a game clip: its front-diagonal still, at the clip's
    scale (sitting or leaning over a cue is shorter than standing, so games scale by this)."""
    im = Image.open(STILLS / f"c{k}_se.png").convert("RGB")
    return body_height(key_magenta(np.asarray(im))) * clip_h / im.height


def loop(job):
    k, d = job
    F = clip_frames(CLIPS / f"c{k}_{d}.mp4")
    K = np.stack([key_magenta(f) for f in F])
    if d in ACTS:
        K = np.stack([despeckle(f) for f in K])
        if d in DESK:
            return desk_loop(k, d, F, K)
        if d in SWINGS:
            return swing_loop(k, d, F, K)
    if d in GAMES or d in ACTS:
        return game_loop(k, d, F, K)
    found = find_loop(F, K)
    if found is None:
        return (k, d, None)
    _, s, p, seam = found
    L = K[s:s + p]
    # drift: fit the feet over one loop and take the linear part out, so the loop closes in place
    pts = np.array([feet(f) for f in K[s:s + p + 1]])
    t = np.arange(p + 1)
    dx, dy = np.polyfit(t, pts[:, 0], 1)[0], np.polyfit(t, pts[:, 1], 1)[0]
    L = [np.asarray(Image.fromarray(f).transform((f.shape[1], f.shape[0]), Image.AFFINE, (1, 0, dx * i, 0, 1, dy * i), Image.BICUBIC)) for i, f in enumerate(L)]
    baseline = float(np.median([feet(f)[1] for f in L]))
    frames = normalise(L, float(np.median([body_height(f) for f in L])), baseline)
    return (k, d, {"frames": frames, "start": int(s), "period": int(p), "seam": round(float(seam), 2)})


def game_loop(k, d, F, K):
    """A game clip starts and ends on the same still, so the whole clip is the loop: it ends at
    whichever late frame comes back closest to the first, and keeps every other frame (12 fps)."""
    end, seam = end_frame(K)
    L = list(K[0:end:2])
    baseline = float(np.median([feet(f)[1] for f in L]))
    frames = close_loop(normalise(L, standing_height(k, F.shape[1]), baseline))
    return (k, d, {"frames": frames, "start": 0, "period": len(L), "seam": round(seam, 2)})


def despeckle(f):
    """Drop specks the model added on its own (a ball in flight): blobs far smaller than the figure."""
    # solid pixels only: faint keying noise would bridge a speck to the figure
    lab, n = ndimage.label(f[..., 3] > 128, structure=np.ones((3, 3)))
    if n < 2:
        return f
    idx = range(1, n + 1)
    sizes = ndimage.sum(np.ones(lab.shape), lab, idx)
    # a ball is small and white; a paddle out of the hand is as small, but red
    white = ndimage.mean(f[..., :3].min(-1).astype(float), lab, idx) > 140
    drop = (sizes < sizes.max() * 0.01) | ((sizes < sizes.max() * 0.05) & white)
    keep = ndimage.binary_dilation(np.isin(lab, 1 + np.nonzero(~drop)[0]), iterations=3)
    out = f.copy()
    out[..., 3] = np.where(keep, f[..., 3], 0)
    return out


def close_loop(frames, n=3):
    """Fade the last few frames of a whole-clip loop into its first, so the seam does not jump."""
    out = list(frames)
    for j in range(n):
        w = (j + 1) / (n + 1)
        i = len(out) - n + j
        out[i] = (out[i].astype(np.float32) * (1 - w) + out[0].astype(np.float32) * w).round().astype(np.uint8)
    return out


def end_frame(K):
    """The late frame that comes back closest to the first (the clips start and end on one still)."""
    small = np.stack([np.asarray(Image.fromarray(f).resize((f.shape[1] // 8, f.shape[0] // 8), Image.BILINEAR)) for f in K]).astype(np.float32)
    sig = small[..., :3] * (small[..., 3:] / 255.0)
    n = len(sig)
    end = min(range(n - 12, n), key=lambda j: np.abs(sig[0] - sig[j]).mean())
    step = np.median(np.abs(sig[2::2] - sig[:-2:2]).mean(axis=(1, 2, 3)))
    return end, float(np.abs(sig[0] - sig[end]).mean() / max(step, 1e-3))


def desk_empty():
    return np.asarray(Image.open(OUT / "desk_empty.webp").convert("RGBA"))


def fit_desk(alpha):
    """Scale and offset (video px per empty-desk px, x, y) that put the empty desk over the desk
    in a frame: the best overlap of the two silhouettes."""
    from scipy.signal import fftconvolve
    E = desk_empty()[..., 3] > 128
    eh, ew = E.shape
    best = None
    for s in np.arange(3.0, 7.0, 0.03):
        h, w = round(alpha.shape[0] / s), round(alpha.shape[1] / s)
        m = np.asarray(Image.fromarray((alpha > 128).astype(np.uint8) * 255).resize((w, h), Image.BILINEAR)) > 127
        if m.shape[0] < eh or m.shape[1] < ew:
            continue
        inter = fftconvolve(m.astype(float), E[::-1, ::-1].astype(float), mode="valid")
        c = np.cumsum(np.cumsum(np.pad(m.astype(float), ((1, 0), (1, 0))), 0), 1)
        win = c[eh:, ew:] - c[:-eh, ew:] - c[eh:, :-ew] + c[:-eh, :-ew]
        iou = inter / (win + E.sum() - inter)
        y, x = np.unravel_index(np.argmax(iou), iou.shape)
        if best is None or iou[y, x] > best[0]:
            best = (float(iou[y, x]), float(s), float(x * s), float(y * s))
    return best


def desk_size():
    eh, ew = desk_empty().shape[:2]
    return round((ew + 2 * DESK_PAD[0]) * DESK_RES), round((eh + DESK_PAD[1]) * DESK_RES)


def desk_loop(k, d, F, K):
    """Typing or reading: the whole clip at 12 fps, cut so the desk lands where the empty one is."""
    end, seam = end_frame(K)
    iou, s, ox, oy = fit_desk(K[0][..., 3])
    eh, ew = desk_empty().shape[:2]
    x0, y0 = ox - DESK_PAD[0] * s, oy - DESK_PAD[1] * s
    box = (x0, y0, x0 + (ew + 2 * DESK_PAD[0]) * s, y0 + (eh + DESK_PAD[1]) * s)
    size = desk_size()
    frames = close_loop([np.asarray(Image.fromarray(f).transform(size, Image.EXTENT, box, Image.BICUBIC)) for f in K[0:end:2]])
    return (k, d, {"frames": frames, "start": 0, "period": len(frames), "seam": round(seam, 2), "fit": round(iou, 3)})


def paddle(f):
    """Centre of the red paddle in a frame, or None."""
    a = f.astype(int)
    red = (a[..., 0] > 170) & (a[..., 0] - np.maximum(a[..., 1], a[..., 2]) > 120) & (a[..., 3] > 128)
    # red hair or a red check shirt are red too: the paddle is the one round blob of its size
    lab, n = ndimage.label(red)
    best = None
    for i, sl in enumerate(ndimage.find_objects(lab), 1):
        area = int((lab[sl] == i).sum())
        fill = area / ((sl[0].stop - sl[0].start) * (sl[1].stop - sl[1].start))
        if 200 <= area <= 1600 and fill >= 0.55 and (best is None or area > best[0]):
            best = (area, i)
    if best is None:
        return None
    ys, xs = np.nonzero(lab == best[1])
    return float(xs.mean()), float(ys.mean())


def swing_loop(k, d, F, K):
    """One ping pong stroke: the most seamless stroke-long loop, turned to start at the hit (where
    the paddle moves fastest), plus where the paddle is then from the frame's bottom centre."""
    # one stroke a second was asked for: 16 to 28 frames keeps it to a single stroke
    found = find_loop(F, K, 16, 28)
    if found is None:
        return (k, d, None)
    _, s, p, seam = found
    baseline = float(np.median([feet(f)[1] for f in K[s:s + p]]))
    frames = normalise(list(K[s:s + p]), standing_height(k, F.shape[1]), baseline)
    pts = [paddle(f) for f in frames]
    speed = [np.hypot(b[0] - a[0], b[1] - a[1]) if a and b else 0.0 for a, b in zip(pts, pts[1:] + pts[:1])]
    hit = int(np.argmax(speed))
    frames = frames[hit:] + frames[:hit]
    extra = {"frames": frames, "start": int(s), "period": int(p), "seam": round(float(seam), 2)}
    if pts[hit]:
        fh, fw = frames[0].shape[:2]
        extra["paddle"] = [round(pts[hit][0] - fw / 2, 1), round(fh - pts[hit][1], 1)]
    return (k, d, extra)


def desk_template(fw, fh):
    """The empty desk where it sits in a desk sheet's frame."""
    e = desk_empty()
    im = Image.fromarray(e).resize((round(e.shape[1] * DESK_RES), round(e.shape[0] * DESK_RES)), Image.LANCZOS)
    t = Image.new("RGBA", (fw, fh), (0, 0, 0, 0))
    t.paste(im, (round(DESK_PAD[0] * DESK_RES), round(DESK_PAD[1] * DESK_RES)))
    return np.asarray(t)


def stand(k):
    """The five stills as one sheet, in DIRS order, all at the same standing height."""
    keyed = [key_magenta(np.asarray(Image.open(STILLS / f"c{k}_{d}.png").convert("RGB"))) for d in DIRS]
    return [normalise([f], float(body_height(f)), feet(f)[1])[0] for f in keyed]


def pack(frames, name, cols):
    fw = max(f.shape[1] for f in frames)
    fh = max(f.shape[0] for f in frames)
    rows = (len(frames) + cols - 1) // cols
    sheet = Image.new("RGBA", (fw * cols, fh * rows), (0, 0, 0, 0))
    for i, f in enumerate(frames):
        # pad each frame to the sheet cell, keeping the feet at the anchor
        x = (i % cols) * fw + (fw - f.shape[1]) // 2
        y = (i // cols) * fh + round(fh * ANCHOR - f.shape[0] * ANCHOR)
        sheet.paste(Image.fromarray(f), (x, y))
    sheet.save(OUT / f"{name}.webp", "WEBP", quality=90, alpha_quality=100, method=6)
    return sheet, fw, fh


# --- recolour masks, learned from each character's labelled sheets ---

def lab(rgb):
    c = rgb.astype(np.float32) / 255.0
    c = np.where(c > 0.04045, ((c + 0.055) / 1.055) ** 2.4, c / 12.92)
    xyz = c @ np.array([[0.4124, 0.2126, 0.0193], [0.3576, 0.7152, 0.1192], [0.1805, 0.0722, 0.9505]], np.float32)
    xyz /= np.array([0.9505, 1.0, 1.089], np.float32)
    f = np.where(xyz > 0.008856, np.cbrt(xyz), 7.787 * xyz + 16 / 116)
    return np.stack([116 * f[..., 1] - 16, 500 * (f[..., 0] - f[..., 1]), 200 * (f[..., 1] - f[..., 2])], -1)


Y_WEIGHT = 70.0  # how much where a pixel sits on the body counts against its colour


def features(sheet, fw, fh):
    """Lab colour plus height on the figure (0 head .. 1 feet) for every opaque pixel."""
    a = np.asarray(sheet)
    alpha = a[..., 3]
    rel = np.zeros(alpha.shape, np.float32)
    for y0 in range(0, a.shape[0], fh):
        for x0 in range(0, a.shape[1], fw):
            cell = alpha[y0:y0 + fh, x0:x0 + fw] > 128
            ys = np.nonzero(cell.any(axis=1))[0]
            if len(ys):
                top, bot = ys.min(), ys.max()
                rel[y0:y0 + fh, x0:x0 + fw] = ((np.arange(fh)[:, None] - top) / max(bot - top, 1)).repeat(fw, 1)[:cell.shape[0], :cell.shape[1]]
    return np.concatenate([lab(a[..., :3]), (rel * Y_WEIGHT)[..., None]], -1), alpha


def classifier(k, meta):
    pts, labels = [], []
    for base in ["walk", "walkback", "idle"]:
        a = meta["anims"].get(base if k == 0 else f"{base}_c{k}")
        if not a or not a.get("mask"):
            continue
        sheet = Image.open(OUT / a["file"]).convert("RGBA")
        mask = np.asarray(Image.open(OUT / a["mask"]).convert("L"))
        feat, alpha = features(sheet, a["fw"], a["fh"])
        ok = alpha > 200
        pts.append(feat[ok])
        labels.append(np.clip(np.round(mask[ok] / 40.0), 0, 5).astype(np.int8))
    pts, labels = np.concatenate(pts), np.concatenate(labels)
    pick = np.random.default_rng(k).choice(len(pts), min(len(pts), 80000), replace=False)
    return cKDTree(pts[pick]), labels[pick]


def mask_for(sheet, fw, fh, tree, labels):
    feat, alpha = features(sheet, fw, fh)
    ok = alpha > 8
    _, idx = tree.query(feat[ok], k=7, workers=-1)
    votes = labels[idx]
    counts = np.zeros((len(votes), 6), np.int16)
    for j in range(votes.shape[1]):
        np.add.at(counts, (np.arange(len(votes)), votes[:, j]), 1)
    lab_map = np.zeros(alpha.shape, np.int8)
    lab_map[ok] = counts.argmax(1)
    # a 3x3 majority keeps regions solid where colour alone is ambiguous
    onehot = np.stack([ndimage.uniform_filter((lab_map == i).astype(np.float32), 3) for i in range(6)])
    smooth = onehot.argmax(0).astype(np.uint8)
    smooth[~ok] = 0
    return Image.fromarray(smooth * 40)


def main():
    meta = json.loads((OUT / "meta.json").read_text())
    chars = sorted({int(p.stem[1:].split("_")[0]) for p in CLIPS.glob("c*_*.mp4")})
    jobs = [(k, d) for k in chars for d in DIRS + GAMES + list(ACTS) if (CLIPS / f"c{k}_{d}.mp4").exists() and (ONLY is None or d in ONLY)]
    with Pool() as pool:
        walks = pool.map(loop, jobs)
        stands = dict(zip(chars, pool.map(stand, chars))) if ONLY is None or "stand" in ONLY else {}
    report = {}
    for k in chars:
        tree, labels = classifier(k, meta)
        suffix = "" if k == 0 else f"_c{k}"
        sheets = [(f"stand8{suffix}", stands[k], 5, {"dirs": DIRS}, False)] if k in stands else []
        for kk, d, w in walks:
            if kk != k or not w:
                continue
            name = d if d in GAMES else ACTS[d] if d in ACTS else "walk8_" + d
            extra = {"fps": 24 if d in DIRS or d in SWINGS else 12, "seam": w["seam"]}
            extra.update({key: w[key] for key in ("paddle", "fit") if key in w})
            sheets.append((f"{name}{suffix}", w["frames"], COLS, extra, d in DESK))
        for name, frames, cols, extra, *desk in sheets:
            sheet, fw, fh = pack(frames, f"anim_{name}", cols)
            mask = mask_for(sheet, fw, fh, tree, labels)
            if desk and desk[0]:
                # the desk and chair keep their own colours: where a pixel matches the empty desk, no recolour
                t = desk_template(fw, fh).astype(int)
                a = np.asarray(sheet).astype(int)
                cell = np.tile(t, (a.shape[0] // fh, a.shape[1] // fw, 1))
                same = (cell[..., 3] > 128) & (np.abs(a[..., :3] - cell[..., :3]).sum(-1) < 90)
                mask = Image.fromarray(np.where(same, 0, np.asarray(mask)).astype(np.uint8))
            mask.save(OUT / f"anim_{name}_mask.png", optimize=True)
            meta["anims"][name] = {"file": f"anim_{name}.webp", "fw": fw, "fh": fh, "frames": len(frames), "cols": cols,
                                   "anchor": [0.5, ANCHOR], "body": BODY, "char": k, "mask": f"anim_{name}_mask.png", **extra}
            report[name] = (len(frames), extra.get("seam"))
    missing = [f"c{k}_{d}" for k, d, w in walks if not w]
    (OUT / "meta.json").write_text(json.dumps(meta, indent=1))
    print(json.dumps({"sheets": report, "no_loop": missing}))


if __name__ == "__main__":
    main()

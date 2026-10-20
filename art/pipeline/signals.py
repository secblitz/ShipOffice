"""The signs drawn over heads: a raised hand (waiting on you) and a Z (dozing off).

Generated once as one image on magenta (google/gemini-3.1-flash-image via OpenRouter, with a
character sheet frame attached for the style), then keyed, cut apart and packed here.
Usage: python3 -I signals.py <raw.png> <out_dir>
"""
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image
from scipy import ndimage

RAW, OUT = Path(sys.argv[1]), Path(sys.argv[2])
NAMES = ["hand", "z"]  # left to right in the raw image
HEIGHT = 96  # px per sign in the atlas; drawn 6 to 22 world px tall
PAD = 4


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


def main():
    keyed = key_magenta(np.asarray(Image.open(RAW).convert("RGB")))
    lab, n = ndimage.label(keyed[..., 3] > 128, structure=np.ones((3, 3)))
    # the big blobs, left to right
    sizes = ndimage.sum(np.ones_like(lab), lab, range(1, n + 1))
    big = [i + 1 for i in np.argsort(sizes)[::-1][: len(NAMES)]]
    boxes = sorted((ndimage.find_objects(lab)[i - 1] for i in big), key=lambda s: s[1].start)
    signs = []
    for (ys, xs) in boxes:
        crop = Image.fromarray(keyed[max(ys.start - PAD, 0):ys.stop + PAD, max(xs.start - PAD, 0):xs.stop + PAD])
        w = round(crop.width * HEIGHT / crop.height)
        signs.append(crop.resize((w, HEIGHT), Image.LANCZOS))
    atlas = Image.new("RGBA", (sum(s.width + 2 for s in signs), HEIGHT), (0, 0, 0, 0))
    items, x = {}, 0
    for name, s in zip(NAMES, signs):
        atlas.paste(s, (x, 0))
        items[name] = [x, 0, s.width, s.height]
        x += s.width + 2
    atlas.save(OUT / "signals.webp", lossless=True)
    meta = json.loads((OUT / "meta.json").read_text())
    meta["signals"] = {"file": "signals.webp", "w": atlas.width, "h": atlas.height, "items": items}
    (OUT / "meta.json").write_text(json.dumps(meta, indent=1))
    print(json.dumps(meta["signals"]))


if __name__ == "__main__":
    main()

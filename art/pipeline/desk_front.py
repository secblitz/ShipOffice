"""The desk on its own, without its chair: drawn over a custom character who sits at a desk, so
the desk stands in front of them while the chair (in desk_empty.webp, drawn first) is behind.
Usage: python3 -I desk_front.py <assets_dir>
"""
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

ASSETS = Path(sys.argv[1])
# the desk top and the laptop on it, in desk_empty px; the chair's arm and back are outside it
TOP = [(5, 114), (24, 95), (56, 110), (82, 125), (89, 128), (88, 141), (60, 150), (5, 137)]


def main():
    d = Image.open(ASSETS / "desk_empty.webp").convert("RGBA")
    a = np.asarray(d).astype(np.int16)
    m = Image.new("L", d.size, 0)
    ImageDraw.Draw(m).polygon(TOP, fill=255)
    keep = np.asarray(m) > 0
    # under the top only the wooden legs: warm pixels, not the chair's grey wheels
    warm = (a[..., 0] - a[..., 2] > 28) & (a[..., 3] > 0)
    # with their dark outline: a pixel either side of the wood
    warm = warm | np.roll(warm, 1, axis=1) | np.roll(warm, -1, axis=1)
    rows = np.arange(d.height)[:, None] >= 130
    keep |= rows & warm & (np.arange(d.width)[None, :] <= 89)
    out = np.asarray(d).copy()
    out[..., 3] = np.where(keep, out[..., 3], 0)
    Image.fromarray(out).save(ASSETS / "desk_front.webp", lossless=True)


if __name__ == "__main__":
    main()

"""Crops the window screenshots and saves them as WebP.

    python3 crop.py RAW_DIR OUT_DIR

The shots are 2560x1520 (1280x760 at 2x). A crop is (left, top, right,
bottom) in those pixels. The side bar is 518 pixels wide.
"""

import sys
from pathlib import Path

from PIL import Image

FULL = None
CODE = (518, 64, 1920, 1100)

CROPS = {
    "ctrl-hover": FULL,
    "class-definition": FULL,
    "method-definition": FULL,
    "hover": (518, 64, 2176, 1024),
    "choose": (518, 64, 1500, 640),
    "action-hover": (518, 64, 1920, 780),
    "view-from-action": CODE,
    "view-menu": CODE,
    "i18n-hover": CODE,
    "i18n": (518, 64, 1500, 700),
    "usages": (518, 64, 2560, 1344),
    "blame": (518, 64, 1920, 1520),
    "go-to-symbol": (518, 110, 2080, 880),
    "commands": (518, 64, 2080, 880),
    "go-to-file": (518, 110, 2080, 330),
    "splash": FULL,
    "diff": FULL,
}


def main(raw: Path, out: Path) -> None:
    out.mkdir(parents=True, exist_ok=True)
    for name, box in CROPS.items():
        img = Image.open(raw / f"{name}.png").convert("RGB")
        if box:
            img = img.crop(box)
        img.save(out / f"{name}.webp", "WEBP", quality=86, method=6)
        print(f"{name}.webp {img.width}x{img.height}")


if __name__ == "__main__":
    main(Path(sys.argv[1]), Path(sys.argv[2]))

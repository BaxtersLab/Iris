#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
# Copyright 2026 Baxters Lab
"""Generate Iris's application icon: a blue square with white lowercase "iris".

Run from the repository root:  python3 packaging/generate_icon.py

The PNGs are committed, so this only needs running when the design changes —
the .deb build does NOT depend on Pillow being installed.

This deliberately FAILS if Pillow or the font is missing. The sibling generator
in Smart File Cabinet writes a 1x1 placeholder in that case "so the build does
not fail", which produces a package containing an icon that is technically
present and visibly nothing. A build that cannot make the icon should say so.
"""
from pathlib import Path
import sys

ICON_DIR = Path(__file__).resolve().parent / "icons"
SIZES = (16, 24, 32, 48, 64, 128, 256)

# A deep, saturated blue: readable behind white text at 16px, and distinct from
# the charcoal the application window uses so the icon does not vanish into it.
BLUE = (21, 84, 191, 255)
WHITE = (255, 255, 255, 255)

FONT_CANDIDATES = (
    "/usr/share/fonts/truetype/ubuntu/Ubuntu-B.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
)


def main() -> int:
    try:
        from PIL import Image, ImageDraw, ImageFont
    except ImportError:
        print("FATAL: Pillow is required (pip install Pillow)", file=sys.stderr)
        return 1

    font_path = next((p for p in FONT_CANDIDATES if Path(p).exists()), None)
    if font_path is None:
        print(f"FATAL: none of these fonts exist: {FONT_CANDIDATES}", file=sys.stderr)
        return 1

    ICON_DIR.mkdir(parents=True, exist_ok=True)
    for size in SIZES:
        img = Image.new("RGBA", (size, size), BLUE)
        draw = ImageDraw.Draw(img)

        # Fit the word to the square rather than guessing a point size: at 16px
        # a fixed ratio either overflows or leaves the icon looking empty.
        target_w = size * 0.84
        # Start ABOVE any plausible fit and let the width test bring it down.
        # Starting at 0.55 * size meant the loop exited immediately at large
        # sizes and the word was capped by the starting guess rather than by
        # the square — changing target_w had no visible effect at all.
        point = max(4, int(size * 1.1))
        while point > 4:
            font = ImageFont.truetype(font_path, point)
            box = draw.textbbox((0, 0), "iris", font=font)
            if (box[2] - box[0]) <= target_w:
                break
            point -= 1

        font = ImageFont.truetype(font_path, point)

        # Centre on the INK, measured, in two passes.
        #
        # `textbbox` reports the layout box, which for "iris" includes ascender
        # and descender room the glyphs do not use — centring on it left the
        # word visibly high and up to 5px off horizontally at 256. So: draw
        # once on a scratch image, find the pixels that actually got painted,
        # and offset the real draw by the error. Costs one throwaway render per
        # size and is exact at every size rather than approximately right.
        scratch = Image.new("RGBA", (size, size), (0, 0, 0, 0))
        ImageDraw.Draw(scratch).text((0, 0), "iris", font=font, fill=WHITE)
        ink = scratch.getbbox()
        if ink is None:
            print(f"FATAL: nothing rendered at {size}px", file=sys.stderr)
            return 1
        ink_w, ink_h = ink[2] - ink[0], ink[3] - ink[1]
        x = (size - ink_w) / 2 - ink[0]
        y = (size - ink_h) / 2 - ink[1]
        draw.text((x, y), "iris", font=font, fill=WHITE)

        out = ICON_DIR / f"iris_{size}x{size}.png"
        img.save(out, "PNG")
        print(f"  {out.relative_to(Path.cwd()) if out.is_relative_to(Path.cwd()) else out}")

    # The canonical single file, used for the window icon and as the fallback.
    import shutil
    shutil.copy(ICON_DIR / "iris_256x256.png", ICON_DIR / "iris.png")
    print(f"  {ICON_DIR / 'iris.png'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

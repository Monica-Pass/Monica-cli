"""Render synthetic Ratatui buffer captures, not native terminal screenshots.

Requires Pillow and terminal fonts. Box drawing uses one cell grid, as terminal
renderers do; mixing font corners with hand-drawn rails causes gaps and offsets.
"""

import argparse
from functools import lru_cache
import json
import math
import os
from pathlib import Path
import re

from PIL import Image, ImageDraw, ImageFont


SIZE = 24
CELL_H = 31
PAD = 12
BACKGROUND = (40, 44, 52)
FOREGROUND = (171, 178, 191)
BOX_GLYPHS = frozenset("─│╭╮╰╯")


@lru_cache(maxsize=128)
def box_mask(symbol, width, height):
    """All border strokes share the same center, thickness and cell edges."""
    scale = 4
    w, h = width * scale, height * scale
    cx, cy = w // 2, h // 2
    half = scale // 2
    mask = Image.new("L", (w, h))
    draw = ImageDraw.Draw(mask)
    if symbol == "│":
        draw.rectangle((cx - half, 0, cx + half - 1, h - 1), fill=255)
    elif symbol == "─":
        draw.rectangle((0, cy - half, w - 1, cy + half - 1), fill=255)
    elif symbol in "╭╮╰╯":
        # Leave a straight run at the edge beyond the resampling filter's
        # support, so antialiasing the arc cannot change a neighboring join.
        radius = max(1, min(width // 2 - 3, height // 2 - 3, 5)) * scale
        # Draw one canonical corner, then mirror it. The straight parts reach
        # the cell edges, so adjoining rows never depend on the font baseline.
        draw.arc(
            (cx - half, cy - half, cx + radius * 2 + half - 1,
             cy + radius * 2 + half - 1),
            180, 270, fill=255, width=scale,
        )
        draw.rectangle((cx - half, cy + radius - 1, cx + half - 1, h - 1), fill=255)
        draw.rectangle((cx + radius - 1, cy - half, w - 1, cy + half - 1), fill=255)
        if symbol in "╮╯":
            mask = mask.transpose(Image.Transpose.FLIP_LEFT_RIGHT)
        if symbol in "╰╯":
            mask = mask.transpose(Image.Transpose.FLIP_TOP_BOTTOM)
    else:
        raise ValueError(f"Unsupported box glyph: {symbol!r}")
    return mask.resize((width, height), Image.Resampling.LANCZOS)


def rgb(value, fallback):
    match = re.fullmatch(r"Rgb\((\d+), (\d+), (\d+)\)", value)
    return tuple(map(int, match.groups())) if match else fallback


def font_role(text):
    if any(0x2190 <= ord(char) <= 0x21FF for char in text):
        return "symbols"
    # The mono font has no glyphs above this block, but U+2500-U+257F stays on
    # the mono grid so box drawing keeps lining up with the rails.
    if any(0x2580 <= ord(char) <= 0x25FF for char in text):
        return "symbols"
    if any(0x1F000 <= ord(char) <= 0x1FAFF for char in text):
        return "emoji"
    if any(0x2E80 <= ord(char) <= 0x9FFF or 0xF900 <= ord(char) <= 0xFAFF or
           0xFF00 <= ord(char) <= 0xFFEF for char in text):
        return "cjk"
    return "mono"


def load_fonts(font_dir, system_font_dir):
    fonts = {}
    for bold in (False, True):
        fonts[("mono", bold)] = ImageFont.truetype(
            str(font_dir / f"0xProtoNerdFontMono-{'Bold' if bold else 'Regular'}.ttf"), SIZE)
        for role, filename in (
            ("cjk", "msyhbd.ttc" if bold else "msyh.ttc"),
            ("emoji", "seguiemj.ttf"), ("symbols", "seguisym.ttf"),
        ):
            fonts[(role, bold)] = ImageFont.truetype(str(system_font_dir / filename), SIZE)
    return fonts


def render(source, fonts):
    data = json.loads(source.read_text(encoding="utf-8"))
    cols, rows = data["width"], data["height"]
    cell_w = fonts[("mono", False)].getlength("M")
    image = Image.new("RGB", (math.ceil(cols * cell_w) + PAD * 2,
                              rows * CELL_H + PAD * 2), BACKGROUND)
    draw = ImageDraw.Draw(image)
    covered = set()
    for index, cell in enumerate(data["cells"]):
        if index in covered:
            continue
        column, row = index % cols, index // cols
        x, y = PAD + round(column * cell_w), PAD + row * CELL_H
        # Wide grapheme continuation cells are reset by Ratatui. Preserve the
        # leading cell's background, just as the terminal does.
        width = cell.get("width", 2 if font_role(cell["text"]) != "mono" else 1)
        covered.update(range(index + 1, index + width))
        right = PAD + round((column + width) * cell_w) - 1
        draw.rectangle((x, y, right, y + CELL_H - 1), fill=rgb(cell["bg"], BACKGROUND))
    for index, cell in enumerate(data["cells"]):
        text = cell["text"]
        if index in covered or not text.strip():
            continue
        column, row = index % cols, index // cols
        x, y = PAD + round(column * cell_w), PAD + row * CELL_H
        color = rgb(cell["fg"], FOREGROUND)
        if text in BOX_GLYPHS:
            right = PAD + round((column + 1) * cell_w)
            image.paste(color, (x, y), box_mask(text, right - x, CELL_H))
        else:
            font = fonts[(font_role(text), cell["bold"])]
            draw.text((x, y + SIZE), text, font=font, fill=color, anchor="ls")
        if cell.get("underline"):
            draw.line((x, y + CELL_H - 3, x + cell_w, y + CELL_H - 3), fill=color)
    output = source.with_suffix(".png")
    image.save(output)
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("captures", nargs="+", type=Path, help="Buffer JSON files or directories")
    parser.add_argument("--font-dir", type=Path, default=
                        Path(os.environ.get("LOCALAPPDATA", "C:/Users/Default/AppData/Local")) /
                        "Microsoft/Windows/Fonts")
    parser.add_argument("--system-font-dir", type=Path, default=
                        Path(os.environ.get("WINDIR", "C:/Windows")) / "Fonts")
    args = parser.parse_args()
    fonts = load_fonts(args.font_dir, args.system_font_dir)
    for path in args.captures:
        for source in sorted(path.glob("*.json")) if path.is_dir() else [path]:
            print(render(source, fonts))


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Draw assets/logo.svg from assets/snake.txt with the same rainbow the app and
the installers use (src/brand.rs). Run it again after changing the snake:

    python3 scripts/make-logo.py
"""
import math
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
HUE_SPAN = 0.83


def rainbow(position):
    """Mirror of brand::rainbow: red at 0 to violet-magenta at 1, vivid throughout."""
    hue = min(1.0, max(0.0, position)) * HUE_SPAN * 6
    sector = int(math.floor(hue))
    f = hue - math.floor(hue)
    low, t, q = 0.1, 1 - 0.9 * (1 - f), 1 - 0.9 * f
    r, g, b = [
        (1.0, t, low), (q, 1.0, low), (low, 1.0, t),
        (low, q, 1.0), (t, low, 1.0), (1.0, low, q),
    ][min(sector, 5)]
    return "#%02x%02x%02x" % (round(r * 255), round(g * 255), round(b * 255))


art = (ROOT / "assets/snake.txt").read_text().rstrip("\n").split("\n")
width = max(len(line) for line in art)
CELL_W, CELL_H, PAD = 12, 24, 28
HALF = CELL_H // 2

parts = []
for row, line in enumerate(art):
    for col, ch in enumerate(line):
        if ch not in "█▀▄":
            continue
        # Half blocks are drawn at half height; the 0.5 overlap hides seams between rows.
        top = PAD + row * CELL_H
        y, h = {"█": (top, CELL_H), "▀": (top, HALF), "▄": (top + HALF, HALF)}[ch]
        parts.append(
            f'<rect x="{PAD + col * CELL_W}" y="{y}" width="{CELL_W + 0.5}" height="{h + 0.5}" '
            f'fill="{rainbow(col / (width - 1))}"/>'
        )

snake_h = len(art) * CELL_H
word = "yurlungr"
letters = []
for i, letter in enumerate(word):
    letters.append(f'<tspan fill="{rainbow(i / (len(word) - 1))}">{letter}</tspan>')

total_w = PAD * 2 + width * CELL_W
text_y = PAD + snake_h + 62
svg = f'''<svg xmlns="http://www.w3.org/2000/svg" width="{total_w}" height="{text_y + 56}" viewBox="0 0 {total_w} {text_y + 56}" role="img" aria-label="yurlungr: a rainbow snake">
  <title>yurlungr</title>
  <g shape-rendering="crispEdges">
    {chr(10).join("    " + p for p in parts).strip()}
  </g>
  <text x="{PAD}" y="{text_y}" font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace" font-size="54" font-weight="700" letter-spacing="14">{"".join(letters)}</text>
  <text x="{PAD + 2}" y="{text_y + 34}" font-family="system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif" font-size="19" fill="#8b8b95">a terminal workspace for coding agents</text>
</svg>
'''
(ROOT / "assets/logo.svg").write_text(svg)
print(f"wrote assets/logo.svg ({total_w}x{text_y + 56}, {len(parts)} blocks)")

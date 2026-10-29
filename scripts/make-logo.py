#!/usr/bin/env python3
"""Draw assets/logo.svg from assets/snake.txt with the same rainbow the app and
the installers use (src/brand.rs). Run it again after changing the snake:

    python3 scripts/make-logo.py
"""
import math
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
HUE_SPAN = 0.83


def rainbow(position, k=1.0):
    """Mirror of brand::rainbow: red at 0 to violet-magenta at 1, vivid throughout.
    `k` dims it: 1 for the lit face, 0.6 for the side, 0.3 for the shadow."""
    hue = min(1.0, max(0.0, position)) * HUE_SPAN * 6
    sector = int(math.floor(hue))
    f = hue - math.floor(hue)
    low, t, q = 0.1, 1 - 0.9 * (1 - f), 1 - 0.9 * f
    r, g, b = [
        (1.0, t, low), (q, 1.0, low), (low, 1.0, t),
        (low, q, 1.0), (t, low, 1.0), (1.0, low, q),
    ][min(sector, 5)]
    return "#%02x%02x%02x" % tuple(round(round(c * 255) * k) for c in (r, g, b))


# Glyph -> (top, bottom, brightness). The face is half blocks; the side and the
# shadow are whole cells (see scripts/make-snake.py).
SHADES = {
    "█": (True, True, 1.0), "▀": (True, False, 1.0), "▄": (False, True, 1.0),
    "▓": (True, True, 0.6), "░": (True, True, 0.3),
}


art = (ROOT / "assets/snake.txt").read_text(encoding="utf-8").rstrip("\n").split("\n")
width = max(len(line) for line in art)
CELL_W, CELL_H, PAD = 12, 24, 28
HALF = CELL_H // 2

parts = []
for row, line in enumerate(art):
    for col, ch in enumerate(line):
        if ch not in SHADES:
            continue
        has_top, has_bottom, k = SHADES[ch]
        # Half blocks are drawn at half height; the 0.5 overlap hides seams between rows.
        top = PAD + row * CELL_H
        y = top + (0 if has_top else HALF)
        h = (HALF if has_top else 0) + (HALF if has_bottom else 0)
        parts.append(
            f'<rect x="{PAD + col * CELL_W}" y="{y}" width="{CELL_W + 0.5}" height="{h + 0.5}" '
            f'fill="{rainbow(col / (width - 1), k)}"/>'
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
  <style>
    .flow {{ animation: flow 9s linear infinite; }}
    @keyframes flow {{ from {{ filter: hue-rotate(0deg); }} to {{ filter: hue-rotate(360deg); }} }}
    @media (prefers-reduced-motion: reduce) {{ .flow {{ animation: none; }} }}
  </style>
  <g class="flow">
    <g shape-rendering="crispEdges">
      {chr(10).join("      " + p for p in parts).strip()}
    </g>
    <text x="{PAD}" y="{text_y}" font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace" font-size="54" font-weight="700" letter-spacing="14">{"".join(letters)}</text>
  </g>
  <text x="{PAD + 2}" y="{text_y + 34}" font-family="system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif" font-size="19" fill="#8b8b95">a terminal workspace for coding agents</text>
</svg>
'''
(ROOT / "assets/logo.svg").write_text(svg, encoding="utf-8", newline="\n")
print(f"wrote assets/logo.svg ({total_w}x{text_y + 56}, {len(parts)} blocks)")

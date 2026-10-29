#!/usr/bin/env python3
"""Draw assets/logo.svg from assets/snake.txt and assets/snake-trail.txt with the
same rainbow the app and the installers use (src/brand.rs). The colour of a block
comes from where it lies along the snake's body (the trail), and the animation
runs along the body too: every block runs the same colour cycle, started a little
earlier or later according to its position, so the colours travel from the tail to
the head round every bend. Run it again after changing the snake:

    python scripts/make-logo.py
"""
import math
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
HUE_SPAN = 0.83
BASE62 = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
CYCLE = 9.0      # seconds for the colours to go once round
STOPS = 36       # colour stops per cycle


def hue_color(turn, k=1.0):
    """The rainbow at `turn` of the way round the hue circle (mirror of brand::rainbow_at).
    `k` dims it: 1 for the lit face, 0.6 for the side, 0.3 for the shadow."""
    hue = (turn % 1.0) * 6
    sector = int(math.floor(hue))
    f = hue - math.floor(hue)
    low, t, q = 0.1, 1 - 0.9 * (1 - f), 1 - 0.9 * f
    r, g, b = [
        (1.0, t, low), (q, 1.0, low), (low, 1.0, t),
        (low, q, 1.0), (t, low, 1.0), (1.0, low, q),
    ][min(sector, 5)]
    return "#%02x%02x%02x" % tuple(round(round(c * 255) * k) for c in (r, g, b))


def rainbow(position, k=1.0):
    """Mirror of brand::rainbow: red at 0 to violet-magenta at 1, vivid throughout."""
    return hue_color(min(1.0, max(0.0, position)) * HUE_SPAN, k)


def delay(position):
    """Negative animation delay that starts a block at its own place in the cycle.
    Progress s of the cycle shows turn (1 - s), so a block at turn u0 starts at 1 - u0."""
    u0 = min(1.0, max(0.0, position)) * HUE_SPAN
    return -((1.0 - u0) % 1.0) * CYCLE


# Glyph -> (top, bottom, brightness, class). The face is half blocks; the side and
# the shadow are whole cells (see scripts/make-snake.py).
SHADES = {
    "█": (True, True, 1.0, "f"), "▀": (True, False, 1.0, "f"), "▄": (False, True, 1.0, "f"),
    "▓": (True, True, 0.6, "s"), "░": (True, True, 0.3, "h"),
}
BRIGHTNESS = {"f": 1.0, "s": 0.6, "h": 0.3}

art = (ROOT / "assets/snake.txt").read_text(encoding="utf-8").rstrip("\n").split("\n")
trail = (ROOT / "assets/snake-trail.txt").read_text(encoding="utf-8").rstrip("\n").split("\n")
width = max(len(line) for line in art)
CELL_W, CELL_H, PAD = 12, 24, 28
HALF = CELL_H // 2

parts = []
for row, line in enumerate(art):
    for col, ch in enumerate(line):
        if ch not in SHADES:
            continue
        has_top, has_bottom, k, cls = SHADES[ch]
        position = BASE62.index(trail[row][col]) / 61
        # Half blocks are drawn at half height; the 0.5 overlap hides seams between rows.
        top = PAD + row * CELL_H
        y = top + (0 if has_top else HALF)
        h = (HALF if has_top else 0) + (HALF if has_bottom else 0)
        parts.append(
            f'<rect class="{cls}" x="{PAD + col * CELL_W}" y="{y}" width="{CELL_W + 0.5}" '
            f'height="{h + 0.5}" fill="{rainbow(position, k)}" '
            f'style="animation-delay:{delay(position):.2f}s"/>'
        )

snake_h = len(art) * CELL_H
word = "yurlungr"
letters = []
for i, letter in enumerate(word):
    position = i / (len(word) - 1)
    letters.append(
        f'<tspan class="f" fill="{rainbow(position)}" '
        f'style="animation-delay:{delay(position):.2f}s">{letter}</tspan>'
    )


def keyframes(name, k):
    stops = []
    for i in range(STOPS + 1):
        s = i / STOPS
        stops.append(f"{round(s * 100, 2):g}%{{fill:{hue_color(1.0 - s, k)}}}")
    return f"@keyframes {name}{{{''.join(stops)}}}"


style = "\n    ".join(
    [
        f".f,.s,.h{{animation-duration:{CYCLE:g}s;animation-timing-function:linear;"
        "animation-iteration-count:infinite}",
        ".f{animation-name:flowf}.s{animation-name:flows}.h{animation-name:flowh}",
        keyframes("flowf", BRIGHTNESS["f"]),
        keyframes("flows", BRIGHTNESS["s"]),
        keyframes("flowh", BRIGHTNESS["h"]),
        "@media (prefers-reduced-motion: reduce){.f,.s,.h{animation:none}}",
    ]
)

total_w = PAD * 2 + width * CELL_W
text_y = PAD + snake_h + 62
svg = f'''<svg xmlns="http://www.w3.org/2000/svg" width="{total_w}" height="{text_y + 56}" viewBox="0 0 {total_w} {text_y + 56}" role="img" aria-label="yurlungr: a rainbow snake">
  <title>yurlungr</title>
  <style>
    {style}
  </style>
  <g shape-rendering="crispEdges">
    {chr(10).join("    " + p for p in parts).strip()}
  </g>
  <text x="{PAD}" y="{text_y}" font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace" font-size="54" font-weight="700" letter-spacing="14">{"".join(letters)}</text>
  <text x="{PAD + 2}" y="{text_y + 34}" font-family="system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif" font-size="19" fill="#8b8b95">a terminal workspace for coding agents</text>
</svg>
'''
(ROOT / "assets/logo.svg").write_text(svg, encoding="utf-8", newline="\n")
print(f"wrote assets/logo.svg ({total_w}x{text_y + 56}, {len(parts)} blocks)")

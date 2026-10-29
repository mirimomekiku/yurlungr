#!/usr/bin/env python3
"""Build the 2.5D snake from the flat one in assets/snake-face.txt.

Two files come out, line for line the same shape:

assets/snake.txt        the picture. The flat snake is the lit top face; it is
                        extruded down and to the right (light comes from the top
                        left) to give a darker side, and a fainter shadow beyond
                        that. Every glyph says which part of the picture it is:

                            █ ▀ ▄   face    full brightness
                            ▓       side    60%
                            ░       shadow  30%

assets/snake-trail.txt  where each painted cell lies along the snake's body, from
                        the tail tip (0) to the head (61), one base-62 digit per
                        cell (0-9, a-z, A-Z). The rainbow is laid along this trail
                        (red tail to violet head) and its animation runs along it,
                        so the colours follow the body round every bend. Side and
                        shadow cells take the position of the face cell they were
                        extruded from.

The trail is worked out from the face alone: it is the distance along the body
from the tail tip, which is the left-most pixel of the face (the lowest, if
several). Edit the face, then run this again:

    python scripts/make-snake.py

The colour code (src/brand.rs, install.sh, install.ps1, make-logo.py) reads both.
"""
import pathlib
from collections import deque

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEPTH = 2    # pixels of side
SHADOW = 1   # further pixels of shadow
BASE62 = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"

face_rows = (ROOT / "assets/snake-face.txt").read_text(encoding="utf-8").rstrip("\n").split("\n")
rows = len(face_rows)
cols = max(len(r) for r in face_rows)

# A pixel is half a character cell tall, so a pixel is about as wide as it is tall.
face = set()
for r, line in enumerate(face_rows):
    for c, ch in enumerate(line):
        if ch in "█▀":
            face.add((c, 2 * r))
        if ch in "█▄":
            face.add((c, 2 * r + 1))

# ---- the trail: distance along the body from the tail tip -----------------------
tail = min(face, key=lambda p: (p[0], -p[1]))
dist = {tail: 0}
queue = deque([tail])
while queue:
    x, y = queue.popleft()
    for dx in (-1, 0, 1):
        for dy in (-1, 0, 1):
            q = (x + dx, y + dy)
            if q in face and q not in dist:
                dist[q] = dist[(x, y)] + 1
                queue.append(q)
# Pixels not joined to the body (a tongue, say) take their nearest neighbour's value.
for p in sorted(face - set(dist)):
    near = min(dist, key=lambda q: abs(q[0] - p[0]) + abs(q[1] - p[1]))
    dist[p] = dist[near]
longest = max(dist.values()) or 1
trail_of = {p: d / longest for p, d in dist.items()}  # 0.0 tail .. 1.0 head


def shifted(k):
    return {(x + k, y + k) for x, y in face}


side = set().union(*(shifted(k) for k in range(1, DEPTH + 1))) - face
shadow = set().union(*(shifted(k) for k in range(DEPTH + 1, DEPTH + SHADOW + 1))) - face - side

FACE, SIDE, SHADOW_CLASS = 3, 2, 1  # priority: higher wins when a cell mixes classes


def klass(p):
    return FACE if p in face else SIDE if p in side else SHADOW_CLASS if p in shadow else 0


def source(p):
    """The face pixel a side or shadow pixel was extruded from."""
    if p in face:
        return p
    for k in range(1, DEPTH + SHADOW + 1):
        q = (p[0] - k, p[1] - k)
        if q in face:
            return q
    raise AssertionError(f"no face pixel behind {p}")


def cell_trail(top, bottom):
    kinds = {p: klass(p) for p in (top, bottom)}
    best = max(kinds.values())
    ts = [trail_of[source(p)] for p, k in kinds.items() if k == best]
    return sum(ts) / len(ts)


out_cols = cols + DEPTH + SHADOW
out_rows = (2 * rows + DEPTH + SHADOW + 1) // 2
art, trail = [], []
for r in range(out_rows):
    line, marks = "", ""
    for c in range(out_cols):
        top, bottom = (c, 2 * r), (c, 2 * r + 1)
        best = max(klass(top), klass(bottom))
        if best == 0:
            line += " "
            marks += " "
            continue
        if best == FACE:
            line += "█" if klass(top) == klass(bottom) else "▀" if klass(top) == FACE else "▄"
        else:
            line += "▓" if best == SIDE else "░"
        marks += BASE62[round(cell_trail(top, bottom) * 61)]
    art.append(line.rstrip())
    trail.append(marks[: len(art[-1])])

while art and not art[-1]:
    art.pop()
    trail.pop()
(ROOT / "assets/snake.txt").write_text("\n".join(art) + "\n", encoding="utf-8", newline="\n")
(ROOT / "assets/snake-trail.txt").write_text(
    "\n".join(trail) + "\n", encoding="utf-8", newline="\n"
)
print(f"wrote assets/snake.txt and assets/snake-trail.txt ({max(len(l) for l in art)}x{len(art)})")

#!/usr/bin/env python3
"""Build the 2.5D snake, assets/snake.txt, from the flat one in assets/snake-face.txt.

The flat snake is the lit top face. It is extruded down and to the right (light
comes from the top left) to give a darker side, and a longer, fainter shadow
beyond that. Every glyph says which part of the picture it belongs to, and the
colour code (src/brand.rs, install.sh, install.ps1, make-logo.py) reads that:

    █ ▀ ▄   face    full brightness
    ▓       side    60%
    ░       shadow  30%

Run it again after changing the face:

    python scripts/make-snake.py
"""
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEPTH = 2    # pixels of side
SHADOW = 2   # further pixels of shadow

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


def shifted(k):
    return {(x + k, y + k) for x, y in face}


side = set().union(*(shifted(k) for k in range(1, DEPTH + 1))) - face
shadow = set().union(*(shifted(k) for k in range(DEPTH + 1, DEPTH + SHADOW + 1))) - face - side

FACE, SIDE, SHADOW = 3, 2, 1  # priority: higher wins when a cell mixes classes


def klass(p):
    return FACE if p in face else SIDE if p in side else SHADOW if p in shadow else 0


out_cols = cols + DEPTH + SHADOW
out_rows = (2 * rows + DEPTH + SHADOW + 1) // 2
art = []
for r in range(out_rows):
    line = ""
    for c in range(out_cols):
        top, bottom = klass((c, 2 * r)), klass((c, 2 * r + 1))
        best = max(top, bottom)
        if best == 0:
            line += " "
        elif best == FACE:
            line += "█" if top == bottom else "▀" if top == FACE else "▄"
        else:
            line += "▓" if best == SIDE else "░"
    art.append(line.rstrip())

while art and not art[-1]:
    art.pop()
(ROOT / "assets/snake.txt").write_text("\n".join(art) + "\n", encoding="utf-8", newline="\n")
print(f"wrote assets/snake.txt ({max(len(l) for l in art)}x{len(art)})")

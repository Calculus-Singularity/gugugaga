#!/usr/bin/env python3
"""Generate animated ASCII art frames — rotating 3D wireframe cube.

Clean wireframe cube that fits entirely within the 17×42 grid with
margin.  Back-face culling hides edges behind the cube.  Smooth line
drawing with locally-adaptive characters.

Output: 17 rows × 42 columns, 36 frames → frames/default/frame_N.txt
"""

from __future__ import annotations

import math
from pathlib import Path

ROWS = 17
COLS = 42
NUM_FRAMES = 36
SCALE = 4.2  # cube half-size — keeps cube inside canvas after rotation
ASPECT = 2.1  # terminal char height/width ratio


# ── 3D math ───────────────────────────────────────────────────────────────

def _rot(x, y, z, ax, ay, az):
    """Apply Euler rotation (X then Y then Z)."""
    # X
    s, c = math.sin(ax), math.cos(ax)
    y, z = y * c - z * s, y * s + z * c
    # Y
    s, c = math.sin(ay), math.cos(ay)
    x, z = x * c + z * s, -x * s + z * c
    # Z
    s, c = math.sin(az), math.cos(az)
    x, y = x * c - y * s, x * s + y * c
    return x, y, z


def _project(x, y, _z) -> tuple[float, float]:
    return (COLS / 2.0 + x * ASPECT, ROWS / 2.0 + y)


# ── Cube definition ───────────────────────────────────────────────────────

_VERTS = [
    (-1, -1, -1), ( 1, -1, -1), ( 1,  1, -1), (-1,  1, -1),  # back
    (-1, -1,  1), ( 1, -1,  1), ( 1,  1,  1), (-1,  1,  1),  # front
]

_EDGES = [
    (0, 1), (1, 2), (2, 3), (3, 0),
    (4, 5), (5, 6), (6, 7), (7, 4),
    (0, 4), (1, 5), (2, 6), (3, 7),
]

# Each face is 4 vertex indices (in CCW order when seen from outside)
_FACES = [
    (0, 3, 2, 1),  # back   (z = -1)
    (4, 5, 6, 7),  # front  (z = +1)
    (0, 1, 5, 4),  # bottom (y = -1)
    (2, 3, 7, 6),  # top    (y = +1)
    (0, 4, 7, 3),  # left   (x = -1)
    (1, 2, 6, 5),  # right  (x = +1)
]

# Which edges belong to which faces
_FACE_EDGES = [
    {(0,1),(1,2),(2,3),(3,0)},
    {(4,5),(5,6),(6,7),(7,4)},
    {(0,1),(1,5),(5,4),(0,4)},
    {(2,3),(3,7),(7,6),(2,6)},
    {(0,3),(3,7),(7,4),(0,4)},
    {(1,2),(2,6),(6,5),(1,5)},
]


def _face_visible(verts_3d, face, cam_z=10.0):
    """Check if a face is front-facing (visible) using cross-product normal."""
    a, b, c, _ = face
    ax, ay, az = verts_3d[a]
    bx, by, bz = verts_3d[b]
    cx, cy, cz = verts_3d[c]
    # Two edge vectors
    e1 = (bx - ax, by - ay, bz - az)
    e2 = (cx - ax, cy - ay, cz - az)
    # Normal (cross product)
    nx = e1[1] * e2[2] - e1[2] * e2[1]
    ny = e1[2] * e2[0] - e1[0] * e2[2]
    nz = e1[0] * e2[1] - e1[1] * e2[0]
    # Camera looks along -Z, so face visible if nz > 0
    return nz > 0


# ── Line drawing ──────────────────────────────────────────────────────────

def _draw_line(canvas, c0, r0, c1, r1):
    """Draw line with locally-adaptive characters."""
    ic0, ir0 = round(c0), round(r0)
    ic1, ir1 = round(c1), round(r1)

    dc = ic1 - ic0
    dr = ir1 - ir0
    steps = max(abs(dc), abs(dr))
    if steps == 0:
        if 0 <= ir0 < ROWS and 0 <= ic0 < COLS:
            canvas[ir0][ic0] = "+"
        return

    # Sample points along the line
    prev_c, prev_r = ic0, ir0
    for i in range(steps + 1):
        t = i / steps
        c = round(c0 + t * (c1 - c0))
        r = round(r0 + t * (r1 - r0))
        if 0 <= r < ROWS and 0 <= c < COLS:
            if i == 0:
                sc, sr = ic1 - ic0, ir1 - ir0
            else:
                sc, sr = c - prev_c, r - prev_r
            # Pick char based on local direction
            if sr == 0:
                ch = "-"
            elif sc == 0:
                ch = "|"
            elif (sc > 0 and sr > 0) or (sc < 0 and sr < 0):
                ch = "\\"
            else:
                ch = "/"
            canvas[r][c] = ch
        prev_c, prev_r = c, r


def _render_frame(frame: int) -> str:
    t = 2.0 * math.pi * frame / NUM_FRAMES
    ax = 0.6                          # fixed ~34° tilt
    ay = t + math.pi / 4             # start at 45° (isometric)
    az = t * 0.35 + 0.3              # slow tumble with offset

    # Transform vertices
    verts_3d = []
    verts_2d = []
    for vx, vy, vz in _VERTS:
        x, y, z = _rot(vx * SCALE, vy * SCALE, vz * SCALE, ax, ay, az)
        verts_3d.append((x, y, z))
        verts_2d.append(_project(x, y, z))

    # Determine visible edges via back-face culling
    visible_edges = set()
    for fi, face in enumerate(_FACES):
        if _face_visible(verts_3d, face):
            for e in _FACE_EDGES[fi]:
                visible_edges.add(e)

    # Canvas
    canvas = [[" "] * COLS for _ in range(ROWS)]

    # Draw visible edges
    for i0, i1 in _EDGES:
        if (i0, i1) not in visible_edges and (i1, i0) not in visible_edges:
            continue
        c0, r0 = verts_2d[i0]
        c1, r1 = verts_2d[i1]
        _draw_line(canvas, c0, r0, c1, r1)

    # Draw vertices
    for sc, sr in verts_2d:
        c, r = round(sc), round(sr)
        if 0 <= r < ROWS and 0 <= c < COLS:
            canvas[r][c] = "o"

    return "\n".join("".join(row) for row in canvas)


def main() -> None:
    root = Path(__file__).resolve().parent.parent / "frames" / "default"
    root.mkdir(parents=True, exist_ok=True)
    for f in range(1, NUM_FRAMES + 1):
        text = _render_frame(f - 1)
        (root / f"frame_{f}.txt").write_text(text, encoding="utf-8")
    print(f"  ✓ {NUM_FRAMES} frames → {root}")


if __name__ == "__main__":
    main()

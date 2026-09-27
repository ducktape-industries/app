#!/usr/bin/env python3
"""Bakes the app's figures into looping frames of glyphs.

The app draws no figure itself: it plays these. Each figure is a signed
distance field, raymarched here once per frame and shaded into the ramp
`src/shell/figure.rs` names. A frame is COLS x ROWS cells, row by row; a
cell is 0 (blank) or a ramp step 1..11, two cells a byte (high nibble
first). Every motion turns whole over the loop, so the last frame runs
back into the first.

    python3 dev/figures/bake.py        # writes assets/figures/*.bin
"""

import pathlib

import numpy as np

COLS, ROWS = 46, 30
SIZE, ADVANCE = 11.0, 7.1  # a cell's height and width, px
FRAMES = 120  # 6 s at 20 frames a second
STEPS = 11  # ramp steps past blank
LIGHT = np.array([-0.55, -0.6, -0.58])
REACH = 1.4
TAU = 2 * np.pi

OUT = pathlib.Path(__file__).resolve().parents[2] / "assets" / "figures"


def rot(p, ax, ay, az):
    """Turns points about x, then y, then z."""
    x, y, z = p[..., 0], p[..., 1], p[..., 2]
    c, s = np.cos(ax), np.sin(ax)
    y, z = y * c - z * s, y * s + z * c
    c, s = np.cos(ay), np.sin(ay)
    x, z = x * c + z * s, -x * s + z * c
    c, s = np.cos(az), np.sin(az)
    x, y = x * c - y * s, x * s + y * c
    return np.stack([x, y, z], axis=-1)


def length(v):
    return np.sqrt((v * v).sum(axis=-1))


def sphere(p, c, r):
    return length(p - np.array(c)) - r


def torus(p, big, small):
    q = np.sqrt(p[..., 0] ** 2 + p[..., 2] ** 2) - big
    return np.sqrt(q * q + p[..., 1] ** 2) - small


def rounded_box(p, b, r=0.04):
    q = np.abs(p) - np.array(b)
    outside = length(np.maximum(q, 0.0))
    inside = np.minimum(q.max(axis=-1), 0.0)
    return outside + inside - r


def box(p, b):
    return rounded_box(p, b, 0.0)


# -- the figures: each is (sdf, tone) for a loop phase in 0..1 ------------


def roll(phase):
    """A roll of duct tape, turning and rocking, its loose end trailing."""
    tilt = 1.15 + 0.18 * np.sin(TAU * phase)
    spin = TAU * phase

    def local(p):
        return rot(rot(p, tilt, spin, 0.25), 0.0, 0.0, 0.0)

    def sdf(p):
        q = local(p)
        r = np.sqrt(q[..., 0] ** 2 + q[..., 2] ** 2)
        # the roll: a hollow cylinder, softly rounded
        d = np.stack([np.abs(r - 0.78) - 0.24, np.abs(q[..., 1]) - 0.3], axis=-1)
        body = length(np.maximum(d, 0.0)) + np.minimum(d.max(axis=-1), 0.0) - 0.03
        # the loose end: a strip leaving the outer face along its tangent
        t = q - np.array([0.0, 0.0, 1.02])
        end = box(rot(t, 0.0, 0.0, 0.0) - np.array([0.36, 0.0, 0.0]), [0.36, 0.3, 0.012])
        return np.minimum(body, end)

    def tone(p):
        q = local(p)
        r = np.sqrt(q[..., 0] ** 2 + q[..., 2] ** 2)
        # the wound layers show as rings on the flat faces; the core is card
        # a few broad bands: finer ones alias into speckle at this size
        layers = np.where(np.mod(r * 6.0, 1.0) < 0.5, 0.8, 1.0)
        face = np.abs(q[..., 1]) > 0.29
        core = r < 0.57
        return np.where(core, 0.55, np.where(face, layers, 1.0))

    return sdf, tone


def ring(phase):
    """A ring, wobbling as it turns: a key."""
    wobble = 1.05 + 0.25 * np.sin(TAU * phase)

    def sdf(p):
        return torus(rot(rot(p, 0.0, TAU * phase, 0.0), wobble, 0.0, 0.35), 0.78, 0.28)

    return sdf, lambda p: np.ones(p.shape[:-1])


def sheets(phase):
    """A written card, swaying: the recovery phrase on paper. A whole turn
    would show its edge, a sliver, twice a loop."""
    sway = 0.75 * np.sin(TAU * phase)
    nod = 0.18 * np.sin(2 * TAU * phase)

    def local(p):
        return rot(rot(p, nod, sway, 0.0), 0.2, 0.0, 0.12)

    def sdf(p):
        return rounded_box(local(p), [0.62, 0.84, 0.025])

    def tone(p):
        return np.where(np.mod(local(p)[..., 1] * 5.5, 1.0) < 0.4, 0.55, 1.0)

    return sdf, tone


def pair(phase):
    """A sphere and its small moon: you, and the network you joined."""
    c, s = np.cos(TAU * phase), np.sin(TAU * phase)

    def sdf(p):
        q = rot(p, 0.0, TAU * phase * 0.5, 0.0)
        return np.minimum(
            sphere(q, [-0.15, 0.1, 0.0], 0.82),
            sphere(q, [-0.15 + c, 0.1 - 0.3 * c, s], 0.2),
        )

    return sdf, lambda p: np.ones(p.shape[:-1])


FIGURES = {"roll": roll, "ring": ring, "sheets": sheets, "pair": pair}


# -- the render ------------------------------------------------------------


def shade(sdf, tone):
    """One frame's brightness per cell, `0..=1`, or -1 where nothing is."""
    span = 1.25
    ux = span / (COLS / 2)
    uy = ux * SIZE / ADVANCE
    i, j = np.meshgrid(np.arange(COLS), np.arange(ROWS))
    x0 = (i - COLS / 2 + 0.5) * ux
    y0 = (j - ROWS / 2 + 0.5) * uy
    # four rays a cell: an edge half covered reads half lit
    offsets = [(-0.25, -0.25), (0.25, -0.25), (-0.25, 0.25), (0.25, 0.25)]
    x = np.stack([x0 + sx * ux for sx, _ in offsets], axis=-1)
    y = np.stack([y0 + sy * uy for _, sy in offsets], axis=-1)
    wide = REACH**2 - x * x - y * y
    live = wide > 0
    far = np.sqrt(np.maximum(wide, 0.0))
    z = -far
    hit = np.zeros_like(live)
    for _ in range(96):
        d = sdf(np.stack([x, y, z], axis=-1))
        hit |= live & (d < 0.002)
        going = live & ~hit & (z <= far)
        z = np.where(going, z + d, z)
    hit &= z <= far
    light = LIGHT / np.linalg.norm(LIGHT)
    half = light - np.array([0.0, 0.0, 1.0])
    half /= np.linalg.norm(half)
    e = 0.002
    p = np.stack([x, y, z], axis=-1)
    n = np.stack(
        [
            sdf(p + [e, 0, 0]) - sdf(p - [e, 0, 0]),
            sdf(p + [0, e, 0]) - sdf(p - [0, e, 0]),
            sdf(p + [0, 0, e]) - sdf(p - [0, 0, e]),
        ],
        axis=-1,
    )
    n /= np.maximum(length(n), 1e-9)[..., None]
    diffuse = np.maximum((n * light).sum(axis=-1), 0.0)
    shine = np.maximum((n * half).sum(axis=-1), 0.0) ** 24
    rim = np.clip(1.0 + n[..., 2], 0.0, 1.0) ** 3
    lit = (0.1 + 0.72 * diffuse + 0.35 * shine + 0.22 * rim) * tone(p)
    total = np.where(hit, lit, 0.0).sum(axis=-1)
    hits = hit.sum(axis=-1)
    return np.where(hits > 0, np.clip(total / 4, 0.0, 1.0) ** 1.15, -1.0)


def steps(lum):
    """A cell's ramp step: 0 blank; a hit is never blank."""
    return np.where(lum < 0, 0, np.clip(np.round(lum * STEPS), 1, STEPS)).astype(np.uint8)


def bake(figure):
    frames = [steps(shade(*figure(k / FRAMES))).ravel() for k in range(FRAMES)]
    cells = np.concatenate(frames)
    return ((cells[0::2] << 4) | cells[1::2]).astype(np.uint8).tobytes()


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    for name, figure in FIGURES.items():
        data = bake(figure)
        (OUT / f"{name}.bin").write_bytes(data)
        print(f"{name}: {FRAMES} frames, {len(data)} bytes")

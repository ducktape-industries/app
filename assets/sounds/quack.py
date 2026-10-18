#!/usr/bin/env python3
"""The quack every ordinary notice plays, synthesized (no recording, so no
licence): `quack.wav` beside this file, 16-bit PCM, mono, 44.1 kHz, 0.36 s,
its peak at -6 dBFS.

    python3 assets/sounds/quack.py           # writes quack.wav
    python3 assets/sounds/quack.py --check   # exits 1 unless quack.wav is what it writes

The same bytes on every machine with IEEE-754 doubles (every one Python
runs on today): a sample is made of + - * / and integers only. The cosine
and exponential below are fixed-length series and the noise an integer LCG;
nothing goes through libm, whose last bits differ between platforms.

A buzzy pulse train (a rounded glottal pulse, differentiated) whose pitch
jumps up and falls away, with a little breath, through three resonators
at a duck's nasal formants, under a fast attack and a long decay.

quack.wav sha256: 82a222b17c7433b57b3e757c3efaeac3af13627ef7d303666d72b7f30dbe21fa
"""

import hashlib
import struct
import sys
from pathlib import Path

RATE = 44100
COUNT = RATE * 36 // 100
PI = 3.141592653589793
PEAK = 0.5


def cos(x):
    """cos x for |x| <= pi, by its Taylor series."""
    total = term = 1.0
    for k in range(1, 24):
        term = -term * x * x / ((2 * k - 1) * (2 * k))
        total += term
    return total


def exp(x):
    """e^x for small |x|, by its Taylor series."""
    total = term = 1.0
    for k in range(1, 40):
        term = term * x / k
        total += term
    return total


class Resonator:
    """A two-pole resonance at `hz`, `width` Hz wide."""

    def __init__(self, hz, width, gain):
        r = exp(-PI * width / RATE)
        self.b1 = 2 * r * cos(2 * PI * hz / RATE)
        self.b2 = -r * r
        self.a0 = (1 - r) * gain
        self.y1 = self.y2 = 0.0

    def __call__(self, x):
        y = self.a0 * x + self.b1 * self.y1 + self.b2 * self.y2
        self.y2, self.y1 = self.y1, y
        return y


class Noise:
    """An integer LCG, in [-1, 1)."""

    def __init__(self):
        self.state = 38

    def __call__(self):
        self.state = (self.state * 1103515245 + 12345) % 2**31
        return self.state / 2**30 - 1.0


def pitch(t):
    """Hz at `t` seconds: up from 240 to 300 in 30 ms, then down to 170."""
    if t < 0.03:
        return 240 + 60 * t / 0.03
    return 300 - 130 * (t - 0.03) / (COUNT / RATE - 0.03)


def envelope(t):
    """Up in 15 ms, held to 80 ms, then away to nothing at the end."""
    if t < 0.015:
        u = t / 0.015
        return u * u
    if t < 0.08:
        return 1.0
    left = 1 - (t - 0.08) / (COUNT / RATE - 0.08)
    return left * left


def glottal(phase):
    """One period of a rounded pulse: open over 55%, shut by 80%."""
    if phase < 0.55:
        u = phase / 0.55
        return 3 * u * u - 2 * u * u * u
    if phase < 0.8:
        v = (phase - 0.55) / 0.25
        return 1 - v * v
    return 0.0


def samples():
    formants = [
        Resonator(1050, 140, 1.0),
        Resonator(2400, 200, 1.3),
        Resonator(3500, 350, 0.6),
    ]
    noise = Noise()
    phase, last, jitter = 0.0, 0.0, 1.0
    out = []
    for n in range(COUNT):
        t = n / RATE
        hz = pitch(t) * jitter
        phase += hz / RATE
        if phase >= 1.0:
            phase -= 1.0
            jitter = 1 + 0.03 * noise()
        pulse = glottal(phase)
        source = (pulse - last) * RATE / hz
        last = pulse
        x = (source + 0.15 * noise()) * envelope(t)
        out.append(sum(formant(x) for formant in formants))
    # no DC: a one-pole high-pass
    blocked, x1, y1 = [], 0.0, 0.0
    for x in out:
        y1 = x - x1 + 0.995 * y1
        x1 = x
        blocked.append(y1)
    peak = max(abs(y) for y in blocked)
    return [round(y / peak * PEAK * 32767) for y in blocked]


def wav(pcm):
    data = b"".join(struct.pack("<h", sample) for sample in pcm)
    fmt = struct.pack("<IHHIIHH", 16, 1, 1, RATE, RATE * 2, 2, 16)
    riff = b"WAVE" + b"fmt " + fmt + b"data" + struct.pack("<I", len(data)) + data
    return b"RIFF" + struct.pack("<I", len(riff)) + riff


def main():
    made = wav(samples())
    file = Path(__file__).with_name("quack.wav")
    if sys.argv[1:] == ["--check"]:
        kept = file.read_bytes() if file.exists() else b""
        if kept != made:
            sys.exit(f"{file} is not what quack.py writes")
    else:
        file.write_bytes(made)
    print(hashlib.sha256(made).hexdigest(), file)


if __name__ == "__main__":
    main()

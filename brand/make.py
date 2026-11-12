"""Draws tparquet's brand files in Aperiodic's pixel style: the mark, the logo, the icon, the
README's size figure and the social preview.

    pip install fonttools uharfbuzz
    python3 brand/make.py --fonts DIR

DIR holds Mona Sans Expanded (MonaSans-Bold.ttf, MonaSans-Regular.ttf) and JetBrains Mono
(JetBrainsMono.ttf, the variable font), both under the SIL Open Font License. Every word is
converted to outlines, so the files render the same without the fonts installed. The figure and
the social preview read their numbers from bench/results.json, the last `make bench`.

social-preview.png is social-preview.svg rendered at 1280x640, for example with
`rsvg-convert social-preview.svg -o social-preview.png`.
"""

from __future__ import annotations

import argparse
import json
from datetime import date
from pathlib import Path

import uharfbuzz as hb
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

HERE = Path(__file__).resolve().parent
RESULTS = HERE.parent / "bench" / "results.json"
BENCH_DATE = date(2026, 11, 7)  # the run in bench/RESULTS.md

# Aperiodic's tokens (aperiodic.io, styles/design-v2.css): paper and black, the pixel field
# and its quiet ramp, and orange for the one thing worth seeing.
LIGHT = {
    "paper": "#f6f6f5",
    "ink": "#141414",
    "muted": "#5e6066",
    "rule": "#d0d6de",
    "signal": "#e8571d",
    "signal_text": "#ad3f0e",
    "base": "#f6f6f5",
    "grout": "#eeeff0",
    "low": "#c9d0da",
    "high": "#1f2937",
}
DARK = {
    "paper": "#0c0c0d",
    "ink": "#efefed",
    "muted": "#9a9aa1",
    "rule": "#232323",
    "signal": "#ef5a1f",
    "signal_text": "#f06a33",
    "base": "#141415",
    "grout": "#08080a",
    "low": "#2c2017",
    "high": "#ffdcb4",
}

CELL = 4  # a tile is 3 units with 1 unit of grout on its top and left, as on aperiodic.io

# The mark: four planks of tiles, 5, 3, 2 and 1 wide, getting quieter, the last one orange.
# Parquet columns, compressed. (first column, width) in a field of 16 x 16 cells.
PLANKS = [(1, 5), (7, 3), (11, 2), (14, 1)]
PLANK_ROWS = range(1, 15)
PLANK_SHADES = [1.0, 0.72, 0.48]  # on the quiet ramp; the fourth plank is the signal

# The Aperiodic mark as aperiodic.io draws it (its Logo component): circles (cx, cy, r) in a
# 100 x 100 box. Drawn from these coordinates, never redrawn.
APERIODIC = [
    (50, 50, 7), (50, 34, 4.5), (65.2031, 45.1016, 4.5), (59.3984, 62.8984, 4.5),
    (40.6016, 62.8984, 4.5), (34.7969, 45.1016, 4.5), (50, 16, 2), (70, 22.5, 2),
    (82.2969, 39.5, 2), (82.2969, 60.5, 2), (70, 77.5, 2), (50, 84, 2), (30, 77.5, 2),
    (17.7031, 60.5, 2), (17.7031, 39.5, 2), (30, 22.5, 2),
]  # fmt: skip


# ---- colours ----------------------------------------------------------------------------------
def _rgb(h):
    return tuple(int(h[i : i + 2], 16) for i in (1, 3, 5))


def _hex(c):
    return "#" + "".join(f"{round(v):02x}" for v in c)


def quiet(p, t):
    """The site's quiet ramp: the empty tile at 0, its low stop at 0.1, its high stop at 1."""
    lo, hi = _rgb(p["low"]), _rgb(p["high"])
    k = (t - 0.1) / 0.9
    return _hex(tuple(a + (b - a) * k for a, b in zip(lo, hi)))


def planks(p):
    return [quiet(p, t) for t in PLANK_SHADES] + [p["signal"]]


# ---- words, as outlines -----------------------------------------------------------------------
def _num(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


class Face:
    def __init__(self, path, wght=None):
        self.tt = TTFont(path)
        if wght is not None and "fvar" in self.tt:
            self.tt = instantiateVariableFont(self.tt, {"wght": wght})
        self.glyphs = self.tt.getGlyphSet()
        self.order = self.tt.getGlyphOrder()
        self.upem = self.tt["head"].unitsPerEm
        self.hb = hb.Font(hb.Face(Path(path).read_bytes()))
        if wght is not None:
            self.hb.set_variations({"wght": wght})

    def shape(self, s):
        buf = hb.Buffer()
        buf.add_str(s)
        buf.guess_segment_properties()
        hb.shape(self.hb, buf, {"kern": True, "liga": False, "calt": False})
        return [(self.order[i.codepoint], p.x_advance) for i, p in zip(buf.glyph_infos, buf.glyph_positions)]

    def width(self, s, size, tracking=0.0):
        adv = sum(a for _, a in self.shape(s)) * size / self.upem
        return adv + tracking * size * max(0, len(s) - 1)

    def path(self, s, size, x, y, anchor="l", tracking=0.0):
        """`s` as one SVG path at baseline `y`, starting at, centred on or ending at `x`."""
        x -= {"l": 0, "m": 0.5, "r": 1}[anchor] * self.width(s, size, tracking)
        k = size / self.upem
        pen = SVGPathPen(self.glyphs, ntos=_num)
        for name, adv in self.shape(s):
            self.glyphs[name].draw(TransformPen(pen, (k, 0, 0, -k, x, y)))
            x += adv * k + tracking * size
        return pen.getCommands()


class Fonts:
    def __init__(self, folder):
        d = Path(folder)
        self.bold = Face(d / "MonaSans-Bold.ttf")
        self.sans = Face(d / "MonaSans-Regular.ttf")
        self.mono = Face(d / "JetBrainsMono.ttf", 400)
        self.mono_bold = Face(d / "JetBrainsMono.ttf", 700)


# ---- an SVG -----------------------------------------------------------------------------------
class Svg:
    def __init__(self, w, h, label):
        self.w, self.h, self.label = w, h, label
        self.defs, self.body = [], []

    def add(self, s):
        self.body.append(s)

    def field(self, p):
        """Empty tiles over the whole picture, as the site's pixel layer draws its pages."""
        self.defs.append(
            f'<pattern id="field" width="{CELL}" height="{CELL}" patternUnits="userSpaceOnUse">'
            f'<rect width="{CELL}" height="{CELL}" fill="{p["grout"]}"/>'
            f'<rect x="1" y="1" width="3" height="3" fill="{p["base"]}"/></pattern>'
        )
        self.add(f'<rect width="{self.w}" height="{self.h}" fill="url(#field)"/>')

    def shade(self, p, y0, y1, a0, a1=None):
        """The page colour over the field where words sit, fading from `a0` to `a1`."""
        a1 = a0 if a1 is None else a1
        gid = f"shade{len(self.defs)}"
        self.defs.append(
            f'<linearGradient id="{gid}" x1="0" y1="0" x2="0" y2="1">'
            f'<stop offset="0" stop-color="{p["paper"]}" stop-opacity="{a0}"/>'
            f'<stop offset="1" stop-color="{p["paper"]}" stop-opacity="{a1}"/></linearGradient>'
        )
        self.add(f'<rect y="{y0}" width="{self.w}" height="{y1 - y0}" fill="url(#{gid})"/>')

    def hshade(self, p, stops):
        """The page colour over the field, left to right: (offset 0 to 1, opacity) stops."""
        gid = f"shade{len(self.defs)}"
        rows = "".join(f'<stop offset="{o}" stop-color="{p["paper"]}" stop-opacity="{a}"/>' for o, a in stops)
        self.defs.append(f'<linearGradient id="{gid}" x1="0" y1="0" x2="1" y2="0">{rows}</linearGradient>')
        self.add(f'<rect width="{self.w}" height="{self.h}" fill="url(#{gid})"/>')

    def tiles(self, cells, color, ox=0, oy=0):
        """Tiles at cells (i, j), in one path."""
        d = "".join(f"M{ox + i * CELL + 1} {oy + j * CELL + 1}h3v3h-3z" for i, j in cells)
        if d:
            self.add(f'<path fill="{color}" d="{d}"/>')

    def text(self, face, s, size, x, y, color, anchor="l", tracking=0.0):
        self.add(f'<path fill="{color}" d="{face.path(s, size, x, y, anchor, tracking)}"/>')

    def rect(self, x, y, w, h, color, opacity=1):
        o = "" if opacity == 1 else f' fill-opacity="{opacity}"'
        self.add(f'<rect x="{_num(x)}" y="{_num(y)}" width="{_num(w)}" height="{_num(h)}" fill="{color}"{o}/>')

    def aperiodic(self, x, y, size, color):
        """The Aperiodic mark, `size` units square, its box's top left at (x, y)."""
        k = size / 100
        dots = "".join(
            f'<circle cx="{_num(x + cx * k)}" cy="{_num(y + cy * k)}" r="{_num(r * k)}"/>' for cx, cy, r in APERIODIC
        )
        self.add(f'<g fill="{color}">{dots}</g>')

    def save(self, name):
        defs = f"<defs>{''.join(self.defs)}</defs>" if self.defs else ""
        svg = (
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {_num(self.w)} {self.h}" '
            f'width="{_num(self.w)}" height="{self.h}" role="img" aria-label="{self.label}">'
            f"{defs}{''.join(self.body)}</svg>\n"
        )
        (HERE / name).write_text(svg)


# ---- the mark, the logo, the icon -------------------------------------------------------------
def mark_into(svg, p, ox=0, oy=0):
    for (col, width), color in zip(PLANKS, planks(p)):
        svg.tiles([(i, j) for i in range(col, col + width) for j in PLANK_ROWS], color, ox, oy)


def mark(p, name):
    svg = Svg(64, 64, "tparquet")
    mark_into(svg, p)
    svg.save(name)


def wordmark_geometry(f):
    """Size and baseline that run the wordmark from the top of its t to the foot of its p and q
    along the planks' height (y 5 to 60)."""
    top, foot = 704, -167  # Mona Sans Expanded Bold: the t's top, the p's descender
    size = 55 / (top - foot) * f.bold.upem
    return size, 5 + top * size / f.bold.upem


def logo(f, p, name):
    size, base = wordmark_geometry(f)
    x0, tracking = 80, -0.01
    w = x0 + f.bold.width("tparquet", size, tracking)
    svg = Svg(round(w, 1), 64, "tparquet")
    mark_into(svg, p)
    svg.text(f.bold, "tparquet", size, x0, base, p["ink"], tracking=tracking)
    svg.save(name)


def icon(name):
    svg = Svg(64, 64, "tparquet")
    svg.field(DARK)
    mark_into(svg, DARK)
    svg.save(name)


# ---- the size figure --------------------------------------------------------------------------
def totals():
    entries = json.loads(RESULTS.read_text())
    rows = sum(e["rows"] for e in entries)
    sizes = [
        ("ZSTD(9)", sum(e["zstd9_bytes"] for e in entries)),
        ("ZSTD(9) tuned", sum(e["tuned_bytes"] for e in entries)),
        ("ZSTD(9) ALP", sum(e["alp_bytes"] for e in entries)),
        ("tparquet", sum(e["tparquet_bytes"] for e in entries)),
    ]
    providers = sorted({e["provider"] for e in entries})
    return rows, len(entries), providers, sizes


def bars(svg, f, p, sizes, x0, y0, label_x, cells_per_250, bar_rows, pitch, size, grid_to):
    """Horizontal bars of tiles from zero, one per baseline and tparquet's in orange, with each
    value at its end and dotted lines at round values (Aperiodic's Scales: bars start at zero,
    show the scale, one scale for everything compared)."""
    i0 = x0 // CELL
    rows = []
    for n, (label, nbytes) in enumerate(sizes):
        mine = label == "tparquet"
        top = y0 // CELL + n * pitch
        length = round(nbytes / 1e6 / 250 * cells_per_250)
        face = f.mono_bold if mine else f.mono
        value = f"{nbytes / 1e6:.1f} MB"
        vx = (i0 + length) * CELL + size
        rows.append((label, mine, top, length, face, value, vx))
    # a value is never crossed by a scale line: the line stops a tile short of its words
    clear = [
        (vx - CELL, vx + face.width(value, size) + CELL, top, top + bar_rows) for _, _, top, _, face, value, vx in rows
    ]
    for k in range(4):  # 0, 250, 500, 750 MB
        i = i0 + k * cells_per_250
        dots = [
            (i, j)
            for j in range(y0 // CELL - 1, grid_to // CELL, 2)
            if not any(a <= i * CELL + CELL and i * CELL <= b and t <= j < u for a, b, t, u in clear)
        ]
        svg.tiles(dots, p["rule"])
        svg.text(f.mono, f"{k * 250} MB" if k else "0", size * 0.85, i * CELL + 1, grid_to + size * 1.2, p["muted"])
    for label, mine, top, length, face, value, vx in rows:
        color = p["signal"] if mine else quiet(p, 0.8)
        svg.tiles([(i, j) for i in range(i0, i0 + length) for j in range(top, top + bar_rows)], color)
        mid = (top + bar_rows / 2) * CELL + size * 0.36
        ink = p["signal_text"] if mine else p["muted"]
        svg.text(face, label.upper(), size, label_x, mid, ink, tracking=0.06)
        svg.text(face, value, size, vx, mid, p["signal_text"] if mine else p["ink"])


def figure(f, p, name):
    rows, n, providers, sizes = totals()
    ratio = sizes[3][1] / sizes[0][1]
    w, h = 800, 304
    svg = Svg(w, h, f"tparquet files are {ratio:.2f} times the size of ZSTD(9) Parquet")
    svg.field(p)
    svg.shade(p, 0, 92, 0.85, 0.6)
    svg.shade(p, 92, 108, 0.6, 0.0)
    svg.shade(p, h - 56, h, 0.85)
    svg.rect(32, 24, 12, 12, p["signal"])
    svg.text(f.mono, "SIZE ON DISK, LOWER IS BETTER", 12, 54, 35, p["muted"], tracking=0.16)
    svg.text(
        f.sans,
        f"{rows / 1e6:.1f}M rows of market data, {n} datasets from {len(providers)} providers",
        15,
        32,
        66,
        p["muted"],
    )
    svg.text(f.bold, f"{ratio:.2f}×", 40, w - 32, 56, p["signal_text"], anchor="r")
    svg.text(f.mono, "THE SIZE OF ZSTD(9)", 11, w - 32, 78, p["muted"], anchor="r", tracking=0.08)
    bars(svg, f, p, sizes, x0=176, y0=116, label_x=32, cells_per_250=32, bar_rows=5, pitch=8, size=12, grid_to=236)
    stamp = f"BENCH · {BENCH_DATE.strftime('%d %b %Y').upper()}"
    foot(svg, f, p, w, h, 32, "make bench", "reproduces it; bench/RESULTS.md has every dataset", stamp, small=True)
    svg.save(name)


def foot(svg, f, p, w, h, margin, title, note, stamp, small):
    """The line under a picture: a hairline, what it points to and a note on the left, the
    stamp on the right."""
    s = 1 if small else 1.5
    y = h - 22 * s
    svg.rect(margin, h - 44 * s, w - 2 * margin, 1, p["ink"], 0.12)
    svg.text(f.mono_bold, title, 13 * s, margin, y, p["ink"])
    svg.text(f.sans, note, 12 * s, margin + f.mono_bold.width(title, 13 * s) + 10 * s, y, p["muted"])
    svg.text(f.mono, stamp, 10 * s, w - margin, y, p["muted"], anchor="r", tracking=0.06)


# ---- the social preview -----------------------------------------------------------------------
def social(f, name):
    p = DARK
    rows, n, _, sizes = totals()
    ratio = sizes[3][1] / sizes[0][1]
    w, h = 1280, 640
    svg = Svg(w, h, "tparquet: Parquet for time series")
    svg.field(p)
    svg.shade(p, 0, 236, 0.86, 0.62)
    svg.shade(p, 236, 268, 0.62, 0.0)
    svg.shade(p, 470, 500, 0.0, 0.8)
    svg.shade(p, 500, h, 0.86)
    svg.rect(60, 48, 12, 12, p["signal"])
    svg.text(f.mono, "OPEN SOURCE · APACHE-2.0", 14, 82, 59, p["muted"], tracking=0.16)
    # the logo, larger
    k = 1.25
    size, base = wordmark_geometry(f)
    ox, oy = 60, 84
    for (col, width), color in zip(PLANKS, planks(p)):
        cells = [(i, j) for i in range(col, col + width) for j in PLANK_ROWS]
        d = "".join(
            f"M{_num(ox + (i * CELL + 1) * k)} {_num(oy + (j * CELL + 1) * k)}h{3 * k}v{3 * k}h{-3 * k}z"
            for i, j in cells
        )
        svg.add(f'<path fill="{color}" d="{d}"/>')
    svg.text(f.bold, "tparquet", size * k, ox + 80 * k, oy + base * k, p["ink"], tracking=-0.01)
    svg.text(f.sans, "Parquet for time series. Lossless, and still Parquet.", 24, 60, 214, p["muted"])
    svg.text(f.bold, f"{ratio:.2f}×", 64, w - 60, 150, p["signal_text"], anchor="r")
    svg.text(f.mono, "THE SIZE OF ZSTD(9) PARQUET", 13, w - 60, 182, p["muted"], anchor="r", tracking=0.08)
    svg.text(f.mono, f"{rows / 1e6:.1f}M ROWS · {n} DATASETS", 13, w - 60, 204, p["muted"], anchor="r", tracking=0.08)
    bars(svg, f, p, sizes, x0=268, y0=280, label_x=60, cells_per_250=46, bar_rows=6, pitch=10, size=15, grid_to=436)
    svg.text(
        f.mono, "Python · pandas · Polars · DuckDB · Go · Rust · Java · Spark · Trino · C++", 16, 60, 504, p["ink"]
    )
    stamp = f"BENCH · {BENCH_DATE.strftime('%d %b %Y').upper()}"
    foot(
        svg, f, p, w, h, 60, "github.com/aperiodic-io/tparquet", "issues and pull requests welcome", stamp, small=False
    )
    svg.save(name)


# ---- the banner under Contributing -----------------------------------------------------------
def banner(f, p, name):
    """Who builds tparquet, in one line: Aperiodic's mark on its field, what it does, and its
    address."""
    w, h, m = 800, 112, 32
    svg = Svg(w, h, "Aperiodic: microstructure, liquidity and flow metrics and indices for RWA and crypto perps")
    svg.field(p)
    svg.hshade(p, [(0, 0.9), (0.45, 0.9), (1, 0.3)])
    svg.rect(m, 28, 12, 12, p["signal"])
    svg.text(f.mono, "RWA AND CRYPTO PERPS · BINANCE · OKX · HYPERLIQUID", 11, m + 22, 39, p["muted"], tracking=0.16)
    title, sub, link = (
        "Microstructure, liquidity and flow metrics",
        "and the Cost, Depth and Impact indices, point in time, as Parquet.",
        "aperiodic.io",
    )
    svg.text(f.bold, title, 22, m, 70, p["ink"])
    svg.text(f.sans, sub, 13, m, 92, p["muted"])
    svg.aperiodic(w - m - f.mono_bold.width(link, 13) / 2 - 30, 10, 60, p["ink"])  # centred over the address
    svg.text(f.mono_bold, link, 13, w - m, 92, p["ink"], anchor="r")
    words = max(f.bold.width(title, 22), f.sans.width(sub, 13))
    assert m + words < w - m - f.mono_bold.width(link, 13) - 24, "the words run into the address"
    svg.save(name)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument(
        "--fonts", required=True, help="folder with MonaSans-Bold.ttf, MonaSans-Regular.ttf, JetBrainsMono.ttf"
    )
    f = Fonts(ap.parse_args().fonts)
    mark(LIGHT, "tparquet-mark.svg")
    mark(DARK, "tparquet-mark-dark.svg")
    logo(f, LIGHT, "tparquet-logo.svg")
    logo(f, DARK, "tparquet-logo-dark.svg")
    icon("tparquet-icon.svg")
    figure(f, LIGHT, "size.svg")
    figure(f, DARK, "size-dark.svg")
    social(f, "social-preview.svg")
    banner(f, LIGHT, "aperiodic.svg")
    banner(f, DARK, "aperiodic-dark.svg")


if __name__ == "__main__":
    main()

# tparquet brand

tparquet is built at [Aperiodic](https://aperiodic.io) and wears its look: paper and black, a
field of square tiles, and orange for the one thing worth seeing.

The mark is four planks of tiles, 5, 3, 2 and 1 tiles wide, each quieter than the last and the
narrowest in orange: Parquet columns, compressed.

| File | Use |
|---|---|
| [`tparquet-logo.svg`](tparquet-logo.svg) | mark and wordmark, on light backgrounds |
| [`tparquet-logo-dark.svg`](tparquet-logo-dark.svg) | mark and wordmark, on dark backgrounds |
| [`tparquet-mark.svg`](tparquet-mark.svg) | mark alone, on light backgrounds |
| [`tparquet-mark-dark.svg`](tparquet-mark-dark.svg) | mark alone, on dark backgrounds |
| [`tparquet-icon.svg`](tparquet-icon.svg) | app icon and favicon: the dark mark on its field of tiles |
| [`size.svg`](size.svg), [`size-dark.svg`](size-dark.svg) | the README's size figure, drawn from [`bench/results.json`](../bench/results.json) |
| [`social-preview.png`](social-preview.png) | GitHub social preview, 1280×640 (repository Settings, General, Social preview); its source is [`social-preview.svg`](social-preview.svg) |
| [`make.py`](make.py) | draws every file above |

## Colors

Aperiodic's tokens, from aperiodic.io.

| | On paper | On black |
|---|---|---|
| Page | `#F6F6F5` | `#0C0C0D` |
| Ink | `#141414` | `#EFEFED` |
| Muted (labels) | `#5E6066` | `#9A9AA1` |
| Orange (signal) | `#E8571D` | `#EF5A1F` |
| Orange text | `#AD3F0E` | `#F06A33` |
| Planks, wide to narrow | `#1F2937` `#545D6A` `#818995` `#E8571D` | `#FFDCB4` `#BDA283` `#856F59` `#EF5A1F` |
| Field: empty tile, grout | `#F6F6F5` `#EEEFF0` | `#141415` `#08080A` |

The planks sit on Aperiodic's quiet ramp, from its low stop to its high stop. The bars of a
figure are ink on the same ramp; tparquet's own bar is orange.

## Tiles

Everything drawn is tiles: 3 units square with 1 unit of grout on the top and left, on a grid of
4, as on aperiodic.io. The mark is a field of 16×16 cells.

## Type

The wordmark and headings are [Mona Sans](https://github.com/github/mona-sans) Expanded Bold;
other words are Mona Sans Expanded Regular, and labels, numbers and code
[JetBrains Mono](https://www.jetbrains.com/lp/mono/). Both are under the SIL Open Font License.
Every word is converted to outlines, so the files render the same without the fonts installed.

## Rules

- Keep the planks' widths, order and colors. Orange is for the narrowest plank, and in a figure
  for one thing only.
- Don't recolor the wordmark beyond ink and paper.
- Square corners, flat colors: no rounded tiles, shadows or gradients on the marks.

## Redrawing

```sh
pip install fonttools uharfbuzz
python3 brand/make.py --fonts DIR    # DIR: MonaSans-Bold.ttf, MonaSans-Regular.ttf, JetBrainsMono.ttf
rsvg-convert brand/social-preview.svg -o brand/social-preview.png
```

Run it after `make bench` so the size figure and the social preview show the new results.

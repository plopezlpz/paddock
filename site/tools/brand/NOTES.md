# paddock brand: final geometry

Files live in `assets/brand/`.
Regenerate the PNGs: `node tools/brand/rasterize.mjs --png`. Contact sheet: `node tools/brand/rasterize.mjs --sheet`
(writes `tools/.out/brand-sheet@{1,2}x.png`; works from any directory).

## Colours

| Token | Light | Dark |
|---|---|---|
| ink (frame, wordmark) | `#141714` | `#e4e4e4` |
| dot (olive, the site accent) | `#a2a970` | `#a2a970` |
| paper (PNG backgrounds) | `#f5f5f0` | |

`mark.svg` and `logo.svg` use `currentColor` for ink and `style="fill:var(--green,#a2a970)"` for the dot, so an inlined copy
follows the site tokens. As a plain `<img>` they fall back to black ink and the light green; use `logo-light.svg` /
`logo-dark.svg` for `<img>` or `<picture>`.

## Mark (`viewBox="0 0 32 32"`)

The "gate": paddock's rounded popup frame with the **top-left corner open**, and one agent dot.

- Design note: a gap in the middle of the top edge (x 10-15) read as a power button, and at 16 px the notch
  nearly closed. The opening is now the whole top-left corner, which stays legible at 16 px.
- Frame, one open path, `stroke-width="3.4"` (matches the wordmark's 3.6 once the lockup scales the mark by 34/32),
  `stroke-linejoin="round"`, butt caps, corner radius 5 on the centreline:
  `M4.5 15.5 V22.5 A5 5 0 0 0 9.5 27.5 H22.5 A5 5 0 0 0 27.5 22.5 V9.5 A5 5 0 0 0 22.5 4.5 H14.5`
  The left post ends at y=15.5, the top rail starts at x=14.5.
- Dot: `cx=16 cy=16 r=4.5`, the geometric centre (no downward offset is needed with an open corner).
- `favicon.svg`: heavier and snapped to the 16 px pixel grid (edges at whole pixels at 16 and 32 px):
  stroke 4, centreline inset 4 (`M4 16 V23 A5 5 0 0 0 9 28 H23 A5 5 0 0 0 28 23 V9 A5 5 0 0 0 23 4 H14`), dot `r=5` at 16,16.
  Its `<style>` switches frame and dot colours with `prefers-color-scheme: dark`. The selectors are bare `path`/`circle`,
  so use it only as a file (`<link rel="icon">`), never inline.

## Wordmark ("paddock", no font)

Monoline, stroke 3.6, butt caps. Baseline y=24, x-height top y=10 (outer), ascender top y=3, descender bottom y=31.
Bowls are circles, centre y=17, centreline radius 5.2 (outer 7, so every glyph box is 14 wide).

| Glyph | Construction (glyph-local x) | Box |
|---|---|---|
| `p` | stem x=1.8, y10-31; bowl centre (7,17) | 14 |
| `a` | bowl (7,17); stem x=12.2, y10-24 | 14 |
| `d` | bowl (7,17); stem x=12.2, y3-24 | 14 |
| `o` | bowl (7,17) | 14 |
| `c` | arc r=5.2 from 40 to 320 degrees, opening right; ends at (10.98, 13.66) and (10.98, 20.34) | about 12 |
| `k` | stem x=1.8, y3-24 (stroked); arm and leg are **filled polygons** with horizontal cuts at y=10 and y=24 | 13.6 |

`k`: arm centreline from (1.8, 20.4) to (10.8, 10); leg starts at 55% along the arm and ends at (10.8, 24). Stroked
diagonals gave slanted terminals that stuck out of the x-height and baseline, hence the polygons (second `<path>`, `fill`).

Spacing between glyph boxes: round-round 2.6, stem-round 3.4, `c` to `k` 3.6. Total width about 114.

## Lockup (`viewBox="0 0 158.06 34"`)

Mark scaled 34/32 (`scale(1.0625)`) at x=0; wordmark translated to x=44 (10 unit gap), y=0. The mark's outer frame spans
y 3 to 31, equal to the wordmark's ascender-to-descender, and the dot centre (y=17) sits on the bowl centreline.

## Reading at small sizes (checked on the contact sheet at DPR 1)

- 16 px: the open corner and the dot are both distinct, on paper and on dark. `favicon.svg` is crisper than `mark.svg`
  (pixel-snapped strokes); use `favicon.svg` below 24 px.
- Monochrome (dot in ink) holds up from 16 px.
- `favicon-32.png`: favicon geometry on a paper rounded square (radius 7), so it shows on dark tab bars.
- `apple-touch-icon.png`: 180x180, paper background, `mark.svg` at 126 px (70%), ink `#141714`, dot `#a2a970`.

// Cell widths and the glyph repertoire of the recorder.
//
// width() follows tmux: East Asian Wide/Fullwidth and emoji presentation are
// 2 columns, combining marks and joiners 0, everything else (ambiguous
// included) 1.
//
// The recorder is stricter than the converter: a recording may only contain
// printable ASCII and the single-width glyphs listed in GLYPHS, each added
// deliberately after it was seen in a real capture. So every cell of a frame
// is one column, and the player and the SVG builder can give each glyph
// exactly one cell.

const WIDE = [
  [0x1100, 0x115f], [0x231a, 0x231b], [0x2329, 0x232a], [0x23e9, 0x23ec], [0x23f0, 0x23f0],
  [0x23f3, 0x23f3], [0x25fd, 0x25fe], [0x2614, 0x2615], [0x2648, 0x2653], [0x267f, 0x267f],
  [0x2693, 0x2693], [0x26a1, 0x26a1], [0x26aa, 0x26ab], [0x26bd, 0x26be], [0x26c4, 0x26c5],
  [0x26ce, 0x26ce], [0x26d4, 0x26d4], [0x26ea, 0x26ea], [0x26f2, 0x26f3], [0x26f5, 0x26f5],
  [0x26fa, 0x26fa], [0x26fd, 0x26fd], [0x2705, 0x2705], [0x270a, 0x270b], [0x2728, 0x2728],
  [0x274c, 0x274c], [0x274e, 0x274e], [0x2753, 0x2755], [0x2757, 0x2757], [0x2795, 0x2797],
  [0x27b0, 0x27b0], [0x27bf, 0x27bf], [0x2b1b, 0x2b1c], [0x2b50, 0x2b50], [0x2b55, 0x2b55],
  [0x2e80, 0x303e], [0x3041, 0x33ff], [0x3400, 0x4dbf], [0x4e00, 0xa4cf], [0xa960, 0xa97f],
  [0xac00, 0xd7a3], [0xf900, 0xfaff], [0xfe30, 0xfe6f], [0xff00, 0xff60], [0xffe0, 0xffe6],
  [0x1f300, 0x1f64f], [0x1f680, 0x1f6ff], [0x1f900, 0x1f9ff], [0x1fa70, 0x1faff],
  [0x20000, 0x3fffd],
];

const ZERO = /^[\p{Mn}\p{Me}\p{Cf}]$/u;

/** Display columns of one character (a string holding a single code point). */
export function width(ch) {
  const cp = ch.codePointAt(0);
  if (cp === 0) return 0;
  if (cp < 0x20 || (cp >= 0x7f && cp < 0xa0)) return 0;
  if (cp < 0x300) return 1;
  if (ZERO.test(ch)) return 0;
  for (const [lo, hi] of WIDE) if (cp >= lo && cp <= hi) return 2;
  return 1;
}

/** Display columns of a string. */
export function strWidth(s) {
  let w = 0;
  for (const ch of s) w += width(ch);
  return w;
}

// Every non-ASCII glyph a recording may contain. Add one only after seeing it
// in a capture and checking it is one column wide in tmux.
export const GLYPHS = new Set(
  [
    '─│├└┌┐┘╭╮╰╯┬┴┼', // box drawing: the popup frame and the tree
    '●○', // agent state dots
    '▌▏', // selection mark, filter cursor
    '›‹', // prompt, back
    '⎿', // tool output
    '✢✳✶✻✽✓✗', // agent progress (claude's spinner), done, failed
    '⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏', // pi's spinner
    '⏎⇥', // key hints
    '▸▾', // window toggle
    '·…', // separators, ellipsis
    '↑↓←→', // arrows
    '⋯⌫', // folded hints, backspace
    '✎✦⇅', // renaming, suggesting, moving
  ].join('').split(''),
);

/** Throws on any character outside printable ASCII and GLYPHS. */
export function checkRepertoire(text, where = 'text') {
  let line = 1;
  let col = 0;
  for (const ch of text) {
    if (ch === '\n') { line++; col = 0; continue; }
    col++;
    const cp = ch.codePointAt(0);
    if (cp === 0x1b || ch === '\r') continue; // escape sequences are parsed elsewhere
    if (cp >= 0x20 && cp < 0x7f) continue;
    if (GLYPHS.has(ch)) continue;
    const w = width(ch);
    const why = w === 2 ? 'a wide character' : w === 0 ? 'a combining/zero-width character' : 'not in the glyph repertoire (tools/lib/wcwidth.mjs)';
    throw new Error(`${where}: U+${cp.toString(16).toUpperCase().padStart(4, '0')} ${JSON.stringify(ch)} at line ${line}, col ${col} is ${why}`);
  }
}

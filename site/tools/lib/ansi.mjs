// SGR text (`tmux capture-pane -e -p -N`) to a grid of cells and back.
//
// A cell is { ch, w, st }: the character, its width (0 for the second half of
// a wide character) and an interned style. A style holds only what is set:
//   fg, bg  0-15 palette index | "#rrggbb" | "fg" / "bg" (the terminal's
//           default colours, which only a resolved reverse produces)
//   b d i u s  bold, dim, italic, underline, strike (1 when on)
// Reverse video is resolved here: fg and bg swap, a default fg becomes "bg"
// and a default bg becomes "fg". SGR state carries across lines, as it does
// in the stream tmux writes.

import { width } from './wcwidth.mjs';

const ESC = '\x1b';
const FLAGS = ['b', 'd', 'i', 'u', 's'];

const interned = new Map();
function intern(st) {
  const clean = {};
  if (st.fg !== undefined) clean.fg = st.fg;
  if (st.bg !== undefined) clean.bg = st.bg;
  for (const f of FLAGS) if (st[f]) clean[f] = 1;
  const key = JSON.stringify(clean);
  let hit = interned.get(key);
  if (!hit) interned.set(key, (hit = Object.freeze(clean)));
  return hit;
}
export const DEFAULT = intern({});

/** xterm 256-colour index to palette index (0-15) or "#rrggbb". */
export function color256(n) {
  if (n < 16) return n;
  const hex = (r, g, b) => '#' + [r, g, b].map((v) => v.toString(16).padStart(2, '0')).join('');
  if (n < 232) {
    const c = n - 16;
    const lv = [0, 95, 135, 175, 215, 255];
    return hex(lv[Math.floor(c / 36)], lv[Math.floor(c / 6) % 6], lv[c % 6]);
  }
  const g = 8 + (n - 232) * 10;
  return hex(g, g, g);
}

const rgb = (r, g, b) => '#' + [r, g, b].map((v) => Math.max(0, Math.min(255, v | 0)).toString(16).padStart(2, '0')).join('');

/** Applies one SGR parameter list (the text between `ESC [` and `m`) to STATE. */
function applySgr(state, body) {
  // Each parameter is a list of its ':' subparameters.
  const params = body === '' ? [['0']] : body.split(';').map((p) => p.split(':'));
  for (let i = 0; i < params.length; i++) {
    const p = params[i];
    const code = p[0] === '' ? 0 : Number(p[0]);
    if (code === 38 || code === 48 || code === 58) {
      let value;
      if (p.length > 1) {
        // 38:5:n, 38:2:r:g:b or 38:2:colourspace:r:g:b
        const sub = p.slice(1).map((v) => (v === '' ? 0 : Number(v)));
        if (sub[0] === 5) value = color256(sub[1] ?? 0);
        else if (sub[0] === 2) {
          const c = sub.length >= 5 ? sub.slice(2, 5) : sub.slice(1, 4);
          value = rgb(...c);
        }
      } else {
        const mode = Number(params[i + 1]?.[0]);
        if (mode === 5) { value = color256(Number(params[i + 2]?.[0] ?? 0)); i += 2; }
        else if (mode === 2) { value = rgb(...[2, 3, 4].map((k) => Number(params[i + k]?.[0] ?? 0))); i += 4; }
        else i += 1;
      }
      if (value !== undefined) {
        if (code === 38) state.fg = value;
        else if (code === 48) state.bg = value;
      }
      continue;
    }
    if (code === 0) { state.fg = state.bg = undefined; state.b = state.d = state.i = state.u = state.s = state.r = 0; }
    else if (code === 1) state.b = 1;
    else if (code === 2) state.d = 1;
    else if (code === 3) state.i = 1;
    else if (code === 4) state.u = p[1] === '0' ? 0 : 1;
    else if (code === 7) state.r = 1;
    else if (code === 9) state.s = 1;
    else if (code === 21) state.u = 1;
    else if (code === 22) state.b = state.d = 0;
    else if (code === 23) state.i = 0;
    else if (code === 24) state.u = 0;
    else if (code === 27) state.r = 0;
    else if (code === 29) state.s = 0;
    else if (code >= 30 && code <= 37) state.fg = code - 30;
    else if (code === 39) state.fg = undefined;
    else if (code >= 40 && code <= 47) state.bg = code - 40;
    else if (code === 49) state.bg = undefined;
    else if (code >= 90 && code <= 97) state.fg = code - 90 + 8;
    else if (code >= 100 && code <= 107) state.bg = code - 100 + 8;
    // blink, hidden, overline, underline colour: no visible effect here
  }
}

/** The interned style of STATE with reverse video resolved. */
function resolve(state) {
  if (!state.r) return intern(state);
  // The (default) background is drawn as the text, the (default) text as the background.
  return intern({ ...state, fg: state.bg === undefined ? 'bg' : state.bg, bg: state.fg === undefined ? 'fg' : state.fg });
}

const blank = () => ({ ch: ' ', w: 1, st: DEFAULT });

/**
 * Parses TEXT into ROWS rows of COLS cells. Escape sequences other than SGR
 * are dropped. A wide character takes two cells (the second has w 0 and ch '').
 * With opts.strict a row longer than COLS or more rows than ROWS throws
 * (tmux leaves rows it never wrote empty, so a short row is not an error).
 */
export function parse(text, cols, rows, opts = {}) {
  const state = { fg: undefined, bg: undefined, b: 0, d: 0, i: 0, u: 0, s: 0, r: 0 };
  const out = [];
  let lines = text.split('\n');
  if (lines.length > 1 && lines[lines.length - 1] === '') lines.pop();
  for (let ln = 0; ln < lines.length; ln++) {
    const s = lines[ln].replace(/\r$/, '');
    const row = [];
    for (let i = 0; i < s.length; ) {
      if (s[i] === ESC) {
        const next = s[i + 1];
        if (next === '[') {
          let j = i + 2;
          while (j < s.length && !/[\x40-\x7e]/.test(s[j])) j++;
          if (s[j] === 'm') applySgr(state, s.slice(i + 2, j));
          i = j + 1;
        } else if (next === ']') {
          let j = i + 2;
          while (j < s.length && s[j] !== '\x07' && !(s[j] === ESC && s[j + 1] === '\\')) j++;
          i = s[j] === '\x07' ? j + 1 : j + 2;
        } else {
          // ESC, intermediate bytes (0x20-0x2f), one final byte: charset selection and the like
          i++;
          while (i < s.length && s.charCodeAt(i) >= 0x20 && s.charCodeAt(i) <= 0x2f) i++;
          i++;
        }
        continue;
      }
      const ch = String.fromCodePoint(s.codePointAt(i));
      i += ch.length;
      const w = width(ch);
      if (w === 0) continue; // combining marks and joiners have no cell of their own
      const st = resolve(state);
      row.push({ ch, w, st });
      if (w === 2) row.push({ ch: '', w: 0, st });
    }
    if (opts.strict && row.length > cols) throw new Error(`row ${ln} has ${row.length} cells, more than ${cols}`);
    while (row.length < cols) row.push(blank());
    out.push(row.slice(0, cols));
  }
  if (opts.strict && out.length > rows) throw new Error(`${out.length} rows, more than ${rows}`);
  while (out.length < rows) out.push(Array.from({ length: cols }, blank));
  return out.slice(0, rows);
}

/** The text of a row of cells. */
export const rowText = (row) => row.map((c) => c.ch).join('');

/** The SGR parameters that give STYLE on a reset terminal, or '' for the default. */
function sgrFor(st) {
  const codes = [];
  let { fg, bg } = st;
  let reverse = false;
  // "fg"/"bg" only come from a resolved reverse: undo it.
  if (fg === 'bg' || bg === 'fg') {
    reverse = true;
    const origFg = bg === 'fg' ? undefined : bg;
    const origBg = fg === 'bg' ? undefined : fg;
    fg = origFg;
    bg = origBg;
  }
  const colour = (v, base, bright) => {
    if (v === undefined) return;
    if (typeof v === 'number') codes.push(v < 8 ? base + v : bright + v - 8);
    else codes.push(`${base + 8};2;${parseInt(v.slice(1, 3), 16)};${parseInt(v.slice(3, 5), 16)};${parseInt(v.slice(5, 7), 16)}`);
  };
  if (st.b) codes.push(1);
  if (st.d) codes.push(2);
  if (st.i) codes.push(3);
  if (st.u) codes.push(4);
  if (st.s) codes.push(9);
  if (reverse) codes.push(7);
  colour(fg, 30, 90);
  colour(bg, 40, 100);
  return codes.join(';');
}

/** Cells back to SGR text, one line per row (for the round-trip test). */
export function emit(grid) {
  return grid
    .map((row) => {
      let out = '';
      let prev = DEFAULT;
      for (const c of row) {
        if (c.w === 0) continue;
        if (c.st !== prev) {
          const sgr = sgrFor(c.st);
          out += sgr ? `${ESC}[0;${sgr}m` : `${ESC}[0m`;
          prev = c.st;
        }
        out += c.ch;
      }
      return prev === DEFAULT ? out : out + `${ESC}[0m`;
    })
    .join('\n');
}

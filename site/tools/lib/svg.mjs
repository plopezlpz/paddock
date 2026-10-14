// Frame data to animated / static SVG.
//
// One SVG is a "film strip": every frame is a slot of the same height inside one group, a step-end
// CSS animation moves the strip up by one slot per frame, and a clip path shows one slot.
// Each unique screen line is defined once in <defs> and referenced with <use>.
// The grid holds in any font: ASCII runs carry textLength, every other glyph owns its cell, and the
// glyphs the UI draws structure with (box drawing, dots, the selection bar) are vector shapes.

import fs from 'node:fs';
import vm from 'node:vm';

// ---- geometry (user units) ----
export const CW = 8.4;
export const CH = 17;
const FS = 14;
const BASE = 13; // text baseline inside a 17 unit row
const BAR = 28; // window title bar
const PADX = 16;
const PADY = 14;
const STRIP = 36; // caption strip under the terminal
const MONO = 'ui-monospace,SFMono-Regular,"SF Mono",Menlo,Consolas,"DejaVu Sans Mono","Liberation Mono",monospace';
const SANS = 'system-ui,-apple-system,"Segoe UI",Roboto,"Helvetica Neue",Arial,sans-serif';

// ---- input ----
export function loadFrames(file) {
  const win = {};
  vm.runInNewContext(fs.readFileSync(file, 'utf8'), { window: win });
  const names = Object.keys(win.PADDOCK_FRAMES);
  return win.PADDOCK_FRAMES[names[0]];
}

/**
 * The terminal palette straight from the site CSS, so the SVGs and the site player cannot drift apart.
 * The terminal is always dark, whatever the page theme, so there is one palette: a README image on a
 * light page keeps its dark terminal. Files are read in order and later values win, so a theme file
 * (assets/css/site.css, its `.page` block) can restyle the terminal that player.css defines.
 */
export function loadPalette(...cssFiles) {
  const vars = {};
  for (const file of cssFiles) {
    const css = fs.readFileSync(file, 'utf8');
    for (const re of [/:root\s*\{([^}]*)\}/, /html\[data-theme="dark"\]\s*\{([^}]*)\}/, /\.page\s*\{([^}]*)\}/]) {
      const m = re.exec(css);
      if (m) for (const v of m[1].matchAll(/--([\w-]+)\s*:\s*([^;]+);/g)) vars[v[1]] = v[2].trim();
    }
  }
  return {
    bg: vars['t-bg'],
    fg: vars['t-fg'],
    ansi: Array.from({ length: 16 }, (_, i) => vars[`t-${i}`]),
    dim: parseFloat(vars['t-dim']) / 100,
    accent: vars['t-accent'] ?? vars['pd-green'],
  };
}

// ---- colour ----
const hex2rgb = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
const rgb2hex = (c) => '#' + c.map((v) => Math.round(v).toString(16).padStart(2, '0')).join('');
/** srgb mix, the same arithmetic as color-mix(in srgb, a p%, b). */
export function mix(a, b, p) {
  const [x, y] = [hex2rgb(a), hex2rgb(b)];
  return rgb2hex(x.map((v, i) => v * p + y[i] * (1 - p)));
}

/**
 * Colours become classes (.cN{color:#...}) so that one file can carry several palettes.
 * Palette 0 is the default; the others sit in a prefers-color-scheme media query.
 */
function makePaints(palettes) {
  const byKey = new Map();
  return {
    cls(fn) {
      const hexes = palettes.map((p) => fn(p.pal));
      const key = hexes.join('|');
      if (!byKey.has(key)) byKey.set(key, { id: 'c' + byKey.size, hexes });
      return byKey.get(key).id;
    },
    css() {
      const all = [...byKey.values()];
      let out = all.map((c) => `.${c.id}{color:${c.hexes[0]}}`).join('');
      palettes.forEach((p, i) => {
        if (i === 0 || !p.media) return;
        out += `@media ${p.media}{${all.filter((c) => c.hexes[i] !== c.hexes[0]).map((c) => `.${c.id}{color:${c.hexes[i]}}`).join('')}}`;
      });
      return out;
    },
  };
}

const colour = (v, pal, fallback) => {
  if (typeof v === 'number') return pal.ansi[v];
  if (v === 'fg') return pal.fg;
  if (v === 'bg') return pal.bg;
  if (typeof v === 'string') return v;
  return fallback;
};

// ---- shapes: glyphs drawn as vectors so they join up and do not depend on the font ----
const R = CW / 2; // corner radius of the rounded box corners
const BOX = {
  // [left, right, up, down] arms; rounded corners are handled apart
  '─': [1, 1, 0, 0], '│': [0, 0, 1, 1], '├': [0, 1, 1, 1], '┤': [1, 0, 1, 1], '┬': [1, 1, 0, 1],
  '┴': [1, 1, 1, 0], '┼': [1, 1, 1, 1], '┌': [0, 1, 0, 1], '┐': [1, 0, 0, 1], '└': [0, 1, 1, 0], '┘': [1, 0, 1, 0],
};
const ROUND = { '╭': 'dr', '╮': 'dl', '╰': 'ur', '╯': 'ul' };
const e = 0.3; // arms overlap a little so scaled renders show no seams
const n2 = (v) => +v.toFixed(2);

function boxPath(ch, x0) {
  const x1 = x0 + CW, cx = x0 + CW / 2, cy = CH / 2;
  if (ROUND[ch]) {
    const [v, h] = ROUND[ch];
    const vy = v === 'd' ? CH + e : -e; // far end of the vertical arm
    const hx = h === 'r' ? x1 + e : x0 - e;
    const ys = v === 'd' ? cy + R : cy - R; // where the vertical arm meets the arc
    const xs = h === 'r' ? cx + R : cx - R;
    // sweep: down-right and up-left are clockwise as drawn, the other two counter-clockwise
    const sweep = (v === 'd') === (h === 'r') ? 1 : 0;
    return `M${n2(cx)} ${n2(vy)}V${n2(ys)}A${n2(R)} ${n2(R)} 0 0 ${sweep} ${n2(xs)} ${n2(cy)}H${n2(hx)}`;
  }
  const [l, r, u, d] = BOX[ch];
  const X0 = x0 - e, X1 = x1 + e, Y0 = -e, Y1 = CH + e;
  const h = (l ? 1 : 0) + (r ? 1 : 0), v = (u ? 1 : 0) + (d ? 1 : 0);
  if (h === 2 && v === 0) return `M${n2(X0)} ${cy}H${n2(X1)}`;
  if (v === 2 && h === 0) return `M${n2(cx)} ${Y0}V${Y1}`;
  let p = '';
  if (h === 2) p += `M${n2(X0)} ${cy}H${n2(X1)}`;
  else if (h === 1) p += `M${n2(l ? X0 : X1)} ${cy}H${n2(cx)}`;
  if (v === 2) p += `M${n2(cx)} ${Y0}V${Y1}`;
  else if (v === 1) p += `M${n2(cx)} ${u ? Y0 : Y1}V${cy}`;
  return p;
}

const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const isAscii = (c) => c >= ' ' && c <= '~';

/** One run: bg rect, underline, text segments, glyphs. Returns {bg, fg} element strings. */
function drawRun({ text, col, st, paints, shapes }) {
  const chars = Array.from(text);
  const x0 = col * CW, w = chars.length * CW;
  const bgOf = (pal) => colour(st.bg, pal, null);
  const fgOf = (pal) => {
    const c = colour(st.fg, pal, pal.fg);
    return st.d ? mix(c, bgOf(pal) ?? pal.bg, pal.dim) : c;
  };
  const hasBg = st.bg !== undefined;
  let bg = '';
  if (hasBg) bg = `<rect class="${paints.cls(bgOf)}" x="${n2(x0)}" y="0" width="${n2(w)}" height="${CH + 0.4}"/>`;
  let body = '';
  // ASCII segments: words separated by single spaces keep textLength; wider gaps split the segment
  let k = 0;
  while (k < chars.length) {
    if (!isAscii(chars[k])) {
      body += glyph(chars[k], col + k, st, shapes);
      k++;
      continue;
    }
    let j = k;
    while (j < chars.length && isAscii(chars[j])) j++;
    const chunk = chars.slice(k, j).join('');
    for (const m of chunk.matchAll(/[^ ]+(?: [^ ]+)*/g)) {
      const x = (col + k + m.index) * CW;
      body += `<text x="${n2(x)}" y="${BASE}" textLength="${n2(m[0].length * CW)}" lengthAdjust="spacingAndGlyphs">${esc(m[0])}</text>`;
    }
    k = j;
  }
  if (st.u) body += `<rect x="${n2(x0)}" y="${CH - 2}" width="${n2(w)}" height="1"/>`;
  if (st.s) body += `<rect x="${n2(x0)}" y="${CH / 2}" width="${n2(w)}" height="1"/>`;
  const attrs = (st.b ? ' font-weight="700"' : '') + (st.i ? ' font-style="italic"' : '');
  const fg = body ? `<g class="${paints.cls(fgOf)}"${attrs}>${body}</g>` : '';
  return { bg, fg };
}

function glyph(ch, col, st, shapes) {
  const x0 = col * CW, cx = x0 + CW / 2;
  if (shapes) {
    if (BOX[ch] || ROUND[ch]) return `<path fill="none" stroke="currentColor" d="${boxPath(ch, x0)}"/>`;
    if (ch === '●') return `<circle cx="${n2(cx)}" cy="8.3" r="3.6"/>`;
    if (ch === '○') return `<circle fill="none" stroke="currentColor" cx="${n2(cx)}" cy="8.3" r="3.1" stroke-width="1.2"/>`;
    if (ch === '▌') return `<rect x="${n2(x0)}" y="0" width="${n2(CW / 2)}" height="${CH + 0.4}"/>`;
    if (ch === '▏') return `<rect x="${n2(x0)}" y="0" width="${n2(CW / 8)}" height="${CH + 0.4}"/>`;
  }
  return `<text x="${n2(cx)}" y="${BASE}" text-anchor="middle">${esc(ch)}</text>`;
}

function drawLine(runs, ctx) {
  let col = 0, bgs = '', fgs = '';
  for (const [si, text] of runs) {
    const n = Array.from(text).length;
    if (text.trim() === '' && ctx.styles[si].bg === undefined && !ctx.styles[si].u && !ctx.styles[si].s) {
      col += n;
      continue;
    }
    const r = drawRun({ text, col, st: ctx.styles[si], paints: ctx.paints, shapes: ctx.shapes });
    bgs += r.bg;
    fgs += r.fg;
    col += n;
  }
  return bgs + fgs;
}

// ---- pacing ----
/**
 * Presentation holds for the README: the recording is brisk, so captioned and keyed frames get more time,
 * quick typing frames keep theirs and the last frame rests before the loop.
 * pace = target loop length in ms, or null to keep the recorded holds.
 */
export function paceHolds(frames, { pace = 30000, endHold = 2500, typingMax = 300, boost = 1.25 } = {}) {
  if (!pace) return frames.map((f) => f.d);
  const last = frames.length - 1;
  const fixed = frames.map((f, i) => (i === last ? Math.max(endHold, f.d) : f.d <= typingMax ? f.d : null));
  const weight = frames.map((f) => (f.cap || f.key ? f.d * boost : f.d));
  const fixedSum = fixed.reduce((a, v) => a + (v ?? 0), 0);
  const free = weight.reduce((a, v, i) => a + (fixed[i] === null ? v : 0), 0);
  const scale = Math.max(0, pace - fixedSum) / free;
  return frames.map((f, i) => fixed[i] ?? Math.round((weight[i] * scale) / 10) * 10);
}

const pct = (v) => String(+v.toFixed(4));
/** Keyframe stops: frame i is shown from t = start[i] ms. */
export function keyframeStops(holds) {
  const total = holds.reduce((a, b) => a + b, 0);
  let t = 0;
  const stops = holds.map((h, i) => {
    const s = { i, t, pct: +((t / total) * 100).toFixed(4) };
    t += h;
    return s;
  });
  return { stops, total };
}

// ---- caption strip ----
function keyParts(label) {
  const m = /^prefix\s+(.+)$/.exec(label);
  if (m) return { caps: ['ctrl-b', ...m[1].split(/\s+/)] };
  if (Array.from(label).length <= 10) return { caps: [label] };
  return { hint: label };
}

function captionSlot({ cap, key, y0, innerW, paints }) {
  const dim = paints.cls((p) => mix(p.fg, p.bg, 0.84));
  const accent = paints.cls((p) => p.accent);
  const mid = y0 + STRIP / 2;
  let out = '';
  if (cap) {
    out += `<g class="${accent}"><circle cx="6" cy="${mid}" r="3.2"/></g>`;
    out += `<text class="${dim}" x="20" y="${mid + 4.8}" font-family='${SANS}' font-size="14">${esc(cap)}</text>`;
  }
  if (key) {
    out += '<g class="key">';
    const { caps, hint } = keyParts(key);
    if (hint) {
      const w = hint.length * 7.2;
      out += `<text class="${paints.cls((p) => mix(p.fg, p.bg, 0.62))}" x="${n2(innerW - w)}" y="${mid + 4.4}" textLength="${n2(w)}" font-size="12" font-style="italic">${esc(hint)}</text>`;
    } else {
      let x = innerW;
      const boxes = [];
      for (const c of [...caps].reverse()) {
        const n = Array.from(c).length;
        const w = n * 7.2 + 14;
        x -= w;
        boxes.unshift({ c, x, w, n });
        x -= 6;
      }
      const face = paints.cls((p) => mix(p.fg, p.bg, 0.12));
      const edge = paints.cls((p) => mix(p.fg, p.bg, 0.3));
      const ink = paints.cls((p) => p.fg);
      for (const b of boxes) {
        out += `<g class="${edge}"><rect x="${n2(b.x)}" y="${mid - 11}" width="${n2(b.w)}" height="23" rx="5"/></g>`;
        out += `<g class="${face}"><rect x="${n2(b.x)}" y="${mid - 11.5}" width="${n2(b.w)}" height="21.5" rx="5"/></g>`;
        out += `<text class="${ink}" x="${n2(b.x + 7)}" y="${mid + 4.2}" textLength="${n2(b.n * 7.2)}" lengthAdjust="spacingAndGlyphs" font-size="12" font-weight="700">${esc(b.c)}</text>`;
      }
    }
    out += '</g>';
  }
  return out;
}

// ---- document ----
/**
 * @param data      frame set (format v1)
 * @param opts.frames   frame indices shown (strip order)
 * @param opts.holds    ms per shown frame; omit for a static image
 * @param opts.palettes [{pal, media?}], the first is the default
 * @param opts.chrome   'window' (title bar + caption strip) | 'phone' (bezel)
 * @param opts.reduceTo index into opts.frames shown under prefers-reduced-motion
 * @param opts.shapes   draw box drawing, dots and bars as vectors (default true)
 */
export function buildSvg(data, opts) {
  const { frames: idxs, holds, palettes, chrome = 'window', title, desc, shapes = true } = opts;
  const animate = Boolean(holds) && idxs.length > 1;
  const paints = makePaints(palettes);
  const ctx = { styles: data.styles, paints, palettes, shapes };
  const isPhone = chrome === 'phone';

  // geometry
  const termW = data.cols * CW, termH = data.rows * CH;
  const pad = isPhone ? { x: 15, y: 30 } : { x: PADX, y: BAR + PADY };
  const slotH = isPhone ? termH : termH + PADY + STRIP;
  const W = isPhone ? termW + 30 : termW + 2 * PADX;
  const H = isPhone ? termH + 54 : BAR + PADY + slotH;
  const innerW = W - 2 * PADX; // caption strip content width

  // lines
  const used = new Set();
  idxs.forEach((fi) => data.frames[fi].l.forEach((li) => used.add(li)));
  const bodies = new Map([...used].sort((a, b) => a - b).map((li) => [li, drawLine(data.lines[li], ctx)]));
  const defs = [...bodies].filter(([, b]) => b).map(([li, b]) => `<g id="L${li}">${b}</g>`).join('');

  // strip slots
  let slots = '';
  idxs.forEach((fi, k) => {
    const f = data.frames[fi];
    const chap = data.chapters?.[f.ch];
    let s = '';
    f.l.forEach((li, r) => {
      if (bodies.get(li)) s += `<use href="#L${li}" y="${r * CH}"/>`;
    });
    if (f.tap && animate) { // static images show the screen as it is, so no marker there
      const [c, r] = f.tap;
      const ring = paints.cls((p) => p.accent);
      s += `<g class="${ring}"><circle cx="${n2((c + 0.5) * CW)}" cy="${n2((r + 0.5) * CH)}" r="13" fill="none" stroke="currentColor" stroke-width="1.6" opacity=".6"/></g>`;
    }
    if (!isPhone) s += captionSlot({ cap: f.cap || chap?.cap, key: animate ? f.key : null, y0: termH + PADY, innerW, paints });
    slots += `<g transform="translate(0 ${k * slotH})">${s}</g>`;
  });

  // chrome
  const frameFill = paints.cls((p) => p.bg);
  let back, front = '';
  if (isPhone) {
    const bez = '#1a1d1c';
    back = `<rect x="0" y="0" width="${W}" height="${H}" rx="38" fill="${bez}"/>`
      + `<rect x=".5" y=".5" width="${W - 1}" height="${H - 1}" rx="37.5" fill="none" stroke="#3a403d"/>`
      + `<rect x="${W / 2 - 27}" y="9" width="54" height="6" rx="3" fill="#0a0c0b"/>`
      + `<g class="${frameFill}"><rect x="9" y="24" width="${n2(termW + 12)}" height="${n2(termH + 12)}" rx="22"/></g>`;
  } else {
    const line = paints.cls((p) => mix(p.fg, p.bg, 0.18));
    const bar = paints.cls((p) => mix(p.fg, p.bg, 0.06));
    const dot = paints.cls((p) => mix(p.fg, p.bg, 0.22));
    const ttl = paints.cls((p) => mix(p.fg, p.bg, 0.7));
    const capBg = paints.cls((p) => mix(p.fg, p.bg, 0.035));
    back = `<g class="${frameFill}"><rect width="${n2(W)}" height="${H}"/></g>`
      + `<g class="${bar}"><rect width="${n2(W)}" height="${BAR}"/></g>`
      + `<g class="${line}"><rect y="${BAR - 1}" width="${n2(W)}" height="1"/></g>`
      + `<g class="${capBg}"><rect y="${BAR + PADY + termH}" width="${n2(W)}" height="${STRIP + PADY}"/></g>`
      + `<g class="${line}"><rect y="${BAR + PADY + termH}" width="${n2(W)}" height="1"/></g>`
      + [0, 1, 2].map((i) => `<g class="${dot}"><circle cx="${18 + i * 16}" cy="${BAR / 2}" r="5"/></g>`).join('')
      + `<text class="${ttl}" x="${n2(W / 2)}" y="${BAR / 2 + 4.2}" text-anchor="middle" font-family='${SANS}' font-size="12">tmux</text>`;
    front = `<rect x=".5" y=".5" width="${n2(W - 1)}" height="${H - 1}" rx="8" fill="none" stroke="currentColor"/>`;
    front = `<g class="${line}">${front}</g>`;
  }
  const fgBase = paints.cls((p) => p.fg);

  // animation
  let css = '';
  if (animate) {
    const { stops, total } = keyframeStops(holds);
    const frames = stops.map((s) => `${pct(s.pct)}%{transform:translateY(${s.i ? -s.i * slotH : 0}px)}`).join('')
      + `100%{transform:translateY(${-(stops.length - 1) * slotH}px)}`;
    css += `@keyframes play{${frames}}#strip{animation:play ${total / 1000}s step-end infinite}`;
    const rk = opts.reduceTo ?? 0;
    // a key hint with no motion to follow is confusing, so the still frame keeps the caption only
    css += `@media (prefers-reduced-motion:reduce){#strip{animation:none;transform:translateY(${rk ? -rk * slotH : 0}px)}.key{display:none}}`;
  }

  const clip = isPhone
    ? `<clipPath id="vp"><rect x="9" y="24" width="${n2(termW + 12)}" height="${n2(termH + 12)}" rx="22"/></clipPath>`
    : `<clipPath id="vp"><rect x="0" y="${BAR + PADY}" width="${n2(W)}" height="${slotH}"/></clipPath>`
      + `<clipPath id="win"><rect width="${n2(W)}" height="${H}" rx="8"/></clipPath>`;
  const body = `<g clip-path="url(#vp)"><g transform="translate(${pad.x} ${pad.y})"><g id="strip">${slots}</g></g></g>`;
  const inner = isPhone ? back + body : `<g clip-path="url(#win)">${back}${body}</g>${front}`;

  return `<svg xmlns="http://www.w3.org/2000/svg" role="img" width="${n2(W)}" height="${H}" viewBox="0 0 ${n2(W)} ${H}" `
    + `font-family='${MONO}' font-size="${FS}" fill="currentColor" class="${fgBase}">`
    + `<title>${esc(title || 'paddock')}</title>${desc ? `<desc>${esc(desc)}</desc>` : ''}`
    + `<style>${paints.css()}${css}</style>`
    + `<defs>${clip}${defs}</defs>${inner}</svg>\n`;
}

export const dims = (data, chrome) => (chrome === 'phone'
  ? { slotH: data.rows * CH, W: data.cols * CW + 30, H: data.rows * CH + 54 }
  : { slotH: data.rows * CH + PADY + STRIP, W: data.cols * CW + 2 * PADX, H: BAR + PADY + data.rows * CH + PADY + STRIP });

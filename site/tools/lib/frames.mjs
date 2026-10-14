// Parsed terminal snapshots to the frame format (v1), the file assets/frames/<name>.js sets window.PADDOCK_FRAMES[name] to:
//   { v: 1, name, cols, rows, source, styles, lines, frames, chapters }
//   styles  interned cell styles: {fg?, bg?, b?, d?, i?, u?, s?}; fg and bg are a palette index, "fg", "bg" or "#rrggbb"
//   lines   interned rows: each is runs of [styleIndex, text], with trailing default blanks left out
//   frames  [{d: hold in ms, l: line indices, ch: chapter index, key?: key label, tap?: [col, row], cap?: caption}]
//   chapters [{id, title, cap, frame, poster}]; source records the paddock build, tmux version and date it was recorded from.

import fs from 'node:fs';
import path from 'node:path';
import { checkRepertoire } from './wcwidth.mjs';

/**
 * A space shows only its background and underline/strike; its colour and
 * weight are invisible. Dropping them keeps runs long without changing what
 * is drawn.
 */
function visible(cell) {
  if (cell.ch !== ' ') return cell.st;
  const { bg, u, s } = cell.st;
  const st = {};
  if (bg !== undefined) st.bg = bg;
  if (u) st.u = 1;
  if (s) st.s = 1;
  return st;
}

const styleKey = (st) => JSON.stringify([st.fg ?? null, st.bg ?? null, st.b ?? 0, st.d ?? 0, st.i ?? 0, st.u ?? 0, st.s ?? 0]);
const isDefault = (st) => styleKey(st) === styleKey({});

/**
 * Runs of one row: [[style, text]], merged, without trailing default spaces.
 * Also returns how many cells were kept.
 */
export function rowRuns(row) {
  let keep = row.length;
  while (keep > 0 && row[keep - 1].ch === ' ' && isDefault(visible(row[keep - 1]))) keep--;
  const runs = [];
  for (let i = 0; i < keep; i++) {
    const cell = row[i];
    if (cell.w === 0) continue;
    const st = visible(cell);
    const k = styleKey(st);
    const last = runs[runs.length - 1];
    if (last && last.k === k) last.text += cell.ch;
    else runs.push({ k, st, text: cell.ch });
  }
  return { runs, keep };
}

/**
 * Text+style fidelity of one snapshot against the plain capture:
 *  - every row has exactly COLS cells and ROWS rows exist;
 *  - the run text equals the plain row (not trimmed where a cell carries a
 *    background), and what the runs leave out is blank.
 */
export function checkFidelity(cells, plain, cols, rows) {
  if (cells.length !== rows || plain.length !== rows) throw new Error(`expected ${rows} rows, got ${cells.length} cells / ${plain.length} plain`);
  for (let r = 0; r < rows; r++) {
    if (cells[r].length !== cols) throw new Error(`row ${r}: ${cells[r].length} cells, expected ${cols}`);
    const chars = [...plain[r].padEnd(cols)];
    if (chars.length !== cols) throw new Error(`row ${r}: plain capture has ${chars.length} columns, expected ${cols}: ${JSON.stringify(plain[r])}`);
    const { runs, keep } = rowRuns(cells[r]);
    const text = runs.map((x) => x.text).join('');
    const want = chars.slice(0, keep).join('');
    if (text !== want) throw new Error(`row ${r}: runs ${JSON.stringify(text)} differ from plain ${JSON.stringify(want)}`);
    if (chars.slice(keep).some((c) => c !== ' ')) throw new Error(`row ${r}: plain capture has text after the runs: ${JSON.stringify(plain[r])}`);
  }
}

/**
 * Builds the frame set. SNAPS: [{cells, plain, hold, key?, tap?, cap?, chapter}]
 * (chapter: index into CHAPTERS, whose `frame` and `poster` are set by the
 * caller). Returns the object that is written to disk.
 */
export function buildSet({ name, cols, rows, source, chapters, snaps }) {
  const styles = [{}];
  const styleIdx = new Map([[styleKey({}), 0]]);
  const lines = [];
  const lineIdx = new Map();
  const styleOf = (st) => {
    const k = styleKey(st);
    if (!styleIdx.has(k)) {
      styleIdx.set(k, styles.length);
      styles.push(st);
    }
    return styleIdx.get(k);
  };
  const frames = snaps.map((snap, n) => {
    checkRepertoire(snap.plain.join('\n'), `${name} frame ${n}`);
    checkFidelity(snap.cells, snap.plain, cols, rows);
    const l = snap.cells.map((row) => {
      const runs = rowRuns(row).runs.map((x) => [styleOf(x.st), x.text]);
      const key = JSON.stringify(runs);
      if (!lineIdx.has(key)) {
        lineIdx.set(key, lines.length);
        lines.push(runs);
      }
      return lineIdx.get(key);
    });
    const f = { d: snap.hold, l, ch: snap.chapter };
    if (snap.key) f.key = snap.key;
    if (snap.tap) f.tap = snap.tap;
    if (snap.cap) f.cap = snap.cap;
    return f;
  });
  return { v: 1, name, cols, rows, source, styles, lines, frames, chapters };
}

/** The data as the .js file the site loads (classic script, no fetch). */
export function toJs(data) {
  const j = (v) => JSON.stringify(v);
  return [
    'window.PADDOCK_FRAMES = window.PADDOCK_FRAMES || {};',
    `window.PADDOCK_FRAMES[${j(data.name)}] = {`,
    `v: ${data.v}, name: ${j(data.name)}, cols: ${data.cols}, rows: ${data.rows},`,
    `source: ${j(data.source)},`,
    `styles: [\n${data.styles.map(j).join(',\n')}\n],`,
    `lines: [\n${data.lines.map(j).join(',\n')}\n],`,
    `frames: [\n${data.frames.map(j).join(',\n')}\n],`,
    `chapters: [\n${data.chapters.map(j).join(',\n')}\n]`,
    '};',
    '',
  ].join('\n');
}

// Palette and glyph boxes for the review sheet only; the site's player.css has
// the real ones and overrides these when it loads.
const FALLBACK_PALETTE = ['#1b2220', '#f2777a', '#7fd49a', '#f0c674', '#7cafe8', '#c792ea', '#6fd3d3', '#c5cec9', '#7f8e86', '#ff9a9c', '#a4e8b8', '#ffd98e', '#a3c8f2', '#ddb4f5', '#9ce6e6', '#f2f6f4'];

function fallbackCss() {
  const vars = FALLBACK_PALETTE.map((c, i) => `--t-${i}:${c};`).join('');
  const rules = FALLBACK_PALETTE.map((_, i) => `.sheet .f${i}{color:var(--t-${i})}.sheet .k${i}{background:var(--t-${i})}`).join('');
  return `.sheet{--t-bg:#0b0e0d;--t-fg:#d5ddd8;${vars}}` +
    '.sheet pre{background:var(--t-bg);color:var(--t-fg);font:13px/16px ui-monospace,SFMono-Regular,Menlo,monospace;margin:0 0 4px;padding:8px;width:max-content;font-variant-ligatures:none}' +
    '.sheet .ffg{color:var(--t-fg)}.sheet .fbg{color:var(--t-bg)}.sheet .kfg{background:var(--t-fg)}.sheet .kbg{background:var(--t-bg)}' +
    '.sheet .B{font-weight:700}.sheet .D{opacity:.6}.sheet .I{font-style:italic}.sheet .U{text-decoration:underline}.sheet .S{text-decoration:line-through}' +
    '.sheet .g{display:inline-block;width:1ch;text-align:center;font-style:normal}.sheet .bgr{display:inline-block;vertical-align:top}' +
    rules +
    '.sheet body,.sheet{background:#222;color:#ddd;font:13px system-ui}.sheet h3{margin:16px 0 4px;font:600 13px ui-monospace,monospace}';
}

const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

function runHtml([si, text], styles) {
  const st = styles[si];
  const cls = [];
  let style = '';
  for (const [prop, pre] of [['fg', 'f'], ['bg', 'k']]) {
    const v = st[prop];
    if (v === undefined) continue;
    if (typeof v === 'number') cls.push(pre + v);
    else if (v === 'fg' || v === 'bg') cls.push(pre + v);
    else style += `${prop === 'fg' ? 'color' : 'background'}:${v};`;
  }
  for (const [f, c] of [['b', 'B'], ['d', 'D'], ['i', 'I'], ['u', 'U'], ['s', 'S']]) if (st[f]) cls.push(c);
  if (st.bg !== undefined) cls.push('bgr');
  const body = [...text].map((ch) => (ch.charCodeAt(0) < 128 ? esc(ch) : `<i class="g">${ch}</i>`)).join('');
  if (!cls.length && !style) return body;
  return `<span${cls.length ? ` class="${cls.join(' ')}"` : ''}${style ? ` style="${style}"` : ''}>${body}</span>`;
}

export function frameHtml(data, n) {
  return data.frames[n].l.map((li) => data.lines[li].map((r) => runHtml(r, data.styles)).join('')).join('\n');
}

/** Every frame stacked with its index, hold and caption. FALLBACK adds a minimal palette for when player.css is missing. */
export function sheetHtml(data, { fallback = false } = {}) {
  const parts = data.frames.map((f, n) => {
    const ch = data.chapters[f.ch];
    const poster = data.chapters.filter((c) => c.poster === n).map((c) => `poster:${c.id}`);
    const meta = [`#${n}`, `${f.d} ms`, ch && `chapter ${ch.id}`, f.key && `key "${f.key}"`, f.tap && `tap ${f.tap}`, f.cap && `cap "${f.cap}"`, ...poster].filter(Boolean).join(' | ');
    const size = `--pd-cols:${data.cols};--pd-rows:${data.rows};--pd-fs:13px;--pd-lh:16px`;
    return `<h3>${esc(meta)}</h3>\n<div class="pd-term" style="width:max-content"><pre class="pd-screen" style="${size}">${frameHtml(data, n)}</pre></div>`;
  });
  return `<!doctype html><meta charset="utf-8"><title>${data.name} frames</title>
<link rel="stylesheet" href="../../assets/css/player.css">
${fallback ? `<style>${fallbackCss()}</style>` : '<style>body{margin:12px;font:13px system-ui}h3{margin:16px 0 4px;font:600 13px ui-monospace,monospace}</style>'}
<body class="sheet">
${parts.join('\n')}
`;
}

/** Writes frames .js, .json and the sheet. Returns the byte size of the .js. */
export function writeAll(data, { framesDir, outDir, playerCss }) {
  fs.mkdirSync(framesDir, { recursive: true });
  fs.mkdirSync(outDir, { recursive: true });
  const js = toJs(data);
  fs.writeFileSync(path.join(framesDir, `${data.name}.js`), js);
  fs.writeFileSync(path.join(outDir, `${data.name}.json`), JSON.stringify(data));
  fs.writeFileSync(path.join(outDir, `${data.name}-sheet.html`), sheetHtml(data, { fallback: !fs.existsSync(playerCss) }));
  return Buffer.byteLength(js);
}

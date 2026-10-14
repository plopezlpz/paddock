import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import { parse } from '../lib/ansi.mjs';
import { buildSet, checkFidelity, rowRuns, sheetHtml, toJs } from '../lib/frames.mjs';
import { spawnSync } from 'node:child_process';
import { checkRepertoire } from '../lib/wcwidth.mjs';
import { overflow, readTranscript } from '../lib/scenario.mjs';

const E = '\x1b';
const COLS = 12;
const grid = (text, rows = 2) => {
  const cells = parse(text, COLS, rows);
  return { cells, plain: cells.map((r) => r.map((c) => c.ch).join('')) };
};

test('runs merge equal styles and drop trailing default spaces', () => {
  const { cells } = grid(`${E}[1mab${E}[0m cd   `);
  const { runs, keep } = rowRuns(cells[0]);
  assert.equal(keep, 5);
  assert.deepEqual(runs.map((r) => [r.st, r.text]), [[{ b: 1 }, 'ab'], [{}, ' cd']]);
});

test('invisible style on spaces does not split runs', () => {
  const { cells } = grid(`a${E}[1;31m ${E}[0mb`);
  assert.deepEqual(rowRuns(cells[0]).runs.map((r) => r.text), ['a b']);
});

test('trailing cells with a background are kept', () => {
  const { cells } = grid(`${E}[7mab  ${E}[0m`);
  const { runs, keep } = rowRuns(cells[0]);
  assert.equal(keep, 4);
  assert.deepEqual(runs.map((r) => [r.st, r.text]), [[{ fg: 'bg', bg: 'fg' }, 'ab'], [{ bg: 'fg' }, '  ']]);
});

test('fidelity passes for a faithful grid and fails on any text difference', () => {
  const g = grid(`${E}[32mhello${E}[0m\nworld`);
  checkFidelity(g.cells, g.plain, COLS, 2);
  checkFidelity(g.cells, ['hello', 'world'], COLS, 2); // short plain rows are padded
  assert.throws(() => checkFidelity(g.cells, ['hellx', 'world'], COLS, 2), /differ/);
  assert.throws(() => checkFidelity(g.cells, ['hello   x', 'world'], COLS, 2), /differ|after the runs/);
  assert.throws(() => checkFidelity(g.cells, g.plain, COLS, 3), /expected 3 rows/);
});

test('fidelity checks reversed trailing spaces against the plain capture', () => {
  const g = grid(`${E}[7mab   ${E}[0m`);
  checkFidelity(g.cells, g.plain, COLS, 2);
  checkFidelity(g.cells, ['ab', ''], COLS, 2);
  assert.throws(() => checkFidelity(g.cells, ['abc', ''], COLS, 2), /differ/);
});

function sample() {
  const a = grid(`${E}[1;31mAAA${E}[0m b\n${E}[7m sel ${E}[0m`);
  const b = grid(`${E}[1;31mAAA${E}[0m b\n${E}[2mdim${E}[0m`);
  return buildSet({
    name: 't', cols: COLS, rows: 2,
    source: { paddock: '0' },
    chapters: [{ id: 'one', title: 'One', cap: 'c', frame: 0, poster: 1 }],
    snaps: [
      { ...a, hold: 100, chapter: 0, key: 'k' },
      { ...b, hold: 200, chapter: 0, tap: [1, 0], cap: 'x' },
    ],
  });
}

test('a frame set interns styles and lines, and round-trips through the .js file', () => {
  const data = sample();
  assert.equal(data.v, 1);
  assert.deepEqual(data.styles[0], {});
  assert.equal(data.frames[0].l[0], data.frames[1].l[0], 'the identical first row is one line');
  assert.notEqual(data.frames[0].l[1], data.frames[1].l[1]);
  assert.deepEqual(data.frames.map((f) => f.d), [100, 200]);
  assert.equal(data.frames[0].key, 'k');
  assert.deepEqual(data.frames[1].tap, [1, 0]);
  const win = {};
  vm.runInNewContext(toJs(data), { window: win });
  assert.deepEqual(JSON.parse(JSON.stringify(win.PADDOCK_FRAMES.t)), JSON.parse(JSON.stringify(data)));
  // every line's text is the row's text
  assert.equal(data.lines[data.frames[0].l[0]].map((r) => r[1]).join(''), 'AAA b');
  assert.deepEqual(data.styles[data.lines[data.frames[0].l[0]][0][0]], { fg: 1, b: 1 });
});

test('the review sheet renders glyphs in their own box and escapes markup', () => {
  const g = grid('a<b ●');
  const data = buildSet({ name: 's', cols: COLS, rows: 2, source: {}, chapters: [{ id: 'c', frame: 0, poster: 0 }], snaps: [{ ...g, hold: 1, chapter: 0 }] });
  const html = sheetHtml(data);
  assert.match(html, /a&lt;b <i class="g">●<\/i>/);
});

test('a glyph outside the repertoire fails the build', () => {
  const g = grid('☃');
  assert.throws(() => buildSet({ name: 's', cols: COLS, rows: 2, source: {}, chapters: [], snaps: [{ ...g, hold: 1, chapter: 0 }] }), /repertoire/);
});

test('transcripts: comments dropped, \\e and \\r turned into bytes, overflow found', () => {
  const file = `${process.env.TMPDIR || '/tmp'}/paddock-demo-ans-test-${process.pid}.ans`;
  fs.writeFileSync(file, '# comment\n\\e[1mhi\\e[0m\\r\n');
  try {
    assert.equal(readTranscript(file), `${E}[1mhi${E}[0m\r\n`);
  } finally {
    fs.rmSync(file, { force: true });
  }
  fs.writeFileSync(file, 'bad \\n escape');
  try {
    assert.throws(() => readTranscript(file), /unsupported escape/);
  } finally {
    fs.rmSync(file, { force: true });
  }
  assert.deepEqual(overflow(`${E}[2Jabc\n${E}[1mabcdef${E}[0m\n`, 6), ['abcdef']);
});

test('every demo transcript reads and stays inside the glyph repertoire', () => {
  const dir = new URL('../demo/transcripts/', import.meta.url);
  const files = fs.readdirSync(dir).filter((f) => f.endsWith('.ans'));
  assert.ok(files.length > 10);
  for (const f of files) checkRepertoire(readTranscript(new URL(f, dir)), f);
});

test('the demo suggest stub answers for the ids of the demo panes and fails for others', () => {
  const stub = new URL('../demo/suggest.sh', import.meta.url).pathname;
  const run = (id) => spawnSync('sh', [stub, id], { encoding: 'utf8' });
  const ok = run('%0');
  assert.equal(ok.status, 0);
  assert.match(ok.stdout.trim(), /^\S.*\S$/);
  const bad = run('%99');
  assert.equal(bad.status, 1);
  assert.equal(bad.stdout, '');
  assert.match(bad.stderr, /no demo title for %99/);
});

import test from 'node:test';
import assert from 'node:assert/strict';
import { parse, emit, color256, rowText } from '../lib/ansi.mjs';
import { width, checkRepertoire } from '../lib/wcwidth.mjs';

const E = '\x1b';
const one = (text, cols = 8) => parse(text, cols, 1)[0];
const st = (cell) => ({ ...cell.st });

test('plain text pads to the grid', () => {
  const g = parse('ab', 4, 2);
  assert.equal(g.length, 2);
  assert.equal(rowText(g[0]), 'ab  ');
  assert.equal(rowText(g[1]), '    ');
  assert.deepEqual(st(g[0][0]), {});
});

test('basic colours, bold and reset', () => {
  const r = one(`${E}[1;31mA${E}[0mB${E}[42mC${E}[49;93mD`);
  assert.deepEqual(st(r[0]), { fg: 1, b: 1 });
  assert.deepEqual(st(r[1]), {});
  assert.deepEqual(st(r[2]), { bg: 2 });
  assert.deepEqual(st(r[3]), { fg: 11 });
});

test('compound reset form \\e[0;2m is a reset then dim', () => {
  const r = one(`${E}[1;32mA${E}[0;2mB`);
  assert.deepEqual(st(r[0]), { fg: 2, b: 1 });
  assert.deepEqual(st(r[1]), { d: 1 });
});

test('22 clears bold and dim; 23, 24, 29 clear italic, underline, strike', () => {
  const r = one(`${E}[1;2;3;4;9mA${E}[22mB${E}[23;24;29mC`);
  assert.deepEqual(st(r[0]), { b: 1, d: 1, i: 1, u: 1, s: 1 });
  assert.deepEqual(st(r[1]), { i: 1, u: 1, s: 1 });
  assert.deepEqual(st(r[2]), {});
});

test('reverse with default colours swaps to fg/bg names', () => {
  const r = one(`${E}[7mA${E}[27mB`);
  assert.deepEqual(st(r[0]), { fg: 'bg', bg: 'fg' });
  assert.deepEqual(st(r[1]), {});
});

test('reverse swaps real colours and keeps the other default as a name', () => {
  const r = one(`${E}[7;32mA${E}[0;7;31;44mB${E}[0;7;44mC`);
  assert.deepEqual(st(r[0]), { fg: 'bg', bg: 2 }); // reversed green on the default background
  assert.deepEqual(st(r[1]), { fg: 4, bg: 1 });
  assert.deepEqual(st(r[2]), { fg: 4, bg: 'fg' });
});

test('reverse keeps bold', () => {
  assert.deepEqual(st(one(`${E}[1;7;36mA`)[0]), { fg: 'bg', bg: 6, b: 1 });
});

test('256 colours: palette range, cube and grey ramp', () => {
  const r = one(`${E}[38;5;9mA${E}[38;5;16mB${E}[38;5;196mC${E}[38;5;231mD${E}[48;5;232mE${E}[48;5;255mF`);
  assert.equal(r[0].st.fg, 9);
  assert.equal(r[1].st.fg, '#000000');
  assert.equal(r[2].st.fg, '#ff0000');
  assert.equal(r[3].st.fg, '#ffffff');
  assert.equal(r[4].st.bg, '#080808');
  assert.equal(r[5].st.bg, '#eeeeee');
  assert.equal(color256(21), '#0000ff');
  assert.equal(color256(110), '#87afd7');
});

test('truecolor, semicolon and colon forms', () => {
  const r = one(`${E}[38;2;255;175;0mA${E}[48;2;1;2;3mB${E}[0;38:2::10:20:30mC${E}[0;38:2:40:50:60mD${E}[0;48:5:12mE${E}[0;38:5:200;1mF`);
  assert.equal(r[0].st.fg, '#ffaf00');
  assert.equal(r[1].st.bg, '#010203');
  assert.equal(r[2].st.fg, '#0a141e');
  assert.equal(r[3].st.fg, '#28323c');
  assert.deepEqual(st(r[4]), { bg: 12 });
  assert.deepEqual(st(r[5]), { fg: '#ff00d7', b: 1 });
});

test('colon underline subparameters', () => {
  const r = one(`${E}[4:3mA${E}[4:0mB`);
  assert.equal(r[0].st.u, 1);
  assert.equal(r[1].st.u, undefined);
});

test('OSC and other CSI sequences are stripped', () => {
  const r = one(`${E}]8;;http://x${E}\\link${E}]0;title\x07${E}[2K${E}[1;5HX${E}[?25l`);
  assert.equal(rowText(r), 'linkX   ');
});

test('ESC sequences with intermediate bytes are skipped whole', () => {
  assert.equal(rowText(one(`${E}(Bab${E}#8c`, 4)), 'abc ');
});

test('state carries across lines, as in the stream', () => {
  const g = parse(`${E}[31mA\nB${E}[0m`, 2, 2);
  assert.equal(g[1][0].st.fg, 1);
});

test('wide characters take two cells; combining marks none', () => {
  const r = one('a日b́c', 6);
  assert.deepEqual(r.map((c) => [c.ch, c.w]), [['a', 1], ['日', 2], ['', 0], ['b', 1], ['c', 1], [' ', 1]]);
  assert.equal(rowText(r), 'a\u65e5bc ');
  assert.equal(width('日'), 2);
  assert.equal(width('́'), 0);
  assert.equal(width('●'), 1);
});

test('strict parse rejects an overlong row or the wrong row count', () => {
  assert.throws(() => parse('abcdef', 4, 1, { strict: true }), /more than 4/);
  assert.throws(() => parse('a\nb\nc', 4, 2, { strict: true }), /3 rows, more than 2/);
  assert.doesNotThrow(() => parse('ab\n\n', 4, 2, { strict: true }));
});

test('round trip: emit then parse gives the same cells', () => {
  const src = [
    `${E}[1;32mgreen bold${E}[0m plain ${E}[7mrev${E}[27m ${E}[7;34mblue-rev${E}[0m`,
    `${E}[2mdim${E}[0;38;5;208m256${E}[0;48;2;10;20;30;4mtruecolor bg${E}[0m ●○▌`,
    `${E}[0;7;41mred bg reversed${E}[0m  ${E}[3;9mit strike${E}[0m`,
  ].join('\n');
  const a = parse(src, 40, 3);
  const b = parse(emit(a), 40, 3);
  assert.deepEqual(b.map((r) => r.map((c) => [c.ch, c.w, { ...c.st }])), a.map((r) => r.map((c) => [c.ch, c.w, { ...c.st }])));
});

test('glyph repertoire: ASCII and listed glyphs pass, others are named in the error', () => {
  assert.doesNotThrow(() => checkRepertoire('a ╭─╮ ●○ ▌ ‣'.replace('‣', '')));
  assert.doesNotThrow(() => checkRepertoire('✎ rename  ✦ suggesting…  ⇅ moving'), 'rename, suggest and move marks');
  assert.throws(() => checkRepertoire('ok\n日'), /wide character/);
  assert.throws(() => checkRepertoire('é'), /combining/);
  assert.throws(() => checkRepertoire('☃'), /glyph repertoire/);
});

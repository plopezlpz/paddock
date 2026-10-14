import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { buildSvg, dims, keyframeStops, loadFrames, loadPalette, mix, paceHolds } from '../lib/svg.mjs';

const site = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const tour = loadFrames(path.join(site, 'assets/frames/tour.js'));
const phone = loadFrames(path.join(site, 'assets/frames/phone.js'));
const playerCss = path.join(site, 'assets/css/player.css');
const pal = loadPalette(playerCss);
const all = tour.frames.map((_, i) => i);
const agents = tour.chapters.find((c) => c.id === 'agents').poster;
const phoneAgents = phone.chapters.find((c) => c.id === 'agents').poster;
const holds = paceHolds(tour.frames, { pace: 30000 });
const demo = (extra = {}) => buildSvg(tour, { frames: all, holds, reduceTo: agents, palettes: [{ pal }], ...extra });

/** Minimal XML well-formedness: balanced tags, quoted attributes, escaped text, unique ids. */
function checkXml(xml) {
  const stack = [], ids = new Set();
  const tag = /<(\/?)([A-Za-z][\w:-]*)((?:\s+[\w:-]+="[^"<]*"|\s+[\w:-]+='[^'<]*')*)\s*(\/?)>/y;
  let i = 0;
  while (i < xml.length) {
    if (xml[i] === '<') {
      tag.lastIndex = i;
      const m = tag.exec(xml);
      assert.ok(m, `bad tag at ${i}: ${xml.slice(i, i + 80)}`);
      const [whole, close, name, attrs, self] = m;
      if (close) assert.equal(stack.pop(), name, `mismatched </${name}> at ${i}`);
      else {
        for (const a of attrs.matchAll(/\sid="([^"]*)"/g)) {
          assert.ok(!ids.has(a[1]), `duplicate id ${a[1]}`);
          ids.add(a[1]);
        }
        if (!self) stack.push(name);
      }
      i += whole.length;
    } else {
      const j = xml.indexOf('<', i);
      const text = xml.slice(i, j < 0 ? xml.length : j);
      assert.ok(!/&(?!amp;|lt;|gt;|quot;|apos;|#\d+;)/.test(text), `unescaped & near ${i}`);
      i += text.length;
    }
  }
  assert.deepEqual(stack, [], 'unclosed tags');
}

test('the palette comes from player.css, and the terminal is always dark', () => {
  assert.equal(pal.bg, '#0b0e0d');
  assert.equal(pal.fg, '#d5ddd8');
  assert.equal(pal.ansi[8], '#7f8e86');
  assert.equal(pal.ansi.length, 16);
  assert.equal(pal.dim, 0.66);
  assert.equal(pal.accent, '#6fd08c');
});

test('a theme file overrides the palette: later files win', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pal-'));
  try {
    const theme = path.join(dir, 'theme.css');
    fs.writeFileSync(theme, '.page {\n  --t-dim: 50%; --t-bg: #181818; --t-fg: #e4e4e4; --t-accent: #a2a970;\n  --t-8: #7e7e7e;\n  position: relative; font: 19px/1.62 var(--serif);\n}\n');
    const themed = loadPalette(playerCss, theme);
    assert.equal(themed.bg, '#181818');
    assert.equal(themed.fg, '#e4e4e4');
    assert.equal(themed.dim, 0.5);
    assert.equal(themed.accent, '#a2a970');
    assert.equal(themed.ansi[8], '#7e7e7e');
    assert.equal(themed.ansi[1], pal.ansi[1], 'colours the theme leaves out keep the player.css value');
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('dim mixes toward the background like color-mix', () => {
  assert.equal(mix('#ffffff', '#000000', 0.5), '#808080');
  assert.equal(mix('#000000', '#ffffff', 0.66), '#575757');
});

test('every svg is well-formed XML', () => {
  checkXml(demo());
  checkXml(buildSvg(phone, { frames: [phoneAgents], chrome: 'phone', palettes: [{ pal }] }));
  checkXml(buildSvg(tour, { frames: [agents], palettes: [{ pal }] }));
});

test('every <use> and url() target exists', () => {
  const svg = demo();
  const ids = new Set([...svg.matchAll(/\sid="([^"]+)"/g)].map((m) => m[1]));
  const refs = [...svg.matchAll(/href="#([^"]+)"/g)].map((m) => m[1]).concat([...svg.matchAll(/url\(#([^)]+)\)/g)].map((m) => m[1]));
  assert.ok(refs.length > 100);
  for (const r of refs) assert.ok(ids.has(r), `missing #${r}`);
  // each frame draws every non-blank row once
  const uses = (svg.match(/<use /g) || []).length;
  const visible = ([si, t]) => t.trim() || tour.styles[si].bg !== undefined || tour.styles[si].u || tour.styles[si].s;
  const expected = tour.frames.reduce((n, f) => n + f.l.filter((li) => tour.lines[li].some(visible)).length, 0);
  assert.equal(uses, expected, 'one <use> per visible row per frame');
});

test('pacing: loop near the target, typing short, end rests, keyed frames longer', () => {
  const total = holds.reduce((a, b) => a + b, 0);
  assert.ok(Math.abs(total - 30000) <= 100, `total ${total}`);
  tour.frames.forEach((f, i) => {
    if (i < tour.frames.length - 1 && f.d <= 300) assert.equal(holds[i], f.d, `typing frame ${i}`);
    if (i < tour.frames.length - 1 && f.d > 300) assert.ok(holds[i] > f.d, `frame ${i} should get more time`);
  });
  assert.ok(holds.at(-1) >= 2500);
  const factor = (pick) => {
    const ix = tour.frames.map((f, i) => i).filter((i) => i < tour.frames.length - 1 && tour.frames[i].d > 300 && pick(tour.frames[i]));
    return ix.reduce((a, i) => a + holds[i], 0) / ix.reduce((a, i) => a + tour.frames[i].d, 0);
  };
  assert.ok(factor((f) => f.key || f.cap) > factor((f) => !f.key && !f.cap), 'keyed or captioned frames get a larger factor');
  assert.deepEqual(paceHolds(tour.frames, { pace: null }), tour.frames.map((f) => f.d));
});

test('keyframes: one stop per frame at the cumulative hold, final hold runs to the loop boundary', () => {
  const svg = demo();
  const { total } = keyframeStops(holds);
  const css = /@keyframes play\{([^@]*?)\}#strip\{animation:play ([\d.]+)s step-end infinite\}/.exec(svg);
  assert.ok(css, 'keyframes and animation rule');
  assert.equal(+css[2] * 1000, total, 'duration equals the sum of the holds');
  const stops = [...css[1].matchAll(/([\d.]+)%\{transform:translateY\((-?\d+(?:\.\d+)?)px\)\}/g)].map((m) => ({ pct: +m[1], y: +m[2] }));
  const { slotH } = dims(tour, 'window');
  assert.equal(stops.length, tour.frames.length + 1, 'a stop per frame plus the 100% stop');
  let t = 0;
  tour.frames.forEach((_, i) => {
    assert.ok(Math.abs(stops[i].pct - (t / total) * 100) < 0.001, `frame ${i} starts at ${t} ms, got ${stops[i].pct}%`);
    assert.equal(Math.abs(stops[i].y), i * slotH, `frame ${i} offset`);
    t += holds[i];
  });
  assert.equal(stops[0].pct, 0);
  const last = stops.at(-1);
  assert.equal(last.pct, 100);
  assert.equal(Math.abs(last.y), (tour.frames.length - 1) * slotH, 'the last frame holds until the loop restarts');
  const lastHold = ((100 - stops.at(-2).pct) / 100) * total;
  assert.ok(Math.abs(lastHold - holds.at(-1)) < 1, `final hold ${lastHold}`);
});

test('reduced motion shows the agents frame; a static image has no animation', () => {
  const { slotH } = dims(tour, 'window');
  assert.ok(demo().includes(`@media (prefers-reduced-motion:reduce){#strip{animation:none;transform:translateY(-${agents * slotH}px)}.key{display:none}}`));
  assert.ok(!buildSvg(tour, { frames: [agents], palettes: [{ pal }] }).includes('@keyframes'));
});

test('one dark palette serves light and dark pages: no colour-scheme override', () => {
  const svg = buildSvg(tour, { frames: [agents], palettes: [{ pal }] });
  assert.ok(!svg.includes('prefers-color-scheme'));
  assert.ok(svg.includes(pal.bg));
  assert.ok(!svg.includes('#fdfdfb'), 'no light terminal background');
});

test('structure glyphs are vectors, other glyphs keep their own cell', () => {
  const svg = demo();
  assert.ok(!/[│─╭╮╰╯├└●○▌▏]/.test(svg), 'no box drawing or dot glyph left as text');
  assert.ok(/<text x="[\d.]+" y="13" text-anchor="middle">[›⎿✻]<\/text>/.test(svg));
});

// ---- grid under two fonts (needs chromium; skipped without it) ----
test('a mixed ASCII / box / symbol line lines up in two fonts', async (t) => {
  let chromium;
  try { ({ chromium } = await import('playwright')); } catch { return t.skip('playwright missing'); }
  const mixed = {
    cols: 14, rows: 1, styles: [{}, { fg: 2 }],
    lines: [[[0, 'ab'], [1, '│'], [0, 'c '], [1, '●'], [0, 'd'], [1, '╭'], [0, 'e⏎f ⇥ '], [1, '→'], [0, 'gh']]],
    frames: [{ d: 1000, l: [0], ch: 0 }], chapters: [{ cap: 'x' }],
  };
  let browser;
  try { browser = await chromium.launch(); } catch { return t.skip('chromium not installed'); }
  try {
    const boxes = {};
    for (const shapes of [true, false]) for (const font of ['Menlo,monospace', 'Times New Roman,serif']) {
      const svg = buildSvg(mixed, { frames: [0], palettes: [{ pal }], shapes }).replace(/font-family='[^']*'/, `font-family="${font}"`);
      const page = await browser.newPage();
      await page.setContent(`<body style="margin:0">${svg}`);
      boxes[`${shapes}/${font}`] = await page.evaluate(() => [...document.querySelectorAll('svg text')]
        .filter((el) => el.closest('defs'))
        .map((el) => { const b = el.getBBox(); return [el.textContent, b.x, b.x + b.width]; }));
      await page.close();
    }
    for (const shapes of [true, false]) {
      const a = boxes[`${shapes}/Menlo,monospace`], b = boxes[`${shapes}/Times New Roman,serif`];
      assert.equal(a.length, b.length);
      assert.ok(a.length >= 3);
      a.forEach(([s, left, right], i) => {
        const [, left2, right2] = b[i];
        const what = `${JSON.stringify(s)} (shapes ${shapes}): ${left}..${right} vs ${left2}..${right2}`;
        if (s.length >= 2) {
          assert.ok(Math.abs(left - left2) < 1 && Math.abs(right - right2) < 1, `run edges move, ${what}`);
        } else if (s.charCodeAt(0) > 126) {
          assert.ok(Math.abs((left + right) / 2 - (left2 + right2) / 2) < 2, `glyph leaves its cell, ${what}`);
        }
      });
    }
  } finally {
    await browser.close();
  }
});

test('tap markers: none in static images, outline only when animated', () => {
  const tapped = phone.frames.findIndex((f) => f.tap);
  assert.ok(tapped >= 0, 'phone recording has a tap frame');
  const ring = /<circle[^>]*r="13"[^>]*>/;
  assert.ok(!ring.test(buildSvg(phone, { frames: [tapped], chrome: 'phone', palettes: [{ pal }] })), 'static frame has no ring');
  const m = ring.exec(buildSvg(phone, { frames: phone.frames.map((_, i) => i), holds: phone.frames.map((f) => f.d), chrome: 'phone', palettes: [{ pal }] }));
  assert.ok(m && m[0].includes('fill="none"') && !m[0].includes('fill-opacity'), 'animated ring is an outline');
});

test('keycaps: shown while animating, hidden under reduced motion, absent from static images', () => {
  const keyed = tour.frames.filter((f) => f.key).length;
  const svg = demo();
  assert.equal((svg.match(/<g class="key">/g) || []).length, keyed, 'one key group per keyed frame');
  assert.ok(svg.includes('.key{display:none}'), 'reduced motion hides the keycaps');
  const still = buildSvg(tour, { frames: [agents], palettes: [{ pal }] });
  assert.ok(!still.includes('class="key"'), 'no keycap in a static image');
  checkXml(svg);
});

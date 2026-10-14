// Screenshots of the generated README SVGs into tools/.out/svg/.
//   node tools/shoot-svg.mjs [--paddock <repo>]   (default: this folder's parent, or PADDOCK_REPO)
// Each animated demo is opened as a document, its animations paused and set to the start of every
// chapter. Every image is shot on a dark and a light page (the terminal is always dark, so each is one file). Reduced motion is checked twice:
// with Playwright's emulation on the document, and through an <img> with chromium's real
// --force-prefers-reduced-motion switch (emulation does not reach images).
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { chromium } from 'playwright';
import { keyframeStops, loadFrames, paceHolds } from './lib/svg.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const site = path.resolve(here, '..');
const argv = process.argv.slice(2);
const pi = argv.indexOf('--paddock');
const brand = path.join(pi < 0 ? path.resolve(process.env.PADDOCK_REPO || path.join(site, '..')) : path.resolve(argv[pi + 1]), 'docs', 'brand');
const out = path.join(here, '.out', 'svg');
fs.mkdirSync(out, { recursive: true });

const tour = loadFrames(path.join(site, 'assets/frames/tour.js'));
const holds = paceHolds(tour.frames, { pace: 30000 });
const { stops } = keyframeStops(holds);
const page_bg = { dark: '#0d1117', light: '#ffffff' }; // what GitHub shows behind a README image

let failed = false;
const check = (ok, what) => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`);
  if (!ok) failed = true;
};
const size = async (p) => p.evaluate(() => {
  const r = document.documentElement.getBoundingClientRect();
  return { w: Math.ceil(r.width), h: Math.ceil(r.height) };
});
const shot = async (p, file) => {
  const { w, h } = await size(p);
  const buf = await p.screenshot({ path: path.join(out, file), clip: { x: 0, y: 0, width: w, height: h } });
  return crypto.createHash('sha1').update(buf).digest('hex');
};

const browser = await chromium.launch();
const open = async (opts, file) => {
  const ctx = await browser.newContext({ deviceScaleFactor: 2, viewport: { width: 1200, height: 800 }, ...opts });
  const p = await ctx.newPage();
  await p.goto(pathToFileURL(file).href);
  await p.evaluate((bg) => { document.documentElement.style.background = bg; }, page_bg[opts.colorScheme || 'light']);
  return p;
};

const FEATURES = ['status', 'preview', 'group', 'notify'];

// chapter starts of the demo, on a dark and a light page
for (const scheme of ['dark', 'light']) {
  const file = path.join(brand, 'demo.svg');
  const p = await open({ colorScheme: scheme }, file);
  const n = await p.evaluate(() => document.getAnimations().length);
  check(n === 1, `demo on a ${scheme} page: one running animation (${n})`);
  const hashes = [];
  for (const [i, c] of tour.chapters.entries()) {
    const t = stops[c.frame].t + Math.min(150, holds[c.frame] / 2);
    await p.evaluate((ms) => document.getAnimations().forEach((a) => { a.pause(); a.currentTime = ms; }), t);
    hashes.push(await shot(p, `demo-${scheme}-ch${i + 1}-${c.id}.png`));
  }
  check(new Set(hashes).size === hashes.length, `demo (${scheme} page): the ${hashes.length} chapter screenshots all differ`);
  // loop boundary: the last instant of the loop shows the last frame, the first instant of the next loop frame 0
  const total = stops.at(-1).t + holds.at(-1);
  const lastHash = [];
  for (const ms of [total - 5, total + 5, total + total - 5]) {
    await p.evaluate((v) => document.getAnimations().forEach((a) => { a.pause(); a.currentTime = v; }), ms);
    lastHash.push(await shot(p, `demo-${scheme}-loop-${Math.round(ms)}.png`));
  }
  check(lastHash[0] !== lastHash[1] && lastHash[0] === lastHash[2], `demo (${scheme} page): loop boundary steps from the last frame to the first and back`);
  await p.context().close();
}

// feature clips: animated, the start and middle shot
for (const name of FEATURES) {
  const p = await open({ colorScheme: 'dark' }, path.join(brand, `${name}.svg`));
  const n = await p.evaluate(() => document.getAnimations().length);
  check(n === 1, `${name}: one running animation (${n})`);
  const dur = await p.evaluate(() => document.getAnimations()[0].effect.getTiming().duration);
  const hashes = [];
  for (const f of [0.02, 0.5, 0.98]) {
    await p.evaluate((ms) => document.getAnimations().forEach((a) => { a.pause(); a.currentTime = ms; }), dur * f);
    hashes.push(await shot(p, `${name}-${Math.round(f * 100)}.png`));
  }
  check(new Set(hashes).size >= 2, `${name}: the clip changes over its ${(dur / 1000).toFixed(1)} s loop`);
  await p.context().close();
}

// reduced motion on the document
for (const name of ['demo', ...FEATURES]) {
  const p = await open({ colorScheme: 'dark', reducedMotion: 'reduce' }, path.join(brand, `${name}.svg`));
  const n = await p.evaluate(() => document.getAnimations().length);
  check(n === 0, `${name} reduced motion (emulated): no animation (${n})`);
  await shot(p, `${name}-reduced.png`);
  await p.context().close();
}

// the phone frame, and the site posters
for (const scheme of ['dark', 'light']) {
  const p = await open({ colorScheme: scheme }, path.join(brand, 'frame-phone.svg'));
  await shot(p, `frame-phone-${scheme}.png`);
  await p.context().close();
}
await browser.close();

// through <img>: real-time animation, and real reduced motion
const html = path.join(out, 'embed.html');
fs.writeFileSync(html, `<!doctype html><meta charset="utf-8"><body style="margin:0">
<img id="demo" src="${pathToFileURL(path.join(brand, 'demo.svg'))}" width="1141">
<img id="phone" src="${pathToFileURL(path.join(brand, 'frame-phone.svg'))}" width="300">`);
for (const [label, args] of [['img-animated', []], ['img-reduced', ['--force-prefers-reduced-motion']]]) {
  const b = await chromium.launch({ args });
  const ctx = await b.newContext({ deviceScaleFactor: 2, viewport: { width: 1141, height: 1400 }, colorScheme: 'dark' });
  const p = await ctx.newPage();
  await p.goto(pathToFileURL(html).href);
  const seen = [];
  for (let i = 0; i < 3; i++) {
    await p.waitForTimeout(i ? 4200 : 600);
    const buf = await p.locator('#demo').screenshot({ path: path.join(out, `${label}-${i}.png`) });
    seen.push(crypto.createHash('sha1').update(buf).digest('hex'));
  }
  if (label === 'img-animated') check(new Set(seen).size === 3, 'img embed: frames advance in real time (3 samples differ)');
  else check(new Set(seen).size === 1, 'img embed with real reduced motion: the image stays still (3 samples equal)');
  await p.locator('#phone').screenshot({ path: path.join(out, `${label}-phone.png`) });
  await b.close();
}
console.log(`screenshots in ${out}`);
if (failed) process.exit(1);

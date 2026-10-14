// Site screenshots plus audits for the scroll-driven page: wide (sticky stage + steps), stacked, reduced motion, no JS.
//   node shoot.mjs                  start python3 -m http.server on a free port, shoot, kill it
//   node shoot.mjs --url http://127.0.0.1:8765/   use a server you already run
//   node shoot.mjs --out some/dir   screenshots go to tools/.out/shots by default
// Exit code 1 on any FAIL. Needs python3 (the local server) and Playwright's chromium (npx playwright install chromium).
import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import net from 'node:net';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const siteRoot = path.resolve(here, '..');
const argv = process.argv.slice(2);
const arg = (name) => {
  const i = argv.findIndex((a) => a === `--${name}` || a.startsWith(`--${name}=`));
  if (i < 0) return null;
  return argv[i].includes('=') ? argv[i].split('=').slice(1).join('=') : argv[i + 1];
};
const outDir = path.resolve(arg('out') || path.join(here, '.out', 'shots'));
fs.mkdirSync(outDir, { recursive: true });

// ---- result bookkeeping ----
const results = [];
const note = (level, name, detail = '') => {
  results.push({ level, name, detail });
  if (level !== 'ok') console.log(`${level.padEnd(7)} ${name}${detail ? ': ' + detail : ''}`);
};
const check = (cond, name, detail = '') => note(cond ? 'ok' : 'FAIL', name, cond ? '' : detail);

// ---- local server ----
let server = null;
const stopServer = () => { if (server) { server.kill('SIGTERM'); server = null; } };
process.on('exit', stopServer);
for (const sig of ['SIGINT', 'SIGTERM']) process.on(sig, () => { stopServer(); process.exit(130); });

async function freePort() {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => resolve(port)); });
    s.on('error', reject);
  });
}
async function startServer() {
  const port = await freePort();
  server = spawn('python3', ['-m', 'http.server', String(port), '--bind', '127.0.0.1', '-d', siteRoot], { stdio: 'ignore' });
  const base = `http://127.0.0.1:${port}/`;
  for (let i = 0; i < 50; i++) {
    try { const r = await fetch(base); if (r.ok) return base; } catch {}
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error('http.server did not start');
}

// ---- files the page needs ----
const onDisk = (rel) => fs.existsSync(path.join(siteRoot, rel));
const needed = [
  'assets/js/player.js', 'assets/css/player.css', ...['status', 'tour', 'phone', 'feat-group', 'feat-preview', 'feat-rename', 'feat-manage', 'feat-notify'].flatMap((n) => [`assets/frames/${n}.js`, `assets/frames/${n}-poster.svg`]),
  'assets/fonts/jetbrains-mono-latin-400-normal.woff2', 'assets/fonts/jetbrains-mono-latin-700-normal.woff2', 'assets/fonts/JetBrainsMono-OFL.txt',
  'assets/brand/favicon.svg', 'assets/brand/favicon-32.png', 'assets/brand/apple-touch-icon.png',
];

// ---- page instrumentation ----
function instrument(page, allowedOrigin) {
  const log = { errors: [], external: [], failed: [] };
  page.on('console', (m) => {
    if (m.type() !== 'error') return;
    log.errors.push(`${m.text()} ${m.location()?.url || ''}`);
  });
  page.on('pageerror', (e) => log.errors.push('pageerror: ' + e.message));
  page.on('request', (r) => {
    const u = r.url();
    if (/^(data|blob|about):/.test(u)) return;
    if (u.startsWith('file:')) return;
    if (allowedOrigin && u.startsWith(allowedOrigin)) return;
    log.external.push(u);
  });
  page.on('requestfailed', (r) => log.failed.push(r.url() + ' ' + (r.failure()?.errorText || '')));
  page.on('response', (r) => { if (r.status() >= 400) log.failed.push(`${r.status()} ${r.url()}`); });
  return log;
}

// ---- in-page audit ----
const auditInPage = (vw) => {
  const out = {};
  out.overflow = Math.max(document.documentElement.scrollWidth, window.innerWidth) - vw;
  out.offenders = out.overflow > 0
    ? [...document.querySelectorAll('body *')].filter((e) => e.getBoundingClientRect().right > vw + 1)
        .slice(0, 6).map((e) => e.tagName.toLowerCase() + '.' + (e.className?.baseVal ?? e.className))
    : [];
  const visible = (e) => { const r = e.getBoundingClientRect(); const s = getComputedStyle(e); return r.width > 0 && r.height > 0 && s.visibility !== 'hidden'; };
  out.imgsNoAlt = [...document.querySelectorAll('img')].filter((i) => !i.hasAttribute('alt')).map((i) => i.src);
  const nameOf = (e) => {
    let n = e.getAttribute('aria-label');
    if (!n && e.getAttribute('aria-labelledby')) n = e.getAttribute('aria-labelledby').split(/\s+/).map((id) => document.getElementById(id)?.textContent || '').join(' ');
    if (!n) n = e.textContent;
    if (!n) n = e.getAttribute('title');
    if (!n) n = [...e.querySelectorAll('img[alt]')].map((i) => i.alt).join(' ');
    return (n || '').trim();
  };
  out.unnamed = [...document.querySelectorAll('button, a[href], [role=tab], [role=button]')].filter((e) => !nameOf(e)).map((e) => e.outerHTML.slice(0, 80));
  out.h1 = document.querySelectorAll('h1').length;
  const levels = [...document.querySelectorAll('h1,h2,h3,h4,h5,h6')].map((h) => +h.tagName[1]);
  out.headingSkips = levels.filter((l, i) => i > 0 && l - levels[i - 1] > 1).length;
  const ids = [...document.querySelectorAll('[id]')].map((e) => e.id);
  out.dupIds = ids.filter((id, i) => ids.indexOf(id) !== i);
  out.badRefs = [...document.querySelectorAll('[aria-controls],[aria-labelledby],a[href^="#"]')].flatMap((e) => {
    const refs = e.hasAttribute('href') ? [e.getAttribute('href').slice(1)] : (e.getAttribute('aria-controls') || e.getAttribute('aria-labelledby')).split(/\s+/);
    return refs.filter((r) => r && !document.getElementById(r));
  });
  out.lang = document.documentElement.lang;
  out.landmarks = { main: document.querySelectorAll('main').length, nav: document.querySelectorAll('nav[aria-label]').length, header: document.querySelectorAll('header').length, footer: document.querySelectorAll('footer').length };
  // touch targets: real controls only, not inline text links
  out.small = [...document.querySelectorAll('button, .btn, .nav a, [role=tab], .footin a')]
    .filter(visible).map((e) => { const r = e.getBoundingClientRect(); return { h: Math.round(r.height), w: Math.round(r.width), el: e.tagName.toLowerCase() + '.' + e.className + ' ' + nameOf(e).slice(0, 20), player: !!e.closest('.pd-player') }; })
    .filter((x) => x.h < 40 || x.w < 40);
  out.clippedCode = [...document.querySelectorAll('.code pre')].filter((p) => visible(p) && p.scrollWidth > p.clientWidth + 1).map((p) => p.textContent.slice(0, 40));
  out.navRows = new Set([...document.querySelectorAll('.nav nav a')].filter(visible).map((a) => Math.round(a.getBoundingClientRect().top))).size;
  out.inlineCodeOverflow = [...document.querySelectorAll('p code, li code, td code, th code')].filter((c) => visible(c) && c.getClientRects().length > 1).map((c) => c.textContent);
  out.demo = window.paddockDemo ? { mode: paddockDemo.mode(), steps: paddockDemo.steps.length } : null;
  out.reveal = document.querySelectorAll('.pd-reveal').length;
  out.fonts = document.fonts.check('13px "JetBrains Mono"');
  out.players = [...document.querySelectorAll('.pd-player')].map((p) => ({ id: p.id, rec: p.getAttribute('data-frames'), len: (p.querySelector('.pd-screen')?.textContent || '').length, vis: visible(p) }));
  out.zoomBtns = [...document.querySelectorAll('.pd-zoom')].filter(visible).length;
  out.stepPlayers = [...document.querySelectorAll('.step .pd-player')].filter(visible).length;
  out.stagePlayers = [...document.querySelectorAll('.sc-stage > .pd-player.on')].length;
  return out;
};

const CONFIGS = [
  { name: 'desktop', sections: true, vp: [1440, 900], steps: true },
  { name: 'desktop-reduced', vp: [1440, 900], reduced: true },
  { name: 'ipad-landscape', sections: true, vp: [1024, 768] },
  { name: 'ipad-portrait', sections: true, vp: [834, 1194], mobile: true },
  { name: 'phone', sections: true, vp: [390, 844], mobile: true, zoom: true },
  { name: 'phone-reduced', vp: [390, 844], mobile: true, reduced: true },
  { name: 'narrow', sections: true, vp: [320, 640], mobile: true, zoom: true },
];

async function waitPlayers(page) {
  try {
    await page.waitForFunction(() => [...document.querySelectorAll('.pd-player[data-frames]')].every((p) => {
      return (p.querySelector('.pd-screen')?.textContent || '').length > 0;
    }), null, { timeout: 4000 });
  } catch { /* reported by the audit */ }
}

const STEP_RECS = ['status', 'tour', 'feat-group', 'feat-preview', 'feat-rename', 'feat-manage', 'feat-notify', 'phone'];

async function shootConfig(browser, base, cfg) {
  const ctx = await browser.newContext({
    viewport: { width: cfg.vp[0], height: cfg.vp[1] },
    colorScheme: 'dark',
    reducedMotion: cfg.reduced ? 'reduce' : 'no-preference',
    isMobile: !!cfg.mobile, hasTouch: !!cfg.mobile, deviceScaleFactor: cfg.mobile ? 2 : 1,
  });
  const page = await ctx.newPage();
  const log = instrument(page, new URL(base).origin);
  await page.goto(base, { waitUntil: 'load' });
  await page.evaluate(() => document.fonts.ready);
  await waitPlayers(page);
  await page.waitForTimeout(400);
  const a = await page.evaluate(auditInPage, cfg.vp[0]);
  const tag = cfg.name;
  check(log.errors.length === 0, `${tag}: no console errors`, log.errors.join(' | '));
  check(log.external.length === 0, `${tag}: no external requests`, log.external.join(' | '));
  check(log.failed.length === 0, `${tag}: no failed requests`, log.failed.join(' | '));
  check(a.overflow <= 0, `${tag}: no horizontal overflow`, `scrollWidth exceeds viewport by ${a.overflow}px; ${a.offenders.join(', ')}`);
  check(a.imgsNoAlt.length === 0, `${tag}: every img has alt`, a.imgsNoAlt.join(', '));
  check(a.unnamed.length === 0, `${tag}: every button and link has a name`, a.unnamed.join(' | '));
  check(a.h1 === 1 && a.headingSkips === 0, `${tag}: one h1, no heading level skips`, `h1=${a.h1} skips=${a.headingSkips}`);
  check(a.dupIds.length === 0 && a.badRefs.length === 0, `${tag}: ids unique, references resolve`, [...a.dupIds, ...a.badRefs].join(', '));
  check(a.lang === 'en', `${tag}: lang=en`);
  check(a.landmarks.main === 1 && a.landmarks.nav >= 1 && a.landmarks.header === 1 && a.landmarks.footer === 1, `${tag}: landmarks (one main, labelled nav, header, footer)`, JSON.stringify(a.landmarks));
  check(a.fonts, `${tag}: JetBrains Mono loaded from assets/fonts`);
  check(a.reveal === 0, `${tag}: no "show the desktop demo" button (desktop recordings play inline)`, `${a.reveal} found`);
  if (cfg.mobile) {
    const own = a.small.filter((s) => !s.player);
    check(own.length === 0, `${tag}: touch targets >= 40px`, JSON.stringify(own));
    if (a.small.some((s) => s.player)) note('warn', `${tag}: player controls under 40px`, JSON.stringify(a.small.filter((s) => s.player)));
  }
  check(a.clippedCode.length === 0, `${tag}: code blocks show their whole text`, a.clippedCode.join(' | '));
  check(a.navRows <= 1, `${tag}: nav links on one row`, `rows=${a.navRows}`);
  check(a.inlineCodeOverflow.length === 0, `${tag}: inline code does not wrap mid-name`, a.inlineCodeOverflow.join(' | '));
  check(a.demo && a.demo.steps === 8, `${tag}: the demo booted with 8 steps`, JSON.stringify(a.demo));
  const wantMode = cfg.vp[0] >= 1100 && !cfg.reduced ? 'wide' : 'stack';
  check(a.demo && a.demo.mode === wantMode, `${tag}: layout is ${wantMode}`, JSON.stringify(a.demo));
  if (wantMode === 'wide') check(a.stagePlayers === 1, `${tag}: one stage player is current`, `on=${a.stagePlayers}`);
  {
    const vis = a.players.filter((p) => p.vis);
    check(vis.length > 0 && vis.every((p) => p.len > 0), `${tag}: visible players rendered frames`, JSON.stringify(a.players));
    const all = a.players.filter((p) => p.rec);
    check(all.length > 0 && all.every((p) => p.len > 0), `${tag}: every player rendered frames`, JSON.stringify(all));
  }

  await page.screenshot({ path: path.join(outDir, `${tag}-fold.png`) });
  await page.screenshot({ path: path.join(outDir, `${tag}-full.png`), fullPage: true });

  if (cfg.sections) {
    for (const [id, sel] of [['hero', '.hero'], ['features', '#features'], ['why', '#why'], ['agents', '#agents'], ['footer', '.foot']]) {
      await page.locator(sel).screenshot({ path: path.join(outDir, `${tag}-sec-${id}.png`) });
    }
  }

  // the desktop recordings on a narrow screen: scaled to fit and live, with an enlarged view on a tap
  if (cfg.zoom) {
    const fig = page.locator('#step-open .pd-player');
    await fig.scrollIntoViewIfNeeded();
    await page.waitForTimeout(500);
    const btn = fig.locator('.pd-zoom');
    check(await btn.isVisible(), `${tag}: the scaled desktop demo offers an Enlarge button`);
    const fs0 = await fig.evaluate((f) => parseFloat(getComputedStyle(f.querySelector('.pd-screen')).fontSize) * (f.querySelector('.pd-window').getBoundingClientRect().width / f.querySelector('.pd-window').offsetWidth));
    check(fs0 > 2.5, `${tag}: scaled terminal text is ${fs0.toFixed(1)}px effective`, String(fs0));
    check((await fig.evaluate((f) => f.querySelector('.pd-window').getBoundingClientRect().width <= f.getBoundingClientRect().width + 1)), `${tag}: the scaled terminal fits the screen width`);
    await fig.screenshot({ path: path.join(outDir, `${tag}-demo-scaled.png`) });
    await btn.click();
    await page.waitForTimeout(400);
    const z = await page.evaluate(() => { const st = document.querySelector('.pd-zoomed .pd-stage'); if (!st) return null; const r = st.getBoundingClientRect(); const fs = parseFloat(getComputedStyle(st.querySelector('.pd-screen')).fontSize); return { w: Math.round(r.width), h: Math.round(r.height), fs, role: st.getAttribute('role'), scrolls: st.scrollWidth > st.clientWidth, focus: document.activeElement.className }; });
    check(!!z && z.fs >= 11 && z.scrolls && z.role === 'dialog', `${tag}: tap opens a full-screen scrollable view at readable size`, JSON.stringify(z));
    check(!!z && z.focus.includes('pd-zoom-close'), `${tag}: focus moves to the Close button`, JSON.stringify(z));
    await page.screenshot({ path: path.join(outDir, `${tag}-demo-enlarged.png`) });
    await page.keyboard.press('Escape');
    await page.waitForTimeout(300);
    check((await page.locator('.pd-zoomed').count()) === 0, `${tag}: Esc closes the enlarged view`);
    await btn.click(); await page.waitForTimeout(200);
    await page.locator('.pd-zoomed .pd-zoom-close').click(); await page.waitForTimeout(200);
    check((await page.locator('.pd-zoomed').count()) === 0, `${tag}: the Close button closes the enlarged view`);
  }

  // the wide layout: each step shows its recording, the heading lines up with the stage, pause persists
  if (cfg.steps) {
    for (let i = 0; i < STEP_RECS.length; i++) {
      await page.evaluate((k) => { const el = paddockDemo.steps[k].el; window.scrollTo({ top: el.getBoundingClientRect().top + pageYOffset - parseFloat(getComputedStyle(document.querySelector('.sc-col')).top), behavior: 'instant' }); }, i);
      await page.waitForTimeout(900);
      const st = await page.evaluate(() => ({ cur: paddockDemo.active(), on: [...document.querySelectorAll('.sc-stage > .pd-player.on')].map((f) => f.getAttribute('data-frames')), was: document.querySelectorAll('.sc-stage > .pd-player.was').length }));
      check(st.cur === i && st.on.length === 1 && st.on[0] === STEP_RECS[i] && st.was === 0, `${tag}: step ${i + 1} shows ${STEP_RECS[i]}`, JSON.stringify(st));
      await page.screenshot({ path: path.join(outDir, `${tag}-step${i + 1}-${STEP_RECS[i]}.png`) });
    }
    // progress fill reaches 100% and loops, on every step's clip
    await page.evaluate(() => window.scrollTo({ top: 0, behavior: 'instant' }));
    // pause persists: pause on step 3, go to step 4, it is still paused; play resumes
    await page.evaluate(() => { const el = paddockDemo.steps[2].el; window.scrollTo({ top: el.getBoundingClientRect().top + pageYOffset - parseFloat(getComputedStyle(document.querySelector('.sc-col')).top), behavior: 'instant' }); });
    await page.waitForTimeout(900);
    await page.locator('.sc-stage > .pd-player.on .pd-play').click();
    await page.evaluate(() => { const el = paddockDemo.steps[3].el; window.scrollTo({ top: el.getBoundingClientRect().top + pageYOffset - parseFloat(getComputedStyle(document.querySelector('.sc-col')).top), behavior: 'instant' }); });
    await page.waitForTimeout(900);
    const fillAt = () => page.evaluate(() => getComputedStyle(document.querySelector('.sc-stage > .pd-player.on .tl .fill')).transform);
    const f1 = await fillAt(); await page.waitForTimeout(900); const f2 = await fillAt();
    const pr = await page.getAttribute('.sc-stage > .pd-player.on .pd-play', 'aria-pressed');
    check(pr === 'false' && f1 === f2, `${tag}: a paused demo stays paused when the next step activates`, `pressed=${pr} fill ${f1} -> ${f2}`);
    await page.locator('.sc-stage > .pd-player.on .pd-play').click();
    await page.waitForTimeout(700);
    const f3 = await fillAt();
    check(f3 !== f2 && (await page.getAttribute('.sc-stage > .pd-player.on .pd-play', 'aria-pressed')) === 'true', `${tag}: pressing play resumes it`, `${f2} -> ${f3}`);
    // past the last step: the sticky stage scrolls away with the section and never covers the line after it
    await page.evaluate(() => window.scrollTo({ top: document.querySelector('.sc-end').getBoundingClientRect().top + pageYOffset - innerHeight * 0.5, behavior: 'instant' }));
    await page.waitForTimeout(500);
    const ov = await page.evaluate(() => ({ stage: document.querySelector('.sc-col').getBoundingClientRect().bottom, end: document.querySelector('.sc-end').getBoundingClientRect().top }));
    check(ov.stage <= ov.end + 1, `${tag}: the stage does not overlap the note after the last step`, JSON.stringify(ov));
    await page.screenshot({ path: path.join(outDir, `${tag}-past-last.png`) });
  }
  await ctx.close();
}

async function noJsShot(browser, base) {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, javaScriptEnabled: false });
  const page = await ctx.newPage();
  const log = instrument(page, new URL(base).origin);
  await page.goto(base, { waitUntil: 'load' });
  const panels = await page.locator('.tabpanel').evaluateAll((els) => els.map((e) => e.getBoundingClientRect().height > 0));
  check(panels.length === 2 && panels.every(Boolean), 'no-js: both install panels are visible');
  check(await page.locator('.tabs').isHidden(), 'no-js: tablist hidden');
  for (const sel of ['.pd-window', '.pd-controls', '.pd-cap', '.pd-player', '.copy', '.sc-col']) {
    const vis = await page.locator(sel).evaluateAll((els) => els.filter((e) => e.getBoundingClientRect().height > 0).length);
    check(vis === 0, `no-js: ${sel} hidden`, `${vis} visible`);
  }
  const posters = await page.locator('.step-fig img').evaluateAll((els) => els.filter((e) => e.naturalWidth > 0 && e.getBoundingClientRect().height > 0).map((e) => e.getAttribute('src')));
  check(posters.length === 8 && new Set(posters).size === 8, 'no-js: all eight steps show their own poster', JSON.stringify(posters));
  check(log.errors.length === 0 && log.external.length === 0 && log.failed.length === 0, 'no-js: no errors, failed or external requests', log.errors.concat(log.external, log.failed).join(' | '));
  await page.screenshot({ path: path.join(outDir, 'nojs-desktop-full.png'), fullPage: true });
  await ctx.close();
}

// ---- behaviour: skip link, tabs, copy. Runs on http and on file:// ----
async function behaviour(browser, url, label, { grantClipboard, blockStorage }) {
  const isHttp = url.startsWith('http');
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 900 }, colorScheme: 'dark' });
  if (grantClipboard) await ctx.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: new URL(url).origin });
  if (blockStorage) {
    await ctx.addInitScript(() => {
      Object.defineProperty(window, 'localStorage', { get() { throw new DOMException('denied', 'SecurityError'); } });
    });
  }
  const page = await ctx.newPage();
  const log = instrument(page, isHttp ? new URL(url).origin : null);
  await page.goto(url, { waitUntil: 'load' });
  const L = (s) => `${label}: ${s}`;

  // the .js class script sits before the stylesheet, so there is no flash of the no-JS layout
  const order = await page.evaluate(() => {
    const kids = [...document.head.children];
    const inline = kids.findIndex((k) => k.tagName === 'SCRIPT' && !k.src);
    const css = kids.findIndex((k) => k.tagName === 'LINK' && k.rel === 'stylesheet');
    return { inline, css };
  });
  check(order.inline >= 0 && order.inline < order.css, L('inline .js script precedes the stylesheet'), JSON.stringify(order));
  check(await page.evaluate(() => document.documentElement.classList.contains('js')), L('html gets the js class'));
  check((await page.evaluate(() => getComputedStyle(document.body).backgroundColor)) === 'rgb(24, 24, 24)', L('dark page'));
  // skip link target takes focus
  await page.focus('.skip'); await page.keyboard.press('Enter');
  check((await page.evaluate(() => document.activeElement.id)) === 'main', L('skip link moves focus to main'));

  // tabs
  const sel = (id) => page.getAttribute(`#${id}`, 'aria-selected');
  await page.click('#tab-brew');
  check((await sel('tab-brew')) === 'true' && (await sel('tab-tpm')) === 'false', L('clicking Homebrew selects it'));
  check((await page.isVisible('#panel-brew')) && !(await page.isVisible('#panel-tpm')), L('only the Homebrew panel shows'));
  check((await page.textContent('#code-brew-sh')) === 'brew install plopezlpz/tap/paddock', L('Homebrew snippet verbatim'));
  await page.focus('#tab-brew');
  await page.keyboard.press('ArrowRight');
  check((await sel('tab-tpm')) === 'true' && (await page.evaluate(() => document.activeElement.id)) === 'tab-tpm', L('ArrowRight wraps to TPM and focuses it'));
  check((await page.$('#tab-cargo')) === null && (await page.$('#panel-cargo')) === null, L('no cargo install tab'));
  await page.keyboard.press('Home');
  check((await sel('tab-tpm')) === 'true', L('Home selects the first tab'));
  await page.keyboard.press('ArrowLeft');
  check((await sel('tab-brew')) === 'true', L('ArrowLeft wraps to the last tab'));
  await page.click('#tab-tpm');

  // copy
  const tpmText = await page.textContent('#code-tpm');
  check(/@paddock-key 'a'/.test(tpmText) && /prefix a opens paddock/.test(tpmText), L('the install snippet uses key a'), tpmText);
  await page.click('[data-copy=code-tpm]');
  await page.waitForTimeout(150);
  const btnText = (await page.textContent('[data-copy=code-tpm]')).trim();
  if (grantClipboard) {
    const clip = await page.evaluate(() => navigator.clipboard.readText());
    check(clip === tpmText, L('copy puts the snippet on the clipboard'), JSON.stringify(clip));
    check(btnText === 'copied', L('copy button says copied'), btnText);
  } else {
    check(btnText === 'copied' || /^press /.test(btnText), L('copy gives feedback (copied or manual hint)'), btnText);
  }
  await page.waitForTimeout(2400);
  check((await page.textContent('[data-copy=code-tpm]')).trim() === 'copy', L('copy button label resets'));

  // denied clipboard: stub the API and execCommand to fail
  await page.evaluate(() => {
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: () => Promise.reject(new DOMException('denied', 'NotAllowedError')) } });
    document.execCommand = () => false;
  });
  await page.click('[data-copy=code-tpm]');
  await page.waitForTimeout(200);
  const manual = (await page.textContent('[data-copy=code-tpm]')).trim();
  const selected = await page.evaluate(() => String(getSelection()));
  check(/^press (⌘C|Ctrl\+C)$/.test(manual), L('denied clipboard shows the manual-copy hint'), manual);
  check(selected.trim() === tpmText.trim(), L('denied clipboard leaves the snippet selected'), JSON.stringify(selected));

  if (!isHttp) {
    await page.waitForTimeout(500);
    const lens = await page.evaluate(() => [...document.querySelectorAll('.pd-player[data-frames]')].map((p) => (p.querySelector('.pd-screen')?.textContent || '').length));
    check(lens.length > 0 && lens.every((n) => n > 0), L('players render frames from file://'), JSON.stringify(lens));
  }
  check(log.errors.length === 0, L('no console errors'), log.errors.join(' | '));
  check(log.external.length === 0, L('no external requests'), log.external.join(' | '));
  await ctx.close();
}

// ---- main ----
let base = arg('url');
try {
  if (!base) base = await startServer();
  if (!base.endsWith('/')) base += '/';
  console.log('serving', base);
  for (const f of needed) check(onDisk(f), `${f} exists`);

  const sizes = ['index.html', 'assets/css/site.css', 'assets/js/site.js'].map((f) => fs.statSync(path.join(siteRoot, f)).size);
  const total = sizes.reduce((x, y) => x + y, 0);
  check(total <= 100 * 1024, `shell budget: html+css+js = ${(total / 1024).toFixed(1)} KB (<= 100 KB)`);
  const html = fs.readFileSync(path.join(siteRoot, 'index.html'), 'utf8');
  check(![...html.matchAll(/<img\b[^>]*>/g)].some((m) => !/\balt=/.test(m[0])), 'index.html source: every <img> (including <noscript>) has alt');
  check(!/(https?:)?\/\/[^"' ]+\.(js|css|woff2?)\b/.test(html.replace(/https:\/\/github\.com[^"']*/g, '')), 'index.html: no third-party script, style or font URLs');
  const recs = ['status', 'tour', 'phone', 'feat-preview', 'feat-rename', 'feat-manage', 'feat-group', 'feat-notify'];
  check(new RegExp(recs.map((r) => `<script defer src="assets/frames/${r}\\.js"></script>\\s*`).join('') + '<script defer src="assets/js/player\\.js"></script>\\s*<script defer src="assets/js/site\\.js"></script>').test(html), 'script order: frames, player.js, site.js');
  for (const f of ['index.html', ...fs.readdirSync(path.join(siteRoot, 'assets/css')).map((x) => 'assets/css/' + x), ...fs.readdirSync(path.join(siteRoot, 'assets/js')).map((x) => 'assets/js/' + x)]) {
    const t = fs.readFileSync(path.join(siteRoot, f), 'utf8');
    check(!/design\//.test(t), `${f}: no reference to design/`);
    check(!/(src|href)="\.\.\//.test(t) && !/url\(["']?\.\.\/(?!fonts\/)/.test(t), `${f}: no path leaves the site (../)`);
  }
  check(/<div class="strip">[^]*?<a href="https:\/\/github\.com\/plopezlpz\/paddock">[^]*?Star[^]*?on GitHub<\/a>/.test(html), 'top strip: a plain "Star on GitHub" link');
  check(!/api\.github\.com|shields\.io|img\.shields/.test(html), 'index.html: no star-count or badge services');
  const rows = [...html.matchAll(/<tr[^>]*>\s*<th scope="row">([^<]*)/g)].map((m) => m[1].trim());
  check(rows.includes('Direct attach to one agent') && !rows.includes('Get to one agent'), 'comparison table: row text matches herdr.dev/compare');
  check(/<tr data-note data-ours><th scope="row">Your config, keys and plugins <span class="ours">added by paddock<\/span>/.test(html) && /The last row is ours, not from herdr's table/.test(html), 'comparison table: the row that is paddock\'s own is marked');
  const text = html.replace(/<script[\s\S]*?<\/script>|<svg[\s\S]*?<\/svg>|<[^>]+>/g, ' ');
  check(!/producer/i.test(html), 'index.html never says "producer"');
  check(!/\bprefix C\b|<kbd>C<\/kbd>/.test(html) && /prefix a\b/.test(text), 'index.html uses prefix a');
  check(!/herd(ing|\b)/i.test(html), 'index.html avoids the h-word');
  for (const f of ['assets/fonts/JetBrainsMono-OFL.txt']) check(fs.existsSync(path.join(siteRoot, f)), `${f} ships with the fonts`);
  check(/<link rel="icon"[^>]*favicon\.svg/.test(html) && /apple-touch-icon/.test(html), 'index.html: favicon and apple-touch-icon');
  const fav = fs.readFileSync(path.join(siteRoot, 'assets/brand/favicon.svg'), 'utf8');
  check(/#a2a970/.test(fav) && !/#1f7a4d|#6fd08c/.test(fav), 'favicon.svg uses the olive accent');

  const browser = await chromium.launch();
  try {
    for (const cfg of CONFIGS) await shootConfig(browser, base, cfg);
    await noJsShot(browser, base);
    await behaviour(browser, base, 'http', { grantClipboard: true, blockStorage: false });
    const fileUrl = pathToFileURL(path.join(siteRoot, 'index.html')).href;
    await behaviour(browser, fileUrl, 'file://', { grantClipboard: false, blockStorage: false });
    await behaviour(browser, fileUrl, 'file:// (storage blocked)', { grantClipboard: false, blockStorage: true });
  } finally {
    await browser.close();
  }
} finally {
  stopServer();
}

const fails = results.filter((r) => r.level === 'FAIL');
const pend = results.filter((r) => r.level === 'warn');
console.log(`\n${results.filter((r) => r.level === 'ok').length} ok, ${fails.length} FAIL, ${pend.length} warn`);
console.log('screenshots:', outDir);
process.exit(fails.length ? 1 : 0);

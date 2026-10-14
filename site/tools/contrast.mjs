// WCAG contrast check for the site tokens (the .page block of assets/css/site.css) and the terminal palette
// (--t-* in assets/css/player.css, overridden by site.css, if present). Exit code 1 if any enforced pair is below its threshold.
//   node contrast.mjs
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// Mini CSS walker: returns [{sel: 'a b', decls: {'--x': '#fff'}}] with @media preludes joined into the selector.
function blocks(css) {
  css = css.replace(/\/\*[\s\S]*?\*\//g, '');
  const out = [];
  const stack = [];
  const re = /([^{}]*)\{|\}|([^{};]+);/g;
  let m;
  while ((m = re.exec(css))) {
    if (m[1] !== undefined) { stack.push({ sel: m[1].trim(), decls: {} }); }
    else if (m[0] === '}') { const b = stack.pop(); if (Object.keys(b.decls).length) out.push({ sel: [...stack.map((s) => s.sel), b.sel].join(' '), decls: b.decls }); }
    else if (stack.length) {
      const i = m[2].indexOf(':');
      if (i > 0) stack[stack.length - 1].decls[m[2].slice(0, i).trim()] = m[2].slice(i + 1).trim();
    }
  }
  return out;
}
const themeOf = (sel) => (/dark/.test(sel) ? 'dark' : /light/.test(sel) ? 'light' : sel.trim().endsWith(':root') || sel.trim() === ':root' ? 'light' : null);
function tokens(file, prefix) {
  const t = { light: {}, dark: {}, darkSources: [] };
  for (const b of blocks(fs.readFileSync(file, 'utf8'))) {
    const th = themeOf(b.sel);
    if (!th) continue;
    const vars = Object.fromEntries(Object.entries(b.decls).filter(([k]) => k.startsWith(prefix)));
    if (!Object.keys(vars).length) continue;
    Object.assign(t[th], vars);
    if (th === 'dark') t.darkSources.push({ sel: b.sel, vars });
  }
  return t;
}

const hex = (h) => { h = h.replace('#', ''); return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16) / 255); };
const lum = (h) => { const [r, g, b] = hex(h).map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)); return 0.2126 * r + 0.7152 * g + 0.0722 * b; };
const ratio = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
const mix = (a, b, pa) => '#' + hex(a).map((c, i) => Math.round((c * pa + hex(b)[i] * (1 - pa)) * 255).toString(16).padStart(2, '0')).join('');

let fails = 0;
const row = (ok, label, r, min, fg, bg) => {
  if (!ok) fails++;
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${r.toFixed(2).padStart(5)} (>= ${min})  ${label}  ${fg} on ${bg}`);
};
// a deliberate choice of the Gruber theme: enforced at the floor, reported when it is under the usual target
const soft = (label, r, target, floor, fg, bg) => {
  const ok = r >= floor;
  if (!ok) fails++;
  console.log(`${ok ? (r >= target ? 'ok  ' : 'note') : 'FAIL'} ${r.toFixed(2).padStart(5)} (floor ${floor}, target ${target})  ${label}  ${fg} on ${bg}`);
};

// ---- site tokens ----
// The site is dark only: its tokens are the .page block of site.css.
const siteCss = path.join(root, 'assets/css/site.css');
const pageBlock = blocks(fs.readFileSync(siteCss, 'utf8')).find((b) => b.sel.trim() === '.page' && b.decls['--bg']);
if (!pageBlock) { console.log('FAIL no .page token block in assets/css/site.css'); process.exit(1); }
const t = pageBlock.decls;
console.log('== site tokens (dark) ==');
const text = ['--ink', '--muted', '--green', '--link', '--hl', '--st-asking', '--st-working', '--st-finished', '--st-idle'];
for (const bg of ['--bg', '--panel', '--sheet']) {
  for (const fg of text) {
    // the asking red is only ever a dot or a legend marker (graphics, 3:1), never body text
    if (fg === '--st-asking') row(ratio(t[fg], t[bg]) >= 3, `${fg} on ${bg} (marker, non-text)`, ratio(t[fg], t[bg]), 3, t[fg], t[bg]);
    else row(ratio(t[fg], t[bg]) >= 4.5, `${fg} on ${bg}`, ratio(t[fg], t[bg]), 4.5, t[fg], t[bg]);
  }
}
row(ratio(t['--bg'], t['--green']) >= 4.5, 'tab / badge text: --bg on --green', ratio(t['--bg'], t['--green']), 4.5, t['--bg'], t['--green']);
row(ratio(t['--green'], t['--bg']) >= 3, 'focus ring: --green on --bg (non-text)', ratio(t['--green'], t['--bg']), 3, t['--green'], t['--bg']);
const e = ratio(t['--edge'], t['--bg']);
console.log(`info ${e.toFixed(2).padStart(5)}  --edge on --bg (decorative control borders; every control also carries a text label)`);

// ---- terminal palette ----
const pcss = path.join(root, 'assets/css/player.css');
if (!fs.existsSync(pcss)) {
  console.log('\nSKIPPED terminal palette: assets/css/player.css not present yet');
} else {
  // The terminal is always dark. player.css defines the palette (:root, repeated under html[data-theme=dark], which must agree);
  // the site's .page block overrides it, and that is the palette that shows.
  const defs = blocks(fs.readFileSync(pcss, 'utf8'))
    .map((b) => ({ sel: b.sel, vars: Object.fromEntries(Object.entries(b.decls).filter(([k]) => k.startsWith('--t-'))) }))
    .filter((b) => Object.keys(b.vars).length);
  const base = Object.assign({}, ...defs.map((d) => d.vars));
  for (const d of defs) {
    const bad = Object.keys(d.vars).filter((k) => d.vars[k] !== base[k]);
    if (bad.length) { fails++; console.log(`FAIL --t-* values differ between blocks ("${d.sel}": ${bad.join(', ')})`); }
    else console.log(`ok   "${d.sel}" agrees with the player.css palette`);
  }
  const t = Object.assign({}, base, Object.fromEntries(Object.entries(pageBlock.decls).filter(([k]) => k.startsWith('--t-'))));
  console.log('\n== terminal palette (always dark) ==');
  const bg = t['--t-bg'];
  if (!bg) { fails++; console.log('FAIL --t-bg missing'); }
  else {
    const names = ['--t-fg', ...Array.from({ length: 16 }, (_, i) => `--t-${i}`)];
    for (const n of names) {
      if (!t[n]) { fails++; console.log(`FAIL ${n} missing`); continue; }
      // colour 0 is "black": a background-only fill (boxes, bars), never text on the screen background
      if (n === '--t-0') { console.log(`skip ${ratio(t[n], bg).toFixed(2).padStart(5)}  ${n} on --t-bg (background-only fill, exempt)`); continue; }
      // slot 8 (bright black) is lifted from the Ghostty value to #7e7e7e so dim text stays readable: floor 4:1
      if (n === '--t-8') soft(`${n} on --t-bg`, ratio(t[n], bg), 4.5, 4, t[n], bg);
      else row(ratio(t[n], bg) >= 4.5, `${n} on --t-bg`, ratio(t[n], bg), 4.5, t[n], bg);
    }
    // dim text: the colour mixed toward the bg by --t-dim (player.css, falls back to 62%); target >= 3:1
    const pct = parseFloat((t['--t-dim'] || '62%')) / 100;
    for (const n of names) {
      if (!t[n] || n === '--t-0') continue;
      const dim = mix(t[n], bg, pct);
      // dim text is secondary (hints, paths); the theme's 50% dim is deliberate: floor 2:1, reported under 3:1
      soft(`dim ${n} (${Math.round(pct * 100)}%) on --t-bg`, ratio(dim, bg), 3, 2, dim, bg);
    }
  }
}

console.log(fails ? `\n${fails} FAIL` : '\nall enforced pairs pass');
process.exit(fails ? 1 : 0);

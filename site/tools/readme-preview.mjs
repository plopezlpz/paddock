// Renders the paddock README the way GitHub does (gh api markdown, mode=gfm),
// maps the rewritten image URLs back to local files, and screenshots it in a
// GitHub-like light and dark wrapper.
// Usage: node tools/readme-preview.mjs [--readme <path>] [--width 1012]
// Output: tools/.out/readme.html, tools/.out/readme-{light,dark}.png
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { chromium } from 'playwright';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(process.env.PADDOCK_REPO || resolve(HERE, '..', '..'));   // the paddock repo: this folder's parent when it is the repo's site/
const OUT = resolve(HERE, '.out');
const flag = (name, def) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : def;
};
const readme = flag('--readme', `${REPO}/README.md`);
const width = Number(flag('--width', 1012));

mkdirSync(OUT, { recursive: true });

const html = execFileSync(
  'gh',
  ['api', '-X', 'POST', 'markdown', '-f', 'mode=gfm', '-f', 'context=plopezlpz/paddock', '-F', `text=@${readme}`],
  { encoding: 'utf8', maxBuffer: 16 << 20 },
);

// The API leaves relative paths alone (<base> resolves them below); the repo
// page rewrites them to https://github.com/<repo>/(raw|blob)/<ref>/<path>[?raw=true],
// so map that form back to a file too.
const missing = new Set();
for (const m of html.matchAll(/(?:src|srcset)="(docs\/[^"]+)"/g)) {
  if (!existsSync(`${REPO}/${m[1]}`)) missing.add(m[1]);
}
const local = html.replace(
  /https:\/\/github\.com\/plopezlpz\/paddock\/(?:raw|blob)\/[^/"]+\/(docs\/[^"?]+)(?:\?raw=true)?/g,
  (_, path) => {
    const file = `${REPO}/${path}`;
    if (!existsSync(file)) missing.add(path);
    return pathToFileURL(file).href;
  },
);

// What must survive the sanitiser.
const checks = {
  'h1 align=center': /<h1[^>]*align="center"/.test(html),
  '<picture>': (html.match(/<picture/g) || []).length,
  '<source prefers-color-scheme>': (html.match(/<source[^>]*prefers-color-scheme/g) || []).length,
  '<details open>': /<details[^>]*\bopen\b/.test(html),
  '<summary>': /<summary/.test(html),
  '<sub>': /<sub>/.test(html),
  'p align=center': (html.match(/<p[^>]*align="center"/g) || []).length,
};

const css = `
:root { color-scheme: light dark; --fg:#1f2328; --bg:#fff; --muted:#59636e; --border:#d1d9e0; --code:#818b981f; --link:#0969da; --quote:#59636e; }
@media (prefers-color-scheme: dark) { :root { --fg:#f0f6fc; --bg:#0d1117; --muted:#9198a1; --border:#3d444d; --code:#656c7633; --link:#4493f8; --quote:#9198a1; } }
body { margin:0; background:var(--bg); }
.markdown-body { box-sizing:border-box; max-width:${width}px; margin:0 auto; padding:32px 32px;
  font:16px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI","Noto Sans",Helvetica,Arial,sans-serif; color:var(--fg); }
.markdown-body a { color:var(--link); text-decoration:none; }
.markdown-body h1, .markdown-body h2 { padding-bottom:.3em; border-bottom:1px solid var(--border); }
.markdown-body h1 { font-size:2em; margin:.67em 0; }
.markdown-body h2 { font-size:1.5em; margin:24px 0 16px; }
.markdown-body h3 { font-size:1.25em; margin:24px 0 16px; }
.markdown-body h1[align=center] { border-bottom:0; text-align:center; }
.markdown-body img, .markdown-body picture { max-width:100%; }
.markdown-body code, .markdown-body tt { background:var(--code); border-radius:6px; padding:.2em .4em; font:85% ui-monospace,SFMono-Regular,"SF Mono",Menlo,monospace; }
.markdown-body pre { background:var(--code); border-radius:6px; padding:16px; overflow:auto; font-size:85%; line-height:1.45; }
.markdown-body pre code { background:none; padding:0; font-size:100%; }
.markdown-body blockquote { margin:0 0 16px; padding:0 1em; color:var(--quote); border-left:.25em solid var(--border); }
.markdown-body table { border-collapse:collapse; display:block; width:max-content; max-width:100%; overflow:auto; margin:0 0 16px; }
.markdown-body th, .markdown-body td { padding:6px 13px; border:1px solid var(--border); }
.markdown-body tr:nth-child(2n) { background:var(--code); }
.markdown-body details > summary { cursor:pointer; }
.markdown-body .anchor { display:none; }
`;
const page = `<!doctype html><meta charset="utf-8"><base href="${pathToFileURL(REPO).href}/"><title>README preview</title><style>${css}</style><article class="markdown-body">${local}</article>`;
const file = `${OUT}/readme.html`;
writeFileSync(file, page);

const browser = await chromium.launch();
const shots = [];
const broken = new Set();
for (const scheme of ['light', 'dark']) {
  const ctx = await browser.newContext({ colorScheme: scheme, viewport: { width: width + 40, height: 900 } });
  const p = await ctx.newPage();
  await p.goto(pathToFileURL(file).href);
  await p.waitForLoadState('load');
  const bad = await p.$$eval('img', (imgs) => imgs.filter((i) => !i.complete || i.naturalWidth === 0).map((i) => i.getAttribute('src')));
  for (const b of bad) broken.add(b);
  const centred = await p.$eval('h1[align=center]', (h) => getComputedStyle(h).textAlign).catch(() => null);
  const open = await p.$eval('details', (d) => d.open).catch(() => null);
  if (scheme === 'light') Object.assign(checks, { 'h1 computed text-align': centred, 'details.open': open });
  const png = `${OUT}/readme-${scheme}.png`;
  await p.screenshot({ path: png, fullPage: true });
  shots.push(png);
  await ctx.close();
}
await browser.close();

console.log('survived the GitHub sanitiser:');
for (const [k, v] of Object.entries(checks)) console.log(`  ${k}: ${v}`);
console.log(`images not found locally: ${missing.size ? [...missing].join(', ') : 'none'}`);
console.log(`images that failed to load: ${broken.size ? [...broken].join(', ') : 'none'}`);
console.log(`html: ${file}`);
for (const s of shots) console.log(`screenshot: ${s}`);

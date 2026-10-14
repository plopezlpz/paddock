// Brand rasteriser. Run from anywhere: node tools/brand/rasterize.mjs [--png] [--sheet]
//   (no flag = --png)  --png    writes assets/brand/favicon-32.png and apple-touch-icon.png
//                      --sheet  writes tools/.out/brand-sheet@{1,2}x.png (contact sheet)
import { chromium } from 'playwright';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const brand = path.resolve(here, '../../assets/brand');
const out = path.resolve(here, '../.out');
const read = (f) => fs.readFileSync(path.join(brand, f), 'utf8').trim();
const args = new Set(process.argv.slice(2));
const doSheet = args.has('--sheet');
const doPng = args.has('--png') || !doSheet;

const PAPER = '#f5f5f0';
const browser = await chromium.launch();

async function shoot(html, { w, h, dpr = 1, file, omitBackground = false, scheme = 'light' }) {
  const ctx = await browser.newContext({ viewport: { width: w, height: h }, deviceScaleFactor: dpr, colorScheme: scheme });
  const page = await ctx.newPage();
  await page.setContent(html);
  await page.screenshot({ path: file, omitBackground, clip: { x: 0, y: 0, width: w, height: h } });
  await ctx.close();
}

if (doPng) {
  const base = 'html,body{margin:0;background:transparent}svg{display:block}';
  // favicon-32.png: favicon geometry on a paper rounded square, so it shows on dark tab bars
  const fav = read('favicon.svg').replace('<svg ', '<svg width="32" height="32" ');
  await shoot(
    `<style>${base}div{width:32px;height:32px;border-radius:7px;background:${PAPER};display:grid;place-items:center}</style><div>${fav}</div>`,
    { w: 32, h: 32, file: path.join(brand, 'favicon-32.png'), omitBackground: true });
  // apple-touch-icon.png: 180x180, paper, mark at 70% (iOS rounds the corners itself)
  const mark = read('mark.svg').replace('<svg ', '<svg width="126" height="126" ')
    .replace('currentColor', '#141714').replace('var(--green,#a2a970)', '#a2a970');
  await shoot(
    `<style>${base}div{width:180px;height:180px;background:${PAPER};display:grid;place-items:center}</style><div>${mark}</div>`,
    { w: 180, h: 180, file: path.join(brand, 'apple-touch-icon.png') });
  console.log('wrote favicon-32.png, apple-touch-icon.png');
}

if (doSheet) {
  fs.mkdirSync(out, { recursive: true });
  const url = (f) => pathToFileURL(path.join(brand, f)).href;
  const darkFav = read('favicon.svg').replace(/<style>[\s\S]*?<\/style>/,
    '<style>path{stroke:#e4e4e4}circle{fill:#a2a970}</style>');
  fs.writeFileSync(path.join(out, 'favicon-dark.svg'), darkFav);
  const subs = {
    mark: read('mark.svg'), logo: read('logo.svg'),
    'favicon.svg': url('favicon.svg'), 'favicon-dark.svg': pathToFileURL(path.join(out, 'favicon-dark.svg')).href,
    'logo-light.svg': url('logo-light.svg'), 'logo-dark.svg': url('logo-dark.svg'),
    'favicon-32.png': url('favicon-32.png'), 'apple-touch-icon.png': url('apple-touch-icon.png'),
  };
  const html = fs.readFileSync(path.join(here, 'sheet.html'), 'utf8').replace(/\{\{([^}]+)\}\}/g, (_, k) => {
    if (!(k in subs)) throw new Error('sheet.html: unknown placeholder ' + k);
    return subs[k];
  });
  // the page loads images by file:// URL, so it has to be a file, not setContent
  const built = path.join(out, 'brand-sheet.html');
  fs.writeFileSync(built, html);
  for (const dpr of [1, 2]) {
    const ctx = await browser.newContext({ viewport: { width: 1100, height: 800 }, deviceScaleFactor: dpr, colorScheme: 'light' });
    const page = await ctx.newPage();
    await page.goto(pathToFileURL(built).href);
    await page.screenshot({ path: path.join(out, `brand-sheet@${dpr}x.png`), fullPage: true });
    await ctx.close();
  }
  console.log('wrote', path.join(out, 'brand-sheet@{1,2}x.png'));
}
await browser.close();

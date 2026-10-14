// Animated SVG demo, static phone frame, feature clips and posters from the recorded frames.
//   node tools/build-svg.mjs                  write everything
//   node tools/build-svg.mjs --pace 30        tour loop length in seconds (default 30), or --pace off for the recorded holds
//   node tools/build-svg.mjs --paddock <repo> the paddock repo the README images go to (default: this
//                                             folder's parent, or PADDOCK_REPO)
//   node tools/build-svg.mjs --only readme    only the README images (docs/brand); --only posters: only the site posters
//   node tools/build-svg.mjs --theme <css>    terminal palette overlay on assets/css/player.css (default assets/css/site.css, the Gruber theme
//                                             the recordings are made in, if it exists; "none" for player.css alone)
// The terminal is always dark, so there is one file per image: it looks the same on a light and a dark page.
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { buildSvg, loadFrames, loadPalette, paceHolds } from './lib/svg.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const site = path.resolve(here, '..');
const argv = process.argv.slice(2);
const arg = (name, def) => {
  const i = argv.indexOf(`--${name}`);
  return i < 0 ? def : argv[i + 1];
};
const pace = arg('pace', '30');
const paceMs = pace === 'off' ? null : Math.round(parseFloat(pace) * 1000);
const paddock = path.resolve(arg('paddock', process.env.PADDOCK_REPO || path.join(site, '..')));
const brand = path.join(paddock, 'docs', 'brand');
const only = arg('only', 'all');
const themeArg = arg('theme', path.join(site, 'assets/css/site.css'));
const theme = themeArg !== 'none' && fs.existsSync(themeArg) ? [themeArg] : [];

const frames = (name) => loadFrames(path.join(site, `assets/frames/${name}.js`));
const tour = frames('tour');
const phone = frames('phone');
const pal = loadPalette(path.join(site, 'assets/css/player.css'), ...theme);
const poster = (data, id) => data.chapters.find((c) => c.id === id).poster;
const all = (data) => data.frames.map((_, i) => i);

const holds = paceHolds(tour.frames, { pace: paceMs });
const tourDesc = `Recorded from paddock ${tour.source.paddock} in a scratch tmux server; the sessions and agent output are staged. `
  + `Steps: ${tour.chapters.map((c, i) => `${i + 1}. ${c.cap}`).join('. ')}.`;
const phoneDesc = `paddock's phone layout on a ${phone.cols} column screen, recorded from paddock ${phone.source.paddock}: the agents tab, grouped by state.`;
const palettes = [{ pal }];

// Feature clips: each recording plays at its recorded pace and loops; the title and desc name what it shows.
const FEATURES = [
  { name: 'status', title: 'paddock: the status dot', what: 'a dot beside each window that has an agent', still: 'finished' },
  { name: 'feat-preview', title: 'paddock: live preview', what: 'the selected pane, live, beside the agents list', still: 'live' },
  { name: 'feat-group', title: 'paddock: grouping', what: 'the agents grouped by status, session and folder', still: 'status' },
  { name: 'feat-notify', title: 'paddock: who is waiting', what: 'a window dot turns red or blue, and the agents tab says who', still: 'open' },
];
const featureJob = ({ name, title, what, still }) => {
  const data = frames(name);
  const steps = data.chapters.map((c, i) => `${i + 1}. ${c.cap}`).join('. ');
  return {
    file: path.join(brand, `${name.replace(/^feat-/, '')}.svg`),
    budget: 700,
    data,
    opts: {
      frames: all(data), holds: paceHolds(data.frames, { pace: null }), reduceTo: poster(data, still), palettes, title,
      desc: `Recorded from paddock ${data.source.paddock} in a scratch tmux server; staged sessions. Shows ${what}. Steps: ${steps}.`,
    },
  };
};

const jobs = [];
if (only === 'all' || only === 'readme') {
  jobs.push(
    { file: path.join(brand, 'demo.svg'), data: tour, opts: { frames: all(tour), holds, reduceTo: poster(tour, 'agents'), palettes, title: 'paddock in tmux', desc: tourDesc } },
    { file: path.join(brand, 'frame-phone.svg'), data: phone, opts: { frames: [poster(phone, 'agents')], chrome: 'phone', palettes, title: 'paddock on a phone', desc: phoneDesc } },
    ...FEATURES.map(featureJob),
  );
}
// Site posters: one still per recording, shown where scripts are off (index.html <noscript>).
const POSTERS = [
  { name: 'status', still: 'finished', title: 'paddock: the status dot', what: 'a dot beside each window that has an agent, here blue because the agent has finished' },
  { name: 'tour', still: 'agents', title: 'paddock in tmux: the agents tab', what: 'paddock open over a tmux window, on the agents tab' },
  { name: 'feat-group', still: 'status', title: 'paddock: grouping', what: 'the agents tab grouped by status' },
  { name: 'feat-preview', still: 'live', title: 'paddock: live preview', what: 'the selected pane, live, beside the agents list' },
  { name: 'feat-rename', still: 'suggest', title: 'paddock: rename', what: 'an agent being renamed, with a suggested title' },
  { name: 'feat-manage', still: 'move', title: 'paddock: sessions, windows and panes', what: 'a window being moved from the popup' },
  { name: 'feat-notify', still: 'open', title: 'paddock: who is waiting', what: 'the agents tab with one agent waiting and one finished' },
];
if (only === 'all' || only === 'posters') {
  for (const { name, still, title, what } of POSTERS) {
    const data = frames(name);
    jobs.push({ file: path.join(site, `assets/frames/${name}-poster.svg`), data, opts: { frames: [poster(data, still)], palettes, title, desc: `Recorded from paddock ${data.source.paddock} in a scratch tmux server; staged sessions. Shows ${what}.` } });
  }
  jobs.push({ file: path.join(site, 'assets/frames/phone-poster.svg'), data: phone, opts: { frames: [poster(phone, 'agents')], chrome: 'phone', palettes, title: 'paddock on a phone: the agents tab', desc: phoneDesc } });
}

if (only === 'all' || only === 'readme') fs.mkdirSync(brand, { recursive: true });   // posters build without the paddock repo
let over = false;
for (const j of jobs) {
  const svg = buildSvg(j.data, j.opts);
  fs.writeFileSync(j.file, svg);
  const raw = Buffer.byteLength(svg), gz = zlib.gzipSync(svg, { level: 9 }).length;
  if (raw > (j.budget || 450) * 1024) over = true;
  console.log(`${String(raw).padStart(7)} raw ${String(gz).padStart(6)} gzip  ${j.file}`);
}
console.log(`loop: ${(holds.reduce((a, b) => a + b, 0) / 1000).toFixed(2)} s over ${holds.length} frames`);
if (over) {
  console.error('FAIL: an SVG is over its size budget (450 KB; 700 KB for a feature clip)');
  process.exit(1);
}

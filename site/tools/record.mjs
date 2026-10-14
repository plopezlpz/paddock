#!/usr/bin/env node
// Records real paddock frames: node tools/record.mjs tour|phone|status|all|feat-<name>|feat [--keep] [--paddock <repo>]
// Needs tmux 3.8+ and a paddock checkout; builds the release binary first.

import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { writeAll } from './lib/frames.mjs';
import { DeadlineError, runScenario } from './lib/scenario.mjs';
import { tmuxBin } from './lib/tmux.mjs';

const toolsDir = path.dirname(fileURLToPath(import.meta.url));
const siteDir = path.dirname(toolsDir);
const NAMES = ['tour', 'phone', 'status'];
const FEATURES = ['feat-preview', 'feat-rename', 'feat-manage', 'feat-group', 'feat-notify'];

function usage(msg) {
  if (msg) console.error(msg);
  console.error('usage: node tools/record.mjs tour|phone|status|all|feat|' + FEATURES.join('|') + ' [--keep] [--paddock <repo>]\n       node tools/record.mjs <scenario> [--conf <tmux.conf>] [--out-dir <dir>] [--name <frames name>]  (--name: one scenario)');
  process.exit(2);
}

const args = process.argv.slice(2);
let keep = false;
let conf;
let outDir;
let frameName;
let repo = process.env.PADDOCK_REPO || path.join(siteDir, '..');   // the paddock repo: this folder's parent when it is the repo's site/
const names = [];
for (let i = 0; i < args.length; i++) {
  if (args[i] === '--keep') keep = true;
  else if (args[i] === '--paddock') repo = args[++i] ?? usage('--paddock needs a path');
  else if (args[i] === '--conf') conf = path.resolve(args[++i] ?? usage('--conf needs a path'));
  else if (args[i] === '--out-dir') outDir = path.resolve(args[++i] ?? usage('--out-dir needs a path'));
  else if (args[i] === '--name') frameName = args[++i] ?? usage('--name needs a name');
  else if (args[i] === 'all') names.push(...NAMES);
  else if (args[i] === 'feat') names.push(...FEATURES);
  else if (NAMES.includes(args[i]) || FEATURES.includes(args[i])) names.push(args[i]);
  else usage('unknown argument ' + args[i]);
}
if (!names.length) usage();
if (frameName && names.length !== 1) usage('--name goes with a single scenario');
if (conf && !fs.existsSync(conf)) usage('no such config: ' + conf);
if (frameName && !/^[a-z][a-z0-9-]*$/.test(frameName)) usage('--name must be lowercase letters, digits and dashes');
repo = path.resolve(repo);
if (!fs.existsSync(path.join(repo, 'Cargo.toml'))) {
  console.error(`no paddock checkout at ${repo}; pass --paddock <repo> or set PADDOCK_REPO`);
  process.exit(2);
}

// Nothing we spawn may see the caller's tmux.
const env = { ...process.env };
delete env.TMUX;
delete env.TMUX_PANE;
const run = (cmd, argv, opts = {}) => spawnSync(cmd, argv, { cwd: repo, env, encoding: 'utf8', ...opts });

console.log(`building paddock in ${repo}`);
const build = run('cargo', ['build', '--release', '--locked'], { stdio: ['ignore', 'inherit', 'inherit'] });
if (build.status !== 0) { console.error('cargo build --release --locked failed'); process.exit(1); }

const cargoToml = fs.readFileSync(path.join(repo, 'Cargo.toml'), 'utf8');
const version = cargoToml.match(/^\[package\][^[]*?^version\s*=\s*"([^"]+)"/ms)?.[1];
if (!version) { console.error('no [package] version in Cargo.toml'); process.exit(1); }
const bin = path.join(repo, 'target/release/paddock');
const reported = run(bin, ['--version']).stdout?.trim();
if (reported !== `paddock ${version}`) {
  console.error(`${bin} reports ${JSON.stringify(reported)}, expected "paddock ${version}": run cargo build --release`);
  process.exit(1);
}
const sha256 = crypto.createHash('sha256').update(fs.readFileSync(bin)).digest('hex');
const commit = run('git', ['rev-parse', '--short', 'HEAD']).stdout.trim();
const dirty = run('git', ['status', '--porcelain', '--', '.', ':(exclude)site']).stdout.trim() !== '';   // site/ (this folder) does not count
const tmuxVersion = spawnSync(tmuxBin(), ['-V'], { env: { PATH: '/usr/bin:/bin' }, encoding: 'utf8' }).stdout.trim().replace(/^tmux /, '');
const info = {
  paddock: version,
  commit: commit + (dirty ? '-dirty' : ''),
  sha256,
  tmux: tmuxVersion,
  recorded: new Date().toLocaleDateString('en-CA'), // local date, YYYY-MM-DD
};
console.log(`paddock ${version} (${info.commit}), tmux ${tmuxVersion}`);

for (const name of names) {
  const def = { ...(await import(`./demo/scenarios/${name}.mjs`)).default };
  if (frameName) def.name = frameName;
  console.log(`\nrecording ${name} (${def.cols}x${def.rows})`);
  let result;
  for (let attempt = 1; ; attempt++) {
    const started = Date.now();
    try {
      result = await runScenario(def, { toolsDir, source: { repo, bin, info: conf ? { ...info, conf: path.basename(conf) } : info }, conf, keep, log: (m) => console.log(m) });
      console.log(`  took ${((Date.now() - started) / 1000).toFixed(1)} s`);
      break;
    } catch (err) {
      if (err instanceof DeadlineError && attempt === 1) { console.log(`  ${err.message}; retrying once`); continue; }
      throw err;
    }
  }
  const { data } = result;
  const bytes = writeAll(data, { framesDir: outDir ?? path.join(siteDir, 'assets/frames'), outDir: path.join(toolsDir, '.out'), playerCss: path.join(siteDir, 'assets/css/player.css') });
  console.log(`text+style fidelity: ${data.frames.length}/${data.frames.length} frames faithful`);
  console.log(`${name}: ${data.frames.length} frames, ${data.lines.length} lines, ${data.styles.length} styles, ${(bytes / 1024).toFixed(1)} KB`);
  console.log('  chapters: ' + data.chapters.map((c) => `${c.id}(frame ${c.frame}, poster ${c.poster})`).join(' '));
}

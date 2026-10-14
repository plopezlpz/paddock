// Runs a scenario against a real paddock in scratch tmux servers and turns
// what the screen showed into a frame set.
//
// Two servers, both named paddock-demo-<runid>[-outer]:
//   inner  holds the fake sessions and runs paddock (bound by paddock.tmux);
//   outer  holds one pane running a client of the inner server, so capturing
//          that pane returns the whole client screen, popup included.
// Keys go to the outer pane. Agent panes run `cat` and are fed canned
// transcripts through their tty.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { parse } from './ansi.mjs';
import { buildSet } from './frames.mjs';
import { Tmux, cleanEnv, socketPath } from './tmux.mjs';
import { GLYPHS, strWidth } from './wcwidth.mjs';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const ESC = '\x1b';
const OUT_PANE = 'outer:0.0';
// The pane command: a tty nobody reads or echoes; transcripts are written to it.
export const PANE_CMD = 'stty -echo; exec cat >/dev/null';
// Agent ages tick over a minute boundary; a scenario must finish before one does.
const AGE_MARGIN_S = 3;

export class DeadlineError extends Error {}

/** A transcript file as bytes: "# " comment lines dropped, \e turned into ESC and \r into CR. */
export function readTranscript(file) {
  const bytes = fs
    .readFileSync(file, 'utf8')
    .split('\n')
    .filter((l) => !l.startsWith('# '))
    .join('\n')
    .replace(/\\e/g, ESC)
    .replace(/\\r/g, '\r');
  const stray = bytes.match(/\\./);
  if (stray) throw new Error(`${file}: unsupported escape ${JSON.stringify(stray[0])} (only \\e and \\r)`);
  return bytes;
}

// How a key is labelled on the HUD.
const KEY_LABEL = { Up: '↑', Down: '↓', Left: '←', Right: '→', Enter: '⏎ Enter', Tab: '⇥ Tab' };

/** Where the SGR-free text of a transcript would wrap or overflow a pane of WIDTH columns. */
export function overflow(bytes, width) {
  return bytes
    .replace(/\x1b\[[0-9;:?]*[A-Za-z]/g, '')
    .split('\n')
    .filter((l) => strWidth(l) > width - 1);
}

class Ctx {
  constructor({ def, tm, home, conf, transcripts, log, rawDir }) {
    Object.assign(this, { def, tm, home, conf, transcripts, log, rawDir });
    this.snaps = [];
    this.chapters = def.chapters.map((c) => ({ ...c }));
    this.chapterIdx = -1;
    this.cap = null;
    this.tapMark = null;
    this.deadline = Infinity;
  }

  inner(...a) { return this.tm.inner(...a); }
  /** Absolute path under the demo home's code directory. */
  dir(rel) { return path.join(this.home, 'code', rel); }

  // ---- agents ---------------------------------------------------------

  /** Narrows the time the scenario may take so no displayed age ticks over. */
  #limit(ageSec) {
    this.deadline = Math.min(this.deadline, Date.now() + (60 - (ageSec % 60) - AGE_MARGIN_S) * 1000);
  }

  /**
   * Sets the pane options an agent's hooks set, then the window's icon (agent-status.tmux's format).
   * LIMIT false: the scenario never shows an age, so none may tick over.
   */
  setAgent(target, { state, unseen = false, kind, title, ageSec = 0, limit = true }) {
    if (limit) this.#limit(ageSec);
    const at = Math.floor(Date.now() / 1000) - ageSec;
    const set = (name, value) => this.inner('set-option', '-p', '-t', target, name, String(value));
    set('@agent-state', state);
    if (unseen) set('@agent-unseen', 1);
    else this.inner('set-option', '-p', '-u', '-t', target, '@agent-unseen');
    if (kind) set('@agent-kind', kind);
    if (title) set('@agent-auto-description', title);
    set('@agent-state-at', at);
    this.updateIcon(this.inner('display-message', '-p', '-t', target, '#{window_id}'));
  }

  /** @agent-icon of WINDOW: the worst state of its panes, as agent-status.tmux formats it. */
  updateIcon(window) {
    const fallback = { working: 'yellow', asking: 'red', finished: 'cyan', idle: 'green' };
    // As agent-status.tmux does: the theme's @agent-color-<state>, else the default.
    const colors = Object.fromEntries(Object.entries(fallback).map(([s, c]) => [s, this.inner('show-option', '-gqv', `@agent-color-${s}`) || c]));
    const rank = { asking: 4, working: 3, finished: 2, idle: 1 };
    let worst = null;
    for (const line of this.inner('list-panes', '-t', window, '-F', '#{@agent-state}\t#{@agent-unseen}').split('\n')) {
      const [state, unseen] = line.split('\t');
      if (!state) continue;
      const s = state === 'idle' && unseen === '1' ? 'finished' : state;
      if (!worst || rank[s] > rank[worst]) worst = s;
    }
    if (!worst) { this.inner('set-option', '-w', '-u', '-t', window, '@agent-icon'); return; }
    const glyph = worst === 'idle' ? '○' : '●';
    this.inner('set-option', '-w', '-t', window, '@agent-icon', `#[push-default]#[fg=${colors[worst]}]${glyph}#[fg=default]#[pop-default]`);
  }

  /** Writes transcripts (names in the transcripts directory) to the pane's tty. */
  writePane(target, files) {
    const names = Array.isArray(files) ? files : [files];
    const bytes = names.map((n) => readTranscript(path.join(this.transcripts, n))).join('');
    const width = Number(this.inner('display-message', '-p', '-t', target, '#{pane_width}'));
    const bad = overflow(bytes, width);
    if (bad.length) throw new Error(`${names.join('+')} would wrap in a ${width}-column pane: ${JSON.stringify(bad[0])}`);
    const tty = this.inner('display-message', '-p', '-t', target, '#{pane_tty}');
    const fd = fs.openSync(tty, fs.constants.O_WRONLY | fs.constants.O_NOCTTY);
    try { fs.writeSync(fd, bytes); } finally { fs.closeSync(fd); }
  }

  // ---- input ----------------------------------------------------------

  async keys(list, gap = 80) {
    for (const k of list) {
      this.tm.outer('send-keys', '-t', OUT_PANE, k);
      await sleep(gap);
    }
  }

  async type(text, perChar = 150) {
    for (const ch of text) {
      this.tm.outer('send-keys', '-t', OUT_PANE, '-l', ch);
      await sleep(perChar);
    }
  }

  /** A tap at the 0-based cell: SGR mouse press and release, as a phone terminal sends them. */
  async tap([col, row]) {
    const hex = (s) => [...Buffer.from(s)].map((b) => b.toString(16).padStart(2, '0'));
    const seq = (end) => hex(`${ESC}[<0;${col + 1};${row + 1}${end}`);
    this.tm.outer('send-keys', '-t', OUT_PANE, '-H', ...seq('M'));
    await sleep(70);
    this.tm.outer('send-keys', '-t', OUT_PANE, '-H', ...seq('m'));
    this.tapMark = [col, row];
  }

  // ---- screen ---------------------------------------------------------

  #capture(args) { return this.tm.outer('capture-pane', '-p', ...args, '-N', '-t', OUT_PANE); }

  /** The screen from a styled capture E; `plain` is derived from the cells, `tmuxPlain` is filled by settle(). */
  #screen(e) {
    const { cols, rows } = this.def;
    const cells = parse(e, cols, rows, { strict: true });
    const plain = cells.map((r) => r.map((c) => c.ch).join(''));
    return { e, cells, plain, text: plain.join('\n'), tmuxPlain: null };
  }

  /** Why PRED does not hold on SCREEN, or null when it does. */
  failing(pred, screen) {
    if (typeof pred === 'string') pred = { text: pred };
    if (pred.text !== undefined) return screen.text.includes(pred.text) ? null : `text ${JSON.stringify(pred.text)} is not on screen`;
    if (pred.notText !== undefined) return screen.text.includes(pred.notText) ? `text ${JSON.stringify(pred.notText)} is still on screen` : null;
    if (pred.cell) {
      const { find, offset = 0, next, ch, fg } = pred.cell;
      const at = findText(screen, find);
      if (!at) return `${JSON.stringify(find)} is not on screen`;
      let col = at.col + (next !== undefined ? find.length : offset);
      // next: the first non-space cell after FIND, whatever the theme puts between them
      if (next !== undefined) while (screen.cells[at.row][col]?.ch === ' ') col++;
      const cell = screen.cells[at.row][col];
      if (!cell) return `no cell at ${find}+${offset}`;
      if (next !== undefined && cell.ch !== next) return `cell after ${find} is ${JSON.stringify(cell.ch)}, not ${JSON.stringify(next)}`;
      if (ch !== undefined && cell.ch !== ch) return `cell ${find}+${offset} is ${JSON.stringify(cell.ch)}, not ${JSON.stringify(ch)}`;
      if (fg !== undefined && cell.st.fg !== fg) return `cell ${find}+${offset} has fg ${cell.st.fg}, not ${fg}`;
      return null;
    }
    if (pred.selected !== undefined) {
      const hit = screen.plain.some((row) => {
        const m = row.indexOf('▌');
        return m >= 0 && row.indexOf(pred.selected, m) > m;
      });
      return hit ? null : `no selected row (▌) contains ${JSON.stringify(pred.selected)}`;
    }
    if (pred.client) {
      const [session, window] = pred.client.window.split(':');
      const now = this.clientWindow();
      return now === `${session}:${window}` ? null : `client is on ${now}, not ${pred.client.window}`;
    }
    throw new Error('unknown predicate ' + JSON.stringify(pred));
  }

  /** session:window of the attached (non-control) client. */
  clientWindow() {
    const rows = this.inner('list-clients', '-F', '#{client_control_mode} #{client_session}:#{window_index}').split('\n');
    return rows.map((r) => r.split(' ')).find(([ctl]) => ctl === '0')?.[1];
  }

  /**
   * Waits until the screen is stable and every predicate holds. Stable: two
   * captures at least 150 ms apart are identical, and the plain capture taken
   * between two more agrees with them. Returns the screen.
   */
  async settle(expect = [], timeout = 6000) {
    const preds = [].concat(expect);
    const stop = Date.now() + timeout;
    let prev = null;
    let since = 0;
    let why = 'never settled';
    for (;;) {
      const e = this.#capture(['-e']);
      const now = Date.now();
      if (e !== prev) { prev = e; since = now; }
      else if (now - since >= 150) {
        const screen = this.#screen(e);
        const failed = preds.map((p) => this.failing(p, screen)).find(Boolean);
        if (!failed) {
          const plain = this.#capture([]);
          const real = plain.split('\n');
          if (real.length === this.def.rows && this.#capture(['-e']) === e && real.map((r) => r.padEnd(this.def.cols)).join('\n') === screen.text) {
            screen.tmuxPlain = real; // tmux's own text, which buildSet checks the cells against
            return screen;
          }
          prev = null;
          why = 'plain capture differs from the styled one';
        } else why = failed;
      }
      if (Date.now() > stop) throw new Error(`screen did not reach the expected state: ${why}\n${prev ? this.#screen(prev).text : ''}`);
      await sleep(40);
    }
  }

  /** Records SCREEN as the next frame. */
  record(screen, { hold, key, cap, poster = false, tap } = {}) {
    if (this.chapterIdx < 0) throw new Error('snap before the first chapter');
    const chapter = this.chapters[this.chapterIdx];
    if (cap !== undefined) this.cap = cap;
    const n = this.snaps.length;
    const mark = tap ?? this.tapMark;
    this.tapMark = null;
    this.snaps.push({
      cells: screen.cells,
      plain: screen.tmuxPlain,
      hold,
      key,
      tap: mark ?? undefined,
      cap: this.cap !== chapter.cap ? this.cap : undefined,
      chapter: this.chapterIdx,
    });
    if (chapter.frame === undefined) chapter.frame = n;
    if (poster) chapter.poster = n;
    fs.mkdirSync(this.rawDir, { recursive: true });
    fs.writeFileSync(path.join(this.rawDir, String(n).padStart(3, '0') + '.ans'), screen.e);
    this.log(`  frame ${n}: ${hold} ms${key ? ` [${key}]` : ''}${poster ? ' (poster)' : ''}`);
    return n;
  }

  async snap(opts = {}) {
    const screen = await this.settle(opts.expect, opts.timeout);
    return this.record(screen, opts);
  }

  /** Presses KEY until PRED holds, recording each state on the way (at most MAX presses). */
  async pressUntil(key, pred, { max = 5, between = 350, ...final }) {
    const label = KEY_LABEL[key] ?? key;
    for (let i = 1; i <= max; i++) {
      const before = this.#capture(['-e']);
      await this.keys([key]);
      // The key must have reached paddock and redrawn before the screen is judged.
      for (let t = 0; t < 40 && this.#capture(['-e']) === before; t++) await sleep(40);
      const screen = await this.settle();
      if (!this.failing(pred, screen)) {
        return this.record(await this.settle(final.expect), { key: label, ...final });
      }
      this.record(screen, { hold: between, key: label });
    }
    throw new Error(`${key} x${max} never reached ${JSON.stringify(pred)}`);
  }

  startChapter(id) {
    const i = this.chapters.findIndex((c) => c.id === id);
    if (i < 0) throw new Error('unknown chapter ' + id);
    this.chapterIdx = i;
    this.cap = this.chapters[i].cap;
  }

  /** Runs one step of a scenario's list. */
  async step(step) {
    await this.#step(step);
    if (Date.now() > this.deadline) throw new DeadlineError('took too long: an agent age may have ticked over');
  }

  async #step(step) {
    if (typeof step === 'function') return step(this);
    if (step.chapter) this.startChapter(step.chapter);
    if (step.run) await step.run(this);
    if (step.agent) this.setAgent(...step.agent);
    if (step.write) this.writePane(...step.write);
    if (step.keys) await this.keys(step.keys, step.gap);
    if (step.tap) await this.tap(typeof step.tap === 'function' ? step.tap(await this.settle()) : step.tap);
    if (step.type) {
      // PREFIX: what precedes the typed text on screen ("› " in the filter). KEY: a label, or a function of the text typed so far.
      const { text, perChar = 150, hold: last, key, expect, prefix = '› ' } = step.type;
      for (let n = 1; n <= text.length; n++) {
        await this.type(text[n - 1], perChar);
        const isLast = n === text.length;
        await this.snap({ expect: [`${prefix}${text.slice(0, n)}`, ...(isLast ? [].concat(expect ?? []) : [])], hold: isLast ? last : perChar, key: typeof key === 'function' ? key(text.slice(0, n)) : key, poster: isLast && step.type.poster });
      }
    }
    if (step.sleep) await sleep(step.sleep);
    if (step.snap) await this.snap(step.snap);
  }
}

/** Row and column of the first occurrence of TEXT on SCREEN. */
export function findText(screen, text) {
  for (let row = 0; row < screen.plain.length; row++) {
    const col = screen.plain[row].indexOf(text);
    if (col >= 0) return { row, col };
  }
  return null;
}

/**
 * Runs DEF once on fresh servers and returns the frame set.
 * OPTS: { toolsDir, source, keep, log }.
 */
export async function runScenario(def, { toolsDir, source, conf, keep = false, log = () => {} }) {
  const runId = `${process.pid}-${Date.now().toString(36)}`;
  const innerName = `paddock-demo-${runId}`;
  const outerName = `${innerName}-outer`;
  // Before anything is created or any handler installed: a refusal must not touch what exists.
  const existing = [innerName, outerName].filter((s) => fs.existsSync(socketPath(s)));
  if (existing.length) throw new Error('socket(s) already exist, refusing to continue: ' + existing.join(', '));

  const tmpRoot = fs.realpathSync(os.tmpdir());
  const base = fs.realpathSync(fs.mkdtempSync(path.join(tmpRoot, 'paddock-demo-')));
  const home = path.join(base, 'home');
  for (const d of ['webshop/api', 'webshop/web', 'infra', 'ml', 'docs']) fs.mkdirSync(path.join(home, 'code', d), { recursive: true });
  const tm = new Tmux({ inner: innerName, outer: outerName, env: cleanEnv(home) });
  const rawDir = path.join(toolsDir, '.out', 'raw', def.name);

  let cleaned = false;
  // Kills our servers, waits for them to die, removes sockets and the tmp dir only once they have.
  const cleanup = () => {
    if (cleaned) return;
    cleaned = true;
    if (keep) {
      console.error(`--keep: servers left running: tmux -L ${innerName} / tmux -L ${outerName}; tmp ${base}`);
      return;
    }
    const alive = tm.reap();
    if (alive.length) {
      console.error('WARNING: tmux servers did not die, sockets and tmp dir kept: ' + alive.join(', ') + ' ' + base);
    } else if (base.startsWith(path.join(tmpRoot, 'paddock-demo-'))) {
      fs.rmSync(base, { recursive: true, force: true });
    }
  };
  const handlers = ['SIGINT', 'SIGTERM', 'SIGHUP'].map((sig) => {
    const fn = () => { cleanup(); process.exit(128 + { SIGHUP: 1, SIGINT: 2, SIGTERM: 15 }[sig]); };
    process.once(sig, fn);
    return [sig, fn];
  });

  try {
    const ctx = new Ctx({
      def,
      tm,
      home,
      conf: conf ?? path.join(toolsDir, 'demo', 'tmux.conf'),
      transcripts: path.join(toolsDir, 'demo', 'transcripts'),
      log,
      rawDir,
    });
    fs.rmSync(rawDir, { recursive: true, force: true });
    ctx.repo = source.repo;
    await def.setup(ctx);
    // The recorder's glyph repertoire is single-width; check that tmux agrees.
    for (const g of GLYPHS) {
      tm.inner('set-option', '-g', '@glyph', g);
      const w = tm.inner('display-message', '-p', '#{w:@glyph}');
      if (w !== '1') throw new Error(`tmux measures ${JSON.stringify(g)} as ${w} columns`);
    }
    tm.inner('set-option', '-gu', '@glyph');
    // paddock.tmux binds the keys; check that it did.
    tm.inner('run-shell', path.join(source.repo, 'paddock.tmux'));
    const keys = tm.inner('list-keys', '-T', 'prefix', 'a');
    if (!keys.includes(source.bin)) throw new Error(`paddock.tmux did not bind prefix a to ${source.bin}:\n${keys}`);

    const attach = `env -u TMUX -u TMUX_PANE ${tm.bin} -u -L ${innerName} attach -t webshop:1`;
    tm.outer('-f', path.join(toolsDir, 'demo', 'outer.conf'), 'new-session', '-d', '-s', 'outer', '-x', String(def.cols), '-y', String(def.rows), attach);
    const size = `${def.cols}x${def.rows}`;
    for (let i = 0; ; i++) {
      const clients = tm.inner('list-clients', '-F', '#{client_control_mode} #{client_width}x#{client_height}').split('\n');
      if (clients.some((c) => c === `0 ${size}`)) break;
      if (i > 100) throw new Error(`no ${size} client attached: ${clients.join(' | ')}`);
      await sleep(100);
    }
    await ctx.settle(['webshop']);
    await def.start(ctx);
    for (const step of def.steps) await ctx.step(step);

    for (const c of ctx.chapters) if (c.frame === undefined || c.poster === undefined) throw new Error(`chapter ${c.id} has no ${c.frame === undefined ? 'frames' : 'poster'}`);
    return {
      ctx,
      data: buildSet({
        name: def.name,
        cols: def.cols,
        rows: def.rows,
        source: { ...source.info, ...def.source, cols: def.cols, rows: def.rows },
        chapters: ctx.chapters,
        snaps: ctx.snaps,
      }),
    };
  } finally {
    for (const [sig, fn] of handlers) process.off(sig, fn);
    cleanup();
  }
}

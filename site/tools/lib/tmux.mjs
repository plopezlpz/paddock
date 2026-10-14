// The only way the recorder talks to tmux. Every call names a socket with -L,
// and the name must start with "paddock-demo-": there is no code path to the
// default server, which the person running this is probably inside.

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

export const PREFIX = 'paddock-demo-';
const BIN_DIRS = ['/opt/homebrew/bin', '/usr/local/bin', '/usr/bin', '/bin'];

/** Absolute path of tmux: $TMUX_BIN, else found in fixed directories (not the caller's PATH). */
export function tmuxBin() {
  if (process.env.TMUX_BIN) return process.env.TMUX_BIN;
  for (const d of BIN_DIRS) {
    const p = path.join(d, 'tmux');
    if (fs.existsSync(p)) return p;
  }
  throw new Error('tmux not found in ' + BIN_DIRS.join(':'));
}

/** Throws unless NAME is a demo socket name. */
export function guard(name) {
  if (typeof name !== 'string' || !name.startsWith(PREFIX) || /[\s/]/.test(name)) {
    throw new Error(`refusing tmux socket ${JSON.stringify(name)}: it must start with ${PREFIX}`);
  }
  return name;
}

/** The clean environment of every spawn: no TMUX, no TMUX_PANE, no inherited PATH. */
export function cleanEnv(home) {
  return {
    HOME: home,
    PATH: BIN_DIRS.join(':'),
    LANG: 'en_US.UTF-8',
    TERM: 'xterm-256color',
    SHELL: '/bin/sh',
  };
}

/** Where tmux keeps the socket NAME (env is clean, so TMUX_TMPDIR is unset). */
export function socketPath(name) {
  return path.join('/tmp', `tmux-${os.userInfo().uid}`, guard(name));
}

/** A pair of sockets and the clean env to talk to them with. */
export class Tmux {
  constructor({ inner, outer, env }) {
    this.innerName = guard(inner);
    this.outerName = guard(outer);
    this.env = env;
    this.bin = tmuxBin();
  }

  #run(socket, args, { allowFail = false } = {}) {
    const res = spawnSync(this.bin, ['-L', guard(socket), ...args], { env: this.env, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
    if (res.status !== 0 && !allowFail) {
      throw new Error(`tmux -L ${socket} ${args.join(' ')} failed (${res.status}): ${res.stderr.trim()}`);
    }
    return res.stdout.replace(/\n$/, '');
  }

  inner(...args) { return this.#run(this.innerName, args); }
  outer(...args) { return this.#run(this.outerName, args); }
  innerTry(...args) { return this.#run(this.innerName, args, { allowFail: true }); }
  outerTry(...args) { return this.#run(this.outerName, args, { allowFail: true }); }

  /** True when a server answers on the socket. */
  alive(socket) {
    return spawnSync(this.bin, ['-L', guard(socket), 'ls'], { env: this.env, stdio: 'ignore' }).status === 0;
  }

  /** Kills the servers this object talks to (they are only ever ones we named). */
  killAll() {
    for (const s of [this.outerName, this.innerName]) {
      spawnSync(this.bin, ['-L', guard(s), 'kill-server'], { env: this.env, stdio: 'ignore' });
    }
  }

  /** The sockets with a server still answering. */
  leftovers() {
    return [this.innerName, this.outerName].filter((s) => this.alive(s));
  }

  /**
   * Kills our servers, waits for them to die, then removes the stale socket
   * files (kill-server leaves them) of the dead ones. Returns the sockets whose
   * server would not die: those files are kept.
   */
  reap({ tries = 20, delayMs = 100 } = {}) {
    this.killAll();
    for (let i = 0; i < tries && this.leftovers().length; i++) {
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, delayMs);
    }
    const alive = this.leftovers();
    for (const s of [this.innerName, this.outerName]) if (!alive.includes(s)) fs.rmSync(socketPath(s), { force: true });
    return alive;
  }
}

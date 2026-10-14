// What the tour and the phone scenario share: the fake sessions and agents.

import { PANE_CMD } from '../../lib/scenario.mjs';

// The agent colours the theme sets (@agent-color-*): the status bar dots are checked against them.
export const STATE_COLOR = { asking: '#cc4048', working: '#cc8c3c', finished: '#96a6c8', idle: '#95a99f' };

/** webshop (api, web), infra (terraform, logs), ml, docs. SPLIT: an editor beside the api agent. */
export function sessions(ctx, { split }) {
  const { cols, rows } = ctx.def;
  const size = ['-x', String(cols), '-y', String(rows)];
  ctx.inner('-f', ctx.conf, 'new-session', '-d', '-s', 'webshop', '-n', 'api', ...size, '-c', ctx.dir('webshop/api'), PANE_CMD);
  if (split) {
    ctx.inner('split-window', '-h', '-l', '52', '-t', 'webshop:1', '-c', ctx.dir('webshop/api'), PANE_CMD);
    ctx.inner('select-pane', '-t', 'webshop:1.0');
  }
  ctx.inner('new-window', '-d', '-t', 'webshop:2', '-n', 'web', '-c', ctx.dir('webshop/web'), PANE_CMD);
  ctx.inner('new-session', '-d', '-s', 'infra', '-n', 'terraform', ...size, '-c', ctx.dir('infra'), PANE_CMD);
  ctx.inner('new-window', '-d', '-t', 'infra:2', '-n', 'logs', '-c', ctx.dir('infra'), PANE_CMD);
  ctx.inner('new-session', '-d', '-s', 'ml', '-n', 'train', ...size, '-c', ctx.dir('ml'), PANE_CMD);
  ctx.inner('new-session', '-d', '-s', 'docs', '-n', 'site', ...size, '-c', ctx.dir('docs'), PANE_CMD);
  // The tab paddock opens on; it is saved when paddock closes.
  ctx.inner('set-option', '-g', '@paddock-last-tab', 'sessions');
  for (const [state, color] of Object.entries(STATE_COLOR)) {
    const set = ctx.inner('show-option', '-gqv', `@agent-color-${state}`);
    if (set !== color) throw new Error(`@agent-color-${state} is ${JSON.stringify(set)}, the scenarios expect ${color}: record with themes/gruber.conf`);
  }
}

// Ages include a few seconds of margin: each scenario finishes in under a minute,
// so 7m stays 7m. (The recorder checks.)
export const AGENTS = {
  api: ['webshop:1.0', { state: 'working', kind: 'claude', title: 'add rate limiting to the checkout API', ageSec: 5 }],
  web: ['webshop:2.0', { state: 'working', kind: 'opencode', title: 'migrate the cart page to the payment element', ageSec: 185 }],
  infra: ['infra:1.0', { state: 'idle', unseen: true, kind: 'pi', title: 'move staging onto the new vpc module', ageSec: 7 * 60 + 5 }],
  ml: ['ml:1.0', { state: 'working', kind: 'kimi', title: 'try a cosine lr schedule with warmup', ageSec: 10 * 60 + 5 }],
  docs: ['docs:1.0', { state: 'idle', kind: 'claude', title: 'rewrite the getting-started guide', ageSec: 65 * 60 + 5 }],
};

/** Sets every agent and writes the idle ones' transcripts; the caller writes the rest. EXTRA goes to each setAgent (limit: false when no age is shown). */
export function startAgents(ctx, extra = {}) {
  ctx.writePane('infra:1.0', 'infra.ans');
  ctx.writePane('infra:2.0', 'logs.ans');
  ctx.writePane('ml:1.0', 'ml.ans');
  ctx.writePane('docs:1.0', 'docs.ans');
  for (const [target, agent] of Object.values(AGENTS)) ctx.setAgent(target, { ...agent, ...extra });
}

/**
 * Predicate: the status bar marks WINDOW (e.g. "2 web") with a dot in the colour of STATE.
 * The theme puts the dot before the index and shows the window as "2:web".
 */
export const dot = (window, state) => ({ cell: { find: window.replace(' ', ':'), offset: -2, ch: '●', fg: STATE_COLOR[state] } });

/** Like dot, for a narrow client (a phone): other windows are the dot and the index only. */
export const dotNarrow = (index, state) => ({ cell: { find: `● ${index}`, ch: '●', fg: STATE_COLOR[state] } });

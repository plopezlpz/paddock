// Skim without navigating, 132x36: the live preview follows the selection on the agents tab, and keeps
// up with what the selected agent writes: Claude Code's spinner and tool calls, then pi streaming.

import { AGENTS, sessions, startAgents } from './common.mjs';

// pi is working in this recording; its age sorts it just after api.
const PI = ['infra:1.0', { ...AGENTS.infra[1], state: 'working', unseen: false, ageSec: 2 * 60 + 5 }];
// What claude-N.ans shows at step N: the verb and the counter, and the newest tool call.
const CLAUDE = { 1: ['Pondering…', '5s · ↑ 0.9k'], 2: ['Noodling…', '6s · ↑ 1.2k'], 3: ['Percolating…', '7s · ↑ 1.6k'], 4: ['Simmering…', '8s · ↑ 1.9k'], 5: ['Noodling…', '9s · ↑ 2.3k'], 6: ['Clauding…', '10s · ↑ 2.6k'] };
const NEWEST = { 3: 'ratelimit.go', 5: 'ratelimit_test.go', 6: 'go test' };

export default {
  name: 'feat-preview',
  cols: 132,
  rows: 36,
  chapters: [
    { id: 'open', title: 'Open on the agents tab', cap: 'The agents tab, with the selected pane beside it' },
    { id: 'live', title: 'The preview is live', cap: 'The pane keeps writing, and so does the preview' },
    { id: 'skim', title: 'Skim with the arrows', cap: 'Move to another agent: the preview follows' },
    { id: 'onward', title: 'Keep skimming', cap: 'Nothing was opened or switched' },
  ],
  setup(ctx) {
    sessions(ctx, { split: true });
    ctx.inner('set-option', '-g', '@paddock-last-tab', 'agents');
  },
  start(ctx) {
    ctx.writePane('webshop:1.0', 'claude-1.ans');
    ctx.writePane('webshop:1.1', 'editor.ans');
    ctx.writePane('webshop:2.0', 'web-1.ans');
    startAgents(ctx);
    ctx.writePane('infra:1.0', 'pi-0.ans');
    ctx.setAgent(...PI);
  },
  steps: [
    {
      chapter: 'open',
      keys: ['C-b', 'a'],
      snap: { expect: ['sessions  agents', 'working 4', { selected: 'add rate limiting' }, 'webshop:1.0 · claude', CLAUDE[1][0], CLAUDE[1][1]], hold: 1600, key: 'prefix a', poster: true },
    },
    ...[2, 3, 4, 5, 6].map((n, i) => ({
      chapter: i === 0 ? 'live' : undefined,
      write: ['webshop:1.0', `claude-${n}.ans`],
      snap: { expect: [CLAUDE[n][0], CLAUDE[n][1], ...(NEWEST[n] ? [NEWEST[n]] : [])], hold: 1000, poster: i === 2 },
    })),
    {
      chapter: 'skim',
      keys: ['Down'],
      snap: { expect: [{ selected: 'move staging' }, 'infra:1.0 · pi', 'read envs/staging/main.tf', 'Working...', '$0.021'], hold: 1500, key: '↓', poster: true },
    },
    { write: ['infra:1.0', 'pi-1.ans'], snap: { expect: ['edit envs/staging/main.tf', '$0.034'], hold: 1100 } },
    { write: ['infra:1.0', 'pi-2.ans'], snap: { expect: ['$ terraform plan', '$0.046'], hold: 1100 } },
    { write: ['infra:1.0', 'pi-3.ans'], snap: { expect: ['private[1]', '$0.058'], hold: 1100 } },
    { write: ['infra:1.0', 'pi-4.ans'], snap: { expect: ['Plan: 0 to add, 4 to change', '$0.071'], hold: 1100 } },
    {
      chapter: 'onward',
      keys: ['Down'],
      snap: { expect: [{ selected: 'migrate the cart' }, 'webshop:2.0 · opencode', 'Checking the payments package'], hold: 1500, key: '↓', poster: true },
    },
    {
      keys: ['Down'],
      snap: { expect: [{ selected: 'cosine lr' }, 'ml:1.0 · kimi', 'epoch 4/20'], hold: 1800, key: '↓' },
    },
  ],
};

// The desktop tour, 132x36: agents at work, open, filter, agents tab, jump.

import { AGENTS, dot, sessions, startAgents } from './common.mjs';

const WAITING = ['webshop:2.0', { ...AGENTS.web[1], state: 'asking', ageSec: 0 }];

export default {
  name: 'tour',
  cols: 132,
  rows: 36,
  chapters: [
    { id: 'work', title: 'Agents at work', cap: 'Agents at work in your tmux' },
    { id: 'open', title: 'Open with prefix a', cap: 'prefix a opens paddock over your panes' },
    { id: 'filter', title: 'Filter', cap: 'Type to narrow the list' },
    { id: 'agents', title: 'Agents tab', cap: 'Tab shows every agent, grouped by state' },
    { id: 'jump', title: 'Jump to the one asking', cap: 'Enter jumps straight to the pane that is waiting' },
  ],
  setup: (ctx) => sessions(ctx, { split: true }),
  start(ctx) {
    ctx.writePane('webshop:1.0', 'api-1.ans');
    ctx.writePane('webshop:1.1', 'editor.ans');
    ctx.writePane('webshop:2.0', 'web-1.ans');
    startAgents(ctx);
  },
  steps: [
    { chapter: 'work', snap: { expect: ['webshop', 'Writing the limiter', 'ratelimit.go', dot('1 api', 'working'), dot('2 web', 'working')], hold: 1500 } },
    { write: ['webshop:1.0', 'api-2.ans'], snap: { expect: 'Testing the limiter', hold: 1100 } },
    {
      agent: WAITING,
      write: ['webshop:2.0', 'web-ask.ans'],
      snap: { expect: [dot('2 web', 'asking')], hold: 1800, cap: 'web is waiting for you', poster: true },
    },
    // The plain window first, so the popup arrives as an event; then the selection moves one row.
    { chapter: 'open', snap: { expect: [{ notText: 'sessions  agents' }, dot('1 api', 'working'), dot('2 web', 'asking')], hold: 1200, cap: 'A tmux window, as usual' } },
    {
      keys: ['C-b', 'a'],
      snap: { expect: ['sessions  agents', '› filter', 'webshop:1.0 · claude', '~/code/webshop'], hold: 1300, key: 'prefix a', cap: 'prefix a opens paddock over your panes', poster: true },
    },
    { keys: ['Down'], snap: { expect: ['sessions  agents', 'webshop:2.0 · opencode', 'Allow command?'], hold: 1000, key: '↓' } },
    { chapter: 'filter', type: { text: 'infra', perChar: 150, hold: 1200, key: (typed) => typed, poster: true } },
    { keys: ['C-u'], snap: { expect: '› filter', hold: 500, key: '^u' } },
    {
      chapter: 'agents',
      keys: ['Tab'],
      snap: { expect: ['waiting 1', 'working 2', 'finished 1', 'idle 1', { selected: 'add rate limiting' }], hold: 2000, key: '⇥ Tab' },
    },
    {
      agent: ['ml:1.0', { ...AGENTS.ml[1], state: 'idle', unseen: true, ageSec: 0 }],
      write: ['ml:1.0', 'ml-done.ans'],
      snap: { expect: ['finished 2', 'working 1'], hold: 1800, cap: 'States update live' },
    },
    (ctx) =>
      ctx.pressUntil('Up', { selected: 'migrate the cart' }, { expect: ['Allow command?', 'webshop:2.0 · opencode'], hold: 1800, poster: true, cap: 'The preview shows what it is asking' }),
    {
      chapter: 'jump',
      keys: ['Enter'],
      snap: { expect: [{ notText: 'sessions  agents' }, { client: { window: 'webshop:2' } }, 'Allow command?', dot('2 web', 'asking')], hold: 1800, key: '⏎ Enter' },
    },
    {
      write: ['webshop:2.0', 'web-yes.ans'],
      agent: ['webshop:2.0', { ...AGENTS.web[1], state: 'working', ageSec: 0 }],
      snap: { expect: ['added 1 package', dot('2 web', 'working')], hold: 2000, cap: 'You answer in the pane: back to work', poster: true },
    },
    {
      write: ['webshop:1.0', 'api-done.ans'],
      agent: ['webshop:1.0', { ...AGENTS.api[1], state: 'idle', unseen: true, ageSec: 0 }],
      snap: { expect: [dot('1 api', 'finished'), dot('2 web', 'working')], hold: 2200, cap: 'Finished: the dot turns blue' },
    },
  ],
};

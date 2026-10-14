// Agents in background windows need you or finish, 132x36: the status bar dots turn
// red (asking) and blue (finished), and paddock lists them. Desktop notifications are
// sent by agent-status.tmux outside tmux; they are not on this screen.

import { AGENTS, dot, sessions, startAgents } from './common.mjs';

const TESTS = ['webshop:3.0', { state: 'working', kind: 'claude', title: 'fix the flaky checkout test', ageSec: 5 }];

export default {
  name: 'feat-notify',
  cols: 132,
  rows: 36,
  chapters: [
    { id: 'work', title: 'Agents at work', cap: 'You are in api; two more agents work in the background' },
    { id: 'asking', title: 'One asks', cap: 'web needs an answer: its dot turns red' },
    { id: 'finished', title: 'One finishes', cap: 'tests is done: its dot turns blue' },
    { id: 'open', title: 'Open paddock', cap: 'paddock shows who is waiting and who is done' },
  ],
  setup(ctx) {
    sessions(ctx, { split: true });
    ctx.inner('new-window', '-d', '-t', 'webshop:3', '-n', 'tests', '-c', ctx.dir('webshop/api'), 'stty -echo; exec cat >/dev/null');
    ctx.inner('set-option', '-g', '@paddock-last-tab', 'agents');
  },
  start(ctx) {
    ctx.writePane('webshop:1.0', 'api-2.ans');
    ctx.writePane('webshop:1.1', 'editor.ans');
    ctx.writePane('webshop:2.0', 'web-1.ans');
    ctx.writePane('webshop:3.0', 'tests-1.ans');
    startAgents(ctx);
    ctx.setAgent(...TESTS);
  },
  steps: [
    { chapter: 'work', snap: { expect: [dot('1 api', 'working'), dot('2 web', 'working'), dot('3 tests', 'working')], hold: 1800, poster: true } },
    {
      chapter: 'asking',
      agent: ['webshop:2.0', { ...AGENTS.web[1], state: 'asking', ageSec: 0 }],
      write: ['webshop:2.0', 'web-ask.ans'],
      snap: { expect: [dot('2 web', 'asking'), dot('3 tests', 'working')], hold: 2200, poster: true },
    },
    {
      chapter: 'finished',
      agent: ['webshop:3.0', { ...TESTS[1], state: 'idle', unseen: true, ageSec: 0 }],
      write: ['webshop:3.0', 'tests-done.ans'],
      snap: { expect: [dot('2 web', 'asking'), dot('3 tests', 'finished')], hold: 2200, poster: true },
    },
    {
      chapter: 'open',
      keys: ['C-b', 'a'],
      snap: { expect: ['sessions  agents', 'waiting 1', 'working 2', 'finished 2', 'idle 1', { selected: 'add rate limiting' }], hold: 2400, key: 'prefix a', poster: true },
    },
    (ctx) => ctx.pressUntil('Up', { selected: 'migrate the cart' }, { expect: ['Allow command?', 'webshop:2.0 · opencode'], hold: 2200, cap: 'The one waiting shows what it asks' }),
    (ctx) => ctx.pressUntil('Down', { selected: 'fix the flaky' }, { expect: ['webshop:3.0 · claude', 'Done. The test waited'], hold: 2200, cap: 'The finished one shows what it did' }),
  ],
};

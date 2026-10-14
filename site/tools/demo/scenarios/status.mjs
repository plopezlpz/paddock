// Your tmux, unchanged, plus an agent state dot, 132x36: no popup. A Claude Code session works in
// the api window, then finishes; its dot in the status bar goes from working to finished.

import { AGENTS, dot, sessions, startAgents } from './common.mjs';

const VERBS = ['Clauding', 'Pondering', 'Noodling', 'Percolating', 'Simmering', 'Noodling', 'Clauding', 'Pondering'];
const FINISHED = ['webshop:1.0', { ...AGENTS.api[1], state: 'idle', unseen: true, ageSec: 0, limit: false }];

export default {
  name: 'status',
  cols: 132,
  rows: 36,
  chapters: [
    { id: 'work', title: 'An agent at work', cap: 'Your tmux, unchanged: a dot beside each window that has an agent' },
    { id: 'finished', title: 'It finishes', cap: 'Finished: the dot turns blue' },
  ],
  setup: (ctx) => sessions(ctx, { split: true }),
  start(ctx) {
    ctx.writePane('webshop:1.0', 'claude-wide-0.ans');
    ctx.writePane('webshop:1.1', 'editor.ans');
    ctx.writePane('webshop:2.0', 'web-1.ans');
    startAgents(ctx, { limit: false });
  },
  steps: [
    {
      chapter: 'work',
      snap: { expect: ['Welcome to Claude Code', `${VERBS[0]}…`, dot('1 api', 'working'), dot('2 web', 'working')], hold: 900, poster: true },
    },
    ...VERBS.slice(1).map((verb, i) => ({
      write: ['webshop:1.0', `claude-wide-${i + 1}.ans`],
      snap: { expect: [`${verb}…`, dot('1 api', 'working'), dot('2 web', 'working')], hold: i === VERBS.length - 2 ? 800 : 650 },
    })),
    {
      chapter: 'finished',
      write: ['webshop:1.0', 'claude-wide-8.ans'],
      agent: FINISHED,
      snap: { expect: ['Worked for', '4 tests pass', { notText: 'esc to interrupt' }, dot('1 api', 'finished'), dot('2 web', 'working')], hold: 2400, poster: true },
    },
  ],
};

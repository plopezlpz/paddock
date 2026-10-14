// Group the agents, 132x36: ctrl-g cycles status, session, folder, none.

import { AGENTS, sessions, startAgents } from './common.mjs';

// A second agent in the docs session that works in another project's folder, so
// grouping by session and by folder differ.
const DOCS_WEB = ['docs:2.0', { state: 'working', kind: 'opencode', title: 'document the payment element props', ageSec: 2 * 60 + 5 }];

export default {
  name: 'feat-group',
  cols: 132,
  rows: 36,
  chapters: [
    { id: 'status', title: 'By status', cap: 'Grouped by status: who needs you comes first' },
    { id: 'session', title: 'By session', cap: 'ctrl-g groups by session' },
    { id: 'folder', title: 'By folder', cap: 'Again: by folder, so one project stays together' },
    { id: 'none', title: 'No groups', cap: 'Once more: no headers, one list' },
  ],
  setup(ctx) {
    sessions(ctx, { split: true });
    ctx.inner('new-window', '-d', '-t', 'docs:2', '-n', 'props', '-c', ctx.dir('webshop/web'), 'stty -echo; exec cat >/dev/null');
    ctx.inner('set-option', '-g', '@paddock-last-tab', 'agents');
  },
  start(ctx) {
    ctx.writePane('webshop:1.0', 'api-2.ans');
    ctx.writePane('webshop:1.1', 'editor.ans');
    ctx.writePane('webshop:2.0', 'web-ask.ans');
    ctx.writePane('docs:2.0', 'docs-2.ans');
    startAgents(ctx);
    ctx.setAgent(...DOCS_WEB);
    ctx.setAgent('webshop:2.0', { ...AGENTS.web[1], state: 'asking', ageSec: 0 });
  },
  steps: [
    {
      chapter: 'status',
      keys: ['C-b', 'a'],
      snap: { expect: ['sessions  agents', 'waiting 1', 'working 3', 'finished 1', 'idle 1', 'group: status'], hold: 2400, key: 'prefix a', poster: true },
    },
    {
      chapter: 'session',
      keys: ['C-g'],
      snap: { expect: ['group: session', 'webshop 2', 'docs 2', 'infra 1', 'ml 1', { notText: 'waiting 1' }], hold: 2400, key: '^g', poster: true },
    },
    {
      chapter: 'folder',
      keys: ['C-g'],
      snap: { expect: ['group: folder', '~/code/webshop/web 2', '~/code/webshop/api 1', '~/code/docs 1', { notText: 'docs 2' }], hold: 2400, key: '^g', poster: true },
    },
    {
      chapter: 'none',
      keys: ['C-g'],
      snap: { expect: ['group: none', { notText: '~/code/webshop/web 2' }, { notText: 'waiting 1' }], hold: 2400, key: '^g', poster: true },
    },
  ],
};

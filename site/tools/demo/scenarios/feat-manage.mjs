// Sessions, windows and panes from the popup, 132x36, on the sessions tab:
// ctrl-s a new window, shift-down to move it, ctrl-z to undo, ctrl-x to kill it.

import { sessions, startAgents } from './common.mjs';

export default {
  name: 'feat-manage',
  cols: 132,
  rows: 36,
  chapters: [
    { id: 'new', title: 'A new window', cap: 'ctrl-s names a new window right where it will appear' },
    { id: 'move', title: 'Move and undo', cap: 'shift-↓ moves the window past its neighbour' },
    { id: 'kill', title: 'Kill, after asking', cap: 'ctrl-x asks before it kills' },
  ],
  setup(ctx) {
    sessions(ctx, { split: true });
    // A window made from the popup runs a shell: keep this machine's host and user out of its prompt.
    ctx.inner('set-option', '-g', 'default-command', "env PS1='$ ' sh");
  },
  start(ctx) {
    ctx.writePane('webshop:1.0', 'api-2.ans');
    ctx.writePane('webshop:1.1', 'editor.ans');
    ctx.writePane('webshop:2.0', 'web-1.ans');
    // The tree shows dots, never ages.
    startAgents(ctx, { limit: false });
  },
  steps: [
    {
      chapter: 'new',
      keys: ['C-b', 'a'],
      snap: { expect: ['sessions  agents', 'terraform', 'logs', { selected: 'api' }], hold: 1400, key: 'prefix a' },
    },
    {
      keys: ['C-s'],
      snap: { expect: ['+ new window', 'window name (empty:'], hold: 1400, key: '^s' },
    },
    { type: { text: 'notes', perChar: 110, hold: 800, prefix: '+ ', key: (typed) => typed, poster: true } },
    {
      keys: ['Enter'],
      snap: { expect: [{ notText: 'sessions  agents' }, { client: { window: 'webshop:2' } }, '2:notes'], hold: 1700, key: '⏎ Enter', cap: 'Enter creates it and switches to it' },
    },
    {
      chapter: 'move',
      keys: ['C-b', 'a'],
      snap: { expect: ['sessions  agents', '2:notes', { selected: 'notes' }], hold: 1200, key: 'prefix a', cap: 'Open paddock again: it is in the tree', poster: true },
    },
    {
      keys: ['S-Down'],
      snap: { expect: ['✓ moved down   ^z undo', { selected: 'notes' }], hold: 1700, key: 'shift ↓', poster: true },
    },
    {
      keys: ['C-z'],
      snap: { expect: ['✓ undone: moved down', { selected: 'notes' }], hold: 1500, key: '^z', cap: 'ctrl-z undoes the last rename or move' },
    },
    {
      chapter: 'kill',
      keys: ['C-x'],
      snap: { expect: ['✗ kill window webshop:2 notes?', "can't be undone"], hold: 2000, key: '^x', poster: true },
    },
    {
      keys: ['Enter'],
      snap: { expect: ['✓ killed window webshop:2 notes', { notText: "can't be undone" }, { notText: '2   notes' }], hold: 1700, key: '⏎ Enter', cap: 'Enter confirms' },
    },
  ],
};

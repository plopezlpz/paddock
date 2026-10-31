// Rename an agent with a suggested title, 132x36: ctrl-r, ctrl-r again, Enter.
// The suggest command is a demo stub (tools/demo/suggest.sh), not agent-status.tmux's.

import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { sessions, startAgents } from './common.mjs';

const stub = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'suggest.sh');
const TITLE = 'add rate limiting to the checkout API';
const SUGGESTED = 'Rate limiting';

export default {
  name: 'feat-rename',
  cols: 132,
  rows: 36,
  source: {
    suggest: "demo stub tools/demo/suggest.sh: canned titles after a 0.8 s wait; agent-status.tmux's real @agent-suggest-command is describe.sh suggest, which asks pi",
  },
  chapters: [
    { id: 'open', title: 'Select an agent', cap: 'Select an agent on the agents tab' },
    { id: 'rename', title: 'Rename in place', cap: 'ctrl-r edits the title in place' },
    { id: 'suggest', title: 'Ask for a title', cap: 'ctrl-r again asks for a suggested title' },
    { id: 'save', title: 'Save it', cap: 'Enter saves it, ctrl-z undoes it' },
  ],
  setup(ctx) {
    sessions(ctx, { split: true });
    ctx.inner('set-option', '-g', '@paddock-last-tab', 'agents');
    // agent-status.tmux's side of the contract: paddock appends the pane id.
    ctx.inner('set-option', '-g', '@agent-suggest-command', `'${stub}'`);
  },
  start(ctx) {
    // The stub's canned titles are keyed by pane id.
    const id = ctx.inner('display-message', '-p', '-t', 'webshop:1.0', '#{pane_id}');
    if (id !== '%0') throw new Error(`webshop:1.0 is ${id}, but suggest.sh expects %0`);
    ctx.writePane('webshop:1.0', 'api-2.ans');
    ctx.writePane('webshop:1.1', 'editor.ans');
    ctx.writePane('webshop:2.0', 'web-1.ans');
    startAgents(ctx);
  },
  steps: [
    {
      chapter: 'open',
      keys: ['C-b', 'a'],
      snap: { expect: ['sessions  agents', { selected: TITLE }, 'webshop:1.0 · claude'], hold: 1500, key: 'prefix a', poster: true },
    },
    {
      chapter: 'rename',
      keys: ['C-r'],
      snap: { expect: [`✎ ${TITLE}`, '✎ renaming', '⏎ save', '^r suggest'], hold: 1700, key: '^r', poster: true },
    },
    {
      chapter: 'suggest',
      keys: ['C-r'],
      snap: { expect: ['✦ suggesting…'], hold: 1000, key: '^r' },
    },
    { sleep: 600, snap: { expect: [`✎ ${SUGGESTED}`, { notText: 'suggesting' }], hold: 2400, cap: 'The suggestion fills the field: keep it, edit it or drop it', poster: true } },
    {
      chapter: 'save',
      keys: ['Enter'],
      snap: { expect: [{ selected: SUGGESTED }, '✓ renamed   ^z undo', { notText: '✎' }], hold: 2400, key: '⏎ Enter', poster: true },
    },
  ],
};

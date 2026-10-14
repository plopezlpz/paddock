// The phone layout, 40x44 (a portrait iPhone): tap the session name, tap "agents", tap a card twice.

import { findText } from '../../lib/scenario.mjs';
import { AGENTS, dotNarrow, sessions } from './common.mjs';

export default {
  name: 'phone',
  cols: 40,
  rows: 44,
  chapters: [
    { id: 'open', title: 'Tap the session name', cap: 'On your phone: tap the session name' },
    { id: 'agents', title: 'Agents tab', cap: 'Tap agents to see who needs you' },
    { id: 'jump', title: 'Jump to the one asking', cap: 'Tap a card to select it, tap again to open it' },
  ],
  setup: (ctx) => sessions(ctx, { split: false }),
  // The panes are 40 columns: the phone-*.ans transcripts are the usual ones with shorter lines.
  start(ctx) {
    ctx.writePane('webshop:1.0', 'phone-api.ans');
    ctx.writePane('webshop:2.0', 'phone-web.ans');
    ctx.writePane('infra:1.0', 'phone-infra.ans');
    ctx.writePane('infra:2.0', 'phone-logs.ans');
    ctx.writePane('ml:1.0', 'ml.ans');
    ctx.writePane('docs:1.0', 'phone-docs.ans');
    for (const [target, agent] of Object.values(AGENTS)) ctx.setAgent(target, agent);
  },
  steps: [
    { chapter: 'open', snap: { expect: ['webshop', 'Writing the limiter'], hold: 1100 } },
    {
      agent: ['webshop:2.0', { ...AGENTS.web[1], state: 'asking', ageSec: 0 }],
      write: ['webshop:2.0', 'phone-web-ask.ans'],
      snap: { expect: [dotNarrow('2', 'asking')], hold: 900, cap: 'web is waiting for you' },
    },
    // status-left is the last row, first columns
    { tap: [2, 43], snap: { expect: ['sessions  agents', '› filter'], hold: 1800, poster: true, cap: 'The same popup, compact under 80 columns' } },
    {
      chapter: 'agents',
      tap: (screen) => [findText(screen, 'agents').col + 2, 0],
      snap: { expect: ['waiting 1'], hold: 1600 },
    },
    {
      tap: (screen) => [10, findText(screen, 'migrate the cart').row],
      snap: { expect: [{ selected: 'migrate the cart' }], hold: 900, poster: true },
    },
    {
      chapter: 'jump',
      tap: (screen) => [10, findText(screen, 'migrate the cart').row],
      snap: { expect: [{ notText: 'sessions  agents' }, { client: { window: 'webshop:2' } }, 'Allow command?'], hold: 1800, poster: true },
    },
  ],
};

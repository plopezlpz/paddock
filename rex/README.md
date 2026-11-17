# paddock on Rex

paddock also runs on a [Rex](https://www.superlogical.com/rex) server, in a
floating pane: the same fuzzy finder over sessions, tabs and panes, and the
agents grouped by state. Agent state is Rex's own program status (OSC 7501),
so agent-status.tmux isn't needed: Claude Code and pi already report it. Each
agent card also shows the status message: what the agent is doing, why it
waits ("Allow edit to main.rs?"), or what failed. A failed agent is
finished, its note saying how ("failed: …"); a finished one turns idle once
paddock has taken you to it, or to its tab or session, until it reports
something new. A sub-task that waits on you (a program's blocked record
under one that works on) shows as the agent's wait.

## Setup

```sh
paddock init --rex
```

installs the action, `paddock.lua`, beside Rex's `init.lua` (usually
`~/.config/rex/`), made for that paddock binary, and prints the lines to add
to `init.lua` unless it loads it already. Run it again after upgrading
paddock: the popup says so when the two don't match. In `init.lua`:

```lua
local paddock = dofile("/Users/you/.config/rex/paddock.lua")
paddock.setup{
  key = "cmd+shift+a",
  -- bin = "/path/to/paddock",   -- default: the one paddock init --rex ran
  -- tab = "agents",              -- default: the tab it was last closed on
  -- bounds = { 0.1, 0.08, 0.8, 0.84 },
  -- colors = { accent = "#e0af68", working = "#7aa2f7" },
  -- icons = "nerd",
  -- theme = { background = "#1a1b26" },   -- the floating terminal's colours
}
```

`bin` is a path or a name on the PATH; a colour paddock can't read is said
when the popup opens, not left as it was. From a checkout, `dofile` its
`rex/paddock.lua` instead (no version check).
Then `rex config check` and `rex config reload`. The action is also in the
command palette as "paddock: Sessions and Agents". Run in a Rex terminal
outside tmux, a bare `paddock` uses Rex too.

The keys are tmux's (see [Use](../README.md#use)): `ctrl-a` opens the
selected row's menu, with everything you can do with it, and `Esc` clears
the filter before it closes.

An agent's title is its pane's label when it has one, else the title the
agent sets for its terminal (Claude Code writes a short topic there). With
[agent-status.tmux](https://github.com/plopezlpz/agent-status.tmux) 0.9.0 and
`AGENT_AUTO_TITLE=on` in your shell's environment, each Claude Code or pi
session's first prompt names its pane with a few words pi writes, as T3 Code
titles its threads; a label you gave stays.

A tab without a name is named after its focused pane: an agent's title, else
the program running there, as tmux names a window.

To jump to a pane in another session, turn on **Remote Control** in Rex's
Settings under Rex Server: paddock asks the app to show that session.

## Remote hosts

Sessions on the other Rex servers your app connects to (`rex hosts`) are
listed too, marked `@host` (their agents' cards too), with their paths shown
as `host:/path`; typing the host's name filters to them. Each host has its own worker and listener
(`rex -S host …`), so a slow or offline host never holds up the popup: its
sessions and previews show the latest that arrived. A host that is away is
tried again every 30 seconds while paddock is open, and shows up when it
answers; one failed snapshot keeps its sessions, a second (after a restart)
drops them. While a server fails, the status line says which and why. Edits on a host's sessions go to that host; moving tabs or panes
between hosts isn't possible. In a host's session, the popup runs paddock on
that host: `bin` if it is there, else the `paddock` on its PATH, whatever
its version.

## How it works

| File | Role |
| --- | --- |
| `paddock.lua` | The action: opens `paddock --rex …` in a floating layer |
| `snapshot.lua` | Every session, tab and terminal pane with its program status, as Rex reports them |
| `worker.lua` | One `rex do` for the popup's lifetime, answering paddock's JSON requests (snapshot, preview, edits, jump) line by line |
| `events.lua` | A `rex do --all` listener that prints one line per burst of changes (`rex.after`), naming the panes that changed and saying whether the layout did, which paddock answers with a new snapshot of that much |

`src/backend/rex.rs` drives them and turns Rex's data into paddock's model;
`src/backend/tmux/` is the tmux side of the same `Backend` trait.

Every server, the one paddock runs on too, is served by a thread of its own,
so the popup never waits for Rex to move the selection or draw: snapshots
are taken there when the listener says something changed, and the preview's
captures run there one at a time, the latest wins, waking the popup when
they land. A quiet pane is captured about four times a second, a pane with a
long scrollback (tens of milliseconds a capture) about once every 40
captures' time, 3 seconds at most: Rex builds the whole scrollback for each
capture, and tells no one when a pane prints. An edit waits for its server, then for a fresh snapshot, so
what paddock shows next includes it (a held ⇧↓ moves a tab step by step).
A worker or listener that fails or ends is started again, after a second
on paddock's own server; while one fails, the status line says why (what it
wrote to stderr, or the error it answered). A listener writes a line every
ten seconds, so one left behind by a paddock that was killed ends then. A failed
capture keeps the preview's last screen and is tried again shortly.

Agents animate their titles several times a second; `events.lua` counts a
title change only when what paddock shows of it changes (the spinner in
front is ignored), so an animating agent costs no snapshots. When only
panes' state, title or folder changed, the listener names them and the
worker reads just those again. A new session, tab or layout reads the
sessions and tabs again (a call each) and keeps the panes as last read, so
only new panes cost their calls: over a network a pane is a tenth of a
second, a session's layout as much. Everything is read when a worker starts, or a listener (once
it says it listens, so nothing falls between the two), and when a pane changed
that no event names; that read of a far host with many panes can take a while
(a minute at most), its sessions showing when it is done, and an edit or a
jump there meanwhile says so instead of waiting. News that comes within three
snapshots' time of the last waits for it (a second at most), so many busy
agents on a big server cost it about a quarter of its time.

A title you give an agent is its pane's Rex label, so Rex shows it too;
clearing it goes back to Rex's default label. Hidden agents, when paddock
last took you to each pane, and the last tab have no place in Rex: they are
kept in `$XDG_STATE_HOME/paddock/rex-state` (default `~/.local/state`). A
closed pane's lines go once its server has answered without it.

`rex do rex/snapshot.lua --json` prints the snapshot on its own, for
debugging. `bash tests/rex-scripts.sh` runs the worker and the listener
against a running Rex server, in a session of its own that it kills when
done (`cargo test` covers what paddock makes of them, with a scripted `rex`).

## Not on Rex

- Moving a tab or pane to another session: Rex moves them within a session,
  and paddock offers only that (`⇧↓` stops at a session's last tab).
- The status-bar dots: Rex shows agent state in its tabs and session picker.

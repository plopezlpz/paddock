# Changelog

Notable changes to paddock, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.11.1] - 2026-11-22

### Added

- `@paddock-autosave` (minutes) saves the tmux workspace to its own bookmark, `auto`, in the background, and restores it when tmux starts; the bookmark it restored is kept as `auto.previous`. A session the terminal just started is replaced by the saved one of its name, and one in use is kept, with the saved one restored as `NAME-restored`.
- `@paddock-save-key` binds a key that saves the default bookmark and shows the result.
- A bookmark keeps each session's start directory.

### Fixed

- A resumed agent that exits no longer leaves its ID behind for the next save; one stopped with Ctrl-Z, or with its shell still starting, keeps it.
- Restore no longer adds a grouped session to a session of that leader's name that was already running.
- Restored sessions no longer keep a fixed default size from the restore.
- A restore cut short no longer leaves the saved session name on placeholder panes. The next restore removes its staging session, or keeps it as `unfinished-restore-PID` if agents had started in it.
- Paths with control characters are no longer saved mangled.

## [0.11.0] - 2026-11-22

### Added

- `paddock save [NAME]`, `paddock restore [NAME]`, `paddock list` and `paddock remove NAME`: named bookmarks of every tmux session on the server (default name `session`), kept in `~/.local/state/paddock/workspaces`. A restore brings back session and window names, layouts, directories, zoom and focus, grouped sessions, and each window's `automatic-rename` and `window-size`. Claude, Pi, Codex and Antigravity panes reopen their conversation by the exact ID the agent-status hooks recorded, typed into a login shell so Ctrl-Z and `fg` work. Running sessions are skipped, and no pane contents are saved.

## [0.10.6] - 2026-11-19

### Fixed

- Rex with other hosts: a new session's row (`ctrl-s ctrl-s`) showed at the very end of the list, under the last host's sessions, whichever host it was made on. It now shows after the sessions of the host it is made on (the selected row's folder's), where it is listed.

## [0.10.5] - 2026-11-18

### Fixed

- Rex: a call that failed while reading the server (a connection's hiccup) could drop an agent waiting on you until its pane next changed, or show a session with no tabs. What was last read now stays, and is read again a second later.
- Rex: an agent's status that outlived its program, with nothing running in the pane, showed as live; it is now unconfirmed, as with a shell in front.
- Rex: the key did nothing when the popup could not open or close; Rex now shows why.
- Rex: sessions reordered elsewhere showed in their old order until something else changed.

## [0.10.4] - 2026-11-18

### Fixed

- tmux: a paste starting with `~` (typing mode) arrived with the home folder in its place, and one like `~~~` or `~nobody` pasted nothing: tmux expands a leading `~` even in double quotes. A paste that fails no longer leaves its text in a tmux buffer.
- A card's changes (`+N −M`) were missing with git in another language, and could be another repository's when paddock's environment had `GIT_DIR` set: git now runs in the C locale, without those.
- Typing into an agent that then exited, with nothing else listed, went on blind: the preview stays while typing.
- The only session could be killed through its only window, or its card's pane; a kill that closes a session now says so.
- A card's changes shown again after a round trip through the sessions tab were the ones from before; and a folder git is slow on (over 250 ms) no longer holds up the others every minute: it is asked again when its agent changes state.
- A click outside a drop's question left the row carried listed against the filter; a menu whose row a note filtered away stayed open over the next row; `ctrl-o` with no room for the preview turned it off unseen (it now says there is no room); sessions to agents from a pane row picked the current window's agent; a swap with a neighbour the filter hides now names it.
- A git-tracked home folder no longer anchors every folder label and folder group: a nearer repository does.
- tmux older than 3.8: the paddock key says why paddock isn't there (the message at start is said to no one).

### Changed

- Filtering no longer reads each repository's HEAD per keystroke: once per snapshot or minute.
- Internal simplification: dead paths gone (Rex's client label lookup, its raw-write send), the layout decided once from the terminal size, and long functions split, with no change in behaviour.

## [0.10.3] - 2026-11-17

### Fixed

- Rex: paddock opened from one server's action into another host's pane (a Mac's `cmd+j` on a devbox session) failed with `client target "client:…" not found`: the app's ID is each server's own. An ID this server doesn't know now stands for the one app connected to it, by its label.

## [0.10.2] - 2026-11-17

### Changed

- On a phone (under 80 columns), the preview starts hidden, leaving the list the room: "show preview" in the menu (`ctrl-o`) shows it, and so does typing into a pane (`ctrl-l`).
- Rex: the docs no longer offer agent-status.tmux's pane titles (`AGENT_AUTO_TITLE`), which it dropped: an agent's title on Rex is its pane's label, else the one it gives its terminal, as before.

## [0.10.1] - 2026-11-17

### Changed

- An agent card leads with its title: the first line is the title (the folder while it has none) and the changes waiting, at the right; the second, after the elapsed time, what the agent asks or says while that is news, else its folder, and where it runs. Renaming edits the first line.

### Fixed

- Rex: a listener that ended just as paddock looked at it could leave paddock waiting without news (state changes, new tabs) until something else woke it; it is started again at once.

## [0.10.0] - 2026-11-17

### Changed

- An agent card's second line says where the agent runs, `session:window` (Rex: session and tab) at its right, so two agents in one folder are told apart; grouped by session, the window alone, and a window named after what runs in it by its number; grouped by folder, the first line keeps its path. Its topic is in plain text, and what the agent last said follows it after `›` only while it is news (working, waiting, finished): an idle agent's last words are in the preview (an untitled one keeps them, muted, as its name). Untracked files show as `N untracked`, not `N new`.
- Agents get T3 Code-style titles with agent-status.tmux 0.9.0's `@agent-auto-title on`: a few words pi writes from the first prompt, shown in place of the prompt's start. On Rex too, with `AGENT_AUTO_TITLE=on`: the title names the agent's pane.
- Rex: the popup opens about 50 ms sooner (the event listener says it listens at once, not after 50 ms), and other hosts start connecting while it does. A far host's first snapshot makes one call fewer per session, and its preview captures a third fewer (a pane's height is read every 8 captures, not each).

## [0.9.0] - 2026-11-17

### Added

- Type into the previewed pane: `ctrl-l` (or "type into it" in the row's menu) sends every key and paste to the pane in the preview, so you can answer an agent's question, approve a tool call or send a follow-up without leaving the popup. `ctrl-h` (back, as `ctrl-l` went in), `ctrl-]`, or a click or tap anywhere, stops. tmux and Rex.
- `Shift+Enter` while typing reaches the pane as itself (a new line in an agent's prompt, not a send): on Rex, and on tmux 3.5+ with `extended-keys` on and `extended-keys-format csi-u`. On Rex, keys go as Rex's key events, which Rex encodes as the pane's program asked (the kitty protocol, application cursor keys).
- `ctrl-f` selects the next agent that needs you, as `prefix A` would jump (also "next needing you" in the menu, and a footer hint while one does). `ctrl-a` `a`, tmux's `prefix a` pressed in the popup, closes it; `ctrl-a` `A` is `ctrl-f`.
- `paddock next` and `prefix A` (`@paddock-next-key`; `off` for none): jump to the agent waiting longest for you, else the one that finished longest ago, without opening the popup; pressed again, the next one. tmux.
- On tmux, a waiting or finished agent's card shows what it last said: the tool call it waits on (`Bash: cargo test`, or its question), or the first line of its last answer (muted once you have seen it). Read from the transcript agent-status.tmux names (Claude Code, pi, Codex), off the popup's thread.
- A waiting card names the call the agent waits on as agent-status.tmux records it (`@agent-asks`): a subagent's ask shows as `subagent: <tool>`, and a new ask updates the card at once.
- A settled agent's card shows the changes waiting in its folder: `+12 −3` against HEAD, and untracked files as `· 2 new`. git runs off the popup's thread, again when the agent changes state, and once a minute.

### Fixed

- A jump (from the popup, or `prefix A`) marks the agent seen through agent-status.tmux's own script: tmux sends no focus change when another terminal already shows the pane, so it stayed "finished".
- Rex: a failed agent is finished, with its "failed: …" note, not waiting; visiting it, or its tab or session, makes it idle, as a finished one. A blocked sub-task under a working program shows as the agent's wait.
- A jump to a pane stays in your own session when it shows the pane too (grouped sessions, linked windows).
- An agent that died without its exit hook no longer looks alive once another program (an editor) runs in its pane: on tmux, against `@agent-command` (agent-status.tmux records it); on Rex, against the process that sent its status.

## [0.8.1] - 2026-11-16

### Changed

- State marks are circles again, told apart by colour: `○` idle, `●` the others (or agent-status.tmux's `@agent-icon-*`, whatever they are), not 0.8.0's `!`, `◐`, `✓`.
- `@paddock-agent-icons` is a format, expanded for the client the popup opens in: a terminal without a Nerd Font (a phone's) can get Unicode marks while another gets Nerd Font logos, e.g. `'#{?#{m:ghostty*,#{client_termtype}},nerd,unicode}'`.

## [0.8.0] - 2026-11-16

### Added

- Rex support (`paddock --rex`, rex/README.md): sessions, tabs, panes and agents on a Rex server and the servers its app connects to, with agent state from Rex's own program status. Snapshots and previews come in on a thread per server, so the popup never waits for Rex; animated agent titles (spinners) cost no snapshots, an agent's news reads only its own pane again, and a new tab or layout reads the sessions and tabs again but keeps the panes as read, so only a server's first read costs a call for each pane (over a network, a tenth of a second each). A quiet pane whose captures are slow (a long scrollback) is captured less often, every 3 seconds at most. An edit waits for a snapshot that has it, so a held `⇧↓` moves a tab step by step, on remote hosts too. A failed worker or listener is restarted; while a server fails the status line says which and why, for as long as it does, and its sessions are marked (`@host · away`). A host that is away is tried again every 30 seconds. A failed agent counts as asking, an unknown state as idle, and a finished agent turns idle once paddock has taken you to it, until it reports something new. Only what Rex can do is offered, in Rex's words: tabs and panes move within their session, and session names need not be unique. A tab without a name is named after its focused pane: an agent's title, else what runs there.
- `paddock init --rex` installs the Rex action, made for that binary, beside Rex's `init.lua` and prints how to load it; paddock refuses an action made for another version and says how to fix it, as with tmux key bindings.
- `ctrl-a` opens the selected row's menu.
- State marks of their own beside the colour: `!` asking, `◐` working, `✓` finished, `○` idle. agent-status.tmux's default `●` for three states is not used for them.
- While filtering, the matched characters are underlined; the selected row is marked `▌` and bold in the accent colour.
- The preview goes below the list when the popup is too narrow for it beside the list but tall enough; the popup fills a small terminal by default.
- An empty Agents tab says how agents get there.
- `↑ N more` and `↓ N more` say that the list goes on past the popup's edge.
- `ctrl-z` also undoes a hide or a show.
- Grouped by folder, a card whose path would only repeat its group's header shows its session and window instead.

### Changed

- `Esc` clears the filter before it closes the popup.
- The tab paddock reopens on is kept as soon as it changes, so it holds even when the popup is closed from outside.
- A tmux preview of a short screen is topped up from the pane's history, except in the alternate screen (a full-screen program).
- Repository labels for linked Git worktrees are found from files, without running Git for the usual layout or a bare main repository, and each HEAD is read again only when it changes.
- Sessions window paths use the same Git-aware repository/worktree labels and branch suffixes as Agents, including explicit detached HEAD labels. Full paths, branches and commit IDs remain searchable, including metadata from inactive panes; window paths follow the active pane. Pane counts remain visible when space permits.
- Grouped by folder, the agents in one Git repository share a group, its worktrees included, headed by the repository's name (with its folder when two share one).
- Switching to Sessions from an agent selects the agent's window, and a selected window's session stays in view when there is room.
- A card with no title gives its agent's message the whole line, not `untitled`.
- A long session or window name is cut with `…` and leaves the path its room; an agent's message takes the room its title leaves, a branch the room its path leaves.
- An agent's title from its pane title keeps punctuation in front (`~/src`, `[wip]`): only the glyphs and spinners agents put there are dropped.
- The default accent is magenta, not yellow, which is also the working colour.

### Fixed

- A kill is confirmed with `Enter` only. `ctrl-x` again used to confirm it, so two presses (or a held key) killed row after row; now any other key cancels.
- The row menu scrolls in a popup too short for it: its selection could move onto items that were not drawn, and `Enter` ran them.
- A rename, or a new session or window, that the server refuses opens again with what was typed and the reason, instead of losing it to a status line.
- A pane moved into a collapsed window opens it; before, the pane was listed alone and its next move seemed to do nothing.
- An undo that fails stays to be tried again; `ctrl-s` cancelled goes back to the tab it came from; a move dropped where it can't go no longer keeps its row listed.
- Wide characters are measured by grapheme when text is cut to fit, so an emoji with a variation selector or a joiner no longer overflows its place.

## [0.7.0] - 2026-10-31

### Added

- A standalone `paddock notification STATE KIND PATH [unicode|nerd]` formatter prints an icon/state title and the same compact path/branch label as agent cards, for optional desktop-notification integrations. Unicode is the portable default; no notification is sent by this command.

### Changed

- Agent cards put the bright status dot, provider icon and directory/branch on the first line, with state-colored elapsed time and a muted topic label below. The topic editor stays on that second line; card heights and Sessions are unchanged.
- Git branch suffixes remain visible when the branch and directory names match. Detached HEAD is explicitly labeled `(detached abc1234)`.

## [0.6.0] - 2026-10-31

### Added

- Agent cards show the checked-out Git branch in parentheses, or a short commit ID for detached HEAD. A branch matching the final directory name is omitted to avoid repetition.
- Agent filtering searches full absolute and home-abbreviated paths, repository labels, full branch names and detached commit IDs, including hidden cards and text shortened on screen.

### Changed

- Agent cards show the working directory instead of tmux session/window/pane coordinates, in both wide and compact layouts. Sessions is unchanged.
- Git-aware labels keep the main repository identifiable: linked worktrees show `repo/…/worktree`, with any subdirectory suffix, regardless of where the worktree is stored. Otherwise labels start at the first enclosing repository from the filesystem root; non-repository paths keep their usual abbreviation.
- Directory labels and branches share the existing muted metadata line; the filter keeps its blank separator rather than displaying a full-path heading.

## [0.5.0] - 2026-10-30

### Added

- Hide/Show agent cards from their menu or with `ctrl-e`, without stopping agents or closing panes. Hidden cards live under one collapsible Hidden section, follow the visible list's grouping/order without nested headings, and retain their state indicators.
- Hidden visibility and explicit expansion persist across popup opens. Search reveals hidden matches without changing either preference; opening a result does not unhide it.

### Changed

- Agent glyphs use the same muted tone as session/window metadata; state indicators keep their colors.

## [0.4.0] - 2026-10-30

### Added

- Agent cards show a small agent-type mark in the terminal's normal foreground after the elapsed time, including in the compact layout and while renaming. The default Unicode marks need no icon font; the state dot stays beside the title.
- `@paddock-agent-icons nerd` opts into Nerd Font Claude/OpenAI logos, a Gemini sparkle, an OpenCode squared outline and plain π. `unicode` (the default, also used for unknown option values) keeps portable text marks. No font installation, image protocol or passthrough is required by Paddock; Nerd mode requires suitable glyphs in the local terminal's font.

### Changed

- Cards no longer repeat the agent name beside the icon; it remains searchable and appears in the preview header.

## [0.3.1] - 2026-10-21

### Fixed

- A kill or move that fails no longer leaves you in the session paddock moved you to: it takes you back where you were.
- A window whose panes paddock can't read no longer crashes the popup.
- Releases: nothing is published unless master can move to the tagged commit, so Homebrew and TPM users never get different versions; a backport release leaves master alone.

## [0.3.0] - 2026-10-20

Faster to open and lighter while open; a kill never drops you out of tmux.

### Changed

- Opening is quicker: the key runs paddock directly, with no shell in between, and the popup creates its `_paddock` session itself, one tmux process fewer per open.
- Lighter while open: the 1 s refresh carries its own snapshot instead of triggering a second full read.
- The preview no longer wakes up for changes elsewhere.
- Pane titles that change on their own (shell prompts, agent spinners) no longer cause redraws.
- `paddock.tmux` loads in about 30 ms instead of about 170 ms.
- The key bindings pass `--tab last` (or `--tab agents`); paddock still opens with no arguments.
- Opening paddock no longer runs your global `pane-focus-in` hooks; they are emptied on `_paddock` like the client hooks.
- State dots use agent-status.tmux's `@agent-icon-*` glyphs when they are one column wide, else `●` (`○` idle).
- The filter matches a window's whole folder path, not the shortened one shown.
- The filter finds waiting agents by `asking` too.
- A version mismatch between the plugin and the binary is reported before any usage error, and says which side to fix: reload tmux when the bindings are the older ones, or install the binary version they expect.

### Fixed

- A kill or move never detaches your terminal: paddock asks tmux where your client is first. This covers a second kill after paddock moved you, a window killed through its link in another session, and the last window of a grouped session.
- An agent whose pane is back at a shell (killed with `kill -9`, crashed) shows as unconfirmed, e.g. `working?`, instead of "working" forever. If you start an agent from a wrapper script, `exec` it, or it shows as unconfirmed too.
- A refresh from before a move no longer puts the old layout back on screen for a moment, and opening no longer fetches a second full snapshot right after the first.
- Undoing a window rename gives naming back to tmux.
- A menu whose row goes away closes instead of acting on the row that took its place.
- Repeated `shift-↓` no longer undoes its own move.
- Leaving a rename unchanged changes nothing.
- `Esc` while naming a new window puts the selection back where naming began.
- `@paddock-key` set to `;` survives a reload.
- A config that runs `unbind -a` before paddock loads still gets its bindings.
- A binding of yours that mentions paddock is no longer taken for paddock's own, and paddock's own `prefix a` is no longer taken for yours (and left unbound) when the binary has another name.

### Removed

- `PADDOCK_DEBUG`.

### Upgrading

The 0.3.0 plugin needs the 0.3.0 binary:

- **TPM:** `prefix U`, then press the paddock key and accept the download.
- **Homebrew:** `brew upgrade paddock`, update the plugin, then reload tmux.
- **From a clone:** `cargo build --release`, then reload tmux.

Until tmux is reloaded, an old binding opening the new binary says so and tells you to reload. If you use agent-status.tmux, update it to 0.4.0 and re-run `bash ~/.tmux/plugins/agent-status.tmux/manage-provider.sh install` to get its current hooks.

## [0.2.0] - 2026-10-14

`prefix a` by default; the first install offers to hook up your agents.

### Added

- `prefix a` opens paddock when none of `@paddock-key`, `@paddock-agents-key` and `@paddock-click` is set, unless your config already binds `prefix a`; then paddock says so and leaves it alone.
- The first TPM install offers to hook up the agents agent-status.tmux finds. Upgrades don't ask again.
- A website: <https://plopezlpz.github.io/paddock/>.

### Changed

- Status is beta (was prototype).
- The README covers the whole setup, agent state included, for TPM and Homebrew.
- The Homebrew formula is updated with each release, and its caveats show the `prefix a` default and link the agent-state setup.

### Removed

- `cargo install` as an install route. Install with TPM or Homebrew, or build from a clone.

### Upgrading

The plugin binds only a binary of its own version. With TPM, `prefix U`, then press the key and accept the download. With Homebrew, `brew upgrade paddock`, then reload tmux.

## [0.1.0] - 2026-10-13

First release: a fast tmux popup for jumping between sessions, windows and panes, and for keeping an eye on the AI coding agents running in them.

### Added

- Sessions tab: a session › window › pane tree with each window's agent state.
- Agents tab: one card per agent, grouped by status, session or folder (`ctrl-g`), the one that needs you first; it opens on the agent you are in.
- Live preview of the selected pane when the popup is at least 102 columns wide (`ctrl-o` toggles it).
- Fuzzy filter over sessions, windows and agents (`'exact`, `^prefix`, `suffix$`, `!not`).
- Rename, move and kill sessions, windows and panes, and create windows and sessions, from the popup; `ctrl-z` undoes the last rename or move.
- Rename an agent's title in place; `ctrl-r` again fills in a suggested title when something sets `@agent-suggest-command`.
- Mouse and touch: click, double-click, drag to move, and a right-click menu.
- A compact two-line layout with tappable words under 80 columns, for phones.
- `@paddock-key` reopens the tab paddock was last closed on; `@paddock-agents-key` opens the agents tab; `@paddock-click` opens it from a click on the session name.
- Popup size, style and colours through `@paddock-popup-*` and `@paddock-color-*`; unset, the accent follows your `pane-active-border-style` and the agent dots follow `@agent-color-*`.
- Agent state read from pane options (`@agent-state` and friends), which [agent-status.tmux](https://github.com/plopezlpz/agent-status.tmux) or any other tool can set.
- One tmux control-mode connection, through a hidden `_paddock` session that exists only while the popup is open; it redraws only when something changes.
- Prebuilt binaries for macOS and Linux, arm64 and x86_64. With TPM, the first key press offers the download (checksum verified), so no Rust is needed. Also installs with Homebrew or `cargo install --git`, with `run-shell 'paddock init'` binding the keys, or from a clone. The plugin binds only a binary of its own version.
- Requires tmux 3.8 or later.

[Unreleased]: https://github.com/plopezlpz/paddock/compare/v0.11.1...HEAD
[0.11.1]: https://github.com/plopezlpz/paddock/compare/v0.11.0...v0.11.1
[0.11.0]: https://github.com/plopezlpz/paddock/compare/v0.10.6...v0.11.0
[0.10.6]: https://github.com/plopezlpz/paddock/compare/v0.10.5...v0.10.6
[0.10.5]: https://github.com/plopezlpz/paddock/compare/v0.10.4...v0.10.5
[0.10.4]: https://github.com/plopezlpz/paddock/compare/v0.10.3...v0.10.4
[0.10.3]: https://github.com/plopezlpz/paddock/compare/v0.10.2...v0.10.3
[0.10.2]: https://github.com/plopezlpz/paddock/compare/v0.10.1...v0.10.2
[0.10.1]: https://github.com/plopezlpz/paddock/compare/v0.10.0...v0.10.1
[0.10.0]: https://github.com/plopezlpz/paddock/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/plopezlpz/paddock/compare/v0.8.1...v0.9.0
[0.8.1]: https://github.com/plopezlpz/paddock/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/plopezlpz/paddock/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/plopezlpz/paddock/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/plopezlpz/paddock/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/plopezlpz/paddock/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/plopezlpz/paddock/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/plopezlpz/paddock/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/plopezlpz/paddock/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/plopezlpz/paddock/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/plopezlpz/paddock/releases/tag/v0.1.0

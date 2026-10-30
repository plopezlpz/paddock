# Changelog

Notable changes to paddock, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Agent cards show a small agent-type mark after the elapsed time, including in the compact layout and while renaming. Text glyphs need no Nerd Font; the state dot stays beside the title.

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

[Unreleased]: https://github.com/plopezlpz/paddock/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/plopezlpz/paddock/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/plopezlpz/paddock/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/plopezlpz/paddock/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/plopezlpz/paddock/releases/tag/v0.1.0

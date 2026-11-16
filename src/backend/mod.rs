//! Where the sessions, windows and panes paddock shows live: a tmux server
//! (`tmux`) or a Rex server (`rex`). The popup's loop (main.rs) is the same
//! for both; a backend answers for the server's side of it.

pub mod rex;
pub mod tmux;

use std::time::Duration;

use crate::model::{Op, Snapshot, Tab, Target};
use crate::theme::Theme;

/// A server's news, posted to the popup's loop.
#[derive(Debug, PartialEq)]
pub enum Msg {
    /// Something changed on the server that a snapshot should reflect.
    Changed,
    /// tmux's refresh subscription's new value: a snapshot.
    Rows(Snapshot),
    /// Rex: a capture came in. The loop's next turn takes it; nothing to
    /// fetch.
    Captured,
    /// The connection ended (tmux: the server is gone, or our session was
    /// killed last).
    Exit,
}

/// What the popup starts from, read once when it opens.
pub struct Start {
    /// The tab paddock was last closed on.
    pub last_tab: Option<Tab>,
    pub theme: Theme,
    /// A shell command line that prints a title for its argument, a pane.
    pub suggest_command: String,
    /// Whether the Hidden section was left expanded.
    pub hidden_expanded: bool,
    /// Where paddock was opened from.
    pub session: String,
    pub window: String,
}

/// What a server can do where tmux and Rex differ, so the popup offers
/// nothing its backend refuses. The default is tmux's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Caps {
    /// Windows and panes move between sessions; on Rex only within theirs.
    pub cross_session_moves: bool,
    /// Session names follow tmux's rules: unique, without `.` or `:`.
    pub tmux_session_names: bool,
    /// What the popup opens is shown while it is still open, so a failure
    /// stays in it, to be read (Rex: its floating pane closes with paddock);
    /// else once it has closed (tmux).
    pub activate_open: bool,
    /// A Rex server: a window is called a tab, and setup advice is Rex's.
    pub rex: bool,
}

impl Caps {
    /// What a window is called on this server.
    pub fn window(&self) -> &'static str {
        if self.rex { "tab" } else { "window" }
    }
}

impl Default for Caps {
    fn default() -> Caps {
        Caps { cross_session_moves: true, tmux_session_names: true, activate_open: false, rex: false }
    }
}

/// What `Backend::run` did: the sessions the client was moved to on the
/// way (to keep it out of a session the op closes, and back after a
/// failure), in order, and the op's result.
pub struct Ran {
    pub moved: Vec<String>,
    pub result: Result<Option<Target>, String>,
    /// It failed without the server saying how it went (its answer never
    /// came): the op may have been made all the same.
    pub unsure: bool,
}

pub trait Backend {
    /// What the server can do.
    fn caps(&self) -> Caps {
        Caps::default()
    }

    /// The server's snapshot. tmux asks its server; Rex gives the latest its
    /// threads took, without waiting.
    fn fetch(&mut self) -> Result<Snapshot, String>;

    /// Replies received so far: an event posted with a smaller count is
    /// older than the last `fetch`.
    fn replied(&self) -> u64 {
        0
    }

    /// The connection is gone for good.
    fn is_dead(&self) -> bool;

    /// News of the connection since the last call, to show: Err while it
    /// fails (and is retried), Ok once it is back.
    fn health(&mut self) -> Option<Result<String, String>> {
        None
    }

    /// PANE's screen as lines with SGR escapes, as `capture-pane -e` prints
    /// it, and how long capturing took. tmux captures now. Rex captures in
    /// the background: this asks for a capture, unless one is under way, and
    /// returns the one that came in since (posting `Msg::Changed` when it
    /// does); None until then.
    fn capture(&mut self, pane: &str) -> Option<(Result<Vec<String>, String>, Duration)>;

    /// Make OP's change on the server.
    fn run(&mut self, op: &Op) -> Ran;

    /// The popup is on TAB, as it may close: the next one opens there.
    fn closed(&mut self, tab: Tab);

    /// Show TARGET in the client paddock was opened from: once the popup
    /// has closed, or before (`Caps::activate_open`).
    fn activate(&mut self, target: &Target) -> Result<(), String>;
}

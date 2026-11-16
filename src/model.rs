//! The popup's model: every session, window and pane, and the agents in
//! them, as a backend reports them; the targets ops act on, and the ops
//! (a backend makes them; the app, which knows no backend, asks for them).

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum State {
    Idle,
    Finished,
    Working,
    Asking,
}

impl State {
    /// Every state, in discriminant order: `[T; State::ALL.len()]` arrays
    /// are indexed by `state as usize`.
    pub const ALL: [State; 4] = [State::Idle, State::Finished, State::Working, State::Asking];

    pub fn label(self) -> &'static str {
        match self {
            State::Asking => "waiting",
            State::Working => "working",
            State::Finished => "finished",
            State::Idle => "idle",
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Agent {
    pub state: State,
    pub kind: String,
    /// What the card shows: the user's title, else the automatic one.
    pub title: String,
    /// The automatic title, which clearing the user's goes back to: the
    /// the tool's @agent-auto-description, else the pane title the agent
    /// set (terminal title escape).
    pub auto: String,
    /// Whether the user set the title (@agent-description).
    pub renamed: bool,
    pub since: Option<u64>,
    /// Paddock's per-pane visibility preference, independent of agent state.
    pub hidden: bool,
    /// Unconfirmed: the tool says so (@agent-stale), or a shell is the
    /// pane's foreground command, as after the agent went without its exit
    /// hook (killed, crashed). That also holds while a shell runs the agent
    /// without exec (`agent; exec $SHELL`, a wrapper script).
    pub stale: bool,
    /// What the agent says about its state, when its server reports it (Rex's
    /// program status message: why it waits, what failed). Empty on tmux.
    pub note: String,
}

#[derive(Debug, PartialEq)]
pub struct Pane {
    pub id: String,
    pub index: String,
    pub active: bool,
    pub command: String,
    pub path: String,
    pub agent: Option<Agent>,
}

#[derive(Debug, PartialEq)]
pub struct Window {
    pub id: String,
    pub index: String,
    pub name: String,
    pub active: bool,
    /// tmux names it after what runs (automatic-rename).
    pub auto: bool,
    pub panes: Vec<Pane>,
}

#[derive(Debug, PartialEq)]
pub struct Session {
    pub id: String,
    pub name: String,
    /// The Rex host it lives on; empty for the server paddock runs on.
    pub host: String,
    /// Its server is failing: what shows of it is the last it said.
    pub away: bool,
    pub windows: Vec<Window>,
}

#[derive(Debug, PartialEq)]
pub struct Snapshot {
    pub sessions: Vec<Session>,
    /// Agents per state, indexed by `State as usize`.
    pub counts: [usize; State::ALL.len()],
}

/// A session, window or pane, by its server's ids (tmux's `$1`, `@2`, `%3`).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Target {
    Session { session: String },
    Window { session: String, window: String },
    Pane { session: String, window: String, pane: String },
}

/// One of the popup's two lists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Sessions,
    Agents,
}

impl Tab {
    pub fn other(self) -> Tab {
        match self {
            Tab::Sessions => Tab::Agents,
            Tab::Agents => Tab::Sessions,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Sessions => "sessions",
            Tab::Agents => "agents",
        }
    }

    pub fn parse(label: &str) -> Option<Tab> {
        [Tab::Sessions, Tab::Agents].into_iter().find(|t| t.label() == label)
    }
}

/// A change paddock makes on the server. Sessions, windows and panes
/// are named by id.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Set PANE's @agent-description; None unsets it (back to automatic).
    SetTitle { pane: String, title: Option<String> },
    SetHidden { pane: String, hidden: bool },
    SetHiddenExpanded(bool),
    RenameSession { session: String, name: String },
    /// An empty NAME turns automatic naming back on.
    RenameWindow { window: String, name: String },
    Kill(Target),
    /// Swap windows A and B of SESSION (named through it: a window linked
    /// into several sessions is the copy in this one).
    SwapWindows { session: String, a: String, b: String },
    SwapPanes(String, String),
    /// Move WINDOW of session FROM beside window TO of session INTO: after
    /// it, or BEFORE.
    MoveWindow { from: String, window: String, into: String, to: String, before: bool },
    /// Move PANE into a split beside pane TO: after it, or BEFORE.
    JoinPane { pane: String, to: String, before: bool },
    /// Make PANE a window of its own at the end of SESSION.
    BreakPane { pane: String, session: String },
    NewSession { name: String, dir: String },
    NewWindow { session: String, after: String, name: String, dir: String },
}

/// A pane title as an agent title: none when it is tmux's default (the
/// host name), without the status glyphs agents put in front (✳, braille
/// spinners, ●, emoji): non-ASCII symbols, not ASCII ones (`~/src`, `[wip]`).
pub fn title_of(pane_title: &str, host: &str) -> String {
    let short = host.split('.').next().unwrap_or(host);
    let t = pane_title.trim();
    if t.is_empty() || t == host || t == short {
        return String::new();
    }
    t.trim_start_matches(|c: char| c.is_whitespace() || !(c.is_ascii() || c.is_alphanumeric())).trim().to_string()
}

/// Whether COMMAND (pane_current_command) is a shell, which a pane shows at
/// its prompt: login shells start with a dash.
pub fn is_shell(command: &str) -> bool {
    let name = command.rsplit('/').next().unwrap_or(command).trim_start_matches('-');
    matches!(name, "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "mksh" | "tcsh" | "csh" | "nu")
}

impl Pane {
    /// What the pane runs: its agent's kind, else the foreground command.
    pub fn label(&self) -> &str {
        self.agent.as_ref().map(|a| a.kind.as_str()).filter(|k| !k.is_empty()).unwrap_or(&self.command)
    }
}

impl Window {
    pub fn active_pane(&self) -> Option<&Pane> {
        self.panes.iter().find(|p| p.active).or(self.panes.first())
    }

    /// The most urgent agent state among the window's panes.
    pub fn state(&self) -> Option<State> {
        self.panes.iter().filter_map(|p| p.agent.as_ref().map(|a| a.state)).max()
    }
}

impl Session {
    pub fn active_window(&self) -> Option<&Window> {
        self.windows.iter().find(|w| w.active).or(self.windows.first())
    }
}

impl Snapshot {
    /// SESSIONS, with their agents counted by state.
    pub fn new(sessions: Vec<Session>) -> Snapshot {
        let mut snap = Snapshot { sessions, counts: [0; State::ALL.len()] };
        snap.counts = snap.unique_panes().filter_map(|(_, _, p)| p.agent.as_ref()).fold(snap.counts, |mut counts, a| {
            counts[a.state as usize] += 1;
            counts
        });
        snap
    }

    /// Every pane with its session and window, once: a window linked into
    /// several sessions lists its panes under each.
    pub fn unique_panes(&self) -> impl Iterator<Item = (&Session, &Window, &Pane)> {
        let mut seen = std::collections::HashSet::new();
        self.sessions
            .iter()
            .flat_map(|s| s.windows.iter().flat_map(move |w| w.panes.iter().map(move |p| (s, w, p))))
            .filter(move |(_, _, p)| seen.insert(p.id.as_str()))
    }

    /// Pane ID with its (first) session and window.
    pub fn pane(&self, id: &str) -> Option<(&Session, &Window, &Pane)> {
        self.sessions.iter().flat_map(|s| s.windows.iter().map(move |w| (s, w))).find_map(|(s, w)| Some((s, w, w.panes.iter().find(|p| p.id == id)?)))
    }

    /// The agent in pane ID.
    pub fn agent(&self, id: &str) -> Option<&Agent> {
        self.pane(id)?.2.agent.as_ref()
    }

    pub fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_lose_status_glyphs_but_not_ascii() {
        for (pane, title) in [
            ("✳ Fix the parser", "Fix the parser"), ("⠋ first prompt", "first prompt"), ("● ⠙  x", "x"), ("🤖 build", "build"),
            ("❤\u{fe0f} ship it", "ship it"), ("~/src/api", "~/src/api"), ("[wip] x", "[wip] x"), ("  -v flag", "-v flag"),
            ("日本語のタイトル", "日本語のタイトル"), ("✳ ", ""), ("pmac", ""), ("pmac.local", ""),
        ] {
            assert_eq!(title_of(pane, "pmac.local"), title, "{pane:?}");
        }
    }
}

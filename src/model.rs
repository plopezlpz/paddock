//! A snapshot of every session, window and pane, with the agent options a
//! producer (such as agent-status.tmux) sets on panes, from a single
//! `list-panes -a`; and the targets commands act on.

use crate::tmux::{quote, Control, SESSION};

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

    fn parse(state: &str, unseen: &str) -> Option<State> {
        match state {
            "asking" => Some(State::Asking),
            "working" => Some(State::Working),
            "idle" if unseen == "1" => Some(State::Finished),
            "idle" => Some(State::Idle),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            State::Asking => "waiting",
            State::Working => "working",
            State::Finished => "finished",
            State::Idle => "idle",
        }
    }
}

pub struct Agent {
    pub state: State,
    pub kind: String,
    /// What the card shows: the user's title, else the automatic one.
    pub title: String,
    /// The automatic title, which clearing the user's goes back to: the
    /// producer's @agent-auto-description, else the pane title the agent
    /// set (terminal title escape).
    pub auto: String,
    /// Whether the user set the title (@agent-description).
    pub renamed: bool,
    pub since: Option<u64>,
    /// The producer hasn't confirmed this state recently.
    pub stale: bool,
}

pub struct Pane {
    pub id: String,
    pub index: String,
    pub active: bool,
    pub command: String,
    pub path: String,
    pub agent: Option<Agent>,
}

pub struct Window {
    pub id: String,
    pub index: String,
    pub name: String,
    pub active: bool,
    pub panes: Vec<Pane>,
}

pub struct Session {
    pub id: String,
    pub name: String,
    pub windows: Vec<Window>,
}

pub struct Snapshot {
    pub sessions: Vec<Session>,
}

/// A session, window or pane, by tmux ids (`$1`, `@2`, `%3`); or the
/// filter's Nth "+ new …" row, which names nothing on the server yet.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Target {
    Session { session: String },
    Window { session: String, window: String },
    Pane { session: String, window: String, pane: String },
    New(usize),
}

impl Target {
    /// The quoted `-t` argument. The session is always named, so a window
    /// linked into several sessions resolves to this one.
    pub fn arg(&self) -> String {
        quote(&match self {
            Target::Session { session } => session.clone(),
            Target::Window { session, window } => format!("{session}:{window}"),
            Target::Pane { session, window, pane } => format!("{session}:{window}.{pane}"),
            Target::New(_) => String::new(),
        })
    }
}

/// The `list-panes -a` fields, by level. The refresh subscription (main.rs)
/// watches the same ones.
pub const SESSION_FIELDS: &[&str] = &["session_id", "session_name"];
pub const WINDOW_FIELDS: &[&str] = &["window_id", "window_index", "window_name", "window_active"];
pub const PANE_FIELDS: &[&str] = &[
    "pane_id",
    "pane_index",
    "pane_active",
    "pane_current_command",
    "pane_title",
    "@agent-state",
    "@agent-unseen",
    "@agent-kind",
    "@agent-auto-description",
    "@agent-description",
    "@agent-state-at",
    "@agent-stale",
    // Last: a test fixture's last field may hold its separator.
    "pane_current_path",
];

pub fn fields() -> Vec<&'static str> {
    [SESSION_FIELDS, WINDOW_FIELDS, PANE_FIELDS].concat()
}

pub fn snapshot(tmux: &mut Control, host: &str) -> Result<Snapshot, String> {
    let rows = tmux.fields("list-panes -a -F", &fields())?;
    Ok(parse(&rows, host))
}

/// A pane title as an agent title: none when it is tmux's default (the
/// host name), without the status glyphs agents put in front (✳, spinners).
fn title_of(pane_title: &str, host: &str) -> String {
    let short = host.split('.').next().unwrap_or(host);
    let t = pane_title.trim();
    if t.is_empty() || t == host || t == short {
        return String::new();
    }
    t.trim_start_matches(|c: char| !c.is_alphanumeric()).trim().to_string()
}

/// Build the tree from `list-panes -a` rows (`fields()`, in session and
/// window order), without paddock's own session. HOST is the server's host
/// name, tmux's default pane title.
pub fn parse(rows: &[Vec<String>], host: &str) -> Snapshot {
    let mut sessions: Vec<Session> = Vec::new();
    for row in rows {
        let [sid, sname, wid, windex, wname, wactive, pid, pindex, pactive, command, pane_title, state, unseen, kind, auto, desc, at, stale, path] =
            row.as_slice()
        else {
            continue;
        };
        if sname == SESSION {
            continue;
        }
        if sessions.last().is_none_or(|s| &s.id != sid) {
            sessions.push(Session { id: sid.clone(), name: sname.clone(), windows: Vec::new() });
        }
        let session = sessions.last_mut().unwrap();
        // The same window can be linked at two indexes of one session.
        if session.windows.last().is_none_or(|w| &w.id != wid || &w.index != windex) {
            session.windows.push(Window {
                id: wid.clone(),
                index: windex.clone(),
                name: wname.clone(),
                active: wactive == "1",
                panes: Vec::new(),
            });
        }
        let agent = State::parse(state, unseen).map(|state| {
            let auto = if auto.trim().is_empty() { title_of(pane_title, host) } else { auto.trim().to_string() };
            let desc = desc.trim();
            Agent {
                state,
                kind: kind.clone(),
                title: if desc.is_empty() { auto.clone() } else { desc.to_string() },
                auto,
                renamed: !desc.is_empty(),
                since: at.parse().ok(),
                stale: stale == "1",
            }
        });
        session.windows.last_mut().unwrap().panes.push(Pane {
            id: pid.clone(),
            index: pindex.clone(),
            active: pactive == "1",
            command: command.clone(),
            path: path.clone(),
            agent,
        });
    }
    Snapshot { sessions }
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
        self.unique_panes().find(|(_, _, p)| p.id == id)
    }

    /// Agents per state, indexed by `State as usize`.
    pub fn counts(&self) -> [usize; State::ALL.len()] {
        let mut counts = [0; State::ALL.len()];
        for a in self.unique_panes().filter_map(|(_, _, p)| p.agent.as_ref()) {
            counts[a.state as usize] += 1;
        }
        counts
    }
}

/// A snapshot from `|`-separated `fields()` lines (host "host").
#[cfg(test)]
pub fn fixture(lines: &[&str]) -> Snapshot {
    let n = fields().len();
    let rows: Vec<Vec<String>> = lines.iter().map(|l| l.splitn(n, '|').map(String::from).collect()).collect();
    parse(&rows, "host")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_tree() {
        let snap = fixture(&[
            "$1|work|@1|1|edit|1|%1|0|1|nvim|||||||||/w",
            "$1|work|@1|1|edit|1|%2|1|0|claude||working|0|claude|fix it||100||/w",
            // The same window linked at a second index.
            "$1|work|@1|2|edit|0|%1|0|1|nvim|||||||||/w",
            "$1|work|@2|3|logs|0|%3|0|1|tail||idle|1||auto|mine|||/w|with|bars",
            "$9|_paddock|@9|0|paddock|1|%9|0|1|cat|||||||||/",
            "broken|line",
        ]);
        assert_eq!(snap.sessions.len(), 1, "paddock's own session is left out");
        let w = &snap.sessions[0].windows;
        assert_eq!(w.iter().map(|w| (w.id.as_str(), w.index.as_str())).collect::<Vec<_>>(), [("@1", "1"), ("@1", "2"), ("@2", "3")]);
        assert_eq!(w[0].panes.len(), 2);
        let agent = w[0].panes[1].agent.as_ref().unwrap();
        assert_eq!((agent.state, agent.title.as_str(), agent.since, agent.renamed), (State::Working, "fix it", Some(100), false));
        let agent = w[2].panes[0].agent.as_ref().unwrap();
        assert_eq!((agent.state, agent.title.as_str(), agent.auto.as_str(), agent.renamed), (State::Finished, "mine", "auto", true));
        assert_eq!(w[2].panes[0].path, "/w|with|bars", "the last field keeps separators");
        assert!(w[0].panes[0].agent.is_none());
    }

    #[test]
    fn titles_fall_back_to_the_pane_title() {
        let snap = fixture(&[
            "$1|s|@1|1|w|1|%1|0|1|zsh|✳ Fix the parser|working|0|claude||||1|/a",
            "$1|s|@1|1|w|1|%2|1|0|zsh|host|working|0|pi|||||/a",
            "$1|s|@1|1|w|1|%3|2|0|zsh|⠋ x|working|0|pi|first prompt||||/a",
        ]);
        let agents: Vec<_> = snap.unique_panes().filter_map(|(_, _, p)| p.agent.as_ref()).map(|a| (a.title.as_str(), a.stale)).collect();
        assert_eq!(agents, [("Fix the parser", true), ("", false), ("first prompt", false)], "status glyphs dropped; the host name is no title");
        assert_eq!(title_of("pmac", "pmac.local"), "", "the short host name too");
    }

    #[test]
    fn targets_name_their_session() {
        let t = Target::Pane { session: "$1".into(), window: "@2".into(), pane: "%3".into() };
        assert_eq!(t.arg(), "'$1:@2.%3'");
    }
}

//! A snapshot of every session, window and pane, with agent-status.tmux's
//! per-pane agent options, from a single `list-panes -a`; and the targets
//! commands act on.

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
    /// The override, else @agent-description, else agent-status.tmux's
    /// @agent-auto-description.
    pub title: String,
    /// Whether the title is an override (not the automatic one).
    pub overridden: bool,
    /// A generated description is on its way.
    pub generating: bool,
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

/// A session, window or pane, by tmux ids (`$1`, `@2`, `%3`).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Target {
    Session { session: String },
    Window { session: String, window: String },
    Pane { session: String, window: String, pane: String },
}

impl Target {
    /// The quoted `-t` argument. The session is always named, so a window
    /// linked into several sessions resolves to this one.
    pub fn arg(&self) -> String {
        quote(&match self {
            Target::Session { session } => session.clone(),
            Target::Window { session, window } => format!("{session}:{window}"),
            Target::Pane { session, window, pane } => format!("{session}:{window}.{pane}"),
        })
    }
}

/// `list-panes -a` fields.
const FIELDS: &[&str] = &[
    "session_id",
    "session_name",
    "window_id",
    "window_index",
    "window_name",
    "window_active",
    "pane_id",
    "pane_index",
    "pane_active",
    "pane_current_command",
    "@agent-state",
    "@agent-unseen",
    "@agent-kind",
    "@agent-auto-description",
    "@agent-state-at",
    "@agent-stale",
    "@agent-description",
    "pane_current_path",
];

/// agent-status.tmux's description overrides: files keyed by server start
/// time and pane number, a `.generating-` marker while pi writes one, and
/// its describe.sh, which sets them. No @agent-descriptions-dir, no
/// overrides.
pub struct Descriptions {
    dir: String,
    start: String,
    scripts: String,
}

/// A pane's description override, if any, and whether one is generating.
#[derive(Default)]
pub struct Described {
    pub text: Option<String>,
    pub generating: bool,
}

impl Descriptions {
    pub fn load(tmux: &mut Control, window: &str) -> Descriptions {
        let mut row = tmux.row(window, &["@agent-descriptions-dir", "start_time", "@agent-scripts-dir"]).into_iter();
        let mut next = || row.next().unwrap_or_default();
        Descriptions { dir: next(), start: next(), scripts: next() }
    }

    fn get(&self, pane: &str) -> Described {
        if self.dir.is_empty() {
            return Described::default();
        }
        let key = format!("{}-{}", self.start, pane.trim_start_matches('%'));
        let text = std::fs::read_to_string(format!("{}/{key}", self.dir))
            .ok()
            .and_then(|t| t.lines().next().map(|l| l.trim().to_string()))
            .filter(|l| !l.is_empty());
        let generating = std::path::Path::new(&format!("{}/.generating-{key}", self.dir)).exists();
        Described { text, generating }
    }

    /// agent-status.tmux's describe.sh, if the plugin advertises it.
    pub fn script(&self) -> Option<std::path::PathBuf> {
        let path = std::path::Path::new(&self.scripts).join("describe.sh");
        (!self.scripts.is_empty() && path.is_file()).then_some(path)
    }
}

pub fn snapshot(tmux: &mut Control, descriptions: &Descriptions) -> Result<Snapshot, String> {
    let rows = tmux.fields("list-panes -a -F", FIELDS)?;
    Ok(parse(&rows, |pane| descriptions.get(pane)))
}

/// Build the tree from `list-panes -a` rows (FIELDS, in session and window
/// order), without paddock's own session. DESCRIBE gives a pane's
/// description override.
pub fn parse(rows: &[Vec<String>], describe: impl Fn(&str) -> Described) -> Snapshot {
    let mut sessions: Vec<Session> = Vec::new();
    for row in rows {
        let [sid, sname, wid, windex, wname, wactive, pid, pindex, pactive, command, state, unseen, kind, auto_desc, at, stale, desc, path] =
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
            let described = describe(pid);
            Agent {
                state,
                kind: kind.clone(),
                overridden: described.text.is_some(),
                title: described.text.unwrap_or_else(|| if desc.is_empty() { auto_desc } else { desc }.clone()),
                generating: described.generating,
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

/// A snapshot from `|`-separated FIELDS lines.
#[cfg(test)]
pub fn fixture(lines: &[&str]) -> Snapshot {
    let rows: Vec<Vec<String>> = lines.iter().map(|l| l.splitn(FIELDS.len(), '|').map(String::from).collect()).collect();
    parse(&rows, |_| Described::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_tree() {
        let rows: Vec<Vec<String>> = [
            "$1|work|@1|1|edit|1|%1|0|1|nvim||||||||/w",
            "$1|work|@1|1|edit|1|%2|1|0|claude|working|0|claude|fix it|100|||/w",
            // The same window linked at a second index.
            "$1|work|@1|2|edit|0|%1|0|1|nvim||||||||/w",
            "$1|work|@2|3|logs|0|%3|0|1|tail|idle|1||||||/w|with|bars",
            "$9|_paddock|@9|0|paddock|1|%9|0|1|cat||||||||/",
            "broken|line",
        ]
        .iter()
        .map(|l| l.splitn(FIELDS.len(), '|').map(String::from).collect())
        .collect();
        let snap = parse(&rows, |p| Described { text: (p == "%3").then(|| "override".to_string()), generating: p == "%3" });
        assert_eq!(snap.sessions.len(), 1, "paddock's own session is left out");
        let w = &snap.sessions[0].windows;
        assert_eq!(w.iter().map(|w| (w.id.as_str(), w.index.as_str())).collect::<Vec<_>>(), [("@1", "1"), ("@1", "2"), ("@2", "3")]);
        assert_eq!(w[0].panes.len(), 2);
        let agent = w[0].panes[1].agent.as_ref().unwrap();
        assert_eq!((agent.state, agent.title.as_str(), agent.since), (State::Working, "fix it", Some(100)));
        let agent = w[2].panes[0].agent.as_ref().unwrap();
        assert_eq!((agent.state, agent.title.as_str(), agent.overridden, agent.generating), (State::Finished, "override", true, true));
        assert_eq!(w[2].panes[0].path, "/w|with|bars", "the last field keeps separators");
        assert!(w[0].panes[0].agent.is_none());
    }

    #[test]
    fn description_and_stale() {
        let snap = fixture(&[
            "$1|s|@1|1|w|1|%1|0|1|zsh|working|0|pi|auto|100|1|set|/a",
            "$1|s|@1|1|w|1|%2|1|0|zsh|working|0|pi|auto|100|||/a",
        ]);
        let agents: Vec<_> = snap.unique_panes().filter_map(|(_, _, p)| p.agent.as_ref()).map(|a| (a.title.as_str(), a.stale)).collect();
        assert_eq!(agents, [("set", true), ("auto", false)], "@agent-description wins over @agent-auto-description");
    }

    #[test]
    fn targets_name_their_session() {
        let t = Target::Pane { session: "$1".into(), window: "@2".into(), pane: "%3".into() };
        assert_eq!(t.arg(), "'$1:@2.%3'");
    }
}

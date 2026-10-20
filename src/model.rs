//! A snapshot of every session, window and pane, with the agent options a
//! tool (such as agent-status.tmux) sets on panes, from one format over the
//! whole server; and the targets commands act on.

use crate::tmux::{field, fields, quote, Control, SEP, SESSION};

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
    /// the tool's @agent-auto-description, else the pane title the agent
    /// set (terminal title escape).
    pub auto: String,
    /// Whether the user set the title (@agent-description).
    pub renamed: bool,
    pub since: Option<u64>,
    /// Unconfirmed: the tool says so (@agent-stale), or a shell is the
    /// pane's foreground command, as after the agent went without its exit
    /// hook (killed, crashed). That also holds while a shell runs the agent
    /// without exec (`agent; exec $SHELL`, a wrapper script).
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
    /// tmux names it after what runs (automatic-rename).
    pub auto: bool,
    pub panes: Vec<Pane>,
}

pub struct Session {
    pub id: String,
    pub name: String,
    pub windows: Vec<Window>,
}

pub struct Snapshot {
    pub sessions: Vec<Session>,
    /// Agents per state, indexed by `State as usize`.
    pub counts: [usize; State::ALL.len()],
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

const SESSION_FIELDS: &[&str] = &["session_id", "session_name"];
const WINDOW_FIELDS: &[&str] = &["window_id", "window_index", "window_name", "window_active", "automatic-rename"];
/// A pane's fields, in order: these, the pane title, AGENT_FIELDS, the path.
const PANE_FIELDS: &[&str] = &["pane_id", "pane_index", "pane_active", "pane_current_command"];
/// Read for agent panes (with an @agent-state) only.
const AGENT_FIELDS: &[&str] = &["@agent-state", "@agent-unseen", "@agent-kind", "@agent-auto-description", "@agent-description", "@agent-state-at", "@agent-stale"];

/// What starts a session, a window and a pane in a snapshot's text
/// (`field` keeps them out of the values).
const SESSION_MARK: char = '\u{1c}';
const WINDOW_MARK: char = '\u{1d}';
const PANE_MARK: char = '\u{1e}';

/// The whole server in one line: sessions by name, windows and panes by
/// index (as `list-panes -a` lists them), each as its mark and its
/// `field`s, so a session's and a window's are expanded once, not for each
/// pane. The refresh subscription (main.rs) watches the same text, so it
/// can stand for a snapshot. Other panes leave the agent fields empty, and
/// agents with an automatic title the pane title, which they would not
/// show: a shell sets it at every prompt, an agent animates it.
pub fn format() -> String {
    let agent = format!("#{{?@agent-auto-description,,{}}}{SEP}{}", field("pane_title"), fields(AGENT_FIELDS));
    let none = SEP.to_string().repeat(AGENT_FIELDS.len());
    // The path last: a test fixture's last field may hold its separator.
    let pane = format!("{PANE_MARK}{}{SEP}#{{?@agent-state,{agent},{none}}}{SEP}{}", fields(PANE_FIELDS), field("pane_current_path"));
    let window = format!("{WINDOW_MARK}{}#{{P/i:{pane}}}", fields(WINDOW_FIELDS));
    format!("#{{S/n:{SESSION_MARK}{}#{{W:{window}}}}}", fields(SESSION_FIELDS))
}

/// The snapshot's text (`format()`), now.
pub fn fetch(tmux: &mut Control) -> Result<String, String> {
    let lines = tmux.cmd(&format!("display-message -p {}", quote(&format())))?;
    Ok(lines.into_iter().next().unwrap_or_default())
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

/// Whether COMMAND (pane_current_command) is a shell, which a pane shows at
/// its prompt: login shells start with a dash.
fn is_shell(command: &str) -> bool {
    let name = command.rsplit('/').next().unwrap_or(command).trim_start_matches('-');
    matches!(name, "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "mksh" | "tcsh" | "csh" | "nu")
}

/// Build the tree from a snapshot's TEXT (`format()`), without paddock's
/// own session. HOST is the server's host name, tmux's default pane title.
pub fn parse(text: &str, host: &str) -> Snapshot {
    let n = PANE_FIELDS.len() + AGENT_FIELDS.len() + 2;
    let mut sessions: Vec<Session> = Vec::new();
    for session in text.split(SESSION_MARK).skip(1) {
        let mut windows = session.split(WINDOW_MARK);
        let head: Vec<&str> = windows.next().unwrap_or_default().split(SEP).collect();
        let [id, name] = head[..] else { continue };
        if name == SESSION {
            continue;
        }
        let mut session = Session { id: id.to_string(), name: name.to_string(), windows: Vec::new() };
        // Each mark is a window: the same one can be linked at two indexes.
        for window in windows {
            let mut panes = window.split(PANE_MARK);
            let head: Vec<&str> = panes.next().unwrap_or_default().split(SEP).collect();
            let [id, index, name, active, auto] = head[..] else { continue };
            let mut window = Window { id: id.to_string(), index: index.to_string(), name: name.to_string(), active: active == "1", auto: auto == "1", panes: Vec::new() };
            for pane in panes {
                let f: Vec<&str> = pane.splitn(n, SEP).collect();
                let [id, index, active, command, pane_title, state, unseen, kind, auto, desc, at, stale, path] = f[..] else {
                    continue;
                };
                let agent = State::parse(state, unseen).map(|state| {
                    let auto = if auto.trim().is_empty() { title_of(pane_title, host) } else { auto.trim().to_string() };
                    let desc = desc.trim();
                    Agent {
                        state,
                        kind: kind.to_string(),
                        title: if desc.is_empty() { auto.clone() } else { desc.to_string() },
                        auto,
                        renamed: !desc.is_empty(),
                        since: at.parse().ok(),
                        stale: stale == "1" || is_shell(command),
                    }
                });
                window.panes.push(Pane { id: id.to_string(), index: index.to_string(), active: active == "1", command: command.to_string(), path: path.to_string(), agent });
            }
            session.windows.push(window);
        }
        sessions.push(session);
    }
    let mut snap = Snapshot { sessions, counts: [0; State::ALL.len()] };
    snap.counts = snap.unique_panes().filter_map(|(_, _, p)| p.agent.as_ref()).fold(snap.counts, |mut counts, a| {
        counts[a.state as usize] += 1;
        counts
    });
    snap
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

/// A snapshot from `|`-separated rows of a pane's session, window and pane
/// fields, as `format()` would put them (host "host"). A row too short for
/// a session and window is a pane of the one before.
#[cfg(test)]
pub fn fixture(lines: &[&str]) -> Snapshot {
    let (s, w) = (SESSION_FIELDS.len(), SESSION_FIELDS.len() + WINDOW_FIELDS.len());
    let n = w + PANE_FIELDS.len() + AGENT_FIELDS.len() + 2;
    let sep = SEP.to_string();
    let mut text = String::new();
    let mut last: Vec<&str> = Vec::new();
    for line in lines {
        let f: Vec<&str> = line.splitn(n, '|').collect();
        let mut pane = &f[..];
        if f.len() > w {
            if last.len() <= w || last[..s] != f[..s] {
                text += &format!("{SESSION_MARK}{}", f[..s].join(&sep));
            }
            if last.len() <= w || last[..w] != f[..w] {
                text += &format!("{WINDOW_MARK}{}", f[s..w].join(&sep));
            }
            pane = &f[w..];
            last = f.clone();
        }
        text += &format!("{PANE_MARK}{}", pane.join(&sep));
    }
    parse(&text, "host")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_tree() {
        let snap = fixture(&[
            "$1|work|@1|1|edit|1|0|%1|0|1|nvim|||||||||/w",
            "$1|work|@1|1|edit|1|0|%2|1|0|claude||working|0|claude|fix it||100||/w",
            // The same window linked at a second index.
            "$1|work|@1|2|edit|0|0|%1|0|1|nvim|||||||||/w",
            "$1|work|@2|3|logs|0|1|%3|0|1|tail||idle|1||auto|mine|||/w|with|bars",
            "broken|line",
            "$9|_paddock|@9|0|paddock|1|0|%9|0|1|cat|||||||||/",
        ]);
        assert_eq!(snap.sessions.len(), 1, "paddock's own session is left out");
        let w = &snap.sessions[0].windows;
        assert_eq!(w.iter().map(|w| (w.id.as_str(), w.index.as_str())).collect::<Vec<_>>(), [("@1", "1"), ("@1", "2"), ("@2", "3")]);
        assert_eq!(w[0].panes.len(), 2);
        assert_eq!(w.iter().map(|w| w.auto).collect::<Vec<_>>(), [false, false, true], "named by tmux (automatic-rename)");
        let agent = w[0].panes[1].agent.as_ref().unwrap();
        assert_eq!((agent.state, agent.title.as_str(), agent.since, agent.renamed), (State::Working, "fix it", Some(100), false));
        let agent = w[2].panes[0].agent.as_ref().unwrap();
        assert_eq!((agent.state, agent.title.as_str(), agent.auto.as_str(), agent.renamed), (State::Finished, "mine", "auto", true));
        assert_eq!(w[2].panes[0].path, "/w|with|bars", "the last field keeps separators");
        assert_eq!(w[2].panes.len(), 1, "a broken pane is skipped");
        assert!(w[0].panes[0].agent.is_none());
    }

    #[test]
    fn titles_fall_back_to_the_pane_title() {
        let snap = fixture(&[
            "$1|s|@1|1|w|1|0|%1|0|1|claude|✳ Fix the parser|working|0|claude||||1|/a",
            "$1|s|@1|1|w|1|0|%2|1|0|pi|host|working|0|pi|||||/a",
            "$1|s|@1|1|w|1|0|%3|2|0|pi|⠋ x|working|0|pi|first prompt||||/a",
        ]);
        let agents: Vec<_> = snap.unique_panes().filter_map(|(_, _, p)| p.agent.as_ref()).map(|a| (a.title.as_str(), a.stale)).collect();
        assert_eq!(agents, [("Fix the parser", true), ("", false), ("first prompt", false)], "status glyphs dropped; the host name is no title");
        assert_eq!(title_of("pmac", "pmac.local"), "", "the short host name too");
    }

    #[test]
    fn an_agent_whose_pane_is_back_at_a_shell_is_unconfirmed() {
        let snap = fixture(&[
            "$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|||||/a",
            "$1|s|@1|1|w|1|0|%2|1|0|-zsh||working|0|claude|||||/a",
            "$1|s|@1|1|w|1|0|%3|2|0|bash||idle|0|pi|||||/a",
            "$1|s|@1|1|w|1|0|%4|3|0|node||asking|0|pi||||1|/a",
        ]);
        let stale: Vec<_> = snap.unique_panes().filter_map(|(_, _, p)| p.agent.as_ref()).map(|a| a.stale).collect();
        assert_eq!(stale, [false, true, true, true], "killed without its exit hook, or the tool says so");
    }

    #[test]
    fn rows_have_every_field_with_or_without_an_agent() {
        // tmux expands one branch of `#{?@agent-state,AGENT,NONE}`.
        let none = SEP.to_string().repeat(AGENT_FIELDS.len());
        let f = format();
        let (_, gated) = f.split_once("#{?@agent-state,").unwrap();
        let (agent, _) = gated.split_once(&format!(",{none}}}")).unwrap();
        // Not counting the SEP in `field`'s regex.
        assert_eq!(agent.replace("[\x01-\x1f]", "").matches(SEP).count(), AGENT_FIELDS.len());
    }

    #[test]
    fn targets_name_their_session() {
        let t = Target::Pane { session: "$1".into(), window: "@2".into(), pane: "%3".into() };
        assert_eq!(t.arg(), "'$1:@2.%3'");
        assert_eq!(Target::Window { session: "$1".into(), window: "@2".into() }.arg(), "'$1:@2'");
    }
}

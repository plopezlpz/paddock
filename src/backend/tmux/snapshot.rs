//! A tmux server's snapshot: every session, window and pane, with the agent
//! options a tool (such as agent-status.tmux) sets on panes, from one format
//! over the whole server.

use super::control::{field, fields, quote, Control, SEP, SESSION};
use crate::model::{is_shell, title_of, Agent, Pane, Session, Snapshot, State, Window};

const SESSION_FIELDS: &[&str] = &["session_id", "session_name"];
const WINDOW_FIELDS: &[&str] = &["window_id", "window_index", "window_name", "window_active", "automatic-rename"];
/// A pane's fields, in order: these, the pane title, AGENT_FIELDS, the path.
const PANE_FIELDS: &[&str] = &["pane_id", "pane_index", "pane_active", "pane_current_command"];
/// Read for agent panes (with an @agent-state) only.
const AGENT_FIELDS: &[&str] = &["@agent-state", "@agent-unseen", "@agent-kind", "@agent-auto-description", "@agent-description", "@agent-state-at", "@agent-stale", "@paddock-hidden"];

/// What starts a session, a window and a pane in a snapshot's text
/// (`field` keeps them out of the values).
const SESSION_MARK: char = '\u{1c}';
const WINDOW_MARK: char = '\u{1d}';
const PANE_MARK: char = '\u{1e}';

/// The whole server in one line: its host name (tmux's default pane title,
/// which is no agent's), then sessions by name, windows and panes by
/// index (as `list-panes -a` lists them), each as its mark and its
/// `field`s, so a session's and a window's are expanded once, not for each
/// pane. The refresh subscription (mod.rs) watches the same text, so it
/// can stand for a snapshot. Other panes leave the agent fields empty, and
/// agents with an automatic title the pane title, which they would not
/// show: a shell sets it at every prompt, an agent animates it.
pub fn format() -> String {
    let agent = format!("#{{?@agent-auto-description,,{}}}{SEP}{}", field("pane_title"), fields(AGENT_FIELDS));
    let none = SEP.to_string().repeat(AGENT_FIELDS.len());
    // The path last: a test fixture's last field may hold its separator.
    let pane = format!("{PANE_MARK}{}{SEP}#{{?@agent-state,{agent},{none}}}{SEP}{}", fields(PANE_FIELDS), field("pane_current_path"));
    let window = format!("{WINDOW_MARK}{}#{{P/i:{pane}}}", fields(WINDOW_FIELDS));
    format!("{}#{{S/n:{SESSION_MARK}{}#{{W:{window}}}}}", field("host"), fields(SESSION_FIELDS))
}

/// The server's snapshot, now.
pub fn fetch(tmux: &mut Control) -> Result<Snapshot, String> {
    let lines = tmux.cmd(&format!("display-message -p {}", quote(&format())))?;
    Ok(parse(lines.first().map_or("", String::as_str)))
}

/// Build the tree from a snapshot's TEXT (`format()`), without paddock's
/// own session.
pub fn parse(text: &str) -> Snapshot {
    let n = PANE_FIELDS.len() + AGENT_FIELDS.len() + 2;
    let mut sessions: Vec<Session> = Vec::new();
    let mut marked = text.split(SESSION_MARK);
    let host = marked.next().unwrap_or_default();
    for session in marked {
        let mut windows = session.split(WINDOW_MARK);
        let head: Vec<&str> = windows.next().unwrap_or_default().split(SEP).collect();
        let [id, name] = head[..] else { continue };
        if name == SESSION {
            continue;
        }
        let mut session = Session { id: id.to_string(), name: name.to_string(), host: String::new(), away: false, windows: Vec::new() };
        // Each mark is a window: the same one can be linked at two indexes.
        for window in windows {
            let mut panes = window.split(PANE_MARK);
            let head: Vec<&str> = panes.next().unwrap_or_default().split(SEP).collect();
            let [id, index, name, active, auto] = head[..] else { continue };
            let mut window = Window { id: id.to_string(), index: index.to_string(), name: name.to_string(), active: active == "1", auto: auto == "1", panes: Vec::new() };
            for pane in panes {
                let f: Vec<&str> = pane.splitn(n, SEP).collect();
                let [id, index, active, command, pane_title, state, unseen, kind, auto, desc, at, stale, hidden, path] = f[..] else {
                    continue;
                };
                let agent = agent_state(state, unseen).map(|state| {
                    let auto = if auto.trim().is_empty() { title_of(pane_title, host) } else { auto.trim().to_string() };
                    let desc = desc.trim();
                    Agent {
                        state,
                        kind: kind.to_string(),
                        title: if desc.is_empty() { auto.clone() } else { desc.to_string() },
                        auto,
                        renamed: !desc.is_empty(),
                        since: at.parse().ok(),
                        hidden: hidden == "1",
                        stale: stale == "1" || is_shell(command),
                        note: String::new(),
                    }
                });
                window.panes.push(Pane { id: id.to_string(), index: index.to_string(), active: active == "1", command: command.to_string(), path: path.to_string(), agent });
            }
            session.windows.push(window);
        }
        sessions.push(session);
    }
    Snapshot::new(sessions)
}

/// An agent state from its @agent-state and @agent-unseen.
fn agent_state(state: &str, unseen: &str) -> Option<State> {
    match state {
        "asking" => Some(State::Asking),
        "working" => Some(State::Working),
        "idle" if unseen == "1" => Some(State::Finished),
        "idle" => Some(State::Idle),
        _ => None,
    }
}

/// A snapshot from `|`-separated rows of a pane's session, window and pane
/// fields, as `format()` would put them (host "host"). A row too short for
/// a session and window is a pane of the one before.
#[cfg(test)]
pub fn fixture(lines: &[&str]) -> Snapshot {
    fixture_hidden(lines, &[])
}

/// The legacy fixture columns, with HIDDEN pane IDs marked hidden.
#[cfg(test)]
pub fn fixture_hidden(lines: &[&str], hidden: &[&str]) -> Snapshot {
    let (s, w) = (SESSION_FIELDS.len(), SESSION_FIELDS.len() + WINDOW_FIELDS.len());
    let n = w + PANE_FIELDS.len() + AGENT_FIELDS.len() + 1;
    let sep = SEP.to_string();
    let mut text = String::from("host");
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
        if let Some((path, rest)) = pane.split_last() {
            let flag = if rest.first().is_some_and(|id| hidden.contains(id)) { "1" } else { "" };
            text += &format!("{PANE_MARK}{}{SEP}{flag}{SEP}{path}", rest.join(&sep));
        }
    }
    parse(&text)
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
    fn hidden_is_a_pane_preference_not_an_agent_state() {
        let lines = [
            "$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|task||100||/a|with|bars",
            "$2|t|@1|1|w|1|0|%1|0|1|claude||working|0|claude|task||100||/a|with|bars",
            "$2|t|@2|2|v|0|0|%2|0|1|pi||idle|0|pi|other||||/b",
        ];
        let snap = fixture_hidden(&lines, &["%1"]);
        let a = snap.agent("%1").unwrap();
        assert!(a.hidden);
        assert_eq!((a.state, a.title.as_str(), a.since), (State::Working, "task", Some(100)));
        assert_eq!(snap.pane("%1").unwrap().2.path, "/a|with|bars");
        assert_eq!(snap.pane("%1").unwrap().2.command, "claude");
        assert!(snap.sessions[1].windows[0].panes[0].agent.as_ref().unwrap().hidden, "linked pane shares its flag");
        assert!(!snap.agent("%2").unwrap().hidden);
        assert_eq!(snap.counts, [1, 0, 1, 0], "overall counts still include hidden agents, once");
        assert_eq!(snap.sessions[0].windows[0].state(), Some(State::Working));
        assert!(!fixture(&lines).agent("%1").unwrap().hidden, "unset is visible");
        assert!(format().contains("@paddock-hidden"), "visibility changes trigger the snapshot subscription");
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
}

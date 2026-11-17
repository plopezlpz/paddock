//! The rows each tab shows: pure functions of a snapshot, the filter and the
//! tab's settings, and the display text they share.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::model::{Mark, Snapshot, State, Target};
use crate::paths::{AgentPath, PathCache};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Status,
    Session,
    Folder,
    /// No headers (shown as "none").
    Flat,
}

impl Group {
    pub fn next(self) -> Group {
        match self {
            Group::Status => Group::Session,
            Group::Session => Group::Folder,
            Group::Folder => Group::Flat,
            Group::Flat => Group::Status,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Group::Status => "status",
            Group::Session => "session",
            Group::Folder => "folder",
            Group::Flat => "none",
        }
    }
}

pub struct Row {
    /// None for headers (only Hidden is selectable) and the "+ new" row,
    /// which names nothing on the server yet.
    pub target: Option<Target>,
    pub kind: RowKind,
    /// Pane shown in the preview.
    pub preview: Option<String>,
    /// How well the row itself matches the filter (None: shown for context).
    pub score: Option<u32>,
}

/// What a row shows. In the tree, `last` (the last window of its session,
/// the last pane of its window) draws └─ instead of ├─, and a pane's
/// `parent_last` leaves its window's │ out. Window paths use the same Git-aware
/// label as agents, muted after the name (or under it, compact).
pub enum RowKind {
    /// `current`: the session the client shows.
    /// `host`: the Rex host it lives on, if not paddock's own server.
    /// AWAY: its server is failing, and this is the last it said.
    Session { name: String, host: String, away: bool, windows: usize, current: bool },
    /// `active`: the session's current window. `expanded`: None for a
    /// single pane (nothing to list), else whether its panes are listed.
    Window { index: String, name: String, path: AgentPath, panes: usize, state: Option<State>, active: bool, last: bool, expanded: Option<bool> },
    Pane { index: String, command: String, state: Option<State>, last: bool, parent_last: bool },
    /// `note`: what the agent last said (shown while it is news: not idle).
    /// `host`: its session's Rex host, if not paddock's own server. `place`:
    /// where it runs, `session:window` (the window alone under its
    /// session's header; an automatically named window by its index).
    /// DIFF: the changes waiting in its folder (`+12 −3`), empty if none or
    /// not known.
    Agent { state: State, stale: bool, kind: String, title: String, note: String, path: AgentPath, age: String, host: String, place: String, diff: String },
    /// `gap`: not the first header (wide, a blank line goes above it).
    Header { label: String, count: usize, gap: bool },
    /// One selectable heading for all hidden cards, without internal headers.
    Hidden { count: usize, expanded: bool, gap: bool },
    /// The "+ new" row being named (App's create mode says what it is).
    New,
}

/// What the "+ new" row being named creates (^s, ^s^s).
#[derive(Clone, Debug, PartialEq)]
pub enum New {
    /// A window after window AFTER of SESSION, in DIR.
    Window { session: String, after: String, dir: String },
    /// A session in DIR.
    Session { dir: String },
}

impl Row {
    /// Screen lines. Agents take two lines; compact (a phone), windows
    /// do too. Wide, a header after the first has a blank line above it.
    pub fn height(&self, compact: bool) -> u16 {
        match self.kind {
            RowKind::Agent { .. } => 2,
            RowKind::Window { .. } if compact => 2,
            RowKind::Header { gap: true, .. } | RowKind::Hidden { gap: true, .. } if !compact => 2,
            _ => 1,
        }
    }
}

/// The query, parsed once per change, and the matcher it reuses (also
/// while drawing, for `hits`).
pub struct Filter {
    pattern: Pattern,
    matcher: RefCell<Matcher>,
    buf: RefCell<Vec<char>>,
}

impl Filter {
    pub fn new() -> Filter {
        Filter { pattern: Pattern::default(), matcher: RefCell::new(Matcher::new(Config::DEFAULT.match_paths())), buf: RefCell::default() }
    }

    pub fn set(&mut self, query: &str) {
        self.pattern.reparse(query, CaseMatching::Smart, Normalization::Smart);
    }

    /// Whether the query has any terms (blank or lone `!` ones have none).
    pub fn active(&self) -> bool {
        !self.pattern.atoms.is_empty()
    }

    /// None when TEXT does not match; everything matches with score 0 when
    /// the filter is inactive (as does a negation-only query).
    pub fn score(&mut self, text: &str) -> Option<u32> {
        if !self.active() {
            return Some(0);
        }
        self.pattern.score(Utf32Str::new(text, self.buf.get_mut()), self.matcher.get_mut())
    }

    /// Where in TEXT, one piece of a row that matched as a whole, each term
    /// matches on its own: grapheme indices, sorted, for highlighting.
    pub fn hits(&self, text: &str) -> Vec<u32> {
        let mut out = Vec::new();
        if !self.active() {
            return out;
        }
        let (mut matcher, mut buf) = (self.matcher.borrow_mut(), self.buf.borrow_mut());
        let haystack = Utf32Str::new(text, &mut buf);
        for atom in self.pattern.atoms.iter().filter(|a| !a.negative) {
            atom.indices(haystack, &mut matcher, &mut out);
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// A folder's search text, each form once (a repeat would let a fuzzy term
/// match across the copies): the path as the server gives it, its ~ form,
/// the Git-aware label where it says more (a worktree's main repo), and the
/// full branch or commit.
fn path_text(abs: &str, path: &AgentPath) -> String {
    let search = path.search_text();
    // "label head": branch names and commit ids have no spaces.
    let (label, head) = search.rsplit_once(' ').unwrap_or((&search, ""));
    let mut out = abs.to_string();
    for part in [path.full.as_str(), label.trim_start_matches("…/"), head] {
        if !part.is_empty() && !out.contains(part) {
            out.push(' ');
            out.push_str(part);
        }
    }
    out
}

/// S cut to MAX display columns, ending in … when cut. Whole graphemes: an
/// emoji sequence (VS16, ZWJ, skin tone) is kept or cut as one.
pub fn fit(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for g in s.graphemes(true) {
        let w = g.width();
        if used + w + 1 > max {
            break;
        }
        out.push_str(g);
        used += w;
    }
    if max > 0 {
        out.push('…');
    }
    out
}

/// A remote pane's path (Rex's `host:/path`, or `host:~/path` as
/// `home_path` gives it) as its host and the path there; a local path has
/// no host.
pub fn split_host(path: &str) -> (&str, &str) {
    // At the first colon before a path: a label may have its own (a port,
    // an IPv6 address).
    let at = [path.find(":/"), path.find(":~")].into_iter().flatten().min();
    match at.map(|i| (&path[..i], &path[i + 1..])) {
        Some((host, rest)) if !host.is_empty() && !host.contains(['/', '~']) => (host, rest),
        _ => ("", path),
    }
}

/// PATH relative to HOME as `~`, on a host too (the same user's home there
/// is the same folder, as with one home on several machines).
pub fn home_path(path: &str, home: &str) -> String {
    let (host, path) = split_host(path);
    let host = if host.is_empty() { String::new() } else { format!("{host}:") };
    match path.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("{host}~{rest}"),
        _ => format!("{host}{path}"),
    }
}

/// `home_path`, shortened to its last two folders when longer than 32
/// columns; a host's path keeps its `host:`.
pub fn short_path(path: &str, home: &str) -> String {
    let p = home_path(path, home);
    if p.width() <= 32 {
        return p;
    }
    let (host, rest) = split_host(&p);
    let parts: Vec<&str> = rest.rsplitn(3, '/').collect();
    match parts.as_slice() {
        [last, parent, _] if host.is_empty() => format!("…/{parent}/{last}"),
        [last, parent, _] => format!("{host}:…/{parent}/{last}"),
        _ => p,
    }
}

/// Compact time since EPOCH: <1m, 5m, 3h, 2d; nothing when unknown.
pub fn age(since: Option<u64>, now: u64) -> String {
    let Some(at) = since else { return String::new() };
    let minutes = now.saturating_sub(at) / 60;
    match minutes {
        0 => "<1m".into(),
        m if m < 60 => format!("{m}m"),
        m if m < 1440 => format!("{}h", m / 60),
        m => format!("{}d", m / 1440),
    }
}

/// "session:window.pane · what it runs" for the preview header, with
/// " @host" after the pane when the session is on a Rex host.
pub fn pane_title(snap: &Snapshot, id: &str) -> String {
    snap.pane(id)
        .map(|(s, w, p)| {
            let at = if s.host.is_empty() { String::new() } else { format!(" @{}", s.host) };
            format!("{}:{}.{}{at} · {}", s.name, w.index, p.index, p.label())
        })
        .unwrap_or_default()
}

/// What the tree shows besides the snapshot and the filter.
pub struct Tree<'a> {
    /// Windows whose panes are listed.
    pub expanded: &'a HashSet<Target>,
    /// Windows the filter would open (matching panes) but the user closed.
    pub collapsed: &'a HashSet<Target>,
    /// Listed whatever the filter says: the row just renamed or moved, so it
    /// doesn't vanish under the user's eyes.
    pub keep: Option<&'a Target>,
    /// The session the client shows (it follows a refuge move).
    pub current: &'a str,
}

/// Session › window tree. A window shows its panes when expanded or, while
/// filtering, the panes that match when the window itself does not, to
/// show why it is listed: a session's name matches its windows without
/// listing every pane.
pub fn sessions(snap: &Snapshot, filter: &mut Filter, tree: &Tree, paths: &mut PathCache) -> Vec<Row> {
    let filtering = filter.active();
    let kept = |t: &Target| tree.keep == Some(t);
    let mut out = Vec::new();
    for s in &snap.sessions {
        // (window row, its pane rows), before the tree lines are known.
        let mut windows: Vec<(Row, Vec<Row>)> = Vec::new();
        for w in &s.windows {
            let target = Target::Window { session: s.id.clone(), window: w.id.clone() };
            let active = w.active_pane();
            // Match full paths and Git metadata even when shortened on screen.
            let path = active.map(|p| paths.get(&p.path)).unwrap_or_else(|| AgentPath::plain("", paths.home()));
            let text = format!("{} {} {} {}", s.name, w.index, w.name, path_text(active.map_or("", |p| p.path.as_str()), &path));
            let score = filter.score(&text);
            let pane_target = |p: &crate::model::Pane| Target::Pane { session: s.id.clone(), window: w.id.clone(), pane: p.id.clone() };
            // A pane matches with its window's text, so every term (and
            // negation) applies to the whole line.
            let pane_scores: Vec<Option<u32>> = if filtering && score.is_none() {
                w.panes
                    .iter()
                    .map(|p| {
                        // What the pane adds to its window's text.
                        let mut own = format!("{text} {}", p.command);
                        if p.label() != p.command {
                            own = format!("{own} {}", p.label());
                        }
                        if active.is_none_or(|a| a.path != p.path) {
                            own = format!("{own} {}", path_text(&p.path, &paths.get(&p.path)));
                        }
                        filter.score(&own)
                    })
                    .collect()
            } else {
                vec![None; w.panes.len()]
            };
            let pane_kept: Vec<bool> = w.panes.iter().map(|p| kept(&pane_target(p))).collect();
            if filtering && score.is_none() && !kept(&target) && pane_scores.iter().zip(&pane_kept).all(|(s, k)| s.is_none() && !k) {
                continue;
            }
            let many = w.panes.len() > 1;
            let open = many && tree.expanded.contains(&target);
            let explains = pane_scores.iter().zip(&pane_kept).any(|(s, k)| s.is_some() || *k);
            let listed = open || (explains && !tree.collapsed.contains(&target));
            let row = Row {
                target: Some(target),
                kind: RowKind::Window {
                    index: w.index.clone(),
                    name: w.name.clone(),
                    path,
                    panes: w.panes.len(),
                    state: w.state(),
                    active: w.active,
                    last: false,
                    expanded: many.then_some(listed),
                },
                preview: active.map(|p| p.id.clone()),
                // A single pane's match is the window's. (A window can have
                // none: parse skips a malformed pane.)
                score: if many { score } else { score.or_else(|| pane_scores.first().copied().flatten()) },
            };
            let mut panes = Vec::new();
            if many && listed {
                for ((p, pscore), keep) in w.panes.iter().zip(pane_scores).zip(pane_kept) {
                    if !open && pscore.is_none() && !keep {
                        continue;
                    }
                    panes.push(Row {
                        target: Some(pane_target(p)),
                        kind: RowKind::Pane {
                            index: p.index.clone(),
                            command: p.label().to_string(),
                            state: p.agent.as_ref().map(|a| a.state),
                            last: false,
                            parent_last: false,
                        },
                        preview: Some(p.id.clone()),
                        // Unfiltered, every row matches.
                        score: pscore.or((!filtering).then_some(0)),
                    });
                }
            }
            windows.push((row, panes));
        }
        let session = Target::Session { session: s.id.clone() };
        let score = filter.score(&if s.host.is_empty() { s.name.clone() } else { format!("{} {}", s.name, s.host) });
        if filtering && windows.is_empty() && score.is_none() && !kept(&session) {
            continue;
        }
        out.push(Row {
            target: Some(session),
            kind: RowKind::Session { name: s.name.clone(), host: s.host.clone(), away: s.away, windows: s.windows.len(), current: s.id == tree.current },
            preview: s.active_window().and_then(|w| w.active_pane()).map(|p| p.id.clone()),
            score,
        });
        // Tree lines follow what is listed, not every window and pane.
        let n = windows.len();
        for (i, (mut row, mut panes)) in windows.into_iter().enumerate() {
            let window_last = i + 1 == n;
            if let RowKind::Window { last, .. } = &mut row.kind {
                *last = window_last;
            }
            let m = panes.len();
            for (j, pane) in panes.iter_mut().enumerate() {
                if let RowKind::Pane { last, parent_last, .. } = &mut pane.kind {
                    *last = j + 1 == m;
                    *parent_last = window_last;
                }
            }
            out.push(row);
            out.extend(panes);
        }
    }
    out
}

/// Put the "+ new" row for NEW where the window or session will appear:
/// after the window it follows (and its listed panes), else at the end of
/// that session's rows; a session at the very end.
pub fn place_new(rows: &mut Vec<Row>, new: &New) {
    let at = match new {
        New::Window { session, after, .. } => {
            let window = Target::Window { session: session.clone(), window: after.clone() };
            let in_session = |r: &Row| match &r.target {
                Some(Target::Session { session: s } | Target::Window { session: s, .. } | Target::Pane { session: s, .. }) => s == session,
                _ => false,
            };
            match rows.iter().position(|r| r.target.as_ref() == Some(&window)) {
                Some(i) => i + 1 + rows[i + 1..].iter().take_while(|r| matches!(r.kind, RowKind::Pane { .. })).count(),
                None => rows.iter().rposition(in_session).map_or(rows.len(), |i| i + 1),
            }
        }
        New::Session { .. } => rows.len(),
    };
    rows.insert(at, Row { target: None, kind: RowKind::New, preview: None, score: None });
}

/// Groups in this order; one kind per build.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum GroupKey {
    /// Most urgent first.
    Status(Reverse<State>),
    /// By label, then what tells groups with one label apart.
    Name(String, String),
    All,
}

/// A card with its group, the group's header label and its sort key (state,
/// then when it entered it).
type GroupItem = (GroupKey, String, (State, Option<u64>), Row);

/// One card per agent pane, grouped under headers (none for Group::Flat);
/// inside a group the most urgent state comes first, then the agent that
/// entered it most recently.
/// Hidden cards have the same ordering, under one collapsible heading without
/// internal group headers. Search reveals them without changing the preference.
/// KEEP is listed whatever the filter says (see `Tree::keep`).
#[cfg(test)]
pub fn agents(snap: &Snapshot, filter: &mut Filter, group: Group, keep: Option<&Target>, paths: &mut PathCache, now: u64, hidden_expanded: bool) -> Vec<Row> {
    agents_noted(snap, filter, group, keep, paths, now, hidden_expanded, &Notes::new())
}

/// What transcripts say (transcript.rs), by file: what for (the agent's
/// mark when it was read), and what.
pub type Notes = HashMap<String, (Mark, String)>;

/// Where an agent runs, as its card shows it: `session:window`, or the
/// window alone (IN_SESSION: under its session's header). A window named
/// after what runs in it says nothing an agent card doesn't: its index.
fn place(s: &crate::model::Session, w: &crate::model::Window, in_session: bool) -> String {
    let auto = w.auto || w.name.is_empty();
    let window = if auto { &w.index } else { &w.name };
    match (in_session, auto) {
        (true, true) => format!(":{window}"),
        (true, false) => window.clone(),
        (false, _) => format!("{}:{window}", s.name),
    }
}

/// `agents`, with an agent that has no note of its own given what its
/// transcript says in NOTES (shown and searched alike).
#[allow(clippy::too_many_arguments)]
pub fn agents_noted(snap: &Snapshot, filter: &mut Filter, group: Group, keep: Option<&Target>, paths: &mut PathCache, now: u64, hidden_expanded: bool, notes: &Notes) -> Vec<Row> {
    let mut items: Vec<GroupItem> = Vec::new();
    let mut hidden: Vec<GroupItem> = Vec::new();
    for (s, w, p) in snap.unique_panes() {
        let Some(a) = &p.agent else { continue };
        let note = match notes.get(&a.transcript) {
            Some((mark, said)) if a.note.is_empty() && *mark == a.mark() => said,
            _ => &a.note,
        };
        let target = Target::Pane { session: s.id.clone(), window: w.id.clone(), pane: p.id.clone() };
        let path = paths.get(&p.path);
        let kind = p.label();
        // The state as shown, and asking as tools and notifications say it.
        let asking = if a.state == State::Asking { " asking" } else { "" };
        // Its session, window and host too: a card shows them (its place,
        // `@host`).
        let score = filter.score(&format!("{} {} {} {} {} {}{asking}{} {}", a.title, place(s, w, false), s.host, path_text(&p.path, &path), kind, a.state.label(), if a.stale { " stale" } else { "" }, note));
        if score.is_none() && keep != Some(&target) {
            continue;
        }
        let (key, label) = match group {
            Group::Status => (GroupKey::Status(Reverse(a.state)), a.state.label().to_string()),
            // Rex allows one name twice, and hosts may share one.
            Group::Session => {
                let label = if s.host.is_empty() { s.name.clone() } else { format!("{} @{}", s.name, s.host) };
                (GroupKey::Name(label.clone(), s.id.clone()), label)
            }
            // A repository's worktrees and folders together.
            Group::Folder => match path.repo() {
                Some((name, anchor)) => (GroupKey::Name(name.to_string(), anchor.to_string()), name.to_string()),
                None => (GroupKey::Name(p.path.clone(), String::new()), short_path(&p.path, paths.home())),
            },
            Group::Flat => (GroupKey::All, String::new()),
        };
        let row = Row {
            target: Some(target),
            kind: RowKind::Agent {
                state: a.state,
                stale: a.stale,
                kind: kind.to_string(),
                title: a.title.clone(),
                note: note.clone(),
                path,
                age: age(a.since, now),
                host: s.host.clone(),
                place: place(s, w, group == Group::Session && !a.hidden),
                diff: String::new(),
            },
            preview: Some(p.id.clone()),
            score,
        };
        let into = if a.hidden { &mut hidden } else { &mut items };
        into.push((key, label, (a.state, a.since), row));
    }
    // Both lists use the same order. Descending: no time (None) sorts last;
    // equal times keep tmux's order.
    for list in [&mut items, &mut hidden] {
        list.sort_by(|a, b| a.0.cmp(&b.0).then(b.2.cmp(&a.2)));
    }
    // Repositories sharing a name are told apart by where they are.
    let mut seen: Vec<(&str, &GroupKey)> = Vec::new();
    let mut shared: Vec<String> = Vec::new();
    for (key, label, ..) in &items {
        if seen.iter().any(|(l, k)| *l == label && *k != key) {
            shared.push(label.clone());
        } else {
            seen.push((label, key));
        }
    }
    let mut out = Vec::new();
    let mut items = items.into_iter().peekable();
    while let Some((key, mut label, _, row)) = items.next() {
        if let GroupKey::Name(_, anchor) = &key
            && group == Group::Folder && !anchor.is_empty() && shared.contains(&label)
        {
            label = short_path(anchor, paths.home());
        }
        let mut group_rows = vec![row];
        while let Some(it) = items.next_if(|it| it.0 == key) {
            group_rows.push(it.3);
        }
        if group != Group::Flat {
            out.push(Row { target: None, kind: RowKind::Header { label, count: group_rows.len(), gap: !out.is_empty() }, preview: None, score: None });
        }
        out.extend(group_rows);
    }
    if !hidden.is_empty() {
        // Searching reveals matches without changing the saved preference.
        let expanded = hidden_expanded || filter.active();
        out.push(Row { target: None, kind: RowKind::Hidden { count: hidden.len(), expanded, gap: !out.is_empty() }, preview: None, score: None });
        if expanded {
            out.extend(hidden.into_iter().map(|it| it.3));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::tmux::fixture;

    fn snap() -> Snapshot {
        fixture(&[
            "$1|work|@1|1|edit|1|0|%1|0|1|nvim|||||||||/h/a/src",
            "$1|work|@1|1|edit|1|0|%2|1|0|claude||working|0|claude|fix parser||||/h/a/src",
            "$1|work|@2|2|logs|0|0|%3|0|1|tail||asking|0|codex|deploy||||/h/b/src",
            "$2|play|@3|1|game|1|0|%4|0|1|zsh|||||||||/h/c",
            // @2 linked into play too.
            "$2|play|@2|2|logs|0|0|%3|0|1|tail||asking|0|codex|deploy||||/h/b/src",
        ])
    }

    fn agents(snap: &Snapshot, filter: &mut Filter, group: Group, keep: Option<&Target>, home: &str, now: u64, hidden_expanded: bool) -> Vec<Row> {
        super::agents(snap, filter, group, keep, &mut PathCache::new(home.into()), now, hidden_expanded)
    }

    fn sessions(snap: &Snapshot, filter: &mut Filter, tree: &Tree) -> Vec<Row> {
        super::sessions(snap, filter, tree, &mut PathCache::new("/h".into()))
    }

    fn filter(q: &str) -> Filter {
        let mut f = Filter::new();
        f.set(q);
        f
    }

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match &r.kind {
                RowKind::Session { name, .. } => name.clone(),
                RowKind::Window { name, .. } => format!("  {name}"),
                RowKind::Pane { command, .. } => format!("    {command}"),
                RowKind::Agent { title, .. } => format!("  {title}"),
                RowKind::Header { label, count, .. } => format!("{label} ({count})"),
                RowKind::Hidden { count, .. } => format!("hidden ({count})"),
                RowKind::New => "new".into(),
            })
            .collect()
    }

    fn tree<'a>(expanded: &'a HashSet<Target>, collapsed: &'a HashSet<Target>, keep: Option<&'a Target>) -> Tree<'a> {
        Tree { expanded, collapsed, keep, current: "$1" }
    }

    fn sessions_for(q: &str) -> Vec<String> {
        let none = HashSet::new();
        names(&sessions(&snap(), &mut filter(q), &tree(&none, &none, None)))
    }

    #[test]
    fn agent_kind_is_searchable_without_repeating_it_in_card_metadata() {
        let snap = snap();
        let rows = agents(&snap, &mut filter("claude"), Group::Flat, None, "/h", 0, false);
        assert_eq!(rows.len(), 1);
        let RowKind::Agent { kind, path, .. } = &rows[0].kind else { panic!("expected an agent card") };
        assert_eq!(kind, "claude");
        assert_eq!(path.display(80), "~/a/src");
        assert_eq!(pane_title(&snap, rows[0].preview.as_deref().unwrap()), "work:1.1 · claude", "no host: none shown");
    }

    #[test]
    fn agents_match_absolute_and_home_paths_even_when_shortened_or_hidden() {
        let line = "$1|session|@1|1|window|1|0|%1|0|1|claude||working|0|claude|task||||/h/source/company/main-repo/worktrees/topic/src";
        for hidden in [false, true] {
            let snap = if hidden { crate::backend::tmux::fixture_hidden(&[line], &["%1"]) } else { fixture(&[line]) };
            for query in ["company", "'/h/source/company", "'~/source/company", "main-repo", "worktrees/topic", "session", "claude", "working", "task"] {
                let rows = agents(&snap, &mut filter(query), Group::Flat, None, "/h", 0, false);
                assert_eq!(rows.iter().filter(|r| matches!(r.kind, RowKind::Agent { .. })).count(), 1, "{query}, hidden={hidden}");
                let path = rows.iter().find_map(|r| match &r.kind { RowKind::Agent { path, .. } => Some(path), _ => None }).unwrap();
                assert_eq!(path.full, "~/source/company/main-repo/worktrees/topic/src");
                assert_eq!(path.display(30), "…/topic/src");
            }
            assert!(agents(&snap, &mut filter("!company"), Group::Flat, None, "/h", 0, false).is_empty());
        }
    }

    #[test]
    fn a_folder_is_searched_once_in_each_form() {
        let mut paths = PathCache::new("/h".into());
        assert_eq!(path_text("/h/a/src", &paths.get("/h/a/src")), "/h/a/src ~/a/src");
        assert_eq!(path_text("/srv/a/very/long/path/to/some/project", &paths.get("/srv/a/very/long/path/to/some/project")), "/srv/a/very/long/path/to/some/project");
        // Three copies of /srv/app let a fuzzy term run across them.
        let snap = fixture(&["$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|t||||/srv/app"]);
        assert!(agents(&snap, &mut filter("pas"), Group::Flat, None, "/h", 0, false).is_empty());
        assert_eq!(agents(&snap, &mut filter("srv/app"), Group::Flat, None, "/h", 0, false).len(), 1);
    }

    #[test]
    fn hits_mark_each_terms_own_match() {
        assert_eq!(filter("src").hits("~/a/src"), [4, 5, 6]);
        assert_eq!(filter("src !zzz a/").hits("~/a/src"), [2, 3, 4, 5, 6], "negations mark nothing");
        assert_eq!(filter("work src").hits("~/a/src"), [4, 5, 6], "a term this piece lacks");
        assert_eq!(filter("日本").hits("~/日本語"), [2, 3], "graphemes, not bytes");
        assert!(filter("").hits("~/a/src").is_empty());
    }

    #[test]
    fn blank_and_negation_only_queries_hide_nothing() {
        let all = sessions_for("");
        assert_eq!(all, ["work", "  edit", "  logs", "play", "  game", "  logs"]);
        assert_eq!(sessions_for("   "), all);
        assert_eq!(sessions_for("!game"), ["work", "  edit", "  logs", "play", "  logs"]);
    }

    #[test]
    fn panes_show_only_to_explain_a_match() {
        // The session name matches its windows: no panes listed.
        assert_eq!(sessions_for("work"), ["work", "  edit", "  logs"]);
        // A pane's agent matches where its window does not.
        assert_eq!(sessions_for("claude"), ["work", "  edit", "    claude"]);
        // A single-pane window is listed without its pane.
        assert_eq!(sessions_for("tail"), ["work", "  logs", "play", "  logs"]);
    }

    #[test]
    fn a_folder_the_display_leaves_out_still_matches() {
        let snap = fixture(&["$1|s|@1|1|w|1|0|%1|0|1|zsh|||||||||/h/source/company/service-name/deep"]);
        let none = HashSet::new();
        let rows = sessions(&snap, &mut filter("company"), &tree(&none, &none, None));
        assert!(matches!(&rows[1].kind, RowKind::Window { path, .. } if path.display(80) == "…/service-name/deep"));
    }

    #[test]
    fn a_window_without_panes_is_listed() {
        // Its only pane line was malformed, so parse left it out.
        let snap = fixture(&["$1|s|@1|1|w|1|0|broken", "$1|s|@2|2|v|0|0|%2|0|1|zsh|||||||||/h"]);
        assert!(snap.sessions[0].windows[0].panes.is_empty());
        let none = HashSet::new();
        let rows = |q: &str| sessions(&snap, &mut filter(q), &tree(&none, &none, None));
        assert_eq!(names(&rows("")), ["s", "  w", "  v"]);
        assert_eq!(rows("")[1].preview, None, "no pane to preview");
        assert!(matches!(&rows("")[1].kind, RowKind::Window { path, panes: 0, .. } if path.display(80).is_empty()),
            "a missing pane must not pick up Paddock's own working directory");
        assert_eq!(names(&rows("w")), ["s", "  w"]);
        assert_eq!(names(&rows("zsh")), ["s", "  v"]);
    }

    #[test]
    fn expanded_windows_list_every_pane() {
        let expanded = HashSet::from([Target::Window { session: "$1".into(), window: "@1".into() }]);
        let rows = sessions(&snap(), &mut filter(""), &tree(&expanded, &HashSet::new(), None));
        assert_eq!(names(&rows)[..4], ["work", "  edit", "    nvim", "    claude"]);
    }

    #[test]
    fn agents_group_by_full_folder_and_count_linked_panes_once() {
        let rows = agents(&snap(), &mut filter(""), Group::Folder, None, "/h", 0, false);
        assert_eq!(names(&rows), ["~/a/src (1)", "  fix parser", "~/b/src (1)", "  deploy"]);
        let rows = agents(&snap(), &mut filter(""), Group::Status, None, "/h", 0, false);
        assert_eq!(names(&rows), ["waiting (1)", "  deploy", "working (1)", "  fix parser"]);
        assert_eq!(snap().counts, [0, 0, 1, 1]);
    }

    #[test]
    fn hidden_cards_follow_every_group_order_without_nested_headers() {
        let lines = [
            "$1|a|@1|1|w|1|0|%1|0|1|claude||idle|0|claude|rest||100||/z",
            "$1|a|@1|1|w|1|0|%2|1|0|codex||working|0|codex|build||200||/b",
            "$2|b|@2|1|w|1|0|%3|0|1|pi||asking|0|pi|question||50||/a",
            "$2|b|@2|1|w|1|0|%4|1|0|claude||idle|1|claude|done||300||/z",
            // The same hidden pane linked into another session is counted once.
            "$3|c|@1|1|w|1|0|%1|0|1|claude||idle|0|claude|rest||100||/z",
        ];
        let hidden_ids = ["%1", "%3", "%4"];
        let mixed = crate::backend::tmux::fixture_hidden(&lines, &hidden_ids);
        let plain = fixture(&lines);
        let ids = |rows: &[Row]| rows.iter().filter_map(|r| r.preview.clone()).collect::<Vec<_>>();
        for group in [Group::Status, Group::Session, Group::Folder, Group::Flat] {
            let baseline = ids(&agents(&plain, &mut filter(""), group, None, "", 400, false));
            let rows = agents(&mixed, &mut filter(""), group, None, "", 400, true);
            let heading = rows.iter().position(|r| matches!(r.kind, RowKind::Hidden { .. })).unwrap();
            assert_eq!(ids(&rows[..heading]), baseline.iter().filter(|id| !hidden_ids.contains(&id.as_str())).cloned().collect::<Vec<_>>());
            assert_eq!(ids(&rows[heading + 1..]), baseline.iter().filter(|id| hidden_ids.contains(&id.as_str())).cloned().collect::<Vec<_>>());
            assert!(matches!(rows[heading].kind, RowKind::Hidden { count: 3, expanded: true, gap: true }));
            assert!(rows[heading + 1..].iter().all(|r| matches!(r.kind, RowKind::Agent { .. })));
            let closed = agents(&mixed, &mut filter(""), group, None, "", 400, false);
            assert_eq!(ids(&closed), ["%2"]);
            assert!(matches!(closed.last().unwrap().kind, RowKind::Hidden { count: 3, expanded: false, .. }));
        }
        let none = HashSet::new();
        assert_eq!(names(&sessions(&mixed, &mut filter(""), &tree(&none, &none, None))), names(&sessions(&plain, &mut filter(""), &tree(&none, &none, None))), "Sessions is unchanged");
    }

    #[test]
    fn all_hidden_and_hidden_search_results_stay_accessible() {
        let snap = crate::backend::tmux::fixture_hidden(&[
            "$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|parser||||/a",
            "$1|s|@1|1|w|1|0|%2|1|0|pi||asking|0|pi|deploy||||/b",
        ], &["%1", "%2"]);
        let rows = agents(&snap, &mut filter(""), Group::Status, None, "", 0, false);
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0].kind, RowKind::Hidden { count: 2, expanded: false, gap: false }));
        for q in ["parser", "claude", "working", "!deploy"] {
            let rows = agents(&snap, &mut filter(q), Group::Status, None, "", 0, false);
            assert_eq!(names(&rows), ["hidden (1)", "  parser"], "{q}");
            assert!(matches!(rows[0].kind, RowKind::Hidden { expanded: true, .. }));
        }
        assert!(agents(&snap, &mut filter("nothing-matches"), Group::Status, None, "", 0, true).is_empty());
        assert!(matches!(agents(&snap, &mut filter("   "), Group::Status, None, "", 0, false)[0].kind, RowKind::Hidden { expanded: false, .. }), "blank query is not a search");
    }

    #[test]
    fn a_waiting_agent_is_found_by_the_tools_word_too() {
        for q in ["waiting", "asking"] {
            assert!(names(&agents(&snap(), &mut filter(q), Group::Flat, None, "/h", 0, false)).contains(&"  deploy".to_string()), "{q}");
        }
    }

    #[test]
    fn agents_in_a_state_list_the_most_recent_first() {
        let snap = fixture(&[
            "$1|s|@1|1|w|1|0|%1|0|1|claude||idle|1|claude|old||100||/a",
            "$1|s|@2|2|w|0|0|%2|0|1|claude||idle|1|claude|unknown||||/a",
            "$1|s|@3|3|w|0|0|%3|0|1|claude||idle|1|claude|new||300||/a",
            "$1|s|@4|4|w|0|0|%4|0|1|claude||asking|0|claude|asks||50||/a",
            "$1|s|@5|5|w|0|0|%5|0|1|claude||idle|1|claude|mid||200||/a",
        ]);
        let rows = agents(&snap, &mut filter(""), Group::Folder, None, "/h", 400, false);
        assert_eq!(names(&rows), ["/a (5)", "  asks", "  new", "  mid", "  old", "  unknown"]);
    }

    #[test]
    fn repositories_or_sessions_sharing_a_name_keep_their_own_groups() {
        let base = std::env::temp_dir().join(format!("paddock-groups-{}", std::process::id()));
        let (a, b) = (base.join("work/api"), base.join("oss/api"));
        for d in [&a, &b] {
            std::fs::create_dir_all(d).unwrap();
            assert!(std::process::Command::new("git").args(["init", "-q"]).arg(d).status().unwrap().success());
        }
        let snap = fixture(&[
            &format!("$1|work|@1|1|w|1|0|%1|0|1|claude||working|0|claude|one||||{}", a.display()),
            &format!("$2|work|@2|1|w|1|0|%2|0|1|claude||asking|0|claude|two||||{}", b.display()),
        ]);
        let folders = names(&agents(&snap, &mut filter(""), Group::Folder, None, "/h", 0, false));
        let sessions = names(&agents(&snap, &mut filter(""), Group::Session, None, "/h", 0, false));
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(folders.iter().filter(|n| n.ends_with("/api (1)")).count(), 2, "told apart by folder: {folders:?}");
        assert_eq!(sessions.iter().filter(|n| *n == "work (1)").count(), 2, "{sessions:?}");
    }

    #[test]
    fn a_window_listing_matching_panes_is_drawn_open() {
        let none = HashSet::new();
        let rows = sessions(&snap(), &mut filter("claude"), &tree(&none, &none, None));
        assert!(matches!(rows[1].kind, RowKind::Window { expanded: Some(true), .. }));
        let rows = sessions(&snap(), &mut filter(""), &tree(&none, &none, None));
        assert!(matches!(rows[1].kind, RowKind::Window { expanded: Some(false), .. }));
        // Closed by the user, it stays listed (it matches) without its panes.
        let collapsed = HashSet::from([Target::Window { session: "$1".into(), window: "@1".into() }]);
        let rows = sessions(&snap(), &mut filter("claude"), &tree(&none, &collapsed, None));
        assert_eq!(names(&rows), ["work", "  edit"]);
        assert!(matches!(rows[1].kind, RowKind::Window { expanded: Some(false), .. }));
    }

    #[test]
    fn unfiltered_every_row_counts_as_a_match() {
        let expanded = HashSet::from([Target::Window { session: "$1".into(), window: "@1".into() }]);
        let rows = sessions(&snap(), &mut filter(""), &tree(&expanded, &HashSet::new(), None));
        assert!(rows.iter().all(|r| r.score.is_some()));
    }

    #[test]
    fn ages() {
        let m = |minutes: u64| age(Some(0), minutes * 60);
        assert_eq!([m(0), m(59), m(60), m(1439), m(1440)], ["<1m", "59m", "1h", "23h", "1d"]);
        assert_eq!(age(None, 100), "", "unknown, not just now");
    }

    #[test]
    fn tree_lines_follow_the_listed_rows() {
        let none = HashSet::new();
        let last = |rows: &[Row]| -> Vec<(String, bool)> {
            rows.iter()
                .filter_map(|r| match &r.kind {
                    RowKind::Window { name, last, .. } => Some((name.clone(), *last)),
                    RowKind::Pane { command, last, .. } => Some((format!("  {command}"), *last)),
                    _ => None,
                })
                .collect()
        };
        // "edit" is work's last listed window, though "logs" follows it in tmux.
        let rows = sessions(&snap(), &mut filter("edit"), &tree(&none, &none, None));
        assert_eq!(last(&rows), [("edit".to_string(), true)]);
        let rows = sessions(&snap(), &mut filter("claude"), &tree(&none, &none, None));
        assert_eq!(last(&rows), [("edit".to_string(), true), ("  claude".to_string(), true)]);
    }

    #[test]
    fn a_kept_row_survives_the_filter() {
        let none = HashSet::new();
        let pane = Target::Pane { session: "$1".into(), window: "@1".into(), pane: "%2".into() };
        let rows = agents(&snap(), &mut filter("zzz"), Group::Flat, Some(&pane), "/h", 0, false);
        assert_eq!(names(&rows), ["  fix parser"]);
        assert!(rows[0].score.is_none(), "kept, not a match");
        let rows = sessions(&snap(), &mut filter("zzz"), &tree(&none, &none, Some(&pane)));
        assert_eq!(names(&rows), ["work", "  edit", "    claude"]);
    }

    #[test]
    fn shortens_paths() {
        assert_eq!(short_path("/home/me/src", "/home/me"), "~/src");
        assert_eq!(short_path("/home/me", "/home/me"), "~");
        assert_eq!(short_path("/home/meg/src", "/home/me"), "/home/meg/src");
        assert_eq!(short_path("/srv/a/very/long/path/to/some/project", ""), "…/some/project");
    }

    #[test]
    fn a_hosts_paths_keep_the_host_and_are_measured_in_columns() {
        assert_eq!(short_path("devbox:/home/me/source/company/project", ""), "devbox:…/company/project");
        assert_eq!(short_path("devbox:/home/me/source/company/project", "/home/me"), "devbox:~/source/company/project", "32 columns");
        assert_eq!(short_path("devbox:/home/me/src", "/home/me"), "devbox:~/src");
        assert_eq!(short_path("devbox:/home/meg/src", "/home/me"), "devbox:/home/meg/src");
        assert_eq!(home_path("devbox:/home/me", "/home/me"), "devbox:~");
        // 22 characters, 36 columns: long enough to shorten.
        assert_eq!(short_path("/srv/日本語/テキスト/もっと長い/名前", ""), "…/もっと長い/名前");
        assert_eq!(short_path("/srv/日本/名前", ""), "/srv/日本/名前");
        assert_eq!(split_host("/a:/b"), ("", "/a:/b"), "a local path with a colon");
        assert_eq!(split_host("~/a:/b"), ("", "~/a:/b"));
        assert_eq!(split_host("devbox:~/a"), ("devbox", "~/a"));
        assert_eq!(split_host("odd:name"), ("", "odd:name"));
        assert_eq!(split_host("box:2222:/home/me"), ("box:2222", "/home/me"), "a label with a colon of its own");
        assert_eq!(split_host("fe80::1:~/x"), ("fe80::1", "~/x"));
    }

    #[test]
    fn fits_whole_graphemes_by_display_width() {
        assert_eq!(fit("ab❤\u{fe0f}cd", 4), "ab…", "VS16 makes it two columns");
        assert_eq!(fit("ab❤\u{fe0f}cd", 5), "ab❤\u{fe0f}…");
        assert_eq!(fit("👨\u{200d}👩\u{200d}👧xyz", 3), "👨\u{200d}👩\u{200d}👧…", "a ZWJ family is one grapheme");
        assert_eq!(fit("👍🏽👍🏽👍🏽", 5), "👍🏽👍🏽…", "skin tones are not columns");
        assert_eq!(fit("e\u{301}tude", 3), "e\u{301}t…");
        for s in ["ab❤\u{fe0f}cd👨\u{200d}👩\u{200d}👧ef", "👍🏽☺\u{fe0f}🏳\u{fe0f}\u{200d}🌈日本", "plain"] {
            for width in 0..20 {
                let out = fit(s, width);
                assert!(out.width() <= width, "{width}: {out}");
                let kept = out.strip_suffix('…').unwrap_or(&out);
                assert!(s.graphemes(true).scan(String::new(), |acc, g| { acc.push_str(g); Some(acc.clone()) }).any(|p| p == kept) || kept.is_empty(), "{width}: cut mid-grapheme: {out:?}");
            }
        }
    }

    #[test]
    fn folder_groups_tell_hosts_apart_and_cards_say_where_they_run() {
        let mut snap = fixture(&[
            "$1|ml|@1|1|train|1|0|%1|0|1|claude||working|0|claude|a||||one:/home/me/source/company/project",
            "$2|ml|@2|1|train|1|0|%2|0|1|claude||working|0|claude|b||||two:/home/me/source/company/project",
            "$3|web|@3|1|api|1|0|%3|0|1|claude||working|0|claude|c||||/h/a/src",
            "$3|web|@3|1|api|1|0|%4|1|0|claude||working|0|claude|d||||/h/a/src",
        ]);
        snap.sessions[0].host = "one".into();
        snap.sessions[1].host = "two".into();
        let rows = agents(&snap, &mut filter(""), Group::Folder, None, "/h", 0, false);
        let headers: Vec<String> = rows.iter().filter_map(|r| match &r.kind { RowKind::Header { label, .. } => Some(label.clone()), _ => None }).collect();
        assert_eq!(headers, ["~/a/src", "one:…/company/project", "two:…/company/project"]);
        let places = |rows: &[Row]| -> Vec<String> { rows.iter().filter_map(|r| match &r.kind { RowKind::Agent { place, .. } => Some(place.clone()), _ => None }).collect() };
        assert_eq!(places(&rows), ["web:api", "web:api", "ml:train", "ml:train"]);
        let hosts: Vec<String> = rows.iter().filter_map(|r| match &r.kind { RowKind::Agent { host, .. } => Some(host.clone()), _ => None }).collect();
        assert_eq!(hosts, ["", "", "one", "two"]);
        // What a card shows finds it: its window, and a host no path names.
        let found = |snap: &Snapshot, query: &str| agents(snap, &mut filter(query), Group::Folder, None, "/h", 0, false).iter().filter(|r| matches!(r.kind, RowKind::Agent { .. })).count();
        assert_eq!(found(&snap, "train"), 2);
        snap.sessions[0].windows[0].panes[0].path.clear();
        snap.sessions[0].host = "zq".into();
        assert_eq!(found(&snap, "zq"), 1);
        // Under its session's header, the window alone; a window named after
        // what runs in it, by its index.
        snap.sessions[2].windows[0].auto = true;
        assert_eq!(places(&agents(&snap, &mut filter(""), Group::Session, None, "/h", 0, false)), ["train", "train", ":1", ":1"]);
        assert_eq!(places(&agents(&snap, &mut filter(""), Group::Status, None, "/h", 0, false)), ["ml:train", "ml:train", "web:1", "web:1"]);
        // Hidden cards have no header: they say their session.
        let hidden = crate::backend::tmux::fixture_hidden(&["$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|a||||/h/a"], &["%1"]);
        assert_eq!(places(&agents(&hidden, &mut filter(""), Group::Session, None, "/h", 0, true)), ["s:w"]);
    }
}

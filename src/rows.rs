//! The rows each tab shows: pure functions of a snapshot, the filter and the
//! tab's settings, and the display text they share.

use std::cmp::Reverse;
use std::collections::HashSet;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::model::{Snapshot, State, Target};

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
    /// None for group headers, which cannot be selected.
    pub target: Option<Target>,
    pub kind: RowKind,
    /// Pane shown in the preview.
    pub preview: Option<String>,
    /// How well the row itself matches the filter (None: shown for context).
    pub score: Option<u32>,
}

/// What a row shows. In the tree, `last` (the last window of its session,
/// the last pane of its window) draws └─ instead of ├─, and a pane's
/// `parent_last` leaves its window's │ out. `meta` is the muted detail
/// after the name (or under it, compact).
pub enum RowKind {
    /// `current`: the session paddock was opened from.
    Session { name: String, windows: usize, current: bool },
    /// `active`: the session's current window. `expanded`: None for a
    /// single pane (nothing to list), else whether its panes are listed.
    Window { index: String, name: String, meta: String, state: Option<State>, active: bool, last: bool, expanded: Option<bool> },
    Pane { index: String, command: String, state: Option<State>, last: bool, parent_last: bool },
    Agent { state: State, stale: bool, title: String, generating: bool, session: String, meta: String, age: String },
    Header { label: String, count: usize },
}

impl Row {
    /// Screen lines. Wide: an agent card is two lines and a blank one.
    /// Compact (a phone): windows and agents take two lines each, with no
    /// blank between cards.
    pub fn height(&self, compact: bool) -> u16 {
        match self.kind {
            RowKind::Agent { .. } if !compact => 3,
            RowKind::Agent { .. } | RowKind::Window { .. } if compact => 2,
            _ => 1,
        }
    }
}

/// The query, parsed once per change, and the matcher it reuses.
pub struct Filter {
    pattern: Pattern,
    matcher: Matcher,
    buf: Vec<char>,
}

impl Filter {
    pub fn new() -> Filter {
        Filter { pattern: Pattern::default(), matcher: Matcher::new(Config::DEFAULT.match_paths()), buf: Vec::new() }
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
        self.pattern.score(Utf32Str::new(text, &mut self.buf), &mut self.matcher)
    }
}

/// PATH relative to HOME as `~`, shortened to its last two folders when long.
pub fn short_path(path: &str, home: &str) -> String {
    let p = match path.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
        _ => path.to_string(),
    };
    if p.chars().count() <= 32 {
        return p;
    }
    let parts: Vec<&str> = p.rsplitn(3, '/').collect();
    match parts.as_slice() {
        [last, parent, _] => format!("…/{parent}/{last}"),
        _ => p,
    }
}

/// Compact time since EPOCH: <1m, 5m, 3h, 2d.
pub fn age(since: Option<u64>, now: u64) -> String {
    let Some(at) = since else { return "<1m".into() };
    let minutes = now.saturating_sub(at) / 60;
    match minutes {
        0 => "<1m".into(),
        m if m < 60 => format!("{m}m"),
        m if m < 1440 => format!("{}h", m / 60),
        m => format!("{}d", m / 1440),
    }
}

/// "session:window.pane · what it runs" for the preview header.
pub fn pane_title(snap: &Snapshot, id: &str) -> String {
    snap.pane(id).map(|(s, w, p)| format!("{}:{}.{} · {}", s.name, w.index, p.index, p.label())).unwrap_or_default()
}

/// Session › window tree. A window shows its panes when EXPANDED or, while
/// filtering, the panes that match when the window itself does not, to
/// show why it is listed: a session's name matches its windows without
/// listing every pane.
pub fn sessions(snap: &Snapshot, filter: &mut Filter, expanded: &HashSet<Target>, current: &str, home: &str) -> Vec<Row> {
    let filtering = filter.active();
    let mut out = Vec::new();
    for s in &snap.sessions {
        let mut block = Vec::new();
        for (wi, w) in s.windows.iter().enumerate() {
            let last = wi + 1 == s.windows.len();
            let active = w.active_pane();
            let path = active.map(|p| short_path(&p.path, home)).unwrap_or_default();
            let text = format!("{} {} {} {}", s.name, w.index, w.name, path);
            let score = filter.score(&text);
            // A pane matches with its window's text, so every term (and
            // negation) applies to the whole line.
            let pane_scores: Vec<Option<u32>> = if filtering && score.is_none() {
                w.panes
                    .iter()
                    .map(|p| filter.score(&format!("{text} {} {} {}", p.label(), p.command, short_path(&p.path, home))))
                    .collect()
            } else {
                vec![None; w.panes.len()]
            };
            if filtering && score.is_none() && pane_scores.iter().all(Option::is_none) {
                continue;
            }
            let target = Target::Window { session: s.id.clone(), window: w.id.clone() };
            let many = w.panes.len() > 1;
            let open = many && expanded.contains(&target);
            let listed = open || pane_scores.iter().any(Option::is_some);
            let mut meta = path;
            if many {
                meta = format!("{meta} · {} panes", w.panes.len());
            }
            block.push(Row {
                target: Some(target),
                kind: RowKind::Window {
                    index: w.index.clone(),
                    name: w.name.clone(),
                    meta,
                    state: w.state(),
                    active: w.active,
                    last,
                    expanded: many.then_some(listed),
                },
                preview: active.map(|p| p.id.clone()),
                // A single pane's match is the window's.
                score: if many { score } else { score.or(pane_scores[0]) },
            });
            if !many {
                continue;
            }
            for (pi, (p, pscore)) in w.panes.iter().zip(pane_scores).enumerate() {
                if !open && pscore.is_none() {
                    continue;
                }
                block.push(Row {
                    target: Some(Target::Pane { session: s.id.clone(), window: w.id.clone(), pane: p.id.clone() }),
                    kind: RowKind::Pane {
                        index: p.index.clone(),
                        command: p.label().to_string(),
                        state: p.agent.as_ref().map(|a| a.state),
                        last: pi + 1 == w.panes.len(),
                        parent_last: last,
                    },
                    preview: Some(p.id.clone()),
                    // Unfiltered, every row matches.
                    score: pscore.or((!filtering).then_some(0)),
                });
            }
        }
        let score = filter.score(&s.name);
        if filtering && block.is_empty() && score.is_none() {
            continue;
        }
        out.push(Row {
            target: Some(Target::Session { session: s.id.clone() }),
            kind: RowKind::Session { name: s.name.clone(), windows: s.windows.len(), current: s.id == current },
            preview: s.active_window().and_then(|w| w.active_pane()).map(|p| p.id.clone()),
            score,
        });
        out.extend(block);
    }
    out
}

/// Groups in this order; one kind per build.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum GroupKey {
    /// Most urgent first.
    Status(Reverse<State>),
    Name(String),
    All,
}

/// One card per agent pane, grouped under headers (none for Group::Flat);
/// inside a group the most urgent state comes first.
pub fn agents(snap: &Snapshot, filter: &mut Filter, group: Group, home: &str, now: u64) -> Vec<Row> {
    let mut items: Vec<(GroupKey, String, State, Row)> = Vec::new();
    for (s, w, p) in snap.unique_panes() {
        let Some(a) = &p.agent else { continue };
        let folder = p.path.rsplit('/').next().filter(|f| !f.is_empty()).unwrap_or(&p.path);
        let kind = p.label();
        let Some(score) = filter.score(&format!("{} {} {} {} {}{}", a.title, s.name, folder, kind, a.state.label(), if a.stale { " stale" } else { "" })) else {
            continue;
        };
        let (key, label) = match group {
            Group::Status => (GroupKey::Status(Reverse(a.state)), a.state.label().to_string()),
            Group::Session => (GroupKey::Name(s.name.clone()), s.name.clone()),
            Group::Folder => (GroupKey::Name(p.path.clone()), short_path(&p.path, home)),
            Group::Flat => (GroupKey::All, String::new()),
        };
        let row = Row {
            target: Some(Target::Pane { session: s.id.clone(), window: w.id.clone(), pane: p.id.clone() }),
            kind: RowKind::Agent {
                state: a.state,
                stale: a.stale,
                title: a.title.clone(),
                generating: a.generating,
                session: s.name.clone(),
                meta: format!("{} › {folder} · {kind} · {}:{}", s.name, w.index, p.index),
                age: age(a.since, now),
            },
            preview: Some(p.id.clone()),
            score: Some(score),
        };
        items.push((key, label, a.state, row));
    }
    // Stable: equal states keep tmux's order.
    items.sort_by(|a, b| a.0.cmp(&b.0).then(b.2.cmp(&a.2)));
    let mut out = Vec::new();
    let mut items = items.into_iter().peekable();
    while let Some((key, label, _, row)) = items.next() {
        let mut group_rows = vec![row];
        while let Some(it) = items.next_if(|it| it.0 == key) {
            group_rows.push(it.3);
        }
        if group != Group::Flat {
            out.push(Row { target: None, kind: RowKind::Header { label, count: group_rows.len() }, preview: None, score: None });
        }
        out.extend(group_rows);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixture;

    fn snap() -> Snapshot {
        fixture(&[
            "$1|work|@1|1|edit|1|%1|0|1|nvim||||||||/h/a/src",
            "$1|work|@1|1|edit|1|%2|1|0|zsh|working|0|claude|fix parser||||/h/a/src",
            "$1|work|@2|2|logs|0|%3|0|1|tail|asking|0|codex|deploy||||/h/b/src",
            "$2|play|@3|1|game|1|%4|0|1|zsh||||||||/h/c",
            // @2 linked into play too.
            "$2|play|@2|2|logs|0|%3|0|1|tail|asking|0|codex|deploy||||/h/b/src",
        ])
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
                RowKind::Header { label, count } => format!("{label} ({count})"),
            })
            .collect()
    }

    fn sessions_for(q: &str) -> Vec<String> {
        names(&sessions(&snap(), &mut filter(q), &HashSet::new(), "$1", "/h"))
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
    fn expanded_windows_list_every_pane() {
        let expanded = HashSet::from([Target::Window { session: "$1".into(), window: "@1".into() }]);
        let rows = sessions(&snap(), &mut filter(""), &expanded, "$1", "/h");
        assert_eq!(names(&rows)[..4], ["work", "  edit", "    nvim", "    claude"]);
    }

    #[test]
    fn agents_group_by_full_folder_and_count_linked_panes_once() {
        let rows = agents(&snap(), &mut filter(""), Group::Folder, "/h", 0);
        assert_eq!(names(&rows), ["~/a/src (1)", "  fix parser", "~/b/src (1)", "  deploy"]);
        let rows = agents(&snap(), &mut filter(""), Group::Status, "/h", 0);
        assert_eq!(names(&rows), ["waiting (1)", "  deploy", "working (1)", "  fix parser"]);
        assert_eq!(snap().counts(), [0, 0, 1, 1]);
    }

    #[test]
    fn a_window_listing_matching_panes_is_drawn_open() {
        let rows = sessions(&snap(), &mut filter("claude"), &HashSet::new(), "$1", "/h");
        assert!(matches!(rows[1].kind, RowKind::Window { expanded: Some(true), .. }));
        let rows = sessions(&snap(), &mut filter(""), &HashSet::new(), "$1", "/h");
        assert!(matches!(rows[1].kind, RowKind::Window { expanded: Some(false), .. }));
    }

    #[test]
    fn unfiltered_every_row_counts_as_a_match() {
        let expanded = HashSet::from([Target::Window { session: "$1".into(), window: "@1".into() }]);
        let rows = sessions(&snap(), &mut filter(""), &expanded, "$1", "/h");
        assert!(rows.iter().all(|r| r.score.is_some()));
    }

    #[test]
    fn ages() {
        let m = |minutes: u64| age(Some(0), minutes * 60);
        assert_eq!([m(0), m(59), m(60), m(1439), m(1440)], ["<1m", "59m", "1h", "23h", "1d"]);
        assert_eq!(age(None, 100), "<1m");
    }

    #[test]
    fn shortens_paths() {
        assert_eq!(short_path("/home/me/src", "/home/me"), "~/src");
        assert_eq!(short_path("/home/me", "/home/me"), "~");
        assert_eq!(short_path("/home/meg/src", "/home/me"), "/home/meg/src");
        assert_eq!(short_path("/srv/a/very/long/path/to/some/project", ""), "…/some/project");
    }
}

//! Popup state: the snapshot, the current tab's rows, the selection, the
//! filter and the mode (the list, renaming, naming something new, a menu, a
//! question, a move), and what keys and commands do to it. Where things
//! landed on screen is the View (view.rs), written by each draw; moves are
//! planned in moves.rs; what can be done, and how it is offered, is in
//! commands.rs.

mod commands;
mod moves;
mod view;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Position;

use crate::backend::Caps;
use crate::diffstat::Diff;
use crate::edit::{Edited, LineEdit};
use crate::keys::{self, Input};
pub use crate::model::{Op, Tab};
use crate::model::{Mark, Session, Snapshot, Target, Window};
use crate::rows::{self, Cards, Filter, Group, New, Row, RowKind, Tree};

pub use commands::{Command, Hint, Menu};
use moves::Carry;
use view::Press;
pub use view::View;

/// What the popup asks the loop to do.
#[derive(Debug)]
pub enum Action {
    None,
    Quit,
    Activate(Target),
    /// Run the job's change on the server, then report through `ran`.
    Run(Job),
    /// Ask the suggest command for PANE's title; the answer comes back
    /// through `suggested` with the same ID.
    Suggest { pane: String, id: u64 },
    /// Type into PANE (typing mode).
    Send(String, Input),
}

/// An Op, what undoes it, what to say once it is done, and what to select
/// (by id) or open afterwards.
#[derive(Debug)]
pub struct Job {
    pub op: Op,
    undo: Option<Op>,
    done: String,
    /// Select this window or pane wherever it lands.
    follow: Option<String>,
    /// Jump to what the op created.
    pub open: bool,
    /// What a failure puts back.
    retry: Option<Box<Retry>>,
}

/// What a job came from, put back when the server refuses it: the editor,
/// with what was typed, or the undo entry it ran.
#[derive(Debug)]
enum Retry {
    Rename(Rename),
    Create(Create),
    Undo(Op, String),
}

impl Job {
    fn new(op: Op, done: impl Into<String>) -> Job {
        Job { op, undo: None, done: done.into(), follow: None, open: false, retry: None }
    }

    fn undo(mut self, op: Option<Op>) -> Job {
        self.undo = op;
        self
    }

    fn follow(mut self, id: &str) -> Job {
        self.follow = Some(id.to_string());
        self
    }

    fn retry(mut self, retry: Retry) -> Job {
        self.retry = Some(Box::new(retry));
        self
    }

    /// The server never said how the op went, so it may have been made:
    /// nothing that would make it a second time is put back. (A rename can
    /// be: twice is the same as once.)
    pub fn unsure(mut self) -> Job {
        self.retry = self.retry.filter(|r| matches!(**r, Retry::Rename(_)));
        self
    }
}

/// A message on the frame's bottom edge until the next key or click.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Done(String),
    Failed(String),
    Note(String),
}

/// A title or name being edited in place.
#[derive(Debug)]
pub struct Rename {
    /// A session or window's name, or a pane's agent's title
    /// (@agent-description).
    pub target: Target,
    pub field: LineEdit,
    /// The title or name when renaming began, and whether the user set it
    /// (else it is automatic: the agent's title, or tmux's window name).
    original: String,
    /// What the field began with (ORIGINAL cut to TITLE_MAX, trimmed):
    /// saving that changes nothing.
    unchanged: String,
    renamed: bool,
    /// What an empty field means: the automatic title or name.
    pub placeholder: String,
    /// The suggestion on its way, if any (agents only).
    pub suggesting: Option<u64>,
    /// Why the last suggestion or save failed.
    pub error: Option<String>,
    /// It was a suggestion that failed: asking again may do better.
    pub(super) retry: bool,
}

/// A change waiting for a yes: what it is, and the question.
#[derive(Debug)]
pub struct Confirm {
    pub job: Job,
    pub question: String,
    /// It is a carried row's drop that asks.
    pub(super) dropped: bool,
}

impl Confirm {
    /// A kill, which can't be undone; else a move that closes something.
    pub fn kills(&self) -> bool {
        matches!(self.job.op, Op::Kill(_))
    }
}

/// A new window or session being named, on its own row where it will go.
#[derive(Debug)]
pub struct Create {
    /// The row selected when naming began, selected again after.
    from: Option<Target>,
    /// The tab naming began on, gone back to on cancel.
    back: Tab,
    pub new: New,
    /// The other one (^s switches): a session for a window, and back.
    other: New,
    pub field: LineEdit,
    /// Why the name was refused.
    pub error: Option<String>,
}

/// Typing into the previewed pane: keys and pastes go to PANE, LABEL
/// (session:window.pane · what runs) says where.
#[derive(Debug)]
pub struct Typing {
    pub pane: String,
    pub label: String,
}

#[derive(Debug)]
pub enum Mode {
    List,
    Type(Typing),
    Rename(Rename),
    Menu(Menu),
    Confirm(Confirm),
    Move(Carry),
    Create(Create),
}

/// Titles are cut to this many characters (graphemes), as suggestions are.
const TITLE_MAX: usize = 80;

/// PageUp and PageDown move this many rows.
const PAGE: isize = 10;

/// The tmux id a target names.
fn id_of(t: &Target) -> &str {
    match t {
        Target::Session { session } => session,
        Target::Window { window, .. } => window,
        Target::Pane { pane, .. } => pane,
    }
}

/// Why NAME will not do for a session (the one with id RENAMED keeps its
/// own name), if it will not: by TMUX's rules too, if they apply.
fn session_name_problem(snap: &Snapshot, tmux: bool, name: &str, renamed: Option<&str>) -> Option<String> {
    if name.is_empty() {
        Some("a session needs a name".into())
    } else if !tmux {
        None
    } else if name.contains(['.', ':']) {
        Some("session names can't have . or :".into())
    } else if snap.sessions.iter().any(|s| s.name == name && Some(s.id.as_str()) != renamed) {
        Some(format!("a session called {name} exists"))
    } else {
        None
    }
}

pub struct App {
    pub(crate) snap: Snapshot,
    pub(crate) tab: Tab,
    pub(crate) group: Group,
    /// Saved preference; searching only overrides the rendered expansion.
    pub(crate) hidden_expanded: bool,
    pub(crate) query: LineEdit,
    pub(crate) rows: Vec<Row>,
    pub(crate) selected: usize,
    /// The preview as toggled; None: shown unless the layout is compact (a
    /// phone, where the list needs the room).
    pub(crate) preview: Option<bool>,
    /// Whether the last draw was in the compact layout (the loop keeps it).
    pub(crate) compact: bool,
    /// Whether the last draw had room for the preview (the loop keeps it):
    /// the menu offers to toggle it only then.
    pub(crate) preview_fits: bool,
    pub(crate) mode: Mode,
    pub(crate) status: Option<Status>,
    /// What a server is failing with, for as long as it does: shown when
    /// nothing newer is, as the list may be stale meanwhile.
    pub(crate) trouble: Option<String>,
    /// Whether a suggest command is configured (@agent-suggest-command).
    pub(crate) can_suggest: bool,
    /// What the server can do (tmux's, unless the loop says otherwise):
    /// nothing else is offered.
    pub(crate) caps: Caps,
    current_session: String,
    current_window: String,
    expanded: HashSet<Target>,
    collapsed: HashSet<Target>,
    /// Listed whatever the filter says until the query changes (just renamed).
    keep: Option<Target>,
    pub(crate) filter: Filter,
    paths: crate::paths::PathCache,
    home: String,
    /// The last suggestion asked for.
    suggestions: u64,
    /// What ^z undoes, latest last, and what it did.
    undo: Vec<(Op, String)>,
    press: Option<Press>,
    /// The last click on a row, for double-clicks.
    last_click: Option<(Target, Instant)>,
    /// A window or pane to select once the next snapshot shows where it is.
    follow: Option<String>,
    /// Changes waiting in agents' folders (local ones), by folder: none
    /// listed when clean.
    diffs: HashMap<String, Diff>,
    /// What each transcript last said, by file, for the state it was read
    /// for (tmux; Rex agents have their note).
    notes: rows::Notes,
}

impl App {
    pub fn new(snap: Snapshot, tab: Tab, current_session: String, current_window: String, home: String, can_suggest: bool, hidden_expanded: bool) -> App {
        let mut app = App {
            snap,
            tab,
            group: Group::Status,
            hidden_expanded,
            query: LineEdit::default(),
            rows: Vec::new(),
            selected: 0,
            preview: None,
            compact: false,
            preview_fits: true,
            mode: Mode::List,
            status: None,
            trouble: None,
            can_suggest,
            caps: Caps::default(),
            current_session,
            current_window,
            expanded: HashSet::new(),
            collapsed: HashSet::new(),
            keep: None,
            filter: Filter::new(),
            paths: crate::paths::PathCache::new(home.clone()),
            home,
            suggestions: 0,
            undo: Vec::new(),
            press: None,
            last_click: None,
            follow: None,
            diffs: HashMap::new(),
            notes: HashMap::new(),
        };
        app.rebuild(true);
        app
    }

    pub fn home(&self) -> &str {
        &self.home
    }

    pub fn selected_target(&self) -> Option<&Target> {
        self.rows.get(self.selected)?.target.as_ref()
    }

    /// Whether the preview is wanted, in the layout COMPACT or not.
    pub fn preview_on(&self, compact: bool) -> bool {
        self.preview.unwrap_or(!compact)
    }

    /// The pane to preview, if the preview is on screen: the one typed
    /// into, else the selected row's.
    pub fn preview_pane(&self, view: &View) -> Option<&str> {
        if !(self.preview_on(view.compact) && view.preview_fits) {
            return None;
        }
        if let Mode::Type(t) = &self.mode {
            return Some(&t.pane);
        }
        self.rows.get(self.selected)?.preview.as_deref()
    }

    /// Whether the last draw had room for the preview: without it, typing
    /// would be blind, so it ends.
    pub fn set_preview_fits(&mut self, fits: bool) {
        self.preview_fits = fits;
        if !fits && matches!(self.mode, Mode::Type(_)) {
            self.mode = Mode::List;
            self.note("no room for the preview: typing ended");
        }
    }

    /// Start typing into the selected row's pane, with the preview on.
    fn start_typing(&mut self) {
        let Some(pane) = self.rows.get(self.selected).and_then(|r| r.preview.clone()) else { return };
        if !self.caps.send {
            return self.note("can't type into panes here");
        }
        if !self.preview_fits {
            return self.note("no room for the preview here");
        }
        self.preview = Some(true);
        // Listed whatever the filter says, so the preview stays: an answered
        // agent no longer matches "asking".
        self.keep = self.selected_target().cloned();
        let label = rows::pane_title(&self.snap, &pane);
        self.mode = Mode::Type(Typing { pane, label });
    }

    /// TARGET as reached from where the client is: a pane in its own session
    /// when that shows it too (grouped sessions, linked windows), not in
    /// whichever lists it first.
    pub fn local(&self, target: Target) -> Target {
        match &target {
            Target::Pane { pane, .. } => self.snap.place_in(pane, &self.current_session).unwrap_or(target),
            _ => target,
        }
    }

    /// The suggestion the loop should still be waiting for.
    pub fn suggesting(&self) -> Option<u64> {
        match &self.mode {
            Mode::Rename(r) => r.suggesting,
            _ => None,
        }
    }

    pub fn set_snapshot(&mut self, snap: Snapshot) {
        // The first sessions to come in (a slow server's, after the popup
        // opened on none) select where the user is, as opening does.
        let first = self.snap.sessions.is_empty() && !snap.sessions.is_empty();
        self.snap = snap;
        // What is being renamed went away: there is nothing left to name.
        if let Mode::Rename(r) = &self.mode {
            let gone = match &r.target {
                Target::Pane { pane, .. } => self.snap.agent(pane).is_none().then(|| "that agent has exited".to_string()),
                Target::Session { session } => self.find(session).is_none().then(|| "that session has closed".to_string()),
                Target::Window { window, .. } => self.find(window).is_none().then(|| format!("that {} has closed", self.caps.window())),
            };
            if let Some(why) = gone {
                self.mode = Mode::List;
                self.status = Some(Status::Failed(why));
            }
        }
        if let Mode::Type(t) = &self.mode
            && self.snap.pane(&t.pane).is_none()
        {
            self.mode = Mode::List;
            self.status = Some(Status::Failed("that pane has closed".into()));
        }
        if let Mode::Move(c) = &self.mode
            && self.find(id_of(&c.target)).is_none()
        {
            let why = format!("the {} has closed", c.label);
            self.mode = Mode::List;
            self.status = Some(Status::Failed(why));
        }
        // A moved window or pane: select it where it landed.
        if let Some(id) = self.follow.take()
            && let Some(target) = self.find(&id)
        {
            self.keep = Some(target.clone());
            let window = match &target {
                Target::Pane { session, window, .. } => Some(Target::Window { session: session.clone(), window: window.clone() }),
                _ => None,
            };
            // A pane's window opens: kept alone, the pane would be listed
            // under a window drawn open, apart from its other panes.
            if let Some(w) = &window {
                self.collapsed.remove(w);
                self.expanded.insert(w.clone());
            }
            self.rebuild(false);
            // A pane alone in its window is shown as the window.
            if let Some(i) = self.position(&target).or_else(|| window.and_then(|w| self.position(&w))) {
                self.selected = i;
            }
        } else {
            self.rebuild(first);
        }
        // The menu's row went away: its items would act on the next one.
        if let Mode::Menu(m) = &self.mode
            && (m.target.as_ref() != self.selected_target() || m.hidden != self.hidden_selected())
        {
            self.mode = Mode::List;
            self.status = Some(Status::Failed("that row has gone".into()));
        }
    }

    /// The session, window or pane with tmux id ID, as a target (a window
    /// linked into several sessions: its first).
    fn find(&self, id: &str) -> Option<Target> {
        for s in &self.snap.sessions {
            if s.id == id {
                return Some(Target::Session { session: s.id.clone() });
            }
            for w in &s.windows {
                if w.id == id {
                    return Some(Target::Window { session: s.id.clone(), window: w.id.clone() });
                }
                if let Some(p) = w.panes.iter().find(|p| p.id == id) {
                    return Some(Target::Pane { session: s.id.clone(), window: w.id.clone(), pane: p.id.clone() });
                }
            }
        }
        None
    }

    /// The client was moved to SESSION (out of one an op closes): it is the
    /// current session now, at its active window.
    pub fn moved(&mut self, session: &str) {
        if let Some(w) = self.snap.session(session).and_then(|s| s.active_window()) {
            self.current_window = w.id.clone();
        }
        self.current_session = session.to_string();
    }

    /// The changes waiting in folder DIR (None: unknown, or not a
    /// repository); whether that changed what is shown.
    pub fn set_diff(&mut self, dir: String, diff: Option<Diff>) -> bool {
        let diff = diff.filter(|d| !d.is_clean());
        let old = match diff {
            Some(d) => self.diffs.insert(dir, d),
            None => self.diffs.remove(&dir),
        };
        let changed = old != diff;
        if changed && self.tab == Tab::Agents {
            self.rebuild(false);
        }
        changed
    }

    /// What transcript FILE says for an agent with MARK (`Agent::mark`);
    /// whether that changed what is shown.
    pub fn set_note(&mut self, file: String, mark: Mark, said: String) -> bool {
        let old = self.notes.insert(file, (mark.clone(), said.clone()));
        let changed = old != Some((mark, said));
        if changed && self.tab == Tab::Agents {
            self.rebuild(false);
        }
        changed
    }

    /// Rebuild from the same snapshot (ages tick over).
    pub fn refresh(&mut self) {
        self.rebuild(false);
    }

    /// Rebuild the rows. After a query or tab change (FRESH) the best match
    /// is selected, unless the selection ties with it; otherwise the
    /// selection stays on its target, or on the nearest row when that is
    /// gone.
    fn rebuild(&mut self, fresh: bool) {
        let keep = self.selected_target().cloned();
        let was_hidden = self.hidden_heading().is_some_and(|i| i <= self.selected);
        let old = self.selected;
        self.rows = match self.tab {
            Tab::Sessions => {
                let tree = Tree {
                    expanded: &self.expanded,
                    collapsed: &self.collapsed,
                    keep: self.keep.as_ref(),
                    current: &self.current_session,
                };
                let mut rows = rows::sessions(&self.snap, &mut self.filter, &tree, &mut self.paths);
                if let Mode::Create(c) = &self.mode {
                    rows::place_new(&mut rows, &c.new);
                }
                rows
            }
            Tab::Agents => {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                let cards = Cards {
                    group: self.group,
                    keep: self.keep.as_ref(),
                    now,
                    hidden_expanded: self.hidden_expanded,
                    notes: &self.notes,
                    diffs: &self.diffs,
                };
                rows::agents(&self.snap, &mut self.filter, &cards, &mut self.paths)
            }
        };
        // When a search ends, a hidden card may fold away: keep the user on
        // its heading rather than jumping to an unrelated visible agent.
        let kept = keep.as_ref().and_then(|t| self.locate(t, old)).or_else(|| was_hidden.then(|| self.hidden_heading()).flatten());
        let pick = if fresh && self.filter.active() {
            // Best match, earliest on ties, but the selection stays when it
            // ties (a negation-only query scores every row the same).
            let best = self.rows.iter().filter(|r| r.target.is_some()).filter_map(|r| r.score).max();
            let top = |i: &usize| best.is_some() && self.rows[*i].target.is_some() && self.rows[*i].score == best;
            kept.filter(top).or_else(|| (0..self.rows.len()).find(top))
        } else {
            kept
        };
        let pick = pick.or_else(|| {
            if !fresh {
                self.nearest(old)
            } else if self.tab == Tab::Sessions {
                // The window of the pane selected before, if it is not
                // listed (a tab change from agents), else the current one.
                let pane_window = match &keep {
                    Some(Target::Pane { session, window, .. }) => self.position(&Target::Window { session: session.clone(), window: window.clone() }),
                    _ => None,
                };
                pane_window.or_else(|| self.position(&Target::Window { session: self.current_session.clone(), window: self.current_window.clone() }))
            } else {
                // The agent in the window selected before (a tab change from
                // sessions), else in the current one.
                let window = match &keep {
                    Some(Target::Window { window, .. }) => window.clone(),
                    _ => self.current_window.clone(),
                };
                self.agent_in(&window)
            }
        });
        self.selected = pick.or_else(|| self.nearest(0)).unwrap_or(0);
        // Naming something new: its row is the selection.
        if matches!(self.mode, Mode::Create(_))
            && let Some(i) = self.rows.iter().position(|r| matches!(r.kind, RowKind::New))
        {
            self.selected = i;
        }
    }

    /// The card of WINDOW's active pane, else of its first agent.
    fn agent_in(&self, window: &str) -> Option<usize> {
        let active = self.snap.unique_panes().find(|(_, w, p)| w.id == window && p.active).map(|(_, _, p)| p.id.as_str());
        let card = |i: &usize| matches!(&self.rows[*i].target, Some(Target::Pane { window: w, .. }) if w == window);
        let pane = |i: &usize| matches!(&self.rows[*i].target, Some(Target::Pane { pane, .. }) if Some(pane.as_str()) == active);
        (0..self.rows.len()).filter(card).find(pane).or_else(|| (0..self.rows.len()).find(card))
    }

    fn position(&self, target: &Target) -> Option<usize> {
        self.rows.iter().position(|r| r.target.as_ref() == Some(target))
    }

    /// TARGET's row: still at index AT if it is there (a window linked twice
    /// into a session is listed twice), else its first.
    fn locate(&self, target: &Target, at: usize) -> Option<usize> {
        let there = self.rows.get(at).is_some_and(|r| r.target.as_ref() == Some(target));
        if there { Some(at) } else { self.position(target) }
    }

    fn selectable(&self, i: usize) -> bool {
        self.rows.get(i).is_some_and(Row::selectable)
    }

    /// First selectable row from I (in bounds) walking FORWARD or back.
    fn find_from(&self, i: usize, forward: bool) -> Option<usize> {
        if forward { (i..self.rows.len()).find(|&j| self.selectable(j)) } else { (0..=i).rev().find(|&j| self.selectable(j)) }
    }

    /// The selectable row at or after I (clamped), else the one before.
    fn nearest(&self, i: usize) -> Option<usize> {
        let i = i.min(self.rows.len().checked_sub(1)?);
        self.find_from(i, true).or_else(|| self.find_from(i, false))
    }

    /// Move the selection by DELTA rows, skipping headers. Past an end, WRAP
    /// comes round from the other end; otherwise the move stops at it.
    fn move_by(&mut self, delta: isize, wrap: bool) {
        let n = self.rows.len();
        if n == 0 || delta == 0 {
            return;
        }
        let forward = delta > 0;
        let to = self.selected.checked_add_signed(delta).filter(|&i| i < n);
        let next = if wrap {
            to.and_then(|i| self.find_from(i, forward)).or_else(|| self.find_from(if forward { 0 } else { n - 1 }, forward))
        } else {
            let to = to.unwrap_or(if forward { n - 1 } else { 0 });
            self.find_from(to, forward).or_else(|| self.find_from(to, !forward))
        };
        if let Some(i) = next {
            self.selected = i;
        }
    }

    /// The window the selected row is, or is a pane of, if it has panes to
    /// list, and whether they are listed.
    fn selected_window(&self) -> Option<(Target, bool)> {
        match self.rows.get(self.selected)? {
            Row { target: Some(t @ Target::Window { .. }), kind: RowKind::Window { expanded: Some(open), .. }, .. } => Some((t.clone(), *open)),
            Row { target: Some(Target::Pane { session, window, .. }), kind: RowKind::Pane { .. }, .. } => {
                Some((Target::Window { session: session.clone(), window: window.clone() }, true))
            }
            _ => None,
        }
    }

    fn hidden_heading(&self) -> Option<usize> {
        self.rows.iter().position(|r| matches!(r.kind, RowKind::Hidden { .. }))
    }

    fn hidden_selected(&self) -> bool {
        self.rows.get(self.selected).is_some_and(|r| matches!(r.kind, RowKind::Hidden { .. }))
    }

    fn selected_agent_hidden(&self) -> bool {
        self.selected_agent().and_then(|(_, pane)| self.snap.agent(&pane)).is_some_and(|a| a.hidden)
    }

    /// Expand/collapse the Hidden section or the selected window's panes.
    fn expand(&mut self, how: Option<bool>) -> Action {
        if self.tab == Tab::Agents && (self.hidden_selected() || self.selected_agent_hidden()) {
            if self.filter.active() {
                self.note("hidden matches stay expanded while filtering");
                return Action::None;
            }
            let expanded = how.unwrap_or(!self.hidden_expanded);
            if expanded == self.hidden_expanded {
                return Action::None;
            }
            return Action::Run(Job::new(Op::SetHiddenExpanded(expanded), if expanded { "hidden expanded" } else { "hidden collapsed" }));
        }
        let Some((window, open)) = self.selected_window() else { return Action::None };
        if how.unwrap_or(!open) {
            self.collapsed.remove(&window);
            self.expanded.insert(window);
        } else {
            if let Some(i) = self.position(&window) {
                self.selected = i;
            }
            self.expanded.remove(&window);
            // A window the filter opens stays closed until the query changes.
            self.collapsed.insert(window);
        }
        self.rebuild(false);
        Action::None
    }

    fn edit_query(&mut self, key: KeyEvent) {
        if self.query.key(key) == Edited::Text {
            self.requery();
        }
    }

    fn requery(&mut self) {
        self.filter.set(self.query.text());
        // What is carried stays listed, whatever the query.
        self.keep = match &self.mode {
            Mode::Move(c) => Some(c.target.clone()),
            _ => None,
        };
        self.collapsed.clear();
        self.rebuild(true);
    }

    /// Pasted text joins the query or the title being edited (bracketed
    /// paste: a newline in it must not act as Enter); typing, it goes to
    /// the pane.
    pub fn paste(&mut self, text: &str) -> Action {
        match &mut self.mode {
            Mode::Type(t) => return Action::Send(t.pane.clone(), Input::Paste(text.to_string())),
            Mode::Rename(_) | Mode::Create(_) => self.edit_name(|f| f.insert(text)),
            Mode::Menu(_) | Mode::Confirm(_) => {}
            Mode::List | Mode::Move(_) => {
                if self.query.insert(text) {
                    self.requery();
                }
            }
        }
        Action::None
    }

    /// Edit the title or name being typed with EDIT, which says whether the
    /// text changed: then a suggestion on its way or a refusal is stale.
    fn edit_name(&mut self, edit: impl FnOnce(&mut LineEdit) -> bool) {
        match &mut self.mode {
            Mode::Rename(r) => {
                if edit(&mut r.field) {
                    r.suggesting = None;
                    r.error = None;
                }
            }
            Mode::Create(c) => {
                if edit(&mut c.field) {
                    c.error = None;
                }
            }
            _ => {}
        }
    }

    fn set_tab(&mut self, tab: Tab) {
        if tab != self.tab {
            self.tab = tab;
            self.keep = None;
            self.rebuild(true);
        }
    }

    /// The agent after the selected one that needs you, as `paddock next`
    /// goes: waiting longest first, then finished.
    fn next_need(&self) -> Option<String> {
        let here = self.selected_agent().map(|(_, pane)| pane).unwrap_or_default();
        match self.snap.needs_you(&here)? {
            Target::Pane { pane, .. } => Some(pane),
            _ => None,
        }
    }

    /// Select the next agent that needs you, on the agents tab: its card
    /// and preview, to answer it (^l) or go there (⏎). A query that hides
    /// it is cleared.
    fn go_next(&mut self) {
        let Some(pane) = self.next_need() else {
            self.note("no other agent needs you");
            return;
        };
        self.set_tab(Tab::Agents);
        let card = |app: &App| app.rows.iter().position(|r| matches!(r.kind, RowKind::Agent(_)) && matches!(&r.target, Some(Target::Pane { pane: p, .. }) if *p == pane));
        if card(self).is_none() && !self.query.is_empty() {
            self.query.set("");
            self.requery();
        }
        if let Some(i) = card(self) {
            self.selected = i;
        }
    }

    /// The agent pane the selected row is (a card, or a pane row), if any.
    fn selected_agent(&self) -> Option<(Target, String)> {
        let target = self.selected_target()?;
        let Target::Pane { pane, .. } = target else { return None };
        self.snap.agent(pane)?;
        Some((target.clone(), pane.clone()))
    }

    /// Start renaming the selected row: an agent's title (a card, or an
    /// agent's pane row), a session's or a window's name.
    fn rename(&mut self) {
        let Some(target) = self.selected_target().cloned() else { return };
        let (text, renamed, placeholder) = match &target {
            Target::Pane { pane, .. } => {
                let Some(agent) = self.snap.agent(pane) else { return };
                (agent.title.clone(), agent.renamed, agent.auto.clone())
            }
            Target::Session { session } => {
                let Some(s) = self.snap.session(session) else { return };
                (s.name.clone(), true, String::new())
            }
            Target::Window { session, window } => {
                let Some((_, _, _, w)) = self.window_at(session, window) else { return };
                (w.name.clone(), !w.auto, "named after what runs".to_string())
            }
        };
        // Listed while it is renamed, whatever its row's text turns into.
        self.keep = Some(target.clone());
        let field = LineEdit::new(&text, Some(TITLE_MAX));
        self.mode = Mode::Rename(Rename {
            target,
            unchanged: field.text().trim().to_string(),
            field,
            original: text,
            renamed,
            placeholder,
            suggesting: None,
            error: None,
            retry: false,
        });
    }

    /// Ask for a title to fill the field with, unless one is on its way.
    fn suggest(&mut self) -> Action {
        let Mode::Rename(r) = &mut self.mode else { return Action::None };
        let Target::Pane { pane, .. } = &r.target else { return Action::None };
        if r.suggesting.is_some() {
            return Action::None;
        }
        if !self.can_suggest {
            let why = if self.caps.rex { "suggest is not set in paddock.setup" } else { "nothing sets @agent-suggest-command" };
            self.status = Some(Status::Note(format!("no title suggestions: {why}")));
            return Action::None;
        }
        self.suggestions += 1;
        r.suggesting = Some(self.suggestions);
        r.error = None;
        Action::Suggest { pane: pane.clone(), id: self.suggestions }
    }

    /// The answer to suggestion ID: it fills the field, unless renaming
    /// ended or the user typed since asking (typing drops the suggestion).
    pub fn suggested(&mut self, id: u64, result: Result<String, String>) {
        let Mode::Rename(r) = &mut self.mode else { return };
        if r.suggesting != Some(id) {
            return;
        }
        r.suggesting = None;
        match result {
            Ok(title) => r.field.set(&title),
            Err(e) => (r.error, r.retry) = (Some(e), true),
        }
    }

    /// Finish renaming: Save sets the name when it changed (an empty title
    /// or window name: back to automatic), unless the server would refuse it, which
    /// keeps the field open with the reason; anything else cancels.
    fn finish_rename(&mut self, save: bool) -> Action {
        let Mode::Rename(r) = &mut self.mode else { return Action::None };
        let text = r.field.text().trim().to_string();
        if !save || text == r.unchanged || (text.is_empty() && !r.renamed) {
            self.mode = Mode::List;
            return Action::None;
        }
        if let Target::Session { session } = &r.target
            && let Some(why) = session_name_problem(&self.snap, self.caps.tmux_session_names, &text, Some(session))
        {
            (r.error, r.retry) = (Some(why), false);
            return Action::None;
        }
        let Mode::Rename(r) = std::mem::replace(&mut self.mode, Mode::List) else { return Action::None };
        // The row stays listed even if its new name doesn't match the filter.
        self.keep = Some(r.target.clone());
        let auto = text.is_empty();
        let original = r.original.clone();
        let (op, undo, done) = match &r.target {
            Target::Pane { pane, .. } => (
                Op::SetTitle { pane: pane.clone(), title: (!auto).then(|| text.clone()) },
                Op::SetTitle { pane: pane.clone(), title: r.renamed.then_some(original) },
                if auto { "back to the automatic title" } else { "renamed" },
            ),
            Target::Session { session } => (
                Op::RenameSession { session: session.clone(), name: text.clone() },
                Op::RenameSession { session: session.clone(), name: original },
                "renamed",
            ),
            Target::Window { window, .. } => (
                Op::RenameWindow { window: window.clone(), name: text.clone() },
                Op::RenameWindow { window: window.clone(), name: if r.renamed { original } else { String::new() } },
                if auto { "named after what runs" } else { "renamed" },
            ),
        };
        Action::Run(Job::new(op, done).undo(Some(undo)).retry(Retry::Rename(r)))
    }

    /// A job failed, the server saying WHY: put back what it came from. Its
    /// editor opens again with what was typed and why (true), if what it
    /// names is still there; an undo can be tried again.
    fn put_back(&mut self, retry: Retry, why: &str) -> bool {
        match retry {
            Retry::Rename(mut r) => {
                let there = match &r.target {
                    Target::Pane { pane, .. } => self.snap.agent(pane).is_some(),
                    Target::Session { session: id } | Target::Window { window: id, .. } => self.find(id).is_some(),
                };
                if !there || !matches!(self.mode, Mode::List) {
                    return false;
                }
                r.suggesting = None;
                (r.error, r.retry) = (Some(why.to_string()), false);
                self.keep = Some(r.target.clone());
                self.rebuild(false);
                if let Some(i) = self.position(&r.target) {
                    self.selected = i;
                }
                self.mode = Mode::Rename(r);
                true
            }
            Retry::Create(mut c) => {
                let there = match &c.new {
                    New::Window { session, after, .. } => self.window_at(session, after).is_some(),
                    New::Session { .. } => true,
                };
                if !there || !matches!(self.mode, Mode::List) {
                    return false;
                }
                c.error = Some(why.to_string());
                self.set_tab(Tab::Sessions);
                self.mode = Mode::Create(c);
                self.rebuild(false);
                true
            }
            // Tried again only while what it names is there: an undo that
            // can no longer be made would stand before every older one.
            Retry::Undo(op, what) => {
                if self.names(&op).iter().all(|id| self.find(id).is_some()) {
                    self.undo.push((op, what));
                }
                false
            }
        }
    }

    /// The sessions, windows and panes OP names, by id.
    fn names<'a>(&self, op: &'a Op) -> Vec<&'a str> {
        let ids: Vec<&String> = match op {
            Op::SetTitle { pane, .. } | Op::SetHidden { pane, .. } => vec![pane],
            Op::RenameSession { session, .. } => vec![session],
            Op::RenameWindow { window, .. } => vec![window],
            Op::Kill(Target::Session { session }) => vec![session],
            Op::Kill(Target::Window { session, window }) => vec![session, window],
            Op::Kill(Target::Pane { pane, .. }) => vec![pane],
            Op::SwapWindows { session, a, b } => vec![session, a, b],
            Op::SwapPanes(a, b) => vec![a, b],
            Op::MoveWindow { from, window, into, to, .. } => vec![from, window, into, to],
            Op::JoinPane { pane, to, .. } => vec![pane, to],
            Op::BreakPane { pane, session } => vec![pane, session],
            Op::NewWindow { session, after, .. } => vec![session, after],
            Op::SetHiddenExpanded(_) | Op::NewSession { .. } => Vec::new(),
        };
        ids.into_iter().map(String::as_str).collect()
    }

    /// The loop ran JOB, which made CREATED (if anything): remember how to
    /// undo it, say how it went, and return what to open.
    pub fn ran(&mut self, job: Job, result: Result<Option<Target>, String>) -> Option<Target> {
        match result {
            Ok(created) => {
                if let Op::SetHiddenExpanded(expanded) = &job.op {
                    self.hidden_expanded = *expanded;
                    if !expanded && let Some(i) = self.hidden_heading() {
                        self.selected = i;
                    }
                    self.rebuild(false);
                }
                if matches!(job.op, Op::SetHidden { .. }) {
                    self.keep = None;
                }
                self.follow = job.follow;
                if let Some(undo) = job.undo {
                    self.undo.push((undo, job.done.clone()));
                    self.status = Some(Status::Done(format!("{}   ^z undo", job.done)));
                } else {
                    self.status = Some(Status::Done(job.done));
                }
                created.filter(|_| job.open)
            }
            Err(e) => {
                if !job.retry.is_some_and(|r| self.put_back(*r, &e)) {
                    self.status = Some(Status::Failed(e));
                }
                None
            }
        }
    }

    fn undo(&mut self) -> Action {
        match self.undo.pop() {
            // A title (or visibility) for an agent that has exited would go
            // to the pane's next one.
            Some((Op::SetTitle { pane, .. } | Op::SetHidden { pane, .. }, _)) if self.snap.agent(&pane).is_none() => {
                self.note("that agent has exited");
                Action::None
            }
            Some((op, what)) => {
                let follow = match &op {
                    Op::MoveWindow { window: id, .. } | Op::JoinPane { pane: id, .. } | Op::SwapWindows { a: id, .. } | Op::SwapPanes(id, _) => Some(id.clone()),
                    _ => None,
                };
                let mut job = Job::new(op.clone(), format!("undone: {what}")).retry(Retry::Undo(op, what));
                job.follow = follow;
                Action::Run(job)
            }
            None => {
                self.status = Some(Status::Note("nothing to undo".into()));
                Action::None
            }
        }
    }

    fn note(&mut self, text: impl Into<String>) {
        self.status = Some(Status::Note(text.into()));
    }

    /// Where a window is: its session and the session's index, the window
    /// (first copy) and its index there.
    fn window_at(&self, session: &str, window: &str) -> Option<(usize, &Session, usize, &Window)> {
        let (si, s) = self.snap.sessions.iter().enumerate().find(|(_, s)| s.id == session)?;
        let (wi, w) = s.windows.iter().enumerate().find(|(_, w)| w.id == window)?;
        Some((si, s, wi, w))
    }

    /// Ask before killing the selected session, window or pane.
    fn kill(&mut self) {
        let Some(target) = self.selected_target().cloned() else { return };
        if matches!(target, Target::Session { .. }) && self.snap.sessions.len() == 1 {
            return self.note(if self.caps.rex { "that is the only session" } else { "that is the only session: kill the server instead" });
        }
        let (label, detail) = self.describe(&target);
        let detail = match (&target, detail.is_empty()) {
            (Target::Pane { pane, .. }, true) => self.snap.pane(pane).map(|(_, _, p)| p.command.clone()).unwrap_or_default(),
            (_, _) => detail,
        };
        let question = if detail.is_empty() { format!("kill {label}?") } else { format!("kill {label}? {detail}") };
        self.mode = Mode::Confirm(Confirm { job: Job::new(Op::Kill(target), format!("killed {label}")), question, dropped: false });
    }

    /// Start naming a new window (after the selected row's window, or last
    /// in a selected session) or a new session, in the selected row's
    /// folder. From the agents tab, the agent's window; the tree shows it.
    fn start_new(&mut self, session: bool) {
        let here = self.selected_target().cloned().unwrap_or(Target::Window { session: self.current_session.clone(), window: self.current_window.clone() });
        let window = match &here {
            Target::Session { session } => self.snap.session(session).and_then(|s| {
                let dir = s.active_window().and_then(|w| w.active_pane()).map(|p| p.path.clone());
                s.windows.last().map(|w| (s.id.clone(), w.id.clone(), dir))
            }),
            Target::Window { session, window } => self.window_at(session, window).map(|(_, _, _, w)| {
                (session.clone(), window.clone(), w.active_pane().map(|p| p.path.clone()))
            }),
            Target::Pane { session, window, pane } => Some((session.clone(), window.clone(), self.snap.pane(pane).map(|(_, _, p)| p.path.clone()))),
        };
        let Some((sid, after, dir)) = window else { return };
        let dir = dir.unwrap_or_else(|| self.home.clone());
        let (window, session_new) = (New::Window { session: sid, after, dir: dir.clone() }, New::Session { dir });
        let (new, other) = if session { (session_new, window) } else { (window, session_new) };
        let (from, back) = (self.selected_target().cloned(), self.tab);
        self.set_tab(Tab::Sessions);
        self.mode = Mode::Create(Create { from, back, new, other, field: LineEdit::new("", Some(TITLE_MAX)), error: None });
        self.rebuild(false);
    }

    /// Switch the row being named between a new window and a new session,
    /// keeping the name typed so far.
    fn switch_new(&mut self) {
        let Mode::Create(c) = &mut self.mode else { return };
        std::mem::swap(&mut c.new, &mut c.other);
        c.error = None;
        self.rebuild(false);
    }

    /// Create the window or session being named and go there, unless the
    /// server would refuse its name (the row says why); or cancel.
    fn finish_create(&mut self, save: bool) -> Action {
        let Mode::Create(c) = &mut self.mode else { return Action::None };
        let name = c.field.text().trim().to_string();
        if save
            && matches!(c.new, New::Session { .. })
            && let Some(why) = session_name_problem(&self.snap, self.caps.tmux_session_names, &name, None)
        {
            c.error = Some(why);
            return Action::None;
        }
        let Mode::Create(c) = std::mem::replace(&mut self.mode, Mode::List) else { return Action::None };
        // The new row goes: back to the row naming began on, not the one
        // that takes the new row's place; cancelled, to its tab too.
        if !save && c.back != self.tab {
            self.set_tab(c.back);
        } else {
            self.rebuild(false);
        }
        if let Some(i) = c.from.as_ref().and_then(|t| self.position(t)) {
            self.selected = i;
        }
        if !save {
            return Action::None;
        }
        let mut job = match &c.new {
            New::Session { dir } => Job::new(Op::NewSession { name: name.clone(), dir: dir.clone() }, format!("created session {name}")),
            New::Window { session, after, dir } => {
                Job::new(Op::NewWindow { session: session.clone(), after: after.clone(), name, dir: dir.clone() }, format!("created a {}", self.caps.window()))
            }
        };
        job.open = true;
        Action::Run(job.retry(Retry::Create(c)))
    }

    /// Do what COMMAND says, whether by key, tap or menu.
    pub fn run(&mut self, command: Command) -> Action {
        match (&mut self.mode, command) {
            (Mode::Type(_), Command::Cancel) => {
                self.mode = Mode::List;
                return Action::None;
            }
            (Mode::Type(_), _) => return Action::None,
            (Mode::Rename(_), Command::Save) => return self.finish_rename(true),
            (Mode::Rename(_), Command::Cancel) => return self.finish_rename(false),
            (Mode::Rename(_), Command::Suggest) => return self.suggest(),
            (Mode::Rename(_) | Mode::Create(_), Command::Clear) => {
                self.edit_name(|f| {
                    let had = !f.is_empty();
                    f.set("");
                    had
                });
                return Action::None;
            }
            (Mode::Rename(_), _) => return Action::None,
            (Mode::Menu(_), Command::Cancel) => {
                self.mode = Mode::List;
                return Action::None;
            }
            (Mode::Menu(_), _) => self.mode = Mode::List,
            (Mode::Confirm(_), Command::Confirm) => {
                let Mode::Confirm(c) = std::mem::replace(&mut self.mode, Mode::List) else { unreachable!() };
                return Action::Run(c.job);
            }
            // A question a drop asked: the row carried is put down.
            (Mode::Confirm(c), _) if c.dropped => return self.put_down(None),
            (Mode::Confirm(_), _) => {
                self.mode = Mode::List;
                return Action::None;
            }
            (Mode::Move(_), Command::Confirm) => return self.drop_here(),
            (Mode::Move(_), Command::Cancel) => return self.put_down(None),
            (Mode::Move(_), Command::Preview) => {}
            (Mode::Move(_), _) => return Action::None,
            (Mode::Create(_), Command::Save) => return self.finish_create(true),
            (Mode::Create(_), Command::NewWindow | Command::NewSession) => {
                self.switch_new();
                return Action::None;
            }
            (Mode::Create(_), Command::Cancel) => return self.finish_create(false),
            (Mode::Create(_), _) => return Action::None,
            (Mode::List, _) => {}
        }
        match command {
            Command::Tab(tab) => self.set_tab(tab),
            Command::Open if self.hidden_selected() => return self.expand(None),
            Command::Open => return self.selected_target().cloned().map_or(Action::None, Action::Activate),
            Command::Panes => return self.expand(None),
            Command::Hidden => {
                if let Some(i) = self.hidden_heading() {
                    self.selected = i;
                    return self.expand(None);
                }
            }
            Command::Hide | Command::Show => {
                if self.can(command, true) && let Some((_, pane)) = self.selected_agent() {
                    let hidden = command == Command::Hide;
                    let undo = Op::SetHidden { pane: pane.clone(), hidden: !hidden };
                    let done = if hidden { "agent hidden (still running)" } else { "agent shown" };
                    return Action::Run(Job::new(Op::SetHidden { pane, hidden }, done).undo(Some(undo)));
                }
            }
            Command::Group if self.tab == Tab::Agents => {
                self.group = self.group.next();
                self.rebuild(false);
            }
            Command::Preview => self.preview = Some(!self.preview_on(self.compact)),
            Command::Type => self.start_typing(),
            Command::Next => self.go_next(),
            Command::Close => return Action::Quit,
            Command::Rename => self.rename(),
            Command::RenameSuggest => {
                self.rename();
                return self.suggest();
            }
            Command::Undo => return self.undo(),
            Command::Kill => self.kill(),
            Command::NewWindow => self.start_new(false),
            Command::NewSession => self.start_new(true),
            Command::Move => self.pick_up(false),
            Command::MoveUp => return self.shift(false),
            Command::MoveDown => return self.shift(true),
            Command::Menu => self.open_menu(Position::default(), self.preview_fits),
            Command::Clear => {
                self.query.set("");
                self.requery();
            }
            Command::Group | Command::Save | Command::Suggest | Command::Cancel | Command::Confirm => {}
        }
        Action::None
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        self.status = None;
        // The kitty protocol (Rex) tells ^[, ^m and ^i from Esc, Enter and
        // Tab; the list takes them as a legacy terminal sends them. Typing,
        // the pane gets them as they are.
        let key = match (&self.mode, key.code) {
            (Mode::Type(_), _) => key,
            (_, KeyCode::Char(c @ ('[' | 'm' | 'i'))) if key.modifiers == KeyModifiers::CONTROL => {
                let code = match c {
                    '[' => KeyCode::Esc,
                    'm' => KeyCode::Enter,
                    _ => KeyCode::Tab,
                };
                KeyEvent { code, modifiers: KeyModifiers::NONE, ..key }
            }
            _ => key,
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match &mut self.mode {
            // Every key goes to the pane (its program may need esc, ^c, ⏎),
            // but the one that leaves.
            Mode::Type(_) if keys::is_leave(&key) => return self.run(Command::Cancel),
            Mode::Type(t) => return Action::Send(t.pane.clone(), Input::Key(key)),
            Mode::Rename(_) | Mode::Create(_) => {
                return match key.code {
                    KeyCode::Enter => self.run(Command::Save),
                    KeyCode::Esc => self.run(Command::Cancel),
                    KeyCode::Char('c') if ctrl => self.run(Command::Cancel),
                    // Renaming: a suggestion. Creating: a window or a session.
                    KeyCode::Char('r') if ctrl => self.run(Command::Suggest),
                    KeyCode::Char('s') if ctrl => self.run(Command::NewSession),
                    _ => {
                        self.edit_name(|f| f.key(key) == Edited::Text);
                        Action::None
                    }
                };
            }
            Mode::Menu(menu) => {
                let n = menu.items.len();
                match key.code {
                    KeyCode::Esc => return self.run(Command::Cancel),
                    KeyCode::Char('c' | 'a') if ctrl => return self.run(Command::Cancel),
                    // ^a a: tmux's prefix a again, which opened the popup.
                    KeyCode::Char('a') if menu.by_key && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META) => return self.run(Command::Close),
                    KeyCode::Char('A') if menu.by_key && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META) => return self.run(Command::Next),
                    KeyCode::Enter => {
                        let command = menu.items[menu.selected].command;
                        return self.run(command);
                    }
                    KeyCode::Up => menu.selected = (menu.selected + n - 1) % n,
                    KeyCode::Char('p' | 'k') if ctrl => menu.selected = (menu.selected + n - 1) % n,
                    KeyCode::Down => menu.selected = (menu.selected + 1) % n,
                    KeyCode::Char('n' | 'j') if ctrl => menu.selected = (menu.selected + 1) % n,
                    // Any other key closes the menu and does what it does.
                    _ => {
                        self.mode = Mode::List;
                        return self.on_key(key);
                    }
                }
                return Action::None;
            }
            Mode::Confirm(_) => {
                // Not ^x or Delete: they asked, and a held key repeats.
                let yes = key.code == KeyCode::Enter;
                return self.run(if yes { Command::Confirm } else { Command::Cancel });
            }
            Mode::Move(_) => match key.code {
                KeyCode::Enter => return self.run(Command::Confirm),
                KeyCode::Esc => return self.run(Command::Cancel),
                KeyCode::Char('c' | 't') if ctrl => return self.run(Command::Cancel),
                // Moving, filtering and the preview work as in the list.
                _ => {}
            },
            Mode::List => {}
        }
        match key.code {
            // A query first, then the popup.
            KeyCode::Esc => return self.run(if self.query.is_empty() { Command::Close } else { Command::Clear }),
            KeyCode::Char('c') if ctrl => return self.run(Command::Close),
            KeyCode::Char('a') if ctrl => {
                let action = self.run(Command::Menu);
                if let Mode::Menu(m) = &mut self.mode {
                    m.by_key = true;
                }
                return action;
            }
            KeyCode::Char('f') if ctrl => return self.run(Command::Next),
            KeyCode::Enter => return self.run(Command::Open),
            KeyCode::Tab | KeyCode::BackTab => return self.run(Command::Tab(self.tab.other())),
            KeyCode::Char('g') if ctrl => return self.run(Command::Group),
            KeyCode::Char('e') if ctrl => return self.run(if self.selected_agent_hidden() { Command::Show } else { Command::Hide }),
            KeyCode::Char('r') if ctrl => return self.run(Command::Rename),
            KeyCode::Char('l') if ctrl => return self.run(Command::Type),
            KeyCode::Char('z') if ctrl => return self.run(Command::Undo),
            KeyCode::Char('x') if ctrl => return self.run(Command::Kill),
            KeyCode::Delete => return self.run(Command::Kill),
            KeyCode::Char('t') if ctrl => return self.run(Command::Move),
            KeyCode::Char('s') if ctrl => return self.run(Command::NewWindow),
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => return self.run(Command::MoveUp),
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => return self.run(Command::MoveDown),
            // ^/ too: terminals send it as ^_, which crossterm reports as
            // ctrl-7 or ctrl-_.
            KeyCode::Char('o' | '/' | '7' | '_') if ctrl => return self.run(Command::Preview),
            KeyCode::Up => self.move_by(-1, true),
            KeyCode::Down => self.move_by(1, true),
            KeyCode::Char('p' | 'k') if ctrl => self.move_by(-1, true),
            KeyCode::Char('n' | 'j') if ctrl => self.move_by(1, true),
            KeyCode::PageUp => self.move_by(-PAGE, false),
            KeyCode::PageDown => self.move_by(PAGE, false),
            KeyCode::Right => return self.expand(Some(true)),
            KeyCode::Left => return self.expand(Some(false)),
            // The query is typed at its end (←/→ belong to the tree): text,
            // Backspace, ^w and ^u. Not ^h: it leaves typing, and one held or
            // pressed twice would eat the query and move the selection.
            KeyCode::Backspace => self.edit_query(key),
            KeyCode::Char('w' | 'u') if ctrl => self.edit_query(key),
            KeyCode::Char(_) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META) => self.edit_query(key),
            _ => {}
        }
        Action::None
    }
}

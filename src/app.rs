//! Popup state: the snapshot, the current tab's rows, the selection, the
//! filter and the mode (the list, renaming, a menu). Where things landed on
//! screen is the View, written by each draw.
//!
//! Every action has one Command, whether it comes from a key, a footer hint,
//! a menu item or a tap; `can` says which apply to the selected row, and
//! `hints` and `menu_items` list them.

use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::edit::{Edited, LineEdit};
use crate::model::{Pane, Session, Snapshot, Target, Window};
use crate::rows::{self, Filter, Group, New, Row, RowKind, Tree};

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

/// What the popup asks the loop to do.
#[derive(Debug)]
pub enum Action {
    None,
    Quit,
    Activate(Target),
    /// Run the job's change on the tmux server, then report through `ran`.
    Run(Job),
    /// Ask the suggest command for PANE's title; the answer comes back
    /// through `suggested` with the same ID.
    Suggest { pane: String, id: u64 },
}

/// A change paddock makes on the tmux server. Sessions, windows and panes
/// are named by id.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Set PANE's @agent-description; None unsets it (back to automatic).
    SetTitle { pane: String, title: Option<String> },
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
}

impl Job {
    fn new(op: Op, done: impl Into<String>) -> Job {
        Job { op, undo: None, done: done.into(), follow: None, open: false }
    }

    fn undo(mut self, op: Option<Op>) -> Job {
        self.undo = op;
        self
    }

    fn follow(mut self, id: &str) -> Job {
        self.follow = Some(id.to_string());
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

/// Every action, whatever triggers it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    Tab(Tab),
    Open,
    /// Expand or collapse the selected window's panes.
    Panes,
    Group,
    Preview,
    Close,
    Rename,
    /// Rename, starting with a suggestion (a menu item for the mouse).
    RenameSuggest,
    Kill,
    /// Name a new window, after the selected one, or a new session (^s;
    /// ^s again switches between them).
    NewWindow,
    NewSession,
    /// Pick the selected window or pane up, to drop it elsewhere.
    Move,
    /// Swap with the row above or below (across sessions and windows at
    /// the ends).
    MoveUp,
    MoveDown,
    Undo,
    /// The selected row's menu (the footer's ⋯, opened by on_mouse).
    Menu,
    // While renaming:
    Save,
    Suggest,
    Clear,
    Cancel,
    /// Confirm (a kill), or drop what is carried.
    Confirm,
}

/// A footer hint or a menu item: the key, the word, and the command a tap
/// runs. Hints are listed in display order; `rank` says which go first when
/// room runs out (lower goes first).
#[derive(Clone, Debug, PartialEq)]
pub struct Hint {
    pub key: &'static str,
    pub label: String,
    pub command: Command,
    pub rank: u8,
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
}

/// A change waiting for a yes: what it is, and the question.
#[derive(Debug)]
pub struct Confirm {
    pub job: Job,
    pub question: String,
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
    pub new: New,
    /// The other one (^s switches): a session for a window, and back.
    other: New,
    pub field: LineEdit,
    /// Why the name was refused.
    pub error: Option<String>,
}

/// A window or pane picked up, to drop after the selected row.
#[derive(Debug)]
pub struct Carry {
    pub target: Target,
    /// "window api", "pane webshop:1.1".
    pub label: String,
    /// Picked up by dragging: the release drops it.
    dragging: bool,
}

#[derive(Debug)]
pub struct Menu {
    /// The row it was opened on: its items act on that row only.
    target: Option<Target>,
    pub items: Vec<Hint>,
    pub selected: usize,
    /// The screen cell it hangs from (the click, or the selected row).
    pub anchor: Position,
}

#[derive(Debug)]
pub enum Mode {
    List,
    Rename(Rename),
    Menu(Menu),
    Confirm(Confirm),
    Move(Carry),
    Create(Create),
}

/// Titles are cut to this many characters (graphemes), as suggestions are.
const TITLE_MAX: usize = 80;
/// Two clicks on a row this close together open it.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
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

/// "codex waiting, claude working" for the agents among PANES.
fn agents_in<'a>(panes: impl Iterator<Item = &'a Pane>) -> String {
    let agents: Vec<String> = panes.filter_map(|p| p.agent.as_ref().map(|a| format!("{} {}", p.label(), a.state.label()))).collect();
    agents.join(", ")
}

/// The move that puts window WI of SESSION back where it is now, from
/// session FROM, where it is going.
fn window_return(session: &Session, wi: usize, from: &str) -> Option<Op> {
    let ws = &session.windows;
    let back = |to: &str, before| Op::MoveWindow { from: from.into(), window: ws[wi].id.clone(), into: session.id.clone(), to: to.into(), before };
    match (wi.checked_sub(1).map(|i| &ws[i]), ws.get(wi + 1)) {
        (Some(prev), _) => Some(back(&prev.id, false)),
        (None, Some(next)) => Some(back(&next.id, true)),
        _ => None,
    }
}

/// Why tmux would refuse NAME for a session (the one with id RENAMED keeps
/// its own name), if it would.
fn session_name_problem(snap: &Snapshot, name: &str, renamed: Option<&str>) -> Option<String> {
    if name.is_empty() {
        Some("a session needs a name".into())
    } else if name.contains(['.', ':']) {
        Some("session names can't have . or :".into())
    } else if snap.sessions.iter().any(|s| s.name == name && Some(s.id.as_str()) != renamed) {
        Some(format!("a session called {name} exists"))
    } else {
        None
    }
}

/// What a mouse press landed on; the release completes it there.
#[derive(Clone, Debug, PartialEq)]
enum Hit {
    /// A list row: its index when drawn, and its target.
    Row(usize, Target),
    Button(Command),
    /// A window's ▸/▾.
    Toggle(Target),
    MenuItem(usize),
}

struct Press {
    hit: Option<Hit>,
    at: Position,
    /// The row was already selected (compact: a tap on it opens).
    was_selected: bool,
}

/// Where the last draw put things: the scroll position and what is under
/// each screen cell. Owned by the loop, written by `ui::draw`.
pub struct View {
    /// Under 80 columns (a phone): fewer details, two-line rows, taps.
    pub compact: bool,
    /// Whether the preview has room. Assumed until the first draw, so the
    /// first frame already has it.
    pub preview_fits: bool,
    /// The first list row on screen.
    pub offset: usize,
    /// List rows: where, their index, their target.
    pub rows: Vec<(Rect, usize, Target)>,
    pub buttons: Vec<(Rect, Command)>,
    pub toggles: Vec<(Rect, Target)>,
    /// The open menu's box and its items.
    pub menu: Option<Rect>,
    pub items: Vec<(Rect, usize)>,
}

impl View {
    pub fn new() -> View {
        View {
            compact: false,
            preview_fits: true,
            offset: 0,
            rows: Vec::new(),
            buttons: Vec::new(),
            toggles: Vec::new(),
            menu: None,
            items: Vec::new(),
        }
    }

    /// Forget the last draw's hit areas.
    pub fn clear(&mut self) {
        self.rows.clear();
        self.buttons.clear();
        self.toggles.clear();
        self.menu = None;
        self.items.clear();
    }

    /// Scroll so the selection (with a header right above it) is visible in
    /// HEIGHT lines, and no space is left empty below the last row.
    pub fn scroll(&mut self, rows: &[Row], selected: usize, height: u16) {
        let n = rows.len();
        if n == 0 {
            self.offset = 0;
            return;
        }
        let h = |i: usize| rows[i].height(self.compact);
        let sel = selected.min(n - 1);
        let top = if sel > 0 && rows[sel - 1].target.is_none() { sel - 1 } else { sel };
        self.offset = self.offset.min(top);
        while self.offset < sel && (self.offset..=sel).map(h).sum::<u16>() > height {
            self.offset += 1;
        }
        let mut below: u16 = (self.offset..n).map(h).sum();
        while self.offset > 0 && below + h(self.offset - 1) <= height {
            self.offset -= 1;
            below += h(self.offset);
        }
    }

    /// What is under AT, menus first (they are drawn on top).
    fn hit(&self, at: Position) -> Option<Hit> {
        let inside = |r: &Rect| r.contains(at);
        if let Some(menu) = self.menu {
            return self.items.iter().find(|(r, _)| inside(r)).map(|&(_, i)| Hit::MenuItem(i)).filter(|_| inside(&menu));
        }
        (self.buttons.iter().find(|(r, _)| inside(r)).map(|&(_, c)| Hit::Button(c)))
            .or_else(|| self.toggles.iter().find(|(r, _)| inside(r)).map(|(_, t)| Hit::Toggle(t.clone())))
            .or_else(|| self.rows.iter().find(|(r, ..)| inside(r)).map(|(_, i, t)| Hit::Row(*i, t.clone())))
    }
}

pub struct App {
    pub(crate) snap: Snapshot,
    pub(crate) tab: Tab,
    pub(crate) group: Group,
    pub(crate) query: LineEdit,
    pub(crate) rows: Vec<Row>,
    pub(crate) selected: usize,
    pub(crate) preview_on: bool,
    pub(crate) mode: Mode,
    pub(crate) status: Option<Status>,
    /// Whether a suggest command is configured (@agent-suggest-command).
    pub(crate) can_suggest: bool,
    current_session: String,
    current_window: String,
    expanded: HashSet<Target>,
    collapsed: HashSet<Target>,
    /// Listed whatever the filter says until the query changes (just renamed).
    keep: Option<Target>,
    filter: Filter,
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
}

impl App {
    pub fn new(snap: Snapshot, tab: Tab, current_session: String, current_window: String, home: String, can_suggest: bool) -> App {
        let mut app = App {
            snap,
            tab,
            group: Group::Status,
            query: LineEdit::default(),
            rows: Vec::new(),
            selected: 0,
            preview_on: true,
            mode: Mode::List,
            status: None,
            can_suggest,
            current_session,
            current_window,
            expanded: HashSet::new(),
            collapsed: HashSet::new(),
            keep: None,
            filter: Filter::new(),
            home,
            suggestions: 0,
            undo: Vec::new(),
            press: None,
            last_click: None,
            follow: None,
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

    /// The pane to preview, if the preview is on screen.
    pub fn preview_pane(&self, view: &View) -> Option<&str> {
        if !(self.preview_on && view.preview_fits) {
            return None;
        }
        self.rows.get(self.selected)?.preview.as_deref()
    }

    /// The suggestion the loop should still be waiting for.
    pub fn suggesting(&self) -> Option<u64> {
        match &self.mode {
            Mode::Rename(r) => r.suggesting,
            _ => None,
        }
    }

    pub fn set_snapshot(&mut self, snap: Snapshot) {
        self.snap = snap;
        // What is being renamed went away: there is nothing left to name.
        if let Mode::Rename(r) = &self.mode {
            let gone = match &r.target {
                Target::Pane { pane, .. } => self.snap.agent(pane).is_none().then_some("that agent has exited"),
                Target::Session { session } => self.find(session).is_none().then_some("that session has closed"),
                Target::Window { window, .. } => self.find(window).is_none().then_some("that window has closed"),
            };
            if let Some(why) = gone {
                self.mode = Mode::List;
                self.status = Some(Status::Failed(why.into()));
            }
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
            self.rebuild(false);
            // A pane alone in its window is shown as the window.
            let window = match &target {
                Target::Pane { session, window, .. } => Some(Target::Window { session: session.clone(), window: window.clone() }),
                _ => None,
            };
            if let Some(i) = self.position(&target).or_else(|| window.and_then(|w| self.position(&w))) {
                self.selected = i;
            }
        } else {
            self.rebuild(false);
        }
        // The menu's row went away: its items would act on the next one.
        if let Mode::Menu(m) = &self.mode
            && m.target.as_ref() != self.selected_target()
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
        let old = self.selected;
        self.rows = match self.tab {
            Tab::Sessions => {
                let tree = Tree {
                    expanded: &self.expanded,
                    collapsed: &self.collapsed,
                    keep: self.keep.as_ref(),
                    current: &self.current_session,
                    home: &self.home,
                };
                let mut rows = rows::sessions(&self.snap, &mut self.filter, &tree);
                if let Mode::Create(c) = &self.mode {
                    rows::place_new(&mut rows, &c.new);
                }
                rows
            }
            Tab::Agents => {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                rows::agents(&self.snap, &mut self.filter, self.group, self.keep.as_ref(), &self.home, now)
            }
        };
        let kept = keep.as_ref().and_then(|t| self.locate(t, old));
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
                self.position(&Target::Window { session: self.current_session.clone(), window: self.current_window.clone() })
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
        self.rows.get(i).is_some_and(|r| r.target.is_some())
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

    /// Expand (Some(true)), collapse (Some(false)) or toggle (None) the
    /// selected row's window; collapsing from a pane selects its window.
    fn expand(&mut self, how: Option<bool>) {
        let Some((window, open)) = self.selected_window() else { return };
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
    /// paste: a newline in it must not act as Enter).
    pub fn paste(&mut self, text: &str) {
        match &mut self.mode {
            Mode::Rename(_) | Mode::Create(_) => self.edit_name(|f| f.insert(text)),
            Mode::Menu(_) | Mode::Confirm(_) => {}
            Mode::List | Mode::Move(_) => {
                if self.query.insert(text) {
                    self.requery();
                }
            }
        }
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
            self.status = Some(Status::Note("no title suggestions: nothing sets @agent-suggest-command".into()));
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
            Err(e) => r.error = Some(e),
        }
    }

    /// Finish renaming: Save sets the name when it changed (an empty title
    /// or window name: back to automatic), unless tmux would refuse it, which
    /// keeps the field open with the reason; anything else cancels.
    fn finish_rename(&mut self, save: bool) -> Action {
        let Mode::Rename(r) = &mut self.mode else { return Action::None };
        let text = r.field.text().trim().to_string();
        if !save || text == r.unchanged || (text.is_empty() && !r.renamed) {
            self.mode = Mode::List;
            return Action::None;
        }
        if let Target::Session { session } = &r.target
            && let Some(why) = session_name_problem(&self.snap, &text, Some(session))
        {
            r.error = Some(why);
            return Action::None;
        }
        let Mode::Rename(r) = std::mem::replace(&mut self.mode, Mode::List) else { return Action::None };
        // The row stays listed even if its new name doesn't match the filter.
        self.keep = Some(r.target.clone());
        let auto = text.is_empty();
        let (op, undo, done) = match r.target {
            Target::Pane { pane, .. } => (
                Op::SetTitle { pane: pane.clone(), title: (!auto).then(|| text.clone()) },
                Op::SetTitle { pane, title: r.renamed.then_some(r.original) },
                if auto { "back to the automatic title" } else { "renamed" },
            ),
            Target::Session { session } => (
                Op::RenameSession { session: session.clone(), name: text.clone() },
                Op::RenameSession { session, name: r.original },
                "renamed",
            ),
            Target::Window { window, .. } => (
                Op::RenameWindow { window: window.clone(), name: text.clone() },
                Op::RenameWindow { window, name: if r.renamed { r.original } else { String::new() } },
                if auto { "named after what runs" } else { "renamed" },
            ),
        };
        Action::Run(Job::new(op, done).undo(Some(undo)))
    }

    /// The loop ran JOB, which made CREATED (if anything): remember how to
    /// undo it, say how it went, and return what to open.
    pub fn ran(&mut self, job: Job, result: Result<Option<Target>, String>) -> Option<Target> {
        match result {
            Ok(created) => {
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
                self.status = Some(Status::Failed(e));
                None
            }
        }
    }

    fn undo(&mut self) -> Action {
        match self.undo.pop() {
            // A title for an agent that has exited would go to the pane's next one.
            Some((Op::SetTitle { pane, .. }, _)) if self.snap.agent(&pane).is_none() => {
                self.note("that agent has exited");
                Action::None
            }
            Some((op, what)) => {
                let follow = match &op {
                    Op::MoveWindow { window: id, .. } | Op::JoinPane { pane: id, .. } | Op::SwapWindows { a: id, .. } | Op::SwapPanes(id, _) => Some(id.clone()),
                    _ => None,
                };
                let mut job = Job::new(op, format!("undone: {what}"));
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

    /// TARGET in words, and what it holds: ("window webshop:2 web",
    /// "2 panes · codex waiting").
    fn describe(&self, target: &Target) -> (String, String) {
        let with = |count: String, agents: String| if agents.is_empty() { count } else { format!("{count} · {agents}") };
        let plural = |n: usize, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
        match target {
            Target::Session { session } => match self.snap.session(session) {
                Some(s) => (format!("session {}", s.name), with(plural(s.windows.len(), "window"), agents_in(s.windows.iter().flat_map(|w| &w.panes)))),
                None => ("session".into(), String::new()),
            },
            Target::Window { session, window } => match self.window_at(session, window) {
                Some((_, s, _, w)) => {
                    (format!("window {}:{} {}", s.name, w.index, w.name), with(plural(w.panes.len(), "pane"), agents_in(w.panes.iter())))
                }
                None => ("window".into(), String::new()),
            },
            Target::Pane { pane, .. } => match self.snap.pane(pane) {
                Some((s, w, p)) => (format!("pane {}:{}.{}", s.name, w.index, p.index), agents_in(std::iter::once(p))),
                None => ("pane".into(), String::new()),
            },
        }
    }

    /// Ask before killing the selected session, window or pane.
    fn kill(&mut self) {
        let Some(target) = self.selected_target().cloned() else { return };
        if matches!(target, Target::Session { .. }) && self.snap.sessions.len() == 1 {
            return self.note("that is the only session: kill the server instead");
        }
        let (label, detail) = self.describe(&target);
        let detail = match (&target, detail.is_empty()) {
            (Target::Pane { pane, .. }, true) => self.snap.pane(pane).map(|(_, _, p)| p.command.clone()).unwrap_or_default(),
            (_, _) => detail,
        };
        let question = if detail.is_empty() { format!("kill {label}?") } else { format!("kill {label}? {detail}") };
        self.mode = Mode::Confirm(Confirm { job: Job::new(Op::Kill(target), format!("killed {label}")), question });
    }

    /// Run JOB now, or ask first when QUESTION says it does more than it
    /// seems (a move that closes a session).
    fn maybe_confirm(&mut self, job: Job, question: Option<String>) -> Action {
        match question {
            Some(question) => {
                self.mode = Mode::Confirm(Confirm { job, question });
                Action::None
            }
            None => Action::Run(job),
        }
    }

    /// Swap the selected window or pane with its neighbour; past the first or
    /// last, into the previous or next session (window) or window (pane).
    fn shift(&mut self, down: bool) -> Action {
        if self.tab != Tab::Sessions {
            return Action::None;
        }
        let result = match self.selected_target().cloned() {
            Some(Target::Window { session, window }) => self.shift_window(&session, &window, down),
            Some(Target::Pane { session, window, pane }) => self.shift_pane(&session, &window, &pane, down),
            Some(Target::Session { .. }) => Err("sessions are listed by name: rename one to move it".into()),
            _ => return Action::None,
        };
        match result {
            Ok((job, question)) => self.maybe_confirm(job, question),
            Err(why) => {
                self.note(why);
                Action::None
            }
        }
    }

    fn shift_window(&self, session: &str, window: &str, down: bool) -> Result<(Job, Option<String>), String> {
        let (si, s, wi, _) = self.window_at(session, window).ok_or("that window has closed")?;
        let sessions = &self.snap.sessions;
        let ws = &s.windows;
        let neighbour = if down { ws.get(wi + 1) } else { wi.checked_sub(1).map(|i| &ws[i]) };
        if let Some(n) = neighbour.filter(|n| n.id != window) {
            let op = Op::SwapWindows { session: session.into(), a: window.into(), b: n.id.clone() };
            return Ok((Job::new(op.clone(), if down { "moved down" } else { "moved up" }).undo(Some(op)).follow(window), None));
        }
        let other = if down { sessions.get(si + 1) } else { si.checked_sub(1).map(|i| &sessions[i]) };
        let other = other.ok_or(if down { "already the last window" } else { "already the first window" })?;
        let to = if down { other.windows.first() } else { other.windows.last() }.ok_or("that session has no windows")?;
        let op = Op::MoveWindow { from: session.into(), window: window.into(), into: other.id.clone(), to: to.id.clone(), before: down };
        let job = Job::new(op, format!("moved to {}", other.name)).undo(window_return(s, wi, &other.id)).follow(window);
        let closes = (ws.len() == 1).then(|| format!("move window {} to {}? {} closes: it is its only window", ws[wi].name, other.name, s.name));
        Ok((job, closes))
    }

    /// The move that puts PANE back beside its neighbours in its window.
    fn pane_return(&self, session: &str, window: &str, pane: &str) -> Option<Op> {
        let (_, _, _, w) = self.window_at(session, window)?;
        let ps = &w.panes;
        let pi = ps.iter().position(|p| p.id == pane)?;
        let to = |p: &Pane, before| Op::JoinPane { pane: pane.into(), to: p.id.clone(), before };
        match (pi.checked_sub(1).map(|i| &ps[i]), ps.get(pi + 1)) {
            (Some(prev), _) => Some(to(prev, false)),
            (None, Some(next)) => Some(to(next, true)),
            _ => None,
        }
    }

    fn shift_pane(&self, session: &str, window: &str, pane: &str, down: bool) -> Result<(Job, Option<String>), String> {
        let (_, _, _, w) = self.window_at(session, window).ok_or("that pane has closed")?;
        let ps = &w.panes;
        let pi = ps.iter().position(|p| p.id == pane).ok_or("that pane has closed")?;
        if ps.len() == 1 {
            return Err("it is its window's only pane: move the window".into());
        }
        let neighbour = if down { ps.get(pi + 1) } else { pi.checked_sub(1).map(|i| &ps[i]) };
        if let Some(n) = neighbour {
            let op = Op::SwapPanes(pane.into(), n.id.clone());
            return Ok((Job::new(op.clone(), if down { "moved down" } else { "moved up" }).undo(Some(op)).follow(pane), None));
        }
        // Into the next (previous) window, in tree order across sessions.
        let windows: Vec<(&Session, &Window)> = self.snap.sessions.iter().flat_map(|s| s.windows.iter().map(move |w| (s, w))).collect();
        let here = windows.iter().position(|(s, w)| s.id == session && w.id == window).ok_or("that pane has closed")?;
        let other = if down { windows.get(here + 1) } else { here.checked_sub(1).map(|i| &windows[i]) };
        let (s, w) = other.ok_or(if down { "already the last pane" } else { "already the first pane" })?;
        let to = if down { w.panes.first() } else { w.panes.last() }.ok_or("that window has no panes")?;
        let op = Op::JoinPane { pane: pane.into(), to: to.id.clone(), before: down };
        Ok((Job::new(op, format!("moved to {}:{} {}", s.name, w.index, w.name)).undo(self.pane_return(session, window, pane)).follow(pane), None))
    }

    /// Pick up the selected window or pane (sessions tab).
    fn pick_up(&mut self, dragging: bool) {
        if !self.movable() {
            if self.tab == Tab::Sessions {
                self.note("pick up a window or a pane");
            }
            return;
        }
        let Some(target) = self.selected_target().cloned() else { return };
        let (label, _) = self.describe(&target);
        self.keep = Some(target.clone());
        self.mode = Mode::Move(Carry { target, label, dragging });
    }

    /// What dropping what is carried after the selected row does: the job
    /// and, when it closes a session or window, the question to ask first;
    /// None when it is already there.
    pub fn drop_job(&self) -> Result<Option<(Job, Option<String>)>, String> {
        let Mode::Move(carry) = &self.mode else { return Ok(None) };
        let at = self.selected_target().ok_or("choose a session, window or pane")?;
        match (&carry.target, at) {
            (Target::Window { session, window }, at) => {
                let (_, s, wi, _) = self.window_at(session, window).ok_or("that window has closed")?;
                let ws = &s.windows;
                let (into, to, before, done) = match at {
                    Target::Session { session: other } => {
                        let o = self.snap.session(other).ok_or("that session has closed")?;
                        let first = o.windows.first().ok_or("that session has no windows")?;
                        (other.clone(), first.id.clone(), true, format!("moved to the top of {}", o.name))
                    }
                    Target::Window { session: other, window: to } | Target::Pane { session: other, window: to, .. } => {
                        let (_, o, _, w) = self.window_at(other, to).ok_or("that window has closed")?;
                        (other.clone(), to.clone(), false, format!("moved after {}:{} {}", o.name, w.index, w.name))
                    }
                };
                let already = to == *window || (!before && wi > 0 && ws[wi - 1].id == to) || (before && ws.get(wi + 1).is_some_and(|w| w.id == to));
                if already {
                    return Ok(None);
                }
                let leaving = into != *session;
                let undo = window_return(s, wi, &into);
                let op = Op::MoveWindow { from: session.clone(), window: window.clone(), into, to, before };
                let job = Job::new(op, done).undo(undo).follow(window);
                let closes = (ws.len() == 1 && leaving).then(|| format!("move {}? {} closes: it is its only window", carry.label, s.name));
                Ok(Some((job, closes)))
            }
            (Target::Pane { session, window, pane }, at) => {
                let undo = self.pane_return(session, window, pane);
                let (op, done) = match at {
                    Target::Session { session: other } => {
                        let name = self.snap.session(other).map(|s| s.name.clone()).unwrap_or_default();
                        (Op::BreakPane { pane: pane.clone(), session: other.clone() }, format!("a window of its own in {name}"))
                    }
                    Target::Window { session: other, window: to } => {
                        if to == window {
                            return Ok(None);
                        }
                        let (_, _, _, w) = self.window_at(other, to).ok_or("that window has closed")?;
                        let last = w.panes.last().ok_or("that window has no panes")?;
                        (Op::JoinPane { pane: pane.clone(), to: last.id.clone(), before: false }, format!("moved into {}", w.name))
                    }
                    Target::Pane { window: to_window, pane: to, .. } => {
                        if to == pane {
                            return Ok(None);
                        }
                        if to_window == window {
                            let op = Op::SwapPanes(pane.clone(), to.clone());
                            return Ok(Some((Job::new(op.clone(), "swapped").undo(Some(op)).follow(pane), None)));
                        }
                        (Op::JoinPane { pane: pane.clone(), to: to.clone(), before: false }, "moved".to_string())
                    }
                };
                let alone = self.window_at(session, window).is_some_and(|(_, _, _, w)| w.panes.len() == 1);
                let closes = alone.then(|| format!("move {}? its window closes: it is its only pane", carry.label));
                Ok(Some((Job::new(op, done).undo(undo).follow(pane), closes)))
            }
            _ => Err("pick up a window or a pane".into()),
        }
    }

    /// What ⏎ would do with what is carried, said on the selected row: Ok
    /// when it would do something.
    pub fn drop_note(&self) -> Result<String, String> {
        match self.drop_job() {
            Ok(Some((job, _))) => Ok(match &job.op {
                Op::MoveWindow { before: true, .. } if matches!(self.selected_target(), Some(Target::Session { .. })) => "first in this session",
                Op::MoveWindow { .. } => "after this window",
                Op::JoinPane { .. } if matches!(self.selected_target(), Some(Target::Window { .. })) => "into this window",
                Op::JoinPane { .. } => "beside this pane",
                Op::SwapPanes(..) => "swap with this pane",
                Op::BreakPane { .. } => "a new window here",
                _ => "here",
            }
            .to_string()),
            Ok(None) => Err("already here".into()),
            Err(why) => Err(why),
        }
    }

    /// Drop what is carried after the selected row.
    fn drop_here(&mut self) -> Action {
        let result = self.drop_job();
        self.mode = Mode::List;
        match result {
            Ok(Some((job, question))) => self.maybe_confirm(job, question),
            Ok(None) => {
                self.note("already there");
                Action::None
            }
            Err(why) => {
                self.note(why);
                Action::None
            }
        }
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
        self.set_tab(Tab::Sessions);
        let from = self.selected_target().cloned();
        self.mode = Mode::Create(Create { from, new, other, field: LineEdit::new("", Some(TITLE_MAX)), error: None });
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

    /// Create the window or session being named and go there, unless tmux
    /// would refuse its name (the row says why); or cancel.
    fn finish_create(&mut self, save: bool) -> Action {
        let Mode::Create(c) = &mut self.mode else { return Action::None };
        let name = c.field.text().trim().to_string();
        if save
            && matches!(c.new, New::Session { .. })
            && let Some(why) = session_name_problem(&self.snap, &name, None)
        {
            c.error = Some(why);
            return Action::None;
        }
        let Mode::Create(c) = std::mem::replace(&mut self.mode, Mode::List) else { return Action::None };
        // The new row goes: back to the row naming began on, not the one
        // that takes the new row's place.
        self.rebuild(false);
        if let Some(i) = c.from.as_ref().and_then(|t| self.position(t)) {
            self.selected = i;
        }
        if !save {
            return Action::None;
        }
        let mut job = match c.new {
            New::Session { dir } => Job::new(Op::NewSession { name: name.clone(), dir }, format!("created session {name}")),
            New::Window { session, after, dir } => Job::new(Op::NewWindow { session, after, name, dir }, "created a window"),
        };
        job.open = true;
        Action::Run(job)
    }

    /// What the selected row can be renamed as, if anything.
    fn renamable(&self) -> bool {
        match self.selected_target() {
            Some(Target::Session { .. } | Target::Window { .. }) => true,
            Some(Target::Pane { .. }) => self.selected_agent().is_some(),
            _ => false,
        }
    }

    /// The selected row's menu, hanging from ANCHOR. PREVIEW_FITS: whether
    /// toggling the preview would show anything.
    fn open_menu(&mut self, anchor: Position, preview_fits: bool) {
        let items = self.menu_items(preview_fits);
        if !items.is_empty() {
            self.mode = Mode::Menu(Menu { target: self.selected_target().cloned(), items, selected: 0, anchor });
        }
    }

    /// Whether the selected row is a window or a pane to move (sessions tab).
    fn movable(&self) -> bool {
        self.tab == Tab::Sessions && matches!(self.selected_target(), Some(Target::Window { .. } | Target::Pane { .. }))
    }

    /// Whether COMMAND applies in the list, to the selected row: what the
    /// footer and the menu offer. PREVIEW_FITS: whether the preview has room.
    fn can(&self, command: Command, preview_fits: bool) -> bool {
        match command {
            Command::Open | Command::Kill => self.selected_target().is_some(),
            Command::Panes => self.selected_window().is_some(),
            Command::Rename => self.renamable(),
            Command::RenameSuggest => self.can_suggest && self.selected_agent().is_some(),
            Command::Move | Command::MoveUp | Command::MoveDown => self.movable(),
            Command::Undo => !self.undo.is_empty(),
            Command::Group => self.tab == Tab::Agents,
            Command::Preview => preview_fits,
            _ => true,
        }
    }

    /// COMMAND's key, as hints and menu items show it (on_key binds them).
    fn key(&self, command: Command) -> &'static str {
        match command {
            Command::Open | Command::Save | Command::Confirm => "⏎",
            Command::Close | Command::Cancel => "esc",
            Command::Tab(_) => "⇥",
            Command::Panes if self.selected_window().is_some_and(|(_, open)| open) => "←",
            Command::Panes => "→",
            Command::Group => "^g",
            Command::Preview => "^o",
            Command::Rename | Command::Suggest => "^r",
            Command::RenameSuggest => "^r^r",
            Command::Kill => "^x",
            Command::NewWindow => "^s",
            // Naming a new window, ^s switches to a session.
            Command::NewSession if matches!(self.mode, Mode::Create(_)) => "^s",
            Command::NewSession => "^s^s",
            Command::Move => "^t",
            Command::MoveUp => "⇧↑",
            Command::MoveDown => "⇧↓",
            Command::Undo => "^z",
            Command::Clear => "^u",
            Command::Menu => "",
        }
    }

    fn hint(&self, command: Command, label: impl Into<String>, rank: u8) -> Hint {
        Hint { key: self.key(command), label: label.into(), command, rank }
    }

    /// What can be done with the selected row, and in the popup.
    pub fn menu_items(&self, preview_fits: bool) -> Vec<Hint> {
        let open = self.selected_window().is_some_and(|(_, open)| open);
        let group = format!("group by {}", self.group.next().label());
        let tab = format!("{} tab", self.tab.other().label());
        [
            (Command::Open, "open"),
            (Command::Panes, if open { "hide panes" } else { "show panes" }),
            (Command::Rename, "rename"),
            (Command::RenameSuggest, "suggest a title"),
            (Command::Move, "move…"),
            (Command::MoveUp, "move up"),
            (Command::MoveDown, "move down"),
            (Command::Kill, "kill…"),
            (Command::NewWindow, "new window here"),
            (Command::NewSession, "new session"),
            (Command::Undo, "undo"),
            (Command::Group, &group),
            (Command::Preview, if self.preview_on { "hide preview" } else { "show preview" }),
            (Command::Tab(self.tab.other()), &tab),
        ]
        .into_iter()
        .filter(|&(c, _)| self.can(c, preview_fits))
        .map(|(c, label)| self.hint(c, label, 0))
        .collect()
    }

    /// The footer: hints in display order, and the one pinned on the right.
    pub fn hints(&self, view: &View) -> (Vec<Hint>, Hint) {
        let cancel = self.hint(Command::Cancel, "cancel", 0);
        match &self.mode {
            Mode::Rename(r) => {
                let mut out = vec![self.hint(Command::Save, "save", 0)];
                if self.can_suggest {
                    out.push(self.hint(Command::Suggest, if r.error.is_some() { "retry" } else { "suggest" }, 1));
                }
                out.push(self.hint(Command::Clear, "clear", 2));
                return (out, cancel);
            }
            Mode::Menu(_) => return (vec![self.hint(Command::Open, "choose", 0)], self.hint(Command::Cancel, "back", 0)),
            Mode::Confirm(c) => return (vec![self.hint(Command::Confirm, if c.kills() { "kill" } else { "move" }, 0)], cancel),
            Mode::Move(_) => return (vec![self.hint(Command::Confirm, "drop here", 0)], cancel),
            Mode::Create(c) => {
                let other = if matches!(c.new, New::Window { .. }) { "session" } else { "window" };
                return (vec![self.hint(Command::Save, "create", 0), self.hint(Command::NewSession, other, 1), self.hint(Command::Clear, "clear", 2)], cancel);
            }
            Mode::List => {}
        }
        let open = self.selected_window().is_some_and(|(_, open)| open);
        let other = self.tab.other();
        let group = if view.compact { "group".to_string() } else { format!("group: {}", self.group.label()) };
        let hints = [
            (Command::Open, "open", 0),
            // Compact (words alone), the other tab's name.
            (Command::Tab(other), if view.compact { other.label() } else { "tab" }, 3),
            (Command::Panes, if open { "hide panes" } else { "panes" }, 4),
            (Command::Group, &group, 5),
            (Command::Rename, "rename", 1),
            (Command::Move, "move", 4),
            (Command::Kill, "kill", 5),
            (Command::NewWindow, "new", 7),
            (Command::Undo, "undo", 2),
            (Command::Preview, "preview", 6),
        ];
        // New windows show in the sessions tab: its footer offers them.
        let out = (hints.into_iter())
            .filter(|&(c, ..)| self.can(c, view.preview_fits) && (c != Command::NewWindow || self.tab == Tab::Sessions))
            .map(|(c, label, rank)| self.hint(c, label, rank))
            .collect();
        (out, self.hint(Command::Close, "close", 0))
    }

    /// Do what COMMAND says, whether by key, tap or menu.
    pub fn run(&mut self, command: Command) -> Action {
        match (&mut self.mode, command) {
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
            (Mode::Confirm(_), _) => {
                self.mode = Mode::List;
                return Action::None;
            }
            (Mode::Move(_), Command::Confirm) => return self.drop_here(),
            (Mode::Move(_), Command::Cancel) => {
                self.mode = Mode::List;
                self.keep = None;
                return Action::None;
            }
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
            Command::Open => return self.selected_target().cloned().map_or(Action::None, Action::Activate),
            Command::Panes => self.expand(None),
            Command::Group if self.tab == Tab::Agents => {
                self.group = self.group.next();
                self.rebuild(false);
            }
            Command::Preview => self.preview_on = !self.preview_on,
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
            // The footer's ⋯ opens the menu itself (on_mouse): its view says
            // whether the preview fits.
            Command::Menu | Command::Group | Command::Save | Command::Suggest | Command::Clear | Command::Cancel | Command::Confirm => {}
        }
        Action::None
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        self.status = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match &mut self.mode {
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
                    KeyCode::Char('c') if ctrl => return self.run(Command::Cancel),
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
            Mode::Confirm(c) => {
                // Not Delete: it asked, and a held key repeats.
                let yes = match key.code {
                    KeyCode::Enter => true,
                    KeyCode::Char('x') => ctrl && c.kills(),
                    _ => false,
                };
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
            KeyCode::Esc => return self.run(Command::Close),
            KeyCode::Char('c') if ctrl => return self.run(Command::Close),
            KeyCode::Enter => return self.run(Command::Open),
            KeyCode::Tab | KeyCode::BackTab => return self.run(Command::Tab(self.tab.other())),
            KeyCode::Char('g') if ctrl => return self.run(Command::Group),
            KeyCode::Char('r') if ctrl => return self.run(Command::Rename),
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
            KeyCode::Right => self.expand(Some(true)),
            KeyCode::Left => self.expand(Some(false)),
            // The query is typed at its end (←/→ belong to the tree): text,
            // Backspace (^h), ^w and ^u.
            KeyCode::Backspace => self.edit_query(key),
            KeyCode::Char('h' | 'w' | 'u') if ctrl => self.edit_query(key),
            KeyCode::Char(_) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => self.edit_query(key),
            _ => {}
        }
        Action::None
    }

    /// A mouse event, resolved against what the last draw showed. A press
    /// selects; the release on the same thing acts, so the rest of a click
    /// never reaches whatever is under the pointer once the popup is gone.
    /// Desktop: a double-click opens a row. Compact (a phone, where a
    /// double-tap may arrive as something else): a tap on the selected row
    /// opens it. Right-click (or ctrl-click) opens the row's menu.
    pub fn on_mouse(&mut self, ev: MouseEvent, view: &View, now: Instant) -> Action {
        let at = Position::new(ev.column, ev.row);
        let hit = view.hit(at);
        match ev.kind {
            MouseEventKind::Down(button) => {
                self.status = None;
                let context = button == MouseButton::Right || (button == MouseButton::Left && ev.modifiers.contains(KeyModifiers::CONTROL));
                self.press = None;
                if matches!(self.mode, Mode::Menu(_)) && hit.is_none() {
                    // A click outside a menu only closes it.
                    self.mode = Mode::List;
                    return Action::None;
                }
                if let Mode::Move(c) = &self.mode {
                    if c.dragging {
                        // The drag's release fell outside the popup: it never came.
                        self.mode = Mode::List;
                        self.keep = None;
                        self.note("move cancelled");
                        return Action::None;
                    }
                    if context {
                        self.note("⏎ drop or esc cancel first");
                        return Action::None;
                    }
                }
                if matches!(self.mode, Mode::Confirm(_)) && !matches!(hit, Some(Hit::Button(_))) {
                    // Only the question's buttons answer it; anything else is no.
                    self.mode = Mode::List;
                    return Action::None;
                }
                // A click on the row being named does nothing (the "+ new" row
                // names nothing, so it is no hit); elsewhere, how to finish.
                let editing = match &self.mode {
                    Mode::Rename(r) => Some((Some(r.target.clone()), "⏎ save or esc cancel first")),
                    Mode::Create(_) => Some((None, "⏎ create or esc cancel first")),
                    _ => None,
                };
                if let Some((own, how)) = editing {
                    match &hit {
                        Some(Hit::Button(_)) => {}
                        Some(Hit::Row(_, t)) if Some(t) == own.as_ref() => return Action::None,
                        None if own.is_none() => return Action::None,
                        _ => {
                            self.note(how);
                            return Action::None;
                        }
                    }
                }
                let mut was_selected = false;
                if let Some(Hit::Row(i, t)) = &hit
                    && let Some(i) = self.locate(t, *i)
                {
                    was_selected = i == self.selected;
                    self.selected = i;
                    if context {
                        self.open_menu(Position::new(at.x.saturating_add(1), at.y), view.preview_fits);
                        // Its release (a right-click's other half) opens nothing.
                        self.press = Some(Press { hit: None, at, was_selected });
                        return Action::None;
                    }
                }
                if button == MouseButton::Left {
                    self.press = Some(Press { hit, at, was_selected });
                }
                Action::None
            }
            MouseEventKind::Up(_) if matches!(&self.mode, Mode::Move(c) if c.dragging) => {
                self.press = None;
                // Released on a row: drop it there; elsewhere: never mind.
                match hit {
                    Some(Hit::Row(i, t)) => {
                        if let Some(i) = self.locate(&t, i) {
                            self.selected = i;
                        }
                        self.drop_here()
                    }
                    _ => {
                        self.mode = Mode::List;
                        self.keep = None;
                        self.note("move cancelled");
                        Action::None
                    }
                }
            }
            MouseEventKind::Up(_) => {
                let Some(press) = self.press.take() else { return Action::None };
                // A press-drag-release onto a menu item picks it.
                if press.hit.is_none() {
                    if let Some(Hit::MenuItem(i)) = hit
                        && at != press.at
                    {
                        return self.choose(i);
                    }
                    return Action::None;
                }
                if press.hit != hit {
                    return Action::None;
                }
                match hit {
                    Some(Hit::Button(Command::Menu)) => {
                        self.open_menu(Position::default(), view.preview_fits);
                        Action::None
                    }
                    Some(Hit::Button(command)) => self.run(command),
                    Some(Hit::MenuItem(i)) => self.choose(i),
                    Some(Hit::Toggle(target)) => {
                        if let Some(i) = self.position(&target) {
                            self.selected = i;
                            self.expand(None);
                        }
                        Action::None
                    }
                    Some(Hit::Row(_, target)) => {
                        let double = self.last_click.take().is_some_and(|(t, when)| t == target && now.duration_since(when) <= DOUBLE_CLICK);
                        if (view.compact && press.was_selected) || (!view.compact && double) {
                            return self.run(Command::Open);
                        }
                        self.last_click = Some((target, now));
                        Action::None
                    }
                    None => Action::None,
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = if ev.kind == MouseEventKind::ScrollUp { -1 } else { 1 };
                match &mut self.mode {
                    Mode::Menu(menu) => {
                        let n = menu.items.len() as isize;
                        menu.selected = (menu.selected as isize + delta).rem_euclid(n) as usize;
                    }
                    Mode::List | Mode::Move(_) => self.move_by(delta, false),
                    Mode::Rename(_) | Mode::Confirm(_) | Mode::Create(_) => {}
                }
                Action::None
            }
            MouseEventKind::Drag(MouseButton::Left) => self.drag(hit, at, view),
            _ => Action::None,
        }
    }

    /// The left button moved while held (only reported with drag tracking
    /// on). Leaving the pressed window or pane's row picks it up; then the
    /// selection follows the pointer, and at the list's top or bottom edge
    /// the list scrolls a row per movement (no movement, no scrolling).
    fn drag(&mut self, hit: Option<Hit>, at: Position, view: &View) -> Action {
        if let (Mode::List, Some(press)) = (&self.mode, &self.press) {
            let Some(Hit::Row(_, from)) = &press.hit else { return Action::None };
            let moved = !matches!(&hit, Some(Hit::Row(_, t)) if t == from);
            if !moved || !self.movable() {
                return Action::None;
            }
            self.pick_up(true);
        }
        if !matches!(&self.mode, Mode::Move(c) if c.dragging) {
            return Action::None;
        }
        match hit {
            Some(Hit::Row(i, t)) => {
                if let Some(i) = self.locate(&t, i) {
                    self.selected = i;
                }
            }
            _ => {
                let top = view.rows.first().map(|(r, ..)| r.y);
                let bottom = view.rows.last().map(|(r, ..)| r.bottom());
                if top.is_some_and(|y| at.y < y) {
                    self.move_by(-1, false);
                } else if bottom.is_some_and(|y| at.y >= y) {
                    self.move_by(1, false);
                }
            }
        }
        Action::None
    }

    /// Run the open menu's item I.
    fn choose(&mut self, i: usize) -> Action {
        let Mode::Menu(menu) = &self.mode else { return Action::None };
        match menu.items.get(i) {
            Some(item) => self.run(item.command),
            None => Action::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixture;
    use crate::theme::Theme;
    use crate::ui::{self, Preview};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};
    use ratatui::style::Modifier;
    use ratatui::text::Text;

    /// An app opened from window WINDOW of session $1, with a suggest command.
    fn new(snap: Snapshot, tab: Tab, window: &str) -> App {
        App::new(snap, tab, "$1".into(), window.into(), String::new(), true)
    }

    /// Agents in two folders: header, card, card, header, card, card.
    fn agents_app() -> App {
        let snap = fixture(&[
            "$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|one||||/a",
            "$1|s|@1|1|w|1|0|%2|1|0|claude||working|0|claude|two||||/a",
            "$1|s|@2|2|w|0|0|%3|0|1|claude||working|0|claude|three||||/b",
            "$1|s|@2|2|w|0|0|%4|1|0|claude||working|0|claude|four||||/b",
        ]);
        let mut app = new(snap, Tab::Agents, "@1");
        app.group = Group::Folder;
        app.rebuild(false);
        app
    }

    fn sessions_app(window: &str) -> App {
        let snap = fixture(&[
            "$1|work|@1|1|edit|1|0|%1|0|1|nvim|||||||||/h/a",
            "$1|work|@1|1|edit|1|0|%2|1|0|zsh|||||||||/h/a",
            "$1|work|@2|2|logs|0|0|%3|0|1|tail|||||||||/h/b",
            "$2|play|@3|1|game|1|0|%4|0|1|codex||asking|0|codex|deploy||||/h/c",
        ]);
        new(snap, Tab::Sessions, window)
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent { code, modifiers, kind: KeyEventKind::Press, state: KeyEventState::NONE }
    }

    fn ctrl(app: &mut App, c: char) -> Action {
        app.on_key(key(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn type_(app: &mut App, text: &str) {
        text.chars().for_each(|c| drop(app.on_key(key(KeyCode::Char(c), KeyModifiers::NONE))));
    }

    fn mouse(app: &mut App, view: &View, kind: MouseEventKind, at: (u16, u16), now: Instant) -> Action {
        app.on_mouse(MouseEvent { kind, column: at.0, row: at.1, modifiers: KeyModifiers::NONE }, view, now)
    }

    /// A press and release at AT, at NOW: the release's action.
    fn click_at(app: &mut App, view: &View, button: MouseButton, at: (u16, u16), now: Instant) -> Action {
        let down = mouse(app, view, MouseEventKind::Down(button), at, now);
        assert!(matches!(down, Action::None), "a press does nothing yet: {down:?}");
        mouse(app, view, MouseEventKind::Up(button), at, now)
    }

    fn click(app: &mut App, view: &View, button: MouseButton, at: (u16, u16)) -> Action {
        click_at(app, view, button, at, Instant::now())
    }

    fn draw(app: &App, view: &mut View, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        let body = Text::default();
        term.draw(|f| ui::draw(f, app, view, &Preview { title: "", body: &body }, &Theme::from_values(&[]))).unwrap();
        term
    }

    fn screen(term: &Terminal<TestBackend>) -> Vec<String> {
        let buf = term.backend().buffer();
        (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>().trim_end().to_string()).collect()
    }

    /// Where TEXT starts on screen.
    fn find(term: &Terminal<TestBackend>, text: &str) -> (u16, u16) {
        let lines = screen(term);
        let (y, line) = lines.iter().enumerate().find(|(_, l)| l.contains(text)).unwrap_or_else(|| panic!("{text:?} not in {lines:#?}"));
        let col = line[..line.find(text).unwrap()].chars().count();
        (col as u16, y as u16)
    }

    fn selected_title(app: &App) -> String {
        match &app.rows[app.selected].kind {
            RowKind::Agent { title, .. } => title.clone(),
            RowKind::Window { name, .. } => name.clone(),
            RowKind::Pane { command, .. } => command.clone(),
            RowKind::Session { name, .. } => name.clone(),
            RowKind::Header { .. } => "header".into(),
            RowKind::New => "new".into(),
        }
    }

    fn field(app: &App) -> Option<&str> {
        match &app.mode {
            Mode::Rename(r) => Some(r.field.text()),
            _ => None,
        }
    }

    /// Whether the "+ new" row is listed.
    fn naming(app: &App) -> bool {
        app.rows.iter().any(|r| matches!(r.kind, RowKind::New))
    }

    fn renaming(app: &App) -> Option<&Rename> {
        match &app.mode {
            Mode::Rename(r) => Some(r),
            _ => None,
        }
    }

    #[test]
    fn opens_on_the_current_window_and_selects_the_best_match() {
        let mut app = sessions_app("@2");
        assert_eq!(selected_title(&app), "logs");
        type_(&mut app, "codex");
        assert_eq!(selected_title(&app), "game", "the window whose agent matches");
    }

    #[test]
    fn agents_open_on_the_agent_you_are_in() {
        let snap = || {
            fixture(&[
                "$1|s|@1|1|w|1|0|%1|0|0|claude||working|0|claude|one||||/a",
                "$1|s|@1|1|w|1|0|%2|1|1|claude||asking|0|claude|two||||/a",
                "$1|s|@2|2|w|0|0|%3|0|1|nvim|||||||||/b",
                "$1|s|@2|2|w|0|0|%4|1|0|codex||idle|0|codex|four||||/b",
                "$1|s|@3|3|w|0|0|%5|0|1|claude||working|0|claude|five||||/c",
            ])
        };
        assert_eq!(selected_title(&new(snap(), Tab::Agents, "@1")), "two", "the active pane");
        assert_eq!(selected_title(&new(snap(), Tab::Agents, "@2")), "four", "the window's agent when its active pane is none");
        let mut app = new(snap(), Tab::Sessions, "@1");
        app.selected = app.position(&Target::Window { session: "$1".into(), window: "@3".into() }).unwrap();
        app.run(Command::Tab(Tab::Agents));
        assert_eq!(selected_title(&app), "five", "the window selected in sessions");
    }

    #[test]
    fn moving_skips_headers_and_wraps_only_when_asked() {
        let mut app = agents_app();
        assert_eq!(app.selected, 1);
        app.move_by(1, true);
        assert_eq!(app.selected, 2);
        app.move_by(1, true);
        assert_eq!(app.selected, 4, "skips the header");
        app.move_by(-1, true);
        assert_eq!(app.selected, 2);
        app.move_by(-1, true);
        app.move_by(-1, true);
        assert_eq!(app.selected, 5, "up from the first card wraps to the last");
        app.move_by(1, false);
        assert_eq!(app.selected, 5, "no wrapping when not asked");
        app.move_by(10, false);
        assert_eq!(app.selected, 5, "a page stops at the end");
        app.move_by(-10, false);
        assert_eq!(app.selected, 1, "and at the first card, not the header");
    }

    #[test]
    fn a_vanished_selection_moves_to_the_nearest_row() {
        let mut app = agents_app();
        app.selected = 4; // "three"
        app.set_snapshot(fixture(&[
            "$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|one||||/a",
            "$1|s|@1|1|w|1|0|%2|1|0|claude||working|0|claude|two||||/a",
            "$1|s|@2|2|w|0|0|%4|1|0|claude||working|0|claude|four||||/b",
        ]));
        assert_eq!(selected_title(&app), "four");
    }

    #[test]
    fn a_window_without_panes_opens_and_draws() {
        // Its only pane line was malformed, so parse left it out.
        let snap = fixture(&["$1|s|@1|1|w|1|0|broken", "$1|s|@2|2|v|0|0|%2|0|1|claude||working|0|claude|two||||/a"]);
        let mut app = new(snap, Tab::Sessions, "@1");
        assert_eq!(app.selected_target(), Some(&Target::Window { session: "$1".into(), window: "@1".into() }));
        assert_eq!(app.preview_pane(&View::new()), None);
        app.expand(Some(true));
        draw(&app, &mut View::new(), 80, 12);
        app.run(Command::Tab(Tab::Agents));
        draw(&app, &mut View::new(), 80, 12);
    }

    #[test]
    fn the_selection_follows_an_agent_into_another_group() {
        let line = |state: &str| format!("$1|s|@1|1|w|1|0|%2|1|0|claude||{state}|0|claude|two||||/a");
        let snap = |state: &str| fixture(&["$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|one||||/a", &line(state)]);
        let mut app = new(snap("working"), Tab::Agents, "@1");
        app.move_by(1, false);
        assert_eq!(selected_title(&app), "two");
        app.set_snapshot(snap("asking"));
        assert_eq!(selected_title(&app), "two");
        assert!(matches!(&app.rows[app.selected - 1].kind, RowKind::Header { label, .. } if label == "waiting"));
    }

    #[test]
    fn switching_tabs_keeps_the_selection_or_falls_back_to_the_current_window() {
        let mut app = agents_app();
        app.run(Command::Tab(Tab::Sessions));
        assert_eq!(app.selected_target(), Some(&Target::Window { session: "$1".into(), window: "@1".into() }), "panes are not listed");
        app.move_by(2, false);
        app.expand(Some(true));
        app.move_by(2, false);
        let pane = Target::Pane { session: "$1".into(), window: "@2".into(), pane: "%4".into() };
        assert_eq!(app.selected_target(), Some(&pane));
        app.run(Command::Tab(Tab::Agents));
        assert_eq!(selected_title(&app), "four");
        app.run(Command::Tab(Tab::Sessions));
        assert_eq!(app.selected_target(), Some(&pane), "back on the listed pane");
    }

    #[test]
    fn a_window_linked_twice_keeps_its_selected_row() {
        let mut app = new(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|zsh|||||||||/a", "$1|s|@1|2|w|0|0|%1|0|1|zsh|||||||||/a"]), Tab::Sessions, "@1");
        assert_eq!(app.selected, 1);
        app.selected = 2;
        app.refresh();
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn a_negation_keeps_the_selection() {
        let mut app = agents_app();
        app.selected = 4;
        app.paste("!zzz");
        assert_eq!(selected_title(&app), "three", "every row ties");
    }

    #[test]
    fn scrolling_keeps_the_group_header_and_fills_the_viewport() {
        let app = agents_app();
        let mut view = View::new();
        // Room for two cards (3 lines each) and a header.
        view.scroll(&app.rows, 5, 7);
        assert_eq!(view.offset, 3, "header of the second group, then its cards");
        view.offset = 4;
        view.scroll(&app.rows, 4, 7);
        assert_eq!(view.offset, 3, "scrolling up to a group's first card shows its header");
        // Rows shrink below the offset: pull back instead of showing blanks.
        view.offset = 5;
        view.scroll(&app.rows, 5, 100);
        assert_eq!(view.offset, 0);
    }

    #[test]
    fn a_double_click_opens_and_a_single_one_selects() {
        let mut app = agents_app();
        let mut view = View::new();
        let term = draw(&app, &mut view, 120, 20);
        let t0 = Instant::now();
        let at = find(&term, "two");
        assert!(matches!(click_at(&mut app, &view, MouseButton::Left, at, t0), Action::None));
        assert_eq!(selected_title(&app), "two");
        // Already selected, a slow second click still doesn't open.
        assert!(matches!(click_at(&mut app, &view, MouseButton::Left, at, t0 + Duration::from_secs(2)), Action::None));
        let open = click_at(&mut app, &view, MouseButton::Left, at, t0 + Duration::from_millis(2300));
        assert!(matches!(&open, Action::Activate(Target::Pane { pane, .. }) if pane == "%2"), "{open:?}");
    }

    #[test]
    fn opening_waits_for_the_release_on_the_same_row() {
        let mut app = agents_app();
        let mut view = View::new();
        let term = draw(&app, &mut view, 120, 20);
        let (two, three) = (find(&term, "two"), find(&term, "three"));
        let t0 = Instant::now();
        click_at(&mut app, &view, MouseButton::Left, two, t0);
        assert!(matches!(mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Left), two, t0), Action::None), "the press never opens");
        assert!(matches!(mouse(&mut app, &view, MouseEventKind::Up(MouseButton::Left), three, t0), Action::None), "released elsewhere");
        assert!(matches!(mouse(&mut app, &view, MouseEventKind::Up(MouseButton::Left), two, t0), Action::None), "a release without a press");
    }

    #[test]
    fn on_a_phone_a_tap_on_the_selected_row_opens() {
        let mut app = agents_app();
        let mut view = View::new();
        let term = draw(&app, &mut view, 46, 20);
        assert!(view.compact);
        let at = find(&term, "two");
        let t0 = Instant::now();
        assert!(matches!(click_at(&mut app, &view, MouseButton::Left, at, t0), Action::None));
        let open = click_at(&mut app, &view, MouseButton::Left, at, t0 + Duration::from_secs(3));
        assert!(matches!(open, Action::Activate(_)));
    }

    #[test]
    fn clicks_hit_list_rows_only() {
        let mut app = agents_app();
        let mut view = View::new();
        // 8 list lines: header, card, card, blank and header, then a card cut off after one line.
        let term = draw(&app, &mut view, 120, 13);
        assert!(view.preview_fits);
        let (x, y) = find(&term, "three");
        let before = app.selected;
        click(&mut app, &view, MouseButton::Left, (x, y - 1));
        assert_eq!(app.selected, before, "headers are not selectable");
        click(&mut app, &view, MouseButton::Right, (x, y - 1));
        assert_eq!(app.selected, before, "not even by right-click");
        assert!(matches!(app.mode, Mode::List), "and no menu");
        click(&mut app, &view, MouseButton::Left, (x + 70, y));
        assert_eq!(app.selected, before, "the preview is not the list");
        click(&mut app, &view, MouseButton::Left, (x, y + 1));
        assert_eq!(app.selected, before, "the cut-off card ends with the list");
        click(&mut app, &view, MouseButton::Left, (x, y));
        assert_eq!(selected_title(&app), "three");
    }

    #[test]
    fn a_click_resolves_to_what_was_drawn() {
        let mut app = agents_app();
        let mut view = View::new();
        let term = draw(&app, &mut view, 120, 30);
        let at = find(&term, "four");
        // A key that empties the list, then a click on where a row was.
        type_(&mut app, "z");
        assert!(app.rows.is_empty());
        assert!(matches!(click(&mut app, &view, MouseButton::Left, at), Action::None));
    }

    #[test]
    fn a_click_on_a_window_linked_twice_picks_the_copy_clicked() {
        let mut app = new(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|zsh|||||||||/a", "$1|s|@1|2|w|0|0|%1|0|1|zsh|||||||||/a"]), Tab::Sessions, "@1");
        let mut view = View::new();
        let term = draw(&app, &mut view, 120, 20);
        let (_, y) = find(&term, "2   w");
        click(&mut app, &view, MouseButton::Left, (6, y));
        assert_eq!(app.selected, 2, "{:#?}", screen(&term));
    }

    #[test]
    fn the_triangle_toggles_a_windows_panes() {
        let mut app = sessions_app("@1");
        let mut view = View::new();
        let term = draw(&app, &mut view, 120, 20);
        assert_eq!(app.rows.len(), 5);
        let open = click(&mut app, &view, MouseButton::Left, find(&term, "▸"));
        assert!(matches!(open, Action::None), "the triangle doesn't open the window");
        assert_eq!(app.rows.len(), 7, "it lists the panes");
        let term = draw(&app, &mut view, 120, 20);
        click(&mut app, &view, MouseButton::Left, find(&term, "▾"));
        assert_eq!(app.rows.len(), 5);
    }

    #[test]
    fn title_and_footer_are_buttons() {
        let mut app = sessions_app("@1");
        let mut view = View::new();
        let term = draw(&app, &mut view, 46, 20);
        click(&mut app, &view, MouseButton::Left, find(&term, "agents"));
        assert_eq!(app.tab, Tab::Agents);
        let term = draw(&app, &mut view, 46, 20);
        let lines = screen(&term);
        assert!(lines.iter().any(|l| l.contains("open   sessions   rename   ⋯") && l.ends_with("close │")), "{lines:#?}");
        click(&mut app, &view, MouseButton::Left, find(&term, "rename"));
        assert!(renaming(&app).is_some());
        app.run(Command::Cancel);
        let term = draw(&app, &mut view, 46, 20);
        assert!(matches!(click(&mut app, &view, MouseButton::Left, find(&term, "close")), Action::Quit));
    }

    #[test]
    fn the_menu_has_every_hint_with_its_key() {
        let mut app = sessions_app("@1");
        app.expanded.insert(window("$1", "@1"));
        let view = View::new();
        for tab in [Tab::Sessions, Tab::Agents] {
            app.run(Command::Tab(tab));
            let rows: Vec<usize> = (0..app.rows.len()).filter(|&i| app.selectable(i)).collect();
            for i in rows {
                app.selected = i;
                let menu = app.menu_items(view.preview_fits);
                for h in app.hints(&view).0 {
                    assert!(menu.iter().any(|m| m.command == h.command && m.key == h.key), "{h:?} on row {i} of {tab:?}: {menu:?}");
                }
            }
        }
    }

    #[test]
    fn the_footer_keeps_close_and_folds_the_rest_into_a_menu() {
        let mut app = agents_app();
        let mut view = View::new();
        let lines = screen(&draw(&app, &mut view, 38, 20));
        let footer = &lines[lines.len() - 2];
        assert!(footer.contains("open") && footer.contains("rename") && footer.contains("⋯") && footer.ends_with("close │"), "{lines:#?}");
        assert!(!footer.contains("sessions"), "the tab switch folds first: {lines:#?}");
        let term = draw(&app, &mut view, 38, 20);
        click(&mut app, &view, MouseButton::Left, find(&term, "⋯"));
        let Mode::Menu(menu) = &app.mode else { panic!("no menu: {:?}", app.mode) };
        assert!(menu.items.iter().any(|i| i.command == Command::Group), "{:?}", menu.items);
        // At 80-101 columns the preview can't show: no hint for it.
        app.run(Command::Cancel);
        let lines = screen(&draw(&app, &mut view, 90, 20));
        assert!(!view.preview_fits);
        assert!(!lines.iter().any(|l| l.contains("preview")), "{lines:#?}");
    }

    #[test]
    fn compact_rows_take_two_lines_with_less_detail() {
        let mut app = sessions_app("@1");
        let mut view = View::new();
        let lines = screen(&draw(&app, &mut view, 46, 20));
        assert!(view.compact);
        let at = lines.iter().position(|l| l.contains("edit")).unwrap();
        assert!(lines[at + 1].contains("/h/a"), "the folder under the window: {lines:#?}");
        assert!(!lines.iter().any(|l| l.contains("windows")), "no window counts");
        app.run(Command::Tab(Tab::Agents));
        let lines = screen(&draw(&app, &mut view, 46, 20));
        let at = lines.iter().position(|l| l.contains("deploy")).unwrap();
        assert!(lines[at + 1].contains("play"), "{lines:#?}");
        assert!(!lines[at + 1].contains("codex"), "no kind on a phone");
    }

    #[test]
    fn renaming_an_agent() {
        let mut app = agents_app();
        ctrl(&mut app, 'r');
        assert_eq!(field(&app), Some("one"), "starts from the current title");
        // An untouched automatic title is not set as the user's.
        assert!(matches!(app.on_key(key(KeyCode::Enter, KeyModifiers::NONE)), Action::None));
        assert!(field(&app).is_none());
        ctrl(&mut app, 'r');
        type_(&mut app, " more");
        let save = app.on_key(key(KeyCode::Enter, KeyModifiers::NONE));
        let Action::Run(job) = save else { panic!("{save:?}") };
        assert_eq!(job.op, Op::SetTitle { pane: "%1".into(), title: Some("one more".into()) });
        assert_eq!(job.undo, Some(Op::SetTitle { pane: "%1".into(), title: None }), "undo goes back to automatic");
        assert_eq!(app.query.text(), "", "typing went to the title, not the filter");
        app.ran(job, Ok(None));
        assert_eq!(app.status, Some(Status::Done("renamed   ^z undo".into())));
        ctrl(&mut app, 'r');
        assert!(matches!(app.on_key(key(KeyCode::Esc, KeyModifiers::NONE)), Action::None));
        assert!(field(&app).is_none(), "esc cancels the rename, not the popup");
    }

    #[test]
    fn clearing_a_set_title_goes_back_to_automatic_and_undo_restores_it() {
        let snap = fixture(&["$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|auto|mine|||/a"]);
        let mut app = new(snap, Tab::Agents, "@1");
        ctrl(&mut app, 'r');
        assert_eq!(renaming(&app).map(|r| r.placeholder.as_str()), Some("auto"));
        ctrl(&mut app, 'u');
        let Action::Run(job) = app.on_key(key(KeyCode::Enter, KeyModifiers::NONE)) else { panic!() };
        assert_eq!(job.op, Op::SetTitle { pane: "%1".into(), title: None });
        app.ran(job, Ok(None));
        let Action::Run(undo) = ctrl(&mut app, 'z') else { panic!() };
        assert_eq!(undo.op, Op::SetTitle { pane: "%1".into(), title: Some("mine".into()) });
        app.ran(undo, Ok(None));
        assert_eq!(app.status, Some(Status::Done("undone: back to the automatic title".into())));
        assert!(matches!(ctrl(&mut app, 'z'), Action::None));
        assert_eq!(app.status, Some(Status::Note("nothing to undo".into())));
        // A failed change says why and can't be undone.
        ctrl(&mut app, 'r');
        type_(&mut app, "!");
        let Action::Run(job) = app.on_key(key(KeyCode::Enter, KeyModifiers::NONE)) else { panic!() };
        app.ran(job, Err("can't find pane: %1".into()));
        assert_eq!(app.status, Some(Status::Failed("can't find pane: %1".into())));
        assert!(app.undo.is_empty());
    }

    #[test]
    fn an_unchanged_title_is_not_saved_over_a_newer_one() {
        let snap = fixture(&["$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|auto|mine|||/a"]);
        let mut app = new(snap, Tab::Agents, "@1");
        ctrl(&mut app, 'r');
        assert!(matches!(app.on_key(key(KeyCode::Enter, KeyModifiers::NONE)), Action::None));
    }

    #[test]
    fn a_long_or_padded_name_left_as_it_was_is_not_saved() {
        let long = "a".repeat(TITLE_MAX + 20);
        let snap = fixture(&[&format!("$1| sp |@1|1|{long}|1|0|%1|0|1|claude||working|0|claude||{long}|||/a")]);
        let mut app = new(snap, Tab::Sessions, "@1");
        for target in [Target::Session { session: "$1".into() }, window("$1", "@1"), pane("$1", "@1", "%1")] {
            if matches!(target, Target::Pane { .. }) {
                app.set_tab(Tab::Agents);
            }
            select(&mut app, target.clone());
            ctrl(&mut app, 'r');
            assert!(renaming(&app).is_some(), "{target:?}");
            assert!(matches!(enter(&mut app), Action::None), "{target:?}: cut to {TITLE_MAX} or trimmed, not changed");
        }
    }

    #[test]
    fn a_renamed_card_stays_listed_under_the_filter() {
        let mut app = agents_app();
        type_(&mut app, "three");
        assert_eq!(app.rows.len(), 2, "header and card");
        ctrl(&mut app, 'r');
        ctrl(&mut app, 'u');
        type_(&mut app, "renamed");
        let Action::Run(job) = app.on_key(key(KeyCode::Enter, KeyModifiers::NONE)) else { panic!() };
        app.ran(job, Ok(None));
        app.set_snapshot(fixture(&[
            "$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|one||||/a",
            "$1|s|@2|2|w|0|0|%3|0|1|claude||working|0|claude|three|renamed|||/b",
        ]));
        assert_eq!(selected_title(&app), "renamed", "it doesn't match \"three\" any more, but stays");
        type_(&mut app, "x");
        assert!(app.rows.is_empty(), "until the query changes");
    }

    #[test]
    fn ctrl_r_again_fills_the_field_with_a_suggestion() {
        let mut app = agents_app();
        ctrl(&mut app, 'r');
        assert!(matches!(ctrl(&mut app, 'r'), Action::Suggest { pane, id: 1 } if pane == "%1"));
        assert!(matches!(ctrl(&mut app, 'r'), Action::None), "one suggestion at a time");
        assert_eq!(app.suggesting(), Some(1));
        let lines = screen(&draw(&app, &mut View::new(), 120, 12));
        assert!(lines.iter().any(|l| l.contains("✎ one") && l.contains("✦ suggesting…")), "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("⏎ save   ^r suggest   ^u clear") && l.ends_with("esc cancel │")), "{lines:#?}");
        app.suggested(1, Ok("Fix the cart page".into()));
        assert_eq!(field(&app), Some("Fix the cart page"));
        assert_eq!(app.suggesting(), None);
        // It is the field's text, to keep, change or drop.
        type_(&mut app, "s");
        let Action::Run(job) = app.on_key(key(KeyCode::Enter, KeyModifiers::NONE)) else { panic!() };
        assert_eq!(job.op, Op::SetTitle { pane: "%1".into(), title: Some("Fix the cart pages".into()) });
    }

    #[test]
    fn a_suggestion_does_not_overwrite_what_came_after_it() {
        let mut app = agents_app();
        // Typing while it generates keeps the typing, and stops waiting.
        ctrl(&mut app, 'r');
        ctrl(&mut app, 'r');
        type_(&mut app, "!");
        assert_eq!(app.suggesting(), None, "the loop stops the suggest command");
        app.suggested(1, Ok("late".into()));
        assert_eq!(field(&app), Some("one!"));
        // Keys that change nothing keep waiting.
        assert!(matches!(ctrl(&mut app, 'r'), Action::Suggest { id: 2, .. }));
        app.on_key(key(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(app.suggesting(), Some(2), "Delete at the end changes nothing");
        // An answer for a rename that was cancelled, reaching a new one.
        app.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.suggesting(), None);
        ctrl(&mut app, 'r');
        app.suggested(2, Ok("stale".into()));
        assert_eq!(field(&app), Some("one"));
        // A failure says why, by the field, and offers a retry.
        assert!(matches!(ctrl(&mut app, 'r'), Action::Suggest { id: 3, .. }));
        app.suggested(3, Err("pi timed out after 30s".into()));
        assert_eq!(field(&app), Some("one"));
        assert_eq!(renaming(&app).and_then(|r| r.error.as_deref()), Some("pi timed out after 30s"));
        let lines = screen(&draw(&app, &mut View::new(), 120, 12));
        assert!(lines.iter().any(|l| l.contains("✗ pi timed out after 30s")), "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("^r retry")), "{lines:#?}");
    }

    #[test]
    fn no_suggestions_without_a_suggest_command() {
        let mut app = agents_app();
        app.can_suggest = false;
        ctrl(&mut app, 'r');
        let lines = screen(&draw(&app, &mut View::new(), 120, 12));
        assert!(!lines.iter().any(|l| l.contains("suggest")), "not offered: {lines:#?}");
        assert!(matches!(ctrl(&mut app, 'r'), Action::None));
        assert!(matches!(&app.status, Some(Status::Note(n)) if n.contains("@agent-suggest-command")), "but says why when asked");
    }

    #[test]
    fn renaming_ends_when_the_agent_exits() {
        let mut app = agents_app();
        ctrl(&mut app, 'r');
        app.set_snapshot(fixture(&["$1|s|@1|1|w|1|0|%2|1|0|claude||working|0|claude|two||||/a"]));
        assert!(field(&app).is_none());
        assert_eq!(app.status, Some(Status::Failed("that agent has exited".into())));
    }

    #[test]
    fn the_title_is_edited_on_its_card() {
        let mut app = agents_app();
        type_(&mut app, "on");
        ctrl(&mut app, 'r');
        let lines = screen(&draw(&app, &mut View::new(), 120, 14));
        assert!(lines.iter().any(|l| l.contains("› on") && l.contains("renaming")), "the filter stays in view: {lines:#?}");
        assert!(lines.iter().any(|l| l.contains("▌ ✎ one")), "{lines:#?}");
        ctrl(&mut app, 'u');
        let lines = screen(&draw(&app, &mut View::new(), 120, 14));
        assert!(lines.iter().any(|l| l.contains("✎ ▏one") && l.contains("automatic")), "the automatic title as a placeholder: {lines:#?}");
    }

    #[test]
    fn the_mouse_renames_too() {
        let mut app = agents_app();
        let mut view = View::new();
        let term = draw(&app, &mut view, 120, 30);
        click(&mut app, &view, MouseButton::Right, find(&term, "three"));
        let Mode::Menu(_) = &app.mode else { panic!("right-click opens the card's menu") };
        let term = draw(&app, &mut view, 120, 30);
        let action = click(&mut app, &view, MouseButton::Left, find(&term, "suggest a title"));
        assert!(matches!(&action, Action::Suggest { pane, .. } if pane == "%3"), "{action:?}");
        assert!(matches!(renaming(&app).map(|r| &r.target), Some(Target::Pane { pane, .. }) if pane == "%3"));
        // Clicking another card while renaming says how to finish.
        let term = draw(&app, &mut view, 120, 30);
        click(&mut app, &view, MouseButton::Left, find(&term, "one"));
        assert!(renaming(&app).is_some());
        assert_eq!(app.status, Some(Status::Note("⏎ save or esc cancel first".into())));
        let term = draw(&app, &mut view, 120, 30);
        click(&mut app, &view, MouseButton::Left, find(&term, "cancel"));
        assert!(renaming(&app).is_none());
    }

    #[test]
    fn a_menu_is_picked_by_key_by_click_or_by_dragging_onto_an_item() {
        let mut app = agents_app();
        let mut view = View::new();
        let term = draw(&app, &mut view, 120, 30);
        let at = find(&term, "two");
        // Right press, then its release where it was pressed: nothing.
        mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Right), at, Instant::now());
        let term = draw(&app, &mut view, 120, 30);
        assert!(matches!(mouse(&mut app, &view, MouseEventKind::Up(MouseButton::Right), at, Instant::now()), Action::None), "its own release picks nothing");
        assert!(matches!(app.mode, Mode::Menu(_)));
        // Right press, drag, release on "rename".
        let item = find(&term, "rename");
        app.mode = Mode::List;
        draw(&app, &mut view, 120, 30);
        mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Right), at, Instant::now());
        draw(&app, &mut view, 120, 30);
        mouse(&mut app, &view, MouseEventKind::Up(MouseButton::Right), item, Instant::now());
        assert!(matches!(renaming(&app).map(|r| &r.target), Some(Target::Pane { pane, .. }) if pane == "%2"));
        app.run(Command::Cancel);
        // Keys: ↓ ↓ then ⏎; any other key closes it and acts.
        app.open_menu(Position::default(), true);
        app.on_key(key(KeyCode::Down, KeyModifiers::NONE));
        app.on_key(key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(renaming(&app).is_some(), "the second item, rename");
        app.run(Command::Cancel);
        app.open_menu(Position::default(), true);
        type_(&mut app, "t");
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.query.text(), "t");
        // A click outside closes it.
        app.open_menu(Position::default(), true);
        draw(&app, &mut view, 120, 30);
        mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Left), (110, 25), Instant::now());
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn a_menu_closes_when_its_row_goes() {
        let mut app = sessions_app("@1");
        select(&mut app, window("$1", "@2"));
        app.open_menu(Position::default(), true);
        let without_logs = || fixture(&["$1|work|@1|1|edit|1|0|%1|0|1|nvim|||||||||/h/a", "$2|play|@3|1|game|1|0|%4|0|1|codex||asking|0|codex|deploy||||/h/c"]);
        app.set_snapshot(without_logs());
        assert!(matches!(app.mode, Mode::List), "its move down would swap another window");
        assert!(matches!(&app.status, Some(Status::Failed(_))));
        // A refresh that keeps the row keeps the menu.
        app.open_menu(Position::default(), true);
        app.set_snapshot(without_logs());
        assert!(matches!(app.mode, Mode::Menu(_)));
    }

    #[test]
    fn nerd_marks_follow_redraws_rename_filter_menu_and_resize() {
        let mut app = agents_app();
        let mut view = View::new();
        let mut theme = Theme::from_values(&[]);
        theme.agent_icons = crate::theme::AgentIcons::Nerd;
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let render = |term: &mut Terminal<TestBackend>, app: &App, view: &mut View| {
            term.draw(|f| ui::draw(f, app, view, &Preview { title: "", body: &Text::default() }, &theme)).unwrap();
        };
        let icon = "\u{ec82}";
        let count = |term: &Terminal<TestBackend>| term.backend().buffer().content.iter().filter(|c| c.symbol() == icon).count();
        render(&mut term, &app, &mut view);
        assert_eq!(count(&term), 4, "one glyph per card");
        let (x, y) = find(&term, icon);
        assert_eq!(term.backend().buffer()[(x, y)].fg, ratatui::style::Color::Reset);
        assert!(screen(&term).iter().all(|l| !l.contains("claude")), "no duplicate provider name");

        app.open_menu(Position::new(x, y), true);
        render(&mut term, &app, &mut view);
        let menu = view.menu.unwrap();
        for row in menu.y..menu.bottom() {
            for col in menu.x..menu.right() {
                assert_ne!(term.backend().buffer()[(col, row)].symbol(), icon, "menu clears glyph cells");
            }
        }
        app.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
        ctrl(&mut app, 'r');
        render(&mut term, &app, &mut view);
        assert_eq!(count(&term), 4, "renaming keeps the icon");
        app.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
        type_(&mut app, "nothing-matches");
        render(&mut term, &app, &mut view);
        assert_eq!(count(&term), 0, "filtering removes old glyphs");
        ctrl(&mut app, 'u');
        for width in [24, 40, 120] {
            term.backend_mut().resize(width, 12);
            term.resize(ratatui::layout::Rect::new(0, 0, width, 12)).unwrap();
            render(&mut term, &app, &mut view);
            assert!(count(&term) > 0);
            for y in 0..12 {
                assert_ne!(term.backend().buffer()[(width - 1, y)].symbol(), icon, "no glyph outside the list");
            }
            app.move_by(1, false);
            render(&mut term, &app, &mut view);
        }
        app.set_tab(Tab::Sessions);
        render(&mut term, &app, &mut view);
        assert_eq!(count(&term), 0, "changing tabs clears the glyphs");
    }

    #[test]
    fn agent_marks_survive_renaming_and_fall_back_to_the_command() {
        for (kind, command, icon) in [("claude", "node", "✳"), ("codex", "node", "›_"), ("", "pi", "π"), ("custom", "node", "▣")] {
            let mut app = new(fixture(&[&format!("$1|s|@1|1|w|1|0|%1|0|1|{command}||working|0|{kind}|task||||/a")]), Tab::Agents, "@1");
            for renaming in [false, true] {
                if renaming {
                    ctrl(&mut app, 'r');
                }
                for width in [40, 80, 120] {
                    let lines = screen(&draw(&app, &mut View::new(), width, 12));
                    let title = lines.iter().position(|l| l.contains("task")).unwrap();
                    assert!(!lines[title].contains(icon), "{lines:#?}");
                    assert!(lines[title + 1].contains(icon), "{lines:#?}");
                }
            }
        }
    }

    #[test]
    fn a_stale_state_is_marked_unconfirmed() {
        let app = new(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|task|||1|/a"]), Tab::Agents, "@1");
        let lines = screen(&draw(&app, &mut View::new(), 120, 12));
        assert!(lines.iter().any(|l| l.contains("working?")), "{lines:#?}");
    }

    #[test]
    fn pasted_newlines_do_not_open() {
        let mut app = agents_app();
        app.paste("tw\no\n");
        assert_eq!(app.query.text(), "two");
        assert_eq!(selected_title(&app), "two");
    }

    #[test]
    fn the_title_is_plain_text_over_a_dim_border() {
        let app = agents_app();
        let term = draw(&app, &mut View::new(), 120, 20);
        let buf = term.backend().buffer();
        let (x, y) = find(&term, "agents");
        assert!(!buf[(x, y)].modifier.contains(Modifier::DIM), "\"agents\" tab");
        assert!(buf[(0, 0)].modifier.contains(Modifier::DIM), "the border itself");
    }

    #[test]
    fn a_narrow_title_drops_the_calmest_counts_whole() {
        let rows: Vec<String> = (1..=12)
            .map(|i| {
                let (state, unseen) = [("asking", "0"), ("working", "0"), ("idle", "1"), ("idle", "0")][i % 4];
                format!("$1|s|@{i}|{i}|w|1|0|%{i}|0|1|claude||{state}|{unseen}|claude|t{i}||||/a")
            })
            .collect();
        let app = new(fixture(&rows.iter().map(|r| r.as_str()).collect::<Vec<_>>()), Tab::Agents, "@1");
        let top = |w| screen(&draw(&app, &mut View::new(), w, 12))[0].clone();
        assert_eq!(top(80).matches(['●', '○']).count(), 4, "{}", top(80));
        for w in 30..44 {
            let line = top(w);
            assert!(line.contains("sessions  agents"), "{line}");
            // Each count follows its dot.
            let words: Vec<&str> = line.trim_matches(|c| "╭╮─ ".contains(c)).split_whitespace().collect();
            for (i, word) in words.iter().enumerate() {
                if word.chars().all(|c| c.is_ascii_digit()) {
                    assert!(matches!(words.get(i.wrapping_sub(1)), Some(&"●" | &"○")), "width {w}: {line}");
                }
            }
        }
        assert!(top(38).contains("● 3"), "the most urgent stay: {}", top(38));
    }

    #[test]
    fn tabs_round_trip_through_their_labels() {
        for tab in [Tab::Sessions, Tab::Agents] {
            assert_eq!(Tab::parse(tab.label()), Some(tab));
        }
        assert_eq!(Tab::parse("Agents"), None);
        assert_eq!(Tab::parse(""), None);
    }

    #[test]
    fn status_shows_on_the_bottom_edge_and_keeps_the_footer() {
        let mut app = agents_app();
        app.status = Some(Status::Failed("tmux said no".into()));
        let mut view = View::new();
        let term = draw(&app, &mut view, 120, 20);
        let lines = screen(&term);
        assert!(lines.last().unwrap().contains("✗ tmux said no"), "{lines:#?}");
        assert!(matches!(click(&mut app, &view, MouseButton::Left, find(&term, "close")), Action::Quit), "a tap still works");
    }

    #[test]
    fn alt_letters_are_not_typed_and_ctrl_h_deletes() {
        let mut app = agents_app();
        app.on_key(key(KeyCode::Char('x'), KeyModifiers::ALT));
        assert_eq!(app.query.text(), "");
        app.on_key(key(KeyCode::Char('T'), KeyModifiers::SHIFT));
        assert_eq!(app.query.text(), "T");
        app.on_key(key(KeyCode::Home, KeyModifiers::NONE));
        type_(&mut app, "x");
        assert_eq!(app.query.text(), "Tx", "the query is typed at its end");
        ctrl(&mut app, 'h');
        ctrl(&mut app, 'h');
        assert_eq!(app.query.text(), "", "^h is Backspace");
    }

    #[test]
    fn left_collapses_and_right_expands() {
        let snap = fixture(&["$1|s|@1|1|w|1|0|%1|0|1|nvim|||||||||/a", "$1|s|@1|1|w|1|0|%2|1|0|zsh|||||||||/a"]);
        let mut app = new(snap, Tab::Sessions, "@1");
        assert_eq!(app.rows.len(), 2);
        app.on_key(key(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.rows.len(), 2, "left does not expand");
        app.on_key(key(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.rows.len(), 4);
        app.on_key(key(KeyCode::Down, KeyModifiers::NONE));
        assert!(matches!(app.rows[app.selected].kind, RowKind::Pane { .. }));
        app.on_key(key(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.rows.len(), 2);
        assert!(matches!(app.rows[app.selected].kind, RowKind::Window { .. }), "back on the window");
    }

    #[test]
    fn a_window_the_filter_opened_can_be_closed() {
        let mut app = sessions_app("@1");
        type_(&mut app, "zsh");
        let panes = |app: &App| app.rows.iter().filter(|r| matches!(r.kind, RowKind::Pane { .. })).count();
        assert_eq!(panes(&app), 1, "the matching pane explains the window");
        app.selected = app.rows.iter().position(|r| matches!(r.kind, RowKind::Window { name: ref n, .. } if n == "edit")).unwrap();
        app.on_key(key(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(panes(&app), 0, "closed");
        app.on_key(key(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(panes(&app), 2, "open: every pane");
    }

    fn select(app: &mut App, target: Target) {
        app.selected = app.position(&target).unwrap_or_else(|| panic!("{target:?} not listed"));
    }

    fn window(session: &str, window: &str) -> Target {
        Target::Window { session: session.into(), window: window.into() }
    }

    fn pane(session: &str, window: &str, pane: &str) -> Target {
        Target::Pane { session: session.into(), window: window.into(), pane: pane.into() }
    }

    fn job(action: Action) -> Job {
        match action {
            Action::Run(job) => job,
            other => panic!("not a job: {other:?}"),
        }
    }

    fn enter(app: &mut App) -> Action {
        app.on_key(key(KeyCode::Enter, KeyModifiers::NONE))
    }

    #[test]
    fn sessions_and_windows_are_renamed_in_place() {
        let mut app = sessions_app("@1");
        select(&mut app, Target::Session { session: "$1".into() });
        ctrl(&mut app, 'r');
        assert_eq!(field(&app), Some("work"));
        // tmux refuses these; the field stays open and says why.
        ctrl(&mut app, 'u');
        type_(&mut app, "play");
        assert!(matches!(enter(&mut app), Action::None));
        assert_eq!(renaming(&app).and_then(|r| r.error.as_deref()), Some("a session called play exists"));
        ctrl(&mut app, 'u');
        type_(&mut app, "a.b");
        enter(&mut app);
        assert_eq!(renaming(&app).and_then(|r| r.error.as_deref()), Some("session names can't have . or :"));
        ctrl(&mut app, 'u');
        type_(&mut app, "office");
        assert_eq!(renaming(&app).and_then(|r| r.error.as_deref()), None, "typing clears the reason");
        let j = job(enter(&mut app));
        assert_eq!(j.op, Op::RenameSession { session: "$1".into(), name: "office".into() });
        assert_eq!(j.undo, Some(Op::RenameSession { session: "$1".into(), name: "work".into() }));
        // A window's empty name hands naming back to tmux.
        select(&mut app, window("$1", "@2"));
        ctrl(&mut app, 'r');
        ctrl(&mut app, 'u');
        let j = job(enter(&mut app));
        assert_eq!(j.op, Op::RenameWindow { window: "@2".into(), name: String::new() });
        assert_eq!(j.done, "named after what runs");
    }

    #[test]
    fn renaming_a_window_tmux_names_is_undone_back_to_automatic() {
        let mut app = new(fixture(&["$1|s|@1|1|zsh|1|1|%1|0|1|zsh|||||||||/a"]), Tab::Sessions, "@1");
        select(&mut app, window("$1", "@1"));
        ctrl(&mut app, 'r');
        type_(&mut app, "x");
        let j = job(enter(&mut app));
        assert_eq!(j.op, Op::RenameWindow { window: "@1".into(), name: "zshx".into() });
        assert_eq!(j.undo, Some(Op::RenameWindow { window: "@1".into(), name: String::new() }), "not frozen at zsh");
        // Its name is already automatic: clearing it changes nothing.
        ctrl(&mut app, 'r');
        ctrl(&mut app, 'u');
        assert!(matches!(enter(&mut app), Action::None));
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn the_current_session_follows_the_client() {
        let mut app = sessions_app("@1");
        let current = |app: &App| -> Vec<String> {
            app.rows.iter().filter_map(|r| if let RowKind::Session { name, current: true, .. } = &r.kind { Some(name.clone()) } else { None }).collect()
        };
        assert_eq!(current(&app), ["work"]);
        app.moved("$2");
        app.refresh();
        assert_eq!(current(&app), ["play"]);
        assert_eq!(app.current_window, "@3", "at its active window");
    }

    #[test]
    fn kills_ask_first_and_say_what_runs_there() {
        let mut app = sessions_app("@1");
        select(&mut app, window("$2", "@3"));
        ctrl(&mut app, 'x');
        let Mode::Confirm(c) = &app.mode else { panic!("{:?}", app.mode) };
        assert_eq!(c.question, "kill window play:1 game? 1 pane · codex waiting");
        let lines = screen(&draw(&app, &mut View::new(), 120, 12));
        assert!(lines.iter().any(|l| l.contains("✗ kill window play:1 game?") && l.contains("can't be undone")), "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("⏎ kill") && l.ends_with("esc cancel │")), "{lines:#?}");
        // Anything but yes is no.
        type_(&mut app, "y");
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.query.text(), "", "the no doesn't type");
        app.on_key(key(KeyCode::Delete, KeyModifiers::NONE));
        assert!(matches!(app.on_key(key(KeyCode::Delete, KeyModifiers::NONE)), Action::None), "a held Delete doesn't answer");
        app.on_key(key(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(job(ctrl(&mut app, 'x')).op, Op::Kill(window("$2", "@3")), "^x twice kills");
        let mut app = new(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|zsh|||||||||/a"]), Tab::Sessions, "@1");
        select(&mut app, Target::Session { session: "$1".into() });
        ctrl(&mut app, 'x');
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.status, Some(Status::Note("that is the only session: kill the server instead".into())));
    }

    #[test]
    fn shift_arrows_swap_neighbours_and_cross_at_the_ends() {
        let shift = |app: &mut App, code| app.on_key(key(code, KeyModifiers::SHIFT));
        let mut app = sessions_app("@1");
        select(&mut app, window("$1", "@1"));
        let j = job(shift(&mut app, KeyCode::Down));
        let swap = Op::SwapWindows { session: "$1".into(), a: "@1".into(), b: "@2".into() };
        assert_eq!((j.op, j.undo), (swap.clone(), Some(swap)));
        assert!(matches!(shift(&mut app, KeyCode::Up), Action::None), "the first window of the first session");
        assert_eq!(app.status, Some(Status::Note("already the first window".into())));
        // The last window of work goes first into play.
        select(&mut app, window("$1", "@2"));
        let j = job(shift(&mut app, KeyCode::Down));
        assert_eq!(j.op, Op::MoveWindow { from: "$1".into(), window: "@2".into(), into: "$2".into(), to: "@3".into(), before: true });
        assert_eq!(j.undo, Some(Op::MoveWindow { from: "$2".into(), window: "@2".into(), into: "$1".into(), to: "@1".into(), before: false }));
        // play's only window leaving closes play: ask.
        select(&mut app, window("$2", "@3"));
        assert!(matches!(shift(&mut app, KeyCode::Up), Action::None));
        let Mode::Confirm(c) = &app.mode else { panic!() };
        assert_eq!(c.question, "move window game to work? play closes: it is its only window");
        assert!(matches!(ctrl(&mut app, 'x'), Action::None) && matches!(app.mode, Mode::List), "^x answers a kill only");
        shift(&mut app, KeyCode::Up);
        assert_eq!(job(enter(&mut app)).op, Op::MoveWindow { from: "$2".into(), window: "@3".into(), into: "$1".into(), to: "@2".into(), before: false });
        // Panes swap inside their window, then join the next one.
        app.expanded.insert(window("$1", "@1"));
        app.refresh();
        select(&mut app, pane("$1", "@1", "%1"));
        assert_eq!(job(shift(&mut app, KeyCode::Down)).op, Op::SwapPanes("%1".into(), "%2".into()));
        select(&mut app, pane("$1", "@1", "%2"));
        let j = job(shift(&mut app, KeyCode::Down));
        assert_eq!(j.op, Op::JoinPane { pane: "%2".into(), to: "%3".into(), before: true });
        assert_eq!(j.undo, Some(Op::JoinPane { pane: "%2".into(), to: "%1".into(), before: false }));
        // Not on the agents tab, where cards are sorted by state.
        app.run(Command::Tab(Tab::Agents));
        assert!(matches!(shift(&mut app, KeyCode::Down), Action::None));
    }

    #[test]
    fn ctrl_t_carries_to_where_enter_drops() {
        let mut app = sessions_app("@1");
        select(&mut app, window("$1", "@2"));
        ctrl(&mut app, 't');
        assert!(matches!(app.mode, Mode::Move(_)));
        // Find the spot by filtering: the carried row stays listed.
        type_(&mut app, "game");
        assert!(app.position(&window("$1", "@2")).is_some(), "what is carried stays listed");
        select(&mut app, window("$2", "@3"));
        let lines = screen(&draw(&app, &mut View::new(), 120, 14));
        assert!(lines.iter().any(|l| l.contains("game") && l.contains("⏎ after this window")), "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("⇅ moving window work:2 logs")), "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("⇅") && l.contains("logs")), "the carried row is marked: {lines:#?}");
        let j = job(enter(&mut app));
        assert_eq!(j.op, Op::MoveWindow { from: "$1".into(), window: "@2".into(), into: "$2".into(), to: "@3".into(), before: false });
        assert!(matches!(app.mode, Mode::List));
        // On a session: first in it. Already there: nothing to do.
        app.query.set("");
        app.requery();
        select(&mut app, window("$1", "@2"));
        ctrl(&mut app, 't');
        select(&mut app, Target::Session { session: "$2".into() });
        assert_eq!(job(enter(&mut app)).op, Op::MoveWindow { from: "$1".into(), window: "@2".into(), into: "$2".into(), to: "@3".into(), before: true });
        select(&mut app, window("$1", "@2"));
        ctrl(&mut app, 't');
        select(&mut app, window("$1", "@1"));
        assert!(matches!(enter(&mut app), Action::None));
        assert_eq!(app.status, Some(Status::Note("already there".into())));
        // Esc puts it down again.
        ctrl(&mut app, 't');
        app.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::List), "esc cancels the move, not the popup");
    }

    #[test]
    fn panes_drop_into_windows_beside_panes_or_on_their_own() {
        let mut app = sessions_app("@1");
        app.expanded.insert(window("$1", "@1"));
        app.refresh();
        let carry = |app: &mut App, to: Target| {
            select(app, pane("$1", "@1", "%2"));
            ctrl(app, 't');
            select(app, to);
            job(enter(app)).op
        };
        assert_eq!(carry(&mut app, window("$1", "@2")), Op::JoinPane { pane: "%2".into(), to: "%3".into(), before: false });
        assert_eq!(carry(&mut app, pane("$1", "@1", "%1")), Op::SwapPanes("%2".into(), "%1".into()));
        assert_eq!(carry(&mut app, Target::Session { session: "$2".into() }), Op::BreakPane { pane: "%2".into(), session: "$2".into() });
    }

    #[test]
    fn dragging_a_row_moves_it() {
        let mut app = sessions_app("@1");
        let mut view = View::new();
        let term = draw(&app, &mut view, 120, 20);
        let (logs, game) = (find(&term, "logs"), find(&term, "game"));
        let now = Instant::now();
        mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Left), logs, now);
        mouse(&mut app, &view, MouseEventKind::Drag(MouseButton::Left), logs, now);
        assert!(matches!(app.mode, Mode::List), "no move until the pointer leaves the row");
        mouse(&mut app, &view, MouseEventKind::Drag(MouseButton::Left), game, now);
        assert!(matches!(app.mode, Mode::Move(_)));
        let j = job(mouse(&mut app, &view, MouseEventKind::Up(MouseButton::Left), game, now));
        assert_eq!(j.op, Op::MoveWindow { from: "$1".into(), window: "@2".into(), into: "$2".into(), to: "@3".into(), before: false });
        // Released off the list: never mind.
        mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Left), logs, now);
        mouse(&mut app, &view, MouseEventKind::Drag(MouseButton::Left), game, now);
        mouse(&mut app, &view, MouseEventKind::Up(MouseButton::Left), (110, 18), now);
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.status, Some(Status::Note("move cancelled".into())));
        // Released outside the popup (no release comes): the next click only
        // ends the drag; it doesn't drop.
        mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Left), logs, now);
        mouse(&mut app, &view, MouseEventKind::Drag(MouseButton::Left), game, now);
        assert!(matches!(click(&mut app, &view, MouseButton::Left, game), Action::None));
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn a_moved_window_stays_selected_where_it_lands() {
        let mut app = sessions_app("@1");
        select(&mut app, window("$1", "@2"));
        let j = job(app.on_key(key(KeyCode::Down, KeyModifiers::SHIFT)));
        assert_eq!(app.ran(j, Ok(None)), None);
        app.set_snapshot(fixture(&[
            "$1|work|@1|1|edit|1|0|%1|0|1|nvim|||||||||/h/a",
            "$1|work|@1|1|edit|1|0|%2|1|0|zsh|||||||||/h/a",
            "$2|play|@2|1|logs|0|0|%3|0|1|tail|||||||||/h/b",
            "$2|play|@3|2|game|1|0|%4|0|1|codex||asking|0|codex|deploy||||/h/c",
        ]));
        assert_eq!(app.selected_target(), Some(&window("$2", "@2")));
        // A pane that becomes a window of its own is selected as that window.
        let mut app = sessions_app("@1");
        app.follow = Some("%4".into());
        app.set_snapshot(fixture(&["$2|play|@3|1|game|1|0|%4|0|1|codex||asking|0|codex|deploy||||/h/c"]));
        assert_eq!(app.selected_target(), Some(&window("$2", "@3")));
        let mut app = sessions_app("@1");
        select(&mut app, window("$1", "@2"));
        let j = job(app.on_key(key(KeyCode::Down, KeyModifiers::SHIFT)));
        app.ran(j, Ok(None));
        app.set_snapshot(fixture(&[
            "$1|work|@1|1|edit|1|0|%1|0|1|nvim|||||||||/h/a",
            "$1|work|@1|1|edit|1|0|%2|1|0|zsh|||||||||/h/a",
            "$2|play|@2|1|logs|0|0|%3|0|1|tail|||||||||/h/b",
            "$2|play|@3|2|game|1|0|%4|0|1|codex||asking|0|codex|deploy||||/h/c",
        ]));
        // And ^z puts it back, following it again.
        let undo = job(ctrl(&mut app, 'z'));
        assert_eq!(undo.op, Op::MoveWindow { from: "$2".into(), window: "@2".into(), into: "$1".into(), to: "@1".into(), before: false });
        assert_eq!(undo.follow.as_deref(), Some("@2"));
    }

    #[test]
    fn ctrl_s_names_a_new_window_where_it_will_appear() {
        let mut app = sessions_app("@1");
        select(&mut app, window("$1", "@1"));
        ctrl(&mut app, 's');
        let Mode::Create(_) = &app.mode else { panic!("{:?}", app.mode) };
        // Right after "edit", before "logs", and selected.
        let names: Vec<String> = app.rows.iter().map(|r| match &r.kind {
            RowKind::Window { name, .. } => name.clone(),
            RowKind::New => "+".into(),
            _ => "·".into(),
        }).collect();
        assert_eq!(names, ["·", "edit", "+", "logs", "·", "game"]);
        assert!(matches!(app.rows[app.selected].kind, RowKind::New));
        let lines = screen(&draw(&app, &mut View::new(), 120, 14));
        assert!(lines.iter().any(|l| l.contains("├─ + ▏window name (empty: named after what runs)  /h/a")), "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("+ new window")), "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("⏎ create   ^s session   ^u clear") && l.ends_with("esc cancel │")), "{lines:#?}");
        let mut view = View::new();
        let term = draw(&app, &mut view, 120, 14);
        click(&mut app, &view, MouseButton::Left, find(&term, "▏window name"));
        assert_eq!(app.status, None, "a click on the row being named");
        click(&mut app, &view, MouseButton::Left, find(&term, "logs"));
        assert_eq!(app.status, Some(Status::Note("⏎ create or esc cancel first".into())));
        assert!(matches!(app.mode, Mode::Create(_)));
        // Moving and filtering wait: keys name it.
        app.on_key(key(KeyCode::Down, KeyModifiers::NONE));
        type_(&mut app, "notes");
        assert_eq!(app.query.text(), "");
        let j = job(enter(&mut app));
        assert_eq!(j.op, Op::NewWindow { session: "$1".into(), after: "@1".into(), name: "notes".into(), dir: "/h/a".into() });
        assert!(j.open, "and goes there");
        assert!(matches!(app.mode, Mode::List));
        assert!(!naming(&app), "the row goes with the mode");
        // On a session row: last in it. No name: tmux names it.
        select(&mut app, Target::Session { session: "$1".into() });
        ctrl(&mut app, 's');
        assert_eq!(job(enter(&mut app)).op, Op::NewWindow { session: "$1".into(), after: "@2".into(), name: String::new(), dir: "/h/a".into() });
        // Esc: nothing made, and the window naming began on stays selected
        // (not "logs", which takes the new row's place).
        select(&mut app, window("$1", "@1"));
        ctrl(&mut app, 's');
        assert!(matches!(app.on_key(key(KeyCode::Esc, KeyModifiers::NONE)), Action::None));
        assert!(matches!(app.mode, Mode::List) && !naming(&app));
        assert_eq!(app.selected_target(), Some(&window("$1", "@1")));
    }

    #[test]
    fn ctrl_s_again_names_a_new_session_and_refuses_what_tmux_would() {
        let mut app = sessions_app("@1");
        select(&mut app, window("$2", "@3"));
        ctrl(&mut app, 's');
        type_(&mut app, "x");
        ctrl(&mut app, 's');
        assert!(matches!(app.rows.last().map(|r| &r.kind), Some(RowKind::New)), "at the end");
        assert!(matches!(&app.mode, Mode::Create(c) if matches!(c.new, New::Session { .. }) && c.field.text() == "x"), "the name stays");
        // And back again.
        ctrl(&mut app, 's');
        assert!(matches!(&app.mode, Mode::Create(c) if matches!(c.new, New::Window { .. })));
        ctrl(&mut app, 's');
        ctrl(&mut app, 'u');
        assert!(matches!(enter(&mut app), Action::None));
        let error = |app: &App| match &app.mode {
            Mode::Create(c) => c.error.clone(),
            _ => None,
        };
        assert_eq!(error(&app).as_deref(), Some("a session needs a name"));
        type_(&mut app, "play");
        assert_eq!(error(&app), None, "typing clears the reason");
        enter(&mut app);
        assert_eq!(error(&app).as_deref(), Some("a session called play exists"));
        let lines = screen(&draw(&app, &mut View::new(), 120, 14));
        assert!(lines.iter().any(|l| l.contains("+ play▏  /h/c") && l.contains("✗ a session called play exists")), "{lines:#?}");
        ctrl(&mut app, 'u');
        type_(&mut app, "blog");
        let j = job(enter(&mut app));
        assert_eq!(j.op, Op::NewSession { name: "blog".into(), dir: "/h/c".into() }, "in the selected row's folder");
        assert!(j.open);
        // From an agent card: the sessions tab, after the agent's window.
        app.run(Command::Tab(Tab::Agents));
        ctrl(&mut app, 's');
        assert_eq!(app.tab, Tab::Sessions);
        assert!(matches!(&app.mode, Mode::Create(Create { new: New::Window { after, .. }, .. }) if after == "@3"));
    }

    #[test]
    fn a_title_is_not_undone_onto_an_exited_agent() {
        let mut app = agents_app();
        ctrl(&mut app, 'r');
        type_(&mut app, "!");
        let j = job(enter(&mut app));
        app.ran(j, Ok(None));
        app.set_snapshot(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|zsh|||||||||/a"]));
        assert!(matches!(ctrl(&mut app, 'z'), Action::None));
        assert_eq!(app.status, Some(Status::Note("that agent has exited".into())));
    }
}

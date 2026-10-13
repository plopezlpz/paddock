//! Popup state: the snapshot, the current tab's rows, selection and input.
//! Where things landed on screen is the View, written by each draw.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::model::{Snapshot, Target};
use crate::rows::{self, Filter, Group, Row, RowKind};

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
}

pub enum Action {
    None,
    Quit,
    Activate(Target),
    Run(Op),
}

/// A change paddock carries out while staying open (Action::Run).
#[derive(Debug, PartialEq)]
pub enum Op {
    /// Set PANE's description override; empty clears it.
    Describe { pane: String, text: String },
    /// Have pi generate PANE's description in the background.
    Generate { pane: String },
}

/// The selected agent's description being edited in the input line.
pub struct Edit {
    pub pane: String,
    pub text: String,
    /// The title when editing began, and whether it was an override: an
    /// untouched automatic description is not saved as one.
    original: String,
    overridden: bool,
}

/// What a key or a tap on the title or footer does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Tab(Tab),
    Open,
    /// Expand or collapse the selected window's panes.
    Panes,
    Group,
    Preview,
    Close,
    /// Edit the selected agent's description.
    Describe,
    Save,
    Generate,
    Cancel,
}

/// Where the last draw put things: the scroll position and what is under
/// each screen cell. Owned by the loop, written by `ui::draw`.
pub struct View {
    /// Under 80 columns (a phone): fewer details, two-line rows.
    pub compact: bool,
    /// Whether the preview has room. Assumed until the first draw, so the
    /// first frame already has it.
    pub preview_fits: bool,
    /// The first list row on screen.
    pub offset: usize,
    pub rows: Vec<(Rect, Target)>,
    pub buttons: Vec<(Rect, Button)>,
}

impl View {
    pub fn new() -> View {
        View { compact: false, preview_fits: true, offset: 0, rows: Vec::new(), buttons: Vec::new() }
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

    fn row_at(&self, at: Position) -> Option<&Target> {
        self.rows.iter().find(|(r, _)| r.contains(at)).map(|(_, t)| t)
    }

    fn button_at(&self, at: Position) -> Option<Button> {
        self.buttons.iter().find(|(r, _)| r.contains(at)).map(|&(_, b)| b)
    }
}

/// PageUp and PageDown move this many rows.
const PAGE: isize = 10;

pub struct App {
    pub(crate) snap: Snapshot,
    pub(crate) tab: Tab,
    pub(crate) group: Group,
    pub(crate) query: String,
    pub(crate) rows: Vec<Row>,
    pub(crate) selected: usize,
    pub(crate) preview_on: bool,
    current_session: String,
    current_window: String,
    /// A message shown instead of the footer until the next key or tap.
    pub(crate) status: String,
    pub(crate) edit: Option<Edit>,
    /// Windows whose panes are listed.
    expanded: HashSet<Target>,
    filter: Filter,
    home: String,
    debug: bool,
    /// Whether descriptions can be edited: off without agent-status.tmux's
    /// describe.sh, so ^e is neither offered nor handled.
    pub(crate) can_describe: bool,
}

impl App {
    pub fn new(snap: Snapshot, tab: Tab, current_session: String, current_window: String, home: String, debug: bool, can_describe: bool) -> App {
        let mut app = App {
            snap,
            tab,
            group: Group::Status,
            query: String::new(),
            rows: Vec::new(),
            selected: 0,
            preview_on: true,
            current_session,
            current_window,
            status: String::new(),
            edit: None,
            expanded: HashSet::new(),
            filter: Filter::new(),
            home,
            debug,
            can_describe,
        };
        app.rebuild(true);
        app
    }

    fn selected_target(&self) -> Option<&Target> {
        self.rows.get(self.selected)?.target.as_ref()
    }

    /// The pane to preview, if the preview is on screen.
    pub fn preview_pane(&self, view: &View) -> Option<&str> {
        if !(self.preview_on && view.preview_fits) {
            return None;
        }
        self.rows.get(self.selected)?.preview.as_deref()
    }

    pub fn set_snapshot(&mut self, snap: Snapshot) {
        self.snap = snap;
        self.rebuild(false);
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
            Tab::Sessions => rows::sessions(&self.snap, &mut self.filter, &self.expanded, &self.current_session, &self.home),
            Tab::Agents => {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                rows::agents(&self.snap, &mut self.filter, self.group, &self.home, now)
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
                None
            }
        });
        self.selected = pick.or_else(|| self.nearest(0)).unwrap_or(0);
    }

    fn position(&self, target: &Target) -> Option<usize> {
        self.rows.iter().position(|r| r.target.as_ref() == Some(target))
    }

    /// TARGET's row: still OLD if it is there (a window linked twice into a
    /// session is listed twice), else its first.
    fn locate(&self, target: &Target, old: usize) -> Option<usize> {
        let at_old = self.rows.get(old).is_some_and(|r| r.target.as_ref() == Some(target));
        if at_old { Some(old) } else { self.position(target) }
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

    /// The expandable window the selected row is, or is a pane of.
    fn selected_window(&self) -> Option<Target> {
        match self.rows.get(self.selected)? {
            Row { target: Some(t @ Target::Window { .. }), kind: RowKind::Window { expanded: Some(_), .. }, .. } => Some(t.clone()),
            Row { target: Some(Target::Pane { session, window, .. }), kind: RowKind::Pane { .. }, .. } => {
                Some(Target::Window { session: session.clone(), window: window.clone() })
            }
            _ => None,
        }
    }

    /// Expand (Some(true)), collapse (Some(false)) or toggle (None) the
    /// selected row's window; collapsing from a pane selects its window.
    fn expand(&mut self, how: Option<bool>) {
        let Some(window) = self.selected_window() else { return };
        if how.unwrap_or(!self.expanded.contains(&window)) {
            self.expanded.insert(window);
        } else {
            if let Some(i) = self.position(&window) {
                self.selected = i;
            }
            self.expanded.remove(&window);
        }
        self.rebuild(false);
    }

    fn requery(&mut self) {
        self.filter.set(&self.query);
        self.rebuild(true);
    }

    /// Pasted text joins the query or the description being edited
    /// (bracketed paste: a newline in it must not act as Enter).
    pub fn paste(&mut self, text: &str) {
        let text = text.chars().filter(|c| !c.is_control());
        match &mut self.edit {
            Some(edit) => edit.text.extend(text),
            None => {
                self.query.extend(text);
                self.requery();
            }
        }
    }

    /// Start editing the selected agent's description.
    fn describe(&mut self) {
        if !self.can_describe {
            return;
        }
        let Some(Target::Pane { pane, .. }) = self.selected_target() else { return };
        let Some(agent) = self.snap.pane(pane).and_then(|(_, _, p)| p.agent.as_ref()) else {
            return;
        };
        let (original, overridden) = (agent.title.clone(), agent.overridden);
        self.edit = Some(Edit { pane: pane.clone(), text: original.clone(), original, overridden });
    }

    /// Finish editing: Save, Generate or Cancel.
    fn finish_edit(&mut self, button: Button) -> Action {
        let Some(edit) = self.edit.take() else { return Action::None };
        let text = edit.text.trim().to_string();
        match button {
            // An untouched automatic description stays automatic.
            Button::Save if edit.overridden || text != edit.original => Action::Run(Op::Describe { pane: edit.pane, text }),
            Button::Generate => Action::Run(Op::Generate { pane: edit.pane }),
            _ => Action::None,
        }
    }

    fn set_tab(&mut self, tab: Tab) {
        if tab != self.tab {
            self.tab = tab;
            self.rebuild(true);
        }
    }

    /// Do what BUTTON says, whether pressed by key or tapped.
    pub fn press(&mut self, button: Button) -> Action {
        if self.edit.is_some() {
            return match button {
                Button::Save | Button::Generate | Button::Cancel => self.finish_edit(button),
                _ => Action::None,
            };
        }
        match button {
            Button::Tab(tab) => self.set_tab(tab),
            Button::Open => return self.selected_target().cloned().map_or(Action::None, Action::Activate),
            Button::Panes => self.expand(None),
            Button::Group if self.tab == Tab::Agents => {
                self.group = self.group.next();
                self.rebuild(false);
            }
            Button::Preview => self.preview_on = !self.preview_on,
            Button::Close => return Action::Quit,
            Button::Describe => self.describe(),
            Button::Group | Button::Save | Button::Generate | Button::Cancel => {}
        }
        Action::None
    }

    /// Keys while editing a description: text, ⏎ save, ^g generate, esc
    /// cancel.
    fn edit_key(&mut self, key: KeyEvent, ctrl: bool, plain: bool) -> Action {
        match key.code {
            KeyCode::Enter => return self.press(Button::Save),
            KeyCode::Esc => return self.press(Button::Cancel),
            KeyCode::Char('c') if ctrl => return self.press(Button::Cancel),
            KeyCode::Char('g') if ctrl => return self.press(Button::Generate),
            _ => {}
        }
        let Some(edit) = &mut self.edit else { return Action::None };
        match key.code {
            KeyCode::Char('u') if ctrl => edit.text.clear(),
            KeyCode::Backspace => {
                edit.text.pop();
            }
            KeyCode::Char(c) if plain => edit.text.push(c),
            _ => {}
        }
        Action::None
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if !self.debug {
            self.status.clear();
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if self.edit.is_some() {
            return self.edit_key(key, ctrl, plain);
        }
        match key.code {
            KeyCode::Esc => return self.press(Button::Close),
            KeyCode::Char('c') if ctrl => return self.press(Button::Close),
            KeyCode::Enter => return self.press(Button::Open),
            KeyCode::Tab | KeyCode::BackTab => return self.press(Button::Tab(self.tab.other())),
            KeyCode::Char('g') if ctrl => return self.press(Button::Group),
            KeyCode::Char('e') if ctrl => return self.press(Button::Describe),
            // ^/ too: terminals send it as ^_, which crossterm reports as
            // ctrl-7 or ctrl-_.
            KeyCode::Char('o' | '/' | '7' | '_') if ctrl => return self.press(Button::Preview),
            KeyCode::Up => self.move_by(-1, true),
            KeyCode::Down => self.move_by(1, true),
            KeyCode::Char('p' | 'k') if ctrl => self.move_by(-1, true),
            KeyCode::Char('n' | 'j') if ctrl => self.move_by(1, true),
            KeyCode::PageUp => self.move_by(-PAGE, false),
            KeyCode::PageDown => self.move_by(PAGE, false),
            KeyCode::Right => self.expand(Some(true)),
            KeyCode::Left => self.expand(Some(false)),
            KeyCode::Char('u') if ctrl && !self.query.is_empty() => {
                self.query.clear();
                self.requery();
            }
            KeyCode::Backspace => {
                if self.query.pop().is_some() {
                    self.requery();
                }
            }
            KeyCode::Char(c) if plain => {
                self.query.push(c);
                self.requery();
            }
            _ => {}
        }
        Action::None
    }

    /// A click or tap, resolved against what the last draw showed: tapping
    /// a row selects it, tapping the selected row opens it (so a
    /// double-click opens too).
    pub fn on_mouse(&mut self, ev: MouseEvent, view: &View) -> Action {
        if self.debug {
            self.status = format!("mouse {:?} at {},{}", ev.kind, ev.column, ev.row);
        } else if !matches!(ev.kind, MouseEventKind::Up(_)) {
            // Not on the release of the tap that set it.
            self.status.clear();
        }
        let at = Position::new(ev.column, ev.row);
        if self.edit.is_some() {
            // Only the footer's save, generate and cancel while editing.
            return match (ev.kind, view.button_at(at)) {
                (MouseEventKind::Down(MouseButton::Left), Some(button)) => self.press(button),
                _ => Action::None,
            };
        }
        match ev.kind {
            MouseEventKind::ScrollUp => self.move_by(-1, false),
            MouseEventKind::ScrollDown => self.move_by(1, false),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(button) = view.button_at(at) {
                    return self.press(button);
                }
                let Some(target) = view.row_at(at) else { return Action::None };
                if self.selected_target() == Some(target) {
                    return Action::Activate(target.clone());
                }
                if let Some(i) = self.position(target) {
                    self.selected = i;
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if let Some(i) = view.row_at(at).and_then(|t| self.position(t)) {
                    self.selected = i;
                    self.expand(None);
                }
            }
            _ => {}
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixture;
    use crate::theme::Theme;
    use crate::ui::{self, Preview};
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;
    use ratatui::text::Text;
    use ratatui::Terminal;

    /// An app opened from window WINDOW of session $1, describe.sh present.
    fn new(snap: Snapshot, tab: Tab, window: &str) -> App {
        App::new(snap, tab, "$1".into(), window.into(), String::new(), false, true)
    }

    /// Agents in two folders: header, card, card, header, card, card.
    fn agents_app() -> App {
        let snap = fixture(&[
            "$1|s|@1|1|w|1|%1|0|1|zsh|working|0|claude|one||/a",
            "$1|s|@1|1|w|1|%2|1|0|zsh|working|0|claude|two||/a",
            "$1|s|@2|2|w|0|%3|0|1|zsh|working|0|claude|three||/b",
            "$1|s|@2|2|w|0|%4|1|0|zsh|working|0|claude|four||/b",
        ]);
        let mut app = new(snap, Tab::Agents, "@1");
        app.group = Group::Folder;
        app.rebuild(false);
        app
    }

    fn sessions_app(window: &str) -> App {
        let snap = fixture(&[
            "$1|work|@1|1|edit|1|%1|0|1|nvim||||||/h/a",
            "$1|work|@1|1|edit|1|%2|1|0|zsh||||||/h/a",
            "$1|work|@2|2|logs|0|%3|0|1|tail||||||/h/b",
            "$2|play|@3|1|game|1|%4|0|1|zsh|asking|0|codex|deploy||/h/c",
        ]);
        new(snap, Tab::Sessions, window)
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent { code, modifiers, kind: KeyEventKind::Press, state: KeyEventState::NONE }
    }

    fn click(app: &mut App, view: &View, button: MouseButton, at: (u16, u16)) -> Action {
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(button), column: at.0, row: at.1, modifiers: KeyModifiers::NONE }, view)
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
        }
    }

    #[test]
    fn opens_on_the_current_window_and_selects_the_best_match() {
        let mut app = sessions_app("@2");
        assert_eq!(selected_title(&app), "logs");
        for c in "codex".chars() {
            app.on_key(key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(selected_title(&app), "game", "the window whose agent matches");
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
            "$1|s|@1|1|w|1|%1|0|1|zsh|working|0|claude|one||/a",
            "$1|s|@1|1|w|1|%2|1|0|zsh|working|0|claude|two||/a",
            "$1|s|@2|2|w|0|%4|1|0|zsh|working|0|claude|four||/b",
        ]));
        assert_eq!(selected_title(&app), "four");
    }

    #[test]
    fn the_selection_follows_an_agent_into_another_group() {
        let line = |state: &str| format!("$1|s|@1|1|w|1|%2|1|0|zsh|{state}|0|claude|two||/a");
        let snap = |state: &str| fixture(&["$1|s|@1|1|w|1|%1|0|1|zsh|working|0|claude|one||/a", &line(state)]);
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
        app.press(Button::Tab(Tab::Sessions));
        assert_eq!(app.selected_target(), Some(&Target::Window { session: "$1".into(), window: "@1".into() }), "panes are not listed");
        app.move_by(2, false);
        app.expand(Some(true));
        app.move_by(2, false);
        let pane = Target::Pane { session: "$1".into(), window: "@2".into(), pane: "%4".into() };
        assert_eq!(app.selected_target(), Some(&pane));
        app.press(Button::Tab(Tab::Agents));
        assert_eq!(selected_title(&app), "four");
        app.press(Button::Tab(Tab::Sessions));
        assert_eq!(app.selected_target(), Some(&pane), "back on the listed pane");
    }

    #[test]
    fn a_window_linked_twice_keeps_its_selected_row() {
        let mut app = new(fixture(&["$1|s|@1|1|w|1|%1|0|1|zsh||||||/a", "$1|s|@1|2|w|0|%1|0|1|zsh||||||/a"]), Tab::Sessions, "@1");
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
    fn tapping_selects_and_tapping_the_selection_opens() {
        let mut app = agents_app();
        let mut view = View::new();
        let term = draw(&app, &mut view, 120, 20);
        assert!(matches!(click(&mut app, &view, MouseButton::Left, find(&term, "two")), Action::None));
        assert_eq!(selected_title(&app), "two");
        let open = click(&mut app, &view, MouseButton::Left, find(&term, "two"));
        assert!(matches!(open, Action::Activate(Target::Pane { pane, .. }) if pane == "%2"));
    }

    #[test]
    fn clicks_hit_list_rows_only() {
        let mut app = agents_app();
        let mut view = View::new();
        // 9 list lines: header, card, card, header, then a card cut off after one line.
        let term = draw(&app, &mut view, 120, 14);
        assert!(view.preview_fits);
        let (x, y) = find(&term, "three");
        let before = app.selected;
        click(&mut app, &view, MouseButton::Left, (x, y - 1));
        assert_eq!(app.selected, before, "headers are not selectable");
        click(&mut app, &view, MouseButton::Right, (x, y - 1));
        assert_eq!(app.selected, before, "not even by right-click");
        click(&mut app, &view, MouseButton::Left, (x + 60, y));
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
        app.on_key(key(KeyCode::Char('z'), KeyModifiers::NONE));
        assert!(app.rows.is_empty());
        assert!(matches!(click(&mut app, &view, MouseButton::Left, at), Action::None));
    }

    #[test]
    fn title_and_footer_are_buttons() {
        let mut app = sessions_app("@1");
        let mut view = View::new();
        let term = draw(&app, &mut view, 46, 20);
        click(&mut app, &view, MouseButton::Left, find(&term, "agents"));
        assert_eq!(app.tab, Tab::Agents);
        let term = draw(&app, &mut view, 46, 20);
        assert!(screen(&term).iter().any(|l| l.contains("open   sessions   group   edit   close")));
        click(&mut app, &view, MouseButton::Left, find(&term, "group"));
        assert!(app.group == Group::Session);
        assert!(matches!(click(&mut app, &view, MouseButton::Left, find(&term, "close")), Action::Quit));
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
        app.press(Button::Tab(Tab::Agents));
        let lines = screen(&draw(&app, &mut view, 46, 20));
        let at = lines.iter().position(|l| l.contains("deploy")).unwrap();
        assert!(lines[at + 1].contains("play · <1m"), "{lines:#?}");
        assert!(!lines[at + 1].contains("codex"), "no kind on a phone");
    }

    #[test]
    fn no_describing_without_the_script() {
        let mut app = agents_app();
        app.can_describe = false;
        app.on_key(key(KeyCode::Char('e'), KeyModifiers::CONTROL));
        assert!(app.edit.as_ref().is_none(), "ctrl-e must not open an editor that can't save");
    }

    #[test]
    fn describing_an_agent() {
        let mut app = agents_app();
        let type_ = |app: &mut App, text: &str| text.chars().for_each(|c| drop(app.on_key(key(KeyCode::Char(c), KeyModifiers::NONE))));
        app.on_key(key(KeyCode::Char('e'), KeyModifiers::CONTROL));
        assert_eq!(app.edit.as_ref().map(|e| e.text.as_str()), Some("one"), "starts from the current title");
        // An untouched automatic description is not pinned as an override.
        assert!(matches!(app.on_key(key(KeyCode::Enter, KeyModifiers::NONE)), Action::None));
        assert!(app.edit.as_ref().is_none());
        app.on_key(key(KeyCode::Char('e'), KeyModifiers::CONTROL));
        type_(&mut app, " more");
        let save = app.on_key(key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(save, Action::Run(Op::Describe { pane, text }) if pane == "%1" && text == "one more"));
        assert_eq!(app.query, "", "typing went to the description, not the filter");
        app.on_key(key(KeyCode::Char('e'), KeyModifiers::CONTROL));
        let generate = app.on_key(key(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert!(matches!(generate, Action::Run(Op::Generate { pane }) if pane == "%1"));
        app.on_key(key(KeyCode::Char('e'), KeyModifiers::CONTROL));
        assert!(matches!(app.on_key(key(KeyCode::Esc, KeyModifiers::NONE)), Action::None));
        assert!(app.edit.as_ref().is_none(), "esc cancels the edit, not the popup");
    }

    #[test]
    fn a_generating_description_shows_on_its_card() {
        let rows = [["$1", "s", "@1", "1", "w", "1", "%1", "0", "1", "zsh", "working", "0", "claude", "task", "", "/a"].map(String::from).to_vec()];
        let snap = crate::model::parse(&rows, |_| crate::model::Described { text: None, generating: true });
        let app = new(snap, Tab::Agents, "@1");
        let lines = screen(&draw(&app, &mut View::new(), 120, 12));
        assert!(lines.iter().any(|l| l.contains("task  ✦ generating…")), "{lines:#?}");
    }

    #[test]
    fn pasted_newlines_do_not_open() {
        let mut app = agents_app();
        app.paste("tw\no\n");
        assert_eq!(app.query, "two");
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
    fn alt_letters_are_not_typed() {
        let mut app = agents_app();
        app.on_key(key(KeyCode::Char('x'), KeyModifiers::ALT));
        assert_eq!(app.query, "");
        app.on_key(key(KeyCode::Char('T'), KeyModifiers::SHIFT));
        assert_eq!(app.query, "T");
    }

    #[test]
    fn left_collapses_and_right_expands() {
        let snap = fixture(&["$1|s|@1|1|w|1|%1|0|1|nvim||||||/a", "$1|s|@1|1|w|1|%2|1|0|zsh||||||/a"]);
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
}

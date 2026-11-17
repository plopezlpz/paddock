//! Where the last draw put things (the View, owned by the loop and written
//! by `ui::draw`), and what a pointer does with them: presses, releases,
//! drags and the wheel.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::{Action, App, Command, Mode};
use crate::model::Target;
use crate::rows::{Row, RowKind};

/// What a mouse press landed on; the release completes it there.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Hit {
    /// A list row: its index when drawn, and its target.
    Row(usize, Target),
    Button(Command),
    /// A window's ▸/▾.
    Toggle(Target),
    Hidden,
    MenuItem(usize),
}

pub(super) struct Press {
    hit: Option<Hit>,
    at: Position,
    /// The row was already selected (compact: a tap on it opens).
    was_selected: bool,
}

/// Two clicks on a row this close together open it.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

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
    pub hidden: Option<Rect>,
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
            hidden: None,
            menu: None,
            items: Vec::new(),
        }
    }

    /// Forget the last draw's hit areas.
    pub fn clear(&mut self) {
        self.rows.clear();
        self.buttons.clear();
        self.toggles.clear();
        self.hidden = None;
        self.menu = None;
        self.items.clear();
    }

    /// Scroll so the selection is visible in HEIGHT lines, with what heads
    /// it when that fits too (a header right above it, a window's or pane's
    /// session), and no space is left empty below the last row.
    pub fn scroll(&mut self, rows: &[Row], selected: usize, height: u16) {
        let n = rows.len();
        if n == 0 {
            self.offset = 0;
            return;
        }
        let h = |i: usize| rows[i].height(self.compact);
        let sel = selected.min(n - 1);
        let head = match rows[sel].kind {
            RowKind::Window { .. } | RowKind::Pane { .. } => (0..sel).rev().find(|&i| matches!(rows[i].kind, RowKind::Session { .. })),
            _ => sel.checked_sub(1).filter(|&i| rows[i].target.is_none()),
        };
        let top = head.filter(|&i| (i..=sel).map(h).sum::<u16>() <= height).unwrap_or(sel);
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
    pub(super) fn hit(&self, at: Position) -> Option<Hit> {
        let inside = |r: &Rect| r.contains(at);
        if let Some(menu) = self.menu {
            return self.items.iter().find(|(r, _)| inside(r)).map(|&(_, i)| Hit::MenuItem(i)).filter(|_| inside(&menu));
        }
        (self.buttons.iter().find(|(r, _)| inside(r)).map(|&(_, c)| Hit::Button(c)))
            .or_else(|| self.hidden.filter(inside).map(|_| Hit::Hidden))
            .or_else(|| self.toggles.iter().find(|(r, _)| inside(r)).map(|(_, t)| Hit::Toggle(t.clone())))
            .or_else(|| self.rows.iter().find(|(r, ..)| inside(r)).map(|(_, i, t)| Hit::Row(*i, t.clone())))
    }
}

impl App {
    /// A mouse event, resolved against what the last draw showed. A press
    /// selects; the release on the same thing acts, so the rest of a click
    /// never reaches whatever is under the pointer once the popup is gone.
    /// Desktop: a double-click opens a row. Compact (a phone, where a
    /// double-tap may arrive as something else): a tap on the selected row
    /// opens it. Right-click (or ctrl-click) opens the row's menu.
    pub fn on_mouse(&mut self, ev: MouseEvent, view: &View, now: Instant) -> Action {
        let at = Position::new(ev.column, ev.row);
        let hit = view.hit(at);
        // Typing: a click or tap only ends it (on a phone, the way out).
        if matches!(self.mode, Mode::Type(_)) {
            if matches!(ev.kind, MouseEventKind::Down(_)) {
                self.mode = Mode::List;
            }
            return Action::None;
        }
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
                        return self.put_down(Some("move cancelled".into()));
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
                if hit == Some(Hit::Hidden) && let Some(i) = self.hidden_heading() {
                    self.selected = i;
                    if context {
                        self.open_menu(Position::new(at.x.saturating_add(1), at.y), view.preview_fits);
                        return Action::None;
                    }
                }
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
                    _ => self.put_down(Some("move cancelled".into())),
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
                    Some(Hit::Button(command)) => self.run(command),
                    Some(Hit::MenuItem(i)) => self.choose(i),
                    Some(Hit::Hidden) => self.run(Command::Hidden),
                    Some(Hit::Toggle(target)) => {
                        if let Some(i) = self.position(&target) {
                            self.selected = i;
                            return self.expand(None);
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
                    Mode::Rename(_) | Mode::Confirm(_) | Mode::Create(_) | Mode::Type(_) => {}
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

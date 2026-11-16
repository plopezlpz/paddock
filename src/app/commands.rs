//! Every action has one Command, whether it comes from a key, a footer hint,
//! a menu item or a tap; `can` says which apply to the selected row, and
//! `hints` and `menu_items` list them.

use ratatui::layout::Position;

use super::{App, Mode, Tab, View};
use crate::model::Target;
use crate::rows::New;

/// Every action, whatever triggers it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    Tab(Tab),
    Open,
    /// Expand or collapse the selected window's panes.
    Panes,
    /// Toggle the Hidden section; unrelated to showing an individual agent.
    Hidden,
    Hide,
    Show,
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
    /// The selected row's menu (^a, the footer's menu).
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

#[derive(Debug)]
pub struct Menu {
    /// The row it was opened on: its items act on that row only.
    pub(super) target: Option<Target>,
    pub(super) hidden: bool,
    pub items: Vec<Hint>,
    pub selected: usize,
    /// The screen cell it hangs from (the click, or the selected row).
    pub anchor: Position,
}

impl App {
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
    pub(super) fn open_menu(&mut self, anchor: Position, preview_fits: bool) {
        let items = self.menu_items(preview_fits);
        if !items.is_empty() {
            self.mode = Mode::Menu(Menu { target: self.selected_target().cloned(), hidden: self.hidden_selected(), items, selected: 0, anchor });
        }
    }

    /// Whether COMMAND applies in the list, to the selected row: what the
    /// footer and the menu offer. PREVIEW_FITS: whether the preview has room.
    pub(super) fn can(&self, command: Command, preview_fits: bool) -> bool {
        match command {
            Command::Open | Command::Kill => self.selected_target().is_some(),
            Command::Panes => self.selected_window().is_some(),
            Command::Hidden => self.hidden_selected() && !self.filter.active(),
            Command::Hide | Command::Show => self.tab == Tab::Agents && self.selected_agent().is_some() && self.selected_agent_hidden() == (command == Command::Show),
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
            // In the list, esc clears the query before it closes.
            Command::Close | Command::Cancel => "esc",
            Command::Clear if matches!(self.mode, Mode::List) => "esc",
            Command::Tab(_) => "⇥",
            Command::Panes if self.selected_window().is_some_and(|(_, open)| open) => "←",
            Command::Panes => "→",
            Command::Hidden => if self.hidden_expanded { "←" } else { "→" },
            Command::Hide | Command::Show => "^e",
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
            Command::Menu => "^a",
        }
    }

    fn hint(&self, command: Command, label: impl Into<String>, rank: u8) -> Hint {
        Hint { key: self.key(command), label: label.into(), command, rank }
    }

    /// What can be done with the selected row, and in the popup.
    pub fn menu_items(&self, preview_fits: bool) -> Vec<Hint> {
        let open = self.selected_window().is_some_and(|(_, open)| open);
        let group = format!("group by {}", self.group.next().label());
        // Rex's tabs are its windows: the popup's two are its lists there.
        let tab = format!("{} {}", self.tab.other().label(), if self.caps.rex { "list" } else { "tab" });
        let new_window = format!("new {} here", self.caps.window());
        [
            (Command::Open, "open"),
            (Command::Panes, if open { "hide panes" } else { "show panes" }),
            (Command::Rename, "rename"),
            (Command::RenameSuggest, "suggest a title"),
            (Command::Hide, "hide"),
            (Command::Show, "show"),
            (Command::Hidden, if self.hidden_expanded { "collapse hidden" } else { "expand hidden" }),
            (Command::Move, "move…"),
            (Command::MoveUp, "move up"),
            (Command::MoveDown, "move down"),
            (Command::Kill, "kill…"),
            (Command::NewWindow, &new_window),
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
                // Suggestions are titles: for agents only.
                if self.can_suggest && matches!(r.target, Target::Pane { .. }) {
                    out.push(self.hint(Command::Suggest, if r.error.is_some() && r.retry { "retry" } else { "suggest" }, 1));
                }
                out.push(self.hint(Command::Clear, "clear", 2));
                return (out, cancel);
            }
            Mode::Menu(_) => return (vec![self.hint(Command::Open, "choose", 0)], self.hint(Command::Cancel, "back", 0)),
            Mode::Confirm(c) => return (vec![self.hint(Command::Confirm, if c.kills() { "kill" } else { "move" }, 0)], cancel),
            Mode::Move(_) => return (vec![self.hint(Command::Confirm, "drop here", 0)], cancel),
            Mode::Create(c) => {
                let other = if matches!(c.new, New::Window { .. }) { "session" } else { self.caps.window() };
                return (vec![self.hint(Command::Save, "create", 0), self.hint(Command::NewSession, other, 1), self.hint(Command::Clear, "clear", 2)], cancel);
            }
            Mode::List => {}
        }
        let open = self.selected_window().is_some_and(|(_, open)| open);
        let other = self.tab.other();
        let group = if view.compact { "group".to_string() } else { format!("group: {}", self.group.label()) };
        let hints = [
            (Command::Open, "open", 0),
            (Command::Tab(other), other.label(), 3),
            (Command::Panes, if open { "hide panes" } else { "panes" }, 4),
            (Command::Group, &group, 5),
            (Command::Rename, "rename", 1),
            (Command::Hide, "hide", 4),
            (Command::Show, "show", 4),
            (Command::Hidden, if self.hidden_expanded { "collapse" } else { "expand" }, 0),
            (Command::Move, "move", 4),
            (Command::Kill, "kill", 5),
            (Command::NewWindow, "new", 7),
            (Command::Undo, "undo", 2),
            (Command::Preview, "preview", 6),
            // Everything else is in it: always offered.
            (Command::Menu, "menu", 0),
        ];
        // New windows show in the sessions tab: its footer offers them.
        let out = (hints.into_iter())
            .filter(|&(c, ..)| self.can(c, view.preview_fits) && (c != Command::NewWindow || self.tab == Tab::Sessions))
            .map(|(c, label, rank)| self.hint(c, label, rank))
            .collect();
        (out, if self.query.is_empty() { self.hint(Command::Close, "close", 0) } else { self.hint(Command::Clear, "clear", 0) })
    }
}

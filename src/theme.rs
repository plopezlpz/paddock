//! Colours, by role. A backend sets the roles it has values for by name
//! (`set`): tmux from its options (backend/tmux), Rex from the action's
//! --color flags; the rest are the terminal's own palette. Values use tmux
//! colour syntax: red, brightred, colour214, #a2a970, default, terminal (not
//! tmux's X11 names, such as orange, which leave the role as it was).
//! State marks are a backend's (`marks`: agent-status.tmux's @agent-icon-*),
//! when one column wide and no other state's (its default `●` for three
//! states is not), else a distinct glyph per state, so state never rests on
//! colour alone.

use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::model::State;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentIcons {
    Unicode,
    Nerd,
}

/// Shared by agent cards and plain-text notifications. Nerd Font marks are
/// opt-in: desktop notification fonts may not contain their private glyphs.
pub fn agent_icon(kind: &str, mode: AgentIcons) -> &'static str {
    match (mode, kind) {
        (AgentIcons::Nerd, "claude") => "\u{ec82}", // cod-claude
        (AgentIcons::Nerd, "codex") => "\u{ec81}", // cod-openai
        (AgentIcons::Nerd, "gemini") => "\u{ec10}", // cod-sparkle
        (AgentIcons::Nerd, "opencode") => "\u{f0763}", // md-square-outline
        (_, "claude") => "✳",
        (_, "codex") => "›_",
        (_, "pi") => "π",
        (_, "opencode") => "◇",
        (_, "gemini") => "✦",
        _ => "▣",
    }
}

pub struct Theme {
    /// Draw the popup's rounded frame. A Rex floating pane has its own
    /// edge, so there the frame's lines are left blank (its titles stay).
    pub frame: bool,
    /// Explicit choice: terminal font coverage cannot be detected over tmux/SSH.
    pub agent_icons: AgentIcons,
    pub accent: Color,
    pub hint: Color,
    /// Secondary text: a colour, or the terminal's dim attribute.
    muted: Option<Color>,
    border: Option<Color>,
    states: [Color; State::ALL.len()],
    icons: [String; State::ALL.len()],
}

/// The roles `set` takes; "waiting" is "asking".
pub const ROLES: &[&str] = &["accent", "muted", "hint", "border", "idle", "finished", "working", "asking", "waiting"];

/// The terminal's own palette, with a frame.
impl Default for Theme {
    fn default() -> Theme {
        Theme {
            frame: true,
            agent_icons: AgentIcons::Unicode,
            // None of the states' colours nor the hint's: a selected card
            // stays apart from its state.
            accent: Color::Magenta,
            hint: Color::Blue,
            muted: None,
            border: None,
            states: [Color::Green, Color::Cyan, Color::Yellow, Color::Red],
            icons: ICONS.map(String::from),
        }
    }
}

impl Theme {
    /// ROLE (one of ROLES) in COLOUR; left as it was when COLOUR is none.
    pub fn set(&mut self, role: &str, colour: &str) {
        let Some(c) = parse_colour(colour) else { return };
        match role {
            "accent" => self.accent = c,
            "muted" => self.muted = Some(c),
            "hint" => self.hint = c,
            "border" => self.border = Some(c),
            "idle" => self.states[State::Idle as usize] = c,
            "finished" => self.states[State::Finished as usize] = c,
            "working" => self.states[State::Working as usize] = c,
            "asking" | "waiting" => self.states[State::Asking as usize] = c,
            _ => {}
        }
    }

    /// MARKS, in State order, as the states' marks: each that takes the
    /// mark's one column and tells its state apart, from the others' marks
    /// and from the ones states without a mark keep (which can leave
    /// another state its own in turn).
    pub fn marks(&mut self, marks: [&str; State::ALL.len()]) {
        let n = marks.len();
        let mut take = marks.map(|m| m.width() == 1 && marks.iter().filter(|o| **o == m).count() == 1);
        while let Some(i) = (0..n).find(|&i| take[i] && (0..n).any(|j| !take[j] && self.icons[j] == marks[i])) {
            take[i] = false;
        }
        for i in (0..n).filter(|&i| take[i]) {
            self.icons[i] = marks[i].to_string();
        }
    }

    pub fn muted(&self) -> Style {
        match self.muted {
            Some(c) => Style::new().fg(c),
            None => Style::new().add_modifier(Modifier::DIM),
        }
    }

    pub fn border(&self) -> Style {
        self.border.map_or_else(|| self.muted(), |c| Style::new().fg(c))
    }

    pub fn state(&self, s: State) -> Style {
        Style::new().fg(self.states[s as usize])
    }

    /// The mark that stands for S.
    pub fn icon(&self, s: State) -> &str {
        &self.icons[s as usize]
    }

    /// The selected row's main text, as the active tab.
    pub fn selected(&self) -> Style {
        Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
    }

    /// Characters the filter matched, over the text's own style.
    pub fn matched(&self) -> Style {
        Style::new().fg(self.accent).add_modifier(Modifier::UNDERLINED).remove_modifier(Modifier::DIM)
    }
}

/// Default state marks, in State order: one column each, told apart by shape.
const ICONS: [&str; State::ALL.len()] = ["○", "✓", "◐", "!"];

/// A tmux colour name, colourN, #rrggbb, or default/terminal (the terminal's
/// own colour). Empty or unknown (e.g. tmux's theme* names) is None.
pub fn parse_colour(s: &str) -> Option<Color> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            let n = u32::from_str_radix(hex, 16).ok()?;
            return Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8));
        }
        return None;
    }
    if let Some(n) = s.strip_prefix("colour").or_else(|| s.strip_prefix("color")) {
        return n.parse::<u8>().ok().map(Color::Indexed);
    }
    Some(match s.as_str() {
        "default" | "terminal" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::Gray,
        "brightblack" => Color::DarkGray,
        "brightred" => Color::LightRed,
        "brightgreen" => Color::LightGreen,
        "brightyellow" => Color::LightYellow,
        "brightblue" => Color::LightBlue,
        "brightmagenta" => Color::LightMagenta,
        "brightcyan" => Color::LightCyan,
        "brightwhite" => Color::White,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tmux_colours() {
        assert_eq!(parse_colour("#A2A970"), Some(Color::Rgb(0xa2, 0xa9, 0x70)));
        assert_eq!(parse_colour("colour214"), Some(Color::Indexed(214)));
        assert_eq!(parse_colour("color8"), Some(Color::Indexed(8)));
        assert_eq!(parse_colour("brightblack"), Some(Color::DarkGray));
        assert_eq!(parse_colour("white"), Some(Color::Gray));
        assert_eq!(parse_colour("terminal"), Some(Color::Reset));
        assert_eq!(parse_colour(""), None);
        assert_eq!(parse_colour("themedarkgrey"), None);
        assert_eq!(parse_colour("#12345"), None);
        assert_eq!(parse_colour("colour300"), None);
    }

    #[test]
    fn terminal_palette_when_nothing_is_set() {
        let t = Theme::default();
        assert_eq!(t.accent, Color::Magenta);
        assert_eq!(t.state(State::Working).fg, Some(Color::Yellow));
        assert!(State::ALL.iter().all(|&s| t.state(s).fg != Some(t.accent)) && t.hint != t.accent, "the accent is no state's colour, nor the hint's");
        assert_eq!(t.muted(), Style::new().add_modifier(Modifier::DIM));
        assert_eq!(t.border(), t.muted());
        assert_eq!(State::ALL.map(|s| t.icon(s)), ["○", "✓", "◐", "!"]);
        assert_eq!(t.agent_icons, AgentIcons::Unicode);
    }

    #[test]
    fn roles_are_set_by_name_and_only_to_colours() {
        let mut t = Theme::default();
        t.set("accent", "brightcyan");
        t.set("muted", "brightblack");
        t.set("waiting", "colour1");
        t.set("working", "orange");
        t.set("sky", "blue");
        assert_eq!(t.accent, Color::LightCyan);
        assert_eq!(t.muted(), Style::new().fg(Color::DarkGray));
        assert_eq!(t.state(State::Asking).fg, Some(Color::Indexed(1)));
        assert_eq!(t.state(State::Working).fg, Some(Color::Yellow), "not a colour: left as it was");
    }

    #[test]
    fn dots_follow_the_status_bar_icons_that_fit() {
        let mut t = Theme::default();
        t.marks(["-", "", "", "??"]); // asking: two columns
        assert_eq!(State::ALL.map(|s| t.icon(s)), ["-", "✓", "◐", "!"]);
        assert!(ICONS.iter().all(|i| i.width() == 1));
    }

    #[test]
    fn status_bar_icons_shared_by_states_are_not_state_marks() {
        // agent-status.tmux's defaults: ● for three states, ○ idle.
        let mut t = Theme::default();
        t.marks(["○", "●", "●", "●"]);
        assert_eq!(State::ALL.map(|s| t.icon(s)), ["○", "✓", "◐", "!"]);
        t.marks(["○", "●", "●", "?"]); // asking, set apart
        assert_eq!(State::ALL.map(|s| t.icon(s)), ["○", "✓", "◐", "?"]);
    }

    #[test]
    fn a_mark_another_state_keeps_is_not_taken() {
        let mut t = Theme::default();
        t.marks(["!", "", "", ""]); // asking keeps its !
        assert_eq!(State::ALL.map(|s| t.icon(s)), ["○", "✓", "◐", "!"]);
        t.marks(["!", "", "", "?"]);
        assert_eq!(State::ALL.map(|s| t.icon(s)), ["!", "✓", "◐", "?"]);
        // Idle keeps its ○, so finished cannot have it either.
        let mut t = Theme::default();
        t.marks(["!", "○", "", ""]);
        assert_eq!(State::ALL.map(|s| t.icon(s)), ["○", "✓", "◐", "!"]);
    }
}

//! Colours. Each role resolves, in order: its @paddock-color-* option, the
//! user's tmux theme (agent-status.tmux's @agent-color-*, the active pane
//! border for the accent), then the terminal's own palette. Values use tmux
//! colour syntax: red, brightred, colour214, #a2a970, default, terminal (not
//! tmux's X11 names, such as orange, which fall through to the next).
//! State dots are agent-status.tmux's @agent-icon-*, when one column wide.

use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::model::State;
use crate::tmux::Control;

pub struct Theme {
    pub accent: Color,
    pub hint: Color,
    /// Secondary text: a colour, or the terminal's dim attribute.
    muted: Option<Color>,
    border: Option<Color>,
    states: [Color; State::ALL.len()],
    icons: [String; State::ALL.len()],
}

/// Roles read from tmux, in this order, in one display-message. E: expands
/// values that are formats themselves (tmux 3.8's default border style is
/// a `#{?...}`).
const OPTIONS: &[&str] = &[
    "E:@paddock-color-accent",
    "E:@paddock-color-muted",
    "E:@paddock-color-hint",
    "E:@paddock-color-border",
    "E:@paddock-color-idle",
    "E:@paddock-color-finished",
    "E:@paddock-color-working",
    "E:@paddock-color-asking",
    "E:@agent-color-idle",
    "E:@agent-color-finished",
    "E:@agent-color-working",
    "E:@agent-color-asking",
    "E:pane-active-border-style",
    "@agent-icon-idle",
    "@agent-icon-finished",
    "@agent-icon-working",
    "@agent-icon-asking",
];

impl Theme {
    pub fn load(tmux: &mut Control, window: &str) -> Theme {
        let row = tmux.row(window, OPTIONS);
        Theme::from_values(&row.iter().map(String::as_str).collect::<Vec<_>>())
    }

    /// VALUES lines up with OPTIONS; missing or unparsable values fall back.
    pub fn from_values(values: &[&str]) -> Theme {
        let v = |i: usize| values.get(i).copied().unwrap_or("");
        let default = |s| match s {
            State::Idle => Color::Green,
            State::Finished => Color::Cyan,
            State::Working => Color::Yellow,
            State::Asking => Color::Red,
        };
        // @paddock-color-<state>, then @agent-color-<state>, in State order.
        let states = State::ALL.map(|s| {
            let i = s as usize;
            parse_colour(v(4 + i)).or(parse_colour(v(8 + i))).unwrap_or(default(s))
        });
        // @agent-icon-<state>, if it takes the dot's one column.
        let icons = State::ALL.map(|s| match v(13 + s as usize) {
            icon if icon.width() == 1 => icon.to_string(),
            _ if s == State::Idle => "○".to_string(),
            _ => "●".to_string(),
        });
        Theme {
            accent: parse_colour(v(0)).or(style_fg(v(12))).unwrap_or(Color::Yellow),
            muted: parse_colour(v(1)),
            hint: parse_colour(v(2)).unwrap_or(Color::Blue),
            border: parse_colour(v(3)),
            states,
            icons,
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

    /// The dot that stands for S.
    pub fn icon(&self, s: State) -> &str {
        &self.icons[s as usize]
    }

    /// The current session's badge: the accent as background, the text in the
    /// terminal's background colour, so it reads on light and dark themes.
    pub fn badge(&self) -> Style {
        Style::new().fg(self.accent).add_modifier(Modifier::REVERSED | Modifier::BOLD)
    }
}

/// The fg= colour of a tmux style such as "bg=#202020,fg=#a2a970".
fn style_fg(style: &str) -> Option<Color> {
    style.split([',', ' ']).find_map(|part| part.trim().strip_prefix("fg=")).and_then(parse_colour)
}

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
    fn reads_fg_from_a_style() {
        assert_eq!(style_fg("fg=#A2A970"), Some(Color::Rgb(0xa2, 0xa9, 0x70)));
        assert_eq!(style_fg("bg=#202020,fg=red"), Some(Color::Red));
        assert_eq!(style_fg("bg=red"), None);
    }

    #[test]
    fn terminal_palette_when_nothing_is_set() {
        let t = Theme::from_values(&[]);
        assert_eq!(t.accent, Color::Yellow);
        assert_eq!(t.state(State::Working).fg, Some(Color::Yellow));
        assert_eq!(t.muted(), Style::new().add_modifier(Modifier::DIM));
        assert_eq!(t.border(), t.muted());
        assert_eq!(State::ALL.map(|s| t.icon(s)), ["○", "●", "●", "●"]);
    }

    #[test]
    fn tmux_theme_then_explicit_options_win() {
        let mut v = vec![""; OPTIONS.len()];
        v[8 + 2] = "#cc8c3c"; // @agent-color-working
        v[12] = "fg=#A2A970"; // pane-active-border-style
        let t = Theme::from_values(&v);
        assert_eq!(t.accent, Color::Rgb(0xa2, 0xa9, 0x70));
        assert_eq!(t.state(State::Working).fg, Some(Color::Rgb(0xcc, 0x8c, 0x3c)));
        v[0] = "magenta";
        v[4 + 2] = "colour1";
        v[1] = "brightblack";
        let t = Theme::from_values(&v);
        assert_eq!(t.accent, Color::Magenta);
        assert_eq!(t.state(State::Working).fg, Some(Color::Indexed(1)));
        assert_eq!(t.muted(), Style::new().fg(Color::DarkGray));
    }

    #[test]
    fn dots_follow_the_status_bar_icons_that_fit() {
        let mut v = vec![""; OPTIONS.len()];
        v[13] = "-"; // @agent-icon-idle
        v[16] = "??"; // @agent-icon-asking: two columns
        let t = Theme::from_values(&v);
        assert_eq!(State::ALL.map(|s| t.icon(s)), ["-", "●", "●", "●"]);
    }
}

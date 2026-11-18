//! Keys typed in the popup, sent on to a pane (typing mode): as tmux's
//! `send-keys` names them, or as Rex's key events, which Rex encodes as the
//! pane's program asked (legacy bytes, or the kitty protocol). Without
//! keyboard enhancements crossterm sees only what a legacy terminal sends:
//! ctrl with `\`, `]`, `^`, `_` arrives as ctrl with 4 to 7. The popup asks
//! for more where it can (`Protocol`): Shift+Enter, which an agent's prompt
//! takes as a new line, is Enter otherwise.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

/// What a pane is sent.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Key(KeyEvent),
    /// Pasted text (bracketed, if the pane's program asked for that).
    Paste(String),
}

/// The keys that leave typing mode: ctrl-h (the way back from ctrl-l, as
/// vim-tmux-navigator goes; Backspace is DEL, never ctrl-h, in tmux and Rex)
/// and ctrl-], which crossterm reports as ctrl-5 (the byte 0x1d).
pub fn is_leave(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('5' | ']' | 'h'))
}

/// Whether KEY comes without a chord modifier (shift aside): text.
pub fn is_plain(key: &KeyEvent) -> bool {
    !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META)
}

/// How the popup asks its terminal for keys a legacy terminal can't tell
/// apart (Shift+Enter from Enter).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Protocol {
    /// As the terminal sends them.
    #[default]
    Legacy,
    /// xterm's modifyOtherKeys, level 1: tmux answers it in CSI u form when
    /// its extended-keys-format is csi-u (the only form crossterm reads).
    ModifyOtherKeys,
    /// The kitty keyboard protocol, disambiguating escape codes (Rex).
    Kitty,
}

impl Protocol {
    /// What turns it on, and what turns it off again.
    pub fn codes(self) -> (&'static str, &'static str) {
        match self {
            Protocol::Legacy => ("", ""),
            Protocol::ModifyOtherKeys => ("\x1b[>4;1m", "\x1b[>4m"),
            Protocol::Kitty => ("\x1b[>1u", "\x1b[<u"),
        }
    }
}

/// How tmux's send-keys takes a key: text to type as is (`-l`), or a key
/// name.
#[derive(Debug, PartialEq)]
pub enum TmuxKey {
    Text(String),
    Name(String),
}

/// KEY for send-keys; None for keys a terminal can't send.
pub fn tmux(key: &KeyEvent) -> Option<TmuxKey> {
    let m = key.modifiers;
    let (ctrl, alt, shift) = (m.contains(KeyModifiers::CONTROL), m.contains(KeyModifiers::ALT), m.contains(KeyModifiers::SHIFT));
    let alt_ = if alt { "M-" } else { "" };
    let name = match key.code {
        KeyCode::Char(c) if ctrl => {
            let base = match c {
                ' ' | '2' | '@' => "Space".to_string(),
                '3' | '[' => "[".to_string(),
                '4' | '\\' => "\\".to_string(),
                '6' | '^' => "^".to_string(),
                '7' | '_' | '/' => "_".to_string(),
                c if c.is_ascii_alphabetic() => c.to_ascii_lowercase().to_string(),
                _ => return None,
            };
            return Some(TmuxKey::Name(format!("C-{alt_}{base}")));
        }
        KeyCode::Char(c) if alt => return Some(TmuxKey::Name(format!("M-{c}"))),
        KeyCode::Char(c) => return Some(TmuxKey::Text(c.to_string())),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Escape".to_string(),
        KeyCode::Backspace => "BSpace".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => return Some(TmuxKey::Name(format!("{alt_}BTab"))),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PPage".to_string(),
        KeyCode::PageDown => "NPage".to_string(),
        KeyCode::Delete => "DC".to_string(),
        KeyCode::Insert => "IC".to_string(),
        KeyCode::F(n) if (1..=12).contains(&n) => format!("F{n}"),
        _ => return None,
    };
    let prefix = format!("{}{alt_}{}", if ctrl { "C-" } else { "" }, if shift { "S-" } else { "" });
    Some(TmuxKey::Name(format!("{prefix}{name}")))
}

/// The keys of a US keyboard that type two characters: unshifted, shifted.
const US: &[(&str, char, char)] = &[
    ("backquote", '`', '~'),
    ("digit_1", '1', '!'),
    ("digit_2", '2', '@'),
    ("digit_3", '3', '#'),
    ("digit_4", '4', '$'),
    ("digit_5", '5', '%'),
    ("digit_6", '6', '^'),
    ("digit_7", '7', '&'),
    ("digit_8", '8', '*'),
    ("digit_9", '9', '('),
    ("digit_0", '0', ')'),
    ("minus", '-', '_'),
    ("equal", '=', '+'),
    ("bracket_left", '[', '{'),
    ("bracket_right", ']', '}'),
    ("backslash", '\\', '|'),
    ("semicolon", ';', ':'),
    ("quote", '\'', '"'),
    ("comma", ',', '<'),
    ("period", '.', '>'),
    ("slash", '/', '?'),
    ("space", ' ', ' '),
];

/// KEY as a Rex key event's payload (block.event's `key`, as pressed): the
/// physical key as a US keyboard has it (another character is
/// `unidentified`, with its text), its modifiers and its text. None for
/// keys it has no name for.
pub fn rex(key: &KeyEvent) -> Option<Value> {
    let m = key.modifiers;
    // Hyper and meta: no key Rex knows.
    if m.intersects(KeyModifiers::HYPER | KeyModifiers::META) {
        return None;
    }
    let (ctrl, alt, sup) = (m.contains(KeyModifiers::CONTROL), m.contains(KeyModifiers::ALT), m.contains(KeyModifiers::SUPER));
    let mut shift = m.contains(KeyModifiers::SHIFT);
    let (name, text, unshifted, consumed) = match key.code {
        KeyCode::Char(c) => {
            // The key, the character it types unshifted, and shifted.
            let (name, plain, up) = if c.is_ascii_alphabetic() {
                (format!("key_{}", c.to_ascii_lowercase()), c.to_ascii_lowercase(), c.to_ascii_uppercase())
            } else if let Some((name, plain, up)) = US.iter().find(|(_, plain, up)| c == *plain || (c == *up && c != ' ')) {
                (name.to_string(), *plain, *up)
            } else {
                ("unidentified".to_string(), c, c)
            };
            // Shift held, or in the character (alt+shift+, comes as alt,
            // shift and ",", without kitty's alternate keys: it types "<").
            let shifted = up != plain && (c == up || shift);
            shift |= shifted;
            // A control character types nothing; its key and ctrl say it.
            let text = if ctrl { String::new() } else if shifted { up.to_string() } else { c.to_string() };
            (name, text, u32::from(plain), if shifted { "shift" } else { "" })
        }
        KeyCode::Enter => ("enter".into(), String::new(), 0, ""),
        KeyCode::Esc => ("escape".into(), String::new(), 0, ""),
        KeyCode::Backspace => ("backspace".into(), String::new(), 0, ""),
        KeyCode::Tab => ("tab".into(), String::new(), 0, ""),
        KeyCode::BackTab => {
            shift = true;
            ("tab".into(), String::new(), 0, "")
        }
        KeyCode::Up => ("arrow_up".into(), String::new(), 0, ""),
        KeyCode::Down => ("arrow_down".into(), String::new(), 0, ""),
        KeyCode::Left => ("arrow_left".into(), String::new(), 0, ""),
        KeyCode::Right => ("arrow_right".into(), String::new(), 0, ""),
        KeyCode::Home => ("home".into(), String::new(), 0, ""),
        KeyCode::End => ("end".into(), String::new(), 0, ""),
        KeyCode::PageUp => ("page_up".into(), String::new(), 0, ""),
        KeyCode::PageDown => ("page_down".into(), String::new(), 0, ""),
        KeyCode::Delete => ("delete".into(), String::new(), 0, ""),
        KeyCode::Insert => ("insert".into(), String::new(), 0, ""),
        KeyCode::F(n @ 1..=24) => (format!("f{n}"), String::new(), 0, ""),
        _ => return None,
    };
    // In Rex's order: shift, ctrl, alt, super.
    let mods: Vec<&str> = [(shift, "shift"), (ctrl, "ctrl"), (alt, "alt"), (sup, "super")].into_iter().filter(|(on, _)| *on).map(|(_, m)| m).collect();
    Some(json!({
        "key": name,
        "mods": mods.join("+"),
        "consumed_mods": consumed,
        "text": text,
        "unshifted_codepoint": unshifted,
        "composing": false,
        "action": "press",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }
    const N: KeyModifiers = KeyModifiers::NONE;
    const C: KeyModifiers = KeyModifiers::CONTROL;
    const A: KeyModifiers = KeyModifiers::ALT;
    const S: KeyModifiers = KeyModifiers::SHIFT;

    fn name(s: &str) -> Option<TmuxKey> {
        Some(TmuxKey::Name(s.into()))
    }

    #[test]
    fn keys_as_tmux_names_them() {
        assert_eq!(tmux(&k(KeyCode::Char('y'), N)), Some(TmuxKey::Text("y".into())));
        assert_eq!(tmux(&k(KeyCode::Char('Y'), S)), Some(TmuxKey::Text("Y".into())), "shift is in the character");
        assert_eq!(tmux(&k(KeyCode::Char('c'), C)), name("C-c"));
        assert_eq!(tmux(&k(KeyCode::Char('x'), C | A)), name("C-M-x"));
        assert_eq!(tmux(&k(KeyCode::Char('f'), A)), name("M-f"));
        assert_eq!(tmux(&k(KeyCode::Char('4'), C)), name("C-\\"));
        assert_eq!(tmux(&k(KeyCode::Char('7'), C)), name("C-_"));
        assert_eq!(tmux(&k(KeyCode::Char(' '), C)), name("C-Space"));
        assert_eq!(tmux(&k(KeyCode::Enter, N)), name("Enter"));
        assert_eq!(tmux(&k(KeyCode::Enter, A)), name("M-Enter"));
        assert_eq!(tmux(&k(KeyCode::Esc, N)), name("Escape"));
        assert_eq!(tmux(&k(KeyCode::Up, S)), name("S-Up"));
        assert_eq!(tmux(&k(KeyCode::Left, C)), name("C-Left"));
        assert_eq!(tmux(&k(KeyCode::BackTab, S)), name("BTab"));
        assert_eq!(tmux(&k(KeyCode::PageDown, N)), name("NPage"));
        assert_eq!(tmux(&k(KeyCode::F(5), N)), name("F5"));
        assert_eq!(tmux(&k(KeyCode::Char('é'), C)), None);
    }

    #[test]
    fn keys_as_rex_key_events() {
        let ev = |code, m| {
            let v = rex(&k(code, m)).unwrap();
            (v["key"].as_str().unwrap().to_string(), v["mods"].as_str().unwrap().to_string(), v["text"].as_str().unwrap().to_string(), v["unshifted_codepoint"].as_u64().unwrap(), v["consumed_mods"].as_str().unwrap().to_string())
        };
        let e = |key: &str, mods: &str, text: &str, cp: u64, consumed: &str| (key.to_string(), mods.to_string(), text.to_string(), cp, consumed.to_string());
        assert_eq!(ev(KeyCode::Enter, S), e("enter", "shift", "", 0, ""), "Shift+Enter, a new line in an agent's prompt");
        assert_eq!(ev(KeyCode::Char('c'), C), e("key_c", "ctrl", "", 99, ""));
        assert_eq!(ev(KeyCode::Char('b'), A), e("key_b", "alt", "b", 98, ""));
        assert_eq!(ev(KeyCode::Char('A'), S), e("key_a", "shift", "A", 97, "shift"));
        assert_eq!(ev(KeyCode::Char('?'), N), e("slash", "shift", "?", 47, "shift"), "shifted on a US keyboard");
        assert_eq!(ev(KeyCode::Char(']'), C), e("bracket_right", "ctrl", "", 93, ""));
        assert_eq!(ev(KeyCode::Char('é'), N), e("unidentified", "", "é", 233, ""));
        assert_eq!(ev(KeyCode::Char(' '), N), e("space", "", " ", 32, ""));
        assert_eq!(ev(KeyCode::BackTab, S), e("tab", "shift", "", 0, ""));
        assert_eq!(ev(KeyCode::Up, C | A), e("arrow_up", "ctrl+alt", "", 0, ""));
        assert_eq!(ev(KeyCode::F(5), N), e("f5", "", "", 0, ""));
        assert_eq!(ev(KeyCode::Char(','), A | S), e("comma", "shift+alt", "<", 44, "shift"), "alt+shift+, types <");
        assert_eq!(ev(KeyCode::Char('b'), A | S), e("key_b", "shift+alt", "B", 98, "shift"));
        assert_eq!(ev(KeyCode::Char('j'), KeyModifiers::SUPER), e("key_j", "super", "j", 106, ""));
        assert_eq!(rex(&k(KeyCode::Char('j'), KeyModifiers::HYPER)), None);
        assert_eq!(rex(&k(KeyCode::Null, N)), None);
    }

    #[test]
    fn ctrl_close_bracket_leaves_as_crossterm_reports_it() {
        assert!(is_leave(&k(KeyCode::Char('5'), C)));
        assert!(is_leave(&k(KeyCode::Char(']'), C)));
        assert!(is_leave(&k(KeyCode::Char('h'), C)), "back from ^l");
        assert!(!is_leave(&k(KeyCode::Backspace, N)));
        assert!(!is_leave(&k(KeyCode::Char(']'), N)));
        assert!(!is_leave(&k(KeyCode::Esc, N)));
    }
}

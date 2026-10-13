//! A one-line text field: the filter query and the rename field. Editing is
//! by grapheme (Backspace removes a whole 👍🏽 or é), with readline's keys.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineEdit {
    text: String,
    /// Byte offset of the cursor, always on a grapheme boundary.
    cursor: usize,
    /// At most this many graphemes (None: no limit).
    max: Option<usize>,
}

/// What a key did to the field.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edited {
    /// The text changed.
    Text,
    /// Only the cursor moved.
    Cursor,
    /// Not an editing key.
    No,
}

impl LineEdit {
    /// TEXT with the cursor at its end.
    pub fn new(text: &str, max: Option<usize>) -> LineEdit {
        let mut field = LineEdit { text: String::new(), cursor: 0, max };
        field.insert(text);
        field
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Replace the text, cursor at the end.
    pub fn set(&mut self, text: &str) {
        self.text.clear();
        self.cursor = 0;
        self.insert(text);
    }

    /// Insert TEXT at the cursor, without control characters, up to the
    /// limit. True when anything was inserted.
    pub fn insert(&mut self, text: &str) -> bool {
        let room = self.max.map_or(usize::MAX, |m| m.saturating_sub(self.text.graphemes(true).count()));
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        let add: String = clean.graphemes(true).take(room).collect();
        self.text.insert_str(self.cursor, &add);
        self.cursor += add.len();
        !add.is_empty()
    }

    fn prev(&self) -> usize {
        self.text[..self.cursor].grapheme_indices(true).next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.text[self.cursor..].graphemes(true).next().map_or(self.cursor, |g| self.cursor + g.len())
    }

    /// The start of the word before the cursor (readline's ^w: back over
    /// blanks, then over the word).
    fn word_start(&self) -> usize {
        let before = &self.text[..self.cursor];
        let end = before.trim_end().len();
        before[..end].rfind(char::is_whitespace).map_or(0, |i| i + before[i..].chars().next().map_or(1, char::len_utf8))
    }

    fn remove(&mut self, from: usize, to: usize) -> Edited {
        if from == to {
            return Edited::No;
        }
        self.text.replace_range(from..to, "");
        self.cursor = from;
        Edited::Text
    }

    fn go(&mut self, to: usize) -> Edited {
        if to == self.cursor {
            return Edited::No;
        }
        self.cursor = to;
        Edited::Cursor
    }

    /// Apply KEY: text, Backspace (or ^h), Delete, ←/→, Home/End (or ^a/^e),
    /// ^w (word back), ^u (to the start). Keys that change nothing (Backspace
    /// at the start) count as handled but report No.
    pub fn key(&mut self, key: KeyEvent) -> Edited {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Char(c) if plain => {
                if self.insert(c.encode_utf8(&mut [0; 4])) { Edited::Text } else { Edited::No }
            }
            KeyCode::Backspace | KeyCode::Char('h') if ctrl || key.code == KeyCode::Backspace => self.remove(self.prev(), self.cursor),
            KeyCode::Delete => self.remove(self.cursor, self.next()),
            KeyCode::Char('w') if ctrl => self.remove(self.word_start(), self.cursor),
            KeyCode::Char('u') if ctrl => self.remove(0, self.cursor),
            KeyCode::Left => self.go(self.prev()),
            KeyCode::Right => self.go(self.next()),
            KeyCode::Home => self.go(0),
            KeyCode::Char('a') if ctrl => self.go(0),
            KeyCode::End => self.go(self.text.len()),
            KeyCode::Char('e') if ctrl => self.go(self.text.len()),
            _ => Edited::No,
        }
    }

    /// The part of the text to show in WIDTH columns, keeping the cursor in
    /// view: (text before the cursor, from the cursor on), each already cut,
    /// with … where text is hidden. One column is left for the cursor.
    pub fn window(&self, width: usize) -> (String, String) {
        let (before, after) = self.text.split_at(self.cursor);
        if self.text.width() < width {
            return (before.to_string(), after.to_string());
        }
        let room = width.saturating_sub(1);
        // Show as much after the cursor as fits beside what comes before it,
        // giving the text before the cursor priority (it is what is typed).
        let mut left = String::new();
        let mut used = 0;
        let parts: Vec<&str> = before.graphemes(true).collect();
        let cut_left = before.width() > room;
        let budget = if cut_left { room.saturating_sub(1) } else { room };
        for g in parts.iter().rev() {
            if used + g.width() > budget {
                break;
            }
            used += g.width();
            left.insert_str(0, g);
        }
        if cut_left {
            left.insert(0, '…');
            used += 1;
        }
        let mut right = String::new();
        let mut rest = room.saturating_sub(used);
        let mut graphemes = after.graphemes(true).peekable();
        while let Some(g) = graphemes.next() {
            let last = graphemes.peek().is_none();
            if g.width() + usize::from(!last) > rest {
                right.push('…');
                break;
            }
            rest -= g.width();
            right.push_str(g);
        }
        (left, right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent { code, modifiers, kind: KeyEventKind::Press, state: KeyEventState::NONE }
    }
    fn press(f: &mut LineEdit, code: KeyCode) -> Edited {
        f.key(key(code, KeyModifiers::NONE))
    }
    fn ctrl(f: &mut LineEdit, c: char) -> Edited {
        f.key(key(KeyCode::Char(c), KeyModifiers::CONTROL))
    }
    /// The text with | at the cursor.
    fn shown(f: &LineEdit) -> String {
        format!("{}|{}", &f.text[..f.cursor], &f.text[f.cursor..])
    }

    #[test]
    fn edits_by_grapheme() {
        let mut f = LineEdit::new("ok 👍🏽 e\u{301}", None);
        assert_eq!(press(&mut f, KeyCode::Backspace), Edited::Text);
        assert_eq!(shown(&f), "ok 👍🏽 |", "a decomposed é goes at once");
        press(&mut f, KeyCode::Left);
        press(&mut f, KeyCode::Left);
        assert_eq!(shown(&f), "ok |👍🏽 ");
        assert_eq!(press(&mut f, KeyCode::Delete), Edited::Text);
        assert_eq!(shown(&f), "ok | ", "the whole emoji with its skin tone");
        assert_eq!(ctrl(&mut f, 'h'), Edited::Text, "^h is Backspace");
        assert_eq!(shown(&f), "ok| ");
    }

    #[test]
    fn readline_keys() {
        let mut f = LineEdit::new("fix the  cart page", None);
        assert_eq!(ctrl(&mut f, 'w'), Edited::Text);
        assert_eq!(shown(&f), "fix the  cart |");
        assert_eq!(ctrl(&mut f, 'w'), Edited::Text);
        assert_eq!(shown(&f), "fix the  |");
        assert_eq!(ctrl(&mut f, 'w'), Edited::Text, "blanks, then the word");
        assert_eq!(shown(&f), "fix |");
        assert_eq!(ctrl(&mut f, 'a'), Edited::Cursor);
        assert_eq!(ctrl(&mut f, 'a'), Edited::No, "already there");
        assert_eq!(press(&mut f, KeyCode::Backspace), Edited::No, "nothing before the cursor");
        f.key(key(KeyCode::Char('X'), KeyModifiers::SHIFT));
        assert_eq!(shown(&f), "X|fix ", "shifted letters are text");
        assert_eq!(ctrl(&mut f, 'e'), Edited::Cursor);
        assert_eq!(ctrl(&mut f, 'u'), Edited::Text);
        assert_eq!(shown(&f), "|");
        assert_eq!(ctrl(&mut f, 'r'), Edited::No, "not an editing key");
    }

    #[test]
    fn keeps_to_its_limit_and_drops_control_characters() {
        let mut f = LineEdit::new("abcdef", Some(4));
        assert_eq!(f.text(), "abcd");
        assert_eq!(press(&mut f, KeyCode::Char('x')), Edited::No);
        f.set("a\u{1b}[31mb\n");
        assert_eq!(f.text(), "a[31", "control characters dropped, then cut");
    }

    #[test]
    fn the_window_follows_the_cursor() {
        let f = LineEdit::new("migrate the cart page", None);
        assert_eq!(f.window(40), ("migrate the cart page".into(), "".into()));
        assert_eq!(f.window(10), ("…art page".into(), "".into()), "typing at the end shows the end");
        let mut f = f;
        ctrl(&mut f, 'a');
        assert_eq!(f.window(10), ("".into(), "migrate …".into()), "at the start, the start");
        for _ in 0..12 {
            press(&mut f, KeyCode::Right);
        }
        let (l, r) = f.window(10);
        assert_eq!(format!("{l}|{r}"), "…ate the |…", "what is before the cursor first");
        let f = LineEdit::new("日本語テキスト", None);
        assert_eq!(f.window(7), ("…スト".into(), "".into()), "by display width");
    }
}

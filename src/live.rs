//! Live preview of one pane. tmux streams pane output (%output) only for
//! the control client's own session, and following the previewed pane there
//! would fire focus hooks on it, so the pane is captured instead: a capture
//! is ~0.2 ms over the control connection. While the pane changes it is
//! captured at ~30 fps; while it is quiet the interval backs off to 250 ms.
//! Only a changed screen is re-parsed and redrawn.

use std::time::{Duration, Instant};

use ansi_to_tui::IntoText;
use ratatui::text::Text;

use crate::tmux::{quote, Control};

const FAST: Duration = Duration::from_millis(33);
const SLOW: Duration = Duration::from_millis(250);
/// Unchanged captures before backing off.
const QUIET: u32 = 15;

pub struct Live {
    pub pane: Option<String>,
    pub body: Text<'static>,
    raw: Vec<String>,
    interval: Duration,
    quiet: u32,
    next: Instant,
}

impl Live {
    pub fn new() -> Live {
        Live { pane: None, body: Text::default(), raw: Vec::new(), interval: FAST, quiet: 0, next: Instant::now() }
    }

    /// Follow PANE (None: nothing to show); a new pane is captured at once.
    pub fn follow(&mut self, pane: Option<&str>) {
        if pane != self.pane.as_deref() {
            self.pane = pane.map(String::from);
            self.raw.clear();
            self.body = Text::default();
            self.wake();
        }
    }

    /// Capture soon at full speed (selection changed, pane may have changed).
    pub fn wake(&mut self) {
        self.interval = FAST;
        self.quiet = 0;
        self.next = Instant::now();
    }

    /// When the next capture is due, if there is a pane to capture.
    pub fn due(&self) -> Option<Instant> {
        self.pane.as_ref().map(|_| self.next)
    }

    /// Capture if due; true when the screen changed.
    pub fn tick(&mut self, tmux: &mut Control) -> bool {
        let Some(pane) = &self.pane else { return false };
        if Instant::now() < self.next {
            return false;
        }
        let lines = match tmux.cmd(&format!("capture-pane -p -e -t {}", quote(pane))) {
            Ok(lines) => lines,
            Err(_) => vec!["\x1b[2mpane is gone\x1b[0m".into()],
        };
        let changed = lines != self.raw;
        if changed {
            self.body = parse(&lines);
            self.raw = lines;
            self.interval = FAST;
            self.quiet = 0;
        } else {
            self.quiet += 1;
            if self.quiet >= QUIET {
                self.interval = (self.interval * 2).min(SLOW);
            }
        }
        self.next = Instant::now() + self.interval;
        changed
    }
}

/// Styled text of captured LINES, without the screen's trailing blank rows.
fn parse(lines: &[String]) -> Text<'static> {
    let joined: Vec<String> = lines.iter().map(|l| strip_osc(l)).collect();
    let mut text = joined.join("\n").into_text().unwrap_or_default();
    while text.lines.last().is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty())) {
        text.lines.pop();
    }
    text
}

/// S without OSC sequences (ESC ] ... ended by BEL or ESC \). tmux ends
/// hyperlinks (OSC 8) with ESC \, which ansi-to-tui does not recognise: it
/// would swallow the rest of the line.
fn strip_osc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("\x1b]") {
        out.push_str(&rest[..i]);
        let osc = &rest[i + 2..];
        let end = [osc.find('\x07').map(|j| j + 1), osc.find("\x1b\\").map(|j| j + 2)].into_iter().flatten().min();
        rest = end.map_or("", |j| &osc[j..]);
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyperlinks_keep_the_rest_of_the_line() {
        assert_eq!(strip_osc("a \x1b]8;;file:///x\x1b\\link\x1b]8;;\x1b\\ b"), "a link b");
        assert_eq!(strip_osc("\x1b]0;title\x07text"), "text");
        assert_eq!(strip_osc("plain \x1b[31mred"), "plain \x1b[31mred");
        let text = parse(&["\x1b]8;;u\x1b\\Edited\x1b]8;;\x1b\\ src/foo.rs".into()]);
        assert_eq!(text.lines[0].to_string(), "Edited src/foo.rs");
    }

    #[test]
    fn trailing_blank_rows_are_dropped() {
        let text = parse(&["top".into(), "\x1b[0m".into(), "   ".into()]);
        assert_eq!(text.lines.len(), 1);
    }
}

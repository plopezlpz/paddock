//! Live preview of one pane, captured through the backend. (tmux streams
//! pane output (%output) only for the control client's own session, and
//! following the previewed pane there would fire focus hooks on it, so the
//! pane is captured instead, over tmux's control connection: ~0.2 ms for a
//! full screen, a few tenths more for a short one topped up from its
//! history. Rex builds a pane's whole scrollback for each capture, from
//! under a millisecond to tens of ms on an agent with a long history, so
//! its captures run on the server's own thread, one at a time, and wake the
//! loop when they land: the popup never waits for one.)
//!
//! A pane is captured at ~30 fps while it changes, backing off once it has
//! been quiet for a while to 250 ms, or 40 captures' time (3 s at most)
//! for a slow one,
//! and never more often than leaves the server free 4/5 of the time. A new
//! pane is captured at once (on tmux before the draw, on Rex as soon as the
//! capture under way is in), unless the selection came to it within a
//! capture's time of the last move: then once it has rested there half as
//! long again as that move took (33 ms at least, a capture's time at most),
//! so a held key captures none of the panes it passes over (a Rex capture
//! can take 100 ms). A pane shown before shows its last screen at once, and
//! is captured again no sooner than it would have been had the selection
//! stayed on it. Only a changed screen is re-parsed. A failed capture keeps the last
//! screen (with none, it shows why) and is tried again after 250 ms.

use std::collections::HashMap;
use std::mem;
use std::time::{Duration, Instant};

use ansi_to_tui::IntoText;
use ratatui::text::Text;

use crate::backend::Backend;

const FAST: Duration = Duration::from_millis(33);
const SLOW: Duration = Duration::from_millis(250);
/// How long a screen stays the same before captures back off.
const QUIET: Duration = Duration::from_millis(500);
/// A quiet pane's interval, in captures' time, when that is over SLOW.
const QUIET_SPACING: u32 = 40;
/// The longest a quiet pane waits, however slow its captures.
const QUIETEST: Duration = Duration::from_secs(3);
/// A capture's interval, in captures' time: the server is free 4/5 of it.
const SPACING: u32 = 5;
/// Screens kept for panes no longer shown.
const KEEP: usize = 64;

struct Screen {
    raw: Vec<String>,
    body: Text<'static>,
    /// When it was captured, and what that cost.
    captured: Instant,
    cost: Duration,
}

pub struct Live {
    pub pane: Option<String>,
    pub body: Text<'static>,
    raw: Vec<String>,
    interval: Duration,
    /// How long the last capture took.
    cost: Duration,
    /// When the screen shown was captured, and what that cost.
    captured: Instant,
    captured_cost: Duration,
    /// When the selection last moved.
    moved: Instant,
    /// When the screen last changed.
    changed: Instant,
    next: Instant,
    /// A capture was asked for and is not in yet.
    asked: bool,
    /// Panes shown before, as last captured.
    seen: HashMap<String, Screen>,
}

impl Live {
    pub fn new() -> Live {
        let now = Instant::now();
        Live { pane: None, body: Text::default(), raw: Vec::new(), interval: FAST, cost: Duration::ZERO, captured: now, captured_cost: Duration::ZERO, moved: now, changed: now, next: now, asked: false, seen: HashMap::new() }
    }

    /// Follow PANE (None: nothing to show). A new pane shows its last
    /// screen, if it had one, and is captured at once, or once the
    /// selection rests if it is moving faster than captures take.
    pub fn follow(&mut self, pane: Option<&str>) {
        if pane == self.pane.as_deref() {
            return;
        }
        let now = Instant::now();
        if let Some(old) = self.pane.take()
            && !self.raw.is_empty()
        {
            if self.seen.len() >= KEEP
                && let Some(oldest) = self.seen.iter().min_by_key(|(_, s)| s.captured).map(|(p, _)| p.clone())
            {
                self.seen.remove(&oldest);
            }
            self.seen.insert(old, Screen { raw: mem::take(&mut self.raw), body: mem::take(&mut self.body), captured: self.captured, cost: self.captured_cost });
        }
        let gap = now - mem::replace(&mut self.moved, now);
        let rest = if gap < self.cost { (gap * 3 / 2).max(FAST).min(self.cost) } else { Duration::ZERO };
        self.next = now + rest;
        self.raw.clear();
        self.body = Text::default();
        if let Some(screen) = pane.and_then(|p| self.seen.remove(p)) {
            // Shown at once; captured again no sooner than it would have
            // been had the selection stayed on it.
            (self.raw, self.body, self.captured, self.captured_cost) = (screen.raw, screen.body, screen.captured, screen.cost);
            self.next = self.next.max(screen.captured + screen.cost * SPACING);
        }
        self.pane = pane.map(String::from);
        self.interval = FAST;
        self.changed = now;
        self.asked = false;
    }

    /// When the next capture is due, if there is a pane to capture.
    pub fn due(&self) -> Option<Instant> {
        self.pane.as_ref().map(|_| self.next)
    }

    /// Capture if due, or take the capture asked for if it is in; true when
    /// the screen changed.
    pub fn tick(&mut self, backend: &mut dyn Backend) -> bool {
        let Some(pane) = &self.pane else { return false };
        if !self.asked && Instant::now() < self.next {
            return false;
        }
        let Some((lines, cost)) = backend.capture(pane) else {
            // Its arrival wakes the loop; look again now and then all the same.
            self.asked = true;
            self.next = Instant::now() + SLOW;
            return false;
        };
        self.asked = false;
        let now = Instant::now();
        let lines = match lines {
            Ok(lines) => lines,
            // No screen: the last stays, else the error shows. It is tried
            // again soon, and its time is no capture's cost.
            Err(e) => {
                self.next = now + SLOW;
                if !self.raw.is_empty() {
                    return false;
                }
                self.raw = vec![format!("\x1b[2m{e}\x1b[0m")];
                self.body = parse(&self.raw);
                return true;
            }
        };
        self.cost = cost;
        (self.captured, self.captured_cost) = (now, cost);
        let changed = lines != self.raw;
        if changed {
            self.body = parse(&lines);
            self.raw = lines;
            self.interval = FAST;
            self.changed = now;
        } else if now - self.changed >= QUIET {
            self.interval = (self.interval * 2).min(SLOW.max((cost * QUIET_SPACING).min(QUIETEST)));
        }
        self.interval = self.interval.max(cost * SPACING);
        self.next = now + self.interval;
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
    use crate::model::{Op, Tab};
    use crate::backend::Ran;
    use crate::model::{Snapshot, Target};

    /// A backend whose captures take COST and come in when READY: at once,
    /// as tmux's, or later, as Rex's.
    struct Fake {
        screen: Result<Vec<String>, String>,
        cost: Duration,
        ready: bool,
        asked: u32,
    }

    impl Backend for Fake {
        fn fetch(&mut self) -> Result<Snapshot, String> {
            Ok(Snapshot::new(Vec::new()))
        }
        fn is_dead(&self) -> bool {
            false
        }
        fn capture(&mut self, _: &str) -> Option<(Result<Vec<String>, String>, Duration)> {
            self.asked += 1;
            self.ready.then(|| (self.screen.clone(), self.cost))
        }
        fn run(&mut self, _: &Op) -> Ran {
            Ran { moved: Vec::new(), result: Ok(None), unsure: false }
        }
        fn closed(&mut self, _: Tab) {}
        fn activate(&mut self, _: &Target) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn a_capture_that_comes_later_is_taken_when_it_does() {
        let mut fake = Fake { screen: Ok(vec!["hello".into()]), cost: Duration::from_millis(80), ready: false, asked: 0 };
        let mut live = Live::new();
        live.follow(Some("block:1"));
        assert!(!live.tick(&mut fake), "asked for, not in");
        assert!(!live.tick(&mut fake), "still under way: asked again, not due");
        fake.ready = true;
        assert!(live.tick(&mut fake), "in: shown at once, though not due");
        assert_eq!(live.body.lines[0].to_string(), "hello");
        assert_eq!(fake.asked, 3);
        // Spaced by its cost from then on.
        assert!(!live.tick(&mut fake));
        assert_eq!(fake.asked, 3);
        assert!(live.due().unwrap() >= Instant::now() + Duration::from_millis(350));
    }

    #[test]
    fn a_pane_come_back_to_shows_its_screen_and_waits_its_turn() {
        let mut fake = Fake { screen: Ok(vec!["x".into()]), cost: Duration::from_millis(100), ready: true, asked: 0 };
        let mut live = Live::new();
        live.follow(Some("a"));
        assert!(live.tick(&mut fake));
        live.moved -= Duration::from_millis(200);
        live.follow(Some("b"));
        live.moved -= Duration::from_millis(200);
        live.follow(Some("a"));
        assert_eq!(live.body.lines.len(), 1, "its last screen, at once");
        assert!(!live.tick(&mut fake), "captured moments ago: not again yet");
        assert_eq!(fake.asked, 1);
        assert!(live.due().unwrap() > Instant::now() + Duration::from_millis(300));
    }

    #[test]
    fn a_held_key_captures_none_of_the_panes_it_passes() {
        let mut fake = Fake { screen: Ok(vec!["x".into()]), cost: Duration::from_millis(100), ready: true, asked: 0 };
        let mut live = Live::new();
        live.follow(Some("a"));
        assert!(live.tick(&mut fake), "a first visit: at once");
        for pane in ["b", "c", "d"] {
            live.follow(Some(pane));
            assert!(!live.tick(&mut fake));
        }
        assert_eq!(fake.asked, 1);
        assert!(live.due().unwrap() <= Instant::now() + Duration::from_millis(100), "once it rests");
        // A move more than a capture's time after the last: at once.
        live.moved -= Duration::from_millis(100);
        fake.cost = Duration::from_micros(200);
        live.follow(Some("e"));
        assert!(live.tick(&mut fake));
        // Cheap captures (tmux's) do not wait for a key a frame later.
        live.moved -= Duration::from_millis(1);
        live.follow(Some("f"));
        assert!(live.tick(&mut fake));
    }

    #[test]
    fn a_quiet_pane_is_captured_less_often_a_slow_one_less_still() {
        for (cost, cap) in [(Duration::from_micros(200), SLOW), (Duration::from_millis(40), Duration::from_millis(1600)), (Duration::from_millis(200), QUIETEST)] {
            let mut fake = Fake { screen: Ok(vec!["same".into()]), cost, ready: true, asked: 0 };
            let mut live = Live::new();
            live.follow(Some("%1"));
            assert!(live.tick(&mut fake));
            for _ in 0..10 {
                live.changed = Instant::now() - QUIET;
                live.next = Instant::now();
                assert!(!live.tick(&mut fake));
            }
            assert_eq!(live.interval, cap);
            // A change: fast again (as its cost allows).
            fake.screen = Ok(vec!["new".into()]);
            live.next = Instant::now();
            assert!(live.tick(&mut fake));
            assert_eq!(live.interval, FAST.max(cost * SPACING));
        }
    }

    #[test]
    fn a_failed_capture_keeps_the_last_screen_and_is_tried_again_soon() {
        let mut fake = Fake { screen: Err("rex stopped answering".into()), cost: Duration::from_secs(5), ready: true, asked: 0 };
        let mut live = Live::new();
        live.follow(Some("block:1"));
        assert!(live.tick(&mut fake), "no screen yet: the error shows");
        assert_eq!(live.body.lines[0].to_string(), "rex stopped answering");
        assert!(live.due().unwrap() <= Instant::now() + SLOW, "its 5 s are no capture's cost");
        fake.screen = Ok(vec!["hello".into()]);
        fake.cost = Duration::from_millis(1);
        live.next = Instant::now();
        assert!(live.tick(&mut fake));
        fake.screen = Err("rex stopped answering".into());
        fake.cost = Duration::from_secs(5);
        live.next = Instant::now();
        assert!(!live.tick(&mut fake));
        assert_eq!(live.body.lines[0].to_string(), "hello", "the last screen stays");
        assert!(live.due().unwrap() <= Instant::now() + SLOW);
        fake.screen = Ok(vec!["back".into()]);
        live.next = Instant::now();
        assert!(live.tick(&mut fake), "back as soon as it answers");
    }

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

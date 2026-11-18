//! Moving windows and panes: swapping one with its neighbour, carrying one
//! to drop after another row, and how each is described, confirmed when it
//! closes something, and undone. The plans (`shift_*`, `drop_job`) are pure
//! over the snapshot and the caps; the rest acts on them.

use super::{Action, App, Confirm, Job, Mode, Tab};
use crate::model::{Op, Pane, Session, Target, Window};

/// A window or pane picked up, to drop after the selected row.
#[derive(Debug)]
pub struct Carry {
    pub target: Target,
    /// "window api", "pane webshop:1.1".
    pub label: String,
    /// Picked up by dragging: the release drops it.
    pub(super) dragging: bool,
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

impl App {
    /// TARGET in words, and what it holds: ("window webshop:2 web",
    /// "2 panes · codex waiting").
    pub(super) fn describe(&self, target: &Target) -> (String, String) {
        let with = |count: String, agents: String| if agents.is_empty() { count } else { format!("{count} · {agents}") };
        let plural = |n: usize, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
        let win = self.caps.window();
        match target {
            Target::Session { session } => match self.snap.session(session) {
                Some(s) => (format!("session {}", s.name), with(plural(s.windows.len(), win), agents_in(s.windows.iter().flat_map(|w| &w.panes)))),
                None => ("session".into(), String::new()),
            },
            Target::Window { session, window } => match self.window_at(session, window) {
                Some((_, s, _, w)) => {
                    (format!("{win} {}:{} {}", s.name, w.index, w.name), with(plural(w.panes.len(), "pane"), agents_in(w.panes.iter())))
                }
                None => (win.into(), String::new()),
            },
            Target::Pane { pane, .. } => match self.snap.pane(pane) {
                Some((s, w, p)) => (format!("pane {}:{}.{}", s.name, w.index, p.index), agents_in(std::iter::once(p))),
                None => ("pane".into(), String::new()),
            },
        }
    }

    /// Run JOB now, or ask first when QUESTION says it does more than it
    /// seems (a move that closes a session).
    fn maybe_confirm(&mut self, job: Job, question: Option<String>) -> Action {
        match question {
            Some(question) => {
                self.mode = Mode::Confirm(Confirm { job, question, dropped: false });
                Action::None
            }
            None => Action::Run(job),
        }
    }

    /// Swap the selected window or pane with its neighbour; past the first or
    /// last, into the previous or next session (window) or window (pane).
    pub(super) fn shift(&mut self, down: bool) -> Action {
        if self.tab != Tab::Sessions {
            return Action::None;
        }
        let result = match self.selected_target().cloned() {
            Some(Target::Window { session, window }) => self.shift_window(&session, &window, down),
            Some(Target::Pane { session, window, pane }) => self.shift_pane(&session, &window, &pane, down),
            Some(Target::Session { .. }) => Err(if self.caps.rex { "sessions keep the order Rex lists them in" } else { "sessions are listed by name: rename one to move it" }.into()),
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
        let win = self.caps.window();
        let (si, s, wi, _) = self.window_at(session, window).ok_or_else(|| format!("that {win} has closed"))?;
        let sessions = &self.snap.sessions;
        let ws = &s.windows;
        let neighbour = if down { ws.get(wi + 1) } else { wi.checked_sub(1).map(|i| &ws[i]) };
        if let Some(n) = neighbour.filter(|n| n.id != window) {
            let op = Op::SwapWindows { session: session.into(), a: window.into(), b: n.id.clone() };
            let listed = self.position(&Target::Window { session: session.into(), window: n.id.clone() }).is_some();
            return Ok((Job::new(op.clone(), moved(down, listed, &n.name)).undo(Some(op)).follow(window), None));
        }
        if !self.caps.cross_session_moves {
            return Err(format!("already the {} {win} in its session", if down { "last" } else { "first" }));
        }
        let other = if down { sessions.get(si + 1) } else { si.checked_sub(1).map(|i| &sessions[i]) };
        let other = other.ok_or_else(|| format!("already the {} {win}", if down { "last" } else { "first" }))?;
        let to = if down { other.windows.first() } else { other.windows.last() }.ok_or_else(|| format!("that session has no {win}s"))?;
        let op = Op::MoveWindow { from: session.into(), window: window.into(), into: other.id.clone(), to: to.id.clone(), before: down };
        let job = Job::new(op, format!("moved to {}", other.name)).undo(window_return(s, wi, &other.id)).follow(window);
        let closes = (ws.len() == 1).then(|| format!("move {win} {} to {}? {} closes: it is its only {win}", ws[wi].name, other.name, s.name));
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
        let win = self.caps.window();
        if ps.len() == 1 {
            return Err(format!("it is its {win}'s only pane: move the {win}"));
        }
        let neighbour = if down { ps.get(pi + 1) } else { pi.checked_sub(1).map(|i| &ps[i]) };
        if let Some(n) = neighbour {
            let op = Op::SwapPanes(pane.into(), n.id.clone());
            let listed = self.position(&Target::Pane { session: session.into(), window: window.into(), pane: n.id.clone() }).is_some();
            return Ok((Job::new(op.clone(), moved(down, listed, &n.command)).undo(Some(op)).follow(pane), None));
        }
        // Into the next (previous) window, in tree order across sessions
        // (its own only, if panes stay in theirs).
        let sessions = self.snap.sessions.iter().filter(|s| self.caps.cross_session_moves || s.id == session);
        let windows: Vec<(&Session, &Window)> = sessions.flat_map(|s| s.windows.iter().map(move |w| (s, w))).collect();
        let here = windows.iter().position(|(s, w)| s.id == session && w.id == window).ok_or("that pane has closed")?;
        let other = if down { windows.get(here + 1) } else { here.checked_sub(1).map(|i| &windows[i]) };
        let (s, w) = other.ok_or(if down { "already the last pane" } else { "already the first pane" })?;
        let to = if down { w.panes.first() } else { w.panes.last() }.ok_or_else(|| format!("that {win} has no panes"))?;
        let op = Op::JoinPane { pane: pane.into(), to: to.id.clone(), before: down };
        Ok((Job::new(op, format!("moved to {}:{} {}", s.name, w.index, w.name)).undo(self.pane_return(session, window, pane)).follow(pane), None))
    }

    /// Pick up the selected window or pane (sessions tab).
    pub(super) fn pick_up(&mut self, dragging: bool) {
        if !self.movable() {
            if self.tab == Tab::Sessions {
                self.note(format!("pick up a {} or a pane", self.caps.window()));
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
        let win = self.caps.window();
        let at = self.selected_target().ok_or_else(|| format!("choose a session, {win} or pane"))?;
        let session = |t: &Target| match t {
            Target::Session { session } | Target::Window { session, .. } | Target::Pane { session, .. } => session.clone(),
        };
        if !self.caps.cross_session_moves && session(at) != session(&carry.target) {
            return Err("it moves only within its session".into());
        }
        match (&carry.target, at) {
            (Target::Window { session, window }, at) => {
                let (_, s, wi, _) = self.window_at(session, window).ok_or_else(|| format!("that {win} has closed"))?;
                let ws = &s.windows;
                let (into, to, before, done) = match at {
                    Target::Session { session: other } => {
                        let o = self.snap.session(other).ok_or("that session has closed")?;
                        let first = o.windows.first().ok_or_else(|| format!("that session has no {win}s"))?;
                        (other.clone(), first.id.clone(), true, format!("moved to the top of {}", o.name))
                    }
                    Target::Window { session: other, window: to } | Target::Pane { session: other, window: to, .. } => {
                        let (_, o, _, w) = self.window_at(other, to).ok_or_else(|| format!("that {win} has closed"))?;
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
                let closes = (ws.len() == 1 && leaving).then(|| format!("move {}? {} closes: it is its only {win}", carry.label, s.name));
                Ok(Some((job, closes)))
            }
            (Target::Pane { session, window, pane }, at) => {
                let undo = self.pane_return(session, window, pane);
                let (op, done) = match at {
                    Target::Session { session: other } => {
                        let name = self.snap.session(other).map(|s| s.name.clone()).unwrap_or_default();
                        (Op::BreakPane { pane: pane.clone(), session: other.clone() }, format!("a {win} of its own in {name}"))
                    }
                    Target::Window { session: other, window: to } => {
                        if to == window {
                            return Ok(None);
                        }
                        let (_, _, _, w) = self.window_at(other, to).ok_or_else(|| format!("that {win} has closed"))?;
                        let last = w.panes.last().ok_or_else(|| format!("that {win} has no panes"))?;
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
                let closes = alone.then(|| format!("move {}? its {win} closes: it is its only pane", carry.label));
                Ok(Some((Job::new(op, done).undo(undo).follow(pane), closes)))
            }
            _ => Err(format!("pick up a {win} or a pane")),
        }
    }

    /// What ⏎ would do with what is carried, said on the selected row: Ok
    /// when it would do something.
    pub fn drop_note(&self) -> Result<String, String> {
        let win = self.caps.window();
        match self.drop_job() {
            Ok(Some((job, _))) => Ok(match &job.op {
                Op::MoveWindow { before: true, .. } if matches!(self.selected_target(), Some(Target::Session { .. })) => "first in this session".to_string(),
                Op::MoveWindow { .. } => format!("after this {win}"),
                Op::JoinPane { .. } if matches!(self.selected_target(), Some(Target::Window { .. })) => format!("into this {win}"),
                Op::JoinPane { .. } => "beside this pane".to_string(),
                Op::SwapPanes(..) => "swap with this pane".to_string(),
                Op::BreakPane { .. } => format!("a new {win} here"),
                _ => "here".to_string(),
            }),
            Ok(None) => Err("already here".into()),
            Err(why) => Err(why),
        }
    }

    /// Drop what is carried after the selected row.
    pub(super) fn drop_here(&mut self) -> Action {
        match self.drop_job() {
            Ok(Some((job, question))) => {
                self.mode = Mode::List;
                let action = self.maybe_confirm(job, question);
                if let Mode::Confirm(c) = &mut self.mode {
                    c.dropped = true;
                } else {
                    // Kept no longer: moved, it is selected where it lands
                    // (follow); refused, the filter lists it only if it matches.
                    self.keep = None;
                }
                action
            }
            Ok(None) => self.put_down(Some("already there".into())),
            Err(why) => self.put_down(Some(why)),
        }
    }

    /// Put what is carried down where it was, saying WHY, if there is a
    /// reason: the filter lists it again only if it matches.
    pub(super) fn put_down(&mut self, why: Option<String>) -> Action {
        self.mode = Mode::List;
        if self.keep.take().is_some() {
            self.rebuild(false);
        }
        if let Some(why) = why {
            self.note(why);
        }
        Action::None
    }

    /// Whether the selected row is a window or a pane to move (sessions tab).
    pub(super) fn movable(&self) -> bool {
        self.tab == Tab::Sessions && matches!(self.selected_target(), Some(Target::Window { .. } | Target::Pane { .. }))
    }
}

/// What a swap with a neighbour says: the neighbour by NAME when the filter
/// leaves it unlisted, as the list alone would not show the move.
fn moved(down: bool, listed: bool, name: &str) -> String {
    match (down, listed) {
        (true, true) => "moved down".into(),
        (false, true) => "moved up".into(),
        (true, false) => format!("moved below {name}"),
        (false, false) => format!("moved above {name}"),
    }
}

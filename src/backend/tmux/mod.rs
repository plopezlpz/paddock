//! paddock on a tmux server: one control-mode connection (control.rs)
//! for the popup's lifetime. The popup binding passes the invoking client
//! and location as PADDOCK_CLIENT, PADDOCK_SESSION and PADDOCK_WINDOW;
//! paddock never guesses a client, since another terminal may become active
//! while it is open.

mod control;
mod snapshot;
pub mod workspace;

use std::process::Command;
use std::time::{Duration, Instant};

use super::{Backend, Msg, Ran, Start};
use crate::keys::{self, Input, Protocol, TmuxKey};
use crate::model::{Op, Snapshot, Tab, Target};
use crate::theme::{AgentIcons, Theme};
use control::{field, quote, Control, SEP, SESSION};
#[cfg(test)]
pub use snapshot::{fixture, fixture_hidden};

/// Global option holding the tab paddock was last closed on.
const LAST_TAB: &str = "@paddock-last-tab";
const HIDDEN_EXPANDED: &str = "@paddock-hidden-expanded";
/// Lines of text a capture tops up to from the pane's history, so the
/// preview of a short or mostly blank pane fills (it shows the end).
const FILL: usize = 48;

/// Refresh, checked by tmux about once a second: the snapshot's text, so a
/// title, state or name changing anywhere (a control client's %* covers only
/// its own session) arrives as the new snapshot. Changes to windows in
/// other sessions (added, closed, renamed) also arrive at once as
/// notifications. Each value is a `field`, so a newline in it cannot end the
/// notification line.
fn subscription() -> String {
    format!("refresh-client -B {}", quote(&format!("panes::{}", snapshot::format())))
}

struct Context {
    client: String,
    session: String,
    window: String,
}

fn context() -> Result<Context, String> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if let (Some(client), Some(session), Some(window)) = (get("PADDOCK_CLIENT"), get("PADDOCK_SESSION"), get("PADDOCK_WINDOW")) {
        return Ok(Context { client, session, window });
    }
    // Run from a shell inside tmux (development): the pane's own client.
    let out = Command::new("tmux")
        .args(["display-message", "-p", &format!("#{{client_name}}{SEP}#{{session_id}}{SEP}#{{window_id}}")])
        .output()
        .map_err(|e| format!("cannot run tmux: {e}"))?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let f: Vec<&str> = line.split(SEP).collect();
    match f.as_slice() {
        [c, s, w] if !c.is_empty() => Ok(Context { client: c.to_string(), session: s.to_string(), window: w.to_string() }),
        _ => Err("run paddock from its tmux binding (or inside tmux)".into()),
    }
}

/// TARGET as a quoted `-t` argument. The session is always named, so a
/// window linked into several sessions resolves to this one.
fn arg(target: &Target) -> String {
    quote(&plain(target))
}

/// A name or folder as a tmux argument. tmux expands formats in these
/// (`#S`, and `#(…)` would run a command), so `#` is doubled to stay literal.
fn literal(s: &str) -> String {
    quote(&s.replace('#', "##"))
}

/// A session as `refuge` sees it, live: its group, and its windows (one
/// linked at two indexes is listed twice) with their panes.
struct Place {
    id: String,
    group: String,
    /// paddock's own session.
    ours: bool,
    windows: Vec<(String, Vec<String>)>,
}

/// For `list-clients` of one client: its session, then every session as
/// `ID OURS @W%P%P@W%P GROUP` (each id starts with its own sigil).
fn places_format() -> String {
    let windows = "#{W:#{window_id}#{P:#{pane_id}}}";
    format!("#{{session_id}}#{{S:{SEP}#{{session_id}} #{{==:#{{session_name}},{SESSION}}} {windows} {}}}", field("session_group"))
}

/// The client's session and every session, from `places_format` output.
fn places(line: &str) -> (String, Vec<Place>) {
    let mut parts = line.split(SEP);
    let current = parts.next().unwrap_or_default().to_string();
    let sessions = parts
        .filter_map(|p| {
            let mut f = p.splitn(4, ' ');
            let (id, ours, windows, group) = (f.next()?, f.next()?, f.next()?, f.next()?);
            let windows = windows
                .split('@')
                .skip(1)
                .map(|w| {
                    let mut ids = w.split('%');
                    (format!("@{}", ids.next().unwrap_or_default()), ids.map(|p| format!("%{p}")).collect())
                })
                .collect();
            Some(Place { id: id.to_string(), group: group.to_string(), ours: ours == "1", windows })
        })
        .collect();
    (current, sessions)
}

/// Where the client must go before OP runs, if OP closes CURRENT, the
/// session it shows: with tmux's default detach-on-destroy it would be
/// detached. A moved window or pane is followed; otherwise any session OP
/// leaves standing (not a group peer that closes with it).
fn refuge(op: &Op, current: &str, sessions: &[Place]) -> Option<String> {
    // A pane leaving its window as its only pane closes the window in every
    // session it is linked into (kill-window too).
    let only_pane = |pane: &str| sessions.iter().flat_map(|s| &s.windows).find(|(_, panes)| panes.iter().any(|p| p == pane)).filter(|(_, panes)| panes.len() == 1);
    let in_group_of = |s: &Place, id: &str| s.id == id || sessions.iter().any(|o| o.id == id && !o.group.is_empty() && o.group == s.group);
    let closes = |s: &Place| {
        let only = |window: &str| s.windows.iter().all(|(w, _)| w == window);
        match op {
            Op::Kill(Target::Session { session }) => s.id == *session,
            Op::Kill(Target::Window { window, .. }) => only(window),
            Op::Kill(Target::Pane { pane, .. }) | Op::JoinPane { pane, .. } => only_pane(pane).is_some_and(|(w, _)| only(w)),
            Op::BreakPane { pane, session } => s.id != *session && only_pane(pane).is_some_and(|(w, _)| only(w)),
            // A window leaves FROM and its group only, once.
            Op::MoveWindow { from, window, into, .. } => from != into && s.windows.len() == 1 && only(window) && in_group_of(s, from),
            _ => false,
        }
    };
    if !closes(sessions.iter().find(|s| s.id == current)?) {
        return None;
    }
    let follow = match op {
        Op::MoveWindow { into, .. } => Some(into.as_str()),
        Op::BreakPane { session, .. } => Some(session.as_str()),
        Op::JoinPane { to, .. } => sessions.iter().find(|s| s.windows.iter().any(|(_, panes)| panes.contains(to))).map(|s| s.id.as_str()),
        _ => None,
    };
    let first = sessions.iter().filter(|s| Some(s.id.as_str()) == follow);
    first.chain(sessions).find(|s| !s.ours && !closes(s)).map(|s| s.id.clone())
}

/// Where `shelter` moved a client: out of session FROM, into TO.
struct Sheltered {
    from: String,
    to: String,
}

/// CLIENT's session and every session, now.
fn client_places(tmux: &mut Control, client: &str) -> Result<(String, Vec<Place>), String> {
    // Not display-message: its formats see paddock's own client, even with -c.
    let only = format!("#{{==:#{{client_name}},{client}}}");
    let line = tmux.cmd(&format!("list-clients -f {} -F {}", quote(&only), quote(&places_format())))?;
    Ok(places(line.first().map_or("", String::as_str)))
}

/// Before OP runs, move CLIENT out of the session it shows (now: it may
/// have moved since the popup opened) if OP closes it; where it went.
fn shelter(tmux: &mut Control, op: &Op, client: &str) -> Result<Option<Sheltered>, String> {
    if !matches!(op, Op::Kill(_) | Op::MoveWindow { .. } | Op::JoinPane { .. } | Op::BreakPane { .. }) {
        return Ok(None);
    }
    let (current, sessions) = client_places(tmux, client)?;
    let Some(to) = refuge(op, &current, &sessions) else { return Ok(None) };
    tmux.cmd(&format!("switch-client -c {} -t {}", quote(client), quote(&to)))?;
    Ok(Some(Sheltered { from: current, to }))
}

/// Where a client `shelter` moved goes back to once its op failed: the
/// session it came from, if that is still there and the client still shows
/// the refuge (CURRENT).
fn homecoming(moved: &Sheltered, current: &str, sessions: &[Place]) -> Option<String> {
    (current == moved.to && sessions.iter().any(|s| s.id == moved.from)).then(|| moved.from.clone())
}

/// After a failed op, put CLIENT back where `shelter` took it from; where
/// it went. Best effort: the op's error is the one to report.
fn unshelter(tmux: &mut Control, moved: &Sheltered, client: &str) -> Option<String> {
    let (current, sessions) = client_places(tmux, client).ok()?;
    let back = homecoming(moved, &current, &sessions)?;
    tmux.cmd(&format!("switch-client -c {} -t {}", quote(client), quote(&back))).ok()?;
    Some(back)
}

/// Make OP's change on the server; what it created, if anything. Moves and
/// swaps keep every client's current window (-d).
fn run_op(tmux: &mut Control, op: &Op) -> Result<Option<Target>, String> {
    let q = |s: &str| quote(s);
    let window_in = |session: &String, window: &String| arg(&Target::Window { session: session.clone(), window: window.clone() });
    let line = match op {
        Op::SetTitle { pane, title: Some(title) } => format!("set-option -p -t {} @agent-description {}", q(pane), q(title)),
        Op::SetTitle { pane, title: None } => format!("set-option -p -u -t {} @agent-description", q(pane)),
        Op::SetHidden { pane, hidden } => format!("set-option -p -t {} @paddock-hidden {}", q(pane), u8::from(*hidden)),
        Op::SetHiddenExpanded(expanded) => format!("set-option -g {HIDDEN_EXPANDED} {}", u8::from(*expanded)),
        Op::RenameSession { session, name } => format!("rename-session -t {} -- {}", q(session), literal(name)),
        Op::RenameWindow { window, name } if name.is_empty() => format!("set-window-option -t {} automatic-rename on", q(window)),
        Op::RenameWindow { window, name } => format!("rename-window -t {} -- {}", q(window), literal(name)),
        Op::Kill(Target::Session { session }) => format!("kill-session -t {}", q(session)),
        Op::Kill(t @ Target::Window { .. }) => format!("kill-window -t {}", arg(t)),
        Op::Kill(Target::Pane { pane, .. }) => format!("kill-pane -t {}", q(pane)),
        Op::SwapWindows { session, a, b } => format!("swap-window -d -s {} -t {}", window_in(session, a), window_in(session, b)),
        Op::SwapPanes(a, b) => format!("swap-pane -d -s {} -t {}", q(a), q(b)),
        Op::MoveWindow { from, window, into, to, before } => {
            format!("move-window -d {} -s {} -t {}", if *before { "-b" } else { "-a" }, window_in(from, window), window_in(into, to))
        }
        Op::JoinPane { pane, to, before } => {
            // Side by side in a wide pane, else one above the other (cells
            // are about twice as tall as wide).
            // Unknown (the pane went): join-pane says so.
            let size = tmux.row(to, &["pane_width", "pane_height"]);
            let n = |i: usize| size.get(i).and_then(|v| v.parse::<u32>().ok());
            let wide = matches!((n(0), n(1)), (Some(w), Some(h)) if w >= 2 * h);
            format!("join-pane -d {}{} -s {} -t {}", if *before { "-b " } else { "" }, if wide { "-h" } else { "-v" }, q(pane), q(to))
        }
        Op::BreakPane { pane, session } => format!("break-pane -d -s {} -t {}", q(pane), q(&format!("{session}:"))),
        Op::NewSession { name, dir } => {
            let id = tmux.cmd(&format!("new-session -d -P -F {} -s {} -c {}", q("#{session_id}"), literal(name), literal(dir)))?;
            return Ok(id.first().map(|session| Target::Session { session: session.clone() }));
        }
        Op::NewWindow { session, after, name, dir } => {
            // No name: tmux names it after what runs.
            let name = if name.is_empty() { String::new() } else { format!(" -n {}", literal(name)) };
            let id = tmux.cmd(&format!("new-window -d -a -P -F {} -t {}{name} -c {}", q("#{window_id}"), window_in(session, after), literal(dir)))?;
            return Ok(id.first().map(|window| Target::Window { session: session.clone(), window: window.clone() }));
        }
    };
    tmux.cmd(&line).map(|_| None)
}

/// Theme roles read from tmux, in one display-message, each over the ones
/// before it when it is a colour: the user's tmux theme, then paddock's own
/// options. E: expands values that are formats themselves (tmux 3.8's
/// default border style is a `#{?...}`).
const COLOURS: &[(&str, &str)] = &[
    ("E:@agent-color-idle", "idle"),
    ("E:@agent-color-finished", "finished"),
    ("E:@agent-color-working", "working"),
    ("E:@agent-color-asking", "asking"),
    // Its fg=.
    ("E:pane-active-border-style", "accent"),
    ("E:@paddock-color-accent", "accent"),
    ("E:@paddock-color-muted", "muted"),
    ("E:@paddock-color-hint", "hint"),
    ("E:@paddock-color-border", "border"),
    ("E:@paddock-color-idle", "idle"),
    ("E:@paddock-color-finished", "finished"),
    ("E:@paddock-color-working", "working"),
    ("E:@paddock-color-asking", "asking"),
];
/// agent-status.tmux's state marks, in State order.
const MARKS: [&str; 4] = ["@agent-icon-idle", "@agent-icon-finished", "@agent-icon-working", "@agent-icon-asking"];
/// A format, so that it can choose by client: a phone's terminal may lack
/// the Nerd Font a desktop's has.
const AGENT_ICONS: &str = "E:@paddock-agent-icons";

/// The theme from the options' values (VALUE of each name).
fn theme<'a>(value: impl Fn(&str) -> &'a str) -> Theme {
    let mut theme = Theme::default();
    for (option, role) in COLOURS {
        let v = value(option);
        theme.set(role, if option.ends_with("-style") { style_fg(v) } else { v });
    }
    theme.marks(MARKS.map(&value));
    if value(AGENT_ICONS) == "nerd" {
        theme.agent_icons = AgentIcons::Nerd;
    }
    theme
}

/// The fg= colour of a tmux style such as "bg=#202020,fg=#a2a970".
fn style_fg(style: &str) -> &str {
    style.split([',', ' ']).find_map(|part| part.trim().strip_prefix("fg=")).unwrap_or("")
}

pub struct Tmux {
    control: Control,
    /// The client paddock was opened from.
    client: String,
    /// agent-status.tmux's scripts (@agent-scripts-dir), if it is loaded.
    scripts: String,
}

/// Attach, subscribe, and read what the popup starts from. EVENTS gets the
/// connection's notifications with the replies sent before each.
pub fn connect(events: impl Fn(u64, Msg) + Send + 'static) -> Result<(Tmux, Start), String> {
    let ctx = context()?;
    let mut tmux = Control::connect(events)?;
    tmux.cmd(&subscription()).map_err(|e| format!("cannot attach to tmux session {SESSION}: {e}"))?;
    let last = tmux.cmd(&format!("show-option -gqv {LAST_TAB}")).unwrap_or_default();
    let last_tab = Tab::parse(last.first().map_or("", |l| l.trim()));
    // Options as the invoking window sees them, not paddock's own session.
    let suggest_command = tmux.row(&ctx.window, &["@agent-suggest-command"]).pop().unwrap_or_default();
    let names: Vec<&str> = COLOURS.iter().map(|(option, _)| *option).chain(MARKS).chain([AGENT_ICONS]).collect();
    // As the client the popup opened in sees them (formats may ask which it
    // is); as any client does if that fails.
    let row = Some(tmux.row_for(Some(&ctx.client), &ctx.window, &names)).filter(|r| !r.is_empty()).unwrap_or_else(|| tmux.row(&ctx.window, &names));
    let theme = theme(|name| names.iter().position(|n| *n == name).and_then(|i| row.get(i)).map_or("", String::as_str));
    let hidden_expanded = tmux.cmd(&format!("show-option -gqv {HIDDEN_EXPANDED}"))?.first().is_some_and(|v| v == "1");
    let scripts = tmux.cmd("show-option -gqv @agent-scripts-dir").unwrap_or_default().pop().unwrap_or_default();
    // Each its own command: a reply is read for each line sent.
    let option = |tmux: &mut Control, name: &str| tmux.cmd(&format!("show-options -sqv {name}")).unwrap_or_default().pop().unwrap_or_default();
    let keys = protocol(&option(&mut tmux, "extended-keys"), &option(&mut tmux, "extended-keys-format"));
    let start = Start { last_tab, theme, suggest_command, hidden_expanded, session: ctx.session, window: ctx.window, keys };
    Ok((Tmux { control: tmux, client: ctx.client, scripts }, start))
}

impl Backend for Tmux {
    fn fetch(&mut self) -> Result<Snapshot, String> {
        snapshot::fetch(&mut self.control)
    }

    fn replied(&self) -> u64 {
        self.control.replied()
    }

    fn is_dead(&self) -> bool {
        self.control.is_dead()
    }

    /// The screen; when it has fewer than FILL lines of text, captured again
    /// with enough history above it to make up the difference (one moment's
    /// history and screen). A full screen costs one command; the alternate
    /// screen, whose history is the main screen's, and a pane with none, two.
    fn capture(&mut self, pane: &str) -> Option<(Result<Vec<String>, String>, Duration)> {
        let start = Instant::now();
        let pane = quote(pane);
        let lines = self.control.cmd(&format!("capture-pane -p -e -t {pane}")).and_then(|screen| {
            if top_up(&screen, FILL) == 0 {
                return Ok(screen);
            }
            let history = self.control.cmd(&format!("display-message -p -t {pane} {}", quote("#{?alternate_on,0,#{history_size}}")))?;
            match top_up(&screen, history.first().and_then(|h| h.trim().parse().ok()).unwrap_or(0)) {
                0 => Ok(screen),
                n => self.control.cmd(&format!("capture-pane -p -e -S -{n} -t {pane}")),
            }
        });
        Some((lines, start.elapsed()))
    }

    fn run(&mut self, op: &Op) -> Ran {
        let mut moved = Vec::new();
        let (control, client) = (&mut self.control, self.client.as_str());
        let result = shelter(control, op, client).and_then(|sheltered| {
            if let Some(m) = &sheltered {
                moved.push(m.to.clone());
            }
            let result = run_op(control, op);
            // It failed: back to the session it was to close, if that
            // still stands.
            if result.is_err()
                && let Some(back) = sheltered.and_then(|m| unshelter(control, &m, client))
            {
                moved.push(back);
            }
            result
        });
        // A command without a reply ends the connection, and the popup.
        Ran { moved, result, unsure: false }
    }

    fn send(&mut self, pane: &str, input: &Input) -> Result<(), String> {
        let pane = quote(pane);
        let line = match input {
            Input::Key(key) => match keys::tmux(key) {
                Some(TmuxKey::Text(text)) => format!("send-keys -l -t {pane} -- {}", quote(&text)),
                Some(TmuxKey::Name(name)) => format!("send-keys -t {pane} {}", quote(&name)),
                None => return Ok(()),
            },
            // Through a buffer, so the pane's program gets it bracketed if
            // it asked (-p); its newlines go as Enter. Two commands, not a
            // `;` list: each is answered on its own, and every command
            // takes one answer.
            Input::Paste(text) => {
                self.control.cmd(&format!("set-buffer -b paddock -- {}", double_quoted(text)))?;
                // -d deletes the buffer only once pasted: a failed paste
                // must not leave the text in tmux.
                return self.control.cmd(&format!("paste-buffer -p -d -b paddock -t {pane}")).map(|_| ()).inspect_err(|_| {
                    let _ = self.control.cmd("delete-buffer -b paddock");
                });
            }
        };
        self.control.cmd(&line).map(|_| ())
    }

    fn closed(&mut self, tab: Tab) {
        let _ = self.control.cmd(&format!("set-option -gq {LAST_TAB} {}", tab.label()));
    }

    fn activate(&mut self, target: &Target) -> Result<(), String> {
        // Switch in one command: a pane target also selects the pane, and -Z
        // keeps a zoomed window zoomed (select-pane would unzoom it).
        let switched = self.control.cmd(&format!("switch-client -Z -c {} -t {}", quote(&self.client), arg(target)));
        if let Err(e) = switched {
            // display-message expands formats: keep a # in the error literal.
            let message = format!("paddock: {e}");
            let shown = self.control.cmd(&format!("display-message -c {} {}", quote(&self.client), literal(&message)));
            // Unseen, the error goes to the popup (-EE keeps it open).
            shown.map_err(|_| e)?;
        } else if !self.scripts.is_empty() {
            let _ = self.control.cmd(&format!("run-shell -b -t {} {}", arg(target), quote(&seen(&self.scripts))));
        }
        Ok(())
    }
}

/// `paddock next`: take CLIENT (else the one tmux says is current) to the
/// agent that has waited longest for an answer, else the one that finished
/// longest ago unseen, but not the one it is on. Two tmux calls, no control
/// client: it runs from a key, outside the popup.
pub fn next(client: Option<&str>) -> Result<(), String> {
    let tmux = |args: &[&str]| -> Result<String, String> {
        // -u: whatever the locale (a hotkey daemon's may be none), the
        // separators come back as they are.
        let out = Command::new("tmux").arg("-u").args(args).output().map_err(|e| format!("cannot run tmux: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    };
    let client = match client {
        Some(c) => c.to_string(),
        None => tmux(&["display-message", "-p", "#{client_name}"])?.trim().to_string(),
    };
    if client.is_empty() {
        return Err("no tmux client to move".into());
    }
    // The client's pane and session and agent-status's scripts, then the
    // server's snapshot, one per line.
    let only = format!("#{{==:#{{client_name}},{client}}}");
    let here = format!("#{{pane_id}}{SEP}#{{session_id}}{SEP}#{{@agent-scripts-dir}}");
    let out = tmux(&["list-clients", "-f", &only, "-F", &here, ";", "display-message", "-c", &client, "-p", &snapshot::format()])?;
    let (here, text) = out.split_once('\n').unwrap_or(("", ""));
    let mut here = here.trim().split(SEP);
    let (pane, session, scripts) = (here.next().unwrap_or_default(), here.next().unwrap_or_default(), here.next().unwrap_or_default());
    let snap = snapshot::parse(text);
    let Some(target) = snap.needs_you(pane) else {
        return tmux(&["display-message", "-c", &client, "paddock: no other agent needs you"]).map(|_| ());
    };
    // Not off to another session when the client's own shows the pane too.
    let target = snap.local(target, session);
    tmux(&["switch-client", "-Z", "-c", &client, "-t", &plain(&target)])?;
    if !scripts.is_empty() {
        let _ = tmux(&["run-shell", "-b", "-t", &plain(&target), &seen(scripts)]);
    }
    Ok(())
}

/// The shell command (for run-shell -t, which expands its formats for the
/// target) that marks the target pane's agent as seen: agent-status.tmux's
/// own, from SCRIPTS (@agent-scripts-dir). Its focus hook does the same, but
/// tmux sends no focus change when another client already shows the pane.
/// Silent and never failing: run-shell -t shows output and a failure in
/// the pane it lands in, and the option can outlive the scripts it names.
fn seen(scripts: &str) -> String {
    let script = format!("{scripts}/clear-finished.sh").replace('#', "##");
    format!("s={}; [ -x \"$s\" ] && \"$s\" #{{pane_id}} >/dev/null 2>&1; :", quote(&script))
}

/// TEXT as a tmux double-quoted string, its newlines and other control
/// characters escaped: a command sent over control mode is one line.
fn double_quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            // `~` too: tmux expands a leading one, even in double quotes.
            '\\' | '"' | '$' | '~' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\x7f' => out.push_str(&format!("\\{:03o}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// TARGET as a `-t` argument given as is, not through control mode's quoting.
fn plain(target: &Target) -> String {
    match target {
        Target::Session { session } => session.clone(),
        Target::Window { session, window } => format!("{session}:{window}"),
        Target::Pane { session, window, pane } => format!("{session}:{window}.{pane}"),
    }
}

/// The popup's key protocol from tmux's extended-keys and
/// extended-keys-format (tmux 3.5+): asked, tmux sends Shift+Enter and the
/// like extended, which crossterm reads in its csi-u form only.
fn protocol(on: &str, format: &str) -> Protocol {
    if matches!(on.trim(), "on" | "always") && format.trim() == "csi-u" { Protocol::ModifyOtherKeys } else { Protocol::Legacy }
}

/// How many of HISTORY's lines to show above SCREEN, so that it shows FILL
/// lines of text.
fn top_up(screen: &[String], history: usize) -> usize {
    let text = screen.len() - screen.iter().rev().take_while(|l| blank(l)).count();
    FILL.saturating_sub(text).min(history)
}

/// Whether a captured LINE shows nothing: spaces and SGR escapes alone.
fn blank(line: &str) -> bool {
    line.split('\x1b').enumerate().all(|(i, part)| {
        // After an escape: up to and including its final letter.
        let text = if i == 0 { part } else { part.find(|c: char| c.is_ascii_alphabetic()).map_or("", |j| &part[j + 1..]) };
        text.trim().is_empty()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::State;

    #[test]
    fn extended_keys_are_asked_for_only_in_the_form_crossterm_reads() {
        assert_eq!(protocol("on", "csi-u"), Protocol::ModifyOtherKeys);
        assert_eq!(protocol("always", "csi-u\n"), Protocol::ModifyOtherKeys);
        assert_eq!(protocol("on", "xterm"), Protocol::Legacy, "CSI 27;2;13~ is no key to crossterm");
        assert_eq!(protocol("off", "csi-u"), Protocol::Legacy);
        assert_eq!(protocol("", ""), Protocol::Legacy, "tmux before 3.5");
    }

    #[test]
    fn a_short_screen_is_topped_up_from_the_history_there_is() {
        let screen = |text: usize, blank: usize| [vec!["x".to_string(); text], vec![" \x1b[0m".to_string(); blank]].concat();
        assert_eq!(top_up(&screen(3, 20), 1000), FILL - 3, "trailing blank rows are not text");
        assert_eq!(top_up(&screen(3, 20), 10), 10, "no more than there is");
        assert_eq!(top_up(&screen(3, 20), 0), 0, "none, or the alternate screen's");
        assert_eq!(top_up(&screen(FILL, 0), 1000), 0, "full");
        assert_eq!(top_up(&screen(FILL + 5, 3), 1000), 0);
    }

    #[test]
    fn blank_lines_are_spaces_and_escapes_alone() {
        for line in ["", "   ", "\x1b[0m", "\x1b[38;5;3m  \x1b[39m "] {
            assert!(blank(line), "{line:?}");
        }
        for line in ["%", "\x1b[31mred\x1b[0m", " \x1b[1m>"] {
            assert!(!blank(line), "{line:?}");
        }
    }

    /// The theme from tmux options VALUES (by name); the rest unset.
    fn themed(values: &[(&str, &str)]) -> Theme {
        theme(|name| values.iter().find(|(n, _)| *n == name).map_or("", |(_, v)| v))
    }

    #[test]
    fn reads_fg_from_a_style() {
        assert_eq!(style_fg("fg=#A2A970"), "#A2A970");
        assert_eq!(style_fg("bg=#202020,fg=red"), "red");
        assert_eq!(style_fg("bg=red"), "");
    }

    #[test]
    fn tmux_theme_then_explicit_options_win() {
        use ratatui::style::{Color, Modifier, Style};
        let theme = [("E:@agent-color-working", "#cc8c3c"), ("E:pane-active-border-style", "fg=#A2A970")];
        let t = themed(&theme);
        assert_eq!(t.accent, Color::Rgb(0xa2, 0xa9, 0x70));
        assert_eq!(t.state(State::Working).fg, Some(Color::Rgb(0xcc, 0x8c, 0x3c)));
        assert_eq!(t.muted(), Style::new().add_modifier(Modifier::DIM));
        let t = themed(&[theme[0], theme[1], ("E:@paddock-color-accent", "brightcyan"), ("E:@paddock-color-working", "colour1"), ("E:@paddock-color-muted", "brightblack")]);
        assert_eq!(t.accent, Color::LightCyan);
        assert_eq!(t.state(State::Working).fg, Some(Color::Indexed(1)));
        assert_eq!(t.muted(), Style::new().fg(Color::DarkGray));
        assert_eq!(themed(&[("E:@paddock-color-accent", "orange")]).accent, Color::Magenta, "not a colour: the next");
    }

    #[test]
    fn agent_icons_are_explicit_and_default_to_unicode() {
        for (value, expected) in [("nerd", AgentIcons::Nerd), ("unicode", AgentIcons::Unicode), ("", AgentIcons::Unicode), ("auto", AgentIcons::Unicode), ("glyphs", AgentIcons::Unicode)] {
            assert_eq!(themed(&[(AGENT_ICONS, value)]).agent_icons, expected, "{value}");
        }
    }

    #[test]
    fn state_marks_are_the_status_bar_icons_that_fit() {
        let t = themed(&[("@agent-icon-idle", "-"), ("@agent-icon-finished", "◉"), ("@agent-icon-working", "●"), ("@agent-icon-asking", "??")]);
        assert_eq!(State::ALL.map(|s| t.icon(s)), ["-", "◉", "●", "●"]);
    }

    #[test]
    fn next_goes_to_the_longest_waiting_then_the_longest_finished() {
        let snap = fixture_hidden(&[
            "$1|s|@1|1|w|1|0|%1|0|1|claude||idle|1|claude|||300||/a",
            "$1|s|@1|1|w|1|0|%2|1|0|claude||asking|0|claude|||200||/a",
            "$1|s|@1|1|w|1|0|%3|2|0|claude||asking|0|claude|||100||/a",
            "$1|s|@1|1|w|1|0|%4|3|0|claude||idle|1|claude|||50||/a",
            "$1|s|@1|1|w|1|0|%5|4|0|claude||asking|0|claude|||10|1|/a",
            "$1|s|@1|1|w|1|0|%6|5|0|claude||asking|0|claude|||5||/a",
            "$1|s|@1|1|w|1|0|%7|6|0|claude||working|0|claude|||1||/a",
            "$1|s|@1|1|w|1|0|%8|7|0|claude||idle|0|claude|||1||/a",
        ], &["%6"]);
        let pane = |id: &str| Some(Target::Pane { session: "$1".into(), window: "@1".into(), pane: id.into() });
        assert_eq!(snap.needs_you(""), pane("%3"), "waiting longest; not stale %5 nor hidden %6");
        assert_eq!(snap.needs_you("%7"), pane("%3"), "from a pane not queued: the first");
        // The queue: %3 %2 (waiting), %4 %1 (finished), round again.
        assert_eq!(snap.needs_you("%3"), pane("%2"));
        assert_eq!(snap.needs_you("%2"), pane("%4"), "past the waiting, to the finished");
        assert_eq!(snap.needs_you("%4"), pane("%1"));
        assert_eq!(snap.needs_you("%1"), pane("%3"), "round to the first");
        let alone = fixture(&["$1|s|@1|1|w|1|0|%2|1|0|claude||asking|0|claude|||200||/a"]);
        assert_eq!(alone.needs_you("%2"), None, "on the only one: nowhere else to go");
        let answered = fixture(&["$1|s|@1|1|w|1|0|%1|0|1|claude||idle|1|claude|||300||/a", "$1|s|@1|1|w|1|0|%4|3|0|claude||idle|1|claude|||50||/a"]);
        assert_eq!(answered.needs_you(""), pane("%4"), "then finished longest ago");
        assert_eq!(fixture(&["$1|s|@1|1|w|1|0|%8|7|0|claude||idle|0|claude|||1||/a"]).needs_you(""), None, "seen idle and working need no one");
    }

    #[test]
    fn pastes_are_one_line_and_literal() {
        assert_eq!(double_quoted("a\nb\t\"$HOME\" \\ #{x}\x1b ~/x"), r#""a\nb\t\"\$HOME\" \\ #{x}\033 \~/x""#);
    }

    #[test]
    fn targets_name_their_session() {
        let t = Target::Pane { session: "$1".into(), window: "@2".into(), pane: "%3".into() };
        assert_eq!(arg(&t), "'$1:@2.%3'");
        assert_eq!(arg(&Target::Window { session: "$1".into(), window: "@2".into() }), "'$1:@2'");
    }

    #[test]
    fn names_and_folders_stay_literal() {
        assert_eq!(literal("#S #(rm -rf ~) it's"), r"'##S ##(rm -rf ~) it'\''s'");
    }

    /// `places_format` output: LINES are the client's session, then each
    /// session's `ID OURS WINDOWS GROUP`.
    fn layout(lines: &[&str]) -> (String, Vec<Place>) {
        places(&lines.join(&SEP.to_string()))
    }

    #[test]
    fn reads_every_session_from_one_line() {
        // As tmux 3.8 printed it: a with a-2 (grouped), "b x", _paddock.
        let (current, sessions) = layout(&["$0", "$0 0 @0%0@1%1%2 a", "$1 0 @2%3 ", "$2 0 @0%0@1%1%2 a", "$3 1 @4%5 "]);
        assert_eq!(current, "$0");
        let s = &sessions[0];
        assert_eq!((s.id.as_str(), s.group.as_str(), s.ours), ("$0", "a", false));
        assert_eq!(s.windows, [("@0".to_string(), vec!["%0".to_string()]), ("@1".to_string(), vec!["%1".to_string(), "%2".to_string()])]);
        assert_eq!(sessions.iter().map(|s| (s.group.as_str(), s.ours)).collect::<Vec<_>>(), [("a", false), ("", false), ("a", false), ("", true)]);
        assert_eq!(layout(&["$0", "$0 0 @0%0 my group"]).1[0].group, "my group");
    }

    #[test]
    fn refuge_is_taken_from_the_session_the_client_shows_now() {
        let kill = |session: &str| Op::Kill(Target::Session { session: session.into() });
        let (_, sessions) = layout(&["", "$0 0 @0%0 ", "$1 0 @1%1 ", "$2 0 @2%2 ", "$9 1 @9%9 "]);
        // The popup opened in $0, but a refuge (or anything else) moved the client to $1.
        assert_eq!(refuge(&kill("$1"), "$1", &sessions).as_deref(), Some("$0"));
        assert_eq!(refuge(&kill("$0"), "$1", &sessions), None, "not the client's session");
        let pane = Op::Kill(Target::Pane { session: "$1".into(), window: "@1".into(), pane: "%1".into() });
        assert_eq!(refuge(&pane, "$1", &sessions).as_deref(), Some("$0"));
        // Never paddock's own session; with nowhere else, nothing.
        let (_, alone) = layout(&["", "$0 0 @0%0 ", "$9 1 @9%9 "]);
        assert_eq!(refuge(&kill("$0"), "$0", &alone), None);
    }

    #[test]
    fn refuge_sees_windows_linked_elsewhere() {
        // $0's only window @0 is also linked into $1 (at two indexes there).
        let (_, sessions) = layout(&["", "$0 0 @0%0 ", "$1 0 @0%0@1%1@0%0 "]);
        let kill = Op::Kill(Target::Window { session: "$1".into(), window: "@0".into() });
        assert_eq!(refuge(&kill, "$0", &sessions).as_deref(), Some("$1"), "killed through $1, it closes $0 too");
        let pane = Op::Kill(Target::Pane { session: "$1".into(), window: "@0".into(), pane: "%0".into() });
        assert_eq!(refuge(&pane, "$0", &sessions).as_deref(), Some("$1"));
        // Linked twice into the client's session, it is still its only window.
        let (_, twice) = layout(&["", "$0 0 @0%0@0%0 ", "$1 0 @1%1 "]);
        let kill = Op::Kill(Target::Window { session: "$0".into(), window: "@0".into() });
        assert_eq!(refuge(&kill, "$0", &twice).as_deref(), Some("$1"));
        // A window with another pane, or a session with another window, stays.
        let (_, more) = layout(&["", "$0 0 @0%0%3 ", "$1 0 @0%0%3@1%1 "]);
        let pane = Op::Kill(Target::Pane { session: "$0".into(), window: "@0".into(), pane: "%0".into() });
        assert_eq!(refuge(&pane, "$0", &more), None);
        assert_eq!(refuge(&Op::Kill(Target::Window { session: "$1".into(), window: "@1".into() }), "$1", &more), None);
    }

    #[test]
    fn refuge_is_never_a_group_peer_that_closes_too() {
        // a ($0) and a-2 ($1) share their one window; b ($2) does not.
        let (_, sessions) = layout(&["", "$0 0 @0%0 a", "$1 0 @0%0 a", "$2 0 @1%1 "]);
        let window = Op::Kill(Target::Window { session: "$0".into(), window: "@0".into() });
        assert_eq!(refuge(&window, "$0", &sessions).as_deref(), Some("$2"));
        let pane = Op::Kill(Target::Pane { session: "$0".into(), window: "@0".into(), pane: "%0".into() });
        assert_eq!(refuge(&pane, "$0", &sessions).as_deref(), Some("$2"));
        // Killing the session alone leaves its peer, windows and all.
        assert_eq!(refuge(&Op::Kill(Target::Session { session: "$0".into() }), "$0", &sessions).as_deref(), Some("$1"));
        // Moving the window out of the peer empties the whole group: follow it.
        let mv = Op::MoveWindow { from: "$1".into(), window: "@0".into(), into: "$2".into(), to: "@1".into(), before: false };
        assert_eq!(refuge(&mv, "$0", &sessions).as_deref(), Some("$2"));
        // Out of an ungrouped session that also links it, it stays in the client's.
        let (_, linked) = layout(&["", "$0 0 @0%0 ", "$1 0 @0%0 ", "$2 0 @1%1 "]);
        assert_eq!(refuge(&mv, "$0", &linked), None);
    }

    #[test]
    fn refuge_follows_a_moved_window_or_pane() {
        let (_, sessions) = layout(&["", "$0 0 @0%0 ", "$1 0 @1%1 ", "$2 0 @2%2 "]);
        let mv = Op::MoveWindow { from: "$0".into(), window: "@0".into(), into: "$2".into(), to: "@2".into(), before: false };
        assert_eq!(refuge(&mv, "$0", &sessions).as_deref(), Some("$2"));
        let join = Op::JoinPane { pane: "%0".into(), to: "%2".into(), before: false };
        assert_eq!(refuge(&join, "$0", &sessions).as_deref(), Some("$2"));
        let break_ = |session: &str| Op::BreakPane { pane: "%0".into(), session: session.into() };
        assert_eq!(refuge(&break_("$2"), "$0", &sessions).as_deref(), Some("$2"));
        assert_eq!(refuge(&break_("$0"), "$0", &sessions), None, "a window of its own in the same session");
    }

    #[test]
    fn a_failed_op_takes_the_client_back_from_its_refuge() {
        let moved = Sheltered { from: "$0".into(), to: "$1".into() };
        let (_, sessions) = layout(&["", "$0 0 @0%0 ", "$1 0 @1%1 ", "$2 0 @2%2 "]);
        assert_eq!(homecoming(&moved, "$1", &sessions).as_deref(), Some("$0"));
        assert_eq!(homecoming(&moved, "$2", &sessions), None, "it moved on since: left there");
        assert_eq!(homecoming(&moved, "", &sessions), None, "the client is gone");
        let (_, closed) = layout(&["", "$1 0 @1%1 ", "$2 0 @2%2 "]);
        assert_eq!(homecoming(&moved, "$1", &closed), None, "the op closed it after all");
    }
}

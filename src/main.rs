//! paddock: a tmux popup for sessions, windows, panes and the AI agents in
//! them. See USAGE.
//!
//! The popup binding passes the invoking client and location as
//! PADDOCK_CLIENT, PADDOCK_SESSION and PADDOCK_WINDOW; paddock never guesses a
//! client, since another terminal may become active while it is open.
//!
//! Event-driven: one channel carries terminal input, tmux notifications and
//! title suggestions; the loop sleeps until one arrives or the live
//! preview's next capture (or a suggestion's deadline) is due, and draws
//! only when something changed.

mod app;
mod edit;
mod live;
mod model;
mod rows;
mod theme;
mod tmux;
mod ui;

use std::env;
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind};
use ratatui::crossterm::execute;

use app::{Action, App, Op, Tab, View};
use live::Live;
use model::{snapshot, Target, PANE_FIELDS, SESSION_FIELDS, WINDOW_FIELDS};
use theme::Theme;
use tmux::{field, quote, Control, Msg, SEP, SESSION};
use ui::Preview;

/// Refresh triggers, checked by tmux about once a second: every pane in
/// every session (a control client's %* covers only its own session) with
/// the fields a snapshot reads, so a title, state or name changing anywhere
/// shows. Changes to windows in other sessions (added, closed, renamed) also
/// arrive at once as notifications. Each value is a `field`, so a newline in
/// it cannot end the notification line.
fn subscription() -> String {
    let f = |names: &[&str]| names.iter().map(|n| field(n)).collect::<String>();
    let all = format!("panes::#{{S:{}#{{W:{}#{{P:{}}}}}}}", f(SESSION_FIELDS), f(WINDOW_FIELDS), f(PANE_FIELDS));
    format!("refresh-client -B {}", quote(&all))
}

enum Ev {
    Input(Event),
    Tmux(Msg),
    /// The suggest command's answer to suggestion ID.
    Suggested(u64, Result<String, String>),
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

/// Without --tab, paddock opens on the tab it was last closed on.
const USAGE: &str = "usage: paddock [--tab sessions|agents] | paddock init | paddock --version";

/// For `paddock init`: a binary installed without the plugin checkout
/// (Homebrew) still binds the keys, by running this script itself.
const PLUGIN: &str = include_str!("../paddock.tmux");

enum Cmd {
    Run(Option<Tab>),
    Init,
    Version,
    Help,
}

fn parse(args: &[&str]) -> Result<Cmd, String> {
    match args {
        [] => Ok(Cmd::Run(None)),
        ["--tab", "sessions"] => Ok(Cmd::Run(Some(Tab::Sessions))),
        ["--tab", "agents"] => Ok(Cmd::Run(Some(Tab::Agents))),
        ["init"] => Ok(Cmd::Init),
        ["--version" | "-V"] => Ok(Cmd::Version),
        ["--help" | "-h"] => Ok(Cmd::Help),
        _ => Err(USAGE.to_string()),
    }
}

/// The error (without main's "paddock: " prefix) if the key bindings that
/// started us, from plugin version PLUGIN, are not for OURS: the popup's
/// flags and env are a contract between the two.
fn version_mismatch(plugin: Option<&str>, ours: &str) -> Option<String> {
    plugin.filter(|p| *p != ours).map(|p| format!("these key bindings are for paddock {p} but this is paddock {ours} -- reload tmux (or update the plugin)"))
}

/// The path the key bindings should use for this binary, one that survives an
/// upgrade: Homebrew's `current_exe()` is a versioned Cellar path that is
/// deleted by the next `brew upgrade`, while the symlink we were started
/// through is not. ARGV0 with a `/` is that path (made absolute against CWD,
/// not canonicalized, to keep the symlink); with no CWD (unreadable) and a
/// relative ARGV0, it falls through to the next step. A bare name was found
/// through PATH: the first absolute entry whose `paddock` is the same file as
/// EXE (by CANON, the canonicalizer). Failing both, EXE.
fn stable_path(argv0: &Path, cwd: Option<&Path>, path: &OsStr, canon: impl Fn(&Path) -> Option<PathBuf>, exe: &Path) -> PathBuf {
    if argv0.to_string_lossy().contains('/') {
        // components() also drops a "." in the middle.
        if argv0.is_absolute() {
            return argv0.components().collect();
        }
        if let Some(cwd) = cwd {
            return cwd.join(argv0).components().collect();
        }
    }
    if let Some(real) = canon(exe) {
        // A relative entry (empty, ".", "bin") is the cwd's, which a key binding
        // would resolve elsewhere: skip it rather than trust it.
        let found = env::split_paths(path).filter(|d| d.is_absolute()).map(|d| d.join("paddock")).find(|c| canon(c).is_some_and(|c| c == real));
        if let Some(found) = found {
            return found;
        }
    }
    exe.to_path_buf()
}

fn self_path() -> Result<PathBuf, String> {
    let exe = env::current_exe().map_err(|e| format!("cannot find the paddock binary: {e}"))?;
    let argv0 = env::args_os().next().unwrap_or_default();
    // Only a relative argv0 with a `/` needs it: an unreadable cwd is not an error otherwise.
    let cwd = env::current_dir().ok();
    Ok(stable_path(Path::new(&argv0), cwd.as_deref(), &env::var_os("PATH").unwrap_or_default(), |p| p.canonicalize().ok(), &exe))
}

/// Bind the keys from the embedded paddock.tmux. It vouches for this binary
/// (PADDOCK_BIN, PADDOCK_VERSION); it prints nothing on success. bash comes
/// from PATH (not every system has /bin/bash).
fn init() -> Result<(), String> {
    let bin = self_path()?;
    let e = Command::new("bash")
        .args(["-c", PLUGIN, "paddock.tmux"])
        .env("PADDOCK_BIN", bin)
        .env("PADDOCK_VERSION", env!("CARGO_PKG_VERSION"))
        .exec();
    Err(format!("cannot run bash: {e}"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = parse(&args).and_then(|cmd| match cmd {
        Cmd::Run(tab) => match version_mismatch(env::var("PADDOCK_PLUGIN_VERSION").ok().as_deref(), env!("CARGO_PKG_VERSION")) {
            Some(e) => Err(e),
            None => run(tab),
        },
        Cmd::Init => init(),
        Cmd::Version => {
            println!("paddock {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Cmd::Help => {
            println!("{USAGE}");
            println!("init: bind the keys from ~/.tmux.conf with run-shell 'paddock init' (no plugin checkout needed)");
            Ok(())
        }
    });
    if let Err(e) = result {
        eprintln!("paddock: {e}");
        std::process::exit(1);
    }
}

/// A name or folder as a tmux argument. tmux expands formats in these
/// (`#S`, and `#(…)` would run a command), so `#` is doubled to stay literal.
fn literal(s: &str) -> String {
    quote(&s.replace('#', "##"))
}

/// `SESSION:WINDOW`: a window linked into several sessions is the copy in
/// SESSION, not whichever tmux finds first.
fn window_in(session: &str, window: &str) -> String {
    quote(&format!("{session}:{window}"))
}

/// The fields of FORMAT for TARGET, split on spaces.
fn probe(tmux: &mut Control, target: &str, format: &str) -> Result<Vec<String>, String> {
    let line = tmux.cmd(&format!("display-message -p -t {target} {}", quote(format)))?;
    Ok(line.first().map(|l| l.split(' ').map(String::from).collect()).unwrap_or_default())
}

/// Where CTX's client must go before OP runs, if OP closes the session it
/// shows: with tmux's default detach-on-destroy the client would be
/// detached. A moved window or pane is followed; otherwise another session.
fn refuge(tmux: &mut Control, op: &Op, ctx: &Context) -> Result<Option<String>, String> {
    let ours = |f: &[String]| f.first().is_some_and(|s| *s == ctx.session);
    let last_window = |f: &[String]| f.get(1).is_some_and(|n| n == "1");
    let last_pane = |f: &[String]| f.get(2).is_some_and(|n| n == "1");
    const PANE: &str = "#{session_id} #{session_windows} #{window_panes}";
    let dest = match op {
        Op::Kill(Target::Session { session }) => (*session == ctx.session).then(String::new),
        Op::Kill(Target::Window { session, window }) => {
            let f = probe(tmux, &window_in(session, window), "#{session_id} #{session_windows}")?;
            (ours(&f) && last_window(&f)).then(String::new)
        }
        Op::Kill(Target::Pane { pane, .. }) => {
            let f = probe(tmux, &quote(pane), PANE)?;
            (ours(&f) && last_window(&f) && last_pane(&f)).then(String::new)
        }
        Op::MoveWindow { from, window, into, .. } if *from == ctx.session && from != into => {
            let f = probe(tmux, &window_in(from, window), "#{session_id} #{session_windows}")?;
            last_window(&f).then(|| into.clone())
        }
        Op::JoinPane { pane, to, .. } | Op::BreakPane { pane, session: to } => {
            let f = probe(tmux, &quote(pane), PANE)?;
            if ours(&f) && last_window(&f) && last_pane(&f) {
                let target = if matches!(op, Op::BreakPane { .. }) { to.clone() } else { probe(tmux, &quote(to), "#{session_id}")?.concat() };
                Some(target)
            } else {
                None
            }
        }
        _ => None,
    };
    let Some(dest) = dest else { return Ok(None) };
    if !dest.is_empty() && dest != ctx.session {
        return Ok(Some(dest));
    }
    // Any other session (not paddock's own).
    let sessions = tmux.cmd(&format!("list-sessions -F {}", quote("#{session_id} #{session_name}")))?;
    Ok(sessions.iter().filter_map(|l| l.split_once(' ')).find(|(id, name)| *id != ctx.session && *name != SESSION).map(|(id, _)| id.to_string()))
}

/// Make OP's change on the server, for CTX's client; what it created, if
/// anything. Moves and swaps keep every client's current window (-d).
fn run_op(tmux: &mut Control, op: &Op, ctx: &Context) -> Result<Option<Target>, String> {
    let q = |s: &str| quote(s);
    if let Some(session) = refuge(tmux, op, ctx)? {
        tmux.cmd(&format!("switch-client -c {} -t {}", q(&ctx.client), q(&session)))?;
    }
    let line = match op {
        Op::SetTitle { pane, title: Some(title) } => format!("set-option -p -t {} @agent-description {}", q(pane), q(title)),
        Op::SetTitle { pane, title: None } => format!("set-option -p -u -t {} @agent-description", q(pane)),
        Op::RenameSession { session, name } => format!("rename-session -t {} -- {}", q(session), literal(name)),
        Op::RenameWindow { window, name } if name.is_empty() => format!("set-window-option -t {} automatic-rename on", q(window)),
        Op::RenameWindow { window, name } => format!("rename-window -t {} -- {}", q(window), literal(name)),
        Op::Kill(Target::Session { session }) => format!("kill-session -t {}", q(session)),
        Op::Kill(Target::Window { session, window }) => format!("kill-window -t {}", window_in(session, window)),
        Op::Kill(Target::Pane { pane, .. }) => format!("kill-pane -t {}", q(pane)),
        Op::Kill(Target::New(_)) => return Ok(None),
        Op::SwapWindows { session, a, b } => format!("swap-window -d -s {} -t {}", window_in(session, a), window_in(session, b)),
        Op::SwapPanes(a, b) => format!("swap-pane -d -s {} -t {}", q(a), q(b)),
        Op::MoveWindow { from, window, into, to, before } => {
            format!("move-window -d {} -s {} -t {}", if *before { "-b" } else { "-a" }, window_in(from, window), window_in(into, to))
        }
        Op::JoinPane { pane, to, before } => {
            // Side by side in a wide pane, else one above the other (cells
            // are about twice as tall as wide).
            let size = probe(tmux, &q(to), "#{pane_width} #{pane_height}")?;
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

/// How long a suggestion may take before it is given up (and stopped).
const SUGGEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A running suggest command. Dropping it stops the command's whole process
/// group (it runs pi, which may have children of its own) unless it ended.
struct Suggestion {
    id: u64,
    pid: libc::pid_t,
    done: Arc<AtomicBool>,
    deadline: Instant,
}

impl Drop for Suggestion {
    fn drop(&mut self) {
        if !self.done.load(Ordering::SeqCst) {
            // SAFETY: kill(2) on our own child's process group; no memory is involved.
            unsafe { libc::kill(-self.pid, libc::SIGTERM) };
        }
    }
}

/// Run the suggest command (COMMAND, a shell command line, with PANE as its
/// argument) in its own process group, and post its answer as suggestion ID.
fn suggest(command: &str, pane: &str, id: u64, tx: Sender<Ev>) -> Result<Suggestion, String> {
    let child = Command::new("sh")
        .arg("-c")
        .arg(format!("{command} \"$1\""))
        .arg("sh")
        .arg(pane)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("cannot run the suggest command: {e}"))?;
    let pid = child.id() as libc::pid_t;
    let done = Arc::new(AtomicBool::new(false));
    let finished = Arc::clone(&done);
    thread::spawn(move || {
        let out = child.wait_with_output();
        finished.store(true, Ordering::SeqCst);
        let line = |bytes: &[u8], last: bool| {
            let text = String::from_utf8_lossy(bytes);
            let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
            if last { lines.next_back() } else { lines.next() }.map(String::from)
        };
        let result = match out {
            Ok(o) if o.status.success() => line(&o.stdout, false).ok_or_else(|| "no title came back".to_string()),
            Ok(o) => Err(line(&o.stderr, true).unwrap_or_else(|| format!("the suggest command failed ({})", o.status))),
            Err(e) => Err(format!("the suggest command failed: {e}")),
        };
        let _ = tx.send(Ev::Suggested(id, result));
    });
    Ok(Suggestion { id, pid, done, deadline: Instant::now() + SUGGEST_TIMEOUT })
}

/// Mouse reporting for presses, releases, the wheel and motion while a
/// button is held (drag to move), in SGR encoding: no hover motion, which
/// would cost a frame per cell.
const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1006h";
const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1002l\x1b[?1000l";

/// The terminal in raw mode on the alternate screen, with mouse reporting
/// and bracketed paste. Dropping it restores the terminal, also when a
/// panic unwinds (ratatui's panic hook restores the rest first).
struct Tui(ratatui::DefaultTerminal);

impl Tui {
    fn new() -> Result<Tui, String> {
        let terminal = ratatui::try_init().map_err(|e| format!("cannot open the terminal: {e}"))?;
        let mut out = std::io::stdout();
        let _ = write!(out, "{MOUSE_ON}");
        let _ = execute!(out, EnableBracketedPaste);
        Ok(Tui(terminal))
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        let _ = write!(out, "{MOUSE_OFF}");
        let _ = execute!(out, DisableBracketedPaste);
        ratatui::restore();
    }
}

/// When the wall clock reaches the next minute, and ages ("5m") tick over.
fn next_minute() -> Instant {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    Instant::now() + Duration::from_secs(60 - now.as_secs() % 60)
}

/// Attach and subscribe. Another popup closing can destroy the shared
/// session between our creating and attaching to it, so that is tried
/// twice; a failed attempt's events never reach the loop.
fn connect(tx: &Sender<Ev>) -> Result<Control, String> {
    let mut failure = String::new();
    for _ in 0..2 {
        let live = Arc::new(AtomicBool::new(true));
        let (forward, events) = (Arc::clone(&live), tx.clone());
        let mut tmux = Control::connect(move |m| {
            if forward.load(Ordering::SeqCst) {
                let _ = events.send(Ev::Tmux(m));
            }
        })?;
        match tmux.cmd(&subscription()) {
            Ok(_) => return Ok(tmux),
            Err(e) => {
                live.store(false, Ordering::SeqCst);
                failure = e;
            }
        }
    }
    Err(format!("cannot attach to tmux session {SESSION}: {failure}"))
}

/// Global option holding the tab paddock was last closed on.
const LAST_TAB: &str = "@paddock-last-tab";

fn run(tab: Option<Tab>) -> Result<(), String> {
    let ctx = context()?;
    let (tx, rx) = mpsc::channel();
    let mut tmux = connect(&tx)?;
    let tab = tab.unwrap_or_else(|| {
        let last = tmux.cmd(&format!("show-option -gqv {LAST_TAB}")).unwrap_or_default();
        Tab::parse(last.first().map_or("", |l| l.trim())).unwrap_or(Tab::Sessions)
    });
    // Options as the invoking window sees them, not paddock's own session.
    let [host, suggest_command] = <[String; 2]>::try_from(tmux.row(&ctx.window, &["host", "@agent-suggest-command"])).unwrap_or_default();
    let theme = Theme::load(&mut tmux, &ctx.window);
    let home = std::env::var("HOME").unwrap_or_default();
    let debug = std::env::var_os("PADDOCK_DEBUG").is_some();
    let can_suggest = !suggest_command.trim().is_empty();
    let mut app = App::new(snapshot(&mut tmux, &host)?, tab, ctx.session.clone(), ctx.window.clone(), home, debug, can_suggest);

    let mut tui = Tui::new()?;
    let answers = tx.clone();
    // Terminal input joins the tmux notifications on one channel.
    thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if tx.send(Ev::Input(ev)).is_err() {
                break;
            }
        }
    });
    let result = (|| -> Result<Option<Target>, String> {
        let mut live = Live::new();
        let mut view = View::new();
        let mut minute = next_minute();
        let mut dirty = true;
        let mut suggestion: Option<Suggestion> = None;
        loop {
            // The preview follows the selection while it is on screen (the
            // last draw says whether there was room for it).
            live.follow(app.preview_pane(&view));
            dirty |= live.tick(&mut tmux);
            if tmux.is_dead() {
                return Err("tmux stopped answering".into());
            }
            if dirty {
                let title = live.pane.as_deref().map(|p| rows::pane_title(&app.snap, p)).unwrap_or_default();
                let preview = Preview { title: &title, body: &live.body };
                tui.0.draw(|f| ui::draw(f, &app, &mut view, &preview, &theme)).map_err(|e| e.to_string())?;
                dirty = false;
                if app.preview_pane(&view) != live.pane.as_deref() {
                    // The preview just appeared or went away: follow it now.
                    dirty = true;
                    continue;
                }
            }

            let deadline = [Some(minute), live.due(), suggestion.as_ref().map(|s| s.deadline)].into_iter().flatten().min().unwrap_or(minute);
            let mut stale = false;
            let mut ev = match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(ev) => Some(ev),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            };
            // Handle everything already queued before drawing again.
            while let Some(e) = ev {
                match e {
                    Ev::Input(input) => {
                        let action = match input {
                            Event::Key(k) if k.kind != KeyEventKind::Release => app.on_key(k),
                            Event::Mouse(m) => app.on_mouse(m, &view, Instant::now()),
                            Event::Paste(text) => {
                                app.paste(&text);
                                Action::None
                            }
                            _ => Action::None,
                        };
                        match action {
                            Action::Quit => return Ok(None),
                            Action::Activate(t) => return Ok(Some(t)),
                            Action::Run(job) => {
                                let result = run_op(&mut tmux, &job.op, &ctx);
                                if let Some(created) = app.ran(job, result) {
                                    return Ok(Some(created));
                                }
                                // Show the result now, not at the next subscription check.
                                stale = true;
                            }
                            Action::Suggest { pane, id } => match suggest(&suggest_command, &pane, id, answers.clone()) {
                                Ok(s) => suggestion = Some(s),
                                Err(e) => app.suggested(id, Err(e)),
                            },
                            Action::None => {}
                        }
                        dirty = true;
                    }
                    Ev::Suggested(id, result) => {
                        if suggestion.as_ref().is_some_and(|s| s.id == id) {
                            suggestion = None;
                        }
                        app.suggested(id, result);
                        dirty = true;
                    }
                    Ev::Tmux(Msg::Changed) => stale = true,
                    Ev::Tmux(Msg::Exit) => return Ok(None),
                }
                ev = rx.try_recv().ok();
            }
            if let Some(s) = &suggestion
                && Instant::now() >= s.deadline
            {
                let id = s.id;
                suggestion = None;
                app.suggested(id, Err(format!("no title after {}s", SUGGEST_TIMEOUT.as_secs())));
                dirty = true;
            }
            // Stop a suggestion nobody waits for any more (cancelled, typed over).
            if suggestion.as_ref().is_some_and(|s| app.suggesting() != Some(s.id)) {
                suggestion = None;
            }
            if stale {
                // One snapshot for any burst of notifications.
                app.set_snapshot(snapshot(&mut tmux, &host)?);
                live.wake();
                dirty = true;
            }
            if Instant::now() >= minute {
                app.refresh();
                minute = next_minute();
                dirty = true;
            }
        }
    })();
    drop(tui);
    // The next bare `paddock` reopens this tab.
    let _ = tmux.cmd(&format!("set-option -gq {LAST_TAB} {}", app.tab.label()));
    if let Some(target) = result? {
        // Switch in one command: a pane target also selects the pane, and -Z
        // keeps a zoomed window zoomed (select-pane would unzoom it).
        let switched = tmux.cmd(&format!("switch-client -Z -c {} -t {}", quote(&ctx.client), target.arg()));
        if let Err(e) = switched {
            // display-message expands formats: keep a # in the error literal.
            let message = format!("paddock: {e}");
            let shown = tmux.cmd(&format!("display-message -c {} {}", quote(&ctx.client), quote(&message.replace('#', "##"))));
            // Unseen, the error goes to the popup (-EE keeps it open).
            shown.map_err(|_| e)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_folders_stay_literal() {
        assert_eq!(literal("#S #(rm -rf ~) it's"), r"'##S ##(rm -rf ~) it'\''s'");
        assert_eq!(window_in("$1", "@2"), "'$1:@2'");
    }

    #[test]
    fn parses_commands() {
        assert!(matches!(parse(&["init"]), Ok(Cmd::Init)));
        assert!(parse(&["init", "x"]).is_err());
        assert!(matches!(parse(&["--tab", "agents"]), Ok(Cmd::Run(Some(Tab::Agents)))));
        assert!(matches!(parse(&["--tab", "sessions"]), Ok(Cmd::Run(Some(Tab::Sessions)))));
        assert!(parse(&["--tab", "nope"]).is_err());
        assert!(matches!(parse(&[]), Ok(Cmd::Run(None))));
        assert!(matches!(parse(&["--version"]), Ok(Cmd::Version)));
        assert!(matches!(parse(&["-V"]), Ok(Cmd::Version)));
        assert!(matches!(parse(&["--help"]), Ok(Cmd::Help)));
        assert!(matches!(parse(&["-h"]), Ok(Cmd::Help)));
        assert_eq!(parse(&["bogus"]).err().as_deref(), Some(USAGE));
    }

    #[test]
    fn embedded_plugin_is_the_init_mode_script() {
        assert!(PLUGIN.contains("PADDOCK_VERSION"));
        assert!(PLUGIN.contains("PADDOCK_BIN"));
    }

    #[test]
    fn handshake_refuses_other_versions() {
        assert_eq!(version_mismatch(None, "0.1.0"), None);
        assert_eq!(version_mismatch(Some("0.1.0"), "0.1.0"), None);
        let e = version_mismatch(Some("0.2.0"), "0.1.0").unwrap();
        assert_eq!(e, "these key bindings are for paddock 0.2.0 but this is paddock 0.1.0 -- reload tmux (or update the plugin)");
    }

    #[test]
    fn stable_path_prefers_how_we_were_started() {
        let exe = Path::new("/opt/Cellar/paddock/0.1.0/bin/paddock");
        let cwd = Path::new("/work");
        let path = OsStr::new("/usr/bin:/opt/homebrew/bin:/other");
        // Only the Homebrew symlink resolves to the exe; /usr/bin/paddock is a different file.
        let canon = |p: &Path| match p.to_str().unwrap() {
            "/opt/Cellar/paddock/0.1.0/bin/paddock" | "/opt/homebrew/bin/paddock" => Some(exe.to_path_buf()),
            "/usr/bin/paddock" => Some(PathBuf::from("/usr/bin/other")),
            _ => None,
        };
        let go = |argv0: &str, path: &OsStr| stable_path(Path::new(argv0), Some(cwd), path, canon, exe);
        // A path is kept as given: absolute, or against cwd, with no canonicalizing.
        assert_eq!(go("/opt/homebrew/bin/paddock", path), Path::new("/opt/homebrew/bin/paddock"));
        assert_eq!(go("./target/debug/paddock", path), Path::new("/work/target/debug/paddock"));
        assert_eq!(go("bin/paddock", path), Path::new("/work/bin/paddock"));
        // A bare name: the PATH entry that is the same file as the exe.
        assert_eq!(go("paddock", path), Path::new("/opt/homebrew/bin/paddock"));
        // No match, an empty PATH, or an exe that cannot be resolved: the exe.
        assert_eq!(go("paddock", OsStr::new("/usr/bin:/other")), exe);
        assert_eq!(go("paddock", OsStr::new("")), exe);
        assert_eq!(stable_path(Path::new("paddock"), Some(cwd), path, |_| None, exe), exe);
        // Relative PATH entries (".", "bin", empty) are never trusted, even when they resolve to the exe.
        let canon_rel = |p: &Path| match p.to_str().unwrap() {
            "./paddock" | "bin/paddock" | "paddock" | "/opt/Cellar/paddock/0.1.0/bin/paddock" | "/opt/homebrew/bin/paddock" => Some(exe.to_path_buf()),
            _ => None,
        };
        let rel = |path: &str| stable_path(Path::new("paddock"), Some(cwd), OsStr::new(path), canon_rel, exe);
        assert_eq!(rel(".:bin::/usr/bin"), exe);
        assert_eq!(rel(".:bin:/opt/homebrew/bin"), Path::new("/opt/homebrew/bin/paddock"));
        // No cwd: an absolute argv0 still stands; a relative one with a `/` falls through to PATH, then the exe.
        let nocwd = |argv0: &str, path: &str| stable_path(Path::new(argv0), None, OsStr::new(path), canon_rel, exe);
        assert_eq!(nocwd("/opt/homebrew/bin/paddock", ""), Path::new("/opt/homebrew/bin/paddock"));
        assert_eq!(nocwd("./target/debug/paddock", "/opt/homebrew/bin"), Path::new("/opt/homebrew/bin/paddock"));
        assert_eq!(nocwd("bin/paddock", "bin"), exe);
    }
}

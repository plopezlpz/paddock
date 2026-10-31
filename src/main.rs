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
mod paths;
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
use model::Target;
use theme::Theme;
use tmux::{field, quote, Control, Msg, SEP, SESSION};
use ui::Preview;

/// Refresh, checked by tmux about once a second: the snapshot's text, so a
/// title, state or name changing anywhere (a control client's %* covers only
/// its own session) arrives with the new snapshot. Changes to windows in
/// other sessions (added, closed, renamed) also arrive at once as
/// notifications. Each value is a `field`, so a newline in it cannot end the
/// notification line.
fn subscription() -> String {
    format!("refresh-client -B {}", quote(&format!("panes::{}", model::format())))
}

enum Ev {
    Input(Event),
    /// A notification, after this many replies (`Control::replied`).
    Tmux(u64, Msg),
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

/// Without --tab (or with --tab last), paddock opens on the tab it was last
/// closed on.
const USAGE: &str = "usage: paddock [--tab sessions|agents|last] | paddock init | paddock --version";

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
        [] | ["--tab", "last"] => Ok(Cmd::Run(None)),
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
/// flags and env are a contract between the two. The fix depends on which
/// side is older; each line fits a popup on an 80-column terminal.
fn version_mismatch(plugin: Option<&str>, ours: &str) -> Option<String> {
    // Numerically, part by part: "0.10.0" is newer than "0.9.0".
    let parts = |v: &str| v.split('.').map(str::parse::<u64>).collect::<Result<Vec<_>, _>>().ok();
    plugin.filter(|p| *p != ours).map(|p| {
        let fix = match (parts(p), parts(ours)) {
            (Some(a), Some(b)) if a < b => "reload tmux to rebind the keys".to_string(),
            (Some(a), Some(b)) if a > b => format!("install paddock {p} (or point PADDOCK_BIN at it)"),
            _ => format!("reload tmux, or install paddock {p}"),
        };
        format!("key bindings for paddock {p}, but this is {ours}\n{fix}")
    })
}

/// The command ARGS ask for, run from key bindings for PLUGIN. Bindings from
/// another version are refused before the arguments are checked: newer ones
/// may pass flags this binary does not know, and the version is what to fix.
fn command(args: &[&str], plugin: Option<&str>, ours: &str) -> Result<Cmd, String> {
    let cmd = parse(args);
    if matches!(cmd, Ok(Cmd::Init | Cmd::Version | Cmd::Help)) {
        return cmd;
    }
    version_mismatch(plugin, ours).map_or(cmd, Err)
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
    let plugin = env::var("PADDOCK_PLUGIN_VERSION").ok();
    let result = command(&args, plugin.as_deref(), env!("CARGO_PKG_VERSION")).and_then(|cmd| match cmd {
        Cmd::Run(tab) => run(tab),
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
    let window_in = |session: &String, window: &String| Target::Window { session: session.clone(), window: window.clone() }.arg();
    let line = match op {
        Op::SetTitle { pane, title: Some(title) } => format!("set-option -p -t {} @agent-description {}", q(pane), q(title)),
        Op::SetTitle { pane, title: None } => format!("set-option -p -u -t {} @agent-description", q(pane)),
        Op::SetHidden { pane, hidden } => format!("set-option -p -t {} @paddock-hidden {}", q(pane), u8::from(*hidden)),
        Op::SetHiddenExpanded(expanded) => format!("set-option -g {HIDDEN_EXPANDED} {}", u8::from(*expanded)),
        Op::RenameSession { session, name } => format!("rename-session -t {} -- {}", q(session), literal(name)),
        Op::RenameWindow { window, name } if name.is_empty() => format!("set-window-option -t {} automatic-rename on", q(window)),
        Op::RenameWindow { window, name } => format!("rename-window -t {} -- {}", q(window), literal(name)),
        Op::Kill(Target::Session { session }) => format!("kill-session -t {}", q(session)),
        Op::Kill(t @ Target::Window { .. }) => format!("kill-window -t {}", t.arg()),
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

/// Attach and subscribe.
fn connect(tx: &Sender<Ev>) -> Result<Control, String> {
    let events = tx.clone();
    let mut tmux = Control::connect(move |replied, m| {
        let _ = events.send(Ev::Tmux(replied, m));
    })?;
    tmux.cmd(&subscription()).map_err(|e| format!("cannot attach to tmux session {SESSION}: {e}"))?;
    Ok(tmux)
}

/// Global option holding the tab paddock was last closed on.
const LAST_TAB: &str = "@paddock-last-tab";
const HIDDEN_EXPANDED: &str = "@paddock-hidden-expanded";

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
    let can_suggest = !suggest_command.trim().is_empty();
    // The snapshot's text, to skip a refresh that changes nothing.
    let mut shown = model::fetch(&mut tmux)?;
    // Notifications tmux sent before the last fetch's reply are in it.
    let mut fetched = tmux.replied();
    let hidden_expanded = tmux.cmd(&format!("show-option -gqv {HIDDEN_EXPANDED}"))?.first().is_some_and(|v| v == "1");
    let mut app = App::new(model::parse(&shown, &host), tab, ctx.session.clone(), ctx.window.clone(), home, can_suggest, hidden_expanded);

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
            let mut fresh = None;
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
                                let result = shelter(&mut tmux, &job.op, &ctx.client).and_then(|moved| {
                                    if let Some(m) = &moved {
                                        app.moved(&m.to);
                                    }
                                    let result = run_op(&mut tmux, &job.op);
                                    // It failed: back to the session it was to close,
                                    // if that still stands.
                                    if result.is_err()
                                        && let Some(back) = moved.and_then(|m| unshelter(&mut tmux, &m, &ctx.client))
                                    {
                                        app.moved(&back);
                                    }
                                    result
                                });
                                if let Some(created) = app.ran(job, result) {
                                    return Ok(Some(created));
                                }
                                // Show the result now, not at the next subscription
                                // check, even if nothing changed (a moved row is
                                // followed then).
                                stale = true;
                                shown.clear();
                                // Events still queued (a held ⇧↓) wait for that
                                // snapshot: from this one, a second swap would
                                // undo the first.
                                dirty = true;
                                break;
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
                    Ev::Tmux(_, Msg::Exit) => return Ok(None),
                    // Older than the snapshot shown: the %sessions-changed
                    // making SESSION sends, or a subscription value from
                    // before an op, which would put back what it changed.
                    Ev::Tmux(replied, _) if replied < fetched => {}
                    Ev::Tmux(_, Msg::Changed) => stale = true,
                    Ev::Tmux(_, Msg::Rows(text)) => fresh = Some(text),
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
            // One snapshot for any burst of notifications: the subscription's
            // latest unless one is due now. The preview's pane is captured on
            // its own schedule.
            let text = if stale {
                let text = model::fetch(&mut tmux)?;
                fetched = tmux.replied();
                Some(text)
            } else {
                fresh
            };
            if let Some(text) = text.filter(|t| *t != shown) {
                app.set_snapshot(model::parse(&text, &host));
                shown = text;
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
            let shown = tmux.cmd(&format!("display-message -c {} {}", quote(&ctx.client), literal(&message)));
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

    #[test]
    fn parses_commands() {
        assert!(matches!(parse(&["init"]), Ok(Cmd::Init)));
        assert!(parse(&["init", "x"]).is_err());
        assert!(matches!(parse(&["--tab", "agents"]), Ok(Cmd::Run(Some(Tab::Agents)))));
        assert!(matches!(parse(&["--tab", "sessions"]), Ok(Cmd::Run(Some(Tab::Sessions)))));
        assert!(parse(&["--tab", "nope"]).is_err());
        assert!(matches!(parse(&[]), Ok(Cmd::Run(None))));
        assert!(matches!(parse(&["--tab", "last"]), Ok(Cmd::Run(None))));
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
        // Stale bindings, an upgraded binary: a reload rebinds them.
        let e = version_mismatch(Some("0.9.0"), "0.10.0").unwrap();
        assert_eq!(e, "key bindings for paddock 0.9.0, but this is 0.10.0\nreload tmux to rebind the keys");
        // An old binary (PADDOCK_BIN, or not rebuilt): reloading does nothing.
        let e = version_mismatch(Some("0.3.0"), "0.2.0").unwrap();
        assert_eq!(e, "key bindings for paddock 0.3.0, but this is 0.2.0\ninstall paddock 0.3.0 (or point PADDOCK_BIN at it)");
        let e = version_mismatch(Some("0.3.0-rc1"), "0.3.0").unwrap();
        assert!(e.ends_with("\nreload tmux, or install paddock 0.3.0-rc1"), "{e}");
        // A popup 80% of 80 columns wide shows each line unwrapped.
        for (p, o) in [("10.10.10", "10.10.9"), ("10.10.9", "10.10.10")] {
            for line in version_mismatch(Some(p), o).unwrap().lines() {
                assert!("paddock: ".len() + line.len() <= 64, "{line}");
            }
        }
    }

    #[test]
    fn version_is_checked_before_arguments() {
        let mismatch = version_mismatch(Some("0.9.0"), "0.8.0");
        // A flag from newer bindings: the version is the error, not usage.
        assert_eq!(command(&["--tab", "future"], Some("0.9.0"), "0.8.0").err(), mismatch);
        assert_eq!(command(&["--tab", "agents"], Some("0.9.0"), "0.8.0").err(), mismatch);
        assert_eq!(command(&["--tab", "future"], Some("0.8.0"), "0.8.0").err().as_deref(), Some(USAGE));
        assert!(matches!(command(&["--tab", "last"], Some("0.8.0"), "0.8.0"), Ok(Cmd::Run(None))));
        assert!(matches!(command(&["--tab", "last"], None, "0.8.0"), Ok(Cmd::Run(None))));
        // These never need bindings, so any environment runs them.
        assert!(matches!(command(&["--version"], Some("0.9.0"), "0.8.0"), Ok(Cmd::Version)));
        assert!(matches!(command(&["--help"], Some("0.9.0"), "0.8.0"), Ok(Cmd::Help)));
        assert!(matches!(command(&["init"], Some("0.9.0"), "0.8.0"), Ok(Cmd::Init)));
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

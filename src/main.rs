//! paddock: a popup for sessions, windows, panes and the AI agents in
//! them, on a tmux or a Rex server (src/backend). See USAGE.
//!
//! Event-driven: one channel carries terminal input, server notifications
//! (on Rex also: a capture came in) and title suggestions; the loop sleeps
//! until one arrives or the live preview's next capture (or a suggestion's
//! deadline) is due, and draws only when something changed.

mod app;
mod backend;
mod diffstat;
mod edit;
mod keys;
mod live;
mod model;
mod notification;
mod paths;
mod rows;
mod theme;
mod transcript;
mod ui;
mod worker;

use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind};
use ratatui::crossterm::execute;

use app::{Action, App, Tab, View};
use live::Live;
use model::Target;
use backend::{Backend, Msg, Ran, Start};
use backend::rex::lua_string as lua;
use ui::Preview;

enum Ev {
    Input(Event),
    /// A server notification, after this many replies (`Backend::replied`).
    Backend(u64, Msg),
    /// The suggest command's answer to suggestion ID.
    Suggested(u64, Result<String, String>),
    /// The changes waiting in a folder (diffstat), and whether git was slow
    /// to say.
    Diff(String, Option<diffstat::Diff>, bool),
    /// What a transcript says, for an agent with that mark (`Agent::mark`).
    Note(String, model::Mark, String),
}

/// Without --tab (or with --tab last), paddock opens on the tab it was last
/// closed on.
const USAGE: &str = "usage: paddock [--tab sessions|agents|last] | paddock init [--rex] | paddock --version\n       paddock --rex [--tab sessions|agents] [--client CLIENT] [--suggest COMMAND] [--color ROLE=COLOUR]... [--icons unicode|nerd]\n       paddock notification finished|asking|waiting|failed KIND PATH [unicode|nerd]\n       paddock next [-c CLIENT]";

/// For `paddock init`: a binary installed without the plugin checkout
/// (Homebrew) still binds the keys, by running this script itself.
const PLUGIN: &str = include_str!("../paddock.tmux");
/// For `paddock init --rex`: the Rex action, made for this binary.
const REX_PLUGIN: &str = include_str!("../rex/paddock.lua");

enum Cmd {
    Run(Option<Tab>),
    /// On Rex: the rex/paddock.lua action's command line.
    Rex(backend::rex::Opts),
    Init,
    InitRex,
    Notification { state: String, kind: String, path: String, icons: theme::AgentIcons },
    /// Jump CLIENT (else tmux's current one) to the next agent that needs you.
    Next(Option<String>),
    Version,
    Help,
}

fn parse(args: &[&str]) -> Result<Cmd, String> {
    match args {
        [] | ["--tab", "last"] => Ok(Cmd::Run(None)),
        ["--tab", "sessions"] => Ok(Cmd::Run(Some(Tab::Sessions))),
        ["--tab", "agents"] => Ok(Cmd::Run(Some(Tab::Agents))),
        ["--rex", opts @ ..] => backend::rex::Opts::parse(opts).map(Cmd::Rex),
        ["init"] => Ok(Cmd::Init),
        ["init", "--rex"] => Ok(Cmd::InitRex),
        ["notification", state @ ("finished" | "asking" | "waiting" | "failed"), kind, path] => {
            Ok(Cmd::Notification { state: (*state).into(), kind: (*kind).into(), path: (*path).into(), icons: theme::AgentIcons::Unicode })
        }
        ["notification", state @ ("finished" | "asking" | "waiting" | "failed"), kind, path, mode @ ("unicode" | "nerd")] => {
            Ok(Cmd::Notification { state: (*state).into(), kind: (*kind).into(), path: (*path).into(), icons: if *mode == "nerd" { theme::AgentIcons::Nerd } else { theme::AgentIcons::Unicode } })
        }
        ["next"] => Ok(Cmd::Next(None)),
        ["next", "-c", client] => Ok(Cmd::Next(Some((*client).to_string()))),
        ["--version" | "-V"] => Ok(Cmd::Version),
        ["--help" | "-h"] => Ok(Cmd::Help),
        _ => Err(USAGE.to_string()),
    }
}

/// The error (without main's "paddock: " prefix) if the key bindings that
/// started us, or on REX rex/paddock.lua, from plugin version PLUGIN, are not
/// for OURS: the popup's flags and env are a contract between the two. The
/// fix depends on which side is older; each line fits a popup on an
/// 80-column terminal.
fn version_mismatch(plugin: Option<&str>, ours: &str, rex: bool) -> Option<String> {
    // Numerically, part by part: "0.10.0" is newer than "0.9.0".
    let parts = |v: &str| v.split('.').map(str::parse::<u64>).collect::<Result<Vec<_>, _>>().ok();
    let (what, update, bin) = if rex { ("paddock.lua", "run paddock init --rex", "set bin in paddock.setup") } else { ("key bindings", "reload tmux", "point PADDOCK_BIN at it") };
    plugin.filter(|p| *p != ours).map(|p| {
        let fix = match (parts(p), parts(ours)) {
            (Some(a), Some(b)) if a < b && rex => format!("{update} to update it"),
            (Some(a), Some(b)) if a < b => format!("{update} to rebind the keys"),
            (Some(a), Some(b)) if a > b => format!("install paddock {p} (or {bin})"),
            _ => format!("{update}, or install paddock {p}"),
        };
        format!("{what} for paddock {p}, but this is {ours}\n{fix}")
    })
}

/// The command ARGS ask for, run from key bindings (or rex/paddock.lua) for
/// PLUGIN. Bindings from another version are refused before the arguments
/// are checked: newer ones may pass flags this binary does not know, and the
/// version is what to fix.
fn command(args: &[&str], plugin: Option<&str>, ours: &str) -> Result<Cmd, String> {
    let cmd = parse(args);
    if matches!(cmd, Ok(Cmd::Init | Cmd::InitRex | Cmd::Version | Cmd::Help | Cmd::Notification { .. } | Cmd::Next(_))) {
        return cmd;
    }
    version_mismatch(plugin, ours, args.first() == Some(&"--rex")).map_or(cmd, Err)
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

/// Where Rex reads its init.lua, by the environment GET gives: REX_CONFIG,
/// else rex/init.lua in $XDG_CONFIG_HOME, else in ~/.config.
fn rex_config(get: impl Fn(&str) -> Option<String>) -> PathBuf {
    let get = |k: &str| get(k).filter(|v| !v.is_empty());
    get("REX_CONFIG").map(PathBuf::from).unwrap_or_else(|| {
        let config = get("XDG_CONFIG_HOME").map_or_else(|| PathBuf::from(get("HOME").unwrap_or_default()).join(".config"), PathBuf::from);
        config.join("rex/init.lua")
    })
}

/// rex/paddock.lua made for binary BIN at VERSION: it runs that binary,
/// which checks it is the version the action was made for.
fn rex_plugin(bin: &Path, version: &str) -> String {
    REX_PLUGIN.replacen("local BIN = nil", &format!("local BIN = {}", lua(&bin.to_string_lossy())), 1).replacen("local VERSION = nil", &format!("local VERSION = {}", lua(version)), 1)
}

/// Install rex/paddock.lua, made for this binary, beside Rex's init.lua,
/// and say how to load it, unless init.lua does.
fn init_rex() -> Result<(), String> {
    let init = rex_config(|k| env::var(k).ok());
    let path = init.with_file_name("paddock.lua");
    let fail = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).map_err(fail)?;
    std::fs::write(&path, rex_plugin(&self_path()?, env!("CARGO_PKG_VERSION"))).map_err(fail)?;
    println!("installed {}", path.display());
    let file = path.to_string_lossy();
    if !std::fs::read_to_string(&init).is_ok_and(|text| text.contains(&*file)) {
        println!("load it from {}:\n\n  local paddock = dofile({})\n  paddock.setup{{ key = \"cmd+shift+a\" }}\n", init.display(), lua(&file));
    }
    println!("then run: rex config reload");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let plugin = env::var("PADDOCK_PLUGIN_VERSION").ok().filter(|v| !v.is_empty());
    let result = command(&args, plugin.as_deref(), env!("CARGO_PKG_VERSION")).and_then(|cmd| match cmd {
        Cmd::Run(tab) => run(tab, Server::detect()),
        Cmd::Rex(opts) => run(opts.tab, Server::Rex(opts)),
        Cmd::Init => init(),
        Cmd::InitRex => init_rex(),
        Cmd::Notification { state, kind, path, icons } => {
            println!("{}", notification::format(&state, &kind, &path, &env::var("HOME").unwrap_or_default(), icons));
            Ok(())
        }
        Cmd::Next(client) => backend::tmux::next(client.as_deref()),
        Cmd::Version => {
            println!("paddock {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Cmd::Help => {
            println!("{USAGE}");
            println!("init: bind the keys from ~/.tmux.conf with run-shell 'paddock init' (no plugin checkout needed)");
            println!("init --rex: install the Rex action (rex/paddock.lua) beside Rex's init.lua, and print how to load it");
            println!("notification: print two plain-text lines (title, path/branch); sends nothing and needs no tmux connection");
            println!("next: take the client to the agent waiting longest for you, else the one that finished longest ago (tmux)");
            Ok(())
        }
    });
    if let Err(e) = result {
        fail(&e, args.first() == Some(&"--rex") && args.contains(&"--popup"));
    }
}

/// Report E and exit. In POPUP, rex/paddock.lua's floating pane, which
/// closes with paddock, the error stays up a moment first, to be read.
fn fail(e: &str, popup: bool) -> ! {
    eprintln!("paddock: {e}");
    if popup {
        thread::sleep(Duration::from_secs(4));
    }
    std::process::exit(1);
}

/// A folder's changes that take git longer than this to tell are not asked
/// for again every minute, only when its agent changes state: one slow
/// folder would hold up every other folder's.
const SLOW_DIFF: Duration = Duration::from_millis(250);

/// How long after an agent changes state its transcript is read again.
const SETTLE: Duration = Duration::from_millis(1500);

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

/// The terminal in raw mode on the alternate screen, with mouse reporting,
/// bracketed paste and the key protocol the backend chose. Dropping it
/// restores the terminal, also when a panic unwinds (ratatui's panic hook
/// restores the rest first).
struct Tui(ratatui::DefaultTerminal, keys::Protocol);

impl Tui {
    fn new(keys: keys::Protocol) -> Result<Tui, String> {
        let terminal = ratatui::try_init().map_err(|e| format!("cannot open the terminal: {e}"))?;
        let mut out = std::io::stdout();
        let _ = write!(out, "{MOUSE_ON}{}", keys.codes().0);
        let _ = execute!(out, EnableBracketedPaste);
        Ok(Tui(terminal, keys))
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        let _ = write!(out, "{}{MOUSE_OFF}", self.1.codes().1);
        let _ = execute!(out, DisableBracketedPaste);
        ratatui::restore();
    }
}

/// When the wall clock reaches the next minute, and ages ("5m") tick over.
fn next_minute() -> Instant {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    Instant::now() + Duration::from_secs(60 - now.as_secs() % 60)
}

/// The server the popup runs on.
enum Server {
    Tmux,
    Rex(backend::rex::Opts),
}

impl Server {
    /// Rex when run in a Rex terminal outside tmux, else tmux.
    fn detect() -> Server {
        let set = |k: &str| env::var(k).is_ok_and(|v| !v.is_empty());
        if !set("TMUX") && set("REX_SESSION") { Server::Rex(backend::rex::Opts::default()) } else { Server::Tmux }
    }
}

fn run(tab: Option<Tab>, server: Server) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let events = tx.clone();
    let post = move |replied, m| {
        let _ = events.send(Ev::Backend(replied, m));
    };
    let (mut backend, start): (Box<dyn Backend>, Start) = match server {
        Server::Tmux => backend::tmux::connect(post).map(|(b, s)| (Box::new(b) as Box<dyn Backend>, s))?,
        Server::Rex(opts) => backend::rex::connect(opts, post).map(|(b, s)| (Box::new(b) as Box<dyn Backend>, s))?,
    };
    let keys = start.keys;
    let mut popup = Popup::new(backend.as_mut(), start, tab, env::var("HOME").unwrap_or_default(), tx.clone())?;

    let mut tui = Tui::new(keys)?;
    // Terminal input joins the server's notifications on one channel.
    thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if tx.send(Ev::Input(ev)).is_err() {
                break;
            }
        }
    });
    let mut view = View::new();
    // The tab to reopen on, kept as it changes: a popup closed from outside
    // (Rex's toggle key, tmux's popup) ends without returning here.
    let mut saved = popup.app.tab;
    let result = (|| -> Result<Option<Target>, String> {
        loop {
            popup.tick(backend.as_mut(), &view)?;
            if popup.dirty {
                let live = &popup.live;
                let title = live.pane.as_deref().map(|p| rows::pane_title(&popup.app.snap, p)).unwrap_or_default();
                let preview = Preview { title: &title, body: &live.body };
                tui.0.draw(|f| ui::draw(f, &popup.app, &mut view, &preview, &popup.theme)).map_err(|e| e.to_string())?;
                popup.dirty = false;
                popup.app.set_preview_fits(view.preview_fits);
                popup.app.compact = view.compact;
                if popup.app.preview_pane(&view) != popup.live.pane.as_deref() {
                    // The preview just appeared or went away: follow it now.
                    popup.dirty = true;
                    continue;
                }
            }
            let ev = match rx.recv_timeout(popup.deadline().saturating_duration_since(Instant::now())) {
                Ok(ev) => Some(ev),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            };
            if let Flow::Close(target) = popup.handle(ev, &rx, backend.as_mut(), &view)? {
                return Ok(target);
            }
            if popup.app.tab != saved {
                saved = popup.app.tab;
                backend.closed(saved);
            }
        }
    })();
    drop(tui);
    // The next bare `paddock` reopens this tab.
    backend.closed(popup.app.tab);
    if let Some(target) = result? {
        backend.activate(&target)?;
    }
    Ok(())
}

/// What the loop does after `Popup::handle`.
#[derive(Debug, PartialEq)]
enum Flow {
    Stay,
    /// Close, then show the target, if any.
    Close(Option<Target>),
}

/// The popup apart from its terminal: the app, what it shows and what it
/// waits for. `tick` comes before each draw, `handle` after each wait.
struct Popup {
    app: App,
    theme: theme::Theme,
    /// Notifications sent before the last fetch's reply are in it.
    fetched: u64,
    live: Live,
    suggest_command: String,
    suggestion: Option<Suggestion>,
    /// Where a suggestion posts its answer.
    answers: Sender<Ev>,
    minute: Instant,
    /// Something changed since the last draw.
    dirty: bool,
    /// Each settled local agent's folder's changes, and what its transcript
    /// says, off the loop's thread.
    diffs: worker::Worker<String>,
    notes: worker::Worker<(String, model::Mark)>,
    /// Each local agent's mark (state, since when, open asks), as last asked
    /// about: a change asks again.
    asked: HashMap<String, model::Mark>,
    /// When to read transcripts again after agents changed state: their
    /// tools may write the last of it just after saying so.
    settle: Option<Instant>,
    /// Folders' changes were asked for: they show on the agents tab only,
    /// so not before it is.
    diffed: bool,
    /// Folders whose changes git was slow to tell (SLOW_DIFF).
    slow: HashSet<String>,
}

impl Popup {
    /// The popup on TAB (else the last one) over BACKEND's first snapshot.
    fn new(backend: &mut dyn Backend, start: Start, tab: Option<Tab>, home: String, answers: Sender<Ev>) -> Result<Popup, String> {
        let tab = tab.or(start.last_tab).unwrap_or(Tab::Sessions);
        let can_suggest = !start.suggest_command.trim().is_empty();
        let mut app = App::new(backend.fetch()?, tab, start.session, start.window, home, can_suggest, start.hidden_expanded);
        app.caps = backend.caps();
        let (theme, fetched, live) = (start.theme, backend.replied(), Live::new());
        let to = answers.clone();
        let diffs = worker::Worker::new(
            |dir: &String| {
                let start = Instant::now();
                (diffstat::diff(dir), start.elapsed() > SLOW_DIFF)
            },
            move |dir, (diff, slow)| {
                let _ = to.send(Ev::Diff(dir, diff, slow));
            },
        );
        let to = answers.clone();
        let notes = worker::Worker::new(|(file, (state, _, asks)): &(String, model::Mark)| transcript::note(file, *state, asks), move |(file, mark), said| {
            let _ = to.send(Ev::Note(file, mark, said));
        });
        let mut popup = Popup { app, theme, fetched, live, suggest_command: start.suggest_command, suggestion: None, answers, minute: next_minute(), dirty: true, diffs, notes, asked: HashMap::new(), settle: None, diffed: false, slow: HashSet::new() };
        popup.ask(true, true, false);
        Ok(popup)
    }

    /// Ask the workers about each settled local agent (not working: its
    /// folder and transcript are on the move): ALL of them, else those new
    /// or changed in state since they were last asked about (and then again
    /// once they settle); for their folders' changes too with DIFFS, but
    /// on the MINUTE's round not for folders git is slow on.
    fn ask(&mut self, all: bool, want_diffs: bool, minute: bool) {
        let diffs = want_diffs && self.app.tab == Tab::Agents;
        // The tab's first showing: every folder.
        let all = all || (diffs && !self.diffed);
        self.diffed |= diffs;
        let mut asked = HashMap::new();
        let mut changed = false;
        for (s, _, p) in self.app.snap.unique_panes() {
            let Some(a) = p.agent.as_ref().filter(|_| s.host.is_empty()) else { continue };
            let mark = a.mark();
            if a.state != model::State::Working && (all || self.asked.get(&p.id) != Some(&mark)) {
                changed |= !all;
                if diffs && !p.path.is_empty() && !(minute && self.slow.contains(&p.path)) {
                    self.diffs.ask(p.path.clone());
                }
                if !a.transcript.is_empty() {
                    self.notes.ask((a.transcript.clone(), mark.clone()));
                }
            }
            asked.insert(p.id.clone(), mark);
        }
        self.asked = asked;
        if changed {
            self.settle = Some(Instant::now() + SETTLE);
            // Changed off the agents tab: its next showing asks every folder.
            if want_diffs && !diffs {
                self.diffed = false;
            }
        }
    }

    /// The preview follows the selection while it is on screen (the last
    /// draw says whether there was room for it): a capture that is due or
    /// in goes in the next draw. News of the connection shows too.
    fn tick(&mut self, backend: &mut dyn Backend, view: &View) -> Result<(), String> {
        self.live.follow(self.app.preview_pane(view));
        self.dirty |= self.live.tick(backend);
        if backend.is_dead() {
            return Err("the server stopped answering".into());
        }
        // Trouble stays up while it lasts (a key clears what else is said).
        if let Some(news) = backend.health() {
            match news {
                Err(trouble) => self.app.trouble = Some(trouble),
                Ok(back) => (self.app.trouble, self.app.status) = (None, Some(app::Status::Note(back))),
            }
            self.dirty = true;
        }
        Ok(())
    }

    /// When to wake with no event: the next minute (ages tick over), the
    /// preview's next capture, or a suggestion's deadline.
    fn deadline(&self) -> Instant {
        [Some(self.minute), self.live.due(), self.suggestion.as_ref().map(|s| s.deadline), self.settle].into_iter().flatten().min().unwrap_or(self.minute)
    }

    /// Handle EV (None: woken by the deadline) and everything already
    /// queued behind it on RX, then take the snapshot they call for.
    fn handle(&mut self, ev: Option<Ev>, rx: &Receiver<Ev>, backend: &mut dyn Backend, view: &View) -> Result<Flow, String> {
        let app = &mut self.app;
        let mut stale = false;
        // An op ran: its snapshot shows even when it is the one shown (a
        // moved row is followed then).
        let mut ran = false;
        let mut fresh = None;
        let mut ev = ev;
        while let Some(e) = ev {
            match e {
                Ev::Input(input) => {
                    let action = match input {
                        Event::Key(k) if k.kind != KeyEventKind::Release => app.on_key(k),
                        Event::Mouse(m) => app.on_mouse(m, view, Instant::now()),
                        Event::Paste(text) => app.paste(&text),
                        _ => Action::None,
                    };
                    let action = match action {
                        // Agents: from the session that lists the pane first.
                        // Sessions: where the user chose.
                        Action::Activate(t) if app.tab == Tab::Agents => Action::Activate(app.local(t)),
                        other => other,
                    };
                    match action {
                        Action::Quit => return Ok(Flow::Close(None)),
                        Action::Activate(t) if !app.caps.activate_open => return Ok(Flow::Close(Some(t))),
                        Action::Activate(t) => match backend.activate(&t) {
                            Ok(()) => return Ok(Flow::Close(None)),
                            Err(e) => app.status = Some(app::Status::Failed(e)),
                        },
                        Action::Run(job) => {
                            let Ran { moved, result, unsure } = backend.run(&job.op);
                            let job = if unsure { job.unsure() } else { job };
                            for session in &moved {
                                app.moved(session);
                            }
                            if let Some(created) = app.ran(job, result) {
                                if !app.caps.activate_open {
                                    return Ok(Flow::Close(Some(created)));
                                }
                                // As any jump there: a failure stays in the popup.
                                match backend.activate(&created) {
                                    Ok(()) => return Ok(Flow::Close(None)),
                                    Err(e) => app.status = Some(app::Status::Failed(e)),
                                }
                            }
                            // Show the result now, not at the next subscription
                            // check.
                            (stale, ran) = (true, true);
                            // Events still queued (a held ⇧↓) wait for that
                            // snapshot: from this one, a second swap would
                            // undo the first.
                            self.dirty = true;
                            break;
                        }
                        Action::Suggest { pane, id } => match suggest(&self.suggest_command, &pane, id, self.answers.clone()) {
                            Ok(s) => self.suggestion = Some(s),
                            Err(e) => app.suggested(id, Err(e)),
                        },
                        Action::Send(pane, input) => match backend.send(&pane, &input) {
                            // The answer shows in the preview: look soon.
                            Ok(()) => self.live.poke(),
                            Err(e) => app.status = Some(app::Status::Failed(e)),
                        },
                        Action::None => {}
                    }
                    self.dirty = true;
                }
                Ev::Diff(dir, diff, slow) => {
                    if slow {
                        self.slow.insert(dir.clone());
                    } else {
                        self.slow.remove(&dir);
                    }
                    self.dirty |= app.set_diff(dir, diff);
                }
                Ev::Note(file, mark, said) => self.dirty |= app.set_note(file, mark, said),
                Ev::Suggested(id, result) => {
                    if self.suggestion.as_ref().is_some_and(|s| s.id == id) {
                        self.suggestion = None;
                    }
                    app.suggested(id, result);
                    self.dirty = true;
                }
                Ev::Backend(_, Msg::Exit) => return Ok(Flow::Close(None)),
                // Older than the snapshot shown: the %sessions-changed
                // making SESSION sends, or a subscription value from
                // before an op, which would put back what it changed.
                Ev::Backend(replied, _) if replied < self.fetched => {}
                Ev::Backend(_, Msg::Changed) => stale = true,
                Ev::Backend(_, Msg::Captured) => {}
                Ev::Backend(_, Msg::Rows(snap)) => fresh = Some(snap),
            }
            ev = rx.try_recv().ok();
        }
        if let Some(s) = &self.suggestion
            && Instant::now() >= s.deadline
        {
            let id = s.id;
            self.suggestion = None;
            app.suggested(id, Err(format!("no title after {}s", SUGGEST_TIMEOUT.as_secs())));
            self.dirty = true;
        }
        // Stop a suggestion nobody waits for any more (cancelled, typed over).
        if self.suggestion.as_ref().is_some_and(|s| app.suggesting() != Some(s.id)) {
            self.suggestion = None;
        }
        // One snapshot for any burst of notifications: the subscription's
        // latest unless one is due now. The preview's pane is captured on
        // its own schedule.
        let snap = if stale {
            let snap = backend.fetch()?;
            self.fetched = backend.replied();
            Some(snap)
        } else {
            fresh
        };
        // A refresh that changes nothing is skipped.
        if let Some(snap) = snap.filter(|s| ran || *s != app.snap) {
            app.set_snapshot(snap);
            self.dirty = true;
            self.ask(false, true, false);
        }
        if self.settle.is_some_and(|at| Instant::now() >= at) {
            self.settle = None;
            self.ask(true, false, false);
        }
        if self.app.tab == Tab::Agents && !self.diffed {
            self.ask(true, true, false);
        }
        if Instant::now() >= self.minute {
            self.app.refresh();
            self.minute = next_minute();
            self.dirty = true;
            // Folders change by hand too, and transcripts late.
            self.ask(true, true, true);
        }
        Ok(Flow::Stay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app::{Op, Status};
    use backend::Caps;
    use model::Snapshot;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn parses_commands() {
        assert!(matches!(parse(&["init"]), Ok(Cmd::Init)));
        assert!(matches!(parse(&["init", "--rex"]), Ok(Cmd::InitRex)));
        assert!(matches!(parse(&["--rex", "--tab", "agents"]), Ok(Cmd::Rex(o)) if o.tab == Some(Tab::Agents)));
        assert!(parse(&["--rex", "--nope"]).is_err());
        assert!(parse(&["init", "x"]).is_err());
        assert!(matches!(parse(&["next"]), Ok(Cmd::Next(None))));
        assert!(matches!(command(&["next", "-c", "/dev/ttys1"], Some("old"), "new"), Ok(Cmd::Next(Some(c))) if c == "/dev/ttys1"), "no version check");
        assert!(parse(&["next", "x"]).is_err());
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
    fn notification_command_is_validated_and_needs_no_popup_bindings() {
        for state in ["finished", "asking", "waiting", "failed"] {
            assert!(matches!(command(&["notification", state, "claude", "/tmp/project"], Some("old"), "new"), Ok(Cmd::Notification { icons: theme::AgentIcons::Unicode, .. })));
            assert!(matches!(parse(&["notification", state, "codex", "/tmp/project", "nerd"]), Ok(Cmd::Notification { icons: theme::AgentIcons::Nerd, .. })));
        }
        assert!(parse(&["notification", "invented", "pi", "/tmp"]).is_err());
        assert!(parse(&["notification", "finished", "pi", "/tmp", "invented"]).is_err());
        assert!(parse(&["notification", "finished", "pi"]).is_err());
        assert!(matches!(parse(&["notification", "finished", "pi", "-literal-path"]), Ok(Cmd::Notification { .. })));
    }

    #[test]
    fn embedded_plugin_is_the_init_mode_script() {
        assert!(PLUGIN.contains("PADDOCK_VERSION"));
        assert!(PLUGIN.contains("PADDOCK_BIN"));
    }

    #[test]
    fn handshake_refuses_other_versions() {
        assert_eq!(version_mismatch(None, "0.1.0", false), None);
        assert_eq!(version_mismatch(Some("0.1.0"), "0.1.0", false), None);
        // Stale bindings, an upgraded binary: a reload rebinds them.
        let e = version_mismatch(Some("0.9.0"), "0.10.0", false).unwrap();
        assert_eq!(e, "key bindings for paddock 0.9.0, but this is 0.10.0\nreload tmux to rebind the keys");
        // An old binary (PADDOCK_BIN, or not rebuilt): reloading does nothing.
        let e = version_mismatch(Some("0.3.0"), "0.2.0", false).unwrap();
        assert_eq!(e, "key bindings for paddock 0.3.0, but this is 0.2.0\ninstall paddock 0.3.0 (or point PADDOCK_BIN at it)");
        let e = version_mismatch(Some("0.3.0-rc1"), "0.3.0", false).unwrap();
        assert!(e.ends_with("\nreload tmux, or install paddock 0.3.0-rc1"), "{e}");
        // On Rex, the copy `paddock init --rex` installed.
        let e = version_mismatch(Some("0.9.0"), "0.10.0", true).unwrap();
        assert_eq!(e, "paddock.lua for paddock 0.9.0, but this is 0.10.0\nrun paddock init --rex to update it");
        let e = version_mismatch(Some("0.3.0"), "0.2.0", true).unwrap();
        assert!(e.ends_with("\ninstall paddock 0.3.0 (or set bin in paddock.setup)"), "{e}");
        // A popup 80% of 80 columns wide shows each line unwrapped.
        for (p, o) in [("10.10.10", "10.10.9"), ("10.10.9", "10.10.10")] {
            for rex in [false, true] {
                for line in version_mismatch(Some(p), o, rex).unwrap().lines() {
                    assert!("paddock: ".len() + line.len() <= 64, "{line}");
                }
            }
        }
    }

    #[test]
    fn version_is_checked_before_arguments() {
        let mismatch = version_mismatch(Some("0.9.0"), "0.8.0", false);
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
        assert!(matches!(command(&["init", "--rex"], Some("0.9.0"), "0.8.0"), Ok(Cmd::InitRex)));
        // rex/paddock.lua's command line too.
        let mismatch = version_mismatch(Some("0.9.0"), "0.8.0", true);
        assert_eq!(command(&["--rex", "--future"], Some("0.9.0"), "0.8.0").err(), mismatch);
        assert!(matches!(command(&["--rex", "--popup"], Some("0.8.0"), "0.8.0"), Ok(Cmd::Rex(_))));
        assert!(matches!(command(&["--rex", "--popup"], None, "0.8.0"), Ok(Cmd::Rex(_))), "a checkout's copy has no version");
    }

    #[test]
    fn the_rex_action_is_made_for_this_binary() {
        let lua = rex_plugin(Path::new("/opt/homebrew/bin/paddock"), "0.8.0");
        assert!(lua.contains("\nlocal BIN = \"/opt/homebrew/bin/paddock\"\nlocal VERSION = \"0.8.0\"\n"), "{lua}");
        assert!(!lua.contains(" = nil\n") && REX_PLUGIN.contains("\nlocal BIN = nil\nlocal VERSION = nil\n"));
        assert!(rex_plugin(Path::new("/a \"b\""), "1").contains(r#"local BIN = "/a \"b\"""#), "quoted for Lua");
        // Beside Rex's init.lua.
        let env = |vars: &'static [(&str, &str)]| move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert_eq!(rex_config(env(&[("HOME", "/h")])), Path::new("/h/.config/rex/init.lua"));
        assert_eq!(rex_config(env(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "/x"), ("REX_CONFIG", "")])), Path::new("/x/rex/init.lua"));
        assert_eq!(rex_config(env(&[("HOME", "/h"), ("REX_CONFIG", "/c/my.lua")])), Path::new("/c/my.lua"));
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

    /// A server the test scripts: snapshots are fixture lines, the next of
    /// `after` once an op runs. It records what it is asked to do.
    #[derive(Default)]
    struct Fake {
        caps: Caps,
        text: String,
        after: Vec<String>,
        fetches: u64,
        ops: Vec<Op>,
        captured: Vec<String>,
        activated: Vec<Target>,
        /// Why `activate` fails, if it does.
        refuse: Option<String>,
        /// What `health` tells of next.
        news: Option<Result<String, String>>,
        /// Why `run` fails, if it does, and whether without an answer.
        fail: Option<String>,
        unsure: bool,
        /// What `run` made, if it succeeds.
        made: Option<Target>,
        /// What was typed into which pane.
        sent: Vec<(String, keys::Input)>,
    }

    impl Backend for Fake {
        fn caps(&self) -> Caps {
            self.caps
        }

        fn fetch(&mut self) -> Result<Snapshot, String> {
            self.fetches += 1;
            Ok(rows(&self.text))
        }

        fn replied(&self) -> u64 {
            self.fetches
        }

        fn is_dead(&self) -> bool {
            false
        }

        fn health(&mut self) -> Option<Result<String, String>> {
            self.news.take()
        }

        fn capture(&mut self, pane: &str) -> Option<(Result<Vec<String>, String>, Duration)> {
            self.captured.push(pane.into());
            Some((Ok(vec![pane.into()]), Duration::ZERO))
        }

        fn run(&mut self, op: &Op) -> Ran {
            self.ops.push(op.clone());
            if !self.after.is_empty() {
                self.text = self.after.remove(0);
            }
            Ran { moved: Vec::new(), result: self.fail.clone().map_or_else(|| Ok(self.made.clone()), Err), unsure: self.fail.is_some() && self.unsure }
        }

        fn send(&mut self, pane: &str, input: &keys::Input) -> Result<(), String> {
            self.sent.push((pane.into(), input.clone()));
            self.fail.clone().map_or(Ok(()), Err)
        }

        fn closed(&mut self, _: Tab) {}

        fn activate(&mut self, target: &Target) -> Result<(), String> {
            self.activated.push(target.clone());
            self.refuse.clone().map_or(Ok(()), Err)
        }
    }

    /// The snapshot of fixture lines TEXT.
    fn rows(text: &str) -> Snapshot {
        backend::tmux::fixture(&text.lines().collect::<Vec<_>>())
    }

    /// Session $1 with windows NAMES (`@N:name`), in that order, a pane each.
    fn windows(names: &[&str]) -> String {
        let line = |(i, w): (usize, &&str)| {
            let (id, name) = w.split_once(':').unwrap();
            format!("$1|s|{id}|{}|{name}|{}|0|%{}|0|1|zsh|||||||||/a", i + 1, u8::from(i == 0), &id[1..])
        };
        names.iter().enumerate().map(line).collect::<Vec<_>>().join("\n")
    }

    /// The popup over FAKE's snapshot, opened from window @1 of $1, its
    /// selection on TARGET; and the channel its events come on.
    fn popup(fake: &mut Fake, target: Target) -> (Popup, Sender<Ev>, Receiver<Ev>) {
        let (tx, rx) = mpsc::channel();
        let start = Start { last_tab: None, theme: theme::Theme::default(), suggest_command: String::new(), hidden_expanded: false, session: "$1".into(), window: "@1".into(), keys: keys::Protocol::Legacy };
        let mut p = Popup::new(fake, start, Some(Tab::Sessions), String::new(), tx.clone()).unwrap();
        p.app.selected = p.app.rows.iter().position(|r| r.target.as_ref() == Some(&target)).unwrap();
        (p, tx, rx)
    }

    fn window(id: &str) -> Target {
        Target::Window { session: "$1".into(), window: id.into() }
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Option<Ev> {
        Some(Ev::Input(Event::Key(KeyEvent::new(code, modifiers))))
    }

    /// The windows of session $1 the popup shows, in order.
    fn shown(p: &Popup) -> Vec<&str> {
        p.app.snap.sessions[0].windows.iter().map(|w| w.id.as_str()).collect()
    }

    #[test]
    fn an_ops_result_shows_at_once_and_older_news_is_ignored() {
        let mut fake = Fake { text: windows(&["@1:a", "@2:b"]), after: vec![windows(&["@2:b", "@1:a"])], ..Fake::default() };
        let (mut p, _tx, rx) = popup(&mut fake, window("@1"));
        let before = rows(&fake.text);
        let view = View::new();
        assert_eq!(p.handle(key(KeyCode::Down, KeyModifiers::SHIFT), &rx, &mut fake, &view), Ok(Flow::Stay));
        assert_eq!(fake.ops, [Op::SwapWindows { session: "$1".into(), a: "@1".into(), b: "@2".into() }]);
        assert_eq!(shown(&p), ["@2", "@1"], "a snapshot taken after the op, without waiting for news");
        assert_eq!(p.app.status, Some(Status::Done("moved down   ^z undo".into())));
        assert_eq!(p.app.selected_target(), Some(&window("@1")), "the moved row is followed");
        // A subscription value sent before that snapshot would put the swap back.
        assert_eq!(p.handle(Some(Ev::Backend(p.fetched - 1, Msg::Rows(before))), &rx, &mut fake, &view), Ok(Flow::Stay));
        assert_eq!(shown(&p), ["@2", "@1"]);
        // One sent since is news.
        let news = rows(&windows(&["@2:b", "@1:a", "@3:c"]));
        p.handle(Some(Ev::Backend(p.fetched, Msg::Rows(news))), &rx, &mut fake, &view).unwrap();
        assert_eq!(shown(&p), ["@2", "@1", "@3"]);
    }

    #[test]
    fn a_held_key_repeats_its_op_from_the_snapshot_after_the_last() {
        let mut fake = Fake { text: windows(&["@1:a", "@2:b", "@3:c"]), after: vec![windows(&["@2:b", "@1:a", "@3:c"]), windows(&["@2:b", "@3:c", "@1:a"])], ..Fake::default() };
        let (mut p, tx, rx) = popup(&mut fake, window("@1"));
        let view = View::new();
        // Both presses are queued before the loop wakes.
        let down = || key(KeyCode::Down, KeyModifiers::SHIFT);
        tx.send(down().unwrap()).unwrap();
        p.handle(down(), &rx, &mut fake, &view).unwrap();
        assert_eq!(fake.ops.len(), 1, "the second press waits for the first one's snapshot");
        p.handle(rx.try_recv().ok(), &rx, &mut fake, &view).unwrap();
        let swap = |a: &str, b: &str| Op::SwapWindows { session: "$1".into(), a: a.into(), b: b.into() };
        assert_eq!(fake.ops, [swap("@1", "@2"), swap("@1", "@3")], "moved down twice, not down and back");
        assert_eq!(shown(&p), ["@2", "@3", "@1"]);
    }

    #[test]
    fn the_preview_follows_the_selection() {
        let mut fake = Fake { text: windows(&["@1:a", "@2:b"]), ..Fake::default() };
        let (mut p, _tx, rx) = popup(&mut fake, window("@1"));
        let view = View::new();
        p.tick(&mut fake, &view).unwrap();
        assert_eq!(fake.captured, ["%1"]);
        assert_eq!(p.live.pane.as_deref(), Some("%1"));
        p.handle(key(KeyCode::Down, KeyModifiers::NONE), &rx, &mut fake, &view).unwrap();
        p.tick(&mut fake, &view).unwrap();
        assert_eq!(fake.captured, ["%1", "%2"], "captured at once, not at the next due time");
        assert!(p.dirty);
        // Off: nothing is captured.
        p.app.preview = Some(false);
        p.tick(&mut fake, &view).unwrap();
        assert_eq!((fake.captured.len(), p.live.pane.as_deref()), (2, None));
    }

    #[test]
    fn closing_and_opening_go_by_what_the_server_can_do() {
        let view = View::new();
        let mut fake = Fake { text: windows(&["@1:a", "@2:b"]), ..Fake::default() };
        let (mut p, _tx, rx) = popup(&mut fake, window("@2"));
        assert_eq!(p.handle(key(KeyCode::Esc, KeyModifiers::NONE), &rx, &mut fake, &view), Ok(Flow::Close(None)));
        // tmux: the popup closes, then the target is shown.
        assert_eq!(p.handle(key(KeyCode::Enter, KeyModifiers::NONE), &rx, &mut fake, &view), Ok(Flow::Close(Some(window("@2")))));
        assert!(fake.activated.is_empty());
        assert_eq!(p.handle(Some(Ev::Backend(0, Msg::Exit)), &rx, &mut fake, &view), Ok(Flow::Close(None)), "the server went");
        // Rex: shown first, so a failure stays in the popup to be read.
        fake.caps.activate_open = true;
        fake.refuse = Some("turn on Remote Control".into());
        let (mut p, _tx, rx) = popup(&mut fake, window("@2"));
        assert_eq!(p.handle(key(KeyCode::Enter, KeyModifiers::NONE), &rx, &mut fake, &view), Ok(Flow::Stay));
        assert_eq!(p.app.status, Some(Status::Failed("turn on Remote Control".into())));
        fake.refuse = None;
        assert_eq!(p.handle(key(KeyCode::Enter, KeyModifiers::NONE), &rx, &mut fake, &view), Ok(Flow::Close(None)));
        assert_eq!(fake.activated, [window("@2"), window("@2")]);
    }

    #[test]
    fn what_is_created_is_gone_to_as_any_target_is() {
        let view = View::new();
        let create = |p: &mut Popup, fake: &mut Fake, rx: &Receiver<Ev>| {
            for (code, mods) in [(KeyCode::Char('s'), KeyModifiers::CONTROL), (KeyCode::Char('n'), KeyModifiers::NONE)] {
                p.handle(key(code, mods), rx, fake, &view).unwrap();
            }
            p.handle(key(KeyCode::Enter, KeyModifiers::NONE), rx, fake, &view)
        };
        // tmux: once the popup has closed.
        let mut fake = Fake { text: windows(&["@1:a"]), made: Some(window("@9")), ..Fake::default() };
        let (mut p, _tx, rx) = popup(&mut fake, window("@1"));
        assert_eq!(create(&mut p, &mut fake, &rx), Ok(Flow::Close(Some(window("@9")))));
        // Rex: first, so a failure stays in the popup to be read.
        fake.caps.activate_open = true;
        fake.refuse = Some("turn on Remote Control".into());
        let (mut p, _tx, rx) = popup(&mut fake, window("@1"));
        assert_eq!(create(&mut p, &mut fake, &rx), Ok(Flow::Stay));
        assert_eq!(p.app.status, Some(Status::Failed("turn on Remote Control".into())));
        assert_eq!(fake.activated, [window("@9")]);
    }

    #[test]
    fn a_refused_op_opens_its_editor_again_unless_its_answer_was_lost() {
        let view = View::new();
        for (unsure, reopens) in [(false, true), (true, false)] {
            let mut fake = Fake { text: windows(&["@1:a", "@2:b"]), fail: Some("no".into()), unsure, ..Fake::default() };
            let (mut p, _tx, rx) = popup(&mut fake, window("@1"));
            p.handle(key(KeyCode::Char('s'), KeyModifiers::CONTROL), &rx, &mut fake, &view).unwrap();
            p.handle(key(KeyCode::Char('n'), KeyModifiers::NONE), &rx, &mut fake, &view).unwrap();
            p.handle(key(KeyCode::Enter, KeyModifiers::NONE), &rx, &mut fake, &view).unwrap();
            assert!(matches!(fake.ops[..], [Op::NewWindow { .. }]));
            assert_eq!(matches!(p.app.mode, app::Mode::Create(_)), reopens, "unsure {unsure}: {:?}", p.app.status);
        }
    }

    #[test]
    fn a_servers_trouble_stays_up_while_it_lasts() {
        let mut fake = Fake { text: windows(&["@1:a", "@2:b"]), ..Fake::default() };
        let (mut p, _tx, rx) = popup(&mut fake, window("@1"));
        let view = View::new();
        fake.news = Some(Err("devbox stopped answering (trying again)".into()));
        p.tick(&mut fake, &view).unwrap();
        assert!(p.dirty && p.app.trouble.is_some());
        // A key clears what else was said, not that: the list may be stale.
        p.handle(key(KeyCode::Down, KeyModifiers::NONE), &rx, &mut fake, &view).unwrap();
        p.tick(&mut fake, &view).unwrap();
        assert_eq!((p.app.trouble.as_deref(), &p.app.status), (Some("devbox stopped answering (trying again)"), &None));
        fake.news = Some(Ok("every server is answering again".into()));
        p.tick(&mut fake, &view).unwrap();
        assert_eq!((&p.app.trouble, &p.app.status), (&None, &Some(Status::Note("every server is answering again".into()))));
    }

    #[test]
    fn a_suggestion_nobody_waits_for_is_dropped() {
        let mut fake = Fake { text: windows(&["@1:a"]), ..Fake::default() };
        let (mut p, _tx, rx) = popup(&mut fake, window("@1"));
        let view = View::new();
        // An answer to a suggestion never asked for changes nothing but the draw.
        p.dirty = false;
        p.handle(Some(Ev::Suggested(7, Ok("title".into()))), &rx, &mut fake, &view).unwrap();
        assert!(p.dirty && p.suggestion.is_none());
        assert_eq!(fake.fetches, 1, "no snapshot without news");
    }

    #[test]
    fn what_is_typed_goes_to_the_pane_and_a_refusal_says_why() {
        let mut fake = Fake { text: windows(&["@1:a", "@2:b"]), ..Fake::default() };
        let (mut p, _tx, rx) = popup(&mut fake, window("@2"));
        let view = View::new();
        p.handle(key(KeyCode::Char('l'), KeyModifiers::CONTROL), &rx, &mut fake, &view).unwrap();
        p.handle(key(KeyCode::Char('y'), KeyModifiers::NONE), &rx, &mut fake, &view).unwrap();
        p.handle(Some(Ev::Input(Event::Paste("two\nlines".into()))), &rx, &mut fake, &view).unwrap();
        assert_eq!(fake.sent, [("%2".to_string(), keys::Input::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))), ("%2".to_string(), keys::Input::Paste("two\nlines".into()))]);
        fake.fail = Some("the pane is gone".into());
        p.handle(key(KeyCode::Enter, KeyModifiers::NONE), &rx, &mut fake, &view).unwrap();
        assert_eq!(p.app.status, Some(Status::Failed("the pane is gone".into())));
        assert!(fake.ops.is_empty() && fake.activated.is_empty(), "⏎ went to the pane, not to a jump");
    }
}

//! paddock: a tmux popup for sessions, windows, panes and the AI agents in
//! them. See USAGE.
//!
//! The popup binding passes the invoking client and location as
//! PADDOCK_CLIENT, PADDOCK_SESSION and PADDOCK_WINDOW; paddock never guesses a
//! client, since another terminal may become active while it is open.
//!
//! Event-driven: one channel carries terminal input and tmux notifications,
//! the loop sleeps until one arrives or the live preview's next capture is
//! due, and draws only when something changed.

mod app;
mod live;
mod model;
mod rows;
mod theme;
mod tmux;
mod ui;

use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use std::io::Write;

use ratatui::crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind};
use ratatui::crossterm::execute;

use app::{Action, App, Op, Tab, View};
use live::Live;
use model::{snapshot, Descriptions, Target};
use theme::Theme;
use tmux::{field, quote, Control, Msg, SEP};
use ui::Preview;

/// Refresh triggers, checked by tmux about once a second: every pane in
/// every session (a control client's %* covers only its own session) with
/// what paddock shows of it, and agent-status's revision token (bumped when a
/// description file changes). Structural changes also arrive at once as
/// notifications (windows added, closed, renamed...). Each value is a
/// `field`, so a newline in it cannot end the notification line.
fn subscriptions() -> [String; 2] {
    let f = |names: &[&str]| names.iter().map(|n| field(n)).collect::<String>();
    let panes = f(&["pane_id", "pane_active", "pane_current_command", "pane_current_path", "@agent-state", "@agent-unseen", "@agent-state-at", "@agent-kind", "@agent-auto-description"]);
    let windows = f(&["window_index", "window_name", "window_active"]);
    let all = format!("panes::#{{S:{}#{{W:{windows}#{{P:{panes}}}}}}}", f(&["session_name"]));
    [all, format!("revision::{}", f(&["@agent-revision"]))].map(|s| format!("refresh-client -B {}", quote(&s)))
}

enum Ev {
    Input(Event),
    Tmux(Msg),
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
    let out = std::process::Command::new("tmux")
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

const USAGE: &str = "usage: paddock [--tab sessions|agents] | paddock --version";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        [] | ["--tab", "sessions"] => run(Tab::Sessions),
        ["--tab", "agents"] => run(Tab::Agents),
        ["--version" | "-V"] => {
            println!("paddock {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        ["--help" | "-h"] => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    };
    if let Err(e) = result {
        eprintln!("paddock: {e}");
        std::process::exit(1);
    }
}

/// Switch CLIENT to TARGET in one command: a pane target also selects the
/// pane, and -Z keeps a zoomed window zoomed (select-pane would unzoom it).
fn activate(tmux: &mut Control, client: &str, target: &Target) -> Result<(), String> {
    tmux.cmd(&format!("switch-client -Z -c {} -t {}", quote(client), target.arg())).map(drop)
}

/// Carry out OP with agent-status.tmux's describe.sh (which bumps
/// @agent-revision, so the change shows up like any other).
fn run_op(descriptions: &Descriptions, op: &Op) -> Result<(), String> {
    let script = descriptions.script().ok_or("needs agent-status.tmux with scripts/agent/describe.sh")?;
    let args = match op {
        Op::Describe { pane, text } => vec!["set", pane, text],
        Op::Generate { pane } => vec!["generate", pane],
    };
    let out = std::process::Command::new(&script)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{}: {e}", script.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Mouse reporting for presses and the wheel only (SGR encoding): no motion
/// events, which would each cost a frame.
const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1006h";
const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1000l";

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

fn connect(session: &str, tx: mpsc::Sender<Ev>) -> Result<Control, String> {
    let mut tmux = Control::connect(session, move |m| {
        let _ = tx.send(Ev::Tmux(m));
    })?;
    for s in subscriptions() {
        tmux.cmd(&s).map_err(|e| format!("cannot attach to tmux session {session}: {e}"))?;
    }
    Ok(tmux)
}

fn run(tab: Tab) -> Result<(), String> {
    let ctx = context()?;
    let (tx, rx) = mpsc::channel();
    let mut tmux = connect(&ctx.session, tx.clone())?;
    let descriptions = Descriptions::load(&mut tmux);
    let theme = Theme::load(&mut tmux);
    let home = std::env::var("HOME").unwrap_or_default();
    let debug = std::env::var_os("PADDOCK_DEBUG").is_some();
    let can_describe = descriptions.script().is_some();
    let mut app = App::new(snapshot(&mut tmux, &descriptions)?, tab, ctx.session.clone(), ctx.window.clone(), home, debug, can_describe);

    let mut tui = Tui::new()?;
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
        loop {
            // The preview follows the selection while it is on screen (the
            // last draw says whether there was room for it).
            live.follow(app.preview_pane(&view));
            dirty |= live.tick(&mut tmux);
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

            let deadline = live.due().map_or(minute, |d| d.min(minute));
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
                            Event::Mouse(m) => app.on_mouse(m, &view),
                            Event::Paste(text) => {
                                app.paste(&text);
                                Action::None
                            }
                            _ => Action::None,
                        };
                        match action {
                            Action::Quit => return Ok(None),
                            Action::Activate(t) => return Ok(Some(t)),
                            Action::Run(op) => {
                                if let Err(e) = run_op(&descriptions, &op) {
                                    app.status = format!("paddock: {e}");
                                }
                                // Show the result now, not at the next subscription check.
                                stale = true;
                            }
                            Action::None => {}
                        }
                        dirty = true;
                    }
                    Ev::Tmux(Msg::Changed) => stale = true,
                    Ev::Tmux(Msg::Exit) => return Ok(None),
                }
                ev = rx.try_recv().ok();
            }
            if stale {
                // One snapshot for any burst of notifications.
                app.set_snapshot(snapshot(&mut tmux, &descriptions)?);
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
    if let Some(target) = result?
        && let Err(e) = activate(&mut tmux, &ctx.client, &target)
    {
        // display-message expands formats: keep a # in the error literal.
        let _ = tmux.cmd(&format!("display-message -c {} {}", quote(&ctx.client), quote(&format!("paddock: {e}").replace('#', "##"))));
    }
    Ok(())
}

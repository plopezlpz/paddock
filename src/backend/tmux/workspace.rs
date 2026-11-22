//! Durable tmux bookmarks. Only typed resume tokens can become commands;
//! a pane's command line, output, environment and transcript are never saved.
use super::control::{SESSION, tmux_command};
use crate::model::is_shell;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Restore builds a session under this prefix, so a partial layout never
/// takes the saved name. Save skips any that an interrupted restore left.
const STAGE: &str = "_paddock_restore_";
/// What a pane runs until restore starts its shell or agent.
const PLACEHOLDER: &str = "86400";

pub enum Action {
    Save(String),
    /// With true, as autosave restores at tmux's start: see Taken::Start.
    Restore(String, bool),
    List,
    Remove(String),
}

impl Action {
    pub fn named(op: &str, name: &str) -> Result<Self, String> {
        if !token(name) {
            return Err("workspace name must start with a letter or digit and contain only letters, digits, '.', '_' or '-' (max 128)".into());
        }
        Ok(match op {
            "save" => Self::Save(name.into()),
            "restore" => Self::Restore(name.into(), false),
            "remove" => Self::Remove(name.into()),
            _ => unreachable!(),
        })
    }

    /// `restore --at-start NAME`, the restore autosave runs at tmux's start.
    pub fn at_start(name: &str) -> Result<Self, String> {
        Self::named("restore", name).map(|_| Self::Restore(name.into(), true))
    }

    pub fn run(self) -> Result<(), String> {
        self.apply(&state_dir()?).map_err(|e| e.to_string())
    }

    fn apply(self, dir: &Path) -> io::Result<()> {
        let path = |name: &str| dir.join(format!("{name}.json"));
        match self {
            Self::List => {
                let entries = match fs::read_dir(dir) {
                    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                    other => other?,
                };
                let mut names = Vec::new();
                for entry in entries {
                    let p = entry?.path();
                    if p.extension().is_some_and(|x| x == "json")
                        && let Some(s) = p.file_stem().and_then(|x| x.to_str()).filter(|s| token(s))
                    {
                        names.push(s.to_owned());
                    }
                }
                names.sort();
                for n in names {
                    println!("{n}");
                }
            }
            Self::Remove(name) => {
                fs::remove_file(path(&name))
                    .map_err(|e| io::Error::other(format!("remove workspace {name}: {e}")))?;
                println!("removed {name}");
            }
            Self::Save(name) => {
                let snapshot = capture()?;
                snapshot.check()?;
                DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
                let tmp = dir.join(format!(".save-{}", std::process::id()));
                // No live process shares our PID: a file by this name is a crash's leftover.
                let _ = fs::remove_file(&tmp);
                let mut f = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&tmp)?;
                let mut commit = || -> io::Result<()> {
                    serde_json::to_writer_pretty(&mut f, &snapshot)?;
                    f.write_all(b"\n")?;
                    f.sync_all()?;
                    fs::rename(&tmp, path(&name))
                };
                if let Err(e) = commit() {
                    let _ = fs::remove_file(tmp);
                    return Err(e);
                }
                println!("saved {name} ({} tmux sessions)", snapshot.sessions.len());
            }
            Self::Restore(name, at_start) => {
                let file = fs::File::open(path(&name))
                    .map_err(|e| io::Error::other(format!("open workspace {name}: {e}")))?;
                let snapshot: Workspace = serde_json::from_reader(file)?;
                snapshot.check()?;
                clean_stages();
                // One broken session does not keep the others from coming back.
                let mut failed = Vec::new();
                // Saved name -> the name it came back as.
                let mut created = HashMap::new();
                let saved: HashSet<&str> = snapshot.sessions.iter().map(|s| s.name.as_str()).collect();
                for s in &snapshot.sessions {
                    match restore(s, &created, &saved, at_start) {
                        Ok(Some(name)) => {
                            created.insert(s.name.as_str(), name);
                        }
                        Ok(None) => {}
                        Err(e) => {
                            eprintln!("{}: {e}", s.name);
                            failed.push(s.name.as_str());
                        }
                    }
                }
                if !failed.is_empty() {
                    return Err(io::Error::other(format!("not restored: {}", failed.join(", "))));
                }
            }
        }
        Ok(())
    }
}

/// How often @paddock-autosave saves: minutes, or seconds with an `s`
/// suffix, at most a day. Anything else (off, 0, empty) turns it off.
fn interval(value: &str) -> Option<Duration> {
    let (n, unit) = value.strip_suffix('s').map_or((value, 60), |n| (n, 1));
    let secs = n.parse::<u64>().ok()? * unit;
    (1..=86_400).contains(&secs).then(|| Duration::from_secs(secs))
}

/// The bookmark autosave keeps: `auto` for tmux's default socket, and
/// `auto-NAME` for another (`tmux -L NAME`), with any character but a
/// letter, digit or `-` as `_`: never a `.`, so never one of its copies
/// (`auto.previous`).
fn auto_name(socket: &str) -> String {
    match Path::new(socket).file_name().map(|n| n.to_string_lossy()) {
        None => "auto".into(),
        Some(n) if n == "default" => "auto".into(),
        Some(n) => {
            let n: String = n.chars().take(100).map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
            format!("auto-{n}")
        }
    }
}

/// `paddock autosave`, which paddock init starts in the background when
/// @paddock-autosave is set. Once per server, within two minutes of its
/// start, it restores the server's auto bookmark (kept first as
/// NAME.previous, and as NAME.unrestored if that fails); then it saves the
/// bookmark at each interval. One job runs per server: the
/// @paddock-autosave-owner option names it, and a job that no longer owns
/// it, or finds autosave off, stops.
pub fn autosave(bin: &Path) -> Result<(), String> {
    let dir = state_dir()?;
    let logs = dir.parent().unwrap_or(&dir).to_path_buf();
    let me = format!("{} {}", std::process::id(), env!("CARGO_PKG_VERSION"));
    let read = |format: &str| -> Option<Vec<String>> {
        let out = tmux(&["display-message", "-p", format]).ok()?;
        Some(out.split('\x1f').map(str::to_owned).collect())
    };
    let Some(info) = read("#{socket_path}\x1f#{start_time}\x1f#{@paddock-autosave}\x1f#{@paddock-autosave-owner}") else {
        return Ok(());
    };
    let [socket, started, every, owner] = &info[..] else { return Ok(()) };
    if interval(every).is_none() {
        return Ok(());
    }
    // Another job of this version, still running, already keeps this server.
    let running = owner
        .split_once(' ')
        .is_some_and(|(pid, version)| version == env!("CARGO_PKG_VERSION") && pid.parse().is_ok_and(alive));
    if running {
        return Ok(());
    }
    tmux(&["set-option", "-g", "@paddock-autosave-owner", &me]).map_err(|e| e.to_string())?;
    DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| e.to_string())?;
    let name = auto_name(socket);
    // Its output, private as the bookmarks are, in NAME.save.log and
    // NAME.restore.log (the last run's). True if it succeeded.
    let run = |args: &[&str], log: &str| {
        let open = || OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(logs.join(format!("{name}.{log}.log")));
        // One open file for both: two would each write from the start.
        let Ok(out) = open() else { return false };
        let Ok(err) = out.try_clone() else { return false };
        Command::new(bin).args(args).stdin(Stdio::null()).stdout(out).stderr(err).status().is_ok_and(|s| s.success())
    };
    let now = now();
    let fresh = started.parse::<u64>().is_ok_and(|t| now.saturating_sub(t) <= 120);
    // -o: only one job, of any started at once, claims the restore.
    if fresh && tmux(&["set-option", "-go", "@paddock-autorestored", "1"]).is_ok() {
        let file = dir.join(format!("{name}.json"));
        if file.exists() {
            // Before any save of this server replaces it.
            let _ = fs::copy(&file, dir.join(format!("{name}.previous.json")));
            if !run(&["restore", "--at-start", &name], "restore") {
                let _ = fs::copy(&file, dir.join(format!("{name}.unrestored.json")));
            }
        }
    }
    let mut every = every.clone();
    while let Some(wait) = interval(&every) {
        std::thread::sleep(wait);
        // Gone (the server exited), or taken over by a newer job.
        let Some(now) = read("#{@paddock-autosave}\x1f#{@paddock-autosave-owner}") else { return Ok(()) };
        if now.get(1) != Some(&me) {
            return Ok(());
        }
        every = now[0].clone();
        if interval(&every).is_some() {
            run(&["save", &name], "save");
        }
    }
    // Turned off: no owner left behind, so turning it on again starts a job.
    let _ = tmux(&["set-option", "-gu", "@paddock-autosave-owner"]);
    Ok(())
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn state_dir() -> Result<PathBuf, String> {
    Ok(env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .ok_or("neither XDG_STATE_HOME nor HOME is set")?
        .join("paddock/workspaces"))
}

fn token(s: &str) -> bool {
    s.len() <= 128
        && s.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

#[derive(Serialize, Deserialize)]
struct Workspace {
    version: u32,
    sessions: Vec<Session>,
}
#[derive(Serialize, Deserialize)]
struct Session {
    name: String,
    /// A grouped session (`new-session -t`) shares an earlier session's
    /// windows, so it saves none of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    group: Option<Group>,
    /// The session's start directory (new windows open there); empty: unknown.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    path: String,
    windows: Vec<Window>,
}
#[derive(Serialize, Deserialize)]
struct Group {
    leader: String,
    /// This session's current window.
    window: u32,
}
#[derive(Serialize, Deserialize)]
struct Window {
    index: u32,
    name: String,
    active: bool,
    layout: String,
    width: u32,
    height: u32,
    /// Window options set apart from the global value; None inherits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    size_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    automatic_rename: Option<bool>,
    zoomed: bool,
    panes: Vec<Pane>,
}
#[derive(Serialize, Deserialize)]
struct Pane {
    cwd: String,
    active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    resume: Option<Resume>,
}
#[derive(Serialize, Deserialize)]
struct Resume {
    agent: Agent,
    id: String,
}
#[derive(Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Agent {
    Claude,
    Pi,
    Codex,
    Agy,
}
impl Agent {
    fn bin(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Pi => "pi",
            Self::Codex => "codex",
            Self::Agy => "agy",
        }
    }

    fn from_bin(bin: &str) -> Option<Self> {
        [Self::Claude, Self::Pi, Self::Codex, Self::Agy].into_iter().find(|a| a.bin() == bin)
    }

    /// The command line before the ID.
    fn resume(self) -> &'static str {
        match self {
            Self::Claude => "claude --resume",
            Self::Pi => "pi --session",
            Self::Codex => "codex --no-daemon resume",
            Self::Agy => "agy --conversation",
        }
    }
}

// agent-status currently scopes @agent-instance as kind:id, and for agy as
// antigravity:pid:conversationId. Keep that producer detail here only.
fn identity(
    kind: &str,
    instance: &str,
    cmd: &str,
    registered: &str,
    stale: &str,
    dead: &str,
) -> Option<Resume> {
    let agent = match kind {
        "claude" => Agent::Claude,
        "pi" => Agent::Pi,
        "codex" => Agent::Codex,
        "antigravity" => Agent::Agy,
        _ => return None,
    };
    if stale == "1"
        || dead == "1"
        || is_shell(cmd)
        || cmd
            != if registered.is_empty() {
                agent.bin()
            } else {
                registered
            }
    {
        return None;
    }
    let (scope, mut id) = instance.split_once(':')?;
    if scope != kind {
        return None;
    }
    if matches!(agent, Agent::Agy) {
        let (pid, conversation) = id.split_once(':')?;
        pid.parse::<std::num::NonZeroU32>().ok()?;
        id = conversation;
    }
    token(id).then(|| Resume {
        agent,
        id: id.into(),
    })
}

fn ensure(ok: bool, why: &str) -> io::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(io::Error::other(why))
    }
}
impl Workspace {
    fn check(&self) -> io::Result<()> {
        ensure(
            self.version == 1 && !self.sessions.is_empty(),
            "empty or unsupported workspace",
        )?;
        let plain = |s: &str| !s.chars().any(char::is_control);
        let printable = |s: &str| !s.is_empty() && plain(s);
        let directory = |d: &str| d.is_empty() || (plain(d) && Path::new(d).is_absolute());
        let mut names = HashSet::new();
        let mut leaders = HashSet::new();
        for s in &self.sessions {
            ensure(
                printable(&s.name)
                    && !s.name.contains([':', '.'])
                    && s.name != SESSION
                    && !s.name.starts_with(STAGE)
                    && names.insert(&s.name),
                "invalid or duplicate tmux session name",
            )?;
            ensure(directory(&s.path), "session directory is not absolute")?;
            if let Some(g) = &s.group {
                ensure(
                    s.windows.is_empty() && leaders.contains(&g.leader),
                    "a grouped session must follow the session it shares windows with",
                )?;
                continue;
            }
            leaders.insert(&s.name);
            ensure(
                s.windows.iter().filter(|w| w.active).count() == 1,
                "missing or ambiguous active window",
            )?;
            let mut indexes = HashSet::new();
            for w in &s.windows {
                ensure(
                    indexes.insert(w.index)
                        && plain(&w.name)
                        && w.width > 0
                        && w.height > 0
                        && w.size_policy.as_deref().is_none_or(|p| {
                            matches!(p, "manual" | "smallest" | "largest" | "latest")
                        }),
                    "invalid window metadata",
                )?;
                ensure(
                    printable(&w.layout) && w.layout.starts_with('{') && w.layout.ends_with('}'),
                    "expected a tmux 3.8 window layout",
                )?;
                ensure(
                    w.panes.iter().filter(|p| p.active).count() == 1,
                    "missing or ambiguous active pane",
                )?;
                for p in &w.panes {
                    ensure(directory(&p.cwd), "pane working directory is not absolute")?;
                    if let Some(r) = &p.resume {
                        ensure(token(&r.id), "invalid agent resume ID")?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn tmux(args: &[&str]) -> io::Result<String> {
    // -u: as the popup's client, names are read as UTF-8 whatever the locale.
    let out = tmux_command().arg("-u").args(args).output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "tmux {}: {}",
            args[0],
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    String::from_utf8(out.stdout)
        .map(|s| s.strip_suffix('\n').unwrap_or(&s).to_owned())
        .map_err(io::Error::other)
}

/// Linked windows are saved in the first session that has them, and windows
/// with floating panes are left out, each with a warning; a session left
/// with no windows is skipped with the sessions grouped with it.
fn capture() -> io::Result<Workspace> {
    let fields = [
        "session_name",
        "session_group",
        "window_id",
        "window_index",
        "window_name",
        "window_active",
        "window_layout",
        "window_width",
        "window_height",
        "window-size",
        "automatic-rename",
        "window_zoomed_flag",
        "pane_floating_flag",
        "pane_active",
        "pane_current_path",
        "session_path",
        "pane_id",
        "@paddock-resume",
        "@agent-kind",
        "@agent-instance",
        "pane_current_command",
        "@agent-command",
        "@agent-stale",
        "pane_dead",
    ];
    // Control characters in a value must never become field or row separators.
    let format = fields
        .map(|f| format!("#{{s/[\x01-\x1f]/\x7f/:{f}}}"))
        .join("\x1f");
    let rows = tmux(&["list-panes", "-a", "-F", &format])?;
    let global_size = tmux(&["show-options", "-gwv", "window-size"])?;
    let global_rename = if tmux(&["show-options", "-gwv", "automatic-rename"])? == "on" { "1" } else { "0" };
    let mut sessions: Vec<Session> = vec![];
    let mut leaders: HashMap<&str, &str> = HashMap::new();
    let mut saved = HashSet::new();
    let mut floating = HashSet::new();
    let (mut current, mut keep) = (("", u32::MAX), false);
    let mut settled = Vec::new();
    let num = |s: &str| s.parse::<u32>().map_err(io::Error::other);
    for row in rows.lines() {
        let f: Vec<_> = row.split('\x1f').collect();
        let [
            session,
            group,
            wid,
            index,
            name,
            active,
            layout,
            width,
            height,
            size_policy,
            auto_rename,
            zoom,
            float,
            focused,
            cwd,
            session_path,
            pane_id,
            pending,
            kind,
            instance,
            cmd,
            registered,
            stale,
            dead,
        ] = f[..]
        else {
            return Err(io::Error::other("incomplete tmux pane record"));
        };
        if session == SESSION || session.starts_with(STAGE) {
            continue;
        }
        let index = num(index)?;
        if sessions.last().is_none_or(|s| s.name != session) {
            let leader = (!group.is_empty())
                .then(|| *leaders.entry(group).or_insert(session))
                .filter(|l| *l != session);
            sessions.push(Session {
                name: session.into(),
                group: leader.map(|l| Group {
                    leader: l.into(),
                    window: index,
                }),
                path: readable(session_path).into(),
                windows: vec![],
            });
        }
        let s = sessions.last_mut().unwrap();
        if let Some(g) = &mut s.group {
            if active == "1" {
                g.window = index;
            }
            continue;
        }
        if (session, index) != current {
            current = (session, index);
            keep = saved.insert(wid);
            if !keep {
                eprintln!("{session}:{index}: linked window; saved only in its first session");
                continue;
            }
            s.windows.push(Window {
                index,
                name: name.into(),
                active: active == "1",
                layout: layout.into(),
                width: num(width)?,
                height: num(height)?,
                size_policy: (size_policy != global_size).then(|| size_policy.into()),
                automatic_rename: (auto_rename != global_rename).then_some(auto_rename == "1"),
                zoomed: zoom == "1",
                panes: vec![],
            });
        }
        if !keep {
            continue;
        }
        if float == "1" {
            floating.insert((session, index));
        }
        let mut resume = identity(kind, instance, cmd, registered, stale, dead);
        if !pending.is_empty() {
            // Restore's tag holds a resumed agent's ID until its hooks report
            // it (agy only does on a turn), so an early save keeps it, even
            // with a shell in front: the agent is starting, or stopped. The
            // typed line drops the tag when the agent exits.
            if !instance.is_empty() || dead == "1" {
                settled.push(pane_id);
            } else {
                resume = pending.split_once(':').and_then(|(bin, id)| {
                    let agent = Agent::from_bin(bin)?;
                    token(id).then(|| Resume { agent, id: id.into() })
                });
            }
        }
        if resume.is_none() && matches!(cmd, "pi" | "claude" | "codex" | "agy") {
            eprintln!("{session}:{index}: no confirmed {cmd} resume ID; saving as a shell");
        }
        s.windows.last_mut().unwrap().panes.push(Pane {
            // Empty when tmux can't read it (a root-owned or dead process).
            cwd: match readable(cwd) {
                "" => readable(session_path),
                cwd => cwd,
            }
            .into(),
            active: focused == "1",
            resume,
        });
    }
    // The hooks took over, or the agent is gone: the tag must not outlive it.
    for pane in settled {
        let _ = tmux(&["set-option", "-pu", "-t", pane, "@paddock-resume"]);
    }
    let mut empty = HashSet::new();
    for s in sessions.iter_mut().filter(|s| s.group.is_none()) {
        s.windows.retain(|w| {
            let tiled = !floating.contains(&(s.name.as_str(), w.index));
            if !tiled {
                eprintln!("{}:{}: floating panes are not supported; window not saved", s.name, w.index);
            }
            tiled
        });
        match s.windows.iter().any(|w| w.active) {
            _ if s.windows.is_empty() => {
                empty.insert(s.name.clone());
            }
            false => s.windows[0].active = true,
            true => {}
        }
    }
    sessions.retain(|s| {
        let gone = empty.contains(&s.name) || s.group.as_ref().is_some_and(|g| empty.contains(&g.leader));
        if gone {
            eprintln!("{}: no windows left to save; session skipped", s.name);
        }
        !gone
    });
    Ok(Workspace {
        version: 1,
        sessions,
    })
}

/// A captured directory, or empty if it had a control character (now \x7f).
fn readable(dir: &str) -> &str {
    if dir.contains('\x7f') { "" } else { dir }
}

// Names and start directories are tmux *formats*, even in separate argv
// entries. Quote hashes; the empty suffix also keeps a final ';' literal.
fn literal(value: &str) -> String {
    format!("{}#{{l:}}", value.replace('#', "##"))
}

/// Restore S, unless a session by its name is running; at tmux's start,
/// see `taken`. The name it came back as, if created. A grouped session is
/// only with a leader this run CREATED (saved name -> name).
fn restore(s: &Session, created: &HashMap<&str, String>, saved: &HashSet<&str>, at_start: bool) -> io::Result<Option<String>> {
    let exists = |name: &str| tmux(&["has-session", "-t", &format!("={name}:")]).is_ok();
    let trivial = s.windows.len() == 1 && s.windows[0].panes.len() == 1 && s.windows[0].panes[0].resume.is_none();
    if exists(&s.name) && (!at_start || s.group.is_some() || trivial) {
        println!("skipped {} (already exists)", s.name);
        return Ok(None);
    }
    let path = literal(&s.path);
    let start = if Path::new(&s.path).is_dir() { vec!["-c", path.as_str()] } else { vec![] };
    if let Some(g) = &s.group {
        let Some(leader) = created.get(g.leader.as_str()) else {
            // Without that session, -t would quietly start a new group by that
            // name; with one this restore skipped, it would join the user's.
            if tmux(&["has-session", "-t", &format!("={}:", g.leader)]).is_err() {
                return Err(io::Error::other(format!("{} is not running to share windows with", g.leader)));
            }
            println!("skipped {} ({} was already running, not restored)", s.name, g.leader);
            return Ok(None);
        };
        // A session target ends in ':', so a name ending in ';' stays an argument.
        let (name, target) = (literal(&s.name), format!("={leader}:"));
        let mut args = vec!["new-session", "-d", "-s", &name, "-t", &target];
        args.extend(&start);
        tmux(&args)?;
        if let Err(e) = tmux(&["select-window", "-t", &format!("={}:{}", s.name, g.window)]) {
            eprintln!("{}: {e}", s.name);
        }
        println!("restored {} (grouped with {leader})", s.name);
        return Ok(Some(s.name.clone()));
    }
    // Construct the layout with inert placeholders before starting any agents.
    // A partial layout never takes the requested name or blocks a retry.
    let stage = format!("{STAGE}{}", std::process::id());
    let mut sid = String::new();
    let mut launches = Vec::new();
    // The agents start before the rename: a restore cut short leaves a
    // staging session (the next restore removes it), never the saved name
    // on panes that are still placeholders.
    let result = build(s, &stage, &start, &mut sid, &mut launches)
        .and_then(|()| {
            launches
                .into_iter()
                .try_for_each(|(pane, p)| launch(&pane, p))
        })
        .and_then(|()| match tmux(&["rename-session", "-t", &sid, &literal(&s.name)]) {
            Ok(_) => Ok(s.name.clone()),
            // Taken (a moment ago, perhaps): at tmux's start, never lost.
            Err(_) if at_start && exists(&s.name) => taken(&s.name, &sid, created, saved),
            Err(e) => Err(e),
        });
    let name = match result {
        Ok(name) => name,
        Err(e) => {
        if !sid.is_empty() {
            tmux(&["kill-session", "-t", &sid]).map_err(|cleanup| {
                io::Error::other(format!(
                    "{e}; cleaning up newly created session {sid}: {cleanup}"
                ))
            })?;
            }
            return Err(e);
        }
    };
    if name == s.name {
        println!("restored {name}");
    } else {
        println!("restored {} as {name} ({} was in use)", s.name, s.name);
    }
    Ok(Some(name))
}

/// At tmux's start, the saved session NAME (built as SID) when a session of
/// that name runs: the one tmux or the terminal just started, a shell in a
/// single pane, is replaced (its clients switched over); one in use stays,
/// and the saved one becomes NAME-restored (or -2, -3...), a name neither
/// SAVED nor CREATED by this restore. The final name.
fn taken(name: &str, sid: &str, created: &HashMap<&str, String>, saved: &HashSet<&str>) -> io::Result<String> {
    let target = format!("={name}:");
    let ours = created.values().any(|c| c == name);
    let fields = tmux(&["display-message", "-p", "-t", &target, "#{session_windows}\x1f#{window_panes}\x1f#{pane_current_command}\x1f#{session_created}"])?;
    let pristine = match fields.split('\x1f').collect::<Vec<_>>()[..] {
        [windows, panes, cmd, created] => {
            windows == "1" && panes == "1" && is_shell(cmd) && created.parse::<u64>().is_ok_and(|t| now().saturating_sub(t) <= 120)
        }
        _ => false,
    };
    if pristine && !ours {
        let clients = tmux(&["list-clients", "-t", &target, "-F", "#{client_name}"]).unwrap_or_default();
        for client in clients.lines() {
            let _ = tmux(&["switch-client", "-c", client, "-t", sid]);
        }
        tmux(&["kill-session", "-t", &target])?;
        tmux(&["rename-session", "-t", sid, &literal(name)])?;
        return Ok(name.into());
    }
    for n in 1..=100 {
        let other = if n == 1 { format!("{name}-restored") } else { format!("{name}-restored-{n}") };
        if saved.contains(other.as_str()) || created.values().any(|c| *c == other) {
            continue;
        }
        if tmux(&["rename-session", "-t", sid, &literal(&other)]).is_ok() {
            return Ok(other);
        }
    }
    Err(io::Error::other(format!("no free name for {name}")))
}

/// Staging sessions whose restore is no longer running: removed, unless
/// agents were started in them, which are kept as `unfinished-restore-PID`.
fn clean_stages() {
    let Ok(names) = tmux(&["list-sessions", "-F", "#{session_name}"]) else { return };
    for name in names.lines() {
        let Some(pid) = name.strip_prefix(STAGE).and_then(|p| p.parse::<i32>().ok()) else { continue };
        if pid <= 0 || alive(pid) {
            continue;
        }
        let target = format!("={name}:");
        let commands = tmux(&["list-panes", "-s", "-t", &target, "-F", "#{pane_start_command}"]).unwrap_or_default();
        if commands.lines().all(|c| c.contains(PLACEHOLDER)) {
            let _ = tmux(&["kill-session", "-t", &target]);
        } else {
            let _ = tmux(&["rename-session", "-t", &target, &format!("unfinished-restore-{pid}")]);
        }
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

fn launch(id: &str, p: &Pane) -> io::Result<()> {
    let exists = Path::new(&p.cwd).is_dir();
    let cwd = literal(&p.cwd);
    let mut args = vec!["respawn-pane", "-k", "-t", id];
    if exists {
        args.extend(["-c", &cwd]);
    } else if !p.cwd.is_empty() {
        eprintln!("{id}: missing directory {}; opening a shell instead", p.cwd);
    }
    // Explicit shell: a configured default-command could be an application.
    args.extend(["--", "sh", "-c", "exec \"${SHELL:-/bin/sh}\" -l"]);
    tmux(&args)?;
    if exists && let Some(r) = &p.resume {
        // Tagged first: the line below drops the tag once the agent exits,
        // however soon that is.
        let tag = format!("{}:{}", r.agent.bin(), r.id);
        tmux(&["set-option", "-p", "-t", id, "@paddock-resume", &tag])?;
        // Typed into the login shell, so its PATH finds the agent and its job
        // control handles Ctrl-Z. `command` skips an alias that would repeat
        // a flag. The ID is a token, so it needs no quoting in any shell.
        // Shells run the rest of the line when Ctrl-Z stops the agent (with
        // the status 128 + SIGTSTP): the tag stays for it.
        // The shell is the pane's: tmux sets its SHELL to default-shell.
        let shell = tmux(&["display-message", "-p", "-t", id, "#{default-shell}"])?;
        let fish = Path::new(shell.trim()).file_name() == Some("fish".as_ref());
        let line = format!(
            "command {} {}; [ {} -eq {} ] || command tmux set-option -pu -t \"$TMUX_PANE\" @paddock-resume 2>/dev/null",
            r.agent.resume(),
            r.id,
            if fish { "$status" } else { "$?" },
            128 + libc::SIGTSTP
        );
        tmux(&["send-keys", "-t", id, "-l", &line])?;
        tmux(&["send-keys", "-t", id, "Enter"])?;
    }
    Ok(())
}

/// Set a window option, or with None, drop it so the global value applies.
fn window_option(window: &str, option: &str, value: Option<&str>) -> io::Result<String> {
    match value {
        Some(v) => tmux(&["set-option", "-w", "-t", window, option, v]),
        None => tmux(&["set-option", "-wu", "-t", window, option]),
    }
}

fn build<'a>(
    s: &'a Session,
    stage: &str,
    start: &[&str],
    sid: &mut String,
    launches: &mut Vec<(String, &'a Pane)>,
) -> io::Result<()> {
    let mut focus = String::new();
    let width = s.windows.iter().map(|w| w.width).max().unwrap().to_string();
    let height = s
        .windows
        .iter()
        .map(|w| w.height)
        .max()
        .unwrap()
        .to_string();
    for w in &s.windows {
        let (mut last, mut active, mut window) = (String::new(), String::new(), String::new());
        for (i, p) in w.panes.iter().enumerate() {
            let first = sid.is_empty();
            let target = format!("{sid}:{}", w.index);
            let title = literal(&w.name);
            let mut args = if first {
                vec![
                    "new-session",
                    "-d",
                    "-s",
                    stage,
                    "-x",
                    &width,
                    "-y",
                    &height,
                    "-n",
                    &title,
                ]
                .into_iter()
                .chain(start.iter().copied())
                .collect()
            } else if i == 0 {
                vec!["new-window", "-d", "-t", &target, "-n", &title]
            } else {
                vec!["split-window", "-dfh", "-l", "2", "-t", &last]
            };
            args.extend([
                "-P",
                "-F",
                "#{session_id} #{window_id} #{window_index} #{pane_id}",
            ]);
            args.extend(["--", "sleep", PLACEHOLDER]);
            let ids = tmux(&args)?;
            let ids: Vec<_> = ids.split_whitespace().collect();
            let [session_id, window_id, index, pane_id] = ids[..] else {
                return Err(io::Error::other("tmux did not report the new pane"));
            };
            if first {
                *sid = session_id.into();
            }
            if first && index != w.index.to_string() {
                tmux(&[
                    "move-window",
                    "-s",
                    window_id,
                    "-t",
                    &format!("{session_id}:{}", w.index),
                ])?;
            }
            window = window_id.into();
            if i == 0 && w.panes.len() > 1 {
                // Lay panes side by side even for narrow, vertically split windows.
                let staging_width = u64::from(w.width).max(w.panes.len() as u64 * 4);
                tmux(&[
                    "resize-window",
                    "-t",
                    &window,
                    "-x",
                    &staging_width.to_string(),
                    "-y",
                    &w.height.to_string(),
                ])?;
            }
            launches.push((pane_id.into(), p));
            last = pane_id.into();
            if p.active {
                active = last.clone();
            }
        }
        tmux(&["select-layout", "-t", &window, &w.layout])?;
        tmux(&[
            "resize-window",
            "-t",
            &window,
            "-x",
            &w.width.to_string(),
            "-y",
            &w.height.to_string(),
        ])?;
        if w.panes.len() > 1 {
            tmux(&["select-layout", "-t", &window, &w.layout])?;
        }
        // resize-window pinned window-size to manual, and -n turned
        // automatic-rename off: put back what the window had.
        window_option(&window, "window-size", w.size_policy.as_deref())?;
        let rename = w.automatic_rename.map(|on| if on { "on" } else { "off" });
        window_option(&window, "automatic-rename", rename)?;
        tmux(&["select-pane", "-t", &active])?;
        if w.zoomed {
            tmux(&["resize-pane", "-Z", "-t", &active])?;
        }
        if w.active {
            focus = window;
        }
    }
    tmux(&["select-window", "-t", &format!("{sid}:{focus}")])?;
    // new-session -x/-y set the session's default-size: the global applies.
    tmux(&["set-option", "-u", "-t", sid, "default-size"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autosave_interval_and_bookmark() {
        let secs = |v| interval(v).map(|d| d.as_secs());
        assert_eq!(secs("5"), Some(300));
        assert_eq!(secs("1440"), Some(86_400));
        assert_eq!(secs("30s"), Some(30));
        for off in ["", "off", "0", "0s", "1441", "-5", "5m", "s", " 5"] {
            assert_eq!(secs(off), None, "{off:?}");
        }
        assert_eq!(auto_name("/tmp/tmux-501/default"), "auto");
        assert_eq!(auto_name(""), "auto");
        assert_eq!(auto_name("/tmp/tmux-501/work"), "auto-work");
        assert_eq!(auto_name("/tmp/x/bad name"), "auto-bad_name");
        // Never a copy's name: auto-x.previous is auto-x's.
        assert_eq!(auto_name("/tmp/x/x.previous"), "auto-x_previous");
        assert!(token(&auto_name(&"é".repeat(300))));
    }

    #[test]
    fn require_live_scoped_resume_identity() {
        let id = |kind, instance, cmd, registered, stale, dead| {
            identity(kind, instance, cmd, registered, stale, dead).map(|r| r.id)
        };
        assert_eq!(
            id("antigravity", "antigravity:42:chat", "agy", "", "", "0").as_deref(),
            Some("chat")
        );
        for invalid in [
            "antigravity:42",
            "antigravity:no:chat",
            "antigravity:0:chat",
            "antigravity:42:--latest",
            "other:42:chat",
        ] {
            assert!(id("antigravity", invalid, "agy", "", "", "0").is_none());
        }
        for (cmd, registered, stale, dead) in [
            ("bash", "bash", "", "0"),
            ("vim", "pi", "", "0"),
            ("pi", "pi", "1", "0"),
            ("pi", "pi", "", "1"),
        ] {
            assert!(id("pi", "pi:chat", cmd, registered, stale, dead).is_none());
        }
    }
}

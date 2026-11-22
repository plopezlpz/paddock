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

/// Restore builds a session under this prefix, so a partial layout never
/// takes the saved name. Save skips any that an interrupted restore left.
const STAGE: &str = "_paddock_restore_";

pub enum Action {
    Save(String),
    Restore(String),
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
            "restore" => Self::Restore(name.into()),
            "remove" => Self::Remove(name.into()),
            _ => unreachable!(),
        })
    }

    pub fn run(self) -> Result<(), String> {
        let dir = env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
            .ok_or("neither XDG_STATE_HOME nor HOME is set")?
            .join("paddock/workspaces");
        self.apply(&dir).map_err(|e| e.to_string())
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
            Self::Restore(name) => {
                let file = fs::File::open(path(&name))
                    .map_err(|e| io::Error::other(format!("open workspace {name}: {e}")))?;
                let snapshot: Workspace = serde_json::from_reader(file)?;
                snapshot.check()?;
                // One broken session does not keep the others from coming back.
                let mut failed = Vec::new();
                for s in &snapshot.sessions {
                    if let Err(e) = restore(s) {
                        eprintln!("{}: {e}", s.name);
                        failed.push(s.name.as_str());
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
                    ensure(
                        p.cwd.is_empty() || (plain(&p.cwd) && Path::new(&p.cwd).is_absolute()),
                        "pane working directory is not absolute",
                    )?;
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
    let out = tmux_command().args(args).output()?;
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
            // it (agy only does on a turn), so an early save keeps it. A shell
            // in front (the agent still starting, or stopped) keeps the tag.
            if !instance.is_empty() || dead == "1" {
                settled.push(pane_id);
            } else if !is_shell(cmd) {
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
            cwd: if cwd.is_empty() { session_path } else { cwd }.into(),
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

// Names and start directories are tmux *formats*, even in separate argv
// entries. Quote hashes; the empty suffix also keeps a final ';' literal.
fn literal(value: &str) -> String {
    format!("{}#{{l:}}", value.replace('#', "##"))
}

fn restore(s: &Session) -> io::Result<()> {
    let target = format!("={}:", s.name);
    if tmux(&["has-session", "-t", &target]).is_ok() {
        println!("skipped {} (already exists)", s.name);
        return Ok(());
    }
    if let Some(g) = &s.group {
        // Without that session, -t would quietly start a new group by that name.
        tmux(&["has-session", "-t", &format!("={}:", g.leader)])
            .map_err(|_| io::Error::other(format!("{} is not running to share windows with", g.leader)))?;
        // A session target ends in ':', so a name ending in ';' stays an argument.
        tmux(&["new-session", "-d", "-s", &literal(&s.name), "-t", &format!("={}:", g.leader)])?;
        if let Err(e) = tmux(&["select-window", "-t", &format!("{target}{}", g.window)]) {
            eprintln!("{}: {e}", s.name);
        }
        println!("restored {} (grouped with {})", s.name, g.leader);
        return Ok(());
    }
    // Construct the layout with inert placeholders before starting any agents.
    // A partial layout never takes the requested name or blocks a retry.
    let stage = format!("{STAGE}{}", std::process::id());
    let mut sid = String::new();
    let mut launches = Vec::new();
    let result = build(s, &stage, &mut sid, &mut launches)
        .and_then(|()| tmux(&["rename-session", "-t", &sid, &literal(&s.name)]))
        .and_then(|_| {
            launches
                .into_iter()
                .try_for_each(|(pane, p)| launch(&pane, p))
        });
    if let Err(e) = result {
        if !sid.is_empty() {
            tmux(&["kill-session", "-t", &sid]).map_err(|cleanup| {
                io::Error::other(format!(
                    "{e}; cleaning up newly created session {sid}: {cleanup}"
                ))
            })?;
        }
        return Err(e);
    }
    println!("restored {}", s.name);
    Ok(())
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
        // Typed into the login shell, so its PATH finds the agent and its job
        // control handles Ctrl-Z. `command` skips an alias that would repeat
        // a flag. The ID is a token, so it needs no quoting in any shell.
        let line = format!("command {} {}", r.agent.resume(), r.id);
        tmux(&["send-keys", "-t", id, "-l", &line])?;
        tmux(&["send-keys", "-t", id, "Enter"])?;
        let tag = format!("{}:{}", r.agent.bin(), r.id);
        tmux(&["set-option", "-p", "-t", id, "@paddock-resume", &tag])?;
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
            args.extend(["--", "sleep", "86400"]);
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

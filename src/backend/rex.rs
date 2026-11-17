//! paddock on a Rex server. Two `rex do` scripts run for the popup's
//! lifetime: a worker (rex/snapshot.lua and rex/worker.lua) that answers
//! JSON requests over its stdin and stdout, and a listener (rex/events.lua)
//! whose events say when to take a new snapshot. Both are started again
//! when they fail or end.
//!
//! A snapshot is Rex's own data (sessions, tabs, terminal blocks and their
//! program status, OSC 7501), turned into paddock's model here. Agents need
//! no plugin: Claude Code and pi report their status to Rex themselves, with
//! a message paddock shows beside the title. A title the user gives an agent
//! is its pane's Rex label.
//!
//! The rex/paddock.lua action opens paddock in a floating pane, passing the
//! app to switch (`--client`) and its options, and the version it was made
//! for, which main.rs checks as it does tmux's. REX_SESSION and REX_BLOCK,
//! which Rex sets in every terminal, say where paddock runs. Hidden agents
//! and the last tab, which Rex has no place for, are kept in a state file.
//!
//! The other Rex servers the app connects to (`rex hosts`) are shown too,
//! with their own worker and listener (`rex -S HOST`). Each server, paddock's
//! own too, is served by a thread of its own: a capture can take tens of
//! milliseconds, a call over the network as long, so snapshots and previews
//! come in the background and the popup shows the latest it has, never
//! waiting for them. An edit waits for its server, and for a snapshot after
//! it: what the popup shows next has the change.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::mem;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

use super::{Backend, Caps, Msg, Ran, Start};
use crate::keys::{self, Input};
use crate::model::{self, Agent, Op, Pane, Session, Snapshot, State, Tab, Target, Window};
use crate::theme::{self, AgentIcons, Theme};

const SNAPSHOT: &str = include_str!("../../rex/snapshot.lua");
const WORKER: &str = include_str!("../../rex/worker.lua");
const EVENTS: &str = include_str!("../../rex/events.lua");

/// How long a request may take before the worker is given up.
const TIMEOUT: Duration = Duration::from_secs(5);
/// How long a whole snapshot may take: every pane costs a few calls, and over
/// a network each call a round trip (40 ms to a host is 0.1 s a pane).
const WHOLE: Duration = Duration::from_secs(60);
/// How long a new listener may take to say it listens before a snapshot is
/// taken without it: what happens between the two would be in neither.
const LISTENING: Duration = Duration::from_secs(4);
/// How long a host's connection may take (`rex --timeout`), and how long
/// after failing it is tried again: a host can be away for a while.
const CONNECT: &str = "3s";
const RETRY: Duration = Duration::from_secs(30);
/// How long after failing paddock's own server is tried again.
const RETRY_HERE: Duration = Duration::from_secs(1);
/// After a snapshot, the server's news waits this many snapshots' time: busy
/// agents on a big server then cost it about a quarter of its time, and a
/// state change shows within a few tenths of a second even there. Never
/// longer than REST, whatever the snapshot took: a whole one of a far host
/// can take many seconds, and its news is not to wait three times that.
const SNAPSHOT_SPACING: u32 = 3;
const REST: Duration = Duration::from_secs(1);

/// The `--rex` command line.
#[derive(Default, Debug, PartialEq)]
pub struct Opts {
    pub tab: Option<Tab>,
    /// The Rex app to switch, as `rex -C` takes it.
    client: Option<String>,
    suggest: String,
    /// Theme roles (accent, muted, hint, border, idle, finished, working,
    /// asking) and their colours.
    colors: Vec<(String, String)>,
    nerd_icons: bool,
    /// The block focused when paddock opened: its window's focus while
    /// paddock's own floating block has it.
    from: String,
}

impl Opts {
    /// The arguments after `--rex`.
    pub fn parse(args: &[&str]) -> Result<Opts, String> {
        let mut opts = Opts::default();
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let mut value = || it.next().map(|v| v.to_string()).ok_or_else(|| format!("{flag} needs a value"));
            match *flag {
                "--tab" => {
                    let v = value()?;
                    opts.tab = if v == "last" { None } else { Some(Tab::parse(&v).ok_or_else(|| format!("no tab {v}"))?) };
                }
                "--client" => opts.client = Some(value()?).filter(|c| !c.is_empty()),
                // Run in rex/paddock.lua's floating pane (main.rs's `fail`).
                "--popup" => {}
                "--suggest" => opts.suggest = value()?,
                "--from" => opts.from = value()?,
                "--color" => {
                    let v = value()?;
                    let (role, colour) = v.split_once('=').ok_or_else(|| format!("--color takes ROLE=COLOUR, not {v}"))?;
                    if !theme::ROLES.contains(&role) {
                        return Err(format!("no colour role {role}"));
                    }
                    // Said now: a colour left as it was looks like a paddock that ignores its options.
                    if theme::parse_colour(colour).is_none() {
                        return Err(format!("{colour} is no colour for {role}: write it as tmux does (#a2a970, red, brightblue, colour214)"));
                    }
                    opts.colors.push((role.to_string(), colour.to_string()));
                }
                "--icons" => {
                    opts.nerd_icons = match value()?.as_str() {
                        "nerd" => true,
                        "unicode" => false,
                        v => return Err(format!("--icons takes unicode or nerd, not {v}")),
                    }
                }
                _ => return Err(format!("unknown option {flag}\n{}", crate::USAGE)),
            }
        }
        Ok(opts)
    }

    /// The colours given, and no frame: a floating pane has its own edge.
    fn theme(&self) -> Theme {
        let mut theme = Theme::default();
        theme.frame = false;
        for (role, colour) in &self.colors {
            theme.set(role, colour);
        }
        if self.nerd_icons {
            theme.agent_icons = AgentIcons::Nerd;
        }
        theme
    }
}

/// A worker's answer line: `{"ok": VALUE}` or `{"err": "MESSAGE"}`.
fn answer(line: &str) -> Result<Value, String> {
    let mut v: Value = serde_json::from_str(line).map_err(|e| format!("rex: unreadable answer: {e}"))?;
    match (v.get_mut("ok").map(Value::take), v.get("err").and_then(Value::as_str)) {
        (Some(ok), _) => Ok(ok),
        (None, Some(err)) => Err(err.to_string()),
        (None, None) => Err(format!("rex: unreadable answer: {line}")),
    }
}

/// How a host's worker and listener are started: PROGRAM (`rex`) for HOST
/// (None: the server paddock runs on) as the app's CLIENT, for paddock's
/// block ME in session HERE (on a host, both empty); tried again RETRY
/// after failing.
struct Spawn {
    program: PathBuf,
    host: Option<String>,
    client: Option<String>,
    me: String,
    here: String,
    /// See Opts::from.
    from: String,
    retry: Duration,
}

impl Spawn {
    /// What its errors are about.
    fn name(&self) -> &str {
        self.host.as_deref().unwrap_or("rex")
    }
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    answers: Receiver<Result<Value, String>>,
    /// The last line rex writes to stderr (why it failed), once it ends.
    stderr: Receiver<String>,
    name: String,
    /// A request went unanswered: a late answer would answer the next one.
    dead: bool,
    /// Its last snapshot failed: it still answers, but is replaced when the
    /// host is next tried, as a new one may do better.
    stale: bool,
    /// It has taken a snapshot: its next can keep what that one read.
    read: bool,
}

/// `rex` for SPAWN's host.
fn rex(spawn: &Spawn) -> Command {
    let mut cmd = Command::new(&spawn.program);
    if let Some(h) = &spawn.host {
        cmd.args(["-S", h, "--timeout", CONNECT]);
    }
    cmd
}

/// A Lua string literal of S, for Lua 5.1 (Rex's GopherLua): control
/// characters as `\ddd`, which it reads (JSON's `\uXXXX` it does not).
pub fn lua_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if c.is_ascii_control() => out.push_str(&format!("\\{:03}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl Worker {
    fn start(spawn: &Spawn) -> Result<Worker, String> {
        let mut cmd = rex(spawn);
        if let Some(c) = &spawn.client {
            cmd.args(["-C", c]);
        }
        let script = format!("PADDOCK_WORKER = true\nPADDOCK_ME = {}\nPADDOCK_HERE = {}\nPADDOCK_FROM = {}\n{SNAPSHOT}\n{WORKER}", lua_string(&spawn.me), lua_string(&spawn.here), lua_string(&spawn.from));
        let mut child = cmd
            .args(["do", "-e", &script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run rex: {e}"))?;
        let stdin = child.stdin.take().ok_or("rex: no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("rex: no stdout")?);
        let errors = BufReader::new(child.stderr.take().ok_or("rex: no stderr")?);
        let (tx, answers) = mpsc::channel();
        thread::spawn(move || {
            for line in stdout.lines().map_while(Result::ok) {
                if tx.send(answer(&line)).is_err() {
                    break;
                }
            }
        });
        let (tx, stderr) = mpsc::channel();
        thread::spawn(move || tx.send(errors.lines().map_while(Result::ok).filter(|l| !l.trim().is_empty()).last().unwrap_or_default()));
        Ok(Worker { child, stdin, answers, stderr, name: spawn.name().to_string(), dead: false, stale: false, read: false })
    }

    /// REQUEST's answer, if it comes within TIMEOUT.
    fn call(&mut self, request: Value, timeout: Duration) -> Result<Value, String> {
        let line = request.to_string() + "\n";
        let failed = if self.stdin.write_all(line.as_bytes()).and_then(|_| self.stdin.flush()).is_err() {
            "exited"
        } else {
            match self.answers.recv_timeout(timeout) {
                Ok(answer) => return answer,
                Err(RecvTimeoutError::Timeout) => "stopped answering",
                Err(RecvTimeoutError::Disconnected) => "exited",
            }
        };
        // Given up: what it said on its way out, if anything, says why (not
        // waited for long: a process it left may hold its stderr open).
        self.dead = true;
        stop(Some(&mut self.child));
        let why = self.stderr.recv_timeout(Duration::from_millis(100)).unwrap_or_default();
        let why = why.trim().trim_start_matches("Error: ");
        Err(if why.is_empty() { format!("{} {failed}", self.name) } else { format!("{} {failed}: {why}", self.name) })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        stop(Some(&mut self.child));
    }
}

/// Kill CHILD, if any, and reap it.
fn stop(child: Option<&mut Child>) {
    if let Some(c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// What a listener (rex/events.lua) says.
#[derive(Debug, PartialEq)]
enum Heard {
    /// It listens: what happens from now on is in an event.
    Ready,
    /// Only these blocks' state, title or folder changed.
    Blocks(Vec<String>),
    /// Sessions, tabs or layouts changed, and these blocks.
    Changed(Vec<String>),
    /// A block no event names changed: any may have.
    All,
    /// It ended, with the last thing it wrote to stderr (why, if it failed).
    Ended(String),
}

/// A listener's LINE; None for one that is no news ("alive", which it writes
/// to find out that paddock is gone).
fn heard(line: &str) -> Option<Heard> {
    let mut words = line.split(' ').filter(|w| !w.is_empty());
    let kind = words.next()?;
    let ids = words.map(String::from).collect();
    match kind {
        "ready" => Some(Heard::Ready),
        "alive" => None,
        "blocks" => Some(Heard::Blocks(ids)),
        "changed" => Some(Heard::Changed(ids)),
        // "all", and what a later listener may say that this paddock does
        // not know: everything is read.
        _ => Some(Heard::All),
    }
}

/// Listen on SPAWN's host for events that can change the snapshot: HEAR gets
/// what the listener says, a burst of events at a time (rex/events.lua
/// gathers them), and that it ended, for it to be started again.
fn listen(spawn: &Spawn, hear: impl Fn(Heard) + Send + 'static) -> Result<Child, String> {
    let mut child = rex(spawn)
        .args(["do", "-e", EVENTS, "--all"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run rex: {e}"))?;
    let stdout = BufReader::new(child.stdout.take().ok_or("rex: no stdout")?);
    let errors = BufReader::new(child.stderr.take().ok_or("rex: no stderr")?);
    // rex says "listening: N handlers" there once it does: no error.
    let (tx, why) = mpsc::channel();
    thread::spawn(move || tx.send(errors.lines().map_while(Result::ok).filter(|l| !l.trim().is_empty() && !l.starts_with("listening:")).last().unwrap_or_default()));
    thread::spawn(move || {
        stdout.lines().map_while(Result::ok).filter_map(|line| heard(&line)).for_each(&hear);
        // Not waited for long: a process it left may hold its stderr open.
        let why = why.recv_timeout(Duration::from_millis(100)).unwrap_or_default();
        hear(Heard::Ended(why.trim().trim_start_matches("Error: ").to_string()));
    });
    Ok(child)
}

/// What a host's thread does.
enum Job {
    /// A request, answered on the channel; not made if it has not started
    /// by the deadline, as its caller may have given up by then.
    Call(Value, Instant, Sender<Reply>),
    /// Take a snapshot, then post `Changed` (and say so on the channel).
    Refresh(Option<Sender<()>>),
    /// The listener started Nth listens: a snapshot that waited for it is
    /// taken.
    Listening(u64),
    /// The listener started Nth ended: the thread starts another.
    Ended(u64),
    /// Capture a block for the preview.
    Capture(String),
    /// paddock is closing.
    Stop,
}

/// How much of a server a snapshot reads again (rex/snapshot.lua).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum Depth {
    /// The blocks named, in the last snapshot.
    #[default]
    Blocks,
    /// Sessions and tabs, with their panes as last read, but for new ones.
    Layout,
    /// Everything.
    Whole,
}

/// A request's answer, and whether it is lost: no answer came, so the
/// request may have been made or not.
type Reply = (Result<Value, String>, bool);

/// A captured screen, as `Backend::capture` returns it.
type Shot = (Result<Vec<String>, String>, Duration);

/// A host's latest, as its thread leaves it for the popup.
#[derive(Default)]
struct Latest {
    /// The last snapshot's sessions, kept through a failed one; None before
    /// the host first answers, and once it stops.
    sessions: Option<Value>,
    /// Snapshots kept so far: `sessions` changed when this did.
    taken: u64,
    refresh_queued: bool,
    /// What the queued refresh is to read again: that much of the server,
    /// and these blocks.
    refresh_depth: Depth,
    refresh_blocks: BTreeSet<String>,
    /// The block the preview shows: the only one worth capturing.
    wanted: String,
    /// A capture is queued or running.
    capturing: bool,
    /// The last capture to come in, and its block, until `capture` takes it.
    landed: Option<(String, Shot)>,
    /// Why the host is failing, until it answers again.
    error: Option<String>,
    /// Since when it has been taking a snapshot, while it is: its requests
    /// wait for that.
    reading: Option<Instant>,
    /// Why its listener ended, until one listens again: without one, news
    /// comes only with the snapshot each restart takes.
    deaf: Option<String>,
    /// Which listener is the current one (the nth started): only its word
    /// counts.
    ear: u64,
    listener: Option<Child>,
    stopped: bool,
}

/// A Rex server, served by a thread of its own: the one paddock runs on, or
/// another the app connects to.
struct Host {
    /// Its `rex hosts` label; None: the server paddock runs on.
    label: Option<String>,
    jobs: Sender<Job>,
    latest: Arc<Mutex<Latest>>,
}

impl Host {
    /// Its thread, with a whole snapshot to take first, once its listener
    /// listens. WAKE wakes the popup's loop when a snapshot (Changed) or a
    /// capture (Captured) comes in.
    fn start(spawn: Spawn, wake: Arc<dyn Fn(Msg) + Send + Sync>) -> Host {
        let (jobs, rx) = mpsc::channel();
        let latest = Arc::new(Mutex::new(Latest::default()));
        let label = spawn.host.clone();
        queue_refresh(&jobs, &latest, Depth::Whole, Vec::new());
        let (queue, shared) = (jobs.clone(), Arc::clone(&latest));
        thread::spawn(move || serve(&spawn, &queue, &rx, &shared, &*wake));
        Host { label, jobs, latest }
    }

    fn lock(&self) -> MutexGuard<'_, Latest> {
        self.latest.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn name(&self) -> &str {
        self.label.as_deref().unwrap_or("rex")
    }

    /// Why it is failing, if it is, under its name.
    fn trouble(&self) -> Option<String> {
        let latest = self.lock();
        let why = latest.error.clone().or_else(|| latest.deaf.as_ref().map(|e| format!("{} hears no news: {e}", self.name())))?;
        // A worker's own errors name it already.
        Some(if why.starts_with(self.name()) { why } else { format!("{}: {why}", self.name()) })
    }

    /// A request, answered once the host has.
    fn call(&self, request: Value) -> Result<Value, String> {
        self.ask(request).map_or_else(Err, |(answer, _)| answer)
    }

    /// The host's reply to REQUEST, or why there is none: then the request
    /// was never made.
    fn ask(&self, request: Value) -> Result<Reply, String> {
        let (tx, rx) = mpsc::channel();
        if self.jobs.send(Job::Call(request, Instant::now() + TIMEOUT, tx)).is_err() {
            return Err(format!("{} is gone", self.name()));
        }
        // Started in time, it is answered in TIMEOUT more; not started in
        // time, it never is.
        rx.recv_timeout(TIMEOUT * 3).map_err(|_| format!("{} is not answering", self.name()))
    }

    /// Wait for its first snapshot, TIMEOUT at most: the popup opens on it.
    fn settle(&self) {
        let by = Instant::now() + TIMEOUT;
        while self.lock().taken == 0 && Instant::now() < by {
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Why a request of the user's would wait long, if it would: the host is
    /// taking a snapshot that reads many panes, which over a network can
    /// take a while. Said at once, not after the popup has hung on it.
    fn busy(&self) -> Result<(), String> {
        match self.lock().reading {
            Some(since) if since.elapsed() > Duration::from_secs(1) => Err(format!("{} is still reading its sessions: try again in a moment", self.name())),
            _ => Ok(()),
        }
    }

    /// A request that changes something, then a snapshot: the popup's next
    /// look shows the change (and a held key's next edit starts from it).
    fn edit(&self, request: Value) -> Reply {
        if let Err(busy) = self.busy() {
            return (Err(busy), false);
        }
        let answer = match self.ask(request) {
            Ok(answer) => answer,
            Err(why) => return (Err(why), false),
        };
        let (tx, rx) = mpsc::channel();
        if self.jobs.send(Job::Refresh(Some(tx))).is_ok() {
            let _ = rx.recv_timeout(TIMEOUT * 2);
        }
        answer
    }

    /// BLOCK's capture, if one came in since the last call; else one is
    /// asked for, unless one is under way: one at a time, of the block shown
    /// last.
    fn capture(&self, block: &str) -> Option<Shot> {
        let mut latest = self.lock();
        if let Some((b, shot)) = latest.landed.take()
            && b == block
        {
            return Some(shot);
        }
        block.clone_into(&mut latest.wanted);
        if !latest.capturing {
            latest.capturing = true;
            let _ = self.jobs.send(Job::Capture(block.to_string()));
        }
        None
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let mut latest = self.lock();
        latest.stopped = true;
        stop(latest.listener.as_mut());
        let _ = self.jobs.send(Job::Stop);
    }
}

/// One refresh at a time: events that come while one is queued are in it.
/// It reads DEPTH of the server again, and BLOCKS.
fn queue_refresh(jobs: &Sender<Job>, latest: &Mutex<Latest>, depth: Depth, blocks: Vec<String>) {
    let mut latest = latest.lock().unwrap_or_else(|e| e.into_inner());
    latest.refresh_depth = latest.refresh_depth.max(depth);
    latest.refresh_blocks.extend(blocks);
    if !mem::replace(&mut latest.refresh_queued, true) {
        let _ = jobs.send(Job::Refresh(None));
    }
}

/// A host's thread: its worker and listener, (re)started when they are
/// down, do the JOBS (QUEUE sends them).
fn serve(spawn: &Spawn, queue: &Sender<Job>, jobs: &Receiver<Job>, latest: &Arc<Mutex<Latest>>, wake: &(dyn Fn(Msg) + Send + Sync)) {
    let lock = || latest.lock().unwrap_or_else(|e| e.into_inner());
    let mut worker: Option<Worker> = None;
    let mut retry = Instant::now();
    // Workers started, and the one whose snapshot failed, if the last did:
    // when a worker started after it fails too, the host's sessions go.
    let mut starts = 0;
    let mut failed = None;
    // News that came before `rest`: its snapshot is taken then.
    let mut rest = Instant::now();
    let mut deferred = false;
    // A new listener has until then to say it listens: news waits for that
    // too, as what happens between a snapshot and then is in no event.
    let mut unheard: Option<Instant> = None;
    // Listeners started: the last one's word is the one waited for.
    let mut listeners = 0;
    loop {
        let listening = || lock().listener.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)));
        let up = |w: &Option<Worker>| w.as_ref().is_some_and(|w| !w.dead && !w.stale);
        if !(up(&worker) && listening()) && Instant::now() >= retry {
            retry = Instant::now() + spawn.retry;
            if !listening() {
                let (q, l) = (queue.clone(), Arc::clone(latest));
                listeners += 1;
                let nth = listeners;
                lock().ear = nth;
                let listener = listen(spawn, move |heard| {
                    // A listener replaced (it ended, another started) has no
                    // say in whether the host hears.
                    let deaf = |why: Option<String>| {
                        let mut latest = l.lock().unwrap_or_else(|e| e.into_inner());
                        if latest.ear == nth {
                            latest.deaf = why;
                        }
                    };
                    match heard {
                        Heard::Ready => {
                            deaf(None);
                            let _ = q.send(Job::Listening(nth));
                        }
                        Heard::Blocks(ids) => queue_refresh(&q, &l, Depth::Blocks, ids),
                        Heard::Changed(ids) => queue_refresh(&q, &l, Depth::Layout, ids),
                        Heard::All => queue_refresh(&q, &l, Depth::Whole, Vec::new()),
                        // Its end wakes this thread, which starts another
                        // (and waits for this one no longer).
                        Heard::Ended(why) => {
                            if !why.is_empty() {
                                deaf(Some(why));
                            }
                            let _ = q.send(Job::Ended(nth));
                        }
                    }
                });
                unheard = listener.is_ok().then(|| Instant::now() + LISTENING.min(spawn.retry));
                let listener = listener.map_err(|e| lock().deaf = Some(e)).ok();
                let mut old = mem::replace(&mut lock().listener, listener);
                stop(old.as_mut());
            }
            if !up(&worker) {
                worker = None;
                starts += 1;
                match Worker::start(spawn) {
                    Ok(w) => worker = Some(w),
                    Err(e) => lock().error = Some(e),
                }
            }
            // What changed while it was down.
            queue_refresh(queue, latest, Depth::Whole, Vec::new());
        }
        let healthy = up(&worker) && listening();
        // A listener that ended is not waited for.
        let hold = unheard.filter(|_| listening()).map_or(rest, |u| rest.max(u));
        let until = [deferred.then_some(hold), (!healthy).then_some(retry)].into_iter().flatten().min();
        let job = match until {
            None => jobs.recv().ok(),
            Some(until) => match jobs.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) if deferred && Instant::now() >= hold => {
                    deferred = false;
                    Some(Job::Refresh(None))
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => None,
            },
        };
        if lock().stopped {
            stop(lock().listener.as_mut());
            return;
        }
        let working = up(&worker);
        // A worker that has read nothing yet has it all to read.
        let unread = worker.as_ref().is_some_and(|w| !w.read);
        // The answer, and whether it is lost: the worker went without
        // answering, so made or not, who knows. One that answers an error
        // refused; one already down was never asked.
        let mut call = |request: Value, timeout: Duration| -> Reply {
            let Some(w) = worker.as_mut().filter(|w| !w.dead) else {
                return (Err(lock().error.clone().unwrap_or_else(|| format!("{} is not reachable", spawn.name()))), false);
            };
            let answer = w.call(request, timeout);
            // A worker whose snapshot failed still answers: no news of that.
            if w.dead {
                lock().error = answer.clone().err();
            } else if !w.stale {
                lock().error = None;
            }
            (answer, w.dead)
        };
        match job {
            None | Some(Job::Stop) => return,
            // An earlier listener's (it said so, then ended) is not this one's.
            Some(Job::Listening(nth)) if nth == listeners => unheard = None,
            // Its output ended, but it may not have exited yet: seen
            // running, it would be waited on with nothing due, so no
            // other starts until some request came. Reaped, it is down.
            Some(Job::Ended(nth)) if nth == listeners => stop(lock().listener.as_mut()),
            Some(Job::Listening(_) | Job::Ended(_)) => {}
            Some(Job::Call(_, by, reply)) if Instant::now() > by => {
                let _ = reply.send((Err(format!("{} is not answering", spawn.name())), false));
            }
            Some(Job::Call(request, _, reply)) => {
                let _ = reply.send(call(request, TIMEOUT));
            }
            // Down: the restart takes one.
            Some(Job::Refresh(None)) if !working => lock().refresh_queued = false,
            // Too soon after the last, or before the listener listens: still
            // queued, so news joins it.
            Some(Job::Refresh(None)) if Instant::now() < hold => deferred = true,
            Some(Job::Refresh(done)) => {
                // An edit's refresh reads the sessions and tabs again; news,
                // what it asks for.
                let (mut depth, blocks) = match done {
                    Some(_) => (Depth::Layout, BTreeSet::new()),
                    None => {
                        let mut latest = lock();
                        latest.refresh_queued = false;
                        (mem::take(&mut latest.refresh_depth), mem::take(&mut latest.refresh_blocks))
                    }
                };
                if unread {
                    depth = Depth::Whole;
                }
                let mut request = json!({ "op": "snapshot", "skip": spawn.me });
                if depth != Depth::Whole {
                    request["blocks"] = json!(blocks);
                    request["layout"] = json!(depth == Depth::Layout);
                }
                // New panes cost what a whole one's do, and how many there
                // are is not known before; a block named, a second at most.
                let timeout = if depth == Depth::Blocks { (TIMEOUT + Duration::from_secs(blocks.len() as u64)).min(WHOLE) } else { WHOLE };
                let start = Instant::now();
                lock().reading = Some(start);
                let (sessions, _) = call(request, timeout);
                lock().reading = None;
                rest = Instant::now() + (start.elapsed() * SNAPSHOT_SPACING).min(REST);
                if let Some(w) = &mut worker {
                    w.stale = sessions.is_err();
                    w.read |= sessions.is_ok();
                }
                let mut latest = lock();
                match sessions {
                    Ok(sessions) => (latest.sessions, latest.error, failed) = (Some(sessions), None, None),
                    Err(e) => {
                        if failed.is_some_and(|at| at < starts) {
                            latest.sessions = None;
                        }
                        failed = failed.or(Some(starts));
                        latest.error = Some(e);
                    }
                }
                latest.taken += 1;
                drop(latest);
                wake(Msg::Changed);
                if let Some(done) = done {
                    let _ = done.send(());
                }
            }
            Some(Job::Capture(block)) => {
                // Moved on while it waited: the popup asks for the block shown.
                if lock().wanted == block {
                    let start = Instant::now();
                    let lines = call(json!({ "op": "capture", "block": block }), TIMEOUT).0.map(|v| screen(&v));
                    lock().landed = Some((block, (lines, start.elapsed())));
                }
                lock().capturing = false;
                wake(Msg::Captured);
            }
        }
    }
}

/// A capture's last `rows` lines (rex/worker.lua's capture), with gaps as
/// spaces.
fn screen(capture: &Value) -> Vec<String> {
    let text = capture["text"].as_str().unwrap_or_default();
    let text = text.strip_suffix('\n').unwrap_or(text);
    let rows = capture["rows"].as_u64().map_or(60, |r| r as usize);
    let lines: Vec<&str> = text.split('\n').collect();
    lines[lines.len().saturating_sub(rows)..].iter().map(|l| spaces(l.strip_suffix('\r').unwrap_or(l))).collect()
}

/// The items of a JSON list (Lua writes an empty one as {}).
fn items(v: Value) -> Vec<Value> {
    match v {
        Value::Array(items) => items,
        _ => Vec::new(),
    }
}

// What rex/snapshot.lua reports. Lua writes an empty list as {}: `list`
// takes either.

#[derive(Deserialize)]
struct RexSession {
    id: String,
    #[serde(default)]
    name: String,
    /// The host it was found on (paddock adds it); None: paddock's server.
    #[serde(skip)]
    host: Option<String>,
    /// Its server is failing (paddock adds it).
    #[serde(skip)]
    away: bool,
    #[serde(default, deserialize_with = "list")]
    windows: Vec<RexWindow>,
}

#[derive(Deserialize)]
struct RexWindow {
    id: String,
    /// Its label: given, or Rex's ID-made default.
    #[serde(default)]
    name: String,
    #[serde(default)]
    active: bool,
    #[serde(default, deserialize_with = "list")]
    panes: Vec<RexPane>,
}

#[derive(Deserialize)]
struct RexPane {
    id: String,
    #[serde(default)]
    active: bool,
    #[serde(default)]
    label: String,
    /// The terminal's title.
    #[serde(default)]
    title: String,
    /// The foreground program, as started.
    #[serde(default)]
    argv0: String,
    /// Its process ID.
    #[serde(default)]
    pid: Option<u64>,
    #[serde(default)]
    cwd: String,
    status: Option<RexStatus>,
}

/// A program status record (OSC 7501).
#[derive(Deserialize)]
struct RexStatus {
    #[serde(default)]
    state: String,
    #[serde(default)]
    app: String,
    /// `app`, inherited from a parent record when the record has none.
    #[serde(default)]
    effective_app: String,
    /// What a blocked program waits for, such as permission.
    #[serde(default)]
    kind: String,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    updated_at: String,
    /// The foreground process when the report arrived.
    #[serde(default)]
    owner: Option<RexOwner>,
}

#[derive(Deserialize)]
struct RexOwner {
    #[serde(default)]
    pid: Option<u64>,
    #[serde(default)]
    name: String,
}

fn list<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum List<T> {
        Items(Vec<T>),
        Empty(serde::de::IgnoredAny),
    }
    Ok(match List::deserialize(d)? {
        List::Items(items) => items,
        List::Empty(_) => Vec::new(),
    })
}

#[derive(Deserialize)]
struct RexSessions(#[serde(deserialize_with = "list")] Vec<RexSession>);

impl RexSession {
    /// Its ID, its tabs' and its panes'.
    fn ids(&self) -> impl Iterator<Item = &str> {
        let windows = self.windows.iter().flat_map(|w| std::iter::once(w.id.as_str()).chain(w.panes.iter().map(|p| p.id.as_str())));
        std::iter::once(self.id.as_str()).chain(windows)
    }
}

/// Whether S is ID-made: a prefix and eight hex digits, as Rex names a
/// tab ("Window 89abcdef") or a pane ("Block 89abcdef") it was not given a
/// name for.
fn made_from_id(s: &str, prefix: &str) -> bool {
    s.strip_prefix(prefix).is_some_and(|id| id.len() == 8 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A pane label someone gave: not empty, the flavour name a new pane starts
/// with, or Rex's ID-made default.
fn given_label(label: &str) -> Option<&str> {
    let label = label.trim();
    (!label.is_empty() && label != "shell" && !made_from_id(label, "Block ")).then_some(label)
}

/// An OSC 7501 app as paddock's agent kind, which picks its mark.
fn kind(app: &str) -> String {
    let app = app.trim();
    let app = app.strip_suffix("-code").or_else(|| app.strip_suffix("-cli")).unwrap_or(app);
    if app.is_empty() { "unknown".into() } else { app.into() }
}

/// Days since 1970-01-01 of a Gregorian date (Howard Hinnant's
/// days_from_civil).
fn days(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// RFC 3339 time (2026-11-04T12:10:56.388+02:00) as Unix seconds.
fn epoch(s: &str) -> Option<u64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (date, time) = s.split_once('T')?;
    if date.len() != 10 || s.as_bytes().get(13) != Some(&b':') {
        return None;
    }
    let mut t = days(n(0..4)?, n(5..7)?, n(8..10)?) * 86_400 + n(11..13)? * 3_600 + n(14..16)? * 60 + n(17..19)?;
    let zone = time.get(8..)?.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if let Some(sign) = zone.chars().next().filter(|c| *c == '+' || *c == '-') {
        let (h, m) = zone[1..].split_once(':')?;
        let off = h.parse::<i64>().ok()? * 3_600 + m.parse::<i64>().ok()? * 60;
        t += if sign == '+' { -off } else { off };
    }
    u64::try_from(t).ok()
}

/// A status record as paddock's state and the note shown with it. A state
/// paddock does not know is idle, as Rex's docs say.
fn state(s: &RexStatus) -> (State, String) {
    let msg = s.msg.trim();
    let state = match s.state.as_str() {
        "blocked" => State::Asking,
        "working" => State::Working,
        "done" => State::Finished,
        // A failed program is done, the note says how: seen once visited,
        // as a finished one is, and shown as tmux shows a failed turn.
        "error" => return (State::Finished, if msg.is_empty() { "failed".into() } else { format!("failed: {msg}") }),
        _ => State::Idle,
    };
    // A blocked program without a message still says what it waits for.
    let note = if msg.is_empty() && state == State::Asking { s.kind.trim() } else { msg };
    (state, note.to_string())
}

/// A working directory as a path: Rex's reported one (OSC 7) is a file URL.
fn local_path(dir: &str) -> String {
    let Some(rest) = dir.strip_prefix("file://") else { return dir.to_string() };
    let path = rest.find('/').map_or("/", |i| &rest[i..]);
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The program's name as a shell reports it: a login shell's without its dash.
fn command(argv0: &str) -> String {
    argv0.rsplit('/').next().unwrap_or(argv0).trim_start_matches('-').to_string()
}

impl RexPane {
    /// A pane on HOST, whose paths are shown as `HOST:PATH`: they are not
    /// this machine's (nor looked up in its file system).
    fn pane(self, index: usize, host: Option<&str>, saved: &Saved) -> Pane {
        let command = command(&self.argv0);
        let agent = self.status.as_ref().map(|status| {
            let (mut state, note) = state(status);
            let since = epoch(&status.updated_at);
            // Done, and the user has been to it since: its record is no newer
            // than the one seen then, so nothing new there. One without a
            // time can't say, so stays as it is.
            if state == State::Finished && saved.visited.get(&self.id).is_some_and(|(seen, _)| since.is_some_and(|s| s <= *seen)) {
                state = State::Idle;
            }
            let auto = model::title_of(&self.title, "");
            let label = given_label(&self.label);
            Agent {
                state,
                kind: kind(if status.effective_app.is_empty() { &status.app } else { &status.effective_app }),
                title: label.map_or_else(|| auto.clone(), String::from),
                auto,
                renamed: label.is_some(),
                since,
                hidden: saved.hidden.contains_key(&self.id),
                // Its program has gone back to the shell (exited, crashed),
                // or another one runs where it reported from: an editor,
                // after it died without saying. A shell's report (a prompt
                // hook) is for what it runs next.
                stale: model::is_shell(&command) || status.owner.as_ref().is_some_and(|owner| owner.pid.filter(|p| *p != 0).zip(self.pid).is_some_and(|(owner, pid)| owner != pid) && !model::is_shell(&owner.name)),
                note,
                transcript: String::new(),
                asks: String::new(),
            }
        });
        let path = local_path(&self.cwd);
        let path = match host {
            Some(h) if !path.is_empty() => format!("{h}:{path}"),
            _ => path,
        };
        Pane { id: self.id, index: index.to_string(), active: self.active, command, path, agent }
    }
}

/// paddock's snapshot of what rex/snapshot.lua reported, with SAVED's
/// hidden agents hidden and its visits seen.
fn snapshot(sessions: Vec<RexSession>, saved: &Saved) -> Snapshot {
    let sessions = sessions
        .into_iter()
        .map(|s| Session {
            id: s.id,
            name: s.name,
            host: s.host.clone().unwrap_or_default(),
            away: s.away,
            windows: s
                .windows
                .into_iter()
                .enumerate()
                .map(|(i, w)| {
                    let panes: Vec<Pane> = w.panes.into_iter().enumerate().map(|(j, p)| p.pane(j + 1, s.host.as_deref(), saved)).collect();
                    let auto = made_from_id(&w.name, "Window ");
                    // Unnamed, it is named after its focused pane: an agent's
                    // title, else what runs there, as tmux names a window. (Rex
                    // shows the terminal's title, which a shell makes of the
                    // user, host and folder: the row has the folder already.)
                    let shown = |p: &Pane| p.agent.as_ref().map(|a| a.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| p.command.clone());
                    let name = match panes.iter().find(|p| p.active).or(panes.first()).map(shown) {
                        Some(name) if auto && !name.is_empty() => name,
                        _ => w.name,
                    };
                    Window { auto, id: w.id, index: (i + 1).to_string(), name, active: w.active, panes }
                })
                .collect(),
        })
        .collect();
    Snapshot::new(sessions)
}

/// What paddock keeps that Rex has no place for, in a file of tab-separated
/// lines. A block's line ends with its host's label, unless it is on
/// paddock's own server.
#[derive(Default, Debug, PartialEq)]
struct Saved {
    /// Hidden agents' blocks, and their hosts ("": paddock's server).
    hidden: BTreeMap<String, String>,
    /// The status record's time (Rex's updated_at, Unix seconds; 0: none)
    /// each block had when paddock last took the user to it, and its host.
    visited: BTreeMap<String, (u64, String)>,
    hidden_expanded: bool,
    last_tab: Option<String>,
}

impl Saved {
    fn path() -> PathBuf {
        let base = std::env::var_os("XDG_STATE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"));
        base.join("paddock/rex-state")
    }

    fn parse(text: &str) -> Saved {
        let mut state = Saved::default();
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            let host = |i: usize| fields.get(i).copied().unwrap_or_default().to_string();
            match fields[..] {
                ["hidden", block, ..] => {
                    state.hidden.insert(block.into(), host(2));
                }
                ["visited", block, at, ..] => {
                    if let Ok(at) = at.parse() {
                        state.visited.insert(block.into(), (at, host(3)));
                    }
                }
                ["hidden-expanded", v] => state.hidden_expanded = v == "1",
                ["last-tab", v] => state.last_tab = Some(v.into()),
                _ => {}
            }
        }
        state
    }

    /// The file's text, without blocks the last snapshot no longer has (it
    /// has those LIVE says), unless their host was not in it (UNANSWERED):
    /// they may still be there.
    fn text(&self, live: impl Fn(&str) -> bool, unanswered: impl Fn(&str) -> bool) -> String {
        let keep = |block: &str, host: &str| live(block) || unanswered(host);
        let on = |host: &str| if host.is_empty() { String::new() } else { format!("\t{host}") };
        let mut out = String::new();
        for (block, host) in self.hidden.iter().filter(|(b, h)| keep(b, h)) {
            out += &format!("hidden\t{block}{}\n", on(host));
        }
        for (block, (at, host)) in self.visited.iter().filter(|(b, (_, h))| keep(b, h)) {
            out += &format!("visited\t{block}\t{at}{}\n", on(host));
        }
        if self.hidden_expanded {
            out += "hidden-expanded\t1\n";
        }
        if let Some(tab) = &self.last_tab {
            out += &format!("last-tab\t{tab}\n");
        }
        out
    }
}

pub struct Rex {
    /// The server paddock runs on, then each host.
    hosts: Vec<Host>,
    /// The host each session, tab and pane of the last snapshot is on, if
    /// not paddock's server.
    routes: HashMap<String, usize>,
    state: Saved,
    state_path: PathBuf,
    /// The last snapshot's panes, each with its status record's time (0:
    /// none), and whether each host was in it.
    live: HashMap<String, u64>,
    /// The pane each tab and session of the last snapshot shows: a jump
    /// there is a visit to it.
    fronts: HashMap<String, String>,
    answered: Vec<bool>,
    /// The trouble `health` last told of.
    trouble: Option<String>,
    /// The last op's answer was lost (`Ran::unsure`).
    unsure: bool,
}

/// The pane SESSION's each tab shows (its active one, else its first), and
/// the one the session shows (its active tab's), by tab and session id.
fn fronts(session: &RexSession) -> Vec<(String, String)> {
    let front = |w: &RexWindow| w.panes.iter().find(|p| p.active).or(w.panes.first()).map(|p| p.id.clone());
    let mut out: Vec<(String, String)> = session.windows.iter().filter_map(|w| Some((w.id.clone(), front(w)?))).collect();
    if let Some(pane) = session.windows.iter().find(|w| w.active).or(session.windows.first()).and_then(front) {
        out.push((session.id.clone(), pane));
    }
    out
}

/// A request's answer as text: an ID, or nothing.
fn text(answer: Value) -> String {
    answer.as_str().unwrap_or_default().to_string()
}

pub fn connect(opts: Opts, events: impl Fn(u64, Msg) + Send + Sync + 'static) -> Result<(Rex, Start), String> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let here = get("REX_SESSION").ok_or("run paddock --rex in a Rex terminal (REX_SESSION is not set)")?;
    let me = get("REX_BLOCK").unwrap_or_default();
    let wake: Arc<dyn Fn(Msg) + Send + Sync> = Arc::new(move |msg| events(0, msg));
    let program = PathBuf::from("rex");
    let local = Host::start(Spawn { program: program.clone(), host: None, client: opts.client.clone(), me: me.clone(), here: here.clone(), from: opts.from.clone(), retry: RETRY_HERE }, Arc::clone(&wake));
    // The first answer says whether the server is there at all; the first
    // snapshot comes once the listener listens.
    let hosts = local.call(json!({ "op": "hosts" })).map(items)?;
    // The app is a client of each host too, under the same label (an ID is
    // this server's own).
    let label = opts.client.as_ref().and_then(|c| match c.starts_with("client:") {
        true => local.call(json!({ "op": "client_label", "client": c })).map(text).ok(),
        false => Some(c.clone()),
    }).filter(|l| !l.is_empty());
    // The hosts start connecting (a far one takes a while) as the popup
    // waits for its own server's first snapshot.
    let mut all = Vec::with_capacity(hosts.len() + 1);
    for host in hosts.iter().filter_map(Value::as_str) {
        let spawn = Spawn { program: program.clone(), host: Some(host.into()), client: label.clone(), me: String::new(), here: String::new(), from: String::new(), retry: RETRY };
        all.push(Host::start(spawn, Arc::clone(&wake)));
    }
    local.settle();
    let window = if me.is_empty() { String::new() } else { local.call(json!({ "op": "where", "block": me })).map(text).unwrap_or_default() };
    all.insert(0, local);
    let state_path = Saved::path();
    let state = Saved::parse(&std::fs::read_to_string(&state_path).unwrap_or_default());
    let start = Start {
        last_tab: state.last_tab.as_deref().and_then(Tab::parse),
        theme: opts.theme(),
        suggest_command: opts.suggest.clone(),
        hidden_expanded: state.hidden_expanded,
        session: here,
        window,
        // Rex speaks it: Shift+Enter comes as itself.
        keys: crate::keys::Protocol::Kitty,
    };
    let rex = Rex { hosts: all, routes: HashMap::new(), state, state_path, live: HashMap::new(), fronts: HashMap::new(), answered: Vec::new(), trouble: None, unsure: false };
    Ok((rex, start))
}

impl Rex {
    /// Make CHANGE to the state file as it is now (another paddock may have
    /// changed it), and replace the file whole.
    fn update(&mut self, change: impl FnOnce(&mut Saved)) -> Result<(), String> {
        let path = &self.state_path;
        let mut state = Saved::parse(&std::fs::read_to_string(path).unwrap_or_default());
        change(&mut state);
        let fail = |e: std::io::Error| format!("cannot save {}: {e}", path.display());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        let unanswered = |label: &str| self.hosts.iter().zip(&self.answered).any(|(h, answered)| h.label.as_deref().unwrap_or_default() == label && !answered);
        std::fs::write(&tmp, state.text(|block| self.live.contains_key(block), unanswered)).and_then(|_| std::fs::rename(&tmp, path)).map_err(fail)?;
        self.state = state;
        Ok(())
    }

    /// The worker's request for OP, or None for what paddock keeps itself.
    fn request(op: &Op) -> Option<Value> {
        Some(match op {
            Op::SetTitle { pane, title } => json!({ "op": "rename_pane", "block": pane, "name": title.as_deref().unwrap_or("") }),
            Op::SetHidden { .. } | Op::SetHiddenExpanded(_) => return None,
            Op::RenameSession { session, name } => json!({ "op": "rename_session", "session": session, "name": name }),
            Op::RenameWindow { window, name } => json!({ "op": "rename_window", "window": window, "name": name }),
            Op::Kill(Target::Session { session }) => json!({ "op": "kill_session", "session": session }),
            Op::Kill(Target::Window { session, window }) => json!({ "op": "kill_window", "session": session, "window": window }),
            Op::Kill(Target::Pane { pane, .. }) => json!({ "op": "kill_pane", "block": pane }),
            Op::SwapWindows { session, a, b } => json!({ "op": "swap_windows", "session": session, "a": a, "b": b }),
            Op::SwapPanes(a, b) => json!({ "op": "swap_panes", "a": a, "b": b }),
            Op::MoveWindow { from, window, into, to, before } => {
                json!({ "op": "move_window", "from": from, "window": window, "into": into, "to": to, "before": before })
            }
            Op::JoinPane { pane, to, before } => json!({ "op": "join_pane", "pane": pane, "to": to, "before": before }),
            Op::BreakPane { pane, session } => json!({ "op": "break_pane", "pane": pane, "session": session }),
            Op::NewSession { name, dir } => json!({ "op": "new_session", "name": name, "dir": dir }),
            Op::NewWindow { session, after, name, dir } => json!({ "op": "new_window", "session": session, "after": after, "name": name, "dir": dir }),
        })
    }

    /// The host ID is on: paddock's server unless the last snapshot found
    /// it on another.
    fn host(&self, id: &str) -> &Host {
        &self.hosts[self.at(id)]
    }

    /// The index of that host.
    fn at(&self, id: &str) -> usize {
        self.routes.get(id).copied().unwrap_or(0)
    }

    /// Where OP goes: the index of the host holding what it names (a new
    /// session's folder says its host), and OP's request for it, with a
    /// host's folder (HOST:PATH, as its panes show) as the host's own path.
    fn route(&self, op: &Op, mut request: Value) -> (usize, Value) {
        let i = match op {
            Op::SetTitle { pane: id, .. }
            | Op::Kill(Target::Pane { pane: id, .. })
            | Op::SwapPanes(id, _)
            | Op::JoinPane { to: id, .. }
            | Op::RenameWindow { window: id, .. }
            | Op::RenameSession { session: id, .. }
            | Op::Kill(Target::Session { session: id } | Target::Window { session: id, .. })
            | Op::SwapWindows { session: id, .. }
            | Op::MoveWindow { from: id, .. }
            | Op::BreakPane { session: id, .. }
            | Op::NewWindow { session: id, .. } => self.at(id),
            Op::NewSession { dir, .. } => self.hosts.iter().position(|h| h.label.as_ref().is_some_and(|l| dir.starts_with(&format!("{l}:")))).unwrap_or(0),
            Op::SetHidden { .. } | Op::SetHiddenExpanded(_) => 0,
        };
        if let Some(label) = &self.hosts[i].label
            && let Some(dir) = request["dir"].as_str().and_then(|d| d.strip_prefix(&format!("{label}:"))).map(String::from)
        {
            request["dir"] = json!(dir);
        }
        (i, request)
    }

    fn op(&mut self, op: &Op) -> Result<Option<Target>, String> {
        if let Some(request) = Rex::request(op) {
            let (i, request) = self.route(op, request);
            let (answer, lost) = self.hosts[i].edit(request);
            self.unsure = lost;
            let id = text(answer?);
            // What it made is on that host, maybe shown before a snapshot
            // says so.
            if !id.is_empty() {
                self.routes.insert(id.clone(), i);
            }
            return Ok(match op {
                Op::NewSession { .. } if !id.is_empty() => Some(Target::Session { session: id }),
                Op::NewWindow { session, .. } if !id.is_empty() => Some(Target::Window { session: session.clone(), window: id }),
                _ => None,
            });
        }
        let host = match op {
            Op::SetHidden { pane, .. } => self.host(pane).label.clone().unwrap_or_default(),
            _ => String::new(),
        };
        self.update(|state| match op {
            Op::SetHidden { pane, hidden: true } => {
                state.hidden.insert(pane.clone(), host);
            }
            Op::SetHidden { pane, hidden: false } => {
                state.hidden.remove(pane);
            }
            Op::SetHiddenExpanded(expanded) => state.hidden_expanded = *expanded,
            _ => {}
        })
        .map(|_| None)
    }
}

impl Backend for Rex {
    fn caps(&self) -> Caps {
        Caps { cross_session_moves: false, tmux_session_names: false, activate_open: true, rex: true, send: true }
    }

    /// Each server's latest sessions, paddock's first, a host's under its
    /// label.
    fn fetch(&mut self) -> Result<Snapshot, String> {
        let mut sessions = Vec::new();
        self.routes.clear();
        self.live.clear();
        self.fronts.clear();
        self.answered.clear();
        for (i, host) in self.hosts.iter().enumerate() {
            let latest = host.lock();
            // Failing, its sessions are the last it reported.
            let away = latest.error.is_some();
            let reported = latest.sessions.as_ref().map(RexSessions::deserialize);
            drop(latest);
            // Sessions that can't be read are not sessions that have gone:
            // what is kept for them stays.
            self.answered.push(matches!(reported, Some(Ok(_))));
            let Some(Ok(RexSessions(reported))) = reported else { continue };
            for mut session in reported {
                session.away = away;
                let panes = session.windows.iter().flat_map(|w| &w.panes);
                self.live.extend(panes.map(|p| (p.id.clone(), p.status.as_ref().and_then(|s| epoch(&s.updated_at)).unwrap_or(0))));
                self.fronts.extend(fronts(&session));
                if host.label.is_some() {
                    self.routes.extend(session.ids().map(|id| (id.to_string(), i)));
                    session.host.clone_from(&host.label);
                }
                sessions.push(session);
            }
        }
        Ok(snapshot(sessions, &self.state))
    }

    // A server that fails is started again.
    fn is_dead(&self) -> bool {
        false
    }

    /// Every server's trouble, paddock's own first: a worker that fails, or
    /// a listener that ended (its news then comes only with each restart).
    fn health(&mut self) -> Option<Result<String, String>> {
        let now: Vec<String> = self.hosts.iter().filter_map(|h| h.trouble()).collect();
        let now = (!now.is_empty()).then(|| now.join(" · "));
        if now == self.trouble {
            return None;
        }
        let was = mem::replace(&mut self.trouble, now);
        match &self.trouble {
            Some(e) => Some(Err(format!("{e} (trying again)"))),
            None => was.map(|_| Ok(if self.hosts.len() == 1 { "rex is answering again" } else { "every server is answering again" }.into())),
        }
    }

    fn capture(&mut self, pane: &str) -> Option<(Result<Vec<String>, String>, Duration)> {
        self.host(pane).capture(pane)
    }

    fn run(&mut self, op: &Op) -> Ran {
        self.unsure = false;
        let result = self.op(op);
        Ran { moved: Vec::new(), result, unsure: self.unsure }
    }

    /// A key as Rex's key event, which Rex encodes as the pane's program
    /// asked (Shift+Enter included), or a paste.
    fn send(&mut self, pane: &str, input: &Input) -> Result<(), String> {
        let request = match input {
            Input::Key(key) => match keys::rex(key) {
                Some(key) => json!({ "op": "send", "block": pane, "key": key }),
                None => return Ok(()),
            },
            Input::Paste(text) => json!({ "op": "send", "block": pane, "text": text, "paste": true }),
        };
        // Not behind a long read: the loop would wait on each key.
        let host = self.host(pane);
        host.busy()?;
        host.call(request).map(|_| ())
    }

    fn closed(&mut self, tab: Tab) {
        let _ = self.update(|state| state.last_tab = Some(tab.label().to_string()));
    }

    /// Show TARGET; a pane's visit is kept, with the time of the status record
    /// it had then: a done agent there is idle until the record changes.
    fn activate(&mut self, target: &Target) -> Result<(), String> {
        let (session, window, block) = match target {
            Target::Session { session } => (session.as_str(), "", ""),
            Target::Window { session, window } => (session.as_str(), window.as_str(), ""),
            Target::Pane { session, window, pane } => (session.as_str(), window.as_str(), pane.as_str()),
        };
        let host = self.host(session);
        host.busy()?;
        host.call(json!({ "op": "jump", "session": session, "window": window, "block": block }))?;
        // A tab or session shows a pane: going there visits it too.
        let shown = match target {
            Target::Pane { .. } => Some(block),
            Target::Window { window, .. } => self.fronts.get(window).map(String::as_str),
            Target::Session { session } => self.fronts.get(session).map(String::as_str),
        };
        if let Some(block) = shown.filter(|b| !b.is_empty()) {
            let (block, host) = (block.to_string(), host.label.clone().unwrap_or_default());
            let seen = self.live.get(&block).copied().unwrap_or(0);
            let _ = self.update(|state| {
                state.visited.insert(block, (seen, host));
            });
        }
        Ok(())
    }
}

/// LINE with Rex's cursor-forward runs (`ESC [ N C`, how its vt format
/// writes gaps) as the spaces they skip, which a parser of styles keeps.
fn spaces(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find("\x1b[") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if after.as_bytes().get(digits) == Some(&b'C') {
            let n = after[..digits].parse::<usize>().unwrap_or(1).max(1);
            out.extend(std::iter::repeat_n(' ', n));
            rest = &after[digits + 1..];
        } else {
            out.push_str("\x1b[");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_keeps_its_last_screenful() {
        let capture = json!({ "rows": 2, "text": "partial\r\none\r\ntwo\x1b[3Cx\n" });
        assert_eq!(screen(&capture), ["one", "two   x"]);
        assert_eq!(screen(&json!({ "rows": 5, "text": "" })), [""]);
    }

    #[test]
    fn parses_the_action_command_line() {
        let opts = Opts::parse(&["--tab", "agents", "--client", "pmac", "--color", "accent=#a2a970", "--icons", "nerd", "--suggest", "title-of"]).unwrap();
        assert_eq!(opts.tab, Some(Tab::Agents));
        assert_eq!(opts.client.as_deref(), Some("pmac"));
        assert_eq!(opts.colors, [("accent".to_string(), "#a2a970".to_string())]);
        assert!(opts.nerd_icons);
        assert_eq!(opts.suggest, "title-of");
        assert_eq!(Opts::parse(&[]).unwrap(), Opts::default());
        assert_eq!(Opts::parse(&["--tab", "last"]).unwrap().tab, None);
        assert!(Opts::parse(&["--color", "sky=blue"]).is_err());
        assert!(Opts::parse(&["--color", "blue"]).is_err());
        let e = Opts::parse(&["--color", "accent=orange"]).unwrap_err();
        assert!(e.starts_with("orange is no colour for accent"), "not left as it was, unsaid: {e}");
        assert!(Opts::parse(&["--tab"]).is_err());
        assert!(Opts::parse(&["--nope"]).is_err());
    }

    #[test]
    fn colours_go_to_their_roles_and_the_frame_goes() {
        let theme = Opts::parse(&["--color", "working=#ffaa00", "--color", "waiting=red"]).unwrap().theme();
        assert_eq!(theme.state(State::Working).fg, Some(ratatui::style::Color::Rgb(0xff, 0xaa, 0x00)));
        assert_eq!(theme.state(State::Asking).fg, Some(ratatui::style::Color::Red));
        assert!(!theme.frame);
    }

    #[test]
    fn lua_strings_escape_what_lua_5_1_reads() {
        assert_eq!(lua_string(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(lua_string("x\ny\u{1b}1é"), "\"x\\ny\\0271é\"", "\\ddd takes three digits, so a digit after it stays");
    }

    #[test]
    fn answers_are_values_or_errors() {
        assert_eq!(answer(r#"{"ok":"block:1"}"#), Ok(json!("block:1")));
        assert_eq!(answer(r#"{"ok":[{"id":"s"}]}"#), Ok(json!([{ "id": "s" }])));
        assert_eq!(answer(r#"{"err":"the tab is gone"}"#), Err("the tab is gone".into()));
        assert!(answer("{}").is_err());
        assert!(answer("not json").is_err());
    }

    /// One session with an idle shell, a blocked Claude Code renamed by the
    /// user, and a pi that is done; Lua's {} for an empty list.
    const REPORT: &str = r#"[{"id":"session:1","name":"work","windows":[
        {"id":"window:1","name":"Window 89abcdef","active":true,"panes":[
            {"id":"block:1","active":false,"label":"shell","title":"~","argv0":"-zsh","cwd":"/src/a"},
            {"id":"block:2","active":true,"label":"fix login","title":"✳ Login bug","argv0":"claude","cwd":"/src/a",
             "status":{"state":"blocked","app":"claude-code","effective_app":"claude-code","kind":"permission","msg":"","updated_at":"2026-11-04T12:00:00+02:00"}}]},
        {"id":"window:2","name":"api","active":false,"panes":[
            {"id":"block:3","label":"Block 0123abcd","title":"π - api","argv0":"/usr/bin/pi","cwd":"/src/api",
             "status":{"state":"done","app":"pi","msg":"3 files changed","updated_at":"2026-11-04T10:00:00Z"}}]},
        {"id":"window:3","name":"empty","panes":{}}]},
        {"id":"session:2","name":"new","windows":{}}]"#;

    #[test]
    fn rex_reports_become_paddocks_model() {
        let sessions = serde_json::from_str::<RexSessions>(REPORT).unwrap().0;
        let saved = Saved { hidden: BTreeMap::from([("block:3".to_string(), String::new())]), ..Saved::default() };
        let snap = snapshot(sessions, &saved);
        assert_eq!(snap.sessions.len(), 2);
        assert!(snap.sessions[1].windows.is_empty());
        let w = &snap.sessions[0].windows;
        assert_eq!(w.iter().map(|w| (w.index.as_str(), w.auto, w.active)).collect::<Vec<_>>(), [("1", true, true), ("2", false, false), ("3", false, false)]);
        assert_eq!(w.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["fix login", "api", "empty"], "unnamed: its focused agent's title");

        let shell = &w[0].panes[0];
        assert_eq!((shell.command.as_str(), shell.path.as_str(), shell.agent.is_none()), ("zsh", "/src/a", true));

        let claude = w[0].panes[1].agent.as_ref().unwrap();
        assert_eq!((claude.state, claude.kind.as_str()), (State::Asking, "claude"));
        assert_eq!((claude.title.as_str(), claude.auto.as_str(), claude.renamed), ("fix login", "Login bug", true));
        assert_eq!(claude.note, "permission", "no message: what it waits for");
        assert_eq!(claude.since, Some(1_793_786_400));
        assert!(!claude.stale && !claude.hidden);

        let pi = w[1].panes[0].agent.as_ref().unwrap();
        assert_eq!((pi.state, pi.kind.as_str(), pi.note.as_str()), (State::Finished, "pi", "3 files changed"));
        assert_eq!((pi.title.as_str(), pi.renamed), ("π - api", false), "Rex's ID-made label is no title");
        assert!(pi.hidden);
        assert_eq!(w[1].panes[0].command, "pi");

        assert_eq!(snap.counts[State::Asking as usize], 1);
        assert_eq!(snap.counts[State::Finished as usize], 1);
    }

    #[test]
    fn a_hosts_sessions_say_where_they_are() {
        let report = r#"[{"id":"session:9","name":"dac","windows":[{"id":"window:9","name":"w","panes":[
            {"id":"block:9","argv0":"-bash","cwd":"file://devbox/home/me/dac"},
            {"id":"block:8","argv0":"-bash"}]}]}]"#;
        let mut sessions = serde_json::from_str::<RexSessions>(report).unwrap().0;
        sessions[0].host = Some("devbox".into());
        let snap = snapshot(sessions, &Saved::default());
        let session = &snap.sessions[0];
        assert_eq!(session.host, "devbox");
        assert_eq!(session.windows[0].panes[0].path, "devbox:/home/me/dac", "not a path on this machine");
        assert_eq!(session.windows[0].panes[1].path, "", "no folder reported");
        let here = snapshot(serde_json::from_str::<RexSessions>(REPORT).unwrap().0, &Saved::default());
        assert_eq!(here.sessions[0].host, "");
        assert_eq!(here.sessions[0].windows[0].panes[0].path, "/src/a");
    }

    #[test]
    fn an_unnamed_tab_without_an_agent_is_named_after_what_runs() {
        let report = r#"[{"id":"s","windows":[
            {"id":"w1","name":"Window 0123abcd","panes":[{"id":"b1","argv0":"-zsh"},{"id":"b2","active":true,"argv0":"/usr/bin/vim"}]},
            {"id":"w2","name":"Window 4567abcd","panes":{}}]}]"#;
        let snap = snapshot(serde_json::from_str::<RexSessions>(report).unwrap().0, &Saved::default());
        let names: Vec<_> = snap.sessions[0].windows.iter().map(|w| (w.name.as_str(), w.auto)).collect();
        assert_eq!(names, [("vim", true), ("Window 4567abcd", true)], "the focused pane's program; nothing runs: Rex's label");
    }

    #[test]
    fn a_reported_session_names_its_tabs_and_panes() {
        let sessions = serde_json::from_str::<RexSessions>(REPORT).unwrap().0;
        assert_eq!(sessions[0].ids().collect::<Vec<_>>(), ["session:1", "window:1", "block:1", "block:2", "window:2", "block:3", "window:3"]);
        assert_eq!(sessions[1].ids().collect::<Vec<_>>(), ["session:2"]);
    }

    #[test]
    fn statuses_map_to_states_and_notes() {
        let s = |state: &str, kind: &str, msg: &str| RexStatus { state: state.into(), app: String::new(), effective_app: String::new(), kind: kind.into(), msg: msg.into(), updated_at: String::new(), owner: None };
        assert_eq!(state(&s("working", "", "Reading files")), (State::Working, "Reading files".into()));
        assert_eq!(state(&s("blocked", "question", "Which branch?")), (State::Asking, "Which branch?".into()));
        assert_eq!(state(&s("blocked", "permission", "")), (State::Asking, "permission".into()));
        assert_eq!(state(&s("done", "", "3 files changed")), (State::Finished, "3 files changed".into()));
        assert_eq!(state(&s("error", "", "tests failed")), (State::Finished, "failed: tests failed".into()), "a failure is done, and says so");
        assert_eq!(state(&s("error", "", "")), (State::Finished, "failed".into()));
        assert_eq!(state(&s("idle", "", "")), (State::Idle, String::new()));
        assert_eq!(state(&s("sleeping", "", "zzz")), (State::Idle, "zzz".into()), "one Rex adds later: idle");
        assert_eq!(state(&s("", "", "")), (State::Idle, String::new()));
    }

    #[test]
    fn a_shell_in_front_makes_an_agent_stale() {
        let pane = RexPane {
            id: "block:9".into(),
            active: true,
            label: String::new(),
            title: String::new(),
            argv0: "-zsh".into(),
            pid: None,
            cwd: String::new(),
            status: Some(RexStatus { state: "done".into(), app: "claude-code".into(), effective_app: String::new(), kind: String::new(), msg: String::new(), updated_at: String::new(), owner: None }),
        };
        let agent = pane.pane(1, None, &Saved::default()).agent.unwrap();
        assert!(agent.stale);
        assert_eq!((agent.kind.as_str(), agent.since, agent.title.as_str()), ("claude", None, ""));
    }

    #[test]
    fn another_program_in_front_makes_an_agent_stale() {
        let stale = |argv0: &str, pid: Option<u64>, owner: Option<u64>| {
            let name = if owner == Some(1) { "zsh" } else { "claude" };
            let status = RexStatus { state: "done".into(), app: "claude-code".into(), effective_app: String::new(), kind: String::new(), msg: String::new(), updated_at: String::new(), owner: owner.map(|pid| RexOwner { pid: Some(pid), name: name.into() }) };
            let pane = RexPane { id: "block:9".into(), active: true, label: String::new(), title: String::new(), argv0: argv0.into(), pid, cwd: String::new(), status: Some(status) };
            pane.pane(1, None, &Saved::default()).agent.unwrap().stale
        };
        assert!(!stale("claude", Some(7), Some(7)), "the program that reported");
        assert!(stale("nvim", Some(8), Some(7)), "an editor, after the agent died");
        assert!(!stale("nvim", Some(8), None), "a record without its owner: as before");
        assert!(!stale("claude", None, Some(7)), "a worker without the pid: as before");
        assert!(!stale("cargo", Some(8), Some(1)), "a shell's report, for what it runs next");
        assert!(!stale("nvim", Some(8), Some(0)), "an owner Rex couldn't tell");
        let odd: Result<RexStatus, _> = serde_json::from_value(json!({ "state": "done", "updated_at": "", "owner": {} }));
        assert!(odd.is_ok(), "an owner without a pid is no reason to drop the server's sessions");
    }

    #[test]
    fn rfc3339_times_are_unix_seconds() {
        assert_eq!(epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch("2026-11-04T12:10:56.388008+02:00"), Some(1_793_787_056));
        assert_eq!(epoch("2026-11-04T10:10:56-00:30"), Some(1_793_787_056 + 1_800));
        assert_eq!(epoch("2000-02-29T00:00:00Z"), Some(951_782_400));
        assert_eq!(epoch(""), None);
        assert_eq!(epoch("yesterday"), None);
    }

    #[test]
    fn labels_and_names_made_from_ids_are_automatic() {
        assert!(made_from_id("Window 89abcdef", "Window "));
        assert!(!made_from_id("Window 89abcdeg", "Window "));
        assert!(!made_from_id("Window 89abcdef0", "Window "));
        assert_eq!(given_label("Block 0123abcd"), None);
        assert_eq!(given_label("shell"), None);
        assert_eq!(given_label("  "), None);
        assert_eq!(given_label(" Tests "), Some("Tests"));
    }

    #[test]
    fn reported_directories_are_paths() {
        assert_eq!(local_path("/src/a"), "/src/a");
        assert_eq!(local_path("file://mac.local/Users/me/my%20project"), "/Users/me/my project");
        assert_eq!(local_path("file:///tmp"), "/tmp");
        assert_eq!(local_path("file://host"), "/");
        assert_eq!(local_path("file:///a%zzb"), "/a%zzb");
    }

    #[test]
    fn gaps_become_spaces() {
        assert_eq!(spaces("a\x1b[3Cb\x1b[Cc"), "a   b c");
        assert_eq!(spaces("\x1b[1;31mred\x1b[0m\x1b[2Cx"), "\x1b[1;31mred\x1b[0m  x");
        assert_eq!(spaces("plain"), "plain");
    }

    #[test]
    fn state_round_trips_and_forgets_closed_blocks() {
        let mut state = Saved::default();
        for (block, host) in [("block:2", ""), ("block:gone", ""), ("block:7", "devbox"), ("block:8", "away")] {
            state.hidden.insert(block.into(), host.into());
        }
        state.visited.insert("block:2".into(), (1_793_786_400, String::new()));
        state.visited.insert("block:9".into(), (5, "away".into()));
        state.visited.insert("block:gone".into(), (5, "devbox".into()));
        state.hidden_expanded = true;
        state.last_tab = Some("agents".into());
        // "away" has not answered yet: its blocks may still be there.
        let text = state.text(|block| ["block:2", "block:7"].contains(&block), |host| host == "away");
        let back = Saved::parse(&text);
        let hidden: Vec<_> = back.hidden.iter().map(|(b, h)| (b.as_str(), h.as_str())).collect();
        assert_eq!(hidden, [("block:2", ""), ("block:7", "devbox"), ("block:8", "away")]);
        assert_eq!(back.visited.keys().collect::<Vec<_>>(), ["block:2", "block:9"]);
        assert_eq!(back.visited["block:9"], (5, "away".to_string()));
        assert!(back.hidden_expanded);
        assert_eq!(back.last_tab.as_deref(), Some("agents"));
        assert_eq!(Saved::parse("hidden\tblock:1\n").hidden["block:1"], "", "a line without a host: paddock's server");
        assert_eq!(Saved::parse("title\tblock:1\told\nvisited\tblock:1\tsoon\nnonsense\n"), Saved::default(), "old title lines are ignored");
    }

    #[test]
    fn a_tab_or_session_jump_visits_the_pane_it_shows() {
        let session: RexSession = serde_json::from_value(json!({
            "id": "session:1",
            "windows": [
                { "id": "window:1", "panes": [{ "id": "block:1" }, { "id": "block:2", "active": true }] },
                { "id": "window:2", "active": true, "panes": [{ "id": "block:3" }] },
                { "id": "window:3", "panes": [] },
            ],
        }))
        .unwrap();
        let got: HashMap<String, String> = fronts(&session).into_iter().collect();
        let want = [("window:1", "block:2"), ("window:2", "block:3"), ("session:1", "block:3")];
        assert_eq!(got, want.into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(), "a tab without panes shows none");
    }

    #[test]
    fn a_done_agent_visited_since_is_idle() {
        let pane = |state: &str| RexPane {
            id: "block:1".into(),
            active: true,
            label: String::new(),
            title: String::new(),
            argv0: "claude".into(),
            pid: None,
            cwd: String::new(),
            status: Some(RexStatus { state: state.into(), app: "claude-code".into(), effective_app: String::new(), kind: String::new(), msg: "all done".into(), updated_at: "2026-11-04T10:00:00Z".into(), owner: None }),
        };
        let done_at = epoch("2026-11-04T10:00:00Z").unwrap();
        let visited = |seen: u64| Saved { visited: BTreeMap::from([("block:1".to_string(), (seen, String::new()))]), ..Saved::default() };
        let state = |state: &str, saved: &Saved| pane(state).pane(1, None, saved).agent.unwrap().state;
        assert_eq!(state("done", &Saved::default()), State::Finished);
        assert_eq!(state("done", &visited(done_at)), State::Idle, "visited with this record");
        assert_eq!(state("done", &visited(done_at - 60)), State::Finished, "done again since the visit");
        assert_eq!(state("done", &visited(0)), State::Finished, "visited before it had a status");
        assert_eq!(state("working", &visited(done_at)), State::Working);
        assert_eq!(state("error", &Saved::default()), State::Finished, "a failure, unseen");
        assert_eq!(state("error", &visited(done_at)), State::Idle, "seen once visited, as done");
    }

    /// A scripted `rex` in a folder of its own, logging what it does to
    /// `log` there, and its snapshot requests to `requests`. Its worker
    /// answers snapshots (failing while `fail` is there; `[{"id":"edited"}]`
    /// after an edit), captures (a block's screen is its name; `slow` takes
    /// 0.3 s) and edits. Its listener says it listens (unless `deaf` is
    /// there), then stays quiet; if `listener-ends` is there, it takes the
    /// file and ends once `end` is.
    struct FakeRex(PathBuf);

    const FAKE: &str = r#"#!/bin/sh
for last; do :; done
if [ "$last" = --all ]; then
  echo listen >> "$D/log"
  [ -e "$D/deaf" ] || echo ready
  if [ -e "$D/listener-ends" ]; then
    rm "$D/listener-ends"
    until [ -e "$D/end" ]; do sleep 0.01; done
    exit 0
  fi
  exec sleep 60
fi
echo start >> "$D/log"
while read -r line; do
  case "$line" in
  *'"op":"snapshot"'*)
    echo snapshot >> "$D/log"
    echo "$line" >> "$D/requests"
    if [ -e "$D/fail" ]; then echo '{"err":"boom"}'
    elif [ -e "$D/edited" ]; then echo '{"ok":[{"id":"edited"}]}'
    else echo '{"ok":[{"id":"s"}]}'; fi ;;
  *'"op":"capture"'*)
    b=${line#*'"block":"'}; b=${b%%'"'*}
    echo "capture $b" >> "$D/log"
    [ "$b" = slow ] && sleep 0.3
    echo "{\"ok\":{\"rows\":1,\"text\":\"$b\"}}" ;;
  *'"op":"vanish"'*) exit 1 ;;
  *'"op":"refuse"'*) echo '{"err":"no such tab"}' ;;
  *) echo edit >> "$D/log"; touch "$D/edited"; echo '{"ok":"done"}' ;;
  esac
done
"#;

    impl FakeRex {
        fn new(files: &[&str]) -> FakeRex {
            use std::os::unix::fs::PermissionsExt;
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!("paddock-rex-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&dir).unwrap();
            let program = dir.join("rex");
            std::fs::write(&program, FAKE.replacen("\n", &format!("\nD='{}'\n", dir.display()), 1)).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            files.iter().for_each(|f| std::fs::write(dir.join(f), "").unwrap());
            FakeRex(dir)
        }

        /// paddock's own server, tried again RETRY after failing.
        fn host(&self, retry: Duration) -> Host {
            let spawn = Spawn { program: self.0.join("rex"), host: None, client: None, me: String::new(), here: String::new(), from: String::new(), retry };
            Host::start(spawn, Arc::new(|_| {}))
        }

        fn log(&self) -> String {
            std::fs::read_to_string(self.0.join("log")).unwrap_or_default()
        }

        /// The snapshot requests so far, a line each.
        fn requests(&self) -> Vec<Value> {
            let text = std::fs::read_to_string(self.0.join("requests")).unwrap_or_default();
            text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
        }
    }

    impl Drop for FakeRex {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Wait (5 s at most) for WHAT.
    fn until(what: &str, done: impl Fn() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < Duration::from_secs(5), "timed out: {what}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_failed_snapshot_keeps_the_rows_until_a_new_worker_fails_too() {
        let fake = FakeRex::new(&[]);
        // Long enough for the first failure to come before a retry is due,
        // however busy the machine.
        let host = fake.host(Duration::from_millis(1500));
        until("the first snapshot", || host.lock().taken == 1);
        std::fs::write(fake.0.join("fail"), "").unwrap();
        queue_refresh(&host.jobs, &host.latest, Depth::Whole, Vec::new());
        until("a failed snapshot", || host.lock().taken >= 2);
        {
            let latest = host.lock();
            assert_eq!(latest.sessions, Some(json!([{ "id": "s" }])), "one failure keeps the rows");
            assert_eq!(latest.error.as_deref(), Some("boom"), "and says why");
        }
        // What else that worker answers is no recovery: its rows are stale.
        until("a capture", || host.capture("B").is_some());
        assert_eq!(host.lock().error.as_deref(), Some("boom"));
        until("a new worker that fails too drops them", || host.lock().sessions.is_none());
        assert_eq!(fake.log().matches("start").count(), 2);
        std::fs::remove_file(fake.0.join("fail")).unwrap();
        until("a worker that answers again", || host.lock().sessions.is_some());
        assert_eq!(host.lock().error, None);
    }

    #[test]
    fn a_worker_whose_first_snapshot_fails_still_answers() {
        // As connect's first request (`hosts`) comes right after it.
        let fake = FakeRex::new(&["fail"]);
        let host = fake.host(Duration::from_secs(1));
        assert_eq!(host.call(json!({ "op": "hosts" })), Ok(json!("done")));
        assert_eq!(fake.log().matches("start").count(), 1);
    }

    #[test]
    fn a_listener_that_ends_is_started_again_and_a_snapshot_catches_up() {
        let fake = FakeRex::new(&["listener-ends"]);
        let host = fake.host(Duration::from_millis(50));
        until("the first snapshot", || host.lock().taken == 1);
        std::fs::write(fake.0.join("end"), "").unwrap();
        // The listener's last event and the restart's: one snapshot, if
        // they come within one snapshot's spacing, after the restart.
        // (The new listener logs itself once running, maybe after that.)
        until("a second listener and a catch-up snapshot", || {
            let log = fake.log();
            log.matches("listen").count() == 2 && log.matches("snapshot").count() >= 2
        });
    }

    #[test]
    fn captures_go_to_the_block_shown_last() {
        let fake = FakeRex::new(&[]);
        let host = fake.host(Duration::from_secs(1));
        until("the first snapshot", || host.lock().taken == 1);
        assert!(host.capture("slow").is_none());
        thread::sleep(Duration::from_millis(50));
        assert!(host.capture("B").is_none(), "one at a time");
        assert!(host.capture("C").is_none());
        let shot = Mutex::new(None);
        until("C's capture", || {
            *shot.lock().unwrap() = host.capture("C");
            shot.lock().unwrap().is_some()
        });
        assert_eq!(shot.into_inner().unwrap().unwrap().0, Ok(vec!["C".to_string()]));
        assert!(!fake.log().contains("capture B"), "moved on before its turn: {}", fake.log());
    }

    #[test]
    fn an_edit_is_answered_after_a_snapshot_that_has_it() {
        let fake = FakeRex::new(&[]);
        let host = fake.host(Duration::from_secs(1));
        until("the first snapshot", || host.lock().taken == 1);
        assert_eq!(host.edit(json!({ "op": "rename_session", "session": "s", "name": "x" })), (Ok(json!("done")), false));
        assert_eq!(host.lock().sessions, Some(json!([{ "id": "edited" }])));
        assert!(fake.log().ends_with("edit\nsnapshot\n"), "{}", fake.log());
    }

    #[test]
    fn a_listener_says_what_changed_and_that_it_lives() {
        assert_eq!(heard("ready"), Some(Heard::Ready));
        assert_eq!(heard("changed"), Some(Heard::Changed(Vec::new())));
        assert_eq!(heard("changed block:3"), Some(Heard::Changed(vec!["block:3".into()])), "a new tab, and a pane's news with it");
        assert_eq!(heard("blocks block:1  block:2"), Some(Heard::Blocks(vec!["block:1".into(), "block:2".into()])));
        assert_eq!(heard("all"), Some(Heard::All));
        assert_eq!(heard("something new"), Some(Heard::All), "not understood: everything is read");
        assert_eq!(heard("alive"), None, "written to find out that paddock is gone");
        assert_eq!(heard(""), None);
    }

    #[test]
    fn news_waiting_its_turn_is_one_snapshot() {
        let (jobs, queued) = mpsc::channel();
        let latest = Mutex::new(Latest::default());
        queue_refresh(&jobs, &latest, Depth::Blocks, vec!["block:2".into()]);
        queue_refresh(&jobs, &latest, Depth::Layout, Vec::new());
        queue_refresh(&jobs, &latest, Depth::Blocks, vec!["block:1".into(), "block:2".into()]);
        assert_eq!(queued.try_iter().count(), 1);
        let latest = latest.into_inner().unwrap();
        assert_eq!((latest.refresh_depth, latest.refresh_blocks), (Depth::Layout, BTreeSet::from(["block:1".to_string(), "block:2".to_string()])), "the most any of it asks for");
    }

    #[test]
    fn news_reads_only_what_it_names_and_an_edit_the_layout() {
        let fake = FakeRex::new(&[]);
        let host = fake.host(Duration::from_secs(1));
        until("the first snapshot", || host.lock().taken == 1);
        assert_eq!(fake.requests(), [json!({ "op": "snapshot", "skip": "" })], "a new worker reads everything");
        queue_refresh(&host.jobs, &host.latest, Depth::Blocks, vec!["block:2".into()]);
        queue_refresh(&host.jobs, &host.latest, Depth::Blocks, vec!["block:1".into()]);
        // In one snapshot when the second comes while the first waits its
        // turn (`news_waiting_its_turn_is_one_snapshot`), else in two.
        let since = |n: usize| fake.requests().split_off(n);
        let named = |n: usize, block: &str| since(n).iter().any(|r| r["blocks"].as_array().is_some_and(|b| b.contains(&json!(block))));
        until("the news' snapshots", || named(1, "block:1") && named(1, "block:2"));
        assert!(since(1).iter().all(|r| r["layout"] == json!(false)), "only what it names: {:?}", since(1));
        let n = fake.requests().len();
        queue_refresh(&host.jobs, &host.latest, Depth::Layout, Vec::new());
        queue_refresh(&host.jobs, &host.latest, Depth::Blocks, vec!["block:3".into()]);
        until("a tab's snapshot", || named(n, "block:3"));
        assert!(since(n).iter().any(|r| r["layout"] == json!(true)), "the panes it has are kept: {:?}", since(n));
        assert!(since(n).iter().all(|r| r.get("blocks").is_some()), "none reads everything: {:?}", since(n));
        host.edit(json!({ "op": "rename_session", "session": "s", "name": "x" })).0.unwrap();
        assert_eq!(fake.requests().pop().unwrap()["layout"], json!(true));
    }

    #[test]
    fn the_first_snapshot_waits_for_the_listener_but_not_for_long() {
        let fake = FakeRex::new(&["deaf"]);
        let start = Instant::now();
        let host = fake.host(Duration::from_millis(900));
        // Requests are answered meanwhile.
        assert_eq!(host.call(json!({ "op": "hosts" })), Ok(json!("done")));
        assert_eq!(host.lock().taken, 0, "what happens before it listens would be in no event");
        until("a snapshot without it", || host.lock().taken == 1);
        assert!(start.elapsed() >= Duration::from_millis(900));
        // One that ended before it listened is not waited for either.
        let fake = FakeRex::new(&["deaf", "listener-ends", "end"]);
        let start = Instant::now();
        let host = fake.host(Duration::from_secs(3));
        until("the first snapshot", || host.lock().taken == 1);
        assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
        // One that listens is not waited for.
        let fake = FakeRex::new(&[]);
        let start = Instant::now();
        let host = fake.host(Duration::from_secs(3));
        until("the first snapshot", || host.lock().taken == 1);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn trouble_is_told_under_the_hosts_name() {
        let fake = FakeRex::new(&[]);
        let mut host = fake.host(Duration::from_secs(1));
        until("the first snapshot", || host.lock().taken == 1);
        assert_eq!(host.trouble(), None);
        host.lock().deaf = Some("no such event".into());
        assert_eq!(host.trouble().as_deref(), Some("rex hears no news: no such event"));
        host.label = Some("devbox".into());
        host.lock().error = Some("devbox stopped answering".into());
        assert_eq!(host.trouble().as_deref(), Some("devbox stopped answering"), "the worker's trouble first, named once");
        host.lock().error = Some("connection refused".into());
        assert_eq!(host.trouble().as_deref(), Some("devbox: connection refused"));
    }

    #[test]
    fn a_host_long_at_reading_everything_says_so_at_once() {
        let fake = FakeRex::new(&[]);
        let host = fake.host(Duration::from_secs(1));
        until("the first snapshot", || host.lock().taken == 1);
        host.lock().reading = Some(Instant::now());
        assert_eq!(host.busy(), Ok(()), "a moment's reading is waited for");
        host.lock().reading = Some(Instant::now() - Duration::from_secs(5));
        let (answer, lost) = host.edit(json!({ "op": "rename_session", "session": "s", "name": "x" }));
        assert_eq!((answer, lost), (Err("rex is still reading its sessions: try again in a moment".into()), false));
        assert!(!fake.log().contains("edit"), "not made: {}", fake.log());
    }

    #[test]
    fn an_edit_refused_is_not_one_whose_answer_was_lost() {
        let fake = FakeRex::new(&[]);
        let host = fake.host(Duration::from_secs(1));
        until("the first snapshot", || host.lock().taken == 1);
        assert_eq!(host.edit(json!({ "op": "refuse" })), (Err("no such tab".into()), false), "the server said no");
        let (answer, lost) = host.edit(json!({ "op": "vanish" }));
        assert!(answer.is_err() && lost, "the worker went without a word: made or not, who knows");
    }

    #[test]
    fn every_edit_is_a_worker_request_except_paddocks_own() {
        let title = Op::SetTitle { pane: "block:1".into(), title: None };
        assert_eq!(Rex::request(&title), Some(json!({ "op": "rename_pane", "block": "block:1", "name": "" })));
        assert_eq!(Rex::request(&Op::SetHidden { pane: "block:1".into(), hidden: true }), None);
        assert_eq!(Rex::request(&Op::SetHiddenExpanded(true)), None);
        let join = Op::JoinPane { pane: "block:1".into(), to: "block:2".into(), before: true };
        assert_eq!(Rex::request(&join), Some(json!({ "op": "join_pane", "pane": "block:1", "to": "block:2", "before": true })));
    }
}

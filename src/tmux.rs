//! One tmux control-mode connection (`tmux -C`) for the popup's lifetime:
//! commands go over it without forking a process each, and tmux pushes
//! structural changes and agent-state subscriptions back as notifications.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

#[derive(Debug, PartialEq)]
pub enum Msg {
    /// Something changed on the server that a snapshot should reflect.
    Changed,
    /// The refresh subscription's new value: a snapshot's text.
    Rows(String),
    /// The control client exited (server gone, or our session killed last).
    Exit,
}

/// Separates the fields of one format's output.
pub const SEP: char = '\u{1f}';

pub struct Control {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<Result<Vec<String>, String>>,
    /// Replies received so far.
    replied: u64,
    /// A reply timed out: a late one would answer the next command.
    dead: bool,
}

/// How long a command may take before the connection is given up.
const TIMEOUT: Duration = Duration::from_secs(3);

/// The session the control client attaches to, created on open and
/// destroyed when the popup closes (kept, it would keep the server alive and
/// be the session a bare `tmux attach` picks). A client attached to the
/// user's session would be a candidate whenever a command run from a pane
/// there names no client, and the newest one until the next key press.
pub const SESSION: &str = "_paddock";

/// Hooks attaching or detaching fires (the attach also focuses SESSION's
/// pane), emptied on SESSION so the user's global ones do not run for
/// paddock (client-created fires before the client has a session, so only
/// the global one applies).
const HOOKS: &[&str] = &["client-attached", "client-session-changed", "client-resized", "client-active", "client-detached", "client-closed", "pane-focus-in"];

/// `tmux` invocation for the server the popup runs on. Inside tmux the TMUX
/// variable names the socket; it is dropped so `attach` is not refused as
/// nesting.
fn tmux_command() -> Command {
    let mut cmd = Command::new("tmux");
    if let Ok(env) = std::env::var("TMUX") {
        if let Some(socket) = env.split(',').next().filter(|s| !s.is_empty()) {
            cmd.arg("-S").arg(socket);
        }
        cmd.env_remove("TMUX");
    }
    cmd
}

impl Control {
    /// Attach a control client to SESSION without resizing windows or
    /// streaming pane output. Notifications are passed to `events` with the
    /// number of replies tmux sent before them: one sent before a reply
    /// (`replied`) is about the server before that command ran.
    pub fn connect(events: impl Fn(u64, Msg) + Send + 'static) -> Result<Control, String> {
        // Exact: a bare name is also tried as a prefix and as a window.
        let target = format!("={SESSION}:");
        let mut cmd = tmux_command();
        // -u: UTF-8 output whatever the popup's locale says.
        cmd.args(["-u", "-C"]);
        // Create SESSION unless it exists (another popup is open), in the
        // same command list as the attach: tmux runs a list in one go, so
        // another popup closing cannot destroy SESSION in between. if-shell
        // runs the create next, before the overrides.
        let exists = format!("#{{S:#{{?#{{==:#{{session_name}},{SESSION}}},1,}}}}");
        let create = format!("new-session -d -s {SESSION} -n paddock cat");
        cmd.args(["if-shell", "-F", &exists, "", &create, ";"]);
        // Every time, before attaching: a session from an older paddock or
        // restored by tmux-resurrect has no overrides.
        for hook in HOOKS {
            cmd.args(["set-hook", "-t", &target, hook, "", ";"]);
        }
        cmd.args(["set-option", "-t", &target, "destroy-unattached", "on", ";"]);
        cmd.args(["attach-session", "-f", "ignore-size,no-output", "-t", &target]);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot run tmux: {e}"))?;
        let stdin = child.stdin.take().ok_or("no tmux stdin")?;
        let stdout = child.stdout.take().ok_or("no tmux stdout")?;
        let (reply_tx, replies) = mpsc::channel();
        thread::spawn(move || read_loop(BufReader::new(stdout), reply_tx, events));
        Ok(Control { child, stdin, replies, replied: 0, dead: false })
    }

    /// How many commands have had their reply.
    pub fn replied(&self) -> u64 {
        self.replied
    }

    /// Whether a command timed out: the connection is no longer usable.
    pub fn is_dead(&self) -> bool {
        self.dead
    }

    /// Run one tmux command (tmux command syntax, so quote formats: `#`
    /// starts a comment) and return its output lines.
    pub fn cmd(&mut self, line: &str) -> Result<Vec<String>, String> {
        // A newline would end the command early and leave a second reply.
        if line.contains('\n') {
            return Err("tmux command contains a newline".into());
        }
        if self.dead {
            return Err("tmux stopped answering".into());
        }
        writeln!(self.stdin, "{line}").map_err(|e| format!("tmux connection lost: {e}"))?;
        self.stdin.flush().map_err(|e| format!("tmux connection lost: {e}"))?;
        let reply = self.replies.recv_timeout(TIMEOUT).map_err(|e| {
            self.dead = true;
            match e {
                mpsc::RecvTimeoutError::Timeout => "tmux stopped answering".to_string(),
                mpsc::RecvTimeoutError::Disconnected => "tmux connection lost".to_string(),
            }
        })?;
        self.replied += 1;
        reply
    }

    /// The one row of `display-message -p` over NAMES in TARGET (so its
    /// session and window options count); empty on failure. A line with
    /// another field count (a notification inside the reply, before tmux
    /// 3.8) is no row.
    pub fn row(&mut self, target: &str, names: &[&str]) -> Vec<String> {
        let lines = self.cmd(&format!("display-message -p -t {} {}", quote(target), quote(&fields(names)))).unwrap_or_default();
        lines.iter().map(|l| l.split(SEP).map(String::from).collect::<Vec<_>>()).find(|f| f.len() == names.len()).unwrap_or_default()
    }
}

/// What tmux makes itself (ids, indexes, flags): no control characters,
/// so `field` leaves them as they are (a regex per field and pane costs).
const PLAIN: &[&str] = &["session_id", "window_id", "window_index", "window_active", "automatic-rename", "pane_id", "pane_index", "pane_active"];

/// `#{NAME}` with control characters as spaces: tmux does not escape
/// values in replies or notifications, so a newline in a path or option
/// would forge control lines, and a SEP (or a snapshot's marks, model.rs)
/// would shift the fields. NAME may carry modifiers (`E:name`).
pub fn field(name: &str) -> String {
    if PLAIN.contains(&name) {
        return format!("#{{{name}}}");
    }
    let (mods, key) = name.rsplit_once(':').map_or((String::new(), name), |(m, k)| (format!("{m};"), k));
    format!("#{{{mods}s/[\x01-\x1f]/ /:{key}}}")
}

/// The `field`s of NAMES, separated by SEP.
pub fn fields(names: &[&str]) -> String {
    names.iter().map(|n| field(n)).collect::<Vec<_>>().join(&SEP.to_string())
}

impl Drop for Control {
    fn drop(&mut self) {
        // An empty line detaches a control client; closing stdin also does.
        let _ = writeln!(self.stdin);
        if self.dead {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// Split the control stream into replies to our commands (%begin/%end
/// blocks with flags 1) and notifications. A block ends only at the %end or
/// %error carrying its %begin's time, number and flags, so output that looks
/// like a guard line (a captured pane showing one) stays inside. Blocks with
/// flags 0 (the attach itself) are dropped, but a failed attach's error is
/// the reply to whichever command is waiting when tmux exits. Each
/// notification goes with the number of replies sent before it.
fn read_loop(out: impl BufRead, replies: Sender<Result<Vec<String>, String>>, events: impl Fn(u64, Msg)) {
    struct Block {
        guard: String,
        ours: bool,
        lines: Vec<String>,
    }
    let mut block: Option<Block> = None;
    let mut attach_error = None;
    let mut replied = 0;
    for line in out.split(b'\n') {
        let Ok(line) = line else { break };
        let line = String::from_utf8_lossy(&line).into_owned();
        if let Some(b) = block.as_mut() {
            let end = line.strip_prefix("%end ") == Some(&b.guard);
            if end || line.strip_prefix("%error ") == Some(&b.guard) {
                let lines = std::mem::take(&mut b.lines);
                if b.ours {
                    let _ = replies.send(if end { Ok(lines) } else { Err(lines.join(" ")) });
                    replied += 1;
                } else if !end {
                    attach_error = Some(lines.join(" "));
                }
                block = None;
            } else {
                b.lines.push(line);
            }
            continue;
        }
        if let Some(guard) = line.strip_prefix("%begin ") {
            block = Some(Block { guard: guard.to_string(), ours: guard.ends_with(" 1"), lines: Vec::new() });
        } else if line == "%exit" || line.starts_with("%exit ") {
            break;
        } else if let Some(change) = line.strip_prefix("%subscription-changed ") {
            // `NAME $S - - - : VALUE`; the only subscription is the refresh one.
            if let Some((_, value)) = change.split_once(" : ") {
                events(replied, Msg::Rows(value.to_string()));
            }
        } else if is_change(&line) {
            events(replied, Msg::Changed);
        }
    }
    if let Some(e) = attach_error {
        let _ = replies.send(Err(e));
    }
    events(replied, Msg::Exit);
}

/// Notifications that can change what the popup shows.
fn is_change(line: &str) -> bool {
    const KINDS: &[&str] = &[
        "%sessions-changed",
        "%session-renamed",
        "%window-add",
        "%window-close",
        "%window-renamed",
        "%unlinked-window-add",
        "%unlinked-window-close",
        "%unlinked-window-renamed",
        "%layout-change",
        "%window-pane-changed",
        "%session-window-changed",
    ];
    KINDS.iter().any(|k| line.starts_with(k))
}

/// Quote a value for tmux command syntax (single quotes, ' escaped).
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    type Replies = Vec<Result<Vec<String>, String>>;

    fn run(stream: &str) -> (Replies, Vec<Msg>) {
        let (replies, events) = run_counted(stream);
        (replies, events.into_iter().map(|(_, m)| m).collect())
    }

    /// RUN, with the replies sent before each notification.
    fn run_counted(stream: &str) -> (Replies, Vec<(u64, Msg)>) {
        let (tx, rx) = mpsc::channel();
        let (etx, erx) = mpsc::channel();
        read_loop(stream.as_bytes(), tx, move |n, m| etx.send((n, m)).unwrap());
        (rx.try_iter().collect(), erx.try_iter().collect())
    }

    #[test]
    fn replies_and_notifications() {
        let (replies, events) = run(concat!(
            "%begin 1 10 0\n%end 1 10 0\n",
            "%session-changed $0 a\n",
            "%begin 1 11 1\nhello\nworld\n%end 1 11 1\n",
            "%window-add @3\n",
            "%subscription-changed panes $1 - - - : a : b\n",
            "%begin 1 12 1\nno such pane\n%error 1 12 1\n",
        ));
        assert_eq!(replies, vec![Ok(vec!["hello".to_string(), "world".to_string()]), Err("no such pane".to_string())]);
        assert_eq!(events, vec![Msg::Changed, Msg::Rows("a : b".into()), Msg::Exit]);
    }

    #[test]
    fn notifications_count_the_replies_before_them() {
        let (_, events) = run_counted(concat!(
            "%begin 1 10 0\n%end 1 10 0\n",
            "%sessions-changed\n",
            "%begin 1 11 1\n%end 1 11 1\n",
            "%begin 1 12 1\nno\n%error 1 12 1\n",
            "%subscription-changed panes $1 - - - : v\n",
        ));
        assert_eq!(events, vec![(0, Msg::Changed), (2, Msg::Rows("v".into())), (2, Msg::Exit)], "the attach's block is no reply");
    }

    #[test]
    fn guard_lookalikes_in_output_stay_in_the_block() {
        let (replies, events) = run(concat!(
            "%begin 1 20 1\n%end 9 9 1\n%error 1 2 1\n%exit\n%begin 3 4 1\n%window-add @1\n%end 1 20 1\n",
            "%begin 1 21 1\nnext\n%end 1 21 1\n",
        ));
        let first = ["%end 9 9 1", "%error 1 2 1", "%exit", "%begin 3 4 1", "%window-add @1"].map(String::from).to_vec();
        assert_eq!(replies, vec![Ok(first), Ok(vec!["next".to_string()])]);
        assert_eq!(events, vec![Msg::Exit]);
    }

    #[test]
    fn a_failed_attach_answers_the_waiting_command() {
        let (replies, events) = run("%begin 1 30 0\ncan't find session: nosuch\n%error 1 30 0\n%exit\n");
        assert_eq!(replies, vec![Err("can't find session: nosuch".to_string())]);
        assert_eq!(events, vec![Msg::Exit]);
    }
}

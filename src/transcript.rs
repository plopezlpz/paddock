//! What an agent last said, from the transcript its tool names in
//! @agent-transcript (agent-status.tmux sets it for Claude Code, pi and
//! Codex): for a waiting agent, the call it waits on (`Bash: cargo test`, or
//! the question it asks), the one @agent-asks names when it names one; for a
//! finished or idle one, the first line of its last answer. A working agent's transcript moves on between snapshots, so
//! it gets none.
//!
//! Read from the file's last TAIL bytes, newest line first, tolerantly: a
//! line that is no message of a known shape is skipped. What a file says is
//! kept until it changes (its length or mtime), so an unchanged transcript
//! costs a stat per snapshot.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;
use std::time::SystemTime;

use serde_json::Value;

use crate::model::State;

/// How much of a transcript's end is read.
const TAIL: u64 = 256 * 1024;
/// A note's longest, in characters: the card cuts it to fit anyway.
const MAX: usize = 160;

/// What a transcript said last: its newest message's last part (a call or a
/// text), its newest text, and the calls since, newest first, by tool.
#[derive(Clone, Debug, Default, PartialEq)]
struct Said {
    last: String,
    text: String,
    calls: Vec<(String, String)>,
}

type Stamp = (u64, SystemTime);

static CACHE: Mutex<Option<HashMap<String, (Stamp, Said)>>> = Mutex::new(None);

/// The note for an agent in STATE whose transcript is PATH. ASKS: its open
/// asks (@agent-asks: `<agent>/<tool>[#call]@<opened>`, oldest first).
pub fn note(path: &str, state: State, asks: &str) -> String {
    if path.is_empty() || state == State::Working {
        return String::new();
    }
    let Ok(meta) = std::fs::metadata(path) else { return String::new() };
    let stamp = (meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH));
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    let said = match cache.get(path) {
        Some((s, said)) if *s == stamp => said.clone(),
        _ => {
            let said = read(path, stamp.0).unwrap_or_default();
            cache.insert(path.to_string(), (stamp, said.clone()));
            said
        }
    };
    if state != State::Asking {
        return said.text;
    }
    asked(&said, asks)
}

/// What a waiting agent waits on: the oldest open ask's call, as the
/// transcript has it; a subagent's, by its tool (its calls are in a
/// transcript of its own); else the newest part. Codex's asks name every
/// call Bash (with the command's hash), its transcript `shell`: one not
/// found by name is the newest part too.
fn asked(said: &Said, asks: &str) -> String {
    let Some((agent, rest)) = asks.split_whitespace().next().and_then(|a| a.split_once('/')) else { return said.last.clone() };
    let tool = rest.split(['#', '@']).next().unwrap_or_default();
    let hashed = rest.contains('#');
    match (agent.is_empty(), tool.is_empty()) {
        (_, true) => said.last.clone(),
        (false, false) => format!("subagent: {}", short(tool)),
        (true, false) => match said.calls.iter().find(|(name, _)| name == tool) {
            Some((_, call)) => call.clone(),
            None if hashed => said.last.clone(),
            None => short(tool).to_string(),
        },
    }
}

/// A tool's name without its MCP server's prefix.
fn short(name: &str) -> &str {
    name.rsplit("__").next().unwrap_or(name)
}

fn read(path: &str, len: u64) -> Option<Said> {
    let mut file = File::open(path).ok()?;
    let from = len.saturating_sub(TAIL);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.take(TAIL).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    // Started mid-line: the first is partial.
    let lines: Vec<&str> = text.lines().skip(usize::from(from > 0)).collect();
    Some(said(lines.into_iter()))
}

/// What LINES (a transcript's, oldest first) said last.
fn said<'a>(lines: impl DoubleEndedIterator<Item = &'a str>) -> Said {
    let mut out = Said::default();
    for line in lines.rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let parts = parts(&v);
        if out.last.is_empty()
            && let Some(Part::Text(s) | Part::Call(_, s)) = parts.last()
        {
            out.last = s.clone();
        }
        out.calls.extend(parts.iter().rev().filter_map(|p| match p {
            Part::Call(name, call) => Some((name.clone(), call.clone())),
            Part::Text(_) => None,
        }));
        if let Some(text) = parts.iter().rev().find_map(|p| match p {
            Part::Text(t) => Some(t),
            Part::Call(..) => None,
        }) {
            out.text = text.clone();
            break;
        }
    }
    out
}

/// Part of an assistant message, as a note: a call with its tool's name.
enum Part {
    Text(String),
    Call(String, String),
}

/// An assistant message's texts and calls, in order; none for anything else.
fn parts(v: &Value) -> Vec<Part> {
    if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return Vec::new();
    }
    // Claude and pi: `message`; Codex: `payload`.
    let Some(m) = v.get("message").or_else(|| v.get("payload")).filter(|m| m.is_object()) else { return Vec::new() };
    let kind = m.get("type").and_then(Value::as_str).unwrap_or_default();
    // Codex: a call is an item of its own.
    if kind.ends_with("_call")
        && let Some(name) = m.get("name").and_then(Value::as_str)
    {
        let args = m.get("arguments").or_else(|| m.get("input"));
        return vec![Part::Call(name.to_string(), call(name, args))];
    }
    if m.get("role").and_then(Value::as_str) != Some("assistant") {
        return Vec::new();
    }
    let Some(content) = m.get("content").and_then(Value::as_array) else {
        return m.get("content").and_then(Value::as_str).and_then(first_line).map(Part::Text).into_iter().collect();
    };
    content
        .iter()
        .filter_map(|c| match c.get("type").and_then(Value::as_str)? {
            "text" | "output_text" => first_line(c.get("text")?.as_str()?).map(Part::Text),
            "tool_use" | "toolCall" => {
                let name = c.get("name")?.as_str()?;
                Some(Part::Call(name.to_string(), call(name, c.get("input").or_else(|| c.get("arguments")))))
            }
            _ => None,
        })
        .collect()
}

/// A call as a note: its tool and what it acts on (a command, a file, …);
/// a question, as the question.
fn call(name: &str, args: Option<&Value>) -> String {
    // Codex passes its arguments as JSON text.
    let parsed;
    let args = match args {
        Some(Value::String(s)) => {
            parsed = serde_json::from_str::<Value>(s).ok();
            parsed.as_ref()
        }
        other => other,
    };
    let name = short(name);
    if let Some(q) = args.and_then(|a| a.pointer("/questions/0/question")).and_then(Value::as_str) {
        return clip(q);
    }
    let what = args.and_then(|a| {
        ["command", "cmd", "file_path", "path", "pattern", "url", "query", "description", "prompt"].iter().find_map(|k| match a.get(k)? {
            Value::String(s) => Some(s.clone()),
            // Codex: ["bash", "-lc", "cargo test"].
            Value::Array(parts) => parts.last()?.as_str().map(String::from),
            _ => None,
        })
    });
    // As written: a path or a pattern is no markdown.
    let what = what.as_deref().and_then(|w| w.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).find(|l| !l.is_empty()));
    match what {
        Some(what) => clip(&format!("{name}: {what}")),
        None => name.to_string(),
    }
}

/// TEXT's first line with words, without markdown's marks, its whitespace
/// collapsed.
fn first_line(text: &str) -> Option<String> {
    let line = text.lines().map(|l| l.trim().trim_start_matches(['#', '>', '-', '*', ' '])).find(|l| !l.trim().is_empty())?;
    let plain: String = line.replace("**", "").replace("__", "").replace('`', "");
    let words = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    (!words.is_empty()).then(|| clip(&words))
}

fn clip(s: &str) -> String {
    match s.char_indices().nth(MAX) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(lines: &[&str]) -> Said {
        said(lines.iter().copied())
    }

    #[test]
    fn claude_answers_and_calls() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"fix it"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"x"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"\n## **Done.** Tests\n  pass now\nmore"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test\n--locked","description":"run"}}]}}"#,
            r#"{"type":"system","subtype":"informational"}"#,
            r#"{"type":"ai-title","aiTitle":"t"}"#,
            "not json",
        ];
        let said = of(&lines);
        assert_eq!((said.last.as_str(), said.text.as_str()), ("Bash: cargo test", "Done. Tests"));
        let said = of(&lines[..3]);
        assert_eq!((said.last.as_str(), said.text.as_str()), ("Done. Tests", "Done. Tests"), "a text, last");
    }

    #[test]
    fn the_open_ask_names_the_call() {
        let lines = [
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Let me check."},{"type":"tool_use","name":"Bash","input":{"command":"rm -rf build"}},{"type":"tool_use","name":"Read","input":{"file_path":"a.rs"}}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Agent","input":{"description":"review"}}]}}"#,
        ];
        let said = of(&lines);
        assert_eq!(asked(&said, ""), "Agent: review", "no ask named: the newest call");
        assert_eq!(asked(&said, "/Bash@100"), "Bash: rm -rf build", "the main thread's ask, by its tool");
        assert_eq!(asked(&said, "/Bash#abc@100 a1/Edit@101"), "Bash: rm -rf build", "the oldest ask");
        assert_eq!(asked(&said, "a1/Edit@101"), "subagent: Edit", "a subagent's, by its tool");
        assert_eq!(asked(&said, "/mcp__srv__fetch@100"), "fetch", "a tool not in the transcript");
        assert_eq!(asked(&said, "/@100"), "Agent: review", "a dialog without a tool");
        let codex = [r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"bash\",\"-lc\",\"make test\"]}"}}"#];
        assert_eq!(asked(&of(&codex), "/Bash#5f3a@100"), "shell: make test", "Codex: every ask is Bash, its calls shell");
    }

    #[test]
    fn a_question_is_its_question() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"AskUserQuestion","input":{"questions":[{"question":"Which key?","options":[]}]}}]}}"#;
        assert_eq!(of(&[line]).last, "Which key?");
        let glob = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Glob","input":{"pattern":"**/__tests__/*.rs"}}]}}"#;
        assert_eq!(of(&[glob]).last, "Glob: **/__tests__/*.rs", "arguments as written");
        let mcp = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"mcp__pencil__execute","input":{}}]}}"#;
        assert_eq!(of(&[mcp]).last, "execute", "no argument it names: the tool alone");
    }

    #[test]
    fn pi_and_codex() {
        let pi = [
            r#"{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"Done. Changed **2,925** items"}]}}"#,
            r#"{"type":"message","message":{"role":"assistant","content":[{"type":"thinking","thinking":""},{"type":"toolCall","name":"bash","arguments":{"command":"ls -la"}}]}}"#,
        ];
        let said = of(&pi);
        assert_eq!((said.last.as_str(), said.text.as_str()), ("bash: ls -la", "Done. Changed 2,925 items"));
        let codex = [
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Yes—`Setup` is procedural"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"bash\",\"-lc\",\"make test\"]}"}}"#,
            r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#,
        ];
        let said = of(&codex);
        assert_eq!((said.last.as_str(), said.text.as_str()), ("shell: make test", "Yes—Setup is procedural"));
    }

    #[test]
    fn subagents_and_users_say_nothing() {
        let lines = [
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"mine"}]}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"role":"assistant","content":[{"type":"text","text":"a subagent's"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"the user's"}]}}"#,
        ];
        let said = of(&lines);
        assert_eq!((said.last.as_str(), said.text.as_str()), ("mine", "mine"));
        assert_eq!(of(&[]), Said::default());
    }

    #[test]
    fn long_lines_are_clipped() {
        let long = "word ".repeat(100);
        let n = first_line(&long).unwrap();
        assert_eq!(n.chars().count(), MAX + 1);
        assert!(n.ends_with('…'));
    }

    #[test]
    fn notes_by_state_from_a_file_read_from_its_end() {
        let dir = std::env::temp_dir().join(format!("paddock-transcript-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        // A first line longer than the tail: only its end is read, and skipped.
        let big = format!(r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"{}"}}]}}}}"#, "x".repeat(TAIL as usize));
        let lines = [
            big.as_str(),
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"All green."}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Edit","input":{"file_path":"src/ui.rs"}}]}}"#,
        ];
        std::fs::write(&path, lines.join("\n")).unwrap();
        let p = path.to_str().unwrap();
        assert_eq!(note(p, State::Asking, ""), "Edit: src/ui.rs");
        assert_eq!(note(p, State::Finished, ""), "All green.");
        assert_eq!(note(p, State::Idle, ""), "All green.");
        assert_eq!(note(p, State::Working, ""), "", "moving on: none");
        assert_eq!(note("/nonexistent/t.jsonl", State::Idle, ""), "");
        std::fs::write(&path, lines[..2].join("\n")).unwrap();
        assert_eq!(note(p, State::Asking, ""), "All green.", "a changed file is read again");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

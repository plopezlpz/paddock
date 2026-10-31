//! Plain-text notification content, without opening tmux or sending anything.
//! Consumers use line one as the title and line two as the body.

use crate::paths::PathCache;
use crate::theme::{agent_icon, AgentIcons};

pub fn format(state: &str, kind: &str, path: &str, home: &str, icons: AgentIcons) -> String {
    let state = if state == "asking" { "waiting" } else { state };
    let mut paths = PathCache::new(home.into());
    let path = paths.get(path).display(100);
    // Directory names may contain newlines, control bytes or ANSI escapes:
    // the output contract is exactly two plain-text lines, never a template.
    let body: String = path.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let body = if body.trim().is_empty() { "unknown directory" } else { body.trim() };
    format!("{} {state}\n{body}", agent_icon(kind, icons))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_share_card_icons_and_path_formatting() {
        for icons in [AgentIcons::Unicode, AgentIcons::Nerd] {
            for kind in ["claude", "codex", "pi", "opencode", "gemini", "unknown"] {
                let text = format("finished", kind, "/home/test/project", "/home/test", icons);
                assert_eq!(text, std::format!("{} finished\n~/project", agent_icon(kind, icons)));
            }
        }
        assert_eq!(format("asking", "pi", "/tmp/project", "", AgentIcons::Unicode), "π waiting\n/tmp/project");
        assert_eq!(format("failed", "pi", "/tmp/project", "", AgentIcons::Unicode), "π failed\n/tmp/project");
    }

    #[test]
    fn output_is_two_safe_lines_even_for_unusual_directory_names() {
        let text = format("finished", "claude", "/tmp/a\nb\rc\td\u{1b}", "", AgentIcons::Unicode);
        assert_eq!(text.lines().count(), 2);
        assert!(!text.contains(['\r', '\t', '\u{1b}']));
        assert_eq!(format("finished", "pi", "", "", AgentIcons::Unicode), "π finished\nunknown directory");
    }
}

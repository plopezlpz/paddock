//! Changes waiting for review in an agent's folder: lines added and removed
//! since HEAD (staged or not) and untracked files, from git (10 to 150 ms on
//! real repos: asked for on the popup's worker, see worker.rs).


#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Diff {
    pub added: u32,
    pub removed: u32,
    /// Untracked files (not ignored).
    pub new: u32,
}

impl Diff {
    pub fn is_clean(&self) -> bool {
        *self == Diff::default()
    }

    /// `+12 −3 · 2 untracked`, without what is zero; empty when clean.
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if self.added > 0 || self.removed > 0 {
            parts.push(format!("+{} −{}", self.added, self.removed));
        }
        if self.new > 0 {
            parts.push(format!("{} untracked", self.new));
        }
        parts.join(" · ")
    }
}

fn git(dir: &str, args: &[&str]) -> Option<String> {
    let out = crate::paths::git(dir).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// DIR's changes, if it is in a repository with a HEAD.
pub fn diff(dir: &str) -> Option<Diff> {
    let stat = git(dir, &["--no-optional-locks", "diff", "HEAD", "--shortstat"])?;
    let new = git(dir, &["--no-optional-locks", "ls-files", "--others", "--exclude-standard", "--", ":/"]).map_or(0, |o| o.lines().count() as u32);
    Some(Diff { new, ..shortstat(&stat) })
}

/// ` 3 files changed, 12 insertions(+), 3 deletions(-)`, either count
/// possibly missing; empty when there is no change.
fn shortstat(text: &str) -> Diff {
    let mut d = Diff::default();
    for part in text.trim().split(", ") {
        let mut words = part.split_whitespace();
        let (Some(n), Some(what)) = (words.next().and_then(|n| n.parse().ok()), words.next()) else { continue };
        if what.starts_with("insertion") {
            d.added = n;
        } else if what.starts_with("deletion") {
            d.removed = n;
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    #[test]
    fn reads_shortstat() {
        assert_eq!(shortstat(" 3 files changed, 12 insertions(+), 3 deletions(-)\n"), Diff { added: 12, removed: 3, new: 0 });
        assert_eq!(shortstat(" 1 file changed, 1 insertion(+)"), Diff { added: 1, removed: 0, new: 0 });
        assert_eq!(shortstat(" 1 file changed, 2 deletions(-)"), Diff { added: 0, removed: 2, new: 0 });
        assert_eq!(shortstat(""), Diff::default());
    }

    #[test]
    fn labels_say_only_what_is_there() {
        assert_eq!(Diff { added: 12, removed: 3, new: 2 }.label(), "+12 −3 · 2 untracked");
        assert_eq!(Diff { added: 0, removed: 4, new: 0 }.label(), "+0 −4");
        assert_eq!(Diff { added: 0, removed: 0, new: 1 }.label(), "1 untracked");
        assert!(Diff::default().label().is_empty() && Diff::default().is_clean());
    }

    #[test]
    fn a_repository_is_diffed_and_a_plain_folder_is_not() {
        let dir = std::env::temp_dir().join(format!("paddock-diffstat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_str().unwrap();
        assert_eq!(diff(d), None, "not a repository");
        let run = |args: &[&str]| assert!(crate::paths::git(d).args(args).stdout(Stdio::null()).status().unwrap().success());
        run(&["init", "-q"]);
        std::fs::write(dir.join("a"), "1\n2\n3\n").unwrap();
        run(&["add", "a"]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "a"]);
        assert_eq!(diff(d), Some(Diff::default()), "clean");
        std::fs::write(dir.join("a"), "1\nchanged\n3\nadded\n").unwrap();
        std::fs::write(dir.join("b"), "new\n").unwrap();
        assert_eq!(diff(d), Some(Diff { added: 2, removed: 1, new: 1 }));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

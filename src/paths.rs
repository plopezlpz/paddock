//! Agent paths: discover repository anchors once per directory, collapse the
//! route to linked worktrees, then fit to width. Sessions keep their labels.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::rows::{home_path, short_path};

#[derive(Clone, Debug)]
pub struct AgentPath {
    /// The actual directory, with only HOME replaced by ~.
    pub full: String,
    /// Repo-relative, or main-repo/…/worktree with any subdirectory suffix.
    label: String,
    anchored: bool,
    head: Option<GitHead>,
}

#[derive(Clone, Debug)]
struct GitHead {
    name: String,
    detached: bool,
}

impl GitHead {
    fn label(&self) -> String {
        if self.detached { format!("detached {}", &self.name[..7]) } else { self.name.clone() }
    }
}

impl AgentPath {
    pub fn plain(path: &str, home: &str) -> Self {
        Self { full: home_path(path, home), label: short_path(path, home), anchored: false, head: None }
    }

    /// Search never depends on display truncation.
    pub fn search_text(&self) -> String {
        format!("{} {}", self.label, self.head.as_ref().map_or("", |head| head.name.as_str()))
    }

    pub fn display(&self, width: usize) -> String {
        let Some(head) = &self.head else { return self.display_path(width) };
        let label = head.label();
        if self.label.width() + label.width() + 3 <= width {
            return format!("{} ({label})", self.label);
        }
        // Keep the Git suffix even when branch and directory names match.
        // Prefer the repo anchor and give the branch up to a third of a narrow
        // card; below five columns there is no room for both path and suffix.
        if width < 5 { return self.display_path(width); }
        let anchor = self.label.split('/').next().unwrap_or("").width();
        let room = (width / 3).max(4).min(width.saturating_sub(anchor).max(4));
        let branch = prefix(&label, room - 3);
        let suffix = format!(" ({branch})");
        format!("{}{suffix}", self.display_path(width.saturating_sub(suffix.width())))
    }

    fn display_path(&self, width: usize) -> String {
        if !self.anchored || self.label.width() <= width {
            return prefix(&self.label, width);
        }
        let parts: Vec<&str> = self.label.split('/').collect();
        let repo = parts[0];
        // Preserve the anchor and as much of the tail as fits. Never silently
        // drop the repo in favour of a long worktree name.
        for start in 1..parts.len() {
            let tail = parts[start..].join("/");
            let label = format!("{repo}/…/{tail}");
            if label.width() <= width {
                return label;
            }
        }
        let base = format!("{repo}/…/");
        if base.width() < width {
            format!("{base}{}", prefix(parts.last().unwrap(), width - base.width()))
        } else {
            prefix(repo, width)
        }
    }
}

fn prefix(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let n = c.width().unwrap_or(0);
        if used + n + 1 > width {
            break;
        }
        out.push(c);
        used += n;
    }
    if width > 0 {
        out.push('…');
    }
    out
}

#[derive(Default)]
pub struct PathCache {
    home: String,
    labels: HashMap<String, Option<Repository>>,
}

impl PathCache {
    pub fn new(home: String) -> Self {
        Self { home, labels: HashMap::new() }
    }

    pub fn home(&self) -> &str { &self.home }

    pub fn get(&mut self, path: &str) -> AgentPath {
        let home = &self.home;
        let repo = self.labels.entry(path.to_string()).or_insert_with(|| repository_info(Path::new(path)));
        match repo {
            Some(repo) => AgentPath {
                full: home_path(path, home), label: repo.label.clone(), anchored: true,
                head: repo.git_dir.as_deref().and_then(read_head),
            },
            None => AgentPath::plain(path, home),
        }
    }
}

struct Repository {
    label: String,
    git_dir: Option<PathBuf>,
}

fn git_dir(root: &Path) -> Option<PathBuf> {
    let marker = root.join(".git");
    if marker.is_dir() { return Some(marker); }
    let contents = std::fs::read_to_string(marker).ok()?;
    Some(root.join(contents.trim_end_matches(['\r', '\n']).strip_prefix("gitdir: ")?))
}

fn read_head(dir: &Path) -> Option<GitHead> {
    let contents = std::fs::read_to_string(dir.join("HEAD")).ok()?;
    let head = contents.trim();
    if let Some(branch) = head.strip_prefix("ref: refs/heads/").filter(|s| !s.is_empty()) {
        return Some(GitHead { name: branch.into(), detached: false });
    }
    if matches!(head.len(), 40 | 64) && head.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(GitHead { name: head.into(), detached: true });
    }
    None
}

#[cfg(test)]
fn repository_label(path: &Path) -> Option<String> {
    repository_info(path).map(|repo| repo.label)
}

fn repository_info(path: &Path) -> Option<Repository> {
    let path = path.canonicalize().ok()?;
    let repos: Vec<&Path> = path.ancestors().filter(|p| p.join(".git").exists()).collect();
    let outermost = *repos.last()?;
    // The branch belongs to the nearest repository, even when the path label
    // is anchored at an enclosing repository or worktree's main checkout.
    let git_dir = git_dir(repos[0]);
    // A linked worktree's gitfile leads to its private admin directory, whose
    // commondir is the marker distinguishing it from a submodule or a repo
    // with --separate-git-dir. Let Git locate the main worktree rather than
    // assuming the common dir's parent is a checkout.
    // Search nearest-first, past ordinary nested repos and submodules: they
    // must not hide an enclosing linked worktree's main-repository identity.
    let linked = repos.iter().copied().find(|repo| {
        let gitfile = std::fs::read_to_string(repo.join(".git")).ok();
        gitfile.as_deref().and_then(|s| s.trim().strip_prefix("gitdir: "))
            .is_some_and(|dir| repo.join(dir).join("commondir").is_file())
    });
    let main = linked.and_then(main_worktree);
    let anchor = main.as_deref().unwrap_or(outermost);
    let name = anchor.file_name()?.to_str()?;
    if main.is_some() {
        // Git identifies the worktree root regardless of its container names
        // or whether it lives inside the main checkout. Always collapse that
        // route, keeping the worktree's own name and any directory below it.
        let worktree = linked?;
        let tail = path.strip_prefix(worktree.parent()?).ok()?;
        return Some(Repository { label: format!("{name}/…/{}", tail.to_str()?), git_dir });
    }
    let relative = path.strip_prefix(anchor).ok()?;
    Some(Repository {
        label: if relative.as_os_str().is_empty() { name.into() } else { format!("{name}/{}", relative.to_str()?) },
        git_dir,
    })
}

fn main_worktree(root: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["-C"]).arg(root)
        .args(["worktree", "list", "--porcelain", "-z"])
        // Paddock's own launch environment must not redirect discovery.
        .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_CONFIG").env_remove("GIT_CONFIG_COUNT")
        .stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let first = output.stdout.split(|b| *b == 0).next()?;
    let path = std::str::from_utf8(first).ok()?.strip_prefix("worktree ")?;
    let path = Path::new(path).canonicalize().ok()?;
    // With --separate-git-dir Git may report only the admin directory, not
    // the main checkout (there is no backlink). Do not mislabel it as a repo;
    // fall back to the first enclosing repo instead. A bare main repo is
    // intentionally a Git directory and has an explicit porcelain marker.
    let bare = output.stdout.split(|b| *b == 0).take_while(|s| !s.is_empty()).any(|s| s == b"bare");
    (path.join(".git").exists() || bare).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!("paddock-paths-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
        fn git(&self, args: &[&str]) {
            // Mutating fixture commands must never inherit a contributor's
            // GIT_DIR, injected config, global hooks, or init templates.
            let output = Command::new("git").arg("-C").arg(&self.0)
                .env_clear().env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("HOME", &self.0).env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_TERMINAL_PROMPT", "0")
                .args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false",
                    "-c", "core.hooksPath=/dev/null", "-c", "init.templateDir=", "-c", "init.defaultBranch=main"])
                .args(args).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn anchors_at_the_first_repo_from_root_and_falls_back_without_one() {
        let f = Fixture::new();
        let main = f.0.join("main repo");
        f.git(&["init", main.to_str().unwrap()]);
        f.git(&["init", main.join("nested").to_str().unwrap()]);
        let dir = main.join("nested/deep/src");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(repository_label(&dir).unwrap(), "main repo/nested/deep/src");
        assert_eq!(repository_label(&main).unwrap(), "main repo");
        assert!(repository_label(&f.0).is_none());
        assert!(repository_label(&f.0.join("missing")).is_none());
        let plain = PathCache::new("/home/me".into()).get("/home/me/project");
        assert_eq!(plain.full, "~/project");
        assert_eq!(plain.display(40), "~/project");
    }

    #[test]
    fn internal_and_external_worktrees_use_the_main_repo() {
        let f = Fixture::new();
        let main = f.0.join("main repo");
        f.git(&["init", main.to_str().unwrap()]);
        f.git(&["-C", main.to_str().unwrap(), "commit", "--allow-empty", "-m", "initial"]);
        f.git(&["init", f.0.join("host").to_str().unwrap()]);
        for (branch, root, expected) in [
            ("internal", main.join(".worktrees/inside"), "main repo/…/inside/src"),
            ("external", f.0.join("worktrees/outside"), "main repo/…/outside/src"),
            ("nested", f.0.join("host/worktrees/nested"), "main repo/…/nested/src"),
            ("claude", main.join(".claude/worktrees/feature-task"), "main repo/…/feature-task/src"),
            ("arbitrary", f.0.join("any/container/custom checkout"), "main repo/…/custom checkout/src"),
        ] {
            f.git(&["-C", main.to_str().unwrap(), "worktree", "add", "-b", branch, root.to_str().unwrap()]);
            std::fs::create_dir_all(root.join("src")).unwrap();
            assert_eq!(repository_label(&root.join("src")).unwrap(), expected);
            let mut cache = PathCache::new(f.0.to_str().unwrap().into());
            let at_root = cache.get(root.to_str().unwrap());
            let label = expected.strip_suffix("/src").unwrap();
            let display = format!("{label} ({branch})");
            assert_eq!(at_root.display(200), display, "collapse even with plenty of room");
            assert_eq!(at_root.full, home_path(root.to_str().unwrap(), f.0.to_str().unwrap()), "inspection keeps the real path");
        }
        assert_eq!(repository_label(&main).unwrap(), "main repo", "main checkout stays unchanged");
        // Git accepts a relative gitfile too.
        let internal = main.join(".worktrees/inside");
        std::fs::write(internal.join(".git"), "gitdir: ../../.git/worktrees/inside\n").unwrap();
        assert_eq!(repository_label(&internal).unwrap(), "main repo/…/inside");
    }

    #[test]
    fn nested_repos_and_submodules_keep_the_enclosing_worktrees_main_anchor() {
        let f = Fixture::new();
        let main = f.0.join("main");
        f.git(&["init", main.to_str().unwrap()]);
        f.git(&["-C", main.to_str().unwrap(), "commit", "--allow-empty", "-m", "initial"]);
        f.git(&["init", f.0.join("host").to_str().unwrap()]);
        let root = f.0.join("host/worktrees/topic");
        f.git(&["-C", main.to_str().unwrap(), "worktree", "add", "-b", "topic", root.to_str().unwrap()]);
        let nested = root.join("vendor/nested");
        f.git(&["init", nested.to_str().unwrap()]);
        std::fs::create_dir_all(nested.join("src")).unwrap();
        assert_eq!(repository_label(&nested.join("src")).unwrap(), "main/…/topic/vendor/nested/src");

        let source = f.0.join("library");
        f.git(&["init", source.to_str().unwrap()]);
        f.git(&["-C", source.to_str().unwrap(), "commit", "--allow-empty", "-m", "initial"]);
        f.git(&["-C", root.to_str().unwrap(), "-c", "protocol.file.allow=always", "submodule", "add", source.to_str().unwrap(), "vendor/library"]);
        let submodule = root.join("vendor/library/src");
        std::fs::create_dir_all(&submodule).unwrap();
        assert_eq!(repository_label(&submodule).unwrap(), "main/…/topic/vendor/library/src");

        // When both ancestors are linked worktrees, the nearer one wins.
        let inner = root.join("worktrees/library-topic");
        f.git(&["-C", source.to_str().unwrap(), "worktree", "add", "-b", "inner", inner.to_str().unwrap()]);
        assert_eq!(repository_label(&inner).unwrap(), "library/…/library-topic");
    }

    #[test]
    fn an_unlocatable_main_checkout_falls_back_to_the_first_enclosing_repo() {
        let f = Fixture::new();
        let main = f.0.join("project");
        let admin = f.0.join("admin");
        f.git(&["init", "--separate-git-dir", admin.to_str().unwrap(), main.to_str().unwrap()]);
        f.git(&["-C", main.to_str().unwrap(), "commit", "--allow-empty", "-m", "initial"]);
        let root = f.0.join("worktrees/topic");
        f.git(&["-C", main.to_str().unwrap(), "worktree", "add", "-b", "topic", root.to_str().unwrap()]);
        assert!(main_worktree(&root).is_none(), "Git only reports the separate admin directory");
        assert_eq!(repository_label(&root).unwrap(), "topic");
    }

    #[test]
    fn symlinked_paths_keep_the_original_full_path_and_cache_discovery() {
        let f = Fixture::new();
        let repo = f.0.join("project");
        f.git(&["init", repo.to_str().unwrap()]);
        let alias = f.0.join("alias");
        std::os::unix::fs::symlink(&repo, &alias).unwrap();
        let mut cache = PathCache::new(f.0.to_str().unwrap().into());
        let path = cache.get(alias.to_str().unwrap());
        assert_eq!(path.full, "~/alias");
        assert_eq!(path.display_path(80), "project");
        // Discovery is stable for this popup, even if metadata changes.
        std::fs::rename(repo.join(".git"), repo.join("saved-git")).unwrap();
        assert_eq!(cache.get(alias.to_str().unwrap()).display_path(80), "project");
        assert_eq!(cache.labels.len(), 1);
    }

    #[test]
    fn branches_and_detached_commits_are_displayed_and_searchable() {
        let f = Fixture::new();
        let repo = f.0.join("project");
        f.git(&["init", repo.to_str().unwrap()]);
        f.git(&["-C", repo.to_str().unwrap(), "commit", "--allow-empty", "-m", "initial"]);
        let root = repo.to_str().unwrap();
        let mut cache = PathCache::new(f.0.to_str().unwrap().into());
        assert_eq!(cache.get(root).display(80), "project (main)");
        f.git(&["-C", root, "checkout", "-b", "feature/searchable-branch"]);
        assert_eq!(cache.get(root).display(80), "project (feature/searchable-branch)", "HEAD is refreshed even when discovery is cached");
        assert_eq!(crate::notification::format("finished", "claude", root, "", crate::theme::AgentIcons::Unicode), "✳ finished\nproject (feature/searchable-branch)");
        let line = format!("$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|task||||{root}");
        let mut filter = crate::rows::Filter::new();
        for hidden in [false, true] {
            let snap = if hidden { crate::model::fixture_hidden(&[&line], &["%1"]) } else { crate::model::fixture(&[&line]) };
            filter.set("'feature/searchable-branch");
            let rows = crate::rows::agents(&snap, &mut filter, crate::rows::Group::Flat, None, &mut cache, 0, false);
            assert_eq!(rows.iter().filter(|r| matches!(r.kind, crate::rows::RowKind::Agent { .. })).count(), 1);
            filter.set("!searchable-branch");
            assert!(crate::rows::agents(&snap, &mut filter, crate::rows::Group::Flat, None, &mut cache, 0, false).is_empty());
        }
        f.git(&["-C", root, "checkout", "--detach"]);
        let path = cache.get(root);
        let head = path.head.as_ref().unwrap();
        assert!(head.detached);
        assert_eq!(path.display(80), format!("project (detached {})", &head.name[..7]));
        assert!(path.search_text().contains(&head.name));
        filter.set(&head.name[..12]);
        assert!(filter.score(&path.search_text()).is_some(), "more than the displayed hash is searchable");
    }

    #[test]
    fn branch_names_use_the_nearest_repos_head_even_when_directory_names_match() {
        let f = Fixture::new();
        let repo = f.0.join("project");
        let nested = repo.join("nested");
        f.git(&["init", repo.to_str().unwrap()]);
        f.git(&["init", nested.to_str().unwrap()]);
        f.git(&["-C", nested.to_str().unwrap(), "symbolic-ref", "HEAD", "refs/heads/child-branch"]);
        let mut cache = PathCache::default();
        assert_eq!(cache.get(nested.to_str().unwrap()).display(80), "project/nested (child-branch)");
        f.git(&["-C", nested.to_str().unwrap(), "symbolic-ref", "HEAD", "refs/heads/nested"]);
        let path = cache.get(nested.to_str().unwrap());
        assert_eq!(path.display(80), "project/nested (nested)");
        assert_eq!(crate::notification::format("asking", "pi", nested.to_str().unwrap(), "", crate::theme::AgentIcons::Unicode), "π waiting\nproject/nested (nested)");
        assert_eq!(path.head.as_ref().unwrap().name, "nested");
        assert!(path.search_text().ends_with(" nested"));
        std::fs::write(nested.join(".git/HEAD"), "not a valid HEAD\n").unwrap();
        assert_eq!(cache.get(nested.to_str().unwrap()).display(80), "project/nested");
    }

    #[test]
    fn branch_labels_fit_without_crowding_out_the_repository() {
        let path = AgentPath {
            full: String::new(), label: "project/…/topic/src".into(), anchored: true,
            head: Some(GitHead { name: "feature/日本語-long-branch".into(), detached: false }),
        };
        assert_eq!(path.display(100), "project/…/topic/src (feature/日本語-long-branch)");
        for width in 0..100 {
            let label = path.display(width);
            assert!(label.width() <= width, "{width}: {label}");
            if width >= 11 { assert!(label.starts_with("project"), "{width}: {label}"); }
            if width >= 5 { assert!(label.contains('('), "{width}: Git suffix stays visible: {label}"); }
            if label.contains('(') { assert!(label.ends_with(')'), "{width}: {label}"); }
        }
    }

    #[test]
    fn labels_keep_the_repo_and_tail_within_display_width() {
        let path = AgentPath { full: String::new(), label: "project/long/intermediate/worktrees/topic".into(), anchored: true, head: None };
        assert_eq!(path.display(30), "project/…/worktrees/topic");
        assert_eq!(path.display(18), "project/…/topic");
        let unicode = AgentPath { label: "日本語/long/worktrees/修正".into(), ..path };
        for width in 0..60 {
            let text = unicode.display(width);
            assert!(text.width() <= width, "{width}: {text}");
            if width >= "日本語".width() { assert!(text.starts_with("日本語"), "{width}: {text}"); }
        }
    }
}

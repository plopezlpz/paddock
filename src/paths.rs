//! Shared directory labels: discover repository anchors once per directory,
//! collapse the route to linked worktrees, then fit paths and Git heads to width.
//! Discovery reads files: Git runs only for layouts the files cannot resolve.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use unicode_width::UnicodeWidthStr;

use crate::rows::{fit, home_path, short_path};

#[derive(Clone, Debug)]
pub struct AgentPath {
    /// The actual directory, with only HOME replaced by ~.
    pub full: String,
    /// Repo-relative, or main-repo/…/worktree with any subdirectory suffix.
    label: String,
    /// The repository's folder (a worktree's: its main checkout's), if any.
    anchor: Option<String>,
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
        Self { full: home_path(path, home), label: short_path(path, home), anchor: None, head: None }
    }

    /// The repository it is in (a worktree's: its main one), by name.
    /// Its name and folder: one name can belong to several repositories.
    pub fn repo(&self) -> Option<(&str, &str)> {
        let anchor = self.anchor.as_deref()?;
        Some((self.label.split('/').next()?, anchor))
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
        // The branch gets what the path leaves, and a third of a narrow card
        // when the path needs it all, never the repo anchor's room; below
        // five columns there is no room for both path and suffix.
        if width < 5 { return self.display_path(width); }
        let anchor = self.label.split('/').next().unwrap_or("").width();
        let room = width.saturating_sub(self.label.width()).max(width / 3).max(4).min(width.saturating_sub(anchor).max(4));
        let branch = fit(&label, room - 3);
        let suffix = format!(" ({branch})");
        format!("{}{suffix}", self.display_path(width.saturating_sub(suffix.width())))
    }

    fn display_path(&self, width: usize) -> String {
        if self.anchor.is_none() || self.label.width() <= width {
            return fit(&self.label, width);
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
            format!("{base}{}", fit(parts.last().unwrap(), width - base.width()))
        } else {
            fit(repo, width)
        }
    }
}

#[derive(Default)]
pub struct PathCache {
    home: String,
    labels: HashMap<String, Option<Repository>>,
    /// Main checkouts by linked worktree root, shared by every path inside it.
    mains: HashMap<PathBuf, Option<PathBuf>>,
    /// HEAD by file, reread only when it is modified or replaced (Git renames
    /// a new one into place), so a row costs one stat and a branch switch
    /// still shows on the next rebuild. None: the stamp is not to be trusted.
    heads: HashMap<PathBuf, (Option<Stamp>, Option<GitHead>)>,
}

impl PathCache {
    pub fn new(home: String) -> Self {
        Self { home, ..Self::default() }
    }

    pub fn home(&self) -> &str { &self.home }

    pub fn get(&mut self, path: &str) -> AgentPath {
        let home = &self.home;
        let mains = &mut self.mains;
        let repo = self.labels.entry(path.to_string()).or_insert_with(|| repository_info(Path::new(path), mains));
        match repo {
            Some(repo) => AgentPath {
                full: home_path(path, home), label: repo.label.clone(), anchor: Some(repo.anchor.clone()),
                head: repo.head.as_deref().and_then(|file| cached_head(&mut self.heads, file)),
            },
            None => AgentPath::plain(path, home),
        }
    }
}

struct Repository {
    label: String,
    anchor: String,
    /// The nearest repository's HEAD file.
    head: Option<PathBuf>,
}

type Stamp = (SystemTime, u64);

/// How much older than its reading a file's mtime must be for the stamp to
/// say the file is unchanged: a coarse clock gives a write in the same tick
/// the same mtime (Git's racy-clean rule).
const RACY: Duration = Duration::from_secs(1);

fn cached_head(heads: &mut HashMap<PathBuf, (Option<Stamp>, Option<GitHead>)>, file: &Path) -> Option<GitHead> {
    let meta = std::fs::metadata(file).ok()?;
    let stamp = (meta.modified().ok()?, meta.ino());
    if let Some((_, head)) = heads.get(file).filter(|(seen, _)| *seen == Some(stamp)) {
        return head.clone();
    }
    let settled = SystemTime::now().duration_since(stamp.0).is_ok_and(|age| age >= RACY);
    let head = read_head(file);
    heads.insert(file.into(), (settled.then_some(stamp), head.clone()));
    head
}

fn git_dir(root: &Path) -> Option<PathBuf> {
    let marker = root.join(".git");
    if marker.is_dir() { return Some(marker); }
    let contents = std::fs::read_to_string(marker).ok()?;
    Some(root.join(contents.trim_end_matches(['\r', '\n']).strip_prefix("gitdir: ")?))
}

fn read_head(file: &Path) -> Option<GitHead> {
    let contents = std::fs::read_to_string(file).ok()?;
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
    repository_info(path, &mut HashMap::new()).map(|repo| repo.label)
}

fn repository_info(path: &Path, mains: &mut HashMap<PathBuf, Option<PathBuf>>) -> Option<Repository> {
    let path = path.canonicalize().ok()?;
    let repos: Vec<&Path> = path.ancestors().filter(|p| p.join(".git").exists()).collect();
    let outermost = *repos.last()?;
    // The branch belongs to the nearest repository, even when the path label
    // is anchored at an enclosing repository or worktree's main checkout.
    let head = git_dir(repos[0]).map(|dir| dir.join("HEAD"));
    // A linked worktree's gitfile leads to its private admin directory, whose
    // commondir is the marker distinguishing it from a submodule or a repo
    // with --separate-git-dir. See main_worktree for locating its checkout.
    // Search nearest-first, past ordinary nested repos and submodules: they
    // must not hide an enclosing linked worktree's main-repository identity.
    let linked = repos.iter().copied().find(|repo| {
        let gitfile = std::fs::read_to_string(repo.join(".git")).ok();
        gitfile.as_deref().and_then(|s| s.trim().strip_prefix("gitdir: "))
            .is_some_and(|dir| repo.join(dir).join("commondir").is_file())
    });
    let main = linked.and_then(|root| mains.entry(root.into()).or_insert_with(|| main_worktree(root)).clone());
    let anchor = main.as_deref().unwrap_or(outermost);
    let name = anchor.file_name()?.to_str()?;
    if main.is_some() {
        // Git identifies the worktree root regardless of its container names
        // or whether it lives inside the main checkout. Always collapse that
        // route, keeping the worktree's own name and any directory below it.
        let worktree = linked?;
        let tail = path.strip_prefix(worktree.parent()?).ok()?;
        return Some(Repository { label: format!("{name}/…/{}", tail.to_str()?), anchor: anchor.to_str()?.into(), head });
    }
    let relative = path.strip_prefix(anchor).ok()?;
    Some(Repository {
        label: if relative.as_os_str().is_empty() { name.into() } else { format!("{name}/{}", relative.to_str()?) },
        anchor: anchor.to_str()?.into(),
        head,
    })
}

/// The main checkout of the linked worktree at ROOT, or a bare main repo.
fn main_worktree(root: &Path) -> Option<PathBuf> {
    main_checkout(root).or_else(|| bare_main(root)).or_else(|| git_main_worktree(root))
}

/// A bare main repo, from files alone: the common dir, when its config says
/// it is bare. Git reports that directory too, but costs a process each.
fn bare_main(root: &Path) -> Option<PathBuf> {
    let admin = git_dir(root)?;
    let common = std::fs::read_to_string(admin.join("commondir")).ok()?;
    let common = admin.join(common.trim_end_matches(['\r', '\n'])).canonicalize().ok()?;
    let config = std::fs::read_to_string(common.join("config")).ok()?;
    let mut core = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            core = line.eq_ignore_ascii_case("[core]");
        } else if core && let Some((key, value)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case("bare")
        {
            return value.trim().eq_ignore_ascii_case("true").then_some(common);
        }
    }
    None
}

/// The usual layout, from files alone: the admin directory's commondir is the
/// main checkout's own .git directory (or a bare main repo named .git, whose
/// folder is taken for its checkout). Anything else (another bare main repo,
/// --separate-git-dir) is left to Git rather than assuming a checkout.
fn main_checkout(root: &Path) -> Option<PathBuf> {
    let admin = git_dir(root)?;
    let common = std::fs::read_to_string(admin.join("commondir")).ok()?;
    let common = admin.join(common.trim_end_matches(['\r', '\n'])).canonicalize().ok()?;
    let main = common.parent()?;
    (main.join(".git").canonicalize().ok()? == common).then(|| main.into())
}

fn git_main_worktree(root: &Path) -> Option<PathBuf> {
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
    fn sessions_and_agents_share_paths_branches_and_fresh_heads() {
        use crate::rows::{self, Cards, Filter, Group, Notes, RowKind, Tree};
        let f = Fixture::new();
        let main = f.0.join("project");
        let worktree = f.0.join("containers/topic");
        let plain = f.0.join("Downloads");
        f.git(&["init", main.to_str().unwrap()]);
        f.git(&["-C", main.to_str().unwrap(), "commit", "--allow-empty", "-m", "initial"]);
        f.git(&["-C", main.to_str().unwrap(), "worktree", "add", "-b", "topic", worktree.to_str().unwrap()]);
        std::fs::create_dir_all(worktree.join("src")).unwrap();
        std::fs::create_dir_all(&plain).unwrap();
        let mut cache = PathCache::new(f.0.to_str().unwrap().into());
        let none = std::collections::HashSet::new();
        let tree = Tree { expanded: &none, collapsed: &none, keep: None, current: "$1" };
        let cards = Cards { group: Group::Flat, keep: None, now: 0, hidden_expanded: false, notes: &Notes::new(), diffs: &HashMap::new() };
        let path_of = |rows: Vec<rows::Row>| rows.into_iter().find_map(|r| match r.kind {
            RowKind::Window { path, .. } | RowKind::Agent(rows::Card { path, .. }) => Some(path), _ => None,
        }).unwrap();
        for (dir, expected) in [
            (&main, "project (main)"), (&worktree, "project/…/topic (topic)"),
            (&worktree.join("src"), "project/…/topic/src (topic)"), (&plain, "~/Downloads"),
        ] {
            let snap = crate::backend::tmux::fixture(&[&format!("$1|s|@1|1|w|1|0|%1|0|1|pi||working|0|pi|task||||{}", dir.display())]);
            let session = path_of(rows::sessions(&snap, &mut Filter::new(), &tree, &mut cache));
            let agent = path_of(rows::agents(&snap, &mut Filter::new(), &cards, &mut cache));
            assert_eq!(session.display(200), expected);
            for width in 0..100 { assert_eq!(session.display(width), agent.display(width)); }
        }
        let snap = crate::backend::tmux::fixture(&[&format!("$1|s|@1|1|w|1|0|%1|0|1|pi||working|0|pi|task||||{}", worktree.display())]);
        f.git(&["-C", worktree.to_str().unwrap(), "checkout", "-b", "feature/searchable-branch"]);
        assert_eq!(path_of(rows::sessions(&snap, &mut Filter::new(), &tree, &mut cache)).display(200),
            "project/…/topic (feature/searchable-branch)", "cached discovery still rereads HEAD");
        for query in [worktree.to_str().unwrap(), "'~/containers/topic", "'project/…/topic", "'feature/searchable-branch"] {
            let mut filter = Filter::new(); filter.set(query);
            assert!(rows::sessions(&snap, &mut filter, &tree, &mut cache).iter().any(|r| matches!(r.kind, RowKind::Window { .. })), "{query}");
        }
        f.git(&["-C", worktree.to_str().unwrap(), "checkout", "--detach"]);
        let session = path_of(rows::sessions(&snap, &mut Filter::new(), &tree, &mut cache));
        let agent = path_of(rows::agents(&snap, &mut Filter::new(), &cards, &mut cache));
        assert_eq!(session.display(200), agent.display(200));
        assert!(session.display(200).contains("(detached "));
        let mut filter = Filter::new(); filter.set(&session.head.as_ref().unwrap().name);
        assert!(rows::sessions(&snap, &mut filter, &tree, &mut cache).iter().any(|r| matches!(r.kind, RowKind::Window { .. })));
    }

    #[test]
    fn sessions_search_inactive_pane_branches_but_display_the_active_directory() {
        use crate::rows::{self, Filter, RowKind, Tree};
        let f = Fixture::new();
        let main = f.0.join("project");
        let other = f.0.join("other");
        f.git(&["init", main.to_str().unwrap()]);
        f.git(&["init", other.to_str().unwrap()]);
        f.git(&["-C", other.to_str().unwrap(), "symbolic-ref", "HEAD", "refs/heads/feature/inactive-pane"]);
        let snap = crate::backend::tmux::fixture(&[
            &format!("$1|s|@1|1|w|1|0|%1|0|1|zsh|||||||||{}", main.display()),
            &format!("$1|s|@1|1|w|1|0|%2|1|0|zsh|||||||||{}", other.display()),
        ]);
        let none = std::collections::HashSet::new();
        let tree = Tree { expanded: &none, collapsed: &none, keep: None, current: "$1" };
        let mut cache = PathCache::new(f.0.to_str().unwrap().into());
        let mut filter = Filter::new(); filter.set("'feature/inactive-pane");
        let rows = rows::sessions(&snap, &mut filter, &tree, &mut cache);
        assert_eq!(rows.len(), 3, "session, window and matching inactive pane");
        assert!(matches!(&rows[1].kind, RowKind::Window { path, panes: 2, expanded: Some(true), .. } if path.display(80) == "project (main)"));
        assert!(matches!(&rows[2].target, Some(crate::model::Target::Pane { pane, .. }) if pane == "%2"));
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
        assert!(main_checkout(&root).is_none(), "the common dir's parent is not a checkout");
        assert!(main_worktree(&root).is_none(), "Git only reports the separate admin directory");
        assert_eq!(repository_label(&root).unwrap(), "topic");
    }

    #[test]
    fn main_checkouts_are_found_from_files_and_git_only_resolves_the_rest() {
        let f = Fixture::new();
        let main = f.0.join("project");
        f.git(&["init", main.to_str().unwrap()]);
        f.git(&["-C", main.to_str().unwrap(), "commit", "--allow-empty", "-m", "initial"]);
        let root = f.0.join("worktrees/topic");
        f.git(&["-C", main.to_str().unwrap(), "worktree", "add", "-b", "topic", root.to_str().unwrap()]);
        assert_eq!(main_checkout(&root), Some(main.clone()), "no Git process for the usual layout");
        assert!(main_checkout(&main).is_none(), "a main checkout is not a linked worktree");
        let mut cache = PathCache::new(f.0.to_str().unwrap().into());
        for dir in [root.clone(), root.join("src"), root.join("src/deep")] {
            std::fs::create_dir_all(&dir).unwrap();
            let label = format!("project/…/{}", dir.strip_prefix(f.0.join("worktrees")).unwrap().display());
            assert_eq!(cache.get(dir.to_str().unwrap()).display(200), format!("{label} (topic)"));
        }
        assert_eq!(cache.get(main.to_str().unwrap()).display(200), "project (main)");
        assert_eq!(cache.mains.len(), 1, "subdirectories share their worktree root's lookup");
        assert_eq!(cache.heads.len(), 2, "one HEAD per repository, not per path");

        // A bare main repo has no checkout: Git reports the repo itself.
        let bare = f.0.join("bare.git");
        f.git(&["clone", "--bare", main.to_str().unwrap(), bare.to_str().unwrap()]);
        let linked = f.0.join("bare-topic");
        f.git(&["-C", bare.to_str().unwrap(), "worktree", "add", "-b", "bare-topic", linked.to_str().unwrap()]);
        assert!(main_checkout(&linked).is_none());
        assert_eq!(bare_main(&linked), Some(bare.clone()), "no Git process for a bare main repo either");
        assert_eq!(main_worktree(&linked), Some(bare));
        assert_eq!(repository_label(&linked).unwrap(), "bare.git/…/bare-topic");
        // One named .git is taken for its folder's checkout.
        let dotgit = f.0.join("dotgit");
        f.git(&["clone", "--bare", main.to_str().unwrap(), dotgit.join(".git").to_str().unwrap()]);
        let linked = f.0.join("dotgit-topic");
        f.git(&["-C", dotgit.join(".git").to_str().unwrap(), "worktree", "add", "-b", "dotgit-topic", linked.to_str().unwrap()]);
        assert_eq!(main_checkout(&linked), Some(dotgit));
    }

    #[test]
    fn a_head_written_in_the_same_tick_it_was_read_is_read_again() {
        let f = Fixture::new();
        let file = f.0.join("HEAD");
        let mut heads = HashMap::new();
        let write = |branch: &str, mtime: SystemTime| {
            // In place, as a coarse clock leaves it: same inode, same mtime.
            let out = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&file).unwrap();
            std::io::Write::write_all(&mut &out, format!("ref: refs/heads/{branch}\n").as_bytes()).unwrap();
            out.set_modified(mtime).unwrap();
        };
        let branch = |heads: &mut HashMap<_, _>| cached_head(heads, &file).unwrap().name;
        let now = SystemTime::now();
        write("one", now);
        assert_eq!(branch(&mut heads), "one");
        write("two", now);
        assert_eq!(branch(&mut heads), "two", "a stamp this fresh is not trusted");
        let old = now - Duration::from_secs(10);
        write("three", old);
        assert_eq!(branch(&mut heads), "three");
        write("four", old);
        assert_eq!(branch(&mut heads), "three", "a settled stamp is: no read");
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
        let cards = crate::rows::Cards { group: crate::rows::Group::Flat, keep: None, now: 0, hidden_expanded: false, notes: &crate::rows::Notes::new(), diffs: &HashMap::new() };
        for hidden in [false, true] {
            let snap = if hidden { crate::backend::tmux::fixture_hidden(&[&line], &["%1"]) } else { crate::backend::tmux::fixture(&[&line]) };
            filter.set("'feature/searchable-branch");
            let rows = crate::rows::agents(&snap, &mut filter, &cards, &mut cache);
            assert_eq!(rows.iter().filter(|r| matches!(r.kind, crate::rows::RowKind::Agent(_))).count(), 1);
            filter.set("!searchable-branch");
            assert!(crate::rows::agents(&snap, &mut filter, &cards, &mut cache).is_empty());
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
            full: String::new(), label: "project/…/topic/src".into(), anchor: Some(String::new()),
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
    fn a_short_label_leaves_the_branch_its_room() {
        let path = AgentPath { full: String::new(), label: "project".into(), anchor: Some(String::new()), head: Some(GitHead { name: "feature/searchable-branch".into(), detached: false }) };
        assert_eq!(path.display(30), "project (feature/searchable-…)");
        assert_eq!(path.display(35), "project (feature/searchable-branch)");
        assert_eq!(path.display(12), "project (f…)");
        for width in 0..60 {
            let label = path.display(width);
            assert!(label.width() <= width, "{width}: {label}");
            if width >= 11 { assert!(label.starts_with("project ("), "{width}: {label}"); }
        }
    }

    #[test]
    fn labels_keep_the_repo_and_tail_within_display_width() {
        let path = AgentPath { full: String::new(), label: "project/long/intermediate/worktrees/topic".into(), anchor: Some(String::new()), head: None };
        assert_eq!(path.display(30), "project/…/worktrees/topic");
        assert_eq!(path.display(18), "project/…/topic");
        assert_eq!((path.repo(), AgentPath::plain("/tmp/x", "").repo()), (Some(("project", "")), None), "folder groups");
        let unicode = AgentPath { label: "日本語/long/worktrees/修正".into(), ..path };
        for width in 0..60 {
            let text = unicode.display(width);
            assert!(text.width() <= width, "{width}: {text}");
            if width >= "日本語".width() { assert!(text.starts_with("日本語"), "{width}: {text}"); }
        }
    }
}

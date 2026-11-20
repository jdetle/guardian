//! Finding and freeing disk space (`guardian disk`).
//!
//! Walks the home directory for git repositories and measures what is both large and cheap
//! to lose: build output and dependency folders inside projects, linked git worktrees,
//! package-manager downloads and tool caches. Each target is marked either as something that
//! comes back on its own or as something to look at first, with how long it has gone
//! untouched.
//!
//! Listing never deletes. Deleting (`guardian disk clean`, `guardian disk delete`) is a
//! separate, confirmed step with its own checks: a build folder is removed only if git tracks
//! nothing in it, and a worktree only through `git worktree remove` without `--force`, after
//! checking it holds no uncommitted files and no commits that exist nowhere else.
//!
//! The scan runs at throttled I/O priority inside a time budget: it is meant to be run on a
//! machine that is already short of disk and memory.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// How much a removal costs the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Safety {
    /// Rebuilt, re-downloaded or re-creatable from what git already holds.
    Regenerates,
    /// May hold something not reproducible, or needs a choice (which models, which images).
    Review,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A tool's download or build cache outside any project.
    Cache,
    /// Build output or installed dependencies inside a project.
    Build,
    /// A linked git worktree.
    Worktree,
}

#[derive(Clone, Debug, Serialize)]
pub struct Target {
    pub path: PathBuf,
    /// The path with the home directory shown as `~`.
    pub display_path: String,
    pub kind: Kind,
    pub what: String,
    /// What removing it costs, in a few words.
    pub cost: String,
    /// The command that removes it by hand.
    pub remove: String,
    pub safety: Safety,
    pub bytes: u64,
    /// False when the time budget ran out mid-measurement: `bytes` is then a lower bound.
    pub complete: bool,
    /// Whole days since the project (or worktree) was last touched. `None` for tool caches.
    pub age_days: Option<u64>,
    /// The repository or worktree it belongs to, shown as `~/…`.
    pub repo: Option<String>,
    /// A worktree's branch.
    pub branch: Option<String>,
    /// What was found when it was checked, e.g. "3 uncommitted or untracked files".
    pub note: Option<String>,
    /// True when `guardian disk delete` will remove it (after re-checking).
    pub deletable: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub volume: String,
    pub used_percent: f64,
    pub available_gb: f64,
    /// Largest first; only targets at or above the size floor.
    pub targets: Vec<Target>,
    /// Candidates found but not measured before the time budget ran out.
    pub unmeasured: usize,
    /// Git repositories found (main checkouts, not counting their worktrees).
    pub repos_scanned: usize,
    pub regenerates_bytes: u64,
    pub review_bytes: u64,
}

pub struct Options {
    /// Extra directories to search (the home directory is always searched).
    pub roots: Vec<PathBuf>,
    pub min_bytes: u64,
    pub top: usize,
    pub budget: Duration,
    /// Leave out project build folders and worktrees touched more recently than this.
    pub older_than_days: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            min_bytes: 100 * 1024 * 1024,
            top: 15,
            budget: Duration::from_secs(45),
            older_than_days: 0,
        }
    }
}

/// Untouched for this long counts as "old" when choosing what to delete in bulk.
pub const OLD_DAYS: u64 = 14;

/// A place to look, before it has been measured.
struct Candidate {
    path: PathBuf,
    kind: Kind,
    what: String,
    cost: String,
    /// `None`: remove with `rm -rf <path>`.
    remove: Option<String>,
    safety: Safety,
    repo: Option<PathBuf>,
    last_touch: Option<SystemTime>,
    branch: Option<String>,
    note: Option<String>,
    deletable: bool,
}

/// Well-known locations under the home directory:
/// (relative path, what, cost, removal command or "" for `rm -rf`, safety).
const KNOWN: &[(&str, &str, &str, &str, Safety)] = &[
    ("Library/Developer/Xcode/DerivedData", "Xcode build output", "rebuilt the next time you build in Xcode", "", Safety::Regenerates),
    ("Library/Developer/Xcode/iOS DeviceSupport", "Xcode device support files", "re-created when a device is next connected", "", Safety::Regenerates),
    ("Library/Developer/CoreSimulator/Caches", "iOS Simulator caches", "rebuilt when the Simulator next runs", "", Safety::Regenerates),
    ("Library/Developer/CoreSimulator/Devices", "iOS Simulator devices and their app data", "this command removes only simulators whose runtime is gone", "xcrun simctl delete unavailable", Safety::Review),
    ("Library/Developer/Xcode/Archives", "Xcode archives of past builds", "needed to re-symbolicate crashes from those builds", "", Safety::Review),
    ("Library/Caches/Homebrew", "Homebrew downloads", "re-downloaded when a formula is next installed", "brew cleanup -s", Safety::Regenerates),
    (".cargo/registry/cache", "Cargo crate downloads", "re-downloaded by the next cargo build (needs network)", "", Safety::Regenerates),
    (".cargo/registry/src", "Cargo unpacked crate sources", "unpacked again by the next cargo build", "", Safety::Regenerates),
    (".cargo/git/checkouts", "Cargo git dependency checkouts", "checked out again by the next cargo build", "", Safety::Regenerates),
    (".npm/_cacache", "npm download cache", "re-downloaded by the next npm install", "npm cache clean --force", Safety::Regenerates),
    ("Library/Caches/Yarn", "Yarn download cache", "re-downloaded by the next yarn install", "yarn cache clean", Safety::Regenerates),
    ("Library/pnpm/store", "pnpm package store", "this command removes only packages no project uses", "pnpm store prune", Safety::Regenerates),
    (".bun/install/cache", "Bun download cache", "re-downloaded by the next bun install", "bun pm cache rm", Safety::Regenerates),
    ("Library/Caches/pip", "pip download cache", "re-downloaded by the next pip install", "pip cache purge", Safety::Regenerates),
    (".cache/uv", "uv package cache", "re-downloaded by the next uv sync", "uv cache clean", Safety::Regenerates),
    ("Library/Caches/go-build", "Go build cache", "rebuilt by the next go build", "go clean -cache", Safety::Regenerates),
    ("go/pkg/mod", "Go module downloads", "re-downloaded by the next go build", "go clean -modcache", Safety::Regenerates),
    (".gradle/caches", "Gradle caches", "re-downloaded by the next Gradle build", "", Safety::Regenerates),
    (".m2/repository", "Maven local repository", "re-downloaded by the next Maven build", "", Safety::Regenerates),
    ("Library/Caches/ms-playwright", "Playwright browsers", "re-downloaded by npx playwright install", "", Safety::Regenerates),
    ("Library/Caches/Cypress", "Cypress binaries", "re-downloaded by the next Cypress install", "", Safety::Regenerates),
    (".cache/puppeteer", "Puppeteer browsers", "re-downloaded by the next Puppeteer install", "", Safety::Regenerates),
    (".rustup/toolchains", "Rust toolchains", "keep the ones you use: rustup toolchain list", "rustup toolchain uninstall <name>", Safety::Review),
    ("Library/Containers/com.docker.docker/Data/vms", "Docker Desktop disk (images, containers, volumes)", "removes unused images and build cache; volumes are kept", "docker system prune -a", Safety::Review),
    (".ollama/models", "Ollama models", "pick models to drop: ollama list", "ollama rm <model>", Safety::Review),
    (".cache/huggingface", "Hugging Face models and datasets", "pick what to drop interactively", "huggingface-cli delete-cache", Safety::Review),
    (".Trash", "Trash", "gone for good once emptied", "osascript -e 'tell application \"Finder\" to empty trash'", Safety::Review),
];

/// Top-level home folders that are never walked: app data, media, and the folders macOS
/// guards with a permission prompt (pass one with `--root` to include it deliberately).
const HOME_SKIP: &[&str] = &[
    "Library", "Applications", "Movies", "Music", "Pictures", "Public", "Desktop", "Documents",
    "Downloads",
];

/// How deep below the home directory a repository may sit.
const MAX_DEPTH: usize = 7;

/// Is `name` (inside `parent`) a build or dependency directory that can be recreated?
fn classify_project_dir(parent: &Path, name: &str) -> Option<(&'static str, &'static str, Safety)> {
    let dir = parent.join(name);
    let sibling = |f: &str| parent.join(f).exists();
    match name {
        "target" if sibling("Cargo.toml") || dir.join("CACHEDIR.TAG").exists() => Some((
            "Rust build output",
            "rebuilt by the next cargo build",
            Safety::Regenerates,
        )),
        "node_modules" => Some((
            "Node dependencies",
            "reinstalled by npm / pnpm / yarn install",
            Safety::Regenerates,
        )),
        ".next" | ".nuxt" | ".turbo" | ".parcel-cache" | ".svelte-kit" if sibling("package.json") => {
            Some(("JavaScript build cache", "rebuilt by the next build", Safety::Regenerates))
        }
        ".build" if sibling("Package.swift") => Some((
            "Swift build output",
            "rebuilt by the next swift build",
            Safety::Regenerates,
        )),
        "DerivedData" => Some((
            "Xcode build output",
            "rebuilt the next time you build in Xcode",
            Safety::Regenerates,
        )),
        "Pods" if sibling("Podfile") => Some((
            "CocoaPods dependencies",
            "reinstalled by pod install",
            Safety::Regenerates,
        )),
        ".venv" | "venv" if dir.join("pyvenv.cfg").exists() => Some((
            "Python virtual environment",
            "recreate it and reinstall its packages",
            Safety::Review,
        )),
        _ => None,
    }
}

#[derive(Default)]
struct Found {
    candidates: Vec<Candidate>,
    /// Main checkouts (`.git` is a directory); their linked worktrees are asked of git.
    repos: Vec<PathBuf>,
}

/// Walk `root` for git repositories and for build / dependency folders, without entering
/// the latter. `skip_top` names children of `root` that are not walked.
fn walk(root: &Path, skip_top: &[&str], deadline: Instant, out: &mut Found) {
    let mut stack: Vec<(PathBuf, usize, Option<PathBuf>)> = vec![(root.to_path_buf(), 0, None)];
    while let Some((dir, depth, inherited)) = stack.pop() {
        if Instant::now() >= deadline {
            return;
        }
        let repo = match fs::symlink_metadata(dir.join(".git")) {
            Ok(meta) => {
                if meta.is_dir() {
                    out.repos.push(dir.clone());
                }
                Some(dir.clone())
            }
            Err(_) => inherited,
        };
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            // Symlinks are never followed: the target may be anywhere.
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if let Some((what, cost, safety)) = classify_project_dir(&dir, name) {
                let path = entry.path();
                let last_touch = newest(&[
                    newest_shallow(&path),
                    repo.as_deref().and_then(repo_activity),
                ]);
                out.candidates.push(Candidate {
                    path,
                    kind: Kind::Build,
                    what: what.to_string(),
                    cost: cost.to_string(),
                    remove: None,
                    safety,
                    repo: repo.clone(),
                    last_touch,
                    branch: None,
                    note: None,
                    deletable: true,
                });
                continue;
            }
            // Hidden directories (.git, editor state) hold no projects worth the walk.
            if name.starts_with('.') || depth + 1 >= MAX_DEPTH {
                continue;
            }
            if depth == 0 && skip_top.contains(&name) {
                continue;
            }
            stack.push((entry.path(), depth + 1, repo.clone()));
        }
    }
}

fn newest(times: &[Option<SystemTime>]) -> Option<SystemTime> {
    times.iter().flatten().max().copied()
}

fn mtime(path: &Path) -> Option<SystemTime> {
    fs::symlink_metadata(path).ok()?.modified().ok()
}

/// Newest modification time of `dir` and its immediate children.
fn newest_shallow(dir: &Path) -> Option<SystemTime> {
    let mut best = mtime(dir);
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten().take(256) {
            best = newest(&[best, entry.metadata().ok().and_then(|m| m.modified().ok())]);
        }
    }
    best
}

/// The git directory of a checkout: `.git` itself, or where a worktree's `.git` file points.
fn git_dir_of(checkout: &Path) -> Option<PathBuf> {
    let dot_git = checkout.join(".git");
    let meta = fs::symlink_metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some(dot_git);
    }
    let text = fs::read_to_string(&dot_git).ok()?;
    let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
    let target = Path::new(target);
    Some(if target.is_absolute() {
        target.to_path_buf()
    } else {
        checkout.join(target)
    })
}

/// When a checkout was last worked in: the newest of git's own bookkeeping files, which
/// change on commit, checkout, add and (usually) status.
fn repo_activity(checkout: &Path) -> Option<SystemTime> {
    let git_dir = git_dir_of(checkout)?;
    newest(&[
        mtime(&git_dir.join("index")),
        mtime(&git_dir.join("HEAD")),
        mtime(&git_dir.join("logs/HEAD")),
    ])
}

/// A `git -C dir` command that answers about `dir` and nothing else: variables that would
/// point git at some other repository are dropped.
fn git_command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir);
    for var in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR"] {
        cmd.env_remove(var);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null());
    cmd
}

/// Longest any one question to git may take. A git that hangs (a network volume, a stale
/// lock) then counts as "git could not answer", which every caller takes as a reason not
/// to delete.
const GIT_TIMEOUT: Duration = Duration::from_secs(20);

/// Run git in `dir`; `Some(stdout)` on success. Never takes optional locks, so asking for
/// status does not rewrite the index (which would make the checkout look freshly used).
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    use std::io::Read;
    let mut child = git_command(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Read on another thread, so a long answer never stalls git on a full pipe.
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + GIT_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let out = reader.join().ok()?;
    status
        .success()
        .then(|| String::from_utf8_lossy(&out).to_string())
}

/// One linked worktree as git lists it.
struct Linked {
    path: PathBuf,
    branch: Option<String>,
    locked: bool,
}

/// Linked worktrees of the main checkout at `main` (not the main checkout itself).
fn linked_worktrees(main: &Path) -> Vec<Linked> {
    let Some(text) = git(main, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };
    let mut all = Vec::new();
    for block in text.split("\n\n") {
        let mut path = None;
        let mut branch = None;
        let mut locked = false;
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if let Some(b) = line.strip_prefix("branch ") {
                branch = Some(b.trim_start_matches("refs/heads/").to_string());
            } else if line == "locked" || line.starts_with("locked ") {
                locked = true;
            }
        }
        if let Some(path) = path {
            all.push(Linked { path, branch, locked });
        }
    }
    // The first entry is always the main checkout.
    all.into_iter().skip(1).collect()
}

/// Is this git-ignored path (relative to `worktree`) something that comes back on its own:
/// build output, installed dependencies or a tool's scratch files?
fn ignored_path_regenerates(worktree: &Path, rel: &str) -> bool {
    const SCRATCH: &[&str] = &[
        ".DS_Store",
        "__pycache__",
        ".pytest_cache",
        ".mypy_cache",
        ".ruff_cache",
        ".eslintcache",
        ".swiftpm",
    ];
    let path = worktree.join(rel);
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if SCRATCH.contains(&name) || name.ends_with(".pyc") || name.ends_with(".tsbuildinfo") {
        return true;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    matches!(
        classify_project_dir(parent, name),
        Some((_, _, Safety::Regenerates))
    )
}

/// What deleting a worktree would lose, beyond what git keeps.
struct WorktreeState {
    safety: Safety,
    deletable: bool,
    note: String,
}

fn worktree_state(wt: &Linked) -> WorktreeState {
    let blocked = |note: String| WorktreeState {
        safety: Safety::Review,
        deletable: false,
        note,
    };
    if wt.locked {
        return blocked("locked with git worktree lock".to_string());
    }
    // Spelled out rather than left to the user's git config: with
    // `status.showUntrackedFiles=no`, a bare `git status` (and the check inside
    // `git worktree remove`) reports a worktree full of new files as clean.
    let Some(status) = git(
        &wt.path,
        &[
            "status",
            "--porcelain",
            "--untracked-files=normal",
            "--ignored=traditional",
            "--ignore-submodules=none",
        ],
    ) else {
        return blocked("could not be checked for unsaved work".to_string());
    };
    let entries = || status.lines().filter(|l| !l.trim().is_empty());
    let dirty = entries().filter(|l| !l.starts_with("!!")).count();
    if dirty > 0 {
        return blocked(format!(
            "{dirty} uncommitted or untracked file{}",
            if dirty == 1 { "" } else { "s" }
        ));
    }
    // Files flagged skip-worktree or assume-unchanged are left out of `git status`, edits
    // and all.
    let Some(flags) = git(&wt.path, &["ls-files", "-v"]) else {
        return blocked("could not be checked for unsaved work".to_string());
    };
    if flags
        .lines()
        .any(|l| l.starts_with(|c: char| c == 'S' || c.is_ascii_lowercase()))
    {
        return blocked(
            "holds files hidden from git status (skip-worktree or assume-unchanged)".to_string(),
        );
    }
    if wt.branch.is_none() {
        // Detached HEAD: commits made here may be on no branch at all.
        let unique = git(
            &wt.path,
            &["rev-list", "--count", "HEAD", "--not", "--branches", "--remotes", "--tags"],
        )
        .and_then(|s| s.trim().parse::<u64>().ok());
        match unique {
            Some(0) => {}
            Some(n) => {
                return blocked(format!(
                    "{n} commit{} only here (detached HEAD)",
                    if n == 1 { " exists" } else { "s exist" }
                ))
            }
            None => return blocked("could not be checked for unsaved commits".to_string()),
        }
    }
    // Ignored files are deleted with the worktree and git keeps no copy: local settings
    // (.env), databases, notes. Build output and dependencies are the exception.
    let kept_nowhere: Vec<String> = entries()
        .filter_map(|l| l.strip_prefix("!! "))
        .map(|p| p.trim_matches('"').trim_end_matches('/').to_string())
        .filter(|p| !ignored_path_regenerates(&wt.path, p))
        .collect();
    if let Some(first) = kept_nowhere.first() {
        let n = kept_nowhere.len();
        return WorktreeState {
            safety: Safety::Review,
            deletable: true,
            note: match n {
                1 => format!("no uncommitted work, but {first} is ignored by git and would be lost"),
                2 => format!("no uncommitted work, but {first} and 1 other ignored item would be lost"),
                _ => format!(
                    "no uncommitted work, but {first} and {} other ignored items would be lost",
                    n - 1
                ),
            },
        };
    }
    WorktreeState {
        safety: Safety::Regenerates,
        deletable: true,
        note: match &wt.branch {
            Some(b) => format!("no uncommitted work; branch {b} stays in the repository"),
            None => "no uncommitted work; its commits are on other branches".to_string(),
        },
    }
}

fn worktree_candidates(main: &Path, home: &Path, deadline: Instant, out: &mut Vec<Candidate>) {
    for wt in linked_worktrees(main) {
        if Instant::now() >= deadline {
            return;
        }
        if !wt.path.is_dir() {
            continue;
        }
        let state = worktree_state(&wt);
        // A worktree outside the home folder is listed, with its command, but never deleted
        // by Guardian (see `guard_path`).
        let deletable = state.deletable && guard_path(&wt.path, home).is_ok();
        out.push(Candidate {
            remove: Some(format!(
                "git -C {} worktree remove {}",
                shell_path(main, home),
                shell_path(&wt.path, home)
            )),
            last_touch: newest(&[repo_activity(&wt.path), mtime(&wt.path)]),
            path: wt.path,
            kind: Kind::Worktree,
            what: "Git worktree".to_string(),
            cost: "re-create it with git worktree add".to_string(),
            safety: state.safety,
            repo: Some(main.to_path_buf()),
            branch: wt.branch,
            note: Some(state.note),
            deletable,
        });
    }
}

/// Disk space used under `root` (allocated blocks, so sparse files such as Docker's disk
/// image count for what they really occupy). Stays on one volume, never follows symlinks.
/// Directories in `known` are not walked again: their recorded size is added instead.
/// Returns `(bytes, complete)`; `complete` is false when `deadline` passed first.
fn dir_size(root: &Path, deadline: Instant, known: &HashMap<PathBuf, (u64, bool)>) -> (u64, bool) {
    let Ok(root_meta) = fs::symlink_metadata(root) else {
        return (0, true);
    };
    let device = root_meta.dev();
    let mut total = root_meta.blocks().saturating_mul(512);
    if !root_meta.file_type().is_dir() {
        return (total, true);
    }
    let mut complete = true;
    let mut stack = vec![root.to_path_buf()];
    let mut seen: u32 = 0;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            seen = seen.wrapping_add(1);
            if seen % 512 == 0 && Instant::now() >= deadline {
                return (total, false);
            }
            // DirEntry::metadata does not follow symlinks.
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.dev() != device {
                continue;
            }
            if meta.file_type().is_dir() {
                let path = entry.path();
                if let Some((bytes, whole)) = known.get(&path) {
                    total = total.saturating_add(*bytes);
                    complete &= *whole;
                    continue;
                }
                total = total.saturating_add(meta.blocks().saturating_mul(512));
                stack.push(path);
            } else {
                total = total.saturating_add(meta.blocks().saturating_mul(512));
            }
        }
    }
    (total, complete)
}

/// `path` as the user would type it: `~/…` under the home directory.
pub fn display_path(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rel) if rel.as_os_str().is_empty() => "~".to_string(),
        Ok(rel) => format!("~/{}", rel.to_string_lossy()),
        Err(_) => path.to_string_lossy().to_string(),
    }
}

/// `path` quoted for pasting into a shell.
pub fn shell_path(path: &Path, home: &Path) -> String {
    let plain = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "/._-+@".contains(c))
    };
    let escape = |s: &str| {
        let mut out = String::new();
        for c in s.chars() {
            if matches!(c, '"' | '\\' | '$' | '`') {
                out.push('\\');
            }
            out.push(c);
        }
        out
    };
    match path.strip_prefix(home) {
        Ok(rel) => {
            let rel = rel.to_string_lossy();
            if plain(&rel) {
                format!("~/{rel}")
            } else {
                format!("\"$HOME/{}\"", escape(&rel))
            }
        }
        Err(_) => {
            let full = path.to_string_lossy();
            if plain(&full) {
                full.to_string()
            } else {
                format!("\"{}\"", escape(&full))
            }
        }
    }
}

pub fn human_size(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.0} MB", b / MB)
    }
}

/// Ask the kernel to treat this process's disk I/O as background work.
fn throttle_io() {
    extern "C" {
        fn setiopolicy_np(iotype: libc::c_int, scope: libc::c_int, policy: libc::c_int) -> libc::c_int;
    }
    const IOPOL_TYPE_DISK: libc::c_int = 0;
    const IOPOL_SCOPE_PROCESS: libc::c_int = 0;
    // "Utility": yields to interactive work without the heavy delays of full throttling,
    // which made a scan of a few dozen repositories take minutes.
    const IOPOL_UTILITY: libc::c_int = 4;
    // Best effort: a failure only means the scan runs at normal priority.
    unsafe {
        setiopolicy_np(IOPOL_TYPE_DISK, IOPOL_SCOPE_PROCESS, IOPOL_UTILITY);
    }
}

fn candidates(home: &Path, opts: &Options, deadline: Instant) -> (Vec<Candidate>, usize) {
    let mut found = Found::default();
    for (rel, what, cost, remove, safety) in KNOWN {
        let path = home.join(rel);
        if !path.exists() {
            continue;
        }
        found.candidates.push(Candidate {
            path,
            kind: Kind::Cache,
            what: what.to_string(),
            cost: cost.to_string(),
            // Only caches removed by plain deletion are ones Guardian deletes itself; the
            // rest have a tool that knows how to do it properly.
            deletable: remove.is_empty() && *safety == Safety::Regenerates,
            remove: (!remove.is_empty()).then(|| remove.to_string()),
            safety: *safety,
            repo: None,
            last_touch: None,
            branch: None,
            note: None,
        });
    }

    walk(home, HOME_SKIP, deadline, &mut found);
    for root in &opts.roots {
        walk(root, &[], deadline, &mut found);
    }

    let mut repos = std::mem::take(&mut found.repos);
    repos.sort();
    repos.dedup();
    for main in &repos {
        worktree_candidates(main, home, deadline, &mut found.candidates);
    }

    // The same directory can be reached twice (an extra root inside the home directory).
    let mut seen = HashSet::new();
    found.candidates.retain(|c| seen.insert(c.path.clone()));
    (found.candidates, repos.len())
}

fn age_days(last_touch: Option<SystemTime>) -> Option<u64> {
    let elapsed = SystemTime::now().duration_since(last_touch?).unwrap_or_default();
    Some(elapsed.as_secs() / 86_400)
}

/// Find and measure targets under `home`. Read-only.
pub fn survey_home(home: &Path, opts: &Options) -> Report {
    let started = Instant::now();
    let deadline = started + opts.budget;
    // Finding things may use at most a third of the budget; measuring gets the rest.
    let discovery_deadline = started + opts.budget / 3;

    let (mut found, repos_scanned) = candidates(home, opts, discovery_deadline);
    let worktree_touch: HashMap<PathBuf, Option<SystemTime>> = found
        .iter()
        .filter(|c| c.kind == Kind::Worktree)
        .map(|c| (c.path.clone(), c.last_touch))
        .collect();
    let worktree_paths: Vec<PathBuf> = worktree_touch.keys().cloned().collect();

    // Measuring order, for when the budget runs out before everything is measured:
    // tool caches first (few and large), then projects from the longest untouched to the
    // most recent, since stale ones are what will actually be deleted. A worktree comes
    // directly after the build folders inside it, whose sizes it reuses.
    let order = |c: &Candidate| {
        let group = match (&c.kind, &c.repo) {
            (Kind::Worktree, _) => Some(c.path.clone()),
            (Kind::Build, Some(repo)) if worktree_touch.contains_key(repo) => Some(repo.clone()),
            _ => None,
        };
        let touched = match &group {
            Some(g) => worktree_touch.get(g).copied().flatten(),
            None => c.last_touch,
        };
        (
            c.kind != Kind::Cache,
            touched.unwrap_or_else(SystemTime::now),
            group.unwrap_or_else(|| c.path.clone()),
            c.kind == Kind::Worktree,
        )
    };
    found.sort_by_cached_key(order);
    // No single folder may use more than this much of the budget; it then reports a lower
    // bound instead of starving everything after it.
    let per_target = (opts.budget / 8).max(Duration::from_secs(2));

    let mut sizes: HashMap<PathBuf, (u64, bool)> = HashMap::new();
    let mut targets = Vec::new();
    let mut unmeasured = 0usize;
    for c in found {
        if Instant::now() >= deadline {
            unmeasured += 1;
            continue;
        }
        let (bytes, complete) = dir_size(&c.path, deadline.min(Instant::now() + per_target), &sizes);
        sizes.insert(c.path.clone(), (bytes, complete));
        let remove = c
            .remove
            .unwrap_or_else(|| format!("rm -rf {}", shell_path(&c.path, home)));
        targets.push(Target {
            display_path: display_path(&c.path, home),
            kind: c.kind,
            what: c.what,
            cost: c.cost,
            remove,
            safety: c.safety,
            bytes,
            complete,
            age_days: age_days(c.last_touch),
            repo: c.repo.as_deref().map(|r| display_path(r, home)),
            branch: c.branch,
            note: c.note,
            deletable: c.deletable,
            path: c.path,
        });
    }

    targets.retain(|t| t.bytes >= opts.min_bytes);
    if opts.older_than_days > 0 {
        targets.retain(|t| {
            t.kind == Kind::Cache || t.age_days.is_some_and(|d| d >= opts.older_than_days)
        });
    }
    targets.sort_by(|a, b| b.bytes.cmp(&a.bytes));

    // A build folder inside a listed worktree is already part of that worktree's size.
    let listed_worktrees: HashSet<&PathBuf> = targets
        .iter()
        .filter(|t| t.kind == Kind::Worktree)
        .map(|t| &t.path)
        .collect();
    let counted_once = |t: &&Target| {
        t.kind == Kind::Worktree
            || !worktree_paths
                .iter()
                .any(|w| listed_worktrees.contains(w) && t.path.starts_with(w))
    };
    let sum = |s: Safety| -> u64 {
        targets
            .iter()
            .filter(counted_once)
            .filter(|t| t.safety == s)
            .map(|t| t.bytes)
            .sum()
    };
    let (regenerates_bytes, review_bytes) = (sum(Safety::Regenerates), sum(Safety::Review));
    targets.truncate(opts.top.max(1));

    let (volume, used_percent, available_gb) =
        crate::sampler::disk::sample_volume_containing(home)
            .map(|(v, _total, avail, used)| (v, used, avail))
            .unwrap_or_else(|| (home.to_string_lossy().to_string(), 0.0, 0.0));

    Report {
        volume,
        used_percent,
        available_gb,
        targets,
        unmeasured,
        repos_scanned,
        regenerates_bytes,
        review_bytes,
    }
}

/// `(used percent, GB available)` on the volume holding `home`.
pub fn volume_usage(home: &Path) -> Option<(f64, f64)> {
    crate::sampler::disk::sample_volume_containing(home).map(|(_, _, avail, used)| (used, avail))
}

/// Survey the current user's home directory at throttled I/O priority.
pub fn survey(opts: &Options) -> Option<Report> {
    let home = dirs::home_dir()?;
    throttle_io();
    Some(survey_home(&home, opts))
}

impl Target {
    pub fn size_text(&self) -> String {
        if self.complete {
            human_size(self.bytes)
        } else {
            format!("{}+", human_size(self.bytes))
        }
    }

    /// "untouched 34 days", or nothing for tool caches.
    pub fn age_text(&self) -> Option<String> {
        let days = self.age_days?;
        Some(match days {
            0 => "used today".to_string(),
            1 => "untouched 1 day".to_string(),
            n => format!("untouched {n} days"),
        })
    }

    /// Something that comes back on its own and has not been touched for [`OLD_DAYS`].
    pub fn is_old_and_safe(&self) -> bool {
        self.deletable
            && self.safety == Safety::Regenerates
            && self.kind != Kind::Cache
            && self.age_days.is_some_and(|d| d >= OLD_DAYS)
    }

    /// One listing entry: what it is, where, how stale, what was found.
    fn describe(&self) -> String {
        let mut line = format!("{} ({})", self.what, self.display_path);
        let mut details: Vec<String> = Vec::new();
        if let Some(age) = self.age_text() {
            details.push(age);
        }
        if let Some(note) = &self.note {
            details.push(note.clone());
        }
        if details.is_empty() {
            details.push(self.cost.clone());
        }
        line.push_str(&format!("\n             {}", details.join("; ")));
        line
    }
}

impl Report {
    pub fn render(&self) -> String {
        let mut out = format!(
            "Disk: {:.1}% used, {:.1} GB free on {}\nScanned {} git repositor{} and the usual tool caches.\n\n",
            self.used_percent,
            self.available_gb,
            self.volume,
            self.repos_scanned,
            if self.repos_scanned == 1 { "y" } else { "ies" }
        );
        if self.targets.is_empty() {
            out.push_str("Nothing large enough to list was found.\n");
        } else {
            out.push_str("Largest first. This listing deletes nothing; `guardian disk clean` offers to.\n\n");
        }
        for (label, safety) in [
            ("Comes back on its own", Safety::Regenerates),
            ("Look before deleting", Safety::Review),
        ] {
            let group: Vec<&Target> = self.targets.iter().filter(|t| t.safety == safety).collect();
            if group.is_empty() {
                continue;
            }
            out.push_str(&format!("{label}\n"));
            for t in group {
                out.push_str(&format!(
                    "  {:>9}  {}\n             $ {}\n",
                    t.size_text(),
                    t.describe(),
                    t.remove
                ));
            }
            out.push('\n');
        }
        if self.regenerates_bytes + self.review_bytes > 0 {
            out.push_str(&format!(
                "Found: {} that comes back on its own, {} to look at first.\n",
                human_size(self.regenerates_bytes),
                human_size(self.review_bytes)
            ));
        }
        if self.targets.iter().any(|t| !t.complete) {
            out.push_str("Sizes ending in + ran out of time and are lower bounds.\n");
        }
        if self.unmeasured > 0 {
            out.push_str(&format!(
                "{} more recently used places were not measured in time (oldest are measured first); rerun with --seconds 120 for all of them.\n",
                self.unmeasured
            ));
        }
        out
    }
}

// ---------------------------------------------------------------------------------------
// Deleting
// ---------------------------------------------------------------------------------------

/// How a target would be removed, decided afresh from what is on disk right now.
#[derive(Debug)]
enum Plan {
    /// Delete the directory at this path (the checked path with symlinks resolved).
    Directory { real: PathBuf },
    /// `git worktree remove` (never `--force`), run in the main checkout.
    Worktree { main: PathBuf },
}

#[derive(Clone, Debug, Serialize)]
pub struct Deleted {
    pub path: PathBuf,
    pub freed_bytes: u64,
}

/// Refuse anything that is not a real directory at least two levels below `home`.
fn guard_path(path: &Path, home: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("not an absolute path".to_string());
    }
    let meta = fs::symlink_metadata(path).map_err(|_| "it no longer exists".to_string())?;
    if !meta.file_type().is_dir() {
        return Err("it is not a directory (symlinks are never deleted through)".to_string());
    }
    let real = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let real_home = fs::canonicalize(home).map_err(|e| e.to_string())?;
    let below = real
        .strip_prefix(&real_home)
        .map_err(|_| "it is outside your home directory".to_string())?;
    if below.components().count() < 2 {
        return Err("it is too close to the top of your home directory".to_string());
    }
    Ok(real)
}

/// Decide whether `path` may be deleted, re-checking everything; nothing is trusted from an
/// earlier scan.
fn plan_delete(path: &Path, home: &Path) -> Result<Plan, String> {
    let real = guard_path(path, home)?;

    // A linked worktree: `.git` is a file.
    if fs::symlink_metadata(path.join(".git")).is_ok_and(|m| m.is_file()) {
        let common = git(path, &["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .ok_or("git could not identify the repository this worktree belongs to")?;
        let main = Path::new(common.trim())
            .parent()
            .ok_or("git reported no main checkout")?
            .to_path_buf();
        let wanted = fs::canonicalize(path).map_err(|e| e.to_string())?;
        let linked = linked_worktrees(&main)
            .into_iter()
            .find(|w| fs::canonicalize(&w.path).is_ok_and(|p| p == wanted))
            .ok_or("git does not list it as a linked worktree (a submodule is not deleted)")?;
        let state = worktree_state(&linked);
        if !state.deletable {
            return Err(state.note);
        }
        return Ok(Plan::Worktree { main });
    }

    // A known tool cache that is removed by plain deletion.
    let is_plain_cache = KNOWN.iter().any(|(rel, _, _, remove, safety)| {
        remove.is_empty() && *safety == Safety::Regenerates && home.join(rel) == path
    });
    if is_plain_cache {
        return Ok(Plan::Directory { real });
    }

    // A folder with a repository of its own is a checkout, whatever it is called.
    if fs::symlink_metadata(path.join(".git")).is_ok() {
        return Err("it is a git checkout of its own".to_string());
    }

    // Build output or dependencies inside a project.
    let (Some(parent), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else {
        return Err("not a path Guardian recognizes".to_string());
    };
    if classify_project_dir(parent, name).is_none() {
        return Err("it is not a build folder, dependency folder, worktree or cache Guardian knows".to_string());
    }
    // Never delete anything git tracks: that is source, whatever the folder is called.
    // Whether the folder is in a checkout is read from disk, not asked of git: a git that
    // is missing or refuses to run must stop the delete, not pass for "not a repository".
    let in_checkout = parent
        .ancestors()
        .any(|dir| fs::symlink_metadata(dir.join(".git")).is_ok());
    if in_checkout || git(parent, &["rev-parse", "--is-inside-work-tree"]).is_some() {
        let tracked = git(parent, &["ls-files", "--", name])
            .ok_or("git could not say whether it holds tracked files")?;
        if !tracked.trim().is_empty() {
            return Err("git tracks files inside it".to_string());
        }
    }
    Ok(Plan::Directory { real })
}

/// Why `path` cannot be deleted by Guardian, or `None` if it can.
pub fn why_not_deletable(path: &Path, home: &Path) -> Option<String> {
    plan_delete(path, home).err()
}

/// Delete one target after re-checking it. Returns the space it occupied.
pub fn delete_target(path: &Path, home: &Path) -> Result<Deleted, String> {
    // Refuse early, before spending time measuring something that will not be deleted.
    plan_delete(path, home)?;
    let (freed_bytes, _) = dir_size(path, Instant::now() + Duration::from_secs(20), &HashMap::new());
    // Measuring can take a while, so the checks are run again and the delete follows at once.
    match plan_delete(path, home)? {
        Plan::Directory { real } => fs::remove_dir_all(real).map_err(|e| e.to_string())?,
        Plan::Worktree { main } => {
            let output = git_command(&main)
                .args(["-c", "status.showUntrackedFiles=normal", "worktree", "remove"])
                .arg(path)
                .output()
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                let why = String::from_utf8_lossy(&output.stderr);
                return Err(why.split_whitespace().collect::<Vec<_>>().join(" "));
            }
        }
    }
    Ok(Deleted {
        path: path.to_path_buf(),
        freed_bytes,
    })
}

/// Parse an answer to "delete which?": item numbers (`1 3 5`, `2,4`, `1-3`), or `old`.
/// `Ok(None)` means nothing was chosen.
pub fn parse_selection(input: &str, targets: &[Target]) -> Result<Option<Vec<usize>>, String> {
    let input = input.trim().to_lowercase();
    if input.is_empty() || input == "n" || input == "no" || input == "q" {
        return Ok(None);
    }
    if input == "old" {
        let picked: Vec<usize> = (0..targets.len())
            .filter(|i| targets[*i].is_old_and_safe())
            .collect();
        return Ok((!picked.is_empty()).then_some(picked));
    }
    let mut picked = Vec::new();
    for part in input.split(|c: char| c == ',' || c.is_whitespace()).filter(|p| !p.is_empty()) {
        let (from, to) = match part.split_once('-') {
            Some((a, b)) => (a, b),
            None => (part, part),
        };
        let parse = |s: &str| {
            s.parse::<usize>()
                .ok()
                .filter(|n| (1..=targets.len()).contains(n))
                .ok_or_else(|| format!("`{part}` is not a number from 1 to {}", targets.len()))
        };
        let (from, to) = (parse(from)?, parse(to)?);
        if from > to {
            return Err(format!("`{part}` runs backwards"));
        }
        picked.extend((from..=to).map(|n| n - 1));
    }
    picked.sort_unstable();
    picked.dedup();
    Ok((!picked.is_empty()).then_some(picked))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system temp dir standing in for a home directory,
    /// removed on drop. Nothing outside it is touched.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("guardian-disk-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            // Temp dirs are reached through a symlink on macOS; the code compares real paths.
            Self(fs::canonicalize(&dir).unwrap())
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }

        fn file(&self, rel: &str, bytes: usize) {
            let path = self.path(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, vec![b'x'; bytes]).unwrap();
        }

        /// Run git inside `rel`, isolated from the user's own git configuration.
        fn git(&self, rel: &str, args: &[&str]) {
            let dir = self.path(rel);
            fs::create_dir_all(&dir).unwrap();
            let output = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
                .args(["-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .output()
                .expect("git is required for these tests");
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        /// A repository at `rel` with one commit.
        fn repo(&self, rel: &str) {
            self.git(rel, &["init", "-q"]);
            self.file(&format!("{rel}/README.md"), 10);
            self.git(rel, &["add", "README.md"]);
            self.git(rel, &["commit", "-q", "-m", "first"]);
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(30)
    }

    fn none() -> HashMap<PathBuf, (u64, bool)> {
        HashMap::new()
    }

    fn small_floor() -> Options {
        Options {
            min_bytes: 1,
            top: 50,
            ..Options::default()
        }
    }

    fn find<'a>(report: &'a Report, display: &str) -> &'a Target {
        report
            .targets
            .iter()
            .find(|t| t.display_path == display)
            .unwrap_or_else(|| panic!("{display} not in {:?}", report.targets.iter().map(|t| &t.display_path).collect::<Vec<_>>()))
    }

    // --- measuring ---

    #[test]
    fn measures_a_tree() {
        let s = Scratch::new("size");
        s.file("a/one.bin", 200_000);
        s.file("a/b/two.bin", 300_000);
        let (bytes, complete) = dir_size(&s.0, far(), &none());
        assert!(complete);
        assert!((500_000..2_000_000).contains(&bytes), "{bytes}");
    }

    #[test]
    fn missing_path_is_zero() {
        assert_eq!(dir_size(Path::new("/nonexistent/guardian/xyz"), far(), &none()), (0, true));
    }

    #[test]
    fn symlinks_are_not_followed_when_measuring() {
        let s = Scratch::new("symlink");
        s.file("big/data.bin", 1_000_000);
        s.file("small/note.txt", 10);
        std::os::unix::fs::symlink(s.path("big"), s.path("small/link")).unwrap();
        let (bytes, _) = dir_size(&s.path("small"), far(), &none());
        assert!(bytes < 500_000, "{bytes}");
    }

    #[test]
    fn already_measured_folders_are_reused_not_walked() {
        let s = Scratch::new("reuse");
        s.file("wt/src/a.rs", 1_000);
        s.file("wt/node_modules/dep/x.js", 500_000);
        let mut known = none();
        known.insert(s.path("wt/node_modules"), (7_000_000, false));
        let (bytes, complete) = dir_size(&s.path("wt"), far(), &known);
        assert!((7_000_000..7_200_000).contains(&bytes), "{bytes}");
        assert!(!complete, "an incomplete inner size makes the outer one a lower bound");
    }

    // --- finding ---

    #[test]
    fn finds_build_folders_in_repos_anywhere_under_home() {
        let s = Scratch::new("projects");
        s.repo("github/org/app");
        s.file("github/org/app/Cargo.toml", 10);
        s.file("github/org/app/target/debug/app", 300_000);
        s.repo("work/web");
        s.file("work/web/package.json", 10);
        s.file("work/web/node_modules/dep/index.js", 200_000);
        // Nested node_modules belongs to the outer one, not a second target.
        s.file("work/web/node_modules/dep/node_modules/inner/i.js", 50_000);
        // A directory merely called target, with no Cargo project around it, is left alone.
        s.file("github/notes/target/plan.md", 400_000);
        // Folders macOS guards, and app data, are not walked.
        s.file("Documents/proj/node_modules/x.js", 900_000);
        s.file("Library/thing/node_modules/x.js", 900_000);

        let report = survey_home(&s.0, &small_floor());
        let paths: Vec<&str> = report.targets.iter().map(|t| t.display_path.as_str()).collect();
        assert_eq!(paths, vec!["~/github/org/app/target", "~/work/web/node_modules"]);
        assert_eq!(report.repos_scanned, 2);

        let target = find(&report, "~/github/org/app/target");
        assert_eq!(target.kind, Kind::Build);
        assert_eq!(target.repo.as_deref(), Some("~/github/org/app"));
        assert_eq!(target.age_days, Some(0));
        assert_eq!(target.remove, "rm -rf ~/github/org/app/target");
        assert!(target.deletable);
        assert_eq!(report.unmeasured, 0);
    }

    #[test]
    fn extra_roots_are_searched_on_request() {
        let s = Scratch::new("roots");
        s.file("Documents/proj/package.json", 10);
        s.file("Documents/proj/node_modules/x.js", 300_000);
        assert!(survey_home(&s.0, &small_floor()).targets.is_empty());
        let with_root = Options {
            roots: vec![s.path("Documents")],
            ..small_floor()
        };
        let report = survey_home(&s.0, &with_root);
        assert_eq!(report.targets.len(), 1);
        assert_eq!(report.targets[0].display_path, "~/Documents/proj/node_modules");
    }

    #[test]
    fn known_cache_uses_its_own_removal_command() {
        let s = Scratch::new("known");
        s.file(".npm/_cacache/blob", 200_000);
        s.file(".ollama/models/m.bin", 300_000);
        s.file("Library/Caches/ms-playwright/chromium/bin", 250_000);
        let report = survey_home(&s.0, &small_floor());
        let npm = find(&report, "~/.npm/_cacache");
        assert_eq!(npm.remove, "npm cache clean --force");
        assert_eq!(npm.safety, Safety::Regenerates);
        assert!(!npm.deletable, "npm has its own cleaner; Guardian does not delete it");
        assert_eq!(npm.age_days, None);
        let ollama = find(&report, "~/.ollama/models");
        assert_eq!(ollama.safety, Safety::Review);
        assert!(!ollama.deletable);
        let playwright = find(&report, "~/Library/Caches/ms-playwright");
        assert!(playwright.deletable);
        assert!(report.regenerates_bytes >= 450_000);
        assert!(report.review_bytes >= 300_000);
    }

    #[test]
    fn size_floor_top_limit_and_age_filter_apply() {
        let s = Scratch::new("floor");
        s.file(".npm/_cacache/blob", 50_000);
        s.file(".cache/uv/blob", 900_000);
        s.file("Library/Caches/pip/blob", 600_000);
        s.file("code/web/node_modules/x.js", 700_000);
        let floor = Options {
            min_bytes: 400_000,
            ..small_floor()
        };
        let paths: Vec<String> = survey_home(&s.0, &floor)
            .targets
            .into_iter()
            .map(|t| t.display_path)
            .collect();
        assert_eq!(paths, vec!["~/.cache/uv", "~/code/web/node_modules", "~/Library/Caches/pip"]);

        let top_one = Options {
            top: 1,
            ..small_floor()
        };
        let report = survey_home(&s.0, &top_one);
        assert_eq!(report.targets.len(), 1);
        // Totals still cover everything found, not just what is listed.
        assert!(report.regenerates_bytes >= 2_200_000);

        // Just created, so "older than a week" leaves the project folder out; caches stay.
        let old_only = Options {
            older_than_days: 7,
            ..small_floor()
        };
        let report = survey_home(&s.0, &old_only);
        assert!(report.targets.iter().all(|t| t.kind == Kind::Cache));
        assert!(!report.targets.is_empty());
    }

    #[test]
    fn exhausted_budget_reports_unmeasured_places() {
        let s = Scratch::new("budget");
        s.file(".npm/_cacache/blob", 50_000);
        s.file(".cache/uv/blob", 50_000);
        let no_time = Options {
            budget: Duration::ZERO,
            ..small_floor()
        };
        let report = survey_home(&s.0, &no_time);
        assert!(report.targets.is_empty());
        assert_eq!(report.unmeasured, 2);
        assert!(report.render().contains("2 more recently used places were not measured"));
    }

    // --- worktrees ---

    #[test]
    fn clean_worktree_is_offered_and_a_dirty_one_is_not() {
        let s = Scratch::new("worktrees");
        s.repo("code/app");
        s.git("code/app", &["worktree", "add", "-q", "-b", "feature", "../app-wt-clean"]);
        s.git("code/app", &["worktree", "add", "-q", "-b", "wip", "../app-wt-dirty"]);
        s.file("code/app-wt-clean/package.json", 10);
        s.git("code/app-wt-clean", &["add", "package.json"]);
        s.git("code/app-wt-clean", &["commit", "-q", "-m", "pkg"]);
        s.file("code/app-wt-clean/node_modules/dep/x.js", 400_000);
        fs::write(s.path("code/app-wt-clean/.gitignore"), "node_modules\n").unwrap();
        s.git("code/app-wt-clean", &["add", ".gitignore"]);
        s.git("code/app-wt-clean", &["commit", "-q", "-m", "ignore"]);
        s.file("code/app-wt-dirty/notes.txt", 100);

        let report = survey_home(&s.0, &small_floor());
        assert_eq!(report.repos_scanned, 1, "worktrees are not counted as repositories");

        let clean = find(&report, "~/code/app-wt-clean");
        assert_eq!(clean.kind, Kind::Worktree);
        assert_eq!(clean.safety, Safety::Regenerates);
        assert!(clean.deletable);
        assert_eq!(clean.branch.as_deref(), Some("feature"));
        assert_eq!(clean.repo.as_deref(), Some("~/code/app"));
        assert!(clean.note.as_deref().unwrap().contains("branch feature stays"), "{:?}", clean.note);
        assert_eq!(clean.remove, "git -C ~/code/app worktree remove ~/code/app-wt-clean");
        assert!(clean.bytes >= 400_000, "the worktree's size includes its node_modules");

        let dirty = find(&report, "~/code/app-wt-dirty");
        assert_eq!(dirty.safety, Safety::Review);
        assert!(!dirty.deletable);
        assert_eq!(dirty.note.as_deref(), Some("1 uncommitted or untracked file"));

        // The node_modules inside the listed worktree is not counted a second time.
        let inner = find(&report, "~/code/app-wt-clean/node_modules");
        assert!(report.regenerates_bytes < clean.bytes + inner.bytes);
    }

    #[test]
    fn worktree_with_env_file_needs_a_look() {
        let s = Scratch::new("envfile");
        s.repo("code/app");
        fs::write(s.path("code/app/.gitignore"), ".env*\n").unwrap();
        s.git("code/app", &["add", ".gitignore"]);
        s.git("code/app", &["commit", "-q", "-m", "ignore env"]);
        s.git("code/app", &["worktree", "add", "-q", "-b", "feature", "../app-wt"]);
        s.file("code/app-wt/.env.local", 20);
        let report = survey_home(&s.0, &small_floor());
        let wt = find(&report, "~/code/app-wt");
        assert_eq!(wt.safety, Safety::Review);
        assert!(wt.deletable);
        assert!(wt.note.as_deref().unwrap().contains(".env"), "{:?}", wt.note);
        assert!(!wt.is_old_and_safe());
    }

    #[test]
    fn detached_worktree_with_commits_found_nowhere_else_is_kept() {
        let s = Scratch::new("detached");
        s.repo("code/app");
        s.git("code/app", &["worktree", "add", "-q", "--detach", "../app-wt"]);
        s.file("code/app-wt/new.txt", 10);
        s.git("code/app-wt", &["add", "new.txt"]);
        s.git("code/app-wt", &["commit", "-q", "-m", "only here"]);
        let report = survey_home(&s.0, &small_floor());
        let wt = find(&report, "~/code/app-wt");
        assert!(!wt.deletable);
        assert_eq!(wt.note.as_deref(), Some("1 commit exists only here (detached HEAD)"));
        assert!(delete_target(&s.path("code/app-wt"), &s.0).is_err());
        assert!(s.path("code/app-wt/new.txt").exists());
    }

    // --- deleting ---

    #[test]
    fn deletes_a_clean_worktree_through_git_and_keeps_its_branch() {
        let s = Scratch::new("delwt");
        s.repo("code/app");
        s.git("code/app", &["worktree", "add", "-q", "-b", "feature", "../app-wt"]);
        s.file("code/app-wt/extra.txt", 10);

        // Untracked file: refused, nothing removed.
        let err = delete_target(&s.path("code/app-wt"), &s.0).unwrap_err();
        assert!(err.contains("1 uncommitted or untracked file"), "{err}");
        assert!(s.path("code/app-wt/extra.txt").exists());

        fs::remove_file(s.path("code/app-wt/extra.txt")).unwrap();
        delete_target(&s.path("code/app-wt"), &s.0).unwrap();
        assert!(!s.path("code/app-wt").exists());
        assert!(linked_worktrees(&s.path("code/app")).is_empty());
        let branches = git(&s.path("code/app"), &["branch", "--list", "feature"]).unwrap();
        assert!(branches.contains("feature"), "the branch must survive: {branches:?}");
    }

    #[test]
    fn deletes_a_build_folder_but_never_one_with_tracked_files() {
        let s = Scratch::new("delbuild");
        s.repo("code/app");
        s.file("code/app/Cargo.toml", 10);
        s.file("code/app/target/debug/app", 300_000);
        let done = delete_target(&s.path("code/app/target"), &s.0).unwrap();
        assert!(done.freed_bytes >= 300_000);
        assert!(!s.path("code/app/target").exists());
        assert!(s.path("code/app/Cargo.toml").exists());

        // Someone committed a folder called node_modules: that is source now.
        s.repo("code/odd");
        s.file("code/odd/node_modules/vendored.js", 100);
        s.git("code/odd", &["add", "-f", "node_modules/vendored.js"]);
        s.git("code/odd", &["commit", "-q", "-m", "vendored"]);
        let err = delete_target(&s.path("code/odd/node_modules"), &s.0).unwrap_err();
        assert_eq!(err, "git tracks files inside it");
        assert!(s.path("code/odd/node_modules/vendored.js").exists());
    }

    #[test]
    fn build_folder_is_kept_when_git_cannot_answer() {
        let s = Scratch::new("gitfails");
        // A checkout git cannot read: `.git` is there, but it is not a repository.
        s.file("code/broken/.git/garbage", 10);
        s.file("code/broken/package.json", 10);
        s.file("code/broken/node_modules/dep/x.js", 100);
        let err = delete_target(&s.path("code/broken/node_modules"), &s.0).unwrap_err();
        assert_eq!(err, "git could not say whether it holds tracked files");
        assert!(s.path("code/broken/node_modules/dep/x.js").exists());
    }

    #[test]
    fn untracked_files_count_even_when_git_is_configured_to_hide_them() {
        let s = Scratch::new("hideuntracked");
        s.repo("code/app");
        s.git("code/app", &["config", "status.showUntrackedFiles", "no"]);
        s.git("code/app", &["worktree", "add", "-q", "-b", "feature", "../app-wt"]);
        s.file("code/app-wt/unsaved.txt", 10);
        let report = survey_home(&s.0, &small_floor());
        let wt = find(&report, "~/code/app-wt");
        assert!(!wt.deletable, "{:?}", wt.note);
        assert_eq!(wt.note.as_deref(), Some("1 uncommitted or untracked file"));
        assert!(delete_target(&s.path("code/app-wt"), &s.0).is_err());
        assert!(s.path("code/app-wt/unsaved.txt").exists());
    }

    #[test]
    fn ignored_files_anywhere_in_a_worktree_need_a_look() {
        let s = Scratch::new("ignored");
        s.repo("code/app");
        s.file("code/app/apps/web/index.js", 10);
        fs::write(s.path("code/app/.gitignore"), ".env*\n*.sqlite\nnode_modules\n.DS_Store\n").unwrap();
        s.git("code/app", &["add", ".gitignore", "apps/web/index.js"]);
        s.git("code/app", &["commit", "-q", "-m", "app"]);
        s.git("code/app", &["worktree", "add", "-q", "-b", "feature", "../app-wt"]);
        s.git("code/app", &["worktree", "add", "-q", "-b", "deps", "../app-wt-deps"]);
        s.file("code/app-wt/apps/web/.env.local", 20);
        s.file("code/app-wt/dev.sqlite", 20);
        // Only dependencies and scratch files: nothing to look at.
        s.file("code/app-wt-deps/node_modules/dep/x.js", 20);
        s.file("code/app-wt-deps/.DS_Store", 20);

        let report = survey_home(&s.0, &small_floor());
        let wt = find(&report, "~/code/app-wt");
        assert_eq!(wt.safety, Safety::Review);
        assert!(wt.deletable);
        assert!(!wt.is_old_and_safe());
        let note = wt.note.as_deref().unwrap();
        assert!(note.contains("1 other ignored item would be lost"), "{note}");

        let deps = find(&report, "~/code/app-wt-deps");
        assert_eq!(deps.safety, Safety::Regenerates, "{:?}", deps.note);
    }

    #[test]
    fn folder_with_its_own_repository_is_never_a_build_folder() {
        let s = Scratch::new("nested");
        s.repo("code/app");
        s.file("code/app/package.json", 10);
        // A clone that happens to be called node_modules.
        s.repo("code/app/node_modules");
        let err = delete_target(&s.path("code/app/node_modules"), &s.0).unwrap_err();
        assert_eq!(err, "it is a git checkout of its own");
        assert!(s.path("code/app/node_modules/README.md").exists());
    }

    #[test]
    fn worktree_with_files_hidden_from_status_is_kept() {
        let s = Scratch::new("skipworktree");
        s.repo("code/app");
        s.git("code/app", &["worktree", "add", "-q", "-b", "feature", "../app-wt"]);
        s.git("code/app-wt", &["update-index", "--skip-worktree", "README.md"]);
        s.file("code/app-wt/README.md", 500);
        let report = survey_home(&s.0, &small_floor());
        let wt = find(&report, "~/code/app-wt");
        assert!(!wt.deletable);
        assert!(wt.note.as_deref().unwrap().contains("hidden from git status"), "{:?}", wt.note);
        assert!(delete_target(&s.path("code/app-wt"), &s.0).is_err());
    }

    #[test]
    fn refuses_everything_it_does_not_recognize() {
        let s = Scratch::new("refuse");
        s.repo("code/app");
        s.file("code/app/src/main.rs", 10);
        s.file("code/app/Cargo.toml", 10);
        s.file("code/app/target/x", 10);
        s.file(".npm/_cacache/blob", 10);
        std::os::unix::fs::symlink(s.path("code/app/target"), s.path("code/app/link")).unwrap();

        let refused = |rel: &str| delete_target(&s.path(rel), &s.0).is_err();
        assert!(refused("code/app/src"), "source folder");
        assert!(refused("code/app"), "a whole repository");
        assert!(refused("code"), "too close to home");
        assert!(refused("code/app/link"), "a symlink to a build folder");
        assert!(refused(".npm/_cacache"), "npm has its own cleaner");
        assert!(refused("code/app/missing"), "nothing there");
        assert!(delete_target(&s.0, &s.0).is_err(), "home itself");
        assert!(delete_target(Path::new("/usr/local/target"), &s.0).is_err(), "outside home");
        assert!(delete_target(Path::new("relative/target"), &s.0).is_err(), "relative path");
        // Everything is still there.
        assert!(s.path("code/app/src/main.rs").exists());
        assert!(s.path("code/app/target/x").exists());
        assert!(s.path(".npm/_cacache/blob").exists());
    }

    #[test]
    fn deletes_a_plain_cache() {
        let s = Scratch::new("delcache");
        s.file("Library/Caches/ms-playwright/chromium/bin", 100_000);
        delete_target(&s.path("Library/Caches/ms-playwright"), &s.0).unwrap();
        assert!(!s.path("Library/Caches/ms-playwright").exists());
        assert!(s.path("Library/Caches").exists());
    }

    // --- choosing ---

    fn target(age: Option<u64>, safety: Safety, kind: Kind, deletable: bool) -> Target {
        Target {
            path: PathBuf::from("/x"),
            display_path: "~/x".to_string(),
            kind,
            what: "w".to_string(),
            cost: "c".to_string(),
            remove: "r".to_string(),
            safety,
            bytes: 1,
            complete: true,
            age_days: age,
            repo: None,
            branch: None,
            note: None,
            deletable,
        }
    }

    #[test]
    fn selection_parsing() {
        let items = vec![
            target(Some(40), Safety::Regenerates, Kind::Build, true),
            target(Some(2), Safety::Regenerates, Kind::Build, true),
            target(Some(90), Safety::Review, Kind::Worktree, true),
            target(Some(30), Safety::Regenerates, Kind::Worktree, true),
            target(None, Safety::Regenerates, Kind::Cache, true),
        ];
        assert_eq!(parse_selection("", &items), Ok(None));
        assert_eq!(parse_selection("  n ", &items), Ok(None));
        assert_eq!(parse_selection("1 3", &items), Ok(Some(vec![0, 2])));
        assert_eq!(parse_selection("3,1,1", &items), Ok(Some(vec![0, 2])));
        assert_eq!(parse_selection("2-4", &items), Ok(Some(vec![1, 2, 3])));
        // `old`: comes back on its own, untouched two weeks, not a shared tool cache.
        assert_eq!(parse_selection("OLD", &items), Ok(Some(vec![0, 3])));
        assert!(parse_selection("0", &items).is_err());
        assert!(parse_selection("6", &items).is_err());
        assert!(parse_selection("4-2", &items).is_err());
        assert!(parse_selection("all", &items).is_err());
        assert!(parse_selection("y", &items).is_err(), "a stray yes selects nothing");
    }

    // --- text ---

    #[test]
    fn paths_are_quoted_for_the_shell() {
        let home = Path::new("/Users/me");
        assert_eq!(shell_path(Path::new("/Users/me/a/b-c_d.e"), home), "~/a/b-c_d.e");
        assert_eq!(
            shell_path(Path::new("/Users/me/Library/Developer/Xcode/iOS DeviceSupport"), home),
            "\"$HOME/Library/Developer/Xcode/iOS DeviceSupport\""
        );
        assert_eq!(
            shell_path(Path::new("/Users/me/we$ird\"dir"), home),
            "\"$HOME/we\\$ird\\\"dir\""
        );
        assert_eq!(shell_path(Path::new("/opt/x y"), home), "\"/opt/x y\"");
        assert_eq!(display_path(Path::new("/Users/me/a b"), home), "~/a b");
        assert_eq!(display_path(Path::new("/opt/x"), home), "/opt/x");
    }

    #[test]
    fn sizes_and_ages_read_naturally() {
        assert_eq!(human_size(5 * 1024 * 1024 * 1024 + 300 * 1024 * 1024), "5.3 GB");
        assert_eq!(human_size(250 * 1024 * 1024), "250 MB");
        let t = |age| target(age, Safety::Regenerates, Kind::Build, true);
        assert_eq!(t(Some(0)).age_text().as_deref(), Some("used today"));
        assert_eq!(t(Some(1)).age_text().as_deref(), Some("untouched 1 day"));
        assert_eq!(t(Some(34)).age_text().as_deref(), Some("untouched 34 days"));
        assert_eq!(t(None).age_text(), None);
    }

    #[test]
    fn listing_groups_by_safety_and_says_it_deletes_nothing() {
        let s = Scratch::new("render");
        s.file(".npm/_cacache/blob", 200_000);
        s.file(".Trash/old.bin", 300_000);
        s.file("code/web/node_modules/x.js", 300_000);
        let text = survey_home(&s.0, &small_floor()).render();
        let own = text.find("Comes back on its own").unwrap();
        let look = text.find("Look before deleting").unwrap();
        assert!(own < look, "{text}");
        assert!(text.contains("This listing deletes nothing"), "{text}");
        assert!(text.contains("$ npm cache clean --force"), "{text}");
        assert!(text.contains("to empty trash"), "{text}");
        assert!(text.contains("used today"), "{text}");
    }
}

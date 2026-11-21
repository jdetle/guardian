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
//! machine that is already short of disk and memory. What it learns is kept in
//! `~/.guardian/disk_index.json`, so the next scan measures only what is new, has changed or
//! has not been measured for a while, and a scan that ran out of time is carried on by the
//! next one. `guardiand` keeps that file fresh in the background (`guardian disk refresh`).
//!
//! Sizes are reported twice: what a folder occupies, and what deleting it would free. The
//! two differ when its files are clones of files elsewhere, as a bun or pnpm install's are.

use crate::dir_list;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How much a removal costs the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Safety {
    /// Rebuilt, re-downloaded or re-creatable from what git already holds.
    Regenerates,
    /// May hold something not reproducible, or needs a choice (which models, which images).
    Review,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// What it occupies on disk.
    pub bytes: u64,
    /// What deleting it would free at once: `bytes`, less whatever its files share with
    /// copies elsewhere (clones) or have other names outside it (hard links).
    pub freed_bytes: u64,
    /// False when the time budget ran out mid-measurement: the sizes are then lower bounds.
    pub complete: bool,
    /// When it was measured, in seconds since 1970.
    pub measured_at: Option<u64>,
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

/// How long something has gone untouched, in the bands a listing is grouped by. Declared
/// from the longest untouched to the most recent, which is also the order they are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Staleness {
    /// 90 days or more.
    Quarter,
    /// 30 to 89 days.
    Month,
    /// 14 to 29 days.
    Fortnight,
    /// 7 to 13 days.
    Week,
    /// Touched in the last week, or with no way to tell.
    InUse,
}

impl Staleness {
    pub fn of(age_days: Option<u64>) -> Self {
        match age_days {
            Some(d) if d >= 90 => Self::Quarter,
            Some(d) if d >= 30 => Self::Month,
            Some(d) if d >= OLD_DAYS => Self::Fortnight,
            Some(d) if d >= 7 => Self::Week,
            _ => Self::InUse,
        }
    }

    pub fn heading(self) -> &'static str {
        match self {
            Self::Quarter => "Untouched 90 days or more",
            Self::Month => "Untouched 30 to 89 days",
            Self::Fortnight => "Untouched 14 to 29 days",
            Self::Week => "Untouched 7 to 13 days",
            Self::InUse => "Used in the last week",
        }
    }

    /// Untouched for [`OLD_DAYS`] or more: what deleting in bulk goes by.
    pub fn is_old(self) -> bool {
        self <= Self::Fortnight
    }
}

/// Stale things that belong together: everything in one project (a repository with its
/// worktrees and build folders), or among the tool caches, that has gone untouched for
/// about as long. Small on their own, they are listed and deleted as one.
#[derive(Clone, Debug, Serialize)]
pub struct Bundle {
    /// The project, as `~/…`, or "Tool caches".
    pub name: String,
    /// False for the bundle of tool caches.
    pub is_project: bool,
    pub staleness: Staleness,
    /// The band in words: "Untouched 30 to 89 days".
    pub heading: String,
    /// How stale it is and how much of it can go without a look, in a sentence.
    pub detail: String,
    /// What it is made of: "3 worktrees, 4 build folders".
    pub contents: String,
    /// What it occupies, and what deleting all of it would free. Each folder is counted
    /// once: a build folder inside a worktree of the bundle is part of the worktree.
    pub bytes: u64,
    pub freed_bytes: u64,
    /// The part of those that comes back on its own.
    pub regenerates_bytes: u64,
    pub regenerates_freed_bytes: u64,
    /// The longest and the shortest any of it has gone untouched, in days.
    pub oldest_days: u64,
    pub newest_days: u64,
    /// What `guardian disk delete` can remove now without a look: Guardian can delete it
    /// and it comes back on its own. Nothing here is inside anything else here.
    pub delete_paths: Vec<PathBuf>,
    /// What deleting `delete_paths` would free.
    pub deletable_freed_bytes: u64,
    /// How many of its folders are left out of `delete_paths`: they need a look first, or
    /// their own command.
    pub held_back: usize,
    /// Its folders, the ones that free the most first.
    pub members: Vec<Target>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub volume: String,
    pub used_percent: f64,
    pub available_gb: f64,
    /// Stale things grouped by project and by how long they have gone untouched, the
    /// longest untouched first. Only bundles at or above the size floor.
    pub bundles: Vec<Bundle>,
    /// Everything not in a bundle, largest first; only targets at or above the size floor.
    pub targets: Vec<Target>,
    /// Candidates found but not measured before the time budget ran out.
    pub unmeasured: usize,
    /// Git repositories found (main checkouts, not counting their worktrees).
    pub repos_scanned: usize,
    /// What the targets of each kind occupy on disk.
    pub regenerates_bytes: u64,
    pub review_bytes: u64,
    /// What deleting all the targets of each kind would free at once.
    pub regenerates_freed_bytes: u64,
    pub review_freed_bytes: u64,
    /// False when the search for repositories ran out of time: there may be more to find.
    pub discovery_complete: bool,
    /// When the scan behind this report finished, in seconds since 1970.
    pub scanned_at: u64,
    /// True when this is a saved scan read back (`guardian disk --cached`), not a new one.
    pub from_cache: bool,
    /// Sizes taken from an earlier scan because nothing had changed since.
    pub reused: usize,
    /// Sizes shown from an earlier scan that were due for measuring again but not reached
    /// in time.
    pub outdated: usize,
    /// Places another scan still has work to do on: never measured, measured only in part,
    /// or due for measuring again and not reached.
    pub pending: usize,
}

pub struct Options {
    /// Extra directories to search (the home directory is always searched).
    pub roots: Vec<PathBuf>,
    pub min_bytes: u64,
    pub top: usize,
    pub budget: Duration,
    /// Leave out project build folders and worktrees touched more recently than this.
    pub older_than_days: u64,
    /// A size from an earlier scan is measured again once it is this old, even if nothing
    /// looks to have changed. Zero measures everything afresh.
    pub max_age: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            min_bytes: 100 * 1024 * 1024,
            top: 15,
            budget: Duration::from_secs(45),
            older_than_days: 0,
            max_age: DEFAULT_MAX_AGE,
        }
    }
}

/// Untouched for this long counts as "old" when choosing what to delete in bulk.
pub const OLD_DAYS: u64 = 14;

/// Exit status of `guardian disk refresh` when it saved what it could but ran out of time
/// with places still to measure or to search: guardiand then runs it again soon.
pub const REFRESH_EXIT_MORE: i32 = 3;

/// How long a saved size stands in for measuring again when nothing looks to have changed.
pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(12 * 3600);

/// What a folder occupies, and what deleting it would give back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    /// Allocated on disk. A file with several names is counted once.
    pub bytes: u64,
    /// Freed at once by deleting the folder: `bytes`, less what its files share with clones
    /// or snapshots, and less files that have another name outside the folder.
    pub freed: u64,
    /// False when time ran out mid-measurement: both numbers are then lower bounds.
    pub complete: bool,
}

/// A place to look. Also what the index file stores, with its size once measured.
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    /// Its size when last measured; `None` until then.
    #[serde(default)]
    size: Option<Size>,
    /// When that was, in seconds since 1970.
    #[serde(default)]
    measured_at: Option<u64>,
    /// A worktree that git should be asked about when it is next measured. Never saved:
    /// each scan decides it anew.
    #[serde(skip)]
    needs_check: bool,
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
/// the latter. `skip_top` names children of `root` that are not walked. False when
/// `deadline` passed before the walk was done.
fn walk(root: &Path, skip_top: &[&str], deadline: Instant, out: &mut Found) -> bool {
    let mut stack: Vec<(PathBuf, usize, Option<PathBuf>)> = vec![(root.to_path_buf(), 0, None)];
    while let Some((dir, depth, inherited)) = stack.pop() {
        if Instant::now() >= deadline {
            return false;
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
                    size: None,
                    measured_at: None,
                    needs_check: false,
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
    true
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

/// What a worktree says until git has been asked about it.
const UNCHECKED: &str = "not yet checked for unsaved work";
const LOCKED: &str = "locked with git worktree lock";

/// List the linked worktrees of `main`. Whether each holds unsaved work is not asked here:
/// that takes git a second or more per worktree, so it is done with the measuring
/// (`check_worktree`), where it is budgeted, carried on by the next scan, and skipped for a
/// worktree that has not changed. False when `deadline` passed before the listing was done.
fn worktree_candidates(main: &Path, home: &Path, deadline: Instant, out: &mut Vec<Candidate>) -> bool {
    for wt in linked_worktrees(main) {
        if Instant::now() >= deadline {
            return false;
        }
        if !wt.path.is_dir() {
            continue;
        }
        // Until checked, a worktree is not offered for deletion.
        let state = WorktreeState {
            safety: Safety::Review,
            deletable: false,
            note: if wt.locked { LOCKED } else { UNCHECKED }.to_string(),
        };
        let deletable = false;
        out.push(Candidate {
            needs_check: !wt.locked,
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
            size: None,
            measured_at: None,
        });
    }
    true
}

/// Ask git whether deleting this worktree would lose anything, and record the answer.
fn check_worktree(c: &mut Candidate, home: &Path) {
    let state = worktree_state(&Linked {
        path: c.path.clone(),
        branch: c.branch.clone(),
        locked: false,
    });
    // A worktree outside the home folder is listed, with its command, but never deleted
    // by Guardian (see `guard_path`).
    c.deletable = state.deletable && guard_path(&c.path, home).is_ok();
    c.safety = state.safety;
    c.note = Some(state.note);
    c.needs_check = false;
}

/// Disk space used under `root` (allocated blocks, so sparse files such as Docker's disk
/// image count for what they really occupy). Stays on one volume, never follows symlinks.
/// Directories in `known` are not walked again: their recorded size is added instead.
/// `complete` is false in the result when `deadline` passed first.
fn dir_size(root: &Path, deadline: Instant, known: &HashMap<PathBuf, Size>) -> Size {
    let Ok(root_meta) = fs::symlink_metadata(root) else {
        return Size { bytes: 0, freed: 0, complete: true };
    };
    let device = root_meta.dev();
    let own = root_meta.blocks().saturating_mul(512);
    let mut size = Size { bytes: own, freed: own, complete: true };
    if !root_meta.file_type().is_dir() {
        return size;
    }
    // Files with more than one name (hard links), by inode: names seen under `root`, names
    // in all, and the space the file would free. Cargo's build output is full of these.
    let mut linked: HashMap<u64, (u64, u64, u64)> = HashMap::new();
    let (mut buf, mut entries) = (Vec::new(), Vec::new());
    let mut stack = vec![root.to_path_buf()];
    let mut seen: u32 = 0;
    'walk: while let Some(dir) = stack.pop() {
        if !dir_list::list(&dir, &mut buf, &mut entries) {
            continue;
        }
        for entry in entries.drain(..) {
            seen = seen.wrapping_add(1);
            if seen % 512 == 0 && Instant::now() >= deadline {
                size.complete = false;
                break 'walk;
            }
            if entry.device != device {
                continue;
            }
            if entry.is_dir {
                let path = dir.join(&entry.name);
                if let Some(inner) = known.get(&path) {
                    size.bytes = size.bytes.saturating_add(inner.bytes);
                    size.freed = size.freed.saturating_add(inner.freed);
                    size.complete &= inner.complete;
                    continue;
                }
                stack.push(path);
            } else if entry.links > 1 {
                let names = linked.entry(entry.inode).or_insert_with(|| {
                    size.bytes = size.bytes.saturating_add(entry.allocated);
                    (0, entry.links, entry.private)
                });
                names.0 += 1;
            } else {
                size.bytes = size.bytes.saturating_add(entry.allocated);
                size.freed = size.freed.saturating_add(entry.private);
            }
        }
    }
    // A file with several names is freed only once every one of them is gone, so it counts
    // only when all of them are inside this folder.
    for (seen_here, in_all, private) in linked.values() {
        if seen_here >= in_all {
            size.freed = size.freed.saturating_add(*private);
        }
    }
    size
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

/// "3 hours ago", for a length of time in seconds.
pub fn ago_text(secs: u64) -> String {
    let (n, unit) = match secs {
        0..=89 => return "just now".to_string(),
        90..=5_399 => ((secs + 30) / 60, "minute"),
        5_400..=129_599 => ((secs + 1_800) / 3_600, "hour"),
        _ => ((secs + 43_200) / 86_400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
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

/// What a search for places to look turned up.
struct Discovery {
    candidates: Vec<Candidate>,
    /// Main checkouts found.
    repos: usize,
    /// False when `deadline` passed before everywhere had been searched.
    complete: bool,
}

fn candidates(home: &Path, opts: &Options, deadline: Instant) -> Discovery {
    let mut found = Found::default();
    for (rel, what, cost, remove, safety) in KNOWN {
        let path = home.join(rel);
        if !path.exists() {
            continue;
        }
        // When something was last added or removed at its top level. Only for caches that
        // refill themselves: a toolchain or a model is not stale for having been installed
        // long ago, and nothing records when it was last used.
        let last_touch = (*safety == Safety::Regenerates)
            .then(|| newest_shallow(&path))
            .flatten();
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
            last_touch,
            branch: None,
            note: None,
            size: None,
            measured_at: None,
            needs_check: false,
        });
    }

    let mut complete = walk(home, HOME_SKIP, deadline, &mut found);
    for root in &opts.roots {
        complete &= walk(root, &[], deadline, &mut found);
    }

    let mut repos = std::mem::take(&mut found.repos);
    repos.sort();
    repos.dedup();
    for main in &repos {
        complete &= worktree_candidates(main, home, deadline, &mut found.candidates);
    }

    // The same directory can be reached twice (an extra root inside the home directory).
    let mut seen = HashSet::new();
    found.candidates.retain(|c| seen.insert(c.path.clone()));
    Discovery {
        candidates: found.candidates,
        repos: repos.len(),
        complete,
    }
}

fn age_days(last_touch: Option<SystemTime>) -> Option<u64> {
    let elapsed = SystemTime::now().duration_since(last_touch?).unwrap_or_default();
    Some(elapsed.as_secs() / 86_400)
}

fn unix_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn unix_now() -> u64 {
    unix_secs(SystemTime::now())
}

/// A directory itself, not a symlink to one.
fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

// ---------------------------------------------------------------------------------------
// The index: what earlier scans found and measured
// ---------------------------------------------------------------------------------------

const INDEX_VERSION: u32 = 1;

/// While measuring, the index is written out this often, so a scan that is stopped part-way
/// keeps most of what it measured.
const SAVE_EVERY: Duration = Duration::from_secs(15);

/// Everything the last scan knew: each place it found, with its size if it has been
/// measured. Read only to save measuring again and to list without scanning; whether
/// something may be deleted is always decided afresh from the disk (`plan_delete`).
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Index {
    version: u32,
    /// The home directory it describes; an index for another one is ignored.
    home: PathBuf,
    /// When it was written, in seconds since 1970.
    scanned_at: u64,
    discovery_complete: bool,
    repos_scanned: usize,
    entries: Vec<Candidate>,
}

/// Where the index is kept: `~/.guardian/disk_index.json`.
pub fn index_path() -> PathBuf {
    crate::config::guardian_dir().join("disk_index.json")
}

/// The index in `file`, if there is one this version wrote for `home`.
fn load_index(file: &Path, home: &Path) -> Option<Index> {
    let text = fs::read_to_string(file).ok()?;
    let index: Index = serde_json::from_str(&text).ok()?;
    (index.version == INDEX_VERSION && index.home == home).then_some(index)
}

/// Write the index whole or not at all: readers never see half a file.
fn save_index(file: &Path, index: &Index) -> std::io::Result<()> {
    let json = match serde_json::to_vec(index) {
        Ok(json) => json,
        Err(_) => {
            // JSON cannot hold a path that is not valid UTF-8. Leave such places out rather
            // than save nothing: they are found and measured again by each scan.
            let utf8 = |p: &Path| p.to_str().is_some();
            let mut plain = index.clone();
            plain
                .entries
                .retain(|c| utf8(&c.path) && c.repo.as_deref().map_or(true, utf8));
            serde_json::to_vec(&plain)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
        }
    };
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension(format!("json.{}.tmp", std::process::id()));
    let result = fs::write(&tmp, json).and_then(|()| fs::rename(&tmp, file));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Held by the one scan allowed to write the index. Released when dropped, or when the
/// process ends however it ends.
struct IndexLock(fs::File);

impl IndexLock {
    /// `None` when another scan holds it, or the lock file cannot be made.
    fn try_take(index_file: &Path) -> Option<Self> {
        use std::os::fd::AsRawFd;
        if let Some(dir) = index_file.parent() {
            fs::create_dir_all(dir).ok()?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(index_file.with_extension("lock"))
            .ok()?;
        let taken = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if !taken {
            return None;
        }
        // A scan stopped in the middle of saving leaves its half-written file behind. With
        // the lock held no scan is saving, so any such file is abandoned.
        if let (Some(dir), Some(name)) = (index_file.parent(), index_file.file_name().and_then(|n| n.to_str())) {
            for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
                let leftover = entry
                    .file_name()
                    .to_str()
                    .is_some_and(|f| f.starts_with(&format!("{name}.")) && f.ends_with(".tmp"));
                if leftover {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        Some(Self(file))
    }
}

/// Find and measure targets under `home`. Read-only, and remembers nothing between calls.
pub fn survey_home(home: &Path, opts: &Options) -> Report {
    survey_with(home, opts, None, &mut |_| {}).0
}

/// Find targets under `home` and measure the ones `prior` has no current size for: those
/// never measured, changed since, measured only in part, or measured longer ago than
/// `opts.max_age`. Returns the report and the index to keep for next time; `save` is also
/// handed the index every so often while measuring.
fn survey_with(
    home: &Path,
    opts: &Options,
    prior: Option<Index>,
    save: &mut dyn FnMut(&Index),
) -> (Report, Index) {
    let started = Instant::now();
    let deadline = started + opts.budget;
    // Finding things may use at most a third of the budget; measuring gets the rest.
    let discovery_deadline = started + opts.budget / 3;

    let found = candidates(home, opts, discovery_deadline);
    let discovery_complete = found.complete;
    let mut repos_scanned = found.repos;
    let mut entries = found.candidates;

    if let Some(prior) = prior {
        let mut earlier: HashMap<PathBuf, Candidate> = prior
            .entries
            .into_iter()
            .map(|c| (c.path.clone(), c))
            .collect();
        for c in &mut entries {
            if let Some(before) = earlier.remove(&c.path) {
                c.size = before.size;
                c.measured_at = before.measured_at;
                if c.needs_check {
                    // What git said last time stands until the worktree is measured again.
                    c.safety = before.safety;
                    c.deletable = before.deletable;
                    if before.note.as_deref() == Some(LOCKED) {
                        // Locked then, not now: ask git this time.
                        c.measured_at = None;
                    } else {
                        c.note = before.note;
                    }
                }
            }
        }
        if !discovery_complete {
            // A search cut short says nothing about the places it did not reach: keep what
            // an earlier search found there, as long as it still exists.
            let mut kept: Vec<Candidate> = earlier
                .into_values()
                .filter(|c| is_real_dir(&c.path))
                .collect();
            for c in &mut kept {
                c.needs_check = c.kind == Kind::Worktree;
            }
            kept.sort_by(|a, b| a.path.cmp(&b.path));
            entries.extend(kept);
            repos_scanned = repos_scanned.max(prior.repos_scanned);
        }
    }

    let due = plan_measuring(&mut entries, opts.max_age, unix_now());

    // No single folder may use more than this much of the budget; it then reports a lower
    // bound instead of starving everything after it.
    let per_target = (opts.budget / 8).max(Duration::from_secs(2));
    let index_of = |entries: &[Candidate]| Index {
        version: INDEX_VERSION,
        home: home.to_path_buf(),
        scanned_at: unix_now(),
        discovery_complete,
        repos_scanned,
        entries: entries.to_vec(),
    };

    // Sizes that stand: a worktree being measured adds these in rather than walking them.
    let mut sizes: HashMap<PathBuf, Size> = entries
        .iter()
        .zip(&due)
        .filter(|(_, due)| !**due)
        .filter_map(|(c, _)| Some((c.path.clone(), c.size?)))
        .collect();
    let reused = sizes.len();
    let mut outdated = 0usize;
    let mut reached = vec![false; entries.len()];
    let mut last_save = Instant::now();
    for i in 0..entries.len() {
        if !due[i] {
            continue;
        }
        if Instant::now() >= deadline {
            // Not reached this time. An earlier size, if there is one, is still shown.
            outdated += usize::from(entries[i].size.is_some());
            continue;
        }
        if entries[i].needs_check {
            check_worktree(&mut entries[i], home);
        }
        let before = entries[i].size;
        // Something that ran out of time before is given all the time that is left.
        let limit = match before {
            Some(size) if !size.complete => deadline,
            _ => deadline.min(Instant::now() + per_target),
        };
        let mut size = dir_size(&entries[i].path, limit, &sizes);
        if let Some(old) = before {
            // Two lower bounds: the larger one says more.
            if !size.complete && !old.complete && old.bytes > size.bytes {
                size = old;
            }
        }
        sizes.insert(entries[i].path.clone(), size);
        entries[i].size = Some(size);
        entries[i].measured_at = Some(unix_now());
        reached[i] = true;
        if last_save.elapsed() >= SAVE_EVERY {
            save(&index_of(&entries));
            last_save = Instant::now();
        }
    }

    let index = index_of(&entries);
    let mut report = report_from(home, opts, &index, false);
    report.reused = reused;
    report.outdated = outdated;
    report.pending = (0..entries.len())
        .filter(|&i| (due[i] && !reached[i]) || entries[i].size.map_or(true, |s| !s.complete))
        .count();
    (report, index)
}

/// Put `entries` in the order they should be measured and say which of them need it: those
/// never measured, measured only in part, measured `max_age` or longer ago, or touched since
/// they were measured. `now` is in seconds since 1970.
fn plan_measuring(entries: &mut Vec<Candidate>, max_age: Duration, now: u64) -> Vec<bool> {
    let stale = |c: &Candidate| match (c.size, c.measured_at) {
        (Some(size), Some(at)) => {
            !size.complete
                || now.saturating_sub(at) >= max_age.as_secs()
                || c.last_touch.is_some_and(|t| unix_secs(t) > at)
        }
        _ => true,
    };

    let worktree_touch: HashMap<PathBuf, Option<SystemTime>> = entries
        .iter()
        .filter(|c| c.kind == Kind::Worktree)
        .map(|c| (c.path.clone(), c.last_touch))
        .collect();
    // The worktree a build folder is inside, or the entry's own path.
    let group_of = |c: &Candidate| match (&c.kind, &c.repo) {
        (Kind::Build, Some(repo)) if worktree_touch.contains_key(repo) => repo.clone(),
        _ => c.path.clone(),
    };
    // A worktree's size is built from the sizes of the build folders inside it, so it is
    // measured again whenever one of them is, and after them. Per worktree with such a
    // folder: the latest time any of those folders was measured (0 for never).
    let mut inner_due: HashMap<PathBuf, u64> = HashMap::new();
    for c in entries.iter() {
        if c.kind == Kind::Build && worktree_touch.contains_key(&group_of(c)) && stale(c) {
            let latest = inner_due.entry(group_of(c)).or_insert(0);
            *latest = (*latest).max(c.measured_at.unwrap_or(0));
        }
    }
    let inner = |c: &Candidate| match c.kind {
        Kind::Worktree => inner_due.get(&c.path).copied(),
        _ => None,
    };
    // A folder inside a worktree is not measured again just because the worktree is: its
    // saved size is added in.
    let due_of = |c: &Candidate| stale(c) || inner(c).is_some();

    // Measuring order, for when the budget runs out first. What has never been measured
    // comes first, then what was measured longest ago, so that successive scans work
    // through everything rather than repeating the same start. Within that: tool caches
    // (few and large), then projects from the longest untouched to the most recent, since
    // stale ones are what will actually be deleted. A worktree comes after the build
    // folders inside it that are being measured.
    let unknown_touch = SystemTime::now();
    let order = |c: &Candidate| {
        let group = group_of(c);
        let touched = match worktree_touch.get(&group) {
            Some(worktree) => *worktree,
            None => c.last_touch,
        };
        (
            c.measured_at.unwrap_or(0).max(inner(c).unwrap_or(0)),
            c.kind != Kind::Cache,
            touched.unwrap_or(unknown_touch),
            group,
            c.kind == Kind::Worktree,
        )
    };
    // The sort borrows `entries` whole, so each entry's key is worked out first.
    let mut keyed: Vec<_> = std::mem::take(entries)
        .into_iter()
        .map(|c| (order(&c), due_of(&c), c))
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    let due = keyed.iter().map(|(_, due, _)| *due).collect();
    *entries = keyed.into_iter().map(|(_, _, c)| c).collect();
    due
}

/// One measured place on its way into a report: the listing row, and what decides which
/// bundle it belongs to.
struct Row {
    target: Target,
    /// The main checkout it belongs to: a worktree's repository, a build folder's
    /// repository (its worktree's, when it is inside one). `None` for tool caches.
    project: Option<PathBuf>,
    /// The worktree a build folder is inside, if that worktree was measured too.
    inside: Option<PathBuf>,
    staleness: Staleness,
}

/// "3 worktrees, 4 build folders": what a bundle is made of.
fn contents_text(members: &[&Row]) -> String {
    let count = |kind: Kind| members.iter().filter(|m| m.target.kind == kind).count();
    let part = |n: usize, one: &str, many: &str| match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {many}")),
    };
    [
        part(count(Kind::Worktree), "worktree", "worktrees"),
        part(count(Kind::Build), "build folder", "build folders"),
        part(count(Kind::Cache), "cache", "caches"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ")
}

/// Group the rows that have gone untouched for a week or more into bundles, one per
/// project and age band, and return them with the rows left over. A bundle smaller than
/// the size floor is not made; its rows are handed back to be listed on their own.
fn bundle_stale(rows: Vec<Row>, home: &Path, min_bytes: u64) -> (Vec<Bundle>, Vec<Row>) {
    let mut groups: HashMap<(Staleness, Option<PathBuf>), Vec<Row>> = HashMap::new();
    let mut alone = Vec::new();
    for row in rows {
        if row.staleness == Staleness::InUse {
            alone.push(row);
        } else {
            groups
                .entry((row.staleness, row.project.clone()))
                .or_default()
                .push(row);
        }
    }

    let mut bundles = Vec::new();
    for ((staleness, project), mut rows) in groups {
        rows.sort_by(|a, b| {
            (b.target.freed_bytes, b.target.bytes, &a.target.path)
                .cmp(&(a.target.freed_bytes, a.target.bytes, &b.target.path))
        });
        let worktrees: HashSet<&PathBuf> = rows
            .iter()
            .filter(|r| r.target.kind == Kind::Worktree)
            .map(|r| &r.target.path)
            .collect();
        let goes_now = |r: &Row| r.target.deletable && r.target.safety == Safety::Regenerates;
        // Worktrees that would be deleted take the build folders inside them along.
        let going: HashSet<&PathBuf> = rows
            .iter()
            .filter(|r| r.target.kind == Kind::Worktree && goes_now(r))
            .map(|r| &r.target.path)
            .collect();
        // A build folder inside a worktree of this bundle is part of that worktree's size.
        let top_level: Vec<&Row> = rows
            .iter()
            .filter(|r| !r.inside.as_ref().is_some_and(|w| worktrees.contains(w)))
            .collect();
        let delete: Vec<&Row> = rows
            .iter()
            .filter(|r| goes_now(r) && !r.inside.as_ref().is_some_and(|w| going.contains(w)))
            .collect();
        let bytes: u64 = top_level.iter().map(|r| r.target.bytes).sum();
        if bytes < min_bytes {
            alone.extend(rows);
            continue;
        }
        // A folder inside a worktree is dated by the worktree, as it is banded by it.
        let ages: Vec<u64> = top_level.iter().filter_map(|r| r.target.age_days).collect();
        let delete_paths: Vec<PathBuf> = delete.iter().map(|r| r.target.path.clone()).collect();
        // Shown: everything counted, and anything inside a kept worktree that can still go.
        let members: Vec<Target> = rows
            .iter()
            .filter(|r| {
                !r.inside.as_ref().is_some_and(|w| worktrees.contains(w)) || delete_paths.contains(&r.target.path)
            })
            .map(|r| r.target.clone())
            .collect();
        let mut bundle = Bundle {
            name: match &project {
                Some(project) => display_path(project, home),
                None => "Tool caches".to_string(),
            },
            is_project: project.is_some(),
            staleness,
            heading: staleness.heading().to_string(),
            detail: String::new(),
            contents: contents_text(&top_level),
            bytes,
            freed_bytes: top_level.iter().map(|r| r.target.freed_bytes).sum(),
            regenerates_bytes: top_level
                .iter()
                .filter(|r| r.target.safety == Safety::Regenerates)
                .map(|r| r.target.bytes)
                .sum(),
            regenerates_freed_bytes: top_level
                .iter()
                .filter(|r| r.target.safety == Safety::Regenerates)
                .map(|r| r.target.freed_bytes)
                .sum(),
            oldest_days: ages.iter().copied().max().unwrap_or(0),
            newest_days: ages.iter().copied().min().unwrap_or(0),
            deletable_freed_bytes: delete.iter().map(|r| r.target.freed_bytes).sum(),
            held_back: top_level
                .iter()
                .filter(|r| !delete_paths.contains(&r.target.path))
                .count(),
            delete_paths,
            members,
        };
        bundle.detail = bundle.detail_text();
        bundles.push(bundle);
    }
    // The longest untouched first; among those, what frees the most right now, then what
    // would free the most once its held-back folders have been looked at.
    bundles.sort_by(|a, b| {
        (a.staleness, b.deletable_freed_bytes, b.freed_bytes, b.bytes, &a.name).cmp(&(
            b.staleness,
            a.deletable_freed_bytes,
            a.freed_bytes,
            a.bytes,
            &b.name,
        ))
    });
    (bundles, alone)
}

/// The listing for what `index` holds: filtered, bundled, sorted and totalled as `opts` asks.
fn report_from(home: &Path, opts: &Options, index: &Index, from_cache: bool) -> Report {
    let worktrees: HashMap<&PathBuf, &Candidate> = index
        .entries
        .iter()
        .filter(|c| c.kind == Kind::Worktree)
        .map(|c| (&c.path, c))
        .collect();

    let mut rows = Vec::new();
    let mut unmeasured = 0usize;
    for c in &index.entries {
        let Some(size) = c.size else {
            unmeasured += 1;
            continue;
        };
        let remove = c
            .remove
            .clone()
            .unwrap_or_else(|| format!("rm -rf {}", shell_path(&c.path, home)));
        // The worktree a build folder is inside, if that worktree has been measured.
        let inside = match (&c.kind, &c.repo) {
            (Kind::Build, Some(repo)) => worktrees
                .get(repo)
                .filter(|w| w.size.is_some())
                .map(|w| w.path.clone()),
            _ => None,
        };
        let project = match c.kind {
            Kind::Cache => None,
            Kind::Worktree => c.repo.clone(),
            Kind::Build => match c.repo.as_ref().and_then(|repo| worktrees.get(repo)) {
                Some(worktree) => worktree.repo.clone(),
                None => c
                    .repo
                    .clone()
                    .or_else(|| c.path.parent().map(Path::to_path_buf)),
            },
        };
        let age = age_days(c.last_touch);
        // A build folder is as stale as the worktree around it, so the two stay together.
        let staleness = match inside.as_ref().and_then(|w| worktrees.get(w)) {
            Some(worktree) => Staleness::of(age_days(worktree.last_touch)),
            None => Staleness::of(age),
        };
        rows.push(Row {
            target: Target {
                display_path: display_path(&c.path, home),
                kind: c.kind,
                what: c.what.clone(),
                cost: c.cost.clone(),
                remove,
                safety: c.safety,
                bytes: size.bytes,
                freed_bytes: size.freed.min(size.bytes),
                complete: size.complete,
                measured_at: c.measured_at,
                age_days: age,
                repo: c.repo.as_deref().map(|r| display_path(r, home)),
                branch: c.branch.clone(),
                note: c.note.clone(),
                deletable: c.deletable,
                path: c.path.clone(),
            },
            project,
            inside,
            staleness,
        });
    }

    if opts.older_than_days > 0 {
        rows.retain(|r| {
            r.target.kind == Kind::Cache
                || r.target.age_days.is_some_and(|d| d >= opts.older_than_days)
        });
    }

    let (mut bundles, alone) = bundle_stale(rows, home, opts.min_bytes);

    // Everything not bundled is listed on its own, by what deleting gives back; among
    // equals, by what is on disk.
    let worktree_paths: Vec<&PathBuf> = worktrees.keys().copied().collect();
    let mut targets: Vec<Target> = alone.into_iter().map(|r| r.target).collect();
    targets.retain(|t| t.bytes >= opts.min_bytes);
    targets.sort_by(|a, b| (b.freed_bytes, b.bytes).cmp(&(a.freed_bytes, a.bytes)));

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
    let sum = |s: Safety, of: fn(&Target) -> u64| -> u64 {
        targets
            .iter()
            .filter(counted_once)
            .filter(|t| t.safety == s)
            .map(of)
            .sum()
    };
    let on_disk: fn(&Target) -> u64 = |t| t.bytes;
    let freed: fn(&Target) -> u64 = |t| t.freed_bytes;
    let bundled = |of: fn(&Bundle) -> u64| -> u64 { bundles.iter().map(of).sum() };
    let regenerates_bytes = sum(Safety::Regenerates, on_disk) + bundled(|b| b.regenerates_bytes);
    let review_bytes = sum(Safety::Review, on_disk) + bundled(|b| b.bytes - b.regenerates_bytes);
    let regenerates_freed_bytes =
        sum(Safety::Regenerates, freed) + bundled(|b| b.regenerates_freed_bytes);
    let review_freed_bytes =
        sum(Safety::Review, freed) + bundled(|b| b.freed_bytes - b.regenerates_freed_bytes);
    targets.truncate(opts.top.max(1));
    bundles.truncate(opts.top.max(1));

    let (volume, used_percent, available_gb) =
        crate::sampler::disk::sample_volume_containing(home)
            .map(|(v, _total, avail, used)| (v, used, avail))
            .unwrap_or_else(|| (home.to_string_lossy().to_string(), 0.0, 0.0));

    Report {
        volume,
        used_percent,
        available_gb,
        bundles,
        targets,
        unmeasured,
        repos_scanned: index.repos_scanned,
        regenerates_bytes,
        review_bytes,
        regenerates_freed_bytes,
        review_freed_bytes,
        discovery_complete: index.discovery_complete,
        scanned_at: index.scanned_at,
        from_cache,
        reused: 0,
        outdated: 0,
        pending: index
            .entries
            .iter()
            .filter(|c| c.size.map_or(true, |s| !s.complete))
            .count(),
    }
}

/// Survey `home`, starting from the index in `index_file` and writing it back. Two scans
/// never write at once: one that finds another already writing still scans, unless
/// `only_as_writer`, when it returns `None` and leaves the other to it.
fn survey_saved(home: &Path, opts: &Options, index_file: &Path, only_as_writer: bool) -> Option<Report> {
    let lock = IndexLock::try_take(index_file);
    if lock.is_none() && only_as_writer {
        return None;
    }
    let prior = load_index(index_file, home);
    let mut save = |index: &Index| {
        if lock.is_some() {
            // Best effort: a scan that cannot save is still a scan.
            let _ = save_index(index_file, index);
        }
    };
    let (report, index) = survey_with(home, opts, prior, &mut save);
    save(&index);
    Some(report)
}

/// The last saved scan of `home`, without looking at the disk beyond checking that each
/// place still exists. `None` when there is no saved scan.
fn cached_at(home: &Path, opts: &Options, index_file: &Path) -> Option<Report> {
    let mut index = load_index(index_file, home)?;
    index.entries.retain(|c| is_real_dir(&c.path));
    Some(report_from(home, opts, &index, true))
}

/// `(used percent, GB available)` on the volume holding `home`.
pub fn volume_usage(home: &Path) -> Option<(f64, f64)> {
    crate::sampler::disk::sample_volume_containing(home).map(|(_, _, avail, used)| (used, avail))
}

/// Survey the current user's home directory at throttled I/O priority, measuring only what
/// the saved index has no current size for, and save what was learned.
pub fn survey(opts: &Options) -> Option<Report> {
    let home = dirs::home_dir()?;
    throttle_io();
    survey_saved(&home, opts, &index_path(), false)
}

/// What a background refresh did.
pub enum Refresh {
    Done(Report),
    /// Another scan was already running; nothing was done.
    Busy,
}

/// Bring the saved index up to date. For running unattended: gives way to a scan that is
/// already under way.
pub fn refresh(opts: &Options) -> Option<Refresh> {
    let home = dirs::home_dir()?;
    throttle_io();
    Some(match survey_saved(&home, opts, &index_path(), true) {
        Some(report) => Refresh::Done(report),
        None => Refresh::Busy,
    })
}

/// The last saved scan, at once and without scanning. `None` when nothing has been saved.
pub fn cached(opts: &Options) -> Option<Report> {
    cached_at(&dirs::home_dir()?, opts, &index_path())
}

impl Target {
    /// What deleting it would free, as text; `+` marks a lower bound.
    pub fn size_text(&self) -> String {
        if self.complete {
            human_size(self.freed_bytes)
        } else {
            format!("{}+", human_size(self.freed_bytes))
        }
    }

    /// Said when a good part of what it occupies would not come back: its files are clones
    /// of files elsewhere (a bun or pnpm install and the cache it came from), are held by a
    /// snapshot, or have other names outside it.
    pub fn shared_text(&self) -> Option<String> {
        let shared = self.bytes.saturating_sub(self.freed_bytes);
        if shared < (self.bytes / 10).max(10 * 1024 * 1024) {
            return None;
        }
        Some(format!(
            "{} in size, of which {} is shared with other copies and freed only when they go too",
            human_size(self.bytes),
            human_size(shared)
        ))
    }

    /// "untouched 34 days", or nothing for tool caches.
    pub fn age_text(&self) -> Option<String> {
        let days = self.age_days?;
        // A cache's date is when its top level last changed, which is not when it was
        // last read.
        Some(match (self.kind, days) {
            (Kind::Cache, 0) => "added to today".to_string(),
            (Kind::Cache, 1) => "nothing new for 1 day".to_string(),
            (Kind::Cache, n) => format!("nothing new for {n} days"),
            (_, 0) => "used today".to_string(),
            (_, 1) => "untouched 1 day".to_string(),
            (_, n) => format!("untouched {n} days"),
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
        if self.note.is_none() && (details.is_empty() || self.kind == Kind::Cache) {
            details.push(self.cost.clone());
        }
        line.push_str(&format!("\n             {}", details.join("; ")));
        if let Some(shared) = self.shared_text() {
            line.push_str(&format!("\n             {shared}"));
        }
        line
    }
}

impl Bundle {
    /// "untouched 35 to 56 days"; for tool caches, "nothing new for 35 to 56 days".
    pub fn ages_text(&self) -> String {
        let lead = if self.is_project { "untouched" } else { "nothing new for" };
        if self.oldest_days == self.newest_days {
            format!("{lead} {} days", self.oldest_days)
        } else {
            format!("{lead} {} to {} days", self.newest_days, self.oldest_days)
        }
    }

    /// How stale it is and how much of it can go without a look.
    pub fn detail_text(&self) -> String {
        let held = match self.held_back {
            0 => String::new(),
            1 => "; 1 needs a look or its own command".to_string(),
            n => format!("; {n} need a look or their own command"),
        };
        match self.delete_paths.len() {
            0 => format!("{}; nothing in it can be deleted without a look or its own command", self.ages_text()),
            n => format!(
                "{}; {} in {n} folder{} can be deleted now{held}",
                self.ages_text(),
                human_size(self.deletable_freed_bytes),
                if n == 1 { "" } else { "s" }
            ),
        }
    }
}

impl Report {
    /// True when another scan would find or measure more: places are pending, or the
    /// search for repositories was cut short.
    pub fn work_left(&self) -> bool {
        self.pending > 0 || !self.discovery_complete
    }

    pub fn render(&self) -> String {
        let mut out = format!(
            "Disk: {:.1}% used, {:.1} GB free on {}\nScanned {} git repositor{} and the usual tool caches.\n\n",
            self.used_percent,
            self.available_gb,
            self.volume,
            self.repos_scanned,
            if self.repos_scanned == 1 { "y" } else { "ies" }
        );
        if self.from_cache {
            out.push_str(&format!(
                "From the scan saved {}; `guardian disk` without --cached brings it up to date.\n\n",
                ago_text(unix_now().saturating_sub(self.scanned_at))
            ));
        }
        if self.targets.is_empty() && self.bundles.is_empty() {
            out.push_str("Nothing large enough to list was found.\n");
        } else {
            out.push_str("Stale things first, bundled by project; then the rest, largest first. Sizes are what deleting would free. This listing deletes nothing; `guardian disk clean` offers to.\n\n");
        }
        let mut heading = None;
        for b in &self.bundles {
            if heading != Some(b.staleness) {
                if heading.is_some() {
                    out.push('\n');
                }
                out.push_str(&format!("{}\n", b.staleness.heading()));
                heading = Some(b.staleness);
            }
            out.push_str(&format!(
                "  {:>9}  {}: {}\n             {}\n",
                human_size(b.freed_bytes),
                b.name,
                b.contents,
                b.detail_text()
            ));
            for m in b.members.iter().take(3) {
                out.push_str(&format!(
                    "             {:>9}  {} ({})\n",
                    m.size_text(),
                    m.what,
                    m.display_path
                ));
            }
            if b.members.len() > 3 {
                out.push_str(&format!("                        and {} more\n", b.members.len() - 3));
            }
            if !b.delete_paths.is_empty() {
                out.push_str("             $ guardian disk clean    (offers this bundle as one choice)\n");
            }
        }
        if heading.is_some() {
            out.push('\n');
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
                human_size(self.regenerates_freed_bytes),
                human_size(self.review_freed_bytes)
            ));
        }
        let shared = (self.regenerates_bytes + self.review_bytes)
            .saturating_sub(self.regenerates_freed_bytes + self.review_freed_bytes);
        if shared >= 100 * 1024 * 1024 {
            out.push_str(&format!(
                "Folder sizes add up to {} more than that: those files are shared between copies (cloned dependency folders and the cache they came from) and are freed only when every copy is deleted.\n",
                human_size(shared)
            ));
        }
        let listed = self.targets.iter().chain(self.bundles.iter().flat_map(|b| &b.members));
        if listed.into_iter().any(|t| !t.complete) {
            out.push_str("Sizes ending in + ran out of time and are lower bounds.\n");
        }
        if self.unmeasured > 0 {
            out.push_str(&format!(
                "{} more recently used places were not measured in time (oldest are measured first); run it again to carry on from here, or give it longer with --seconds 120.\n",
                self.unmeasured
            ));
        }
        if self.outdated > 0 {
            out.push_str(&format!(
                "{} sizes are from an earlier scan and were not measured again in time; run it again to carry on.\n",
                self.outdated
            ));
        }
        if !self.discovery_complete {
            out.push_str("The search for repositories ran out of time, so there may be more to find; give it longer with --seconds 120.\n");
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
    // What the delete will give back, not what the folder occupies: clones of files kept
    // elsewhere free nothing.
    let freed_bytes = dir_size(path, Instant::now() + Duration::from_secs(20), &HashMap::new()).freed;
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

/// Parse an answer to "delete which?" about a numbered list of `count` choices: numbers
/// (`1 3 5`, `2,4`, `1-3`), or `old` for the choices at the indices in `old`.
/// `Ok(None)` means nothing was chosen.
pub fn parse_selection(input: &str, count: usize, old: &[usize]) -> Result<Option<Vec<usize>>, String> {
    let input = input.trim().to_lowercase();
    if input.is_empty() || input == "n" || input == "no" || input == "q" {
        return Ok(None);
    }
    if input == "old" {
        return Ok((!old.is_empty()).then(|| old.to_vec()));
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
                .filter(|n| (1..=count).contains(n))
                .ok_or_else(|| format!("`{part}` is not a number from 1 to {count}"))
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

        /// Make `rel` look last changed `days` ago: the folder and what is directly in it.
        fn age(&self, rel: &str, days: u64) {
            let when = SystemTime::now() - Duration::from_secs(days * 86_400 + 3_600);
            let dir = self.path(rel);
            let set = |path: &Path| {
                if fs::symlink_metadata(path).is_ok_and(|m| !m.file_type().is_symlink()) {
                    fs::File::open(path).unwrap().set_modified(when).unwrap();
                }
            };
            if dir.is_dir() {
                for entry in fs::read_dir(&dir).unwrap().flatten() {
                    set(&entry.path());
                }
            }
            set(&dir);
        }

        /// Make the checkout at `rel` look last worked in `days` ago. `git_dir` is its git
        /// directory, relative to the scratch home: `<rel>/.git`, or for a linked worktree
        /// `<main>/.git/worktrees/<name>`.
        fn age_checkout(&self, rel: &str, git_dir: &str, days: u64) {
            self.age(&format!("{git_dir}/logs"), days);
            self.age(git_dir, days);
            self.age(rel, days);
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

    fn none() -> HashMap<PathBuf, Size> {
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
        let Size { bytes, freed, complete } = settled(&s.0, |size| size.freed == size.bytes);
        assert!(complete);
        assert_eq!(freed, bytes, "nothing here is shared");
        assert!((500_000..2_000_000).contains(&bytes), "{bytes}");
    }

    #[test]
    fn missing_path_is_zero() {
        assert_eq!(
            dir_size(Path::new("/nonexistent/guardian/xyz"), far(), &none()),
            Size { bytes: 0, freed: 0, complete: true }
        );
    }

    #[test]
    fn symlinks_are_not_followed_when_measuring() {
        let s = Scratch::new("symlink");
        s.file("big/data.bin", 1_000_000);
        s.file("small/note.txt", 10);
        std::os::unix::fs::symlink(s.path("big"), s.path("small/link")).unwrap();
        let bytes = dir_size(&s.path("small"), far(), &none()).bytes;
        assert!(bytes < 500_000, "{bytes}");
    }

    #[test]
    fn already_measured_folders_are_reused_not_walked() {
        let s = Scratch::new("reuse");
        s.file("wt/src/a.rs", 1_000);
        s.file("wt/node_modules/dep/x.js", 500_000);
        let mut known = none();
        known.insert(s.path("wt/node_modules"), Size { bytes: 7_000_000, freed: 3_000_000, complete: false });
        let Size { bytes, freed, complete } = dir_size(&s.path("wt"), far(), &known);
        assert!((7_000_000..7_200_000).contains(&bytes), "{bytes}");
        assert!((3_000_000..3_200_000).contains(&freed), "{freed}");
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
        assert_eq!(npm.age_text().as_deref(), Some("added to today"));
        let ollama = find(&report, "~/.ollama/models");
        assert_eq!(ollama.safety, Safety::Review);
        assert!(!ollama.deletable);
        assert_eq!(ollama.age_days, None, "a model is not stale for having been downloaded long ago");
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
            freed_bytes: 1,
            complete: true,
            measured_at: None,
            age_days: age,
            repo: None,
            branch: None,
            note: None,
            deletable,
        }
    }

    #[test]
    fn selection_parsing() {
        // Five choices; the first and fourth are what `old` stands for.
        let pick = |input: &str| parse_selection(input, 5, &[0, 3]);
        assert_eq!(pick(""), Ok(None));
        assert_eq!(pick("  n "), Ok(None));
        assert_eq!(pick("1 3"), Ok(Some(vec![0, 2])));
        assert_eq!(pick("3,1,1"), Ok(Some(vec![0, 2])));
        assert_eq!(pick("2-4"), Ok(Some(vec![1, 2, 3])));
        assert_eq!(pick("OLD"), Ok(Some(vec![0, 3])));
        assert_eq!(parse_selection("old", 5, &[]), Ok(None), "nothing old to pick");
        assert!(pick("0").is_err());
        assert!(pick("6").is_err());
        assert!(pick("4-2").is_err());
        assert!(pick("all").is_err());
        assert!(pick("y").is_err(), "a stray yes selects nothing");
    }

    #[test]
    fn old_and_safe_is_two_weeks_untouched_and_not_a_shared_cache() {
        let old = |age, safety, kind, deletable| target(age, safety, kind, deletable).is_old_and_safe();
        assert!(old(Some(40), Safety::Regenerates, Kind::Build, true));
        assert!(old(Some(30), Safety::Regenerates, Kind::Worktree, true));
        assert!(!old(Some(2), Safety::Regenerates, Kind::Build, true));
        assert!(!old(Some(90), Safety::Review, Kind::Worktree, true));
        assert!(!old(Some(90), Safety::Regenerates, Kind::Build, false));
        assert!(!old(Some(90), Safety::Regenerates, Kind::Cache, true));
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

    // --- stale things, bundled ---

    fn bundle<'a>(report: &'a Report, name: &str, staleness: Staleness) -> &'a Bundle {
        report
            .bundles
            .iter()
            .find(|b| b.name == name && b.staleness == staleness)
            .unwrap_or_else(|| {
                panic!(
                    "no {staleness:?} bundle {name} in {:?}",
                    report.bundles.iter().map(|b| (&b.name, b.staleness)).collect::<Vec<_>>()
                )
            })
    }

    #[test]
    fn ages_fall_into_bands() {
        assert_eq!(Staleness::of(None), Staleness::InUse);
        assert_eq!(Staleness::of(Some(6)), Staleness::InUse);
        assert_eq!(Staleness::of(Some(7)), Staleness::Week);
        assert_eq!(Staleness::of(Some(13)), Staleness::Week);
        assert_eq!(Staleness::of(Some(14)), Staleness::Fortnight);
        assert_eq!(Staleness::of(Some(30)), Staleness::Month);
        assert_eq!(Staleness::of(Some(89)), Staleness::Month);
        assert_eq!(Staleness::of(Some(90)), Staleness::Quarter);
        assert!(Staleness::Quarter < Staleness::Month && Staleness::Week < Staleness::InUse);
        assert!(Staleness::Fortnight.is_old() && !Staleness::Week.is_old());
    }

    #[test]
    fn stale_folders_are_bundled_by_project_the_longest_untouched_first() {
        let s = Scratch::new("bundles");
        // Untouched for two months: two folders, each too small to list on its own.
        s.repo("code/old-app");
        s.file("code/old-app/web/package.json", 10);
        s.file("code/old-app/web/node_modules/x.js", 300_000);
        s.file("code/old-app/api/node_modules/x.js", 350_000);
        s.age("code/old-app/web/node_modules", 60);
        s.age("code/old-app/api/node_modules", 75);
        // A folder is as fresh as the latest sign of work in its repository.
        s.age_checkout("code/old-app", "code/old-app/.git", 75);
        // Untouched for three weeks.
        s.repo("code/mid-app");
        s.file("code/mid-app/Cargo.toml", 10);
        s.file("code/mid-app/target/debug/app", 700_000);
        s.age("code/mid-app/target", 20);
        s.age_checkout("code/mid-app", "code/mid-app/.git", 20);
        // In use.
        s.repo("code/live-app");
        s.file("code/live-app/node_modules/x.js", 900_000);
        // Tool caches nothing has been added to for six weeks: one Guardian deletes, one
        // with a cleaner of its own.
        s.file("Library/Caches/ms-playwright/chromium/bin", 800_000);
        s.age("Library/Caches/ms-playwright", 45);
        s.file(".cache/uv/blob", 600_000);
        s.age(".cache/uv", 45);

        let opts = Options {
            min_bytes: 500_000,
            top: 50,
            ..Options::default()
        };
        let report = survey_home(&s.0, &opts);
        let order: Vec<(&str, Staleness)> = report.bundles.iter().map(|b| (b.name.as_str(), b.staleness)).collect();
        assert_eq!(
            order,
            vec![
                ("Tool caches", Staleness::Month),
                ("~/code/old-app", Staleness::Month),
                ("~/code/mid-app", Staleness::Fortnight),
            ],
            "longest untouched first; within a band, what frees the most"
        );
        assert_eq!(listed(&report), vec!["~/code/live-app/node_modules"], "what is in use is listed on its own");

        let old = bundle(&report, "~/code/old-app", Staleness::Month);
        assert_eq!(old.contents, "2 build folders");
        assert_eq!((old.newest_days, old.oldest_days), (60, 75));
        assert_eq!(old.delete_paths.len(), 2);
        assert_eq!(old.held_back, 0);
        assert!(old.is_project);
        assert!(old.freed_bytes >= 650_000 && old.freed_bytes == old.deletable_freed_bytes);
        assert_eq!(old.members[0].display_path, "~/code/old-app/api/node_modules", "largest first");
        assert_eq!(old.ages_text(), "untouched 60 to 75 days");

        let caches = bundle(&report, "Tool caches", Staleness::Month);
        assert!(!caches.is_project);
        assert_eq!(caches.contents, "2 caches");
        assert_eq!(caches.delete_paths, vec![s.path("Library/Caches/ms-playwright")]);
        assert_eq!(caches.held_back, 1, "uv has its own cleaner");
        assert_eq!(caches.ages_text(), "nothing new for 45 days");

        // The totals cover bundles and single folders alike.
        assert!(report.regenerates_bytes >= 300_000 + 350_000 + 700_000 + 900_000 + 800_000 + 600_000);

        let text = report.render();
        let month = text.find("Untouched 30 to 89 days").unwrap();
        let fortnight = text.find("Untouched 14 to 29 days").unwrap();
        let single = text.find("Comes back on its own").unwrap();
        assert!(month < fortnight && fortnight < single, "{text}");
        assert!(text.contains("~/code/old-app: 2 build folders"), "{text}");
        assert!(text.contains("in 2 folders can be deleted now"), "{text}");
        assert!(text.contains("1 needs a look or its own command"), "{text}");
    }

    #[test]
    fn a_bundle_too_small_to_list_hands_its_folders_back() {
        let s = Scratch::new("bundle-floor");
        s.file("code/app/node_modules/x.js", 300_000);
        s.age("code/app/node_modules", 40);
        let big_floor = Options {
            min_bytes: 5_000_000,
            ..small_floor()
        };
        let report = survey_home(&s.0, &big_floor);
        assert!(report.bundles.is_empty() && report.targets.is_empty());
        // With a floor it clears, it is a bundle of one.
        let report = survey_home(&s.0, &small_floor());
        assert_eq!(report.bundles.len(), 1);
        assert_eq!(report.bundles[0].contents, "1 build folder");
        assert!(report.targets.is_empty());
    }

    #[test]
    fn a_stale_worktree_is_bundled_with_what_is_inside_it() {
        let s = Scratch::new("bundle-worktrees");
        s.repo("code/app");
        fs::write(s.path("code/app/.gitignore"), "node_modules\n").unwrap();
        s.git("code/app", &["add", ".gitignore"]);
        s.git("code/app", &["commit", "-q", "-m", "ignore"]);
        s.git("code/app", &["worktree", "add", "-q", "-b", "done", "../app-wt-done"]);
        s.git("code/app", &["worktree", "add", "-q", "-b", "wip", "../app-wt-wip"]);
        s.file("code/app-wt-done/node_modules/dep/x.js", 400_000);
        s.file("code/app-wt-wip/node_modules/dep/x.js", 500_000);
        s.file("code/app-wt-wip/notes.txt", 100);
        s.age_checkout("code/app-wt-done", "code/app/.git/worktrees/app-wt-done", 20);
        s.age_checkout("code/app-wt-wip", "code/app/.git/worktrees/app-wt-wip", 22);

        let report = survey_home(&s.0, &small_floor());
        let b = bundle(&report, "~/code/app", Staleness::Fortnight);
        assert_eq!(b.contents, "2 worktrees", "the folders inside them are part of them");
        // The clean worktree goes whole. The one with unsaved work stays, but the
        // dependencies inside it can still go.
        assert_eq!(
            b.delete_paths,
            vec![s.path("code/app-wt-wip/node_modules"), s.path("code/app-wt-done")]
        );
        assert_eq!(b.held_back, 1);
        let shown: Vec<&str> = b.members.iter().map(|m| m.display_path.as_str()).collect();
        assert_eq!(shown, vec!["~/code/app-wt-wip", "~/code/app-wt-wip/node_modules", "~/code/app-wt-done"]);
        assert!(b.freed_bytes >= 900_000, "both worktrees, each counted once: {}", b.freed_bytes);
        assert!(b.freed_bytes < 1_400_000, "{}", b.freed_bytes);
    }

    #[test]
    fn time_since_a_scan_reads_naturally() {
        assert_eq!(ago_text(30), "just now");
        assert_eq!(ago_text(90), "2 minutes ago");
        assert_eq!(ago_text(3_600), "60 minutes ago");
        assert_eq!(ago_text(7_200), "2 hours ago");
        assert_eq!(ago_text(3 * 86_400), "3 days ago");
    }

    // --- space shared with other copies ---

    /// Measure `path`, again if need be until `want` holds or a few seconds pass. Seen once
    /// in a few dozen runs of these tests: a file written a moment ago, with many other
    /// tests writing at the same time, briefly reports none of its space as its own.
    fn settled(path: &Path, want: impl Fn(&Size) -> bool) -> Size {
        let mut size = dir_size(path, far(), &none());
        for _ in 0..60 {
            if want(&size) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
            size = dir_size(path, far(), &none());
        }
        size
    }

    /// Clone `from` to `to` the way bun and pnpm install from their caches. False on a
    /// volume that cannot clone (anything but APFS).
    fn clone_file(from: &Path, to: &Path) -> bool {
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        Command::new("cp")
            .arg("-c")
            .arg(from)
            .arg(to)
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|st| st.success())
    }

    #[test]
    fn a_file_with_two_names_counts_once_and_frees_only_if_both_are_inside() {
        let s = Scratch::new("links");
        s.file("tree/a.bin", 400_000);
        fs::hard_link(s.path("tree/a.bin"), s.path("tree/a-again.bin")).unwrap();
        s.file("tree/b.bin", 300_000);
        fs::hard_link(s.path("tree/b.bin"), s.path("b-elsewhere.bin")).unwrap();
        let size = settled(&s.path("tree"), |size| size.freed >= 400_000);
        assert!(size.complete);
        assert!((700_000..800_000).contains(&size.bytes), "a and b, once each: {}", size.bytes);
        assert!((400_000..500_000).contains(&size.freed), "only a comes back: {}", size.freed);
    }

    #[test]
    fn cloned_dependencies_are_sized_by_what_deleting_them_frees() {
        let s = Scratch::new("clones");
        s.file("pkg-cache/big.bin", 12_000_000);
        if !clone_file(&s.path("pkg-cache/big.bin"), &s.path("code/web/node_modules/big.bin")) {
            eprintln!("skipped: this volume cannot clone files");
            return;
        }
        s.file("code/web/node_modules/own.js", 500_000);

        settled(&s.path("code/web/node_modules"), |size| size.freed >= 500_000);
        let report = survey_home(&s.0, &small_floor());
        let t = find(&report, "~/code/web/node_modules");
        assert!(t.bytes >= 12_500_000, "{}", t.bytes);
        assert!((500_000..700_000).contains(&t.freed_bytes), "{}", t.freed_bytes);
        assert_eq!(report.regenerates_freed_bytes, t.freed_bytes);
        assert!(report.regenerates_bytes >= 12_500_000);
        let shared = t.shared_text().expect("most of it is shared");
        assert!(shared.contains("shared with other copies"), "{shared}");
        assert!(report.render().contains("shared with other copies"));

        let deleted = delete_target(&s.path("code/web/node_modules"), &s.0).unwrap();
        assert!(deleted.freed_bytes < 700_000, "the clone frees nothing: {}", deleted.freed_bytes);
        assert!(s.path("pkg-cache/big.bin").exists());
    }

    #[test]
    fn a_folder_that_shares_nothing_says_nothing_about_sharing() {
        let mut t = target(Some(1), Safety::Regenerates, Kind::Build, true);
        t.bytes = 500 * 1024 * 1024;
        t.freed_bytes = t.bytes;
        assert_eq!(t.shared_text(), None);
        // A little sharing is not worth a line.
        t.freed_bytes = t.bytes - 5 * 1024 * 1024;
        assert_eq!(t.shared_text(), None);
        t.freed_bytes = 0;
        assert_eq!(t.size_text(), "0 MB");
        assert!(t.shared_text().unwrap().starts_with("500 MB in size, of which 500 MB is shared"));
    }

    // --- the saved index ---

    fn index_file(s: &Scratch) -> PathBuf {
        s.path(".guardian/disk_index.json")
    }

    fn listed(report: &Report) -> Vec<&str> {
        let mut paths: Vec<&str> = report.targets.iter().map(|t| t.display_path.as_str()).collect();
        paths.sort_unstable();
        paths
    }

    /// Make `dir` look modified a few seconds from now: later than any scan so far.
    fn touch_later(dir: &Path) {
        fs::File::open(dir)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
    }

    #[test]
    fn a_second_scan_measures_only_what_changed() {
        let s = Scratch::new("index-reuse");
        s.file(".npm/_cacache/blob", 200_000);
        s.file(".cache/uv/blob", 300_000);
        s.file("code/web/node_modules/x.js", 400_000);
        let index = index_file(&s);

        let first = survey_saved(&s.0, &small_floor(), &index, false).unwrap();
        assert_eq!((first.reused, first.unmeasured, first.outdated), (0, 0, 0));
        assert_eq!(first.targets.len(), 3);
        assert!(first.discovery_complete && !first.from_cache);

        let second = survey_saved(&s.0, &small_floor(), &index, false).unwrap();
        assert_eq!(second.reused, 3, "nothing changed, so nothing is measured again");
        assert_eq!(listed(&second), listed(&first));

        s.file("code/web/node_modules/y.js", 600_000);
        touch_later(&s.path("code/web/node_modules"));
        let third = survey_saved(&s.0, &small_floor(), &index, false).unwrap();
        assert_eq!(third.reused, 2, "only the folder that changed is measured again");
        assert!(find(&third, "~/code/web/node_modules").bytes >= 1_000_000);
        assert_eq!(
            find(&third, "~/.npm/_cacache").measured_at,
            find(&first, "~/.npm/_cacache").measured_at
        );

        let afresh = Options {
            max_age: Duration::ZERO,
            ..small_floor()
        };
        assert_eq!(survey_saved(&s.0, &afresh, &index, false).unwrap().reused, 0);
    }

    #[test]
    fn a_scan_with_no_time_keeps_what_earlier_scans_found() {
        let s = Scratch::new("index-cut");
        s.file(".npm/_cacache/blob", 200_000);
        s.file("code/web/node_modules/x.js", 400_000);
        s.file("code/api/node_modules/x.js", 400_000);
        let index = index_file(&s);
        survey_saved(&s.0, &small_floor(), &index, false).unwrap();

        fs::remove_dir_all(s.path("code/api/node_modules")).unwrap();
        s.file(".cache/uv/blob", 300_000);
        // No time to search the home folder or to measure anything.
        let no_time = Options {
            budget: Duration::ZERO,
            max_age: Duration::ZERO,
            ..small_floor()
        };
        let report = survey_saved(&s.0, &no_time, &index, false).unwrap();
        assert!(!report.discovery_complete);
        assert_eq!(
            listed(&report),
            vec!["~/.npm/_cacache", "~/code/web/node_modules"],
            "found earlier and still there; the deleted folder is dropped"
        );
        assert_eq!(report.unmeasured, 1, "the new cache is known but not yet measured");
        assert_eq!(report.outdated, 2);
        assert_eq!(report.pending, 3);
        assert!(report.work_left());
        let text = report.render();
        assert!(text.contains("2 sizes are from an earlier scan"), "{text}");
        assert!(text.contains("there may be more to find"), "{text}");

        // The next scan with time carries on: it measures the new cache and searches again.
        let report = survey_saved(&s.0, &small_floor(), &index, false).unwrap();
        assert!(report.discovery_complete);
        assert_eq!(
            listed(&report),
            vec!["~/.cache/uv", "~/.npm/_cacache", "~/code/web/node_modules"]
        );
        assert_eq!((report.unmeasured, report.outdated, report.pending), (0, 0, 0));
        assert!(!report.work_left());
    }

    #[test]
    fn never_measured_goes_first_then_the_longest_unmeasured() {
        let now = 1_000_000;
        let at = |secs: u64| UNIX_EPOCH + Duration::from_secs(secs);
        let c = |path: &str, kind: Kind, repo: Option<&str>, measured: Option<u64>, touched: Option<u64>| Candidate {
            path: PathBuf::from(path),
            kind,
            what: String::new(),
            cost: String::new(),
            remove: None,
            safety: Safety::Regenerates,
            repo: repo.map(PathBuf::from),
            last_touch: touched.map(at),
            branch: None,
            note: None,
            deletable: true,
            size: measured.map(|_| Size { bytes: 1, freed: 1, complete: true }),
            measured_at: measured,
            needs_check: false,
        };
        let mut entries = vec![
            c("/h/.cache/recent", Kind::Cache, None, Some(now - 10), None),
            c("/h/.cache/old", Kind::Cache, None, Some(now - 90_000), None),
            c("/h/wt", Kind::Worktree, Some("/h/main"), Some(now - 20), Some(100)),
            c("/h/wt/node_modules", Kind::Build, Some("/h/wt"), None, Some(100)),
            c("/h/new/target", Kind::Build, Some("/h/new"), None, Some(50)),
            c("/h/edited/target", Kind::Build, Some("/h/edited"), Some(now - 30), Some(now - 5)),
            c("/h/settled/target", Kind::Build, Some("/h/settled"), Some(now - 40), Some(now - 500)),
        ];
        let due = plan_measuring(&mut entries, DEFAULT_MAX_AGE, now);
        let planned: Vec<(&str, bool)> = entries
            .iter()
            .map(|c| c.path.to_str().unwrap())
            .zip(due.iter().copied())
            .collect();
        assert_eq!(
            planned,
            vec![
                // Never measured, longest untouched first.
                ("/h/new/target", true),
                ("/h/wt/node_modules", true),
                // Measured longer ago than a saved size is trusted for.
                ("/h/.cache/old", true),
                // Measured recently and untouched since: its size stands.
                ("/h/settled/target", false),
                // Touched after it was measured.
                ("/h/edited/target", true),
                // Measured again because a folder inside it is, and after that folder.
                ("/h/wt", true),
                ("/h/.cache/recent", false),
            ]
        );
    }

    #[test]
    fn a_size_measured_in_part_is_measured_again() {
        let s = Scratch::new("index-part");
        s.file(".npm/_cacache/blob", 200_000);
        let index = index_file(&s);
        survey_saved(&s.0, &small_floor(), &index, false).unwrap();
        let mut saved = load_index(&index, &s.0).unwrap();
        saved.entries[0].size = Some(Size { bytes: 5, freed: 5, complete: false });
        save_index(&index, &saved).unwrap();

        let saved_report = cached_at(&s.0, &small_floor(), &index).unwrap();
        assert_eq!(saved_report.pending, 1, "a size measured in part is work left over");

        let report = survey_saved(&s.0, &small_floor(), &index, false).unwrap();
        assert_eq!((report.reused, report.pending), (0, 0));
        let npm = find(&report, "~/.npm/_cacache");
        assert!(npm.complete && npm.bytes >= 200_000);
    }

    #[test]
    fn only_one_scan_writes_the_index() {
        let s = Scratch::new("index-lock");
        s.file(".npm/_cacache/blob", 200_000);
        let index = index_file(&s);
        let held = IndexLock::try_take(&index).expect("the first to ask gets it");
        assert!(IndexLock::try_take(&index).is_none());

        assert!(
            survey_saved(&s.0, &small_floor(), &index, true).is_none(),
            "a background refresh gives way to a scan already running"
        );
        let report = survey_saved(&s.0, &small_floor(), &index, false).expect("a listing still scans");
        assert_eq!(report.targets.len(), 1);
        assert!(!index.exists(), "but writes nothing while another scan holds the index");

        drop(held);
        // Other tests start git in this same process, and a child being started holds a
        // copy of every open file for an instant, the lock included. A real scan is the
        // only thing its process does, so there the lock goes when the scan does.
        let mut saved = None;
        for _ in 0..100 {
            saved = survey_saved(&s.0, &small_floor(), &index, true);
            if saved.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(saved.is_some(), "once the other scan is done, a refresh runs");
        assert!(index.exists());
    }

    #[test]
    fn cached_listing_reads_the_saved_scan_without_measuring() {
        let s = Scratch::new("index-cached");
        s.file(".npm/_cacache/blob", 200_000);
        s.file("code/web/node_modules/x.js", 400_000);
        let index = index_file(&s);
        assert!(cached_at(&s.0, &small_floor(), &index).is_none(), "nothing saved yet");
        survey_saved(&s.0, &small_floor(), &index, false).unwrap();

        // Neither change is measured; a folder that has gone is simply left out.
        s.file("code/web/node_modules/y.js", 900_000);
        fs::remove_dir_all(s.path(".npm/_cacache")).unwrap();
        let report = cached_at(&s.0, &small_floor(), &index).unwrap();
        assert!(report.from_cache);
        assert_eq!(listed(&report), vec!["~/code/web/node_modules"]);
        assert!(report.targets[0].bytes < 900_000);
        assert!(report.render().contains("From the scan saved just now"));

        // An index for another home directory, or from another version, is not used.
        let elsewhere = Scratch::new("index-cached-other");
        assert!(cached_at(&elsewhere.0, &small_floor(), &index).is_none());
        let mut saved = load_index(&index, &s.0).unwrap();
        saved.version += 1;
        save_index(&index, &saved).unwrap();
        assert!(cached_at(&s.0, &small_floor(), &index).is_none());
        assert!(survey_saved(&s.0, &small_floor(), &index, false).unwrap().reused == 0);
    }

    #[test]
    fn a_worktree_is_checked_again_only_once_it_has_been_touched() {
        let s = Scratch::new("index-worktree");
        s.repo("code/app");
        s.git("code/app", &["worktree", "add", "-q", "-b", "feature", "../app-wt"]);
        s.file("code/app-wt/src/lib.rs", 10);
        s.git("code/app-wt", &["add", "src/lib.rs"]);
        s.git("code/app-wt", &["commit", "-q", "-m", "src"]);
        let index = index_file(&s);

        let first = survey_saved(&s.0, &small_floor(), &index, false).unwrap();
        let wt = find(&first, "~/code/app-wt");
        assert!(wt.deletable && wt.note.as_deref().unwrap().contains("no uncommitted work"), "{:?}", wt.note);

        // A new file in a folder that already existed moves none of the times a scan looks
        // at, so the next scan keeps what git said rather than asking again...
        s.file("code/app-wt/src/scratch.rs", 10);
        let second = survey_saved(&s.0, &small_floor(), &index, false).unwrap();
        assert_eq!(find(&second, "~/code/app-wt").note, wt.note);
        // ...which is why deleting never goes by the listing: it asks git there and then.
        let why = why_not_deletable(&s.path("code/app-wt"), &s.0).expect("unsaved work is found");
        assert!(why.contains("1 uncommitted or untracked file"), "{why}");

        // Once the worktree shows any sign of use, the scan asks git again.
        touch_later(&s.path("code/app-wt"));
        let third = survey_saved(&s.0, &small_floor(), &index, false).unwrap();
        let wt = find(&third, "~/code/app-wt");
        assert!(!wt.deletable);
        assert_eq!(wt.note.as_deref(), Some("1 uncommitted or untracked file"));

        // So does a scan told to trust nothing saved.
        fs::remove_file(s.path("code/app-wt/src/scratch.rs")).unwrap();
        let afresh = Options {
            max_age: Duration::ZERO,
            ..small_floor()
        };
        let fourth = survey_saved(&s.0, &afresh, &index, false).unwrap();
        assert!(find(&fourth, "~/code/app-wt").deletable);
    }

    #[test]
    fn a_worktree_not_yet_checked_is_never_offered() {
        let s = Scratch::new("index-unchecked");
        s.repo("code/app");
        s.git("code/app", &["worktree", "add", "-q", "-b", "feature", "../app-wt"]);
        // Time to find it (two thirds of nothing is still nothing to measure with).
        let found = candidates(&s.0, &small_floor(), far());
        let wt = found
            .candidates
            .iter()
            .find(|c| c.kind == Kind::Worktree)
            .expect("the worktree is listed");
        assert!(found.complete);
        assert!(!wt.deletable && wt.safety == Safety::Review);
        assert_eq!(wt.note.as_deref(), Some(UNCHECKED));
        assert!(wt.needs_check);
    }

    #[test]
    fn a_half_written_index_left_by_a_stopped_scan_is_cleared_away() {
        let s = Scratch::new("index-leftover");
        s.file(".npm/_cacache/blob", 200_000);
        s.file(".guardian/disk_index.json.4242.tmp", 10);
        s.file(".guardian/something-else.tmp", 10);
        survey_saved(&s.0, &small_floor(), &index_file(&s), false).unwrap();
        assert!(!s.path(".guardian/disk_index.json.4242.tmp").exists());
        assert!(s.path(".guardian/something-else.tmp").exists());
        assert!(index_file(&s).exists());
    }

    #[test]
    fn a_path_that_json_cannot_hold_is_left_out_of_the_saved_index() {
        use std::os::unix::ffi::OsStringExt;
        let s = Scratch::new("index-utf8");
        let entry = |path: PathBuf| Candidate {
            path,
            kind: Kind::Build,
            what: String::new(),
            cost: String::new(),
            remove: None,
            safety: Safety::Regenerates,
            repo: None,
            last_touch: None,
            branch: None,
            note: None,
            deletable: true,
            size: Some(Size { bytes: 1, freed: 1, complete: true }),
            measured_at: Some(1),
            needs_check: false,
        };
        let not_utf8 = PathBuf::from(std::ffi::OsString::from_vec(b"/h/caf\xe9/target".to_vec()));
        let index = Index {
            version: INDEX_VERSION,
            home: s.0.clone(),
            scanned_at: 1,
            discovery_complete: true,
            repos_scanned: 0,
            entries: vec![entry(not_utf8), entry(PathBuf::from("/h/plain/target"))],
        };
        save_index(&index_file(&s), &index).unwrap();
        let saved = load_index(&index_file(&s), &s.0).unwrap();
        let paths: Vec<&Path> = saved.entries.iter().map(|c| c.path.as_path()).collect();
        assert_eq!(paths, vec![Path::new("/h/plain/target")]);
    }

    #[test]
    fn a_broken_index_file_is_ignored_and_replaced() {
        let s = Scratch::new("index-broken");
        s.file(".npm/_cacache/blob", 200_000);
        s.file(".guardian/disk_index.json", 0);
        fs::write(index_file(&s), "{ not json").unwrap();
        let report = survey_saved(&s.0, &small_floor(), &index_file(&s), false).unwrap();
        assert_eq!((report.targets.len(), report.reused), (1, 0));
        assert!(load_index(&index_file(&s), &s.0).is_some());
    }
}

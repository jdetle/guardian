//! Saving a worktree's unsaved work so that the worktree can then be removed
//! (`guardian disk save`): commit what is uncommitted, push it, and ask the remote whether
//! it has the commit.
//!
//! It exists because a worktree with uncommitted files is otherwise a dead end in a
//! cleanup: Guardian will not delete it, and saving it by hand means opening each one.
//!
//! The rules it keeps, because it publishes to a remote on the strength of one click:
//!
//! - It never changes a branch that is already on the remote. Such a branch is usually a
//!   pull request, where a new commit runs CI, notifies reviewers and may be merged. The
//!   save goes beside it, as `guardian-save/<branch>`.
//! - It never pushes to a shared branch (the remote's default, `main`, `develop`, …), never
//!   forces, and pushes where the user's own `git push` would (`pushRemote`,
//!   `remote.pushDefault`).
//! - It runs the repository's commit and push hooks, which is where secret scanners live,
//!   unless told not to. On top of that it refuses new files that look like credentials by
//!   name or by content, dependency folders that are not ignored, and anything large.
//! - It commits exactly what it showed: a plan has an id, and a save given that id refuses
//!   if the worktree has changed since.
//! - It refuses rather than guesses when a plain commit would miss something: a merge or
//!   rebase under way, files hidden from git status, a nested repository, a dirty submodule.

use super::{classify_project_dir, git_command, guard_path, ignored_path_regenerates, linked_worktrees, LOCKED};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// The message of the commit a save makes.
pub const COMMIT_MESSAGE: &str = "WIP: saved by Guardian before removing this worktree";

/// A new file larger than this is not committed: hosts reject pushes with files not much
/// larger, and a file this size in a worktree is rarely source.
const MAX_NEW_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// More new files, or more new data, than this is not work in progress: it is a folder
/// that should have been ignored.
const MAX_NEW_FILES: usize = 2_000;
const MAX_NEW_BYTES: u64 = 200 * 1024 * 1024;
/// How much of a new file is read when looking for a private key or a token.
const SCAN_BYTES: u64 = 256 * 1024;

const LOCAL_TIMEOUT: Duration = Duration::from_secs(60);
/// Commit hooks may lint or run tests.
const COMMIT_TIMEOUT: Duration = Duration::from_secs(300);
const REMOTE_TIMEOUT: Duration = Duration::from_secs(45);
const PUSH_TIMEOUT: Duration = Duration::from_secs(180);

/// What saving a worktree would do, worked out without changing anything.
#[derive(Clone, Debug, Serialize)]
pub struct SavePlan {
    pub path: PathBuf,
    /// The local branch the work is committed to.
    pub branch: String,
    /// True when `branch` is created for the save: HEAD is detached, or it is on a shared
    /// branch that work in progress must not go on.
    pub new_branch: bool,
    /// The shared branch it was on, when that is why `branch` is new.
    pub from_branch: Option<String>,
    /// Changed and new files that will be committed, relative to the worktree.
    pub files: Vec<String>,
    pub remote: String,
    /// The name it is pushed under. `branch` itself when that is not on the remote yet;
    /// otherwise a new `guardian-save/…` name, so the branch on the remote is left alone.
    pub remote_branch: String,
    /// True when `branch` is already on the remote and the save is pushed beside it.
    pub beside: bool,
    /// True when there is nothing to do: nothing is uncommitted and the remote already has
    /// the branch at this commit.
    pub up_to_date: bool,
    /// Files git ignores that are kept nowhere else (a `.env`, a local database). A commit
    /// does not save them and removing the worktree deletes them.
    pub ignored: Vec<String>,
    /// Stands for the commit it starts from, where it goes, and the files to commit with
    /// their sizes and modification times. A save given this id refuses if a fresh plan's
    /// differs: a file added, removed or edited since.
    pub id: String,
    /// What HEAD was, to tell whether a commit that reported failure was made after all.
    #[serde(skip)]
    head: String,
    /// Something was staged before the save touched anything.
    #[serde(skip)]
    had_staged: bool,
}

/// How a save is to be done.
#[derive(Clone, Debug, Default)]
pub struct SaveOptions {
    /// The id of the plan the person agreed to. The save refuses if the worktree no longer
    /// matches it.
    pub expected_id: Option<String>,
    /// Skip the repository's commit hooks. Push hooks always run.
    pub no_verify: bool,
}

/// What a save did.
#[derive(Clone, Debug, Serialize)]
pub struct Saved {
    pub path: PathBuf,
    pub branch: String,
    /// How many files went into the commit; 0 when there was nothing uncommitted.
    pub committed_files: usize,
    /// `origin/feature`: where the work now is on the remote.
    pub pushed_to: String,
}

/// Run git in `dir` and return its output, or what it said went wrong. Hooks and signing
/// run as the repository has them set up; a git that hangs is stopped after `timeout`.
fn run(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut child = git_command(dir)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git could not be started: {e}"))?;
    // Output is collected as it comes and read after git exits, without waiting for the
    // pipe to close: something git started (an ssh connection kept open for reuse) can hold
    // the pipe long after git itself is done.
    let collect = |pipe: Option<Box<dyn Read + Send>>| {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let (done, finished) = mpsc::channel::<()>();
        let into = Arc::clone(&buffer);
        std::thread::spawn(move || {
            if let Some(mut pipe) = pipe {
                let mut chunk = [0u8; 8192];
                while let Ok(n) = pipe.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut buffer) = into.lock() {
                        buffer.extend_from_slice(&chunk[..n]);
                    }
                }
            }
            let _ = done.send(());
        });
        (buffer, finished)
    };
    let (out, out_done) = collect(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let (err, err_done) = collect(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "`git {}` did not finish in {} seconds",
                    args.first().copied().unwrap_or(""),
                    timeout.as_secs()
                ));
            }
        }
    };
    let text = |buffer: &Arc<Mutex<Vec<u8>>>, done: &mpsc::Receiver<()>| {
        let _ = done.recv_timeout(Duration::from_secs(2));
        buffer
            .lock()
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .unwrap_or_default()
    };
    if status.success() {
        return Ok(text(&out, &out_done));
    }
    let said: String = text(&err, &err_done)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(400)
        .collect();
    Err(if said.is_empty() {
        format!("`git {}` failed", args.first().copied().unwrap_or(""))
    } else {
        said
    })
}

/// A question to git: its trimmed answer, or `None` if it had none.
fn ask(dir: &Path, args: &[&str]) -> Option<String> {
    run(dir, args, LOCAL_TIMEOUT)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Does a new file with this name look like it holds credentials? Erring towards yes:
/// a refusal costs a commit by hand, a wrong no publishes a secret.
pub fn looks_secret(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    if name.starts_with(".env") {
        // Templates meant to be committed.
        return ![".example", ".sample", ".template", ".dist", ".defaults"]
            .iter()
            .any(|end| name.ends_with(end));
    }
    const NAMES: &[&str] = &[
        "id_rsa", "id_dsa", "id_ecdsa", "id_ed25519", ".npmrc", ".netrc", ".pypirc", ".htpasswd",
        ".git-credentials", ".pgpass", ".my.cnf", ".vault-token", ".dockercfg", "kubeconfig",
        "credentials", "credentials.json", "secrets.json", "secrets.yaml", "secrets.yml",
        "secret.json", "secret.yaml", "secret.yml",
    ];
    const ENDINGS: &[&str] = &[
        ".pem", ".key", ".p8", ".p12", ".pfx", ".ppk", ".jks", ".keystore", ".kdbx", ".tfvars",
        ".tfstate", ".tfstate.backup", ".kubeconfig", ".mobileprovision",
    ];
    NAMES.contains(&name.as_str())
        || ENDINGS.iter().any(|end| name.ends_with(end))
        || (name.starts_with("service-account") && name.ends_with(".json"))
}

/// Does this text hold a private key, or a token in one of the well-known formats? A
/// backstop for a secret in a file with an ordinary name, not a scanner: the repository's
/// own hooks are the real check, and they run unless told not to.
pub fn holds_secret(text: &str) -> bool {
    const MARKERS: &[&str] = &[
        "PRIVATE KEY-----",
        "github_pat_",
        "ghp_",
        "gho_",
        "ghs_",
        "glpat-",
        "xoxb-",
        "xoxp-",
        "sk-ant-",
        "sk_live_",
        "rk_live_",
        "AIzaSy",
    ];
    if MARKERS.iter().any(|m| text.contains(m)) {
        return true;
    }
    // An AWS access key id: AKIA or ASIA and sixteen more capitals or digits.
    text.match_indices("AKIA").chain(text.match_indices("ASIA")).any(|(at, _)| {
        let rest = &text.as_bytes()[at + 4..];
        rest.len() >= 16 && rest[..16].iter().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    })
}

/// One changed path from `git status --porcelain -z`.
pub(super) struct Change {
    pub(super) path: String,
    pub(super) untracked: bool,
    /// Added to the index but in no commit yet (`git add` of a new file).
    added: bool,
    /// Something of it is staged.
    staged: bool,
    /// Ignored by git (`!!`): not part of any commit.
    pub(super) ignored: bool,
    conflicted: bool,
}

/// Parse `git status --porcelain -z`: NUL-separated `XY path` records, where a rename or
/// copy is followed by a record holding the old path. With `-z` git writes paths as they
/// are; without it, a name with anything unusual in it comes quoted and escaped.
pub(super) fn parse_status(text: &str) -> Vec<Change> {
    let mut changes = Vec::new();
    let mut records = text.split('\0').filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        // Two status letters, a space, the path.
        let (Some(code), Some(path)) = (record.get(..2), record.get(3..)) else {
            continue;
        };
        if code.starts_with('R') || code.starts_with('C') {
            // The path it was renamed or copied from: not a change of its own.
            let _ = records.next();
        }
        changes.push(Change {
            path: path.to_string(),
            untracked: code == "??",
            added: code.starts_with('A'),
            staged: !code.starts_with([' ', '?', '!']),
            ignored: code == "!!",
            conflicted: code.contains('U') || code == "AA" || code == "DD",
        });
    }
    changes
}

/// What the remote has, asked of the remote itself.
struct RemoteState {
    /// The branch its HEAD points at.
    default_branch: Option<String>,
    /// Its branches and the commits they are at.
    heads: HashMap<String, String>,
}

/// Parse `git ls-remote --symref <remote> HEAD 'refs/heads/*'`.
fn parse_remote(text: &str) -> RemoteState {
    let mut state = RemoteState { default_branch: None, heads: HashMap::new() };
    for line in text.lines() {
        let Some((left, name)) = line.split_once('\t') else {
            continue;
        };
        if let Some(target) = left.strip_prefix("ref: ") {
            if name == "HEAD" {
                state.default_branch = target.trim().strip_prefix("refs/heads/").map(str::to_string);
            }
        } else if let Some(branch) = name.strip_prefix("refs/heads/") {
            state.heads.insert(branch.to_string(), left.trim().to_string());
        }
    }
    state
}

fn remote_state(dir: &Path, remote: &str) -> Result<RemoteState, String> {
    run(dir, &["ls-remote", "--symref", remote, "HEAD", "refs/heads/*"], REMOTE_TIMEOUT)
        .map(|text| parse_remote(&text))
        .map_err(|e| format!("{remote} could not be reached to see what is there: {e}"))
}

/// A name that says other people build on the branch, whatever the remote calls its default.
fn shared_name(branch: &str) -> bool {
    const SHARED: &[&str] = &[
        "main", "master", "develop", "development", "dev", "trunk", "staging", "stage", "production",
        "prod", "next", "canary", "stable", "beta", "live", "gh-pages",
    ];
    let lower = branch.to_lowercase();
    SHARED.contains(&lower.as_str())
        || ["release/", "release-", "releases/"].iter().any(|p| lower.starts_with(p))
}

/// A folder-safe name for a branch made for a detached HEAD, from the worktree's folder.
fn branch_name_for(worktree: &Path) -> String {
    let folder: String = worktree
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let folder = folder.trim_matches('-');
    format!("guardian-save/{}", if folder.is_empty() { "worktree" } else { folder })
}

/// `base`, or `base-2` … `base-9`: the first that `taken` does not have.
fn free_name(base: &str, taken: impl Fn(&str) -> bool) -> Option<String> {
    std::iter::once(base.to_string())
        .chain((2..10).map(|n| format!("{base}-{n}")))
        .find(|name| !taken(name))
}

/// Is `rel` inside a build or dependency folder (`node_modules`, `target`, a virtual
/// environment)? Returns that folder. Such a folder with new files in it is one that
/// should have been ignored, not work to commit.
fn inside_build_folder(worktree: &Path, rel: &str) -> Option<String> {
    let mut parent = worktree.to_path_buf();
    let mut so_far = String::new();
    let parts: Vec<&str> = rel.split('/').collect();
    for name in &parts[..parts.len().saturating_sub(1)] {
        if !so_far.is_empty() {
            so_far.push('/');
        }
        so_far.push_str(name);
        if classify_project_dir(&parent, name).is_some() {
            return Some(so_far);
        }
        parent.push(name);
    }
    None
}

/// Work out what saving the worktree at `path` would do, or why Guardian will not. Changes
/// nothing, here or on the remote; it does ask the remote what branches it has.
pub fn plan_save(path: &Path, home: &Path) -> Result<SavePlan, String> {
    guard_path(path, home)?;
    if !fs::symlink_metadata(path.join(".git")).is_ok_and(|m| m.is_file()) {
        return Err("it is not a linked git worktree".to_string());
    }
    let common = ask(path, &["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .ok_or("git could not identify the repository this worktree belongs to")?;
    let main = Path::new(&common)
        .parent()
        .ok_or("git reported no main checkout")?
        .to_path_buf();
    let wanted = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let linked = linked_worktrees(&main)
        .into_iter()
        .find(|w| fs::canonicalize(&w.path).is_ok_and(|p| p == wanted))
        .ok_or("git does not list it as a linked worktree")?;
    if linked.locked {
        return Err(LOCKED.to_string());
    }

    let git_dir = ask(path, &["rev-parse", "--absolute-git-dir"])
        .ok_or("git could not find this worktree's own directory")?;
    const UNDER_WAY: &[&str] = &[
        "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "rebase-merge", "rebase-apply", "BISECT_LOG",
    ];
    if UNDER_WAY.iter().any(|f| Path::new(&git_dir).join(f).exists()) {
        return Err("a merge, rebase, cherry-pick or bisect is under way in it; finish or abort that first".to_string());
    }
    let head = ask(path, &["rev-parse", "HEAD"]).ok_or("it has no commit yet")?;

    // Files flagged skip-worktree or assume-unchanged are left out of status and of a
    // commit, edits and all.
    let flags = run(path, &["ls-files", "-v"], LOCAL_TIMEOUT)
        .map_err(|_| "git could not list its files".to_string())?;
    if flags
        .lines()
        .any(|l| l.starts_with(|c: char| c == 'S' || c.is_ascii_lowercase()))
    {
        return Err("it holds files hidden from git status (skip-worktree or assume-unchanged), which a commit would miss".to_string());
    }

    // Spelled out rather than left to the user's git config, as in `worktree_state`.
    // Asked twice: every new file by name for what will be committed, and ignored things
    // by folder for what will not. Asking for both at once names every ignored file, and
    // there are tens of thousands of those in an installed dependency folder.
    let status = |untracked: &str, ignored: &str| {
        run(
            path,
            &["status", "--porcelain", "-z", untracked, ignored, "--ignore-submodules=none"],
            LOCAL_TIMEOUT,
        )
        .map_err(|_| "git could not say what is uncommitted in it".to_string())
    };
    let ignored: Vec<String> = parse_status(&status("--untracked-files=normal", "--ignored=traditional")?)
        .into_iter()
        .filter(|change| change.ignored)
        .map(|change| change.path.trim_end_matches('/').to_string())
        .filter(|shown| !ignored_path_regenerates(path, shown))
        .collect();
    let mut files = Vec::new();
    let mut stamps: Vec<(u64, u128)> = Vec::new();
    let mut secrets = Vec::new();
    let mut had_staged = false;
    let (mut new_files, mut new_bytes) = (0usize, 0u64);
    for change in parse_status(&status("--untracked-files=all", "--ignored=no")?) {
        let on_disk = path.join(&change.path);
        if change.ignored {
            continue;
        }
        had_staged |= change.staged;
        if change.conflicted {
            return Err(format!("{} has an unresolved merge conflict", change.path));
        }
        if change.untracked && change.path.ends_with('/') {
            return Err(format!(
                "{} is a git repository of its own inside it; a commit here would not save what is in there",
                change.path.trim_end_matches('/')
            ));
        }
        if on_disk.join(".git").exists() {
            return Err(format!(
                "{} is a submodule or a repository of its own with changes; save those first",
                change.path
            ));
        }
        // New to git whether or not it has been staged: `git add` does not make a file any
        // safer to publish.
        let new_to_git = change.untracked || change.added;
        let meta = fs::symlink_metadata(&on_disk).ok();
        stamps.push((
            meta.as_ref().map(|m| m.len()).unwrap_or(0),
            meta.as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        if new_to_git {
            if let Some(folder) = inside_build_folder(path, &change.path) {
                return Err(format!(
                    "{folder} is not ignored by git, so a commit would add everything in it; add it to .gitignore first"
                ));
            }
            let bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            if bytes > MAX_NEW_FILE_BYTES {
                return Err(format!(
                    "{} is {} and not in git yet: too large to commit and push without a look",
                    change.path,
                    super::human_size(bytes)
                ));
            }
            new_files += 1;
            new_bytes += bytes;
            if new_files > MAX_NEW_FILES || new_bytes > MAX_NEW_BYTES {
                return Err(format!(
                    "it has more than {MAX_NEW_FILES} new files or {} of them, which looks like a folder that should be ignored rather than work in progress",
                    super::human_size(MAX_NEW_BYTES)
                ));
            }
            if looks_secret(&change.path) {
                secrets.push(change.path.clone());
            }
        }
        // Every file that would be committed is read for a key or a token, new or not: an
        // edit to a file git already has can add one.
        let by_content = meta.is_some_and(|m| m.is_file())
            && fs::File::open(&on_disk)
                .ok()
                .map(|file| {
                    let mut start = Vec::new();
                    let _ = file.take(SCAN_BYTES).read_to_end(&mut start);
                    holds_secret(&String::from_utf8_lossy(&start))
                })
                .unwrap_or(false);
        if by_content && !secrets.contains(&change.path) {
            secrets.push(change.path.clone());
        }
        files.push(change.path);
    }
    if !secrets.is_empty() {
        return Err(format!(
            "{} look{} like credentials and would be pushed; add {} to .gitignore, or commit by hand",
            secrets.join(", "),
            if secrets.len() == 1 { "s" } else { "" },
            if secrets.len() == 1 { "it" } else { "them" }
        ));
    }
    // Sorted together, so that the same worktree always gives the same id.
    let mut listed: Vec<(String, (u64, u128))> = files.into_iter().zip(stamps).collect();
    listed.sort();
    let (files, stamps): (Vec<String>, Vec<(u64, u128)>) = listed.into_iter().unzip();

    let current = ask(path, &["symbolic-ref", "--short", "-q", "HEAD"]);
    let configured = |key: &str| {
        current
            .as_ref()
            .and_then(|branch| ask(path, &["config", "--get", &format!("branch.{branch}.{key}")]))
    };
    let remotes: Vec<String> = ask(path, &["remote"])
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default();
    if remotes.is_empty() {
        return Err("its repository has no remote to push to".to_string());
    }
    // Where the user's own `git push` would go: the branch's push remote, else the
    // repository's default push remote (a fork, with the original as what it follows),
    // else the remote it follows; failing those, `origin`, or the only remote there is. A
    // push setting that names something else (a URL) is not second-guessed.
    let push_setting = |value: Option<String>, setting: &str| match value {
        Some(remote) if remotes.contains(&remote) => Ok(Some(remote)),
        Some(other) => Err(format!(
            "{setting} sends its pushes to {other}, which is not one of the repository's remotes; push it yourself"
        )),
        None => Ok(None),
    };
    let remote = match push_setting(configured("pushRemote"), "its branch's pushRemote")? {
        Some(remote) => remote,
        None => match push_setting(ask(path, &["config", "--get", "remote.pushDefault"]), "remote.pushDefault")? {
            Some(remote) => remote,
            None => configured("remote")
                .filter(|remote| remote != "." && remotes.contains(remote))
                .or_else(|| remotes.iter().find(|r| *r == "origin").cloned())
                .or_else(|| (remotes.len() == 1).then(|| remotes[0].clone()))
                .ok_or("its repository has several remotes and none is called origin; push it yourself")?,
        },
    };

    let there = remote_state(path, &remote)?;
    let local_exists = |name: &str| ask(path, &["rev-parse", "-q", "--verify", &format!("refs/heads/{name}")]).is_some();

    // Work in progress never goes on a branch other people build on: the remote's default
    // branch, or one named like it. That work, and a detached HEAD's, gets a branch of its
    // own that is in use neither here nor there.
    let shared = |branch: &str| Some(branch) == there.default_branch.as_deref() || shared_name(branch);
    let (branch, new_branch, from_branch) = match current.clone() {
        Some(branch) if !shared(&branch) => (branch, false, None),
        on => {
            let free = free_name(&branch_name_for(path), |name| {
                local_exists(name) || there.heads.contains_key(name)
            })
            .ok_or("no branch name is free to save its work under")?;
            (free, true, on)
        }
    };

    // A branch that is already on the remote is not added to: it is usually a pull
    // request. The save goes beside it under a name the remote does not have.
    let on_remote = there.heads.get(&branch);
    let up_to_date = files.is_empty() && !new_branch && on_remote == Some(&head);
    let beside = on_remote.is_some() && !up_to_date;
    let remote_branch = if beside {
        free_name(&format!("guardian-save/{branch}"), |name| there.heads.contains_key(name))
            .ok_or("no branch name is free on the remote to save its work under")?
    } else {
        branch.clone()
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (&head, &branch, new_branch, &remote, &remote_branch, &files, &stamps, &ignored).hash(&mut hasher);
    Ok(SavePlan {
        path: path.to_path_buf(),
        branch,
        new_branch,
        from_branch,
        files,
        remote,
        remote_branch,
        beside,
        up_to_date,
        ignored,
        id: format!("{:016x}", hasher.finish()),
        head,
        had_staged,
    })
}

/// Commit what is uncommitted in the worktree at `path`, push it, and ask the remote
/// whether it has the commit. Everything is decided afresh here; of an earlier plan only
/// its id is used, to refuse if things have changed. On an error after the commit, the
/// commit stays on its branch in the repository.
pub fn save_worktree(path: &Path, home: &Path, options: &SaveOptions) -> Result<Saved, String> {
    let plan = plan_save(path, home)?;
    if options.expected_id.as_ref().is_some_and(|id| *id != plan.id) {
        return Err(format!(
            "it has changed since you were asked ({} file{} to commit now); look at it again",
            plan.files.len(),
            if plan.files.len() == 1 { "" } else { "s" }
        ));
    }
    let branch = plan.branch.as_str();
    let pushed_to = format!("{}/{}", plan.remote, plan.remote_branch);
    if plan.up_to_date {
        return Ok(Saved {
            path: path.to_path_buf(),
            branch: plan.branch.clone(),
            committed_files: 0,
            pushed_to,
        });
    }

    if plan.new_branch {
        run(path, &["checkout", "-q", "-b", branch], LOCAL_TIMEOUT)
            .map_err(|e| format!("could not make branch {branch} to save its work on: {e}"))?;
    }
    if !plan.files.is_empty() {
        let committed = run(path, &["add", "-A"], LOCAL_TIMEOUT)
            .map_err(|e| format!("could not stage its files: {e}"))
            .and_then(|_| {
                let mut commit = vec!["commit", "-q", "-m", COMMIT_MESSAGE];
                if options.no_verify {
                    commit.push("--no-verify");
                }
                run(path, &commit, COMMIT_TIMEOUT).map_err(|e| {
                    format!(
                        "the commit did not go through{}: {e}",
                        if options.no_verify { "" } else { " (a commit hook may have refused it)" }
                    )
                })
            });
        if let Err(why) = committed {
            // git can report a problem after the commit exists (a hook that runs afterwards,
            // a timeout). Then the work is in that commit, and nothing here is undone.
            if ask(path, &["rev-parse", "HEAD"]).as_deref() != Some(plan.head.as_str()) {
                return Err(format!(
                    "its work is committed to branch {branch}, which stays in the repository, but git reported a problem and nothing was pushed: {why}"
                ));
            }
            // Leave it as it was found: back on what it had checked out, and with nothing
            // staged unless something already was.
            if !plan.had_staged {
                let _ = run(path, &["reset", "-q"], LOCAL_TIMEOUT);
            }
            let mut back = String::new();
            if plan.new_branch {
                let to = plan.from_branch.clone().unwrap_or_else(|| plan.head.clone());
                let mut checkout = vec!["checkout", "-q"];
                if plan.from_branch.is_none() {
                    checkout.push("--detach");
                }
                checkout.push(&to);
                let restored = run(path, &checkout, LOCAL_TIMEOUT).is_ok()
                    && run(path, &["branch", "-q", "-D", branch], LOCAL_TIMEOUT).is_ok();
                if !restored {
                    back = format!("; it was left on the new branch {branch}");
                }
            }
            return Err(format!("{why}{back}"));
        }
    }
    let kept = if plan.files.is_empty() && !plan.new_branch {
        String::new()
    } else {
        format!("its work is committed to branch {branch}, which stays in the repository, but ")
    };
    let refspec = format!("refs/heads/{branch}:refs/heads/{}", plan.remote_branch);
    // Never forced, and with the repository's own push hooks: they may be what keeps
    // something from being published. What the branch follows is not changed either.
    run(path, &["push", "-q", plan.remote.as_str(), refspec.as_str()], PUSH_TIMEOUT)
        .map_err(|e| format!("{kept}the push to {} did not go through: {e}", plan.remote))?;

    // Ask the remote itself, not this repository's record of it.
    let head = ask(path, &["rev-parse", "HEAD"]);
    let there = remote_state(path, &plan.remote).map_err(|e| format!("{kept}{e}"))?;
    if head.is_none() || there.heads.get(&plan.remote_branch) != head.as_ref() {
        return Err(format!("{kept}{pushed_to} does not have the commit after the push"));
    }
    Ok(Saved {
        path: path.to_path_buf(),
        branch: plan.branch,
        committed_files: plan.files.len(),
        pushed_to,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_files_that_look_like_credentials() {
        for secret in [
            ".env", ".env.local", "apps/web/.env.production", ".envrc", "id_ed25519", "deploy/server.pem",
            "certs/site.KEY", ".npmrc", "config/credentials.json", "prod.tfvars", "service-account-prod.json",
            "terraform.tfstate", "terraform.tfstate.backup", ".git-credentials", ".pgpass", "AuthKey_AB12CD34EF.p8",
            "server.ppk", "kubeconfig", ".vault-token",
        ] {
            assert!(looks_secret(secret), "{secret}");
        }
        for plain in [
            ".env.example", "apps/web/.env.sample", "src/environment.ts", "id_ed25519.pub", "keyboard.rs",
            "docs/secrets.md", "package.json", "src/credentials.rs",
        ] {
            assert!(!looks_secret(plain), "{plain}");
        }
    }

    #[test]
    fn text_that_holds_a_key_or_a_token() {
        // Built from halves so that this file does not itself look like it holds a secret.
        let pem = ["-----BEGIN OPENSSH ", "PRIVATE KEY-----\nabc"].concat();
        let aws = ["aws_access_key_id = AKIA", "IOSFODNN7EXAMPLE"].concat();
        let github = ["token: ghp", "_0123456789abcdefghijklmnopqrstuvwxyz"].concat();
        for secret in [pem.as_str(), aws.as_str(), github.as_str()] {
            assert!(holds_secret(secret), "{secret}");
        }
        for plain in ["fn main() {}", "AKIA is a prefix", "see the private key docs", "ghp is short for something"] {
            assert!(!holds_secret(plain), "{plain}");
        }
    }

    #[test]
    fn status_records_are_read_with_renames_and_odd_names() {
        let text = " M README.md\0?? new file.txt\0R  after.rs\0before.rs\0!! node_modules/\0UU both.rs\0?? vendor/lib/\0";
        let changes = parse_status(text);
        let seen: Vec<(&str, bool, bool, bool)> = changes
            .iter()
            .map(|c| (c.path.as_str(), c.untracked, c.ignored, c.conflicted))
            .collect();
        let staged: Vec<bool> = changes.iter().map(|c| c.staged).collect();
        assert_eq!(staged, vec![false, false, true, false, true, false]);
        let added = parse_status("A  new.rs\0AM edited-since.rs\0 M old.rs\0");
        assert_eq!(added.iter().map(|c| (c.added, c.staged)).collect::<Vec<_>>(), vec![(true, true), (true, true), (false, false)]);
        assert_eq!(
            seen,
            vec![
                ("README.md", false, false, false),
                ("new file.txt", true, false, false),
                ("after.rs", false, false, false),
                ("node_modules/", false, true, false),
                ("both.rs", false, false, true),
                ("vendor/lib/", true, false, false),
            ]
        );
        assert!(parse_status("").is_empty());
    }

    #[test]
    fn what_the_remote_has_is_read_from_ls_remote() {
        let text = "ref: refs/heads/trunk\tHEAD\nabc123\tHEAD\nabc123\trefs/heads/trunk\ndef456\trefs/heads/feat/x\n";
        let state = parse_remote(text);
        assert_eq!(state.default_branch.as_deref(), Some("trunk"));
        assert_eq!(state.heads.get("feat/x").map(String::as_str), Some("def456"));
        assert_eq!(state.heads.len(), 2);
        let empty = parse_remote("");
        assert!(empty.default_branch.is_none() && empty.heads.is_empty());
    }

    #[test]
    fn branches_other_people_build_on_are_known_by_name() {
        for shared in ["main", "Master", "develop", "gh-pages", "next", "release/2.1", "releases/v3", "prod"] {
            assert!(shared_name(shared), "{shared}");
        }
        for own in ["feat/login", "fix/main-menu", "claude/prune-pages", "mainline-idea", "hotfix/x"] {
            assert!(!shared_name(own), "{own}");
        }
    }

    #[test]
    fn a_detached_head_is_named_after_its_folder() {
        assert_eq!(branch_name_for(Path::new("/h/code/app wt")), "guardian-save/app-wt");
        assert_eq!(branch_name_for(Path::new("/h/code/fix_login-2")), "guardian-save/fix_login-2");
        assert_eq!(branch_name_for(Path::new("/h/code/...")), "guardian-save/worktree");
        assert_eq!(free_name("a", |n| n == "a" || n == "a-2").as_deref(), Some("a-3"));
        assert_eq!(free_name("a", |_| true), None);
    }
}

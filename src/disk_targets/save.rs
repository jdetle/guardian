//! Saving a worktree's unsaved work so that the worktree can then be removed
//! (`guardian disk save`): commit what is uncommitted, push the branch, and check that the
//! remote has it.
//!
//! It exists because a worktree with uncommitted files is otherwise a dead end in a
//! cleanup: Guardian will not delete it, and saving it by hand means opening each one.
//!
//! It refuses rather than guesses. It never forces a push, never commits a file that looks
//! like a secret, never commits into a merge or rebase that is under way, and stops if
//! anything in the worktree would be missed by a plain commit (files hidden from git
//! status, a submodule or nested repository with changes of its own).

use super::{git_command, guard_path, ignored_path_regenerates, linked_worktrees, LOCKED};
use serde::Serialize;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// The message of the commit a save makes.
pub const COMMIT_MESSAGE: &str = "WIP: saved by Guardian before removing this worktree";

/// A new file larger than this is not committed: hosts reject pushes with files not much
/// larger, and a file this size in a worktree is rarely source.
const MAX_NEW_FILE_BYTES: u64 = 50 * 1024 * 1024;

const LOCAL_TIMEOUT: Duration = Duration::from_secs(60);
const PUSH_TIMEOUT: Duration = Duration::from_secs(180);

/// What saving a worktree would do, worked out without changing anything.
#[derive(Clone, Debug, Serialize)]
pub struct SavePlan {
    pub path: PathBuf,
    /// The branch the work is committed to and pushed.
    pub branch: String,
    /// True when `branch` is created for the save: HEAD is detached, or it is on a shared
    /// branch that work in progress must not be pushed to.
    pub new_branch: bool,
    /// The shared branch it was on, when that is why `branch` is new.
    pub from_branch: Option<String>,
    /// Changed and new files that will be committed, relative to the worktree.
    pub files: Vec<String>,
    /// Where the branch is pushed: always under its own name, so `remote_branch` is `branch`.
    pub remote: String,
    pub remote_branch: String,
    /// False when the branch is not on the remote yet: the push puts it there.
    pub has_upstream: bool,
    /// Make the pushed branch the one this branch follows (only when it follows none).
    #[serde(skip)]
    set_upstream: bool,
    /// Files git ignores that are kept nowhere else (a `.env`, a local database). A commit
    /// does not save them and removing the worktree deletes them.
    pub ignored: Vec<String>,
}

/// What a save did.
#[derive(Clone, Debug, Serialize)]
pub struct Saved {
    pub path: PathBuf,
    pub branch: String,
    /// How many files went into the commit; 0 when there was nothing uncommitted.
    pub committed_files: usize,
    /// `origin/feature`: where the branch now is.
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
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
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
    let out = String::from_utf8_lossy(&out.join().unwrap_or_default()).to_string();
    if status.success() {
        return Ok(out);
    }
    let said = String::from_utf8_lossy(&err.join().unwrap_or_default())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let said: String = said.chars().take(400).collect();
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
        "credentials", "credentials.json", "secrets.json", "secrets.yaml", "secrets.yml",
        "secret.json", "secret.yaml", "secret.yml",
    ];
    const ENDINGS: &[&str] = &[
        ".pem", ".key", ".p12", ".pfx", ".jks", ".keystore", ".kdbx", ".tfvars", ".tfstate",
    ];
    NAMES.contains(&name.as_str())
        || ENDINGS.iter().any(|end| name.ends_with(end))
        || (name.starts_with("service-account") && name.ends_with(".json"))
}

/// One changed path from `git status --porcelain -z`.
struct Change {
    path: String,
    untracked: bool,
    /// Ignored by git (`!!`): not part of any commit.
    ignored: bool,
    conflicted: bool,
}

/// Parse `git status --porcelain -z`: NUL-separated `XY path` records, where a rename or
/// copy is followed by a record holding the old path.
fn parse_status(text: &str) -> Vec<Change> {
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
            ignored: code == "!!",
            conflicted: code.contains('U') || code == "AA" || code == "DD",
        });
    }
    changes
}

/// A name for the branch made for a detached HEAD, from the worktree's folder name.
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

/// Work out what saving the worktree at `path` would do, or why Guardian will not. Changes
/// nothing.
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
    let mut secrets = Vec::new();
    for change in parse_status(&status("--untracked-files=all", "--ignored=no")?) {
        let on_disk = path.join(&change.path);
        if change.ignored {
            continue;
        }
        if change.conflicted {
            return Err(format!("{} has an unresolved merge conflict", change.path));
        }
        if change.untracked && change.path.ends_with('/') {
            return Err(format!(
                "{} is a git repository of its own inside it; a commit here would not save what is in there",
                change.path.trim_end_matches('/')
            ));
        }
        if !change.untracked && on_disk.join(".git").exists() {
            return Err(format!(
                "the submodule {} has changes of its own; save those first",
                change.path
            ));
        }
        if change.untracked {
            if looks_secret(&change.path) {
                secrets.push(change.path.clone());
            }
            let bytes = fs::symlink_metadata(&on_disk).map(|m| m.len()).unwrap_or(0);
            if bytes > MAX_NEW_FILE_BYTES {
                return Err(format!(
                    "{} is {} and not in git yet: too large to commit and push without a look",
                    change.path,
                    super::human_size(bytes)
                ));
            }
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

    let current = ask(path, &["symbolic-ref", "--short", "-q", "HEAD"]);
    let configured = |key: &str| {
        current
            .as_ref()
            .and_then(|branch| ask(path, &["config", "--get", &format!("branch.{branch}.{key}")]))
    };
    // The remote the branch already follows, else `origin`, else the only one there is.
    let remote = match configured("remote").filter(|remote| remote != ".") {
        Some(remote) => remote,
        None => {
            let remotes: Vec<String> = ask(path, &["remote"])
                .map(|text| text.lines().map(str::to_string).collect())
                .unwrap_or_default();
            if remotes.iter().any(|r| r == "origin") {
                "origin".to_string()
            } else if remotes.len() == 1 {
                remotes[0].clone()
            } else if remotes.is_empty() {
                return Err("its repository has no remote to push to".to_string());
            } else {
                return Err("its repository has several remotes and none is called origin; push it yourself".to_string());
            }
        }
    };

    // Work in progress is never pushed onto a branch other people build on: the remote's
    // default branch, or one with a name that says as much.
    let default_branch = ask(path, &["symbolic-ref", "-q", "--short", &format!("refs/remotes/{remote}/HEAD")])
        .and_then(|head| head.strip_prefix(&format!("{remote}/")).map(str::to_string));
    let shared = |branch: &str| {
        const SHARED: &[&str] = &[
            "main", "master", "develop", "development", "dev", "trunk", "staging", "production", "prod",
        ];
        Some(branch) == default_branch.as_deref()
            || SHARED.contains(&branch)
            || branch.starts_with("release/")
            || branch.starts_with("release-")
    };
    let (branch, new_branch, from_branch) = match current.clone() {
        Some(branch) if !shared(&branch) => (branch, false, None),
        on => {
            // A detached HEAD, or a shared branch: the work gets a branch of its own, one
            // not in use yet.
            let base = branch_name_for(path);
            let free = std::iter::once(base.clone())
                .chain((2..10).map(|n| format!("{base}-{n}")))
                .find(|name| {
                    ask(path, &["rev-parse", "-q", "--verify", &format!("refs/heads/{name}")]).is_none()
                        && ask(path, &["rev-parse", "-q", "--verify", &format!("refs/remotes/{remote}/{name}")]).is_none()
                })
                .ok_or("no branch name is free to save its work under")?;
            (free, true, on)
        }
    };

    // Always pushed under its own name. A branch's configured upstream is often the branch
    // it was started from (`origin/main`), and pushing there would publish work in progress
    // to it.
    let has_upstream = !new_branch
        && ask(path, &["rev-parse", "-q", "--verify", &format!("refs/remotes/{remote}/{branch}")]).is_some();
    // Only where nothing is configured: an upstream someone set is left as it is.
    let set_upstream = new_branch || configured("merge").is_none();

    Ok(SavePlan {
        path: path.to_path_buf(),
        remote_branch: branch.clone(),
        branch,
        new_branch,
        from_branch,
        files,
        remote,
        has_upstream,
        set_upstream,
        ignored,
    })
}

/// Commit what is uncommitted in the worktree at `path` and push its branch, then check the
/// remote has it. Everything is decided afresh here; nothing is taken from an earlier plan.
/// On an error after the commit, the commit stays on its branch in the repository.
pub fn save_worktree(path: &Path, home: &Path) -> Result<Saved, String> {
    let plan = plan_save(path, home)?;
    let branch = plan.branch.as_str();
    if plan.new_branch {
        run(path, &["checkout", "-q", "-b", branch], LOCAL_TIMEOUT)
            .map_err(|e| format!("could not make branch {branch} to save its work on: {e}"))?;
    }
    if !plan.files.is_empty() {
        run(path, &["add", "-A"], LOCAL_TIMEOUT).map_err(|e| format!("could not stage its files: {e}"))?;
        // Without commit hooks: this is work in progress being put somewhere safe, not a
        // change being proposed, and a failing linter must not be what loses it.
        run(path, &["commit", "-q", "--no-verify", "-m", COMMIT_MESSAGE], LOCAL_TIMEOUT)
            .map_err(|e| format!("could not commit its files: {e}"))?;
    }
    let kept = if plan.files.is_empty() && !plan.new_branch {
        String::new()
    } else {
        format!("its work is committed to branch {branch}, which stays in the repository, but ")
    };
    let refspec = format!("refs/heads/{branch}:refs/heads/{}", plan.remote_branch);
    let mut push = vec!["push", "-q"];
    if plan.set_upstream {
        push.push("--set-upstream");
    }
    push.extend([plan.remote.as_str(), refspec.as_str()]);
    // Never forced, and with the repository's own push hooks: they may be what keeps
    // something from being published.
    run(path, &push, PUSH_TIMEOUT)
        .map_err(|e| format!("{kept}the push to {} did not go through: {e}", plan.remote))?;

    // A push that succeeded has moved the remote-tracking branch to this commit.
    let head = ask(path, &["rev-parse", "HEAD"]);
    let pushed_to = format!("{}/{}", plan.remote, plan.remote_branch);
    let there = ask(path, &["rev-parse", "-q", "--verify", &format!("refs/remotes/{pushed_to}")]);
    if head.is_none() || head != there {
        return Err(format!("{kept}{pushed_to} does not show the commit after the push"));
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
            "terraform.tfstate",
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
    fn status_records_are_read_with_renames_and_odd_names() {
        let text = " M README.md\0?? new file.txt\0R  after.rs\0before.rs\0!! node_modules/\0UU both.rs\0?? vendor/lib/\0";
        let changes = parse_status(text);
        let seen: Vec<(&str, bool, bool, bool)> = changes
            .iter()
            .map(|c| (c.path.as_str(), c.untracked, c.ignored, c.conflicted))
            .collect();
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
    fn a_detached_head_is_named_after_its_folder() {
        assert_eq!(branch_name_for(Path::new("/h/code/app wt")), "guardian-save/app-wt");
        assert_eq!(branch_name_for(Path::new("/h/code/fix_login-2")), "guardian-save/fix_login-2");
        assert_eq!(branch_name_for(Path::new("/h/code/...")), "guardian-save/worktree");
    }
}

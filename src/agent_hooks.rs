//! Guardian's hooks for Cursor, Claude Code and Codex, carried inside the `guardian` binary
//! (`guardian hooks status`, `guardian hooks install`).
//!
//! The hooks are shell scripts in `hooks/`, installed by three shell installers. That used
//! to mean a clone of the repository was needed to install or update them, and nothing told
//! you when the ones on disk had fallen behind the daemon. Here they are compiled in: the
//! binary can put them in place by running the same installers from its own copy, and can
//! say, per agent, whether what is installed is the set it carries.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

macro_rules! assets {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $path)))),*]
    };
}

/// Every file the installers need, by its path in the repository. A test checks that
/// nothing under `hooks/` is missing from this list.
pub const ASSETS: &[(&str, &str)] = assets![
    "hooks/before-read-file.sh",
    "hooks/before-submit-prompt.sh",
    "hooks/claude-code-hooks.json",
    "hooks/claude/post-tool-use.sh",
    "hooks/claude/pre-tool-use.sh",
    "hooks/claude/session-end.sh",
    "hooks/claude/session-start.sh",
    "hooks/claude/stop.sh",
    "hooks/claude/user-prompt-submit.sh",
    "hooks/codex-hooks.json",
    "hooks/codex/post-tool-use.sh",
    "hooks/codex/pre-tool-use.sh",
    "hooks/codex/session-end.sh",
    "hooks/codex/session-start.sh",
    "hooks/codex/stop.sh",
    "hooks/codex/user-prompt-submit.sh",
    "hooks/cursor-commands/guardian-once.md",
    "hooks/cursor-commands/guardian-snooze.md",
    "hooks/cursorignore-checklist.json",
    "hooks/cursorignore_check.py",
    "hooks/hook_policy.default.json",
    "hooks/hooks.json",
    "hooks/install-hooks.sh",
    "hooks/iso_to_epoch.py",
    "hooks/lib.sh",
    "hooks/mac/Guardian-Once.command",
    "hooks/mac/Guardian-Snooze-15m.command",
    "hooks/merge-agent-hooks.jq",
    "hooks/merge-agent-hooks.sh",
    "hooks/post-tool-use.sh",
    "hooks/pre-tool-use.sh",
    "hooks/prompt-gate-common.sh",
    "hooks/resources.md",
    "hooks/session-end.sh",
    "hooks/session-event.jq",
    "hooks/session-event.sh",
    "hooks/session-start.sh",
    "hooks/stop.sh",
    "hooks/subagent-start.sh",
    "hooks/tool-gate-common.sh",
    "scripts/guardian-queue.sh",
    "scripts/install-claude-hooks.sh",
    "scripts/install-codex-hooks.sh",
];

/// An agent Guardian has hooks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Client {
    Cursor,
    Claude,
    Codex,
}

impl Client {
    pub const ALL: [Client; 3] = [Client::Cursor, Client::Claude, Client::Codex];

    /// The name used in the sessions database and in JSON.
    pub fn key(self) -> &'static str {
        match self {
            Client::Cursor => "cursor",
            Client::Claude => "claude",
            Client::Codex => "codex",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Client::Cursor => "Cursor",
            Client::Claude => "Claude Code",
            Client::Codex => "Codex",
        }
    }

    /// The hook configuration this agent reads, as the binary carries it.
    fn template(self) -> &'static str {
        let path = match self {
            Client::Cursor => "hooks/hooks.json",
            Client::Claude => "hooks/claude-code-hooks.json",
            Client::Codex => "hooks/codex-hooks.json",
        };
        asset(path)
    }

    /// The installer for this agent, by its path among the assets.
    fn installer(self) -> &'static str {
        match self {
            Client::Cursor => "hooks/install-hooks.sh",
            Client::Claude => "scripts/install-claude-hooks.sh",
            Client::Codex => "scripts/install-codex-hooks.sh",
        }
    }
}

fn asset(path: &str) -> &'static str {
    ASSETS
        .iter()
        .find(|(p, _)| *p == path)
        .map(|(_, text)| *text)
        .unwrap_or("")
}

/// Where each agent keeps its configuration, and where Guardian keeps its own.
#[derive(Clone, Debug)]
pub struct Places {
    pub home: PathBuf,
    pub cursor: PathBuf,
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub guardian: PathBuf,
}

impl Places {
    /// Everything in its usual place under `home`.
    pub fn under(home: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
            cursor: home.join(".cursor"),
            claude: home.join(".claude"),
            codex: home.join(".codex"),
            guardian: home.join(".guardian"),
        }
    }

    /// As this process finds them: `CLAUDE_CONFIG_DIR` and `CODEX_HOME` are honoured, as
    /// the agents and the installers honour them.
    pub fn from_process() -> Option<Self> {
        let mut places = Self::under(&dirs::home_dir()?);
        let set = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(dir) = set("CLAUDE_CONFIG_DIR") {
            places.claude = dir;
        }
        if let Some(dir) = set("CODEX_HOME") {
            places.codex = dir;
        }
        Some(places)
    }

    fn config_dir(&self, client: Client) -> &Path {
        match client {
            Client::Cursor => &self.cursor,
            Client::Claude => &self.claude,
            Client::Codex => &self.codex,
        }
    }

    /// The file the agent reads its hooks from.
    pub fn config_file(&self, client: Client) -> PathBuf {
        self.config_dir(client).join(match client {
            Client::Claude => "settings.json",
            Client::Cursor | Client::Codex => "hooks.json",
        })
    }

    /// Where Guardian's scripts for this agent are installed.
    pub fn hook_root(&self, client: Client) -> PathBuf {
        self.config_dir(client).join("hooks").join("guardian")
    }
}

/// How one agent's Guardian hooks stand.
#[derive(Clone, Debug, Serialize)]
pub struct ClientStatus {
    pub client: &'static str,
    pub label: &'static str,
    /// The agent's configuration folder exists: it is set up on this machine.
    pub present: bool,
    /// At least one Guardian hook is registered in its configuration.
    pub installed: bool,
    /// The Guardian version the installed scripts came from, if they say.
    pub version: Option<String>,
    /// Events this Guardian has a hook for that are not registered.
    pub missing_events: Vec<String>,
    /// Installed, complete, and from this version of Guardian.
    pub current: bool,
    pub config: PathBuf,
    /// One sentence saying where it stands and what to do, if anything.
    pub summary: String,
}

/// The events a template registers hooks for.
fn template_events(template: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(template)
        .ok()
        .and_then(|v| v.get("hooks").and_then(|h| h.as_object()).map(|h| h.keys().cloned().collect()))
        .unwrap_or_default()
}

/// Does this list of an event's entries hold a handler whose command runs a script under
/// `root`? Entries are handlers themselves (Cursor) or hold them under `hooks` (the others).
fn has_guardian_handler(entries: &serde_json::Value, root: &str) -> bool {
    let runs_ours = |handler: &serde_json::Value| {
        handler
            .get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|c| c.contains(root))
    };
    entries.as_array().is_some_and(|entries| {
        entries.iter().any(|entry| {
            runs_ours(entry)
                || entry
                    .get("hooks")
                    .and_then(|h| h.as_array())
                    .is_some_and(|handlers| handlers.iter().any(runs_ours))
        })
    })
}

/// How `client`'s hooks stand, read from its configuration. `version` is this binary's.
pub fn client_status(client: Client, places: &Places, version: &str) -> ClientStatus {
    let config = places.config_file(client);
    let root = places.hook_root(client);
    let root_text = format!("{}/", root.to_string_lossy());
    let registered = fs::read_to_string(&config)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v.get("hooks").cloned());
    let events = template_events(client.template());
    let has = |event: &str| {
        registered
            .as_ref()
            .and_then(|hooks| hooks.get(event))
            .is_some_and(|entries| has_guardian_handler(entries, &root_text))
    };
    let missing_events: Vec<String> = events.iter().filter(|e| !has(e)).cloned().collect();
    let installed = missing_events.len() < events.len();
    let installed_version = fs::read_to_string(root.join("VERSION"))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let present = places.config_dir(client).is_dir();
    let current = installed && missing_events.is_empty() && installed_version.as_deref() == Some(version);

    let summary = if current {
        format!("hooks {version}, up to date")
    } else if !installed && !present {
        "not set up on this machine".to_string()
    } else if !installed {
        format!("no Guardian hooks: guardian hooks install --{}", client.key())
    } else if !missing_events.is_empty() {
        format!(
            "hooks {} installed, without {}: guardian hooks install",
            installed_version.as_deref().unwrap_or("of an older Guardian"),
            missing_events.join(", ")
        )
    } else {
        format!(
            "hooks {} installed, this Guardian has {version}: guardian hooks install",
            installed_version.as_deref().unwrap_or("of an older Guardian")
        )
    };
    ClientStatus {
        client: client.key(),
        label: client.label(),
        present,
        installed,
        version: installed_version,
        missing_events,
        current,
        config,
        summary,
    }
}

pub fn status(places: &Places, version: &str) -> Vec<ClientStatus> {
    Client::ALL.iter().map(|c| client_status(*c, places, version)).collect()
}

/// Write the assets out under `dir` (replacing what is there), laid out as in the
/// repository, with the scripts executable.
fn unpack(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if dir.exists() {
        fs::remove_dir_all(dir)?;
    }
    for (rel, text) in ASSETS {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, text)?;
        let runs = [".sh", ".py", ".command"].iter().any(|end| rel.ends_with(end));
        fs::set_permissions(&path, fs::Permissions::from_mode(if runs { 0o755 } else { 0o644 }))?;
    }
    Ok(())
}

/// Install (or bring up to date) Guardian's hooks for `client`, by running its installer
/// from this binary's own copy of the hooks. Returns what the installer printed.
pub fn install(client: Client, places: &Places, version: &str) -> Result<String, String> {
    // Unpacked afresh each time, somewhere of Guardian's own: the installers copy from it.
    let share = places.guardian.join("share");
    unpack(&share).map_err(|e| format!("could not unpack the hooks to {}: {e}", share.display()))?;
    let output = Command::new("bash")
        .arg(share.join(client.installer()))
        .env("HOME", &places.home)
        .env("CLAUDE_CONFIG_DIR", &places.claude)
        .env("CODEX_HOME", &places.codex)
        .env("GUARDIAN_DIR", &places.guardian)
        .env("GUARDIAN_HOOKS_VERSION", version)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run the installer: {e}"))?;
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        Ok(said)
    } else {
        Err(said.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("guardian-hooks-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(fs::canonicalize(&dir).unwrap())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn files_under(dir: &Path, root: &Path, out: &mut BTreeSet<String>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                files_under(&path, root, out);
            } else {
                out.insert(path.strip_prefix(root).unwrap().to_string_lossy().to_string());
            }
        }
    }

    #[test]
    fn every_hook_file_is_carried_in_the_binary() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut on_disk = BTreeSet::new();
        files_under(&repo.join("hooks"), repo, &mut on_disk);
        let carried: BTreeSet<String> = ASSETS.iter().map(|(p, _)| p.to_string()).collect();
        let missing: Vec<&String> = on_disk.difference(&carried).collect();
        assert!(missing.is_empty(), "add to ASSETS in src/agent_hooks.rs: {missing:?}");
        for needed in ["scripts/install-claude-hooks.sh", "scripts/install-codex-hooks.sh", "scripts/guardian-queue.sh"] {
            assert!(carried.contains(needed), "{needed}");
        }
        assert!(ASSETS.iter().all(|(_, text)| !text.is_empty()));
    }

    #[test]
    fn each_agent_gets_the_hooks_that_follow_a_session() {
        let events = |c: Client| template_events(c.template());
        for needed in ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop", "SessionEnd"] {
            assert!(events(Client::Claude).iter().any(|e| e == needed), "Claude Code: {needed}");
            assert!(events(Client::Codex).iter().any(|e| e == needed), "Codex: {needed}");
        }
        for needed in ["sessionStart", "beforeSubmitPrompt", "preToolUse", "postToolUse", "stop", "sessionEnd"] {
            assert!(events(Client::Cursor).iter().any(|e| e == needed), "Cursor: {needed}");
        }
    }

    #[test]
    fn status_tells_missing_from_old_from_current() {
        let s = Scratch::new("status");
        let places = Places::under(&s.0);
        let claude = |places: &Places| client_status(Client::Claude, places, "9.9.9");

        let st = claude(&places);
        assert!(!st.present && !st.installed && !st.current);
        assert_eq!(st.summary, "not set up on this machine");

        fs::create_dir_all(&places.claude).unwrap();
        fs::write(places.config_file(Client::Claude), r#"{"model":"x","hooks":{"Stop":[{"hooks":[{"type":"command","command":"/someone/elses/hook.sh"}]}]}}"#).unwrap();
        let st = claude(&places);
        assert!(st.present && !st.installed, "another tool's hook is not Guardian's");
        assert!(st.summary.contains("guardian hooks install --claude"), "{}", st.summary);

        // The three hooks an older Guardian installed, with no version stamp.
        let root = places.hook_root(Client::Claude);
        let hook = |script: &str| serde_json::json!([{"hooks": [{"type": "command", "command": format!("{}/claude/{script}", root.display())}]}]);
        let old = serde_json::json!({"hooks": {
            "SessionStart": hook("session-start.sh"),
            "UserPromptSubmit": hook("user-prompt-submit.sh"),
            "PreToolUse": hook("pre-tool-use.sh"),
        }});
        fs::write(places.config_file(Client::Claude), old.to_string()).unwrap();
        let st = claude(&places);
        assert!(st.installed && !st.current);
        assert_eq!(st.missing_events, vec!["PostToolUse", "SessionEnd", "Stop"]);
        assert!(st.summary.contains("without PostToolUse, SessionEnd, Stop"), "{}", st.summary);
    }

    #[test]
    fn install_puts_every_agents_hooks_in_place_from_the_binary_alone() {
        if Command::new("jq").arg("--version").output().is_err() {
            eprintln!("skipped: jq is not installed");
            return;
        }
        let s = Scratch::new("install");
        let places = Places::under(&s.0);
        // Someone else's hook, which must survive.
        fs::create_dir_all(&places.claude).unwrap();
        fs::write(
            places.config_file(Client::Claude),
            r#"{"theme":"dark","hooks":{"Stop":[{"hooks":[{"type":"command","command":"/someone/elses/hook.sh"}]}]}}"#,
        )
        .unwrap();

        for client in Client::ALL {
            let said = install(client, &places, "9.9.9").unwrap_or_else(|e| panic!("{}: {e}", client.label()));
            assert!(!said.is_empty());
            let st = client_status(client, &places, "9.9.9");
            assert!(st.current, "{}: {}", client.label(), st.summary);
            assert_eq!(st.version.as_deref(), Some("9.9.9"));
            assert!(st.missing_events.is_empty());
            // The scripts it registered are there and can run.
            assert!(places.hook_root(client).join("session-event.sh").is_file());
            assert!(places.hook_root(client).join("session-event.jq").is_file());
        }
        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(places.config_file(Client::Claude)).unwrap()).unwrap();
        assert_eq!(settings["theme"], "dark");
        let stop = settings["hooks"]["Stop"].to_string();
        assert!(stop.contains("/someone/elses/hook.sh") && stop.contains("claude/stop.sh"), "{stop}");

        // Installing again changes nothing and is still current; a newer Guardian sees
        // what is installed as older.
        install(Client::Claude, &places, "9.9.9").unwrap();
        let again: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(places.config_file(Client::Claude)).unwrap()).unwrap();
        assert_eq!(again, settings);
        let newer = client_status(Client::Claude, &places, "10.0.0");
        assert!(newer.installed && !newer.current);
        assert!(newer.summary.contains("hooks 9.9.9 installed, this Guardian has 10.0.0"), "{}", newer.summary);

        // And the installed hooks record a session, in the scratch home.
        let record = |script: &str, input: &str| {
            let mut child = Command::new("bash")
                .arg(places.hook_root(Client::Claude).join("claude").join(script))
                .env("HOME", &s.0)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            use std::io::Write;
            child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(out.status.success(), "{script}");
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        assert_eq!(record("post-tool-use.sh", r#"{"session_id":"s1","tool_name":"Bash","cwd":"/w"}"#), "", "record only");
        assert_eq!(record("stop.sh", r#"{"session_id":"s1"}"#), "");
        let rows = Command::new("sqlite3")
            .arg(places.guardian.join("sessions.db"))
            .arg("select client, state, tool_call_count, turn_count, cwd from sessions")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&rows.stdout).trim(), "claude|idle|1|1|/w");
    }
}

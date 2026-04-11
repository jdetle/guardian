//! Writes `~/.guardian/hook_policy.json` so shell hooks can mirror daemon policy without parsing TOML.

use crate::config::{CursorIgnorePolicyConfig, GuardianConfig, PromptGateConfig, SessionBudgetConfig};
use crate::config::guardian_dir;
use serde::Serialize;
use std::fs;
use std::io::Write;

#[derive(Serialize)]
struct HookPolicyJson {
    prompt_gate: PromptGateConfig,
    session_budget: SessionBudgetConfig,
    cursorignore_policy: CursorIgnorePolicyConfig,
}

pub fn write_hook_policy(cfg: &GuardianConfig) -> std::io::Result<()> {
    let dir = guardian_dir();
    fs::create_dir_all(&dir)?;

    let export = HookPolicyJson {
        prompt_gate: cfg.prompt_gate.clone(),
        session_budget: cfg.session_budget.clone(),
        cursorignore_policy: cfg.cursorignore_policy.clone(),
    };

    let json = serde_json::to_string_pretty(&export)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    let tmp_path = dir.join("hook_policy.json.tmp");
    let final_path = dir.join("hook_policy.json");

    if final_path.is_symlink() {
        fs::remove_file(&final_path)?;
    }

    let mut file = fs::File::create(&tmp_path)?;
    file.write_all(json.as_bytes())?;
    file.sync_all()?;
    fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

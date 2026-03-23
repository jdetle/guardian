use crate::config::guardian_dir;
use std::path::PathBuf;

pub fn db_path() -> PathBuf {
    guardian_dir().join("sessions.db")
}

/// SQL schema for the sessions database.
/// Hooks write to this DB using sqlite3 CLI; the SwiftUI app reads from it.
pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sessions (
    conversation_id TEXT PRIMARY KEY,
    model TEXT NOT NULL,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    ended_at TEXT,
    status TEXT,
    loop_count INTEGER DEFAULT 0,
    tool_call_count INTEGER DEFAULT 0,
    pressure_at_start TEXT,
    pressure_avg TEXT,
    duration_ms INTEGER,
    files_modified INTEGER DEFAULT 0,
    lines_changed INTEGER DEFAULT 0,
    estimated_cost_usd REAL DEFAULT 0.0,
    roi_score REAL
);

CREATE TABLE IF NOT EXISTS tool_calls (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    duration_ms INTEGER,
    model TEXT,
    recorded_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (conversation_id) REFERENCES sessions(conversation_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS pressure_samples (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT,
    pressure TEXT NOT NULL,
    cpu_percent REAL,
    memory_available_gb REAL,
    docker_cpu_percent REAL,
    sampled_at TEXT NOT NULL DEFAULT (datetime('now'))
);
"#;

pub fn ensure_db() -> std::io::Result<()> {
    let path = db_path();
    if path.exists() {
        return Ok(());
    }

    let dir = guardian_dir();
    std::fs::create_dir_all(&dir)?;

    let output = std::process::Command::new("sqlite3")
        .arg(path.to_string_lossy().as_ref())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                stdin.write_all(SCHEMA.as_bytes())?;
            }
            child.wait_with_output()
        });

    match output {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("sqlite3 init failed: {stderr}"),
            ))
        }
        Err(e) => Err(e),
    }
}

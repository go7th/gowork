use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::config::sessions_dir;
use crate::llm::Message;

#[derive(Debug, Serialize, Deserialize)]
pub struct Session {
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub model: String,
    pub messages: Vec<Message>,
}

impl Session {
    pub fn new(name: String, model: String, messages: Vec<Message>) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            name,
            created_at: now.clone(),
            updated_at: now,
            model,
            messages,
        }
    }
}

pub fn session_path(name: &str) -> PathBuf {
    let safe = sanitize_name(name);
    sessions_dir().join(format!("{}.json", safe))
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn save_session(session: &Session) -> Result<PathBuf> {
    crate::config::ensure_dirs()?;
    let path = session_path(&session.name);
    let json = serde_json::to_string_pretty(session)?;
    std::fs::write(&path, json)
        .with_context(|| format!("Failed to write session: {}", path.display()))?;
    Ok(path)
}

pub fn load_session(name: &str) -> Result<Session> {
    let path = session_path(name);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read session: {}", path.display()))?;
    let session: Session = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse session: {}", path.display()))?;
    Ok(session)
}

pub fn list_sessions() -> Result<Vec<String>> {
    let dir = sessions_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::llm::LlmConfig;

const DEFAULT_BASE_URL: &str = "http://localhost:8080/v1";
const DEFAULT_MODEL: &str = "mlx-community/Qwen3.5-4B-OptiQ-4bit";

/// Per-role configuration override.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoleConfig {
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    /// If true, this role defaults to --no-tools (skip tool registration).
    pub no_tools: Option<bool>,
    /// Human-readable description shown by --list-roles.
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileConfig {
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    /// SearXNG instance URL for web_search (e.g. "http://localhost:8888")
    /// If set, web_search uses SearXNG JSON API instead of DuckDuckGo HTML scraping.
    pub searxng_url: Option<String>,
    /// Named role presets (e.g. "summarize", "code", "chat").
    /// Use BTreeMap so `--list-roles` output is stable/sorted.
    #[serde(default)]
    pub roles: BTreeMap<String, RoleConfig>,
}

/// Returns ~/.gowork
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".gowork"))
        .unwrap_or_else(|| PathBuf::from(".gowork"))
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn sessions_dir() -> PathBuf {
    config_dir().join("sessions")
}

pub fn ensure_dirs() -> Result<()> {
    let dir = config_dir();
    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create config dir: {}", dir.display()))?;
    }
    let sd = sessions_dir();
    if !sd.exists() {
        std::fs::create_dir_all(&sd).ok();
    }
    Ok(())
}

/// Load config from disk if exists, return defaults otherwise
pub fn load_file_config() -> FileConfig {
    let path = config_path();
    if !path.exists() {
        return FileConfig::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => toml::from_str(&content).unwrap_or_default(),
        Err(_) => FileConfig::default(),
    }
}

/// Save config to disk
#[allow(dead_code)]
pub fn save_file_config(cfg: &FileConfig) -> Result<()> {
    ensure_dirs()?;
    let content = toml::to_string_pretty(cfg)?;
    std::fs::write(config_path(), content)?;
    Ok(())
}

/// Resolved role with the effective `no_tools` flag after merging.
#[derive(Debug, Clone, Default)]
pub struct ResolvedRole {
    /// Whether this role wants --no-tools by default (can still be overridden by CLI).
    pub no_tools: bool,
    /// Whether the user-supplied role name was found; false means an unknown role was silently ignored.
    pub found: bool,
}

/// Resolve the final LLM config by merging:
///   CLI args > role config > top-level file config > built-in defaults
pub fn resolve_llm_config_with_role(
    cli_base_url: Option<String>,
    cli_model: Option<String>,
    cli_api_key: Option<String>,
    role: Option<&str>,
) -> (LlmConfig, ResolvedRole) {
    let file_cfg = load_file_config();

    // Find the named role if any.
    let (role_cfg, found) = match role {
        Some(name) => match file_cfg.roles.get(name) {
            Some(rc) => (Some(rc.clone()), true),
            None => (None, false),
        },
        None => (None, false),
    };

    let base_url = cli_base_url
        .or_else(|| role_cfg.as_ref().and_then(|r| r.base_url.clone()))
        .or(file_cfg.base_url.clone())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let model = cli_model
        .or_else(|| role_cfg.as_ref().and_then(|r| r.model.clone()))
        .or(file_cfg.model.clone())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let api_key = cli_api_key
        .or_else(|| role_cfg.as_ref().and_then(|r| r.api_key.clone()))
        .or(file_cfg.api_key.clone());

    let resolved = ResolvedRole {
        no_tools: role_cfg.as_ref().and_then(|r| r.no_tools).unwrap_or(false),
        found,
    };

    (LlmConfig { base_url, model, api_key }, resolved)
}

/// Backward-compat wrapper (no role).
#[allow(dead_code)]
pub fn resolve_llm_config(
    cli_base_url: Option<String>,
    cli_model: Option<String>,
    cli_api_key: Option<String>,
) -> LlmConfig {
    resolve_llm_config_with_role(cli_base_url, cli_model, cli_api_key, None).0
}

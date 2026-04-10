use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::llm::LlmConfig;

const DEFAULT_BASE_URL: &str = "http://localhost:8080/v1";
const DEFAULT_MODEL: &str = "mlx-community/Qwen3.5-4B-OptiQ-4bit";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileConfig {
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    /// SearXNG instance URL for web_search (e.g. "http://localhost:8888")
    /// If set, web_search uses SearXNG JSON API instead of DuckDuckGo HTML scraping.
    pub searxng_url: Option<String>,
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

/// Resolve final LLM config: CLI args > env > config file > defaults
pub fn resolve_llm_config(
    cli_base_url: Option<String>,
    cli_model: Option<String>,
    cli_api_key: Option<String>,
) -> LlmConfig {
    let file_cfg = load_file_config();

    LlmConfig {
        base_url: cli_base_url
            .or(file_cfg.base_url)
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
        model: cli_model
            .or(file_cfg.model)
            .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        api_key: cli_api_key.or(file_cfg.api_key),
    }
}

use sha2::{Sha256, Digest};
use std::path::PathBuf;

/// Cache directory: ~/.gowork/cache/
fn cache_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".gowork").join("cache"))
        .unwrap_or_else(|| PathBuf::from(".gowork/cache"))
}

/// Build a cache key from the prompt and optional file path + mtime
fn cache_key(prompt: &str, file_path: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prompt.as_bytes());

    if let Some(path) = file_path {
        hasher.update(path.as_bytes());
        // Include file modification time so cache invalidates on file change
        if let Ok(metadata) = std::fs::metadata(path) {
            if let Ok(mtime) = metadata.modified() {
                let duration = mtime
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                hasher.update(duration.as_secs().to_le_bytes());
            }
        }
    }

    hex::encode(hasher.finalize())
}

/// Get a cached result, returns None if not found
pub fn get(prompt: &str, file_path: Option<&str>) -> Option<String> {
    let key = cache_key(prompt, file_path);
    let path = cache_dir().join(&key);
    std::fs::read_to_string(path).ok()
}

/// Store a result in the cache
pub fn set(prompt: &str, file_path: Option<&str>, output: &str) {
    let key = cache_key(prompt, file_path);
    let dir = cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(&key);
    let _ = std::fs::write(path, output);
}

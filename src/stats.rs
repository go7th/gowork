use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Token estimation: ~3.5 chars per token for code, ~2 chars per token for Chinese
fn estimate_tokens(text: &str) -> usize {
    let total_chars = text.len();
    // Heuristic: count CJK chars for better estimation
    let cjk_chars = text.chars().filter(|c| *c > '\u{2E80}').count();
    let ascii_chars = total_chars - cjk_chars;
    // CJK: ~1.5 tokens per char, ASCII: ~0.28 tokens per char (1/3.5)
    let tokens = (cjk_chars as f64 * 1.5) + (ascii_chars as f64 / 3.5);
    tokens.ceil() as usize
}

fn stats_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".gowork").join("stats.json"))
        .unwrap_or_else(|| PathBuf::from(".gowork/stats.json"))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    /// Total calls made
    pub total_calls: u64,
    /// Total input tokens sent to local model
    pub input_tokens: u64,
    /// Total output tokens from local model
    pub output_tokens: u64,
    /// Estimated Claude tokens saved (input that Claude didn't need to read)
    pub claude_tokens_saved: u64,
    /// Cache hits
    pub cache_hits: u64,
    /// Total processing time in milliseconds
    pub total_time_ms: u64,
}

impl Stats {
    pub fn load() -> Self {
        let path = stats_path();
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = stats_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, serde_json::to_string_pretty(self).unwrap_or_default());
    }

    /// Record a gowork call
    pub fn record_call(&mut self, input_text: &str, output_text: &str, duration_ms: u64) {
        let input_tok = estimate_tokens(input_text) as u64;
        let output_tok = estimate_tokens(output_text) as u64;

        self.total_calls += 1;
        self.input_tokens += input_tok;
        self.output_tokens += output_tok;
        // Claude would have consumed the full input + generated its own output
        // With gowork, Claude only reads the gowork output (~output_tok)
        // So savings = input_tok (Claude didn't read the raw file)
        self.claude_tokens_saved += input_tok;
        self.total_time_ms += duration_ms;
        self.save();
    }

    /// Record a cache hit
    pub fn record_cache_hit(&mut self, input_text: &str) {
        let input_tok = estimate_tokens(input_text) as u64;
        self.cache_hits += 1;
        self.claude_tokens_saved += input_tok;
        self.save();
    }

    /// Format stats for display
    pub fn display(&self) -> String {
        let cost_saved = self.claude_tokens_saved as f64 / 1_000_000.0 * 3.0; // ~$3/MTok input for Claude
        let avg_time = if self.total_calls > 0 {
            self.total_time_ms / self.total_calls
        } else {
            0
        };

        format!(
            "gowork Token Stats\n\
             ─────────────────────────────────\n\
             Total calls:          {}\n\
             Cache hits:           {}\n\
             Local input tokens:   {}\n\
             Local output tokens:  {}\n\
             Claude tokens saved:  {}\n\
             Estimated cost saved: ${:.4}\n\
             Avg response time:    {}ms\n\
             Total time:           {:.1}s",
            self.total_calls,
            self.cache_hits,
            format_number(self.input_tokens),
            format_number(self.output_tokens),
            format_number(self.claude_tokens_saved),
            cost_saved,
            avg_time,
            self.total_time_ms as f64 / 1000.0,
        )
    }

    /// Reset all stats
    pub fn reset(&mut self) {
        *self = Self::default();
        self.save();
    }
}

fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

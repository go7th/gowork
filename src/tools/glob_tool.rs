use anyhow::Result;
use serde_json::{json, Value};
use std::path::PathBuf;

use super::registry::{Tool, ToolResult};
use crate::llm::{FunctionDefinition, ToolDefinition};

pub struct GlobTool;

#[async_trait::async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "glob".to_string(),
                description: "Find files matching a glob pattern (e.g. '**/*.rs', 'src/**/*.ts'). Returns matching file paths sorted by modification time (newest first). Use this to locate files when you don't need to search their contents.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Glob pattern (e.g. '**/*.rs', 'src/**/*.{ts,tsx}')"
                        },
                        "path": {
                            "type": "string",
                            "description": "Base directory to search from (default: current working directory)"
                        }
                    },
                    "required": ["pattern"]
                }),
            },
        }
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let pattern = args["pattern"].as_str().unwrap_or("");
        let base = args["path"].as_str();

        if pattern.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: "Error: pattern is required".to_string(),
            });
        }

        // Build full glob pattern
        let full_pattern = match base {
            Some(b) if !b.is_empty() => {
                let bp = PathBuf::from(b);
                bp.join(pattern).to_string_lossy().to_string()
            }
            _ => pattern.to_string(),
        };

        let entries = match glob::glob(&full_pattern) {
            Ok(e) => e,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: format!("Invalid glob pattern: {}", e),
                });
            }
        };

        // Collect with mtime for sorting
        let mut files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
        for entry in entries.flatten() {
            if entry.is_file() {
                let mtime = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                files.push((entry, mtime));
            }
        }

        // Sort by mtime descending (newest first)
        files.sort_by(|a, b| b.1.cmp(&a.1));

        if files.is_empty() {
            return Ok(ToolResult {
                success: true,
                output: format!("No files matched pattern: {}", pattern),
            });
        }

        let total = files.len();
        let display_limit = 200;
        let truncated = total > display_limit;
        let display: Vec<String> = files
            .iter()
            .take(display_limit)
            .map(|(p, _)| p.display().to_string())
            .collect();

        let mut output = display.join("\n");
        if truncated {
            output.push_str(&format!(
                "\n... ({} more files, total {} matches)",
                total - display_limit,
                total
            ));
        }

        Ok(ToolResult {
            success: true,
            output,
        })
    }
}

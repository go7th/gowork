use anyhow::Result;
use serde_json::{json, Value};
use tokio::process::Command;

use super::registry::{Tool, ToolResult};
use crate::llm::{FunctionDefinition, ToolDefinition};

pub struct GrepTool;

#[async_trait::async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "grep".to_string(),
                description: "Search file contents using regex patterns. Uses ripgrep (rg) if available, falls back to grep.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "The regex pattern to search for"
                        },
                        "path": {
                            "type": "string",
                            "description": "Directory or file to search in (default: current directory)"
                        },
                        "include": {
                            "type": "string",
                            "description": "File glob pattern to include (e.g. '*.rs')"
                        },
                        "case_insensitive": {
                            "type": "boolean",
                            "description": "Case insensitive search"
                        }
                    },
                    "required": ["pattern"]
                }),
            },
        }
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let pattern = args["pattern"].as_str().unwrap_or("");
        let path = args["path"].as_str().unwrap_or(".");
        let include = args["include"].as_str();
        let case_insensitive = args["case_insensitive"].as_bool().unwrap_or(false);

        if pattern.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: "Error: pattern is required".to_string(),
            });
        }

        // Try ripgrep first, fall back to grep
        let (cmd, use_rg) = if Command::new("rg").arg("--version").output().await.is_ok() {
            ("rg", true)
        } else {
            ("grep", false)
        };

        let mut command = Command::new(cmd);

        if use_rg {
            command.arg("--line-number").arg("--no-heading");
            if case_insensitive {
                command.arg("-i");
            }
            if let Some(glob) = include {
                command.arg("--glob").arg(glob);
            }
            command.arg(pattern).arg(path);
        } else {
            command.arg("-rn");
            if case_insensitive {
                command.arg("-i");
            }
            if let Some(glob) = include {
                command.arg("--include").arg(glob);
            }
            command.arg(pattern).arg(path);
        }

        match command.output().await {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let mut result = stdout.to_string();

                if result.len() > 50000 {
                    result.truncate(50000);
                    result.push_str("\n... (output truncated)");
                }

                Ok(ToolResult {
                    success: true,
                    output: if result.is_empty() {
                        "No matches found.".to_string()
                    } else {
                        result
                    },
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                output: format!("Error running search: {}", e),
            }),
        }
    }
}

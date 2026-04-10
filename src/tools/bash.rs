use anyhow::Result;
use serde_json::{json, Value};
use tokio::process::Command;

use super::registry::{Tool, ToolResult};
use crate::llm::{FunctionDefinition, ToolDefinition};

pub struct BashTool;

#[async_trait::async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "bash".to_string(),
                description: "Execute a bash command and return its output.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The bash command to execute"
                        },
                        "timeout": {
                            "type": "integer",
                            "description": "Timeout in seconds (default: 120)"
                        }
                    },
                    "required": ["command"]
                }),
            },
        }
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let command = args["command"].as_str().unwrap_or("");
        let timeout_secs = args["timeout"].as_u64().unwrap_or(120);

        if command.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: "Error: command is required".to_string(),
            });
        }

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            Command::new("bash")
                .arg("-c")
                .arg(command)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                let mut result_text = String::new();
                if !stdout.is_empty() {
                    result_text.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result_text.is_empty() {
                        result_text.push('\n');
                    }
                    result_text.push_str("[stderr] ");
                    result_text.push_str(&stderr);
                }

                // Truncate if too long
                if result_text.len() > 50000 {
                    result_text.truncate(50000);
                    result_text.push_str("\n... (output truncated)");
                }

                Ok(ToolResult {
                    success: output.status.success(),
                    output: if result_text.is_empty() {
                        "(no output)".to_string()
                    } else {
                        result_text
                    },
                })
            }
            Ok(Err(e)) => Ok(ToolResult {
                success: false,
                output: format!("Error executing command: {}", e),
            }),
            Err(_) => Ok(ToolResult {
                success: false,
                output: format!("Command timed out after {}s", timeout_secs),
            }),
        }
    }
}

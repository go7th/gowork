use anyhow::Result;
use serde_json::{json, Value};

use super::registry::{Tool, ToolResult};
use crate::llm::{FunctionDefinition, ToolDefinition};

pub struct ReadFileTool;

#[async_trait::async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "read_file".to_string(),
                description: "Read the contents of a file. Returns the file content with line numbers.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "The file path to read"
                        },
                        "offset": {
                            "type": "integer",
                            "description": "Line number to start reading from (0-based)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of lines to read"
                        }
                    },
                    "required": ["path"]
                }),
            },
        }
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let path = args["path"].as_str().unwrap_or("");
        let offset = args["offset"].as_u64().unwrap_or(0) as usize;
        let limit = args["limit"].as_u64().unwrap_or(2000) as usize;

        if path.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: "Error: path is required".to_string(),
            });
        }

        match tokio::fs::read_to_string(path).await {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let end = (offset + limit).min(lines.len());
                let selected: Vec<String> = lines[offset..end]
                    .iter()
                    .enumerate()
                    .map(|(i, line)| format!("{:>4}\t{}", offset + i + 1, line))
                    .collect();

                Ok(ToolResult {
                    success: true,
                    output: selected.join("\n"),
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                output: format!("Error reading file: {}", e),
            }),
        }
    }
}

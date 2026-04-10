use anyhow::Result;
use colored::Colorize;
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};

use super::registry::{Tool, ToolResult};
use crate::llm::{FunctionDefinition, ToolDefinition};

/// Render a colored unified diff between two strings
pub fn render_diff(old: &str, new: &str, path: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();
    out.push_str(&format!(
        "{} {}\n",
        "---".dimmed(),
        path.dimmed()
    ));
    out.push_str(&format!(
        "{} {}\n",
        "+++".dimmed(),
        path.dimmed()
    ));

    for group in diff.grouped_ops(3) {
        // Hunk header
        if let (Some(first), Some(last)) = (group.first(), group.last()) {
            let old_range = first.old_range();
            let new_range = last.new_range();
            out.push_str(&format!(
                "{}\n",
                format!(
                    "@@ -{},{} +{},{} @@",
                    old_range.start + 1,
                    old_range.len(),
                    new_range.start + 1,
                    new_range.len()
                )
                .cyan()
            ));
        }

        for op in group {
            for change in diff.iter_changes(&op) {
                let (sign, line) = match change.tag() {
                    ChangeTag::Delete => (
                        "-".to_string(),
                        format!("-{}", change.value()).red().to_string(),
                    ),
                    ChangeTag::Insert => (
                        "+".to_string(),
                        format!("+{}", change.value()).green().to_string(),
                    ),
                    ChangeTag::Equal => (
                        " ".to_string(),
                        format!(" {}", change.value()).dimmed().to_string(),
                    ),
                };
                let _ = sign;
                out.push_str(&line);
                if !change.value().ends_with('\n') {
                    out.push('\n');
                }
            }
        }
    }
    out
}

pub struct EditFileTool;

#[async_trait::async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "edit_file".to_string(),
                description: "Edit a file by replacing old_string with new_string. If the file doesn't exist and old_string is empty, creates a new file with new_string as content.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "The file path to edit"
                        },
                        "old_string": {
                            "type": "string",
                            "description": "The exact string to find and replace"
                        },
                        "new_string": {
                            "type": "string",
                            "description": "The replacement string"
                        }
                    },
                    "required": ["path", "old_string", "new_string"]
                }),
            },
        }
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let path = args["path"].as_str().unwrap_or("");
        let old_string = args["old_string"].as_str().unwrap_or("");
        let new_string = args["new_string"].as_str().unwrap_or("");

        if path.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: "Error: path is required".to_string(),
            });
        }

        // Create new file if old_string is empty and file doesn't exist
        if old_string.is_empty() {
            if let Some(parent) = std::path::Path::new(path).parent() {
                tokio::fs::create_dir_all(parent).await.ok();
            }
            tokio::fs::write(path, new_string).await?;
            // Show diff for new file
            let diff = render_diff("", new_string, path);
            println!("\n{}", diff);
            return Ok(ToolResult {
                success: true,
                output: format!("Created file: {}", path),
            });
        }

        // Read existing file
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: format!("Error reading file: {}", e),
                });
            }
        };

        // Check that old_string appears exactly once
        let count = content.matches(old_string).count();
        if count == 0 {
            return Ok(ToolResult {
                success: false,
                output: "Error: old_string not found in file".to_string(),
            });
        }
        if count > 1 {
            return Ok(ToolResult {
                success: false,
                output: format!(
                    "Error: old_string found {} times, must be unique. Provide more context.",
                    count
                ),
            });
        }

        let new_content = content.replacen(old_string, new_string, 1);
        tokio::fs::write(path, &new_content).await?;

        // Print colored diff
        let diff = render_diff(&content, &new_content, path);
        println!("\n{}", diff);

        Ok(ToolResult {
            success: true,
            output: format!("Edited file: {}", path),
        })
    }
}

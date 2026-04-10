use anyhow::Result;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

use super::registry::{Tool, ToolResult};
use crate::llm::{FunctionDefinition, ToolDefinition};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    #[serde(rename = "activeForm")]
    pub active_form: String,
    pub status: TodoStatus,
}

/// Shared todo list state across the session
pub type TodoState = Arc<Mutex<Vec<TodoItem>>>;

pub fn new_todo_state() -> TodoState {
    Arc::new(Mutex::new(Vec::new()))
}

/// Render the current todo list to a colored string for display
pub fn render_todo_list(todos: &[TodoItem]) -> String {
    if todos.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(&format!("{}\n", "Todos:".bold().cyan()));
    for item in todos {
        let marker = match item.status {
            TodoStatus::Pending => "[ ]".dimmed(),
            TodoStatus::InProgress => "[~]".yellow().bold(),
            TodoStatus::Completed => "[x]".green(),
        };
        let text = match item.status {
            TodoStatus::InProgress => item.active_form.clone().yellow().to_string(),
            TodoStatus::Completed => item.content.clone().dimmed().to_string(),
            TodoStatus::Pending => item.content.clone(),
        };
        out.push_str(&format!("  {} {}\n", marker, text));
    }
    out
}

pub struct TodoWriteTool {
    state: TodoState,
}

impl TodoWriteTool {
    pub fn new(state: TodoState) -> Self {
        Self { state }
    }
}

#[async_trait::async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "todo_write"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "todo_write".to_string(),
                description: "Create and manage a structured task list for the current session. Use proactively for multi-step tasks (3+ steps). Each item needs both 'content' (imperative form like 'Run tests') and 'activeForm' (present continuous like 'Running tests'). Exactly ONE task should be in_progress at any time. Mark tasks completed IMMEDIATELY after finishing.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "todos": {
                            "type": "array",
                            "description": "The complete updated todo list",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "content": {
                                        "type": "string",
                                        "description": "Imperative form of the task"
                                    },
                                    "activeForm": {
                                        "type": "string",
                                        "description": "Present continuous form shown when in progress"
                                    },
                                    "status": {
                                        "type": "string",
                                        "enum": ["pending", "in_progress", "completed"]
                                    }
                                },
                                "required": ["content", "activeForm", "status"]
                            }
                        }
                    },
                    "required": ["todos"]
                }),
            },
        }
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let todos_value = args.get("todos").cloned().unwrap_or(Value::Null);
        let new_todos: Vec<TodoItem> = match serde_json::from_value(todos_value) {
            Ok(v) => v,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: format!("Invalid todo list format: {}", e),
                });
            }
        };

        // Validate: at most one in_progress
        let in_progress_count = new_todos
            .iter()
            .filter(|t| t.status == TodoStatus::InProgress)
            .count();
        if in_progress_count > 1 {
            return Ok(ToolResult {
                success: false,
                output: format!(
                    "Error: only ONE task can be in_progress at a time, found {}",
                    in_progress_count
                ),
            });
        }

        // Update state
        {
            let mut state = self.state.lock().unwrap();
            *state = new_todos.clone();
        }

        // Print to user
        let rendered = render_todo_list(&new_todos);
        if !rendered.is_empty() {
            println!("\n{}", rendered);
        }

        // Return summary to LLM
        let summary = new_todos
            .iter()
            .map(|t| {
                let status = match t.status {
                    TodoStatus::Pending => "pending",
                    TodoStatus::InProgress => "in_progress",
                    TodoStatus::Completed => "completed",
                };
                format!("[{}] {}", status, t.content)
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(ToolResult {
            success: true,
            output: format!("Todo list updated ({} items):\n{}", new_todos.len(), summary),
        })
    }
}

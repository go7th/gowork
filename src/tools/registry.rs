use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;

use crate::llm::ToolDefinition;

/// Result of a tool execution
#[derive(Debug)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
}

/// Trait that all tools must implement
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;
    async fn execute(&self, args: Value) -> Result<ToolResult>;
}

/// Registry holding all available tools
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.definition()).collect()
    }

    pub async fn execute(&self, name: &str, args: Value) -> Result<ToolResult> {
        let tool = self.tools.get(name).ok_or_else(|| {
            anyhow::anyhow!("Unknown tool: {}", name)
        })?;
        tool.execute(args).await
    }
}

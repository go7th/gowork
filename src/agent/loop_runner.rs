use anyhow::Result;
use colored::Colorize;
use std::io::Write;

use crate::llm::{LlmClient, LlmConfig, Message, StreamEvent};
use crate::tools::ToolRegistry;

const SYSTEM_PROMPT: &str = r#"You are a powerful coding assistant. You help users with software engineering tasks including writing code, debugging, refactoring, and explaining code.

You have access to tools to interact with the local filesystem and execute commands. Use them when needed.

Rules:
- Read files before editing them
- Be concise and direct
- Prefer editing existing files over creating new ones
- Always explain what you're doing briefly before taking action"#;

pub struct AgentLoop {
    client: LlmClient,
    config: LlmConfig,
    tools: ToolRegistry,
    messages: Vec<Message>,
}

impl AgentLoop {
    pub fn new(config: LlmConfig, tools: ToolRegistry) -> Self {
        let client = LlmClient::new(config.clone());
        let messages = vec![Message::system(SYSTEM_PROMPT)];

        Self {
            client,
            config,
            tools,
            messages,
        }
    }

    /// Get a clone of the current message history
    pub fn messages(&self) -> Vec<Message> {
        self.messages.clone()
    }

    /// Get the current model name
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Switch to a different model (rebuilds the LLM client)
    pub fn set_model(&mut self, model: String) {
        self.config.model = model;
        self.client = LlmClient::new(self.config.clone());
    }

    /// List available models from the LLM backend
    pub async fn list_available_models(&self) -> Result<Vec<String>> {
        self.client.list_models().await
    }

    /// Replace the message history (for /load)
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// Reset to a fresh conversation
    pub fn reset(&mut self) {
        self.messages = vec![Message::system(SYSTEM_PROMPT)];
    }


    /// Process a user message and return when the agent is done
    pub async fn process(&mut self, user_input: &str) -> Result<()> {
        self.process_with_images(user_input, Vec::new()).await
    }

    /// Process a user message with optional images
    pub async fn process_with_images(
        &mut self,
        user_input: &str,
        image_data_urls: Vec<String>,
    ) -> Result<()> {
        if image_data_urls.is_empty() {
            self.messages.push(Message::user(user_input));
        } else {
            self.messages
                .push(Message::user_with_images(user_input, image_data_urls));
        }
        self.run_loop(false).await.map(|_| ())
    }

    /// Process and capture output text (for one-shot + cache)
    pub async fn process_capture(&mut self, user_input: &str) -> Result<String> {
        self.process_with_images_capture(user_input, Vec::new()).await
    }

    /// Process with images and capture output text
    pub async fn process_with_images_capture(
        &mut self,
        user_input: &str,
        image_data_urls: Vec<String>,
    ) -> Result<String> {
        if image_data_urls.is_empty() {
            self.messages.push(Message::user(user_input));
        } else {
            self.messages
                .push(Message::user_with_images(user_input, image_data_urls));
        }
        self.run_loop(true).await
    }

    async fn run_loop(&mut self, capture: bool) -> Result<String> {
        let mut captured_output = String::new();

        loop {
            let tool_defs = self.tools.definitions();
            let tools = if tool_defs.is_empty() {
                None
            } else {
                Some(tool_defs)
            };

            let mut rx = self
                .client
                .chat_stream(self.messages.clone(), tools)
                .await?;

            let mut assistant_content = String::new();
            let mut tool_calls = Vec::new();

            while let Some(event) = rx.recv().await {
                match event {
                    StreamEvent::Token(token) => {
                        print!("{}", token);
                        let _ = std::io::stdout().flush();
                        assistant_content.push_str(&token);
                    }
                    StreamEvent::ToolCall(tc) => {
                        tool_calls.push(tc);
                    }
                    StreamEvent::Done => break,
                    StreamEvent::Error(e) => {
                        eprintln!("\n{}: {}", "Error".red().bold(), e);
                        return Ok(captured_output);
                    }
                }
            }

            // Build assistant message
            if !assistant_content.is_empty() || !tool_calls.is_empty() {
                let mut msg = Message::assistant(&assistant_content);
                if !tool_calls.is_empty() {
                    msg.tool_calls = Some(tool_calls.clone());
                }
                self.messages.push(msg);
            }

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                if !assistant_content.is_empty() {
                    println!();
                    if capture {
                        captured_output.push_str(&assistant_content);
                    }
                }
                return Ok(captured_output);
            }

            // Capture intermediate content if in capture mode
            if capture && !assistant_content.is_empty() {
                captured_output.push_str(&assistant_content);
                captured_output.push('\n');
            }

            // Execute tool calls and add results
            for tc in &tool_calls {
                println!(
                    "\n{} {}",
                    "Tool:".cyan().bold(),
                    tc.function.name.yellow()
                );

                let args: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_default();

                // Show brief args summary
                print_tool_args(&tc.function.name, &args);

                let result = self.tools.execute(&tc.function.name, args).await?;

                let status = if result.success {
                    "OK".green().bold()
                } else {
                    "FAIL".red().bold()
                };

                // Show truncated output
                let display_output = if result.output.len() > 500 {
                    let mut end = 500;
                    while !result.output.is_char_boundary(end) {
                        end -= 1;
                    }
                    format!("{}... ({} bytes total)", &result.output[..end], result.output.len())
                } else {
                    result.output.clone()
                };
                println!("  {} {}", status, display_output.dimmed());

                self.messages
                    .push(Message::tool_result(&tc.id, &result.output));
            }

            // Continue the loop - LLM will process tool results
        }
    }
}

fn print_tool_args(tool_name: &str, args: &serde_json::Value) {
    match tool_name {
        "read_file" => {
            if let Some(path) = args["path"].as_str() {
                println!("  {} {}", "path:".dimmed(), path);
            }
        }
        "edit_file" => {
            if let Some(path) = args["path"].as_str() {
                println!("  {} {}", "path:".dimmed(), path);
            }
        }
        "bash" => {
            if let Some(cmd) = args["command"].as_str() {
                let display = if cmd.len() > 100 {
                    let mut end = 100;
                    while !cmd.is_char_boundary(end) {
                        end -= 1;
                    }
                    &cmd[..end]
                } else { cmd };
                println!("  {} {}", "$".dimmed(), display);
            }
        }
        "grep" => {
            if let Some(pattern) = args["pattern"].as_str() {
                println!("  {} {}", "pattern:".dimmed(), pattern);
            }
        }
        _ => {}
    }
}

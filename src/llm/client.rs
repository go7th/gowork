use anyhow::{Result, Context};
use futures::StreamExt;
use reqwest::Client;
use tokio::sync::mpsc;

use super::types::*;

/// Events emitted during streaming
#[derive(Debug)]
pub enum StreamEvent {
    /// Text content token
    Token(String),
    /// A complete tool call
    ToolCall(ToolCall),
    /// Stream finished
    Done,
    /// Error occurred
    Error(String),
}

pub struct LlmClient {
    client: Client,
    config: LlmConfig,
}

impl LlmClient {
    pub fn new(config: LlmConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    /// List available models from the API (/v1/models endpoint)
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/models", self.config.base_url);
        let mut req = self.client.get(&url);
        if let Some(ref key) = self.config.api_key {
            req = req.bearer_auth(key);
        }
        let response = req.send().await.context("Failed to fetch models")?;
        if !response.status().is_success() {
            anyhow::bail!("Models API error: {}", response.status());
        }
        let body: serde_json::Value = response.json().await?;
        let models = body["data"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m["id"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Ok(models)
    }

    /// Send a chat completion request with streaming.
    /// Returns a channel receiver that yields StreamEvents.
    pub async fn chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let request = ChatRequest {
            model: self.config.model.clone(),
            messages,
            stream: true,
            tools,
            temperature: Some(0.1),
        };

        let url = format!("{}/chat/completions", self.config.base_url);

        let mut req_builder = self.client.post(&url).json(&request);

        if let Some(ref api_key) = self.config.api_key {
            req_builder = req_builder.bearer_auth(api_key);
        }

        let response = req_builder
            .send()
            .await
            .context("Failed to connect to LLM API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("LLM API error ({}): {}", status, body);
        }

        let (tx, rx) = mpsc::channel(256);

        tokio::spawn(async move {
            if let Err(e) = process_stream(response, &tx).await {
                let _ = tx.send(StreamEvent::Error(e.to_string())).await;
            }
            let _ = tx.send(StreamEvent::Done).await;
        });

        Ok(rx)
    }
}

async fn process_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<StreamEvent>,
) -> Result<()> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    // Accumulators for tool calls
    let mut tool_calls: Vec<PartialToolCall> = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Stream read error")?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Process complete SSE lines
        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim().to_string();
            buffer = buffer[pos + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }

            let data = line.strip_prefix("data: ").unwrap_or(&line);

            if data == "[DONE]" {
                // Emit any accumulated tool calls
                for tc in tool_calls.drain(..) {
                    if let Some(tool_call) = tc.into_tool_call() {
                        let _ = tx.send(StreamEvent::ToolCall(tool_call)).await;
                    }
                }
                return Ok(());
            }

            let chunk: StreamChunk = match serde_json::from_str(data) {
                Ok(c) => c,
                Err(_) => continue,
            };

            for choice in &chunk.choices {
                // Handle text content
                if let Some(ref content) = choice.delta.content {
                    let _ = tx.send(StreamEvent::Token(content.clone())).await;
                }

                // Handle tool calls (streamed incrementally)
                if let Some(ref delta_tool_calls) = choice.delta.tool_calls {
                    for dtc in delta_tool_calls {
                        // Ensure we have enough slots
                        while tool_calls.len() <= dtc.index {
                            tool_calls.push(PartialToolCall::default());
                        }

                        let partial = &mut tool_calls[dtc.index];

                        if let Some(ref id) = dtc.id {
                            partial.id = Some(id.clone());
                        }
                        if let Some(ref func) = dtc.function {
                            if let Some(ref name) = func.name {
                                partial.name = Some(name.clone());
                            }
                            if let Some(ref args) = func.arguments {
                                partial.arguments.push_str(args);
                            }
                        }
                    }
                }

                // If finished with tool_calls reason, emit them
                if choice.finish_reason.as_deref() == Some("tool_calls") {
                    for tc in tool_calls.drain(..) {
                        if let Some(tool_call) = tc.into_tool_call() {
                            let _ = tx.send(StreamEvent::ToolCall(tool_call)).await;
                        }
                    }
                }
            }
        }
    }

    // Emit any remaining tool calls
    for tc in tool_calls.drain(..) {
        if let Some(tool_call) = tc.into_tool_call() {
            let _ = tx.send(StreamEvent::ToolCall(tool_call)).await;
        }
    }

    Ok(())
}

#[derive(Default)]
struct PartialToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl PartialToolCall {
    fn into_tool_call(self) -> Option<ToolCall> {
        Some(ToolCall {
            id: self.id?,
            call_type: "function".to_string(),
            function: FunctionCall {
                name: self.name?,
                arguments: self.arguments,
            },
        })
    }
}

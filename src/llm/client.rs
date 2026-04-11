use anyhow::{Result, Context};
use futures::StreamExt;
use reqwest::Client;
use std::sync::atomic::{AtomicU64, Ordering};
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
        // Activate the inline Hermes/Qwen tool-call parser only when we
        // actually registered tools for this request AND the user hasn't
        // disabled the fallback. This guarantees zero behavioral change for
        // roles like `summarize` (no_tools=true) and for backends that
        // already speak OpenAI tool_calls natively.
        let enable_fallback = self.config.tool_parse_fallback && tools.is_some();

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
            if let Err(e) = process_stream(response, &tx, enable_fallback).await {
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
    enable_fallback: bool,
) -> Result<()> {
    let mut stream = response.bytes_stream();
    // Byte buffer — SSE is line-framed by `\n` (ASCII), so we scan by byte
    // and only UTF-8 decode complete lines. Using `from_utf8_lossy` on raw
    // TCP chunks corrupts multi-byte characters split across packets.
    let mut buffer: Vec<u8> = Vec::new();

    // Accumulators for native OpenAI-shape tool calls.
    let mut tool_calls: Vec<PartialToolCall> = Vec::new();

    // Inline Hermes/Qwen XML tool-call parser. Only used when enable_fallback
    // is true. As soon as the backend emits a native tool_call delta, we
    // disable it (flushing any held-back text as plain tokens) so the two
    // paths can never double-fire.
    let mut hermes = HermesFilter::new(enable_fallback);

    // Runaway/repetition guard: accumulate emitted content bytes and bail
    // out if the tail shows the same block repeating 3+ times. Local
    // backends (mlx_lm.server w/ Qwen3-Coder) occasionally fail to stop on
    // EOS and loop forever on the same paragraph.
    let mut emitted_content: Vec<u8> = Vec::new();
    let mut last_repetition_check: usize = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Stream read error")?;
        buffer.extend_from_slice(&chunk);

        // Process complete SSE lines
        while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
            let line_bytes = buffer.drain(..=pos).collect::<Vec<u8>>();
            // Strip the trailing '\n' and trim whitespace. SSE line contents
            // (after `data: `) are always valid UTF-8 JSON or `[DONE]`.
            let line = match std::str::from_utf8(&line_bytes[..line_bytes.len() - 1]) {
                Ok(s) => s.trim().to_string(),
                Err(_) => continue,
            };

            if line.is_empty() || line.starts_with(':') {
                continue;
            }

            let data = line.strip_prefix("data: ").unwrap_or(&line);

            if data == "[DONE]" {
                flush_hermes_finish(&mut hermes, tx).await;
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
                    let (text_out, calls_out) = if hermes.active() {
                        hermes.feed(content)
                    } else {
                        (content.clone(), Vec::new())
                    };

                    if !text_out.is_empty() {
                        emitted_content.extend_from_slice(text_out.as_bytes());
                        let _ = tx.send(StreamEvent::Token(text_out)).await;

                        // Throttle the O(n·p) scan: only recheck every 64 new
                        // bytes once we're past the minimum window.
                        if emitted_content.len() >= last_repetition_check + 64
                            && looks_repetitive(&emitted_content)
                        {
                            let _ = tx
                                .send(StreamEvent::Error(
                                    "aborted: model stuck in a repetition loop (backend EOS not firing)"
                                        .to_string(),
                                ))
                                .await;
                            return Ok(());
                        }
                        last_repetition_check = emitted_content.len();
                    }
                    for tc in calls_out {
                        // Reset the repetition window when a tool call fires
                        // — a new turn after tool output is a clean slate.
                        emitted_content.clear();
                        last_repetition_check = 0;
                        let _ = tx.send(StreamEvent::ToolCall(tc)).await;
                    }
                }

                // Handle tool calls (streamed incrementally)
                if let Some(ref delta_tool_calls) = choice.delta.tool_calls {
                    // Backend speaks native OpenAI tool_calls — drop the
                    // fallback parser and flush whatever it was holding.
                    if hermes.active() {
                        let leftover = hermes.disable();
                        if !leftover.is_empty() {
                            let _ = tx.send(StreamEvent::Token(leftover)).await;
                        }
                    }

                    for dtc in delta_tool_calls {
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
                    flush_hermes_finish(&mut hermes, tx).await;
                    for tc in tool_calls.drain(..) {
                        if let Some(tool_call) = tc.into_tool_call() {
                            let _ = tx.send(StreamEvent::ToolCall(tool_call)).await;
                        }
                    }
                }
            }
        }
    }

    flush_hermes_finish(&mut hermes, tx).await;
    for tc in tool_calls.drain(..) {
        if let Some(tool_call) = tc.into_tool_call() {
            let _ = tx.send(StreamEvent::ToolCall(tool_call)).await;
        }
    }

    Ok(())
}

async fn flush_hermes_finish(hermes: &mut HermesFilter, tx: &mpsc::Sender<StreamEvent>) {
    let (text, calls) = hermes.finish();
    if !text.is_empty() {
        let _ = tx.send(StreamEvent::Token(text)).await;
    }
    for tc in calls {
        let _ = tx.send(StreamEvent::ToolCall(tc)).await;
    }
}

// ---------------------------------------------------------------------------
// Hermes / Qwen3-Coder inline tool-call parser
// ---------------------------------------------------------------------------
//
// Some local backends (notably mlx_lm.server with Qwen3-Coder) don't translate
// the model's `<tool_call>` / `<function=...>` XML tags into OpenAI structured
// tool_calls. The text just shows up in the content stream, so the agent
// loop never sees a ToolCall event and never executes the tool.
//
// HermesFilter sits in front of the SSE content delta and:
//   1. Holds back text that looks like the start of a tool-call tag
//   2. Once a complete tag is captured, parses it into a ToolCall
//   3. Otherwise streams content through unchanged with at most ~12 bytes
//      of latency (length of the longest start marker)
//
// Two formats are recognized:
//
//   Standard hermes JSON:
//     <tool_call>{"name": "bash", "arguments": {"command": "ls"}}</tool_call>
//
//   Qwen3-Coder XML (may or may not be wrapped in <tool_call>):
//     <function=bash>
//     <parameter=command>ls</parameter>
//     </function>

const START_TOOL_CALL: &str = "<tool_call>";
const END_TOOL_CALL: &str = "</tool_call>";
const START_FUNCTION: &str = "<function=";
const END_FUNCTION: &str = "</function>";

static FALLBACK_CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

fn fallback_call_id() -> String {
    let n = FALLBACK_CALL_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("call_fb_{}", n)
}

#[derive(Debug, Clone, Copy)]
enum InTool {
    /// Inside `<tool_call>...</tool_call>`. Body may itself contain `<function=...>`.
    ToolCall,
    /// Inside a bare `<function=...>...</function>`. The opening `<function=`
    /// has already been pushed into `tool_buf` so the body parser can see the
    /// function name.
    Function,
}

struct HermesFilter {
    enabled: bool,
    /// Unflushed text. May contain a partial start-marker prefix at the end.
    pending: String,
    /// When inside a tool call, accumulates the body text.
    tool_buf: String,
    in_tool: Option<InTool>,
}

impl HermesFilter {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            pending: String::new(),
            tool_buf: String::new(),
            in_tool: None,
        }
    }

    fn active(&self) -> bool {
        self.enabled
    }

    /// Disable the filter (e.g. backend started emitting native tool_calls).
    /// Returns any held-back text that should be flushed as a plain token.
    fn disable(&mut self) -> String {
        self.enabled = false;
        let mut out = std::mem::take(&mut self.tool_buf);
        out.push_str(&std::mem::take(&mut self.pending));
        self.in_tool = None;
        out
    }

    /// Feed a content chunk. Returns (text_to_emit, tool_calls_to_emit).
    fn feed(&mut self, chunk: &str) -> (String, Vec<ToolCall>) {
        if !self.enabled {
            return (chunk.to_string(), Vec::new());
        }
        self.pending.push_str(chunk);

        let mut emit_text = String::new();
        let mut emit_calls = Vec::new();

        loop {
            match self.in_tool {
                Some(mode) => {
                    let end_marker = match mode {
                        InTool::ToolCall => END_TOOL_CALL,
                        InTool::Function => END_FUNCTION,
                    };

                    if let Some(end_pos) = self.pending.find(end_marker) {
                        // For Function mode the body parser expects to see
                        // the closing tag, so include it in tool_buf.
                        let upto = match mode {
                            InTool::ToolCall => end_pos,
                            InTool::Function => end_pos + end_marker.len(),
                        };
                        self.tool_buf.push_str(&self.pending[..upto]);
                        let advance = end_pos + end_marker.len();
                        self.pending = self.pending[advance..].to_string();
                        self.in_tool = None;

                        let body = std::mem::take(&mut self.tool_buf);
                        if let Some((name, args)) = parse_hermes_body(&body) {
                            emit_calls.push(ToolCall {
                                id: fallback_call_id(),
                                call_type: "function".to_string(),
                                function: FunctionCall {
                                    name,
                                    arguments: args,
                                },
                            });
                        }
                        // Loop again — there could be another tool call right after.
                        continue;
                    } else {
                        // No end marker yet. Move most of pending into tool_buf
                        // but hold back the last few bytes in case the end
                        // marker is split across chunks.
                        let safe = char_safe_tail(&self.pending, end_marker.len());
                        self.tool_buf.push_str(&self.pending[..safe]);
                        self.pending = self.pending[safe..].to_string();
                        break;
                    }
                }
                None => {
                    let s_tool = self.pending.find(START_TOOL_CALL);
                    let s_func = self.pending.find(START_FUNCTION);
                    let next = match (s_tool, s_func) {
                        (Some(a), Some(b)) if a <= b => Some((a, InTool::ToolCall)),
                        (Some(_), Some(b)) => Some((b, InTool::Function)),
                        (Some(a), None) => Some((a, InTool::ToolCall)),
                        (None, Some(b)) => Some((b, InTool::Function)),
                        (None, None) => None,
                    };

                    if let Some((pos, mode)) = next {
                        emit_text.push_str(&self.pending[..pos]);
                        match mode {
                            InTool::ToolCall => {
                                let advance = pos + START_TOOL_CALL.len();
                                self.pending = self.pending[advance..].to_string();
                            }
                            InTool::Function => {
                                // Keep the `<function=` in tool_buf so the
                                // body parser can read the function name.
                                self.tool_buf.push_str(START_FUNCTION);
                                let advance = pos + START_FUNCTION.len();
                                self.pending = self.pending[advance..].to_string();
                            }
                        }
                        self.in_tool = Some(mode);
                        continue;
                    } else {
                        // No start marker. Flush most of pending, but hold
                        // back enough bytes to cover a possible split start
                        // marker (longest is `<tool_call>` = 11 bytes).
                        let safe = char_safe_tail(&self.pending, START_TOOL_CALL.len());
                        emit_text.push_str(&self.pending[..safe]);
                        self.pending = self.pending[safe..].to_string();
                        break;
                    }
                }
            }
        }

        (emit_text, emit_calls)
    }

    /// Stream ended. Flush whatever's left as plain text — if we were
    /// mid-tool-call, treat the partial as plain text rather than dropping it
    /// or fabricating an incomplete ToolCall.
    fn finish(&mut self) -> (String, Vec<ToolCall>) {
        if !self.enabled {
            let out = std::mem::take(&mut self.pending);
            return (out, Vec::new());
        }
        let mut out = std::mem::take(&mut self.tool_buf);
        out.push_str(&std::mem::take(&mut self.pending));
        self.in_tool = None;
        (out, Vec::new())
    }
}

/// Detect a runaway generation where the tail of `bytes` is the same block
/// repeating three or more times. Pure byte comparison — fine because we
/// only use it to decide whether to abort the stream.
fn looks_repetitive(bytes: &[u8]) -> bool {
    // Need at least 3 * 40 bytes of tail to bother checking.
    if bytes.len() < 120 {
        return false;
    }
    let max_period = (bytes.len() / 3).min(400);
    for period in 40..=max_period {
        let tail = &bytes[bytes.len() - period * 3..];
        let a = &tail[..period];
        let b = &tail[period..period * 2];
        let c = &tail[period * 2..];
        if a == b && b == c {
            return true;
        }
    }
    false
}

/// Return the largest byte index `<= s.len() - keep_bytes` that lies on a
/// UTF-8 char boundary. Used to safely split a string while holding back the
/// last `keep_bytes` bytes (or more, when the cut would land mid-codepoint).
fn char_safe_tail(s: &str, keep_bytes: usize) -> usize {
    if s.len() <= keep_bytes {
        return 0;
    }
    let mut i = s.len() - keep_bytes;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Parse a tool-call body. Returns (name, arguments_json_string).
///
/// Tries the JSON form first, then the Qwen XML `<function=...>` form.
fn parse_hermes_body(body: &str) -> Option<(String, String)> {
    let trimmed = body.trim();

    // Form A: hermes JSON.
    if let Some(brace_start) = trimmed.find('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed[brace_start..].trim()) {
            let name = v.get("name").and_then(|x| x.as_str());
            let args = v.get("arguments").or_else(|| v.get("parameters"));
            if let (Some(name), Some(args)) = (name, args) {
                let args_str = match args {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                return Some((name.to_string(), args_str));
            }
        }
    }

    // Form B: Qwen XML `<function=NAME>...<parameter=KEY>VALUE</parameter>...</function>`.
    let func_start = trimmed.find(START_FUNCTION)?;
    let after = &trimmed[func_start + START_FUNCTION.len()..];
    let name_end = after.find('>')?;
    let name = after[..name_end].trim().to_string();
    let mut inner = &after[name_end + 1..];
    if let Some(close) = inner.find(END_FUNCTION) {
        inner = &inner[..close];
    }

    let mut args = serde_json::Map::new();
    let mut rest = inner;
    while let Some(p_start) = rest.find("<parameter=") {
        let after_p = &rest[p_start + "<parameter=".len()..];
        let key_end = match after_p.find('>') {
            Some(e) => e,
            None => break,
        };
        let key = after_p[..key_end].trim().to_string();
        let val_rest = &after_p[key_end + 1..];
        let val_end = match val_rest.find("</parameter>") {
            Some(e) => e,
            None => break,
        };
        let value = val_rest[..val_end].trim_matches('\n').to_string();
        // Best-effort: if the value parses as a JSON scalar/object, store it
        // as that; otherwise store as a string. This matches what most tool
        // schemas expect.
        let json_val = serde_json::from_str::<serde_json::Value>(&value)
            .unwrap_or(serde_json::Value::String(value));
        args.insert(key, json_val);
        rest = &val_rest[val_end + "</parameter>".len()..];
    }

    Some((name, serde_json::Value::Object(args).to_string()))
}

#[cfg(test)]
mod hermes_tests {
    use super::*;

    fn drive(filter: &mut HermesFilter, chunks: &[&str]) -> (String, Vec<ToolCall>) {
        let mut text = String::new();
        let mut calls = Vec::new();
        for c in chunks {
            let (t, mut cs) = filter.feed(c);
            text.push_str(&t);
            calls.append(&mut cs);
        }
        let (t, mut cs) = filter.finish();
        text.push_str(&t);
        calls.append(&mut cs);
        (text, calls)
    }

    #[test]
    fn disabled_passes_text_through_unchanged() {
        let mut f = HermesFilter::new(false);
        let (text, calls) = drive(&mut f, &["hello ", "<tool_call>foo</tool_call>", " world"]);
        assert_eq!(text, "hello <tool_call>foo</tool_call> world");
        assert!(calls.is_empty());
    }

    #[test]
    fn plain_text_streams_through_with_no_tags() {
        let mut f = HermesFilter::new(true);
        let (text, calls) = drive(&mut f, &["hello ", "world ", "this is fine"]);
        assert_eq!(text, "hello world this is fine");
        assert!(calls.is_empty());
    }

    #[test]
    fn json_form_tool_call_extracted() {
        let mut f = HermesFilter::new(true);
        let body = r#"<tool_call>{"name":"bash","arguments":{"command":"ls"}}</tool_call>"#;
        let (text, calls) = drive(&mut f, &["before ", body, " after"]);
        assert_eq!(text, "before  after");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "bash");
        assert!(calls[0].function.arguments.contains("\"command\":\"ls\""));
    }

    #[test]
    fn xml_form_bare_function_extracted() {
        let mut f = HermesFilter::new(true);
        let body = "<function=bash>\n<parameter=command>\necho hi\n</parameter>\n</function>";
        let (text, calls) = drive(&mut f, &["see: ", body]);
        assert_eq!(text, "see: ");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "bash");
        assert!(calls[0].function.arguments.contains("\"command\""));
        assert!(calls[0].function.arguments.contains("echo hi"));
    }

    #[test]
    fn xml_form_wrapped_in_tool_call_extracted() {
        let mut f = HermesFilter::new(true);
        let body = "<tool_call>\n<function=bash>\n<parameter=command>\nls -la\n</parameter>\n</function>\n</tool_call>";
        let (_text, calls) = drive(&mut f, &[body]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "bash");
        assert!(calls[0].function.arguments.contains("ls -la"));
    }

    #[test]
    fn split_start_marker_across_chunks() {
        let mut f = HermesFilter::new(true);
        let (text, calls) = drive(
            &mut f,
            &[
                "answer: <too",
                "l_call>{\"name\":\"bash\",\"arguments\":",
                "{\"command\":\"pwd\"}}</tool_call> done",
            ],
        );
        assert_eq!(text, "answer:  done");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "bash");
    }

    #[test]
    fn split_end_marker_across_chunks() {
        let mut f = HermesFilter::new(true);
        let body = r#"<tool_call>{"name":"bash","arguments":{"command":"id"}}</tool"#;
        let (text, calls) = drive(&mut f, &[body, "_call> ok"]);
        assert_eq!(text, " ok");
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn two_consecutive_tool_calls() {
        let mut f = HermesFilter::new(true);
        let body = r#"<tool_call>{"name":"a","arguments":{}}</tool_call><tool_call>{"name":"b","arguments":{}}</tool_call>"#;
        let (text, calls) = drive(&mut f, &[body]);
        assert_eq!(text, "");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "a");
        assert_eq!(calls[1].function.name, "b");
    }

    #[test]
    fn unclosed_tool_call_flushed_as_text_on_finish() {
        let mut f = HermesFilter::new(true);
        let (text, calls) = drive(&mut f, &["before <tool_call>{\"name\":\"x\""]);
        assert!(calls.is_empty());
        assert!(text.contains("before "));
        assert!(text.contains("\"name\":\"x\""));
    }

    #[test]
    fn disable_midstream_flushes_buffered_text() {
        // No bytes should be lost across the feed -> disable boundary, even
        // when feed() is holding back a possible-marker prefix at the tail.
        let mut f = HermesFilter::new(true);
        let (t1, c1) = f.feed("partial <tool_ca");
        assert!(c1.is_empty());
        let leftover = f.disable();
        assert_eq!(format!("{}{}", t1, leftover), "partial <tool_ca");
    }

    #[test]
    fn repetition_detector_catches_triple_repeat() {
        let line = "这是一段重复的回复，模型卡住了，应该被中断。".repeat(1);
        let triple = line.repeat(3);
        assert!(looks_repetitive(triple.as_bytes()));
    }

    #[test]
    fn repetition_detector_ignores_normal_text() {
        let s = "a quick brown fox jumps over the lazy dog near the old stone bridge while the sun sets slowly behind the distant mountains";
        assert!(!looks_repetitive(s.as_bytes()));
    }

    #[test]
    fn utf8_held_back_safely() {
        let mut f = HermesFilter::new(true);
        let (text, calls) = drive(&mut f, &["你好世界 没有工具调用"]);
        assert_eq!(text, "你好世界 没有工具调用");
        assert!(calls.is_empty());
    }
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

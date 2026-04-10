use anyhow::Result;
use dom_smoothie::{Article, Readability};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use std::time::Duration;

use super::registry::{Tool, ToolResult};
use crate::llm::{FunctionDefinition, ToolDefinition};

const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

const MAX_BODY_BYTES: usize = 512 * 1024; // 512 KB raw HTML
const DEFAULT_MAX_CHARS: usize = 8000; // text returned to model

// Pre-compiled regexes (case-insensitive, dot-matches-newline)
static RE_SCRIPT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap());
static RE_STYLE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<style[^>]*>.*?</style>").unwrap());
static RE_TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]*>").unwrap());
static RE_WS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());

/// Strip HTML to plain text using the same approach as ArticleFlow:
/// remove <script>, <style>, then all tags, then collapse whitespace.
pub fn html_to_text(html: &str) -> String {
    let s = RE_SCRIPT.replace_all(html, "");
    let s = RE_STYLE.replace_all(&s, "");
    let s = RE_TAG.replace_all(&s, " ");
    RE_WS.replace_all(&s, " ").trim().to_string()
}

/// Readability extraction result
struct Extracted {
    title: Option<String>,
    byline: Option<String>,
    excerpt: Option<String>,
    text: String,
    source: &'static str, // "readability" or "regex"
}

/// Try readability first, fall back to regex stripping if it fails or
/// returns too little content (< 200 chars).
fn extract_article(html: &str, base_url: Option<&str>) -> Extracted {
    match Readability::new(html, base_url, None).and_then(|mut r| r.parse()) {
        Ok(article) => {
            let Article {
                title,
                byline,
                excerpt,
                text_content,
                ..
            } = article;
            let text = text_content.trim().to_string();
            if text.chars().count() >= 200 {
                return Extracted {
                    title: Some(title.to_string()).filter(|s| !s.is_empty()),
                    byline: byline.map(|s| s.to_string()).filter(|s| !s.is_empty()),
                    excerpt: excerpt.map(|s| s.to_string()).filter(|s| !s.is_empty()),
                    text,
                    source: "readability",
                };
            }
            // Readability succeeded but content was too sparse — fall back
        }
        Err(_) => {
            // Readability failed — fall back
        }
    }

    Extracted {
        title: None,
        byline: None,
        excerpt: None,
        text: html_to_text(html),
        source: "regex",
    }
}

pub struct WebFetchTool;

#[async_trait::async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "web_fetch".to_string(),
                description: "Fetch the contents of a URL and return it as clean text. By default uses the Readability algorithm (Mozilla Readability port) to extract the main article content, stripping nav/ads/sidebars. Returns title, byline, excerpt, and body text. Falls back to plain HTML stripping for non-article pages. Does NOT execute JavaScript — SPA sites may return empty content.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "The URL to fetch (must start with http:// or https://)"
                        },
                        "max_chars": {
                            "type": "integer",
                            "description": "Maximum number of characters to return (default: 8000)"
                        },
                        "mode": {
                            "type": "string",
                            "enum": ["auto", "readability", "raw"],
                            "description": "auto (default): try readability, fall back to raw strip. readability: force readability (fail if it errors). raw: skip readability, use regex stripping directly."
                        }
                    },
                    "required": ["url"]
                }),
            },
        }
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let url = args["url"].as_str().unwrap_or("");
        let max_chars = args["max_chars"].as_u64().unwrap_or(DEFAULT_MAX_CHARS as u64) as usize;
        let mode = args["mode"].as_str().unwrap_or("auto");

        if url.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: "Error: url is required".to_string(),
            });
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Ok(ToolResult {
                success: false,
                output: "Error: url must start with http:// or https://".to_string(),
            });
        }

        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(USER_AGENT)
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: format!("Failed to build HTTP client: {}", e),
                });
            }
        };

        let response = match client
            .get(url)
            .header("Accept", "text/html,application/xhtml+xml,*/*")
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: format!("Request failed: {}", e),
                });
            }
        };

        let status = response.status();
        if !status.is_success() {
            return Ok(ToolResult {
                success: false,
                output: format!("HTTP error: {}", status),
            });
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        // Read body up to MAX_BODY_BYTES
        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: format!("Failed to read body: {}", e),
                });
            }
        };
        let truncated_bytes = if bytes.len() > MAX_BODY_BYTES {
            &bytes[..MAX_BODY_BYTES]
        } else {
            &bytes[..]
        };
        let raw = String::from_utf8_lossy(truncated_bytes).to_string();

        // If JSON or plain text, return as-is (just clamp)
        let lower_ct = content_type.to_lowercase();
        let is_html = !(lower_ct.contains("json")
            || lower_ct.contains("text/plain")
            || lower_ct.contains("text/markdown"));

        let extracted = if !is_html {
            Extracted {
                title: None,
                byline: None,
                excerpt: None,
                text: raw,
                source: "passthrough",
            }
        } else {
            match mode {
                "raw" => Extracted {
                    title: None,
                    byline: None,
                    excerpt: None,
                    text: html_to_text(&raw),
                    source: "regex",
                },
                "readability" => {
                    match Readability::new(raw.as_str(), Some(url), None).and_then(|mut r| r.parse()) {
                        Ok(article) => Extracted {
                            title: Some(article.title.to_string())
                                .filter(|s| !s.is_empty()),
                            byline: article
                                .byline
                                .map(|s| s.to_string())
                                .filter(|s| !s.is_empty()),
                            excerpt: article
                                .excerpt
                                .map(|s| s.to_string())
                                .filter(|s| !s.is_empty()),
                            text: article.text_content.trim().to_string(),
                            source: "readability",
                        },
                        Err(e) => {
                            return Ok(ToolResult {
                                success: false,
                                output: format!("Readability extraction failed: {}", e),
                            });
                        }
                    }
                }
                _ => extract_article(&raw, Some(url)),
            }
        };

        // Build formatted output with metadata header
        let mut header = format!("URL: {}\n", url);
        header.push_str(&format!("Content-Type: {}\n", content_type));
        header.push_str(&format!("Extracted-By: {}\n", extracted.source));
        if let Some(ref t) = extracted.title {
            header.push_str(&format!("Title: {}\n", t));
        }
        if let Some(ref b) = extracted.byline {
            header.push_str(&format!("Byline: {}\n", b));
        }
        if let Some(ref e) = extracted.excerpt {
            header.push_str(&format!("Excerpt: {}\n", e));
        }
        header.push('\n');

        // Truncate body to max_chars (by char count, not byte)
        let chars: Vec<char> = extracted.text.chars().collect();
        let total_chars = chars.len();
        let truncated = total_chars > max_chars;
        let display: String = chars.into_iter().take(max_chars).collect();

        let mut output = format!("{}{}", header, display);
        if truncated {
            output.push_str(&format!(
                "\n\n... (truncated, {} of {} chars shown)",
                max_chars, total_chars
            ));
        }

        Ok(ToolResult {
            success: true,
            output,
        })
    }
}

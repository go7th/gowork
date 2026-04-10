use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use super::registry::{Tool, ToolResult};
use crate::llm::{FunctionDefinition, ToolDefinition};

const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

const DDG_URL: &str = "https://html.duckduckgo.com/html/";
const MIN_INTERVAL: Duration = Duration::from_secs(2);
const DEFAULT_COUNT: usize = 8;

// Global rate-limiter for DDG
static LAST_SEARCH: Lazy<Mutex<Option<Instant>>> = Lazy::new(|| Mutex::new(None));

// Pre-compiled regexes for parsing DDG HTML
static RE_LINK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?is)class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#).unwrap()
});
static RE_SNIPPET: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?is)class="result__snippet"[^>]*>(.*?)</a>"#).unwrap());
static RE_TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]*>").unwrap());

#[derive(Debug)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
    engines: Option<String>,
}

fn strip_tags(s: &str) -> String {
    RE_TAG.replace_all(s, "").trim().to_string()
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn unwrap_uddg(raw: &str) -> String {
    if !raw.contains("uddg=") {
        return raw.to_string();
    }
    if let Some(idx) = raw.find("uddg=") {
        let after = &raw[idx + 5..];
        let end = after.find('&').unwrap_or(after.len());
        let encoded = &after[..end];
        if let Ok(decoded) = urlencoding::decode(encoded) {
            return decoded.into_owned();
        }
    }
    raw.to_string()
}

// ---------------------------------------------------------------------------
// SearXNG JSON API backend
// ---------------------------------------------------------------------------

async fn search_searxng(
    base_url: &str,
    query: &str,
    count: usize,
) -> Result<Vec<SearchResult>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(USER_AGENT)
        .build()?;

    let url = format!("{}/search", base_url.trim_end_matches('/'));

    let response = client
        .get(&url)
        .query(&[
            ("q", query),
            ("format", "json"),
            ("categories", "general"),
        ])
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("SearXNG request failed: {}", e))?;

    if !response.status().is_success() {
        anyhow::bail!("SearXNG returned status: {}", response.status());
    }

    let body: Value = response.json().await?;

    let results_arr = body["results"].as_array();
    let mut results = Vec::new();

    if let Some(arr) = results_arr {
        for item in arr {
            if results.len() >= count {
                break;
            }
            let url_str = item["url"].as_str().unwrap_or("").to_string();
            if url_str.is_empty() || !url_str.starts_with("http") {
                continue;
            }
            let title = item["title"].as_str().unwrap_or("").to_string();
            let snippet = item["content"].as_str().unwrap_or("").to_string();

            // Gather which engines found this result
            let engines = item["engines"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                });

            results.push(SearchResult {
                title: decode_entities(&title),
                url: url_str,
                snippet: decode_entities(&snippet),
                engines,
            });
        }
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// DuckDuckGo HTML backend (fallback)
// ---------------------------------------------------------------------------

async fn ddg_rate_limit() {
    let mut guard = LAST_SEARCH.lock().await;
    if let Some(prev) = *guard {
        let elapsed = prev.elapsed();
        if elapsed < MIN_INTERVAL {
            tokio::time::sleep(MIN_INTERVAL - elapsed).await;
        }
    }
    *guard = Some(Instant::now());
}

async fn search_ddg(query: &str, count: usize) -> Result<Vec<SearchResult>> {
    ddg_rate_limit().await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| anyhow::anyhow!("client build failed: {}", e))?;

    let form = [("q", query)];

    let response = client
        .post(DDG_URL)
        .header("Referer", "https://html.duckduckgo.com/")
        .header("Accept", "text/html,application/xhtml+xml")
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .form(&form)
        .send()
        .await
        .map_err(|e| {
            let mut msg = format!("POST {} failed: {}", DDG_URL, e);
            let mut src = std::error::Error::source(&e);
            while let Some(s) = src {
                msg.push_str(&format!(" | caused by: {}", s));
                src = s.source();
            }
            anyhow::anyhow!(msg)
        })?;

    if !response.status().is_success() {
        anyhow::bail!("DuckDuckGo returned status: {}", response.status());
    }

    let html = response.text().await?;

    let links: Vec<_> = RE_LINK.captures_iter(&html).collect();
    let snippets: Vec<_> = RE_SNIPPET.captures_iter(&html).collect();

    let mut results = Vec::new();
    for (i, link_cap) in links.iter().enumerate() {
        if results.len() >= count {
            break;
        }
        let raw_url = link_cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        let raw_title = link_cap.get(2).map(|m| m.as_str()).unwrap_or("");

        let url = unwrap_uddg(raw_url);
        if !url.starts_with("http") || url.contains("duckduckgo.com") {
            continue;
        }

        let title = decode_entities(&strip_tags(raw_title));
        let snippet = snippets
            .get(i)
            .and_then(|c| c.get(1))
            .map(|m| decode_entities(&strip_tags(m.as_str())))
            .unwrap_or_default();

        results.push(SearchResult {
            title,
            url,
            snippet,
            engines: Some("duckduckgo".to_string()),
        });
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// Tool implementation
// ---------------------------------------------------------------------------

pub struct WebSearchTool {
    searxng_url: Option<String>,
}

impl WebSearchTool {
    pub fn new(searxng_url: Option<String>) -> Self {
        Self { searxng_url }
    }
}

#[async_trait::async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn definition(&self) -> ToolDefinition {
        let backend = if self.searxng_url.is_some() {
            "SearXNG"
        } else {
            "DuckDuckGo"
        };
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "web_search".to_string(),
                description: format!(
                    "Search the web using {} and return a list of result titles, URLs, and snippets. \
                     Use this to find information that may be more recent than your training data, \
                     or to discover relevant pages before fetching them with web_fetch.",
                    backend
                ),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query"
                        },
                        "count": {
                            "type": "integer",
                            "description": "Maximum number of results to return (default: 8, max: 20)"
                        }
                    },
                    "required": ["query"]
                }),
            },
        }
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let query = args["query"].as_str().unwrap_or("").trim();
        let count = args["count"].as_u64().unwrap_or(DEFAULT_COUNT as u64) as usize;
        let count = count.clamp(1, 20);

        if query.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: "Error: query is required".to_string(),
            });
        }

        // Try SearXNG first if configured, fall back to DDG
        let results = if let Some(ref surl) = self.searxng_url {
            match search_searxng(surl, query, count).await {
                Ok(r) => r,
                Err(e) => {
                    // SearXNG failed — fall back to DDG
                    eprintln!("[web_search] SearXNG failed ({}), falling back to DDG", e);
                    search_ddg(query, count).await.unwrap_or_default()
                }
            }
        } else {
            search_ddg(query, count).await?
        };

        if results.is_empty() {
            return Ok(ToolResult {
                success: true,
                output: format!("No results for: {}", query),
            });
        }

        let backend = if self.searxng_url.is_some() {
            "SearXNG"
        } else {
            "DuckDuckGo"
        };

        let mut out = format!(
            "Search results for \"{}\" (via {}):\n\n",
            query, backend
        );
        for (i, r) in results.iter().enumerate() {
            out.push_str(&format!("{}. {}\n   {}\n", i + 1, r.title, r.url));
            if !r.snippet.is_empty() {
                out.push_str(&format!("   {}\n", r.snippet));
            }
            if let Some(ref eng) = r.engines {
                out.push_str(&format!("   [engines: {}]\n", eng));
            }
            out.push('\n');
        }

        Ok(ToolResult {
            success: true,
            output: out.trim_end().to_string(),
        })
    }
}

mod agent;
mod cli;
mod config;
mod image;
mod llm;
mod session;
mod tools;

use anyhow::Result;
use clap::Parser;
use std::io::Read as _;

use cli::Shell;
use config::{load_file_config, resolve_llm_config};

mod cache;
pub mod stats;

#[derive(Parser)]
#[command(name = "gowork", version, about = "A high-performance coding assistant CLI")]
struct Args {
    /// LLM API base URL (default: http://localhost:11434/v1 for Ollama)
    #[arg(long, env = "GOWORK_BASE_URL")]
    base_url: Option<String>,

    /// Model name
    #[arg(long, short, env = "GOWORK_MODEL")]
    model: Option<String>,

    /// API key (optional, for OpenAI-compatible services)
    #[arg(long, env = "GOWORK_API_KEY")]
    api_key: Option<String>,

    /// SearXNG instance URL (overrides config file)
    #[arg(long, env = "GOWORK_SEARXNG_URL")]
    searxng_url: Option<String>,

    /// One-shot mode: run a single prompt and exit
    #[arg(long, short)]
    prompt: Option<String>,

    /// Disable tool registration (pure chat mode, faster for preprocessing)
    #[arg(long)]
    no_tools: bool,

    /// Read file content and prepend to prompt
    #[arg(long)]
    file: Option<String>,

    /// Batch mode: process multiple files matching a glob pattern.
    /// Use {} as placeholder for file content in the prompt.
    /// Example: --batch "docs/*.md" -p "one-line summary: {}"
    #[arg(long)]
    batch: Option<String>,

    /// Use cached result if available (cache key: file path + mtime + prompt)
    #[arg(long)]
    cache: bool,

    /// Show token savings statistics
    #[arg(long)]
    stats: bool,

    /// Reset token statistics
    #[arg(long)]
    stats_reset: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Resolve config: CLI > env (handled by clap) > config file > defaults
    let file_cfg = load_file_config();
    let llm_config = resolve_llm_config(args.base_url, args.model, args.api_key);
    let searxng_url = args.searxng_url.or(file_cfg.searxng_url);

    // Ensure config dirs exist
    let _ = config::ensure_dirs();

    // Stats commands
    if args.stats_reset {
        let mut s = stats::Stats::load();
        s.reset();
        eprintln!("Stats reset.");
        return Ok(());
    }
    if args.stats {
        let s = stats::Stats::load();
        println!("{}", s.display());
        return Ok(());
    }

    if let Some(ref batch_pattern) = args.batch {
        // Batch mode
        let prompt_template = args.prompt.unwrap_or_else(|| "summarize: {}".to_string());
        run_batch(&llm_config, batch_pattern, &prompt_template, args.no_tools, args.cache, searxng_url).await?;
    } else if let Some(prompt) = args.prompt {
        // One-shot mode
        let final_prompt = build_prompt(&prompt, args.file.as_deref())?;

        // Check cache
        if args.cache {
            if let Some(cached) = cache::get(&final_prompt, args.file.as_deref()) {
                print!("{}", cached);
                let mut s = stats::Stats::load();
                s.record_cache_hit(&final_prompt);
                return Ok(());
            }
        }

        let start = std::time::Instant::now();
        let output = run_oneshot(&llm_config, &final_prompt, args.no_tools, searxng_url).await?;
        let duration_ms = start.elapsed().as_millis() as u64;

        // Record stats
        let mut s = stats::Stats::load();
        s.record_call(&final_prompt, &output, duration_ms);

        // Save to cache
        if args.cache {
            cache::set(&final_prompt, args.file.as_deref(), &output);
        }
    } else {
        // Interactive mode
        let shell = Shell::new(llm_config, searxng_url);
        shell.run().await?;
    }

    Ok(())
}

/// Build the final prompt by combining stdin, --file, and -p content
fn build_prompt(prompt: &str, file_path: Option<&str>) -> Result<String> {
    let mut parts = Vec::new();

    // Read stdin if piped (not a terminal)
    if !atty::is(atty::Stream::Stdin) {
        let mut stdin_content = String::new();
        std::io::stdin().read_to_string(&mut stdin_content)?;
        if !stdin_content.trim().is_empty() {
            parts.push(stdin_content);
        }
    }

    // Read --file content
    if let Some(path) = file_path {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read file '{}': {}", path, e))?;
        parts.push(content);
    }

    // Combine: file/stdin content first, then the prompt instruction
    if parts.is_empty() {
        Ok(prompt.to_string())
    } else {
        parts.push(prompt.to_string());
        Ok(parts.join("\n\n"))
    }
}

/// Run a single one-shot prompt and return the output
async fn run_oneshot(
    llm_config: &llm::LlmConfig,
    prompt: &str,
    no_tools: bool,
    searxng_url: Option<String>,
) -> Result<String> {
    let mut registry = tools::ToolRegistry::new();

    if !no_tools {
        let todo_state = tools::new_todo_state();
        registry.register(Box::new(tools::ReadFileTool));
        registry.register(Box::new(tools::EditFileTool));
        registry.register(Box::new(tools::BashTool));
        registry.register(Box::new(tools::GrepTool));
        registry.register(Box::new(tools::GlobTool));
        registry.register(Box::new(tools::TodoWriteTool::new(todo_state)));
        registry.register(Box::new(tools::WebFetchTool));
        registry.register(Box::new(tools::WebSearchTool::new(searxng_url)));
    }

    let mut agent = agent::AgentLoop::new(llm_config.clone(), registry);

    let (cleaned, image_paths) = image::parse_image_refs(prompt);
    let mut data_urls = Vec::new();
    for p in image_paths {
        data_urls.push(image::image_to_data_url(&p)?);
    }

    let output = if data_urls.is_empty() {
        agent.process_capture(&cleaned).await?
    } else {
        agent.process_with_images_capture(&cleaned, data_urls).await?
    };

    Ok(output)
}

/// Batch mode: process multiple files matching a glob pattern
async fn run_batch(
    llm_config: &llm::LlmConfig,
    pattern: &str,
    prompt_template: &str,
    no_tools: bool,
    use_cache: bool,
    searxng_url: Option<String>,
) -> Result<()> {
    let paths: Vec<_> = glob::glob(pattern)
        .map_err(|e| anyhow::anyhow!("Invalid glob pattern '{}': {}", pattern, e))?
        .filter_map(|p| p.ok())
        .collect();

    if paths.is_empty() {
        eprintln!("No files matched pattern: {}", pattern);
        return Ok(());
    }

    eprintln!("Processing {} files...", paths.len());

    for path in &paths {
        let path_str = path.display().to_string();
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("  Skip {}: {}", path_str, e);
                continue;
            }
        };

        // Replace {} placeholder with file content, or append
        let prompt = if prompt_template.contains("{}") {
            prompt_template.replace("{}", &content)
        } else {
            format!("{}\n\n{}", content, prompt_template)
        };

        // Check cache
        if use_cache {
            if let Some(cached) = cache::get(&prompt, Some(&path_str)) {
                println!("=== {} ===", path_str);
                println!("{}", cached);
                println!();
                let mut s = stats::Stats::load();
                s.record_cache_hit(&prompt);
                continue;
            }
        }

        println!("=== {} ===", path_str);
        let start = std::time::Instant::now();
        let output = run_oneshot(llm_config, &prompt, no_tools, searxng_url.clone()).await?;
        let duration_ms = start.elapsed().as_millis() as u64;

        // Record stats
        let mut s = stats::Stats::load();
        s.record_call(&prompt, &output, duration_ms);

        // Cache result
        if use_cache {
            cache::set(&prompt, Some(&path_str), &output);
        }

        println!();
    }

    Ok(())
}

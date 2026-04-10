use anyhow::Result;
use colored::Colorize;
use rustyline::error::ReadlineError;
use rustyline::history::FileHistory;
use rustyline::{Config, Editor};
use std::time::Instant;

use crate::agent::AgentLoop;
use crate::config::config_dir;
use crate::image::{image_to_data_url, parse_image_refs};
use crate::llm::LlmConfig;
use crate::session::{list_sessions, load_session, save_session, Session};
use crate::tools::{
    new_todo_state, render_todo_list, BashTool, EditFileTool, GlobTool, GrepTool, ReadFileTool,
    TodoState, TodoWriteTool, ToolRegistry, WebFetchTool, WebSearchTool,
};

pub struct Shell {
    config: LlmConfig,
    searxng_url: Option<String>,
}

impl Shell {
    pub fn new(config: LlmConfig, searxng_url: Option<String>) -> Self {
        Self { config, searxng_url }
    }

    pub async fn run(&self) -> Result<()> {
        print_banner(&self.config, &self.searxng_url);

        let todo_state = new_todo_state();
        let mut agent = build_agent(self.config.clone(), todo_state.clone(), self.searxng_url.clone());

        // Set up readline with history
        let rl_config = Config::builder()
            .auto_add_history(true)
            .max_history_size(1000)?
            .build();
        let mut rl: Editor<(), FileHistory> = Editor::with_config(rl_config)?;
        let history_path = config_dir().join("history");
        let _ = std::fs::create_dir_all(config_dir());
        let _ = rl.load_history(&history_path);

        // Double Ctrl+C state: track timestamp of last Ctrl+C
        let mut last_ctrl_c: Option<Instant> = None;
        const CTRL_C_WINDOW_MS: u128 = 1500;

        loop {
            let prompt = format!("{} ", ">>".green().bold());
            match rl.readline(&prompt) {
                Ok(line) => {
                    last_ctrl_c = None;
                    let input = line.trim();
                    if input.is_empty() {
                        continue;
                    }

                    // Slash commands
                    if let Some(cmd) = input.strip_prefix('/') {
                        match handle_slash_command(
                            cmd,
                            &mut agent,
                            &todo_state,
                            &self.config,
                        )
                        .await
                        {
                            SlashResult::Continue => continue,
                            SlashResult::Quit => break,
                            SlashResult::Unknown => {
                                eprintln!(
                                    "{} Unknown command: /{}. Try /help",
                                    "?".yellow(),
                                    cmd
                                );
                                continue;
                            }
                        }
                    }

                    // Parse @image references
                    let (cleaned_text, image_paths) = parse_image_refs(input);

                    // Convert images to data URLs
                    let mut data_urls = Vec::new();
                    let mut image_error = false;
                    for path in &image_paths {
                        match image_to_data_url(path) {
                            Ok(url) => {
                                println!("{} {}", "Image:".cyan(), path.dimmed());
                                data_urls.push(url);
                            }
                            Err(e) => {
                                eprintln!("{}: {}", "Image error".red().bold(), e);
                                image_error = true;
                            }
                        }
                    }
                    if image_error {
                        continue;
                    }

                    // Process with agent
                    let result = if data_urls.is_empty() {
                        agent.process(&cleaned_text).await
                    } else {
                        agent.process_with_images(&cleaned_text, data_urls).await
                    };

                    if let Err(e) = result {
                        eprintln!("{}: {}", "Error".red().bold(), e);
                    }
                }
                Err(ReadlineError::Interrupted) => {
                    // Ctrl+C handling: single press clears input (rustyline already
                    // discarded the line), double press within window exits.
                    let now = Instant::now();
                    if let Some(prev) = last_ctrl_c {
                        if now.duration_since(prev).as_millis() < CTRL_C_WINDOW_MS {
                            println!("{}", "Bye!".dimmed());
                            break;
                        }
                    }
                    last_ctrl_c = Some(now);
                    println!(
                        "{}",
                        "(Press Ctrl+C again to exit, or Ctrl+D)".dimmed()
                    );
                    continue;
                }
                Err(ReadlineError::Eof) => {
                    println!("{}", "Bye!".dimmed());
                    break;
                }
                Err(e) => {
                    eprintln!("{}: {}", "Error".red().bold(), e);
                    break;
                }
            }
        }

        let _ = rl.save_history(&history_path);
        Ok(())
    }
}

enum SlashResult {
    Continue,
    Quit,
    Unknown,
}

async fn handle_slash_command(
    cmd: &str,
    agent: &mut AgentLoop,
    todo_state: &TodoState,
    config: &LlmConfig,
) -> SlashResult {
    let mut parts = cmd.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("");
    let args = parts.next().unwrap_or("").trim();

    match name {
        "quit" | "exit" | "q" => SlashResult::Quit,

        "clear" => {
            agent.reset();
            todo_state.lock().unwrap().clear();
            println!("{}", "Context cleared.".dimmed());
            SlashResult::Continue
        }

        "help" => {
            print_help();
            SlashResult::Continue
        }

        "todos" => {
            let todos = todo_state.lock().unwrap().clone();
            if todos.is_empty() {
                println!("{}", "No todos.".dimmed());
            } else {
                println!("\n{}", render_todo_list(&todos));
            }
            SlashResult::Continue
        }

        "save" => {
            if args.is_empty() {
                eprintln!("{} Usage: /save <name>", "?".yellow());
                return SlashResult::Continue;
            }
            let session = Session::new(
                args.to_string(),
                config.model.clone(),
                agent.messages(),
            );
            match save_session(&session) {
                Ok(path) => println!(
                    "{} {}",
                    "Saved:".green(),
                    path.display().to_string().dimmed()
                ),
                Err(e) => eprintln!("{}: {}", "Error".red().bold(), e),
            }
            SlashResult::Continue
        }

        "load" => {
            if args.is_empty() {
                // List sessions
                match list_sessions() {
                    Ok(names) if names.is_empty() => {
                        println!("{}", "No saved sessions.".dimmed());
                    }
                    Ok(names) => {
                        println!("{}", "Saved sessions:".bold());
                        for n in names {
                            println!("  {}", n.cyan());
                        }
                        println!("{}", "Use /load <name> to load.".dimmed());
                    }
                    Err(e) => eprintln!("{}: {}", "Error".red().bold(), e),
                }
                return SlashResult::Continue;
            }
            match load_session(args) {
                Ok(session) => {
                    agent.set_messages(session.messages);
                    println!(
                        "{} {} ({} messages)",
                        "Loaded:".green(),
                        session.name.cyan(),
                        agent.messages().len()
                    );
                }
                Err(e) => eprintln!("{}: {}", "Error".red().bold(), e),
            }
            SlashResult::Continue
        }

        "model" => {
            if args.is_empty() {
                // Show current model + list available
                println!(
                    "{} {}",
                    "Current model:".bold(),
                    agent.model().yellow()
                );
                match agent.list_available_models().await {
                    Ok(models) if models.is_empty() => {
                        println!("{}", "(no models reported by backend)".dimmed());
                    }
                    Ok(models) => {
                        println!("{}", "Available models:".bold());
                        for m in models {
                            let marker = if m == agent.model() {
                                "*".green().bold().to_string()
                            } else {
                                " ".to_string()
                            };
                            println!("  {} {}", marker, m.cyan());
                        }
                        println!(
                            "{}",
                            "Use /model <name> to switch.".dimmed()
                        );
                    }
                    Err(e) => {
                        eprintln!(
                            "{} {}",
                            "Could not list models:".red(),
                            e.to_string().dimmed()
                        );
                    }
                }
                return SlashResult::Continue;
            }
            // Switch model
            let new_model = args.to_string();
            agent.set_model(new_model.clone());
            println!(
                "{} {}",
                "Switched to model:".green(),
                new_model.yellow()
            );
            SlashResult::Continue
        }

        "sessions" => {
            match list_sessions() {
                Ok(names) if names.is_empty() => {
                    println!("{}", "No saved sessions.".dimmed());
                }
                Ok(names) => {
                    println!("{}", "Saved sessions:".bold());
                    for n in names {
                        println!("  {}", n.cyan());
                    }
                }
                Err(e) => eprintln!("{}: {}", "Error".red().bold(), e),
            }
            SlashResult::Continue
        }

        _ => SlashResult::Unknown,
    }
}

fn build_agent(config: LlmConfig, todo_state: TodoState, searxng_url: Option<String>) -> AgentLoop {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadFileTool));
    registry.register(Box::new(EditFileTool));
    registry.register(Box::new(BashTool));
    registry.register(Box::new(GrepTool));
    registry.register(Box::new(GlobTool));
    registry.register(Box::new(TodoWriteTool::new(todo_state)));
    registry.register(Box::new(WebFetchTool));
    registry.register(Box::new(WebSearchTool::new(searxng_url)));
    AgentLoop::new(config, registry)
}

fn print_banner(config: &LlmConfig, searxng_url: &Option<String>) {
    println!(
        "\n{}",
        "╔══════════════════════════════════════╗".cyan().bold()
    );
    println!(
        "{}",
        "║            gowork v0.1.0             ║".cyan().bold()
    );
    println!(
        "{}",
        "╚══════════════════════════════════════╝".cyan().bold()
    );
    println!(
        "  {} {} ({})",
        "Model:".dimmed(),
        config.model.yellow(),
        config.base_url.dimmed()
    );
    println!(
        "  {} {}",
        "CWD:".dimmed(),
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    );
    let search_backend = match searxng_url {
        Some(u) => format!("SearXNG ({})", u),
        None => "DuckDuckGo".to_string(),
    };
    println!(
        "  {} {}",
        "Search:".dimmed(),
        search_backend.dimmed()
    );
    println!(
        "  {} {}",
        "Tips:".dimmed(),
        "type /help, attach images with @./image.png".dimmed()
    );
    println!();
}

fn print_help() {
    println!("\n{}", "Commands:".bold());
    println!("  {}                 Show this help", "/help".green());
    println!("  {}                Clear conversation context", "/clear".green());
    println!("  {}                Show current todo list", "/todos".green());
    println!("  {} {}     Show or switch model", "/model".green(), "[name]".dimmed());
    println!("  {} {}      Save current session", "/save".green(), "<name>".dimmed());
    println!("  {} {}      Load a saved session", "/load".green(), "<name>".dimmed());
    println!("  {}             List saved sessions", "/sessions".green());
    println!("  {}                 Exit (or double Ctrl+C)", "/quit".green());

    println!("\n{}", "Image input:".bold());
    println!(
        "  Use {} in your prompt to attach an image",
        "@./path/to/image.png".cyan()
    );
    println!("  Example: {}", "describe @./screenshot.png".dimmed());

    println!("\n{}", "Tools:".bold());
    println!("  {}   - Read file contents", "read_file".cyan());
    println!("  {}   - Edit files (find & replace, with diff)", "edit_file".cyan());
    println!("  {}        - Execute shell commands", "bash".cyan());
    println!("  {}        - Search file contents (regex)", "grep".cyan());
    println!("  {}        - Find files by glob pattern", "glob".cyan());
    println!("  {}  - Manage multi-step task list", "todo_write".cyan());
    println!("  {}   - Fetch a URL and return plain text", "web_fetch".cyan());
    println!("  {}  - Search the web (DuckDuckGo)", "web_search".cyan());

    println!("\n{}", "Keybindings:".bold());
    println!("  {}    Browse history", "↑/↓".cyan());
    println!("  {}     Clear current input (single press)", "Ctrl+C".cyan());
    println!("  {}     Exit (double press, within 1.5s)", "Ctrl+C".cyan());
    println!("  {}     Exit (EOF)", "Ctrl+D".cyan());
    println!();
}

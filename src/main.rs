//! Gugugaga CLI
//!
//! A gugugaga agent that wraps Codex to monitor and correct its behavior.

use clap::{Args, CommandFactory, Parser, Subcommand};
use gugugaga::issues::{
    self, CreateIssueInput, IssueStore, ListIssuesOptions, ListSort, UpdateIssueInput,
};
use gugugaga::trust;
use gugugaga::tui::App;
use gugugaga::{GugugagaConfig, Interceptor};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

/// Gugugaga - Monitor and correct Codex behavior
#[derive(Parser, Debug)]
#[command(name = "gugugaga")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Working directory for the project
    #[arg(short = 'C', long, default_value = ".")]
    cwd: PathBuf,

    /// Path to the persistent memory file
    #[arg(long)]
    memory_file: Option<PathBuf>,

    /// Strict mode: interrupt on any violation
    #[arg(long)]
    strict: bool,

    /// Verbose output: show gugugaga evaluations
    #[arg(short, long)]
    verbose: bool,

    /// Disable TUI and use plain text mode
    #[arg(long)]
    no_tui: bool,

    /// Extra command groups
    #[command(subcommand)]
    command: Option<Commands>,

    /// Initial prompt to send to Codex
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Built-in local issue tracker
    Issues(IssuesArgs),
}

#[derive(Args, Debug)]
struct IssuesArgs {
    /// Print JSONL output for machine consumption
    #[arg(long, global = true)]
    json: bool,

    /// Remove existing lock before write operations
    #[arg(long, global = true)]
    force: bool,

    /// Disable ANSI colors in text output
    #[arg(long, global = true)]
    no_color: bool,

    #[command(subcommand)]
    command: IssuesCommand,
}

#[derive(Subcommand, Debug)]
enum IssuesCommand {
    /// Initialize .issues/issues.jsonl in the workspace
    Init,
    /// Print AGENTS.md onboarding snippet
    Onboard,
    /// Print workflow guidance prompt
    Prime,
    /// Start local web viewer
    Serve {
        /// Bind address
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Bind port
        #[arg(long, default_value_t = 7385)]
        port: u16,
    },
    /// Create a new issue
    Create {
        /// Issue title
        #[arg(required = true, num_args = 1..)]
        title: Vec<String>,
        /// Description
        #[arg(long, default_value = "")]
        description: String,
        /// Priority (smaller means more important)
        #[arg(short = 'p', long, default_value_t = 2)]
        priority: u8,
        /// Dependencies (repeat or pass comma-separated values)
        #[arg(long, value_delimiter = ',')]
        deps: Vec<String>,
        /// Notes
        #[arg(long, default_value = "")]
        notes: String,
    },
    /// List issues
    List {
        /// Filter by status (open, in_progress, blocked, closed)
        #[arg(long)]
        status: Option<String>,
        /// Filter by priority
        #[arg(short = 'p', long)]
        priority: Option<u8>,
        /// Only show ready issues (all deps closed)
        #[arg(long)]
        ready: bool,
        /// Include closed issues when --status is not set
        #[arg(long)]
        all: bool,
        /// Search in title/description/notes
        #[arg(long)]
        search: Option<String>,
        /// Sort mode (supports: priority)
        #[arg(long)]
        sort: Option<String>,
    },
    /// Show a single issue
    Show { id: String },
    /// Mark issue status
    Status { id: String, status: String },
    /// Mark issue closed
    Close { id: String },
    /// List ready issues
    Ready,
    /// Update fields on an issue
    Update {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long)]
        append_notes: Option<String>,
        #[arg(short = 'p', long)]
        priority: Option<u8>,
    },
    /// Delete an issue
    Delete { id: String },
    /// Manage dependencies
    Dep {
        #[command(subcommand)]
        command: DepCommand,
    },
}

#[derive(Subcommand, Debug)]
enum DepCommand {
    /// Add dependency child -> parent
    Add { child: String, parent: String },
    /// Remove dependency child -> parent
    Remove { child: String, parent: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install panic hook that writes to a crash log file
    // so we can debug "flash-exit" issues.
    std::panic::set_hook(Box::new(|info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        let msg = format!(
            "gugugaga crashed!\n{}\nBacktrace:\n{:?}\n",
            info,
            std::backtrace::Backtrace::capture()
        );
        let _ = std::fs::write("gugugaga-crash.log", &msg);
        eprintln!("{msg}");
    }));

    let cli = Cli::parse();

    // Treat stray top-level words as unknown commands in normal mode.
    // This avoids accidentally treating `gugugaga xxx` as a prompt.
    if cli.command.is_none() && !cli.prompt.is_empty() && !cli.no_tui {
        eprintln!("error: unrecognized command '{}'\n", cli.prompt[0]);
        let mut cmd = Cli::command();
        let _ = cmd.write_help(&mut std::io::stderr());
        eprintln!();
        std::process::exit(2);
    }

    if let Some(Commands::Issues(args)) = &cli.command {
        let root = gugugaga::issues::workspace_root(&cli.cwd)?;
        run_issues_command(&root, args)?;
        return Ok(());
    }

    // Resolve paths
    let cwd = std::fs::canonicalize(&cli.cwd)?;
    let codex_home = get_codex_home()?;

    // Get project name from cwd
    let project_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    if cli.no_tui {
        // Plain text mode
        run_plain_mode(cli, cwd, codex_home).await
    } else {
        // TUI mode
        run_tui_mode(cli, cwd, codex_home, project_name).await
    }
}

async fn run_tui_mode(
    cli: Cli,
    cwd: PathBuf,
    codex_home: PathBuf,
    project_name: String,
) -> anyhow::Result<()> {
    // ── Trust directory onboarding ──
    // Check if trust decision is needed — actual prompt is shown in the TUI
    // Welcome phase (animation + trust UI together, like Codex).
    let trust_ctx = trust::check_trust(&codex_home, &cwd);

    // Create config
    let mut config = GugugagaConfig::new(cwd.clone(), codex_home)
        .with_strict_mode(cli.strict)
        .with_verbose(cli.verbose);

    if let Some(memory_file) = cli.memory_file {
        config = config.with_memory_file(memory_file);
    }

    // Create TUI app
    let cwd_str = cwd.to_string_lossy().to_string();
    let mut app = App::new(project_name, cwd_str, trust_ctx)?;

    // Setup channels for communication
    let (user_input_tx, user_input_rx) = mpsc::channel::<String>(32);
    let (output_tx, output_rx) = mpsc::channel::<String>(32);

    // Set channels on app
    app.set_channels(user_input_tx.clone(), output_rx);

    // Create interceptor — if this fails, restore terminal first
    let interceptor = match Interceptor::new(config).await {
        Ok(i) => i,
        Err(e) => {
            drop(app);
            eprintln!("\x1b[1;31mError:\x1b[0m Failed to start Codex backend: {e}");
            eprintln!("Make sure codex app-server is installed and accessible.");
            std::process::exit(1);
        }
    };

    // Share notebook with TUI so the right-side panel updates live
    let notebook = interceptor.notebook();
    app.set_notebook(notebook);

    // Run interceptor in background — capture errors so we can log them
    let mut interceptor_handle = tokio::spawn(async move {
        if let Err(e) = interceptor.run(user_input_rx, output_tx).await {
            let msg = format!("Interceptor error: {e}\n");
            let _ = std::fs::write("gugugaga-crash.log", &msg);
        }
    });

    // Send initialize sequence to app-server
    // Step 1: initialize request
    let init_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialize",
        "id": 0,
        "params": {
            "clientInfo": {
                "name": "codex-gugugaga",
                "title": "Gugugaga",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": {
                "experimentalApi": true
            }
        }
    })
    .to_string();
    let _ = user_input_tx.send(init_msg).await;

    // Give app-server time to process initialize
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Step 2: initialized notification (required after initialize response)
    let initialized_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized"
    })
    .to_string();
    let _ = user_input_tx.send(initialized_msg).await;

    // Step 3: Start a thread with workspace-write sandbox so Codex can
    // actually create/edit files.  The app-server default is read-only.
    let thread_start_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "thread/start",
        "id": 1,
        "params": {
            "sandbox": "workspace-write",
            "approvalPolicy": "on-request",
            "config": {
                "experimental_use_freeform_apply_patch": true
            }
        }
    })
    .to_string();
    let _ = user_input_tx.send(thread_start_msg).await;

    // Run TUI (this blocks until quit)
    let run_result = app.run().await;

    // Drop app first to restore terminal before printing any error
    drop(app);

    // Drop the remaining sender so the interceptor's user_input_rx.recv()
    // returns None, allowing its main loop to exit and save the session.
    drop(user_input_tx);

    // Wait for interceptor to finish saving session (with timeout).
    // If it doesn't finish in time, abort it so the process can exit.
    let save_timeout = tokio::time::Duration::from_secs(3);
    if tokio::time::timeout(save_timeout, &mut interceptor_handle)
        .await
        .is_err()
    {
        interceptor_handle.abort();
    }

    // Now check if the TUI run had an error
    if let Err(e) = run_result {
        let msg = format!(
            "TUI error: {e}\nBacktrace:\n{:?}\n",
            std::backtrace::Backtrace::capture()
        );
        let _ = std::fs::write("gugugaga-crash.log", &msg);
        eprintln!("\x1b[1;31mError:\x1b[0m TUI exited unexpectedly: {e}");
        eprintln!("Details saved to gugugaga-crash.log");
        std::process::exit(1);
    }

    Ok(())
}

async fn run_plain_mode(cli: Cli, cwd: PathBuf, codex_home: PathBuf) -> anyhow::Result<()> {
    // Setup logging for plain mode
    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    info!("Starting Gugugaga (plain mode)");
    info!("Working directory: {:?}", cwd);
    info!("Codex home: {:?}", codex_home);

    // Create config
    let mut config = GugugagaConfig::new(cwd.clone(), codex_home)
        .with_strict_mode(cli.strict)
        .with_verbose(cli.verbose);

    if let Some(memory_file) = cli.memory_file {
        config = config.with_memory_file(memory_file);
    }

    // Create interceptor
    let interceptor = Interceptor::new(config).await?;

    // Setup channels for communication
    let (user_input_tx, user_input_rx) = mpsc::channel::<String>(32);
    let (output_tx, mut output_rx) = mpsc::channel::<String>(32);

    // Spawn blocking task to read from stdin
    let stdin_tx = user_input_tx.clone();
    std::thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => {
                    if stdin_tx.blocking_send(line).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("Error reading stdin: {}", e);
                    break;
                }
            }
        }
    });

    // Spawn task to write to stdout
    tokio::spawn(async move {
        while let Some(msg) = output_rx.recv().await {
            if let Err(e) = writeln!(io::stdout(), "{}", msg) {
                error!("Error writing stdout: {}", e);
                break;
            }
            let _ = io::stdout().flush();
        }
    });

    // Send initial prompt if provided
    if !cli.prompt.is_empty() {
        let prompt_text = cli.prompt.join(" ");
        let init_msg = create_init_message(&prompt_text);
        user_input_tx.send(init_msg).await?;
    }

    // Run the interceptor
    interceptor.run(user_input_rx, output_tx).await?;

    Ok(())
}

fn run_issues_command(workspace_root: &Path, args: &IssuesArgs) -> anyhow::Result<()> {
    let store = IssueStore::new(workspace_root);
    let color = !args.no_color;

    match &args.command {
        IssuesCommand::Init => {
            let (path, doc_change) = store.init(args.force)?;
            if args.json {
                write_json_line(&serde_json::json!({
                    "action": "init",
                    "path": path.to_string_lossy(),
                    "agents": doc_change,
                }))?;
            } else {
                println!("initialized {}", path.display());
                if let Some(msg) = doc_change {
                    println!("{msg}");
                }
            }
        }
        IssuesCommand::Onboard => {
            print!("{}", issues::onboard_text());
        }
        IssuesCommand::Prime => {
            print!("{}", issues::prime_prompt());
        }
        IssuesCommand::Serve { host, port } => {
            println!("serving http://{}:{}", host, port);
            issues::serve(&store, host, *port)?;
        }
        IssuesCommand::Create {
            title,
            description,
            priority,
            deps,
            notes,
        } => {
            let issue = store.with_lock(args.force, |s| {
                s.create_issue(
                    &title.join(" "),
                    CreateIssueInput {
                        description: description.clone(),
                        priority: *priority,
                        deps: deps.clone(),
                        notes: notes.clone(),
                    },
                )
            })?;
            if args.json {
                write_json_line(&issue)?;
            } else {
                println!("{}", issue.id);
            }
        }
        IssuesCommand::List {
            status,
            priority,
            ready,
            all,
            search,
            sort,
        } => {
            let sort_mode = match sort.as_deref() {
                None => ListSort::CreatedDesc,
                Some("priority") => ListSort::Priority,
                Some(other) => return Err(anyhow::anyhow!("unknown sort: {other}")),
            };
            let issues = store.list_issues(&ListIssuesOptions {
                status: status.clone(),
                priority: *priority,
                ready_only: *ready,
                include_closed: *all,
                search: search.clone(),
                sort: sort_mode,
            })?;
            if issues.is_empty() && !args.json {
                println!("no issues");
                return Ok(());
            }
            for issue in &issues {
                if args.json {
                    write_json_line(issue)?;
                } else {
                    println!("{}", issues::format_issue_line(issue, color));
                }
            }
        }
        IssuesCommand::Show { id } => {
            let issue = store
                .get_issue(id)?
                .ok_or_else(|| anyhow::anyhow!("issue not found: {id}"))?;
            if args.json {
                write_json_line(&issue)?;
            } else {
                for line in issues::format_issue_details(&issue, color) {
                    println!("{line}");
                }
            }
        }
        IssuesCommand::Status { id, status } => {
            let issue = store.with_lock(args.force, |s| s.set_status(id, status))?;
            if args.json {
                write_json_line(&issue)?;
            } else {
                println!("updated {} -> {}", issue.id, issue.status);
            }
        }
        IssuesCommand::Close { id } => {
            let issue = store.with_lock(args.force, |s| s.close_issue(id))?;
            if args.json {
                write_json_line(&issue)?;
            } else {
                println!("updated {} -> {}", issue.id, issue.status);
            }
        }
        IssuesCommand::Ready => {
            let issues = store.ready_issues()?;
            if issues.is_empty() && !args.json {
                println!("no ready issues");
                return Ok(());
            }
            for issue in &issues {
                if args.json {
                    write_json_line(issue)?;
                } else {
                    println!("{}", issues::format_issue_line(issue, color));
                }
            }
        }
        IssuesCommand::Update {
            id,
            title,
            description,
            notes,
            append_notes,
            priority,
        } => {
            let issue = store.with_lock(args.force, |s| {
                s.update_issue(
                    id,
                    UpdateIssueInput {
                        title: title.clone(),
                        description: description.clone(),
                        notes: notes.clone(),
                        append_notes: append_notes.clone(),
                        priority: *priority,
                    },
                )
            })?;
            if args.json {
                write_json_line(&issue)?;
            } else {
                println!("updated {}", issue.id);
            }
        }
        IssuesCommand::Delete { id } => {
            store.with_lock(args.force, |s| s.delete_issue(id))?;
            if args.json {
                write_json_line(&serde_json::json!({
                    "action": "delete",
                    "id": id,
                }))?;
            } else {
                println!("deleted {}", id);
            }
        }
        IssuesCommand::Dep { command } => match command {
            DepCommand::Add { child, parent } => {
                let issue = store.with_lock(args.force, |s| s.add_dependency(child, parent))?;
                if args.json {
                    write_json_line(&issue)?;
                } else {
                    println!("added dependency {} -> {}", child, parent);
                }
            }
            DepCommand::Remove { child, parent } => {
                let issue = store.with_lock(args.force, |s| s.remove_dependency(child, parent))?;
                if args.json {
                    write_json_line(&issue)?;
                } else {
                    println!("removed dependency {} -> {}", child, parent);
                }
            }
        },
    }

    Ok(())
}

fn write_json_line<T: serde::Serialize>(value: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

/// Get the Codex home directory
fn get_codex_home() -> anyhow::Result<PathBuf> {
    // Check CODEX_HOME env var
    if let Ok(home) = std::env::var("CODEX_HOME") {
        return Ok(PathBuf::from(home));
    }

    // Default to ~/.codex
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
    Ok(home.join(".codex"))
}

/// Create initialization message for app-server
fn create_init_message(_prompt: &str) -> String {
    serde_json::json!({
        "method": "initialize",
        "id": 0,
        "params": {
            "clientInfo": {
                "name": "codex-gugugaga",
                "title": "Gugugaga",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": {
                "experimentalApi": true
            }
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_codex_home() {
        // Should not panic
        let result = get_codex_home();
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_init_message() {
        let msg = create_init_message("test prompt");
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(parsed["method"], "initialize");
        assert_eq!(parsed["params"]["capabilities"]["experimentalApi"], true);
    }
}

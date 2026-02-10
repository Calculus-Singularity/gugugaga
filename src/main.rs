//! Gugugaga CLI
//!
//! A gugugaga agent that wraps Codex to monitor and correct its behavior.

use clap::Parser;
use gugugaga::{Interceptor, GugugagaConfig};
use gugugaga::tui::App;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
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

    /// Initial prompt to send to Codex
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

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
    // Create config
    let mut config = GugugagaConfig::new(cwd.clone(), codex_home)
        .with_strict_mode(cli.strict)
        .with_verbose(cli.verbose);

    if let Some(memory_file) = cli.memory_file {
        config = config.with_memory_file(memory_file);
    }

    // Create TUI app
    let cwd_str = cwd.to_string_lossy().to_string();
    let mut app = App::new(project_name, cwd_str)?;

    // Setup channels for communication
    let (user_input_tx, user_input_rx) = mpsc::channel::<String>(32);
    let (output_tx, output_rx) = mpsc::channel::<String>(32);

    // Set channels on app
    app.set_channels(user_input_tx.clone(), output_rx);

    // Create interceptor
    let interceptor = Interceptor::new(config).await?;

    // Share notebook with TUI so the right-side panel updates live
    let notebook = interceptor.notebook();
    app.set_notebook(notebook);

    // Run interceptor in background
    let interceptor_handle = tokio::spawn(async move {
        let _ = interceptor.run(user_input_rx, output_tx).await;
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

    // Step 3: Start a thread
    let thread_start_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "thread/start",
        "id": 1,
        "params": {}
    })
    .to_string();
    let _ = user_input_tx.send(thread_start_msg).await;

    // Run TUI (this blocks until quit)
    app.run().await?;

    // Cleanup
    interceptor_handle.abort();

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
    }
}

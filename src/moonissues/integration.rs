//! Moonissues CLI integration

use crate::{Result, GugugagaError};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;
use tracing::{debug, info, warn};

/// Integration with moonissues issue tracker
pub struct MoonissuesIntegration;

#[derive(Debug, Clone, Deserialize)]
pub struct Issue {
    pub id: String,
    pub title: String,
    pub status: String,
    pub priority: Option<i32>,
    pub notes: Option<String>,
}

impl MoonissuesIntegration {
    /// Check if moonissues is available in PATH
    pub fn is_available() -> bool {
        Command::new("moonissues")
            .arg("--help")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Ensure moonissues is initialized in the project directory
    pub fn ensure_initialized(cwd: &Path) -> Result<()> {
        let issues_dir = cwd.join(".issues");

        if issues_dir.exists() {
            debug!("moonissues already initialized at {:?}", cwd);
            return Ok(());
        }

        if !Self::is_available() {
            warn!("moonissues not found in PATH, skipping initialization");
            return Ok(());
        }

        info!("Initializing moonissues in {:?}", cwd);

        let output = Command::new("moonissues")
            .arg("init")
            .current_dir(cwd)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GugugagaError::Moonissues(format!(
                "Failed to initialize moonissues: {}",
                stderr
            )));
        }

        info!("moonissues initialized successfully");
        Ok(())
    }

    /// Get list of issues from moonissues
    pub fn get_issues(cwd: &Path) -> Result<Vec<Issue>> {
        if !Self::is_available() {
            return Ok(Vec::new());
        }

        let output = Command::new("moonissues")
            .args(["list", "--json"])
            .current_dir(cwd)
            .output()?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut issues = Vec::new();

        // Parse JSONL output (one JSON object per line)
        for line in stdout.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(issue) = serde_json::from_str::<Issue>(line) {
                issues.push(issue);
            }
        }

        Ok(issues)
    }

    /// Get ready issues (no blockers)
    pub fn get_ready_issues(cwd: &Path) -> Result<Vec<Issue>> {
        if !Self::is_available() {
            return Ok(Vec::new());
        }

        let output = Command::new("moonissues")
            .args(["ready", "--json"])
            .current_dir(cwd)
            .output()?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut issues = Vec::new();

        for line in stdout.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(issue) = serde_json::from_str::<Issue>(line) {
                issues.push(issue);
            }
        }

        Ok(issues)
    }

    /// Format issues for inclusion in gugugaga context
    pub fn format_issues_for_context(issues: &[Issue]) -> String {
        if issues.is_empty() {
            return "No active issues in moonissues.".to_string();
        }

        let mut output = String::from("=== Current Issues (moonissues) ===\n");
        for issue in issues {
            output.push_str(&format!(
                "- [{}] {} ({})\n",
                issue.id, issue.title, issue.status
            ));
            if let Some(notes) = &issue.notes {
                if !notes.is_empty() {
                    output.push_str(&format!("  Notes: {}\n", notes));
                }
            }
        }
        output
    }

    /// Create a new issue
    pub fn create_issue(cwd: &Path, title: &str) -> Result<String> {
        if !Self::is_available() {
            return Err(GugugagaError::Moonissues("moonissues not available".to_string()));
        }

        let output = Command::new("moonissues")
            .args(["create", title])
            .current_dir(cwd)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GugugagaError::Moonissues(format!(
                "Failed to create issue: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.trim().to_string())
    }

    /// Close an issue
    pub fn close_issue(cwd: &Path, id: &str) -> Result<()> {
        if !Self::is_available() {
            return Err(GugugagaError::Moonissues("moonissues not available".to_string()));
        }

        let output = Command::new("moonissues")
            .args(["close", id])
            .current_dir(cwd)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GugugagaError::Moonissues(format!(
                "Failed to close issue: {}",
                stderr
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_issues() {
        let issues = vec![
            Issue {
                id: "1".to_string(),
                title: "Implement login".to_string(),
                status: "open".to_string(),
                priority: Some(1),
                notes: None,
            },
            Issue {
                id: "2".to_string(),
                title: "Add tests".to_string(),
                status: "in_progress".to_string(),
                priority: Some(2),
                notes: Some("Need unit tests".to_string()),
            },
        ];

        let output = MoonissuesIntegration::format_issues_for_context(&issues);
        assert!(output.contains("[1] Implement login"));
        assert!(output.contains("[2] Add tests"));
        assert!(output.contains("Need unit tests"));
    }

    #[test]
    fn test_format_empty_issues() {
        let output = MoonissuesIntegration::format_issues_for_context(&[]);
        assert!(output.contains("No active issues"));
    }
}

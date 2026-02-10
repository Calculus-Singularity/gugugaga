//! Trust directory onboarding — aligned with Codex behavior.
//!
//! Before spawning the app-server, check whether the current working directory
//! has a `trust_level` entry in `~/.codex/config.toml`.  If not, present a
//! blocking prompt so the user can choose:
//!
//! * **Trust** — sets `trust_level = "trusted"` → `AskForApproval::OnRequest`
//!   (safe commands auto-approved, sandbox enforced for the rest)
//! * **Don't trust** — sets `trust_level = "untrusted"` →
//!   `AskForApproval::UnlessTrusted` (everything except safe-list needs
//!   approval)
//!
//! The prompt mirrors what Codex's own TUI shows during onboarding.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

// ────────────────────────────────────────────
// Public entry point
// ────────────────────────────────────────────

/// Ensure the user has made a trust decision for `cwd`.
///
/// If a decision already exists in `config.toml`, this is a no-op.
/// Otherwise, an interactive prompt is printed to the terminal and the
/// result is persisted exactly the way Codex does it.
pub fn ensure_trust_decision(codex_home: &Path, cwd: &Path) -> anyhow::Result<()> {
    let project_key = resolve_project_key(cwd);
    let config_path = codex_home.join("config.toml");

    // Read existing config (or start with an empty document)
    let existing = if config_path.exists() {
        std::fs::read_to_string(&config_path)?
    } else {
        String::new()
    };

    // Check if trust_level is already set
    if has_trust_level(&existing, &project_key) {
        return Ok(());
    }

    // Show the trust prompt
    let is_git = is_git_repo(cwd);
    let trust = prompt_trust_decision(cwd, is_git)?;

    // Write the decision
    write_trust_level(codex_home, &project_key, trust)?;

    Ok(())
}

// ────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────

/// Resolve the key used in `[projects."<key>"]`.
///
/// Mirrors Codex: use the root of the main git repo (handling worktrees),
/// falling back to `cwd` itself.
fn resolve_project_key(cwd: &Path) -> String {
    resolve_git_root(cwd)
        .unwrap_or_else(|| cwd.to_path_buf())
        .to_string_lossy()
        .to_string()
}

/// Same logic as Codex's `resolve_root_git_project_for_trust`.
fn resolve_git_root(cwd: &Path) -> Option<PathBuf> {
    let base = if cwd.is_dir() { cwd } else { cwd.parent()? };
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(base)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let git_dir_s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    let git_dir_raw = if Path::new(&git_dir_s).is_absolute() {
        PathBuf::from(&git_dir_s)
    } else {
        base.join(&git_dir_s)
    };
    let git_dir = std::fs::canonicalize(&git_dir_raw).unwrap_or(git_dir_raw);
    git_dir.parent().map(Path::to_path_buf)
}

fn is_git_repo(cwd: &Path) -> bool {
    std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(cwd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if the config already contains a `trust_level` for this project key.
fn has_trust_level(config_text: &str, project_key: &str) -> bool {
    let Ok(doc) = config_text.parse::<toml_edit::DocumentMut>() else {
        return false;
    };
    doc.get("projects")
        .and_then(|p| p.get(project_key))
        .and_then(|proj| proj.get("trust_level"))
        .is_some()
}

/// Interactive prompt — printed directly to stderr so it works even before TUI
/// alternate screen is entered.
fn prompt_trust_decision(cwd: &Path, is_git: bool) -> anyhow::Result<bool> {
    let stderr = io::stderr();
    let mut out = stderr.lock();

    writeln!(out)?;
    writeln!(
        out,
        "  You are running Gugugaga in {}",
        cwd.display()
    )?;
    writeln!(out)?;

    if is_git {
        writeln!(
            out,
            "  Since this folder is version controlled, you may wish to allow"
        )?;
        writeln!(
            out,
            "  Codex to work in this folder without asking for approval."
        )?;
    } else {
        writeln!(
            out,
            "  Since this folder is not version controlled, we recommend"
        )?;
        writeln!(
            out,
            "  requiring approval of all edits and commands."
        )?;
    }

    writeln!(out)?;

    if is_git {
        writeln!(
            out,
            "  [1] Yes, allow Codex to work in this folder without asking for approval"
        )?;
        writeln!(out, "  [2] No, ask me to approve edits and commands")?;
    } else {
        writeln!(
            out,
            "  [1] Allow Codex to work in this folder without asking for approval"
        )?;
        writeln!(out, "  [2] Require approval of edits and commands")?;
    }

    writeln!(out)?;
    write!(out, "  Choice [1/2]: ")?;
    out.flush()?;

    // Read from stdin (raw terminal is not yet active)
    let stdin = io::stdin();
    let mut input = String::new();
    stdin.read_line(&mut input)?;

    let choice = input.trim();
    match choice {
        "1" | "y" | "Y" => Ok(true),
        _ => Ok(false), // default to untrusted (safer)
    }
}

/// Write the trust decision to `~/.codex/config.toml`, using the same
/// `toml_edit` approach Codex uses to preserve the file's existing content.
fn write_trust_level(
    codex_home: &Path,
    project_key: &str,
    trusted: bool,
) -> anyhow::Result<()> {
    let config_path = codex_home.join("config.toml");

    // Ensure codex_home directory exists
    std::fs::create_dir_all(codex_home)?;

    let existing = if config_path.exists() {
        std::fs::read_to_string(&config_path)?
    } else {
        String::new()
    };

    let mut doc: toml_edit::DocumentMut = existing.parse()?;

    // Ensure [projects] table exists
    {
        let root = doc.as_table_mut();
        let existing_projects = root.get("projects").cloned();
        if existing_projects.as_ref().is_none_or(|i| !i.is_table()) {
            let mut projects_tbl = toml_edit::Table::new();
            projects_tbl.set_implicit(true);

            // Migrate any inline table entries
            if let Some(ref existing) = existing_projects {
                if let Some(inline_tbl) = existing.as_inline_table() {
                    for (k, v) in inline_tbl.iter() {
                        if let Some(inner_tbl) = v.as_inline_table() {
                            let new_tbl = inner_tbl.clone().into_table();
                            projects_tbl.insert(k, toml_edit::Item::Table(new_tbl));
                        }
                    }
                }
            }
            root.insert("projects", toml_edit::Item::Table(projects_tbl));
        }
    }

    let projects_tbl = doc["projects"]
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("projects table missing after initialization"))?;

    // Insert or update the project entry
    if !projects_tbl.contains_key(project_key)
        || projects_tbl.get(project_key).and_then(|i| i.as_table()).is_none()
    {
        projects_tbl.insert(project_key, toml_edit::table());
    }

    let proj_tbl = projects_tbl
        .get_mut(project_key)
        .and_then(|i| i.as_table_mut())
        .ok_or_else(|| anyhow::anyhow!("project table missing for {project_key}"))?;

    proj_tbl.set_implicit(false);
    let level = if trusted { "trusted" } else { "untrusted" };
    proj_tbl["trust_level"] = toml_edit::value(level);

    std::fs::write(&config_path, doc.to_string())?;

    let label = if trusted { "Trusted" } else { "Untrusted" };
    eprintln!("  {} → {} ✓\n", label, project_key);

    Ok(())
}

//! Trust directory onboarding — aligned with Codex behavior.
//!
//! Provides helpers to check and persist trust decisions.  The interactive
//! prompt is handled by the TUI Welcome phase (animation + trust UI in one
//! screen), so this module only exposes non-interactive building blocks.

use std::path::{Path, PathBuf};

// ────────────────────────────────────────────
// Public types
// ────────────────────────────────────────────

/// Everything the TUI needs to display and persist a trust decision.
pub struct TrustContext {
    /// The project key used in `config.toml`.
    pub project_key: String,
    /// Whether the project directory is inside a git repo.
    pub is_git: bool,
    /// Codex home directory (for writing the decision).
    pub codex_home: PathBuf,
    /// Display path shown to the user.
    pub display_path: String,
}

// ────────────────────────────────────────────
// Public API
// ────────────────────────────────────────────

/// Check whether a trust decision is needed for `cwd`.
///
/// Returns `Some(TrustContext)` if the user still needs to choose,
/// or `None` if the decision was already persisted.
pub fn check_trust(codex_home: &Path, cwd: &Path) -> Option<TrustContext> {
    let project_key = resolve_project_key(cwd);
    let config_path = codex_home.join("config.toml");

    let existing = if config_path.exists() {
        std::fs::read_to_string(&config_path).unwrap_or_default()
    } else {
        String::new()
    };

    if has_trust_level(&existing, &project_key) {
        return None;
    }

    Some(TrustContext {
        project_key,
        is_git: is_git_repo(cwd),
        codex_home: codex_home.to_path_buf(),
        display_path: cwd.display().to_string(),
    })
}

/// Persist a trust decision to `~/.codex/config.toml`.
pub fn write_trust_decision(ctx: &TrustContext, trusted: bool) -> anyhow::Result<()> {
    write_trust_level(&ctx.codex_home, &ctx.project_key, trusted)
}

// ────────────────────────────────────────────
// Helpers (unchanged)
// ────────────────────────────────────────────

fn resolve_project_key(cwd: &Path) -> String {
    resolve_git_root(cwd)
        .unwrap_or_else(|| cwd.to_path_buf())
        .to_string_lossy()
        .to_string()
}

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

fn has_trust_level(config_text: &str, project_key: &str) -> bool {
    let Ok(doc) = config_text.parse::<toml_edit::DocumentMut>() else {
        return false;
    };
    doc.get("projects")
        .and_then(|p| p.get(project_key))
        .and_then(|proj| proj.get("trust_level"))
        .is_some()
}

fn write_trust_level(
    codex_home: &Path,
    project_key: &str,
    trusted: bool,
) -> anyhow::Result<()> {
    let config_path = codex_home.join("config.toml");
    std::fs::create_dir_all(codex_home)?;

    let existing = if config_path.exists() {
        std::fs::read_to_string(&config_path)?
    } else {
        String::new()
    };

    let mut doc: toml_edit::DocumentMut = existing.parse()?;

    {
        let root = doc.as_table_mut();
        let existing_projects = root.get("projects").cloned();
        if existing_projects.as_ref().is_none_or(|i| !i.is_table()) {
            let mut projects_tbl = toml_edit::Table::new();
            projects_tbl.set_implicit(true);
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
        .ok_or_else(|| anyhow::anyhow!("projects table missing"))?;

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
    Ok(())
}

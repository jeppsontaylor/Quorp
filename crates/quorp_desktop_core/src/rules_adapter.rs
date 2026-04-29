//! Filesystem scanner for `.rules` files surfaced via the
//! `list_rules` Tauri command.
//!
//! Three discovery roots:
//! 1. Repo-level: `<workspace>/.rules` — active by default.
//! 2. Project-level: `<workspace>/.quorp/rules/*.rules` — typically
//!    user-authored, lifecycle defaults to draft until promoted.
//! 3. Global: `~/.quorp/rules/*.rules` — shared across all
//!    workspaces, lifecycle defaults to active.
//!
//! Lifecycle changes via `update_rule_lifecycle` go through
//! `<workspace>/.quorp/rules/lifecycle.json` — a small JSON ledger
//! that maps rule id → lifecycle state. We don't move files
//! around; the ledger is authoritative for the lifecycle column and
//! `rule_forge` reads the same file when it lands as the canonical
//! persistence layer.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize)]
pub struct RuleSummaryDto {
    pub id: String,
    pub display_name: String,
    pub source_path: String,
    pub lifecycle: String,
    pub evidence_count: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum RulesAdapterError {
    #[error("io error scanning rules: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed lifecycle ledger: {0}")]
    Ledger(String),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct LifecycleLedger {
    /// rule id → lifecycle (`draft|active|suspended|archived`).
    states: HashMap<String, String>,
}

const VALID_LIFECYCLES: &[&str] = &["draft", "active", "suspended", "archived"];

/// List every rule file under the three discovery roots, applying
/// the workspace's lifecycle ledger overlay.
pub fn list_rules(workspace_root: &Path) -> Result<Vec<RuleSummaryDto>, RulesAdapterError> {
    let ledger = load_ledger(workspace_root).unwrap_or_default();
    let mut out = Vec::new();

    // 1. Repo-level
    let repo = workspace_root.join(".rules");
    if repo.is_file() {
        out.push(summarize(&repo, "repo", "active", &ledger)?);
    }

    // 2. Project-level
    let project_dir = workspace_root.join(".quorp").join("rules");
    if project_dir.is_dir() {
        out.extend(scan_dir(&project_dir, "project", "draft", &ledger)?);
    }

    // 3. Global
    if let Some(home) = dirs::home_dir() {
        let global_dir = home.join(".quorp").join("rules");
        if global_dir.is_dir() {
            out.extend(scan_dir(&global_dir, "global", "active", &ledger)?);
        }
    }

    out.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    Ok(out)
}

/// Update the lifecycle for a rule id. Persists to the ledger; never
/// moves the source file. Returns the new lifecycle so the caller can
/// echo it back in the receipt.
pub fn update_lifecycle(
    workspace_root: &Path,
    rule_id: &str,
    lifecycle: &str,
) -> Result<String, RulesAdapterError> {
    let lifecycle = lifecycle.trim().to_lowercase();
    if !VALID_LIFECYCLES.contains(&lifecycle.as_str()) {
        return Err(RulesAdapterError::Ledger(format!(
            "unknown lifecycle: {lifecycle}; expected one of {VALID_LIFECYCLES:?}"
        )));
    }
    let mut ledger = load_ledger(workspace_root).unwrap_or_default();
    ledger.states.insert(rule_id.to_string(), lifecycle.clone());
    save_ledger(workspace_root, &ledger)?;
    Ok(lifecycle)
}

fn scan_dir(
    dir: &Path,
    scope: &str,
    default_lifecycle: &str,
    ledger: &LifecycleLedger,
) -> Result<Vec<RuleSummaryDto>, RulesAdapterError> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name_ok = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.ends_with(".rules") || s == ".rules")
            .unwrap_or(false);
        if !name_ok {
            continue;
        }
        out.push(summarize(&path, scope, default_lifecycle, ledger)?);
    }
    Ok(out)
}

fn summarize(
    path: &Path,
    scope: &str,
    default_lifecycle: &str,
    ledger: &LifecycleLedger,
) -> Result<RuleSummaryDto, RulesAdapterError> {
    let id = derive_rule_id(path);
    let display_name = display_name_for(path, scope);
    let lifecycle = ledger
        .states
        .get(&id)
        .cloned()
        .unwrap_or_else(|| default_lifecycle.to_string());
    let evidence_count = count_evidence(path)?;
    Ok(RuleSummaryDto {
        id,
        display_name,
        source_path: path.display().to_string(),
        lifecycle,
        evidence_count,
    })
}

fn display_name_for(path: &Path, scope: &str) -> String {
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    format!("{scope} · {base}")
}

/// Stable rule id from the canonical path: short SHA-256 prefix +
/// the file name. Stable across runs and machines, robust to file
/// moves within the same scope.
fn derive_rule_id(path: &Path) -> String {
    let canonical = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    let leaf = canonical
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("rule");
    format!("rule-{hex}-{leaf}")
}

/// "Evidence" rows are the bullet items inside the rule body that
/// provide a justification for it. We scan for `*` / `-` / `+` markers
/// at line start; works for both Markdown and plain `.rules` styles.
fn count_evidence(path: &Path) -> Result<u32, RulesAdapterError> {
    let body = fs::read_to_string(path)?;
    let mut count: u32 = 0;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
            count = count.saturating_add(1);
        }
    }
    Ok(count)
}

fn ledger_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(".quorp")
        .join("rules")
        .join("lifecycle.json")
}

fn load_ledger(workspace_root: &Path) -> Result<LifecycleLedger, RulesAdapterError> {
    let path = ledger_path(workspace_root);
    if !path.exists() {
        return Ok(LifecycleLedger::default());
    }
    let body = fs::read_to_string(&path)?;
    if body.trim().is_empty() {
        return Ok(LifecycleLedger::default());
    }
    serde_json::from_str::<LifecycleLedger>(&body)
        .map_err(|err| RulesAdapterError::Ledger(err.to_string()))
}

fn save_ledger(
    workspace_root: &Path,
    ledger: &LifecycleLedger,
) -> Result<(), RulesAdapterError> {
    let path = ledger_path(workspace_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(ledger)
        .map_err(|err| RulesAdapterError::Ledger(err.to_string()))?;
    fs::write(&path, body)?;
    Ok(())
}

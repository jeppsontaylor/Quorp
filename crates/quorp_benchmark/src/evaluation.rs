use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ChallengeMetadata;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvaluatorOutcome {
    pub name: String,
    pub script: PathBuf,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub duration_ms: u64,
    pub exit_code: i32,
    pub passed: bool,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_shell_command(
    name: &str,
    command: &str,
    script: &Path,
    current_dir: &Path,
) -> anyhow::Result<EvaluatorOutcome> {
    run_shell_command_with_env(name, command, script, current_dir, &[])
}

pub fn run_shell_command_with_env(
    name: &str,
    command: &str,
    script: &Path,
    current_dir: &Path,
    environment: &[(&str, &OsStr)],
) -> anyhow::Result<EvaluatorOutcome> {
    let started_at = std::time::Instant::now();
    #[allow(clippy::disallowed_methods)]
    let mut shell = Command::new("bash");
    shell.arg("-lc").arg(command).current_dir(current_dir);
    for (key, value) in environment {
        shell.env(key, value);
    }
    let output = shell
        .output()
        .with_context(|| format!("failed to run {} command `{}`", name, command))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(EvaluatorOutcome {
        name: name.to_string(),
        script: script.to_path_buf(),
        command: Some(command.to_string()),
        duration_ms: started_at.elapsed().as_millis() as u64,
        exit_code: output.status.code().unwrap_or(-1),
        passed: evaluator_passed(output.status.success(), &stdout),
        stdout,
        stderr,
    })
}

pub fn run_visible_evaluator(script: &Path, workspace_dir: &Path) -> anyhow::Result<EvaluatorOutcome> {
    let started_at = std::time::Instant::now();
    #[allow(clippy::disallowed_methods)]
    let output = Command::new(script)
        .current_dir(workspace_dir)
        .output()
        .with_context(|| format!("failed to run visible evaluator {}", script.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(EvaluatorOutcome {
        name: "visible".to_string(),
        script: script.to_path_buf(),
        command: Some(script.display().to_string()),
        duration_ms: started_at.elapsed().as_millis() as u64,
        exit_code: output.status.code().unwrap_or(-1),
        passed: evaluator_passed(output.status.success(), &stdout),
        stdout,
        stderr,
    })
}

pub fn run_collector_evaluator(
    script: &Path,
    workspace_dir: &Path,
    attempt_dir: &Path,
) -> anyhow::Result<EvaluatorOutcome> {
    let started_at = std::time::Instant::now();
    #[allow(clippy::disallowed_methods)]
    let output = Command::new(script)
        .arg(workspace_dir)
        .env("QUORP_BENCHMARK_WORKSPACE", workspace_dir)
        .env("QUORP_BENCHMARK_ATTEMPT_DIR", attempt_dir)
        .current_dir(script.parent().unwrap_or_else(|| Path::new("/")))
        .output()
        .with_context(|| format!("failed to run collector evaluator {}", script.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(EvaluatorOutcome {
        name: "collector".to_string(),
        script: script.to_path_buf(),
        command: Some(format!("{} {}", script.display(), workspace_dir.display())),
        duration_ms: started_at.elapsed().as_millis() as u64,
        exit_code: output.status.code().unwrap_or(-1),
        passed: evaluator_passed(output.status.success(), &stdout),
        stdout,
        stderr,
    })
}

pub fn challenge_evaluation_target_dir(
    challenge_metadata: &ChallengeMetadata,
    attempt_number: usize,
    evaluation_cache_dir_name: &str,
) -> PathBuf {
    challenge_metadata
        .sandbox_root
        .parent()
        .unwrap_or(&challenge_metadata.sandbox_root)
        .join(evaluation_cache_dir_name)
        .join(&challenge_metadata.condition)
        .join(format!("attempt-{attempt_number:03}"))
}

pub fn challenge_evaluation_env<'a>(
    challenge_metadata: &ChallengeMetadata,
    evaluation_target_dir: &'a Path,
) -> Vec<(&'static str, &'a OsStr)> {
    let mut env = Vec::new();
    if challenge_evaluation_needs_sdkroot_override(challenge_metadata) {
        env.push(("SDKROOT", Path::new("/").as_os_str()));
    }
    if challenge_evaluation_is_cargo_dist_snapshot_sensitive(challenge_metadata) {
        env
    } else {
        env.push(("CARGO_TARGET_DIR", evaluation_target_dir.as_os_str()));
        env
    }
}

pub fn evaluator_passed(exit_success: bool, stdout: &str) -> bool {
    if let Some(summary) = parse_benchmark_summary_value(stdout)
        && let Some(success) = summary.get("success").and_then(serde_json::Value::as_bool)
    {
        return exit_success && success;
    }
    exit_success
}

pub fn parse_benchmark_summary_value(stdout: &str) -> Option<serde_json::Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(value);
    }
    let start = trimmed.find('{')?;
    let candidate = trimmed.get(start..)?.trim();
    serde_json::from_str::<serde_json::Value>(candidate).ok()
}

fn challenge_evaluation_is_cargo_dist_snapshot_sensitive(
    challenge_metadata: &ChallengeMetadata,
) -> bool {
    challenge_metadata
        .allowed_generated_files
        .iter()
        .any(|path| path == "cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap")
}

fn challenge_evaluation_needs_sdkroot_override(challenge_metadata: &ChallengeMetadata) -> bool {
    challenge_metadata
        .case_root
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "05-cc-rs-compile-intermediates")
        || (challenge_metadata.tags.iter().any(|tag| tag == "cc-rs")
            && challenge_metadata
                .expected_files_touched
                .iter()
                .any(|path| path == "src/lib.rs"))
}

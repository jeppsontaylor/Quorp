use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use async_zip::ZipEntryBuilder;
use async_zip::base::write::ZipFileWriter;
use futures::AsyncWriteExt as _;
use futures::io::Cursor;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedWorkspaceObjective {
    pub workspace_root: PathBuf,
    pub challenge_root: PathBuf,
    pub editable_workspace_root: PathBuf,
    pub editable_workspace_relative_root: Option<String>,
    pub objective_file: PathBuf,
    pub evaluate_command: Option<String>,
    pub reset_command: Option<String>,
    pub selected_condition: Option<String>,
    pub success_file: Option<PathBuf>,
    pub context_files: Vec<PathBuf>,
    pub repair_artifacts: Vec<PathBuf>,
    pub workspace_root_entries: Vec<String>,
    pub editable_workspace_entries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsPaths {
    pub logs_dir: PathBuf,
    pub quorp_log: PathBuf,
    pub memory_log: PathBuf,
    pub tui_diagnostics_log: PathBuf,
}

static RUN_RESULT_DIR_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestRunInfo {
    pub run_dir: PathBuf,
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandOutcome {
    pub command: String,
    pub exit_code: i32,
    pub passed: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluatorOutcome {
    pub command: String,
    pub process_exit_code: i32,
    pub process_passed: bool,
    pub logical_success: Option<bool>,
    pub evaluation_passed: bool,
    pub parsed_json_path: Option<PathBuf>,
    pub parsed_from_stdout: bool,
    pub stdout: String,
    pub stderr: String,
    pub payload: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChallengeManifest {
    repo_condition: Vec<String>,
    objective_file: String,
    success_file: String,
    reset_command: String,
    evaluate_command: String,
    #[serde(default)]
    allowed_generated_files: Vec<String>,
}

pub fn diagnostics_paths() -> DiagnosticsPaths {
    DiagnosticsPaths {
        logs_dir: ::paths::log_file()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| ::paths::home_dir().to_path_buf()),
        quorp_log: ::paths::log_file().to_path_buf(),
        memory_log: ::paths::memory_log_file().to_path_buf(),
        tui_diagnostics_log: crate::quorp::tui::diagnostics::diagnostics_log_file(),
    }
}

pub fn resolve_workspace_objective(
    workspace: &Path,
    explicit_condition: Option<&str>,
) -> anyhow::Result<ResolvedWorkspaceObjective> {
    let workspace_root = fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    if let Some(case_root) = find_ancestor_with_file(&workspace_root, "benchmark.json") {
        let manifest_path = case_root.join("benchmark.json");
        let manifest: ChallengeManifest = serde_json::from_str(
            &fs::read_to_string(&manifest_path)
                .with_context(|| format!("failed to read {}", manifest_path.display()))?,
        )
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
        let condition = resolve_challenge_condition(
            &workspace_root,
            &case_root,
            &manifest,
            explicit_condition,
        )?;
        let editable_workspace_root = case_root.join("workspace").join(&condition);
        let editable_workspace_root =
            fs::canonicalize(&editable_workspace_root).unwrap_or(editable_workspace_root);
        let editable_workspace_relative_root = editable_workspace_root
            .strip_prefix(&workspace_root)
            .ok()
            .map(|relative| relative.display().to_string())
            .filter(|relative| !relative.is_empty());
        let objective_file = fs::canonicalize(case_root.join(&manifest.objective_file))
            .unwrap_or_else(|_| case_root.join(&manifest.objective_file));
        let success_file = fs::canonicalize(case_root.join(&manifest.success_file))
            .unwrap_or_else(|_| case_root.join(&manifest.success_file));
        return Ok(ResolvedWorkspaceObjective {
            workspace_root: workspace_root.clone(),
            challenge_root: case_root.clone(),
            editable_workspace_root: editable_workspace_root.clone(),
            editable_workspace_relative_root,
            objective_file,
            evaluate_command: Some(substitute_condition(&manifest.evaluate_command, &condition)),
            reset_command: Some(substitute_condition(&manifest.reset_command, &condition)),
            selected_condition: Some(condition),
            success_file: Some(success_file),
            context_files: collect_context_files(&case_root),
            repair_artifacts: collect_repair_artifacts(&case_root),
            workspace_root_entries: list_root_entries(&workspace_root),
            editable_workspace_entries: list_root_entries(&editable_workspace_root),
        });
    }

    let objective_file = [
        workspace_root.join("START_HERE.md"),
        workspace_root.join("README.md"),
    ]
    .into_iter()
    .find(|path| path.exists())
    .ok_or_else(|| {
        anyhow::anyhow!(
            "no objective file found in {}; expected START_HERE.md or README.md",
            workspace_root.display()
        )
    })?;

    let evaluate_command = [
        ("./evaluate.sh", workspace_root.join("evaluate.sh")),
        (
            "./evaluate_visible.sh",
            workspace_root.join("evaluate_visible.sh"),
        ),
    ]
    .into_iter()
    .find_map(|(command, path)| path.exists().then(|| command.to_string()));

    Ok(ResolvedWorkspaceObjective {
        workspace_root: workspace_root.clone(),
        challenge_root: workspace_root.clone(),
        editable_workspace_root: workspace_root.clone(),
        editable_workspace_relative_root: None,
        objective_file,
        evaluate_command,
        reset_command: None,
        selected_condition: None,
        success_file: None,
        context_files: collect_context_files(&workspace_root),
        repair_artifacts: collect_repair_artifacts(&workspace_root),
        workspace_root_entries: list_root_entries(&workspace_root),
        editable_workspace_entries: list_root_entries(&workspace_root),
    })
}

pub fn default_run_result_dir(workspace: &Path, scope: &str) -> PathBuf {
    let workspace_name = workspace
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("workspace");
    let sequence = RUN_RESULT_DIR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    diagnostics_paths()
        .logs_dir
        .join("runs")
        .join(scope)
        .join(format!(
            "{}-{:04}-{}",
            timestamp_ms(),
            sequence,
            sanitize_component(workspace_name)
        ))
}

pub fn latest_run_dir(scope: Option<&str>) -> anyhow::Result<Option<LatestRunInfo>> {
    let runs_root = diagnostics_paths().logs_dir.join("runs");
    if !runs_root.exists() {
        return Ok(None);
    }

    let scopes = if let Some(scope) = scope {
        vec![(scope.to_string(), runs_root.join(scope))]
    } else {
        fs::read_dir(&runs_root)?
            .filter_map(std::result::Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                path.is_dir().then(|| {
                    (
                        entry.file_name().to_string_lossy().to_string(),
                        entry.path(),
                    )
                })
            })
            .collect()
    };

    let mut best: Option<(SystemTime, LatestRunInfo)> = None;
    for (scope_name, scope_root) in scopes {
        if !scope_root.exists() {
            continue;
        }
        for entry in fs::read_dir(scope_root)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH);
            let candidate = LatestRunInfo {
                run_dir: path,
                scope: scope_name.clone(),
            };
            let should_replace = best
                .as_ref()
                .map(|(best_modified, _)| modified > *best_modified)
                .unwrap_or(true);
            if should_replace {
                best = Some((modified, candidate));
            }
        }
    }
    Ok(best.map(|(_, info)| info))
}

pub fn snapshot_logs(result_dir: &Path, app_run_id: Option<&str>) -> anyhow::Result<()> {
    let log_snapshot_dir = result_dir.join("logs");
    fs::create_dir_all(&log_snapshot_dir)?;

    let paths = diagnostics_paths();
    copy_if_exists(&paths.quorp_log, &log_snapshot_dir.join("Quorp.log"))?;
    copy_if_exists(&paths.memory_log, &log_snapshot_dir.join("QuorpMemory.log"))?;
    if let Some(id) = app_run_id {
        write_filtered_tui_log(
            &paths.tui_diagnostics_log,
            &log_snapshot_dir.join("QuorpTuiDiagnostics.log"),
            id,
        )?;
    } else {
        copy_if_exists(
            &paths.tui_diagnostics_log,
            &log_snapshot_dir.join("QuorpTuiDiagnostics.log"),
        )?;
    }

    let provider = crate::quorp::provider_config::env_value("QUORP_PROVIDER");
    let model = crate::quorp::provider_config::resolved_model_env();
    let executor = std::env::var("QUORP_EXECUTOR").ok();
    write_json(
        &log_snapshot_dir.join("system.json"),
        &serde_json::json!({
            "captured_at_ms": timestamp_ms(),
            "app_run_id": app_run_id,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "logs_dir": paths.logs_dir,
            "provider": provider,
            "model": model,
            "executor": executor,
            "runtime": {"mode": "native"},
        }),
    )?;
    Ok(())
}

pub fn summarize_run_dir(run_dir: &Path) -> anyhow::Result<String> {
    let request = read_json_value(&run_dir.join("request.json")).unwrap_or(Value::Null);
    let metadata = read_json_value(&run_dir.join("metadata.json")).unwrap_or(Value::Null);
    let summary = read_json_value(&run_dir.join("summary.json")).unwrap_or(Value::Null);
    let events = read_events(&run_dir.join("events.jsonl"))?;

    let objective_file = metadata
        .get("objective_file")
        .or_else(|| request.get("objective_file"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "unknown".to_string());
    let evaluate_command = metadata
        .get("evaluate_command")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            request
                .get("evaluate_command")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let provider = metadata
        .get("provider")
        .and_then(Value::as_str)
        .or_else(|| request.get("provider").and_then(Value::as_str))
        .unwrap_or("unknown");
    let model = metadata
        .get("model_id")
        .and_then(Value::as_str)
        .or_else(|| request.get("model_id").and_then(Value::as_str))
        .unwrap_or("unknown");
    let logical_success = summary
        .get("logical_success")
        .or_else(|| metadata.get("logical_success"))
        .and_then(Value::as_bool);
    let process_exit_code = summary
        .get("process_exit_code")
        .or_else(|| metadata.get("process_exit_code"))
        .and_then(Value::as_i64);
    let evaluation_passed = summary
        .get("evaluation_passed")
        .or_else(|| metadata.get("evaluation_passed"))
        .and_then(Value::as_bool);
    let routing = summary
        .get("routing")
        .or_else(|| metadata.get("routing"))
        .cloned()
        .unwrap_or(Value::Null);

    let first_failure = events.iter().find_map(first_failure_from_event);
    let first_edit = events.iter().find_map(first_edit_from_event);
    let first_bad_path = events
        .iter()
        .find(|event| event_name(event) == Some("agent.path_resolution_failed"))
        .and_then(|event| {
            let request_path = event_field(event, "request_path")
                .and_then(Value::as_str)
                .map(str::to_string)?;
            let suggested_path = event_field(event, "suggested_path")
                .and_then(Value::as_str)
                .map(str::to_string);
            Some((request_path, suggested_path))
        });
    let recovery_turns = events
        .iter()
        .filter(|event| event_name(event) == Some("agent.recovery_turn_queued"))
        .count();
    let parser_recovery_turns = events
        .iter()
        .filter(|event| event_name(event) == Some("agent.parse_recovery_queued"))
        .count();
    let parser_recovery_exhausted = events
        .iter()
        .any(|event| event_name(event) == Some("agent.parse_recovery_exhausted"));
    let parser_warning_count = events
        .iter()
        .filter_map(parser_warning_count_from_event)
        .sum::<usize>();
    let verification_ran = events.iter().any(|event| {
        matches!(
            event_name(event),
            Some("validation_started") | Some("ValidationStarted")
        )
    });
    let pending_validation_blocked = events
        .iter()
        .any(|event| event_name(event) == Some("run.blocked_on_pending_validation"));
    let verifier_queued = events
        .iter()
        .any(|event| event_name(event) == Some("agent.verifier_queued"));
    let retries = events
        .iter()
        .filter(|event| event_name(event) == Some("run.retry_started"))
        .count();
    let validations = events
        .iter()
        .filter_map(validation_line_from_event)
        .collect::<Vec<_>>();
    let last_validation = validations.last().cloned();
    let final_stop_reason = summary
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    let mut lines = Vec::new();
    lines.push(format!("Run directory: {}", run_dir.display()));
    lines.push(format!("Objective: {objective_file}"));
    if let Some(command) = evaluate_command {
        lines.push(format!("Evaluator: {command}"));
    }
    lines.push(format!("Provider/Model: {provider}/{model}"));
    if let Some(scenario_label) = summary.get("scenario_label").and_then(Value::as_str) {
        lines.push(format!("Scenario label: {scenario_label}"));
    } else if let Some(scenario_label) = routing.get("scenario_label").and_then(Value::as_str) {
        lines.push(format!("Scenario label: {scenario_label}"));
    }
    if let Some(routing_mode) = routing.get("routing_mode").and_then(Value::as_str) {
        lines.push(format!("Routing mode: {routing_mode}"));
    }
    if let Some(effective_model) = routing.get("effective_model").and_then(Value::as_str) {
        lines.push(format!("Effective model: {effective_model}"));
    }
    if routing
        .get("used_fallback")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let reason = routing
            .get("fallback_reason")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        lines.push(format!("Fallback: yes ({reason})"));
    }
    if let Some(mode) = metadata
        .get("runtime")
        .or_else(|| request.get("runtime"))
        .and_then(|value| value.get("mode"))
        .and_then(Value::as_str)
    {
        lines.push(format!("Runtime: {mode}"));
    }
    if let Some(first_edit) = first_edit {
        lines.push(format!("First edit: {first_edit}"));
    } else {
        lines.push("First edit: none recorded".to_string());
    }
    if let Some(failure) = first_failure {
        lines.push(format!("First failing action: {failure}"));
    } else {
        lines.push("First failing action: none recorded".to_string());
    }
    if let Some((request_path, suggested_path)) = first_bad_path {
        lines.push(format!("First bad path: {request_path}"));
        lines.push(format!(
            "Suggested correction: {}",
            suggested_path.unwrap_or_else(|| "none inferred".to_string())
        ));
    } else {
        lines.push("First bad path: none recorded".to_string());
    }
    lines.push(format!("Recovery turns queued: {recovery_turns}"));
    lines.push(format!(
        "Parser recovery turns queued: {parser_recovery_turns}"
    ));
    lines.push(format!("Parser warnings observed: {parser_warning_count}"));
    lines.push(format!(
        "Parser recovery exhausted: {}",
        parser_recovery_exhausted
    ));
    lines.push(format!("Full retries attempted: {retries}"));
    lines.push(format!("Verification ran: {verification_ran}"));
    lines.push(format!("Validation queued before stop: {verifier_queued}"));
    lines.push(format!(
        "Evaluator logical success: {}",
        logical_success
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    ));
    lines.push(format!(
        "Evaluator process exit: {}",
        process_exit_code
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    ));
    lines.push(format!(
        "Evaluation passed: {}",
        evaluation_passed
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    ));
    if let Some(validation) = last_validation {
        lines.push(format!("Last validation: {validation}"));
    } else {
        lines.push("Last validation: none recorded".to_string());
    }
    if parser_recovery_exhausted {
        lines.push("Failure class: unrecoverable parser error".to_string());
    } else if evaluation_passed == Some(false) {
        lines.push("Failure class: later validation/evaluator issue".to_string());
    } else if parser_recovery_turns > 0 || parser_warning_count > 0 {
        lines.push("Failure class: retryable parser error recovered".to_string());
    }
    if pending_validation_blocked {
        lines.push("Stop blocked by pending validation: true".to_string());
    }
    lines.push(format!("Final stop reason: {final_stop_reason}"));
    Ok(lines.join("\n"))
}

pub fn bundle_run_dir(run_dir: &Path, output_path: &Path) -> anyhow::Result<PathBuf> {
    let mut writer = Cursor::new(Vec::<u8>::new());
    futures::executor::block_on(async {
        let mut zip = ZipFileWriter::new(&mut writer);
        for entry in walkdir::WalkDir::new(run_dir) {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let relative = path
                .strip_prefix(run_dir)
                .with_context(|| format!("failed to relativize {}", path.display()))?;
            let data =
                fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
            let builder = ZipEntryBuilder::new(
                relative.to_string_lossy().replace('\\', "/").into(),
                async_zip::Compression::Stored,
            );
            zip.write_entry_whole(builder, &data).await?;
        }
        zip.close().await?;
        writer.flush().await?;
        Ok::<(), anyhow::Error>(())
    })?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, writer.into_inner())?;
    Ok(output_path.to_path_buf())
}

pub fn default_bundle_path(run_dir: &Path) -> PathBuf {
    let file_name = run_dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("run");
    diagnostics_paths()
        .logs_dir
        .join("bundles")
        .join(format!("{file_name}.zip"))
}

pub fn write_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(path, bytes)?;
    Ok(())
}

pub fn append_event_record(path: &Path, payload: Value) -> anyhow::Result<()> {
    let mut existing = String::new();
    if path.exists() {
        existing = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
    } else if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let record = serde_json::json!({
        "ts_ms": timestamp_ms(),
        "payload": payload,
    });
    existing.push_str(&record.to_string());
    existing.push('\n');
    fs::write(path, existing)?;
    Ok(())
}

pub fn append_named_event(path: &Path, event_name: &str, fields: Value) -> anyhow::Result<()> {
    let mut payload = serde_json::Map::new();
    payload.insert("event".to_string(), Value::String(event_name.to_string()));
    if let Some(object) = fields.as_object() {
        for (key, value) in object {
            payload.insert(key.clone(), value.clone());
        }
    }
    append_event_record(path, Value::Object(payload))
}

pub fn run_command(cwd: &Path, command: &str) -> anyhow::Result<CommandOutcome> {
    #[allow(clippy::disallowed_methods)]
    let output = Command::new("sh")
        .arg("-lc")
        .arg(command)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to run `{command}` in {}", cwd.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(CommandOutcome {
        command: command.to_string(),
        exit_code: output.status.code().unwrap_or(-1),
        passed: output.status.success(),
        stdout,
        stderr,
    })
}

pub fn run_evaluator(
    cwd: &Path,
    command: &str,
    selected_condition: Option<&str>,
) -> anyhow::Result<EvaluatorOutcome> {
    let command_outcome = run_command(cwd, command)?;
    let stdout_trimmed = command_outcome.stdout.trim();
    let mut parsed_payload = serde_json::from_str::<Value>(stdout_trimmed).ok();
    let mut parsed_json_path = None;
    let mut parsed_from_stdout = parsed_payload.is_some();

    if parsed_payload.is_none()
        && let Some(condition) = selected_condition
    {
        let candidate = cwd
            .join("workspace")
            .join(condition)
            .join("benchmark-evaluation.json");
        if candidate.exists() {
            parsed_payload = read_json_value(&candidate).ok();
            if parsed_payload.is_some() {
                parsed_json_path = Some(candidate);
                parsed_from_stdout = false;
            }
        }
    }

    let logical_success = parsed_payload
        .as_ref()
        .and_then(|payload| payload.get("success"))
        .and_then(Value::as_bool);
    let evaluation_passed = logical_success.unwrap_or(command_outcome.passed);

    Ok(EvaluatorOutcome {
        command: command_outcome.command,
        process_exit_code: command_outcome.exit_code,
        process_passed: command_outcome.passed,
        logical_success,
        evaluation_passed,
        parsed_json_path,
        parsed_from_stdout,
        stdout: command_outcome.stdout,
        stderr: command_outcome.stderr,
        payload: parsed_payload,
    })
}

pub fn copy_run_artifacts_without_events(
    source_dir: &Path,
    destination_dir: &Path,
) -> anyhow::Result<()> {
    for file_name in [
        "request.json",
        "summary.json",
        "transcript.json",
        "metadata.json",
        "checkpoint.json",
        "final.diff",
    ] {
        let source = source_dir.join(file_name);
        let destination = destination_dir.join(file_name);
        copy_if_exists(&source, &destination)?;
    }
    if source_dir.join("logs").exists() {
        copy_dir_recursive(&source_dir.join("logs"), &destination_dir.join("logs"))?;
    }
    if source_dir.join("artifacts").exists() {
        copy_dir_recursive(
            &source_dir.join("artifacts"),
            &destination_dir.join("artifacts"),
        )?;
    }
    Ok(())
}

pub fn append_event_log(destination_path: &Path, source_path: &Path) -> anyhow::Result<()> {
    if !source_path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(source_path)
        .with_context(|| format!("failed to read {}", source_path.display()))?;
    if content.trim().is_empty() {
        return Ok(());
    }
    if let Some(parent) = destination_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut existing = String::new();
    if destination_path.exists() {
        existing = fs::read_to_string(destination_path)
            .with_context(|| format!("failed to read {}", destination_path.display()))?;
    }
    existing.push_str(&content);
    if !existing.ends_with('\n') {
        existing.push('\n');
    }
    fs::write(destination_path, existing)
        .with_context(|| format!("failed to write {}", destination_path.display()))?;
    Ok(())
}

fn collect_context_files(workspace_root: &Path) -> Vec<PathBuf> {
    [
        workspace_root.join("START_HERE.md"),
        workspace_root.join("REFERENCE.md"),
        workspace_root.join("YOU_ARE_HERE.txt"),
        workspace_root.join("AGENTS.md"),
        workspace_root.join("agent-map.json"),
        workspace_root.join("test-map.json"),
        workspace_root.join(".witness").join("witness-graph.json"),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .collect()
}

fn list_root_entries(path: &Path) -> Vec<String> {
    let mut entries = match fs::read_dir(path) {
        Ok(entries) => entries
            .filter_map(std::result::Result::ok)
            .filter_map(|entry| {
                let file_name = entry.file_name().into_string().ok()?;
                let metadata = entry.metadata().ok()?;
                Some(if metadata.is_dir() {
                    format!("{file_name}/")
                } else {
                    file_name
                })
            })
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    entries.sort();
    entries.truncate(24);
    entries
}

fn collect_repair_artifacts(workspace_root: &Path) -> Vec<PathBuf> {
    [
        workspace_root
            .join("target")
            .join("agent")
            .join("repair-bundle.json"),
        workspace_root
            .join("target")
            .join("agent")
            .join("last-failure.json"),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .collect()
}

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn substitute_condition(command: &str, condition: &str) -> String {
    command.replace("<condition>", condition)
}

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn copy_if_exists(source: &Path, destination: &Path) -> anyhow::Result<()> {
    if !source.exists() {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination).with_context(|| {
        format!(
            "failed to copy {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> anyhow::Result<()> {
    if !source.exists() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(source) {
        let entry = entry?;
        let path = entry.path();
        let relative = path
            .strip_prefix(source)
            .with_context(|| format!("failed to relativize {}", path.display()))?;
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else {
            copy_if_exists(path, &target)?;
        }
    }
    Ok(())
}

fn find_ancestor_with_file(path: &Path, file_name: &str) -> Option<PathBuf> {
    for ancestor in path.ancestors() {
        if ancestor.join(file_name).exists() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn resolve_challenge_condition(
    canonical: &Path,
    case_root: &Path,
    manifest: &ChallengeManifest,
    explicit_condition: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(explicit) = explicit_condition {
        if manifest
            .repo_condition
            .iter()
            .any(|condition| condition == explicit)
        {
            return Ok(explicit.to_string());
        }
        anyhow::bail!(
            "challenge condition `{}` is not listed in benchmark.json repo_condition",
            explicit
        );
    }

    if let Some(inferred) = infer_condition_from_workspace_path(canonical, case_root, manifest) {
        return Ok(inferred);
    }

    if manifest
        .repo_condition
        .iter()
        .any(|condition| condition == "proof-full")
    {
        return Ok("proof-full".to_string());
    }

    manifest
        .repo_condition
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("benchmark.json did not list any repo_condition values"))
}

fn infer_condition_from_workspace_path(
    canonical: &Path,
    case_root: &Path,
    manifest: &ChallengeManifest,
) -> Option<String> {
    let workspace_root = case_root.join("workspace");
    if !canonical.starts_with(&workspace_root) {
        return None;
    }
    let relative = canonical.strip_prefix(&workspace_root).ok()?;
    let inferred = relative
        .components()
        .next()?
        .as_os_str()
        .to_str()?
        .to_string();
    manifest
        .repo_condition
        .iter()
        .any(|condition| condition == &inferred)
        .then_some(inferred)
}

fn write_filtered_tui_log(
    source: &Path,
    destination: &Path,
    app_run_id: &str,
) -> anyhow::Result<()> {
    if !source.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(source)
        .with_context(|| format!("failed to read {}", source.display()))?;
    let filtered = content
        .lines()
        .filter(|line| {
            serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|value| {
                    value
                        .get("app_run_id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .as_deref()
                == Some(app_run_id)
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !filtered.is_empty() {
        fs::write(destination, format!("{filtered}\n"))?;
    }
    Ok(())
}

fn read_json_value(path: &Path) -> anyhow::Result<Value> {
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

fn read_events(path: &Path) -> anyhow::Result<Vec<Value>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut events = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value = serde_json::from_str(line)
            .with_context(|| format!("failed to parse event line in {}", path.display()))?;
        events.push(value);
    }
    Ok(events)
}

fn event_payload(event: &Value) -> Option<&Value> {
    event.get("payload")
}

fn event_name(event: &Value) -> Option<&str> {
    let payload = event_payload(event)?;
    payload.get("event").and_then(Value::as_str).or_else(|| {
        payload
            .as_object()
            .and_then(|object| object.keys().next().map(String::as_str))
    })
}

fn event_field<'a>(event: &'a Value, key: &str) -> Option<&'a Value> {
    let payload = event_payload(event)?;
    if payload.get("event").is_some() {
        return payload.get(key);
    }
    let wrapper = payload
        .as_object()
        .and_then(|object| object.keys().next())
        .and_then(|name| payload.get(name))?;
    wrapper.get(key)
}

fn first_failure_from_event(event: &Value) -> Option<String> {
    match event_name(event) {
        Some("tool_call_finished") | Some("ToolCallFinished") => {
            let status = event_field(event, "status")?.as_str()?;
            (status != "success").then(|| {
                let action = event_field(event, "action")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown tool");
                format!("{action} -> {status}")
            })
        }
        Some("validation_finished") | Some("ValidationFinished") => {
            let status = event_field(event, "status")?.as_str()?;
            (status != "success").then(|| {
                let summary = event_field(event, "summary")
                    .and_then(Value::as_str)
                    .unwrap_or("validation");
                format!("{summary} -> {status}")
            })
        }
        Some("fatal_error") | Some("FatalError") => event_field(event, "error")
            .and_then(Value::as_str)
            .map(|error| format!("fatal -> {error}")),
        _ => None,
    }
}

fn first_edit_from_event(event: &Value) -> Option<String> {
    if !matches!(
        event_name(event),
        Some("tool_call_finished") | Some("ToolCallFinished")
    ) {
        return None;
    }
    if event_field(event, "status")?.as_str()? != "success" {
        return None;
    }
    let action_kind = event_field(event, "action_kind")?.as_str()?;
    if !matches!(
        action_kind,
        "write_file" | "apply_patch" | "replace_block" | "set_executable"
    ) {
        return None;
    }
    let path = event_field(event, "target_path")
        .and_then(Value::as_str)
        .unwrap_or("unknown path");
    let summary = event_field(event, "edit_summary")
        .and_then(Value::as_str)
        .unwrap_or(action_kind);
    Some(format!("{path} ({summary})"))
}

fn validation_line_from_event(event: &Value) -> Option<String> {
    if !matches!(
        event_name(event),
        Some("validation_finished") | Some("ValidationFinished")
    ) {
        return None;
    }
    let summary = event_field(event, "summary")?.as_str()?;
    let status = event_field(event, "status")?.as_str()?;
    Some(format!("{summary} [{status}]"))
}

fn parser_warning_count_from_event(event: &Value) -> Option<usize> {
    if !matches!(
        event_name(event),
        Some("assistant_turn_summary") | Some("AssistantTurnSummary")
    ) {
        return None;
    }
    event_field(event, "parse_warning_count")
        .and_then(Value::as_u64)
        .map(|count| count as usize)
}

pub fn objective_metadata_json(resolved: &ResolvedWorkspaceObjective, workspace: &Path) -> Value {
    let mut context_map = BTreeMap::new();
    for path in &resolved.context_files {
        if let Ok(relative) = path.strip_prefix(workspace) {
            context_map.insert(relative.display().to_string(), path.display().to_string());
        } else {
            context_map.insert(path.display().to_string(), path.display().to_string());
        }
    }
    serde_json::json!({
        "objective_file": resolved.objective_file,
        "evaluate_command": resolved.evaluate_command,
        "reset_command": resolved.reset_command,
        "selected_condition": resolved.selected_condition,
        "challenge_root": resolved.challenge_root,
        "editable_workspace_root": resolved.editable_workspace_root,
        "editable_workspace_relative_root": resolved.editable_workspace_relative_root,
        "success_file": resolved.success_file,
        "context_files": resolved.context_files,
        "repair_artifacts": resolved.repair_artifacts,
        "context_file_map": context_map,
        "workspace_root_entries": resolved.workspace_root_entries,
        "editable_workspace_entries": resolved.editable_workspace_entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objective_resolution_prefers_start_here() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        fs::write(temp_dir.path().join("START_HERE.md"), "start").expect("start here");
        fs::write(temp_dir.path().join("README.md"), "readme").expect("readme");
        fs::write(temp_dir.path().join("evaluate.sh"), "#!/bin/sh").expect("evaluate");

        let resolved =
            resolve_workspace_objective(temp_dir.path(), None).expect("resolved objective");
        let expected = fs::canonicalize(temp_dir.path().join("START_HERE.md"))
            .unwrap_or_else(|_| temp_dir.path().join("START_HERE.md"));

        assert_eq!(resolved.objective_file, expected);
        assert_eq!(resolved.evaluate_command.as_deref(), Some("./evaluate.sh"));
    }

    #[test]
    fn default_run_result_dir_is_unique_for_parallel_launches() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let first = default_run_result_dir(temp_dir.path(), "full-auto");
        let second = default_run_result_dir(temp_dir.path(), "full-auto");
        let workspace_component = sanitize_component(
            temp_dir
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("workspace"),
        );
        let first_name = first
            .file_name()
            .and_then(|name| name.to_str())
            .expect("first run dir name");
        let second_name = second
            .file_name()
            .and_then(|name| name.to_str())
            .expect("second run dir name");

        assert_ne!(first, second);
        assert!(first_name.ends_with(&format!("-{workspace_component}")));
        assert!(second_name.ends_with(&format!("-{workspace_component}")));
    }

    #[test]
    fn summarize_run_dir_reports_failure_and_stop_reason() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        write_json(
            &temp_dir.path().join("summary.json"),
            &serde_json::json!({
                "stop_reason": "max_steps",
                "logical_success": false,
                "process_exit_code": 0,
                "evaluation_passed": false,
            }),
        )
        .expect("summary");
        write_json(
            &temp_dir.path().join("metadata.json"),
            &serde_json::json!({
                "objective_file": "START_HERE.md",
                "evaluate_command": "./evaluate.sh",
                "provider": "nvidia",
                "model_id": "nvidia/qwen/qwen3-coder-480b-a35b-instruct",
                "logical_success": false,
                "process_exit_code": 0,
                "evaluation_passed": false,
            }),
        )
        .expect("metadata");
        fs::write(
            temp_dir.path().join("events.jsonl"),
            concat!(
                "{\"payload\":{\"event\":\"agent.path_resolution_failed\",\"request_path\":\"workspace/crates/reconciliation-core\",\"suggested_path\":\"crates/reconciliation-core\",\"reason\":\"redundant_workspace_prefix\"}}\n",
                "{\"payload\":{\"event\":\"agent.recovery_turn_queued\",\"action\":\"ls workspace/crates/reconciliation-core\",\"suggested_path\":\"crates/reconciliation-core\"}}\n",
                "{\"payload\":{\"event\":\"agent.parse_recovery_queued\",\"step\":2,\"error_class\":\"trailing_characters\",\"failures\":1,\"budget\":2,\"message\":\"[Parser] retry\"}}\n",
                "{\"payload\":{\"event\":\"assistant_turn_summary\",\"step\":2,\"assistant_message\":\"working\",\"actions\":[\"read_file foo\"],\"wrote_files\":false,\"validation_queued\":false,\"parse_warning_count\":1}}\n",
                "{\"payload\":{\"event\":\"tool_call_finished\",\"action\":\"replace_block crates/reconciliation-core/src/lib.rs\",\"status\":\"success\",\"action_kind\":\"replace_block\",\"target_path\":\"crates/reconciliation-core/src/lib.rs\",\"edit_summary\":\"replace 2 lines -> 3 lines\"}}\n",
                "{\"payload\":{\"event\":\"agent.verifier_queued\",\"plans\":[\"fmt\",\"workspace_tests\"],\"reason\":\"post_edit\"}}\n",
                "{\"payload\":{\"event\":\"tool_call_finished\",\"action\":\"read_file fixtures/out_of_order_recovery/bare/Cargo.toml\",\"status\":\"failure\",\"action_kind\":\"read_file\"}}\n",
                "{\"payload\":{\"event\":\"validation_finished\",\"summary\":\"./evaluate.sh proof-full\",\"status\":\"failure\"}}\n",
                "{\"payload\":{\"event\":\"run.retry_started\",\"attempt\":2}}\n"
            ),
        )
        .expect("events");

        let summary = summarize_run_dir(temp_dir.path()).expect("summary text");

        assert!(summary.contains("START_HERE.md"));
        assert!(summary.contains("First edit: crates/reconciliation-core/src/lib.rs"));
        assert!(summary.contains("First failing action: read_file fixtures/out_of_order_recovery/bare/Cargo.toml -> failure"));
        assert!(summary.contains("First bad path: workspace/crates/reconciliation-core"));
        assert!(summary.contains("Suggested correction: crates/reconciliation-core"));
        assert!(summary.contains("Recovery turns queued: 1"));
        assert!(summary.contains("Parser recovery turns queued: 1"));
        assert!(summary.contains("Parser warnings observed: 1"));
        assert!(summary.contains("Failure class: later validation/evaluator issue"));
        assert!(summary.contains("Full retries attempted: 1"));
        assert!(summary.contains("Validation queued before stop: true"));
        assert!(summary.contains("Evaluator logical success: false"));
        assert!(summary.contains("Evaluation passed: false"));
        assert!(summary.contains("Final stop reason: max_steps"));
    }

    #[test]
    fn run_evaluator_prefers_logical_success_from_json_output() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let script_path = temp_dir.path().join("evaluate.sh");
        fs::write(
            &script_path,
            "#!/bin/sh\ncat <<'EOF'\n{\"success\": false, \"command\": \"demo\"}\nEOF\nexit 0\n",
        )
        .expect("script");
        #[allow(clippy::disallowed_methods)]
        std::process::Command::new("chmod")
            .arg("+x")
            .arg(&script_path)
            .status()
            .expect("chmod");

        let outcome = run_evaluator(temp_dir.path(), "./evaluate.sh", None).expect("evaluate");

        assert!(outcome.process_passed);
        assert_eq!(outcome.process_exit_code, 0);
        assert_eq!(outcome.logical_success, Some(false));
        assert!(!outcome.evaluation_passed);
        assert!(outcome.parsed_from_stdout);
    }

    #[test]
    fn challenge_resolution_defaults_to_proof_full_and_substitutes_condition() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(temp_dir.path().join("workspace").join("proof-full"))
            .expect("workspace");
        fs::write(temp_dir.path().join("START_HERE.md"), "brief").expect("brief");
        fs::write(temp_dir.path().join("expected.md"), "success").expect("success");
        fs::write(
            temp_dir.path().join("benchmark.json"),
            serde_json::json!({
                "repo_condition": ["bare", "proof-full"],
                "objective_file": "START_HERE.md",
                "success_file": "expected.md",
                "reset_command": "./reset.sh <condition>",
                "evaluate_command": "./evaluate.sh <condition>",
            })
            .to_string(),
        )
        .expect("benchmark");

        let resolved = resolve_workspace_objective(temp_dir.path(), None).expect("resolved");

        assert_eq!(resolved.selected_condition.as_deref(), Some("proof-full"));
        assert_eq!(
            resolved.evaluate_command.as_deref(),
            Some("./evaluate.sh proof-full")
        );
        assert_eq!(
            resolved.reset_command.as_deref(),
            Some("./reset.sh proof-full")
        );
        assert_eq!(
            resolved.editable_workspace_root,
            temp_dir
                .path()
                .join("workspace")
                .join("proof-full")
                .canonicalize()
                .unwrap_or_else(|_| temp_dir.path().join("workspace").join("proof-full"))
        );
    }

    #[test]
    fn bundle_run_dir_creates_zip_file() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        fs::write(temp_dir.path().join("request.json"), "{}").expect("request");
        let output_path = temp_dir.path().join("bundle.zip");

        let bundle_path = bundle_run_dir(temp_dir.path(), &output_path).expect("bundle");

        assert_eq!(bundle_path, output_path);
        assert!(bundle_path.exists());
    }
}

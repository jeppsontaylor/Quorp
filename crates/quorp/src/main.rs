#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod quorp;

use ::paths;
use anyhow::Context as _;
use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, enable_raw_mode,
    size as terminal_size,
};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthChar;
use util::paths::PathWithPosition;

fn main() {
    let args = CliArgs::parse();

    init_logging(&args);

    if let Err(error) = run(args) {
        eprintln!("quorp: {error:#}");
        std::process::exit(1);
    }
}

fn init_logging(args: &CliArgs) {
    quorp_log::init();
    if let Some(path) = benchmark_log_file_override(args) {
        let leaked_path = Box::leak(Box::new(path));
        quorp_log::init_output_file(leaked_path, None).ok();
    } else {
        quorp_log::init_output_file(paths::log_file(), Some(paths::old_log_file())).ok();
    }
    quorp_tracing::init();
}

fn benchmark_log_file_override(args: &CliArgs) -> Option<PathBuf> {
    match args.command.as_ref()? {
        Command::Benchmark {
            command: BenchmarkCommand::Run(run_args),
        } => run_args.log_file.clone(),
        _ => None,
    }
}

fn run(args: CliArgs) -> anyhow::Result<()> {
    let tui_mode = args.tui;
    match args.command {
        Some(Command::Doctor) => crate::quorp::cli_demos::run_doctor_command(),
        Some(Command::Exec(args)) => run_exec_command(args),
        Some(Command::MemAnalyze) => run_mem_analyze(),
        Some(Command::MemLogPath) => run_mem_log_path(),
        Some(Command::Session(args)) => run_session(args, tui_mode),
        Some(Command::Run(args)) => run_autonomous_command(args),
        Some(Command::Diagnostics { command }) => run_diagnostics_command(command),
        Some(Command::Agent { command }) => run_agent_command(command),
        Some(Command::Benchmark { command }) => run_benchmark_command(command),
        Some(Command::RenderDemo) => crate::quorp::cli_demos::run_render_demo(),
        Some(Command::Commands { prefix }) => crate::quorp::cli_demos::run_commands_command(prefix),
        Some(Command::Scan { workspace, symbols }) => {
            crate::quorp::cli_demos::run_scan_command(workspace, symbols)
        }
        Some(Command::Permissions {
            mode,
            tool,
            capability,
            command,
            allow_command,
        }) => crate::quorp::cli_demos::run_permissions_command(
            mode.into(),
            tool,
            capability.map(Into::into),
            command,
            allow_command,
        ),
        None => run_inline_cli(SessionLaunchConfig::from_paths_or_urls(
            args.paths_or_urls,
            tui_mode,
            parse_prompt_compaction_policy_arg(args.prompt_compaction_policy.as_deref())?,
        )),
    }
}

fn load_workspace_settings(workspace: &Path) -> anyhow::Result<quorp_config::LoadedSettings> {
    quorp_config::load_settings(workspace).with_context(|| {
        format!(
            "failed to load QUORP settings for workspace {}",
            workspace.display()
        )
    })
}

fn default_provider_for_workspace(
    workspace: &Path,
) -> anyhow::Result<crate::quorp::executor::InteractiveProviderKind> {
    Ok(crate::quorp::executor::interactive_provider_for_workspace(
        &std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf()),
    ))
}

fn default_model_for_workspace(
    _workspace: &Path,
    _provider: crate::quorp::executor::InteractiveProviderKind,
) -> anyhow::Result<String> {
    Ok(crate::quorp::provider_config::NVIDIA_QWEN_MODEL.to_string())
}

fn default_base_url_for_workspace(
    workspace: &Path,
    provider: crate::quorp::executor::InteractiveProviderKind,
) -> anyhow::Result<Option<String>> {
    let loaded = load_workspace_settings(workspace)?;
    let base_url = loaded.settings.provider.base_url.trim();
    if base_url.is_empty() {
        return Ok(None);
    }
    let _ = provider;
    Ok(Some(base_url.to_string()))
}

fn default_sandbox_for_workspace(workspace: &Path) -> anyhow::Result<CliSandboxMode> {
    let loaded = load_workspace_settings(workspace)?;
    Ok(match loaded.settings.sandbox.mode {
        quorp_core::SandboxMode::Host => CliSandboxMode::Host,
        quorp_core::SandboxMode::TmpCopy => CliSandboxMode::TmpCopy,
    })
}

fn run_exec_command(args: ExecArgs) -> anyhow::Result<()> {
    let workspace = args.workspace.unwrap_or_else(default_workspace_root);
    let workspace = std::fs::canonicalize(&workspace).unwrap_or(workspace);
    let result_dir = args
        .result_dir
        .unwrap_or_else(|| crate::quorp::run_support::default_run_result_dir(&workspace, "exec"));
    std::fs::create_dir_all(&result_dir)?;
    let objective_file = result_dir.join("objective.md");
    std::fs::write(&objective_file, args.task)?;
    run_autonomous_command(RunCliArgs {
        command: None,
        start: RunArgs {
            workspace: Some(workspace),
            condition: None,
            objective_file: Some(objective_file),
            base_url: args.base_url,
            max_steps: args.max_steps,
            max_seconds: args.max_seconds,
            max_retries: 0,
            max_total_tokens: args.max_total_tokens,
            result_dir: Some(result_dir),
            sandbox: args.sandbox,
            keep_sandbox: args.keep_sandbox,
            autonomy_profile: args.autonomy_profile,
            yolo: args.yolo,
        },
    })
}

fn run_id_from_result_dir(result_dir: &Path) -> String {
    result_dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("run")
        .to_string()
}

fn resolve_objective_path_for_workspace(
    source_workspace: &Path,
    active_workspace: &Path,
    objective_file: &Path,
) -> PathBuf {
    if objective_file.is_absolute() {
        objective_file
            .strip_prefix(source_workspace)
            .map(|relative| active_workspace.join(relative))
            .unwrap_or_else(|_| objective_file.to_path_buf())
    } else {
        active_workspace.join(objective_file)
    }
}

fn resolve_run_objective(
    workspace: &Path,
    objective_file: Option<PathBuf>,
    condition: Option<&str>,
) -> anyhow::Result<crate::quorp::run_support::ResolvedWorkspaceObjective> {
    match (
        crate::quorp::run_support::resolve_workspace_objective(workspace, condition),
        objective_file,
    ) {
        (Ok(discovered), Some(objective_file)) => {
            Ok(crate::quorp::run_support::ResolvedWorkspaceObjective {
                objective_file,
                ..discovered
            })
        }
        (Ok(discovered), None) => Ok(discovered),
        (Err(error), Some(objective_file)) => {
            if !objective_file.exists() {
                return Err(error).with_context(|| {
                    format!(
                        "explicit objective file {} does not exist",
                        objective_file.display()
                    )
                });
            }
            let workspace_root =
                std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
            Ok(crate::quorp::run_support::ResolvedWorkspaceObjective {
                workspace_root: workspace_root.clone(),
                challenge_root: workspace_root.clone(),
                editable_workspace_root: workspace_root,
                editable_workspace_relative_root: None,
                objective_file,
                evaluate_command: None,
                reset_command: None,
                selected_condition: None,
                success_file: None,
                context_files: Vec::new(),
                repair_artifacts: Vec::new(),
                workspace_root_entries: Vec::new(),
                editable_workspace_entries: Vec::new(),
            })
        }
        (Err(error), None) => Err(error),
    }
}

struct RunProofReceiptInput<'a> {
    result_dir: &'a Path,
    source_workspace: &'a Path,
    active_workspace: &'a Path,
    sandbox_root: Option<PathBuf>,
    provider: &'a str,
    model_id: &'a str,
    resolved: &'a crate::quorp::run_support::ResolvedWorkspaceObjective,
    outcome: &'a quorp_agent_core::AgentRunOutcome,
    evaluation: Option<&'a crate::quorp::run_support::EvaluatorOutcome>,
}

fn write_run_proof_receipt(input: RunProofReceiptInput<'_>) -> anyhow::Result<()> {
    let mut receipt = quorp_core::ProofReceipt::new(run_id_from_result_dir(input.result_dir));
    receipt.sandbox_path = input.sandbox_root;
    receipt.changed_files = changed_files_for_workspace(input.active_workspace)?;
    receipt.provider = Some(input.provider.to_string());
    receipt.model = Some(input.model_id.to_string());
    receipt.usage.insert(
        "total_billed_tokens".to_string(),
        input.outcome.total_billed_tokens,
    );
    receipt.evaluator_result = input.evaluation.map(|evaluation| {
        format!(
            "passed={} process_exit_code={} logical_success={:?}",
            evaluation.evaluation_passed, evaluation.process_exit_code, evaluation.logical_success
        )
    });
    if let Some(evaluation) = input.evaluation {
        receipt.validation.push(quorp_core::ValidationRecord {
            command: evaluation.command.clone(),
            cwd: input.resolved.challenge_root.clone(),
            exit_code: evaluation.process_exit_code,
            raw_log_path: None,
            raw_log_sha256: None,
        });
    }
    for (name, path) in [
        ("request", input.result_dir.join("request.json")),
        ("metadata", input.result_dir.join("metadata.json")),
        ("summary", input.result_dir.join("summary.json")),
        ("events", input.result_dir.join("events.jsonl")),
    ] {
        if path.exists() {
            receipt.raw_artifacts.insert(
                name.to_string(),
                quorp_core::RawArtifact {
                    sha256: sha256_file_if_exists(&path)?,
                    path,
                },
            );
        }
    }
    if input.source_workspace != input.active_workspace {
        receipt.residual_risks.push(
            "run used a copied workspace; inspect sandbox path or result artifacts for final edits"
                .to_string(),
        );
    }
    if input.outcome.stop_reason != quorp_agent_core::StopReason::Success
        && input
            .evaluation
            .is_none_or(|evaluation| !evaluation.evaluation_passed)
    {
        receipt
            .residual_risks
            .push("agent run did not reach a successful stop condition".to_string());
    }
    crate::quorp::run_support::write_json(&input.result_dir.join("proof-receipt.json"), &receipt)
}

fn changed_files_for_workspace(workspace: &Path) -> anyhow::Result<Vec<PathBuf>> {
    #[allow(clippy::disallowed_methods)]
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .arg("diff")
        .arg("--name-only")
        .output()
        .with_context(|| format!("failed to inspect changed files in {}", workspace.display()))?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(PathBuf::from)
        .collect())
}

fn sha256_file_if_exists(path: &Path) -> anyhow::Result<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut hasher = Sha256::new();
            hasher.update(bytes);
            Ok(Some(format!("{:x}", hasher.finalize())))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn run_autonomous_command(args: RunCliArgs) -> anyhow::Result<()> {
    match args.command {
        Some(RunSubcommand::Resume(args)) => {
            let outcome =
                crate::quorp::agent_runner::resume_headless_agent(args.result_dir.clone())?;
            crate::quorp::run_support::snapshot_logs(&args.result_dir, None)?;
            println!("Run directory: {}", args.result_dir.display());
            if outcome.stop_reason == quorp_agent_core::StopReason::Success {
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "autonomous resume stopped with {:?}; see {}",
                    outcome.stop_reason,
                    args.result_dir.display()
                ))
            }
        }
        None => {
            let start = args.start;
            let workspace_arg = start
                .workspace
                .clone()
                .ok_or_else(|| anyhow::anyhow!("`quorp run` requires --workspace <dir>"))?;
            let source_workspace =
                std::fs::canonicalize(&workspace_arg).unwrap_or_else(|_| workspace_arg.clone());
            let result_dir = start.result_dir.clone().unwrap_or_else(|| {
                crate::quorp::run_support::default_run_result_dir(&source_workspace, "run")
            });
            std::fs::create_dir_all(&result_dir)?;
            let provider = start
                .base_url
                .as_ref()
                .map(|_| crate::quorp::executor::InteractiveProviderKind::Nvidia)
                .unwrap_or(default_provider_for_workspace(&source_workspace)?);
            let model_id = default_model_for_workspace(&source_workspace, provider)?;
            let base_url_override = start
                .base_url
                .clone()
                .map(Some)
                .unwrap_or(default_base_url_for_workspace(&source_workspace, provider)?);
            let (sandbox_override, autonomy_profile_label) =
                resolve_yolo_run_mode(start.yolo, start.sandbox, start.autonomy_profile)?;
            let sandbox_mode = quorp_core::SandboxMode::from(
                sandbox_override.unwrap_or(default_sandbox_for_workspace(&source_workspace)?),
            );
            let sandbox_lease = match sandbox_mode {
                quorp_core::SandboxMode::Host => None,
                quorp_core::SandboxMode::TmpCopy => Some(quorp_sandbox::create_sandbox(
                    quorp_sandbox::SandboxRequest {
                        source_workspace: source_workspace.clone(),
                        run_id: run_id_from_result_dir(&result_dir),
                        attempt: 1,
                        mode: quorp_core::SandboxMode::TmpCopy,
                        keep_sandbox: start.keep_sandbox,
                    },
                )?),
            };
            let workspace = sandbox_lease
                .as_ref()
                .map(|lease| lease.workspace_path().to_path_buf())
                .unwrap_or_else(|| source_workspace.clone());
            let objective_file = start.objective_file.as_ref().map(|objective_file| {
                resolve_objective_path_for_workspace(&source_workspace, &workspace, objective_file)
            });
            let resolved =
                resolve_run_objective(&workspace, objective_file, start.condition.as_deref())?;
            apply_session_env_overrides(&SessionLaunchConfig {
                workspace_root: resolved.editable_workspace_root.clone(),
                provider: Some(provider),
                model: Some(model_id.clone()),
                base_url: base_url_override.clone(),
                prompt_compaction_policy: None,
                tui_mode: CliTuiMode::Auto,
                initial_prompt: None,
            });

            let mut final_outcome: Option<quorp_agent_core::AgentRunOutcome> = None;
            let mut final_evaluation: Option<crate::quorp::run_support::EvaluatorOutcome> = None;
            let mut retry_context: Option<String> = None;
            let mut attempts_run = 0usize;
            let max_attempts = start.max_retries.saturating_add(1).max(1);
            let autonomy_profile = parse_autonomy_profile(&autonomy_profile_label)?;

            for attempt in 1..=max_attempts {
                attempts_run = attempt;
                let attempt_dir = result_dir.join(format!("attempt-{attempt:03}"));
                std::fs::create_dir_all(&attempt_dir)?;
                if attempt > 1 {
                    crate::quorp::run_support::append_named_event(
                        &result_dir.join("events.jsonl"),
                        "run.retry_started",
                        serde_json::json!({
                            "attempt": attempt,
                            "max_attempts": max_attempts,
                        }),
                    )?;
                }

                let mut objective_metadata =
                    crate::quorp::run_support::objective_metadata_json(&resolved, &workspace);
                if let Some(retry_context_text) = retry_context.as_ref()
                    && let Some(object) = objective_metadata.as_object_mut()
                {
                    object.insert(
                        "retry_context".to_string(),
                        serde_json::Value::String(retry_context_text.clone()),
                    );
                }

                let outcome = crate::quorp::agent_runner::run_headless_agent(
                    crate::quorp::agent_runner::HeadlessRunOptions {
                        workspace: resolved.editable_workspace_root.clone(),
                        objective_file: resolved.objective_file.clone(),
                        model_id: model_id.clone(),
                        base_url_override: base_url_override.clone(),
                        max_steps: start.max_steps,
                        max_seconds: Some(start.max_seconds),
                        max_total_tokens: start.max_total_tokens,
                        result_dir: attempt_dir.clone(),
                        autonomy_profile,
                        completion_policy: quorp_agent_core::CompletionPolicy::default(),
                        objective_metadata,
                        seed_context: Vec::new(),
                    },
                )?;
                crate::quorp::run_support::snapshot_logs(&attempt_dir, None)?;
                crate::quorp::run_support::append_event_log(
                    &result_dir.join("events.jsonl"),
                    &attempt_dir.join("events.jsonl"),
                )?;
                crate::quorp::run_support::copy_run_artifacts_without_events(
                    &attempt_dir,
                    &result_dir,
                )?;

                crate::quorp::run_support::append_named_event(
                    &result_dir.join("events.jsonl"),
                    "run.phase_changed",
                    serde_json::json!({
                        "phase": "evaluating",
                        "attempt": attempt,
                        "detail": resolved.evaluate_command,
                    }),
                )?;
                let evaluation = if let Some(command) = resolved.evaluate_command.as_ref() {
                    Some(crate::quorp::run_support::run_evaluator(
                        &resolved.challenge_root,
                        command,
                        resolved.selected_condition.as_deref(),
                    )?)
                } else {
                    None
                };
                if let Some(evaluation_outcome) = evaluation.as_ref() {
                    crate::quorp::run_support::append_named_event(
                        &result_dir.join("events.jsonl"),
                        "run.evaluator_result",
                        serde_json::json!({
                            "attempt": attempt,
                            "command": evaluation_outcome.command,
                            "process_exit_code": evaluation_outcome.process_exit_code,
                            "process_passed": evaluation_outcome.process_passed,
                            "logical_success": evaluation_outcome.logical_success,
                            "evaluation_passed": evaluation_outcome.evaluation_passed,
                            "parsed_json_path": evaluation_outcome.parsed_json_path,
                            "parsed_from_stdout": evaluation_outcome.parsed_from_stdout,
                        }),
                    )?;
                }
                if attempt > 1 {
                    if let Some(evaluation_outcome) = evaluation.as_ref() {
                        crate::quorp::run_support::append_named_event(
                            &result_dir.join("events.jsonl"),
                            "run.retry_finished",
                            serde_json::json!({
                                "attempt": attempt,
                                "stop_reason": format!("{:?}", outcome.stop_reason),
                                "evaluator_command": evaluation_outcome.command,
                                "process_exit_code": evaluation_outcome.process_exit_code,
                                "process_passed": evaluation_outcome.process_passed,
                                "logical_success": evaluation_outcome.logical_success,
                                "evaluator_passed": evaluation_outcome.evaluation_passed,
                            }),
                        )?;
                    } else {
                        crate::quorp::run_support::append_named_event(
                            &result_dir.join("events.jsonl"),
                            "run.retry_finished",
                            serde_json::json!({
                                "attempt": attempt,
                                "stop_reason": format!("{:?}", outcome.stop_reason),
                                "evaluator_command": serde_json::Value::Null,
                                "evaluator_passed": outcome.stop_reason == quorp_agent_core::StopReason::Success,
                            }),
                        )?;
                    }
                }

                let passed = evaluation
                    .as_ref()
                    .map(|outcome| outcome.evaluation_passed)
                    .unwrap_or_else(|| {
                        outcome.stop_reason == quorp_agent_core::StopReason::Success
                    });

                final_outcome = Some(outcome);
                final_evaluation = evaluation.clone();

                if passed {
                    break;
                }

                if attempt < max_attempts {
                    retry_context = Some(match evaluation.as_ref() {
                        Some(outcome) => format!(
                            "Attempt {attempt} failed evaluator `{}` (process exit {}, logical success {:?}). Stdout:\n{}\nStderr:\n{}",
                            outcome.command,
                            outcome.process_exit_code,
                            outcome.logical_success,
                            outcome.stdout.trim(),
                            outcome.stderr.trim()
                        ),
                        None => format!(
                            "Attempt {attempt} stopped with {:?}.",
                            final_outcome
                                .as_ref()
                                .map(|outcome| outcome.stop_reason)
                                .unwrap_or(quorp_agent_core::StopReason::FatalError)
                        ),
                    });
                    if let Some(reset_command) = resolved.reset_command.as_ref() {
                        crate::quorp::run_support::append_named_event(
                            &result_dir.join("events.jsonl"),
                            "run.phase_changed",
                            serde_json::json!({
                                "phase": "retrying",
                                "attempt": attempt + 1,
                                "detail": reset_command,
                            }),
                        )?;
                        let reset_outcome = crate::quorp::run_support::run_command(
                            &resolved.challenge_root,
                            reset_command,
                        )?;
                        crate::quorp::run_support::append_named_event(
                            &result_dir.join("events.jsonl"),
                            "run.reset_finished",
                            serde_json::json!({
                                "attempt": attempt,
                                "command": reset_outcome.command,
                                "exit_code": reset_outcome.exit_code,
                                "passed": reset_outcome.passed,
                            }),
                        )?;
                        if !reset_outcome.passed {
                            anyhow::bail!(
                                "reset command `{}` failed with exit code {}",
                                reset_outcome.command,
                                reset_outcome.exit_code
                            );
                        }
                    }
                }
            }

            let outcome = final_outcome.ok_or_else(|| {
                anyhow::anyhow!("autonomous run did not record an attempt outcome")
            })?;
            crate::quorp::run_support::write_json(
                &result_dir.join("request.json"),
                &serde_json::json!({
                    "workspace": workspace.clone(),
                    "source_workspace": source_workspace.clone(),
                    "challenge_root": resolved.challenge_root.clone(),
                    "editable_workspace_root": resolved.editable_workspace_root.clone(),
                    "objective_file": resolved.objective_file.clone(),
                    "sandbox": format!("{:?}", sandbox_mode),
                    "sandbox_backend": sandbox_lease.as_ref().map(|lease| format!("{:?}", lease.backend())),
                    "sandbox_source_workspace": sandbox_lease.as_ref().map(|lease| lease.source_workspace().to_path_buf()),
                    "sandbox_root": sandbox_lease.as_ref().map(|lease| lease.sandbox_root().to_path_buf()),
                    "provider": provider.label(),
                    "model_id": model_id.clone(),
                    "condition": resolved.selected_condition.clone(),
                    "max_retries": start.max_retries,
                    "evaluate_command": resolved.evaluate_command.clone(),
                    "reset_command": resolved.reset_command.clone(),
                    "runtime": {"mode": "native"},
                }),
            )?;
            crate::quorp::run_support::write_json(
                &result_dir.join("metadata.json"),
                &serde_json::json!({
                    "workspace": workspace.clone(),
                    "source_workspace": source_workspace.clone(),
                    "challenge_root": resolved.challenge_root.clone(),
                    "editable_workspace_root": resolved.editable_workspace_root.clone(),
                    "objective_file": resolved.objective_file.clone(),
                    "sandbox": format!("{:?}", sandbox_mode),
                    "sandbox_backend": sandbox_lease.as_ref().map(|lease| format!("{:?}", lease.backend())),
                    "sandbox_source_workspace": sandbox_lease.as_ref().map(|lease| lease.source_workspace().to_path_buf()),
                    "sandbox_root": sandbox_lease.as_ref().map(|lease| lease.sandbox_root().to_path_buf()),
                    "provider": provider.label(),
                    "model_id": model_id.clone(),
                    "condition": resolved.selected_condition.clone(),
                    "attempts_run": attempts_run,
                    "evaluate_command": resolved.evaluate_command.clone(),
                    "reset_command": resolved.reset_command.clone(),
                    "last_evaluation": final_evaluation.clone(),
                    "process_exit_code": final_evaluation.as_ref().map(|evaluation| evaluation.process_exit_code),
                    "process_passed": final_evaluation.as_ref().map(|evaluation| evaluation.process_passed),
                    "logical_success": final_evaluation.as_ref().and_then(|evaluation| evaluation.logical_success),
                    "evaluation_passed": final_evaluation.as_ref().map(|evaluation| evaluation.evaluation_passed),
                    "objective": crate::quorp::run_support::objective_metadata_json(&resolved, &workspace),
                    "runtime": {"mode": "native"},
                }),
            )?;
            crate::quorp::run_support::write_json(
                &result_dir.join("summary.json"),
                &serde_json::json!({
                    "stop_reason": outcome.stop_reason,
                    "total_steps": outcome.total_steps,
                    "total_billed_tokens": outcome.total_billed_tokens,
                    "duration_ms": outcome.duration_ms,
                    "error_message": outcome.error_message,
                    "process_exit_code": final_evaluation.as_ref().map(|evaluation| evaluation.process_exit_code),
                    "process_passed": final_evaluation.as_ref().map(|evaluation| evaluation.process_passed),
                    "logical_success": final_evaluation.as_ref().and_then(|evaluation| evaluation.logical_success),
                    "evaluation_passed": final_evaluation.as_ref().map(|evaluation| evaluation.evaluation_passed),
                }),
            )?;
            crate::quorp::run_support::append_named_event(
                &result_dir.join("events.jsonl"),
                "run.stop_cause",
                serde_json::json!({
                    "stop_reason": outcome.stop_reason,
                    "error_message": outcome.error_message,
                    "evaluation_passed": final_evaluation.as_ref().map(|evaluation| evaluation.evaluation_passed),
                    "logical_success": final_evaluation.as_ref().and_then(|evaluation| evaluation.logical_success),
                }),
            )?;
            write_run_proof_receipt(RunProofReceiptInput {
                result_dir: &result_dir,
                source_workspace: &source_workspace,
                active_workspace: &workspace,
                sandbox_root: sandbox_lease
                    .as_ref()
                    .map(|lease| lease.sandbox_root().to_path_buf()),
                provider: provider.label(),
                model_id: &model_id,
                resolved: &resolved,
                outcome: &outcome,
                evaluation: final_evaluation.as_ref(),
            })?;
            println!("Run directory: {}", result_dir.display());
            let success = final_evaluation
                .as_ref()
                .map(|evaluation| evaluation.evaluation_passed)
                .unwrap_or_else(|| outcome.stop_reason == quorp_agent_core::StopReason::Success);
            if success {
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "autonomous run stopped with {:?}; see {}",
                    outcome.stop_reason,
                    result_dir.display()
                ))
            }
        }
    }
}

fn run_diagnostics_command(command: DiagnosticsCommand) -> anyhow::Result<()> {
    match command {
        DiagnosticsCommand::Path => {
            let paths = crate::quorp::run_support::diagnostics_paths();
            println!("Logs directory: {}", paths.logs_dir.display());
            println!("Quorp log: {}", paths.quorp_log.display());
            println!("Memory log: {}", paths.memory_log.display());
            println!(
                "TUI diagnostics log: {}",
                paths.tui_diagnostics_log.display()
            );
            if let Some(latest) = crate::quorp::run_support::latest_run_dir(None)? {
                println!("Latest run: {}", latest.run_dir.display());
            }
            Ok(())
        }
        DiagnosticsCommand::Bundle(args) => {
            let run_dir = resolve_run_dir_arg(args.run, args.latest)?;
            let output_path = args
                .output
                .unwrap_or_else(|| crate::quorp::run_support::default_bundle_path(&run_dir));
            let bundle = crate::quorp::run_support::bundle_run_dir(&run_dir, &output_path)?;
            println!("{}", bundle.display());
            Ok(())
        }
        DiagnosticsCommand::Summarize(args) => {
            let run_dir = resolve_run_dir_arg(args.run, args.latest)?;
            println!(
                "{}",
                crate::quorp::run_support::summarize_run_dir(&run_dir)?
            );
            Ok(())
        }
    }
}

fn resolve_run_dir_arg(run: Option<PathBuf>, latest: bool) -> anyhow::Result<PathBuf> {
    if let Some(run_dir) = run {
        return Ok(run_dir);
    }
    if latest {
        return crate::quorp::run_support::latest_run_dir(None)?
            .map(|info| info.run_dir)
            .ok_or_else(|| anyhow::anyhow!("no recorded runs found"));
    }
    Err(anyhow::anyhow!(
        "pass --run <dir> or --latest to choose a diagnostics bundle"
    ))
}

fn run_agent_command(command: AgentCommand) -> anyhow::Result<()> {
    match command {
        AgentCommand::Run(args) => {
            let objective_file = args.objective_file.clone();
            let outcome = crate::quorp::agent_runner::run_headless_agent(
                crate::quorp::agent_runner::HeadlessRunOptions {
                    workspace: std::fs::canonicalize(&args.workspace)
                        .unwrap_or_else(|_| args.workspace.clone()),
                    objective_file: PathBuf::from(objective_file.clone()),
                    model_id: crate::quorp::provider_config::NVIDIA_QWEN_MODEL.to_string(),
                    base_url_override: args.base_url,
                    max_steps: args.max_steps,
                    max_seconds: Some(args.max_seconds),
                    max_total_tokens: args.max_total_tokens,
                    result_dir: args.result_dir,
                    autonomy_profile: parse_autonomy_profile(&args.autonomy_profile)?,
                    completion_policy: quorp_agent_core::CompletionPolicy::default(),
                    objective_metadata: serde_json::json!({
                        "objective_file": objective_file,
                    }),
                    seed_context: Vec::new(),
                },
            )?;
            if outcome.stop_reason == quorp_agent_core::StopReason::Success {
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "headless agent run stopped with {:?}",
                    outcome.stop_reason
                ))
            }
        }
        AgentCommand::Resume(args) => {
            let outcome = crate::quorp::agent_runner::resume_headless_agent(args.result_dir)?;
            if outcome.stop_reason == quorp_agent_core::StopReason::Success {
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "headless agent resume stopped with {:?}",
                    outcome.stop_reason
                ))
            }
        }
    }
}

fn run_benchmark_command(command: BenchmarkCommand) -> anyhow::Result<()> {
    match command {
        BenchmarkCommand::Run(args) => {
            let workspace = default_workspace_root();
            let provider = default_provider_for_workspace(&workspace)?;
            let model = default_model_for_workspace(&workspace, provider)?;
            let base_url = args
                .base_url
                .clone()
                .map(Some)
                .unwrap_or(default_base_url_for_workspace(&workspace, provider)?);
            apply_session_env_overrides(&SessionLaunchConfig::from_workspace(
                workspace.clone(),
                CliTuiMode::Auto,
                Some(provider),
                Some(model.clone()),
                base_url.clone(),
                None,
            ));
            let result_dir = args
                .result_dir
                .clone()
                .unwrap_or_else(crate::quorp::run_support::default_benchmark_run_result_dir);
            let compaction_policy = crate::quorp::benchmark::parse_prompt_compaction_policy(
                args.compaction_policy.as_deref(),
            )?;
            crate::quorp::benchmark::run_benchmark(crate::quorp::benchmark::BenchmarkRunOptions {
                path: std::fs::canonicalize(&args.path).unwrap_or_else(|_| args.path.clone()),
                executor: crate::quorp::benchmark::BenchmarkExecutor::Native,
                model_id: Some(model),
                base_url_override: base_url,
                briefing_file: Some(args.briefing_file),
                compaction_policy,
                seed_transcript: args.seed_transcript,
                max_steps: args.max_steps,
                max_seconds: Some(args.max_seconds),
                max_total_tokens: args.token_budget,
                result_dir: result_dir.clone(),
                autonomy_profile: parse_autonomy_profile(&args.autonomy_profile)?,
                max_attempts: args.max_attempts,
                condition: args.condition,
                keep_sandbox: args.keep_sandbox,
            })
            .and_then(|_| ensure_benchmark_succeeded(&result_dir))
        }
        BenchmarkCommand::Prompt(args) => {
            let workspace = default_workspace_root();
            let provider = default_provider_for_workspace(&workspace)?;
            let model = default_model_for_workspace(&workspace, provider)?;
            let base_url = default_base_url_for_workspace(&workspace, provider)?;
            apply_session_env_overrides(&SessionLaunchConfig::from_workspace(
                workspace.clone(),
                CliTuiMode::Auto,
                Some(provider),
                Some(model.clone()),
                base_url,
                None,
            ));
            let bundle = crate::quorp::benchmark::prepare_benchmark_prompt_bundle(
                &args.path,
                &args.workspace_dir,
                crate::quorp::benchmark::BenchmarkExecutor::Native,
                Some(model),
                Some(args.briefing_file.as_path()),
                args.max_steps,
                Some(args.max_seconds),
                args.token_budget,
            )?;
            println!("{}", serde_json::to_string_pretty(&bundle)?);
            Ok(())
        }
        BenchmarkCommand::Resume(args) => crate::quorp::benchmark::resume_benchmark(
            crate::quorp::benchmark::BenchmarkResumeOptions {
                result_dir: args.result_dir.clone(),
            },
        )
        .and_then(|_| ensure_benchmark_succeeded(&args.result_dir)),
        BenchmarkCommand::Score(args) => {
            let artifacts = crate::quorp::benchmark::score_benchmark_reports(
                crate::quorp::benchmark::BenchmarkScoreOptions {
                    run_dirs: args.run_dirs,
                    suite: args.suite,
                    reports_root: args.reports_root,
                    output_root: args.output_root,
                    fail_on_regression: args.fail_on_regression,
                },
            )?;
            println!("{}", artifacts.markdown);
            eprintln!("Scoreboard written to {}", artifacts.output_dir.display());
            Ok(())
        }
        BenchmarkCommand::Batch(args) => {
            let workspace = default_workspace_root();
            let provider = default_provider_for_workspace(&workspace)?;
            let model = default_model_for_workspace(&workspace, provider)?;
            let base_url = args
                .base_url
                .clone()
                .map(Some)
                .unwrap_or(default_base_url_for_workspace(&workspace, provider)?);
            apply_session_env_overrides(&SessionLaunchConfig::from_workspace(
                workspace,
                CliTuiMode::Auto,
                Some(provider),
                Some(model.clone()),
                base_url.clone(),
                None,
            ));
            let compaction_policy = crate::quorp::benchmark::parse_prompt_compaction_policy(
                args.compaction_policy.as_deref(),
            )?;
            crate::quorp::benchmark::run_benchmark_batch(
                crate::quorp::benchmark::BenchmarkBatchRunOptions {
                    cases_root: std::fs::canonicalize(&args.cases_root)
                        .unwrap_or_else(|_| args.cases_root.clone()),
                    result_dir: args.result_dir.unwrap_or_else(
                        crate::quorp::run_support::default_benchmark_batch_result_dir,
                    ),
                    executor: crate::quorp::benchmark::BenchmarkExecutor::Native,
                    model_id: Some(model),
                    base_url_override: base_url,
                    briefing_file: Some(args.briefing_file),
                    compaction_policy,
                    seed_transcript: args.seed_transcript,
                    max_steps: args.max_steps,
                    max_seconds: Some(args.max_seconds),
                    max_total_tokens: args.token_budget,
                    max_attempts: args.max_attempts,
                    autonomy_profile: parse_autonomy_profile(&args.autonomy_profile)?,
                    condition: args.condition,
                    keep_sandbox: args.keep_sandbox,
                    log_dir: args.log_dir,
                },
            )
        }
    }
}

fn ensure_benchmark_succeeded(result_dir: &Path) -> anyhow::Result<()> {
    let report_path = result_dir.join("benchmark-report.json");
    let report_text = std::fs::read_to_string(&report_path)
        .with_context(|| format!("failed to read {}", report_path.display()))?;
    let report: serde_json::Value = serde_json::from_str(&report_text)
        .with_context(|| format!("failed to parse {}", report_path.display()))?;
    if report
        .get("success")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(());
    }
    let exit_code = report
        .get("exit_code")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(1);
    anyhow::bail!(
        "benchmark run did not succeed (exit_code={exit_code}); see {}",
        report_path.display()
    );
}

fn parse_autonomy_profile(value: &str) -> anyhow::Result<quorp_agent_core::AutonomyProfile> {
    match value.trim() {
        "interactive" => Ok(quorp_agent_core::AutonomyProfile::Interactive),
        "autonomous_host" => Ok(quorp_agent_core::AutonomyProfile::AutonomousHost),
        "autonomous_sandboxed" => Ok(quorp_agent_core::AutonomyProfile::AutonomousSandboxed),
        other => Err(anyhow::anyhow!("unknown autonomy profile `{other}`")),
    }
}

fn resolve_yolo_run_mode(
    yolo: bool,
    sandbox: Option<CliSandboxMode>,
    autonomy_profile: String,
) -> anyhow::Result<(Option<CliSandboxMode>, String)> {
    if !yolo {
        return Ok((sandbox, autonomy_profile));
    }
    if matches!(sandbox, Some(CliSandboxMode::Host)) {
        anyhow::bail!("--yolo requires an isolated sandbox; remove --sandbox host");
    }
    Ok((
        Some(CliSandboxMode::TmpCopy),
        "autonomous_sandboxed".to_string(),
    ))
}

fn parse_prompt_compaction_policy_arg(
    value: Option<&str>,
) -> anyhow::Result<Option<quorp_agent_core::PromptCompactionPolicy>> {
    value
        .map(|raw| {
            quorp_agent_core::PromptCompactionPolicy::parse(raw)
                .ok_or_else(|| anyhow::anyhow!("unknown compaction policy `{raw}`"))
        })
        .transpose()
}

fn run_session(args: SessionArgs, tui_mode: CliTuiMode) -> anyhow::Result<()> {
    let workspace = args.workspace.unwrap_or_else(default_workspace_root);
    let provider = default_provider_for_workspace(&workspace)?;
    let model = default_model_for_workspace(&workspace, provider)?;
    let base_url = default_base_url_for_workspace(&workspace, provider)?;
    let launch = SessionLaunchConfig::from_workspace(
        workspace,
        tui_mode,
        Some(provider),
        Some(model),
        base_url,
        parse_prompt_compaction_policy_arg(args.prompt_compaction_policy.as_deref())?,
    );
    run_inline_cli(launch)
}

fn run_inline_cli(launch: SessionLaunchConfig) -> anyhow::Result<()> {
    use std::io::{self, IsTerminal as _, Write};

    apply_session_env_overrides(&launch);
    let workspace_root = launch.workspace_root.clone();
    let model = launch.model.as_deref().unwrap_or("default remote model");
    let loaded = load_workspace_settings(&workspace_root)?;
    let mut run_mode = quorp_core::RunMode::Act;
    let mut permission_mode = loaded.settings.permissions.mode;
    let mut sandbox = loaded.settings.sandbox.mode;

    let color = quorp_render::RenderProfile::detect_from_env().color;
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let use_fullscreen =
        interactive && matches!(launch.tui_mode, CliTuiMode::Auto | CliTuiMode::Fullscreen);
    if use_fullscreen {
        return run_fullscreen_cli(launch, run_mode, permission_mode, sandbox);
    }
    if interactive {
        print_inline_startup_splash(&workspace_root, model, permission_mode, sandbox, color)?;
    }
    println!(
        "{}",
        quorp_render::render_session_frame(
            &quorp_render::SessionFrame {
                title: "ad hoc agent ready".to_string(),
                subtitle: format!("{model} · {}", workspace_root.display()),
                tasks: vec![
                    quorp_render::TaskRow {
                        label: "Ad hoc mode: type a task or use /plan, /act, /full-auto, /sandbox tmp-copy"
                            .to_string(),
                        state: quorp_render::TaskState::Active,
                    },
                    quorp_render::TaskRow {
                        label: format!(
                            "mode={run_mode:?} permissions={permission_mode:?} sandbox={sandbox:?}"
                        ),
                        state: quorp_render::TaskState::Done,
                    },
                ],
                commands: Vec::new(),
                footer: "NVIDIA/OpenAI-compatible provider · scrollback-native renderer"
                    .to_string(),
            },
            86,
            color,
        )
    );

    if let Some(prompt) = launch
        .initial_prompt
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        return run_inline_task(
            &workspace_root,
            launch.clone(),
            prompt.to_string(),
            run_mode,
            permission_mode,
            sandbox,
        );
    }

    if interactive {
        let mut composer =
            crate::quorp::inline_composer::TerminalComposer::new(quorp_slash::Registry::new());
        loop {
            let prompt = inline_prompt(color);
            let Some(input) = composer.read_line(&prompt, color)? else {
                break;
            };
            let input = input.trim();
            if input.is_empty() {
                continue;
            }
            if !handle_inline_input(
                input,
                &workspace_root,
                &launch,
                &mut run_mode,
                &mut permission_mode,
                &mut sandbox,
            )? {
                break;
            }
        }
        return Ok(());
    }

    let stdin = io::stdin();
    let mut line = String::new();
    loop {
        print!("{}", inline_prompt(color));
        io::stdout().flush()?;
        line.clear();
        if stdin.read_line(&mut line)? == 0 {
            break;
        }
        let input = line.trim();
        if !handle_inline_input(
            input,
            &workspace_root,
            &launch,
            &mut run_mode,
            &mut permission_mode,
            &mut sandbox,
        )? {
            break;
        }
    }
    Ok(())
}

fn print_inline_startup_splash(
    workspace_root: &Path,
    model: &str,
    permission_mode: quorp_core::PermissionMode,
    sandbox: quorp_core::SandboxMode,
    color: quorp_render::ColorCapability,
) -> anyhow::Result<()> {
    use quorp_render::splash::{SplashStatus, SplashStep, render_splash};
    use std::io::Write as _;

    print!(
        "{}",
        crate::quorp::inline_composer::render_quorp_loader("quorp · terminal runtime", color)
    );
    let steps = [
        SplashStep {
            name: "workspace".into(),
            detail: workspace_root.display().to_string(),
            status: SplashStatus::Done,
        },
        SplashStep {
            name: "provider".into(),
            detail: model.to_string(),
            status: SplashStatus::Done,
        },
        SplashStep {
            name: "sandbox".into(),
            detail: format!("{sandbox:?}"),
            status: SplashStatus::Done,
        },
        SplashStep {
            name: "permissions".into(),
            detail: format!("{permission_mode:?}"),
            status: SplashStatus::Done,
        },
        SplashStep {
            name: "slash".into(),
            detail: "live command palette armed".into(),
            status: SplashStatus::Done,
        },
    ];
    print!("{}", render_splash("boot checklist", &steps, color));
    println!();
    std::io::stdout().flush()?;
    Ok(())
}

fn inline_prompt(color: quorp_render::ColorCapability) -> String {
    if matches!(color, quorp_render::ColorCapability::NoColor) {
        return "> ".to_string();
    }
    format!(
        "{}>{} ",
        quorp_render::palette::ACCENT_YELLOW.fg(),
        quorp_render::palette::RESET
    )
}

fn handle_inline_input(
    input: &str,
    workspace_root: &Path,
    launch: &SessionLaunchConfig,
    run_mode: &mut quorp_core::RunMode,
    permission_mode: &mut quorp_core::PermissionMode,
    sandbox: &mut quorp_core::SandboxMode,
) -> anyhow::Result<bool> {
    if input.is_empty() {
        return Ok(true);
    }
    if matches!(input, "/quit" | "/exit") {
        return Ok(false);
    }
    if let Some(command) = quorp_term::parse_slash_command(input) {
        match command {
            quorp_term::SlashCommand::Doctor => crate::quorp::cli_demos::run_doctor_command()?,
            quorp_term::SlashCommand::Help => print_inline_help(),
            quorp_term::SlashCommand::Unknown(name) => {
                println!(
                    "{}",
                    quorp_term::render_card(&quorp_term::TranscriptCard::ApprovalWarning {
                        title: format!("unknown slash command /{name}"),
                        detail: "try /help for supported commands".to_string(),
                    })
                );
            }
            other => {
                quorp_term::apply_mode_command(&other, run_mode, permission_mode, sandbox);
                println!(
                    "{}",
                    quorp_term::render_card(&quorp_term::TranscriptCard::ToolCall {
                        name: "mode".to_string(),
                        detail: format!(
                            "run={run_mode:?} permissions={permission_mode:?} sandbox={sandbox:?}"
                        ),
                    })
                );
            }
        }
        return Ok(true);
    }
    run_inline_task(
        workspace_root,
        launch.clone(),
        input.to_string(),
        *run_mode,
        *permission_mode,
        *sandbox,
    )?;
    Ok(true)
}

fn run_inline_task(
    workspace_root: &Path,
    launch: SessionLaunchConfig,
    task: String,
    run_mode: quorp_core::RunMode,
    permission_mode: quorp_core::PermissionMode,
    sandbox: quorp_core::SandboxMode,
) -> anyhow::Result<()> {
    let color = quorp_render::RenderProfile::detect_from_env().color;
    let (terminal_width, _) = match terminal_size() {
        Ok((width, _)) => (usize::from(width), 0usize),
        Err(_) => (86usize, 0usize),
    };
    let autonomy_profile = match run_mode {
        quorp_core::RunMode::Plan => quorp_agent_core::AutonomyProfile::Interactive,
        quorp_core::RunMode::Act => {
            if matches!(sandbox, quorp_core::SandboxMode::TmpCopy) {
                quorp_agent_core::AutonomyProfile::AutonomousSandboxed
            } else {
                quorp_agent_core::AutonomyProfile::AutonomousHost
            }
        }
    };
    let mode_label = if matches!(run_mode, quorp_core::RunMode::Plan) {
        "Plan"
    } else {
        "Act"
    };
    let color_plan_indicator = if matches!(run_mode, quorp_core::RunMode::Plan) {
        format!(
            " {}Plan mode{} ",
            quorp_render::palette::ACCENT_VIOLET.fg(),
            quorp_render::palette::RESET
        )
    } else {
        String::new()
    };
    let result_dir = crate::quorp::run_support::default_run_result_dir(workspace_root, "inline");
    let result_dir_display = result_dir.display().to_string();
    std::fs::create_dir_all(&result_dir)?;
    let objective_file = result_dir.join("objective.md");
    std::fs::write(&objective_file, &task)?;

    let model = launch
        .model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("default model")
        .to_string();

    let (event_tx, event_rx) = std::sync::mpsc::sync_channel::<crate::quorp::tui::TuiEvent>(256);
    let workspace = workspace_root.to_path_buf();
    let options = crate::quorp::agent_runner::HeadlessRunOptions {
        workspace: workspace.clone(),
        objective_file: objective_file.clone(),
        model_id: model.clone(),
        base_url_override: launch.base_url.clone(),
        max_steps: 12,
        max_seconds: Some(3600),
        max_total_tokens: None,
        result_dir: result_dir.clone(),
        autonomy_profile,
        completion_policy: quorp_agent_core::CompletionPolicy::default(),
        objective_metadata: serde_json::json!({
            "origin": "inline",
            "run_mode": format!("{run_mode:?}"),
            "permission_mode": format!("{permission_mode:?}"),
            "sandbox": format!("{sandbox:?}"),
            "task": task.clone(),
        }),
        seed_context: Vec::new(),
    };

    let mut worker = Some(thread::spawn(move || {
        crate::quorp::agent_runner::run_headless_agent_with_progress(options, Some(event_tx))
    }));

    let mut command_state = quorp_render::CommandState::Active { frame_time: 0.0 };
    let mut output_buffer = VecDeque::<String>::new();
    let mut last_status = "starting inline agent run".to_string();
    let mut last_summary = "remote provider run initialized".to_string();
    let mut task_completed = false;
    let start_time = Instant::now();
    let mut last_render = Instant::now();
    let mut final_outcome: Option<quorp_agent_core::AgentRunOutcome> = None;
    let mut command_output_exit: Option<i32> = None;

    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, Hide, Clear(ClearType::All), MoveTo(0, 0));

    while !task_completed {
        let mut had_event = false;
        loop {
            match event_rx.recv_timeout(Duration::from_millis(75)) {
                Ok(crate::quorp::tui::TuiEvent::Chat(chat_event)) => {
                    had_event = true;
                    match chat_event {
                        crate::quorp::tui::ChatUiEvent::CommandOutput(_, line) => {
                            if !line.is_empty() {
                                if output_buffer.len() >= 5 {
                                    output_buffer.pop_front();
                                }
                                output_buffer.push_back(line);
                            }
                            if output_buffer.len() >= 3 {
                                last_summary = output_buffer
                                    .iter()
                                    .rev()
                                    .take(2)
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .into_iter()
                                    .rev()
                                    .collect::<Vec<_>>()
                                    .join(" · ");
                            }
                            last_status = "streaming command output".to_string();
                        }
                        crate::quorp::tui::ChatUiEvent::Error(_, message) => {
                            if !message.is_empty() {
                                if output_buffer.len() >= 5 {
                                    output_buffer.pop_front();
                                }
                                output_buffer.push_back(format!("error: {message}"));
                            }
                            last_status = "runtime error".to_string();
                        }
                        crate::quorp::tui::ChatUiEvent::CommandFinished(_, outcome) => {
                            match outcome {
                                quorp_agent_core::ActionOutcome::Success { action, .. } => {
                                    last_status = format!("completed: {:?}", action);
                                    command_state = quorp_render::CommandState::Passed {
                                        exit_code: 0,
                                        duration: format!("{:.2?}", start_time.elapsed()),
                                    };
                                    command_output_exit = Some(0);
                                }
                                quorp_agent_core::ActionOutcome::Failure { action, error } => {
                                    last_status = format!("failed: {:?} — {error}", action);
                                    command_state = quorp_render::CommandState::Failed {
                                        exit_code: 1,
                                        duration: format!("{:.2?}", start_time.elapsed()),
                                    };
                                    if output_buffer.len() >= 5 {
                                        output_buffer.pop_front();
                                    }
                                    output_buffer
                                        .push_back(format!("tool failure: {:?} · {error}", action));
                                    command_output_exit = Some(1);
                                }
                            }
                        }
                        crate::quorp::tui::ChatUiEvent::AssistantDelta(_, line) => {
                            if !line.trim().is_empty() {
                                last_status = format!("assistant: {line}");
                            }
                        }
                        crate::quorp::tui::ChatUiEvent::StreamFinished(_) => {
                            last_status = "assistant stream finished".to_string();
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        let worker_finished = worker
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished);
        if final_outcome.is_none() && worker_finished {
            let joined_worker = worker
                .take()
                .expect("worker handle should exist when finished");
            final_outcome = match joined_worker.join() {
                Ok(result) => Some(result?),
                Err(error) => {
                    let _ = execute!(stdout, Show);
                    return Err(anyhow::anyhow!("inline worker panicked: {:?}", error));
                }
            };
            while let Ok(crate::quorp::tui::TuiEvent::Chat(chat_event)) =
                event_rx.recv_timeout(Duration::from_millis(5))
            {
                match chat_event {
                    crate::quorp::tui::ChatUiEvent::CommandOutput(_, line) => {
                        if output_buffer.len() >= 5 {
                            output_buffer.pop_front();
                        }
                        output_buffer.push_back(line);
                    }
                    crate::quorp::tui::ChatUiEvent::Error(_, message) => {
                        if output_buffer.len() >= 5 {
                            output_buffer.pop_front();
                        }
                        output_buffer.push_back(format!("error: {message}"));
                    }
                    crate::quorp::tui::ChatUiEvent::CommandFinished(_, outcome) => match outcome {
                        quorp_agent_core::ActionOutcome::Success { action, .. } => {
                            command_state = quorp_render::CommandState::Passed {
                                exit_code: 0,
                                duration: format!("{:.2?}", start_time.elapsed()),
                            };
                            output_buffer.push_back(format!("completed: {:?}", action));
                            command_output_exit = Some(0);
                        }
                        quorp_agent_core::ActionOutcome::Failure { action, error } => {
                            command_state = quorp_render::CommandState::Failed {
                                exit_code: 1,
                                duration: format!("{:.2?}", start_time.elapsed()),
                            };
                            output_buffer.push_back(format!("failed: {:?}: {error}", action));
                            command_output_exit = Some(1);
                        }
                    },
                    _ => {}
                }
            }

            if !matches!(
                command_state,
                quorp_render::CommandState::Passed { .. }
                    | quorp_render::CommandState::Failed { .. }
            ) {
                command_state = quorp_render::CommandState::Failed {
                    exit_code: command_output_exit.unwrap_or(1),
                    duration: format!("{:.2?}", start_time.elapsed()),
                };
            }
            task_completed = true;
            had_event = true;
        }

        if had_event || last_render.elapsed() > Duration::from_millis(90) {
            let width = terminal_width.max(48);
            let output_summary = if output_buffer.is_empty() {
                last_summary.clone()
            } else {
                output_buffer
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            let frame = quorp_render::SessionFrame {
                title: "inline agent runtime".to_string(),
                subtitle: format!(
                    "task: {}",
                    truncate_for_frame(&task, width.saturating_sub(40))
                ),
                tasks: vec![
                    quorp_render::TaskRow {
                        label: format!("model: {model}"),
                        state: quorp_render::TaskState::Done,
                    },
                    quorp_render::TaskRow {
                        label: format!("status: {last_status}"),
                        state: if matches!(command_state, quorp_render::CommandState::Active { .. })
                        {
                            quorp_render::TaskState::Active
                        } else {
                            quorp_render::TaskState::Done
                        },
                    },
                ],
                commands: vec![quorp_render::CommandCard {
                    label: "agent run".to_string(),
                    command: format!("quorp exec --sandbox {sandbox:?}"),
                    cwd: workspace_root.display().to_string(),
                    state: match command_state {
                        quorp_render::CommandState::Active { .. } => {
                            quorp_render::CommandState::Active {
                                frame_time: start_time.elapsed().as_secs_f32(),
                            }
                        }
                        _ => command_state.clone(),
                    },
                    output_summary,
                }],
                footer: format!(
                    "model={model} · mode={mode_label}{color_plan_indicator} · cwd={}",
                    workspace_root.display()
                )
                .trim()
                .to_string(),
            };
            let rendered = quorp_render::render_session_frame(&frame, width, color);
            let _ = execute!(stdout, MoveTo(0, 0), Clear(ClearType::All));
            println!("{}", rendered);
            last_render = Instant::now();
        }
    }

    let _ = execute!(stdout, Show);
    let outcome = final_outcome.context("inline worker exited without outcome")?;
    if let Some(exit_code) = command_output_exit
        && exit_code == 0
    {
        println!(
            "{}",
            quorp_term::render_card(&quorp_term::TranscriptCard::Validation {
                label: "agent run".to_string(),
                status: quorp_term::ValidationStatus::Passed,
                frame: 0,
            })
        );
        println!(
            "{}",
            quorp_term::render_card(&quorp_term::TranscriptCard::ProofReceipt {
                path: format!("{result_dir_display}/metadata.json"),
                summary: format!(
                    "stop_reason={:?} · billed_tokens={} · runtime_ms={}",
                    outcome.stop_reason, outcome.total_billed_tokens, outcome.duration_ms
                ),
            })
        );
        return Ok(());
    }

    println!(
        "{}",
        quorp_term::render_card(&quorp_term::TranscriptCard::Validation {
            label: "agent run".to_string(),
            status: quorp_term::ValidationStatus::Failed,
            frame: 0,
        })
    );
    println!(
        "{}",
        quorp_term::render_card(&quorp_term::TranscriptCard::ProofReceipt {
            path: format!("{result_dir_display}/summary.json"),
            summary: format!(
                "stop_reason={:?} · error_message={:?} · total_billed_tokens={}",
                outcome.stop_reason, outcome.error_message, outcome.total_billed_tokens
            ),
        })
    );
    Err(anyhow::anyhow!(
        "inline run ended with non-success status: {:?}",
        outcome.stop_reason
    ))
}

fn truncate_for_frame(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        value.to_string()
    } else {
        let mut truncated = String::new();
        for ch in value.chars().take(max_len.saturating_sub(1)) {
            truncated.push(ch);
        }
        truncated.push('…');
        truncated
    }
}

fn run_fullscreen_cli(
    launch: SessionLaunchConfig,
    run_mode: quorp_core::RunMode,
    permission_mode: quorp_core::PermissionMode,
    sandbox: quorp_core::SandboxMode,
) -> anyhow::Result<()> {
    use std::io::{self, Write as _};

    let workspace_root = launch.workspace_root.clone();
    let _loaded = load_workspace_settings(&workspace_root)?;
    let profile = quorp_render::RenderProfile::detect_from_env();
    let mut shell = FullscreenShell::new(
        launch,
        workspace_root,
        profile,
        run_mode,
        permission_mode,
        sandbox,
    );

    let _terminal = FullscreenTerminalGuard::enter()?;

    if let Some(initial_prompt) = shell.launch.initial_prompt.clone()
        && !initial_prompt.trim().is_empty()
    {
        shell.start_prompt(initial_prompt)?;
    }

    loop {
        shell.drain_agent_events()?;
        shell.reap_finished_worker()?;

        let (width, height) = terminal_size().unwrap_or((120, 40));
        let render_output = shell.render(usize::from(width), usize::from(height));

        let mut stdout = io::stdout();
        execute!(stdout, MoveTo(0, 0), Clear(ClearType::All))?;
        for (row, line) in render_output.lines.iter().enumerate() {
            execute!(stdout, MoveTo(0, row as u16), Clear(ClearType::CurrentLine))?;
            write!(stdout, "{line}")?;
        }
        execute!(
            stdout,
            MoveTo(render_output.cursor_col, render_output.cursor_row)
        )?;
        stdout.flush()?;

        if shell.exit_requested {
            break;
        }

        if event::poll(Duration::from_millis(40))? {
            match event::read()? {
                Event::Key(key) => shell.handle_key(key)?,
                Event::Resize(_, _) => {}
                Event::Mouse(mouse_event) => shell.handle_mouse(mouse_event)?,
                _ => {}
            }
        }
    }

    Ok(())
}

struct FullscreenTerminalGuard;

impl FullscreenTerminalGuard {
    fn enter() -> anyhow::Result<Self> {
        let mut stdout = std::io::stdout();
        enable_raw_mode().context("failed to enable terminal raw mode")?;
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            Hide,
            Clear(ClearType::All),
            MoveTo(0, 0)
        )
        .context("failed to enter fullscreen terminal mode")?;
        Ok(Self)
    }
}

impl Drop for FullscreenTerminalGuard {
    fn drop(&mut self) {
        let mut stdout = std::io::stdout();
        let _ = execute!(
            stdout,
            Show,
            DisableMouseCapture,
            LeaveAlternateScreen,
            MoveTo(0, 0),
            Clear(ClearType::All)
        );
        if let Err(error) = crossterm::terminal::disable_raw_mode() {
            eprintln!("quorp: failed to restore terminal mode: {error}");
        }
    }
}

struct FullscreenShell {
    launch: SessionLaunchConfig,
    workspace_root: PathBuf,
    profile: quorp_render::RenderProfile,
    run_mode: quorp_core::RunMode,
    permission_mode: quorp_core::PermissionMode,
    sandbox: quorp_core::SandboxMode,
    registry: quorp_slash::Registry,
    composer: crate::quorp::inline_composer::ComposerState,
    transcript: VecDeque<quorp_render::TranscriptItem>,
    active_command_index: Option<usize>,
    running_worker: Option<RunningPromptSession>,
    queued_prompt: Option<String>,
    prompt_history: Vec<String>,
    history_cursor: Option<usize>,
    scroll_offset: usize,
    exit_requested: bool,
    model: String,
    provider_label: String,
    status_line: String,
    boot_started: Instant,
}

struct RunningPromptSession {
    worker: thread::JoinHandle<anyhow::Result<quorp_agent_core::AgentRunOutcome>>,
    event_rx: std::sync::mpsc::Receiver<crate::quorp::tui::TuiEvent>,
    start_time: Instant,
    command_output: VecDeque<String>,
}

struct ShellRenderOutput {
    lines: Vec<String>,
    cursor_row: u16,
    cursor_col: u16,
}

impl FullscreenShell {
    fn new(
        launch: SessionLaunchConfig,
        workspace_root: PathBuf,
        profile: quorp_render::RenderProfile,
        run_mode: quorp_core::RunMode,
        permission_mode: quorp_core::PermissionMode,
        sandbox: quorp_core::SandboxMode,
    ) -> Self {
        let model = launch
            .model
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("default remote model")
            .to_string();
        let provider_label = launch
            .provider
            .map(|provider| provider.label().to_string())
            .unwrap_or_else(|| "provider".to_string());
        let mut transcript = VecDeque::new();
        transcript.push_back(quorp_render::TranscriptItem::System {
            text: "type a task, or press / for commands".to_string(),
        });

        Self {
            launch,
            workspace_root,
            profile,
            run_mode,
            permission_mode,
            sandbox,
            registry: quorp_slash::Registry::new(),
            composer: crate::quorp::inline_composer::ComposerState::default(),
            transcript,
            active_command_index: None,
            running_worker: None,
            queued_prompt: None,
            prompt_history: Vec::new(),
            history_cursor: None,
            scroll_offset: 0,
            exit_requested: false,
            model,
            provider_label,
            status_line: "idle".to_string(),
            boot_started: Instant::now(),
        }
    }

    fn start_prompt(&mut self, prompt: String) -> anyhow::Result<()> {
        if prompt.trim().is_empty() {
            return Ok(());
        }
        if self.running_worker.is_some() {
            self.queued_prompt = Some(prompt.clone());
            self.status_line = "queued follow-up prompt".to_string();
            self.push_transcript(quorp_render::TranscriptItem::System {
                text: format!("queued follow-up: {}", truncate_for_frame(&prompt, 96)),
            });
            return Ok(());
        }
        self.prompt_history.push(prompt.clone());
        self.history_cursor = None;
        self.start_prompt_session(prompt)
    }

    fn start_prompt_session(&mut self, prompt: String) -> anyhow::Result<()> {
        let result_dir =
            crate::quorp::run_support::default_run_result_dir(&self.workspace_root, "fullscreen");
        std::fs::create_dir_all(&result_dir)?;
        let objective_file = result_dir.join("objective.md");
        std::fs::write(&objective_file, &prompt)?;
        let autonomy_profile = match self.run_mode {
            quorp_core::RunMode::Plan => quorp_agent_core::AutonomyProfile::Interactive,
            quorp_core::RunMode::Act => {
                if matches!(self.sandbox, quorp_core::SandboxMode::TmpCopy) {
                    quorp_agent_core::AutonomyProfile::AutonomousSandboxed
                } else {
                    quorp_agent_core::AutonomyProfile::AutonomousHost
                }
            }
        };
        let objective_metadata = serde_json::json!({
            "origin": "fullscreen",
            "run_mode": format!("{:?}", self.run_mode),
            "permission_mode": format!("{:?}", self.permission_mode),
            "sandbox": format!("{:?}", self.sandbox),
            "task": prompt.clone(),
        });
        let options = crate::quorp::agent_runner::HeadlessRunOptions {
            workspace: self.workspace_root.clone(),
            objective_file,
            model_id: self.model.clone(),
            base_url_override: self.launch.base_url.clone(),
            max_steps: 12,
            max_seconds: Some(3600),
            max_total_tokens: None,
            result_dir: result_dir.clone(),
            autonomy_profile,
            completion_policy: quorp_agent_core::CompletionPolicy::default(),
            objective_metadata,
            seed_context: Vec::new(),
        };
        let (event_tx, event_rx) =
            std::sync::mpsc::sync_channel::<crate::quorp::tui::TuiEvent>(256);
        let worker = thread::spawn(move || {
            crate::quorp::agent_runner::run_headless_agent_with_progress(options, Some(event_tx))
        });

        self.running_worker = Some(RunningPromptSession {
            worker,
            event_rx,
            start_time: Instant::now(),
            command_output: VecDeque::new(),
        });
        self.push_transcript(quorp_render::TranscriptItem::Thinking {
            label: "thinking".to_string(),
        });
        self.active_command_index = None;
        self.status_line = "running".to_string();
        Ok(())
    }

    fn submit_buffer(&mut self, input: String) -> anyhow::Result<()> {
        let input = input.trim().to_string();
        if input.is_empty() {
            return Ok(());
        }
        if let Some(command) = quorp_term::parse_slash_command(&input) {
            self.handle_slash_command(command)?;
            return Ok(());
        }
        self.push_transcript(quorp_render::TranscriptItem::User {
            text: input.clone(),
        });
        self.start_prompt(input)
    }

    fn handle_slash_command(&mut self, command: quorp_term::SlashCommand) -> anyhow::Result<()> {
        match command {
            quorp_term::SlashCommand::Plan
            | quorp_term::SlashCommand::Act
            | quorp_term::SlashCommand::Auto
            | quorp_term::SlashCommand::Manual
            | quorp_term::SlashCommand::FullAuto
            | quorp_term::SlashCommand::FullPermissions
            | quorp_term::SlashCommand::Permissions(_)
            | quorp_term::SlashCommand::Sandbox(_) => {
                quorp_term::apply_mode_command(
                    &command,
                    &mut self.run_mode,
                    &mut self.permission_mode,
                    &mut self.sandbox,
                );
                self.status_line = format!(
                    "mode updated · run={:?} permissions={:?} sandbox={:?}",
                    self.run_mode, self.permission_mode, self.sandbox
                );
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: self.status_line.clone(),
                });
            }
            quorp_term::SlashCommand::Clear => {
                self.transcript.clear();
                self.active_command_index = None;
                self.scroll_offset = 0;
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: "cleared transcript".to_string(),
                });
            }
            quorp_term::SlashCommand::Status => {
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: format!(
                        "status · model={} · cwd={} · run={:?} · permissions={:?} · sandbox={:?}",
                        self.model,
                        self.workspace_root.display(),
                        self.run_mode,
                        self.permission_mode,
                        self.sandbox
                    ),
                });
            }
            quorp_term::SlashCommand::Model(Some(model)) => {
                self.model = model;
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: format!("model switched to {}", self.model),
                });
            }
            quorp_term::SlashCommand::Model(None) => {
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: format!("model {}", self.model),
                });
            }
            quorp_term::SlashCommand::Provider(Some(provider)) => {
                self.provider_label = provider;
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: format!("provider switched to {}", self.provider_label),
                });
            }
            quorp_term::SlashCommand::Provider(None) => {
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: format!("provider {}", self.provider_label),
                });
            }
            quorp_term::SlashCommand::Help | quorp_term::SlashCommand::Unknown(_) => {
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: "try /plan, /act, /full-auto, /permissions, /sandbox, /status, /clear"
                        .to_string(),
                });
            }
            quorp_term::SlashCommand::Doctor => {
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: "run `quorp doctor` from a regular shell for full diagnostics"
                        .to_string(),
                });
            }
            quorp_term::SlashCommand::Tasks
            | quorp_term::SlashCommand::Checkpoint
            | quorp_term::SlashCommand::Rollback
            | quorp_term::SlashCommand::Theme
            | quorp_term::SlashCommand::Memory
            | quorp_term::SlashCommand::Rules
            | quorp_term::SlashCommand::Session(_)
            | quorp_term::SlashCommand::Init
            | quorp_term::SlashCommand::Edit(_)
            | quorp_term::SlashCommand::Undo
            | quorp_term::SlashCommand::Redo
            | quorp_term::SlashCommand::Files
            | quorp_term::SlashCommand::Hooks
            | quorp_term::SlashCommand::Mcp
            | quorp_term::SlashCommand::Diff
            | quorp_term::SlashCommand::Apply
            | quorp_term::SlashCommand::Revert
            | quorp_term::SlashCommand::Test
            | quorp_term::SlashCommand::Verify
            | quorp_term::SlashCommand::Save
            | quorp_term::SlashCommand::Load(_)
            | quorp_term::SlashCommand::Think
            | quorp_term::SlashCommand::Compact => {
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: "that command is not available in the fullscreen shell yet".to_string(),
                });
            }
        }
        self.history_cursor = None;
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> anyhow::Result<()> {
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.exit_requested = true;
                return Ok(());
            }
            (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                self.transcript.clear();
                self.active_command_index = None;
                self.scroll_offset = 0;
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: "cleared screen".to_string(),
                });
                return Ok(());
            }
            (KeyCode::PageUp, _) => {
                self.scroll_offset = self.scroll_offset.saturating_add(4);
                return Ok(());
            }
            (KeyCode::PageDown, _) => {
                self.scroll_offset = self.scroll_offset.saturating_sub(4);
                return Ok(());
            }
            (KeyCode::Home, _) => {
                self.scroll_offset = usize::MAX / 4;
                return Ok(());
            }
            (KeyCode::End, _) => {
                self.scroll_offset = 0;
                return Ok(());
            }
            (KeyCode::Up, _) | (KeyCode::Down, _) => {
                let command_palette_visible =
                    self.composer.suggestions_visible() && self.composer.buffer().starts_with('/');
                if !command_palette_visible && self.handle_history_navigation(key.code) {
                    return Ok(());
                }
            }
            _ => {}
        }

        match self.composer.handle_key(key, &self.registry) {
            crate::quorp::inline_composer::ComposerAction::Continue => {}
            crate::quorp::inline_composer::ComposerAction::Cancel => {
                if self.running_worker.is_some() {
                    self.exit_requested = true;
                }
            }
            crate::quorp::inline_composer::ComposerAction::Submit(input) => {
                self.composer.clear();
                self.submit_buffer(input)?;
            }
        }
        Ok(())
    }

    fn handle_mouse(&mut self, mouse_event: crossterm::event::MouseEvent) -> anyhow::Result<()> {
        match mouse_event.kind {
            crossterm::event::MouseEventKind::ScrollUp => {
                self.scroll_offset = self.scroll_offset.saturating_add(2);
            }
            crossterm::event::MouseEventKind::ScrollDown => {
                self.scroll_offset = self.scroll_offset.saturating_sub(2);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_history_navigation(&mut self, key_code: KeyCode) -> bool {
        let current_buffer_is_empty = self.composer.buffer().trim().is_empty();
        if self.prompt_history.is_empty() {
            return false;
        }
        match key_code {
            KeyCode::Up => {
                let next_index = match self.history_cursor {
                    Some(index) if index > 0 => index - 1,
                    Some(_) | None => self.prompt_history.len().saturating_sub(1),
                };
                self.history_cursor = Some(next_index);
                if let Some(value) = self.prompt_history.get(next_index) {
                    let mut composer =
                        crate::quorp::inline_composer::ComposerState::with_buffer(value);
                    composer.set_suggestions_visible(true);
                    self.composer = composer;
                }
                true
            }
            KeyCode::Down => {
                let Some(index) = self.history_cursor else {
                    return false;
                };
                if index + 1 >= self.prompt_history.len() {
                    self.history_cursor = None;
                    self.composer.clear();
                } else if let Some(value) = self.prompt_history.get(index + 1) {
                    self.history_cursor = Some(index + 1);
                    self.composer =
                        crate::quorp::inline_composer::ComposerState::with_buffer(value);
                }
                true
            }
            _ => current_buffer_is_empty,
        }
    }

    fn drain_agent_events(&mut self) -> anyhow::Result<()> {
        loop {
            let event = {
                let Some(running_worker) = self.running_worker.as_mut() else {
                    return Ok(());
                };
                running_worker.event_rx.try_recv()
            };
            match event {
                Ok(crate::quorp::tui::TuiEvent::Chat(chat_event)) => match chat_event {
                    crate::quorp::tui::ChatUiEvent::CommandOutput(_, line) => {
                        if !line.trim().is_empty() {
                            let summary = {
                                let Some(running_worker) = self.running_worker.as_mut() else {
                                    return Ok(());
                                };
                                running_worker.command_output.push_back(line.clone());
                                if running_worker.command_output.len() > 4 {
                                    running_worker.command_output.pop_front();
                                }
                                running_worker
                                    .command_output
                                    .iter()
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join(" · ")
                            };
                            self.update_command_tail("tool output".to_string(), summary);
                        }
                        self.status_line = "streaming command output".to_string();
                    }
                    crate::quorp::tui::ChatUiEvent::Error(_, message) => {
                        if !message.trim().is_empty() {
                            self.push_transcript(quorp_render::TranscriptItem::Error {
                                title: "runtime error".to_string(),
                                detail: truncate_for_frame(&message, 160),
                            });
                        }
                        self.status_line = "runtime error".to_string();
                    }
                    crate::quorp::tui::ChatUiEvent::CommandFinished(_, outcome) => match outcome {
                        quorp_agent_core::ActionOutcome::Success { action, .. } => {
                            self.status_line = format!("completed {:?}", action);
                            let summary = self
                                .running_worker
                                .as_ref()
                                .map(|running_worker| {
                                    running_worker
                                        .command_output
                                        .iter()
                                        .cloned()
                                        .chain(std::iter::once("success".to_string()))
                                        .collect::<Vec<_>>()
                                        .join(" · ")
                                })
                                .unwrap_or_else(|| "success".to_string());
                            self.finish_active_command(quorp_render::ToolStatus::Passed, summary);
                        }
                        quorp_agent_core::ActionOutcome::Failure { action, error } => {
                            self.status_line = format!("failed {:?} · {}", action, error);
                            self.finish_active_command(
                                quorp_render::ToolStatus::Failed,
                                format!("{action:?} · {error}"),
                            );
                        }
                    },
                    crate::quorp::tui::ChatUiEvent::AssistantDelta(_, line) => {
                        if !line.trim().is_empty() {
                            self.append_assistant_delta(&line);
                            self.status_line =
                                format!("assistant: {}", truncate_for_frame(&line, 96));
                        }
                    }
                    crate::quorp::tui::ChatUiEvent::StreamFinished(_) => {
                        self.status_line = "assistant stream finished".to_string();
                    }
                },
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        Ok(())
    }

    fn reap_finished_worker(&mut self) -> anyhow::Result<()> {
        let Some(running_worker) = self.running_worker.as_ref() else {
            return Ok(());
        };
        if !running_worker.worker.is_finished() {
            return Ok(());
        }
        let running_worker = self
            .running_worker
            .take()
            .expect("worker should exist when finished");
        let outcome = match running_worker.worker.join() {
            Ok(result) => result?,
            Err(error) => {
                self.push_transcript(quorp_render::TranscriptItem::Error {
                    title: "worker panicked".to_string(),
                    detail: format!("{error:?}"),
                });
                self.status_line = "worker panicked".to_string();
                return Ok(());
            }
        };
        self.status_line = format!(
            "run finished · {:?} · {} tokens",
            outcome.stop_reason, outcome.total_billed_tokens
        );
        self.push_transcript(quorp_render::TranscriptItem::Receipt {
            text: format!(
                "run finished · {:?} · {} tokens",
                outcome.stop_reason, outcome.total_billed_tokens
            ),
            success: outcome.stop_reason == quorp_agent_core::StopReason::Success,
        });
        if let Some(queued_prompt) = self.queued_prompt.take()
            && !queued_prompt.trim().is_empty()
        {
            self.start_prompt(queued_prompt)?;
        }
        Ok(())
    }

    fn render(&mut self, width: usize, height: usize) -> ShellRenderOutput {
        let buffer = self.composer.buffer().to_string();
        let overlay =
            if self.composer.suggestions_visible() && self.composer.buffer().starts_with('/') {
                Some(quorp_render::ShellOverlay::SlashPalette {
                    selected: self.composer.selected(),
                    entries: self
                        .composer
                        .suggestions(&self.registry)
                        .into_iter()
                        .map(|entry| quorp_render::shell::PaletteRow {
                            value: entry.value,
                            detail: entry.detail,
                            description: entry.description,
                        })
                        .collect(),
                })
            } else {
                None
            };
        let live_turn = self
            .running_worker
            .as_ref()
            .map(|running_worker| quorp_render::LiveTurn {
                label: "working".to_string(),
                elapsed_ms: running_worker.start_time.elapsed().as_millis() as u64,
            });
        let status = quorp_render::StatusLine {
            left: format!("{} · {}", self.model, self.provider_label),
            center: format!("{:?} · {:?}", self.permission_mode, self.sandbox),
            right: if matches!(self.run_mode, quorp_core::RunMode::Plan) {
                "Plan mode".to_string()
            } else {
                format!("{} · {}", self.workspace_root.display(), self.status_line)
            },
        };
        let composer = quorp_render::ComposerView {
            prompt: ">".to_string(),
            buffer: buffer.clone(),
            blink_on: self.boot_started.elapsed().as_millis() % 1000 < 500,
        };
        let frame = quorp_render::ShellFrame {
            transcript: self.transcript.iter().cloned().collect(),
            live_turn,
            composer,
            status,
            overlay,
        };
        let mut body = Vec::new();
        let show_boot = self.boot_started.elapsed() < Duration::from_millis(1200)
            || (self.transcript.len() <= 1 && self.running_worker.is_none());
        if show_boot {
            body.extend(
                quorp_render::logo::render_boot_card(
                    &self.workspace_root.display().to_string(),
                    &self.model,
                    &format!("{:?}", self.sandbox),
                    self.profile,
                )
                .lines()
                .map(|line| line.to_string()),
            );
            body.push(String::new());
        }
        body.extend(quorp_render::render_shell_frame(
            &frame,
            width,
            self.profile.color,
        ));
        let suggestions =
            quorp_render::shell::render_shell_overlay(&frame.overlay, width, self.profile.color);
        let footer =
            quorp_render::shell::render_status_line(&frame.status, width, self.profile.color);
        let footer_height = 1usize;
        let prompt_height = 1usize;
        let suggestion_height = suggestions.len().min(8).min(height.saturating_sub(2));
        let body_height = height.saturating_sub(footer_height + prompt_height + suggestion_height);
        let visible_body = if self.scroll_offset == 0 || body.len() <= body_height {
            body.split_off(body.len().saturating_sub(body_height))
        } else {
            let max_scroll = body.len().saturating_sub(body_height);
            let scroll_offset = self.scroll_offset.min(max_scroll);
            let end = body.len().saturating_sub(scroll_offset);
            let start = end.saturating_sub(body_height);
            body[start..end].to_vec()
        };

        let mut lines = Vec::new();
        if visible_body.len() < body_height {
            lines.extend(visible_body);
            while lines.len() < body_height {
                lines.push(String::new());
            }
        } else {
            lines.extend(visible_body);
        }
        lines.extend(suggestions);
        let cursor_row = lines.len() as u16;
        let cursor_col = (2usize + buffer_width_to_cursor(&buffer, self.composer.cursor()))
            .min(u16::MAX as usize) as u16;
        lines.push(quorp_render::shell::render_composer(
            &frame.composer,
            self.profile.color,
        ));
        lines.push(footer);

        ShellRenderOutput {
            lines,
            cursor_row,
            cursor_col,
        }
    }

    fn push_transcript(&mut self, item: quorp_render::TranscriptItem) {
        if self.transcript.len() >= 300 {
            self.transcript.pop_front();
        }
        self.transcript.push_back(item);
        if self.scroll_offset == 0 {
            self.scroll_offset = 0;
        }
    }

    fn append_assistant_delta(&mut self, delta: &str) {
        if let Some(quorp_render::TranscriptItem::Assistant { text, streaming }) =
            self.transcript.back_mut()
        {
            if !text.ends_with('\n') && !text.is_empty() {
                text.push(' ');
            }
            text.push_str(delta.trim());
            *streaming = true;
            return;
        }
        self.push_transcript(quorp_render::TranscriptItem::Assistant {
            text: delta.trim().to_string(),
            streaming: true,
        });
    }

    fn update_command_tail(&mut self, command: String, summary: String) {
        let output_tail = summary
            .split(" · ")
            .filter(|line| !line.trim().is_empty())
            .map(|line| truncate_for_frame(line, 120))
            .collect::<Vec<_>>();
        if let Some(index) = self.active_command_index
            && let Some(quorp_render::TranscriptItem::Command {
                output_tail: current_tail,
                status,
                ..
            }) = self.transcript.get_mut(index)
        {
            *current_tail = output_tail;
            *status = quorp_render::ToolStatus::Running;
            return;
        }
        let index = self.transcript.len();
        self.push_transcript(quorp_render::TranscriptItem::Command {
            command,
            cwd: self.workspace_root.display().to_string(),
            output_tail,
            status: quorp_render::ToolStatus::Running,
        });
        self.active_command_index = Some(index);
    }

    fn finish_active_command(&mut self, status: quorp_render::ToolStatus, summary: String) {
        let output_tail = summary
            .split(" · ")
            .filter(|line| !line.trim().is_empty())
            .map(|line| truncate_for_frame(line, 120))
            .collect::<Vec<_>>();
        if let Some(index) = self.active_command_index
            && let Some(quorp_render::TranscriptItem::Command {
                output_tail: current_tail,
                status: current_status,
                ..
            }) = self.transcript.get_mut(index)
        {
            *current_tail = output_tail;
            *current_status = status;
            self.active_command_index = None;
            return;
        }
        self.push_transcript(quorp_render::TranscriptItem::Command {
            command: "tool".to_string(),
            cwd: self.workspace_root.display().to_string(),
            output_tail,
            status,
        });
    }
}

fn buffer_width_to_cursor(buffer: &str, cursor: usize) -> usize {
    buffer[..cursor]
        .chars()
        .map(|value| value.width().unwrap_or(0))
        .sum()
}

fn print_inline_help() {
    println!(
        "{}",
        quorp_term::render_card(&quorp_term::TranscriptCard::Plan {
            title: "slash commands".to_string(),
            steps: vec![
                "/plan, /act, /full-auto, /full-permissions".to_string(),
                "/permissions <mode>, /sandbox <host|tmp-copy>".to_string(),
                "/hooks, /mcp, /diff, /apply, /revert, /compact, /doctor, /help".to_string(),
                "/exit or /quit".to_string(),
            ],
        })
    );
}

fn apply_session_env_overrides(launch: &SessionLaunchConfig) {
    if let Some(provider) = launch.provider {
        unsafe {
            std::env::set_var("QUORP_PROVIDER", provider.label());
        }
    }
    if let Some(model) = launch.model.as_deref() {
        unsafe {
            std::env::set_var("QUORP_MODEL", model);
        }
    }
    match (launch.provider, launch.base_url.as_deref()) {
        (Some(crate::quorp::executor::InteractiveProviderKind::Nvidia), Some(base_url)) => unsafe {
            std::env::set_var("QUORP_NVIDIA_BASE_URL", base_url);
            std::env::remove_var("QUORP_BASE_URL");
            std::env::remove_var("QUORP_CHAT_BASE_URL");
        },
        _ => unsafe {
            std::env::remove_var("QUORP_BASE_URL");
            std::env::remove_var("QUORP_CHAT_BASE_URL");
            std::env::remove_var("QUORP_NVIDIA_BASE_URL");
        },
    }
    match launch.prompt_compaction_policy {
        Some(policy) => unsafe {
            std::env::set_var("QUORP_PROMPT_COMPACTION_POLICY", policy.as_str());
        },
        None => unsafe {
            std::env::remove_var("QUORP_PROMPT_COMPACTION_POLICY");
        },
    }
}

fn run_mem_analyze() -> anyhow::Result<()> {
    let path = paths::memory_log_file();
    let summary = crate::quorp::memory_fingerprint::analyze_current_memory_log()
        .with_context(|| format!("resolved memory log path: {}", path.display()))?;
    println!(
        "{}",
        crate::quorp::memory_fingerprint::format_memory_summary(path, &summary,)
    );
    Ok(())
}

fn run_mem_log_path() -> anyhow::Result<()> {
    println!("{}", paths::memory_log_file().display());
    Ok(())
}

fn default_workspace_root() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| paths::home_dir().clone())
}

fn default_benchmark_briefing_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/src/development/quorp-tui-leaning-plan.md")
}

fn initial_workspace_root(paths_or_urls: &[String]) -> PathBuf {
    let fallback = || std::env::current_dir().unwrap_or_else(|_| paths::home_dir().clone());
    let Some(first) = paths_or_urls.first() else {
        return fallback();
    };
    if first.contains("://") {
        return fallback();
    }
    let parsed = PathWithPosition::parse_str(first);
    let path = parsed.path;
    if path.as_os_str().is_empty() {
        return fallback();
    }
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.is_dir() => std::fs::canonicalize(&path).unwrap_or(path),
        Ok(metadata) if metadata.is_file() => path
            .parent()
            .map(|parent| {
                if parent.as_os_str().is_empty() {
                    fallback()
                } else {
                    std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf())
                }
            })
            .unwrap_or_else(fallback),
        _ => path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf()))
            .unwrap_or_else(fallback),
    }
}

#[derive(Debug, Clone)]
struct SessionLaunchConfig {
    workspace_root: PathBuf,
    provider: Option<crate::quorp::executor::InteractiveProviderKind>,
    model: Option<String>,
    base_url: Option<String>,
    prompt_compaction_policy: Option<quorp_agent_core::PromptCompactionPolicy>,
    tui_mode: CliTuiMode,
    initial_prompt: Option<String>,
}

impl SessionLaunchConfig {
    fn from_workspace(
        workspace: PathBuf,
        tui_mode: CliTuiMode,
        provider: Option<crate::quorp::executor::InteractiveProviderKind>,
        model: Option<String>,
        base_url: Option<String>,
        prompt_compaction_policy: Option<quorp_agent_core::PromptCompactionPolicy>,
    ) -> Self {
        Self {
            workspace_root: initial_workspace_root(&[workspace.display().to_string()]),
            provider,
            model: model.filter(|value| !value.trim().is_empty()),
            base_url: base_url.filter(|value| !value.trim().is_empty()),
            prompt_compaction_policy,
            tui_mode,
            initial_prompt: None,
        }
    }

    fn from_paths_or_urls(
        paths_or_urls: Vec<String>,
        tui_mode: CliTuiMode,
        prompt_compaction_policy: Option<quorp_agent_core::PromptCompactionPolicy>,
    ) -> Self {
        let workspace_root = initial_workspace_root(&paths_or_urls);
        let initial_prompt = inline_prompt_from_args(&paths_or_urls, &workspace_root);
        let provider = default_provider_for_workspace(&workspace_root).ok();
        let model = provider
            .and_then(|provider| default_model_for_workspace(&workspace_root, provider).ok());
        let base_url = provider
            .and_then(|provider| default_base_url_for_workspace(&workspace_root, provider).ok())
            .flatten();
        Self {
            workspace_root,
            provider,
            model,
            base_url,
            prompt_compaction_policy,
            tui_mode,
            initial_prompt,
        }
    }
}

fn inline_prompt_from_args(args: &[String], workspace_root: &Path) -> Option<String> {
    if args.is_empty() {
        return None;
    }
    if args.len() == 1 {
        let candidate = Path::new(&args[0]);
        if candidate.exists() {
            return None;
        }
    }
    let prompt = args.join(" ");
    (!prompt.trim().is_empty() && workspace_root.exists()).then_some(prompt)
}

#[derive(Parser, Debug)]
#[command(name = "quorp", version = env!("CARGO_PKG_VERSION"))]
struct CliArgs {
    #[arg(long, value_enum, default_value_t = CliTuiMode::Auto)]
    tui: CliTuiMode,
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(long)]
    prompt_compaction_policy: Option<String>,
    paths_or_urls: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    Doctor,
    Exec(ExecArgs),
    MemAnalyze,
    MemLogPath,
    Session(SessionArgs),
    Run(RunCliArgs),
    Diagnostics {
        #[command(subcommand)]
        command: DiagnosticsCommand,
    },
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    Benchmark {
        #[command(subcommand)]
        command: BenchmarkCommand,
    },
    /// Render a brief animated demo of the brilliant-CLI primitives
    /// (splash checklist, oscillating shimmer, status footer,
    /// transcript lines, permission modal). Useful while wiring
    /// `quorp_render` into the inline CLI.
    RenderDemo,
    /// Print every slash command Quorp knows about. Drawn from the
    /// `quorp_slash::Registry` so the source of truth is one place.
    Commands {
        /// Optional fuzzy-match prefix; ranks commands by subsequence
        /// score and only prints matches.
        #[arg(value_name = "PREFIX")]
        prefix: Option<String>,
    },
    /// Walk the workspace via `quorp_repo_scan`, group files by
    /// language, and print a splash-style summary.
    Scan {
        /// Workspace root. Defaults to the current directory.
        #[arg(long, value_name = "PATH")]
        workspace: Option<PathBuf>,
        /// Also harvest top-level Rust symbols and report the count.
        #[arg(long)]
        symbols: bool,
    },
    /// Exercise `quorp_permissions::Permissions::check` against a
    /// proposed tool action. Useful for previewing the approval modal
    /// or testing allowlist patterns before committing to a policy.
    Permissions {
        /// Permission mode to evaluate against.
        #[arg(long, value_enum, default_value_t = CliPermissionMode::Ask)]
        mode: CliPermissionMode,
        /// Tool name (e.g. `read_file`, `run_command`, `write_file`).
        #[arg(long)]
        tool: String,
        /// Capability the tool wants. Defaults inferred from the tool
        /// name when possible.
        #[arg(long, value_enum)]
        capability: Option<CliCapability>,
        /// Rendered command string used for command-allowlist matching.
        #[arg(long)]
        command: Option<String>,
        /// Glob pattern to add to the command allowlist before checking.
        /// Useful for "preview what happens once I add this allow".
        #[arg(long, value_name = "GLOB")]
        allow_command: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliSandboxMode {
    Host,
    TmpCopy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliTuiMode {
    Auto,
    Fullscreen,
    Scrollback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliPermissionMode {
    ReadOnly,
    Ask,
    AcceptEdits,
    AutoSafe,
    YoloSandbox,
}

impl From<CliPermissionMode> for quorp_permissions::Mode {
    fn from(value: CliPermissionMode) -> Self {
        match value {
            CliPermissionMode::ReadOnly => quorp_permissions::Mode::ReadOnly,
            CliPermissionMode::Ask => quorp_permissions::Mode::Ask,
            CliPermissionMode::AcceptEdits => quorp_permissions::Mode::AcceptEdits,
            CliPermissionMode::AutoSafe => quorp_permissions::Mode::AutoSafe,
            CliPermissionMode::YoloSandbox => quorp_permissions::Mode::YoloSandbox,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliCapability {
    Read,
    WriteFile,
    DeleteFile,
    RunCommand,
    Network,
    Mcp,
}

impl From<CliCapability> for quorp_permissions::Capability {
    fn from(value: CliCapability) -> Self {
        match value {
            CliCapability::Read => quorp_permissions::Capability::Read,
            CliCapability::WriteFile => quorp_permissions::Capability::WriteFile,
            CliCapability::DeleteFile => quorp_permissions::Capability::DeleteFile,
            CliCapability::RunCommand => quorp_permissions::Capability::RunCommand,
            CliCapability::Network => quorp_permissions::Capability::Network,
            CliCapability::Mcp => quorp_permissions::Capability::Mcp,
        }
    }
}

impl From<CliSandboxMode> for quorp_core::SandboxMode {
    fn from(value: CliSandboxMode) -> Self {
        match value {
            CliSandboxMode::Host => Self::Host,
            CliSandboxMode::TmpCopy => Self::TmpCopy,
        }
    }
}

#[derive(ClapArgs, Debug)]
pub struct ExecArgs {
    task: String,
    #[arg(long)]
    workspace: Option<PathBuf>,
    #[arg(long)]
    result_dir: Option<PathBuf>,
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long, default_value_t = 12)]
    max_steps: usize,
    #[arg(long, default_value_t = 3600)]
    max_seconds: u64,
    #[arg(long)]
    max_total_tokens: Option<u64>,
    #[arg(long, value_enum)]
    sandbox: Option<CliSandboxMode>,
    #[arg(long, default_value_t = true)]
    keep_sandbox: bool,
    #[arg(long, default_value = "autonomous_host")]
    autonomy_profile: String,
    #[arg(long, default_value_t = false)]
    yolo: bool,
}

#[derive(ClapArgs, Debug)]
pub struct SessionArgs {
    #[arg(long)]
    workspace: Option<PathBuf>,
    #[arg(long)]
    prompt_compaction_policy: Option<String>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
pub enum AgentCommand {
    Run(AgentRunArgs),
    Resume(AgentResumeArgs),
}

#[derive(Subcommand, Debug)]
pub enum RunSubcommand {
    Resume(RunResumeArgs),
}

#[derive(Subcommand, Debug)]
pub enum DiagnosticsCommand {
    Path,
    Bundle(DiagnosticsBundleArgs),
    Summarize(DiagnosticsSummarizeArgs),
}

#[derive(Subcommand, Debug)]
pub enum BenchmarkCommand {
    Run(BenchmarkRunArgs),
    Prompt(BenchmarkPromptArgs),
    Resume(BenchmarkResumeArgs),
    Score(BenchmarkScoreArgs),
    Batch(BenchmarkBatchArgs),
}

#[derive(ClapArgs, Debug)]
pub struct AgentRunArgs {
    #[arg(long)]
    workspace: PathBuf,
    #[arg(long, default_value = "README.md")]
    objective_file: String,
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long, default_value_t = 12)]
    max_steps: usize,
    #[arg(long, default_value_t = 3600)]
    max_seconds: u64,
    #[arg(long)]
    max_total_tokens: Option<u64>,
    #[arg(long)]
    result_dir: PathBuf,
    #[arg(long, default_value = "autonomous_host")]
    autonomy_profile: String,
}

#[derive(ClapArgs, Debug)]
pub struct AgentResumeArgs {
    #[arg(long)]
    result_dir: PathBuf,
}

#[derive(ClapArgs, Debug)]
pub struct RunArgs {
    #[arg(long)]
    workspace: Option<PathBuf>,
    #[arg(long)]
    condition: Option<String>,
    #[arg(long)]
    objective_file: Option<PathBuf>,
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long, default_value_t = 12)]
    max_steps: usize,
    #[arg(long, default_value_t = 3600)]
    max_seconds: u64,
    #[arg(long, default_value_t = 2)]
    max_retries: usize,
    #[arg(long)]
    max_total_tokens: Option<u64>,
    #[arg(long)]
    result_dir: Option<PathBuf>,
    #[arg(long, value_enum)]
    sandbox: Option<CliSandboxMode>,
    #[arg(long, default_value_t = false)]
    keep_sandbox: bool,
    #[arg(long, default_value = "autonomous_host")]
    autonomy_profile: String,
    #[arg(long, default_value_t = false)]
    yolo: bool,
}

#[derive(ClapArgs, Debug)]
pub struct RunCliArgs {
    #[command(subcommand)]
    command: Option<RunSubcommand>,
    #[command(flatten)]
    start: RunArgs,
}

#[derive(ClapArgs, Debug)]
pub struct RunResumeArgs {
    #[arg(long)]
    result_dir: PathBuf,
}

#[derive(ClapArgs, Debug)]
pub struct DiagnosticsBundleArgs {
    #[arg(long)]
    run: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    latest: bool,
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(ClapArgs, Debug)]
pub struct DiagnosticsSummarizeArgs {
    #[arg(long)]
    run: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    latest: bool,
}

#[derive(ClapArgs, Debug)]
pub struct BenchmarkRunArgs {
    #[arg(long)]
    path: PathBuf,
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long, default_value_os_t = default_benchmark_briefing_file())]
    briefing_file: PathBuf,
    #[arg(long)]
    compaction_policy: Option<String>,
    #[arg(long)]
    seed_transcript: Option<PathBuf>,
    #[arg(long, default_value_t = 12)]
    max_steps: usize,
    #[arg(long, default_value_t = 3600)]
    max_seconds: u64,
    #[arg(long = "token-budget", alias = "max-total-tokens")]
    token_budget: Option<u64>,
    #[arg(long)]
    max_attempts: Option<usize>,
    #[arg(long)]
    result_dir: Option<PathBuf>,
    #[arg(long, default_value = "autonomous_host")]
    autonomy_profile: String,
    #[arg(long)]
    condition: Option<String>,
    #[arg(long, default_value_t = false)]
    keep_sandbox: bool,
    #[arg(long, value_enum, default_value = "tmp-copy")]
    sandbox: CliSandboxMode,
    #[arg(long)]
    log_file: Option<PathBuf>,
}

#[derive(ClapArgs, Debug)]
pub struct BenchmarkResumeArgs {
    #[arg(long)]
    result_dir: PathBuf,
}

#[derive(ClapArgs, Debug)]
pub struct BenchmarkScoreArgs {
    #[arg(long = "run-dir")]
    run_dirs: Vec<PathBuf>,
    #[arg(long, default_value = "rust-swebench-top5")]
    suite: String,
    #[arg(long, default_value = "/Volumes/MOE/models/reports")]
    reports_root: PathBuf,
    #[arg(long)]
    output_root: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    fail_on_regression: bool,
}

#[derive(ClapArgs, Debug)]
pub struct BenchmarkPromptArgs {
    #[arg(long)]
    path: PathBuf,
    #[arg(long)]
    workspace_dir: PathBuf,
    #[arg(long, default_value_os_t = default_benchmark_briefing_file())]
    briefing_file: PathBuf,
    #[arg(long)]
    compaction_policy: Option<String>,
    #[arg(long)]
    seed_transcript: Option<PathBuf>,
    #[arg(long, default_value_t = 100)]
    max_steps: usize,
    #[arg(long, default_value_t = 3600)]
    max_seconds: u64,
    #[arg(long = "token-budget", alias = "max-total-tokens")]
    token_budget: Option<u64>,
}

#[derive(ClapArgs, Debug)]
pub struct BenchmarkBatchArgs {
    #[arg(long)]
    cases_root: PathBuf,
    #[arg(long)]
    result_dir: Option<PathBuf>,
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long, default_value_os_t = default_benchmark_briefing_file())]
    briefing_file: PathBuf,
    #[arg(long)]
    compaction_policy: Option<String>,
    #[arg(long)]
    seed_transcript: Option<PathBuf>,
    #[arg(long, default_value_t = 100)]
    max_steps: usize,
    #[arg(long, default_value_t = 3600)]
    max_seconds: u64,
    #[arg(long = "token-budget", alias = "max-total-tokens")]
    token_budget: Option<u64>,
    #[arg(long)]
    max_attempts: Option<usize>,
    #[arg(long, default_value = "autonomous_host")]
    autonomy_profile: String,
    #[arg(long)]
    condition: Option<String>,
    #[arg(long, default_value_t = false)]
    keep_sandbox: bool,
    #[arg(long, value_enum, default_value = "tmp-copy")]
    sandbox: CliSandboxMode,
    #[arg(long)]
    log_dir: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorthand_and_session_workspace_resolution_match() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let challenge = temp_dir.path().join("04-entitlement-recovery-replay");
        std::fs::create_dir_all(&challenge).expect("challenge dir");

        let shorthand = SessionLaunchConfig::from_paths_or_urls(
            vec![challenge.display().to_string()],
            CliTuiMode::Auto,
            None,
        );
        let explicit = SessionLaunchConfig::from_workspace(
            challenge,
            CliTuiMode::Auto,
            None,
            None,
            None,
            None,
        );

        assert_eq!(shorthand.workspace_root, explicit.workspace_root);
    }

    #[test]
    fn benchmark_briefing_file_defaults_to_public_note() {
        let run_args = CliArgs::parse_from([
            "quorp",
            "benchmark",
            "run",
            "--path",
            "benchmark/exhaustive/issues/ISSUE-00-toy-preview",
        ]);

        let prompt_args = CliArgs::parse_from([
            "quorp",
            "benchmark",
            "prompt",
            "--path",
            "benchmark/exhaustive/issues/ISSUE-00-toy-preview",
            "--workspace-dir",
            "/tmp/quorp-workspace",
        ]);

        let batch_args = CliArgs::parse_from([
            "quorp",
            "benchmark",
            "batch",
            "--cases-root",
            "benchmark/exhaustive/issues",
        ]);

        let (run_briefing_file, run_result_dir) = match run_args.command {
            Some(Command::Benchmark {
                command: BenchmarkCommand::Run(ref run_args),
            }) => (
                run_args.briefing_file.clone(),
                run_args
                    .result_dir
                    .clone()
                    .unwrap_or_else(crate::quorp::run_support::default_benchmark_run_result_dir),
            ),
            other => panic!("unexpected parsed command: {other:?}"),
        };

        let prompt_briefing_file = match prompt_args.command {
            Some(Command::Benchmark {
                command: BenchmarkCommand::Prompt(prompt_args),
            }) => prompt_args.briefing_file,
            other => panic!("unexpected parsed command: {other:?}"),
        };

        let (batch_briefing_file, batch_result_dir) = match batch_args.command {
            Some(Command::Benchmark {
                command: BenchmarkCommand::Batch(ref batch_args),
            }) => (
                batch_args.briefing_file.clone(),
                batch_args
                    .result_dir
                    .clone()
                    .unwrap_or_else(crate::quorp::run_support::default_benchmark_batch_result_dir),
            ),
            other => panic!("unexpected parsed command: {other:?}"),
        };

        let expected_briefing_file = default_benchmark_briefing_file();
        assert_eq!(run_briefing_file, expected_briefing_file);
        assert_eq!(prompt_briefing_file, expected_briefing_file);
        assert_eq!(batch_briefing_file, expected_briefing_file);
        assert!(run_result_dir.starts_with(paths::temp_dir()));
        assert!(batch_result_dir.starts_with(paths::temp_dir()));
    }

    #[test]
    fn yolo_forces_sandboxed_autonomy() {
        let (sandbox, autonomy_profile) =
            resolve_yolo_run_mode(true, None, "autonomous_host".to_string()).expect("resolve");

        assert_eq!(sandbox, Some(CliSandboxMode::TmpCopy));
        assert_eq!(autonomy_profile, "autonomous_sandboxed");
    }

    #[test]
    fn yolo_rejects_host_sandbox() {
        let error = resolve_yolo_run_mode(
            true,
            Some(CliSandboxMode::Host),
            "autonomous_host".to_string(),
        )
        .expect_err("host yolo rejected");

        assert!(error.to_string().contains("isolated sandbox"));
    }
}

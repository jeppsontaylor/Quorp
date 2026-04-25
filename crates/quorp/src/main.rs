#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod quorp;

use ::paths;
use anyhow::Context as _;
use clap::{Args as ClapArgs, Parser, Subcommand};
use std::path::{Path, PathBuf};
use util::paths::PathWithPosition;

fn main() {
    let args = CliArgs::parse();
    crate::quorp::docker::bootstrap_custom_data_dir_from_env();

    match crate::quorp::docker::maybe_reexec_in_docker(&args) {
        Ok(Some(exit_code)) => std::process::exit(exit_code),
        Ok(None) => {}
        Err(error) => {
            eprintln!("quorp: {error:#}");
            std::process::exit(1);
        }
    }

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
    match args.command {
        Some(Command::SsdDoctor) => run_ssd_doctor(),
        Some(Command::MemAnalyze) => run_mem_analyze(),
        Some(Command::MemLogPath) => run_mem_log_path(),
        Some(Command::Session(args)) => run_session(args),
        Some(Command::Run(args)) => run_autonomous_command(args),
        Some(Command::Diagnostics { command }) => run_diagnostics_command(command),
        Some(Command::Agent { command }) => run_agent_command(command),
        Some(Command::Benchmark { command }) => run_benchmark_command(command),
        None => run_native_tui(SessionLaunchConfig::from_paths_or_urls(
            args.paths_or_urls,
            parse_prompt_compaction_policy_arg(args.prompt_compaction_policy.as_deref())?,
        )),
    }
}

fn run_autonomous_command(args: RunCliArgs) -> anyhow::Result<()> {
    match args.command {
        Some(RunSubcommand::Resume(args)) => {
            let outcome =
                crate::quorp::agent_local::resume_headless_agent(args.result_dir.clone())?;
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
            let workspace =
                std::fs::canonicalize(&workspace_arg).unwrap_or_else(|_| workspace_arg.clone());
            let resolved = if let Some(objective_file) = start.objective_file.clone() {
                let path = if objective_file.is_absolute() {
                    objective_file
                } else {
                    workspace.join(objective_file)
                };
                let discovered = crate::quorp::run_support::resolve_workspace_objective(
                    &workspace,
                    start.condition.as_deref(),
                )?;
                crate::quorp::run_support::ResolvedWorkspaceObjective {
                    objective_file: path,
                    ..discovered
                }
            } else {
                crate::quorp::run_support::resolve_workspace_objective(
                    &workspace,
                    start.condition.as_deref(),
                )?
            };

            let provider = start
                .provider
                .unwrap_or_else(crate::quorp::executor::interactive_provider_from_env);
            let model_id = start
                .model
                .clone()
                .or_else(crate::quorp::provider_config::resolved_model_env)
                .or_else(|| {
                    crate::quorp::tui::model_registry::default_interactive_model_id(provider)
                })
                .ok_or_else(|| {
                    anyhow::anyhow!("no model could be resolved for {}", provider.label())
                })?;
            let result_dir = start.result_dir.unwrap_or_else(|| {
                crate::quorp::run_support::default_run_result_dir(&workspace, "run")
            });
            std::fs::create_dir_all(&result_dir)?;

            apply_session_env_overrides(&SessionLaunchConfig {
                workspace_root: resolved.editable_workspace_root.clone(),
                provider: Some(provider),
                model: Some(model_id.clone()),
                prompt_compaction_policy: None,
            });

            let mut final_outcome: Option<quorp_agent_core::AgentRunOutcome> = None;
            let mut final_evaluation: Option<crate::quorp::run_support::EvaluatorOutcome> = None;
            let mut retry_context: Option<String> = None;
            let mut attempts_run = 0usize;
            let max_attempts = start.max_retries.saturating_add(1).max(1);
            let autonomy_profile = parse_autonomy_profile(&start.autonomy_profile)?;

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

                let outcome = crate::quorp::agent_local::run_headless_agent(
                    crate::quorp::agent_local::HeadlessRunOptions {
                        workspace: resolved.editable_workspace_root.clone(),
                        objective_file: resolved.objective_file.clone(),
                        executor: start.executor,
                        codex_session_strategy: crate::quorp::executor::CodexSessionStrategy {
                            mode: start.codex_session_mode,
                            session_id: start.codex_session_id.clone(),
                        },
                        model_id: model_id.clone(),
                        base_url_override: start.base_url.clone(),
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
                    "workspace": workspace,
                    "challenge_root": resolved.challenge_root,
                    "editable_workspace_root": resolved.editable_workspace_root,
                    "objective_file": resolved.objective_file,
                    "provider": provider.label(),
                    "model_id": model_id,
                    "condition": resolved.selected_condition,
                    "max_retries": start.max_retries,
                    "evaluate_command": resolved.evaluate_command,
                    "reset_command": resolved.reset_command,
                    "runtime": crate::quorp::docker::runtime_metadata_json(),
                }),
            )?;
            crate::quorp::run_support::write_json(
                &result_dir.join("metadata.json"),
                &serde_json::json!({
                    "workspace": workspace,
                    "challenge_root": resolved.challenge_root,
                    "editable_workspace_root": resolved.editable_workspace_root,
                    "objective_file": resolved.objective_file,
                    "provider": provider.label(),
                    "model_id": model_id,
                    "condition": resolved.selected_condition,
                    "attempts_run": attempts_run,
                    "evaluate_command": resolved.evaluate_command,
                    "reset_command": resolved.reset_command,
                    "last_evaluation": final_evaluation,
                    "process_exit_code": final_evaluation.as_ref().map(|evaluation| evaluation.process_exit_code),
                    "process_passed": final_evaluation.as_ref().map(|evaluation| evaluation.process_passed),
                    "logical_success": final_evaluation.as_ref().and_then(|evaluation| evaluation.logical_success),
                    "evaluation_passed": final_evaluation.as_ref().map(|evaluation| evaluation.evaluation_passed),
                    "objective": crate::quorp::run_support::objective_metadata_json(&resolved, &workspace),
                    "runtime": crate::quorp::docker::runtime_metadata_json(),
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
            let outcome = crate::quorp::agent_local::run_headless_agent(
                crate::quorp::agent_local::HeadlessRunOptions {
                    workspace: std::fs::canonicalize(&args.workspace)
                        .unwrap_or_else(|_| args.workspace.clone()),
                    objective_file: PathBuf::from(objective_file.clone()),
                    executor: args.executor,
                    codex_session_strategy: crate::quorp::executor::CodexSessionStrategy {
                        mode: args.codex_session_mode,
                        session_id: args.codex_session_id.clone(),
                    },
                    model_id: args.model,
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
            let outcome = crate::quorp::agent_local::resume_headless_agent(args.result_dir)?;
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
            let result_dir = args.result_dir.clone();
            let compaction_policy = crate::quorp::benchmark::parse_prompt_compaction_policy(
                args.compaction_policy.as_deref(),
            )?;
            crate::quorp::benchmark::run_benchmark(crate::quorp::benchmark::BenchmarkRunOptions {
                path: std::fs::canonicalize(&args.path).unwrap_or_else(|_| args.path.clone()),
                executor: args.executor,
                model_id: args.model,
                base_url_override: args.base_url,
                briefing_file: Some(args.briefing_file),
                compaction_policy,
                seed_transcript: args.seed_transcript,
                max_steps: args.max_steps,
                max_seconds: Some(args.max_seconds),
                max_total_tokens: args.token_budget,
                result_dir: args.result_dir,
                autonomy_profile: parse_autonomy_profile(&args.autonomy_profile)?,
                max_attempts: args.max_attempts,
                allow_heavy_local_model: args.allow_heavy_local_model,
                condition: args.condition,
                keep_sandbox: args.keep_sandbox,
            })
            .and_then(|_| ensure_benchmark_succeeded(&result_dir))
        }
        BenchmarkCommand::Prompt(args) => {
            let bundle = crate::quorp::benchmark::prepare_benchmark_prompt_bundle(
                &args.path,
                &args.workspace_dir,
                args.executor,
                args.model,
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
                },
            )?;
            println!("{}", artifacts.markdown);
            eprintln!("Scoreboard written to {}", artifacts.output_dir.display());
            Ok(())
        }
        BenchmarkCommand::Batch(args) => {
            let compaction_policy = crate::quorp::benchmark::parse_prompt_compaction_policy(
                args.compaction_policy.as_deref(),
            )?;
            crate::quorp::benchmark::run_benchmark_batch(
                crate::quorp::benchmark::BenchmarkBatchRunOptions {
                    cases_root: std::fs::canonicalize(&args.cases_root)
                        .unwrap_or_else(|_| args.cases_root.clone()),
                    result_dir: args.result_dir,
                    executor: args.executor,
                    model_id: args.model,
                    base_url_override: args.base_url,
                    briefing_file: Some(args.briefing_file),
                    compaction_policy,
                    seed_transcript: args.seed_transcript,
                    max_steps: args.max_steps,
                    max_seconds: Some(args.max_seconds),
                    max_total_tokens: args.token_budget,
                    max_attempts: args.max_attempts,
                    autonomy_profile: parse_autonomy_profile(&args.autonomy_profile)?,
                    allow_heavy_local_model: args.allow_heavy_local_model,
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

fn run_session(args: SessionArgs) -> anyhow::Result<()> {
    let launch = SessionLaunchConfig::from_workspace(
        args.workspace.unwrap_or_else(default_workspace_root),
        args.provider,
        args.model,
        parse_prompt_compaction_policy_arg(args.prompt_compaction_policy.as_deref())?,
    );
    run_native_tui(launch)
}

fn run_native_tui(launch: SessionLaunchConfig) -> anyhow::Result<()> {
    apply_session_env_overrides(&launch);
    let workspace_root = launch.workspace_root;
    let app_run_id = crate::quorp::tui::diagnostics::app_run_id().to_string();
    crate::quorp::tui::diagnostics::log_event(
        "app.run_native_tui",
        serde_json::json!({
            "workspace_root": workspace_root.display().to_string(),
            "runtime": crate::quorp::docker::runtime_metadata_json(),
        }),
    );
    if let Err(error) =
        crate::quorp::memory_fingerprint::start_memory_logger(&workspace_root, &app_run_id)
    {
        crate::quorp::tui::diagnostics::log_event(
            "memory.sampler_error",
            serde_json::json!({
                "detail": error.to_string(),
            }),
        );
    } else {
        crate::quorp::tui::diagnostics::log_event(
            "memory.logger_started",
            serde_json::json!({
                "path": paths::memory_log_file().display().to_string(),
                "sample_interval_ms": crate::quorp::memory_fingerprint::MEMORY_SAMPLE_INTERVAL.as_millis() as u64,
                "runtime": crate::quorp::docker::runtime_metadata_json(),
            }),
        );
    }
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel::<crate::quorp::tui::TuiEvent>(
        crate::quorp::tui::TUI_EVENT_QUEUE_CAPACITY,
    );
    let (input_tx, input_rx) = std::sync::mpsc::channel::<crossterm::event::Event>();
    let chat_tx = event_tx.clone();

    crate::quorp::tui::run(
        workspace_root,
        event_rx,
        input_rx,
        input_tx,
        chat_tx,
        None,
        None,
        None,
        None,
    )
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
    match launch.prompt_compaction_policy {
        Some(policy) => unsafe {
            std::env::set_var("QUORP_PROMPT_COMPACTION_POLICY", policy.as_str());
        },
        None => unsafe {
            std::env::remove_var("QUORP_PROMPT_COMPACTION_POLICY");
        },
    }
}

fn run_ssd_doctor() -> anyhow::Result<()> {
    let model = crate::quorp::tui::model_registry::get_saved_model().ok_or_else(|| {
        anyhow::anyhow!("shared SSD-MOE broker did not return any runnable models")
    })?;
    println!(
        "{}",
        crate::quorp::tui::ssd_moe_tui::SsdMoeManager::doctor_report(&model)
    );
    Ok(())
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
        .join("../../docs/qwen3-coder-30b-a3b-ssd-moe-benchmark.md")
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
    prompt_compaction_policy: Option<quorp_agent_core::PromptCompactionPolicy>,
}

impl SessionLaunchConfig {
    fn from_workspace(
        workspace: PathBuf,
        provider: Option<crate::quorp::executor::InteractiveProviderKind>,
        model: Option<String>,
        prompt_compaction_policy: Option<quorp_agent_core::PromptCompactionPolicy>,
    ) -> Self {
        Self {
            workspace_root: initial_workspace_root(&[workspace.display().to_string()]),
            provider,
            model: model.filter(|value| !value.trim().is_empty()),
            prompt_compaction_policy,
        }
    }

    fn from_paths_or_urls(
        paths_or_urls: Vec<String>,
        prompt_compaction_policy: Option<quorp_agent_core::PromptCompactionPolicy>,
    ) -> Self {
        Self {
            workspace_root: initial_workspace_root(&paths_or_urls),
            provider: None,
            model: None,
            prompt_compaction_policy,
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "quorp", version = env!("CARGO_PKG_VERSION"))]
struct CliArgs {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(long)]
    prompt_compaction_policy: Option<String>,
    paths_or_urls: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    SsdDoctor,
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
}

#[derive(ClapArgs, Debug)]
pub struct SessionArgs {
    #[arg(long)]
    workspace: Option<PathBuf>,
    #[arg(long, value_enum)]
    provider: Option<crate::quorp::executor::InteractiveProviderKind>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    prompt_compaction_policy: Option<String>,
    #[command(flatten)]
    docker: crate::quorp::docker::DockerArgs,
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
    model: String,
    #[arg(long, value_enum, default_value = "native")]
    executor: crate::quorp::executor::QuorpExecutor,
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long, value_enum, default_value = "fresh")]
    codex_session_mode: crate::quorp::executor::CodexSessionMode,
    #[arg(long)]
    codex_session_id: Option<String>,
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
    #[command(flatten)]
    docker: crate::quorp::docker::DockerArgs,
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
    #[arg(long, value_enum)]
    provider: Option<crate::quorp::executor::InteractiveProviderKind>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long, value_enum, default_value = "native")]
    executor: crate::quorp::executor::QuorpExecutor,
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long, value_enum, default_value = "fresh")]
    codex_session_mode: crate::quorp::executor::CodexSessionMode,
    #[arg(long)]
    codex_session_id: Option<String>,
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
    #[arg(long, default_value = "autonomous_host")]
    autonomy_profile: String,
    #[command(flatten)]
    docker: crate::quorp::docker::DockerArgs,
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
    #[arg(long, value_enum, default_value = "codex")]
    executor: crate::quorp::benchmark::BenchmarkExecutor,
    #[arg(long)]
    model: Option<String>,
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
    result_dir: PathBuf,
    #[arg(long, default_value = "autonomous_host")]
    autonomy_profile: String,
    #[arg(long, default_value_t = false)]
    allow_heavy_local_model: bool,
    #[arg(long)]
    condition: Option<String>,
    #[arg(long, default_value_t = false)]
    keep_sandbox: bool,
    #[arg(long)]
    log_file: Option<PathBuf>,
    #[command(flatten)]
    docker: crate::quorp::docker::DockerArgs,
}

#[derive(ClapArgs, Debug)]
pub struct BenchmarkResumeArgs {
    #[arg(long)]
    result_dir: PathBuf,
    #[command(flatten)]
    docker: crate::quorp::docker::DockerArgs,
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
}

#[derive(ClapArgs, Debug)]
pub struct BenchmarkPromptArgs {
    #[arg(long)]
    path: PathBuf,
    #[arg(long)]
    workspace_dir: PathBuf,
    #[arg(long, value_enum, default_value = "codex")]
    executor: crate::quorp::benchmark::BenchmarkExecutor,
    #[arg(long)]
    model: Option<String>,
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
    result_dir: PathBuf,
    #[arg(long, value_enum, default_value = "codex")]
    executor: crate::quorp::benchmark::BenchmarkExecutor,
    #[arg(long)]
    model: Option<String>,
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
    #[arg(long, default_value_t = false)]
    allow_heavy_local_model: bool,
    #[arg(long)]
    condition: Option<String>,
    #[arg(long, default_value_t = false)]
    keep_sandbox: bool,
    #[arg(long)]
    log_dir: Option<PathBuf>,
    #[command(flatten)]
    docker: crate::quorp::docker::DockerArgs,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorthand_and_session_workspace_resolution_match() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let challenge = temp_dir.path().join("04-entitlement-recovery-replay");
        std::fs::create_dir_all(&challenge).expect("challenge dir");

        let shorthand =
            SessionLaunchConfig::from_paths_or_urls(vec![challenge.display().to_string()], None);
        let explicit = SessionLaunchConfig::from_workspace(challenge, None, None, None);

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
            "--result-dir",
            "/tmp/quorp-benchmark-result",
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
            "--result-dir",
            "/tmp/quorp-benchmark-batch-result",
        ]);

        let run_briefing_file = match run_args.command {
            Some(Command::Benchmark {
                command: BenchmarkCommand::Run(run_args),
            }) => run_args.briefing_file,
            other => panic!("unexpected parsed command: {other:?}"),
        };

        let prompt_briefing_file = match prompt_args.command {
            Some(Command::Benchmark {
                command: BenchmarkCommand::Prompt(prompt_args),
            }) => prompt_args.briefing_file,
            other => panic!("unexpected parsed command: {other:?}"),
        };

        let batch_briefing_file = match batch_args.command {
            Some(Command::Benchmark {
                command: BenchmarkCommand::Batch(batch_args),
            }) => batch_args.briefing_file,
            other => panic!("unexpected parsed command: {other:?}"),
        };

        let expected_briefing_file = default_benchmark_briefing_file();
        assert_eq!(run_briefing_file, expected_briefing_file);
        assert_eq!(prompt_briefing_file, expected_briefing_file);
        assert_eq!(batch_briefing_file, expected_briefing_file);
    }
}

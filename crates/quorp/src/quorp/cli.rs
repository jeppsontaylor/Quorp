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
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthChar;
use util::paths::PathWithPosition;

#[path = "cli_runtime.rs"]
mod runtime;
pub(crate) use runtime::{
    apply_session_env_overrides, run_inline_cli, run_mem_analyze, run_mem_log_path,
};

pub(crate) fn main() {
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
        Some(Command::Replay(args)) => run_replay_command(args),
        Some(Command::Proof { command }) => run_proof_command(command),
        Some(Command::Diagnostics { command }) => run_diagnostics_command(command),
        Some(Command::Agent { command }) => run_agent_command(command),
        Some(Command::Benchmark { command }) => run_benchmark_command(command),
        Some(Command::RenderDemo) => crate::quorp::cli_demos::run_render_demo(),
        Some(Command::Commands { prefix }) => crate::quorp::cli_demos::run_commands_command(prefix),
        Some(Command::Scan { workspace, symbols }) => {
            crate::quorp::cli_demos::run_scan_command(workspace, symbols)
        }
        Some(Command::Index { command }) => match command {
            IndexCommand::Build { workspace } => {
                crate::quorp::cli_demos::run_index_build_command(workspace)
            }
            IndexCommand::Status { workspace } => {
                crate::quorp::cli_demos::run_index_status_command(workspace)
            }
            IndexCommand::Explain { workspace, symbol } => {
                crate::quorp::cli_demos::run_index_explain_command(workspace, symbol)
            }
        },
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
    provider: crate::quorp::executor::InteractiveProviderKind,
) -> anyhow::Result<String> {
    Ok(match provider {
        crate::quorp::executor::InteractiveProviderKind::Local => {
            "ssd_moe/qwen3-coder-30b-a3b".to_string()
        }
        crate::quorp::executor::InteractiveProviderKind::Nvidia => {
            crate::quorp::provider_config::NVIDIA_QWEN_MODEL.to_string()
        }
    })
}

fn default_base_url_for_workspace(
    workspace: &Path,
    provider: crate::quorp::executor::InteractiveProviderKind,
) -> anyhow::Result<Option<String>> {
    let loaded = load_workspace_settings(workspace)?;
    let base_url = match provider {
        crate::quorp::executor::InteractiveProviderKind::Local => crate::quorp::provider_config::env_value("QUORP_LOCAL_BASE_URL")
            .or_else(|| {
                let base_url = loaded.settings.provider.base_url.trim();
                (!base_url.is_empty()).then(|| base_url.to_string())
            }),
        crate::quorp::executor::InteractiveProviderKind::Nvidia => crate::quorp::provider_config::env_value("QUORP_NVIDIA_BASE_URL")
            .or_else(|| {
                let base_url = loaded.settings.provider.base_url.trim();
                (!base_url.is_empty()).then(|| base_url.to_string())
            }),
    };
    Ok(base_url)
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
        ("run-ledger", input.result_dir.join("run-ledger.jsonl")),
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
    extend_receipt_with_verify_artifacts(&mut receipt, input.active_workspace)?;
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

fn sha256_file_required(path: &Path) -> anyhow::Result<String> {
    sha256_file_if_exists(path)?.ok_or_else(|| anyhow::anyhow!("{} does not exist", path.display()))
}

fn insert_artifact_if_exists(
    artifacts: &mut BTreeMap<String, quorp_core::RawArtifact>,
    name: impl Into<String>,
    path: PathBuf,
) -> anyhow::Result<()> {
    if path.exists() {
        artifacts.insert(
            name.into(),
            quorp_core::RawArtifact {
                sha256: Some(sha256_file_required(&path)?),
                path,
            },
        );
    }
    Ok(())
}

fn collect_verify_dag_artifacts(
    workspace: &Path,
) -> anyhow::Result<Vec<(String, PathBuf, quorp_verify::ProofDag)>> {
    let runs_dir = workspace.join(".quorp").join("verify").join("runs");
    if !runs_dir.exists() {
        return Ok(Vec::new());
    }
    let mut dag_paths = Vec::new();
    for entry in std::fs::read_dir(&runs_dir)? {
        let entry = entry?;
        let path = entry.path().join("proof-dag.json");
        if path.exists() {
            dag_paths.push(path);
        }
    }
    dag_paths.sort();
    let mut dags = Vec::new();
    for path in dag_paths {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let dag: quorp_verify::ProofDag = serde_json::from_str(&text)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        let label = dag.run_id.as_str().to_string();
        dags.push((label, path, dag));
    }
    Ok(dags)
}

fn extend_receipt_with_verify_artifacts(
    receipt: &mut quorp_core::ProofReceipt,
    workspace: &Path,
) -> anyhow::Result<()> {
    let dags = collect_verify_dag_artifacts(workspace)?;
    for (index, (run_id, dag_path, dag)) in dags.into_iter().enumerate() {
        let dag_name = if index == 0 {
            "proof-dag".to_string()
        } else {
            format!("proof-dag-{run_id}")
        };
        insert_artifact_if_exists(&mut receipt.raw_artifacts, dag_name, dag_path)?;
        for node in dag.nodes {
            for artifact in node.artifacts {
                if artifact.role == "raw_log" {
                    let name = format!(
                        "verify-log-{}-{}",
                        node.stage_id,
                        receipt.raw_artifacts.len()
                    );
                    receipt.raw_artifacts.insert(
                        name,
                        quorp_core::RawArtifact {
                            sha256: Some(
                                sha256_file_if_exists(&artifact.path)?
                                    .unwrap_or(artifact.sha256),
                            ),
                            path: artifact.path,
                        },
                    );
                }
            }
        }
    }
    Ok(())
}

fn run_replay_command(args: ReplayArgs) -> anyhow::Result<()> {
    println!("{}", render_replay_summary(&args.run_dir)?);
    Ok(())
}

fn render_replay_summary(run_dir: &Path) -> anyhow::Result<String> {
    let ledger_path = run_dir.join("run-ledger.jsonl");
    if !ledger_path.exists() {
        anyhow::bail!("{} is required for replay", ledger_path.display());
    }
    let reader = crate::quorp::run_support::RunLedgerReader::open(&ledger_path);
    let report = reader.validate_hash_chain()?;
    let events = reader.read_all()?;
    let mut lines = vec![
        format!("run_dir: {}", run_dir.display()),
        format!("run_id: {}", report.run_id.as_deref().unwrap_or("unknown")),
        format!("events: {}", report.event_count),
        format!(
            "first_seq: {}",
            report
                .first_seq
                .map(|seq| seq.to_string())
                .unwrap_or_else(|| "none".to_string())
        ),
        format!(
            "last_seq: {}",
            report
                .last_seq
                .map(|seq| seq.to_string())
                .unwrap_or_else(|| "none".to_string())
        ),
        "kind_counts:".to_string(),
    ];
    for (kind, count) in report.kind_counts {
        lines.push(format!("- {kind}: {count}"));
    }
    lines.push(format!(
        "run_started: {}",
        replay_fact(&events, &["RunStarted", "run.started"]).unwrap_or_else(|| "none".to_string())
    ));
    lines.push(format!(
        "run_finished: {}",
        replay_fact(&events, &["RunFinished", "run.finished", "run.stop_cause"])
            .unwrap_or_else(|| "none".to_string())
    ));
    Ok(lines.join("\n"))
}

fn replay_fact(
    events: &[crate::quorp::run_support::RunLedgerEvent],
    names: &[&str],
) -> Option<String> {
    events.iter().find_map(|event| {
        if names.iter().any(|name| event.kind == *name) {
            return Some(compact_json(&event.payload));
        }
        for name in names {
            if let Some(value) = event.payload.get(*name) {
                return Some(compact_json(value));
            }
            if event.payload.get("event").and_then(serde_json::Value::as_str) == Some(*name) {
                return Some(compact_json(&event.payload));
            }
        }
        None
    })
}

fn compact_json(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "unprintable".to_string())
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ProofBundle {
    bundle_version: u32,
    receipt: quorp_core::ProofReceipt,
    ledger_path: Option<PathBuf>,
    ledger_hash: Option<String>,
    proof_dag_path: Option<PathBuf>,
    proof_dag_hash: Option<String>,
    proof_dag: Option<quorp_verify::ProofDag>,
    raw_artifacts: BTreeMap<String, quorp_core::RawArtifact>,
}

fn run_proof_command(command: ProofCommand) -> anyhow::Result<()> {
    match command {
        ProofCommand::Show(args) => {
            println!("{}", render_proof_show(&args.run_dir)?);
            Ok(())
        }
        ProofCommand::Export(args) => {
            let bundle = proof_bundle_for_run(&args.run_dir)?;
            if let Some(parent) = args.output.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&args.output, serde_json::to_vec_pretty(&bundle)?)?;
            println!("{}", args.output.display());
            Ok(())
        }
        ProofCommand::Verify(args) => {
            verify_proof_input(&args.input)?;
            println!("proof verified: {}", args.input.display());
            Ok(())
        }
    }
}

fn render_proof_show(run_dir: &Path) -> anyhow::Result<String> {
    let bundle = proof_bundle_for_run(run_dir)?;
    let verdict = bundle
        .proof_dag
        .as_ref()
        .map(proof_dag_verdict)
        .unwrap_or_else(|| "unknown".to_string());
    let mut lines = vec![
        format!("receipt: {}", run_dir.join("proof-receipt.json").display()),
        format!("run_id: {}", bundle.receipt.run_id),
        format!(
            "ledger: {}",
            bundle
                .ledger_hash
                .as_deref()
                .map(|hash| format!("sha256={hash}"))
                .unwrap_or_else(|| "missing".to_string())
        ),
        format!(
            "proof_dag: {}",
            bundle
                .proof_dag_hash
                .as_deref()
                .map(|hash| format!("sha256={hash}"))
                .unwrap_or_else(|| "missing".to_string())
        ),
        format!("verdict: {verdict}"),
        format!("artifacts: {}", bundle.raw_artifacts.len()),
    ];
    for (name, artifact) in &bundle.raw_artifacts {
        lines.push(format!(
            "- {name}: {} {}",
            artifact.path.display(),
            artifact.sha256.as_deref().unwrap_or("no-hash")
        ));
    }
    Ok(lines.join("\n"))
}

fn proof_bundle_for_run(run_dir: &Path) -> anyhow::Result<ProofBundle> {
    let receipt_path = run_dir.join("proof-receipt.json");
    let receipt_text = std::fs::read_to_string(&receipt_path)
        .with_context(|| format!("failed to read {}", receipt_path.display()))?;
    let receipt: quorp_core::ProofReceipt = serde_json::from_str(&receipt_text)
        .with_context(|| format!("failed to parse {}", receipt_path.display()))?;
    let ledger_path = run_dir.join("run-ledger.jsonl");
    let ledger_hash = sha256_file_if_exists(&ledger_path)?;
    let ledger_path = ledger_hash.as_ref().map(|_| ledger_path);
    let proof_dag_path = receipt
        .raw_artifacts
        .iter()
        .find(|(name, _)| name.starts_with("proof-dag"))
        .map(|(_, artifact)| resolve_artifact_path(run_dir, &artifact.path));
    let proof_dag = proof_dag_path
        .as_ref()
        .map(|path| {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
        })
        .transpose()?;
    let proof_dag_hash = proof_dag_path
        .as_ref()
        .map(|path| sha256_file_required(path))
        .transpose()?;
    Ok(ProofBundle {
        bundle_version: 1,
        raw_artifacts: receipt.raw_artifacts.clone(),
        receipt,
        ledger_path,
        ledger_hash,
        proof_dag_path,
        proof_dag_hash,
        proof_dag,
    })
}

fn verify_proof_input(input: &Path) -> anyhow::Result<()> {
    if input.is_dir() {
        verify_proof_run_dir(input)
    } else {
        verify_proof_bundle_file(input)
    }
}

fn verify_proof_run_dir(run_dir: &Path) -> anyhow::Result<()> {
    let bundle = proof_bundle_for_run(run_dir)?;
    verify_bundle_hashes(&bundle, run_dir)?;
    if let Some(ledger_path) = bundle.ledger_path.as_ref() {
        crate::quorp::run_support::RunLedgerReader::open(ledger_path).validate_hash_chain()?;
    }
    Ok(())
}

fn verify_proof_bundle_file(path: &Path) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let bundle: ProofBundle = serde_json::from_str(&text)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    verify_bundle_hashes(&bundle, base)
}

fn verify_bundle_hashes(bundle: &ProofBundle, base: &Path) -> anyhow::Result<()> {
    for (name, artifact) in &bundle.raw_artifacts {
        if let Some(expected) = artifact.sha256.as_deref() {
            let path = resolve_artifact_path(base, &artifact.path);
            let actual = sha256_file_required(&path)
                .with_context(|| format!("failed to verify artifact `{name}`"))?;
            if actual != expected {
                anyhow::bail!(
                    "artifact `{name}` hash mismatch: expected {expected}, got {actual}"
                );
            }
        }
    }
    if let (Some(path), Some(expected)) = (&bundle.ledger_path, &bundle.ledger_hash) {
        let path = resolve_artifact_path(base, path);
        let actual = sha256_file_required(&path)?;
        if &actual != expected {
            anyhow::bail!("ledger hash mismatch: expected {expected}, got {actual}");
        }
    }
    if let Some(dag) = bundle.proof_dag.as_ref() {
        verify_dag_artifacts(dag, base)?;
    }
    Ok(())
}

fn verify_dag_artifacts(dag: &quorp_verify::ProofDag, base: &Path) -> anyhow::Result<()> {
    for node in &dag.nodes {
        for artifact in &node.artifacts {
            let path = resolve_artifact_path(base, &artifact.path);
            let actual = sha256_file_required(&path)?;
            if actual != artifact.sha256 {
                anyhow::bail!(
                    "proof DAG artifact {} hash mismatch: expected {}, got {}",
                    path.display(),
                    artifact.sha256,
                    actual
                );
            }
        }
    }
    Ok(())
}

fn proof_dag_verdict(dag: &quorp_verify::ProofDag) -> String {
    if dag.nodes.is_empty() {
        return "empty".to_string();
    }
    if dag
        .nodes
        .iter()
        .all(|node| matches!(node.status, quorp_verify::ProofNodeStatus::Pass | quorp_verify::ProofNodeStatus::Cached))
    {
        "pass".to_string()
    } else if dag
        .nodes
        .iter()
        .any(|node| matches!(node.status, quorp_verify::ProofNodeStatus::Fail))
    {
        "fail".to_string()
    } else {
        "partial".to_string()
    }
}

fn resolve_artifact_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
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
            let provider = crate::quorp::provider_config::resolved_provider_env().unwrap_or_else(
                || {
                    if matches!(
                        crate::quorp::provider_config::resolved_routing_mode(),
                        crate::quorp::provider_config::RoutingMode::Local
                    ) {
                        crate::quorp::executor::InteractiveProviderKind::Local
                    } else {
                        default_provider_for_workspace(&source_workspace)
                            .unwrap_or(crate::quorp::executor::InteractiveProviderKind::Nvidia)
                    }
                },
            );
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
pub(crate) struct SessionLaunchConfig {
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
    Replay(ReplayArgs),
    Proof {
        #[command(subcommand)]
        command: ProofCommand,
    },
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
    Index {
        #[command(subcommand)]
        command: IndexCommand,
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
pub enum IndexCommand {
    Build {
        #[arg(long, value_name = "PATH")]
        workspace: Option<PathBuf>,
    },
    Status {
        #[arg(long, value_name = "PATH")]
        workspace: Option<PathBuf>,
    },
    Explain {
        #[arg(long, value_name = "PATH")]
        workspace: Option<PathBuf>,
        symbol: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum RunSubcommand {
    Resume(RunResumeArgs),
}

#[derive(Subcommand, Debug)]
pub enum ProofCommand {
    Show(ProofShowArgs),
    Export(ProofExportArgs),
    Verify(ProofVerifyArgs),
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
pub struct ReplayArgs {
    #[arg(value_name = "RUN_DIR")]
    run_dir: PathBuf,
}

#[derive(ClapArgs, Debug)]
pub struct ProofShowArgs {
    #[arg(value_name = "RUN_DIR")]
    run_dir: PathBuf,
}

#[derive(ClapArgs, Debug)]
pub struct ProofExportArgs {
    #[arg(value_name = "RUN_DIR")]
    run_dir: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

#[derive(ClapArgs, Debug)]
pub struct ProofVerifyArgs {
    #[arg(value_name = "PROOF_FILE_OR_RUN_DIR")]
    input: PathBuf,
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
#[path = "../../../../testing/quorp/quorp/cli/tests.rs"]
mod tests;

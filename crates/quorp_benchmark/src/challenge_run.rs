use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use anyhow::Context as _;
use serde::Serialize;

use crate::{
    ChallengeMetadata, EvaluatorOutcome, ResolvedBenchmark, ResolvedChallengeCase,
    build_challenge_objective, collect_challenge_context_files, collect_repair_artifacts,
    compile_challenge_capsule, copy_dir_all, copy_file_if_different, ensure_git_baseline,
    maybe_materialize_flat_challenge_reset_script, maybe_materialize_rustbench_workspace,
    resolve_challenge_workspace_dir, run_shell_command, substitute_condition,
    write_benchmark_sandbox_cargo_config, write_workspace_challenge_command_wrappers,
};

#[derive(Debug, Clone)]
pub struct PreparedChallengeRun {
    pub resolved: ResolvedBenchmark,
    pub challenge_metadata: ChallengeMetadata,
    pub reset_outcome: EvaluatorOutcome,
}

pub fn prepare_challenge_run<FLog, FAgentConfig>(
    result_dir: &Path,
    challenge: &ResolvedChallengeCase,
    challenge_sandbox_dir_name: &str,
    challenge_objective_file_name: &str,
    challenge_cargo_cache_dir_name: &str,
    mut log_sandbox: FLog,
    mut write_agent_config: FAgentConfig,
) -> anyhow::Result<PreparedChallengeRun>
where
    FLog: FnMut(String),
    FAgentConfig: FnMut(&Path) -> anyhow::Result<()>,
{
    let sandbox_root = result_dir.join(challenge_sandbox_dir_name);
    if sandbox_root.exists() {
        fs::remove_dir_all(&sandbox_root)
            .with_context(|| format!("failed to clean {}", sandbox_root.display()))?;
    }
    log_sandbox(format!(
        "copying challenge bundle {} -> {}",
        challenge.case_root.display(),
        sandbox_root.display()
    ));
    copy_dir_all(&challenge.case_root, &sandbox_root)?;
    if let Some(materialized) =
        maybe_materialize_rustbench_workspace(&sandbox_root, &challenge.condition)?
    {
        log_sandbox(format!(
            "materializing Rustbench workspace {} @ {} -> {}",
            materialized.repo,
            materialized.base_commit,
            materialized.workspace_dir.display()
        ));
    }
    maybe_materialize_flat_challenge_reset_script(result_dir, &sandbox_root)?;

    let objective_path = sandbox_root.join(challenge_objective_file_name);
    let sandbox_objective_source = challenge
        .objective_source
        .strip_prefix(&challenge.case_root)
        .map(|relative| sandbox_root.join(relative))
        .unwrap_or_else(|_| challenge.objective_source.clone());
    let sandbox_success_source = challenge
        .success_source
        .strip_prefix(&challenge.case_root)
        .map(|relative| sandbox_root.join(relative))
        .unwrap_or_else(|_| challenge.success_source.clone());
    let capsule = compile_challenge_capsule(challenge, &sandbox_root)?;
    write_benchmark_sandbox_cargo_config(
        &sandbox_root,
        &challenge.condition,
        challenge_cargo_cache_dir_name,
    )?;
    let reset_command =
        substitute_condition(&challenge.manifest.reset_command, &challenge.condition);
    let reset_outcome = run_shell_command(
        "reset",
        &reset_command,
        &sandbox_root.join("reset.sh"),
        &sandbox_root,
    )?;

    let workspace_dir = resolve_challenge_workspace_dir(&sandbox_root, &challenge.condition)?;
    let workspace_objective_file = workspace_dir.join(
        sandbox_objective_source
            .file_name()
            .unwrap_or_else(|| OsStr::new("START_HERE.md")),
    );
    let workspace_success_file = workspace_dir.join(
        sandbox_success_source
            .file_name()
            .unwrap_or_else(|| OsStr::new("SUCCESS.md")),
    );
    let workspace_benchmark_file = workspace_dir.join("benchmark.json");
    let sandbox_reference_source = sandbox_root.join("REFERENCE.md");
    let workspace_reference_file = sandbox_reference_source
        .exists()
        .then(|| workspace_dir.join("REFERENCE.md"));
    let capsule_file = workspace_dir.join(".quorp").join("challenge-capsule.json");

    fs::create_dir_all(&workspace_dir)
        .with_context(|| format!("failed to create {}", workspace_dir.display()))?;
    copy_file_if_different(&sandbox_objective_source, &workspace_objective_file).with_context(
        || {
            format!(
                "failed to mirror challenge objective {} into {}",
                sandbox_objective_source.display(),
                workspace_objective_file.display()
            )
        },
    )?;
    copy_file_if_different(&sandbox_success_source, &workspace_success_file).with_context(|| {
        format!(
            "failed to mirror challenge success file {} into {}",
            sandbox_success_source.display(),
            workspace_success_file.display()
        )
    })?;
    copy_file_if_different(
        &sandbox_root.join("benchmark.json"),
        &workspace_benchmark_file,
    )
    .with_context(|| {
        format!(
            "failed to mirror challenge manifest into {}",
            workspace_benchmark_file.display()
        )
    })?;
    if let Some(workspace_reference_file) = workspace_reference_file.as_ref() {
        copy_file_if_different(&sandbox_reference_source, workspace_reference_file).with_context(
            || {
                format!(
                    "failed to mirror challenge reference file {} into {}",
                    sandbox_reference_source.display(),
                    workspace_reference_file.display()
                )
            },
        )?;
    }

    if let Some(parent) = capsule_file.parent() {
        fs::create_dir_all(parent)?;
    }
    write_json_file(&capsule_file, &capsule)?;

    let challenge_metadata = ChallengeMetadata {
        case_root: challenge.case_root.clone(),
        sandbox_root: sandbox_root.clone(),
        workspace_dir: workspace_dir.clone(),
        condition: challenge.condition.clone(),
        objective_file: workspace_objective_file,
        success_file: workspace_success_file,
        reference_file: workspace_reference_file,
        reset_command: challenge.manifest.reset_command.clone(),
        evaluate_command: challenge.manifest.evaluate_command.clone(),
        expected_files_touched: challenge.manifest.expected_files_touched.clone(),
        allowed_generated_files: challenge.manifest.allowed_generated_files.clone(),
        primary_metrics: challenge.manifest.primary_metrics.clone(),
        tags: challenge.manifest.tags.clone(),
        capsule_file,
        capsule,
    };
    let objective_text = build_challenge_objective(challenge, &challenge_metadata)?;
    fs::write(&objective_path, objective_text)
        .with_context(|| format!("failed to write {}", objective_path.display()))?;

    let resolved = ResolvedBenchmark {
        benchmark_root: sandbox_root.clone(),
        issue_id: challenge.manifest.id.clone(),
        benchmark_name: challenge.manifest.title.clone(),
        issue_dir: None,
        workspace_source: workspace_dir.clone(),
        objective_source: objective_path,
        visible_evaluator: None,
        collector_evaluator: None,
        context_files: collect_challenge_context_files(&challenge_metadata),
        repair_artifacts: collect_repair_artifacts(&workspace_dir),
    };

    if reset_outcome.passed {
        write_workspace_challenge_command_wrappers(&workspace_dir)?;
        ensure_git_baseline(&workspace_dir)?;
        write_benchmark_sandbox_cargo_config(
            &sandbox_root,
            &challenge.condition,
            challenge_cargo_cache_dir_name,
        )?;
        write_agent_config(&workspace_dir)?;
    }

    Ok(PreparedChallengeRun {
        resolved,
        challenge_metadata,
        reset_outcome,
    })
}

pub fn reset_challenge_workspace_for_attempt<FAgentConfig>(
    challenge_metadata: &ChallengeMetadata,
    attempt_number: usize,
    executor_is_native: bool,
    challenge_cargo_cache_dir_name: &str,
    mut write_agent_config: FAgentConfig,
) -> anyhow::Result<Option<EvaluatorOutcome>>
where
    FAgentConfig: FnMut(&Path) -> anyhow::Result<()>,
{
    if attempt_number <= 1 {
        return Ok(None);
    }

    let reset_command = substitute_condition(
        &challenge_metadata.reset_command,
        &challenge_metadata.condition,
    );
    let reset_outcome = run_shell_command(
        "reset",
        &reset_command,
        &challenge_metadata.sandbox_root.join("reset.sh"),
        &challenge_metadata.sandbox_root,
    )?;
    if !reset_outcome.passed {
        return Ok(Some(reset_outcome));
    }

    if let Some(parent) = challenge_metadata.capsule_file.parent() {
        fs::create_dir_all(parent)?;
    }
    write_json_file(
        &challenge_metadata.capsule_file,
        &challenge_metadata.capsule,
    )?;
    write_workspace_challenge_command_wrappers(&challenge_metadata.workspace_dir)?;
    ensure_git_baseline(&challenge_metadata.workspace_dir)?;
    write_benchmark_sandbox_cargo_config(
        &challenge_metadata.sandbox_root,
        &challenge_metadata.condition,
        challenge_cargo_cache_dir_name,
    )?;
    if executor_is_native {
        write_agent_config(&challenge_metadata.workspace_dir)?;
    }
    Ok(Some(reset_outcome))
}

fn write_json_file(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    let content = serde_json::to_vec_pretty(value)?;
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

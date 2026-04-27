//! Benchmark contract types for the agent-first crate split.

mod challenge_prep;
mod challenge_run;
mod challenge_workspace;
mod evaluation;
mod reporting;
mod reporting_types;
mod resolution;
mod rustbench;
mod scoring;
mod workspace;

use std::path::PathBuf;

use quorp_core::{ProofReceipt, SandboxMode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use challenge_prep::{
    RustSweCaseProfile, build_challenge_objective, collect_challenge_context_files,
    compile_challenge_capsule, rust_swe_case_profile, substitute_condition,
    summarize_markdown_brief, summarize_workspace_root,
};
pub use challenge_run::{
    PreparedChallengeRun, prepare_challenge_run, reset_challenge_workspace_for_attempt,
};
pub use challenge_workspace::{
    maybe_materialize_flat_challenge_reset_script, resolve_challenge_workspace_dir,
    write_benchmark_sandbox_cargo_config, write_workspace_challenge_command_wrappers,
};
pub use evaluation::{
    EvaluatorOutcome, challenge_evaluation_env, challenge_evaluation_target_dir, evaluator_passed,
    parse_benchmark_summary_value, run_collector_evaluator, run_shell_command,
    run_shell_command_with_env, run_visible_evaluator,
};
pub use reporting::{
    read_case_report_scorecard, render_batch_report, render_report_markdown, render_run_summary,
    summarize_batch_report, summarize_run_report,
};
pub use reporting_types::{
    AttemptReport, BatchCaseReport, BatchReport, BenchmarkExecutor, BenchmarkReport,
    BenchmarkScoreCase, BenchmarkScoreReport, ChallengeJudgeOutcome, PromptTokenTurnSample,
    ReadRangeObservation, RoutingSummary, RunSummary, RunSummaryCase,
};
pub use resolution::{
    ChallengeCapsule, ChallengeManifest, ChallengeMetadata, ResolvedBenchmark,
    ResolvedChallengeCase, collect_context_files, collect_repair_artifacts, looks_like_issue_dir,
    looks_like_proof_full_workspace, looks_like_warpos_staged_workspace, resolve_benchmark,
    resolve_challenge_case,
};
pub use rustbench::{RustbenchMaterialization, maybe_materialize_rustbench_workspace};
pub use scoring::{BenchmarkScoreArtifacts, BenchmarkScoreOptions, score_benchmark_reports};
pub use workspace::{
    copy_dir_all, copy_file_if_different, ensure_git_baseline, rebase_attempt_path,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BenchmarkRunRequest {
    pub resolved: ResolvedBenchmark,
    pub challenge: Option<ChallengeMetadata>,
    pub sandbox: SandboxMode,
    pub result_dir: PathBuf,
    pub max_attempts: usize,
    pub keep_sandbox: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BenchmarkRunReceipt {
    pub benchmark_name: String,
    pub challenge_id: Option<String>,
    pub success: bool,
    pub attempts_run: usize,
    pub proof: ProofReceipt,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_receipt_serializes_with_proof_receipt() {
        let receipt = BenchmarkRunReceipt {
            benchmark_name: "issue-00".to_string(),
            challenge_id: Some("issue-00".to_string()),
            success: true,
            attempts_run: 1,
            proof: ProofReceipt::new("issue-00"),
        };

        let json = serde_json::to_string(&receipt).expect("serialize");

        assert!(json.contains("issue-00"));
        assert!(json.contains("receipt_version"));
    }
}

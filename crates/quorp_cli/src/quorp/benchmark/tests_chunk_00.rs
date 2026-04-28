use super::*;
use quorp_agent_core::{TranscriptMessage, TranscriptRole};
use quorp_benchmark::{
    BatchReport, BenchmarkScoreReport, ChallengeCapsule, ChallengeJudgeOutcome,
    collect_context_files, compile_challenge_capsule, copy_dir_all, ensure_git_baseline,
    evaluator_passed, looks_like_issue_dir, looks_like_proof_full_workspace,
    looks_like_warpos_staged_workspace, run_shell_command, rust_swe_case_profile,
    write_workspace_challenge_command_wrappers,
};
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

fn test_env_guard() -> std::sync::MutexGuard<'static, ()> {
    TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn clear_benchmark_completion_policy_env_overrides() {
    unsafe {
        std::env::remove_var("QUORP_BENCH_FIRST_TURN_MAX_COMPLETION_TOKENS");
        std::env::remove_var("QUORP_BENCH_LATER_TURN_MAX_COMPLETION_TOKENS");
        std::env::remove_var("QUORP_BENCH_DISABLE_REASONING");
        std::env::remove_var("QUORP_BENCH_NATIVE_TOOL_CALLS");
        std::env::remove_var("QUORP_BENCH_PROMPT_COMPACTION_POLICY");
        std::env::remove_var("QUORP_BENCHMARK_SKIP_LOCK");
    }
}


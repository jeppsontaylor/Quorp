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

#[test]
fn detects_proof_full_workspace_path() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    fs::write(temp_dir.path().join("AGENTS.md"), "rules").expect("agents");
    fs::write(temp_dir.path().join("agent-map.json"), "{}").expect("agent-map");
    fs::write(temp_dir.path().join("test-map.json"), "{}").expect("test-map");
    assert!(looks_like_proof_full_workspace(temp_dir.path()));
}

#[test]
fn detects_warpos_staged_workspace_path() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        temp_dir.path().join(".benchmark-root.json"),
        serde_json::json!({
            "benchmark": "atlas-billing",
            "issue": "ISSUE-00-toy",
            "handoff_root": temp_dir.path().display().to_string(),
        })
        .to_string(),
    )
    .expect("marker");
    fs::write(temp_dir.path().join("issue.json"), "{}").expect("issue");
    fs::write(temp_dir.path().join("Cargo.toml"), "[workspace]\n").expect("cargo");
    fs::write(temp_dir.path().join("evaluate.sh"), "#!/usr/bin/env bash\n").expect("eval");
    fs::write(temp_dir.path().join("START_HERE.md"), "# Objective\n").expect("brief");
    assert!(looks_like_warpos_staged_workspace(temp_dir.path()));
}

#[test]
fn detects_issue_directory_path() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let issue_dir = temp_dir.path().join("ISSUE-00-toy");
    fs::create_dir_all(&issue_dir).expect("mkdir");
    fs::write(issue_dir.join("README.md"), "brief").expect("readme");
    assert!(looks_like_issue_dir(&issue_dir));
}

#[test]
fn resolves_warpos_staged_workspace_from_marker() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let benchmarks_root = temp_dir.path().join("benchmarks");
    let issue_id = "ISSUE-00-toy";
    let handoff_root = benchmarks_root
        .join("handoffs")
        .join("atlas-billing")
        .join(issue_id)
        .join("bare");
    let issue_root = benchmarks_root.join("issues").join(issue_id);
    let hidden_dir = issue_root.join("hidden");

    fs::create_dir_all(&handoff_root).expect("handoff root");
    fs::create_dir_all(&hidden_dir).expect("hidden");
    fs::write(hidden_dir.join("check.sh"), "#!/usr/bin/env bash\n").expect("collector");

    let session_workspace = temp_dir.path().join("session").join("workspace");
    fs::create_dir_all(&session_workspace).expect("session workspace");
    fs::write(
        session_workspace.join(".benchmark-root.json"),
        serde_json::json!({
            "benchmark": "atlas-billing",
            "issue": issue_id,
            "condition": "bare",
            "suite": "psd-prod",
            "handoff_root": handoff_root.display().to_string(),
        })
        .to_string(),
    )
    .expect("marker");
    fs::write(session_workspace.join("issue.json"), "{}").expect("issue");
    fs::write(session_workspace.join("Cargo.toml"), "[workspace]\n").expect("cargo");
    fs::write(
        session_workspace.join("evaluate.sh"),
        "#!/usr/bin/env bash\n",
    )
    .expect("eval");
    fs::write(session_workspace.join("START_HERE.md"), "# Objective\n").expect("brief");
    fs::write(
        session_workspace.join("YOU_ARE_HERE.txt"),
        "owner: billing-domain\n",
    )
    .expect("you are here");

    let resolved = resolve_benchmark(&session_workspace).expect("resolved benchmark");
    assert_eq!(resolved.issue_id, issue_id);
    assert_eq!(resolved.benchmark_name, "atlas-billing");
    assert_eq!(
        resolved.workspace_source,
        fs::canonicalize(&session_workspace).expect("canonical workspace")
    );
    assert_eq!(
        resolved.visible_evaluator,
        Some(
            fs::canonicalize(session_workspace.join("evaluate.sh"))
                .expect("canonical visible evaluator"),
        )
    );
    assert_eq!(
        resolved.collector_evaluator,
        Some(fs::canonicalize(hidden_dir.join("check.sh")).expect("canonical collector"))
    );
    assert!(
        resolved.context_files.contains(
            &fs::canonicalize(session_workspace.join(".benchmark-root.json"))
                .expect("canonical benchmark marker")
        )
    );
    assert!(resolved.context_files.contains(
        &fs::canonicalize(session_workspace.join("issue.json")).expect("canonical issue marker")
    ));
}

#[test]
fn widening_detection_flags_multiple_roots() {
    assert!(detect_widening(&[
        "crates/a/src/lib.rs".to_string(),
        "crates/b/src/lib.rs".to_string(),
    ]));
    assert!(!detect_widening(&[
        "crates/a/src/lib.rs".to_string(),
        "crates/a/tests/visible.rs".to_string(),
    ]));
}

#[test]
fn parse_prompt_compaction_policy_accepts_known_values() {
    assert_eq!(
        parse_prompt_compaction_policy(Some("current-default")).expect("parse"),
        Some(PromptCompactionPolicy::CurrentDefault)
    );
    assert_eq!(
        parse_prompt_compaction_policy(Some("last6-ledger768")).expect("parse"),
        Some(PromptCompactionPolicy::Last6Ledger768)
    );
    assert_eq!(
        parse_prompt_compaction_policy(Some("benchmark-repair-minimal")).expect("parse"),
        Some(PromptCompactionPolicy::BenchmarkRepairMinimal)
    );
    assert_eq!(
        parse_prompt_compaction_policy(Some("benchmark-state-packet")).expect("parse"),
        Some(PromptCompactionPolicy::BenchmarkStatePacket)
    );
    assert!(
        parse_prompt_compaction_policy(Some("unknown-policy"))
            .expect_err("invalid policy should fail")
            .to_string()
            .contains("unknown compaction policy")
    );
}

#[test]
fn load_seed_context_reads_latest_checkpoint_messages() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let session_path = temp_dir.path().join("session-0001.json");
    fs::write(
        &session_path,
        serde_json::json!({
            "checkpoints": [
                {
                    "messages": [
                        {"role": "user", "content": "old user"},
                        {"role": "assistant", "content": "old assistant"}
                    ]
                },
                {
                    "messages": [
                        {"role": "system", "content": "seed ledger"},
                        {"role": "assistant", "content": "assistant context"},
                        {"role": "user", "content": "active objective context"},
                        {"role": "assistant", "content": "   "}
                    ]
                }
            ]
        })
        .to_string(),
    )
    .expect("write session");

    let messages = load_seed_context(Some(&session_path)).expect("load seed context");
    assert_eq!(
        messages,
        vec![
            TranscriptMessage {
                role: TranscriptRole::System,
                content: "seed ledger".to_string(),
            },
            TranscriptMessage {
                role: TranscriptRole::Assistant,
                content: "assistant context".to_string(),
            },
            TranscriptMessage {
                role: TranscriptRole::User,
                content: "active objective context".to_string(),
            },
        ]
    );
}

fn create_challenge_case_fixture() -> (tempfile::TempDir, PathBuf) {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let case_root = temp_dir.path().join("01-sample-case");
    fs::create_dir_all(case_root.join("expected")).expect("expected");
    fs::create_dir_all(case_root.join("workspace").join("proof-full").join("src"))
        .expect("workspace");
    fs::write(
        case_root.join("START_HERE.md"),
        "# Objective\n\nFix the sample challenge.\n",
    )
    .expect("objective");
    fs::write(
        case_root.join("LOCAL_REPRO.md"),
        "# Repro\n\n- `cargo test --quiet`\n",
    )
    .expect("repro file");
    fs::write(
        case_root.join("REFERENCE.md"),
        "# Reference\n\n- sample provenance\n",
    )
    .expect("reference");
    fs::write(
        case_root.join("expected").join("success-criteria.md"),
        "# Success\n\nThe sample challenge passes.\n",
    )
    .expect("success");
    fs::write(
        case_root
            .join("workspace")
            .join("proof-full")
            .join("src")
            .join("lib.rs"),
        "pub fn sample() -> u32 { 1 }\n",
    )
    .expect("workspace file");
    fs::write(
        case_root.join("benchmark.json"),
        serde_json::json!({
            "id": "01-sample-case",
            "title": "Sample challenge",
            "difficulty": "easy",
            "category": "sample",
            "repo_condition": ["bare", "proof-core", "proof-full"],
            "objective_file": "START_HERE.md",
            "success_file": "expected/success-criteria.md",
            "reset_command": "./reset.sh <condition>",
            "evaluate_command": "./evaluate.sh <condition>",
            "estimated_minutes": 1,
            "expected_files_touched": ["src/lib.rs"],
            "primary_metrics": ["total_tokens"],
            "tags": ["rust", "sample"],
        })
        .to_string(),
    )
    .expect("benchmark");
    (temp_dir, case_root)
}

fn create_toy_preview_benchmark_fixture() -> (tempfile::TempDir, PathBuf) {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let benchmark_root = temp_dir.path().join("benchmark");
    let issue_dir = benchmark_root
        .join("exhaustive")
        .join("issues")
        .join("ISSUE-00-toy-preview");
    let workspace_dir = benchmark_root
        .join("handoffs")
        .join("proof")
        .join("ISSUE-00-toy-preview")
        .join("proof-full");
    fs::create_dir_all(workspace_dir.join("crates/toy-domain/src")).expect("workspace");
    fs::create_dir_all(issue_dir.join(".hidden")).expect("issue");
    fs::write(
        issue_dir.join("README.md"),
        "# Toy Preview\n\nChange delayed preview behavior to scheduled_at_period_end.\n",
    )
    .expect("readme");
    fs::write(
        issue_dir.join(".hidden").join("evaluate_hidden.sh"),
        r#"#!/usr/bin/env bash
set -euo pipefail
workspace="${1:?workspace}"
grep -q 'scheduled_at_period_end' "$workspace/crates/toy-domain/src/lib.rs"
"#,
    )
    .expect("hidden evaluator");
    fs::write(
        workspace_dir.join("evaluate_visible.sh"),
        r#"#!/usr/bin/env bash
set -euo pipefail
grep -q 'scheduled_at_period_end' crates/toy-domain/src/lib.rs
"#,
    )
    .expect("visible evaluator");
    fs::write(
        workspace_dir.join("START_HERE.md"),
        "# Objective\n\nPatch the toy preview change reason.\n",
    )
    .expect("objective");
    fs::write(
        workspace_dir.join("Cargo.toml"),
        r#"[workspace]
members = ["crates/toy-domain"]
resolver = "2"
"#,
    )
    .expect("workspace cargo manifest");
    fs::write(
        workspace_dir.join("crates/toy-domain/Cargo.toml"),
        r#"[package]
name = "toy-domain"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    )
    .expect("toy cargo manifest");
    fs::write(
            workspace_dir.join("crates/toy-domain/src/lib.rs"),
            "pub fn preview_change_reason(delayed_change: bool) -> &'static str {\n    if delayed_change {\n        \"immediate\"\n    } else {\n        \"immediate\"\n    }\n}\n",
        )
        .expect("toy source");
    for script in [
        issue_dir.join(".hidden").join("evaluate_hidden.sh"),
        workspace_dir.join("evaluate_visible.sh"),
    ] {
        let mut permissions = fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(script, permissions).expect("chmod");
    }
    (temp_dir, issue_dir)
}

fn rust_swebench_top5_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("benchmark")
        .join("challenges")
        .join("rust-swebench-top5")
}

fn rust_swebench_top5_case_roots() -> Vec<PathBuf> {
    discover_challenge_case_roots(&rust_swebench_top5_root()).expect("discover rust cohort")
}

fn copy_case_root_to_temp(case_root: &Path) -> (tempfile::TempDir, PathBuf) {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let copied_root = temp_dir.path().join(
        case_root
            .file_name()
            .expect("case root file name should exist"),
    );
    copy_dir_all(case_root, &copied_root).expect("copy case root");
    (temp_dir, copied_root)
}

fn create_retry_reset_fixture() -> (tempfile::TempDir, BenchmarkManifest, PathBuf) {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let sandbox_root = temp_dir.path().join("sandbox");
    let workspace_dir = sandbox_root.join("workspace").join("proof-full");
    fs::create_dir_all(workspace_dir.join("src")).expect("workspace");
    fs::write(
        workspace_dir.join("Cargo.toml"),
        r#"[package]
name = "retry-reset-fixture"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    )
    .expect("cargo manifest");
    fs::write(
        workspace_dir.join("src").join("lib.rs"),
        "pub fn sample() -> u32 { 1 }\n",
    )
    .expect("workspace file");
    fs::write(
        sandbox_root.join("START_HERE.md"),
        "# Objective\n\nRestore the clean workspace before each attempt.\n",
    )
    .expect("objective");
    fs::write(
        sandbox_root.join("SUCCESS.md"),
        "# Success\n\nThe retry reset restores the workspace baseline.\n",
    )
    .expect("success");
    fs::write(
        sandbox_root.join("reset.sh"),
        r#"#!/usr/bin/env bash
set -euo pipefail

condition="${1:-proof-full}"
workspace="workspace/${condition}"

rm -rf "$workspace/.git" "$workspace/.quorp"
mkdir -p "$workspace/src"
cat <<'EOF' > "$workspace/Cargo.toml"
[package]
name = "retry-reset-fixture"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
EOF
cat <<'EOF' > "$workspace/src/lib.rs"
pub fn sample() -> u32 { 1 }
EOF
"#,
    )
    .expect("reset");
    #[cfg(unix)]
    {
        let permissions = fs::Permissions::from_mode(0o755);
        fs::set_permissions(sandbox_root.join("reset.sh"), permissions)
            .expect("set reset executable");
    }

    let manifest = BenchmarkManifest {
        resolved: ResolvedBenchmark {
            benchmark_root: sandbox_root.clone(),
            issue_id: "retry-reset-fixture".to_string(),
            benchmark_name: "Retry reset fixture".to_string(),
            issue_dir: None,
            workspace_source: workspace_dir.clone(),
            objective_source: sandbox_root.join("START_HERE.md"),
            visible_evaluator: None,
            collector_evaluator: None,
            context_files: Vec::new(),
            repair_artifacts: Vec::new(),
        },
        executor: BenchmarkExecutor::Native,
        model_id: "fixture-model".to_string(),
        safety_mode_label: default_safe_mode_label(),
        scenario_label: None,
        base_url_override: None,
        briefing_file: None,
        compaction_policy: None,
        seed_transcript: None,
        max_steps: 1,
        max_seconds: Some(30),
        max_total_tokens: None,
        autonomy_profile: "autonomous_host".to_string(),
        max_attempts: 2,
        challenge: Some(ChallengeMetadata {
            case_root: sandbox_root.clone(),
            sandbox_root: sandbox_root.clone(),
            workspace_dir: workspace_dir.clone(),
            condition: "proof-full".to_string(),
            objective_file: sandbox_root.join("START_HERE.md"),
            success_file: sandbox_root.join("SUCCESS.md"),
            reference_file: Some(sandbox_root.join("REFERENCE.md")),
            reset_command: "./reset.sh <condition>".to_string(),
            evaluate_command: "cargo test --quiet".to_string(),
            expected_files_touched: vec!["src/lib.rs".to_string()],
            allowed_generated_files: Vec::new(),
            primary_metrics: vec!["evaluate_passed".to_string()],
            tags: vec!["rust".to_string(), "fixture".to_string()],
            capsule_file: workspace_dir.join(".quorp").join("challenge-capsule.json"),
            capsule: ChallengeCapsule::default(),
        }),
        keep_sandbox: true,
        completion_policy: quorp_agent_core::CompletionPolicy::default(),
    };

    (temp_dir, manifest, workspace_dir)
}

fn apply_patch_in_workspace(workspace_root: &Path, patch_path: &Path, reverse: bool) -> bool {
    let mut command = Command::new("git");
    command.arg("-C").arg(workspace_root).arg("apply");
    if reverse {
        command.arg("-R");
    }
    let status = command
        .arg("--whitespace=nowarn")
        .arg(patch_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("apply patch");
    status.success()
}

fn workspace_probe_path(case_root: &Path) -> PathBuf {
    let workspace_root = case_root.join("workspace").join("proof-full");
    let cargo_manifest = workspace_root.join("Cargo.toml");
    if cargo_manifest.exists() {
        return cargo_manifest;
    }

    let mut stack = vec![workspace_root.clone()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).expect("read dir");
        for entry in entries {
            let entry = entry.expect("entry");
            let path = entry.path();
            let file_type = entry.file_type().expect("file type");
            if file_type.is_dir() {
                stack.push(path);
            } else {
                return path;
            }
        }
    }

    panic!("no workspace file found under {}", workspace_root.display());
}

#[test]
fn challenge_resolution_accepts_case_root_objective_and_workspace_paths() {
    let (_temp_dir, case_root) = create_challenge_case_fixture();
    let expected_objective =
        fs::canonicalize(case_root.join("START_HERE.md")).expect("canonical objective");

    let resolved_from_root = resolve_challenge_case(&case_root, None)
        .expect("resolve from case root")
        .expect("challenge case");
    assert_eq!(resolved_from_root.condition, "proof-full");
    assert_eq!(resolved_from_root.objective_source, expected_objective);

    let resolved_from_objective = resolve_challenge_case(&case_root.join("START_HERE.md"), None)
        .expect("resolve from objective")
        .expect("challenge case");
    assert_eq!(resolved_from_objective.condition, "proof-full");
    assert_eq!(resolved_from_objective.objective_source, expected_objective);

    let resolved_from_workspace = resolve_challenge_case(
        &case_root
            .join("workspace")
            .join("proof-full")
            .join("src")
            .join("lib.rs"),
        Some("bare"),
    )
    .expect("resolve from workspace path")
    .expect("challenge case");
    assert_eq!(resolved_from_workspace.condition, "bare");
    assert_eq!(resolved_from_workspace.objective_source, expected_objective);
}

#[test]
fn challenge_resolution_rejects_mismatched_objective_markdown() {
    let (_temp_dir, case_root) = create_challenge_case_fixture();
    fs::write(case_root.join("README.md"), "alternate brief").expect("readme");
    let error = resolve_challenge_case(&case_root.join("README.md"), None)
        .expect_err("mismatched markdown should be rejected");
    assert!(
        error
            .to_string()
            .contains("does not match the declared objective file")
    );
}

#[test]
fn challenge_case_discovery_finds_case_roots() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    for case_name in ["01-a", "02-b", "03-c", "04-d"] {
        let case_root = temp_dir.path().join(case_name);
        fs::create_dir_all(&case_root).expect("case dir");
        fs::write(case_root.join("benchmark.json"), "{}").expect("benchmark");
    }
    let case_roots = discover_challenge_case_roots(temp_dir.path()).expect("discover cases");
    assert_eq!(case_roots.len(), 4);
    assert!(case_roots.iter().any(|path| path.ends_with("01-a")));
    assert!(case_roots.iter().any(|path| path.ends_with("04-d")));
}

#[test]
fn rust_swebench_top5_structure_and_resolution() {
    let case_roots = rust_swebench_top5_case_roots();
    assert_eq!(case_roots.len(), 5);

    for case_root in case_roots {
        for relative in [
            "benchmark.json",
            "START_HERE.md",
            "SUCCESS.md",
            "REFERENCE.md",
            "reset.sh",
            "evaluate.sh",
            "upstream/metadata.json",
            "upstream/problem_statement.md",
            "upstream/fix.patch",
            "upstream/test.patch",
        ] {
            assert!(
                case_root.join(relative).exists(),
                "missing `{relative}` for {}",
                case_root.display()
            );
        }

        let manifest_path = case_root.join("benchmark.json");
        let manifest: ChallengeManifest =
            serde_json::from_str(&fs::read_to_string(&manifest_path).expect("read manifest"))
                .expect("parse challenge manifest");
        assert_eq!(manifest.repo_condition, vec!["proof-full".to_string()]);
        assert!(!manifest.expected_files_touched.is_empty());

        let resolved_from_root = resolve_challenge_case(&case_root, None)
            .expect("resolve from case root")
            .expect("challenge case");
        assert_eq!(resolved_from_root.condition, "proof-full");

        let resolved_from_objective =
            resolve_challenge_case(&case_root.join("START_HERE.md"), None)
                .expect("resolve from objective")
                .expect("challenge case");
        assert_eq!(resolved_from_objective.condition, "proof-full");

        let workspace_root = case_root.join("workspace").join("proof-full");
        if !workspace_root.exists() {
            eprintln!(
                "skipping optional unpacked workspace checks for {}",
                case_root.display()
            );
            continue;
        }

        for relative in [
            "AGENTS.md",
            "agent-map.json",
            "test-map.json",
            ".witness/witness-graph.json",
        ] {
            assert!(
                workspace_root.join(relative).exists(),
                "missing workspace fixture `{relative}` in {}",
                workspace_root.display()
            );
        }

        let probe_path = workspace_probe_path(&case_root);
        let resolved_from_workspace = resolve_challenge_case(&probe_path, None)
            .expect("resolve from workspace path")
            .expect("challenge case");
        assert_eq!(resolved_from_workspace.condition, "proof-full");

        assert!(
            !workspace_root.join("target").exists(),
            "vendored cargo target should not exist in {}",
            workspace_root.display()
        );
        for expected in &manifest.expected_files_touched {
            assert!(
                workspace_root.join(expected).exists(),
                "missing expected touch target `{expected}` in {}",
                workspace_root.display()
            );
        }
    }
}

#[test]
#[ignore = "expensive real benchmark validation"]
fn rust_swebench_top5_gold_patch_validation() {
    for case_root in rust_swebench_top5_case_roots() {
        let (_temp_dir, copied_root) = copy_case_root_to_temp(&case_root);
        let manifest: ChallengeManifest = serde_json::from_str(
            &fs::read_to_string(copied_root.join("benchmark.json")).expect("read manifest"),
        )
        .expect("parse manifest");

        let reset = run_shell_command(
            "reset",
            "./reset.sh proof-full",
            &copied_root.join("reset.sh"),
            &copied_root,
        )
        .expect("reset challenge workspace");
        assert!(reset.passed, "reset failed for {}", copied_root.display());

        let baseline = run_shell_command(
            "evaluation",
            "./evaluate.sh proof-full",
            &copied_root.join("evaluate.sh"),
            &copied_root,
        )
        .expect("run baseline evaluation");
        assert!(
            !baseline.passed,
            "baseline unexpectedly passed for {}",
            copied_root.display()
        );

        let workspace_root = copied_root.join("workspace").join("proof-full");
        for expected in &manifest.expected_files_touched {
            assert!(
                workspace_root.join(expected).exists(),
                "missing expected touch target `{expected}` in {}",
                workspace_root.display()
            );
        }

        assert!(
            apply_patch_in_workspace(
                &workspace_root,
                &copied_root.join("upstream").join("fix.patch"),
                false,
            ),
            "fix patch failed to apply for {}",
            copied_root.display()
        );

        let gold = run_shell_command(
            "evaluation",
            "./evaluate.sh proof-full",
            &copied_root.join("evaluate.sh"),
            &copied_root,
        )
        .expect("run gold evaluation");
        assert!(
            gold.passed,
            "gold patch failed for {}: stdout={} stderr={}",
            copied_root.display(),
            gold.stdout,
            gold.stderr
        );

        let (_replay_temp_dir, replay_root) = copy_case_root_to_temp(&case_root);
        let replay_reset = run_shell_command(
            "reset",
            "./reset.sh proof-full",
            &replay_root.join("reset.sh"),
            &replay_root,
        )
        .expect("reset replay workspace");
        assert!(
            replay_reset.passed,
            "replay reset failed for {}",
            replay_root.display()
        );

        let replay_workspace = replay_root.join("workspace").join("proof-full");
        assert!(
            apply_patch_in_workspace(
                &replay_workspace,
                &replay_root.join("upstream").join("test.patch"),
                true,
            ),
            "reverse test patch failed for {}",
            replay_root.display()
        );
        assert!(
            apply_patch_in_workspace(
                &replay_workspace,
                &replay_root.join("upstream").join("test.patch"),
                false,
            ),
            "test patch failed to apply for {}",
            replay_root.display()
        );
        assert!(
            apply_patch_in_workspace(
                &replay_workspace,
                &replay_root.join("upstream").join("fix.patch"),
                false,
            ),
            "fix patch replay failed for {}",
            replay_root.display()
        );

        let replay_gold = run_shell_command(
            "evaluation",
            "./evaluate.sh proof-full",
            &replay_root.join("evaluate.sh"),
            &replay_root,
        )
        .expect("run replay gold evaluation");
        assert!(
            replay_gold.passed,
            "replayed test+fix patch failed for {}: stdout={} stderr={}",
            replay_root.display(),
            replay_gold.stdout,
            replay_gold.stderr
        );
    }
}

#[test]
fn rust_swebench_retry_reset_restores_clean_workspace() {
    let (_temp_dir, manifest, workspace_dir) = create_retry_reset_fixture();
    let first_attempt =
        reset_challenge_workspace_for_attempt(&manifest, 1).expect("attempt one should not fail");
    assert!(first_attempt.is_none());
    let second_attempt = reset_challenge_workspace_for_attempt(&manifest, 2)
        .expect("attempt two reset")
        .expect("attempt two should run reset");
    assert!(second_attempt.passed, "initial reset should succeed");
    let baseline_status = Command::new("git")
        .arg("status")
        .arg("--porcelain")
        .current_dir(&workspace_dir)
        .output()
        .expect("git status after initial reset");
    assert!(
        baseline_status.status.success(),
        "initial git status should succeed"
    );
    let baseline_status_stdout = String::from_utf8_lossy(&baseline_status.stdout).to_string();
    assert!(
        workspace_dir.join(".quorp").join("agent.toml").exists(),
        "initial reset should recreate the agent config"
    );

    fs::write(
        workspace_dir.join("src").join("lib.rs"),
        "pub fn sample() -> u32 { 99 }\n",
    )
    .expect("mutate workspace");
    fs::create_dir_all(workspace_dir.join(".quorp")).expect("seed .quorp directory");
    fs::write(workspace_dir.join(".quorp").join("stale.txt"), "stale").expect("seed stale config");

    let third_attempt = reset_challenge_workspace_for_attempt(&manifest, 3)
        .expect("attempt three reset")
        .expect("attempt three should run reset");
    assert!(third_attempt.passed, "reset should succeed");
    assert_eq!(
        fs::read_to_string(workspace_dir.join("src").join("lib.rs")).expect("read restored file"),
        "pub fn sample() -> u32 { 1 }\n"
    );
    assert!(
        workspace_dir.join(".git").exists(),
        "git baseline should be restored"
    );
    let agent_config = workspace_dir.join(".quorp").join("agent.toml");
    assert!(
        agent_config.exists(),
        "agent config should be rewritten after reset"
    );
    assert!(
        fs::read_to_string(&agent_config)
            .expect("read agent config")
            .contains("[defaults]"),
        "agent config should contain benchmark defaults"
    );
    let capsule_file = workspace_dir.join(".quorp").join("challenge-capsule.json");
    assert!(
        capsule_file.exists(),
        "challenge capsule should be rewritten after reset"
    );

    let status = Command::new("git")
        .arg("status")
        .arg("--porcelain")
        .current_dir(&workspace_dir)
        .output()
        .expect("git status");
    assert!(status.status.success(), "git status should succeed");
    assert!(
        String::from_utf8_lossy(&status.stdout) == baseline_status_stdout,
        "workspace should match the initial attempt state after reset"
    );
}

#[test]
fn judge_response_parser_accepts_strict_json() {
    let parsed = parse_challenge_judge_response(
        r#"{"passed":true,"summary":"looks good","rationale":"objective was satisfied"}"#,
    )
    .expect("parse judge");
    assert!(parsed.passed);
    assert_eq!(parsed.summary, "looks good");
    assert_eq!(parsed.rationale, "objective was satisfied");
}

#[test]
fn batch_report_sums_case_metrics() {
    let report = summarize_batch_report(
        PathBuf::from("/tmp/cases"),
        PathBuf::from("/tmp/results"),
        vec![
            BatchCaseReport {
                case_id: "case-a".to_string(),
                case_root: PathBuf::from("/tmp/cases/case-a"),
                objective_path: PathBuf::from("/tmp/cases/case-a/START_HERE.md"),
                result_dir: PathBuf::from("/tmp/results/case-a"),
                log_file: PathBuf::from("/tmp/results/logs/case-a.log"),
                executor: BenchmarkExecutor::Native,
                success: true,
                exit_code: 0,
                wall_clock_ms: 100,
                total_requests: 3,
                total_billed_tokens: 12,
                lines_added: 4,
                lines_removed: 1,
                mistakes_corrected: 1,
                judge_passed: Some(true),
                deterministic_evaluation_passed: Some(true),
                first_request_prompt_token_estimate: Some(1200),
                first_request_raw_prompt_token_estimate: Some(1200),
                first_request_compacted_prompt_token_estimate: Some(700),
                first_request_first_token_latency_ms: Some(800),
                first_model_turn_started: true,
                first_action_emitted: true,
                final_stop_reason: Some(quorp_agent_core::StopReason::Success),
                primary_failure: None,
                agent_final_failure_classification: Some("success".to_string()),
                adaptive_action_mode_retry: false,
                report_path: PathBuf::from("/tmp/results/case-a/benchmark-report.json"),
                error: None,
            },
            BatchCaseReport {
                case_id: "case-b".to_string(),
                case_root: PathBuf::from("/tmp/cases/case-b"),
                objective_path: PathBuf::from("/tmp/cases/case-b/START_HERE.md"),
                result_dir: PathBuf::from("/tmp/results/case-b"),
                log_file: PathBuf::from("/tmp/results/logs/case-b.log"),
                executor: BenchmarkExecutor::Native,
                success: false,
                exit_code: 1,
                wall_clock_ms: 200,
                total_requests: 2,
                total_billed_tokens: 8,
                lines_added: 2,
                lines_removed: 3,
                mistakes_corrected: 0,
                judge_passed: Some(false),
                deterministic_evaluation_passed: Some(false),
                first_request_prompt_token_estimate: Some(1400),
                first_request_raw_prompt_token_estimate: Some(1400),
                first_request_compacted_prompt_token_estimate: None,
                first_request_first_token_latency_ms: Some(900),
                first_model_turn_started: false,
                first_action_emitted: false,
                final_stop_reason: Some(quorp_agent_core::StopReason::FatalError),
                primary_failure: Some("agent_fatal_error".to_string()),
                agent_final_failure_classification: Some("parser_tool_schema".to_string()),
                adaptive_action_mode_retry: false,
                report_path: PathBuf::from("/tmp/results/case-b/benchmark-report.json"),
                error: Some("failed".to_string()),
            },
        ],
    );
    assert_eq!(report.total_requests, 5);
    assert_eq!(report.total_billed_tokens, 20);
    assert_eq!(report.lines_added, 6);
    assert_eq!(report.lines_removed, 4);
    assert_eq!(report.mistakes_corrected, 1);
    assert_eq!(report.successful_cases, 1);
    assert_eq!(report.failed_cases, 1);
}

#[test]
fn synthetic_failure_report_marks_launch_failures() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let case_manifest = ChallengeManifest {
        id: "01-sample-case".to_string(),
        title: "Sample challenge".to_string(),
        difficulty: "easy".to_string(),
        category: "sample".to_string(),
        repo_condition: vec!["proof-full".to_string()],
        objective_file: "START_HERE.md".to_string(),
        success_file: "expected/success-criteria.md".to_string(),
        reset_command: "./reset.sh <condition>".to_string(),
        evaluate_command: "./evaluate.sh <condition>".to_string(),
        estimated_minutes: Some(1),
        expected_files_touched: Vec::new(),
        allowed_generated_files: Vec::new(),
        primary_metrics: Vec::new(),
        tags: Vec::new(),
    };

    write_synthetic_failure_report(
        &case_manifest,
        temp_dir.path(),
        BenchmarkExecutor::Native,
        crate::quorp::provider_config::NVIDIA_QWEN_MODEL,
        3,
        "runtime never became ready".to_string(),
        None,
    )
    .expect("write synthetic report");

    let report: BenchmarkReport = serde_json::from_str(
        &fs::read_to_string(temp_dir.path().join("benchmark-report.json")).expect("read report"),
    )
    .expect("parse report");
    assert!(!report.success);
    assert_eq!(report.primary_failure.as_deref(), Some("launch_failed"));
    assert_eq!(
        report.run_error.as_deref(),
        Some("runtime never became ready")
    );
}

#[test]
fn challenge_setup_failure_writes_benchmark_report() {
    let (_temp_dir, case_root) = create_challenge_case_fixture();
    fs::write(
            case_root.join("reset.sh"),
            "#!/usr/bin/env bash\nset -euo pipefail\ncondition=\"$1\"\nrm -rf \"workspace/${condition}\"\n",
        )
        .expect("reset script");
    let mut permissions = fs::metadata(case_root.join("reset.sh"))
        .expect("reset metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(case_root.join("reset.sh"), permissions).expect("chmod reset");
    let challenge = resolve_challenge_case(&case_root.join("START_HERE.md"), Some("proof-full"))
        .expect("resolve challenge")
        .expect("challenge case");
    let result_dir = tempfile::tempdir().expect("result dir");
    let options = BenchmarkRunOptions {
        path: case_root.join("START_HERE.md"),
        executor: BenchmarkExecutor::Native,
        model_id: Some("test-model".to_string()),
        base_url_override: None,
        briefing_file: None,
        compaction_policy: None,
        seed_transcript: None,
        max_steps: 1,
        max_seconds: Some(1),
        max_total_tokens: None,
        result_dir: result_dir.path().to_path_buf(),
        autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousSandboxed,
        max_attempts: Some(1),
        condition: Some("proof-full".to_string()),
        keep_sandbox: true,
    };

    let error = run_challenge_benchmark(&options, challenge).expect_err("setup failure");
    assert!(error.to_string().contains("layout_resolution_failed"));

    let report: BenchmarkReport = serde_json::from_str(
        &fs::read_to_string(result_dir.path().join("benchmark-report.json")).expect("read report"),
    )
    .expect("parse report");
    assert!(!report.success);
    assert_eq!(
        report.setup_failure_class.as_deref(),
        Some("layout_resolution_failed")
    );
    assert_eq!(
        report.primary_failure.as_deref(),
        Some("layout_resolution_failed")
    );
}

#[test]
fn bootstrap_tracker_records_progress_and_first_task_request() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let attempt_dir = temp_dir.path().join("attempt-001");
    fs::create_dir_all(&attempt_dir).expect("attempt dir");

    let tracker =
        BenchmarkBootstrapTracker::new(temp_dir.path(), &attempt_dir, 1).expect("create tracker");
    tracker
        .update(
            BOOTSTRAP_PHASE_CONTROL_LOOP_STARTED,
            Some("benchmark control loop entered".to_string()),
        )
        .expect("update phase");
    tracker
        .mark_first_task_model_request()
        .expect("mark first request");

    let progress = read_bootstrap_progress(&attempt_bootstrap_progress_path(&attempt_dir))
        .expect("read progress")
        .expect("progress exists");
    assert_eq!(
        progress.bootstrap_phase,
        BOOTSTRAP_PHASE_FIRST_TASK_MODEL_REQUEST
    );
    assert!(progress.first_task_model_request_seen);
    assert!(
        progress
            .bootstrap_elapsed_ms_before_first_task_request
            .is_some()
    );
}

#[test]
fn write_report_preserves_pre_model_bootstrap_stall_fields() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let workspace_dir = temp_dir.path().join("workspace");
    let attempt_dir = temp_dir.path().join("attempt-001");
    let agent_result_dir = attempt_dir.join("agent");
    fs::create_dir_all(&workspace_dir).expect("workspace");
    fs::create_dir_all(&agent_result_dir).expect("agent");

    let manifest = BenchmarkManifest {
        resolved: ResolvedBenchmark {
            benchmark_root: temp_dir.path().join("benchmark-root"),
            issue_id: "06-rust-swebench-bincode-serde-decoder-memory".to_string(),
            benchmark_name: "Bootstrap stall case".to_string(),
            issue_dir: None,
            workspace_source: workspace_dir.clone(),
            objective_source: workspace_dir.join("START_HERE.md"),
            visible_evaluator: None,
            collector_evaluator: None,
            context_files: Vec::new(),
            repair_artifacts: Vec::new(),
        },
        executor: BenchmarkExecutor::Native,
        model_id: "nvidia/qwen/qwen3-coder-480b-a35b-instruct".to_string(),
        safety_mode_label: "remote_api".to_string(),
        scenario_label: Some("QuorpRemote".to_string()),
        base_url_override: Some("http://127.0.0.1:49919".to_string()),
        briefing_file: None,
        compaction_policy: None,
        seed_transcript: None,
        max_steps: 8,
        max_seconds: Some(120),
        max_total_tokens: Some(5_000),
        autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousHost
            .label()
            .to_string(),
        max_attempts: 1,
        challenge: None,
        keep_sandbox: false,
        completion_policy: benchmark_completion_policy(
            BenchmarkExecutor::Native,
            "remote_api",
            Some("nvidia/qwen/qwen3-coder-480b-a35b-instruct"),
        ),
    };
    let progress = BenchmarkBootstrapProgress {
        attempt: 1,
        bootstrap_phase: BOOTSTRAP_PHASE_CONTROL_LOOP_STARTED.to_string(),
        bootstrap_phase_detail: Some(
            "benchmark control loop started but never reached the model".to_string(),
        ),
        started_at_epoch_ms: 1,
        updated_at_epoch_ms: 2,
        first_task_model_request_seen: false,
        bootstrap_elapsed_ms_before_first_task_request: None,
        pre_model_bootstrap_stalled: true,
        bootstrap_stall_class: Some(BOOTSTRAP_STALL_CLASS_PRE_MODEL.to_string()),
    };
    let attempt = attempt_report_for_bootstrap_stall(
        &manifest,
        1,
        &attempt_dir,
        &workspace_dir,
        &agent_result_dir,
        &progress,
    );

    write_report(temp_dir.path(), &manifest, &[attempt], None, None).expect("write report");

    let report: BenchmarkReport = serde_json::from_str(
        &fs::read_to_string(temp_dir.path().join("benchmark-report.json")).expect("read report"),
    )
    .expect("parse report");
    assert!(report.pre_model_bootstrap_stalled);
    assert_eq!(
        report.bootstrap_phase.as_deref(),
        Some(BOOTSTRAP_PHASE_CONTROL_LOOP_STARTED)
    );
    assert!(!report.first_task_model_request_seen);
    assert_eq!(
        report.primary_failure.as_deref(),
        Some(BOOTSTRAP_STALL_CLASS_PRE_MODEL)
    );
}

#[test]
fn partial_batch_summary_is_persisted() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let options = BenchmarkBatchRunOptions {
        cases_root: PathBuf::from("/tmp/cases"),
        result_dir: temp_dir.path().to_path_buf(),
        executor: BenchmarkExecutor::Native,
        model_id: None,
        base_url_override: None,
        briefing_file: None,
        compaction_policy: None,
        seed_transcript: None,
        max_steps: 8,
        max_seconds: Some(60),
        max_total_tokens: Some(1000),
        max_attempts: Some(2),
        autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousSandboxed,
        condition: None,
        keep_sandbox: false,
        log_dir: None,
    };
    let cases = vec![BatchCaseReport {
        case_id: "case-a".to_string(),
        case_root: PathBuf::from("/tmp/cases/case-a"),
        objective_path: PathBuf::from("/tmp/cases/case-a/START_HERE.md"),
        result_dir: PathBuf::from("/tmp/results/case-a"),
        log_file: PathBuf::from("/tmp/results/logs/case-a.log"),
        executor: BenchmarkExecutor::Native,
        success: false,
        exit_code: 1,
        wall_clock_ms: 77,
        total_requests: 1,
        total_billed_tokens: 42,
        lines_added: 0,
        lines_removed: 0,
        mistakes_corrected: 0,
        judge_passed: None,
        deterministic_evaluation_passed: None,
        first_request_prompt_token_estimate: None,
        first_request_raw_prompt_token_estimate: None,
        first_request_compacted_prompt_token_estimate: None,
        first_request_first_token_latency_ms: None,
        first_model_turn_started: false,
        first_action_emitted: false,
        final_stop_reason: Some(quorp_agent_core::StopReason::FatalError),
        primary_failure: Some("agent_fatal_error".to_string()),
        agent_final_failure_classification: Some("parser_tool_schema".to_string()),
        adaptive_action_mode_retry: false,
        report_path: PathBuf::from("/tmp/results/case-a/benchmark-report.json"),
        error: Some("fatal".to_string()),
    }];

    write_batch_summary_artifacts(&options, &cases, 123).expect("write partial batch summary");

    let report: BatchReport = serde_json::from_str(
        &fs::read_to_string(temp_dir.path().join("batch-report.json")).expect("read report"),
    )
    .expect("parse report");
    assert_eq!(report.cases.len(), 1);
    assert_eq!(report.total_billed_tokens, 42);
    let rendered =
        fs::read_to_string(temp_dir.path().join("batch-report.md")).expect("read markdown");
    assert!(rendered.contains("failure=agent_fatal_error"));
    let run_summary =
        fs::read_to_string(temp_dir.path().join("run-summary.md")).expect("read summary");
    assert!(run_summary.contains("agent=parser_tool_schema"));
}

#[test]
fn score_benchmark_reports_writes_session_scoreboard() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let run_dir = temp_dir.path().join("run");
    let case_a_dir = run_dir.join("01-case-a");
    let case_b_dir = run_dir.join("02-case-b");
    fs::create_dir_all(&case_a_dir).expect("case a dir");
    fs::create_dir_all(&case_b_dir).expect("case b dir");
    let case_a_report = case_a_dir.join("benchmark-report.json");
    let case_b_report = case_b_dir.join("benchmark-report.json");
    write_json(
        &case_a_report,
        &serde_json::json!({
            "benchmark_name": "Case A",
            "issue_id": "01-case-a",
            "model_id": "nvidia/qwen/qwen3-coder-480b-a35b-instruct",
            "success": false,
            "attempts_run": 1,
            "max_attempts": 1,
            "total_billed_tokens": 100,
            "changed_files": ["Cargo.toml"],
            "widening_happened": false,
            "attempts": [],
            "run_dir": case_a_dir,
            "wall_clock_ms": 10,
            "total_requests": 2,
            "write_count": 1,
            "lines_added": 1,
            "lines_removed": 0,
            "deterministic_evaluation_passed": true,
            "first_model_turn_started": true,
            "first_action_emitted": true,
            "fast_loop_command_seen": true,
            "validation_commands_run": 2,
            "evaluation_commands_run": 1,
            "post_fast_loop_validation_rerun_attempted": true,
            "watchdog_near_limit": true,
            "first_request_first_token_latency_ms": 30000,
            "agent_final_failure_classification": "model_edit_strategy",
            "agent_repair_scorecard": {
                "first_valid_write_step": 4,
                "modify_toml_count": 1
            }
        }),
    )
    .expect("write case a report");
    write_json(
        &case_b_report,
        &serde_json::json!({
            "benchmark_name": "Case B",
            "issue_id": "02-case-b",
            "model_id": "nvidia/qwen/qwen3-coder-480b-a35b-instruct",
            "success": false,
            "attempts_run": 1,
            "max_attempts": 1,
            "total_billed_tokens": 50,
            "widening_happened": false,
            "attempts": [],
            "run_dir": case_b_dir,
            "wall_clock_ms": 20,
            "total_requests": 1,
            "write_count": 2,
            "lines_added": 80,
            "lines_removed": 50,
            "changed_files": ["src/a.rs", "src/b.rs"],
            "first_request_first_token_latency_ms": 500,
            "first_model_turn_started": true,
            "first_action_emitted": true,
            "primary_failure": "agent_fatal_error",
            "agent_final_failure_classification": "parser_tool_schema",
            "agent_repair_scorecard": {
                "parser_recovery_count": 2
            }
        }),
    )
    .expect("write case b report");
    let batch = summarize_batch_report(
        PathBuf::from("/tmp/rust-swebench-top5"),
        run_dir.clone(),
        vec![
            BatchCaseReport {
                case_id: "01-case-a".to_string(),
                case_root: PathBuf::from("/tmp/rust-swebench-top5/01-case-a"),
                objective_path: PathBuf::from("/tmp/rust-swebench-top5/01-case-a/START_HERE.md"),
                result_dir: case_a_dir.clone(),
                log_file: run_dir.join("logs/01-case-a.log"),
                executor: BenchmarkExecutor::Native,
                success: false,
                exit_code: 1,
                wall_clock_ms: 10,
                total_requests: 2,
                total_billed_tokens: 100,
                lines_added: 1,
                lines_removed: 0,
                mistakes_corrected: 0,
                judge_passed: None,
                deterministic_evaluation_passed: Some(true),
                first_request_prompt_token_estimate: None,
                first_request_raw_prompt_token_estimate: None,
                first_request_compacted_prompt_token_estimate: None,
                first_request_first_token_latency_ms: Some(30000),
                first_model_turn_started: true,
                first_action_emitted: true,
                final_stop_reason: Some(quorp_agent_core::StopReason::FatalError),
                primary_failure: Some("agent_fatal_error".to_string()),
                agent_final_failure_classification: Some("model_edit_strategy".to_string()),
                adaptive_action_mode_retry: false,
                report_path: case_a_report,
                error: None,
            },
            BatchCaseReport {
                case_id: "02-case-b".to_string(),
                case_root: PathBuf::from("/tmp/rust-swebench-top5/02-case-b"),
                objective_path: PathBuf::from("/tmp/rust-swebench-top5/02-case-b/START_HERE.md"),
                result_dir: case_b_dir,
                log_file: run_dir.join("logs/02-case-b.log"),
                executor: BenchmarkExecutor::Native,
                success: false,
                exit_code: 1,
                wall_clock_ms: 20,
                total_requests: 1,
                total_billed_tokens: 50,
                lines_added: 0,
                lines_removed: 0,
                mistakes_corrected: 0,
                judge_passed: None,
                deterministic_evaluation_passed: None,
                first_request_prompt_token_estimate: None,
                first_request_raw_prompt_token_estimate: None,
                first_request_compacted_prompt_token_estimate: None,
                first_request_first_token_latency_ms: Some(500),
                first_model_turn_started: true,
                first_action_emitted: true,
                final_stop_reason: Some(quorp_agent_core::StopReason::FatalError),
                primary_failure: Some("agent_fatal_error".to_string()),
                agent_final_failure_classification: Some("parser_tool_schema".to_string()),
                adaptive_action_mode_retry: true,
                report_path: case_b_report,
                error: Some("fatal".to_string()),
            },
        ],
    );
    write_json(&run_dir.join("batch-report.json"), &batch).expect("write batch report");

    let output_root = temp_dir.path().join("scoreboards");
    let artifacts = score_benchmark_reports(BenchmarkScoreOptions {
        run_dirs: vec![run_dir],
        suite: "rust-swebench-top5".to_string(),
        reports_root: temp_dir.path().join("reports"),
        output_root: Some(output_root.clone()),
        fail_on_regression: false,
    })
    .expect("score reports");

    assert!(artifacts.markdown.contains("Solved score: `0/2`"));
    assert!(artifacts.markdown.contains("SecureETTS tokens: `0`"));
    assert!(
        artifacts
            .markdown
            .contains("Total patch size: `1 changed lines`")
    );
    assert!(artifacts.markdown.contains("Total retries: `1`"));
    assert!(artifacts.markdown.contains("Slow first-token cases: `1`"));
    assert!(
        artifacts
            .markdown
            .contains("Watchdog near-limit cases: `1`")
    );
    assert!(artifacts.markdown.contains("Patch quality risk cases: `0`"));
    assert!(artifacts.markdown.contains("first_token_ms=30000"));
    assert!(artifacts.markdown.contains("near_limit=true"));
    assert!(artifacts.markdown.contains("## Proof Lanes"));
    assert!(
        artifacts
            .markdown
            .contains("Valid implementation writes: `1/2`")
    );
    assert!(artifacts.markdown.contains("Post-write validation: `1/2`"));
    assert!(artifacts.output_dir.join("scoreboard.json").exists());
    assert!(output_root.join("latest.md").exists());
    let score: BenchmarkScoreReport = serde_json::from_str(
        &fs::read_to_string(output_root.join("latest.json")).expect("read latest score"),
    )
    .expect("parse score");
    assert_eq!(score.valid_write_cases, 1);
    assert_eq!(score.post_write_validation_cases, 1);
    assert_eq!(score.total_wall_clock_ms, 30);
    assert_eq!(score.median_wall_clock_ms, 20);
    assert_eq!(score.total_patch_lines_changed, 1);
    assert_eq!(score.total_retries, 1);
    assert_eq!(score.slow_first_token_cases, 1);
    assert_eq!(score.watchdog_near_limit_cases, 1);
    assert_eq!(score.patch_quality_risk_cases, 0);
    assert_eq!(score.proof_lane_counts.get("fast"), Some(&1));
    assert_eq!(score.proof_lane_counts.get("medium"), Some(&1));
    assert_eq!(score.proof_lane_counts.get("evaluation"), Some(&1));
    assert_eq!(score.proof_lane_counts.get("deterministic"), Some(&1));
    assert_eq!(score.blocker_counts.get("parser_tool_schema"), Some(&1));
}

#[test]
fn git_numstat_counts_added_and_removed_lines() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let workspace = temp_dir.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::write(workspace.join("sample.txt"), "alpha\nbeta\ngamma\n").expect("baseline file");
    ensure_git_baseline(&workspace).expect("baseline git repo");
    fs::write(
        workspace.join("sample.txt"),
        "alpha\nbeta updated\ngamma\ndelta\n",
    )
    .expect("modified file");

    let (lines_added, lines_removed) = git_numstat(&workspace).expect("git numstat");
    assert_eq!(lines_added, 2);
    assert_eq!(lines_removed, 1);
}

#[test]
fn reportable_changed_files_ignore_target_artifacts() {
    assert!(is_reportable_changed_file("crates/toy-domain/src/lib.rs"));
    assert!(!is_reportable_changed_file("target/.rustc_info.json"));
    assert!(!is_reportable_changed_file(".quorp/challenge-capsule.json"));
    assert!(!is_reportable_changed_file(
        ".warpos-capture-probe/events.jsonl"
    ));
    assert!(is_support_or_generated_changed_file("START_HERE.md"));
    assert!(is_support_or_generated_changed_file(
        "benchmark-report.json"
    ));
    assert!(!is_support_or_generated_changed_file("src/lib.rs"));
}

#[test]
fn challenge_ignored_changed_files_exclude_benchmark_support_files() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let workspace_dir = temp_dir.path().join("workspace");
    let quorp_dir = workspace_dir.join(".quorp");
    fs::create_dir_all(&quorp_dir).expect("mkdir");
    let objective_file = workspace_dir.join("START_HERE.md");
    let success_file = workspace_dir.join("SUCCESS.md");
    let reference_file = workspace_dir.join("REFERENCE.md");
    let benchmark_manifest = workspace_dir.join("benchmark.json");
    let capsule_file = quorp_dir.join("challenge-capsule.json");
    for path in [
        &objective_file,
        &success_file,
        &reference_file,
        &benchmark_manifest,
        &capsule_file,
    ] {
        fs::write(path, "placeholder").expect("write support file");
    }
    let metadata = ChallengeMetadata {
        case_root: temp_dir.path().join("case"),
        sandbox_root: temp_dir.path().join("sandbox"),
        workspace_dir: workspace_dir.clone(),
        condition: "proof-full".to_string(),
        objective_file,
        success_file,
        reference_file: Some(reference_file),
        reset_command: "./reset.sh proof-full".to_string(),
        evaluate_command: "./evaluate.sh proof-full".to_string(),
        expected_files_touched: vec!["src/lib.rs".to_string()],
        allowed_generated_files: Vec::new(),
        primary_metrics: Vec::new(),
        tags: Vec::new(),
        capsule_file,
        capsule: ChallengeCapsule::default(),
    };

    let ignored = challenge_ignored_changed_files(&metadata, &workspace_dir);
    let changed = vec![
        "START_HERE.md".to_string(),
        "SUCCESS.md".to_string(),
        "REFERENCE.md".to_string(),
        "benchmark.json".to_string(),
        ".quorp/challenge-capsule.json".to_string(),
        "src/lib.rs".to_string(),
    ];

    assert_eq!(
        filter_ignored_changed_files(&changed, &ignored),
        vec!["src/lib.rs".to_string()]
    );
}

#[test]
fn extract_read_range_observations_from_checkpoint_transcript() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let checkpoint_path = temp_dir.path().join("checkpoint.json");
    let checkpoint = quorp_agent_core::AgentCheckpoint {
            snapshot: quorp_agent_core::AgentTaskStateSnapshot {
                current_mode: quorp_agent_core::AgentMode::Act,
                acceptance_criteria: Vec::new(),
                working_set: BTreeSet::new(),
                last_tool_summary: None,
                last_failing_verifier: None,
                last_safe_checkpoint: None,
                last_parse_error: None,
                stall_count: 0,
                redundant_inspection_turns: 0,
                recoverable_inspection_failures: 0,
                parser_recovery_failures: 0,
                parser_recovery_validation_fingerprint: None,
                parser_recovery_same_validation_streak: 0,
                has_mutating_change: false,
                verified_green: false,
                validation_queue: std::collections::VecDeque::new(),
                total_billed_tokens: 0,
                last_failed_tool_error: None,
                repair_recovery_turns_remaining: 0,
                benchmark_case_ledger: None,
                repair_requirement: None,
                last_successful_write_action: None,
                benchmark_repair_state: None,
                failed_edit_records: Vec::new(),
                agent_repair_memory: quorp_agent_core::AgentRepairMemory::default(),
            },
            transcript: vec![TranscriptMessage {
                role: TranscriptRole::User,
                content: "[Tool Output]\nstatus: success\naction: read_file src/round.rs lines 390-450\npath: src/round.rs\nrequested_range: 390-450\nhonored_range: 390-450\nround excerpt".to_string(),
            }],
            step: 2,
            request_counter: 1,
        };
    write_json(&checkpoint_path, &checkpoint).expect("write checkpoint");

    let observations =
        extract_read_range_observations(&checkpoint_path).expect("read observations");

    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].path, "src/round.rs");
    assert_eq!(observations[0].requested_range.as_deref(), Some("390-450"));
    assert_eq!(observations[0].honored_range.as_deref(), Some("390-450"));
}

#[test]
fn extract_action_evidence_counts_reads_writes_and_gate_commands() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let checkpoint_path = temp_dir.path().join("checkpoint.json");
    let checkpoint = quorp_agent_core::AgentCheckpoint {
            snapshot: quorp_agent_core::AgentTaskStateSnapshot {
                current_mode: quorp_agent_core::AgentMode::Act,
                acceptance_criteria: Vec::new(),
                working_set: BTreeSet::new(),
                last_tool_summary: None,
                last_failing_verifier: None,
                last_safe_checkpoint: None,
                last_parse_error: None,
                stall_count: 0,
                redundant_inspection_turns: 0,
                recoverable_inspection_failures: 0,
                parser_recovery_failures: 0,
                parser_recovery_validation_fingerprint: None,
                parser_recovery_same_validation_streak: 0,
                has_mutating_change: false,
                verified_green: false,
                validation_queue: std::collections::VecDeque::new(),
                total_billed_tokens: 0,
                last_failed_tool_error: None,
                repair_recovery_turns_remaining: 0,
                benchmark_case_ledger: None,
                repair_requirement: None,
                last_successful_write_action: None,
                benchmark_repair_state: None,
                failed_edit_records: Vec::new(),
                agent_repair_memory: quorp_agent_core::AgentRepairMemory::default(),
            },
            transcript: vec![
                TranscriptMessage {
                    role: TranscriptRole::User,
                    content: "[Tool Output]\nstatus: success\naction: read_file src/round.rs lines 1-20\npath: src/round.rs\n".to_string(),
                },
                TranscriptMessage {
                    role: TranscriptRole::User,
                    content: "[Tool Output]\nstatus: success\naction: replace_block src/round.rs lines 10-12\n".to_string(),
                },
                TranscriptMessage {
                    role: TranscriptRole::User,
                    content: "[Tool Output]\nstatus: failure\naction: run: cargo test --quiet --lib round::tests::\n".to_string(),
                },
                TranscriptMessage {
                    role: TranscriptRole::User,
                    content: "[Tool Output]\nstatus: success\naction: run: ./evaluate.sh proof-full\n".to_string(),
                },
            ],
            step: 5,
            request_counter: 2,
        };
    write_json(&checkpoint_path, &checkpoint).expect("write checkpoint");
    let capsule = ChallengeCapsule {
        fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
        ..ChallengeCapsule::default()
    };

    let evidence = extract_action_evidence(
        &checkpoint_path,
        Some(&capsule),
        Some("./evaluate.sh proof-full"),
    )
    .expect("extract evidence");

    assert_eq!(evidence.read_count, 1);
    assert_eq!(evidence.write_count, 1);
    assert_eq!(evidence.command_execution_count, 2);
    assert!(evidence.fast_loop_command_seen);
    assert!(evidence.final_evaluate_command_seen);
}

#[test]
fn rust_swe_case_profiles_cover_recovery_gate_cases() {
    let expected = [
        (
            "06-rust-swebench-bincode-serde-decoder-memory",
            "cargo test --quiet --features serde --test issues issue_474",
        ),
        (
            "07-rust-swebench-chrono-epoch-truncation",
            "cargo test --quiet --lib round::tests::",
        ),
        (
            "08-rust-swebench-axum-fallback-merge",
            "cargo test --quiet -p axum --lib --features headers routing::tests::",
        ),
        (
            "09-rust-swebench-cargo-dist-create-release",
            "cargo test --quiet -p cargo-dist --test integration-tests axolotlsay_edit_existing -- --exact",
        ),
        (
            "10-rust-swebench-cc-rs-compile-intermediates",
            "cargo test --quiet compile_intermediates",
        ),
    ];

    for (case_id, fast_loop) in expected {
        let profile = rust_swe_case_profile(case_id).expect("profile");
        assert_eq!(profile.final_eval_command, "./evaluate.sh proof-full");
        assert!(
            profile
                .fast_loop_commands
                .iter()
                .any(|command| *command == fast_loop),
            "missing fast loop for {case_id}"
        );
        assert!(
            !profile.likely_owner_files.is_empty(),
            "missing owners for {case_id}"
        );
    }
}

#[test]
fn read_checkpoint_validation_state_parses_repair_phase_fields() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let checkpoint_path = temp_dir.path().join("checkpoint.json");
    let checkpoint = serde_json::json!({
        "snapshot": {
            "benchmark_case_ledger": {
                "validation_status": "failed: fast-loop",
                "last_validation_failure": "test `round::tests::test_duration_round_close_to_min_max` failed | at src/round.rs:800",
                "validation_details": {
                    "failing_test_names": ["round::tests::test_duration_round_close_to_min_max"],
                    "primary_failure_test_name": "round::tests::test_duration_round_close_to_min_max",
                    "primary_failure_path": "src/round.rs",
                    "primary_failure_line": 800,
                    "assertion_excerpt": "assertion `left == right` failed",
                    "repair_required": true,
                    "repair_phase_terminal": "needs_patch",
                    "failure_anchor_reread_attempted": true,
                    "failure_anchor_reread_honored": true,
                    "implementation_reread_allowed": true,
                    "implementation_reread_attempted": true,
                    "implementation_reread_honored": true,
                    "repair_phase_invalid_action_count": 1,
                    "post_fast_loop_patch_attempted": true,
                    "post_fast_loop_validation_rerun_attempted": false,
                    "patch_packet_injected": true,
                    "patch_packet_honored_range": "188-254",
                    "recommended_rerun_command": "cargo test --quiet --lib round::tests::test_duration_round_close_to_min_max",
                    "fast_loop_rerun_match_kind": "subset_fast_loop",
                    "failed_edit_records": [{
                        "action_kind": "replace_block",
                        "path": "src/round.rs",
                        "search_hash": "abc",
                        "replace_hash": "def",
                        "failure_reason": "Search block is ambiguous; found 2 matches at lines 151, 188",
                        "matching_line_numbers": [151, 188],
                        "attempts": 1
                    }]
                }
            }
        }
    });
    write_json(&checkpoint_path, &checkpoint).expect("write checkpoint");

    let state = read_checkpoint_validation_state(&checkpoint_path).expect("validation state");

    assert_eq!(
        state.primary_failure_test_name.as_deref(),
        Some("round::tests::test_duration_round_close_to_min_max")
    );
    assert_eq!(state.repair_phase_terminal.as_deref(), Some("needs_patch"));
    assert!(state.failure_anchor_reread_attempted);
    assert!(state.failure_anchor_reread_honored);
    assert!(state.implementation_reread_allowed);
    assert!(state.implementation_reread_attempted);
    assert!(state.implementation_reread_honored);
    assert_eq!(state.repair_phase_invalid_action_count, 1);
    assert!(state.patch_packet_injected);
    assert_eq!(state.patch_packet_honored_range.as_deref(), Some("188-254"));
    assert_eq!(
        state.recommended_rerun_command.as_deref(),
        Some("cargo test --quiet --lib round::tests::test_duration_round_close_to_min_max")
    );
    assert_eq!(
        state.fast_loop_rerun_match_kind.as_deref(),
        Some("subset_fast_loop")
    );
    assert_eq!(state.failed_edit_records.len(), 1);
    assert_eq!(
        state.failed_edit_records[0].matching_line_numbers,
        vec![151, 188]
    );
}

#[test]
fn judge_output_summary_truncates_large_logs() {
    let large = (0..80)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let summary = summarize_judge_output(&large);
    assert!(summary.contains("truncated 80 lines"));
    assert!(summary.contains("line 0"));
    assert!(summary.contains("line 79"));
}

#[test]
fn run_shell_command_with_env_applies_cargo_target_dir_override() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let target_dir = temp_dir.path().join("eval-target");
    let outcome = run_shell_command_with_env(
        "evaluation",
        "printf '%s' \"$CARGO_TARGET_DIR\"",
        &temp_dir.path().join("evaluate.sh"),
        temp_dir.path(),
        &[("CARGO_TARGET_DIR", target_dir.as_os_str())],
    )
    .expect("shell command");

    assert!(outcome.passed);
    assert_eq!(outcome.stdout, target_dir.display().to_string());
}

#[test]
fn workspace_challenge_command_wrappers_point_to_case_root_scripts() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let sandbox_root = temp_dir.path().join("sandbox");
    let workspace_dir = sandbox_root.join("workspace").join("proof-full");
    fs::create_dir_all(&workspace_dir).expect("workspace");

    write_workspace_challenge_command_wrappers(&workspace_dir).expect("write wrappers");
    let evaluate_wrapper =
        fs::read_to_string(workspace_dir.join("evaluate.sh")).expect("read evaluate wrapper");
    let reset_wrapper =
        fs::read_to_string(workspace_dir.join("reset.sh")).expect("read reset wrapper");
    assert!(evaluate_wrapper.contains("cd \"$(dirname \"$0\")/../..\""));
    assert!(evaluate_wrapper.contains("exec ./evaluate.sh"));
    assert!(reset_wrapper.contains("cd \"$(dirname \"$0\")/../..\""));
    assert!(reset_wrapper.contains("exec ./reset.sh"));
}

#[test]
fn challenge_evaluation_target_dir_is_attempt_scoped() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let metadata = ChallengeMetadata {
        case_root: temp_dir.path().join("case"),
        sandbox_root: temp_dir.path().join("run").join(CHALLENGE_SANDBOX_DIR),
        workspace_dir: temp_dir
            .path()
            .join("run")
            .join("workspace")
            .join("proof-full"),
        condition: "proof-full".to_string(),
        objective_file: temp_dir.path().join("run").join("START_HERE.md"),
        success_file: temp_dir.path().join("run").join("SUCCESS.md"),
        reference_file: None,
        reset_command: "./reset.sh proof-full".to_string(),
        evaluate_command: "./evaluate.sh proof-full".to_string(),
        expected_files_touched: Vec::new(),
        allowed_generated_files: Vec::new(),
        primary_metrics: Vec::new(),
        tags: Vec::new(),
        capsule_file: temp_dir
            .path()
            .join("run")
            .join("workspace")
            .join("proof-full")
            .join(".quorp")
            .join("challenge-capsule.json"),
        capsule: ChallengeCapsule::default(),
    };

    let attempt_one =
        challenge_evaluation_target_dir(&metadata, 1, CHALLENGE_EVALUATION_CARGO_CACHE_DIR);
    let attempt_two =
        challenge_evaluation_target_dir(&metadata, 2, CHALLENGE_EVALUATION_CARGO_CACHE_DIR);
    assert_ne!(attempt_one, attempt_two);
    assert!(attempt_one.ends_with("attempt-001"));
    assert!(attempt_two.ends_with("attempt-002"));
    assert!(
        attempt_one
            .components()
            .any(|component| component.as_os_str() == CHALLENGE_EVALUATION_CARGO_CACHE_DIR)
    );
}

#[test]
fn cargo_dist_snapshot_challenge_uses_workspace_target_for_evaluation() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let mut metadata = ChallengeMetadata {
        case_root: temp_dir.path().join("case"),
        sandbox_root: temp_dir.path().join("run").join(CHALLENGE_SANDBOX_DIR),
        workspace_dir: temp_dir
            .path()
            .join("run")
            .join("workspace")
            .join("proof-full"),
        condition: "proof-full".to_string(),
        objective_file: temp_dir.path().join("run").join("START_HERE.md"),
        success_file: temp_dir.path().join("run").join("SUCCESS.md"),
        reference_file: None,
        reset_command: "./reset.sh proof-full".to_string(),
        evaluate_command: "./evaluate.sh proof-full".to_string(),
        expected_files_touched: Vec::new(),
        allowed_generated_files: Vec::new(),
        primary_metrics: Vec::new(),
        tags: Vec::new(),
        capsule_file: temp_dir
            .path()
            .join("run")
            .join("workspace")
            .join("proof-full")
            .join(".quorp")
            .join("challenge-capsule.json"),
        capsule: ChallengeCapsule::default(),
    };
    let evaluation_target_dir = temp_dir.path().join("eval-target");

    assert_eq!(
        challenge_evaluation_env(&metadata, &evaluation_target_dir).len(),
        1
    );

    metadata.allowed_generated_files =
        vec!["cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap".to_string()];
    assert!(challenge_evaluation_env(&metadata, &evaluation_target_dir).is_empty());
}

#[test]
fn cc_rs_challenge_sets_sdkroot_for_macos_sdk_free_evaluation() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let metadata = ChallengeMetadata {
        case_root: temp_dir.path().join("05-cc-rs-compile-intermediates"),
        sandbox_root: temp_dir.path().join("run").join(CHALLENGE_SANDBOX_DIR),
        workspace_dir: temp_dir
            .path()
            .join("run")
            .join("workspace")
            .join("proof-full"),
        condition: "proof-full".to_string(),
        objective_file: temp_dir.path().join("run").join("START_HERE.md"),
        success_file: temp_dir.path().join("run").join("SUCCESS.md"),
        reference_file: None,
        reset_command: "./reset.sh proof-full".to_string(),
        evaluate_command: "./evaluate.sh proof-full".to_string(),
        expected_files_touched: vec!["src/lib.rs".to_string()],
        allowed_generated_files: Vec::new(),
        primary_metrics: Vec::new(),
        tags: vec!["cc-rs".to_string()],
        capsule_file: temp_dir
            .path()
            .join("run")
            .join("workspace")
            .join("proof-full")
            .join(".quorp")
            .join("challenge-capsule.json"),
        capsule: ChallengeCapsule::default(),
    };
    let evaluation_target_dir = temp_dir.path().join("eval-target");
    let env = challenge_evaluation_env(&metadata, &evaluation_target_dir);

    assert!(
        env.iter()
            .any(|(name, value)| { *name == "SDKROOT" && *value == Path::new("/").as_os_str() })
    );
    assert!(env.iter().any(|(name, value)| {
        *name == "CARGO_TARGET_DIR" && *value == evaluation_target_dir.as_os_str()
    }));
}

#[test]
fn challenge_capsule_extracts_chrono_owner_and_fast_loop() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root");
    let case_root =
        repo_root.join("benchmark/challenges/rust-swebench-top5/02-chrono-epoch-truncation");
    let challenge = resolve_challenge_case(&case_root.join("START_HERE.md"), Some("proof-full"))
        .expect("resolve challenge")
        .expect("challenge case");
    let capsule = compile_challenge_capsule(&challenge, &case_root).expect("capsule");
    assert_eq!(capsule.case_class, "narrow-owner-first");
    assert!(
        capsule
            .owner_files
            .iter()
            .any(|path| path == "src/round.rs")
    );
    assert!(
        capsule
            .fast_loop_commands
            .iter()
            .any(|command| command.contains("round::tests::"))
    );
}

#[test]
fn challenge_capsule_detects_axum_companion_files() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root");
    let case_root =
        repo_root.join("benchmark/challenges/rust-swebench-top5/03-axum-fallback-merge");
    let challenge = resolve_challenge_case(&case_root.join("START_HERE.md"), None)
        .expect("resolve challenge")
        .expect("challenge case");
    let capsule = compile_challenge_capsule(&challenge, &case_root).expect("capsule");
    assert_eq!(capsule.case_class, "breadth-heavy-companion");
    assert!(
        capsule
            .companion_files_required
            .iter()
            .any(|path| path == "axum/CHANGELOG.md")
    );
    assert!(
        capsule
            .strong_hints
            .iter()
            .any(|hint| hint.contains("panic strings"))
    );
}

#[test]
fn prepare_challenge_run_restores_capsule_after_reset() {
    let (_temp_dir, case_root) = create_challenge_case_fixture();
    fs::write(
            case_root.join("reset.sh"),
            "#!/usr/bin/env bash\nset -euo pipefail\ncondition=\"$1\"\nrm -rf \"workspace/${condition}/.quorp\"\nmkdir -p \"workspace/${condition}\"\n",
        )
        .expect("reset script");
    fs::write(
        case_root.join("evaluate.sh"),
        "#!/usr/bin/env bash\nset -euo pipefail\nexit 0\n",
    )
    .expect("evaluate script");

    let challenge = resolve_challenge_case(&case_root.join("START_HERE.md"), None)
        .expect("resolve challenge")
        .expect("challenge case");
    let result_dir = tempfile::tempdir().expect("result dir");

    let prepared = prepare_challenge_run(result_dir.path(), &challenge).expect("prepare");

    assert!(prepared.challenge_metadata.capsule_file.exists());
    let capsule_json =
        fs::read_to_string(&prepared.challenge_metadata.capsule_file).expect("read capsule");
    assert!(capsule_json.contains("\"owner_files\""));
}

#[test]
fn prepare_challenge_run_uses_flat_warpos_workspace_root() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let case_root = temp_dir.path().join("06-flat-case");
    fs::create_dir_all(case_root.join("src")).expect("src");
    fs::write(
        case_root.join("START_HERE.md"),
        "# Objective\n\nFix the flat challenge.\n",
    )
    .expect("objective");
    fs::write(case_root.join("SUCCESS.md"), "# Success\n").expect("success");
    fs::write(case_root.join("REFERENCE.md"), "# Reference\n").expect("reference");
    fs::write(case_root.join("LOCAL_REPRO.md"), "# Repro\n").expect("repro");
    fs::write(
        case_root.join("Cargo.toml"),
        "[package]\nname = \"flat_case\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("cargo");
    fs::write(
        case_root.join("src").join("lib.rs"),
        "pub fn sample() -> u32 { 1 }\n",
    )
    .expect("lib");
    fs::write(
        case_root.join(".benchmark-root.json"),
        serde_json::json!({
            "suite": "rust-swebench-top5",
            "issue": "06-flat-case",
            "condition": "proof-full",
        })
        .to_string(),
    )
    .expect("marker");
    fs::write(case_root.join("issue.json"), "{}").expect("issue");
    fs::write(
        case_root.join("benchmark.json"),
        serde_json::json!({
            "id": "06-flat-case",
            "title": "Flat challenge",
            "difficulty": "medium",
            "category": "rust",
            "repo_condition": ["proof-full"],
            "objective_file": "START_HERE.md",
            "success_file": "SUCCESS.md",
            "reset_command": "./reset.sh <condition>",
            "evaluate_command": "./evaluate.sh <condition>",
            "estimated_minutes": 1,
            "expected_files_touched": ["src/lib.rs"],
            "primary_metrics": ["total_tokens"],
            "tags": ["rust", "flat"],
        })
        .to_string(),
    )
    .expect("benchmark");
    fs::write(
        case_root.join("evaluate.sh"),
        "#!/usr/bin/env bash\nset -euo pipefail\nexit 0\n",
    )
    .expect("evaluate");

    let challenge = resolve_challenge_case(&case_root.join("START_HERE.md"), None)
        .expect("resolve challenge")
        .expect("challenge case");
    let result_dir = tempfile::tempdir().expect("result dir");

    let prepared = prepare_challenge_run(result_dir.path(), &challenge).expect("prepare");
    let sandbox_root = result_dir.path().join(CHALLENGE_SANDBOX_DIR);

    assert_eq!(prepared.challenge_metadata.workspace_dir, sandbox_root);
    assert_eq!(
        prepared.challenge_metadata.objective_file,
        sandbox_root.join("START_HERE.md")
    );
    assert_eq!(
        prepared.challenge_metadata.success_file,
        sandbox_root.join("SUCCESS.md")
    );
    assert!(sandbox_root.join("reset.sh").exists());
    assert!(result_dir.path().join(".quorp-flat-baseline").exists());
    assert!(prepared.challenge_metadata.capsule_file.exists());
    assert!(!sandbox_root.join("workspace").join("proof-full").exists());
}

#[test]
fn allowed_generated_files_do_not_count_as_widening() {
    assert!(!detect_widening_against_expected(
        &[
            "cargo-dist/src/config.rs".to_string(),
            "cargo-dist/tests/snapshots/demo.snap".to_string(),
        ],
        &["cargo-dist/src/config.rs".to_string()],
        &["cargo-dist/tests/snapshots/demo.snap".to_string()],
    ));
    assert!(detect_widening_against_expected(
        &[
            "cargo-dist/src/config.rs".to_string(),
            "cargo-dist/tests/snapshots/demo.snap".to_string(),
        ],
        &["cargo-dist/src/config.rs".to_string()],
        &[],
    ));
}

#[test]
fn benchmark_objective_includes_context_files() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let workspace = temp_dir.path().join("proof-full");
    fs::create_dir_all(&workspace).expect("mkdir");
    fs::write(
        workspace.join(".benchmark-root.json"),
        "{\"benchmark\":\"toy\"}",
    )
    .expect("root");
    fs::write(workspace.join("issue.json"), "{\"issue\":\"ISSUE-00\"}").expect("issue");
    fs::write(workspace.join("START_HERE.md"), "read start here").expect("start");
    fs::write(workspace.join("YOU_ARE_HERE.txt"), "toy workspace").expect("you are here");
    let objective = temp_dir.path().join("README.md");
    fs::write(&objective, "Fix the bug.").expect("objective");
    let resolved = ResolvedBenchmark {
        benchmark_root: temp_dir.path().to_path_buf(),
        issue_id: "ISSUE-00".to_string(),
        benchmark_name: "ISSUE-00".to_string(),
        issue_dir: None,
        workspace_source: workspace.clone(),
        objective_source: objective,
        visible_evaluator: None,
        collector_evaluator: None,
        context_files: collect_context_files(&workspace),
        repair_artifacts: Vec::new(),
    };
    let rendered =
        build_benchmark_objective(&resolved, &workspace, "remote_api", None).expect("objective");
    assert!(rendered.contains("Fix the bug."));
    assert!(rendered.contains("issue.json"));
    assert!(rendered.contains("START_HERE.md"));
}

#[test]
fn benchmark_objective_includes_helper_briefing_when_present() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let workspace = temp_dir.path().join("proof-full");
    fs::create_dir_all(&workspace).expect("mkdir");
    let objective = temp_dir.path().join("README.md");
    fs::write(&objective, "Fix the bug.").expect("objective");
    let resolved = ResolvedBenchmark {
        benchmark_root: temp_dir.path().to_path_buf(),
        issue_id: "ISSUE-00".to_string(),
        benchmark_name: "ISSUE-00".to_string(),
        issue_dir: None,
        workspace_source: workspace.clone(),
        objective_source: objective,
        visible_evaluator: None,
        collector_evaluator: None,
        context_files: Vec::new(),
        repair_artifacts: Vec::new(),
    };
    let rendered = build_benchmark_objective(
        &resolved,
        &workspace,
        "remote_api",
        Some("{\"summary\":\"look at pricing\"}"),
    )
    .expect("objective");
    assert!(rendered.contains("## Helper Briefing"));
    assert!(rendered.contains("\"summary\":\"look at pricing\""));
}

#[test]
fn load_benchmark_briefing_prefers_case_specific_json_entry() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let briefing_path = temp_dir.path().join("briefings.json");
    fs::write(
        &briefing_path,
        serde_json::json!({
            "default": "{\"summary\":\"default\"}",
            "ISSUE-42": "{\"summary\":\"case-specific\"}"
        })
        .to_string(),
    )
    .expect("write briefing map");
    let briefing =
        load_benchmark_briefing(Some(&briefing_path), "ISSUE-42").expect("load briefing");
    assert_eq!(briefing.as_deref(), Some("{\"summary\":\"case-specific\"}"));
}

#[test]
fn remote_benchmark_model_defaults_to_qwen() {
    let model_id =
        resolve_benchmark_model_id(BenchmarkExecutor::Native, None).expect("default model");
    assert_eq!(model_id, "qwen/qwen3-coder-480b-a35b-instruct");
}

#[test]
fn native_benchmark_defaults_use_ambient_remote_model_env() {
    let _guard = test_env_guard();
    let original_model = std::env::var("QUORP_MODEL").ok();
    let original_provider = std::env::var("QUORP_PROVIDER").ok();
    unsafe {
        std::env::set_var("QUORP_MODEL", "qwen/qwen3-coder-480b-a35b-instruct");
        std::env::set_var("QUORP_PROVIDER", "nvidia");
    }

    let resolved =
        resolve_benchmark_model_id(BenchmarkExecutor::Native, None).expect("remote model");

    if let Some(value) = original_model {
        unsafe {
            std::env::set_var("QUORP_MODEL", value);
        }
    } else {
        unsafe {
            std::env::remove_var("QUORP_MODEL");
        }
    }
    if let Some(value) = original_provider {
        unsafe {
            std::env::set_var("QUORP_PROVIDER", value);
        }
    } else {
        unsafe {
            std::env::remove_var("QUORP_PROVIDER");
        }
    }

    assert_eq!(resolved, "qwen/qwen3-coder-480b-a35b-instruct");
}

#[test]
fn safe_prompt_is_trimmed_under_cap() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let workspace = temp_dir.path().join("proof-full");
    fs::create_dir_all(workspace.join(".witness")).expect("mkdir");
    fs::write(workspace.join("AGENTS.md"), "read map first\n".repeat(80)).expect("agents");
    fs::write(
            workspace.join("agent-map.json"),
            serde_json::json!({
                "owners": [{"crate": "toy", "paths": ["crates/toy"], "validation": ["cargo test --quiet"]}]
            })
            .to_string(),
        )
        .expect("agent-map");
    fs::write(
        workspace.join("test-map.json"),
        serde_json::json!({
            "crates": [{"crate": "toy", "tests": ["cargo test -p toy-domain --quiet"]}]
        })
        .to_string(),
    )
    .expect("test-map");
    fs::write(
        workspace.join(".witness").join("witness-graph.json"),
        serde_json::json!({"nodes": [{"id": "toy-domain"}], "edges": []}).to_string(),
    )
    .expect("witness");
    let objective = temp_dir.path().join("README.md");
    fs::write(
        &objective,
        "# ISSUE\n\n".to_string() + &"Long brief line.\n".repeat(200),
    )
    .expect("objective");
    let resolved = ResolvedBenchmark {
        benchmark_root: temp_dir.path().to_path_buf(),
        issue_id: "ISSUE-00".to_string(),
        benchmark_name: "ISSUE-00".to_string(),
        issue_dir: None,
        workspace_source: workspace.clone(),
        objective_source: objective,
        visible_evaluator: None,
        collector_evaluator: None,
        context_files: collect_context_files(&workspace),
        repair_artifacts: Vec::new(),
    };
    let rendered =
        build_benchmark_objective(&resolved, &workspace, "remote_api", None).expect("objective");
    assert!(estimate_token_count(&rendered) <= SAFE_PROMPT_TOKEN_CAP + 64);
}

#[test]
fn trimmed_prompt_rebases_paths_into_attempt_workspace() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let workspace = temp_dir.path().join("proof-full");
    fs::create_dir_all(workspace.join(".witness")).expect("mkdir");
    fs::write(workspace.join("START_HERE.md"), "start here\n".repeat(120)).expect("start");
    fs::write(workspace.join("AGENTS.md"), "guardrails\n".repeat(120)).expect("agents");
    fs::write(
            workspace.join("agent-map.json"),
            serde_json::json!({
                "owners": [{"crate": "toy", "paths": ["crates/toy"], "validation": ["cargo test -p toy-domain --quiet"]}]
            })
            .to_string(),
        )
        .expect("agent-map");
    fs::write(
        workspace.join("test-map.json"),
        serde_json::json!({
            "crates": [{"crate": "toy", "tests": ["cargo test -p toy-domain --quiet"]}]
        })
        .to_string(),
    )
    .expect("test-map");
    fs::write(
        workspace.join(".witness").join("witness-graph.json"),
        serde_json::json!({"nodes": [{"id": "toy-domain"}], "edges": []}).to_string(),
    )
    .expect("witness");

    let objective = workspace.join("README.md");
    fs::write(&objective, "Long brief line.\n".repeat(200)).expect("objective");
    let repair_artifact = workspace.join("repair-notes.md");
    fs::write(&repair_artifact, "repair").expect("repair notes");

    let resolved = ResolvedBenchmark {
        benchmark_root: temp_dir.path().to_path_buf(),
        issue_id: "ISSUE-00".to_string(),
        benchmark_name: "ISSUE-00".to_string(),
        issue_dir: None,
        workspace_source: workspace.clone(),
        objective_source: objective,
        visible_evaluator: None,
        collector_evaluator: None,
        context_files: collect_context_files(&workspace),
        repair_artifacts: vec![repair_artifact.clone()],
    };

    let rendered =
        build_benchmark_objective(&resolved, &workspace, "remote_api", None).expect("objective");
    assert!(rendered.contains("README.md"));
    assert!(rendered.contains("START_HERE.md"));
}

#[test]
fn benchmark_completion_policy_keeps_repo_capsule_for_safe_mode() {
    let _guard = test_env_guard();
    clear_benchmark_completion_policy_env_overrides();

    let policy = benchmark_completion_policy(
        BenchmarkExecutor::Native,
        "remote_api",
        Some("openai-compatible/deepseek-coder-v2-lite-turbo"),
    );
    assert!(policy.include_repo_capsule);
    assert_eq!(policy.first_turn_max_completion_tokens, Some(1536));
    assert_eq!(policy.later_turn_max_completion_tokens, Some(2048));
    assert!(!policy.disable_reasoning);
    assert!(policy.native_tool_calls);
    assert_eq!(
        policy
            .watchdog
            .as_ref()
            .and_then(|watchdog| watchdog.total_timeout_ms),
        Some(360_000)
    );
    assert_eq!(
        benchmark_action_contract_mode(&policy),
        "native_tool_calls_v1"
    );
    assert_eq!(
        policy.prompt_compaction_policy,
        Some(PromptCompactionPolicy::BenchmarkStatePacket)
    );
}

#[test]
fn benchmark_completion_policy_applies_action_contract_overrides() {
    let _guard = test_env_guard();
    clear_benchmark_completion_policy_env_overrides();
    unsafe {
        std::env::set_var("QUORP_BENCH_NATIVE_TOOL_CALLS", "false");
        std::env::set_var("QUORP_BENCH_PROMPT_COMPACTION_POLICY", "last6-ledger768");
    }

    let policy = benchmark_completion_policy(
        BenchmarkExecutor::Native,
        "remote_api",
        Some("nvidia/qwen/qwen3-coder-480b-a35b-instruct"),
    );
    clear_benchmark_completion_policy_env_overrides();

    assert!(!policy.native_tool_calls);
    assert_eq!(
        policy.prompt_compaction_policy,
        Some(PromptCompactionPolicy::Last6Ledger768)
    );
    assert_eq!(benchmark_action_contract_mode(&policy), "strict_json_v1");
}

#[test]
fn nvidia_qwen_coder_benchmark_defaults_use_strict_json_and_profile_label() {
    let safety_label = benchmark_safety_mode_label(
        BenchmarkExecutor::Native,
        "nvidia/qwen/qwen3-coder-480b-a35b-instruct",
    );
    let policy = benchmark_completion_policy(
        BenchmarkExecutor::Native,
        &safety_label,
        Some("nvidia/qwen/qwen3-coder-480b-a35b-instruct"),
    );

    assert_eq!(safety_label, "nvidia_qwen_benchmark");
    assert!(policy.include_repo_capsule);
    assert!(policy.disable_reasoning);
    assert!(!policy.native_tool_calls);
    assert_eq!(policy.first_turn_max_completion_tokens, Some(4096));
    assert_eq!(policy.later_turn_max_completion_tokens, Some(4096));
    assert_eq!(
        policy.prompt_compaction_policy,
        Some(PromptCompactionPolicy::BenchmarkStatePacket)
    );
    assert_eq!(
        policy.safety_mode_label.as_deref(),
        Some("nvidia_qwen_benchmark")
    );
    assert_eq!(benchmark_action_contract_mode(&policy), "strict_json_v1");
}

#[test]
fn nvidia_qwen_coder_model_id_matches_remote_profiles() {
    assert!(is_nvidia_qwen_coder_model_id(
        "nvidia/qwen/qwen3-coder-480b-a35b-instruct"
    ));
    assert!(is_nvidia_qwen_coder_model_id(
        "qwen/qwen3-coder-480b-a35b-instruct"
    ));
    assert!(!is_nvidia_qwen_coder_model_id("other-model"));
}

#[test]
fn requested_compaction_override_preserves_existing_default_when_absent() {
    let mut policy = benchmark_completion_policy(
        BenchmarkExecutor::Native,
        "nvidia_qwen_benchmark",
        Some("nvidia/qwen/qwen3-coder-480b-a35b-instruct"),
    );

    apply_requested_prompt_compaction_override(&mut policy, None);
    assert_eq!(
        policy.prompt_compaction_policy,
        Some(PromptCompactionPolicy::BenchmarkStatePacket)
    );

    apply_requested_prompt_compaction_override(&mut policy, Some(PromptCompactionPolicy::Off));
    assert_eq!(
        policy.prompt_compaction_policy,
        Some(PromptCompactionPolicy::Off)
    );
}

#[test]
fn evaluator_requires_structured_success_flag_when_present() {
    assert!(evaluator_passed(true, "{\"success\": true}"));
    assert!(!evaluator_passed(true, "{\"success\": false}"));
    assert!(!evaluator_passed(false, "{\"success\": true}"));
    assert!(evaluator_passed(true, "plain stdout"));
}

#[test]
fn benchmark_lock_refuses_second_holder() {
    let temp_home = tempfile::tempdir().expect("tempdir");
    let lock_path = benchmark_run_lock_path_for_home(temp_home.path());
    let first_lock = BenchmarkRunLock::acquire_at(lock_path.clone()).expect("first lock");
    let second_error = BenchmarkRunLock::acquire_at(lock_path).expect_err("second lock must fail");
    assert!(second_error.to_string().contains("benchmark lock"));
    drop(first_lock);
}

#[test]
fn benchmark_lock_can_be_skipped_for_child_runs() {
    let _env_guard = test_env_guard();
    clear_benchmark_completion_policy_env_overrides();
    let temp_home = tempfile::tempdir().expect("temp home");
    let lock_path = benchmark_run_lock_path_for_home(temp_home.path());
    fs::create_dir_all(lock_path.parent().expect("lock parent")).expect("mkdir");
    fs::write(&lock_path, "occupied").expect("seed lock");

    let original_home = std::env::var("HOME").ok();
    unsafe {
        std::env::set_var("HOME", temp_home.path());
        std::env::set_var("QUORP_BENCHMARK_SKIP_LOCK", "1");
    }

    let lock = BenchmarkRunLock::acquire().expect("skipped lock");
    drop(lock);

    assert_eq!(
        fs::read_to_string(&lock_path).expect("lock still present"),
        "occupied"
    );

    match original_home {
        Some(home) => unsafe { std::env::set_var("HOME", home) },
        None => unsafe { std::env::remove_var("HOME") },
    }
}

#[test]
fn benchmark_run_completes_with_fake_model_server() {
    let _env_guard = test_env_guard();
    let temp_home = tempfile::tempdir().expect("temp home");
    let temp_results = tempfile::tempdir().expect("temp results");
    let (_fixture_dir, issue_dir) = create_toy_preview_benchmark_fixture();

    let original_home = std::env::var("HOME").ok();
    unsafe {
        std::env::set_var("HOME", temp_home.path());
    }

    let (base_url, server_handle) = start_fake_completion_server(
            serde_json::json!({
                "assistant_message": "Applying the smallest possible fix.",
                "actions": [
                    {
                        "ApplyPatch": {
                            "path": "crates/toy-domain/src/lib.rs",
                            "patch": "--- a/crates/toy-domain/src/lib.rs\n+++ b/crates/toy-domain/src/lib.rs\n@@ -1,6 +1,6 @@\n pub fn preview_change_reason(delayed_change: bool) -> &'static str {\n     if delayed_change {\n-        \"immediate\"\n+        \"scheduled_at_period_end\"\n     } else {\n         \"immediate\"\n     }\n"
                        }
                    }
                ],
                "task_updates": [],
                "memory_updates": [],
                "requested_mode_change": null,
                "verifier_plan": null
            })
            .to_string(),
            Duration::from_millis(250),
        );

    let result = run_benchmark(BenchmarkRunOptions {
        path: issue_dir,
        executor: BenchmarkExecutor::Native,
        model_id: Some("qwen3-coder-30b-a3b".to_string()),
        base_url_override: Some(base_url),
        briefing_file: None,
        compaction_policy: None,
        seed_transcript: None,
        max_steps: 8,
        max_seconds: Some(120),
        max_total_tokens: Some(1_000),
        result_dir: temp_results.path().to_path_buf(),
        autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousHost,
        max_attempts: Some(1),
        condition: None,
        keep_sandbox: false,
    });

    if let Some(home) = original_home {
        unsafe {
            std::env::set_var("HOME", home);
        }
    } else {
        unsafe {
            std::env::remove_var("HOME");
        }
    }

    result.expect("benchmark run should complete");
    server_handle.join().expect("join fake model server");

    let report_path = temp_results.path().join("benchmark-report.json");
    let report: BenchmarkReport =
        serde_json::from_str(&fs::read_to_string(&report_path).expect("read benchmark report"))
            .expect("parse benchmark report");
    assert!(report.success, "expected mocked benchmark to succeed");
    assert_eq!(report.attempts_run, 1);
    assert_eq!(report.provider_kind, "nvidia");
    assert_eq!(report.auth_mode, "test_loopback_api_key");
    assert_eq!(report.usage_source, "provider_response");
    assert!(!report.proxy_visible_remote_egress_expected);
    assert_eq!(report.requested_provider.as_deref(), Some("nvidia"));
    assert_eq!(
        report.requested_model.as_deref(),
        Some("qwen/qwen3-coder-480b-a35b-instruct")
    );
    assert_eq!(
        report.effective_model.as_deref(),
        Some("qwen/qwen3-coder-480b-a35b-instruct")
    );
    assert!(!report.used_fallback);
    assert_eq!(
        report.final_stop_reason,
        Some(quorp_agent_core::StopReason::Success)
    );
    assert!(
        report
            .attempts
            .first()
            .and_then(|attempt| attempt.visible_evaluation.as_ref())
            .is_some_and(|outcome| outcome.passed)
    );
    assert!(
        report
            .attempts
            .first()
            .and_then(|attempt| attempt.collector_evaluation.as_ref())
            .is_some_and(|outcome| outcome.passed)
    );
    assert!(
        report
            .attempts
            .first()
            .map(|attempt| attempt
                .changed_files
                .iter()
                .any(|path| path == "crates/toy-domain/src/lib.rs"))
            .unwrap_or(false)
    );

    let fixed_file = temp_results
        .path()
        .join("attempt-001")
        .join("workspace")
        .join("crates/toy-domain/src/lib.rs");
    let fixed_content = fs::read_to_string(&fixed_file).expect("read fixed file");
    assert!(fixed_content.contains("scheduled_at_period_end"));
}

#[test]
fn benchmark_run_completes_with_fake_remote_model_server_with_explicit_model() {
    let _env_guard = test_env_guard();
    let temp_home = tempfile::tempdir().expect("temp home");
    let temp_results = tempfile::tempdir().expect("temp results");
    let (_fixture_dir, issue_dir) = create_toy_preview_benchmark_fixture();

    let original_home = std::env::var("HOME").ok();
    unsafe {
        std::env::set_var("HOME", temp_home.path());
    }

    let (base_url, server_handle) = start_fake_completion_server(
            serde_json::json!({
                "assistant_message": "Applying the smallest possible fix.",
                "actions": [
                    {
                        "ApplyPatch": {
                            "path": "crates/toy-domain/src/lib.rs",
                            "patch": "--- a/crates/toy-domain/src/lib.rs\n+++ b/crates/toy-domain/src/lib.rs\n@@ -1,6 +1,6 @@\n pub fn preview_change_reason(delayed_change: bool) -> &'static str {\n     if delayed_change {\n-        \"immediate\"\n+        \"scheduled_at_period_end\"\n     } else {\n         \"immediate\"\n     }\n"
                        }
                    }
                ],
                "task_updates": [],
                "memory_updates": [],
                "requested_mode_change": null,
                "verifier_plan": null
            })
            .to_string(),
            Duration::from_millis(250),
        );

    let result = run_benchmark(BenchmarkRunOptions {
        path: issue_dir,
        executor: BenchmarkExecutor::Native,
        model_id: Some("openai-compatible/deepseek-coder-v2-lite-turbo".to_string()),
        base_url_override: Some(base_url),
        briefing_file: None,
        compaction_policy: None,
        seed_transcript: None,
        max_steps: 8,
        max_seconds: Some(120),
        max_total_tokens: Some(1_000),
        result_dir: temp_results.path().to_path_buf(),
        autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousHost,
        max_attempts: Some(1),
        condition: None,
        keep_sandbox: false,
    });

    if let Some(home) = original_home {
        unsafe {
            std::env::set_var("HOME", home);
        }
    } else {
        unsafe {
            std::env::remove_var("HOME");
        }
    }

    result.expect("remote benchmark run should complete");
    server_handle.join().expect("join fake model server");

    let report_path = temp_results.path().join("benchmark-report.json");
    let report: BenchmarkReport =
        serde_json::from_str(&fs::read_to_string(&report_path).expect("read benchmark report"))
            .expect("parse benchmark report");
    assert!(report.success, "expected mocked benchmark to succeed");
    assert_eq!(report.provider_kind, "nvidia");
    assert_eq!(
        report.requested_model.as_deref(),
        Some("qwen/qwen3-coder-480b-a35b-instruct")
    );
    assert_eq!(
        report.effective_model.as_deref(),
        Some("qwen/qwen3-coder-480b-a35b-instruct")
    );
}

#[test]
fn benchmark_run_records_effective_prompt_compaction_policy_for_verified_27b() {
    let _env_guard = test_env_guard();
    let temp_home = tempfile::tempdir().expect("temp home");
    let temp_results = tempfile::tempdir().expect("temp results");
    let (_fixture_dir, issue_dir) = create_toy_preview_benchmark_fixture();

    let original_home = std::env::var("HOME").ok();
    unsafe {
        std::env::set_var("HOME", temp_home.path());
    }

    let (base_url, server_handle) = start_fake_completion_server(
            serde_json::json!({
                "assistant_message": "Applying the smallest possible fix.",
                "actions": [
                    {
                        "ApplyPatch": {
                            "path": "crates/toy-domain/src/lib.rs",
                            "patch": "--- a/crates/toy-domain/src/lib.rs\n+++ b/crates/toy-domain/src/lib.rs\n@@ -1,6 +1,6 @@\n pub fn preview_change_reason(delayed_change: bool) -> &'static str {\n     if delayed_change {\n-        \"immediate\"\n+        \"scheduled_at_period_end\"\n     } else {\n         \"immediate\"\n     }\n"
                        }
                    }
                ],
                "task_updates": [],
                "memory_updates": [],
                "requested_mode_change": null,
                "verifier_plan": null
            })
            .to_string(),
            Duration::from_millis(250),
        );

    run_benchmark(BenchmarkRunOptions {
        path: issue_dir,
        executor: BenchmarkExecutor::Native,
        model_id: Some("qwen/qwen3-coder-480b-a35b-instruct".to_string()),
        base_url_override: Some(base_url),
        briefing_file: None,
        compaction_policy: None,
        seed_transcript: None,
        max_steps: 8,
        max_seconds: Some(120),
        max_total_tokens: Some(1_000),
        result_dir: temp_results.path().to_path_buf(),
        autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousHost,
        max_attempts: Some(1),
        condition: None,
        keep_sandbox: false,
    })
    .expect("remote benchmark run should complete");
    server_handle.join().expect("join fake model server");

    if let Some(home) = original_home {
        unsafe {
            std::env::set_var("HOME", home);
        }
    } else {
        unsafe {
            std::env::remove_var("HOME");
        }
    }

    let manifest_path = temp_results.path().join("benchmark-manifest.json");
    let manifest: BenchmarkManifest =
        serde_json::from_str(&fs::read_to_string(&manifest_path).expect("read manifest"))
            .expect("parse manifest");
    assert_eq!(
        manifest.compaction_policy,
        Some(PromptCompactionPolicy::BenchmarkStatePacket)
    );

    let request_path = temp_results
        .path()
        .join("attempt-001")
        .join("agent")
        .join("request.json");
    let request: quorp_agent_core::AgentRunRequest =
        serde_json::from_str(&fs::read_to_string(&request_path).expect("read request"))
            .expect("parse request");
    assert_eq!(
        request.completion_policy.prompt_compaction_policy,
        Some(PromptCompactionPolicy::BenchmarkStatePacket)
    );

    let turn_request_path = temp_results
        .path()
        .join("attempt-001")
        .join("agent")
        .join("artifacts")
        .join("model_turns")
        .join("request-0001.json");
    let turn_request: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&turn_request_path).expect("read turn request"))
            .expect("parse turn request");
    assert_eq!(
        turn_request["prompt_compaction_policy"].as_str(),
        Some("benchmark-state-packet")
    );
}

#[test]
fn benchmark_resume_replays_from_checkpoint_with_fake_model_server() {
    let _env_guard = test_env_guard();
    let temp_home = tempfile::tempdir().expect("temp home");
    let temp_results = tempfile::tempdir().expect("temp results");
    let (_fixture_dir, issue_dir) = create_toy_preview_benchmark_fixture();

    let original_home = std::env::var("HOME").ok();
    unsafe {
        std::env::set_var("HOME", temp_home.path());
    }

    let (initial_base_url, initial_server_handle) = start_fake_completion_server(
            serde_json::json!({
                "assistant_message": "Applying the smallest possible fix.",
                "actions": [
                    {
                        "ApplyPatch": {
                            "path": "crates/toy-domain/src/lib.rs",
                            "patch": "--- a/crates/toy-domain/src/lib.rs\n+++ b/crates/toy-domain/src/lib.rs\n@@ -1,6 +1,6 @@\n pub fn preview_change_reason(delayed_change: bool) -> &'static str {\n     if delayed_change {\n-        \"immediate\"\n+        \"scheduled_at_period_end\"\n     } else {\n         \"immediate\"\n     }\n"
                        }
                    }
                ],
                "task_updates": [],
                "memory_updates": [],
                "requested_mode_change": null,
                "verifier_plan": null
            })
            .to_string(),
            Duration::from_secs(3),
        );

    run_benchmark(BenchmarkRunOptions {
        path: issue_dir.clone(),
        executor: BenchmarkExecutor::Native,
        model_id: Some("qwen3-coder-30b-a3b".to_string()),
        base_url_override: Some(initial_base_url.clone()),
        briefing_file: None,
        compaction_policy: None,
        seed_transcript: None,
        max_steps: 8,
        max_seconds: Some(120),
        max_total_tokens: Some(1_000),
        result_dir: temp_results.path().to_path_buf(),
        autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousHost,
        max_attempts: Some(1),
        condition: None,
        keep_sandbox: false,
    })
    .expect("initial benchmark run should complete");
    let request_path = temp_results
        .path()
        .join("attempt-001")
        .join("agent")
        .join("request.json");
    let mut request: quorp_agent_core::AgentRunRequest =
        serde_json::from_str(&fs::read_to_string(&request_path).expect("read request"))
            .expect("parse request");
    request.base_url_override = Some(initial_base_url.clone());
    fs::write(
        &request_path,
        serde_json::to_vec_pretty(&request).expect("serialize request"),
    )
    .expect("write request");

    resume_benchmark(BenchmarkResumeOptions {
        result_dir: temp_results.path().to_path_buf(),
    })
    .expect("resume should complete");
    initial_server_handle
        .join()
        .expect("join initial fake model server");

    if let Some(home) = original_home {
        unsafe {
            std::env::set_var("HOME", home);
        }
    } else {
        unsafe {
            std::env::remove_var("HOME");
        }
    }

    let report_path = temp_results.path().join("benchmark-report.json");
    let report: BenchmarkReport =
        serde_json::from_str(&fs::read_to_string(&report_path).expect("read benchmark report"))
            .expect("parse benchmark report");
    assert!(
        report.success,
        "expected resumed benchmark to remain successful"
    );
    assert_eq!(report.attempts_run, 1);
    assert_eq!(
        report.final_stop_reason,
        Some(quorp_agent_core::StopReason::Success)
    );
    assert!(
        report
            .attempts
            .first()
            .and_then(|attempt| attempt.visible_evaluation.as_ref())
            .is_some_and(|outcome| outcome.passed)
    );
    assert!(
        report
            .attempts
            .first()
            .and_then(|attempt| attempt.collector_evaluation.as_ref())
            .is_some_and(|outcome| outcome.passed)
    );
}

#[test]
fn benchmark_run_reports_failure_cleanly_with_bad_model_response() {
    let _env_guard = test_env_guard();
    let temp_home = tempfile::tempdir().expect("temp home");
    let temp_results = tempfile::tempdir().expect("temp results");
    let (_fixture_dir, issue_dir) = create_toy_preview_benchmark_fixture();

    let original_home = std::env::var("HOME").ok();
    unsafe {
        std::env::set_var("HOME", temp_home.path());
    }

    let (base_url, server_handle) = start_fake_completion_server(
        "{\"assistant_message\":\"oops\"".to_string(),
        Duration::from_secs(5),
    );

    run_benchmark(BenchmarkRunOptions {
        path: issue_dir,
        executor: BenchmarkExecutor::Native,
        model_id: Some("qwen3-coder-30b-a3b".to_string()),
        base_url_override: Some(base_url),
        briefing_file: None,
        compaction_policy: None,
        seed_transcript: None,
        max_steps: 8,
        max_seconds: Some(120),
        max_total_tokens: Some(1_000),
        result_dir: temp_results.path().to_path_buf(),
        autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousHost,
        max_attempts: Some(1),
        condition: None,
        keep_sandbox: false,
    })
    .expect("benchmark run should still complete reporting after failure");
    server_handle.join().expect("join bad fake model server");

    if let Some(home) = original_home {
        unsafe {
            std::env::set_var("HOME", home);
        }
    } else {
        unsafe {
            std::env::remove_var("HOME");
        }
    }

    let report_path = temp_results.path().join("benchmark-report.json");
    let report: BenchmarkReport =
        serde_json::from_str(&fs::read_to_string(&report_path).expect("read benchmark report"))
            .expect("parse benchmark report");
    assert!(
        !report.success,
        "expected malformed completion to fail the benchmark"
    );
    assert_eq!(
        report.final_stop_reason,
        Some(quorp_agent_core::StopReason::FatalError)
    );
    assert!(
        report
            .attempts
            .first()
            .and_then(|attempt| attempt.agent_error_message.as_ref())
            .is_some_and(|message| message.contains("Structured agent turn was invalid JSON"))
    );
    assert!(
        report
            .attempts
            .first()
            .and_then(|attempt| attempt.visible_evaluation.as_ref())
            .is_some_and(|outcome| !outcome.passed)
    );
    assert!(
        report
            .attempts
            .first()
            .and_then(|attempt| attempt.collector_evaluation.as_ref())
            .is_some_and(|outcome| !outcome.passed)
    );
}

#[test]
fn challenge_judge_native_completes_with_remote_model_server() {
    let _env_guard = test_env_guard();
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let sandbox_root = temp_dir.path().join("sandbox");
    let workspace_dir = sandbox_root.join("workspace").join("proof-full");
    let attempt_dir = temp_dir.path().join("attempt-001");
    fs::create_dir_all(&workspace_dir).expect("workspace");
    fs::create_dir_all(&attempt_dir).expect("attempt");
    fs::write(workspace_dir.join("START_HERE.md"), "Fix the issue.").expect("objective");
    fs::write(workspace_dir.join("SUCCESS.md"), "Make the evaluator pass.").expect("success");
    fs::write(workspace_dir.join("REFERENCE.md"), "Upstream provenance.").expect("reference");

    let (base_url, server_handle) = start_fake_completion_server(
        r#"{"passed":true,"summary":"looks good","rationale":"the evaluation passed"}"#.to_string(),
        Duration::from_millis(250),
    );

    let manifest = BenchmarkManifest {
        resolved: ResolvedBenchmark {
            benchmark_root: sandbox_root.clone(),
            issue_id: "01-safe-judge".to_string(),
            benchmark_name: "Safe judge benchmark".to_string(),
            issue_dir: None,
            workspace_source: workspace_dir.clone(),
            objective_source: workspace_dir.join("START_HERE.md"),
            visible_evaluator: None,
            collector_evaluator: None,
            context_files: vec![workspace_dir.join("REFERENCE.md")],
            repair_artifacts: Vec::new(),
        },
        executor: BenchmarkExecutor::Native,
        model_id: "qwen/qwen3-coder-480b-a35b-instruct".to_string(),
        safety_mode_label: "remote_api".to_string(),
        scenario_label: None,
        base_url_override: Some(base_url),
        briefing_file: None,
        compaction_policy: None,
        seed_transcript: None,
        max_steps: 1,
        max_seconds: Some(30),
        max_total_tokens: None,
        autonomy_profile: "autonomous_host".to_string(),
        max_attempts: 1,
        challenge: Some(ChallengeMetadata {
            case_root: sandbox_root.clone(),
            sandbox_root: sandbox_root.clone(),
            workspace_dir: workspace_dir.clone(),
            condition: "proof-full".to_string(),
            objective_file: workspace_dir.join("START_HERE.md"),
            success_file: workspace_dir.join("SUCCESS.md"),
            reference_file: Some(workspace_dir.join("REFERENCE.md")),
            reset_command: "./reset.sh proof-full".to_string(),
            evaluate_command: "./evaluate.sh proof-full".to_string(),
            expected_files_touched: vec!["src/lib.rs".to_string()],
            allowed_generated_files: Vec::new(),
            primary_metrics: vec!["evaluate_passed".to_string()],
            tags: vec!["rust".to_string()],
            capsule_file: workspace_dir.join(".quorp").join("challenge-capsule.json"),
            capsule: ChallengeCapsule::default(),
        }),
        keep_sandbox: true,
        completion_policy: quorp_agent_core::CompletionPolicy::default(),
    };
    let evaluation = EvaluatorOutcome {
        name: "evaluation".to_string(),
        script: sandbox_root.join("evaluate.sh"),
        command: Some("./evaluate.sh proof-full".to_string()),
        duration_ms: 10,
        exit_code: 0,
        passed: true,
        stdout: "{\"success\":true}".to_string(),
        stderr: String::new(),
    };
    let outcome = quorp_agent_core::AgentRunOutcome {
        stop_reason: quorp_agent_core::StopReason::Success,
        total_steps: 1,
        total_billed_tokens: 12,
        duration_ms: 25,
        transcript: Vec::new(),
        error_message: None,
    };
    let metrics = RequestMetricsSummary {
        max_prompt_token_estimate: Some(256),
        max_completion_token_cap: Some(512),
        watchdog_near_limit: false,
        watchdog_triggered: false,
        first_request_prompt_token_estimate: Some(256),
        first_request_raw_prompt_token_estimate: Some(256),
        first_request_compacted_prompt_token_estimate: None,
        first_request_first_token_latency_ms: Some(10),
        first_model_turn_started: true,
        first_action_emitted: false,
        prompt_token_series_by_turn: Vec::new(),
    };
    let usage = crate::quorp::agent_runner::HeadlessUsageSummary {
        model_requests: 1,
        reported_billed_tokens: 320,
        estimated_billed_tokens: 320,
        total_billed_tokens: 320,
        input_tokens: 256,
        output_tokens: 64,
        reasoning_tokens: 0,
        cache_read_input_tokens: 0,
        cache_write_input_tokens: 0,
    };
    let changed_files = vec!["src/lib.rs".to_string()];
    let validations: Vec<String> = Vec::new();
    let context = ChallengeJudgeContext {
        manifest: &manifest,
        metadata: manifest.challenge.as_ref().expect("challenge metadata"),
        attempt_number: 1,
        attempt_dir: &attempt_dir,
        outcome: &outcome,
        evaluation: &evaluation,
        changed_files: &changed_files,
        validations: &validations,
        metrics: &metrics,
        usage: &usage,
    };

    let judge = run_challenge_judge(&context);
    server_handle.join().expect("join judge model server");

    assert!(judge.passed, "expected judge request to succeed");
    assert_eq!(judge.summary, "looks good");
    assert_eq!(judge.rationale, "the evaluation passed");
}

#[test]
fn judge_transport_failure_does_not_block_deterministic_success() {
    let transport_failure = ChallengeJudgeOutcome {
        passed: false,
        summary: "judge request failed".to_string(),
        rationale: "first token timeout after 30000ms".to_string(),
        model_id: "nvidia/qwen/qwen3-coder-480b-a35b-instruct".to_string(),
        raw_response: serde_json::json!({}),
        error: None,
    };
    assert!(!judge_blocks_deterministic_success(&transport_failure));

    let semantic_failure = ChallengeJudgeOutcome {
        passed: false,
        summary: "patch changed unrelated files".to_string(),
        rationale: "the diff widened beyond the target".to_string(),
        model_id: "nvidia/qwen/qwen3-coder-480b-a35b-instruct".to_string(),
        raw_response: serde_json::json!({}),
        error: None,
    };
    assert!(judge_blocks_deterministic_success(&semantic_failure));
}

#[test]
fn transient_challenge_judge_errors_are_retryable() {
    assert!(transient_challenge_judge_error(
        "NVIDIA NIM returned 503 Service Unavailable: ResourceExhausted"
    ));
    assert!(transient_challenge_judge_error(
        "first token timeout after 30000ms"
    ));
    assert!(!transient_challenge_judge_error(
        "judge response could not be parsed"
    ));
}

fn start_fake_completion_server(
    turn_content: String,
    idle_shutdown_after: Duration,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake model server");
    listener.set_nonblocking(true).expect("set nonblocking");
    let address = listener.local_addr().expect("local addr");
    let base_url = format!("http://{address}/v1");
    let stream_response_body = serde_json::json!({
        "id": "chatcmpl-fake",
        "choices": [
            {
                "index": 0,
                "delta": { "content": turn_content },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 42,
            "completion_tokens": 24,
            "total_tokens": 66
        }
    })
    .to_string();
    let json_response_body = serde_json::json!({
        "id": "chatcmpl-fake",
        "choices": [
            {
                "index": 0,
                "message": { "content": turn_content },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 42,
            "completion_tokens": 24,
            "total_tokens": 66
        }
    })
    .to_string();
    let stream_body = format!("data: {stream_response_body}\n\ndata: [DONE]\n\n");
    let handle = thread::spawn(move || {
        let mut served_requests = 0usize;
        let mut last_request_at = Instant::now();
        let no_request_shutdown_after = Duration::from_secs(60);
        loop {
            match listener.accept() {
                Ok((mut stream, _peer)) => {
                    if let Err(error) = stream.set_read_timeout(Some(Duration::from_secs(2))) {
                        log::trace!("fake model server set read timeout failed: {error}");
                    }
                    let mut request_bytes = Vec::new();
                    loop {
                        let mut buffer = [0u8; 8192];
                        match stream.read(&mut buffer) {
                            Ok(0) => break,
                            Ok(bytes_read) => {
                                request_bytes.extend_from_slice(&buffer[..bytes_read]);
                                if expected_http_request_len(&request_bytes)
                                    .is_some_and(|expected_len| request_bytes.len() >= expected_len)
                                {
                                    break;
                                }
                            }
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) =>
                            {
                                break;
                            }
                            Err(error) => {
                                log::trace!("fake model server read failed: {error}");
                                break;
                            }
                        }
                    }
                    let request_text = String::from_utf8_lossy(&request_bytes);
                    let (content_type, body) = if request_text.contains("\"stream\":false") {
                        ("application/json", json_response_body.as_str())
                    } else {
                        ("text/event-stream", stream_body.as_str())
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    if let Err(error) = stream.write_all(response.as_bytes()) {
                        log::trace!("fake model server write failed: {error}");
                    }
                    if let Err(error) = stream.flush() {
                        log::trace!("fake model server flush failed: {error}");
                    }
                    served_requests += 1;
                    last_request_at = Instant::now();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if served_requests > 0 && last_request_at.elapsed() >= idle_shutdown_after {
                        break;
                    }
                    if last_request_at.elapsed() >= no_request_shutdown_after {
                        break;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break,
            }
        }
    });
    (base_url, handle)
}

fn expected_http_request_len(request_bytes: &[u8]) -> Option<usize> {
    let header_end = request_bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")?;
    let headers = std::str::from_utf8(&request_bytes[..header_end]).ok()?;
    let content_length = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find_map(|(name, value)| {
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    Some(header_end + 4 + content_length)
}

#[test]
fn classify_failure_labels_repair_loop_stalled_from_agent_error() {
    let report: BenchmarkReport = serde_json::from_value(serde_json::json!({
            "benchmark_name": "Example",
            "issue_id": "example",
            "success": false,
            "attempts_run": 1,
            "max_attempts": 1,
            "total_billed_tokens": 0,
            "final_stop_reason": "stalled",
            "changed_files": [],
            "widening_happened": false,
            "attempts": [{
                "attempt": 1,
                "executor": "native",
                "model_id": "nvidia/qwen/qwen3-coder-480b-a35b-instruct",
                "safety_mode_label": "safe",
                "scenario_label": null,
                "agent_stop_reason": "stalled",
                "agent_error_message": "Autonomous repair loop stalled because the model kept responding without a concrete repair action.",
                "total_steps": 3,
                "total_billed_tokens": 0,
                "changed_files": [],
                "validations": [],
                "widening_happened": false,
                "attempt_dir": "/tmp/attempt",
                "workspace_dir": "/tmp/workspace",
                "agent_result_dir": "/tmp/agent"
            }]
        }))
        .expect("report");

    assert_eq!(
        classify_primary_failure(&report).as_deref(),
        Some("repair_loop_stalled")
    );
    assert_eq!(
        classify_agent_failure(&report, Some("repair_loop_stalled")).as_deref(),
        Some("repair_loop_stalled")
    );
}

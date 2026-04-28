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
    clear_benchmark_completion_policy_env_overrides();
    let temp_home = tempfile::tempdir().expect("temp home");
    let temp_results = tempfile::tempdir().expect("temp results");
    let (_fixture_dir, issue_dir) = create_toy_preview_benchmark_fixture();

    let original_home = std::env::var("HOME").ok();
    let original_native_tool_calls = std::env::var("QUORP_BEN_NATIVE_TOOL_CALLS").ok();
    unsafe {
        std::env::set_var("HOME", temp_home.path());
        std::env::set_var("QUORP_BEN_NATIVE_TOOL_CALLS", "false");
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
            Duration::from_secs(5),
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
    match original_native_tool_calls {
        Some(value) => unsafe { std::env::set_var("QUORP_BEN_NATIVE_TOOL_CALLS", value) },
        None => unsafe { std::env::remove_var("QUORP_BEN_NATIVE_TOOL_CALLS") },
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
            Duration::from_secs(5),
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
            Duration::from_secs(5),
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
        Duration::from_secs(5),
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

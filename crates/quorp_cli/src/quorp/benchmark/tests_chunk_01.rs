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


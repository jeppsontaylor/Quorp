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

#[test]
fn replay_summary_validates_synthetic_run() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let ledger_path = temp_dir.path().join("run-ledger.jsonl");
    let mut writer = crate::quorp::run_support::RunLedgerWriter::open(&ledger_path, "run-1")
        .expect("ledger writer");
    writer
        .append(
            "runtime",
            "RunStarted",
            serde_json::json!({"RunStarted": {"goal": "ship", "model_id": "test"}}),
            1,
        )
        .expect("started");
    writer
        .append(
            "runtime",
            "RunFinished",
            serde_json::json!({"RunFinished": {"reason": "Success", "total_steps": 1}}),
            2,
        )
        .expect("finished");

    let summary = render_replay_summary(temp_dir.path()).expect("summary");

    assert!(summary.contains("run_id: run-1"));
    assert!(summary.contains("events: 2"));
    assert!(summary.contains("- RunStarted: 1"));
    assert!(summary.contains("run_finished:"));
}

#[test]
fn proof_verify_succeeds_on_synthetic_run_and_fails_after_raw_log_tamper() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let run_dir = temp_dir.path();
    let ledger_path = run_dir.join("run-ledger.jsonl");
    let mut writer = crate::quorp::run_support::RunLedgerWriter::open(&ledger_path, "run-1")
        .expect("ledger writer");
    writer
        .append(
            "runtime",
            "run.started",
            serde_json::json!({"event": "run.started"}),
            1,
        )
        .expect("ledger event");
    let raw_log = run_dir.join("raw.log");
    std::fs::write(&raw_log, "ok").expect("raw log");
    let raw_hash = sha256_file_required(&raw_log).expect("raw hash");
    let dag = quorp_verify::ProofDag {
        run_id: quorp_ids::VerifyRunId::new("verify-test"),
        provenance: serde_json::json!({"source": "test"}),
        nodes: vec![quorp_verify::ProofNode {
            id: "node".to_string(),
            stage_id: "fmt".to_string(),
            status: quorp_verify::ProofNodeStatus::Pass,
            summary: "ok".to_string(),
            artifacts: vec![quorp_verify::ProofArtifactRef {
                role: "raw_log".to_string(),
                path: raw_log.clone(),
                sha256: raw_hash.clone(),
            }],
            cache_key: None,
            from_cache: false,
            packet: None,
            report: None,
        }],
        edges: Vec::new(),
    };
    let dag_path = run_dir.join("proof-dag.json");
    std::fs::write(&dag_path, serde_json::to_vec_pretty(&dag).expect("dag json"))
        .expect("dag");

    let mut receipt = quorp_core::ProofReceipt::new("run-1");
    receipt.raw_artifacts.insert(
        "run-ledger".to_string(),
        quorp_core::RawArtifact {
            path: ledger_path.clone(),
            sha256: Some(sha256_file_required(&ledger_path).expect("ledger hash")),
        },
    );
    receipt.raw_artifacts.insert(
        "proof-dag".to_string(),
        quorp_core::RawArtifact {
            path: dag_path.clone(),
            sha256: Some(sha256_file_required(&dag_path).expect("dag hash")),
        },
    );
    receipt.raw_artifacts.insert(
        "verify-log-fmt".to_string(),
        quorp_core::RawArtifact {
            path: raw_log.clone(),
            sha256: Some(raw_hash),
        },
    );
    std::fs::write(
        run_dir.join("proof-receipt.json"),
        serde_json::to_vec_pretty(&receipt).expect("receipt json"),
    )
    .expect("receipt");

    verify_proof_input(run_dir).expect("proof verifies");
    let bundle = proof_bundle_for_run(run_dir).expect("proof bundle");
    assert_eq!(bundle.receipt.run_id, "run-1");

    std::fs::write(&raw_log, "tampered").expect("tamper raw log");

    assert!(verify_proof_input(run_dir).is_err());
}

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
    let explicit =
        SessionLaunchConfig::from_workspace(challenge, CliTuiMode::Auto, None, None, None, None);

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

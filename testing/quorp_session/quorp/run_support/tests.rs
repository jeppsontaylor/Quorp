use super::*;

#[test]
fn objective_resolution_prefers_start_here() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    fs::write(temp_dir.path().join("START_HERE.md"), "start").expect("start here");
    fs::write(temp_dir.path().join("README.md"), "readme").expect("readme");
    fs::write(temp_dir.path().join("evaluate.sh"), "#!/bin/sh").expect("evaluate");

    let resolved = resolve_workspace_objective(temp_dir.path(), None).expect("resolved objective");
    let expected = fs::canonicalize(temp_dir.path().join("START_HERE.md"))
        .unwrap_or_else(|_| temp_dir.path().join("START_HERE.md"));

    assert_eq!(resolved.objective_file, expected);
    assert_eq!(resolved.evaluate_command.as_deref(), Some("./evaluate.sh"));
}

#[test]
fn default_run_result_dir_is_unique_for_parallel_launches() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let first = default_run_result_dir(temp_dir.path(), "full-auto");
    let second = default_run_result_dir(temp_dir.path(), "full-auto");
    let workspace_component = sanitize_component(
        temp_dir
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace"),
    );
    let first_name = first
        .file_name()
        .and_then(|name| name.to_str())
        .expect("first run dir name");
    let second_name = second
        .file_name()
        .and_then(|name| name.to_str())
        .expect("second run dir name");

    assert_ne!(first, second);
    assert!(first_name.ends_with(&format!("-{workspace_component}")));
    assert!(second_name.ends_with(&format!("-{workspace_component}")));
}

#[test]
fn summarize_run_dir_reports_failure_and_stop_reason() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    write_json(
        &temp_dir.path().join("summary.json"),
        &serde_json::json!({
            "stop_reason": "max_steps",
            "logical_success": false,
            "process_exit_code": 0,
            "evaluation_passed": false,
        }),
    )
    .expect("summary");
    write_json(
        &temp_dir.path().join("metadata.json"),
        &serde_json::json!({
            "objective_file": "START_HERE.md",
            "evaluate_command": "./evaluate.sh",
            "provider": "nvidia",
            "model_id": "nvidia/qwen/qwen3-coder-480b-a35b-instruct",
            "logical_success": false,
            "process_exit_code": 0,
            "evaluation_passed": false,
        }),
    )
    .expect("metadata");
    fs::write(
        temp_dir.path().join("events.jsonl"),
        concat!(
            "{\"payload\":{\"event\":\"agent.path_resolution_failed\",\"request_path\":\"workspace/crates/reconciliation-core\",\"suggested_path\":\"crates/reconciliation-core\",\"reason\":\"redundant_workspace_prefix\"}}\n",
            "{\"payload\":{\"event\":\"agent.recovery_turn_queued\",\"action\":\"ls workspace/crates/reconciliation-core\",\"suggested_path\":\"crates/reconciliation-core\"}}\n",
            "{\"payload\":{\"event\":\"agent.parse_recovery_queued\",\"step\":2,\"error_class\":\"trailing_characters\",\"failures\":1,\"budget\":2,\"message\":\"[Parser] retry\"}}\n",
            "{\"payload\":{\"event\":\"assistant_turn_summary\",\"step\":2,\"assistant_message\":\"working\",\"actions\":[\"read_file foo\"],\"wrote_files\":false,\"validation_queued\":false,\"parse_warning_count\":1}}\n",
            "{\"payload\":{\"event\":\"tool_call_finished\",\"action\":\"replace_block crates/reconciliation-core/src/lib.rs\",\"status\":\"success\",\"action_kind\":\"replace_block\",\"target_path\":\"crates/reconciliation-core/src/lib.rs\",\"edit_summary\":\"replace 2 lines -> 3 lines\"}}\n",
            "{\"payload\":{\"event\":\"agent.verifier_queued\",\"plans\":[\"fmt\",\"workspace_tests\"],\"reason\":\"post_edit\"}}\n",
            "{\"payload\":{\"event\":\"tool_call_finished\",\"action\":\"read_file fixtures/out_of_order_recovery/bare/Cargo.toml\",\"status\":\"failure\",\"action_kind\":\"read_file\"}}\n",
            "{\"payload\":{\"event\":\"validation_finished\",\"summary\":\"./evaluate.sh proof-full\",\"status\":\"failure\"}}\n",
            "{\"payload\":{\"event\":\"run.retry_started\",\"attempt\":2}}\n"
        ),
    )
    .expect("events");

    let summary = summarize_run_dir(temp_dir.path()).expect("summary text");

    assert!(summary.contains("START_HERE.md"));
    assert!(summary.contains("First edit: crates/reconciliation-core/src/lib.rs"));
    assert!(summary.contains(
        "First failing action: read_file fixtures/out_of_order_recovery/bare/Cargo.toml -> failure"
    ));
    assert!(summary.contains("First bad path: workspace/crates/reconciliation-core"));
    assert!(summary.contains("Suggested correction: crates/reconciliation-core"));
    assert!(summary.contains("Recovery turns queued: 1"));
    assert!(summary.contains("Parser recovery turns queued: 1"));
    assert!(summary.contains("Parser warnings observed: 1"));
    assert!(summary.contains("Failure class: later validation/evaluator issue"));
    assert!(summary.contains("Full retries attempted: 1"));
    assert!(summary.contains("Validation queued before stop: true"));
    assert!(summary.contains("Evaluator logical success: false"));
    assert!(summary.contains("Evaluation passed: false"));
    assert!(summary.contains("Final stop reason: max_steps"));
}

#[test]
fn run_evaluator_prefers_logical_success_from_json_output() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let script_path = temp_dir.path().join("evaluate.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\ncat <<'EOF'\n{\"success\": false, \"command\": \"demo\"}\nEOF\nexit 0\n",
    )
    .expect("script");
    #[allow(clippy::disallowed_methods)]
    std::process::Command::new("chmod")
        .arg("+x")
        .arg(&script_path)
        .status()
        .expect("chmod");

    let outcome = run_evaluator(temp_dir.path(), "./evaluate.sh", None).expect("evaluate");

    assert!(outcome.process_passed);
    assert_eq!(outcome.process_exit_code, 0);
    assert_eq!(outcome.logical_success, Some(false));
    assert!(!outcome.evaluation_passed);
    assert!(outcome.parsed_from_stdout);
}

#[test]
fn challenge_resolution_defaults_to_proof_full_and_substitutes_condition() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp_dir.path().join("workspace").join("proof-full")).expect("workspace");
    fs::write(temp_dir.path().join("START_HERE.md"), "brief").expect("brief");
    fs::write(temp_dir.path().join("expected.md"), "success").expect("success");
    fs::write(
        temp_dir.path().join("benchmark.json"),
        serde_json::json!({
            "repo_condition": ["bare", "proof-full"],
            "objective_file": "START_HERE.md",
            "success_file": "expected.md",
            "reset_command": "./reset.sh <condition>",
            "evaluate_command": "./evaluate.sh <condition>",
        })
        .to_string(),
    )
    .expect("benchmark");

    let resolved = resolve_workspace_objective(temp_dir.path(), None).expect("resolved");

    assert_eq!(resolved.selected_condition.as_deref(), Some("proof-full"));
    assert_eq!(
        resolved.evaluate_command.as_deref(),
        Some("./evaluate.sh proof-full")
    );
    assert_eq!(
        resolved.reset_command.as_deref(),
        Some("./reset.sh proof-full")
    );
    assert_eq!(
        resolved.editable_workspace_root,
        temp_dir
            .path()
            .join("workspace")
            .join("proof-full")
            .canonicalize()
            .unwrap_or_else(|_| temp_dir.path().join("workspace").join("proof-full"))
    );
}

#[test]
fn bundle_run_dir_creates_zip_file() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    fs::write(temp_dir.path().join("request.json"), "{}").expect("request");
    let output_path = temp_dir.path().join("bundle.zip");

    let bundle_path = bundle_run_dir(temp_dir.path(), &output_path).expect("bundle");

    assert_eq!(bundle_path, output_path);
    assert!(bundle_path.exists());
}

#[test]
fn append_event_record_writes_ledger_sidecar_and_prefers_it_for_readback() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let events_path = temp_dir.path().join("events.jsonl");

    append_named_event(
        &events_path,
        "run.phase_changed",
        serde_json::json!({ "phase": "planning" }),
    )
    .expect("event one");
    append_named_event(
        &events_path,
        "run.phase_changed",
        serde_json::json!({ "phase": "executing" }),
    )
    .expect("event two");

    let ledger_path = run_ledger_path(&events_path);
    let ledger = read_run_ledger(&ledger_path).expect("ledger");
    assert_eq!(ledger.len(), 2);
    assert_eq!(ledger[0].seq, 1);
    assert_eq!(ledger[1].seq, 2);
    assert_eq!(ledger[1].prev_hash, ledger[0].hash);
    assert_ne!(ledger[0].hash, ledger[1].hash);

    fs::remove_file(&events_path).expect("remove legacy events");
    let readback = read_run_event_payloads(temp_dir.path()).expect("readback");
    assert_eq!(readback.len(), 2);
    assert_eq!(event_name(&readback[0]), Some("run.phase_changed"));
    assert_eq!(
        event_field(&readback[0], "phase").and_then(Value::as_str),
        Some("planning")
    );
}

#[test]
fn append_event_log_merges_run_ledger_chain() {
    let source_dir = tempfile::tempdir().expect("source dir");
    let destination_dir = tempfile::tempdir().expect("destination dir");
    let source_events = source_dir.path().join("events.jsonl");
    let destination_events = destination_dir.path().join("events.jsonl");

    append_named_event(
        &source_events,
        "run.phase_changed",
        serde_json::json!({ "phase": "retrying" }),
    )
    .expect("source event one");
    append_named_event(
        &source_events,
        "run.phase_changed",
        serde_json::json!({ "phase": "evaluating" }),
    )
    .expect("source event two");

    append_event_log(&destination_events, &source_events).expect("append log");

    let ledger = read_run_ledger(&run_ledger_path(&destination_events)).expect("dest ledger");
    assert_eq!(ledger.len(), 2);
    assert_eq!(ledger[0].seq, 1);
    assert_eq!(ledger[1].seq, 2);
    assert_eq!(ledger[1].prev_hash, ledger[0].hash);
    assert_eq!(ledger[0].kind, "run.phase_changed");
}

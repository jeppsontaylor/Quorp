use super::*;

#[test]
fn detects_phase_nine_score_regressions() {
    let previous = score_report_for_test(1, 1, 100, 100, 10, 0, 1);
    let current = score_report_for_test(1, 0, 100, 100, 10, 0, 1);

    let regressions = detect_score_regressions(Some(&previous), &current);

    assert!(
        regressions
            .iter()
            .any(|regression| regression.contains("secure success cases decreased"))
    );
    assert!(
        regressions
            .iter()
            .any(|regression| regression.contains("secure success rate decreased"))
    );
}

#[test]
fn detects_phase_nine_cost_regressions_at_equal_coverage() {
    let previous = score_report_for_test(1, 1, 100, 100, 10, 0, 1);
    let current = score_report_for_test(1, 1, 150, 125, 20, 1, 2);

    let regressions = detect_score_regressions(Some(&previous), &current);

    assert!(
        regressions
            .iter()
            .any(|regression| regression.contains("total billed tokens increased"))
    );
    assert!(
        regressions
            .iter()
            .any(|regression| regression.contains("SecureETTS tokens increased"))
    );
    assert!(
        regressions
            .iter()
            .any(|regression| regression.contains("median wall time increased"))
    );
    assert!(
        regressions
            .iter()
            .any(|regression| regression.contains("total retries increased"))
    );
    assert!(
        regressions
            .iter()
            .any(|regression| regression.contains("total patch size increased"))
    );
}

#[test]
fn fail_on_regression_writes_scoreboard_then_returns_error() {
    let test_root = unique_temp_dir("quorp-score-gate");
    let run_dir = test_root.join("run");
    let output_root = test_root.join("scoreboards");
    fs::create_dir_all(&run_dir).expect("run dir");
    fs::create_dir_all(&output_root).expect("output root");
    write_json(
        &output_root.join("latest.json"),
        &score_report_for_test(1, 1, 100, 100, 10, 0, 1),
    )
    .expect("previous score");
    write_json(
        &run_dir.join("benchmark-report.json"),
        &serde_json::json!({
            "benchmark_name": "Regression Fixture",
            "issue_id": "case-a",
            "success": false,
            "attempts_run": 1,
            "max_attempts": 1,
            "total_billed_tokens": 150,
            "max_total_tokens": null,
            "final_stop_reason": "fatal_error",
            "changed_files": [],
            "widening_happened": false,
            "attempts": [],
            "run_dir": run_dir,
            "wall_clock_ms": 20,
            "total_requests": 1,
            "first_model_turn_started": true,
            "first_action_emitted": true,
            "agent_final_failure_classification": "model_edit_strategy"
        }),
    )
    .expect("benchmark report");

    let error = score_benchmark_reports(BenchmarkScoreOptions {
        run_dirs: vec![run_dir],
        suite: String::new(),
        reports_root: test_root.join("reports"),
        output_root: Some(output_root.clone()),
        fail_on_regression: true,
    })
    .expect_err("regression gate should fail");

    assert!(error.to_string().contains("benchmark score regressed"));
    assert!(output_root.join("latest.json").exists());
    assert!(output_root.join("latest.md").exists());
    fs::remove_dir_all(test_root).expect("cleanup");
}

fn score_report_for_test(
    solved_cases: usize,
    secure_success_cases: usize,
    total_billed_tokens: u64,
    secure_etts_tokens: u64,
    median_wall_clock_ms: u64,
    total_retries: usize,
    total_patch_lines_changed: u64,
) -> BenchmarkScoreReport {
    let total_cases = 1;
    BenchmarkScoreReport {
        suite: "test".to_string(),
        generated_at_unix_seconds: 1,
        output_dir: PathBuf::from("/tmp/quorp-score-test"),
        run_dirs: vec![PathBuf::from("/tmp/quorp-score-test/run")],
        total_cases,
        solved_cases,
        valid_write_cases: solved_cases,
        post_write_validation_cases: solved_cases,
        diagnostic_classified_cases: total_cases,
        tooling_healthy_cases: total_cases,
        success_rate_ppm: rate_ppm(solved_cases, total_cases),
        secure_success_cases,
        secure_success_rate_ppm: rate_ppm(secure_success_cases, total_cases),
        total_requests: 1,
        total_billed_tokens,
        secure_etts_tokens,
        total_wall_clock_ms: median_wall_clock_ms,
        median_wall_clock_ms,
        total_patch_lines_changed,
        total_retries,
        proof_lane_counts: BTreeMap::new(),
        slow_first_token_cases: 0,
        watchdog_near_limit_cases: 0,
        patch_quality_risk_cases: 0,
        common_blocker: None,
        blocker_counts: BTreeMap::new(),
        regressions: Vec::new(),
        cases: Vec::new(),
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{}-{suffix}", std::process::id()))
}

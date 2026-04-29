use super::*;

#[test]
fn nearest_rank_p95_uses_expected_index() {
    assert_eq!(nearest_rank_index(1, 95), 0);
    assert_eq!(nearest_rank_index(10, 95), 9);
    assert_eq!(nearest_rank_index(20, 95), 18);
}

#[test]
fn analyzer_ignores_non_sample_rows_and_reports_summary() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let log_path = temp_dir.path().join("memory.log");
    std::fs::write(
        &log_path,
        concat!(
            "{\"event\":\"memory.run_started\",\"app_run_id\":\"run-1\",\"sample_interval_ms\":1000}\n",
            "not-json\n",
            "{\"event\":\"memory.sample\",\"ts_ms\":1000,\"rss_bytes_total\":104857600}\n",
            "{\"event\":\"memory.sampler_error\",\"detail\":\"ignored\"}\n",
            "{\"event\":\"memory.sample\",\"ts_ms\":2000,\"rss_bytes_total\":209715200}\n"
        ),
    )
    .expect("write log");

    let summary = analyze_memory_log(&log_path).expect("analyze");
    assert_eq!(summary.run_id.as_deref(), Some("run-1"));
    assert_eq!(summary.sample_interval_ms, Some(1000));
    assert_eq!(summary.sample_count, 2);
    assert_eq!(summary.duration_ms, 1000);
    assert_eq!(summary.min_rss_bytes, 104_857_600);
    assert_eq!(summary.max_rss_bytes, 209_715_200);
    assert_eq!(summary.p95_rss_bytes, 209_715_200);
    assert_eq!(summary.mean_rss_bytes, 157_286_400);
}

#[test]
fn analyzer_error_mentions_missing_path() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let missing = temp_dir.path().join("missing-memory.log");

    let error = analyze_memory_log(&missing).expect_err("missing log should fail");
    assert!(error.to_string().contains(&missing.display().to_string()));
}

#[test]
fn formatting_memory_summary_is_human_readable() {
    let summary = MemorySummary {
        run_id: Some("run-2".to_string()),
        sample_interval_ms: Some(1000),
        sample_count: 3,
        duration_ms: 2000,
        mean_rss_bytes: 157_286_400,
        min_rss_bytes: 104_857_600,
        max_rss_bytes: 209_715_200,
        p95_rss_bytes: 209_715_200,
    };
    let rendered = format_memory_summary(Path::new("/tmp/QuorpMemory.log"), &summary);
    assert!(rendered.contains("Run id: run-2"));
    assert!(rendered.contains("Mean RSS: 150.0 MB"));
    assert!(rendered.contains("P95 RSS: 200.0 MB"));
}

#[test]
fn start_memory_logger_truncates_and_writes_header_and_samples() {
    clear_managed_pids_for_test();
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let log_path = temp_dir.path().join("memory.log");
    std::fs::write(&log_path, "stale-data\n").expect("seed stale log");

    let config = LoggerConfig {
        path: log_path.clone(),
        app_run_id: "run-logger".to_string(),
        workspace_root: temp_dir.path().display().to_string(),
        version: "test-version".to_string(),
        interval: Duration::from_millis(20),
        root_pid: std::process::id(),
        started_at: Instant::now(),
    };

    start_memory_logger_with_config(config).expect("start logger");
    std::thread::sleep(Duration::from_millis(70));

    let contents = std::fs::read_to_string(&log_path).expect("read log");
    assert!(!contents.contains("stale-data"));
    assert!(contents.contains("\"event\":\"memory.run_started\""));
    assert!(contents.contains("\"event\":\"memory.sample\""));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn process_tree_sample_counts_child_processes() {
    #[allow(clippy::disallowed_methods)]
    fn spawn_sleep_child() -> std::io::Result<std::process::Child> {
        std::process::Command::new("sh")
            .arg("-c")
            .arg("sleep 2")
            .spawn()
    }

    clear_managed_pids_for_test();
    let mut child = spawn_sleep_child().expect("spawn child");
    std::thread::sleep(Duration::from_millis(100));

    let snapshot = sample_process_tree(std::process::id(), "test-run").expect("sample");
    assert!(snapshot.process_count >= 2);
    assert!(snapshot.rss_bytes_total >= snapshot.rss_bytes_main);
    assert_eq!(
        snapshot.rss_bytes_total,
        snapshot.rss_bytes_main + snapshot.rss_bytes_children
    );

    if let Err(error) = child.kill()
        && error.kind() != std::io::ErrorKind::InvalidInput
    {
        panic!("kill child: {error}");
    }
    child.wait().expect("wait child");
}

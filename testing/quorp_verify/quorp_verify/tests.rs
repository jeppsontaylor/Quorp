use super::*;
use rand::{Rng, SeedableRng, rngs::StdRng};

#[test]
fn cache_key_is_deterministic() {
    let key = CacheKey {
        git_sha: "abc".into(),
        changed_files_hash: "deadbeef".into(),
        features: vec!["default".into()],
        target_triple: "aarch64-apple-darwin".into(),
        rustc_version: "1.93.0".into(),
        stage_id: "L1Check".into(),
    };
    let s1 = cache_key_canonical_string(&key);
    let s2 = cache_key_canonical_string(&key);
    assert_eq!(s1, s2);
    assert!(s1.contains("abc"));
    assert!(s1.contains("L1Check"));
}

#[test]
fn fresh_run_id_is_non_empty() {
    let id = fresh_run_id();
    assert!(id.as_str().starts_with("verify-"));
}

#[test]
fn proof_packet_preserves_cargo_json_decisive_facts() {
    let output = r#"{"reason":"compiler-message","message":{"level":"error","code":{"code":"E0308"},"message":"mismatched types","spans":[{"file_name":"src/lib.rs","line_start":12,"column_start":5,"is_primary":true}]}}"#;
    let packet = proof_packet_from_command(CommandOutputEvidence {
        command: "cargo check --message-format=json",
        cwd: Path::new("/tmp/work"),
        exit_code: 101,
        duration_ms: 10,
        output,
        raw_log_path: PathBuf::from("logs/check.ndjson"),
        tool_version: Some("rustc 1.93.0".to_string()),
        truncated: false,
    });

    assert_eq!(packet.kind, ProofPacketKind::Compiler);
    assert_eq!(packet.command.exit_code, 101);
    assert_eq!(packet.diagnostics[0].code.as_deref(), Some("E0308"));
    assert_eq!(
        packet.diagnostics[0]
            .primary_span
            .as_ref()
            .map(|span| span.line),
        Some(12)
    );
    assert_eq!(packet.raw_log_ref.sha256, sha256_hex(output.as_bytes()));
    assert!(packet.summary.contains("exit_code=101"));
}

#[test]
fn proof_packet_preserves_failing_test_name_and_panic() {
    let output = "\
running 1 test
test billing::tests::grace_period_upgrade ... FAILED

---- billing::tests::grace_period_upgrade stdout ----
thread 'billing::tests::grace_period_upgrade' panicked at src/lib.rs:9: expected later
";
    let packet = proof_packet_from_command(CommandOutputEvidence {
        command: "cargo test -p billing-domain",
        cwd: Path::new("/tmp/work"),
        exit_code: 101,
        duration_ms: 22,
        output,
        raw_log_path: PathBuf::from("logs/test.log"),
        tool_version: None,
        truncated: false,
    });

    assert_eq!(packet.kind, ProofPacketKind::Test);
    assert!(
        packet
            .failing_tests
            .iter()
            .any(|test| test.name == "billing::tests::grace_period_upgrade")
    );
    assert!(
        packet
            .summary
            .contains("first_test=billing::tests::grace_period_upgrade")
    );
}

#[test]
fn proof_packet_preserves_security_advisory() {
    let output = "advisory RUSTSEC-2024-0001 severity: high package: example vulnerability";
    let packet = proof_packet_from_command(CommandOutputEvidence {
        command: "cargo audit",
        cwd: Path::new("/tmp/work"),
        exit_code: 1,
        duration_ms: 3,
        output,
        raw_log_path: PathBuf::from("logs/audit.log"),
        tool_version: None,
        truncated: false,
    });

    assert_eq!(packet.kind, ProofPacketKind::Security);
    assert_eq!(
        packet.security_findings[0].advisory_id.as_deref(),
        Some("RUSTSEC-2024-0001")
    );
    assert!(packet.summary.contains("first_advisory=RUSTSEC-2024-0001"));
}

#[test]
fn failure_parser_extracts_compiler_span_and_summary() {
    let output = r#"{"reason":"compiler-message","message":{"level":"error","code":{"code":"E0308"},"message":"mismatched types","spans":[{"file_name":"src/lib.rs","line_start":12,"column_start":5,"is_primary":true}]}}"#;
    let packet = parse_failure_packet("cargo check --message-format=json", output);

    assert_eq!(packet.kind, FailurePacketKind::Compiler);
    assert_eq!(
        packet.primary_span.as_ref().map(|span| span.file.clone()),
        Some(PathBuf::from("src/lib.rs"))
    );
    assert!(packet.summary.contains("diagnostics=1"));
    assert!(
        packet
            .failures
            .iter()
            .any(|failure| failure.code.as_deref() == Some("E0308"))
    );
}

#[test]
fn stage_report_from_packet_keeps_raw_log_reference() {
    let packet = proof_packet_from_command(CommandOutputEvidence {
        command: "cargo check",
        cwd: Path::new("/tmp/work"),
        exit_code: 0,
        duration_ms: 5,
        output: "ok",
        raw_log_path: PathBuf::from("logs/check.log"),
        tool_version: None,
        truncated: false,
    });
    let report = stage_report_from_packet(
        &packet,
        CacheKey {
            git_sha: "abc".to_string(),
            changed_files_hash: "def".to_string(),
            features: vec!["default".to_string()],
            target_triple: "aarch64-apple-darwin".to_string(),
            rustc_version: "1.93.0".to_string(),
            stage_id: "fast".to_string(),
        },
    );

    assert_eq!(report.status, StageStatus::Pass);
    assert_eq!(
        report
            .raw_log_ref
            .as_ref()
            .map(|artifact| artifact.path.clone()),
        Some(PathBuf::from("logs/check.log"))
    );
}

#[test]
fn cache_key_canonical_string_changes_when_any_field_changes() {
    let mut rng = StdRng::seed_from_u64(0x5e5f_2026);

    for _ in 0..32 {
        let base = CacheKey {
            git_sha: format!("{:016x}", rng.random::<u64>()),
            changed_files_hash: format!("{:016x}", rng.random::<u64>()),
            features: vec![
                "default".to_string(),
                format!("feature-{}", rng.random::<u16>()),
            ],
            target_triple: "aarch64-apple-darwin".to_string(),
            rustc_version: format!(
                "1.{}.{}",
                rng.random_range(80..100),
                rng.random_range(0..10)
            ),
            stage_id: format!("stage-{}", rng.random::<u32>()),
        };

        let canonical = cache_key_canonical_string(&base);
        assert_eq!(canonical, cache_key_canonical_string(&base));

        let mut changed_git_sha = base.clone();
        changed_git_sha.git_sha.push('x');
        assert_ne!(canonical, cache_key_canonical_string(&changed_git_sha));

        let mut changed_hash = base.clone();
        changed_hash.changed_files_hash.push('y');
        assert_ne!(canonical, cache_key_canonical_string(&changed_hash));

        let mut changed_stage = base.clone();
        changed_stage.stage_id.push('z');
        assert_ne!(canonical, cache_key_canonical_string(&changed_stage));
    }
}

#[test]
fn execute_verify_request_with_cache_uses_explicit_cache_after_first_run() {
    let request = VerifyRequest {
        plan: VerifyPlan {
            run_id: VerifyRunId::new("verify-test"),
            level: VerifyLevel::L2Targeted,
            targets: vec![VerifyTarget::Workspace],
            time_budget: Duration::from_secs(30),
            fail_fast: false,
        },
        commands: vec![VerifyCommand {
            stage_id: "fmt".to_string(),
            command: "cargo fmt --all --check".to_string(),
            cwd: PathBuf::from("."),
        }],
        git_sha: "abc".to_string(),
        changed_files_hash: "def".to_string(),
        features: Vec::new(),
        target_triple: "aarch64-apple-darwin".to_string(),
        rustc_version: "1.93.0".to_string(),
    };

    let mut executions = 0;
    let cache = MemoryVerifyCache::default();
    let first = execute_verify_request_with_cache(&request, &cache, |_| {
        executions += 1;
        Ok(VerifyCommandResult {
            exit_code: 0,
            duration_ms: 12,
            output: "ok".to_string(),
            raw_log_path: PathBuf::from("logs/fmt.log"),
            tool_version: None,
            truncated: false,
        })
    })
    .expect("first verify run");
    let second = execute_verify_request_with_cache(&request, &cache, |_| {
        executions += 1;
        Ok(VerifyCommandResult {
            exit_code: 0,
            duration_ms: 99,
            output: "should not run".to_string(),
            raw_log_path: PathBuf::from("logs/fmt.log"),
            tool_version: None,
            truncated: false,
        })
    })
    .expect("second verify run");

    assert_eq!(executions, 1);
    assert_eq!(first.cache_hits, 0);
    assert_eq!(second.cache_hits, 1);
    assert!(second.stages[0].from_cache);
    assert_eq!(second.stages[0].duration_ms, 0);
}

#[test]
fn file_cache_round_trips_cached_stage_report() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let store = VerifyStore::for_workspace(temp_dir.path());
    let cache = FileVerifyCache::new(store.clone());
    let key = CacheKey {
        git_sha: "abc".to_string(),
        changed_files_hash: "def".to_string(),
        features: Vec::new(),
        target_triple: "aarch64-apple-darwin".to_string(),
        rustc_version: "1.93.0".to_string(),
        stage_id: "fmt".to_string(),
    };
    let packet = proof_packet_from_command(CommandOutputEvidence {
        command: "cargo fmt",
        cwd: temp_dir.path(),
        exit_code: 0,
        duration_ms: 10,
        output: "ok",
        raw_log_path: temp_dir.path().join("fmt.log"),
        tool_version: None,
        truncated: false,
    });
    let mut report = stage_report_from_packet(&packet, key.clone());
    report.stage_id = "fmt".to_string();
    cache
        .put(
            &key,
            &VerifyCacheEntry {
                report: report.clone(),
                packet: packet.clone(),
            },
        )
        .expect("put cache");

    let reopened = FileVerifyCache::new(store);
    let entry = reopened.get(&key).expect("get cache").expect("cache entry");

    assert_eq!(entry.report.stage_id, "fmt");
    assert_eq!(entry.packet.command.command, "cargo fmt");
}

#[test]
fn durable_verify_reopen_marks_from_cache_and_writes_dag() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let store = VerifyStore::for_workspace(temp_dir.path());
    let request = VerifyRequest {
        plan: VerifyPlan {
            run_id: VerifyRunId::new("verify-test"),
            level: VerifyLevel::L2Targeted,
            targets: vec![VerifyTarget::Workspace],
            time_budget: Duration::from_secs(30),
            fail_fast: false,
        },
        commands: vec![VerifyCommand {
            stage_id: "fmt".to_string(),
            command: "cargo fmt --all --check".to_string(),
            cwd: temp_dir.path().to_path_buf(),
        }],
        git_sha: "abc".to_string(),
        changed_files_hash: "def".to_string(),
        features: Vec::new(),
        target_triple: "aarch64-apple-darwin".to_string(),
        rustc_version: "1.93.0".to_string(),
    };
    let raw_log_path = store.raw_log_path(&request.plan.run_id, "fmt");
    let first = execute_verify_request_durable(&store, &request, serde_json::json!({}), |_| {
        Ok(VerifyCommandResult {
            exit_code: 0,
            duration_ms: 12,
            output: "ok".to_string(),
            raw_log_path: raw_log_path.clone(),
            tool_version: None,
            truncated: false,
        })
    })
    .expect("first durable run");
    let second_store = VerifyStore::for_workspace(temp_dir.path());
    let second =
        execute_verify_request_durable(&second_store, &request, serde_json::json!({}), |_| {
            Err("should not execute".to_string())
        })
        .expect("second durable run");

    assert_eq!(first.cache_hits, 0);
    assert_eq!(second.cache_hits, 1);
    assert!(second.stages[0].from_cache);
    assert!(store.proof_dag_path(&request.plan.run_id).exists());
}

#[test]
fn raw_log_hash_mismatch_fails_artifact_verification() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let raw_log = temp_dir.path().join("raw.log");
    std::fs::write(&raw_log, "ok").expect("write raw log");
    let dag = ProofDag {
        run_id: VerifyRunId::new("verify-test"),
        provenance: serde_json::json!({}),
        nodes: vec![ProofNode {
            id: "node".to_string(),
            stage_id: "fmt".to_string(),
            status: ProofNodeStatus::Pass,
            summary: "ok".to_string(),
            artifacts: vec![ProofArtifactRef {
                role: "raw_log".to_string(),
                path: raw_log.clone(),
                sha256: sha256_hex(b"ok"),
            }],
            cache_key: None,
            from_cache: false,
            packet: None,
            report: None,
        }],
        edges: Vec::new(),
    };

    std::fs::write(&raw_log, "tampered").expect("tamper raw log");

    assert!(verify_proof_dag_artifacts(&dag).is_err());
}

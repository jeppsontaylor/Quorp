use super::*;

#[test]
fn verify_plan_round_trip() {
    let plan = VerifyPlan {
        run_id: VerifyRunId::new("v-001"),
        level: VerifyLevel::L1Check,
        targets: vec![VerifyTarget::Workspace],
        time_budget: Duration::from_secs(60),
        fail_fast: true,
    };
    let json = serde_json::to_string(&plan).unwrap();
    let back: VerifyPlan = serde_json::from_str(&json).unwrap();
    assert_eq!(back.run_id, plan.run_id);
    assert_eq!(back.time_budget, Duration::from_secs(60));
}

#[test]
fn proof_packet_round_trips_with_decisive_fields() {
    let packet = ProofPacket {
        kind: ProofPacketKind::Compiler,
        command: CommandEvidence {
            command: "cargo check --message-format=json".to_string(),
            cwd: PathBuf::from("."),
            exit_code: 1,
            duration_ms: 42,
            tool_version: Some("rustc 1.93.0".to_string()),
        },
        summary: "1 compiler diagnostic".to_string(),
        diagnostics: vec![CargoDiagnostic {
            level: "error".to_string(),
            code: Some("E0308".to_string()),
            message: "mismatched types".to_string(),
            primary_span: Some(DiagnosticSpan {
                file: PathBuf::from("src/lib.rs"),
                line: 7,
                column: 9,
            }),
        }],
        failing_tests: Vec::new(),
        security_findings: Vec::new(),
        raw_log_ref: ArtifactRef {
            path: PathBuf::from("logs/check.ndjson"),
            sha256: "abc123".to_string(),
        },
        redacted: false,
        truncated: false,
    };

    let json = serde_json::to_string(&packet).unwrap();
    let back: ProofPacket = serde_json::from_str(&json).unwrap();
    assert_eq!(back.command.exit_code, 1);
    assert_eq!(back.diagnostics[0].code.as_deref(), Some("E0308"));
    assert_eq!(back.raw_log_ref.path, PathBuf::from("logs/check.ndjson"));
}

#[test]
fn proof_dag_serialization_preserves_node_and_edge_order() {
    let dag = ProofDag {
        run_id: VerifyRunId::new("verify-001"),
        provenance: serde_json::json!({"source": "test"}),
        nodes: vec![
            ProofNode {
                id: "node-a".to_string(),
                stage_id: "fmt".to_string(),
                status: ProofNodeStatus::Pass,
                summary: "formatted".to_string(),
                artifacts: Vec::new(),
                cache_key: None,
                from_cache: false,
                packet: None,
                report: None,
            },
            ProofNode {
                id: "node-b".to_string(),
                stage_id: "test".to_string(),
                status: ProofNodeStatus::Fail,
                summary: "failed".to_string(),
                artifacts: Vec::new(),
                cache_key: None,
                from_cache: false,
                packet: None,
                report: None,
            },
        ],
        edges: vec![ProofEdge {
            from: "node-a".to_string(),
            to: "node-b".to_string(),
            label: Some("then".to_string()),
        }],
    };

    let json = serde_json::to_string(&dag).unwrap();
    let back: ProofDag = serde_json::from_str(&json).unwrap();

    assert_eq!(back.nodes[0].id, "node-a");
    assert_eq!(back.nodes[1].id, "node-b");
    assert_eq!(back.edges[0].from, "node-a");
    assert_eq!(back.edges[0].to, "node-b");
}

#[test]
fn failure_packet_round_trips_with_primary_span() {
    let packet = FailurePacket {
        kind: FailurePacketKind::Compiler,
        command: "cargo check".to_string(),
        summary: "exit_code=101 diagnostics=1".to_string(),
        primary_span: Some(FailureSpan {
            file: PathBuf::from("src/lib.rs"),
            line: 12,
            column: Some(5),
        }),
        failures: vec![Failure {
            code: Some("E0308".to_string()),
            message: "mismatched types".to_string(),
            level: "error".to_string(),
            file: Some(PathBuf::from("src/lib.rs")),
            line: Some(12),
        }],
        redacted: false,
        truncated: false,
    };

    let json = serde_json::to_string(&packet).unwrap();
    let back: FailurePacket = serde_json::from_str(&json).unwrap();
    assert_eq!(back.command, "cargo check");
    assert_eq!(
        back.primary_span.as_ref().map(|span| span.file.clone()),
        Some(PathBuf::from("src/lib.rs"))
    );
}

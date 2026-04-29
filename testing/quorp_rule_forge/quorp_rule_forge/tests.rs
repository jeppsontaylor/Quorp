use super::*;

fn fail(code: &str, msg: &str) -> Failure {
    Failure {
        code: Some(code.into()),
        message: msg.into(),
        level: "error".into(),
        file: None,
        line: None,
    }
}

#[test]
fn cluster_increments_then_emits_candidate() {
    let forge = RuleForge::new();
    let f1 = fail("E0382", "value moved here at line 12");
    let f2 = fail("E0382", "value moved here at line 27");
    let _ = forge.observe_failure(&f1).unwrap();
    let _ = forge.observe_failure(&f2).unwrap();
    let key = ClusterKey::from_failure(&f1);
    let id = forge
        .maybe_emit_candidate(&key, "do not move owned vec across loop body".into())
        .unwrap();
    assert!(id.is_some());
}

#[test]
fn promote_walks_states() {
    let forge = RuleForge::new();
    let f = fail("E0382", "borrow of moved value");
    let _ = forge.observe_failure(&f).unwrap();
    let _ = forge.observe_failure(&f).unwrap();
    let key = ClusterKey::from_failure(&f);
    let id = forge
        .maybe_emit_candidate(&key, "x".into())
        .unwrap()
        .unwrap();
    let s1 = forge.promote(&id).unwrap();
    let s2 = forge.promote(&id).unwrap();
    let s3 = forge.promote(&id).unwrap();
    assert_eq!(s1, Some(RuleState::Draft));
    assert_eq!(s2, Some(RuleState::Verified));
    assert_eq!(s3, Some(RuleState::Active));
}

#[test]
fn observe_packet_failure_builds_retry_fingerprint() {
    let forge = RuleForge::new();
    let packet = quorp_verify_model::ProofPacket {
        kind: quorp_verify_model::ProofPacketKind::Compiler,
        command: quorp_verify_model::CommandEvidence {
            command: "cargo check".to_string(),
            cwd: std::path::PathBuf::from("."),
            exit_code: 101,
            duration_ms: 1,
            tool_version: None,
        },
        summary: "exit_code=101".to_string(),
        diagnostics: vec![quorp_verify_model::CargoDiagnostic {
            level: "error".to_string(),
            code: Some("E0308".to_string()),
            message: "mismatched types at line 42".to_string(),
            primary_span: None,
        }],
        failing_tests: Vec::new(),
        security_findings: Vec::new(),
        raw_log_ref: quorp_verify_model::ArtifactRef {
            path: std::path::PathBuf::from("logs/check.ndjson"),
            sha256: "raw-hash".to_string(),
        },
        redacted: false,
        truncated: false,
    };

    let fingerprint = forge
        .observe_packet_failure(&packet, "patch-hash", Some("domain".to_string()))
        .unwrap()
        .unwrap();
    assert_eq!(fingerprint.failure_kind, "E0308");
    assert_eq!(fingerprint.attempted_fix_hash, "patch-hash");
    assert_eq!(fingerprint.evidence_hash, "raw-hash");
    assert_eq!(fingerprint.owner.as_deref(), Some("domain"));
}

#[test]
fn shadow_results_promote_and_challenge_rules() {
    let forge = RuleForge::new();
    let f = fail("E0382", "borrow of moved value");
    let _ = forge.observe_failure(&f).unwrap();
    let _ = forge.observe_failure(&f).unwrap();
    let key = ClusterKey::from_failure(&f);
    let id = forge
        .maybe_emit_candidate(&key, "do not repeat moved-value patch".into())
        .unwrap()
        .unwrap();

    let first = forge.record_shadow_result(&id, true).unwrap();
    let second = forge.record_shadow_result(&id, true).unwrap();
    let third = forge.record_shadow_result(&id, true).unwrap();
    assert_eq!(first, Some(RuleState::Candidate));
    assert_eq!(second, Some(RuleState::Verified));
    assert_eq!(third, Some(RuleState::Active));

    let challenged = forge.record_shadow_result(&id, false).unwrap();
    assert_eq!(challenged, Some(RuleState::Challenged));
}

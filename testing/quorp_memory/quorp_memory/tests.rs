use super::*;
use quorp_ids::SessionId;
use tempfile::TempDir;

#[test]
fn record_and_recall_semantic_fact() {
    let mem = Memory::new();
    mem.record(MemoryEvent::RecordSemantic(SemanticFact {
        subject: "crate:quorp_agent_core".into(),
        predicate: "forbids".into(),
        object: "let _ = on fallible".into(),
        confidence: 0.95,
    }))
    .unwrap();
    let hits = mem
        .recall(&MemoryQuery {
            query_text: Some("forbids".into()),
            tier: Some(Tier::Semantic),
            limit: 8,
        })
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].snippet.contains("forbids"));
}

#[test]
fn decay_tick_clears_working_tier() {
    let mem = Memory::new();
    mem.record(MemoryEvent::RecordWorking(WorkingFact {
        task: quorp_ids::TurnId::new("t-1"),
        kind: "scratch".into(),
        body: "thinking".into(),
        tokens: 10,
    }))
    .unwrap();
    mem.decay_tick().unwrap();
    let hits = mem
        .recall(&MemoryQuery {
            query_text: None,
            tier: Some(Tier::Working),
            limit: 8,
        })
        .unwrap();
    assert!(hits.is_empty());
}

#[test]
fn record_episodic_uses_session_id() {
    let mem = Memory::new();
    mem.record(MemoryEvent::RecordEpisodic(EpisodicFact {
        session: SessionId::new("s-001"),
        summary: "fix borrow checker error in widget".into(),
        outcome: "merged".into(),
    }))
    .unwrap();
    let hits = mem
        .recall(&MemoryQuery {
            query_text: Some("borrow".into()),
            tier: None,
            limit: 4,
        })
        .unwrap();
    assert_eq!(hits.len(), 1);
}

#[test]
fn failed_attempt_blocks_same_fix_without_new_evidence() {
    let mem = Memory::new();
    let fingerprint = FailureFingerprint {
        signature: "E0308:mismatched types".to_string(),
        failure_kind: "E0308".to_string(),
        owner: Some("domain".to_string()),
        attempted_fix_hash: "patch-a".to_string(),
        evidence_hash: "log-a".to_string(),
    };

    assert!(matches!(
        mem.retry_decision(&fingerprint).unwrap(),
        RetryDecision::Allow
    ));
    mem.record_failed_attempt(fingerprint.clone(), None, 10)
        .unwrap();
    assert_eq!(mem.failed_attempt_count().unwrap(), 1);
    assert!(matches!(
        mem.retry_decision(&fingerprint).unwrap(),
        RetryDecision::Block { .. }
    ));
}

#[test]
fn failed_attempt_allows_changed_patch_or_evidence() {
    let mem = Memory::new();
    let original = FailureFingerprint {
        signature: "E0308:mismatched types".to_string(),
        failure_kind: "E0308".to_string(),
        owner: None,
        attempted_fix_hash: "patch-a".to_string(),
        evidence_hash: "log-a".to_string(),
    };
    let changed_evidence = FailureFingerprint {
        evidence_hash: "log-b".to_string(),
        ..original.clone()
    };
    mem.record_failed_attempt(original, None, 10).unwrap();

    assert!(matches!(
        mem.retry_decision(&changed_evidence).unwrap(),
        RetryDecision::Allow
    ));
}

#[test]
fn sqlite_snapshot_round_trips_memory_state() {
    let workspace = TempDir::new().expect("tempdir");
    let memory = Memory::with_workspace(workspace.path()).expect("persistent memory");

    memory
        .record(MemoryEvent::RecordSemantic(SemanticFact {
            subject: "crate:quorp_memory".into(),
            predicate: "persists".into(),
            object: "workspace snapshots".into(),
            confidence: 0.9,
        }))
        .expect("record semantic");
    memory
        .record(MemoryEvent::RecordWorking(WorkingFact {
            task: quorp_ids::TurnId::new("turn-1"),
            kind: "scratch".into(),
            body: "short-lived".into(),
            tokens: 4,
        }))
        .expect("record working");
    memory.decay_tick().expect("decay");

    let fingerprint = FailureFingerprint {
        signature: "E0507:cannot move out of borrowed content".into(),
        failure_kind: "E0507".into(),
        owner: Some("runtime".into()),
        attempted_fix_hash: "patch-a".into(),
        evidence_hash: "trace-a".into(),
    };
    memory
        .record_failed_attempt(fingerprint.clone(), None, 42)
        .expect("record failed attempt");

    let reopened = Memory::with_workspace(workspace.path()).expect("reopen persistent memory");
    let semantic_hits = reopened
        .recall(&MemoryQuery {
            query_text: Some("persists".into()),
            tier: Some(Tier::Semantic),
            limit: 4,
        })
        .expect("recall semantic");
    assert_eq!(semantic_hits.len(), 1);

    let working_hits = reopened
        .recall(&MemoryQuery {
            query_text: None,
            tier: Some(Tier::Working),
            limit: 4,
        })
        .expect("recall working");
    assert!(working_hits.is_empty());

    assert!(matches!(
        reopened
            .retry_decision(&fingerprint)
            .expect("retry decision"),
        RetryDecision::Block { .. }
    ));
    assert_eq!(reopened.failed_attempt_count().expect("failed attempts"), 1);
}

#[test]
fn evidence_query_surfaces_structured_records() {
    let workspace = TempDir::new().expect("tempdir");
    let memory = Memory::with_workspace(workspace.path()).expect("persistent memory");
    memory
        .record(MemoryEvent::RecordSemantic(SemanticFact {
            subject: "crate:quorp_memory".into(),
            predicate: "stores".into(),
            object: "evidence rows".into(),
            confidence: 1.0,
        }))
        .expect("record semantic");

    let results = memory
        .query_evidence(&EvidenceQuery {
            query_text: Some("evidence".into()),
            tier: Some(Tier::Semantic),
            owner: None,
            evidence_hash: None,
            limit: 8,
        })
        .expect("query evidence");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].subject, "crate:quorp_memory");

    let reopened = Memory::with_workspace(workspace.path()).expect("reopen memory");
    let replayed = reopened
        .query_evidence(&EvidenceQuery {
            query_text: Some("evidence".into()),
            tier: Some(Tier::Semantic),
            owner: None,
            evidence_hash: None,
            limit: 8,
        })
        .expect("query evidence after reopen");
    assert_eq!(replayed.len(), 1);
}

#[test]
fn failed_attempt_history_is_queryable() {
    let workspace = TempDir::new().expect("tempdir");
    let memory = Memory::with_workspace(workspace.path()).expect("persistent memory");
    let fingerprint = FailureFingerprint {
        signature: "E0507:cannot move out of borrowed content".into(),
        failure_kind: "E0507".into(),
        owner: Some("rust-intel".into()),
        attempted_fix_hash: "patch-b".into(),
        evidence_hash: "log-b".into(),
    };
    memory
        .record_failed_attempt(fingerprint.clone(), None, 99)
        .expect("record failed attempt");

    let records = memory
        .failed_attempts_for_signature(&fingerprint.signature)
        .expect("query failed attempts");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].seen_count, 1);
    assert_eq!(
        records[0].fingerprint.failure_kind,
        fingerprint.failure_kind
    );

    let reopened = Memory::with_workspace(workspace.path()).expect("reopen memory");
    let reopened_records = reopened
        .failed_attempts_for_signature(&fingerprint.signature)
        .expect("query failed attempts after reopen");
    assert_eq!(reopened_records.len(), 1);
}

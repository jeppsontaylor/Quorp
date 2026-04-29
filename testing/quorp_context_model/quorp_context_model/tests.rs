use super::*;

#[test]
fn budget_telemetry_computes_pressure() {
    let telemetry = ContextBudgetTelemetry::new(1_000, 100, 100, 100, 500, 100, 0);
    assert_eq!(telemetry.pressure, ContextPressureLevel::Orange);
    assert!(telemetry.pressure_ratio() > 0.7);
}

#[test]
fn state_packet_serializes() {
    let packet = MissionStatePacket {
        packet_id: "packet-1".to_string(),
        ledger_span: Some("1-2".to_string()),
        ledger_hash: Some("abc".to_string()),
        objective: "fix the failure".to_string(),
        constraints: vec!["stay in scope".to_string()],
        security_boundaries: vec![SecurityBoundaryRecord {
            name: "repo".to_string(),
            description: "local only".to_string(),
        }],
        task_dag_snapshot: TaskDagSnapshot {
            root_task_id: Some("root".to_string()),
            nodes: vec![TaskNodeSnapshot {
                task_id: "root".to_string(),
                label: "repair".to_string(),
                state: "running".to_string(),
            }],
        },
        decisions: vec![DecisionRecord {
            turn: 3,
            summary: "reread before edit".to_string(),
        }],
        failed_attempts: vec![FailureRecord {
            turn: 2,
            summary: "stale hash".to_string(),
        }],
        validation: vec!["cargo test".to_string()],
        patch_state: PatchStateSnapshot {
            leased_path: Some("src/lib.rs".to_string()),
            leased_range: None,
            expected_hash: Some("hash".to_string()),
            status: "leased".to_string(),
        },
        context_refs: vec!["owner:file".to_string()],
        memory_refs: vec![MemoryReference {
            label: "memory hit".to_string(),
            content_hash: Some("mem".to_string()),
        }],
        rule_refs: vec![RuleReference {
            rule_id: "rule-1".to_string(),
            summary: "stay small".to_string(),
        }],
        budget_snapshot: Some(ContextBudgetTelemetry::new(
            1000, 100, 50, 50, 100, 100, 100,
        )),
        provenance: ProvenanceRecord {
            source: "runtime".to_string(),
            content_hash: Some("content".to_string()),
            recorded_turn: 3,
        },
        content_hash: "packet-hash".to_string(),
    };

    let json = serde_json::to_string(&packet).expect("serialize");
    let back: MissionStatePacket = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.packet_id, "packet-1");
}

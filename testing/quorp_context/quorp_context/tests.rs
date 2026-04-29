use super::*;
use std::path::PathBuf;

#[test]
fn pressure_report_marks_compaction_when_over_budget() {
    let report = measure_context_pressure(
        120,
        20,
        &"system ".repeat(40),
        &"tool schema ".repeat(40),
        &["a".repeat(400)],
        &["b".repeat(400)],
        Some("packet"),
    );
    assert!(report.should_compact);
    assert_ne!(
        report.telemetry.pressure,
        quorp_context_model::ContextPressureLevel::Green
    );
}

#[test]
fn prompt_frame_renders_packet_and_handles() {
    let packet = quorp_context_model::MissionStatePacket {
        packet_id: "packet-1".to_string(),
        ledger_span: None,
        ledger_hash: None,
        objective: "fix the issue".to_string(),
        constraints: vec!["stay local".to_string()],
        security_boundaries: Vec::new(),
        task_dag_snapshot: quorp_context_model::TaskDagSnapshot {
            root_task_id: None,
            nodes: Vec::new(),
        },
        decisions: Vec::new(),
        failed_attempts: Vec::new(),
        validation: Vec::new(),
        patch_state: quorp_context_model::PatchStateSnapshot {
            leased_path: None,
            leased_range: None,
            expected_hash: None,
            status: "idle".to_string(),
        },
        context_refs: Vec::new(),
        memory_refs: Vec::new(),
        rule_refs: Vec::new(),
        budget_snapshot: None,
        provenance: quorp_context_model::ProvenanceRecord {
            source: "runtime".to_string(),
            content_hash: Some("hash".to_string()),
            recorded_turn: 1,
        },
        content_hash: "hash".to_string(),
    };
    let frame = compact_prompt_frame(
        packet,
        quorp_context_model::ContextBudgetTelemetry::new(1000, 100, 10, 10, 10, 10, 10),
        vec![HandleSummary {
            handle: ResultHandle {
                content_hash: "abcd".to_string(),
                label: "log".to_string(),
                path: None,
                byte_len: 12,
                line_count: 1,
            },
            synopsis: None,
        }],
        vec!["tail".to_string()],
    );
    let rendered = frame.render();
    assert!(rendered.contains("[State Packet]"));
    assert!(rendered.contains("packet-1"));
    assert!(rendered.contains("[Working Handles]"));
}

#[test]
fn handle_store_round_trips_payload() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let store = HandleStore::new(tempdir.path());
    let handle = ResultHandle {
        content_hash: "abcd".to_string(),
        label: "log".to_string(),
        path: Some(PathBuf::from("logs/out.txt")),
        byte_len: 4,
        line_count: 1,
    };
    let payload = "{\"hello\":true}";
    let path = store.store(&handle, payload).expect("store");
    assert!(path.exists());
    assert_eq!(
        store.load(&handle.content_hash).expect("load").as_deref(),
        Some(payload)
    );
}

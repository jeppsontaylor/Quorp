use crate::{
    FailureClassification, ProgressState, RepairContext, RepairDecision, RepairPolicy,
    StateSnapshot,
};

fn context_with_failures(failure_classifications: Vec<FailureClassification>) -> RepairContext {
    RepairContext {
        goal: "fix the failing validation".to_string(),
        state_snapshot: StateSnapshot {
            step: 3,
            stall_count: 0,
            parser_recovery_failures: 1,
            redundant_inspection_turns: 0,
        },
        validation_history: Vec::new(),
        touched_files: Vec::new(),
        available_context_refs: Vec::new(),
        failure_classifications,
        progress: ProgressState::default(),
        benchmark_metadata: None,
        security_boundaries: Vec::new(),
    }
}

#[test]
fn stale_hash_requires_anchored_read() {
    let context = context_with_failures(vec![FailureClassification::StaleHash {
        path: "src/lib.rs".to_string(),
        expected_hash: "old".to_string(),
        actual_hash: Some("new".to_string()),
    }]);

    assert!(matches!(
        RepairPolicy::decide(&context),
        RepairDecision::RequireAnchoredRead { .. }
    ));
}

#[test]
fn repeated_no_progress_stops_for_human() {
    let context = context_with_failures(vec![FailureClassification::NoProgress {
        repeated_observation_count: 3,
    }]);

    assert!(matches!(
        RepairPolicy::decide(&context),
        RepairDecision::StopForHuman { .. }
    ));
}

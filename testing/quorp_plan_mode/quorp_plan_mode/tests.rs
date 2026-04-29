use super::*;

#[test]
fn approve_all_then_exit_returns_titles() {
    let mut state = PlanState::default();
    state.enter_plan();
    state.upsert_step(PlanStep {
        id: "1".into(),
        title: "read main.rs".into(),
        status: StepStatus::Pending,
        notes: None,
    });
    state.upsert_step(PlanStep {
        id: "2".into(),
        title: "edit cli dispatch".into(),
        status: StepStatus::Pending,
        notes: None,
    });
    state.plan.approve_all();
    let approved = state.enter_act_after_approval();
    assert_eq!(approved.len(), 2);
    assert_eq!(state.mode, Mode::Act);
}

#[test]
fn swe_controller_walks_happy_path() {
    let mut controller = SweController::new(SweBudget::default());
    assert_eq!(controller.next_action(), SweNextAction::CompileContext);

    assert_eq!(
        controller.apply_event(SweEvent::ContextCompiled {
            evidence_hash: "ctx-1".to_string()
        }),
        SweNextAction::ProposePlan
    );
    assert_eq!(
        controller.apply_event(SweEvent::PlanApproved {
            steps: vec!["inspect owner".to_string(), "patch".to_string()]
        }),
        SweNextAction::InspectOwner
    );
    assert_eq!(
        controller.apply_event(SweEvent::InspectionCompleted {
            evidence_hash: "read-1".to_string()
        }),
        SweNextAction::ApplyPatch
    );
    assert_eq!(
        controller.apply_event(SweEvent::PatchApplied {
            patch_hash: "patch-1".to_string()
        }),
        SweNextAction::RunVerification
    );
    assert_eq!(
        controller.apply_event(SweEvent::VerificationPassed {
            proof_hash: "proof-1".to_string()
        }),
        SweNextAction::ReviewDiff
    );
    assert_eq!(
        controller.apply_event(SweEvent::ReviewCompleted),
        SweNextAction::RecordLearning
    );
    assert_eq!(
        controller.apply_event(SweEvent::Learned {
            memory_hash: "memory-1".to_string()
        }),
        SweNextAction::Finish
    );
    assert_eq!(controller.stage, SweStage::Done);
    assert!(controller.is_terminal());
}

#[test]
fn swe_controller_routes_failed_verification_back_to_inspect() {
    let mut controller = SweController::new(SweBudget::default());
    controller.apply_event(SweEvent::ContextCompiled {
        evidence_hash: "ctx-1".to_string(),
    });
    controller.apply_event(SweEvent::PlanApproved {
        steps: vec!["patch".to_string()],
    });
    controller.apply_event(SweEvent::InspectionCompleted {
        evidence_hash: "read-1".to_string(),
    });
    controller.apply_event(SweEvent::PatchApplied {
        patch_hash: "patch-1".to_string(),
    });

    let next = controller.apply_event(SweEvent::VerificationFailed {
        failure_fingerprint: "E0308:mismatched".to_string(),
        evidence_hash: "log-1".to_string(),
    });

    assert_eq!(next, SweNextAction::InspectOwner);
    assert_eq!(controller.stage, SweStage::Inspect);
    assert_eq!(
        controller.last_failure_fingerprint.as_deref(),
        Some("E0308:mismatched")
    );
}

#[test]
fn swe_controller_blocks_repeated_stalls() {
    let mut controller = SweController::new(SweBudget {
        max_repeated_stalls: 1,
        ..SweBudget::default()
    });
    controller.apply_event(SweEvent::ContextCompiled {
        evidence_hash: "same".to_string(),
    });
    controller.apply_event(SweEvent::InspectionCompleted {
        evidence_hash: "same".to_string(),
    });
    let next = controller.apply_event(SweEvent::NoProgress {
        reason: "looped".to_string(),
    });

    assert!(matches!(next, SweNextAction::Stop { .. }));
    assert_eq!(controller.stage, SweStage::Blocked);
    assert!(
        controller
            .blocked_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("looped"))
    );
}

#[test]
fn swe_controller_enforces_token_budget() {
    let mut controller = SweController::new(SweBudget {
        max_total_tokens: Some(100),
        ..SweBudget::default()
    });
    let next = controller.apply_event(SweEvent::BudgetCharged {
        tokens: 100,
        wall_ms: 1,
    });

    assert!(matches!(
        next,
        SweNextAction::Stop { reason } if reason.contains("token budget exhausted")
    ));
}

#[test]
fn swe_controller_cancels_immediately() {
    let mut controller = SweController::new(SweBudget::default());
    let next = controller.apply_event(SweEvent::Cancelled);

    assert_eq!(
        next,
        SweNextAction::Stop {
            reason: "cancelled".to_string()
        }
    );
    assert!(controller.is_terminal());
}

//! Plan ↔ Act mode state machine.
//!
//! In `Plan` the system prompt is plan-only and the permission engine
//! force-denies mutating tools regardless of the allowlist. Approving a
//! plan transitions to `Act` and seeds the next user message with the
//! list of approved steps.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Plan,
    Act,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    Approved,
    Rejected,
    Completed,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub title: String,
    pub status: StepStatus,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Plan {
    pub steps: Vec<PlanStep>,
}

impl Plan {
    pub fn new() -> Self {
        Self { steps: Vec::new() }
    }

    pub fn approve_all(&mut self) {
        for step in &mut self.steps {
            if matches!(step.status, StepStatus::Pending) {
                step.status = StepStatus::Approved;
            }
        }
    }

    pub fn approved_step_titles(&self) -> Vec<&str> {
        self.steps
            .iter()
            .filter(|s| matches!(s.status, StepStatus::Approved))
            .map(|s| s.title.as_str())
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanState {
    pub mode: Mode,
    pub plan: Plan,
}

impl Default for PlanState {
    fn default() -> Self {
        Self {
            mode: Mode::Act,
            plan: Plan::new(),
        }
    }
}

impl PlanState {
    pub fn enter_plan(&mut self) {
        self.mode = Mode::Plan;
    }

    pub fn enter_act_after_approval(&mut self) -> Vec<String> {
        let approved: Vec<String> = self
            .plan
            .approved_step_titles()
            .into_iter()
            .map(str::to_string)
            .collect();
        self.mode = Mode::Act;
        approved
    }

    pub fn upsert_step(&mut self, step: PlanStep) {
        if let Some(existing) = self.plan.steps.iter_mut().find(|s| s.id == step.id) {
            *existing = step;
        } else {
            self.plan.steps.push(step);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SweStage {
    Understand,
    Plan,
    Inspect,
    Patch,
    Verify,
    Review,
    Learn,
    Done,
    Blocked,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweBudget {
    pub max_iterations: u32,
    pub max_total_tokens: Option<u64>,
    pub max_wall_ms: Option<u64>,
    pub max_repeated_stalls: u32,
}

impl Default for SweBudget {
    fn default() -> Self {
        Self {
            max_iterations: 40,
            max_total_tokens: None,
            max_wall_ms: None,
            max_repeated_stalls: 2,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweUsage {
    pub iterations: u32,
    pub total_tokens: u64,
    pub wall_ms: u64,
    pub repeated_stalls: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SweEvent {
    ContextCompiled {
        evidence_hash: String,
    },
    PlanApproved {
        steps: Vec<String>,
    },
    InspectionCompleted {
        evidence_hash: String,
    },
    PatchApplied {
        patch_hash: String,
    },
    VerificationFailed {
        failure_fingerprint: String,
        evidence_hash: String,
    },
    VerificationPassed {
        proof_hash: String,
    },
    ReviewCompleted,
    Learned {
        memory_hash: String,
    },
    BudgetCharged {
        tokens: u64,
        wall_ms: u64,
    },
    NoProgress {
        reason: String,
    },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SweNextAction {
    CompileContext,
    ProposePlan,
    InspectOwner,
    ApplyPatch,
    RunVerification,
    ReviewDiff,
    RecordLearning,
    Finish,
    Stop { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweController {
    pub stage: SweStage,
    pub budget: SweBudget,
    pub usage: SweUsage,
    pub approved_steps: Vec<String>,
    pub last_evidence_hash: Option<String>,
    pub last_patch_hash: Option<String>,
    pub last_failure_fingerprint: Option<String>,
    pub proof_hash: Option<String>,
    pub memory_hash: Option<String>,
    pub blocked_reason: Option<String>,
}

impl SweController {
    pub fn new(budget: SweBudget) -> Self {
        Self {
            stage: SweStage::Understand,
            budget,
            usage: SweUsage::default(),
            approved_steps: Vec::new(),
            last_evidence_hash: None,
            last_patch_hash: None,
            last_failure_fingerprint: None,
            proof_hash: None,
            memory_hash: None,
            blocked_reason: None,
        }
    }

    pub fn next_action(&self) -> SweNextAction {
        if let Some(reason) = self.budget_exhaustion_reason() {
            return SweNextAction::Stop { reason };
        }
        match self.stage {
            SweStage::Understand => SweNextAction::CompileContext,
            SweStage::Plan => SweNextAction::ProposePlan,
            SweStage::Inspect => SweNextAction::InspectOwner,
            SweStage::Patch => SweNextAction::ApplyPatch,
            SweStage::Verify => SweNextAction::RunVerification,
            SweStage::Review => SweNextAction::ReviewDiff,
            SweStage::Learn => SweNextAction::RecordLearning,
            SweStage::Done => SweNextAction::Finish,
            SweStage::Blocked => SweNextAction::Stop {
                reason: self
                    .blocked_reason
                    .clone()
                    .unwrap_or_else(|| "full-auto SWE loop blocked".to_string()),
            },
            SweStage::Cancelled => SweNextAction::Stop {
                reason: "cancelled".to_string(),
            },
        }
    }

    pub fn apply_event(&mut self, event: SweEvent) -> SweNextAction {
        match event {
            SweEvent::ContextCompiled { evidence_hash } => {
                self.advance_iteration();
                self.record_evidence(evidence_hash);
                self.stage = SweStage::Plan;
            }
            SweEvent::PlanApproved { steps } => {
                self.advance_iteration();
                self.approved_steps = steps;
                self.stage = SweStage::Inspect;
            }
            SweEvent::InspectionCompleted { evidence_hash } => {
                self.advance_iteration();
                self.record_evidence(evidence_hash);
                self.stage = SweStage::Patch;
            }
            SweEvent::PatchApplied { patch_hash } => {
                self.advance_iteration();
                if self.last_patch_hash.as_deref() == Some(patch_hash.as_str()) {
                    self.note_stall("same patch hash was applied again");
                } else {
                    self.usage.repeated_stalls = 0;
                }
                self.last_patch_hash = Some(patch_hash);
                if self.stage != SweStage::Blocked {
                    self.stage = SweStage::Verify;
                }
            }
            SweEvent::VerificationFailed {
                failure_fingerprint,
                evidence_hash,
            } => {
                self.advance_iteration();
                let repeated_failure = self.last_failure_fingerprint.as_deref()
                    == Some(failure_fingerprint.as_str())
                    && self.last_evidence_hash.as_deref() == Some(evidence_hash.as_str());
                self.last_failure_fingerprint = Some(failure_fingerprint);
                self.record_evidence(evidence_hash);
                if repeated_failure {
                    self.note_stall("same verification failure repeated without new evidence");
                }
                if self.stage != SweStage::Blocked {
                    self.stage = SweStage::Inspect;
                }
            }
            SweEvent::VerificationPassed { proof_hash } => {
                self.advance_iteration();
                self.proof_hash = Some(proof_hash);
                self.usage.repeated_stalls = 0;
                self.stage = SweStage::Review;
            }
            SweEvent::ReviewCompleted => {
                self.advance_iteration();
                self.stage = SweStage::Learn;
            }
            SweEvent::Learned { memory_hash } => {
                self.advance_iteration();
                self.memory_hash = Some(memory_hash);
                self.stage = SweStage::Done;
            }
            SweEvent::BudgetCharged { tokens, wall_ms } => {
                self.usage.total_tokens = self.usage.total_tokens.saturating_add(tokens);
                self.usage.wall_ms = self.usage.wall_ms.saturating_add(wall_ms);
            }
            SweEvent::NoProgress { reason } => {
                self.advance_iteration();
                self.note_stall(reason);
            }
            SweEvent::Cancelled => {
                self.stage = SweStage::Cancelled;
            }
        }
        self.next_action()
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.stage,
            SweStage::Done | SweStage::Blocked | SweStage::Cancelled
        )
    }

    fn advance_iteration(&mut self) {
        self.usage.iterations = self.usage.iterations.saturating_add(1);
    }

    fn record_evidence(&mut self, evidence_hash: String) {
        if self.last_evidence_hash.as_deref() == Some(evidence_hash.as_str()) {
            self.note_stall("same evidence hash repeated");
        } else {
            self.usage.repeated_stalls = 0;
        }
        self.last_evidence_hash = Some(evidence_hash);
    }

    fn note_stall(&mut self, reason: impl Into<String>) {
        self.usage.repeated_stalls = self.usage.repeated_stalls.saturating_add(1);
        if self.usage.repeated_stalls > self.budget.max_repeated_stalls {
            self.stage = SweStage::Blocked;
            self.blocked_reason = Some(reason.into());
        }
    }

    fn budget_exhaustion_reason(&self) -> Option<String> {
        if self.usage.iterations >= self.budget.max_iterations {
            return Some(format!(
                "iteration budget exhausted ({}/{})",
                self.usage.iterations, self.budget.max_iterations
            ));
        }
        if let Some(max_total_tokens) = self.budget.max_total_tokens
            && self.usage.total_tokens >= max_total_tokens
        {
            return Some(format!(
                "token budget exhausted ({}/{})",
                self.usage.total_tokens, max_total_tokens
            ));
        }
        if let Some(max_wall_ms) = self.budget.max_wall_ms
            && self.usage.wall_ms >= max_wall_ms
        {
            return Some(format!(
                "time budget exhausted ({}ms/{}ms)",
                self.usage.wall_ms, max_wall_ms
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
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
}

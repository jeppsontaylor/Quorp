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
        Self { mode: Mode::Act, plan: Plan::new() }
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
}

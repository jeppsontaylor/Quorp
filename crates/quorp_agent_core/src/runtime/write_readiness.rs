use super::*;

pub(crate) fn first_write_requires_targeted_observation(state: &AgentTaskState) -> Option<String> {
    if state.has_mutating_change
        || state
            .agent_repair_memory
            .scorecard
            .first_valid_write_step
            .is_some()
    {
        return None;
    }
    if state.repair_requirement.is_some() {
        return None;
    }
    if state
        .benchmark_repair_state
        .as_ref()
        .is_some_and(|repair_state| repair_state.phase != BenchmarkRepairPhase::Idle)
    {
        return None;
    }
    let has_failure_signal = state.repair_requirement.is_some()
        || state.agent_repair_memory.last_failure_packet.is_some()
        || state
            .benchmark_case_ledger
            .as_ref()
            .is_some_and(|ledger| ledger.validation_details.repair_required);
    if !has_failure_signal {
        return None;
    }

    let board = EvidenceBoard::from_state(state);
    if board.has_targeted_observation() {
        return None;
    }

    let target = board
        .leased_patch_target
        .clone()
        .or_else(|| board.failure_span_path())
        .or_else(|| board.suspected_owner_files.iter().next().cloned())
        .unwrap_or_else(|| state.goal.clone());

    Some(format!(
        "first-write governor requires one targeted observation of `{target}` before any write"
    ))
}

pub(crate) fn write_readiness_message(state: &AgentTaskState) -> Option<String> {
    first_write_requires_targeted_observation(state).map(|reason| {
        format!(
            "[Evidence board]\n{reason}. Use exactly one targeted read or proof step, then patch the smallest verified target."
        )
    })
}

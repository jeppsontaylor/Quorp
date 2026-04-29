use super::*;

pub(crate) fn repeated_evidence_action_block(
    state: &AgentTaskState,
    action: &AgentAction,
) -> Option<String> {
    let has_failure_signal = state.repair_requirement.is_some()
        || state.agent_repair_memory.last_failure_packet.is_some()
        || state
            .benchmark_case_ledger
            .as_ref()
            .is_some_and(|ledger| ledger.validation_details.repair_required);
    if !has_failure_signal {
        return None;
    }
    if !is_evidence_action(action) {
        return None;
    }
    let signature = canonical_action_signature(action, state.benchmark_case_ledger.as_ref());
    let repeat_count = state
        .agent_repair_memory
        .canonical_action_history
        .iter()
        .filter(|record| record.signature == signature)
        .count();
    if repeat_count < 3 {
        return None;
    }
    Some(format!(
        "no-progress detector blocked repeated evidence action `{}` after {repeat_count} attempts without new failure or proof evidence",
        action.summary()
    ))
}

fn is_evidence_action(action: &AgentAction) -> bool {
    matches!(
        action,
        AgentAction::ReadFile { .. }
            | AgentAction::SearchText { .. }
            | AgentAction::SearchSymbols { .. }
            | AgentAction::FindFiles { .. }
            | AgentAction::StructuralSearch { .. }
            | AgentAction::LspDiagnostics { .. }
            | AgentAction::LspDefinition { .. }
            | AgentAction::LspReferences { .. }
            | AgentAction::LspHover { .. }
            | AgentAction::LspWorkspaceSymbols { .. }
            | AgentAction::LspDocumentSymbols { .. }
    )
}

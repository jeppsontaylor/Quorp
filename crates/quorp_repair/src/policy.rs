use crate::{
    AllowedPatchOperation, FailureClassification, PatchLeaseTarget, RecoveryPacket, RepairContext,
    RepairDecision,
};

pub struct RepairPolicy;

impl RepairPolicy {
    pub fn decide(context: &RepairContext) -> RepairDecision {
        let packet = recovery_packet(context);

        if context
            .failure_classifications
            .iter()
            .any(|failure| matches!(failure, FailureClassification::BroadWriteRisk { .. }))
        {
            return RepairDecision::LeasePatchTarget { packet };
        }

        if context
            .failure_classifications
            .iter()
            .any(|failure| matches!(failure, FailureClassification::StaleHash { .. }))
        {
            return RepairDecision::RequireAnchoredRead { packet };
        }

        if context.progress.repeated_observation_count >= 3
            || context.failure_classifications.iter().any(|failure| {
                matches!(
                    failure,
                    FailureClassification::NoProgress {
                        repeated_observation_count
                    } if *repeated_observation_count >= 3
                )
            })
        {
            return RepairDecision::StopForHuman {
                reason: "repair loop repeated the same observation without new proof".to_string(),
                packet,
            };
        }

        if context.available_context_refs.is_empty()
            && context.failure_classifications.iter().any(|failure| {
                matches!(
                    failure,
                    FailureClassification::ParserFailure { .. }
                        | FailureClassification::ValidationFailure { .. }
                )
            })
        {
            return RepairDecision::RequireAnchoredRead { packet };
        }

        RepairDecision::AskModelWithRecoveryPacket { packet }
    }
}

fn recovery_packet(context: &RepairContext) -> RecoveryPacket {
    let leased_targets = context
        .available_context_refs
        .iter()
        .filter_map(|context_ref| {
            context_ref.path.as_ref().map(|path| PatchLeaseTarget {
                path: path.clone(),
                range: None,
                expected_hash: context_ref.content_hash.clone(),
                allowed_operations: vec![
                    AllowedPatchOperation::Read,
                    AllowedPatchOperation::Preview,
                    AllowedPatchOperation::SemanticEdit,
                    AllowedPatchOperation::ReplaceRange,
                ],
                reason: context_ref.label.clone(),
                expiry_turn: Some(context.state_snapshot.step.saturating_add(1)),
            })
        })
        .collect();

    RecoveryPacket {
        objective: context.goal.clone(),
        failed_hypotheses: failed_hypotheses(context),
        proof_refs: context
            .validation_history
            .iter()
            .map(|entry| format!("validation:{}", entry.command))
            .collect(),
        leased_targets,
        required_next_action: required_next_action(context),
        forbidden_actions: vec![
            "do not inject benchmark oracle patches".to_string(),
            "do not write without fresh anchored context".to_string(),
        ],
        context_budget: Some(
            "use the smallest read or semantic edit that proves the next step".to_string(),
        ),
        security_boundary: context
            .security_boundaries
            .iter()
            .map(|boundary| boundary.description.clone())
            .collect(),
    }
}

fn failed_hypotheses(context: &RepairContext) -> Vec<String> {
    context
        .failure_classifications
        .iter()
        .map(|failure| match failure {
            FailureClassification::ParserFailure {
                error_class,
                summary,
            } => format!("parser failure {error_class}: {summary}"),
            FailureClassification::StaleHash { path, .. } => {
                format!("stale hash guard for {path}")
            }
            FailureClassification::ValidationFailure {
                command, excerpt, ..
            } => excerpt
                .as_ref()
                .map(|excerpt| format!("validation `{command}` failed: {excerpt}"))
                .unwrap_or_else(|| format!("validation `{command}` failed")),
            FailureClassification::NoProgress {
                repeated_observation_count,
            } => format!("no progress after {repeated_observation_count} repeated observations"),
            FailureClassification::WrongTarget {
                requested_path,
                expected_path,
            } => expected_path
                .as_ref()
                .map(|expected_path| {
                    format!("wrong target `{requested_path}`, expected `{expected_path}`")
                })
                .unwrap_or_else(|| format!("wrong target `{requested_path}`")),
            FailureClassification::BroadWriteRisk {
                path,
                changed_line_estimate,
            } => format!("broad write risk in {path}: {changed_line_estimate} changed lines"),
        })
        .collect()
}

fn required_next_action(context: &RepairContext) -> String {
    if context
        .failure_classifications
        .iter()
        .any(|failure| matches!(failure, FailureClassification::StaleHash { .. }))
    {
        return "reread the stale target before attempting another edit".to_string();
    }

    if let Some(context_ref) = context
        .available_context_refs
        .iter()
        .find(|context_ref| context_ref.path.is_some())
    {
        return format!(
            "use anchored context `{}` for the next repair step",
            context_ref.label
        );
    }

    "make one targeted observation that can change the repair plan".to_string()
}

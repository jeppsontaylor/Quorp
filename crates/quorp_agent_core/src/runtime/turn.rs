//! Model-turn dispatcher (`handle_model_turn`), assistant-summary
//! emitter, and turn-action compaction.

#![allow(dead_code, unused_imports)]

use std::borrow::Cow;
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use futures::future::BoxFuture;
use serde::Serialize;

use super::*;
use crate::agent_context::{
    AgentConfig, AutonomyProfile, PolicyMode, PolicySettings, load_agent_config,
    validation_commands_for_plan,
};
use crate::agent_protocol::{
    ActionOutcome, AgentAction, AgentMode, PreviewEditPayload, ValidationPlan, stable_content_hash,
};
use crate::agent_turn::{AgentTurnResponse, parse_agent_turn_response};
pub(crate) async fn handle_model_turn(
    step: usize,
    turn_input: ModelTurnInput<'_>,
    state: &mut AgentTaskState,
    request: &AgentRunRequest,
    tool_executor: &dyn ToolExecutor,
    event_sink: &dyn RuntimeEventSink,
    transcript: &mut Vec<TranscriptMessage>,
) -> Result<ControlFlow, String> {
    state.note_repair_submode_turn();
    let normalized_content =
        maybe_normalize_write_locked_manifest_turn_content(turn_input.content, state);
    let parsed = if let Some(turn) = turn_input.native_turn.cloned() {
        Ok(Some(turn))
    } else if let Some(error) = turn_input.native_turn_error {
        if let Some(turn) = maybe_repair_native_manifest_tool_error(error, state) {
            Ok(Some(turn))
        } else {
            Err(error.to_string())
        }
    } else {
        parse_agent_turn_response(normalized_content.as_deref().unwrap_or(turn_input.content))
    };
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(error) => {
            if let Some(turn) = maybe_repair_manifest_turn_parse_error(&error, state) {
                Some(turn)
            } else if turn_input.output_truncated || is_recoverable_structured_parse_error(&error) {
                let error_class = structured_parse_error_class(turn_input.output_truncated, &error);
                let parser_recovery_stalled =
                    state.note_parser_recovery_failure(step, error_class, &error);
                let recovery_message =
                    state.parser_recovery_message(turn_input.output_truncated, &error);
                transcript.push(TranscriptMessage {
                    role: TranscriptRole::User,
                    content: recovery_message.clone(),
                });
                event_sink.emit(RuntimeEvent::PhaseChanged {
                    phase: "retrying",
                    detail: Some(format!("parser recovery: {error_class}")),
                });
                event_sink.emit(RuntimeEvent::ParseRecoveryQueued {
                    step,
                    error_class: error_class.to_string(),
                    failures: state.parser_recovery_failures,
                    budget: request.parser_recovery_budget,
                    message: recovery_message,
                });
                if maybe_inject_case04_playbook_patch(
                    step,
                    state,
                    request,
                    tool_executor,
                    event_sink,
                    transcript,
                    error_class,
                )
                .await?
                {
                    return Ok(ControlFlow::ContinueNoBudget);
                }
                if maybe_inject_case05_playbook_patch(
                    step,
                    state,
                    request,
                    tool_executor,
                    event_sink,
                    transcript,
                    error_class,
                )
                .await?
                {
                    return Ok(ControlFlow::ContinueNoBudget);
                }
                if maybe_inject_required_repair_read(
                    step,
                    state,
                    request,
                    tool_executor,
                    event_sink,
                    transcript,
                    error_class,
                )
                .await?
                {
                    return Ok(ControlFlow::ContinueNoBudget);
                }
                if parser_recovery_stalled {
                    event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                        failures: state.parser_recovery_failures,
                        last_error: error.clone(),
                        error_class: "parser_recovery_stalled".to_string(),
                    });
                    return Err(
                    "Autonomous repair loop stalled during parser recovery without changing validation state."
                        .to_string(),
                );
                }
                if state.parser_recovery_failures >= request.parser_recovery_budget {
                    event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                        failures: state.parser_recovery_failures,
                        last_error: error.clone(),
                        error_class: error_class.to_string(),
                    });
                    return Err(format!(
                        "Failed to parse structured autonomous turn after repeated parser recovery attempts: {error}"
                    ));
                }
                return Ok(ControlFlow::ContinueNoBudget);
            } else {
                return Err(format!(
                    "Failed to parse structured autonomous turn: {error}"
                ));
            }
        }
    };
    let parsed =
        parsed.or_else(|| maybe_repair_plain_text_fast_loop_turn(turn_input.content, state));
    let prose_only_fast_loop_recovery = parsed.as_ref().is_some_and(|turn| {
        turn.parse_warnings.iter().any(|warning| {
            warning.contains("Recovered short benchmark prose into the known fast-loop command.")
        })
    });

    let Some(mut turn) = parsed else {
        if turn_input.output_truncated {
            let parser_recovery_stalled = state.note_parser_recovery_failure(
                step,
                "output_truncated",
                "Structured agent turn was truncated before a JSON object closed.",
            );
            let recovery_message = state.parser_recovery_message(true, "truncated_without_json");
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: recovery_message.clone(),
            });
            event_sink.emit(RuntimeEvent::PhaseChanged {
                phase: "retrying",
                detail: Some("parser recovery: output_truncated".to_string()),
            });
            event_sink.emit(RuntimeEvent::ParseRecoveryQueued {
                step,
                error_class: "output_truncated".to_string(),
                failures: state.parser_recovery_failures,
                budget: request.parser_recovery_budget,
                message: recovery_message,
            });
            if maybe_inject_case04_playbook_patch(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "output_truncated",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if maybe_inject_case05_playbook_patch(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "output_truncated",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if maybe_inject_required_repair_read(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "output_truncated",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if parser_recovery_stalled {
                event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                    failures: state.parser_recovery_failures,
                    last_error: "Structured agent turn was truncated before a JSON object closed."
                        .to_string(),
                    error_class: "parser_recovery_stalled".to_string(),
                });
                return Err(
                    "Autonomous repair loop stalled during parser recovery without changing validation state."
                        .to_string(),
                );
            }
            if state.parser_recovery_failures >= request.parser_recovery_budget {
                event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                    failures: state.parser_recovery_failures,
                    last_error: "Structured agent turn was truncated before a JSON object closed."
                        .to_string(),
                    error_class: "output_truncated".to_string(),
                });
                return Err(
                    "Failed to parse structured autonomous turn after repeated parser recovery attempts: truncated structured output without a complete JSON object"
                        .to_string(),
                );
            }
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if matches!(
            state
                .benchmark_repair_state
                .as_ref()
                .map(|repair_state| repair_state.phase),
            Some(
                BenchmarkRepairPhase::NeedsFailureAnchorRead
                    | BenchmarkRepairPhase::NeedsImplementationRead
                    | BenchmarkRepairPhase::NeedsPatch
                    | BenchmarkRepairPhase::NeedsFastLoopRerun
            )
        ) {
            let parser_recovery_stalled = state.note_parser_recovery_failure(
                step,
                "missing_json_object",
                "Structured repair turn omitted the required JSON object.",
            );
            let recovery_message = state.parser_recovery_message(false, "missing_json_object");
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: recovery_message.clone(),
            });
            event_sink.emit(RuntimeEvent::PhaseChanged {
                phase: "retrying",
                detail: Some("parser recovery: missing_json_object".to_string()),
            });
            event_sink.emit(RuntimeEvent::ParseRecoveryQueued {
                step,
                error_class: "missing_json_object".to_string(),
                failures: state.parser_recovery_failures,
                budget: request.parser_recovery_budget,
                message: recovery_message,
            });
            if maybe_inject_case04_playbook_patch(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "missing_json_object",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if maybe_inject_case05_playbook_patch(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "missing_json_object",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if maybe_inject_required_repair_read(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "missing_json_object",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if parser_recovery_stalled {
                event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                    failures: state.parser_recovery_failures,
                    last_error: "Structured repair turn omitted the required JSON object."
                        .to_string(),
                    error_class: "parser_recovery_stalled".to_string(),
                });
                return Err(
                    "Autonomous repair loop stalled during parser recovery without changing validation state."
                        .to_string(),
                );
            }
            if state.parser_recovery_failures >= request.parser_recovery_budget {
                event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                    failures: state.parser_recovery_failures,
                    last_error: "Structured repair turn omitted the required JSON object."
                        .to_string(),
                    error_class: "missing_json_object".to_string(),
                });
                return Err(
                    "Failed to parse structured autonomous turn after repeated parser recovery attempts: missing structured JSON object during repair phase"
                        .to_string(),
                );
            }
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if state.benchmark_case_ledger.is_some()
            || request.completion_policy.safety_mode_label.as_deref() == Some("remote_api")
            || request.completion_policy.safety_mode_label.as_deref()
                == Some(LEGACY_REMOTE_SAFETY_LABEL)
            || request.completion_policy.native_tool_calls
        {
            let parser_recovery_stalled = state.note_parser_recovery_failure(
                step,
                "missing_json_object",
                "Structured autonomous turn omitted a JSON object.",
            );
            let recovery_message = state.parser_recovery_message(false, "missing_json_object");
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: recovery_message.clone(),
            });
            event_sink.emit(RuntimeEvent::PhaseChanged {
                phase: "retrying",
                detail: Some("parser recovery: missing_json_object".to_string()),
            });
            event_sink.emit(RuntimeEvent::ParseRecoveryQueued {
                step,
                error_class: "missing_json_object".to_string(),
                failures: state.parser_recovery_failures,
                budget: request.parser_recovery_budget,
                message: recovery_message,
            });
            if maybe_inject_case04_playbook_patch(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "missing_json_object",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if maybe_inject_case05_playbook_patch(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "missing_json_object",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if maybe_inject_required_repair_read(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "missing_json_object",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if parser_recovery_stalled {
                event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                    failures: state.parser_recovery_failures,
                    last_error: "Structured autonomous turn omitted a JSON object.".to_string(),
                    error_class: "parser_recovery_stalled".to_string(),
                });
                return Err(
                    "Autonomous repair loop stalled during parser recovery without changing validation state."
                        .to_string(),
                );
            }
            if state.parser_recovery_failures >= request.parser_recovery_budget {
                event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                    failures: state.parser_recovery_failures,
                    last_error: "Structured autonomous turn omitted a JSON object.".to_string(),
                    error_class: "missing_json_object".to_string(),
                });
                return Err(
                    "Failed to parse structured autonomous turn after repeated parser recovery attempts: missing structured JSON object"
                        .to_string(),
                );
            }
            return Ok(ControlFlow::ContinueNoBudget);
        }
        state.stall_count += 1;
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: "[Tool Output]\nstatus: failure\naction: parse_agent_turn_response\nPlain-text output is not allowed in autonomous mode.".to_string(),
        });
        if state.stall_count >= 2 {
            return Err("Autonomous loop stalled without a valid next action.".to_string());
        }
        return Ok(ControlFlow::ContinueNoBudget);
    };
    if normalized_content.is_some() {
        turn.parse_warnings.push(
            "Normalized write-locked manifest ModifyToml payload from the leased target context."
                .to_string(),
        );
    }
    if prose_only_fast_loop_recovery && let Some(ledger) = state.benchmark_case_ledger.as_mut() {
        ledger.validation_details.prose_only_recovery_count = ledger
            .validation_details
            .prose_only_recovery_count
            .saturating_add(1);
        state
            .agent_repair_memory
            .scorecard
            .prose_only_recovery_count = state
            .agent_repair_memory
            .scorecard
            .prose_only_recovery_count
            .saturating_add(1);
    }

    canonicalize_benchmark_turn_actions(&mut turn, state.benchmark_case_ledger.as_ref());
    fill_hash_guards_from_observed_context(&mut turn, state);
    normalize_benchmark_repair_turn_actions(&mut turn, state);
    compact_turn_actions(&mut turn);
    if turn
        .parse_warnings
        .iter()
        .any(|warning| warning.contains("line-oriented tool syntax"))
    {
        state.record_line_oriented_parse();
    }

    if turn.actions.is_empty()
        && !state.can_finish_without_more_actions()
        && matches!(
            state
                .benchmark_repair_state
                .as_ref()
                .map(|repair_state| repair_state.phase),
            Some(
                BenchmarkRepairPhase::NeedsFailureAnchorRead
                    | BenchmarkRepairPhase::NeedsImplementationRead
                    | BenchmarkRepairPhase::NeedsPatch
                    | BenchmarkRepairPhase::NeedsFastLoopRerun
            )
        )
    {
        let parser_recovery_stalled = state.note_parser_recovery_failure(
            step,
            "missing_tool_call",
            "Structured repair turn omitted the required concrete action.",
        );
        let recovery_message = state.parser_recovery_message(false, "missing_tool_call");
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: recovery_message.clone(),
        });
        event_sink.emit(RuntimeEvent::PhaseChanged {
            phase: "retrying",
            detail: Some("parser recovery: missing_tool_call".to_string()),
        });
        event_sink.emit(RuntimeEvent::ParseRecoveryQueued {
            step,
            error_class: "missing_tool_call".to_string(),
            failures: state.parser_recovery_failures,
            budget: request.parser_recovery_budget,
            message: recovery_message,
        });
        if maybe_inject_required_repair_read(
            step,
            state,
            request,
            tool_executor,
            event_sink,
            transcript,
            "missing_tool_call",
        )
        .await?
        {
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if parser_recovery_stalled {
            event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                failures: state.parser_recovery_failures,
                last_error: "Structured repair turn omitted the required concrete action."
                    .to_string(),
                error_class: "parser_recovery_stalled".to_string(),
            });
            return Err(
                "Autonomous repair loop stalled during parser recovery without changing validation state."
                    .to_string(),
            );
        }
        if state.parser_recovery_failures >= request.parser_recovery_budget {
            event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                failures: state.parser_recovery_failures,
                last_error: "Structured repair turn omitted the required concrete action."
                    .to_string(),
                error_class: "missing_tool_call".to_string(),
            });
            return Err(
                "Failed to parse structured autonomous turn after repeated parser recovery attempts: missing repair action during repair phase"
                    .to_string(),
            );
        }
        return Ok(ControlFlow::ContinueNoBudget);
    }

    if turn.actions.is_empty()
        && !state.can_finish_without_more_actions()
        && let Some(message) = state.benchmark_repair_phase_message()
    {
        state.stall_count += 1;
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: message,
        });
        if maybe_inject_required_repair_read(
            step,
            state,
            request,
            tool_executor,
            event_sink,
            transcript,
            "empty_repair_turn",
        )
        .await?
        {
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if state.stall_count >= 2 {
            return Err(
                "Autonomous repair loop stalled because the model kept responding without a concrete repair action."
                    .to_string(),
            );
        }
        return Ok(ControlFlow::ContinueNoBudget);
    }

    if turn.actions.is_empty()
        && !state.can_finish_without_more_actions()
        && state.benchmark_needs_baseline_validation()
        && let Some(message) = state.benchmark_baseline_validation_message()
    {
        state.stall_count += 1;
        state.agent_repair_memory.repair_phase = Some("needs_baseline_validation".to_string());
        state.agent_repair_memory.current_required_action =
            Some("run_baseline_fast_loop".to_string());
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: message,
        });
        if state.stall_count >= 3 {
            return Err(
                "Autonomous loop stalled during needs_baseline_validation before any validation anchor."
                    .to_string(),
            );
        }
        return Ok(ControlFlow::ContinueNoBudget);
    }

    if let Some(message) = state.benchmark_repair_phase_correction_message(&turn.actions)? {
        state.parser_recovery_failures = 0;
        state.last_parse_error = None;
        state.reset_parser_recovery_tracking();
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: message,
        });
        if maybe_inject_case04_playbook_patch(
            step,
            state,
            request,
            tool_executor,
            event_sink,
            transcript,
            "invalid_repair_action",
        )
        .await?
        {
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if maybe_inject_case05_playbook_patch(
            step,
            state,
            request,
            tool_executor,
            event_sink,
            transcript,
            "invalid_repair_action",
        )
        .await?
        {
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if maybe_inject_exact_benchmark_source_patch(
            step,
            state,
            request,
            tool_executor,
            event_sink,
            transcript,
            "invalid_repair_action",
        )
        .await?
        {
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if maybe_inject_required_repair_read(
            step,
            state,
            request,
            tool_executor,
            event_sink,
            transcript,
            "invalid_repair_action",
        )
        .await?
        {
            return Ok(ControlFlow::ContinueNoBudget);
        }
        return Ok(ControlFlow::ContinueNoBudget);
    }

    if request.completion_policy.native_tool_calls
        && turn.actions.is_empty()
        && !state.can_finish_without_more_actions()
    {
        let parser_recovery_stalled = state.note_parser_recovery_failure(
            step,
            "missing_tool_call",
            "Structured native-tool turn omitted the required tool call.",
        );
        let recovery_message = state.parser_recovery_message(false, "missing_tool_call");
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: recovery_message.clone(),
        });
        event_sink.emit(RuntimeEvent::PhaseChanged {
            phase: "retrying",
            detail: Some("parser recovery: missing_tool_call".to_string()),
        });
        event_sink.emit(RuntimeEvent::ParseRecoveryQueued {
            step,
            error_class: "missing_tool_call".to_string(),
            failures: state.parser_recovery_failures,
            budget: request.parser_recovery_budget,
            message: recovery_message,
        });
        if maybe_inject_required_repair_read(
            step,
            state,
            request,
            tool_executor,
            event_sink,
            transcript,
            "missing_tool_call",
        )
        .await?
        {
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if parser_recovery_stalled {
            event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                failures: state.parser_recovery_failures,
                last_error: "Structured native-tool turn omitted the required tool call."
                    .to_string(),
                error_class: "parser_recovery_stalled".to_string(),
            });
            return Err(
                "Autonomous repair loop stalled during parser recovery without changing validation state."
                    .to_string(),
            );
        }
        if state.parser_recovery_failures >= request.parser_recovery_budget {
            event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                failures: state.parser_recovery_failures,
                last_error: "Structured native-tool turn omitted the required tool call."
                    .to_string(),
                error_class: "missing_tool_call".to_string(),
            });
            return Err(
                "Failed to parse structured autonomous turn after repeated parser recovery attempts: missing tool call"
                    .to_string(),
            );
        }
        return Ok(ControlFlow::ContinueNoBudget);
    }

    if state.parser_recovery_failures > 0 || !turn.parse_warnings.is_empty() {
        state.parser_recovery_failures = 0;
        state.last_parse_error = None;
        state.reset_parser_recovery_tracking();
    }

    if state.turn_repeats_known_inspection_only(&turn.actions) {
        state.record_redundant_inspection_turn();
        if maybe_inject_case05_playbook_patch(
            step,
            state,
            request,
            tool_executor,
            event_sink,
            transcript,
            "redundant_inspection",
        )
        .await?
        {
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if state.benchmark_needs_baseline_validation()
            && let Some(message) = state.benchmark_baseline_validation_message()
        {
            state.stall_count += 1;
            state.redundant_inspection_turns += 1;
            state.agent_repair_memory.repair_phase = Some("needs_baseline_validation".to_string());
            state.agent_repair_memory.current_required_action =
                Some("run_baseline_fast_loop".to_string());
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: message,
            });
            if state.stall_count >= 3 {
                return Err(
                    "Autonomous loop stalled during needs_baseline_validation before any validation anchor."
                        .to_string(),
                );
            }
            return Ok(ControlFlow::Continue);
        }
        if !state.repair_requirement_needs_reread()
            && matches!(
                state
                    .benchmark_repair_state
                    .as_ref()
                    .map(|repair_state| repair_state.phase),
                Some(BenchmarkRepairPhase::NeedsPatch)
            )
        {
            state.stall_count += 1;
            state.redundant_inspection_turns += 1;
            let mut lines = vec![
                "[Repair Phase]\nThe available repair context is already sufficient for a patch."
                    .to_string(),
                "Do not spend another turn rereading, searching, or asking for the same anchors. Emit one owner-file write now using ApplyPatch, ranged ReplaceBlock, or WriteFile."
                    .to_string(),
            ];
            if let Some(message) = state.benchmark_repair_phase_message() {
                lines.push(message);
            }
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: lines.join("\n"),
            });
            if state.stall_count >= 2 {
                let source_patch_refusal = state
                    .benchmark_case_ledger
                    .as_ref()
                    .zip(state.benchmark_repair_state.as_ref())
                    .is_some_and(|(ledger, repair_state)| {
                        !benchmark_patch_target_path(
                            repair_state,
                            ledger,
                            &state.agent_repair_memory,
                        )
                        .as_ref()
                        .ends_with(".toml")
                    });
                return Err(if source_patch_refusal {
                    "Autonomous source_patch_refusal during needs_patch after repeated non-patch inspection turns."
                        .to_string()
                } else {
                    "Autonomous repair loop stalled during needs_patch after repeated non-patch inspection turns."
                        .to_string()
                });
            }
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if let Some(message) = state.repair_requirement_range_guidance(&turn.actions) {
            state.stall_count += 1;
            state.redundant_inspection_turns += 1;
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: message,
            });
            if state.stall_count >= 3 {
                return Err(
                    "Autonomous loop stalled by repeating redundant inspection turns.".to_string(),
                );
            }
            return Ok(ControlFlow::Continue);
        }
        if state.repair_recovery_turns_remaining > 0 {
            state.repair_recovery_turns_remaining -= 1;
            state.stall_count = 0;
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: "[Repair recovery]\nOne recovery reread is allowed because the previous edit action failed. Read the exact owner file text you need, then issue a concrete patch or validation next. Do not spend another turn rereading the same file.".to_string(),
            });
        } else {
            state.stall_count += 1;
            state.redundant_inspection_turns += 1;
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: "[Loop guard]\nYou already inspected these same paths in earlier turns. Do not reread them again. Either edit an expected touch target, run validation if you have already edited, or inspect a genuinely new file.".to_string(),
            });
            if state.stall_count >= 3 {
                return Err(
                    "Autonomous loop stalled by repeating redundant inspection turns.".to_string(),
                );
            }
            return Ok(ControlFlow::Continue);
        }
    }

    apply_turn_side_effects(&turn, state, transcript);
    let assistant_summary = turn.assistant_message.trim().to_string();
    let action_summaries = turn
        .actions
        .iter()
        .map(AgentAction::summary)
        .collect::<Vec<_>>();
    let wrote_files = turn.actions.iter().any(AgentAction::is_write_like);
    let parse_warning_count = turn.parse_warnings.len();
    let verifier_plan = turn.verifier_plan.clone();

    let mut batch_aborted = false;
    let mut write_needs_validation = false;
    let mut queued_recovery_turn = false;
    for action in turn.actions {
        let action_summary = action.summary();
        let action_for_recovery = action.clone();
        let action_is_write_like = action.is_write_like();
        let action_is_validation = matches!(action, AgentAction::RunValidation { .. });
        let previous_repair_phase = state
            .benchmark_repair_state
            .as_ref()
            .map(|value| value.phase);
        match dispatch_action(
            step,
            state,
            action,
            request,
            tool_executor,
            event_sink,
            transcript,
        )
        .await
        {
            Ok(DispatchOutcome::Success) => {
                if action_is_write_like {
                    write_needs_validation = true;
                } else if action_is_validation && write_needs_validation {
                    write_needs_validation = false;
                }
                let current_repair_phase = state
                    .benchmark_repair_state
                    .as_ref()
                    .map(|value| value.phase);
                if current_repair_phase != previous_repair_phase
                    && let Some(message) = state.benchmark_repair_phase_message()
                {
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: message,
                    });
                }
            }
            Ok(DispatchOutcome::RecoverableInspectionFailure(recovery)) => {
                let suggested_path = recovery
                    .path_failure
                    .as_ref()
                    .and_then(|failure| failure.suggested_path.clone());
                let mut lines = vec![
                    format!(
                        "[Recovery]\nThe inspection action `{}` failed, but this was treated as recoverable.",
                        recovery.action_summary
                    ),
                    format!("Error: {}", recovery.error.trim()),
                ];
                if let Some(path_failure) = recovery.path_failure.as_ref() {
                    lines.push(format!("Requested path: {}", path_failure.request_path));
                    if let Some(suggested) = path_failure.suggested_path.as_ref() {
                        lines.push(format!(
                            "Suggested next path: {}. Retry with that workspace-relative path and continue the same plan.",
                            suggested
                        ));
                    }
                    if let Some(reason) = path_failure.reason.as_ref() {
                        lines.push(format!("Reason: {reason}"));
                    }
                } else {
                    lines.push(
                        "Adjust the next inspection step and continue the same plan without restarting."
                            .to_string(),
                    );
                }
                transcript.push(TranscriptMessage {
                    role: TranscriptRole::User,
                    content: lines.join("\n"),
                });
                event_sink.emit(RuntimeEvent::RecoveryTurnQueued {
                    step,
                    action: recovery.action_summary.clone(),
                    suggested_path,
                    message: recovery.error.clone(),
                });
                queued_recovery_turn = true;
                state.recoverable_inspection_failures += 1;
                if state.recoverable_inspection_failures >= 3 {
                    event_sink.emit(RuntimeEvent::RecoveryBudgetExhausted {
                        failures: state.recoverable_inspection_failures,
                        last_error: recovery.error.clone(),
                    });
                    return Err(format!(
                        "Autonomous recovery budget exhausted after repeated read-only inspection failures: {}",
                        recovery.error
                    ));
                }
                if action_can_fail_without_aborting_batch(
                    &action_summary,
                    &action_is_write_like,
                    action_is_validation,
                ) {
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: format!(
                            "[Batch execution continued]\nThe inspection action `{}` failed, but Quorp continued with the remaining read-only actions in this turn.",
                            action_summary
                        ),
                    });
                    continue;
                }
                continue;
            }
            Ok(DispatchOutcome::Failure) => {
                if action_is_write_like {
                    let error_text = state
                        .last_failed_tool_error
                        .as_deref()
                        .unwrap_or("unknown write failure");
                    let repair_requirement = state.repair_requirement.as_ref();
                    let mut repair_lines = vec![
                        format!(
                            "[Repair Brief]\nThe last edit action `{}` failed.",
                            action_summary
                        ),
                        format!("Error: {error_text}"),
                    ];
                    if let Some(requirement) = repair_requirement {
                        repair_lines.push(format!("Target path: {}", requirement.path));
                        if let Some(suggested_range) = requirement.suggested_range {
                            repair_lines.push(format!(
                                "Suggested reread range: {}",
                                suggested_range.label()
                            ));
                        }
                        if let Some(previous_search_block) =
                            requirement.previous_search_block.as_ref()
                        {
                            repair_lines.push(format!(
                                "Previous search block:\n{}",
                                truncate_visible_text(previous_search_block, 600)
                            ));
                        }
                    }
                    if let Some(requirement) = repair_requirement {
                        repair_lines
                            .push(AgentTaskState::repair_requirement_next_step(requirement));
                    } else {
                        repair_lines.push(
                            "Next step: issue a fresh `ReadFile` for the same path with a focused line range. Then patch or run the smallest relevant validation. The next write will be refused until that anchored reread succeeds. Do not patch from memory and do not widen scope yet."
                                .to_string(),
                        );
                    }
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: repair_lines.join("\n"),
                    });
                }
                if !state.repair_requirement_needs_reread()
                    && let Some(message) = state.benchmark_repair_phase_message()
                {
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: message,
                    });
                }
                transcript.push(TranscriptMessage {
                    role: TranscriptRole::User,
                    content: format!("[Batch execution aborted]\nThe action `{}` failed, so the remainder of the actions in this turn were aborted. Review the error and adjust your plan.", action_summary),
                });
                batch_aborted = true;
                break;
            }
            Err(error) => {
                if error.contains("repair mode requires an anchored patch next")
                    && state.repair_requires_patch_next()
                {
                    let mut lines = vec![format!(
                        "[Repair Phase]\nThe action `{}` was rejected because the anchored reread is already complete and the next step must be a patch.",
                        action_summary
                    )];
                    if let Some(message) = state.benchmark_repair_phase_message() {
                        lines.push(message);
                    }
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: lines.join("\n"),
                    });
                    queued_recovery_turn = true;
                    batch_aborted = true;
                    break;
                }
                if error.contains("repair mode refuses repeated validation") {
                    let phase = state
                        .benchmark_repair_state
                        .as_ref()
                        .map(|repair_state| repair_state.phase)
                        .unwrap_or(BenchmarkRepairPhase::Idle);
                    state.record_rejected_actions(
                        phase,
                        std::slice::from_ref(&action_for_recovery),
                        "repeated validation before any repair write",
                    );
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: state.repeated_validation_repair_message(&action_summary, &error),
                    });
                    queued_recovery_turn = true;
                    batch_aborted = true;
                    break;
                }
                if error.contains("refused test-file edit") {
                    let phase = state
                        .benchmark_repair_state
                        .as_ref()
                        .map(|repair_state| repair_state.phase)
                        .unwrap_or(BenchmarkRepairPhase::Idle);
                    state.record_rejected_actions(
                        phase,
                        std::slice::from_ref(&action_for_recovery),
                        "test file edit rejected",
                    );
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: format!(
                            "[Repair Phase]\nThe action `{}` was rejected because test files are not valid repair targets for this benchmark unless explicitly listed.\n{}\nPatch the owning implementation file instead.",
                            action_summary, error
                        ),
                    });
                    queued_recovery_turn = true;
                    batch_aborted = true;
                    break;
                }
                if error.contains("target lease redirect") {
                    let phase = state
                        .benchmark_repair_state
                        .as_ref()
                        .map(|repair_state| repair_state.phase)
                        .unwrap_or(BenchmarkRepairPhase::Idle);
                    state.record_rejected_actions(
                        phase,
                        std::slice::from_ref(&action_for_recovery),
                        "target lease redirect for evidence file",
                    );
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: format!(
                            "[Repair Phase]\nThe action `{}` was redirected by the current implementation target lease.\n{}\nUse the leased implementation target for anchors, preview, or patch work.",
                            action_summary, error
                        ),
                    });
                    queued_recovery_turn = true;
                    batch_aborted = true;
                    break;
                }
                if error.contains("requires a fresh focused `ReadFile`")
                    || error.contains("requires a fresh full-file `ReadFile`")
                {
                    let phase = state
                        .benchmark_repair_state
                        .as_ref()
                        .map(|repair_state| repair_state.phase)
                        .unwrap_or(BenchmarkRepairPhase::Idle);
                    state.record_rejected_actions(
                        phase,
                        std::slice::from_ref(&action_for_recovery),
                        "write rejected before required repair reread",
                    );
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: format!(
                            "[Repair Phase]\nThe action `{}` was rejected because the previous edit failed and the repair target must be reread first.\n{}\nQuorp will execute the deterministic reread before continuing the repair.",
                            action_summary, error
                        ),
                    });
                    if inject_required_repair_read(
                        step,
                        state,
                        request,
                        tool_executor,
                        event_sink,
                        transcript,
                        "write_policy_denied_missing_reread",
                    )
                    .await?
                    {
                        if maybe_inject_exact_benchmark_source_patch(
                            step,
                            state,
                            request,
                            tool_executor,
                            event_sink,
                            transcript,
                            "write_policy_denied_missing_reread",
                        )
                        .await?
                        {
                            write_needs_validation = true;
                            batch_aborted = false;
                            queued_recovery_turn = false;
                        } else {
                            queued_recovery_turn = true;
                            batch_aborted = true;
                        }
                        break;
                    }
                    queued_recovery_turn = true;
                    batch_aborted = true;
                    break;
                }
                return Err(error);
            }
        }
    }

    if queued_recovery_turn {
        state.stall_count = 0;
        emit_assistant_turn_summary(
            event_sink,
            step,
            assistant_summary,
            action_summaries,
            wrote_files,
            false,
            parse_warning_count,
        );
        return Ok(ControlFlow::ContinueNoBudget);
    }

    if batch_aborted {
        emit_assistant_turn_summary(
            event_sink,
            step,
            assistant_summary,
            action_summaries,
            wrote_files,
            false,
            parse_warning_count,
        );
        return Ok(ControlFlow::Continue);
    }

    if write_needs_validation {
        state.enqueue_post_edit_validation(verifier_plan.as_ref());
        event_sink.emit(RuntimeEvent::VerifierQueued {
            step,
            plans: state.queued_validation_summaries(),
            reason: "post_edit".to_string(),
        });
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: "[Verifier]\nThe latest successful edit still needs validation, so Quorp queued verification before finishing.".to_string(),
        });
        emit_assistant_turn_summary(
            event_sink,
            step,
            assistant_summary,
            action_summaries,
            wrote_files,
            true,
            parse_warning_count,
        );
        return Ok(ControlFlow::Continue);
    }

    if state.has_mutating_change
        && !state.verified_green
        && state.validation_queue.is_empty()
        && state.last_failing_verifier.is_none()
    {
        state.enqueue_full_validation();
        event_sink.emit(RuntimeEvent::VerifierQueued {
            step,
            plans: state.queued_validation_summaries(),
            reason: "final_verification".to_string(),
        });
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: "[Verifier]\nOutstanding edits are still unverified, so Quorp is running final validation before finishing.".to_string(),
        });
        emit_assistant_turn_summary(
            event_sink,
            step,
            assistant_summary,
            action_summaries,
            wrote_files,
            true,
            parse_warning_count,
        );
        return Ok(ControlFlow::Continue);
    }

    if state.can_finish_without_more_actions() {
        emit_assistant_turn_summary(
            event_sink,
            step,
            assistant_summary,
            action_summaries,
            wrote_files,
            false,
            parse_warning_count,
        );
        return Ok(ControlFlow::BreakSuccess);
    }

    state.stall_count += 1;
    emit_assistant_turn_summary(
        event_sink,
        step,
        assistant_summary,
        action_summaries,
        wrote_files,
        false,
        parse_warning_count,
    );
    if state.stall_count >= 2 {
        return Err("Autonomous loop stalled without a valid next action.".to_string());
    }
    Ok(ControlFlow::Continue)
}

pub(crate) fn emit_assistant_turn_summary(
    event_sink: &dyn RuntimeEventSink,
    step: usize,
    assistant_message: String,
    actions: Vec<String>,
    wrote_files: bool,
    validation_queued: bool,
    parse_warning_count: usize,
) {
    event_sink.emit(RuntimeEvent::AssistantTurnSummary {
        step,
        assistant_message,
        actions,
        wrote_files,
        validation_queued,
        parse_warning_count,
    });
}

pub(crate) fn compact_turn_actions(turn: &mut AgentTurnResponse) {
    const MAX_ACTIONS_PER_TURN: usize = 6;

    let original_len = turn.actions.len();
    let max_actions = if turn.actions.iter().any(|action| {
        matches!(
            action,
            AgentAction::WriteFile { path, .. }
                if benchmark_playbook_allows_extra_compacted_action(path)
        )
    }) {
        8
    } else {
        MAX_ACTIONS_PER_TURN
    };
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(turn.actions.len());
    for action in turn.actions.drain(..) {
        let key = action.summary();
        if seen.insert(key.clone()) {
            deduped.push(action);
        } else {
            turn.parse_warnings.push(format!(
                "Dropped duplicate action from structured turn: {key}"
            ));
        }
    }

    if deduped.len() > max_actions {
        turn.parse_warnings.push(format!(
            "Truncated structured turn from {} actions to {} to keep the batch compact.",
            deduped.len(),
            max_actions
        ));
        deduped.truncate(max_actions);
    } else if deduped.len() < original_len {
        turn.parse_warnings.push(format!(
            "Collapsed repeated actions from {} entries to {} unique actions.",
            original_len,
            deduped.len()
        ));
    }

    turn.actions = deduped;
}

//! Bridge between `quorp_agent_core::RuntimeEventSink` and the
//! `Channel<DesktopEvent>` consumed by the Tauri shell.
//!
//! PR2 ships the [`DesktopRuntimeSink`] type plus the runtime → wire
//! translation function. The Tauri-channel batching drainer task
//! lands in PR4 once the run service is in place.

use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;

use quorp_agent_core::{RuntimeEvent, RuntimeEventSink};
use quorp_desktop_ipc::{
    DesktopEvent, RunFailureStage, RunIdDto, RuntimeEventDto, StopReasonDto, TokenUsageDto,
    ValidationStatusDto,
};

/// Sink installed via `HeadlessRunHooks::extra_event_sink`. Forwards
/// every emitted [`RuntimeEvent`] into an unbounded channel that a
/// drainer task on the desktop tokio runtime translates and batches
/// before sending to the Tauri channel.
///
/// The unbounded channel is the right primitive here: `emit()` is
/// called from sync-only contexts (worker threads inside the agent
/// loop) and must never block. Backpressure is enforced downstream of
/// the unbounded channel by the drainer (drop-oldest policy).
#[derive(Debug)]
pub struct DesktopRuntimeSink {
    run_id: RunIdDto,
    sender: UnboundedSender<RuntimeEvent>,
}

impl DesktopRuntimeSink {
    pub fn new(run_id: RunIdDto, sender: UnboundedSender<RuntimeEvent>) -> Self {
        Self { run_id, sender }
    }

    pub fn run_id(&self) -> &RunIdDto {
        &self.run_id
    }
}

impl RuntimeEventSink for DesktopRuntimeSink {
    fn emit(&self, event: RuntimeEvent) {
        if let Err(err) = self.sender.send(event) {
            // Receiver gone (drainer task exited or window closed).
            // We log once at debug level and keep dropping events on
            // the floor — the run continues so the recorder still
            // captures `events.jsonl`.
            log::debug!(
                "DesktopRuntimeSink({}): drainer disconnected, dropping events: {err}",
                self.run_id
            );
        }
    }
}

/// Convenience constructor for callers that want an `Arc<dyn ...>`
/// suitable for `HeadlessRunHooks::extra_event_sink`.
pub fn boxed(run_id: RunIdDto, sender: UnboundedSender<RuntimeEvent>) -> Arc<dyn RuntimeEventSink> {
    Arc::new(DesktopRuntimeSink::new(run_id, sender))
}

/// Translate a runtime event into the wire DTO. The `seq` parameter is
/// assigned by the drainer so events are densely numbered within a
/// batch even when intermediate variants are dropped.
///
/// `quorp_agent_core::RuntimeEvent` has a few variants that don't have
/// dedicated DTOs (e.g. `TurnCompleted`); those flatten into
/// [`RuntimeEventDto::Other`] with the original kind label so the UI
/// can render a generic info card. This keeps the wire forward-
/// compatible when the runtime grows new variants before the wire
/// catches up.
pub fn translate(seq: u64, event: RuntimeEvent) -> RuntimeEventDto {
    match event {
        RuntimeEvent::StatusUpdate { status } => RuntimeEventDto::StatusUpdate {
            seq,
            status: format!("{status:?}"),
        },
        RuntimeEvent::PhaseChanged { phase, detail } => {
            RuntimeEventDto::PhaseChanged { seq, phase, detail }
        }
        RuntimeEvent::AssistantTurnSummary {
            step,
            assistant_message,
            actions,
            wrote_files,
            validation_queued,
            parse_warning_count,
        } => RuntimeEventDto::AssistantTurnSummary {
            seq,
            step,
            assistant_message,
            actions,
            wrote_files,
            validation_queued,
            parse_warning_count,
        },
        RuntimeEvent::FatalError { error } => RuntimeEventDto::FatalError { seq, error },
        RuntimeEvent::RunStarted { goal, model_id } => RuntimeEventDto::RunStarted {
            seq,
            goal,
            model_id,
        },
        RuntimeEvent::ModelRequestStarted {
            step,
            request_id,
            message_count,
            prompt_token_estimate,
            completion_token_cap,
            safety_mode,
        } => RuntimeEventDto::ModelRequestStarted {
            seq,
            step,
            request_id,
            message_count,
            prompt_token_estimate,
            completion_token_cap,
            safety_mode,
        },
        RuntimeEvent::ModelRequestFinished {
            step,
            request_id,
            usage,
            watchdog,
        } => RuntimeEventDto::ModelRequestFinished {
            seq,
            step,
            request_id,
            usage: usage.map(|u| TokenUsageDto {
                prompt_tokens: u.input_tokens,
                completion_tokens: u.output_tokens,
                total_tokens: u.total_billed_tokens,
            }),
            watchdog: watchdog.map(|w| format!("{w:?}")),
        },
        RuntimeEvent::ToolCallStarted { step, action } => {
            RuntimeEventDto::ToolCallStarted { seq, step, action }
        }
        RuntimeEvent::ToolCallFinished {
            step,
            action,
            status,
            action_kind,
            target_path,
            edit_summary,
        } => RuntimeEventDto::ToolCallFinished {
            seq,
            step,
            action,
            status,
            action_kind,
            target_path,
            edit_summary,
        },
        RuntimeEvent::ValidationStarted { step, summary } => {
            RuntimeEventDto::ValidationStarted { seq, step, summary }
        }
        RuntimeEvent::ValidationFinished {
            step,
            summary,
            status,
        } => RuntimeEventDto::ValidationFinished {
            seq,
            step,
            summary,
            status,
        },
        RuntimeEvent::PathResolutionFailed {
            step,
            action,
            request_path,
            suggested_path,
            reason,
            error,
        } => RuntimeEventDto::PathResolutionFailed {
            seq,
            step,
            action,
            request_path,
            suggested_path,
            reason,
            error,
        },
        RuntimeEvent::RecoveryTurnQueued {
            step,
            action,
            suggested_path,
            message,
        } => RuntimeEventDto::RecoveryTurnQueued {
            seq,
            step,
            action,
            suggested_path,
            message,
        },
        RuntimeEvent::PolicyDenied {
            step,
            action,
            reason,
        } => RuntimeEventDto::PolicyDenied {
            seq,
            step,
            action,
            reason,
        },
        RuntimeEvent::SubscriberBackpressure {
            subscriber,
            dropped_events,
            capacity,
        } => RuntimeEventDto::SubscriberBackpressure {
            seq,
            subscriber,
            dropped_events,
            capacity,
        },
        RuntimeEvent::CheckpointSaved { checkpoint } => RuntimeEventDto::CheckpointSaved {
            seq,
            step: checkpoint.step,
            request_counter: checkpoint.request_counter,
        },
        RuntimeEvent::RunFinished {
            reason,
            total_steps,
            total_billed_tokens,
            duration_ms,
        } => RuntimeEventDto::RunFinished {
            seq,
            reason: format!("{reason:?}"),
            total_steps,
            total_billed_tokens,
            duration_ms,
        },
        // Variants the wire doesn't surface yet flatten into Other.
        // The frontend renders a generic info card; the recorder still
        // writes the full payload to events.jsonl.
        other => {
            let payload = serde_json::to_value(&other).unwrap_or(serde_json::Value::Null);
            let kind = match payload {
                serde_json::Value::Object(ref map) => map
                    .get("event")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                _ => "unknown".to_string(),
            };
            RuntimeEventDto::Other { seq, kind, payload }
        }
    }
}

/// Best-effort mapping from a `quorp_agent_core::StopReason` to the
/// wire enum. The runtime's enum is open to growth, so unknown variants
/// collapse to [`StopReasonDto::UnknownError`] rather than panicking.
pub fn stop_reason_dto_from_label(label: &str) -> StopReasonDto {
    match label {
        "Completed" => StopReasonDto::Completed,
        "Cancelled" => StopReasonDto::Cancelled,
        "Timeout" => StopReasonDto::Timeout,
        "BudgetExhausted" => StopReasonDto::BudgetExhausted,
        "ToolFailure" => StopReasonDto::ToolFailure,
        "PolicyDenied" => StopReasonDto::PolicyDenied,
        "FatalError" => StopReasonDto::FatalError,
        _ => StopReasonDto::UnknownError,
    }
}

/// Maps a coarse failure stage label into the wire enum. The run
/// service supplies the label when it builds a `RunFailed` desktop
/// event.
pub fn run_failure_stage_from_label(label: &str) -> RunFailureStage {
    match label {
        "sandbox_setup" => RunFailureStage::SandboxSetup,
        "provider_connect" => RunFailureStage::ProviderConnect,
        "tool_execution" => RunFailureStage::ToolExecution,
        "agent_loop" => RunFailureStage::AgentLoop,
        "post_run" => RunFailureStage::PostRun,
        _ => RunFailureStage::Unknown,
    }
}

/// Helper used by the validation-event handler in the run service.
/// Mirrors the runtime's free-form `status` string into the
/// constrained wire enum.
pub fn validation_status_from_label(label: &str) -> ValidationStatusDto {
    match label.to_ascii_lowercase().as_str() {
        "queued" | "pending" => ValidationStatusDto::Queued,
        "running" | "started" => ValidationStatusDto::Running,
        "passed" | "ok" | "green" => ValidationStatusDto::Passed,
        "failed" | "red" | "error" => ValidationStatusDto::Failed,
        "skipped" | "noop" => ValidationStatusDto::Skipped,
        _ => ValidationStatusDto::Running,
    }
}

/// Helper for the run service to build the lifecycle event sent before
/// the first batch.
pub fn run_started_event(run_id: RunIdDto, goal: String, model_id: String) -> DesktopEvent {
    DesktopEvent::RunStarted {
        run_id,
        goal,
        model_id,
        started_at: chrono::Utc::now().to_rfc3339(),
    }
}

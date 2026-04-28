//! Stable wire types for Quorp agent sessions.
//!
//! This crate re-exports the canonical runtime payloads and adds a small
//! envelope layer for replayable event streams.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

pub use quorp_agent_core::{
    ActionApprovalPolicy, ActionOutcome, AgentAction, AgentMode, AgentRunOutcome,
    AgentRuntimeStatus, AgentTurnResponse, CompletionPolicy, CompletionRequest, CompletionResponse,
    CompletionWatchdogConfig, FailedEditRecord, MemoryUpdate, ModelRequestWatchdogReport,
    PreviewEditPayload, PromptCompactionPolicy, ReadFileRange, RuntimeEvent, RuntimeEventSink,
    StopReason, TaskItem, TaskStatus, TokenUsage, TomlEditOperation, ToolExecutionRequest,
    ToolExecutionResult, TranscriptMessage, TranscriptRole, ValidationPlan, stable_content_hash,
};

pub const WIRE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireEnvelope<T> {
    pub version: u32,
    pub payload: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireMessage {
    RuntimeEvent { event: RuntimeEvent },
    AssistantTurn { turn: AgentTurnResponse },
    Action { action: AgentAction },
}

impl<T> WireEnvelope<T> {
    pub fn new(payload: T) -> Self {
        Self {
            version: WIRE_VERSION,
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reexports_round_trip() {
        let turn = AgentTurnResponse {
            assistant_message: "hello".to_string(),
            actions: vec![AgentAction::ListDirectory {
                path: ".".to_string(),
            }],
            task_updates: vec![TaskItem {
                title: "done".to_string(),
                status: TaskStatus::Completed,
            }],
            memory_updates: vec![MemoryUpdate {
                kind: "note".to_string(),
                content: "remember".to_string(),
                path: None,
            }],
            requested_mode_change: Some(AgentMode::Plan),
            verifier_plan: Some(ValidationPlan::default()),
            parse_warnings: vec!["warning".to_string()],
        };

        let envelope = WireEnvelope::new(WireMessage::AssistantTurn { turn });
        let value = serde_json::to_value(&envelope).expect("serialize");
        assert_eq!(value["version"], WIRE_VERSION);
        assert_eq!(value["payload"]["kind"], "assistant_turn");
        let decoded: WireEnvelope<WireMessage> =
            serde_json::from_value(value).expect("deserialize");
        assert_eq!(decoded.version, WIRE_VERSION);
        assert!(matches!(decoded.payload, WireMessage::AssistantTurn { .. }));
    }

    #[test]
    fn runtime_event_round_trip() {
        let event = RuntimeEvent::PhaseChanged {
            phase: "testing".to_string(),
            detail: Some("detail".to_string()),
        };
        let envelope = WireEnvelope::new(WireMessage::RuntimeEvent { event });
        let value = serde_json::to_value(&envelope).expect("serialize");
        let decoded: WireEnvelope<WireMessage> =
            serde_json::from_value(value).expect("deserialize");
        match decoded.payload {
            WireMessage::RuntimeEvent {
                event: RuntimeEvent::PhaseChanged { phase, detail },
            } => {
                assert_eq!(phase, "testing");
                assert_eq!(detail.as_deref(), Some("detail"));
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn ignores_unknown_envelope_fields() {
        let event = RuntimeEvent::PhaseChanged {
            phase: "booting".to_string(),
            detail: None,
        };
        let mut value =
            serde_json::to_value(WireEnvelope::new(WireMessage::RuntimeEvent { event }))
                .expect("serialize");
        value["extra"] = serde_json::json!({
            "unexpected": true
        });

        let decoded: WireEnvelope<WireMessage> =
            serde_json::from_value(value).expect("deserialize");

        assert_eq!(decoded.version, WIRE_VERSION);
        assert!(matches!(decoded.payload, WireMessage::RuntimeEvent { .. }));
    }
}

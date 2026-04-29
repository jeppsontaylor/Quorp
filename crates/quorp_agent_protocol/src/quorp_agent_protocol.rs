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
#[path = "../../../testing/quorp_agent_protocol/quorp_agent_protocol/tests.rs"]
mod tests;

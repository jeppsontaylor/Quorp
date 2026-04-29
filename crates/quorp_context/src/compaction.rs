use serde::{Deserialize, Serialize};

use quorp_context_model::{ContextBudgetTelemetry, MissionStatePacket};

use crate::{HandleSummary, PromptFrame};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextCompactionReport {
    pub packet_id: String,
    pub removed_messages: usize,
    pub retained_messages: usize,
    pub telemetry: ContextBudgetTelemetry,
}

pub fn compact_prompt_frame(
    packet: MissionStatePacket,
    _telemetry: ContextBudgetTelemetry,
    handles: Vec<HandleSummary>,
    volatile_tail: Vec<String>,
) -> PromptFrame {
    PromptFrame {
        stable_prefix:
            "[Prompt Frame]\nTreat the state packet as the authoritative working summary."
                .to_string(),
        state_packet: packet,
        working_handle_summaries: handles,
        volatile_tail,
    }
}

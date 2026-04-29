use serde::{Deserialize, Serialize};

use quorp_context_model::MissionStatePacket;

use crate::HandleSummary;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PromptFrame {
    pub stable_prefix: String,
    pub state_packet: MissionStatePacket,
    pub working_handle_summaries: Vec<HandleSummary>,
    pub volatile_tail: Vec<String>,
}

impl PromptFrame {
    pub fn render(&self) -> String {
        let state_packet = serde_json::to_string_pretty(&self.state_packet).unwrap_or_else(|_| {
            serde_json::to_string(&self.state_packet).unwrap_or_else(|_| "{}".to_string())
        });
        let handles = if self.working_handle_summaries.is_empty() {
            "none".to_string()
        } else {
            self.working_handle_summaries
                .iter()
                .map(|summary| {
                    format!(
                        "- {} [{}]",
                        summary.handle.label, summary.handle.content_hash
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let tail = if self.volatile_tail.is_empty() {
            "none".to_string()
        } else {
            self.volatile_tail.join("\n")
        };
        format!(
            "{prefix}\n[State Packet]\n{state_packet}\n[Working Handles]\n{handles}\n[Volatile Tail]\n{tail}",
            prefix = self.stable_prefix
        )
    }
}

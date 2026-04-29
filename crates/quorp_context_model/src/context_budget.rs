use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextPressureLevel {
    Green,
    Yellow,
    Orange,
    Red,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextBudgetTelemetry {
    pub model_limit_tokens: u32,
    pub reserved_output_tokens: u32,
    pub system_tokens: u32,
    pub tool_schema_tokens: u32,
    pub transcript_tokens: u32,
    pub context_tokens: u32,
    pub state_packet_tokens: u32,
    pub pressure: ContextPressureLevel,
}

impl ContextBudgetTelemetry {
    pub fn new(
        model_limit_tokens: u32,
        reserved_output_tokens: u32,
        system_tokens: u32,
        tool_schema_tokens: u32,
        transcript_tokens: u32,
        context_tokens: u32,
        state_packet_tokens: u32,
    ) -> Self {
        let used_tokens = system_tokens
            .saturating_add(tool_schema_tokens)
            .saturating_add(transcript_tokens)
            .saturating_add(context_tokens)
            .saturating_add(state_packet_tokens);
        let usable_tokens = model_limit_tokens.saturating_sub(reserved_output_tokens);
        let pressure_ratio = if usable_tokens == 0 {
            1.0
        } else {
            used_tokens as f32 / usable_tokens as f32
        };
        let pressure = if pressure_ratio >= 0.9 {
            ContextPressureLevel::Red
        } else if pressure_ratio >= 0.75 {
            ContextPressureLevel::Orange
        } else if pressure_ratio >= 0.55 {
            ContextPressureLevel::Yellow
        } else {
            ContextPressureLevel::Green
        };
        Self {
            model_limit_tokens,
            reserved_output_tokens,
            system_tokens,
            tool_schema_tokens,
            transcript_tokens,
            context_tokens,
            state_packet_tokens,
            pressure,
        }
    }

    pub fn pressure_ratio(&self) -> f32 {
        let used_tokens = self
            .system_tokens
            .saturating_add(self.tool_schema_tokens)
            .saturating_add(self.transcript_tokens)
            .saturating_add(self.context_tokens)
            .saturating_add(self.state_packet_tokens);
        let usable_tokens = self
            .model_limit_tokens
            .saturating_sub(self.reserved_output_tokens);
        if usable_tokens == 0 {
            1.0
        } else {
            used_tokens as f32 / usable_tokens as f32
        }
    }
}

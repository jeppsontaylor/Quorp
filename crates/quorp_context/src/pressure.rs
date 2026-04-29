use quorp_context_model::{ContextBudgetTelemetry, ContextPressureLevel};

use crate::estimate_tokens;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ContextPressureReport {
    pub telemetry: ContextBudgetTelemetry,
    pub should_compact: bool,
}

pub fn measure_context_pressure(
    model_limit_tokens: u32,
    reserved_output_tokens: u32,
    system_prompt: &str,
    tool_schema: &str,
    transcript: &[String],
    context_items: &[String],
    state_packet: Option<&str>,
) -> ContextPressureReport {
    let system_tokens = estimate_tokens(system_prompt);
    let tool_schema_tokens = estimate_tokens(tool_schema);
    let transcript_tokens = estimate_tokens(&transcript.join("\n"));
    let context_tokens = estimate_tokens(&context_items.join("\n"));
    let state_packet_tokens = state_packet.map(estimate_tokens).unwrap_or(0);
    let telemetry = ContextBudgetTelemetry::new(
        model_limit_tokens,
        reserved_output_tokens,
        system_tokens,
        tool_schema_tokens,
        transcript_tokens,
        context_tokens,
        state_packet_tokens,
    );
    ContextPressureReport {
        should_compact: !matches!(telemetry.pressure, ContextPressureLevel::Green),
        telemetry,
    }
}

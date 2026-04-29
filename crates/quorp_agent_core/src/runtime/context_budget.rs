use crate::{CompletionRequest, TranscriptMessage};
use quorp_context::estimate_tokens;
use quorp_context_model::ContextBudgetTelemetry;

pub(crate) fn telemetry_for_request(
    request: &CompletionRequest,
    messages: &[TranscriptMessage],
    system_prompt: &str,
    tool_schema: &str,
    context_tokens: u32,
    state_packet_tokens: u32,
) -> ContextBudgetTelemetry {
    let transcript_tokens = messages
        .iter()
        .map(|message| estimate_tokens(&message.content))
        .sum::<u32>();
    ContextBudgetTelemetry::new(
        request.max_completion_tokens.unwrap_or(128_000),
        request
            .max_completion_tokens
            .map(|value| value / 8)
            .unwrap_or(8_000),
        estimate_tokens(system_prompt),
        estimate_tokens(tool_schema),
        transcript_tokens,
        context_tokens,
        state_packet_tokens,
    )
}

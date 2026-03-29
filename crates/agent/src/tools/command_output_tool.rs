use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::rc::Rc;
use std::sync::Arc;

use crate::{AgentTool, ThreadEnvironment, ToolCallEventStream, ToolInput};

/// Returns current output from a terminal started by the `terminal` tool (including background runs).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CommandOutputToolInput {
    /// Terminal id from the `terminal` tool response (same string as in the UI / tool result).
    pub terminal_id: String,
}

pub struct CommandOutputTool {
    environment: Rc<dyn ThreadEnvironment>,
}

impl CommandOutputTool {
    pub fn new(environment: Rc<dyn ThreadEnvironment>) -> Self {
        Self { environment }
    }
}

impl AgentTool for CommandOutputTool {
    type Input = CommandOutputToolInput;
    type Output = String;

    const NAME: &'static str = "command_output";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        match input {
            Ok(input) => format!("Terminal {}", input.terminal_id).into(),
            Err(_) => "Terminal output".into(),
        }
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        _event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<Self::Output, Self::Output>> {
        cx.spawn(async move |cx| {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("Failed to receive tool input: {e}"))?;

            let terminal_id = acp::TerminalId::new(input.terminal_id);
            let output_task = self
                .environment
                .terminal_current_output(terminal_id, cx);
            let response = output_task.await.map_err(|e| e.to_string())?;

            let text = response.output;
            let truncated_note = if response.truncated {
                "\n\n(Output was truncated.)"
            } else {
                ""
            };
            let exit = response
                .exit_status
                .as_ref()
                .and_then(|s| s.exit_code)
                .map(|c| format!("\n(exit code: {c})"))
                .unwrap_or_default();
            Ok(format!("```\n{text}\n```{truncated_note}{exit}"))
        })
    }
}

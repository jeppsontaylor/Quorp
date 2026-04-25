use crate::quorp::tui::agent_protocol::{ActionOutcome, AgentAction, ValidationPlan};
use crate::quorp::tui::app::Pane;
use crate::quorp::tui::chat::{ChatMessage, ChatUiEvent};

use super::harness::TuiTestHarness;

/// Verifies that when a generic task fails (e.g. RunValidation returns an error because of a bad compile),
/// the transcript shows the rollback message correctly.
#[test]
fn rollback_event_surfaces_in_transcript() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Chat;
    h.app.complete_bootstrap_for_test();

    // Seed chat
    h.app.chat.seed_messages_for_test(vec![
        ChatMessage::User("fix my build".into()),
        ChatMessage::Assistant(String::new()),
    ]);

    let plan = ValidationPlan {
        fmt: false,
        clippy: false,
        workspace_tests: false,
        tests: vec![],
        custom_commands: vec!["cargo check".to_string()],
    };

    let session_id = 0;
    let fallback_msg = "error[E0308]: mismatched types\n\n[System] Changes were safely rolled back. Please analyze the error and try applying a corrected fix.";

    let outcome = ActionOutcome::Failure {
        action: AgentAction::RunValidation { plan },
        error: fallback_msg.to_string(),
    };

    // Fire the command output and failure events
    h.apply_chat_event(ChatUiEvent::CommandOutput(
        session_id,
        fallback_msg.to_string(),
    ));
    h.apply_chat_event(ChatUiEvent::CommandFinished(session_id, outcome));

    h.draw();

    // The transcript should contain the formatted system rollback text
    h.assert_buffer_contains("Changes were safely rolled back");
    h.assert_buffer_contains("mismatched types");
}

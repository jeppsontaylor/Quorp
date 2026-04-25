use std::path::PathBuf;

use futures::channel::oneshot;

use crate::quorp::tui::agent_protocol::{ActionOutcome, AgentAction};

#[cfg_attr(not(test), allow(dead_code))]
pub enum CommandBridgeRequest {
    ExecuteAction {
        session_id: usize,
        action: AgentAction,
        project_root: PathBuf,
        cwd: PathBuf,
        responder: Option<oneshot::Sender<ActionOutcome>>,
        enable_rollback_on_validation_failure: bool,
    },
}

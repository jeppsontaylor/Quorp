//! Shared agent services for the inline QUORP CLI.
//!
//! The old alternate-screen dashboard has been removed from the default
//! product. This module now exposes only the services still used by the
//! terminal-first agent, benchmark runner, and typed tool executor.

#[path = "tui/agent_context.rs"]
pub mod agent_context;
#[path = "tui/agent_protocol.rs"]
pub mod agent_protocol;
#[path = "tui/agent_turn.rs"]
pub mod agent_turn;
#[path = "tui/chat_service.rs"]
pub mod chat_service;
#[path = "tui/command_bridge.rs"]
pub mod command_bridge;
#[path = "tui/diagnostics.rs"]
pub mod diagnostics;
#[path = "tui/mcp_client.rs"]
pub mod mcp_client;
#[path = "tui/model_registry.rs"]
pub mod model_registry;
#[path = "tui/native_backend.rs"]
pub mod native_backend;
#[path = "tui/path_guard.rs"]
pub mod path_guard;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ChatUiEvent {
    AssistantDelta(usize, String),
    StreamFinished(usize),
    Error(usize, String),
    CommandOutput(usize, String),
    CommandFinished(usize, agent_protocol::ActionOutcome),
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum TuiEvent {
    Chat(ChatUiEvent),
}

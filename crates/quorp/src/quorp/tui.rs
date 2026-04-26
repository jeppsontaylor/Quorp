//! Shared agent services for the inline QUORP CLI.
//!
//! The old alternate-screen dashboard has been removed from the default
//! product. This module now exposes only the services still used by the
//! terminal-first agent, benchmark runner, and typed tool executor.

pub mod agent_context;
pub mod agent_protocol;
pub mod agent_turn;
pub mod chat_service;
pub mod command_bridge;
pub mod diagnostics;
pub mod mcp_client;
pub mod model_registry;
pub mod native_backend;
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

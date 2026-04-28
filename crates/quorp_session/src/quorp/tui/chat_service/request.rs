use std::path::PathBuf;

use quorp_agent_core::AgentTurnResponse;

use crate::quorp::agent_runner::RoutingDecision;
use crate::quorp::tui::agent_protocol::AgentMode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatServiceRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatServiceMessage {
    pub role: ChatServiceRole,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct StreamRequest {
    pub request_id: u64,
    pub session_id: usize,
    pub model_id: String,
    pub agent_mode: AgentMode,
    pub latest_input: String,
    pub messages: Vec<ChatServiceMessage>,
    pub project_root: PathBuf,
    pub base_url_override: Option<String>,
    pub max_completion_tokens: Option<u32>,
    pub include_repo_capsule: bool,
    pub disable_reasoning: bool,
    pub native_tool_calls: bool,
    pub watchdog: Option<quorp_agent_core::CompletionWatchdogConfig>,
    pub safety_mode_label: Option<String>,
    pub prompt_compaction_policy: Option<quorp_agent_core::PromptCompactionPolicy>,
    pub capture_scope: Option<String>,
    pub capture_call_class: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SingleCompletionResult {
    pub content: String,
    pub reasoning_content: String,
    pub native_turn: Option<AgentTurnResponse>,
    pub native_turn_error: Option<String>,
    pub usage: Option<quorp_agent_core::TokenUsage>,
    pub raw_response: serde_json::Value,
    pub watchdog: Option<quorp_agent_core::ModelRequestWatchdogReport>,
    #[allow(dead_code)]
    pub routing: RoutingDecision,
}

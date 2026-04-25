use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt as _;
use quorp_agent_core::{
    AgentAction, AgentTurnResponse, PreviewEditPayload, ReadFileRange, ValidationPlan,
};
use reqwest::header::{ACCEPT, ACCEPT_ENCODING};
use serde_json::json;

use crate::quorp::agent_local::RoutingDecision;
use crate::quorp::codex_executor::{
    CodexCompletionOptions, default_interactive_artifact_dir, request_codex_completion,
};
use crate::quorp::executor::{
    CodexSessionMode, InteractiveProviderKind, codex_session_strategy_from_env,
};
use crate::quorp::prompt_compaction::{PromptMessage, PromptMessageRole, apply_prompt_compaction};
use crate::quorp::provider_config;
use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::agent_context::{
    load_instruction_context, render_instruction_context_for_prompt,
};
use crate::quorp::tui::agent_protocol::AgentMode;
use crate::quorp::tui::chat::ChatUiEvent;
use crate::quorp::tui::model_registry;
use crate::quorp::tui::openai_compatible_client::{
    OpenAiCompatibleChatMessage as SsdMoeChatMessage,
    OpenAiCompatibleChatRequest as SsdMoeChatRequest,
    OpenAiCompatibleClientConfig as SsdMoeClientConfig,
    OpenAiCompatibleStreamEvent as SsdMoeStreamEvent, build_request_body, chat_completions_url,
    parse_sse_data_line, parse_sse_payload,
};
use crate::quorp::tui::ssd_moe_client::local_bearer_token;
use crate::quorp::tui::ssd_moe_tui::SsdMoeRuntimeHandle;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const READ_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_SERVER_READY_TIMEOUT: Duration = Duration::from_secs(10);
const TURBOHERO_SERVER_READY_TIMEOUT: Duration = Duration::from_secs(180);
const TOKEN_COALESCE_WINDOW: Duration = Duration::from_millis(35);
const SYSTEM_PROMPT_PREFIX: &str = r#"You are Quorp, a rich terminal coding assistant.

You are working inside a real project checkout. Be concrete and concise.
When the transcript includes tool or command output, use it directly instead of inventing results.
Return a strict JSON object only. Do not wrap it in markdown or prose.
Never emit fake `[Tool Output]`, `[read_file]`, `[run_command]`, or simulated command/test output.
Never claim an action already ran unless that result appears in a user tool-output message.
You may batch multiple read and edit actions in a single turn. They will execute sequentially. If a mutating action fails, the remainder of the batch will be aborted. Pure inspection actions may continue after one failure.
If an optional field is hard to fill in correctly, omit it instead of inventing placeholder content.
Keep `assistant_message` short. Prefer compact actions over long prose. When editing an existing file, prefer `ReplaceBlock` unless you truly need a multi-file unified diff.
Emit at most 4 actions per turn and never repeat the same action/path in one turn.
Once you have identified the likely fix, prefer one edit action and at most one validation action instead of more exploratory reads.

JSON schema:
{
  "assistant_message": "user-visible explanation",
  "actions": [],
  "task_updates": [],
  "memory_updates": [],
  "requested_mode_change": null,
  "verifier_plan": null
}

Action variants:
- {"RunCommand":{"command":"cargo test -p quorp","timeout_ms":30000}}
- {"ReadFile":{"path":"src/main.rs"}}
- {"ReadFile":{"path":"src/main.rs","range":[390,450]}}
- {"ListDirectory":{"path":"."}}
- {"SearchText":{"query":"AgentTurnResponse","limit":6}}
- {"SearchSymbols":{"query":"render_agent_turn_text","limit":6}}
- {"FindFiles":{"query":"benchmark","limit":10}}
- {"StructuralSearch":{"pattern":"impl $T { $$$ }","language":"rust","path":"crates/quorp","limit":8}}
- {"CargoDiagnostics":{"include_clippy":false}}
- {"GetRepoCapsule":{"query":"agent runtime","limit":8}}
- {"WriteFile":{"path":"src/main.rs","content":"full file contents"}}
- {"ApplyPatch":{"path":"src/main.rs","patch":"--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new"}}
- {"ReplaceBlock":{"path":"src/main.rs","search_block":"exact old lines","replace_block":"new lines"}}
- {"ReplaceBlock":{"path":"src/main.rs","search_block":"exact old lines","replace_block":"new lines","range":{"start_line":390,"end_line":450}}}
- {"SetExecutable":{"path":"scripts/run.sh"}}
- {"McpCallTool":{"server_name":"docs","tool_name":"search","arguments":{"query":"validation plan"}}}
- {"RunValidation":{"plan":{"fmt":true,"clippy":true,"workspace_tests":false,"tests":["chat_flows"],"custom_commands":[]}}}

All file paths are relative to the workspace root and will be rejected if they escape the project. Do not use absolute paths in tool calls. If you need orientation, start with `{"ListDirectory":{"path":"."}}`.
Use `SearchText`, `SearchSymbols`, and `GetRepoCapsule` before broad file reads when you need repo-wide context.
`ApplyPatch` applies unified diff patches and can update, add, delete, or rename files in one action.
`ReplaceBlock` finds a match of `search_block` in the file (ignoring indentation) and replaces it with `replace_block`. Prefer it for small exact edits to existing files, and output ONLY the lines that need changing. If a snippet is repeated, include `range` from the latest read slice or use `ApplyPatch`; never guess which duplicate to edit.
Use `SetExecutable` when a script already exists but needs its executable bit enabled.
Use `McpCallTool` only when configured MCP servers are listed below. Always choose a listed `server_name` and pass JSON arguments.
Use `RunValidation` instead of raw shell commands for fmt, clippy, or test verification where possible.
For `RunValidation.tests`, pass selectors or crate arguments, not a full `cargo test ...` command. If you already have the exact shell command, place it in `custom_commands` instead.
For `task_updates`, `status` must be exactly one of `pending`, `in_progress`, `completed`, or `blocked`.
`memory_updates` is optional and should usually be `[]`.
Do not invent `progress`, `type`, or placeholder file contents unless you are intentionally recording a real note.
If a previous response was truncated or malformed, the next response must be shorter and simpler than the last one.

Minimal valid example:
{
  "assistant_message": "I will inspect the pricing logic first.",
  "actions": [
    {"ListDirectory":{"path":"."}},
    {"ReadFile":{"path":"crates/orders-core/src/lib.rs"}}
  ],
  "task_updates": [],
  "memory_updates": [],
  "requested_mode_change": null,
  "verifier_plan": null
}"#;

const NATIVE_TOOL_SYSTEM_PROMPT_PREFIX: &str = r#"You are Quorp, a rich terminal coding assistant.

Use native tool calls for every inspection, edit, and validation action.
When native tool definitions are present, the runtime requires a native tool call response before the task is complete. Prose-only turns will be rejected and ignored.
Keep assistant text brief. Prefer a short plan or status note followed by tool calls.
On the first turn, state a concise plan and orient with search/list/repo-capsule calls before editing unless the exact fix is already obvious.
Before the task is complete, every turn must include at least one concrete tool call. Planning-only responses are invalid.
Emit at most 4 tool calls per turn and do not repeat the same path/action in one turn.
All tool paths must be relative to the workspace root. Never use absolute paths.
When the brief names fields, symbols, tests, or endpoints, search before guessing filenames.
Prefer `ApplyPatch` for larger or multi-hunk edits. Use `ReplaceBlock` only when you know the exact current block.
Never print simulated directory listings, file contents, shell output, or tool results in assistant text.
If native tool calls are unavailable in this runtime, fall back to one raw JSON object only:
{
  "assistant_message": "brief status",
  "tool_calls": [
    {
      "tool_name": "search_text",
      "tool_args": { "query": "grace_period_ends_at", "limit": 4 }
    }
  ],
  "task_updates": [],
  "verifier_plan": null
}
Accepted fallback tool names are the same as the native tool definitions: `run_command`, `read_file`, `list_directory`, `search_text`, `search_symbols`, `find_files`, `structural_search`, `structural_edit_preview`, `cargo_diagnostics`, `get_repo_capsule`, `explain_validation_failure`, `suggest_implementation_targets`, `suggest_edit_anchors`, `preview_edit`, `replace_range`, `modify_toml`, `apply_preview`, `write_file`, `apply_patch`, `replace_block`, `set_executable`, and `run_validation`.
If you include `task_updates`, it must be an array of objects. If you include `verifier_plan`, it must be an object, not a free-form sentence.
If the task is complete and validation is green, return a short assistant message with no tool calls.
Do not switch to the raw JSON fallback unless the runtime explicitly says native tool calls are unavailable."#;

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
pub enum ChatServiceRequest {
    SubmitPrompt {
        session_id: usize,
        model_id: String,
        agent_mode: AgentMode,
        latest_input: String,
        messages: Vec<ChatServiceMessage>,
        project_root: PathBuf,
        base_url_override: Option<String>,
        prompt_compaction_policy: Option<quorp_agent_core::PromptCompactionPolicy>,
    },
    Cancel {
        session_id: usize,
    },
    SummarizeCommandOutput {
        session_id: usize,
        model_id: String,
        agent_mode: AgentMode,
        command: String,
        command_output: String,
        messages: Vec<ChatServiceMessage>,
        project_root: PathBuf,
        base_url_override: Option<String>,
        prompt_compaction_policy: Option<quorp_agent_core::PromptCompactionPolicy>,
    },
}

pub fn spawn_chat_service_loop(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
) -> futures::channel::mpsc::UnboundedSender<ChatServiceRequest> {
    let (request_tx, mut request_rx) = futures::channel::mpsc::unbounded();
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::Error(
                    0,
                    format!("Failed to start native chat runtime: {error}"),
                )));
                return;
            }
        };
        let ssd_moe_runtime = SsdMoeRuntimeHandle::shared_handle();
        let active_streams: Arc<Mutex<HashMap<usize, tokio::task::AbortHandle>>> =
            Arc::new(Mutex::new(HashMap::new()));

        runtime.block_on(async move {
            while let Some(request) = request_rx.next().await {
                match request {
                    ChatServiceRequest::Cancel { session_id } => {
                        crate::quorp::tui::diagnostics::log_event(
                            "chat.request_cancel",
                            json!({
                                "session_id": session_id,
                            }),
                        );
                        cancel_stream(&active_streams, session_id);
                    }
                    ChatServiceRequest::SubmitPrompt {
                        session_id,
                        model_id,
                        agent_mode,
                        latest_input,
                        messages,
                        project_root,
                        base_url_override,
                        prompt_compaction_policy,
                    } => {
                        let trimmed = latest_input.trim();
                        if let Some(command) = parse_inline_command(trimmed) {
                            let response = format!(
                                "Command request queued for confirmation.\n<run_command timeout_ms=\"30000\">{command}</run_command>"
                            );
                            let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::AssistantDelta(
                                session_id,
                                response,
                            )));
                            let _ = event_tx
                                .send(TuiEvent::Chat(ChatUiEvent::StreamFinished(session_id)));
                            continue;
                        }
                        if trimmed.is_empty() {
                            let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::Error(
                                session_id,
                                "Chat input was empty.".to_string(),
                            )));
                            continue;
                        }
                        if trimmed.contains("<run_command") {
                            let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::Error(
                                session_id,
                                "Direct `<run_command>` blocks are no longer accepted in chat input. Use `/run <command>` instead.".to_string(),
                            )));
                            continue;
                        }

                        cancel_stream(&active_streams, session_id);
                        let request_id = crate::quorp::tui::diagnostics::next_request_id();
                        crate::quorp::tui::diagnostics::log_event(
                            "chat.request_accepted",
                            json!({
                                "request_id": request_id,
                                "session_id": session_id,
                                "model_id": model_id,
                                "message_count": messages.len(),
                            }),
                        );
                        spawn_stream_task(
                            event_tx.clone(),
                            active_streams.clone(),
                            ssd_moe_runtime.clone(),
                            StreamRequest {
                                request_id,
                                session_id,
                                model_id,
                                agent_mode,
                                latest_input,
                                messages,
                                project_root,
                                base_url_override,
                                max_completion_tokens: Some(4096),
                                include_repo_capsule: true,
                                disable_reasoning: false,
                                native_tool_calls: false,
                                watchdog: None,
                                safety_mode_label: None,
                                prompt_compaction_policy,
                                capture_scope: None,
                                capture_call_class: None,
                            },
                        );
                    }
                    ChatServiceRequest::SummarizeCommandOutput {
                        session_id,
                        model_id,
                        agent_mode,
                        command,
                        command_output,
                        messages,
                        project_root,
                        base_url_override,
                        prompt_compaction_policy,
                    } => {
                        let _ = command_output;
                        let _ = command;
                        cancel_stream(&active_streams, session_id);
                        let request_id = crate::quorp::tui::diagnostics::next_request_id();
                        crate::quorp::tui::diagnostics::log_event(
                            "chat.request_accepted",
                            json!({
                                "request_id": request_id,
                                "session_id": session_id,
                                "model_id": model_id,
                                "message_count": messages.len(),
                                "source": "command_summary",
                            }),
                        );
                        spawn_stream_task(
                            event_tx.clone(),
                            active_streams.clone(),
                            ssd_moe_runtime.clone(),
                            StreamRequest {
                                request_id,
                                session_id,
                                model_id,
                                agent_mode,
                                latest_input: format!(
                                    "Command output for {command}:\n{command_output}"
                                ),
                                messages,
                                project_root,
                                base_url_override,
                                max_completion_tokens: Some(4096),
                                include_repo_capsule: true,
                                disable_reasoning: false,
                                native_tool_calls: false,
                                watchdog: None,
                                safety_mode_label: None,
                                prompt_compaction_policy,
                                capture_scope: None,
                                capture_call_class: None,
                            },
                        );
                    }
                }
            }
        });
    });
    request_tx
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
    pub routing: RoutingDecision,
}

#[derive(Debug, Clone)]
struct ResolvedModelTarget {
    provider: InteractiveProviderKind,
    provider_model_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedClientConfig {
    provider: InteractiveProviderKind,
    client: SsdMoeClientConfig,
    bearer_token: Option<String>,
    routing: RoutingDecision,
}

fn resolved_provider(model_id: &str) -> InteractiveProviderKind {
    model_registry::chat_model_provider(
        model_id,
        crate::quorp::executor::interactive_provider_from_env(),
    )
}

fn resolve_model_target(model_id: &str) -> ResolvedModelTarget {
    ResolvedModelTarget {
        provider: resolved_provider(model_id),
        provider_model_id: model_registry::chat_model_raw_id(model_id).to_string(),
    }
}

pub(crate) fn normalize_remote_base_url(base_url: &str, append_v1: bool) -> anyhow::Result<String> {
    provider_config::normalize_remote_base_url(base_url, append_v1)
}

fn empty_request_overrides() -> (
    BTreeMap<String, String>,
    serde_json::Map<String, serde_json::Value>,
) {
    (BTreeMap::new(), serde_json::Map::new())
}

fn env_flag_disabled(name: &str) -> bool {
    provider_config::env_value(name).is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

fn env_u64(name: &str) -> Option<u64> {
    provider_config::env_value(name)?.trim().parse::<u64>().ok()
}

fn nvidia_rate_limit_retries() -> u64 {
    env_u64("QUORP_NVIDIA_RATE_LIMIT_RETRIES").unwrap_or(2)
}

fn parse_retry_after_seconds(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn nvidia_rate_limit_backoff_seconds(
    headers: &reqwest::header::HeaderMap,
    attempt_index: u64,
) -> u64 {
    parse_retry_after_seconds(headers)
        .unwrap_or_else(|| 30_u64.saturating_mul(attempt_index.saturating_add(1)))
        .clamp(1, 120)
}

fn is_nvidia_kimi_model(model_id: &str) -> bool {
    model_id
        .to_ascii_lowercase()
        .starts_with("moonshotai/kimi-k2")
}

fn request_uses_nvidia_kimi(request: &StreamRequest) -> bool {
    let model_target = resolve_model_target(&request.model_id);
    model_target.provider == InteractiveProviderKind::Nvidia
        && is_nvidia_kimi_model(&model_target.provider_model_id)
}

fn is_nvidia_qwen_coder_model(model_id: &str) -> bool {
    model_id
        .to_ascii_lowercase()
        .starts_with("qwen/qwen3-coder-480b-a35b-instruct")
}

fn request_uses_nvidia_qwen_coder(request: &StreamRequest) -> bool {
    let model_target = resolve_model_target(&request.model_id);
    model_target.provider == InteractiveProviderKind::Nvidia
        && is_nvidia_qwen_coder_model(&model_target.provider_model_id)
}

fn nvidia_kimi_benchmark_profile(request: &StreamRequest) -> bool {
    request_uses_nvidia_kimi(request)
        && request
            .safety_mode_label
            .as_deref()
            .is_some_and(|label| label == "nvidia_kimi_benchmark")
}

fn nvidia_qwen_benchmark_profile(request: &StreamRequest) -> bool {
    request_uses_nvidia_qwen_coder(request)
        && request
            .safety_mode_label
            .as_deref()
            .is_some_and(|label| label == "nvidia_qwen_benchmark")
}

fn nvidia_controller_benchmark_profile(request: &StreamRequest) -> bool {
    nvidia_kimi_benchmark_profile(request) || nvidia_qwen_benchmark_profile(request)
}

fn nvidia_request_body_overrides(
    provider_model_id: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let mut body = serde_json::Map::new();
    if is_nvidia_kimi_model(provider_model_id) {
        body.insert("temperature".to_string(), serde_json::json!(1.0));
        body.insert("top_p".to_string(), serde_json::json!(0.95));
        if env_flag_disabled("QUORP_NVIDIA_KIMI_THINKING") {
            body.insert(
                "chat_template_kwargs".to_string(),
                serde_json::json!({ "thinking": false }),
            );
        }
    }
    body
}

fn remote_request_headers(
    request: &StreamRequest,
    _provider: InteractiveProviderKind,
    routing_mode: &str,
) -> BTreeMap<String, String> {
    let action_contract_mode = if request.native_tool_calls {
        "native_tool_calls_v1"
    } else {
        "json_action_contract_v1"
    };
    let mut headers = BTreeMap::from([
        (
            "User-Agent".to_string(),
            format!("quorp/{}", env!("CARGO_PKG_VERSION")),
        ),
        (
            "X-Quorp-Run-Id".to_string(),
            correlation_run_id(&request.project_root),
        ),
        (
            "X-Quorp-Session-Id".to_string(),
            request.session_id.to_string(),
        ),
        (
            "X-Quorp-Request-Id".to_string(),
            request.request_id.to_string(),
        ),
        ("X-Quorp-Routing-Mode".to_string(), routing_mode.to_string()),
        (
            "X-Quorp-Action-Contract-Mode".to_string(),
            action_contract_mode.to_string(),
        ),
        (
            "X-Quorp-Repo-Capsule-Injected".to_string(),
            request.include_repo_capsule.to_string(),
        ),
        (
            "X-Quorp-Reasoning-Enabled".to_string(),
            (!request.disable_reasoning).to_string(),
        ),
        (
            "X-Quorp-Executor-Model".to_string(),
            request.model_id.clone(),
        ),
        ("X-WarpOS-Agent".to_string(), "quorp".to_string()),
    ]);
    if let Some(scope) = request.capture_scope.as_deref() {
        headers.insert("X-WarpOS-Scope".to_string(), scope.to_string());
    }
    if let Some(call_class) = request.capture_call_class.as_deref() {
        headers.insert("X-WarpOS-Call-Class".to_string(), call_class.to_string());
    }
    headers
}

fn correlation_run_id(project_root: &std::path::Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    project_root.to_string_lossy().hash(&mut hasher);
    format!("quorp-run-{:016x}", hasher.finish())
}

fn default_routing_decision(
    provider: InteractiveProviderKind,
    requested_model: String,
    effective_model: String,
    provider_base_url: Option<String>,
    auth_mode: Option<String>,
    comparable: bool,
    proxy_visible_remote_egress_expected: bool,
) -> RoutingDecision {
    RoutingDecision {
        routing_mode: provider_config::resolved_routing_mode().label().to_string(),
        requested_provider: provider.label().to_string(),
        requested_model,
        candidate_models: vec![effective_model.clone()],
        effective_provider: provider.label().to_string(),
        effective_model,
        used_local_fallback: false,
        fallback_reason: None,
        comparable,
        provider_base_url,
        auth_mode,
        proxy_visible_remote_egress_expected,
    }
}

fn wrap_raw_provider_response(
    provider_response: serde_json::Value,
    routing: &RoutingDecision,
) -> serde_json::Value {
    serde_json::json!({
        "provider_response": provider_response,
        "routing": routing,
    })
}

pub(crate) fn resolve_ollama_base_url(base_url_override: Option<&str>) -> anyhow::Result<String> {
    if let Some(base_url_override) = base_url_override
        && !base_url_override.trim().is_empty()
    {
        return normalize_remote_base_url(base_url_override, true);
    }
    if let Some(host) = provider_config::env_value("QUORP_OLLAMA_HOST")
        && !host.trim().is_empty()
    {
        return normalize_remote_base_url(host.trim(), true);
    }
    normalize_remote_base_url("http://127.0.0.1:11434", true)
}

fn chat_completions_url_for_provider(
    provider: InteractiveProviderKind,
    base_url: &str,
) -> anyhow::Result<String> {
    match provider {
        InteractiveProviderKind::Local => chat_completions_url(base_url),
        InteractiveProviderKind::Ollama
        | InteractiveProviderKind::OpenAiCompatible
        | InteractiveProviderKind::Nvidia => Ok(format!(
            "{}/chat/completions",
            base_url.trim_end_matches('/')
        )),
        InteractiveProviderKind::Codex => {
            anyhow::bail!("codex chat requests do not use the OpenAI-compatible stream client")
        }
    }
}

fn provider_connection_name(provider: InteractiveProviderKind) -> &'static str {
    match provider {
        InteractiveProviderKind::Local => "SSD-MOE",
        InteractiveProviderKind::Ollama => "Ollama",
        InteractiveProviderKind::OpenAiCompatible => "OpenAI-compatible",
        InteractiveProviderKind::Nvidia => "NVIDIA NIM",
        InteractiveProviderKind::Codex => "Codex",
    }
}

fn cancel_stream(
    active_streams: &Arc<Mutex<HashMap<usize, tokio::task::AbortHandle>>>,
    session_id: usize,
) {
    if let Some(handle) = active_streams
        .lock()
        .expect("chat stream map lock")
        .remove(&session_id)
    {
        handle.abort();
    }
}

fn spawn_stream_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    active_streams: Arc<Mutex<HashMap<usize, tokio::task::AbortHandle>>>,
    ssd_moe_runtime: crate::quorp::tui::ssd_moe_tui::SsdMoeRuntimeHandle,
    request: StreamRequest,
) {
    let session_id = request.session_id;
    let request_id = request.request_id;
    let provider = resolved_provider(&request.model_id);
    let task = tokio::spawn(async move {
        let result = match provider {
            InteractiveProviderKind::Codex => {
                run_codex_stream_request(event_tx.clone(), request).await
            }
            InteractiveProviderKind::Local
            | InteractiveProviderKind::Ollama
            | InteractiveProviderKind::OpenAiCompatible
            | InteractiveProviderKind::Nvidia => {
                run_stream_request(event_tx.clone(), ssd_moe_runtime, request).await
            }
        };
        if let Err(error) = result {
            crate::quorp::tui::diagnostics::log_event(
                "chat.request_error",
                json!({
                    "request_id": request_id,
                    "session_id": session_id,
                    "error": error,
                }),
            );
            let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::Error(session_id, error)));
            let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::StreamFinished(session_id)));
        }
    });
    active_streams
        .lock()
        .expect("chat stream map lock")
        .insert(session_id, task.abort_handle());
}

async fn run_codex_stream_request(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    request: StreamRequest,
) -> Result<(), String> {
    let prompt = build_codex_prompt(&request);
    let artifact_dir = default_interactive_artifact_dir(&request.project_root, "chat");
    let session_strategy = codex_session_strategy_from_env(CodexSessionMode::ResumeLastForCwd);
    let session_id = request.session_id;
    let model_target = resolve_model_target(&request.model_id);
    let completion = tokio::task::spawn_blocking(move || {
        request_codex_completion(CodexCompletionOptions {
            workspace: request.project_root,
            prompt,
            model_id: model_target.provider_model_id,
            max_seconds: Some(3600),
            artifact_dir,
            session_strategy,
        })
    })
    .await
    .map_err(|error| format!("Codex request task join error: {error}"))?
    .map_err(|error| error.to_string())?;

    let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::AssistantDelta(
        session_id,
        completion.content,
    )));
    let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::StreamFinished(session_id)));
    Ok(())
}

async fn run_stream_request(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    ssd_moe_runtime: crate::quorp::tui::ssd_moe_tui::SsdMoeRuntimeHandle,
    request: StreamRequest,
) -> Result<(), String> {
    let resolve_started_at = std::time::Instant::now();
    let client_config = finalize_client_config_for_request(
        &ssd_moe_runtime,
        &request,
        resolve_client_config(&ssd_moe_runtime, &request).map_err(|error| error.to_string())?,
    )
    .await
    .map_err(|error| error.to_string())?;
    crate::quorp::tui::diagnostics::log_event(
        "chat.client_config_resolved",
        json!({
            "request_id": request.request_id,
            "session_id": request.session_id,
            "provider": client_config.provider.label(),
            "base_url": client_config.client.base_url,
            "routing": client_config.routing,
            "resolve_ms": resolve_started_at.elapsed().as_millis(),
            "runtime_status": ssd_moe_runtime.status().label(),
        }),
    );
    let request_body = build_completion_request_body(&request, &client_config.client, true);
    let url =
        chat_completions_url_for_provider(client_config.provider, &client_config.client.base_url)
            .map_err(|error| error.to_string())?;

    let http_client = reqwest::Client::builder()
        .connect_timeout(client_config.client.connect_timeout)
        .read_timeout(client_config.client.read_timeout)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .build()
        .map_err(|error| format!("Failed to build HTTP client: {error}"))?;

    let mut request_builder = http_client
        .post(url)
        .header(ACCEPT, "text/event-stream")
        .header(ACCEPT_ENCODING, "identity")
        .json(&request_body);
    if let Some(bearer_token) = client_config.bearer_token.as_deref() {
        request_builder = request_builder.bearer_auth(bearer_token);
    }
    request_builder = apply_extra_headers(request_builder, &client_config.client.extra_headers);
    let response = request_builder.send().await.map_err(|error| {
        format!(
            "Failed to connect to {}: {error}",
            provider_connection_name(client_config.provider)
        )
    })?;
    crate::quorp::tui::diagnostics::log_event(
        "chat.stream_connected",
        json!({
            "request_id": request.request_id,
            "session_id": request.session_id,
            "status": response.status().as_u16(),
            "provider": client_config.provider.label(),
            "base_url": client_config.client.base_url,
        }),
    );

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<body unavailable>".to_string());
        return Err(format!(
            "{} returned {} while starting chat stream: {}",
            provider_connection_name(client_config.provider),
            status,
            body.trim()
        ));
    }

    stream_response_to_ui(
        response,
        request.session_id,
        request.request_id,
        &ssd_moe_runtime,
        &event_tx,
    )
    .await
}

pub(crate) fn resolve_client_config(
    ssd_moe_runtime: &crate::quorp::tui::ssd_moe_tui::SsdMoeRuntimeHandle,
    request: &StreamRequest,
) -> anyhow::Result<ResolvedClientConfig> {
    let model_target = resolve_model_target(&request.model_id);
    match model_target.provider {
        InteractiveProviderKind::Local => resolve_local_client_config(
            ssd_moe_runtime,
            request,
            &model_target.provider_model_id,
            None,
        ),
        InteractiveProviderKind::Ollama => {
            resolve_ollama_client_config(request, &model_target.provider_model_id)
        }
        InteractiveProviderKind::OpenAiCompatible => {
            resolve_openai_compatible_client_config(request, &model_target.provider_model_id)
        }
        InteractiveProviderKind::Nvidia => {
            resolve_nvidia_client_config(request, &model_target.provider_model_id)
        }
        InteractiveProviderKind::Codex => {
            anyhow::bail!("codex requests do not use the OpenAI-compatible chat client")
        }
    }
}

fn resolve_local_client_config(
    ssd_moe_runtime: &crate::quorp::tui::ssd_moe_tui::SsdMoeRuntimeHandle,
    request: &StreamRequest,
    local_model_id: &str,
    routing_override: Option<RoutingDecision>,
) -> anyhow::Result<ResolvedClientConfig> {
    let local_base_url_override = resolved_local_base_url_override(request);
    let base_url = if let Some(base_url_override) = local_base_url_override.as_ref() {
        crate::quorp::tui::ssd_moe_client::validate_local_runtime_base_url(base_url_override)
            .map_err(anyhow::Error::msg)?;
        base_url_override.trim().trim_end_matches('/').to_string()
    } else {
        let model = model_registry::local_moe_spec_for_registry_id(local_model_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown local SSD-MOE model `{}`", local_model_id))?;
        crate::quorp::tui::diagnostics::log_event(
            "chat.runtime_prepare_begin",
            json!({
                "request_id": request.request_id,
                "session_id": request.session_id,
                "model_id": model.id,
            }),
        );
        ssd_moe_runtime.ensure_running(&request.project_root, &model);
        ssd_moe_runtime
            .wait_until_ready(server_ready_timeout(request))
            .map_err(anyhow::Error::msg)?;
        crate::quorp::tui::diagnostics::log_event(
            "chat.runtime_prepare_ready",
            json!({
                "request_id": request.request_id,
                "session_id": request.session_id,
                "runtime_status": ssd_moe_runtime.status().label(),
                "base_url": ssd_moe_runtime.base_url(),
                "runtime_metadata": ssd_moe_runtime.runtime_metadata().map(|runtime| {
                    json!({
                        "served_model_id": runtime.get("served_model_id").cloned(),
                        "kv_mode": runtime.get("kv_mode").cloned(),
                        "turboquant_enabled": runtime.get("turboquant_enabled").cloned(),
                        "max_ctx": runtime.get("max_ctx").cloned(),
                        "load_time_ms": runtime.get("load_time_ms").cloned(),
                        "pid": runtime.get("pid").cloned(),
                    })
                }),
            }),
        );
        ssd_moe_runtime.base_url()
    };
    let routing = routing_override.unwrap_or_else(|| {
        default_routing_decision(
            InteractiveProviderKind::Local,
            request.model_id.clone(),
            local_model_id.to_string(),
            Some(base_url.clone()),
            Some("local_bearer".to_string()),
            true,
            !provider_config::is_loopback_base_url(&base_url),
        )
    });
    Ok(ResolvedClientConfig {
        provider: InteractiveProviderKind::Local,
        client: SsdMoeClientConfig {
            base_url: base_url.clone(),
            model_id: local_model_id.to_string(),
            connect_timeout: CONNECT_TIMEOUT,
            read_timeout: READ_TIMEOUT,
            extra_headers: {
                let mut headers = remote_request_headers(
                    request,
                    InteractiveProviderKind::Local,
                    provider_config::resolved_routing_mode().label(),
                );
                headers.insert(
                    "X-Quorp-Local-Model".to_string(),
                    local_model_id.to_string(),
                );
                headers
            },
            extra_body: empty_request_overrides().1,
        },
        bearer_token: Some(local_bearer_token(&base_url).map_err(anyhow::Error::msg)?),
        routing,
    })
}

fn resolved_local_base_url_override(request: &StreamRequest) -> Option<String> {
    request.base_url_override.as_ref().cloned().or_else(|| {
        if request.safety_mode_label.as_deref() == Some("heavy_local") {
            None
        } else {
            provider_config::resolved_local_base_url_env()
        }
    })
}

fn resolve_ollama_client_config(
    request: &StreamRequest,
    ollama_model_id: &str,
) -> anyhow::Result<ResolvedClientConfig> {
    let base_url = resolve_ollama_base_url(request.base_url_override.as_deref())?;
    let routing = default_routing_decision(
        InteractiveProviderKind::Ollama,
        request.model_id.clone(),
        ollama_model_id.to_string(),
        Some(base_url.clone()),
        Some("none".to_string()),
        true,
        !provider_config::is_loopback_base_url(&base_url),
    );
    Ok(ResolvedClientConfig {
        provider: InteractiveProviderKind::Ollama,
        client: SsdMoeClientConfig {
            base_url: base_url.clone(),
            model_id: ollama_model_id.to_string(),
            connect_timeout: CONNECT_TIMEOUT,
            read_timeout: READ_TIMEOUT,
            extra_headers: remote_request_headers(
                request,
                InteractiveProviderKind::Ollama,
                provider_config::resolved_routing_mode().label(),
            ),
            extra_body: empty_request_overrides().1,
        },
        bearer_token: None,
        routing,
    })
}

fn resolve_openai_compatible_client_config(
    request: &StreamRequest,
    provider_model_id: &str,
) -> anyhow::Result<ResolvedClientConfig> {
    let runtime =
        provider_config::resolve_openai_compatible_runtime(request.base_url_override.as_deref())?;
    let routing = default_routing_decision(
        InteractiveProviderKind::OpenAiCompatible,
        request.model_id.clone(),
        provider_model_id.to_string(),
        Some(runtime.base_url.clone()),
        Some(runtime.auth_mode.clone()),
        true,
        runtime.proxy_visible_remote_egress_expected,
    );
    Ok(ResolvedClientConfig {
        provider: InteractiveProviderKind::OpenAiCompatible,
        client: SsdMoeClientConfig {
            base_url: runtime.base_url.clone(),
            model_id: provider_model_id.to_string(),
            connect_timeout: CONNECT_TIMEOUT,
            read_timeout: READ_TIMEOUT,
            extra_headers: remote_request_headers(
                request,
                InteractiveProviderKind::OpenAiCompatible,
                provider_config::resolved_routing_mode().label(),
            ),
            extra_body: empty_request_overrides().1,
        },
        bearer_token: Some(runtime.api_key),
        routing,
    })
}

fn resolve_nvidia_client_config(
    request: &StreamRequest,
    provider_model_id: &str,
) -> anyhow::Result<ResolvedClientConfig> {
    let runtime = provider_config::resolve_nvidia_runtime(request.base_url_override.as_deref())?;
    let routing = default_routing_decision(
        InteractiveProviderKind::Nvidia,
        request.model_id.clone(),
        provider_model_id.to_string(),
        Some(runtime.base_url.clone()),
        Some(runtime.auth_mode.clone()),
        true,
        runtime.proxy_visible_remote_egress_expected,
    );
    Ok(ResolvedClientConfig {
        provider: InteractiveProviderKind::Nvidia,
        client: SsdMoeClientConfig {
            base_url: runtime.base_url.clone(),
            model_id: provider_model_id.to_string(),
            connect_timeout: CONNECT_TIMEOUT,
            read_timeout: READ_TIMEOUT,
            extra_headers: remote_request_headers(
                request,
                InteractiveProviderKind::Nvidia,
                provider_config::resolved_routing_mode().label(),
            ),
            extra_body: nvidia_request_body_overrides(provider_model_id),
        },
        bearer_token: Some(runtime.api_key),
        routing,
    })
}

async fn finalize_client_config_for_request(
    _ssd_moe_runtime: &crate::quorp::tui::ssd_moe_tui::SsdMoeRuntimeHandle,
    _request: &StreamRequest,
    client_config: ResolvedClientConfig,
) -> anyhow::Result<ResolvedClientConfig> {
    Ok(client_config)
}

fn apply_extra_headers(
    mut request_builder: reqwest::RequestBuilder,
    extra_headers: &BTreeMap<String, String>,
) -> reqwest::RequestBuilder {
    for (name, value) in extra_headers {
        request_builder = request_builder.header(name, value);
    }
    request_builder
}

fn server_ready_timeout(request: &StreamRequest) -> Duration {
    if request.base_url_override.is_some()
        || matches!(
            resolved_provider(&request.model_id),
            InteractiveProviderKind::Ollama
                | InteractiveProviderKind::OpenAiCompatible
                | InteractiveProviderKind::Nvidia
        )
    {
        return DEFAULT_SERVER_READY_TIMEOUT;
    }
    if model_registry::local_moe_spec_for_registry_id(
        &resolve_model_target(&request.model_id).provider_model_id,
    )
    .is_some()
    {
        TURBOHERO_SERVER_READY_TIMEOUT
    } else {
        DEFAULT_SERVER_READY_TIMEOUT
    }
}

fn native_tool_definitions() -> Vec<serde_json::Value> {
    vec![
        function_tool(
            "run_command",
            "Run an allowlisted shell command inside the workspace.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "timeout_ms": { "type": "integer", "minimum": 1000 }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "read_file",
            "Read a workspace-relative file. Optionally request a focused inclusive line range [start_line, end_line].",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "range": {
                        "type": "array",
                        "items": { "type": "integer", "minimum": 1 },
                        "minItems": 2,
                        "maxItems": 2
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "list_directory",
            "List a workspace-relative directory.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "search_text",
            "Search repo text for a literal query.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 32 }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "search_symbols",
            "Search indexed symbols before guessing file paths.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 32 }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "find_files",
            "Find repo files by name or path using the configured fd integration when available.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 64 }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "structural_search",
            "Run syntax-aware structural search with ast-grep. Prefer this for Rust constructs when regex would be brittle.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "language": { "type": "string" },
                    "path": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 32 }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "structural_edit_preview",
            "Dry-run an ast-grep structural rewrite. This never mutates files; apply the returned preview_id with apply_preview if the preview is correct.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "rewrite": { "type": "string" },
                    "language": { "type": "string" },
                    "path": { "type": "string" }
                },
                "required": ["pattern", "rewrite"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "cargo_diagnostics",
            "Run configured Cargo JSON diagnostics and return compact compiler errors with file/line anchors.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "include_clippy": { "type": "boolean" }
                },
                "additionalProperties": false
            }),
        ),
        function_tool(
            "get_repo_capsule",
            "Fetch a compact repository capsule for orientation.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 32 }
                },
                "additionalProperties": false
            }),
        ),
        function_tool(
            "explain_validation_failure",
            "Summarize observed validation output into failing tests, excerpts, and file/line anchors. This is read-only and never proposes a gold patch.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "output": { "type": "string" }
                },
                "required": ["command", "output"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "suggest_implementation_targets",
            "Rank likely implementation repair targets from observed validation output and an optional failing location. This is read-only and never proposes code changes.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "output": { "type": "string" },
                    "failing_path": { "type": "string" },
                    "failing_line": { "type": "integer", "minimum": 1 }
                },
                "required": ["command", "output"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "suggest_edit_anchors",
            "Inspect a focused file region and return unique edit anchors plus safe ApplyPatch/ranged ReplaceBlock scaffolds. This is read-only.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "range": {
                        "type": "array",
                        "items": { "type": "integer", "minimum": 1 },
                        "minItems": 2,
                        "maxItems": 2
                    },
                    "search_hint": { "type": "string" }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "preview_edit",
            "Dry-run an edit intent against a workspace-relative file. This is read-only and never mutates files. Successful previews return a preview_id that can be applied with apply_preview.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "edit": {
                        "type": "object",
                        "properties": {
                            "apply_patch": {
                                "type": "object",
                                "properties": {
                                    "patch": { "type": "string" }
                                },
                                "required": ["patch"],
                                "additionalProperties": false
                            },
                            "replace_block": {
                                "type": "object",
                                "properties": {
                                    "search_block": { "type": "string" },
                                    "replace_block": { "type": "string" },
                                    "range": {
                                        "type": "array",
                                        "items": { "type": "integer", "minimum": 1 },
                                        "minItems": 2,
                                        "maxItems": 2
                                    }
                                },
                                "required": ["search_block", "replace_block"],
                                "additionalProperties": false
                            },
                            "replace_range": {
                                "type": "object",
                                "properties": {
                                    "range": {
                                        "type": "array",
                                        "items": { "type": "integer", "minimum": 1 },
                                        "minItems": 2,
                                        "maxItems": 2
                                    },
                                    "expected_hash": { "type": "string" },
                                    "replacement": { "type": "string" }
                                },
                                "required": ["range", "expected_hash", "replacement"],
                                "additionalProperties": false
                            },
                            "modify_toml": {
                                "type": "object",
                                "properties": {
                                    "expected_hash": { "type": "string" },
                                    "operations": toml_operations_schema()
                                },
                                "required": ["expected_hash", "operations"],
                                "additionalProperties": false
                            }
                        },
                        "additionalProperties": false
                    }
                },
                "required": ["path", "edit"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "replace_range",
            "Replace an already-read line range when the range content_hash still matches. Prefer this over unified diffs for small source edits.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "range": {
                        "type": "array",
                        "items": { "type": "integer", "minimum": 1 },
                        "minItems": 2,
                        "maxItems": 2
                    },
                    "expected_hash": { "type": "string" },
                    "replacement": { "type": "string" }
                },
                "required": ["path", "range", "expected_hash", "replacement"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "modify_toml",
            "Safely edit Cargo/TOML dependency tables using a full-file content_hash. Operations support set_dependency and remove_dependency.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "expected_hash": { "type": "string" },
                    "operations": toml_operations_schema()
                },
                "required": ["path", "expected_hash", "operations"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "apply_preview",
            "Apply a previously successful preview_id if the target file still has the same base hash.",
            json!({
                "type": "object",
                "properties": {
                    "preview_id": { "type": "string" }
                },
                "required": ["preview_id"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "write_file",
            "Write a full workspace-relative file.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "apply_patch",
            "Apply a unified diff patch to a workspace-relative file.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "patch": { "type": "string" }
                },
                "required": ["path", "patch"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "replace_block",
            "Replace an exact block inside a workspace-relative file.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "search_block": { "type": "string" },
                    "replace_block": { "type": "string" }
                },
                "required": ["path", "search_block", "replace_block"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "set_executable",
            "Mark an existing workspace-relative path executable.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "run_validation",
            "Run fmt, clippy, tests, or custom validation commands.",
            json!({
                "type": "object",
                "properties": {
                    "fmt": { "type": "boolean" },
                    "clippy": { "type": "boolean" },
                    "workspace_tests": { "type": "boolean" },
                    "tests": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "custom_commands": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                },
                "additionalProperties": false
            }),
        ),
    ]
}

fn toml_operations_schema() -> serde_json::Value {
    json!({
        "type": "array",
        "minItems": 1,
        "items": {
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "op": { "const": "set_dependency" },
                        "table": {
                            "type": "string",
                            "enum": ["dependencies", "dev-dependencies", "build-dependencies"]
                        },
                        "name": { "type": "string", "minLength": 1 },
                        "version": { "type": "string" },
                        "features": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "default_features": { "type": "boolean" },
                        "optional": { "type": "boolean" },
                        "package": { "type": "string" },
                        "path": { "type": "string" }
                    },
                    "required": ["op", "table", "name"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": {
                        "op": { "const": "remove_dependency" },
                        "table": {
                            "type": "string",
                            "enum": ["dependencies", "dev-dependencies", "build-dependencies"]
                        },
                        "name": { "type": "string", "minLength": 1 }
                    },
                    "required": ["op", "table", "name"],
                    "additionalProperties": false
                }
            ]
        }
    })
}

fn native_tool_definitions_for_request(request: &StreamRequest) -> Vec<serde_json::Value> {
    let config =
        crate::quorp::tui::agent_context::load_agent_config(request.project_root.as_path());
    let tools = native_tool_definitions();
    let Some(allowed_tools) = native_tool_allowlist_for_request(request) else {
        return filter_native_tools_for_config(tools, &config);
    };
    filter_native_tools_for_config(tools, &config)
        .into_iter()
        .filter(|tool| {
            tool.get("function")
                .and_then(|function| function.get("name"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| allowed_tools.contains(&name))
        })
        .collect()
}

fn filter_native_tools_for_config(
    tools: Vec<serde_json::Value>,
    config: &crate::quorp::tui::agent_context::AgentConfig,
) -> Vec<serde_json::Value> {
    tools
        .into_iter()
        .filter(|tool| {
            let Some(name) = tool
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(serde_json::Value::as_str)
            else {
                return false;
            };
            external_native_tool_enabled(name, config)
        })
        .collect()
}

fn external_native_tool_enabled(
    name: &str,
    config: &crate::quorp::tui::agent_context::AgentConfig,
) -> bool {
    let tools = &config.agent_tools;
    match name {
        "find_files" => tools.enabled && tools.fd.enabled,
        "structural_search" => {
            tools.enabled && tools.ast_grep.enabled && configured_ast_grep_command(tools).is_some()
        }
        "structural_edit_preview" => {
            tools.enabled
                && tools.ast_grep.enabled
                && tools.ast_grep.allow_rewrite_preview
                && configured_ast_grep_command(tools).is_some()
        }
        "cargo_diagnostics" => {
            tools.enabled
                && tools.cargo_diagnostics.enabled
                && quorp_agent_core::command_is_available(&tools.cargo_diagnostics.check_command)
        }
        _ => true,
    }
}

fn configured_ast_grep_command(
    tools: &crate::quorp::tui::agent_context::AgentToolsSettings,
) -> Option<String> {
    if quorp_agent_core::command_is_available(&tools.ast_grep.command) {
        return Some(tools.ast_grep.command.clone());
    }
    if tools.ast_grep.command == "ast-grep" && quorp_agent_core::command_is_available("sg") {
        return Some("sg".to_string());
    }
    None
}

fn native_tool_allowlist_for_request(request: &StreamRequest) -> Option<Vec<&'static str>> {
    if !nvidia_controller_benchmark_profile(request) {
        return None;
    }
    let transcript = request
        .messages
        .iter()
        .rev()
        .take(4)
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if transcript.contains("exactly one `ApplyPreview`")
        || transcript.contains("clean manifest preview already exists")
    {
        return Some(vec!["apply_preview"]);
    }
    if transcript.contains("exactly one `PreviewEdit` with `modify_toml`")
        || transcript.contains("Manifest patch mode is still active")
        || transcript.contains("Manifest patch mode rejected the previous plan")
    {
        return Some(vec!["preview_edit"]);
    }
    if transcript.contains("NeedsFastLoopRerun")
        || transcript.contains("needs_fast_loop_rerun")
        || transcript.contains("rerun the smallest fast loop")
    {
        return Some(vec!["run_command", "run_validation"]);
    }
    None
}

fn native_tool_choice_for_tools(tools: &[serde_json::Value]) -> serde_json::Value {
    if tools.len() != 1 {
        return serde_json::json!("required");
    }
    let Some(name) = tools
        .first()
        .and_then(|tool| tool.get("function"))
        .and_then(|function| function.get("name"))
        .and_then(serde_json::Value::as_str)
    else {
        return serde_json::json!("required");
    };
    serde_json::json!({
        "type": "function",
        "function": {
            "name": name
        }
    })
}

fn function_tool(
    name: &str,
    description: &str,
    parameters: serde_json::Value,
) -> serde_json::Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "strict": true,
            "parameters": parameters
        }
    })
}

pub(crate) fn build_completion_request_body(
    request: &StreamRequest,
    client_config: &SsdMoeClientConfig,
    stream: bool,
) -> serde_json::Value {
    let model_target = resolve_model_target(&request.model_id);
    let mut request_body = build_request_body(
        client_config,
        &SsdMoeChatRequest {
            messages: build_request_messages(request),
            max_tokens: request.max_completion_tokens.or(Some(4096)),
            reasoning_effort: if request.disable_reasoning {
                None
            } else {
                reasoning_effort_for_model(&model_target.provider_model_id)
            },
        },
    );
    if nvidia_kimi_benchmark_profile(request) {
        request_body["chat_template_kwargs"] = serde_json::json!({ "thinking": false });
    }
    request_body["stream"] = serde_json::json!(stream);
    if request.native_tool_calls {
        let native_tools = native_tool_definitions_for_request(request);
        request_body["tool_choice"] = native_tool_choice_for_tools(&native_tools);
        request_body["tools"] = serde_json::Value::Array(native_tools);
        request_body["parallel_tool_calls"] = serde_json::json!(false);
    }
    request_body
}

#[allow(dead_code)]
pub(crate) async fn request_single_completion(
    ssd_moe_runtime: &crate::quorp::tui::ssd_moe_tui::SsdMoeRuntimeHandle,
    request: &StreamRequest,
) -> Result<String, String> {
    request_single_completion_details(ssd_moe_runtime, request)
        .await
        .map(|result| result.content)
}

pub(crate) async fn request_single_completion_details(
    ssd_moe_runtime: &crate::quorp::tui::ssd_moe_tui::SsdMoeRuntimeHandle,
    request: &StreamRequest,
) -> Result<SingleCompletionResult, String> {
    use futures::StreamExt;

    let started_at = std::time::Instant::now();
    let client_config = finalize_client_config_for_request(
        ssd_moe_runtime,
        request,
        resolve_client_config(ssd_moe_runtime, request).map_err(|error| error.to_string())?,
    )
    .await
    .map_err(|error| error.to_string())?;
    let use_stream =
        !(request.native_tool_calls && client_config.provider == InteractiveProviderKind::Local);
    let request_body = build_completion_request_body(request, &client_config.client, use_stream);
    let url =
        chat_completions_url_for_provider(client_config.provider, &client_config.client.base_url)
            .map_err(|error| error.to_string())?;

    let http_client = reqwest::Client::builder()
        .connect_timeout(client_config.client.connect_timeout)
        .read_timeout(client_config.client.read_timeout)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .build()
        .map_err(|error| format!("Failed to build HTTP client: {error}"))?;

    let max_rate_limit_retries = if client_config.provider == InteractiveProviderKind::Nvidia {
        nvidia_rate_limit_retries()
    } else {
        0
    };
    let mut attempt_index = 0_u64;
    let response = loop {
        let mut request_builder = http_client
            .post(url.clone())
            .header(
                reqwest::header::ACCEPT,
                if use_stream {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .json(&request_body);
        if let Some(bearer_token) = client_config.bearer_token.as_deref() {
            request_builder = request_builder.bearer_auth(bearer_token);
        }
        request_builder = apply_extra_headers(request_builder, &client_config.client.extra_headers);
        let response = request_builder.send().await.map_err(|error| {
            format!(
                "Failed to connect to {}: {error}",
                provider_connection_name(client_config.provider)
            )
        })?;
        if response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS
            || attempt_index >= max_rate_limit_retries
        {
            break response;
        }
        let backoff_seconds = nvidia_rate_limit_backoff_seconds(response.headers(), attempt_index);
        tokio::time::sleep(Duration::from_secs(backoff_seconds)).await;
        attempt_index = attempt_index.saturating_add(1);
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<body unavailable>".to_string());
        return Err(format!(
            "{} returned {} while requesting agent completion: {}",
            provider_connection_name(client_config.provider),
            status,
            body.trim()
        ));
    }

    let watchdog = request.watchdog.clone().unwrap_or_default();

    if !use_stream {
        let response_json = response
            .json::<serde_json::Value>()
            .await
            .map_err(|error| format!("Failed to decode completion response JSON: {error}"))?;
        let mut raw_response = response_json.clone();
        let response_id = response_json
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let response_model = response_json
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let finish_reason = response_json
            .get("choices")
            .and_then(serde_json::Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let content = response_json
            .get("choices")
            .and_then(serde_json::Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let reasoning_content = response_json
            .get("choices")
            .and_then(serde_json::Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| {
                message
                    .get("reasoning_content")
                    .or_else(|| message.get("reasoning"))
            })
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut native_tool_builders = BTreeMap::new();
        merge_native_tool_call_chunk(&response_json, &mut native_tool_builders);
        let usage_payload = response_json.get("usage").cloned();
        let mut usage = usage_payload.as_ref().and_then(|usage_payload| {
            parse_usage_payload(
                usage_payload,
                started_at.elapsed().as_millis() as u64,
                finish_reason.clone(),
                response_id.as_deref(),
            )
            .ok()
        });
        if usage.is_none()
            && client_config.provider == InteractiveProviderKind::Local
            && let Some((runtime_usage, runtime_payload)) = fetch_local_runtime_usage(
                &http_client,
                &client_config.client.base_url,
                &client_config.client.model_id,
                request,
            )
            .await
        {
            raw_response = serde_json::json!({
                "response": response_json,
                "runtime": runtime_payload,
            });
            usage = Some(runtime_usage);
        }
        if usage.is_none() {
            let latency_ms = started_at.elapsed().as_millis() as u64;
            let input_tokens = estimate_token_count(&request_body["messages"]);
            let output_tokens = estimate_token_count(&(content.clone() + &reasoning_content));
            usage = Some(quorp_agent_core::TokenUsage {
                input_tokens,
                output_tokens,
                total_billed_tokens: input_tokens.saturating_add(output_tokens),
                reasoning_tokens: (!reasoning_content.is_empty()).then_some(output_tokens),
                cache_read_input_tokens: None,
                cache_write_input_tokens: None,
                provider_request_id: response_id.clone(),
                latency_ms,
                finish_reason: finish_reason.clone().or_else(|| Some("stop".to_string())),
                usage_source: quorp_agent_core::UsageSource::Estimated,
            });
        }

        let finalized_tool_calls = finalized_native_tool_calls(&native_tool_builders);
        let (native_turn, native_turn_error) = if finalized_tool_calls.is_empty() {
            match native_turn_from_content_fallback(&content) {
                Ok(turn) => (turn, None),
                Err(error) => (None, Some(error)),
            }
        } else {
            match native_turn_from_tool_calls(&content, &finalized_tool_calls) {
                Ok(turn) => (turn, None),
                Err(error) => (None, Some(error)),
            }
        };
        if !finalized_tool_calls.is_empty() {
            raw_response = json!({
                "id": response_id,
                "model": response_model,
                "choices": [{
                    "finish_reason": finish_reason,
                    "message": {
                        "content": content,
                        "tool_calls": finalized_tool_calls,
                    }
                }],
                "usage": usage_payload,
            });
        }

        let total_elapsed_ms = started_at.elapsed().as_millis() as u64;
        return Ok(SingleCompletionResult {
            content,
            reasoning_content,
            native_turn,
            native_turn_error,
            usage,
            raw_response: wrap_raw_provider_response(raw_response, &client_config.routing),
            watchdog: Some(quorp_agent_core::ModelRequestWatchdogReport {
                first_token_timeout_ms: watchdog.first_token_timeout_ms,
                idle_timeout_ms: watchdog.idle_timeout_ms,
                total_timeout_ms: watchdog.total_timeout_ms,
                first_token_latency_ms: Some(total_elapsed_ms),
                max_idle_gap_ms: None,
                total_elapsed_ms,
                near_limit: watchdog_near_limit(
                    &watchdog,
                    Some(total_elapsed_ms),
                    0,
                    total_elapsed_ms,
                ),
                triggered_reason: None,
            }),
            routing: client_config.routing,
        });
    }

    let mut bytes_stream = response.bytes_stream();
    let mut line_buffer = String::new();
    let mut content = String::new();
    let mut reasoning_content = String::new();
    let mut usage = None;
    let mut raw_response = serde_json::json!({});
    let mut usage_payload = None;
    let mut response_id = None;
    let mut response_model = None;
    let mut finish_reason = None;
    let mut native_tool_builders = BTreeMap::new();
    let mut stream_finished = false;
    let mut first_token_latency_ms = None;
    let mut last_progress_at = started_at;
    let mut max_idle_gap_ms = 0u64;

    if let Some(first_token_timeout_ms) = watchdog.first_token_timeout_ms
        && started_at.elapsed().as_millis() as u64 >= first_token_timeout_ms
    {
        return Err(format!(
            "first token timeout after {first_token_timeout_ms}ms"
        ));
    }

    loop {
        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        if let Some(total_timeout_ms) = watchdog.total_timeout_ms
            && elapsed_ms >= total_timeout_ms
        {
            return Err(format!("model request timeout after {total_timeout_ms}ms"));
        }
        if content.is_empty()
            && reasoning_content.is_empty()
            && let Some(first_token_timeout_ms) = watchdog.first_token_timeout_ms
            && elapsed_ms >= first_token_timeout_ms
        {
            return Err(format!(
                "first token timeout after {first_token_timeout_ms}ms"
            ));
        }

        let timeout_ms = if content.is_empty() && reasoning_content.is_empty() {
            watchdog.first_token_timeout_ms
        } else {
            watchdog.idle_timeout_ms
        };
        let next_chunk = if let Some(timeout_ms) = timeout_ms {
            match tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms),
                bytes_stream.next(),
            )
            .await
            {
                Ok(chunk) => chunk,
                Err(_) if content.is_empty() && reasoning_content.is_empty() => {
                    return Err(format!("first token timeout after {timeout_ms}ms"));
                }
                Err(_) => {
                    return Err(format!("stream idle timeout after {timeout_ms}ms"));
                }
            }
        } else {
            bytes_stream.next().await
        };
        let Some(chunk) = next_chunk else {
            break;
        };
        let bytes = chunk.map_err(|error| format!("SSD-MOE stream error: {error}"))?;
        let chunk_text = String::from_utf8_lossy(&bytes);
        line_buffer.push_str(&chunk_text);

        while let Some(newline_index) = line_buffer.find('\n') {
            let line = line_buffer[..newline_index].to_string();
            line_buffer.drain(..=newline_index);
            let Some(payload) = parse_sse_data_line(&line) else {
                continue;
            };

            if payload == "[DONE]" {
                stream_finished = true;
                break;
            }

            if let Ok(chunk_val) = serde_json::from_str::<serde_json::Value>(payload) {
                raw_response = chunk_val.clone(); // store the last chunk for raw response mapping
                if let Some(id) = chunk_val.get("id").and_then(|value| value.as_str()) {
                    response_id = Some(id.to_string());
                }
                if let Some(model) = chunk_val.get("model").and_then(|value| value.as_str()) {
                    response_model = Some(model.to_string());
                }
                if let Some(reason) = chunk_val
                    .get("choices")
                    .and_then(|choices| choices.get(0))
                    .and_then(|choice| choice.get("finish_reason"))
                    .and_then(|value| value.as_str())
                {
                    finish_reason = Some(reason.to_string());
                }
                merge_native_tool_call_chunk(&chunk_val, &mut native_tool_builders);
                if let Some(u) = chunk_val.get("usage") {
                    usage_payload = Some(u.clone());
                    let latency_ms = started_at.elapsed().as_millis() as u64;
                    let provider_request_id = chunk_val.get("id").and_then(|v| v.as_str());
                    usage = parse_usage_payload(
                        u,
                        latency_ms,
                        finish_reason.clone(),
                        provider_request_id,
                    )
                    .ok();
                }
            }

            if let Ok(events) = parse_sse_payload(payload) {
                for event in events {
                    match event {
                        SsdMoeStreamEvent::TextDelta(fragment) => {
                            let now = std::time::Instant::now();
                            let idle_gap_ms =
                                now.duration_since(last_progress_at).as_millis() as u64;
                            max_idle_gap_ms = max_idle_gap_ms.max(idle_gap_ms);
                            last_progress_at = now;
                            if first_token_latency_ms.is_none() {
                                first_token_latency_ms =
                                    Some(now.duration_since(started_at).as_millis() as u64);
                            }
                            content.push_str(&fragment);
                        }
                        SsdMoeStreamEvent::ReasoningDelta(fragment) => {
                            let now = std::time::Instant::now();
                            let idle_gap_ms =
                                now.duration_since(last_progress_at).as_millis() as u64;
                            max_idle_gap_ms = max_idle_gap_ms.max(idle_gap_ms);
                            last_progress_at = now;
                            if first_token_latency_ms.is_none() {
                                first_token_latency_ms =
                                    Some(now.duration_since(started_at).as_millis() as u64);
                            }
                            reasoning_content.push_str(&fragment);
                        }
                        SsdMoeStreamEvent::Finished => {}
                    }
                }
            }
        }

        if stream_finished {
            break;
        }
    }

    // Capture remaining if any
    if let Some(payload) = parse_sse_data_line(&line_buffer)
        && payload != "[DONE]"
        && let Ok(events) = parse_sse_payload(payload)
    {
        for event in events {
            match event {
                SsdMoeStreamEvent::TextDelta(fragment) => content.push_str(&fragment),
                SsdMoeStreamEvent::ReasoningDelta(fragment) => {
                    reasoning_content.push_str(&fragment)
                }
                SsdMoeStreamEvent::Finished => {}
            }
        }
    }

    if let Some(payload) = parse_sse_data_line(&line_buffer)
        && payload != "[DONE]"
        && let Ok(chunk_val) = serde_json::from_str::<serde_json::Value>(payload)
    {
        merge_native_tool_call_chunk(&chunk_val, &mut native_tool_builders);
        if let Some(id) = chunk_val.get("id").and_then(|value| value.as_str()) {
            response_id = Some(id.to_string());
        }
        if let Some(model) = chunk_val.get("model").and_then(|value| value.as_str()) {
            response_model = Some(model.to_string());
        }
        if let Some(reason) = chunk_val
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(|value| value.as_str())
        {
            finish_reason = Some(reason.to_string());
        }
        if let Some(u) = chunk_val.get("usage") {
            usage_payload = Some(u.clone());
        }
    }

    if !stream_finished {
        return Err("SSD-MOE stream ended before sending [DONE].".to_string());
    }

    if usage.is_none()
        && client_config.provider == InteractiveProviderKind::Local
        && let Some((runtime_usage, runtime_payload)) = fetch_local_runtime_usage(
            &http_client,
            &client_config.client.base_url,
            &client_config.client.model_id,
            request,
        )
        .await
    {
        raw_response = serde_json::json!({
            "runtime": runtime_payload,
        });
        usage = Some(runtime_usage);
    }

    // Fallback if no usage was streamed
    if usage.is_none() {
        let latency_ms = started_at.elapsed().as_millis() as u64;
        let input_tokens = estimate_token_count(&request_body["messages"]);
        let output_tokens = estimate_token_count(&(content.clone() + &reasoning_content));
        usage = Some(quorp_agent_core::TokenUsage {
            input_tokens,
            output_tokens,
            total_billed_tokens: input_tokens.saturating_add(output_tokens),
            reasoning_tokens: (!reasoning_content.is_empty()).then_some(output_tokens),
            cache_read_input_tokens: None,
            cache_write_input_tokens: None,
            provider_request_id: None,
            latency_ms,
            finish_reason: Some("stop".to_string()),
            usage_source: quorp_agent_core::UsageSource::Estimated,
        });
    }

    let finalized_tool_calls = finalized_native_tool_calls(&native_tool_builders);
    let (native_turn, native_turn_error) = if finalized_tool_calls.is_empty() {
        match native_turn_from_content_fallback(&content) {
            Ok(turn) => (turn, None),
            Err(error) => (None, Some(error)),
        }
    } else {
        match native_turn_from_tool_calls(&content, &finalized_tool_calls) {
            Ok(turn) => (turn, None),
            Err(error) => (None, Some(error)),
        }
    };
    if !finalized_tool_calls.is_empty() {
        raw_response = json!({
            "id": response_id,
            "model": response_model,
            "choices": [{
                "finish_reason": finish_reason,
                "message": {
                    "content": content,
                    "tool_calls": finalized_tool_calls,
                }
            }],
            "usage": usage_payload,
        });
    }

    Ok(SingleCompletionResult {
        content,
        reasoning_content,
        native_turn,
        native_turn_error,
        usage,
        raw_response: wrap_raw_provider_response(raw_response, &client_config.routing),
        watchdog: Some(quorp_agent_core::ModelRequestWatchdogReport {
            first_token_timeout_ms: watchdog.first_token_timeout_ms,
            idle_timeout_ms: watchdog.idle_timeout_ms,
            total_timeout_ms: watchdog.total_timeout_ms,
            first_token_latency_ms,
            max_idle_gap_ms: (max_idle_gap_ms > 0).then_some(max_idle_gap_ms),
            total_elapsed_ms: started_at.elapsed().as_millis() as u64,
            near_limit: watchdog_near_limit(
                &watchdog,
                first_token_latency_ms,
                max_idle_gap_ms,
                started_at.elapsed().as_millis() as u64,
            ),
            triggered_reason: None,
        }),
        routing: client_config.routing,
    })
}

fn estimate_token_count(value: &impl serde::Serialize) -> u64 {
    let text = serde_json::to_string(value).unwrap_or_default();
    let char_count = text.chars().count() as u64;
    char_count.div_ceil(4).max(1)
}

fn parse_local_runtime_usage(payload: &serde_json::Value) -> Option<quorp_agent_core::TokenUsage> {
    let last_request = payload.get("last_request")?;
    let input_tokens = last_request.get("prompt_tokens")?.as_u64()?;
    let output_tokens = last_request.get("emitted_tokens")?.as_u64()?;
    let finish_reason = last_request
        .get("finish_reason")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let latency_ms = match (
        last_request
            .get("started_at_ms")
            .and_then(serde_json::Value::as_u64),
        last_request
            .get("finished_at_ms")
            .and_then(serde_json::Value::as_u64),
    ) {
        (Some(started_at_ms), Some(finished_at_ms)) if finished_at_ms >= started_at_ms => {
            finished_at_ms - started_at_ms
        }
        _ => 0,
    };
    Some(quorp_agent_core::TokenUsage {
        input_tokens,
        output_tokens,
        total_billed_tokens: input_tokens.saturating_add(output_tokens),
        reasoning_tokens: last_request
            .get("reasoning_tokens")
            .and_then(serde_json::Value::as_u64),
        cache_read_input_tokens: last_request
            .get("cached_tokens")
            .and_then(serde_json::Value::as_u64),
        cache_write_input_tokens: last_request
            .get("cache_write_tokens")
            .and_then(serde_json::Value::as_u64),
        provider_request_id: last_request
            .get("request_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                last_request
                    .get("provider_request_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            }),
        latency_ms,
        finish_reason,
        usage_source: quorp_agent_core::UsageSource::Reported,
    })
}

async fn fetch_local_runtime_usage(
    http_client: &reqwest::Client,
    base_url: &str,
    model_id: &str,
    request: &StreamRequest,
) -> Option<(quorp_agent_core::TokenUsage, serde_json::Value)> {
    let runtime_url = format!("{}/runtime", base_url.trim_end_matches('/'));
    let mut request_builder = http_client
        .get(runtime_url)
        .header("X-Quorp-Run-Id", correlation_run_id(&request.project_root))
        .header("X-Quorp-Session-Id", request.session_id.to_string())
        .header("X-Quorp-Request-Id", request.request_id.to_string())
        .header("X-Quorp-Routing-Mode", "local")
        .header(
            "X-Quorp-Action-Contract-Mode",
            if request.native_tool_calls {
                "native_tool_calls_v1"
            } else {
                "json_action_contract_v1"
            },
        )
        .header(
            "X-Quorp-Repo-Capsule-Injected",
            request.include_repo_capsule.to_string(),
        )
        .header(
            "X-Quorp-Reasoning-Enabled",
            (!request.disable_reasoning).to_string(),
        )
        .header("X-Quorp-Executor-Model", model_id)
        .header("X-Quorp-Local-Model", model_id)
        .header("X-WarpOS-Agent", "quorp");
    if let Some(scope) = request.capture_scope.as_deref() {
        request_builder = request_builder.header("X-WarpOS-Scope", scope);
    }
    if let Some(call_class) = request.capture_call_class.as_deref() {
        request_builder = request_builder.header("X-WarpOS-Call-Class", call_class);
    }
    let response = request_builder.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let payload = response.json::<serde_json::Value>().await.ok()?;
    let usage = parse_local_runtime_usage(&payload)?;
    Some((usage, payload))
}

#[derive(Debug, Default, Clone)]
struct NativeToolCallBuilder {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

fn merge_native_tool_call_chunk(
    payload: &serde_json::Value,
    builders: &mut BTreeMap<usize, NativeToolCallBuilder>,
) {
    let Some(choice) = payload
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return;
    };
    let tool_calls = choice
        .get("delta")
        .and_then(|delta| delta.get("tool_calls"))
        .or_else(|| {
            choice
                .get("message")
                .and_then(|message| message.get("tool_calls"))
        })
        .and_then(serde_json::Value::as_array);
    let Some(tool_calls) = tool_calls else {
        return;
    };
    for (ordinal, tool_call) in tool_calls.iter().enumerate() {
        let index = tool_call
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(ordinal);
        let builder = builders.entry(index).or_default();
        if let Some(id) = tool_call.get("id").and_then(serde_json::Value::as_str) {
            builder.id = Some(id.to_string());
        }
        if let Some(name) = tool_call_name(tool_call) {
            builder.name = Some(name.to_string());
        }
        if let Some(arguments) = tool_call_arguments_as_string(tool_call) {
            builder.arguments.push_str(arguments);
        }
    }
}

fn finalized_native_tool_calls(
    builders: &BTreeMap<usize, NativeToolCallBuilder>,
) -> Vec<serde_json::Value> {
    builders
        .values()
        .map(|builder| {
            json!({
                "id": builder.id,
                "type": "function",
                "function": {
                    "name": builder.name,
                    "arguments": builder.arguments,
                }
            })
        })
        .collect()
}

fn native_turn_from_tool_calls(
    assistant_message: &str,
    tool_calls: &[serde_json::Value],
) -> Result<Option<AgentTurnResponse>, String> {
    if tool_calls.is_empty() {
        return Ok(None);
    }
    let mut actions = Vec::new();
    let mut parse_warnings = Vec::new();
    for tool_call in tool_calls {
        match parse_actions_from_tool_call(tool_call) {
            Ok(parsed_actions) => actions.extend(parsed_actions),
            Err(error) => parse_warnings.push(error),
        }
    }
    if actions.is_empty() {
        return Err(parse_warnings.join(" | "));
    }
    Ok(Some(AgentTurnResponse {
        assistant_message: assistant_message.trim().to_string(),
        actions,
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings,
    }))
}

fn native_turn_from_content_fallback(content: &str) -> Result<Option<AgentTurnResponse>, String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    for (assistant_message, candidate) in native_turn_json_candidates(content) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&candidate) else {
            continue;
        };
        if let Some(turn) = native_turn_from_json_value(&value, &assistant_message)? {
            return Ok(Some(turn));
        }
    }
    if let Some(turn) = native_turn_from_pseudo_tool_lines(content)? {
        return Ok(Some(turn));
    }
    Ok(None)
}

fn native_turn_from_json_value(
    value: &serde_json::Value,
    assistant_message_fallback: &str,
) -> Result<Option<AgentTurnResponse>, String> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let assistant_message = object
        .get("assistant_message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(assistant_message_fallback);
    if let Some(tool_calls) = object
        .get("tool_calls")
        .and_then(serde_json::Value::as_array)
    {
        return native_turn_from_tool_calls(assistant_message, tool_calls);
    }
    if tool_call_name(value).is_some() {
        let single_tool_call = vec![value.clone()];
        return native_turn_from_tool_calls(assistant_message, &single_tool_call);
    }
    Ok(None)
}

fn native_turn_json_candidates(content: &str) -> Vec<(String, String)> {
    let mut candidates = Vec::new();
    let trimmed = content.trim();
    if !trimmed.is_empty() {
        candidates.push((String::new(), trimmed.to_string()));
    }

    let mut search_offset = 0usize;
    while let Some(fence_start) = content[search_offset..].find("```") {
        let fence_start = search_offset + fence_start;
        let language_start = fence_start + 3;
        let Some(first_newline_rel) = content[language_start..].find('\n') else {
            break;
        };
        let body_start = language_start + first_newline_rel + 1;
        let Some(fence_end_rel) = content[body_start..].find("```") else {
            break;
        };
        let fence_end = body_start + fence_end_rel;
        let body = content[body_start..fence_end].trim();
        if !body.is_empty() {
            let assistant_message = content[..fence_start].trim().to_string();
            candidates.push((assistant_message, body.to_string()));
        }
        search_offset = fence_end + 3;
    }

    if let Some(object_text) = first_balanced_json_object(trimmed)
        && object_text != trimmed
    {
        let assistant_message = trimmed
            .split_once(object_text)
            .map(|(prefix, _)| prefix.trim().to_string())
            .unwrap_or_default();
        candidates.push((assistant_message, object_text.to_string()));
    }

    candidates
}

fn first_balanced_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = start + index + ch.len_utf8();
                    return Some(&text[start..end]);
                }
            }
            _ => {}
        }
    }
    None
}

fn native_turn_from_pseudo_tool_lines(content: &str) -> Result<Option<AgentTurnResponse>, String> {
    let mut actions = Vec::new();
    let mut parse_warnings = Vec::new();
    let mut assistant_lines = Vec::new();
    let mut seen_signatures = HashSet::new();
    let mut saw_tool_intent = false;
    let lines = content.lines().map(str::trim).collect::<Vec<_>>();

    let mut index = 0usize;
    while index < lines.len() {
        let line = lines[index];
        if line.is_empty() || line.starts_with("```") {
            index += 1;
            continue;
        }
        if let Some(tool_call) =
            pseudo_tool_call_from_line_pair(line, lines.get(index + 1).copied())
        {
            saw_tool_intent = true;
            let signature = serde_json::to_string(&tool_call).unwrap_or_else(|_| line.to_string());
            if seen_signatures.insert(signature) {
                match parse_actions_from_tool_call(&tool_call) {
                    Ok(parsed_actions) => {
                        for action in parsed_actions {
                            actions.push(action);
                            if actions.len() >= 4 {
                                break;
                            }
                        }
                        if actions.len() >= 4 {
                            break;
                        }
                    }
                    Err(error) => parse_warnings.push(format!("{line}: {error}")),
                }
            }
            index += 2;
            continue;
        }
        if let Some(tool_call) = pseudo_tool_call_from_line(line) {
            saw_tool_intent = true;
            let signature = serde_json::to_string(&tool_call).unwrap_or_else(|_| line.to_string());
            if !seen_signatures.insert(signature) {
                index += 1;
                continue;
            }
            match parse_actions_from_tool_call(&tool_call) {
                Ok(parsed_actions) => {
                    for action in parsed_actions {
                        actions.push(action);
                        if actions.len() >= 4 {
                            break;
                        }
                    }
                    if actions.len() >= 4 {
                        break;
                    }
                }
                Err(error) => parse_warnings.push(format!("{line}: {error}")),
            }
            index += 1;
            continue;
        }
        if !saw_tool_intent && assistant_lines.len() < 2 {
            assistant_lines.push(line.to_string());
        }
        index += 1;
    }

    if actions.is_empty() {
        return Ok(None);
    }

    Ok(Some(AgentTurnResponse {
        assistant_message: assistant_lines.join(" ").trim().to_string(),
        actions,
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings,
    }))
}

fn pseudo_tool_call_from_line_pair(
    line: &str,
    next_line: Option<&str>,
) -> Option<serde_json::Value> {
    let (raw_tool_name, remainder) = split_pseudo_tool_line(line)?;
    let tool_name = normalize_tool_name(raw_tool_name);
    let mut arguments = arguments_object_from_line(next_line?)?;
    if let serde_json::Value::Object(object) = &mut arguments
        && object.get("path").is_none()
        && let Some(path) = pseudo_tool_path_from_remainder(remainder)
    {
        object.insert(
            "path".to_string(),
            serde_json::Value::String(path.to_string()),
        );
    }
    Some(json!({ "tool_name": tool_name, "tool_args": arguments }))
}

fn arguments_object_from_line(line: &str) -> Option<serde_json::Value> {
    let trimmed = line.trim();
    let rest = trimmed
        .strip_prefix("arguments")
        .or_else(|| trimmed.strip_prefix("Arguments"))?
        .trim_start_matches(|character: char| character == ':' || character.is_whitespace());
    let object_text = first_balanced_json_object(rest)?;
    serde_json::from_str::<serde_json::Value>(object_text)
        .ok()
        .filter(serde_json::Value::is_object)
}

fn pseudo_tool_path_from_remainder(remainder: &str) -> Option<&str> {
    let trimmed = remainder.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = trimmed
        .split_whitespace()
        .next()?
        .trim_matches(|character| matches!(character, '`' | '"' | '\'' | ',' | ':' | '[' | ']'));
    (!candidate.is_empty()).then_some(candidate)
}

fn pseudo_tool_call_from_line(line: &str) -> Option<serde_json::Value> {
    let (raw_tool_name, remainder) = split_pseudo_tool_line(line)?;
    let tool_name = normalize_tool_name(raw_tool_name);
    let fields = extract_named_fields(
        remainder,
        &[
            "path",
            "query",
            "limit",
            "command",
            "timeout_ms",
            "content",
            "patch",
            "search_block",
            "replace_block",
            "fmt",
            "clippy",
            "workspace_tests",
            "tests",
            "custom_commands",
        ],
    );

    match tool_name {
        "read_file" | "list_directory" | "set_executable" => fields
            .get("path")
            .map(|path| json!({ "tool_name": tool_name, "tool_args": { "path": path } })),
        "search_text" | "search_symbols" => {
            let query = fields.get("query")?;
            let mut tool_args = serde_json::Map::new();
            tool_args.insert(
                "query".to_string(),
                serde_json::Value::String(query.clone()),
            );
            if let Some(limit) = fields
                .get("limit")
                .and_then(|value| value.parse::<u64>().ok())
            {
                tool_args.insert("limit".to_string(), serde_json::json!(limit));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        "get_repo_capsule" => {
            let mut tool_args = serde_json::Map::new();
            if let Some(query) = fields.get("query") {
                tool_args.insert(
                    "query".to_string(),
                    serde_json::Value::String(query.clone()),
                );
            }
            if let Some(limit) = fields
                .get("limit")
                .and_then(|value| value.parse::<u64>().ok())
            {
                tool_args.insert("limit".to_string(), serde_json::json!(limit));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        "run_command" => {
            let command = fields.get("command")?;
            let mut tool_args = serde_json::Map::new();
            tool_args.insert(
                "command".to_string(),
                serde_json::Value::String(command.clone()),
            );
            if let Some(timeout_ms) = fields
                .get("timeout_ms")
                .and_then(|value| value.parse::<u64>().ok())
            {
                tool_args.insert("timeout_ms".to_string(), serde_json::json!(timeout_ms));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        _ => None,
    }
}

fn split_pseudo_tool_line(line: &str) -> Option<(&str, &str)> {
    [
        "RunCommand",
        "ReadFile",
        "ListDirectory",
        "SearchText",
        "SearchSymbols",
        "GetRepoCapsule",
        "ExplainValidationFailure",
        "SuggestImplementationTargets",
        "SuggestEditAnchors",
        "PreviewEdit",
        "ReplaceRange",
        "ModifyToml",
        "ApplyPreview",
        "WriteFile",
        "ApplyPatch",
        "ReplaceBlock",
        "SetExecutable",
        "RunValidation",
    ]
    .into_iter()
    .find_map(|name| {
        let remainder = line.strip_prefix(name)?;
        let boundary = remainder.chars().next()?;
        if boundary.is_whitespace() || boundary == ':' {
            Some((name, remainder.trim()))
        } else {
            None
        }
    })
}

fn extract_named_fields(input: &str, keys: &[&str]) -> HashMap<String, String> {
    let mut markers = Vec::new();
    for key in keys {
        let needle = format!("{key}:");
        let mut search_start = 0usize;
        while let Some(relative_index) = input[search_start..].find(&needle) {
            let index = search_start + relative_index;
            let boundary_ok = index == 0
                || input[..index].chars().last().is_some_and(|character| {
                    character.is_whitespace() || matches!(character, '(' | '[' | '{' | ',' | ';')
                });
            if boundary_ok {
                markers.push((index, *key));
            }
            search_start = index + needle.len();
        }
    }
    markers.sort_by_key(|(index, _)| *index);
    markers.dedup_by_key(|(index, _)| *index);

    let mut fields = HashMap::new();
    for (ordinal, (start, key)) in markers.iter().enumerate() {
        let value_start = start + key.len() + 1;
        let value_end = markers
            .get(ordinal + 1)
            .map(|(next_start, _)| *next_start)
            .unwrap_or_else(|| input.len());
        let value = input[value_start..value_end]
            .trim()
            .trim_end_matches(',')
            .trim()
            .trim_matches('"')
            .trim_matches('`')
            .trim();
        if !value.is_empty() {
            fields.insert((*key).to_string(), value.to_string());
        }
    }
    fields
}

fn parse_actions_from_tool_call(tool_call: &serde_json::Value) -> Result<Vec<AgentAction>, String> {
    let raw_tool_name = tool_call_name(tool_call)
        .ok_or_else(|| "native tool call was missing a tool name".to_string())?;
    let tool_name = normalize_tool_name(raw_tool_name);
    let argument_values = tool_call_argument_values(tool_call, tool_name)?;
    let mut actions = Vec::with_capacity(argument_values.len());
    for arguments in &argument_values {
        actions.push(parse_action_from_arguments(tool_name, arguments)?);
    }
    Ok(actions)
}

fn parse_action_from_arguments(
    tool_name: &str,
    arguments: &serde_json::Value,
) -> Result<AgentAction, String> {
    match tool_name {
        "run_command" => Ok(AgentAction::RunCommand {
            command: required_string_argument(arguments, tool_name, "command")?,
            timeout_ms: optional_u64_argument(arguments, "timeout_ms").unwrap_or(30_000),
        }),
        "read_file" => Ok(AgentAction::ReadFile {
            path: required_string_argument(arguments, tool_name, "path")?,
            range: optional_read_file_range_argument(arguments, "range"),
        }),
        "list_directory" => Ok(AgentAction::ListDirectory {
            path: required_string_argument(arguments, tool_name, "path")?,
        }),
        "search_text" => Ok(AgentAction::SearchText {
            query: required_string_argument(arguments, tool_name, "query")?,
            limit: optional_usize_argument(arguments, "limit").unwrap_or(8),
        }),
        "search_symbols" => Ok(AgentAction::SearchSymbols {
            query: required_string_argument(arguments, tool_name, "query")?,
            limit: optional_usize_argument(arguments, "limit").unwrap_or(8),
        }),
        "find_files" => Ok(AgentAction::FindFiles {
            query: required_string_argument(arguments, tool_name, "query")?,
            limit: optional_usize_argument(arguments, "limit").unwrap_or(12),
        }),
        "structural_search" => Ok(AgentAction::StructuralSearch {
            pattern: required_string_argument(arguments, tool_name, "pattern")?,
            language: optional_string_argument(arguments, "language"),
            path: optional_string_argument(arguments, "path"),
            limit: optional_usize_argument(arguments, "limit").unwrap_or(8),
        }),
        "structural_edit_preview" => Ok(AgentAction::StructuralEditPreview {
            pattern: required_string_argument(arguments, tool_name, "pattern")?,
            rewrite: required_string_argument(arguments, tool_name, "rewrite")?,
            language: optional_string_argument(arguments, "language"),
            path: optional_string_argument(arguments, "path"),
        }),
        "cargo_diagnostics" => Ok(AgentAction::CargoDiagnostics {
            command: optional_string_argument(arguments, "command"),
            include_clippy: optional_bool_argument(arguments, "include_clippy").unwrap_or(false),
        }),
        "get_repo_capsule" => Ok(AgentAction::GetRepoCapsule {
            query: optional_string_argument(arguments, "query"),
            limit: optional_usize_argument(arguments, "limit").unwrap_or(8),
        }),
        "explain_validation_failure" => Ok(AgentAction::ExplainValidationFailure {
            command: required_string_argument(arguments, tool_name, "command")?,
            output: required_string_argument(arguments, tool_name, "output")?,
        }),
        "suggest_implementation_targets" => Ok(AgentAction::SuggestImplementationTargets {
            command: required_string_argument(arguments, tool_name, "command")?,
            output: required_string_argument(arguments, tool_name, "output")?,
            failing_path: optional_string_argument(arguments, "failing_path"),
            failing_line: optional_usize_argument(arguments, "failing_line"),
        }),
        "suggest_edit_anchors" => Ok(AgentAction::SuggestEditAnchors {
            path: required_string_argument(arguments, tool_name, "path")?,
            range: optional_read_file_range_argument(arguments, "range"),
            search_hint: optional_string_argument(arguments, "search_hint"),
        }),
        "preview_edit" => {
            let path = required_string_argument(arguments, tool_name, "path")?;
            match preview_edit_payload_argument(arguments) {
                Ok(edit) => Ok(AgentAction::PreviewEdit { path, edit }),
                Err(error) => {
                    if let Some(range) = preview_edit_hash_prerequisite_range(arguments) {
                        Ok(AgentAction::ReadFile { path, range })
                    } else {
                        Err(error)
                    }
                }
            }
        }
        "replace_range" => {
            let path = required_string_argument(arguments, tool_name, "path")?;
            let range = required_read_file_range_argument(arguments, tool_name, "range")?;
            let Some(expected_hash) = optional_string_argument_alias(
                arguments,
                &["expected_hash", "content_hash", "hash"],
            ) else {
                return Ok(AgentAction::ReadFile {
                    path,
                    range: Some(range),
                });
            };
            Ok(AgentAction::ReplaceRange {
                path,
                range,
                expected_hash,
                replacement: required_string_argument(arguments, tool_name, "replacement")?,
            })
        }
        "modify_toml" => {
            let path = required_string_argument(arguments, tool_name, "path")?;
            let Some(expected_hash) = optional_string_argument_alias(
                arguments,
                &["expected_hash", "content_hash", "hash"],
            ) else {
                return Ok(AgentAction::ReadFile { path, range: None });
            };
            Ok(AgentAction::ModifyToml {
                path,
                expected_hash,
                operations: toml_operations_argument(arguments, tool_name)?,
            })
        }
        "apply_preview" => Ok(AgentAction::ApplyPreview {
            preview_id: required_string_argument(arguments, tool_name, "preview_id")?,
        }),
        "write_file" => Ok(AgentAction::WriteFile {
            path: required_string_argument(arguments, tool_name, "path")?,
            content: required_string_argument(arguments, tool_name, "content")?,
        }),
        "apply_patch" => Ok(AgentAction::ApplyPatch {
            path: required_string_argument(arguments, tool_name, "path")?,
            patch: required_string_argument(arguments, tool_name, "patch")?,
        }),
        "replace_block" => Ok(AgentAction::ReplaceBlock {
            path: required_string_argument(arguments, tool_name, "path")?,
            search_block: required_string_argument(arguments, tool_name, "search_block")?,
            replace_block: required_string_argument(arguments, tool_name, "replace_block")?,
            range: optional_read_file_range_argument(arguments, "range"),
        }),
        "set_executable" => Ok(AgentAction::SetExecutable {
            path: required_string_argument(arguments, tool_name, "path")?,
        }),
        "run_validation" => Ok(AgentAction::RunValidation {
            plan: ValidationPlan {
                fmt: optional_bool_argument(arguments, "fmt").unwrap_or(false),
                clippy: optional_bool_argument(arguments, "clippy").unwrap_or(false),
                workspace_tests: optional_bool_argument(arguments, "workspace_tests")
                    .unwrap_or(false),
                tests: optional_string_list_argument(arguments, "tests"),
                custom_commands: optional_string_list_argument(arguments, "custom_commands"),
            },
        }),
        other => Err(format!("unsupported native tool call `{other}`")),
    }
}

fn tool_call_name(tool_call: &serde_json::Value) -> Option<&str> {
    tool_call
        .get("function")
        .and_then(|function| function.get("name"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            tool_call
                .get("tool_name")
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| tool_call.get("name").and_then(serde_json::Value::as_str))
        .or_else(|| tool_call.get("type").and_then(serde_json::Value::as_str))
}

fn tool_call_argument_values(
    tool_call: &serde_json::Value,
    tool_name: &str,
) -> Result<Vec<serde_json::Value>, String> {
    if let Some(raw_arguments) = tool_call_arguments_as_string(tool_call) {
        return parse_tool_argument_values(tool_name, raw_arguments);
    }
    if let Some(arguments) = tool_call
        .get("function")
        .and_then(|function| function.get("arguments"))
        .or_else(|| tool_call.get("tool_args"))
        .or_else(|| tool_call.get("arguments"))
        .cloned()
    {
        return normalize_tool_argument_values(tool_name, vec![arguments]);
    }
    if let Some(arguments) = tool_call_inline_arguments(tool_call) {
        return normalize_tool_argument_values(tool_name, vec![arguments]);
    }
    Ok(vec![json!({})])
}

fn tool_call_arguments_as_string(tool_call: &serde_json::Value) -> Option<&str> {
    tool_call
        .get("function")
        .and_then(|function| function.get("arguments"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            tool_call
                .get("arguments")
                .and_then(serde_json::Value::as_str)
        })
}

fn tool_call_inline_arguments(tool_call: &serde_json::Value) -> Option<serde_json::Value> {
    let object = tool_call.as_object()?;
    let arguments = object
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "id" | "index"
                    | "type"
                    | "name"
                    | "function"
                    | "arguments"
                    | "tool_name"
                    | "tool_args"
                    | "tool_call_id"
                    | "tool_call_type"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<String, serde_json::Value>>();
    (!arguments.is_empty()).then_some(serde_json::Value::Object(arguments))
}

fn normalize_tool_name(raw: &str) -> &str {
    match raw {
        "RunCommand" => "run_command",
        "ReadFile" => "read_file",
        "ListDirectory" => "list_directory",
        "SearchText" => "search_text",
        "SearchSymbols" => "search_symbols",
        "FindFiles" => "find_files",
        "StructuralSearch" => "structural_search",
        "StructuralEditPreview" => "structural_edit_preview",
        "CargoDiagnostics" => "cargo_diagnostics",
        "GetRepoCapsule" => "get_repo_capsule",
        "ExplainValidationFailure" => "explain_validation_failure",
        "SuggestImplementationTargets" => "suggest_implementation_targets",
        "SuggestEditAnchors" => "suggest_edit_anchors",
        "PreviewEdit" => "preview_edit",
        "ReplaceRange" => "replace_range",
        "ModifyToml" => "modify_toml",
        "ApplyPreview" => "apply_preview",
        "WriteFile" => "write_file",
        "ApplyPatch" => "apply_patch",
        "ReplaceBlock" => "replace_block",
        "SetExecutable" => "set_executable",
        "McpCallTool" => "mcp_call_tool",
        "RunValidation" => "run_validation",
        other => other,
    }
}

fn parse_tool_argument_values(
    tool_name: &str,
    raw_arguments: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let trimmed = raw_arguments.trim();
    let normalized = if trimmed.is_empty() { "{}" } else { trimmed };
    if let Ok(arguments) = serde_json::from_str::<serde_json::Value>(normalized) {
        return normalize_tool_argument_values(tool_name, vec![arguments]);
    }

    let mut arguments = Vec::new();
    for value in serde_json::Deserializer::from_str(normalized).into_iter::<serde_json::Value>() {
        let value = value.map_err(|error| {
            format!("native tool `{tool_name}` arguments were invalid JSON: {error}")
        })?;
        arguments.push(value);
    }
    if arguments.is_empty() {
        return Err(format!(
            "native tool `{tool_name}` arguments were invalid JSON: empty payload"
        ));
    }
    normalize_tool_argument_values(tool_name, arguments)
}

fn normalize_tool_argument_values(
    tool_name: &str,
    raw_values: Vec<serde_json::Value>,
) -> Result<Vec<serde_json::Value>, String> {
    let mut normalized = Vec::new();
    for value in raw_values {
        match value {
            serde_json::Value::Object(_) => normalized.push(value),
            serde_json::Value::Array(items) => {
                for item in items {
                    if item.is_object() {
                        normalized.push(item);
                    } else {
                        return Err(format!(
                            "native tool `{tool_name}` arguments must be JSON objects"
                        ));
                    }
                }
            }
            _ => {
                return Err(format!(
                    "native tool `{tool_name}` arguments must be JSON objects"
                ));
            }
        }
    }
    if normalized.is_empty() {
        normalized.push(json!({}));
    }
    Ok(normalized)
}

fn required_string_argument(
    arguments: &serde_json::Value,
    tool_name: &str,
    field: &str,
) -> Result<String, String> {
    arguments
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("native tool `{tool_name}` was missing `{field}`"))
}

fn optional_string_argument(arguments: &serde_json::Value, field: &str) -> Option<String> {
    arguments
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn optional_string_argument_alias(
    arguments: &serde_json::Value,
    fields: &[&str],
) -> Option<String> {
    fields
        .iter()
        .filter_map(|field| optional_string_argument(arguments, field))
        .next()
}

fn preview_edit_hash_prerequisite_range(
    arguments: &serde_json::Value,
) -> Option<Option<ReadFileRange>> {
    if let Some(edit_object) = arguments.get("edit").and_then(serde_json::Value::as_object) {
        if let Some(modify_toml) = edit_object
            .get("ModifyToml")
            .or_else(|| edit_object.get("modify_toml"))
            .and_then(serde_json::Value::as_object)
            && string_argument_alias(modify_toml, &["expected_hash", "content_hash", "hash"])
                .is_none()
        {
            return Some(None);
        }
        if let Some(replace_range) = edit_object
            .get("ReplaceRange")
            .or_else(|| edit_object.get("replace_range"))
            .and_then(serde_json::Value::as_object)
            && string_argument_alias(replace_range, &["expected_hash", "content_hash", "hash"])
                .is_none()
        {
            let value = serde_json::Value::Object(replace_range.clone());
            return Some(optional_read_file_range_argument(&value, "range"));
        }
        if edit_object.get("operations").is_some()
            && string_argument_alias(edit_object, &["expected_hash", "content_hash", "hash"])
                .is_none()
        {
            return Some(None);
        }
        if edit_object.get("replacement").is_some()
            && edit_object.get("range").is_some()
            && string_argument_alias(edit_object, &["expected_hash", "content_hash", "hash"])
                .is_none()
        {
            let value = serde_json::Value::Object(edit_object.clone());
            return Some(optional_read_file_range_argument(&value, "range"));
        }
    }
    if arguments.get("operations").is_some()
        && optional_string_argument_alias(arguments, &["expected_hash", "content_hash", "hash"])
            .is_none()
    {
        return Some(None);
    }
    if arguments.get("replacement").is_some()
        && arguments.get("range").is_some()
        && optional_string_argument_alias(arguments, &["expected_hash", "content_hash", "hash"])
            .is_none()
    {
        return Some(optional_read_file_range_argument(arguments, "range"));
    }
    None
}

fn optional_read_file_range_argument(
    arguments: &serde_json::Value,
    field: &str,
) -> Option<ReadFileRange> {
    let value = arguments.get(field)?;
    match value {
        serde_json::Value::Array(values) if values.len() == 2 => {
            let start_line = values.first()?.as_u64()? as usize;
            let end_line = values.get(1)?.as_u64()? as usize;
            ReadFileRange {
                start_line,
                end_line,
            }
            .normalized()
        }
        serde_json::Value::Object(map) => {
            let start_line = map.get("start_line")?.as_u64()? as usize;
            let end_line = map.get("end_line")?.as_u64()? as usize;
            ReadFileRange {
                start_line,
                end_line,
            }
            .normalized()
        }
        _ => None,
    }
}

fn required_read_file_range_argument(
    arguments: &serde_json::Value,
    tool_name: &str,
    field: &str,
) -> Result<ReadFileRange, String> {
    optional_read_file_range_argument(arguments, field)
        .ok_or_else(|| format!("native tool `{tool_name}` was missing valid `{field}`"))
}

fn toml_operations_argument(
    arguments: &serde_json::Value,
    tool_name: &str,
) -> Result<Vec<quorp_agent_core::agent_protocol::TomlEditOperation>, String> {
    let operations = arguments
        .get("operations")
        .ok_or_else(|| format!("native tool `{tool_name}` was missing `operations`"))?;
    let operations = normalize_toml_operations_value(operations.clone())
        .ok_or_else(|| format!("native tool `{tool_name}` had invalid `operations`"))?;
    serde_json::from_value::<Vec<quorp_agent_core::agent_protocol::TomlEditOperation>>(operations)
        .map_err(|error| format!("native tool `{tool_name}` had invalid `operations`: {error}"))
}

fn normalize_toml_operations_value(value: serde_json::Value) -> Option<serde_json::Value> {
    let serde_json::Value::Array(items) = value else {
        return None;
    };
    let normalized = items
        .into_iter()
        .map(normalize_toml_operation_value)
        .collect::<Option<Vec<_>>>()?;
    Some(serde_json::Value::Array(normalized))
}

fn normalize_toml_operation_value(value: serde_json::Value) -> Option<serde_json::Value> {
    if serde_json::from_value::<quorp_agent_core::agent_protocol::TomlEditOperation>(value.clone())
        .is_ok()
    {
        return Some(value);
    }
    let serde_json::Value::Object(mut object) = value else {
        return None;
    };
    for (key, op) in [
        ("set_dependency", "set_dependency"),
        ("SetDependency", "set_dependency"),
        ("remove_dependency", "remove_dependency"),
        ("RemoveDependency", "remove_dependency"),
    ] {
        if let Some(payload) = object.remove(key) {
            let serde_json::Value::Object(mut payload) = payload else {
                return None;
            };
            normalize_toml_dependency_map_shorthand(&mut payload, op);
            payload.insert("op".to_string(), serde_json::Value::String(op.to_string()));
            normalize_toml_operation_aliases(&mut payload);
            return Some(serde_json::Value::Object(payload));
        }
    }
    if object.get("op").is_none()
        && let Some(op) = object
            .get("operation")
            .or_else(|| object.get("kind"))
            .or_else(|| object.get("type"))
            .or_else(|| object.get("action"))
            .and_then(serde_json::Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase().replace(['-', ' '], "_"))
    {
        object.insert("op".to_string(), serde_json::Value::String(op));
    }
    if object.get("op").is_none() && object.get("table").is_some() && object.get("name").is_some() {
        let inferred = if object
            .get("remove")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            "remove_dependency"
        } else {
            "set_dependency"
        };
        object.insert(
            "op".to_string(),
            serde_json::Value::String(inferred.to_string()),
        );
    }
    normalize_toml_operation_aliases(&mut object);
    Some(serde_json::Value::Object(object))
}

fn normalize_toml_operation_aliases(object: &mut serde_json::Map<String, serde_json::Value>) {
    if object.get("name").is_none() {
        for alias in [
            "dependency",
            "dependency_name",
            "crate",
            "crate_name",
            "package_name",
        ] {
            if let Some(value) = object.get(alias).cloned() {
                object.insert("name".to_string(), value);
                break;
            }
        }
    }
    if object.get("table").is_none()
        && object
            .get("op")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|op| matches!(op, "set_dependency" | "remove_dependency"))
    {
        object.insert(
            "table".to_string(),
            serde_json::Value::String("dependencies".to_string()),
        );
    }
    if object.get("default_features").is_none()
        && let Some(value) = object.get("default-features").cloned()
    {
        object.insert("default_features".to_string(), value);
    }
    if object.get("version").is_none()
        && let Some(value) = object.get("spec").cloned()
    {
        object.insert("version".to_string(), value);
    }
}

fn normalize_toml_dependency_map_shorthand(
    object: &mut serde_json::Map<String, serde_json::Value>,
    op: &str,
) {
    if object.get("name").is_some() || object.len() != 1 {
        return;
    }
    let Some((candidate_name, candidate_value)) = object
        .iter()
        .next()
        .map(|(key, value)| (key.clone(), value.clone()))
    else {
        return;
    };
    if is_toml_operation_field(&candidate_name) {
        return;
    }
    object.insert(
        "name".to_string(),
        serde_json::Value::String(candidate_name),
    );
    if op == "set_dependency" && object.get("version").is_none() {
        if let Some(version) = candidate_value.as_str() {
            object.insert(
                "version".to_string(),
                serde_json::Value::String(version.to_string()),
            );
        } else if let Some(fields) = candidate_value.as_object() {
            for (key, value) in fields {
                object.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }
}

fn is_toml_operation_field(field: &str) -> bool {
    matches!(
        field,
        "op" | "operation"
            | "kind"
            | "type"
            | "action"
            | "table"
            | "name"
            | "dependency"
            | "dependency_name"
            | "crate"
            | "crate_name"
            | "package"
            | "package_name"
            | "version"
            | "spec"
            | "features"
            | "default_features"
            | "default-features"
            | "optional"
            | "path"
            | "remove"
    )
}

fn preview_edit_payload_argument(
    arguments: &serde_json::Value,
) -> Result<PreviewEditPayload, String> {
    if let Some(edit) = arguments.get("edit")
        && let Ok(payload) = serde_json::from_value::<PreviewEditPayload>(edit.clone())
    {
        return Ok(payload);
    }
    if let Some(edit_object) = arguments.get("edit").and_then(serde_json::Value::as_object) {
        if let Some(apply_patch) = edit_object
            .get("ApplyPatch")
            .or_else(|| edit_object.get("apply_patch"))
        {
            let patch = apply_patch
                .get("patch")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    "native tool `preview_edit` edit.apply_patch was missing `patch`".to_string()
                })?;
            return Ok(PreviewEditPayload::ApplyPatch {
                patch: patch.to_string(),
            });
        }
        if let Some(replace_range) = edit_object
            .get("ReplaceRange")
            .or_else(|| edit_object.get("replace_range"))
        {
            return Ok(PreviewEditPayload::ReplaceRange {
                range: required_read_file_range_argument(
                    replace_range,
                    "preview_edit.replace_range",
                    "range",
                )?,
                expected_hash: required_string_argument(
                    replace_range,
                    "preview_edit.replace_range",
                    "expected_hash",
                )?,
                replacement: required_string_argument(
                    replace_range,
                    "preview_edit.replace_range",
                    "replacement",
                )?,
            });
        }
        if let Some(modify_toml) = edit_object
            .get("ModifyToml")
            .or_else(|| edit_object.get("modify_toml"))
        {
            return Ok(PreviewEditPayload::ModifyToml {
                expected_hash: required_string_argument(
                    modify_toml,
                    "preview_edit.modify_toml",
                    "expected_hash",
                )?,
                operations: toml_operations_argument(modify_toml, "preview_edit.modify_toml")?,
            });
        }
        if let Some(replace_block) = edit_object
            .get("ReplaceBlock")
            .or_else(|| edit_object.get("replace_block"))
        {
            return Ok(PreviewEditPayload::ReplaceBlock {
                search_block: required_string_argument(
                    replace_block,
                    "preview_edit.replace_block",
                    "search_block",
                )?,
                replace_block: required_string_argument(
                    replace_block,
                    "preview_edit.replace_block",
                    "replace_block",
                )?,
                range: optional_read_file_range_argument(replace_block, "range"),
            });
        }
        let edit_kind = edit_object
            .get("kind")
            .or_else(|| edit_object.get("type"))
            .and_then(serde_json::Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase().replace(['-', ' '], "_"));
        if edit_kind
            .as_deref()
            .is_some_and(|kind| matches!(kind, "applypatch" | "apply_patch" | "patch"))
            || edit_object.get("patch").is_some()
        {
            let patch = edit_object
                .get("patch")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "native tool `preview_edit` edit was missing `patch`".to_string())?;
            return Ok(PreviewEditPayload::ApplyPatch {
                patch: patch.to_string(),
            });
        }
        if edit_kind
            .as_deref()
            .is_some_and(|kind| matches!(kind, "replaceblock" | "replace_block" | "replace"))
            || edit_object.get("search_block").is_some()
            || edit_object.get("search").is_some()
        {
            let search_block =
                string_argument_alias(edit_object, &["search_block", "search", "old"]).ok_or_else(
                    || "native tool `preview_edit` edit was missing `search_block`".to_string(),
                )?;
            let replace_block = string_argument_alias(
                edit_object,
                &[
                    "replace_block",
                    "replace_with",
                    "replacement",
                    "replace",
                    "new",
                ],
            )
            .ok_or_else(|| {
                "native tool `preview_edit` edit was missing `replace_block`".to_string()
            })?;
            return Ok(PreviewEditPayload::ReplaceBlock {
                search_block,
                replace_block,
                range: {
                    let edit_value = serde_json::Value::Object(edit_object.clone());
                    optional_read_file_range_argument(&edit_value, "range")
                        .or_else(|| optional_read_file_range_argument(&edit_value, "line_range"))
                },
            });
        }
        if edit_kind
            .as_deref()
            .is_some_and(|kind| matches!(kind, "replacerange" | "replace_range"))
            || edit_object.get("replacement").is_some()
                && edit_object.get("expected_hash").is_some()
                && edit_object.get("range").is_some()
        {
            let edit_value = serde_json::Value::Object(edit_object.clone());
            return Ok(PreviewEditPayload::ReplaceRange {
                range: required_read_file_range_argument(
                    &edit_value,
                    "preview_edit.replace_range",
                    "range",
                )?,
                expected_hash: string_argument_alias(
                    edit_object,
                    &["expected_hash", "content_hash", "hash"],
                )
                .ok_or_else(|| {
                    "native tool `preview_edit` edit was missing `expected_hash`".to_string()
                })?,
                replacement: string_argument_alias(
                    edit_object,
                    &["replacement", "replace_with", "replace", "new", "content"],
                )
                .ok_or_else(|| {
                    "native tool `preview_edit` edit was missing `replacement`".to_string()
                })?,
            });
        }
        if edit_kind
            .as_deref()
            .is_some_and(|kind| matches!(kind, "modifytoml" | "modify_toml" | "toml"))
            || edit_object.get("operations").is_some() && edit_object.get("expected_hash").is_some()
        {
            let edit_value = serde_json::Value::Object(edit_object.clone());
            return Ok(PreviewEditPayload::ModifyToml {
                expected_hash: string_argument_alias(
                    edit_object,
                    &["expected_hash", "content_hash", "hash"],
                )
                .ok_or_else(|| {
                    "native tool `preview_edit` edit was missing `expected_hash`".to_string()
                })?,
                operations: toml_operations_argument(&edit_value, "preview_edit.modify_toml")?,
            });
        }
    }
    if let Some(patch) = optional_string_argument(arguments, "patch") {
        return Ok(PreviewEditPayload::ApplyPatch { patch });
    }
    if arguments.get("operations").is_some()
        && optional_string_argument(arguments, "expected_hash").is_some()
    {
        return Ok(PreviewEditPayload::ModifyToml {
            expected_hash: required_string_argument(arguments, "preview_edit", "expected_hash")?,
            operations: toml_operations_argument(arguments, "preview_edit")?,
        });
    }
    if optional_read_file_range_argument(arguments, "range").is_some()
        && optional_string_argument(arguments, "expected_hash").is_some()
        && optional_string_argument(arguments, "replacement").is_some()
    {
        return Ok(PreviewEditPayload::ReplaceRange {
            range: required_read_file_range_argument(arguments, "preview_edit", "range")?,
            expected_hash: required_string_argument(arguments, "preview_edit", "expected_hash")?,
            replacement: required_string_argument(arguments, "preview_edit", "replacement")?,
        });
    }
    let object = arguments
        .as_object()
        .ok_or_else(|| "native tool `preview_edit` arguments must be a JSON object".to_string())?;
    let search_block = string_argument_alias(object, &["search_block", "search", "old"])
        .ok_or_else(|| "native tool `preview_edit` was missing `edit`".to_string())?;
    let replace_block = string_argument_alias(
        object,
        &[
            "replace_block",
            "replace_with",
            "replacement",
            "replace",
            "new",
        ],
    )
    .ok_or_else(|| "native tool `preview_edit` was missing `replace_block`".to_string())?;
    Ok(PreviewEditPayload::ReplaceBlock {
        search_block,
        replace_block,
        range: optional_read_file_range_argument(arguments, "range")
            .or_else(|| optional_read_file_range_argument(arguments, "line_range")),
    })
}

fn string_argument_alias(
    object: &serde_json::Map<String, serde_json::Value>,
    fields: &[&str],
) -> Option<String> {
    fields
        .iter()
        .filter_map(|field| object.get(*field))
        .filter_map(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .next()
}

fn optional_string_list_argument(arguments: &serde_json::Value, field: &str) -> Vec<String> {
    arguments
        .get(field)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn optional_u64_argument(arguments: &serde_json::Value, field: &str) -> Option<u64> {
    arguments
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            arguments
                .get(field)
                .and_then(serde_json::Value::as_i64)
                .map(|value| value.max(0) as u64)
        })
}

fn optional_usize_argument(arguments: &serde_json::Value, field: &str) -> Option<usize> {
    optional_u64_argument(arguments, field).map(|value| value as usize)
}

fn optional_bool_argument(arguments: &serde_json::Value, field: &str) -> Option<bool> {
    arguments.get(field).and_then(serde_json::Value::as_bool)
}

fn parse_usage_payload(
    payload: &serde_json::Value,
    latency_ms: u64,
    finish_reason: Option<String>,
    provider_request_id: Option<&str>,
) -> Result<quorp_agent_core::TokenUsage, String> {
    let usage_u64 = |paths: &[&[&str]]| {
        paths.iter().find_map(|path| {
            let mut current = payload;
            for key in *path {
                current = current.get(*key)?;
            }
            current
                .as_u64()
                .or_else(|| current.as_i64().map(|value| value.max(0) as u64))
                .or_else(|| current.as_str().and_then(|value| value.parse::<u64>().ok()))
        })
    };
    let input_tokens = usage_u64(&[&["prompt_tokens"], &["input_tokens"]]).unwrap_or_default();
    let output_tokens =
        usage_u64(&[&["completion_tokens"], &["output_tokens"]]).unwrap_or_default();
    let total_billed_tokens =
        usage_u64(&[&["total_tokens"]]).unwrap_or(input_tokens.saturating_add(output_tokens));
    Ok(quorp_agent_core::TokenUsage {
        input_tokens,
        output_tokens,
        total_billed_tokens,
        reasoning_tokens: usage_u64(&[
            &["reasoning_tokens"],
            &["output_tokens_details", "reasoning_tokens"],
            &["completion_tokens_details", "reasoning_tokens"],
        ]),
        cache_read_input_tokens: usage_u64(&[
            &["cache_read_input_tokens"],
            &["input_tokens_details", "cached_tokens"],
            &["prompt_tokens_details", "cached_tokens"],
        ]),
        cache_write_input_tokens: usage_u64(&[
            &["cache_write_input_tokens"],
            &["input_tokens_details", "cache_write_tokens"],
            &["prompt_tokens_details", "cache_write_tokens"],
        ]),
        provider_request_id: provider_request_id.map(str::to_string).or_else(|| {
            payload
                .get("provider_request_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        }),
        latency_ms,
        finish_reason,
        usage_source: quorp_agent_core::UsageSource::Reported,
    })
}

pub fn build_system_prompt(request: &StreamRequest) -> String {
    let instruction_context = load_instruction_context(
        request.project_root.as_path(),
        request.latest_input.as_str(),
    );
    let mut prompt = String::from(if request.native_tool_calls {
        NATIVE_TOOL_SYSTEM_PROMPT_PREFIX
    } else {
        SYSTEM_PROMPT_PREFIX
    });
    prompt.push_str("\n\nCurrent mode: ");
    prompt.push_str(request.agent_mode.label());
    prompt.push_str(". ");
    match request.agent_mode {
        AgentMode::Ask => {
            prompt.push_str(
                "Only inspect with read-only actions. Do not propose writes or shell commands.",
            );
        }
        AgentMode::Plan => {
            prompt.push_str("You may inspect and request validation, but do not propose writes or arbitrary shell commands.");
        }
        AgentMode::Act => {
            prompt
                .push_str("You may inspect, edit, and validate. Prefer the smallest safe action.");
        }
    }
    if let Some(safety_mode_label) = request.safety_mode_label.as_deref() {
        prompt.push_str("\nSafety mode: ");
        prompt.push_str(safety_mode_label);
        prompt.push('.');
    }
    if nvidia_kimi_benchmark_profile(request) {
        if request.native_tool_calls {
            prompt.push_str(
                "\n\nKimi benchmark tool contract:\n\
- Use native function calls for Quorp tools; do not describe tools in prose.\n\
- In locked repair phases, make exactly one tool call.\n\
- Paths are workspace-relative.\n\
- For manifest repairs, follow PreviewEdit -> ApplyPreview -> validation.\n\
- Do not re-read files already named as loaded in the latest repair packet.",
            );
        } else {
            prompt.push_str(
                "\n\nKimi benchmark JSON action contract:\n\
- Return one raw JSON object only; no Markdown, no prose before or after it.\n\
- Put concrete Quorp actions in the `actions` array.\n\
- In locked repair phases, emit exactly one action.\n\
- Paths are workspace-relative.\n\
- For manifest repairs, follow PreviewEdit -> ApplyPreview -> validation.",
            );
        }
    } else if nvidia_qwen_benchmark_profile(request) {
        prompt.push_str(
            "\n\nQwen benchmark JSON contract:\n\
- Return one raw JSON object only; no Markdown, no prose before or after it.\n\
- Repair packets are authoritative; obey the required next action before doing anything else.\n\
- In locked repair phases, emit exactly one action.\n\
- Do not reread files the latest repair packet says are already loaded.\n\
- Prefer patch -> validation over explanation or broader inspection.",
        );
    }
    let rendered_instructions = render_instruction_context_for_prompt(&instruction_context);
    if !rendered_instructions.trim().is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(rendered_instructions.trim());
    }
    let rendered_mcp_servers =
        render_mcp_servers_for_prompt(&instruction_context.config.mcp_servers);
    if !rendered_mcp_servers.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(&rendered_mcp_servers);
    }
    let rendered_agent_tools = render_agent_tools_for_prompt(&instruction_context.config);
    if !rendered_agent_tools.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(&rendered_agent_tools);
    }

    if request.include_repo_capsule {
        let capsule = crate::quorp::tui::path_index::build_repo_capsule(
            request.project_root.as_path(),
            None,
            40,
        );
        let map_text = crate::quorp::tui::path_index::render_repo_capsule(None, &capsule);
        if !map_text.trim().is_empty() {
            prompt.push_str("\n\n--- Repository Map (Auto-Injected) ---\n");
            prompt.push_str(&map_text);
        }
    }

    prompt
}

fn render_agent_tools_for_prompt(config: &crate::quorp::tui::agent_context::AgentConfig) -> String {
    let tools = &config.agent_tools;
    if !tools.enabled {
        return String::new();
    }
    let mut lines = Vec::new();
    if tools.fd.enabled {
        lines.push(
            "- FindFiles: locate candidate files before broad directory reads; falls back to the repo ignore-walk when fd is unavailable."
                .to_string(),
        );
    }
    if tools.ast_grep.enabled && configured_ast_grep_command(tools).is_some() {
        lines.push(
            "- StructuralSearch: syntax-aware Rust search for functions, impls, calls, and patterns."
                .to_string(),
        );
        if tools.ast_grep.allow_rewrite_preview {
            lines.push(
                "- StructuralEditPreview: read-only ast-grep rewrite preview; apply real writes only with PreviewEdit/ApplyPreview."
                    .to_string(),
            );
        }
    }
    if tools.cargo_diagnostics.enabled
        && quorp_agent_core::command_is_available(&tools.cargo_diagnostics.check_command)
    {
        lines.push(
            "- CargoDiagnostics: compact cargo check JSON diagnostics; use after edits or when compiler errors matter."
                .to_string(),
        );
    }
    if tools.nextest.enabled
        && tools.nextest.prefer_for_workspace_tests
        && quorp_agent_core::command_is_available(&tools.nextest.command)
    {
        lines.push(
            "- RunValidation workspace_tests may use cargo-nextest for faster Rust test feedback."
                .to_string(),
        );
    }
    if lines.is_empty() {
        return String::new();
    }
    let mut rendered = String::from("Configured coding tools:\n");
    rendered.push_str(&lines.join("\n"));
    rendered
}

fn render_mcp_servers_for_prompt(
    servers: &[crate::quorp::tui::agent_context::McpServerConfig],
) -> String {
    if servers.is_empty() {
        return "Configured MCP servers: none. Do not emit `McpCallTool` unless a server is listed."
            .to_string();
    }

    let mut rendered = String::from(
        "Configured MCP servers:\nUse `McpCallTool` only with one of these stdio server names.\n",
    );
    for server in servers {
        rendered.push_str("- ");
        rendered.push_str(&server.name);
        rendered.push_str(" (configured stdio server)\n");
    }
    rendered.push_str(
        "When you use `McpCallTool`, set `server_name`, `tool_name`, and JSON `arguments` explicitly.",
    );
    rendered
}

pub fn compact_transcript_with_policy(
    messages: &[ChatServiceMessage],
    policy: Option<quorp_agent_core::PromptCompactionPolicy>,
) -> Vec<ChatServiceMessage> {
    let prompt_messages = messages
        .iter()
        .map(|message| PromptMessage {
            role: match message.role {
                ChatServiceRole::System => PromptMessageRole::System,
                ChatServiceRole::User => PromptMessageRole::User,
                ChatServiceRole::Assistant => PromptMessageRole::Assistant,
            },
            content: message.content.clone(),
        })
        .collect::<Vec<_>>();
    apply_prompt_compaction(&prompt_messages, policy)
        .into_iter()
        .map(|message| ChatServiceMessage {
            role: match message.role {
                PromptMessageRole::System => ChatServiceRole::System,
                PromptMessageRole::User => ChatServiceRole::User,
                PromptMessageRole::Assistant => ChatServiceRole::Assistant,
            },
            content: message.content,
        })
        .collect()
}

fn build_request_messages(request: &StreamRequest) -> Vec<SsdMoeChatMessage> {
    let mut request_messages = vec![SsdMoeChatMessage {
        role: "system",
        content: build_system_prompt(request),
    }];
    let compacted =
        compact_transcript_with_policy(&request.messages, request.prompt_compaction_policy);
    request_messages.extend(compacted.into_iter().filter_map(|message| {
        if message.content.trim().is_empty() {
            return None;
        }
        Some(SsdMoeChatMessage {
            role: match message.role {
                ChatServiceRole::System => "system",
                ChatServiceRole::User => "user",
                ChatServiceRole::Assistant => "assistant",
            },
            content: message.content,
        })
    }));
    request_messages
}

fn build_codex_prompt(request: &StreamRequest) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "You are Quorp running through the real Codex executor inside the current workspace.\n",
    );
    prompt.push_str("Use the existing conversation below as context, inspect the real files on disk when needed, and respond directly to the latest user message.\n");
    prompt.push_str(
        "If editing or validation is helpful, do it inside the workspace before replying.\n\n",
    );
    prompt.push_str("## Quorp Guidance\n");
    prompt.push_str(&build_system_prompt(request));
    prompt.push_str("\n\n## Conversation So Far\n");
    for message in
        compact_transcript_with_policy(&request.messages, request.prompt_compaction_policy)
    {
        let role = match message.role {
            ChatServiceRole::System => "System",
            ChatServiceRole::User => "User",
            ChatServiceRole::Assistant => "Assistant",
        };
        prompt.push_str(&format!("{role}:\n{}\n\n", message.content));
    }
    prompt.push_str("## Latest User Request\n");
    prompt.push_str(&request.latest_input);
    prompt.push('\n');
    prompt
}

fn reasoning_effort_for_model(model_id: &str) -> Option<String> {
    if model_id.eq_ignore_ascii_case("qwen3-coder-30b-a3b")
        || model_id.eq_ignore_ascii_case("ssd_moe/qwen3-coder-30b-a3b")
    {
        return None;
    }
    model_registry::local_moe_spec_for_registry_id(model_id)
        .filter(|model| model.has_think_tokens)
        .map(|_| "medium".to_string())
}

fn watchdog_near_limit(
    watchdog: &quorp_agent_core::CompletionWatchdogConfig,
    first_token_latency_ms: Option<u64>,
    max_idle_gap_ms: u64,
    total_elapsed_ms: u64,
) -> bool {
    let first_token_near_limit = watchdog
        .first_token_timeout_ms
        .zip(first_token_latency_ms)
        .is_some_and(|(limit_ms, observed_ms)| {
            observed_ms.saturating_mul(100) >= limit_ms.saturating_mul(80)
        });
    let idle_near_limit = watchdog
        .idle_timeout_ms
        .is_some_and(|limit_ms| max_idle_gap_ms.saturating_mul(100) >= limit_ms.saturating_mul(80));
    let total_near_limit = watchdog.total_timeout_ms.is_some_and(|limit_ms| {
        total_elapsed_ms.saturating_mul(100) >= limit_ms.saturating_mul(80)
    });
    first_token_near_limit || idle_near_limit || total_near_limit
}

async fn stream_response_to_ui(
    response: reqwest::Response,
    session_id: usize,
    request_id: u64,
    ssd_moe_runtime: &SsdMoeRuntimeHandle,
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
) -> Result<(), String> {
    let mut bytes_stream = response.bytes_stream();
    let mut coalescer = FragmentCoalescer::new();
    let mut line_buffer = String::new();
    let mut reasoning_header_sent = false;
    let mut stream_finished = false;

    loop {
        tokio::select! {
            _ = &mut coalescer.flush_timer, if coalescer.flush_armed => {
                coalescer.flush_buffered_text(event_tx, session_id);
                coalescer.flush_armed = false;
            }
            next_chunk = bytes_stream.next() => {
                let Some(next_chunk) = next_chunk else {
                    break;
                };
                let bytes = next_chunk.map_err(|error| {
                    if coalescer.sent_first_delta
                        && !matches!(
                            ssd_moe_runtime.status(),
                            crate::quorp::tui::ssd_moe_tui::ModelStatus::Running
                        )
                    {
                        format!(
                            "SSD-MOE stream failed after partial output while runtime was {}: {error}",
                            ssd_moe_runtime.status().label()
                        )
                    } else {
                        format!("SSD-MOE stream error: {error}")
                    }
                })?;
                let chunk_text = String::from_utf8_lossy(&bytes);
                line_buffer.push_str(&chunk_text);

                while let Some(newline_index) = line_buffer.find('\n') {
                    let line = line_buffer[..newline_index].to_string();
                    line_buffer.drain(..=newline_index);
                    let Some(payload) = parse_sse_data_line(&line) else {
                        continue;
                    };
                    let events = parse_sse_payload(payload)?;
                    for event in events {
                        match event {
                            SsdMoeStreamEvent::TextDelta(fragment) => {
                                coalescer.queue_fragment_with_context(
                                    event_tx,
                                    session_id,
                                    request_id,
                                    &fragment,
                                );
                            }
                            SsdMoeStreamEvent::ReasoningDelta(fragment) => {
                                let reasoning_fragment = if reasoning_header_sent {
                                    fragment
                                } else {
                                    reasoning_header_sent = true;
                                    format!("\n[Reasoning]\n{fragment}")
                                };
                                coalescer.queue_fragment_with_context(
                                    event_tx,
                                    session_id,
                                    request_id,
                                    &reasoning_fragment,
                                );
                            }
                            SsdMoeStreamEvent::Finished => {
                                stream_finished = true;
                            }
                        }
                    }
                }

                if stream_finished {
                    break;
                }
            }
        }
    }

    if let Some(payload) = parse_sse_data_line(&line_buffer) {
        for event in parse_sse_payload(payload)? {
            match event {
                SsdMoeStreamEvent::TextDelta(fragment) => coalescer
                    .queue_fragment_with_context(event_tx, session_id, request_id, &fragment),
                SsdMoeStreamEvent::ReasoningDelta(fragment) => {
                    let reasoning_fragment = if reasoning_header_sent {
                        fragment
                    } else {
                        format!("\n[Reasoning]\n{fragment}")
                    };
                    coalescer.queue_fragment_with_context(
                        event_tx,
                        session_id,
                        request_id,
                        &reasoning_fragment,
                    );
                }
                SsdMoeStreamEvent::Finished => {}
            }
        }
    }

    coalescer.flush_buffered_text(event_tx, session_id);
    if !stream_finished {
        crate::quorp::tui::diagnostics::log_event(
            "chat.stream_ended_early",
            json!({
                "request_id": request_id,
                "session_id": session_id,
                "runtime_status": ssd_moe_runtime.status().label(),
                "sent_first_delta": coalescer.sent_first_delta,
            }),
        );
        return Err("SSD-MOE stream ended before sending [DONE].".to_string());
    }
    crate::quorp::tui::diagnostics::log_event(
        "chat.stream_finished",
        json!({
            "request_id": request_id,
            "session_id": session_id,
            "runtime_status": ssd_moe_runtime.status().label(),
            "sent_first_delta": coalescer.sent_first_delta,
        }),
    );
    let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::StreamFinished(session_id)));
    Ok(())
}

struct FragmentCoalescer {
    buffered_text: String,
    sent_first_delta: bool,
    flush_timer: std::pin::Pin<Box<tokio::time::Sleep>>,
    flush_armed: bool,
}

impl FragmentCoalescer {
    fn new() -> Self {
        Self {
            buffered_text: String::new(),
            sent_first_delta: false,
            flush_timer: Box::pin(tokio::time::sleep(Duration::from_secs(3600))),
            flush_armed: false,
        }
    }

    fn queue_fragment_with_context(
        &mut self,
        event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
        session_id: usize,
        request_id: u64,
        fragment: &str,
    ) {
        if fragment.is_empty() {
            return;
        }
        if !self.sent_first_delta {
            crate::quorp::tui::diagnostics::log_event(
                "chat.first_token",
                json!({
                    "request_id": request_id,
                    "session_id": session_id,
                }),
            );
            let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::AssistantDelta(
                session_id,
                fragment.to_string(),
            )));
            self.sent_first_delta = true;
            return;
        }

        self.buffered_text.push_str(fragment);
        if should_flush_immediately(fragment) {
            self.flush_buffered_text(event_tx, session_id);
            self.flush_armed = false;
            return;
        }

        self.flush_timer
            .as_mut()
            .reset(tokio::time::Instant::now() + TOKEN_COALESCE_WINDOW);
        self.flush_armed = true;
    }

    fn flush_buffered_text(
        &mut self,
        event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
        session_id: usize,
    ) {
        if self.buffered_text.is_empty() {
            return;
        }
        let delta = std::mem::take(&mut self.buffered_text);
        let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::AssistantDelta(
            session_id, delta,
        )));
    }
}

fn should_flush_immediately(fragment: &str) -> bool {
    fragment.contains('\n')
        || fragment.ends_with('.')
        || fragment.ends_with('!')
        || fragment.ends_with('?')
        || fragment.ends_with(':')
}

fn parse_inline_command(input: &str) -> Option<String> {
    if let Some(command) = input.strip_prefix("/run ") {
        let trimmed = command.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Some(command) = input.strip_prefix('!') {
        let trimmed = command.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::thread;

    struct MockSseServer {
        address: std::net::SocketAddr,
        base_url: String,
        thread_handle: Option<thread::JoinHandle<std::io::Result<()>>>,
    }

    impl MockSseServer {
        fn new(response_body: &'static str) -> Self {
            Self::with_response_delay(response_body, Duration::ZERO)
        }

        fn with_response_delay(response_body: &'static str, response_delay: Duration) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
            let address = listener.local_addr().expect("local addr");
            let thread_handle = thread::spawn(move || -> std::io::Result<()> {
                let (mut socket, _) = listener.accept().expect("accept");
                let mut request = [0u8; 4096];
                let bytes_read = socket.read(&mut request).expect("read request");
                if bytes_read == 0 {
                    return Ok(());
                }
                if !response_delay.is_zero() {
                    thread::sleep(response_delay);
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                socket.write_all(response.as_bytes())
            });
            Self {
                address,
                base_url: format!("http://127.0.0.1:{}/v1", address.port()),
                thread_handle: Some(thread_handle),
            }
        }

        fn base_url(&self) -> String {
            self.base_url.clone()
        }
    }

    impl Drop for MockSseServer {
        fn drop(&mut self) {
            if let Ok(mut socket) = std::net::TcpStream::connect(self.address) {
                let _ = socket.write_all(b"GET /shutdown HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n");
            }
            if let Some(thread_handle) = self.thread_handle.take() {
                match thread_handle.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
                    Ok(Err(error)) => panic!("mock sse server failed: {error}"),
                    Err(panic_payload) => std::panic::resume_unwind(panic_payload),
                }
            }
        }
    }

    struct MockJsonServer {
        address: std::net::SocketAddr,
        base_url: String,
        thread_handle: Option<thread::JoinHandle<std::io::Result<()>>>,
    }

    impl MockJsonServer {
        fn new(response_body: &'static str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
            let address = listener.local_addr().expect("local addr");
            let thread_handle = thread::spawn(move || -> std::io::Result<()> {
                let (mut socket, _) = listener.accept().expect("accept");
                let mut request = [0u8; 16384];
                let bytes_read = socket.read(&mut request).expect("read request");
                if bytes_read == 0 {
                    return Ok(());
                }
                let request_text = String::from_utf8_lossy(&request[..bytes_read]);
                assert!(
                    request_text.contains("\"stream\":false"),
                    "expected native tool call request to disable streaming, got: {request_text}"
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                socket.write_all(response.as_bytes())
            });
            Self {
                address,
                base_url: format!("http://127.0.0.1:{}/v1", address.port()),
                thread_handle: Some(thread_handle),
            }
        }

        fn base_url(&self) -> String {
            self.base_url.clone()
        }
    }

    impl Drop for MockJsonServer {
        fn drop(&mut self) {
            if let Ok(mut socket) = std::net::TcpStream::connect(self.address) {
                let _ = socket.write_all(b"GET /shutdown HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n");
            }
            if let Some(thread_handle) = self.thread_handle.take() {
                match thread_handle.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
                    Ok(Err(error)) => panic!("mock json server failed: {error}"),
                    Err(panic_payload) => std::panic::resume_unwind(panic_payload),
                }
            }
        }
    }

    fn collect_submit_prompt_events(
        session_id: usize,
        base_url: String,
        timeout: Duration,
    ) -> Vec<TuiEvent> {
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(32);
        let runtime = tokio::runtime::Runtime::new().expect("chat service test runtime");
        let request = StreamRequest {
            request_id: 1,
            session_id,
            model_id: "qwen35-35b-a3b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "hello".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "hello".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: Some(base_url),
            max_completion_tokens: Some(4096),
            include_repo_capsule: true,
            disable_reasoning: false,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: None,
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };
        let result = runtime.block_on(run_stream_request(
            event_tx.clone(),
            SsdMoeRuntimeHandle::shared_handle(),
            request,
        ));
        if let Err(error) = result {
            let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::Error(session_id, error)));
            let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::StreamFinished(session_id)));
        }
        drop(event_tx);

        let deadline = std::time::Instant::now() + timeout;
        let mut events = Vec::new();
        while std::time::Instant::now() < deadline {
            match event_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => {
                    let is_finish = matches!(
                        event,
                        TuiEvent::Chat(ChatUiEvent::StreamFinished(finished_session_id))
                            if finished_session_id == session_id
                    );
                    events.push(event);
                    if is_finish {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        events
    }

    #[test]
    fn parse_inline_command_supports_run_prefix() {
        assert_eq!(
            parse_inline_command("/run cargo test"),
            Some("cargo test".to_string())
        );
    }

    #[test]
    fn parse_inline_command_supports_bang_prefix() {
        assert_eq!(parse_inline_command("!ls -la"), Some("ls -la".to_string()));
    }

    #[test]
    fn submit_prompt_streams_loopback_sse() {
        let server = MockSseServer::new(concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n",
            "data: [DONE]\n\n",
        ));
        let events = collect_submit_prompt_events(7, server.base_url(), Duration::from_secs(20));
        let saw_delta = events.iter().any(|event| {
            matches!(
                event,
                TuiEvent::Chat(ChatUiEvent::AssistantDelta(7, text)) if text.contains("hello")
            )
        });
        let saw_finish = events
            .iter()
            .any(|event| matches!(event, TuiEvent::Chat(ChatUiEvent::StreamFinished(7))));
        if let Some(TuiEvent::Chat(ChatUiEvent::Error(7, error))) = events
            .iter()
            .find(|event| matches!(event, TuiEvent::Chat(ChatUiEvent::Error(7, _))))
        {
            panic!("unexpected stream error: {error}");
        }

        assert!(saw_delta, "expected assistant delta from local SSE server");
        assert!(
            saw_finish,
            "expected stream finished event from local SSE server"
        );
    }

    #[test]
    fn submit_prompt_streams_local_sse_with_base_url_override() {
        let server = MockSseServer::new(concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello-from-ollama\"},\"finish_reason\":null}]}\n\n",
            "data: [DONE]\n\n",
        ));
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(32);
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let session_id = 11usize;
        let request = StreamRequest {
            request_id: 2,
            session_id,
            model_id: "ssd_moe/qwen3-coder-30b-a3b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "hello".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "hello".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: Some(server.base_url()),
            max_completion_tokens: Some(4096),
            include_repo_capsule: true,
            disable_reasoning: false,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: None,
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };
        let result = runtime.block_on(run_stream_request(
            event_tx.clone(),
            SsdMoeRuntimeHandle::shared_handle(),
            request,
        ));
        if let Err(error) = result {
            let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::Error(session_id, error)));
            let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::StreamFinished(session_id)));
        }
        drop(event_tx);

        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut saw_delta = false;
        let mut saw_finish = false;
        while std::time::Instant::now() < deadline {
            match event_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(TuiEvent::Chat(ChatUiEvent::AssistantDelta(11, text))) => {
                    if text.contains("hello-from-ollama") {
                        saw_delta = true;
                    }
                }
                Ok(TuiEvent::Chat(ChatUiEvent::StreamFinished(11))) => {
                    saw_finish = true;
                    break;
                }
                Ok(TuiEvent::Chat(ChatUiEvent::Error(11, error))) => {
                    panic!("unexpected local stream error: {error}");
                }
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        assert!(saw_delta, "expected assistant delta from local SSE server");
        assert!(saw_finish, "expected local stream finished event");
    }

    #[test]
    fn cancel_request_aborts_stream_before_tokens_arrive() {
        let server = MockSseServer::with_response_delay(
            concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"late-token\"},\"finish_reason\":null}]}\n\n",
                "data: [DONE]\n\n",
            ),
            Duration::from_millis(400),
        );

        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(32);
        let request_tx = spawn_chat_service_loop(event_tx);
        request_tx
            .unbounded_send(ChatServiceRequest::SubmitPrompt {
                session_id: 3,
                model_id: "qwen35-35b-a3b".to_string(),
                agent_mode: AgentMode::Act,
                latest_input: "hello".to_string(),
                messages: vec![ChatServiceMessage {
                    role: ChatServiceRole::User,
                    content: "hello".to_string(),
                }],
                project_root: PathBuf::from("/tmp"),
                base_url_override: Some(server.base_url()),
                prompt_compaction_policy: None,
            })
            .expect("send request");
        request_tx
            .unbounded_send(ChatServiceRequest::Cancel { session_id: 3 })
            .expect("cancel request");

        let deadline = std::time::Instant::now() + Duration::from_millis(700);
        while std::time::Instant::now() < deadline {
            match event_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(TuiEvent::Chat(ChatUiEvent::AssistantDelta(3, text))) => {
                    panic!("unexpected streamed token after cancel: {text}");
                }
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => panic!("event channel error after cancel: {error}"),
            }
        }
    }

    #[test]
    fn resolve_client_config_supports_ollama_for_native_runs() {
        let _env_lock = crate::quorp::tui::ssd_moe_tui::test_env_lock();
        let original_provider = std::env::var("QUORP_PROVIDER").ok();
        let original_host = std::env::var("QUORP_OLLAMA_HOST").ok();
        unsafe {
            std::env::set_var("QUORP_PROVIDER", "ollama");
            std::env::set_var("QUORP_OLLAMA_HOST", "http://127.0.0.1:22442");
        }

        let request = StreamRequest {
            request_id: 1,
            session_id: 0,
            model_id: "ollama/qwen2.5-coder:32b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: None,
            max_completion_tokens: Some(512),
            include_repo_capsule: false,
            disable_reasoning: true,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: None,
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let resolved =
            resolve_client_config(&SsdMoeRuntimeHandle::shared_handle(), &request).expect("config");
        assert_eq!(resolved.provider, InteractiveProviderKind::Ollama);
        assert_eq!(resolved.client.base_url, "http://127.0.0.1:22442/v1");
        assert_eq!(resolved.client.model_id, "qwen2.5-coder:32b");
        assert_eq!(resolved.bearer_token, None);
        assert_eq!(resolved.routing.effective_provider, "ollama");
        assert_eq!(resolved.routing.effective_model, "qwen2.5-coder:32b");

        if let Some(value) = original_provider {
            unsafe {
                std::env::set_var("QUORP_PROVIDER", value);
            }
        } else {
            unsafe {
                std::env::remove_var("QUORP_PROVIDER");
            }
        }
        if let Some(value) = original_host {
            unsafe {
                std::env::set_var("QUORP_OLLAMA_HOST", value);
            }
        } else {
            unsafe {
                std::env::remove_var("QUORP_OLLAMA_HOST");
            }
        }
    }

    #[test]
    fn resolve_client_config_supports_openai_compatible_for_native_runs() {
        let _env_lock = crate::quorp::tui::ssd_moe_tui::test_env_lock();
        let temp_home = tempfile::tempdir().expect("temp home");
        let original_home = std::env::var("HOME").ok();
        let original_provider = std::env::var("QUORP_PROVIDER").ok();
        let original_api_key = std::env::var("QUORP_API_KEY").ok();
        std::fs::create_dir_all(temp_home.path().join(".quorp")).expect("create .quorp");
        std::fs::write(
            temp_home.path().join(".quorp/.env"),
            "QUORP_PROVIDER=openai-compatible\nQUORP_BASE_URL=https://models.example.test/v1\nQUORP_API_KEY=home-env-key\n",
        )
        .expect("write home env");
        unsafe {
            std::env::set_var("HOME", temp_home.path());
            std::env::remove_var("QUORP_PROVIDER");
            std::env::remove_var("QUORP_API_KEY");
        }

        let request = StreamRequest {
            request_id: 2,
            session_id: 0,
            model_id: "openai-compatible/gpt-4.1-mini".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: None,
            max_completion_tokens: Some(512),
            include_repo_capsule: false,
            disable_reasoning: true,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: None,
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let resolved =
            resolve_client_config(&SsdMoeRuntimeHandle::shared_handle(), &request).expect("config");
        assert_eq!(resolved.provider, InteractiveProviderKind::OpenAiCompatible);
        assert_eq!(resolved.client.base_url, "https://models.example.test/v1");
        assert_eq!(resolved.client.model_id, "gpt-4.1-mini");
        assert_eq!(resolved.bearer_token.as_deref(), Some("home-env-key"));
        assert_eq!(resolved.routing.effective_provider, "openai-compatible");
        assert_eq!(resolved.routing.effective_model, "gpt-4.1-mini");

        if let Some(value) = original_provider {
            unsafe {
                std::env::set_var("QUORP_PROVIDER", value);
            }
        } else {
            unsafe {
                std::env::remove_var("QUORP_PROVIDER");
            }
        }
        if let Some(value) = original_api_key {
            unsafe {
                std::env::set_var("QUORP_API_KEY", value);
            }
        } else {
            unsafe {
                std::env::remove_var("QUORP_API_KEY");
            }
        }
        if let Some(value) = original_home {
            unsafe {
                std::env::set_var("HOME", value);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }
    }

    #[test]
    fn resolve_client_config_supports_nvidia_for_native_runs() {
        let _env_lock = crate::quorp::tui::ssd_moe_tui::test_env_lock();
        let temp_home = tempfile::tempdir().expect("temp home");
        let original_home = std::env::var("HOME").ok();
        let original_provider = std::env::var("QUORP_PROVIDER").ok();
        let original_nvidia_api_key = std::env::var("NVIDIA_API_KEY").ok();
        let original_quorp_nvidia_api_key = std::env::var("QUORP_NVIDIA_API_KEY").ok();
        let original_quorp_api_key = std::env::var("QUORP_API_KEY").ok();
        let original_thinking = std::env::var("QUORP_NVIDIA_KIMI_THINKING").ok();
        std::fs::create_dir_all(temp_home.path().join(".quorp")).expect("create .quorp");
        std::fs::write(
            temp_home.path().join(".quorp/.env"),
            "NVIDIA_API_KEY=home-nvidia-key\n",
        )
        .expect("write home env");
        unsafe {
            std::env::set_var("HOME", temp_home.path());
            std::env::remove_var("QUORP_PROVIDER");
            std::env::remove_var("NVIDIA_API_KEY");
            std::env::remove_var("QUORP_NVIDIA_API_KEY");
            std::env::remove_var("QUORP_API_KEY");
            std::env::remove_var("QUORP_NVIDIA_KIMI_THINKING");
        }

        let request = StreamRequest {
            request_id: 3,
            session_id: 0,
            model_id: "nvidia/moonshotai/kimi-k2.5".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: None,
            max_completion_tokens: Some(512),
            include_repo_capsule: false,
            disable_reasoning: true,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: None,
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let resolved =
            resolve_client_config(&SsdMoeRuntimeHandle::shared_handle(), &request).expect("config");
        let request_body = build_completion_request_body(&request, &resolved.client, true);
        assert_eq!(resolved.provider, InteractiveProviderKind::Nvidia);
        assert_eq!(
            resolved.client.base_url,
            crate::quorp::provider_config::NVIDIA_NIM_BASE_URL
        );
        assert_eq!(resolved.client.model_id, "moonshotai/kimi-k2.5");
        assert_eq!(resolved.bearer_token.as_deref(), Some("home-nvidia-key"));
        assert_eq!(resolved.routing.effective_provider, "nvidia");
        assert_eq!(resolved.routing.effective_model, "moonshotai/kimi-k2.5");
        assert_eq!(request_body["temperature"], serde_json::json!(1.0));
        assert_eq!(request_body["top_p"], serde_json::json!(0.95));
        assert!(request_body.get("chat_template_kwargs").is_none());

        if let Some(value) = original_thinking {
            unsafe {
                std::env::set_var("QUORP_NVIDIA_KIMI_THINKING", value);
            }
        } else {
            unsafe {
                std::env::remove_var("QUORP_NVIDIA_KIMI_THINKING");
            }
        }
        if let Some(value) = original_quorp_api_key {
            unsafe {
                std::env::set_var("QUORP_API_KEY", value);
            }
        } else {
            unsafe {
                std::env::remove_var("QUORP_API_KEY");
            }
        }
        if let Some(value) = original_quorp_nvidia_api_key {
            unsafe {
                std::env::set_var("QUORP_NVIDIA_API_KEY", value);
            }
        } else {
            unsafe {
                std::env::remove_var("QUORP_NVIDIA_API_KEY");
            }
        }
        if let Some(value) = original_nvidia_api_key {
            unsafe {
                std::env::set_var("NVIDIA_API_KEY", value);
            }
        } else {
            unsafe {
                std::env::remove_var("NVIDIA_API_KEY");
            }
        }
        if let Some(value) = original_provider {
            unsafe {
                std::env::set_var("QUORP_PROVIDER", value);
            }
        } else {
            unsafe {
                std::env::remove_var("QUORP_PROVIDER");
            }
        }
        if let Some(value) = original_home {
            unsafe {
                std::env::set_var("HOME", value);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }
    }

    #[test]
    fn nvidia_kimi_body_overrides_disable_thinking_when_requested() {
        let _env_lock = crate::quorp::tui::ssd_moe_tui::test_env_lock();
        let original_thinking = std::env::var("QUORP_NVIDIA_KIMI_THINKING").ok();
        unsafe {
            std::env::set_var("QUORP_NVIDIA_KIMI_THINKING", "0");
        }

        let body = nvidia_request_body_overrides("moonshotai/kimi-k2.5");

        assert_eq!(body["temperature"], serde_json::json!(1.0));
        assert_eq!(body["top_p"], serde_json::json!(0.95));
        assert_eq!(
            body["chat_template_kwargs"],
            serde_json::json!({ "thinking": false })
        );

        if let Some(value) = original_thinking {
            unsafe {
                std::env::set_var("QUORP_NVIDIA_KIMI_THINKING", value);
            }
        } else {
            unsafe {
                std::env::remove_var("QUORP_NVIDIA_KIMI_THINKING");
            }
        }
    }

    #[test]
    fn resolve_local_client_config_uses_home_local_base_url_override() {
        let _env_lock = crate::quorp::tui::ssd_moe_tui::test_env_lock();
        let temp_home = tempfile::tempdir().expect("temp home");
        let original_home = std::env::var("HOME").ok();
        let original_provider = std::env::var("QUORP_PROVIDER").ok();
        std::fs::create_dir_all(temp_home.path().join(".quorp")).expect("create .quorp");
        std::fs::write(
            temp_home.path().join(".quorp/.env"),
            "QUORP_PROVIDER=local\nQUORP_LOCAL_BASE_URL=http://127.0.0.1:53878/v1\n",
        )
        .expect("write home env");
        unsafe {
            std::env::set_var("HOME", temp_home.path());
            std::env::remove_var("QUORP_PROVIDER");
        }

        let request = StreamRequest {
            request_id: 9,
            session_id: 0,
            model_id: "ssd_moe/qwen3-coder-30b-a3b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: None,
            max_completion_tokens: Some(512),
            include_repo_capsule: false,
            disable_reasoning: true,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: None,
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        assert_eq!(
            resolved_local_base_url_override(&request).as_deref(),
            Some("http://127.0.0.1:53878/v1")
        );

        let resolved =
            resolve_client_config(&SsdMoeRuntimeHandle::shared_handle(), &request).expect("config");
        assert_eq!(resolved.provider, InteractiveProviderKind::Local);
        assert_eq!(resolved.client.base_url, "http://127.0.0.1:53878/v1");
        assert_eq!(resolved.client.model_id, "qwen3-coder-30b-a3b");

        if let Some(value) = original_provider {
            unsafe {
                std::env::set_var("QUORP_PROVIDER", value);
            }
        } else {
            unsafe {
                std::env::remove_var("QUORP_PROVIDER");
            }
        }
        if let Some(value) = original_home {
            unsafe {
                std::env::set_var("HOME", value);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }
    }

    #[test]
    fn heavy_local_requests_ignore_home_local_base_url_override() {
        let _env_lock = crate::quorp::tui::ssd_moe_tui::test_env_lock();
        let temp_home = tempfile::tempdir().expect("temp home");
        let original_home = std::env::var("HOME").ok();
        let original_provider = std::env::var("QUORP_PROVIDER").ok();
        std::fs::create_dir_all(temp_home.path().join(".quorp")).expect("create .quorp");
        std::fs::write(
            temp_home.path().join(".quorp/.env"),
            "QUORP_PROVIDER=local\nQUORP_LOCAL_BASE_URL=https://warpos-capture-probe:8443/quorp/v1\n",
        )
        .expect("write home env");
        unsafe {
            std::env::set_var("HOME", temp_home.path());
            std::env::remove_var("QUORP_PROVIDER");
        }

        let request = StreamRequest {
            request_id: 10,
            session_id: 0,
            model_id: "ssd_moe/qwen3-coder-30b-a3b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: None,
            max_completion_tokens: Some(512),
            include_repo_capsule: false,
            disable_reasoning: true,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: Some("heavy_local".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        assert_eq!(resolved_local_base_url_override(&request), None);

        if let Some(value) = original_provider {
            unsafe {
                std::env::set_var("QUORP_PROVIDER", value);
            }
        } else {
            unsafe {
                std::env::remove_var("QUORP_PROVIDER");
            }
        }
        if let Some(value) = original_home {
            unsafe {
                std::env::set_var("HOME", value);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }
    }

    #[test]
    fn parse_usage_payload_reads_nested_usage_fields() {
        let usage = parse_usage_payload(
            &serde_json::json!({
                "prompt_tokens": 120,
                "completion_tokens": 14,
                "total_tokens": 134,
                "prompt_tokens_details": {
                    "cached_tokens": 16,
                    "cache_write_tokens": 4
                },
                "completion_tokens_details": {
                    "reasoning_tokens": 6
                }
            }),
            33,
            Some("stop".to_string()),
            Some("req_123"),
        )
        .expect("usage");
        assert_eq!(usage.input_tokens, 120);
        assert_eq!(usage.output_tokens, 14);
        assert_eq!(usage.total_billed_tokens, 134);
        assert_eq!(usage.cache_read_input_tokens, Some(16));
        assert_eq!(usage.cache_write_input_tokens, Some(4));
        assert_eq!(usage.reasoning_tokens, Some(6));
        assert_eq!(usage.provider_request_id.as_deref(), Some("req_123"));
    }

    #[test]
    fn parse_local_runtime_usage_reads_prompt_and_output_tokens() {
        let usage = parse_local_runtime_usage(&serde_json::json!({
            "last_request": {
                "prompt_tokens": 5602,
                "emitted_tokens": 160,
                "cached_tokens": 44,
                "cache_write_tokens": 12,
                "reasoning_tokens": 31,
                "request_id": "runtime_req_7",
                "finish_reason": "stop",
                "started_at_ms": 1000,
                "finished_at_ms": 7123
            }
        }))
        .expect("local runtime usage");
        assert_eq!(usage.input_tokens, 5602);
        assert_eq!(usage.output_tokens, 160);
        assert_eq!(usage.total_billed_tokens, 5762);
        assert_eq!(usage.cache_read_input_tokens, Some(44));
        assert_eq!(usage.cache_write_input_tokens, Some(12));
        assert_eq!(usage.reasoning_tokens, Some(31));
        assert_eq!(usage.provider_request_id.as_deref(), Some("runtime_req_7"));
        assert_eq!(usage.finish_reason.as_deref(), Some("stop"));
        assert_eq!(usage.latency_ms, 6123);
        assert_eq!(usage.usage_source, quorp_agent_core::UsageSource::Reported);
    }

    #[test]
    fn single_completion_uses_usage_chunk_when_stream_emits_it() {
        let server = MockSseServer::new(concat!(
            "data: {\"id\":\"chatcmpl_usage\",\"model\":\"qwen3-coder-30b-a3b\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_usage\",\"model\":\"qwen3-coder-30b-a3b\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":21,\"completion_tokens\":4,\"total_tokens\":25,\"prompt_tokens_details\":{\"cached_tokens\":3},\"completion_tokens_details\":{\"reasoning_tokens\":1}}}\n\n",
            "data: [DONE]\n\n",
        ));
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let request = StreamRequest {
            request_id: 11,
            session_id: 0,
            model_id: "ssd_moe/qwen3-coder-30b-a3b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "hello".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "hello".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: Some(server.base_url()),
            max_completion_tokens: Some(512),
            include_repo_capsule: false,
            disable_reasoning: true,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: Some("safe_local".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let result = runtime
            .block_on(request_single_completion_details(
                &SsdMoeRuntimeHandle::shared_handle(),
                &request,
            ))
            .expect("single completion");

        let usage = result.usage.expect("usage");
        assert_eq!(result.content, "hello");
        assert_eq!(usage.input_tokens, 21);
        assert_eq!(usage.output_tokens, 4);
        assert_eq!(usage.total_billed_tokens, 25);
        assert_eq!(usage.cache_read_input_tokens, Some(3));
        assert_eq!(usage.reasoning_tokens, Some(1));
        assert_eq!(usage.provider_request_id.as_deref(), Some("chatcmpl_usage"));
    }

    #[test]
    fn build_completion_request_body_includes_native_tool_definitions() {
        let request = StreamRequest {
            request_id: 17,
            session_id: 0,
            model_id: "ssd_moe/qwen3-coder-30b-a3b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: Some("https://warpos-capture-probe:8443/quorp/v1".to_string()),
            max_completion_tokens: Some(640),
            include_repo_capsule: true,
            disable_reasoning: false,
            native_tool_calls: true,
            watchdog: None,
            safety_mode_label: Some("heavy_local".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };
        let config = SsdMoeClientConfig {
            base_url: "https://warpos-capture-probe:8443/quorp/v1".to_string(),
            model_id: "qwen3-coder-30b-a3b".to_string(),
            extra_headers: BTreeMap::new(),
            extra_body: serde_json::Map::new(),
            connect_timeout: CONNECT_TIMEOUT,
            read_timeout: READ_TIMEOUT,
        };

        let body = build_completion_request_body(&request, &config, true);
        let tools = body
            .get("tools")
            .and_then(serde_json::Value::as_array)
            .expect("native tool definitions");
        assert!(!tools.is_empty(), "expected native tools to be included");
        assert_eq!(
            body.get("tool_choice").and_then(serde_json::Value::as_str),
            Some("required")
        );
        assert_eq!(
            body.get("parallel_tool_calls")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn native_modify_toml_tool_schema_defines_dependency_operations() {
        let tools = native_tool_definitions();
        let modify_toml = tools
            .iter()
            .find(|tool| {
                tool.get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(serde_json::Value::as_str)
                    == Some("modify_toml")
            })
            .expect("modify_toml tool");
        let operation_schema =
            &modify_toml["function"]["parameters"]["properties"]["operations"]["items"]["oneOf"];

        assert!(operation_schema.is_array(), "schema: {operation_schema}");
        assert_eq!(
            operation_schema[0]["required"],
            serde_json::json!(["op", "table", "name"])
        );
        assert_eq!(
            operation_schema[0]["properties"]["features"]["items"]["type"],
            serde_json::json!("string")
        );

        let preview_edit = tools
            .iter()
            .find(|tool| {
                tool.get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(serde_json::Value::as_str)
                    == Some("preview_edit")
            })
            .expect("preview_edit tool");
        assert_eq!(
            preview_edit["function"]["parameters"]["properties"]["edit"]["properties"]["modify_toml"]
                ["properties"]["operations"]["items"]["oneOf"],
            *operation_schema
        );
    }

    #[test]
    fn nvidia_kimi_benchmark_manifest_phase_scopes_native_tool_choice() {
        let request = StreamRequest {
            request_id: 18,
            session_id: 0,
            model_id: "nvidia/moonshotai/kimi-k2.5".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "repair".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "Allowed next action: exactly one `PreviewEdit` with `modify_toml` on the leased manifest target.".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: None,
            max_completion_tokens: Some(640),
            include_repo_capsule: false,
            disable_reasoning: true,
            native_tool_calls: true,
            watchdog: None,
            safety_mode_label: Some("nvidia_kimi_benchmark".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };
        let config = SsdMoeClientConfig {
            base_url: "https://integrate.api.nvidia.com/v1".to_string(),
            model_id: "moonshotai/kimi-k2.5".to_string(),
            extra_headers: BTreeMap::new(),
            extra_body: serde_json::Map::new(),
            connect_timeout: CONNECT_TIMEOUT,
            read_timeout: READ_TIMEOUT,
        };

        let body = build_completion_request_body(&request, &config, true);
        let tools = body
            .get("tools")
            .and_then(serde_json::Value::as_array)
            .expect("tools");

        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0]["function"]["name"],
            serde_json::json!("preview_edit")
        );
        assert_eq!(
            body["tool_choice"],
            serde_json::json!({
                "type": "function",
                "function": { "name": "preview_edit" }
            })
        );
        assert_eq!(
            body["chat_template_kwargs"],
            serde_json::json!({ "thinking": false })
        );
    }

    #[test]
    fn nvidia_kimi_benchmark_prompt_includes_tool_contract_card() {
        let request = StreamRequest {
            request_id: 19,
            session_id: 0,
            model_id: "nvidia/moonshotai/kimi-k2.5".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "repair".to_string(),
            messages: Vec::new(),
            project_root: PathBuf::from("/tmp"),
            base_url_override: None,
            max_completion_tokens: Some(640),
            include_repo_capsule: false,
            disable_reasoning: true,
            native_tool_calls: true,
            watchdog: None,
            safety_mode_label: Some("nvidia_kimi_benchmark".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let prompt = build_system_prompt(&request);

        assert!(prompt.contains("Kimi benchmark tool contract"));
        assert!(prompt.contains("PreviewEdit -> ApplyPreview -> validation"));
    }

    #[test]
    fn nvidia_qwen_benchmark_prompt_includes_json_contract_without_kimi_extras() {
        let request = StreamRequest {
            request_id: 20,
            session_id: 0,
            model_id: "nvidia/qwen/qwen3-coder-480b-a35b-instruct".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "repair".to_string(),
            messages: Vec::new(),
            project_root: PathBuf::from("/tmp"),
            base_url_override: None,
            max_completion_tokens: Some(1024),
            include_repo_capsule: false,
            disable_reasoning: true,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: Some("nvidia_qwen_benchmark".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };
        let config = SsdMoeClientConfig {
            base_url: "https://integrate.api.nvidia.com/v1".to_string(),
            model_id: "qwen/qwen3-coder-480b-a35b-instruct".to_string(),
            extra_headers: BTreeMap::new(),
            extra_body: serde_json::Map::new(),
            connect_timeout: CONNECT_TIMEOUT,
            read_timeout: READ_TIMEOUT,
        };

        let prompt = build_system_prompt(&request);
        let body = build_completion_request_body(&request, &config, true);

        assert!(prompt.contains("Qwen benchmark JSON contract"));
        assert!(prompt.contains("Repair packets are authoritative"));
        assert!(body.get("chat_template_kwargs").is_none());
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn native_modify_toml_accepts_flat_dependency_operation() {
        let action = parse_action_from_arguments(
            "modify_toml",
            &serde_json::json!({
                "path": "Cargo.toml",
                "expected_hash": "0123456789abcdef",
                "operations": [
                    {
                        "table": "dependencies",
                        "name": "chrono",
                        "version": "0.4",
                        "default-features": false
                    }
                ]
            }),
        )
        .expect("parse native modify_toml");

        assert!(matches!(
            action,
            AgentAction::ModifyToml {
                ref path,
                ref expected_hash,
                ref operations,
            } if path == "Cargo.toml"
                && expected_hash == "0123456789abcdef"
                && operations.len() == 1
        ));
    }

    #[test]
    fn native_modify_toml_defaults_missing_dependency_table() {
        let action = parse_action_from_arguments(
            "modify_toml",
            &serde_json::json!({
                "path": "Cargo.toml",
                "expected_hash": "0123456789abcdef",
                "operations": [
                    {
                        "set_dependency": {
                            "name": "chrono",
                            "version": "0.4"
                        }
                    }
                ]
            }),
        )
        .expect("parse native modify_toml");

        match action {
            AgentAction::ModifyToml { operations, .. } => {
                assert!(matches!(
                    &operations[0],
                    quorp_agent_core::agent_protocol::TomlEditOperation::SetDependency {
                        table,
                        name,
                        ..
                    } if table == "dependencies" && name == "chrono"
                ));
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn native_modify_toml_accepts_dependency_name_aliases() {
        let action = parse_action_from_arguments(
            "modify_toml",
            &serde_json::json!({
                "path": "Cargo.toml",
                "expected_hash": "0123456789abcdef",
                "operations": [
                    {
                        "set_dependency": {
                            "dependency_name": "chrono",
                            "version": "0.4"
                        }
                    },
                    {
                        "type": "set_dependency",
                        "dependency": "uuid",
                        "version": "1"
                    },
                    {
                        "set_dependency": {
                            "rand": "0.8"
                        }
                    }
                ]
            }),
        )
        .expect("parse native modify_toml aliases");

        match action {
            AgentAction::ModifyToml { operations, .. } => {
                assert!(matches!(
                    &operations[0],
                    quorp_agent_core::agent_protocol::TomlEditOperation::SetDependency {
                        table,
                        name,
                        ..
                    } if table == "dependencies" && name == "chrono"
                ));
                assert!(matches!(
                    &operations[1],
                    quorp_agent_core::agent_protocol::TomlEditOperation::SetDependency {
                        table,
                        name,
                        ..
                    } if table == "dependencies" && name == "uuid"
                ));
                assert!(matches!(
                    &operations[2],
                    quorp_agent_core::agent_protocol::TomlEditOperation::SetDependency {
                        table,
                        name,
                        version,
                        ..
                    } if table == "dependencies" && name == "rand" && version.as_deref() == Some("0.8")
                ));
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn native_modify_toml_missing_hash_becomes_read_prerequisite() {
        let action = parse_action_from_arguments(
            "modify_toml",
            &serde_json::json!({
                "path": "Cargo.toml",
                "operations": [
                    {
                        "table": "dependencies",
                        "name": "chrono",
                        "version": "0.4"
                    }
                ]
            }),
        )
        .expect("parse native modify_toml prerequisite");

        assert!(matches!(
            action,
            AgentAction::ReadFile { ref path, range: None } if path == "Cargo.toml"
        ));
    }

    #[test]
    fn native_preview_modify_toml_missing_hash_becomes_read_prerequisite() {
        let action = parse_action_from_arguments(
            "preview_edit",
            &serde_json::json!({
                "path": "Cargo.toml",
                "edit": {
                    "modify_toml": {
                        "operations": [
                            {
                                "set_dependency": {
                                    "name": "chrono",
                                    "version": "0.4"
                                }
                            }
                        ]
                    }
                }
            }),
        )
        .expect("parse native preview prerequisite");

        assert!(matches!(
            action,
            AgentAction::ReadFile { ref path, range: None } if path == "Cargo.toml"
        ));
    }

    #[test]
    fn single_completion_parses_native_tool_calls_into_agent_turn() {
        let server = MockSseServer::new(concat!(
            "data: {\"id\":\"chatcmpl_tools\",\"model\":\"qwen3-coder-30b-a3b\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"I will inspect the target files first.\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_tools\",\"model\":\"qwen3-coder-30b-a3b\",\"choices\":[{\"index\":0,\"message\":{\"tool_calls\":[{\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"search_text\",\"arguments\":\"{\\\"query\\\":\\\"entitlement grace\\\",\\\"limit\\\":3}\"}}]},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":30,\"completion_tokens\":9,\"total_tokens\":39}}\n\n",
            "data: [DONE]\n\n",
        ));
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let request = StreamRequest {
            request_id: 19,
            session_id: 0,
            model_id: "ollama/qwen3-coder:30b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: Some(server.base_url()),
            max_completion_tokens: Some(768),
            include_repo_capsule: true,
            disable_reasoning: false,
            native_tool_calls: true,
            watchdog: None,
            safety_mode_label: Some("heavy_local".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let result = runtime
            .block_on(request_single_completion_details(
                &SsdMoeRuntimeHandle::shared_handle(),
                &request,
            ))
            .expect("single completion");

        let native_turn = result.native_turn.expect("native turn");
        assert!(result.native_turn_error.is_none(), "unexpected parse error");
        assert_eq!(
            native_turn.assistant_message,
            "I will inspect the target files first."
        );
        assert_eq!(native_turn.actions.len(), 1);
        assert_eq!(
            native_turn.actions[0],
            AgentAction::SearchText {
                query: "entitlement grace".to_string(),
                limit: 3,
            }
        );
        let usage = result.usage.expect("usage");
        assert_eq!(usage.total_billed_tokens, 39);
        assert_eq!(usage.provider_request_id.as_deref(), Some("chatcmpl_tools"));
    }

    #[test]
    fn single_completion_parses_non_stream_native_tool_calls_for_local_provider() {
        let server = MockJsonServer::new(
            "{\"id\":\"chatcmpl_tools_json\",\"model\":\"qwen3-coder-30b-a3b\",\"choices\":[{\"index\":0,\"message\":{\"content\":\"I will inspect the target files first.\",\"tool_calls\":[{\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"list_directory\",\"arguments\":\"{\\\"path\\\":\\\".\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":30,\"completion_tokens\":9,\"total_tokens\":39}}",
        );
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let request = StreamRequest {
            request_id: 119,
            session_id: 0,
            model_id: "ssd_moe/qwen3-coder-30b-a3b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: Some(server.base_url()),
            max_completion_tokens: Some(768),
            include_repo_capsule: true,
            disable_reasoning: false,
            native_tool_calls: true,
            watchdog: None,
            safety_mode_label: Some("heavy_local".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let result = runtime
            .block_on(request_single_completion_details(
                &SsdMoeRuntimeHandle::shared_handle(),
                &request,
            ))
            .expect("single completion");

        let native_turn = result.native_turn.expect("native turn");
        assert!(result.native_turn_error.is_none(), "unexpected parse error");
        assert_eq!(
            native_turn.assistant_message,
            "I will inspect the target files first."
        );
        assert_eq!(
            native_turn.actions,
            vec![AgentAction::ListDirectory {
                path: ".".to_string(),
            }]
        );
        let usage = result.usage.expect("usage");
        assert_eq!(usage.total_billed_tokens, 39);
        assert_eq!(
            usage.provider_request_id.as_deref(),
            Some("chatcmpl_tools_json")
        );
    }

    #[test]
    fn single_completion_salvages_concatenated_native_tool_arguments_into_multiple_actions() {
        let server = MockSseServer::new(concat!(
            "data: {\"id\":\"chatcmpl_concat_tools\",\"model\":\"qwen3-coder-30b-a3b\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"I will search the grace-period symbols first.\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_concat_tools\",\"model\":\"qwen3-coder-30b-a3b\",\"choices\":[{\"index\":0,\"message\":{\"tool_calls\":[{\"id\":\"call_concat\",\"type\":\"function\",\"function\":{\"name\":\"search_text\",\"arguments\":\"{\\\"query\\\":\\\"GracePeriod\\\"}{\\\"query\\\":\\\"grace_period_ends_at\\\"}{\\\"query\\\":\\\"within_grace_period\\\",\\\"limit\\\":4}\"}}]},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":32,\"completion_tokens\":16,\"total_tokens\":48}}\n\n",
            "data: [DONE]\n\n",
        ));
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let request = StreamRequest {
            request_id: 20,
            session_id: 0,
            model_id: "ollama/qwen3-coder:30b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: Some(server.base_url()),
            max_completion_tokens: Some(768),
            include_repo_capsule: true,
            disable_reasoning: false,
            native_tool_calls: true,
            watchdog: None,
            safety_mode_label: Some("heavy_local".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let result = runtime
            .block_on(request_single_completion_details(
                &SsdMoeRuntimeHandle::shared_handle(),
                &request,
            ))
            .expect("single completion");

        let native_turn = result.native_turn.expect("native turn");
        assert!(result.native_turn_error.is_none(), "unexpected parse error");
        assert_eq!(
            native_turn.assistant_message,
            "I will search the grace-period symbols first."
        );
        assert_eq!(
            native_turn.actions,
            vec![
                AgentAction::SearchText {
                    query: "GracePeriod".to_string(),
                    limit: 8,
                },
                AgentAction::SearchText {
                    query: "grace_period_ends_at".to_string(),
                    limit: 8,
                },
                AgentAction::SearchText {
                    query: "within_grace_period".to_string(),
                    limit: 4,
                },
            ]
        );
        let usage = result.usage.expect("usage");
        assert_eq!(usage.total_billed_tokens, 48);
        assert_eq!(
            usage.provider_request_id.as_deref(),
            Some("chatcmpl_concat_tools")
        );
    }

    #[test]
    fn single_completion_parses_json_tool_calls_into_agent_turn() {
        let server = MockSseServer::new(concat!(
            "data: {\"id\":\"chatcmpl_json_tools\",\"model\":\"qwen3-coder-30b-a3b\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"{\\n  \\\"tool_calls\\\": [\\n    {\\n      \\\"type\\\": \\\"ListDirectory\\\",\\n      \\\"name\\\": \\\"ListDirectory\\\",\\n      \\\"arguments\\\": {\\n        \\\"path\\\": \\\".\\\"\\n      }\\n    }\\n  ]\\n}\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":31,\"completion_tokens\":11,\"total_tokens\":42}}\n\n",
            "data: [DONE]\n\n",
        ));
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let request = StreamRequest {
            request_id: 20,
            session_id: 0,
            model_id: "ollama/qwen3-coder:30b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: Some(server.base_url()),
            max_completion_tokens: Some(768),
            include_repo_capsule: true,
            disable_reasoning: false,
            native_tool_calls: true,
            watchdog: None,
            safety_mode_label: Some("heavy_local".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let result = runtime
            .block_on(request_single_completion_details(
                &SsdMoeRuntimeHandle::shared_handle(),
                &request,
            ))
            .expect("single completion");

        let native_turn = result.native_turn.expect("native turn");
        assert!(result.native_turn_error.is_none(), "unexpected parse error");
        assert_eq!(native_turn.actions.len(), 1);
        assert_eq!(
            native_turn.actions[0],
            AgentAction::ListDirectory {
                path: ".".to_string(),
            }
        );
        let usage = result.usage.expect("usage");
        assert_eq!(usage.total_billed_tokens, 42);
        assert_eq!(
            usage.provider_request_id.as_deref(),
            Some("chatcmpl_json_tools")
        );
    }

    #[test]
    fn single_completion_parses_json_tool_calls_with_inline_fields_into_agent_turn() {
        let server = MockSseServer::new(concat!(
            "data: {\"id\":\"chatcmpl_inline_tools\",\"model\":\"qwen3-coder-30b-a3b\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"{\\n  \\\"tool_calls\\\": [\\n    {\\n      \\\"type\\\": \\\"ListDirectory\\\",\\n      \\\"path\\\": \\\"crates/billing-domain\\\"\\n    }\\n  ]\\n}\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":33,\"completion_tokens\":12,\"total_tokens\":45}}\n\n",
            "data: [DONE]\n\n",
        ));
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let request = StreamRequest {
            request_id: 21,
            session_id: 0,
            model_id: "ollama/qwen3-coder:30b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: Some(server.base_url()),
            max_completion_tokens: Some(768),
            include_repo_capsule: true,
            disable_reasoning: false,
            native_tool_calls: true,
            watchdog: None,
            safety_mode_label: Some("heavy_local".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let result = runtime
            .block_on(request_single_completion_details(
                &SsdMoeRuntimeHandle::shared_handle(),
                &request,
            ))
            .expect("single completion");

        let native_turn = result.native_turn.expect("native turn");
        assert!(result.native_turn_error.is_none(), "unexpected parse error");
        assert_eq!(
            native_turn.actions,
            vec![AgentAction::ListDirectory {
                path: "crates/billing-domain".to_string(),
            }]
        );
        let usage = result.usage.expect("usage");
        assert_eq!(usage.total_billed_tokens, 45);
        assert_eq!(
            usage.provider_request_id.as_deref(),
            Some("chatcmpl_inline_tools")
        );
    }

    #[test]
    fn single_completion_parses_json_tool_calls_with_tool_name_and_tool_args() {
        let server = MockSseServer::new(concat!(
            "data: {\"id\":\"chatcmpl_tool_args\",\"model\":\"qwen3-coder-30b-a3b\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"{\\n  \\\"tool_calls\\\": [\\n    {\\n      \\\"tool_call_id\\\": \\\"call_0\\\",\\n      \\\"tool_call_type\\\": \\\"tool\\\",\\n      \\\"tool_name\\\": \\\"ListDirectory\\\",\\n      \\\"tool_args\\\": {\\n        \\\"path\\\": \\\"crates/billing-domain/src\\\"\\n      }\\n    }\\n  ]\\n}\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":34,\"completion_tokens\":14,\"total_tokens\":48}}\n\n",
            "data: [DONE]\n\n",
        ));
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let request = StreamRequest {
            request_id: 22,
            session_id: 0,
            model_id: "ollama/qwen3-coder:30b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: Some(server.base_url()),
            max_completion_tokens: Some(768),
            include_repo_capsule: true,
            disable_reasoning: false,
            native_tool_calls: true,
            watchdog: None,
            safety_mode_label: Some("heavy_local".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let result = runtime
            .block_on(request_single_completion_details(
                &SsdMoeRuntimeHandle::shared_handle(),
                &request,
            ))
            .expect("single completion");

        let native_turn = result.native_turn.expect("native turn");
        assert!(result.native_turn_error.is_none(), "unexpected parse error");
        assert_eq!(
            native_turn.actions,
            vec![AgentAction::ListDirectory {
                path: "crates/billing-domain/src".to_string(),
            }]
        );
        let usage = result.usage.expect("usage");
        assert_eq!(usage.total_billed_tokens, 48);
        assert_eq!(
            usage.provider_request_id.as_deref(),
            Some("chatcmpl_tool_args")
        );
    }

    #[test]
    fn single_completion_parses_mixed_content_with_fenced_tool_json_into_agent_turn() {
        let server = MockSseServer::new(concat!(
            "data: {\"id\":\"chatcmpl_fenced_tool\",\"model\":\"qwen3-coder-30b-a3b\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"I will inspect the billing-domain crate first:\\n```json\\n{\\n  \\\"tool_name\\\": \\\"list_directory\\\",\\n  \\\"tool_args\\\": {\\n    \\\"path\\\": \\\"crates/billing-domain\\\"\\n  }\\n}\\n```\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":35,\"completion_tokens\":27,\"total_tokens\":62}}\n\n",
            "data: [DONE]\n\n",
        ));
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let request = StreamRequest {
            request_id: 23,
            session_id: 0,
            model_id: "ollama/qwen3-coder:30b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: Some(server.base_url()),
            max_completion_tokens: Some(768),
            include_repo_capsule: true,
            disable_reasoning: false,
            native_tool_calls: true,
            watchdog: None,
            safety_mode_label: Some("heavy_local".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let result = runtime
            .block_on(request_single_completion_details(
                &SsdMoeRuntimeHandle::shared_handle(),
                &request,
            ))
            .expect("single completion");

        let native_turn = result.native_turn.expect("native turn");
        assert!(result.native_turn_error.is_none(), "unexpected parse error");
        assert_eq!(
            native_turn.actions,
            vec![AgentAction::ListDirectory {
                path: "crates/billing-domain".to_string(),
            }]
        );
        assert_eq!(
            native_turn.assistant_message,
            "I will inspect the billing-domain crate first:"
        );
        let usage = result.usage.expect("usage");
        assert_eq!(usage.total_billed_tokens, 62);
        assert_eq!(
            usage.provider_request_id.as_deref(),
            Some("chatcmpl_fenced_tool")
        );
    }

    #[test]
    fn single_completion_recovers_pseudo_tool_lines_from_plain_text() {
        let server = MockSseServer::new(concat!(
            "data: {\"id\":\"chatcmpl_pseudo_tools\",\"model\":\"qwen3-coder-30b-a3b\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"I will inspect the owning crate first.\\n```bash\\nListDirectory path: .\\nSearchText path: crates/billing-domain/src/lib.rs query: \\\"grace_period_ends_at\\\" limit: 4\\nReadFile path: crates/billing-domain/src/lib.rs\\nReadFile path: crates/billing-domain/src/lib.rs\\n```\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":38,\"completion_tokens\":44,\"total_tokens\":82}}\n\n",
            "data: [DONE]\n\n",
        ));
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let request = StreamRequest {
            request_id: 24,
            session_id: 0,
            model_id: "ollama/qwen3-coder:30b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: Some(server.base_url()),
            max_completion_tokens: Some(768),
            include_repo_capsule: true,
            disable_reasoning: false,
            native_tool_calls: true,
            watchdog: None,
            safety_mode_label: Some("heavy_local".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let result = runtime
            .block_on(request_single_completion_details(
                &SsdMoeRuntimeHandle::shared_handle(),
                &request,
            ))
            .expect("single completion");

        let native_turn = result.native_turn.expect("native turn");
        assert!(result.native_turn_error.is_none(), "unexpected parse error");
        assert_eq!(
            native_turn.assistant_message,
            "I will inspect the owning crate first."
        );
        assert_eq!(
            native_turn.actions,
            vec![
                AgentAction::ListDirectory {
                    path: ".".to_string(),
                },
                AgentAction::SearchText {
                    query: "grace_period_ends_at".to_string(),
                    limit: 4,
                },
                AgentAction::ReadFile {
                    path: "crates/billing-domain/src/lib.rs".to_string(),
                    range: None,
                },
            ]
        );
        let usage = result.usage.expect("usage");
        assert_eq!(usage.total_billed_tokens, 82);
        assert_eq!(
            usage.provider_request_id.as_deref(),
            Some("chatcmpl_pseudo_tools")
        );
    }

    #[test]
    fn native_pseudo_tool_lines_coalesce_modify_toml_arguments() {
        let turn = native_turn_from_pseudo_tool_lines(
            "ModifyToml Cargo.toml [success]\narguments {\"expected_hash\":\"0123456789abcdef\",\"operations\":[{\"set_dependency\":{\"dependency_name\":\"chrono\",\"version\":\"0.4\"}}]}",
        )
        .expect("parse pseudo modify_toml")
        .expect("turn");

        assert_eq!(turn.actions.len(), 1);
        assert!(turn.parse_warnings.is_empty());
        assert!(matches!(
            &turn.actions[0],
            AgentAction::ModifyToml {
                path,
                expected_hash,
                operations,
            } if path == "Cargo.toml"
                && expected_hash == "0123456789abcdef"
                && matches!(
                    &operations[0],
                    quorp_agent_core::agent_protocol::TomlEditOperation::SetDependency {
                        table,
                        name,
                        version,
                        ..
                    } if table == "dependencies"
                        && name == "chrono"
                        && version.as_deref() == Some("0.4")
                )
        ));
    }

    #[test]
    fn build_request_messages_include_mode_and_repo_instructions() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp_dir.path().join(".rules"),
            "Prefer detailed Rust reasoning.",
        )
        .expect("write rules");
        let request = StreamRequest {
            request_id: 1,
            session_id: 0,
            model_id: "qwen35-35b-a3b".to_string(),
            agent_mode: AgentMode::Plan,
            latest_input: "inspect the repo".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect the repo".to_string(),
            }],
            project_root: temp_dir.path().to_path_buf(),
            base_url_override: Some("http://127.0.0.1:1234/v1".to_string()),
            max_completion_tokens: Some(4096),
            include_repo_capsule: true,
            disable_reasoning: false,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: None,
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let messages = build_request_messages(&request);
        assert!(messages[0].content.contains("Current mode: Plan"));
        assert!(
            messages[0]
                .content
                .contains("Prefer detailed Rust reasoning.")
        );
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn build_request_messages_keeps_only_leading_system_message_when_compacted() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let mut transcript = Vec::new();
        for index in 0..8 {
            transcript.push(ChatServiceMessage {
                role: if index % 2 == 0 {
                    ChatServiceRole::User
                } else {
                    ChatServiceRole::Assistant
                },
                content: format!("message {index} {}", "x".repeat(1_500)),
            });
        }

        let request = StreamRequest {
            request_id: 99,
            session_id: 0,
            model_id: "ssd_moe/qwen35-27b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "fix it".to_string(),
            messages: transcript,
            project_root: temp_dir.path().to_path_buf(),
            base_url_override: Some("http://127.0.0.1:5416/v1".to_string()),
            max_completion_tokens: Some(512),
            include_repo_capsule: false,
            disable_reasoning: true,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: Some("heavy_local".to_string()),
            prompt_compaction_policy: Some(
                quorp_agent_core::PromptCompactionPolicy::Last6Ledger768,
            ),
            capture_scope: None,
            capture_call_class: None,
        };

        let messages = build_request_messages(&request);
        assert_eq!(messages[0].role, "system");
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.role == "system")
                .count(),
            1
        );
        assert!(messages.iter().any(|message| {
            message.role == "user" && message.content.starts_with("[Compacted Prior Context]")
        }));
    }

    #[test]
    fn build_system_prompt_skips_repo_capsule_when_disabled() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp_dir.path().join("Cargo.toml"),
            "[package]\nname = \"toy\"\nversion = \"0.1.0\"\n",
        )
        .expect("cargo");
        let request = StreamRequest {
            request_id: 1,
            session_id: 0,
            model_id: "qwen3-coder-30b-a3b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: temp_dir.path().to_path_buf(),
            base_url_override: Some("http://127.0.0.1:1234/v1".to_string()),
            max_completion_tokens: Some(512),
            include_repo_capsule: false,
            disable_reasoning: true,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: Some("safe_local".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let prompt = build_system_prompt(&request);
        assert!(prompt.contains("Safety mode: safe_local."));
        assert!(!prompt.contains("Repository Map (Auto-Injected)"));
        assert!(prompt.contains("Configured MCP servers: none."));
        assert!(prompt.contains("For `task_updates`, `status` must be exactly one of"));
        assert!(prompt.contains("Minimal valid example:"));
        assert!(prompt.contains("Never emit fake `[Tool Output]`"));
        assert!(prompt.contains("Emit at most 4 actions per turn"));
    }

    #[test]
    fn build_system_prompt_lists_configured_mcp_servers() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp_dir.path().join(".quorp")).expect("config dir");
        std::fs::write(
            temp_dir.path().join(".quorp/agent.toml"),
            r#"[[mcp_servers]]
name = "docs"
command = "docs-mcp"

[[mcp_servers]]
name = "filesystem"
command = "fs-mcp"
"#,
        )
        .expect("write config");
        let request = StreamRequest {
            request_id: 2,
            session_id: 0,
            model_id: "qwen3-coder-30b-a3b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "search docs".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "search docs".to_string(),
            }],
            project_root: temp_dir.path().to_path_buf(),
            base_url_override: Some("http://127.0.0.1:1234/v1".to_string()),
            max_completion_tokens: Some(512),
            include_repo_capsule: false,
            disable_reasoning: true,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: None,
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let prompt = build_system_prompt(&request);
        assert!(prompt.contains(r#"{"McpCallTool":{"server_name":"docs""#));
        assert!(prompt.contains("Configured MCP servers:"));
        assert!(prompt.contains("- docs (configured stdio server)"));
        assert!(prompt.contains("- filesystem (configured stdio server)"));
    }

    #[test]
    fn agent_tools_prompt_and_schema_filtering_use_settings() {
        let _env_lock = crate::quorp::tui::ssd_moe_tui::test_env_lock();
        struct HomeGuard(Option<std::ffi::OsString>);
        impl Drop for HomeGuard {
            fn drop(&mut self) {
                unsafe {
                    match self.0.as_ref() {
                        Some(value) => std::env::set_var("HOME", value),
                        None => std::env::remove_var("HOME"),
                    }
                }
            }
        }
        let temp_home = tempfile::tempdir().expect("home");
        let project = tempfile::tempdir().expect("project");
        std::fs::create_dir_all(temp_home.path().join(".quorp")).expect("home config");
        std::fs::write(
            temp_home.path().join(".quorp/settings.json"),
            r#"{
              "agent_tools": {
                "enabled": true,
                "tools": {
                  "fd": {"enabled": true, "command": "fd"},
                  "ast_grep": {"enabled": false, "command": "ast-grep"},
                  "cargo_diagnostics": {
                    "enabled": true,
                    "check_command": "cargo check --message-format=json"
                  }
                }
              }
            }"#,
        )
        .expect("settings");
        let _home_guard = HomeGuard(std::env::var_os("HOME"));
        unsafe {
            std::env::set_var("HOME", temp_home.path());
        }
        let request = StreamRequest {
            request_id: 3,
            session_id: 0,
            model_id: "nvidia/qwen/qwen3-coder-480b-a35b-instruct".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "inspect".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "inspect".to_string(),
            }],
            project_root: project.path().to_path_buf(),
            base_url_override: Some("https://integrate.api.nvidia.com/v1".to_string()),
            max_completion_tokens: Some(512),
            include_repo_capsule: false,
            disable_reasoning: true,
            native_tool_calls: true,
            watchdog: None,
            safety_mode_label: None,
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let prompt = build_system_prompt(&request);
        assert!(prompt.contains("Configured coding tools:"));
        assert!(prompt.contains("FindFiles:"));
        assert!(prompt.contains("CargoDiagnostics:"));
        assert!(!prompt.contains("StructuralSearch:"));

        let names = native_tool_definitions_for_request(&request)
            .into_iter()
            .filter_map(|tool| {
                tool.get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .collect::<Vec<_>>();
        assert!(names.contains(&"find_files".to_string()));
        assert!(names.contains(&"cargo_diagnostics".to_string()));
        assert!(!names.contains(&"structural_search".to_string()));
        assert!(!names.contains(&"structural_edit_preview".to_string()));
    }

    #[test]
    fn turbohero_models_get_longer_ready_timeout() {
        let request = StreamRequest {
            request_id: 1,
            session_id: 0,
            model_id: "ssd_moe/deepseek-coder-v2-lite-turbo".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "fix it".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "fix it".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: None,
            max_completion_tokens: Some(512),
            include_repo_capsule: false,
            disable_reasoning: true,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: Some("safe_local".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        assert_eq!(
            server_ready_timeout(&request),
            TURBOHERO_SERVER_READY_TIMEOUT
        );
    }

    #[test]
    fn single_completion_watchdog_reports_first_token_timeout() {
        let server = MockSseServer::with_response_delay(
            concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"slow\"},\"finish_reason\":null}]}\n\n",
                "data: [DONE]\n\n",
            ),
            Duration::from_millis(200),
        );
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let request = StreamRequest {
            request_id: 1,
            session_id: 0,
            model_id: "qwen3-coder-30b-a3b".to_string(),
            agent_mode: AgentMode::Act,
            latest_input: "hello".to_string(),
            messages: vec![ChatServiceMessage {
                role: ChatServiceRole::User,
                content: "hello".to_string(),
            }],
            project_root: PathBuf::from("/tmp"),
            base_url_override: Some(server.base_url()),
            max_completion_tokens: Some(512),
            include_repo_capsule: false,
            disable_reasoning: true,
            native_tool_calls: false,
            watchdog: Some(quorp_agent_core::CompletionWatchdogConfig {
                first_token_timeout_ms: Some(50),
                idle_timeout_ms: Some(50),
                total_timeout_ms: Some(500),
            }),
            safety_mode_label: Some("safe_local".to_string()),
            prompt_compaction_policy: None,
            capture_scope: None,
            capture_call_class: None,
        };

        let error = runtime
            .block_on(request_single_completion_details(
                &SsdMoeRuntimeHandle::shared_handle(),
                &request,
            ))
            .expect_err("watchdog should time out");
        assert!(error.contains("first token timeout"));
    }

    #[test]
    fn malformed_sse_payload_surfaces_error() {
        let server = MockSseServer::new(concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n",
            "data: {not-json}\n\n",
        ));
        let events = collect_submit_prompt_events(11, server.base_url(), Duration::from_secs(20));
        let saw_delta = events.iter().any(|event| {
            matches!(
                event,
                TuiEvent::Chat(ChatUiEvent::AssistantDelta(11, text)) if text.contains("hello")
            )
        });
        let saw_error = events.iter().any(|event| {
            matches!(
                event,
                TuiEvent::Chat(ChatUiEvent::Error(11, error))
                    if error.contains("Malformed SSE payload")
            )
        });
        let saw_finish = events
            .iter()
            .any(|event| matches!(event, TuiEvent::Chat(ChatUiEvent::StreamFinished(11))));

        assert!(
            saw_delta,
            "expected partial output before malformed SSE failure"
        );
        assert!(saw_error, "expected parse error from malformed SSE payload");
        assert!(
            saw_finish,
            "expected stream to terminate after malformed SSE failure"
        );
    }

    #[test]
    fn unexpected_stream_end_surfaces_error_after_partial_output() {
        let server = MockSseServer::new(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n",
        );
        let events = collect_submit_prompt_events(12, server.base_url(), Duration::from_secs(20));
        let saw_partial = events.iter().any(|event| {
            matches!(
                event,
                TuiEvent::Chat(ChatUiEvent::AssistantDelta(12, text)) if text.contains("partial")
            )
        });
        let saw_error = events.iter().any(|event| {
            matches!(
                event,
                TuiEvent::Chat(ChatUiEvent::Error(12, error))
                    if error.contains("before sending [DONE]")
            )
        });
        let saw_finish = events
            .iter()
            .any(|event| matches!(event, TuiEvent::Chat(ChatUiEvent::StreamFinished(12))));

        assert!(
            saw_partial,
            "expected partial assistant output before disconnect"
        );
        assert!(saw_error, "expected disconnect error for truncated stream");
        assert!(
            saw_finish,
            "expected stream finished after disconnect error"
        );
    }

    #[test]
    fn compact_transcript_prunes_structured_tool_results() {
        let mut messages = vec![ChatServiceMessage {
            role: ChatServiceRole::User,
            content: "goal".to_string(),
        }];
        for index in 0..8 {
            messages.push(ChatServiceMessage {
                role: ChatServiceRole::Assistant,
                content: format!("assistant {index}"),
            });
            let mut tool_output = String::from("[Tool Success] replace_block\n");
            for line in 0..40 {
                tool_output.push_str(&format!("line {line}\n"));
            }
            messages.push(ChatServiceMessage {
                role: ChatServiceRole::User,
                content: tool_output,
            });
        }

        let compacted = compact_transcript_with_policy(&messages, None);
        assert!(
            compacted
                .iter()
                .any(|message| { message.content.contains("lines pruned for context length") })
        );
    }
}

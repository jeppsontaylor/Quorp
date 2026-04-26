use quorp_agent_core::{
    AgentAction, AgentTurnResponse, PreviewEditPayload, ReadFileRange, ValidationPlan,
};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use crate::quorp::executor::InteractiveProviderKind;
use crate::quorp::prompt_compaction::{PromptMessage, PromptMessageRole, apply_prompt_compaction};
use crate::quorp::tui::agent_context::{
    load_instruction_context, render_instruction_context_for_prompt,
};
use crate::quorp::tui::agent_protocol::AgentMode;
use quorp_provider::openai_compatible_client::{
    OpenAiCompatibleChatMessage as RemoteChatMessage,
    OpenAiCompatibleChatRequest as RemoteChatRequest,
    OpenAiCompatibleClientConfig as RemoteClientConfig,
    OpenAiCompatibleStreamEvent as RemoteStreamEvent, build_request_body, parse_sse_data_line,
    parse_sse_payload,
};

mod provider;
mod request;

use provider::*;
pub use request::{ChatServiceMessage, ChatServiceRole, SingleCompletionResult, StreamRequest};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const READ_TIMEOUT: Duration = Duration::from_secs(120);
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
- {"ReplaceRange":{"path":"src/main.rs","range":{"start_line":390,"end_line":450},"replacement":"full replacement text for that line range"}}
- {"SetExecutable":{"path":"scripts/run.sh"}}
- {"McpCallTool":{"server_name":"docs","tool_name":"search","arguments":{"query":"validation plan"}}}
- {"RunValidation":{"plan":{"fmt":true,"clippy":true,"workspace_tests":false,"tests":["chat_flows"],"custom_commands":[]}}}

All file paths are relative to the workspace root and will be rejected if they escape the project. Do not use absolute paths in tool calls. If you need orientation, start with `{"ListDirectory":{"path":"."}}`.
Use `SearchText`, `SearchSymbols`, and `GetRepoCapsule` before broad file reads when you need repo-wide context.
`ApplyPatch` applies unified diff patches and can update, add, delete, or rename files in one action.
`ReplaceBlock` finds a match of `search_block` in the file (ignoring indentation) and replaces it with `replace_block`. Prefer it for small exact edits to existing files, and output ONLY the lines that need changing. If a snippet is repeated, include `range` from the latest read slice or use `ApplyPatch`; never guess which duplicate to edit.
`ReplaceRange` replaces exactly the observed line range from a recent `ReadFile`. Prefer it during benchmark repair when the runtime gives an honored range and patch output would be long.
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

fn apply_extra_headers(
    mut request_builder: reqwest::RequestBuilder,
    extra_headers: &BTreeMap<String, String>,
) -> reqwest::RequestBuilder {
    for (name, value) in extra_headers {
        request_builder = request_builder.header(name, value);
    }
    request_builder
}

mod tools_schema;
mod turn_parse;

pub(crate) use tools_schema::*;
pub(crate) use turn_parse::*;

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
    if nvidia_qwen_benchmark_profile(request) {
        prompt.push_str(
            "\n\nQwen benchmark JSON contract:\n\
- Return one raw JSON object only; no Markdown, no prose before or after it.\n\
- Prefer minimal action-only JSON with `assistant_message` set to \"\".\n\
- Repair packets are authoritative; obey the required next action before doing anything else.\n\
- In locked repair phases, emit exactly one action.\n\
- Do not reread files the latest repair packet says are already loaded.\n\
- Prefer `ReplaceRange` for an already-loaded owner slice. Do not emit full-file unified diffs.\n\
- Do not patch manifest/dependency files unless the latest failure explicitly names a missing dependency.\n\
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
        let capsule =
            quorp_tools::path_index::build_repo_capsule(request.project_root.as_path(), None, 40);
        let map_text = quorp_tools::path_index::render_repo_capsule(None, &capsule);
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

fn build_request_messages(request: &StreamRequest) -> Vec<RemoteChatMessage> {
    let mut request_messages = vec![RemoteChatMessage {
        role: "system",
        content: build_system_prompt(request),
    }];
    let compacted =
        compact_transcript_with_policy(&request.messages, request.prompt_compaction_policy);
    request_messages.extend(compacted.into_iter().filter_map(|message| {
        if message.content.trim().is_empty() {
            return None;
        }
        Some(RemoteChatMessage {
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

fn reasoning_effort_for_model(model_id: &str) -> Option<String> {
    if model_id.eq_ignore_ascii_case("qwen/qwen3-coder-480b-a35b-instruct")
        || model_id.eq_ignore_ascii_case("qwen3-coder-480b-a35b-instruct")
    {
        Some("medium".to_string())
    } else {
        None
    }
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

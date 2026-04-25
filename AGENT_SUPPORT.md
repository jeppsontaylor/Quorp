# AGENT_SUPPORT.md

## 1. Purpose and Evidence Policy

This document is a support audit for Quorp's current native Rust TUI and agent runtime. It describes what the current worktree can actually do, how the major flows are wired, and where the remaining gaps still are.

Primary native entrypoints:

- `crates/quorp/src/main.rs`
- `crates/quorp/src/quorp/tui.rs`
- `crates/quorp/src/quorp/tui/app.rs`

Evidence labels used below:

- `Flow-tested`: exercised by `cargo test -p quorp tui_flow_tests`
- `Unit-tested`: covered by local unit tests only
- `Implemented`: present in the current code path, but not clearly flow-tested end-to-end
- `Placeholder / not fully wired`: visible in code or UI, but not functionally complete

Current validation baseline for this document:

- `cargo test -p quorp tui_flow_tests -- --list` reports **120 TUI flow tests**
- `cargo test -p quorp -- --list` reports **381 tests**
- Recent targeted verification in this worktree passed:
  - `cargo test -p quorp agent_runtime -- --nocapture`
  - `cargo test -p quorp compact_transcript_prunes_structured_tool_results -- --nocapture`
  - `cargo test -p quorp apply_tui_backend_event_routes_start_agent_task_into_runtime -- --nocapture`
  - `cargo test -p quorp --no-run`
  - `cargo clippy -p quorp --all-targets --no-deps -- -D warnings`

Rules for interpreting support:

- A capability is only called fully supported in the TUI when the native path can reach it and the evidence is strong.
- UI text and placeholder panes do not count as support unless the backing code path is connected.
- Optional hooks are called out separately when the main native app path does not currently supply them.

## 1.1 Headless Benchmark Safety Defaults

The headless benchmark runner now has a separate safety posture from the TUI:

- `quorp benchmark run` now asks the shared SSD-MOE broker for its default primary model when `--model` is omitted.
- If the shared broker is unavailable, safe benchmark fallback stays on `ssd_moe/deepseek-coder-v2-lite-turbo` rather than silently escalating to the heavy 30B path.
- Heavy local benchmark use of `qwen35-35b-a3b` still requires `--allow-heavy-local-model`.
- Safe benchmark runs stage benchmark context instead of inlining every context file into the first prompt.
- Safe benchmark runs disable auto repo-capsule injection for the model request.
- Safe benchmark runs cap completion sizes and disable reasoning mode.
- Safe benchmark runs apply model-request watchdogs for first token, idle stream time, and total request time.
- A global lockfile prevents overlapping headless benchmark runs from sharing the same local benchmark execution slot.
- The supported benchmark contract is now local shared-MOE only.
- `ssd_moe/deepseek-coder-v2-lite-turbo` remains the lightweight smoke and preflight model.
- `ssd_moe/qwen3-coder-30b-a3b` is the publication benchmark model for the managed WarpOS lane.

## 1.2 Managed WarpOS Integration

Quorp now supports a managed WarpOS lane without requiring a Quorp-owned Docker sandbox:

- WarpOS can stage a built `quorp` Rust binary into `workspace-ssh`.
- Portable auth/runtime config can live in `~/.quorp/.env`.
- The managed WarpOS benchmark path is local-only and reaches the shared SSD-MOE runtime through the managed HTTPS capture probe.
- The WarpOS MITM/proxy path now observes Quorp local traffic through that wrapper, so Quorp can enter the same fused telemetry path as the other agents.
- The legacy `--docker` re-exec path still exists for compatibility, but it is no longer the preferred integration model for WarpOS.

## 2. Architecture Overview

### End-to-end architecture

The native TUI is a single-process Rust application with cooperating subsystems:

1. `main.rs` resolves the workspace root, starts diagnostics and memory logging, creates event channels, and enters `tui::run(...)`.
2. `tui::run(...)` in `crates/quorp/src/quorp/tui.rs` initializes the terminal, creates a Tokio runtime, and starts the native event loops.
3. `TuiApp` in `crates/quorp/src/quorp/tui/app.rs` owns the visible shell state and routes input, overlay state, pane focus, assistant rendering, terminal drawer state, and workspace/thread persistence.
4. `ChatPane` and `ChatService` implement the main assistant coding flow.
5. `AgentRuntime` in `crates/quorp/src/quorp/tui/agent_runtime.rs` implements the autonomous `/agent` background loop.
6. `TerminalPane` plus the PTY backend provide the integrated shell.
7. `WorkspaceStore` persists project and thread metadata plus chat snapshots.

### Architecture truth

The current TUI hosts a real autonomous agent runtime. For the supported WarpOS benchmark lane, Quorp is local-only: it uses the shared SSD-MOE runtime and reaches it through the managed HTTPS capture wrapper so the same HTTP/SSE telemetry path can be fused with the other agents.

Key current facts:

- `/agent <goal>` launches a typed `AgentTaskRequest` from `ChatPane`.
- `TuiEvent::StartAgentTask(agent_runtime::AgentTaskRequest)` is now a first-class event.
- `TuiEvent::AgentRuntime(agent_runtime::AgentUiEvent)` carries runtime status updates back to the UI.
- `AgentRuntimeCommand::StartTask`, `AgentRuntimeCommand::ToolFinished`, and `AgentRuntimeCommand::Cancel` drive the runtime loop.
- `AgentRuntime` uses a reserved `AGENT_RUNTIME_SESSION_ID` so tool results from the native backend can be routed back into the autonomous loop.
- Normal chat still uses streaming SSE.
- The autonomous runtime uses a single-shot completion path through `request_single_completion(...)` rather than the normal streaming chat path.
- The runtime tracks goal, mode, autonomy profile, acceptance criteria, working set, last tool summary, last failing verifier, last safe checkpoint, stall count, mutation state, verification state, and a validation queue.
- `AgentPane` is a status/review surface that visualizes runtime updates; it is not the primary task input surface.

### File tree view

```text
crates/quorp/src/quorp/tui/
├── action_discovery.rs
├── agent_context.rs
├── agent_pane.rs
├── agent_protocol.rs
├── agent_runtime.rs
├── agent_turn.rs
├── assistant_transcript.rs
├── bootstrap_loader.rs
├── bridge.rs
├── buffer_png.rs
├── chrome.rs
├── chrome_v2.rs
├── chat.rs
├── chat_service.rs
├── command_bridge.rs
├── app.rs
├── diagnostics.rs
├── editor_pane.rs
├── file_tree.rs
├── hitmap.rs
├── mcp_client.rs
├── mention_links.rs
├── model_registry.rs
├── models_pane.rs
├── native_backend.rs
├── openai_compatible_client.rs
├── path_guard.rs
├── paint.rs
├── path_index.rs
├── shell.rs
├── ssd_moe_client.rs
├── ssd_moe_tui.rs
├── terminal_pane.rs
├── terminal_surface.rs
├── terminal_trace.rs
├── theme.rs
├── tui_backend.rs
├── workbench.rs
├── workspace_state.rs
└── tui_flow_tests/
    ├── backend_flows.rs
    ├── chat_flows.rs
    ├── chat_http_mock.rs
    ├── editor_pane_flows.rs
    ├── file_tree_flows.rs
    ├── global_shortcuts.rs
    ├── mention_flows.rs
    ├── models_picker_flows.rs
    ├── mouse_flows.rs
    ├── navigation_flows.rs
    ├── rollback_flows.rs
    ├── rust_capture_flows.rs
    ├── screenshot_suite.rs
    ├── session_isolation_flows.rs
    ├── tab_strip_flows.rs
    ├── terminal_certification_flows.rs
    ├── terminal_flows.rs
    ├── visual_flows.rs
    ├── visual_regression.rs
    └── vim_navigation_matrix.rs
```

### Core interfaces and types

These types define the actual support boundary:

- `TuiEvent` in `crates/quorp/src/quorp/tui.rs`
- `TuiToBackendRequest` in `crates/quorp/src/quorp/tui/bridge.rs`
- `BackendToTuiResponse` in `crates/quorp/src/quorp/tui/bridge.rs`
- `CommandBridgeRequest` in `crates/quorp/src/quorp/tui/command_bridge.rs`
- `ChatUiEvent` in `crates/quorp/src/quorp/tui/chat.rs`
- `AgentTurnResponse` in `crates/quorp/src/quorp/tui/agent_turn.rs`
- `AgentAction` in `crates/quorp/src/quorp/tui/agent_protocol.rs`
- `ValidationPlan` in `crates/quorp/src/quorp/tui/agent_protocol.rs`
- `AutonomyProfile` in `crates/quorp/src/quorp/tui/agent_context.rs`
- `AgentUiEvent` in `crates/quorp/src/quorp/tui/agent_runtime.rs`
- `PendingCommand` in `crates/quorp/src/quorp/tui/chat.rs`
- `ModelStatus` in `crates/quorp/src/quorp/tui/ssd_moe_tui.rs`
- `ThreadStatus` in `crates/quorp/src/quorp/tui/workspace_state.rs`

## 3. TUI Capability Matrix

### Chat and agent interaction

| Capability | User entrypoint | Core files | Backend/service path | Safety model | Current status | Evidence |
| --- | --- | --- | --- | --- | --- | --- |
| Multi-session assistant threads | `Ctrl+t`, tab strip, session pills | `chat.rs`, `app.rs`, `shell.rs` | In-memory session state in `ChatPane` | No extra confirmation; session-local state only | Supported | `Flow-tested` via `tui_flow_tests/chat_flows.rs` and `tui_flow_tests/session_isolation_flows.rs` |
| Session switching and closing | Tab strip focus, `Alt+Up`, arrows, `Delete`, `Ctrl+w`, `Ctrl+Shift+w` | `chat.rs`, `app.rs` | Pure UI/session routing | No extra confirmation | Supported | `Flow-tested` via `tui_flow_tests/chat_flows.rs` and `tui_flow_tests/tab_strip_flows.rs` |
| Streaming token updates | Submit composer with `Enter` | `chat.rs`, `chat_service.rs` | SSE stream into `ChatUiEvent::AssistantDelta` | Local SSD-MOE stays on validated loopback; remote providers use normalized OpenAI-compatible base URLs | Supported | `Flow-tested` via `tui_flow_tests/chat_http_mock.rs`; unit coverage in `chat_service.rs` |
| Reasoning stream rendering | Assistant emits reasoning deltas | `chat_service.rs`, `assistant_transcript.rs` | SSE `reasoning_content` becomes transcript reasoning text | Rendering only | Implemented | Unit-tested in `openai_compatible_client.rs` and transcript code |
| Session isolation for streamed events | Multi-session chat use | `chat.rs` | Session id carried in `ChatUiEvent` | Session ids isolate transcript updates | Supported | `Flow-tested` via `tui_flow_tests/session_isolation_flows.rs` |
| Transcript rendering | Assistant feed and chat transcript | `assistant_transcript.rs`, `chat.rs`, `shell.rs` | Parsed assistant segments cached and rendered for chat/shell surfaces | Rendering only | Supported | `Flow-tested` via `tui_flow_tests/chat_flows.rs`; unit-tested parser coverage in `assistant_transcript.rs` |
| Code-fence syntax highlighting | Assistant emits fenced code blocks | `assistant_transcript.rs` | `syntect` highlight cache and rich rendering | Rendering only | Supported | `Flow-tested` via `tui_flow_tests/chat_flows.rs`; unit-tested in `assistant_transcript.rs` |
| Error surfacing in transcript | Chat failures or transport errors | `chat.rs`, `chat_service.rs` | `ChatUiEvent::Error` updates assistant text and state | Error is visible to user instead of being dropped | Supported | `Flow-tested` via `tui_flow_tests/backend_flows.rs` and `tui_flow_tests/chat_http_mock.rs` |
| Assistant feed scrolling | `PgUp`, `PgDn`, mouse wheel, scrollbar click | `app.rs`, `shell.rs` | Shell feed snapshot and hit map | Pure UI behavior | Supported | `Flow-tested` via `tui_flow_tests/chat_flows.rs` |
| Assistant feed link navigation | `Alt+Down`, `Alt+Enter`, mouse click on rendered links | `app.rs`, `shell.rs` | `open_external_target()` uses OS opener | Opens external target immediately | Implemented | Code path exists in `app.rs`; not clearly flow-tested |
| `/run` and `!command` inline command entry | Composer input | `chat_service.rs`, `chat.rs` | Converted into a synthetic `<run_command>` block | Still passes through confirmation flow | Supported | `Unit-tested` in `chat_service.rs` |
| `/agent <goal>` autonomous launch | Composer input | `chat.rs`, `tui.rs`, `app.rs`, `agent_runtime.rs` | Typed `AgentTaskRequest` is routed to `AgentRuntime` | Bounded by autonomy profile, runtime max iterations, and validation gating | Supported | `Unit-tested` in `chat.rs` and `app.rs`; `Flow-tested` through runtime routing tests |
| Autonomous runtime status display | `/agent <goal>` or runtime events | `agent_runtime.rs`, `agent_pane.rs`, `app.rs` | `AgentUiEvent::StatusUpdate`, `TurnCompleted`, `FatalError` | Display only; no direct editing from the pane | Supported | `Unit-tested` in `agent_runtime.rs` and `app.rs` |

Additional notes:

- The dedicated agent runtime is now real and runs independently of the normal assistant streaming path.
- `ChatPane` launches `/agent` tasks, but `AgentPane` is the visual status surface for the background runtime.

### Structured coding tool support

The assistant-side coding tool story is now a structured-turn + typed-action + command-bridge model.

The preferred path is:

1. `chat_service.rs` prompts for a strict JSON `AgentTurnResponse`.
2. `chat.rs` parses that JSON when a stream finishes.
3. The transcript is rewritten into human-readable text plus action receipts.
4. Read-only actions can auto-execute.
5. Mutating actions become `PendingCommand`s and require `y/n`.
6. `native_backend.rs` executes the typed action and streams output back.

Legacy transcript tags are still supported as a compatibility fallback, but they are no longer the preferred path.

#### Structured turn payload

`AgentTurnResponse` is the current contract for a model turn.

| Field | Meaning | Notes |
| --- | --- | --- |
| `assistant_message` | Human-visible explanation | Rendered into the transcript |
| `actions` | Structured agent operations | Current renderer only executes the first action in a turn |
| `task_updates` | Task progress items | Rendered as assistant-visible checklist items |
| `memory_updates` | Compact memory write suggestions | Rendered as notes for the user |
| `requested_mode_change` | Ask / Plan / Act transition request | Shown in the transcript and applied by the runtime where appropriate |
| `verifier_plan` | Suggested validation plan | Passed through to validation plumbing |

#### Action support

| Tool capability | How the assistant emits it | How the TUI extracts it | Native execution path | Safety model | Current status | Evidence |
| --- | --- | --- | --- | --- | --- | --- |
| `search_text` | `{"SearchText":{"query":"...","limit":N}}` | Parsed into `AgentAction::SearchText` | `native_backend.rs` via repo text search | Auto-approved read-only action | Supported | Unit-tested in `path_index.rs`, `native_backend.rs`, and `chat.rs`; flow-tested in `tui_flow_tests/chat_http_mock.rs` |
| `search_symbols` | `{"SearchSymbols":{"query":"...","limit":N}}` | Parsed into `AgentAction::SearchSymbols` | `native_backend.rs` via repo symbol search | Auto-approved read-only action | Supported | Unit-tested in `path_index.rs` and `native_backend.rs` |
| `get_repo_capsule` | `{"GetRepoCapsule":{"query":"...","limit":N}}` | Parsed into `AgentAction::GetRepoCapsule` | `native_backend.rs` via repo capsule builder | Auto-approved read-only action | Supported | Unit-tested in `path_index.rs` and `native_backend.rs` |
| `run_validation` | `{"RunValidation":{"plan":{...}}}` | Parsed into `AgentAction::RunValidation` | Validation commands are resolved from `.quorp/agent.toml` | Explicit `y/n`; command policy can auto-approve selected validation commands | Supported | Unit-tested in `agent_protocol.rs`, `agent_context.rs`, `native_backend.rs`; flow-tested through structured chat and runtime routes |
| `run_command` | `<run_command timeout_ms="...">cmd</run_command>` or structured turn action | Parsed into `PendingCommand::Run` or `AgentAction::RunCommand` | `native_backend.rs::spawn_run_command_task` | Explicit `y/n` confirmation before dispatch; timeout enforced | Supported | Unit-tested in `chat.rs` and command bridge code |
| `read_file` | `<read_file path="..."></read_file>` or structured turn action | Parsed into `PendingCommand::ReadFile` | `native_backend.rs::spawn_read_file_task` | Explicit `y/n`; path sanitized to project root | Supported | Unit-tested in `chat.rs` and `native_backend.rs` |
| `list_directory` | `<list_directory path="..."></list_directory>` or structured turn action | Parsed into `PendingCommand::ListDirectory` | `native_backend.rs::spawn_list_directory_task` | Explicit `y/n`; path sanitized to project root | Supported | Unit-tested in `chat.rs`; implemented in `native_backend.rs` |
| `write_file` | `<write_file path="...">full file content</write_file>` or structured turn action | Parsed into `PendingCommand::WriteFile` | `native_backend.rs::spawn_write_file_task` | Explicit `y/n`; path sanitized; temp-file rewrite strategy | Supported | Unit-tested in `chat.rs`; write path implemented in `native_backend.rs` |
| `ReplaceBlock` | Structured turn action with `search_block` and `replace_block` | Parsed into `AgentAction::ReplaceBlock` | Native backend performs search/replace block application | Explicit `y/n`; this is the preferred structured edit shape for existing files | Supported | Unit-tested in `native_backend.rs` and `agent_protocol.rs` |
| `apply_patch` | Legacy XML tag | Parsed into `PendingCommand::ApplyPatch` | Native backend currently treats it as full replacement content | Explicit `y/n`; unified diff payloads are rejected | Supported with important limitation | Unit-tested in `chat.rs` and `native_backend.rs` |
| `mcp_call_tool` | `{"McpCallTool":{"server_name":"...","tool_name":"...","arguments":{...}}}` | Parsed into `PendingCommand::McpCallTool` or structured turn action | Native backend currently returns a placeholder error | Explicit `y/n`; not wired to real native execution yet | Placeholder / not fully wired | Unit-tested in `chat.rs` and `mcp_client.rs`; current backend stub in `native_backend.rs` |

#### Rollback and write safety

The native backend keeps a session-scoped shadow copy of files before mutating operations and can restore that snapshot when validation fails.

| Capability | Trigger | Core files | Behavior | Current status | Evidence |
| --- | --- | --- | --- | --- | --- |
| Session-scoped rollback restore | `RunValidation` failure after prior write-like actions | `native_backend.rs`, `tui_flow_tests/rollback_flows.rs` | Restores stashed files and appends a rollback explanation to the failure text | Supported | `Flow-tested` via `tui_flow_tests/rollback_flows.rs`; unit coverage in `native_backend.rs` |

Important current truth for edit semantics:

- Legacy `<apply_patch>` is still a full replacement path.
- Unified diff payloads are explicitly rejected.
- The structured `ReplaceBlock` action is the current higher-value edit shape for existing files.

#### Tool flow in detail

1. `chat_service.rs` system prompt tells the assistant to prefer structured JSON turns and may fall back to legacy tags for:
   - `<run_command>`
   - `<read_file>`
   - `<list_directory>`
   - `<write_file>`
   - `<apply_patch>`
2. `chat.rs::apply_structured_turn_for_session(...)` parses and applies the preferred `AgentTurnResponse` path.
3. If no structured turn is present, `assistant_transcript.rs` plus `chat.rs::try_extract_pending_command_for_session(...)` provide the legacy fallback.
4. `chat.rs::execute_pending_command(...)` converts the pending tool into a `CommandBridgeRequest`.
5. `native_backend.rs::spawn_command_service_loop(...)` dispatches the concrete operation on a background thread.
6. Output is streamed back into the chat transcript via `ChatUiEvent::CommandOutput` and `ChatUiEvent::CommandFinished`.
7. `chat.rs::submit_input_for_followup(...)` sends the tool output back into the assistant for a follow-up response.

### Agent runtime specifics

The autonomous runtime is a distinct subsystem rather than a UI trick.

Key runtime behavior:

- It uses `request_single_completion(...)` so the agent can get a whole structured turn without streaming the turn itself.
- It injects the same repo and instruction context used by the normal chat path.
- It uses `AgentMode` to constrain action choices:
  - `Ask` is read-only
  - `Plan` allows reads plus validation
  - `Act` allows the full action set
- It honors `AutonomyProfile`:
  - `interactive`
  - `autonomous_host`
  - `autonomous_sandboxed`
- `AutonomousHost` blocks high-risk shell commands and refuses MCP tool execution.
- `AutonomousSandboxed` is present in config/model space but is not implemented in the native TUI yet.
- After a write-like action succeeds, the runtime automatically queues validation.
- When no further action is proposed after a mutation, the runtime runs final validation before declaring success.
- Plain-text output in autonomous mode is treated as a failure signal.

### User-initiated command entry

The current TUI also supports user-driven command intent from the composer:

- `/run <command>`
- `!command`

Behavior:

- `chat_service.rs::parse_inline_command(...)` detects those forms before the normal model request path.
- The TUI does not execute them immediately.
- Instead, the service emits assistant text that contains a synthetic `<run_command>` block.
- That block then enters the same pending-command confirmation flow used for assistant-proposed commands.

This remains a strong safety property: direct user command text still passes through the assistant tool confirmation path rather than silently executing.

### File attachment and mentions

| Capability | User entrypoint | Core files | Backend/service path | Safety model | Current status | Evidence |
| --- | --- | --- | --- | --- | --- | --- |
| `@file` mention popup | Type `@` in composer | `chat.rs`, `path_index.rs` | Path index query from current project root | Mention suggestions stay within project | Supported | `Flow-tested` via `tui_flow_tests/mention_flows.rs` |
| Mention acceptance into composer | `Tab` or `Enter` when popup active | `chat.rs`, `mention_links.rs` | Builds `[@label](file://...)` link | File URL must resolve inside project when expanded | Supported | `Flow-tested` via `tui_flow_tests/mention_flows.rs` |
| Mention popup scrolling | `Up`, `Down`, `PgUp`, `PgDn` semantics in popup | `chat.rs` | Pure UI state | No side effects | Supported | `Flow-tested` via `tui_flow_tests/mention_flows.rs` |
| Link expansion into attached file content | Submit message with mention link | `mention_links.rs`, `chat.rs` | Mention links become inline attachment context for model input | File URIs are resolved only if inside project root | Supported | `Unit-tested` in `mention_links.rs` |
| Link expansion into attached directory listing | Submit message with directory mention | `mention_links.rs` | `ignore::WalkBuilder` limited listing | Directory contents constrained to project root; max entries enforced | Supported | `Unit-tested` in `mention_links.rs` |
| Default native path index mode | Normal native app use | `path_index.rs`, `app.rs` | Local `ignore` walk + `notify` refresh | Local project-scoped index | Supported | `Flow-tested` for end-user mention behavior; implementation evident in current native path |
| Snapshot-backed path index mode | Alternate/project-backed mode | `path_index.rs`, `app.rs` | `PathIndex::new_project_backed(...)` and `apply_bridge_snapshot(...)` | Snapshot source determines visible entries | Implemented | `Flow-tested` in `tui_flow_tests/backend_flows.rs`, but not the default native app mode |

Current limits and guardrails:

- Mentioned file expansion limit: `512 KiB`
- Mentioned directory listing limit: `200` entries
- Snapshot-backed mention behavior is specifically covered in `tui_flow_tests/backend_flows.rs`
- The default native app path relies on local indexing; it does not receive `PathIndexSnapshot` events from `main.rs`

### File tree and code preview support for coding workflows

| Capability | User entrypoint | Core files | Backend/service path | Safety model | Current status | Evidence |
| --- | --- | --- | --- | --- | --- | --- |
| Explorer navigation | Focus file tree, arrows, `Enter` | `file_tree.rs`, `app.rs` | Local read or backend list request | Project path guard applied | Supported | `Flow-tested` via `tui_flow_tests/file_tree_flows.rs` and `tui_flow_tests/navigation_flows.rs` |
| Open file into preview | `Enter` on file row | `file_tree.rs`, `editor_pane.rs`, `app.rs` | Native backend emits `BufferSnapshot` | Preview only; no direct human edit path | Supported | `Flow-tested` via `tui_flow_tests/file_tree_flows.rs` and `tui_flow_tests/backend_flows.rs` |
| Read-only code preview | Editor pane | `editor_pane.rs` | Disk read or backend snapshot apply | Preview constrained to project root | Supported | `Flow-tested` and unit-tested |
| Snapshot-backed preview updates | `TuiEvent::BufferSnapshot` | `editor_pane.rs`, `app.rs`, `native_backend.rs` | Native backend publishes snapshots | No mutation on render | Supported | `Flow-tested` via `tui_flow_tests/backend_flows.rs` |
| Preview tabbed state | Multiple opened files, tab strip | `editor_pane.rs`, `app.rs` | UI-local tab state | No side effects besides active tab switch | Supported | `Flow-tested` via `tui_flow_tests/tab_strip_flows.rs`; unit-tested in `editor_pane.rs` |
| Scroll preservation per file | Reopen/switch tabs | `editor_pane.rs` | Local per-path scroll cache | No side effects | Supported | `Unit-tested` in `editor_pane.rs` |
| Path guard behavior | Open/preview selected file | `path_guard.rs`, `editor_pane.rs`, `file_tree.rs` | Rejects paths outside project root | Prevents escaping project for preview | Supported | `Unit-tested` in `editor_pane.rs` and explorer behavior is flow-tested |

Important current truth:

- `EditorPane` is a read-only preview, not a general-purpose editor.
- Human file modification is currently done through assistant tool operations, the integrated shell, or external tools opened from the shell.

### Integrated terminal support

| Capability | User entrypoint | Core files | Backend/service path | Safety model | Current status | Evidence |
| --- | --- | --- | --- | --- | --- | --- |
| PTY-backed integrated shell | Terminal pane / terminal drawer | `terminal_pane.rs`, `native_backend.rs`, `terminal_surface.rs` | `portable-pty` session managed by native backend | Local process only; no remote transport | Supported | `Flow-tested` plus backend unit coverage |
| Capture mode vs navigation mode | `Ctrl+g`, `Enter`, terminal focus rules | `terminal_pane.rs`, `app.rs` | TUI routes keys differently by mode | Navigation mode prevents accidental shell capture of global actions | Supported | `Flow-tested` via `tui_flow_tests/terminal_certification_flows.rs` |
| Resize synchronization | TUI resize / pane size changes | `terminal_pane.rs`, `native_backend.rs`, `app.rs` | `TuiToBackendRequest::TerminalResize` | Safe no-op on zero-size | Supported | `Flow-tested` via `tui_flow_tests/terminal_certification_flows.rs` |
| Focus synchronization | Terminal focus changes | `terminal_pane.rs`, `native_backend.rs`, `app.rs` | `TerminalFocusChanged { focused }` | Pure PTY metadata signal | Supported | `Flow-tested` via terminal certification flows |
| Scrollback page up/down | `Shift+PgUp`, `Shift+PgDn` | `terminal_pane.rs`, `native_backend.rs` | Scroll requests to PTY service | Separate from transcript scroll | Supported | `Flow-tested` via `tui_flow_tests/terminal_flows.rs` and `tui_flow_tests/terminal_certification_flows.rs` |
| Bracketed paste | Paste while terminal requests bracketed mode | `terminal_pane.rs`, `terminal_surface.rs` | `TerminalInput(Vec<u8>)` with bracket markers | Only enabled when terminal snapshot asks for it | Supported | `Flow-tested` via `tui_flow_tests/terminal_certification_flows.rs` |
| Alternate-screen fullscreen behavior | Fullscreen terminal apps such as `vim` | `terminal_pane.rs`, `terminal_surface.rs`, `app.rs`, `shell.rs` | Snapshot `alternate_screen` flag changes shell layout | TUI keeps footer pinned and restores layout after exit | Supported | `Flow-tested` via `tui_flow_tests/terminal_certification_flows.rs` and visual regression tests |
| Restart after shell exit | `Enter` after PTY closes | `terminal_pane.rs`, `app.rs` | Reissues resize to spawn a new PTY | Explicit user action required | Supported | `Flow-tested` via `tui_flow_tests/backend_flows.rs` |
| Metadata propagation | Current cwd, shell label, window title | `native_backend.rs`, `terminal_pane.rs` | Terminal frames carry metadata | Metadata affects labels only | Supported | `Flow-tested` via `tui_flow_tests/terminal_certification_flows.rs`; backend unit tests cover PTY metadata paths |

Additional current behavior:

- Terminal frames are rendered from a snapshot model, not directly from raw stream bytes.
- Global shortcuts are intentionally suppressed while capture mode is active, except the defined escape hatches.
- The shell drawer can be toggled independently of terminal focus with `Ctrl+\``.

### Model/runtime integration

| Capability | User entrypoint | Core files | Backend/service path | Safety model | Current status | Evidence |
| --- | --- | --- | --- | --- | --- | --- |
| Local SSD-MOE model catalog | Model picker, default model boot | `model_registry.rs` | Local catalog only | Local ids separated from provider-style ids | Supported | Implemented and unit-tested in `model_registry.rs` |
| Runtime lifecycle | App boot, model switch, stop | `ssd_moe_tui.rs`, `app.rs` | Shared runtime acquisition, polling, transitions | Runtime state visible in shell/bootstrap UI | Supported | Unit-tested heavily in `ssd_moe_tui.rs`; runtime UI partially flow-tested |
| Local SSD-MOE HTTP chat transport | Normal chat submit path with `local/...` or SSD-MOE models | `chat_service.rs`, `ssd_moe_client.rs` | `reqwest` SSE against validated loopback URL | Rejects non-loopback base URLs | Supported | `Flow-tested` via `tui_flow_tests/chat_http_mock.rs`; unit-tested in `ssd_moe_client.rs` |
| Remote OpenAI-compatible HTTP chat transport | Normal chat submit path with `openai-compatible/...` or remote provider env | `chat_service.rs`, `openai_compatible_client.rs`, `provider_config.rs` | `reqwest` SSE against normalized remote base URL with bearer auth | Managed runs reject loopback base URLs when `WARPOS_NETWORK_MODE=inspect` unless explicitly overridden | Supported | Unit-tested in `chat_service.rs` and `provider_config.rs` |
| Single-shot agent completion transport | `/agent <goal>` runtime path | `agent_runtime.rs`, `chat_service.rs` | `request_single_completion(...)` against local or remote provider config | Reuses the same provider resolution and runtime startup checks as normal chat | Supported | Unit-tested in `chat_service.rs` and `agent_runtime.rs` |
| Runtime bootstrap gating | App startup shell scene | `app.rs`, `ssd_moe_tui.rs`, `bootstrap_loader.rs` | Bootstrap scene waits on terminal/workspace/runtime gates | Startup status is visible instead of silent | Supported | `Flow-tested` for visible runtime header and terminal baselines; lifecycle details unit-tested |
| Model picker UI | `Ctrl+m`, arrows, `Enter` | `app.rs`, `models_pane.rs` | Syncs from `ChatPane` model list | Selection only; no destructive actions wired | Supported | `Flow-tested` via `tui_flow_tests/global_shortcuts.rs` and `tui_flow_tests/models_picker_flows.rs` |
| Save active local model selection | Pick local model id | `model_registry.rs`, `app.rs` | Writes `~/.config/quorp-tui/active_model.txt` | Only local SSD-MOE ids are persisted | Supported | `Flow-tested` via `tui_flow_tests/models_picker_flows.rs` |
| Cloud/provider model ids in picker | Registry-style ids such as `provider/model` | `models_pane.rs`, `app.rs` | Displayed and selectable in chat state | Not written to local SSD-MOE active model file | Supported with caveat | `Flow-tested` via `tui_flow_tests/models_picker_flows.rs` |

Important current truth:

- The native entrypoint does not pass a unified language model registry into `tui::run(...)`.
- The current native app path is still oriented around local SSD-MOE chat/runtime support.
- Registry-style `provider/model` ids are supported in UI state and tests, and the native runtime now supports local SSD-MOE, Ollama, Codex, and remote OpenAI-compatible provider routing.

### Workspace/thread persistence

| Capability | User entrypoint | Core files | Backend/service path | Safety model | Current status | Evidence |
| --- | --- | --- | --- | --- | --- | --- |
| Workspace/project state persistence | App startup and shutdown lifecycle | `workspace_state.rs`, `app.rs` | JSON state plus per-thread transcript files | Working/queued threads are downgraded to interrupted on reload | Supported | Implemented in active app path |
| Active-thread restoration | Startup into previous thread | `workspace_state.rs`, `app.rs` | `load_active_thread_snapshot()` -> `ChatPane::import_thread_snapshot(...)` | Loads persisted snapshot instead of silently discarding | Supported | Implemented in active app path |
| Thread status transitions | Chat streaming / error / idle state | `workspace_state.rs`, `app.rs` | `persist_workspace_state()` maps chat state to `ThreadStatus` | Status stored per thread | Supported | Implemented in active app path |
| New thread prompt | `Ctrl+n` or sidebar new-thread hit target | `app.rs`, `workspace_state.rs` | Creates project/thread record, resets UI root, loads new snapshot | Explicit user action | Supported | Implemented; not clearly flow-tested |
| Sidebar project activation | Mouse selection in sidebar | `app.rs`, `workspace_state.rs` | Activates project and associated thread | Explicit user action | Supported | Implemented; not clearly flow-tested |
| Sidebar thread activation | Mouse selection in sidebar | `app.rs`, `workspace_state.rs` | Activates thread and loads its snapshot | Explicit user action | Supported | Implemented; not clearly flow-tested |

### Operational limits and guardrails

The current code enforces several important limits:

| Area | Current limit / guardrail | Primary file |
| --- | --- | --- |
| Assistant message size | `512 KiB` max per message | `crates/quorp/src/quorp/tui/chat.rs` |
| Stored message count | `500` messages before old pairs are trimmed | `crates/quorp/src/quorp/tui/chat.rs` |
| Command output capture | `16 KiB` cap | `crates/quorp/src/quorp/tui/native_backend.rs` |
| `read_file` output | `64 KiB` cap and UTF-8 only | `crates/quorp/src/quorp/tui/native_backend.rs` |
| `list_directory` output | Max `512` entries, names truncated to `80` chars | `crates/quorp/src/quorp/tui/native_backend.rs` |
| Mentioned file expansion | `512 KiB` cap | `crates/quorp/src/quorp/tui/mention_links.rs` |
| Mentioned directory expansion | Max `200` entries | `crates/quorp/src/quorp/tui/mention_links.rs` |
| Path escaping | Absolute paths and `..` traversal rejected for assistant tools | `crates/quorp/src/quorp/tui/native_backend.rs` |
| Chat base URL | Local SSD-MOE must remain on loopback; remote OpenAI-compatible providers use normalized `http(s)://.../v1` URLs | `crates/quorp/src/quorp/tui/ssd_moe_client.rs`, `crates/quorp/src/quorp/provider_config.rs` |
| Autonomous host profile | Blocks high-risk shell commands and MCP tool execution | `crates/quorp/src/quorp/tui/agent_runtime.rs` |

## 4. Key Technologies and Libraries

Primary dependency anchors:

- `Cargo.toml`
- `crates/quorp/Cargo.toml`

| Technology | Where used | Why it matters |
| --- | --- | --- |
| Rust workspace and `quorp` binary | Workspace root and `crates/quorp` | Entire TUI stack, backend logic, runtime integration, and tests are native Rust |
| `ratatui` | `crates/quorp/Cargo.toml`, many `tui/*.rs` files | Core terminal UI layout, rendering, widgets, buffers |
| `crossterm` | main TUI loop, `app.rs`, panes | Terminal input events, raw mode, alternate screen management |
| `portable-pty` | `native_backend.rs` | Integrated shell PTY creation, terminal session management |
| `tokio` | `tui.rs`, `chat_service.rs`, `agent_runtime.rs`, `ssd_moe_tui.rs` | Async runtime for chat streaming and runtime coordination |
| `reqwest` | `chat_service.rs`, `ssd_moe_client.rs`, `openai_compatible_client.rs` | Local loopback SSD-MOE plus remote OpenAI-compatible chat transport and single-shot agent completion |
| SSE parsing over HTTP | `openai_compatible_client.rs`, `chat_service.rs` | Incremental assistant token and reasoning streaming for local and remote providers |
| `syntect` | `assistant_transcript.rs` | Syntax highlighting for assistant code blocks |
| `ignore` | `path_index.rs`, `mention_links.rs` | File indexing and directory expansion with standard ignore semantics |
| `notify` | `path_index.rs` | Background refresh for local project path index |
| SSD-MOE shared crates | `crates/quorp/Cargo.toml`, `ssd_moe_tui.rs` | Local model runtime discovery, broker communication, lifecycle management |
| `serde` / `serde_json` | `workspace_state.rs`, diagnostics, chat transport | Snapshot persistence, bridge payloads, diagnostics events, HTTP body construction |
| `regex` | `path_index.rs`, `mcp_client.rs` | Lightweight symbol and protocol parsing helpers |
| `toml` | `agent_context.rs`, `workspace_state.rs` | Repo-local policy/config parsing and model persistence |
| `futures` | `chat_service.rs`, `agent_runtime.rs`, `native_backend.rs` | Unbounded request streams and async event loops |
| `std::sync` primitives (`Arc`, `Mutex`, `RwLock`, `OnceLock`, atomics`) | `path_index.rs`, `native_backend.rs`, `workspace_state.rs`, `agent_runtime.rs` | Shared snapshots, rollback stash, and background worker coordination |
| `vt100` (`fnug-vt100`) | `terminal_surface.rs`, terminal stack | Terminal snapshot and escape-sequence interpretation |

Additional implementation notes:

- The TUI uses a shell-first layout model defined in `shell.rs` and `workbench.rs`.
- Visual regression and screenshot review infrastructure is built into the repo under `tui_flow_tests`, `buffer_png.rs`, and `script/tui-screenshot-suite`.
- Diagnostics and memory logging are always part of the native startup path through `diagnostics.rs` and `memory_fingerprint.rs`.

## 5. Current Gaps and Misleading Surfaces

This section focuses on places where the current code exposes an agent or coding affordance that is incomplete, misleading, or broader in appearance than the active native TUI path really supports.

### P0: Accuracy and UX gaps

#### 1. `AgentPane` is a runtime status surface, not a task editor

Evidence:

- `crates/quorp/src/quorp/tui/agent_pane.rs`
- `crates/quorp/src/quorp/tui/chat.rs`
- `crates/quorp/src/quorp/tui/agent_runtime.rs`

Current behavior:

- `AgentPane` renders `AgentUiEvent::StatusUpdate`, `TurnCompleted`, and `FatalError`.
- The actual autonomous task starts from `/agent <goal>` in `ChatPane`.
- The pane is visual only; it does not initiate or edit agent tasks directly.

Why this matters:

- The TUI now has a real autonomous background workflow, but the control surface is still the chat composer rather than the agent pane itself.

Support label:

- `Supported` via chat invocation

#### 2. `autonomous_sandboxed` exists in config/model space but is not implemented

Evidence:

- `crates/quorp/src/quorp/tui/agent_context.rs`
- `crates/quorp/src/quorp/tui/agent_runtime.rs`

Current behavior:

- The enum value exists and can be parsed from config.
- The native runtime returns an explicit error if that profile is selected.

Why this matters:

- The config surface advertises a sandboxed profile that the current native runtime cannot actually execute.

Support label:

- `Placeholder / not fully wired`

#### 3. `mcp_call_tool` is parsed, but native execution is still a placeholder

Evidence:

- `crates/quorp/src/quorp/tui/mcp_client.rs`
- `crates/quorp/src/quorp/tui/native_backend.rs`
- `crates/quorp/src/quorp/tui/agent_runtime.rs`

Current behavior:

- MCP tool requests are represented in the model and the instruction layer.
- The native backend still does not execute them as a real tool path.

Why this matters:

- This is the largest remaining gap between agent intent and real tool execution in the current worktree.

Support label:

- `Placeholder / not fully wired`

#### 4. `apply_patch` still means full replacement content

Evidence:

- `crates/quorp/src/quorp/tui/native_backend.rs`
- `crates/quorp/src/quorp/tui/chat_service.rs`

Current behavior:

- Legacy unified diff payloads are rejected.
- The legacy `apply_patch` path is still a full-file replacement operation.
- The newer structured edit path is `ReplaceBlock`.

Why this matters:

- The name `apply_patch` still suggests diff semantics that the legacy XML tag does not provide.

Support label:

- Supported with important semantic limitation

#### 5. Shortcut/help coverage is still inconsistent in places

Evidence:

- `crates/quorp/src/quorp/tui/app.rs`
- `crates/quorp/src/quorp/tui/action_discovery.rs`
- `crates/quorp/src/quorp/tui/shell.rs`

Current behavior:

- `Ctrl+n` opens the new-thread prompt.
- Action discovery and shell hints do not always make the split between chat sessions and persisted workspace threads equally clear.

Why this matters:

- The app has both ephemeral chat session concepts and persisted thread concepts, and the help surface still under-explains that distinction.

Support label:

- Supported behavior with partially misleading help surface

### P1: Implemented but not fully flow-tested or not default-native

#### 6. Quick open is implemented but not clearly flow-tested end-to-end

Evidence:

- `crates/quorp/src/quorp/tui/app.rs`

Current behavior:

- `Ctrl+p` opens `Overlay::QuickOpen`.
- Selection opens a file in the preview pane.

Gap:

- No clear `tui_flow_tests` coverage was found for the full quick-open path.

Support label:

- `Implemented`

#### 7. New-thread prompt and sidebar project/thread activation are implemented but not clearly flow-tested

Evidence:

- `crates/quorp/src/quorp/tui/app.rs`
- `crates/quorp/src/quorp/tui/workspace_state.rs`

Current behavior:

- `Ctrl+n` opens the new-thread chooser.
- Sidebar hit targets can activate projects and threads.

Gap:

- No clear TUI flow tests were found for creating a new thread through the chooser or switching projects/threads from the sidebar.

Support label:

- `Implemented`

#### 8. Assistant feed link opening is implemented but not clearly flow-tested

Evidence:

- `crates/quorp/src/quorp/tui/app.rs`

Current behavior:

- Mouse and keyboard paths exist to open assistant feed links.
- The implementation shells out to `open`, `xdg-open`, or `cmd /C start`.

Gap:

- No clear TUI flow test was found for the actual link-open behavior.

Support label:

- `Implemented`

#### 9. Snapshot-backed indexing exists, but it is not the default native app path

Evidence:

- `crates/quorp/src/quorp/tui/path_index.rs`
- `crates/quorp/src/quorp/tui/app.rs`
- `crates/quorp/src/quorp/tui/native_backend.rs`

Current behavior:

- `PathIndex::new_project_backed(...)` and `apply_bridge_snapshot(...)` exist.
- The default native path created by `main.rs` and `tui::run(...)` uses the local index walker.
- The native backend loop does not emit `PathIndexSnapshot`.

Why this matters:

- Snapshot-backed indexing is real and tested, but it is not the normal native mode a user gets from the current app entrypoint.

Support label:

- Supported alternate mode, not default native path

#### 10. The TUI still lacks several higher-level autonomous features discussed in earlier plans

Current gaps that are not present in the native TUI path yet:

- No LSP-backed semantic tool layer
- No sandboxed autonomous execution profile
- No external evaluation harness binary for benchmarking agent runs
- No true memory store or vector recall system for autonomous trace compression
- No diff-native patch application API beyond the current structured `ReplaceBlock` and legacy `apply_patch` behavior

Support label:

- Not present in the current TUI

## 6. Recent Tests and Evidence Files

Primary TUI flow-test references used by this document:

- `crates/quorp/src/quorp/tui/tui_flow_tests/backend_flows.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/chat_flows.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/chat_http_mock.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/editor_pane_flows.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/file_tree_flows.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/global_shortcuts.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/mention_flows.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/models_picker_flows.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/mouse_flows.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/navigation_flows.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/rollback_flows.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/rust_capture_flows.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/screenshot_suite.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/session_isolation_flows.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/tab_strip_flows.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/terminal_certification_flows.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/terminal_flows.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/visual_flows.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/visual_regression.rs`
- `crates/quorp/src/quorp/tui/tui_flow_tests/vim_navigation_matrix.rs`

Primary unit-test-heavy implementation files used by this document:

- `crates/quorp/src/quorp/tui/app.rs`
- `crates/quorp/src/quorp/tui/agent_context.rs`
- `crates/quorp/src/quorp/tui/agent_protocol.rs`
- `crates/quorp/src/quorp/tui/agent_runtime.rs`
- `crates/quorp/src/quorp/tui/agent_turn.rs`
- `crates/quorp/src/quorp/tui/assistant_transcript.rs`
- `crates/quorp/src/quorp/tui/chat.rs`
- `crates/quorp/src/quorp/tui/chat_service.rs`
- `crates/quorp/src/quorp/tui/mcp_client.rs`
- `crates/quorp/src/quorp/tui/native_backend.rs`
- `crates/quorp/src/quorp/tui/mention_links.rs`
- `crates/quorp/src/quorp/tui/path_index.rs`
- `crates/quorp/src/quorp/tui/ssd_moe_client.rs`
- `crates/quorp/src/quorp/tui/openai_compatible_client.rs`
- `crates/quorp/src/quorp/provider_config.rs`
- `crates/quorp/src/quorp/tui/ssd_moe_tui.rs`

Specific recent runtime tests that now exist:

- `agent_runtime.rs`
  - `high_risk_shell_commands_are_detected`
  - `interactive_profile_blocks_write_actions`
  - `observe_outcome_formats_prunable_tool_output`
  - `post_edit_validation_queues_fast_and_full_when_configured`
  - `state_requires_green_validation_after_mutation`
  - `wait_for_tool_result_returns_cancelled`
- `app.rs`
  - `apply_tui_backend_event_handles_agent_runtime_event`
  - `apply_tui_backend_event_routes_start_agent_task_into_runtime`
  - `runtime_session_command_finished_is_routed_to_agent_runtime`
- `chat_service.rs`
  - `compact_transcript_prunes_structured_tool_results`
- `agent_context.rs`
  - `load_agent_config_parses_defaults_validation_and_rules`
  - `validation_commands_expand_from_plan`
- `agent_turn.rs`
  - `parses_raw_json_turn`
  - `parses_fenced_json_turn`
  - `ignores_plain_text_without_json`
  - `render_includes_action_receipts_and_verifier_summary`
- `native_backend.rs`
  - `apply_patch_task_performs_full_replacement`
  - `apply_patch_task_rejects_unified_diff_payload`
  - `test_try_parse_search_replace_blocks`
  - `test_perform_block_replacement_exact_match`
  - `test_perform_block_replacement_ambiguous`

Current config evidence:

- `.quorp/agent.toml` now sets:
  - `defaults.mode = "act"`
  - `autonomy.profile = "autonomous_host"`
  - `validation.workspace_test_command = "cargo test -p quorp"`
  - `validation.targeted_test_prefix = "cargo test -p quorp"`
- `run_validation` approval rules now auto-approve the configured `fmt`, `clippy`, and `cargo test -p quorp` commands.

## 7. Recommendations

These are the highest-value next gaps from the current worktree:

1. Wire a real MCP execution path if autonomous tool execution is expected to use external services.
2. Decide whether `apply_patch` should remain a legacy full-replacement tag or be renamed to match its real semantics.
3. Add flow tests for quick open, new-thread activation, and link opening if those paths remain user-facing.
4. If sandboxed autonomy is a goal, implement it explicitly instead of leaving `AutonomousSandboxed` as a config-only marker.
5. If agent quality is the next major focus, the biggest missing pieces are LSP-backed semantics, deeper repository intelligence, and a benchmark harness for autonomous runs.

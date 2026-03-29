# TUI Phase 2 seam implementation plan (started March 29, 2026)

Phase 2 objective: complete backend seam extraction for all TUI verticals so TUI modules no longer depend on GPUI runtime/entity types outside adapter boundaries.

## Scope for Phase 2

- Path index and project-backed file discovery
- Integrated terminal session lifecycle and stream I/O
- Chat/model request orchestration and streamed events
- Command execution bridge used by TUI actions

## Required seam interfaces

## 1) Path index seam

### Contract

- `request_path_snapshot(root, options) -> PathSnapshot`
- `request_search(query, scope) -> PathSearchResult`

### TUI modules to migrate

- file tree loaders and directory expansion flows
- mention/file lookup paths currently reading project-backed state indirectly

### Tests required

- unit test: path snapshot success populates tree model
- unit test: backend failure surfaces actionable error in TUI state
- flow test: mention lookup path remains functional with seam backend

## 2) Terminal seam

### Contract

- `create_terminal_session(config) -> TerminalSessionId`
- `resize_terminal_session(session_id, rows, columns)`
- `send_terminal_input(session_id, bytes)`
- `subscribe_terminal_frames(session_id) -> Stream<TerminalFrame>`
- `close_terminal_session(session_id)`

### TUI modules to migrate

- terminal pane input handling
- pane resize handling
- frame rendering update path

### Tests required

- unit test: create+resize+input sequence sends expected backend requests
- unit test: stream disconnect sets disconnected/error state in terminal pane
- flow test: terminal pane smoke with frame updates and input echo path

## 3) Chat/model seam

### Contract

- `submit_chat_turn(request) -> ChatRequestId`
- `stream_chat_events(request_id) -> Stream<ChatEvent>`
- `cancel_chat_turn(request_id)`
- `list_models() -> Vec<ModelDescriptor>`

### TUI modules to migrate

- agent/chat pane send path
- streaming response aggregation path
- model picker loading/select path

### Tests required

- unit test: submit dispatches request with current context attachments
- unit test: stream error sets retryable user-visible state
- flow test: chat round-trip with incremental streaming events

## 4) Command execution seam

### Contract

- `execute_command(command_request) -> CommandExecutionId`
- `stream_command_events(execution_id) -> Stream<CommandEvent>`
- `cancel_command(execution_id)`

### TUI modules to migrate

- action dispatch path in TUI app
- command status updates and event application path

### Tests required

- unit test: command dispatch routes through seam backend
- unit test: cancel path sends cancellation to backend
- flow test: command execution updates activity/status surfaces

## Cross-cutting constraints

- Keep GPUI references isolated inside adapter implementations only.
- TUI modules under `crates/quorp/src/quorp/tui/**` should consume seam traits and plain Rust domain types.
- Errors from seam calls must propagate to user-visible TUI state.

## Implementation order

1. Path index seam (lowest integration risk and reused by file tree/mentions).
2. Terminal seam (high user impact and independent verification surface).
3. Chat/model seam (streaming + cancellation correctness).
4. Command seam (final action dispatch normalization).

## Phase 2 verification protocol

After each seam batch:

1. Run `cargo test -p quorp` for seam-specific unit tests added in that batch.
2. Run `cargo test -p quorp tui_flow_tests`.
3. Run `./script/tui-verify`.

Do not proceed to the next seam until all three pass for the current seam batch.

## Phase 2 exit criteria

- All four seam families are implemented and consumed by TUI modules.
- No direct GPUI runtime/entity usage in TUI modules outside adapter boundaries.
- `./script/tui-verify` passes all stages with seam-based TUI code paths.

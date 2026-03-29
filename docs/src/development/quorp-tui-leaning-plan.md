# Quorp TUI Leaning Plan

## Goal

Trim non-essential complexity while preserving world-class TUI capabilities:

- file tree and project navigation
- `@file` lookup and mention insertion in chat
- integrated LLM workflows (streaming, model selection, tool calling)
- terminal + command execution + git-aware flows

## Constraints

- Keep all production bridge paths intact (`path_index_bridge`, `command_bridge`, `bridge`).
- Do not remove functionality if a TUI flow test depends on it.
- Treat GPUI integration as backend plumbing; keep it where needed for data and model wiring.

## Phase 0 — Safety Net (completed)

1. Ensure Rust workspace manifests parse reliably (`cargo metadata` must pass).
2. Keep TUI flow tests as the primary regression harness.
3. Add deterministic screenshot/export tests for core states (default/help/model picker).

## Phase 1 — Inventory and classify bloat

### 1.1 TUI runtime modules

Create a module map in `crates/quorp/src/quorp/tui/` and classify each file as:

- **Core runtime** (must keep)
- **Test-only / visual scoring** (keep but isolate)
- **Legacy compatibility path** (candidate cleanup)

### 1.2 Critical capability matrix

Map each capability to code paths + tests:

- file tree list/open/selection
- chat submit/stream/model picker
- mention popup/filter/insert
- terminal pane + bridge updates
- agent pane dispatch + status updates
- backend event application in `TuiApp`

If a capability has no direct flow test, add one before refactoring.

## Phase 2 — Refactor for lean runtime

### 2.1 Flatten optional visual polish paths

Keep the minimal default draw path hot:

- title bar
- activity + explorer
- workbench panes
- status bar

Move optional visual-regression-specific setup behind explicit helpers (already partly done in `new_for_prismforge_regression`).

### 2.2 Isolate compatibility helpers

Keep compatibility/heuristics utilities (e.g. image likeness scoring) separate from runtime-critical code paths so they can be changed without touching the interactive TUI loop.

### 2.3 Keep backend bridges explicit

Avoid hidden behavior:

- all `TuiEvent` -> UI updates should stay in one place (`apply_tui_backend_event`)
- bridge senders should be optional but explicit in constructors

## Phase 3 — Test hardening and support

### 3.1 Core flow suite

Run and maintain these suites together:

- `tui_flow_tests::navigation_flows`
- `tui_flow_tests::file_tree_flows`
- `tui_flow_tests::mention_flows`
- `tui_flow_tests::chat_flows`
- `tui_flow_tests::terminal_flows`
- `tui_flow_tests::backend_flows`
- `tui_flow_tests::rust_capture_flows`

### 3.2 Screenshot support

Use Rust-only screenshot capture through `TuiTestHarness::save_screenshot` to avoid external browser dependencies for TUI images.

### 3.3 Workspace support

Keep manifest inheritance repairable:

- regenerate root workspace dependency table with `script/sync-workspace-deps.py`
- verify parse with `cargo metadata --no-deps`

## Phase 4 — Known risks and mitigations

1. **Risk:** Removing apparently “GUI-only” code breaks backend wiring.
   - **Mitigation:** keep bridge modules and integration tests first-class; remove only after a passing flow test replacement.

2. **Risk:** Optional model/provider paths regress silently.
   - **Mitigation:** preserve model-picker and streaming tests with multiple model IDs.

3. **Risk:** Refactors break deterministic screenshots.
   - **Mitigation:** run visual regression + rust capture flows per refactor batch.

4. **Risk:** Workspace drift blocks builds again.
   - **Mitigation:** keep dependency sync script and metadata check in pre-merge verification.

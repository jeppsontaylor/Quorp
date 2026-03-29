# TUI-first dependency gut audit and final plan

## Why `crates/gpui` is still present today

Short answer: the current TUI path still uses GPUI as a backend runtime and entity graph, even though rendering is done with ratatui/crossterm.

Concrete coupling points:

- `main.rs` always boots a GPUI `Application` and uses it to initialize global state and async orchestration before starting TUI flow. It calls `Application::with_platform(gpui_platform::current_platform(tui_mode))`, `app.run(...)`, and `gpui_tokio::init(cx)`.  
- TUI bridge/runtime code directly uses GPUI types (`gpui::App`, `gpui::AsyncApp`, `gpui::Entity`, `gpui::Task`) in `bridge.rs`, `path_index_bridge.rs`, `command_bridge.rs`, and `tui_tool_runtime.rs`.
- Integrated terminal and project/worktree interactions in TUI are currently wired through existing GPUI-backed editor services rather than a separate non-GPUI service layer.

So today, removing `gpui` immediately would break TUI backend behavior, not just GUI behavior.

## What can be safely gutted now (low risk)

These are safe candidates because they are GUI-facing and not required for ratatui rendering or TUI interaction loops:

1. **Visual test runner path from regular TUI workflows**
   - Keep `quorp_visual_test_runner` behind `visual-tests` only (already done), and avoid enabling that feature in TUI CI.
2. **GUI-only verification from TUI pipelines**
   - Build/test only TUI targets in CI (`quorp -- --tui` paths + `tui_flow_tests`) and stop treating GUI smoke checks as blockers.
3. **Non-TUI UI crates from immediate dependency closure**
   - Audit direct dependencies in `crates/quorp/Cargo.toml` and move obviously GUI-only crates behind a future `gui` feature gate.

## What should NOT be gutted yet (high break risk)

Do not remove these until replacement abstractions exist:

- `gpui`, `gpui_platform`, `gpui_tokio` from `crates/quorp`.
- TUI bridge files that call GPUI entity APIs directly.
- Project/worktree/session services currently resolved through GPUI-owned state and contexts.

## Final plan to reach a healthy, fully-working TUI-first product

### Phase 0 (stability baseline)

- Freeze current behavior with explicit TUI acceptance checks:
  - startup and shutdown terminal restore behavior,
  - file tree navigation,
  - code preview rendering,
  - integrated terminal spawn/resize/input,
  - chat request/response event flow.
- Keep GPUI in place while the behavior baseline is locked.

### Phase 1 (seam extraction: decouple TUI from GPUI APIs)

- Introduce a `TuiBackend` trait boundary for services TUI actually needs:
  - project/worktree navigation,
  - file read/search/index,
  - integrated terminal session I/O,
  - command execution,
  - model/chat request orchestration.
- Implement adapter #1: `GpuiTuiBackend` (wrap existing GPUI entities).
- Refactor TUI modules to depend on `TuiBackend` instead of `gpui::*` types.

Exit criterion: no direct `gpui::*` usage in `crates/quorp/src/quorp/tui/**` except inside adapter implementation.

### Phase 2 (runtime split)

- Add a dedicated TUI binary/runtime path that does **not** construct GPUI `Application`.
- Keep existing binary behavior behind a `gui` feature/path until parity is proven.
- Route TUI startup through Tokio/native async services directly.

Exit criterion: TUI launches and all Phase 0 acceptance checks pass with GPUI runtime disabled.

### Phase 3 (dependency gut)

- Remove GPUI crates from TUI build graph.
- Move GUI-only crates and code behind `gui` feature or separate crate.
- Make `tui` the default build flavor for this repo branch.

Exit criterion: `cargo build -p quorp --no-default-features --features tui` (or equivalent final feature contract) succeeds without GPUI dependencies.

### Phase 4 (hardening and cleanup)

- Delete now-unused bridge shims and GPUI adapter code.
- Simplify settings/bootstrap to TUI-first defaults.
- Finalize CI matrix: TUI required, GUI optional/legacy lane.

## Immediate next execution order

1. Add dependency inventory labeling each `crates/quorp` dependency as `tui-required`, `backend-shared`, or `gui-only`.
2. Introduce `TuiBackend` trait and one thin `GpuiTuiBackend` adapter.
3. Migrate one vertical slice end-to-end first: file tree + preview read path.
4. Migrate integrated terminal path.
5. Migrate chat/model path.
6. Switch TUI startup to non-GPUI runtime path.
7. Remove GPUI from TUI dependency graph and clean dead code.

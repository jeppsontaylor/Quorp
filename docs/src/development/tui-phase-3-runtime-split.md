# TUI Phase 3 runtime split plan (started March 29, 2026)

Phase 3 objective: run TUI through a dedicated non-GPUI startup/runtime path while preserving backend behavior parity.

## Runtime split target

- Add a TUI startup path that does not construct `gpui::Application`.
- Keep GPUI-backed startup as a temporary compatibility lane.
- Validate both lanes until parity is demonstrated.

## Deliverables

## 1) Startup entrypoints

- Introduce an explicit runtime selector with two startup lanes:
  - `tui_runtime = gpui_adapter` (compatibility lane)
  - `tui_runtime = native_async` (target lane)
- Ensure flag/config contract is deterministic and documented.

### Required checks

- startup smoke test for each runtime lane
- terminal restore behavior verified in both lanes

## 2) Service bootstrap split

- Move TUI service initialization out of GPUI-only bootstrap paths.
- Build backend service graph for native async lane from seam adapters introduced in Phase 2.
- Keep adapter injection explicit and testable.

### Required checks

- unit test: runtime selector chooses expected bootstrap path
- unit test: missing service adapter fails with clear startup error

## 3) Event loop and task ownership

- Ensure task lifetimes are owned by the active runtime lane.
- Keep cancellation and shutdown order deterministic.
- Verify no hidden dependency on GPUI scheduler from TUI runtime path.

### Required checks

- shutdown test: all spawned runtime tasks terminate cleanly
- failure test: startup failure path restores terminal state

## 4) CI lane strategy

- Add two explicit CI lanes during transition:
  - `tui-gpui-compat` (temporary)
  - `tui-native-runtime` (target)
- Keep both required until parity matrix is complete.

### Required checks

- both lanes run seam unit tests and `tui_flow_tests`
- both lanes run `./script/tui-verify` (or equivalent lane-specific wrapper)

## Feature parity matrix for Phase 3 signoff

Parity must be shown for both runtime lanes before deprecating GPUI-backed TUI startup:

- file tree navigation
- editor preview/open behavior
- terminal create/resize/input/output
- chat submit/stream/cancel
- command execution and status updates

## Phase 3 verification protocol

For each runtime-split batch:

1. Run runtime-selector unit tests.
2. Run seam-focused unit tests changed in the batch.
3. Run `cargo test -p quorp tui_flow_tests` in each runtime lane.
4. Run startup smoke tests in each runtime lane.
5. Run `./script/tui-verify`.

Do not remove GPUI-backed TUI startup until all parity checks pass in both lanes.

## Phase 3 exit criteria

- Native async runtime lane runs TUI without constructing `gpui::Application`.
- Parity matrix is complete and green for both runtime lanes.
- GPUI-backed startup is marked legacy for TUI and ready for Phase 4 deprecation work.

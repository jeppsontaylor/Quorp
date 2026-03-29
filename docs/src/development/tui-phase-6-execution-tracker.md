# TUI Phase 6 execution tracker (started March 29, 2026)

Phase 6 objective: convert Phase 0–5 planning artifacts into an implementation-ready backlog with explicit ownership, sequencing, and merge gates.

## Scope

- implementation ticketization across seams and runtime split
- merge sequencing to avoid long-running divergence
- explicit definition-of-done per work package

## Work package template

Each work package should include:

- **ID** (for example, `TUI-P6-001`)
- **Scope** (single vertical or single runtime concern)
- **Dependencies** (prerequisite packages)
- **Code touchpoints** (expected file/module targets)
- **Required tests** (unit + flow + verification script)
- **Rollback plan** (how to revert safely)
- **Owner** and **target merge window**

## Initial Phase 6 work packages

## TUI-P6-001 — Stage A dependency edge proof pass

- Confirm dependency edges for all `unknown` entries from Phase 1 inventory.
- Gate or defer `legacy-gui` dependencies from TUI-targeted compile path.
- Exit: `stage-a-next-blocker` points only to proven `tui-critical` or `backend-shared` crates.

## TUI-P6-002 — Path index seam implementation batch

- Implement Phase 2 path index seam contract and migrate first consumers.
- Add unit tests for success and failure propagation.
- Exit: path index flows are seam-backed and passing flow tests.

## TUI-P6-003 — Terminal seam implementation batch

- Implement terminal lifecycle seam and migrate terminal pane interaction path.
- Add tests for create/resize/input/output and disconnect behavior.
- Exit: terminal pane behavior passes seam and flow checks.

## TUI-P6-004 — Chat/model seam implementation batch

- Implement chat submit/stream/cancel/model-list seams.
- Migrate chat pane and model picker paths.
- Exit: streaming and cancellation paths pass seam tests and flow tests.

## TUI-P6-005 — Command seam implementation batch

- Implement command execute/stream/cancel seam contracts.
- Migrate TUI action dispatch and command status updates.
- Exit: command dispatch path is seam-backed and tested.

## TUI-P6-006 — Runtime split native lane enablement

- Introduce native async runtime lane with explicit runtime selector.
- Keep GPUI compatibility lane until parity evidence is complete.
- Exit: native lane startup smoke and parity matrix pass.

## TUI-P6-007 — Legacy lane deprecation and cleanup

- Remove or disable GPUI-backed TUI startup path after signoff.
- Remove obsolete compatibility shims and dead dependency edges.
- Exit: TUI-targeted build runs without GPUI runtime dependency.

## Merge and release policy for Phase 6

- Merge only one work package per vertical at a time.
- Require green `./script/tui-verify` before merging seam-affecting packages.
- Require updated artifact docs (Phase 1–5) in the same PR when scope changes.
- Keep PRs focused to one work package unless explicitly approved.

## Phase 6 verification protocol

For every Phase 6 package:

1. Run package-specific unit tests.
2. Run `cargo test -p quorp tui_flow_tests`.
3. Run `./script/tui-verify`.
4. Re-run `./script/stage-a-next-blocker` if dependency graph changed.
5. Record results in the package checklist.

## Phase 6 exit criteria

- All Phase 2 seam packages are merged and verified.
- Runtime split package is merged with parity evidence.
- Cleanup package is merged and TUI-first release gates are active.

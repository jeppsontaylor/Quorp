# TUI full-backend parity plan

Goal: ship a fully functional TUI app that preserves all backend capabilities while gradually removing GPUI runtime coupling.

## Guardrails

- Preserve backend capabilities: project/worktree operations, integrated terminal, command execution, language-model/chat orchestration.
- Prefer seam extraction over rewrites.
- Keep each migration step verifiable with explicit checks.

## Next logic stages

### Stage A — Build/test reliability baseline

1. Resolve workspace dependency blockers that prevent `cargo check -p quorp --bin quorp`.
2. Keep a deterministic verification entrypoint (`script/tui-verify`) to gate every follow-up refactor.

Verification gate:

- `./script/tui-verify`

### Stage B — Backend seams for all TUI verticals

1. Keep `TuiBackend` for file tree + editor preview.
2. Add `PathIndexBackend` seam for project-backed path snapshots.
3. Add `TerminalBackend` seam for PTY lifecycle and frame streaming.
4. Add `ChatBackend` seam for model streaming and tool execution.

Verification gate:

- Targeted seam unit tests for each pane/vertical.
- Existing flow tests (`tui_flow_tests`) remain green.

### Stage C — Runtime split (without backend regressions)

1. Introduce a TUI-only startup path that does not construct `gpui::Application`.
2. Keep a GPUI-backed adapter implementation initially, then add non-GPUI adapter implementations.
3. Run both startup paths in CI until parity is proven.

Verification gate:

- TUI startup smoke tests for both runtime paths.
- Feature parity matrix (file tree, preview, terminal, chat, commands).

### Stage D — Deprecate GPUI-dependent path for TUI runtime

1. Mark GPUI runtime path as legacy for TUI once parity is confirmed.
2. Remove direct GPUI usage from `crates/quorp/src/quorp/tui/**` except compatibility shims.
3. Remove compatibility shims after one full release cycle of stable TUI-only runtime.

Verification gate:

- `./script/tui-verify`
- Release-parity signoff checklist for backend functionality.

## Why this is safest

This path keeps backend behavior intact while reducing risk: each migration step has a hard verification gate before moving to the next stage.

## Stage A blocker workflow (repeatable)

1. Run `./script/stage-a-next-blocker`.
2. Resolve exactly one missing dependency blocker (prefer local path crates for unavailable private/legacy package names).
3. Re-run `./script/stage-a-next-blocker`.
4. Repeat until `cargo check -p quorp --bin quorp` succeeds, then run `./script/tui-verify`.

This keeps Stage A deterministic and prevents hidden dependency regressions from surprising later migration phases.

## Stage A reset strategy (to avoid blocker whack-a-mole)

If `./script/stage-a-next-blocker` keeps surfacing long tails of missing GUI/editor crates, stop and run `./script/stage-a-audit`.

Then split work into two tracks:

1. **TUI-critical track (required now)**: dependencies required by TUI panes/runtime and backend services.
2. **Legacy-GUI track (defer)**: dependencies only needed for non-TUI UI surfaces.

Goal: complete TUI-critical track first so `./script/tui-verify` can reach Stage 2/3, then return to GUI-only blockers later.

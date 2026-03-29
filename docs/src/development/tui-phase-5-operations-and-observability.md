# TUI Phase 5 operations and observability plan (started March 29, 2026)

Phase 5 objective: harden the fully migrated TUI runtime for day-2 operations, regression prevention, and release confidence.

## Scope

- operational telemetry and runtime health signals for TUI lanes
- crash and error triage readiness specific to TUI flows
- release governance after GPUI-dependent TUI paths are removed

## Deliverables

## 1) Runtime health instrumentation

- Define TUI runtime health events for startup, shutdown, backend connectivity, and stream failures.
- Capture terminal/session lifecycle metrics (create, resize, close, failure counts).
- Capture chat/model stream success/error/cancellation counters.

### Required checks

- unit tests for health-event emission on success and failure transitions
- smoke checks verify startup and shutdown emit expected health markers

## 2) Crash triage readiness

- Map top TUI failure domains to crash investigation prompts and runbooks.
- Ensure crash metadata includes runtime lane and active seam identifiers.
- Add playbook steps for immediate rollback or feature-flag mitigation.

### Required checks

- synthetic failure tests confirm crash context includes runtime lane and seam data
- runbook dry-run for one startup failure and one stream failure scenario

## 3) Regression prevention gates

- Freeze high-value TUI acceptance scenarios as release-blocking checks.
- Keep seam contract tests as API stability gates.
- Add dependency drift detection for TUI-targeted build graph.

### Required checks

- CI gate fails on seam contract breaking changes without corresponding test updates
- dependency drift check fails when GUI-only crates leak into TUI-targeted graph

## 4) Release and rollback policy

- Define release checklist for TUI-first rollout cohorts.
- Define rollback triggers and decision thresholds.
- Document supported configuration matrix and known limitations.

### Required checks

- release checklist exercised in a staging rehearsal
- rollback drill executed with documented time-to-recovery target

## Phase 5 verification protocol

For each operational hardening batch:

1. Run seam and flow tests for impacted verticals.
2. Run `./script/tui-verify`.
3. Run observability-focused unit/integration checks for changed runtime signals.
4. Run crash-context synthetic scenarios.
5. Verify dependency drift checks for TUI-targeted build graph.

## Phase 5 exit criteria

- TUI runtime has actionable observability and crash-triage context.
- release and rollback policy is validated in staging rehearsal.
- regression gates prevent seam drift and GUI dependency re-introduction.

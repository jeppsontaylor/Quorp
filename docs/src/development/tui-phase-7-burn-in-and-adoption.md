# TUI Phase 7 burn-in and adoption plan (started March 29, 2026)

Phase 7 objective: run a structured burn-in period for the TUI-first runtime, validate real-world reliability, and complete team adoption.

## Scope

- burn-in monitoring under realistic workloads
- migration of contributor workflows to TUI-first defaults
- final retirement of temporary migration controls

## Deliverables

## 1) Burn-in cohorts and duration

- Define cohort windows (internal dogfood, preview cohort, broad rollout).
- Define minimum burn-in durations and success thresholds per cohort.
- Track incidents by severity and rollback impact.

### Required checks

- cohort reports include startup success rate and crash rate trends
- no unresolved Sev-1/Sev-2 incidents at cohort graduation

## 2) Reliability scorecard

- Establish scorecard metrics:
  - startup success rate
  - median startup latency
  - terminal session stability
  - chat stream completion rate
  - command execution failure rate
- Compare against pre-migration baselines.

### Required checks

- scorecard generated for each cohort window
- regressions above threshold require hold-and-fix before rollout expansion

## 3) Developer workflow adoption

- Update contributor onboarding docs for TUI-first development path.
- Ensure default local verification commands emphasize TUI gates.
- Remove legacy references that imply GPUI runtime is primary for TUI.

### Required checks

- onboarding docs validated in a fresh environment walkthrough
- developer verification checklist includes `./script/tui-verify` as required step

## 4) Migration control retirement

- Remove temporary rollout flags that are no longer needed.
- Archive migration-only docs into historical appendix once stable.
- Keep incident runbooks and operational dashboards current.

### Required checks

- config audit confirms retired flags are not referenced in active paths
- docs audit confirms canonical guidance points to stable TUI-first flow

## Phase 7 verification protocol

For each cohort milestone:

1. Run TUI verification gates (`./script/tui-verify` and flow tests).
2. Generate reliability scorecard for milestone window.
3. Review incident summaries and confirm graduation criteria.
4. Validate onboarding and release docs reflect current defaults.
5. Approve expansion or hold rollout based on evidence.

## Phase 7 exit criteria

- Burn-in cohorts complete with reliability thresholds met.
- TUI-first workflow is default in contributor and release documentation.
- Temporary migration controls are retired and historical artifacts are archived.

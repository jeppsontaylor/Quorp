# TUI Phase 8 steady-state governance plan (started March 29, 2026)

Phase 8 objective: institutionalize long-term governance so TUI-first architecture remains stable, maintainable, and resistant to regression drift.

## Scope

- architectural governance for seam boundaries
- dependency policy enforcement for TUI-first build graph
- periodic health reviews and roadmap refresh cycles

## Deliverables

## 1) Architecture governance cadence

- Establish quarterly architecture review for TUI runtime and seam boundaries.
- Require ADR updates when seam contracts change.
- Track and retire technical debt items introduced during migration.

### Required checks

- architecture review notes stored for each quarter
- seam contract changes reference ADR IDs in PR descriptions

## 2) Dependency policy guardrails

- Maintain allowlist/denylist rules for TUI-targeted dependencies.
- Enforce policy checks in CI to prevent GUI-only dependency leakage.
- Define escalation path when policy exceptions are necessary.

### Required checks

- CI fails on dependency policy violations in TUI-targeted graph
- approved exceptions include expiration date and owner

## 3) Test suite stewardship

- Curate a stable core TUI suite (seam unit tests + `tui_flow_tests` + `tui-verify`).
- Track flaky tests and enforce remediation SLAs.
- Rotate ownership for critical test infrastructure.

### Required checks

- weekly flake report with aging and remediation status
- no critical TUI test remains flaky beyond SLA window

## 4) Release governance and change management

- Maintain release risk rubric for TUI runtime and seam changes.
- Require change impact assessment for runtime or seam contract modifications.
- Keep rollback guides synchronized with current runtime architecture.

### Required checks

- release notes classify TUI-risk level for each relevant change
- rollback guides validated at least once per release cycle

## Phase 8 verification protocol

For each governance cycle:

1. Run core TUI verification gates (`./script/tui-verify`, `tui_flow_tests`).
2. Run dependency policy checks and review exceptions.
3. Review architecture/seam change log and ADR updates.
4. Review flaky-test reports and SLA compliance.
5. Approve governance report with assigned follow-ups.

## Phase 8 exit criteria

- Governance cadences are operational for architecture, dependency policy, and test stewardship.
- TUI-first dependency and release guardrails are continuously enforced.
- Long-term maintenance ownership is established with recurring review cycles.

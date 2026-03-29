# TUI Phase 4 deprecation and cleanup plan (started March 29, 2026)

Phase 4 objective: remove legacy GPUI-dependent TUI runtime paths and finalize a stable TUI-first delivery posture.

## Scope

- Deprecate GPUI-backed startup lane for TUI after Phase 3 parity signoff
- Remove obsolete compatibility shims and dead bridge glue
- Finalize CI/release gates with TUI as required path

## Deliverables

## 1) Legacy path deprecation

- Mark GPUI-backed TUI startup as legacy in docs and runtime selection help text.
- Add explicit deprecation window and removal target milestone.
- Keep fallback path available only during the deprecation window.

### Required checks

- startup tests cover both lanes while deprecation window is active
- deprecation warnings are visible in logs/help output for legacy lane usage

## 2) Compatibility shim removal

- Remove GPUI-specific shim code that is no longer required by TUI modules.
- Delete dead adapter wiring replaced by native async runtime services.
- Keep seams stable while removing only unused code paths.

### Required checks

- compile with deprecation feature flags disabled
- no references to removed shims in TUI modules and tests

## 3) Dependency graph cleanup

- Remove GPUI crates from the TUI-targeted dependency graph.
- Move GUI-only crates behind explicit GUI feature gates or isolated crates.
- Ensure TUI-targeted build path does not require GUI-only dependencies.

### Required checks

- TUI-targeted `cargo check` succeeds without GPUI runtime dependencies
- dependency audit confirms GUI-only crates are excluded from TUI path

## 4) CI and release hardening

- Require TUI lane in CI as non-optional gate.
- Keep GUI lane optional/legacy until formally removed.
- Update release signoff checklist to include TUI-first parity assertions.

### Required checks

- CI matrix includes required TUI compile + seam tests + flow tests
- release checklist references TUI parity matrix and runtime-lane status

## Phase 4 verification protocol

For each cleanup batch:

1. Run TUI-targeted `cargo check` and seam unit tests.
2. Run `cargo test -p quorp tui_flow_tests`.
3. Run `./script/tui-verify`.
4. Validate dependency graph expectations for TUI-targeted build.
5. Confirm no regressions in startup/shutdown terminal behavior.

## Phase 4 exit criteria

- GPUI-backed startup path removed (or fully disabled) for TUI runtime.
- TUI-targeted build and verification pass without GPUI runtime dependencies.
- CI/release process treats TUI lane as the primary required path.

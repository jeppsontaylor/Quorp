# Issue Context

## User Story
Release teams want cargo-dist to generate CI, build artifacts, and keep the release pipeline intact while optionally skipping GitHub release creation itself.

## Actual Failure
The existing workflow assumes a GitHub release object is always created, so there is no supported config path for "do everything except create the release."

## Expected Behavior
A config flag should disable release creation while preserving the rest of the generated CI and release task graph.

## Why This Repo Cares
- This is a workflow-plumbing change, not just a boolean in config.
- The integration test uses a snapshot-backed gallery project, so the config layer, task graph, CI template, init flow, and docs must all stay in sync.

## Likely Surfaces
- `cargo-dist/src/config.rs`
- `cargo-dist/src/tasks.rs`
- `cargo-dist/src/backend/ci/github.rs`
- `cargo-dist/templates/ci/github_ci.yml.j2`
- `cargo-dist/tests/integration-tests.rs`

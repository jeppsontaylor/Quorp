# Objective

Fix `axodotdev__cargo-dist-367` in the local `proof-full` workspace.

The release planning flow needs a config switch that skips creating the GitHub release while preserving the rest of the CI pipeline.

## Why This Matters
- This is a real release-engineering scenario: teams may want CI to build, upload, and announce artifacts without also creating a GitHub release object.
- The change crosses config parsing, task planning, CI template generation, and docs, so consistency matters as much as the code path itself.
- The intended implementation is narrower than it first appears: the existing metadata/planning types already have the right layering, and the fix is about threading one workspace-global boolean through them consistently.

## Available Local Context
- [`ISSUE_CONTEXT.md`](ISSUE_CONTEXT.md) captures the release-workflow requirement and the expected downstream behavior.
- [`LOCAL_REPRO.md`](LOCAL_REPRO.md) points at the integration snapshot, config plumbing, and templating layers involved.
- [`REFERENCE.md`](REFERENCE.md) keeps the dataset provenance and upstream links.

## Constraints
- Keep the workspace in Rust and preserve the public API shape unless the tests require a documented breaking change.
- Prefer the smallest correct change in the owning files before widening.
- Run `./evaluate.sh proof-full` before you stop.

## Fast Loop
```bash
cargo test --quiet -p cargo-dist --test integration-tests axolotlsay_edit_existing -- --exact
```

## Likely Owners
- Primary owners: `cargo-dist/src/config.rs`, `cargo-dist/src/backend/ci/github.rs`, and `cargo-dist/src/tasks.rs`.
- Snapshot-backed integration coverage lives in `cargo-dist/tests/`.

## Strong Hints
- The new option belongs on `DistMetadata` as `create-release`; do not invent a separate runtime-only config struct field.
- This is a workspace-global setting. Package-level `package.metadata.dist.create-release` should warn and be ignored, just like other global-only options.
- `DistGraph` and the GitHub CI planner/template need a plain boolean that gates release creation while leaving upload/publish behavior intact.
- `init.rs` and `book/src/config.md` are part of the gold patch because generated config/docs must mention the new option.

## Expected Touch Targets
- `book/src/config.md`
- `cargo-dist/src/backend/ci/github.rs`
- `cargo-dist/src/config.rs`
- `cargo-dist/src/init.rs`
- `cargo-dist/src/tasks.rs`
- `cargo-dist/templates/ci/github_ci.yml.j2`

## Final Verify
```bash
./evaluate.sh proof-full
```

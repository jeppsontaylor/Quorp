# Local Repro

## Fast Loop
```bash
cargo test --quiet -p cargo-dist --test integration-tests axolotlsay_edit_existing -- --exact
```

## First Reads
- `cargo-dist/src/config.rs`
- `cargo-dist/src/backend/ci/github.rs`
- `cargo-dist/src/tasks.rs`
- `cargo-dist/tests/integration-tests.rs`
- `cargo-dist/src/init.rs`

## What To Watch
- The new flag must flow from config parsing into release planning and template generation.
- The snapshot-backed integration test is the quickest source of truth for whether all layers agree.
- Docs and init/config surfaces need to reflect the new option so generated output stays self-consistent.
- The common wrong turn is adding a duplicate field in the wrong config layer; the upstream fix threads `create-release` through existing metadata and graph structs instead.

## Done Looks Like
- The targeted integration snapshot passes without unexpected widening.
- Generated CI still builds artifacts and uploads them; only release creation is suppressed.
- `./evaluate.sh proof-full` passes from the workspace root.

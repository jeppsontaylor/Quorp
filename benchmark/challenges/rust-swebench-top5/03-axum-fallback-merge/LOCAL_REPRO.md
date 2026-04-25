# Local Repro

## Fast Loop
```bash
cargo test --quiet -p axum --lib --features headers routing::tests::
```

## First Reads
- `axum/src/routing/mod.rs`
- `axum/src/routing/tests/mod.rs`
- `axum/src/docs/routing/fallback.md`
- Focus specifically on the `nest` router destructuring and the fallback-selection match in `merge`.

## What To Watch
- The user-facing panic message matters; it is part of the behavior contract.
- `merge` and `nest` should agree about what happens when a nested router already owns a custom fallback.
- Docs and changelog updates are part of the intended patch, not optional cleanup.
- The relevant panic tests already exist in `axum/src/routing/tests/mod.rs`:
  - `merging_routers_with_fallbacks_panics`
  - `nesting_router_with_fallbacks_panics`
- `fallback.md` currently documents the buggy behavior, so editing only the code will leave the evaluator failing.

## Done Looks Like
- The fallback panic tests pass.
- The routing docs describe the unsupported multiple-fallback case accurately.
- `./evaluate.sh proof-full` passes from the workspace root.

# Local Repro

## Fast Loop
```bash
cargo test --quiet compile_intermediates
cargo test --quiet gnu_smoke
cargo test --quiet msvc_smoke
```

## First Reads
- `tests/test.rs`
- `src/lib.rs`

## What To Watch
- The new API is the feature request, but the `gnu_smoke` and `msvc_smoke` cases protect the old hashed object-path behavior.
- A good fix usually shares the object-path computation between `compile()` and the new intermediate-object entry point.
- This case uses the full `cargo test --quiet` evaluator in the end, so quick checks are only for iteration.

## Done Looks Like
- `compile_intermediates` returns object paths in input order.
- Existing `compile()` behavior still emits the hashed object paths that the smoke tests expect.
- `./evaluate.sh proof-full` passes from the workspace root.

# Objective

Fix `rust-lang__cc-rs-914` in the local `proof-full` workspace.

Callers need a first-class way to compile source files into intermediate object files without forcing archive creation or guessing the hashed output paths.

## Why This Matters
- Rust's bootstrap flow wants stable access to intermediate object files; the current archive-and-guess workaround broke once `cc-rs` changed its hashed object naming.
- This is easy to regress because the new API should expose the paths without changing the existing `compile()` behavior or object naming rules.

## Available Local Context
- [`ISSUE_CONTEXT.md`](ISSUE_CONTEXT.md) describes the bootstrap use case and the API shape callers are asking for.
- [`LOCAL_REPRO.md`](LOCAL_REPRO.md) calls out the smoke tests that guard hashed object naming plus a quicker iteration loop.
- [`REFERENCE.md`](REFERENCE.md) keeps the dataset provenance and upstream links.

## Constraints
- Keep the workspace in Rust and preserve the public API shape unless the tests require a documented change.
- Prefer the smallest correct change in the owning files before widening.
- Preserve the existing object-path semantics when compiling inputs: the new intermediate-object API must reuse the same hashed output naming behavior that `compile()` relies on, including for relative paths like `foo.c`.
- Run `./evaluate.sh proof-full` before you stop.

## Fast Loop
```bash
cargo test --quiet compile_intermediates
cargo test --quiet gnu_smoke
cargo test --quiet msvc_smoke
```

## Likely Owners
- Primary owner: `src/lib.rs`.
- Regression coverage lives in `tests/test.rs`.

## Expected Touch Targets
- `src/lib.rs`

## Final Verify
```bash
./evaluate.sh proof-full
```

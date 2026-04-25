# Objective

Fix `tokio-rs__axum-529` in the local `proof-full` workspace.

Merging or nesting routers with fallbacks currently hides an unsupported behavior instead of surfacing it immediately.

## Why This Matters
- Silently picking one fallback teaches users the wrong mental model for router composition and can hide production routing bugs.
- This is partly a behavior change and partly a docs change: the panic is intentional, and the routing docs need to explain the constraint clearly.
- The implementation is narrower than it first looks: the gold fix only changes the `nest` fallback destructuring, the `merge` fallback match arm, and the docs that described the old silent-discard behavior.

## Available Local Context
- [`ISSUE_CONTEXT.md`](ISSUE_CONTEXT.md) explains why multiple fallbacks are unsupported in axum's routing model.
- [`LOCAL_REPRO.md`](LOCAL_REPRO.md) points at the panic tests, the merge/nest implementation, and the documentation surfaces.
- [`REFERENCE.md`](REFERENCE.md) keeps the dataset provenance and upstream links.

## Constraints
- Keep the workspace in Rust and preserve the public API shape unless the tests require a documented breaking change.
- Prefer the smallest correct change in the owning files before widening.
- Run `./evaluate.sh proof-full` before you stop.

## Fast Loop
```bash
cargo test --quiet -p axum --lib --features headers routing::tests::
```

## Likely Owners
- Primary owner: `axum/src/routing/mod.rs`.
- Docs and changelog changes are part of the gold patch; tests live under `axum/src/routing/tests/`.

## Strong Hints
- In `Router::nest`, the nested router's `fallback` is currently discarded. The intended fix is to keep destructuring that field and panic if it is `Fallback::Custom(_)`.
- In `Router::merge`, the only behavior change is the `(Fallback::Custom(_), Fallback::Custom(_))` arm: it should panic instead of silently picking one side.
- The exact panic strings are asserted in `axum/src/routing/tests/mod.rs`, so match those tests before you stop.
- `axum/src/docs/routing/fallback.md` still documents the old merge/nest behavior and needs to be corrected, not extended.

## Expected Touch Targets
- `axum/CHANGELOG.md`
- `axum/src/docs/routing/fallback.md`
- `axum/src/docs/routing/merge.md`
- `axum/src/docs/routing/nest.md`
- `axum/src/routing/mod.rs`

## Final Verify
```bash
./evaluate.sh proof-full
```

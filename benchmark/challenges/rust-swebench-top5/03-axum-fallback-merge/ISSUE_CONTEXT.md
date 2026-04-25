# Issue Context

## User Story
Users composing routers with `merge` or `nest` should get immediate feedback when they try to combine multiple fallbacks, because axum does not support that routing model.

## Actual Failure
Today axum silently keeps one fallback and discards the other, which makes the API look more permissive than it really is.

## Expected Behavior
If both sides bring a fallback, router construction should panic with a clear explanation instead of quietly choosing one.
If a nested router already owns a custom fallback, `nest` should also panic immediately instead of dropping it.

## Why This Repo Cares
- Axum intentionally keeps routing internals flat for performance and predictability.
- This is also a documentation change: the behavior needs to be described in the routing guides so users understand why the panic exists.

## Likely Surfaces
- `axum/src/routing/mod.rs` for the merge/nest behavior
- `axum/src/routing/tests/mod.rs` for panic coverage
- `axum/src/docs/routing/{fallback,merge,nest}.md` and `axum/CHANGELOG.md`

## Narrow Fix Shape
- `nest` is not a routing-tree rewrite; the bug is that the nested router's `fallback` gets destructured and ignored.
- `merge` already centralizes fallback selection in one match expression; only the `Custom/Custom` case should change.
- The old `fallback.md` text explicitly says the second merged fallback wins and that nested fallbacks are discarded. That text should be removed or replaced because it describes the bug.

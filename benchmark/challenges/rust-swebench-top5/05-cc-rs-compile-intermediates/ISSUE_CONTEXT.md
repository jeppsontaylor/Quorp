# Issue Context

## User Story
Rust's bootstrap wants access to the intermediate object files produced by `cc-rs` so it can reuse them directly instead of creating an archive and guessing the hashed object paths afterward.

## Actual Failure
There is no first-class public API for "compile inputs into object files and tell me where they landed," which forced downstream callers to rely on fragile filename guessing.

## Expected Behavior
`cc-rs` should expose an intermediate-object compilation API that returns the object paths while preserving the existing `compile()` semantics and object naming behavior.

## Why This Repo Cares
- The object names are intentionally hashed in some cases; callers need the actual paths, not a guessed convention.
- The new API should reuse the existing object-path logic rather than introducing a second naming rule.

## Likely Surfaces
- `src/lib.rs` for the public API and shared object-path helper
- `tests/test.rs` for both the new API coverage and the smoke tests that assert hashed object naming

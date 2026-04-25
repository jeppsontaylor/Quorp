# Objective

Fix `chronotope__chrono-1403` in the local `proof-full` workspace.

Timestamp rounding close to the epoch incorrectly errors instead of truncating or rounding to the expected boundary.

## Why This Matters
- Chrono callers expect epoch-adjacent timestamps to truncate or round to `0`, not fail because the duration span is larger than the absolute timestamp.
- The failing cases sit in logic that also protects min/max timestamp behavior, so the fix needs to preserve overflow handling while correcting epoch semantics.

## Available Local Context
- [`ISSUE_CONTEXT.md`](ISSUE_CONTEXT.md) explains the negative/epoch edge case in user terms.
- [`LOCAL_REPRO.md`](LOCAL_REPRO.md) lists the focused round-module checks and the key code paths.
- [`REFERENCE.md`](REFERENCE.md) keeps the dataset provenance and upstream links.

## Constraints
- Keep the workspace in Rust and preserve the public API shape unless the tests require a documented breaking change.
- Prefer the smallest correct change in the owning files before widening.
- Run `./evaluate.sh proof-full` before you stop.

## Fast Loop
```bash
cargo test --quiet --lib round::tests::
```

## Likely Owners
- Primary owner: `src/round.rs`.
- All fail-to-pass coverage is in the round module tests.

## Expected Touch Targets
- `src/round.rs`

## Final Verify
```bash
./evaluate.sh proof-full
```

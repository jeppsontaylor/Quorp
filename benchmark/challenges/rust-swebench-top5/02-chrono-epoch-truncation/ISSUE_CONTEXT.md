# Issue Context

## User Story
Callers rounding or truncating timestamps close to the Unix epoch expect results like `1970-01-01T00:00:00Z`, not an error.

## Actual Failure
`duration_trunc` and related rounding logic reject some epoch-adjacent timestamps because they compare the span to the absolute timestamp and return `DurationExceedsTimestamp`.

## Expected Behavior
Epoch-adjacent values should round or truncate to the nearest valid boundary when that arithmetic is still representable.

## Why This Repo Cares
- The round/trunc helpers are shared API, so a fix has to preserve existing min/max overflow protection.
- The regression tests exercise both the epoch case and nearby boundary behavior to make sure the change is not over-broad.

## Likely Surfaces
- `src/round.rs`, especially `duration_round` and `duration_trunc`
- The `round::tests::*close_to_epoch*` tests in the same module

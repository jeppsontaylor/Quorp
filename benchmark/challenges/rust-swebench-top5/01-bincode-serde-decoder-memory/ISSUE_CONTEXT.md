# Issue Context

## User Story
A cache layer serializes owned Rust values with `bincode::serde::encode_to_vec` and later decodes the cached bytes with `decode_from_slice`.

## Actual Failure
Simple owned structs round-trip, but owned types that deserialize through richer serde visitors, such as `Uuid` and `chrono::DateTime<Utc>`, fail with `CannotBorrowOwnedData`.

## Expected Behavior
The owned serde decode entry point should deserialize those values from a byte slice without forcing borrowed lifetimes.

## Why This Repo Cares
- The failure blocks memory-cache and queue style workloads that store bytes once and hydrate owned application models later.
- The regression test in `tests/issues/issue_474.rs` models that exact cache round-trip.

## Likely Surfaces
- `src/features/serde/de_owned.rs` owns the serde-backed "decode owned value from slice" path.
- `Cargo.toml` participates because the issue coverage depends on serde-enabled chrono/uuid test support.

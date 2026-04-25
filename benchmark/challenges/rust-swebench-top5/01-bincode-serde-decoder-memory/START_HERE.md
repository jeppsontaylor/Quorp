# Objective

Fix `bincode-org__bincode-475` in the local `proof-full` workspace.

A serde-backed bincode decode path rejects owned payloads for types that should deserialize cleanly from cached bytes.

## Why This Matters
- The reported failure shows up in a realistic cache flow: bytes are stored once, then decoded later into fully owned application structs.
- The bug is subtle because plain owned strings work, while serde-heavy types such as `Uuid` and `DateTime<Utc>` trigger `CannotBorrowOwnedData`.

## Available Local Context
- [`ISSUE_CONTEXT.md`](ISSUE_CONTEXT.md) summarizes the user-visible failure and acceptance target.
- [`LOCAL_REPRO.md`](LOCAL_REPRO.md) gives a fast loop, first reads, and review checklist.
- [`REFERENCE.md`](REFERENCE.md) keeps the dataset provenance and upstream links.

## Constraints
- Keep the workspace in Rust and preserve the public API shape unless the tests require a documented breaking change.
- Prefer the smallest correct change in the owning files before widening.
- Run `./evaluate.sh proof-full` before you stop.

## Fast Loop
```bash
cargo test --quiet --features serde --test issues issue_474
```

## Likely Owners
- Primary owner: `src/features/serde/de_owned.rs`.
- Tests live in `tests/issues.rs` and `tests/issues/issue_474.rs`.

## Expected Touch Targets
- `Cargo.toml`
- `src/features/serde/de_owned.rs`

## Final Verify
```bash
./evaluate.sh proof-full
```

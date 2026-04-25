# Local Repro

## Fast Loop
```bash
cargo test --quiet --features serde --test issues issue_474
```

## First Reads
- `tests/issues/issue_474.rs`
- `tests/issues.rs`
- `src/features/serde/de_owned.rs`

## What To Watch
- This is specifically the owned decode path, not the borrowed-slice helpers.
- Keep existing serde round-trip behavior intact for strings, byte buffers, and borrowed data.
- If you touch `Cargo.toml`, make sure it is only to support the regression coverage already implied by the issue.

## Done Looks Like
- The focused issue test passes.
- `./evaluate.sh proof-full` passes from the workspace root.

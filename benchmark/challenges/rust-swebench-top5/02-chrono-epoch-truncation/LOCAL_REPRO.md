# Local Repro

## Fast Loop
```bash
cargo test --quiet --lib round::tests::
```

## First Reads
- `src/round.rs`
- The `test_duration_trunc_close_to_epoch` and `test_duration_round_close_to_epoch` cases

## What To Watch
- Negative timestamps use different modulo behavior than positive ones.
- The min/max timestamp guards still matter; the fix should not turn overflow cases into silent arithmetic.
- This challenge should stay in `src/round.rs`; widening would be a sign the core arithmetic was not understood yet.

## Done Looks Like
- The close-to-epoch tests pass.
- The broader `round::tests::` set still passes.
- `./evaluate.sh proof-full` passes from the workspace root.

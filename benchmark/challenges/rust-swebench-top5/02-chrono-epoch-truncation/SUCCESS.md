# Success Criteria

- `cargo test --quiet --lib round::tests::` exits successfully.
- The fix addresses the bug described in [`START_HERE.md`](START_HERE.md) without reverting the dataset test patch.
- Changes stay focused on the owning implementation and any gold-patch documentation/config files that are needed.

## Dataset Fail-to-Pass Coverage
- `round::tests::test_duration_round_close_to_epoch`
- `round::tests::test_duration_round_close_to_min_max`
- `round::tests::test_duration_trunc_close_to_epoch`

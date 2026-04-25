# Reference

- Dataset: `user2f86/rustbench`
- Split: `train`
- Instance: `chronotope__chrono-1403`
- Repository: `chronotope/chrono`
- Base commit: `ef9a4c9539da5e463a0b8c9dd45920f3a265f421`
- Dataset version: `0.4`
- Issue: https://github.com/chronotope/chrono/issues/1375
- Pull request: https://github.com/chronotope/chrono/pull/1403

## Problem Statement

See [`upstream/problem_statement.md`](upstream/problem_statement.md).

## Gold Patch Files

- `src/round.rs`

## Dataset Fail-to-Pass Tests

- `round::tests::test_duration_round_close_to_epoch`
- `round::tests::test_duration_round_close_to_min_max`
- `round::tests::test_duration_trunc_close_to_epoch`

## Dataset Pass-to-Pass Tests

- `date::tests::test_date_add_assign`
- `date::tests::test_date_sub_assign`
- `date::tests::test_years_elapsed`
- `date::tests::test_date_add_assign_local`
- `date::tests::test_date_sub_assign_local`
- `datetime::tests::nano_roundrip`
- `datetime::tests::signed_duration_since_autoref`
- `datetime::tests::test_add_sub_months`
- `datetime::tests::test_auto_conversion`
- `datetime::tests::test_core_duration_ops`
- `datetime::tests::test_datetime_add_assign`
- `datetime::tests::test_core_duration_max - should panic`
- `datetime::tests::test_datetime_add_days`
- `datetime::tests::test_datetime_add_months`
- `datetime::tests::test_datetime_date_and_time`
- `datetime::tests::test_datetime_fixed_offset`
- `datetime::tests::test_datetime_from_local`
- `datetime::tests::test_datetime_format_with_local`
- `datetime::tests::test_datetime_from_timestamp_millis`
- `datetime::tests::test_datetime_is_send_and_copy`
- `datetime::tests::test_datetime_from_str`
- `datetime::tests::test_datetime_offset`
- `datetime::tests::test_datetime_add_assign_local`
- `datetime::tests::test_datetime_local_from_preserves_offset`
- `datetime::tests::test_datetime_rfc2822`
- `...` (478 more pass-to-pass checks in dataset metadata)

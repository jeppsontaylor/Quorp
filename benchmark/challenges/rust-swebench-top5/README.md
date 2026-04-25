# Rust SWE-bench Top 5 Cohort

This directory is Quorp's local mirror of the canonical Rust SWE-bench cohort now published in
`/Users/bentaylor/Code/WarpOS/challenges/cases/06-*` through `10-*`.
Each case is sourced from the official `user2f86/rustbench` dataset, pinned to the dataset
`base_commit`, and materialized as a local `proof-full` workspace with the dataset `test_patch`
already applied.
Each bundle now includes a richer handoff surface:
- `START_HERE.md` for the benchmark contract and final verify command
- `ISSUE_CONTEXT.md` for the user-facing failure story
- `LOCAL_REPRO.md` for the local fast loop and first-read guidance

Quorp should be run from the built binary when possible so repeated study loops do not pay a full
rebuild cost.
`benchmark run` and `benchmark batch` now default to the `codex` executor; pass
`--executor native` only when you are explicitly exercising the repaired local runtime path.

```bash
cargo build -p quorp
QUORP_BIN=${QUORP_BIN:-./target/debug/quorp}
```

## Run one case

```bash
$QUORP_BIN benchmark run \
  --path benchmark/challenges/rust-swebench-top5/01-bincode-serde-decoder-memory \
  --result-dir /tmp/quorp-rustbench-case
```

## Run the full cohort

```bash
$QUORP_BIN benchmark batch \
  --cases-root benchmark/challenges/rust-swebench-top5 \
  --result-dir /tmp/quorp-rustbench-batch
```

## Run the five-agent matrix

```bash
python3 /Users/bentaylor/Code/quorp/script/quorp-rust-swebench-five-agent-matrix
```

The matrix runner reads the canonical WarpOS suite at
`/Users/bentaylor/Code/WarpOS/challenges/suites/rust-swebench-top5.json`,
runs `antigravity`, `cursor`, `claude`, `codex`, and `quorp` in that order,
and writes matrix summaries under
`~/.warpos/reports/rust-swebench-top5-five-agent/run-<timestamp>/`.

## Validate the packaged cases

```bash
cargo test -p quorp rust_swebench_top5_structure_and_resolution
cargo test -p quorp rust_swebench_retry_reset_restores_clean_workspace
cargo test -p quorp -- --ignored rust_swebench_top5_gold_patch_validation
```

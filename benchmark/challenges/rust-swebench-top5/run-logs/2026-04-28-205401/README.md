# Rust SWE-bench Top 5 Run Logs

This directory contains a copied, read-only archive of the successful 5/5 Quorp benchmark run.

Source run:

`/Users/bentaylor/Library/Caches/Quorp/benchmarks/rust-swebench-top5-full-fixed-20260428-205401/run`

How to read it:

1. Start with `batch-report.json` and `run-summary.json` at the archive root.
2. For a case, open `cases/<case-name>/benchmark-report.md` first.
3. Then inspect `cases/<case-name>/attempt-001/summary.json` and `cases/<case-name>/attempt-001/agent/events.jsonl` for the agent execution trace.
4. Use `cases/<case-name>/attempt-001/judge-request.json` and `judge-response.json` to see the final evaluation exchange.
5. Use `logs/<case-name>.log` for the raw per-case benchmark log stream.

The archived case directories intentionally omit the sandbox payloads from the source run. The useful artifacts for review are the reports, attempt metadata, event stream, and raw logs.

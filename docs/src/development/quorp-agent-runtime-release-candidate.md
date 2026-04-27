# Quorp Agent Runtime Release Candidate

This release candidate closes the terminal-first Rust agent runtime work tracked in `agent/phase-tracker.md`.

## Shipped Capabilities

- Stream-first terminal UX with deterministic render demo capture and no-color fallback.
- OpenAI-compatible provider/session core with NVIDIA/Qwen profile defaults, stream normalization, request metadata, usage, retry-after handling, and raw payload hashes.
- Sandboxed full-auto execution with git-worktree or tmp-copy isolation, `--yolo` sandbox enforcement, and typed permission classification.
- Patch VM write path with previews, stable preview IDs, hash preconditions, risky-edit gating, rollback tokens, and large full-file rewrite rejection.
- Context OS support for owner maps, test maps, proof lanes, generated zones, bounded excerpts, duplicate suppression, and lexical retrieval.
- ProofLens-style verification packets for command evidence, cargo diagnostics, failing tests, security findings, raw-log paths, and hashes.
- Memory and self-improvement scaffolding for failed-attempt fingerprints, retry blocking, proof-packet fingerprint extraction, and shadow rule lifecycle promotion/challenge.
- Typed full-auto SWE loop controller with stages, budgets, stall detection, cancellation, proof hashes, and memory hashes.
- Evaluation scoreboards with success, secure success, SecureETTS, tokens, wall time, patch size, retries, proof-lane counts, regression detection, and CI gate wiring.

## Release Gates

Required gates for this release candidate:

- `cargo fmt --all --check`
- `cargo check --workspace`
- `cargo test --workspace --lib`
- `./script/clippy`
- `just benchmark-gate`
- `just medium`
- `just deep`
- `cargo build --release -p quorp`
- `QUORP_RENDER_DEMO_STATIC=1 target/release/quorp render-demo`
- `NO_COLOR=1 QUORP_RENDER_DEMO_STATIC=1 target/release/quorp render-demo`
- `actionlint .github/workflows/benchmark-evaluation.yml .github/workflows/loc-cap.yml`

Conditional gates:

- `just security` when `cargo audit` and `gitleaks` are installed.
- `QUORP_BENCHMARK_RUN_SMOKE=1 just benchmark-gate` when live provider credentials are available.
- `QUORP_BENCHMARK_RUN_DIR=<run_dir> just benchmark-gate` when scoring an existing live run.

## Release Notes

- Added terminal-first agent runtime foundations across UX, provider sessions, sandboxing, Patch VM writes, context, verification, memory, autonomous SWE control, and evaluation.
- Improved release confidence with deterministic benchmark regression gates and proof-lane tracking.
- N/A for crates.io publication: workspace publishing remains disabled.

## Receipt

Recorded locally on 2026-04-26 22:54:08 MDT:

- Passed `cargo fmt --all --check`.
- Passed `cargo check --workspace`.
- Passed `cargo test --workspace --lib` with 458 tests across 39 suites.
- Passed `./script/clippy`.
- Passed `just benchmark-gate`; live provider smoke was not requested.
- Passed `just medium`.
- Passed `just deep` after stabilizing parallel all-features diagnostics/MCP tests.
- Passed `just security` after updating `rustls-webpki` from `0.103.10` to `0.103.13`; `cargo audit` found no vulnerabilities and reported allowed warnings.
- At initial receipt time, `gitleaks` was not installed locally; the follow-up live audit below records the installed-tool run.
- Passed `cargo build --release -p quorp`.
- Passed `target/release/quorp --help`.
- Passed `QUORP_RENDER_DEMO_STATIC=1 target/release/quorp render-demo`.
- Passed `NO_COLOR=1 QUORP_RENDER_DEMO_STATIC=1 target/release/quorp render-demo`.
- At initial receipt time, `actionlint` was not installed locally; the follow-up live audit below records the installed-tool run.
- At initial receipt time, live provider benchmark smoke was not run; the follow-up live audit below records smoke and top5 runs.

## Live Benchmark Audit

Recorded locally on 2026-04-26 23:29:40 MDT after installing `gitleaks` and `actionlint` with Homebrew:

- Passed `actionlint .github/workflows/benchmark-evaluation.yml .github/workflows/loc-cap.yml`.
- Passed `just security` with `cargo audit`, `gitleaks`, and the LOC cap lane available. `cargo audit` reported no vulnerabilities and retained the existing allowed warnings.
- `gitleaks detect --source . --redact` reported 9 redacted historical findings in deleted/old paths. `gitleaks detect --source . --redact --no-git` reported 2 current local findings in `.env` lines 3 and 5; values were not printed.
- Passed one-case live provider smoke on `06-rust-swebench-bincode-serde-decoder-memory`: solved 1/1, secure success 1/1, 9 requests, 39,396 billed tokens, 151,224 ms wall time.
- Fixed a live batch runner path defect by canonicalizing `result_dir` and `log_dir` before launching per-case benchmark children from each case directory.
- Passed live Rust SWE top5 after the path fix: solved 3/5, secure success 3/5, 34 requests, 204,978 billed tokens, SecureETTS 134,027, 865,472 ms scored wall time, 323 changed lines.
- Rescored the live top5 run with the release binary and audit fields: 5/5 watchdog near-limit cases, 1 patch-quality-risk case, 0 slow first-request-token cases at the 30,000 ms threshold. Scoreboard: `target/quorp-live-benchmarks/scoreboards/top5-release-audit/session-1777267956`.

Remaining targeted improvements from this audit:

- Case 07 reached the correct owner range but stopped after repeated no-op/prose turns; the repair controller should force a concrete ranged edit or preview after an honored patch packet.
- Case 08 timed out in the fast loop without a parseable failure anchor; the controller should synthesize a target anchor from the case owner/test map or rerun a narrower named proof before declaring `repair_loop_stalled`.
- Case 06 still allowed a manifest/dependency detour after a source failure; dependency edits should be gated on explicit missing-crate/import diagnostics.
- Case 09 passed but used multi-file full writes without structured edit evidence; this is now surfaced as a scoreboard patch-quality risk.

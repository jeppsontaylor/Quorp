# AGENT_SUPPORT.md

This file is the current support audit for Quorp's terminal-first Rust agent runtime. It intentionally describes only code that exists in this worktree.

## Current Product Shape

Quorp is a single Rust binary with a stream-first CLI, a headless autonomous run path, typed agent tools, git-worktree or tmp-copy sandboxing, permission policy primitives, Patch VM writes, proof packets, memory/rule scaffolding, benchmark scoreboards, and proof receipts.

Primary entrypoints:

- `crates/quorp/src/main.rs`
- `crates/quorp/src/quorp/run_support.rs`
- `crates/quorp/src/quorp/agent_runner.rs`
- `crates/quorp_agent_core/src/runtime.rs`
- `crates/quorp_term/src/lib.rs`
- `crates/quorp_render/src/quorp_render.rs`
- `crates/quorp_render/src/session.rs`

Current default provider direction:

- NVIDIA NIM / Qwen3-Coder through an OpenAI-compatible chat-completions API.
- Provider configuration lives in `crates/quorp/src/quorp/provider_config.rs` and shared defaults live in `crates/quorp_core/src/lib.rs`.
- The runtime should treat NVIDIA as a default profile, not as a product-specific special case.

## Verification Baseline

Release-candidate verification currently requires:

- `cargo check --workspace`
- `cargo test --workspace --lib`
- `./script/clippy`
- `just benchmark-gate`
- `just medium`

Optional release gates:

- `just deep`
- `just security`
- `cargo build --release -p quorp`
- Static and no-color `quorp render-demo` smoke checks.

## Supported Surfaces

### Inline CLI

The default no-subcommand path starts an inline terminal session. It prints startup and plan cards, accepts slash commands, and can launch a remote agent run for a prompt. The release-candidate session renderer is stream-first and supports deterministic demo capture plus no-color fallback.

Core files:

- `crates/quorp/src/main.rs`
- `crates/quorp_term/src/lib.rs`
- `crates/quorp_render/src/*`

Important commands:

- `quorp`
- `quorp <task text>`
- `quorp session --workspace <path>`
- `quorp render-demo`
- `quorp commands [prefix]`
- `quorp doctor`

### Headless Autonomous Runs

`quorp run` and `quorp exec` run the autonomous runtime against a workspace/objective, write event logs, and emit a proof receipt.

Core files:

- `crates/quorp/src/main.rs`
- `crates/quorp/src/quorp/agent_runner.rs`
- `crates/quorp/src/quorp/run_support.rs`
- `crates/quorp_agent_core/src/runtime.rs`

Important outputs:

- `request.json`
- `metadata.json`
- `summary.json`
- `events.jsonl`
- `proof-receipt.json`
- `logs/`

### Agent Runtime

The runtime owns the multi-step control loop: request completion, parse a structured turn, dispatch typed tools, track validation state, checkpoint, retry, and stop.

Core files:

- `crates/quorp_agent_core/src/runtime.rs`
- `crates/quorp_agent_core/src/runtime/*.rs`
- `crates/quorp_agent_core/src/agent_protocol.rs`
- `crates/quorp_agent_core/src/agent_turn.rs`

Supported action families include reads, listing, text/symbol/file search, structural search/preview, cargo diagnostics, repo capsules, edit previews, replace-range, TOML edits, patch application, validation, shell commands, and MCP calls through the native backend path.

### Sandbox And Permissions

Current sandbox support:

- `host`
- `tmp-copy`
- `git-worktree` for git repositories, with tmp-copy fallback

Current permission primitives:

- `read-only`
- `ask`
- `accept-edits`
- `auto-safe`
- `yolo-sandbox`

Core files:

- `crates/quorp_sandbox/src/lib.rs`
- `crates/quorp_permissions/src/quorp_permissions.rs`
- `crates/quorp_tools/src/lib.rs`

`--yolo` is an explicit sandboxed full-auto shortcut and is rejected with direct host sandboxing.

### Context, Memory, Rules, And Verification

These crates are release-candidate support systems with deterministic APIs and tests:

- `crates/quorp_context`
- `crates/quorp_memory`
- `crates/quorp_rule_forge`
- `crates/quorp_verify`
- `crates/quorp_repo_scan`
- `crates/quorp_repo_graph`
- `crates/quorp_rust_intel`

Current support includes workspace context compilation from `agent/` contracts, ProofLens-style proof packet parsing, negative retry memory, and rule-forge shadow lifecycle accounting. Some pieces remain product support crates rather than fully autonomous runtime policy.

### Benchmark And Evaluation Gates

Benchmark scoring is a supported release surface:

- `quorp benchmark score`
- `quorp benchmark score --fail-on-regression`
- `script/quorp-benchmark-regression-gate`
- `just benchmark-gate`

The deterministic gate runs without provider credentials. Live benchmark smoke scoring is opt-in through `QUORP_BENCHMARK_RUN_SMOKE=1` or `QUORP_BENCHMARK_RUN_DIR=<run_dir>`. Scoreboards also surface live-run audit signals for first-request latency, watchdog near-limit pressure, and successful patches that used broad writes without structured edit evidence.

## Agent-Facing Repository Contracts

MSS-style artifacts live under `agent/`:

- `agent/owner-map.json`
- `agent/test-map.json`
- `agent/proof-lanes.toml`
- `agent/generated-zones.toml`
- `agent/unsafe-ledger.toml`
- `agent/phase-tracker.md`

Agents should consult these before widening edits or validation.

## Known Gaps

- Live provider benchmark scoring requires operator-provided API credentials and remains opt-in for CI.
- `gitleaks` is available locally and the security lane runs it, but the current recipe treats findings as non-fatal. A worktree-only scan currently reports local `.env` findings, so provider credentials should stay out of source-scanned worktrees before making this a hard gate.
- Context, memory, rule-forge, and verification crates are deterministic support systems; deeper runtime mediation and durable long-term memory remain future work.
- Release packaging/signing is not represented here; this workspace has `publish = false` and the current closure is a binary/worktree release candidate.
- `AGENT_SUPPORT.md` does not claim support for deleted historical TUI files, Docker re-exec, or SSD-MOE-local model management.

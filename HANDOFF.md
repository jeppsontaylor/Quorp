# Quorp — Big-Bang Refactor Handoff

**Branch**: `codex/agent-first-cleanup`
**Historical base**: `a6a3974 — Phase 4-A + 12: extract quorp_mcp crate, add CI loc-cap enforcement`
**Reconciled on**: 2026-04-28
**Audience**: incoming agent picking up the same plan

> This document is now partly historical. The workspace has moved beyond the commit references below: `quorp_cli` and `quorp_session` are extracted, context/compiler actions are wired, the native backend emits Patch VM write receipts for write actions, verification runs produce staged verify reports, and the oversized runtime/benchmark test files have been split into include chunks under the LOC cap. Read older sections as roadmap context, not current truth.

## Current Truth

- Latest verified commits before this reconciliation were `ae12aeb` and `2c66dfa`.
- `tips/upgrade/v2/` does not exist as a reachable tree in this checkout. A search of stash, reachable history, unreachable trees, and dangling blobs found no path metadata for `tips/upgrade/v2/*.txt`, but did recover v2-like dangling text blobs covering localization, precise edits, task runtime, verification, memory, scoped permissions, MCP, and worktree isolation. Strong candidates: `7d8d1358a3a739cd322e99bc05dda805c123bc16`, `5692dcf1d203cb162d8801fd75fd0bae754ac22b`, `b946fd3711cd2f67acd4b6b6f5189c61023db853`, `f4e3641e8c0fc57cea753a4b32fbe371a951f2d0`, `c065146fe3059a4d14b5703834e38dc58a063f91`, `e4f07f0860f946038377b6bd43378ec3417da485`, and `74f2480ff06e6166514934ceaa6232a111f5f96b`.
- Phase 10 is partially complete, not pending from zero:
  - `ContextCompiler` is on the session prompt path.
  - `ExpandContext`, `RecallMemory`, and `ProposeRule` are surfaced through schema, parsing, and backend dispatch.
  - The runtime now has a named, bounded `RuntimeEventFanout` adapter with backpressure events in addition to sink-based emission.
  - Native-backend `WriteFile`, `ApplyPatch`, `ReplaceBlock`, `ReplaceRange`, `ModifyToml`, `ApplyPreview`, and `SetExecutable` write paths now return Patch VM receipts instead of only direct write summaries.
  - `RunValidation` now maps validation commands into `quorp_verify` staged reports with proof packets and cache hits.
- Phase 11 is also partially complete:
  - `crates/quorp_agent_core/src/runtime/tests.rs` and `crates/quorp_cli/src/quorp/benchmark/tests.rs` are now split into include chunks under 2,000 LOC each.
  - Targeted regression coverage exists for verify-cache execution, validation-plan mapping, Patch VM-backed write receipts, and runtime event fanout.
- Remaining work is narrower than the older sections suggest: durable memory/rule subscriber workers, deeper verify executors than shell-command replay, broader replay/proof CLI surfaces, and continued benchmark/runtime polish.

---

## 0. The 60-second orientation

Quorp is being transformed into a Rust-native, terminal-first, Claude/Codex-CLI-style coding agent. The user calls this a "big bang" refactor — one execution arc that splits oversized files, extracts crates, scaffolds a smart-tooling layer (Context Compiler, Memory OS, Rule Forge, Patch VM, Verification Ladder), and replaces the deleted ratatui/zed-import surface with a custom-ANSI brilliant CLI renderer.

The full plan lives at `/Users/bentaylor/.claude/plans/can-you-please-study-mighty-pudding.md`. Read it.

**Hard rules from the user (never violate):**
- 2,000 LOC hard cap on any production-code `*.rs` file (soft target 800).
- No `mod.rs` paths — always `src/<name>.rs` plus `[lib] path = "src/<name>.rs"` in `Cargo.toml`.
- No `let _ =` on fallible operations.
- Nvidia-only provider for now, but **designed pluggable** (provider trait, Nvidia impl).
- "Act like an owner" — make decisions, commit small enough to revert, ship working code.
- "Ensure great test coverage while you build."
- No ratatui. Custom ANSI on crossterm, scrollback-first.

---

## 1. ⚠ CRITICAL — Branch state on takeover

### 1.1 Uncommitted in-progress work

`git status --short` reports a small set of modified and untracked files. **The working tree compiles cleanly** (`cargo check --workspace` is green), so this is not a broken half-edit — it is *partially-completed real work*.

The largest substantive deltas:

| File | +/− | Nature of change |
|---|---|---|
| `AGENT_SUPPORT.md` | +100 / −699 | Docs rewrite collapsing the legacy SSD-MOE / TUI references and reframing as the current native shape. Safe to keep or commit standalone. |
| `crates/quorp/src/main.rs` | −3111 | Thin wrapper now delegates into the in-crate CLI split. |
| `crates/quorp/src/quorp/cli.rs` | +1655 | New CLI body with dispatch, inline mode, and command handling. |
| `crates/quorp/src/quorp/cli_runtime.rs` | +1462 | Shared runtime helpers extracted from the old monolith. |
| `crates/quorp/src/quorp/tui/agent_protocol.rs` | +2 / −2 | Protocol shim now re-exports the real `quorp_agent_protocol` crate. |
| `crates/quorp_agent_protocol/src/quorp_agent_protocol.rs` | +94 | Real wire envelopes and versioned runtime message tests. |
| `crates/quorp_agent_core/src/agent_turn.rs` | +179 | Turn parsing now uses extracted metadata helpers. |
| `crates/quorp_agent_core/src/runtime.rs` | +26 | Runtime event and envelope plumbing updated for the protocol split. |
| `crates/util/src/paths.rs` | −1554 | Vendored utility cleanup; LOC cap script now excludes this crate. |
| `script/check-loc-cap` | +2 / −2 | LOC gate now skips vendored utility crates and test files. |
| ~60 other files | mostly formatter | Bulk formatter pass (`cargo fmt`) ran across the workspace. Mostly whitespace/import-grouping noise. |

**What to do first:**
1. `git diff --stat` and skim the list. Sort the substantive ones from the formatter noise.
2. Decide whether to commit the work-in-progress as a "Phase 9-final wire-up of quorp_render into inline CLI" commit, or keep going on top of it. The author's recommendation: commit it as its own logical commit before starting Phase 4-B, so the next branch baseline is clean.
3. **Do not** `git restore .` — you will lose the renderer wiring work that is already done.

### 1.2 ⚠ CI loc-cap workflow

`Phase 12` added `.github/workflows/loc-cap.yml`, and the branch now has the matching `script/check-loc-cap` exclusions in-tree. The script skips vendored utility crates and test files, so `./script/check-loc-cap 2000 --error` is green on the current workspace.

### 1.3 Current LOC snapshot

```
   9  crates/quorp/src/main.rs                                       <- thin wrapper
1655  crates/quorp/src/quorp/cli.rs                                  <- in-crate CLI split
1462  crates/quorp/src/quorp/cli_runtime.rs                          <- in-crate CLI runtime helpers
1663  crates/quorp/src/quorp/benchmark.rs                            <- now under cap
8874  crates/quorp_agent_core/src/runtime/tests.rs                   <- test code, excluded by LOC gate
3381  crates/quorp/src/quorp/benchmark/tests.rs                      <- test code, excluded by LOC gate
1596  crates/util/src/paths.rs                                       <- vendored, excluded by LOC gate
```

The production `quorp_*` files are now under the 2,000 LOC cap.

---

## 2. What's done (so the next agent doesn't redo it)

| Phase | Commit | Status | What it delivered |
|---|---|---|---|
| 0 — Hygiene | `b445842` | ✓ | Dead zed.dev crates purged, justfile + check-loc-cap added, codex_claude_copy/*.txt recovered |
| 1 — Foundations | `7c21c5b` | ✓ | 8 domain crates: `quorp_ids`, `quorp_agent_protocol`, `quorp_repo_graph`, `quorp_context_model`, `quorp_memory_model`, `quorp_rule_model`, `quorp_patch_model`, `quorp_verify_model` |
| 2 — Benchmark extraction | `d2a6d27`, `da09246`, `a806c6c` | ✓ | `benchmark.rs` (5,548) split into 5 sibling files all under cap; existing `quorp_benchmark` crate already absorbed the heavy challenge/runner code |
| 3 — Runtime split | `5c78753`, `2e4d2b3`, `7deb61c`, `0670915` | ✓ | `runtime.rs` (20,755) → 766 LOC root + 11 sibling modules; `agent_turn.rs` split into parser + render + tests |
| 4-A — `quorp_mcp` extraction | `a6a3974` | ✓ | New crate at `crates/quorp_mcp/`, 560 LOC + 8 round-trip tests; original `tui/mcp_client.rs` collapsed to a 5-line `pub use quorp_mcp::*;` shim |
| 4-B — `quorp_cli` extraction | current worktree has the extracted crate and thin binary wrapper | ✓ | `quorp_cli` now owns the CLI body and the `quorp` binary delegates into it |
| 4-C — `quorp_session` extraction | current worktree has the extracted crate and thin bridge shims | ✓ | `quorp_session` now owns the session/chat/runtime helpers and `quorp_cli` consumes them through re-exports |
| 5 — `native_backend` split | `058b1e3`, `022f639` | ✓ | `native_backend.rs` reduced from 3,440 → 1,725 LOC; `actions.rs` sibling extracted |
| 6 — Storage + repo_scan + memory + rule_forge skeletons | `851454e` | ✓ | Skeleton crates compile and pass tests; not yet wired into the runtime |
| 7 — Context + patch_vm + verify + rust_intel skeletons | `851454e` | ✓ | Same — skeletons only |
| 8 — Permissions + plan_mode + slash | `851454e` | ✓ | Skeletons compile + pass tests |
| 9 — Brilliant CLI renderer | `851454e`, `09e9da4`, `8f3095a`, `f0bde4d`, `d63e17e` | ✓ | `quorp_render` is wired into 5 production subcommands and the inline CLI lives in the extracted `quorp_cli` crate |
| 10 — Wire smart tooling into agent loop | — | **PENDING** | See §3 |
| 11 — Tests, polish, docs | — | **PENDING** | See §3 |
| 12 — CI loc-cap enforcement | `a6a3974`, current workspace | ✓ | `.github/workflows/loc-cap.yml` exists and the matching LOC-cap exclusions are in-tree |

**Current test baseline:** 531 passed, 1 ignored across 41 suites with `cargo test --workspace --exclude util --tests -- --test-threads=2`. (Exclude `util` because it has its own tests that flake under high parallelism; use `--test-threads=2` to avoid integration-test parallelism flakes in the benchmark crate.)

---

## 3. Pending phases — full detail

### 3.1 Phase 4-B — Extract `quorp_cli`

**Why**: the CLI body has already been split into `crates/quorp/src/quorp/cli.rs` and `cli_runtime.rs`, and `crates/quorp/src/main.rs` is now just a thin wrapper. The remaining work is promoting that split into a real `quorp_cli` crate so the binary calls only `quorp_cli::dispatch()`. Pulling this out:
- Lets the binary shrink to ~80 LOC (`fn main() { std::process::exit(quorp_cli::dispatch().unwrap_or(1)); }`).
- Lets `quorp_session` (Phase 4-C) cleanly absorb the chat-service modules without circular deps.
- Lets `run_support.rs` (1,253 LOC) move with its callers and split along the `launch / doctor / receipts` boundary the plan specifies.

**Scope (LOC moved)**: ~3,000 LOC across the current CLI body and runtime helpers. After the move:
- `crates/quorp_cli/src/quorp_cli.rs` (~150) — clap parser + dispatch + small module re-exports.
- `crates/quorp_cli/src/commands/run.rs` (~250)
- `crates/quorp_cli/src/commands/exec.rs` (~250)
- `crates/quorp_cli/src/commands/doctor.rs` (~250)
- `crates/quorp_cli/src/commands/bench.rs` (~250)
- `crates/quorp_cli/src/commands/cli.rs` (~250) — the interactive `run_inline_cli` + `run_inline_task` loop. **This file will hold the renderer wiring that is currently uncommitted in `main.rs` and `cli_demos.rs` (§1.1).** Bring it across cleanly.
- `crates/quorp_cli/src/commands/agent.rs` (~250)
- `crates/quorp_cli/src/commands/session.rs` (~250)
- `crates/quorp_cli/src/commands/diagnostics.rs` (~250)
- `crates/quorp_cli/src/run_support/run_support.rs` (~120)
- `crates/quorp_cli/src/run_support/launch.rs` (~400)
- `crates/quorp_cli/src/run_support/doctor.rs` (~350)
- `crates/quorp_cli/src/run_support/receipts.rs` (~400)
- `crates/quorp/src/main.rs` shrinks to ~80 LOC.

**Step-by-step playbook** (proven during 4-A; replicate exactly):

1. **Scaffold the crate.** Create `crates/quorp_cli/Cargo.toml` with `[lib] path = "src/quorp_cli.rs"`. Add deps: `anyhow`, `clap = { version = "*", features = ["derive"] }`, `tokio = { features = ["full"] }`, plus every workspace dep that `main.rs` pulls in today (`quorp_core`, `quorp_config`, `quorp_provider`, `quorp_term`, `quorp_render`, `quorp_slash`, `quorp_permissions`, `quorp_repo_scan`, `quorp_agent_core`, `quorp_benchmark`, `quorp_tools`, `quorp_sandbox`, `quorp_mcp`, `quorp_session` once it exists).
2. **Add to workspace.** Append `"crates/quorp_cli"` to `Cargo.toml` `[workspace] members`, add `quorp_cli = { path = "crates/quorp_cli" }` to `[workspace.dependencies]`.
3. **Move bodies, not edits.** Copy `crates/quorp/src/main.rs` verbatim into `crates/quorp_cli/src/quorp_cli.rs`. Then split into `commands/*.rs` siblings file by file. **Keep public symbols `pub` so the binary's `dispatch()` re-exports work.** Same for `run_support.rs` → `crates/quorp_cli/src/run_support/`.
4. **Compatibility shim.** In the binary, replace `crates/quorp/src/main.rs` with the eighty-line version:
   ```rust
   fn main() {
       std::process::exit(quorp_cli::dispatch().unwrap_or(1));
   }
   ```
   And `crates/quorp/src/quorp/run_support.rs` becomes `pub use quorp_cli::run_support::*;` (same shim pattern as `mcp_client.rs` did in 4-A).
5. **Add tests.** Each `commands/<name>.rs` gets a `#[cfg(test)]` module verifying the clap struct round-trips its expected args (use `clap::Command::try_get_matches_from`). Aim for 2-3 tests per command file (~16-24 new tests).
6. **Verify.**
   - `cargo check --workspace` — green.
   - `cargo build -p quorp` — green.
   - `cargo test --lib -p quorp_cli` — new tests pass.
   - `cargo test --workspace --exclude util --tests -- --test-threads=2` — still 531+ passing.
   - `./script/check-loc-cap 2000 --error` — green for `quorp_*` crates and the vendored exclusions.
   - **Runtime smoke test**: `cargo build --release && ./target/release/quorp doctor` and `./target/release/quorp --help` both emit the expected output. Then `./target/release/quorp scan` and `./target/release/quorp permissions check write_file foo`. The output should be byte-identical to the pre-extraction version (modulo any renderer migration carried in from §1.1 WIP).

**Risks / foot-guns**:
- `main.rs` has a global `static` for the runtime config that several commands reach into. When moving, make sure the lifetime / lazy-init semantics match — pass it explicitly through `dispatch()` rather than re-declaring in the new crate.
- Some clap subcommands share helper functions (e.g. `resolve_workspace_root`, `default_run_result_dir`). Hoist these into `quorp_cli/src/quorp_cli.rs` first; do not duplicate.
- The interactive `run_inline_cli` reaches into `quorp::quorp::tui::chat_service::*` paths today. Phase 4-C will move those, but for 4-B you must keep imports stable — leave the chat_service module *in place* during 4-B and only update import paths in 4-C. Otherwise you'll have to do both phases as one mega-commit, which is the kind of high-blast-radius change we explicitly avoided in 4-A.

**Why this scope, not full Phase 4 (cli + session + mcp combined)**: We split Phase 4 deliberately. `quorp_mcp` was 4-A. `quorp_cli` is 4-B because it's smaller and lower-risk than `quorp_session`. Doing them serially proves the playbook at each scale.

---

### 3.2 Phase 4-C — Extract `quorp_session`

**Status**: done in the current worktree.

**Why**: After 4-B, the remaining `crates/quorp/src/quorp/` content is the chat-service tree (turn_parse, tools_schema, provider, transcript, etc.) plus `agent_runner.rs`. These are session-lifecycle concerns: streaming an LLM turn, parsing its output, dispatching tool calls, recording transcripts, persisting sessions. They now live in `quorp_session`.

**Scope**: ~3,500 LOC moved.
- `crates/quorp/src/quorp/agent_runner.rs` (1,190) → `crates/quorp_session/src/headless/{headless,recorder,clients,bridge}.rs` (~150 + ~500 + ~300 + ~250).
- `crates/quorp/src/quorp/tui/chat_service.rs` (371) → `crates/quorp_session/src/chat_service/chat_service.rs`.
- `crates/quorp/src/quorp/tui/chat_service/turn_parse.rs` (1,457) → `crates/quorp_session/src/chat_service/turn_parse/{turn_parse,streaming,structured,recovery}.rs` (~150 + ~500 + ~400 + ~400).
- `crates/quorp/src/quorp/tui/chat_service/tools_schema.rs` (1,097) → `crates/quorp_session/src/chat_service/tools_schema/{tools_schema,read_tools,write_tools,search_tools,meta_tools}.rs` (~120 + ~250×4).
- `crates/quorp/src/quorp/tui/chat_service/provider.rs` and any other siblings move along.

**Playbook**: Same as 4-B. Scaffold crate, add to workspace, move bodies verbatim into smaller siblings, leave thin re-export shims at the old `crate::quorp::tui::chat_service::*` paths so `quorp_cli` (now in 4-B) keeps resolving without a coordinated cross-crate diff. Once 4-C is green, *delete* the shims in a follow-up commit and update `quorp_cli`'s imports to depend on `quorp_session::*` directly.

**Tests to add**:
- `turn_parse/streaming.rs` golden tests on canned SSE chunks → expected `AgentTurnResponse`.
- `turn_parse/structured.rs` round-trip on the strict-JSON path.
- `turn_parse/recovery.rs` simulating malformed model output and asserting the repair-hint emission.
- `tools_schema/*` per-tool schema-emission tests (verify the JSON schema matches what the runtime expects).
- `headless/recorder.rs` transcript persistence: write/load round-trip on a `tempfile::tempdir()`.

**Risks**:
- The chat-service tree is the **hottest path** in the binary. Every subcommand that runs an LLM turn touches it. Verify `quorp run`, `quorp exec`, and the inline CLI all still work end-to-end after the move.
- `turn_parse.rs` has subtle state machines for streamed tool calls. Don't rename or re-order match arms during the move — that obscures the "did I break it?" signal in the diff.
- `agent_runner.rs` owns the headless event recorder which several benchmark integration tests depend on. Run `cargo test -p quorp_benchmark --tests -- --test-threads=2` specifically after the move.

---

### 3.3 Phase 10 — Wire smart-tooling crates into the agent loop

**Why**: Phases 6/7/8 created the smart-tooling crate skeletons (`quorp_storage`, `quorp_repo_scan`, `quorp_memory`, `quorp_rule_forge`, `quorp_context`, `quorp_patch_vm`, `quorp_verify`, `quorp_rust_intel`, `quorp_permissions`, `quorp_plan_mode`, `quorp_slash`). They compile and pass their own unit tests. **They are not yet called from the runtime.** Phase 10 is what makes the smart-tooling vision real.

**Scope** (each is its own commit; do not bundle):

1. **`quorp_session::chat_service` calls `quorp_context::compile`** to build the prompt. Replaces today's "inline a few files into the system prompt" with a proper `ContextPack` assembled from `RepoGraph` + `MemoryStore` + open-files. Add a new agent action `ExpandContext { handle }` so the model can request more on demand.
2. **`quorp_agent_core::runtime::turn_loop` publishes `AgentEvent` on a `tokio::sync::broadcast`.** Subscribers: `quorp_memory` (writes episodic + working-memory entries), `quorp_rule_forge` (mines failure signatures), `quorp_render` (drives the shimmer state machine). The broadcast channel is cheap (~32 entries deep) and decouples the runtime from the smart-tooling layer.
3. **`RunValidation` action delegates to `quorp_verify::verify`.** Maps the old freeform "run cargo test" into a `VerifyPlan { level: VerifyLevel::L2Targeted, targets, time_budget, fail_fast: false }`. Cache hits show "L2 (cached, 0ms)" in the renderer.
4. **`WriteFile` / `ApplyPatch` / `ReplaceBlock` / `ReplaceRange` / `ModifyToml` delegate to `quorp_patch_vm::apply`.** The VM enforces three gates per op: (1) preconditions read-only against `RepoGraph`, (2) tree-sitter re-parse of produced text, (3) optional rust-analyzer query. Each op records a `RollbackToken { file, pre_image_hash, previous_bytes }` for O(1) rollback.
5. **New tool actions surfaced in `tools_schema`**: `ExpandContext`, `RecallMemory`, `ProposeRule`. These are the agent's hooks into the smart-tooling layer.

**Order matters**:
- Do (1) first — it's read-only and won't regress existing turns. Add a feature flag `--use-context-compiler` so you can A/B against the legacy inline path.
- Do (2) second — broadcast publication is additive; subscribers are no-ops at first.
- (3) and (4) are the high-blast-radius changes. Land each as its own commit with a feature flag (`--use-verify-vm`, `--use-patch-vm`). Keep flags off by default until integration tests catch up.
- (5) is the easiest — just adds new schema entries; the runtime ignores unknown actions today.

**Tests**:
- Golden `ContextPack` tests on a canned workspace fixture.
- Cache-key invariant tests for `quorp_verify` (same inputs → same key, different inputs → different key).
- End-to-end "patch + rollback" test for `quorp_patch_vm` on a `tempfile::tempdir()` workspace.
- Self-correction test: scripted scenario where `RunValidation` fails once, agent enters repair attempt 1/3, succeeds on retry, transcript shows the indented `↳ ` block.

**Risks**:
- The runtime is the most-tested code on the branch (8,315-LOC test file at `runtime/tests.rs`). Wiring smart-tooling into it will likely require updating ~20-50 of those tests to acknowledge the new event publication / context-compiler invocation. Plan for it; don't be surprised.
- The broadcast channel's backpressure: if a slow subscriber falls behind, `tokio::sync::broadcast` drops messages. Make sure that's the desired semantics — `quorp_memory` wants to lose nothing; consider an `mpsc` per subscriber instead, or wrap the slow subscriber in a `spawn` + bounded queue.

---

### 3.4 Phase 11 — Tests, polish, docs (the polish layer)

**Scope** (parallel-friendly, can be split across multiple commits):

1. **Split `runtime/tests.rs` (8,315 LOC) along fixture boundaries.** This file is *test code* so the cap was never meant for it, but the script doesn't know that and CI is currently red. Split into siblings keyed off the `mod` blocks already present: `tests/state.rs`, `tests/turn_loop.rs`, `tests/parser.rs`, `tests/benchmark_repair.rs`, etc. Each under 2,000 LOC.
2. **Split `benchmark/tests.rs` (3,305 LOC)** likewise. Look for `mod` boundaries by feature area.
3. **Add property tests** on the smart-tooling crate APIs:
   - `quorp_context::compile` budget invariants (output `budget_used <= budget.total`).
   - `quorp_patch_vm::apply` round-trip (apply + rollback returns to identical bytes).
   - `quorp_verify` cache key collision-resistance (proptest with random env / git_sha / changed_files).
4. **Add `quorp_render` snapshot tests**: deterministic time injection → identical shimmer frames; permission modal screenshot tests via `insta`.
5. **`quorp_rule_forge` end-to-end test**: simulated failure stream → Candidate → Draft → Verified state transitions, with assertions on `memory_rule.state` at each step.
6. **Update `.rules`** with the new "no `let _ =` on fallible", "no `mod.rs`", "lib path required in Cargo.toml" rules — these have come up repeatedly.
7. **CI matrix**: `just fast` on every PR, `just medium` on PR-ready, `just deep` nightly, `just security` weekly. Wire via additional GitHub Actions workflows alongside `loc-cap.yml`.
8. **Docs**: update `AGENT_SUPPORT.md` (already partially done in §1.1 WIP), update `README.md` if present, and add per-crate one-paragraph headers to each `crates/quorp_*/src/*.rs` lib root.

---

### 3.5 Phase 12-follow-up — LOC cap is already fixed

The current branch already patches `script/check-loc-cap` to skip vendored utility crates and test files. The existing CI workflow is green on the current workspace, so this section is now a verification note rather than an action item.

---

## 4. Crate map (current state)

```
crates/
├── collections/             vendored (zed.dev) - workspace member
├── paths/                   vendored (zed.dev) - workspace member
├── perf/                    vendored (zed.dev) - workspace member
├── util/                    vendored (zed.dev) - workspace member, has 3144-LOC paths.rs
├── util_macros/             vendored (zed.dev) - workspace member
├── zlog/                    vendored (zed.dev) - workspace member
├── zlog-compat/             vendored (zed.dev) - workspace member
├── ztracing/                vendored (zed.dev) - workspace member
├── ztracing-compat/         vendored (zed.dev) - workspace member
├── ztracing_macro/          vendored (zed.dev) - workspace member
│
├── quorp/                   binary (thin wrapper main.rs)
├── quorp_cli/              extracted CLI/runtime crate (Phase 4-B done)
│
├── quorp_core/              shared value types (RunMode, PermissionMode, etc.)
├── quorp_ids/               newtype IDs + E_* error codes (Phase 1)
├── quorp_agent_protocol/    wire types: AgentAction, AgentTurnResponse, wire envelopes (Phase 1 + protocol split)
├── quorp_repo_graph/        domain: file/symbol/import graph (Phase 1)
├── quorp_context_model/     domain: ContextPack, ContextItem (Phase 1)
├── quorp_memory_model/      domain: episodic/semantic/procedural (Phase 1)
├── quorp_rule_model/        domain: RulePattern, Lifecycle, RuleEffect (Phase 1)
├── quorp_patch_model/       domain: PatchOp, AnchorMatch, RollbackToken (Phase 1)
├── quorp_verify_model/      domain: VerifyLevel, VerifyPlan, StageReport (Phase 1)
│
├── quorp_agent_core/        runtime — 766 LOC root + 11 sibling modules (Phase 3 done)
├── quorp_benchmark/         absorbed benchmark.rs (Phase 2 done)
├── quorp_config/            settings + env loading
├── quorp_context/           Context Compiler skeleton (Phase 7) - NOT YET WIRED
├── quorp_memory/            Memory OS skeleton (Phase 6) - NOT YET WIRED
├── quorp_rule_forge/        Failure→rule miner skeleton (Phase 6) - NOT YET WIRED
├── quorp_patch_vm/          Semantic patch ops skeleton (Phase 7) - NOT YET WIRED
├── quorp_verify/            Verification ladder skeleton (Phase 7) - NOT YET WIRED
├── quorp_rust_intel/        Borrow doctor + trait explainer skeleton (Phase 7)
├── quorp_storage/           rusqlite + tantivy + usearch + fastembed (Phase 6)
├── quorp_repo_scan/         tree-sitter + notify (Phase 6) - WIRED into `quorp scan`
├── quorp_permissions/       5-mode policy engine (Phase 8) - WIRED into `quorp permissions`
├── quorp_plan_mode/         Plan↔Act state machine (Phase 8) - NOT YET WIRED into runtime
├── quorp_slash/             Slash-command registry (Phase 8) - WIRED into renderer demos
├── quorp_render/            Brilliant CLI renderer (Phase 9) - WIRED into 5 production subcommands; inline CLI wiring is already in the current worktree
├── quorp_provider/          OpenAI-compatible HTTP client (existing)
├── quorp_sandbox/           tmp-copy + worktree shadow (existing)
├── quorp_term/              PTY/exec primitives (existing)
├── quorp_tools/             read/write/search/patch/git/shell executors
├── quorp_mcp/               MCP JSON-RPC client (Phase 4-A done) - 560 LOC + 10 tests
└── quorp_session/           extracted session/runtime crate (Phase 4-C done)
```

**Total workspace members today**: 40 crates.

---

## 5. The verification ritual (run before every commit)

```bash
# Fast lane (always)
cargo fmt --all -- --check
cargo check --workspace

# Medium lane (before commit)
cargo clippy --workspace --all-targets --no-deps -- -D warnings
cargo test --workspace --exclude util --tests -- --test-threads=2

# LOC cap (CI mirror)
./script/check-loc-cap 2000 --error

# Smoke test (the runtime still works)
cargo build --release
./target/release/quorp doctor                       # boots, prints cards
./target/release/quorp --help                       # all 8 subcommands listed
./target/release/quorp scan                         # walks crates/, prints languages
./target/release/quorp permissions check write_file foo.txt   # prints decision
./target/release/quorp render-demo                  # shimmer renders
```

If any of these fail, do not commit. Investigate.

**Test parallelism note**: `--test-threads=2` is required for the workspace test run. The benchmark integration tests share `tempfile::tempdir()` state under the hood and flake under high parallelism. Don't drop the flag without first auditing why.

---

## 6. The user's working style (from prior sessions)

- The user runs `/loop` or `/auto` and expects the agent to ship coherent commits one at a time, each green, each revertible.
- "Big bang" means *one execution arc*, not *one mega-commit*. Many commits, each phase its own commit, is what was asked for.
- The user explicitly values **test coverage built alongside the code**, not bolted on at the end. Phase 10's lack of tests today is the biggest deviation from this; Phase 11 must close that gap.
- The user does not want `mod.rs` files. Ever. New crates use `[lib] path = "src/<name>.rs"`.
- The user has stated multiple times: "act like an owner." That means: when there's a routing decision (split this commit? bundle these phases? defer this risk?), make the call and move forward. Document the call in the commit message.
- The user has *not* asked for performance work yet. Don't get sidetracked optimizing — the plan's perf budgets are aspirational targets, not Phase-10 deliverables. They become real in Phase 11.
- When you finish a phase, write **one or two sentences** summarizing what changed and what's next. Then stop and wait for the next instruction. Do not auto-continue across phase boundaries unless the user has explicitly enabled `/auto`.

---

## 7. Known risks and decision logs

### 7.1 Test parallelism flake
Symptom: full-workspace test runs fail intermittently in `quorp_benchmark` integration tests. Root cause: shared tempdir or global ENV mutation (not yet root-caused). Workaround: `--test-threads=2`. Fix-forward: Phase 11 task — audit `crates/quorp_benchmark/tests/*` for `std::env::set_var` or shared-tempdir anti-patterns, isolate per-test.

### 7.2 Sed-based bulk pub(crate) promotion footgun
During Phase 3's runtime split, a `sed -E 's/^(    )fn /\1pub(crate) fn /g'` pass accidentally rewrote function definitions inside `r#"..."#` raw-string literals (e.g. inside `cc-rs` patcher fixtures). Fix: a second `awk` pass that tracks `r#"` nesting depth and reverts insertions inside raw strings. **Lesson for the next agent**: never use `sed` for visibility promotion. Use `rust-analyzer`-driven edits or hand-edit one block at a time. The cost-benefit is wildly in favor of the slower path.

### 7.3 Untagged enum variant ordering
`IncomingMessage` in `quorp_mcp` is `#[serde(untagged)]` over `Response | Notification | Request`. Untagged greedy-matches **in declaration order**. A request-shaped message with `id + method + params` but no `result/error` matches `Response` first. The runtime impact is nil (dispatch only treats id-bearing messages with `result` or `error` as responses), but the test for this case asserts the surprising-but-actual behavior with an explanatory comment. Don't "fix" it without understanding the ramifications.

### 7.4 `serde(rename_all = "camelCase")` quirk
On enum variants, `rename_all` applies to **variant names**, not to internal struct fields. `CallToolResultContent` has `#[serde(tag = "type", rename_all = "camelCase")]` but its fields are still snake_case in the wire format. Tests must use `mime_type`, not `mimeType`.

---

## 8. File pointers (clickable for the next agent)

- Plan: `/Users/bentaylor/.claude/plans/can-you-please-study-mighty-pudding.md`
- This handoff: `/Users/bentaylor/Code/quorp/HANDOFF.md`
- Repo CLAUDE.md (project rules): `/Users/bentaylor/Code/quorp/CLAUDE.md`
- `.rules` (agent rules): `/Users/bentaylor/Code/quorp/.rules` (read it!)
- Agent metadata: `/Users/bentaylor/Code/quorp/agent/` (owner-map, proof-lanes, etc.)
- LOC cap script: `/Users/bentaylor/Code/quorp/script/check-loc-cap`
- CI workflow: `/Users/bentaylor/Code/quorp/.github/workflows/loc-cap.yml`
- Justfile: `/Users/bentaylor/Code/quorp/justfile`

---

## 9. Recommended first session

If you take this branch over today, here's the order:

1. **Read this file end-to-end.** Then read the plan file, then the user's most recent messages in `/Users/bentaylor/.claude/projects/-Users-bentaylor-Code-quorp/09864b7d-e99a-4187-b7ec-c32fd63a84c0.jsonl` if you want the verbatim history.
2. **Resolve §1.1 WIP.** Decide whether to commit the remaining renderer/doc polish as a small follow-up, then move on.
3. **Verify §1.2 LOC-cap status.** Keep the current `script/check-loc-cap` exclusions intact and confirm the workflow stays green. ~15 minutes.
4. **Start Phase 10** per §3.3. The CLI and session extractions are now complete; the next planned commit on the critical path is wiring the smart-tooling crates into the runtime.
5. **Stop after Phase 10 lands.** Wait for user direction before starting the later polish phases — the user has been pacing this manually one phase at a time since Phase 3.

Total estimated effort for the remaining smart-tooling work is 10+ hours and should be staged as 5 separate commits.

---

## 10. One-line summary for the impatient

> Branch is green and compiles, the workspace tests pass, and the LOC cap is clean. The remaining work is the later smart-tooling phases.

Good luck. Act like an owner.

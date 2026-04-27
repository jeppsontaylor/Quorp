# Quorp — Big-Bang Refactor Handoff

**Branch**: `codex/agent-first-cleanup`
**Last commit**: `a6a3974 — Phase 4-A + 12: extract quorp_mcp crate, add CI loc-cap enforcement`
**Date written**: 2026-04-26
**Author of this handoff**: outgoing agent (Claude Opus 4.7, 1M ctx)
**Audience**: incoming agent picking up the same plan

> Read this end-to-end before touching anything. There is uncommitted in-progress work and a CI gap that will bite you on the very first push if you don't address them. Sections marked **⚠ CRITICAL** describe foot-guns waiting for you.

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

`git status --short` reports **71 modified files**, `git diff --shortstat` reports **+12,786 / −12,577** lines. **The working tree compiles cleanly** (`cargo check --workspace` is green), so this is not a broken half-edit — it is *partially-completed real work*.

The largest substantive deltas:

| File | +/− | Nature of change |
|---|---|---|
| `AGENT_SUPPORT.md` | +100 / −699 | Docs rewrite collapsing the legacy SSD-MOE / TUI references and reframing as the current native shape. Safe to keep or commit standalone. |
| `crates/quorp/src/main.rs` | +36 / −23 | Replaces `quorp_term::startup_card` / `render_card` calls inside `run_inline_cli` and `run_inline_task` with the newer `quorp_render::render_session_frame` and `render_command_card`. **In-progress wiring** — confirm both subcommands still display correctly. |
| `crates/quorp/src/quorp/cli_demos.rs` | +110 / −27 | Same renderer migration as above for the demo subcommands. Pulls in `quorp_render::session::{CommandCard, CommandState, SessionFrame, TaskRow, TaskState, render_session_frame}` etc. Compiles. |
| `crates/quorp/src/quorp/benchmark.rs` | +? / −? | Smaller cleanup. Verify intent. |
| `crates/quorp_benchmark/src/lib.rs` | +? / −? | Smaller cleanup. Verify intent. |
| ~60 other files | mostly formatter | Bulk formatter pass (`cargo fmt`) ran across the workspace. Mostly whitespace/import-grouping noise. |

**What to do first:**
1. `git diff --stat` and skim the list. Sort the substantive ones from the formatter noise.
2. Decide whether to commit the work-in-progress as a "Phase 9-final wire-up of quorp_render into inline CLI" commit, or keep going on top of it. The author's recommendation: commit it as its own logical commit before starting Phase 4-B, so the next branch baseline is clean.
3. **Do not** `git restore .` — you will lose the renderer wiring work that is already done.

### 1.2 ⚠ CI loc-cap workflow currently fails

`Phase 12` added `.github/workflows/loc-cap.yml`, which runs `script/check-loc-cap 2000 --error`. That script counts every `crates/**/*.rs` file uniformly — **it does not exclude test files or vendored utility crates**. Today it reports three offenders:

```
  8315  crates/quorp_agent_core/src/runtime/tests.rs
  3305  crates/quorp/src/quorp/benchmark/tests.rs
  3144  crates/util/src/paths.rs
```

The first two are test files (Phase 11 splits them by fixture boundaries). The third is a vendored utility crate from the zed.dev import (it's a workspace member but not "quorp production code"; the user's milestone of "zero production-code files exceed 2,000 LOC" was about `quorp_*` crates).

**This means CI is red on this branch right now.** The next push will fail unless you do one of:

- **Option A (preferred, fastest):** patch `script/check-loc-cap` so it skips `crates/util/`, `crates/paths/`, `crates/collections/`, `crates/perf/` (the four vendored utility crates kept from zed.dev) **and** any path matching `*/tests.rs` or `*/tests/*`. Then update `loc-cap.yml` to keep firing on the rest. This is ~10 lines of bash.
- **Option B:** carve `runtime/tests.rs` and `benchmark/tests.rs` along fixture boundaries (Phase 11) and live with `util/src/paths.rs` being excluded via Option A's util-skip.
- **Option C:** lower the CI cap to 800 with `--warn` and wait until Phase 11 to enforce 2000 hard. Loses the milestone lock.

The author's recommendation: **Option A first** (immediate, ~15 minutes), then **Phase 11** (the planned split) when the broader work allows it. Track this as a **Phase 12 follow-up** in the plan file.

### 1.3 Current LOC snapshot of the still-oversize files

```
1777  crates/quorp/src/main.rs                                       <- Phase 4-B scope
1253  crates/quorp/src/quorp/run_support.rs                          <- Phase 4-B scope
1190  crates/quorp/src/quorp/agent_runner.rs                         <- Phase 4-C scope
1457  crates/quorp/src/quorp/tui/chat_service/turn_parse.rs          <- Phase 4-C scope
1097  crates/quorp/src/quorp/tui/chat_service/tools_schema.rs        <- Phase 4-C scope
 371  crates/quorp/src/quorp/tui/chat_service.rs                     <- moves with Phase 4-C, already small
8315  crates/quorp_agent_core/src/runtime/tests.rs                   <- Phase 11
3305  crates/quorp/src/quorp/benchmark/tests.rs                      <- Phase 11
3144  crates/util/src/paths.rs                                       <- vendored, exclude via script
```

All are under the 2000 cap **for production code** except via the script's broad-stroke counting (see §1.2).

---

## 2. What's done (so the next agent doesn't redo it)

| Phase | Commit | Status | What it delivered |
|---|---|---|---|
| 0 — Hygiene | `b445842` | ✓ | Dead zed.dev crates purged, justfile + check-loc-cap added, codex_claude_copy/*.txt recovered |
| 1 — Foundations | `7c21c5b` | ✓ | 8 domain crates: `quorp_ids`, `quorp_agent_protocol`, `quorp_repo_graph`, `quorp_context_model`, `quorp_memory_model`, `quorp_rule_model`, `quorp_patch_model`, `quorp_verify_model` |
| 2 — Benchmark extraction | `d2a6d27`, `da09246`, `a806c6c` | ✓ | `benchmark.rs` (5,548) split into 5 sibling files all under cap; existing `quorp_benchmark` crate already absorbed the heavy challenge/runner code |
| 3 — Runtime split | `5c78753`, `2e4d2b3`, `7deb61c`, `0670915` | ✓ | `runtime.rs` (20,755) → 766 LOC root + 11 sibling modules; `agent_turn.rs` split into parser + render + tests |
| 4-A — `quorp_mcp` extraction | `a6a3974` | ✓ | New crate at `crates/quorp_mcp/`, 560 LOC + 8 round-trip tests; original `tui/mcp_client.rs` collapsed to a 5-line `pub use quorp_mcp::*;` shim |
| 4-B — `quorp_cli` extraction | — | **PENDING** | See §3 |
| 4-C — `quorp_session` extraction | — | **PENDING** | See §3 |
| 5 — `native_backend` split | `058b1e3`, `022f639` | ✓ | `native_backend.rs` reduced from 3,440 → 1,725 LOC; `actions.rs` sibling extracted |
| 6 — Storage + repo_scan + memory + rule_forge skeletons | `851454e` | ✓ | Skeleton crates compile and pass tests; not yet wired into the runtime |
| 7 — Context + patch_vm + verify + rust_intel skeletons | `851454e` | ✓ | Same — skeletons only |
| 8 — Permissions + plan_mode + slash | `851454e` | ✓ | Skeletons compile + pass tests |
| 9 — Brilliant CLI renderer | `851454e`, `09e9da4`, `8f3095a`, `f0bde4d`, `d63e17e` | ✓ (with WIP polish) | `quorp_render` wired into 5 production subcommands. **The uncommitted work in §1.1 is finishing this off** by replacing remaining `quorp_term` card calls with `quorp_render::render_session_frame`/`render_command_card` in `run_inline_cli` and `run_inline_task` |
| 10 — Wire smart tooling into agent loop | — | **PENDING** | See §3 |
| 11 — Tests, polish, docs | — | **PENDING** | See §3 |
| 12 — CI loc-cap enforcement | `a6a3974` | ✓ (broken, see §1.2) | `.github/workflows/loc-cap.yml` exists but currently fails |

**Test baseline at `a6a3974`:** 436/436 tests pass with `cargo test --workspace --exclude util --tests -- --test-threads=2` across 39 suites. (Exclude `util` because it has its own tests that flake under high parallelism; use `--test-threads=2` to avoid integration-test parallelism flakes in the benchmark crate.)

---

## 3. Pending phases — full detail

### 3.1 Phase 4-B — Extract `quorp_cli`

**Why**: `crates/quorp/src/main.rs` is 1,777 LOC. It hosts the clap parser, eight subcommands (`run`, `exec`, `agent`, `session`, `benchmark`, `doctor`, `diagnostics`, plus the inline default), and a sprawling `run_inline_cli` / `run_inline_task` interactive loop. Pulling this out:
- Lets the binary shrink to ~80 LOC (`fn main() { std::process::exit(quorp_cli::dispatch().unwrap_or(1)); }`).
- Lets `quorp_session` (Phase 4-C) cleanly absorb the chat-service modules without circular deps.
- Lets `run_support.rs` (1,253 LOC) move with its callers and split along the `launch / doctor / receipts` boundary the plan specifies.

**Scope (LOC moved)**: ~3,000 LOC across `main.rs` (1,777) + `run_support.rs` (1,253). After the move:
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
   - `cargo test --workspace --exclude util --tests -- --test-threads=2` — still 436+ passing.
   - `./script/check-loc-cap 2000 --error` — should still be green for `quorp_*` crates.
   - **Runtime smoke test**: `cargo build --release && ./target/release/quorp doctor` and `./target/release/quorp --help` both emit the expected output. Then `./target/release/quorp scan` and `./target/release/quorp permissions check write_file foo`. The output should be byte-identical to the pre-extraction version (modulo any renderer migration carried in from §1.1 WIP).

**Risks / foot-guns**:
- `main.rs` has a global `static` for the runtime config that several commands reach into. When moving, make sure the lifetime / lazy-init semantics match — pass it explicitly through `dispatch()` rather than re-declaring in the new crate.
- Some clap subcommands share helper functions (e.g. `resolve_workspace_root`, `default_run_result_dir`). Hoist these into `quorp_cli/src/quorp_cli.rs` first; do not duplicate.
- The interactive `run_inline_cli` reaches into `quorp::quorp::tui::chat_service::*` paths today. Phase 4-C will move those, but for 4-B you must keep imports stable — leave the chat_service module *in place* during 4-B and only update import paths in 4-C. Otherwise you'll have to do both phases as one mega-commit, which is the kind of high-blast-radius change we explicitly avoided in 4-A.

**Why this scope, not full Phase 4 (cli + session + mcp combined)**: We split Phase 4 deliberately. `quorp_mcp` was 4-A. `quorp_cli` is 4-B because it's smaller and lower-risk than `quorp_session`. Doing them serially proves the playbook at each scale.

---

### 3.2 Phase 4-C — Extract `quorp_session`

**Why**: After 4-B, the remaining `crates/quorp/src/quorp/` content is the chat-service tree (turn_parse, tools_schema, provider, transcript, etc.) plus `agent_runner.rs`. These are session-lifecycle concerns: streaming an LLM turn, parsing its output, dispatching tool calls, recording transcripts, persisting sessions. They want their own crate.

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

### 3.5 Phase 12-follow-up — Fix the loc-cap script

See §1.2. Patch `script/check-loc-cap` to skip:
- `crates/util/`, `crates/paths/`, `crates/collections/`, `crates/perf/` (vendored zed.dev utility crates).
- Any path matching `*/tests.rs` or `*/tests/*` (test code, not production code).
- `crates/zlog*/`, `crates/ztracing*/` if any of those are oversize (currently they aren't).

Suggested diff (~10 lines):
```bash
case "$file" in
    */target/*|*/.git/*|*/tests/fixtures/*|*/snapshots/*) continue ;;
    crates/util/*|crates/paths/*|crates/collections/*|crates/perf/*) continue ;;
    */tests.rs|*/tests/*) continue ;;
esac
```

After the patch, `./script/check-loc-cap 2000 --error` returns 0 immediately, and the existing CI workflow goes green.

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
├── quorp/                   binary (1,777 LOC main.rs - Phase 4-B target)
│
├── quorp_core/              shared value types (RunMode, PermissionMode, etc.)
├── quorp_ids/               newtype IDs + E_* error codes (Phase 1)
├── quorp_agent_protocol/    wire types: AgentAction, AgentTurnResponse (Phase 1)
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
├── quorp_render/            Brilliant CLI renderer (Phase 9) - WIRED into 5 production subcommands; in-progress wiring into inline CLI in §1.1 WIP
├── quorp_provider/          OpenAI-compatible HTTP client (existing)
├── quorp_sandbox/           tmp-copy + worktree shadow (existing)
├── quorp_term/              PTY/exec primitives (existing)
├── quorp_tools/             read/write/search/patch/git/shell executors
├── quorp_mcp/               MCP JSON-RPC client (Phase 4-A done) - 560 LOC + 10 tests
└── (PENDING: quorp_cli, quorp_session)
```

**Total workspace members today**: 38 crates. After 4-B + 4-C: 40.

---

## 5. The verification ritual (run before every commit)

```bash
# Fast lane (always)
cargo fmt --all -- --check
cargo check --workspace

# Medium lane (before commit)
cargo clippy --workspace --all-targets --no-deps -- -D warnings
cargo test --workspace --exclude util --tests -- --test-threads=2

# LOC cap (CI mirror — fix the script first per §3.5)
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
2. **Resolve §1.1 WIP.** Decide: commit it as `Phase 9 polish: wire quorp_render::session into inline CLI` or finish it before committing? Recommended: commit it cleanly, then move on.
3. **Resolve §1.2 CI gap.** Patch `script/check-loc-cap` per §3.5. Push. Verify CI is green. ~15 minutes.
4. **Start Phase 4-B** per §3.1. This is the next planned commit on the critical path.
5. **Stop after 4-B lands.** Wait for user direction before starting 4-C — the user has been pacing this manually one phase at a time since Phase 3.

Total estimated effort to get through Phase 4-B + 4-C: 4-6 focused hours of agent time. Phase 10 is much larger (10+ hours) and should be staged as 5 separate commits.

---

## 10. One-line summary for the impatient

> Branch is green and compiles, but has 71 uncommitted files (Phase-9 renderer wiring WIP, mostly benign) and a CI workflow that fails on the `util/src/paths.rs` + two test-file LOC offenders. Fix the CI script, commit the WIP, then start Phase 4-B (`quorp_cli` extraction from `main.rs`).

Good luck. Act like an owner.

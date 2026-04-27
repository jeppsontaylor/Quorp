# Quorp World-Class Agent Plan

This tracker turns the product plan into verifiable phases. A phase is complete only when its listed checks pass or the gap is explicitly recorded.

## Phase 0: Restore Trust

Status: complete

Scope:
- Make the current workspace tests and proof lanes reliable.
- Remove stale support claims that contradict the current code.
- Keep the terminal-first, NVIDIA/OpenAI-compatible, sandboxed-agent direction explicit.

Verification:
- `cargo check --workspace`
- `cargo test --workspace --lib`
- `./script/clippy`
- `just medium`

Open work:
- None for this phase.

Completed:
- Fixed current `cargo test --workspace --lib` failures.
- Reduced `cargo check --workspace` to a warning-clean pass.
- Reduced `./script/clippy` to a clean strict pass.
- Refreshed `AGENT_SUPPORT.md` against the actual crate tree.
- Verified `cargo fmt --all --check`.
- Verified `cargo check --workspace`.
- Verified `cargo test --workspace --lib` with 421 passing tests across 39 suites.
- Verified `./script/clippy`.
- Verified `just medium`.

## Phase 1: Signature Terminal UX

Status: complete

Scope:
- Make the stream-first truecolor renderer the default session surface.
- Show active commands with oscillating color, stable cards, and no-color fallback.
- Add deterministic ANSI snapshot coverage.

Verification:
- Renderer unit tests.
- ANSI/no-color snapshot tests.
- Manual `quorp render-demo` capture.

Completed:
- Added a stream-first `quorp_render::session` scene renderer for the brand header, task list, active command card, completed command card, and footer.
- Added active command shimmer inside stable-width command cards with no-color fallback.
- Wired the default inline CLI startup and task launch surfaces to the new session renderer.
- Wired `quorp render-demo` to show the Phase 1 session scene.
- Added static demo mode for deterministic capture with `QUORP_RENDER_DEMO_STATIC=1`.
- Added paper-friendly capture docs and SVG at `docs/src/development/phase-1-terminal-ux.md` and `docs/src/development/phase-1-terminal-ux.svg`.
- Verified `cargo test -p quorp_render --lib` with 19 passing tests.
- Verified `NO_COLOR=1 QUORP_RENDER_DEMO_STATIC=1 cargo run -q -p quorp -- render-demo`.
- Verified `./script/clippy`.
- Verified `just medium`.

## Phase 2: Provider And Session Core

Status: complete

Scope:
- Treat NVIDIA/Qwen as a default OpenAI-compatible provider profile, not a hardcoded runtime.
- Normalize streaming events, request IDs, usage, backoff, raw artifact hashes, and session receipts.

Verification:
- Mock provider streaming tests.
- Rate-limit and malformed-SSE tests.
- Headless `quorp run` smoke with a loopback mock.

Completed:
- Added `OpenAiCompatibleEndpoint` so provider profile normalization is reusable and receipt-friendly.
- Moved OpenAI-compatible base URL normalization into `quorp_provider`.
- Added stream chunk parsing that preserves text/reasoning events, provider request IDs, model IDs, finish reasons, usage, and raw SSE payload SHA-256 hashes.
- Added provider-level retry-after/backoff helpers and wired NVIDIA rate-limit backoff through them.
- Wired the streaming completion path to consume normalized stream chunks and attach `stream_payload_sha256` values to raw provider responses.
- Verified provider profile, URL, stream metadata, usage, malformed-SSE, and retry-backoff tests with `cargo test -p quorp_provider --lib`.
- Verified loopback mock provider integration with `cargo test -p quorp --bin quorp benchmark_run_completes_with_fake_model_server`.
- Verified `./script/clippy`.
- Verified `just medium`.

## Phase 3: Sandbox, Permissions, And Tool VM

Status: complete

Scope:
- Prefer git-worktree sandboxes for git repositories and tmp-copy fallback elsewhere.
- Make `--yolo` mean full auto inside a sandbox.
- Route all filesystem, shell, network, and MCP actions through typed permission checks.

Verification:
- Sandbox isolation tests.
- Permission policy matrix tests.
- Rollback and audit-log tests.

Completed:
- Added a git-worktree sandbox backend for git repositories, with tmp-copy fallback for non-git workspaces or failed worktree setup.
- Added `SandboxBackend` metadata and exposed sandbox backend/source paths for run artifacts.
- Added sandbox isolation tests for tmp-copy and git-worktree backends.
- Added `--yolo` to `quorp run` and `quorp exec`; it forces tmp-copy sandboxing and `autonomous_sandboxed` autonomy.
- Rejected `--yolo --sandbox host` so full auto does not run directly on the source checkout.
- Added sandbox-aware yolo policy enforcement: `YoloSandbox` allows actions only on the sandbox execution surface.
- Added typed tool action classification for reads, writes, deletes, shell commands, network commands, and MCP calls.
- Wired the permissions demo through the shared classifier.
- Verified `cargo test -p quorp_sandbox --lib`.
- Verified `cargo test -p quorp_permissions --lib`.
- Verified `cargo test -p quorp --bin quorp yolo_`.
- Verified `./script/clippy`.
- Verified `just medium`.

## Phase 4: Patch VM

Status: complete

Scope:
- Make semantic patch operations the preferred write path.
- Require hashes or preview IDs for risky edits.
- Reject ambiguous full-file rewrites unless explicitly permitted.

Verification:
- Patch VM unit and property tests.
- Corruption and rollback tests.
- Multi-file diff tests.

Completed:
- Reviewed `/Users/bentaylor/.claude/plans/can-you-please-study-mighty-pudding.md`; its MCP extraction recommendation is already present in this workspace, so Phase 4 stayed focused on the existing Patch VM gap.
- Expanded `quorp_patch_vm` from hash validation into a file-change VM with previews, stable preview IDs, touched-path receipts, rollback tokens, hash preconditions, high-risk gating, and large full-file rewrite rejection by default.
- Routed `quorp_tools` unified-diff application through `PatchVm` so add/update/delete/move writes use the same preflight and apply path.
- Added tests for hash-only low-risk updates, high-risk multi-file preview enforcement, stale-hash rejection, and full-file rewrite policy rejection.
- Verified `cargo test -p quorp_patch_vm --lib`.
- Verified `cargo test -p quorp_patch_model --lib`.
- Verified `cargo test -p quorp_tools --lib`.
- Verified `cargo test -p quorp --bin quorp apply_patch_task`.
- Verified `cargo fmt --all --check`.
- Verified `./script/clippy`.
- Verified `just medium`.

## Phase 5: Context OS

Status: complete

Scope:
- Replace regex-only repo scanning with Cargo metadata, rust-analyzer/Tree-sitter-backed symbols, lexical search, and context packs.
- Integrate owner map, test map, generated zones, and proof lanes into retrieval.

Verification:
- Context-pack golden tests.
- Owner routing tests.
- Token budget and duplicate-read tests.

Completed:
- Added agent-contract context items so context packs can carry owner-map, test-map, proof-lane, and generated-zone evidence as typed metadata instead of ambient prose.
- Added `ContextCompiler::compile_workspace`, which reads workspace-local `agent/owner-map.json`, `agent/test-map.json`, `agent/proof-lanes.toml`, and `agent/generated-zones.toml`.
- Added deterministic file/range anchor excerpts with per-item caps, output reserve handling, duplicate suppression, and handle spillover for oversized context.
- Added glob-based owner/test/generated-zone routing for workspace-relative paths.
- Added bounded lexical retrieval for query anchors, skipping `target/`, `.git/`, `.quorp/`, and `node_modules/`.
- Added tests for owner/test/proof context inclusion, duplicate file-anchor suppression, token-budget spillover, and lexical query retrieval.
- Verified `cargo test -p quorp_context --lib`.
- Verified `cargo test -p quorp_context_model --lib`.
- Verified `cargo test -p quorp --bin quorp context`.
- Verified `./script/clippy`.
- Verified `just medium`.

## Phase 6: Verification And ProofLens

Status: complete

Scope:
- Convert validation output into proof-preserving packets.
- Preserve exit codes, spans, failing tests, advisory IDs, raw-log paths, hashes, and tool versions.
- Build proof receipts as first-class review artifacts.

Verification:
- Cargo JSON diagnostic parser tests.
- Proof packet recall tests.
- Receipt schema tests.

Completed:
- Added proof-preserving packet domain types for command evidence, cargo diagnostics, primary spans, failing tests, security findings, and raw artifact references.
- Extended verify reports so they can carry proof packets alongside stage reports.
- Added `proof_packet_from_command`, which preserves command, cwd, exit code, duration, optional tool version, raw-log path, raw-log SHA-256, truncation state, cargo diagnostics, failing tests, and security findings.
- Added packet-to-stage-report conversion so compact packets can feed the existing staged verification ladder.
- Added cargo JSON diagnostic parsing with stable error code and primary span extraction.
- Added test-output and security-finding parsers for failing test names, panic summaries, and advisory IDs.
- Added tests for cargo diagnostic packets, failing test packets, security advisory packets, stage-report raw-log preservation, and packet schema round trips.
- Verified `cargo test -p quorp_verify --lib`.
- Verified `cargo test -p quorp_verify_model --lib`.
- Verified `./script/clippy`.
- Verified `just medium`.

## Phase 7: Memory And Self-Improvement

Status: complete

Scope:
- Persist working, episodic, semantic, procedural, negative, and rule memory.
- Promote rules only through candidate, draft, shadow-verified, and active states.
- Prevent repeated failed fixes without new evidence.

Verification:
- SQLite-backed memory tests.
- Rule lifecycle tests.
- Failure-fingerprint regression tests.

Completed:
- Added explicit failure fingerprints and failed-attempt records to the memory model.
- Added retry decisions so memory blocks an identical failed fix when the failure signature, attempted patch hash, and evidence hash are unchanged.
- Kept changed patch/evidence paths allowed so the agent can retry only after new information or a materially different fix.
- Added failed-attempt recording and recall through the negative memory tier.
- Added rule-forge extraction from ProofLens packets into retry fingerprints, preserving owner, attempted fix hash, and raw evidence hash.
- Added shadow-result accounting so rules gain confidence and promote toward verified/active when they prevent failures, and become challenged/rejected on false positives.
- Added tests for repeated-fix blocking, changed-evidence allowance, proof-packet fingerprint extraction, and shadow lifecycle promotion/challenge.
- Verified `cargo test -p quorp_memory_model --lib`.
- Verified `cargo test -p quorp_memory --lib`.
- Verified `cargo test -p quorp_rule_forge --lib`.
- Verified `./script/clippy`.
- Verified `just medium`.

## Phase 8: Full Auto SWE Loop

Status: complete

Scope:
- Drive tasks through understand, plan, inspect, patch, verify, review, learn.
- Add task lists, budgets, validation drains, stall detection, cancellation, and optional parallel worktree attempts.

Verification:
- Controller state-machine tests.
- End-to-end mock SWE tasks.
- Budget and cancellation tests.

Completed:
- Added `SweController` as the typed full-auto SWE loop controller with explicit Understand -> Plan -> Inspect -> Patch -> Verify -> Review -> Learn -> Done stages.
- Added `SweBudget`, `SweUsage`, `SweEvent`, and `SweNextAction` so runtime integration can drive the loop without embedding phase logic in prompts.
- Added approved-step tracking, evidence hash tracking, patch hash tracking, verification failure tracking, proof hash tracking, and memory hash tracking.
- Added budget enforcement for max iterations, token budget, and wall-clock budget.
- Added stall detection for repeated evidence, repeated patch hashes, repeated verification failures without new evidence, and explicit no-progress events.
- Added cancellation as a terminal controller event.
- Added controller tests for the happy path, failed-verification repair loop, repeated-stall blocking, token-budget exhaustion, and cancellation.
- Verified `cargo test -p quorp_plan_mode --lib`.
- Verified `./script/clippy`.
- Verified `just medium`.

## Phase 9: Evaluation

Status: complete

Scope:
- Maintain local Rust SWE benchmark suites with receipts.
- Track success rate, SecureETTS, tokens, wall time, memory, patch size, retries, and proof-lane choice.
- Add CI gates for maintained proof lanes and structural guardrails, including the existing LOC cap script once the current refactor churn is ready for enforcement.

Verification:
- Benchmark smoke tests.
- Scoreboard generation tests.
- Regression gates in CI.

Completed:
- Extended benchmark score reports with success rate, secure success rate, SecureETTS tokens, total and median wall time, patch lines changed, retry counts, proof-lane counts, and optional per-case memory peak fields.
- Added per-case evaluation fields so scoreboards preserve secure success, SecureETTS contribution, wall time, patch size, retry count, proof lanes, and memory peak in JSON and Markdown.
- Derived proof-lane labels from detailed benchmark reports, including fast, medium, evaluation, deterministic, and judge lanes.
- Preserved adaptive retry evidence even when a detailed case report has a single recorded attempt.
- Updated scoreboard Markdown with Phase 9 aggregate metrics and a proof-lane section.
- Added scoreboard-generation assertions for Phase 9 metrics.
- Added a strict `--fail-on-regression` benchmark score mode so CI can fail after writing the scoreboard artifacts.
- Extended regression detection to Phase 9 metrics: secure success, success rates, billed tokens, SecureETTS, median wall time, retries, and patch size.
- Kept cost regressions comparable by gating token/time/retry/patch-size increases only when total cases, solved cases, and secure successes are unchanged.
- Verified `cargo fmt --all --check`.
- Verified `cargo test -p quorp --bin quorp score_benchmark_reports_writes_session_scoreboard`.
- Verified `cargo test -p quorp_benchmark --lib`.
- Verified `./script/clippy`.
- Verified `just medium`.

## Phase 10: CI Evaluation Gates

Status: complete

Scope:
- Make Phase 9 evaluation checks runnable as a local proof lane.
- Add CI coverage for deterministic scoring and optional live benchmark smoke scoring.
- Keep live provider-dependent benchmark runs opt-in so pull requests are not blocked by missing secrets.

Verification:
- Deterministic benchmark gate script.
- CI workflow syntax by inspection.
- Existing medium lane.

Completed:
- Added `script/quorp-benchmark-regression-gate` to run deterministic scoring tests and optionally score either `QUORP_BENCHMARK_RUN_DIR` or a live smoke run.
- Added `just benchmark-gate` in both `justfile` and `Justfile`.
- Added `.github/workflows/benchmark-evaluation.yml` so PRs and pushes run deterministic evaluation checks, while `workflow_dispatch` can opt into the live one-case smoke.
- Wired live smoke scoring to `benchmark score --fail-on-regression`.
- Verified `./script/quorp-benchmark-regression-gate`.
- Verified `just benchmark-gate`.
- Verified `cargo fmt --all --check`.
- Verified `./script/clippy`.
- Verified `just medium`.

## Phase 11: Release Candidate Closure

Status: complete

Scope:
- Freeze feature work and close this version as a binary/worktree release candidate.
- Refresh release-facing docs so they match completed Phases 0-10.
- Run release-grade proof lanes and record any conditional gates that are skipped.
- Keep live provider benchmark smoke optional unless credentials are available.

Verification:
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
- `just security` when `cargo audit` and `gitleaks` are installed.
- Live benchmark smoke when provider credentials are available.

Completed:
- Refreshed `AGENT_SUPPORT.md` to describe the release-candidate support surface instead of earlier phase gaps.
- Added `docs/src/development/quorp-agent-runtime-release-candidate.md`.
- Stabilized all-features parallel tests by removing a shared diagnostics log-file race from the test assertion and serializing MCP subprocess tests.
- Updated `Cargo.lock` to `rustls-webpki 0.103.13` to clear RustSec vulnerabilities reported by `cargo audit`.
- Verified `cargo fmt --all --check`.
- Verified `cargo check --workspace`.
- Verified `cargo test --workspace --lib` with 458 tests across 39 suites.
- Verified `./script/clippy`.
- Verified `just benchmark-gate`; live provider smoke was not requested.
- Verified `just medium`.
- Verified `just deep`.
- Verified `just security`; `cargo audit` reported no vulnerabilities and `gitleaks` was unavailable but non-fatal in the current lane.
- Verified `cargo build --release -p quorp`.
- Verified `target/release/quorp --help`.
- Verified `QUORP_RENDER_DEMO_STATIC=1 target/release/quorp render-demo`.
- Verified `NO_COLOR=1 QUORP_RENDER_DEMO_STATIC=1 target/release/quorp render-demo`.
- Noted `actionlint` is not installed locally; workflow syntax was reviewed by inspection.

## Phase 12: Live Provider Audit And Release Hardening

Status: complete

Scope:
- Install missing local security/workflow tools.
- Run live provider smoke and Rust SWE top5 benchmarks.
- Capture behavior gaps as scoreboard fields and release notes instead of ad hoc log-reading.

Completed:
- Installed `gitleaks` and `actionlint` with Homebrew.
- Verified workflow syntax with `actionlint .github/workflows/benchmark-evaluation.yml .github/workflows/loc-cap.yml`.
- Re-ran `just security` with `cargo audit`, `gitleaks`, and the LOC cap lane available.
- Ran a one-case live provider smoke: solved 1/1, secure success 1/1, 9 requests, 39,396 billed tokens.
- Fixed live batch execution by canonicalizing `result_dir` and optional `log_dir` before launching per-case children from case-local working directories.
- Ran live Rust SWE top5: solved 3/5, secure success 3/5, 34 requests, 204,978 billed tokens, SecureETTS 134,027, 323 changed lines.
- Added scoreboard audit fields for slow first-request latency, watchdog near-limit cases, and patch-quality risk cases.
- Rescored the live top5 run with the release binary and new audit fields: 5 watchdog near-limit cases and 1 patch-quality risk case.
- Documented remaining targeted runtime improvements for owner-range no-op turns, timeout-without-anchor recovery, dependency-edit gating, and broad write risk.

Verification:
- `actionlint .github/workflows/benchmark-evaluation.yml .github/workflows/loc-cap.yml`.
- `just security`.
- Live one-case provider smoke.
- Live Rust SWE top5.
- `./target/release/quorp benchmark score --run-dir target/quorp-live-benchmarks/top5-20260426-230929 --suite rust-swebench-top5 --reports-root target/quorp-live-benchmarks/reports --output-root target/quorp-live-benchmarks/scoreboards/top5-release-audit --fail-on-regression`.
- `cargo test -p quorp --bin quorp challenge_setup_failure_writes_benchmark_report`.
- `cargo test -p quorp --bin quorp score_benchmark_reports_writes_session_scoreboard`.
- `cargo test -p quorp_benchmark --lib`.
- `cargo fmt --all --check`.

## Phase 13: General Rust SWE Recovery Upgrade

Status: in progress

Scope:
- Improve the failing 3/5 top5 run without benchmark-oracle patches.
- Keep code-derived semantic source repair and controller repair injection available for stalled benchmark repair loops.
- Make benchmark fast-loop reruns use the full 120 second policy budget.
- Prefer owner source targets over support surfaces when validation fails by timeout.
- Keep source patch phases in a patch-only correction lane long enough for a real model edit.

Completed:
- Kept observed-slice semantic source repair and controller injection available for repeated read-only loops when Quorp can derive the edit from loaded Rust code.
- Normalized benchmark fast-loop `RunCommand` actions to `timeout_ms: 120000`.
- Updated parser recovery examples to emit 120 second reruns.
- Changed timeout/unknown-failure target ranking to lease owner source before changelog/docs support surfaces.
- Added patch-intent correction packets with target, range, hash, rerun command, and minimal JSON example.
- Increased source patch-phase fatal escalation from two invalid turns to four invalid turns.
- Added focused unit tests for timeout owner leasing, timeout rerun recommendations, fast-loop timeout normalization, and patch-intent retries.

Verification:
- `cargo test -p quorp_agent_core --lib` passed with 170 tests.
- `cargo fmt --all --check` passed.
- `./script/clippy` passed.
- `just medium` passed.
- `cargo build --release -p quorp` passed.
- `just benchmark-gate` passed without a live run.

## Phase 14: Mac CLI Installability

Status: complete

Scope:
- Make `quorp` callable from any folder on this Mac.
- Ensure plain `quorp` reacts to the caller's current directory.
- Make the no-argument default visibly ad hoc.

Completed:
- Built `target/release/quorp`.
- Installed the release binary to `/opt/homebrew/bin/quorp`.
- Updated the no-argument startup frame to say `ad hoc agent ready`.

Verification:
- `which quorp` from `/Users/bentaylor/Code/zitpit` resolves to `/opt/homebrew/bin/quorp`.
- `quorp --version` from `/Users/bentaylor/Code/zitpit` prints `quorp 0.231.0`.
- `printf '/exit\n' | quorp` from `/Users/bentaylor/Code/zitpit` starts the ad hoc session against `/Users/bentaylor/Code/zitpit`.

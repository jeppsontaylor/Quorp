# WORKPLAN: Quorp Agent OS Next Roadmap

This workplan records the current state of the roadmap described in the agent task plan. It is intentionally evidence-first: each item notes what exists in the workspace, what was changed in this pass, what verification was actually run now, and what remains a second-pass candidate.

Some entries describe earlier workspace state that was already present when this pass began. Those are called out so the record does not overclaim fresh implementation.

## Current Status

Partially complete. Ledger Kernel V2 and Proof Engine V2 are now implemented and verified; the remaining roadmap items are still system-scale follow-up work.

## Completed

| Item | Status |
| --- | --- |
| Truth/drift gate documentation | Done |
| `script/quorp-audit-gaps` | Done |
| `just security` audit wiring | Done |
| Benchmark action-budget gating | Done |
| Main/session native backend Patch VM parity | Done |
| Ledger cursor/snapshot/replay kernel | Done |
| Durable proof DAG and proof CLI | Done |

## Outstanding Work

| Item | Status |
| --- | --- |
| Repo-wide Patch VM write-path enforcement | Open (`NEEDS_REVIEW`) |
| Managed settings, trust, permissions, and sandbox policy | Open (`NEEDS_REVIEW`) |
| Persistent context index and code intelligence | Open (`NEEDS_REVIEW`) |
| Durable memory/rule lifecycle | Open (`NEEDS_REVIEW`) |
| Ledgered task DAG and subagents | Open (`NEEDS_REVIEW`) |
| Fullscreen TUI mission-control parity | Open (`NEEDS_REVIEW`) |
| Replayable eval and release lanes | Open (`NEEDS_REVIEW`) |

These items stay open until the corresponding subsystem has code, tests, and verification evidence.

## 1. Truth And Drift Gate

- Work item: add a tracker section for remaining world-class gaps and a non-mutating audit gate for roadmap drift.
- Result: added an `Open World-Class Gaps` section to `agent/phase-tracker.md`, and added `script/quorp-audit-gaps` to verify `tips/upgrade/v2/` is still tracked, tracked evidence files still exist, benchmark playbook extras are gated on benchmark policy, main/session native action handlers stay identical, and native action handlers do not reintroduce direct production writes outside approved raw-log writes.
- Review result: strengthened the original gate after review because the first version did not actually prove native write parity or benchmark-only behavior.
- Test / verification: `./script/quorp-audit-gaps`
- Status: complete

## 2. Ledger Kernel V2

- Work item: make the run ledger the durable kernel with cursors, snapshots, and replayable consumers.
- Result: added `quorp_agent_core::ledger` with hash-chain events, writer-local disk cursor initialization, reader validation, `read_from` subscriber resume, disk cursors under `artifacts/runtime-subscribers/<subscriber>/cursor.json`, and snapshot creation. The mirrored run-support layers now delegate ledger behavior to the shared module while preserving `events.jsonl` compatibility. Durable runtime consumers now treat live fanout events as wake signals, read missed events from `run-ledger.jsonl`, apply memory/rule/proof/benchmark side effects, and commit subscriber cursors after successful side effects. Added `quorp replay <run-dir>` to the CLI surface.
- Test / verification: `cargo test -p quorp_agent_core --lib ledger`; `cargo test -p quorp_session --lib durable_runtime_consumers_persist_memory_and_journals -- --test-threads=2`; `cargo test -p quorp --bin quorp replay`; `cargo check --workspace`
- Status: complete

## 3. Patch VM As Sole Write Contract

- Work item: centralize writes through the Patch VM and quarantine benchmark-only repair paths.
- Result: reconciled the main and session native backend action handlers so the main binary no longer keeps the older direct `write_full_file` native write paths. The audit gate now fails if those files drift again or if native action handlers reintroduce direct writes outside raw verification logs. Added `script/quorp-write-path-audit` plus `agent/write-path-allowlist.toml` to enforce the repo-wide write audit against the current approved sinks.
- Review result: fixed the largest prior `NEEDS_REVIEW` item for native action-handler parity. The repo-wide write audit now exists, but the broader Patch VM migration is still a separate hardening pass because the current workspace still relies on reviewed allowlisted write sinks.
- Test / verification: `cargo check -p quorp`; `cargo test -p quorp --bin quorp -- --test-threads=2`; `./script/quorp-audit-gaps`; `./script/quorp-write-path-audit`
- Status: complete for native backend parity; NEEDS_REVIEW for repo-wide write-path enforcement

## 4. Verification And Proof Engine V2

- Work item: make verification own proof DAG execution and exportability.
- Result: added durable proof DAG schema to `quorp_verify_model`, file-backed verify cache/store support in `quorp_verify`, explicit memory/file cache traits, durable verify execution that writes proof DAGs and raw-log artifacts, and proof receipt extensions for `run-ledger.jsonl`, proof DAGs, and raw verification logs. Added `quorp proof show`, `quorp proof export`, and `quorp proof verify` to the CLI surface with hash recomputation and tamper detection.
- Test / verification: `cargo test -p quorp_verify --lib`; `cargo test -p quorp_verify_model --lib`; `cargo test -p quorp_cli --lib`; `cargo test -p quorp --bin quorp proof`; `cargo check --workspace`
- Status: complete

## 5. Settings, Trust, Permissions, Sandbox

- Work item: expand settings, trust mediation, and parsed permission policy.
- Result: sandboxing and permission primitives are present, but the richer trust/settings model and compound-command policy remain future work.
- Verification reference from the existing tracker: `cargo test -p quorp_permissions --lib` (not rerun in this pass)
- Status: NEEDS_REVIEW

## 6. Context OS And Code Intelligence V2

- Work item: make context compilation persistent and backed by repo intelligence.
- Result: context compilation, owner/test routing, and retrieval exist, but the persistent index/LSP-backed fact store from the roadmap is not complete.
- Verification reference from the existing tracker: `cargo test -p quorp_context --lib` (not rerun in this pass)
- Status: NEEDS_REVIEW

## 7. Memory And Rule Forge V2

- Work item: persist memory and rule-forge lifecycle state.
- Result: negative memory and rule-forge scaffolding exist, and runtime events now feed durable memory/rule observation paths, but the full candidate/shadow/active/deprecated lifecycle is not yet a complete policy store.
- Verification reference from the existing tracker: `cargo test -p quorp_memory --lib` (not rerun in this pass)
- Status: NEEDS_REVIEW

## 8. Task DAG And Subagents

- Work item: ledger task runs, roles, and scoped subagent returns.
- Result: the autonomous controller loop exists, and runtime consumers now persist durable artifacts, but the full task DAG / subagent orchestration surface from the roadmap is still a broader system change.
- Verification reference from the existing tracker: `cargo test -p quorp_plan_mode --lib` (not rerun in this pass)
- Status: NEEDS_REVIEW

## 9. TUI And Product Parity

- Work item: make the fullscreen mission-control UI execute the same commands as the stream-first surface.
- Result: the stream-first renderer is present, but the fullscreen pane parity and slash-command execution surface are not yet complete.
- Verification reference from the existing tracker: `cargo test -p quorp_render --lib` (not rerun in this pass)
- Status: NEEDS_REVIEW

## 10. Evals And Release

- Work item: broaden eval lanes and tie release readiness to replayable evidence.
- Result: benchmark scoreboards and release-candidate docs exist, but the broader eval matrix and release packaging/signing evidence are still future work.
- Review result: fixed one benchmark quarantine leak by requiring `BenchmarkAutonomous` before `compact_turn_actions` can use the benchmark-only expanded generated-snapshot action budget. Added a negative test for normal runs.
- Verification reference from the existing tracker: `just benchmark-gate` (not rerun in this pass)
- Test / verification from this pass: `cargo test -p quorp_agent_core --lib compact_turn_actions -- --test-threads=2`
- Status: NEEDS_REVIEW for broader eval/release coverage

## Verification Run

- `cargo test -p quorp_agent_core --lib runtime_event_worker_drains_subscriber_queue -- --test-threads=2`
- `cargo test -p quorp_agent_core --lib ledger`
- `cargo test -p quorp_session --lib durable_runtime_consumers_persist_memory_and_journals -- --test-threads=2`
- `cargo test -p quorp --bin quorp -- --test-threads=2`
- `cargo test -p quorp --bin quorp replay`
- `cargo test -p quorp --bin quorp proof`
- `cargo test -p quorp_verify --lib`
- `cargo test -p quorp_verify_model --lib`
- `cargo test -p quorp_cli --lib`
- `./script/quorp-audit-gaps`
- `cargo fmt --all -- --check`
- `cargo check --workspace`
- `cargo test -p quorp_agent_core --lib compact_turn_actions -- --test-threads=2`

## Follow-Up

- `NEEDS_REVIEW`: the remaining roadmap items are system-scale work: repo-wide Patch VM migration beyond the reviewed allowlist, persistent context indexing, managed settings/trust policy, durable rule lifecycle storage, task-DAG subagents, fullscreen TUI parity, and broader replayable eval/release lanes.

## Implementation Plan For Outstanding Work

This section is the handoff-grade plan for finishing the remaining roadmap. It is intentionally concrete: each workstream lists the current files, the code to add or change, the order of implementation, the tests to write, and the expected outcome. Do not mark a workstream complete in `agent/phase-tracker.md` until its acceptance tests pass and the corresponding workplan entry has a dated result.

### 0. Sequencing Rules

Recommended order:

1. Ledger Kernel V2
2. Proof Engine V2
3. Patch VM repo-wide write audit
4. Settings, trust, permissions, sandbox
5. Context index
6. Memory and rule forge persistence
7. Task DAG and subagents
8. TUI parity
9. Evals and release

Reasoning:

- Ledger must come first because proof, context, memory/rules, task DAG, and replayable evals should write facts through one append path.
- Proof should come before subagents and release because later systems need proof receipts and verification DAG facts as completion criteria.
- Patch VM write auditing should come before high-autonomy task DAG work so subagents cannot bypass the write contract.
- Settings/trust must precede full-auto/subagent expansion because policy must decide which tools, network, MCP, and sandboxes are available.

Global implementation rule:

- Any new runtime fact that affects user-visible behavior or proof should become a ledger event before downstream consumers observe it.
- Any new file-write path must either use Patch VM or be explicitly classified as artifact/log/storage in the audit gate.
- Every new `NEEDS_REVIEW` closure must update this file, `agent/phase-tracker.md`, and `script/quorp-audit-gaps` if a regression can be checked mechanically.

### 1. Ledger Kernel V2

Status: complete. The notes below are retained as the implementation checklist that was closed in this pass.

Current code:

- Sidecar ledger helpers live in both `crates/quorp/src/quorp/run_support.rs` and `crates/quorp_session/src/quorp/run_support.rs`.
- Current types/functions: `RunLedgerEvent`, `RunLedgerCursor`, `read_run_ledger`, `read_run_event_payloads`, `append_run_ledger_record`, `append_run_ledger_from_existing_event`, `run_ledger_hash`.
- Runtime fanout and worker code lives in `crates/quorp_agent_core/src/runtime.rs`: `RuntimeEventFanout`, `RuntimeEventSubscription`, `RuntimeEventWorker`.
- Durable runtime consumers are currently duplicated in `crates/quorp/src/quorp/agent_runner.rs` and `crates/quorp_session/src/quorp/agent_runner.rs`: `spawn_runtime_event_consumers`, `spawn_runtime_event_consumer`, `append_consumer_event`.

Target design:

- Move ledger domain/API into a shared crate/module, preferably `crates/quorp_agent_core/src/ledger.rs` or a new `crates/quorp_ledger` if reuse outside agent core becomes cleaner.
- Keep compatibility wrappers in `run_support.rs` so existing run directory code does not break immediately.
- Add these public types:
  - `RunLedgerEvent { run_id, seq, prev_hash, hash, actor, kind, payload, timestamp_ms }`
  - `RunLedgerWriter { path, run_id }`
  - `RunLedgerReader { path }`
  - `RunLedger { reader, writer }` if a combined handle reduces boilerplate
  - `RunSnapshot { run_id, through_seq, through_hash, event_count, created_at_ms, state: serde_json::Value }`
  - `SubscriberCursor { subscriber, seq, hash, updated_at }`
  - `LedgerValidationReport { valid, event_count, first_error }`
  - `LedgerEventKind` enum if string kinds start drifting

Implementation steps:

1. Create `ledger.rs` with pure hashing, append, read, validate, and snapshot code.
2. Replace the in-memory static cursor map with per-ledger cursor loading from disk. The current `RUN_LEDGER_CURSORS` is process-local and is not enough for crash resume.
3. Store subscriber cursors under `run_dir/artifacts/runtime-subscribers/<subscriber>/cursor.json`.
4. Add `RunLedgerWriter::append(actor, kind, payload)` returning `RunLedgerEvent`.
5. Add `RunLedgerReader::read_from(after: Option<SubscriberCursor>, limit: usize)` returning ordered events and a next cursor.
6. Add `RunLedgerReader::validate_hash_chain()` that recomputes every hash and checks `seq` monotonicity plus `prev_hash`.
7. Add `RunLedgerReader::snapshot(name, state)` that writes `snapshots/<name>.json` and appends a `snapshot.created` event.
8. Change `HeadlessEventRecorder::emit` in both `agent_runner.rs` files so ledger append is the first durable write and `events.jsonl` becomes a legacy mirror.
9. Change `RuntimeEventFanout::with_downstream` flow so downstream UI notifications derive from successful ledger writes during headless runs.
10. Replace durable consumers with ledger readers:
    - worker loop reads from `RunLedgerReader` at its cursor
    - worker applies side effects
    - worker commits `SubscriberCursor`
    - worker resumes from cursor after restart
11. Keep `RuntimeEventFanout` for live UI notification only; do not let durable consumers depend on live in-memory queues for correctness.
12. Add a `quorp replay <run-dir>` command once reader validation works. Initial output can reconstruct transcript/proof summary from ledger events.

Key tests:

- Unit: hash-chain validation accepts a valid ledger and rejects modified payload/hash/seq.
- Unit: `RunLedgerReader::read_from` resumes after cursor and never repeats committed events.
- Unit: cursor write is atomic enough for crash simulation: write events, commit cursor after N, reopen, consume N+1.
- Integration: durable consumers persist memory/proof/benchmark journals, process stops, new worker resumes and consumes only missing events.
- Integration: `read_run_event_payloads` still supports legacy `events.jsonl`.
- CLI smoke: `quorp replay <run-dir>` validates ledger and prints summary.

Commands:

- `cargo test -p quorp_agent_core --lib ledger`
- `cargo test -p quorp_session --lib durable_runtime_consumers_persist_memory_and_journals -- --test-threads=2`
- `cargo test -p quorp --bin quorp replay`
- `cargo check --workspace`

Definition of done:

- No durable runtime consumer owns correctness through `RuntimeEventSubscription`.
- Every consumer has a disk cursor.
- `run-ledger.jsonl` validates after normal run and resume.
- A run can be replayed without `events.jsonl`.

### 2. Proof Engine V2

Status: complete. The notes below are retained as the implementation checklist that was closed in this pass.

Current code:

- Verification API is in `crates/quorp_verify/src/quorp_verify.rs`.
- Current key functions/types: `VerifyRequest`, `VerifyCommand`, `VerifyCommandResult`, `execute_verify_request`, `proof_packet_from_command`, `stage_report_from_packet`, `VERIFY_CACHE`.
- Native validation uses proof packets in `crates/quorp/src/quorp/tui/native_backend/actions.rs` and the mirrored `crates/quorp_session/.../actions.rs`.
- Run proof receipts are written in `crates/quorp/src/quorp/cli.rs` via `write_run_proof_receipt`.
- Benchmark proof receipts are in `crates/quorp/src/quorp/benchmark/reporting.rs`: `write_benchmark_proof_receipt`.

Target design:

- `quorp_verify` owns durable execution and proof DAG persistence, not just packet parsing.
- Cache persists under `.quorp/verify/cache/` by content hash and toolchain fingerprint.
- Proof DAG persists under `.quorp/verify/runs/<verify_run_id>/proof-dag.json`.
- CLI commands:
  - `quorp proof show <run-dir>`
  - `quorp proof export <run-dir> --output <path>`
  - `quorp proof verify <proof-file-or-run-dir>`

Implementation steps:

1. Add `VerifyStore` in `quorp_verify` with root path, cache path, run path, and raw-log path helpers.
2. Replace static `VERIFY_CACHE` with a trait-backed cache:
   - `VerifyCache` trait: `get(&CacheKey)`, `put(&CacheKey, CachedStageReport)`
   - `MemoryVerifyCache` for unit tests
   - `FileVerifyCache` for `.quorp/verify/cache/<sha>.json`
3. Add `ProofDag` model:
   - `ProofNode { id, kind, inputs, outputs, artifact_refs, status }`
   - `ProofEdge { from, to, reason }`
   - `ProofDag { verify_run_id, nodes, edges, root_artifacts, final_verdict }`
4. Convert each `VerifyCommand` execution into:
   - raw log artifact
   - `ProofPacket`
   - stage node
   - cache node if reused
5. Add command provenance fields to `VerifyRequest`:
   - sandbox mode
   - permission decision id/hash
   - context pack id
   - patch receipt ids
   - ledger event seq/hash at start
6. Update native backend `run_validation_commands` to call a durable executor in `quorp_verify`, not just `execute_verify_request` with a closure.
7. Update `write_run_proof_receipt` to include:
   - `run-ledger.jsonl` hash
   - proof DAG path/hash
   - verification raw-log refs
   - patch receipts and rollback tokens from ledger facts
8. Implement `quorp proof` subcommands in `crates/quorp/src/quorp/cli.rs`.
9. Add proof reconstruction from ledger facts once Ledger Kernel V2 lands.

Key tests:

- Unit: file cache returns same `StageReport` and marks `from_cache = true`.
- Unit: proof DAG serialization round trip preserves node/edge order.
- Unit: proof verification fails when raw log hash changes.
- Integration: `RunValidation` writes `.quorp/verify/runs/.../proof-dag.json`.
- CLI: `quorp proof verify <run-dir>` succeeds for a synthetic run.
- Regression: in-memory static cache is not required for correctness across processes.

Commands:

- `cargo test -p quorp_verify --lib`
- `cargo test -p quorp --bin quorp proof`
- `cargo test -p quorp_session --lib run_validation`
- `cargo check --workspace`

Definition of done:

- Proof artifacts can be verified after process restart.
- Proof receipt references raw logs by hash.
- `quorp proof verify` detects tampering.

### 3. Patch VM Repo-Wide Write Audit

Current code:

- Native backend parity is now guarded by `script/quorp-audit-gaps`.
- Native action handlers now share Patch VM helpers such as `apply_single_file_change`, `apply_set_executable_change`, and `render_patch_vm_receipt`.
- Remaining risk: other crates may still use direct `fs::write`, `remove_file`, `rename`, `OpenOptions`, or chmod paths for production mutations.

Target design:

- Direct writes are permitted only in approved classes:
  - artifact/log/receipt/report writes
  - storage database internals
  - test setup
  - vendor/build scripts where applicable
- Production workspace mutations use Patch VM receipts.

Implementation steps:

1. Add `script/quorp-write-path-audit`.
2. Use `rg` to scan `crates/` for:
   - `fs::write`
   - `std::fs::write`
   - `remove_file`
   - `rename(`
   - `OpenOptions::new`
   - `set_permissions`
   - `File::create`
3. Maintain an allowlist file, for example `agent/write-path-allowlist.toml`, with fields:
   - `path`
   - `pattern`
   - `reason`
   - `owner`
   - `expires_or_review_by`
4. Fail if a direct write hit is not allowlisted.
5. Move repeated artifact helpers into a shared module if the allowlist gets noisy.
6. For production workspace writes found outside native backend, route through Patch VM:
   - construct `FileChange`
   - require expected hash where file exists
   - return/render `PatchReceipt`
   - append ledger event `patch.receipt`
7. Wire `script/quorp-write-path-audit` into `script/quorp-audit-gaps` or `just security`.

Key tests:

- Script test: create a temporary disallowed sample and verify the audit fails.
- Script test: allowlisted artifact path passes.
- Rust test: Patch VM rejects stale hashes and hidden protected paths.
- Native backend tests still pass after any write helper move.

Commands:

- `./script/quorp-write-path-audit`
- `./script/quorp-audit-gaps`
- `cargo test -p quorp_patch_vm --lib`
- `cargo test -p quorp --bin quorp -- --test-threads=2`

Definition of done:

- A new direct production write cannot enter `crates/` without an explicit reviewed allowlist entry.
- Every workspace mutation path produces `PatchReceipt` and rollback evidence.

### 4. Settings, Trust, Permissions, Sandbox

Current code:

- JSON settings loader is in `crates/quorp_config/src/lib.rs`.
- Existing settings fields: `provider`, `sandbox`, `permissions`, `hooks`, `allowed_commands`, `proof_lanes`.
- CLI doctor is in `crates/quorp/src/quorp/cli_demos.rs`: `run_doctor_command`.
- Permission policy is in `crates/quorp_permissions/src/quorp_permissions.rs`.
- Command classification is currently string-first: `classify_tool_action`, `classify_command_capability`.

Target design:

- `settings.json` is canonical.
- `.quorp/agent.toml` remains compatibility-only and doctor warns when present.
- Project settings cannot elevate permissions unless the project is trusted.
- Command permissions use parsed argv/capability tokens, not broad command globs.
- Full-auto requires sandboxed execution and network-off by default.

Implementation steps:

1. Extend `Settings` with versioned sections:
   - `profiles`
   - `model_routing`
   - `trust`
   - `mcp`
   - `tool_registry`
   - `proof`
   - `context`
   - `memory`
   - `rules`
   - `tui`
   - `evals`
   - `managed_policy`
2. Add `SettingsVersion` or `schema_version` so future migrations are explicit.
3. Add merge semantics that cannot elevate from project settings unless trusted:
   - user settings can grant
   - project settings can narrow by default
   - project settings can elevate only if `trust.project_id` is trusted in user settings
4. Add `TrustState`:
   - project root canonical path
   - git remote fingerprint
   - settings hash
   - trusted bool
5. Add command parser model in `quorp_permissions`:
   - `ParsedCommandPolicyInput { argv, shell_meta, wrappers, env_assignments }`
   - `CapabilityToken` enum: `ShellMeta`, `CompoundCommand`, `Network`, `DependencyInstall`, `Docker`, `GitRemote`, `FindDelete`, `FindExec`, `SecretsRead`, `GeneratedExecutable`, `Mcp`, `Browser`, `FilesystemWrite`
6. Implement parser conservatively:
   - split simple argv with shell-words crate if available, otherwise add a small parser with tests
   - detect `&&`, `||`, `;`, `|`, backticks, `$()`, redirects, globstar as shell meta
   - detect wrappers: `sh -c`, `bash -c`, `env`, `xargs`, `find -exec`
7. Replace glob-only allow matching with:
   - deny tokens first
   - explicit allow tokens
   - command allowlist only after parsed token check
8. Expand `run_doctor_command` to report:
   - settings source precedence
   - trust state
   - provider name/model/base URL with API key redacted
   - sandbox viability
   - network policy
   - MCP servers
   - proof lanes
   - gitleaks availability
   - release/license metadata
9. Add tests for project settings unable to elevate permission mode without trust.

Key tests:

- `quorp_config`: project full-auto is downgraded/blocked when untrusted.
- `quorp_permissions`: `cargo test && curl ...` yields `CompoundCommand` and `Network`.
- `quorp_permissions`: `find . -delete` and `find . -exec rm` are denied unless explicitly allowed.
- `quorp_permissions`: `git remote add` is separate from ordinary `git status`.
- CLI: `quorp doctor` prints redacted API key status and trust state.

Commands:

- `cargo test -p quorp_config --lib`
- `cargo test -p quorp_permissions --lib`
- `cargo test -p quorp --bin quorp doctor`
- `quorp doctor`

Definition of done:

- Untrusted project config cannot increase permission, network, MCP, or sandbox privileges.
- Doctor makes the effective policy explainable without exposing secrets.

### 5. Context OS And Code Intelligence V2

Current code:

- Context compiler is in `crates/quorp_context/src/quorp_context.rs`.
- Current key APIs: `ContextCompiler`, `CompileRequest`, `CompileContext`, `compile`, `compile_workspace`.
- Existing support: memory recall, anchor handles, agent contract loading, lexical excerpts, owner/test/proof/generated contract items.
- No persistent `.quorp/index` exists yet.

Target design:

- Persistent SQLite index under `.quorp/index/index.sqlite`.
- Index stores lexical chunks, file hashes, symbols, definitions, references, diagnostics, imports/calls, tests, proof history, memory/rule triggers, and context-pack provenance.
- `ContextPackV2` records source hash, trust level, freshness, cost, and reason selected for every item.

Implementation steps:

1. Create `quorp_context::index` module or new crate `quorp_index`.
2. Schema tables:
   - `files(path primary key, sha256, language, bytes, indexed_at_unix)`
   - `chunks(id, path, range_start, range_end, text_hash, text, token_estimate)`
   - `symbols(id, path, name, kind, range_start, range_end, definition_hash)`
   - `references(symbol_id, path, range_start, range_end)`
   - `diagnostics(id, path, severity, code, message, range_start, range_end, source)`
   - `imports(path, target, kind)`
   - `tests(id, path, name, owner_path, command)`
   - `proof_history(path, command, verdict, proof_hash, updated_at_unix)`
   - `context_packs(pack_id, request_hash, generated_at_unix, budget, selected_json)`
3. Add `IndexBuilder::build(workspace_root)`:
   - skip `.git`, `target`, `.quorp`, `node_modules`
   - hash files first
   - only reindex changed files
4. Add language passes:
   - Rust first using existing `quorp_repo_scan::harvest_rust_symbols`
   - TS/JS, Python, Go as lexical chunks initially if tree-sitter is not already wired
   - later add tree-sitter facts behind feature/config
5. Add `IndexReader` APIs:
   - `chunks_for_query`
   - `symbols_for_name`
   - `definitions`
   - `references`
   - `diagnostics_for_path`
   - `test_owners_for_path`
6. Replace `ContextCompiler::compile_workspace` internals to prefer `IndexReader` when `.quorp/index` exists, with fallback to current lexical path.
7. Add `ContextPackV2` or extend model types if backward compatible:
   - `selected_reason`
   - `source_hash`
   - `trust_level`
   - `freshness`
   - `budget_cost`
8. Add CLI:
   - `quorp index build`
   - `quorp index status`
   - `quorp index explain <symbol>`
   - `quorp index watch` later, after build/status are stable

Key tests:

- Index invalidation: edit one file, rebuild, only that file changes.
- Deterministic budget: same query/order gives same pack.
- Symbol lookup: Rust function appears with path/range.
- Context pack provenance includes source hash and reason.
- Fallback: no index still uses existing `compile_workspace`.

Commands:

- `cargo test -p quorp_context --lib`
- `cargo test -p quorp_repo_scan --lib`
- `cargo test -p quorp --bin quorp index`
- `quorp index build --workspace .`
- `quorp index status --workspace .`

Definition of done:

- Context packs are reproducible and explainable.
- Index survives process restart.
- Compiler uses persistent repo facts before broad lexical scanning.

### 6. Memory And Rule Forge V2

Current code:

- Memory store is in `crates/quorp_memory/src/quorp_memory.rs`.
- Current key APIs: `Memory::with_workspace`, `record`, `recall`, `query_evidence`, `retry_decision`, `failed_attempts_for_signature`.
- Rule forge is in `crates/quorp_rule_forge/src/quorp_rule_forge.rs`.
- Current key APIs: `RuleForge::observe_failure`, `maybe_emit_candidate`, `observe_packet_failure`, `record_shadow_result`, `promote`, `active_rules`.
- Runtime durable consumers currently create `RuleForge::new()` in memory and write observation journals.

Target design:

- Rule forge state persists under `.quorp/rules/`.
- Memory negative retry logic is injected during runtime turn/action selection.
- Runtime injects only top triggered rules, not all active rules.

Implementation steps:

1. Add persistent `RuleStore`:
   - root `.quorp/rules`
   - folders `candidate`, `shadow`, `active`, `rejected`, `deprecated`
   - each rule card is JSON or TOML
2. Rule card fields:
   - `id`
   - `scope`
   - `trigger`
   - `statement`
   - `evidence_hashes`
   - `prevented_failures`
   - `false_positives`
   - `confidence`
   - `owner_paths`
   - `state`
   - `created_at_unix`
   - `updated_at_unix`
3. Change `RuleForge::new()` to keep an in-memory option but add `RuleForge::with_workspace(path)`.
4. Change durable runtime consumers to call `RuleForge::with_workspace(workspace_root)`.
5. Persist cluster counts and emitted candidates so process restart does not reset learning.
6. Add rule-trigger selection:
   - input: current action, failure packet, file path, symbol path, message skeleton
   - output: top N rules by confidence/relevance
7. Add memory negative retry gate in `AgentTaskState` or turn dispatcher:
   - before repeating a failed patch hash/evidence hash, call `Memory::retry_decision`
   - if blocked, emit runtime event and ask for new evidence
8. CLI commands:
   - `quorp rules list`
   - `quorp rules explain <id>`
   - `quorp rules shadow <id>`
   - `quorp rules promote <id>`
   - `quorp rules reject <id>`
   - `quorp memory recall <query>`
   - `quorp memory evidence <query>`
   - `quorp memory prune`

Key tests:

- Candidate persists after restart.
- Shadow result promotes after required prevented failures.
- False positives reject/deprecate.
- Negative memory blocks identical failed fix and allows changed patch hash.
- Runtime consumer writes rule card from repeated failure events.

Commands:

- `cargo test -p quorp_memory --lib`
- `cargo test -p quorp_rule_forge --lib`
- `cargo test -p quorp_session --lib durable_runtime_consumers_persist_memory_and_journals -- --test-threads=2`
- `cargo test -p quorp --bin quorp rules`
- `cargo test -p quorp --bin quorp memory`

Definition of done:

- Rule lifecycle survives process restart.
- Runtime prompts are not flooded with all rules.
- Identical failed fixes are blocked unless evidence or patch changes.

### 7. Task DAG And Subagents

Current code:

- Autonomous controller exists in `quorp_plan_mode` per tracker, but the full ledgered task DAG is not in the visible runtime surface.
- Runtime loop and events live in `crates/quorp_agent_core/src/runtime.rs` and `runtime/*`.
- Current event flow can persist runtime events through headless runners.

Target design:

- Task DAG is a first-class ledger artifact.
- Subagents run as scoped workers with assigned files, capabilities, context budget, sandbox/worktree policy, max turns, and proof requirements.
- Results return through ledger artifacts and patch/proof receipts.

Implementation steps:

1. Add model types in `quorp_agent_core`:
   - `TaskRun { run_id, root_node_id, status, started_at, finished_at }`
   - `TaskNode { node_id, parent_id, role, objective, assigned_files, capabilities, context_budget, worktree_policy, max_turns, proof_requirement, status }`
   - `TaskNodeResult { node_id, summary, patch_receipts, proof_receipts, artifacts, confidence, next_actions }`
   - `AgentRole { Planner, Explorer, Implementer, Verifier, Reviewer, Security, Docs, Browser }`
2. Add runtime events:
   - `TaskNodeCreated`
   - `TaskNodeStarted`
   - `TaskNodeFinished`
   - `TaskNodeFailed`
   - `FileLeaseRequested`
   - `FileLeaseGranted`
   - `FileLeaseDenied`
3. Persist all task events to ledger before UI fanout.
4. Add file lease manager:
   - tracks path glob ownership per task node
   - denies overlapping writes
   - read-only exploration can overlap
5. Add sandbox/worktree policy:
   - implementer nodes get isolated tmp-copy or git worktree
   - verifier/reviewer can read artifacts and proof, not mutate source
6. Add subagent execution harness:
   - start with in-process sequential runner for tests
   - then add parallel worker threads/processes only after ledger/proof is stable
7. Add patch tournament mode:
   - spawn N implementers
   - each returns Patch VM receipts plus proof reports
   - verifier/reviewer ranks by proof score and patch minimality
8. CLI:
   - `quorp agents list`
   - `quorp agents show <node-id>`
   - `/agents` in TUI displays nodes and statuses

Key tests:

- DAG serialization and parent/child validation.
- File lease conflict denies overlapping write assignment.
- Subagent result cannot include raw hidden context; only artifacts.
- Isolated worktree does not mutate source checkout until selected.
- Patch tournament picks passing minimal patch over failing/larger patch.

Commands:

- `cargo test -p quorp_agent_core --lib task`
- `cargo test -p quorp_session --lib subagent`
- `cargo test -p quorp --bin quorp agents`

Definition of done:

- Every subagent transition is replayable from ledger.
- File leases prevent parallel write conflicts.
- Results are proof-backed and artifact-based.

### 8. TUI And Product Parity

Current code:

- Slash registry is `crates/quorp_slash/src/quorp_slash.rs`.
- Inline slash parsing is split between `quorp_slash` and `quorp_term`.
- Scrollback inline runtime is `crates/quorp/src/quorp/cli_runtime.rs`.
- Doctor/commands/permissions demos are in `crates/quorp/src/quorp/cli_demos.rs`.
- Fullscreen path is selected in `run_inline_cli` through `run_fullscreen_cli`.

Target design:

- Fullscreen mission control has panes:
  - transcript
  - task DAG
  - current diff
  - diagnostics/tests
  - proof
  - context pack
  - memory/rules
  - permissions/sandbox
  - subagents
  - MCP/browser/GitHub status
- Slash commands execute, not just list.

Implementation steps:

1. Unify slash command source of truth:
   - either make `quorp_term::SlashCommand` generated/mapped from `quorp_slash::Registry`
   - or add missing command specs to both with tests that compare names
2. Define `SlashAction` enum:
   - `Plan`, `Permissions`, `Sandbox`, `Context`, `Compact`, `Proof`, `Diff`, `Rollback`, `Verify`, `Memory`, `Rules`, `Mcp`, `Settings`, `Model`, `Agents`, `Browser`, `Github`, `Resume`, `Export`
3. Create `SlashDispatcher` used by both scrollback and fullscreen:
   - input: parsed command plus session state
   - output: state mutation, command job, or rendered panel update
4. Implement command handlers incrementally:
   - `/plan` changes run mode
   - `/permissions` updates permission mode or opens policy pane
   - `/sandbox` updates sandbox state with trust checks
   - `/verify` calls `quorp_verify`
   - `/proof` shows proof DAG/receipt
   - `/diff` reads current git diff
   - `/rollback` uses Patch VM rollback token
   - `/context` shows current context pack
   - `/memory` and `/rules` call persisted stores
   - `/agents` shows task DAG
5. Add pane state model:
   - `MissionControlState { active_pane, transcript, task_dag, diff, diagnostics, proof, context, memory_rules, permissions, integrations }`
6. Add queued user input:
   - long-running task can keep running or pause
   - input events become ledger events
   - cancellation/pause is explicit
7. Add rendering snapshots for no-color and truecolor.

Key tests:

- Slash registry parity test: all required commands exist.
- Dispatcher test: `/verify` enqueues verification job.
- Dispatcher test: `/permissions auto-safe` changes policy and emits ledger event.
- Fullscreen render snapshot includes pane labels and active state.
- Long task input queue preserves input order.

Commands:

- `cargo test -p quorp_slash --lib`
- `cargo test -p quorp_term --lib`
- `cargo test -p quorp --bin quorp slash`
- `cargo test -p quorp_render --lib`
- Manual smoke: `QUORP_TUI_MODE=fullscreen quorp`

Definition of done:

- Required slash commands execute in both scrollback and fullscreen.
- Panes reflect live ledger/proof/context/task state.
- Long tasks can pause/resume without losing ledger state.

### 9. Evals And Release

Current code:

- Benchmark CLI is in `crates/quorp/src/quorp/cli.rs` and benchmark modules under `crates/quorp/src/quorp/benchmark/`.
- Scoreboard/proof receipt code is in `crates/quorp/src/quorp/benchmark/reporting.rs`.
- Deterministic gate script is `script/quorp-benchmark-regression-gate`.
- Security lane is in `justfile`.

Target design:

- Broader eval lanes with replayable scoreboards and no benchmark-oracle reachability in normal runtime.
- Release only with license metadata, SBOM/checksums/signing, and `quorp replay` reconstruction.

Implementation steps:

1. Extend benchmark suite model with lane metadata:
   - Rust SWE top-N
   - private/fresh tasks
   - multilingual tasks
   - context recall
   - terminal-bench
   - browser/frontend
   - MCP poisoning
   - sandbox escape
   - rule-forge regression
   - patch minimality
2. Add `eval_only` feature/config switch:
   - exact benchmark playbooks require `PolicyMode::BenchmarkAutonomous`
   - audit gate proves no benchmark playbook calls bypass policy
   - consider moving exact playbooks into an eval-only module gated by cargo feature once tests can support it
3. Scoreboard additions:
   - replay success
   - proof completeness
   - context precision/recall
   - permission prompt count
   - patch minimality score
   - sandbox/network violations
4. Add replay validation:
   - for each scored run, call ledger validation
   - reconstruct transcript/proof summary
   - compare final report facts
5. Add release artifacts:
   - SBOM generation script
   - checksum generation
   - signing placeholder or real signing path
   - license metadata review file
6. Add CI/local gates:
   - `just benchmark-gate`
   - `just release`
   - `quorp replay <run-dir>`
   - `quorp proof verify <run-dir>`

Key tests:

- Scoreboard includes replay success field.
- Benchmark score fails if run ledger is invalid.
- Normal runtime cannot trigger exact benchmark playbooks.
- Patch minimality score detects large broad rewrites.
- Release script fails if SBOM/checksum missing.

Commands:

- `just benchmark-gate`
- `cargo test -p quorp_benchmark --lib`
- `cargo test -p quorp --bin quorp score_benchmark_reports_writes_session_scoreboard`
- `cargo test -p quorp --bin quorp replay`
- `just release`

Definition of done:

- Scoreboards are reproducible from ledger plus artifacts.
- Release gate proves artifacts, checksums, and proof verification.
- Eval-only deterministic playbooks are unreachable in normal runtime.

### 10. Final Acceptance Checklist

Run this full set before calling the roadmap complete:

- `cargo fmt --all -- --check`
- `cargo check --workspace`
- `cargo test --workspace --lib`
- `cargo test --workspace --tests --exclude util -- --test-threads=2`
- `./script/check-loc-cap 2000 --error`
- `./script/quorp-audit-gaps`
- `./script/quorp-write-path-audit`
- `./script/clippy`
- `just benchmark-gate`
- `just medium`
- `just deep`
- `just security`
- `cargo build -p quorp`
- `quorp --help`
- `quorp doctor`
- `quorp scan`
- `quorp replay <known-good-run-dir>`
- `quorp proof verify <known-good-run-dir>`

Completion criteria:

- No `NEEDS_REVIEW` item remains without a concrete follow-up issue or tracker entry.
- Every phase has a proof command listed and recently run.
- Ledger, proof, context, memory/rules, task DAG, and eval artifacts all survive process restart.
- Normal runtime cannot reach benchmark oracle patches.
- Full-auto cannot run on host without a sandbox/trust policy explicitly allowing it.

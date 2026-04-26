# Minimum Semantic Surface: A Token-Aware Operating Standard for Agent-Efficient Rust Codebases

Author: Jepson Taylor (VEOX Inc., Research Group, jepson@veox.ai)
Survey cutoff: April 23, 2026
Format: Detailed Markdown companion to `main.tex`. The TeX source is canonical; this mirror exists to give agent readers a structured, paste-friendly version of the same paper with all sections, tables, code listings, figures (described), and the appendix.

Evidence tiers used throughout:
- [Evidence-backed] direct or adjacent peer-reviewed / preprint evidence
- [Mechanism] official Rust/Cargo or first-party tool documentation
- [Production substrate] first-party engineering posts about Rust adoption
- [Project claim] this research group's local implementation evidence
- [Proposed doctrine] this paper's synthesis or proposed future system

---

## Abstract

[Proposed doctrine]

Repository-scale coding agents fail less from syntax ignorance than from repository-control errors: wrong owner, hidden contract, wrong proof lane, setup friction, noisy evidence, retry burn, and security-blind success. We propose minimum semantic surface (MSS) as a review-backed Rust operating standard: expose the smallest high-signal set of navigation, contract, proof, diagnostic, and policy artifacts needed for a valid change. Rust is a strong substrate because ownership, typed APIs, module privacy, Cargo metadata, compiler diagnostics, and supply-chain tooling convert architectural boundaries into executable constraints rather than ambient prose. The paper synthesizes repository-agent, Rust-specific, context, security, and tooling evidence; defines MSS and its canonical artifacts; and specifies token discipline, proof lanes, and review receipts for repositories that aim to export the smallest lawful repair program while preserving correctness, security-gated validation, auditability, and human review. A bounded runtime mediation layer is discussed only as companion evidence for residual waste classes that survive after repository shape has been disciplined; it is not presented as core proof of MSS.

Keywords: Rust, coding agents, token efficiency, software architecture, repository-scale repair, verification, software security, developer productivity.

---

## 1. Introduction

[Evidence-backed]

Coding agents rarely fail because they cannot emit Rust syntax. They fail because the repository does not tell them, early and cheaply, *who* owns a surface, *what* contract must remain true, *how* a change is proved, and *what* evidence must not be hidden. A typical failure trace is now familiar: broad search, wrong file, plausible patch, visible green, hidden invariant miss, then a second round spent reconstructing what should have been explicit at the outset.

Repository-scale agent work is therefore a repository-interface problem as much as a model problem. SWE-bench established execution-based issue repair, and SWE-agent showed that navigation, editing, and testing interfaces materially change outcomes. Subsequent work broadens the question from "can a model patch?" to "can a system reach correct, security-gated, reviewable change at acceptable cost?" Context studies sharpen the same point: more retrieved text is not automatically better, because agents over-retrieve, under-use explored context, and can be harmed by broad instruction files or mismatched skill packs. Careful workflow studies are equally sobering: experienced open-source developers were measured slower on their own repositories with early-2025 AI tooling, and more recent field work finds that professional developers supervise, scope, and control agent loops rather than let them run free.

[Production substrate + Evidence-backed]

Rust deserves focused treatment because, in agentic development, the programming language is part of the control system. Ownership, enums, traits, module privacy, Cargo workspaces, compiler diagnostics, tests, and typed errors can make architectural boundaries executable rather than merely described. Rust-SWE-bench and RUSTFORGER quantify the Rust-specific bottleneck directly: across 500 issue-resolution tasks drawn from 34 real Rust repositories, the RUSTFORGER baseline resolves 21.2% of tasks and the reported configuration reaches 28.6%. Production reports show that Rust is already used where the cost of wrong changes is high: Google's 2025 Android Rust post reports roughly 1000× lower memory-safety vulnerability density, ~4× lower rollback rate, and ~25% less code-review time versus Android C/C++ changes. Cloudflare, AWS, Firecracker, and Hugging Face Tokenizers corroborate the substrate maturity story.

[Proposed doctrine]

This paper takes a bounded stance. Rust is not always easiest for agents to write and not always cheapest by raw token count. It is a strong mainstream default when mistake cost dominates first-edit speed: security-sensitive services, parsers, protocol code, CLIs, infrastructure tools, long-lived libraries, and concurrency-heavy systems. The paper's contribution is a review-backed operating standard for such codebases. It defines the vocabulary and review discipline needed to separate evidence, project claims, and doctrine, and it maps common agent failure stages to concrete Rust repository controls. It specifies canonical repository artifacts, proof lanes, token-accounting rules, and reviewer receipts. A bounded runtime companion layer appears later only to show which residual waste classes remain visible once repository shape has already been disciplined. It closes with three buildable future systems (`cargo-mss`, ProofLens, `cargo-obligation-cache`) that could materially shrink navigation waste, proof-output waste, and repeated-proof waste without weakening correctness or security.

---

## 2. Review Method and Evidence Taxonomy

[Proposed doctrine]

This manuscript is a review and operating standard, not a local benchmark report. Because no counts of screened or excluded sources are reported, the protocol below is labeled a structured review protocol rather than a full PRISMA-style systematic review. Its role is to make evidence classes explicit so the reader can audit which later claims are public, official, project-reported, or doctrinal.

**Source classes searched (7).**
- (i) repository-agent benchmarks and failure analyses
- (ii) Rust-specific repair and migration studies
- (iii) context, pruning, and token-efficiency work
- (iv) harness and tool-interface guidance from major agent platforms
- (v) official Rust and Cargo documentation
- (vi) first-party production Rust engineering posts
- (vii) project documentation for tools that directly affect proof cost, contract drift, or token shaping

**Search themes:** repository repair, Rust-specific repair, context pruning, repository-level instruction files, continuous integration, build-loop acceleration, supply chain, secure agent operation.

**Date window:** primary search closed April 23, 2026; live refresh windows are tracked per-claim in the supplement claim ledger.

**Inclusion rules:**
- (a) primary peer-reviewed studies and preprints with described method
- (b) official project and standards-body documentation
- (c) first-party engineering posts when used as production-substrate evidence
- (d) tool documentation when used as mechanism-plausibility evidence

**Exclusion rules:**
- (a) anonymous summaries
- (b) vendor leaderboard claims without method
- (c) listicle-style roundups
- (d) crate recommendations justified only by popularity

**Evidence classes:** six labeled explicitly in-text — direct evidence, adjacent evidence, mechanism evidence, production substrate, project claim, proposed doctrine.

**Risk-of-bias sentence:** company engineering posts select for publishable wins, project documentation selects for mechanism favorability, and popular benchmarks carry contamination risk as they leak into training data; we call these out in-line where they materially affect a claim.

---

## 3. Agent Failure Funnel

[Evidence-backed + Proposed doctrine]

Minimum semantic surface starts from a failure model. Agents do not merely "need context"; they fail at identifiable stages, each of which has a corresponding repository control and a measurable metric. Failures often progress through six stages — wrong owner, wrong contract, wrong patch scope, wrong proof lane, unsafe evidence compression, security-blind success, and a reviewer-reconstruction tax that charges the human even when the patch is nominally correct — though a given session can enter at any stage. Each stage is an independent failure class: fixing owner discovery does not close the security gate, and compressing tool output does not fix wrong-owner edits.

### Table I: Failure funnel × motivating evidence × Rust response

| failure stage | typical agent symptom | motivating evidence | Rust repository response |
| --- | --- | --- | --- |
| wrong owner | broad search, wrong crate, many files opened | localization and graph-guidance work | owner map, narrow crates, test map, rust-analyzer navigation |
| hidden contract | visible pass, hidden invariant regression | Rust repair and migration work | newtypes, private fields, validated constructors, generated contracts, local tests |
| wrong patch scope | broad speculative patch; cross-crate drift | AGENTS.md and context studies | legal edit zones, generated-zone markers, widening rules |
| wrong proof lane | too little validation or whole-workspace thrash | setup, CI, and harness studies | canonical proof lanes, change-type routing, one-command setup |
| noisy evidence | compiler/test/log output dominates context | cost and pruning work | JSON diagnostics, failure-first summaries, raw-output tee |
| security-blind success | functional patch remains exploitable | secure-correctness and standards | security-gated lanes, secret scanning, unsafe ledger, dependency review |
| reviewer reconstruction tax | human cannot reproduce why a change is correct | workflow and productivity research | proof receipts, contract diffs, stable diagnostic codes |

### Worked trace: one issue through all six stages

[Project claim]

Consider a small-but-realistic billing issue: *"grace-period users are billed one cycle early when they upgrade plans."* An unprepared agent begins with a repository-wide search for "grace" (**wrong owner**: 40+ files opened, most in the web layer) and proposes a patch in `api-server/routes/billing.rs` that changes a status-code mapping (**wrong contract**: the invariant lives in `crates/billing-domain/src/plan_change.rs`, gated by a private constructor). Workspace tests pass (**unsafe evidence compression**: the domain unit test that exercises the boundary is not triggered because the patch is upstream of it) and a full `cargo test --workspace` is run to "be sure" (**wrong proof lane**: 9 min vs the 70 s fast lane scoped to `-p billing-domain`). Review catches the functional bug, but the dependency upgrade that shipped with it introduces a known advisory (**security-blind success**); the reviewer then spends another 30 min reconstructing which commands were run and against which commit (**reviewer reconstruction tax**). An MSS-compliant repository turns every one of those stages into a local artifact lookup: the owner map names `billing-domain` as owner of grace-window logic; the generated contract names the one function whose signature must remain stable; `just fast -p billing-domain` is the canonical fast lane; the security lane surfaces the advisory on the same CI gate; and the proof receipt emits the commands, hashes, and owners so the reviewer reads one page instead of replaying one hour.

---

## 4. Evidence Landscape

[Evidence-backed]

Three evidence families do the most work for the token-aware standard:

- **ContextBench** reports that explored and actually used context diverge materially on repository tasks, which argues for routing and pruning rather than indiscriminate long context.
- **SWE-Effi** argues that time and token cost belong in evaluation rather than as afterthoughts, which grounds the paper's ETTS and SecureETTS objectives.
- **SUSVIBES** shows that functional success and secure success diverge badly on real-world tasks, which is why security must be a separately gated proof lane.

Rust-specific work tightens the picture: Rust-SWE-bench and RUSTFORGER identify repository structure, strict type and trait semantics, and issue reproduction as first-order difficulties. RustAssistant, AutoVerus, and Debug2Fix show that compiler diagnostics and proof generation are usable levers for narrowing agent edits. RustEvo2 emphasizes that API evolution and safe migration are recurring confusion surfaces. Recent scaffolding work shows that repository layout itself shapes LLM agent behavior before any prompt is read. Harness guidance from OpenAI and Anthropic converges on three mechanical levers: short and executable agent-facing instructions, context preservation across long runs, and tool interfaces that summarize without hiding evidence.

[Proposed doctrine]

The convergent lesson is that agents fail less from lack of code-generation capacity than from poor repository interfaces. Repository-agent benchmarks motivate owner maps, one-command setup, and proof-lane routing; Rust-specific studies motivate typed contracts and generated boundaries; context work motivates short routers and bounded manifests; harness guidance motivates machine-readable diagnostics and raw-evidence paths; and security plus workflow work motivates a separate security gate and reviewer receipts.

---

## 5. Why Rust, and When Not Rust

[Mechanism + Production substrate]

Rust is not the easiest language for agents to write. It is one of the best languages for making wrong agent edits fail early, locally, and diagnostically. Ownership and borrowing express aliasing constraints; enums express closed state; traits expose capability surfaces; module privacy limits accidental reach; Cargo exposes a machine-readable workspace graph; and the ecosystem provides well-documented tools for features, APIs, unsafe code, contracts, tests, and supply chain. Rust adoption at Google, Cloudflare, AWS, Firecracker, and Hugging Face Tokenizers shows substrate maturity in security-, performance-, and correctness-critical settings.

[Proposed doctrine]

Rust is a strong mainstream default when mistake cost dominates first-edit speed: security-sensitive services, parsers, protocol code, CLIs, infrastructure tools, long-lived libraries, and concurrency-heavy systems. It is not the right pick for: simple operational services where Go is preferable, UI surfaces where TypeScript remains pragmatic, exploratory data work where Python is strongest, and hardware or legacy edges where C or C++ may still be required.

---

## 6. Minimum Semantic Surface Standard

### 6.1 The Five Surfaces

[Proposed doctrine]

MSS is the smallest high-signal set of artifacts that answers owner, contract, proof, diagnostic, and policy questions before a broad edit:

1. **Navigation surface** — owner map, narrow crates, test map, AGENTS.md routing.
2. **Contract surface** — typed IDs, validated constructors, generated schemas, public-API checks.
3. **Proof surface** — fast / medium / deep / security / release lanes, each one canonical command.
4. **Diagnostic surface** — JSON diagnostics, stable error codes, raw-output tee, failure-first summaries.
5. **Policy surface** — legal edit zones, security-gated lanes, unsafe ledger, review receipts.

### 6.2 Canonical Repository Shape

The canonical MSS repository keeps invariants in narrow domain crates, pushes I/O to adapters, separates generated from handwritten code, and publishes agent-facing artifacts under a machine-readable `agent/` directory.

**Listing 1 — Canonical MSS workspace tree (compact):**

```
crates/
  domain/        # invariants, no I/O
  application/   # use cases, orchestration
  adapters/      # I/O, drivers, side effects
  api-server/    # HTTP surface
generated/       # generated, never hand-edited
agent/
  AGENTS.md
  generated-zones.toml
  proof-lanes.toml
  unsafe-ledger.toml
  test-map.json
  owner-map.json
```

**Listing 2 — Canonical root `AGENTS.md` (excerpt):**

```
Mission: invariants live in crates/domain.
Start: just fast.  Handoff: just medium.
Map: domain=logic; application=use cases; adapters=I/O.
Forbidden: generated/ and paths in agent/generated-zones.
Never compress: exit code, failing test name,
panic text, span, advisory ID, fuzz seed,
raw-log path, raw-log hash.
Security lane: authz, secrets, unsafe, FFI, CI/CD, shell.
```

The governing rules are: generated zones must be explicit, root instructions must stay short, proof lanes must be commandable rather than prose, and raw output must remain recoverable by stable path or hash.

---

## 7. Agent-Efficient Rust Architecture

### 7.1 Four Canonical Micro-Patterns

[Mechanism + Proposed doctrine]

The recurring patterns are small and conservative. Each narrows the legal edit zone and concentrates the first failing proof on one owner, so an agent that misreads the surface is corrected by `rustc` rather than by a reviewer.

#### Typed IDs

Raw strings lose their identity at the function boundary and invite silent mix-ups between users, orders, and sessions. A newtype keeps the compiler honest.

```rust
// before: any string passes typecheck
fn load_user(id: String) -> User { /*...*/ }

// after: only UserId satisfies the signature
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserId(String);
impl UserId {
    pub fn parse(s: &str) -> Result<Self, DomainError> {
        if s.starts_with("u_") && s.len() <= 36 {
            Ok(Self(s.to_owned()))
        } else { Err(DomainError::BadUserId) }
    }
}
fn load_user(id: UserId) -> User { /*...*/ }
```

#### Validated constructor

Public mutable fields let a patch skip the invariant. A private field with a `new()` returning `Result` forces the check to live in exactly one place.

```rust
// before: invariant lives in prose
pub struct Job { pub deadline: Timestamp }

// after: invariant lives in code
pub struct Job { deadline: Timestamp }
impl Job {
    pub fn new(d: Timestamp) -> Result<Self, JobError> {
        if d > Timestamp::now() { Ok(Self { deadline: d }) }
        else { Err(JobError::DeadlineInPast) }
    }
    pub fn deadline(&self) -> Timestamp { self.deadline }
}
```

#### Enum state machine

A bag of booleans lets illegal combinations compile. An enum collapses the state space so `match` on every edit path tells the agent what still needs handling.

```rust
// before: 2^3 = 8 possible combinations,
//   most of which make no sense
pub struct Task {
    pub is_running: bool,
    pub is_done:    bool,
    pub has_error:  bool,
}

// after: four legal states only, match-exhaustive
pub enum TaskState {
    Pending,
    Running,
    Done,
    Failed(DomainError),
}
```

#### Typed errors with stable codes

`Result<T, String>` hides the failure taxonomy inside free text, so both the agent and the reviewer re-derive it each time. A typed error carries stable identity and preserves raw evidence by construction.

```rust
// before: failure taxonomy lives in strings
fn load_user(id: UserId) -> Result<User, String> { /*...*/ }

// after: failure taxonomy is the type
#[derive(Debug, thiserror::Error)]
pub enum LoadUserError {
    #[error("E_USER_NOT_FOUND: {0}")]
    NotFound(UserId),
    #[error("E_USER_STORE_IO")]
    Store(#[from] std::io::Error),
    #[error("E_USER_DECODE")]
    Decode(#[from] serde_json::Error),
}
fn load_user(id: UserId) -> Result<User, LoadUserError> { /*...*/ }
```

These four are compile-time MSS primitives: each one moves an invariant out of prose and into a type, which is the cheapest way to stop an agent from patching around the invariant instead of honoring it.

### 7.2 Additional Rust Rules

[Mechanism]

- **SAFETY comments on every `unsafe` block.** Rust API Guidelines and ecosystem conventions expect a `// SAFETY:` comment justifying each `unsafe` block; the unsafe ledger in `agent/unsafe-ledger.toml` keeps these reviewable.
- **Additive, unified features.** Cargo features unify across the dependency graph: the union of enabled features applies to every build, so features must be additive by policy rather than inferred.
- **Async ownership and cancellation.** Never hold a `MutexGuard` across `.await`; prefer bounded channels and explicit shutdown to unbounded `Arc`-of-`Mutex` conventions; use blocking-task handoff for CPU or blocking I/O inside async contexts.
- **Public API hygiene.** Use public-API checks, semver checks, `cargo-check-external-types` for unintentional type leakage, and an explicit MSRV; `#[non_exhaustive]` on public enums prevents accidental breaking changes.
- **Generated contracts.** Schema, API, protobuf, FFI, and packaging tools should make boundary truth single-sourced rather than hand-copied.

File size hurts agents via token locality before it hurts `rustc` via compile time: a file that mixes domain and I/O, wiring and policy, or handwritten and generated code forces broad reads on every task. Keep files within one owner concept when possible, split when multiple invariants accumulate, and treat wide multi-owner patches as design-review events rather than silent widening.

---

## 8. Token Economy, Proof Lanes, and Security-Gated Validation

### 8.1 Definitions

[Proposed doctrine]

Let `C_i` be model-visible or charged tokens for attempt `i`, `S` the first hidden-passing attempt, `S_sg` the first hidden-and-security-gated passing attempt, and `R` the retry cap:

- `ETTS_R = E[ sum_{i=1..min(S,R)} C_i ]`
- `SecureETTS_R = E[ sum_{i=1..min(S_sg,R)} C_i ]`

### 8.2 Safe-Compression Rule and Guardrails

[Evidence-backed]

A token reduction is a savings only if it preserves decisive evidence and does not increase hidden failures, security regressions, false-green summaries, or reviewer reconstruction cost. Published compression evidence is convergent: task-conditioned pruning reports 23–54% token reductions with minimal quality loss; task-conditioned tool-output pruning reports up to 92% input-token removal while preserving recall and F1; repository-sufficient context compression reports 51.8–71.3% token-budget reductions while improving resolution by 5.0–9.2%; and tool-schema collapse via Code-Mode-style evaluation of API surfaces reports reductions of roughly two orders of magnitude on tool-definition tokens.

| surface | safe summary preserves | never drop |
| --- | --- | --- |
| compiler output | primary span, crate, error code, first diagnostic family | exit code, decisive span, error identity |
| test output | failing test name, panic summary, seed, owner crate | failing test identity, panic text, raw log path |
| security scans | advisory or secret class, affected package or path | advisory ID, severity, file path, raw artifact |
| traces and logs | relevant span tree and terminal failure event | crash marker, request ID, panic or denial signal |
| generated diffs | source-of-truth artifact and changed contract kind | raw diff path, generation command, schema identity |

### 8.3 Opportunity Envelope

[Proposed doctrine, anchored to evidence-backed bands]

**Figure 1 (envelope).** Cumulative SecureETTS opportunity envelope across four doctrinal tiers. Tiers compound multiplicatively over overlapping token pools, so the top-tier midpoint (~80%) is not the linear sum of per-system contributions. Each band is a range, not a point.

| Tier | Midpoint | Range | What's in the tier |
| --- | --- | --- | --- |
| 0 — Baseline | 0% | reference | unstructured repository, no routing, no packets, no runtime layer |
| 1 — Repository-native | ~50% | 45–55% | MSS (AGENTS.md routing, owner maps, JSON diagnostics, proof-lane routing) plus published compression: 23–54% from task-conditioned pruning, up to 92% on individual tool calls, 51.8–71.3% repository-sufficient context compression, plus project-reported command-output filtering |
| 2 — +Residual runtime | ~65% | 60–70% | adds a mediation layer that attacks the residual carriers in §10; grounded in local corpus evidence and project mechanism only |
| 3 — +All flagships and upstream horizon | ~80% | 75–85% | adds `cargo-mss`, ProofLens, `cargo-obligation-cache`, plus `cargo-agentmode`, `#[agent(...)]`, `cargo-trace-autopilot` under their falsification criteria |

In the rendered figure, each row's solid colored portion is the cumulative reduction at that tier; light gray fills the rest, marking unrealized opportunity. None of the numbers are deployment outcomes.

### 8.4 Savings Hierarchy

The ordering of token-saving levers is strict: first eliminate wrong-owner search, then wrong-proof-lane selection, then duplicated schema or context reads, then compress proof output safely, and only then optimize free-form narration. Validated savings are lower SecureETTS without loss of hidden correctness, security proof, raw evidence, or review quality.

| token class | waste pattern | preferred control |
| --- | --- | --- |
| instruction | repeated broad root guidance | short router plus path-local docs |
| navigation / search | repeated probing for ownership | Cargo metadata, owner maps, rust-analyzer, semantic search |
| file-read | whole files opened to find one symbol | signature-first summaries, test maps, rust-analyzer hover |
| reasoning | model infers hidden ownership or feature state | bounded crates, explicit feature matrix, generated contracts |
| patch | broad speculative edits | legal edit zones, patch-width review triggers |
| proof-output | full compiler / test / log output pasted back | JSON diagnostics, failure-first summaries, raw-output tee |
| wrong-turn / recovery | repeated edits in wrong crate or lane | typed diagnostics, repair packets, owner-aware proof hints |
| review-transfer | reviewer reconstructs patch story | proof receipt, changed-owner list, raw hashes |

### 8.5 Low-Token Narration Policy

[Proposed doctrine]

Agent-facing prose should be concise, free of filler, and biased toward short receipts rather than theatrical narration, but it must never compress away code, commands, paths, identifiers, failing-test names, panic text, compiler error codes, exit codes, advisory IDs, fuzz seeds, spans, line numbers, raw-log paths or hashes, or tool versions. Apply the policy to root and path-local instruction files, proof-lane summaries, repair packets, failure receipts, progress updates, and handoff notes. Do not apply it to SAFETY comments, authz logic, public API documentation, security reasoning, migration plans, or schema ownership.

### 8.6 Proof Lanes and Security-Gated Validation

[Mechanism + Proposed doctrine]

The repository should publish the smallest deterministic proof that matches the changed surface.

| lane | use when | canonical commands | escalation |
| --- | --- | --- | --- |
| fast | any code edit | `cargo fmt --all --check`; `cargo check --workspace --all-targets --message-format=json`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo nextest run -p <owner>` | public API or feature change |
| medium | public behavior, schema, feature, integration | workspace nextest, doctests, snapshots, compile-fail, feature matrix via `cargo hack` | hidden boundary or broader owner spread |
| deep | unsafe, parser, state-machine, concurrency, perf | fuzzing, Miri, Loom, Kani, mutation, coverage, Criterion | aliasing, ordering, panic risk |
| security | authz, secrets, unsafe, FFI, CI, migration, shell, deserialization, path / URL / SSRF | `cargo-deny`, `cargo-audit`, `cargo-vet`, `cargo-about`, `cargo-geiger`, `cargo-scan`, `gitleaks`, `detect-secrets`, Syft, Grype, `actionlint`, `zizmor` | advisory, secret, workflow, unsafe finding |
| release | ship gate | `cargo metadata --no-deps --format-version 1`; `cargo build --timings`; `cargo about generate`; `cargo auditable build --release` | external rollout checks |

Each lane should have a single `just` entry: `just fast` (30–120 s), `just medium` (<10–15 min), `just deep` (trigger-only), `just security` (trigger-only), `just release` (ship gate). Nightly-dependent tools (Miri) should be called out explicitly.

Build-loop acceleration: workspace `default-members`, package-scoped `-p`, stable `CARGO_TARGET_DIR`, `cargo-nextest` for fast and medium, diagnose with `cargo build --timings`, `sccache` only where measured, `Swatinem/rust-cache` in CI, `cargo-chef` for container layers only, and `bacon`, `cargo-binstall`, or `cargo-hakari` only where workspace size justifies them.

Flake policy: `cargo-nextest` retries are for identifying flakiness, not for covering wrong theories. Quarantine, attach a failure capsule, schedule a fix.

---

## 9. Rust Proof-Cost Tooling Stack and ARI-v0

### 9.1 Tiered Tooling Stack

[Mechanism + Project claim]

The tooling stack is a control surface, not a shopping list. A tool belongs in the standard only if it reduces one of: owner ambiguity, proof cost, token noise, security risk, or contract drift.

| tier | role | core tools | effect |
| --- | --- | --- | --- |
| required | fast proof | `cargo check`, `cargo fmt`, Clippy, `cargo-nextest` | earliest red signal, narrowest retries |
| required | structural navigation | Cargo metadata, rust-analyzer, `ripgrep`, `fd`, `jq` | fewer blind reads, ownership visible |
| required | command surface | `just` or `xtask` | one canonical command per lane |
| required | output shaping | JSON diagnostics, `tracing`, `miette`, `thiserror`, `anyhow` | structured diagnostics, stable failure surfaces |
| strong | contracts and drift | `serde`, `sqlx`, `schemars`, `utoipa`, `ts-rs`, `specta`, `cargo-public-api`, `cargo-semver-checks`, `cargo-hack` | generated boundary truth, explicit API drift, feature-matrix coverage |
| strong | supply chain / security | `cargo-deny`, `cargo-audit`, `cargo-vet`, `cargo-about`, `cargo-auditable`, `cargo-scan`, `gitleaks`, `detect-secrets`, Syft, `actionlint` | reviewable dependency, provenance, secret, and workflow state |
| strong | build acceleration | `sccache`, `Swatinem/rust-cache`, `bacon`, `cargo build --timings`, `cargo-binstall` | shorter iteration without hiding evidence |
| strong | output shaping (project-reported) | RTK (Rust Token Killer), `cargo-limit` | smaller visible logs with raw preserved; project-reported claims not validated under hidden-pass criteria |
| specialized | deep proof | `proptest`, `insta`, `trybuild`, `cargo-fuzz`, Miri (nightly), Loom, Kani, `cargo-mutants`, `cargo-llvm-cov`, Criterion | hidden-fail signal, UB detection, concurrency schedules, coverage, micro-bench |
| specialized | CI hardening | `zizmor`, Grype, `cross`, `cargo-hakari` (large workspaces) | safer workflows, image scanning, cross-compile, workspace-hack |
| specialized | container layer cache | `cargo-chef` (container/CI only) | dependency-layer caching in container builds |
| specialized | polyglot boundaries | Buf, `tonic`, `prost`, `openapi-typescript`, Zod, PyO3, maturin, `wasm-pack` | generated contracts across languages |
| avoid | (n/a) | `cargo-watch` (archived Jan 2025); use `bacon` or `watchexec` instead | project unmaintained |

### 9.2 Provenance: Open-Source vs Project-Reported vs Proposed

[Critical for reviewer trust]

The standard mixes three provenance classes:

**Open-source, public, third-party tools the standard cites and recommends:**
`rust-analyzer`, `cargo-nextest`, Clippy, `rustfmt`, `cargo-public-api`, `cargo-semver-checks`, `cargo-hack`, `cargo-deny`, `cargo-audit`, `cargo-vet`, `cargo-about`, `cargo-auditable`, `cargo-geiger`, `cargo-fuzz`, `cargo-mutants`, `cargo-llvm-cov`, `proptest`, `insta`, `trybuild`, Miri, Loom, Kani, Criterion, `ripgrep`, `fd`, `jq`, `just`, `bacon`, `sccache`, `cargo-chef`, `cargo-binstall`, `cargo-hakari`, `actionlint`, `zizmor`, Syft, Grype, `gitleaks`, `detect-secrets`, RTK (Rust Token Killer), `cargo-limit`, `cargo-scan`, `cargo-check-external-types`. Each is referenced in the bibliography by its public repository or documentation.

**Project-reported systems built by this research group** (used in this paper as bounded mechanism evidence rather than as proof):
- WarpOS — telemetry mediation layer (Appendix A)
- CrateAtlas — content-addressed Rust code-intelligence prototype
- `cargo-vrc`, `cargo-witness`, `witness-rt`, `cargo-aer`, `arc-bench` — local five-crate control plane referenced in §11

These are not yet released as open source; their numbers in this paper are explicitly labeled local-corpus or project-reported.

**Proposed but not yet built:**
- Flagship trio: `cargo-mss`, ProofLens, `cargo-obligation-cache`
- Upstream horizon: `cargo-agentmode`, `#[agent(...)]`, `cargo-trace-autopilot`

Every claim attached to a proposed system is doctrine paired with falsification criteria, not measured outcome.

### 9.3 Agent Readiness Index (ARI-v0)

[Proposed doctrine]

ARI is a dashboard, not a pseudo-precise scalar. It makes the MSS state of a repository legible before agents start editing, and makes regressions visible over time. Following the SPACE framework, we decline to collapse productivity into one number. Six dimensions are scored 0–4, and the overall readiness claim is capped by the lowest-scoring dimension rather than averaged.

| dimension | 0 (absent) | 1 (informal) | 2 (partial) | 3 (consistent) | 4 (MSS-compliant) |
| --- | --- | --- | --- | --- | --- |
| locality | no owner map; one large module | prose description of ownership only | partial owner map, mixed concerns | owner map plus narrow domain crates; adapter crates separate | owner map, test map, legal-edit zones, machine-readable |
| executability | no canonical commands | README lists commands informally | `just`/`xtask` exists but not deterministic | one-command setup; fast lane <2 min, reproducible | fast / medium / deep / security / release lanes each one canonical command |
| contracts | hand-copied types across boundaries | typed errors in some modules only | generated schemas exist for main boundary | generated contracts plus `cargo-public-api` / `cargo-semver-checks` | generated contracts plus public-API and semver checks gate CI |
| observability | free-form logs; varying error text | `tracing` in places; ad-hoc errors | JSON diagnostics on fast lane | JSON diagnostics plus stable error codes + raw-log handles | stable diagnostic codes, tee'd raw logs, failure-first summaries, receipts |
| security | no security lane; no secret scan | ad-hoc `cargo audit` or `cargo deny` only | one of deny/audit/secret scan in CI | deny + audit + secret scan + workflow lint in CI | full security lane + unsafe ledger + dependency-review receipts |
| maintainability | >500-LOC files routinely; no patch policy | file-size soft cap only | file-size cap plus narrow-patch guidance | patch receipts on most PRs; widening triggers review | proof receipts on every PR; widening is a first-class review event |

A readiness claim is capped when one-command setup is missing, when the fast lane is nondeterministic, when no high-risk security lane exists, or when hidden- and security-gated validation is absent. A minimum proof receipt should accompany every agent patch and at least list changed owners, contracts touched, commands run, raw artifact paths or hashes, dependency or unsafe changes, and residual risk.

**Worked example.** A billing workspace begins with a 900-LOC `utils.rs`, no owner map, a >5 minute fast lane, and no secret scan: locality scores 1, executability 0, security 0; overall readiness is capped at *not ready*. After refactor, domain invariants move into `crates/domain`, `AGENTS.md` becomes a 1.2 KB router, `just fast` falls to 70 seconds, and `gitleaks`, `cargo-deny`, and an unsafe ledger gate CI; locality rises to 3, executability to 3, security to 3, contracts and observability to 3. The dashboard reads "consistent" across all six dimensions while still naming the next missing upgrade (medium-lane feature-matrix coverage to earn a 4) explicit.

---

## 10. Residual Runtime Waste: A Companion Mediation Layer

### 10.1 Bridge from MSS at Rest to Residual Waste in Flight

[Proposed doctrine + Project claim]

MSS minimizes the lawful repair program at rest: the repository answers owner, contract, proof, diagnostic, and policy questions before an agent broadens search. A runtime mediation layer addresses the residual waste that survives in flight: repeated control chatter, replayed schemas or proof output, equivalent reruns, and wrong-turn actions that still occur after repository shape is already disciplined. WarpOS is not a second thesis. It is bounded companion evidence that the same token discipline can be applied on the live wire between agent and model.

The runtime layer stays paper-facing and carrier-centric. Its five stages are descriptive rather than product-specific:
- **Suppress** removes duplicated, inert, or wrong-routed traffic.
- **Compress** preserves decisive evidence while shrinking replayable output.
- **Reuse** avoids re-paying for conservatively equivalent work.
- **Steer** changes the next action before a wrong turn compounds.
- **Guard** interrupts destructive or false-grounded actions before they touch the machine.

### 10.2 Residual Carriers Table

| residual carrier | what MSS already removes | what runtime mediation can still remove | evidence tier | claim status |
| --- | --- | --- | --- | --- |
| control-plane chatter (Suppress) | broad search, wrong-owner probing, instruction rereads reduced by owner maps, proof lanes, short routers | duplicated bootstrap, analytics, catalog, wrong-route traffic suppressed before model-visible cost accumulates | local corpus + project mechanism | bounded local opportunity |
| replayable context and proof output (Compress) | generated contracts, JSON diagnostics, raw-output handles, failure-first lanes already narrow what's shown | repeated tool schemas, environment context, compiler/test output, SSE/log replay packetized or delta-compressed under decisive-fact preservation | public pruning evidence + local corpus | bounded local opportunity |
| repeated proof or forward-pass work (Reuse) | proof lanes and narrow crates already reduce unnecessary reruns | content-addressed checkpoints and proof certificates avoid equivalent reruns when state and obligations are conservatively unchanged | doctrine + local mechanism evidence | projected, falsifiable |
| wrong-turn and unsafe-action overhead (Steer + Guard) | legal edit zones, explicit policy, security lanes already reduce broad or unsafe edits | slice certificates, proof-call narrowing, hazard-gated nudges, exec firewalls, consistency checks interrupt wrong owner/proof choices, destructive commands, hallucinated paths, false green claims | local corpus + offline replay ceiling | projected, shadow-first |

### 10.3 WarpOS as a Residual-Waste Lens

[Project claim]

To make the residual carriers concrete and falsifiable, this paper uses a local MITM-plus-OS-telemetry prototype (WarpOS; full architecture in Appendix A) as a lens on four running coding agents:
- Claude (Anthropic Messages API)
- code-agent (ChatGPT backend)
- Cursor (`aiserver.v1` gRPC-web)
- Antigravity (a Gemini-based coding agent)

**Figure 2 (per-agent opportunity).** One raw session per agent decomposed into task-core (work that remains after every intervention), Suppress (deterministic protocol-side), Compress (structural packet compression), and projected removable (hazard-gated Reuse, Steer, Guard under shadow-first discipline). Solid bands are observed or directly derived from the local telemetry corpus; hatched bands are projected opportunity, bounded by the offline replay ceiling of Appendix A.6.

| Agent | Raw tokens | Task-core | Suppress | Compress | Projected | Cumulative |
| --- | --- | --- | --- | --- | --- | --- |
| Claude | 302,950 | 11% | 60% | 16% | 13% | ~89% |
| code-agent | 1,891,748 | 46% | 20% | 18% | 16% | ~54% |
| Cursor | 334,840 | 1% | 20% | 20% | 59% (shadow) | ~99% |
| Antigravity | 133,357 | 66% | 6% | 16% | 12% | ~34% |

Three observations follow:
1. The shape is **agent-specific**: a single runtime setting cannot be tuned once and shipped.
2. The shape is **carrier-local**: almost all of Cursor's waste rides one carrier (analytics chatter), almost all of code-agent's rides another (plugin catalog plus retry storms), and almost all of Antigravity's rides a third (opening-turn environment-context replay).
3. The projected band is smallest where the task lane is cleanest (Antigravity at 12%, code-agent at 16%) and largest where the task lane is hidden behind control-plane noise (Cursor at 59% under shadow caveat).

### 10.4 Additional MSS-Supporting Attack Modes

[Proposed doctrine]

Four concrete attack modes surface directly from the carrier decomposition, each aligned with an existing MSS surface:

1. **Content-addressed request coalescing and catalog stubbing.** Both Claude's platform-bootstrap tax and code-agent's plugin-catalog tax are idempotent reads agents re-issue across every turn. A mediation layer that canonicalizes requests, hashes them, and serves TTL-safe cached responses removes the carrier deterministically. Maps to the canonical command rule (§6) extended from local commands to outbound tool calls.

2. **Decisive-fact packets for tool output.** code-agent's visible 74k→90k task-lane growth across one session is re-replayed tool output. A typed packet contract that preserves exit code, failing test, primary span, advisory ID, fuzz seed, raw-log hash, and tool version while discarding surrounding prose is ProofLens evaluated at the wire layer.

3. **Content-addressed delta compression for repeated context.** Antigravity's environment-context dump (a 7,400-character repo-tree reattached to every subsequent turn) is a concrete instance of repeated-context compounding. A content hash plus diff rewrite collapses it. Maps to the canonical-router rule for AGENTS.md.

4. **Shadow-mode hazard gate over runtime-safe features only.** The offline ISSUE-05 replay result of 18 of 34 invalid runs prevented (Appendix A.6) is the ceiling for a hazard-gated Steer, not a deployment claim. A feature manifest that marks each signal as `runtime_safe`, `hindsight_only`, or `oracle_only` and a scorer that dispatches only when the top non-continue action exceeds continue plus a tuned `abstain_margin` is what lets Steer ship shadow-first. Maps to the security gate rule (§8.6).

### 10.5 CrateAtlas as Substrate

[Project claim]

CrateAtlas is substrate, not a sixth stage. A content-addressed repository graph with owner, dependency, reference, and test edges can feed owner ranking, dependency and reference slicing, top-K path ranking for shell-packet summaries, symbol or path existence checks, and repair-capsule or ignore-certificate generation.

The flagship trio aligns directly:
- `cargo-mss` is the compile-time substrate for runtime Steer and path-scope Guard.
- ProofLens is the packet contract behind Compress.
- `cargo-obligation-cache` is the conservative proof-reuse substrate behind Reuse.

---

## 11. Future Work

[Proposed doctrine + Project claim]

**Reference implementation note.** A local five-crate Rust control plane (`cargo-vrc`, `cargo-witness`, `witness-rt`, `cargo-aer`, `arc-bench`) shows that pieces of MSS can be compiled into concrete routing, witness, repair, and audit artifacts. The paper uses those crates only as mechanism-plausibility and artifact-shape inspiration, not as proof of the standard.

**Relation to the residual runtime layer.** Each of the three flagships is the compile-time substrate for one part of §10:
- `cargo-mss` is the substrate for Steer (slice certificates, ignore certificates).
- ProofLens is the substrate for Compress (proof-preserving packet contract).
- `cargo-obligation-cache` is the substrate for Reuse (content-addressed certificates).

### 11.1 cargo-mss: Semantic-Surface Compiler

The highest-value repo-local concept: it attacks wrong-owner discovery, broad reads, legal-edit ambiguity, proof-lane confusion, and reviewer reconstruction together. The missing leap is *negative context*: compile what can be safely ignored for this task until contradiction appears.

**Artifacts and pipeline.** Starting from a failing test, compiler diagnostic, panic span, issue text, diff, security finding, or schema delta, ingest Cargo metadata, rustdoc or rust-analyzer semantics, the test map, feature graph, generated-contract provenance, unsafe ledger, and dependency rationale; compute an obligation cone by backward-cause slicing, forward proof slicing, contract and risk expansion, and legal-edit filtering; then emit `repair_capsule.json`, `ignore_certificate.json`, and `edit_grammar.json`. Capsule or shadow-workspace materialization is a mode of `cargo-mss`, not a fourth flagship.

**Evaluation and falsification.** Token-to-owner, files-opened-before-owner, wrong-owner edits, illegal-touch rate, wrong-proof-lane rate, reviewer reconstruction cost, hidden-pass rate, security-gated-pass rate, SecureETTS. Fails if staleness makes widening constant, shadow workspaces hide dependencies, or reviewer trust or hidden/security-gated pass regresses.

### 11.2 ProofLens: Proof-Preserving Compression

Turns log compression into an auditable evidence contract. RTK shows that command-output filtering can be practically useful, while InsForge's project-reported semantic layer shows the complementary lesson that agents should consume structured execution state rather than parse raw logs when systems can expose it. ProofLens proposes a structured-state-first packet format that preserves decisive facts and raw-evidence recovery for compiler, test, security, deep-lane, and operational-state output.

**Packet contract.** Every packet preserves command, working directory, exit code, tool version, primary span or failing test, panic text when present, advisory IDs when present, fuzz seed when applicable, raw-log path, raw-log hash, redaction state, and truncation state.

**Evaluation and falsification.** Compare raw logs, generic compression, RTK-style filtering, structured-state packets, and ProofLens packets using compression ratio, decisive-fact recall, false-green rate, raw-escalation rate, token-to-first-red, token-to-first-green, hidden-pass rate, security-gated-pass rate, SecureETTS. Fails if any packet hides decisive facts or weakens auditability even while visible tokens fall.

### 11.3 cargo-obligation-cache: Content-Addressed Proof-Obligation Engine

Replaces generic file caching with obligation-aware invalidation. The trio composes: `cargo-mss` narrows search surface first, ProofLens narrows visible proof-output cost second, and `cargo-obligation-cache` removes repeated reruns once the right scope and right proof have already been found. Each proof lane decomposes into obligations keyed on public API, contract hash, feature set, target triple, tool versions, unsafe-ledger state, dependency set, and relevant test identities.

**Evaluation and falsification.** Repeated-verification-time reduction, repeated proof-output-token reduction, obligation-graph coverage, reviewer trust in skipped-lane explanations, false-green rate from stale receipts, SecureETTS on multi-patch tasks. Fails if stale receipts create false greens or make reviewer reconstruction harder than rerunning proof.

### 11.4 Additional Systems and Upstream Horizon

The strongest secondary repo-local concept remains AgentDiagnostic for typed repair packets and proof-carrying patch receipts. On the runtime side, further dedup, packetization, checkpointing, and guard mechanisms belong in the supplementary runtime catalog rather than as a second main-paper roadmap.

**Upstream horizon ideas:**
- `cargo-agentmode` — native `rustc --error-format=agent-json` and `cargo <verb> --output=agent-json`
- `#[agent(...)]` — proc-macro attribute attaching owner / proof-lane / contract-id / risk / invariant to source items, validated at compile time
- `cargo-trace-autopilot` — trace-driven semantic-surface refactoring suggestions

These are aspirational and require ecosystem adoption rather than local implementation.

---

## 12. Threats to Validity and Limitations

[Proposed doctrine]

- **Construct validity.** MSS, ETTS, SecureETTS, ARI, and "lawful repair program" are doctrinal constructs. Their exact operationalization can improve as public repair traces, proof-preserving tooling, and agent benchmarks mature.
- **Claim discipline.** Review triggers, code-shape budgets, and token policies are proposed defaults rather than universal laws. Project-reported tooling claims remain mechanism evidence, not validated outcome proof.
- **Scope.** This paper does not claim original empirical proof that MSS improves public benchmark outcomes. Authority comes from evidence synthesis, official tooling mechanisms, and falsifiable future-work interfaces.
- **Ecological validity.** Model behavior, token pricing, and harness defaults drift quickly. Benchmark contamination remains an active concern.
- **Measurement validity.** Provider token accounting differs in how cached, tool-input, and system-prompt tokens are priced or reported.
- **External validity.** Rust is not universal. Token minimization is not a valid optimization if it weakens hidden correctness, security-gated validation, auditability, or reviewer understanding.
- **Residual-runtime measurement debt.** The companion runtime discussion in §10 rests on a corpus narrower than the thesis itself. Three gaps remain: (i) single issue family (ISSUE-05 only); (ii) single-session captures (no session-to-session variance yet); (iii) label coverage debt (several Guard and Steer interventions still need shadow-mode labels).

---

## 13. Conclusion

[Proposed doctrine]

Agent-efficient Rust is boring, local, typed, executable, observable, security-gated, token-aware, and loud when wrong. The repository should answer owner, contract, proof, and failure questions before the model starts broad search, not after it burns tokens reconstructing architecture from ambient text. Rust is not uniquely easy for agents, but it is unusually good at turning bad edits into local, diagnostic failures and reviewable proof obligations, which makes it a strong substrate where mistake cost dominates first-edit speed.

Map owner, patch narrowly, prove cheaply, preserve raw evidence, and escalate only with contract and security proof. The main paper's strongest future bets are structural routing (`cargo-mss`), proof-preserving evidence shaping (`ProofLens`), and conservative repeated-proof elimination (`cargo-obligation-cache`). A bounded runtime mediation layer may still remove residual chatter, replay, and wrong-turn overhead in flight, but that companion layer stays downstream of the main contribution. The best Rust for agents exports the smallest lawful repair program while preserving correctness, security-gated validation, auditability, and human review.

---

## Appendix A: WarpOS — A Residual-Waste Lens on Agent Runtime

[Project claim]

This appendix is a reference for the WarpOS telemetry layer that grounds Figures 1 and 2 and motivates the additional attack modes in §10.4. WarpOS is a local research prototype presented as a lens on residual agent waste, not a productized system or a deployment claim.

### A.1 Architectural Stance

WarpOS sits between the agent client and the model provider as a transparent mediation plane. It is not a model wrapper, a prompt library, or a training harness. Responsibilities:
1. Capture wire-level agent–model traffic with TLS termination.
2. Capture OS-level process, file, and command telemetry for the same session.
3. Normalize heterogeneous provider shapes into a common schema via per-agent extraction rules.
4. Make the merged session timeline available to intervention services that can suppress, compress, reuse, steer, or guard the wire in flight.

Three stances:
- Every intervention must be reversible in principle (raw bodies preserved by SHA-256).
- Structural interventions ship before learned ones (build order: Suppress → Compress → Reuse → Steer → Guard, with hazard-gated steering shadow-first).
- The substrate is not the policy: the layer publishes what it sees; rewrite and gate policies are versioned artifacts.

### A.2 Network Topology and Dataflow

The agent client is configured with `HTTPS_PROXY` pointing at the local WarpOS proxy and with the WarpOS root certificate installed so TLS interception is transparent. The proxy parses provider-specific shapes through extraction rules and writes both a decoded event stream and a raw body store. An OS-level tap (eBPF-assisted on Linux, ptrace-assisted elsewhere) writes process, file, and command events into parallel NDJSON streams. Every stream is keyed by `session_id` and `turn_id`.

The architecture diagram (rendered in main.tex via `\input{warpos-diagram}`) shows three rows:
- Row 1 (network layer): agent client ↔ warpos-proxy ↔ model provider
- Row 2 (OS layer): user shell → warpos-tap + warpos-tls-tap → warpos-runtime-sentinel
- Row 3 (storage): session store consolidating decoded segments, OS events, sentinels
- Row 4 (mediation): warpos-extraction, warpos-compactor, warpos-semantic-checkpoint, warpos-wastegate, warpos-waste-model, warpos-invariant-guard
- Row 5 (cascade stages): Suppress, Compress, Reuse, Steer, Guard
- Intervention control loops back from mediation services into the proxy

### A.3 Data Families

The session store contains parallel NDJSON streams plus a content-addressed payload blob store:

| stream | contents |
| --- | --- |
| `http_transaction.ndjson` | normalized request/response pairs with decoded body and scrub-rule projection |
| `llm_call.ndjson` | per-turn model, tokens, scope, call class, prompt fingerprint, tool-schema hash |
| `llm_segment.ndjson` | typed segments per turn: `system_message`, `user_message`, `tool_schema`, `tool_call`, `tool_result`, `visible_reasoning`, `runtime_error_text` |
| `tls_event.ndjson` | TLS handshake observations for traffic outside MITM scope |
| `exception_event.ndjson` | runtime-sentinel tripwires |
| `command_execution.ndjson` | argv, cwd, exit code, stdout/stderr references paired with the model turn |
| `file_event.ndjson` | path, op, bytes, content hash before/after |
| `process_exec.ndjson` | pid lineage, parent PID, argv, image |
| `terminal_io.ndjson` | pty input/output events tied to interactive sessions |
| `payloads/req-*.bin`, `resp-*.bin` | raw request and response bodies keyed by SHA-256 |

### A.4 MITM Addon Pipeline

The reference `mitmproxy` addon attaches at four hook points: `request` (pre-dial), `response` (post-dial, pre-body), body-parse (for both request and response segments), and session-finalize. At each stage the addon reads one or more per-agent extraction rules and writes one or more NDJSON records plus, if applicable, a content-addressed payload blob.

Segmentation is the most consequential step: a single LLM response turn is decomposed into a typed sequence of segments. Each segment carries a `source` field recording where in the provider envelope it came from. This is what lets the Compress stage emit a typed packet in place of a `tool_result` without mutating the enclosing turn envelope, and what lets the Guard stage ask whether a claim of "tests passed" is supported by a `command_execution` row with `exit_code==0` in the same turn.

Normalization handles content encoding (gzip, brotli, zstd) and SSE framing; payloads are decoded before hashing so two semantically identical responses with different compression produce the same `payload_hash`.

### A.5 Extraction Rules and Provider Normalization

Per-agent `extraction_rules.json` files (schema version 2) are the dispatch table that makes the rest of the stack provider-neutral. Seven rule groups:
- Provider rules (host/path/user-agent → provider family)
- Scope rules (path → scope class)
- Call-class rules (call class per path)
- Usage rules (where to read token counts)
- Model and feature rules (normalizing identity)
- Payload projection rules (what sub-objects to lift into `llm_segment`)
- Scrub rules (request-scoped fields to strip before fingerprinting)

These rules are why the same five cascade stages present uniformly across Claude (Anthropic Messages), code-agent (ChatGPT backend), Cursor (`aiserver.v1` gRPC-web), and Antigravity (Gemini `/v1internal`).

### A.6 Hazard Model and Intervention Dispatch

The learned part of the stack materializes state/action rows with ~219 features. The feature manifest marks each feature as `runtime_safe`, `hindsight_only`, or `oracle_only`; only runtime-safe features may be consumed online. The training script fits five gradient-boosted hazard heads plus one utility head.

**Five PRIMARY labels:**
- next-step illegal touch
- next-three-steps requires widen lease
- current state should switch to patch-proposal mode
- current run needs hard-freeze of boundary writes
- next-five-steps validation blowup

**Canonical action space (seven tokens):**
- `continue` (no intervention)
- `trap_now`
- `force_lease_now`
- `hard_freeze_boundary_writes`
- `switch_to_patch_proposal_now`
- `suppress_broad_validation_now`
- `reveal_next_probe_now`

At runtime, the hazard service extracts the runtime-safe feature subset from the live session, scores the five heads, computes adjusted utility per action, and dispatches only when the top non-`continue` action exceeds `continue` plus an `abstain_margin` (default 50). Below that margin, the row is logged but no action fires (shadow-mode discipline).

The current offline replay result is **18 of 34 invalid runs prevented on ISSUE-05**, with **1,242,298 optimistic tokens saved**. That number is the ceiling that scales by `abstain_margin` for online projections throughout §10. It is not a deployment outcome; generalization to other issue families requires rebuilding the dataset from those runsets.

### A.7 Storage, Retention, and Replay

Session directories are append-only: NDJSON streams never mutate in place, and payload blobs are write-once and deduplicated by content hash. A session manifest is written at teardown recording session identity, policy hashes, and stream counts; replaying a past session under a different policy produces a different manifest hash and can never be silently confused with the original. Retention is operator-controlled and defaults to session-scope. Cross-session deduplication is intrinsically content-addressed and therefore privacy-bounded to what the hash already reveals.

### A.8 Safety, Privacy, and Limitations

WarpOS captures traffic and OS events that can contain credentials, tokens, customer data, and source code. Scrub rules strip obvious secrets at extraction time, but the content-addressed payload store holds raw bodies by design and must be managed with the same care as any other credential-adjacent artifact. The mediation plane runs with the user's local permissions; setups that share a workstation across users should run the plane per-user rather than as a system service, and a hardened deployment should run it inside a network namespace with an explicit egress policy.

**Three limitations bound every runtime claim in this paper:**
1. **Single-session corpus.** Figure 2 uses one session per agent, so session-to-session variance is not yet measured.
2. **Single issue family.** Hazard-gated steering is grounded in ISSUE-05 offline replay only; generalization to ISSUE-01, ISSUE-03, and ISSUE-04 has not been rerun publicly, and ISSUE-06 through ISSUE-10 remain blueprint.
3. **Label coverage debt.** Several Guard and Steer interventions still require shadow-mode labels before they can ship.

Claims labeled "projected" are bounded by these three gaps and should not be read past them.

---

*End of detailed Markdown companion. The canonical TeX source is `main.tex`; the architecture diagram source is `warpos-diagram.tex` (compilable on its own via `warpos-diagram-standalone.tex`); the supplementary artifacts in `paper/artifacts/` carry the engineering catalogs and worked examples that do not fit the main manuscript.*

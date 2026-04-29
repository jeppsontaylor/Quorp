//! Staged verification ladder L0..L4 with content-addressed result cache.
//!
//! Phase 7 ships the API surface and a minimal blake3-style cache key
//! computation so downstream crates can target the type. The cargo /
//! rustfmt / miri executors land in the runtime wire-up phase.

use quorp_ids::VerifyRunId;
pub use quorp_verify_model::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Compose the canonical cache-key string for hashing.
pub fn cache_key_canonical_string(key: &CacheKey) -> String {
    format!(
        "{git_sha}\0{changed_files_hash}\0{features}\0{target_triple}\0{rustc_version}\0{stage_id}",
        git_sha = key.git_sha,
        changed_files_hash = key.changed_files_hash,
        features = key.features.join(","),
        target_triple = key.target_triple,
        rustc_version = key.rustc_version,
        stage_id = key.stage_id,
    )
}

#[derive(Debug, Clone, Default)]
pub struct VerifierStats {
    pub completed: u32,
    pub cached: u32,
    pub failed: u32,
}

/// Trait implemented by per-stage executors. The `quorp_session` crate
/// supplies the cargo / rustfmt / miri / fuzz runners during wire-up.
pub trait StageExecutor: Send + Sync {
    fn stage_id(&self) -> &str;
    fn level(&self) -> VerifyLevel;
}

/// Plan a run id for a synthetic verify request. Stable enough for tests.
pub fn fresh_run_id() -> VerifyRunId {
    VerifyRunId::new(format!(
        "verify-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}

#[derive(Debug, Clone)]
pub struct CommandOutputEvidence<'a> {
    pub command: &'a str,
    pub cwd: &'a Path,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub output: &'a str,
    pub raw_log_path: PathBuf,
    pub tool_version: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct VerifyCommand {
    pub stage_id: String,
    pub command: String,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone)]
pub struct VerifyCommandResult {
    pub exit_code: i32,
    pub duration_ms: u64,
    pub output: String,
    pub raw_log_path: PathBuf,
    pub tool_version: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct VerifyRequest {
    pub plan: VerifyPlan,
    pub commands: Vec<VerifyCommand>,
    pub git_sha: String,
    pub changed_files_hash: String,
    pub features: Vec<String>,
    pub target_triple: String,
    pub rustc_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyCacheEntry {
    pub report: StageReport,
    pub packet: ProofPacket,
}

pub trait VerifyCache {
    fn get(&self, key: &CacheKey) -> Result<Option<VerifyCacheEntry>, String>;
    fn put(&self, key: &CacheKey, entry: &VerifyCacheEntry) -> Result<(), String>;
}

#[derive(Debug, Default)]
pub struct MemoryVerifyCache {
    entries: Mutex<HashMap<String, VerifyCacheEntry>>,
}

#[derive(Debug, Clone)]
pub struct FileVerifyCache {
    store: VerifyStore,
}

#[derive(Debug, Clone)]
pub struct VerifyStore {
    workspace_root: PathBuf,
}

impl VerifyStore {
    pub fn for_workspace(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    pub fn root(&self) -> PathBuf {
        self.workspace_root.join(".quorp").join("verify")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root().join("cache")
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.root().join("runs")
    }

    pub fn cache_path(&self, key: &CacheKey) -> PathBuf {
        self.cache_dir()
            .join(format!("{}.json", cache_key_hash(key)))
    }

    pub fn run_dir(&self, run_id: &VerifyRunId) -> PathBuf {
        self.runs_dir().join(run_id.as_str())
    }

    pub fn raw_log_path(&self, run_id: &VerifyRunId, stage_id: &str) -> PathBuf {
        self.run_dir(run_id)
            .join("raw")
            .join(format!("{}.log", sanitize_component(stage_id)))
    }

    pub fn proof_dag_path(&self, run_id: &VerifyRunId) -> PathBuf {
        self.run_dir(run_id).join("proof-dag.json")
    }

    pub fn write_proof_dag(&self, dag: &ProofDag) -> Result<PathBuf, String> {
        let path = self.proof_dag_path(&dag.run_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(dag).map_err(|error| error.to_string())?;
        fs::write(&path, bytes).map_err(|error| error.to_string())?;
        Ok(path)
    }
}

impl FileVerifyCache {
    pub fn new(store: VerifyStore) -> Self {
        Self { store }
    }
}

impl VerifyCache for MemoryVerifyCache {
    fn get(&self, key: &CacheKey) -> Result<Option<VerifyCacheEntry>, String> {
        self.entries
            .lock()
            .map_err(|_| "verify cache lock poisoned".to_string())
            .map(|entries| entries.get(&cache_key_canonical_string(key)).cloned())
    }

    fn put(&self, key: &CacheKey, entry: &VerifyCacheEntry) -> Result<(), String> {
        self.entries
            .lock()
            .map_err(|_| "verify cache lock poisoned".to_string())?
            .insert(cache_key_canonical_string(key), entry.clone());
        Ok(())
    }
}

impl VerifyCache for FileVerifyCache {
    fn get(&self, key: &CacheKey) -> Result<Option<VerifyCacheEntry>, String> {
        let path = self.store.cache_path(key);
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))
    }

    fn put(&self, key: &CacheKey, entry: &VerifyCacheEntry) -> Result<(), String> {
        let path = self.store.cache_path(key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(entry).map_err(|error| error.to_string())?;
        fs::write(&path, bytes)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))
    }
}

pub fn cache_key_hash(key: &CacheKey) -> String {
    sha256_hex(cache_key_canonical_string(key).as_bytes())
}

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

mod failure_parser;
pub use failure_parser::*;

pub fn proof_packet_from_command(input: CommandOutputEvidence<'_>) -> ProofPacket {
    let diagnostics = parse_cargo_json_diagnostics(input.output, 32);
    let failing_tests = parse_test_failures(input.output, 16);
    let security_findings = parse_security_findings(input.output, 16);
    let kind = classify_packet_kind(
        input.command,
        &diagnostics,
        &failing_tests,
        &security_findings,
    );
    let summary = packet_summary(
        input.exit_code,
        &diagnostics,
        &failing_tests,
        &security_findings,
    );
    ProofPacket {
        kind,
        command: CommandEvidence {
            command: input.command.to_string(),
            cwd: input.cwd.to_path_buf(),
            exit_code: input.exit_code,
            duration_ms: input.duration_ms,
            tool_version: input.tool_version,
        },
        summary,
        diagnostics,
        failing_tests,
        security_findings,
        raw_log_ref: ArtifactRef {
            path: input.raw_log_path,
            sha256: sha256_hex(input.output.as_bytes()),
        },
        redacted: false,
        truncated: input.truncated,
    }
}

pub fn stage_report_from_packet(packet: &ProofPacket, cache_key: CacheKey) -> StageReport {
    StageReport {
        stage_id: packet.command.command.clone(),
        status: if packet.command.exit_code == 0 {
            StageStatus::Pass
        } else {
            StageStatus::Fail
        },
        duration_ms: packet.command.duration_ms,
        summary: packet.summary.clone(),
        failures: packet_failures(packet),
        raw_log_ref: Some(packet.raw_log_ref.clone()),
        cache_key,
        from_cache: false,
    }
}

fn cache_key_for_request(request: &VerifyRequest, command: &VerifyCommand) -> CacheKey {
    CacheKey {
        git_sha: request.git_sha.clone(),
        changed_files_hash: request.changed_files_hash.clone(),
        features: request.features.clone(),
        target_triple: request.target_triple.clone(),
        rustc_version: request.rustc_version.clone(),
        stage_id: command.stage_id.clone(),
    }
}

fn report_overall(stages: &[StageReport]) -> Verdict {
    if stages.is_empty() {
        Verdict::Cancelled
    } else if stages.iter().all(|stage| stage.status == StageStatus::Pass) {
        Verdict::Pass
    } else if stages.iter().any(|stage| stage.status == StageStatus::Fail) {
        Verdict::Fail
    } else {
        Verdict::Partial
    }
}

pub fn execute_verify_request_with_cache<C, F>(
    request: &VerifyRequest,
    cache: &C,
    mut run_command: F,
) -> Result<VerifyReport, String>
where
    C: VerifyCache,
    F: FnMut(&VerifyCommand) -> Result<VerifyCommandResult, String>,
{
    let started_at = Instant::now();
    let mut stages = Vec::with_capacity(request.commands.len());
    let mut proof_packets = Vec::with_capacity(request.commands.len());
    let mut cache_hits = 0_u32;

    for command in &request.commands {
        let cache_key = cache_key_for_request(request, command);

        if let Some(cached) = cache.get(&cache_key)? {
            let mut report = cached.report;
            report.from_cache = true;
            report.duration_ms = 0;
            cache_hits += 1;
            stages.push(report);
            proof_packets.push(cached.packet);
            if request.plan.fail_fast
                && matches!(
                    stages.last().map(|stage| &stage.status),
                    Some(StageStatus::Fail)
                )
            {
                break;
            }
            continue;
        }

        let result = run_command(command)?;
        let packet = proof_packet_from_command(CommandOutputEvidence {
            command: &command.command,
            cwd: &command.cwd,
            exit_code: result.exit_code,
            duration_ms: result.duration_ms,
            output: &result.output,
            raw_log_path: result.raw_log_path.clone(),
            tool_version: result.tool_version.clone(),
            truncated: result.truncated,
        });
        let mut report = stage_report_from_packet(&packet, cache_key);
        report.stage_id = command.stage_id.clone();
        cache.put(
            &report.cache_key,
            &VerifyCacheEntry {
                report: report.clone(),
                packet: packet.clone(),
            },
        )?;
        let stage_failed = report.status == StageStatus::Fail;
        stages.push(report);
        proof_packets.push(packet);
        if request.plan.fail_fast && stage_failed {
            break;
        }
    }

    let overall = report_overall(&stages);
    let wall_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;

    Ok(VerifyReport {
        plan: request.plan.clone(),
        stages,
        proof_packets,
        overall,
        cache_hits,
        wall_ms,
    })
}

pub fn execute_verify_request<F>(
    request: &VerifyRequest,
    run_command: F,
) -> Result<VerifyReport, String>
where
    F: FnMut(&VerifyCommand) -> Result<VerifyCommandResult, String>,
{
    let cache = MemoryVerifyCache::default();
    execute_verify_request_with_cache(request, &cache, run_command)
}

pub fn execute_verify_request_durable<F>(
    store: &VerifyStore,
    request: &VerifyRequest,
    provenance: serde_json::Value,
    mut run_command: F,
) -> Result<VerifyReport, String>
where
    F: FnMut(&VerifyCommand) -> Result<VerifyCommandResult, String>,
{
    let started_at = Instant::now();
    let cache = FileVerifyCache::new(store.clone());
    let mut stages = Vec::with_capacity(request.commands.len());
    let mut proof_packets = Vec::with_capacity(request.commands.len());
    let mut nodes = Vec::with_capacity(request.commands.len());
    let mut edges = Vec::new();
    let mut cache_hits = 0_u32;
    let mut previous_node_id: Option<String> = None;

    for command in &request.commands {
        let cache_key = cache_key_for_request(request, command);
        let node_id = format!(
            "{}-{}",
            nodes.len() + 1,
            sanitize_component(&command.stage_id)
        );

        if let Some(cached) = cache.get(&cache_key)? {
            let mut report = cached.report;
            report.from_cache = true;
            report.duration_ms = 0;
            cache_hits += 1;
            let artifacts = report
                .raw_log_ref
                .iter()
                .map(|artifact| ProofArtifactRef {
                    role: "raw_log".to_string(),
                    path: artifact.path.clone(),
                    sha256: artifact.sha256.clone(),
                })
                .collect::<Vec<_>>();
            let node = ProofNode {
                id: node_id.clone(),
                stage_id: command.stage_id.clone(),
                status: ProofNodeStatus::Cached,
                summary: report.summary.clone(),
                artifacts,
                cache_key: Some(cache_key),
                from_cache: true,
                packet: Some(cached.packet.clone()),
                report: Some(report.clone()),
            };
            if let Some(previous) = previous_node_id.replace(node_id.clone()) {
                edges.push(ProofEdge {
                    from: previous,
                    to: node_id,
                    label: Some("then".to_string()),
                });
            }
            stages.push(report);
            proof_packets.push(cached.packet);
            nodes.push(node);
            if request.plan.fail_fast
                && matches!(
                    stages.last().map(|stage| &stage.status),
                    Some(StageStatus::Fail)
                )
            {
                break;
            }
            continue;
        }

        let result = run_command(command)?;
        persist_raw_log(&result.raw_log_path, &result.output)?;
        let packet = proof_packet_from_command(CommandOutputEvidence {
            command: &command.command,
            cwd: &command.cwd,
            exit_code: result.exit_code,
            duration_ms: result.duration_ms,
            output: &result.output,
            raw_log_path: result.raw_log_path.clone(),
            tool_version: result.tool_version.clone(),
            truncated: result.truncated,
        });
        let mut report = stage_report_from_packet(&packet, cache_key);
        report.stage_id = command.stage_id.clone();
        cache.put(
            &report.cache_key,
            &VerifyCacheEntry {
                report: report.clone(),
                packet: packet.clone(),
            },
        )?;
        let artifacts = vec![ProofArtifactRef {
            role: "raw_log".to_string(),
            path: packet.raw_log_ref.path.clone(),
            sha256: packet.raw_log_ref.sha256.clone(),
        }];
        let node = ProofNode {
            id: node_id.clone(),
            stage_id: command.stage_id.clone(),
            status: proof_status_from_stage_status(report.status),
            summary: report.summary.clone(),
            artifacts,
            cache_key: Some(report.cache_key.clone()),
            from_cache: false,
            packet: Some(packet.clone()),
            report: Some(report.clone()),
        };
        if let Some(previous) = previous_node_id.replace(node_id.clone()) {
            edges.push(ProofEdge {
                from: previous,
                to: node_id,
                label: Some("then".to_string()),
            });
        }
        let stage_failed = report.status == StageStatus::Fail;
        stages.push(report);
        proof_packets.push(packet);
        nodes.push(node);
        if request.plan.fail_fast && stage_failed {
            break;
        }
    }

    let overall = report_overall(&stages);
    let wall_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let dag = ProofDag {
        run_id: request.plan.run_id.clone(),
        provenance,
        nodes,
        edges,
    };
    store.write_proof_dag(&dag)?;

    Ok(VerifyReport {
        plan: request.plan.clone(),
        stages,
        proof_packets,
        overall,
        cache_hits,
        wall_ms,
    })
}

fn persist_raw_log(path: &Path, output: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, output.as_bytes())
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn proof_status_from_stage_status(status: StageStatus) -> ProofNodeStatus {
    match status {
        StageStatus::Pass => ProofNodeStatus::Pass,
        StageStatus::Fail => ProofNodeStatus::Fail,
        StageStatus::Skipped => ProofNodeStatus::Skipped,
        StageStatus::Cancelled => ProofNodeStatus::Cancelled,
    }
}

pub fn verify_proof_dag_artifacts(dag: &ProofDag) -> Result<(), String> {
    for node in &dag.nodes {
        for artifact in &node.artifacts {
            let bytes = fs::read(&artifact.path)
                .map_err(|error| format!("failed to read {}: {error}", artifact.path.display()))?;
            let actual = sha256_hex(&bytes);
            if actual != artifact.sha256 {
                return Err(format!(
                    "artifact hash mismatch for {}: expected {}, got {}",
                    artifact.path.display(),
                    artifact.sha256,
                    actual
                ));
            }
        }
    }
    Ok(())
}

pub fn default_verify_plan(level: VerifyLevel, targets: Vec<VerifyTarget>) -> VerifyPlan {
    VerifyPlan {
        run_id: fresh_run_id(),
        level,
        targets,
        time_budget: Duration::from_secs(180),
        fail_fast: false,
    }
}

pub fn parse_cargo_json_diagnostics(output: &str, limit: usize) -> Vec<CargoDiagnostic> {
    let mut diagnostics = Vec::new();
    for line in output.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let primary_span = message
            .get("spans")
            .and_then(serde_json::Value::as_array)
            .and_then(|spans| {
                spans.iter().find(|span| {
                    span.get("is_primary").and_then(serde_json::Value::as_bool) == Some(true)
                })
            })
            .and_then(|span| {
                let line = u32::try_from(span.get("line_start")?.as_u64()?).ok()?;
                let column = u32::try_from(span.get("column_start")?.as_u64()?).ok()?;
                Some(DiagnosticSpan {
                    file: PathBuf::from(span.get("file_name")?.as_str()?),
                    line,
                    column,
                })
            });
        diagnostics.push(CargoDiagnostic {
            level: message
                .get("level")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("diagnostic")
                .to_string(),
            code: message
                .get("code")
                .and_then(|code| code.get("code"))
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            message: message
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            primary_span,
        });
        if diagnostics.len() >= limit {
            break;
        }
    }
    diagnostics
}

pub fn parse_test_failures(output: &str, limit: usize) -> Vec<TestFailure> {
    let mut failures = Vec::new();
    let mut pending_name: Option<String> = None;
    for line in output.lines() {
        if let Some(name) = line
            .strip_prefix("test ")
            .and_then(|rest| rest.split(" ... FAILED").next())
            .filter(|name| !name.is_empty() && line.contains(" ... FAILED"))
        {
            failures.push(TestFailure {
                name: name.to_string(),
                panic: None,
            });
            if failures.len() >= limit {
                break;
            }
            continue;
        }
        if line.starts_with("---- ") && line.ends_with(" stdout ----") {
            pending_name = line
                .strip_prefix("---- ")
                .and_then(|rest| rest.strip_suffix(" stdout ----"))
                .map(ToOwned::to_owned);
            continue;
        }
        if let Some(panic) = line.trim().strip_prefix("thread '")
            && let Some(name) = pending_name.take()
        {
            failures.push(TestFailure {
                name,
                panic: Some(format!("thread '{panic}")),
            });
            if failures.len() >= limit {
                break;
            }
        }
    }
    failures
}

pub fn parse_security_findings(output: &str, limit: usize) -> Vec<SecurityFinding> {
    let mut findings = Vec::new();
    for line in output.lines() {
        let lower = line.to_ascii_lowercase();
        if !(lower.contains("rustsec")
            || lower.contains("advisory")
            || lower.contains("vulnerability")
            || lower.contains("secret"))
        {
            continue;
        }
        findings.push(SecurityFinding {
            advisory_id: extract_advisory_id(line),
            severity: extract_after_label(line, "severity"),
            package: extract_after_label(line, "package"),
            path: extract_pathish(line).map(PathBuf::from),
            message: truncate(line.trim(), 300),
        });
        if findings.len() >= limit {
            break;
        }
    }
    findings
}

fn classify_packet_kind(
    command: &str,
    diagnostics: &[CargoDiagnostic],
    failing_tests: &[TestFailure],
    security_findings: &[SecurityFinding],
) -> ProofPacketKind {
    let command = command.to_ascii_lowercase();
    if !security_findings.is_empty()
        || command.contains("cargo audit")
        || command.contains("cargo deny")
        || command.contains("gitleaks")
    {
        ProofPacketKind::Security
    } else if !diagnostics.is_empty()
        || command.contains("cargo check")
        || command.contains("clippy")
    {
        ProofPacketKind::Compiler
    } else if !failing_tests.is_empty()
        || command.contains("cargo test")
        || command.contains("nextest")
    {
        ProofPacketKind::Test
    } else {
        ProofPacketKind::Command
    }
}

fn packet_summary(
    exit_code: i32,
    diagnostics: &[CargoDiagnostic],
    failing_tests: &[TestFailure],
    security_findings: &[SecurityFinding],
) -> String {
    let mut parts = vec![format!("exit_code={exit_code}")];
    if !diagnostics.is_empty() {
        parts.push(format!("diagnostics={}", diagnostics.len()));
        if let Some(first) = diagnostics.first() {
            parts.push(format!(
                "first={}{}",
                first
                    .code
                    .as_deref()
                    .map(|code| format!("{code}:"))
                    .unwrap_or_default(),
                truncate(&first.message, 120)
            ));
        }
    }
    if !failing_tests.is_empty() {
        parts.push(format!("failing_tests={}", failing_tests.len()));
        if let Some(first) = failing_tests.first() {
            parts.push(format!("first_test={}", first.name));
        }
    }
    if !security_findings.is_empty() {
        parts.push(format!("security_findings={}", security_findings.len()));
        if let Some(first) = security_findings.first()
            && let Some(advisory_id) = &first.advisory_id
        {
            parts.push(format!("first_advisory={advisory_id}"));
        }
    }
    parts.join(" ")
}

fn packet_failures(packet: &ProofPacket) -> Vec<Failure> {
    let mut failures = packet
        .diagnostics
        .iter()
        .map(|diagnostic| Failure {
            code: diagnostic.code.clone(),
            message: diagnostic.message.clone(),
            level: diagnostic.level.clone(),
            file: diagnostic
                .primary_span
                .as_ref()
                .map(|span| span.file.clone()),
            line: diagnostic.primary_span.as_ref().map(|span| span.line),
        })
        .collect::<Vec<_>>();
    failures.extend(packet.failing_tests.iter().map(|test| {
        Failure {
            code: None,
            message: test
                .panic
                .clone()
                .unwrap_or_else(|| "test failed".to_string()),
            level: "test".to_string(),
            file: None,
            line: None,
        }
    }));
    failures.extend(packet.security_findings.iter().map(|finding| {
        Failure {
            code: finding.advisory_id.clone(),
            message: finding.message.clone(),
            level: finding
                .severity
                .clone()
                .unwrap_or_else(|| "security".to_string()),
            file: finding.path.clone(),
            line: None,
        }
    }));
    failures
}

fn extract_advisory_id(line: &str) -> Option<String> {
    line.split(|character: char| !character.is_ascii_alphanumeric() && character != '-')
        .find(|part| part.starts_with("RUSTSEC-") || part.starts_with("GHSA-"))
        .map(ToOwned::to_owned)
}

fn extract_after_label(line: &str, label: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let index = lower.find(label)?;
    let rest = line
        .get(index + label.len()..)?
        .trim_start_matches([' ', ':', '=']);
    rest.split([',', ';'])
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| truncate(value, 80))
}

fn extract_pathish(line: &str) -> Option<&str> {
    line.split_whitespace()
        .find(|part| part.contains('/') && (part.contains(".rs") || part.contains(".toml")))
        .map(|part| part.trim_matches(|character| matches!(character, ',' | ';' | ':' | '"')))
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push_str("...");
    out
}
#[cfg(test)]
#[path = "../../../testing/quorp_verify/quorp_verify/tests.rs"]
mod tests;

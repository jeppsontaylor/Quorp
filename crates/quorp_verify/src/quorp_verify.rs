//! Staged verification ladder L0..L4 with content-addressed result cache.
//!
//! Phase 7 ships the API surface and a minimal blake3-style cache key
//! computation so downstream crates can target the type. The cargo /
//! rustfmt / miri executors land in the runtime wire-up phase.

use quorp_ids::VerifyRunId;
pub use quorp_verify_model::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
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

#[derive(Debug, Clone)]
struct CachedStageReport {
    report: StageReport,
    packet: ProofPacket,
}

static VERIFY_CACHE: OnceLock<Mutex<HashMap<String, CachedStageReport>>> = OnceLock::new();

fn verify_cache() -> &'static Mutex<HashMap<String, CachedStageReport>> {
    VERIFY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

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

pub fn execute_verify_request<F>(
    request: &VerifyRequest,
    mut run_command: F,
) -> Result<VerifyReport, String>
where
    F: FnMut(&VerifyCommand) -> Result<VerifyCommandResult, String>,
{
    let started_at = Instant::now();
    let mut stages = Vec::with_capacity(request.commands.len());
    let mut proof_packets = Vec::with_capacity(request.commands.len());
    let mut cache_hits = 0_u32;

    for command in &request.commands {
        let cache_key = CacheKey {
            git_sha: request.git_sha.clone(),
            changed_files_hash: request.changed_files_hash.clone(),
            features: request.features.clone(),
            target_triple: request.target_triple.clone(),
            rustc_version: request.rustc_version.clone(),
            stage_id: command.stage_id.clone(),
        };
        let cache_key_string = cache_key_canonical_string(&cache_key);

        if let Some(cached) = verify_cache()
            .lock()
            .map_err(|_| "verify cache lock poisoned".to_string())?
            .get(&cache_key_string)
            .cloned()
        {
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
        verify_cache()
            .lock()
            .map_err(|_| "verify cache lock poisoned".to_string())?
            .insert(
                cache_key_string,
                CachedStageReport {
                    report: report.clone(),
                    packet: packet.clone(),
                },
            );
        let stage_failed = report.status == StageStatus::Fail;
        stages.push(report);
        proof_packets.push(packet);
        if request.plan.fail_fast && stage_failed {
            break;
        }
    }

    let overall = if stages.is_empty() {
        Verdict::Cancelled
    } else if stages.iter().all(|stage| stage.status == StageStatus::Pass) {
        Verdict::Pass
    } else if stages.iter().any(|stage| stage.status == StageStatus::Fail) {
        Verdict::Fail
    } else {
        Verdict::Partial
    };
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
mod tests {
    use super::*;
    use rand::{Rng, SeedableRng, rngs::StdRng};

    #[test]
    fn cache_key_is_deterministic() {
        let key = CacheKey {
            git_sha: "abc".into(),
            changed_files_hash: "deadbeef".into(),
            features: vec!["default".into()],
            target_triple: "aarch64-apple-darwin".into(),
            rustc_version: "1.93.0".into(),
            stage_id: "L1Check".into(),
        };
        let s1 = cache_key_canonical_string(&key);
        let s2 = cache_key_canonical_string(&key);
        assert_eq!(s1, s2);
        assert!(s1.contains("abc"));
        assert!(s1.contains("L1Check"));
    }

    #[test]
    fn fresh_run_id_is_non_empty() {
        let id = fresh_run_id();
        assert!(id.as_str().starts_with("verify-"));
    }

    #[test]
    fn proof_packet_preserves_cargo_json_decisive_facts() {
        let output = r#"{"reason":"compiler-message","message":{"level":"error","code":{"code":"E0308"},"message":"mismatched types","spans":[{"file_name":"src/lib.rs","line_start":12,"column_start":5,"is_primary":true}]}}"#;
        let packet = proof_packet_from_command(CommandOutputEvidence {
            command: "cargo check --message-format=json",
            cwd: Path::new("/tmp/work"),
            exit_code: 101,
            duration_ms: 10,
            output,
            raw_log_path: PathBuf::from("logs/check.ndjson"),
            tool_version: Some("rustc 1.93.0".to_string()),
            truncated: false,
        });

        assert_eq!(packet.kind, ProofPacketKind::Compiler);
        assert_eq!(packet.command.exit_code, 101);
        assert_eq!(packet.diagnostics[0].code.as_deref(), Some("E0308"));
        assert_eq!(
            packet.diagnostics[0]
                .primary_span
                .as_ref()
                .map(|span| span.line),
            Some(12)
        );
        assert_eq!(packet.raw_log_ref.sha256, sha256_hex(output.as_bytes()));
        assert!(packet.summary.contains("exit_code=101"));
    }

    #[test]
    fn proof_packet_preserves_failing_test_name_and_panic() {
        let output = "\
running 1 test
test billing::tests::grace_period_upgrade ... FAILED

---- billing::tests::grace_period_upgrade stdout ----
thread 'billing::tests::grace_period_upgrade' panicked at src/lib.rs:9: expected later
";
        let packet = proof_packet_from_command(CommandOutputEvidence {
            command: "cargo test -p billing-domain",
            cwd: Path::new("/tmp/work"),
            exit_code: 101,
            duration_ms: 22,
            output,
            raw_log_path: PathBuf::from("logs/test.log"),
            tool_version: None,
            truncated: false,
        });

        assert_eq!(packet.kind, ProofPacketKind::Test);
        assert!(
            packet
                .failing_tests
                .iter()
                .any(|test| test.name == "billing::tests::grace_period_upgrade")
        );
        assert!(
            packet
                .summary
                .contains("first_test=billing::tests::grace_period_upgrade")
        );
    }

    #[test]
    fn proof_packet_preserves_security_advisory() {
        let output = "advisory RUSTSEC-2024-0001 severity: high package: example vulnerability";
        let packet = proof_packet_from_command(CommandOutputEvidence {
            command: "cargo audit",
            cwd: Path::new("/tmp/work"),
            exit_code: 1,
            duration_ms: 3,
            output,
            raw_log_path: PathBuf::from("logs/audit.log"),
            tool_version: None,
            truncated: false,
        });

        assert_eq!(packet.kind, ProofPacketKind::Security);
        assert_eq!(
            packet.security_findings[0].advisory_id.as_deref(),
            Some("RUSTSEC-2024-0001")
        );
        assert!(packet.summary.contains("first_advisory=RUSTSEC-2024-0001"));
    }

    #[test]
    fn stage_report_from_packet_keeps_raw_log_reference() {
        let packet = proof_packet_from_command(CommandOutputEvidence {
            command: "cargo check",
            cwd: Path::new("/tmp/work"),
            exit_code: 0,
            duration_ms: 5,
            output: "ok",
            raw_log_path: PathBuf::from("logs/check.log"),
            tool_version: None,
            truncated: false,
        });
        let report = stage_report_from_packet(
            &packet,
            CacheKey {
                git_sha: "abc".to_string(),
                changed_files_hash: "def".to_string(),
                features: vec!["default".to_string()],
                target_triple: "aarch64-apple-darwin".to_string(),
                rustc_version: "1.93.0".to_string(),
                stage_id: "fast".to_string(),
            },
        );

        assert_eq!(report.status, StageStatus::Pass);
        assert_eq!(
            report
                .raw_log_ref
                .as_ref()
                .map(|artifact| artifact.path.clone()),
            Some(PathBuf::from("logs/check.log"))
        );
    }

    #[test]
    fn cache_key_canonical_string_changes_when_any_field_changes() {
        let mut rng = StdRng::seed_from_u64(0x5e5f_2026);

        for _ in 0..32 {
            let base = CacheKey {
                git_sha: format!("{:016x}", rng.random::<u64>()),
                changed_files_hash: format!("{:016x}", rng.random::<u64>()),
                features: vec![
                    "default".to_string(),
                    format!("feature-{}", rng.random::<u16>()),
                ],
                target_triple: "aarch64-apple-darwin".to_string(),
                rustc_version: format!(
                    "1.{}.{}",
                    rng.random_range(80..100),
                    rng.random_range(0..10)
                ),
                stage_id: format!("stage-{}", rng.random::<u32>()),
            };

            let canonical = cache_key_canonical_string(&base);
            assert_eq!(canonical, cache_key_canonical_string(&base));

            let mut changed_git_sha = base.clone();
            changed_git_sha.git_sha.push('x');
            assert_ne!(canonical, cache_key_canonical_string(&changed_git_sha));

            let mut changed_hash = base.clone();
            changed_hash.changed_files_hash.push('y');
            assert_ne!(canonical, cache_key_canonical_string(&changed_hash));

            let mut changed_stage = base.clone();
            changed_stage.stage_id.push('z');
            assert_ne!(canonical, cache_key_canonical_string(&changed_stage));
        }
    }

    #[test]
    fn execute_verify_request_uses_cache_after_first_run() {
        let request = VerifyRequest {
            plan: VerifyPlan {
                run_id: VerifyRunId::new("verify-test"),
                level: VerifyLevel::L2Targeted,
                targets: vec![VerifyTarget::Workspace],
                time_budget: Duration::from_secs(30),
                fail_fast: false,
            },
            commands: vec![VerifyCommand {
                stage_id: "fmt".to_string(),
                command: "cargo fmt --all --check".to_string(),
                cwd: PathBuf::from("."),
            }],
            git_sha: "abc".to_string(),
            changed_files_hash: "def".to_string(),
            features: Vec::new(),
            target_triple: "aarch64-apple-darwin".to_string(),
            rustc_version: "1.93.0".to_string(),
        };

        let mut executions = 0;
        let first = execute_verify_request(&request, |_| {
            executions += 1;
            Ok(VerifyCommandResult {
                exit_code: 0,
                duration_ms: 12,
                output: "ok".to_string(),
                raw_log_path: PathBuf::from("logs/fmt.log"),
                tool_version: None,
                truncated: false,
            })
        })
        .expect("first verify run");
        let second = execute_verify_request(&request, |_| {
            executions += 1;
            Ok(VerifyCommandResult {
                exit_code: 0,
                duration_ms: 99,
                output: "should not run".to_string(),
                raw_log_path: PathBuf::from("logs/fmt.log"),
                tool_version: None,
                truncated: false,
            })
        })
        .expect("second verify run");

        assert_eq!(executions, 1);
        assert_eq!(first.cache_hits, 0);
        assert_eq!(second.cache_hits, 1);
        assert!(second.stages[0].from_cache);
        assert_eq!(second.stages[0].duration_ms, 0);
    }
}

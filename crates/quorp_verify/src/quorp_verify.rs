//! Staged verification ladder L0..L4 with content-addressed result cache.
//!
//! Phase 7 ships the API surface and a minimal blake3-style cache key
//! computation so downstream crates can target the type. The cargo /
//! rustfmt / miri executors land in the runtime wire-up phase.

use quorp_ids::VerifyRunId;
pub use quorp_verify_model::*;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

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
}

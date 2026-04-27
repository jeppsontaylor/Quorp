//! Domain types for Quorp's staged verification ladder.

#![allow(dead_code)]

use std::path::PathBuf;
use std::time::Duration;

use quorp_ids::VerifyRunId;
use quorp_repo_graph::SymbolPath;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerifyLevel {
    L0Static,
    L1Check,
    L2Targeted,
    L3Broad,
    L4Deep,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VerifyTarget {
    Workspace,
    Package(String),
    File(PathBuf),
    Symbol(SymbolPath),
    Test(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyPlan {
    pub run_id: VerifyRunId,
    pub level: VerifyLevel,
    pub targets: Vec<VerifyTarget>,
    #[serde(with = "duration_secs")]
    pub time_budget: Duration,
    pub fail_fast: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Pass,
    Fail,
    Partial,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageStatus {
    Pass,
    Fail,
    Skipped,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Failure {
    pub code: Option<String>,
    pub message: String,
    pub level: String,
    pub file: Option<PathBuf>,
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEvidence {
    pub command: String,
    pub cwd: PathBuf,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub tool_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticSpan {
    pub file: PathBuf,
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CargoDiagnostic {
    pub level: String,
    pub code: Option<String>,
    pub message: String,
    pub primary_span: Option<DiagnosticSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestFailure {
    pub name: String,
    pub panic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityFinding {
    pub advisory_id: Option<String>,
    pub severity: Option<String>,
    pub package: Option<String>,
    pub path: Option<PathBuf>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofPacketKind {
    Compiler,
    Test,
    Security,
    Command,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofPacket {
    pub kind: ProofPacketKind,
    pub command: CommandEvidence,
    pub summary: String,
    pub diagnostics: Vec<CargoDiagnostic>,
    pub failing_tests: Vec<TestFailure>,
    pub security_findings: Vec<SecurityFinding>,
    pub raw_log_ref: ArtifactRef,
    pub redacted: bool,
    pub truncated: bool,
}

/// Content-addressed cache key components — full key is the blake3 hash
/// of the concatenation in canonical order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheKey {
    pub git_sha: String,
    pub changed_files_hash: String,
    pub features: Vec<String>,
    pub target_triple: String,
    pub rustc_version: String,
    pub stage_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageReport {
    pub stage_id: String,
    pub status: StageStatus,
    pub duration_ms: u64,
    pub summary: String,
    pub failures: Vec<Failure>,
    pub raw_log_ref: Option<ArtifactRef>,
    pub cache_key: CacheKey,
    pub from_cache: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    pub plan: VerifyPlan,
    pub stages: Vec<StageReport>,
    #[serde(default)]
    pub proof_packets: Vec<ProofPacket>,
    pub overall: Verdict,
    pub cache_hits: u32,
    pub wall_ms: u64,
}

mod duration_secs {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Duration, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_f64(value.as_secs_f64())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Duration, D::Error> {
        let secs = f64::deserialize(de)?;
        Ok(Duration::from_secs_f64(secs.max(0.0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_plan_round_trip() {
        let plan = VerifyPlan {
            run_id: VerifyRunId::new("v-001"),
            level: VerifyLevel::L1Check,
            targets: vec![VerifyTarget::Workspace],
            time_budget: Duration::from_secs(60),
            fail_fast: true,
        };
        let json = serde_json::to_string(&plan).unwrap();
        let back: VerifyPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(back.run_id, plan.run_id);
        assert_eq!(back.time_budget, Duration::from_secs(60));
    }

    #[test]
    fn proof_packet_round_trips_with_decisive_fields() {
        let packet = ProofPacket {
            kind: ProofPacketKind::Compiler,
            command: CommandEvidence {
                command: "cargo check --message-format=json".to_string(),
                cwd: PathBuf::from("."),
                exit_code: 1,
                duration_ms: 42,
                tool_version: Some("rustc 1.93.0".to_string()),
            },
            summary: "1 compiler diagnostic".to_string(),
            diagnostics: vec![CargoDiagnostic {
                level: "error".to_string(),
                code: Some("E0308".to_string()),
                message: "mismatched types".to_string(),
                primary_span: Some(DiagnosticSpan {
                    file: PathBuf::from("src/lib.rs"),
                    line: 7,
                    column: 9,
                }),
            }],
            failing_tests: Vec::new(),
            security_findings: Vec::new(),
            raw_log_ref: ArtifactRef {
                path: PathBuf::from("logs/check.ndjson"),
                sha256: "abc123".to_string(),
            },
            redacted: false,
            truncated: false,
        };

        let json = serde_json::to_string(&packet).unwrap();
        let back: ProofPacket = serde_json::from_str(&json).unwrap();
        assert_eq!(back.command.exit_code, 1);
        assert_eq!(back.diagnostics[0].code.as_deref(), Some("E0308"));
        assert_eq!(back.raw_log_ref.path, PathBuf::from("logs/check.ndjson"));
    }
}

use serde::{Deserialize, Serialize};

use crate::failure::FailureClassification;
use crate::patch_lease::PatchLeaseTarget;
use crate::progress::ProgressState;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationHistoryEntry {
    pub command: String,
    pub status: String,
    pub excerpt: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct AvailableContextRef {
    pub label: String,
    pub path: Option<String>,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkTelemetry {
    pub issue_id: Option<String>,
    pub validation_status: Option<String>,
    pub non_authoritative: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SecurityBoundary {
    pub description: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub step: usize,
    pub stall_count: usize,
    pub parser_recovery_failures: usize,
    pub redundant_inspection_turns: usize,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepairContext {
    pub goal: String,
    pub state_snapshot: StateSnapshot,
    pub validation_history: Vec<ValidationHistoryEntry>,
    pub touched_files: Vec<String>,
    pub available_context_refs: Vec<AvailableContextRef>,
    pub failure_classifications: Vec<FailureClassification>,
    pub progress: ProgressState,
    pub benchmark_metadata: Option<BenchmarkTelemetry>,
    pub security_boundaries: Vec<SecurityBoundary>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecoveryPacket {
    pub objective: String,
    pub failed_hypotheses: Vec<String>,
    pub proof_refs: Vec<String>,
    pub leased_targets: Vec<PatchLeaseTarget>,
    pub required_next_action: String,
    pub forbidden_actions: Vec<String>,
    pub context_budget: Option<String>,
    pub security_boundary: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum RepairDecision {
    AskModelWithRecoveryPacket {
        packet: RecoveryPacket,
    },
    RequireAnchoredRead {
        packet: RecoveryPacket,
    },
    LeasePatchTarget {
        packet: RecoveryPacket,
    },
    RollBackAndReplan {
        packet: RecoveryPacket,
    },
    StopForHuman {
        reason: String,
        packet: RecoveryPacket,
    },
}

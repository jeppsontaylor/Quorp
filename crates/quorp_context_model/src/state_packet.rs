use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskNodeSnapshot {
    pub task_id: String,
    pub label: String,
    pub state: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskDagSnapshot {
    pub root_task_id: Option<String>,
    pub nodes: Vec<TaskNodeSnapshot>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SecurityBoundaryRecord {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub turn: usize,
    pub summary: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct FailureRecord {
    pub turn: usize,
    pub summary: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PatchStateSnapshot {
    pub leased_path: Option<String>,
    pub leased_range: Option<String>,
    pub expected_hash: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryReference {
    pub label: String,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuleReference {
    pub rule_id: String,
    pub summary: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProvenanceRecord {
    pub source: String,
    pub content_hash: Option<String>,
    pub recorded_turn: usize,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MissionStatePacket {
    pub packet_id: String,
    pub ledger_span: Option<String>,
    pub ledger_hash: Option<String>,
    pub objective: String,
    pub constraints: Vec<String>,
    pub security_boundaries: Vec<SecurityBoundaryRecord>,
    pub task_dag_snapshot: TaskDagSnapshot,
    pub decisions: Vec<DecisionRecord>,
    pub failed_attempts: Vec<FailureRecord>,
    pub validation: Vec<String>,
    pub patch_state: PatchStateSnapshot,
    pub context_refs: Vec<String>,
    pub memory_refs: Vec<MemoryReference>,
    pub rule_refs: Vec<RuleReference>,
    pub budget_snapshot: Option<crate::ContextBudgetTelemetry>,
    pub provenance: ProvenanceRecord,
    pub content_hash: String,
}

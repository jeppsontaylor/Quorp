use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowedPatchOperation {
    Read,
    Preview,
    SemanticEdit,
    ReplaceRange,
    ApplyPatch,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PatchLeaseTarget {
    pub path: String,
    pub range: Option<(usize, usize)>,
    pub expected_hash: Option<String>,
    pub allowed_operations: Vec<AllowedPatchOperation>,
    pub reason: String,
    pub expiry_turn: Option<usize>,
}

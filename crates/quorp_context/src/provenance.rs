use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PromptProvenance {
    pub source: String,
    pub recorded_turn: usize,
    pub content_hash: String,
}

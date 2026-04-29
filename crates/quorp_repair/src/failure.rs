use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PrimarySpan {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "snake_case")]
pub enum FailureClassification {
    ParserFailure {
        error_class: String,
        summary: String,
    },
    StaleHash {
        path: String,
        expected_hash: String,
        actual_hash: Option<String>,
    },
    ValidationFailure {
        command: String,
        primary_span: Option<PrimarySpan>,
        excerpt: Option<String>,
    },
    NoProgress {
        repeated_observation_count: usize,
    },
    WrongTarget {
        requested_path: String,
        expected_path: Option<String>,
    },
    BroadWriteRisk {
        path: String,
        changed_line_estimate: usize,
    },
}

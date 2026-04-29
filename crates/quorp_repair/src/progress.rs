use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProgressObservation {
    pub turn: usize,
    pub signal: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, Default)]
pub struct ProgressState {
    pub repeated_observation_count: usize,
    pub observations: Vec<ProgressObservation>,
}

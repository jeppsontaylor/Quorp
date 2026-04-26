//! Wire types for Quorp's agent runtime — actions, turn responses, runtime
//! events, transcript shapes.
//!
//! Phase 1 ships this crate as a placeholder so downstream crates can
//! depend on it. The actual wire types are extracted from
//! `quorp_agent_core::runtime` in Phase 3 of the refactor.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// Placeholder marker type. Replaced in Phase 3 with the full
/// `AgentTurnResponse` extracted from `quorp_agent_core::runtime`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolPlaceholder;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_serialises() {
        let value = ProtocolPlaceholder;
        let json = serde_json::to_string(&value).unwrap();
        assert!(!json.is_empty());
    }
}

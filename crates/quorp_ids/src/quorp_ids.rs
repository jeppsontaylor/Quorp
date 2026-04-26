//! Stable newtype identifiers and `E_*` error-code primitives for Quorp.
//!
//! Every identifier in the system has a typed home here so that wrong-owner
//! mistakes (e.g. handing a `TurnId` to something that wants a `SessionId`)
//! surface as compile errors instead of runtime confusion.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }
    };
}

string_id!(
    /// Identifier for a Quorp session (one user-facing conversation).
    SessionId
);

string_id!(
    /// Identifier for a single agent turn within a session.
    TurnId
);

string_id!(
    /// Identifier for a tool invocation within a turn.
    ToolCallId
);

string_id!(
    /// Identifier for a chunk in a context pack.
    ChunkId
);

string_id!(
    /// Identifier for an attention lease (cooperative cancellation handle).
    LeaseId
);

string_id!(
    /// Identifier for a learned rule emitted by the rule forge.
    RuleId
);

string_id!(
    /// Identifier for a compiled context pack.
    PackId
);

string_id!(
    /// Identifier for a patch plan (group of patch ops applied atomically).
    PatchId
);

string_id!(
    /// Identifier for a verification run.
    VerifyRunId
);

/// Stable Quorp error codes. Codes survive renames and refactors — once
/// published, they are part of the public contract and may only be
/// deprecated, never repurposed.
#[derive(Debug, thiserror::Error)]
pub enum QuorpError {
    #[error("E_SESSION_NOT_FOUND: no session named {0}")]
    SessionNotFound(SessionId),

    #[error("E_TURN_NOT_FOUND: no turn named {0}")]
    TurnNotFound(TurnId),

    #[error("E_TOOL_CALL_NOT_FOUND: no tool call {0}")]
    ToolCallNotFound(ToolCallId),

    #[error("E_PERMISSION_DENIED: action denied by permission policy")]
    PermissionDenied,

    #[error("E_PATH_ESCAPES_WORKSPACE: path {path} escapes workspace {workspace}")]
    PathEscapesWorkspace { path: String, workspace: String },

    #[error("E_PRECONDITION_FAILED: {0}")]
    PreconditionFailed(String),

    #[error("E_BUDGET_EXCEEDED: token budget exhausted")]
    BudgetExceeded,

    #[error("E_TIMEOUT: operation timed out after {0}")]
    Timeout(String),

    #[error("E_CANCELLED: operation cancelled")]
    Cancelled,

    #[error("E_INVALID_INPUT: {0}")]
    InvalidInput(String),

    #[error("E_INTERNAL: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_display_and_eq() {
        let a = SessionId::new("abc123");
        let b: SessionId = "abc123".into();
        assert_eq!(a, b);
        assert_eq!(a.to_string(), "abc123");
    }

    #[test]
    fn distinct_id_types_are_not_interchangeable() {
        // This test passes by virtue of compiling — we just instantiate
        // both, and the compiler enforces type distinctness elsewhere.
        let _ = SessionId::new("s");
        let _ = TurnId::new("t");
    }

    #[test]
    fn error_code_prefixes_are_stable() {
        let err = QuorpError::PermissionDenied;
        assert!(err.to_string().starts_with("E_PERMISSION_DENIED"));
        let err = QuorpError::BudgetExceeded;
        assert!(err.to_string().starts_with("E_BUDGET_EXCEEDED"));
    }
}

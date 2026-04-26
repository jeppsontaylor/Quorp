//! Rust-specific analysers — borrow doctor, trait obligation explainer,
//! Send/Sync auditor, feature matrix runner, macro expander.
//!
//! Phase 7 lands the public surface and the borrow-doctor classifier
//! that turns common rustc error codes into structured advice. Real
//! rust-analyzer integration follows.

use quorp_verify_model::Failure;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BorrowKind {
    BorrowAfterMove,
    MutableBorrowConflict,
    LifetimeMismatch,
    DanglingReference,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BorrowExplanation {
    pub kind: BorrowKind,
    pub summary: String,
    pub fix_hints: Vec<String>,
}

pub fn diagnose_borrow_failure(failure: &Failure) -> Option<BorrowExplanation> {
    let code = failure.code.as_deref()?;
    match code {
        "E0382" => Some(BorrowExplanation {
            kind: BorrowKind::BorrowAfterMove,
            summary: "value was moved earlier and cannot be used again".into(),
            fix_hints: vec![
                "clone the value before moving".into(),
                "borrow with & instead of moving".into(),
                "restructure to keep ownership in one place".into(),
            ],
        }),
        "E0499" | "E0500" | "E0502" => Some(BorrowExplanation {
            kind: BorrowKind::MutableBorrowConflict,
            summary: "two borrows that conflict according to the borrow checker".into(),
            fix_hints: vec![
                "narrow the scope of the first borrow".into(),
                "use split-borrow patterns or shadowing".into(),
            ],
        }),
        "E0623" | "E0312" | "E0495" => Some(BorrowExplanation {
            kind: BorrowKind::LifetimeMismatch,
            summary: "lifetimes did not unify".into(),
            fix_hints: vec![
                "introduce an explicit lifetime parameter".into(),
                "tie the return lifetime to one input borrow".into(),
            ],
        }),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraitObligation {
    pub trait_name: String,
    pub missing_impl_for: Option<String>,
    pub suggestions: Vec<String>,
}

pub fn explain_trait_failure(failure: &Failure) -> Option<TraitObligation> {
    let msg = &failure.message;
    if !(msg.contains("trait bound") || msg.contains("not satisfied")) {
        return None;
    }
    Some(TraitObligation {
        trait_name: extract_quoted(msg, "trait `").unwrap_or_else(|| "<unknown>".into()),
        missing_impl_for: extract_quoted(msg, "for `"),
        suggestions: vec![
            "derive or implement the trait for the offending type".into(),
            "constrain the generic parameter at the call site".into(),
        ],
    })
}

fn extract_quoted(haystack: &str, needle: &str) -> Option<String> {
    let start = haystack.find(needle)? + needle.len();
    let rest = &haystack[start..];
    let end = rest.find('`')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(code: &str, msg: &str) -> Failure {
        Failure {
            code: Some(code.into()),
            message: msg.into(),
            level: "error".into(),
            file: None,
            line: None,
        }
    }

    #[test]
    fn borrow_after_move_classifies() {
        let f = failure("E0382", "borrow of moved value");
        let exp = diagnose_borrow_failure(&f).unwrap();
        assert_eq!(exp.kind, BorrowKind::BorrowAfterMove);
        assert!(!exp.fix_hints.is_empty());
    }

    #[test]
    fn unrelated_code_returns_none() {
        let f = failure("E9999", "not real");
        assert!(diagnose_borrow_failure(&f).is_none());
    }

    #[test]
    fn trait_obligation_extracts_name() {
        let f = failure(
            "E0277",
            "the trait `Send` is not satisfied for type `MyType`",
        );
        let obligation = explain_trait_failure(&f).unwrap();
        assert_eq!(obligation.trait_name, "Send");
        assert!(!obligation.suggestions.is_empty());
    }
}

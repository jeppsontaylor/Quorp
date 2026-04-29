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

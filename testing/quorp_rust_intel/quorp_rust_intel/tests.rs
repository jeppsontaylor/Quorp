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

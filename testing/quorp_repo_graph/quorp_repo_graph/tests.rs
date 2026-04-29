use super::*;

#[test]
fn symbol_path_round_trips() {
    let p = SymbolPath::new("quorp_core::PermissionMode");
    let json = serde_json::to_string(&p).unwrap();
    let back: SymbolPath = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

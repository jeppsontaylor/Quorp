use std::path::Path;

use super::*;

#[test]
fn classify_write_file_denies_large_source_write_without_lease() {
    let content = vec!["line".to_string(); 201].join("\n");
    let decision = classify_write_file(Path::new("src/lib.rs"), &content, None);
    assert!(matches!(decision, WriteContractDecision::Denied { .. }));
}

#[test]
fn classify_write_file_allows_small_write() {
    let decision = classify_write_file(Path::new("src/lib.rs"), "short\n", None);
    assert!(matches!(decision, WriteContractDecision::Allowed { .. }));
}

#[test]
fn classify_write_file_allows_generated_path_without_lease() {
    let content = vec!["line".to_string(); 201].join("\n");
    let decision = classify_write_file(Path::new("target/generated.rs"), &content, None);
    assert!(matches!(decision, WriteContractDecision::Allowed { .. }));
}

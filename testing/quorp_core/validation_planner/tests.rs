use super::*;

#[test]
fn detects_mixed_projects_and_plans_browser_validation() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let root = tempdir.path();
    fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("cargo");
    fs::write(
        root.join("package.json"),
        r#"{
  "name": "demo",
  "scripts": {
    "lint": "eslint .",
    "test": "vitest"
  }
}"#,
    )
    .expect("package");
    fs::write(root.join("playwright.config.ts"), "export default {};").expect("playwright");

    let result = plan_validation(root);
    assert!(result.project.is_some());
    assert!(
        result
            .project
            .as_ref()
            .expect("project")
            .kinds
            .contains(&ProjectKind::Rust)
    );
    assert!(
        result
            .project
            .as_ref()
            .expect("project")
            .kinds
            .contains(&ProjectKind::Node)
    );
    assert!(
        result
            .commands
            .iter()
            .any(|command| command.stage == ValidationStage::Format && command.ecosystem == "rust")
    );
    assert!(
        result
            .commands
            .iter()
            .any(|command| command.stage == ValidationStage::Browser)
    );
    assert!(
        result
            .commands
            .iter()
            .any(|command| command.command == "npm run lint")
    );
}

#[test]
fn summarizes_validation_failure_with_anchor_and_excerpt() {
    let output = r#"
error: something broke
  --> src/main.rs:12:5
   |
12 | let value = missing();
   |     ^^^^^
FAILED
"#;
    let failure = summarize_validation_failure("cargo test", output);
    assert_eq!(failure.command, "cargo test");
    assert_eq!(failure.summary, "rust_validation_failure");
    assert_eq!(failure.path.as_deref(), Some(Path::new("src/main.rs")));
    assert_eq!(failure.line, Some(12));
    assert!(!failure.excerpts.is_empty());
}

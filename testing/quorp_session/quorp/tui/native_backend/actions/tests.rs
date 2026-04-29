use super::*;

#[test]
fn timeout_cleanup_errors_are_included_in_failure_output() {
    let mut output = String::from("partial output\n[Command timed out]");

    append_command_timeout_cleanup_errors(
        &mut output,
        &[
            "failed to kill command process group 123: no such process".to_string(),
            "failed to wait on timed-out command: child already waited".to_string(),
        ],
    );

    assert!(output.contains("[Command cleanup failed: "));
    assert!(output.contains("failed to kill command process group 123"));
    assert!(output.contains("failed to wait on timed-out command"));
}

#[test]
fn validation_plan_maps_to_verify_request() {
    let plan = crate::quorp::tui::agent_protocol::ValidationPlan {
        fmt: true,
        clippy: true,
        workspace_tests: false,
        tests: vec!["crate::tests::smoke".to_string()],
        custom_commands: vec!["cargo check -p quorp_session".to_string()],
    };
    let commands = vec![
        "cargo fmt --all --check".to_string(),
        "cargo clippy --workspace".to_string(),
        "cargo test crate::tests::smoke".to_string(),
        "cargo check -p quorp_session".to_string(),
    ];

    let request = validation_plan_to_verify_request(Path::new("."), &plan, &commands);

    assert_eq!(request.plan.level, VerifyLevel::L3Broad);
    assert_eq!(request.commands.len(), 4);
    assert_eq!(request.commands[0].stage_id, "fmt");
    assert_eq!(request.commands[1].stage_id, "clippy");
    assert!(
        request.plan.targets.iter().any(
            |target| matches!(target, VerifyTarget::Test(name) if name == "crate::tests::smoke")
        )
    );
}

#[test]
fn apply_single_file_change_returns_patch_vm_receipt_and_updates_bytes() {
    let root = tempfile::tempdir().expect("tempdir");
    let target = root.path().join("target.txt");

    let receipt = apply_single_file_change(
        41,
        "target.txt".to_string(),
        target.clone(),
        "hello\n",
        EditProvenance::WriteFile {
            path: PathBuf::from("target.txt"),
        },
    )
    .expect("apply");

    assert!(receipt.contains("patch_vm_receipt"));
    assert!(receipt.contains("patch_vm_receipt_v2"));
    assert!(receipt.contains("rollback_tokens: 0"));
    assert_eq!(std::fs::read_to_string(target).expect("read"), "hello\n");
}

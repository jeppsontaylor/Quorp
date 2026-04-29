use super::*;

#[test]
fn path_guard_rejects_edits_outside_workspace() {
    let workspace = tempfile::tempdir().expect("workspace");
    let guard = PathGuard::new(workspace.path(), PermissionMode::Ask);

    let error = guard
        .ensure_editable(Path::new("/tmp/not-in-this-workspace.txt"))
        .expect_err("outside path rejected");

    assert!(error.to_string().contains("outside workspace"));
}

//! GPUI-thread tests: [`crate::quorp::tui::path_index_bridge::collect_path_entries_from_project`]
//! against a real [`project::Project`] — same entry set the TUI path-index bridge publishes.

use gpui::TestAppContext;
use project::Project;
use serde_json::json;
use settings::SettingsStore;

use crate::quorp::tui::path_index_bridge::collect_path_entries_from_project;
use util::path;

fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let settings_store = SettingsStore::test(cx);
        cx.set_global(settings_store);
    });
}

#[gpui::test]
async fn collect_path_entries_matches_worktree_files(cx: &mut TestAppContext) {
    init_test(cx);

    let fs = fs::FakeFs::new(cx.executor());
    fs.insert_tree(
        path!("/project"),
        json!({
            "src": { "main.rs": "fn main() {}" },
            "README.md": "# hi",
        }),
    )
    .await;

    let project = Project::test(fs.clone(), [path!("/project").as_ref()], cx).await;
    cx.executor().run_until_parked();

    let root = path!("/project");
    let entries = cx.update(|cx| collect_path_entries_from_project(&project, root.as_ref(), cx));

    let rels: Vec<&str> = entries
        .iter()
        .map(|e| e.relative_display.as_str())
        .filter(|r| *r != ".")
        .collect();
    assert!(
        rels.iter().any(|r| *r == "README.md"),
        "expected README.md in {:?}",
        rels
    );
    assert!(
        rels.iter().any(|r| *r == "src" || r.starts_with("src/")),
        "expected src in {:?}",
        rels
    );
    assert!(
        rels.iter().any(|r| *r == "src/main.rs" || *r == "main.rs"),
        "expected main.rs path in {:?}",
        rels
    );
}

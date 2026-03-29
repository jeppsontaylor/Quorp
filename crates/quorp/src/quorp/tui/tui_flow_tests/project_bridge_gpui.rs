//! GPUI-thread tests for [`crate::quorp::tui::bridge::list_children_sync`] against a real
//! [`project::Project`] on [`fs::FakeFs`], matching production TUI file-tree data.

use gpui::TestAppContext;
use project::Project;
use serde_json::json;
use settings::SettingsStore;

use crate::quorp::tui::bridge::list_children_sync;
use util::path;

fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let settings_store = SettingsStore::test(cx);
        cx.set_global(settings_store);
    });
}

#[gpui::test]
async fn list_children_sync_matches_fake_fs_tree(cx: &mut TestAppContext) {
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

    let children = cx.update(|cx| list_children_sync(&project, path!("/project").as_ref(), cx));
    assert!(
        children.is_ok(),
        "list project root: {:?}",
        children.as_ref().err()
    );
    let children = children.unwrap();

    let names: Vec<_> = children.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"README.md"),
        "expected README.md in {names:?}"
    );
    assert!(names.contains(&"src"), "expected src dir in {names:?}");
    let src = children.iter().find(|c| c.name == "src").expect("src row");
    assert!(src.is_directory);

    let src_children = cx.update(|cx| list_children_sync(&project, src.path.as_path(), cx));
    assert!(
        src_children.is_ok(),
        "list src: {:?}",
        src_children.as_ref().err()
    );
    let src_children = src_children.unwrap();
    assert_eq!(src_children.len(), 1);
    assert_eq!(src_children[0].name, "main.rs");
    assert!(!src_children[0].is_directory);
}

#[gpui::test]
async fn list_children_sync_separate_worktree_roots(cx: &mut TestAppContext) {
    init_test(cx);

    let fs = fs::FakeFs::new(cx.executor());
    fs.insert_tree(
        path!("/alpha"),
        json!({
            "only_a.txt": "a",
        }),
    )
    .await;
    fs.insert_tree(
        path!("/beta"),
        json!({
            "only_b.txt": "b",
        }),
    )
    .await;

    let project = Project::test(
        fs.clone(),
        [path!("/alpha").as_ref(), path!("/beta").as_ref()],
        cx,
    )
    .await;
    cx.executor().run_until_parked();

    let alpha_children = cx.update(|cx| list_children_sync(&project, path!("/alpha").as_ref(), cx));
    let alpha_children = alpha_children.expect("list /alpha");
    let alpha_names: Vec<_> = alpha_children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(alpha_names, vec!["only_a.txt"]);

    let beta_children = cx.update(|cx| list_children_sync(&project, path!("/beta").as_ref(), cx));
    let beta_children = beta_children.expect("list /beta");
    let beta_names: Vec<_> = beta_children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(beta_names, vec!["only_b.txt"]);
}

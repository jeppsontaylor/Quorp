use super::*;

#[test]
fn discovers_skills_and_recipes_from_project_roots() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let project_root = tempdir.path();
    let skill_root = project_root.join(".quorp/skills/rust-debugging");
    fs::create_dir_all(&skill_root).expect("skill root");
    fs::write(
        skill_root.join("SKILL.md"),
        r#"---
name: rust-debugging
description: Fix Rust compiler failures.
---
# rust-debugging
Use cargo and rust-analyzer.
"#,
    )
    .expect("skill file");
    fs::create_dir_all(project_root.join(".quorp/recipes")).expect("recipes root");
    fs::write(
        project_root.join(".quorp/recipes/fix-ci.toml"),
        r#"
name = "fix-ci"
description = "Repair CI failures"
allowed_tools = ["cargo", "git"]
validation_commands = ["cargo test", "cargo clippy"]
success_criteria = ["green checks"]
"#,
    )
    .expect("recipe file");

    let catalog = discover_skill_catalog(project_root);
    assert_eq!(catalog.skills.len(), 1);
    assert_eq!(catalog.skills[0].name, "rust-debugging");
    assert_eq!(catalog.recipes.len(), 1);
    assert_eq!(catalog.recipes[0].name, "fix-ci");
    let rendered = catalog.render_prompt_section();
    assert!(rendered.contains("rust-debugging"));
    assert!(rendered.contains("fix-ci"));
}

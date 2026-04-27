use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillCatalog {
    pub skills: Vec<SkillMetadata>,
    pub recipes: Vec<RecipeDefinition>,
}

impl SkillCatalog {
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty() && self.recipes.is_empty()
    }

    pub fn render_prompt_section(&self) -> String {
        if self.is_empty() {
            return String::new();
        }

        let mut lines = vec!["Available skills and recipes:".to_string()];
        if !self.skills.is_empty() {
            lines.push("Skills:".to_string());
            for skill in &self.skills {
                let description = skill
                    .description
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("no description");
                lines.push(format!("- {}: {}", skill.name, description));
            }
        }
        if !self.recipes.is_empty() {
            lines.push("Recipes:".to_string());
            for recipe in &self.recipes {
                let description = recipe
                    .description
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("no description");
                lines.push(format!("- {}: {}", recipe.name, description));
            }
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    pub description: Option<String>,
    pub root: PathBuf,
    pub instructions_path: PathBuf,
    pub scripts: Vec<PathBuf>,
    pub references: Vec<PathBuf>,
    pub assets: Vec<PathBuf>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecipeDefinition {
    pub name: String,
    pub description: Option<String>,
    pub path: PathBuf,
    pub allowed_tools: Vec<String>,
    pub validation_commands: Vec<String>,
    pub success_criteria: Vec<String>,
    pub subrecipes: Vec<String>,
}

pub fn discover_skill_catalog(project_root: &Path) -> SkillCatalog {
    let mut catalog = SkillCatalog::default();
    let mut seen_skill_roots = BTreeSet::new();
    for root in skill_root_candidates(project_root) {
        if !seen_skill_roots.insert(root.clone()) {
            continue;
        }
        catalog.skills.extend(discover_skills_in_root(&root));
    }
    let mut seen_recipe_roots = BTreeSet::new();
    for root in recipe_root_candidates(project_root) {
        if !seen_recipe_roots.insert(root.clone()) {
            continue;
        }
        catalog.recipes.extend(discover_recipes_in_root(&root));
    }
    catalog
        .skills
        .sort_by(|left, right| left.name.cmp(&right.name));
    catalog
        .recipes
        .sort_by(|left, right| left.name.cmp(&right.name));
    catalog
}

fn skill_root_candidates(project_root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![
        project_root.join(".quorp/skills"),
        project_root.join(".agents/skills"),
        project_root.join(".claude/skills"),
        project_root.join(".cursor/skills"),
    ];
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        roots.push(home.join(".config/quorp/skills"));
        roots.push(home.join(".codex/skills"));
        roots.push(home.join(".agents/skills"));
        roots.push(home.join(".claude/skills"));
        roots.push(home.join(".cursor/skills"));
    }
    roots
}

fn recipe_root_candidates(project_root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![project_root.join(".quorp/recipes")];
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        roots.push(home.join(".config/quorp/recipes"));
        roots.push(home.join(".codex/recipes"));
        roots.push(home.join(".agents/recipes"));
        roots.push(home.join(".claude/recipes"));
    }
    roots
}

fn discover_skills_in_root(root: &Path) -> Vec<SkillMetadata> {
    let mut skills = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return skills;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let instructions_path = path.join("SKILL.md");
        if !instructions_path.is_file() {
            continue;
        }
        skills.push(load_skill_metadata(&path, &instructions_path));
    }
    skills
}

fn load_skill_metadata(root: &Path, instructions_path: &Path) -> SkillMetadata {
    let text = fs::read_to_string(instructions_path).unwrap_or_default();
    let frontmatter = parse_skill_frontmatter(&text);
    let name = frontmatter
        .get("name")
        .cloned()
        .or_else(|| {
            root.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "skill".to_string());
    let description = frontmatter.get("description").cloned().or_else(|| {
        first_nonempty_heading_or_summary(&text)
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.to_string())
    });
    SkillMetadata {
        name,
        description,
        root: root.to_path_buf(),
        instructions_path: instructions_path.to_path_buf(),
        scripts: list_child_files(root.join("scripts")),
        references: list_child_files(root.join("references")),
        assets: list_child_files(root.join("assets")),
    }
}

fn list_child_files(root: PathBuf) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect()
}

fn parse_skill_frontmatter(text: &str) -> std::collections::BTreeMap<String, String> {
    let mut result = std::collections::BTreeMap::new();
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return result;
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if !key.is_empty() && !value.is_empty() {
            result.insert(key.to_string(), value.to_string());
        }
    }
    result
}

fn first_nonempty_heading_or_summary(text: &str) -> Option<&str> {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "---" {
            continue;
        }
        if let Some(title) = trimmed.strip_prefix('#') {
            return Some(title.trim_start_matches('#').trim());
        }
        return Some(trimmed);
    }
    None
}

fn discover_recipes_in_root(root: &Path) -> Vec<RecipeDefinition> {
    let mut recipes = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return recipes;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_toml = path.extension().and_then(|ext| ext.to_str()) == Some("toml");
        if !is_toml || !path.is_file() {
            continue;
        }
        recipes.push(load_recipe_definition(&path));
    }
    recipes
}

fn load_recipe_definition(path: &Path) -> RecipeDefinition {
    let text = fs::read_to_string(path).unwrap_or_default();
    let parsed = text.parse::<toml::Value>().ok();
    let name = parsed
        .as_ref()
        .and_then(|value| value.get("name"))
        .and_then(toml_value_as_string)
        .or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "recipe".to_string());
    let description = parsed
        .as_ref()
        .and_then(|value| value.get("description"))
        .and_then(toml_value_as_string);
    let allowed_tools = parsed
        .as_ref()
        .and_then(|value| value.get("allowed_tools"))
        .and_then(toml_value_as_string_array)
        .unwrap_or_default();
    let validation_commands = parsed
        .as_ref()
        .and_then(|value| value.get("validation_commands"))
        .and_then(toml_value_as_string_array)
        .unwrap_or_default();
    let success_criteria = parsed
        .as_ref()
        .and_then(|value| value.get("success_criteria"))
        .and_then(toml_value_as_string_array)
        .unwrap_or_default();
    let subrecipes = parsed
        .as_ref()
        .and_then(|value| value.get("subrecipes"))
        .and_then(toml_value_as_string_array)
        .unwrap_or_default();
    RecipeDefinition {
        name,
        description,
        path: path.to_path_buf(),
        allowed_tools,
        validation_commands,
        success_criteria,
        subrecipes,
    }
}

fn toml_value_as_string(value: &toml::Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

fn toml_value_as_string_array(value: &toml::Value) -> Option<Vec<String>> {
    value.as_array().map(|items| {
        items
            .iter()
            .filter_map(toml_value_as_string)
            .filter(|item| !item.trim().is_empty())
            .collect()
    })
}

#[cfg(test)]
mod tests {
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
}

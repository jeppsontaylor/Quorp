use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    Rust,
    Node,
    Python,
    Go,
    Make,
    Just,
}

impl ProjectKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Node => "node",
            Self::Python => "python",
            Self::Go => "go",
            Self::Make => "make",
            Self::Just => "just",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStage {
    Format,
    Lint,
    Test,
    Browser,
    Custom,
}

impl ValidationStage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Format => "format",
            Self::Lint => "lint",
            Self::Test => "test",
            Self::Browser => "browser",
            Self::Custom => "custom",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationCommand {
    pub stage: ValidationStage,
    pub command: String,
    pub required: bool,
    pub ecosystem: String,
    pub confidence: u8,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationFailure {
    pub command: String,
    pub summary: String,
    #[serde(default)]
    pub ecosystem: Option<String>,
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub line: Option<usize>,
    #[serde(default)]
    pub excerpts: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DetectedProject {
    pub root: PathBuf,
    pub kinds: Vec<ProjectKind>,
    pub indicators: Vec<PathBuf>,
    pub confidence: u8,
}

impl DetectedProject {
    pub fn primary_kind(&self) -> Option<ProjectKind> {
        self.kinds.first().copied()
    }

    pub fn is_mixed(&self) -> bool {
        self.kinds.len() > 1
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationPlannerResult {
    pub project: Option<DetectedProject>,
    pub commands: Vec<ValidationCommand>,
    pub notes: Vec<String>,
}

impl ValidationPlannerResult {
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

pub fn detect_project(root: &Path) -> Option<DetectedProject> {
    let mut kinds = Vec::new();
    let mut indicators = Vec::new();
    let mut push_kind = |kind: ProjectKind, indicator: PathBuf| {
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
        if !indicators.contains(&indicator) {
            indicators.push(indicator);
        }
    };

    if root.join("Cargo.toml").is_file()
        || root.join("rust-toolchain.toml").is_file()
        || root.join("rustfmt.toml").is_file()
    {
        push_kind(ProjectKind::Rust, root.join("Cargo.toml"));
    }
    if root.join("package.json").is_file()
        || root.join("pnpm-lock.yaml").is_file()
        || root.join("yarn.lock").is_file()
        || root.join("package-lock.json").is_file()
        || root.join("bun.lockb").is_file()
        || root.join("bun.lock").is_file()
        || root.join("tsconfig.json").is_file()
        || root.join("jsconfig.json").is_file()
    {
        push_kind(ProjectKind::Node, root.join("package.json"));
    }
    if root.join("pyproject.toml").is_file()
        || root.join("requirements.txt").is_file()
        || root.join("setup.py").is_file()
        || root.join("setup.cfg").is_file()
        || root.join("tox.ini").is_file()
        || root.join("pytest.ini").is_file()
    {
        push_kind(ProjectKind::Python, root.join("pyproject.toml"));
    }
    if root.join("go.mod").is_file() {
        push_kind(ProjectKind::Go, root.join("go.mod"));
    }
    if root.join("Makefile").is_file()
        || root.join("makefile").is_file()
        || root.join("GNUmakefile").is_file()
    {
        push_kind(ProjectKind::Make, root.join("Makefile"));
    }
    if root.join("Justfile").is_file() || root.join("justfile").is_file() {
        push_kind(ProjectKind::Just, root.join("Justfile"));
    }

    if kinds.is_empty() {
        return None;
    }
    let confidence = (kinds.len() as u8).saturating_mul(25).min(100);
    Some(DetectedProject {
        root: root.to_path_buf(),
        kinds,
        indicators,
        confidence,
    })
}

pub fn plan_validation(root: &Path) -> ValidationPlannerResult {
    let project = detect_project(root);
    let mut commands = Vec::new();
    let mut notes = Vec::new();

    match project.as_ref() {
        Some(project) => {
            if project.is_mixed() {
                notes.push(format!(
                    "mixed project kinds detected: {}",
                    project
                        .kinds
                        .iter()
                        .map(|kind| kind.label())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            for kind in &project.kinds {
                commands.extend(plan_for_kind(root, *kind));
            }
        }
        None => {
            notes.push(
                "no recognized project markers found; using a Rust-like fallback".to_string(),
            );
            commands.extend(rust_validation_commands(75));
        }
    }

    dedup_commands(&mut commands);
    ValidationPlannerResult {
        project,
        commands,
        notes,
    }
}

pub fn summarize_validation_failure(command: &str, output: &str) -> ValidationFailure {
    let mut path = None;
    let mut line = None;
    let mut excerpts = Vec::new();
    let lower_output = output.to_ascii_lowercase();
    let lower_command = command.to_ascii_lowercase();
    let command_program = lower_command.split_whitespace().next().unwrap_or_default();
    let summary = if lower_output.contains("playwright") {
        "browser_validation_failure"
    } else if lower_output.contains("pytest") || lower_output.contains("python") {
        "python_test_failure"
    } else if command_program == "go"
        || lower_output.contains("go test")
        || lower_output.contains("go vet")
    {
        "go_test_failure"
    } else if command_program == "cargo"
        || lower_output.contains("cargo")
        || lower_output.contains("rustc")
    {
        "rust_validation_failure"
    } else {
        "validation_failure"
    };

    for raw_line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if path.is_none()
            && let Some((candidate_path, candidate_line)) = parse_anchor(raw_line)
        {
            path = Some(candidate_path);
            line = candidate_line;
        }
        if excerpts.len() < 6
            && (raw_line.starts_with("error")
                || raw_line.contains("failed")
                || raw_line.contains("AssertionError")
                || raw_line.contains("FAILED"))
        {
            excerpts.push(truncate_excerpt(raw_line, 160));
        }
    }

    ValidationFailure {
        command: command.trim().to_string(),
        summary: summary.to_string(),
        ecosystem: None,
        path,
        line,
        excerpts,
    }
}

fn plan_for_kind(root: &Path, kind: ProjectKind) -> Vec<ValidationCommand> {
    match kind {
        ProjectKind::Rust => rust_validation_commands(100),
        ProjectKind::Node => node_validation_commands(root),
        ProjectKind::Python => python_validation_commands(root),
        ProjectKind::Go => go_validation_commands(),
        ProjectKind::Make => make_validation_commands(root),
        ProjectKind::Just => just_validation_commands(root),
    }
}

fn rust_validation_commands(confidence: u8) -> Vec<ValidationCommand> {
    vec![
        ValidationCommand {
            stage: ValidationStage::Format,
            command: "cargo fmt --all -- --check".to_string(),
            required: true,
            ecosystem: "rust".to_string(),
            confidence,
            notes: vec!["fast syntax hygiene".to_string()],
        },
        ValidationCommand {
            stage: ValidationStage::Lint,
            command: "cargo clippy --all-targets --all-features --no-deps -- -D warnings"
                .to_string(),
            required: true,
            ecosystem: "rust".to_string(),
            confidence,
            notes: vec!["semantic lint".to_string()],
        },
        ValidationCommand {
            stage: ValidationStage::Test,
            command: "cargo test --workspace".to_string(),
            required: true,
            ecosystem: "rust".to_string(),
            confidence,
            notes: vec!["workspace tests".to_string()],
        },
    ]
}

fn node_validation_commands(root: &Path) -> Vec<ValidationCommand> {
    let package_json = root.join("package.json");
    let script_names = read_node_scripts(&package_json);
    let runner = detect_node_runner(root, &script_names);
    let mut commands = Vec::new();

    if has_playwright_support(root) {
        commands.push(ValidationCommand {
            stage: ValidationStage::Browser,
            command: playwright_command(&runner),
            required: false,
            ecosystem: "browser".to_string(),
            confidence: 80,
            notes: vec!["browser validation".to_string()],
        });
    }

    if script_names.contains("lint") {
        commands.push(validation_script_command(&runner, "lint", 80));
    }
    if script_names.contains("typecheck") {
        commands.push(validation_script_command(&runner, "typecheck", 80));
    } else if script_names.contains("check") {
        commands.push(validation_script_command(&runner, "check", 70));
    }
    if script_names.contains("test") {
        commands.push(validation_script_command(&runner, "test", 90));
    } else {
        commands.push(ValidationCommand {
            stage: ValidationStage::Test,
            command: package_runner_test_command(&runner),
            required: true,
            ecosystem: "node".to_string(),
            confidence: 65,
            notes: vec!["default test runner".to_string()],
        });
    }

    if commands.is_empty() {
        commands.push(ValidationCommand {
            stage: ValidationStage::Test,
            command: package_runner_test_command(&runner),
            required: true,
            ecosystem: "node".to_string(),
            confidence: 60,
            notes: vec!["fallback node validation".to_string()],
        });
    }
    commands
}

fn python_validation_commands(root: &Path) -> Vec<ValidationCommand> {
    let mut commands = Vec::new();
    if root.join("ruff.toml").is_file()
        || root.join(".ruff.toml").is_file()
        || root.join("pyproject.toml").is_file()
    {
        commands.push(ValidationCommand {
            stage: ValidationStage::Lint,
            command: "ruff check .".to_string(),
            required: false,
            ecosystem: "python".to_string(),
            confidence: 70,
            notes: vec!["static lint".to_string()],
        });
    }
    if root.join("pytest.ini").is_file()
        || root.join("tox.ini").is_file()
        || root.join("tests").is_dir()
        || root.join("test").is_dir()
    {
        commands.push(ValidationCommand {
            stage: ValidationStage::Test,
            command: "python -m pytest".to_string(),
            required: true,
            ecosystem: "python".to_string(),
            confidence: 85,
            notes: vec!["pytest coverage".to_string()],
        });
    } else {
        commands.push(ValidationCommand {
            stage: ValidationStage::Test,
            command: "python -m compileall .".to_string(),
            required: true,
            ecosystem: "python".to_string(),
            confidence: 55,
            notes: vec!["compile smoke".to_string()],
        });
    }
    commands
}

fn go_validation_commands() -> Vec<ValidationCommand> {
    vec![
        ValidationCommand {
            stage: ValidationStage::Lint,
            command: "go vet ./...".to_string(),
            required: false,
            ecosystem: "go".to_string(),
            confidence: 80,
            notes: vec!["static analysis".to_string()],
        },
        ValidationCommand {
            stage: ValidationStage::Test,
            command: "go test ./...".to_string(),
            required: true,
            ecosystem: "go".to_string(),
            confidence: 90,
            notes: vec!["workspace tests".to_string()],
        },
    ]
}

fn make_validation_commands(root: &Path) -> Vec<ValidationCommand> {
    let makefile = makefile_path(root);
    let text = fs::read_to_string(&makefile).unwrap_or_default();
    let mut commands = Vec::new();
    if has_target(&text, "check") {
        commands.push(ValidationCommand {
            stage: ValidationStage::Lint,
            command: "make check".to_string(),
            required: false,
            ecosystem: "make".to_string(),
            confidence: 70,
            notes: vec!["makefile check target".to_string()],
        });
    }
    if has_target(&text, "test") || commands.is_empty() {
        commands.push(ValidationCommand {
            stage: ValidationStage::Test,
            command: "make test".to_string(),
            required: true,
            ecosystem: "make".to_string(),
            confidence: if has_target(&text, "test") { 75 } else { 45 },
            notes: vec!["makefile test target".to_string()],
        });
    }
    commands
}

fn just_validation_commands(root: &Path) -> Vec<ValidationCommand> {
    let justfile = justfile_path(root);
    let text = fs::read_to_string(&justfile).unwrap_or_default();
    let mut commands = Vec::new();
    if has_target(&text, "check") {
        commands.push(ValidationCommand {
            stage: ValidationStage::Lint,
            command: "just check".to_string(),
            required: false,
            ecosystem: "just".to_string(),
            confidence: 70,
            notes: vec!["justfile check recipe".to_string()],
        });
    }
    if has_target(&text, "test") || commands.is_empty() {
        commands.push(ValidationCommand {
            stage: ValidationStage::Test,
            command: "just test".to_string(),
            required: true,
            ecosystem: "just".to_string(),
            confidence: if has_target(&text, "test") { 75 } else { 45 },
            notes: vec!["justfile test recipe".to_string()],
        });
    }
    commands
}

fn validation_script_command(
    runner: &PackageManager,
    script: &str,
    confidence: u8,
) -> ValidationCommand {
    ValidationCommand {
        stage: if script == "lint" || script == "typecheck" || script == "check" {
            ValidationStage::Lint
        } else {
            ValidationStage::Test
        },
        command: package_runner_script_command(runner, script),
        required: script == "test" || script == "typecheck",
        ecosystem: "node".to_string(),
        confidence,
        notes: vec![format!("package.json script: {script}")],
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

fn detect_node_runner(root: &Path, _scripts: &BTreeSet<String>) -> PackageManager {
    if root.join("pnpm-lock.yaml").is_file() || root.join("pnpm-workspace.yaml").is_file() {
        return PackageManager::Pnpm;
    }
    if root.join("yarn.lock").is_file() {
        return PackageManager::Yarn;
    }
    if root.join("bun.lockb").is_file() || root.join("bun.lock").is_file() {
        return PackageManager::Bun;
    }
    PackageManager::Npm
}

fn read_node_scripts(package_json: &Path) -> BTreeSet<String> {
    let text = fs::read_to_string(package_json).unwrap_or_default();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    value
        .get("scripts")
        .and_then(serde_json::Value::as_object)
        .map(|scripts| scripts.keys().cloned().collect())
        .unwrap_or_default()
}

fn has_playwright_support(root: &Path) -> bool {
    root.join("playwright.config.ts").is_file()
        || root.join("playwright.config.js").is_file()
        || root.join("playwright.config.mjs").is_file()
        || root.join("playwright.config.cjs").is_file()
        || root.join("playwright.config.json").is_file()
        || root.join("tests/e2e").is_dir()
        || root.join("e2e").is_dir()
}

fn package_runner_script_command(runner: &PackageManager, script: &str) -> String {
    match runner {
        PackageManager::Npm => format!("npm run {script}"),
        PackageManager::Pnpm => format!("pnpm run {script}"),
        PackageManager::Yarn => format!("yarn {script}"),
        PackageManager::Bun => format!("bun run {script}"),
    }
}

fn package_runner_test_command(runner: &PackageManager) -> String {
    match runner {
        PackageManager::Npm => "npm test".to_string(),
        PackageManager::Pnpm => "pnpm test".to_string(),
        PackageManager::Yarn => "yarn test".to_string(),
        PackageManager::Bun => "bun test".to_string(),
    }
}

fn playwright_command(runner: &PackageManager) -> String {
    match runner {
        PackageManager::Npm => "npx playwright test".to_string(),
        PackageManager::Pnpm => "pnpm exec playwright test".to_string(),
        PackageManager::Yarn => "yarn playwright test".to_string(),
        PackageManager::Bun => "bunx playwright test".to_string(),
    }
}

fn makefile_path(root: &Path) -> PathBuf {
    for name in ["Makefile", "makefile", "GNUmakefile"] {
        let candidate = root.join(name);
        if candidate.is_file() {
            return candidate;
        }
    }
    root.join("Makefile")
}

fn justfile_path(root: &Path) -> PathBuf {
    for name in ["Justfile", "justfile"] {
        let candidate = root.join(name);
        if candidate.is_file() {
            return candidate;
        }
    }
    root.join("Justfile")
}

fn has_target(text: &str, name: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with(&format!("{name}:"))
            || trimmed.starts_with(&format!("{name}::"))
            || trimmed.starts_with(&format!("{name} "))
    })
}

fn dedup_commands(commands: &mut Vec<ValidationCommand>) {
    let mut seen = BTreeSet::new();
    commands.retain(|command| seen.insert(command.command.clone()));
}

fn parse_anchor(line: &str) -> Option<(PathBuf, Option<usize>)> {
    let trimmed_line = line.trim();
    if !trimmed_line.starts_with("-->") {
        return None;
    }
    let trimmed = trimmed_line.trim_start_matches("-->").trim();
    let candidate = trimmed.split_whitespace().next()?;
    let path_part = candidate.split(':').next()?.trim();
    if path_part.is_empty() {
        return None;
    }
    let mut segments = candidate.rsplitn(3, ':').collect::<Vec<_>>();
    segments.reverse();
    let line = if segments.len() >= 2 {
        segments
            .get(segments.len().saturating_sub(2))
            .and_then(|value| value.parse::<usize>().ok())
    } else {
        None
    };
    Some((PathBuf::from(path_part), line))
}

fn truncate_excerpt(text: &str, max_chars: usize) -> String {
    let mut truncated = String::new();
    for (index, character) in text.chars().enumerate() {
        if index >= max_chars {
            truncated.push_str("...");
            break;
        }
        truncated.push(character);
    }
    truncated
}

#[cfg(test)]
mod tests {
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
                .any(|command| command.stage == ValidationStage::Format
                    && command.ecosystem == "rust")
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
}

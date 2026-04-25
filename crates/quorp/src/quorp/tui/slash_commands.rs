use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::quorp::benchmark::{self, BenchmarkExecutor};
use crate::quorp::executor::{InteractiveProviderKind, interactive_provider_from_env};
use crate::quorp::tui::agent_context::AutonomyProfile;
use crate::quorp::tui::agent_protocol::AgentMode;
use crate::quorp::tui::agent_runtime::AgentTaskRequest;
use crate::quorp::tui::chat_service::ChatServiceMessage;

pub const DEFAULT_FULL_AUTO_GOAL: &str =
    "Please read START_HERE.md and execute until the visible evaluator passes.";
pub const LAUNCH_SPEC_FILE_NAME: &str = "launch_spec.json";
pub const DEFAULT_FULL_AUTO_STEPS: usize = 40;
pub const DEFAULT_BENCHMARK_STEPS: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlashCommandKind {
    FullAuto,
    Benchmark,
    ResumeLast,
    OpenRunArtifacts,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    FullAuto {
        goal: String,
        sandbox_mode: FullAutoSandboxMode,
    },
    Benchmark {
        path: PathBuf,
        goal: Option<String>,
        sandbox_mode: FullAutoSandboxMode,
    },
    ResumeLast {
        result_dir: Option<PathBuf>,
    },
    OpenRunArtifacts,
    Agent {
        goal: String,
        sandbox_mode: FullAutoSandboxMode,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FullAutoResolvedMode {
    WorkspaceObjective,
    Benchmark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FullAutoSandboxMode {
    LocalCopy,
    Docker,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullAutoLaunchSpec {
    pub kind: SlashCommandKind,
    pub goal: String,
    pub target_path: PathBuf,
    pub workspace_root: PathBuf,
    pub resolved_mode: FullAutoResolvedMode,
    pub sandbox_mode: FullAutoSandboxMode,
    pub docker_image: Option<String>,
    pub autonomy_profile: AutonomyProfile,
    pub max_steps: usize,
    pub max_seconds: Option<u64>,
    pub max_total_tokens: Option<u64>,
    pub result_dir: PathBuf,
    pub objective_file: Option<PathBuf>,
    pub evaluate_command: Option<String>,
    pub objective_metadata: serde_json::Value,
}

#[derive(Debug, Clone, Copy)]
pub struct LaunchDefaults {
    pub autonomy_profile: AutonomyProfile,
    pub max_seconds: Option<u64>,
    pub max_total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub struct CommandDeckEntry {
    pub kind: SlashCommandKind,
    pub template: &'static str,
    pub label: &'static str,
    pub detail: &'static str,
    pub safety_mode: &'static str,
    pub target_scope: &'static str,
    pub expected_outcome: &'static str,
}

const COMMAND_DECK_ENTRIES: &[CommandDeckEntry] = &[
    CommandDeckEntry {
        kind: SlashCommandKind::FullAuto,
        template: "/full-auto Please read START_HERE.md and execute until the visible evaluator passes.",
        label: "/full-auto",
        detail: "Launch a sandboxed autonomous run against the current workspace.",
        safety_mode: "Local copy",
        target_scope: "Current workspace",
        expected_outcome: "Run until visible evaluator or stop reason.",
    },
    CommandDeckEntry {
        kind: SlashCommandKind::FullAuto,
        template: "/full-auto --docker Please read START_HERE.md and execute until the visible evaluator passes.",
        label: "/full-auto --docker",
        detail: "Launch a Docker-backed autonomous run against the current workspace sandbox.",
        safety_mode: "Docker",
        target_scope: "Current workspace",
        expected_outcome: "Run in container while preserving watch artifacts.",
    },
    CommandDeckEntry {
        kind: SlashCommandKind::Benchmark,
        template: "/benchmark ./benchmarks/case Please read START_HERE.md and execute until the visible evaluator passes.",
        label: "/benchmark",
        detail: "Launch a sandboxed benchmark or proof-full workspace run.",
        safety_mode: "Local copy",
        target_scope: "Benchmark path",
        expected_outcome: "Prepare sandbox, run evaluator-backed watch mode.",
    },
    CommandDeckEntry {
        kind: SlashCommandKind::Benchmark,
        template: "/benchmark --docker ./benchmarks/case Please read START_HERE.md and execute until the visible evaluator passes.",
        label: "/benchmark --docker",
        detail: "Launch a Docker-backed benchmark run while keeping local-copy watch artifacts.",
        safety_mode: "Docker",
        target_scope: "Benchmark path",
        expected_outcome: "Run benchmark in container with watch artifacts.",
    },
    CommandDeckEntry {
        kind: SlashCommandKind::ResumeLast,
        template: "/resume-last",
        label: "/resume-last",
        detail: "Resume the latest recorded full-auto launch spec.",
        safety_mode: "Recorded",
        target_scope: "Last run",
        expected_outcome: "Restore the prior launch spec and restart it.",
    },
    CommandDeckEntry {
        kind: SlashCommandKind::OpenRunArtifacts,
        template: "/open-run-artifacts",
        label: "/open-run-artifacts",
        detail: "Show the latest run directory and the key artifact files inside it.",
        safety_mode: "Read-only",
        target_scope: "Latest run",
        expected_outcome: "Summarize artifacts and where they live on disk.",
    },
    CommandDeckEntry {
        kind: SlashCommandKind::Agent,
        template: "/agent Please read START_HERE.md and execute until the visible evaluator passes.",
        label: "/agent",
        detail: "Compatibility alias for /full-auto.",
        safety_mode: "Alias",
        target_scope: "Current workspace",
        expected_outcome: "Same as /full-auto.",
    },
];

impl FullAutoLaunchSpec {
    pub fn write_to_disk(&self) -> anyhow::Result<PathBuf> {
        fs::create_dir_all(&self.result_dir).with_context(|| {
            format!(
                "failed to create run directory {}",
                self.result_dir.display()
            )
        })?;
        let path = self.result_dir.join(LAUNCH_SPEC_FILE_NAME);
        let content = serde_json::to_string_pretty(self)
            .context("failed to serialize full-auto launch spec")?;
        fs::write(&path, content).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(path)
    }

    pub fn load_from(result_dir: &Path) -> anyhow::Result<Self> {
        let path = result_dir.join(LAUNCH_SPEC_FILE_NAME);
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse {}", path.display()))
    }

    pub fn to_agent_task_request(
        &self,
        initial_context: Vec<ChatServiceMessage>,
        model_id: String,
        agent_mode: AgentMode,
        base_url_override: Option<String>,
    ) -> AgentTaskRequest {
        AgentTaskRequest {
            goal: self.goal.clone(),
            initial_context,
            model_id,
            agent_mode,
            base_url_override,
            workspace_root: self.workspace_root.clone(),
            target_path: self.target_path.clone(),
            command_kind: self.kind,
            resolved_mode: self.resolved_mode,
            sandbox_mode: self.sandbox_mode,
            docker_image: self.docker_image.clone(),
            max_iterations: self.max_steps,
            max_seconds: self.max_seconds,
            max_total_tokens: self.max_total_tokens,
            autonomy_profile: self.autonomy_profile,
            result_dir: self.result_dir.clone(),
            objective_file: self.objective_file.clone(),
            evaluate_command: self.evaluate_command.clone(),
            objective_metadata: self.objective_metadata.clone(),
        }
    }
}

pub fn filter_command_deck_entries(query: &str) -> Vec<CommandDeckEntry> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return COMMAND_DECK_ENTRIES.to_vec();
    }
    let normalized = trimmed.trim_start_matches('/').to_ascii_lowercase();
    COMMAND_DECK_ENTRIES
        .iter()
        .copied()
        .filter(|entry| {
            entry.label.trim_start_matches('/').contains(&normalized)
                || entry.detail.to_ascii_lowercase().contains(&normalized)
                || entry
                    .expected_outcome
                    .to_ascii_lowercase()
                    .contains(&normalized)
        })
        .collect()
}

pub fn parse_slash_command(input: &str) -> Result<Option<SlashCommand>, String> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return Ok(None);
    }
    if trimmed.starts_with("/compaction") {
        return Ok(None);
    }
    let parts = trimmed
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let Some(command) = parts.first().map(String::as_str) else {
        return Ok(None);
    };
    let parsed = ParsedSlashArgs::new(&parts[1..]);
    match command {
        "/full-auto" => Ok(Some(SlashCommand::FullAuto {
            goal: normalized_goal(&parsed.trailing.join(" ")),
            sandbox_mode: parsed.sandbox_mode,
        })),
        "/agent" => Ok(Some(SlashCommand::Agent {
            goal: normalized_goal(&parsed.trailing.join(" ")),
            sandbox_mode: parsed.sandbox_mode,
        })),
        "/benchmark" => {
            let Some(path) = parsed.trailing.first() else {
                return Err(
                    "Usage: /benchmark <path> [goal]. Example: /benchmark ./cases/case-1"
                        .to_string(),
                );
            };
            let goal = (parsed.trailing.len() > 1).then(|| parsed.trailing[1..].join(" "));
            Ok(Some(SlashCommand::Benchmark {
                path: PathBuf::from(path),
                goal,
                sandbox_mode: parsed.sandbox_mode,
            }))
        }
        "/resume-last" => Ok(Some(SlashCommand::ResumeLast {
            result_dir: parts.get(1).map(PathBuf::from),
        })),
        "/open-run-artifacts" => Ok(Some(SlashCommand::OpenRunArtifacts)),
        other => Err(format!("Unknown slash command `{other}`.")),
    }
}

pub fn latest_resume_target(explicit: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Some(info) = crate::quorp::run_support::latest_run_dir(Some("full-auto"))? {
        return Ok(info.run_dir);
    }
    if let Some(info) = crate::quorp::run_support::latest_run_dir(None)? {
        return Ok(info.run_dir);
    }
    anyhow::bail!("No previous run directory was found.")
}

pub fn latest_artifact_summary(explicit: Option<&Path>) -> anyhow::Result<String> {
    let run_dir = latest_resume_target(explicit)?;
    let mut lines = vec![format!("Latest run artifacts: {}", run_dir.display())];
    for file_name in [
        "request.json",
        "events.jsonl",
        "transcript.json",
        "summary.json",
        LAUNCH_SPEC_FILE_NAME,
    ] {
        let path = run_dir.join(file_name);
        if path.exists() {
            lines.push(format!("- {}", path.display()));
        }
    }
    let artifacts_dir = run_dir.join("artifacts");
    if artifacts_dir.exists() {
        lines.push(format!("- {}", artifacts_dir.display()));
    }
    Ok(lines.join("\n"))
}

pub fn prepare_launch_spec(
    command: &SlashCommand,
    project_root: &Path,
    model_id: &str,
    defaults: LaunchDefaults,
) -> anyhow::Result<Option<FullAutoLaunchSpec>> {
    match command {
        SlashCommand::OpenRunArtifacts | SlashCommand::ResumeLast { .. } => Ok(None),
        SlashCommand::FullAuto { goal, sandbox_mode }
        | SlashCommand::Agent { goal, sandbox_mode } => {
            let result_dir =
                crate::quorp::run_support::default_run_result_dir(project_root, "full-auto");
            let resolved = crate::quorp::run_support::prepare_workspace_run_sandbox(
                project_root,
                &result_dir,
            )?;
            let result_dir = fs::canonicalize(&result_dir).unwrap_or(result_dir);
            let workspace_root = fs::canonicalize(&resolved.workspace_root)
                .unwrap_or(resolved.workspace_root.clone());
            Ok(Some(FullAutoLaunchSpec {
                kind: match command {
                    SlashCommand::Agent { .. } => SlashCommandKind::Agent,
                    _ => SlashCommandKind::FullAuto,
                },
                goal: normalized_goal(goal),
                target_path: project_root.to_path_buf(),
                workspace_root: workspace_root.clone(),
                resolved_mode: FullAutoResolvedMode::WorkspaceObjective,
                sandbox_mode: *sandbox_mode,
                docker_image: docker_image_for_mode(*sandbox_mode),
                autonomy_profile: defaults.autonomy_profile,
                max_steps: DEFAULT_FULL_AUTO_STEPS,
                max_seconds: defaults.max_seconds,
                max_total_tokens: defaults.max_total_tokens,
                result_dir: result_dir.clone(),
                objective_file: Some(resolved.objective_file.clone()),
                evaluate_command: resolved.evaluate_command.clone(),
                objective_metadata: crate::quorp::run_support::objective_metadata_json(
                    &resolved,
                    &workspace_root,
                ),
            }))
        }
        SlashCommand::Benchmark {
            path,
            goal,
            sandbox_mode,
        } => {
            let target_path = absolutize_path(project_root, path);
            let result_dir =
                crate::quorp::run_support::default_run_result_dir(&target_path, "full-auto");
            let provider = crate::quorp::tui::model_registry::chat_model_provider(
                model_id,
                interactive_provider_from_env(),
            );
            let executor = match provider {
                InteractiveProviderKind::Codex => BenchmarkExecutor::Codex,
                _ => BenchmarkExecutor::Native,
            };
            let prepared = benchmark::prepare_tui_benchmark_launch(
                &target_path,
                &result_dir,
                executor,
                Some(model_id.to_string()),
                None,
                DEFAULT_BENCHMARK_STEPS,
                defaults.max_seconds,
                defaults.max_total_tokens,
            )?;
            let result_dir = fs::canonicalize(&result_dir).unwrap_or(result_dir);
            let workspace_root =
                fs::canonicalize(&prepared.workspace_dir).unwrap_or(prepared.workspace_dir.clone());
            Ok(Some(FullAutoLaunchSpec {
                kind: SlashCommandKind::Benchmark,
                goal: normalized_goal(goal.as_deref().unwrap_or(DEFAULT_FULL_AUTO_GOAL)),
                target_path,
                workspace_root,
                resolved_mode: FullAutoResolvedMode::Benchmark,
                sandbox_mode: *sandbox_mode,
                docker_image: docker_image_for_mode(*sandbox_mode),
                autonomy_profile: defaults.autonomy_profile,
                max_steps: DEFAULT_BENCHMARK_STEPS,
                max_seconds: defaults.max_seconds,
                max_total_tokens: defaults.max_total_tokens,
                result_dir,
                objective_file: Some(prepared.objective_file),
                evaluate_command: prepared.evaluate_command,
                objective_metadata: prepared.objective_metadata,
            }))
        }
    }
}

fn normalized_goal(goal: &str) -> String {
    let trimmed = goal.trim();
    if trimmed.is_empty() {
        DEFAULT_FULL_AUTO_GOAL.to_string()
    } else {
        trimmed.to_string()
    }
}

#[derive(Debug, Clone)]
struct ParsedSlashArgs {
    sandbox_mode: FullAutoSandboxMode,
    trailing: Vec<String>,
}

impl ParsedSlashArgs {
    fn new(parts: &[String]) -> Self {
        let mut sandbox_mode = FullAutoSandboxMode::LocalCopy;
        let mut trailing = Vec::new();
        for part in parts {
            if part == "--docker" {
                sandbox_mode = FullAutoSandboxMode::Docker;
                continue;
            }
            trailing.push(part.clone());
        }
        Self {
            sandbox_mode,
            trailing,
        }
    }
}

fn docker_image_for_mode(sandbox_mode: FullAutoSandboxMode) -> Option<String> {
    if sandbox_mode != FullAutoSandboxMode::Docker {
        return None;
    }
    crate::quorp::docker::DockerArgs::default()
        .enabled()
        .then(|| {
            std::env::var("QUORP_DOCKER_IMAGE").unwrap_or_else(|_| "quorp-runner:dev".to_string())
        })
        .or_else(|| Some("quorp-runner:dev".to_string()))
}

fn absolutize_path(project_root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_full_auto_without_goal_to_default() {
        let command = parse_slash_command("/full-auto")
            .expect("parse")
            .expect("command");
        assert_eq!(
            command,
            SlashCommand::FullAuto {
                goal: DEFAULT_FULL_AUTO_GOAL.to_string(),
                sandbox_mode: FullAutoSandboxMode::LocalCopy,
            }
        );
    }

    #[test]
    fn parses_benchmark_with_path_and_goal() {
        let command = parse_slash_command("/benchmark ./cases/demo fix failing tests")
            .expect("parse")
            .expect("command");
        assert_eq!(
            command,
            SlashCommand::Benchmark {
                path: PathBuf::from("./cases/demo"),
                goal: Some("fix failing tests".to_string()),
                sandbox_mode: FullAutoSandboxMode::LocalCopy,
            }
        );
    }

    #[test]
    fn parses_full_auto_with_docker_flag() {
        let command = parse_slash_command("/full-auto --docker ship it")
            .expect("parse")
            .expect("command");
        assert_eq!(
            command,
            SlashCommand::FullAuto {
                goal: "ship it".to_string(),
                sandbox_mode: FullAutoSandboxMode::Docker,
            }
        );
    }

    #[test]
    fn parses_benchmark_with_docker_flag() {
        let command = parse_slash_command("/benchmark --docker ./cases/demo fix failing tests")
            .expect("parse")
            .expect("command");
        assert_eq!(
            command,
            SlashCommand::Benchmark {
                path: PathBuf::from("./cases/demo"),
                goal: Some("fix failing tests".to_string()),
                sandbox_mode: FullAutoSandboxMode::Docker,
            }
        );
    }

    #[test]
    fn filters_deck_entries_by_query() {
        let entries = filter_command_deck_entries("resume");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, SlashCommandKind::ResumeLast);
    }

    #[test]
    fn writes_and_reads_launch_spec() {
        let temp_dir = tempdir().expect("tempdir");
        let spec = FullAutoLaunchSpec {
            kind: SlashCommandKind::FullAuto,
            goal: "ship it".to_string(),
            target_path: temp_dir.path().to_path_buf(),
            workspace_root: temp_dir.path().join("workspace"),
            resolved_mode: FullAutoResolvedMode::WorkspaceObjective,
            sandbox_mode: FullAutoSandboxMode::LocalCopy,
            docker_image: None,
            autonomy_profile: AutonomyProfile::AutonomousSandboxed,
            max_steps: 40,
            max_seconds: Some(90),
            max_total_tokens: Some(1_000),
            result_dir: temp_dir.path().join("run"),
            objective_file: Some(temp_dir.path().join("workspace").join("START_HERE.md")),
            evaluate_command: Some("./evaluate.sh".to_string()),
            objective_metadata: serde_json::json!({"evaluate_command":"./evaluate.sh"}),
        };
        let path = spec.write_to_disk().expect("write");
        assert!(path.exists());
        let loaded = FullAutoLaunchSpec::load_from(&spec.result_dir).expect("load");
        assert_eq!(loaded.goal, spec.goal);
        assert_eq!(loaded.workspace_root, spec.workspace_root);
    }

    #[test]
    fn writes_and_reads_docker_launch_spec() {
        let temp_dir = tempdir().expect("tempdir");
        let spec = FullAutoLaunchSpec {
            kind: SlashCommandKind::Benchmark,
            goal: "docker run".to_string(),
            target_path: temp_dir.path().join("case-a"),
            workspace_root: temp_dir.path().join("workspace"),
            resolved_mode: FullAutoResolvedMode::Benchmark,
            sandbox_mode: FullAutoSandboxMode::Docker,
            docker_image: Some("quorp-runner:test".to_string()),
            autonomy_profile: AutonomyProfile::AutonomousSandboxed,
            max_steps: 60,
            max_seconds: Some(300),
            max_total_tokens: Some(2_000),
            result_dir: temp_dir.path().join("docker-run"),
            objective_file: Some(temp_dir.path().join("workspace").join("START_HERE.md")),
            evaluate_command: Some("./evaluate.sh".to_string()),
            objective_metadata: serde_json::json!({"evaluate_command":"./evaluate.sh"}),
        };
        spec.write_to_disk().expect("write");
        let loaded = FullAutoLaunchSpec::load_from(&spec.result_dir).expect("load");
        assert_eq!(loaded.sandbox_mode, FullAutoSandboxMode::Docker);
        assert_eq!(loaded.docker_image.as_deref(), Some("quorp-runner:test"));
    }
}

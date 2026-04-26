use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedBenchmark {
    pub benchmark_root: PathBuf,
    pub issue_id: String,
    pub benchmark_name: String,
    pub issue_dir: Option<PathBuf>,
    pub workspace_source: PathBuf,
    pub objective_source: PathBuf,
    pub visible_evaluator: Option<PathBuf>,
    pub collector_evaluator: Option<PathBuf>,
    pub context_files: Vec<PathBuf>,
    pub repair_artifacts: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChallengeManifest {
    pub id: String,
    pub title: String,
    pub difficulty: String,
    pub category: String,
    pub repo_condition: Vec<String>,
    pub objective_file: String,
    pub success_file: String,
    pub reset_command: String,
    pub evaluate_command: String,
    pub estimated_minutes: Option<u64>,
    pub expected_files_touched: Vec<String>,
    #[serde(default)]
    pub allowed_generated_files: Vec<String>,
    pub primary_metrics: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChallengeCapsule {
    #[serde(default)]
    pub case_class: String,
    #[serde(default)]
    pub owner_files: Vec<String>,
    #[serde(default)]
    pub first_reads: Vec<String>,
    #[serde(default)]
    pub fast_loop_commands: Vec<String>,
    #[serde(default)]
    pub expected_touch_targets: Vec<String>,
    #[serde(default)]
    pub companion_files_required: Vec<String>,
    #[serde(default)]
    pub strong_hints: Vec<String>,
    #[serde(default)]
    pub watch_points: Vec<String>,
    #[serde(default)]
    pub named_tests: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChallengeMetadata {
    pub case_root: PathBuf,
    pub sandbox_root: PathBuf,
    pub workspace_dir: PathBuf,
    pub condition: String,
    pub objective_file: PathBuf,
    pub success_file: PathBuf,
    #[serde(default)]
    pub reference_file: Option<PathBuf>,
    pub reset_command: String,
    pub evaluate_command: String,
    pub expected_files_touched: Vec<String>,
    #[serde(default)]
    pub allowed_generated_files: Vec<String>,
    pub primary_metrics: Vec<String>,
    pub tags: Vec<String>,
    pub capsule_file: PathBuf,
    #[serde(default)]
    pub capsule: ChallengeCapsule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedChallengeCase {
    pub case_root: PathBuf,
    pub manifest: ChallengeManifest,
    pub condition: String,
    pub objective_source: PathBuf,
    pub success_source: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
struct WarposBenchmarkRootMarker {
    benchmark: Option<String>,
    issue: String,
    #[allow(dead_code)]
    condition: Option<String>,
    #[allow(dead_code)]
    suite: Option<String>,
    handoff_root: PathBuf,
}

pub fn resolve_benchmark(path: &Path) -> anyhow::Result<ResolvedBenchmark> {
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("failed to resolve benchmark path {}", path.display()))?;
    if looks_like_warpos_staged_workspace(&canonical) {
        return resolve_from_warpos_staged_workspace(&canonical);
    }
    if looks_like_proof_full_workspace(&canonical) {
        return resolve_from_workspace_root(&canonical);
    }
    if looks_like_issue_dir(&canonical) {
        return resolve_from_issue_dir(&canonical);
    }
    anyhow::bail!(
        "benchmark path `{}` was not recognized as an issue brief directory or proof-full workspace root",
        canonical.display()
    );
}

pub fn resolve_challenge_case(
    path: &Path,
    explicit_condition: Option<&str>,
) -> anyhow::Result<Option<ResolvedChallengeCase>> {
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("failed to resolve challenge path {}", path.display()))?;
    let Some(case_root) = find_ancestor_with_file(&canonical, "benchmark.json") else {
        return Ok(None);
    };
    let manifest_path = case_root.join("benchmark.json");
    let manifest: ChallengeManifest = serde_json::from_str(
        &fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    let condition =
        resolve_challenge_condition(&canonical, &case_root, &manifest, explicit_condition)?;
    let declared_objective = case_root.join(&manifest.objective_file);
    let objective_source = if canonical == declared_objective {
        canonical.clone()
    } else if canonical.is_dir()
        || canonical.starts_with(case_root.join("workspace"))
        || looks_like_proof_full_workspace(&case_root)
    {
        declared_objective.clone()
    } else if canonical.starts_with(&case_root) {
        anyhow::bail!(
            "provided challenge path {} does not match the declared objective file {}; pass the case root, the objective markdown, or a workspace file",
            canonical.display(),
            declared_objective.display()
        );
    } else {
        declared_objective.clone()
    };
    if !objective_source.exists() {
        anyhow::bail!(
            "failed to locate challenge objective file at {}",
            objective_source.display()
        );
    }
    let success_source = case_root.join(&manifest.success_file);
    if !success_source.exists() {
        anyhow::bail!(
            "failed to locate challenge success file at {}",
            success_source.display()
        );
    }
    Ok(Some(ResolvedChallengeCase {
        case_root,
        manifest,
        condition,
        objective_source,
        success_source,
    }))
}

pub fn looks_like_proof_full_workspace(path: &Path) -> bool {
    path.join("AGENTS.md").exists()
        && path.join("agent-map.json").exists()
        && path.join("test-map.json").exists()
}

pub fn looks_like_warpos_staged_workspace(path: &Path) -> bool {
    path.join(".benchmark-root.json").exists()
        && path.join("issue.json").exists()
        && path.join("Cargo.toml").exists()
        && path.join("evaluate.sh").exists()
        && (path.join("START_HERE.md").exists() || path.join("README.md").exists())
}

pub fn looks_like_issue_dir(path: &Path) -> bool {
    path.join("README.md").exists()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("ISSUE-"))
}

pub fn collect_context_files(workspace_root: &Path) -> Vec<PathBuf> {
    [
        workspace_root.join(".benchmark-root.json"),
        workspace_root.join("issue.json"),
        workspace_root.join("START_HERE.md"),
        workspace_root.join("YOU_ARE_HERE.txt"),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .collect()
}

pub fn collect_repair_artifacts(workspace_root: &Path) -> Vec<PathBuf> {
    [
        workspace_root
            .join("target")
            .join("agent")
            .join("repair-bundle.json"),
        workspace_root
            .join("target")
            .join("agent")
            .join("last-failure.json"),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .collect()
}

fn resolve_challenge_condition(
    canonical: &Path,
    case_root: &Path,
    manifest: &ChallengeManifest,
    explicit_condition: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(explicit) = explicit_condition {
        if manifest
            .repo_condition
            .iter()
            .any(|condition| condition == explicit)
        {
            return Ok(explicit.to_string());
        }
        anyhow::bail!(
            "challenge condition `{}` is not listed in benchmark.json repo_condition",
            explicit
        );
    }

    if let Some(inferred) = infer_condition_from_workspace_path(canonical, case_root, manifest) {
        return Ok(inferred);
    }

    if manifest
        .repo_condition
        .iter()
        .any(|condition| condition == "proof-full")
    {
        return Ok("proof-full".to_string());
    }

    manifest
        .repo_condition
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("benchmark.json did not list any repo_condition values"))
}

fn infer_condition_from_workspace_path(
    canonical: &Path,
    case_root: &Path,
    manifest: &ChallengeManifest,
) -> Option<String> {
    let workspace_root = case_root.join("workspace");
    if !canonical.starts_with(&workspace_root) {
        return None;
    }
    let relative = canonical.strip_prefix(&workspace_root).ok()?;
    let inferred = relative
        .components()
        .next()?
        .as_os_str()
        .to_str()?
        .to_string();
    manifest
        .repo_condition
        .iter()
        .any(|condition| condition == &inferred)
        .then_some(inferred)
}

fn find_ancestor_with_file(path: &Path, file_name: &str) -> Option<PathBuf> {
    for ancestor in path.ancestors() {
        if ancestor.join(file_name).exists() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn resolve_from_warpos_staged_workspace(
    workspace_root: &Path,
) -> anyhow::Result<ResolvedBenchmark> {
    let marker = read_warpos_benchmark_root_marker(workspace_root)?;
    let handoff_root = resolve_marker_handoff_root(workspace_root, &marker);
    let benchmark_root = find_warpos_benchmarks_root(&handoff_root).unwrap_or(handoff_root.clone());
    let issue_dir = find_warpos_issue_dir(&benchmark_root, &marker.issue);
    Ok(ResolvedBenchmark {
        benchmark_root,
        issue_id: marker.issue.clone(),
        benchmark_name: marker
            .benchmark
            .clone()
            .unwrap_or_else(|| marker.issue.clone()),
        issue_dir: issue_dir.clone(),
        workspace_source: workspace_root.to_path_buf(),
        objective_source: [
            workspace_root.join("START_HERE.md"),
            workspace_root.join("README.md"),
        ]
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| anyhow::anyhow!("failed to locate benchmark objective file"))?,
        visible_evaluator: [
            workspace_root.join("evaluate.sh"),
            workspace_root.join("evaluate_visible.sh"),
        ]
        .into_iter()
        .find(|path| path.exists()),
        collector_evaluator: issue_dir
            .as_ref()
            .and_then(|path| find_collector_script(path)),
        context_files: collect_context_files(workspace_root),
        repair_artifacts: collect_repair_artifacts(workspace_root),
    })
}

fn read_warpos_benchmark_root_marker(
    workspace_root: &Path,
) -> anyhow::Result<WarposBenchmarkRootMarker> {
    let marker_path = workspace_root.join(".benchmark-root.json");
    serde_json::from_str::<WarposBenchmarkRootMarker>(
        &fs::read_to_string(&marker_path)
            .with_context(|| format!("failed to read {}", marker_path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", marker_path.display()))
}

fn resolve_marker_handoff_root(
    workspace_root: &Path,
    marker: &WarposBenchmarkRootMarker,
) -> PathBuf {
    let handoff_root = if marker.handoff_root.is_absolute() {
        marker.handoff_root.clone()
    } else {
        workspace_root.join(&marker.handoff_root)
    };
    fs::canonicalize(&handoff_root).unwrap_or(handoff_root)
}

fn find_warpos_benchmarks_root(path: &Path) -> Option<PathBuf> {
    path.ancestors().find_map(|ancestor| {
        (ancestor.file_name().and_then(|name| name.to_str()) == Some("benchmarks"))
            .then(|| ancestor.to_path_buf())
    })
}

fn find_warpos_issue_dir(benchmarks_root: &Path, issue_id: &str) -> Option<PathBuf> {
    [
        benchmarks_root.join("issues").join(issue_id),
        benchmarks_root
            .join("exhaustive")
            .join("issues")
            .join(issue_id),
    ]
    .into_iter()
    .find(|path| path.exists())
}

fn resolve_from_workspace_root(workspace_root: &Path) -> anyhow::Result<ResolvedBenchmark> {
    let issue_id = workspace_root
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            anyhow::anyhow!("failed to infer issue id from {}", workspace_root.display())
        })?
        .to_string();
    let benchmark_root = find_benchmark_root(workspace_root)?;
    let issue_dir = benchmark_root
        .join("exhaustive")
        .join("issues")
        .join(&issue_id);
    let issue_dir = issue_dir.exists().then_some(issue_dir);
    Ok(ResolvedBenchmark {
        benchmark_root: benchmark_root.clone(),
        issue_id: issue_id.clone(),
        benchmark_name: issue_id.clone(),
        issue_dir: issue_dir.clone(),
        workspace_source: workspace_root.to_path_buf(),
        objective_source: issue_dir
            .as_ref()
            .map(|dir| dir.join("README.md"))
            .filter(|path| path.exists())
            .or_else(|| {
                [
                    workspace_root.join("START_HERE.md"),
                    workspace_root.join("README.md"),
                ]
                .into_iter()
                .find(|path| path.exists())
            })
            .ok_or_else(|| anyhow::anyhow!("failed to locate benchmark objective file"))?,
        visible_evaluator: [
            workspace_root.join("evaluate.sh"),
            workspace_root.join("evaluate_visible.sh"),
        ]
        .into_iter()
        .find(|path| path.exists()),
        collector_evaluator: issue_dir
            .as_ref()
            .and_then(|path| find_collector_script(path)),
        context_files: collect_context_files(workspace_root),
        repair_artifacts: collect_repair_artifacts(workspace_root),
    })
}

fn resolve_from_issue_dir(issue_dir: &Path) -> anyhow::Result<ResolvedBenchmark> {
    let issue_id = issue_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("failed to infer issue id from {}", issue_dir.display()))?
        .to_string();
    let benchmark_root = find_benchmark_root(issue_dir)?;
    let handoffs_root = benchmark_root.join("handoffs");
    let workspace_source =
        find_workspace_for_issue(&handoffs_root, &issue_id)?.ok_or_else(|| {
            anyhow::anyhow!(
                "failed to find proof-full workspace for issue `{}` under {}",
                issue_id,
                handoffs_root.display()
            )
        })?;
    Ok(ResolvedBenchmark {
        benchmark_root,
        issue_id: issue_id.clone(),
        benchmark_name: issue_id.clone(),
        issue_dir: Some(issue_dir.to_path_buf()),
        workspace_source: workspace_source.clone(),
        objective_source: issue_dir.join("README.md"),
        visible_evaluator: [
            workspace_source.join("evaluate.sh"),
            workspace_source.join("evaluate_visible.sh"),
        ]
        .into_iter()
        .find(|path| path.exists()),
        collector_evaluator: find_collector_script(issue_dir),
        context_files: collect_context_files(&workspace_source),
        repair_artifacts: collect_repair_artifacts(&workspace_source),
    })
}

fn find_collector_script(issue_dir: &Path) -> Option<PathBuf> {
    [
        issue_dir.join("evaluate.sh"),
        issue_dir.join(".hidden").join("evaluate_hidden.sh"),
        issue_dir.join("hidden").join("check.sh"),
    ]
    .into_iter()
    .find(|path| path.exists())
}

fn find_workspace_for_issue(
    handoffs_root: &Path,
    issue_id: &str,
) -> anyhow::Result<Option<PathBuf>> {
    if !handoffs_root.exists() {
        return Ok(None);
    }
    for entry in fs::read_dir(handoffs_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let candidate = entry.path().join(issue_id).join("proof-full");
        if candidate.exists() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn find_benchmark_root(path: &Path) -> anyhow::Result<PathBuf> {
    for ancestor in path.ancestors() {
        if ancestor.file_name().and_then(|name| name.to_str()) == Some("benchmark") {
            return Ok(ancestor.to_path_buf());
        }
    }
    anyhow::bail!(
        "failed to find enclosing `benchmark` directory for {}",
        path.display()
    )
}

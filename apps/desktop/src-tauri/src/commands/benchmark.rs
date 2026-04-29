//! Benchmark library: surface fixtures from
//! `benchmark/challenges/rust-swebench-top5/*` and let the desktop
//! launch them under the apple sandbox.
//!
//! v1 supports the same demo-run semantics as `start_agent_run`. PR6
//! wires the apple-sandbox lifecycle so a benchmark run actually
//! confines tooling to `/private/tmp/quorp/<run-id>/work/`.

use std::path::{Path, PathBuf};

use quorp_desktop_ipc::{
    BenchmarkFixture, BenchmarkOptions, DesktopEvent, IpcError, IpcErrorCode, RunHandle,
    StartRunRequest, WorkspaceId,
};
use tokio::sync::mpsc;

use crate::state::AppHandleState;

/// Walk `benchmark/challenges/rust-swebench-top5/*` from the active
/// workspace's repo root (or the running app's cwd) and return one
/// `BenchmarkFixture` per fixture directory that has a `benchmark.json`.
#[tauri::command]
pub fn list_benchmark_fixtures(
    state: tauri::State<'_, AppHandleState>,
) -> Result<Vec<BenchmarkFixture>, IpcError> {
    let _ = state;
    let root = match resolve_repo_root() {
        Some(root) => root,
        None => return Ok(Vec::new()),
    };
    let challenges = root
        .join("benchmark")
        .join("challenges")
        .join("rust-swebench-top5");
    if !challenges.is_dir() {
        return Ok(Vec::new());
    }
    let mut fixtures = Vec::new();
    let entries = match std::fs::read_dir(&challenges) {
        Ok(entries) => entries,
        Err(err) => {
            return Err(IpcError::new(
                IpcErrorCode::FilesystemError,
                err.to_string(),
            ));
        }
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir = entry.path();
        let benchmark_json = dir.join("benchmark.json");
        if !benchmark_json.exists() {
            continue;
        }
        let workspace_path = dir.join("upstream").join("workspace");
        if !workspace_path.is_dir() {
            continue;
        }
        let proof_dir = dir.join("proof-full");
        let has_reference_proof = proof_dir.is_dir();
        fixtures.push(BenchmarkFixture {
            fixture_id: dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            set: "rust-swebench-top5".to_string(),
            display_name: dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            description: read_first_line(&dir.join("README.md"))
                .unwrap_or_else(|| "(no README.md)".to_string()),
            workspace_path: workspace_path.display().to_string(),
            reference_proof_path: has_reference_proof
                .then(|| proof_dir.display().to_string()),
            has_reference_proof,
        });
    }
    fixtures.sort_by(|a, b| a.fixture_id.cmp(&b.fixture_id));
    Ok(fixtures)
}

/// Start a benchmark run. The benchmark fixture's `upstream/workspace`
/// is added to the workspace registry (if not already) and a
/// [`StartRunRequest`] is composed with the user's options. The actual
/// confinement (apple sandbox + `/tmp/quorp/<run-id>/work/`) lands in
/// PR6.
#[tauri::command]
pub async fn start_benchmark_run(
    state: tauri::State<'_, AppHandleState>,
    options: BenchmarkOptions,
    on_event: tauri::ipc::Channel<DesktopEvent>,
) -> Result<RunHandle, IpcError> {
    let fixtures = list_benchmark_fixtures(state.clone())?;
    let fixture = fixtures
        .into_iter()
        .find(|f| f.fixture_id == options.fixture_id)
        .ok_or_else(|| {
            IpcError::new(
                IpcErrorCode::InvalidInput,
                format!("unknown benchmark fixture: {}", options.fixture_id),
            )
        })?;
    let workspace = state
        .core
        .workspaces
        .add(Path::new(&fixture.workspace_path))
        .map_err(|err| IpcError::new(IpcErrorCode::FilesystemError, err.to_string()))?;
    let workspace_id = WorkspaceId::new(workspace.id.0.clone());

    let request = StartRunRequest {
        workspace_id,
        goal: format!("Benchmark: {}", fixture.fixture_id),
        permission_mode: options.permission_mode,
        sandbox_mode: options.sandbox_mode,
        model_id: options.model_id,
        wall_clock_budget_seconds: options.wall_clock_budget_seconds,
    };

    let sanitized = state
        .core
        .runs
        .sanitize_request(&state.core.workspaces, request)
        .map_err(IpcError::from)?;

    let (forward_tx, mut forward_rx) = mpsc::unbounded_channel::<DesktopEvent>();
    let handle = state
        .core
        .runs
        .start_demo_run(sanitized, forward_tx)
        .map_err(IpcError::from)?;

    let runtime = state.core.runtime.clone();
    runtime.spawn(async move {
        while let Some(event) = forward_rx.recv().await {
            if on_event.send(event).is_err() {
                break;
            }
        }
    });

    Ok(handle)
}

fn read_first_line(path: &Path) -> Option<String> {
    let body = std::fs::read_to_string(path).ok()?;
    body.lines()
        .find(|line| !line.trim().is_empty())
        .map(|s| s.trim().to_string())
}

/// Best-effort: walk parents from the app's CWD looking for a
/// `benchmark/` directory. Works for `pnpm tauri dev` from the repo
/// and for running the bundled app as long as the user launched it
/// from a clone (e.g. via `quorp app .`). When absent the benchmark
/// list is empty and the UI surfaces a friendly message.
fn resolve_repo_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    for ancestor in cwd.ancestors() {
        if ancestor.join("benchmark").join("challenges").is_dir() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

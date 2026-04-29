//! Manages the lifecycle of desktop-driven runs.
//!
//! Holds the active-runs registry, applies trust-aware sandbox
//! sanitization, and exposes two run-driving entry points:
//! [`RunService::start_demo_run`] (synthetic event stream, used when
//! no API key is configured) and [`RunService::start_real_run`]
//! (drives `quorp_session::quorp::agent_runner::run_headless_agent_with_hooks`
//! on the desktop's dedicated tokio runtime).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use parking_lot::RwLock;
use tokio::sync::mpsc::UnboundedSender;

use quorp_desktop_ipc::{
    DesktopEvent, IpcError, IpcErrorCode, PermissionModeDto, RunFailureStage, RunHandle, RunIdDto,
    RunPhaseDto, RunStatusDto, RuntimeEventDto, SandboxModeDto, StartRunRequest, StopReasonDto,
    TrustDecision, WorkspaceId,
};
use quorp_session::quorp::agent_runner::{
    HeadlessRunHooks, HeadlessRunOptions, run_headless_agent_with_hooks,
};

use crate::event_bridge::DesktopRuntimeSink;
use crate::provider_registry::ProviderRegistry;
use crate::secret_keychain::SecretStore;
use crate::workspace_registry::WorkspaceRegistry;
use crate::{DESKTOP_CORE_VERSION, event_bridge};

/// Errors returned from run-service operations.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("workspace not found: {0}")]
    WorkspaceNotFound(WorkspaceId),
    #[error("workspace must be trusted to run in mode `{0:?}`")]
    TrustRequired(PermissionModeDto),
    #[error("run not found: {0}")]
    NotFound(RunIdDto),
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<RunError> for IpcError {
    fn from(err: RunError) -> Self {
        let code = match &err {
            RunError::WorkspaceNotFound(_) => IpcErrorCode::WorkspaceNotFound,
            RunError::TrustRequired(_) => IpcErrorCode::TrustRequired,
            RunError::NotFound(_) => IpcErrorCode::RunNotFound,
            RunError::Internal(_) => IpcErrorCode::Internal,
        };
        IpcError::new(code, err.to_string())
    }
}

/// Public registration entry per active run.
#[derive(Debug)]
struct ActiveRun {
    handle: RunHandle,
    workspace_id: WorkspaceId,
    permission_mode: PermissionModeDto,
    sandbox_mode: SandboxModeDto,
    model_id: String,
    cancellation_flag: Arc<AtomicBool>,
    /// Phase carries the latest snapshot for `status()`.
    phase: RwLock<RunPhaseDto>,
    started_at: String,
    finished_at: RwLock<Option<String>>,
    stop_reason: RwLock<Option<StopReasonDto>>,
}

/// Service holding the active-runs map and the sanitization rules.
#[derive(Debug, Default)]
pub struct RunService {
    runs: RwLock<HashMap<RunIdDto, Arc<ActiveRun>>>,
}

impl RunService {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply trust-aware policies to a request before spawning a run.
    /// Returns the sanitized request or a [`RunError::TrustRequired`]
    /// when the user has chosen a mode the workspace is not allowed
    /// to use.
    ///
    /// Rules (matching the plan):
    /// - Untrusted workspaces cannot escalate beyond `Ask` mode; any
    ///   stronger mode is rejected.
    /// - Untrusted workspaces cannot use the `Host` sandbox; we
    ///   silently rewrite to `MacAppleSandbox` (or `TmpCopy` on non-
    ///   macOS hosts).
    pub fn sanitize_request(
        &self,
        registry: &WorkspaceRegistry,
        mut request: StartRunRequest,
    ) -> Result<StartRunRequest, RunError> {
        let workspace = registry
            .get(&request.workspace_id)
            .ok_or_else(|| RunError::WorkspaceNotFound(request.workspace_id.clone()))?;

        if matches!(workspace.trust, TrustDecision::Untrusted) {
            // Reject permission modes that would let the agent escape
            // the user's review without an explicit trust grant.
            match request.permission_mode {
                PermissionModeDto::ReadOnly | PermissionModeDto::Ask => {}
                other => return Err(RunError::TrustRequired(other)),
            }
            // Rewrite Host -> safest available confined backend.
            if request.sandbox_mode == SandboxModeDto::Host {
                request.sandbox_mode = if cfg!(target_os = "macos") {
                    SandboxModeDto::MacAppleSandbox
                } else {
                    SandboxModeDto::TmpCopy
                };
            }
        }
        Ok(request)
    }

    /// Cooperatively cancel the named run. Idempotent: cancelling an
    /// already-finished run is a no-op.
    pub fn cancel_run(&self, run_id: &RunIdDto) -> Result<(), RunError> {
        let runs = self.runs.read();
        let active = runs
            .get(run_id)
            .ok_or_else(|| RunError::NotFound(run_id.clone()))?;
        active.cancellation_flag.store(true, Ordering::SeqCst);
        *active.phase.write() = RunPhaseDto::Cancelling;
        Ok(())
    }

    pub fn status(&self, run_id: &RunIdDto) -> Option<RunStatusDto> {
        let runs = self.runs.read();
        let active = runs.get(run_id)?;
        Some(RunStatusDto {
            run_id: run_id.clone(),
            workspace_id: active.workspace_id.clone(),
            phase: *active.phase.read(),
            permission_mode: active.permission_mode,
            sandbox_mode: active.sandbox_mode,
            model_id: active.model_id.clone(),
            current_step: 0,
            total_billed_tokens: 0,
            context_pressure: None,
            started_at: active.started_at.clone(),
            finished_at: active.finished_at.read().clone(),
            stop_reason: *active.stop_reason.read(),
        })
    }

    pub fn active_handles(&self) -> Vec<RunHandle> {
        self.runs
            .read()
            .values()
            .map(|run| run.handle.clone())
            .collect()
    }

    /// Demo run: registers a new run id, emits a tiny synthetic event
    /// stream on `sink` (RunStarted -> two Runtime batches -> RunFinished),
    /// and resolves the active-runs entry. Used in PR4 to verify the
    /// channel pipeline end-to-end without depending on the agent
    /// runtime. PR5 replaces this with the real
    /// `run_headless_agent_with_hooks` invocation under the apple
    /// sandbox.
    pub fn start_demo_run(
        self: &Arc<Self>,
        request: StartRunRequest,
        sink: UnboundedSender<DesktopEvent>,
    ) -> Result<RunHandle, RunError> {
        let model_id = request
            .model_id
            .clone()
            .unwrap_or_else(|| quorp_desktop_ipc::DEFAULT_MODEL_ID.to_string());
        let run_id = RunIdDto::new(format!("run-{}", Utc::now().format("%Y%m%dT%H%M%S%3f")));
        let started_at = Utc::now().to_rfc3339();
        let handle = RunHandle {
            run_id: run_id.clone(),
            started_at: started_at.clone(),
        };
        let active = Arc::new(ActiveRun {
            handle: handle.clone(),
            workspace_id: request.workspace_id.clone(),
            permission_mode: request.permission_mode,
            sandbox_mode: request.sandbox_mode,
            model_id: model_id.clone(),
            cancellation_flag: Arc::new(AtomicBool::new(false)),
            phase: RwLock::new(RunPhaseDto::Starting),
            started_at: started_at.clone(),
            finished_at: RwLock::new(None),
            stop_reason: RwLock::new(None),
        });
        self.runs.write().insert(run_id.clone(), active.clone());

        let sink_for_task = sink.clone();
        let service_for_task = self.clone();
        let model_for_task = model_id.clone();
        let run_id_for_task = run_id.clone();

        tokio::spawn(async move {
            let _ = service_for_task
                .drive_demo(active, sink_for_task, model_for_task, run_id_for_task)
                .await;
        });
        Ok(handle)
    }

    /// Mark a run finished. Internal; kept on the type so other
    /// run-driving code paths can share the bookkeeping.
    fn mark_finished(&self, run_id: &RunIdDto, reason: StopReasonDto) {
        if let Some(active) = self.runs.read().get(run_id).cloned() {
            *active.phase.write() = if matches!(reason, StopReasonDto::Cancelled) {
                RunPhaseDto::Cancelling
            } else {
                RunPhaseDto::Finished
            };
            *active.finished_at.write() = Some(Utc::now().to_rfc3339());
            *active.stop_reason.write() = Some(reason);
        }
    }

    async fn drive_demo(
        &self,
        active: Arc<ActiveRun>,
        sink: UnboundedSender<DesktopEvent>,
        model_id: String,
        run_id: RunIdDto,
    ) {
        if sink
            .send(event_bridge::run_started_event(
                run_id.clone(),
                "demo run".to_string(),
                model_id.clone(),
            ))
            .is_err()
        {
            self.mark_finished(&run_id, StopReasonDto::UnknownError);
            return;
        }
        *active.phase.write() = RunPhaseDto::Running;

        for batch_seq in 0..2u64 {
            if active.cancellation_flag.load(Ordering::SeqCst) {
                self.mark_finished(&run_id, StopReasonDto::Cancelled);
                let _ = sink.send(DesktopEvent::RunFinished {
                    run_id: run_id.clone(),
                    stop_reason: StopReasonDto::Cancelled,
                    total_steps: batch_seq as usize,
                    total_billed_tokens: 0,
                    duration_ms: 0,
                });
                return;
            }
            let batch = vec![
                RuntimeEventDto::PhaseChanged {
                    seq: batch_seq * 2,
                    phase: format!("demo-batch-{batch_seq}"),
                    detail: Some(format!("desktop_core {DESKTOP_CORE_VERSION}")),
                },
                RuntimeEventDto::AssistantTurnSummary {
                    seq: batch_seq * 2 + 1,
                    step: batch_seq as usize + 1,
                    assistant_message: format!("synthetic message {batch_seq}"),
                    actions: vec![],
                    wrote_files: false,
                    validation_queued: false,
                    parse_warning_count: 0,
                },
            ];
            if sink
                .send(DesktopEvent::Runtime {
                    run_id: run_id.clone(),
                    batch,
                    batch_seq,
                })
                .is_err()
            {
                self.mark_finished(&run_id, StopReasonDto::UnknownError);
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.mark_finished(&run_id, StopReasonDto::Completed);
        let _ = sink.send(DesktopEvent::RunFinished {
            run_id: run_id.clone(),
            stop_reason: StopReasonDto::Completed,
            total_steps: 2,
            total_billed_tokens: 0,
            duration_ms: 0,
        });
    }
}

/// Build a synthetic `RunFailed` event the run service emits when its
/// own setup fails before the agent loop starts.
pub fn run_failed_event(run_id: RunIdDto, error: String, stage: RunFailureStage) -> DesktopEvent {
    DesktopEvent::RunFailed {
        run_id,
        error,
        stage,
    }
}

/// Options for [`RunService::start_real_run`]. Built from a sanitized
/// [`StartRunRequest`], the workspace's canonical path, and any
/// agent-level overrides the desktop wants to apply.
#[derive(Debug, Clone)]
pub struct RealRunOptions {
    pub workspace_root: PathBuf,
    pub goal: String,
    pub model_id: String,
    pub permission_mode: PermissionModeDto,
    pub sandbox_mode: SandboxModeDto,
    pub max_steps: usize,
    pub max_seconds: Option<u64>,
}

impl RealRunOptions {
    /// Default ceilings for a desktop-driven run: 64 agent turns, no
    /// hard wall-clock cap. Override per call when the user picks a
    /// custom budget.
    pub fn defaults_for(
        workspace_root: PathBuf,
        goal: String,
        model_id: String,
        permission_mode: PermissionModeDto,
        sandbox_mode: SandboxModeDto,
    ) -> Self {
        Self {
            workspace_root,
            goal,
            model_id,
            permission_mode,
            sandbox_mode,
            max_steps: 64,
            max_seconds: None,
        }
    }
}

impl RunService {
    /// Drive `quorp_session::quorp::agent_runner::run_headless_agent_with_hooks`
    /// against a real provider and stream every emitted RuntimeEvent
    /// through `sink` as `DesktopEvent::Runtime` batches.
    ///
    /// The function returns immediately with a [`RunHandle`]; the run
    /// itself runs on the desktop's tokio runtime via `spawn_blocking`
    /// (the underlying agent_runner builds its own per-call tokio
    /// runtime, which is incompatible with running directly inside
    /// our async context).
    ///
    /// Pre-conditions:
    /// - `provider_registry` must have `has_api_key() == true`.
    /// - `workspace_root` must exist and be readable.
    ///
    /// On the spawning thread:
    /// 1. Read the NIM API key from the keychain.
    /// 2. Set `NVIDIA_API_KEY` (the env var quorp_session reads).
    ///    Best-effort serialization with the OS env table is OK
    ///    because the key value is identical across concurrent runs.
    /// 3. Materialize the goal as `<run_dir>/objective.md` so
    ///    `run_headless_agent_with_hooks` can read it.
    /// 4. Spawn the agent under `spawn_blocking` and await completion.
    /// 5. On terminal (success or failure) emit `RunFinished` /
    ///    `RunFailed` and clear the active-runs entry.
    pub fn start_real_run(
        self: &Arc<Self>,
        options: RealRunOptions,
        provider_registry: Arc<ProviderRegistry>,
        secret_store: Arc<dyn SecretStore>,
        runtime: Arc<tokio::runtime::Runtime>,
        sink: UnboundedSender<DesktopEvent>,
    ) -> Result<RunHandle, RunError> {
        if !provider_registry.has_api_key() {
            return Err(RunError::Internal(
                "real run requires a NIM API key in keychain".to_string(),
            ));
        }
        if !options.workspace_root.exists() {
            return Err(RunError::Internal(format!(
                "workspace root missing: {}",
                options.workspace_root.display()
            )));
        }

        let run_id = RunIdDto::new(format!("run-{}", Utc::now().format("%Y%m%dT%H%M%S%3f")));
        let started_at = Utc::now().to_rfc3339();
        let handle = RunHandle {
            run_id: run_id.clone(),
            started_at: started_at.clone(),
        };
        let cancellation_flag = Arc::new(AtomicBool::new(false));
        let active = Arc::new(ActiveRun {
            handle: handle.clone(),
            workspace_id: WorkspaceId::new(options.workspace_root.display().to_string()),
            permission_mode: options.permission_mode,
            sandbox_mode: options.sandbox_mode,
            model_id: options.model_id.clone(),
            cancellation_flag: cancellation_flag.clone(),
            phase: RwLock::new(RunPhaseDto::Starting),
            started_at: started_at.clone(),
            finished_at: RwLock::new(None),
            stop_reason: RwLock::new(None),
        });
        self.runs.write().insert(run_id.clone(), active.clone());

        let service_self = self.clone();
        let run_id_clone = run_id.clone();
        let model_for_started = options.model_id.clone();
        let goal_for_started = options.goal.clone();
        let sink_for_task = sink.clone();
        let runtime_for_drainer = runtime.clone();

        runtime.spawn(async move {
            // Emit RunStarted immediately on the run channel so the
            // UI flips to the active run before any agent work
            // begins.
            let _ = sink_for_task.send(event_bridge::run_started_event(
                run_id_clone.clone(),
                goal_for_started,
                model_for_started,
            ));
            *active.phase.write() = RunPhaseDto::Running;

            // Build the bridge pipe: agent -> RuntimeEvent ->
            // (translate) -> DesktopEvent::Runtime batches -> sink.
            let (rt_event_tx, mut rt_event_rx) =
                tokio::sync::mpsc::unbounded_channel::<quorp_agent_core::RuntimeEvent>();
            let drainer_sink = sink_for_task.clone();
            let drainer_run_id = run_id_clone.clone();
            let drainer_handle = runtime_for_drainer.spawn(async move {
                let mut batch_seq: u64 = 0;
                let mut buffer: Vec<RuntimeEventDto> = Vec::with_capacity(64);
                let mut next_seq: u64 = 0;
                loop {
                    let recv = rt_event_rx.recv().await;
                    match recv {
                        Some(event) => {
                            buffer.push(event_bridge::translate(next_seq, event));
                            next_seq += 1;
                            // Greedily drain anything else queued so
                            // bursts get coalesced into a single batch.
                            while let Ok(event) = rt_event_rx.try_recv() {
                                buffer.push(event_bridge::translate(next_seq, event));
                                next_seq += 1;
                                if buffer.len() >= 128 {
                                    break;
                                }
                            }
                            let _ = drainer_sink.send(DesktopEvent::Runtime {
                                run_id: drainer_run_id.clone(),
                                batch: std::mem::take(&mut buffer),
                                batch_seq,
                            });
                            batch_seq += 1;
                        }
                        None => break,
                    }
                }
            });

            let desktop_sink = Arc::new(DesktopRuntimeSink::new(
                run_id_clone.clone(),
                rt_event_tx,
            ));
            let hooks = HeadlessRunHooks {
                progress_tx: None,
                extra_event_sink: Some(
                    desktop_sink.clone() as Arc<dyn quorp_agent_core::RuntimeEventSink>,
                ),
                cancellation_flag: Some(cancellation_flag.clone()),
            };

            let result_dir = options
                .workspace_root
                .join(".quorp")
                .join("runs")
                .join(run_id_clone.as_str());
            let model_id_owned = options.model_id.clone();
            let max_steps = options.max_steps;
            let max_seconds = options.max_seconds;
            let workspace_root = options.workspace_root.clone();
            let goal = options.goal.clone();
            let secret_store_for_blocking = secret_store.clone();

            // Spawn the synchronous run on a blocking thread. The
            // `run_headless_agent_with_hooks` function builds its own
            // per-call tokio runtime; spawning under `spawn_blocking`
            // avoids the inner block_on conflicting with our outer
            // async context.
            let handle_run_id = run_id_clone.clone();
            let outcome: tokio::task::JoinHandle<Result<RunOutcomeBrief, String>> =
                tokio::task::spawn_blocking(move || {
                    // Set NIM key in env before the agent reads it.
                    if let Ok(Some(secret)) = secret_store_for_blocking
                        .get(crate::provider_registry::NIM_KEYCHAIN_ACCOUNT)
                    {
                        // Safe (best-effort): we never read the key
                        // back through env; the value is the same
                        // across concurrent runs because there's a
                        // single provider in v1.
                        unsafe {
                            std::env::set_var("NVIDIA_API_KEY", secret);
                        }
                    }
                    if let Err(err) = std::fs::create_dir_all(&result_dir) {
                        return Err(format!("create result_dir: {err}"));
                    }
                    let objective_path = result_dir.join("objective.md");
                    if let Err(err) = std::fs::write(&objective_path, &goal) {
                        return Err(format!("write objective.md: {err}"));
                    }

                    let opts = HeadlessRunOptions {
                        workspace: workspace_root,
                        objective_file: objective_path,
                        model_id: model_id_owned,
                        base_url_override: None,
                        max_steps,
                        max_seconds,
                        max_total_tokens: None,
                        result_dir: result_dir.clone(),
                        autonomy_profile: quorp_agent_core::AutonomyProfile::default(),
                        completion_policy: quorp_agent_core::CompletionPolicy::default(),
                        objective_metadata: serde_json::json!({}),
                        seed_context: Vec::new(),
                    };
                    match run_headless_agent_with_hooks(opts, hooks) {
                        Ok(outcome) => Ok(RunOutcomeBrief {
                            stop_reason: format!("{:?}", outcome.stop_reason),
                            total_steps: outcome.total_steps,
                            total_billed_tokens: outcome.total_billed_tokens,
                            duration_ms: outcome.duration_ms,
                        }),
                        Err(err) => Err(format!("{err:?}")),
                    }
                });

            let result = outcome.await;
            // Closing rt_event_tx (already dropped above when
            // desktop_sink goes out of scope at the end of this
            // closure) will let the drainer drain remaining buffered
            // events. Wait for it before announcing terminal events.
            drop(desktop_sink);
            let _ = drainer_handle.await;

            match result {
                Ok(Ok(brief)) => {
                    service_self
                        .mark_finished(&handle_run_id, map_stop_reason(&brief.stop_reason));
                    let _ = sink_for_task.send(DesktopEvent::RunFinished {
                        run_id: handle_run_id,
                        stop_reason: map_stop_reason(&brief.stop_reason),
                        total_steps: brief.total_steps,
                        total_billed_tokens: brief.total_billed_tokens,
                        duration_ms: brief.duration_ms,
                    });
                }
                Ok(Err(err)) => {
                    service_self.mark_finished(&handle_run_id, StopReasonDto::FatalError);
                    let _ = sink_for_task.send(DesktopEvent::RunFailed {
                        run_id: handle_run_id,
                        error: err,
                        stage: RunFailureStage::AgentLoop,
                    });
                }
                Err(join_err) => {
                    service_self.mark_finished(&handle_run_id, StopReasonDto::FatalError);
                    let _ = sink_for_task.send(DesktopEvent::RunFailed {
                        run_id: handle_run_id,
                        error: format!("join error: {join_err}"),
                        stage: RunFailureStage::AgentLoop,
                    });
                }
            }
        });

        Ok(handle)
    }
}

#[derive(Debug, Clone)]
struct RunOutcomeBrief {
    stop_reason: String,
    total_steps: usize,
    total_billed_tokens: u64,
    duration_ms: u64,
}

fn map_stop_reason(label: &str) -> StopReasonDto {
    match label {
        "Success" => StopReasonDto::Completed,
        "Cancelled" => StopReasonDto::Cancelled,
        "BudgetExhausted" => StopReasonDto::BudgetExhausted,
        "TimeBudgetExhausted" | "FirstTokenTimeout" | "StreamIdleTimeout"
        | "ModelRequestTimeout" => StopReasonDto::Timeout,
        "MaxIterations" | "PendingValidation" => StopReasonDto::Completed,
        _ => StopReasonDto::UnknownError,
    }
}

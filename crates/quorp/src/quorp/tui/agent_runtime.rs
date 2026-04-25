use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt as _;
use quorp_agent_core::StopReason;
use serde_json::Value;

use crate::quorp::agent_local::{
    CommandBridgeToolExecutor, HeadlessEventRecorder, LocalCompletionClient, TuiRuntimeEventSink,
    chat_to_core_message, write_final_diff, write_json,
};
use crate::quorp::codex_executor::{
    CodexProgressCallback, CodexProgressEvent, CodexTaskRunOptions, is_validation_command,
    run_codex_task,
};
use crate::quorp::executor::{
    CodexSessionMode, InteractiveProviderKind, codex_session_strategy_from_env,
};
use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::agent_context::AutonomyProfile;
use crate::quorp::tui::agent_protocol::{ActionOutcome, AgentMode};
use crate::quorp::tui::chat_service::{ChatServiceMessage, ChatServiceRole};
use crate::quorp::tui::command_bridge::CommandBridgeRequest;
use crate::quorp::tui::rail_event::{AgentPhase, RailEvent, ToolKind};
use crate::quorp::tui::slash_commands::{
    FullAutoResolvedMode, FullAutoSandboxMode, SlashCommandKind,
};
use crate::quorp::tui::ssd_moe_tui::SsdMoeRuntimeHandle;

#[derive(Debug, Clone)]
pub enum AgentRuntimeStatus {
    Idle,
    Thinking,
    ExecutingTool(String),
    Validating(String),
    Failed(String),
    Success,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum AgentUiEvent {
    StatusUpdate(AgentRuntimeStatus),
    TurnCompleted(Vec<ChatServiceMessage>),
    ArtifactsReady(PathBuf),
    FatalError(String),
}

pub const AGENT_RUNTIME_SESSION_ID: usize = usize::MAX - 1000;

#[derive(Debug, Clone)]
pub struct AgentTaskRequest {
    pub goal: String,
    pub initial_context: Vec<ChatServiceMessage>,
    pub model_id: String,
    pub agent_mode: AgentMode,
    pub base_url_override: Option<String>,
    pub workspace_root: PathBuf,
    pub target_path: PathBuf,
    pub command_kind: SlashCommandKind,
    pub resolved_mode: FullAutoResolvedMode,
    pub sandbox_mode: FullAutoSandboxMode,
    pub docker_image: Option<String>,
    pub max_iterations: usize,
    pub max_seconds: Option<u64>,
    pub max_total_tokens: Option<u64>,
    pub autonomy_profile: AutonomyProfile,
    pub result_dir: PathBuf,
    pub objective_file: Option<PathBuf>,
    pub evaluate_command: Option<String>,
    pub objective_metadata: serde_json::Value,
}

#[allow(dead_code, clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum AgentRuntimeCommand {
    StartTask(AgentTaskRequest),
    ToolFinished(ActionOutcome),
    Cancel,
}

pub struct AgentRuntimeHandle {
    pub tx: futures::channel::mpsc::UnboundedSender<AgentRuntimeCommand>,
}

struct ActiveRun {
    cancelled: Arc<AtomicBool>,
    abort_handle: tokio::task::AbortHandle,
}

struct CombinedRuntimeEventSink {
    tui: TuiRuntimeEventSink,
    recorder: HeadlessEventRecorder,
}

impl CombinedRuntimeEventSink {
    fn new(ui_tx: std::sync::mpsc::SyncSender<TuiEvent>, recorder: HeadlessEventRecorder) -> Self {
        Self {
            tui: TuiRuntimeEventSink::new(ui_tx),
            recorder,
        }
    }

    fn usage_summary(&self) -> crate::quorp::agent_local::HeadlessUsageSummary {
        self.recorder.usage_summary()
    }
}

impl quorp_agent_core::RuntimeEventSink for CombinedRuntimeEventSink {
    fn emit(&self, event: quorp_agent_core::RuntimeEvent) {
        self.tui.emit(event.clone());
        self.recorder.emit(event);
    }
}

pub fn spawn_agent_runtime(
    handle: tokio::runtime::Handle,
    _project_root: PathBuf,
    ui_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    command_bridge_tx: Option<futures::channel::mpsc::UnboundedSender<CommandBridgeRequest>>,
) -> AgentRuntimeHandle {
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let active_run: Arc<Mutex<Option<ActiveRun>>> = Arc::new(Mutex::new(None));

    let runtime_handle = handle.clone();
    handle.spawn({
        let active_run = Arc::clone(&active_run);
        async move {
            while let Some(command) = rx.next().await {
                match command {
                    AgentRuntimeCommand::StartTask(task) => {
                        if let Some(current) = active_run.lock().expect("active run lock").take() {
                            current.cancelled.store(true, Ordering::Relaxed);
                            current.abort_handle.abort();
                        }

                        let cancelled = Arc::new(AtomicBool::new(false));
                        let cancelled_for_task = Arc::clone(&cancelled);
                        let ui_tx_for_task = ui_tx.clone();
                        let command_bridge_tx_for_task = command_bridge_tx.clone();
                        let task_handle = runtime_handle.spawn(async move {
                            let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                AgentUiEvent::StatusUpdate(AgentRuntimeStatus::Thinking),
                            ));
                            if crate::quorp::tui::model_registry::chat_model_provider(
                                &task.model_id,
                                crate::quorp::executor::interactive_provider_from_env(),
                            ) == InteractiveProviderKind::Codex
                            {
                                let initial_context = task.initial_context.clone();
                                let model_id = crate::quorp::tui::model_registry::chat_model_raw_id(
                                    &task.model_id,
                                )
                                .to_string();
                                let goal = task.goal.clone();
                                let project_root_for_codex = task.workspace_root.clone();
                                let result_dir = task.result_dir.clone();
                                let objective_file = task.objective_file.clone();
                                let evaluate_command = task.evaluate_command.clone();
                                let objective_metadata = task.objective_metadata.clone();
                                let agent_mode = task.agent_mode;
                                let autonomy_profile = task.autonomy_profile;
                                let max_iterations = task.max_iterations;
                                let max_seconds = task.max_seconds;
                                let max_total_tokens = task.max_total_tokens;
                                let progress_callback =
                                    codex_progress_callback(ui_tx_for_task.clone());
                                let codex_result = tokio::task::spawn_blocking(move || {
                                    let mut prompt = String::new();
                                    prompt.push_str("You are running Quorp's autonomous `/agent` flow through the real Codex executor.\n");
                                    prompt.push_str("Inspect the real workspace, make the smallest correct changes, run local validation, and finish with a concise summary.\n\n");
                                    prompt.push_str("## Prior Chat Context\n");
                                    for message in initial_context {
                                        let role = match message.role {
                                            ChatServiceRole::System => "System",
                                            ChatServiceRole::User => "User",
                                            ChatServiceRole::Assistant => "Assistant",
                                        };
                                        prompt.push_str(&format!("{role}:\n{}\n\n", message.content));
                                    }
                                    prompt.push_str("## Current Autonomous Goal\n");
                                    prompt.push_str(&goal);
                                    let prompt_fingerprint =
                                        crate::quorp::codex_executor::fingerprint_prompt_text(
                                            &prompt,
                                        );
                                    run_codex_task(CodexTaskRunOptions {
                                        workspace: project_root_for_codex.clone(),
                                        prompt,
                                        prompt_fingerprint,
                                        user_message: goal,
                                        model_id,
                                        max_steps: max_iterations,
                                        max_seconds,
                                        max_total_tokens,
                                        result_dir,
                                        session_strategy: codex_session_strategy_from_env(
                                            CodexSessionMode::ResumeLastForCwd,
                                        ),
                                        metadata: serde_json::json!({
                                            "source": "tui-agent-runtime",
                                            "agent_mode": format!("{:?}", agent_mode),
                                            "autonomy_profile": autonomy_profile.label(),
                                            "objective_file": objective_file,
                                            "evaluate_command": evaluate_command,
                                            "objective": objective_metadata,
                                        }),
                                        progress_callback: Some(progress_callback),
                                    })
                                })
                                .await;
                                match codex_result {
                                    Ok(Ok(outcome)) => {
                                        if let Err(error) = crate::quorp::run_support::snapshot_logs(
                                            &task.result_dir,
                                            Some(crate::quorp::tui::diagnostics::app_run_id()),
                                        ) {
                                            let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                                AgentUiEvent::FatalError(format!(
                                                    "Failed to snapshot diagnostics logs: {error}"
                                                )),
                                            ));
                                        }
                                        let assistant_messages = outcome
                                            .transcript
                                            .into_iter()
                                            .filter(|message| {
                                                message.role
                                                    == quorp_agent_core::TranscriptRole::Assistant
                                            })
                                            .map(|message| ChatServiceMessage {
                                                role: ChatServiceRole::Assistant,
                                                content: message.content,
                                            })
                                            .collect::<Vec<_>>();
                                        let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                            AgentUiEvent::TurnCompleted(assistant_messages),
                                        ));
                                        let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                            AgentUiEvent::ArtifactsReady(task.result_dir.clone()),
                                        ));
                                        let _ = ui_tx_for_task.send(TuiEvent::RailEvent(
                                            RailEvent::ArtifactReady {
                                                label: "result_dir".to_string(),
                                                path: task.result_dir.display().to_string(),
                                            },
                                        ));
                                        let _ = ui_tx_for_task.send(TuiEvent::RailEvent(
                                            RailEvent::StopReasonSet {
                                                reason: format!("{:?}", outcome.stop_reason)
                                                    .to_ascii_lowercase(),
                                            },
                                        ));
                                        let status = if outcome.stop_reason
                                            == quorp_agent_core::StopReason::Success
                                        {
                                            AgentRuntimeStatus::Success
                                        } else {
                                            AgentRuntimeStatus::Failed(
                                                outcome.error_message.unwrap_or_else(|| {
                                                    format!(
                                                        "Codex agent run stopped with {:?}",
                                                        outcome.stop_reason
                                                    )
                                                }),
                                            )
                                        };
                                        let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                            AgentUiEvent::StatusUpdate(status),
                                        ));
                                    }
                                    Ok(Err(error)) => {
                                        let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                            AgentUiEvent::FatalError(error.to_string()),
                                        ));
                                    }
                                    Err(error) => {
                                        let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                            AgentUiEvent::FatalError(format!(
                                                "Codex agent task join error: {error}"
                                            )),
                                        ));
                                    }
                                }
                            } else if task.sandbox_mode == FullAutoSandboxMode::Docker {
                                let docker_ui_tx = ui_tx_for_task.clone();
                                match tokio::task::spawn_blocking(move || {
                                    run_docker_task_with_watch(task, docker_ui_tx)
                                })
                                .await
                                {
                                    Ok(Ok(())) => {}
                                    Ok(Err(error)) => {
                                        let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                            AgentUiEvent::FatalError(error.to_string()),
                                        ));
                                    }
                                    Err(error) => {
                                        let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                            AgentUiEvent::FatalError(format!(
                                                "Docker-backed agent task join error: {error}"
                                            )),
                                        ));
                                    }
                                }
                            } else {
                                let Some(command_bridge_tx) = command_bridge_tx_for_task else {
                                    let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                        AgentUiEvent::StatusUpdate(AgentRuntimeStatus::Failed(
                                            "No command bridge is available for autonomous execution."
                                                .to_string(),
                                        )),
                                    ));
                                    let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                        AgentUiEvent::FatalError(
                                            "No command bridge is available for autonomous execution."
                                                .to_string(),
                                        ),
                                    ));
                                    let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                        AgentUiEvent::StatusUpdate(AgentRuntimeStatus::Idle),
                                    ));
                                    return;
                                };
                                let completion_client = LocalCompletionClient::new(
                                    SsdMoeRuntimeHandle::shared_handle(),
                                );
                                let tool_executor =
                                    CommandBridgeToolExecutor::new(command_bridge_tx);
                                if let Err(error) = std::fs::create_dir_all(&task.result_dir) {
                                    let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                        AgentUiEvent::FatalError(format!(
                                            "Failed to create agent artifact directory: {error}"
                                        )),
                                    ));
                                    return;
                                }
                                if let Err(error) =
                                    std::fs::create_dir_all(task.result_dir.join("artifacts"))
                                {
                                    let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                        AgentUiEvent::FatalError(format!(
                                            "Failed to create agent artifacts directory: {error}"
                                        )),
                                    ));
                                    return;
                                }
                                let autonomy_profile_label =
                                    task.autonomy_profile.label().to_string();
                                let request = quorp_agent_core::AgentRunRequest {
                                    session_id: AGENT_RUNTIME_SESSION_ID,
                                    goal: task.goal,
                                    initial_context: task
                                        .initial_context
                                        .iter()
                                        .map(chat_to_core_message)
                                        .collect(),
                                    model_id: task.model_id,
                                    agent_mode: task.agent_mode,
                                    base_url_override: task.base_url_override,
                                    max_iterations: task.max_iterations,
                                    verifier_drain_budget: 4,
                                    parser_recovery_budget: 2,
                                    max_total_tokens: task.max_total_tokens,
                                    max_seconds: task.max_seconds,
                                    autonomy_profile: task.autonomy_profile,
                                    project_root: task.workspace_root.clone(),
                                    cwd: task.workspace_root.clone(),
                                    enable_rollback_on_validation_failure: true,
                                    completion_policy: quorp_agent_core::CompletionPolicy::default(
                                    ),
                                    run_metadata: serde_json::json!({
                                        "evaluate_command": task.evaluate_command,
                                        "objective_file": task.objective_file,
                                        "objective": task.objective_metadata,
                                    }),
                                    cancellation_flag: Some(cancelled_for_task),
                                };
                                if let Err(error) =
                                    write_json(&task.result_dir.join("request.json"), &request)
                                {
                                    let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                        AgentUiEvent::FatalError(format!(
                                            "Failed to write request.json: {error}"
                                        )),
                                    ));
                                    return;
                                }
                                let recorder = match HeadlessEventRecorder::new(
                                    &task.result_dir.join("events.jsonl"),
                                    task.result_dir.clone(),
                                    false,
                                ) {
                                    Ok(recorder) => recorder,
                                    Err(error) => {
                                        let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                            AgentUiEvent::FatalError(format!(
                                                "Failed to create events.jsonl recorder: {error}"
                                            )),
                                        ));
                                        return;
                                    }
                                };
                                let sink =
                                    CombinedRuntimeEventSink::new(ui_tx_for_task.clone(), recorder);
                                let outcome = quorp_agent_core::run_agent_task(
                                    &request,
                                    &completion_client,
                                    &tool_executor,
                                    &sink,
                                    None,
                                )
                                .await;
                                if let Err(error) = write_json(
                                    &task.result_dir.join("transcript.json"),
                                    &outcome.transcript,
                                ) {
                                    let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                        AgentUiEvent::FatalError(format!(
                                            "Failed to write transcript.json: {error}"
                                        )),
                                    ));
                                }
                                if let Err(error) = write_json(
                                    &task.result_dir.join("summary.json"),
                                    &serde_json::json!({
                                        "stop_reason": outcome.stop_reason,
                                        "total_steps": outcome.total_steps,
                                        "total_billed_tokens": outcome.total_billed_tokens,
                                        "duration_ms": outcome.duration_ms,
                                        "error_message": outcome.error_message,
                                        "usage": sink.usage_summary(),
                                    }),
                                ) {
                                    let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                        AgentUiEvent::FatalError(format!(
                                            "Failed to write summary.json: {error}"
                                        )),
                                    ));
                                }
                                if let Err(error) = write_json(
                                    &task.result_dir.join("metadata.json"),
                                    &serde_json::json!({
                                        "source": "tui-agent-runtime",
                                        "workspace": request.project_root.clone(),
                                        "model_id": request.model_id.clone(),
                                        "provider": crate::quorp::executor::interactive_provider_from_env().label(),
                                        "objective_file": task.objective_file,
                                        "evaluate_command": task.evaluate_command,
                                        "autonomy_profile": autonomy_profile_label,
                                        "objective": task.objective_metadata,
                                    }),
                                ) {
                                    let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                        AgentUiEvent::FatalError(format!(
                                            "Failed to write metadata.json: {error}"
                                        )),
                                    ));
                                }
                                if let Err(error) = write_final_diff(
                                    &request.project_root,
                                    &task.result_dir.join("final.diff"),
                                ) {
                                    let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                        AgentUiEvent::FatalError(format!(
                                            "Failed to write final.diff: {error}"
                                        )),
                                    ));
                                }
                                if let Err(error) = crate::quorp::run_support::snapshot_logs(
                                    &task.result_dir,
                                    Some(crate::quorp::tui::diagnostics::app_run_id()),
                                ) {
                                    let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                        AgentUiEvent::FatalError(format!(
                                            "Failed to snapshot diagnostics logs: {error}"
                                        )),
                                    ));
                                }
                                let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                    AgentUiEvent::ArtifactsReady(task.result_dir.clone()),
                                ));
                                let _ = ui_tx_for_task.send(TuiEvent::RailEvent(
                                    RailEvent::ArtifactReady {
                                        label: "result_dir".to_string(),
                                        path: task.result_dir.display().to_string(),
                                    },
                                ));
                            }
                            let _ = ui_tx_for_task.send(TuiEvent::AgentRuntime(
                                AgentUiEvent::StatusUpdate(AgentRuntimeStatus::Idle),
                            ));
                        });

                        *active_run.lock().expect("active run lock") = Some(ActiveRun {
                            cancelled,
                            abort_handle: task_handle.abort_handle(),
                        });
                    }
                    AgentRuntimeCommand::Cancel => {
                        if let Some(current) = active_run.lock().expect("active run lock").take() {
                            current.cancelled.store(true, Ordering::Relaxed);
                            current.abort_handle.abort();
                        }
                        let _ = ui_tx.send(TuiEvent::RailEvent(RailEvent::OneSecondStory {
                            summary: "Run cancelled by operator.".to_string(),
                        }));
                        let _ = ui_tx.send(TuiEvent::RailEvent(RailEvent::StopReasonSet {
                            reason: "cancelled".to_string(),
                        }));
                        let _ = ui_tx.send(TuiEvent::AgentRuntime(AgentUiEvent::StatusUpdate(
                            AgentRuntimeStatus::Idle,
                        )));
                    }
                    AgentRuntimeCommand::ToolFinished(_) => {}
                }
            }
        }
    });

    AgentRuntimeHandle { tx }
}

fn codex_progress_callback(ui_tx: std::sync::mpsc::SyncSender<TuiEvent>) -> CodexProgressCallback {
    let tool_id = Arc::new(std::sync::atomic::AtomicU64::new(1));
    let turn_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    Arc::new(move |event| {
        for rail_event in codex_progress_to_rail_events(&event, &tool_id, &turn_count) {
            if let Err(error) = ui_tx.send(TuiEvent::RailEvent(rail_event)) {
                log::error!("codex progress rail event channel closed: {error}");
                break;
            }
        }
    })
}

fn codex_progress_to_rail_events(
    event: &CodexProgressEvent,
    tool_id: &std::sync::Arc<std::sync::atomic::AtomicU64>,
    turn_count: &std::sync::Arc<std::sync::atomic::AtomicU32>,
) -> Vec<RailEvent> {
    match event {
        CodexProgressEvent::ThreadStarted { .. } => vec![
            RailEvent::PhaseChanged(AgentPhase::Planning),
            RailEvent::WaitReason {
                explanation: "Codex session attached. Bootstrapping the autonomous run."
                    .to_string(),
            },
            RailEvent::OneSecondStory {
                summary: "Codex is grounding the plan before it edits.".to_string(),
            },
            RailEvent::TopDoubtUpdated {
                doubt: "Need the first Codex evidence pass before editing.".to_string(),
            },
            RailEvent::TimeToProofUpdated {
                eta_seconds: Some(180),
                confidence_target: Some(0.82),
            },
        ],
        CodexProgressEvent::AssistantMessage { text } => vec![RailEvent::OneSecondStory {
            summary: truncate_story(text, 96),
        }],
        CodexProgressEvent::CommandExecution {
            command,
            exit_code,
            status,
        } => {
            let next_tool_id = tool_id.fetch_add(1, Ordering::Relaxed);
            let success = exit_code.unwrap_or_default() == 0;
            if is_validation_command(command) {
                vec![
                    RailEvent::PhaseChanged(AgentPhase::Verifying),
                    RailEvent::WaitReason {
                        explanation: command.clone(),
                    },
                    RailEvent::ToolStarted {
                        tool_id: next_tool_id,
                        name: "validation".to_string(),
                        kind: ToolKind::Test,
                        target: command.clone(),
                        cwd: None,
                        expected_outcome: "prove the latest change".to_string(),
                        validation_kind: Some("codex-validation".to_string()),
                    },
                    RailEvent::ToolCompleted {
                        tool_id: next_tool_id,
                        exit_code: *exit_code,
                        duration_ms: 0,
                        files_changed: 0,
                        confidence_delta: Some(if success { 0.08 } else { -0.12 }),
                    },
                    RailEvent::ProofProgress {
                        tests_passed: if success { 1 } else { 0 },
                        tests_total: 1,
                        coverage_delta: 0.0,
                    },
                    RailEvent::ConfidenceUpdate {
                        understanding: if success { 0.82 } else { 0.58 },
                        merge_safety: if success { 0.9 } else { 0.34 },
                        delta: if success { 0.08 } else { -0.12 },
                    },
                    RailEvent::OneSecondStory {
                        summary: format!("Validation {}: {}", status, truncate_story(command, 80)),
                    },
                    RailEvent::TimeToProofUpdated {
                        eta_seconds: Some(if success { 0 } else { 60 }),
                        confidence_target: Some(if success { 0.95 } else { 0.7 }),
                    },
                ]
            } else {
                vec![
                    RailEvent::PhaseChanged(AgentPhase::Editing),
                    RailEvent::ToolStarted {
                        tool_id: next_tool_id,
                        name: "command".to_string(),
                        kind: ToolKind::classify("command", command),
                        target: command.clone(),
                        cwd: None,
                        expected_outcome: "advance the current autonomous plan".to_string(),
                        validation_kind: None,
                    },
                    RailEvent::ToolCompleted {
                        tool_id: next_tool_id,
                        exit_code: *exit_code,
                        duration_ms: 0,
                        files_changed: 0,
                        confidence_delta: Some(if success { 0.03 } else { -0.05 }),
                    },
                    RailEvent::OneSecondStory {
                        summary: truncate_story(command, 96),
                    },
                ]
            }
        }
        CodexProgressEvent::TurnCompleted {
            input_tokens,
            output_tokens,
            cached_input_tokens: _,
        } => {
            let completed_turn = turn_count.fetch_add(1, Ordering::Relaxed) + 1;
            let understanding = (0.45 + (completed_turn as f32 * 0.07)).min(0.94);
            let merge_safety = (0.4 + (completed_turn as f32 * 0.08)).min(0.92);
            vec![
                RailEvent::ConfidenceUpdate {
                    understanding,
                    merge_safety,
                    delta: 0.05,
                },
                RailEvent::OneSecondStory {
                    summary: format!(
                        "Codex completed turn {} ({} in / {} out tokens).",
                        completed_turn, input_tokens, output_tokens
                    ),
                },
                RailEvent::TimeToProofUpdated {
                    eta_seconds: Some(120u64.saturating_sub(u64::from(completed_turn) * 20)),
                    confidence_target: Some(merge_safety.max(understanding)),
                },
            ]
        }
    }
}

#[allow(clippy::disallowed_methods)]
fn run_docker_task_with_watch(
    task: AgentTaskRequest,
    ui_tx: std::sync::mpsc::SyncSender<TuiEvent>,
) -> anyhow::Result<()> {
    if !docker_available() {
        anyhow::bail!("Docker sandbox requested, but `docker` is not installed or not running.");
    }
    let _ = ui_tx.send(TuiEvent::RailEvent(RailEvent::PhaseChanged(
        AgentPhase::Planning,
    )));
    let _ = ui_tx.send(TuiEvent::RailEvent(RailEvent::WaitReason {
        explanation: "Launching Docker-backed autonomous run.".to_string(),
    }));
    let _ = ui_tx.send(TuiEvent::RailEvent(RailEvent::OneSecondStory {
        summary: format!(
            "Starting {} in Docker for {}.",
            match task.resolved_mode {
                FullAutoResolvedMode::Benchmark => "benchmark watch mode",
                FullAutoResolvedMode::WorkspaceObjective => "full-auto watch mode",
            },
            task.target_path.display()
        ),
    }));
    let _ = ui_tx.send(TuiEvent::RailEvent(RailEvent::TimeToProofUpdated {
        eta_seconds: Some(180),
        confidence_target: Some(0.82),
    }));

    let model_id = crate::quorp::tui::model_registry::chat_model_raw_id(&task.model_id).to_string();
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command.arg(match task.resolved_mode {
        FullAutoResolvedMode::Benchmark => "benchmark",
        FullAutoResolvedMode::WorkspaceObjective => "agent",
    });
    command.arg("run");
    if task.resolved_mode == FullAutoResolvedMode::Benchmark {
        command.arg("--path").arg(&task.target_path);
        command.arg("--executor").arg(
            if crate::quorp::tui::model_registry::chat_model_provider(
                &task.model_id,
                crate::quorp::executor::interactive_provider_from_env(),
            ) == InteractiveProviderKind::Codex
            {
                "codex"
            } else {
                "native"
            },
        );
        command.arg("--model").arg(&model_id);
        command.arg("--result-dir").arg(&task.result_dir);
        command
            .arg("--max-steps")
            .arg(task.max_iterations.to_string());
        if let Some(max_seconds) = task.max_seconds {
            command.arg("--max-seconds").arg(max_seconds.to_string());
        }
        if let Some(max_total_tokens) = task.max_total_tokens {
            command
                .arg("--token-budget")
                .arg(max_total_tokens.to_string());
        }
    } else {
        command.arg("--workspace").arg(&task.workspace_root);
        command.arg("--objective-file").arg(
            task.objective_file
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "START_HERE.md".to_string()),
        );
        command.arg("--executor").arg(
            if crate::quorp::tui::model_registry::chat_model_provider(
                &task.model_id,
                crate::quorp::executor::interactive_provider_from_env(),
            ) == InteractiveProviderKind::Codex
            {
                "codex"
            } else {
                "native"
            },
        );
        command.arg("--model").arg(&model_id);
        command.arg("--result-dir").arg(&task.result_dir);
        command
            .arg("--max-steps")
            .arg(task.max_iterations.to_string());
        if let Some(max_seconds) = task.max_seconds {
            command.arg("--max-seconds").arg(max_seconds.to_string());
        }
        if let Some(max_total_tokens) = task.max_total_tokens {
            command
                .arg("--max-total-tokens")
                .arg(max_total_tokens.to_string());
        }
        command
            .arg("--autonomy-profile")
            .arg(task.autonomy_profile.label());
    }
    if let Some(base_url) = task.base_url_override.as_ref() {
        command.arg("--base-url").arg(base_url);
    }
    if let Some(docker_image) = task.docker_image.as_ref() {
        command.arg("--docker-image").arg(docker_image);
    }
    command.arg("--docker");
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());

    let mut child = command.spawn()?;
    let mut event_offset = 0usize;
    loop {
        pump_run_events_into_tui(
            &task.result_dir.join("events.jsonl"),
            &ui_tx,
            &mut event_offset,
        )?;
        if let Some(status) = child.try_wait()? {
            pump_run_events_into_tui(
                &task.result_dir.join("events.jsonl"),
                &ui_tx,
                &mut event_offset,
            )?;
            let summary = read_summary(&task.result_dir);
            let transcript_messages = read_assistant_transcript(&task.result_dir);
            if !transcript_messages.is_empty() {
                let _ = ui_tx.send(TuiEvent::AgentRuntime(AgentUiEvent::TurnCompleted(
                    transcript_messages,
                )));
            }
            let _ = ui_tx.send(TuiEvent::AgentRuntime(AgentUiEvent::ArtifactsReady(
                task.result_dir.clone(),
            )));
            let _ = ui_tx.send(TuiEvent::RailEvent(RailEvent::ArtifactReady {
                label: "result_dir".to_string(),
                path: task.result_dir.display().to_string(),
            }));
            let stop_reason = summary
                .as_ref()
                .and_then(|value| value.get("stop_reason"))
                .and_then(Value::as_str)
                .map(stop_reason_from_str)
                .unwrap_or_else(|| {
                    if status.success() {
                        StopReason::Success
                    } else {
                        StopReason::FatalError
                    }
                });
            let final_status = if matches!(stop_reason, StopReason::Success) {
                AgentRuntimeStatus::Success
            } else {
                AgentRuntimeStatus::Failed(
                    summary
                        .as_ref()
                        .and_then(|value| value.get("error_message"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("Docker run stopped with {:?}", stop_reason)),
                )
            };
            let _ = ui_tx.send(TuiEvent::RailEvent(RailEvent::OneSecondStory {
                summary: format!("Docker-backed run finished with {:?}", stop_reason),
            }));
            let _ = ui_tx.send(TuiEvent::RailEvent(RailEvent::StopReasonSet {
                reason: format!("{:?}", stop_reason).to_ascii_lowercase(),
            }));
            let _ = ui_tx.send(TuiEvent::AgentRuntime(AgentUiEvent::StatusUpdate(
                final_status,
            )));
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn pump_run_events_into_tui(
    events_path: &std::path::Path,
    ui_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    processed_bytes: &mut usize,
) -> anyhow::Result<()> {
    let raw = match std::fs::read_to_string(events_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if raw.len() <= *processed_bytes || !raw.ends_with('\n') {
        return Ok(());
    }
    for line in raw[*processed_bytes..]
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let value: Value = serde_json::from_str(line)?;
        let payload = value.get("payload").cloned().unwrap_or(Value::Null);
        for rail_event in synthetic_payload_to_rail_events(&payload) {
            let _ = ui_tx.send(TuiEvent::RailEvent(rail_event));
        }
    }
    *processed_bytes = raw.len();
    Ok(())
}

fn synthetic_payload_to_rail_events(payload: &Value) -> Vec<RailEvent> {
    match payload.get("event").and_then(Value::as_str) {
        Some("run_started") => vec![
            RailEvent::PhaseChanged(AgentPhase::Planning),
            RailEvent::WaitReason {
                explanation: "Run started inside Docker.".to_string(),
            },
            RailEvent::TopDoubtUpdated {
                doubt: "Waiting for the first Docker-backed proof signal.".to_string(),
            },
        ],
        Some("phase_changed") => {
            let phase = match payload
                .get("phase")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "thinking" | "planning" => AgentPhase::Planning,
                "exploring" => AgentPhase::Exploring,
                "editing" => AgentPhase::Editing,
                "verifying" => AgentPhase::Verifying,
                "retrying" => AgentPhase::Debugging,
                "approval" => AgentPhase::WaitingForApproval,
                _ => AgentPhase::Idle,
            };
            let mut events = vec![RailEvent::PhaseChanged(phase)];
            if let Some(detail) = payload.get("detail").and_then(Value::as_str) {
                events.push(RailEvent::OneSecondStory {
                    summary: truncate_story(detail, 96),
                });
            }
            events
        }
        Some("model_request_started") => vec![RailEvent::WaitReason {
            explanation: "Model is planning the next step.".to_string(),
        }],
        Some("assistant_turn_summary") => payload
            .get("assistant_message")
            .and_then(Value::as_str)
            .map(|message| {
                vec![RailEvent::OneSecondStory {
                    summary: truncate_story(message, 96),
                }]
            })
            .unwrap_or_default(),
        Some("tool_call_started") => {
            let action = payload
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let step = payload
                .get("step")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            vec![
                RailEvent::ToolStarted {
                    tool_id: step,
                    name: action.clone(),
                    kind: ToolKind::classify(&action, &action),
                    target: action.clone(),
                    cwd: None,
                    expected_outcome: "advance the current autonomous plan".to_string(),
                    validation_kind: None,
                },
                RailEvent::OneSecondStory {
                    summary: truncate_story(&action, 96),
                },
            ]
        }
        Some("tool_call_finished") => {
            let step = payload
                .get("step")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let status = payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("completed");
            vec![RailEvent::ToolCompleted {
                tool_id: step,
                exit_code: (status != "ok" && status != "completed").then_some(1),
                duration_ms: 0,
                files_changed: 0,
                confidence_delta: Some(if status == "ok" || status == "completed" {
                    0.03
                } else {
                    -0.05
                }),
            }]
        }
        Some("validation_started") => vec![
            RailEvent::PhaseChanged(AgentPhase::Verifying),
            RailEvent::WaitReason {
                explanation: payload
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
        ],
        Some("validation_finished") => {
            let success = matches!(
                payload.get("status").and_then(Value::as_str),
                Some("success") | Some("ok")
            );
            vec![
                RailEvent::ProofProgress {
                    tests_passed: if success { 1 } else { 0 },
                    tests_total: 1,
                    coverage_delta: 0.0,
                },
                RailEvent::ConfidenceUpdate {
                    understanding: if success { 0.82 } else { 0.56 },
                    merge_safety: if success { 0.9 } else { 0.35 },
                    delta: if success { 0.12 } else { -0.14 },
                },
                RailEvent::TimeToProofUpdated {
                    eta_seconds: Some(if success { 0 } else { 45 }),
                    confidence_target: Some(if success { 0.95 } else { 0.7 }),
                },
            ]
        }
        Some("run_finished") => {
            let reason = payload
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            vec![
                RailEvent::OneSecondStory {
                    summary: format!("Run finished with {reason}"),
                },
                RailEvent::StopReasonSet {
                    reason: reason.to_string(),
                },
            ]
        }
        _ => Vec::new(),
    }
}

#[allow(clippy::disallowed_methods)]
fn docker_available() -> bool {
    std::process::Command::new("docker")
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn read_summary(result_dir: &std::path::Path) -> Option<Value> {
    let path = result_dir.join("summary.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

fn read_assistant_transcript(result_dir: &std::path::Path) -> Vec<ChatServiceMessage> {
    let path = result_dir.join("transcript.json");
    let Some(raw) = std::fs::read_to_string(path).ok() else {
        return Vec::new();
    };
    let Some(transcript) =
        serde_json::from_str::<Vec<quorp_agent_core::TranscriptMessage>>(&raw).ok()
    else {
        return Vec::new();
    };
    transcript
        .into_iter()
        .filter(|message| message.role == quorp_agent_core::TranscriptRole::Assistant)
        .map(|message| ChatServiceMessage {
            role: ChatServiceRole::Assistant,
            content: message.content,
        })
        .collect()
}

fn stop_reason_from_str(value: &str) -> StopReason {
    match value {
        "success" | "Success" => StopReason::Success,
        "fatal_error" | "FatalError" => StopReason::FatalError,
        "time_budget_exhausted" | "TimeBudgetExhausted" => StopReason::TimeBudgetExhausted,
        "max_iterations" | "MaxIterations" => StopReason::MaxIterations,
        "budget_exhausted" | "BudgetExhausted" => StopReason::BudgetExhausted,
        "cancelled" | "Cancelled" => StopReason::Cancelled,
        "stalled" | "Stalled" => StopReason::Stalled,
        _ => StopReason::FatalError,
    }
}

fn truncate_story(text: &str, max_chars: usize) -> String {
    let mut truncated = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max_chars {
            truncated.push_str("...");
            break;
        }
        truncated.push(ch);
    }
    truncated.replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, AtomicU64};

    use super::{codex_progress_to_rail_events, spawn_agent_runtime, stop_reason_from_str};
    use crate::quorp::codex_executor::CodexProgressEvent;
    use crate::quorp::tui::TuiEvent;
    use crate::quorp::tui::rail_event::RailEvent;

    #[test]
    fn spawn_agent_runtime_accepts_explicit_runtime_handle_from_sync_context() {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let handle = runtime.handle().clone();
        let (ui_tx, _ui_rx) = std::sync::mpsc::sync_channel::<TuiEvent>(8);

        let agent_runtime = spawn_agent_runtime(handle, std::env::temp_dir(), ui_tx, None);

        assert!(!agent_runtime.tx.is_closed());
    }

    #[test]
    fn codex_progress_command_execution_maps_to_validation_rail_events() {
        let tool_counter = Arc::new(AtomicU64::new(1));
        let turn_counter = Arc::new(AtomicU32::new(0));
        let events = codex_progress_to_rail_events(
            &CodexProgressEvent::CommandExecution {
                command: "cargo test -p quorp".to_string(),
                exit_code: Some(0),
                status: "completed".to_string(),
            },
            &tool_counter,
            &turn_counter,
        );
        assert!(events.iter().any(|event| matches!(
            event,
            RailEvent::ProofProgress {
                tests_passed: 1,
                tests_total: 1,
                ..
            }
        )));
    }

    #[test]
    fn stop_reason_mapping_handles_common_values() {
        assert_eq!(
            stop_reason_from_str("success"),
            quorp_agent_core::StopReason::Success
        );
        assert_eq!(
            stop_reason_from_str("time_budget_exhausted"),
            quorp_agent_core::StopReason::TimeBudgetExhausted
        );
        assert_eq!(
            stop_reason_from_str("max_iterations"),
            quorp_agent_core::StopReason::MaxIterations
        );
    }
}

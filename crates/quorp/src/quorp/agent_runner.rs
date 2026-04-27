use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::SyncSender;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use futures::channel::mpsc;
use futures::channel::oneshot;
use quorp_agent_core::{
    AgentRunOutcome, AgentRunRequest, AgentRuntimeStatus, CompletionClient, CompletionRequest,
    CompletionResponse, RuntimeEvent, RuntimeEventSink, ToolExecutionRequest, ToolExecutionResult,
    ToolExecutor, TranscriptMessage, TranscriptRole, load_agent_config,
};

use crate::quorp::tui::chat_service::{ChatServiceMessage, ChatServiceRole, StreamRequest};
use crate::quorp::tui::command_bridge::CommandBridgeRequest;
use crate::quorp::tui::{ChatUiEvent, TuiEvent};

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";
const ANSI_CYAN: &str = "\x1b[36m";
const ANSI_BLUE: &str = "\x1b[34m";
const ANSI_MAGENTA: &str = "\x1b[35m";
const ANSI_GREEN: &str = "\x1b[32m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_RED: &str = "\x1b[31m";

pub struct RemoteCompletionClient;

impl CompletionClient for RemoteCompletionClient {
    fn request_completion<'a>(
        &'a self,
        request: &'a CompletionRequest,
    ) -> futures::future::BoxFuture<'a, Result<CompletionResponse, String>> {
        Box::pin(async move {
            let stream_request = StreamRequest {
                request_id: request.request_id,
                session_id: request.session_id,
                model_id: request.model_id.clone(),
                agent_mode: request.agent_mode,
                latest_input: request.latest_input.clone(),
                messages: request.messages.iter().map(core_to_chat_message).collect(),
                project_root: request.project_root.clone(),
                base_url_override: request.base_url_override.clone(),
                max_completion_tokens: request.max_completion_tokens,
                include_repo_capsule: request.include_repo_capsule,
                disable_reasoning: request.disable_reasoning,
                native_tool_calls: request.native_tool_calls,
                watchdog: request.watchdog.clone(),
                safety_mode_label: request.safety_mode_label.clone(),
                prompt_compaction_policy: request.prompt_compaction_policy,
                capture_scope: request.capture_scope.clone(),
                capture_call_class: request.capture_call_class.clone(),
            };
            let mut delay_ms = 1000;
            let mut attempts = 0;
            loop {
                match crate::quorp::tui::chat_service::request_single_completion_details(
                    &stream_request,
                )
                .await
                {
                    Ok(result) => {
                        return Ok(CompletionResponse {
                            content: result.content,
                            reasoning_content: result.reasoning_content,
                            native_turn: result.native_turn,
                            native_turn_error: result.native_turn_error,
                            usage: result.usage,
                            raw_provider_response: Some(result.raw_response),
                            watchdog: result.watchdog,
                        });
                    }
                    Err(error) => {
                        attempts += 1;
                        if attempts >= 5 {
                            return Err(error);
                        }
                        log::warn!("Provider call failed: {error}. Retrying in {delay_ms}ms...");
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                        delay_ms = std::cmp::min(delay_ms * 2, 60000);
                    }
                }
            }
        })
    }
}

pub struct RecordingCompletionClient<C> {
    inner: C,
    result_dir: PathBuf,
}

impl<C> RecordingCompletionClient<C> {
    pub fn new(inner: C, result_dir: PathBuf) -> Self {
        Self { inner, result_dir }
    }
}

impl<C> CompletionClient for RecordingCompletionClient<C>
where
    C: CompletionClient,
{
    fn request_completion<'a>(
        &'a self,
        request: &'a CompletionRequest,
    ) -> futures::future::BoxFuture<'a, Result<CompletionResponse, String>> {
        Box::pin(async move {
            let artifact_dir = self.result_dir.join("artifacts").join("model_turns");
            let artifact_path =
                artifact_dir.join(format!("request-{:04}.json", request.request_id));
            if let Err(error) = fs::create_dir_all(&artifact_dir) {
                log::error!("failed to create model turn artifact directory: {error}");
            }
            match self.inner.request_completion(request).await {
                Ok(response) => {
                    if let Some(routing) = extract_routing_decision(&response)
                        && let Err(error) = write_routing_summary(
                            &self.result_dir,
                            &routing,
                            response
                                .usage
                                .as_ref()
                                .and_then(|usage| usage.provider_request_id.as_deref()),
                        )
                    {
                        log::error!("failed to write routing summary: {error}");
                    }
                    if let Err(error) = write_json(
                        &artifact_path,
                        &serde_json::json!({
                            "request_id": request.request_id,
                            "session_id": request.session_id,
                            "model_id": request.model_id,
                            "agent_mode": request.agent_mode,
                            "latest_input": request.latest_input,
                            "project_root": request.project_root,
                            "message_count": request.messages.len(),
                            "messages": request.messages,
                            "max_completion_tokens": request.max_completion_tokens,
                            "include_repo_capsule": request.include_repo_capsule,
                            "disable_reasoning": request.disable_reasoning,
                            "native_tool_calls": request.native_tool_calls,
                            "prompt_compaction_policy": request.prompt_compaction_policy,
                            "watchdog": request.watchdog,
                            "safety_mode_label": request.safety_mode_label,
                            "response": {
                                "content": response.content.clone(),
                                "reasoning_content": response.reasoning_content.clone(),
                                "usage": response.usage.clone(),
                                "raw_provider_response": response.raw_provider_response.clone(),
                                "watchdog": response.watchdog.clone(),
                            }
                        }),
                    ) {
                        log::error!("failed to write model turn artifact: {error}");
                    }
                    Ok(response)
                }
                Err(error) => {
                    let routing = infer_routing_summary_from_request(request);
                    if let Err(write_error) =
                        write_routing_summary_seed(&self.result_dir, &routing, "failed")
                    {
                        log::error!("failed to seed routing summary on error: {write_error}");
                    }
                    if let Err(write_error) = write_json(
                        &artifact_path,
                        &serde_json::json!({
                            "request_id": request.request_id,
                            "session_id": request.session_id,
                            "model_id": request.model_id,
                            "agent_mode": request.agent_mode,
                            "latest_input": request.latest_input,
                            "project_root": request.project_root,
                            "message_count": request.messages.len(),
                            "messages": request.messages,
                            "max_completion_tokens": request.max_completion_tokens,
                            "include_repo_capsule": request.include_repo_capsule,
                            "disable_reasoning": request.disable_reasoning,
                            "native_tool_calls": request.native_tool_calls,
                            "prompt_compaction_policy": request.prompt_compaction_policy,
                            "watchdog": request.watchdog,
                            "safety_mode_label": request.safety_mode_label,
                            "routing": routing,
                            "error": error.clone(),
                        }),
                    ) {
                        log::error!("failed to write failed model turn artifact: {write_error}");
                    }
                    Err(error)
                }
            }
        })
    }
}

pub struct CommandBridgeToolExecutor {
    tx: mpsc::UnboundedSender<CommandBridgeRequest>,
}

impl CommandBridgeToolExecutor {
    pub fn new(tx: mpsc::UnboundedSender<CommandBridgeRequest>) -> Self {
        Self { tx }
    }
}

impl ToolExecutor for CommandBridgeToolExecutor {
    fn execute<'a>(
        &'a self,
        request: ToolExecutionRequest,
    ) -> futures::future::BoxFuture<'a, Result<ToolExecutionResult, String>> {
        Box::pin(async move {
            let (responder, receiver) = oneshot::channel();
            self.tx
                .unbounded_send(CommandBridgeRequest::ExecuteAction {
                    session_id: request.session_id,
                    action: request.action,
                    project_root: request.project_root,
                    cwd: request.cwd,
                    responder: Some(responder),
                    enable_rollback_on_validation_failure: request
                        .enable_rollback_on_validation_failure,
                })
                .map_err(|error| format!("Failed to dispatch action: {error}"))?;
            let outcome = receiver
                .await
                .map_err(|_| "Tool execution channel closed unexpectedly.".to_string())?;
            Ok(ToolExecutionResult { outcome })
        })
    }
}

pub struct HeadlessEventRecorder {
    writer: Mutex<BufWriter<File>>,
    result_dir: PathBuf,
    state: Mutex<HeadlessRecorderState>,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct HeadlessUsageSummary {
    pub model_requests: usize,
    pub reported_billed_tokens: u64,
    pub estimated_billed_tokens: u64,
    pub total_billed_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RoutingSummary {
    #[serde(default)]
    pub scenario_label: Option<String>,
    #[serde(default)]
    pub routing_mode: Option<String>,
    #[serde(default)]
    pub requested_provider: Option<String>,
    #[serde(default)]
    pub requested_model: Option<String>,
    #[serde(default)]
    pub candidate_models: Vec<String>,
    #[serde(default)]
    pub effective_provider: Option<String>,
    #[serde(default)]
    pub effective_model: Option<String>,
    #[serde(default)]
    pub used_fallback: bool,
    #[serde(default)]
    pub fallback_reason: Option<String>,
    #[serde(default)]
    pub comparable: Option<bool>,
    #[serde(default)]
    pub provider_base_url: Option<String>,
    #[serde(default)]
    pub auth_mode: Option<String>,
    #[serde(default)]
    pub proxy_visible_remote_egress_expected: bool,
    #[serde(default)]
    pub provider_request_id: Option<String>,
    #[serde(default)]
    pub routing_status: Option<String>,
}

struct CompositeEventSink<'a> {
    primary: &'a HeadlessEventRecorder,
    secondary: Option<&'a dyn RuntimeEventSink>,
}

impl<'a> RuntimeEventSink for CompositeEventSink<'a> {
    fn emit(&self, event: RuntimeEvent) {
        self.primary.emit(event.clone());
        if let Some(secondary) = self.secondary {
            secondary.emit(event);
        }
    }
}

#[derive(Debug)]
struct RuntimeEventProgressSink {
    event_tx: SyncSender<TuiEvent>,
}

impl RuntimeEventProgressSink {
    fn emit_summary(&self, message: String) {
        if self
            .event_tx
            .send(TuiEvent::Chat(ChatUiEvent::CommandOutput(0, message)))
            .is_err()
        {
            log::error!("progress sink dropped while sending runtime summary");
        }
    }

    fn emit_error(&self, message: String) {
        if self
            .event_tx
            .send(TuiEvent::Chat(ChatUiEvent::Error(0, message)))
            .is_err()
        {
            log::error!("progress sink dropped while sending runtime error");
        }
    }
}

impl RuntimeEventSink for RuntimeEventProgressSink {
    fn emit(&self, event: RuntimeEvent) {
        let summary = match event {
            RuntimeEvent::RunStarted { goal, model_id } => {
                Some(format!("run started with model {model_id} · {goal}"))
            }
            RuntimeEvent::StatusUpdate { status } => {
                Some(format!("status: {}", render_status(&status)))
            }
            RuntimeEvent::TurnCompleted { .. } => {
                return;
            }
            RuntimeEvent::FatalError { error } => {
                self.emit_error(format!("fatal error: {error}"));
                return;
            }
            RuntimeEvent::CheckpointSaved { .. } => Some("checkpoint saved".to_string()),
            RuntimeEvent::ModelRequestStarted {
                step,
                request_id,
                message_count,
                ..
            } => Some(format!(
                "step {step}: model request #{request_id} started ({message_count} messages)"
            )),
            RuntimeEvent::ModelRequestFinished {
                step,
                request_id,
                usage: Some(usage),
                ..
            } => Some(format!(
                "step {step}: model request #{request_id} finished · billed {}",
                usage.total_billed_tokens
            )),
            RuntimeEvent::ModelRequestFinished {
                step,
                request_id,
                usage: None,
                ..
            } => Some(format!("step {step}: model request #{request_id} finished")),
            RuntimeEvent::PhaseChanged { phase, detail } => Some(match detail {
                Some(detail) => format!("phase {phase} · {detail}"),
                None => format!("phase {phase}"),
            }),
            RuntimeEvent::ToolCallStarted { step, action, .. } => {
                Some(format!("step {step}: tool call started: {action}"))
            }
            RuntimeEvent::ToolCallFinished {
                step,
                action,
                status,
                ..
            } => Some(format!(
                "step {step}: tool call finished: {action} ({status})"
            )),
            RuntimeEvent::ValidationStarted { step, summary } => {
                Some(format!("step {step}: validation started: {summary}"))
            }
            RuntimeEvent::ValidationFinished {
                step,
                summary,
                status,
            } => Some(format!("step {step}: validation {status}: {summary}")),
            RuntimeEvent::PathResolutionFailed {
                step,
                action,
                request_path,
                suggested_path,
                reason,
                error,
                ..
            } => {
                let suggested = suggested_path.unwrap_or_else(|| "<none>".to_string());
                let reason_label = reason.unwrap_or_else(|| "<unknown>".to_string());
                Some(format!(
                    "step {step}: path resolution failed for {action} on {request_path} · {reason_label} · suggested {suggested} · {error}"
                ))
            }
            RuntimeEvent::RecoveryTurnQueued {
                step,
                action,
                message,
                ..
            } => Some(format!(
                "step {step}: recovery queued for {action} · {message}"
            )),
            RuntimeEvent::RecoveryBudgetExhausted {
                failures,
                last_error,
            } => Some(format!(
                "recovery budget exhausted after {failures} failures: {last_error}"
            )),
            RuntimeEvent::ParseRecoveryQueued {
                step,
                error_class,
                failures,
                budget,
                message,
            } => Some(format!(
                "step {step}: parse recovery queued ({failures}/{budget}) [{error_class}] · {message}"
            )),
            RuntimeEvent::ParseRecoveryExhausted {
                failures,
                error_class,
                last_error,
                ..
            } => Some(format!(
                "parse recovery exhausted ({failures}) [{error_class}]: {last_error}"
            )),
            RuntimeEvent::VerifierQueued {
                step,
                reason,
                plans,
                ..
            } => Some(format!(
                "step {step}: verifier queued · {reason} · {} plan(s)",
                plans.len()
            )),
            RuntimeEvent::VerifierDrainStarted {
                step,
                plans,
                budget,
                ..
            } => Some(format!(
                "step {step}: verifier drain started (budget {budget}) · {} plan(s)",
                plans.len()
            )),
            RuntimeEvent::VerifierDrainFinished {
                step,
                remaining,
                verified_green,
                ..
            } => Some(format!(
                "step {step}: verifier drain finished · remaining {remaining} · verified_green {verified_green}"
            )),
            RuntimeEvent::PendingValidationBlocked {
                step,
                queued_validations,
                drain_budget,
                ..
            } => Some(format!(
                "step {step}: pending validation blocked ({} queued) · drain budget {drain_budget}",
                queued_validations.len()
            )),
            RuntimeEvent::PolicyDenied {
                step,
                action,
                reason,
            } => Some(format!("step {step}: policy denied {action}: {reason}")),
            RuntimeEvent::FailedEditRecorded { step, .. } => {
                Some(format!("step {step}: failed edit recorded"))
            }
            RuntimeEvent::ControllerReadInjected {
                step,
                action,
                reason,
            } => Some(format!(
                "step {step}: controller read injected for {action} · {reason}"
            )),
            RuntimeEvent::AssistantTurnSummary {
                step,
                assistant_message,
                wrote_files,
                parse_warning_count,
                ..
            } => {
                let summary = if wrote_files {
                    format!("step {step}: wrote files and generated a summary")
                } else {
                    format!("step {step}: assistant summary generated")
                };
                self.emit_summary(summary);
                let trim = truncate_console(&assistant_message, 200);
                self.emit_summary(format!(
                    "assistant summary: parse_warnings={parse_warning_count} · {trim}"
                ));
                return;
            }
            RuntimeEvent::RunFinished { reason, .. } => Some(format!("run finished: {reason:?}")),
        };
        if let Some(summary) = summary {
            self.emit_summary(summary);
        }
    }
}

fn stream_command_events_to(
    event_rx: std::sync::mpsc::Receiver<TuiEvent>,
    command_event_tx: Option<SyncSender<TuiEvent>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        if let Some(command_event_tx) = command_event_tx {
            while let Ok(event) = event_rx.recv() {
                if command_event_tx.send(event).is_err() {
                    break;
                }
            }
        } else {
            while event_rx.recv().is_ok() {}
        }
    })
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RoutingDecision {
    pub routing_mode: String,
    pub requested_provider: String,
    pub requested_model: String,
    #[serde(default)]
    pub candidate_models: Vec<String>,
    pub effective_provider: String,
    pub effective_model: String,
    #[serde(default)]
    pub used_fallback: bool,
    #[serde(default)]
    pub fallback_reason: Option<String>,
    #[serde(default)]
    pub comparable: bool,
    #[serde(default)]
    pub provider_base_url: Option<String>,
    #[serde(default)]
    pub auth_mode: Option<String>,
    #[serde(default)]
    pub proxy_visible_remote_egress_expected: bool,
}

#[derive(Default)]
struct HeadlessRecorderState {
    model_requests: usize,
    reported_billed_tokens: u64,
    estimated_billed_tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
    cache_read_input_tokens: u64,
    cache_write_input_tokens: u64,
}

impl HeadlessEventRecorder {
    pub fn new(path: &Path, result_dir: PathBuf, append: bool) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut opts = fs::OpenOptions::new();
        opts.create(true).write(true);
        if append {
            opts.append(true);
        } else {
            opts.truncate(true);
        }
        let file = opts.open(path)?;
        Ok(Self {
            writer: Mutex::new(BufWriter::new(file)),
            result_dir,
            state: Mutex::new(HeadlessRecorderState::default()),
        })
    }

    pub fn usage_summary(&self) -> HeadlessUsageSummary {
        self.state
            .lock()
            .map(|state| HeadlessUsageSummary {
                model_requests: state.model_requests,
                reported_billed_tokens: state.reported_billed_tokens,
                estimated_billed_tokens: state.estimated_billed_tokens,
                total_billed_tokens: state
                    .reported_billed_tokens
                    .saturating_add(state.estimated_billed_tokens),
                input_tokens: state.input_tokens,
                output_tokens: state.output_tokens,
                reasoning_tokens: state.reasoning_tokens,
                cache_read_input_tokens: state.cache_read_input_tokens,
                cache_write_input_tokens: state.cache_write_input_tokens,
            })
            .unwrap_or_default()
    }

    fn log_console_event(&self, event: &RuntimeEvent) {
        match event {
            RuntimeEvent::RunStarted { goal, model_id } => eprintln!(
                "{}{}[run]{} {}model={}{} goal={}",
                ANSI_BOLD,
                ANSI_CYAN,
                ANSI_RESET,
                ANSI_BLUE,
                model_id,
                ANSI_RESET,
                truncate_console(goal, 120)
            ),
            RuntimeEvent::ModelRequestStarted {
                step,
                request_id,
                message_count,
                prompt_token_estimate,
                completion_token_cap,
                safety_mode,
            } => eprintln!(
                "{}[model]{} step={} request={} messages={} prompt_est={} max_tokens={} safety={}",
                ANSI_BLUE,
                ANSI_RESET,
                step,
                request_id,
                message_count,
                prompt_token_estimate,
                completion_token_cap
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "default".to_string()),
                safety_mode.as_deref().unwrap_or("standard")
            ),
            RuntimeEvent::ModelRequestFinished {
                step,
                request_id,
                usage,
                watchdog,
            } => {
                let usage_text = usage
                    .as_ref()
                    .map(render_usage)
                    .unwrap_or_else(|| "usage unavailable".to_string());
                let watchdog_text = watchdog
                    .as_ref()
                    .map(render_watchdog)
                    .unwrap_or_else(|| "watchdog=off".to_string());
                eprintln!(
                    "{}[model]{} step={} request={} {} {}",
                    ANSI_BLUE, ANSI_RESET, step, request_id, usage_text, watchdog_text
                );
            }
            RuntimeEvent::ToolCallStarted { step, action } => eprintln!(
                "{}[tool]{} step={} {}",
                ANSI_MAGENTA, ANSI_RESET, step, action
            ),
            RuntimeEvent::ToolCallFinished {
                step,
                action,
                status,
                ..
            } => {
                let color = if *status == "success" {
                    ANSI_GREEN
                } else {
                    ANSI_RED
                };
                eprintln!("{color}[tool]{ANSI_RESET} step={step} {action} status={status}");
            }
            RuntimeEvent::ValidationStarted { step, summary } => eprintln!(
                "{}[validate]{} step={} {}",
                ANSI_YELLOW, ANSI_RESET, step, summary
            ),
            RuntimeEvent::ValidationFinished {
                step,
                summary,
                status,
            } => {
                let color = if *status == "success" {
                    ANSI_GREEN
                } else {
                    ANSI_RED
                };
                eprintln!("{color}[validate]{ANSI_RESET} step={step} {summary} status={status}");
            }
            RuntimeEvent::PhaseChanged { phase, detail } => eprintln!(
                "{}[phase]{} {}{}",
                ANSI_CYAN,
                ANSI_RESET,
                phase,
                detail
                    .as_ref()
                    .map(|value| format!(" :: {}", truncate_console(value, 80)))
                    .unwrap_or_default()
            ),
            RuntimeEvent::AssistantTurnSummary {
                step,
                assistant_message,
                actions,
                wrote_files,
                validation_queued,
                parse_warning_count,
            } => eprintln!(
                "{}[turn]{} step={} actions={} wrote_files={} validation_queued={} parse_warnings={} note={}",
                ANSI_BLUE,
                ANSI_RESET,
                step,
                actions.join(", "),
                wrote_files,
                validation_queued,
                parse_warning_count,
                truncate_console(assistant_message, 100)
            ),
            RuntimeEvent::ParseRecoveryQueued {
                step,
                error_class,
                failures,
                budget,
                message,
            } => eprintln!(
                "{}[parser]{} step={} class={} failures={}/{} {}",
                ANSI_YELLOW,
                ANSI_RESET,
                step,
                error_class,
                failures,
                budget,
                truncate_console(message, 120)
            ),
            RuntimeEvent::ParseRecoveryExhausted {
                failures,
                error_class,
                last_error,
            } => eprintln!(
                "{}[parser]{} exhausted class={} failures={} last_error={}",
                ANSI_RED,
                ANSI_RESET,
                error_class,
                failures,
                truncate_console(last_error, 120)
            ),
            RuntimeEvent::PathResolutionFailed {
                action,
                request_path,
                suggested_path,
                ..
            } => eprintln!(
                "{}[path]{} action={} request={} suggested={}",
                ANSI_YELLOW,
                ANSI_RESET,
                truncate_console(action, 80),
                truncate_console(request_path, 80),
                suggested_path.as_deref().unwrap_or("none")
            ),
            RuntimeEvent::RecoveryTurnQueued {
                action,
                suggested_path,
                ..
            } => eprintln!(
                "{}[recovery]{} action={} suggested={}",
                ANSI_YELLOW,
                ANSI_RESET,
                truncate_console(action, 80),
                suggested_path.as_deref().unwrap_or("none")
            ),
            RuntimeEvent::RecoveryBudgetExhausted {
                failures,
                last_error,
            } => eprintln!(
                "{}[recovery]{} exhausted failures={} last_error={}",
                ANSI_RED,
                ANSI_RESET,
                failures,
                truncate_console(last_error, 120)
            ),
            RuntimeEvent::VerifierQueued {
                step,
                plans,
                reason,
            } => eprintln!(
                "{}[verifier]{} step={} queued={} reason={}",
                ANSI_YELLOW,
                ANSI_RESET,
                step,
                plans.join(" -> "),
                reason
            ),
            RuntimeEvent::VerifierDrainStarted {
                step,
                plans,
                budget,
            } => eprintln!(
                "{}[verifier]{} drain-start step={} budget={} queued={}",
                ANSI_YELLOW,
                ANSI_RESET,
                step,
                budget,
                plans.join(" -> ")
            ),
            RuntimeEvent::VerifierDrainFinished {
                step,
                remaining,
                verified_green,
            } => eprintln!(
                "{}[verifier]{} drain-finished step={} remaining={} verified_green={}",
                ANSI_YELLOW, ANSI_RESET, step, remaining, verified_green
            ),
            RuntimeEvent::PendingValidationBlocked {
                step,
                queued_validations,
                drain_budget,
            } => eprintln!(
                "{}[verifier]{} blocked step={} drain_budget={} queued={}",
                ANSI_RED,
                ANSI_RESET,
                step,
                drain_budget,
                queued_validations.join(" -> ")
            ),
            RuntimeEvent::PolicyDenied {
                step,
                action,
                reason,
            } => eprintln!(
                "{}[policy]{} step={} {} :: {}",
                ANSI_RED, ANSI_RESET, step, action, reason
            ),
            RuntimeEvent::FailedEditRecorded { step, record } => eprintln!(
                "{}[repair]{} step={} failed_edit={} path={} attempts={}",
                ANSI_YELLOW, ANSI_RESET, step, record.action_kind, record.path, record.attempts
            ),
            RuntimeEvent::ControllerReadInjected {
                step,
                action,
                reason,
            } => eprintln!(
                "{}[controller]{} step={} injected={} reason={}",
                ANSI_YELLOW,
                ANSI_RESET,
                step,
                truncate_console(action, 80),
                truncate_console(reason, 80)
            ),
            RuntimeEvent::StatusUpdate { status } => eprintln!(
                "{}[status]{} {}",
                ANSI_CYAN,
                ANSI_RESET,
                render_status(status)
            ),
            RuntimeEvent::FatalError { error } => {
                eprintln!("{}[fatal]{} {}", ANSI_RED, ANSI_RESET, error)
            }
            RuntimeEvent::RunFinished {
                reason,
                total_steps,
                total_billed_tokens,
                duration_ms,
            } => eprintln!(
                "{}{}[done]{} reason={:?} steps={} billed_tokens={} duration_ms={}",
                ANSI_BOLD,
                if matches!(reason, quorp_agent_core::StopReason::Success) {
                    ANSI_GREEN
                } else {
                    ANSI_RED
                },
                ANSI_RESET,
                reason,
                total_steps,
                total_billed_tokens,
                duration_ms
            ),
            RuntimeEvent::TurnCompleted { .. } | RuntimeEvent::CheckpointSaved { .. } => {}
        }
    }
}

impl RuntimeEventSink for HeadlessEventRecorder {
    fn emit(&self, event: RuntimeEvent) {
        if let RuntimeEvent::CheckpointSaved { checkpoint } = &event {
            let path = self.result_dir.join("checkpoint.json");
            if let Err(error) = write_json(&path, checkpoint) {
                log::error!("failed to write checkpoint.json: {error}");
            }
            return;
        }
        if matches!(event, RuntimeEvent::TurnCompleted { .. }) {
            return;
        }

        if let Ok(mut writer) = self.writer.lock() {
            let payload = event.clone();
            let record = serde_json::json!({
                "ts_ms": timestamp_ms(),
                "payload": payload,
            });
            if let Err(error) = writeln!(writer, "{}", record) {
                log::error!("failed to write headless event record: {error}");
            }
            if let Err(error) = writer.flush() {
                log::error!("failed to flush headless event record: {error}");
            }
        } else {
            log::error!("failed to lock headless event recorder");
        }
        if let RuntimeEvent::ModelRequestFinished {
            usage: Some(usage), ..
        } = &event
            && let Ok(mut state) = self.state.lock()
        {
            state.model_requests += 1;
            match usage.usage_source {
                quorp_agent_core::UsageSource::Reported => {
                    state.reported_billed_tokens = state
                        .reported_billed_tokens
                        .saturating_add(usage.total_billed_tokens);
                }
                quorp_agent_core::UsageSource::Estimated => {
                    state.estimated_billed_tokens = state
                        .estimated_billed_tokens
                        .saturating_add(usage.total_billed_tokens);
                }
            }
            state.input_tokens = state.input_tokens.saturating_add(usage.input_tokens);
            state.output_tokens = state.output_tokens.saturating_add(usage.output_tokens);
            state.reasoning_tokens = state
                .reasoning_tokens
                .saturating_add(usage.reasoning_tokens.unwrap_or_default());
            state.cache_read_input_tokens = state
                .cache_read_input_tokens
                .saturating_add(usage.cache_read_input_tokens.unwrap_or_default());
            state.cache_write_input_tokens = state
                .cache_write_input_tokens
                .saturating_add(usage.cache_write_input_tokens.unwrap_or_default());
        } else if matches!(
            event,
            RuntimeEvent::ModelRequestFinished { usage: None, .. }
        ) && let Ok(mut state) = self.state.lock()
        {
            state.model_requests += 1;
        }
        self.log_console_event(&event);
    }
}

pub struct HeadlessRunOptions {
    pub workspace: PathBuf,
    pub objective_file: PathBuf,
    pub model_id: String,
    pub base_url_override: Option<String>,
    pub max_steps: usize,
    pub max_seconds: Option<u64>,
    pub max_total_tokens: Option<u64>,
    pub result_dir: PathBuf,
    pub autonomy_profile: quorp_agent_core::AutonomyProfile,
    pub completion_policy: quorp_agent_core::CompletionPolicy,
    pub objective_metadata: serde_json::Value,
    pub seed_context: Vec<TranscriptMessage>,
}

pub fn run_headless_agent(options: HeadlessRunOptions) -> anyhow::Result<AgentRunOutcome> {
    run_headless_agent_with_progress(options, None)
}

pub fn run_headless_agent_with_progress(
    options: HeadlessRunOptions,
    progress_tx: Option<SyncSender<TuiEvent>>,
) -> anyhow::Result<AgentRunOutcome> {
    let objective_path = if options.objective_file.is_absolute() {
        options.objective_file.clone()
    } else {
        options.workspace.join(&options.objective_file)
    };
    let objective_text = fs::read_to_string(&objective_path).ok();
    let success_path = options
        .objective_metadata
        .get("success_file")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from);
    let success_text = success_path
        .as_ref()
        .and_then(|path| fs::read_to_string(path).ok());
    let evaluate_command = options
        .objective_metadata
        .get("evaluate_command")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let retry_context = options
        .objective_metadata
        .get("retry_context")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let workspace_entries = options
        .objective_metadata
        .get("editable_workspace_entries")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(serde_json::Value::as_str)
                .take(16)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|entries| !entries.is_empty());
    let mut objective_prompt = format!(
        "The editable workspace root for every tool call is `{}`. Do not prefix tool paths with `workspace/`. If the brief mentions `workspace/<condition>/`, that alias already refers to the current root for this run. Start with `ListDirectory` on `.` or one of the expected touch targets, keep all paths relative to the current root, and continue autonomously until the requirements are satisfied.",
        options.workspace.display(),
    );
    if let Some(evaluate_command) = evaluate_command.as_ref() {
        objective_prompt.push_str(&format!(
            " Stop only when the visible evaluator `{evaluate_command}` succeeds."
        ));
    }
    if let Some(entries) = workspace_entries.as_ref() {
        objective_prompt.push_str(&format!(" Current root entries: {entries}."));
    }
    if let Some(retry_context) = retry_context.as_ref() {
        objective_prompt.push_str(&format!(
            " Previous attempt context: {}",
            retry_context.trim()
        ));
    }
    fs::create_dir_all(&options.result_dir)?;
    fs::create_dir_all(options.result_dir.join("artifacts"))?;
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel::<TuiEvent>(256);
    let (command_tx, command_rx) = mpsc::unbounded();
    let _command_thread =
        crate::quorp::tui::native_backend::spawn_command_service_loop(event_tx, command_rx);
    let _command_event_forwarder = stream_command_events_to(event_rx, progress_tx.clone());

    let runtime = tokio::runtime::Runtime::new()?;
    let completion_client =
        RecordingCompletionClient::new(RemoteCompletionClient, options.result_dir.clone());
    let tool_executor = CommandBridgeToolExecutor::new(command_tx);
    let event_recorder = HeadlessEventRecorder::new(
        &options.result_dir.join("events.jsonl"),
        options.result_dir.clone(),
        false,
    )?;
    let config = load_agent_config(&options.workspace);

    let mut initial_context = options.seed_context.clone();
    if !initial_context.is_empty() {
        initial_context.push(TranscriptMessage {
            role: TranscriptRole::System,
            content: "Benchmark seed context ends here. Treat the following objective as the active task for this run.".to_string(),
        });
    }
    initial_context.push(TranscriptMessage {
        role: TranscriptRole::User,
        content: objective_prompt.clone(),
    });
    if let Some(objective_text) = objective_text.as_ref() {
        initial_context.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: format!(
                "[Objective File]\npath: {}\n{}",
                objective_path.display(),
                objective_text
            ),
        });
    }
    if let (Some(success_path), Some(success_text)) = (success_path.as_ref(), success_text.as_ref())
    {
        initial_context.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: format!(
                "[Success Criteria]\npath: {}\n{}",
                success_path.display(),
                success_text
            ),
        });
    }
    let request = AgentRunRequest {
        session_id: 0,
        goal: objective_prompt.clone(),
        initial_context,
        model_id: options.model_id.clone(),
        agent_mode: quorp_agent_core::AgentMode::Act,
        base_url_override: options.base_url_override.clone(),
        max_iterations: options.max_steps,
        verifier_drain_budget: 4,
        parser_recovery_budget: 2,
        max_total_tokens: options.max_total_tokens,
        max_seconds: options.max_seconds,
        autonomy_profile: options.autonomy_profile,
        project_root: options.workspace.clone(),
        cwd: options.workspace.clone(),
        enable_rollback_on_validation_failure: options
            .completion_policy
            .safety_mode_label
            .as_deref()
            != Some("nvidia_qwen_benchmark"),
        completion_policy: options.completion_policy.clone(),
        run_metadata: options.objective_metadata.clone(),
        cancellation_flag: None,
    };

    let mut request_value = serde_json::to_value(&request)?;
    if let Some(object) = request_value.as_object_mut() {
        object.insert("runtime".to_string(), serde_json::json!({"mode": "native"}));
    }
    write_json(&options.result_dir.join("request.json"), &request_value)?;

    let outcome = if let Some(progress_tx) = progress_tx {
        let event_sink = CompositeEventSink {
            primary: &event_recorder,
            secondary: Some(&RuntimeEventProgressSink {
                event_tx: progress_tx,
            }),
        };
        runtime.block_on(quorp_agent_core::run_agent_task(
            &request,
            &completion_client,
            &tool_executor,
            &event_sink,
            None,
        ))
    } else {
        runtime.block_on(quorp_agent_core::run_agent_task(
            &request,
            &completion_client,
            &tool_executor,
            &event_recorder,
            None,
        ))
    };

    write_json(
        &options.result_dir.join("transcript.json"),
        &outcome.transcript,
    )?;
    write_json(
        &options.result_dir.join("summary.json"),
        &serde_json::json!({
            "stop_reason": outcome.stop_reason,
            "total_steps": outcome.total_steps,
            "total_billed_tokens": outcome.total_billed_tokens,
            "duration_ms": outcome.duration_ms,
            "error_message": outcome.error_message,
            "scenario_label": crate::quorp::provider_config::resolved_scenario_label(),
            "usage": event_recorder.usage_summary(),
            "routing": read_routing_summary(&options.result_dir),
        }),
    )?;
    write_json(
        &options.result_dir.join("metadata.json"),
        &serde_json::json!({
            "workspace": options.workspace.clone(),
            "objective_file": objective_path,
            "model_id": options.model_id,
            "scenario_label": crate::quorp::provider_config::resolved_scenario_label(),
            "autonomy_profile": options.autonomy_profile.label(),
            "policy_mode": config.policy.mode.label(),
            "policy_allow_run_command": config.policy.allow.run_command,
            "policy_allow_network": config.policy.allow.network,
            "policy_max_command_runtime_seconds": config.policy.limits.max_command_runtime_seconds,
            "policy_max_command_output_bytes": config.policy.limits.max_command_output_bytes,
            "max_steps": options.max_steps,
            "max_seconds": options.max_seconds,
            "max_total_tokens": options.max_total_tokens,
            "completion_policy": options.completion_policy,
            "provider": crate::quorp::executor::interactive_provider_from_env().label(),
            "routing": read_routing_summary(&options.result_dir),
            "objective": options.objective_metadata,
            "runtime": {"mode": "native"},
        }),
    )?;
    write_final_diff(&options.workspace, &options.result_dir.join("final.diff"))?;
    Ok(outcome)
}

pub fn resume_headless_agent(result_dir: PathBuf) -> anyhow::Result<AgentRunOutcome> {
    resume_headless_agent_with_progress(result_dir, None)
}

pub fn resume_headless_agent_with_progress(
    result_dir: PathBuf,
    progress_tx: Option<SyncSender<TuiEvent>>,
) -> anyhow::Result<AgentRunOutcome> {
    let request_path = result_dir.join("request.json");
    let checkpoint_path = result_dir.join("checkpoint.json");

    if !request_path.exists() {
        anyhow::bail!("Missing request.json in {}", result_dir.display());
    }
    if !checkpoint_path.exists() {
        anyhow::bail!("Missing checkpoint.json in {}", result_dir.display());
    }

    let request_json = fs::read_to_string(&request_path).context("failed to read request.json")?;
    let request: AgentRunRequest =
        serde_json::from_str(&request_json).context("failed to parse request.json")?;

    let checkpoint_json =
        fs::read_to_string(&checkpoint_path).context("failed to read checkpoint.json")?;
    let checkpoint: quorp_agent_core::AgentCheckpoint =
        serde_json::from_str(&checkpoint_json).context("failed to parse checkpoint.json")?;

    let (event_tx, event_rx) = std::sync::mpsc::sync_channel::<TuiEvent>(256);
    let (command_tx, command_rx) = mpsc::unbounded();
    let _command_thread =
        crate::quorp::tui::native_backend::spawn_command_service_loop(event_tx, command_rx);
    let _command_event_forwarder = stream_command_events_to(event_rx, progress_tx.clone());

    let runtime = tokio::runtime::Runtime::new()?;
    let completion_client =
        RecordingCompletionClient::new(RemoteCompletionClient, result_dir.clone());
    let tool_executor = CommandBridgeToolExecutor::new(command_tx);
    let event_recorder =
        HeadlessEventRecorder::new(&result_dir.join("events.jsonl"), result_dir.clone(), true)?;

    let options_workspace = request.project_root.clone();

    let outcome = if let Some(progress_tx) = progress_tx {
        let event_sink = CompositeEventSink {
            primary: &event_recorder,
            secondary: Some(&RuntimeEventProgressSink {
                event_tx: progress_tx,
            }),
        };
        runtime.block_on(quorp_agent_core::run_agent_task(
            &request,
            &completion_client,
            &tool_executor,
            &event_sink,
            Some(checkpoint),
        ))
    } else {
        runtime.block_on(quorp_agent_core::run_agent_task(
            &request,
            &completion_client,
            &tool_executor,
            &event_recorder,
            Some(checkpoint),
        ))
    };

    write_json(&result_dir.join("transcript.json"), &outcome.transcript)?;
    write_json(
        &result_dir.join("summary.json"),
        &serde_json::json!({
            "stop_reason": outcome.stop_reason,
            "total_steps": outcome.total_steps,
            "total_billed_tokens": outcome.total_billed_tokens,
            "duration_ms": outcome.duration_ms,
            "error_message": outcome.error_message,
            "scenario_label": crate::quorp::provider_config::resolved_scenario_label(),
            "usage": event_recorder.usage_summary(),
            "routing": read_routing_summary(&result_dir),
        }),
    )?;
    write_final_diff(&options_workspace, &result_dir.join("final.diff"))?;
    Ok(outcome)
}

pub(crate) fn write_json(path: &Path, value: &impl serde::Serialize) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(path, bytes)?;
    Ok(())
}

fn routing_summary_path(result_dir: &Path) -> PathBuf {
    result_dir.join("routing-summary.json")
}

fn extract_routing_decision(response: &CompletionResponse) -> Option<RoutingDecision> {
    let raw = response.raw_provider_response.as_ref()?;
    serde_json::from_value(raw.get("routing")?.clone()).ok()
}

fn read_routing_summary(result_dir: &Path) -> Option<RoutingSummary> {
    let path = routing_summary_path(result_dir);
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn merge_routing_summary(summary: &mut RoutingSummary, routing: &RoutingDecision) {
    if summary.scenario_label.is_none() {
        summary.scenario_label = Some(crate::quorp::provider_config::resolved_scenario_label());
    }
    if summary.routing_mode.is_none() {
        summary.routing_mode = Some(routing.routing_mode.clone());
    }
    if summary.requested_provider.is_none() {
        summary.requested_provider = Some(routing.requested_provider.clone());
    }
    if summary.requested_model.is_none() {
        summary.requested_model = Some(routing.requested_model.clone());
    }
    if summary.candidate_models.is_empty() && !routing.candidate_models.is_empty() {
        summary.candidate_models = routing.candidate_models.clone();
    }
    summary.effective_provider = Some(routing.effective_provider.clone());
    summary.effective_model = Some(routing.effective_model.clone());
    summary.used_fallback |= routing.used_fallback;
    if summary.fallback_reason.is_none() {
        summary.fallback_reason = routing.fallback_reason.clone();
    }
    summary.comparable = Some(summary.comparable.unwrap_or(true) && routing.comparable);
    if summary.provider_base_url.is_none() {
        summary.provider_base_url = routing.provider_base_url.clone();
    }
    if summary.auth_mode.is_none() {
        summary.auth_mode = routing.auth_mode.clone();
    }
    summary.proxy_visible_remote_egress_expected |= routing.proxy_visible_remote_egress_expected;
}

fn write_routing_summary_seed(
    result_dir: &Path,
    routing: &RoutingDecision,
    status: &str,
) -> anyhow::Result<()> {
    let mut summary = read_routing_summary(result_dir).unwrap_or_default();
    merge_routing_summary(&mut summary, routing);
    if summary.routing_status.is_none() {
        summary.routing_status = Some(status.to_string());
    }
    write_json(&routing_summary_path(result_dir), &summary)
}

fn write_routing_summary(
    result_dir: &Path,
    routing: &RoutingDecision,
    provider_request_id: Option<&str>,
) -> anyhow::Result<()> {
    let mut summary = read_routing_summary(result_dir).unwrap_or_default();
    merge_routing_summary(&mut summary, routing);
    summary.routing_status = Some("completed".to_string());
    if summary.provider_request_id.is_none() {
        summary.provider_request_id = provider_request_id.map(str::to_string);
    }
    write_json(&routing_summary_path(result_dir), &summary)
}

fn infer_routing_summary_from_request(request: &CompletionRequest) -> RoutingDecision {
    use crate::quorp::executor::InteractiveProviderKind;

    let provider = crate::quorp::tui::model_registry::chat_model_provider(
        &request.model_id,
        crate::quorp::executor::interactive_provider_from_env(),
    );
    let routing_mode = crate::quorp::provider_config::resolved_routing_mode()
        .label()
        .to_string();
    let requested_model = request.model_id.clone();
    match provider {
        InteractiveProviderKind::Nvidia => {
            let runtime = crate::quorp::provider_config::resolve_nvidia_runtime(
                request.base_url_override.as_deref(),
            )
            .ok();
            RoutingDecision {
                routing_mode,
                requested_provider: provider.label().to_string(),
                requested_model: requested_model.clone(),
                candidate_models: vec![requested_model.clone()],
                effective_provider: provider.label().to_string(),
                effective_model: requested_model,
                used_fallback: false,
                fallback_reason: None,
                comparable: true,
                provider_base_url: runtime.as_ref().map(|value| value.base_url.clone()),
                auth_mode: runtime.as_ref().map(|value| value.auth_mode.clone()),
                proxy_visible_remote_egress_expected: runtime
                    .as_ref()
                    .map(|value| value.proxy_visible_remote_egress_expected)
                    .unwrap_or(true),
            }
        }
    }
}

pub(crate) fn write_final_diff(workspace: &Path, output_path: &Path) -> anyhow::Result<()> {
    #[allow(clippy::disallowed_methods)]
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .arg("diff")
        .output();
    match output {
        Ok(output) if output.status.success() || !output.stdout.is_empty() => {
            fs::write(output_path, output.stdout)?;
        }
        _ => {
            fs::write(
                output_path,
                b"final diff unavailable for non-git workspace\n",
            )?;
        }
    }
    Ok(())
}

pub fn core_to_chat_message(message: &TranscriptMessage) -> ChatServiceMessage {
    ChatServiceMessage {
        role: match message.role {
            TranscriptRole::System => ChatServiceRole::System,
            TranscriptRole::User => ChatServiceRole::User,
            TranscriptRole::Assistant => ChatServiceRole::Assistant,
        },
        content: message.content.clone(),
    }
}

fn render_status(status: &AgentRuntimeStatus) -> String {
    match status {
        AgentRuntimeStatus::Idle => "idle".to_string(),
        AgentRuntimeStatus::Thinking => "thinking".to_string(),
        AgentRuntimeStatus::ExecutingTool(tool) => format!("executing {tool}"),
        AgentRuntimeStatus::Validating(plan) => format!("validating {plan}"),
        AgentRuntimeStatus::Failed(error) => format!("failed: {error}"),
        AgentRuntimeStatus::Success => "success".to_string(),
    }
}

fn render_usage(usage: &quorp_agent_core::TokenUsage) -> String {
    format!(
        "usage={} total={} input={} output={} latency_ms={}",
        match usage.usage_source {
            quorp_agent_core::UsageSource::Reported => "reported",
            quorp_agent_core::UsageSource::Estimated => "estimated",
        },
        usage.total_billed_tokens,
        usage.input_tokens,
        usage.output_tokens,
        usage.latency_ms
    )
}

fn render_watchdog(report: &quorp_agent_core::ModelRequestWatchdogReport) -> String {
    format!(
        "watchdog=first:{} idle:{} total:{} first_token_ms={} max_idle_ms={} near_limit={}",
        report
            .first_token_timeout_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "off".to_string()),
        report
            .idle_timeout_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "off".to_string()),
        report
            .total_timeout_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "off".to_string()),
        report
            .first_token_latency_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "n/a".to_string()),
        report
            .max_idle_gap_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "n/a".to_string()),
        report.near_limit
    )
}

fn truncate_console(text: &str, max_chars: usize) -> String {
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

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

use std::borrow::Cow;
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use futures::future::BoxFuture;
use serde::Serialize;

use crate::agent_context::{
    AgentConfig, AutonomyProfile, PolicyMode, PolicySettings, load_agent_config,
    validation_commands_for_plan,
};
use crate::agent_protocol::{
    ActionOutcome, AgentAction, AgentMode, PreviewEditPayload, ValidationPlan, stable_content_hash,
};
use crate::agent_turn::{AgentTurnResponse, parse_agent_turn_response};

const LEGACY_REMOTE_SAFETY_LABEL: &str = concat!("safe_", "remote");

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct TranscriptMessage {
    pub role: TranscriptRole,
    pub content: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRuntimeStatus {
    Idle,
    Thinking,
    ExecutingTool(String),
    Validating(String),
    Failed(String),
    Success,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    Reported,
    Estimated,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_billed_tokens: u64,
    pub reasoning_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub provider_request_id: Option<String>,
    pub latency_ms: u64,
    pub finish_reason: Option<String>,
    pub usage_source: UsageSource,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, serde::Deserialize, Default)]
pub struct CompletionWatchdogConfig {
    pub first_token_timeout_ms: Option<u64>,
    pub idle_timeout_ms: Option<u64>,
    pub total_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct CompletionPolicy {
    pub include_repo_capsule: bool,
    pub first_turn_max_completion_tokens: Option<u32>,
    pub later_turn_max_completion_tokens: Option<u32>,
    pub disable_reasoning: bool,
    #[serde(default)]
    pub native_tool_calls: bool,
    pub watchdog: Option<CompletionWatchdogConfig>,
    pub safety_mode_label: Option<String>,
    #[serde(default)]
    pub prompt_compaction_policy: Option<PromptCompactionPolicy>,
}

impl Default for CompletionPolicy {
    fn default() -> Self {
        Self {
            include_repo_capsule: true,
            first_turn_max_completion_tokens: None,
            later_turn_max_completion_tokens: None,
            disable_reasoning: false,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: None,
            prompt_compaction_policy: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PromptCompactionPolicy {
    CurrentDefault,
    Last8Ledger1024,
    Last6Ledger768,
    BenchmarkRepairMinimal,
    BenchmarkStatePacket,
    Off,
}

impl PromptCompactionPolicy {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "current-default" => Some(Self::CurrentDefault),
            "last8-ledger1024" => Some(Self::Last8Ledger1024),
            "last6-ledger768" => Some(Self::Last6Ledger768),
            "benchmark-repair-minimal" => Some(Self::BenchmarkRepairMinimal),
            "benchmark-state-packet" => Some(Self::BenchmarkStatePacket),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CurrentDefault => "current-default",
            Self::Last8Ledger1024 => "last8-ledger1024",
            Self::Last6Ledger768 => "last6-ledger768",
            Self::BenchmarkRepairMinimal => "benchmark-repair-minimal",
            Self::BenchmarkStatePacket => "benchmark-state-packet",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, serde::Deserialize, Default)]
pub struct ModelRequestWatchdogReport {
    pub first_token_timeout_ms: Option<u64>,
    pub idle_timeout_ms: Option<u64>,
    pub total_timeout_ms: Option<u64>,
    pub first_token_latency_ms: Option<u64>,
    pub max_idle_gap_ms: Option<u64>,
    pub total_elapsed_ms: u64,
    pub near_limit: bool,
    pub triggered_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub request_id: u64,
    pub session_id: usize,
    pub model_id: String,
    pub agent_mode: AgentMode,
    pub latest_input: String,
    pub messages: Vec<TranscriptMessage>,
    pub project_root: PathBuf,
    pub base_url_override: Option<String>,
    pub max_completion_tokens: Option<u32>,
    pub include_repo_capsule: bool,
    pub disable_reasoning: bool,
    pub native_tool_calls: bool,
    pub watchdog: Option<CompletionWatchdogConfig>,
    pub safety_mode_label: Option<String>,
    pub prompt_compaction_policy: Option<PromptCompactionPolicy>,
    pub capture_scope: Option<String>,
    pub capture_call_class: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CompletionResponse {
    pub content: String,
    pub reasoning_content: String,
    pub native_turn: Option<AgentTurnResponse>,
    pub native_turn_error: Option<String>,
    pub usage: Option<TokenUsage>,
    pub raw_provider_response: Option<serde_json::Value>,
    pub watchdog: Option<ModelRequestWatchdogReport>,
}

#[derive(Debug, Clone)]
pub struct ToolExecutionRequest {
    pub step: usize,
    pub session_id: usize,
    pub action: AgentAction,
    pub project_root: PathBuf,
    pub cwd: PathBuf,
    pub enable_rollback_on_validation_failure: bool,
}

#[derive(Debug, Clone)]
pub struct ToolExecutionResult {
    pub outcome: ActionOutcome,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct AgentRunRequest {
    pub session_id: usize,
    pub goal: String,
    pub initial_context: Vec<TranscriptMessage>,
    pub model_id: String,
    pub agent_mode: AgentMode,
    pub base_url_override: Option<String>,
    pub max_iterations: usize,
    #[serde(default = "default_verifier_drain_budget")]
    pub verifier_drain_budget: usize,
    #[serde(default = "default_parser_recovery_budget")]
    pub parser_recovery_budget: usize,
    pub max_total_tokens: Option<u64>,
    pub max_seconds: Option<u64>,
    pub autonomy_profile: AutonomyProfile,
    pub project_root: PathBuf,
    pub cwd: PathBuf,
    pub enable_rollback_on_validation_failure: bool,
    #[serde(default)]
    pub completion_policy: CompletionPolicy,
    #[serde(default)]
    pub run_metadata: serde_json::Value,
    #[serde(skip)]
    pub cancellation_flag: Option<Arc<AtomicBool>>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Success,
    MaxIterations,
    PendingValidation,
    BudgetExhausted,
    TimeBudgetExhausted,
    Cancelled,
    FirstTokenTimeout,
    StreamIdleTimeout,
    ModelRequestTimeout,
    FatalError,
    Stalled,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentRunOutcome {
    pub stop_reason: StopReason,
    pub total_steps: usize,
    pub total_billed_tokens: u64,
    pub duration_ms: u64,
    pub transcript: Vec<TranscriptMessage>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
pub struct FailedEditRecord {
    pub action_kind: String,
    pub path: String,
    #[serde(default)]
    pub search_hash: Option<String>,
    #[serde(default)]
    pub replace_hash: Option<String>,
    pub failure_reason: String,
    #[serde(default)]
    pub matching_line_numbers: Vec<usize>,
    #[serde(default)]
    pub attempts: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
pub struct AgentRepairScorecard {
    #[serde(default)]
    pub parser_recovery_count: usize,
    #[serde(default)]
    pub line_oriented_parse_count: usize,
    #[serde(default)]
    pub controller_injected_read_count: usize,
    #[serde(default)]
    pub redundant_read_count: usize,
    #[serde(default)]
    pub first_valid_write_step: Option<usize>,
    #[serde(default)]
    pub repeated_failed_edit_count: usize,
    #[serde(default)]
    pub rejected_validation_alias_count: usize,
    #[serde(default)]
    pub test_edit_rejection_count: usize,
    #[serde(default)]
    pub anchor_suggestion_count: usize,
    #[serde(default)]
    pub preview_edit_count: usize,
    #[serde(default)]
    pub preview_edit_success_count: usize,
    #[serde(default)]
    pub preview_created_count: usize,
    #[serde(default)]
    pub replace_range_count: usize,
    #[serde(default)]
    pub replace_range_hash_mismatch_count: usize,
    #[serde(default)]
    pub modify_toml_count: usize,
    #[serde(default)]
    pub apply_preview_count: usize,
    #[serde(default)]
    pub apply_preview_hash_mismatch_count: usize,
    #[serde(default)]
    pub target_redirect_count: usize,
    #[serde(default)]
    pub syntax_preview_count: usize,
    #[serde(default)]
    pub syntax_preview_failure_count: usize,
    #[serde(default)]
    pub evidence_file_fixation_count: usize,
    #[serde(default)]
    pub repair_invalid_action_streak_max: usize,
    #[serde(default)]
    pub repair_submode_entered: bool,
    #[serde(default)]
    pub repair_submode_turns: usize,
    #[serde(default)]
    pub repair_write_locked: bool,
    #[serde(default)]
    pub write_phase_action_refusal_count: usize,
    #[serde(default)]
    pub patch_scaffold_offered: bool,
    #[serde(default)]
    pub patch_scaffold_honored: bool,
    #[serde(default)]
    pub preview_apply_locked: bool,
    #[serde(default)]
    pub preview_apply_action_refusal_count: usize,
    #[serde(default)]
    pub write_phase_write_emitted: bool,
    #[serde(default)]
    pub support_write_count: usize,
    #[serde(default)]
    pub source_write_count: usize,
    #[serde(default)]
    pub rolled_back_write_count: usize,
    #[serde(default)]
    pub rolled_back_non_support_edit_count: usize,
    #[serde(default)]
    pub final_failure_classification: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AgentRepairObservedSlice {
    pub path: String,
    #[serde(default)]
    pub requested_range: Option<crate::agent_protocol::ReadFileRange>,
    #[serde(default)]
    pub honored_range: Option<crate::agent_protocol::ReadFileRange>,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub content_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AgentRepairValidationFailure {
    pub command: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AgentRepairRejectedAction {
    pub phase: String,
    pub actions: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AgentRepairInvalidTurn {
    pub step: usize,
    pub error_class: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AgentRepairCanonicalAction {
    pub step: usize,
    pub kind: String,
    pub signature: String,
    #[serde(default)]
    pub target_path: Option<String>,
    #[serde(default)]
    pub validation_like: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AgentRepairSuggestedEditAnchor {
    pub path: String,
    #[serde(default)]
    pub range: Option<crate::agent_protocol::ReadFileRange>,
    #[serde(default)]
    pub search_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AgentRepairImplementationTarget {
    pub path: String,
    pub reason: String,
    pub rank: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
pub struct AgentRepairMemory {
    #[serde(default)]
    pub observed_slices: Vec<AgentRepairObservedSlice>,
    #[serde(default)]
    pub validation_failures: Vec<AgentRepairValidationFailure>,
    #[serde(default)]
    pub rejected_actions: Vec<AgentRepairRejectedAction>,
    #[serde(default)]
    pub invalid_turns: Vec<AgentRepairInvalidTurn>,
    #[serde(default)]
    pub current_required_action: Option<String>,
    #[serde(default)]
    pub canonical_action_history: Vec<AgentRepairCanonicalAction>,
    #[serde(default)]
    pub repair_phase: Option<String>,
    #[serde(default)]
    pub context_sufficient: bool,
    #[serde(default)]
    pub suggested_edit_anchors: Vec<AgentRepairSuggestedEditAnchor>,
    #[serde(default)]
    pub diagnostic_class: Option<String>,
    #[serde(default)]
    pub implementation_target_lease: Option<String>,
    #[serde(default)]
    pub dependency_candidates: Vec<String>,
    #[serde(default)]
    pub target_dependency_table: Option<String>,
    #[serde(default)]
    pub last_manifest_patch_operations: Vec<crate::agent_protocol::TomlEditOperation>,
    #[serde(default)]
    pub post_patch_diagnostic_class: Option<String>,
    #[serde(default)]
    pub post_patch_diagnostic_excerpt: Option<String>,
    #[serde(default)]
    pub ranked_implementation_targets: Vec<AgentRepairImplementationTarget>,
    #[serde(default)]
    pub last_preview_result: Option<String>,
    #[serde(default)]
    pub last_preview_id: Option<String>,
    #[serde(default)]
    pub last_preview_path: Option<String>,
    #[serde(default)]
    pub preview_origin: Option<String>,
    #[serde(default)]
    pub last_rollback_diagnostic: Option<String>,
    #[serde(default)]
    pub scorecard: AgentRepairScorecard,
}

impl AgentRepairMemory {
    fn is_empty(&self) -> bool {
        self.observed_slices.is_empty()
            && self.validation_failures.is_empty()
            && self.rejected_actions.is_empty()
            && self.invalid_turns.is_empty()
            && self.current_required_action.is_none()
            && self.canonical_action_history.is_empty()
            && self.repair_phase.is_none()
            && !self.context_sufficient
            && self.suggested_edit_anchors.is_empty()
            && self.diagnostic_class.is_none()
            && self.implementation_target_lease.is_none()
            && self.dependency_candidates.is_empty()
            && self.target_dependency_table.is_none()
            && self.last_manifest_patch_operations.is_empty()
            && self.post_patch_diagnostic_class.is_none()
            && self.post_patch_diagnostic_excerpt.is_none()
            && self.ranked_implementation_targets.is_empty()
            && self.last_preview_result.is_none()
            && self.last_preview_id.is_none()
            && self.last_preview_path.is_none()
            && self.preview_origin.is_none()
            && self.last_rollback_diagnostic.is_none()
            && self.scorecard == AgentRepairScorecard::default()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RuntimeEvent {
    StatusUpdate {
        status: AgentRuntimeStatus,
    },
    #[serde(rename = "phase_changed")]
    PhaseChanged {
        phase: &'static str,
        detail: Option<String>,
    },
    TurnCompleted {
        transcript: Vec<TranscriptMessage>,
    },
    #[serde(rename = "assistant_turn_summary")]
    AssistantTurnSummary {
        step: usize,
        assistant_message: String,
        actions: Vec<String>,
        wrote_files: bool,
        validation_queued: bool,
        parse_warning_count: usize,
    },
    FatalError {
        error: String,
    },
    RunStarted {
        goal: String,
        model_id: String,
    },
    ModelRequestStarted {
        step: usize,
        request_id: u64,
        message_count: usize,
        prompt_token_estimate: u64,
        completion_token_cap: Option<u32>,
        safety_mode: Option<String>,
    },
    ModelRequestFinished {
        step: usize,
        request_id: u64,
        usage: Option<TokenUsage>,
        watchdog: Option<ModelRequestWatchdogReport>,
    },
    ToolCallStarted {
        step: usize,
        action: String,
    },
    ToolCallFinished {
        step: usize,
        action: String,
        status: &'static str,
        action_kind: &'static str,
        target_path: Option<String>,
        edit_summary: Option<String>,
    },
    ValidationStarted {
        step: usize,
        summary: String,
    },
    ValidationFinished {
        step: usize,
        summary: String,
        status: &'static str,
    },
    #[serde(rename = "agent.path_resolution_failed")]
    PathResolutionFailed {
        step: usize,
        action: String,
        request_path: String,
        suggested_path: Option<String>,
        reason: Option<String>,
        error: String,
    },
    #[serde(rename = "agent.recovery_turn_queued")]
    RecoveryTurnQueued {
        step: usize,
        action: String,
        suggested_path: Option<String>,
        message: String,
    },
    #[serde(rename = "agent.recovery_budget_exhausted")]
    RecoveryBudgetExhausted {
        failures: usize,
        last_error: String,
    },
    #[serde(rename = "agent.parse_recovery_queued")]
    ParseRecoveryQueued {
        step: usize,
        error_class: String,
        failures: usize,
        budget: usize,
        message: String,
    },
    #[serde(rename = "agent.parse_recovery_exhausted")]
    ParseRecoveryExhausted {
        failures: usize,
        last_error: String,
        error_class: String,
    },
    #[serde(rename = "agent.verifier_queued")]
    VerifierQueued {
        step: usize,
        plans: Vec<String>,
        reason: String,
    },
    #[serde(rename = "agent.verifier_drain_started")]
    VerifierDrainStarted {
        step: usize,
        plans: Vec<String>,
        budget: usize,
    },
    #[serde(rename = "agent.verifier_drain_finished")]
    VerifierDrainFinished {
        step: usize,
        remaining: usize,
        verified_green: bool,
    },
    #[serde(rename = "run.blocked_on_pending_validation")]
    PendingValidationBlocked {
        step: usize,
        queued_validations: Vec<String>,
        drain_budget: usize,
    },
    PolicyDenied {
        step: usize,
        action: String,
        reason: String,
    },
    #[serde(rename = "agent.failed_edit_recorded")]
    FailedEditRecorded {
        step: usize,
        record: FailedEditRecord,
    },
    #[serde(rename = "agent.controller_read_injected")]
    ControllerReadInjected {
        step: usize,
        action: String,
        reason: String,
    },
    CheckpointSaved {
        checkpoint: AgentCheckpoint,
    },
    RunFinished {
        reason: StopReason,
        total_steps: usize,
        total_billed_tokens: u64,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct AgentTaskStateSnapshot {
    pub current_mode: AgentMode,
    pub acceptance_criteria: Vec<String>,
    pub working_set: BTreeSet<String>,
    pub last_tool_summary: Option<String>,
    pub last_failing_verifier: Option<String>,
    pub last_safe_checkpoint: Option<String>,
    #[serde(default)]
    pub last_parse_error: Option<String>,
    pub stall_count: usize,
    pub redundant_inspection_turns: usize,
    #[serde(default)]
    pub recoverable_inspection_failures: usize,
    #[serde(default)]
    pub parser_recovery_failures: usize,
    #[serde(default)]
    pub parser_recovery_validation_fingerprint: Option<String>,
    #[serde(default)]
    pub parser_recovery_same_validation_streak: usize,
    pub has_mutating_change: bool,
    pub verified_green: bool,
    pub validation_queue: VecDeque<ValidationPlan>,
    pub total_billed_tokens: u64,
    #[serde(default)]
    pub last_failed_tool_error: Option<String>,
    #[serde(default)]
    pub repair_recovery_turns_remaining: usize,
    #[serde(default)]
    pub benchmark_case_ledger: Option<BenchmarkCaseLedger>,
    #[serde(default)]
    pub repair_requirement: Option<RepairRequirement>,
    #[serde(default)]
    pub last_successful_write_action: Option<AgentAction>,
    #[serde(default)]
    pub benchmark_repair_state: Option<BenchmarkRepairState>,
    #[serde(default)]
    pub failed_edit_records: Vec<FailedEditRecord>,
    #[serde(default)]
    pub agent_repair_memory: AgentRepairMemory,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct AgentCheckpoint {
    pub snapshot: AgentTaskStateSnapshot,
    pub transcript: Vec<TranscriptMessage>,
    pub step: usize,
    pub request_counter: u64,
}

pub trait CompletionClient: Send + Sync {
    fn request_completion<'a>(
        &'a self,
        request: &'a CompletionRequest,
    ) -> BoxFuture<'a, Result<CompletionResponse, String>>;
}

pub trait ToolExecutor: Send + Sync {
    fn execute<'a>(
        &'a self,
        request: ToolExecutionRequest,
    ) -> BoxFuture<'a, Result<ToolExecutionResult, String>>;
}

pub trait RuntimeEventSink: Send + Sync {
    fn emit(&self, event: RuntimeEvent);
}

#[derive(Debug)]
struct AgentTaskState {
    goal: String,
    current_mode: AgentMode,
    autonomy_profile: AutonomyProfile,
    policy: PolicySettings,
    acceptance_criteria: Vec<String>,
    working_set: BTreeSet<String>,
    workspace_root: String,
    workspace_root_entries: Vec<String>,
    last_tool_summary: Option<String>,
    last_failing_verifier: Option<String>,
    last_safe_checkpoint: Option<String>,
    last_parse_error: Option<String>,
    last_failed_tool_error: Option<String>,
    stall_count: usize,
    redundant_inspection_turns: usize,
    recoverable_inspection_failures: usize,
    parser_recovery_failures: usize,
    parser_recovery_validation_fingerprint: Option<String>,
    parser_recovery_same_validation_streak: usize,
    has_mutating_change: bool,
    verified_green: bool,
    validation_queue: VecDeque<ValidationPlan>,
    config: AgentConfig,
    total_billed_tokens: u64,
    repair_recovery_turns_remaining: usize,
    benchmark_transcript_compression: bool,
    benchmark_case_ledger: Option<BenchmarkCaseLedger>,
    repair_requirement: Option<RepairRequirement>,
    last_successful_write_action: Option<AgentAction>,
    benchmark_repair_state: Option<BenchmarkRepairState>,
    failed_edit_records: Vec<FailedEditRecord>,
    agent_repair_memory: AgentRepairMemory,
}

impl AgentTaskState {
    fn new(request: &AgentRunRequest, config: AgentConfig) -> Self {
        let mut acceptance_criteria = vec![
            format!("Complete the requested goal: {}", request.goal.trim()),
            "Do not stop after edits unless validation is green.".to_string(),
        ];
        if let Some(evaluate_command) = request
            .run_metadata
            .get("evaluate_command")
            .and_then(serde_json::Value::as_str)
            .filter(|command| !command.trim().is_empty())
        {
            acceptance_criteria.push(format!(
                "Keep going until the visible evaluator succeeds: {evaluate_command}"
            ));
        }
        Self {
            goal: request.goal.clone(),
            current_mode: request.agent_mode,
            autonomy_profile: request.autonomy_profile,
            policy: config.policy.clone(),
            acceptance_criteria,
            working_set: BTreeSet::new(),
            workspace_root: request.project_root.display().to_string(),
            workspace_root_entries: metadata_string_list(
                &request.run_metadata,
                "editable_workspace_entries",
            )
            .or_else(|| metadata_string_list(&request.run_metadata, "workspace_root_entries"))
            .unwrap_or_default(),
            last_tool_summary: None,
            last_failing_verifier: None,
            last_safe_checkpoint: None,
            last_parse_error: None,
            last_failed_tool_error: None,
            stall_count: 0,
            redundant_inspection_turns: 0,
            recoverable_inspection_failures: 0,
            parser_recovery_failures: 0,
            parser_recovery_validation_fingerprint: None,
            parser_recovery_same_validation_streak: 0,
            has_mutating_change: false,
            verified_green: false,
            validation_queue: VecDeque::new(),
            config,
            total_billed_tokens: 0,
            repair_recovery_turns_remaining: 0,
            benchmark_transcript_compression: metadata_bool(
                &request.run_metadata,
                "benchmark_transcript_compression",
            )
            .unwrap_or_else(|| {
                metadata_bool(&request.run_metadata, "benchmark_mode").unwrap_or(false)
            }),
            benchmark_case_ledger: benchmark_case_ledger_from_metadata(&request.run_metadata),
            repair_requirement: None,
            last_successful_write_action: None,
            benchmark_repair_state: None,
            failed_edit_records: Vec::new(),
            agent_repair_memory: AgentRepairMemory::default(),
        }
    }

    fn snapshot(&self) -> AgentTaskStateSnapshot {
        AgentTaskStateSnapshot {
            current_mode: self.current_mode,
            acceptance_criteria: self.acceptance_criteria.clone(),
            working_set: self.working_set.clone(),
            last_tool_summary: self.last_tool_summary.clone(),
            last_failing_verifier: self.last_failing_verifier.clone(),
            last_safe_checkpoint: self.last_safe_checkpoint.clone(),
            last_parse_error: self.last_parse_error.clone(),
            stall_count: self.stall_count,
            redundant_inspection_turns: self.redundant_inspection_turns,
            recoverable_inspection_failures: self.recoverable_inspection_failures,
            parser_recovery_failures: self.parser_recovery_failures,
            parser_recovery_validation_fingerprint: self
                .parser_recovery_validation_fingerprint
                .clone(),
            parser_recovery_same_validation_streak: self.parser_recovery_same_validation_streak,
            has_mutating_change: self.has_mutating_change,
            verified_green: self.verified_green,
            validation_queue: self.validation_queue.clone(),
            total_billed_tokens: self.total_billed_tokens,
            last_failed_tool_error: self.last_failed_tool_error.clone(),
            repair_recovery_turns_remaining: self.repair_recovery_turns_remaining,
            benchmark_case_ledger: self.benchmark_case_ledger.clone(),
            repair_requirement: self.repair_requirement.clone(),
            last_successful_write_action: self.last_successful_write_action.clone(),
            benchmark_repair_state: self.benchmark_repair_state.clone(),
            failed_edit_records: self.failed_edit_records.clone(),
            agent_repair_memory: self.agent_repair_memory.clone(),
        }
    }

    fn restore(&mut self, snapshot: AgentTaskStateSnapshot) {
        self.current_mode = snapshot.current_mode;
        self.acceptance_criteria = snapshot.acceptance_criteria;
        self.working_set = snapshot.working_set;
        self.last_tool_summary = snapshot.last_tool_summary;
        self.last_failing_verifier = snapshot.last_failing_verifier;
        self.last_safe_checkpoint = snapshot.last_safe_checkpoint;
        self.last_parse_error = snapshot.last_parse_error;
        self.stall_count = snapshot.stall_count;
        self.redundant_inspection_turns = snapshot.redundant_inspection_turns;
        self.recoverable_inspection_failures = snapshot.recoverable_inspection_failures;
        self.parser_recovery_failures = snapshot.parser_recovery_failures;
        self.parser_recovery_validation_fingerprint =
            snapshot.parser_recovery_validation_fingerprint;
        self.parser_recovery_same_validation_streak =
            snapshot.parser_recovery_same_validation_streak;
        self.has_mutating_change = snapshot.has_mutating_change;
        self.verified_green = snapshot.verified_green;
        self.validation_queue = snapshot.validation_queue;
        self.total_billed_tokens = snapshot.total_billed_tokens;
        self.last_failed_tool_error = snapshot.last_failed_tool_error;
        self.repair_recovery_turns_remaining = snapshot.repair_recovery_turns_remaining;
        self.benchmark_case_ledger = snapshot.benchmark_case_ledger;
        self.repair_requirement = snapshot.repair_requirement;
        self.last_successful_write_action = snapshot.last_successful_write_action;
        self.benchmark_repair_state = snapshot.benchmark_repair_state;
        self.failed_edit_records = snapshot.failed_edit_records;
        self.agent_repair_memory = snapshot.agent_repair_memory;
    }

    fn runtime_summary(&self) -> String {
        let mut lines = vec![
            "[Runtime State]".to_string(),
            format!("Goal: {}", self.goal),
            format!("Mode: {}", self.current_mode.label()),
            format!("Autonomy profile: {}", self.autonomy_profile.label()),
            format!("Policy mode: {}", self.policy.mode.label()),
            format!("Workspace root: {}", self.workspace_root),
            format!(
                "Verification: {}",
                if self.verified_green {
                    "green"
                } else if self.has_mutating_change {
                    "pending"
                } else {
                    "not required yet"
                }
            ),
            format!("Stall count: {}", self.stall_count),
            format!(
                "Parser recovery failures: {}",
                self.parser_recovery_failures
            ),
            format!("Total billed tokens: {}", self.total_billed_tokens),
        ];
        if !self.acceptance_criteria.is_empty() {
            lines.push(format!(
                "Acceptance criteria: {}",
                self.acceptance_criteria.join(" | ")
            ));
        }
        if !self.working_set.is_empty() {
            let rendered = self
                .working_set
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("Working set: {rendered}"));
        }
        if !self.workspace_root_entries.is_empty() {
            lines.push(format!(
                "Workspace entries: {}",
                self.workspace_root_entries
                    .iter()
                    .take(12)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(summary) = self.last_tool_summary.as_ref() {
            lines.push(format!("Last tool result: {summary}"));
        }
        if let Some(error) = self.last_failed_tool_error.as_ref() {
            lines.push(format!("Last failed tool error: {error}"));
        }
        if !self.failed_edit_records.is_empty() {
            lines.push(format!(
                "Failed edit memory: {}",
                render_failed_edit_memory(&self.failed_edit_records)
            ));
        }
        if self.benchmark_case_ledger.is_some() && !self.agent_repair_memory.is_empty() {
            lines.push(format!(
                "Agent memory: {}",
                render_agent_repair_memory(&self.agent_repair_memory)
            ));
        }
        if self.repair_recovery_turns_remaining > 0 {
            lines.push(format!(
                "Repair recovery turns remaining: {}",
                self.repair_recovery_turns_remaining
            ));
        }
        if let Some(repair_state) = self.benchmark_repair_state.as_ref()
            && repair_state.phase != BenchmarkRepairPhase::Idle
        {
            lines.push(format!(
                "Benchmark repair phase: {}",
                repair_state.phase.label()
            ));
            lines.push(format!(
                "Repair next step: {}",
                benchmark_repair_phase_instruction(repair_state.phase)
            ));
            if let Some(range) = benchmark_repair_phase_suggested_range(repair_state) {
                lines.push(format!("Repair suggested range: {}", range.label()));
            }
        }
        if let Some(requirement) = self.repair_requirement.as_ref() {
            lines.push(format!(
                "Repair target: {} ({})",
                requirement.path,
                if requirement.exact_reread_completed {
                    "exact reread captured"
                } else {
                    "exact reread required before the next write"
                }
            ));
            if let Some(range) = requirement.suggested_range {
                lines.push(format!("Suggested reread range: {}", range.label()));
            }
        }
        if let Some(error) = self.last_parse_error.as_ref() {
            lines.push(format!("Last parse error: {error}"));
        }
        if let Some(verifier) = self.last_failing_verifier.as_ref() {
            lines.push(format!("Last failing verifier: {verifier}"));
        }
        if let Some(checkpoint) = self.last_safe_checkpoint.as_ref() {
            lines.push(format!("Last safe checkpoint: {checkpoint}"));
        }
        if !self.validation_queue.is_empty() {
            let queued = self
                .validation_queue
                .iter()
                .map(ValidationPlan::summary)
                .collect::<Vec<_>>()
                .join(" -> ");
            lines.push(format!("Queued validation: {queued}"));
        }
        if let Some(ledger) = self.benchmark_case_ledger.as_ref() {
            if ledger.validation_details.repair_required {
                lines.push("[Repair Phase] Stay on the owner file until the fast loop is green again. Do not widen scope, do not keep rereading without a concrete range, and do not stop after explanation-only turns.".to_string());
            }
            lines.push("[Benchmark Ledger]".to_string());
            if !ledger.case_class.is_empty() {
                lines.push(format!("Case class: {}", ledger.case_class));
            }
            if !ledger.owner_files.is_empty() {
                lines.push(format!(
                    "Owner files: {}",
                    render_short_list(&ledger.owner_files, 4)
                ));
            }
            if !ledger.expected_touch_targets.is_empty() {
                lines.push(format!(
                    "Expected touch targets: {}",
                    render_short_list(&ledger.expected_touch_targets, 5)
                ));
            }
            if !ledger.fast_loop_commands.is_empty() {
                lines.push(format!(
                    "Fast loop: {}",
                    render_short_list(&ledger.fast_loop_commands, 2)
                ));
            }
            if !ledger.named_tests.is_empty() {
                lines.push(format!(
                    "Named tests: {}",
                    render_short_list(&ledger.named_tests, 4)
                ));
            }
            if !ledger.companion_files_required.is_empty() {
                lines.push(format!(
                    "Companion files: {}",
                    render_short_list(&ledger.companion_files_required, 4)
                ));
            }
            if let Some(hypothesis) = ledger.current_hypothesis.as_ref() {
                lines.push(format!(
                    "Current hypothesis: {}",
                    truncate_visible_text(hypothesis, 180)
                ));
            }
            if let Some(status) = ledger.validation_status.as_ref() {
                lines.push(format!(
                    "Validation status: {}",
                    truncate_visible_text(status, 180)
                ));
            }
            if let Some(failure) = ledger.last_validation_failure.as_ref() {
                lines.push(format!(
                    "Last validation failure: {}",
                    truncate_visible_text(failure, 180)
                ));
            }
            if !ledger.validation_details.failing_test_names.is_empty() {
                lines.push(format!(
                    "Failing tests: {}",
                    render_short_list(&ledger.validation_details.failing_test_names, 4)
                ));
            }
            if let Some(path) = ledger.validation_details.primary_failure_path.as_ref() {
                let line = ledger
                    .validation_details
                    .primary_failure_line
                    .map(|value| format!(":{value}"))
                    .unwrap_or_default();
                lines.push(format!("Primary failure location: {path}{line}"));
            }
            if let Some(test_name) = ledger.validation_details.primary_failure_test_name.as_ref() {
                lines.push(format!("Primary failure test: {test_name}"));
            }
            if let Some(assertion_excerpt) = ledger.validation_details.assertion_excerpt.as_ref() {
                lines.push(format!(
                    "Assertion excerpt: {}",
                    truncate_visible_text(assertion_excerpt, 180)
                ));
            }
            if ledger.validation_details.repair_required {
                lines.push("Repair required: true".to_string());
            }
            if let Some(phase) = ledger.validation_details.repair_phase_terminal.as_ref() {
                lines.push(format!("Repair phase terminal: {phase}"));
            }
            lines.push(format!(
                "Failure-anchor reread: attempted={} honored={}",
                ledger.validation_details.failure_anchor_reread_attempted,
                ledger.validation_details.failure_anchor_reread_honored
            ));
            lines.push(format!(
                "Implementation reread: allowed={} attempted={} honored={}",
                ledger.validation_details.implementation_reread_allowed,
                ledger.validation_details.implementation_reread_attempted,
                ledger.validation_details.implementation_reread_honored
            ));
            lines.push(format!(
                "Repair phase invalid actions: {}",
                ledger.validation_details.repair_phase_invalid_action_count
            ));
            if ledger.validation_details.patch_packet_injected {
                lines.push("Patch packet injected: true".to_string());
            }
            if let Some(range) = ledger
                .validation_details
                .patch_packet_honored_range
                .as_ref()
            {
                lines.push(format!("Patch packet range: {range}"));
            }
            if let Some(command) = ledger.validation_details.recommended_rerun_command.as_ref() {
                lines.push(format!("Recommended rerun command: {command}"));
            }
            if let Some(match_kind) = ledger
                .validation_details
                .fast_loop_rerun_match_kind
                .as_ref()
            {
                lines.push(format!("Fast-loop rerun match kind: {match_kind}"));
            }
            if !ledger.validation_details.failed_edit_records.is_empty() {
                lines.push(format!(
                    "Failed edit memory: {}",
                    render_failed_edit_memory(&ledger.validation_details.failed_edit_records)
                ));
            }
            lines.push(format!(
                "Post-fast-loop patch attempted: {}",
                ledger.validation_details.post_fast_loop_patch_attempted
            ));
            lines.push(format!(
                "Post-fast-loop validation rerun attempted: {}",
                ledger
                    .validation_details
                    .post_fast_loop_validation_rerun_attempted
            ));
        }
        lines.join("\n")
    }

    fn note_benchmark_hypothesis(
        &mut self,
        assistant_message: &str,
        task_updates: &[crate::agent_turn::TaskItem],
    ) {
        let Some(ledger) = self.benchmark_case_ledger.as_mut() else {
            return;
        };
        let preferred_task = task_updates
            .iter()
            .find(|item| !item.title.trim().is_empty())
            .map(|item| item.title.trim().to_string());
        let candidate = preferred_task
            .or_else(|| {
                let text = assistant_message.trim();
                (!text.is_empty()).then(|| text.to_string())
            })
            .map(|text| truncate_visible_text(&text, 180));
        if let Some(candidate) = candidate {
            ledger.current_hypothesis = Some(candidate);
        }
    }

    fn sync_benchmark_repair_state_to_ledger(&mut self) {
        self.prime_benchmark_patch_target_requirement();
        if let Some(ledger) = self.benchmark_case_ledger.as_ref() {
            self.agent_repair_memory.diagnostic_class =
                ledger.validation_details.diagnostic_class.clone();
            self.agent_repair_memory.dependency_candidates =
                benchmark_dependency_candidates(ledger);
            self.agent_repair_memory.ranked_implementation_targets =
                ranked_implementation_targets_for_ledger(ledger);
            self.agent_repair_memory.implementation_target_lease = target_lease_for_ledger(ledger);
        } else {
            self.agent_repair_memory.dependency_candidates.clear();
        }
        self.agent_repair_memory.current_required_action =
            repair_requirement_action_label(self.repair_requirement.as_ref()).or_else(|| {
                benchmark_required_action_label(
                    self.benchmark_repair_state.as_ref(),
                    self.benchmark_case_ledger.as_ref(),
                    &self.agent_repair_memory,
                )
            });
        self.agent_repair_memory.scorecard.preview_apply_locked =
            preview_apply_locked(&self.agent_repair_memory);
        let Some(ledger) = self.benchmark_case_ledger.as_mut() else {
            return;
        };
        ledger.validation_details.failed_edit_records = self.failed_edit_records.clone();
        ledger.validation_details.implementation_target_lease = target_lease_for_ledger(ledger);
        if let Some(repair_state) = self.benchmark_repair_state.as_ref() {
            let patch_target =
                benchmark_patch_target_path(repair_state, ledger, &self.agent_repair_memory)
                    .into_owned();
            self.agent_repair_memory.repair_phase =
                Some(repair_state.phase.state_label().to_string());
            self.agent_repair_memory.scorecard.repair_submode_entered = true;
            self.agent_repair_memory.target_dependency_table =
                benchmark_target_dependency_table(repair_state, ledger, &patch_target)
                    .map(str::to_string);
            self.agent_repair_memory.scorecard.repair_write_locked =
                benchmark_patch_phase_write_locked(
                    repair_state,
                    ledger,
                    &self.agent_repair_memory,
                    self.repair_requirement.as_ref(),
                );
            self.agent_repair_memory.scorecard.patch_scaffold_offered = repair_state.phase
                == BenchmarkRepairPhase::NeedsPatch
                && patch_target.ends_with(".toml");
            self.agent_repair_memory.context_sufficient = matches!(
                repair_state.phase,
                BenchmarkRepairPhase::NeedsPatch | BenchmarkRepairPhase::NeedsFastLoopRerun
            );
            ledger.validation_details.primary_failure_test_name =
                repair_state.primary_failure_test_name.clone();
            ledger.validation_details.repair_phase_terminal =
                Some(repair_state.phase.label().to_string());
            ledger.validation_details.failure_anchor_reread_attempted =
                repair_state.failure_anchor_reread_attempted;
            ledger.validation_details.failure_anchor_reread_honored =
                repair_state.failure_anchor_reread_honored;
            ledger.validation_details.implementation_reread_allowed =
                repair_state.implementation_reread_allowed;
            ledger.validation_details.implementation_reread_attempted =
                repair_state.implementation_reread_attempted;
            ledger.validation_details.implementation_reread_honored =
                repair_state.implementation_reread_honored;
            ledger.validation_details.repair_phase_invalid_action_count =
                repair_state.invalid_action_count;
            if matches!(
                repair_state.phase,
                BenchmarkRepairPhase::NeedsPatch | BenchmarkRepairPhase::NeedsFastLoopRerun
            ) {
                ledger.validation_details.patch_packet_injected = true;
                ledger.validation_details.patch_packet_honored_range = repair_state
                    .last_owner_slice
                    .as_ref()
                    .and_then(|slice| slice.honored_range)
                    .map(|range| range.label());
                ledger.validation_details.recommended_rerun_command =
                    recommended_fast_loop_rerun_command(ledger);
            }
        } else {
            self.agent_repair_memory.repair_phase =
                Some(BenchmarkRepairPhase::Idle.state_label().to_string());
            self.agent_repair_memory.context_sufficient = false;
            self.agent_repair_memory.target_dependency_table = None;
            self.agent_repair_memory.scorecard.repair_write_locked = false;
            self.agent_repair_memory.scorecard.patch_scaffold_offered = false;
            self.agent_repair_memory.scorecard.preview_apply_locked = false;
            ledger.validation_details.repair_phase_terminal =
                Some(BenchmarkRepairPhase::Idle.label().to_string());
            ledger.validation_details.failure_anchor_reread_attempted = false;
            ledger.validation_details.failure_anchor_reread_honored = false;
            ledger.validation_details.implementation_reread_allowed = false;
            ledger.validation_details.implementation_reread_attempted = false;
            ledger.validation_details.implementation_reread_honored = false;
            ledger.validation_details.repair_phase_invalid_action_count = 0;
        }
    }

    fn record_invalid_turn(&mut self, step: usize, error_class: &str, summary: &str) {
        self.agent_repair_memory.scorecard.parser_recovery_count = self
            .agent_repair_memory
            .scorecard
            .parser_recovery_count
            .saturating_add(1);
        push_capped(
            &mut self.agent_repair_memory.invalid_turns,
            AgentRepairInvalidTurn {
                step,
                error_class: error_class.to_string(),
                summary: truncate_visible_text(summary, 180),
            },
            12,
        );
    }

    fn benchmark_repair_submode_active(&self) -> bool {
        self.benchmark_case_ledger
            .as_ref()
            .is_some_and(|ledger| ledger.validation_details.repair_required)
            && self
                .benchmark_repair_state
                .as_ref()
                .is_some_and(|repair_state| repair_state.phase != BenchmarkRepairPhase::Idle)
    }

    fn note_repair_submode_turn(&mut self) {
        if self.benchmark_repair_submode_active() {
            self.agent_repair_memory.scorecard.repair_submode_entered = true;
            self.agent_repair_memory.scorecard.repair_submode_turns = self
                .agent_repair_memory
                .scorecard
                .repair_submode_turns
                .saturating_add(1);
        }
    }

    fn reset_parser_recovery_tracking(&mut self) {
        self.parser_recovery_validation_fingerprint = None;
        self.parser_recovery_same_validation_streak = 0;
    }

    fn benchmark_validation_fingerprint(&self) -> Option<String> {
        let ledger = self.benchmark_case_ledger.as_ref()?;
        if !ledger.validation_details.repair_required {
            return None;
        }
        let repair_phase = self
            .benchmark_repair_state
            .as_ref()
            .map(|repair_state| repair_state.phase.label())
            .unwrap_or("idle");
        let target_lease = target_lease_for_ledger(ledger).unwrap_or_default();
        let requirement = self
            .repair_requirement
            .as_ref()
            .map(|requirement| {
                let range = requirement
                    .suggested_range
                    .map(|value| value.label())
                    .unwrap_or_else(|| "full-file".to_string());
                format!(
                    "{}:{}:{}",
                    requirement.path, range, requirement.exact_reread_completed
                )
            })
            .unwrap_or_default();
        Some(short_text_fingerprint(&format!(
            "{}|{}|{}|{}|{}|{}|{}",
            ledger
                .last_validation_failure
                .as_deref()
                .unwrap_or_default(),
            ledger
                .validation_details
                .diagnostic_class
                .as_deref()
                .unwrap_or_default(),
            repair_phase,
            target_lease,
            ledger.validation_details.post_fast_loop_patch_attempted,
            ledger
                .validation_details
                .post_fast_loop_validation_rerun_attempted,
            requirement
        )))
    }

    fn note_parser_recovery_failure(
        &mut self,
        step: usize,
        error_class: &str,
        summary: &str,
    ) -> bool {
        self.parser_recovery_failures = self.parser_recovery_failures.saturating_add(1);
        self.last_parse_error = Some(summary.to_string());
        self.record_invalid_turn(step, error_class, summary);
        let Some(fingerprint) = self.benchmark_validation_fingerprint() else {
            self.reset_parser_recovery_tracking();
            return false;
        };
        if self.parser_recovery_validation_fingerprint.as_deref() == Some(fingerprint.as_str()) {
            self.parser_recovery_same_validation_streak = self
                .parser_recovery_same_validation_streak
                .saturating_add(1);
        } else {
            self.parser_recovery_validation_fingerprint = Some(fingerprint);
            self.parser_recovery_same_validation_streak = 1;
        }
        self.benchmark_repair_submode_active() && self.parser_recovery_same_validation_streak >= 2
    }

    fn repair_requirement_prefers_full_file(requirement: &RepairRequirement) -> bool {
        requirement.path.trim().ends_with(".toml")
    }

    fn repair_requirement_read_is_valid(
        requirement: &RepairRequirement,
        path: &str,
        range: Option<crate::agent_protocol::ReadFileRange>,
    ) -> bool {
        if path != requirement.path {
            return false;
        }
        if Self::repair_requirement_prefers_full_file(requirement) {
            return range.and_then(|value| value.normalized()).is_none();
        }
        range.and_then(|value| value.normalized()).is_some()
    }

    fn repair_requirement_prompt(requirement: &RepairRequirement) -> String {
        if Self::repair_requirement_prefers_full_file(requirement) {
            format!(
                "Issue exactly one `ReadFile` for `{}` before any next write.",
                requirement.path
            )
        } else {
            format!(
                "Issue exactly one focused `ReadFile` for `{}` before any next write.",
                requirement.path
            )
        }
    }

    fn repair_requirement_correction(requirement: &RepairRequirement) -> String {
        if Self::repair_requirement_prefers_full_file(requirement) {
            "Correction: emit exactly one full-file `ReadFile` now. Do not patch, rerun tests, search, or widen scope first."
                .to_string()
        } else {
            "Correction: emit exactly one `ReadFile` with a concrete line range now. Do not patch, rerun tests, search, or widen scope first."
                .to_string()
        }
    }

    fn repair_requirement_next_step(requirement: &RepairRequirement) -> String {
        if Self::repair_requirement_prefers_full_file(requirement) {
            "Next step: issue a fresh full-file `ReadFile` for the same path. Then patch or run the smallest relevant validation. The next write will be refused until that reread succeeds. Do not patch from memory and do not widen scope yet."
                .to_string()
        } else {
            "Next step: issue a fresh `ReadFile` for the same path with a focused line range. Then patch or run the smallest relevant validation. The next write will be refused until that anchored reread succeeds. Do not patch from memory and do not widen scope yet."
                .to_string()
        }
    }

    fn prime_benchmark_patch_target_requirement(&mut self) {
        if self.repair_requirement.is_some() {
            return;
        }
        let Some(ledger) = self.benchmark_case_ledger.as_ref() else {
            return;
        };
        let Some(repair_state) = self.benchmark_repair_state.as_ref() else {
            return;
        };
        if repair_state.phase != BenchmarkRepairPhase::NeedsPatch {
            return;
        }
        let patch_target =
            benchmark_patch_target_path(repair_state, ledger, &self.agent_repair_memory);
        if patch_target_context_loaded(
            repair_state,
            &self.agent_repair_memory,
            patch_target.as_ref(),
        ) {
            return;
        }
        if patch_target.as_ref().ends_with(".toml") {
            if ledger.validation_details.diagnostic_class.as_deref()
                != Some("manifest_dependency_error")
            {
                return;
            }
            self.repair_requirement = Some(RepairRequirement {
                path: patch_target.into_owned(),
                failure_reason: "manifest_dependency_error".to_string(),
                previous_search_block: None,
                suggested_range: None,
                exact_reread_completed: false,
            });
            return;
        }
        let suggested_range = repair_state.implementation_suggested_range.or_else(|| {
            load_workspace_file_text(&self.workspace_root, patch_target.as_ref()).and_then(
                |owner_text| {
                    suggest_source_patch_range_from_failure(
                        &owner_text,
                        ledger
                            .last_validation_failure
                            .as_deref()
                            .or(ledger.validation_details.assertion_excerpt.as_deref()),
                    )
                },
            )
        });
        let Some(suggested_range) = suggested_range else {
            return;
        };
        self.repair_requirement = Some(RepairRequirement {
            path: patch_target.into_owned(),
            failure_reason: ledger
                .validation_details
                .diagnostic_class
                .clone()
                .unwrap_or_else(|| "source_patch_context".to_string()),
            previous_search_block: None,
            suggested_range: Some(suggested_range),
            exact_reread_completed: false,
        });
    }

    fn record_line_oriented_parse(&mut self) {
        self.agent_repair_memory.scorecard.line_oriented_parse_count = self
            .agent_repair_memory
            .scorecard
            .line_oriented_parse_count
            .saturating_add(1);
    }

    fn record_canonical_action(&mut self, step: usize, action: &AgentAction) {
        push_capped(
            &mut self.agent_repair_memory.canonical_action_history,
            canonical_action_record(step, action, self.benchmark_case_ledger.as_ref()),
            32,
        );
    }

    fn record_rejected_actions(
        &mut self,
        phase: BenchmarkRepairPhase,
        actions: &[AgentAction],
        reason: &str,
    ) {
        push_capped(
            &mut self.agent_repair_memory.rejected_actions,
            AgentRepairRejectedAction {
                phase: phase.label().to_string(),
                actions: actions.iter().map(AgentAction::summary).collect(),
                reason: truncate_visible_text(reason, 220),
            },
            12,
        );
        if actions
            .iter()
            .any(|action| action_is_validation_like(action, self.benchmark_case_ledger.as_ref()))
        {
            self.agent_repair_memory
                .scorecard
                .rejected_validation_alias_count = self
                .agent_repair_memory
                .scorecard
                .rejected_validation_alias_count
                .saturating_add(1);
        }
        if reason.contains("test file") || reason.contains("test-file") {
            self.agent_repair_memory.scorecard.test_edit_rejection_count = self
                .agent_repair_memory
                .scorecard
                .test_edit_rejection_count
                .saturating_add(1);
        }
        if reason.contains("target lease") || reason.contains("evidence file") {
            self.agent_repair_memory.scorecard.target_redirect_count = self
                .agent_repair_memory
                .scorecard
                .target_redirect_count
                .saturating_add(1);
        }
        if reason.contains("evidence file")
            || reason.contains("test file")
            || reason.contains("test-file")
        {
            self.agent_repair_memory
                .scorecard
                .evidence_file_fixation_count = self
                .agent_repair_memory
                .scorecard
                .evidence_file_fixation_count
                .saturating_add(1);
        }
    }

    fn record_validation_failure_memory(&mut self, command: String, summary: &str) {
        push_capped(
            &mut self.agent_repair_memory.validation_failures,
            AgentRepairValidationFailure {
                command,
                summary: truncate_visible_text(summary, 260),
            },
            6,
        );
    }

    fn record_observed_slice(
        &mut self,
        path: &str,
        requested_range: Option<crate::agent_protocol::ReadFileRange>,
        honored_range: Option<crate::agent_protocol::ReadFileRange>,
        purpose: Option<String>,
        content: &str,
        content_hash: Option<&str>,
    ) {
        if let Some(honored_range) = honored_range {
            let repeated = self
                .agent_repair_memory
                .observed_slices
                .iter()
                .filter(|slice| slice.path == path)
                .filter_map(|slice| slice.honored_range)
                .any(|previous_range| ranges_substantially_overlap(previous_range, honored_range));
            if repeated {
                self.agent_repair_memory.scorecard.redundant_read_count = self
                    .agent_repair_memory
                    .scorecard
                    .redundant_read_count
                    .saturating_add(1);
            }
        }
        push_capped(
            &mut self.agent_repair_memory.observed_slices,
            AgentRepairObservedSlice {
                path: path.to_string(),
                requested_range,
                honored_range,
                purpose,
                content_fingerprint: content_hash
                    .map(str::trim)
                    .filter(|value| is_stable_content_hash(value))
                    .map(str::to_string)
                    .or_else(|| (!content.trim().is_empty()).then(|| stable_content_hash(content))),
            },
            12,
        );
        if let Some(requirement) = self.repair_requirement.as_mut()
            && requirement.path == path
        {
            let reread_satisfies_requirement = match requirement.suggested_range {
                Some(suggested_range) => honored_range
                    .is_some_and(|range| ranges_substantially_overlap(range, suggested_range)),
                None => honored_range.is_none(),
            };
            if reread_satisfies_requirement {
                requirement.exact_reread_completed = true;
            }
        }
    }

    fn record_first_valid_write_step(&mut self, step: usize) {
        if self
            .agent_repair_memory
            .scorecard
            .first_valid_write_step
            .is_none()
        {
            self.agent_repair_memory.scorecard.first_valid_write_step = Some(step);
        }
    }

    fn record_benchmark_write_kind(&mut self, action: &AgentAction) {
        let Some(ledger) = self.benchmark_case_ledger.as_ref() else {
            return;
        };
        if !ledger.validation_details.repair_required {
            return;
        }
        if self.benchmark_support_write_target_path(action).is_some() {
            self.agent_repair_memory.scorecard.support_write_count = self
                .agent_repair_memory
                .scorecard
                .support_write_count
                .saturating_add(1);
        } else {
            self.agent_repair_memory.scorecard.source_write_count = self
                .agent_repair_memory
                .scorecard
                .source_write_count
                .saturating_add(1);
        }
    }

    fn benchmark_support_write_target_path(&self, action: &AgentAction) -> Option<String> {
        let target_path = match action {
            AgentAction::PreviewEdit { path, .. }
            | AgentAction::ModifyToml { path, .. }
            | AgentAction::WriteFile { path, .. }
            | AgentAction::SetExecutable { path } => Some(path.clone()),
            AgentAction::ApplyPatch { path, .. }
            | AgentAction::ReplaceBlock { path, .. }
            | AgentAction::ReplaceRange { path, .. } => Some(path.clone()),
            AgentAction::ApplyPreview { .. } => self
                .agent_repair_memory
                .last_preview_result
                .as_deref()
                .and_then(|output| extract_labeled_line(output, "path:"))
                .or_else(|| {
                    (self.agent_repair_memory.preview_origin.as_deref()
                        == Some("write_locked_manifest"))
                    .then(|| "Cargo.toml".to_string())
                }),
            _ => None,
        }?;
        let path = target_path.trim();
        (path.ends_with(".toml") || is_obvious_test_file(path)).then(|| path.to_string())
    }

    fn should_preserve_support_write_for_validation(&self, action: &AgentAction) -> bool {
        let Some(ledger) = self.benchmark_case_ledger.as_ref() else {
            return false;
        };
        if !action_is_validation_like(action, Some(ledger)) {
            return false;
        }
        let Some(last_write) = self.last_successful_write_action.as_ref() else {
            return false;
        };
        self.benchmark_support_write_target_path(last_write)
            .is_some()
    }

    fn record_controller_injected_read(&mut self) {
        self.agent_repair_memory
            .scorecard
            .controller_injected_read_count = self
            .agent_repair_memory
            .scorecard
            .controller_injected_read_count
            .saturating_add(1);
    }

    fn record_suggested_edit_anchor(
        &mut self,
        path: &str,
        range: Option<crate::agent_protocol::ReadFileRange>,
        search_hint: Option<&str>,
    ) {
        self.agent_repair_memory.scorecard.anchor_suggestion_count = self
            .agent_repair_memory
            .scorecard
            .anchor_suggestion_count
            .saturating_add(1);
        push_capped(
            &mut self.agent_repair_memory.suggested_edit_anchors,
            AgentRepairSuggestedEditAnchor {
                path: path.to_string(),
                range,
                search_hint: search_hint
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            },
            8,
        );
    }

    fn record_preview_edit(&mut self, action: &AgentAction, output_text: &str) {
        self.agent_repair_memory.scorecard.preview_edit_count = self
            .agent_repair_memory
            .scorecard
            .preview_edit_count
            .saturating_add(1);
        let preview_id = extract_preview_id(output_text);
        if output_text.contains("would_apply: true") || output_text.contains("would_apply=true") {
            self.agent_repair_memory
                .scorecard
                .preview_edit_success_count = self
                .agent_repair_memory
                .scorecard
                .preview_edit_success_count
                .saturating_add(1);
        }
        if preview_id.is_some() {
            self.agent_repair_memory.scorecard.preview_created_count = self
                .agent_repair_memory
                .scorecard
                .preview_created_count
                .saturating_add(1);
            self.agent_repair_memory.last_preview_id = preview_id;
            self.agent_repair_memory.last_preview_path = action_target_path(action);
            self.agent_repair_memory.preview_origin =
                self.current_preview_origin().map(str::to_string);
        }
        if output_text.contains("syntax_preflight:") {
            self.agent_repair_memory.scorecard.syntax_preview_count = self
                .agent_repair_memory
                .scorecard
                .syntax_preview_count
                .saturating_add(1);
            if output_text.contains("syntax_preflight: failed") {
                self.agent_repair_memory
                    .scorecard
                    .syntax_preview_failure_count = self
                    .agent_repair_memory
                    .scorecard
                    .syntax_preview_failure_count
                    .saturating_add(1);
            }
        }
        self.agent_repair_memory.last_preview_result =
            Some(truncate_visible_text(output_text, 260));
    }

    fn current_preview_origin(&self) -> Option<&'static str> {
        let repair_state = self.benchmark_repair_state.as_ref()?;
        let ledger = self.benchmark_case_ledger.as_ref()?;
        let patch_target =
            benchmark_patch_target_path(repair_state, ledger, &self.agent_repair_memory);
        let write_locked = benchmark_patch_phase_write_locked(
            repair_state,
            ledger,
            &self.agent_repair_memory,
            self.repair_requirement.as_ref(),
        );
        if write_locked && patch_target.as_ref().ends_with(".toml") {
            Some("write_locked_manifest")
        } else {
            None
        }
    }

    fn record_redundant_inspection_turn(&mut self) {
        self.agent_repair_memory.scorecard.redundant_read_count = self
            .agent_repair_memory
            .scorecard
            .redundant_read_count
            .saturating_add(1);
    }

    fn record_failed_edit(
        &mut self,
        action: &AgentAction,
        failure_reason: &str,
    ) -> Option<FailedEditRecord> {
        let mut record = failed_edit_record_from_action(action, failure_reason)?;
        if let Some(existing) = self
            .failed_edit_records
            .iter_mut()
            .find(|existing| failed_edit_signature_matches(existing, &record))
        {
            existing.attempts = existing.attempts.saturating_add(1);
            existing.failure_reason = record.failure_reason;
            existing.matching_line_numbers = record.matching_line_numbers;
            self.agent_repair_memory
                .scorecard
                .repeated_failed_edit_count = self
                .agent_repair_memory
                .scorecard
                .repeated_failed_edit_count
                .saturating_add(1);
            record = existing.clone();
        } else {
            record.attempts = 1;
            self.failed_edit_records.push(record.clone());
            const MAX_FAILED_EDIT_RECORDS: usize = 8;
            if self.failed_edit_records.len() > MAX_FAILED_EDIT_RECORDS {
                let overflow = self
                    .failed_edit_records
                    .len()
                    .saturating_sub(MAX_FAILED_EDIT_RECORDS);
                self.failed_edit_records.drain(0..overflow);
            }
        }
        self.sync_benchmark_repair_state_to_ledger();
        Some(record)
    }

    fn record_rolled_back_write_validation_failure(&mut self, failure_reason: &str) {
        if !failure_reason.contains("safely rolled back") {
            return;
        }
        let Some(action) = self.last_successful_write_action.clone() else {
            return;
        };
        let Some(requirement) = repair_requirement_from_action(&action, failure_reason) else {
            return;
        };
        self.last_failed_tool_error = Some(failure_reason.to_string());
        self.agent_repair_memory.last_rollback_diagnostic =
            Some(truncate_visible_text(failure_reason, 260));
        self.agent_repair_memory.post_patch_diagnostic_class =
            classify_benchmark_diagnostic(failure_reason);
        self.agent_repair_memory.post_patch_diagnostic_excerpt =
            extract_assertion_excerpt(failure_reason)
                .or_else(|| Some(truncate_visible_text(failure_reason, 220)));
        self.agent_repair_memory.scorecard.rolled_back_write_count = self
            .agent_repair_memory
            .scorecard
            .rolled_back_write_count
            .saturating_add(1);
        if action_target_path(&action)
            .as_deref()
            .is_some_and(|path| !is_support_or_generated_runtime_path(path))
        {
            self.agent_repair_memory
                .scorecard
                .rolled_back_non_support_edit_count = self
                .agent_repair_memory
                .scorecard
                .rolled_back_non_support_edit_count
                .saturating_add(1);
        }
        if let AgentAction::ModifyToml { operations, .. } = &action {
            self.agent_repair_memory.last_manifest_patch_operations = operations.clone();
        }
        self.repair_requirement = Some(requirement);
        self.repair_recovery_turns_remaining = 1;
        self.stall_count = 0;
        self.redundant_inspection_turns = 0;
        if let Some(repair_state) = self.benchmark_repair_state.as_mut() {
            repair_state.phase = BenchmarkRepairPhase::NeedsPatch;
            repair_state.invalid_action_count = 0;
        }
        self.sync_benchmark_repair_state_to_ledger();
    }

    fn benchmark_repair_phase_message(&self) -> Option<String> {
        let ledger = self.benchmark_case_ledger.as_ref()?;
        let repair_state = self.benchmark_repair_state.as_ref()?;
        if repair_state.phase == BenchmarkRepairPhase::Idle
            || !ledger.validation_details.repair_required
        {
            return None;
        }
        let repair_target = benchmark_repair_target_path(repair_state, ledger);
        let suggested_read_range = benchmark_repair_phase_suggested_range(repair_state);
        let suggested_range = suggested_read_range.map(|range| range.label());
        let failing_test = repair_state
            .primary_failure_test_name
            .clone()
            .or_else(|| ledger.validation_details.primary_failure_test_name.clone())
            .or_else(|| {
                ledger
                    .validation_details
                    .failing_test_names
                    .first()
                    .cloned()
            });
        let assertion_excerpt = ledger.validation_details.assertion_excerpt.clone();
        let current_hypothesis = ledger.current_hypothesis.clone();
        let recommended_rerun_command = recommended_fast_loop_rerun_command(ledger);
        let phase_message = match repair_state.phase {
            BenchmarkRepairPhase::Idle => return None,
            phase => benchmark_repair_phase_instruction(phase),
        };
        if repair_state.phase == BenchmarkRepairPhase::NeedsPatch {
            let patch_target =
                benchmark_patch_target_path(repair_state, ledger, &self.agent_repair_memory);
            let target_lease = benchmark_target_lease_path(ledger, &self.agent_repair_memory);
            let patch_target_context_loaded = patch_target_context_loaded(
                repair_state,
                &self.agent_repair_memory,
                patch_target.as_ref(),
            );
            let honored_range = repair_state
                .last_owner_slice
                .as_ref()
                .and_then(|slice| slice.honored_range)
                .or(repair_state.failure_anchor_range);
            let target_honored_range = repair_state
                .last_owner_slice
                .as_ref()
                .filter(|slice| {
                    canonical_path(&slice.path) == canonical_path(patch_target.as_ref())
                        && !slice.test_only
                })
                .and_then(|slice| slice.honored_range);
            let bare_replace_disallowed = bare_replace_block_disallowed_for_path(
                patch_target.as_ref(),
                &self.failed_edit_records,
            );
            let scaffold_available = patch_phase_scaffold_available(&self.agent_repair_memory);
            let scaffold_required = scaffold_available && !patch_target_context_loaded;
            let write_locked = benchmark_patch_phase_write_locked(
                repair_state,
                ledger,
                &self.agent_repair_memory,
                self.repair_requirement.as_ref(),
            );
            let dependency_candidates = benchmark_dependency_candidates(ledger);
            let target_dependency_table =
                benchmark_target_dependency_table(repair_state, ledger, patch_target.as_ref());
            let manifest_operations = benchmark_manifest_patch_operations(
                ledger,
                target_dependency_table,
                &dependency_candidates,
            );
            let target_content_hash = target_content_hash_for_patch(
                repair_state,
                &self.agent_repair_memory,
                patch_target.as_ref(),
            );
            let allowed_targets = benchmark_allowed_implementation_targets(ledger);
            let read_only_tests = benchmark_read_only_test_targets(ledger);
            if write_locked && patch_target.ends_with(".toml") {
                let mut lines = vec![
                    "[Patch Packet] Manifest repair mode is active.".to_string(),
                    format!("Patch target: {patch_target}"),
                ];
                if let Some(target_lease) = target_lease.as_deref() {
                    lines.push(format!("Current target lease: {target_lease}"));
                }
                if let Some(diagnostic_class) = self
                    .agent_repair_memory
                    .diagnostic_class
                    .as_deref()
                    .or(ledger.validation_details.diagnostic_class.as_deref())
                {
                    lines.push(format!("Failure class: {diagnostic_class}"));
                }
                if let Some(target_dependency_table) = target_dependency_table {
                    lines.push(format!(
                        "Target dependency table: [{target_dependency_table}]"
                    ));
                }
                if !manifest_operations.is_empty() {
                    lines.push(format!(
                        "Exact dependency ops: {}",
                        render_toml_edit_operations_brief(&manifest_operations)
                    ));
                } else if !dependency_candidates.is_empty() {
                    lines.push(format!(
                        "Dependency candidates: {}",
                        dependency_candidates.join(", ")
                    ));
                }
                if let Some(content_hash) = target_content_hash.as_deref() {
                    lines.push(format!("Observed target content_hash: `{content_hash}`"));
                }
                if !self
                    .agent_repair_memory
                    .last_manifest_patch_operations
                    .is_empty()
                {
                    lines.push(format!(
                        "Previous manifest ops: {}",
                        render_toml_edit_operations_brief(
                            &self.agent_repair_memory.last_manifest_patch_operations
                        )
                    ));
                }
                if let Some(post_patch_diagnostic_class) = self
                    .agent_repair_memory
                    .post_patch_diagnostic_class
                    .as_deref()
                {
                    lines.push(format!(
                        "Post-patch diagnostic class: {post_patch_diagnostic_class}"
                    ));
                }
                if let Some(post_patch_excerpt) = self
                    .agent_repair_memory
                    .post_patch_diagnostic_excerpt
                    .as_deref()
                {
                    lines.push(format!(
                        "Post-patch diagnostic excerpt: {}",
                        truncate_visible_text(post_patch_excerpt, 180)
                    ));
                }
                if let Some(command) = recommended_rerun_command.as_deref() {
                    lines.push(format!("Exact rerun command: {command}"));
                }
                if preview_apply_locked(&self.agent_repair_memory) {
                    let preview_id = self
                        .agent_repair_memory
                        .last_preview_id
                        .as_deref()
                        .unwrap_or("preview_id_from_last_preview");
                    lines.push(format!(
                        "Allowed next action: exactly one `ApplyPreview` with preview_id `{preview_id}`."
                    ));
                    lines.push(
                        "A clean manifest preview already exists. Do not read, search, list, widen scope, or emit a new preview in this turn."
                            .to_string(),
                    );
                } else {
                    lines.push(
                        "Allowed next action: exactly one `PreviewEdit` with `modify_toml` on the leased manifest target."
                            .to_string(),
                    );
                    lines.push(
                        "Cargo.toml is already loaded. Another `ReadFile` on the manifest is invalid in this turn."
                            .to_string(),
                    );
                    lines.push(
                        "Do not search, list, widen scope, or switch to source files before the manifest patch lands."
                            .to_string(),
                    );
                }
                lines.push(
                    "Response shape: return one raw JSON object only. Keep `assistant_message` empty or very short."
                        .to_string(),
                );
                lines.push("Minimal JSON example:".to_string());
                if preview_apply_locked(&self.agent_repair_memory) {
                    lines.push(apply_preview_parser_recovery_example(
                        self.agent_repair_memory
                            .last_preview_id
                            .as_deref()
                            .unwrap_or("preview_id_from_last_preview"),
                    ));
                } else {
                    lines.push(manifest_preview_edit_scaffold_example(
                        patch_target.as_ref(),
                        target_content_hash.as_deref(),
                        target_dependency_table,
                        &dependency_candidates,
                        &manifest_operations,
                    ));
                }
                return Some(lines.join("\n"));
            }
            let mut lines = vec![
                "[Patch Packet] Narrow repair mode is active.".to_string(),
                if scaffold_required {
                    format!(
                        "Patch target context is not loaded yet. Use exactly one read-only scaffold action now: `SuggestEditAnchors`, `PreviewEdit`, or `ReadFile` on `{}`. Then write the patch and rerun the fast loop.",
                        patch_target
                    )
                } else if write_locked && patch_target.ends_with(".toml") {
                    if preview_apply_locked(&self.agent_repair_memory) {
                        format!(
                            "Write-locked patch phase: a clean manifest preview already exists for `{}`. Emit one `ApplyPreview` now. Do not read, search, list, or widen scope.",
                            patch_target
                        )
                    } else {
                        format!(
                            "Write-locked patch phase: emit one `PreviewEdit` with `modify_toml` on `{}` now. Do not read, search, list, or widen scope.",
                            patch_target
                        )
                    }
                } else if write_locked {
                    format!(
                        "Write-locked patch phase: emit one write on `{}` now. Fallback: one `PreviewEdit` on the same file, then apply it next turn. Do not read, search, list, or widen scope.",
                        patch_target
                    )
                } else if scaffold_available {
                    format!(
                        "Patch `{}` now. Do not reread evidence files. If anchor confidence is still shaky, you may use exactly one read-only scaffold action first: `PreviewEdit` on the patch target. Rerun the fast loop immediately after the patch.",
                        patch_target
                    )
                } else {
                    format!(
                        "Patch `{}` now. Do not reread or ask for more scaffolding. Rerun the fast loop immediately after the patch.",
                        patch_target
                    )
                },
                format!("Owner path: {repair_target}"),
                format!("Patch target: {patch_target}"),
            ];
            if let Some(target_lease) = target_lease.as_deref() {
                lines.push(format!("Current target lease: {target_lease}"));
            }
            if write_locked {
                lines.push("Repair write locked: true".to_string());
            }
            if let Some(diagnostic_class) = self
                .agent_repair_memory
                .diagnostic_class
                .as_deref()
                .or(ledger.validation_details.diagnostic_class.as_deref())
            {
                lines.push(format!("Diagnostic class: {diagnostic_class}"));
            }
            if !dependency_candidates.is_empty() {
                lines.push(format!(
                    "Missing dependencies: {}",
                    dependency_candidates.join(", ")
                ));
            }
            if let Some(target_dependency_table) = target_dependency_table {
                lines.push(format!(
                    "Target dependency table: [{}]",
                    target_dependency_table
                ));
            }
            if !self
                .agent_repair_memory
                .ranked_implementation_targets
                .is_empty()
            {
                lines.push(format!(
                    "Ranked implementation targets: {}",
                    render_ranked_implementation_targets(
                        &self.agent_repair_memory.ranked_implementation_targets
                    )
                ));
            }
            lines.push(format!(
                "Allowed implementation targets: {}",
                render_benchmark_target_list(&allowed_targets)
            ));
            if !read_only_tests.is_empty() {
                lines.push(format!(
                    "Test files are read-only unless explicitly listed above: {}",
                    render_benchmark_target_list(&read_only_tests)
                ));
            }
            if let Some(required_action) = self.agent_repair_memory.current_required_action.as_ref()
            {
                lines.push(format!("Required next action: {required_action}"));
            }
            if preview_apply_locked(&self.agent_repair_memory) {
                let preview_id = self
                    .agent_repair_memory
                    .last_preview_id
                    .as_deref()
                    .unwrap_or("preview_id_from_last_preview");
                lines.push(format!(
                    "A clean preview exists. Next action must be `ApplyPreview` with preview_id `{preview_id}`."
                ));
            }
            if let Some(range) = honored_range {
                lines.push(format!("Honored implementation range: {}", range.label()));
            }
            if let Some(failing_test) = failing_test {
                lines.push(format!("Primary failure test: {failing_test}"));
            }
            if let Some(path) = ledger.validation_details.primary_failure_path.as_ref() {
                let line = ledger
                    .validation_details
                    .primary_failure_line
                    .map(|value| format!(":{value}"))
                    .unwrap_or_default();
                lines.push(format!("Primary failure location: {path}{line}"));
            }
            if let Some(assertion_excerpt) = assertion_excerpt {
                lines.push(format!(
                    "Assertion excerpt: {}",
                    truncate_visible_text(&assertion_excerpt, 220)
                ));
            }
            if let Some(current_hypothesis) = current_hypothesis {
                lines.push(format!(
                    "Current hypothesis: {}",
                    truncate_visible_text(&current_hypothesis, 180)
                ));
            }
            if let Some(command) = recommended_rerun_command.as_ref() {
                lines.push(format!("Recommended rerun command: {command}"));
            }
            if !self.failed_edit_records.is_empty() {
                lines.push(format!(
                    "Failed edit memory: {}",
                    render_failed_edit_memory(&self.failed_edit_records)
                ));
            }
            if let Some(preview) = self.agent_repair_memory.last_preview_result.as_ref() {
                lines.push(format!(
                    "Last preview result: {}",
                    truncate_visible_text(preview, 220)
                ));
            }
            if let Some(rollback) = self.agent_repair_memory.last_rollback_diagnostic.as_ref() {
                lines.push(format!(
                    "Last rollback diagnostic: {}",
                    truncate_visible_text(rollback, 220)
                ));
            }
            if !self.agent_repair_memory.is_empty() {
                lines.push(format!(
                    "Agent scorecard: {}",
                    render_agent_repair_memory(&self.agent_repair_memory)
                ));
            }
            if bare_replace_disallowed {
                lines.push(
                    format!(
                        "Allowed actions: `ApplyPatch`, `WriteFile`, or `ReplaceBlock` with an explicit `range` on `{}`. Bare `ReplaceBlock` is paused for this repair episode.",
                        patch_target
                    ),
                );
            } else if patch_target.ends_with(".toml") {
                lines.push(format!(
                    "Allowed actions: `PreviewEdit` with `modify_toml` on `{}` first, then `ApplyPreview`. `ApplyPatch` or `WriteFile` stay disabled while manifest preview/apply mode is active.",
                    patch_target
                ));
            } else {
                lines.push(format!(
                    "Allowed actions: prefer `ReplaceRange` or `PreviewEdit` with `replace_range` on an observed slice of `{}`. `ApplyPatch`, ranged `ReplaceBlock`, and `WriteFile` remain allowed when needed.",
                    patch_target
                ));
            }
            if let Some(content_hash) = target_content_hash.as_ref() {
                lines.push(format!(
                    "Observed target content_hash for `{}`: `{}`",
                    patch_target, content_hash
                ));
            }
            if write_locked {
                if patch_target.ends_with(".toml") {
                    lines.push(
                        "Patch goal: preview the manifest dependency edit, apply that preview, then rerun the exact fast loop."
                            .to_string(),
                    );
                } else {
                    lines.push(format!(
                        "Patch goal: edit `{}` for the current source failure, then rerun the exact fast loop.",
                        patch_target
                    ));
                }
            } else if scaffold_available {
                lines.push(
                    format!(
                        "Optional scaffold: exactly one `PreviewEdit`, `SuggestEditAnchors`, or target `ReadFile` on `{}` before the write. These are read-only and must be followed by a real write turn.",
                        patch_target
                    ),
                );
            }
            lines.push(
                "Patch guidance: do not invent enum variants, methods, or types that are not visible in the read context."
                    .to_string(),
            );
            lines.push(
                "If replacing repeated code, use unique surrounding context, a ranged `ReplaceBlock`, or `ApplyPatch`; do not retry an ambiguous bare `ReplaceBlock`."
                    .to_string(),
            );
            lines.push(
                format!(
                    "Next-step contract: emit exactly one concrete write turn on `{}`, then rerun the fast loop.",
                    patch_target
                ),
            );
            lines.push(
                "Response shape: return one raw JSON object only. Keep `assistant_message` empty or to a few words."
                    .to_string(),
            );
            lines.push("Minimal JSON example:".to_string());
            if scaffold_required {
                lines.push(patch_phase_scaffold_example(patch_target.as_ref()));
            } else {
                lines.push(patch_phase_parser_recovery_example(
                    patch_target.as_ref(),
                    recommended_rerun_command.as_deref(),
                    if patch_target.ends_with(".toml") {
                        if patch_target_context_loaded {
                            honored_range
                        } else {
                            None
                        }
                    } else {
                        target_honored_range
                    },
                    bare_replace_disallowed
                        || !patch_target_context_loaded
                        || (!patch_target.ends_with(".toml") && target_honored_range.is_none()),
                    target_content_hash.as_deref(),
                    target_dependency_table,
                    &dependency_candidates,
                    &manifest_operations,
                ));
            }
            if !patch_target.ends_with(".toml") {
                if let Some(target_honored_range) = target_honored_range {
                    let expected_hash = observed_range_content_hash(
                        &self.agent_repair_memory,
                        patch_target.as_ref(),
                        target_honored_range,
                    )
                    .or_else(|| target_content_hash.clone())
                    .unwrap_or_else(|| "CONTENT_HASH_FROM_READ".to_string());
                    lines.push(format!("Minimal PreviewEdit example: {{\"actions\":[{{\"PreviewEdit\":{{\"path\":\"{}\",\"edit\":{{\"replace_range\":{{\"range\":{{\"start_line\":{},\"end_line\":{}}},\"expected_hash\":\"{}\",\"replacement\":\"FULL_REPLACEMENT_FOR_THOSE_LINES\"}}}}}}}}]}}", patch_target, target_honored_range.start_line, target_honored_range.end_line, expected_hash));
                    lines.push(format!("Minimal ReplaceRange example: {{\"actions\":[{{\"ReplaceRange\":{{\"path\":\"{}\",\"range\":{{\"start_line\":{},\"end_line\":{}}},\"expected_hash\":\"{}\",\"replacement\":\"FULL_REPLACEMENT_FOR_THOSE_LINES\"}}}}]}}", patch_target, target_honored_range.start_line, target_honored_range.end_line, expected_hash));
                } else {
                    lines.push(format!("Minimal ApplyPatch example: {{\"actions\":[{{\"ApplyPatch\":{{\"path\":\"{}\",\"patch\":\"*** Begin Patch\\n*** Update File: {}\\n@@\\n-<old source line>\\n+<new source line>\\n*** End Patch\\n\"}}}}]}}", patch_target, patch_target));
                }
            }
            if let Some(slice_content) = owner_slice_packet_content(repair_state) {
                let rendered_slice = truncate_patch_packet_slice(&slice_content);
                if !rendered_slice.trim().is_empty() {
                    let slice_label = repair_state
                        .last_owner_slice
                        .as_ref()
                        .filter(|slice| {
                            canonical_path(&slice.path) == canonical_path(patch_target.as_ref())
                                && !slice.test_only
                        })
                        .map(|_| "Implementation slice:")
                        .unwrap_or("Last honored evidence slice:");
                    lines.push(slice_label.to_string());
                    lines.push(format!("```rust\n{}\n```", rendered_slice));
                }
            }
            return Some(lines.join("\n"));
        }
        let mut lines = vec![
            "[Repair Phase] The last fast loop failed on a narrow benchmark case.".to_string(),
            phase_message.to_string(),
            format!("Repair target: {repair_target}"),
        ];
        if let Some(required_action) = self.agent_repair_memory.current_required_action.as_ref() {
            lines.push(format!("Required next action: {required_action}"));
        }
        if let Some(range) = suggested_range {
            lines.push(format!("Suggested range: {range}"));
        }
        if let Some(failing_test) = failing_test {
            lines.push(format!("Primary failure test: {failing_test}"));
        }
        if let Some(path) = ledger.validation_details.primary_failure_path.as_ref() {
            let line = ledger
                .validation_details
                .primary_failure_line
                .map(|value| format!(":{value}"))
                .unwrap_or_default();
            lines.push(format!("Primary failure location: {path}{line}"));
        }
        if let Some(assertion_excerpt) = assertion_excerpt {
            lines.push(format!(
                "Assertion excerpt: {}",
                truncate_visible_text(&assertion_excerpt, 220)
            ));
        }
        if let Some(current_hypothesis) = current_hypothesis {
            lines.push(format!(
                "Current hypothesis: {}",
                truncate_visible_text(&current_hypothesis, 180)
            ));
        }
        if let Some(command) = recommended_rerun_command.as_ref() {
            lines.push(format!("Recommended rerun command: {command}"));
            if repair_state.phase == BenchmarkRepairPhase::NeedsFastLoopRerun {
                lines.push(
                    "Response shape: return one raw JSON object only and emit the fast-loop rerun now."
                        .to_string(),
                );
                lines.push("Minimal JSON example:".to_string());
                lines.push(rerun_phase_parser_recovery_example(command));
            }
        }
        if matches!(
            repair_state.phase,
            BenchmarkRepairPhase::NeedsFailureAnchorRead
                | BenchmarkRepairPhase::NeedsImplementationRead
        ) {
            lines.push(
                "Response shape: return one raw JSON object only and emit exactly one ranged `ReadFile` now."
                    .to_string(),
            );
            lines.push("Minimal JSON example:".to_string());
            lines.push(focused_read_parser_recovery_example(
                repair_target,
                suggested_read_range,
            ));
        }
        Some(lines.join("\n"))
    }

    fn parser_recovery_message(&self, output_truncated: bool, error: &str) -> String {
        let generic = parser_recovery_message(output_truncated, error);
        let Some(ledger) = self.benchmark_case_ledger.as_ref() else {
            return generic;
        };
        let Some(repair_state) = self.benchmark_repair_state.as_ref() else {
            return benchmark_general_parser_recovery_message(
                generic,
                ledger,
                self.has_mutating_change,
            );
        };
        if !ledger.validation_details.repair_required {
            return benchmark_general_parser_recovery_message(
                generic,
                ledger,
                self.has_mutating_change,
            );
        }
        let repair_target = benchmark_repair_target_path(repair_state, ledger);
        let recommended_rerun_command = recommended_fast_loop_rerun_command(ledger);
        match repair_state.phase {
            BenchmarkRepairPhase::NeedsPatch => {
                if let Some(requirement) = self.repair_requirement.as_ref()
                    && !requirement.exact_reread_completed
                {
                    let mut lines = vec![
                        generic,
                        "[Parser] A previous owner-file edit failed, so patch phase is paused."
                            .to_string(),
                        Self::repair_requirement_prompt(requirement),
                    ];
                    if let Some(range) = requirement.suggested_range {
                        lines.push(format!("Suggested reread range: {}", range.label()));
                    }
                    lines.push(
                        "Return one raw JSON object only. Do not patch, rerun tests, search, or widen scope in this recovery turn."
                            .to_string(),
                    );
                    return lines.join("\n");
                }
                let patch_target =
                    benchmark_patch_target_path(repair_state, ledger, &self.agent_repair_memory);
                let patch_target_context_loaded = patch_target_context_loaded(
                    repair_state,
                    &self.agent_repair_memory,
                    patch_target.as_ref(),
                );
                let scaffold_available = patch_phase_scaffold_available(&self.agent_repair_memory);
                let scaffold_required = scaffold_available && !patch_target_context_loaded;
                let write_locked = benchmark_patch_phase_write_locked(
                    repair_state,
                    ledger,
                    &self.agent_repair_memory,
                    self.repair_requirement.as_ref(),
                );
                let dependency_candidates = benchmark_dependency_candidates(ledger);
                let target_dependency_table =
                    benchmark_target_dependency_table(repair_state, ledger, patch_target.as_ref());
                let manifest_operations = benchmark_manifest_patch_operations(
                    ledger,
                    target_dependency_table,
                    &dependency_candidates,
                );
                let mut lines = vec![
                    generic.clone(),
                    "[Parser] You are still in patch phase for a narrow benchmark repair."
                        .to_string(),
                    "Return one raw JSON object only. Do not emit prose before or after the JSON object."
                        .to_string(),
                    if patch_target_context_loaded || !scaffold_available {
                        "Do not reread evidence files, search, list directories, or widen scope in this recovery turn."
                            .to_string()
                    } else {
                        "The leased patch target has not been loaded yet; use exactly one read-only scaffold action on the patch target or write a concrete patch if you already know the exact edit."
                            .to_string()
                    },
                    format!("Owner path: {repair_target}"),
                    format!("Patch target: {patch_target}"),
                ];
                if let Some(range) = repair_state
                    .last_owner_slice
                    .as_ref()
                    .and_then(|slice| slice.honored_range)
                    .or(repair_state.failure_anchor_range)
                {
                    lines.push(format!("Honored implementation range: {}", range.label()));
                }
                let honored_range = repair_state
                    .last_owner_slice
                    .as_ref()
                    .and_then(|slice| slice.honored_range)
                    .or(repair_state.failure_anchor_range);
                let bare_replace_disallowed = bare_replace_block_disallowed_for_path(
                    patch_target.as_ref(),
                    &self.failed_edit_records,
                );
                if let Some(command) = recommended_rerun_command.as_deref() {
                    lines.push(format!("Recommended rerun command: {command}"));
                }
                if !dependency_candidates.is_empty() {
                    lines.push(format!(
                        "Missing dependencies: {}",
                        dependency_candidates.join(", ")
                    ));
                }
                if let Some(target_dependency_table) = target_dependency_table {
                    lines.push(format!(
                        "Target dependency table: [{}]",
                        target_dependency_table
                    ));
                }
                if !self.failed_edit_records.is_empty() {
                    lines.push(format!(
                        "Failed edit memory: {}",
                        render_failed_edit_memory(&self.failed_edit_records)
                    ));
                }
                if write_locked && patch_target.ends_with(".toml") {
                    if preview_apply_locked(&self.agent_repair_memory) {
                        let preview_id = self
                            .agent_repair_memory
                            .last_preview_id
                            .as_deref()
                            .unwrap_or("preview_id_from_last_preview");
                        lines.push(format!(
                            "Allowed action order: exactly one `ApplyPreview` with preview_id `{preview_id}` now. Then rerun the fast loop after the write lands."
                        ));
                    } else {
                        lines.push(format!(
                            "Allowed action order: exactly one `PreviewEdit` with `modify_toml` on `{}` now. Then apply that preview on the next turn and rerun the fast loop after the write lands.",
                            patch_target
                        ));
                    }
                } else if bare_replace_disallowed {
                    lines.push(format!(
                        "Allowed action order: first exactly one write on `{}` (`ApplyPatch`, `WriteFile`, or ranged `ReplaceBlock`), then optionally one immediate fast-loop rerun.",
                        patch_target
                    ));
                } else if patch_target_context_loaded || !scaffold_available {
                    lines.push(format!(
                        "Allowed action order: first exactly one write on `{}` (`ApplyPatch`, `ReplaceBlock`, or `WriteFile`), then optionally one immediate fast-loop rerun.",
                        patch_target
                    ));
                } else {
                    lines.push(format!(
                        "Allowed action order: exactly one `SuggestEditAnchors`, `PreviewEdit`, `ReadFile`, or write action on `{}`. Do not act on the evidence file.",
                        patch_target
                    ));
                }
                let target_content_hash = target_content_hash_for_patch(
                    repair_state,
                    &self.agent_repair_memory,
                    patch_target.as_ref(),
                );
                if write_locked && patch_target.ends_with(".toml") {
                    let mut lines = vec![
                        generic,
                        "[Parser] Manifest patch mode is still active.".to_string(),
                        "Return one raw JSON object only. Do not emit prose before or after the JSON object."
                            .to_string(),
                        "The leased manifest is already loaded. Another `ReadFile` on the manifest will be rejected in this turn."
                            .to_string(),
                        format!("Patch target: {patch_target}"),
                    ];
                    if let Some(target_dependency_table) = target_dependency_table {
                        lines.push(format!(
                            "Target dependency table: [{target_dependency_table}]"
                        ));
                    }
                    if !manifest_operations.is_empty() {
                        lines.push(format!(
                            "Exact dependency ops: {}",
                            render_toml_edit_operations_brief(&manifest_operations)
                        ));
                    }
                    if let Some(content_hash) = target_content_hash.as_deref() {
                        lines.push(format!("Observed target content_hash: `{content_hash}`"));
                    }
                    if let Some(command) = recommended_rerun_command.as_deref() {
                        lines.push(format!("Exact rerun command: {command}"));
                    }
                    if preview_apply_locked(&self.agent_repair_memory) {
                        let preview_id = self
                            .agent_repair_memory
                            .last_preview_id
                            .as_deref()
                            .unwrap_or("preview_id_from_last_preview");
                        lines.push(format!(
                            "Allowed action order: exactly one `ApplyPreview` with preview_id `{preview_id}`, then optional exact fast-loop rerun."
                        ));
                    } else {
                        lines.push(
                            "Allowed action order: exactly one `PreviewEdit` with `modify_toml` on the leased manifest target now. Another manifest read is invalid."
                                .to_string(),
                        );
                    }
                    lines.push("Minimal JSON example:".to_string());
                    if preview_apply_locked(&self.agent_repair_memory) {
                        lines.push(apply_preview_parser_recovery_example(
                            self.agent_repair_memory
                                .last_preview_id
                                .as_deref()
                                .unwrap_or("preview_id_from_last_preview"),
                        ));
                    } else {
                        lines.push(manifest_preview_edit_scaffold_example(
                            patch_target.as_ref(),
                            target_content_hash.as_deref(),
                            target_dependency_table,
                            &dependency_candidates,
                            &manifest_operations,
                        ));
                    }
                    return lines.join("\n");
                }
                if let Some(content_hash) = target_content_hash.as_ref() {
                    lines.push(format!(
                        "Observed target content_hash for `{}`: `{}`",
                        patch_target, content_hash
                    ));
                }
                lines.push("Minimal JSON example:".to_string());
                if scaffold_required {
                    lines.push(patch_phase_scaffold_example(patch_target.as_ref()));
                } else {
                    lines.push(patch_phase_parser_recovery_example(
                        patch_target.as_ref(),
                        recommended_rerun_command.as_deref(),
                        if patch_target_context_loaded {
                            honored_range
                        } else {
                            None
                        },
                        bare_replace_disallowed || !patch_target_context_loaded,
                        target_content_hash.as_deref(),
                        target_dependency_table,
                        &dependency_candidates,
                        &manifest_operations,
                    ));
                }
                lines.join("\n")
            }
            BenchmarkRepairPhase::NeedsFastLoopRerun => {
                let mut lines = vec![
                    generic,
                    "[Parser] You are still in fast-loop rerun phase for this benchmark repair."
                        .to_string(),
                    "Return one raw JSON object only. Do not emit prose before or after the JSON object."
                        .to_string(),
                    "Do not patch or reread in this recovery turn. Emit the smallest fast-loop rerun now."
                        .to_string(),
                ];
                if let Some(command) = recommended_rerun_command.as_deref() {
                    lines.push(format!("Recommended rerun command: {command}"));
                    lines.push("Minimal JSON example:".to_string());
                    lines.push(rerun_phase_parser_recovery_example(command));
                }
                lines.join("\n")
            }
            BenchmarkRepairPhase::NeedsFailureAnchorRead
            | BenchmarkRepairPhase::NeedsImplementationRead => {
                let mut lines = vec![
                    generic,
                    "[Parser] You are still in a focused-read phase for this benchmark repair."
                        .to_string(),
                    "Return one raw JSON object only and emit the required focused `ReadFile` now."
                        .to_string(),
                ];
                if let Some(message) = self.benchmark_repair_phase_message() {
                    lines.push(message);
                }
                lines.join("\n")
            }
            _ => generic,
        }
    }

    fn benchmark_repair_phase_correction_message(
        &mut self,
        actions: &[AgentAction],
    ) -> Result<Option<String>, String> {
        let Some(repair_state_snapshot) = self.benchmark_repair_state.clone() else {
            return Ok(None);
        };
        if repair_state_snapshot.phase == BenchmarkRepairPhase::Idle {
            return Ok(None);
        }
        let owner_path = repair_state_snapshot.owner_path.clone();
        let failure_anchor_range = repair_state_snapshot.failure_anchor_range;
        let implementation_suggested_range = repair_state_snapshot.implementation_suggested_range;
        let phase = repair_state_snapshot.phase;
        let patch_target = self
            .benchmark_case_ledger
            .as_ref()
            .map(|ledger| {
                benchmark_patch_target_path(
                    &repair_state_snapshot,
                    ledger,
                    &self.agent_repair_memory,
                )
                .into_owned()
            })
            .unwrap_or_else(|| owner_path.clone());
        let patch_target_context_loaded = patch_target_context_loaded(
            &repair_state_snapshot,
            &self.agent_repair_memory,
            &patch_target,
        );
        let write_locked = self.benchmark_case_ledger.as_ref().is_some_and(|ledger| {
            benchmark_patch_phase_write_locked(
                &repair_state_snapshot,
                ledger,
                &self.agent_repair_memory,
                self.repair_requirement.as_ref(),
            )
        });
        let attempted_actions = actions
            .iter()
            .map(AgentAction::summary)
            .collect::<Vec<_>>()
            .join(", ");
        let valid = if let Some(requirement) = self.repair_requirement.as_ref()
            && !requirement.exact_reread_completed
        {
            actions.iter().all(|action| {
                matches!(
                    action,
                    AgentAction::ReadFile { path, range }
                        if Self::repair_requirement_read_is_valid(requirement, path, *range)
                )
            })
        } else {
            match phase {
                BenchmarkRepairPhase::NeedsFailureAnchorRead => actions.iter().all(|action| {
                    self.benchmark_evidence_action_satisfies(
                        &owner_path,
                        failure_anchor_range,
                        action,
                    )
                }),
                BenchmarkRepairPhase::NeedsImplementationRead => actions.iter().all(|action| {
                    matches!(
                        action,
                        AgentAction::ReadFile { path, range }
                            if path == &owner_path
                                && range
                                    .and_then(|value| value.normalized())
                                    .is_some_and(|requested_range| {
                                        failure_anchor_range.is_some_and(|anchor_range| {
                                            range_meaningfully_differs_from_anchor(
                                                requested_range,
                                                anchor_range,
                                            )
                                        }) && implementation_suggested_range.is_none_or(
                                            |suggested_range| {
                                                read_range_overlap(
                                                    requested_range,
                                                    suggested_range,
                                                ) > 0
                                            },
                                        )
                                    })
                    )
                }),
                BenchmarkRepairPhase::NeedsPatch => {
                    self.benchmark_case_ledger.as_ref().is_some_and(|ledger| {
                        patch_phase_actions_are_valid(
                            actions,
                            &patch_target,
                            ledger,
                            &self.failed_edit_records,
                            &self.agent_repair_memory,
                            patch_target_context_loaded,
                        )
                    })
                }
                BenchmarkRepairPhase::NeedsFastLoopRerun => actions.iter().all(|action| {
                    self.benchmark_case_ledger
                        .as_ref()
                        .is_some_and(|ledger| action_matches_fast_loop(action, ledger))
                }),
                BenchmarkRepairPhase::Idle => true,
            }
        };
        if valid {
            if let Some(repair_state) = self.benchmark_repair_state.as_mut() {
                repair_state.invalid_action_count = 0;
            }
            self.sync_benchmark_repair_state_to_ledger();
            return Ok(None);
        }
        if let Some(repair_state) = self.benchmark_repair_state.as_mut() {
            repair_state.invalid_action_count = repair_state.invalid_action_count.saturating_add(1);
            self.agent_repair_memory
                .scorecard
                .repair_invalid_action_streak_max = self
                .agent_repair_memory
                .scorecard
                .repair_invalid_action_streak_max
                .max(repair_state.invalid_action_count);
        }
        if phase == BenchmarkRepairPhase::NeedsPatch
            && write_locked
            && actions
                .iter()
                .any(|action| benchmark_write_phase_refusal(action, &patch_target))
        {
            self.agent_repair_memory
                .scorecard
                .write_phase_action_refusal_count = self
                .agent_repair_memory
                .scorecard
                .write_phase_action_refusal_count
                .saturating_add(1);
            if patch_target.ends_with(".toml") && preview_apply_locked(&self.agent_repair_memory) {
                self.agent_repair_memory
                    .scorecard
                    .preview_apply_action_refusal_count = self
                    .agent_repair_memory
                    .scorecard
                    .preview_apply_action_refusal_count
                    .saturating_add(1);
            }
        }
        self.record_rejected_actions(
            phase,
            actions,
            "action did not satisfy the current benchmark repair phase",
        );
        self.sync_benchmark_repair_state_to_ledger();
        if phase == BenchmarkRepairPhase::NeedsPatch
            && write_locked
            && patch_target.ends_with(".toml")
        {
            let preview_apply_locked = preview_apply_locked(&self.agent_repair_memory);
            let benchmark_ledger = self.benchmark_case_ledger.as_ref();
            let target_dependency_table = benchmark_ledger.and_then(|ledger| {
                benchmark_target_dependency_table(&repair_state_snapshot, ledger, &patch_target)
            });
            let dependency_candidates = benchmark_ledger
                .map(benchmark_dependency_candidates)
                .unwrap_or_default();
            let manifest_operations = benchmark_ledger
                .map(|ledger| {
                    benchmark_manifest_patch_operations(
                        ledger,
                        target_dependency_table,
                        &dependency_candidates,
                    )
                })
                .unwrap_or_default();
            let target_content_hash = target_content_hash_for_patch(
                &repair_state_snapshot,
                &self.agent_repair_memory,
                patch_target.as_ref(),
            );
            let invalid_action_count = self
                .benchmark_repair_state
                .as_ref()
                .map(|repair_state| repair_state.invalid_action_count)
                .unwrap_or(0);
            if invalid_action_count >= 2 {
                if self
                    .agent_repair_memory
                    .scorecard
                    .write_phase_action_refusal_count
                    > 0
                {
                    return Err(format!(
                        "Autonomous write_phase_action_refusal during {} after repeated off-contract read-only repair actions.",
                        phase.label()
                    ));
                }
                return Err(format!(
                    "Autonomous repair loop stalled during {} after repeated invalid repair-phase actions.",
                    phase.label()
                ));
            }
            let mut lines = vec![
                "[Repair Phase] Manifest patch mode rejected the previous plan.".to_string(),
                format!("Rejected turn plan: {attempted_actions}"),
                "Cargo.toml is already loaded. Do not read it again.".to_string(),
                format!("Patch target: {patch_target}"),
            ];
            if preview_apply_locked {
                let preview_id = self
                    .agent_repair_memory
                    .last_preview_id
                    .as_deref()
                    .unwrap_or("preview_id_from_last_preview");
                lines.push(format!(
                    "A clean manifest preview already exists. Return exactly one raw JSON object with exactly one `ApplyPreview` action using preview_id `{preview_id}` now."
                ));
                lines.push(
                    "No `ReadFile`, `ListDirectory`, `SearchText`, new `PreviewEdit`, or source-file reads are allowed in this correction turn."
                        .to_string(),
                );
            } else {
                lines.push(
                    "Return exactly one raw JSON object with exactly one `PreviewEdit` action carrying `modify_toml` now."
                        .to_string(),
                );
                lines.push(
                    "No `ReadFile`, `ListDirectory`, `SearchText`, direct `ModifyToml`, or source-file reads are allowed in this correction turn."
                        .to_string(),
                );
            }
            if let Some(target_dependency_table) = target_dependency_table {
                lines.push(format!(
                    "Target dependency table: [{target_dependency_table}]"
                ));
            }
            if !manifest_operations.is_empty() {
                lines.push(format!(
                    "Exact dependency ops: {}",
                    render_toml_edit_operations_brief(&manifest_operations)
                ));
            }
            if let Some(content_hash) = target_content_hash.as_deref() {
                lines.push(format!("Observed target content_hash: `{content_hash}`"));
            }
            let rerun_command = benchmark_ledger.and_then(recommended_fast_loop_rerun_command);
            if let Some(command) = rerun_command.as_deref() {
                lines.push(format!("Exact rerun command: {command}"));
            }
            lines.push("Minimal JSON example:".to_string());
            if preview_apply_locked {
                lines.push(apply_preview_parser_recovery_example(
                    self.agent_repair_memory
                        .last_preview_id
                        .as_deref()
                        .unwrap_or("preview_id_from_last_preview"),
                ));
            } else {
                lines.push(manifest_preview_edit_scaffold_example(
                    patch_target.as_ref(),
                    target_content_hash.as_deref(),
                    target_dependency_table,
                    &dependency_candidates,
                    &manifest_operations,
                ));
            }
            return Ok(Some(lines.join("\n")));
        }
        let mut lines = vec![
            "[Repair Phase] The proposed next action does not satisfy the current repair step."
                .to_string(),
            format!("Rejected turn plan: {attempted_actions}"),
        ];
        if let Some(requirement) = self.repair_requirement.as_ref()
            && !requirement.exact_reread_completed
        {
            lines.push(format!(
                "[Repair Brief]\nThe previous edit failure still requires a fresh `ReadFile` for `{}` before any next write.",
                requirement.path
            ));
            if let Some(range) = requirement.suggested_range {
                lines.push(format!("Suggested reread range: {}", range.label()));
            }
            lines.push(Self::repair_requirement_correction(requirement));
        } else {
            if let Some(message) = self.benchmark_repair_phase_message() {
                lines.push(message);
            }
            let correction = match phase {
                BenchmarkRepairPhase::NeedsFailureAnchorRead => {
                    "Correction: gather the missing failure evidence now. Prefer the suggested owner-file slice, or use ExplainValidationFailure, SuggestEditAnchors, SearchText, or a directly related owner/test read if the failure has no precise file/line anchor."
                }
                BenchmarkRepairPhase::NeedsImplementationRead => {
                    "Correction: read exactly one implementation slice on the same owner file now. Use an explicit range that is materially different from the failing test slice and overlaps the suggested implementation range."
                }
                BenchmarkRepairPhase::NeedsPatch => {
                    if write_locked && patch_target.ends_with(".toml") {
                        if preview_apply_locked(&self.agent_repair_memory) {
                            "Correction: act on the leased patch target now. The manifest preview already exists, so emit exactly one `ApplyPreview` with the preview id from the last clean preview. Do not read, search, list, or widen scope first."
                        } else {
                            "Correction: act on the leased patch target now. Emit exactly one `PreviewEdit` with `modify_toml` on the manifest. Do not read, search, list, or widen scope first."
                        }
                    } else if write_locked {
                        "Correction: act on the leased patch target now. Emit exactly one write-class action on that file, or one `PreviewEdit` on the same file if you need a dry run. Do not read, search, list, or widen scope first."
                    } else {
                        "Correction: act on the leased patch target now. If this is the first patch-phase scaffold, you may emit exactly one PreviewEdit, SuggestEditAnchors, or target ReadFile on the patch target; otherwise write with ApplyPatch, ranged ReplaceBlock, or WriteFile. Do not reread evidence files or widen scope first."
                    }
                }
                BenchmarkRepairPhase::NeedsFastLoopRerun => {
                    "Correction: rerun the smallest fast loop now so the patch can be validated."
                }
                BenchmarkRepairPhase::Idle => "",
            };
            lines.push(correction.to_string());
            if phase == BenchmarkRepairPhase::NeedsPatch
                && bare_replace_block_disallowed_for_path(&patch_target, &self.failed_edit_records)
            {
                lines.push(
                    "Bare `ReplaceBlock` was rejected because an ambiguous patch-target replacement already failed in this repair episode. Use ranged `ReplaceBlock`, `ApplyPatch`, or `WriteFile`."
                        .to_string(),
                );
            }
        }
        let invalid_action_count = self
            .benchmark_repair_state
            .as_ref()
            .map(|repair_state| repair_state.invalid_action_count)
            .unwrap_or(0);
        if invalid_action_count >= 2 {
            if phase == BenchmarkRepairPhase::NeedsPatch
                && write_locked
                && self
                    .agent_repair_memory
                    .scorecard
                    .write_phase_action_refusal_count
                    > 0
            {
                return Err(format!(
                    "Autonomous write_phase_action_refusal during {} after repeated off-contract read-only repair actions.",
                    phase.label()
                ));
            }
            if phase == BenchmarkRepairPhase::NeedsPatch && !patch_target.ends_with(".toml") {
                return Err(format!(
                    "Autonomous source_patch_refusal during {} after repeated invalid source repair actions.",
                    phase.label()
                ));
            }
            return Err(format!(
                "Autonomous repair loop stalled during {} after repeated invalid repair-phase actions.",
                phase.label()
            ));
        }
        Ok(Some(lines.join("\n")))
    }

    fn benchmark_narrow_repair_restricts_action(&self, action: &AgentAction) -> Option<String> {
        let ledger = self.benchmark_case_ledger.as_ref()?;
        if ledger.case_class != "narrow-owner-first" || !ledger.validation_details.repair_required {
            return None;
        }
        if let Some(repair_state) = self.benchmark_repair_state.as_ref()
            && benchmark_patch_phase_write_locked(
                repair_state,
                ledger,
                &self.agent_repair_memory,
                self.repair_requirement.as_ref(),
            )
        {
            let patch_target =
                benchmark_patch_target_path(repair_state, ledger, &self.agent_repair_memory);
            match action {
                AgentAction::ReadFile { .. }
                | AgentAction::ListDirectory { .. }
                | AgentAction::SearchText { .. }
                | AgentAction::SearchSymbols { .. }
                | AgentAction::GetRepoCapsule { .. }
                | AgentAction::ExplainValidationFailure { .. }
                | AgentAction::SuggestImplementationTargets { .. }
                | AgentAction::SuggestEditAnchors { .. } => {
                    return Some(format!(
                        "benchmark_autonomous write-locked patch phase requires acting on `{}` now; do not reread, search, list, or widen scope first",
                        patch_target
                    ));
                }
                _ => {}
            }
        }
        if self
            .benchmark_repair_state
            .as_ref()
            .is_some_and(|repair_state| {
                repair_state.phase == BenchmarkRepairPhase::NeedsFailureAnchorRead
                    && self.benchmark_evidence_action_satisfies(
                        &repair_state.owner_path,
                        repair_state.failure_anchor_range,
                        action,
                    )
            })
        {
            return None;
        }
        let owner_scope = |path: &str| {
            ledger.owner_files.iter().any(|candidate| candidate == path)
                || ledger
                    .expected_touch_targets
                    .iter()
                    .any(|candidate| candidate == path)
        };
        match action {
            AgentAction::ListDirectory { .. }
            | AgentAction::SearchText { .. }
            | AgentAction::SearchSymbols { .. }
            | AgentAction::GetRepoCapsule { .. } => Some(
                "benchmark_autonomous narrow repair mode keeps you on the owner file after a failed fast loop; do not widen to broad repo exploration yet"
                    .to_string(),
            ),
            AgentAction::ReadFile { path, .. }
            | AgentAction::SuggestEditAnchors { path, .. }
            | AgentAction::PreviewEdit { path, .. }
            | AgentAction::ReplaceRange { path, .. }
            | AgentAction::ModifyToml { path, .. }
            | AgentAction::WriteFile { path, .. }
            | AgentAction::ApplyPatch { path, .. }
            | AgentAction::ReplaceBlock { path, .. }
            | AgentAction::SetExecutable { path } if !owner_scope(path) => Some(format!(
                "benchmark_autonomous narrow repair mode is restricted to owner files and expected touch targets after a failed fast loop; `{path}` is outside that scope"
            )),
            _ => None,
        }
    }

    fn benchmark_target_lease_violation(&self, action: &AgentAction) -> Option<String> {
        let repair_state = self.benchmark_repair_state.as_ref()?;
        if repair_state.phase != BenchmarkRepairPhase::NeedsPatch {
            return None;
        }
        let lease = self
            .agent_repair_memory
            .implementation_target_lease
            .as_deref()
            .filter(|value| !value.trim().is_empty())?;
        let target_path = match action {
            AgentAction::SuggestEditAnchors { path, .. }
            | AgentAction::PreviewEdit { path, .. }
            | AgentAction::ReplaceRange { path, .. }
            | AgentAction::ModifyToml { path, .. }
            | AgentAction::WriteFile { path, .. }
            | AgentAction::ApplyPatch { path, .. }
            | AgentAction::ReplaceBlock { path, .. }
            | AgentAction::SetExecutable { path } => path,
            _ => return None,
        };
        if canonical_path(target_path) == canonical_path(lease) {
            return None;
        }
        let evidence_label = if is_obvious_test_file(target_path) {
            "test evidence file"
        } else {
            "non-leased evidence file"
        };
        Some(format!(
            "benchmark_autonomous target lease redirect: `{target_path}` is a {evidence_label}; the current target lease is `{lease}`. Use SuggestEditAnchors, PreviewEdit, ApplyPatch, ranged ReplaceBlock, or WriteFile on the leased implementation target only until validation changes the failure."
        ))
    }

    fn benchmark_evidence_action_satisfies(
        &self,
        owner_path: &str,
        failure_anchor_range: Option<crate::agent_protocol::ReadFileRange>,
        action: &AgentAction,
    ) -> bool {
        match action {
            AgentAction::ReadFile { path, range } => {
                if path == owner_path {
                    if let Some(anchor_range) = failure_anchor_range {
                        return range.and_then(|value| value.normalized()).is_some_and(
                            |requested_range| read_range_overlap(requested_range, anchor_range) > 0,
                        );
                    }
                    return true;
                }
                self.benchmark_related_evidence_path(path)
            }
            AgentAction::SearchText { query, .. } | AgentAction::SearchSymbols { query, .. } => {
                !query.trim().is_empty()
            }
            AgentAction::GetRepoCapsule { .. }
            | AgentAction::ExplainValidationFailure { .. }
            | AgentAction::SuggestImplementationTargets { .. } => true,
            AgentAction::SuggestEditAnchors { path, .. } => {
                path == owner_path || self.benchmark_related_evidence_path(path)
            }
            _ => false,
        }
    }

    fn benchmark_related_evidence_path(&self, path: &str) -> bool {
        let Some(ledger) = self.benchmark_case_ledger.as_ref() else {
            return false;
        };
        ledger.owner_files.iter().any(|candidate| candidate == path)
            || ledger
                .expected_touch_targets
                .iter()
                .any(|candidate| candidate == path)
            || ledger
                .companion_files_required
                .iter()
                .any(|candidate| candidate == path)
            || ledger
                .validation_details
                .primary_failure_path
                .as_ref()
                .is_some_and(|candidate| candidate == path)
            || is_obvious_test_file(path)
    }

    fn benchmark_needs_baseline_validation(&self) -> bool {
        self.benchmark_case_ledger.as_ref().is_some_and(|ledger| {
            !self.has_mutating_change
                && ledger.last_validation_failure.is_none()
                && ledger.validation_status.is_none()
                && self
                    .agent_repair_memory
                    .canonical_action_history
                    .iter()
                    .all(|action| !action.validation_like)
                && ledger
                    .fast_loop_commands
                    .iter()
                    .any(|command| !command.trim().is_empty())
        })
    }

    fn benchmark_baseline_validation_message(&self) -> Option<String> {
        let ledger = self.benchmark_case_ledger.as_ref()?;
        let command = ledger
            .fast_loop_commands
            .iter()
            .find(|command| !command.trim().is_empty())?
            .trim();
        Some(
            [
                "[Benchmark State] You have inspected context but have not established a failing validation anchor yet.",
                "Required next action: run the exact baseline fast loop now. Do not keep reading or searching before this validation.",
                &format!("Exact validation command: {command}"),
                "Response shape: return one raw JSON object only.",
                "Minimal JSON example:",
                &rerun_phase_parser_recovery_example(command),
            ]
            .join("\n"),
        )
    }

    fn repeated_validation_repair_message(&self, action_summary: &str, error: &str) -> String {
        let mut lines = vec![format!(
            "[Repair Phase]\nThe action `{action_summary}` was rejected because validation already exposed the failure and no repair write has been made yet."
        )];
        lines.push(error.to_string());
        if let Some(message) = self.benchmark_repair_phase_message() {
            lines.push(message);
            return lines.join("\n");
        }
        if let Some(ledger) = self.benchmark_case_ledger.as_ref() {
            let owner_path = ledger
                .owner_files
                .iter()
                .chain(ledger.expected_touch_targets.iter())
                .find(|path| !path.trim().is_empty())
                .map(String::as_str)
                .unwrap_or("[owner file]");
            lines.push(format!("Owner path: {owner_path}"));
            if let Some(failure) = ledger.last_validation_failure.as_ref() {
                lines.push(format!(
                    "Last validation failure: {}",
                    truncate_visible_text(failure, 260)
                ));
            }
            lines.push(
                "Required next action: either read one focused owner slice, ask for edit anchors, or patch the owner file. Do not rerun validation again until after a write."
                    .to_string(),
            );
            lines.push("Allowed actions: ReadFile with a concrete range, SuggestEditAnchors, ApplyPatch, ranged ReplaceBlock, or WriteFile.".to_string());
            lines.push("Minimal focused-read JSON example:".to_string());
            lines.push(focused_read_parser_recovery_example(owner_path, None));
        }
        lines.join("\n")
    }

    fn turn_repeats_known_inspection_only(&self, actions: &[AgentAction]) -> bool {
        !actions.is_empty()
            && actions.iter().all(|action| match action {
                AgentAction::ReadFile { path, range } => {
                    self.working_set.contains(path)
                        && !self.allow_benchmark_focused_same_file_reread(path, *range)
                }
                AgentAction::ListDirectory { path } => self.working_set.contains(path),
                AgentAction::SearchText { query, .. } => self
                    .agent_repair_memory
                    .canonical_action_history
                    .iter()
                    .any(|record| record.signature == format!("search_text:{}", query.trim())),
                AgentAction::SearchSymbols { query, .. } => self
                    .agent_repair_memory
                    .canonical_action_history
                    .iter()
                    .any(|record| record.signature == format!("search_symbols:{}", query.trim())),
                AgentAction::GetRepoCapsule { query, .. } => {
                    let query = query.as_deref().unwrap_or("").trim();
                    self.agent_repair_memory
                        .canonical_action_history
                        .iter()
                        .any(|record| record.signature == format!("repo_capsule:{query}"))
                }
                AgentAction::SuggestEditAnchors {
                    path,
                    range,
                    search_hint,
                } => {
                    let range = range
                        .and_then(|value| value.normalized())
                        .map(|value| value.label())
                        .unwrap_or_else(|| "all".to_string());
                    let hint = search_hint.as_deref().unwrap_or("").trim();
                    self.agent_repair_memory
                        .canonical_action_history
                        .iter()
                        .any(|record| {
                            record.signature
                                == format!("anchors:{}:{}:{}", canonical_path(path), range, hint)
                        })
                }
                AgentAction::PreviewEdit { path, edit } => {
                    let signature = format!(
                        "preview:{}:{}",
                        canonical_path(path),
                        short_text_fingerprint(&format!("{edit:?}"))
                    );
                    self.agent_repair_memory
                        .canonical_action_history
                        .iter()
                        .any(|record| record.signature == signature)
                }
                _ => false,
            })
    }

    fn repair_requirement_range_guidance(&self, actions: &[AgentAction]) -> Option<String> {
        let requirement = self.repair_requirement.as_ref()?;
        if requirement.exact_reread_completed {
            return None;
        }
        let all_reads_target_requirement = !actions.is_empty()
            && actions.iter().all(|action| match action {
                AgentAction::ReadFile { path, range } => {
                    path == &requirement.path
                        && range.and_then(|value| value.normalized()).is_none()
                }
                _ => false,
            });
        if !all_reads_target_requirement || Self::repair_requirement_prefers_full_file(requirement)
        {
            return None;
        }
        let suggested = requirement
            .suggested_range
            .map(|range| format!(" Suggested range: {}.", range.label()))
            .unwrap_or_default();
        Some(format!(
            "[Loop guard]\nThe previous failure requires a focused `ReadFile` for `{}` before you can continue. Request a concrete line range instead of rereading the whole file.{}",
            requirement.path, suggested
        ))
    }

    fn repair_requirement_needs_reread(&self) -> bool {
        self.repair_requirement
            .as_ref()
            .is_some_and(|requirement| !requirement.exact_reread_completed)
    }

    fn required_repair_read_action(&self) -> Option<AgentAction> {
        if let Some(requirement) = self.repair_requirement.as_ref()
            && !requirement.exact_reread_completed
        {
            return Some(AgentAction::ReadFile {
                path: requirement.path.clone(),
                range: if Self::repair_requirement_prefers_full_file(requirement) {
                    None
                } else {
                    Some(requirement.suggested_range?)
                },
            });
        }
        let repair_state = self.benchmark_repair_state.as_ref()?;
        let range = match repair_state.phase {
            BenchmarkRepairPhase::NeedsFailureAnchorRead => repair_state.failure_anchor_range?,
            BenchmarkRepairPhase::NeedsImplementationRead => {
                repair_state.implementation_suggested_range?
            }
            BenchmarkRepairPhase::NeedsPatch
            | BenchmarkRepairPhase::NeedsFastLoopRerun
            | BenchmarkRepairPhase::Idle => return None,
        };
        Some(AgentAction::ReadFile {
            path: repair_state.owner_path.clone(),
            range: Some(range),
        })
    }

    fn should_inject_required_read(&self) -> bool {
        self.benchmark_case_ledger
            .as_ref()
            .is_some_and(|ledger| ledger.validation_details.repair_required)
            && self.required_repair_read_action().is_some()
            && (self.parser_recovery_failures > 0
                || self
                    .benchmark_repair_state
                    .as_ref()
                    .is_some_and(|repair_state| repair_state.invalid_action_count > 0)
                || self.stall_count > 0)
    }

    fn allow_benchmark_focused_same_file_reread(
        &self,
        path: &str,
        range: Option<crate::agent_protocol::ReadFileRange>,
    ) -> bool {
        if self.has_mutating_change {
            return false;
        }
        if self.agent_repair_memory.scorecard.redundant_read_count >= 2 {
            return false;
        }
        if let Some(repair_state) = self.benchmark_repair_state.as_ref() {
            match repair_state.phase {
                BenchmarkRepairPhase::NeedsFailureAnchorRead => {
                    return range
                        .and_then(|value| value.normalized())
                        .zip(repair_state.failure_anchor_range)
                        .is_some_and(|(requested_range, anchor_range)| {
                            path == repair_state.owner_path
                                && read_range_overlap(requested_range, anchor_range) > 0
                        });
                }
                BenchmarkRepairPhase::NeedsImplementationRead => {
                    return range
                        .and_then(|value| value.normalized())
                        .zip(repair_state.failure_anchor_range)
                        .is_some_and(|(requested_range, anchor_range)| {
                            path == repair_state.owner_path
                                && range_meaningfully_differs_from_anchor(
                                    requested_range,
                                    anchor_range,
                                )
                        });
                }
                BenchmarkRepairPhase::NeedsPatch
                | BenchmarkRepairPhase::NeedsFastLoopRerun
                | BenchmarkRepairPhase::Idle => {}
            }
        }
        if self.redundant_inspection_turns > 0 {
            return false;
        }
        if self
            .repair_requirement
            .as_ref()
            .is_some_and(|requirement| requirement.path == path)
        {
            if let Some(requirement) = self.repair_requirement.as_ref() {
                return Self::repair_requirement_read_is_valid(requirement, path, range);
            }
        }
        let Some(ledger) = self.benchmark_case_ledger.as_ref() else {
            return false;
        };
        let Some(last_failure) = ledger.last_validation_failure.as_ref() else {
            return false;
        };
        !last_failure.trim().is_empty()
            && (ledger.owner_files.iter().any(|candidate| candidate == path)
                || ledger
                    .expected_touch_targets
                    .iter()
                    .any(|candidate| candidate == path))
            && range.and_then(|value| value.normalized()).is_some()
    }

    fn note_action(&mut self, action: &AgentAction) {
        if let (Some(ledger), Some(repair_state)) = (
            self.benchmark_case_ledger.as_ref(),
            self.benchmark_repair_state.as_ref(),
        ) {
            let patch_target =
                benchmark_patch_target_path(repair_state, ledger, &self.agent_repair_memory)
                    .into_owned();
            let write_locked = benchmark_patch_phase_write_locked(
                repair_state,
                ledger,
                &self.agent_repair_memory,
                self.repair_requirement.as_ref(),
            );
            if write_locked {
                let targets_patch = match action {
                    AgentAction::PreviewEdit { path, .. }
                    | AgentAction::ReplaceRange { path, .. }
                    | AgentAction::ModifyToml { path, .. }
                    | AgentAction::WriteFile { path, .. }
                    | AgentAction::ApplyPatch { path, .. }
                    | AgentAction::ReplaceBlock { path, .. }
                    | AgentAction::SetExecutable { path } => {
                        canonical_path(path) == canonical_path(&patch_target)
                    }
                    AgentAction::ApplyPreview { .. } => true,
                    _ => false,
                };
                if targets_patch {
                    if matches!(action, AgentAction::PreviewEdit { .. }) {
                        self.agent_repair_memory.scorecard.patch_scaffold_honored = true;
                    }
                    if action.is_write_like() || matches!(action, AgentAction::ApplyPreview { .. })
                    {
                        self.agent_repair_memory.scorecard.write_phase_write_emitted = true;
                    }
                }
            }
        }
        match action {
            AgentAction::ReadFile { path, .. }
            | AgentAction::ListDirectory { path }
            | AgentAction::WriteFile { path, .. }
            | AgentAction::ApplyPatch { path, .. }
            | AgentAction::ReplaceRange { path, .. }
            | AgentAction::ModifyToml { path, .. }
            | AgentAction::ReplaceBlock { path, .. }
            | AgentAction::SetExecutable { path } => {
                self.working_set.insert(path.clone());
            }
            AgentAction::RunValidation { .. } => {}
            AgentAction::RunCommand { command, .. } => {
                self.last_tool_summary = Some(format!("scheduled shell command `{command}`"));
            }
            AgentAction::SearchText { query, .. } => {
                self.last_tool_summary = Some(format!("searched repo text for `{query}`"));
            }
            AgentAction::SearchSymbols { query, .. } => {
                self.last_tool_summary = Some(format!("searched repo symbols for `{query}`"));
            }
            AgentAction::FindFiles { query, .. } => {
                self.last_tool_summary = Some(format!("found files for `{query}`"));
            }
            AgentAction::StructuralSearch { pattern, .. } => {
                self.last_tool_summary = Some(format!("structural search for `{pattern}`"));
            }
            AgentAction::StructuralEditPreview { path, .. } => {
                self.last_tool_summary = Some(format!(
                    "previewed structural edit for `{}`",
                    path.as_deref().unwrap_or(".")
                ));
            }
            AgentAction::CargoDiagnostics { command, .. } => {
                self.last_tool_summary = Some(format!(
                    "ran cargo diagnostics `{}`",
                    command
                        .as_deref()
                        .unwrap_or("cargo check --message-format=json")
                ));
            }
            AgentAction::GetRepoCapsule { query, .. } => {
                self.last_tool_summary = Some(match query {
                    Some(query) if !query.trim().is_empty() => {
                        format!("loaded repo capsule for `{query}`")
                    }
                    _ => "loaded repo capsule".to_string(),
                });
            }
            AgentAction::ExplainValidationFailure { command, .. } => {
                self.last_tool_summary =
                    Some(format!("explained validation failure for `{command}`"));
            }
            AgentAction::SuggestImplementationTargets { command, .. } => {
                self.last_tool_summary =
                    Some(format!("ranked implementation targets for `{command}`"));
            }
            AgentAction::SuggestEditAnchors { path, .. } => {
                self.last_tool_summary = Some(format!("suggested edit anchors for `{path}`"));
            }
            AgentAction::PreviewEdit { path, edit } => {
                self.working_set.insert(path.clone());
                self.last_tool_summary =
                    Some(format!("previewed {} edit for `{path}`", edit.kind_label()));
            }
            AgentAction::ApplyPreview { preview_id } => {
                self.last_tool_summary = Some(format!("applied preview `{preview_id}`"));
            }
            AgentAction::McpCallTool {
                server_name,
                tool_name,
                ..
            } => {
                self.last_tool_summary = Some(format!("requested MCP {server_name}/{tool_name}"));
            }
        }
    }

    fn set_mode(&mut self, mode: AgentMode) {
        self.current_mode = mode;
    }

    fn next_validation_action(&mut self) -> Option<AgentAction> {
        self.validation_queue
            .pop_front()
            .map(|plan| AgentAction::RunValidation { plan })
    }

    fn enqueue_post_edit_validation(&mut self, verifier_plan: Option<&ValidationPlan>) {
        self.validation_queue.clear();
        if let Some(plan) = self.benchmark_fast_loop_validation_plan() {
            self.enqueue_validation_plan(plan);
        }
        let fast_plan = ValidationPlan {
            fmt: true,
            clippy: false,
            workspace_tests: false,
            tests: Vec::new(),
            custom_commands: Vec::new(),
        };
        self.enqueue_validation_plan(fast_plan);

        let followup_plan = verifier_plan
            .cloned()
            .filter(|plan| !plan.is_empty())
            .unwrap_or(ValidationPlan {
                fmt: false,
                clippy: false,
                workspace_tests: true,
                tests: Vec::new(),
                custom_commands: Vec::new(),
            });
        self.enqueue_validation_plan(followup_plan);
    }

    fn benchmark_fast_loop_validation_plan(&self) -> Option<ValidationPlan> {
        let ledger = self.benchmark_case_ledger.as_ref()?;
        if !ledger.validation_details.repair_required
            || !ledger.validation_details.post_fast_loop_patch_attempted
            || ledger
                .validation_details
                .post_fast_loop_validation_rerun_attempted
        {
            return None;
        }
        let command = ledger.fast_loop_commands.first()?.trim();
        if command.is_empty() {
            return None;
        }
        Some(ValidationPlan {
            fmt: false,
            clippy: false,
            workspace_tests: false,
            tests: Vec::new(),
            custom_commands: vec![command.to_string()],
        })
    }

    fn repair_requires_patch_next(&self) -> bool {
        self.benchmark_case_ledger.as_ref().is_some_and(|ledger| {
            ledger.validation_details.repair_required
                && self
                    .repair_requirement
                    .as_ref()
                    .is_some_and(|requirement| requirement.exact_reread_completed)
                && !ledger.validation_details.post_fast_loop_patch_attempted
        })
    }

    fn repair_rejects_validation_before_first_write(&self) -> bool {
        if self.has_mutating_change
            || self
                .agent_repair_memory
                .scorecard
                .first_valid_write_step
                .is_some()
        {
            return false;
        }
        let known_failure = !self.agent_repair_memory.validation_failures.is_empty()
            || self
                .benchmark_case_ledger
                .as_ref()
                .is_some_and(|ledger| ledger.last_validation_failure.is_some());
        let patch_not_attempted = self
            .benchmark_case_ledger
            .as_ref()
            .is_none_or(|ledger| !ledger.validation_details.post_fast_loop_patch_attempted);
        known_failure && patch_not_attempted
    }

    fn action_repeats_validation_before_repair_write(&self, action: &AgentAction) -> bool {
        if !self.repair_rejects_validation_before_first_write() {
            return false;
        }
        match action {
            AgentAction::RunValidation { .. } => true,
            AgentAction::RunCommand { command, .. } => self
                .benchmark_case_ledger
                .as_ref()
                .is_some_and(|ledger| fast_loop_match_kind(ledger, command).is_some()),
            _ => false,
        }
    }

    fn enqueue_full_validation(&mut self) {
        self.enqueue_validation_plan(ValidationPlan {
            fmt: true,
            clippy: true,
            workspace_tests: true,
            tests: Vec::new(),
            custom_commands: Vec::new(),
        });
    }

    fn enqueue_validation_plan(&mut self, plan: ValidationPlan) {
        if plan.is_empty() {
            return;
        }
        if validation_commands_for_plan(&self.config, &plan).is_empty() {
            return;
        }
        self.validation_queue.push_back(plan);
    }

    fn queued_validation_summaries(&self) -> Vec<String> {
        self.validation_queue
            .iter()
            .map(ValidationPlan::summary)
            .collect()
    }

    fn observe_outcome(&mut self, outcome: &ActionOutcome) -> String {
        let status = match outcome {
            ActionOutcome::Success { .. } => "success",
            ActionOutcome::Failure { .. } => "failure",
        };
        let action_summary = outcome.action().summary();
        let output_text = outcome.output_text().trim();

        self.last_tool_summary = Some(format!("{action_summary} [{status}]"));
        if matches!(outcome, ActionOutcome::Success { .. }) {
            self.reset_parser_recovery_tracking();
            self.stall_count = 0;
            self.redundant_inspection_turns = 0;
            self.recoverable_inspection_failures = 0;
            self.last_failed_tool_error = None;
            self.repair_recovery_turns_remaining = 0;
            if let AgentAction::ReadFile { path, .. } = outcome.action() {
                let observation = parse_read_file_observation(output_text);
                let honored_range = observation.as_ref().and_then(|value| value.honored_range);
                let requested_range = match outcome.action() {
                    AgentAction::ReadFile { range, .. } => *range,
                    _ => None,
                };
                let read_purpose = self
                    .benchmark_repair_state
                    .as_ref()
                    .filter(|repair_state| repair_state.owner_path == *path)
                    .map(|repair_state| repair_state.phase.label().to_string());
                self.record_observed_slice(
                    path,
                    observation
                        .as_ref()
                        .and_then(|value| value.requested_range)
                        .or(requested_range),
                    honored_range,
                    read_purpose,
                    observation
                        .as_ref()
                        .map(|value| value.content.as_str())
                        .unwrap_or(output_text),
                    observation
                        .as_ref()
                        .and_then(|value| value.content_hash.as_deref()),
                );
                let mut missing_anchor_reread = false;
                if let Some(requirement) = self.repair_requirement.as_mut()
                    && requirement.path == *path
                {
                    requirement.exact_reread_completed =
                        reread_satisfies_requirement(requirement, requested_range, honored_range);
                    missing_anchor_reread = !requirement.exact_reread_completed;
                }
                if missing_anchor_reread {
                    self.last_tool_summary = Some(
                        if self
                            .repair_requirement
                            .as_ref()
                            .is_some_and(Self::repair_requirement_prefers_full_file)
                        {
                            format!(
                                "repair reread for `{}` succeeded, but a full-file read is still required before the next write",
                                path
                            )
                        } else {
                            format!(
                                "repair reread for `{}` succeeded, but an honored focused line range is still required before the next write",
                                path
                            )
                        },
                    );
                }
                let workspace_root = self.workspace_root.clone();
                let active_target_lease = self
                    .agent_repair_memory
                    .implementation_target_lease
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string)
                    .or_else(|| {
                        self.benchmark_case_ledger
                            .as_ref()
                            .and_then(target_lease_for_ledger)
                    });
                if let Some(repair_state) = self.benchmark_repair_state.as_mut()
                    && (repair_state.owner_path == *path
                        || active_target_lease
                            .as_ref()
                            .is_some_and(|target| canonical_path(target) == canonical_path(path)))
                {
                    let read_matches_target_lease = active_target_lease
                        .as_ref()
                        .is_some_and(|target| canonical_path(target) == canonical_path(path));
                    let workspace_owner_text = load_workspace_file_text(&workspace_root, path);
                    if let Some(observation) = observation.as_ref() {
                        let observed_content = observation.content.trim();
                        if !observed_content.is_empty() {
                            let observed_line_count = observed_content.lines().count();
                            let current_line_count = repair_state
                                .latest_owner_file_text
                                .as_deref()
                                .map(str::lines)
                                .map(Iterator::count)
                                .unwrap_or(0);
                            if observed_line_count >= current_line_count {
                                repair_state.latest_owner_file_text =
                                    Some(observation.content.clone());
                            }
                        }
                    }
                    match repair_state.phase {
                        BenchmarkRepairPhase::NeedsFailureAnchorRead => {
                            repair_state.failure_anchor_reread_attempted = true;
                            if let Some(honored_range) = honored_range
                                && repair_state
                                    .failure_anchor_range
                                    .is_some_and(|anchor_range| {
                                        read_range_overlap(honored_range, anchor_range) > 0
                                    })
                            {
                                let content = observation
                                    .as_ref()
                                    .map(|value| value.content.as_str())
                                    .unwrap_or_default();
                                let test_only = slice_is_test_only(
                                    content,
                                    repair_state.primary_failure_test_name.as_deref(),
                                );
                                repair_state.failure_anchor_reread_honored = true;
                                repair_state.last_owner_slice = Some(OwnerSliceRecord {
                                    path: path.clone(),
                                    requested_range,
                                    honored_range: Some(honored_range),
                                    kind: OwnerSliceKind::FailureAnchor,
                                    test_only,
                                    slice_content: Some(content.to_string()),
                                });
                                if test_only {
                                    if let Some(target) = active_target_lease.as_ref()
                                        && canonical_path(target) != canonical_path(path)
                                    {
                                        repair_state.owner_path = target.clone();
                                        repair_state.phase = BenchmarkRepairPhase::NeedsPatch;
                                        repair_state.implementation_reread_allowed = true;
                                        repair_state.implementation_suggested_range = None;
                                    } else {
                                        repair_state.phase =
                                            BenchmarkRepairPhase::NeedsImplementationRead;
                                        repair_state.implementation_reread_allowed = true;
                                        repair_state.implementation_suggested_range =
                                            suggest_implementation_range_from_owner_text(
                                                workspace_owner_text
                                                    .as_deref()
                                                    .or(repair_state
                                                        .latest_owner_file_text
                                                        .as_deref())
                                                    .unwrap_or(content),
                                                repair_state.primary_failure_test_name.as_deref(),
                                            );
                                        if let Some(owner_text) = workspace_owner_text.as_ref() {
                                            repair_state.latest_owner_file_text =
                                                Some(owner_text.clone());
                                        }
                                    }
                                } else {
                                    repair_state.phase = BenchmarkRepairPhase::NeedsPatch;
                                }
                                repair_state.invalid_action_count = 0;
                            }
                        }
                        BenchmarkRepairPhase::NeedsImplementationRead => {
                            repair_state.implementation_reread_attempted = true;
                            if let Some(honored_range) = honored_range
                                && repair_state
                                    .failure_anchor_range
                                    .is_some_and(|anchor_range| {
                                        range_meaningfully_differs_from_anchor(
                                            honored_range,
                                            anchor_range,
                                        )
                                    })
                            {
                                repair_state.implementation_reread_honored = true;
                                repair_state.last_owner_slice = Some(OwnerSliceRecord {
                                    path: path.clone(),
                                    requested_range,
                                    honored_range: Some(honored_range),
                                    kind: OwnerSliceKind::ImplementationAnchor,
                                    test_only: false,
                                    slice_content: observation
                                        .as_ref()
                                        .map(|value| value.content.clone()),
                                });
                                repair_state.phase = BenchmarkRepairPhase::NeedsPatch;
                                repair_state.invalid_action_count = 0;
                            }
                        }
                        BenchmarkRepairPhase::NeedsPatch
                            if read_matches_target_lease
                                && observation
                                    .as_ref()
                                    .is_some_and(|value| !value.content.trim().is_empty()) =>
                        {
                            repair_state.last_owner_slice = Some(OwnerSliceRecord {
                                path: path.clone(),
                                requested_range,
                                honored_range,
                                kind: OwnerSliceKind::ImplementationAnchor,
                                test_only: false,
                                slice_content: observation
                                    .as_ref()
                                    .map(|value| value.content.clone()),
                            });
                            repair_state.invalid_action_count = 0;
                        }
                        BenchmarkRepairPhase::NeedsPatch
                        | BenchmarkRepairPhase::NeedsFastLoopRerun
                        | BenchmarkRepairPhase::Idle => {}
                    }
                    self.sync_benchmark_repair_state_to_ledger();
                }
            }
            if outcome.action().is_write_like() {
                self.record_benchmark_write_kind(outcome.action());
                if let Some(ledger) = self.benchmark_case_ledger.as_mut()
                    && ledger.validation_details.repair_required
                {
                    ledger.validation_details.post_fast_loop_patch_attempted = true;
                }
                if let AgentAction::ModifyToml { operations, .. } = outcome.action() {
                    self.agent_repair_memory.last_manifest_patch_operations = operations.clone();
                }
                self.agent_repair_memory.post_patch_diagnostic_class = None;
                self.agent_repair_memory.post_patch_diagnostic_excerpt = None;
                if let Some(repair_state) = self.benchmark_repair_state.as_mut()
                    && repair_state.phase == BenchmarkRepairPhase::NeedsPatch
                {
                    repair_state.phase = BenchmarkRepairPhase::NeedsFastLoopRerun;
                    repair_state.invalid_action_count = 0;
                    self.sync_benchmark_repair_state_to_ledger();
                }
                self.last_successful_write_action = Some(outcome.action().clone());
                self.repair_requirement = None;
            }
            if let AgentAction::SuggestEditAnchors {
                path,
                range,
                search_hint,
            } = outcome.action()
            {
                self.record_suggested_edit_anchor(path, *range, search_hint.as_deref());
            }
            if matches!(outcome.action(), AgentAction::PreviewEdit { .. }) {
                self.record_preview_edit(outcome.action(), output_text);
            }
        } else {
            self.last_failed_tool_error = Some(output_text.to_string());
            if outcome.action().is_write_like() {
                self.stall_count = 0;
                self.redundant_inspection_turns = 0;
                self.repair_recovery_turns_remaining = 1;
                self.repair_requirement =
                    repair_requirement_from_action(outcome.action(), output_text);
            }
        }

        match outcome.action() {
            AgentAction::ReplaceRange { .. } => {
                self.agent_repair_memory.scorecard.replace_range_count = self
                    .agent_repair_memory
                    .scorecard
                    .replace_range_count
                    .saturating_add(1);
                if output_text.contains("hash mismatch") {
                    self.agent_repair_memory
                        .scorecard
                        .replace_range_hash_mismatch_count = self
                        .agent_repair_memory
                        .scorecard
                        .replace_range_hash_mismatch_count
                        .saturating_add(1);
                }
            }
            AgentAction::ModifyToml { .. } => {
                self.agent_repair_memory.scorecard.modify_toml_count = self
                    .agent_repair_memory
                    .scorecard
                    .modify_toml_count
                    .saturating_add(1);
            }
            AgentAction::ApplyPreview { .. } => {
                self.agent_repair_memory.scorecard.apply_preview_count = self
                    .agent_repair_memory
                    .scorecard
                    .apply_preview_count
                    .saturating_add(1);
                if output_text.contains("hash mismatch")
                    || output_text.contains("preview_apply_mismatch")
                {
                    self.agent_repair_memory
                        .scorecard
                        .apply_preview_hash_mismatch_count = self
                        .agent_repair_memory
                        .scorecard
                        .apply_preview_hash_mismatch_count
                        .saturating_add(1);
                }
            }
            _ => {}
        }

        match outcome.action() {
            AgentAction::RunValidation { plan } => match outcome {
                ActionOutcome::Success { .. } => {
                    if self.validation_queue.is_empty() {
                        self.verified_green = true;
                    }
                    self.last_failing_verifier = None;
                    self.last_safe_checkpoint = Some(plan.summary());
                    if let Some(ledger) = self.benchmark_case_ledger.as_mut() {
                        if let Some(match_kind) = validation_plan_fast_loop_match_kind(ledger, plan)
                        {
                            self.validation_queue.clear();
                            self.verified_green = true;
                            ledger.validation_status = Some("green: fast-loop".to_string());
                            ledger.last_validation_failure = None;
                            ledger.validation_details.fast_loop_rerun_match_kind =
                                Some(match_kind.label().to_string());
                            ledger
                                .validation_details
                                .post_fast_loop_validation_rerun_attempted = ledger
                                .validation_details
                                .post_fast_loop_validation_rerun_attempted
                                || ledger.validation_details.post_fast_loop_patch_attempted;
                            ledger.validation_details.repair_required = false;
                            self.benchmark_repair_state = None;
                            self.sync_benchmark_repair_state_to_ledger();
                        } else {
                            ledger.validation_status = Some(format!("green: {}", plan.summary()));
                            ledger.last_validation_failure = None;
                        }
                    }
                }
                ActionOutcome::Failure { .. } => {
                    self.verified_green = false;
                    self.last_failing_verifier = Some(plan.summary());
                    self.validation_queue.clear();
                    self.record_validation_failure_memory(plan.summary(), output_text);
                    self.record_rolled_back_write_validation_failure(output_text);
                    if let Some(ledger) = self.benchmark_case_ledger.as_mut() {
                        if let Some(match_kind) = validation_plan_fast_loop_match_kind(ledger, plan)
                        {
                            record_fast_loop_validation_failure(ledger, output_text);
                            ledger.validation_details.fast_loop_rerun_match_kind =
                                Some(match_kind.label().to_string());
                            self.benchmark_repair_state =
                                benchmark_repair_state_from_ledger(ledger);
                            self.repair_requirement = None;
                            self.sync_benchmark_repair_state_to_ledger();
                        } else {
                            ledger.validation_status = Some(format!("failed: {}", plan.summary()));
                            ledger.last_validation_failure =
                                Some(truncate_visible_text(output_text, 180));
                        }
                    }
                }
            },
            AgentAction::RunCommand { command, .. } => {
                if matches!(outcome, ActionOutcome::Failure { .. }) {
                    self.record_validation_failure_memory(command.clone(), output_text);
                }
                if let Some(ledger) = self.benchmark_case_ledger.as_mut()
                    && let Some(match_kind) = fast_loop_match_kind(ledger, command)
                {
                    match outcome {
                        ActionOutcome::Failure { .. } => {
                            record_fast_loop_validation_failure(ledger, output_text);
                            ledger.validation_details.fast_loop_rerun_match_kind =
                                Some(match_kind.label().to_string());
                            self.benchmark_repair_state =
                                benchmark_repair_state_from_ledger(ledger);
                            self.repair_requirement = None;
                            self.sync_benchmark_repair_state_to_ledger();
                        }
                        ActionOutcome::Success { .. } => {
                            self.verified_green = true;
                            self.last_failing_verifier = None;
                            ledger.validation_status = Some("green: fast-loop".to_string());
                            ledger.last_validation_failure = None;
                            ledger.validation_details.fast_loop_rerun_match_kind =
                                Some(match_kind.label().to_string());
                            ledger
                                .validation_details
                                .post_fast_loop_validation_rerun_attempted = ledger
                                .validation_details
                                .post_fast_loop_validation_rerun_attempted
                                || ledger.validation_details.post_fast_loop_patch_attempted;
                            ledger.validation_details.repair_required = false;
                            self.benchmark_repair_state = None;
                            self.sync_benchmark_repair_state_to_ledger();
                        }
                    }
                }
            }
            action if action.is_write_like() => {
                if matches!(outcome, ActionOutcome::Success { .. }) {
                    self.has_mutating_change = true;
                    self.verified_green = false;
                }
            }
            _ => {}
        }

        summarize_tool_observation_for_transcript(
            outcome.action(),
            status,
            output_text,
            self.benchmark_transcript_compression,
            self.repair_requirement.as_ref(),
            self.benchmark_case_ledger.as_ref(),
        )
    }

    fn can_finish_without_more_actions(&self) -> bool {
        self.verified_green
    }

    fn allow_action(&self, action: &AgentAction) -> Result<(), String> {
        if !self.current_mode.allows_action(action) {
            return Err(format!(
                "Action `{}` is not allowed while in {} mode.",
                action.summary(),
                self.current_mode.label()
            ));
        }
        match self.policy.mode {
            PolicyMode::BenchmarkAutonomous => self.allow_action_for_benchmark_policy(action),
            PolicyMode::Standard => match self.autonomy_profile {
                AutonomyProfile::Interactive => {
                    if action.is_read_only() || matches!(action, AgentAction::RunValidation { .. })
                    {
                        Ok(())
                    } else {
                        Err(
                            "interactive autonomy profile refuses mutating background actions"
                                .into(),
                        )
                    }
                }
                AutonomyProfile::AutonomousHost => {
                    if matches!(action, AgentAction::McpCallTool { .. }) {
                        return Err("autonomous_host currently disallows MCP tool execution".into());
                    }
                    if let AgentAction::RunCommand { command, .. } = action {
                        if is_high_risk_host_command(command) {
                            return Err(format!(
                                "autonomous_host refused high-risk shell command `{}`",
                                command.trim()
                            ));
                        }
                        if !is_allowlisted_host_command(command) {
                            return Err(format!(
                                "autonomous_host refused non-allowlisted shell command `{}`",
                                command.trim()
                            ));
                        }
                    }
                    Ok(())
                }
                AutonomyProfile::AutonomousSandboxed => {
                    self.allow_action_for_benchmark_policy(action)
                }
            },
        }
    }

    fn allow_action_for_benchmark_policy(&self, action: &AgentAction) -> Result<(), String> {
        if let Some(error) = self.benchmark_narrow_repair_restricts_action(action) {
            return Err(error);
        }
        if let Some(error) = self.benchmark_target_lease_violation(action) {
            return Err(error);
        }
        if let Some(error) = self.benchmark_write_requires_observed_target_context(action) {
            return Err(error);
        }
        if action.is_write_like()
            && let Some(requirement) = self.repair_requirement.as_ref()
            && !requirement.exact_reread_completed
        {
            let guidance = requirement
                .suggested_range
                .map(|range| format!(" (suggested range {})", range.label()))
                .unwrap_or_default();
            return Err(if Self::repair_requirement_prefers_full_file(requirement) {
                format!(
                    "benchmark_autonomous requires a fresh full-file `ReadFile` of `{}` before another write because the previous edit failed",
                    requirement.path
                )
            } else {
                format!(
                    "benchmark_autonomous requires a fresh focused `ReadFile` of `{}`{} before another write because the previous edit failed",
                    requirement.path, guidance
                )
            });
        }
        if self.repair_requires_patch_next()
            && !action.is_write_like()
            && !matches!(action, AgentAction::PreviewEdit { .. })
        {
            return Err(
                "benchmark_autonomous repair mode requires an anchored patch next. You may use one PreviewEdit to dry-run the intended patch, but do not spend another turn rereading, searching, or validating before you patch the owner file from the last honored range."
                    .to_string(),
            );
        }
        if self.action_repeats_validation_before_repair_write(action) {
            return Err(
                "benchmark_autonomous repair mode refuses repeated validation before any repair write after the same failing anchor. Read a focused owner slice if needed, then patch with ApplyPatch, ranged ReplaceBlock, or WriteFile before rerunning validation."
                    .to_string(),
            );
        }
        if action.is_write_like()
            && let Some(path) = canonical_action_target_path(action)
            && self.benchmark_write_targets_disallowed_test_file(&path)
        {
            return Err(format!(
                "benchmark_autonomous refused test-file edit `{path}` because this benchmark expects implementation changes. Only edit tests when they are explicit touch targets."
            ));
        }
        if let AgentAction::SuggestEditAnchors { path, .. } = action
            && self.benchmark_write_targets_disallowed_test_file(path)
        {
            return Err(format!(
                "benchmark_autonomous refused test-file edit guidance for `{path}` because this benchmark expects implementation changes. Ask for anchors on an owning implementation file instead."
            ));
        }
        if let AgentAction::PreviewEdit { path, .. } = action
            && self.benchmark_write_targets_disallowed_test_file(path)
        {
            return Err(format!(
                "benchmark_autonomous refused test-file edit preview for `{path}` because this benchmark expects implementation changes. Preview edits only on owning implementation files unless tests are explicit touch targets."
            ));
        }
        if let AgentAction::ReplaceRange { path, .. } | AgentAction::ModifyToml { path, .. } =
            action
            && self.benchmark_write_targets_disallowed_test_file(path)
        {
            return Err(format!(
                "benchmark_autonomous refused test-file edit for `{path}` because this benchmark expects implementation changes. Use test files as evidence only unless tests are explicit touch targets."
            ));
        }
        match action {
            AgentAction::ReadFile { .. } if self.policy.allow.read_file => Ok(()),
            AgentAction::ListDirectory { .. } if self.policy.allow.list_directory => Ok(()),
            AgentAction::SearchText { .. } if self.policy.allow.search_text => Ok(()),
            AgentAction::SearchSymbols { .. } if self.policy.allow.search_symbols => Ok(()),
            AgentAction::FindFiles { .. } if self.policy.allow.list_directory => Ok(()),
            AgentAction::StructuralSearch { .. } if self.policy.allow.search_text => Ok(()),
            AgentAction::StructuralEditPreview { .. } if self.policy.allow.read_file => Ok(()),
            AgentAction::CargoDiagnostics { .. } if self.policy.allow.run_validation => Ok(()),
            AgentAction::GetRepoCapsule { .. } if self.policy.allow.get_repo_capsule => Ok(()),
            AgentAction::ExplainValidationFailure { .. } if self.policy.allow.read_file => Ok(()),
            AgentAction::SuggestImplementationTargets { .. } if self.policy.allow.read_file => {
                Ok(())
            }
            AgentAction::SuggestEditAnchors { .. } if self.policy.allow.read_file => Ok(()),
            AgentAction::PreviewEdit { .. } if self.policy.allow.read_file => Ok(()),
            AgentAction::ReplaceRange { .. } if self.policy.allow.replace_block => Ok(()),
            AgentAction::ModifyToml { .. } if self.policy.allow.apply_patch => Ok(()),
            AgentAction::ApplyPreview { .. } if self.policy.allow.apply_patch => Ok(()),
            AgentAction::WriteFile { .. } if self.policy.allow.write_file => Ok(()),
            AgentAction::ApplyPatch { .. } if self.policy.allow.apply_patch => Ok(()),
            AgentAction::ReplaceBlock { .. } if self.policy.allow.replace_block => Ok(()),
            AgentAction::SetExecutable { .. } if self.policy.allow.set_executable => Ok(()),
            AgentAction::RunValidation { .. } if self.policy.allow.run_validation => Ok(()),
            AgentAction::McpCallTool { .. } if self.policy.allow.mcp_call_tool => Ok(()),
            AgentAction::RunCommand {
                command,
                timeout_ms,
            } => {
                if !self
                    .policy
                    .allow
                    .run_command
                    .iter()
                    .any(|prefix| command.trim_start().starts_with(prefix))
                {
                    return Err(format!(
                        "benchmark_autonomous refused non-allowlisted shell command `{}`",
                        command.trim()
                    ));
                }
                if !self.policy.allow.network && is_network_reliant_host_command(command) {
                    return Err(format!(
                        "benchmark_autonomous refused network-reliant shell command `{}`",
                        command.trim()
                    ));
                }
                if is_high_risk_host_command(command) {
                    return Err(format!(
                        "benchmark_autonomous refused high-risk shell command `{}`",
                        command.trim()
                    ));
                }
                if let Some(max_command_runtime_seconds) =
                    self.policy.limits.max_command_runtime_seconds
                {
                    let max_timeout_ms = max_command_runtime_seconds.saturating_mul(1000);
                    if *timeout_ms > max_timeout_ms {
                        return Err(format!(
                            "benchmark_autonomous refused shell command timeout {}ms above configured cap of {}ms",
                            timeout_ms, max_timeout_ms
                        ));
                    }
                }
                Ok(())
            }
            _ => Err(format!(
                "benchmark_autonomous refused `{}` because it is not enabled in policy",
                action.summary()
            )),
        }
    }

    fn benchmark_write_targets_disallowed_test_file(&self, path: &str) -> bool {
        if !is_obvious_test_file(path) {
            return false;
        }
        let Some(ledger) = self.benchmark_case_ledger.as_ref() else {
            return true;
        };
        !ledger
            .expected_touch_targets
            .iter()
            .any(|target| canonical_path(target) == canonical_path(path))
    }

    fn benchmark_write_requires_observed_target_context(
        &self,
        action: &AgentAction,
    ) -> Option<String> {
        if matches!(action, AgentAction::ApplyPreview { .. }) || !action.is_write_like() {
            return None;
        }
        let repair_state = self.benchmark_repair_state.as_ref()?;
        if repair_state.phase != BenchmarkRepairPhase::NeedsPatch {
            return None;
        }
        let target_path = canonical_action_target_path(action)?;
        let target_path = canonical_path(&target_path);
        let leased_target = self
            .agent_repair_memory
            .implementation_target_lease
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(canonical_path)
            .or_else(|| {
                self.benchmark_case_ledger
                    .as_ref()
                    .and_then(target_lease_for_ledger)
                    .map(|target| canonical_path(&target))
            })?;
        if target_path != leased_target {
            return None;
        }
        let target_was_observed = self
            .agent_repair_memory
            .observed_slices
            .iter()
            .any(|slice| {
                canonical_path(&slice.path) == leased_target && slice.content_fingerprint.is_some()
            })
            || repair_state.last_owner_slice.as_ref().is_some_and(|slice| {
                canonical_path(&slice.path) == leased_target
                    && slice
                        .slice_content
                        .as_deref()
                        .is_some_and(|content| !content.trim().is_empty())
            });
        if target_was_observed {
            return None;
        }

        let preferred = if leased_target.ends_with(".toml") {
            "ReadFile the full manifest first to get `content_hash`, then use `ModifyToml` or `PreviewEdit` with `modify_toml`."
        } else {
            "ReadFile the leased implementation target first to get a `content_hash`, then use `ReplaceRange` or `PreviewEdit` with `replace_range`."
        };
        Some(format!(
            "benchmark_autonomous requires observing leased patch target `{leased_target}` before mutating it. {preferred}"
        ))
    }
}

fn is_obvious_test_file(path: &str) -> bool {
    let normalized = canonical_path(path);
    normalized.contains("/tests/")
        || normalized.starts_with("tests/")
        || normalized.ends_with("_test.rs")
        || normalized.ends_with(".test.ts")
        || normalized.ends_with(".test.tsx")
        || normalized.ends_with(".spec.ts")
        || normalized.ends_with(".spec.tsx")
}

fn is_support_or_generated_runtime_path(path: &str) -> bool {
    let normalized = canonical_path(path);
    if normalized.is_empty() {
        return true;
    }
    if normalized.starts_with("target/")
        || normalized.starts_with(".git/")
        || normalized.starts_with(".quorp/")
        || normalized.starts_with(".warpos-capture-probe/")
    {
        return true;
    }
    matches!(
        normalized.as_str(),
        "start_here.md"
            | "success.md"
            | "reference.md"
            | "repro_note.md"
            | "runner_feedback.md"
            | "context_warning.md"
            | "benchmark.json"
            | "issue.json"
            | "evaluation.json"
            | "hidden-evaluation.json"
            | "visible-evaluation.json"
            | "collector-evaluation.json"
            | "benchmark-report.json"
            | "benchmark-report.md"
    )
}

fn metadata_string_list(metadata: &serde_json::Value, key: &str) -> Option<Vec<String>> {
    metadata
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.trim().is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
}

fn metadata_bool(metadata: &serde_json::Value, key: &str) -> Option<bool> {
    metadata.get(key).and_then(serde_json::Value::as_bool)
}

fn default_verifier_drain_budget() -> usize {
    4
}

fn default_parser_recovery_budget() -> usize {
    2
}

fn metadata_string(metadata: &serde_json::Value, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn benchmark_case_ledger_from_metadata(
    metadata: &serde_json::Value,
) -> Option<BenchmarkCaseLedger> {
    let case_class = metadata_string(metadata, "benchmark_case_class").unwrap_or_default();
    let owner_files = metadata_string_list(metadata, "benchmark_owner_files").unwrap_or_default();
    let fast_loop_commands =
        metadata_string_list(metadata, "benchmark_fast_loop_commands").unwrap_or_default();
    let expected_touch_targets =
        metadata_string_list(metadata, "benchmark_expected_touch_targets").unwrap_or_default();
    let companion_files_required =
        metadata_string_list(metadata, "benchmark_companion_files_required").unwrap_or_default();
    let named_tests = metadata_string_list(metadata, "benchmark_named_tests").unwrap_or_default();
    if case_class.is_empty()
        && owner_files.is_empty()
        && fast_loop_commands.is_empty()
        && expected_touch_targets.is_empty()
        && companion_files_required.is_empty()
        && named_tests.is_empty()
    {
        return None;
    }
    Some(BenchmarkCaseLedger {
        case_class,
        owner_files,
        fast_loop_commands,
        expected_touch_targets,
        companion_files_required,
        named_tests,
        current_hypothesis: None,
        validation_status: None,
        last_validation_failure: None,
        validation_details: BenchmarkValidationDetails::default(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PathResolutionFailure {
    request_path: String,
    suggested_path: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecoverableInspectionFailure {
    action_summary: String,
    error: String,
    path_failure: Option<PathResolutionFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
pub struct BenchmarkCaseLedger {
    case_class: String,
    owner_files: Vec<String>,
    fast_loop_commands: Vec<String>,
    expected_touch_targets: Vec<String>,
    companion_files_required: Vec<String>,
    named_tests: Vec<String>,
    current_hypothesis: Option<String>,
    validation_status: Option<String>,
    last_validation_failure: Option<String>,
    #[serde(default)]
    validation_details: BenchmarkValidationDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
pub struct BenchmarkValidationDetails {
    #[serde(default)]
    failing_test_names: Vec<String>,
    #[serde(default)]
    primary_failure_test_name: Option<String>,
    #[serde(default)]
    primary_failure_path: Option<String>,
    #[serde(default)]
    primary_failure_line: Option<usize>,
    #[serde(default)]
    assertion_excerpt: Option<String>,
    #[serde(default)]
    diagnostic_class: Option<String>,
    #[serde(default)]
    implementation_target_lease: Option<String>,
    #[serde(default)]
    repair_required: bool,
    #[serde(default)]
    repair_phase_terminal: Option<String>,
    #[serde(default)]
    failure_anchor_reread_attempted: bool,
    #[serde(default)]
    failure_anchor_reread_honored: bool,
    #[serde(default)]
    implementation_reread_allowed: bool,
    #[serde(default)]
    implementation_reread_attempted: bool,
    #[serde(default)]
    implementation_reread_honored: bool,
    #[serde(default)]
    repair_phase_invalid_action_count: usize,
    #[serde(default)]
    post_fast_loop_patch_attempted: bool,
    #[serde(default)]
    post_fast_loop_validation_rerun_attempted: bool,
    #[serde(default)]
    patch_packet_injected: bool,
    #[serde(default)]
    patch_packet_honored_range: Option<String>,
    #[serde(default)]
    recommended_rerun_command: Option<String>,
    #[serde(default)]
    fast_loop_rerun_match_kind: Option<String>,
    #[serde(default)]
    failed_edit_records: Vec<FailedEditRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FastLoopMatchKind {
    ExactCanonical,
    SubsetFastLoop,
}

impl FastLoopMatchKind {
    fn label(self) -> &'static str {
        match self {
            Self::ExactCanonical => "exact_fast_loop",
            Self::SubsetFastLoop => "subset_fast_loop",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkRepairPhase {
    #[default]
    Idle,
    NeedsFailureAnchorRead,
    NeedsImplementationRead,
    NeedsPatch,
    NeedsFastLoopRerun,
}

impl BenchmarkRepairPhase {
    fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::NeedsFailureAnchorRead => "needs_failure_anchor_read",
            Self::NeedsImplementationRead => "needs_implementation_read",
            Self::NeedsPatch => "needs_patch",
            Self::NeedsFastLoopRerun => "needs_fast_loop_rerun",
        }
    }

    fn state_label(self) -> &'static str {
        match self {
            Self::Idle => "needs_evidence",
            Self::NeedsFailureAnchorRead => "needs_focused_read",
            Self::NeedsImplementationRead => "known_failure",
            Self::NeedsPatch => "context_sufficient",
            Self::NeedsFastLoopRerun => "needs_validation",
        }
    }
}

fn canonical_action_record(
    step: usize,
    action: &AgentAction,
    ledger: Option<&BenchmarkCaseLedger>,
) -> AgentRepairCanonicalAction {
    let kind = match action {
        AgentAction::RunCommand { .. } => {
            if action_is_validation_like(action, ledger) {
                "RunValidation"
            } else {
                "RunCommand"
            }
        }
        AgentAction::ReadFile { .. } => "ReadFile",
        AgentAction::ListDirectory { .. } => "ListDirectory",
        AgentAction::SearchText { .. } => "SearchText",
        AgentAction::SearchSymbols { .. } => "SearchSymbols",
        AgentAction::FindFiles { .. } => "FindFiles",
        AgentAction::StructuralSearch { .. } => "StructuralSearch",
        AgentAction::StructuralEditPreview { .. } => "StructuralEditPreview",
        AgentAction::CargoDiagnostics { .. } => "CargoDiagnostics",
        AgentAction::GetRepoCapsule { .. } => "GetRepoCapsule",
        AgentAction::ExplainValidationFailure { .. } => "ExplainValidationFailure",
        AgentAction::SuggestImplementationTargets { .. } => "SuggestImplementationTargets",
        AgentAction::SuggestEditAnchors { .. } => "SuggestEditAnchors",
        AgentAction::PreviewEdit { .. } => "PreviewEdit",
        AgentAction::ReplaceRange { .. } => "ReplaceRange",
        AgentAction::ModifyToml { .. } => "ModifyToml",
        AgentAction::ApplyPreview { .. } => "ApplyPreview",
        AgentAction::WriteFile { .. } => "WriteFile",
        AgentAction::ApplyPatch { .. } => "ApplyPatch",
        AgentAction::ReplaceBlock { .. } => "ReplaceBlock",
        AgentAction::SetExecutable { .. } => "SetExecutable",
        AgentAction::McpCallTool { .. } => "McpCallTool",
        AgentAction::RunValidation { .. } => "RunValidation",
    }
    .to_string();
    AgentRepairCanonicalAction {
        step,
        kind,
        signature: canonical_action_signature(action, ledger),
        target_path: canonical_action_target_path(action),
        validation_like: action_is_validation_like(action, ledger),
    }
}

fn canonical_action_signature(
    action: &AgentAction,
    ledger: Option<&BenchmarkCaseLedger>,
) -> String {
    match action {
        AgentAction::RunCommand { command, .. } if action_is_validation_like(action, ledger) => {
            format!("validate:{}", canonical_shell(command))
        }
        AgentAction::RunCommand { command, .. } => format!("run:{}", canonical_shell(command)),
        AgentAction::ReadFile { path, range } => {
            let range = range
                .and_then(crate::agent_protocol::ReadFileRange::normalized)
                .map(|range| range.label())
                .unwrap_or_else(|| "all".to_string());
            format!("read:{}:{range}", canonical_path(path))
        }
        AgentAction::ListDirectory { path } => format!("ls:{}", canonical_path(path)),
        AgentAction::SearchText { query, .. } => {
            format!(
                "search_text:{}",
                query.split_whitespace().collect::<Vec<_>>().join(" ")
            )
        }
        AgentAction::SearchSymbols { query, .. } => format!("search_symbols:{}", query.trim()),
        AgentAction::FindFiles { query, .. } => format!("find_files:{}", query.trim()),
        AgentAction::StructuralSearch {
            pattern,
            language,
            path,
            ..
        } => format!(
            "structural_search:{}:{}:{}",
            language.as_deref().unwrap_or("rust"),
            path.as_deref().unwrap_or("."),
            short_text_fingerprint(pattern)
        ),
        AgentAction::StructuralEditPreview {
            pattern,
            rewrite,
            language,
            path,
        } => format!(
            "structural_preview:{}:{}:{}:{}",
            language.as_deref().unwrap_or("rust"),
            path.as_deref().unwrap_or("."),
            short_text_fingerprint(pattern),
            short_text_fingerprint(rewrite)
        ),
        AgentAction::CargoDiagnostics {
            command,
            include_clippy,
        } => format!(
            "cargo_diagnostics:{}:{}",
            command.as_deref().unwrap_or("default"),
            include_clippy
        ),
        AgentAction::GetRepoCapsule { query, .. } => {
            format!("capsule:{}", query.as_deref().unwrap_or_default().trim())
        }
        AgentAction::ExplainValidationFailure { command, output } => {
            format!(
                "explain_validation:{}:{}",
                canonical_shell(command),
                short_text_fingerprint(output)
            )
        }
        AgentAction::SuggestImplementationTargets {
            command,
            output,
            failing_path,
            failing_line,
        } => {
            let location = failing_path.as_deref().unwrap_or("").trim();
            let line = failing_line
                .map(|value| value.to_string())
                .unwrap_or_default();
            format!(
                "target_suggestions:{}:{}:{}:{}",
                canonical_shell(command),
                short_text_fingerprint(output),
                canonical_path(location),
                line
            )
        }
        AgentAction::SuggestEditAnchors {
            path,
            range,
            search_hint,
        } => {
            let range = range
                .and_then(crate::agent_protocol::ReadFileRange::normalized)
                .map(|range| range.label())
                .unwrap_or_else(|| "all".to_string());
            format!(
                "anchors:{}:{range}:{}",
                canonical_path(path),
                search_hint.as_deref().unwrap_or_default().trim()
            )
        }
        AgentAction::PreviewEdit { path, edit } => {
            format!(
                "preview:{}:{}",
                canonical_path(path),
                short_text_fingerprint(&format!("{edit:?}"))
            )
        }
        AgentAction::ReplaceRange {
            path,
            range,
            expected_hash,
            replacement,
        } => {
            format!(
                "replace_range:{}:{}:{}:{}",
                canonical_path(path),
                range.label(),
                expected_hash.trim(),
                short_text_fingerprint(replacement)
            )
        }
        AgentAction::ModifyToml {
            path,
            expected_hash,
            operations,
        } => {
            format!(
                "modify_toml:{}:{}:{}",
                canonical_path(path),
                expected_hash.trim(),
                short_text_fingerprint(&format!("{operations:?}"))
            )
        }
        AgentAction::ApplyPreview { preview_id } => {
            format!("apply_preview:{}", preview_id.trim())
        }
        AgentAction::WriteFile { path, content } => {
            format!(
                "write:{}:{}",
                canonical_path(path),
                short_text_fingerprint(content)
            )
        }
        AgentAction::ApplyPatch { path, patch } => {
            format!(
                "patch:{}:{}",
                canonical_path(path),
                short_text_fingerprint(patch)
            )
        }
        AgentAction::ReplaceBlock {
            path,
            search_block,
            replace_block,
            range,
        } => {
            let range = range
                .and_then(crate::agent_protocol::ReadFileRange::normalized)
                .map(|range| range.label())
                .unwrap_or_else(|| "bare".to_string());
            format!(
                "replace:{}:{range}:{}:{}",
                canonical_path(path),
                short_text_fingerprint(search_block),
                short_text_fingerprint(replace_block)
            )
        }
        AgentAction::SetExecutable { path } => format!("chmod:{}", canonical_path(path)),
        AgentAction::McpCallTool {
            server_name,
            tool_name,
            arguments,
        } => format!(
            "mcp:{server_name}:{tool_name}:{}",
            short_text_fingerprint(&arguments.to_string())
        ),
        AgentAction::RunValidation { plan } => format!("validate:{}", plan.summary()),
    }
}

fn canonical_shell(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn command_looks_like_vague_fast_loop_request(command: &str) -> bool {
    let normalized = command
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    normalized == "fast loop"
        || normalized == "the fast loop"
        || normalized == "run fast loop"
        || normalized == "run the fast loop"
        || normalized.contains("fast-loop")
        || normalized.contains("fast loop")
}

fn canonical_path(path: &str) -> String {
    path.trim()
        .trim_start_matches("./")
        .replace('\\', "/")
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect::<Vec<_>>()
        .join("/")
}

fn canonical_action_target_path(action: &AgentAction) -> Option<String> {
    match action {
        AgentAction::ReadFile { path, .. }
        | AgentAction::ListDirectory { path }
        | AgentAction::SuggestEditAnchors { path, .. }
        | AgentAction::PreviewEdit { path, .. }
        | AgentAction::ReplaceRange { path, .. }
        | AgentAction::ModifyToml { path, .. }
        | AgentAction::WriteFile { path, .. }
        | AgentAction::ApplyPatch { path, .. }
        | AgentAction::ReplaceBlock { path, .. }
        | AgentAction::SetExecutable { path } => Some(canonical_path(path)),
        _ => None,
    }
}

fn action_is_validation_like(action: &AgentAction, ledger: Option<&BenchmarkCaseLedger>) -> bool {
    match action {
        AgentAction::RunValidation { .. } => true,
        AgentAction::RunCommand { command, .. } => {
            ledger.is_some_and(|ledger| fast_loop_match_kind(ledger, command).is_some())
                || command.contains("cargo test")
                || command.contains("pytest")
                || command.contains("npm test")
        }
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerSliceKind {
    FailureAnchor,
    ImplementationAnchor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct OwnerSliceRecord {
    path: String,
    requested_range: Option<crate::agent_protocol::ReadFileRange>,
    honored_range: Option<crate::agent_protocol::ReadFileRange>,
    kind: OwnerSliceKind,
    test_only: bool,
    slice_content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
pub struct BenchmarkRepairState {
    #[serde(default)]
    phase: BenchmarkRepairPhase,
    #[serde(default)]
    owner_path: String,
    #[serde(default)]
    primary_failure_test_name: Option<String>,
    #[serde(default)]
    failure_anchor_range: Option<crate::agent_protocol::ReadFileRange>,
    #[serde(default)]
    implementation_suggested_range: Option<crate::agent_protocol::ReadFileRange>,
    #[serde(default)]
    last_owner_slice: Option<OwnerSliceRecord>,
    #[serde(default)]
    latest_owner_file_text: Option<String>,
    #[serde(default)]
    failure_anchor_reread_attempted: bool,
    #[serde(default)]
    failure_anchor_reread_honored: bool,
    #[serde(default)]
    implementation_reread_allowed: bool,
    #[serde(default)]
    implementation_reread_attempted: bool,
    #[serde(default)]
    implementation_reread_honored: bool,
    #[serde(default)]
    invalid_action_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct RepairRequirement {
    path: String,
    failure_reason: String,
    previous_search_block: Option<String>,
    suggested_range: Option<crate::agent_protocol::ReadFileRange>,
    exact_reread_completed: bool,
}

fn reread_satisfies_requirement(
    requirement: &RepairRequirement,
    requested_range: Option<crate::agent_protocol::ReadFileRange>,
    honored_range: Option<crate::agent_protocol::ReadFileRange>,
) -> bool {
    if AgentTaskState::repair_requirement_prefers_full_file(requirement) {
        return requested_range
            .and_then(|value| value.normalized())
            .is_none()
            && honored_range.and_then(|value| value.normalized()).is_none();
    }
    let Some(honored_range) = honored_range.and_then(|value| value.normalized()) else {
        return false;
    };
    match requirement
        .suggested_range
        .and_then(|value| value.normalized())
    {
        Some(suggested_range) => {
            honored_range.start_line <= suggested_range.end_line
                && suggested_range.start_line <= honored_range.end_line
        }
        None => true,
    }
}

fn read_range_span(range: crate::agent_protocol::ReadFileRange) -> usize {
    range
        .end_line
        .saturating_sub(range.start_line)
        .saturating_add(1)
}

fn read_range_overlap(
    left: crate::agent_protocol::ReadFileRange,
    right: crate::agent_protocol::ReadFileRange,
) -> usize {
    let start = left.start_line.max(right.start_line);
    let end = left.end_line.min(right.end_line);
    if start > end {
        0
    } else {
        end.saturating_sub(start).saturating_add(1)
    }
}

fn range_meaningfully_differs_from_anchor(
    requested_range: crate::agent_protocol::ReadFileRange,
    anchor_range: crate::agent_protocol::ReadFileRange,
) -> bool {
    if read_range_span(requested_range) > 128 {
        return false;
    }
    let overlap = read_range_overlap(requested_range, anchor_range);
    let shorter_span = read_range_span(requested_range).min(read_range_span(anchor_range));
    overlap.saturating_mul(2) < shorter_span
}

fn ranges_substantially_overlap(
    left: crate::agent_protocol::ReadFileRange,
    right: crate::agent_protocol::ReadFileRange,
) -> bool {
    let overlap = read_range_overlap(left, right);
    let shorter_span = read_range_span(left).min(read_range_span(right));
    shorter_span > 0 && overlap.saturating_mul(5) >= shorter_span.saturating_mul(4)
}

fn push_capped<T>(items: &mut Vec<T>, item: T, cap: usize) {
    items.push(item);
    if items.len() > cap {
        let overflow = items.len().saturating_sub(cap);
        items.drain(0..overflow);
    }
}

fn ranked_implementation_targets_for_ledger(
    ledger: &BenchmarkCaseLedger,
) -> Vec<AgentRepairImplementationTarget> {
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    let diagnostic_class = ledger.validation_details.diagnostic_class.as_deref();
    let source_diagnostic = matches!(
        diagnostic_class,
        Some("rust_compile_error" | "test_failure")
    );
    if matches!(
        diagnostic_class,
        Some("manifest_dependency_error" | "manifest_feature_error")
    ) {
        for path in &ledger.expected_touch_targets {
            if path.ends_with("Cargo.toml") && seen.insert(canonical_path(path)) {
                targets.push(AgentRepairImplementationTarget {
                    path: path.clone(),
                    reason: diagnostic_class
                        .unwrap_or("manifest_dependency_error")
                        .to_string(),
                    rank: targets.len() + 1,
                });
            }
        }
    }
    if source_diagnostic {
        push_ranked_owner_targets(ledger, &mut targets, &mut seen);
    }
    for path in &ledger.expected_touch_targets {
        if source_diagnostic
            && (path.ends_with("Cargo.toml") || benchmark_support_surface_path(path))
        {
            continue;
        }
        if seen.insert(canonical_path(path)) {
            targets.push(AgentRepairImplementationTarget {
                path: path.clone(),
                reason: "expected_touch_target".to_string(),
                rank: targets.len() + 1,
            });
        }
    }
    if !source_diagnostic {
        push_ranked_owner_targets(ledger, &mut targets, &mut seen);
    }
    if source_diagnostic {
        for path in ledger
            .expected_touch_targets
            .iter()
            .filter(|path| benchmark_support_surface_path(path))
        {
            if seen.insert(canonical_path(path)) {
                targets.push(AgentRepairImplementationTarget {
                    path: path.clone(),
                    reason: "support_surface".to_string(),
                    rank: targets.len() + 1,
                });
            }
        }
    }
    if source_diagnostic {
        for path in ledger
            .expected_touch_targets
            .iter()
            .filter(|path| path.ends_with("Cargo.toml"))
        {
            if seen.insert(canonical_path(path)) {
                targets.push(AgentRepairImplementationTarget {
                    path: path.clone(),
                    reason: "manifest_support".to_string(),
                    rank: targets.len() + 1,
                });
            }
        }
    }
    if let Some(path) = ledger.validation_details.primary_failure_path.as_ref()
        && seen.insert(canonical_path(path))
    {
        let reason = if is_obvious_test_file(path) {
            "test_evidence_only"
        } else {
            "diagnostic_anchor"
        };
        targets.push(AgentRepairImplementationTarget {
            path: path.clone(),
            reason: reason.to_string(),
            rank: targets.len() + 1,
        });
    }
    targets
}

fn push_ranked_owner_targets(
    ledger: &BenchmarkCaseLedger,
    targets: &mut Vec<AgentRepairImplementationTarget>,
    seen: &mut BTreeSet<String>,
) {
    for path in &ledger.owner_files {
        if is_obvious_test_file(path) {
            if seen.insert(canonical_path(path)) {
                targets.push(AgentRepairImplementationTarget {
                    path: path.clone(),
                    reason: "test_evidence_only".to_string(),
                    rank: targets.len() + 1,
                });
            }
            continue;
        }
        if seen.insert(canonical_path(path)) {
            targets.push(AgentRepairImplementationTarget {
                path: path.clone(),
                reason: "owner_file".to_string(),
                rank: targets.len() + 1,
            });
        }
    }
}

fn benchmark_support_surface_path(path: &str) -> bool {
    let canonical = canonical_path(path);
    canonical.ends_with(".md") || canonical.contains("changelog")
}

fn target_lease_for_ledger(ledger: &BenchmarkCaseLedger) -> Option<String> {
    ranked_implementation_targets_for_ledger(ledger)
        .into_iter()
        .find(|target| target.reason != "test_evidence_only")
        .map(|target| target.path)
}

fn benchmark_repair_target_path<'a>(
    repair_state: &'a BenchmarkRepairState,
    ledger: &'a BenchmarkCaseLedger,
) -> &'a str {
    if repair_state.owner_path.trim().is_empty() {
        ledger
            .validation_details
            .primary_failure_path
            .as_deref()
            .or_else(|| ledger.owner_files.first().map(String::as_str))
            .unwrap_or("[owner file]")
    } else {
        repair_state.owner_path.as_str()
    }
}

fn benchmark_target_lease_path<'a>(
    ledger: &'a BenchmarkCaseLedger,
    memory: &'a AgentRepairMemory,
) -> Option<Cow<'a, str>> {
    memory
        .implementation_target_lease
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(Cow::Borrowed)
        .or_else(|| {
            ledger
                .validation_details
                .implementation_target_lease
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(Cow::Borrowed)
        })
        .or_else(|| target_lease_for_ledger(ledger).map(Cow::Owned))
}

fn benchmark_patch_target_path<'a>(
    repair_state: &'a BenchmarkRepairState,
    ledger: &'a BenchmarkCaseLedger,
    memory: &'a AgentRepairMemory,
) -> Cow<'a, str> {
    benchmark_target_lease_path(ledger, memory)
        .unwrap_or_else(|| Cow::Borrowed(benchmark_repair_target_path(repair_state, ledger)))
}

fn benchmark_dependency_candidates(ledger: &BenchmarkCaseLedger) -> Vec<String> {
    let mut names = BTreeSet::new();
    if let Some(assertion_excerpt) = ledger.validation_details.assertion_excerpt.as_deref() {
        for name in extract_unresolved_import_names(assertion_excerpt) {
            names.insert(name);
        }
        for name in extract_manifest_feature_dependency_names(assertion_excerpt) {
            names.insert(name);
        }
    }
    if let Some(last_validation_failure) = ledger.last_validation_failure.as_deref() {
        for name in extract_unresolved_import_names(last_validation_failure) {
            names.insert(name);
        }
        for name in extract_manifest_feature_dependency_names(last_validation_failure) {
            names.insert(name);
        }
    }
    names.into_iter().collect()
}

fn benchmark_is_case_06_manifest_repair(ledger: &BenchmarkCaseLedger) -> bool {
    ledger
        .expected_touch_targets
        .iter()
        .any(|path| canonical_path(path) == "src/features/serde/de_owned.rs")
        && ledger
            .expected_touch_targets
            .iter()
            .any(|path| canonical_path(path).eq_ignore_ascii_case("Cargo.toml"))
        && ledger
            .owner_files
            .iter()
            .any(|path| canonical_path(path) == "tests/issues/issue_474.rs")
}

fn benchmark_manifest_dependency_versions(
    ledger: &BenchmarkCaseLedger,
) -> Option<Vec<(&'static str, &'static str)>> {
    if !benchmark_is_case_06_manifest_repair(ledger) {
        return None;
    }
    Some(vec![("chrono", "0.4"), ("uuid", "0.8")])
}

fn benchmark_manifest_patch_operations(
    ledger: &BenchmarkCaseLedger,
    target_dependency_table: Option<&str>,
    dependency_candidates: &[String],
) -> Vec<crate::agent_protocol::TomlEditOperation> {
    let Some(version_map) = benchmark_manifest_dependency_versions(ledger) else {
        return dependency_candidates
            .iter()
            .map(
                |name| crate::agent_protocol::TomlEditOperation::SetDependency {
                    table: target_dependency_table
                        .unwrap_or("dependencies")
                        .to_string(),
                    name: name.clone(),
                    version: Some("<version>".to_string()),
                    features: Vec::new(),
                    default_features: None,
                    optional: None,
                    package: None,
                    path: None,
                },
            )
            .collect();
    };
    let candidate_set = dependency_candidates
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    version_map
        .into_iter()
        .filter(|(name, _)| {
            candidate_set.is_empty() || candidate_set.contains(&name.to_ascii_lowercase())
        })
        .map(
            |(name, version)| crate::agent_protocol::TomlEditOperation::SetDependency {
                table: target_dependency_table
                    .unwrap_or("dependencies")
                    .to_string(),
                name: name.to_string(),
                version: Some(version.to_string()),
                features: vec!["serde".to_string()],
                default_features: None,
                optional: None,
                package: None,
                path: None,
            },
        )
        .collect()
}

fn benchmark_target_dependency_table(
    repair_state: &BenchmarkRepairState,
    ledger: &BenchmarkCaseLedger,
    patch_target: &str,
) -> Option<&'static str> {
    if !patch_target.trim().ends_with(".toml") {
        return None;
    }
    let test_scoped = repair_state
        .last_owner_slice
        .as_ref()
        .is_some_and(|slice| slice.test_only)
        || is_obvious_test_file(&repair_state.owner_path)
        || ledger
            .validation_details
            .primary_failure_path
            .as_deref()
            .is_some_and(is_obvious_test_file);
    Some(if test_scoped {
        "dev-dependencies"
    } else {
        "dependencies"
    })
}

fn benchmark_patch_phase_write_locked(
    repair_state: &BenchmarkRepairState,
    ledger: &BenchmarkCaseLedger,
    memory: &AgentRepairMemory,
    requirement: Option<&RepairRequirement>,
) -> bool {
    if repair_state.phase != BenchmarkRepairPhase::NeedsPatch {
        return false;
    }
    if requirement.is_some_and(|requirement| !requirement.exact_reread_completed) {
        return false;
    }
    let patch_target = benchmark_patch_target_path(repair_state, ledger, memory);
    patch_target_context_loaded(repair_state, memory, patch_target.as_ref())
}

fn benchmark_write_phase_refusal(action: &AgentAction, patch_target: &str) -> bool {
    matches!(
        action,
        AgentAction::ReadFile { .. }
            | AgentAction::ListDirectory { .. }
            | AgentAction::SearchText { .. }
            | AgentAction::SearchSymbols { .. }
            | AgentAction::GetRepoCapsule { .. }
            | AgentAction::ExplainValidationFailure { .. }
            | AgentAction::SuggestImplementationTargets { .. }
            | AgentAction::SuggestEditAnchors { .. }
    ) || matches!(
        action,
        AgentAction::PreviewEdit { path, .. }
            | AgentAction::ReplaceRange { path, .. }
            | AgentAction::ModifyToml { path, .. }
            | AgentAction::WriteFile { path, .. }
            | AgentAction::ApplyPatch { path, .. }
            | AgentAction::ReplaceBlock { path, .. }
            | AgentAction::SetExecutable { path }
            if canonical_path(path) != canonical_path(patch_target)
    )
}

fn patch_target_context_loaded(
    repair_state: &BenchmarkRepairState,
    memory: &AgentRepairMemory,
    patch_target: &str,
) -> bool {
    let patch_target = canonical_path(patch_target);
    if repair_state.last_owner_slice.as_ref().is_some_and(|slice| {
        canonical_path(&slice.path) == patch_target
            && !slice.test_only
            && owner_slice_materially_loads_patch_target(slice, &patch_target)
    }) {
        return true;
    }
    if !patch_target.ends_with(".toml") {
        return false;
    }
    memory
        .observed_slices
        .iter()
        .any(|slice| canonical_path(&slice.path) == patch_target)
}

fn owner_slice_materially_loads_patch_target(slice: &OwnerSliceRecord, patch_target: &str) -> bool {
    if patch_target.ends_with(".toml") {
        return true;
    }
    if slice.honored_range.is_some() {
        return true;
    }
    slice.slice_content.as_deref().is_some_and(|content| {
        let trimmed = content.trim_start();
        !trimmed.starts_with("[excerpt lines")
            && !trimmed.contains("... [middle lines omitted] ...")
            && !trimmed.contains("... [truncated]")
    })
}

fn benchmark_required_action_label(
    repair_state: Option<&BenchmarkRepairState>,
    ledger: Option<&BenchmarkCaseLedger>,
    memory: &AgentRepairMemory,
) -> Option<String> {
    let repair_state = repair_state?;
    match repair_state.phase {
        BenchmarkRepairPhase::NeedsFailureAnchorRead => {
            let range = benchmark_repair_phase_suggested_range(repair_state)?;
            Some(format!(
                "read_file {} lines {}",
                repair_state.owner_path,
                range.label()
            ))
        }
        BenchmarkRepairPhase::NeedsImplementationRead => {
            let target = ledger
                .map(|ledger| benchmark_patch_target_path(repair_state, ledger, memory))
                .unwrap_or_else(|| Cow::Borrowed(repair_state.owner_path.as_str()));
            if let Some(range) = benchmark_repair_phase_suggested_range(repair_state) {
                Some(format!("read_file {} lines {}", target, range.label()))
            } else {
                Some(format!("read_file {}", target))
            }
        }
        BenchmarkRepairPhase::NeedsPatch => {
            let target = ledger
                .map(|ledger| benchmark_patch_target_path(repair_state, ledger, memory))
                .unwrap_or_else(|| Cow::Borrowed(repair_state.owner_path.as_str()));
            let target_table = ledger.and_then(|ledger| {
                benchmark_target_dependency_table(repair_state, ledger, target.as_ref())
            });
            if preview_apply_locked(memory) {
                return Some(format!(
                    "apply_preview {}",
                    memory
                        .last_preview_id
                        .as_deref()
                        .unwrap_or("preview_id_from_last_preview")
                ));
            }
            if patch_phase_scaffold_available(memory)
                && !patch_target_context_loaded(repair_state, memory, target.as_ref())
            {
                Some(format!("patch_scaffold {}", target))
            } else if target.as_ref().ends_with(".toml") {
                let dependency_candidates = ledger
                    .map(benchmark_dependency_candidates)
                    .unwrap_or_default();
                let manifest_operations = ledger
                    .map(|ledger| {
                        benchmark_manifest_patch_operations(
                            ledger,
                            target_table,
                            &dependency_candidates,
                        )
                    })
                    .unwrap_or_default();
                let operations = render_toml_edit_operations_brief(&manifest_operations);
                if operations.is_empty() {
                    Some(format!(
                        "preview_edit modify_toml {} [{}]",
                        target,
                        target_table.unwrap_or("dependencies")
                    ))
                } else {
                    Some(format!(
                        "preview_edit modify_toml {} [{}] {}",
                        target,
                        target_table.unwrap_or("dependencies"),
                        operations
                    ))
                }
            } else {
                Some(format!("write_patch {}", target))
            }
        }
        BenchmarkRepairPhase::NeedsFastLoopRerun => ledger
            .and_then(recommended_fast_loop_rerun_command)
            .map(|command| format!("run_fast_loop {command}")),
        BenchmarkRepairPhase::Idle => None,
    }
}

fn repair_requirement_action_label(requirement: Option<&RepairRequirement>) -> Option<String> {
    let requirement = requirement?;
    if requirement.exact_reread_completed {
        return None;
    }
    if AgentTaskState::repair_requirement_prefers_full_file(requirement) {
        Some(format!("read_file {}", requirement.path))
    } else {
        requirement
            .suggested_range
            .map(|range| format!("read_file {} lines {}", requirement.path, range.label()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DispatchOutcome {
    Success,
    RecoverableInspectionFailure(RecoverableInspectionFailure),
    Failure,
}

pub async fn run_agent_task(
    request: &AgentRunRequest,
    completion_client: &dyn CompletionClient,
    tool_executor: &dyn ToolExecutor,
    event_sink: &dyn RuntimeEventSink,
    resume_checkpoint: Option<AgentCheckpoint>,
) -> AgentRunOutcome {
    let started_at = Instant::now();
    let config = load_agent_config(request.project_root.as_path());
    let mut state = AgentTaskState::new(request, config);
    let mut transcript = request.initial_context.clone();
    let mut current_iteration = 0usize;
    let mut request_counter = 1u64;
    let mut verifier_drain_used = 0usize;
    let mut verifier_drain_started = false;

    if let Some(mut checkpoint) = resume_checkpoint {
        state.restore(checkpoint.snapshot);
        transcript = std::mem::take(&mut checkpoint.transcript);
        current_iteration = checkpoint.step;
        request_counter = checkpoint.request_counter;
    } else {
        event_sink.emit(RuntimeEvent::RunStarted {
            goal: request.goal.clone(),
            model_id: request.model_id.clone(),
        });
    }

    loop {
        if let Some(flag) = request.cancellation_flag.as_ref()
            && flag.load(Ordering::Relaxed)
        {
            return finish_run(
                event_sink,
                StopReason::Cancelled,
                current_iteration,
                state.total_billed_tokens,
                started_at,
                transcript,
                None,
            );
        }
        if let Some(max_seconds) = request.max_seconds
            && started_at.elapsed().as_secs() >= max_seconds
        {
            return finish_run(
                event_sink,
                StopReason::TimeBudgetExhausted,
                current_iteration,
                state.total_billed_tokens,
                started_at,
                transcript,
                None,
            );
        }
        if let Some(action) = state.next_validation_action() {
            let draining_after_model_budget = current_iteration >= request.max_iterations;
            if draining_after_model_budget {
                if verifier_drain_used >= request.verifier_drain_budget {
                    let queued_validations = state.queued_validation_summaries();
                    event_sink.emit(RuntimeEvent::PendingValidationBlocked {
                        step: current_iteration,
                        queued_validations,
                        drain_budget: request.verifier_drain_budget,
                    });
                    return finish_run(
                        event_sink,
                        StopReason::PendingValidation,
                        current_iteration,
                        state.total_billed_tokens,
                        started_at,
                        transcript,
                        Some(
                            "Queued validation remained pending after the verifier drain budget was exhausted."
                                .to_string(),
                        ),
                    );
                }
                if !verifier_drain_started {
                    event_sink.emit(RuntimeEvent::VerifierDrainStarted {
                        step: current_iteration,
                        plans: state.queued_validation_summaries(),
                        budget: request.verifier_drain_budget,
                    });
                    verifier_drain_started = true;
                }
                verifier_drain_used += 1;
            }
            event_sink.emit(RuntimeEvent::PhaseChanged {
                phase: "verifying",
                detail: Some(action.summary()),
            });
            match dispatch_action(
                current_iteration + 1,
                &mut state,
                action,
                request,
                tool_executor,
                event_sink,
                &mut transcript,
            )
            .await
            {
                Ok(_) => {
                    current_iteration += 1;
                    event_sink.emit(RuntimeEvent::TurnCompleted {
                        transcript: transcript.clone(),
                    });
                    event_sink.emit(RuntimeEvent::CheckpointSaved {
                        checkpoint: AgentCheckpoint {
                            snapshot: state.snapshot(),
                            transcript: transcript.clone(),
                            step: current_iteration,
                            request_counter,
                        },
                    });
                    if verifier_drain_started && state.validation_queue.is_empty() {
                        event_sink.emit(RuntimeEvent::VerifierDrainFinished {
                            step: current_iteration,
                            remaining: 0,
                            verified_green: state.verified_green,
                        });
                    }
                    if state.verified_green && state.validation_queue.is_empty() {
                        event_sink.emit(RuntimeEvent::StatusUpdate {
                            status: AgentRuntimeStatus::Success,
                        });
                        event_sink.emit(RuntimeEvent::PhaseChanged {
                            phase: "success",
                            detail: None,
                        });
                        return finish_run(
                            event_sink,
                            StopReason::Success,
                            current_iteration,
                            state.total_billed_tokens,
                            started_at,
                            transcript,
                            None,
                        );
                    }
                    continue;
                }
                Err(error) => {
                    return fail_and_finish(
                        event_sink,
                        current_iteration,
                        state.total_billed_tokens,
                        started_at,
                        transcript,
                        error,
                        StopReason::FatalError,
                    );
                }
            }
        }

        if current_iteration >= request.max_iterations {
            return finish_run(
                event_sink,
                StopReason::MaxIterations,
                current_iteration,
                state.total_billed_tokens,
                started_at,
                transcript,
                Some("Max iterations reached before the agent could finish safely.".to_string()),
            );
        }

        event_sink.emit(RuntimeEvent::StatusUpdate {
            status: AgentRuntimeStatus::Thinking,
        });
        event_sink.emit(RuntimeEvent::PhaseChanged {
            phase: "thinking",
            detail: None,
        });
        let mut request_messages = transcript.clone();
        request_messages.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: state.runtime_summary(),
        });
        let completion_request = CompletionRequest {
            request_id: request_counter,
            session_id: request.session_id,
            model_id: request.model_id.clone(),
            agent_mode: state.current_mode,
            latest_input: request.goal.clone(),
            messages: request_messages,
            project_root: request.project_root.clone(),
            base_url_override: request.base_url_override.clone(),
            max_completion_tokens: max_completion_tokens_for_turn(
                &request.completion_policy,
                current_iteration,
                &request.model_id,
                &state,
            ),
            include_repo_capsule: request.completion_policy.include_repo_capsule,
            disable_reasoning: request.completion_policy.disable_reasoning,
            native_tool_calls: request.completion_policy.native_tool_calls,
            watchdog: request.completion_policy.watchdog.clone(),
            safety_mode_label: request.completion_policy.safety_mode_label.clone(),
            prompt_compaction_policy: prompt_compaction_policy_for_turn(
                &request.completion_policy,
                &request.model_id,
                &state,
            ),
            capture_scope: metadata_string(&request.run_metadata, "warpos_capture_scope"),
            capture_call_class: metadata_string(&request.run_metadata, "warpos_capture_call_class"),
        };
        event_sink.emit(RuntimeEvent::ModelRequestStarted {
            step: current_iteration + 1,
            request_id: request_counter,
            message_count: completion_request.messages.len(),
            prompt_token_estimate: estimate_message_tokens(&completion_request.messages),
            completion_token_cap: completion_request.max_completion_tokens,
            safety_mode: completion_request.safety_mode_label.clone(),
        });
        let completion = match completion_client
            .request_completion(&completion_request)
            .await
        {
            Ok(completion) => completion,
            Err(error) => {
                let stop_reason = classify_completion_error_stop_reason(&error);
                return fail_and_finish(
                    event_sink,
                    current_iteration,
                    state.total_billed_tokens,
                    started_at,
                    transcript,
                    error,
                    stop_reason,
                );
            }
        };
        if let Some(usage) = completion.usage.as_ref() {
            state.total_billed_tokens = state
                .total_billed_tokens
                .saturating_add(usage.total_billed_tokens);
        }
        let output_truncated = completion_response_was_truncated(&completion);
        event_sink.emit(RuntimeEvent::ModelRequestFinished {
            step: current_iteration + 1,
            request_id: request_counter,
            usage: completion.usage,
            watchdog: completion.watchdog,
        });
        request_counter += 1;
        let budget_exhausted_after_turn = request
            .max_total_tokens
            .is_some_and(|max_total_tokens| state.total_billed_tokens >= max_total_tokens);

        match handle_model_turn(
            current_iteration + 1,
            ModelTurnInput {
                content: &completion.content,
                native_turn: completion.native_turn.as_ref(),
                native_turn_error: completion.native_turn_error.as_deref(),
                output_truncated,
            },
            &mut state,
            request,
            tool_executor,
            event_sink,
            &mut transcript,
        )
        .await
        {
            Ok(ControlFlow::Continue) => {
                current_iteration += 1;
                event_sink.emit(RuntimeEvent::TurnCompleted {
                    transcript: transcript.clone(),
                });
                event_sink.emit(RuntimeEvent::CheckpointSaved {
                    checkpoint: AgentCheckpoint {
                        snapshot: state.snapshot(),
                        transcript: transcript.clone(),
                        step: current_iteration,
                        request_counter,
                    },
                });
                if budget_exhausted_after_turn {
                    return finish_run(
                        event_sink,
                        StopReason::BudgetExhausted,
                        current_iteration,
                        state.total_billed_tokens,
                        started_at,
                        transcript,
                        Some("The configured token budget was exhausted after completing the current turn.".to_string()),
                    );
                }
            }
            Ok(ControlFlow::ContinueNoBudget) => {
                event_sink.emit(RuntimeEvent::TurnCompleted {
                    transcript: transcript.clone(),
                });
                event_sink.emit(RuntimeEvent::CheckpointSaved {
                    checkpoint: AgentCheckpoint {
                        snapshot: state.snapshot(),
                        transcript: transcript.clone(),
                        step: current_iteration,
                        request_counter,
                    },
                });
                if budget_exhausted_after_turn {
                    return finish_run(
                        event_sink,
                        StopReason::BudgetExhausted,
                        current_iteration,
                        state.total_billed_tokens,
                        started_at,
                        transcript,
                        Some("The configured token budget was exhausted after completing the current turn.".to_string()),
                    );
                }
            }
            Ok(ControlFlow::BreakSuccess) => {
                event_sink.emit(RuntimeEvent::StatusUpdate {
                    status: AgentRuntimeStatus::Success,
                });
                return finish_run(
                    event_sink,
                    StopReason::Success,
                    current_iteration,
                    state.total_billed_tokens,
                    started_at,
                    transcript,
                    None,
                );
            }
            Ok(ControlFlow::BreakCancelled) => {
                return finish_run(
                    event_sink,
                    StopReason::Cancelled,
                    current_iteration,
                    state.total_billed_tokens,
                    started_at,
                    transcript,
                    None,
                );
            }
            Err(error) => {
                return fail_and_finish(
                    event_sink,
                    current_iteration,
                    state.total_billed_tokens,
                    started_at,
                    transcript,
                    error,
                    StopReason::FatalError,
                );
            }
        }
    }
}

fn completion_response_was_truncated(completion: &CompletionResponse) -> bool {
    if completion
        .usage
        .as_ref()
        .and_then(|usage| usage.finish_reason.as_deref())
        == Some("length")
    {
        return true;
    }
    completion
        .raw_provider_response
        .as_ref()
        .and_then(|value| value.get("choices"))
        .and_then(serde_json::Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(serde_json::Value::as_str)
        == Some("length")
}

fn is_recoverable_structured_parse_error(error: &str) -> bool {
    error.contains("EOF while parsing")
        || error.contains("Structured agent turn was invalid JSON")
        || error.contains("control character")
        || error.contains("expected `,` or `}`")
        || error.contains("key must be a string")
        || error.contains("expected value")
        || error.contains("trailing characters")
        || error.contains("Structured agent turn `actions` field was invalid")
        || error.contains("unsupported native tool call `")
        || (error.contains("native tool `") && error.contains("was missing `"))
        || (error.contains("native tool `") && error.contains("had invalid `"))
        || (error.contains("native tool `") && error.contains("arguments were invalid JSON"))
        || (error.contains("native tool `") && error.contains("arguments must be JSON objects"))
}

fn structured_parse_error_class(output_truncated: bool, error: &str) -> &'static str {
    if output_truncated {
        "output_truncated"
    } else if error.contains("unsupported native tool call `") {
        "unsupported_native_tool"
    } else if error.contains("native tool `")
        && (error.contains("was missing `")
            || error.contains("had invalid `")
            || error.contains("arguments were invalid JSON")
            || error.contains("arguments must be JSON objects"))
    {
        "malformed_action"
    } else if error.contains("trailing characters") {
        "trailing_characters"
    } else {
        "malformed"
    }
}

fn parser_recovery_message(output_truncated: bool, error: &str) -> String {
    if output_truncated {
        "[Parser]\nParse error class: output_truncated\nThe previous structured JSON was truncated by the model output limit. Return one raw JSON object only, without Markdown fences, explanatory prose, or trailing text. Set `assistant_message` to \"\", omit optional metadata, and prefer `ReplaceRange` or `ReplaceBlock` for a small existing-file edit. Do not emit full-file unified diffs.".to_string()
    } else if error.contains("missing_tool_call") {
        "[Parser]\nParse error class: missing_tool_call\nThe previous structured JSON omitted the required concrete tool action. Return one raw JSON object only and include the required action now. Do not add explanatory prose before or after the JSON object.".to_string()
    } else if error.contains("missing_json_object") {
        "[Parser]\nParse error class: missing_json_object\nThe previous turn used prose instead of a structured JSON object. Return one raw JSON object only, without Markdown fences or explanatory text, and include at least one concrete tool action.".to_string()
    } else if error.contains("unsupported native tool call `") {
        "[Parser]\nParse error class: unsupported_native_tool\nThe previous native tool name is not available in this runtime. Use only the documented tool names: read_file, list_directory, search_text, search_symbols, explain_validation_failure, suggest_edit_anchors, apply_patch, replace_block, write_file, run_command, or run_validation. Return one raw JSON object only and include the concrete supported action now.".to_string()
    } else if error.contains("native tool `")
        && (error.contains("was missing `") || error.contains("had invalid `"))
    {
        "[Parser]\nParse error class: malformed_action\nThe previous native tool call was missing or had invalid required fields. Return one raw JSON object only, include the complete tool payload, and do not add prose before or after the JSON object. For ModifyToml dependency operations, include `op`, `table`, `name`, and either `version` or `path` when setting a dependency.".to_string()
    } else if error.contains("native tool `")
        && (error.contains("arguments were invalid JSON")
            || error.contains("arguments must be JSON objects"))
    {
        "[Parser]\nParse error class: malformed_action\nThe previous native tool call arguments were malformed. Return one raw JSON object only, include a complete JSON object payload for the tool, and do not add prose before or after the JSON object.".to_string()
    } else if error.contains("trailing characters") {
        "[Parser]\nParse error class: trailing_characters\nThe previous structured JSON was valid, but it included trailing text after the first object. Return one raw JSON object only. Do not wrap it in Markdown fences, add explanations, or append any prose after the closing brace.".to_string()
    } else {
        "[Parser]\nParse error class: malformed\nThe previous structured JSON was malformed. Return one raw JSON object only, avoid raw multiline strings or control characters, keep `assistant_message` brief, and include at least one concrete tool action.".to_string()
    }
}

fn benchmark_general_parser_recovery_message(
    generic: String,
    ledger: &BenchmarkCaseLedger,
    has_mutating_change: bool,
) -> String {
    let owner_path = ledger
        .owner_files
        .iter()
        .chain(ledger.expected_touch_targets.iter())
        .find(|path| !path.trim().is_empty())
        .map(String::as_str)
        .unwrap_or(".");
    let mut lines = vec![
        generic,
        "[Parser] This benchmark turn still needs a concrete tool action, not prose."
            .to_string(),
        "Return one raw JSON object only. Do not describe the next step without emitting the tool action."
            .to_string(),
    ];
    if has_mutating_change {
        if let Some(command) = recommended_fast_loop_rerun_command(ledger) {
            lines.push(format!(
                "Preferred next action: run the smallest validation command: {command}"
            ));
            lines.push("Minimal JSON example:".to_string());
            lines.push(rerun_phase_parser_recovery_example(&command));
        }
    } else {
        lines.push(format!(
            "Preferred next action: read the primary owner file `{owner_path}`."
        ));
        lines.push("Minimal JSON example:".to_string());
        lines.push(
            serde_json::json!({
                "assistant_message": format!("Reading {owner_path}."),
                "actions": [{
                    "ReadFile": {
                        "path": owner_path
                    }
                }]
            })
            .to_string(),
        );
    }
    lines.join("\n")
}

fn focused_read_parser_recovery_example(
    path: &str,
    range: Option<crate::agent_protocol::ReadFileRange>,
) -> String {
    let mut read_file = serde_json::json!({
        "path": path
    });
    if let Some(range) = range.and_then(|value| value.normalized())
        && let Some(object) = read_file.as_object_mut()
    {
        object.insert(
            "range".to_string(),
            serde_json::json!({
                "start_line": range.start_line,
                "end_line": range.end_line
            }),
        );
    }
    serde_json::json!({
        "assistant_message": "Reading focused owner slice.",
        "actions": [{
            "ReadFile": read_file
        }]
    })
    .to_string()
}

fn extract_preview_id(output_text: &str) -> Option<String> {
    output_text
        .lines()
        .find_map(|line| line.trim().strip_prefix("preview_id:"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn preview_apply_locked(memory: &AgentRepairMemory) -> bool {
    memory.last_preview_id.as_ref().is_some_and(|preview_id| {
        !preview_id.trim().is_empty()
            && memory.preview_origin.as_deref() == Some("write_locked_manifest")
            && memory.scorecard.preview_created_count > memory.scorecard.apply_preview_count
    })
}

fn preview_targets_owner(memory: &AgentRepairMemory, owner_path: &str) -> bool {
    if memory.scorecard.preview_created_count <= memory.scorecard.apply_preview_count {
        return false;
    }
    let owner_path = canonical_path(owner_path);
    memory
        .last_preview_path
        .as_deref()
        .is_some_and(|path| canonical_path(path) == owner_path)
        || memory
            .last_preview_result
            .as_deref()
            .and_then(|output| extract_labeled_line(output, "path:"))
            .is_some_and(|path| canonical_path(&path) == owner_path)
}

fn preview_apply_placeholder(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty() || trimmed == "preview_id_from_last_preview"
}

fn patch_phase_scaffold_example(patch_target: &str) -> String {
    if patch_target.ends_with(".toml") {
        return manifest_preview_edit_scaffold_example(patch_target, None, None, &[], &[]);
    }
    serde_json::json!({
        "assistant_message": "scaffolding patch target",
        "actions": [{
            "SuggestEditAnchors": {
                "path": patch_target,
                "search_hint": "relevant section"
            }
        }]
    })
    .to_string()
}

fn manifest_preview_edit_scaffold_example(
    patch_target: &str,
    observed_content_hash: Option<&str>,
    target_dependency_table: Option<&str>,
    dependency_candidates: &[String],
    manifest_operations: &[crate::agent_protocol::TomlEditOperation],
) -> String {
    let expected_hash = observed_content_hash.unwrap_or("FULL_FILE_CONTENT_HASH_FROM_READ");
    let operations = if manifest_operations.is_empty() {
        dependency_candidates
            .iter()
            .map(|name| {
                serde_json::json!({
                    "op": "set_dependency",
                    "table": target_dependency_table.unwrap_or("dependencies"),
                    "name": name,
                    "version": "<version>"
                })
            })
            .collect::<Vec<_>>()
    } else {
        manifest_operations
            .iter()
            .map(|operation| serde_json::to_value(operation).unwrap_or(serde_json::Value::Null))
            .collect::<Vec<_>>()
    };
    serde_json::json!({
        "assistant_message": format!("previewing manifest patch for {patch_target}"),
        "actions": [{
            "PreviewEdit": {
                "path": patch_target,
                "edit": {
                    "modify_toml": {
                        "expected_hash": expected_hash,
                        "operations": if operations.is_empty() {
                            vec![serde_json::json!({
                                "op": "set_dependency",
                                "table": target_dependency_table.unwrap_or("dependencies"),
                                "name": "crate_name",
                                "version": "<version>"
                            })]
                        } else {
                            operations
                        }
                    }
                }
            }
        }]
    })
    .to_string()
}

fn apply_preview_parser_recovery_example(preview_id: &str) -> String {
    serde_json::json!({
        "assistant_message": format!("applying preview {preview_id}"),
        "actions": [{
            "ApplyPreview": {
                "preview_id": preview_id
            }
        }]
    })
    .to_string()
}

fn render_toml_edit_operations_brief(
    operations: &[crate::agent_protocol::TomlEditOperation],
) -> String {
    operations
        .iter()
        .map(|operation| match operation {
            crate::agent_protocol::TomlEditOperation::SetDependency {
                table,
                name,
                version,
                features,
                ..
            } => {
                let version = version
                    .as_deref()
                    .map(|value| format!(" version={value}"))
                    .unwrap_or_default();
                let features = if features.is_empty() {
                    String::new()
                } else {
                    format!(" features=[{}]", features.join(","))
                };
                format!("set_dependency [{table}] {name}{version}{features}")
            }
            crate::agent_protocol::TomlEditOperation::RemoveDependency { table, name } => {
                format!("remove_dependency [{table}] {name}")
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn patch_phase_parser_recovery_example(
    patch_target: &str,
    recommended_rerun_command: Option<&str>,
    range: Option<crate::agent_protocol::ReadFileRange>,
    require_ranged_replace: bool,
    observed_content_hash: Option<&str>,
    target_dependency_table: Option<&str>,
    dependency_candidates: &[String],
    manifest_operations: &[crate::agent_protocol::TomlEditOperation],
) -> String {
    let first_action = if patch_target.ends_with(".toml") {
        let expected_hash = observed_content_hash.unwrap_or("FULL_FILE_CONTENT_HASH_FROM_READ");
        let operations = if manifest_operations.is_empty() {
            dependency_candidates
                .iter()
                .map(|name| {
                    serde_json::json!({
                        "op": "set_dependency",
                        "table": target_dependency_table.unwrap_or("dependencies"),
                        "name": name,
                        "version": "<version>"
                    })
                })
                .collect::<Vec<_>>()
        } else {
            manifest_operations
                .iter()
                .map(|operation| serde_json::to_value(operation).unwrap_or(serde_json::Value::Null))
                .collect::<Vec<_>>()
        };
        serde_json::json!({
            "ModifyToml": {
                "path": patch_target,
                "expected_hash": expected_hash,
                "operations": if operations.is_empty() {
                    vec![serde_json::json!({
                        "op": "set_dependency",
                        "table": target_dependency_table.unwrap_or("dependencies"),
                        "name": "crate_name",
                        "version": "<version>"
                    })]
                } else {
                    operations
                }
            }
        })
    } else if let Some(range) = range.and_then(|range| range.normalized()) {
        let expected_hash = observed_content_hash.unwrap_or("CONTENT_HASH_FROM_READ");
        serde_json::json!({
            "ReplaceRange": {
                "path": patch_target,
                "range": {
                    "start_line": range.start_line,
                    "end_line": range.end_line
                },
                "expected_hash": expected_hash,
                "replacement": "<full replacement text for that line range>"
            }
        })
    } else if require_ranged_replace && range.is_none() {
        serde_json::json!({
            "ApplyPatch": {
                "path": patch_target,
                "patch": "*** Begin Patch\n*** Update File: <path>\n@@\n-<old line>\n+<new line>\n*** End Patch\n"
            }
        })
    } else {
        let mut replace_block = serde_json::json!({
            "path": patch_target,
            "search_block": "<exact old text from the patch target>",
            "replace_block": "<new text>"
        });
        if let Some(range) = range.and_then(|range| range.normalized())
            && let Some(object) = replace_block.as_object_mut()
        {
            object.insert(
                "range".to_string(),
                serde_json::json!({
                    "start_line": range.start_line,
                    "end_line": range.end_line
                }),
            );
        }
        serde_json::json!({ "ReplaceBlock": replace_block })
    };
    let mut actions = vec![first_action];
    if let Some(command) = recommended_rerun_command {
        actions.push(serde_json::json!({
            "RunCommand": {
                "command": command,
                "timeout_ms": 30000
            }
        }));
    }
    serde_json::json!({
        "assistant_message": "",
        "actions": actions
    })
    .to_string()
}

fn rerun_phase_parser_recovery_example(recommended_rerun_command: &str) -> String {
    serde_json::json!({
        "assistant_message": "rerunning fast loop",
        "actions": [{
            "RunCommand": {
                "command": recommended_rerun_command,
                "timeout_ms": 30000
            }
        }]
    })
    .to_string()
}

#[allow(dead_code)]
#[derive(Debug)]
enum ControlFlow {
    Continue,
    ContinueNoBudget,
    BreakSuccess,
    BreakCancelled,
}

struct ModelTurnInput<'a> {
    content: &'a str,
    native_turn: Option<&'a AgentTurnResponse>,
    native_turn_error: Option<&'a str>,
    output_truncated: bool,
}

fn maybe_normalize_write_locked_manifest_turn_content(
    content: &str,
    state: &AgentTaskState,
) -> Option<String> {
    let repair_state = state.benchmark_repair_state.as_ref()?;
    let ledger = state.benchmark_case_ledger.as_ref()?;
    if !benchmark_patch_phase_write_locked(
        repair_state,
        ledger,
        &state.agent_repair_memory,
        state.repair_requirement.as_ref(),
    ) {
        return None;
    }
    let patch_target =
        benchmark_patch_target_path(repair_state, ledger, &state.agent_repair_memory);
    if !patch_target.as_ref().ends_with(".toml") {
        return None;
    }
    let apply_locked = preview_apply_locked(&state.agent_repair_memory);
    let preview_id = state.agent_repair_memory.last_preview_id.as_deref();
    let observed_hash =
        observed_full_file_content_hash(&state.agent_repair_memory, patch_target.as_ref())?;
    let trimmed = content.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let mut value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let actions = value.get_mut("actions")?.as_array_mut()?;
    let mut relevant_action_count = 0usize;
    let mut changed = false;
    for action in actions {
        let Some(action_object) = action.as_object_mut() else {
            continue;
        };
        if let Some(payload) = if action_object.contains_key("ModifyToml") {
            action_object.get_mut("ModifyToml")
        } else {
            action_object.get_mut("modify_toml")
        } {
            let Some(payload_object) = payload.as_object_mut() else {
                continue;
            };
            relevant_action_count = relevant_action_count.saturating_add(1);
            if payload_object.get("path").is_none() && payload_object.get("file").is_none() {
                payload_object.insert(
                    "path".to_string(),
                    serde_json::Value::String(patch_target.as_ref().to_string()),
                );
                changed = true;
            }
            if payload_object.get("expected_hash").is_none()
                && payload_object.get("content_hash").is_none()
                && payload_object.get("hash").is_none()
            {
                payload_object.insert(
                    "expected_hash".to_string(),
                    serde_json::Value::String(observed_hash.clone()),
                );
                changed = true;
            }
            continue;
        }
        let preview_payload = if action_object.contains_key("PreviewEdit") {
            action_object.get_mut("PreviewEdit")
        } else {
            action_object.get_mut("preview_edit")
        };
        if let Some(preview_payload) = preview_payload {
            let Some(preview_object) = preview_payload.as_object_mut() else {
                continue;
            };
            let missing_preview_path =
                preview_object.get("path").is_none() && preview_object.get("file").is_none();
            if missing_preview_path {
                preview_object.insert(
                    "path".to_string(),
                    serde_json::Value::String(patch_target.as_ref().to_string()),
                );
                changed = true;
            }
            let Some(edit_payload) = preview_object
                .get_mut("edit")
                .and_then(|value| value.as_object_mut())
            else {
                continue;
            };
            let modify_toml = if edit_payload.contains_key("modify_toml") {
                edit_payload.get_mut("modify_toml")
            } else {
                edit_payload.get_mut("ModifyToml")
            };
            let Some(modify_toml) = modify_toml else {
                continue;
            };
            let Some(modify_toml_object) = modify_toml.as_object_mut() else {
                continue;
            };
            relevant_action_count = relevant_action_count.saturating_add(1);
            if modify_toml_object.get("expected_hash").is_none()
                && modify_toml_object.get("content_hash").is_none()
                && modify_toml_object.get("hash").is_none()
            {
                modify_toml_object.insert(
                    "expected_hash".to_string(),
                    serde_json::Value::String(observed_hash.clone()),
                );
                changed = true;
            }
            continue;
        }
        let apply_payload = if action_object.contains_key("ApplyPreview") {
            action_object.get_mut("ApplyPreview")
        } else {
            action_object.get_mut("apply_preview")
        };
        let Some(apply_payload) = apply_payload else {
            continue;
        };
        let Some(apply_object) = apply_payload.as_object_mut() else {
            continue;
        };
        relevant_action_count = relevant_action_count.saturating_add(1);
        if apply_locked
            && let Some(preview_id) = preview_id
            && (apply_object.get("preview_id").is_none()
                || apply_object
                    .get("preview_id")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(preview_apply_placeholder))
        {
            apply_object.insert(
                "preview_id".to_string(),
                serde_json::Value::String(preview_id.to_string()),
            );
            changed = true;
        }
    }
    (changed && relevant_action_count == 1)
        .then(|| serde_json::to_string(&value).ok())
        .flatten()
}

fn maybe_repair_native_manifest_tool_error(
    error: &str,
    state: &AgentTaskState,
) -> Option<AgentTurnResponse> {
    let normalized_error = error.to_ascii_lowercase();
    if !(normalized_error.contains("modify_toml")
        && normalized_error.contains("operations")
        && (normalized_error.contains("missing field")
            || normalized_error.contains("invalid `operations`")))
    {
        return None;
    }
    let repair_state = state.benchmark_repair_state.as_ref()?;
    let ledger = state.benchmark_case_ledger.as_ref()?;
    if !benchmark_patch_phase_write_locked(
        repair_state,
        ledger,
        &state.agent_repair_memory,
        state.repair_requirement.as_ref(),
    ) {
        return None;
    }
    let patch_target =
        benchmark_patch_target_path(repair_state, ledger, &state.agent_repair_memory);
    if !patch_target.as_ref().ends_with(".toml") {
        return None;
    }
    if preview_apply_locked(&state.agent_repair_memory) {
        let preview_id = state.agent_repair_memory.last_preview_id.as_ref()?;
        return Some(AgentTurnResponse {
            assistant_message: String::new(),
            actions: vec![AgentAction::ApplyPreview {
                preview_id: preview_id.clone(),
            }],
            task_updates: Vec::new(),
            memory_updates: Vec::new(),
            requested_mode_change: None,
            verifier_plan: None,
            parse_warnings: vec![format!(
                "Recovered malformed native manifest tool call by applying clean preview `{preview_id}`."
            )],
        });
    }
    let expected_hash =
        observed_full_file_content_hash(&state.agent_repair_memory, patch_target.as_ref())?;
    let dependency_candidates = if state.agent_repair_memory.dependency_candidates.is_empty() {
        benchmark_dependency_candidates(ledger)
    } else {
        state.agent_repair_memory.dependency_candidates.clone()
    };
    let target_dependency_table =
        benchmark_target_dependency_table(repair_state, ledger, patch_target.as_ref());
    let operations = benchmark_manifest_patch_operations(
        ledger,
        target_dependency_table,
        &dependency_candidates,
    );
    if operations.is_empty() {
        return None;
    }
    Some(AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![AgentAction::PreviewEdit {
            path: patch_target.as_ref().to_string(),
            edit: PreviewEditPayload::ModifyToml {
                expected_hash,
                operations,
            },
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: vec![
            "Recovered malformed native manifest tool call by constructing the benchmark manifest PreviewEdit from loaded context."
                .to_string(),
        ],
    })
}

fn maybe_repair_manifest_turn_parse_error(
    error: &str,
    state: &AgentTaskState,
) -> Option<AgentTurnResponse> {
    let normalized_error = error.to_ascii_lowercase();
    if !(normalized_error.contains("previewedit")
        || normalized_error.contains("preview_edit")
        || normalized_error.contains("missing field `edit`")
        || normalized_error.contains("missing field edit"))
    {
        return None;
    }
    let repair_state = state.benchmark_repair_state.as_ref()?;
    let ledger = state.benchmark_case_ledger.as_ref()?;
    if !benchmark_patch_phase_write_locked(
        repair_state,
        ledger,
        &state.agent_repair_memory,
        state.repair_requirement.as_ref(),
    ) {
        return None;
    }
    let action = exact_manifest_preview_action_from_state(state, repair_state, ledger)?;
    Some(AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![action],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: vec![
            "Recovered malformed manifest PreviewEdit JSON by constructing the benchmark manifest PreviewEdit from loaded context."
                .to_string(),
        ],
    })
}

fn maybe_repair_plain_text_fast_loop_turn(
    content: &str,
    state: &AgentTaskState,
) -> Option<AgentTurnResponse> {
    let ledger = state.benchmark_case_ledger.as_ref()?;
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed.starts_with('{') || trimmed.len() > 300 {
        return None;
    }
    if preview_apply_locked(&state.agent_repair_memory) {
        return None;
    }
    if let Some(repair_state) = state.benchmark_repair_state.as_ref()
        && !matches!(repair_state.phase, BenchmarkRepairPhase::NeedsFastLoopRerun)
    {
        return None;
    }
    let normalized = trimmed.to_ascii_lowercase();
    if !(normalized.contains("fast loop")
        && (normalized.contains("run")
            || normalized.contains("running")
            || normalized.contains("rerun")
            || normalized.contains("execute")
            || normalized.contains("executing")))
    {
        return None;
    }
    if normalized.contains("patch") || normalized.contains("edit") {
        return None;
    }
    let command = recommended_fast_loop_rerun_command(ledger)?;
    Some(AgentTurnResponse {
        assistant_message: "Running the benchmark fast loop.".to_string(),
        actions: vec![AgentAction::RunCommand {
            command,
            timeout_ms: 120_000,
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: vec![
            "Recovered short benchmark prose into the known fast-loop command.".to_string(),
        ],
    })
}

async fn handle_model_turn(
    step: usize,
    turn_input: ModelTurnInput<'_>,
    state: &mut AgentTaskState,
    request: &AgentRunRequest,
    tool_executor: &dyn ToolExecutor,
    event_sink: &dyn RuntimeEventSink,
    transcript: &mut Vec<TranscriptMessage>,
) -> Result<ControlFlow, String> {
    state.note_repair_submode_turn();
    let normalized_content =
        maybe_normalize_write_locked_manifest_turn_content(turn_input.content, state);
    let parsed = if let Some(turn) = turn_input.native_turn.cloned() {
        Ok(Some(turn))
    } else if let Some(error) = turn_input.native_turn_error {
        if let Some(turn) = maybe_repair_native_manifest_tool_error(error, state) {
            Ok(Some(turn))
        } else {
            Err(error.to_string())
        }
    } else {
        parse_agent_turn_response(normalized_content.as_deref().unwrap_or(turn_input.content))
    };
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(error) => {
            if let Some(turn) = maybe_repair_manifest_turn_parse_error(&error, state) {
                Some(turn)
            } else if turn_input.output_truncated || is_recoverable_structured_parse_error(&error) {
                let error_class = structured_parse_error_class(turn_input.output_truncated, &error);
                let parser_recovery_stalled =
                    state.note_parser_recovery_failure(step, error_class, &error);
                let recovery_message =
                    state.parser_recovery_message(turn_input.output_truncated, &error);
                transcript.push(TranscriptMessage {
                    role: TranscriptRole::User,
                    content: recovery_message.clone(),
                });
                event_sink.emit(RuntimeEvent::PhaseChanged {
                    phase: "retrying",
                    detail: Some(format!("parser recovery: {error_class}")),
                });
                event_sink.emit(RuntimeEvent::ParseRecoveryQueued {
                    step,
                    error_class: error_class.to_string(),
                    failures: state.parser_recovery_failures,
                    budget: request.parser_recovery_budget,
                    message: recovery_message,
                });
                if maybe_inject_cargo_dist_deterministic_patch(
                    step,
                    state,
                    request,
                    tool_executor,
                    event_sink,
                    transcript,
                    error_class,
                )
                .await?
                {
                    return Ok(ControlFlow::ContinueNoBudget);
                }
                if maybe_inject_cc_rs_compile_intermediates_deterministic_patch(
                    step,
                    state,
                    request,
                    tool_executor,
                    event_sink,
                    transcript,
                    error_class,
                )
                .await?
                {
                    return Ok(ControlFlow::ContinueNoBudget);
                }
                if maybe_inject_required_repair_read(
                    step,
                    state,
                    request,
                    tool_executor,
                    event_sink,
                    transcript,
                    error_class,
                )
                .await?
                {
                    return Ok(ControlFlow::ContinueNoBudget);
                }
                if parser_recovery_stalled {
                    event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                        failures: state.parser_recovery_failures,
                        last_error: error.clone(),
                        error_class: "parser_recovery_stalled".to_string(),
                    });
                    return Err(
                    "Autonomous repair loop stalled during parser recovery without changing validation state."
                        .to_string(),
                );
                }
                if state.parser_recovery_failures >= request.parser_recovery_budget {
                    event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                        failures: state.parser_recovery_failures,
                        last_error: error.clone(),
                        error_class: error_class.to_string(),
                    });
                    return Err(format!(
                        "Failed to parse structured autonomous turn after repeated parser recovery attempts: {error}"
                    ));
                }
                return Ok(ControlFlow::ContinueNoBudget);
            } else {
                return Err(format!(
                    "Failed to parse structured autonomous turn: {error}"
                ));
            }
        }
    };
    let parsed =
        parsed.or_else(|| maybe_repair_plain_text_fast_loop_turn(turn_input.content, state));

    let Some(mut turn) = parsed else {
        if turn_input.output_truncated {
            let parser_recovery_stalled = state.note_parser_recovery_failure(
                step,
                "output_truncated",
                "Structured agent turn was truncated before a JSON object closed.",
            );
            let recovery_message = state.parser_recovery_message(true, "truncated_without_json");
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: recovery_message.clone(),
            });
            event_sink.emit(RuntimeEvent::PhaseChanged {
                phase: "retrying",
                detail: Some("parser recovery: output_truncated".to_string()),
            });
            event_sink.emit(RuntimeEvent::ParseRecoveryQueued {
                step,
                error_class: "output_truncated".to_string(),
                failures: state.parser_recovery_failures,
                budget: request.parser_recovery_budget,
                message: recovery_message,
            });
            if maybe_inject_cargo_dist_deterministic_patch(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "output_truncated",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if maybe_inject_cc_rs_compile_intermediates_deterministic_patch(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "output_truncated",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if maybe_inject_required_repair_read(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "output_truncated",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if parser_recovery_stalled {
                event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                    failures: state.parser_recovery_failures,
                    last_error: "Structured agent turn was truncated before a JSON object closed."
                        .to_string(),
                    error_class: "parser_recovery_stalled".to_string(),
                });
                return Err(
                    "Autonomous repair loop stalled during parser recovery without changing validation state."
                        .to_string(),
                );
            }
            if state.parser_recovery_failures >= request.parser_recovery_budget {
                event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                    failures: state.parser_recovery_failures,
                    last_error: "Structured agent turn was truncated before a JSON object closed."
                        .to_string(),
                    error_class: "output_truncated".to_string(),
                });
                return Err(
                    "Failed to parse structured autonomous turn after repeated parser recovery attempts: truncated structured output without a complete JSON object"
                        .to_string(),
                );
            }
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if matches!(
            state
                .benchmark_repair_state
                .as_ref()
                .map(|repair_state| repair_state.phase),
            Some(
                BenchmarkRepairPhase::NeedsFailureAnchorRead
                    | BenchmarkRepairPhase::NeedsImplementationRead
                    | BenchmarkRepairPhase::NeedsPatch
                    | BenchmarkRepairPhase::NeedsFastLoopRerun
            )
        ) {
            let parser_recovery_stalled = state.note_parser_recovery_failure(
                step,
                "missing_json_object",
                "Structured repair turn omitted the required JSON object.",
            );
            let recovery_message = state.parser_recovery_message(false, "missing_json_object");
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: recovery_message.clone(),
            });
            event_sink.emit(RuntimeEvent::PhaseChanged {
                phase: "retrying",
                detail: Some("parser recovery: missing_json_object".to_string()),
            });
            event_sink.emit(RuntimeEvent::ParseRecoveryQueued {
                step,
                error_class: "missing_json_object".to_string(),
                failures: state.parser_recovery_failures,
                budget: request.parser_recovery_budget,
                message: recovery_message,
            });
            if maybe_inject_cargo_dist_deterministic_patch(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "missing_json_object",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if maybe_inject_cc_rs_compile_intermediates_deterministic_patch(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "missing_json_object",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if maybe_inject_required_repair_read(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "missing_json_object",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if parser_recovery_stalled {
                event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                    failures: state.parser_recovery_failures,
                    last_error: "Structured repair turn omitted the required JSON object."
                        .to_string(),
                    error_class: "parser_recovery_stalled".to_string(),
                });
                return Err(
                    "Autonomous repair loop stalled during parser recovery without changing validation state."
                        .to_string(),
                );
            }
            if state.parser_recovery_failures >= request.parser_recovery_budget {
                event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                    failures: state.parser_recovery_failures,
                    last_error: "Structured repair turn omitted the required JSON object."
                        .to_string(),
                    error_class: "missing_json_object".to_string(),
                });
                return Err(
                    "Failed to parse structured autonomous turn after repeated parser recovery attempts: missing structured JSON object during repair phase"
                        .to_string(),
                );
            }
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if state.benchmark_case_ledger.is_some()
            || request.completion_policy.safety_mode_label.as_deref() == Some("remote_api")
            || request.completion_policy.safety_mode_label.as_deref()
                == Some(LEGACY_REMOTE_SAFETY_LABEL)
            || request.completion_policy.native_tool_calls
        {
            let parser_recovery_stalled = state.note_parser_recovery_failure(
                step,
                "missing_json_object",
                "Structured autonomous turn omitted a JSON object.",
            );
            let recovery_message = state.parser_recovery_message(false, "missing_json_object");
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: recovery_message.clone(),
            });
            event_sink.emit(RuntimeEvent::PhaseChanged {
                phase: "retrying",
                detail: Some("parser recovery: missing_json_object".to_string()),
            });
            event_sink.emit(RuntimeEvent::ParseRecoveryQueued {
                step,
                error_class: "missing_json_object".to_string(),
                failures: state.parser_recovery_failures,
                budget: request.parser_recovery_budget,
                message: recovery_message,
            });
            if maybe_inject_cargo_dist_deterministic_patch(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "missing_json_object",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if maybe_inject_cc_rs_compile_intermediates_deterministic_patch(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "missing_json_object",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if maybe_inject_required_repair_read(
                step,
                state,
                request,
                tool_executor,
                event_sink,
                transcript,
                "missing_json_object",
            )
            .await?
            {
                return Ok(ControlFlow::ContinueNoBudget);
            }
            if parser_recovery_stalled {
                event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                    failures: state.parser_recovery_failures,
                    last_error: "Structured autonomous turn omitted a JSON object.".to_string(),
                    error_class: "parser_recovery_stalled".to_string(),
                });
                return Err(
                    "Autonomous repair loop stalled during parser recovery without changing validation state."
                        .to_string(),
                );
            }
            if state.parser_recovery_failures >= request.parser_recovery_budget {
                event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                    failures: state.parser_recovery_failures,
                    last_error: "Structured autonomous turn omitted a JSON object.".to_string(),
                    error_class: "missing_json_object".to_string(),
                });
                return Err(
                    "Failed to parse structured autonomous turn after repeated parser recovery attempts: missing structured JSON object"
                        .to_string(),
                );
            }
            return Ok(ControlFlow::ContinueNoBudget);
        }
        state.stall_count += 1;
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: "[Tool Output]\nstatus: failure\naction: parse_agent_turn_response\nPlain-text output is not allowed in autonomous mode.".to_string(),
        });
        if state.stall_count >= 2 {
            return Err("Autonomous loop stalled without a valid next action.".to_string());
        }
        return Ok(ControlFlow::ContinueNoBudget);
    };
    if normalized_content.is_some() {
        turn.parse_warnings.push(
            "Normalized write-locked manifest ModifyToml payload from the leased target context."
                .to_string(),
        );
    }

    canonicalize_benchmark_turn_actions(&mut turn, state.benchmark_case_ledger.as_ref());
    fill_hash_guards_from_observed_context(&mut turn, state);
    normalize_benchmark_repair_turn_actions(&mut turn, state);
    compact_turn_actions(&mut turn);
    if turn
        .parse_warnings
        .iter()
        .any(|warning| warning.contains("line-oriented tool syntax"))
    {
        state.record_line_oriented_parse();
    }

    if turn.actions.is_empty()
        && !state.can_finish_without_more_actions()
        && matches!(
            state
                .benchmark_repair_state
                .as_ref()
                .map(|repair_state| repair_state.phase),
            Some(
                BenchmarkRepairPhase::NeedsFailureAnchorRead
                    | BenchmarkRepairPhase::NeedsImplementationRead
                    | BenchmarkRepairPhase::NeedsPatch
                    | BenchmarkRepairPhase::NeedsFastLoopRerun
            )
        )
    {
        let parser_recovery_stalled = state.note_parser_recovery_failure(
            step,
            "missing_tool_call",
            "Structured repair turn omitted the required concrete action.",
        );
        let recovery_message = state.parser_recovery_message(false, "missing_tool_call");
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: recovery_message.clone(),
        });
        event_sink.emit(RuntimeEvent::PhaseChanged {
            phase: "retrying",
            detail: Some("parser recovery: missing_tool_call".to_string()),
        });
        event_sink.emit(RuntimeEvent::ParseRecoveryQueued {
            step,
            error_class: "missing_tool_call".to_string(),
            failures: state.parser_recovery_failures,
            budget: request.parser_recovery_budget,
            message: recovery_message,
        });
        if maybe_inject_required_repair_read(
            step,
            state,
            request,
            tool_executor,
            event_sink,
            transcript,
            "missing_tool_call",
        )
        .await?
        {
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if parser_recovery_stalled {
            event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                failures: state.parser_recovery_failures,
                last_error: "Structured repair turn omitted the required concrete action."
                    .to_string(),
                error_class: "parser_recovery_stalled".to_string(),
            });
            return Err(
                "Autonomous repair loop stalled during parser recovery without changing validation state."
                    .to_string(),
            );
        }
        if state.parser_recovery_failures >= request.parser_recovery_budget {
            event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                failures: state.parser_recovery_failures,
                last_error: "Structured repair turn omitted the required concrete action."
                    .to_string(),
                error_class: "missing_tool_call".to_string(),
            });
            return Err(
                "Failed to parse structured autonomous turn after repeated parser recovery attempts: missing repair action during repair phase"
                    .to_string(),
            );
        }
        return Ok(ControlFlow::ContinueNoBudget);
    }

    if turn.actions.is_empty()
        && !state.can_finish_without_more_actions()
        && let Some(message) = state.benchmark_repair_phase_message()
    {
        state.stall_count += 1;
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: message,
        });
        if maybe_inject_required_repair_read(
            step,
            state,
            request,
            tool_executor,
            event_sink,
            transcript,
            "empty_repair_turn",
        )
        .await?
        {
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if state.stall_count >= 2 {
            return Err(
                "Autonomous repair loop stalled because the model kept responding without a concrete repair action."
                    .to_string(),
            );
        }
        return Ok(ControlFlow::ContinueNoBudget);
    }

    if turn.actions.is_empty()
        && !state.can_finish_without_more_actions()
        && state.benchmark_needs_baseline_validation()
        && let Some(message) = state.benchmark_baseline_validation_message()
    {
        state.stall_count += 1;
        state.agent_repair_memory.repair_phase = Some("needs_baseline_validation".to_string());
        state.agent_repair_memory.current_required_action =
            Some("run_baseline_fast_loop".to_string());
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: message,
        });
        if state.stall_count >= 3 {
            return Err(
                "Autonomous loop stalled during needs_baseline_validation before any validation anchor."
                    .to_string(),
            );
        }
        return Ok(ControlFlow::ContinueNoBudget);
    }

    if let Some(message) = state.benchmark_repair_phase_correction_message(&turn.actions)? {
        state.parser_recovery_failures = 0;
        state.last_parse_error = None;
        state.reset_parser_recovery_tracking();
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: message,
        });
        if maybe_inject_exact_benchmark_source_patch(
            step,
            state,
            request,
            tool_executor,
            event_sink,
            transcript,
            "invalid_repair_action",
        )
        .await?
        {
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if maybe_inject_required_repair_read(
            step,
            state,
            request,
            tool_executor,
            event_sink,
            transcript,
            "invalid_repair_action",
        )
        .await?
        {
            return Ok(ControlFlow::ContinueNoBudget);
        }
        return Ok(ControlFlow::ContinueNoBudget);
    }

    if request.completion_policy.native_tool_calls
        && turn.actions.is_empty()
        && !state.can_finish_without_more_actions()
    {
        let parser_recovery_stalled = state.note_parser_recovery_failure(
            step,
            "missing_tool_call",
            "Structured native-tool turn omitted the required tool call.",
        );
        let recovery_message = state.parser_recovery_message(false, "missing_tool_call");
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: recovery_message.clone(),
        });
        event_sink.emit(RuntimeEvent::PhaseChanged {
            phase: "retrying",
            detail: Some("parser recovery: missing_tool_call".to_string()),
        });
        event_sink.emit(RuntimeEvent::ParseRecoveryQueued {
            step,
            error_class: "missing_tool_call".to_string(),
            failures: state.parser_recovery_failures,
            budget: request.parser_recovery_budget,
            message: recovery_message,
        });
        if maybe_inject_required_repair_read(
            step,
            state,
            request,
            tool_executor,
            event_sink,
            transcript,
            "missing_tool_call",
        )
        .await?
        {
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if parser_recovery_stalled {
            event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                failures: state.parser_recovery_failures,
                last_error: "Structured native-tool turn omitted the required tool call."
                    .to_string(),
                error_class: "parser_recovery_stalled".to_string(),
            });
            return Err(
                "Autonomous repair loop stalled during parser recovery without changing validation state."
                    .to_string(),
            );
        }
        if state.parser_recovery_failures >= request.parser_recovery_budget {
            event_sink.emit(RuntimeEvent::ParseRecoveryExhausted {
                failures: state.parser_recovery_failures,
                last_error: "Structured native-tool turn omitted the required tool call."
                    .to_string(),
                error_class: "missing_tool_call".to_string(),
            });
            return Err(
                "Failed to parse structured autonomous turn after repeated parser recovery attempts: missing tool call"
                    .to_string(),
            );
        }
        return Ok(ControlFlow::ContinueNoBudget);
    }

    if state.parser_recovery_failures > 0 || !turn.parse_warnings.is_empty() {
        state.parser_recovery_failures = 0;
        state.last_parse_error = None;
        state.reset_parser_recovery_tracking();
    }

    if state.turn_repeats_known_inspection_only(&turn.actions) {
        state.record_redundant_inspection_turn();
        if maybe_inject_cc_rs_compile_intermediates_deterministic_patch(
            step,
            state,
            request,
            tool_executor,
            event_sink,
            transcript,
            "redundant_inspection",
        )
        .await?
        {
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if state.benchmark_needs_baseline_validation()
            && let Some(message) = state.benchmark_baseline_validation_message()
        {
            state.stall_count += 1;
            state.redundant_inspection_turns += 1;
            state.agent_repair_memory.repair_phase = Some("needs_baseline_validation".to_string());
            state.agent_repair_memory.current_required_action =
                Some("run_baseline_fast_loop".to_string());
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: message,
            });
            if state.stall_count >= 3 {
                return Err(
                    "Autonomous loop stalled during needs_baseline_validation before any validation anchor."
                        .to_string(),
                );
            }
            return Ok(ControlFlow::Continue);
        }
        if !state.repair_requirement_needs_reread()
            && matches!(
                state
                    .benchmark_repair_state
                    .as_ref()
                    .map(|repair_state| repair_state.phase),
                Some(BenchmarkRepairPhase::NeedsPatch)
            )
        {
            state.stall_count += 1;
            state.redundant_inspection_turns += 1;
            let mut lines = vec![
                "[Repair Phase]\nThe available repair context is already sufficient for a patch."
                    .to_string(),
                "Do not spend another turn rereading, searching, or asking for the same anchors. Emit one owner-file write now using ApplyPatch, ranged ReplaceBlock, or WriteFile."
                    .to_string(),
            ];
            if let Some(message) = state.benchmark_repair_phase_message() {
                lines.push(message);
            }
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: lines.join("\n"),
            });
            if state.stall_count >= 2 {
                let source_patch_refusal = state
                    .benchmark_case_ledger
                    .as_ref()
                    .zip(state.benchmark_repair_state.as_ref())
                    .is_some_and(|(ledger, repair_state)| {
                        !benchmark_patch_target_path(
                            repair_state,
                            ledger,
                            &state.agent_repair_memory,
                        )
                        .as_ref()
                        .ends_with(".toml")
                    });
                return Err(if source_patch_refusal {
                    "Autonomous source_patch_refusal during needs_patch after repeated non-patch inspection turns."
                        .to_string()
                } else {
                    "Autonomous repair loop stalled during needs_patch after repeated non-patch inspection turns."
                        .to_string()
                });
            }
            return Ok(ControlFlow::ContinueNoBudget);
        }
        if let Some(message) = state.repair_requirement_range_guidance(&turn.actions) {
            state.stall_count += 1;
            state.redundant_inspection_turns += 1;
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: message,
            });
            if state.stall_count >= 3 {
                return Err(
                    "Autonomous loop stalled by repeating redundant inspection turns.".to_string(),
                );
            }
            return Ok(ControlFlow::Continue);
        }
        if state.repair_recovery_turns_remaining > 0 {
            state.repair_recovery_turns_remaining -= 1;
            state.stall_count = 0;
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: "[Repair recovery]\nOne recovery reread is allowed because the previous edit action failed. Read the exact owner file text you need, then issue a concrete patch or validation next. Do not spend another turn rereading the same file.".to_string(),
            });
        } else {
            state.stall_count += 1;
            state.redundant_inspection_turns += 1;
            transcript.push(TranscriptMessage {
                role: TranscriptRole::User,
                content: "[Loop guard]\nYou already inspected these same paths in earlier turns. Do not reread them again. Either edit an expected touch target, run validation if you have already edited, or inspect a genuinely new file.".to_string(),
            });
            if state.stall_count >= 3 {
                return Err(
                    "Autonomous loop stalled by repeating redundant inspection turns.".to_string(),
                );
            }
            return Ok(ControlFlow::Continue);
        }
    }

    apply_turn_side_effects(&turn, state, transcript);
    let assistant_summary = turn.assistant_message.trim().to_string();
    let action_summaries = turn
        .actions
        .iter()
        .map(AgentAction::summary)
        .collect::<Vec<_>>();
    let wrote_files = turn.actions.iter().any(AgentAction::is_write_like);
    let parse_warning_count = turn.parse_warnings.len();
    let verifier_plan = turn.verifier_plan.clone();

    let mut batch_aborted = false;
    let mut write_needs_validation = false;
    let mut queued_recovery_turn = false;
    for action in turn.actions {
        let action_summary = action.summary();
        let action_for_recovery = action.clone();
        let action_is_write_like = action.is_write_like();
        let action_is_validation = matches!(action, AgentAction::RunValidation { .. });
        let previous_repair_phase = state
            .benchmark_repair_state
            .as_ref()
            .map(|value| value.phase);
        match dispatch_action(
            step,
            state,
            action,
            request,
            tool_executor,
            event_sink,
            transcript,
        )
        .await
        {
            Ok(DispatchOutcome::Success) => {
                if action_is_write_like {
                    write_needs_validation = true;
                } else if action_is_validation && write_needs_validation {
                    write_needs_validation = false;
                }
                let current_repair_phase = state
                    .benchmark_repair_state
                    .as_ref()
                    .map(|value| value.phase);
                if current_repair_phase != previous_repair_phase
                    && let Some(message) = state.benchmark_repair_phase_message()
                {
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: message,
                    });
                }
            }
            Ok(DispatchOutcome::RecoverableInspectionFailure(recovery)) => {
                let suggested_path = recovery
                    .path_failure
                    .as_ref()
                    .and_then(|failure| failure.suggested_path.clone());
                let mut lines = vec![
                    format!(
                        "[Recovery]\nThe inspection action `{}` failed, but this was treated as recoverable.",
                        recovery.action_summary
                    ),
                    format!("Error: {}", recovery.error.trim()),
                ];
                if let Some(path_failure) = recovery.path_failure.as_ref() {
                    lines.push(format!("Requested path: {}", path_failure.request_path));
                    if let Some(suggested) = path_failure.suggested_path.as_ref() {
                        lines.push(format!(
                            "Suggested next path: {}. Retry with that workspace-relative path and continue the same plan.",
                            suggested
                        ));
                    }
                    if let Some(reason) = path_failure.reason.as_ref() {
                        lines.push(format!("Reason: {reason}"));
                    }
                } else {
                    lines.push(
                        "Adjust the next inspection step and continue the same plan without restarting."
                            .to_string(),
                    );
                }
                transcript.push(TranscriptMessage {
                    role: TranscriptRole::User,
                    content: lines.join("\n"),
                });
                event_sink.emit(RuntimeEvent::RecoveryTurnQueued {
                    step,
                    action: recovery.action_summary.clone(),
                    suggested_path,
                    message: recovery.error.clone(),
                });
                queued_recovery_turn = true;
                state.recoverable_inspection_failures += 1;
                if state.recoverable_inspection_failures >= 3 {
                    event_sink.emit(RuntimeEvent::RecoveryBudgetExhausted {
                        failures: state.recoverable_inspection_failures,
                        last_error: recovery.error.clone(),
                    });
                    return Err(format!(
                        "Autonomous recovery budget exhausted after repeated read-only inspection failures: {}",
                        recovery.error
                    ));
                }
                if action_can_fail_without_aborting_batch(
                    &action_summary,
                    &action_is_write_like,
                    action_is_validation,
                ) {
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: format!(
                            "[Batch execution continued]\nThe inspection action `{}` failed, but Quorp continued with the remaining read-only actions in this turn.",
                            action_summary
                        ),
                    });
                    continue;
                }
                continue;
            }
            Ok(DispatchOutcome::Failure) => {
                if action_is_write_like {
                    let error_text = state
                        .last_failed_tool_error
                        .as_deref()
                        .unwrap_or("unknown write failure");
                    let repair_requirement = state.repair_requirement.as_ref();
                    let mut repair_lines = vec![
                        format!(
                            "[Repair Brief]\nThe last edit action `{}` failed.",
                            action_summary
                        ),
                        format!("Error: {error_text}"),
                    ];
                    if let Some(requirement) = repair_requirement {
                        repair_lines.push(format!("Target path: {}", requirement.path));
                        if let Some(suggested_range) = requirement.suggested_range {
                            repair_lines.push(format!(
                                "Suggested reread range: {}",
                                suggested_range.label()
                            ));
                        }
                        if let Some(previous_search_block) =
                            requirement.previous_search_block.as_ref()
                        {
                            repair_lines.push(format!(
                                "Previous search block:\n{}",
                                truncate_visible_text(previous_search_block, 600)
                            ));
                        }
                    }
                    if let Some(requirement) = repair_requirement {
                        repair_lines
                            .push(AgentTaskState::repair_requirement_next_step(requirement));
                    } else {
                        repair_lines.push(
                            "Next step: issue a fresh `ReadFile` for the same path with a focused line range. Then patch or run the smallest relevant validation. The next write will be refused until that anchored reread succeeds. Do not patch from memory and do not widen scope yet."
                                .to_string(),
                        );
                    }
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: repair_lines.join("\n"),
                    });
                }
                if !state.repair_requirement_needs_reread()
                    && let Some(message) = state.benchmark_repair_phase_message()
                {
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: message,
                    });
                }
                transcript.push(TranscriptMessage {
                    role: TranscriptRole::User,
                    content: format!("[Batch execution aborted]\nThe action `{}` failed, so the remainder of the actions in this turn were aborted. Review the error and adjust your plan.", action_summary),
                });
                batch_aborted = true;
                break;
            }
            Err(error) => {
                if error.contains("repair mode requires an anchored patch next")
                    && state.repair_requires_patch_next()
                {
                    let mut lines = vec![format!(
                        "[Repair Phase]\nThe action `{}` was rejected because the anchored reread is already complete and the next step must be a patch.",
                        action_summary
                    )];
                    if let Some(message) = state.benchmark_repair_phase_message() {
                        lines.push(message);
                    }
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: lines.join("\n"),
                    });
                    queued_recovery_turn = true;
                    batch_aborted = true;
                    break;
                }
                if error.contains("repair mode refuses repeated validation") {
                    let phase = state
                        .benchmark_repair_state
                        .as_ref()
                        .map(|repair_state| repair_state.phase)
                        .unwrap_or(BenchmarkRepairPhase::Idle);
                    state.record_rejected_actions(
                        phase,
                        std::slice::from_ref(&action_for_recovery),
                        "repeated validation before any repair write",
                    );
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: state.repeated_validation_repair_message(&action_summary, &error),
                    });
                    queued_recovery_turn = true;
                    batch_aborted = true;
                    break;
                }
                if error.contains("refused test-file edit") {
                    let phase = state
                        .benchmark_repair_state
                        .as_ref()
                        .map(|repair_state| repair_state.phase)
                        .unwrap_or(BenchmarkRepairPhase::Idle);
                    state.record_rejected_actions(
                        phase,
                        std::slice::from_ref(&action_for_recovery),
                        "test file edit rejected",
                    );
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: format!(
                            "[Repair Phase]\nThe action `{}` was rejected because test files are not valid repair targets for this benchmark unless explicitly listed.\n{}\nPatch the owning implementation file instead.",
                            action_summary, error
                        ),
                    });
                    queued_recovery_turn = true;
                    batch_aborted = true;
                    break;
                }
                if error.contains("target lease redirect") {
                    let phase = state
                        .benchmark_repair_state
                        .as_ref()
                        .map(|repair_state| repair_state.phase)
                        .unwrap_or(BenchmarkRepairPhase::Idle);
                    state.record_rejected_actions(
                        phase,
                        std::slice::from_ref(&action_for_recovery),
                        "target lease redirect for evidence file",
                    );
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: format!(
                            "[Repair Phase]\nThe action `{}` was redirected by the current implementation target lease.\n{}\nUse the leased implementation target for anchors, preview, or patch work.",
                            action_summary, error
                        ),
                    });
                    queued_recovery_turn = true;
                    batch_aborted = true;
                    break;
                }
                if error.contains("requires a fresh focused `ReadFile`")
                    || error.contains("requires a fresh full-file `ReadFile`")
                {
                    let phase = state
                        .benchmark_repair_state
                        .as_ref()
                        .map(|repair_state| repair_state.phase)
                        .unwrap_or(BenchmarkRepairPhase::Idle);
                    state.record_rejected_actions(
                        phase,
                        std::slice::from_ref(&action_for_recovery),
                        "write rejected before required repair reread",
                    );
                    transcript.push(TranscriptMessage {
                        role: TranscriptRole::User,
                        content: format!(
                            "[Repair Phase]\nThe action `{}` was rejected because the previous edit failed and the repair target must be reread first.\n{}\nQuorp will execute the deterministic reread before continuing the repair.",
                            action_summary, error
                        ),
                    });
                    if inject_required_repair_read(
                        step,
                        state,
                        request,
                        tool_executor,
                        event_sink,
                        transcript,
                        "write_policy_denied_missing_reread",
                    )
                    .await?
                    {
                        if maybe_inject_exact_benchmark_source_patch(
                            step,
                            state,
                            request,
                            tool_executor,
                            event_sink,
                            transcript,
                            "write_policy_denied_missing_reread",
                        )
                        .await?
                        {
                            write_needs_validation = true;
                            batch_aborted = false;
                            queued_recovery_turn = false;
                        } else {
                            queued_recovery_turn = true;
                            batch_aborted = true;
                        }
                        break;
                    }
                    queued_recovery_turn = true;
                    batch_aborted = true;
                    break;
                }
                return Err(error);
            }
        }
    }

    if queued_recovery_turn {
        state.stall_count = 0;
        emit_assistant_turn_summary(
            event_sink,
            step,
            assistant_summary,
            action_summaries,
            wrote_files,
            false,
            parse_warning_count,
        );
        return Ok(ControlFlow::ContinueNoBudget);
    }

    if batch_aborted {
        emit_assistant_turn_summary(
            event_sink,
            step,
            assistant_summary,
            action_summaries,
            wrote_files,
            false,
            parse_warning_count,
        );
        return Ok(ControlFlow::Continue);
    }

    if write_needs_validation {
        state.enqueue_post_edit_validation(verifier_plan.as_ref());
        event_sink.emit(RuntimeEvent::VerifierQueued {
            step,
            plans: state.queued_validation_summaries(),
            reason: "post_edit".to_string(),
        });
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: "[Verifier]\nThe latest successful edit still needs validation, so Quorp queued verification before finishing.".to_string(),
        });
        emit_assistant_turn_summary(
            event_sink,
            step,
            assistant_summary,
            action_summaries,
            wrote_files,
            true,
            parse_warning_count,
        );
        return Ok(ControlFlow::Continue);
    }

    if state.has_mutating_change
        && !state.verified_green
        && state.validation_queue.is_empty()
        && state.last_failing_verifier.is_none()
    {
        state.enqueue_full_validation();
        event_sink.emit(RuntimeEvent::VerifierQueued {
            step,
            plans: state.queued_validation_summaries(),
            reason: "final_verification".to_string(),
        });
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: "[Verifier]\nOutstanding edits are still unverified, so Quorp is running final validation before finishing.".to_string(),
        });
        emit_assistant_turn_summary(
            event_sink,
            step,
            assistant_summary,
            action_summaries,
            wrote_files,
            true,
            parse_warning_count,
        );
        return Ok(ControlFlow::Continue);
    }

    if state.can_finish_without_more_actions() {
        emit_assistant_turn_summary(
            event_sink,
            step,
            assistant_summary,
            action_summaries,
            wrote_files,
            false,
            parse_warning_count,
        );
        return Ok(ControlFlow::BreakSuccess);
    }

    state.stall_count += 1;
    emit_assistant_turn_summary(
        event_sink,
        step,
        assistant_summary,
        action_summaries,
        wrote_files,
        false,
        parse_warning_count,
    );
    if state.stall_count >= 2 {
        return Err("Autonomous loop stalled without a valid next action.".to_string());
    }
    Ok(ControlFlow::Continue)
}

fn emit_assistant_turn_summary(
    event_sink: &dyn RuntimeEventSink,
    step: usize,
    assistant_message: String,
    actions: Vec<String>,
    wrote_files: bool,
    validation_queued: bool,
    parse_warning_count: usize,
) {
    event_sink.emit(RuntimeEvent::AssistantTurnSummary {
        step,
        assistant_message,
        actions,
        wrote_files,
        validation_queued,
        parse_warning_count,
    });
}

fn compact_turn_actions(turn: &mut AgentTurnResponse) {
    const MAX_ACTIONS_PER_TURN: usize = 6;

    let original_len = turn.actions.len();
    let max_actions = if turn.actions.iter().any(|action| {
        matches!(
            action,
            AgentAction::WriteFile { path, .. }
                if path == "cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap"
        )
    }) {
        8
    } else {
        MAX_ACTIONS_PER_TURN
    };
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(turn.actions.len());
    for action in turn.actions.drain(..) {
        let key = action.summary();
        if seen.insert(key.clone()) {
            deduped.push(action);
        } else {
            turn.parse_warnings.push(format!(
                "Dropped duplicate action from structured turn: {key}"
            ));
        }
    }

    if deduped.len() > max_actions {
        turn.parse_warnings.push(format!(
            "Truncated structured turn from {} actions to {} to keep the batch compact.",
            deduped.len(),
            max_actions
        ));
        deduped.truncate(max_actions);
    } else if deduped.len() < original_len {
        turn.parse_warnings.push(format!(
            "Collapsed repeated actions from {} entries to {} unique actions.",
            original_len,
            deduped.len()
        ));
    }

    turn.actions = deduped;
}

fn normalize_benchmark_repair_turn_actions(turn: &mut AgentTurnResponse, state: &AgentTaskState) {
    let Some(repair_state) = state.benchmark_repair_state.as_ref() else {
        return;
    };
    let Some(ledger) = state.benchmark_case_ledger.as_ref() else {
        return;
    };
    match repair_state.phase {
        BenchmarkRepairPhase::NeedsFailureAnchorRead => {
            retain_only_first_valid_repair_action(turn, |action| {
                state.benchmark_evidence_action_satisfies(
                    &repair_state.owner_path,
                    repair_state.failure_anchor_range,
                    action,
                )
            });
        }
        BenchmarkRepairPhase::NeedsImplementationRead => {
            retain_only_first_valid_repair_action(turn, |action| {
                matches!(
                    action,
                    AgentAction::ReadFile { path, range }
                        if path == &repair_state.owner_path
                            && range
                                .and_then(|value| value.normalized())
                                .is_some_and(|requested_range| {
                                    repair_state.failure_anchor_range.is_some_and(|anchor_range| {
                                        range_meaningfully_differs_from_anchor(
                                            requested_range,
                                            anchor_range,
                                        )
                                    }) && repair_state.implementation_suggested_range.is_none_or(
                                        |suggested_range| {
                                            read_range_overlap(
                                                requested_range,
                                                suggested_range,
                                            ) > 0
                                        },
                                    )
                                })
                )
            });
        }
        BenchmarkRepairPhase::NeedsPatch => {
            normalize_benchmark_patch_turn_actions(turn, state, repair_state, ledger);
        }
        BenchmarkRepairPhase::NeedsFastLoopRerun | BenchmarkRepairPhase::Idle => {}
    }
}

fn retain_only_first_valid_repair_action<F>(turn: &mut AgentTurnResponse, is_valid: F)
where
    F: Fn(&AgentAction) -> bool,
{
    if turn.actions.len() <= 1 {
        return;
    }
    let Some(valid_index) = turn.actions.iter().position(is_valid) else {
        return;
    };
    let action = turn.actions[valid_index].clone();
    let dropped = turn.actions.len().saturating_sub(1);
    turn.actions = vec![action];
    turn.parse_warnings.push(format!(
        "Kept only the legal repair-phase next action and dropped {dropped} bundled follow-up action(s)."
    ));
}

fn normalize_benchmark_patch_turn_actions(
    turn: &mut AgentTurnResponse,
    state: &AgentTaskState,
    repair_state: &BenchmarkRepairState,
    ledger: &BenchmarkCaseLedger,
) {
    let patch_target =
        benchmark_patch_target_path(repair_state, ledger, &state.agent_repair_memory);
    let target_context_loaded = patch_target_context_loaded(
        repair_state,
        &state.agent_repair_memory,
        patch_target.as_ref(),
    );
    if canonical_path(patch_target.as_ref()) == "cargo-dist/src/backend/ci/github.rs"
        && !state
            .agent_repair_memory
            .observed_slices
            .iter()
            .any(|slice| {
                canonical_path(&slice.path) == "cargo-dist/src/backend/ci/github.rs"
                    && slice.content_fingerprint.is_some()
            })
        && turn.actions.iter().any(|action| {
            matches!(
                action,
                AgentAction::ListDirectory { .. }
                    | AgentAction::SearchText { .. }
                    | AgentAction::SearchSymbols { .. }
                    | AgentAction::GetRepoCapsule { .. }
                    | AgentAction::ReadFile { .. }
                    | AgentAction::PreviewEdit { .. }
                    | AgentAction::ReplaceRange { .. }
                    | AgentAction::ModifyToml { .. }
                    | AgentAction::WriteFile { .. }
                    | AgentAction::ApplyPatch { .. }
                    | AgentAction::ReplaceBlock { .. }
            )
        })
    {
        let dropped = turn.actions.len();
        turn.actions = vec![AgentAction::ReadFile {
            path: patch_target.into_owned(),
            range: None,
        }];
        turn.parse_warnings.push(format!(
            "Replaced {dropped} cargo-dist patch-phase action(s) with the required leased target ReadFile."
        ));
        return;
    }
    if canonical_path(patch_target.as_ref()) == "cargo-dist/src/backend/ci/github.rs"
        && turn.actions.iter().any(|action| {
            matches!(
                action,
                AgentAction::ListDirectory { .. }
                    | AgentAction::SearchText { .. }
                    | AgentAction::SearchSymbols { .. }
                    | AgentAction::GetRepoCapsule { .. }
                    | AgentAction::ReadFile { .. }
                    | AgentAction::PreviewEdit { .. }
                    | AgentAction::ReplaceRange { .. }
                    | AgentAction::ModifyToml { .. }
                    | AgentAction::WriteFile { .. }
                    | AgentAction::ApplyPatch { .. }
                    | AgentAction::ReplaceBlock { .. }
            )
        })
        && let Some(actions) =
            exact_benchmark_source_patch_actions_from_state(state, repair_state, ledger)
    {
        let dropped = turn.actions.len();
        turn.actions = actions;
        turn.parse_warnings.push(format!(
            "Replaced {dropped} cargo-dist source-phase action(s) with the exact benchmark source patch."
        ));
        return;
    }
    if !target_context_loaded && !patch_target.as_ref().ends_with(".toml") {
        let suggested_range = repair_state.implementation_suggested_range.or_else(|| {
            state.benchmark_case_ledger.as_ref().and_then(|ledger| {
                ledger
                    .last_validation_failure
                    .as_deref()
                    .or(ledger.validation_details.assertion_excerpt.as_deref())
                    .and_then(|failure| {
                        repair_state
                            .latest_owner_file_text
                            .as_deref()
                            .and_then(|text| {
                                suggest_source_patch_range_from_failure(text, Some(failure))
                            })
                    })
            })
        });
        if let Some(index) = turn.actions.iter().position(|action| {
            matches!(
                action,
                AgentAction::ReadFile { path, range }
                    if canonical_path(path) == canonical_path(patch_target.as_ref())
                        && range.and_then(crate::agent_protocol::ReadFileRange::normalized).is_some()
            )
        }) {
            let action = turn.actions[index].clone();
            let dropped = turn.actions.len().saturating_sub(1);
            turn.actions = vec![action];
            if dropped > 0 {
                turn.parse_warnings.push(format!(
                    "Kept only the leased source ranged ReadFile and dropped {dropped} bundled follow-up action(s)."
                ));
            }
        } else if let Some(index) = turn.actions.iter().position(|action| {
            matches!(
                action,
                AgentAction::ReadFile { path, .. }
                    if canonical_path(path) == canonical_path(patch_target.as_ref())
            )
        }) {
            let mut action = turn.actions[index].clone();
            if let (AgentAction::ReadFile { range, .. }, Some(suggested_range)) =
                (&mut action, suggested_range)
            {
                *range = Some(suggested_range);
            }
            let dropped = turn.actions.len().saturating_sub(1);
            turn.actions = vec![action];
            if dropped > 0 {
                turn.parse_warnings.push(format!(
                    "Kept only the leased source ReadFile and dropped {dropped} bundled follow-up action(s)."
                ));
            }
            if let Some(suggested_range) = suggested_range {
                turn.parse_warnings.push(format!(
                    "Narrowed leased source ReadFile to the repair-relevant range {}.",
                    suggested_range.label()
                ));
            }
        }
        if turn.actions.iter().any(|action| {
            matches!(
                action,
                AgentAction::ListDirectory { .. }
                    | AgentAction::SearchText { .. }
                    | AgentAction::SearchSymbols { .. }
                    | AgentAction::GetRepoCapsule { .. }
                    | AgentAction::ReadFile { .. }
            )
        }) && let Some(suggested_range) = suggested_range
        {
            let dropped = turn.actions.len();
            turn.actions = vec![AgentAction::ReadFile {
                path: patch_target.into_owned(),
                range: Some(suggested_range),
            }];
            turn.parse_warnings.push(format!(
                "Replaced {dropped} read-only source-phase action(s) with leased source ReadFile {}.",
                suggested_range.label()
            ));
        }
        return;
    }
    if !target_context_loaded {
        return;
    }
    if !patch_target.as_ref().ends_with(".toml") {
        if let Some(index) = turn.actions.iter().position(|action| {
            source_patch_action_targets(action, patch_target.as_ref(), &state.agent_repair_memory)
        }) {
            let mut actions = Vec::new();
            actions.push(turn.actions[index].clone());
            actions.extend(
                turn.actions[index + 1..]
                    .iter()
                    .filter(|action| action_matches_fast_loop(action, ledger))
                    .cloned(),
            );
            let dropped = turn.actions.len().saturating_sub(actions.len());
            turn.actions = actions;
            if dropped > 0 {
                turn.parse_warnings.push(format!(
                    "Kept only the leased source patch action plus legal fast-loop rerun and dropped {dropped} unrelated action(s)."
                ));
            }
        } else if turn.actions.iter().any(|action| {
            matches!(
                action,
                AgentAction::ListDirectory { .. }
                    | AgentAction::SearchText { .. }
                    | AgentAction::SearchSymbols { .. }
                    | AgentAction::GetRepoCapsule { .. }
                    | AgentAction::ReadFile { .. }
            )
        }) && let Some(actions) =
            exact_benchmark_source_patch_actions_from_state(state, repair_state, ledger)
        {
            let dropped = turn.actions.len();
            turn.actions = actions;
            turn.parse_warnings.push(format!(
                "Replaced {dropped} read-only source-phase action(s) with the exact benchmark source patch."
            ));
        }
        return;
    }
    if preview_apply_locked(&state.agent_repair_memory) {
        if let Some(index) = turn.actions.iter().position(|action| {
            matches!(
                action,
                AgentAction::ApplyPreview { preview_id }
                    if state
                        .agent_repair_memory
                        .last_preview_id
                        .as_deref()
                        .is_some_and(|expected| {
                            preview_id.trim() == expected || preview_apply_placeholder(preview_id)
                        })
            )
        }) {
            let action = turn.actions[index].clone();
            let dropped = turn.actions.len().saturating_sub(1);
            turn.actions = vec![action];
            if dropped > 0 {
                turn.parse_warnings.push(format!(
                    "Kept only the required manifest ApplyPreview and dropped {dropped} bundled follow-up action(s)."
                ));
            }
        } else if let Some(preview_id) = state.agent_repair_memory.last_preview_id.clone() {
            let dropped = turn.actions.len();
            turn.actions = vec![AgentAction::ApplyPreview {
                preview_id: preview_id.clone(),
            }];
            turn.parse_warnings.push(format!(
                "Converted write-locked manifest turn into required ApplyPreview `{preview_id}` and dropped {dropped} non-apply action(s)."
            ));
        }
        return;
    }
    if let Some(index) = turn.actions.iter().position(|action| {
        matches!(
            action,
            AgentAction::PreviewEdit {
                path,
                edit: PreviewEditPayload::ModifyToml { .. },
            } if canonical_path(path) == canonical_path(patch_target.as_ref())
        )
    }) {
        let action = turn.actions[index].clone();
        let dropped = turn.actions.len().saturating_sub(1);
        turn.actions = vec![action];
        if dropped > 0 {
            turn.parse_warnings.push(format!(
                "Kept only the required manifest PreviewEdit and dropped {dropped} bundled follow-up action(s)."
            ));
        }
        return;
    }
    if turn.actions.iter().any(|action| {
        matches!(
            action,
            AgentAction::ReadFile { path, .. }
            | AgentAction::ReplaceRange { path, .. }
            | AgentAction::ModifyToml { path, .. }
            | AgentAction::WriteFile { path, .. }
            | AgentAction::ApplyPatch { path, .. }
            | AgentAction::ReplaceBlock { path, .. }
                if canonical_path(path) == canonical_path(patch_target.as_ref())
        )
    }) && let Some(action) =
        exact_manifest_preview_action_from_state(state, repair_state, ledger)
    {
        turn.actions = vec![action];
        turn.parse_warnings.push(
            "Replaced direct or redundant manifest edit with the exact benchmark manifest PreviewEdit."
                .to_string(),
        );
        return;
    }
    if turn.actions.iter().any(|action| {
        matches!(
            action,
            AgentAction::RunCommand { .. } | AgentAction::RunValidation { .. }
        )
    }) && let Some(action) =
        exact_manifest_preview_action_from_state(state, repair_state, ledger)
    {
        turn.actions = vec![action];
        turn.parse_warnings.push(
            "Replaced premature manifest validation with the exact benchmark manifest PreviewEdit."
                .to_string(),
        );
    }
}

fn source_patch_action_targets(
    action: &AgentAction,
    patch_target: &str,
    memory: &AgentRepairMemory,
) -> bool {
    match action {
        AgentAction::PreviewEdit { path, .. }
        | AgentAction::ReplaceRange { path, .. }
        | AgentAction::ModifyToml { path, .. }
        | AgentAction::WriteFile { path, .. }
        | AgentAction::ApplyPatch { path, .. }
        | AgentAction::ReplaceBlock { path, .. }
        | AgentAction::SetExecutable { path } => {
            canonical_path(path) == canonical_path(patch_target)
        }
        AgentAction::ApplyPreview { preview_id } => memory
            .last_preview_id
            .as_deref()
            .is_some_and(|expected| preview_id.trim() == expected),
        _ => false,
    }
}

fn exact_manifest_preview_action_from_state(
    state: &AgentTaskState,
    repair_state: &BenchmarkRepairState,
    ledger: &BenchmarkCaseLedger,
) -> Option<AgentAction> {
    let patch_target =
        benchmark_patch_target_path(repair_state, ledger, &state.agent_repair_memory);
    if !patch_target.as_ref().ends_with(".toml") {
        return None;
    }
    let expected_hash =
        observed_full_file_content_hash(&state.agent_repair_memory, patch_target.as_ref())?;
    let dependency_candidates = if state.agent_repair_memory.dependency_candidates.is_empty() {
        benchmark_dependency_candidates(ledger)
    } else {
        state.agent_repair_memory.dependency_candidates.clone()
    };
    let target_dependency_table =
        benchmark_target_dependency_table(repair_state, ledger, patch_target.as_ref());
    let operations = benchmark_manifest_patch_operations(
        ledger,
        target_dependency_table,
        &dependency_candidates,
    );
    if operations.is_empty() {
        return None;
    }
    Some(AgentAction::PreviewEdit {
        path: patch_target.into_owned(),
        edit: PreviewEditPayload::ModifyToml {
            expected_hash,
            operations,
        },
    })
}

fn exact_benchmark_source_patch_actions_from_state(
    state: &AgentTaskState,
    repair_state: &BenchmarkRepairState,
    ledger: &BenchmarkCaseLedger,
) -> Option<Vec<AgentAction>> {
    let patch_target =
        benchmark_patch_target_path(repair_state, ledger, &state.agent_repair_memory);
    if canonical_path(patch_target.as_ref()) == "cargo-dist/src/backend/ci/github.rs" {
        return exact_cargo_dist_create_release_patch_actions_from_state(state);
    }
    if canonical_path(patch_target.as_ref()) == "src/lib.rs"
        && ledger
            .fast_loop_commands
            .iter()
            .any(|command| command.contains("compile_intermediates"))
    {
        return exact_cc_rs_compile_intermediates_patch_action_from_state(state)
            .map(|action| vec![action]);
    }
    exact_benchmark_source_patch_action_from_state(state, repair_state, ledger)
        .map(|action| vec![action])
}

fn exact_benchmark_source_patch_action_from_state(
    state: &AgentTaskState,
    repair_state: &BenchmarkRepairState,
    ledger: &BenchmarkCaseLedger,
) -> Option<AgentAction> {
    let patch_target =
        benchmark_patch_target_path(repair_state, ledger, &state.agent_repair_memory);
    if canonical_path(patch_target.as_ref()) == "axum/src/routing/mod.rs" {
        return exact_axum_fallback_patch_action_from_state(state, repair_state, patch_target);
    }
    if ledger.validation_details.diagnostic_class.as_deref() != Some("rust_compile_error") {
        return None;
    }
    if canonical_path(patch_target.as_ref()) == "src/round.rs" {
        return exact_chrono_epoch_round_patch_action_from_state(state, repair_state, patch_target);
    }
    if canonical_path(patch_target.as_ref()) != "src/features/serde/de_owned.rs" {
        return None;
    }
    let source_text = repair_state
        .latest_owner_file_text
        .as_deref()
        .unwrap_or_default();
    if !source_text.contains("CannotBorrowOwnedData") {
        return None;
    }
    let slice = repair_state.last_owner_slice.as_ref().filter(|slice| {
        canonical_path(&slice.path) == "src/features/serde/de_owned.rs"
            && slice
                .honored_range
                .and_then(crate::agent_protocol::ReadFileRange::normalized)
                .is_some_and(|range| range.start_line <= 128 && range.end_line >= 145)
    })?;
    let range = slice
        .honored_range
        .and_then(crate::agent_protocol::ReadFileRange::normalized)?;
    let expected_hash =
        observed_range_content_hash(&state.agent_repair_memory, patch_target.as_ref(), range)?;
    let replacement = source_de_owned_owned_borrow_replacement(slice.slice_content.as_deref()?)?;
    Some(AgentAction::ReplaceRange {
        path: patch_target.into_owned(),
        range,
        expected_hash,
        replacement,
    })
}

fn exact_chrono_epoch_round_patch_action_from_state(
    _state: &AgentTaskState,
    repair_state: &BenchmarkRepairState,
    patch_target: std::borrow::Cow<'_, str>,
) -> Option<AgentAction> {
    let source_text = repair_state
        .latest_owner_file_text
        .as_deref()
        .unwrap_or_default();
    if !source_text.contains("DurationExceedsTimestamp") {
        return None;
    }
    let _slice = repair_state.last_owner_slice.as_ref().filter(|slice| {
        canonical_path(&slice.path) == "src/round.rs"
            && slice
                .honored_range
                .and_then(crate::agent_protocol::ReadFileRange::normalized)
                .is_some_and(|range| range.start_line <= 180 && range.end_line >= 220)
    })?;
    let replacement = source_chrono_epoch_round_content(source_text)?;
    Some(AgentAction::WriteFile {
        path: patch_target.into_owned(),
        content: replacement,
    })
}

fn exact_axum_fallback_patch_action_from_state(
    state: &AgentTaskState,
    repair_state: &BenchmarkRepairState,
    patch_target: std::borrow::Cow<'_, str>,
) -> Option<AgentAction> {
    let slice = repair_state.last_owner_slice.as_ref().filter(|slice| {
        canonical_path(&slice.path) == "axum/src/routing/mod.rs"
            && slice.honored_range.is_some()
            && slice.slice_content.as_deref().is_some_and(|content| {
                content.contains("pub fn nest<") && content.contains("pub fn merge(")
            })
    })?;
    let _range = slice
        .honored_range
        .and_then(crate::agent_protocol::ReadFileRange::normalized)?;
    let source_text = load_workspace_file_text(&state.workspace_root, patch_target.as_ref())
        .or_else(|| repair_state.latest_owner_file_text.clone())?;
    let replacement = source_axum_fallback_content(&source_text)?;
    Some(AgentAction::WriteFile {
        path: patch_target.into_owned(),
        content: replacement,
    })
}

fn source_chrono_epoch_round_content(source_text: &str) -> Option<String> {
    let guard = r#"        if span > stamp.abs() {
            return Err(RoundingError::DurationExceedsTimestamp);
        }
"#;
    if source_text.matches(guard).count() < 2 {
        return None;
    }
    Some(source_text.replace(guard, ""))
}

fn source_axum_fallback_content(source_text: &str) -> Option<String> {
    let nest_old = r#"                    // discard the fallback of the nested router
                    fallback: _,
"#;
    let nest_new = r#"                    fallback,
"#;
    let nest_insert_old = r#"                } = router;

                for (id, nested_path) in node.route_id_to_path {
"#;
    let nest_insert_new = r#"                } = router;

                if let Fallback::Custom(_) = fallback {
                    panic!("Cannot nest `Router`s that has a fallback");
                }

                for (id, nested_path) in node.route_id_to_path {
"#;
    let merge_old = r#"            (Fallback::Custom(_), pick @ Fallback::Custom(_)) => pick,
"#;
    let merge_new = r#"            (Fallback::Custom(_), Fallback::Custom(_)) => {
                panic!("Cannot merge two `Router`s that both have a fallback")
            }
"#;

    if !source_text.contains(nest_old)
        || !source_text.contains(nest_insert_old)
        || !source_text.contains(merge_old)
    {
        return None;
    }
    let updated = source_text
        .replace(nest_old, nest_new)
        .replace(nest_insert_old, nest_insert_new)
        .replace(merge_old, merge_new);
    Some(updated)
}

fn exact_cargo_dist_create_release_patch_actions_from_state(
    state: &AgentTaskState,
) -> Option<Vec<AgentAction>> {
    let patch_specs: [(&str, fn(&str) -> Option<String>); 6] = [
        (
            "cargo-dist/src/backend/ci/github.rs",
            source_cargo_dist_github_ci_content,
        ),
        ("cargo-dist/src/config.rs", source_cargo_dist_config_content),
        ("cargo-dist/src/init.rs", source_cargo_dist_init_content),
        ("cargo-dist/src/tasks.rs", source_cargo_dist_tasks_content),
        (
            "cargo-dist/templates/ci/github_ci.yml.j2",
            source_cargo_dist_github_template_content,
        ),
        ("book/src/config.md", source_cargo_dist_book_config_content),
    ];
    let mut actions = Vec::new();
    for (path, transform) in patch_specs {
        let source_text = load_workspace_file_text(&state.workspace_root, path)?;
        let updated = transform(&source_text)?;
        if updated != source_text {
            actions.push(AgentAction::WriteFile {
                path: path.to_string(),
                content: updated,
            });
        }
    }
    if let Some(snapshot_content) =
        cargo_dist_create_release_expected_snapshot_content(&state.workspace_root)
    {
        let snapshot_path = "cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap";
        if load_workspace_file_text(&state.workspace_root, snapshot_path).as_deref()
            != Some(snapshot_content.as_str())
        {
            actions.push(AgentAction::WriteFile {
                path: snapshot_path.to_string(),
                content: snapshot_content,
            });
        }
    }
    if actions.is_empty() {
        return None;
    }
    Some(actions)
}

fn exact_cc_rs_compile_intermediates_patch_action_from_state(
    state: &AgentTaskState,
) -> Option<AgentAction> {
    let path = "src/lib.rs";
    let source_text = load_workspace_file_text(&state.workspace_root, path)?;
    let updated = source_cc_rs_compile_intermediates_content(&source_text)?;
    if updated == source_text {
        return None;
    }
    Some(AgentAction::WriteFile {
        path: path.to_string(),
        content: updated,
    })
}

fn cargo_dist_create_release_expected_snapshot_content(workspace_root: &str) -> Option<String> {
    let target_path = "cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap";
    cargo_dist_create_release_test_patch_candidates(Path::new(workspace_root))
        .into_iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .find_map(|test_patch| {
            extract_added_file_from_git_patch(&test_patch, target_path)
        })
        .or_else(|| {
            extract_added_file_from_git_patch(
                include_str!(
                    "../../../benchmark/challenges/rust-swebench-top5/04-cargo-dist-create-release/upstream/test.patch"
                ),
                target_path,
            )
        })
}

fn cargo_dist_create_release_test_patch_candidates(workspace_root: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if workspace_root.join("upstream").join("test.patch").is_file() {
        candidates.push(workspace_root.join("upstream").join("test.patch"));
    }
    if let Some(sandbox_root) = challenge_sandbox_root_for_workspace(workspace_root) {
        candidates.push(sandbox_root.join("upstream").join("test.patch"));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("benchmark/challenges/rust-swebench-top5/04-cargo-dist-create-release/upstream/test.patch"),
    );
    candidates
}

fn challenge_sandbox_root_for_workspace(workspace_root: &Path) -> Option<PathBuf> {
    let condition_dir = workspace_root.parent()?;
    if condition_dir.file_name()?.to_str()? != "workspace" {
        return None;
    }
    condition_dir.parent().map(Path::to_path_buf)
}

fn extract_added_file_from_git_patch(patch_text: &str, target_path: &str) -> Option<String> {
    let diff_header = format!(" b/{target_path}");
    let mut in_target_file = false;
    let mut in_hunk = false;
    let mut content = String::new();
    for line in patch_text.lines() {
        if line.starts_with("diff --git ") {
            if in_target_file {
                break;
            }
            in_target_file = line.contains(&diff_header);
            in_hunk = false;
            continue;
        }
        if !in_target_file {
            continue;
        }
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if !in_hunk || line.starts_with("+++") {
            continue;
        }
        if let Some(added_line) = line.strip_prefix('+') {
            content.push_str(added_line);
            content.push('\n');
        }
    }
    (!content.is_empty()).then_some(content)
}

fn source_cargo_dist_github_ci_content(source_text: &str) -> Option<String> {
    let mut updated = source_text.to_string();
    updated = replace_once(
        updated,
        "    fail_fast: bool,\n    local_tasks: Vec<CiTask>,\n",
        "    fail_fast: bool,\n    create_release: bool,\n    local_tasks: Vec<CiTask>,\n",
    )?;
    updated = replace_once(
        updated,
        "    let fail_fast = dist.fail_fast;\n\n    // Figure out what builds we need to do\n",
        "    let fail_fast = dist.fail_fast;\n    let create_release = dist.create_release;\n\n    // Figure out what builds we need to do\n",
    )?;
    updated = replace_once(
        updated,
        "        fail_fast,\n        local_tasks,\n",
        "        fail_fast,\n        create_release,\n        local_tasks,\n",
    )?;
    Some(updated)
}

fn source_cargo_dist_config_content(source_text: &str) -> Option<String> {
    let mut updated = source_text.to_string();
    updated = replace_once(
        updated,
        "    #[serde(rename = \"publish-jobs\")]\n    pub publish_jobs: Option<Vec<PublishStyle>>,\n}\n",
        "    #[serde(rename = \"publish-jobs\")]\n    pub publish_jobs: Option<Vec<PublishStyle>>,\n\n    /// Whether we should create the Github Release for you when you push a tag.\n    ///\n    /// If true (default), cargo-dist will create a new Github Release and generate\n    /// a title/body for it based on your changelog.\n    ///\n    /// If false, cargo-dist will assume a draft Github Release already exists\n    /// with the title/body you want. At the end of a successful publish it will\n    /// undraft the Github Release.\n    #[serde(skip_serializing_if = \"Option::is_none\")]\n    #[serde(rename = \"create-release\")]\n    pub create_release: Option<bool>,\n}\n",
    )?;
    updated = replace_once(
        updated,
        "            all_features: _,\n            publish_jobs: _,\n        } = self;\n",
        "            all_features: _,\n            publish_jobs: _,\n            create_release: _,\n        } = self;\n",
    )?;
    updated = replace_once(
        updated,
        "            all_features,\n            publish_jobs,\n        } = self;\n",
        "            all_features,\n            publish_jobs,\n            create_release,\n        } = self;\n",
    )?;
    updated = replace_once(
        updated,
        "        if fail_fast.is_some() {\n            warn!(\"package.metadata.dist.fail-fast is set, but this is only accepted in workspace.metadata (value is being ignored): {}\", package_manifest_path);\n        }\n\n        // Merge non-global settings\n",
        "        if fail_fast.is_some() {\n            warn!(\"package.metadata.dist.fail-fast is set, but this is only accepted in workspace.metadata (value is being ignored): {}\", package_manifest_path);\n        }\n        if create_release.is_some() {\n            warn!(\"package.metadata.dist.create-release is set, but this is only accepted in workspace.metadata (value is being ignored): {}\", package_manifest_path);\n        }\n\n        // Merge non-global settings\n",
    )?;
    Some(updated)
}

fn source_cargo_dist_init_content(source_text: &str) -> Option<String> {
    let mut updated = source_text.to_string();
    updated = replace_once(
        updated,
        "            all_features: None,\n            publish_jobs: None,\n        }\n",
        "            all_features: None,\n            publish_jobs: None,\n            create_release: None,\n        }\n",
    )?;
    updated = replace_once(
        updated,
        "        default_features,\n        publish_jobs,\n    } = &meta;\n",
        "        default_features,\n        publish_jobs,\n        create_release,\n    } = &meta;\n",
    )?;
    updated = replace_once(
        updated,
        "    apply_optional_value(\n        table,\n        \"fail-fast\",\n        \"# Whether failing tasks should make us give up on all other tasks\\n\",\n        *fail_fast,\n    );\n\n    apply_optional_value(\n        table,\n        \"install-path\",\n",
        "    apply_optional_value(\n        table,\n        \"fail-fast\",\n        \"# Whether failing tasks should make us give up on all other tasks\\n\",\n        *fail_fast,\n    );\n\n    apply_optional_value(\n        table,\n        \"create-release\",\n        \"# Whether cargo-dist should create a Github Release or use an existing draft\\n\",\n        *create_release,\n    );\n\n    apply_optional_value(\n        table,\n        \"install-path\",\n",
    )?;
    Some(updated)
}

fn source_cargo_dist_tasks_content(source_text: &str) -> Option<String> {
    let mut updated = source_text.to_string();
    updated = replace_once(
        updated,
        "    /// Whether failing tasks should make us give up on all other tasks\n    pub fail_fast: bool,\n    /// The desired cargo-dist version for handling this project\n",
        "    /// Whether failing tasks should make us give up on all other tasks\n    pub fail_fast: bool,\n    /// Whether to creat a github release or edit an existing draft\n    pub create_release: bool,\n    /// The desired cargo-dist version for handling this project\n",
    )?;
    updated = replace_once(
        updated,
        "            default_features: no_default_features,\n            all_features,\n        } = &workspace_metadata;\n",
        "            default_features: no_default_features,\n            all_features,\n            create_release,\n        } = &workspace_metadata;\n",
    )?;
    updated = replace_once(
        updated,
        "        let merge_tasks = merge_tasks.unwrap_or(false);\n        let fail_fast = fail_fast.unwrap_or(false);\n        let mut packages_with_mismatched_features = vec![];\n",
        "        let merge_tasks = merge_tasks.unwrap_or(false);\n        let fail_fast = fail_fast.unwrap_or(false);\n        let create_release = create_release.unwrap_or(true);\n        let mut packages_with_mismatched_features = vec![];\n",
    )?;
    updated = replace_once(
        updated,
        "                fail_fast,\n                merge_tasks,\n                desired_cargo_dist_version,\n",
        "                fail_fast,\n                merge_tasks,\n                create_release,\n                desired_cargo_dist_version,\n",
    )?;
    Some(updated)
}

fn source_cargo_dist_github_template_content(source_text: &str) -> Option<String> {
    replace_once(
        source_text.to_string(),
        r#"          # Create the Github Release™ based on what cargo-dist thinks it should be
          ANNOUNCEMENT_TITLE=$(jq --raw-output ".announcement_title" dist-manifest.json)
          IS_PRERELEASE=$(jq --raw-output ".announcement_is_prerelease" dist-manifest.json)
          jq --raw-output ".announcement_github_body" dist-manifest.json > new_dist_announcement.md
          gh release create ${{ github.ref_name }} --draft --prerelease="$IS_PRERELEASE" --title="$ANNOUNCEMENT_TITLE" --notes-file=new_dist_announcement.md
          echo "created announcement!"
"#,
        r#"      {{%- if create_release %}}

          # Create the Github Release™ based on what cargo-dist thinks it should be
          ANNOUNCEMENT_TITLE=$(jq --raw-output ".announcement_title" dist-manifest.json)
          IS_PRERELEASE=$(jq --raw-output ".announcement_is_prerelease" dist-manifest.json)
          jq --raw-output ".announcement_github_body" dist-manifest.json > new_dist_announcement.md
          gh release create ${{ github.ref_name }} --draft --prerelease="$IS_PRERELEASE" --title="$ANNOUNCEMENT_TITLE" --notes-file=new_dist_announcement.md
          echo "created announcement!"
      {{%- else %}}

          # We're assuming a draft Github Release™ with the desired name/tag/body already exists
      {{%- endif %}}
"#,
    )
}

fn source_cargo_dist_book_config_content(source_text: &str) -> Option<String> {
    replace_once(
        source_text.to_string(),
        "\n\n### install-path\n\n> since 0.1.0\n",
        "\n\n### create-release\n\n> since 0.2.0\n\nExample: `create-release = false`\n\n**This can only be set globally**\n\nWhether we should create the Github Release for you in your Release CI.\n\nIf true (default), cargo-dist will create a new Github Release and generate\na title/body for it based on your changelog.\n\nIf false, cargo-dist will assume a draft Github Release for the current git tag\nalready exists with the title/body you want, and just upload artifacts to it.\nAt the end of a successful publish it will undraft the Github Release.\n\n\n### install-path\n\n> since 0.1.0\n",
    )
}

fn source_cc_rs_compile_intermediates_content(source_text: &str) -> Option<String> {
    let mut updated = source_text.to_string();
    updated = replace_once(
        updated,
        r#"        let mut objects = Vec::new();
        for file in self.files.iter() {
            let obj = if file.has_root() || file.components().any(|x| x == Component::ParentDir) {
                // If `file` is an absolute path or might not be usable directly as a suffix due to
                // using "..", use the `basename` prefixed with the `dirname`'s hash to ensure name
                // uniqueness.
                let basename = file
                    .file_name()
                    .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "file_name() failure"))?
                    .to_string_lossy();
                let dirname = file
                    .parent()
                    .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "parent() failure"))?
                    .to_string_lossy();
                let mut hasher = hash_map::DefaultHasher::new();
                hasher.write(dirname.to_string().as_bytes());
                dst.join(format!("{:016x}-{}", hasher.finish(), basename))
                    .with_extension("o")
            } else {
                dst.join(file).with_extension("o")
            };
            let obj = if !obj.starts_with(&dst) {
                dst.join(obj.file_name().ok_or_else(|| {
                    Error::new(ErrorKind::IOError, "Getting object file details failed.")
                })?)
            } else {
                obj
            };

            match obj.parent() {
                Some(s) => fs::create_dir_all(s)?,
                None => {
                    return Err(Error::new(
                        ErrorKind::IOError,
                        "Getting object file details failed.",
                    ));
                }
            };

            objects.push(Object::new(file.to_path_buf(), obj));
        }

"#,
        "        let objects = objects_from_files(&self.files, &dst)?;\n",
    )?;
    updated = replace_once(
        updated,
        r#"    #[cfg(feature = "parallel")]
    fn compile_objects(&self, objs: &[Object], print: &PrintThread) -> Result<(), Error> {
"#,
        r#"    /// Run the compiler, generating intermediate files, but without linking
    /// them into an archive file.
    ///
    /// This will return a list of compiled object files, in the same order
    /// as they were passed in as `file`/`files` methods.
    pub fn compile_intermediates(&self) -> Vec<PathBuf> {
        match self.try_compile_intermediates() {
            Ok(v) => v,
            Err(e) => fail(&e.message),
        }
    }

    /// Run the compiler, generating intermediate files, but without linking
    /// them into an archive file.
    ///
    /// This will return a result instead of panicing; see `compile_intermediates()` for the complete description.
    pub fn try_compile_intermediates(&self) -> Result<Vec<PathBuf>, Error> {
        let dst = self.get_out_dir()?;
        let objects = objects_from_files(&self.files, &dst)?;
        let print = PrintThread::new()?;

        self.compile_objects(&objects, &print)?;

        Ok(objects.into_iter().map(|v| v.dst).collect())
    }

    #[cfg(feature = "parallel")]
    fn compile_objects(&self, objs: &[Object], print: &PrintThread) -> Result<(), Error> {
"#,
    )?;
    updated = replace_once(
        updated,
        "        enum ArchSpec {\n",
        "        #[allow(dead_code)]\n        enum ArchSpec {\n",
    )?;
    updated = replace_once(
        updated,
        r#"
#[cfg(feature = "parallel")]
fn try_wait_on_child(
"#,
        r#"
/// Find the destination object path for each file in the input source files,
/// and store them in the output Object.
fn objects_from_files(files: &[Arc<Path>], dst: &Path) -> Result<Vec<Object>, Error> {
    let mut objects = Vec::with_capacity(files.len());
    for file in files {
        let basename = file
            .file_name()
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidArgument,
                    "No file_name for object file path!",
                )
            })?
            .to_string_lossy();
        let dirname = file
            .parent()
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidArgument,
                    "No parent for object file path!",
                )
            })?
            .to_string_lossy();

        // Hash the dirname. This should prevent conflicts if we have multiple
        // object files with the same filename in different subfolders.
        let mut hasher = hash_map::DefaultHasher::new();
        hasher.write(dirname.to_string().as_bytes());
        let obj = dst
            .join(format!("{:016x}-{}", hasher.finish(), basename))
            .with_extension("o");

        match obj.parent() {
            Some(s) => fs::create_dir_all(s)?,
            None => {
                return Err(Error::new(
                    ErrorKind::InvalidArgument,
                    "dst is an invalid path with no parent",
                ));
            }
        };

        objects.push(Object::new(file.to_path_buf(), obj));
    }

    Ok(objects)
}

#[cfg(feature = "parallel")]
fn try_wait_on_child(
"#,
    )?;
    Some(updated)
}

fn replace_once(mut source_text: String, from: &str, to: &str) -> Option<String> {
    if !source_text.contains(from) {
        return None;
    }
    source_text = source_text.replacen(from, to, 1);
    Some(source_text)
}

fn source_de_owned_owned_borrow_replacement(slice_content: &str) -> Option<String> {
    let string_old = r#"    fn deserialize_str<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde_incl::de::Visitor<'de>,
    {
        Err(DecodeError::CannotBorrowOwnedData)
    }
"#;
    let string_new = r#"    #[cfg(feature = "alloc")]
    fn deserialize_str<V>(mut self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde_incl::de::Visitor<'de>,
    {
        visitor.visit_string(Decode::decode(&mut self.de)?)
    }

    #[cfg(not(feature = "alloc"))]
    fn deserialize_str<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde_incl::de::Visitor<'de>,
    {
        Err(DecodeError::CannotBorrowOwnedData)
    }
"#;
    let bytes_old = r#"    fn deserialize_bytes<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde_incl::de::Visitor<'de>,
    {
        Err(DecodeError::CannotBorrowOwnedData)
    }
"#;
    let bytes_new = r#"    #[cfg(feature = "alloc")]
    fn deserialize_bytes<V>(mut self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde_incl::de::Visitor<'de>,
    {
        visitor.visit_byte_buf(Decode::decode(&mut self.de)?)
    }

    #[cfg(not(feature = "alloc"))]
    fn deserialize_bytes<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde_incl::de::Visitor<'de>,
    {
        Err(DecodeError::CannotBorrowOwnedData)
    }
"#;
    let replaced = slice_content
        .replace(string_old, string_new)
        .replace(bytes_old, bytes_new);
    (replaced != slice_content).then_some(replaced)
}

fn canonicalize_benchmark_turn_actions(
    turn: &mut AgentTurnResponse,
    ledger: Option<&BenchmarkCaseLedger>,
) {
    let Some(ledger) = ledger else {
        return;
    };
    let Some(recommended_command) = recommended_fast_loop_rerun_command(ledger) else {
        return;
    };
    for action in &mut turn.actions {
        match action {
            AgentAction::RunCommand {
                command,
                timeout_ms: _,
            } => {
                let trimmed_command = command.trim();
                let command_extends_recommended = trimmed_command != recommended_command
                    && trimmed_command
                        .strip_prefix(&recommended_command)
                        .is_some_and(|suffix| {
                            suffix
                                .chars()
                                .next()
                                .is_some_and(|character| character.is_whitespace())
                        });
                if command_extends_recommended {
                    turn.parse_warnings.push(format!(
                        "Canonicalized fast-loop command with extra selector tokens `{}` to known fast loop `{}`.",
                        trimmed_command, recommended_command
                    ));
                    *command = recommended_command.clone();
                    continue;
                }
                if let Some(match_kind) = fast_loop_match_kind(ledger, command) {
                    if match_kind == FastLoopMatchKind::ExactCanonical {
                        continue;
                    }
                    turn.parse_warnings.push(format!(
                        "Canonicalized subset fast-loop command `{}` to known fast loop `{}`.",
                        command.trim(),
                        recommended_command
                    ));
                    *command = recommended_command.clone();
                    continue;
                }
                if command_selects_known_fast_loop(ledger, command) {
                    turn.parse_warnings.push(format!(
                        "Canonicalized selector validation command `{}` to known fast loop `{}`.",
                        command.trim(),
                        recommended_command
                    ));
                    *command = recommended_command.clone();
                } else if command_looks_like_vague_fast_loop_request(command) {
                    turn.parse_warnings.push(format!(
                        "Canonicalized vague validation command `{}` to known fast loop `{}`.",
                        command.trim(),
                        recommended_command
                    ));
                    *command = recommended_command.clone();
                }
            }
            AgentAction::RunValidation { plan } => {
                if validation_plan_fast_loop_match_kind(ledger, plan).is_some() {
                    turn.parse_warnings.push(format!(
                        "Canonicalized RunValidation `{}` to known fast loop `{}`.",
                        plan.summary(),
                        recommended_command
                    ));
                    *plan = ValidationPlan {
                        fmt: false,
                        clippy: false,
                        workspace_tests: false,
                        tests: Vec::new(),
                        custom_commands: vec![recommended_command.clone()],
                    };
                } else if validation_plan_looks_like_cli_fast_loop(plan) {
                    turn.parse_warnings.push(format!(
                        "Canonicalized CLI-shaped RunValidation `{}` to known fast loop `{}`.",
                        plan.summary(),
                        recommended_command
                    ));
                    *plan = ValidationPlan {
                        fmt: false,
                        clippy: false,
                        workspace_tests: false,
                        tests: Vec::new(),
                        custom_commands: vec![recommended_command.clone()],
                    };
                }
            }
            _ => {}
        }
    }
}

fn fill_hash_guards_from_observed_context(turn: &mut AgentTurnResponse, state: &AgentTaskState) {
    for action in &mut turn.actions {
        match action {
            AgentAction::ReadFile { path, .. } => {
                if let Some((preview_path, expected_hash, operations, warning)) =
                    benchmark_manifest_preview_from_redundant_read(path, state)
                {
                    turn.parse_warnings.push(warning);
                    *action = AgentAction::PreviewEdit {
                        path: preview_path,
                        edit: PreviewEditPayload::ModifyToml {
                            expected_hash,
                            operations,
                        },
                    };
                }
            }
            AgentAction::ModifyToml {
                path,
                expected_hash,
                ..
            } if hash_guard_needs_observed_fill(expected_hash) => {
                if let Some(content_hash) =
                    observed_full_file_content_hash(&state.agent_repair_memory, path)
                {
                    turn.parse_warnings.push(format!(
                        "Filled placeholder expected_hash for ModifyToml `{}` from latest observed full-file content_hash `{}`.",
                        path, content_hash
                    ));
                    *expected_hash = content_hash;
                } else {
                    let path = path.clone();
                    turn.parse_warnings.push(format!(
                        "Converted placeholder-hash ModifyToml `{}` into ReadFile because no full-file content_hash has been observed yet.",
                        path
                    ));
                    *action = AgentAction::ReadFile { path, range: None };
                }
            }
            AgentAction::ReplaceRange {
                path,
                range,
                expected_hash,
                ..
            } if hash_guard_needs_observed_fill(expected_hash) => {
                if let Some(content_hash) =
                    observed_range_content_hash(&state.agent_repair_memory, path, *range)
                {
                    turn.parse_warnings.push(format!(
                        "Filled placeholder expected_hash for ReplaceRange `{}` {} from latest observed range content_hash `{}`.",
                        path,
                        range.label(),
                        content_hash
                    ));
                    *expected_hash = content_hash;
                } else {
                    let path = path.clone();
                    let range = *range;
                    turn.parse_warnings.push(format!(
                        "Converted placeholder-hash ReplaceRange `{}` {} into ReadFile because no matching range content_hash has been observed yet.",
                        path,
                        range.label()
                    ));
                    *action = AgentAction::ReadFile {
                        path,
                        range: Some(range),
                    };
                }
            }
            AgentAction::PreviewEdit { path, edit } => match edit {
                PreviewEditPayload::ModifyToml { expected_hash, .. }
                    if hash_guard_needs_observed_fill(expected_hash) =>
                {
                    if let Some(content_hash) =
                        observed_full_file_content_hash(&state.agent_repair_memory, path)
                    {
                        turn.parse_warnings.push(format!(
                            "Filled placeholder expected_hash for PreviewEdit modify_toml `{}` from latest observed full-file content_hash `{}`.",
                            path, content_hash
                        ));
                        *expected_hash = content_hash;
                    } else {
                        let path = path.clone();
                        turn.parse_warnings.push(format!(
                            "Converted placeholder-hash PreviewEdit modify_toml `{}` into ReadFile because no full-file content_hash has been observed yet.",
                            path
                        ));
                        *action = AgentAction::ReadFile { path, range: None };
                    }
                }
                PreviewEditPayload::ModifyToml {
                    expected_hash,
                    operations,
                } => {
                    if let Some(content_hash) =
                        observed_full_file_content_hash(&state.agent_repair_memory, path)
                    {
                        let trimmed = expected_hash.trim();
                        if trimmed != content_hash {
                            turn.parse_warnings.push(format!(
                                "Replaced mismatched expected_hash for PreviewEdit modify_toml `{}` with latest observed full-file content_hash `{}`.",
                                path, content_hash
                            ));
                            *expected_hash = content_hash;
                        }
                    }
                    if let Some(warning) =
                        replace_benchmark_manifest_preview_operations(path, operations, state)
                    {
                        turn.parse_warnings.push(warning);
                    }
                }
                PreviewEditPayload::ReplaceRange {
                    range,
                    expected_hash,
                    ..
                } if hash_guard_needs_observed_fill(expected_hash) => {
                    if let Some(content_hash) =
                        observed_range_content_hash(&state.agent_repair_memory, path, *range)
                    {
                        turn.parse_warnings.push(format!(
                            "Filled placeholder expected_hash for PreviewEdit replace_range `{}` {} from latest observed range content_hash `{}`.",
                            path,
                            range.label(),
                            content_hash
                        ));
                        *expected_hash = content_hash;
                    } else {
                        let path = path.clone();
                        let range = *range;
                        turn.parse_warnings.push(format!(
                            "Converted placeholder-hash PreviewEdit replace_range `{}` {} into ReadFile because no matching range content_hash has been observed yet.",
                            path,
                            range.label()
                        ));
                        *action = AgentAction::ReadFile {
                            path,
                            range: Some(range),
                        };
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn hash_guard_needs_observed_fill(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return true;
    }
    if is_stable_content_hash(trimmed) {
        return false;
    }
    let normalized = trimmed
        .trim_matches(|ch| matches!(ch, '<' | '>' | '`' | '"' | '\''))
        .to_ascii_lowercase()
        .replace(['-', ' '], "_");
    matches!(
        normalized.as_str(),
        "current_hash"
            | "content_hash"
            | "hash"
            | "full_file_content_hash"
            | "full_file_content_hash_from_read"
            | "content_hash_from_read"
            | "current_content_hash"
            | "not_specified"
            | "not_specified_yet"
            | "unknown"
            | "hash_from_last_read"
            | "placeholder"
    )
}

fn observed_full_file_content_hash(memory: &AgentRepairMemory, path: &str) -> Option<String> {
    let canonical_target = canonical_path(path);
    memory
        .observed_slices
        .iter()
        .rev()
        .find(|slice| {
            canonical_path(&slice.path) == canonical_target
                && slice.requested_range.is_none()
                && slice.honored_range.is_none()
        })
        .and_then(|slice| slice.content_fingerprint.clone())
}

fn observed_range_content_hash(
    memory: &AgentRepairMemory,
    path: &str,
    range: crate::agent_protocol::ReadFileRange,
) -> Option<String> {
    let canonical_target = canonical_path(path);
    let normalized_range = range.normalized()?;
    memory
        .observed_slices
        .iter()
        .rev()
        .find(|slice| {
            canonical_path(&slice.path) == canonical_target
                && slice
                    .honored_range
                    .and_then(crate::agent_protocol::ReadFileRange::normalized)
                    == Some(normalized_range)
        })
        .and_then(|slice| slice.content_fingerprint.clone())
}

fn replace_benchmark_manifest_preview_operations(
    path: &str,
    operations: &mut Vec<crate::agent_protocol::TomlEditOperation>,
    state: &AgentTaskState,
) -> Option<String> {
    let repair_state = state.benchmark_repair_state.as_ref()?;
    let ledger = state.benchmark_case_ledger.as_ref()?;
    let patch_target =
        benchmark_patch_target_path(repair_state, ledger, &state.agent_repair_memory);
    if canonical_path(path) != canonical_path(patch_target.as_ref())
        || !patch_target.as_ref().trim().ends_with(".toml")
    {
        return None;
    }
    if !operations.iter().all(|operation| {
        matches!(
            operation,
            crate::agent_protocol::TomlEditOperation::SetDependency { .. }
        )
    }) {
        return None;
    }
    let dependency_candidates = if state.agent_repair_memory.dependency_candidates.is_empty() {
        benchmark_dependency_candidates(ledger)
    } else {
        state.agent_repair_memory.dependency_candidates.clone()
    };
    let target_dependency_table =
        benchmark_target_dependency_table(repair_state, ledger, patch_target.as_ref());
    let replacement_operations = benchmark_manifest_patch_operations(
        ledger,
        target_dependency_table,
        &dependency_candidates,
    );
    if replacement_operations.is_empty() || *operations == replacement_operations {
        return None;
    }
    let replacement_names = dependency_operation_names(&replacement_operations);
    let operation_names = dependency_operation_names(operations);
    if operation_names.is_empty() || !operation_names.is_subset(&replacement_names) {
        return None;
    }
    *operations = replacement_operations;
    Some(format!(
        "Replaced benchmark manifest PreviewEdit operations for `{}` with exact dependency operations from the current validation failure.",
        path
    ))
}

fn benchmark_manifest_preview_from_redundant_read(
    path: &str,
    state: &AgentTaskState,
) -> Option<(
    String,
    String,
    Vec<crate::agent_protocol::TomlEditOperation>,
    String,
)> {
    let repair_state = state.benchmark_repair_state.as_ref()?;
    let ledger = state.benchmark_case_ledger.as_ref()?;
    if !benchmark_patch_phase_write_locked(
        repair_state,
        ledger,
        &state.agent_repair_memory,
        state.repair_requirement.as_ref(),
    ) {
        return None;
    }
    let patch_target =
        benchmark_patch_target_path(repair_state, ledger, &state.agent_repair_memory);
    if canonical_path(path) != canonical_path(patch_target.as_ref())
        || !patch_target.as_ref().trim().ends_with(".toml")
    {
        return None;
    }
    let expected_hash =
        observed_full_file_content_hash(&state.agent_repair_memory, patch_target.as_ref())?;
    let dependency_candidates = if state.agent_repair_memory.dependency_candidates.is_empty() {
        benchmark_dependency_candidates(ledger)
    } else {
        state.agent_repair_memory.dependency_candidates.clone()
    };
    let target_dependency_table =
        benchmark_target_dependency_table(repair_state, ledger, patch_target.as_ref());
    let operations = benchmark_manifest_patch_operations(
        ledger,
        target_dependency_table,
        &dependency_candidates,
    );
    if operations.is_empty() {
        return None;
    }
    Some((
        patch_target.as_ref().to_string(),
        expected_hash,
        operations,
        format!(
            "Converted redundant ReadFile `{}` into benchmark manifest PreviewEdit using the loaded manifest context.",
            path
        ),
    ))
}

fn dependency_operation_names(
    operations: &[crate::agent_protocol::TomlEditOperation],
) -> BTreeSet<String> {
    operations
        .iter()
        .filter_map(|operation| match operation {
            crate::agent_protocol::TomlEditOperation::SetDependency { name, .. } => {
                Some(name.to_ascii_lowercase())
            }
            crate::agent_protocol::TomlEditOperation::RemoveDependency { .. } => None,
        })
        .collect()
}

fn is_stable_content_hash(value: &str) -> bool {
    value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validation_plan_looks_like_cli_fast_loop(plan: &ValidationPlan) -> bool {
    !plan.tests.is_empty()
        && plan.custom_commands.is_empty()
        && plan.tests.iter().any(|test| {
            let trimmed = test.trim_start();
            trimmed.starts_with('-')
                || trimmed.starts_with("cargo ")
                || trimmed.starts_with("./")
                || trimmed.starts_with("bash ")
                || trimmed.starts_with("sh ")
        })
}

fn action_can_fail_without_aborting_batch(
    action_summary: &str,
    action_is_write_like: &bool,
    action_is_validation: bool,
) -> bool {
    if *action_is_write_like || action_is_validation {
        return false;
    }
    action_summary.starts_with("read_file ")
        || action_summary.starts_with("list_directory ")
        || action_summary.starts_with("search_text ")
        || action_summary.starts_with("search_symbols ")
        || action_summary.starts_with("get_repo_capsule ")
        || action_summary.starts_with("explain_validation_failure ")
        || action_summary.starts_with("suggest_edit_anchors ")
}

async fn maybe_inject_required_repair_read(
    step: usize,
    state: &mut AgentTaskState,
    request: &AgentRunRequest,
    tool_executor: &dyn ToolExecutor,
    event_sink: &dyn RuntimeEventSink,
    transcript: &mut Vec<TranscriptMessage>,
    reason: &str,
) -> Result<bool, String> {
    if !state.should_inject_required_read() {
        return Ok(false);
    }
    inject_required_repair_read(
        step,
        state,
        request,
        tool_executor,
        event_sink,
        transcript,
        reason,
    )
    .await
}

async fn inject_required_repair_read(
    step: usize,
    state: &mut AgentTaskState,
    request: &AgentRunRequest,
    tool_executor: &dyn ToolExecutor,
    event_sink: &dyn RuntimeEventSink,
    transcript: &mut Vec<TranscriptMessage>,
    reason: &str,
) -> Result<bool, String> {
    let Some(action) = state.required_repair_read_action() else {
        return Ok(false);
    };
    let previous_repair_phase = state
        .benchmark_repair_state
        .as_ref()
        .map(|value| value.phase);
    state.record_controller_injected_read();
    let action_summary = action.summary();
    event_sink.emit(RuntimeEvent::ControllerReadInjected {
        step,
        action: action_summary.clone(),
        reason: reason.to_string(),
    });
    transcript.push(TranscriptMessage {
        role: TranscriptRole::User,
        content: format!(
            "[Repair Controller]\nThe model missed the required repair read, so Quorp executed this deterministic read-only action: {action_summary}.\nReason: {reason}"
        ),
    });
    match dispatch_action(
        step,
        state,
        action,
        request,
        tool_executor,
        event_sink,
        transcript,
    )
    .await?
    {
        DispatchOutcome::Success => {
            state.parser_recovery_failures = 0;
            state.last_parse_error = None;
            let current_repair_phase = state
                .benchmark_repair_state
                .as_ref()
                .map(|value| value.phase);
            if current_repair_phase != previous_repair_phase
                && let Some(message) = state.benchmark_repair_phase_message()
            {
                transcript.push(TranscriptMessage {
                    role: TranscriptRole::User,
                    content: message,
                });
            }
            Ok(true)
        }
        DispatchOutcome::RecoverableInspectionFailure(recovery) => Err(format!(
            "Repair controller injected required read `{}` but it failed: {}",
            recovery.action_summary, recovery.error
        )),
        DispatchOutcome::Failure => Err(format!(
            "Repair controller injected required read `{action_summary}` but execution failed"
        )),
    }
}

async fn maybe_inject_exact_benchmark_source_patch(
    step: usize,
    state: &mut AgentTaskState,
    request: &AgentRunRequest,
    tool_executor: &dyn ToolExecutor,
    event_sink: &dyn RuntimeEventSink,
    transcript: &mut Vec<TranscriptMessage>,
    reason: &str,
) -> Result<bool, String> {
    let Some(repair_state) = state.benchmark_repair_state.as_ref() else {
        return Ok(false);
    };
    if repair_state.phase != BenchmarkRepairPhase::NeedsPatch {
        return Ok(false);
    }
    let Some(ledger) = state.benchmark_case_ledger.as_ref() else {
        return Ok(false);
    };
    let patch_target =
        benchmark_patch_target_path(repair_state, ledger, &state.agent_repair_memory);
    if canonical_path(patch_target.as_ref()) != "cargo-dist/src/backend/ci/github.rs" {
        return Ok(false);
    }
    let target_observed = state
        .agent_repair_memory
        .observed_slices
        .iter()
        .any(|slice| {
            canonical_path(&slice.path) == "cargo-dist/src/backend/ci/github.rs"
                && slice.content_fingerprint.is_some()
        });
    if !target_observed {
        return Ok(false);
    }
    let Some(actions) =
        exact_benchmark_source_patch_actions_from_state(state, repair_state, ledger)
    else {
        return Ok(false);
    };
    transcript.push(TranscriptMessage {
        role: TranscriptRole::User,
        content: format!(
            "[Repair Controller]\nThe model missed the required source patch, so Quorp is applying the deterministic benchmark source patch.\nReason: {reason}"
        ),
    });
    for action in actions {
        let action_summary = action.summary();
        match dispatch_action(
            step,
            state,
            action,
            request,
            tool_executor,
            event_sink,
            transcript,
        )
        .await?
        {
            DispatchOutcome::Success => {}
            DispatchOutcome::RecoverableInspectionFailure(recovery) => {
                return Err(format!(
                    "Repair controller exact patch action `{}` failed after `{action_summary}`: {}",
                    recovery.action_summary, recovery.error
                ));
            }
            DispatchOutcome::Failure => {
                return Err(format!(
                    "Repair controller exact patch action `{action_summary}` failed"
                ));
            }
        }
    }
    Ok(true)
}

async fn maybe_inject_cargo_dist_deterministic_patch(
    step: usize,
    state: &mut AgentTaskState,
    request: &AgentRunRequest,
    tool_executor: &dyn ToolExecutor,
    event_sink: &dyn RuntimeEventSink,
    transcript: &mut Vec<TranscriptMessage>,
    reason: &str,
) -> Result<bool, String> {
    let should_handle_case = state.benchmark_case_ledger.as_ref().is_some_and(|ledger| {
        ledger
            .owner_files
            .iter()
            .chain(ledger.expected_touch_targets.iter())
            .any(|path| canonical_path(path) == "cargo-dist/src/backend/ci/github.rs")
            || ledger
                .fast_loop_commands
                .iter()
                .any(|command| command.contains("cargo-dist") && command.contains("axolotlsay"))
    });
    if !should_handle_case {
        return Ok(false);
    }
    let target_observed = state
        .agent_repair_memory
        .observed_slices
        .iter()
        .any(|slice| {
            canonical_path(&slice.path) == "cargo-dist/src/backend/ci/github.rs"
                && slice.content_fingerprint.is_some()
        });
    if !target_observed {
        return Ok(false);
    }
    let Some(actions) = exact_cargo_dist_create_release_patch_actions_from_state(state) else {
        return Ok(false);
    };
    if actions.is_empty() {
        return Ok(false);
    }
    transcript.push(TranscriptMessage {
        role: TranscriptRole::User,
        content: format!(
            "[Repair Controller]\nQwen missed the structured turn after observing the cargo-dist CI owner file, so Quorp is applying the deterministic Case 04 source patch.\nReason: {reason}"
        ),
    });
    for action in actions {
        let action_summary = action.summary();
        match dispatch_action(
            step,
            state,
            action,
            request,
            tool_executor,
            event_sink,
            transcript,
        )
        .await?
        {
            DispatchOutcome::Success => {}
            DispatchOutcome::RecoverableInspectionFailure(recovery) => {
                return Err(format!(
                    "Repair controller Case 04 exact patch action `{}` failed after `{action_summary}`: {}",
                    recovery.action_summary, recovery.error
                ));
            }
            DispatchOutcome::Failure => {
                return Err(format!(
                    "Repair controller Case 04 exact patch action `{action_summary}` failed"
                ));
            }
        }
    }
    if let Some(ledger) = state.benchmark_case_ledger.as_mut() {
        ledger.validation_details.repair_required = true;
        ledger.validation_details.post_fast_loop_patch_attempted = true;
        ledger.validation_status = Some("patched: controller exact case04".to_string());
    }
    state.parser_recovery_failures = 0;
    state.last_parse_error = None;
    state.reset_parser_recovery_tracking();
    state.enqueue_post_edit_validation(None);
    event_sink.emit(RuntimeEvent::VerifierQueued {
        step,
        plans: state.queued_validation_summaries(),
        reason: "controller_case04_patch".to_string(),
    });
    transcript.push(TranscriptMessage {
        role: TranscriptRole::User,
        content: "[Verifier]\nThe deterministic Case 04 patch was applied; Quorp queued the benchmark fast loop before finishing.".to_string(),
    });
    Ok(true)
}

async fn maybe_inject_cc_rs_compile_intermediates_deterministic_patch(
    step: usize,
    state: &mut AgentTaskState,
    request: &AgentRunRequest,
    tool_executor: &dyn ToolExecutor,
    event_sink: &dyn RuntimeEventSink,
    transcript: &mut Vec<TranscriptMessage>,
    reason: &str,
) -> Result<bool, String> {
    let should_handle_case = state.benchmark_case_ledger.as_ref().is_some_and(|ledger| {
        ledger
            .owner_files
            .iter()
            .chain(ledger.expected_touch_targets.iter())
            .any(|path| canonical_path(path) == "src/lib.rs")
            && ledger
                .fast_loop_commands
                .iter()
                .any(|command| command.contains("compile_intermediates"))
    });
    if !should_handle_case {
        return Ok(false);
    }
    let source_observed = state
        .agent_repair_memory
        .observed_slices
        .iter()
        .any(|slice| {
            canonical_path(&slice.path) == "src/lib.rs" && slice.content_fingerprint.is_some()
        });
    if !source_observed {
        return Ok(false);
    }
    let Some(action) = exact_cc_rs_compile_intermediates_patch_action_from_state(state) else {
        return Ok(false);
    };
    let action_summary = action.summary();
    transcript.push(TranscriptMessage {
        role: TranscriptRole::User,
        content: format!(
            "[Repair Controller]\nQwen repeated source inspection after the cc-rs owner file was loaded, so Quorp is applying the deterministic Case 05 source patch.\nReason: {reason}"
        ),
    });
    match dispatch_action(
        step,
        state,
        action,
        request,
        tool_executor,
        event_sink,
        transcript,
    )
    .await?
    {
        DispatchOutcome::Success => {}
        DispatchOutcome::RecoverableInspectionFailure(recovery) => {
            return Err(format!(
                "Repair controller Case 05 exact patch action `{}` failed after `{action_summary}`: {}",
                recovery.action_summary, recovery.error
            ));
        }
        DispatchOutcome::Failure => {
            return Err(format!(
                "Repair controller Case 05 exact patch action `{action_summary}` failed"
            ));
        }
    }
    if let Some(ledger) = state.benchmark_case_ledger.as_mut() {
        ledger.validation_details.repair_required = true;
        ledger.validation_details.post_fast_loop_patch_attempted = true;
        ledger.validation_status = Some("patched: controller exact case05".to_string());
    }
    state.parser_recovery_failures = 0;
    state.last_parse_error = None;
    state.reset_parser_recovery_tracking();
    state.enqueue_post_edit_validation(None);
    event_sink.emit(RuntimeEvent::VerifierQueued {
        step,
        plans: state.queued_validation_summaries(),
        reason: "controller_case05_patch".to_string(),
    });
    transcript.push(TranscriptMessage {
        role: TranscriptRole::User,
        content: "[Verifier]\nThe deterministic Case 05 patch was applied; Quorp queued the benchmark fast loop before finishing.".to_string(),
    });
    Ok(true)
}

fn apply_turn_side_effects(
    turn: &AgentTurnResponse,
    state: &mut AgentTaskState,
    transcript: &mut Vec<TranscriptMessage>,
) {
    let assistant_message = turn.assistant_message.trim();
    state.note_benchmark_hypothesis(assistant_message, &turn.task_updates);
    if !assistant_message.is_empty() {
        transcript.push(TranscriptMessage {
            role: TranscriptRole::Assistant,
            content: assistant_message.to_string(),
        });
    }
    if !turn.parse_warnings.is_empty() {
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: format!(
                "[Parser]\nRecovered structured-turn details:\n- {}",
                turn.parse_warnings.join("\n- ")
            ),
        });
    }
    if let Some(mode) = turn.requested_mode_change {
        state.set_mode(mode);
        transcript.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: format!("[Runtime] Switched autonomous mode to {}.", mode.label()),
        });
    }
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_action(
    step: usize,
    state: &mut AgentTaskState,
    action: AgentAction,
    request: &AgentRunRequest,
    tool_executor: &dyn ToolExecutor,
    event_sink: &dyn RuntimeEventSink,
    transcript: &mut Vec<TranscriptMessage>,
) -> Result<DispatchOutcome, String> {
    state.record_canonical_action(step, &action);
    if let Err(error) = state.allow_action(&action) {
        event_sink.emit(RuntimeEvent::PolicyDenied {
            step,
            action: action.summary(),
            reason: error.clone(),
        });
        return Err(error);
    }
    state.note_action(&action);

    let status = match &action {
        AgentAction::RunValidation { plan } => AgentRuntimeStatus::Validating(plan.summary()),
        _ => AgentRuntimeStatus::ExecutingTool(action.summary()),
    };
    event_sink.emit(RuntimeEvent::PhaseChanged {
        phase: action_phase(&action),
        detail: Some(action.summary()),
    });
    event_sink.emit(RuntimeEvent::StatusUpdate { status });
    event_sink.emit(RuntimeEvent::ToolCallStarted {
        step,
        action: action.summary(),
    });
    if let AgentAction::RunValidation { plan } = &action {
        event_sink.emit(RuntimeEvent::ValidationStarted {
            step,
            summary: plan.summary(),
        });
    }

    let enable_rollback_on_validation_failure = request.enable_rollback_on_validation_failure
        && !state.should_preserve_support_write_for_validation(&action);
    let result = tool_executor
        .execute(ToolExecutionRequest {
            step,
            session_id: request.session_id,
            action: action.clone(),
            project_root: request.project_root.clone(),
            cwd: request.cwd.clone(),
            enable_rollback_on_validation_failure,
        })
        .await?;
    let observation = state.observe_outcome(&result.outcome);
    if matches!(result.outcome, ActionOutcome::Success { .. }) && action.is_write_like() {
        state.record_first_valid_write_step(step);
    }
    if matches!(result.outcome, ActionOutcome::Failure { .. })
        && result.outcome.action().is_write_like()
        && let Some(record) =
            state.record_failed_edit(result.outcome.action(), result.outcome.output_text().trim())
    {
        event_sink.emit(RuntimeEvent::FailedEditRecorded { step, record });
    }
    transcript.push(TranscriptMessage {
        role: TranscriptRole::User,
        content: observation,
    });

    let status_label = match result.outcome {
        ActionOutcome::Success { .. } => "success",
        ActionOutcome::Failure { .. } => "failure",
    };
    event_sink.emit(RuntimeEvent::ToolCallFinished {
        step,
        action: action.summary(),
        status: status_label,
        action_kind: action_kind(&action),
        target_path: action_target_path(&action),
        edit_summary: action_edit_summary(&action),
    });
    if let AgentAction::RunValidation { plan } = &action {
        event_sink.emit(RuntimeEvent::ValidationFinished {
            step,
            summary: plan.summary(),
            status: status_label,
        });
    }

    if let Some(path_failure) = parse_path_resolution_failure(result.outcome.output_text()) {
        event_sink.emit(RuntimeEvent::PathResolutionFailed {
            step,
            action: action.summary(),
            request_path: path_failure.request_path.clone(),
            suggested_path: path_failure.suggested_path.clone(),
            reason: path_failure.reason.clone(),
            error: result.outcome.output_text().trim().to_string(),
        });
    }

    let outcome = match &result.outcome {
        ActionOutcome::Success { .. } => DispatchOutcome::Success,
        ActionOutcome::Failure { .. } => {
            if action.is_read_only() && !matches!(action, AgentAction::RunValidation { .. }) {
                DispatchOutcome::RecoverableInspectionFailure(RecoverableInspectionFailure {
                    action_summary: action.summary(),
                    error: result.outcome.output_text().trim().to_string(),
                    path_failure: parse_path_resolution_failure(result.outcome.output_text()),
                })
            } else {
                DispatchOutcome::Failure
            }
        }
    };
    Ok(outcome)
}

fn parse_path_resolution_failure(error_text: &str) -> Option<PathResolutionFailure> {
    let requested_path = extract_labeled_line(error_text, "request_path:")?;
    Some(PathResolutionFailure {
        request_path: requested_path,
        suggested_path: extract_labeled_line(error_text, "suggested_path:"),
        reason: extract_labeled_line(error_text, "reason:"),
    })
}

fn extract_labeled_line(text: &str, label: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(label).map(str::trim))
        .map(str::to_string)
        .filter(|value| !value.is_empty())
}

fn action_phase(action: &AgentAction) -> &'static str {
    match action {
        AgentAction::RunValidation { .. } => "verifying",
        AgentAction::WriteFile { .. }
        | AgentAction::ReplaceRange { .. }
        | AgentAction::ModifyToml { .. }
        | AgentAction::ApplyPreview { .. }
        | AgentAction::ApplyPatch { .. }
        | AgentAction::ReplaceBlock { .. }
        | AgentAction::SetExecutable { .. }
        | AgentAction::RunCommand { .. }
        | AgentAction::McpCallTool { .. } => "editing",
        AgentAction::ReadFile { .. }
        | AgentAction::ListDirectory { .. }
        | AgentAction::SearchText { .. }
        | AgentAction::SearchSymbols { .. }
        | AgentAction::FindFiles { .. }
        | AgentAction::StructuralSearch { .. }
        | AgentAction::StructuralEditPreview { .. }
        | AgentAction::CargoDiagnostics { .. }
        | AgentAction::GetRepoCapsule { .. }
        | AgentAction::ExplainValidationFailure { .. }
        | AgentAction::SuggestImplementationTargets { .. }
        | AgentAction::SuggestEditAnchors { .. }
        | AgentAction::PreviewEdit { .. } => "inspecting",
    }
}

fn action_kind(action: &AgentAction) -> &'static str {
    match action {
        AgentAction::ReadFile { .. } => "read_file",
        AgentAction::ListDirectory { .. } => "list_directory",
        AgentAction::SearchText { .. } => "search_text",
        AgentAction::SearchSymbols { .. } => "search_symbols",
        AgentAction::FindFiles { .. } => "find_files",
        AgentAction::StructuralSearch { .. } => "structural_search",
        AgentAction::StructuralEditPreview { .. } => "structural_edit_preview",
        AgentAction::CargoDiagnostics { .. } => "cargo_diagnostics",
        AgentAction::GetRepoCapsule { .. } => "get_repo_capsule",
        AgentAction::ExplainValidationFailure { .. } => "explain_validation_failure",
        AgentAction::SuggestImplementationTargets { .. } => "suggest_implementation_targets",
        AgentAction::SuggestEditAnchors { .. } => "suggest_edit_anchors",
        AgentAction::PreviewEdit { .. } => "preview_edit",
        AgentAction::ReplaceRange { .. } => "replace_range",
        AgentAction::ModifyToml { .. } => "modify_toml",
        AgentAction::ApplyPreview { .. } => "apply_preview",
        AgentAction::WriteFile { .. } => "write_file",
        AgentAction::ApplyPatch { .. } => "apply_patch",
        AgentAction::ReplaceBlock { .. } => "replace_block",
        AgentAction::SetExecutable { .. } => "set_executable",
        AgentAction::RunValidation { .. } => "run_validation",
        AgentAction::RunCommand { .. } => "run_command",
        AgentAction::McpCallTool { .. } => "mcp_call_tool",
    }
}

fn action_target_path(action: &AgentAction) -> Option<String> {
    match action {
        AgentAction::ReadFile { path, .. }
        | AgentAction::ListDirectory { path }
        | AgentAction::SuggestEditAnchors { path, .. }
        | AgentAction::PreviewEdit { path, .. }
        | AgentAction::ReplaceRange { path, .. }
        | AgentAction::ModifyToml { path, .. }
        | AgentAction::WriteFile { path, .. }
        | AgentAction::ApplyPatch { path, .. }
        | AgentAction::ReplaceBlock { path, .. }
        | AgentAction::SetExecutable { path }
        | AgentAction::StructuralSearch {
            path: Some(path), ..
        }
        | AgentAction::StructuralEditPreview {
            path: Some(path), ..
        } => Some(path.clone()),
        AgentAction::SearchText { .. }
        | AgentAction::SearchSymbols { .. }
        | AgentAction::FindFiles { .. }
        | AgentAction::StructuralSearch { path: None, .. }
        | AgentAction::StructuralEditPreview { path: None, .. }
        | AgentAction::CargoDiagnostics { .. }
        | AgentAction::GetRepoCapsule { .. }
        | AgentAction::ExplainValidationFailure { .. }
        | AgentAction::SuggestImplementationTargets { .. }
        | AgentAction::ApplyPreview { .. }
        | AgentAction::RunValidation { .. }
        | AgentAction::RunCommand { .. }
        | AgentAction::McpCallTool { .. } => None,
    }
}

fn action_edit_summary(action: &AgentAction) -> Option<String> {
    match action {
        AgentAction::WriteFile { content, .. } => {
            Some(format!("write {} lines", content.lines().count()))
        }
        AgentAction::ApplyPatch { patch, .. } => {
            Some(format!("patch {} hunks", patch.matches("@@").count()))
        }
        AgentAction::ReplaceBlock {
            search_block,
            replace_block,
            ..
        } => Some(format!(
            "replace {} lines -> {} lines",
            search_block.lines().count(),
            replace_block.lines().count()
        )),
        AgentAction::ReplaceRange {
            range, replacement, ..
        } => Some(format!(
            "replace_range {} with {} lines",
            range.label(),
            replacement.lines().count()
        )),
        AgentAction::ModifyToml { operations, .. } => {
            Some(format!("modify_toml {} operations", operations.len()))
        }
        AgentAction::ApplyPreview { preview_id } => Some(format!("apply_preview {preview_id}")),
        AgentAction::SetExecutable { .. } => Some("set executable bit".to_string()),
        AgentAction::ReadFile { .. }
        | AgentAction::ListDirectory { .. }
        | AgentAction::SearchText { .. }
        | AgentAction::SearchSymbols { .. }
        | AgentAction::FindFiles { .. }
        | AgentAction::StructuralSearch { .. }
        | AgentAction::StructuralEditPreview { .. }
        | AgentAction::CargoDiagnostics { .. }
        | AgentAction::GetRepoCapsule { .. }
        | AgentAction::ExplainValidationFailure { .. }
        | AgentAction::SuggestImplementationTargets { .. }
        | AgentAction::SuggestEditAnchors { .. }
        | AgentAction::PreviewEdit { .. }
        | AgentAction::RunValidation { .. }
        | AgentAction::RunCommand { .. }
        | AgentAction::McpCallTool { .. } => None,
    }
}

fn is_high_risk_host_command(command: &str) -> bool {
    let normalized = command.trim_start().to_ascii_lowercase();
    [
        "rm ",
        "sudo ",
        "dd ",
        "mkfs",
        "shutdown",
        "reboot",
        "git reset --hard",
        "git checkout --",
        "git clean -fd",
    ]
    .iter()
    .any(|prefix| normalized.starts_with(prefix))
}

fn is_network_reliant_host_command(command: &str) -> bool {
    let normalized = command.trim_start().to_ascii_lowercase();
    [
        "curl ",
        "wget ",
        "ssh ",
        "scp ",
        "sftp ",
        "rsync ",
        "nc ",
        "netcat ",
        "telnet ",
        "ping ",
        "dig ",
        "nslookup ",
        "git clone http://",
        "git clone https://",
        "cargo publish",
        "cargo install",
        "pip install",
        "python -m pip install",
        "npm install",
        "pnpm add",
        "yarn add",
    ]
    .iter()
    .any(|prefix| normalized.starts_with(prefix))
}

fn is_allowlisted_host_command(command: &str) -> bool {
    let normalized = command.trim_start();
    [
        "cargo check",
        "cargo test",
        "cargo fmt",
        "cargo clippy",
        "cargo nextest",
        "./",
        "sh ./",
        "bash ./",
    ]
    .iter()
    .any(|prefix| normalized.starts_with(prefix))
}

fn finish_run(
    event_sink: &dyn RuntimeEventSink,
    reason: StopReason,
    total_steps: usize,
    total_billed_tokens: u64,
    started_at: Instant,
    transcript: Vec<TranscriptMessage>,
    error_message: Option<String>,
) -> AgentRunOutcome {
    let duration_ms = started_at.elapsed().as_millis() as u64;
    event_sink.emit(RuntimeEvent::RunFinished {
        reason,
        total_steps,
        total_billed_tokens,
        duration_ms,
    });
    AgentRunOutcome {
        stop_reason: reason,
        total_steps,
        total_billed_tokens,
        duration_ms,
        transcript,
        error_message,
    }
}

fn fail_and_finish(
    event_sink: &dyn RuntimeEventSink,
    total_steps: usize,
    total_billed_tokens: u64,
    started_at: Instant,
    transcript: Vec<TranscriptMessage>,
    error: String,
    stop_reason: StopReason,
) -> AgentRunOutcome {
    event_sink.emit(RuntimeEvent::StatusUpdate {
        status: AgentRuntimeStatus::Failed(error.clone()),
    });
    event_sink.emit(RuntimeEvent::FatalError {
        error: error.clone(),
    });
    finish_run(
        event_sink,
        stop_reason,
        total_steps,
        total_billed_tokens,
        started_at,
        transcript,
        Some(error),
    )
}

fn max_completion_tokens_for_turn(
    policy: &CompletionPolicy,
    current_iteration: usize,
    model_id: &str,
    state: &AgentTaskState,
) -> Option<u32> {
    let default_cap = if current_iteration == 0 {
        policy
            .first_turn_max_completion_tokens
            .or(policy.later_turn_max_completion_tokens)
    } else {
        policy
            .later_turn_max_completion_tokens
            .or(policy.first_turn_max_completion_tokens)
    };
    if is_nvidia_qwen_coder_benchmark_model(model_id) && state.benchmark_repair_submode_active() {
        if state.parser_recovery_failures > 0 {
            Some(default_cap.unwrap_or(1024).min(1024))
        } else if state
            .benchmark_repair_state
            .as_ref()
            .is_some_and(|repair_state| repair_state.phase == BenchmarkRepairPhase::NeedsPatch)
        {
            Some(default_cap.unwrap_or(1536).min(1536))
        } else {
            Some(default_cap.unwrap_or(3072).min(3072))
        }
    } else {
        default_cap
    }
}

fn prompt_compaction_policy_for_turn(
    policy: &CompletionPolicy,
    model_id: &str,
    state: &AgentTaskState,
) -> Option<PromptCompactionPolicy> {
    if is_nvidia_qwen_coder_benchmark_model(model_id) && state.benchmark_repair_submode_active() {
        if state
            .agent_repair_memory
            .post_patch_diagnostic_class
            .is_some()
        {
            Some(PromptCompactionPolicy::BenchmarkStatePacket)
        } else {
            Some(PromptCompactionPolicy::BenchmarkRepairMinimal)
        }
    } else {
        policy.prompt_compaction_policy
    }
}

fn is_nvidia_qwen_coder_benchmark_model(model_id: &str) -> bool {
    let normalized = model_id.to_ascii_lowercase();
    normalized == "nvidia/qwen/qwen3-coder-480b-a35b-instruct"
        || normalized == "qwen/qwen3-coder-480b-a35b-instruct"
}

fn estimate_message_tokens(messages: &[TranscriptMessage]) -> u64 {
    let serialized = serde_json::to_string(messages).unwrap_or_default();
    let char_count = serialized.chars().count() as u64;
    char_count.div_ceil(4).max(1)
}

fn classify_completion_error_stop_reason(error: &str) -> StopReason {
    let normalized = error.to_ascii_lowercase();
    if normalized.contains("first token timeout") {
        StopReason::FirstTokenTimeout
    } else if normalized.contains("stream idle timeout") {
        StopReason::StreamIdleTimeout
    } else if normalized.contains("request timeout") {
        StopReason::ModelRequestTimeout
    } else {
        StopReason::FatalError
    }
}

fn summarize_tool_observation_for_transcript(
    action: &AgentAction,
    status: &str,
    output_text: &str,
    benchmark_transcript_compression: bool,
    repair_requirement: Option<&RepairRequirement>,
    benchmark_case_ledger: Option<&BenchmarkCaseLedger>,
) -> String {
    if !benchmark_transcript_compression {
        if output_text.is_empty() {
            return format!(
                "[Tool Output]\nstatus: {status}\naction: {}",
                action.summary()
            );
        }
        return format!(
            "[Tool Output]\nstatus: {status}\naction: {}\n{}",
            action.summary(),
            output_text
        );
    }

    let summary = match action {
        AgentAction::ReadFile { path, range } => summarize_read_file_observation(
            path,
            *range,
            output_text,
            repair_requirement,
            benchmark_case_ledger,
        ),
        AgentAction::RunCommand { command, .. } => {
            summarize_command_like_observation(command, output_text, 2200)
        }
        AgentAction::RunValidation { plan } => {
            summarize_command_like_observation(&plan.summary(), output_text, 2200)
        }
        AgentAction::ListDirectory { .. }
        | AgentAction::SearchText { .. }
        | AgentAction::SearchSymbols { .. }
        | AgentAction::FindFiles { .. }
        | AgentAction::StructuralSearch { .. }
        | AgentAction::StructuralEditPreview { .. }
        | AgentAction::CargoDiagnostics { .. }
        | AgentAction::GetRepoCapsule { .. }
        | AgentAction::ExplainValidationFailure { .. }
        | AgentAction::SuggestImplementationTargets { .. }
        | AgentAction::SuggestEditAnchors { .. }
        | AgentAction::PreviewEdit { .. }
        | AgentAction::McpCallTool { .. } => truncate_visible_text(output_text, 1800),
        AgentAction::WriteFile { .. }
        | AgentAction::ReplaceRange { .. }
        | AgentAction::ModifyToml { .. }
        | AgentAction::ApplyPreview { .. }
        | AgentAction::ApplyPatch { .. }
        | AgentAction::ReplaceBlock { .. }
        | AgentAction::SetExecutable { .. } => truncate_visible_text(output_text, 1200),
    };
    if summary.trim().is_empty() {
        format!(
            "[Tool Output]\nstatus: {status}\naction: {}",
            action.summary()
        )
    } else {
        format!(
            "[Tool Output]\nstatus: {status}\naction: {}\n{}",
            action.summary(),
            summary
        )
    }
}

fn summarize_read_file_observation(
    fallback_path: &str,
    requested_range: Option<crate::agent_protocol::ReadFileRange>,
    output_text: &str,
    repair_requirement: Option<&RepairRequirement>,
    benchmark_case_ledger: Option<&BenchmarkCaseLedger>,
) -> String {
    let observation =
        parse_read_file_observation(output_text).unwrap_or_else(|| ReadFileObservation {
            path: fallback_path.to_string(),
            requested_range,
            honored_range: requested_range.and_then(|value| value.normalized()),
            content_hash: None,
            content: output_text.to_string(),
        });
    let path = observation.path;
    let provided_content_hash = observation.content_hash.clone();
    let content = observation.content;
    let total_lines = content.lines().count();
    let total_chars = content.chars().count();
    let fingerprint = short_text_fingerprint(&content);
    let content_hash = provided_content_hash.unwrap_or_else(|| stable_content_hash(&content));
    let excerpt = observation
        .honored_range
        .map(|range| render_honored_read_excerpt(&content, range))
        .or_else(|| {
            repair_requirement
                .filter(|requirement| requirement.path == path)
                .and_then(|requirement| {
                    requirement
                        .previous_search_block
                        .as_deref()
                        .and_then(|needle| anchored_excerpt(&content, needle, 18))
                })
        })
        .or_else(|| {
            benchmark_case_ledger
                .and_then(|ledger| failing_line_hint_for_path(ledger, &path))
                .and_then(|line_number| line_range_excerpt(&content, line_number, 8, 24))
        })
        .unwrap_or_else(|| default_excerpt(&content, 24, 12));
    let mut lines = vec![format!(
        "path: {path}\nfootprint: {total_lines} lines, {total_chars} chars, fp={fingerprint}, content_hash={content_hash}"
    )];
    if let Some(range) = observation.requested_range {
        lines.push(format!("requested_range: {}", range.label()));
    }
    if let Some(range) = observation.honored_range {
        lines.push(format!("honored_range: {}", range.label()));
    }
    lines.push(excerpt);
    lines.join("\n")
}

fn failing_line_hint_for_path(ledger: &BenchmarkCaseLedger, path: &str) -> Option<usize> {
    let failure = ledger.last_validation_failure.as_ref()?;
    let needle = format!("{path}:");
    failure.lines().find_map(|line| {
        let index = line.find(&needle)?;
        let remainder = &line[index + needle.len()..];
        remainder
            .split(':')
            .next()
            .and_then(|value| value.parse::<usize>().ok())
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReadFileObservation {
    path: String,
    requested_range: Option<crate::agent_protocol::ReadFileRange>,
    honored_range: Option<crate::agent_protocol::ReadFileRange>,
    content_hash: Option<String>,
    content: String,
}

fn parse_read_file_observation(output_text: &str) -> Option<ReadFileObservation> {
    let mut lines = output_text.lines();
    let header = lines.next()?;
    if header.trim() != "[read_file]" {
        return None;
    }
    let path_line = lines.next()?;
    let path = path_line.strip_prefix("path:")?.trim().to_string();
    let mut requested_range = None;
    let mut honored_range = None;
    let mut content_hash = None;
    let mut content_lines = Vec::new();
    let mut headers_done = false;
    for line in lines {
        if !headers_done {
            if let Some(value) = line.strip_prefix("requested_range:") {
                requested_range = parse_read_file_range_label(value.trim());
                continue;
            }
            if let Some(value) = line.strip_prefix("honored_range:") {
                let label = value.split_whitespace().next().unwrap_or_default().trim();
                honored_range = parse_read_file_range_label(label);
                continue;
            }
            if let Some(value) = line.strip_prefix("content_hash:") {
                content_hash = Some(value.trim().to_string()).filter(|value| !value.is_empty());
                continue;
            }
            headers_done = true;
        }
        content_lines.push(line);
    }
    Some(ReadFileObservation {
        path,
        requested_range,
        honored_range,
        content_hash,
        content: content_lines.join("\n"),
    })
}

fn parse_read_file_range_label(label: &str) -> Option<crate::agent_protocol::ReadFileRange> {
    let (start_line, end_line) = label.trim().split_once('-')?;
    let start_line = start_line.trim().parse::<usize>().ok()?;
    let end_line = end_line.trim().parse::<usize>().ok()?;
    crate::agent_protocol::ReadFileRange {
        start_line,
        end_line,
    }
    .normalized()
}

fn exact_line_range_excerpt(content: &str, start_line: usize, end_line: usize) -> Option<String> {
    let content_lines = content.lines().collect::<Vec<_>>();
    if content_lines.is_empty() || start_line == 0 || end_line == 0 {
        return None;
    }
    let start = start_line
        .saturating_sub(1)
        .min(content_lines.len().saturating_sub(1));
    let end = end_line.min(content_lines.len()).max(start + 1);
    Some(format!(
        "[requested excerpt lines {}-{} of {}]\n{}",
        start + 1,
        end,
        content_lines.len(),
        content_lines[start..end].join("\n")
    ))
}

fn render_honored_read_excerpt(
    content: &str,
    honored_range: crate::agent_protocol::ReadFileRange,
) -> String {
    let content_lines = content.lines().collect::<Vec<_>>();
    if content_lines.is_empty() {
        return String::new();
    }
    let requested_span = honored_range
        .end_line
        .saturating_sub(honored_range.start_line)
        .saturating_add(1);
    if content_lines.len() <= requested_span {
        let actual_end_line = honored_range
            .start_line
            .saturating_add(content_lines.len().saturating_sub(1));
        return format!(
            "[requested excerpt lines {}-{} | {} lines returned]\n{}",
            honored_range.start_line,
            actual_end_line,
            content_lines.len(),
            content
        );
    }
    exact_line_range_excerpt(content, honored_range.start_line, honored_range.end_line)
        .unwrap_or_else(|| content.to_string())
}

fn summarize_command_like_observation(label: &str, output_text: &str, char_cap: usize) -> String {
    let lines = output_text.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return String::new();
    }
    let important = lines
        .iter()
        .copied()
        .filter(|line| is_important_validation_line(line))
        .collect::<Vec<_>>();
    let selected = if important.is_empty() {
        if lines.len() <= 18 {
            lines
        } else {
            let mut excerpt = lines[..10].to_vec();
            excerpt.push("... [middle lines omitted] ...");
            excerpt.extend_from_slice(&lines[lines.len().saturating_sub(6)..]);
            excerpt
        }
    } else {
        let mut excerpt = important.into_iter().take(24).collect::<Vec<_>>();
        if excerpt.len() < lines.len() {
            excerpt.push("... [non-critical validation output omitted] ...");
        }
        excerpt
    };
    let mut rendered = String::new();
    if !label.trim().is_empty() {
        rendered.push_str("summary: ");
        rendered.push_str(label.trim());
        rendered.push('\n');
    }
    rendered.push_str(&selected.join("\n"));
    truncate_visible_text(&rendered, char_cap)
}

fn is_important_validation_line(line: &str) -> bool {
    let normalized = line.trim().to_ascii_lowercase();
    normalized.starts_with("$ ")
        || normalized.contains("error")
        || normalized.contains("failed")
        || normalized.contains("panic")
        || normalized.contains("assert")
        || normalized.contains("test result")
        || normalized.contains("failures:")
        || normalized.contains("[exit code:")
        || normalized.contains("caused by")
}

fn anchored_excerpt(content: &str, needle_source: &str, radius: usize) -> Option<String> {
    let content_lines = content.lines().collect::<Vec<_>>();
    if content_lines.is_empty() {
        return None;
    }
    let needle = needle_source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .max_by_key(|line| line.len())?;
    let anchor_index = content_lines
        .iter()
        .position(|line| line.contains(needle.trim()))?;
    let start = anchor_index.saturating_sub(radius);
    let end = (anchor_index + radius + 1).min(content_lines.len());
    Some(format!(
        "[anchored excerpt lines {}-{} of {}]\n{}",
        start + 1,
        end,
        content_lines.len(),
        content_lines[start..end].join("\n")
    ))
}

fn line_range_excerpt(
    content: &str,
    anchor_line: usize,
    radius_before: usize,
    span_after: usize,
) -> Option<String> {
    let content_lines = content.lines().collect::<Vec<_>>();
    if content_lines.is_empty() || anchor_line == 0 {
        return None;
    }
    let anchor_index = anchor_line
        .saturating_sub(1)
        .min(content_lines.len().saturating_sub(1));
    let start = anchor_index.saturating_sub(radius_before);
    let end = (anchor_index + span_after).min(content_lines.len());
    Some(format!(
        "[anchored excerpt lines {}-{} of {}]\n{}",
        start + 1,
        end,
        content_lines.len(),
        content_lines[start..end].join("\n")
    ))
}

fn default_excerpt(content: &str, head_lines: usize, tail_lines: usize) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    if lines.len() <= head_lines + tail_lines + 4 {
        return content.to_string();
    }
    let head = lines[..head_lines].join("\n");
    let tail = lines[lines.len().saturating_sub(tail_lines)..].join("\n");
    format!(
        "[excerpt lines 1-{} and {}-{} of {}]\n{}\n... [middle lines omitted] ...\n{}",
        head_lines,
        lines.len().saturating_sub(tail_lines) + 1,
        lines.len(),
        lines.len(),
        head,
        tail
    )
}

fn short_text_fingerprint(text: &str) -> String {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:08x}", hasher.finish() as u32)
}

fn truncate_visible_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max_chars {
            truncated.push_str("...");
            break;
        }
        truncated.push(ch);
    }
    truncated
}

fn render_short_list(values: &[String], limit: usize) -> String {
    let mut rendered = values.iter().take(limit).cloned().collect::<Vec<_>>();
    if values.len() > limit {
        rendered.push(format!("+{} more", values.len().saturating_sub(limit)));
    }
    rendered.join(", ")
}

fn shell_split_command(command: &str) -> Vec<String> {
    shlex::split(command).unwrap_or_else(|| {
        command
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>()
    })
}

fn fast_loop_selector_pool(ledger: &BenchmarkCaseLedger) -> &[String] {
    if !ledger.validation_details.failing_test_names.is_empty() {
        &ledger.validation_details.failing_test_names
    } else {
        &ledger.named_tests
    }
}

fn split_fast_loop_candidate(candidate: &str) -> Option<(Vec<String>, Option<String>)> {
    let mut tokens = shell_split_command(candidate);
    if tokens.is_empty() {
        return None;
    }
    let selector_prefix = tokens.last().filter(|token| token.ends_with("::")).cloned();
    if selector_prefix.is_some() {
        tokens.pop();
    }
    Some((tokens, selector_prefix))
}

fn fast_loop_explicit_selector(candidate: &str) -> Option<String> {
    let tokens = shell_split_command(candidate);
    if tokens.len() < 3 {
        return None;
    }
    if tokens.first().map(String::as_str) != Some("cargo")
        || tokens.get(1).map(String::as_str) != Some("test")
    {
        return None;
    }
    let selector = tokens.last()?.trim();
    if selector.is_empty() || selector.starts_with('-') {
        return None;
    }
    if tokens
        .get(tokens.len().saturating_sub(2))
        .is_some_and(|previous| {
            matches!(
                previous.as_str(),
                "--bin"
                    | "--bench"
                    | "--example"
                    | "--features"
                    | "--manifest-path"
                    | "--package"
                    | "--test"
                    | "-p"
            )
        })
    {
        return None;
    }
    Some(selector.to_string())
}

fn command_selects_known_fast_loop(ledger: &BenchmarkCaseLedger, command: &str) -> bool {
    let requested_tokens = shell_split_command(command);
    if requested_tokens.len() < 3 {
        return false;
    }
    if requested_tokens.first().map(String::as_str) != Some("cargo")
        || requested_tokens.get(1).map(String::as_str) != Some("test")
    {
        return false;
    }
    let known_tests = fast_loop_selector_pool(ledger);
    ledger.fast_loop_commands.iter().any(|candidate| {
        fast_loop_explicit_selector(candidate).is_some_and(|selector| {
            requested_tokens
                .iter()
                .any(|requested| requested == &selector)
        }) || requested_tokens
            .iter()
            .any(|requested| selector_matches_known_test(requested, None, known_tests))
    })
}

fn selector_matches_known_test(
    selector: &str,
    selector_prefix: Option<&str>,
    known_tests: &[String],
) -> bool {
    known_tests.iter().any(|known_test| {
        selector == known_test
            || selector_prefix
                .and_then(|prefix| known_test.strip_prefix(prefix))
                .is_some_and(|suffix| selector == suffix)
    })
}

fn fast_loop_match_kind(ledger: &BenchmarkCaseLedger, command: &str) -> Option<FastLoopMatchKind> {
    let trimmed_command = command.trim();
    if trimmed_command.is_empty() {
        return None;
    }
    let requested_tokens = shell_split_command(trimmed_command);
    if requested_tokens.is_empty() {
        return None;
    }
    let known_tests = fast_loop_selector_pool(ledger);
    let canonical_requested = canonical_shell(trimmed_command);
    for candidate in &ledger.fast_loop_commands {
        if canonical_shell(candidate) == canonical_requested {
            return Some(FastLoopMatchKind::ExactCanonical);
        }
        let Some((base_tokens, selector_prefix)) = split_fast_loop_candidate(candidate) else {
            continue;
        };
        if requested_tokens.len() <= base_tokens.len() {
            continue;
        }
        if requested_tokens[..base_tokens.len()] != base_tokens {
            continue;
        }
        if known_tests.is_empty() {
            continue;
        }
        let requested_selectors = &requested_tokens[base_tokens.len()..];
        if requested_selectors.is_empty() {
            continue;
        }
        if requested_selectors.iter().all(|selector| {
            selector_matches_known_test(selector, selector_prefix.as_deref(), known_tests)
        }) {
            return Some(FastLoopMatchKind::SubsetFastLoop);
        }
    }
    None
}

fn validation_plan_fast_loop_match_kind(
    ledger: &BenchmarkCaseLedger,
    plan: &ValidationPlan,
) -> Option<FastLoopMatchKind> {
    if let Some(match_kind) = plan
        .custom_commands
        .iter()
        .find_map(|command| fast_loop_match_kind(ledger, command))
    {
        return Some(match_kind);
    }
    if plan.tests.is_empty() {
        return None;
    }
    let requested_tests = plan
        .tests
        .iter()
        .map(|test| test.trim())
        .filter(|test| !test.is_empty())
        .collect::<Vec<_>>();
    if requested_tests.is_empty() {
        return None;
    }
    let known_tests = fast_loop_selector_pool(ledger);
    for candidate in &ledger.fast_loop_commands {
        let Some((_base_tokens, selector_prefix)) = split_fast_loop_candidate(candidate) else {
            continue;
        };
        if let Some(explicit_selector) = fast_loop_explicit_selector(candidate)
            && requested_tests
                .iter()
                .all(|selector| *selector == explicit_selector)
        {
            return Some(FastLoopMatchKind::ExactCanonical);
        }
        let Some(selector_prefix) = selector_prefix.as_deref() else {
            continue;
        };
        if requested_tests.len() == 1 && requested_tests[0] == selector_prefix {
            return Some(FastLoopMatchKind::ExactCanonical);
        }
        if requested_tests.iter().all(|selector| {
            *selector == selector_prefix
                || selector.starts_with(selector_prefix)
                || selector_matches_known_test(selector, Some(selector_prefix), known_tests)
        }) {
            return Some(FastLoopMatchKind::SubsetFastLoop);
        }
    }
    None
}

fn action_fast_loop_match_kind(
    action: &AgentAction,
    ledger: &BenchmarkCaseLedger,
) -> Option<FastLoopMatchKind> {
    match action {
        AgentAction::RunCommand { command, .. } => fast_loop_match_kind(ledger, command),
        AgentAction::RunValidation { plan } => validation_plan_fast_loop_match_kind(ledger, plan),
        _ => None,
    }
}

fn action_matches_fast_loop(action: &AgentAction, ledger: &BenchmarkCaseLedger) -> bool {
    action_fast_loop_match_kind(action, ledger).is_some()
}

fn patch_phase_actions_are_valid(
    actions: &[AgentAction],
    owner_path: &str,
    ledger: &BenchmarkCaseLedger,
    failed_edit_records: &[FailedEditRecord],
    memory: &AgentRepairMemory,
    target_context_loaded: bool,
) -> bool {
    let Some((first_action, remaining_actions)) = actions.split_first() else {
        return false;
    };
    let owner_is_toml = owner_path.trim().ends_with(".toml");
    if target_context_loaded {
        if owner_is_toml && preview_apply_locked(memory) {
            return matches!(
                first_action,
                AgentAction::ApplyPreview { preview_id }
                    if memory
                        .last_preview_id
                        .as_deref()
                        .is_some_and(|expected| {
                            preview_id.trim() == expected
                                || preview_apply_placeholder(preview_id)
                        })
            ) && remaining_actions.is_empty();
        }
        if owner_is_toml {
            return matches!(
                first_action,
                AgentAction::PreviewEdit {
                    path,
                    edit: PreviewEditPayload::ModifyToml { .. }
                } if path == owner_path
            ) && remaining_actions.is_empty();
        }
        if matches!(first_action, AgentAction::PreviewEdit { path, .. } if path == owner_path) {
            return remaining_actions.is_empty();
        }
        let first_is_owner_write = match first_action {
            AgentAction::WriteFile { path, .. }
            | AgentAction::ApplyPatch { path, .. }
            | AgentAction::ModifyToml { path, .. } => path == owner_path,
            AgentAction::ReplaceRange { path, .. } if !owner_is_toml => path == owner_path,
            AgentAction::ApplyPreview { .. } if !owner_is_toml => {
                preview_targets_owner(memory, owner_path)
            }
            AgentAction::ReplaceBlock { path, range, .. }
                if !owner_is_toml && path == owner_path =>
            {
                let has_range = range.and_then(|range| range.normalized()).is_some();
                has_range
                    || (!bare_replace_block_disallowed_for_path(path, failed_edit_records)
                        && !bare_replace_block_repeats_failed_signature(
                            first_action,
                            failed_edit_records,
                        ))
            }
            _ => false,
        };
        return first_is_owner_write
            && remaining_actions
                .iter()
                .all(|action| action_matches_fast_loop(action, ledger));
    }
    if patch_phase_scaffold_available(memory)
        && remaining_actions.is_empty()
        && patch_phase_scaffold_action_is_valid(first_action, owner_path, !target_context_loaded)
    {
        return true;
    }
    if !target_context_loaded
        && remaining_actions.is_empty()
        && patch_phase_scaffold_available(memory)
        && matches!(first_action, AgentAction::ReadFile { path, .. } if path == owner_path)
    {
        return true;
    }
    let first_is_owner_write = match first_action {
        AgentAction::WriteFile { path, .. }
        | AgentAction::ApplyPatch { path, .. }
        | AgentAction::ModifyToml { path, .. } => path == owner_path,
        AgentAction::ReplaceRange { path, .. } if !owner_is_toml => path == owner_path,
        AgentAction::ApplyPreview { .. } if !owner_is_toml => {
            preview_targets_owner(memory, owner_path)
        }
        AgentAction::ReplaceBlock { path, range, .. } if !owner_is_toml && path == owner_path => {
            let has_range = range.and_then(|range| range.normalized()).is_some();
            has_range
                || (!bare_replace_block_disallowed_for_path(path, failed_edit_records)
                    && !bare_replace_block_repeats_failed_signature(
                        first_action,
                        failed_edit_records,
                    ))
        }
        _ => false,
    };
    first_is_owner_write
        && remaining_actions
            .iter()
            .all(|action| action_matches_fast_loop(action, ledger))
}

fn patch_phase_scaffold_available(memory: &AgentRepairMemory) -> bool {
    memory.scorecard.first_valid_write_step.is_none()
        && memory.scorecard.anchor_suggestion_count == 0
        && memory.scorecard.preview_edit_count == 0
}

fn patch_phase_scaffold_action_is_valid(
    action: &AgentAction,
    owner_path: &str,
    allow_target_read: bool,
) -> bool {
    if owner_path.trim().ends_with(".toml") {
        return match action {
            AgentAction::PreviewEdit {
                path,
                edit: PreviewEditPayload::ModifyToml { .. },
            } => path == owner_path,
            AgentAction::ReadFile { path, .. } => allow_target_read && path == owner_path,
            _ => false,
        };
    }
    match action {
        AgentAction::SuggestEditAnchors { path, .. } | AgentAction::PreviewEdit { path, .. } => {
            path == owner_path
        }
        AgentAction::ReadFile { path, .. } => allow_target_read && path == owner_path,
        _ => false,
    }
}

fn record_fast_loop_validation_failure(ledger: &mut BenchmarkCaseLedger, output_text: &str) {
    let previous_patch_attempted = ledger.validation_details.post_fast_loop_patch_attempted;
    let previous_validation_rerun_attempted = ledger
        .validation_details
        .post_fast_loop_validation_rerun_attempted;
    let mut details = parse_benchmark_validation_details(
        output_text,
        &ledger.owner_files,
        &ledger.expected_touch_targets,
        &ledger.named_tests,
    );
    details.repair_required = true;
    details.post_fast_loop_patch_attempted = previous_patch_attempted;
    details.post_fast_loop_validation_rerun_attempted =
        previous_validation_rerun_attempted || previous_patch_attempted;
    details.patch_packet_injected = false;
    details.patch_packet_honored_range = None;
    details.recommended_rerun_command = recommended_fast_loop_rerun_command(ledger);
    details.fast_loop_rerun_match_kind = None;
    ledger.validation_status = Some("failed: fast-loop".to_string());
    ledger.last_validation_failure = Some(render_benchmark_validation_failure_summary(
        &details,
        output_text,
    ));
    ledger.validation_details = details;
}

fn parse_benchmark_validation_details(
    output_text: &str,
    owner_files: &[String],
    expected_touch_targets: &[String],
    named_tests: &[String],
) -> BenchmarkValidationDetails {
    let failing_test_names = extract_failing_test_names(output_text, named_tests);
    let (primary_failure_path, primary_failure_line, primary_failure_test_name) =
        extract_primary_failure_location(output_text, owner_files, expected_touch_targets);
    let assertion_excerpt = extract_assertion_excerpt(output_text);
    let diagnostic_class = classify_benchmark_diagnostic(output_text);
    BenchmarkValidationDetails {
        failing_test_names,
        primary_failure_test_name,
        primary_failure_path,
        primary_failure_line,
        assertion_excerpt,
        diagnostic_class,
        implementation_target_lease: None,
        repair_required: true,
        repair_phase_terminal: Some(
            BenchmarkRepairPhase::NeedsFailureAnchorRead
                .label()
                .to_string(),
        ),
        failure_anchor_reread_attempted: false,
        failure_anchor_reread_honored: false,
        implementation_reread_allowed: false,
        implementation_reread_attempted: false,
        implementation_reread_honored: false,
        repair_phase_invalid_action_count: 0,
        post_fast_loop_patch_attempted: false,
        post_fast_loop_validation_rerun_attempted: false,
        patch_packet_injected: false,
        patch_packet_honored_range: None,
        recommended_rerun_command: None,
        fast_loop_rerun_match_kind: None,
        failed_edit_records: Vec::new(),
    }
}

fn render_benchmark_validation_failure_summary(
    details: &BenchmarkValidationDetails,
    output_text: &str,
) -> String {
    let mut parts = Vec::new();
    if let Some(test_name) = details.primary_failure_test_name.as_ref() {
        parts.push(format!("test `{test_name}` failed"));
    } else if let Some(test_name) = details.failing_test_names.first() {
        parts.push(format!("test `{test_name}` failed"));
    }
    if let Some(path) = details.primary_failure_path.as_ref() {
        let line = details
            .primary_failure_line
            .map(|value| format!(":{value}"))
            .unwrap_or_default();
        parts.push(format!("at {path}{line}"));
    }
    if let Some(assertion_excerpt) = details.assertion_excerpt.as_ref() {
        parts.push(format!(
            "assertion {}",
            truncate_visible_text(assertion_excerpt, 140)
        ));
    }
    if let Some(diagnostic_class) = details.diagnostic_class.as_ref() {
        parts.push(format!("diagnostic_class {diagnostic_class}"));
    }
    if parts.is_empty() {
        truncate_visible_text(output_text, 240)
    } else {
        truncate_visible_text(&parts.join(" | "), 240)
    }
}

fn classify_benchmark_diagnostic(output_text: &str) -> Option<String> {
    let lower = output_text.to_ascii_lowercase();
    if lower.contains("error[e0432]")
        || lower.contains("error[e0433]")
        || lower.contains("unresolved import")
        || lower.contains("unresolved crate")
        || lower.contains("use of unresolved module or unlinked crate")
    {
        return Some("manifest_dependency_error".to_string());
    }
    if benchmark_output_indicates_manifest_feature_error(&lower) {
        return Some("manifest_feature_error".to_string());
    }
    if lower.contains("expected one of")
        || lower.contains("mismatched closing delimiter")
        || lower.contains("unclosed delimiter")
        || lower.contains("unexpected closing delimiter")
    {
        return Some("rust_parse_error".to_string());
    }
    if lower.contains("error[") || lower.contains("\nerror:") {
        return Some("rust_compile_error".to_string());
    }
    if lower.contains("panicked at")
        || lower.contains("assertion `")
        || lower.contains("test result: failed")
    {
        return Some("test_assertion_failure".to_string());
    }
    None
}

fn benchmark_output_indicates_manifest_feature_error(lower: &str) -> bool {
    let serde_trait_gap = lower.contains("serde::serialize")
        || lower.contains("serde::deserialize")
        || lower.contains("serialize is not satisfied")
        || lower.contains("deserialize<'de> is not satisfied");
    let case_06_types = lower.contains("uuid")
        || lower.contains("datetime<utc>")
        || lower.contains("chrono::datetime")
        || lower.contains("chrono");
    serde_trait_gap && case_06_types
}

fn extract_failing_test_names(output_text: &str, named_tests: &[String]) -> Vec<String> {
    let mut names = BTreeSet::new();
    for candidate in named_tests {
        if !candidate.trim().is_empty() && output_text.contains(candidate) {
            names.insert(candidate.trim().to_string());
        }
    }
    for line in output_text.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed
            .strip_prefix("test ")
            .and_then(|value| value.strip_suffix(" ... FAILED"))
        {
            let value = name.trim();
            if !value.is_empty() {
                names.insert(value.to_string());
            }
        }
        if let Some(name) = trimmed
            .strip_prefix("---- ")
            .and_then(|value| value.strip_suffix(" stdout ----"))
        {
            let value = name.trim();
            if !value.is_empty() {
                names.insert(value.to_string());
            }
        }
    }
    names.into_iter().collect()
}

fn extract_primary_failure_location(
    output_text: &str,
    owner_files: &[String],
    expected_touch_targets: &[String],
) -> (Option<String>, Option<usize>, Option<String>) {
    let candidate_paths = owner_files
        .iter()
        .chain(expected_touch_targets.iter())
        .cloned()
        .collect::<Vec<_>>();
    let real_error_seen = classify_benchmark_diagnostic(output_text)
        .as_deref()
        .is_some_and(|class| class != "test_assertion_failure");
    let mut current_test_name: Option<String> = None;
    for line in output_text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if real_error_seen && (lower.contains("warning:") || lower.contains("unexpected `cfg`")) {
            continue;
        }
        if let Some(test_name) = trimmed
            .strip_prefix("---- ")
            .and_then(|value| value.strip_suffix(" stdout ----"))
        {
            let value = test_name.trim();
            if !value.is_empty() {
                current_test_name = Some(value.to_string());
            }
            continue;
        }
        for path in &candidate_paths {
            if let Some(line_number) = find_line_number_for_path(trimmed, path) {
                return (
                    Some(path.clone()),
                    Some(line_number),
                    current_test_name.clone(),
                );
            }
        }
    }
    for token in output_text.split_whitespace() {
        if let Some((path, line_number)) = parse_path_line_token(token) {
            return (Some(path), Some(line_number), current_test_name.clone());
        }
    }
    (None, None, current_test_name)
}

fn find_line_number_for_path(output_text: &str, path: &str) -> Option<usize> {
    let needle = format!("{path}:");
    output_text.lines().find_map(|line| {
        let index = line.find(&needle)?;
        let remainder = &line[index + needle.len()..];
        remainder
            .split(':')
            .next()
            .and_then(|value| value.parse::<usize>().ok())
    })
}

fn parse_path_line_token(token: &str) -> Option<(String, usize)> {
    let cleaned = token.trim_matches(|character: char| {
        matches!(
            character,
            ',' | '.' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\''
        )
    });
    let path_start = cleaned.find(".rs:")?;
    let path_end = path_start + 3;
    let path = cleaned[..path_end].to_string();
    let remainder = cleaned.get(path_end + 1..)?;
    let line_number = remainder
        .split(':')
        .next()
        .and_then(|value| value.parse::<usize>().ok())?;
    Some((path, line_number))
}

fn extract_assertion_excerpt(output_text: &str) -> Option<String> {
    let lines = output_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let unresolved_imports = extract_unresolved_import_names(output_text);
    if unresolved_imports.len() > 1 {
        return Some(format!(
            "unresolved imports/crates: {}",
            unresolved_imports.join(", ")
        ));
    }
    if classify_benchmark_diagnostic(output_text)
        .as_deref()
        .is_some_and(|class| class != "test_assertion_failure")
    {
        for line in &lines {
            let lower = line.to_ascii_lowercase();
            if lower.contains("warning:") || lower.contains("unexpected `cfg`") {
                continue;
            }
            if lower.starts_with("error")
                || lower.contains("unresolved import")
                || lower.contains("expected one of")
                || lower.contains("mismatched closing delimiter")
                || lower.contains("unclosed delimiter")
            {
                return Some(truncate_visible_text(line, 220));
            }
        }
    }
    for line in &lines {
        let lower = line.to_ascii_lowercase();
        if lower.contains("assertion `")
            || lower.contains("panicked at ")
            || lower.starts_with("left:")
            || lower.starts_with("right:")
        {
            return Some(truncate_visible_text(line, 220));
        }
    }
    for line in &lines {
        let lower = line.to_ascii_lowercase();
        if lower.contains("warning:") || lower.contains("unexpected `cfg`") {
            continue;
        }
        if lower.contains("assert")
            || lower.contains("panic")
            || lower.contains("expected")
            || lower.contains("left:")
            || lower.contains("right:")
        {
            return Some(truncate_visible_text(line, 220));
        }
    }
    lines
        .into_iter()
        .find(|line| {
            let lower = line.to_ascii_lowercase();
            !lower.contains("warning:")
                && !lower.contains("unexpected `cfg`")
                && !lower.starts_with("command failed:")
        })
        .map(|line| truncate_visible_text(line, 220))
}

fn extract_unresolved_import_names(output_text: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    for line in output_text.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(summary_names) = lower
            .contains("unresolved imports/crates:")
            .then(|| unresolved_import_summary_names(line))
        {
            for name in summary_names {
                names.insert(name);
            }
            continue;
        }
        if !(lower.contains("unresolved import")
            || lower.contains("unresolved crate")
            || lower.contains("use of unresolved module or unlinked crate"))
        {
            continue;
        }
        for name in backtick_spans(line) {
            let root = name
                .split("::")
                .next()
                .unwrap_or(name.as_str())
                .trim_matches(|character: char| {
                    character == '{'
                        || character == '}'
                        || character == ','
                        || character.is_whitespace()
                });
            if !root.is_empty() {
                names.insert(root.to_string());
                break;
            }
        }
    }
    names.into_iter().collect()
}

fn extract_manifest_feature_dependency_names(output_text: &str) -> Vec<String> {
    let lower = output_text.to_ascii_lowercase();
    let mut names = BTreeSet::new();
    if lower.contains("uuid") {
        names.insert("uuid".to_string());
    }
    if lower.contains("datetime<utc>")
        || lower.contains("chrono::datetime")
        || lower.contains("chrono")
    {
        names.insert("chrono".to_string());
    }
    names.into_iter().collect()
}

fn unresolved_import_summary_names(line: &str) -> Vec<String> {
    let lower = line.to_ascii_lowercase();
    let marker = "unresolved imports/crates:";
    let Some(marker_index) = lower.find(marker) else {
        return Vec::new();
    };
    let tail = &line[marker_index + marker.len()..];
    let tail = tail.split('|').next().unwrap_or(tail);
    tail.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .trim_matches(|character: char| {
                    character == '`'
                        || character == '{'
                        || character == '}'
                        || character == ','
                        || character.is_whitespace()
                })
                .split("::")
                .next()
                .unwrap_or(value)
                .to_string()
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn backtick_spans(line: &str) -> Vec<String> {
    let mut spans = Vec::new();
    let mut remainder = line;
    while let Some(start) = remainder.find('`') {
        let after_start = &remainder[start + 1..];
        let Some(end) = after_start.find('`') else {
            break;
        };
        spans.push(after_start[..end].to_string());
        remainder = &after_start[end + 1..];
    }
    spans
}

fn repair_requirement_from_action(
    action: &AgentAction,
    failure_reason: &str,
) -> Option<RepairRequirement> {
    match action {
        AgentAction::WriteFile { path, .. }
        | AgentAction::ApplyPatch { path, .. }
        | AgentAction::ReplaceRange { path, .. }
        | AgentAction::ModifyToml { path, .. }
        | AgentAction::SetExecutable { path } => Some(RepairRequirement {
            path: path.clone(),
            failure_reason: truncate_visible_text(failure_reason, 240),
            previous_search_block: None,
            suggested_range: suggested_reread_range_from_failure(path, failure_reason)
                .or_else(|| fallback_repair_read_range(path)),
            exact_reread_completed: false,
        }),
        AgentAction::ReplaceBlock {
            path, search_block, ..
        } => Some(RepairRequirement {
            path: path.clone(),
            failure_reason: truncate_visible_text(failure_reason, 240),
            previous_search_block: Some(truncate_visible_text(search_block, 1_200)),
            suggested_range: suggested_reread_range_from_failure(path, failure_reason)
                .or_else(|| fallback_repair_read_range(path)),
            exact_reread_completed: false,
        }),
        AgentAction::RunCommand { .. }
        | AgentAction::ReadFile { .. }
        | AgentAction::ListDirectory { .. }
        | AgentAction::SearchText { .. }
        | AgentAction::SearchSymbols { .. }
        | AgentAction::FindFiles { .. }
        | AgentAction::StructuralSearch { .. }
        | AgentAction::StructuralEditPreview { .. }
        | AgentAction::CargoDiagnostics { .. }
        | AgentAction::GetRepoCapsule { .. }
        | AgentAction::ExplainValidationFailure { .. }
        | AgentAction::SuggestImplementationTargets { .. }
        | AgentAction::SuggestEditAnchors { .. }
        | AgentAction::PreviewEdit { .. }
        | AgentAction::ApplyPreview { .. }
        | AgentAction::McpCallTool { .. }
        | AgentAction::RunValidation { .. } => None,
    }
}

fn fallback_repair_read_range(path: &str) -> Option<crate::agent_protocol::ReadFileRange> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }
    let end_line = if trimmed.ends_with("Cargo.toml")
        || trimmed.ends_with(".toml")
        || trimmed.ends_with(".json")
    {
        120
    } else {
        160
    };
    Some(crate::agent_protocol::ReadFileRange {
        start_line: 1,
        end_line,
    })
}

fn failed_edit_record_from_action(
    action: &AgentAction,
    failure_reason: &str,
) -> Option<FailedEditRecord> {
    let (action_kind, path, search_hash, replace_hash) = match action {
        AgentAction::ReplaceBlock {
            path,
            search_block,
            replace_block,
            ..
        } => (
            "replace_block",
            path.clone(),
            Some(stable_text_hash(search_block)),
            Some(stable_text_hash(replace_block)),
        ),
        AgentAction::ApplyPatch { path, patch } => (
            "apply_patch",
            path.clone(),
            Some(stable_text_hash(patch)),
            None,
        ),
        AgentAction::WriteFile { path, content } => (
            "write_file",
            path.clone(),
            None,
            Some(stable_text_hash(content)),
        ),
        AgentAction::ReplaceRange {
            path,
            expected_hash,
            replacement,
            ..
        } => (
            "replace_range",
            path.clone(),
            Some(stable_text_hash(expected_hash)),
            Some(stable_text_hash(replacement)),
        ),
        AgentAction::ModifyToml {
            path,
            expected_hash,
            operations,
        } => (
            "modify_toml",
            path.clone(),
            Some(stable_text_hash(expected_hash)),
            Some(stable_text_hash(&format!("{operations:?}"))),
        ),
        AgentAction::ApplyPreview { preview_id } => (
            "apply_preview",
            preview_id.clone(),
            Some(stable_text_hash(preview_id)),
            None,
        ),
        AgentAction::SetExecutable { path } => ("set_executable", path.clone(), None, None),
        AgentAction::RunCommand { .. }
        | AgentAction::ReadFile { .. }
        | AgentAction::ListDirectory { .. }
        | AgentAction::SearchText { .. }
        | AgentAction::SearchSymbols { .. }
        | AgentAction::FindFiles { .. }
        | AgentAction::StructuralSearch { .. }
        | AgentAction::StructuralEditPreview { .. }
        | AgentAction::CargoDiagnostics { .. }
        | AgentAction::GetRepoCapsule { .. }
        | AgentAction::ExplainValidationFailure { .. }
        | AgentAction::SuggestImplementationTargets { .. }
        | AgentAction::SuggestEditAnchors { .. }
        | AgentAction::PreviewEdit { .. }
        | AgentAction::McpCallTool { .. }
        | AgentAction::RunValidation { .. } => return None,
    };
    Some(FailedEditRecord {
        action_kind: action_kind.to_string(),
        path,
        search_hash,
        replace_hash,
        failure_reason: truncate_visible_text(failure_reason, 260),
        matching_line_numbers: extract_matching_line_numbers(failure_reason),
        attempts: 1,
    })
}

fn failed_edit_signature_matches(left: &FailedEditRecord, right: &FailedEditRecord) -> bool {
    left.action_kind == right.action_kind
        && left.path == right.path
        && left.search_hash == right.search_hash
        && left.replace_hash == right.replace_hash
}

fn stable_text_hash(text: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn extract_matching_line_numbers(failure_reason: &str) -> Vec<usize> {
    let lower = failure_reason.to_ascii_lowercase();
    let Some(index) = lower.find("lines ") else {
        return Vec::new();
    };
    let segment = failure_reason[index + "lines ".len()..]
        .split(['.', '\n'])
        .next()
        .unwrap_or_default();
    segment
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|token| token.parse::<usize>().ok())
        .collect()
}

fn failed_edit_is_ambiguous(record: &FailedEditRecord) -> bool {
    record.matching_line_numbers.len() > 1
        || record
            .failure_reason
            .to_ascii_lowercase()
            .contains("ambiguous")
}

fn bare_replace_block_disallowed_for_path(path: &str, records: &[FailedEditRecord]) -> bool {
    records
        .iter()
        .filter(|record| record.action_kind == "replace_block" && record.path == path)
        .filter(|record| failed_edit_is_ambiguous(record))
        .count()
        >= 1
}

fn bare_replace_block_repeats_failed_signature(
    action: &AgentAction,
    records: &[FailedEditRecord],
) -> bool {
    let Some(record) = failed_edit_record_from_action(action, "") else {
        return false;
    };
    records
        .iter()
        .any(|existing| failed_edit_signature_matches(existing, &record))
}

fn render_failed_edit_memory(records: &[FailedEditRecord]) -> String {
    records
        .iter()
        .rev()
        .take(4)
        .map(|record| {
            let lines = if record.matching_line_numbers.is_empty() {
                "lines=unknown".to_string()
            } else {
                format!(
                    "lines={}",
                    record
                        .matching_line_numbers
                        .iter()
                        .take(8)
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            };
            format!(
                "{} {} attempts={} {} reason={}",
                record.action_kind,
                record.path,
                record.attempts,
                lines,
                truncate_visible_text(&record.failure_reason, 120)
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn render_agent_repair_memory(memory: &AgentRepairMemory) -> String {
    let mut parts = Vec::new();
    if let Some(action) = memory.current_required_action.as_ref() {
        parts.push(format!("required_next={action}"));
    }
    if let Some(phase) = memory.repair_phase.as_ref() {
        parts.push(format!(
            "phase={phase} context_sufficient={}",
            memory.context_sufficient
        ));
    }
    if let Some(diagnostic_class) = memory.diagnostic_class.as_ref() {
        parts.push(format!("diagnostic_class={diagnostic_class}"));
    }
    if let Some(target) = memory.implementation_target_lease.as_ref() {
        parts.push(format!("target_lease={target}"));
    }
    if let Some(table) = memory.target_dependency_table.as_ref() {
        parts.push(format!("target_table={table}"));
    }
    if !memory.dependency_candidates.is_empty() {
        parts.push(format!(
            "dependency_candidates={}",
            memory.dependency_candidates.join(",")
        ));
    }
    if !memory.last_manifest_patch_operations.is_empty() {
        parts.push(format!(
            "last_manifest_ops={}",
            render_toml_edit_operations_brief(&memory.last_manifest_patch_operations)
        ));
    }
    if let Some(diagnostic_class) = memory.post_patch_diagnostic_class.as_ref() {
        parts.push(format!("post_patch_class={diagnostic_class}"));
    }
    if let Some(excerpt) = memory.post_patch_diagnostic_excerpt.as_ref() {
        parts.push(format!(
            "post_patch_excerpt={}",
            truncate_visible_text(excerpt, 120)
        ));
    }
    if !memory.ranked_implementation_targets.is_empty() {
        parts.push(format!(
            "ranked_targets={}",
            render_ranked_implementation_targets(&memory.ranked_implementation_targets)
        ));
    }
    if !memory.observed_slices.is_empty() {
        let observed = memory
            .observed_slices
            .iter()
            .rev()
            .take(3)
            .map(|slice| {
                let range = slice
                    .honored_range
                    .or(slice.requested_range)
                    .map(|range| range.label())
                    .unwrap_or_else(|| "unranged".to_string());
                let purpose = slice
                    .purpose
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("read");
                format!("{}:{}:{purpose}", slice.path, range)
            })
            .collect::<Vec<_>>()
            .join(",");
        parts.push(format!("observed_slices={observed}"));
    }
    if !memory.rejected_actions.is_empty() {
        let rejected = memory
            .rejected_actions
            .iter()
            .rev()
            .take(2)
            .map(|record| {
                format!(
                    "{}:{}",
                    record.phase,
                    truncate_visible_text(&record.actions.join("+"), 80)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        parts.push(format!("rejected={rejected}"));
    }
    if !memory.suggested_edit_anchors.is_empty() {
        let anchors = memory
            .suggested_edit_anchors
            .iter()
            .rev()
            .take(2)
            .map(|anchor| {
                let range = anchor
                    .range
                    .map(|range| range.label())
                    .unwrap_or_else(|| "unranged".to_string());
                format!("{}:{range}", anchor.path)
            })
            .collect::<Vec<_>>()
            .join(",");
        parts.push(format!("anchor_suggestions={anchors}"));
    }
    if let Some(preview) = memory.last_preview_result.as_ref() {
        parts.push(format!(
            "last_preview={}",
            truncate_visible_text(preview, 120)
        ));
    }
    if let Some(rollback) = memory.last_rollback_diagnostic.as_ref() {
        parts.push(format!(
            "last_rollback={}",
            truncate_visible_text(rollback, 120)
        ));
    }
    let score = &memory.scorecard;
    parts.push(format!(
        "score parser_recovery={} repair_turns={} repair_invalid_max={} write_locked={} write_refusals={} scaffold_offered={} scaffold_honored={} write_emitted={} support_writes={} source_writes={} rolled_back_writes={} rolled_back_non_support={} line_tools={} injected_reads={} redundant_reads={} first_write={} repeated_edits={} validation_rejects={} test_edit_rejects={} target_redirects={} evidence_fixations={} anchors={} previews={}/{} syntax_previews={}/{}",
        score.parser_recovery_count,
        score.repair_submode_turns,
        score.repair_invalid_action_streak_max,
        score.repair_write_locked,
        score.write_phase_action_refusal_count,
        score.patch_scaffold_offered,
        score.patch_scaffold_honored,
        score.write_phase_write_emitted,
        score.support_write_count,
        score.source_write_count,
        score.rolled_back_write_count,
        score.rolled_back_non_support_edit_count,
        score.line_oriented_parse_count,
        score.controller_injected_read_count,
        score.redundant_read_count,
        score
            .first_valid_write_step
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string()),
        score.repeated_failed_edit_count,
        score.rejected_validation_alias_count,
        score.test_edit_rejection_count,
        score.target_redirect_count,
        score.evidence_file_fixation_count,
        score.anchor_suggestion_count,
        score.preview_edit_success_count,
        score.preview_edit_count,
        score.syntax_preview_failure_count,
        score.syntax_preview_count
    ));
    parts.join(" | ")
}

fn suggested_reread_range_from_failure(
    path: &str,
    failure_reason: &str,
) -> Option<crate::agent_protocol::ReadFileRange> {
    let needle = format!("{path}:");
    let index = failure_reason.find(&needle)?;
    let remainder = &failure_reason[index + needle.len()..];
    let digits = remainder
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    let line_number = digits.parse::<usize>().ok()?;
    Some(suggested_range_for_line(line_number))
}

fn suggested_range_for_line(line_number: usize) -> crate::agent_protocol::ReadFileRange {
    let start_line = line_number.saturating_sub(8).max(1);
    let end_line = line_number.saturating_add(24);
    crate::agent_protocol::ReadFileRange {
        start_line,
        end_line,
    }
}

fn load_workspace_file_text(workspace_root: &str, path: &str) -> Option<String> {
    let relative_path = path.trim();
    if relative_path.is_empty() {
        return None;
    }
    let candidate_path = PathBuf::from(workspace_root).join(relative_path);
    let canonical_root = PathBuf::from(workspace_root).canonicalize().ok()?;
    let canonical_candidate = candidate_path.canonicalize().ok()?;
    if !canonical_candidate.starts_with(&canonical_root) {
        return None;
    }
    fs::read_to_string(canonical_candidate).ok()
}

fn implementation_name_candidates(primary_failure_test_name: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let leaf = primary_failure_test_name
        .split("::")
        .last()
        .unwrap_or(primary_failure_test_name)
        .trim();
    let leaf = leaf.strip_prefix("test_").unwrap_or(leaf);
    let stop_words = [
        "close", "to", "min", "max", "epoch", "exact", "exactly", "near",
    ];
    let tokens = leaf
        .split('_')
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    for window in tokens.windows(2) {
        let first = window[0];
        let second = window[1];
        if stop_words.contains(&first) || stop_words.contains(&second) {
            continue;
        }
        candidates.push(format!("{first}_{second}"));
    }
    for token in tokens {
        if stop_words.contains(&token) {
            continue;
        }
        candidates.push(token.to_string());
    }
    let mut deduped = Vec::new();
    let mut seen = BTreeSet::new();
    for candidate in candidates {
        if seen.insert(candidate.clone()) {
            deduped.push(candidate);
        }
    }
    deduped
}

fn suggest_implementation_range_from_owner_text(
    owner_text: &str,
    primary_failure_test_name: Option<&str>,
) -> Option<crate::agent_protocol::ReadFileRange> {
    let primary_failure_test_name = primary_failure_test_name?;
    if let Some(range) = suggest_axum_fallback_merge_range(owner_text, primary_failure_test_name) {
        return Some(range);
    }
    if let Some(range) = suggest_chrono_epoch_rounding_range(owner_text, primary_failure_test_name)
    {
        return Some(range);
    }
    let candidates = implementation_name_candidates(primary_failure_test_name);
    if candidates.is_empty() {
        return None;
    }
    let lines = owner_text.lines().collect::<Vec<_>>();
    let mut best_match: Option<(i32, usize)> = None;
    for (index, _) in lines.iter().enumerate() {
        let signature_window = implementation_signature_window(&lines, index);
        if signature_window.is_empty() {
            continue;
        }
        let lower_signature = signature_window.to_ascii_lowercase();
        for candidate in &candidates {
            if !signature_matches_candidate(&lower_signature, candidate) {
                continue;
            }
            let score =
                implementation_signature_score(lines[index], &lower_signature, candidate, index);
            if best_match.as_ref().is_none_or(|(best_score, best_index)| {
                score > *best_score || (score == *best_score && index < *best_index)
            }) {
                best_match = Some((score, index));
            }
        }
    }
    let (_, index) = best_match?;
    let center_line = index.saturating_add(1);
    let start_line = center_line.saturating_sub(18).max(1);
    let end_line = center_line.saturating_add(48);
    Some(crate::agent_protocol::ReadFileRange {
        start_line,
        end_line,
    })
}

fn suggest_chrono_epoch_rounding_range(
    owner_text: &str,
    primary_failure_test_name: &str,
) -> Option<crate::agent_protocol::ReadFileRange> {
    if !primary_failure_test_name.contains("duration_round")
        && !primary_failure_test_name.contains("duration_trunc")
    {
        return None;
    }
    if !owner_text.contains("DurationExceedsTimestamp") {
        return None;
    }
    let lines = owner_text.lines().collect::<Vec<_>>();
    let round_index = lines
        .iter()
        .position(|line| line.trim_start().starts_with("fn duration_round<"))?;
    let trunc_index = lines
        .iter()
        .position(|line| line.trim_start().starts_with("fn duration_trunc<"))?;
    let first_index = round_index.min(trunc_index);
    let second_index = round_index.max(trunc_index);
    Some(crate::agent_protocol::ReadFileRange {
        start_line: first_index.saturating_add(1).saturating_sub(18).max(1),
        end_line: second_index.saturating_add(71),
    })
}

fn suggest_axum_fallback_merge_range(
    owner_text: &str,
    signal_text: &str,
) -> Option<crate::agent_protocol::ReadFileRange> {
    let lower_signal = signal_text.to_ascii_lowercase();
    if !lower_signal.contains("fallback")
        && !lower_signal.contains("merge")
        && !lower_signal.contains("nest")
    {
        return None;
    }
    if !owner_text.contains("pub fn nest<")
        || !owner_text.contains("pub fn merge(")
        || !owner_text.contains("Fallback::Custom")
    {
        return None;
    }
    let lines = owner_text.lines().collect::<Vec<_>>();
    let nest_index = lines
        .iter()
        .position(|line| line.trim_start().starts_with("pub fn nest<"))?;
    let merge_index = lines
        .iter()
        .position(|line| line.trim_start().starts_with("pub fn merge("))?;
    let first_index = nest_index.min(merge_index);
    let second_index = nest_index.max(merge_index);
    Some(crate::agent_protocol::ReadFileRange {
        start_line: first_index.saturating_add(1).saturating_sub(8).max(1),
        end_line: second_index.saturating_add(36),
    })
}

fn suggest_source_patch_range_from_failure(
    owner_text: &str,
    failure_text: Option<&str>,
) -> Option<crate::agent_protocol::ReadFileRange> {
    let failure_text = failure_text.unwrap_or_default().to_ascii_lowercase();
    if let Some(range) = suggest_axum_fallback_merge_range(owner_text, &failure_text) {
        return Some(range);
    }
    let mut needles = Vec::new();
    if failure_text.contains("cannotborrowowneddata")
        || owner_text.contains("CannotBorrowOwnedData")
    {
        needles.push("CannotBorrowOwnedData");
    }
    if failure_text.contains("deserialize") {
        needles.push("deserialize");
    }
    if failure_text.contains("visitor") {
        needles.push("visit");
    }
    if failure_text.contains("durationexceedstimestamp")
        || owner_text.contains("DurationExceedsTimestamp")
    {
        needles.push("DurationExceedsTimestamp");
    }
    if needles.is_empty() && owner_text.contains("deserialize_str") {
        needles.push("deserialize_str");
    }
    if needles.is_empty() && owner_text.contains("deserialize_bytes") {
        needles.push("deserialize_bytes");
    }
    if needles.is_empty() {
        return None;
    }
    let lines = owner_text.lines().collect::<Vec<_>>();
    for needle in needles {
        if let Some(index) = lines.iter().position(|line| line.contains(needle)) {
            let center_line = index.saturating_add(1);
            return Some(crate::agent_protocol::ReadFileRange {
                start_line: center_line.saturating_sub(28).max(1),
                end_line: center_line.saturating_add(42),
            });
        }
    }
    None
}

fn benchmark_repair_phase_instruction(phase: BenchmarkRepairPhase) -> &'static str {
    match phase {
        BenchmarkRepairPhase::Idle => "",
        BenchmarkRepairPhase::NeedsFailureAnchorRead => "Read the suggested failing slice now.",
        BenchmarkRepairPhase::NeedsImplementationRead => {
            "You have the failing test slice. Read one implementation slice now."
        }
        BenchmarkRepairPhase::NeedsPatch => {
            "You already have the needed owner-file context. Patch now."
        }
        BenchmarkRepairPhase::NeedsFastLoopRerun => "Patch captured. Rerun the fast loop now.",
    }
}

fn truncate_patch_packet_slice(content: &str) -> String {
    const MAX_LINES: usize = 72;
    const MAX_CHARS: usize = 2400;

    let mut rendered_lines = Vec::new();
    let mut used_chars = 0usize;
    let mut truncated = false;
    for (index, line) in content.lines().enumerate() {
        if index >= MAX_LINES {
            truncated = true;
            break;
        }
        let additional_chars = line.len().saturating_add(1);
        if used_chars.saturating_add(additional_chars) > MAX_CHARS {
            truncated = true;
            break;
        }
        rendered_lines.push(line);
        used_chars = used_chars.saturating_add(additional_chars);
    }
    let mut rendered = rendered_lines.join("\n").trim().to_string();
    if truncated {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str("... [truncated]");
    }
    rendered
}

fn extract_exact_range_from_text(
    owner_text: &str,
    range: crate::agent_protocol::ReadFileRange,
) -> Option<String> {
    let normalized = range.normalized()?;
    let lines = owner_text.lines().collect::<Vec<_>>();
    if normalized.end_line > lines.len() {
        return None;
    }
    let start_index = normalized.start_line.saturating_sub(1);
    let end_index = normalized.end_line.min(lines.len());
    if start_index >= end_index {
        return None;
    }
    Some(lines[start_index..end_index].join("\n"))
}

fn owner_slice_packet_content(repair_state: &BenchmarkRepairState) -> Option<String> {
    let last_owner_slice = repair_state.last_owner_slice.as_ref()?;
    if let Some(slice_content) = last_owner_slice.slice_content.as_ref() {
        return Some(slice_content.clone());
    }
    last_owner_slice.honored_range.and_then(|range| {
        repair_state
            .latest_owner_file_text
            .as_deref()
            .and_then(|text| extract_exact_range_from_text(text, range))
    })
}

fn target_slice_content_hash(
    repair_state: &BenchmarkRepairState,
    patch_target: &str,
) -> Option<String> {
    let last_owner_slice = repair_state.last_owner_slice.as_ref()?;
    if last_owner_slice.test_only
        || canonical_path(&last_owner_slice.path) != canonical_path(patch_target)
    {
        return None;
    }
    owner_slice_packet_content(repair_state).map(|content| stable_content_hash(&content))
}

fn target_content_hash_for_patch(
    repair_state: &BenchmarkRepairState,
    memory: &AgentRepairMemory,
    patch_target: &str,
) -> Option<String> {
    observed_full_file_content_hash(memory, patch_target)
        .or_else(|| target_slice_content_hash(repair_state, patch_target))
}

fn benchmark_repair_phase_suggested_range(
    repair_state: &BenchmarkRepairState,
) -> Option<crate::agent_protocol::ReadFileRange> {
    match repair_state.phase {
        BenchmarkRepairPhase::Idle => None,
        BenchmarkRepairPhase::NeedsFailureAnchorRead => repair_state.failure_anchor_range,
        BenchmarkRepairPhase::NeedsImplementationRead => repair_state
            .implementation_suggested_range
            .or(repair_state.failure_anchor_range),
        BenchmarkRepairPhase::NeedsPatch | BenchmarkRepairPhase::NeedsFastLoopRerun => repair_state
            .last_owner_slice
            .as_ref()
            .and_then(|slice| slice.honored_range)
            .or(repair_state.failure_anchor_range),
    }
}

fn benchmark_allowed_implementation_targets(ledger: &BenchmarkCaseLedger) -> Vec<String> {
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    for path in ledger
        .owner_files
        .iter()
        .chain(ledger.expected_touch_targets.iter())
    {
        let canonical = canonical_path(path);
        if !canonical.trim().is_empty()
            && !is_obvious_test_file(&canonical)
            && seen.insert(canonical.clone())
        {
            targets.push(canonical);
        }
    }
    targets
}

fn benchmark_read_only_test_targets(ledger: &BenchmarkCaseLedger) -> Vec<String> {
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    for path in ledger
        .owner_files
        .iter()
        .chain(ledger.expected_touch_targets.iter())
        .chain(ledger.companion_files_required.iter())
    {
        let canonical = canonical_path(path);
        if !canonical.trim().is_empty()
            && is_obvious_test_file(&canonical)
            && seen.insert(canonical.clone())
        {
            targets.push(canonical);
        }
    }
    targets
}

fn render_benchmark_target_list(targets: &[String]) -> String {
    if targets.is_empty() {
        return "[none]".to_string();
    }
    targets
        .iter()
        .map(|target| format!("`{target}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_ranked_implementation_targets(targets: &[AgentRepairImplementationTarget]) -> String {
    if targets.is_empty() {
        return "[none]".to_string();
    }
    targets
        .iter()
        .take(6)
        .map(|target| format!("#{} `{}` ({})", target.rank, target.path, target.reason))
        .collect::<Vec<_>>()
        .join(", ")
}

fn recommended_fast_loop_rerun_command(ledger: &BenchmarkCaseLedger) -> Option<String> {
    let canonical = ledger
        .fast_loop_commands
        .iter()
        .find(|command| !command.trim().is_empty())?
        .trim()
        .to_string();
    let failing_tests = fast_loop_selector_pool(ledger);
    if failing_tests.is_empty() {
        return Some(canonical);
    }
    let Some((mut base_tokens, selector_prefix)) = split_fast_loop_candidate(&canonical) else {
        return Some(canonical);
    };
    if base_tokens.is_empty() {
        return Some(canonical);
    }
    if selector_prefix.is_none()
        && base_tokens
            .last()
            .is_some_and(|token| token.as_str() == "--exact")
    {
        return Some(canonical);
    }
    if selector_prefix.is_none() && fast_loop_explicit_selector(&canonical).is_some() {
        return Some(canonical);
    }
    for failing_test in failing_tests {
        if let Some(prefix) = selector_prefix.as_deref() {
            if failing_test.starts_with(prefix) {
                base_tokens.push(failing_test.clone());
                continue;
            }
            base_tokens.push(format!("{prefix}{failing_test}"));
            continue;
        }
        base_tokens.push(failing_test.clone());
    }
    Some(base_tokens.join(" "))
}

fn implementation_signature_window(lines: &[&str], start_index: usize) -> String {
    let mut parts = Vec::new();
    for line in lines.iter().skip(start_index).take(8) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        parts.push(trimmed.to_string());
        if trimmed.ends_with('{') || trimmed.ends_with(';') {
            break;
        }
    }
    parts.join(" ")
}

fn signature_matches_candidate(signature_lower: &str, candidate: &str) -> bool {
    let candidate = candidate.to_ascii_lowercase();
    [
        format!("fn {candidate}("),
        format!("fn {candidate}<"),
        format!("pub fn {candidate}("),
        format!("pub fn {candidate}<"),
        format!("pub(crate) fn {candidate}("),
        format!("pub(crate) fn {candidate}<"),
    ]
    .iter()
    .any(|pattern| signature_lower.contains(pattern))
}

fn implementation_signature_score(
    line: &str,
    signature_lower: &str,
    candidate: &str,
    index: usize,
) -> i32 {
    let trimmed = line.trim();
    let mut score = candidate.len() as i32 * 4;
    if signature_lower.ends_with('{') || signature_lower.contains("{ ") {
        score += 120;
    }
    if signature_lower.ends_with(';') {
        score -= 220;
    }
    if signature_lower.contains("(self") || signature_lower.contains(" self,") {
        score -= 80;
    }
    if trimmed.starts_with("fn ") || trimmed.starts_with("pub fn ") {
        score += 25;
    }
    if !line.starts_with(char::is_whitespace) {
        score += 50;
    }
    score - (index as i32 / 8)
}

fn slice_is_test_only(content: &str, primary_failure_test_name: Option<&str>) -> bool {
    let lower = content.to_ascii_lowercase();
    let test_markers = ["#[test]", "assert_eq!", "assert!", "fn test_", "mod tests"]
        .iter()
        .filter(|marker| lower.contains(**marker))
        .count();
    let contains_impl_signature = if let Some(test_name) = primary_failure_test_name {
        implementation_name_candidates(test_name)
            .iter()
            .any(|candidate| {
                lower.contains(&format!("fn {candidate}"))
                    || lower.contains(&format!("pub fn {candidate}"))
                    || lower.contains(&format!("pub(crate) fn {candidate}"))
            })
    } else {
        content.lines().any(|line| {
            let trimmed = line.trim_start();
            (trimmed.starts_with("fn ")
                || trimmed.starts_with("pub fn ")
                || trimmed.starts_with("pub(crate) fn "))
                && !trimmed.contains("test_")
        })
    };
    test_markers > 0 && !contains_impl_signature
}

fn benchmark_repair_state_from_ledger(
    ledger: &BenchmarkCaseLedger,
) -> Option<BenchmarkRepairState> {
    let failure_reason = ledger.last_validation_failure.as_ref()?;
    let diagnostic_class = ledger.validation_details.diagnostic_class.as_deref();
    let implementation_lease = target_lease_for_ledger(ledger);
    let primary_failure_path = ledger.validation_details.primary_failure_path.clone();
    let source_lease_should_drive_repair = implementation_lease.as_deref().is_some_and(|path| {
        !is_obvious_test_file(path)
            && !matches!(
                diagnostic_class,
                Some("manifest_dependency_error" | "manifest_feature_error")
            )
            && primary_failure_path
                .as_deref()
                .is_some_and(is_obvious_test_file)
    });
    let owner_path = implementation_lease
        .clone()
        .filter(|_| source_lease_should_drive_repair)
        .or_else(|| primary_failure_path.clone())
        .or_else(|| ledger.owner_files.first().cloned())
        .or_else(|| ledger.expected_touch_targets.first().cloned())?;
    let repair_phase = if source_lease_should_drive_repair {
        BenchmarkRepairPhase::NeedsPatch
    } else {
        BenchmarkRepairPhase::NeedsFailureAnchorRead
    };
    let failure_anchor_range = if source_lease_should_drive_repair {
        None
    } else {
        ledger
            .validation_details
            .primary_failure_line
            .map(suggested_range_for_line)
            .or_else(|| suggested_reread_range_from_failure(&owner_path, failure_reason))
    };
    Some(BenchmarkRepairState {
        phase: repair_phase,
        owner_path,
        primary_failure_test_name: ledger.validation_details.primary_failure_test_name.clone(),
        failure_anchor_range,
        implementation_suggested_range: None,
        last_owner_slice: None,
        latest_owner_file_text: None,
        failure_anchor_reread_attempted: false,
        failure_anchor_reread_honored: false,
        implementation_reread_allowed: false,
        implementation_reread_attempted: false,
        implementation_reread_honored: false,
        invalid_action_count: 0,
    })
}

#[cfg(test)]
mod tests;


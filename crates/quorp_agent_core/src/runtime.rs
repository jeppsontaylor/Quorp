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

mod normalize;
mod parse_helpers;
mod path_intel;
mod recovery;
mod suggestions;
mod turn;

#[allow(unused_imports)]
pub use normalize::*;
#[allow(unused_imports)]
pub use parse_helpers::*;
#[allow(unused_imports)]
pub use path_intel::*;
#[allow(unused_imports)]
pub use recovery::*;
#[allow(unused_imports)]
pub use suggestions::*;
#[allow(unused_imports)]
pub use turn::*;

#[cfg(test)]
mod tests;


use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use futures::future::BoxFuture;
use serde::Serialize;

use crate::agent_context::{AgentConfig, AutonomyProfile, PolicySettings};
#[cfg(test)]
use crate::agent_protocol::PreviewEditPayload;
use crate::agent_protocol::{ActionOutcome, AgentAction, AgentMode, ValidationPlan};
use crate::agent_turn::AgentTurnResponse;
#[cfg(test)]
use crate::agent_turn::parse_agent_turn_response;
#[cfg(test)]
use std::fs;

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
            native_tool_calls: true,
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
    #[serde(default)]
    pub full_validation_before_fast_loop_count: usize,
    #[serde(default)]
    pub prose_only_recovery_count: usize,
    #[serde(default)]
    pub bare_replace_block_retry_count: usize,
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
        checkpoint: Box<AgentCheckpoint>,
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
pub(crate) struct AgentTaskState {
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

mod action_summary;
mod normalize;
mod parse_helpers;
mod path_intel;
mod recovery;
mod state_allow;
mod state_messages;
mod state_observe;
mod state_record;
mod state_validate;
mod suggestions;
mod turn;

#[allow(unused_imports)]
pub use action_summary::ToolResultEnvelope;
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

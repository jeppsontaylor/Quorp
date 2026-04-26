//! `AgentTaskState`: terminal-state and action-allowance gates
//! (`can_finish_without_more_actions`, `allow_action`,
//! `allow_action_for_benchmark_policy`, write-target restriction
//! checks).

#![allow(dead_code, unused_imports)]

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

use super::*;
use crate::agent_context::{
    AgentConfig, AutonomyProfile, PolicyMode, PolicySettings, load_agent_config,
    validation_commands_for_plan,
};
use crate::agent_protocol::{
    ActionOutcome, AgentAction, AgentMode, PreviewEditPayload, ValidationPlan, stable_content_hash,
};
use crate::agent_turn::{AgentTurnResponse, parse_agent_turn_response};

impl AgentTaskState {
    pub(crate) fn allow_action(&self, action: &AgentAction) -> Result<(), String> {
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

    pub(crate) fn allow_action_for_benchmark_policy(&self, action: &AgentAction) -> Result<(), String> {
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

    pub(crate) fn benchmark_write_targets_disallowed_test_file(&self, path: &str) -> bool {
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

    pub(crate) fn benchmark_write_requires_observed_target_context(
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

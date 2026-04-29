use std::collections::{BTreeMap, BTreeSet};

use quorp_verify_model::{FailurePacket, FailureSpan};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EvidenceBoard {
    pub failing_span: Option<FailureSpan>,
    pub suspected_owner_files: BTreeSet<String>,
    pub latest_hashes: BTreeMap<String, String>,
    pub missing_evidence: BTreeSet<String>,
    pub repeated_reads: BTreeMap<String, usize>,
    pub validation_state: Option<String>,
    pub leased_patch_target: Option<String>,
    pub failure_packet: Option<FailurePacket>,
}

impl EvidenceBoard {
    pub(crate) fn from_state(state: &AgentTaskState) -> Self {
        let failure_packet = state.agent_repair_memory.last_failure_packet.clone();
        let failing_span = failure_packet
            .as_ref()
            .and_then(|packet| packet.primary_span.clone());
        let mut suspected_owner_files = BTreeSet::new();
        if let Some(path) = failure_packet
            .as_ref()
            .and_then(FailurePacket::primary_path)
            .map(|path| path.display().to_string())
        {
            suspected_owner_files.insert(path);
        }
        if let Some(requirement) = state.repair_requirement.as_ref() {
            suspected_owner_files.insert(requirement.path.clone());
        }
        if let Some(ledger) = state.benchmark_case_ledger.as_ref() {
            suspected_owner_files.extend(ledger.owner_files.iter().cloned());
            suspected_owner_files.extend(ledger.expected_touch_targets.iter().cloned());
        }
        if let Some(repair_state) = state.benchmark_repair_state.as_ref() {
            suspected_owner_files.insert(repair_state.owner_path.clone());
        }

        let mut latest_hashes = BTreeMap::new();
        let mut repeated_reads = BTreeMap::new();
        for slice in &state.agent_repair_memory.observed_slices {
            if let Some(hash) = slice.content_fingerprint.as_ref() {
                latest_hashes.insert(slice.path.clone(), hash.clone());
            }
            let range = slice
                .honored_range
                .or(slice.requested_range)
                .map(|range| range.label());
            let key = observation_key(&slice.path, range);
            *repeated_reads.entry(key).or_insert(0) += 1;
        }

        let mut missing_evidence = BTreeSet::new();
        if let Some(requirement) = state.repair_requirement.as_ref()
            && state.repair_requirement_needs_reread()
        {
            missing_evidence.insert(requirement.path.clone());
        } else if let Some(span) = failing_span.as_ref() {
            let span_path = span.file.display().to_string();
            if !latest_hashes.contains_key(&span_path) {
                missing_evidence.insert(span_path);
            }
        }

        let validation_state = state
            .benchmark_case_ledger
            .as_ref()
            .and_then(|ledger| ledger.validation_details.diagnostic_class.clone())
            .or_else(|| {
                state
                    .repair_requirement
                    .as_ref()
                    .map(|value| value.failure_reason.clone())
            })
            .or_else(|| {
                state
                    .agent_repair_memory
                    .last_failure_packet
                    .as_ref()
                    .map(|packet| packet.summary.clone())
            });

        let leased_patch_target = state
            .repair_requirement
            .as_ref()
            .map(|requirement| requirement.path.clone())
            .or_else(|| {
                state
                    .agent_repair_memory
                    .implementation_target_lease
                    .as_ref()
                    .cloned()
            });

        Self {
            failing_span,
            suspected_owner_files,
            latest_hashes,
            missing_evidence,
            repeated_reads,
            validation_state,
            leased_patch_target,
            failure_packet,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn has_failure_signal(&self) -> bool {
        self.failure_packet.is_some()
            || self.failing_span.is_some()
            || !self
                .validation_state
                .as_deref()
                .unwrap_or_default()
                .is_empty()
    }

    pub(crate) fn has_targeted_observation(&self) -> bool {
        if let Some(target) = self.leased_patch_target.as_ref()
            && self.latest_hashes.contains_key(target)
        {
            return true;
        }
        if let Some(span) = self.failing_span.as_ref() {
            let path = span.file.display().to_string();
            if self.latest_hashes.contains_key(&path) {
                return true;
            }
        }
        self.suspected_owner_files
            .iter()
            .any(|path| self.latest_hashes.contains_key(path))
    }

    #[allow(dead_code)]
    pub(crate) fn repeated_read_count(&self, key: &str) -> usize {
        self.repeated_reads.get(key).copied().unwrap_or(0)
    }

    pub(crate) fn failure_span_path(&self) -> Option<String> {
        self.failing_span
            .as_ref()
            .map(|span| span.file.display().to_string())
    }
}

pub(crate) fn observation_key(path: &str, range: Option<String>) -> String {
    match range {
        Some(range) => format!("{path}:{range}"),
        None => format!("{path}:all"),
    }
}

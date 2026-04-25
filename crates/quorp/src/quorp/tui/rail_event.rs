#[derive(Debug, Clone, PartialEq)]
pub enum AgentPhase {
    Idle,
    Planning,
    Exploring,
    Editing,
    Verifying,
    Debugging,
    WaitingForApproval,
}

impl AgentPhase {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Planning => "PLANNING",
            Self::Exploring => "EXPLORING",
            Self::Editing => "EDITING",
            Self::Verifying => "VERIFYING",
            Self::Debugging => "DEBUGGING",
            Self::WaitingForApproval => "APPROVAL",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskSeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl RiskSeverity {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolState {
    Queued,
    Running,
    Streaming,
    Done,
    Failed,
    Superseded,
}

impl ToolState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Streaming => "streaming",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Superseded => "superseded",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Superseded)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Search,
    Read,
    Edit,
    Command,
    Test,
    Git,
    WebOrMcp,
    Subagent,
    Plan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolPresentation {
    pub kind: ToolKind,
    pub icon: &'static str,
    pub label: &'static str,
}

impl ToolKind {
    pub fn presentation(self) -> ToolPresentation {
        match self {
            Self::Search => ToolPresentation {
                kind: self,
                icon: "◌",
                label: "search",
            },
            Self::Read => ToolPresentation {
                kind: self,
                icon: "│",
                label: "read",
            },
            Self::Edit => ToolPresentation {
                kind: self,
                icon: "✎",
                label: "edit",
            },
            Self::Command => ToolPresentation {
                kind: self,
                icon: "▸",
                label: "command",
            },
            Self::Test => ToolPresentation {
                kind: self,
                icon: "✓",
                label: "test",
            },
            Self::Git => ToolPresentation {
                kind: self,
                icon: "◆",
                label: "git",
            },
            Self::WebOrMcp => ToolPresentation {
                kind: self,
                icon: "↗",
                label: "external",
            },
            Self::Subagent => ToolPresentation {
                kind: self,
                icon: "◎",
                label: "subagent",
            },
            Self::Plan => ToolPresentation {
                kind: self,
                icon: "◈",
                label: "plan",
            },
        }
    }

    pub fn classify(name: &str, target: &str) -> Self {
        let haystack = format!("{name} {target}").to_ascii_lowercase();
        if haystack.contains("cargo test")
            || haystack.contains("pytest")
            || haystack.contains("npm test")
            || haystack.contains("pnpm test")
            || haystack.contains("evaluate.sh")
            || haystack.contains("validation")
            || haystack.contains("verify")
        {
            Self::Test
        } else if haystack.contains("git ")
            || haystack.starts_with("git")
            || haystack.contains("diff")
            || haystack.contains("checkout")
        {
            Self::Git
        } else if haystack.contains("grep")
            || haystack.contains("rg ")
            || haystack.contains("search")
            || haystack.contains("find ")
            || haystack.contains("fd ")
        {
            Self::Search
        } else if haystack.contains("read")
            || haystack.contains("cat ")
            || haystack.contains("sed -n")
            || haystack.contains("head ")
            || haystack.contains("tail ")
            || haystack.contains("less ")
        {
            Self::Read
        } else if haystack.contains("apply_patch")
            || haystack.contains("write")
            || haystack.contains("edit")
            || haystack.contains("replace")
            || haystack.contains("patch")
        {
            Self::Edit
        } else if haystack.contains("web ")
            || haystack.contains("http")
            || haystack.contains("curl ")
            || haystack.contains("wget ")
            || haystack.contains("mcp")
        {
            Self::WebOrMcp
        } else if haystack.contains("subagent") || haystack.contains("worker") {
            Self::Subagent
        } else if haystack.contains("plan") {
            Self::Plan
        } else {
            Self::Command
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlanStep {
    pub label: String,
    pub status: PlanStepStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanStepStatus {
    Pending,
    Active,
    Blocked,
    Completed,
    Skipped,
    Invalidated,
}

impl PlanStepStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Completed => "done",
            Self::Skipped => "skipped",
            Self::Invalidated => "invalidated",
        }
    }
}

/// Structured events that drive the Proof Rail and Confidence Engine.
///
/// Every event emitted by the backend or agent runtime flows through
/// this type to update the right-rail state, confidence scores, and
/// narrative stream enrichment.
#[derive(Debug, Clone)]
pub enum RailEvent {
    PhaseChanged(AgentPhase),

    ConfidenceUpdate {
        understanding: f32,
        merge_safety: f32,
        delta: f32,
    },

    ToolStarted {
        tool_id: u64,
        name: String,
        kind: ToolKind,
        target: String,
        cwd: Option<String>,
        expected_outcome: String,
        validation_kind: Option<String>,
    },

    ToolProgress {
        tool_id: u64,
        state: ToolState,
        latest_output: Option<String>,
    },

    ToolCompleted {
        tool_id: u64,
        exit_code: Option<i32>,
        duration_ms: u64,
        files_changed: u16,
        confidence_delta: Option<f32>,
    },

    EvidenceGained {
        fact: String,
        source: String,
    },

    RiskPromoted {
        description: String,
        severity: RiskSeverity,
        blast_radius: u16,
    },

    RiskResolved {
        description: String,
    },

    BlastRadiusUpdate {
        files_touched: Vec<String>,
        symbols_changed: u32,
        net_lines_delta: i32,
    },

    UnknownSurfaced {
        description: String,
    },

    UnknownResolved {
        description: String,
    },

    /// Emitted proactively when the agent knows why it's waiting.
    /// Powers "Zero Dark Minutes" — the UI renders this if silence
    /// exceeds 1.2 seconds.
    WaitReason {
        explanation: String,
    },

    ProofProgress {
        tests_passed: u32,
        tests_total: u32,
        coverage_delta: f32,
    },

    PlanLocked {
        steps: Vec<PlanStep>,
    },

    PlanStepUpdate {
        step_index: usize,
        status: PlanStepStatus,
    },

    /// One-sentence summary of what the agent is doing right now.
    /// Continuously updated; rendered at the top of the proof rail.
    OneSecondStory {
        summary: String,
    },

    TopDoubtUpdated {
        doubt: String,
    },

    TimeToProofUpdated {
        eta_seconds: Option<u64>,
        confidence_target: Option<f32>,
    },

    ReasoningStep {
        objective: String,
        evidence: Vec<String>,
        action: String,
        expected_result: String,
        rollback: Option<String>,
        rejected_branch: Option<String>,
    },

    StopReasonSet {
        reason: String,
    },

    WatchpointAdded {
        label: String,
    },

    WatchpointTriggered {
        label: String,
        detail: String,
    },

    RollbackReadinessChanged {
        ready: bool,
        summary: String,
    },

    ArtifactReady {
        label: String,
        path: String,
    },

    ContextPressure {
        tokens_used: u64,
        tokens_limit: u64,
        facts_compacted: u32,
    },

    SessionCheckpoint {
        label: String,
        commit_hash: Option<String>,
    },
}

/// Accumulated state derived from a stream of `RailEvent`s.
/// The proof rail renders from this snapshot rather than
/// re-processing the full event history each frame.
#[derive(Debug, Clone)]
pub struct RailSnapshot {
    pub phase: AgentPhase,
    pub one_second_story: String,
    pub top_doubt: String,

    pub confidence_understanding: f32,
    pub confidence_merge_safety: f32,
    pub confidence_composite: f32,
    pub time_to_proof_seconds: Option<u64>,
    pub time_to_proof_confidence_target: Option<f32>,

    pub plan_steps: Vec<PlanStep>,
    pub active_tools: Vec<ActiveToolEntry>,
    pub evidence_log: Vec<EvidenceEntry>,
    pub risk_items: Vec<RiskEntry>,
    pub unknowns: Vec<String>,
    pub latest_reasoning: Option<ReasoningLedgerEntry>,
    pub watchpoints: Vec<WatchpointEntry>,
    pub artifacts: Vec<ArtifactEntry>,

    pub files_touched: Vec<String>,
    pub symbols_changed: u32,
    pub net_lines_delta: i32,

    pub tests_passed: u32,
    pub tests_total: u32,

    pub wait_reason: Option<String>,
    pub stop_reason: Option<String>,
    pub rollback_ready: bool,
    pub rollback_summary: Option<String>,

    pub context_tokens_used: u64,
    pub context_tokens_limit: u64,
    pub facts_compacted: u32,

    pub checkpoints: Vec<CheckpointEntry>,
}

#[derive(Debug, Clone)]
pub struct ActiveToolEntry {
    pub tool_id: u64,
    pub name: String,
    pub kind: ToolKind,
    pub target: String,
    pub cwd: Option<String>,
    pub expected_outcome: String,
    pub validation_kind: Option<String>,
    pub state: ToolState,
    pub latest_output: Option<String>,
    pub duration_ms: u64,
    pub files_changed: u16,
    pub confidence_delta: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct EvidenceEntry {
    pub fact: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct RiskEntry {
    pub description: String,
    pub severity: RiskSeverity,
    pub blast_radius: u16,
}

#[derive(Debug, Clone)]
pub struct CheckpointEntry {
    pub label: String,
    pub commit_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WatchpointEntry {
    pub label: String,
    pub triggered: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ArtifactEntry {
    pub label: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct ReasoningLedgerEntry {
    pub objective: String,
    pub evidence: Vec<String>,
    pub action: String,
    pub expected_result: String,
    pub rollback: Option<String>,
    pub rejected_branch: Option<String>,
}

impl Default for RailSnapshot {
    fn default() -> Self {
        Self {
            phase: AgentPhase::Idle,
            one_second_story: String::new(),
            top_doubt: String::new(),
            confidence_understanding: 0.0,
            confidence_merge_safety: 0.0,
            confidence_composite: 0.0,
            time_to_proof_seconds: None,
            time_to_proof_confidence_target: None,
            plan_steps: Vec::new(),
            active_tools: Vec::new(),
            evidence_log: Vec::new(),
            risk_items: Vec::new(),
            unknowns: Vec::new(),
            latest_reasoning: None,
            watchpoints: Vec::new(),
            artifacts: Vec::new(),
            files_touched: Vec::new(),
            symbols_changed: 0,
            net_lines_delta: 0,
            tests_passed: 0,
            tests_total: 0,
            wait_reason: None,
            stop_reason: None,
            rollback_ready: false,
            rollback_summary: None,
            context_tokens_used: 0,
            context_tokens_limit: 0,
            facts_compacted: 0,
            checkpoints: Vec::new(),
        }
    }
}

impl RailSnapshot {
    /// Apply a single event to the snapshot, updating accumulated state.
    pub fn apply(&mut self, event: &RailEvent) {
        match event {
            RailEvent::PhaseChanged(phase) => {
                self.phase = phase.clone();
                self.wait_reason = None;
            }

            RailEvent::ConfidenceUpdate {
                understanding,
                merge_safety,
                delta: _,
            } => {
                self.confidence_understanding = *understanding;
                self.confidence_merge_safety = *merge_safety;
                self.recompute_composite();
            }

            RailEvent::ToolStarted {
                tool_id,
                name,
                kind,
                target,
                cwd,
                expected_outcome,
                validation_kind,
            } => {
                self.active_tools.push(ActiveToolEntry {
                    tool_id: *tool_id,
                    name: name.clone(),
                    kind: *kind,
                    target: target.clone(),
                    cwd: cwd.clone(),
                    expected_outcome: expected_outcome.clone(),
                    validation_kind: validation_kind.clone(),
                    state: ToolState::Running,
                    latest_output: None,
                    duration_ms: 0,
                    files_changed: 0,
                    confidence_delta: None,
                });
                self.wait_reason = None;
            }

            RailEvent::ToolProgress {
                tool_id,
                state,
                latest_output,
            } => {
                if let Some(entry) = self
                    .active_tools
                    .iter_mut()
                    .find(|tool| tool.tool_id == *tool_id)
                {
                    entry.state = state.clone();
                    if let Some(output) = latest_output {
                        entry.latest_output = Some(output.clone());
                    }
                }
            }

            RailEvent::ToolCompleted {
                tool_id,
                exit_code: _,
                duration_ms,
                files_changed,
                confidence_delta,
            } => {
                if let Some(entry) = self
                    .active_tools
                    .iter_mut()
                    .find(|tool| tool.tool_id == *tool_id)
                {
                    entry.state = ToolState::Done;
                    entry.duration_ms = *duration_ms;
                    entry.files_changed = *files_changed;
                    entry.confidence_delta = *confidence_delta;
                }
                self.active_tools
                    .retain(|tool| !tool.state.is_terminal() || tool.tool_id == *tool_id);
            }

            RailEvent::EvidenceGained { fact, source } => {
                self.evidence_log.push(EvidenceEntry {
                    fact: fact.clone(),
                    source: source.clone(),
                });
            }

            RailEvent::RiskPromoted {
                description,
                severity,
                blast_radius,
            } => {
                self.risk_items.push(RiskEntry {
                    description: description.clone(),
                    severity: severity.clone(),
                    blast_radius: *blast_radius,
                });
                if self.top_doubt.is_empty() {
                    self.top_doubt = description.clone();
                }
                self.recompute_composite();
            }

            RailEvent::RiskResolved { description } => {
                self.risk_items
                    .retain(|risk| risk.description != *description);
                self.recompute_composite();
            }

            RailEvent::BlastRadiusUpdate {
                files_touched,
                symbols_changed,
                net_lines_delta,
            } => {
                self.files_touched = files_touched.clone();
                self.symbols_changed = *symbols_changed;
                self.net_lines_delta = *net_lines_delta;
            }

            RailEvent::UnknownSurfaced { description } => {
                if !self.unknowns.contains(description) {
                    self.unknowns.push(description.clone());
                }
                if self.top_doubt.is_empty() {
                    self.top_doubt = description.clone();
                }
                self.recompute_composite();
            }

            RailEvent::UnknownResolved { description } => {
                self.unknowns.retain(|item| item != description);
                self.recompute_composite();
            }

            RailEvent::WaitReason { explanation } => {
                self.wait_reason = Some(explanation.clone());
            }

            RailEvent::ProofProgress {
                tests_passed,
                tests_total,
                coverage_delta: _,
            } => {
                self.tests_passed = *tests_passed;
                self.tests_total = *tests_total;
                self.recompute_composite();
            }

            RailEvent::PlanLocked { steps } => {
                self.plan_steps = steps.clone();
            }

            RailEvent::PlanStepUpdate { step_index, status } => {
                if let Some(step) = self.plan_steps.get_mut(*step_index) {
                    step.status = status.clone();
                }
            }

            RailEvent::OneSecondStory { summary } => {
                self.one_second_story = summary.clone();
            }

            RailEvent::TopDoubtUpdated { doubt } => {
                self.top_doubt = doubt.clone();
            }

            RailEvent::TimeToProofUpdated {
                eta_seconds,
                confidence_target,
            } => {
                self.time_to_proof_seconds = *eta_seconds;
                self.time_to_proof_confidence_target = *confidence_target;
            }

            RailEvent::ReasoningStep {
                objective,
                evidence,
                action,
                expected_result,
                rollback,
                rejected_branch,
            } => {
                self.latest_reasoning = Some(ReasoningLedgerEntry {
                    objective: objective.clone(),
                    evidence: evidence.clone(),
                    action: action.clone(),
                    expected_result: expected_result.clone(),
                    rollback: rollback.clone(),
                    rejected_branch: rejected_branch.clone(),
                });
            }

            RailEvent::StopReasonSet { reason } => {
                self.stop_reason = Some(reason.clone());
                self.wait_reason = None;
            }

            RailEvent::WatchpointAdded { label } => {
                if !self.watchpoints.iter().any(|watch| watch.label == *label) {
                    self.watchpoints.push(WatchpointEntry {
                        label: label.clone(),
                        triggered: false,
                        detail: None,
                    });
                }
            }

            RailEvent::WatchpointTriggered { label, detail } => {
                if let Some(entry) = self
                    .watchpoints
                    .iter_mut()
                    .find(|watch| watch.label == *label)
                {
                    entry.triggered = true;
                    entry.detail = Some(detail.clone());
                } else {
                    self.watchpoints.push(WatchpointEntry {
                        label: label.clone(),
                        triggered: true,
                        detail: Some(detail.clone()),
                    });
                }
                if self.top_doubt.is_empty() {
                    self.top_doubt = format!("{label}: {detail}");
                }
            }

            RailEvent::RollbackReadinessChanged { ready, summary } => {
                self.rollback_ready = *ready;
                self.rollback_summary = Some(summary.clone());
                self.recompute_composite();
            }

            RailEvent::ArtifactReady { label, path } => {
                if let Some(entry) = self
                    .artifacts
                    .iter_mut()
                    .find(|entry| entry.label == *label)
                {
                    entry.path = path.clone();
                } else {
                    self.artifacts.push(ArtifactEntry {
                        label: label.clone(),
                        path: path.clone(),
                    });
                }
            }

            RailEvent::ContextPressure {
                tokens_used,
                tokens_limit,
                facts_compacted,
            } => {
                self.context_tokens_used = *tokens_used;
                self.context_tokens_limit = *tokens_limit;
                self.facts_compacted = *facts_compacted;
            }

            RailEvent::SessionCheckpoint { label, commit_hash } => {
                self.checkpoints.push(CheckpointEntry {
                    label: label.clone(),
                    commit_hash: commit_hash.clone(),
                });
                self.rollback_ready = true;
                self.rollback_summary = Some(format!("Checkpoint saved: {label}"));
                self.recompute_composite();
            }
        }
    }

    fn recompute_composite(&mut self) {
        // Tip5 formula:
        //   25% contract grounded (approximated by understanding)
        //   25% verification progress
        //   20% scope stability (approximated by inverse risk count)
        //   15% rollback readiness (approximated by checkpoint existence)
        //   15% unknowns bounded (inverse of unknowns count)
        let verification = if self.tests_total > 0 {
            self.tests_passed as f32 / self.tests_total as f32
        } else {
            0.0
        };

        let scope_stability = if self.risk_items.is_empty() {
            1.0
        } else {
            (1.0 / (1.0 + self.risk_items.len() as f32)).min(1.0)
        };

        let rollback_readiness = if self.rollback_ready || !self.checkpoints.is_empty() {
            1.0
        } else {
            0.2
        };

        let unknowns_bounded = if self.unknowns.is_empty() {
            1.0
        } else {
            (1.0 / (1.0 + self.unknowns.len() as f32)).min(1.0)
        };

        self.confidence_composite = (0.25 * self.confidence_understanding
            + 0.25 * verification
            + 0.20 * scope_stability
            + 0.15 * rollback_readiness
            + 0.15 * unknowns_bounded)
            .clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_snapshot_has_zero_confidence() {
        let snapshot = RailSnapshot::default();
        assert_eq!(snapshot.confidence_composite, 0.0);
        assert_eq!(snapshot.phase, AgentPhase::Idle);
    }

    #[test]
    fn phase_change_clears_wait_reason() {
        let mut snapshot = RailSnapshot::default();
        snapshot.apply(&RailEvent::WaitReason {
            explanation: "compiling".to_string(),
        });
        assert!(snapshot.wait_reason.is_some());

        snapshot.apply(&RailEvent::PhaseChanged(AgentPhase::Editing));
        assert!(snapshot.wait_reason.is_none());
        assert_eq!(snapshot.phase, AgentPhase::Editing);
    }

    #[test]
    fn tool_lifecycle_tracks_state() {
        let mut snapshot = RailSnapshot::default();
        snapshot.apply(&RailEvent::ToolStarted {
            tool_id: 1,
            name: "cargo_test".to_string(),
            kind: ToolKind::Test,
            target: "quorp".to_string(),
            cwd: None,
            expected_outcome: "14 tests pass".to_string(),
            validation_kind: Some("cargo-test".to_string()),
        });
        assert_eq!(snapshot.active_tools.len(), 1);
        assert_eq!(snapshot.active_tools[0].state, ToolState::Running);

        snapshot.apply(&RailEvent::ToolProgress {
            tool_id: 1,
            state: ToolState::Streaming,
            latest_output: Some("running 14 tests".to_string()),
        });
        assert_eq!(snapshot.active_tools[0].state, ToolState::Streaming);

        snapshot.apply(&RailEvent::ToolCompleted {
            tool_id: 1,
            exit_code: Some(0),
            duration_ms: 2340,
            files_changed: 0,
            confidence_delta: Some(0.12),
        });
        assert_eq!(snapshot.active_tools[0].state, ToolState::Done);
    }

    #[test]
    fn confidence_recomputes_on_update() {
        let mut snapshot = RailSnapshot::default();
        snapshot.apply(&RailEvent::ConfidenceUpdate {
            understanding: 0.8,
            merge_safety: 0.6,
            delta: 0.1,
        });
        assert!(snapshot.confidence_composite > 0.0);
        assert_eq!(snapshot.confidence_understanding, 0.8);
    }

    #[test]
    fn evidence_accumulates() {
        let mut snapshot = RailSnapshot::default();
        snapshot.apply(&RailEvent::EvidenceGained {
            fact: "wrapper paths resolve correctly".to_string(),
            source: "planner.rs:42".to_string(),
        });
        snapshot.apply(&RailEvent::EvidenceGained {
            fact: "test coverage includes edge case".to_string(),
            source: "tests/wrapper.rs:18".to_string(),
        });
        assert_eq!(snapshot.evidence_log.len(), 2);
    }

    #[test]
    fn risk_promote_and_resolve() {
        let mut snapshot = RailSnapshot::default();
        snapshot.apply(&RailEvent::RiskPromoted {
            description: "public API widened".to_string(),
            severity: RiskSeverity::High,
            blast_radius: 3,
        });
        assert_eq!(snapshot.risk_items.len(), 1);

        snapshot.apply(&RailEvent::RiskResolved {
            description: "public API widened".to_string(),
        });
        assert_eq!(snapshot.risk_items.len(), 0);
    }

    #[test]
    fn unknowns_deduplicate() {
        let mut snapshot = RailSnapshot::default();
        snapshot.apply(&RailEvent::UnknownSurfaced {
            description: "flaky test".to_string(),
        });
        snapshot.apply(&RailEvent::UnknownSurfaced {
            description: "flaky test".to_string(),
        });
        assert_eq!(snapshot.unknowns.len(), 1);
    }

    #[test]
    fn plan_step_updates() {
        let mut snapshot = RailSnapshot::default();
        snapshot.apply(&RailEvent::PlanLocked {
            steps: vec![
                PlanStep {
                    label: "read files".to_string(),
                    status: PlanStepStatus::Active,
                },
                PlanStep {
                    label: "patch planner".to_string(),
                    status: PlanStepStatus::Pending,
                },
            ],
        });
        assert_eq!(snapshot.plan_steps.len(), 2);

        snapshot.apply(&RailEvent::PlanStepUpdate {
            step_index: 0,
            status: PlanStepStatus::Completed,
        });
        assert_eq!(snapshot.plan_steps[0].status, PlanStepStatus::Completed);
    }

    #[test]
    fn composite_confidence_with_proof_and_checkpoints() {
        let mut snapshot = RailSnapshot::default();
        snapshot.apply(&RailEvent::ConfidenceUpdate {
            understanding: 0.9,
            merge_safety: 0.8,
            delta: 0.0,
        });
        snapshot.apply(&RailEvent::ProofProgress {
            tests_passed: 14,
            tests_total: 14,
            coverage_delta: 0.0,
        });
        snapshot.apply(&RailEvent::SessionCheckpoint {
            label: "pre-patch".to_string(),
            commit_hash: Some("abc123".to_string()),
        });
        // With high understanding, all tests passing, no risks, no unknowns,
        // and a checkpoint, composite should be high.
        assert!(
            snapshot.confidence_composite > 0.5,
            "expected moderate-high confidence, got {}",
            snapshot.confidence_composite
        );
    }

    #[test]
    fn trust_signals_are_accumulated() {
        let mut snapshot = RailSnapshot::default();
        snapshot.apply(&RailEvent::TopDoubtUpdated {
            doubt: "public API widening".to_string(),
        });
        snapshot.apply(&RailEvent::WatchpointAdded {
            label: "no migrations".to_string(),
        });
        snapshot.apply(&RailEvent::WatchpointTriggered {
            label: "no migrations".to_string(),
            detail: "schema.sql touched".to_string(),
        });
        snapshot.apply(&RailEvent::ArtifactReady {
            label: "events".to_string(),
            path: "/tmp/run/events.jsonl".to_string(),
        });
        snapshot.apply(&RailEvent::RollbackReadinessChanged {
            ready: true,
            summary: "single checkpoint".to_string(),
        });
        assert_eq!(snapshot.top_doubt, "public API widening");
        assert_eq!(snapshot.watchpoints.len(), 1);
        assert!(snapshot.watchpoints[0].triggered);
        assert_eq!(snapshot.artifacts.len(), 1);
        assert!(snapshot.rollback_ready);
    }
}

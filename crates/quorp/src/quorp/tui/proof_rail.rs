use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::quorp::tui::rail_event::{
    AgentPhase, PlanStepStatus, RailSnapshot, RiskSeverity, ToolKind, ToolState,
};
use crate::quorp::tui::theme::Theme;

/// Which lens the right rail is currently displaying.
/// Transitions happen automatically based on `RailEvent` flow,
/// but the user can override with hotkeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailMode {
    ControlTower,
    DiffReactor,
    VerifyRadar,
    TraceLens,
    ToolOrchestra,
    RiskLedger,
    WhyWaiting,
    MemoryViewport,
    TimelineScrubber,
}

impl RailMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::ControlTower => "CONTROL TOWER",
            Self::DiffReactor => "DIFF REACTOR",
            Self::VerifyRadar => "VERIFY RADAR",
            Self::TraceLens => "TRACE LENS",
            Self::ToolOrchestra => "TOOL ORCHESTRA",
            Self::RiskLedger => "RISK LEDGER",
            Self::WhyWaiting => "WHY WAITING",
            Self::MemoryViewport => "MEMORY VIEWPORT",
            Self::TimelineScrubber => "TIMELINE",
        }
    }

    /// Suggest the best mode given current snapshot state.
    /// The user can override this via hotkeys.
    pub fn auto_select(snapshot: &RailSnapshot) -> Self {
        if snapshot.wait_reason.is_some() {
            return Self::WhyWaiting;
        }

        if snapshot
            .risk_items
            .iter()
            .any(|risk| matches!(risk.severity, RiskSeverity::Critical | RiskSeverity::High))
        {
            return Self::RiskLedger;
        }

        if snapshot.active_tools.len() > 1 {
            return Self::ToolOrchestra;
        }

        if snapshot.context_tokens_limit > 0
            && snapshot.context_tokens_used * 100
                >= snapshot.context_tokens_limit.saturating_mul(85)
        {
            return Self::MemoryViewport;
        }

        match snapshot.phase {
            AgentPhase::Verifying => Self::VerifyRadar,
            AgentPhase::Editing => {
                if !snapshot.files_touched.is_empty() {
                    Self::DiffReactor
                } else {
                    Self::ControlTower
                }
            }
            _ => Self::ControlTower,
        }
    }
}

/// Top-level state for the proof rail pane.
#[derive(Debug, Clone)]
pub struct ProofRailState {
    pub mode: RailMode,
    pub user_mode_override: Option<RailMode>,
    pub snapshot: RailSnapshot,
}

impl Default for ProofRailState {
    fn default() -> Self {
        Self {
            mode: RailMode::ControlTower,
            user_mode_override: None,
            snapshot: RailSnapshot::default(),
        }
    }
}

impl ProofRailState {
    pub fn apply_event(&mut self, event: &crate::quorp::tui::rail_event::RailEvent) {
        self.snapshot.apply(event);
        if matches!(
            event,
            crate::quorp::tui::rail_event::RailEvent::BlastRadiusUpdate { .. }
        ) {
            self.evaluate_watchpoints();
        }

        if self.user_mode_override.is_none() {
            self.mode = RailMode::auto_select(&self.snapshot);
        }
    }

    pub fn set_user_mode(&mut self, mode: RailMode) {
        self.user_mode_override = Some(mode);
        self.mode = mode;
    }

    pub fn clear_user_mode(&mut self) {
        self.user_mode_override = None;
        self.mode = RailMode::auto_select(&self.snapshot);
    }

    pub fn effective_mode(&self) -> RailMode {
        self.mode
    }

    fn evaluate_watchpoints(&mut self) {
        let files = self.snapshot.files_touched.clone();
        let mut first_triggered_detail = None;
        for watchpoint in &mut self.snapshot.watchpoints {
            if watchpoint.triggered {
                continue;
            }
            let triggered_detail = match watchpoint.label.as_str() {
                "no migrations" => files
                    .iter()
                    .find(|path| {
                        let normalized = path.to_ascii_lowercase();
                        normalized.contains("migration")
                            || normalized.ends_with(".sql")
                            || normalized.contains("schema")
                    })
                    .map(|path| format!("{path} entered the blast radius")),
                "no public API widening" => files
                    .iter()
                    .find(|path| {
                        let normalized = path.to_ascii_lowercase();
                        normalized.ends_with("lib.rs")
                            || normalized.contains("/api/")
                            || normalized.contains("public")
                    })
                    .map(|path| format!("{path} may widen public surface")),
                "auth untouched" => files
                    .iter()
                    .find(|path| path.to_ascii_lowercase().contains("auth"))
                    .map(|path| format!("{path} touched auth-adjacent code")),
                "one-hop write radius" if files.len() > 1 => Some(format!(
                    "{} files touched instead of a single cluster",
                    files.len()
                )),
                _ => None,
            };
            if let Some(detail) = triggered_detail {
                watchpoint.triggered = true;
                watchpoint.detail = Some(detail.clone());
                if first_triggered_detail.is_none() {
                    first_triggered_detail = Some(format!("{}: {}", watchpoint.label, detail));
                }
            }
        }
        if let Some(detail) = first_triggered_detail
            && (self.snapshot.top_doubt.is_empty()
                || self.snapshot.top_doubt.contains("first grounded proof"))
        {
            self.snapshot.top_doubt = detail;
        }
    }

    pub fn render(&self, theme: &Theme, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        lines.extend(self.render_mode_header(theme, width));
        lines.push(Line::from(""));

        match self.effective_mode() {
            RailMode::ControlTower => {
                lines.extend(self.render_one_second_story(theme, width));
                lines.extend(self.render_mission_status(theme));
                lines.extend(self.render_confidence_card(theme, width));
                lines.extend(self.render_time_to_proof(theme));
                lines.extend(self.render_top_doubt(theme));
                lines.extend(self.render_plan_lattice(theme, width));
                lines.extend(self.render_tool_bus(theme, width));
                lines.extend(self.render_blast_radius(theme, width));
                lines.extend(self.render_watchpoints(theme));
                lines.extend(self.render_stop_and_rollback(theme));
                lines.extend(self.render_artifacts(theme));
                lines.extend(self.render_unknowns(theme, width));
            }
            RailMode::DiffReactor => {
                lines.extend(self.render_blast_radius(theme, width));
                lines.extend(self.render_stop_and_rollback(theme));
                lines.extend(self.render_confidence_card(theme, width));
            }
            RailMode::VerifyRadar => {
                lines.extend(self.render_proof_progress(theme, width));
                lines.extend(self.render_time_to_proof(theme));
                lines.extend(self.render_top_doubt(theme));
                lines.extend(self.render_confidence_card(theme, width));
                lines.extend(self.render_unknowns(theme, width));
            }
            RailMode::TraceLens => {
                lines.extend(self.render_trace_lens(theme));
                lines.extend(self.render_stop_and_rollback(theme));
                lines.extend(self.render_confidence_card(theme, width));
            }
            RailMode::ToolOrchestra => {
                lines.extend(self.render_tool_bus(theme, width));
                lines.extend(self.render_top_doubt(theme));
                lines.extend(self.render_confidence_card(theme, width));
            }
            RailMode::RiskLedger => {
                lines.extend(self.render_risk_items(theme, width));
                lines.extend(self.render_top_doubt(theme));
                lines.extend(self.render_watchpoints(theme));
                lines.extend(self.render_blast_radius(theme, width));
                lines.extend(self.render_confidence_card(theme, width));
            }
            RailMode::WhyWaiting => {
                lines.extend(self.render_wait_reason(theme, width));
                lines.extend(self.render_tool_bus(theme, width));
                lines.extend(self.render_top_doubt(theme));
                lines.extend(self.render_confidence_card(theme, width));
            }
            RailMode::MemoryViewport => {
                lines.extend(self.render_context_pressure(theme, width));
                lines.extend(self.render_watchpoints(theme));
                lines.extend(self.render_confidence_card(theme, width));
            }
            RailMode::TimelineScrubber => {
                lines.extend(self.render_timeline(theme));
                lines.extend(self.render_artifacts(theme));
                lines.extend(self.render_stop_and_rollback(theme));
            }
        }

        if self.snapshot.context_tokens_limit > 0
            && !matches!(self.effective_mode(), RailMode::MemoryViewport)
        {
            lines.push(Line::from(""));
            lines.extend(self.render_context_pressure(theme, width));
        }

        lines
    }

    fn render_mode_header(&self, theme: &Theme, _width: u16) -> Vec<Line<'static>> {
        let mode_label = self.effective_mode().label();
        let phase_label = self.snapshot.phase.label();

        let header_style = Style::default()
            .fg(theme.palette.link_blue)
            .add_modifier(Modifier::BOLD);
        let phase_style = Style::default().fg(theme.palette.text_muted);

        vec![Line::from(vec![
            Span::styled(format!("◆ {mode_label}"), header_style),
            Span::styled(format!("  {phase_label}"), phase_style),
        ])]
    }

    fn render_one_second_story(&self, theme: &Theme, _width: u16) -> Vec<Line<'static>> {
        if self.snapshot.one_second_story.is_empty() {
            return vec![];
        }
        let style = Style::default()
            .fg(theme.palette.text)
            .add_modifier(Modifier::ITALIC);
        vec![
            Line::from(Span::styled(self.snapshot.one_second_story.clone(), style)),
            Line::from(""),
        ]
    }

    fn render_mission_status(&self, theme: &Theme) -> Vec<Line<'static>> {
        let header = Style::default()
            .fg(theme.palette.secondary_teal)
            .add_modifier(Modifier::BOLD);
        let rollback_color = if self.snapshot.rollback_ready {
            theme.palette.success_green
        } else {
            theme.palette.warning_yellow
        };
        vec![
            Line::from(Span::styled("MISSION", header)),
            Line::from(vec![
                Span::styled(
                    format!(
                        "  phase {}  ",
                        self.snapshot.phase.label().to_ascii_lowercase()
                    ),
                    Style::default().fg(theme.palette.text),
                ),
                Span::styled(
                    if self.snapshot.rollback_ready {
                        "rollback ready"
                    } else {
                        "rollback pending"
                    },
                    Style::default()
                        .fg(rollback_color)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
        ]
    }

    fn render_time_to_proof(&self, theme: &Theme) -> Vec<Line<'static>> {
        let Some(eta_seconds) = self.snapshot.time_to_proof_seconds else {
            return vec![];
        };
        let header = Style::default()
            .fg(theme.palette.secondary_teal)
            .add_modifier(Modifier::BOLD);
        let eta_label = if eta_seconds >= 60 {
            format!("{}m {}s", eta_seconds / 60, eta_seconds % 60)
        } else {
            format!("{eta_seconds}s")
        };
        let confidence_target = self
            .snapshot
            .time_to_proof_confidence_target
            .map(|value| format!("  target {}%", (value * 100.0).round() as u32))
            .unwrap_or_default();
        vec![
            Line::from(Span::styled("TIME TO PROOF", header)),
            Line::from(Span::styled(
                format!("  {eta_label}{confidence_target}"),
                Style::default().fg(theme.palette.text_muted),
            )),
            Line::from(""),
        ]
    }

    fn render_top_doubt(&self, theme: &Theme) -> Vec<Line<'static>> {
        if self.snapshot.top_doubt.is_empty() {
            return vec![];
        }
        vec![
            Line::from(Span::styled(
                "TOP DOUBT",
                Style::default()
                    .fg(theme.palette.warning_yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled("  ? ", Style::default().fg(theme.palette.warning_yellow)),
                Span::styled(
                    self.snapshot.top_doubt.clone(),
                    Style::default().fg(theme.palette.text_muted),
                ),
            ]),
            Line::from(""),
        ]
    }

    fn render_confidence_card(&self, theme: &Theme, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let header = Style::default()
            .fg(theme.palette.secondary_teal)
            .add_modifier(Modifier::BOLD);
        let _value_style = Style::default()
            .fg(theme.palette.text)
            .add_modifier(Modifier::BOLD);
        let bar_width = width.saturating_sub(20) as usize;

        lines.push(Line::from(Span::styled("CONFIDENCE", header)));

        let composite_pct = (self.snapshot.confidence_composite * 100.0).round() as u32;
        let composite_color = confidence_color(theme, self.snapshot.confidence_composite);
        lines.push(Line::from(vec![
            Span::styled(
                "  On Rails  ",
                Style::default().fg(theme.palette.text_muted),
            ),
            Span::styled(
                format!("{composite_pct}%"),
                Style::default()
                    .fg(composite_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        lines.push(render_mini_bar(
            "  understand",
            self.snapshot.confidence_understanding,
            bar_width,
            theme,
        ));
        lines.push(render_mini_bar(
            "  merge safe",
            self.snapshot.confidence_merge_safety,
            bar_width,
            theme,
        ));

        lines.push(Line::from(""));
        lines
    }

    fn render_plan_lattice(&self, theme: &Theme, _width: u16) -> Vec<Line<'static>> {
        if self.snapshot.plan_steps.is_empty() {
            return vec![];
        }

        let mut lines = Vec::new();
        let header = Style::default()
            .fg(theme.palette.secondary_teal)
            .add_modifier(Modifier::BOLD);
        lines.push(Line::from(Span::styled("PLAN", header)));

        for step in &self.snapshot.plan_steps {
            let (glyph, color) = match step.status {
                PlanStepStatus::Completed => ("✓", theme.palette.success_green),
                PlanStepStatus::Active => ("▸", theme.palette.link_blue),
                PlanStepStatus::Blocked => ("⊘", theme.palette.warning_yellow),
                PlanStepStatus::Pending => ("○", theme.palette.text_faint),
                PlanStepStatus::Skipped => ("–", theme.palette.text_faint),
                PlanStepStatus::Invalidated => ("✗", theme.palette.danger_orange),
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {glyph} "), Style::default().fg(color)),
                Span::styled(
                    step.label.clone(),
                    Style::default().fg(theme.palette.text_muted),
                ),
            ]));
        }

        lines.push(Line::from(""));
        lines
    }

    fn render_tool_bus(&self, theme: &Theme, _width: u16) -> Vec<Line<'static>> {
        if self.snapshot.active_tools.is_empty() {
            return vec![];
        }

        let mut lines = Vec::new();
        let header = Style::default()
            .fg(theme.palette.secondary_teal)
            .add_modifier(Modifier::BOLD);
        lines.push(Line::from(Span::styled("TOOL BUS", header)));

        for tool in &self.snapshot.active_tools {
            let state_color = match tool.state {
                ToolState::Queued => theme.palette.text_faint,
                ToolState::Running => theme.palette.link_blue,
                ToolState::Streaming => theme.palette.secondary_teal,
                ToolState::Done => theme.palette.success_green,
                ToolState::Failed => theme.palette.danger_orange,
                ToolState::Superseded => theme.palette.text_faint,
            };
            let (tool_icon, tool_color) = tool_style(theme, tool.kind);
            let validation_suffix = tool
                .validation_kind
                .as_ref()
                .map(|kind| format!(" · {kind}"))
                .unwrap_or_default();

            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {} ", tool.state.label()),
                    Style::default()
                        .fg(state_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{tool_icon} {}", tool.name),
                    Style::default().fg(tool_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    validation_suffix,
                    Style::default().fg(theme.palette.text_faint),
                ),
            ]));

            if !tool.target.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("       → {}", tool.target),
                    Style::default().fg(theme.palette.text_faint),
                )));
            }

            if let Some(cwd) = &tool.cwd {
                lines.push(Line::from(Span::styled(
                    format!("       cwd {cwd}"),
                    Style::default().fg(theme.palette.text_faint),
                )));
            }

            if let Some(output) = &tool.latest_output {
                let truncated: String = output.chars().take(60).collect();
                lines.push(Line::from(Span::styled(
                    format!("       {truncated}"),
                    Style::default().fg(theme.palette.text_faint),
                )));
            }

            if tool.files_changed > 0 || tool.confidence_delta.is_some() {
                let confidence = tool
                    .confidence_delta
                    .map(|delta| format!("  Δconf {:+.2}", delta))
                    .unwrap_or_default();
                lines.push(Line::from(Span::styled(
                    format!("       {} files changed{confidence}", tool.files_changed),
                    Style::default().fg(theme.palette.text_faint),
                )));
            }
        }

        lines.push(Line::from(""));
        lines
    }

    fn render_blast_radius(&self, theme: &Theme, _width: u16) -> Vec<Line<'static>> {
        if self.snapshot.files_touched.is_empty() {
            return vec![];
        }

        let mut lines = Vec::new();
        let header = Style::default()
            .fg(theme.palette.secondary_teal)
            .add_modifier(Modifier::BOLD);
        lines.push(Line::from(Span::styled("BLAST RADIUS", header)));

        let delta_sign = if self.snapshot.net_lines_delta >= 0 {
            "+"
        } else {
            ""
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {} files", self.snapshot.files_touched.len()),
                Style::default().fg(theme.palette.text),
            ),
            Span::styled(
                format!("  {delta_sign}{} lines", self.snapshot.net_lines_delta),
                Style::default().fg(theme.palette.text_muted),
            ),
            Span::styled(
                format!("  {} symbols", self.snapshot.symbols_changed),
                Style::default().fg(theme.palette.text_muted),
            ),
        ]));

        for file in self.snapshot.files_touched.iter().take(6) {
            let basename = file.rsplit('/').next().unwrap_or(file);
            lines.push(Line::from(Span::styled(
                format!("    {basename}"),
                Style::default().fg(theme.palette.text_faint),
            )));
        }
        let remaining = self.snapshot.files_touched.len().saturating_sub(6);
        if remaining > 0 {
            lines.push(Line::from(Span::styled(
                format!("    +{remaining} more"),
                Style::default().fg(theme.palette.text_faint),
            )));
        }

        lines.push(Line::from(""));
        lines
    }

    fn render_unknowns(&self, theme: &Theme, _width: u16) -> Vec<Line<'static>> {
        if self.snapshot.unknowns.is_empty() {
            return vec![];
        }

        let mut lines = Vec::new();
        let header = Style::default()
            .fg(theme.palette.warning_yellow)
            .add_modifier(Modifier::BOLD);
        lines.push(Line::from(Span::styled("UNKNOWNS", header)));

        for unknown in self.snapshot.unknowns.iter().take(3) {
            lines.push(Line::from(vec![
                Span::styled("  ? ", Style::default().fg(theme.palette.warning_yellow)),
                Span::styled(
                    unknown.clone(),
                    Style::default().fg(theme.palette.text_muted),
                ),
            ]));
        }

        lines.push(Line::from(""));
        lines
    }

    fn render_watchpoints(&self, theme: &Theme) -> Vec<Line<'static>> {
        if self.snapshot.watchpoints.is_empty() {
            return vec![];
        }

        let mut lines = Vec::new();
        lines.push(Line::from(Span::styled(
            "WATCHPOINTS",
            Style::default()
                .fg(theme.palette.warning_yellow)
                .add_modifier(Modifier::BOLD),
        )));
        for watchpoint in self.snapshot.watchpoints.iter().take(4) {
            let color = if watchpoint.triggered {
                theme.palette.danger_orange
            } else {
                theme.palette.success_green
            };
            let glyph = if watchpoint.triggered { "!" } else { "✓" };
            lines.push(Line::from(vec![
                Span::styled(format!("  {glyph} "), Style::default().fg(color)),
                Span::styled(
                    watchpoint.label.clone(),
                    Style::default().fg(theme.palette.text_muted),
                ),
            ]));
            if let Some(detail) = &watchpoint.detail {
                lines.push(Line::from(Span::styled(
                    format!("      {detail}"),
                    Style::default().fg(theme.palette.text_faint),
                )));
            }
        }
        lines.push(Line::from(""));
        lines
    }

    fn render_risk_items(&self, theme: &Theme, _width: u16) -> Vec<Line<'static>> {
        if self.snapshot.risk_items.is_empty() {
            return vec![];
        }

        let mut lines = Vec::new();
        let header = Style::default()
            .fg(theme.palette.danger_orange)
            .add_modifier(Modifier::BOLD);
        lines.push(Line::from(Span::styled("RISK LEDGER", header)));

        for risk in &self.snapshot.risk_items {
            let severity_color = match risk.severity {
                RiskSeverity::Low => theme.palette.text_muted,
                RiskSeverity::Medium => theme.palette.warning_yellow,
                RiskSeverity::High => theme.palette.danger_orange,
                RiskSeverity::Critical => theme.palette.danger_orange,
            };

            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {} ", risk.severity.label()),
                    Style::default()
                        .fg(severity_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    risk.description.clone(),
                    Style::default().fg(theme.palette.text),
                ),
            ]));
        }

        lines.push(Line::from(""));
        lines
    }

    fn render_wait_reason(&self, theme: &Theme, _width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let header = Style::default()
            .fg(theme.palette.warning_yellow)
            .add_modifier(Modifier::BOLD);
        lines.push(Line::from(Span::styled("WHY WAITING", header)));

        if let Some(reason) = &self.snapshot.wait_reason {
            lines.push(Line::from(Span::styled(
                format!("  {reason}"),
                Style::default().fg(theme.palette.text),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "  agent silent, no wait reason emitted",
                Style::default().fg(theme.palette.text_faint),
            )));
        }

        lines.push(Line::from(""));
        lines
    }

    fn render_stop_and_rollback(&self, theme: &Theme) -> Vec<Line<'static>> {
        if self.snapshot.stop_reason.is_none() && self.snapshot.rollback_summary.is_none() {
            return vec![];
        }

        let mut lines = Vec::new();
        lines.push(Line::from(Span::styled(
            "SHIP STATE",
            Style::default()
                .fg(theme.palette.secondary_teal)
                .add_modifier(Modifier::BOLD),
        )));
        if let Some(reason) = &self.snapshot.stop_reason {
            lines.push(Line::from(Span::styled(
                format!("  stop {reason}"),
                Style::default().fg(theme.palette.text_muted),
            )));
        }
        if let Some(summary) = &self.snapshot.rollback_summary {
            let rollback_color = if self.snapshot.rollback_ready {
                theme.palette.success_green
            } else {
                theme.palette.warning_yellow
            };
            lines.push(Line::from(vec![
                Span::styled("  rollback ", Style::default().fg(theme.palette.text_muted)),
                Span::styled(summary.clone(), Style::default().fg(rollback_color)),
            ]));
        }
        lines.push(Line::from(""));
        lines
    }

    fn render_proof_progress(&self, theme: &Theme, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let header = Style::default()
            .fg(theme.palette.secondary_teal)
            .add_modifier(Modifier::BOLD);
        lines.push(Line::from(Span::styled("PROOF STACK", header)));

        if self.snapshot.tests_total == 0 {
            lines.push(Line::from(Span::styled(
                "  no tests queued",
                Style::default().fg(theme.palette.text_faint),
            )));
        } else {
            let ratio = self.snapshot.tests_passed as f32 / self.snapshot.tests_total as f32;
            let bar_width = width.saturating_sub(20) as usize;
            lines.push(render_mini_bar("  proof", ratio, bar_width, theme));
            lines.push(Line::from(Span::styled(
                format!(
                    "  {} / {} passed",
                    self.snapshot.tests_passed, self.snapshot.tests_total
                ),
                Style::default().fg(theme.palette.text_muted),
            )));
        }

        lines.push(Line::from(""));
        lines
    }

    fn render_context_pressure(&self, theme: &Theme, _width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let header = Style::default()
            .fg(theme.palette.text_faint)
            .add_modifier(Modifier::BOLD);
        lines.push(Line::from(Span::styled("CONTEXT", header)));

        let used_k = self.snapshot.context_tokens_used / 1000;
        let limit_k = self.snapshot.context_tokens_limit / 1000;
        let ratio = if self.snapshot.context_tokens_limit > 0 {
            self.snapshot.context_tokens_used as f32 / self.snapshot.context_tokens_limit as f32
        } else {
            0.0
        };

        let pressure_color = if ratio > 0.85 {
            theme.palette.danger_orange
        } else if ratio > 0.6 {
            theme.palette.warning_yellow
        } else {
            theme.palette.text_faint
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!("  {used_k}k / {limit_k}k tokens"),
                Style::default().fg(pressure_color),
            ),
            Span::styled(
                format!("  {} facts compacted", self.snapshot.facts_compacted),
                Style::default().fg(theme.palette.text_faint),
            ),
        ]));

        lines
    }

    fn render_artifacts(&self, theme: &Theme) -> Vec<Line<'static>> {
        if self.snapshot.artifacts.is_empty() {
            return vec![];
        }

        let mut lines = Vec::new();
        lines.push(Line::from(Span::styled(
            "ARTIFACTS",
            Style::default()
                .fg(theme.palette.secondary_teal)
                .add_modifier(Modifier::BOLD),
        )));
        for artifact in self.snapshot.artifacts.iter().take(4) {
            lines.push(Line::from(vec![
                Span::styled("  ↳ ", Style::default().fg(theme.palette.link_blue)),
                Span::styled(
                    artifact.label.clone(),
                    Style::default().fg(theme.palette.text),
                ),
            ]));
            lines.push(Line::from(Span::styled(
                format!("      {}", artifact.path),
                Style::default().fg(theme.palette.text_faint),
            )));
        }
        lines.push(Line::from(""));
        lines
    }

    fn render_trace_lens(&self, theme: &Theme) -> Vec<Line<'static>> {
        let Some(reasoning) = self.snapshot.latest_reasoning.as_ref() else {
            return vec![
                Line::from(Span::styled(
                    "TRACE LENS",
                    Style::default()
                        .fg(theme.palette.secondary_teal)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    "  no reasoning artifact captured yet",
                    Style::default().fg(theme.palette.text_faint),
                )),
                Line::from(""),
            ];
        };

        let mut lines = vec![Line::from(Span::styled(
            "TRACE LENS",
            Style::default()
                .fg(theme.palette.secondary_teal)
                .add_modifier(Modifier::BOLD),
        ))];
        lines.push(Line::from(vec![
            Span::styled(
                "  objective ",
                Style::default().fg(theme.palette.text_faint),
            ),
            Span::styled(
                reasoning.objective.clone(),
                Style::default().fg(theme.palette.text),
            ),
        ]));
        if !reasoning.evidence.is_empty() {
            lines.push(Line::from(Span::styled(
                "  evidence",
                Style::default().fg(theme.palette.text_faint),
            )));
            for evidence in reasoning.evidence.iter().take(3) {
                lines.push(Line::from(Span::styled(
                    format!("    • {evidence}"),
                    Style::default().fg(theme.palette.text_muted),
                )));
            }
        }
        lines.push(Line::from(vec![
            Span::styled("  action ", Style::default().fg(theme.palette.text_faint)),
            Span::styled(
                reasoning.action.clone(),
                Style::default().fg(theme.palette.text_muted),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  expect ", Style::default().fg(theme.palette.text_faint)),
            Span::styled(
                reasoning.expected_result.clone(),
                Style::default().fg(theme.palette.text_muted),
            ),
        ]));
        if let Some(rollback) = &reasoning.rollback {
            lines.push(Line::from(vec![
                Span::styled("  rollback ", Style::default().fg(theme.palette.text_faint)),
                Span::styled(
                    rollback.clone(),
                    Style::default().fg(theme.palette.text_muted),
                ),
            ]));
        }
        if let Some(rejected) = &reasoning.rejected_branch {
            lines.push(Line::from(vec![
                Span::styled("  rejected ", Style::default().fg(theme.palette.text_faint)),
                Span::styled(
                    rejected.clone(),
                    Style::default().fg(theme.palette.warning_yellow),
                ),
            ]));
        }
        lines.push(Line::from(""));
        lines
    }

    fn render_timeline(&self, theme: &Theme) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from(Span::styled(
            "TIMELINE",
            Style::default()
                .fg(theme.palette.secondary_teal)
                .add_modifier(Modifier::BOLD),
        ))];
        if self.snapshot.checkpoints.is_empty() {
            lines.push(Line::from(Span::styled(
                "  no checkpoints yet",
                Style::default().fg(theme.palette.text_faint),
            )));
        } else {
            for checkpoint in self.snapshot.checkpoints.iter().rev().take(4) {
                let commit = checkpoint
                    .commit_hash
                    .as_deref()
                    .map(|hash| format!("  {hash}"))
                    .unwrap_or_default();
                lines.push(Line::from(vec![
                    Span::styled("  • ", Style::default().fg(theme.palette.link_blue)),
                    Span::styled(
                        checkpoint.label.clone(),
                        Style::default().fg(theme.palette.text),
                    ),
                    Span::styled(commit, Style::default().fg(theme.palette.text_faint)),
                ]));
            }
        }
        if self.snapshot.stop_reason.is_none() {
            lines.push(Line::from(Span::styled(
                "  [ / ] scrub history  ·  . jump live",
                Style::default().fg(theme.palette.text_faint),
            )));
        }
        lines.push(Line::from(""));
        lines
    }
}

fn confidence_color(theme: &Theme, value: f32) -> ratatui::style::Color {
    if value >= 0.7 {
        theme.palette.success_green
    } else if value >= 0.4 {
        theme.palette.warning_yellow
    } else {
        theme.palette.danger_orange
    }
}

fn tool_style(theme: &Theme, kind: ToolKind) -> (&'static str, ratatui::style::Color) {
    let presentation = kind.presentation();
    let color = match kind {
        ToolKind::Search => theme.palette.tool_search,
        ToolKind::Read => theme.palette.tool_search,
        ToolKind::Edit => theme.palette.tool_edit,
        ToolKind::Command => theme.palette.terminal_accent,
        ToolKind::Test => theme.palette.tool_verify,
        ToolKind::Git => theme.palette.tool_git,
        ToolKind::WebOrMcp => theme.palette.link_blue,
        ToolKind::Subagent => theme.palette.tool_plan,
        ToolKind::Plan => theme.palette.tool_plan,
    };
    (presentation.icon, color)
}

fn render_mini_bar(label: &str, value: f32, max_width: usize, theme: &Theme) -> Line<'static> {
    let filled = ((value * max_width as f32).round() as usize).min(max_width);
    let empty = max_width.saturating_sub(filled);
    let pct = (value * 100.0).round() as u32;
    let bar_color = confidence_color(theme, value);

    Line::from(vec![
        Span::styled(
            format!("{label:>12} "),
            Style::default().fg(theme.palette.text_muted),
        ),
        Span::styled("█".repeat(filled), Style::default().fg(bar_color)),
        Span::styled(
            "░".repeat(empty),
            Style::default().fg(theme.palette.subtle_border),
        ),
        Span::styled(
            format!(" {pct}%"),
            Style::default().fg(theme.palette.text_faint),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quorp::tui::rail_event::{AgentPhase, RailEvent, RiskSeverity};

    #[test]
    fn auto_select_defaults_to_control_tower() {
        let snapshot = RailSnapshot::default();
        assert_eq!(RailMode::auto_select(&snapshot), RailMode::ControlTower);
    }

    #[test]
    fn auto_select_switches_to_why_waiting() {
        let mut snapshot = RailSnapshot::default();
        snapshot.apply(&RailEvent::WaitReason {
            explanation: "cargo lock".to_string(),
        });
        assert_eq!(RailMode::auto_select(&snapshot), RailMode::WhyWaiting);
    }

    #[test]
    fn auto_select_switches_to_risk_ledger_on_critical() {
        let mut snapshot = RailSnapshot::default();
        snapshot.apply(&RailEvent::RiskPromoted {
            description: "public API widened".to_string(),
            severity: RiskSeverity::Critical,
            blast_radius: 5,
        });
        assert_eq!(RailMode::auto_select(&snapshot), RailMode::RiskLedger);
    }

    #[test]
    fn auto_select_switches_to_verify_radar() {
        let mut snapshot = RailSnapshot::default();
        snapshot.apply(&RailEvent::PhaseChanged(AgentPhase::Verifying));
        assert_eq!(RailMode::auto_select(&snapshot), RailMode::VerifyRadar);
    }

    #[test]
    fn user_override_sticks() {
        let mut state = ProofRailState::default();
        state.set_user_mode(RailMode::ToolOrchestra);
        state.apply_event(&RailEvent::PhaseChanged(AgentPhase::Verifying));
        assert_eq!(state.effective_mode(), RailMode::ToolOrchestra);
    }

    #[test]
    fn clear_override_returns_to_auto() {
        let mut state = ProofRailState::default();
        state.set_user_mode(RailMode::ToolOrchestra);
        state.apply_event(&RailEvent::PhaseChanged(AgentPhase::Verifying));
        state.clear_user_mode();
        assert_eq!(state.effective_mode(), RailMode::VerifyRadar);
    }

    #[test]
    fn render_produces_lines() {
        let theme = Theme::void_neon();
        let state = ProofRailState::default();
        let lines = state.render(&theme, 40);
        assert!(
            !lines.is_empty(),
            "render should produce at least the header"
        );
    }

    #[test]
    fn render_with_tools_shows_tool_bus() {
        let theme = Theme::void_neon();
        let mut state = ProofRailState::default();
        state.apply_event(&RailEvent::ToolStarted {
            tool_id: 1,
            name: "grep".to_string(),
            kind: ToolKind::Search,
            target: "src/".to_string(),
            cwd: None,
            expected_outcome: "find usage".to_string(),
            validation_kind: None,
        });
        let lines = state.render(&theme, 40);
        let joined: String = lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect();
        assert!(joined.contains("TOOL BUS"), "should show tool bus card");
        assert!(joined.contains("grep"), "should show tool name");
    }
}

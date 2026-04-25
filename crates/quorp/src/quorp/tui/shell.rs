#![allow(dead_code)]

use crate::quorp::tui::bootstrap_loader::{BootstrapFrame, BootstrapLayoutMode};
use crate::quorp::tui::paint::{draw_text, fill_rect};
use crate::quorp::tui::proof_rail::ProofRailState;
use crate::quorp::tui::text_width::{truncate_fit, truncate_middle_fit, wrap_plain_lines};
use crate::quorp::tui::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::borrow::Cow;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellLayoutMode {
    Compact,
    Standard,
    Full,
    Cinema,
}

impl ShellLayoutMode {
    pub fn for_area(area: Rect) -> Self {
        match (area.width, area.height) {
            (..=100, _) | (_, ..=30) => Self::Compact,
            (..=139, _) | (_, ..=39) => Self::Standard,
            (..=179, _) | (_, ..=49) => Self::Full,
            _ => Self::Cinema,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellScene {
    Bootstrap,
    Ready,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellExperienceMode {
    Bootstrap,
    CommandCenter,
    DiffLens,
    VerifyRadar,
    TraceLens,
    Timeline,
    LegacyWorkbench,
}

impl ShellExperienceMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bootstrap => "BOOT",
            Self::CommandCenter => "COMMAND CENTER",
            Self::DiffLens => "DIFF LENS",
            Self::VerifyRadar => "VERIFY RADAR",
            Self::TraceLens => "TRACE LENS",
            Self::Timeline => "TIMELINE",
            Self::LegacyWorkbench => "LEGACY",
        }
    }

    pub fn is_shell_first(self) -> bool {
        !matches!(self, Self::LegacyWorkbench)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellFocus {
    Sidebar,
    Feed,
    Files,
    Terminal,
    Overlay,
    Explorer,
    Main,
    Assistant,
    Dock,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapStatus {
    Pending,
    Ok,
    Warn,
    Failed,
}

#[derive(Clone, Debug)]
pub struct BootstrapProbe {
    pub label: String,
    pub status: BootstrapStatus,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct ShellBootstrapView {
    pub subtitle: String,
    pub probes: Vec<BootstrapProbe>,
    pub footer: String,
    pub loader_frame: BootstrapFrame,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedItemTone {
    User,
    Assistant,
    Reasoning,
    Tool,
    Command,
    Validation,
    Muted,
    Warning,
    Error,
    Success,
    FileChange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssistantTone {
    Normal,
    Muted,
    Error,
    Success,
}

#[derive(Clone, Debug)]
pub struct AssistantBlock {
    pub role: &'static str,
    pub text: String,
    pub tone: AssistantTone,
    pub rich_lines: Option<Vec<Line<'static>>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MainWorkspaceMode {
    Preview,
    Terminal,
}

#[derive(Clone, Debug)]
pub struct ShellMainView {
    pub title: String,
    pub mode: MainWorkspaceMode,
    pub lines: Vec<String>,
    pub terminal_title: String,
    pub terminal_lines: Vec<String>,
    pub show_terminal_drawer: bool,
}

#[derive(Clone, Debug)]
pub struct ShellAssistantView {
    pub session_label: String,
    pub runtime_label: String,
    pub transcript: Vec<AssistantBlock>,
    pub composer_text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionPillTone {
    Normal,
    Active,
    Busy,
    Muted,
}

#[derive(Clone, Debug)]
pub struct ShellSessionPill {
    pub label: String,
    pub tone: SessionPillTone,
}

#[derive(Clone, Debug)]
pub struct ShellFeedItem {
    pub title: String,
    pub lines: Vec<String>,
    pub rich_lines: Option<Vec<Line<'static>>>,
    pub tone: FeedItemTone,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Clone, Debug)]
pub struct ShellExplorerItem {
    pub label: String,
    pub selected: bool,
}

#[derive(Clone, Debug)]
pub struct ShellProjectItem {
    pub label: String,
    pub status: String,
    pub selected: bool,
}

#[derive(Clone, Debug)]
pub struct ShellThreadItem {
    pub label: String,
    pub summary: String,
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    pub selected: bool,
}

#[derive(Clone, Debug)]
pub struct ShellSidebarView {
    pub projects: Vec<ShellProjectItem>,
    pub threads: Vec<ShellThreadItem>,
    pub active_project_root: String,
}

#[derive(Clone, Debug)]
pub struct ShellCenterView {
    pub thread_title: String,
    pub project_label: String,
    pub workspace_label: String,
    pub provider_label: String,
    pub runtime_label: String,
    pub model_label: String,
    pub session_identity: String,
    pub runtime_model_label: String,
    pub runtime_state_label: String,
    pub runtime_state_kind: ShellRuntimeStateKind,
    pub animation_phase: u8,
    pub feed: Vec<ShellFeedItem>,
    pub feed_scroll_top: usize,
    pub feed_total_lines: usize,
    pub feed_viewport_lines: usize,
    pub feed_scrollbar_hovered: bool,
    pub feed_lines: Vec<Line<'static>>,
    pub feed_links: Vec<AssistantFeedLink>,
    pub active_feed_link: Option<usize>,
    pub composer_text: String,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellRuntimeStateKind {
    Online,
    Transition,
    Ready,
    Offline,
}

#[derive(Clone, Debug)]
pub struct ShellDrawerView {
    pub title: String,
    pub collapsed_label: String,
    pub visible: bool,
    pub badge_label: Option<String>,
    pub detail_label: Option<String>,
    pub lines: Vec<String>,
    pub snapshot: Option<crate::quorp::tui::terminal_surface::TerminalSnapshot>,
    pub fullscreen: bool,
    pub capture_mode: bool,
}

#[derive(Clone, Debug)]
pub struct ShellOverlay {
    pub title: String,
    pub lines: Vec<Cow<'static, str>>,
}

#[derive(Clone, Debug)]
pub struct ShellState {
    pub scene: ShellScene,
    pub experience_mode: ShellExperienceMode,
    pub app_name: String,
    pub version_label: String,
    pub active_mode: String,
    pub focus: ShellFocus,
    pub status_hint: String,
    pub workspace_root: String,
    pub runtime_label: String,
    pub explorer_visible: bool,
    pub assistant_overlay: bool,
    pub explorer_items: Vec<ShellExplorerItem>,
    pub main: ShellMainView,
    pub assistant: ShellAssistantView,
    pub main_sessions: Vec<ShellSessionPill>,
    pub assistant_sessions: Vec<ShellSessionPill>,
    pub sidebar: ShellSidebarView,
    pub center: ShellCenterView,
    pub files: ShellDrawerView,
    pub terminal: ShellDrawerView,
    pub proof_rail_visible: bool,
    pub proof_rail: Option<ProofRailState>,
    pub diff_reactor: Option<crate::quorp::tui::diff_reactor::DiffReactorState>,
    pub attention_lease: Option<crate::quorp::tui::attention_lease::AttentionLease>,
    pub tool_orchestra: Option<crate::quorp::tui::tool_orchestra::ToolOrchestra>,
    pub overlay: Option<ShellOverlay>,
    pub bootstrap: Option<ShellBootstrapView>,
}

#[derive(Clone, Debug)]
pub struct AssistantFeedLink {
    pub row: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub target: String,
}

#[derive(Clone, Debug)]
struct FeedLineSegment {
    text: String,
    style: Style,
    link_target: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RenderedFeed {
    pub lines: Vec<Line<'static>>,
    pub links: Vec<AssistantFeedLink>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrandArtCell {
    pub symbol: char,
    pub fg: Color,
    pub bg: Color,
}

#[derive(Clone, Copy, Debug)]
pub struct ShellGeometry {
    pub header: Rect,
    pub body: Rect,
    pub sidebar: Rect,
    pub center: Rect,
    pub explorer: Option<Rect>,
    pub main: Rect,
    pub assistant: Option<Rect>,
    pub files_rail: Rect,
    pub files_drawer: Option<Rect>,
    pub terminal_bar: Rect,
    pub dock: Option<Rect>,
    pub terminal_drawer: Option<Rect>,
    pub proof_rail: Option<Rect>,
    pub footer: Rect,
    pub overlay: Option<Rect>,
}

impl ShellGeometry {
    pub fn for_state(area: Rect, state: &ShellState) -> Self {
        let header = Rect::new(area.x, area.y, area.width, 1.min(area.height));
        let footer = Rect::new(
            area.x,
            area.bottom().saturating_sub(1),
            area.width,
            if area.height > 2 { 1 } else { 0 },
        );
        let body_y = header.bottom();
        let body_h = area
            .height
            .saturating_sub(header.height)
            .saturating_sub(footer.height);
        let body = Rect::new(area.x, body_y, area.width, body_h);

        let terminal_bar_height = if body.height > 1 { 1 } else { 0 };
        let terminal_drawer_height = if state.terminal.visible {
            if state.terminal.fullscreen {
                body.height.saturating_sub(terminal_bar_height)
            } else {
                body.height / 2
            }
        } else {
            0
        };
        let content_height = body
            .height
            .saturating_sub(terminal_bar_height)
            .saturating_sub(terminal_drawer_height);
        let content = Rect::new(body.x, body.y, body.width, content_height);

        let terminal_bar = Rect::new(
            body.x,
            content.bottom(),
            body.width,
            terminal_bar_height.min(body.bottom().saturating_sub(content.bottom())),
        );
        let terminal_drawer = if state.terminal.visible && terminal_drawer_height > 0 {
            Some(Rect::new(
                body.x,
                terminal_bar.bottom(),
                body.width,
                body.bottom().saturating_sub(terminal_bar.bottom()),
            ))
        } else {
            None
        };

        let sidebar_width = if state.terminal.fullscreen || state.experience_mode.is_shell_first() {
            0
        } else {
            match ShellLayoutMode::for_area(area) {
                ShellLayoutMode::Compact => 24,
                ShellLayoutMode::Standard => 28,
                ShellLayoutMode::Full => 30,
                ShellLayoutMode::Cinema => 32,
            }
            .min(content.width.saturating_sub(24))
            .max(20)
        };

        let is_wide = area.width >= 140;
        let proof_rail_width = if state.terminal.fullscreen {
            0
        } else if is_wide || state.proof_rail_visible {
            (area.width as f32 * 0.32).round() as u16
        } else {
            0
        }
        .clamp(0, 80)
        .min(
            content
                .width
                .saturating_sub(sidebar_width)
                .saturating_sub(40),
        );

        let proof_rail = if proof_rail_width > 0 {
            Some(Rect::new(
                content.right().saturating_sub(proof_rail_width),
                content.y,
                proof_rail_width,
                content.height,
            ))
        } else {
            None
        };

        let files_rail_width = if state.terminal.fullscreen { 0 } else { 1u16 };
        let files_drawer_width = if state.terminal.fullscreen {
            0
        } else if state.files.visible {
            ((content.width as f32) * 0.30).round() as u16
        } else {
            0
        }
        .clamp(0, 36)
        .min(
            content
                .width
                .saturating_sub(sidebar_width)
                .saturating_sub(proof_rail_width)
                .saturating_sub(24),
        );

        let sidebar = Rect::new(content.x, content.y, sidebar_width, content.height);

        let files_rail_x = if let Some(pr) = proof_rail {
            pr.x.saturating_sub(files_rail_width)
        } else {
            content.right().saturating_sub(files_rail_width)
        };
        let files_rail = Rect::new(files_rail_x, content.y, files_rail_width, content.height);

        let files_drawer_x = files_rail.x.saturating_sub(files_drawer_width);
        let files_drawer = if state.files.visible && files_drawer_width > 0 {
            Some(Rect::new(
                files_drawer_x,
                content.y,
                files_drawer_width,
                content.height,
            ))
        } else {
            None
        };
        let center_right = files_drawer.map(|rect| rect.x).unwrap_or(files_rail.x);
        let center_left = if sidebar_width > 0 {
            sidebar.right()
        } else {
            content.x
        };
        let center = Rect::new(
            center_left,
            content.y,
            center_right.saturating_sub(center_left),
            content.height,
        );

        let overlay = (!state.terminal.fullscreen)
            .then_some(state.overlay.as_ref())
            .flatten()
            .map(|_| {
                let width = (center.width * 3 / 4)
                    .max(36)
                    .min(center.width.saturating_sub(4));
                let height = (center.height * 2 / 3)
                    .max(10)
                    .min(center.height.saturating_sub(2));
                Rect::new(
                    center.x + center.width.saturating_sub(width) / 2,
                    center.y + center.height.saturating_sub(height) / 2,
                    width,
                    height,
                )
            });

        Self {
            header,
            body,
            sidebar,
            center,
            explorer: files_drawer,
            main: center,
            assistant: Some(center),
            files_rail,
            files_drawer,
            terminal_bar,
            dock: terminal_drawer,
            terminal_drawer,
            proof_rail,
            footer,
            overlay,
        }
    }

    pub fn terminal_content_rect(&self, _state: &ShellState) -> Option<Rect> {
        self.terminal_drawer.map(|drawer| {
            Rect::new(
                drawer.x.saturating_add(1),
                drawer.y.saturating_add(1),
                drawer.width.saturating_sub(2),
                drawer.height.saturating_sub(2),
            )
        })
    }
}

const SHELL_COMPOSER_MIN_HEIGHT: u16 = 3;
const SHELL_COMPOSER_MAX_INPUT_LINES: u16 = 5;
const SHELL_COMPOSER_MIN_FEED_HEIGHT: u16 = 6;

pub(crate) fn shell_composer_input_width(composer_width: u16) -> usize {
    composer_width.saturating_sub(2) as usize
}

pub(crate) fn shell_composer_wrapped_lines(text: &str, composer_width: u16) -> Vec<String> {
    wrap_plain_lines(text, shell_composer_input_width(composer_width).max(1))
}

pub(crate) fn shell_composer_height_for_text(
    text: &str,
    composer_width: u16,
    max_available_height: u16,
) -> u16 {
    if max_available_height == 0 {
        return 0;
    }
    if max_available_height <= SHELL_COMPOSER_MIN_HEIGHT {
        return max_available_height;
    }

    let wrapped_lines = shell_composer_wrapped_lines(text, composer_width);
    let input_lines = wrapped_lines
        .len()
        .clamp(1, SHELL_COMPOSER_MAX_INPUT_LINES as usize) as u16;
    let desired_height = input_lines.saturating_add(2);
    let reserved_feed_height = SHELL_COMPOSER_MIN_FEED_HEIGHT
        .min(max_available_height.saturating_sub(SHELL_COMPOSER_MIN_HEIGHT));
    let max_composer_height = max_available_height
        .saturating_sub(reserved_feed_height)
        .max(SHELL_COMPOSER_MIN_HEIGHT);

    desired_height
        .min(max_composer_height)
        .min(max_available_height)
}

pub struct ShellRenderer;

impl ShellRenderer {
    pub fn render(buf: &mut Buffer, area: Rect, state: &ShellState, theme: &Theme) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        fill_rect(buf, area, theme.palette.canvas_bg);
        match state.scene {
            ShellScene::Bootstrap => Self::render_bootstrap(buf, area, state, theme),
            ShellScene::Ready => Self::render_ready(buf, area, state, theme),
        }
    }

    fn render_ready(buf: &mut Buffer, area: Rect, state: &ShellState, theme: &Theme) {
        let geometry = ShellGeometry::for_state(area, state);
        fill_rect(buf, area, theme.palette.canvas_bg);
        Self::render_header(buf, geometry.header, state, theme);
        if state.terminal.fullscreen {
            Self::render_terminal_bar(buf, geometry.terminal_bar, state, theme);
            if let Some(terminal_drawer) = geometry.terminal_drawer {
                Self::render_terminal_drawer(buf, terminal_drawer, state, theme);
                Self::draw_outline(
                    buf,
                    terminal_drawer,
                    Self::outline_color(matches!(state.focus, ShellFocus::Terminal), theme),
                );
            }
            Self::render_footer(buf, geometry.footer, state, theme);
            return;
        }
        if geometry.sidebar.width > 0 {
            Self::render_sidebar(buf, geometry.sidebar, state, theme);
        }
        Self::render_center(buf, geometry.center, state, theme);
        Self::render_files_rail(buf, geometry.files_rail, state, theme);
        if let Some(files_drawer) = geometry.files_drawer {
            Self::render_files_drawer(buf, files_drawer, state, theme);
        }
        if let Some(proof_rail) = geometry.proof_rail {
            Self::render_proof_rail(buf, proof_rail, state, theme);
        }
        Self::render_terminal_bar(buf, geometry.terminal_bar, state, theme);
        if let Some(terminal_drawer) = geometry.terminal_drawer {
            Self::render_terminal_drawer(buf, terminal_drawer, state, theme);
        }
        Self::render_footer(buf, geometry.footer, state, theme);
        Self::render_structure_lines(buf, &geometry, state, theme);

        if let (Some(overlay_rect), Some(overlay)) = (geometry.overlay, state.overlay.as_ref()) {
            fill_rect(buf, overlay_rect, theme.palette.panel_bg);
            Self::draw_outline(buf, overlay_rect, theme.palette.grid_line_focus);
            Self::render_overlay(buf, overlay_rect, overlay, theme);
        }
    }

    fn render_header(buf: &mut Buffer, rect: Rect, state: &ShellState, theme: &Theme) {
        fill_rect(buf, rect, theme.palette.titlebar_bg);
        let left = format!(
            " {} {}  {} ",
            state.app_name.to_uppercase(),
            state.version_label,
            state.experience_mode.label()
        );
        draw_text(
            buf,
            rect.x,
            rect.y,
            &left,
            Style::default()
                .fg(theme.palette.text_primary)
                .bg(theme.palette.titlebar_bg)
                .add_modifier(Modifier::BOLD),
            rect.width,
        );

        let proof_snapshot = state.proof_rail.as_ref().map(|rail| &rail.snapshot);
        let confidence_pct = proof_snapshot
            .map(|snapshot| (snapshot.confidence_composite * 100.0).round() as u32)
            .unwrap_or(0);
        let time_to_proof = proof_snapshot
            .and_then(|snapshot| snapshot.time_to_proof_seconds)
            .map(|seconds| {
                if seconds >= 60 {
                    format!("ttp {}m{}s", seconds / 60, seconds % 60)
                } else {
                    format!("ttp {seconds}s")
                }
            })
            .unwrap_or_else(|| "ttp --".to_string());
        let stop_reason = proof_snapshot
            .and_then(|snapshot| snapshot.stop_reason.as_ref())
            .map(|reason| format!(" stop {reason}"))
            .unwrap_or_default();
        let stats = format!(
            "rails {}%  {}  +{} -{}{}",
            confidence_pct,
            time_to_proof,
            state.center.additions,
            state.center.deletions,
            stop_reason
        );
        let stats_style = Style::default()
            .fg(theme.palette.success_green)
            .bg(theme.palette.titlebar_bg)
            .add_modifier(Modifier::BOLD);
        let stats_x = rect
            .right()
            .saturating_sub(stats.chars().count() as u16)
            .saturating_sub(1);
        draw_text(buf, stats_x, rect.y, &stats, stats_style, rect.width);

        let mut center_x = rect.x + left.chars().count() as u16;
        let available = stats_x.saturating_sub(center_x).saturating_sub(2);
        if available == 0 {
            return;
        }
        let story = proof_snapshot
            .map(|snapshot| snapshot.one_second_story.clone())
            .filter(|story| !story.is_empty())
            .unwrap_or_else(|| "waiting for a first proof signal".to_string());
        let meta_prefix = format!("{}  ·  {}", state.center.project_label, story);
        let prefix_text = truncate_middle_fit(&meta_prefix, available as usize);
        draw_text(
            buf,
            center_x,
            rect.y,
            &prefix_text,
            Style::default()
                .fg(theme.palette.text_muted)
                .bg(theme.palette.titlebar_bg)
                .add_modifier(Modifier::BOLD),
            available,
        );
        center_x = center_x.saturating_add(prefix_text.chars().count() as u16);

        if center_x >= stats_x.saturating_sub(1) {
            return;
        }

        let model_fg = match state.center.runtime_state_kind {
            ShellRuntimeStateKind::Online => theme.palette.runtime_online,
            ShellRuntimeStateKind::Transition => theme.palette.runtime_transition,
            ShellRuntimeStateKind::Ready => theme.palette.warning_yellow,
            ShellRuntimeStateKind::Offline => theme.palette.runtime_offline,
        };
        let model_style = Style::default()
            .fg(model_fg)
            .bg(theme.palette.titlebar_bg)
            .add_modifier(Modifier::BOLD);
        let runtime_chip = format!(" {} ", state.center.runtime_state_label);
        let reserved_runtime_width = runtime_chip.chars().count() as u16 + 2;
        let model_budget = stats_x
            .saturating_sub(center_x)
            .saturating_sub(reserved_runtime_width)
            .saturating_sub(1);
        let model_text =
            truncate_middle_fit(&state.center.runtime_model_label, model_budget as usize);
        draw_text(
            buf,
            center_x,
            rect.y,
            &model_text,
            model_style,
            model_budget,
        );
        center_x = center_x.saturating_add(model_text.chars().count() as u16);

        if center_x >= stats_x.saturating_sub(1) {
            return;
        }

        let sep = " ";
        draw_text(
            buf,
            center_x,
            rect.y,
            sep,
            Style::default()
                .fg(theme.palette.text_muted)
                .bg(theme.palette.titlebar_bg),
            1,
        );
        center_x = center_x.saturating_add(1);

        let (status_fg, status_bg) = match state.center.runtime_state_kind {
            ShellRuntimeStateKind::Online => {
                if state.center.animation_phase % 4 >= 2 {
                    (theme.palette.canvas_bg, theme.palette.runtime_online_hi)
                } else {
                    (theme.palette.canvas_bg, theme.palette.runtime_online)
                }
            }
            ShellRuntimeStateKind::Transition => {
                (theme.palette.canvas_bg, theme.palette.runtime_transition)
            }
            ShellRuntimeStateKind::Ready => {
                (theme.palette.titlebar_bg, theme.palette.warning_yellow)
            }
            ShellRuntimeStateKind::Offline => {
                (theme.palette.canvas_bg, theme.palette.runtime_offline)
            }
        };
        let runtime_chip = truncate_fit(
            &runtime_chip,
            stats_x.saturating_sub(center_x).saturating_sub(1) as usize,
        );
        draw_text(
            buf,
            center_x,
            rect.y,
            &runtime_chip,
            Style::default()
                .fg(status_fg)
                .bg(status_bg)
                .add_modifier(Modifier::BOLD),
            stats_x.saturating_sub(center_x).saturating_sub(1),
        );
    }

    fn render_sidebar(buf: &mut Buffer, rect: Rect, state: &ShellState, theme: &Theme) {
        fill_rect(buf, rect, theme.palette.sidebar_bg);
        let content = Rect::new(
            rect.x.saturating_add(1),
            rect.y.saturating_add(1),
            rect.width.saturating_sub(2),
            rect.height.saturating_sub(2),
        );
        if content.width == 0 || content.height == 0 {
            return;
        }
        draw_text(
            buf,
            content.x,
            content.y,
            " New Thread ",
            badge_style(
                matches!(state.focus, ShellFocus::Sidebar),
                theme.palette.accent_blue,
                theme,
            ),
            content.width,
        );
        draw_text(
            buf,
            content.x,
            content.y + 1,
            &truncate_middle_fit(&state.sidebar.active_project_root, content.width as usize),
            Style::default()
                .fg(theme.palette.terminal_path_fg)
                .bg(theme.palette.sidebar_bg),
            content.width,
        );
        draw_text(
            buf,
            content.x,
            content.y + 3,
            "Projects",
            Style::default()
                .fg(theme.palette.text_muted)
                .bg(theme.palette.sidebar_bg)
                .add_modifier(Modifier::BOLD),
            content.width,
        );

        let mut row = content.y + 4;
        for project in &state.sidebar.projects {
            if row >= content.bottom().saturating_sub(3) {
                break;
            }
            let status_style = project_status_style(project.status.as_str(), theme);
            let style = if project.selected {
                Style::default()
                    .fg(theme.palette.text_primary)
                    .bg(theme.palette.row_selected_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme.palette.text_primary)
                    .bg(theme.palette.sidebar_bg)
            };
            draw_text(
                buf,
                content.x,
                row,
                &truncate_fit(&project.label, content.width as usize),
                style,
                content.width,
            );
            row = row.saturating_add(1);
            if row >= content.bottom().saturating_sub(3) {
                break;
            }
            let status_bg = if project.selected {
                theme.palette.row_selected_bg
            } else {
                theme.palette.sidebar_bg
            };
            let status_text = project_status_indicator(
                project.status.as_str(),
                state.center.animation_phase,
                status_style,
            );
            draw_text(
                buf,
                content.x + 1,
                row,
                &truncate_fit(&status_text, content.width.saturating_sub(1) as usize),
                Style::default()
                    .fg(status_style)
                    .bg(status_bg)
                    .add_modifier(if project.status.eq_ignore_ascii_case("working") {
                        Modifier::BOLD
                    } else {
                        Modifier::DIM
                    }),
                content.width.saturating_sub(1),
            );
            row = row.saturating_add(1);
        }

        row = row.saturating_add(1);
        if row < content.bottom().saturating_sub(2) {
            draw_text(
                buf,
                content.x,
                row,
                "Threads",
                Style::default()
                    .fg(theme.palette.text_muted)
                    .bg(theme.palette.sidebar_bg)
                    .add_modifier(Modifier::BOLD),
                content.width,
            );
            row = row.saturating_add(1);
        }
        for thread in &state.sidebar.threads {
            if row >= content.bottom().saturating_sub(2) {
                break;
            }
            let style = if thread.selected {
                Style::default()
                    .fg(theme.palette.text_primary)
                    .bg(theme.palette.row_selected_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme.palette.text_muted)
                    .bg(theme.palette.sidebar_bg)
            };
            let first_line = format!(
                "{}  +{} -{}",
                thread.label, thread.additions, thread.deletions
            );
            draw_text(
                buf,
                content.x,
                row,
                &truncate_fit(&first_line, content.width as usize),
                style,
                content.width,
            );
            row = row.saturating_add(1);
            if row >= content.bottom().saturating_sub(2) {
                break;
            }
            let status_style = thread_status_style(thread.status.as_str(), theme);
            draw_text(
                buf,
                content.x + 1,
                row,
                &truncate_fit(
                    &format!(
                        "{} {}",
                        thread_indicator(thread.status.as_str(), state.center.animation_phase),
                        thread.summary
                    ),
                    content.width.saturating_sub(1) as usize,
                ),
                Style::default()
                    .fg(status_style)
                    .bg(if thread.selected {
                        theme.palette.row_selected_bg
                    } else {
                        theme.palette.sidebar_bg
                    })
                    .add_modifier(Modifier::BOLD),
                content.width.saturating_sub(1),
            );
            row = row.saturating_add(1);
        }

        let settings_y = content.bottom().saturating_sub(1);
        draw_text(
            buf,
            content.x,
            settings_y,
            " Settings ",
            Style::default()
                .fg(theme.palette.text_primary)
                .bg(theme.palette.sidebar_bg)
                .add_modifier(Modifier::BOLD),
            content.width,
        );
    }

    fn render_center(buf: &mut Buffer, rect: Rect, state: &ShellState, theme: &Theme) {
        fill_rect(buf, rect, theme.palette.panel_bg);
        if rect.width == 0 || rect.height == 0 {
            return;
        }

        let inner = Rect::new(
            rect.x.saturating_add(1),
            rect.y.saturating_add(1),
            rect.width.saturating_sub(2),
            rect.height.saturating_sub(2),
        );
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let header = Rect::new(inner.x, inner.y, inner.width, 2.min(inner.height));
        fill_rect(buf, header, theme.palette.panel_bg);
        let preview_mode = state.focus == ShellFocus::Main;
        let header_title = if preview_mode {
            format!("Preview · {}", state.main.title)
        } else {
            state.center.thread_title.clone()
        };
        draw_text(
            buf,
            header.x + 1,
            header.y,
            &truncate_fit(&header_title, header.width.saturating_sub(2) as usize),
            Style::default()
                .fg(theme.palette.text_primary)
                .bg(theme.palette.panel_bg)
                .add_modifier(Modifier::BOLD),
            header.width.saturating_sub(2),
        );
        let session_strip = if preview_mode {
            state
                .main_sessions
                .iter()
                .map(|pill| pill.label.as_str())
                .collect::<Vec<_>>()
                .join("  ")
        } else if state.assistant_sessions.is_empty() {
            String::new()
        } else {
            state
                .assistant_sessions
                .iter()
                .map(|pill| pill.label.as_str())
                .collect::<Vec<_>>()
                .join("  ")
        };
        let meta = if preview_mode {
            if session_strip.is_empty() {
                format!(
                    "Workspace: {}  ·  Alt+Down cycle targets  ·  Alt+Enter open  ·  D diff lens",
                    state.center.workspace_label
                )
            } else {
                format!("Targets: {session_strip}")
            }
        } else if session_strip.is_empty() {
            format!(
                "Workspace: {}  ·  Provider: {}  ·  Model: {}",
                state.center.workspace_label, state.center.provider_label, state.center.model_label,
            )
        } else {
            format!(
                "Sessions: {}  ·  Provider: {}  ·  Model: {}",
                session_strip, state.center.provider_label, state.center.model_label,
            )
        };
        draw_text(
            buf,
            header.x + 1,
            header.y + 1.min(header.height.saturating_sub(1)),
            &truncate_fit(&meta, header.width.saturating_sub(2) as usize),
            Style::default()
                .fg(theme.palette.chat_accent)
                .bg(theme.palette.panel_bg)
                .add_modifier(Modifier::BOLD),
            header.width.saturating_sub(2),
        );

        let composer_height = 3.min(inner.height.saturating_sub(header.height));
        let feed_rect = Rect::new(
            inner.x,
            header.bottom(),
            inner.width,
            inner
                .height
                .saturating_sub(header.height)
                .saturating_sub(composer_height),
        );
        let composer_rect = Rect::new(inner.x, feed_rect.bottom(), inner.width, composer_height);
        Self::render_feed(buf, feed_rect, state, theme);
        Self::render_composer(buf, composer_rect, state, theme);
    }

    fn render_feed(buf: &mut Buffer, rect: Rect, state: &ShellState, theme: &Theme) {
        use ratatui::widgets::{Paragraph, StatefulWidget, Widget};

        fill_rect(buf, rect, theme.palette.panel_bg);
        if rect.width == 0 || rect.height == 0 {
            return;
        }

        let show_scrollbar = state.center.feed_total_lines > rect.height as usize && rect.width > 1;
        let text_rect = if show_scrollbar {
            Rect::new(rect.x, rect.y, rect.width.saturating_sub(1), rect.height)
        } else {
            rect
        };
        let lines = if !state.center.feed_lines.is_empty() {
            state.center.feed_lines.clone()
        } else {
            ShellState::render_feed_lines(&state.center.feed, theme, text_rect.width as usize).lines
        };
        let visible = lines
            .into_iter()
            .skip(state.center.feed_scroll_top)
            .take(rect.height as usize)
            .collect::<Vec<_>>();
        Paragraph::new(visible).render(text_rect, buf);

        if let Some(active_index) = state.center.active_feed_link
            && let Some(link) = state.center.feed_links.get(active_index)
        {
            let row = link.row;
            let visible_start = state.center.feed_scroll_top;
            let visible_end = visible_start.saturating_add(rect.height as usize);
            if row >= visible_start && row < visible_end {
                let row = text_rect.y + (row.saturating_sub(visible_start)) as u16;
                if row < text_rect.bottom() {
                    let start_col = link.start_col.min(text_rect.width as usize);
                    let end_col = link.end_col.min(text_rect.width as usize);
                    let focus_style = Style::default()
                        .fg(theme.palette.canvas_bg)
                        .bg(theme.palette.link_blue);
                    for col in start_col..end_col {
                        if let Some(cell) =
                            buf.cell_mut((text_rect.x.saturating_add(col as u16), row))
                        {
                            cell.set_fg(focus_style.fg.unwrap_or(Color::Reset))
                                .set_bg(focus_style.bg.unwrap_or(Color::Reset));
                        }
                    }
                }
            }
        }

        if show_scrollbar {
            let mut scrollbar =
                ratatui::widgets::ScrollbarState::new(state.center.feed_total_lines)
                    .position(state.center.feed_scroll_top);
            StatefulWidget::render(
                ratatui::widgets::Scrollbar::new(
                    ratatui::widgets::ScrollbarOrientation::VerticalRight,
                )
                .thumb_style(Style::default().fg(if state.center.feed_scrollbar_hovered {
                    theme.palette.scrollbar_thumb_hi
                } else {
                    theme.palette.scrollbar_thumb
                }))
                .track_style(Style::default().bg(theme.palette.scrollbar_track)),
                rect,
                buf,
                &mut scrollbar,
            );
        }
    }

    fn render_composer(buf: &mut Buffer, rect: Rect, state: &ShellState, theme: &Theme) {
        fill_rect(buf, rect, theme.palette.inset_bg);
        Self::draw_outline(buf, rect, theme.palette.input_border);
        if rect.height == 0 {
            return;
        }
        let model_chip = format!(" {} ", state.center.model_label);
        draw_text(
            buf,
            rect.x + 1,
            rect.y,
            &model_chip,
            Style::default()
                .fg(theme.palette.canvas_bg)
                .bg(theme.palette.chat_accent)
                .add_modifier(Modifier::BOLD),
            rect.width.saturating_sub(2),
        );
        let submit = " Submit ";
        let submit_x = rect
            .right()
            .saturating_sub(submit.chars().count() as u16 + 1);
        draw_text(
            buf,
            submit_x,
            rect.y,
            submit,
            Style::default()
                .fg(theme.palette.canvas_bg)
                .bg(theme.palette.success_green)
                .add_modifier(Modifier::BOLD),
            submit.chars().count() as u16,
        );
        if rect.height <= 2 || rect.width <= 2 {
            return;
        }

        let input_rect = Rect::new(
            rect.x.saturating_add(1),
            rect.y.saturating_add(1),
            rect.width.saturating_sub(2),
            rect.height.saturating_sub(2),
        );
        let visible_rows = input_rect.height as usize;
        let lines = shell_composer_wrapped_lines(&state.center.composer_text, rect.width);
        let start_index = lines.len().saturating_sub(visible_rows);
        let visible_lines = lines.into_iter().skip(start_index).take(visible_rows);
        let text_style = Style::default()
            .fg(theme.palette.text_primary)
            .bg(theme.palette.inset_bg);

        for (row_index, line) in visible_lines.enumerate() {
            draw_text(
                buf,
                input_rect.x,
                input_rect.y.saturating_add(row_index as u16),
                &truncate_fit(&line, input_rect.width as usize),
                text_style,
                input_rect.width,
            );
        }
    }

    fn render_files_rail(buf: &mut Buffer, rect: Rect, state: &ShellState, theme: &Theme) {
        fill_rect(buf, rect, theme.palette.canvas_bg);
        let label = if state.files.visible { "▕" } else { "F" };
        draw_text(
            buf,
            rect.x,
            rect.y + rect.height / 2,
            label,
            Style::default()
                .fg(theme.palette.text_primary)
                .bg(theme.palette.canvas_bg)
                .add_modifier(Modifier::BOLD),
            rect.width,
        );
    }

    fn render_proof_rail(buf: &mut Buffer, rect: Rect, state: &ShellState, theme: &Theme) {
        fill_rect(buf, rect, theme.palette.sidebar_bg);
        if rect.width <= 2 || rect.height <= 2 {
            return;
        }

        if let Some(bootstrap) = state.bootstrap.as_ref() {
            Self::render_bootstrap_watermark(buf, rect, bootstrap, theme);
        }

        if let Some(proof_rail) = &state.proof_rail {
            let mut row = rect.y;
            for line in proof_rail.render(theme, rect.width) {
                if row >= rect.bottom() {
                    break;
                }
                let mut x = rect.x + 1;
                for span in line.spans {
                    let width = span.width() as u16;
                    draw_text(
                        buf,
                        x,
                        row,
                        &span.content,
                        span.style,
                        rect.width.saturating_sub(x - rect.x),
                    );
                    x += width;
                }
                row += 1;
            }
            return;
        }

        let header = Rect::new(rect.x, rect.y, rect.width, 1);
        draw_text(
            buf,
            header.x + 1,
            header.y,
            "PROOF // RAIL",
            badge_style(true, theme.palette.accent_blue, theme),
            header.width.saturating_sub(1),
        );

        // Control tower mock for now
        let mut row = rect.y + 2;

        if row < rect.bottom() {
            draw_text(
                buf,
                rect.x + 1,
                row,
                "Mission Status",
                Style::default()
                    .fg(theme.palette.text_muted)
                    .add_modifier(Modifier::BOLD),
                rect.width,
            );
            row += 1;
        }
        if row < rect.bottom() {
            draw_text(
                buf,
                rect.x + 2,
                row,
                "• Plan Grounded",
                Style::default().fg(theme.palette.success_green),
                rect.width,
            );
            row += 1;
        }
        if row < rect.bottom() {
            draw_text(
                buf,
                rect.x + 2,
                row,
                "• Risk Bounded",
                Style::default().fg(theme.palette.success_green),
                rect.width,
            );
            row += 2;
        }

        if row < rect.bottom() {
            draw_text(
                buf,
                rect.x + 1,
                row,
                "Tool Train",
                Style::default()
                    .fg(theme.palette.text_muted)
                    .add_modifier(Modifier::BOLD),
                rect.width,
            );
            row += 1;
        }
        if row < rect.bottom() {
            draw_text(
                buf,
                rect.x + 2,
                row,
                "[cargo test]",
                Style::default().fg(theme.palette.text_primary),
                rect.width,
            );
            row += 2;
        }

        if let Some(tool_orchestra) = &state.tool_orchestra {
            for (i, line) in tool_orchestra
                .render(theme, rect.width)
                .into_iter()
                .enumerate()
            {
                if row + i as u16 >= rect.bottom() {
                    break;
                }
                let mut x = rect.x + 1;
                for span in line.spans {
                    let w = span.width() as u16;
                    draw_text(
                        buf,
                        x,
                        row + i as u16,
                        &span.content,
                        span.style,
                        rect.width.saturating_sub(x - rect.x),
                    );
                    x += w;
                }
            }
            row += (tool_orchestra.agents.len() * 2 + 1) as u16 + 1;
        }

        if let Some(attention_lease) = &state.attention_lease {
            for (i, line) in attention_lease
                .render(theme, rect.width)
                .into_iter()
                .enumerate()
            {
                if row + i as u16 >= rect.bottom() {
                    break;
                }
                let mut x = rect.x + 1;
                for span in line.spans {
                    let w = span.width() as u16;
                    draw_text(
                        buf,
                        x,
                        row + i as u16,
                        &span.content,
                        span.style,
                        rect.width.saturating_sub(x - rect.x),
                    );
                    x += w;
                }
            }
            row += attention_lease.options.len() as u16 + 5;
        }

        if let Some(diff_reactor) = &state.diff_reactor {
            for (i, line) in diff_reactor
                .render(theme, rect.width)
                .into_iter()
                .enumerate()
            {
                if row + i as u16 >= rect.bottom() {
                    break;
                }
                let mut x = rect.x + 1;
                for span in line.spans {
                    let w = span.width() as u16;
                    draw_text(
                        buf,
                        x,
                        row + i as u16,
                        &span.content,
                        span.style,
                        rect.width.saturating_sub(x - rect.x),
                    );
                    x += w;
                }
            }
        }
    }

    fn render_files_drawer(buf: &mut Buffer, rect: Rect, state: &ShellState, theme: &Theme) {
        fill_rect(buf, rect, theme.palette.panel_bg);
        let content = Rect::new(
            rect.x.saturating_add(1),
            rect.y.saturating_add(1),
            rect.width.saturating_sub(2),
            rect.height.saturating_sub(2),
        );
        if content.width == 0 || content.height == 0 {
            return;
        }
        draw_text(
            buf,
            content.x,
            content.y,
            &format!(" {} ", state.files.title),
            badge_style(
                matches!(state.focus, ShellFocus::Files),
                theme.palette.explorer_accent,
                theme,
            ),
            content.width,
        );
        let mut row = content.y + 2;
        for line in &state.files.lines {
            if row >= content.bottom() {
                break;
            }
            draw_text(
                buf,
                content.x,
                row,
                &truncate_fit(line, content.width as usize),
                Style::default()
                    .fg(theme.palette.text_primary)
                    .bg(theme.palette.panel_bg),
                content.width,
            );
            row = row.saturating_add(1);
        }
    }

    fn render_terminal_bar(buf: &mut Buffer, rect: Rect, state: &ShellState, theme: &Theme) {
        fill_rect(buf, rect, theme.palette.terminal_bg);
        let label = if state.terminal.visible {
            let mode = if state.terminal.capture_mode {
                "CAPTURE"
            } else {
                "NAV"
            };
            let shell_label = state.terminal.badge_label.as_deref().unwrap_or("shell");
            format!(" {}  {}  {} ", state.terminal.title, shell_label, mode)
        } else {
            format!(" {} ", state.terminal.collapsed_label)
        };
        draw_text(
            buf,
            rect.x + 1,
            rect.y,
            &label,
            badge_style(
                matches!(state.focus, ShellFocus::Terminal),
                theme.palette.terminal_accent,
                theme,
            ),
            rect.width.saturating_sub(2),
        );
    }

    fn render_terminal_drawer(buf: &mut Buffer, rect: Rect, state: &ShellState, theme: &Theme) {
        fill_rect(buf, rect, theme.palette.terminal_bg);
        let inner = Rect::new(
            rect.x + 1,
            rect.y + 1,
            rect.width.saturating_sub(2),
            rect.height.saturating_sub(2),
        );
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        if let Some(snapshot) = state.terminal.snapshot.as_ref() {
            snapshot.render(
                buf,
                inner,
                theme.palette.terminal_bg,
                matches!(state.focus, ShellFocus::Terminal) && state.terminal.capture_mode,
            );
            return;
        }

        let mut row = inner.y;
        for line in &state.terminal.lines {
            if row >= inner.bottom() {
                break;
            }
            draw_text(
                buf,
                inner.x,
                row,
                &truncate_fit(line, inner.width as usize),
                Self::terminal_line_style(line, theme),
                inner.width,
            );
            row = row.saturating_add(1);
        }
    }

    fn render_footer(buf: &mut Buffer, rect: Rect, state: &ShellState, theme: &Theme) {
        fill_rect(buf, rect, theme.palette.inset_bg);
        let focus = format!(" {} ", state.active_mode);
        draw_text(
            buf,
            rect.x,
            rect.y,
            &focus,
            Style::default()
                .fg(theme.palette.text_primary)
                .bg(theme.palette.status_blue)
                .add_modifier(Modifier::BOLD),
            focus.chars().count() as u16,
        );
        draw_text(
            buf,
            rect.x + focus.chars().count() as u16,
            rect.y,
            &truncate_fit(
                &state.status_hint,
                rect.width.saturating_sub(focus.chars().count() as u16 + 1) as usize,
            ),
            Style::default()
                .fg(theme.palette.text_primary)
                .bg(theme.palette.inset_bg),
            rect.width.saturating_sub(focus.chars().count() as u16 + 1),
        );
    }

    fn render_overlay(buf: &mut Buffer, rect: Rect, overlay: &ShellOverlay, theme: &Theme) {
        draw_text(
            buf,
            rect.x + 1,
            rect.y,
            &overlay.title,
            Style::default()
                .fg(theme.palette.text_primary)
                .bg(theme.palette.panel_bg)
                .add_modifier(Modifier::BOLD),
            rect.width.saturating_sub(2),
        );
        let mut row = rect.y + 2;
        for line in &overlay.lines {
            if row >= rect.bottom().saturating_sub(1) {
                break;
            }
            draw_text(
                buf,
                rect.x + 1,
                row,
                &truncate_fit(line.as_ref(), rect.width.saturating_sub(2) as usize),
                Style::default()
                    .fg(theme.palette.text_primary)
                    .bg(theme.palette.panel_bg),
                rect.width.saturating_sub(2),
            );
            row = row.saturating_add(1);
        }
    }

    fn render_bootstrap(buf: &mut Buffer, area: Rect, state: &ShellState, theme: &Theme) {
        fill_rect(buf, area, theme.palette.canvas_bg);
        let Some(bootstrap) = state.bootstrap.as_ref() else {
            return;
        };
        match BootstrapLayoutMode::for_area(area) {
            BootstrapLayoutMode::Compact => {
                Self::render_bootstrap_compact(buf, area, bootstrap, theme)
            }
            BootstrapLayoutMode::Standard
            | BootstrapLayoutMode::Full
            | BootstrapLayoutMode::Cinema => {
                Self::render_bootstrap_wide(buf, area, bootstrap, theme)
            }
        }
    }

    fn render_bootstrap_wide(
        buf: &mut Buffer,
        area: Rect,
        bootstrap: &ShellBootstrapView,
        theme: &Theme,
    ) {
        let margin_x = area.width / 24;
        let margin_y = area.height / 12;
        let inner = Rect::new(
            area.x + margin_x,
            area.y + margin_y,
            area.width.saturating_sub(margin_x * 2),
            area.height.saturating_sub(margin_y * 2),
        );
        let gutter = (inner.width / 24).max(3);
        let art_width = ((inner.width as f32) * 0.32).round() as u16;
        let art_width = art_width.clamp(28, inner.width.saturating_sub(34));
        let left_width = inner.width.saturating_sub(art_width).saturating_sub(gutter);
        let brand_block_height = (inner.height / 2).max(11);
        let brand_rect = Rect::new(inner.x, inner.y, left_width, brand_block_height);
        let art_rect = Rect::new(
            inner.x + left_width + gutter,
            inner.y,
            art_width,
            inner.height,
        );
        let probe_rect = Rect::new(
            inner.x,
            brand_rect.bottom().saturating_add(1),
            left_width,
            inner
                .bottom()
                .saturating_sub(brand_rect.bottom().saturating_add(1)),
        );
        Self::render_bootstrap_branding(buf, brand_rect, bootstrap, theme);
        Self::render_bootstrap_mascot(buf, art_rect, bootstrap, theme);
        Self::render_bootstrap_probes(buf, probe_rect, bootstrap, theme);
    }

    fn render_bootstrap_compact(
        buf: &mut Buffer,
        area: Rect,
        bootstrap: &ShellBootstrapView,
        theme: &Theme,
    ) {
        let margin_x = 2.min(area.width / 8);
        let margin_y = 1.min(area.height / 12);
        let inner = Rect::new(
            area.x + margin_x,
            area.y + margin_y,
            area.width.saturating_sub(margin_x * 2),
            area.height.saturating_sub(margin_y * 2),
        );
        let art_height = (inner.height / 3).max(10);
        let brand_height = 9.min(inner.height.saturating_sub(art_height + 4)).max(6);
        let art_rect = Rect::new(inner.x, inner.y, inner.width, art_height);
        let brand_rect = Rect::new(inner.x, inner.y + art_height + 1, inner.width, brand_height);
        let probe_rect = Rect::new(
            inner.x,
            brand_rect.bottom().saturating_add(1),
            inner.width,
            inner
                .bottom()
                .saturating_sub(brand_rect.bottom().saturating_add(1)),
        );
        Self::render_bootstrap_mascot(buf, art_rect, bootstrap, theme);
        Self::render_bootstrap_branding(buf, brand_rect, bootstrap, theme);
        Self::render_bootstrap_probes(buf, probe_rect, bootstrap, theme);
    }

    fn render_bootstrap_branding(
        buf: &mut Buffer,
        rect: Rect,
        bootstrap: &ShellBootstrapView,
        theme: &Theme,
    ) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        let frame = &bootstrap.loader_frame;
        let ember_gold = Color::Rgb(0xFF, 0xC3, 0x56);
        let flame_orange = Color::Rgb(0xFF, 0x95, 0x22);
        let inferno_orange = Color::Rgb(0xFF, 0x6A, 0x1A);
        let spark_orange = Color::Rgb(0xFF, 0xB1, 0x3B);
        let warm_ash = Color::Rgb(0xE9, 0xD8, 0xC7);
        let muted_ember = Color::Rgb(0xC7, 0x9A, 0x73);
        let title_colors = [
            ember_gold,
            spark_orange,
            flame_orange,
            inferno_orange,
            warm_ash,
        ];
        for (index, line) in frame.wordmark.lines.iter().enumerate() {
            let y = rect.y + index as u16;
            if y >= rect.bottom() {
                break;
            }
            let x = rect.x.saturating_add(line.offset.max(0) as u16);
            let color = title_colors[(index + frame.wordmark.accent_phase) % title_colors.len()];
            draw_text(
                buf,
                x,
                y,
                &truncate_fit(&line.text, rect.width as usize),
                Style::default()
                    .fg(color)
                    .bg(theme.palette.editor_bg)
                    .add_modifier(Modifier::BOLD),
                rect.width.saturating_sub(x.saturating_sub(rect.x)),
            );
        }

        let version_y = rect.y + frame.wordmark.lines.len() as u16 + 1;
        if version_y < rect.bottom() {
            draw_text(
                buf,
                rect.x,
                version_y,
                "v2.01",
                Style::default()
                    .fg(ember_gold)
                    .bg(theme.palette.editor_bg)
                    .add_modifier(Modifier::BOLD),
                rect.width,
            );
        }
        let badge_y = version_y.saturating_add(1);
        if badge_y < rect.bottom() {
            let badge_text = format!(" {} ", frame.phase_badge);
            draw_text(
                buf,
                rect.x,
                badge_y,
                &badge_text,
                Style::default()
                    .fg(theme.palette.editor_bg)
                    .bg(flame_orange)
                    .add_modifier(Modifier::BOLD),
                rect.width,
            );
        }
        let subtitle_y = badge_y.saturating_add(2);
        if subtitle_y < rect.bottom() {
            draw_text(
                buf,
                rect.x,
                subtitle_y,
                &bootstrap.subtitle,
                Style::default().fg(warm_ash).bg(theme.palette.editor_bg),
                rect.width,
            );
        }
        let subtitle_hint_y = subtitle_y.saturating_add(2);
        if subtitle_hint_y < rect.bottom() {
            draw_text(
                buf,
                rect.x,
                subtitle_hint_y,
                "Igniting the shared local runtime and session state.",
                Style::default()
                    .fg(muted_ember)
                    .bg(theme.palette.editor_bg)
                    .add_modifier(Modifier::BOLD),
                rect.width,
            );
        }
    }

    fn render_bootstrap_mascot(
        buf: &mut Buffer,
        rect: Rect,
        bootstrap: &ShellBootstrapView,
        theme: &Theme,
    ) {
        let art = &bootstrap.loader_frame.mascot.rows;
        let art_width = art.iter().map(|row| row.len() as u16).max().unwrap_or(0);
        let art_height = art.len() as u16;
        if art_width == 0 || art_height == 0 || rect.width == 0 || rect.height == 0 {
            return;
        }
        let x = rect.x + rect.width.saturating_sub(art_width) / 2;
        let y = rect.y + rect.height.saturating_sub(art_height) / 2;
        Self::render_brand_art(buf, x, y, art, theme.palette.editor_bg);
    }

    fn render_bootstrap_watermark(
        buf: &mut Buffer,
        rect: Rect,
        bootstrap: &ShellBootstrapView,
        theme: &Theme,
    ) {
        let art = &bootstrap.loader_frame.mascot.rows;
        let art_width = art.iter().map(|row| row.len() as u16).max().unwrap_or(0);
        let art_height = art.len() as u16;
        if art_width == 0 || art_height == 0 || rect.width <= 4 || rect.height <= 4 {
            return;
        }
        let x = rect.x
            + rect
                .width
                .saturating_sub(art_width.min(rect.width.saturating_sub(2)))
                / 2;
        let y = rect.y
            + rect
                .height
                .saturating_sub(art_height.min(rect.height.saturating_sub(2)))
                / 2;
        for (row_index, row) in art.iter().enumerate() {
            for (column_index, cell) in row.iter().enumerate() {
                if cell.symbol == ' ' {
                    continue;
                }
                let target_x = x.saturating_add(column_index as u16);
                let target_y = y.saturating_add(row_index as u16);
                if target_x >= rect.right() || target_y >= rect.bottom() {
                    continue;
                }
                if let Some(buffer_cell) = buf.cell_mut((target_x, target_y)) {
                    buffer_cell
                        .set_char(if (row_index + column_index) % 3 == 0 {
                            '·'
                        } else {
                            cell.symbol
                        })
                        .set_fg(theme.palette.grid_line)
                        .set_bg(theme.palette.sidebar_bg);
                }
            }
        }
    }

    fn render_bootstrap_probes(
        buf: &mut Buffer,
        rect: Rect,
        bootstrap: &ShellBootstrapView,
        theme: &Theme,
    ) {
        let footer_ember = Color::Rgb(0xA7, 0x7A, 0x57);
        let mut row = rect.y;
        for probe in &bootstrap.probes {
            if row >= rect.bottom() {
                break;
            }
            let (badge, tone) = match probe.status {
                BootstrapStatus::Pending => ("[...] ", theme.palette.warning_yellow),
                BootstrapStatus::Ok => ("[ok]  ", theme.palette.success_green),
                BootstrapStatus::Warn => ("[warn]", theme.palette.warning_yellow),
                BootstrapStatus::Failed => ("[fail]", theme.palette.danger_orange),
            };
            draw_text(
                buf,
                rect.x,
                row,
                badge,
                Style::default().fg(tone).bg(theme.palette.editor_bg),
                rect.width,
            );
            let label_x = rect.x + badge.chars().count() as u16 + 1;
            draw_text(
                buf,
                label_x,
                row,
                &format!("{}  {}", probe.label, probe.detail),
                Style::default()
                    .fg(theme.palette.text_primary)
                    .bg(theme.palette.editor_bg),
                rect.width.saturating_sub(label_x.saturating_sub(rect.x)),
            );
            row = row.saturating_add(1);
        }

        if row < rect.bottom() {
            draw_text(
                buf,
                rect.x,
                rect.bottom().saturating_sub(1),
                &bootstrap.footer,
                Style::default()
                    .fg(footer_ember)
                    .bg(theme.palette.editor_bg),
                rect.width,
            );
        }
    }

    fn render_structure_lines(
        buf: &mut Buffer,
        geometry: &ShellGeometry,
        state: &ShellState,
        theme: &Theme,
    ) {
        if geometry.sidebar.width > 0 {
            let sidebar_color =
                Self::outline_color(matches!(state.focus, ShellFocus::Sidebar), theme);
            Self::draw_horizontal_rule(
                buf,
                geometry.sidebar.y,
                geometry.sidebar.x,
                geometry.sidebar.width,
                sidebar_color,
            );
            Self::draw_horizontal_rule(
                buf,
                geometry.sidebar.bottom().saturating_sub(1),
                geometry.sidebar.x,
                geometry.sidebar.width,
                sidebar_color,
            );
            Self::draw_vertical_rule(
                buf,
                geometry.sidebar.x,
                geometry.sidebar.y,
                geometry.sidebar.height,
                sidebar_color,
            );
        }

        let center_color = Self::outline_color(
            matches!(
                state.focus,
                ShellFocus::Feed | ShellFocus::Main | ShellFocus::Assistant
            ),
            theme,
        );
        Self::draw_horizontal_rule(
            buf,
            geometry.center.y,
            geometry.center.x,
            geometry.center.width,
            center_color,
        );
        Self::draw_horizontal_rule(
            buf,
            geometry.center.bottom().saturating_sub(1),
            geometry.center.x,
            geometry.center.width,
            center_color,
        );
        Self::draw_vertical_rule(
            buf,
            geometry.center.x,
            geometry.center.y,
            geometry.center.height,
            center_color,
        );
        if geometry.files_drawer.is_none() {
            Self::draw_vertical_rule(
                buf,
                geometry.center.right().saturating_sub(1),
                geometry.center.y,
                geometry.center.height,
                center_color,
            );
        }

        if let Some(files_drawer) = geometry.files_drawer {
            let files_color = Self::outline_color(matches!(state.focus, ShellFocus::Files), theme);
            Self::draw_horizontal_rule(
                buf,
                files_drawer.y,
                files_drawer.x,
                files_drawer.width,
                files_color,
            );
            Self::draw_horizontal_rule(
                buf,
                files_drawer.bottom().saturating_sub(1),
                files_drawer.x,
                files_drawer.width,
                files_color,
            );
            Self::draw_vertical_rule(
                buf,
                files_drawer.x,
                files_drawer.y,
                files_drawer.height,
                files_color,
            );
            Self::draw_vertical_rule(
                buf,
                files_drawer.right().saturating_sub(1),
                files_drawer.y,
                files_drawer.height,
                files_color,
            );
        }
        if let Some(terminal_drawer) = geometry.terminal_drawer {
            Self::draw_outline(
                buf,
                terminal_drawer,
                Self::outline_color(matches!(state.focus, ShellFocus::Terminal), theme),
            );
        }
    }

    fn terminal_line_style(line: &str, theme: &Theme) -> Style {
        let trimmed = line.trim_start();
        let foreground = if trimmed.starts_with('$')
            || trimmed.starts_with('%')
            || trimmed.starts_with('>')
            || trimmed.starts_with("➜")
        {
            theme.palette.terminal_prompt_fg
        } else {
            theme.palette.text_muted
        };
        Style::default()
            .fg(foreground)
            .bg(theme.palette.terminal_bg)
    }

    fn outline_color(focused: bool, theme: &Theme) -> Color {
        if focused {
            theme.palette.grid_line_focus
        } else {
            theme.palette.grid_line
        }
    }

    fn draw_horizontal_rule(buf: &mut Buffer, y: u16, x: u16, width: u16, color: Color) {
        if width == 0 {
            return;
        }
        for column in x..x.saturating_add(width) {
            if let Some(cell) = buf.cell_mut((column, y)) {
                cell.set_char('─').set_fg(color);
            }
        }
    }

    fn draw_vertical_rule(buf: &mut Buffer, x: u16, y: u16, height: u16, color: Color) {
        if height == 0 {
            return;
        }
        for row in y..y.saturating_add(height) {
            if let Some(cell) = buf.cell_mut((x, row)) {
                cell.set_char('│').set_fg(color);
            }
        }
    }

    fn draw_outline(buf: &mut Buffer, rect: Rect, color: Color) {
        if rect.width < 2 || rect.height < 2 {
            return;
        }
        for x in rect.left()..rect.right() {
            if let Some(cell) = buf.cell_mut((x, rect.y)) {
                cell.set_char('─').set_fg(color);
            }
            if let Some(cell) = buf.cell_mut((x, rect.bottom().saturating_sub(1))) {
                cell.set_char('─').set_fg(color);
            }
        }
        for y in rect.top()..rect.bottom() {
            if let Some(cell) = buf.cell_mut((rect.x, y)) {
                cell.set_char('│').set_fg(color);
            }
            if let Some(cell) = buf.cell_mut((rect.right().saturating_sub(1), y)) {
                cell.set_char('│').set_fg(color);
            }
        }
        if let Some(cell) = buf.cell_mut((rect.x, rect.y)) {
            cell.set_char('┌').set_fg(color);
        }
        if let Some(cell) = buf.cell_mut((rect.right().saturating_sub(1), rect.y)) {
            cell.set_char('┐').set_fg(color);
        }
        if let Some(cell) = buf.cell_mut((rect.x, rect.bottom().saturating_sub(1))) {
            cell.set_char('└').set_fg(color);
        }
        if let Some(cell) = buf.cell_mut((
            rect.right().saturating_sub(1),
            rect.bottom().saturating_sub(1),
        )) {
            cell.set_char('┘').set_fg(color);
        }
    }

    fn render_brand_art(
        buf: &mut Buffer,
        x: u16,
        y: u16,
        art: &[Vec<BrandArtCell>],
        fallback_bg: Color,
    ) {
        for (row_index, row) in art.iter().enumerate() {
            for (column_index, cell) in row.iter().enumerate() {
                if let Some(buffer_cell) =
                    buf.cell_mut((x + column_index as u16, y + row_index as u16))
                {
                    buffer_cell
                        .set_char(cell.symbol)
                        .set_fg(match cell.symbol {
                            ' ' => Color::Reset,
                            _ => cell.fg,
                        })
                        .set_bg(match cell.symbol {
                            ' ' => fallback_bg,
                            _ => cell.bg,
                        });
                }
            }
        }
    }
}

fn badge_style(focused: bool, accent: Color, theme: &Theme) -> Style {
    let background = if focused {
        accent
    } else {
        theme.palette.panel_bg
    };
    let foreground = if focused {
        theme.palette.canvas_bg
    } else {
        theme.palette.text_primary
    };
    Style::default()
        .fg(foreground)
        .bg(background)
        .add_modifier(Modifier::BOLD)
}

fn project_status_color(status: &str, theme: &Theme) -> Color {
    if status.eq_ignore_ascii_case("idle") {
        theme.palette.runtime_transition
    } else if status.eq_ignore_ascii_case("working") || status.eq_ignore_ascii_case("online") {
        theme.palette.runtime_online
    } else if status.eq_ignore_ascii_case("queued") {
        theme.palette.warning_yellow
    } else if status.eq_ignore_ascii_case("interrupted") {
        theme.palette.danger_orange
    } else {
        theme.palette.text_primary
    }
}

fn project_status_style(status: &str, theme: &Theme) -> Color {
    project_status_color(status, theme)
}

fn project_status_indicator(status: &str, phase: u8, _color: Color) -> String {
    if !status.eq_ignore_ascii_case("working") {
        return format!("▏ {} ", status);
    }
    let width = 12usize;
    let pulse = (phase as usize) % (width.saturating_mul(2).max(1));
    let head = if pulse < width {
        pulse
    } else {
        width.saturating_mul(2).saturating_sub(1 + pulse)
    };
    let mut bar = String::with_capacity(width.saturating_add(10));
    for column in 0..width {
        if column == head {
            bar.push('▀');
        } else if column == head.saturating_sub(1) || column == head.saturating_add(1) {
            bar.push('▌');
        } else {
            bar.push('━');
        }
    }
    format!("▎{bar} Working")
}

fn thread_status_color(status: &str, theme: &Theme) -> Color {
    if status.eq_ignore_ascii_case("idle") {
        theme.palette.runtime_transition
    } else if status.eq_ignore_ascii_case("working") || status.eq_ignore_ascii_case("online") {
        theme.palette.chat_accent
    } else if status.eq_ignore_ascii_case("queued") {
        theme.palette.warning_yellow
    } else if status.eq_ignore_ascii_case("interrupted") {
        theme.palette.danger_orange
    } else {
        theme.palette.text_primary
    }
}

fn thread_status_style(status: &str, theme: &Theme) -> Color {
    thread_status_color(status, theme)
}

fn thread_indicator(status: &str, phase: u8) -> String {
    if !status.eq_ignore_ascii_case("working") {
        return format!("▏ {} ", status);
    }
    let width = 10usize;
    let pulse = (phase as usize) % (width.saturating_mul(2).max(1));
    let head = if pulse < width {
        pulse
    } else {
        width.saturating_mul(2).saturating_sub(1 + pulse)
    };
    let mut bar = String::with_capacity(width.saturating_add(8));
    for column in 0..width {
        if column == head {
            bar.push('█');
        } else if column == head.saturating_add(1) {
            bar.push('▎');
        } else if column == head.saturating_sub(1) {
            bar.push('▍');
        } else {
            bar.push('━');
        }
    }
    format!("▏{bar} Working")
}

impl ShellState {
    pub fn for_scenario(scenario: ShellScenario, area: Rect) -> Self {
        let theme = Theme::core_tui();
        match scenario {
            ShellScenario::Startup => Self {
                scene: ShellScene::Bootstrap,
                experience_mode: ShellExperienceMode::Bootstrap,
                app_name: "quorp".to_string(),
                version_label: "v2.01".to_string(),
                active_mode: "Bootstrap".to_string(),
                focus: ShellFocus::Overlay,
                status_hint: "Boot continues automatically once hard checks pass.".to_string(),
                workspace_root: "/workspace/fixture-project".to_string(),
                proof_rail_visible: true,
                diff_reactor: None,
                attention_lease: None,
                tool_orchestra: None,
                runtime_label: "Starting".to_string(),
                explorer_visible: false,
                assistant_overlay: false,
                explorer_items: Vec::new(),
                main: ShellMainView {
                    title: String::new(),
                    mode: MainWorkspaceMode::Preview,
                    lines: Vec::new(),
                    terminal_title: String::new(),
                    terminal_lines: Vec::new(),
                    show_terminal_drawer: false,
                },
                assistant: ShellAssistantView {
                    session_label: String::new(),
                    runtime_label: String::new(),
                    transcript: Vec::new(),
                    composer_text: String::new(),
                },
                main_sessions: Vec::new(),
                assistant_sessions: Vec::new(),
                sidebar: ShellSidebarView {
                    projects: Vec::new(),
                    threads: Vec::new(),
                    active_project_root: "/workspace/fixture-project".to_string(),
                },
                center: ShellCenterView {
                    thread_title: "Loading".to_string(),
                    project_label: "fixture-project".to_string(),
                    workspace_label: "/workspace/fixture-project".to_string(),
                    provider_label: "Local".to_string(),
                    runtime_label: "Starting".to_string(),
                    model_label: "qwen3-coder-30b-a3b".to_string(),
                    session_identity: "Assistant Loading · act".to_string(),
                    runtime_model_label: "qwen3-coder-30b-a3b".to_string(),
                    runtime_state_label: "Starting".to_string(),
                    runtime_state_kind: ShellRuntimeStateKind::Transition,
                    animation_phase: 0,
                    feed: Vec::new(),
                    feed_scroll_top: 0,
                    feed_total_lines: 1,
                    feed_viewport_lines: 1,
                    feed_scrollbar_hovered: false,
                    feed_lines: Vec::new(),
                    feed_links: Vec::new(),
                    active_feed_link: None,
                    composer_text: String::new(),
                    additions: 0,
                    deletions: 0,
                },
                files: ShellDrawerView {
                    title: "Files".to_string(),
                    collapsed_label: "Files".to_string(),
                    visible: false,
                    badge_label: None,
                    detail_label: None,
                    lines: Vec::new(),
                    snapshot: None,
                    fullscreen: false,
                    capture_mode: false,
                },
                terminal: ShellDrawerView {
                    title: "Terminal".to_string(),
                    collapsed_label: "Terminal".to_string(),
                    visible: false,
                    badge_label: Some("zsh".to_string()),
                    detail_label: Some("/workspace/fixture-project".to_string()),
                    lines: Vec::new(),
                    snapshot: None,
                    fullscreen: false,
                    capture_mode: true,
                },
                proof_rail: Some(ProofRailState::default()),
                overlay: None,
                bootstrap: Some(ShellBootstrapView {
                    subtitle:
                        "Verifying the terminal, workspace, local model runtime, and session state."
                            .to_string(),
                    probes: vec![
                        BootstrapProbe {
                            label: "Terminal".to_string(),
                            status: BootstrapStatus::Ok,
                            detail: "alternate screen ready".to_string(),
                        },
                        BootstrapProbe {
                            label: "Workspace".to_string(),
                            status: BootstrapStatus::Ok,
                            detail: "/workspace/fixture-project".to_string(),
                        },
                        BootstrapProbe {
                            label: "SSD-MOE".to_string(),
                            status: BootstrapStatus::Pending,
                            detail: "attach-or-spawn in progress".to_string(),
                        },
                    ],
                    footer: "Boot continues automatically once hard checks pass.".to_string(),
                    loader_frame: crate::quorp::tui::bootstrap_loader::BootstrapLoader::frame(
                        area,
                        crate::quorp::tui::bootstrap_loader::BOOTSTRAP_REVEAL_FRAMES
                            .saturating_sub(1),
                        "Starting",
                        &theme,
                    ),
                }),
            },
            _ => Self {
                scene: ShellScene::Ready,
                experience_mode: ShellExperienceMode::CommandCenter,
                app_name: "quorp".to_string(),
                version_label: "v2.01".to_string(),
                active_mode: "COMMAND CENTER".to_string(),
                focus: ShellFocus::Feed,
                status_hint:
                    "/ workflow deck  ·  Ctrl+k control deck  ·  d diff  v verify  r trace  t timeline  m memory"
                        .to_string(),
                workspace_root: "/workspace/quorp".to_string(),
                proof_rail_visible: true,
                diff_reactor: None,
                attention_lease: None,
                tool_orchestra: None,
                runtime_label: "Runtime ready".to_string(),
                explorer_visible: false,
                assistant_overlay: false,
                explorer_items: vec![ShellExplorerItem {
                    label: "src/main.rs".to_string(),
                    selected: true,
                }],
                main: ShellMainView {
                    title: "src/main.rs".to_string(),
                    mode: MainWorkspaceMode::Preview,
                    lines: vec!["fn main() {}".to_string()],
                    terminal_title: "Terminal".to_string(),
                    terminal_lines: vec!["$ cargo test -p quorp".to_string()],
                    show_terminal_drawer: false,
                },
                assistant: ShellAssistantView {
                    session_label: "Thread".to_string(),
                    runtime_label: "Runtime ready".to_string(),
                    transcript: vec![AssistantBlock {
                        role: "Assistant:",
                        text: "Chat-first shell is active.".to_string(),
                        tone: AssistantTone::Normal,
                        rich_lines: None,
                    }],
                    composer_text: "Ask for follow-up changes".to_string(),
                },
                main_sessions: Vec::new(),
                assistant_sessions: Vec::new(),
                sidebar: ShellSidebarView {
                    projects: vec![ShellProjectItem {
                        label: "quorp".to_string(),
                        status: "Working".to_string(),
                        selected: true,
                    }],
                    threads: vec![ShellThreadItem {
                        label: "Codex layout".to_string(),
                        summary: "Implementing new shell".to_string(),
                        status: "Working".to_string(),
                        additions: 42,
                        deletions: 7,
                        selected: true,
                    }],
                    active_project_root: "/workspace/quorp".to_string(),
                },
                center: ShellCenterView {
                    thread_title: "Codex layout".to_string(),
                    project_label: "quorp".to_string(),
                    workspace_label: "/workspace/quorp".to_string(),
                    provider_label: "Local".to_string(),
                    runtime_label: "Runtime ready".to_string(),
                    model_label: "qwen3-coder-30b-a3b".to_string(),
                    session_identity: "Assistant Thread · act".to_string(),
                    runtime_model_label: "qwen3-coder-30b-a3b".to_string(),
                    runtime_state_label: "Online".to_string(),
                    runtime_state_kind: ShellRuntimeStateKind::Online,
                    animation_phase: 0,
                    feed: vec![ShellFeedItem {
                        title: "Assistant".to_string(),
                        lines: vec!["Chat-first shell is active.".to_string()],
                        rich_lines: None,
                        tone: FeedItemTone::Assistant,
                        additions: 0,
                        deletions: 0,
                    }],
                    feed_scroll_top: 0,
                    feed_total_lines: 1,
                    feed_viewport_lines: 1,
                    feed_scrollbar_hovered: false,
                    feed_lines: Vec::new(),
                    feed_links: Vec::new(),
                    active_feed_link: None,
                    composer_text: "Ask for follow-up changes".to_string(),
                    additions: 42,
                    deletions: 7,
                },
                files: ShellDrawerView {
                    title: "Files".to_string(),
                    collapsed_label: "Files".to_string(),
                    visible: false,
                    badge_label: None,
                    detail_label: None,
                    lines: vec!["src/main.rs".to_string()],
                    snapshot: None,
                    fullscreen: false,
                    capture_mode: false,
                },
                terminal: ShellDrawerView {
                    title: "Terminal".to_string(),
                    collapsed_label: "Terminal".to_string(),
                    visible: false,
                    badge_label: Some("zsh".to_string()),
                    detail_label: Some("/workspace/quorp".to_string()),
                    lines: vec!["$ cargo test -p quorp".to_string()],
                    snapshot: None,
                    fullscreen: false,
                    capture_mode: true,
                },
                proof_rail: Some(ProofRailState::default()),
                overlay: None,
                bootstrap: None,
            },
        }
    }

    pub fn render_feed_lines(
        feed: &[ShellFeedItem],
        theme: &Theme,
        max_width: usize,
    ) -> RenderedFeed {
        if max_width == 0 {
            return RenderedFeed {
                lines: Vec::new(),
                links: Vec::new(),
            };
        }

        let mut lines = Vec::new();
        let mut links = Vec::new();
        let link_color = match theme.palette.link_blue {
            Color::Rgb(red, green, blue) => Color::Rgb(red.max(120), green.max(200), blue.max(230)),
            other => other,
        };
        let link_style = Style::default()
            .fg(link_color)
            .bg(theme.palette.panel_bg)
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::UNDERLINED);

        for item in feed {
            let badge = match item.tone {
                FeedItemTone::User => "USER",
                FeedItemTone::Assistant => "ASSIST",
                FeedItemTone::Reasoning => "THINK",
                FeedItemTone::Tool => "TOOL",
                FeedItemTone::Command => "SHELL",
                FeedItemTone::Validation => "CHECK",
                FeedItemTone::Muted => "INFO",
                FeedItemTone::Warning => "WARN",
                FeedItemTone::Error => "ERROR",
                FeedItemTone::Success => "RUN",
                FeedItemTone::FileChange => "FILES",
            };
            let badge_style = match item.tone {
                FeedItemTone::User => theme.palette.accent_blue,
                FeedItemTone::Assistant => theme.palette.chat_accent,
                FeedItemTone::Reasoning => theme.palette.secondary_teal,
                FeedItemTone::Tool => theme.palette.status_blue,
                FeedItemTone::Command => theme.palette.terminal_accent,
                FeedItemTone::Validation => theme.palette.warning_yellow,
                FeedItemTone::Muted => theme.palette.text_faint,
                FeedItemTone::Warning => theme.palette.warning_yellow,
                FeedItemTone::Error => theme.palette.danger_orange,
                FeedItemTone::Success => theme.palette.success_green,
                FeedItemTone::FileChange => theme.palette.warning_yellow,
            };
            let title_style = Style::default()
                .fg(theme.palette.text_primary)
                .bg(theme.palette.panel_bg)
                .add_modifier(Modifier::BOLD);
            let body_fg = match item.tone {
                FeedItemTone::User => theme.palette.text_primary,
                FeedItemTone::Assistant => theme.palette.text,
                FeedItemTone::Reasoning => theme.palette.secondary_teal,
                FeedItemTone::Tool => theme.palette.text_primary,
                FeedItemTone::Command => theme.palette.terminal_accent,
                FeedItemTone::Validation => theme.palette.warning_yellow,
                FeedItemTone::Error => theme.palette.danger_orange,
                FeedItemTone::Muted => theme.palette.text_muted,
                FeedItemTone::Warning => theme.palette.warning_yellow,
                FeedItemTone::Success => theme.palette.runtime_online,
                FeedItemTone::FileChange => theme.palette.text_primary,
            };
            let body_style = Style::default().fg(body_fg).bg(theme.palette.panel_bg);
            let stat_suffix = if item.additions > 0 || item.deletions > 0 {
                format!("  +{} -{}", item.additions, item.deletions)
            } else {
                String::new()
            };
            let title = format!("{}{}", item.title, stat_suffix);
            let header = Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!(" {} ", badge),
                    Style::default()
                        .fg(theme.palette.canvas_bg)
                        .bg(badge_style)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(title, title_style),
            ]);
            Self::append_wrapped_segments(
                &Self::line_to_segments_with_links(&header, body_style, link_style),
                max_width,
                &mut lines,
                &mut links,
            );

            if let Some(rich_lines) = item.rich_lines.as_ref() {
                for line in rich_lines {
                    let indented = Self::indent_line(line.clone(), "  ", body_style);
                    Self::append_wrapped_segments(
                        &Self::line_to_segments_with_links(&indented, body_style, link_style),
                        max_width,
                        &mut lines,
                        &mut links,
                    );
                }
            } else {
                for line in &item.lines {
                    let rendered = Line::from(vec![
                        Span::styled("  ", body_style),
                        Span::styled(line.clone(), body_style),
                    ]);
                    Self::append_wrapped_segments(
                        &Self::line_to_segments_with_links(&rendered, body_style, link_style),
                        max_width,
                        &mut lines,
                        &mut links,
                    );
                }
            }

            lines.push(Line::from(""));
        }

        if lines.is_empty() {
            lines.push(Line::from(""));
        }
        RenderedFeed { lines, links }
    }

    fn line_to_segments_with_links(
        line: &Line<'static>,
        body_style: Style,
        link_style: Style,
    ) -> Vec<FeedLineSegment> {
        let mut segments = Vec::new();
        let raw = line
            .spans
            .iter()
            .map(|span| (span.content.to_string(), span.style))
            .collect::<Vec<_>>();

        for (text, style) in raw {
            if text.is_empty() {
                continue;
            }
            let parsed = Self::parse_markdown_links(&text, style, link_style);
            for segment in parsed {
                segments.extend(Self::split_path_like_tokens(
                    segment.text,
                    segment.style,
                    segment.link_target,
                    link_style,
                ));
            }
        }

        if segments.is_empty() {
            vec![FeedLineSegment {
                text: String::new(),
                style: body_style,
                link_target: None,
            }]
        } else {
            segments
        }
    }

    fn parse_markdown_links(
        text: &str,
        body_style: Style,
        link_style: Style,
    ) -> Vec<FeedLineSegment> {
        let mut out = Vec::new();
        let mut search_start = 0usize;

        while search_start < text.len() {
            let Some(open_rel) = text[search_start..].find('[') else {
                let tail = text[search_start..].to_string();
                if !tail.is_empty() {
                    out.push(FeedLineSegment {
                        text: tail,
                        style: body_style,
                        link_target: None,
                    });
                }
                break;
            };

            let open = search_start + open_rel;
            if open > search_start {
                out.push(FeedLineSegment {
                    text: text[search_start..open].to_string(),
                    style: body_style,
                    link_target: None,
                });
            }

            let Some(close_rel) = text[open..].find(']') else {
                out.push(FeedLineSegment {
                    text: text[open..].to_string(),
                    style: body_style,
                    link_target: None,
                });
                break;
            };
            let close = open + close_rel;
            let Some(_paren_open) = text.get(close + 1..close + 2).filter(|c| *c == "(") else {
                out.push(FeedLineSegment {
                    text: text[open..=close].to_string(),
                    style: body_style,
                    link_target: None,
                });
                search_start = close + 1;
                continue;
            };
            let paren_open = close + 1;
            let Some(paren_close_rel) = text[paren_open..].find(')') else {
                out.push(FeedLineSegment {
                    text: text[open..].to_string(),
                    style: body_style,
                    link_target: None,
                });
                break;
            };
            let paren_close = paren_open + paren_close_rel;
            let label = text[open + 1..close].to_string();
            let target = text[paren_open + 1..paren_close].to_string();

            if Self::is_openable_link(&target) {
                out.push(FeedLineSegment {
                    text: label,
                    style: link_style,
                    link_target: Some(target),
                });
                search_start = paren_close + 1;
            } else {
                out.push(FeedLineSegment {
                    text: text[open..=paren_close].to_string(),
                    style: body_style,
                    link_target: None,
                });
                search_start = paren_close + 1;
            }
        }

        out
    }

    fn split_path_like_tokens(
        text: String,
        style: Style,
        link_target: Option<String>,
        link_style: Style,
    ) -> Vec<FeedLineSegment> {
        if link_target.is_some() {
            return vec![FeedLineSegment {
                text,
                style,
                link_target,
            }];
        }

        let mut segments = Vec::new();
        let mut segment_start = 0usize;
        let bytes = text.as_bytes();

        for (i, &byte) in bytes.iter().enumerate() {
            if byte.is_ascii_whitespace() {
                if segment_start < i {
                    let token = &text[segment_start..i];
                    segments.extend(Self::split_path_like_token(
                        token.to_string(),
                        style,
                        link_style,
                    ));
                }
                segments.push(FeedLineSegment {
                    text: text[i..i + 1].to_string(),
                    style,
                    link_target: None,
                });
                segment_start = i + 1;
            }
        }

        if segment_start < text.len() {
            let token = text[segment_start..].to_string();
            segments.extend(Self::split_path_like_token(token, style, link_style));
        }

        if segments.is_empty() {
            vec![FeedLineSegment {
                text,
                style,
                link_target,
            }]
        } else {
            segments
        }
    }

    fn split_path_like_token(
        token: String,
        style: Style,
        link_style: Style,
    ) -> Vec<FeedLineSegment> {
        let mut segments = Vec::new();
        if token.is_empty() {
            return segments;
        }

        let mut end = token.len();
        while let Some(last) = token[..end].chars().last() {
            if last == '.'
                || last == ','
                || last == ';'
                || last == ':'
                || last == ')'
                || last == ']'
                || last == '}'
                || last == '!'
                || last == '?'
            {
                end = end.saturating_sub(last.len_utf8());
                continue;
            }
            break;
        }
        if end == 0 {
            segments.push(FeedLineSegment {
                text: token,
                style,
                link_target: None,
            });
            return segments;
        }

        let core = &token[..end];
        let suffix = &token[end..];
        if Self::is_openable_link(core) {
            segments.push(FeedLineSegment {
                text: core.to_string(),
                style: link_style,
                link_target: Some(core.to_string()),
            });
        } else {
            segments.push(FeedLineSegment {
                text: core.to_string(),
                style,
                link_target: None,
            });
        }
        if !suffix.is_empty() {
            segments.push(FeedLineSegment {
                text: suffix.to_string(),
                style,
                link_target: None,
            });
        }

        segments
    }

    fn is_openable_link(candidate: &str) -> bool {
        if candidate == "/" {
            return false;
        }
        if candidate.starts_with("http://")
            || candidate.starts_with("https://")
            || candidate.starts_with("file://")
        {
            return true;
        }

        if candidate.starts_with('/')
            && candidate[1..].chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            return false;
        }

        if candidate.starts_with('/')
            || candidate.starts_with("./")
            || candidate.starts_with("../")
            || candidate.starts_with('~')
            || candidate.contains('\u{005C}')
        {
            return true;
        }

        if candidate.len() > 2 {
            let bytes = candidate.as_bytes();
            if bytes.len() > 2 && bytes[1] == b':' && bytes[2].is_ascii_alphabetic() {
                return true;
            }
        }

        candidate.contains('/') || candidate.contains('\\')
    }

    fn append_wrapped_segments(
        segments: &[FeedLineSegment],
        max_width: usize,
        lines: &mut Vec<Line<'static>>,
        links: &mut Vec<AssistantFeedLink>,
    ) {
        if segments.is_empty() {
            lines.push(Line::from(""));
            return;
        }

        let mut current_width = 0usize;
        let mut current_spans: Vec<Span<'static>> = Vec::new();
        for segment in segments {
            let mut remaining = segment.text.as_str();
            if remaining.is_empty() {
                continue;
            }

            while !remaining.is_empty() {
                if current_width == max_width {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                    current_width = 0;
                }

                let available = max_width.saturating_sub(current_width);
                let (take_bytes, take_width) = Self::take_prefix_by_width(remaining, available);
                let chunk = &remaining[..take_bytes];
                let chunk_style = segment.style;
                let link_target = segment.link_target.clone();

                if take_bytes == 0 {
                    if !current_spans.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut current_spans)));
                        current_width = 0;
                    } else {
                        lines.push(Line::from(""));
                        break;
                    }
                    continue;
                }

                let start_col = current_width;
                current_spans.push(Span::styled(chunk.to_string(), chunk_style));
                current_width = current_width.saturating_add(take_width);
                remaining = &remaining[take_bytes..];

                if let Some(target) = link_target.as_deref()
                    && take_width > 0
                {
                    links.push(AssistantFeedLink {
                        row: lines.len(),
                        start_col,
                        end_col: start_col.saturating_add(take_width),
                        target: target.to_string(),
                    });
                }

                if current_width >= max_width {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                    current_width = 0;
                }
            }
        }

        if current_spans.is_empty() && lines.is_empty() {
            lines.push(Line::from(""));
        } else if !current_spans.is_empty() {
            lines.push(Line::from(current_spans));
        }
    }

    fn indent_line(line: Line<'static>, indent: &str, style: Style) -> Line<'static> {
        let mut spans = Vec::with_capacity(line.spans.len() + 1);
        spans.push(Span::styled(indent.to_string(), style));
        spans.extend(line.spans);
        Line::from(spans)
    }

    fn line_to_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    fn width_of(text: &str) -> usize {
        UnicodeWidthStr::width(text)
    }

    fn take_prefix_by_width(text: &str, width: usize) -> (usize, usize) {
        if width == 0 {
            return (0, 0);
        }
        let mut consumed = 0usize;
        let mut total_width = 0usize;
        for (byte_index, ch) in text.char_indices() {
            let char_width = ch.width().unwrap_or(0);
            if total_width + char_width > width {
                if byte_index == 0 {
                    return (ch.len_utf8(), std::cmp::max(char_width, 1));
                }
                return (byte_index, total_width);
            }
            consumed = byte_index + ch.len_utf8();
            total_width += char_width;
            if total_width == width {
                break;
            }
        }
        if consumed == 0 {
            (text.len(), Self::width_of(text))
        } else {
            (consumed, total_width)
        }
    }

    fn wrap_lines(lines: Vec<Line<'static>>, max_width: usize) -> Vec<Line<'static>> {
        if max_width == 0 {
            return Vec::new();
        }
        let mut result = Vec::new();
        for line in lines {
            if line.spans.is_empty() {
                result.push(Line::from(""));
                continue;
            }
            let mut current_width = 0usize;
            let mut current_spans: Vec<Span<'static>> = Vec::new();
            for span in line.spans {
                let style = span.style;
                let mut remaining = span.content.into_owned();
                if remaining.is_empty() {
                    current_spans.push(Span::styled(String::new(), style));
                    continue;
                }
                while !remaining.is_empty() {
                    let mut take_bytes = 0usize;
                    let mut take_width = 0usize;
                    for (byte_index, ch) in remaining.char_indices() {
                        let char_width = ch.width().unwrap_or(0);
                        if current_width + take_width + char_width > max_width && take_bytes > 0 {
                            break;
                        }
                        if current_width + take_width + char_width > max_width && take_bytes == 0 {
                            take_bytes = byte_index + ch.len_utf8();
                            take_width = char_width;
                            break;
                        }
                        take_bytes = byte_index + ch.len_utf8();
                        take_width += char_width;
                    }
                    if take_bytes == 0 {
                        result.push(Line::from(std::mem::take(&mut current_spans)));
                        current_width = 0;
                        continue;
                    }
                    let chunk = remaining[..take_bytes].to_string();
                    current_spans.push(Span::styled(chunk, style));
                    current_width += take_width;
                    remaining = remaining[take_bytes..].to_string();
                    if current_width >= max_width && !remaining.is_empty() {
                        result.push(Line::from(std::mem::take(&mut current_spans)));
                        current_width = 0;
                    }
                }
            }
            result.push(Line::from(current_spans));
        }
        result
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellScenario {
    Startup,
    Preview,
    AssistantStreaming,
    CommandRunning,
    CommandError,
    ModelPicker,
    Help,
    RuntimeStarting,
    RuntimeFailed,
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn render_shell(area: Rect, state: &ShellState) -> Buffer {
        let backend = TestBackend::new(area.width, area.height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let theme = Theme::core_tui();
        terminal
            .draw(|frame| {
                ShellRenderer::render(frame.buffer_mut(), area, state, &theme);
            })
            .expect("draw shell");
        terminal.backend().buffer().clone()
    }

    #[test]
    fn startup_shell_uses_bootstrap_scene() {
        let state = ShellState::for_scenario(ShellScenario::Startup, Rect::new(0, 0, 120, 40));
        assert_eq!(state.scene, ShellScene::Bootstrap);
        assert!(state.bootstrap.is_some());
    }

    #[test]
    fn geometry_exposes_terminal_content_rect_when_terminal_is_open() {
        let mut state = ShellState::for_scenario(ShellScenario::Preview, Rect::new(0, 0, 160, 50));
        state.terminal.visible = true;
        let geometry = ShellGeometry::for_state(Rect::new(0, 0, 160, 50), &state);
        let rect = geometry
            .terminal_content_rect(&state)
            .expect("terminal content");
        assert!(rect.width > 10);
        assert!(rect.height > 4);
    }

    #[test]
    fn shell_first_ready_geometry_hides_legacy_sidebar_and_keeps_thirty_two_percent_rail() {
        let area = Rect::new(0, 0, 180, 50);
        let state = ShellState::for_scenario(ShellScenario::Preview, area);
        let geometry = ShellGeometry::for_state(area, &state);
        assert_eq!(geometry.sidebar.width, 0);
        let proof_rail = geometry.proof_rail.expect("proof rail");
        assert_eq!(proof_rail.width, 58);
    }

    #[test]
    fn rendered_shell_contains_terminal_bar() {
        let state = ShellState::for_scenario(ShellScenario::Preview, Rect::new(0, 0, 120, 40));
        let buffer = render_shell(Rect::new(0, 0, 120, 40), &state);
        let footer = (0..120u16)
            .map(|x| buffer[(x, 38)].symbol().to_string())
            .collect::<String>();
        assert!(footer.contains("Terminal"));
    }

    #[test]
    fn terminal_drawer_uses_black_background_and_renders_path() {
        let area = Rect::new(0, 0, 120, 40);
        let mut state = ShellState::for_scenario(ShellScenario::Preview, area);
        state.terminal.visible = true;
        state.terminal.badge_label = Some("zsh".to_string());
        state.terminal.detail_label = Some("/Users/jepsontaylor/code/quorp".to_string());

        let geometry = ShellGeometry::for_state(area, &state);
        let drawer = geometry.terminal_drawer.expect("terminal drawer");
        let terminal_bar = geometry.terminal_bar;
        let buffer = render_shell(area, &state);
        let theme = Theme::core_tui();

        assert_eq!(
            buffer[(drawer.x + 2, drawer.y + 2)].bg,
            theme.palette.terminal_bg
        );

        let title_row = (terminal_bar.x + 1..terminal_bar.right().saturating_sub(1))
            .map(|x| buffer[(x, terminal_bar.y)].symbol().to_string())
            .collect::<String>();
        assert!(title_row.contains("Terminal"));
        assert!(title_row.contains("zsh"));
    }

    #[test]
    fn ready_shell_draws_separator_lines_between_panes() {
        let area = Rect::new(0, 0, 120, 40);
        let state = ShellState::for_scenario(ShellScenario::Preview, area);
        let geometry = ShellGeometry::for_state(area, &state);
        let buffer = render_shell(area, &state);

        assert_eq!(
            buffer[(geometry.sidebar.x + 8, geometry.sidebar.y)].symbol(),
            "─"
        );
        assert_eq!(
            buffer[(geometry.center.x, geometry.center.y + 6)].symbol(),
            "│"
        );
        assert_eq!(
            buffer[(
                geometry.center.x + 8,
                geometry.center.bottom().saturating_sub(1)
            )]
                .symbol(),
            "─"
        );
    }
}

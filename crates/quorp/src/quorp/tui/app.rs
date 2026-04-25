#![allow(unused)]
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::quorp::tui::chat::ChatPane;

use crate::quorp::tui::action_discovery::{
    ActionDeckCommand, ActionDeckEntry, ActionDiscoveryModel, OverlayTextInput,
};
use crate::quorp::tui::editor_pane::EditorPane;
use crate::quorp::tui::engage_target::{
    EngageResolution, EngageTarget, EngageTargetKind, extract_openable_tokens, resolve_target,
};
use crate::quorp::tui::file_tree::{FileTree, FileTreeKeyOutcome};
use crate::quorp::tui::path_guard::path_within_project;
use crate::quorp::tui::ssd_moe_tui::SsdMoeManager;
use crate::quorp::tui::tui_backend::SharedTuiBackend;

use crate::quorp::tui::agent_pane::AgentPane;
use crate::quorp::tui::bootstrap_loader::{BOOTSTRAP_REVEAL_FRAMES, BootstrapLoader};
use crate::quorp::tui::models_pane::ModelsPane;
use crate::quorp::tui::proof_rail::{ProofRailState, RailMode};
use crate::quorp::tui::rail_event::RailEvent;
use crate::quorp::tui::shell::{
    AssistantFeedLink, AssistantTone, BootstrapProbe, BootstrapStatus, FeedItemTone,
    MainWorkspaceMode, SessionPillTone, ShellAssistantView, ShellBootstrapView, ShellCenterView,
    ShellDrawerView, ShellExperienceMode, ShellExplorerItem, ShellFeedItem, ShellFocus,
    ShellGeometry, ShellLayoutMode, ShellMainView, ShellOverlay, ShellProjectItem, ShellRenderer,
    ShellRuntimeStateKind, ShellScene, ShellSessionPill, ShellSidebarView, ShellState,
    ShellThreadItem, shell_composer_height_for_text,
};
use crate::quorp::tui::slash_commands::{self, CommandDeckEntry};
use crate::quorp::tui::terminal_pane::TerminalPane;
use crate::quorp::tui::theme::Theme;
use crate::quorp::tui::workspace_state::{ThreadStatus, WorkspaceStore, canonical_project_root};

use crate::quorp::tui::hitmap::{HitMap, HitTarget};
use crate::quorp::tui::text_width::truncate_fit;
use crate::quorp::tui::workbench::{LeafId, WorkspaceNode};
use serde_json::json;

pub type PaneType = LeafId;

#[allow(non_snake_case, non_upper_case_globals)]
pub mod Pane {
    use crate::quorp::tui::workbench::LeafId;
    pub const EditorPane: LeafId = LeafId(1);
    pub const Terminal: LeafId = LeafId(2);
    pub const Chat: LeafId = LeafId(3);
    pub const Agent: LeafId = LeafId(4);
    pub const FileTree: LeafId = LeafId(0);

    pub fn display_label(pane: LeafId) -> &'static str {
        match pane {
            EditorPane => "Preview",
            Terminal => "Terminal",
            Chat => "Assistant",
            Agent => "Assistant",
            FileTree => "Files",
            _ => "Unknown",
        }
    }

    pub fn next(pane: LeafId) -> LeafId {
        match pane {
            EditorPane => Terminal,
            Terminal => Chat,
            Chat => FileTree,
            Agent => FileTree,
            FileTree => EditorPane,
            _ => pane,
        }
    }

    pub fn prev(pane: LeafId) -> LeafId {
        match pane {
            EditorPane => FileTree,
            Terminal => EditorPane,
            Chat => Terminal,
            FileTree => Chat,
            Agent => Chat,
            _ => pane,
        }
    }
}

pub trait PaneExt {
    fn display_label(self) -> &'static str;
    fn next(self) -> Self;
    fn prev(self) -> Self;
}

impl PaneExt for LeafId {
    fn display_label(self) -> &'static str {
        Pane::display_label(self)
    }
    fn next(self) -> Self {
        Pane::next(self)
    }
    fn prev(self) -> Self {
        Pane::prev(self)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Overlay {
    #[default]
    None,
    Help,
    ModelPicker,
    Explorer,
    QuickOpen,
    NewThreadPrompt,
    SlashCommandDeck,
    ActionDeck,
}

impl Overlay {
    #[inline]
    const fn is_active(self) -> bool {
        !matches!(self, Self::None)
    }

    #[inline]
    const fn is_help(self) -> bool {
        matches!(self, Self::Help)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SplitterVisualState {
    #[default]
    Idle,
    Hover {
        index: usize,
    },
    Dragging {
        index: usize,
    },
}

#[derive(Clone, Debug)]
struct QuickOpenState {
    query: String,
    selected_index: usize,
    matches: Vec<(String, std::path::PathBuf)>,
}

#[derive(Clone, Debug, Default)]
struct NewThreadPrompt {
    query: String,
    selected_index: usize,
    matches: Vec<(String, std::path::PathBuf)>,
}

#[derive(Clone, Debug, Default)]
struct SlashCommandDeckState {
    query: String,
    selected_index: usize,
    matches: Vec<CommandDeckEntry>,
}

#[derive(Clone, Debug, Default)]
struct ActionDeckState {
    query: String,
    selected_index: usize,
    matches: Vec<ActionDeckEntry>,
}

#[derive(Clone, Debug)]
struct EngagePreviewOverride {
    title: String,
    lines: Vec<String>,
}

impl QuickOpenState {
    fn new() -> Self {
        Self {
            query: String::new(),
            selected_index: 0,
            matches: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
enum BootstrapRemoteRuntimeProbe {
    Pending(String),
    Ready(String),
    Failed(String),
}

impl BootstrapRemoteRuntimeProbe {
    fn status(&self) -> BootstrapStatus {
        match self {
            Self::Pending(_) => BootstrapStatus::Pending,
            Self::Ready(_) => BootstrapStatus::Ok,
            Self::Failed(_) => BootstrapStatus::Failed,
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Pending(detail) | Self::Ready(detail) | Self::Failed(detail) => detail,
        }
    }
}

#[derive(Clone, Debug)]
struct BootstrapProgress {
    started_at: Instant,
    visible_started_at: Option<Instant>,
    frame_index: usize,
    terminal_probe_ok: bool,
    workspace_probe_ok: bool,
    pty_probe: Result<String, String>,
    session_restore: Result<String, String>,
    remote_runtime_probe: Option<BootstrapRemoteRuntimeProbe>,
}

impl BootstrapProgress {
    fn new(workspace_root: &std::path::Path) -> Self {
        let workspace_probe = if workspace_root.exists() {
            Ok(workspace_root.display().to_string())
        } else {
            Err(format!("missing {}", workspace_root.display()))
        };
        Self {
            started_at: Instant::now(),
            visible_started_at: None,
            frame_index: 0,
            terminal_probe_ok: true,
            workspace_probe_ok: workspace_probe.is_ok(),
            pty_probe: Err("waiting for terminal grid".to_string()),
            session_restore: Err("starting fresh session".to_string()),
            remote_runtime_probe: None,
        }
    }
}

fn bootstrap_remote_probe_for_provider(
    provider: crate::quorp::executor::InteractiveProviderKind,
) -> Option<BootstrapRemoteRuntimeProbe> {
    match provider {
        crate::quorp::executor::InteractiveProviderKind::Local => None,
        crate::quorp::executor::InteractiveProviderKind::Ollama => Some(
            BootstrapRemoteRuntimeProbe::Pending("probing Ollama endpoint".to_string()),
        ),
        crate::quorp::executor::InteractiveProviderKind::OpenAiCompatible => {
            Some(BootstrapRemoteRuntimeProbe::Pending(
                "checking remote endpoint configuration".to_string(),
            ))
        }
        crate::quorp::executor::InteractiveProviderKind::Nvidia => {
            Some(BootstrapRemoteRuntimeProbe::Pending(
                "checking NVIDIA NIM endpoint configuration".to_string(),
            ))
        }
        crate::quorp::executor::InteractiveProviderKind::Codex => Some(
            BootstrapRemoteRuntimeProbe::Ready("executor session ready".to_string()),
        ),
    }
}

const BOOTSTRAP_MIN_DURATION: Duration = Duration::from_millis(1800);
const RUNTIME_HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(400);
const DRAW_BUDGET_MS: u128 = 16;

pub struct TuiApp {
    pub focused: PaneType,
    pub right_pane: PaneType,
    /// Last focused pane in the left column; used when returning from the file tree with Ctrl+h.
    last_left_pane: PaneType,
    pub file_tree: FileTree,
    pub editor_pane: EditorPane,
    pub terminal: TerminalPane,
    pub agent_pane: AgentPane,
    pub chat: ChatPane,
    pub models_pane: ModelsPane,
    pub ssd_moe: SsdMoeManager,
    _runtime: Option<tokio::runtime::Runtime>,
    _event_rx_keepalive: Option<std::sync::mpsc::Receiver<crate::quorp::tui::TuiEvent>>,
    pub overlay: Overlay,
    overlay_snapshot_cache: Option<ShellOverlay>,
    last_full_area: Rect,
    pub theme: Theme,
    pub hitmap: HitMap,
    pub workspace: WorkspaceNode,
    /// When set, replaces the file-tree root display in the status bar center (visual regression / tooling).
    pub visual_status_center_override: Option<String>,
    pub visual_title_override: Option<String>,
    pub visual_status_left_override: Option<String>,
    pub visual_status_right_override: Option<String>,
    /// When true, recompute [`WorkspaceNode`] from [`crate::quorp::tui::workbench::prismforge_tree_for_workspace`] each draw.
    pub prismforge_dynamic_layout: bool,
    /// User-dragged `(vertical, horizontal)` ratios for PrismForge dynamic layout; cleared on resize.
    prismforge_split_ratio_lock: Option<(u16, u16)>,
    pub splitter_visual_state: SplitterVisualState,
    pub tab_strip_focus: Option<PaneType>,
    pub compact_ui: bool,
    /// Incremented each full draw; drives indexing spinner in the status bar.
    draw_frame_seq: u64,
    pub explorer_collapsed: bool,
    pub terminal_dock_open: bool,
    assistant_feed_scroll_top: usize,
    assistant_feed_follow_latest: bool,
    assistant_feed_total_lines: usize,
    assistant_feed_viewport_lines: usize,
    assistant_feed_scrollbar_hovered: bool,
    assistant_feed_active_link: Option<usize>,
    active_engage_target_key: Option<String>,
    engage_preview_override: Option<EngagePreviewOverride>,
    bootstrap: BootstrapProgress,
    quick_open: QuickOpenState,
    new_thread_prompt: NewThreadPrompt,
    slash_command_deck: SlashCommandDeckState,
    action_deck: ActionDeckState,
    workspace_store: WorkspaceStore,
    sidebar_project_ids: Vec<String>,
    sidebar_thread_ids: Vec<String>,
    has_completed_bootstrap: Cell<bool>,
    last_shell_scene_logged: Cell<Option<ShellScene>>,
    last_shell_gate_summary: RefCell<String>,
    last_runtime_health_poll_at: RefCell<Option<Instant>>,
    pub agent_runtime_tx: Option<
        futures::channel::mpsc::UnboundedSender<
            crate::quorp::tui::agent_runtime::AgentRuntimeCommand,
        >,
    >,
    pub last_working_tick: Option<Instant>,
    #[cfg(test)]
    _test_model_config_guard: Option<crate::quorp::tui::model_registry::TestModelConfigGuard>,
    pub proof_rail: ProofRailState,
}

impl TuiApp {
    fn refresh_sidebar_cache(&mut self) {
        self.sidebar_project_ids = self
            .workspace_store
            .projects_sorted()
            .into_iter()
            .map(|project| project.id.clone())
            .collect();
        self.sidebar_thread_ids = self
            .workspace_store
            .active_project()
            .map(|project| {
                self.workspace_store
                    .threads_for_project(&project.id)
                    .into_iter()
                    .map(|thread| thread.id.clone())
                    .collect()
            })
            .unwrap_or_default();
    }

    pub fn persist_workspace_state(&mut self) {
        let snapshot = self.chat.export_active_thread_snapshot();
        let status = if self.chat.is_streaming() {
            ThreadStatus::Working
        } else if snapshot.last_error.is_some() {
            ThreadStatus::Failed
        } else {
            ThreadStatus::Idle
        };
        if let Err(error) = self
            .workspace_store
            .upsert_active_thread_snapshot(&snapshot, status)
        {
            log::error!("tui: failed to persist active thread snapshot: {error:#}");
        }
        self.refresh_sidebar_cache();
    }

    #[inline]
    fn close_overlay(&mut self) {
        self.set_overlay(Overlay::None);
    }

    #[inline]
    fn open_help_overlay(&mut self) {
        self.set_overlay(Overlay::Help);
    }

    #[inline]
    fn set_overlay(&mut self, overlay: Overlay) {
        if self.overlay == overlay {
            return;
        }
        self.overlay = overlay;
        self.overlay_snapshot_cache = None;
    }

    #[inline]
    fn apply_text_overlay_backspace(
        query: &mut String,
        key: &KeyEvent,
        clear_with_control: bool,
    ) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && clear_with_control {
            if query.is_empty() {
                return false;
            }
            query.clear();
            return true;
        }
        query.pop().is_some()
    }

    #[inline]
    fn move_overlay_selection_index(index: &mut usize, max: usize, delta: isize) -> bool {
        match delta {
            -1 => {
                let next = index.saturating_sub(1);
                if *index == next {
                    return false;
                }
                *index = next;
                true
            }
            1 => {
                if *index + 1 >= max {
                    return false;
                }
                *index += 1;
                true
            }
            _ => false,
        }
    }

    #[inline]
    fn invalidate_overlay_snapshot_cache(&mut self) {
        self.overlay_snapshot_cache = None;
    }

    fn handle_overlay_key_event(
        &mut self,
        key: &KeyEvent,
    ) -> Option<std::ops::ControlFlow<(), ()>> {
        if !self.overlay.is_active() {
            return None;
        }

        match self.overlay {
            Overlay::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                    self.close_overlay();
                    return Some(ControlFlow::Continue(()));
                }
                Some(ControlFlow::Continue(()))
            }
            Overlay::QuickOpen => {
                match ActionDiscoveryModel::parse_text_overlay_input(key) {
                    OverlayTextInput::Close => {
                        self.close_overlay();
                    }
                    OverlayTextInput::MoveUp => {
                        if Self::move_overlay_selection_index(
                            &mut self.quick_open.selected_index,
                            self.quick_open.matches.len(),
                            -1,
                        ) {
                            self.invalidate_overlay_snapshot_cache();
                        }
                    }
                    OverlayTextInput::MoveDown => {
                        if Self::move_overlay_selection_index(
                            &mut self.quick_open.selected_index,
                            self.quick_open.matches.len(),
                            1,
                        ) {
                            self.invalidate_overlay_snapshot_cache();
                        }
                    }
                    OverlayTextInput::Confirm => {
                        self.accept_quick_open_selection();
                    }
                    OverlayTextInput::Backspace => {
                        if Self::apply_text_overlay_backspace(
                            &mut self.quick_open.query,
                            key,
                            false,
                        ) {
                            self.refresh_quick_open_matches();
                            self.invalidate_overlay_snapshot_cache();
                        }
                    }
                    OverlayTextInput::InsertChar(character) => {
                        self.quick_open.query.push(character);
                        self.refresh_quick_open_matches();
                        self.invalidate_overlay_snapshot_cache();
                    }
                    OverlayTextInput::Ignore => {}
                }
                Some(ControlFlow::Continue(()))
            }
            Overlay::SlashCommandDeck => {
                match ActionDiscoveryModel::parse_text_overlay_input(key) {
                    OverlayTextInput::Close => {
                        self.close_overlay();
                    }
                    OverlayTextInput::MoveUp => {
                        if Self::move_overlay_selection_index(
                            &mut self.slash_command_deck.selected_index,
                            self.slash_command_deck.matches.len(),
                            -1,
                        ) {
                            self.invalidate_overlay_snapshot_cache();
                        }
                    }
                    OverlayTextInput::MoveDown => {
                        if Self::move_overlay_selection_index(
                            &mut self.slash_command_deck.selected_index,
                            self.slash_command_deck.matches.len(),
                            1,
                        ) {
                            self.invalidate_overlay_snapshot_cache();
                        }
                    }
                    OverlayTextInput::Confirm => {
                        self.accept_slash_command_selection();
                    }
                    OverlayTextInput::Backspace => {
                        if Self::apply_text_overlay_backspace(
                            &mut self.slash_command_deck.query,
                            key,
                            false,
                        ) {
                            self.refresh_slash_command_matches();
                        }
                    }
                    OverlayTextInput::InsertChar(character) => {
                        self.slash_command_deck.query.push(character);
                        self.refresh_slash_command_matches();
                    }
                    OverlayTextInput::Ignore => {}
                }
                Some(ControlFlow::Continue(()))
            }
            Overlay::ActionDeck => {
                match ActionDiscoveryModel::parse_text_overlay_input(key) {
                    OverlayTextInput::Close => {
                        self.close_overlay();
                    }
                    OverlayTextInput::MoveUp => {
                        if Self::move_overlay_selection_index(
                            &mut self.action_deck.selected_index,
                            self.action_deck.matches.len(),
                            -1,
                        ) {
                            self.invalidate_overlay_snapshot_cache();
                        }
                    }
                    OverlayTextInput::MoveDown => {
                        if Self::move_overlay_selection_index(
                            &mut self.action_deck.selected_index,
                            self.action_deck.matches.len(),
                            1,
                        ) {
                            self.invalidate_overlay_snapshot_cache();
                        }
                    }
                    OverlayTextInput::Confirm => {
                        self.accept_action_deck_selection();
                    }
                    OverlayTextInput::Backspace => {
                        if Self::apply_text_overlay_backspace(
                            &mut self.action_deck.query,
                            key,
                            false,
                        ) {
                            self.refresh_action_deck_matches();
                        }
                    }
                    OverlayTextInput::InsertChar(character) => {
                        self.action_deck.query.push(character);
                        self.refresh_action_deck_matches();
                    }
                    OverlayTextInput::Ignore => {}
                }
                Some(ControlFlow::Continue(()))
            }
            Overlay::NewThreadPrompt => {
                match ActionDiscoveryModel::parse_text_overlay_input(key) {
                    OverlayTextInput::Close => {
                        self.close_new_thread_prompt();
                    }
                    OverlayTextInput::MoveUp => {
                        if Self::move_overlay_selection_index(
                            &mut self.new_thread_prompt.selected_index,
                            self.new_thread_prompt.matches.len(),
                            -1,
                        ) {
                            self.invalidate_overlay_snapshot_cache();
                        }
                    }
                    OverlayTextInput::MoveDown => {
                        if Self::move_overlay_selection_index(
                            &mut self.new_thread_prompt.selected_index,
                            self.new_thread_prompt.matches.len(),
                            1,
                        ) {
                            self.invalidate_overlay_snapshot_cache();
                        }
                    }
                    OverlayTextInput::Confirm => {
                        self.confirm_new_thread_prompt();
                    }
                    OverlayTextInput::Backspace => {
                        if Self::apply_text_overlay_backspace(
                            &mut self.new_thread_prompt.query,
                            key,
                            true,
                        ) {
                            self.refresh_new_thread_prompt_matches();
                            self.invalidate_overlay_snapshot_cache();
                        }
                    }
                    OverlayTextInput::InsertChar(character) => {
                        self.new_thread_prompt.query.push(character);
                        self.refresh_new_thread_prompt_matches();
                        self.invalidate_overlay_snapshot_cache();
                    }
                    OverlayTextInput::Ignore => {}
                }
                Some(ControlFlow::Continue(()))
            }
            Overlay::Explorer => {
                if key.code == KeyCode::Esc
                    || (key.code == KeyCode::Char('b')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                {
                    self.close_overlay();
                    Some(ControlFlow::Continue(()))
                } else {
                    None
                }
            }
            Overlay::ModelPicker => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.models_pane.handle_up();
                    self.invalidate_overlay_snapshot_cache();
                    Some(ControlFlow::Continue(()))
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.models_pane.handle_down();
                    self.invalidate_overlay_snapshot_cache();
                    Some(ControlFlow::Continue(()))
                }
                KeyCode::Enter => {
                    let Some(entry) = self
                        .models_pane
                        .entries
                        .get(self.models_pane.selected_index)
                        .cloned()
                    else {
                        self.close_overlay();
                        return Some(ControlFlow::Continue(()));
                    };
                    if crate::quorp::tui::model_registry::local_moe_spec_for_registry_id(
                        &entry.registry_id,
                    )
                    .is_some()
                        && let Err(error) =
                            crate::quorp::tui::model_registry::save_model(&entry.registry_id)
                    {
                        log::error!(
                            "tui: failed to persist local model selection {:?}: {}",
                            entry.registry_id,
                            error
                        );
                    }
                    self.chat
                        .request_persist_default_model_to_agent_settings(&entry.registry_id);
                    self.chat.set_model_index(self.models_pane.selected_index);
                    let root = self.file_tree.root().to_path_buf();
                    if let Some(spec) =
                        crate::quorp::tui::model_registry::local_moe_spec_for_registry_id(
                            &entry.registry_id,
                        )
                    {
                        self.ssd_moe.switch_model(&root, &spec);
                    }
                    self.close_overlay();
                    Some(ControlFlow::Continue(()))
                }
                KeyCode::Esc => {
                    self.close_overlay();
                    Some(ControlFlow::Continue(()))
                }
                _ => None,
            },
            Overlay::None => None,
        }
    }

    fn restore_workspace_state(&mut self, requested_root: std::path::PathBuf) {
        self.workspace_store = WorkspaceStore::load_or_create(&requested_root);
        self.refresh_sidebar_cache();
        if let Some(project) = self.workspace_store.active_project() {
            self.file_tree.set_root(project.root.clone());
            self.editor_pane.close_all_file_tabs(project.root.as_path());
            self.chat.ensure_project_root(project.root.as_path());
            self.bootstrap.workspace_probe_ok = project.root.exists();
        }
        match self.workspace_store.load_active_thread_snapshot() {
            Ok(Some(snapshot)) => {
                self.chat.import_thread_snapshot(snapshot);
                self.bootstrap.session_restore = Ok("restored saved thread".to_string());
            }
            Ok(None) => {
                self.bootstrap.session_restore = Err("starting fresh thread".to_string());
            }
            Err(error) => {
                self.bootstrap.session_restore = Err(error.to_string());
            }
        }
        self.models_pane = ModelsPane::sync_from_chat(&self.chat);
        self.focused = Pane::Chat;
        self.last_left_pane = Pane::Chat;
        self.explorer_collapsed = true;
        self.terminal_dock_open = false;
    }

    pub fn new() -> Self {
        #[cfg(test)]
        let _ssd_moe_env_lock = crate::quorp::tui::ssd_moe_tui::test_env_lock();
        #[cfg(test)]
        let test_model_config_guard =
            Some(crate::quorp::tui::model_registry::isolated_test_model_config_guard());
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let handle = runtime.handle().clone();
        let (tx, rx) = std::sync::mpsc::sync_channel::<crate::quorp::tui::TuiEvent>(128);
        let file_tree = FileTree::new();
        let project_root = file_tree.root().to_path_buf();
        let path_index = std::sync::Arc::new(crate::quorp::tui::path_index::PathIndex::new(
            project_root.clone(),
        ));
        let mut ssd_moe = SsdMoeManager::new();
        if crate::quorp::executor::interactive_provider_from_env()
            == crate::quorp::executor::InteractiveProviderKind::Local
            && let Some(default_model) = crate::quorp::tui::model_registry::get_saved_model()
        {
            ssd_moe.ensure_running(&project_root, &default_model);
        }
        let theme = Theme::session_default();
        let chat = ChatPane::new(tx, project_root.clone(), path_index, None, None);
        let models_pane = ModelsPane::sync_from_chat(&chat);
        let mut bootstrap = BootstrapProgress::new(file_tree.root());
        bootstrap.remote_runtime_probe =
            bootstrap_remote_probe_for_provider(chat.current_provider_kind());
        if cfg!(test) {
            bootstrap.visible_started_at = Some(Instant::now() - BOOTSTRAP_MIN_DURATION);
            bootstrap.frame_index = BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1);
            ssd_moe.set_status_for_test(crate::quorp::tui::ssd_moe_tui::ModelStatus::Running);
        }
        let mut app = Self {
            focused: Pane::EditorPane,
            right_pane: Pane::Chat,
            last_left_pane: Pane::EditorPane,
            file_tree,
            editor_pane: EditorPane::new(),
            terminal: TerminalPane::new(),
            agent_pane: AgentPane::new(),
            chat,
            models_pane,
            ssd_moe,
            _runtime: Some(runtime),
            _event_rx_keepalive: Some(rx),
            overlay: Overlay::None,
            overlay_snapshot_cache: None,
            last_full_area: Rect::default(),
            theme,
            hitmap: HitMap::new(),
            workspace: crate::quorp::tui::workbench::default_core_tui_tree(),
            visual_status_center_override: None,
            visual_title_override: None,
            visual_status_left_override: None,
            visual_status_right_override: None,
            prismforge_dynamic_layout: false,
            prismforge_split_ratio_lock: None,
            splitter_visual_state: SplitterVisualState::Idle,
            tab_strip_focus: None,
            compact_ui: false,
            draw_frame_seq: 0,
            explorer_collapsed: false,
            terminal_dock_open: false,
            assistant_feed_scroll_top: 0,
            assistant_feed_follow_latest: true,
            assistant_feed_total_lines: 1,
            assistant_feed_viewport_lines: 1,
            assistant_feed_scrollbar_hovered: false,
            assistant_feed_active_link: None,
            active_engage_target_key: None,
            engage_preview_override: None,
            bootstrap,
            quick_open: QuickOpenState::new(),
            new_thread_prompt: NewThreadPrompt::default(),
            slash_command_deck: SlashCommandDeckState::default(),
            action_deck: ActionDeckState::default(),
            workspace_store: WorkspaceStore::load_or_create(&project_root),
            sidebar_project_ids: Vec::new(),
            sidebar_thread_ids: Vec::new(),
            has_completed_bootstrap: Cell::new(false),
            last_shell_scene_logged: Cell::new(None),
            last_shell_gate_summary: RefCell::new(String::new()),
            last_runtime_health_poll_at: RefCell::new(None),
            agent_runtime_tx: None,
            last_working_tick: None,
            #[cfg(test)]
            _test_model_config_guard: test_model_config_guard,
            proof_rail: ProofRailState::default(),
        };
        app.restore_workspace_state(project_root);
        app
    }

    pub(crate) fn new_with_chat_sender(
        tx: std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
        handle: tokio::runtime::Handle,
    ) -> Self {
        #[cfg(test)]
        let _ssd_moe_env_lock = crate::quorp::tui::ssd_moe_tui::test_env_lock();
        #[cfg(test)]
        let test_model_config_guard =
            Some(crate::quorp::tui::model_registry::isolated_test_model_config_guard());
        let file_tree = FileTree::new();
        let project_root = file_tree.root().to_path_buf();
        let path_index = std::sync::Arc::new(crate::quorp::tui::path_index::PathIndex::new(
            project_root.clone(),
        ));

        let mut ssd_moe = SsdMoeManager::new();
        if crate::quorp::executor::interactive_provider_from_env()
            == crate::quorp::executor::InteractiveProviderKind::Local
            && let Some(default_model) = crate::quorp::tui::model_registry::get_saved_model()
        {
            ssd_moe.ensure_running(&project_root, &default_model);
        }
        let theme = Theme::session_default();
        let chat = ChatPane::new(tx, project_root.clone(), path_index, None, None);
        let models_pane = ModelsPane::sync_from_chat(&chat);
        let mut bootstrap = BootstrapProgress::new(file_tree.root());
        bootstrap.remote_runtime_probe =
            bootstrap_remote_probe_for_provider(chat.current_provider_kind());
        if cfg!(test) {
            bootstrap.visible_started_at = Some(Instant::now() - BOOTSTRAP_MIN_DURATION);
            bootstrap.frame_index = BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1);
            ssd_moe.set_status_for_test(crate::quorp::tui::ssd_moe_tui::ModelStatus::Running);
        }
        let mut app = Self {
            focused: Pane::EditorPane,
            right_pane: Pane::Chat,
            last_left_pane: Pane::EditorPane,
            file_tree,
            editor_pane: EditorPane::new(),
            terminal: TerminalPane::new(),
            agent_pane: AgentPane::new(),
            chat,
            models_pane,
            ssd_moe,
            _runtime: None,
            _event_rx_keepalive: None,
            overlay: Overlay::None,
            overlay_snapshot_cache: None,
            last_full_area: Rect::default(),
            theme,
            hitmap: HitMap::new(),
            workspace: crate::quorp::tui::workbench::default_core_tui_tree(),
            visual_status_center_override: None,
            visual_title_override: None,
            visual_status_left_override: None,
            visual_status_right_override: None,
            prismforge_dynamic_layout: false,
            prismforge_split_ratio_lock: None,
            splitter_visual_state: SplitterVisualState::Idle,
            tab_strip_focus: None,
            compact_ui: false,
            draw_frame_seq: 0,
            explorer_collapsed: false,
            terminal_dock_open: false,
            assistant_feed_scroll_top: 0,
            assistant_feed_follow_latest: true,
            assistant_feed_total_lines: 1,
            assistant_feed_viewport_lines: 1,
            assistant_feed_scrollbar_hovered: false,
            assistant_feed_active_link: None,
            active_engage_target_key: None,
            engage_preview_override: None,
            bootstrap,
            quick_open: QuickOpenState::new(),
            new_thread_prompt: NewThreadPrompt::default(),
            slash_command_deck: SlashCommandDeckState::default(),
            action_deck: ActionDeckState::default(),
            workspace_store: WorkspaceStore::load_or_create(&project_root),
            sidebar_project_ids: Vec::new(),
            sidebar_thread_ids: Vec::new(),
            has_completed_bootstrap: Cell::new(false),
            last_shell_scene_logged: Cell::new(None),
            last_shell_gate_summary: RefCell::new(String::new()),
            last_runtime_health_poll_at: RefCell::new(None),
            agent_runtime_tx: None,
            last_working_tick: None,
            #[cfg(test)]
            _test_model_config_guard: test_model_config_guard,
            proof_rail: ProofRailState::default(),
        };
        app.restore_workspace_state(project_root);
        app
    }

    pub(crate) fn new_with_backend(
        workspace_root: std::path::PathBuf,
        tx: std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
        handle: tokio::runtime::Handle,

        unified_language_model: Option<(
            futures::channel::mpsc::UnboundedSender<crate::quorp::tui::bridge::TuiToBackendRequest>,
            Vec<String>,
            usize,
        )>,

        path_index_display_root: Option<std::sync::Arc<std::sync::RwLock<std::path::PathBuf>>>,
        command_bridge_tx: Option<
            futures::channel::mpsc::UnboundedSender<
                crate::quorp::tui::command_bridge::CommandBridgeRequest,
            >,
        >,
        unified_bridge_tx: Option<
            futures::channel::mpsc::UnboundedSender<crate::quorp::tui::bridge::TuiToBackendRequest>,
        >,
    ) -> Self {
        #[cfg(test)]
        let _ssd_moe_env_lock = crate::quorp::tui::ssd_moe_tui::test_env_lock();
        #[cfg(test)]
        let test_model_config_guard =
            Some(crate::quorp::tui::model_registry::isolated_test_model_config_guard());
        let mut file_tree = FileTree::with_root(workspace_root);
        let backend = unified_bridge_tx
            .clone()
            .map(crate::quorp::tui::bridge::UnifiedBridgeTuiBackend::new)
            .map(|backend| std::sync::Arc::new(backend) as SharedTuiBackend);
        if let Some(backend) = backend.clone() {
            file_tree.set_backend(backend);
        }
        let project_root = file_tree.root().to_path_buf();
        let path_index = match path_index_display_root {
            Some(watch) => std::sync::Arc::new(
                crate::quorp::tui::path_index::PathIndex::new_project_backed(
                    project_root.clone(),
                    std::sync::Arc::clone(&watch),
                ),
            ),
            None => std::sync::Arc::new(crate::quorp::tui::path_index::PathIndex::new(
                project_root.clone(),
            )),
        };

        let mut ssd_moe = SsdMoeManager::new();
        if crate::quorp::executor::interactive_provider_from_env()
            == crate::quorp::executor::InteractiveProviderKind::Local
            && let Some(default_model) = crate::quorp::tui::model_registry::get_saved_model()
        {
            ssd_moe.ensure_running(&project_root, &default_model);
        }
        let theme = Theme::session_default();
        let chat_uses_language_model_registry = unified_language_model.is_some();
        let mut chat = ChatPane::new(
            tx.clone(),
            project_root.clone(),
            path_index,
            unified_language_model,
            command_bridge_tx.clone(),
        );
        // `active_model.txt` stores local SSD-MOE weight ids for `SsdMoeManager`, not `provider/model`
        // lines from [`language_model::LanguageModelRegistry`]. Do not apply it to chat when integrated.
        if !chat_uses_language_model_registry
            && let Some(saved) = crate::quorp::tui::model_registry::get_saved_model_id_raw()
            && let Some(i) = chat
                .model_list()
                .iter()
                .position(|m| m.as_str() == saved.as_str())
        {
            chat.set_model_index(i);
        }
        let models_pane = ModelsPane::sync_from_chat(&chat);
        let mut bootstrap = BootstrapProgress::new(file_tree.root());
        bootstrap.remote_runtime_probe =
            bootstrap_remote_probe_for_provider(chat.current_provider_kind());
        if cfg!(test) {
            bootstrap.visible_started_at = Some(Instant::now() - BOOTSTRAP_MIN_DURATION);
            bootstrap.frame_index = BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1);
        }
        let agent_runtime = crate::quorp::tui::agent_runtime::spawn_agent_runtime(
            handle.clone(),
            project_root.clone(),
            tx.clone(),
            command_bridge_tx.clone(),
        );
        let mut app = Self {
            focused: Pane::EditorPane,
            right_pane: Pane::Chat,
            last_left_pane: Pane::EditorPane,
            file_tree,
            editor_pane: EditorPane::with_buffer_bridge(backend),
            terminal: unified_bridge_tx
                .clone()
                .map(|tx| TerminalPane::with_bridge(Some(tx)))
                .unwrap_or_else(TerminalPane::new),
            agent_pane: AgentPane::new(),
            chat,
            models_pane,
            ssd_moe,
            _runtime: None,
            _event_rx_keepalive: None,
            overlay: Overlay::None,
            overlay_snapshot_cache: None,
            last_full_area: Rect::default(),
            theme,
            hitmap: HitMap::new(),
            workspace: crate::quorp::tui::workbench::default_core_tui_tree(),
            prismforge_dynamic_layout: false,
            visual_status_center_override: None,
            visual_title_override: None,
            visual_status_left_override: None,
            visual_status_right_override: None,

            prismforge_split_ratio_lock: None,
            splitter_visual_state: SplitterVisualState::Idle,
            tab_strip_focus: None,
            compact_ui: false,
            draw_frame_seq: 0,
            explorer_collapsed: false,
            terminal_dock_open: false,
            assistant_feed_scroll_top: 0,
            assistant_feed_follow_latest: true,
            assistant_feed_total_lines: 1,
            assistant_feed_viewport_lines: 1,
            assistant_feed_scrollbar_hovered: false,
            assistant_feed_active_link: None,
            active_engage_target_key: None,
            engage_preview_override: None,
            bootstrap,
            quick_open: QuickOpenState::new(),
            new_thread_prompt: NewThreadPrompt::default(),
            slash_command_deck: SlashCommandDeckState::default(),
            action_deck: ActionDeckState::default(),
            workspace_store: WorkspaceStore::load_or_create(&project_root),
            sidebar_project_ids: Vec::new(),
            sidebar_thread_ids: Vec::new(),
            has_completed_bootstrap: Cell::new(false),
            last_shell_scene_logged: Cell::new(None),
            last_shell_gate_summary: RefCell::new(String::new()),
            last_runtime_health_poll_at: RefCell::new(None),
            agent_runtime_tx: Some(agent_runtime.tx),
            last_working_tick: None,
            #[cfg(test)]
            _test_model_config_guard: test_model_config_guard,
            proof_rail: ProofRailState::default(),
        };
        app.restore_workspace_state(project_root);
        app
    }

    fn status_center_for_draw(&self) -> String {
        if let Some(ref s) = self.visual_status_center_override {
            return s.clone();
        }
        self.file_tree.root().display().to_string()
    }

    fn indexing_status_suffix(&self) -> Option<String> {
        use crate::quorp::tui::path_index::PathIndexPhase;
        let p = self.chat.path_index_progress();
        if p.phase != PathIndexPhase::Scanning {
            return None;
        }
        const SPIN: &[char] = &['|', '/', '-', '\\'];
        let sp = SPIN[(self.draw_frame_seq as usize) % SPIN.len()];
        let n = p.files_seen;
        let n_str = if n >= 10_000 {
            format!("{}k", n / 1000)
        } else {
            n.to_string()
        };
        Some(format!("Idx{sp} {n_str}"))
    }

    fn status_center_for_status_bar(&self) -> String {
        let mut c = self.status_center_for_draw();
        if let Some(suf) = self.indexing_status_suffix() {
            c = format!("{c} {suf}");
        }
        c
    }

    fn update_compact_ui(&mut self, full: Rect) {
        self.compact_ui = matches!(ShellLayoutMode::for_area(full), ShellLayoutMode::Compact);
    }

    fn active_workspace_tree(&self, metrics: &crate::quorp::tui::theme::Metrics) -> WorkspaceNode {
        if self.compact_ui {
            crate::quorp::tui::workbench::compact_workspace_tree(metrics)
        } else {
            self.workspace.clone()
        }
    }

    fn assistant_should_overlay(&self) -> bool {
        self.compact_ui || self.overlay == Overlay::Explorer
    }

    fn assistant_overlay_visible(&self) -> bool {
        self.assistant_should_overlay()
            && (matches!(self.focused, Pane::Chat | Pane::Agent)
                || self.chat.is_streaming()
                || self.overlay == Overlay::ModelPicker)
    }

    fn set_focus(&mut self, pane: PaneType) {
        let previous_focus = self.focused;
        if self.tab_strip_focus.is_some_and(|leaf| leaf != pane) {
            self.tab_strip_focus = None;
        }
        self.focused = if self.compact_ui && pane == Pane::Agent {
            Pane::Chat
        } else {
            pane
        };
        if self.focused == Pane::Terminal {
            self.terminal.enter_capture_mode();
        }
        if previous_focus == Pane::Terminal && self.focused != Pane::Terminal {
            self.terminal.notify_focus_changed(false);
        } else if previous_focus != Pane::Terminal && self.focused == Pane::Terminal {
            self.terminal.notify_focus_changed(true);
        }
        if matches!(pane, Pane::EditorPane | Pane::Terminal | Pane::Chat) {
            self.last_left_pane = pane;
        }
    }

    /// Full status line for tests and layout; draw applies [`truncate_fit`] to the status row width.
    pub fn status_bar_text(&self) -> String {
        let mode = self.focused.display_label();
        let model = self.chat.current_model_display_label();
        let path = self.status_center_for_status_bar();
        let help_hint = ActionDiscoveryModel::help_hint(self.overlay.is_help());
        format!("Mode: {mode} | Model: {model} | Path: {path} | {help_hint}")
    }

    pub fn terminal_pane_content_size(&mut self, full: Rect) -> Option<(u16, u16)> {
        self.update_compact_ui(full);
        let state = self.shell_state_snapshot(full);
        let geometry = ShellGeometry::for_state(full, &state);
        geometry
            .terminal_content_rect(&state)
            .and_then(|rect| {
                (rect.width > 1 && rect.height > 1).then_some((rect.width, rect.height))
            })
            .or_else(|| {
                let layout_mode = ShellLayoutMode::for_area(full);
                let cols = full.width.saturating_sub(4).max(40);
                let rows = match layout_mode {
                    ShellLayoutMode::Compact => full.height.saturating_sub(6).max(10),
                    ShellLayoutMode::Standard => 10,
                    ShellLayoutMode::Full => 12,
                    ShellLayoutMode::Cinema => 14,
                };
                (cols > 1 && rows > 1).then_some((cols, rows))
            })
    }

    fn navigate_left(&mut self) {
        if matches!(self.focused, Pane::EditorPane | Pane::Terminal | Pane::Chat) {
            self.last_left_pane = self.focused;
            self.set_focus(Pane::FileTree);
        }
    }

    fn navigate_right(&mut self) {
        if self.focused == Pane::FileTree {
            self.set_focus(self.last_left_pane);
        } else if self.focused == Pane::EditorPane
            || self.focused == Pane::Terminal
            || self.focused == Pane::Agent
        {
            self.set_focus(self.right_pane);
        }
    }

    fn navigate_down(&mut self) {
        if self.focused == Pane::EditorPane {
            self.set_focus(Pane::Terminal);
        } else if self.focused == Pane::Terminal {
            self.set_focus(Pane::Chat);
        }
    }

    fn navigate_up(&mut self) {
        if self.focused == Pane::Terminal {
            self.set_focus(Pane::EditorPane);
        } else if self.focused == Pane::Chat {
            self.set_focus(Pane::Terminal);
        } else if self.focused == Pane::Agent {
            self.set_focus(Pane::Chat);
        }
    }

    fn try_handle_vim_pane_navigation(&mut self, key: &KeyEvent) -> bool {
        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }
        match key.code {
            KeyCode::Char('h') | KeyCode::Char('H') => self.navigate_left(),
            KeyCode::Char('l') | KeyCode::Char('L') => self.navigate_right(),
            KeyCode::Char('j') | KeyCode::Char('J') => self.navigate_down(),
            KeyCode::Char('k') | KeyCode::Char('K') => self.navigate_up(),
            KeyCode::Left => self.navigate_left(),
            KeyCode::Right => self.navigate_right(),
            KeyCode::Down => self.navigate_down(),
            KeyCode::Up => self.navigate_up(),
            _ => return false,
        }
        true
    }

    #[inline]
    fn is_focused(&self, pane: PaneType) -> bool {
        self.focused == pane
    }

    /// Snapshot layout after applying the same workspace sync as [`TuiApp::draw`] (for tests and hit geometry).
    pub fn workbench_layout_snapshot(
        &mut self,
        full: Rect,
    ) -> crate::quorp::tui::workbench::WorkbenchLayout {
        let metrics = self.theme.metrics;
        self.update_compact_ui(full);
        let shell = crate::quorp::tui::workbench::compute_shell(full, &metrics);
        self.sync_prismforge_workspace(shell.workspace, &metrics);
        let workspace_tree = self.active_workspace_tree(&metrics);
        crate::quorp::tui::workbench::compute_workbench(shell.workspace, &workspace_tree, &metrics)
    }

    fn sync_prismforge_workspace(
        &mut self,
        workspace_rect: ratatui::layout::Rect,
        metrics: &crate::quorp::tui::theme::Metrics,
    ) {
        if !self.prismforge_dynamic_layout {
            return;
        }
        let fresh =
            crate::quorp::tui::workbench::prismforge_tree_for_workspace(workspace_rect, metrics);
        let (fv, fh) = crate::quorp::tui::workbench::prismforge_ratios_from_tree(&fresh);
        let (v, h) = self.prismforge_split_ratio_lock.unwrap_or((fv, fh));
        self.workspace = crate::quorp::tui::workbench::prismforge_tree_with_ratios(v, h, 1);
    }

    fn context_hints_for_focused_pane(&self) -> &'static str {
        if self.focused == Pane::Terminal {
            self.terminal.interaction_hint()
        } else {
            ActionDiscoveryModel::context_hint(self.focused, self.compact_ui)
        }
    }

    fn splitter_hit_color(&self, splitter_index: usize) -> Color {
        let p = &self.theme.palette;
        match self.splitter_visual_state {
            SplitterVisualState::Dragging { index } if index == splitter_index => p.drag_accent,
            SplitterVisualState::Hover { index } if index == splitter_index => p.chat_accent,
            _ => p.divider_bg,
        }
    }

    fn register_splitter_hit_targets(
        &mut self,
        layout: &crate::quorp::tui::workbench::WorkbenchLayout,
    ) {
        for (index, div) in layout.splitters.iter().enumerate() {
            let hit = crate::quorp::tui::workbench::expand_splitter_hit_rect(*div);
            self.hitmap.push(hit, HitTarget::Splitter(index));
        }
    }

    fn hit_splitter_index_at(&mut self, col: u16, row: u16) -> Option<usize> {
        let full = self.last_full_area;
        if full.width == 0 || full.height == 0 {
            return None;
        }
        let metrics = self.theme.metrics;
        self.update_compact_ui(full);
        let shell = crate::quorp::tui::workbench::compute_shell(full, &metrics);
        self.sync_prismforge_workspace(shell.workspace, &metrics);
        let workspace_tree = self.active_workspace_tree(&metrics);
        let layout = crate::quorp::tui::workbench::compute_workbench(
            shell.workspace,
            &workspace_tree,
            &metrics,
        );
        for (index, div) in layout.splitters.iter().enumerate() {
            let hit = crate::quorp::tui::workbench::expand_splitter_hit_rect(*div);
            if col >= hit.x
                && col < hit.x.saturating_add(hit.width)
                && row >= hit.y
                && row < hit.y.saturating_add(hit.height)
            {
                return Some(index);
            }
        }
        None
    }

    fn apply_drag_to_splitter(&mut self, splitter_index: usize, col: u16, row: u16) {
        let full = self.last_full_area;
        if full.width == 0 || full.height == 0 {
            return;
        }
        let metrics = self.theme.metrics;
        self.update_compact_ui(full);
        let shell = crate::quorp::tui::workbench::compute_shell(full, &metrics);
        self.sync_prismforge_workspace(shell.workspace, &metrics);
        let workspace_tree = self.active_workspace_tree(&metrics);
        let Some((parent, axis, divider)) =
            crate::quorp::tui::workbench::split_parent_rect_for_index(
                shell.workspace,
                &workspace_tree,
                splitter_index,
            )
        else {
            return;
        };
        let primary = match axis {
            crate::quorp::tui::workbench::Axis::Vertical => col,
            crate::quorp::tui::workbench::Axis::Horizontal => row,
        };
        let new_bp = crate::quorp::tui::workbench::ratio_bp_from_drag_position(
            parent, axis, primary, divider,
        );
        if self.prismforge_dynamic_layout {
            let (mut v, mut h) =
                crate::quorp::tui::workbench::prismforge_ratios_from_tree(&self.workspace);
            match axis {
                crate::quorp::tui::workbench::Axis::Vertical => v = new_bp,
                crate::quorp::tui::workbench::Axis::Horizontal => h = new_bp,
            }
            self.prismforge_split_ratio_lock = Some((v, h));
            self.sync_prismforge_workspace(shell.workspace, &metrics);
        } else {
            let _ = crate::quorp::tui::workbench::set_splitter_ratio_bp(
                &mut self.workspace,
                splitter_index,
                new_bp,
            );
        }
    }

    pub fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        if self.overlay.is_help() {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.close_overlay();
            }
            return;
        }

        match mouse.kind {
            MouseEventKind::Moved => match self.splitter_visual_state {
                SplitterVisualState::Dragging { index } => {
                    self.apply_drag_to_splitter(index, mouse.column, mouse.row);
                }
                _ => {
                    let next = match self.hit_splitter_index_at(mouse.column, mouse.row) {
                        Some(index) => SplitterVisualState::Hover { index },
                        None => SplitterVisualState::Idle,
                    };
                    if next != self.splitter_visual_state {
                        self.splitter_visual_state = next;
                    }
                    self.assistant_feed_scrollbar_hovered = matches!(
                        self.hitmap.hit(mouse.column, mouse.row),
                        Some(HitTarget::AssistantFeedScrollbar)
                    );
                }
            },
            MouseEventKind::Drag(MouseButton::Left) => {
                if let SplitterVisualState::Dragging { index } = self.splitter_visual_state {
                    self.apply_drag_to_splitter(index, mouse.column, mouse.row);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_mouse_click(mouse.column, mouse.row);
            }
            MouseEventKind::ScrollUp => {
                self.handle_mouse_scroll(mouse.column, mouse.row, true);
            }
            MouseEventKind::ScrollDown => {
                self.handle_mouse_scroll(mouse.column, mouse.row, false);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let SplitterVisualState::Dragging { .. } = self.splitter_visual_state {
                    let next = match self.hit_splitter_index_at(mouse.column, mouse.row) {
                        Some(index) => SplitterVisualState::Hover { index },
                        None => SplitterVisualState::Idle,
                    };
                    self.splitter_visual_state = next;
                }
            }
            _ => {}
        }
    }

    fn handle_mouse_scroll(&mut self, col: u16, row: u16, upwards: bool) {
        if self.overlay != Overlay::None {
            return;
        }
        match self.hitmap.hit(col, row).copied() {
            Some(HitTarget::AssistantFeed) | Some(HitTarget::AssistantFeedScrollbar) => {
                self.set_focus(Pane::Chat);
                if upwards {
                    self.scroll_assistant_feed_up_lines(self.assistant_feed_line_step());
                } else {
                    self.scroll_assistant_feed_down_lines(self.assistant_feed_line_step());
                }
            }
            Some(HitTarget::ExplorerRow(_)) | Some(HitTarget::ExplorerMenu) => {
                self.set_focus(Pane::FileTree);
                let key = if upwards { KeyCode::Up } else { KeyCode::Down };
                let _ = self
                    .file_tree
                    .handle_key_event(&KeyEvent::new(key, KeyModifiers::NONE));
            }
            Some(HitTarget::LeafBody(leaf)) if leaf == Pane::Terminal => {}
            _ => {}
        }
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        self.draw_shell_preview(frame);
    }

    fn render_activity_bar(&mut self, frame: &mut Frame<'_>, area: Rect) {
        self.hitmap.push(area, HitTarget::Activity(0));
        let bg = Style::default().bg(self.theme.palette.activity_bg);
        frame.render_widget(Block::default().style(bg), area);

        let local_labels = ["F", "P", "A"];
        let pane_map = [Pane::FileTree, Pane::EditorPane, Pane::Chat];
        for (i, label) in local_labels.iter().enumerate() {
            if label.is_empty() {
                continue;
            }
            let y = area.y + i as u16 * 2 + 1; // spread them out a bit
            if y >= area.y + area.height {
                break;
            }
            let is_active = pane_map.get(i).is_some_and(|p| *p == self.focused);
            let style = if is_active {
                Style::default()
                    .bg(self.theme.palette.pill_bg)
                    .fg(self.theme.palette.icon_active)
            } else {
                Style::default()
                    .bg(self.theme.palette.activity_bg)
                    .fg(self.theme.palette.icon_inactive)
            };

            // Center the icon in the 6-col width
            let padding = "  ";
            let line = Line::from(vec![
                Span::styled(padding, style),
                Span::styled(*label, style.add_modifier(Modifier::BOLD)),
                Span::styled("   ", style),
            ]);
            frame.render_widget(Paragraph::new(line), Rect::new(area.x, y, area.width, 1));
        }
    }

    fn render_explorer(
        &mut self,
        frame: &mut Frame<'_>,
        header_area: Rect,
        body_area: Rect,
        explorer_focused: bool,
    ) {
        self.hitmap.push(header_area, HitTarget::ExplorerMenu);
        self.hitmap.push(body_area, HitTarget::ExplorerRow(0));

        let bg = Style::default().bg(self.theme.palette.sidebar_bg);
        frame.render_widget(Block::default().style(bg), header_area);
        frame.render_widget(Block::default().style(bg), body_area);

        frame.render_widget(
            crate::quorp::tui::chrome::ExplorerHeader { theme: &self.theme },
            header_area,
        );

        let accent_line_y = header_area
            .y
            .saturating_add(header_area.height.saturating_sub(1));
        let explorer_accent = if explorer_focused {
            self.theme.palette.explorer_accent
        } else {
            self.theme.palette.subtle_border
        };
        for x in header_area.left()..header_area.right() {
            if let Some(cell) = frame.buffer_mut().cell_mut((x, accent_line_y)) {
                cell.set_symbol(" ").set_bg(explorer_accent);
            }
        }

        self.file_tree
            .render(frame, body_area, explorer_focused, &self.theme);
    }

    fn shell_focus(&self, overlay_active: bool) -> ShellFocus {
        if overlay_active {
            return ShellFocus::Overlay;
        }
        match self.focused {
            Pane::FileTree => ShellFocus::Files,
            Pane::Terminal => ShellFocus::Terminal,
            Pane::EditorPane => ShellFocus::Main,
            Pane::Chat | Pane::Agent => ShellFocus::Feed,
            _ => ShellFocus::Feed,
        }
    }

    fn assistant_feed_max_scroll(&self) -> usize {
        self.assistant_feed_total_lines
            .saturating_sub(self.assistant_feed_viewport_lines.max(1))
    }

    fn clamp_assistant_feed_scroll(&mut self) {
        let max_scroll = self.assistant_feed_max_scroll();
        if self.assistant_feed_follow_latest || self.assistant_feed_scroll_top > max_scroll {
            self.assistant_feed_scroll_top = max_scroll;
        }
    }

    fn scroll_assistant_feed_to_bottom(&mut self) {
        self.assistant_feed_follow_latest = true;
        self.assistant_feed_scroll_top = self.assistant_feed_max_scroll();
    }

    fn assistant_feed_page_step(&self) -> usize {
        self.assistant_feed_viewport_lines.saturating_sub(1).max(1)
    }

    fn assistant_feed_line_step(&self) -> usize {
        3.min(self.assistant_feed_viewport_lines.max(1))
    }

    fn page_assistant_feed_up(&mut self) {
        self.assistant_feed_follow_latest = false;
        self.assistant_feed_scroll_top = self
            .assistant_feed_scroll_top
            .saturating_sub(self.assistant_feed_page_step());
    }

    fn page_assistant_feed_down(&mut self) {
        let max_scroll = self.assistant_feed_max_scroll();
        self.assistant_feed_scroll_top =
            (self.assistant_feed_scroll_top + self.assistant_feed_page_step()).min(max_scroll);
        if self.assistant_feed_scroll_top >= max_scroll {
            self.assistant_feed_follow_latest = true;
        }
    }

    fn scroll_assistant_feed_up_lines(&mut self, delta: usize) {
        self.assistant_feed_follow_latest = false;
        self.assistant_feed_scroll_top = self.assistant_feed_scroll_top.saturating_sub(delta);
    }

    fn scroll_assistant_feed_down_lines(&mut self, delta: usize) {
        let max_scroll = self.assistant_feed_max_scroll();
        self.assistant_feed_scroll_top = (self.assistant_feed_scroll_top + delta).min(max_scroll);
        if self.assistant_feed_scroll_top >= max_scroll {
            self.assistant_feed_follow_latest = true;
        }
    }

    fn on_assistant_feed_content_changed(&mut self, force_follow: bool) {
        if force_follow {
            self.assistant_feed_follow_latest = true;
        }
        self.assistant_feed_active_link = None;
        self.clamp_assistant_feed_scroll();
    }

    pub(crate) fn handle_chat_ui_event(&mut self, event: crate::quorp::tui::chat::ChatUiEvent) {
        self.last_working_tick = Some(std::time::Instant::now());
        if let crate::quorp::tui::chat::ChatUiEvent::CommandFinished(session_id, outcome) = &event
            && *session_id == crate::quorp::tui::agent_runtime::AGENT_RUNTIME_SESSION_ID
        {
            if let Some(agent_tx) = &self.agent_runtime_tx {
                let _ = agent_tx.unbounded_send(
                    crate::quorp::tui::agent_runtime::AgentRuntimeCommand::ToolFinished(
                        outcome.clone(),
                    ),
                );
            }
            return;
        }

        self.chat.apply_chat_event(event, &self.theme);
        self.on_assistant_feed_content_changed(false);
    }

    pub(crate) fn handle_backend_response(
        &mut self,
        response: crate::quorp::tui::bridge::BackendToTuiResponse,
    ) {
        match response {
            crate::quorp::tui::bridge::BackendToTuiResponse::AgentStatusUpdate(update) => {
                self.agent_pane.apply_status_update(update);
                self.on_assistant_feed_content_changed(false);
            }
        }
    }

    fn shell_runtime_label(&self) -> String {
        match self.chat.current_provider_kind() {
            crate::quorp::executor::InteractiveProviderKind::Local => {}
            crate::quorp::executor::InteractiveProviderKind::Ollama
            | crate::quorp::executor::InteractiveProviderKind::OpenAiCompatible
            | crate::quorp::executor::InteractiveProviderKind::Nvidia => {
                return format!("{} remote", self.chat.current_model_display_label());
            }
            crate::quorp::executor::InteractiveProviderKind::Codex => {
                return format!("{} executor", self.chat.current_model_display_label());
            }
        }
        let status = self.ssd_moe.status();
        let model = self
            .ssd_moe
            .active_model()
            .map(|model| crate::quorp::tui::model_registry::chat_model_raw_id(model.id).to_string())
            .unwrap_or_else(|| self.chat.current_model_display_label());
        format!("Local {model} {}", status.label())
    }

    fn shell_runtime_parts(&self) -> (String, String, ShellRuntimeStateKind) {
        match self.chat.current_provider_kind() {
            crate::quorp::executor::InteractiveProviderKind::Local => {}
            crate::quorp::executor::InteractiveProviderKind::Ollama
            | crate::quorp::executor::InteractiveProviderKind::OpenAiCompatible
            | crate::quorp::executor::InteractiveProviderKind::Nvidia => {
                return (
                    self.chat.current_model_display_label(),
                    "Remote".to_string(),
                    ShellRuntimeStateKind::Ready,
                );
            }
            crate::quorp::executor::InteractiveProviderKind::Codex => {
                return (
                    self.chat.current_model_display_label(),
                    "Executor".to_string(),
                    ShellRuntimeStateKind::Ready,
                );
            }
        }
        let status = self.ssd_moe.status();
        let model = self
            .ssd_moe
            .active_model()
            .map(|model| crate::quorp::tui::model_registry::chat_model_raw_id(model.id).to_string())
            .unwrap_or_else(|| self.chat.current_model_display_label());
        let label = status.label();
        let kind = match status {
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Running => ShellRuntimeStateKind::Online,
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Starting
            | crate::quorp::tui::ssd_moe_tui::ModelStatus::WaitingForBroker
            | crate::quorp::tui::ssd_moe_tui::ModelStatus::Stopping
            | crate::quorp::tui::ssd_moe_tui::ModelStatus::Downloading { .. }
            | crate::quorp::tui::ssd_moe_tui::ModelStatus::Packing { .. } => {
                ShellRuntimeStateKind::Transition
            }
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Ready => ShellRuntimeStateKind::Ready,
            crate::quorp::tui::ssd_moe_tui::ModelStatus::NotDownloaded
            | crate::quorp::tui::ssd_moe_tui::ModelStatus::Failed(_) => {
                ShellRuntimeStateKind::Offline
            }
        };
        (model, label, kind)
    }

    fn bootstrap_elapsed(&self) -> Duration {
        self.bootstrap
            .visible_started_at
            .map(|started_at| {
                Instant::now()
                    .checked_duration_since(started_at)
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }

    fn bootstrap_min_duration_met(&self) -> bool {
        self.bootstrap_elapsed() >= BOOTSTRAP_MIN_DURATION
    }

    fn shell_scene(&self) -> ShellScene {
        if self.has_completed_bootstrap.get() {
            return ShellScene::Ready;
        }
        let hard_gates_ready = self.bootstrap_hard_gates_ready();
        if hard_gates_ready && self.bootstrap_min_duration_met() {
            self.has_completed_bootstrap.set(true);
            return ShellScene::Ready;
        }
        ShellScene::Bootstrap
    }

    fn shell_experience_mode(&self) -> ShellExperienceMode {
        if std::env::var_os("QUORP_TUI_LEGACY_WORKBENCH").is_some() {
            return ShellExperienceMode::LegacyWorkbench;
        }
        if self.shell_scene() == ShellScene::Bootstrap {
            return ShellExperienceMode::Bootstrap;
        }
        match self.proof_rail.effective_mode() {
            RailMode::DiffReactor => ShellExperienceMode::DiffLens,
            RailMode::VerifyRadar => ShellExperienceMode::VerifyRadar,
            RailMode::TraceLens => ShellExperienceMode::TraceLens,
            RailMode::TimelineScrubber => ShellExperienceMode::Timeline,
            _ => ShellExperienceMode::CommandCenter,
        }
    }

    fn shell_control_hint(&self) -> String {
        let slash_hint = "/ workflow deck";
        let action_hint = "Ctrl+k control deck";
        let engage_hint = "Alt+↓ target  Alt+Enter open  D diff lens";
        let mode_hint = match self.shell_experience_mode() {
            ShellExperienceMode::Bootstrap => "wait for runtime readiness",
            ShellExperienceMode::CommandCenter => "d diff  v verify  r trace  t timeline  m memory",
            ShellExperienceMode::DiffLens => "Esc clear lens  v verify  r trace",
            ShellExperienceMode::VerifyRadar => "d diff  r trace  t timeline",
            ShellExperienceMode::TraceLens => "d diff  v verify  t timeline",
            ShellExperienceMode::Timeline => ". jump live  [ / ] scrub replay",
            ShellExperienceMode::LegacyWorkbench => {
                ActionDiscoveryModel::context_hint(self.focused, self.compact_ui)
            }
        };
        format!("{slash_hint}  ·  {action_hint}  ·  {engage_hint}  ·  {mode_hint}")
    }

    fn shell_target_label(&self, target: &EngageTarget) -> String {
        let base = target
            .path
            .strip_prefix(self.file_tree.root())
            .ok()
            .unwrap_or(target.path.as_path())
            .display()
            .to_string();
        match (target.line, target.column) {
            (Some(line), Some(column)) => format!("{base}:{line}:{column}"),
            (Some(line), None) => format!("{base}:{line}"),
            _ => base,
        }
    }

    fn shell_engage_suggestion_lines(&self) -> Vec<String> {
        let mut lines = vec![
            "Open the next useful target in place.".to_string(),
            "Use Alt+Down / Alt+Up to cycle and Alt+Enter to engage.".to_string(),
        ];
        let mut seen = HashSet::new();

        for file in self.proof_rail.snapshot.files_touched.iter().take(3) {
            if let Some(target) = self.resolve_engage_target(
                file,
                EngageTargetKind::ChangedFile,
                true,
                "blast radius",
            ) {
                let label = self.shell_target_label(&target);
                if seen.insert(label.clone()) {
                    lines.push(format!("Changed  {label}"));
                }
            }
        }

        for artifact in self.proof_rail.snapshot.artifacts.iter().take(2) {
            if let Some(target) = self.resolve_engage_target(
                &artifact.path,
                EngageTargetKind::Artifact,
                false,
                "artifact",
            ) {
                let label = self.shell_target_label(&target);
                if seen.insert(label.clone()) {
                    lines.push(format!("Artifact  {label}"));
                }
            }
        }

        for line in self.terminal.shell_lines(16) {
            for token in extract_openable_tokens(&line) {
                if let Some(target) = self.resolve_engage_target(
                    &token,
                    EngageTargetKind::TerminalPath,
                    self.path_has_diff_target(&token),
                    "terminal output",
                ) {
                    let label = self.shell_target_label(&target);
                    if seen.insert(label.clone()) {
                        lines.push(format!("Terminal  {label}"));
                    }
                    if lines.len() >= 7 {
                        return lines;
                    }
                }
            }
        }

        if lines.len() == 2 {
            lines.push("Use Ctrl+P for quick open, or / for workflow actions.".to_string());
        }
        lines
    }

    fn shell_main_preview(&self) -> (String, Vec<String>) {
        if let Some(preview_override) = &self.engage_preview_override {
            return (
                preview_override.title.clone(),
                preview_override.lines.clone(),
            );
        }

        let title = self.editor_pane.shell_title();
        let lines = self.editor_pane.shell_preview_lines(24);
        let is_placeholder = self.editor_pane.active_preview_path().is_none()
            && lines.len() == 1
            && lines[0] == "Select a file in the tree to preview it.";
        if is_placeholder {
            return (
                "Engage Here".to_string(),
                self.shell_engage_suggestion_lines(),
            );
        }
        (title, lines)
    }

    fn shell_main_session_pills(&self) -> Vec<ShellSessionPill> {
        let active_path = self
            .editor_pane
            .active_preview_path()
            .map(|path| path.to_path_buf());
        let mut pills = Vec::new();
        let mut seen = HashSet::new();

        for (label, active) in self.editor_pane.shell_tab_pills(3) {
            if seen.insert(label.clone()) {
                pills.push(ShellSessionPill {
                    label,
                    tone: if active {
                        SessionPillTone::Active
                    } else {
                        SessionPillTone::Muted
                    },
                });
            }
        }

        for file in self.proof_rail.snapshot.files_touched.iter().take(4) {
            let Some(target) = self.resolve_engage_target(
                file,
                EngageTargetKind::ChangedFile,
                true,
                "blast radius",
            ) else {
                continue;
            };
            let label = self.shell_target_label(&target);
            if !seen.insert(label.clone()) {
                continue;
            }
            let is_active = active_path
                .as_ref()
                .is_some_and(|path| path == &target.path);
            pills.push(ShellSessionPill {
                label,
                tone: if is_active {
                    SessionPillTone::Active
                } else {
                    SessionPillTone::Busy
                },
            });
            if pills.len() >= 5 {
                break;
            }
        }

        if pills.len() < 5 {
            for artifact in self.proof_rail.snapshot.artifacts.iter().take(2) {
                let Some(target) = self.resolve_engage_target(
                    &artifact.path,
                    EngageTargetKind::Artifact,
                    false,
                    "artifact",
                ) else {
                    continue;
                };
                let label = format!("artifact {}", self.shell_target_label(&target));
                if !seen.insert(label.clone()) {
                    continue;
                }
                pills.push(ShellSessionPill {
                    label,
                    tone: SessionPillTone::Normal,
                });
                if pills.len() >= 5 {
                    break;
                }
            }
        }

        pills.push(ShellSessionPill {
            label: "Terminal".to_string(),
            tone: if self.focused == Pane::Terminal || self.terminal_dock_open {
                SessionPillTone::Busy
            } else {
                SessionPillTone::Muted
            },
        });
        pills
    }

    fn mark_bootstrap_visible(&mut self) {
        if self.bootstrap.visible_started_at.is_none() {
            self.bootstrap.visible_started_at = Some(Instant::now());
        }
    }

    fn log_shell_scene_state(&self) {
        let runtime_status = match self.bootstrap_provider() {
            crate::quorp::executor::InteractiveProviderKind::Local => {
                self.ssd_moe.status().label().to_string()
            }
            crate::quorp::executor::InteractiveProviderKind::Ollama
            | crate::quorp::executor::InteractiveProviderKind::OpenAiCompatible
            | crate::quorp::executor::InteractiveProviderKind::Nvidia
            | crate::quorp::executor::InteractiveProviderKind::Codex => self
                .bootstrap
                .remote_runtime_probe
                .as_ref()
                .map(|probe| format!("{:?}", probe.status()))
                .unwrap_or_else(|| "pending".to_string()),
        };
        let runtime_reason = match self.bootstrap_provider() {
            crate::quorp::executor::InteractiveProviderKind::Local => self
                .ssd_moe
                .last_transition_reason()
                .unwrap_or_else(|| "<none>".to_string()),
            crate::quorp::executor::InteractiveProviderKind::Ollama
            | crate::quorp::executor::InteractiveProviderKind::OpenAiCompatible
            | crate::quorp::executor::InteractiveProviderKind::Nvidia
            | crate::quorp::executor::InteractiveProviderKind::Codex => self
                .bootstrap
                .remote_runtime_probe
                .as_ref()
                .map(|probe| probe.detail().to_string())
                .unwrap_or_else(|| "<none>".to_string()),
        };
        let scene = self.shell_scene();
        let summary = format!(
            "terminal_ok={} workspace_ok={} runtime_status={} bootstrap_completed={} last_runtime_reason={}",
            self.bootstrap.terminal_probe_ok,
            self.bootstrap.workspace_probe_ok,
            runtime_status,
            self.has_completed_bootstrap.get(),
            runtime_reason
        );
        let scene_changed = self.last_shell_scene_logged.get() != Some(scene);
        let summary_changed = *self.last_shell_gate_summary.borrow() != summary;
        if scene_changed || summary_changed {
            crate::quorp::tui::diagnostics::log_event(
                "shell.scene_state",
                json!({
                    "scene": format!("{scene:?}"),
                    "bootstrap_elapsed_ms": self.bootstrap_elapsed().as_millis(),
                    "bootstrap_frame_index": self.bootstrap.frame_index,
                    "terminal_probe_ok": self.bootstrap.terminal_probe_ok,
                    "workspace_probe_ok": self.bootstrap.workspace_probe_ok,
                    "runtime_status": runtime_status,
                    "bootstrap_completed": self.has_completed_bootstrap.get(),
                    "last_runtime_reason": runtime_reason,
                }),
            );
            self.last_shell_scene_logged.set(Some(scene));
            *self.last_shell_gate_summary.borrow_mut() = summary;
        }
    }

    fn bootstrap_provider(&self) -> crate::quorp::executor::InteractiveProviderKind {
        self.chat.current_provider_kind()
    }

    fn bootstrap_runtime_probe_label(&self) -> &'static str {
        match self.bootstrap_provider() {
            crate::quorp::executor::InteractiveProviderKind::Local => "SSD-MOE",
            crate::quorp::executor::InteractiveProviderKind::Ollama => "Ollama",
            crate::quorp::executor::InteractiveProviderKind::OpenAiCompatible => {
                "OpenAI-compatible"
            }
            crate::quorp::executor::InteractiveProviderKind::Nvidia => "NVIDIA NIM",
            crate::quorp::executor::InteractiveProviderKind::Codex => "Codex",
        }
    }

    fn bootstrap_subtitle(&self) -> String {
        match self.bootstrap_provider() {
            crate::quorp::executor::InteractiveProviderKind::Local => {
                "Verifying the terminal, workspace, local model runtime, and session state."
                    .to_string()
            }
            crate::quorp::executor::InteractiveProviderKind::Ollama => {
                "Verifying the terminal, workspace, Ollama endpoint, model availability, and session state."
                    .to_string()
            }
            crate::quorp::executor::InteractiveProviderKind::OpenAiCompatible => {
                "Verifying the terminal, workspace, HTTPS endpoint configuration, and session state."
                    .to_string()
            }
            crate::quorp::executor::InteractiveProviderKind::Nvidia => {
                "Verifying the terminal, workspace, NVIDIA NIM endpoint configuration, and session state."
                    .to_string()
            }
            crate::quorp::executor::InteractiveProviderKind::Codex => {
                "Verifying the terminal, workspace, executor session, and session state."
                    .to_string()
            }
        }
    }

    fn bootstrap_runtime_detail(&self) -> String {
        if let Some(probe) = self.bootstrap.remote_runtime_probe.as_ref()
            && !matches!(
                self.bootstrap_provider(),
                crate::quorp::executor::InteractiveProviderKind::Local
            )
        {
            return truncate_fit(probe.detail(), 52);
        }
        let runtime_probe_state = self.ssd_moe.bootstrap_state();
        let runtime_acquire = self.ssd_moe.acquire_metadata();
        let runtime_wait = self.ssd_moe.wait_metadata();
        let runtime_detail = match self.ssd_moe.status() {
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Running => {
                if let Some(acquire) = runtime_acquire.as_ref() {
                    format!(
                        "{} · instance {} · {} lease(s)",
                        acquire.base_url, acquire.instance_id, acquire.lease_count
                    )
                } else {
                    "local loopback runtime ready".to_string()
                }
            }
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Starting => {
                "attach-or-spawn in progress".to_string()
            }
            crate::quorp::tui::ssd_moe_tui::ModelStatus::WaitingForBroker => runtime_wait
                .as_ref()
                .map(|wait| wait.message.clone())
                .unwrap_or_else(|| "waiting for shared runtime availability".to_string()),
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Ready => {
                "model selected; waiting for runtime health".to_string()
            }
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Stopping => {
                "runtime is stopping".to_string()
            }
            crate::quorp::tui::ssd_moe_tui::ModelStatus::NotDownloaded => {
                "selected model is not downloaded".to_string()
            }
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Downloading { progress_pct, .. } => {
                format!("downloading model {:.1}%", progress_pct)
            }
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Packing {
                layer,
                total_layers,
            } => {
                format!("packing {layer}/{total_layers}")
            }
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Failed(message) => {
                truncate_fit(&message, 52)
            }
        };
        if matches!(
            self.ssd_moe.status(),
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Failed(_)
                | crate::quorp::tui::ssd_moe_tui::ModelStatus::Downloading { .. }
                | crate::quorp::tui::ssd_moe_tui::ModelStatus::Packing { .. }
                | crate::quorp::tui::ssd_moe_tui::ModelStatus::WaitingForBroker
        ) {
            truncate_fit(&runtime_probe_state.detail, 52)
        } else {
            runtime_detail
        }
    }

    fn bootstrap_failure_footer(&self) -> String {
        match self.bootstrap_provider() {
            crate::quorp::executor::InteractiveProviderKind::Local
                if matches!(
                    self.ssd_moe.status(),
                    crate::quorp::tui::ssd_moe_tui::ModelStatus::Failed(_)
                ) =>
            {
                "Startup blocked: fix the SSD-MOE runtime issue and restart Quorp.".to_string()
            }
            crate::quorp::executor::InteractiveProviderKind::Ollama
            | crate::quorp::executor::InteractiveProviderKind::OpenAiCompatible
                if matches!(
                    self.bootstrap.remote_runtime_probe,
                    Some(BootstrapRemoteRuntimeProbe::Failed(_))
                ) =>
            {
                format!("Startup blocked: {}.", self.bootstrap_runtime_detail())
            }
            _ => "Right rail booting now. / opens workflows once ready. Ctrl+k opens the control deck.".to_string(),
        }
    }

    fn bootstrap_ollama_model_id(&self) -> String {
        crate::quorp::tui::model_registry::chat_model_raw_id(self.chat.current_model_id())
            .to_string()
    }

    fn probe_ollama_runtime(model_id: &str) -> Result<String, String> {
        let base_url = crate::quorp::tui::chat_service::resolve_ollama_base_url(None)
            .map_err(|error| format!("invalid Ollama host: {error}"))?;
        let display_host = base_url
            .strip_suffix("/v1")
            .unwrap_or(base_url.as_str())
            .to_string();
        let models_url = format!("{}/models", base_url.trim_end_matches('/'));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("failed to start Ollama probe runtime: {error}"))?;
        runtime.block_on(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_millis(900))
                .build()
                .map_err(|error| format!("failed to build Ollama probe client: {error}"))?;
            let response = client.get(&models_url).send().await.map_err(|error| {
                if error.is_connect() || error.is_timeout() {
                    format!("Ollama unreachable at {display_host}")
                } else {
                    format!("Ollama probe failed at {display_host}: {error}")
                }
            })?;
            let status = response.status();
            if !status.is_success() {
                return Err(format!(
                    "Ollama endpoint returned HTTP {} at {}",
                    status.as_u16(),
                    display_host
                ));
            }
            let payload = response
                .json::<serde_json::Value>()
                .await
                .map_err(|error| {
                    format!("invalid Ollama models response from {display_host}: {error}")
                })?;
            let models = payload
                .get("data")
                .and_then(|value| value.as_array())
                .ok_or_else(|| {
                    format!(
                        "invalid Ollama models response from {display_host}: missing data array"
                    )
                })?;
            let available_models = models
                .iter()
                .filter_map(|entry| {
                    entry
                        .get("id")
                        .or_else(|| entry.get("model"))
                        .or_else(|| entry.get("name"))
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                })
                .collect::<Vec<_>>();
            if available_models
                .iter()
                .any(|candidate| candidate == model_id)
            {
                Ok(format!(
                    "endpoint ready at {} · model {} available",
                    display_host, model_id
                ))
            } else {
                let available = if available_models.is_empty() {
                    "<none>".to_string()
                } else {
                    available_models.join(", ")
                };
                Err(format!(
                    "model {} is not available at {} (found: {})",
                    model_id, display_host, available
                ))
            }
        })
    }

    fn probe_openai_compatible_runtime() -> Result<String, String> {
        let config = crate::quorp::provider_config::resolve_openai_compatible_runtime(None)
            .map_err(|error| format!("invalid OpenAI-compatible configuration: {error}"))?;
        Ok(format!(
            "endpoint configured at {} · auth {} ready",
            config.base_url, config.auth_mode
        ))
    }

    fn probe_nvidia_runtime() -> Result<String, String> {
        let config = crate::quorp::provider_config::resolve_nvidia_runtime(None)
            .map_err(|error| format!("invalid NVIDIA NIM configuration: {error}"))?;
        Ok(format!(
            "endpoint configured at {} · auth {} ready",
            config.base_url, config.auth_mode
        ))
    }

    fn refresh_remote_runtime_probe(&mut self) {
        match self.bootstrap_provider() {
            crate::quorp::executor::InteractiveProviderKind::Local => {
                self.bootstrap.remote_runtime_probe = None;
            }
            crate::quorp::executor::InteractiveProviderKind::Codex => {
                self.bootstrap.remote_runtime_probe = Some(BootstrapRemoteRuntimeProbe::Ready(
                    "executor session ready".to_string(),
                ));
            }
            crate::quorp::executor::InteractiveProviderKind::OpenAiCompatible => {
                if matches!(
                    self.bootstrap.remote_runtime_probe,
                    Some(BootstrapRemoteRuntimeProbe::Ready(_))
                        | Some(BootstrapRemoteRuntimeProbe::Failed(_))
                ) {
                    return;
                }
                self.bootstrap.remote_runtime_probe =
                    Some(match Self::probe_openai_compatible_runtime() {
                        Ok(detail) => BootstrapRemoteRuntimeProbe::Ready(detail),
                        Err(detail) => BootstrapRemoteRuntimeProbe::Failed(detail),
                    });
            }
            crate::quorp::executor::InteractiveProviderKind::Nvidia => {
                if matches!(
                    self.bootstrap.remote_runtime_probe,
                    Some(BootstrapRemoteRuntimeProbe::Ready(_))
                        | Some(BootstrapRemoteRuntimeProbe::Failed(_))
                ) {
                    return;
                }
                self.bootstrap.remote_runtime_probe = Some(match Self::probe_nvidia_runtime() {
                    Ok(detail) => BootstrapRemoteRuntimeProbe::Ready(detail),
                    Err(detail) => BootstrapRemoteRuntimeProbe::Failed(detail),
                });
            }
            crate::quorp::executor::InteractiveProviderKind::Ollama => {
                if matches!(
                    self.bootstrap.remote_runtime_probe,
                    Some(BootstrapRemoteRuntimeProbe::Ready(_))
                        | Some(BootstrapRemoteRuntimeProbe::Failed(_))
                ) {
                    return;
                }
                let model_id = self.bootstrap_ollama_model_id();
                self.bootstrap.remote_runtime_probe =
                    Some(match Self::probe_ollama_runtime(model_id.as_str()) {
                        Ok(detail) => BootstrapRemoteRuntimeProbe::Ready(detail),
                        Err(detail) => BootstrapRemoteRuntimeProbe::Failed(detail),
                    });
            }
        }
    }

    fn bootstrap_hard_gates_ready(&self) -> bool {
        if !(self.bootstrap.terminal_probe_ok && self.bootstrap.workspace_probe_ok) {
            return false;
        }
        match self.bootstrap_provider() {
            crate::quorp::executor::InteractiveProviderKind::Local => matches!(
                self.ssd_moe.status(),
                crate::quorp::tui::ssd_moe_tui::ModelStatus::Running
            ),
            crate::quorp::executor::InteractiveProviderKind::Ollama
            | crate::quorp::executor::InteractiveProviderKind::OpenAiCompatible
            | crate::quorp::executor::InteractiveProviderKind::Nvidia => matches!(
                self.bootstrap.remote_runtime_probe,
                Some(BootstrapRemoteRuntimeProbe::Ready(_))
            ),
            crate::quorp::executor::InteractiveProviderKind::Codex => true,
        }
    }

    fn bootstrap_status_from_runtime(&self) -> BootstrapStatus {
        if let Some(probe) = self.bootstrap.remote_runtime_probe.as_ref()
            && !matches!(
                self.bootstrap_provider(),
                crate::quorp::executor::InteractiveProviderKind::Local
            )
        {
            return probe.status();
        }
        match self.ssd_moe.status() {
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Running => BootstrapStatus::Ok,
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Starting => BootstrapStatus::Pending,
            crate::quorp::tui::ssd_moe_tui::ModelStatus::WaitingForBroker => {
                BootstrapStatus::Pending
            }
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Ready => BootstrapStatus::Pending,
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Stopping => BootstrapStatus::Pending,
            crate::quorp::tui::ssd_moe_tui::ModelStatus::NotDownloaded
            | crate::quorp::tui::ssd_moe_tui::ModelStatus::Downloading { .. }
            | crate::quorp::tui::ssd_moe_tui::ModelStatus::Packing { .. } => {
                BootstrapStatus::Pending
            }
            crate::quorp::tui::ssd_moe_tui::ModelStatus::Failed(_) => BootstrapStatus::Failed,
        }
    }

    fn poll_runtime_health_if_due(&mut self, now: Instant) {
        let should_poll = self
            .last_runtime_health_poll_at
            .borrow()
            .as_ref()
            .is_none_or(|last| now.duration_since(*last) >= RUNTIME_HEALTH_POLL_INTERVAL);
        if !should_poll {
            return;
        }
        match self.bootstrap_provider() {
            crate::quorp::executor::InteractiveProviderKind::Local => self.ssd_moe.poll_health(),
            crate::quorp::executor::InteractiveProviderKind::Ollama
            | crate::quorp::executor::InteractiveProviderKind::OpenAiCompatible
            | crate::quorp::executor::InteractiveProviderKind::Nvidia
            | crate::quorp::executor::InteractiveProviderKind::Codex => {
                self.refresh_remote_runtime_probe();
            }
        }
        *self.last_runtime_health_poll_at.borrow_mut() = Some(now);
    }

    pub fn poll_runtime_health(&mut self) {
        self.poll_runtime_health_if_due(Instant::now());
    }

    fn log_draw_perf_if_needed(
        &self,
        draw_ms: u128,
        snapshot_ms: u128,
        render_ms: u128,
        transcript_message_count: usize,
        segment_count: usize,
        code_block_count: usize,
    ) {
        if draw_ms <= DRAW_BUDGET_MS {
            return;
        }
        crate::quorp::tui::diagnostics::log_event(
            "tui.draw_perf",
            json!({
                "event_kind": "draw",
                "focused_pane": self.focused.display_label(),
                "transcript_message_count": transcript_message_count,
                "segment_count": segment_count,
                "code_block_count": code_block_count,
                "draw_ms": draw_ms,
                "snapshot_ms": snapshot_ms,
                "render_ms": render_ms,
            }),
        );
    }

    fn bootstrap_view_snapshot(&self, area: Rect) -> ShellBootstrapView {
        let runtime_status = self.bootstrap_status_from_runtime();
        let runtime_detail = self.bootstrap_runtime_detail();
        let model_id = self.chat.current_model_id().to_string();
        let path_index = self.chat.path_index_progress();
        let path_index_status = match path_index.phase {
            crate::quorp::tui::path_index::PathIndexPhase::Ready => BootstrapStatus::Ok,
            crate::quorp::tui::path_index::PathIndexPhase::Scanning => BootstrapStatus::Pending,
        };
        let pty_status = if self.bootstrap.pty_probe.is_ok() {
            BootstrapStatus::Ok
        } else if self.bootstrap.frame_index < 4 {
            BootstrapStatus::Pending
        } else {
            BootstrapStatus::Warn
        };
        let pty_detail = self
            .bootstrap
            .pty_probe
            .clone()
            .unwrap_or_else(|message| message);
        let session_status = if self.bootstrap.session_restore.is_ok() {
            BootstrapStatus::Ok
        } else {
            BootstrapStatus::Warn
        };
        let session_detail = self
            .bootstrap
            .session_restore
            .clone()
            .unwrap_or_else(|message| message);
        let footer = self.bootstrap_failure_footer();
        let mut probes = vec![
            BootstrapProbe {
                label: "Terminal".to_string(),
                status: if self.bootstrap.terminal_probe_ok {
                    BootstrapStatus::Ok
                } else {
                    BootstrapStatus::Failed
                },
                detail: "alternate screen ready".to_string(),
            },
            BootstrapProbe {
                label: "Workspace".to_string(),
                status: if self.bootstrap.workspace_probe_ok {
                    BootstrapStatus::Ok
                } else {
                    BootstrapStatus::Failed
                },
                detail: self.file_tree.root().display().to_string(),
            },
            BootstrapProbe {
                label: "Model selection".to_string(),
                status: if model_id.is_empty() {
                    BootstrapStatus::Warn
                } else {
                    BootstrapStatus::Ok
                },
                detail: if model_id.is_empty() {
                    "using fallback local model".to_string()
                } else {
                    model_id
                },
            },
        ];

        let loader_phase_label = if matches!(
            self.bootstrap_provider(),
            crate::quorp::executor::InteractiveProviderKind::Local
        ) {
            let broker_status = self.ssd_moe.broker_installation_status();
            let runtime_probe_state = self.ssd_moe.bootstrap_state();
            let runtime_diagnostic = runtime_probe_state.diagnostic.clone();
            let runtime_acquire = self.ssd_moe.acquire_metadata();
            probes.push(BootstrapProbe {
                label: "Broker".to_string(),
                status: if runtime_diagnostic.broker_probe.health.is_some() {
                    BootstrapStatus::Ok
                } else {
                    BootstrapStatus::Warn
                },
                detail: if let Some(acquire) = runtime_acquire.as_ref() {
                    format!(
                        "{} via {:?} at {}",
                        acquire.instance_id, acquire.disposition, acquire.base_url
                    )
                } else if let Some(health) = runtime_diagnostic.broker_probe.health.as_ref() {
                    format!(
                        "{} at {} · {} instance(s) · {} lease(s)",
                        health.status,
                        runtime_diagnostic
                            .broker_probe
                            .broker_url
                            .clone()
                            .unwrap_or_else(|| "loopback".to_string()),
                        health.instance_count,
                        health.lease_count
                    )
                } else if broker_status.installed_binary_exists {
                    format!(
                        "{}; binary at {}",
                        runtime_diagnostic
                            .broker_probe
                            .probe_error
                            .clone()
                            .unwrap_or_else(|| "shared runtime not responding".to_string()),
                        broker_status.expected_binary_path.display()
                    )
                } else {
                    format!(
                        "install shared broker at {}",
                        broker_status.expected_binary_path.display()
                    )
                },
            });
            runtime_probe_state.phase_label
        } else {
            self.bootstrap_runtime_probe_label().to_string()
        };

        probes.extend([
            BootstrapProbe {
                label: self.bootstrap_runtime_probe_label().to_string(),
                status: runtime_status,
                detail: runtime_detail,
            },
            BootstrapProbe {
                label: "PTY".to_string(),
                status: pty_status,
                detail: pty_detail,
            },
            BootstrapProbe {
                label: "Path index".to_string(),
                status: path_index_status,
                detail: match path_index.phase {
                    crate::quorp::tui::path_index::PathIndexPhase::Ready => {
                        format!("{} files indexed", path_index.files_seen)
                    }
                    crate::quorp::tui::path_index::PathIndexPhase::Scanning => {
                        format!("warming {} files", path_index.files_seen)
                    }
                },
            },
            BootstrapProbe {
                label: "Session restore".to_string(),
                status: session_status,
                detail: session_detail,
            },
        ]);

        ShellBootstrapView {
            subtitle: self.bootstrap_subtitle(),
            probes,
            footer,
            loader_frame: BootstrapLoader::frame(
                area,
                self.bootstrap.frame_index,
                loader_phase_label,
                &self.theme,
            ),
        }
    }

    fn refresh_quick_open_matches(&mut self) {
        self.quick_open.matches = self
            .chat
            .shell_quick_open_matches(&self.quick_open.query, 12);
        if self.quick_open.selected_index >= self.quick_open.matches.len() {
            self.quick_open.selected_index = self.quick_open.matches.len().saturating_sub(1);
        }
        self.invalidate_overlay_snapshot_cache();
    }

    fn open_quick_open(&mut self) {
        self.quick_open = QuickOpenState::new();
        self.refresh_quick_open_matches();
        self.set_overlay(Overlay::QuickOpen);
    }

    fn refresh_slash_command_matches(&mut self) {
        self.slash_command_deck.matches =
            slash_commands::filter_command_deck_entries(&self.slash_command_deck.query);
        if self.slash_command_deck.selected_index >= self.slash_command_deck.matches.len() {
            self.slash_command_deck.selected_index =
                self.slash_command_deck.matches.len().saturating_sub(1);
        }
        self.invalidate_overlay_snapshot_cache();
    }

    fn refresh_action_deck_matches(&mut self) {
        self.action_deck.matches = crate::quorp::tui::action_discovery::filter_action_deck_entries(
            &self.action_deck.query,
        );
        if self.action_deck.selected_index >= self.action_deck.matches.len() {
            self.action_deck.selected_index = self.action_deck.matches.len().saturating_sub(1);
        }
        self.invalidate_overlay_snapshot_cache();
    }

    fn open_slash_command_deck(&mut self) {
        self.slash_command_deck = SlashCommandDeckState::default();
        self.refresh_slash_command_matches();
        self.set_overlay(Overlay::SlashCommandDeck);
    }

    fn open_action_deck(&mut self) {
        self.action_deck = ActionDeckState::default();
        self.refresh_action_deck_matches();
        self.set_overlay(Overlay::ActionDeck);
    }

    fn accept_slash_command_selection(&mut self) {
        let Some(entry) = self
            .slash_command_deck
            .matches
            .get(self.slash_command_deck.selected_index)
            .copied()
        else {
            self.close_overlay();
            return;
        };
        self.chat.set_input_text(entry.template);
        self.close_overlay();
    }

    fn accept_action_deck_selection(&mut self) {
        let Some(entry) = self
            .action_deck
            .matches
            .get(self.action_deck.selected_index)
            .copied()
        else {
            self.close_overlay();
            return;
        };

        match entry.command {
            ActionDeckCommand::SetRailMode(Some(mode)) => {
                self.proof_rail.set_user_mode(mode);
            }
            ActionDeckCommand::SetRailMode(None) => {
                self.proof_rail.clear_user_mode();
            }
            ActionDeckCommand::AddWatchpoint(label) => {
                self.proof_rail.apply_event(&RailEvent::WatchpointAdded {
                    label: label.to_string(),
                });
                self.proof_rail.apply_event(&RailEvent::OneSecondStory {
                    summary: format!("Watchpoint armed: {label}."),
                });
            }
            ActionDeckCommand::InsertSlash(command) => {
                self.chat.execute_input(&self.theme, command);
                self.on_assistant_feed_content_changed(true);
            }
            ActionDeckCommand::OpenFirstTarget(kind) => {
                self.open_first_engage_target_by_kind(kind);
            }
            ActionDeckCommand::OpenDiffTarget => {
                if self.promote_current_preview_to_diff_lens() {
                    if let Some(target) = self.current_preview_engage_target() {
                        self.open_engage_target(target);
                    } else {
                        self.open_first_engage_target_by_kind(EngageTargetKind::ChangedFile);
                    }
                }
            }
        }
        self.close_overlay();
    }

    fn open_new_thread_prompt(&mut self) {
        self.new_thread_prompt.query.clear();
        self.new_thread_prompt.matches.clear();
        self.new_thread_prompt.selected_index = 0;
        self.refresh_new_thread_prompt_matches();
        self.set_overlay(Overlay::NewThreadPrompt);
    }

    fn explorer_overlay_lines(&self, max_rows: usize) -> Vec<Cow<'static, str>> {
        self.file_tree
            .explorer_rows_snapshot(max_rows)
            .into_iter()
            .map(|row| {
                Cow::Owned(format!(
                    "{}{}",
                    if row.selected { "> " } else { "  " },
                    row.label
                ))
            })
            .collect()
    }

    fn refresh_new_thread_prompt_matches(&mut self) {
        let query = self.new_thread_prompt.query.trim();
        let root_label = self
            .file_tree
            .root()
            .display()
            .to_string()
            .trim_end_matches(std::path::MAIN_SEPARATOR)
            .to_string();
        self.new_thread_prompt.matches = self
            .chat
            .shell_directory_matches(query, 12)
            .into_iter()
            .filter(|(_, path)| path.exists())
            .collect();
        if !self
            .new_thread_prompt
            .matches
            .iter()
            .any(|(_, path)| path == self.file_tree.root())
        {
            self.new_thread_prompt
                .matches
                .insert(0, (root_label, self.file_tree.root().to_path_buf()));
        }
        if self.new_thread_prompt.selected_index >= self.new_thread_prompt.matches.len() {
            self.new_thread_prompt.selected_index =
                self.new_thread_prompt.matches.len().saturating_sub(1);
        }
        self.invalidate_overlay_snapshot_cache();
    }

    fn resolve_new_thread_root(&self) -> Option<std::path::PathBuf> {
        if let Some((_, path)) = self
            .new_thread_prompt
            .matches
            .get(self.new_thread_prompt.selected_index)
        {
            return Some(path.clone());
        }
        let query = self.new_thread_prompt.query.trim();
        if query.is_empty() {
            return Some(self.file_tree.root().to_path_buf());
        }
        let candidate =
            if query.starts_with('/') || query.starts_with("..") || query.starts_with(".") {
                std::path::PathBuf::from(query)
            } else {
                self.file_tree.root().join(query)
            };
        Some(candidate)
    }

    fn close_new_thread_prompt(&mut self) {
        self.new_thread_prompt.query.clear();
        self.new_thread_prompt.matches.clear();
        self.new_thread_prompt.selected_index = 0;
        self.close_overlay();
    }

    fn open_assistant_feed_link_at_index(&mut self, index: usize) {
        if self.last_full_area.width == 0 || self.last_full_area.height == 0 {
            return;
        }
        let state = self.shell_state_snapshot(self.last_full_area);
        let Some(link) = state.center.feed_links.get(index) else {
            return;
        };
        if let Err(error) =
            self.open_target_in_shell_or_external(&link.target, EngageTargetKind::FeedLink)
        {
            log::error!("failed to open assistant link {:?}: {error}", link.target);
        }
        self.assistant_feed_active_link = Some(index);
    }

    fn open_active_assistant_feed_link(&mut self) {
        let state = self.shell_state_snapshot(self.last_full_area);
        if let Some(index) = self.assistant_feed_active_link {
            if let Some(link) = state.center.feed_links.get(index) {
                if let Err(error) =
                    self.open_target_in_shell_or_external(&link.target, EngageTargetKind::FeedLink)
                {
                    log::error!("failed to open assistant link {:?}: {error}", link.target);
                }
                return;
            }
            self.assistant_feed_active_link = None;
        }
        if let Some(link) = state
            .center
            .feed_links
            .iter()
            .find(|link| {
                link.row >= state.center.feed_scroll_top
                    && link.row < state.center.feed_scroll_top + state.center.feed_viewport_lines
            })
            .cloned()
        {
            if let Err(error) =
                self.open_target_in_shell_or_external(&link.target, EngageTargetKind::FeedLink)
            {
                log::error!("failed to open assistant link {:?}: {error}", link.target);
            }
            self.assistant_feed_active_link = state
                .center
                .feed_links
                .iter()
                .position(|candidate| candidate.target == link.target);
        }
    }

    fn move_active_assistant_feed_link(&mut self, delta: isize) {
        if self.last_full_area.width == 0 || self.last_full_area.height == 0 {
            return;
        }
        let state = self.shell_state_snapshot(self.last_full_area);
        let scroll_top = state.center.feed_scroll_top;
        let viewport = state.center.feed_viewport_lines;
        let visible_links: Vec<usize> = state
            .center
            .feed_links
            .iter()
            .enumerate()
            .filter(|(_, link)| {
                link.row >= scroll_top && link.row < scroll_top.saturating_add(viewport)
            })
            .map(|(index, _)| index)
            .collect();
        if visible_links.is_empty() {
            self.assistant_feed_active_link = None;
            return;
        }
        let mut position = visible_links
            .iter()
            .position(|index| Some(*index) == self.assistant_feed_active_link);
        if position.is_none() {
            position = Some(if delta > 0 {
                0
            } else {
                visible_links.len().saturating_sub(1)
            });
        }
        let next = if delta > 0 {
            (position.unwrap_or(0) + 1).min(visible_links.len().saturating_sub(1))
        } else {
            position.unwrap_or(0).saturating_sub(1)
        };
        if delta < 0 && visible_links.len() == 1 {
            self.assistant_feed_active_link = Some(visible_links[0]);
            return;
        }
        self.assistant_feed_active_link = Some(visible_links[next]);
    }

    fn collect_shell_engage_targets(&self, state: &ShellState) -> Vec<EngageTarget> {
        let mut targets = Vec::new();
        let mut seen = HashSet::new();

        for link in &state.center.feed_links {
            if let Some(target) = self.resolve_engage_target(
                &link.target,
                EngageTargetKind::FeedLink,
                self.path_has_diff_target(&link.target),
                "assistant feed",
            ) && seen.insert(target.key.clone())
            {
                targets.push(target);
            }
        }

        for file in &self.proof_rail.snapshot.files_touched {
            if let Some(target) = self.resolve_engage_target(
                file,
                EngageTargetKind::ChangedFile,
                true,
                "blast radius",
            ) && seen.insert(target.key.clone())
            {
                targets.push(target);
            }
        }

        for artifact in &self.proof_rail.snapshot.artifacts {
            if let Some(target) = self.resolve_engage_target(
                &artifact.path,
                EngageTargetKind::Artifact,
                false,
                "artifact",
            ) && seen.insert(target.key.clone())
            {
                targets.push(target);
            }
        }

        for tool in &self.proof_rail.snapshot.active_tools {
            if let Some(target) = self.resolve_engage_target(
                &tool.target,
                EngageTargetKind::ToolTarget,
                self.path_has_diff_target(&tool.target),
                "tool target",
            ) && seen.insert(target.key.clone())
            {
                targets.push(target);
            }
        }

        for line in self.terminal.shell_lines(64) {
            for token in extract_openable_tokens(&line) {
                if let Some(target) = self.resolve_engage_target(
                    &token,
                    EngageTargetKind::TerminalPath,
                    self.path_has_diff_target(&token),
                    "terminal output",
                ) && seen.insert(target.key.clone())
                {
                    targets.push(target);
                }
            }
        }

        targets
    }

    #[allow(clippy::disallowed_methods)]
    fn open_external_target_secondary(&mut self, target: &str) -> io::Result<()> {
        let mut command = if cfg!(target_os = "macos") {
            let mut command = Command::new("open");
            command.arg(target);
            command
        } else if cfg!(target_os = "windows") {
            let mut command = Command::new("cmd");
            command.args(["/C", "start", "", target]);
            command
        } else {
            let mut command = Command::new("xdg-open");
            command.arg(target);
            command
        };
        command.spawn().map(|_| ()).map_err(|error| {
            io::Error::new(error.kind(), format!("open {target:?} failed: {error}"))
        })
    }

    fn resolve_engage_target(
        &self,
        target: &str,
        kind: EngageTargetKind,
        diff_capable: bool,
        source: &'static str,
    ) -> Option<EngageTarget> {
        match resolve_target(target, self.file_tree.root(), kind, source, diff_capable)? {
            EngageResolution::Local(target) => Some(target),
            EngageResolution::External(_) => None,
        }
    }

    fn open_target_in_shell_or_external(
        &mut self,
        target: &str,
        kind: EngageTargetKind,
    ) -> io::Result<()> {
        match resolve_target(target, self.file_tree.root(), kind, "assistant link", false) {
            Some(EngageResolution::Local(target)) => {
                self.open_engage_target(target);
                Ok(())
            }
            Some(EngageResolution::External(target)) => {
                self.open_external_target_secondary(&target)
            }
            None => Ok(()),
        }
    }

    fn path_has_diff_target(&self, target: &str) -> bool {
        let Some(resolution) = resolve_target(
            target,
            self.file_tree.root(),
            EngageTargetKind::File,
            "diff check",
            false,
        ) else {
            return false;
        };
        let EngageResolution::Local(target) = resolution else {
            return false;
        };
        let path = target.path;
        self.proof_rail
            .snapshot
            .files_touched
            .iter()
            .any(|candidate| {
                resolve_target(
                    candidate,
                    self.file_tree.root(),
                    EngageTargetKind::ChangedFile,
                    "blast radius",
                    true,
                )
                .and_then(|resolution| match resolution {
                    EngageResolution::Local(target) => Some(target.path == path),
                    EngageResolution::External(_) => None,
                })
                .unwrap_or(false)
            })
    }

    fn sync_active_engage_target(&mut self, targets: &[EngageTarget]) {
        if targets.is_empty() {
            self.active_engage_target_key = None;
            return;
        }
        if let Some(current) = self.active_engage_target_key.as_ref()
            && targets.iter().any(|target| &target.key == current)
        {
            return;
        }
        self.active_engage_target_key = targets.first().map(|target| target.key.clone());
    }

    fn set_active_engage_target_key(&mut self, key: String, state: &ShellState) {
        self.active_engage_target_key = Some(key.clone());
        self.assistant_feed_active_link = state.center.feed_links.iter().position(|link| {
            self.resolve_engage_target(
                &link.target,
                EngageTargetKind::FeedLink,
                self.path_has_diff_target(&link.target),
                "assistant feed",
            )
            .is_some_and(|target| target.key == key)
        });
    }

    fn move_active_engage_target(&mut self, delta: isize) {
        if self.last_full_area.width == 0 || self.last_full_area.height == 0 {
            return;
        }
        let state = self.shell_state_snapshot(self.last_full_area);
        let targets = self.collect_shell_engage_targets(&state);
        if targets.is_empty() {
            self.active_engage_target_key = None;
            self.assistant_feed_active_link = None;
            return;
        }
        let next_index = if let Some(current_index) = self
            .active_engage_target_key
            .as_ref()
            .and_then(|key| targets.iter().position(|target| &target.key == key))
        {
            if delta >= 0 {
                (current_index + 1).min(targets.len().saturating_sub(1))
            } else {
                current_index.saturating_sub(1)
            }
        } else if delta >= 0 {
            0
        } else {
            targets.len().saturating_sub(1)
        };
        self.set_active_engage_target_key(targets[next_index].key.clone(), &state);
    }

    fn open_active_engage_target(&mut self) {
        if self.last_full_area.width == 0 || self.last_full_area.height == 0 {
            return;
        }
        let state = self.shell_state_snapshot(self.last_full_area);
        let targets = self.collect_shell_engage_targets(&state);
        if targets.is_empty() {
            return;
        }
        self.sync_active_engage_target(&targets);
        let target = self
            .active_engage_target_key
            .as_ref()
            .and_then(|key| targets.iter().find(|target| &target.key == key))
            .cloned()
            .unwrap_or_else(|| targets[0].clone());
        self.open_engage_target(target);
    }

    fn current_preview_engage_target(&self) -> Option<EngageTarget> {
        let current_path = self.editor_pane.active_preview_path()?;
        let diff_capable = self.path_has_diff_target(current_path.display().to_string().as_str());
        self.resolve_engage_target(
            current_path.display().to_string().as_str(),
            EngageTargetKind::File,
            diff_capable,
            "preview",
        )
    }

    fn promote_current_preview_to_diff_lens(&mut self) -> bool {
        let Some(target) = self.current_preview_engage_target() else {
            return false;
        };
        if !target.diff_capable {
            return false;
        }
        self.active_engage_target_key = Some(target.key);
        self.proof_rail.set_user_mode(RailMode::DiffReactor);
        true
    }

    fn open_first_engage_target_by_kind(&mut self, kind: EngageTargetKind) {
        if self.last_full_area.width == 0 || self.last_full_area.height == 0 {
            return;
        }
        let state = self.shell_state_snapshot(self.last_full_area);
        let targets = self.collect_shell_engage_targets(&state);
        if let Some(target) = targets.into_iter().find(|target| target.kind == kind) {
            self.open_engage_target(target);
        }
    }

    fn open_engage_target(&mut self, target: EngageTarget) {
        self.active_engage_target_key = Some(target.key.clone());
        self.assistant_feed_scroll_top = 0;
        self.assistant_feed_follow_latest = false;
        if matches!(target.kind, EngageTargetKind::Directory) {
            self.open_directory_preview(&target.path);
        } else {
            self.engage_preview_override = None;
            if path_within_project(&target.path, self.file_tree.root()) {
                self.file_tree.set_selected_file(Some(target.path.clone()));
            }
            self.editor_pane
                .open_preview_target(target.path.as_path(), self.file_tree.root());
            self.editor_pane.ensure_active_loaded(self.file_tree.root());
            self.editor_pane.focus_line(target.line);
        }
        self.set_focus(Pane::EditorPane);
        self.close_overlay();
    }

    fn open_directory_preview(&mut self, path: &Path) {
        self.assistant_feed_scroll_top = 0;
        self.assistant_feed_follow_latest = false;
        let mut entries = match fs::read_dir(path) {
            Ok(read_dir) => read_dir
                .flatten()
                .map(|entry| {
                    let path = entry.path();
                    if path.is_dir() {
                        format!("{}/", entry.file_name().to_string_lossy())
                    } else {
                        entry.file_name().to_string_lossy().to_string()
                    }
                })
                .collect::<Vec<_>>(),
            Err(error) => vec![format!("Error reading {}: {error}", path.display())],
        };
        entries.sort();
        let mut lines = vec![format!("Directory: {}", path.display())];
        lines.extend(
            entries
                .into_iter()
                .take(32)
                .map(|entry| format!("  {entry}")),
        );
        self.engage_preview_override = Some(EngagePreviewOverride {
            title: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Directory")
                .to_string(),
            lines,
        });
    }

    fn set_default_active_assistant_feed_link(&mut self, state: &ShellState) {
        let visible_link = state.center.feed_links.iter().position(|link| {
            link.row >= state.center.feed_scroll_top
                && link.row < state.center.feed_scroll_top + state.center.feed_viewport_lines
        });
        if state.center.feed_links.is_empty() {
            self.assistant_feed_active_link = None;
            return;
        }
        let Some(active_index) = self.assistant_feed_active_link else {
            self.assistant_feed_active_link = visible_link;
            return;
        };
        let valid = state
            .center
            .feed_links
            .get(active_index)
            .is_some_and(|link| {
                link.row >= state.center.feed_scroll_top
                    && link.row < state.center.feed_scroll_top + state.center.feed_viewport_lines
            });
        if !valid {
            self.assistant_feed_active_link = visible_link;
        }
    }

    fn confirm_new_thread_prompt(&mut self) {
        self.persist_workspace_state();
        self.refresh_new_thread_prompt_matches();
        let requested_root = self
            .resolve_new_thread_root()
            .unwrap_or_else(|| self.file_tree.root().to_path_buf());
        let requested_root = canonical_project_root(&requested_root);
        if let Ok(thread_id) = self.workspace_store.create_thread_for_root(&requested_root) {
            self.file_tree.set_root(requested_root.clone());
            self.chat.ensure_project_root(&requested_root);
            if let Ok(Some(snapshot)) = self.workspace_store.load_thread_snapshot(&thread_id) {
                self.chat.import_thread_snapshot(snapshot);
            }
            self.models_pane = ModelsPane::sync_from_chat(&self.chat);
            self.refresh_sidebar_cache();
            self.persist_workspace_state();
        }
        self.close_overlay();
    }

    fn activate_sidebar_project(&mut self, index: usize) {
        self.persist_workspace_state();
        let Some(project_id) = self.sidebar_project_ids.get(index).cloned() else {
            return;
        };
        let Some(thread_id) = self.workspace_store.activate_project(&project_id) else {
            return;
        };
        self.persist_workspace_state();
        if let Some(project) = self.workspace_store.active_project() {
            self.file_tree.set_root(project.root.clone());
            self.editor_pane.close_all_file_tabs(project.root.as_path());
            self.chat.ensure_project_root(project.root.as_path());
        }
        if let Ok(Some(snapshot)) = self.workspace_store.load_thread_snapshot(&thread_id) {
            self.chat.import_thread_snapshot(snapshot);
        }
        self.models_pane = ModelsPane::sync_from_chat(&self.chat);
        self.refresh_sidebar_cache();
    }

    fn activate_sidebar_thread(&mut self, index: usize) {
        self.persist_workspace_state();
        let Some(thread_id) = self.sidebar_thread_ids.get(index).cloned() else {
            return;
        };
        if self.workspace_store.activate_thread(&thread_id).is_none() {
            return;
        }
        if let Some(project) = self.workspace_store.active_project() {
            self.file_tree.set_root(project.root.clone());
            self.editor_pane.close_all_file_tabs(project.root.as_path());
            self.chat.ensure_project_root(project.root.as_path());
        }
        if let Ok(Some(snapshot)) = self.workspace_store.load_thread_snapshot(&thread_id) {
            self.chat.import_thread_snapshot(snapshot);
        }
        self.models_pane = ModelsPane::sync_from_chat(&self.chat);
        self.refresh_sidebar_cache();
    }

    fn accept_quick_open_selection(&mut self) {
        let Some((_, path)) = self
            .quick_open
            .matches
            .get(self.quick_open.selected_index)
            .cloned()
        else {
            return;
        };
        if let Some(target) = self.resolve_engage_target(
            path.display().to_string().as_str(),
            EngageTargetKind::File,
            self.path_has_diff_target(path.display().to_string().as_str()),
            "quick open",
        ) {
            self.open_engage_target(target);
        }
        self.close_overlay();
    }

    fn attach_current_context_to_chat(&mut self) {
        let selected = self
            .file_tree
            .selected_file()
            .map(|path| path.to_path_buf())
            .or_else(|| {
                self.editor_pane.shell_tab_pills(1).first().and_then(|_| {
                    self.file_tree
                        .selected_file()
                        .map(|path| path.to_path_buf())
                })
            });
        let Some(path) = selected else {
            return;
        };
        let label = path
            .strip_prefix(self.file_tree.root())
            .ok()
            .and_then(|relative| relative.to_str())
            .unwrap_or_else(|| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("file")
            });
        if self.chat.insert_context_link(label, &path).is_ok() {
            self.set_focus(Pane::Chat);
        }
    }

    pub fn bootstrap_completed(&self) -> bool {
        self.shell_scene() == ShellScene::Ready && self.has_completed_bootstrap.get()
    }

    pub fn advance_bootstrap_tick(&mut self) {
        self.bootstrap.frame_index = self
            .bootstrap
            .frame_index
            .saturating_add(1)
            .min(BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1));
    }

    pub fn mark_bootstrap_pty_result(&mut self, result: anyhow::Result<()>) {
        self.bootstrap.pty_probe = result
            .map(|_| "terminal grid ready".to_string())
            .map_err(|error| error.to_string());
    }

    fn shell_overlay_snapshot(&mut self) -> Option<ShellOverlay> {
        if matches!(self.overlay, Overlay::None) {
            self.overlay_snapshot_cache = None;
            return self
                .chat
                .shell_mention_popup_lines(8)
                .map(|lines| ShellOverlay {
                    title: "Mention Results".to_string(),
                    lines: lines.into_iter().map(Cow::Owned).collect(),
                });
        }
        if self.overlay_snapshot_cache.is_none() {
            let overlay = match self.overlay {
                Overlay::Help => Some(ActionDiscoveryModel::help_overlay_snapshot().clone()),
                Overlay::ModelPicker => Some(ActionDiscoveryModel::model_picker_overlay_snapshot(
                    &self.models_pane.entries,
                    self.models_pane.selected_index,
                )),
                Overlay::Explorer => Some(ShellOverlay {
                    title: "Files".to_string(),
                    lines: self.explorer_overlay_lines(18),
                }),
                Overlay::QuickOpen => Some(ActionDiscoveryModel::quick_open_overlay_snapshot(
                    &self.quick_open.query,
                    &self.quick_open.matches,
                    self.quick_open.selected_index,
                )),
                Overlay::SlashCommandDeck => {
                    Some(ActionDiscoveryModel::slash_command_overlay_snapshot(
                        &self.slash_command_deck.query,
                        &self.slash_command_deck.matches,
                        self.slash_command_deck.selected_index,
                    ))
                }
                Overlay::ActionDeck => Some(ActionDiscoveryModel::action_deck_overlay_snapshot(
                    &self.action_deck.query,
                    &self.action_deck.matches,
                    self.action_deck.selected_index,
                )),
                Overlay::NewThreadPrompt => {
                    Some(ActionDiscoveryModel::new_thread_overlay_snapshot(
                        &self.new_thread_prompt.query,
                        &self.new_thread_prompt.matches,
                        self.new_thread_prompt.selected_index,
                    ))
                }
                Overlay::None => None,
            };
            self.overlay_snapshot_cache = overlay;
        }
        self.overlay_snapshot_cache.clone()
    }

    pub fn shell_state_snapshot(&mut self, full: Rect) -> ShellState {
        let mut overlay = self.shell_overlay_snapshot();
        if overlay.is_none()
            && matches!(ShellLayoutMode::for_area(full), ShellLayoutMode::Compact)
            && self.focused == Pane::FileTree
        {
            overlay = Some(ShellOverlay {
                title: "Files".to_string(),
                lines: self.explorer_overlay_lines(18),
            });
        }
        let runtime_label = self.shell_runtime_label();
        let (runtime_model_label, runtime_state_label, runtime_state_kind) =
            self.shell_runtime_parts();
        let runtime_degraded = matches!(
            self.chat.current_provider_kind(),
            crate::quorp::executor::InteractiveProviderKind::Local
        ) && self.has_completed_bootstrap.get()
            && !matches!(
                self.ssd_moe.status(),
                crate::quorp::tui::ssd_moe_tui::ModelStatus::Running
            );
        let mut transcript = self.chat.shell_transcript_blocks(&self.theme);
        if let Some(status) = self.agent_pane.status_lines.last() {
            transcript.push(crate::quorp::tui::shell::AssistantBlock {
                role: "Status:",
                text: status.clone(),
                tone: AssistantTone::Muted,
                rich_lines: None,
            });
        }
        if transcript.is_empty() {
            transcript.push(crate::quorp::tui::shell::AssistantBlock {
                role: "Assistant:",
                text: "Ask about the current file, run a command, or request a summary."
                    .to_string(),
                tone: AssistantTone::Muted,
                rich_lines: None,
            });
        }

        let (main_title, main_lines) = self.shell_main_preview();

        let mut feed = if self.focused == Pane::EditorPane {
            vec![ShellFeedItem {
                title: main_title.clone(),
                lines: main_lines.clone(),
                rich_lines: None,
                tone: FeedItemTone::Muted,
                additions: 0,
                deletions: 0,
            }]
        } else {
            transcript
                .iter()
                .map(|block| {
                    let tone = match block.role.trim_end_matches(':') {
                        "User" => FeedItemTone::User,
                        "Reasoning" => FeedItemTone::Reasoning,
                        "Tool" => FeedItemTone::Tool,
                        "Command" => FeedItemTone::Command,
                        "Validation" => FeedItemTone::Validation,
                        "Files" => FeedItemTone::FileChange,
                        "Status" => FeedItemTone::Warning,
                        "Output" => FeedItemTone::Muted,
                        "Error" => FeedItemTone::Error,
                        _ => match block.tone {
                            AssistantTone::Normal => FeedItemTone::Assistant,
                            AssistantTone::Muted => FeedItemTone::Muted,
                            AssistantTone::Error => FeedItemTone::Error,
                            AssistantTone::Success => FeedItemTone::Success,
                        },
                    };
                    ShellFeedItem {
                        title: block.role.trim_end_matches(':').to_string(),
                        lines: block.text.lines().map(ToOwned::to_owned).collect(),
                        rich_lines: block.rich_lines.clone(),
                        tone,
                        additions: 0,
                        deletions: 0,
                    }
                })
                .collect::<Vec<_>>()
        };
        if runtime_degraded {
            let degrade_reason = self
                .ssd_moe
                .last_transition_reason()
                .unwrap_or_else(|| "runtime became unavailable after initial startup".to_string());
            let metadata_line = self
                .ssd_moe
                .acquire_metadata()
                .map(|metadata| {
                    format!(
                        "Last runtime {} at {} · leases={} · stale={}",
                        metadata.instance_id,
                        metadata.base_url,
                        metadata.lease_count,
                        metadata.stale
                    )
                })
                .unwrap_or_else(|| "No broker runtime metadata retained".to_string());
            feed.insert(
                0,
                ShellFeedItem {
                    title: "Runtime degraded".to_string(),
                    lines: vec![degrade_reason, metadata_line],
                    rich_lines: None,
                    tone: FeedItemTone::Error,
                    additions: 0,
                    deletions: 0,
                },
            );
        }

        let active_project = self.workspace_store.active_project();
        let active_thread = self.workspace_store.active_thread();
        let additions = active_thread.map(|thread| thread.additions).unwrap_or(0);
        let deletions = active_thread.map(|thread| thread.deletions).unwrap_or(0);
        let project_label = active_project
            .map(|project| project.display_name.clone())
            .unwrap_or_else(|| self.file_tree.root().display().to_string());
        let thread_title = active_thread
            .map(|thread| thread.title.clone())
            .unwrap_or_else(|| self.chat.shell_session_label());

        let sidebar = ShellSidebarView {
            projects: self
                .sidebar_project_ids
                .iter()
                .filter_map(|project_id| self.workspace_store.project(project_id))
                .map(|project| ShellProjectItem {
                    label: project.display_name.clone(),
                    status: self
                        .workspace_store
                        .project_status(&project.id)
                        .label()
                        .to_string(),
                    selected: self.workspace_store.state().active_project_id.as_ref()
                        == Some(&project.id),
                })
                .collect(),
            threads: self
                .sidebar_thread_ids
                .iter()
                .filter_map(|thread_id| self.workspace_store.thread(thread_id))
                .map(|thread| ShellThreadItem {
                    label: thread.title.clone(),
                    summary: thread.last_activity_summary.clone(),
                    status: thread.status.label().to_string(),
                    additions: thread.additions,
                    deletions: thread.deletions,
                    selected: self.workspace_store.state().active_thread_id.as_ref()
                        == Some(&thread.id),
                })
                .collect(),
            active_project_root: self.status_center_for_draw(),
        };

        let mut active_tool_orchestra = None;
        if self.chat.is_streaming()
            && let Some(ticks) = self.last_working_tick
        {
            let ms_since = std::time::Instant::now().duration_since(ticks).as_millis();
            if ms_since > 1200 {
                active_tool_orchestra = Some(crate::quorp::tui::tool_orchestra::ToolOrchestra {
                    agents: vec![crate::quorp::tui::tool_orchestra::OrchestraAgentState {
                        name: "Agent Swarm".to_string(),
                        task: format!("Zero Dark ({}s): Blocking operation", ms_since / 1000),
                        is_active: true,
                        progress: ((ms_since % 1000) / 10) as u8,
                    }],
                });
            }
        }

        let mut state = ShellState {
            scene: self.shell_scene(),
            experience_mode: self.shell_experience_mode(),
            app_name: "quorp".to_string(),
            version_label: "v2.01".to_string(),
            workspace_root: self.status_center_for_draw(),
            proof_rail_visible: true,
            diff_reactor: None,
            attention_lease: None,
            tool_orchestra: active_tool_orchestra,
            active_mode: if self.overlay.is_active() {
                match self.overlay {
                    Overlay::ActionDeck => "Control Deck".to_string(),
                    Overlay::SlashCommandDeck => "Workflow Deck".to_string(),
                    _ => "Overlay".to_string(),
                }
            } else {
                self.shell_experience_mode().label().to_string()
            },
            runtime_label: runtime_label.clone(),
            focus: self.shell_focus(overlay.is_some()),
            explorer_visible: !self.explorer_collapsed,
            assistant_overlay: false,
            explorer_items: self
                .file_tree
                .explorer_rows_snapshot(24)
                .into_iter()
                .map(|row| ShellExplorerItem {
                    label: row.label,
                    selected: row.selected,
                })
                .collect(),
            main: ShellMainView {
                title: main_title,
                mode: MainWorkspaceMode::Preview,
                lines: main_lines,
                terminal_title: self.terminal.shell_title(),
                terminal_lines: self.terminal.shell_lines(24),
                show_terminal_drawer: self.terminal_dock_open,
            },
            assistant: ShellAssistantView {
                session_label: self.chat.shell_session_label(),
                runtime_label: runtime_label.clone(),
                transcript,
                composer_text: self.chat.shell_composer_text(),
            },
            status_hint: if runtime_degraded {
                "Runtime lost after startup. Quorp is staying in the main shell while recovery runs."
                    .to_string()
            } else {
                self.shell_control_hint()
            },
            overlay: overlay.clone(),
            main_sessions: self.shell_main_session_pills(),
            assistant_sessions: self
                .chat
                .shell_session_pills(3)
                .into_iter()
                .map(|(label, active, streaming)| ShellSessionPill {
                    label,
                    tone: if streaming {
                        SessionPillTone::Busy
                    } else if active {
                        SessionPillTone::Active
                    } else {
                        SessionPillTone::Muted
                    },
                })
                .collect(),
            sidebar,
            center: ShellCenterView {
                thread_title,
                project_label,
                workspace_label: self.status_center_for_draw(),
                provider_label: self.chat.current_provider_label().to_string(),
                runtime_label,
                model_label: self.chat.current_model_display_label(),
                session_identity: self.chat.shell_session_identity(),
                runtime_model_label,
                runtime_state_label,
                runtime_state_kind,
                animation_phase: (self.draw_frame_seq % 4) as u8,
                feed,
                feed_scroll_top: self.assistant_feed_scroll_top,
                feed_total_lines: self.assistant_feed_total_lines,
                feed_viewport_lines: self.assistant_feed_viewport_lines,
                feed_scrollbar_hovered: self.assistant_feed_scrollbar_hovered,
                feed_lines: Vec::new(),
                feed_links: Vec::new(),
                active_feed_link: self.assistant_feed_active_link,
                composer_text: self.chat.shell_composer_text(),
                additions,
                deletions,
            },
            files: ShellDrawerView {
                title: "Files".to_string(),
                collapsed_label: "Files".to_string(),
                visible: !self.explorer_collapsed || self.focused == Pane::FileTree,
                badge_label: None,
                detail_label: None,
                lines: self
                    .file_tree
                    .explorer_rows_snapshot(24)
                    .into_iter()
                    .map(|row| format!("{}{}", if row.selected { "> " } else { "  " }, row.label))
                    .collect(),
                snapshot: None,
                fullscreen: false,
                capture_mode: false,
            },
            terminal: ShellDrawerView {
                title: self
                    .terminal
                    .shell_window_title()
                    .unwrap_or_else(|| self.terminal.shell_title()),
                collapsed_label: "Terminal".to_string(),
                visible: self.terminal_dock_open || self.focused == Pane::Terminal,
                badge_label: Some(self.terminal.shell_label()),
                detail_label: Some(self.terminal.shell_path_label(self.file_tree.root())),
                lines: self.terminal.shell_lines(24),
                snapshot: Some(self.terminal.snapshot()),
                fullscreen: self.focused == Pane::Terminal
                    && self.terminal.alternate_screen_active(),
                capture_mode: self.terminal.in_capture_mode(),
            },
            proof_rail: Some(self.proof_rail.clone()),
            bootstrap: Some(self.bootstrap_view_snapshot(full)),
        };

        let center_rect = ShellGeometry::for_state(full, &state).center;
        let center_inner = Rect::new(
            center_rect.x.saturating_add(1),
            center_rect.y.saturating_add(1),
            center_rect.width.saturating_sub(2),
            center_rect.height.saturating_sub(2),
        );
        let header_height = 2.min(center_inner.height);
        let composer_height = shell_composer_height_for_text(
            &state.center.composer_text,
            center_inner.width,
            center_inner.height.saturating_sub(header_height),
        );
        let feed_rect = Rect::new(
            center_inner.x,
            center_inner.y.saturating_add(header_height),
            center_inner.width,
            center_inner
                .height
                .saturating_sub(header_height)
                .saturating_sub(composer_height),
        );
        self.assistant_feed_viewport_lines = feed_rect.height.max(1) as usize;

        let mut feed_width = feed_rect.width as usize;
        let mut rendered_feed =
            ShellState::render_feed_lines(&state.center.feed, &self.theme, feed_width);
        let mut rendered_lines = std::mem::take(&mut rendered_feed.lines);
        state.center.feed_links = std::mem::take(&mut rendered_feed.links);
        if rendered_lines.len() > self.assistant_feed_viewport_lines && feed_width > 1 {
            feed_width -= 1;
            let mut rendered_feed =
                ShellState::render_feed_lines(&state.center.feed, &self.theme, feed_width);
            rendered_lines = std::mem::take(&mut rendered_feed.lines);
            state.center.feed_links = std::mem::take(&mut rendered_feed.links);
        }
        self.assistant_feed_total_lines = rendered_lines.len().max(1);
        self.clamp_assistant_feed_scroll();

        state.center.feed_scroll_top = self.assistant_feed_scroll_top;
        state.center.feed_total_lines = self.assistant_feed_total_lines;
        state.center.feed_viewport_lines = self.assistant_feed_viewport_lines;
        state.center.feed_lines = rendered_lines;
        self.set_default_active_assistant_feed_link(&state);
        state.center.active_feed_link = self.assistant_feed_active_link;
        state
    }

    pub fn draw_shell_preview(&mut self, frame: &mut Frame<'_>) {
        let draw_started_at = Instant::now();
        let full = frame.area();
        if full.width < 60 || full.height < 15 {
            let message = Paragraph::new("Terminal too small. Please resize (minimum 40×13).");
            frame.render_widget(message, full);
            return;
        }

        self.draw_frame_seq = self.draw_frame_seq.wrapping_add(1);
        if self.shell_scene() == ShellScene::Bootstrap {
            self.mark_bootstrap_visible();
        }
        self.log_shell_scene_state();
        self.update_compact_ui(full);
        self.chat.ensure_project_root(self.file_tree.root());
        self.editor_pane
            .sync_tree_selection(self.file_tree.selected_file(), self.file_tree.root());
        self.editor_pane.ensure_active_loaded(self.file_tree.root());

        let snapshot_started_at = Instant::now();
        let (transcript_message_count, segment_count, code_block_count) =
            self.chat.transcript_metrics();
        let state = self.shell_state_snapshot(full);
        let snapshot_ms = snapshot_started_at.elapsed().as_millis();
        let render_started_at = Instant::now();
        ShellRenderer::render(frame.buffer_mut(), full, &state, &self.theme);
        let render_ms = render_started_at.elapsed().as_millis();
        self.register_shell_hit_targets(full, &state);
        self.last_full_area = full;
        self.log_draw_perf_if_needed(
            draw_started_at.elapsed().as_millis(),
            snapshot_ms,
            render_ms,
            transcript_message_count,
            segment_count,
            code_block_count,
        );
    }

    fn shell_center_regions(&self, full: Rect, state: &ShellState) -> (Rect, Rect, Rect, Rect) {
        let geometry = ShellGeometry::for_state(full, state);
        let center = geometry.center;
        let inner = Rect::new(
            center.x.saturating_add(1),
            center.y.saturating_add(1),
            center.width.saturating_sub(2),
            center.height.saturating_sub(2),
        );
        let header = Rect::new(inner.x, inner.y, inner.width, 2.min(inner.height));
        let composer_height = shell_composer_height_for_text(
            &state.center.composer_text,
            inner.width,
            inner.height.saturating_sub(header.height),
        );
        let feed = Rect::new(
            inner.x,
            header.bottom(),
            inner.width,
            inner
                .height
                .saturating_sub(header.height)
                .saturating_sub(composer_height),
        );
        let show_scrollbar = state.center.feed_total_lines > feed.height as usize && feed.width > 1;
        let scrollbar = if show_scrollbar {
            Rect::new(feed.right().saturating_sub(1), feed.y, 1, feed.height)
        } else {
            Rect::new(feed.right(), feed.y, 0, feed.height)
        };
        let composer = Rect::new(inner.x, feed.bottom(), inner.width, composer_height);
        (header, feed, scrollbar, composer)
    }

    fn jump_assistant_feed_scrollbar(&mut self, row: u16) {
        let full = self.last_full_area;
        if full.width == 0 || full.height == 0 {
            return;
        }
        let state = self.shell_state_snapshot(full);
        let (_, feed, _, _) = self.shell_center_regions(full, &state);
        if feed.height == 0 {
            return;
        }
        let relative_row = row
            .saturating_sub(feed.y)
            .min(feed.height.saturating_sub(1)) as usize;
        let max_scroll = self.assistant_feed_max_scroll();
        if max_scroll == 0 {
            self.scroll_assistant_feed_to_bottom();
            return;
        }
        let target = max_scroll.saturating_mul(relative_row) / feed.height.max(1) as usize;
        self.assistant_feed_follow_latest = target >= max_scroll;
        self.assistant_feed_scroll_top = target.min(max_scroll);
    }

    fn register_shell_hit_targets(&mut self, full: Rect, state: &ShellState) {
        self.hitmap.clear();
        let geometry = ShellGeometry::for_state(full, state);
        let (assistant_header, assistant_feed, assistant_scrollbar, composer_rect) =
            self.shell_center_regions(full, state);
        if geometry.sidebar.width > 0 {
            self.hitmap.push(
                Rect::new(
                    geometry.sidebar.x,
                    geometry.sidebar.y,
                    geometry.sidebar.width,
                    1,
                ),
                HitTarget::SidebarNewThread,
            );
            let mut row = geometry.sidebar.y + 3;
            for (index, _) in self.sidebar_project_ids.iter().enumerate() {
                if row >= geometry.sidebar.bottom().saturating_sub(3) {
                    break;
                }
                self.hitmap.push(
                    Rect::new(geometry.sidebar.x, row, geometry.sidebar.width, 1),
                    HitTarget::SidebarProject(index),
                );
                row = row.saturating_add(1);
            }
            row = row.saturating_add(2);
            for (index, _) in self.sidebar_thread_ids.iter().enumerate() {
                if row >= geometry.sidebar.bottom().saturating_sub(2) {
                    break;
                }
                self.hitmap.push(
                    Rect::new(geometry.sidebar.x, row, geometry.sidebar.width, 2),
                    HitTarget::SidebarThread(index),
                );
                row = row.saturating_add(2);
            }
            self.hitmap.push(
                Rect::new(
                    geometry.sidebar.x,
                    geometry.sidebar.bottom().saturating_sub(1),
                    geometry.sidebar.width,
                    1,
                ),
                HitTarget::SidebarSettings,
            );
        }
        self.hitmap
            .push(assistant_header, HitTarget::AssistantHeader);
        self.hitmap.push(
            assistant_feed,
            if self.focused == Pane::EditorPane {
                HitTarget::LeafBody(Pane::EditorPane)
            } else {
                HitTarget::AssistantFeed
            },
        );
        let show_scrollbar = state.center.feed_total_lines > assistant_feed.height as usize
            && assistant_feed.width > 1;
        let text_rect = if show_scrollbar {
            Rect::new(
                assistant_feed.x,
                assistant_feed.y,
                assistant_feed.width.saturating_sub(1),
                assistant_feed.height,
            )
        } else {
            assistant_feed
        };
        let visible_start = state.center.feed_scroll_top;
        let visible_end = visible_start.saturating_add(state.center.feed_viewport_lines);
        for (index, link) in state.center.feed_links.iter().enumerate() {
            if link.row < visible_start || link.row >= visible_end {
                continue;
            }
            let row = text_rect.y + (link.row.saturating_sub(state.center.feed_scroll_top)) as u16;
            if text_rect.height == 0 || row >= text_rect.bottom() {
                continue;
            }
            let start_col = link.start_col.min(text_rect.width as usize);
            let end_col = link.end_col.min(text_rect.width as usize);
            if end_col <= start_col {
                continue;
            }
            let width = (end_col.saturating_sub(start_col)) as u16;
            if width == 0 {
                continue;
            }
            self.hitmap.push(
                Rect::new(text_rect.x.saturating_add(start_col as u16), row, width, 1),
                HitTarget::AssistantFeedLink(index),
            );
        }
        if assistant_scrollbar.width > 0 {
            self.hitmap
                .push(assistant_scrollbar, HitTarget::AssistantFeedScrollbar);
        }
        self.hitmap
            .push(composer_rect, HitTarget::ComposerInput(Pane::Chat));
        self.hitmap
            .push(geometry.files_rail, HitTarget::FileDrawerToggle);
        if let Some(files_drawer) = geometry.files_drawer {
            self.hitmap.push(files_drawer, HitTarget::ExplorerRow(0));
        }
        self.hitmap
            .push(geometry.terminal_bar, HitTarget::TerminalDrawerToggle);
        if let Some(dock) = geometry.terminal_drawer {
            self.hitmap.push(dock, HitTarget::LeafBody(Pane::Terminal));
        }
        if let Some(overlay) = geometry.overlay {
            self.hitmap
                .push(overlay, HitTarget::ComposerInput(Pane::Chat));
            if self.overlay == Overlay::NewThreadPrompt {
                let mut row = overlay.y.saturating_add(4);
                for index in 0..self.new_thread_prompt.matches.len() {
                    if row >= overlay.bottom().saturating_sub(1) {
                        break;
                    }
                    self.hitmap.push(
                        Rect::new(
                            overlay.x.saturating_add(1),
                            row,
                            overlay.width.saturating_sub(2),
                            1,
                        ),
                        HitTarget::NewThreadChooserRow(index),
                    );
                    row = row.saturating_add(1);
                }
            }
        }
    }

    pub fn handle_event(&mut self, event: Event) -> ControlFlow<(), ()> {
        if self.shell_scene() == ShellScene::Bootstrap {
            if let Event::Key(key) = event {
                if key.kind == KeyEventKind::Release {
                    return ControlFlow::Continue(());
                }
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(key.code, KeyCode::Char('c'))
                {
                    return ControlFlow::Break(());
                }
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    return ControlFlow::Break(());
                }
            }
            return ControlFlow::Continue(());
        }

        match event {
            Event::Key(key) => {
                if key.kind == KeyEventKind::Release {
                    return ControlFlow::Continue(());
                }

                if let Some(flow) = self.handle_overlay_key_event(&key) {
                    return flow;
                }

                let terminal_capture_active = self.focused == Pane::Terminal
                    && self.overlay == Overlay::None
                    && self.terminal.in_capture_mode();
                if terminal_capture_active {
                    if key.modifiers.contains(KeyModifiers::ALT) {
                        match key.code {
                            KeyCode::Char('1') => {
                                self.set_focus(Pane::FileTree);
                                if self.compact_ui {
                                    self.set_overlay(Overlay::Explorer);
                                }
                                return ControlFlow::Continue(());
                            }
                            KeyCode::Char('2') => {
                                self.set_overlay(Overlay::None);
                                self.set_focus(Pane::EditorPane);
                                return ControlFlow::Continue(());
                            }
                            KeyCode::Char('3') => {
                                self.set_overlay(Overlay::None);
                                self.set_focus(Pane::Chat);
                                return ControlFlow::Continue(());
                            }
                            KeyCode::Char('4') => {
                                self.set_overlay(Overlay::None);
                                self.terminal_dock_open = true;
                                self.set_focus(Pane::Terminal);
                                return ControlFlow::Continue(());
                            }
                            _ => {}
                        }
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        match key.code {
                            KeyCode::Char('`') => {
                                self.terminal_dock_open = !self.terminal_dock_open;
                                if !self.terminal_dock_open {
                                    self.terminal.enter_navigation_mode();
                                    self.set_focus(Pane::EditorPane);
                                }
                                return ControlFlow::Continue(());
                            }
                            KeyCode::Char('g') => {
                                self.terminal.enter_navigation_mode();
                                return ControlFlow::Continue(());
                            }
                            _ => {}
                        }
                    }
                    if let Ok(true) = self.terminal.try_handle_key(&key) {
                        return ControlFlow::Continue(());
                    }
                    return ControlFlow::Continue(());
                }

                if key.code == KeyCode::Char('?') && key.modifiers.is_empty() {
                    self.open_help_overlay();
                    return ControlFlow::Continue(());
                }

                if self.overlay == Overlay::None
                    && !self.chat.mention_popup_open()
                    && key.modifiers.contains(KeyModifiers::ALT)
                {
                    match key.code {
                        KeyCode::Enter => {
                            self.open_active_engage_target();
                            return ControlFlow::Continue(());
                        }
                        KeyCode::Down => {
                            self.move_active_engage_target(1);
                            return ControlFlow::Continue(());
                        }
                        KeyCode::Up => {
                            self.move_active_engage_target(-1);
                            return ControlFlow::Continue(());
                        }
                        _ => {}
                    }
                }

                if self.overlay == Overlay::None
                    && self.focused == Pane::EditorPane
                    && self.tab_strip_focus.is_none()
                    && key.modifiers.is_empty()
                    && matches!(key.code, KeyCode::Char('d') | KeyCode::Char('D'))
                    && self.promote_current_preview_to_diff_lens()
                {
                    return ControlFlow::Continue(());
                }

                if key.modifiers.contains(KeyModifiers::ALT) {
                    match key.code {
                        KeyCode::Char('1') => {
                            self.set_focus(Pane::FileTree);
                            if self.compact_ui {
                                self.set_overlay(Overlay::Explorer);
                            }
                            return ControlFlow::Continue(());
                        }
                        KeyCode::Char('2') => {
                            self.set_overlay(Overlay::None);
                            self.set_focus(Pane::EditorPane);
                            return ControlFlow::Continue(());
                        }
                        KeyCode::Char('3') => {
                            self.set_overlay(Overlay::None);
                            self.set_focus(Pane::Chat);
                            return ControlFlow::Continue(());
                        }
                        KeyCode::Char('4') => {
                            self.set_overlay(Overlay::None);
                            self.terminal_dock_open = true;
                            self.set_focus(Pane::Terminal);
                            return ControlFlow::Continue(());
                        }
                        _ => {}
                    }
                }

                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match key.code {
                        KeyCode::Char('k') => {
                            self.open_action_deck();
                            return ControlFlow::Continue(());
                        }
                        KeyCode::Char('b') => {
                            if self.compact_ui {
                                self.set_overlay(if self.overlay == Overlay::Explorer {
                                    Overlay::None
                                } else {
                                    Overlay::Explorer
                                });
                                self.set_focus(Pane::FileTree);
                            } else {
                                self.explorer_collapsed = !self.explorer_collapsed;
                                if self.explorer_collapsed && self.focused == Pane::FileTree {
                                    self.set_focus(Pane::EditorPane);
                                }
                            }
                            return ControlFlow::Continue(());
                        }
                        KeyCode::Char('`') => {
                            self.terminal_dock_open = !self.terminal_dock_open;
                            if self.terminal_dock_open {
                                self.set_focus(Pane::Terminal);
                            } else if self.focused == Pane::Terminal {
                                self.set_focus(Pane::EditorPane);
                            }
                            return ControlFlow::Continue(());
                        }
                        KeyCode::Char('p') => {
                            self.open_quick_open();
                            return ControlFlow::Continue(());
                        }
                        KeyCode::Char('n') => {
                            self.open_new_thread_prompt();
                            return ControlFlow::Continue(());
                        }
                        KeyCode::Char('g') if self.focused == Pane::Terminal => {
                            self.terminal.enter_navigation_mode();
                            return ControlFlow::Continue(());
                        }
                        _ => {}
                    }
                }

                if key.code == KeyCode::Char(' ')
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !matches!(self.focused, Pane::Chat | Pane::Terminal)
                {
                    self.attach_current_context_to_chat();
                    return ControlFlow::Continue(());
                }

                if self.focused != Pane::Terminal && self.try_handle_vim_pane_navigation(&key) {
                    return ControlFlow::Continue(());
                }

                if matches!(self.focused, Pane::EditorPane | Pane::Chat)
                    && key.code == KeyCode::Up
                    && key.modifiers.contains(KeyModifiers::ALT)
                {
                    self.tab_strip_focus = Some(self.focused);
                    return ControlFlow::Continue(());
                }

                if self.try_handle_tab_strip_keys(&key) {
                    return ControlFlow::Continue(());
                }

                if self.focused == Pane::Chat
                    && key.code == KeyCode::Char('t')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    self.chat.new_chat_session(&self.theme);
                    return ControlFlow::Continue(());
                }

                if self.focused == Pane::FileTree {
                    match self.file_tree.handle_key_event(&key) {
                        FileTreeKeyOutcome::NotHandled => {}
                        FileTreeKeyOutcome::Handled => {
                            return ControlFlow::Continue(());
                        }
                        FileTreeKeyOutcome::OpenedFile => {
                            if let Some(path) = self.file_tree.selected_file()
                                && let Some(target) = self.resolve_engage_target(
                                    path.display().to_string().as_str(),
                                    EngageTargetKind::File,
                                    self.path_has_diff_target(path.display().to_string().as_str()),
                                    "file tree",
                                )
                            {
                                self.active_engage_target_key = Some(target.key);
                                self.engage_preview_override = None;
                            }
                            self.set_focus(Pane::EditorPane);
                            return ControlFlow::Continue(());
                        }
                    }
                }
                if self.focused == Pane::Chat
                    && self.overlay == Overlay::None
                    && self.tab_strip_focus.is_none()
                    && !self.chat.mention_popup_open()
                    && key.modifiers.is_empty()
                {
                    match key.code {
                        KeyCode::PageUp => {
                            self.page_assistant_feed_up();
                            return ControlFlow::Continue(());
                        }
                        KeyCode::PageDown => {
                            self.page_assistant_feed_down();
                            return ControlFlow::Continue(());
                        }
                        _ => {}
                    }
                }
                if self.overlay == Overlay::None
                    && self.tab_strip_focus.is_none()
                    && self.focused != Pane::Chat
                    && self.focused != Pane::Terminal
                    && key.modifiers.is_empty()
                {
                    let rail_mode = match key.code {
                        KeyCode::Char('d') => Some(RailMode::DiffReactor),
                        KeyCode::Char('v') => Some(RailMode::VerifyRadar),
                        KeyCode::Char('r') => Some(RailMode::TraceLens),
                        KeyCode::Char('t') => Some(RailMode::TimelineScrubber),
                        KeyCode::Char('m') => Some(RailMode::MemoryViewport),
                        KeyCode::Char('o') => Some(RailMode::ToolOrchestra),
                        _ => None,
                    };
                    if let Some(mode) = rail_mode {
                        self.proof_rail.set_user_mode(mode);
                        return ControlFlow::Continue(());
                    }
                }
                if self.focused == Pane::EditorPane && self.editor_pane.handle_key_event(&key) {
                    return ControlFlow::Continue(());
                }
                if self.focused == Pane::Chat
                    && self.overlay == Overlay::None
                    && self.tab_strip_focus.is_none()
                    && !self.chat.mention_popup_open()
                    && key.modifiers.is_empty()
                    && key.code == KeyCode::Char('/')
                    && self.chat.composer_is_empty()
                {
                    self.open_slash_command_deck();
                    return ControlFlow::Continue(());
                }
                if self.focused == Pane::Chat && self.chat.handle_key_event(&key, &self.theme) {
                    let force_follow = self.chat.take_shell_feed_submitted();
                    if self.chat.take_shell_feed_dirty() || force_follow {
                        self.on_assistant_feed_content_changed(force_follow);
                    }
                    return ControlFlow::Continue(());
                }
                if self.focused == Pane::Agent
                    && let Ok(true) = self.agent_pane.try_handle_key(&key)
                {
                    return ControlFlow::Continue(());
                }
                if self.focused == Pane::Terminal {
                    if !self.terminal.in_capture_mode()
                        && key.modifiers.is_empty()
                        && key.code == KeyCode::Enter
                    {
                        self.terminal.enter_capture_mode();
                        return ControlFlow::Continue(());
                    }
                    match self.terminal.try_handle_key(&key) {
                        Ok(true) => return ControlFlow::Continue(()),
                        Ok(false) => {
                            if self.try_handle_vim_pane_navigation(&key) {
                                return ControlFlow::Continue(());
                            }
                        }
                        Err(e) => log::error!("tui: terminal key handling failed: {e:#}"),
                    }
                }
                match key.code {
                    KeyCode::Tab => {
                        if self.tab_strip_focus.is_some() {
                            return ControlFlow::Continue(());
                        }
                        if key.modifiers.contains(KeyModifiers::SHIFT) {
                            self.set_focus(self.focused.prev());
                        } else {
                            self.set_focus(self.focused.next());
                        }
                    }
                    KeyCode::BackTab => {
                        if self.tab_strip_focus.is_some() {
                            return ControlFlow::Continue(());
                        }
                        self.set_focus(self.focused.prev());
                    }
                    KeyCode::Char('m') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if self.overlay == Overlay::ModelPicker {
                            self.close_overlay();
                        } else {
                            self.models_pane = ModelsPane::sync_from_chat(&self.chat);
                            if !self.models_pane.entries.is_empty() {
                                self.models_pane.selected_index = self
                                    .chat
                                    .model_index()
                                    .min(self.models_pane.entries.len() - 1);
                            }
                            self.invalidate_overlay_snapshot_cache();
                            self.set_overlay(Overlay::ModelPicker);
                        }
                        return ControlFlow::Continue(());
                    }
                    KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.ssd_moe.stop();
                        return ControlFlow::Continue(());
                    }
                    KeyCode::Esc => {
                        if self.tab_strip_focus.take().is_some() {
                            return ControlFlow::Continue(());
                        }
                        if self.overlay == Overlay::None
                            && self.proof_rail.user_mode_override.is_some()
                            && self.shell_experience_mode() != ShellExperienceMode::LegacyWorkbench
                        {
                            self.proof_rail.clear_user_mode();
                            return ControlFlow::Continue(());
                        }
                        return ControlFlow::Break(());
                    }
                    KeyCode::Char('c')
                        if key.modifiers.contains(KeyModifiers::CONTROL)
                            && self.focused != Pane::Terminal =>
                    {
                        return ControlFlow::Break(());
                    }
                    _ => {}
                }
            }
            Event::Mouse(mouse) => {
                self.handle_mouse_event(mouse);
            }
            Event::Resize(_, _) => {
                self.prismforge_split_ratio_lock = None;
                self.on_assistant_feed_content_changed(false);
            }
            Event::Paste(text) => {
                if self.focused == Pane::Terminal
                    && self.overlay == Overlay::None
                    && self.terminal.in_capture_mode()
                {
                    if let Err(error) = self.terminal.handle_paste(&text) {
                        log::error!("tui: terminal paste failed: {error:#}");
                    }
                } else if self.focused == Pane::Chat {
                    for character in text.chars() {
                        self.chat.handle_key_event(
                            &KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
                            &self.theme,
                        );
                    }
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }

    pub fn handle_mouse_click(&mut self, col: u16, row: u16) {
        if self.overlay.is_help() {
            self.close_overlay();
            return;
        }

        if let Some(index) = self.hit_splitter_index_at(col, row) {
            self.splitter_visual_state = SplitterVisualState::Dragging { index };
            self.apply_drag_to_splitter(index, col, row);
            return;
        }

        if let Some(hit) = self.hitmap.hit(col, row).copied() {
            match hit {
                HitTarget::LeafTab { leaf, tab } => {
                    self.tab_strip_focus = Some(leaf);
                    self.set_focus(leaf);
                    let root = self.file_tree.root();
                    match leaf {
                        Pane::EditorPane => {
                            self.editor_pane.activate_file_tab(tab, root);
                        }
                        Pane::Agent => {}
                        Pane::Chat => {
                            self.chat.activate_chat_session(tab, &self.theme);
                        }
                        _ => {}
                    }
                }
                HitTarget::LeafTabClose { leaf, tab } => {
                    let root = self.file_tree.root();
                    match leaf {
                        Pane::EditorPane => {
                            self.editor_pane.close_file_tab_at(tab, root);
                        }
                        Pane::Agent => {}
                        Pane::Chat => {
                            self.chat.close_chat_session_at(tab, &self.theme);
                        }
                        _ => {}
                    }
                    self.set_focus(leaf);
                }
                HitTarget::LeafBody(leaf) => {
                    self.tab_strip_focus = None;
                    self.set_focus(leaf);
                }
                HitTarget::AssistantFeed => {
                    self.tab_strip_focus = None;
                    self.set_focus(Pane::Chat);
                }
                HitTarget::AssistantFeedScrollbar => {
                    self.tab_strip_focus = None;
                    self.set_focus(Pane::Chat);
                    self.jump_assistant_feed_scrollbar(row);
                }
                HitTarget::AssistantFeedLink(index) => {
                    self.tab_strip_focus = None;
                    self.set_focus(Pane::Chat);
                    self.open_assistant_feed_link_at_index(index);
                }
                HitTarget::AssistantHeader => {
                    self.tab_strip_focus = None;
                    if self.focused == Pane::EditorPane {
                        self.set_focus(Pane::EditorPane);
                    } else {
                        self.set_focus(Pane::Chat);
                    }
                }
                HitTarget::NewThreadChooserRow(index) => {
                    self.new_thread_prompt.selected_index = index;
                    self.confirm_new_thread_prompt();
                }
                HitTarget::PanelTab { leaf, .. } => {
                    self.set_focus(leaf);
                }
                HitTarget::ComposerInput(leaf) => {
                    self.tab_strip_focus = None;
                    self.set_focus(leaf);
                }
                HitTarget::ExplorerRow(_) | HitTarget::ExplorerMenu | HitTarget::Activity(_) => {
                    self.set_focus(Pane::FileTree);
                }
                HitTarget::SidebarNewThread => {
                    self.open_new_thread_prompt();
                }
                HitTarget::SidebarProject(index) => {
                    self.activate_sidebar_project(index);
                    self.set_focus(Pane::Chat);
                }
                HitTarget::SidebarThread(index) => {
                    self.activate_sidebar_thread(index);
                    self.set_focus(Pane::Chat);
                }
                HitTarget::SidebarSettings => {
                    self.open_help_overlay();
                }
                HitTarget::FileDrawerToggle => {
                    self.explorer_collapsed = !self.explorer_collapsed;
                    if !self.explorer_collapsed {
                        self.set_focus(Pane::FileTree);
                    }
                }
                HitTarget::TerminalDrawerToggle => {
                    self.terminal_dock_open = !self.terminal_dock_open;
                    if self.terminal_dock_open {
                        self.set_focus(Pane::Terminal);
                    } else if self.focused == Pane::Terminal {
                        self.set_focus(Pane::Chat);
                    }
                }
                _ => {}
            }
        }
    }

    fn try_handle_tab_strip_keys(&mut self, key: &KeyEvent) -> bool {
        let Some(strip_leaf) = self.tab_strip_focus else {
            return false;
        };
        if self.focused != strip_leaf {
            self.tab_strip_focus = None;
            return false;
        }
        let root = self.file_tree.root();
        match key.code {
            KeyCode::Esc => {
                self.tab_strip_focus = None;
                true
            }
            KeyCode::Tab | KeyCode::BackTab => true,
            KeyCode::Left => {
                match strip_leaf {
                    Pane::EditorPane => {
                        self.editor_pane.cycle_file_tab(-1, root);
                    }
                    Pane::Agent => {}
                    Pane::Chat => {
                        self.chat.cycle_chat_session(-1, &self.theme);
                    }
                    _ => {}
                }
                true
            }
            KeyCode::Right => {
                match strip_leaf {
                    Pane::EditorPane => {
                        self.editor_pane.cycle_file_tab(1, root);
                    }
                    Pane::Agent => {}
                    Pane::Chat => {
                        self.chat.cycle_chat_session(1, &self.theme);
                    }
                    _ => {}
                }
                true
            }
            KeyCode::Delete => {
                match strip_leaf {
                    Pane::EditorPane => {
                        let i = self.editor_pane.active_tab_index();
                        self.editor_pane.close_file_tab_at(i, root);
                    }
                    Pane::Agent => {}
                    Pane::Chat => {
                        let i = self.chat.active_session_index();
                        self.chat.close_chat_session_at(i, &self.theme);
                    }
                    _ => {}
                }
                true
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    match strip_leaf {
                        Pane::EditorPane => {
                            self.editor_pane.close_all_file_tabs(root);
                        }
                        Pane::Agent => {}
                        Pane::Chat => {
                            self.chat.close_all_chat_sessions(&self.theme);
                        }
                        _ => {}
                    }
                } else {
                    match strip_leaf {
                        Pane::EditorPane => {
                            let i = self.editor_pane.active_tab_index();
                            self.editor_pane.close_file_tab_at(i, root);
                        }
                        Pane::Agent => {}
                        Pane::Chat => {
                            let i = self.chat.active_session_index();
                            self.chat.close_chat_session_at(i, &self.theme);
                        }
                        _ => {}
                    }
                }
                true
            }
            _ => false,
        }
    }
}

impl TuiApp {
    fn new_fixture_inner(
        tx: std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
        handle: tokio::runtime::Handle,
        fixture_root: std::path::PathBuf,
        runtime: Option<tokio::runtime::Runtime>,
        event_rx_keepalive: Option<std::sync::mpsc::Receiver<crate::quorp::tui::TuiEvent>>,
        unified_language_model_boot: Option<(
            futures::channel::mpsc::UnboundedSender<crate::quorp::tui::bridge::TuiToBackendRequest>,
            Vec<String>,
            usize,
        )>,
    ) -> Self {
        #[cfg(test)]
        let _ssd_moe_env_lock = crate::quorp::tui::ssd_moe_tui::test_env_lock();
        #[cfg(test)]
        let test_model_config_guard =
            Some(crate::quorp::tui::model_registry::isolated_test_model_config_guard());
        let mut ssd_moe = SsdMoeManager::new();
        ssd_moe.set_status_for_test(crate::quorp::tui::ssd_moe_tui::ModelStatus::Running);
        if let Some(model) = crate::quorp::tui::model_registry::local_moe_catalog()
            .into_iter()
            .next()
        {
            ssd_moe.set_active_model_for_test(Some(model));
        }
        let path_index = std::sync::Arc::new(crate::quorp::tui::path_index::PathIndex::new(
            fixture_root.clone(),
        ));
        let uses_language_model_registry = unified_language_model_boot.is_some();
        let registry_fixture_model_index = unified_language_model_boot
            .as_ref()
            .map(|(_, _, model_index)| *model_index);
        let mut chat = ChatPane::new(
            tx,
            fixture_root.clone(),
            path_index,
            unified_language_model_boot.clone(),
            None,
        );
        if uses_language_model_registry {
            #[cfg(test)]
            {
                let fixture_models = unified_language_model_boot
                    .as_ref()
                    .map(|(_, models, _)| models.clone())
                    .unwrap_or_default();
                let initial_model_index = if let Some(saved_chat_model_id) =
                    crate::quorp::tui::model_registry::get_saved_chat_model_id()
                {
                    fixture_models
                        .iter()
                        .position(|model| model == &saved_chat_model_id)
                        .unwrap_or_else(|| registry_fixture_model_index.unwrap_or(0))
                } else {
                    registry_fixture_model_index.unwrap_or(0)
                };
                chat.set_models_for_test(fixture_models, initial_model_index);
            }
        } else {
            chat.set_model_index_for_test(0);
        }
        let models_pane = ModelsPane::sync_from_chat(&chat);
        let theme = Theme::session_default();
        let mut bootstrap = BootstrapProgress::new(fixture_root.as_path());
        bootstrap.remote_runtime_probe =
            bootstrap_remote_probe_for_provider(chat.current_provider_kind());
        bootstrap.visible_started_at = Some(Instant::now() - BOOTSTRAP_MIN_DURATION);
        bootstrap.frame_index = BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1);
        Self {
            focused: Pane::EditorPane,
            right_pane: Pane::Chat,
            last_left_pane: Pane::EditorPane,
            file_tree: FileTree::with_root(fixture_root.clone()),
            editor_pane: EditorPane::new(),
            terminal: TerminalPane::with_bridge(
                unified_language_model_boot
                    .as_ref()
                    .map(|(tx, _, _)| tx.clone()),
            ),
            agent_pane: AgentPane::new(),
            chat,
            models_pane,
            ssd_moe,
            _runtime: runtime,
            _event_rx_keepalive: event_rx_keepalive,
            overlay: Overlay::None,
            overlay_snapshot_cache: None,
            last_full_area: Rect::default(),
            theme,
            hitmap: HitMap::new(),
            workspace: crate::quorp::tui::workbench::default_core_tui_tree(),
            visual_status_center_override: Some("/fixture/project".to_string()),
            visual_title_override: None,
            visual_status_left_override: None,
            visual_status_right_override: None,
            prismforge_dynamic_layout: false,
            prismforge_split_ratio_lock: None,
            splitter_visual_state: SplitterVisualState::Idle,
            tab_strip_focus: None,
            compact_ui: false,
            draw_frame_seq: 0,
            explorer_collapsed: false,
            terminal_dock_open: false,
            assistant_feed_scroll_top: 0,
            assistant_feed_follow_latest: true,
            assistant_feed_total_lines: 1,
            assistant_feed_viewport_lines: 1,
            assistant_feed_scrollbar_hovered: false,
            assistant_feed_active_link: None,
            active_engage_target_key: None,
            engage_preview_override: None,
            bootstrap,
            quick_open: QuickOpenState::new(),
            new_thread_prompt: NewThreadPrompt::default(),
            slash_command_deck: SlashCommandDeckState::default(),
            action_deck: ActionDeckState::default(),
            workspace_store: {
                #[cfg(test)]
                {
                    WorkspaceStore::load_or_create_ephemeral(fixture_root.as_path())
                }
                #[cfg(not(test))]
                {
                    WorkspaceStore::load_or_create(fixture_root.as_path())
                }
            },
            sidebar_project_ids: Vec::new(),
            sidebar_thread_ids: Vec::new(),
            has_completed_bootstrap: Cell::new(false),
            last_shell_scene_logged: Cell::new(None),
            last_shell_gate_summary: RefCell::new(String::new()),
            last_runtime_health_poll_at: RefCell::new(None),
            agent_runtime_tx: None,
            last_working_tick: None,
            #[cfg(test)]
            _test_model_config_guard: test_model_config_guard,
            proof_rail: ProofRailState::default(),
        }
    }
}

#[cfg(test)]
impl TuiApp {
    pub fn leak_runtime_for_test_exit(&mut self) {
        std::mem::forget(self._runtime.take());
    }

    pub fn set_last_runtime_health_poll_at_for_test(&self, instant: Instant) {
        *self.last_runtime_health_poll_at.borrow_mut() = Some(instant);
    }

    pub fn force_bootstrap_for_test(&mut self) {
        self.has_completed_bootstrap.set(false);
        self.bootstrap.visible_started_at = Some(Instant::now());
        self.bootstrap.frame_index = BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1);
    }

    pub fn complete_bootstrap_for_test(&self) {
        self.has_completed_bootstrap.set(true);
    }

    pub fn shell_engage_target_keys_for_test(
        &mut self,
        full: Rect,
    ) -> Vec<(String, EngageTargetKind)> {
        let state = self.shell_state_snapshot(full);
        self.collect_shell_engage_targets(&state)
            .into_iter()
            .map(|target| (self.shell_target_label(&target), target.kind))
            .collect()
    }
}

impl TuiApp {
    /// Deterministic app state for visual regression (no SSD-MOE autostart, fixed model index).
    pub fn new_for_visual_regression(
        tx: std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
        handle: tokio::runtime::Handle,
        fixture_root: std::path::PathBuf,
    ) -> Self {
        Self::new_fixture_inner(tx, handle, fixture_root, None, None, None)
    }

    /// PrismForge theme, Mock1 workbench tree, and title/status overrides for heuristic scoring vs `prismforge_target.png`.
    pub fn new_for_prismforge_regression(
        tx: std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
        handle: tokio::runtime::Handle,
        fixture_root: std::path::PathBuf,
    ) -> Self {
        let mut app = Self::new_fixture_inner(tx, handle, fixture_root, None, None, None);
        app.theme = Theme::prism_forge();
        app.workspace = crate::quorp::tui::workbench::default_prismforge_tree();
        app.prismforge_dynamic_layout = true;
        app.visual_title_override = Some("PrismForge — quorp-tui".to_string());
        app.visual_status_left_override = Some("main • 3 agents • 0 errors • 12 tasks".to_string());
        app.visual_status_right_override = Some("SSD-MOE • Online".to_string());
        let planner = app.file_tree.root().join("planner.rs");
        let plan_md = app.file_tree.root().join("multi_tab_preview.plan.md");
        let renderer = app.file_tree.root().join("renderer.rs");
        app.editor_pane.set_regression_file_tabs(
            vec![planner, plan_md.clone(), renderer],
            1,
            app.file_tree.root(),
        );
        app.file_tree.set_selected_file(Some(plan_md));
        app
    }

    /// Fixture-backed app with a live Tokio runtime for chat flow tests. Caller must
    /// keep the returned receiver alive so the UI event channel stays open.
    pub fn new_for_flow_tests(
        fixture_root: std::path::PathBuf,
    ) -> (Self, std::sync::mpsc::Receiver<crate::quorp::tui::TuiEvent>) {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let handle = runtime.handle().clone();
        let (tx, rx) = std::sync::mpsc::sync_channel::<crate::quorp::tui::TuiEvent>(128);
        let app = Self::new_fixture_inner(tx, handle, fixture_root, Some(runtime), None, None);
        (app, rx)
    }

    /// Same as [`Self::new_for_flow_tests`], but chat uses production-style `provider/model` ids and
    /// exposes the backend request sender. Returns the receiver for tests that assert requests.
    #[cfg(test)]
    pub fn new_for_flow_tests_with_registry_chat(
        fixture_root: std::path::PathBuf,
        models: Vec<String>,
        model_index: usize,
    ) -> (
        Self,
        std::sync::mpsc::Receiver<crate::quorp::tui::TuiEvent>,
        futures::channel::mpsc::UnboundedReceiver<crate::quorp::tui::bridge::TuiToBackendRequest>,
    ) {
        let (bridge_tx, bridge_rx) = futures::channel::mpsc::unbounded();
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let handle = runtime.handle().clone();
        let (tx, rx) = std::sync::mpsc::sync_channel::<crate::quorp::tui::TuiEvent>(128);
        let app = Self::new_fixture_inner(
            tx,
            handle,
            fixture_root,
            Some(runtime),
            None,
            Some((bridge_tx, models, model_index)),
        );
        (app, rx, bridge_rx)
    }
}

#[cfg(test)]
impl TuiApp {
    pub fn assistant_feed_scroll_top_for_test(&self) -> usize {
        self.assistant_feed_scroll_top
    }

    pub fn assistant_feed_follow_latest_for_test(&self) -> bool {
        self.assistant_feed_follow_latest
    }

    /// Applies backend-driven events the same way as [`crate::quorp::tui::run`].
    pub fn apply_tui_backend_event(&mut self, event: crate::quorp::tui::TuiEvent) {
        use crate::quorp::tui::TuiEvent;
        match event {
            TuiEvent::Chat(ev) => self.handle_chat_ui_event(ev),
            TuiEvent::TerminalFrame(frame) => self.terminal.apply_integrated_frame(frame),
            TuiEvent::TerminalClosed => self.terminal.mark_integrated_session_closed(),
            TuiEvent::BootstrapTick => self.advance_bootstrap_tick(),
            TuiEvent::RuntimeHealthTick => self.poll_runtime_health(),
            TuiEvent::FileTreeListed(listing) => {
                self.file_tree
                    .apply_project_listing(listing.parent, listing.result);
            }
            TuiEvent::BufferSnapshot(snapshot) => {
                self.editor_pane.apply_editor_pane_buffer_snapshot(
                    snapshot.path,
                    snapshot.lines,
                    snapshot.error,
                    snapshot.truncated,
                )
            }
            TuiEvent::BackendResponse(response) => self.handle_backend_response(response),
            TuiEvent::PathIndexSnapshot(snapshot) => self.chat.apply_path_index_snapshot(
                snapshot.root,
                snapshot.entries,
                snapshot.files_seen,
            ),
            TuiEvent::AgentRuntime(event) => self.agent_pane.apply_event(event),
            TuiEvent::StartAgentTask(task) => {
                if let Some(tx) = &self.agent_runtime_tx {
                    let _ = tx.unbounded_send(
                        crate::quorp::tui::agent_runtime::AgentRuntimeCommand::StartTask(task),
                    );
                    self.focused = Pane::Chat;
                    self.agent_pane
                        .apply_status_update("Agent loop started.".to_string());
                }
            }
            TuiEvent::Crossterm(ev) => {
                let _ = self.handle_event(ev);
            }
            TuiEvent::RailEvent(event) => {
                self.proof_rail.apply_event(&event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyEventKind};
    use futures::StreamExt as _;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    fn spawn_ollama_models_server(
        status_line: &str,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind Ollama models server");
        let address = listener.local_addr().expect("local addr");
        let status_line = status_line.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0u8; 2048];
            let _ = stream.read(&mut buffer);
            let response = format!(
                "HTTP/1.1 {status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });
        (format!("http://{address}"), handle)
    }

    fn reserve_unused_local_port() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind unused port");
        let address = listener.local_addr().expect("local addr");
        drop(listener);
        format!("http://{address}")
    }

    fn set_ollama_env_for_test(
        host: &str,
    ) -> (
        std::sync::MutexGuard<'static, ()>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        let env_lock = crate::quorp::tui::ssd_moe_tui::test_env_lock();
        let original_provider = std::env::var("QUORP_PROVIDER").ok();
        let original_model = std::env::var("QUORP_MODEL").ok();
        let original_host = std::env::var("QUORP_OLLAMA_HOST").ok();
        unsafe {
            std::env::set_var("QUORP_PROVIDER", "ollama");
            std::env::set_var("QUORP_MODEL", "qwen2.5-coder:32b");
            std::env::set_var("QUORP_OLLAMA_HOST", host);
        }
        (env_lock, original_provider, original_model, original_host)
    }

    fn restore_env_var(key: &str, original: Option<String>) {
        match original {
            Some(value) => unsafe {
                std::env::set_var(key, value);
            },
            None => unsafe {
                std::env::remove_var(key);
            },
        }
    }

    #[test]
    fn draw_with_selected_rust_file_no_panic() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("sample.rs");
        std::fs::write(&file, "fn main() {}\n").expect("write");
        let mut app = TuiApp::new();
        app.file_tree = FileTree::with_root(dir.path().to_path_buf());
        app.file_tree.set_selected_file(Some(file));
        terminal.draw(|frame| app.draw(frame)).expect("draw");
    }

    #[test]
    fn editor_pane_down_scrolls_when_focused() {
        let mut app = TuiApp::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let mut long = String::new();
        for i in 0..50 {
            long.push_str(&format!("// line {i}\n"));
        }
        let file = dir.path().join("long.rs");
        std::fs::write(&file, long).expect("write");
        app.file_tree = FileTree::with_root(dir.path().to_path_buf());
        app.file_tree.set_selected_file(Some(file));
        app.focused = Pane::EditorPane;
        app.editor_pane
            .sync_from_selected_file(app.file_tree.selected_file(), app.file_tree.root());
        assert_eq!(app.editor_pane.vertical_scroll_for_test(), 0);
        let down = Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(app.handle_event(down).is_continue());
        assert!(app.editor_pane.vertical_scroll_for_test() > 0);
    }

    #[test]
    fn editor_pane_down_does_not_scroll_when_other_pane_focused() {
        let mut app = TuiApp::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("long.rs");
        std::fs::write(&file, "// x\n".repeat(50)).expect("write");
        app.file_tree = FileTree::with_root(dir.path().to_path_buf());
        app.file_tree.set_selected_file(Some(file));
        app.focused = Pane::Terminal;
        app.editor_pane
            .sync_from_selected_file(app.file_tree.selected_file(), app.file_tree.root());
        let down = Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(app.handle_event(down).is_continue());
        assert_eq!(app.editor_pane.vertical_scroll_for_test(), 0);
    }

    #[test]
    fn pane_next_prev_cycles() {
        let mut p = Pane::EditorPane;
        for _ in 0..4 {
            p = p.next();
        }
        assert_eq!(p, Pane::EditorPane);

        let mut p = Pane::EditorPane;
        for _ in 0..4 {
            p = p.prev();
        }
        assert_eq!(p, Pane::EditorPane);
    }

    #[test]
    fn tab_and_backtab_move_focus() {
        let mut app = TuiApp::new();
        assert_eq!(app.focused, Pane::Chat);

        let tab = Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(app.handle_event(tab).is_continue());
        assert_eq!(app.focused, Pane::FileTree);

        let back = Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert!(app.handle_event(back).is_continue());
        assert_eq!(app.focused, Pane::Chat);
    }

    #[test]
    fn esc_quits_from_every_pane() {
        for pane in [Pane::EditorPane, Pane::Chat, Pane::FileTree] {
            let mut app = TuiApp::new();
            app.focused = pane;
            let esc = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            assert!(app.handle_event(esc).is_break());
        }
    }

    #[test]
    fn esc_stays_in_terminal_capture_mode() {
        let mut app = TuiApp::new();
        app.focused = Pane::Terminal;
        app.terminal.enter_capture_mode();
        let esc = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.handle_event(esc).is_continue());
        assert_eq!(app.focused, Pane::Terminal);
        assert!(app.terminal.in_capture_mode());
    }

    #[test]
    fn ctrl_c_quits_only_from_non_terminal_panes() {
        let mut app = TuiApp::new();
        app.focused = Pane::EditorPane;
        let ctrl_c = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.handle_event(ctrl_c.clone()).is_break());

        app.focused = Pane::Terminal;
        assert!(app.handle_event(ctrl_c).is_continue());
    }

    #[test]
    fn key_release_ignored() {
        let mut app = TuiApp::new();
        let ev = Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));
        assert!(app.handle_event(ev).is_continue());
    }

    #[test]
    fn q_does_not_quit_when_terminal_focused() {
        let mut app = TuiApp::new();
        app.focused = Pane::Terminal;
        let q = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.handle_event(q).is_continue());
    }

    #[test]
    fn shift_tab_moves_back() {
        let mut app = TuiApp::new();
        let ev = Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert!(app.handle_event(ev).is_continue());
        assert_eq!(app.focused, Pane::Terminal);
    }

    #[test]
    fn resize_is_noop() {
        let mut app = TuiApp::new();
        assert!(app.handle_event(Event::Resize(100, 40)).is_continue());
    }

    #[test]
    fn tab_switches_pane_when_file_tree_focused() {
        let mut app = TuiApp::new();
        app.focused = Pane::FileTree;
        let tab = Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(app.handle_event(tab).is_continue());
        assert_eq!(app.focused, Pane::EditorPane);
    }

    #[test]
    fn chat_bracket_cycles_model_without_changing_focus() {
        let mut app = TuiApp::new();
        app.focused = Pane::Chat;
        let before = app.chat.model_index_for_test();
        let ev = Event::Key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
        assert!(app.handle_event(ev).is_continue());
        assert_ne!(app.chat.model_index_for_test(), before);
        assert_eq!(app.focused, Pane::Chat);
    }

    #[test]
    fn tab_still_moves_focus_when_chat_focused() {
        let mut app = TuiApp::new();
        app.focused = Pane::Chat;
        let tab = Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(app.handle_event(tab).is_continue());
        assert_eq!(app.focused, Pane::FileTree);
    }

    #[test]
    fn handle_event_many_keys_no_panic_terminal_focus_unchanged() {
        let mut app = TuiApp::new();
        app.focused = Pane::Terminal;
        let key = Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        for _ in 0..1000 {
            assert!(app.handle_event(key.clone()).is_continue());
        }
        assert_eq!(app.focused, Pane::Terminal);
    }

    #[test]
    fn vi_navigation_escapes_terminal_pane() {
        let mut app = TuiApp::new();
        app.focused = Pane::Terminal;
        app.terminal.enter_capture_mode();
        let ctrl_g = Event::Key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert!(app.handle_event(ctrl_g).is_continue());
        let ctrl_l = Event::Key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
        assert!(app.handle_event(ctrl_l).is_continue());
        assert_eq!(app.focused, Pane::Chat);
    }

    #[test]
    fn ctrl_h_navigates_from_editor_pane_to_file_tree() {
        let mut app = TuiApp::new();
        app.focused = Pane::EditorPane;
        let ctrl_h = Event::Key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
        assert!(app.handle_event(ctrl_h).is_continue());
        assert_eq!(app.focused, Pane::FileTree);
    }

    #[test]
    fn vi_navigation_ctrl_j_moves_down_left_column() {
        let mut app = TuiApp::new();

        // EditorPane -> Terminal
        app.focused = Pane::EditorPane;
        app.last_left_pane = Pane::EditorPane;
        let ctrl_j = Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert!(app.handle_event(ctrl_j.clone()).is_continue());
        assert_eq!(app.focused, Pane::Terminal);

        // Chat stays in assistant; there is no separate agent pane in the default layout.
        app.focused = Pane::Chat;
        assert!(app.handle_event(ctrl_j).is_continue());
        assert_eq!(app.focused, Pane::Chat);
    }

    #[test]
    fn status_bar_updates_reflect_focus_model_and_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path_fragment = dir.path().to_string_lossy();
        let mut app = TuiApp::new();
        app.file_tree = FileTree::with_root(dir.path().to_path_buf());
        app.focused = Pane::EditorPane;
        app.last_left_pane = Pane::EditorPane;
        let s = app.status_bar_text();
        assert!(s.contains("Mode: Preview"), "{s}");
        assert!(
            s.contains("Model:") && s.contains(&app.chat.current_model_display_label()),
            "{s}"
        );
        assert!(
            s.contains("Path:") && s.contains(path_fragment.as_ref()),
            "{s}"
        );

        app.focused = Pane::Terminal;
        app.last_left_pane = Pane::Terminal;
        assert!(app.status_bar_text().contains("Mode: Terminal"));

        app.focused = Pane::Chat;
        app.last_left_pane = Pane::Chat;
        assert!(app.status_bar_text().contains("Mode: Assistant"));

        app.focused = Pane::FileTree;
        assert!(app.status_bar_text().contains("Mode: Files"));
    }

    #[test]
    fn mouse_click_focuses_panes() {
        let mut app = TuiApp::new();
        app.terminal_dock_open = true;
        app.explorer_collapsed = false;
        let backend = ratatui::backend::TestBackend::new(232, 64);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let full = Rect::new(0, 0, 232, 64);
        let state = app.shell_state_snapshot(full);
        let geometry = ShellGeometry::for_state(full, &state);
        let explorer = geometry.explorer.expect("explorer");
        let dock = geometry.dock.expect("dock");
        let center = geometry.center;

        app.focused = Pane::Chat;
        app.handle_mouse_click(explorer.x + 2, explorer.y + 2);
        assert_eq!(app.focused, Pane::FileTree);

        app.handle_mouse_click(center.x + 2, center.y + 2);
        assert_eq!(app.focused, Pane::Chat);

        app.handle_mouse_click(dock.x + 2, dock.y + 2);
        assert_eq!(app.focused, Pane::Terminal);
    }

    #[test]
    fn mouse_click_dismisses_help() {
        let mut app = TuiApp::new();
        let backend = ratatui::backend::TestBackend::new(232, 64);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();

        app.open_help_overlay();
        app.handle_mouse_click(5, 5);
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn ctrl_j_escapes_terminal_pane() {
        let mut app = TuiApp::new();
        app.focused = Pane::Terminal;
        app.last_left_pane = Pane::Terminal;
        app.terminal.enter_capture_mode();
        let ctrl_g = Event::Key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert!(app.handle_event(ctrl_g).is_continue());
        let ctrl_j = Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert!(app.handle_event(ctrl_j).is_continue());
        assert_eq!(app.focused, Pane::Chat);
    }

    #[test]
    fn ctrl_k_escapes_terminal_pane() {
        let mut app = TuiApp::new();
        app.focused = Pane::Terminal;
        app.last_left_pane = Pane::Terminal;
        app.terminal.enter_capture_mode();
        let ctrl_g = Event::Key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert!(app.handle_event(ctrl_g).is_continue());
        let ctrl_k = Event::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert!(app.handle_event(ctrl_k).is_continue());
        assert_eq!(app.focused, Pane::Terminal);
        assert_eq!(app.overlay, Overlay::ActionDeck);
    }

    #[test]
    fn ctrl_l_from_file_tree_returns_to_last_left() {
        let mut app = TuiApp::new();
        app.focused = Pane::FileTree;
        app.last_left_pane = Pane::Chat;
        let ctrl_l = Event::Key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
        assert!(app.handle_event(ctrl_l).is_continue());
        assert_eq!(app.focused, Pane::Chat);
    }

    #[test]
    fn help_toggle_from_terminal() {
        let mut app = TuiApp::new();
        app.focused = Pane::Terminal;
        let q = Event::Key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(app.handle_event(q).is_continue());
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(app.focused, Pane::Terminal);
    }

    #[test]
    fn help_toggle_from_terminal_navigation_mode() {
        let mut app = TuiApp::new();
        app.focused = Pane::Terminal;
        app.terminal.enter_navigation_mode();
        let q = Event::Key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(app.handle_event(q).is_continue());
        assert_eq!(app.overlay, Overlay::Help);
    }

    #[test]
    fn help_toggle_from_chat() {
        let mut app = TuiApp::new();
        app.focused = Pane::Chat;
        let q = Event::Key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(app.handle_event(q).is_continue());
        assert_eq!(app.overlay, Overlay::Help);
    }

    #[test]
    fn help_overlay_toggles_with_question_mark() {
        let mut app = TuiApp::new();
        app.focused = Pane::Chat;
        let q = Event::Key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(app.handle_event(q.clone()).is_continue());
        assert_eq!(app.overlay, Overlay::Help);

        assert!(app.handle_event(q).is_continue());
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn shell_state_snapshot_uses_model_picker_overlay() {
        let mut app = TuiApp::new();
        app.focused = Pane::Chat;
        app.overlay = Overlay::ModelPicker;

        let shell = app.shell_state_snapshot(Rect::new(0, 0, 120, 40));

        assert_eq!(shell.focus, ShellFocus::Overlay);
        assert_eq!(
            shell.overlay.as_ref().map(|overlay| overlay.title.as_str()),
            Some("Model Selector")
        );
        assert!(!shell.assistant_overlay);
    }

    #[test]
    fn shell_state_snapshot_maps_live_chat_transcript() {
        let mut app = TuiApp::new();
        app.focused = Pane::Chat;
        app.chat.seed_messages_for_test(vec![
            crate::quorp::tui::chat::ChatMessage::User("Explain startup flow".to_string()),
            crate::quorp::tui::chat::ChatMessage::Assistant(
                "Quorp starts the runtime and draws the shell.".to_string(),
            ),
        ]);
        app.chat.set_streaming_for_test(true);

        let shell = app.shell_state_snapshot(Rect::new(0, 0, 120, 40));

        assert!(shell.assistant.session_label.starts_with("Assistant "));
        assert_eq!(shell.assistant.composer_text, "Streaming response...");
        assert!(
            shell.assistant.transcript.iter().any(|block| {
                block.role == "User:" && block.text.contains("Explain startup flow")
            }),
            "{:?}",
            shell.assistant.transcript
        );
        assert!(
            shell.assistant.transcript.iter().any(|block| {
                block.role == "Assistant:" && block.text.contains("Quorp starts the runtime")
            }),
            "{:?}",
            shell.assistant.transcript
        );
    }

    #[test]
    fn compact_shell_snapshot_surfaces_focused_explorer_as_overlay() {
        let mut app = TuiApp::new();
        app.focused = Pane::FileTree;

        let shell = app.shell_state_snapshot(Rect::new(0, 0, 100, 32));

        assert_eq!(shell.focus, ShellFocus::Overlay);
        assert_eq!(
            shell.overlay.as_ref().map(|overlay| overlay.title.as_str()),
            Some("Files")
        );
        assert!(
            shell
                .overlay
                .as_ref()
                .is_some_and(|overlay| !overlay.lines.is_empty())
        );
    }

    #[test]
    fn bootstrap_blocks_ready_scene_when_runtime_failed() {
        let mut app = TuiApp::new();
        app.has_completed_bootstrap.set(false);
        app.bootstrap.visible_started_at = Some(Instant::now() - BOOTSTRAP_MIN_DURATION);
        app.bootstrap.frame_index = BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1);
        app.ssd_moe
            .set_status_for_test(crate::quorp::tui::ssd_moe_tui::ModelStatus::Failed(
                "weights missing".to_string(),
            ));

        let shell = app.shell_state_snapshot(Rect::new(0, 0, 120, 40));

        assert_eq!(shell.scene, ShellScene::Bootstrap);
        assert!(shell.bootstrap.is_some());
        assert!(
            shell
                .bootstrap
                .as_ref()
                .is_some_and(|bootstrap| bootstrap.footer.contains("Startup blocked"))
        );
    }

    #[test]
    fn probe_ollama_runtime_uses_host_and_model_normalization() {
        let (host, handle) =
            spawn_ollama_models_server("200 OK", r#"{"data":[{"id":"qwen2.5-coder:32b"}]}"#);
        let (_env_lock, original_provider, original_model, original_host) =
            set_ollama_env_for_test(&host);

        let raw_model =
            crate::quorp::tui::model_registry::chat_model_raw_id("ollama/qwen2.5-coder:32b");
        assert_eq!(raw_model, "qwen2.5-coder:32b");
        let detail = TuiApp::probe_ollama_runtime(raw_model).expect("successful probe");
        assert!(detail.contains("endpoint ready at"));
        assert!(detail.contains("model qwen2.5-coder:32b available"));

        handle.join().expect("join server");
        restore_env_var("QUORP_PROVIDER", original_provider);
        restore_env_var("QUORP_MODEL", original_model);
        restore_env_var("QUORP_OLLAMA_HOST", original_host);
    }

    #[test]
    fn probe_ollama_runtime_reports_unreachable_host() {
        let unused_host = reserve_unused_local_port();
        let (_env_lock, original_provider, original_model, original_host) =
            set_ollama_env_for_test(&unused_host);

        let error =
            TuiApp::probe_ollama_runtime("qwen2.5-coder:32b").expect_err("unreachable host");
        assert!(error.contains("Ollama unreachable"));

        restore_env_var("QUORP_PROVIDER", original_provider);
        restore_env_var("QUORP_MODEL", original_model);
        restore_env_var("QUORP_OLLAMA_HOST", original_host);
    }

    #[test]
    fn bootstrap_stays_visible_before_minimum_duration_even_when_runtime_is_ready() {
        let mut app = TuiApp::new();
        app.has_completed_bootstrap.set(false);
        app.bootstrap.visible_started_at = Some(Instant::now() + BOOTSTRAP_MIN_DURATION);
        app.bootstrap.frame_index = BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1);
        app.bootstrap.terminal_probe_ok = true;
        app.bootstrap.workspace_probe_ok = true;
        app.ssd_moe
            .set_status_for_test(crate::quorp::tui::ssd_moe_tui::ModelStatus::Running);

        let shell = app.shell_state_snapshot(Rect::new(0, 0, 120, 40));

        assert_eq!(shell.scene, ShellScene::Bootstrap);
    }

    #[test]
    fn bootstrap_timer_waits_for_first_visible_frame() {
        let mut app = TuiApp::new();
        app.has_completed_bootstrap.set(false);
        app.bootstrap.started_at = Instant::now() - Duration::from_secs(5);
        app.bootstrap.visible_started_at = None;
        app.bootstrap.frame_index = BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1);
        app.bootstrap.terminal_probe_ok = true;
        app.bootstrap.workspace_probe_ok = true;
        app.ssd_moe
            .set_status_for_test(crate::quorp::tui::ssd_moe_tui::ModelStatus::Running);

        let shell = app.shell_state_snapshot(Rect::new(0, 0, 120, 40));

        assert_eq!(shell.scene, ShellScene::Bootstrap);
    }

    #[test]
    fn bootstrap_stays_visible_after_minimum_duration_until_runtime_is_ready() {
        let mut app = TuiApp::new();
        app.has_completed_bootstrap.set(false);
        app.bootstrap.visible_started_at = Some(Instant::now() - BOOTSTRAP_MIN_DURATION);
        app.bootstrap.frame_index = BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1);
        app.bootstrap.terminal_probe_ok = true;
        app.bootstrap.workspace_probe_ok = true;
        app.ssd_moe
            .set_status_for_test(crate::quorp::tui::ssd_moe_tui::ModelStatus::Starting);

        let shell = app.shell_state_snapshot(Rect::new(0, 0, 120, 40));

        assert_eq!(shell.scene, ShellScene::Bootstrap);
    }

    #[test]
    fn bootstrap_transitions_to_ready_when_hard_gates_and_time_pass() {
        let mut app = TuiApp::new();
        app.has_completed_bootstrap.set(false);
        app.bootstrap.visible_started_at = Some(Instant::now() - BOOTSTRAP_MIN_DURATION);
        app.bootstrap.frame_index = BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1);
        app.bootstrap.terminal_probe_ok = true;
        app.bootstrap.workspace_probe_ok = true;
        app.ssd_moe
            .set_status_for_test(crate::quorp::tui::ssd_moe_tui::ModelStatus::Running);

        let shell = app.shell_state_snapshot(Rect::new(0, 0, 120, 40));

        assert_eq!(shell.scene, ShellScene::Ready);
    }

    #[test]
    fn bootstrap_ollama_ready_does_not_require_ssd_moe_running() {
        let fixture_root = tempfile::tempdir().expect("tempdir");
        let (mut app, _rx, _bridge_rx) = TuiApp::new_for_flow_tests_with_registry_chat(
            fixture_root.path().to_path_buf(),
            vec!["ollama/qwen2.5-coder:32b".to_string()],
            0,
        );
        app.has_completed_bootstrap.set(false);
        app.bootstrap.visible_started_at = Some(Instant::now() - BOOTSTRAP_MIN_DURATION);
        app.bootstrap.frame_index = BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1);
        app.bootstrap.terminal_probe_ok = true;
        app.bootstrap.workspace_probe_ok = true;
        app.bootstrap.remote_runtime_probe = Some(BootstrapRemoteRuntimeProbe::Ready(
            "endpoint ready at http://127.0.0.1:11434 · model qwen2.5-coder:32b available"
                .to_string(),
        ));
        app.ssd_moe
            .set_status_for_test(crate::quorp::tui::ssd_moe_tui::ModelStatus::Ready);

        let shell = app.shell_state_snapshot(Rect::new(0, 0, 120, 40));

        assert_eq!(shell.scene, ShellScene::Ready);
    }

    #[test]
    fn bootstrap_ollama_failure_surfaces_clear_message() {
        let fixture_root = tempfile::tempdir().expect("tempdir");
        let (mut app, _rx, _bridge_rx) = TuiApp::new_for_flow_tests_with_registry_chat(
            fixture_root.path().to_path_buf(),
            vec!["ollama/qwen2.5-coder:32b".to_string()],
            0,
        );
        app.has_completed_bootstrap.set(false);
        app.bootstrap.visible_started_at = Some(Instant::now() - BOOTSTRAP_MIN_DURATION);
        app.bootstrap.frame_index = BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1);
        app.bootstrap.terminal_probe_ok = true;
        app.bootstrap.workspace_probe_ok = true;
        app.bootstrap.remote_runtime_probe = Some(BootstrapRemoteRuntimeProbe::Failed(
            "Ollama unreachable at http://127.0.0.1:11434".to_string(),
        ));
        app.ssd_moe
            .set_status_for_test(crate::quorp::tui::ssd_moe_tui::ModelStatus::Ready);

        let shell = app.shell_state_snapshot(Rect::new(0, 0, 120, 40));
        let bootstrap = shell.bootstrap.expect("bootstrap snapshot");
        let runtime_probe = bootstrap
            .probes
            .iter()
            .find(|probe| probe.label == "Ollama")
            .expect("ollama probe");

        assert_eq!(shell.scene, ShellScene::Bootstrap);
        assert!(runtime_probe.detail.contains("Ollama unreachable"));
        assert!(bootstrap.footer.contains("Startup blocked"));
    }

    #[test]
    fn bootstrap_ollama_missing_model_surfaces_clear_message() {
        let fixture_root = tempfile::tempdir().expect("tempdir");
        let (mut app, _rx, _bridge_rx) = TuiApp::new_for_flow_tests_with_registry_chat(
            fixture_root.path().to_path_buf(),
            vec!["ollama/qwen2.5-coder:32b".to_string()],
            0,
        );
        app.has_completed_bootstrap.set(false);
        app.bootstrap.visible_started_at = Some(Instant::now() - BOOTSTRAP_MIN_DURATION);
        app.bootstrap.frame_index = BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1);
        app.bootstrap.terminal_probe_ok = true;
        app.bootstrap.workspace_probe_ok = true;
        app.bootstrap.remote_runtime_probe = Some(BootstrapRemoteRuntimeProbe::Failed(
            "model qwen2.5-coder:32b is not available at http://127.0.0.1:11434 (found: llama3.1:8b)"
                .to_string(),
        ));
        app.ssd_moe
            .set_status_for_test(crate::quorp::tui::ssd_moe_tui::ModelStatus::Ready);

        let shell = app.shell_state_snapshot(Rect::new(0, 0, 120, 40));
        let bootstrap = shell.bootstrap.expect("bootstrap snapshot");
        let runtime_probe = bootstrap
            .probes
            .iter()
            .find(|probe| probe.label == "Ollama")
            .expect("ollama probe");

        assert_eq!(shell.scene, ShellScene::Bootstrap);
        assert!(runtime_probe.detail.contains("is not available"));
        assert!(runtime_probe.detail.contains("qwen2.5-coder:32b"));
    }

    #[test]
    fn once_ready_runtime_drop_stays_in_main_shell() {
        let mut app = TuiApp::new();
        app.has_completed_bootstrap.set(false);
        app.bootstrap.visible_started_at = Some(Instant::now() - BOOTSTRAP_MIN_DURATION);
        app.bootstrap.frame_index = BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1);
        app.bootstrap.terminal_probe_ok = true;
        app.bootstrap.workspace_probe_ok = true;
        app.ssd_moe
            .set_status_for_test(crate::quorp::tui::ssd_moe_tui::ModelStatus::Running);

        let ready_shell = app.shell_state_snapshot(Rect::new(0, 0, 120, 40));
        assert_eq!(ready_shell.scene, ShellScene::Ready);

        app.ssd_moe
            .set_status_for_test(crate::quorp::tui::ssd_moe_tui::ModelStatus::Ready);
        let degraded_shell = app.shell_state_snapshot(Rect::new(0, 0, 120, 40));

        assert_eq!(degraded_shell.scene, ShellScene::Ready);
        assert!(
            degraded_shell
                .center
                .feed
                .iter()
                .any(|item| item.title == "Runtime degraded"),
            "{:?}",
            degraded_shell.center.feed
        );
        assert!(
            degraded_shell
                .status_hint
                .contains("Runtime lost after startup")
        );
    }

    #[test]
    fn apply_tui_backend_event_handles_agent_runtime_event() {
        let mut app = TuiApp::new();
        app.apply_tui_backend_event(crate::quorp::tui::TuiEvent::AgentRuntime(
            crate::quorp::tui::agent_runtime::AgentUiEvent::StatusUpdate(
                crate::quorp::tui::agent_runtime::AgentRuntimeStatus::Thinking,
            ),
        ));
        assert!(
            app.agent_pane
                .status_lines
                .iter()
                .any(|line| line.contains("[Thinking]"))
        );
    }

    #[test]
    fn apply_tui_backend_event_routes_start_agent_task_into_runtime() {
        let mut app = TuiApp::new();
        let (agent_tx, mut agent_rx) = futures::channel::mpsc::unbounded();
        app.agent_runtime_tx = Some(agent_tx);

        app.apply_tui_backend_event(crate::quorp::tui::TuiEvent::StartAgentTask(
            crate::quorp::tui::agent_runtime::AgentTaskRequest {
                goal: "fix README".to_string(),
                initial_context: Vec::new(),
                model_id: "qwen35-35b-a3b".to_string(),
                agent_mode: crate::quorp::tui::agent_protocol::AgentMode::Act,
                base_url_override: None,
                workspace_root: std::env::temp_dir().join("quorp-agent-test-workspace"),
                target_path: std::env::temp_dir().join("quorp-agent-test-workspace"),
                command_kind: crate::quorp::tui::slash_commands::SlashCommandKind::FullAuto,
                resolved_mode:
                    crate::quorp::tui::slash_commands::FullAutoResolvedMode::WorkspaceObjective,
                sandbox_mode: crate::quorp::tui::slash_commands::FullAutoSandboxMode::LocalCopy,
                docker_image: None,
                max_iterations: 3,
                max_seconds: None,
                max_total_tokens: None,
                autonomy_profile: crate::quorp::tui::agent_context::AutonomyProfile::AutonomousHost,
                result_dir: std::env::temp_dir().join("quorp-agent-test"),
                objective_file: None,
                evaluate_command: None,
                objective_metadata: serde_json::Value::Null,
            },
        ));

        let next = futures::executor::block_on(agent_rx.next()).expect("runtime command");
        match next {
            crate::quorp::tui::agent_runtime::AgentRuntimeCommand::StartTask(task) => {
                assert_eq!(task.goal, "fix README");
                assert_eq!(app.focused, Pane::Chat);
            }
            other => panic!("unexpected runtime command: {other:?}"),
        }
    }

    #[test]
    fn runtime_session_command_finished_is_routed_to_agent_runtime() {
        let mut app = TuiApp::new();
        let (agent_tx, mut agent_rx) = futures::channel::mpsc::unbounded();
        app.agent_runtime_tx = Some(agent_tx);

        app.handle_chat_ui_event(crate::quorp::tui::chat::ChatUiEvent::CommandFinished(
            crate::quorp::tui::agent_runtime::AGENT_RUNTIME_SESSION_ID,
            crate::quorp::tui::agent_protocol::ActionOutcome::Success {
                action: crate::quorp::tui::agent_protocol::AgentAction::SearchText {
                    query: "agent".to_string(),
                    limit: 3,
                },
                output: "ok".to_string(),
            },
        ));

        let next = futures::executor::block_on(agent_rx.next()).expect("runtime command");
        assert!(matches!(
            next,
            crate::quorp::tui::agent_runtime::AgentRuntimeCommand::ToolFinished(
                crate::quorp::tui::agent_protocol::ActionOutcome::Success { .. }
            )
        ));
    }
}

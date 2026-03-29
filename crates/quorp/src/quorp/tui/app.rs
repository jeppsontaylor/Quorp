#![allow(unused)]
use std::ops::ControlFlow;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::quorp::tui::chat::ChatPane;

use crate::quorp::tui::editor_pane::EditorPane;
use crate::quorp::tui::file_tree::{FileTree, FileTreeKeyOutcome};
use crate::quorp::tui::ssd_moe_tui::SsdMoeManager;

use crate::quorp::tui::models_pane::ModelsPane;
use crate::quorp::tui::terminal_pane::TerminalPane;
use crate::quorp::tui::agent_pane::AgentPane;
use crate::quorp::tui::theme::Theme;

use crate::quorp::tui::workbench::{LeafId, WorkspaceNode};
use crate::quorp::tui::hitmap::{HitMap, HitTarget};

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
            EditorPane => "Code",
            Terminal => "Terminal",
            Chat => "Chat",
            Agent => "Agent",
            FileTree => "Files",
            _ => "Unknown",
        }
    }

    pub fn next(pane: LeafId) -> LeafId {
        match pane {
            EditorPane => Terminal,
            Terminal => Chat,
            Chat => Agent,
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
            FileTree => Agent,
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
    pub app_state: Option<std::sync::Arc<crate::AppState>>,
    pub unified_bridge_tx: Option<futures::channel::mpsc::UnboundedSender<crate::quorp::tui::bridge::TuiToBackendRequest>>,

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
    /// Incremented each full draw; drives indexing spinner in the status bar.
    draw_frame_seq: u64,
}

impl TuiApp {
    pub fn app_state(&self) -> Option<&std::sync::Arc<crate::AppState>> {
        self.app_state.as_ref()
    }

    pub fn new() -> Self {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let handle = runtime.handle().clone();
        let (tx, rx) = std::sync::mpsc::sync_channel::<crate::quorp::tui::TuiEvent>(128);
        let file_tree = FileTree::new();
        let project_root = file_tree.root().to_path_buf();
        let path_index = std::sync::Arc::new(crate::quorp::tui::path_index::PathIndex::new(
            project_root.clone(),
        ));
        let mut ssd_moe = SsdMoeManager::new();
        let default_model = crate::quorp::tui::model_registry::get_saved_model();
        ssd_moe.ensure_running(&project_root, &default_model);
        let theme = Theme::antigravity();
        let chat = ChatPane::new(tx, handle, project_root, path_index, None, None);
        let models_pane = ModelsPane::sync_from_chat(&chat);
        Self {
            focused: Pane::EditorPane,
            right_pane: Pane::Chat,
            last_left_pane: Pane::EditorPane,
            file_tree,
            editor_pane: EditorPane::new(),
            terminal: TerminalPane::new(),
            agent_pane: AgentPane::new(None),
            chat,
            models_pane,
            ssd_moe,
            _runtime: Some(runtime),
            _event_rx_keepalive: Some(rx),
            overlay: Overlay::None,
            app_state: None,
            unified_bridge_tx: None,
            last_full_area: Rect::default(),
            theme,
            hitmap: HitMap::new(),
            workspace: crate::quorp::tui::workbench::default_antigravity_tree(),
            visual_status_center_override: None,
            visual_title_override: None,
            visual_status_left_override: None,
            visual_status_right_override: None,
            prismforge_dynamic_layout: false,
            prismforge_split_ratio_lock: None,
            splitter_visual_state: SplitterVisualState::Idle,
            tab_strip_focus: None,
            draw_frame_seq: 0,
        }
    }

    pub(crate) fn new_with_chat_sender(
        tx: std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
        handle: tokio::runtime::Handle,
    ) -> Self {
        let file_tree = FileTree::new();
        let project_root = file_tree.root().to_path_buf();
        let path_index = std::sync::Arc::new(crate::quorp::tui::path_index::PathIndex::new(
            project_root.clone(),
        ));

        let mut ssd_moe = SsdMoeManager::new();
        let default_model = crate::quorp::tui::model_registry::get_saved_model();
        ssd_moe.ensure_running(&project_root, &default_model);
        let theme = Theme::antigravity();
        let chat = ChatPane::new(tx, handle, project_root, path_index, None, None);
        let models_pane = ModelsPane::sync_from_chat(&chat);
        Self {
            focused: Pane::EditorPane,
            right_pane: Pane::Chat,
            last_left_pane: Pane::EditorPane,
            file_tree,
            editor_pane: EditorPane::new(),
            terminal: TerminalPane::new(),
            agent_pane: AgentPane::new(None),
            chat,
            models_pane,
            ssd_moe,
            _runtime: None,
            _event_rx_keepalive: None,
            overlay: Overlay::None,
            app_state: None,
            unified_bridge_tx: None,
            last_full_area: Rect::default(),
            theme,
            hitmap: HitMap::new(),
            workspace: crate::quorp::tui::workbench::default_antigravity_tree(),
            visual_status_center_override: None,
            visual_title_override: None,
            visual_status_left_override: None,
            visual_status_right_override: None,
            prismforge_dynamic_layout: false,
            prismforge_split_ratio_lock: None,
            splitter_visual_state: SplitterVisualState::Idle,
            tab_strip_focus: None,
            draw_frame_seq: 0,
        }
    }

    pub(crate) fn new_with_backend(
        app_state: std::sync::Arc<workspace::AppState>,
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
        let mut file_tree = FileTree::with_root(workspace_root);
        if let Some(sender) = unified_bridge_tx.clone() {
            file_tree.set_project_list_sender(sender);
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
        let default_model = crate::quorp::tui::model_registry::get_saved_model();
        ssd_moe.ensure_running(&project_root, &default_model);
        let theme = Theme::prism_forge();
        let terminal = match &unified_language_model {
            Some((tx, _, _)) => TerminalPane::with_bridge(Some(tx.clone())),
            None => TerminalPane::new(),
        };
        let chat_uses_language_model_registry = unified_language_model.is_some();
        let mut chat = ChatPane::new(
            tx,
            handle,
            project_root,
            path_index,
            unified_language_model,
            command_bridge_tx,
        );
        // `active_model.txt` stores local SSD-MOE weight ids for `SsdMoeManager`, not `provider/model`
        // lines from [`language_model::LanguageModelRegistry`]. Do not apply it to chat when integrated.
        if !chat_uses_language_model_registry {
            if let Some(saved) = crate::quorp::tui::model_registry::get_saved_model_id_raw() {
                if let Some(i) = chat
                    .model_list()
                    .iter()
                    .position(|m| m.as_str() == saved.as_str())
                {
                    chat.set_model_index(i);
                }
            }
        }
        let models_pane = ModelsPane::sync_from_chat(&chat);
        Self {
            focused: Pane::EditorPane,
            right_pane: Pane::Chat,
            last_left_pane: Pane::EditorPane,
            file_tree,
            editor_pane: EditorPane::with_buffer_bridge(unified_bridge_tx.clone()),
            terminal: TerminalPane::with_bridge(unified_bridge_tx.clone()),
            agent_pane: AgentPane::new(unified_bridge_tx.clone()),
            chat,
            models_pane,
            ssd_moe,
            _runtime: None,
            _event_rx_keepalive: None,
            overlay: Overlay::None,
            app_state: Some(app_state),
            unified_bridge_tx,
            last_full_area: Rect::default(),
            theme,
            hitmap: HitMap::new(),
            workspace: crate::quorp::tui::workbench::default_prismforge_tree(),
            prismforge_dynamic_layout: true,
            visual_status_center_override: None,
            visual_title_override: None,
            visual_status_left_override: None,
            visual_status_right_override: None,

            prismforge_split_ratio_lock: None,
            splitter_visual_state: SplitterVisualState::Idle,
            tab_strip_focus: None,
            draw_frame_seq: 0,
        }
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

    fn set_focus(&mut self, pane: PaneType) {
        if self.tab_strip_focus.is_some_and(|leaf| leaf != pane) {
            self.tab_strip_focus = None;
        }
        self.focused = pane;
        if matches!(
            pane,
            Pane::EditorPane | Pane::Terminal | Pane::Chat
        ) {
            self.last_left_pane = pane;
        }
    }

    /// Full status line for tests and layout; draw applies [`truncate_fit`] to the status row width.
    pub fn status_bar_text(&self) -> String {
        let mode = self.focused.display_label();
        let model = self.chat.current_model_id();
        let path = self.status_center_for_status_bar();
        let help_hint = if self.overlay == Overlay::Help {
            "Press ? or Esc to close help"
        } else {
            "Press ? for help"
        };
        format!("Mode: {mode} | Model: {model} | Path: {path} | {help_hint}")
    }

    pub fn terminal_pane_content_size(&mut self, full: Rect) -> Option<(u16, u16)> {
        let metrics = self.theme.metrics.clone();
        let shell = crate::quorp::tui::workbench::compute_shell(full, &metrics);
        self.sync_prismforge_workspace(shell.workspace, &metrics);
        let layout = crate::quorp::tui::workbench::compute_workbench(shell.workspace, &self.workspace, &metrics);
        if let Some(rects) = layout.leaves.get(&Pane::Terminal) {
            let width = rects.body.width;
            let height = rects.body.height;
            if width > 1 && height > 1 {
                return Some((width, height));
            }
        }
        None
    }

    fn navigate_left(&mut self) {
        if matches!(
            self.focused,
            Pane::EditorPane | Pane::Terminal | Pane::Chat
        ) {
            self.last_left_pane = self.focused;
            self.set_focus(Pane::FileTree);
        }
    }

    fn navigate_right(&mut self) {
        if self.focused == Pane::FileTree {
            self.set_focus(self.last_left_pane);
        } else if self.focused == Pane::EditorPane || self.focused == Pane::Terminal || self.focused == Pane::Agent {
            self.set_focus(self.right_pane);
        }
    }

    fn navigate_down(&mut self) {
        if self.focused == Pane::EditorPane {
            self.set_focus(Pane::Terminal);
        } else if self.focused == Pane::Terminal {
            self.set_focus(Pane::Chat);
        } else if self.focused == Pane::Chat {
            self.set_focus(Pane::Agent);
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
    pub fn workbench_layout_snapshot(&mut self, full: Rect) -> crate::quorp::tui::workbench::WorkbenchLayout {
        let metrics = self.theme.metrics.clone();
        let shell = crate::quorp::tui::workbench::compute_shell(full, &metrics);
        self.sync_prismforge_workspace(shell.workspace, &metrics);
        crate::quorp::tui::workbench::compute_workbench(shell.workspace, &self.workspace, &metrics)
    }

    fn sync_prismforge_workspace(&mut self, workspace_rect: ratatui::layout::Rect, metrics: &crate::quorp::tui::theme::Metrics) {
        if !self.prismforge_dynamic_layout {
            return;
        }
        let fresh =
            crate::quorp::tui::workbench::prismforge_tree_for_workspace(workspace_rect, metrics);
        let (fv, fh) = crate::quorp::tui::workbench::prismforge_ratios_from_tree(&fresh);
        let (v, h) = self
            .prismforge_split_ratio_lock
            .unwrap_or((fv, fh));
        self.workspace = crate::quorp::tui::workbench::prismforge_tree_with_ratios(v, h, 1);
    }

    fn context_hints_for_focused_pane(&self) -> String {
        match self.focused {
            Pane::FileTree => "↑↓ Navigate  Enter Open  ←→ Expand/Collapse  Ctrl+→ Code".to_string(),
            Pane::EditorPane => "↑↓ Scroll  Ctrl+←→↑↓ Navigate  Tab Next  ? Help".to_string(),
            Pane::Terminal => "Shell input active  Ctrl+←→↑↓ Navigate  ? Help".to_string(),
            Pane::Chat => "Enter Send  Ctrl+T New  [ ] Model  Ctrl+←→↑↓ Navigate".to_string(),
            Pane::Agent => "Enter Dispatch  Ctrl+↑ Chat  Ctrl+←→↑↓ Navigate".to_string(),
            _ => "Ctrl+←→↑↓ Navigate  ? Help  Esc Quit".to_string(),
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

    fn register_splitter_hit_targets(&mut self, layout: &crate::quorp::tui::workbench::WorkbenchLayout) {
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
        let metrics = self.theme.metrics.clone();
        let shell = crate::quorp::tui::workbench::compute_shell(full, &metrics);
        self.sync_prismforge_workspace(shell.workspace, &metrics);
        let layout = crate::quorp::tui::workbench::compute_workbench(shell.workspace, &self.workspace, &metrics);
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
        let metrics = self.theme.metrics.clone();
        let shell = crate::quorp::tui::workbench::compute_shell(full, &metrics);
        self.sync_prismforge_workspace(shell.workspace, &metrics);
        let Some((parent, axis, divider)) = crate::quorp::tui::workbench::split_parent_rect_for_index(
            shell.workspace,
            &self.workspace,
            splitter_index,
        ) else {
            return;
        };
        let primary = match axis {
            crate::quorp::tui::workbench::Axis::Vertical => col,
            crate::quorp::tui::workbench::Axis::Horizontal => row,
        };
        let new_bp =
            crate::quorp::tui::workbench::ratio_bp_from_drag_position(parent, axis, primary, divider);
        if self.prismforge_dynamic_layout {
            let (mut v, mut h) = crate::quorp::tui::workbench::prismforge_ratios_from_tree(&self.workspace);
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
        if self.overlay == Overlay::Help {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.overlay = Overlay::None;
            }
            return;
        }

        match mouse.kind {
            MouseEventKind::Moved => {
                match self.splitter_visual_state {
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
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let SplitterVisualState::Dragging { index } = self.splitter_visual_state {
                    self.apply_drag_to_splitter(index, mouse.column, mouse.row);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_mouse_click(mouse.column, mouse.row);
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

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let full = frame.area();
        if full.width < 60 || full.height < 15 {
            let message = Paragraph::new("Terminal too small. Please resize (minimum 40×13).");
            frame.render_widget(message, full);
            return;
        }

        self.draw_frame_seq = self.draw_frame_seq.wrapping_add(1);
        self.ssd_moe.poll_health();

        self.chat
            .ensure_project_root(self.file_tree.root());

        let metrics = self.theme.metrics.clone();
        let shell = crate::quorp::tui::workbench::compute_shell(full, &metrics);
        self.sync_prismforge_workspace(shell.workspace, &metrics);
        let layout = crate::quorp::tui::workbench::compute_workbench(shell.workspace, &self.workspace, &metrics);

        self.hitmap.clear();

        let bg = Style::default().bg(self.theme.palette.editor_bg);
        frame.render_widget(Block::default().style(bg), full);

        frame.render_widget(
            crate::quorp::tui::chrome::TitleBar {
                text: self
                    .visual_title_override
                    .as_deref()
                    .unwrap_or("quorp-tui"),
                theme: &self.theme,
            },
            shell.titlebar,
        );

        self.render_activity_bar(frame, shell.activity);
        
        ratatui::widgets::Widget::render(
            ratatui::widgets::Block::default().style(ratatui::style::Style::default().bg(self.theme.palette.divider_bg)),
            shell.explorer_divider,
            frame.buffer_mut(),
        );
        
        self.render_explorer(
            frame,
            shell.explorer_header,
            shell.explorer_body,
            self.is_focused(Pane::FileTree),
        );

        for (splitter_index, div) in layout.splitters.iter().enumerate() {
            let bg = self.splitter_hit_color(splitter_index);
            ratatui::widgets::Widget::render(
                ratatui::widgets::Block::default().style(Style::default().bg(bg)),
                *div,
                frame.buffer_mut(),
            );
        }

        for (id, leaf_rects) in layout.leaves.iter() {
            let leaf_bg = self.theme.palette.editor_bg;
            crate::quorp::tui::paint::fill_rect(frame.buffer_mut(), leaf_rects.tabs, leaf_bg);
            crate::quorp::tui::paint::fill_rect(frame.buffer_mut(), leaf_rects.body, leaf_bg);
            crate::quorp::tui::paint::fill_rect(frame.buffer_mut(), leaf_rects.scrollbar, leaf_bg);
            if let Some(banner) = leaf_rects.banner {
                crate::quorp::tui::paint::fill_rect(frame.buffer_mut(), banner, leaf_bg);
            }
            if let Some(composer) = leaf_rects.composer {
                crate::quorp::tui::paint::fill_rect(frame.buffer_mut(), composer, leaf_bg);
            }
            if let Some(panel_tabs) = leaf_rects.panel_tabs {
                crate::quorp::tui::paint::fill_rect(frame.buffer_mut(), panel_tabs, leaf_bg);
            }
            let is_focused = self.focused == *id;

            if matches!(*id, Pane::EditorPane | Pane::Chat) {
                // Tab hit targets registered when the leaf is drawn.
            } else {
                self.hitmap.push(leaf_rects.tabs, HitTarget::LeafTab { leaf: *id, tab: 0 });
                self.hitmap.push(leaf_rects.body, HitTarget::LeafTab { leaf: *id, tab: 0 });
                if let Some(panel) = leaf_rects.panel_tabs {
                    self.hitmap.push(panel, HitTarget::PanelTab { leaf: *id, tab: 0 });
                }
            }
            if let Some(composer) = leaf_rects.composer {
                self.hitmap.push(composer, HitTarget::ComposerInput(*id));
            }
            if let Some(banner) = leaf_rects.banner {
                if matches!(*id, Pane::Chat) {
                    self.hitmap.push(banner, HitTarget::LeafBody(Pane::Chat));
                } else if !matches!(*id, Pane::EditorPane) {
                    self.hitmap.push(banner, HitTarget::LeafTab { leaf: *id, tab: 0 });
                }
            }

            match *id {
                Pane::EditorPane => {
                    self.editor_pane.sync_tree_selection(
                        self.file_tree.selected_file(),
                        self.file_tree.root(),
                    );
                    self.editor_pane
                        .ensure_active_loaded(self.file_tree.root());
                    let tab_cells = {
                        let buf = frame.buffer_mut();
                        let (cells, _) =
                            self.editor_pane.draw_tab_strip(buf, leaf_rects.tabs, &self.theme);
                        cells
                    };
                    for cell in &tab_cells {
                        self.hitmap.push(
                            cell.select_rect,
                            HitTarget::LeafTab {
                                leaf: Pane::EditorPane,
                                tab: cell.tab_index,
                            },
                        );
                        if let Some(cr) = cell.close_rect {
                            self.hitmap.push(
                                cr,
                                HitTarget::LeafTabClose {
                                    leaf: Pane::EditorPane,
                                    tab: cell.tab_index,
                                },
                            );
                        }
                    }
                    self.hitmap
                        .push(leaf_rects.body, HitTarget::LeafBody(Pane::EditorPane));
                    self.hitmap.push(
                        leaf_rects.scrollbar,
                        HitTarget::LeafBody(Pane::EditorPane),
                    );
                    self.editor_pane.render_in_leaf(
                        frame.buffer_mut(),
                        leaf_rects,
                        crate::quorp::tui::editor_pane::EditorRenderMode::Code,
                        is_focused,
                        &self.theme,
                    );
                }
                Pane::Terminal => {
                    self.terminal
                        .render_in_leaf(frame.buffer_mut(), leaf_rects, is_focused, &self.theme);
                }
                Pane::Agent => {
                    self.agent_pane.render_in_leaf(frame.buffer_mut(), leaf_rects.body, is_focused, &self.theme);
                }
                        Pane::Chat => {
                    let tab_cells = {
                        let buf = frame.buffer_mut();
                        let (cells, _) =
                            self.chat.draw_tab_strip(buf, leaf_rects.tabs, &self.theme);
                        cells
                    };
                    for cell in &tab_cells {
                        self.hitmap.push(
                            cell.select_rect,
                            HitTarget::LeafTab {
                                leaf: Pane::Chat,
                                tab: cell.tab_index,
                            },
                        );
                        if let Some(cr) = cell.close_rect {
                            self.hitmap.push(
                                cr,
                                HitTarget::LeafTabClose {
                                    leaf: Pane::Chat,
                                    tab: cell.tab_index,
                                },
                            );
                        }
                    }
                    self.hitmap
                        .push(leaf_rects.body, HitTarget::LeafBody(Pane::Chat));
                    self.hitmap.push(
                        leaf_rects.scrollbar,
                        HitTarget::LeafBody(Pane::Chat),
                    );
                    self.chat.render_in_leaf(
                        frame.buffer_mut(),
                        leaf_rects,
                        is_focused,
                        &self.theme,
                    );
                }
                _ => {}
            }
        }

        self.register_splitter_hit_targets(&layout);

        if self.overlay == Overlay::ModelPicker {
            let cx = full.width / 2;
            let cy = full.height / 2;
            let rw = 60.min(full.width.saturating_sub(4));
            let rh = 20.min(full.height.saturating_sub(4));
            let r = Rect::new(
                cx.saturating_sub(rw / 2),
                cy.saturating_sub(rh / 2),
                rw,
                rh,
            );
            self.models_pane.render(
                frame,
                r,
                &self.theme,
                true,
                self.ssd_moe.active_model.as_ref().map(|m| m.id),
                &self.ssd_moe.status(),
            );
        }

        let mode = self.focused.display_label();
        let status_indicator = self.ssd_moe.status().indicator().to_string();
        let status_label = self.ssd_moe.status().label().to_string();
        let status_right_default = format!("Flash-MOE {} {}", status_indicator, status_label);
        let status_left = self
            .visual_status_left_override
            .as_deref()
            .unwrap_or(mode);
        let status_right = self
            .visual_status_right_override
            .as_deref()
            .unwrap_or(status_right_default.as_str());
        let hints = match &self.visual_status_center_override {
            Some(s) => s.clone(),
            None => self.context_hints_for_focused_pane(),
        };

        frame.render_widget(
            crate::quorp::tui::chrome::StatusBar {
                left: status_left,
                center: &hints,
                right_status: status_right,
                theme: &self.theme,
            },
            shell.statusbar,
        );

        self.last_full_area = full;

        if self.overlay == Overlay::Help {
            self.render_help(frame, full);
        }
    }

    fn render_activity_bar(&mut self, frame: &mut Frame<'_>, area: Rect) {
        self.hitmap.push(area, HitTarget::Activity(0));
        let bg = Style::default().bg(self.theme.palette.activity_bg);
        frame.render_widget(Block::default().style(bg), area);

        let local_icons = ["☰", "⌕", "⚙", "◈"];
        let pane_map = [Pane::FileTree, Pane::EditorPane, Pane::Chat];
        for (i, icon) in local_icons.iter().enumerate() {
            let y = area.y + i as u16 * 2 + 1; // spread them out a bit
            if y >= area.y + area.height {
                break;
            }
            let is_active = pane_map.get(i).is_some_and(|p| {
                *p == self.focused
            });
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
            let padding = "  "; // 2 spaces
            let line = Line::from(vec![
                Span::styled(padding, style),
                Span::styled(*icon, style),
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

    fn render_help(&self, frame: &mut Frame<'_>, area: Rect) {
        use ratatui::widgets::{Clear, Row, Table};
        let popup_y = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                ratatui::layout::Constraint::Percentage(40),
                ratatui::layout::Constraint::Percentage(20),
                ratatui::layout::Constraint::Percentage(40),
            ])
            .split(area)[1];
        let popup_area = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Horizontal)
            .constraints([
                ratatui::layout::Constraint::Percentage(10),
                ratatui::layout::Constraint::Percentage(80),
                ratatui::layout::Constraint::Percentage(10),
            ])
            .split(popup_y)[1];
        frame.render_widget(Clear, popup_area);

        let rows = vec![
            Row::new(vec!["Click", "Global", "Focus pane"]),
            Row::new(vec!["Tab / Shift+Tab", "Global", "Cycle pane focus"]),
            Row::new(vec!["Ctrl+h/j/k/l", "Global", "Vim-style pane navigation"]),
            Row::new(vec!["Ctrl+m", "Global", "Toggle Models pane"]),
            Row::new(vec!["Esc", "Global", "Quit (or dismiss help)"]),
            Row::new(vec!["Up / Down", "File Tree", "Navigate entries"]),
            Row::new(vec!["Home / End", "File Tree", "Jump to first / last"]),
            Row::new(vec!["Enter", "File Tree", "Expand dir / Select file"]),
            Row::new(vec![
                "Left / Right",
                "File Tree",
                "Collapse (or parent) / Expand",
            ]),
            Row::new(vec!["Up/Down/PgUp/PgDn/Home/End", "Code Preview", "Scroll"]),
            Row::new(vec![
                "Alt+Up",
                "Code / Chat",
                "Focus tab strip (arrows switch tabs)",
            ]),
            Row::new(vec![
                "Left/Right",
                "Tab strip focused",
                "Previous / next tab",
            ]),
            Row::new(vec![
                "Delete / Ctrl+w",
                "Tab strip focused",
                "Close active tab",
            ]),
            Row::new(vec![
                "Ctrl+Shift+w",
                "Tab strip focused",
                "Close all tabs",
            ]),
            Row::new(vec!["Ctrl+t", "Chat", "New chat session"]),
            Row::new(vec!["Shift+PgUp/PgDn", "Terminal", "Scrollback offset"]),
            Row::new(vec!["[ / ]", "Chat", "Cycle model"]),
            Row::new(vec!["Enter", "Chat", "Send message"]),
            Row::new(vec!["Enter", "Models", "Switch model / Download"]),
            Row::new(vec!["d", "Models", "Delete downloaded model"]),
            Row::new(vec!["Ctrl+s", "Global", "Stop running model"]),
            Row::new(vec!["?", "Global", "Toggle this help"]),
        ];

        let widths = [
            ratatui::layout::Constraint::Length(30),
            ratatui::layout::Constraint::Length(25),
            ratatui::layout::Constraint::Min(20),
        ];

        let table = Table::new(rows, widths)
            .header(
                Row::new(vec!["Key", "Context", "Action"])
                    .style(Style::default().add_modifier(Modifier::BOLD)),
            )
            .block(
                Block::default()
                    .title(" Keybindings ")
                    .borders(Borders::ALL),
            )
            .column_spacing(2);

        frame.render_widget(table, popup_area);
    }

    pub fn handle_event(&mut self, event: Event) -> ControlFlow<(), ()> {
        match event {
            Event::Key(key) => {
                if key.kind == KeyEventKind::Release {
                    return ControlFlow::Continue(());
                }

                if self.overlay == Overlay::Help {
                    if key.code == crossterm::event::KeyCode::Char('k') {
                        return ControlFlow::Break(());
                    }
                    if key.code == KeyCode::Esc {
                        self.overlay = Overlay::None;
                        return ControlFlow::Continue(());
                    }
                    return ControlFlow::Continue(());
                }

                if key.code == KeyCode::Char('?') && key.modifiers.is_empty() {
                    self.overlay = Overlay::Help;
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
                            self.set_focus(Pane::EditorPane);
                            return ControlFlow::Continue(());
                        }
                    }
                }
                if self.focused == Pane::EditorPane && self.editor_pane.handle_key_event(&key) {
                    return ControlFlow::Continue(());
                }
                if self.focused == Pane::Chat && self.chat.handle_key_event(&key, &self.theme) {
                    return ControlFlow::Continue(());
                }
                if self.focused == Pane::Agent {
                    if let Ok(true) = self.agent_pane.try_handle_key(&key) {
                        return ControlFlow::Continue(());
                    }
                }
                if self.overlay == Overlay::ModelPicker {
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') => {
                            self.models_pane.handle_up();
                            return ControlFlow::Continue(());
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            self.models_pane.handle_down();
                            return ControlFlow::Continue(());
                        }
                        KeyCode::Enter => {
                            let Some(entry) = self
                                .models_pane
                                .entries
                                .get(self.models_pane.selected_index)
                                .cloned()
                            else {
                                self.overlay = Overlay::None;
                                return ControlFlow::Continue(());
                            };
                            if crate::quorp::tui::model_registry::local_moe_spec_for_registry_id(
                                &entry.registry_id,
                            )
                            .is_some()
                            {
                                crate::quorp::tui::model_registry::save_model(&entry.registry_id);
                            }
                            self.chat
                                .request_persist_default_model_to_agent_settings(&entry.registry_id);
                            self.chat
                                .set_model_index(self.models_pane.selected_index);
                            let root = self.file_tree.root().to_path_buf();
                            if let Some(spec) = crate::quorp::tui::model_registry::local_moe_spec_for_registry_id(&entry.registry_id) {
                                self.ssd_moe.switch_model(&root, &spec);
                            }
                            self.overlay = Overlay::None;
                            return ControlFlow::Continue(());
                        }
                        KeyCode::Esc => {
                            self.overlay = Overlay::None;
                            return ControlFlow::Continue(());
                        }
                        _ => {}
                    }
                }
                if self.focused == Pane::Terminal {
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
                            self.overlay = Overlay::None;
                        } else {
                            self.models_pane = ModelsPane::sync_from_chat(&self.chat);
                            if !self.models_pane.entries.is_empty() {
                                self.models_pane.selected_index = self
                                    .chat
                                    .model_index()
                                    .min(self.models_pane.entries.len() - 1);
                            }
                            self.overlay = Overlay::ModelPicker;
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
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }

    pub fn handle_mouse_click(&mut self, col: u16, row: u16) {
        if self.overlay == Overlay::Help {
            self.overlay = Overlay::None;
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
                        Pane::Agent => {

                        }
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
                        Pane::Agent => {

                        }
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
                    Pane::Agent => {

                        }
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
                    Pane::Agent => {

                        }
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
                        let _ = self.editor_pane.close_file_tab_at(i, root);
                    }
                    Pane::Agent => {

                        }
                        Pane::Chat => {
                        let i = self.chat.active_session_index();
                        let _ = self.chat.close_chat_session_at(i, &self.theme);
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
                        Pane::Agent => {

                        }
                        Pane::Chat => {
                            self.chat.close_all_chat_sessions(&self.theme);
                        }
                        _ => {}
                    }
                } else {
                    match strip_leaf {
                        Pane::EditorPane => {
                            let i = self.editor_pane.active_tab_index();
                            let _ = self.editor_pane.close_file_tab_at(i, root);
                        }
                        Pane::Agent => {

                        }
                        Pane::Chat => {
                            let i = self.chat.active_session_index();
                            let _ = self.chat.close_chat_session_at(i, &self.theme);
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
            futures::channel::mpsc::UnboundedSender<
                crate::quorp::tui::bridge::TuiToBackendRequest,
            >,
            Vec<String>,
            usize,
        )>,
    ) -> Self {
        let mut ssd_moe = SsdMoeManager::new();
        ssd_moe.set_status_for_test(crate::quorp::tui::ssd_moe_tui::ModelStatus::Running);
        ssd_moe.active_model =
            Some(crate::quorp::tui::model_registry::local_moe_catalog()[0].clone());
        let path_index = std::sync::Arc::new(crate::quorp::tui::path_index::PathIndex::new(
            fixture_root.clone(),
        ));
        let agent_bridge_tx = unified_language_model_boot.as_ref().map(|(tx, _, _)| tx.clone());
        let uses_language_model_registry = unified_language_model_boot.is_some();
        let mut chat = ChatPane::new(
            tx,
            handle,
            fixture_root.clone(),
            path_index,
            unified_language_model_boot.clone(),
            None,
        );
        if !uses_language_model_registry {
            chat.set_model_index_for_test(0);
        }
        let models_pane = ModelsPane::sync_from_chat(&chat);
        let theme = Theme::antigravity();
        Self {
            focused: Pane::EditorPane,
            right_pane: Pane::Chat,
            last_left_pane: Pane::EditorPane,
            file_tree: FileTree::with_root(fixture_root),
            editor_pane: EditorPane::new(),
            terminal: TerminalPane::with_bridge(unified_language_model_boot.as_ref().map(|(tx, _, _)| tx.clone())),
            agent_pane: AgentPane::new(agent_bridge_tx),
            chat,
            models_pane,
            ssd_moe,
            _runtime: runtime,
            _event_rx_keepalive: event_rx_keepalive,
            overlay: Overlay::None,
            app_state: None,
            unified_bridge_tx: None,
            last_full_area: Rect::default(),
            theme,
            hitmap: HitMap::new(),
            workspace: crate::quorp::tui::workbench::default_antigravity_tree(),
            visual_status_center_override: Some("/fixture/project".to_string()),
            visual_title_override: None,
            visual_status_left_override: None,
            visual_status_right_override: None,
            prismforge_dynamic_layout: false,
            prismforge_split_ratio_lock: None,
            splitter_visual_state: SplitterVisualState::Idle,
            tab_strip_focus: None,
            draw_frame_seq: 0,
        }
    }
}

impl Drop for TuiApp {
    fn drop(&mut self) {
        self.ssd_moe.stop();
    }
}

#[cfg(test)]
impl TuiApp {
    pub fn leak_runtime_for_test_exit(&mut self) {
        std::mem::forget(self._runtime.take());
    }
}

impl TuiApp {
    /// Deterministic app state for visual regression (no Flash-MOE autostart, fixed model index).
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
        app.visual_status_left_override =
            Some("main • 3 agents • 0 errors • 12 tasks".to_string());
        app.visual_status_right_override = Some("Flash-MOE • Online".to_string());
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

    /// Fixture-backed app with a live Tokio runtime for chat HTTP / streaming tests. Caller must
    /// keep the returned receiver alive so the UI event channel stays open.
    pub fn new_for_flow_tests(
        fixture_root: std::path::PathBuf,
    ) -> (
        Self,
        std::sync::mpsc::Receiver<crate::quorp::tui::TuiEvent>,
    ) {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let handle = runtime.handle().clone();
        let (tx, rx) = std::sync::mpsc::sync_channel::<crate::quorp::tui::TuiEvent>(128);
        let app = Self::new_fixture_inner(tx, handle, fixture_root, Some(runtime), None, None);
        (app, rx)
    }

    /// Same as [`Self::new_for_flow_tests`], but chat uses `provider/model` ids and the language-model
    /// bridge sender (production-shaped). Returns the bridge receiver for tests that assert requests.
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
    /// Applies backend-driven events the same way as [`crate::quorp::tui::run`] (bridges on the GPUI side).
    pub fn apply_tui_backend_event(&mut self, event: crate::quorp::tui::TuiEvent) {
        use crate::quorp::tui::TuiEvent;
        match event {
            TuiEvent::Chat(ev) => self.chat.apply_chat_event(ev, &self.theme),
            TuiEvent::TerminalFrame(frame) => self.terminal.apply_integrated_frame(frame),
            TuiEvent::TerminalClosed => self.terminal.mark_integrated_session_closed(),
            TuiEvent::FileTreeListed { parent, result } => {
                self.file_tree.apply_project_listing(parent, result);
            }
            TuiEvent::UnifiedResponse(crate::quorp::tui::bridge::BackendToTuiResponse::BufferChunk {
                path,
                lines,
                error,
                truncated,
            }) => self.editor_pane.apply_editor_pane_buffer_snapshot(
                path,
                lines,
                error,
                truncated,
            ),
            TuiEvent::UnifiedResponse(crate::quorp::tui::bridge::BackendToTuiResponse::AgentStatusUpdate(s)) => {
                self.agent_pane.apply_status_update(s);
            }
            TuiEvent::PathIndexSnapshot {
                root,
                entries,
                files_seen,
            } => self
                .chat
                .apply_path_index_snapshot(root, entries, files_seen),
            TuiEvent::Crossterm(ev) => {
                let _ = self.handle_event(ev);
            }
            TuiEvent::UnifiedResponse(resp) => {
                log::debug!("Unhandled unified response in test: {:?}", resp);
            }
            TuiEvent::ThemeReloaded => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

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
        for _ in 0..5 {
            p = p.next();
        }
        assert_eq!(p, Pane::EditorPane);

        let mut p = Pane::EditorPane;
        for _ in 0..5 {
            p = p.prev();
        }
        assert_eq!(p, Pane::EditorPane);
    }

    #[test]
    fn tab_and_backtab_move_focus() {
        let mut app = TuiApp::new();
        assert_eq!(app.focused, Pane::EditorPane);

        let tab = Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(app.handle_event(tab).is_continue());
        assert_eq!(app.focused, Pane::Terminal);

        let back = Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert!(app.handle_event(back).is_continue());
        assert_eq!(app.focused, Pane::EditorPane);
    }

    #[test]
    fn esc_quits_from_every_pane() {
        for pane in [
            Pane::EditorPane,
            Pane::Terminal,
            Pane::Chat,
            Pane::FileTree,
        ] {
            let mut app = TuiApp::new();
            app.focused = pane;
            let esc = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            assert!(app.handle_event(esc).is_break());
        }
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
        assert_eq!(app.focused, Pane::FileTree);
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
        assert_eq!(app.focused, Pane::Agent);
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

        // Chat -> Agent
        app.focused = Pane::Chat;
        assert!(app.handle_event(ctrl_j).is_continue());
        assert_eq!(app.focused, Pane::Agent);
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
        assert!(s.contains("Mode: Code"), "{s}");
        assert!(
            s.contains("Model:") && s.contains(app.chat.current_model_id()),
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
        assert!(app.status_bar_text().contains("Mode: Chat"));

        app.focused = Pane::FileTree;
        assert!(app.status_bar_text().contains("Mode: Files"));
    }

    #[test]
    fn mouse_click_focuses_panes() {
        let mut app = TuiApp::new();
        let backend = ratatui::backend::TestBackend::new(232, 64);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();

        app.focused = Pane::EditorPane;
        app.handle_mouse_click(10, 10); // x=10, y=10 -> Explorer
        assert_eq!(app.focused, Pane::FileTree);

        app.handle_mouse_click(50, 5); // x=50, y=5 -> EditorPane
        assert_eq!(app.focused, Pane::EditorPane);

        app.handle_mouse_click(50, 30); // x=50, y=30 -> Terminal
        assert_eq!(app.focused, Pane::Terminal);

        app.handle_mouse_click(150, 30); // x=150, y=30 -> Chat
        assert_eq!(app.focused, Pane::Chat);
    }

    #[test]
    fn mouse_click_dismisses_help() {
        let mut app = TuiApp::new();
        let backend = ratatui::backend::TestBackend::new(232, 64);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();

        app.overlay = Overlay::Help;
        app.handle_mouse_click(5, 5);
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn ctrl_j_escapes_terminal_pane() {
        let mut app = TuiApp::new();
        app.focused = Pane::Terminal;
        app.last_left_pane = Pane::Terminal;
        let ctrl_j = Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert!(app.handle_event(ctrl_j).is_continue());
        assert_eq!(app.focused, Pane::Chat);
    }

    #[test]
    fn ctrl_k_escapes_terminal_pane() {
        let mut app = TuiApp::new();
        app.focused = Pane::Terminal;
        app.last_left_pane = Pane::Terminal;
        let ctrl_k = Event::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert!(app.handle_event(ctrl_k).is_continue());
        assert_eq!(app.focused, Pane::EditorPane);
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
}

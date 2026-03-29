#![allow(unused)]
//! Navigable project file tree. With a project bridge (see [`crate::quorp::tui::project_bridge`]),
//! listings use Quorp worktrees (gitignore, trust, multi-root). Without a bridge (flow tests, `ui_lab`),
//! listings use a plain directory read.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use unicode_width::UnicodeWidthStr;

use crate::quorp::tui::path_guard::path_within_project;
use crate::quorp::tui::text_width::truncate_fit;

#[derive(Clone, Debug)]
pub struct TreeChild {
    pub path: PathBuf,
    pub name: String,
    pub is_directory: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileTreeKeyOutcome {
    NotHandled,
    Handled,
    OpenedFile,
}

#[derive(Clone, Debug)]
struct VisibleRow {
    path: PathBuf,
    display_label: String,
    is_dir: bool,
    depth: usize,
}

fn path_is_descendant(child: &Path, ancestor: &Path) -> bool {
    child != ancestor
        && child
            .strip_prefix(ancestor)
            .is_ok_and(|rest| !rest.as_os_str().is_empty())
}

pub struct FileTree {
    root: PathBuf,
    root_error: Option<String>,
    expanded: HashSet<PathBuf>,
    children: HashMap<PathBuf, Vec<TreeChild>>,
    visible_rows: Vec<VisibleRow>,
    selected_index: usize,
    scroll_offset: usize,
    selected_file: Option<PathBuf>,
    last_error: Option<String>,
    viewport_height: usize,
    project_list_tx: Option<futures::channel::mpsc::UnboundedSender<crate::quorp::tui::bridge::TuiToBackendRequest>>,
    pending_loads: HashSet<PathBuf>,
}

/// Direct children of `dir` for standalone / harness use (no [`Project`] entity).
///
/// Does not apply `.gitignore`; production `quorp` uses `project_bridge` / worktree entries instead.
pub fn read_children(dir: &Path, project_root: &Path) -> Result<Vec<TreeChild>, String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.as_path() == dir {
            continue;
        }
        if !path_within_project(&path, project_root) {
            continue;
        }
        let is_directory = path.is_dir();
        let name = entry
            .file_name()
            .to_str()
            .unwrap_or("<invalid>")
            .to_string();
        out.push(TreeChild {
            path,
            name,
            is_directory,
        });
    }
    out.sort_by(|a, b| match (a.is_directory, b.is_directory) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(out)
}
fn file_style(path: &Path, is_dir: bool, palette: &crate::quorp::tui::theme::Palette) -> Style {
    if is_dir {
        return Style::default()
            .fg(palette.folder_blue)
            .add_modifier(Modifier::BOLD);
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let color = match ext.as_str() {
        "rs" => palette.file_rust,
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => palette.file_ts_js,
        "py" | "pyi" => palette.file_py,
        "md" | "txt" | "adoc" | "rst" => palette.file_doc,
        "toml" | "yaml" | "yml" | "json" | "json5" | "ini" | "cfg" | "conf" | "env" => palette.file_cfg,
        "sh" | "bash" | "zsh" | "fish" | "nu" => palette.file_shell,
        "html" | "htm" | "css" | "scss" | "sass" | "less" => palette.file_orange,
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" | "webp" | "bmp" => palette.text_muted,
        "lock" => palette.text_faint,
        "go" => palette.file_ts_js,
        "c" | "h" => palette.file_ts_js,
        "cpp" | "cxx" | "cc" | "hpp" | "hxx" => palette.file_ts_js,
        "java" | "kt" | "kts" => palette.file_orange,
        "rb" | "erb" => palette.file_orange,
        "swift" => palette.file_orange,
        "zig" => palette.file_orange,
        _ => palette.text_primary,
    };
    Style::default().fg(color)
}

impl FileTree {
    pub fn new() -> Self {
        match std::env::current_dir() {
            Ok(p) => Self::with_root(p),
            Err(e) => Self::with_root_failed(PathBuf::from("."), e.to_string()),
        }
    }

    fn with_root_failed(root: PathBuf, err: String) -> Self {
        let mut tree = Self {
            root,
            root_error: Some(err),
            expanded: HashSet::new(),
            children: HashMap::new(),
            visible_rows: Vec::new(),
            selected_index: 0,
            scroll_offset: 0,
            selected_file: None,
            last_error: None,
            viewport_height: 24,
            project_list_tx: None,
            pending_loads: HashSet::new(),
        };
        tree.rebuild_visible_rows();
        tree
    }

    pub fn with_root(root: PathBuf) -> Self {
        let (root, root_error) = match root.canonicalize() {
            Ok(p) => (p, None),
            Err(e) => (root, Some(e.to_string())),
        };
        let mut tree = Self {
            root,
            root_error,
            expanded: HashSet::new(),
            children: HashMap::new(),
            visible_rows: Vec::new(),
            selected_index: 0,
            scroll_offset: 0,
            selected_file: None,
            last_error: None,
            viewport_height: 24,
            project_list_tx: None,
            pending_loads: HashSet::new(),
        };
        tree.expanded.insert(tree.root.clone());
        tree.rebuild_visible_rows();
        tree
    }

    pub fn set_project_list_sender(
        &mut self,
        sender: futures::channel::mpsc::UnboundedSender<crate::quorp::tui::bridge::TuiToBackendRequest>,
    ) {
        self.project_list_tx = Some(sender);
    }

    pub fn apply_project_listing(
        &mut self,
        parent: PathBuf,
        result: Result<Vec<TreeChild>, String>,
    ) {
        self.pending_loads.remove(&parent);
        match result {
            Ok(children) => {
                self.children.insert(parent, children);
                self.last_error = None;
            }
            Err(message) => {
                self.last_error = Some(message);
            }
        }
        self.rebuild_visible_rows();
    }

    pub fn selected_file(&self) -> Option<&Path> {
        self.selected_file.as_deref()
    }

    /// Project root (canonicaliquorp when `with_root` succeeds). Used to constrain preview reads.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn set_selected_file(&mut self, path: Option<PathBuf>) {
        self.selected_file = path;
    }

    fn load_children(&mut self, path: &Path) -> bool {
        if self.children.contains_key(path) {
            return true;
        }
        if let Some(ref tx) = self.project_list_tx {
            let path_buf = path.to_path_buf();
            if self.pending_loads.contains(&path_buf) {
                return false;
            }
            self.pending_loads.insert(path_buf.clone());
            if tx
                .unbounded_send(crate::quorp::tui::bridge::TuiToBackendRequest::ListDirectory(
                    path_buf.clone(),
                ))
                .is_err()
            {
                self.pending_loads.remove(&path_buf);
                self.last_error = Some("file tree bridge disconnected".to_string());
                return false;
            }
            return false;
        }
        match read_children(path, &self.root) {
            Ok(children) => {
                self.children.insert(path.to_path_buf(), children);
                self.last_error = None;
                true
            }
            Err(message) => {
                self.last_error = Some(message);
                false
            }
        }
    }

    fn rebuild_visible_rows(&mut self) {
        self.visible_rows.clear();
        if self.root_error.is_some() {
            return;
        }
        let root_path = self.root.clone();
        let root_name = root_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("/")
            .to_string();
        self.visit_node(&root_path, &root_name, 0);
        self.clamp_selection();
    }

    fn visit_node(
        &mut self,
        path: &Path,
        display_name: &str,
        depth: usize,
    ) {
        let is_dir = path.is_dir();
        let display_label = if is_dir {
            format!("{}/", display_name)
        } else {
            display_name.to_string()
        };
        self.visible_rows.push(VisibleRow {
            path: path.to_path_buf(),
            display_label,
            is_dir,
            depth,
        });

        if !is_dir || !self.expanded.contains(path) {
            return;
        }

        if !self.load_children(path) {
            return;
        }
        let Some(children) = self.children.get(path).cloned() else {
            return;
        };

        for child in children.iter() {
            let next_depth = if path == self.root { depth } else { depth + 1 };
            self.visit_node(&child.path, &child.name, next_depth);
        }
    }

    fn clamp_selection(&mut self) {
        if self.visible_rows.is_empty() {
            self.selected_index = 0;
            return;
        }
        if self.selected_index >= self.visible_rows.len() {
            self.selected_index = self.visible_rows.len() - 1;
        }
    }

    fn collapse_adjust_selection(&mut self, collapsed_path: &Path) {
        let Some(selected) = self.visible_rows.get(self.selected_index) else {
            return;
        };
        if !path_is_descendant(&selected.path, collapsed_path) {
            return;
        }
        if let Some(idx) = self
            .visible_rows
            .iter()
            .position(|r| r.path == collapsed_path)
        {
            self.selected_index = idx;
        }
    }

    fn toggle_expand(&mut self, path: &Path) {
        if self.expanded.contains(path) {
            self.collapse_adjust_selection(path);
            self.expanded.remove(path);
        } else if self.load_children(path) {
            self.expanded.insert(path.to_path_buf());
        }
        self.rebuild_visible_rows();
    }

    fn expand_only(&mut self, path: &Path) {
        if self.expanded.contains(path) {
            return;
        }
        if !self.load_children(path) {
            return;
        }
        self.expanded.insert(path.to_path_buf());
        self.rebuild_visible_rows();
    }

    fn collapse_only(&mut self, path: &Path) {
        if !self.expanded.contains(path) {
            return;
        }
        self.collapse_adjust_selection(path);
        self.expanded.remove(path);
        self.rebuild_visible_rows();
    }

    /// Selects a file for preview, toggles a directory, or records an error. Returns `true` when a
    /// file was opened (caller may move focus to code preview).
    fn on_enter(&mut self) -> bool {
        let Some(row) = self.visible_rows.get(self.selected_index).cloned() else {
            return false;
        };
        if row.is_dir {
            self.toggle_expand(&row.path);
            false
        } else if path_within_project(&row.path, &self.root) {
            self.last_error = None;
            self.selected_file = Some(row.path);
            true
        } else {
            self.last_error =
                Some("Refusing selection: path resolves outside the project root".to_string());
            false
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.visible_rows.is_empty() {
            return;
        }
        let len = self.visible_rows.len();
        let current = self.selected_index as isize;
        let next = (current + delta).clamp(0, len as isize - 1) as usize;
        self.selected_index = next;
    }

    fn page_move(&mut self, viewport_height: usize, forward: bool) {
        if viewport_height == 0 || self.visible_rows.is_empty() {
            return;
        }
        let step = viewport_height.saturating_sub(1).max(1);
        let delta = if forward {
            step as isize
        } else {
            -(step as isize)
        };
        self.move_selection(delta);
    }

    fn ensure_scroll(&mut self, viewport_height: usize) {
        if self.visible_rows.is_empty() {
            self.scroll_offset = 0;
            return;
        }
        if viewport_height == 0 {
            return;
        }
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset.saturating_add(viewport_height) {
            self.scroll_offset = self
                .selected_index
                .saturating_sub(viewport_height.saturating_sub(1));
        }
        let max_scroll = self
            .visible_rows
            .len()
            .saturating_sub(viewport_height.max(1));
        if self.scroll_offset > max_scroll {
            self.scroll_offset = max_scroll;
        }
    }

    pub fn handle_key_event(&mut self, key: &KeyEvent) -> FileTreeKeyOutcome {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT)
        {
            return FileTreeKeyOutcome::NotHandled;
        }
        match key.code {
            KeyCode::Up => {
                self.move_selection(-1);
                FileTreeKeyOutcome::Handled
            }
            KeyCode::Down => {
                self.move_selection(1);
                FileTreeKeyOutcome::Handled
            }
            KeyCode::Enter => {
                if self.on_enter() {
                    FileTreeKeyOutcome::OpenedFile
                } else {
                    FileTreeKeyOutcome::Handled
                }
            }
            KeyCode::Right => {
                if let Some(row) = self.visible_rows.get(self.selected_index).cloned() {
                    if row.is_dir {
                        self.expand_only(&row.path);
                    }
                }
                FileTreeKeyOutcome::Handled
            }
            KeyCode::Left => {
                if let Some(row) = self.visible_rows.get(self.selected_index).cloned() {
                    if row.is_dir {
                        self.collapse_only(&row.path);
                    } else if let Some(parent) = row.path.parent() {
                        if let Some(idx) = self.visible_rows.iter().position(|r| r.path == parent) {
                            self.selected_index = idx;
                        }
                    }
                }
                FileTreeKeyOutcome::Handled
            }
            KeyCode::PageUp => {
                self.page_move(self.viewport_height, false);
                FileTreeKeyOutcome::Handled
            }
            KeyCode::PageDown => {
                self.page_move(self.viewport_height, true);
                FileTreeKeyOutcome::Handled
            }
            KeyCode::Home => {
                self.selected_index = 0;
                FileTreeKeyOutcome::Handled
            }
            KeyCode::End => {
                if !self.visible_rows.is_empty() {
                    self.selected_index = self.visible_rows.len() - 1;
                }
                FileTreeKeyOutcome::Handled
            }
            _ => FileTreeKeyOutcome::NotHandled,
        }
    }

    pub fn render(&mut self, frame: &mut Frame<'_>, inner: Rect, pane_focused: bool, theme: &crate::quorp::tui::theme::Theme) {
        if inner.height == 0 || inner.width == 0 {
            return;
        }

        self.viewport_height = inner.height as usize;

        if let Some(ref err) = self.root_error {
            let line = Line::from(format!("Error: {}", err));
            frame.render_widget(Paragraph::new(line), inner);
            return;
        }

        let viewport = self.viewport_height;
        let error_reserve = usize::from(self.last_error.is_some());
        let tree_viewport = viewport.saturating_sub(error_reserve);
        self.ensure_scroll(tree_viewport);

        let total_rows = self.visible_rows.len();
        let show_scrollbar = total_rows > tree_viewport && inner.width > 1;
        let content_width = if show_scrollbar {
            inner.width.saturating_sub(1)
        } else {
            inner.width
        } as usize;

        let start = self.scroll_offset;
        let end = (start + tree_viewport).min(total_rows);
        let mut lines: Vec<Line> = Vec::new();

        if let Some(msg) = &self.last_error {
            lines.push(Line::from(Span::styled(
                truncate_fit(&format!("! {}", msg), content_width),
                Style::default().fg(Color::Red),
            )));
        }

        for index in start..end {
            let Some(row) = self.visible_rows.get(index) else {
                continue;
            };
            let is_selected = pane_focused && index == self.selected_index;
            let depth = row.depth * 2;
            let indent = " ".repeat(depth);
            let icon = if row.is_dir {
                if self.expanded.contains(&row.path) {
                    theme.glyphs.chevron_down
                } else {
                    theme.glyphs.chevron_right
                }
            } else {
                theme.glyphs.file_icon
            };

            let prefix = if row.path == self.root {
                String::new()
            } else {
                format!("{indent}{icon} ")
            };

            let prefix_width = UnicodeWidthStr::width(prefix.as_str());
            let name_budget = content_width.saturating_sub(prefix_width);
            let label_str = if row.is_dir { 
                row.display_label.trim_end_matches('/').to_string() 
            } else { 
                row.display_label.clone() 
            };
            let label = truncate_fit(&label_str, name_budget);

            let bg = if is_selected {
                theme.palette.row_selected_bg
            } else {
                theme.palette.sidebar_bg
            };

            let mut style = file_style(&row.path, row.is_dir, &theme.palette).bg(bg);
            let prefix_style = Style::default().fg(theme.palette.text_muted).bg(bg);

            if is_selected {
                // If we want a stronger indication than just bg, we can optionally reverse or bold.
                // The current visual spec just implies subtle row selection bg.
                style = style.add_modifier(Modifier::BOLD);
            }

            // Fill the rest of the row with background
            let text_width = prefix_width + UnicodeWidthStr::width(label.as_str());
            let padding_len = content_width.saturating_sub(text_width);
            let padding = " ".repeat(padding_len);

            lines.push(Line::from(vec![
                Span::styled(prefix, prefix_style),
                Span::styled(label, style),
                Span::styled(padding, Style::default().bg(bg)),
            ]));
        }

        while lines.len() < viewport {
            lines.push(Line::from(""));
        }

        let scrollbar_total = total_rows.max(1);

        let content_area = if show_scrollbar {
            Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width.saturating_sub(1),
                height: inner.height,
            }
        } else {
            inner
        };

        frame.render_widget(Paragraph::new(lines), content_area);

        if show_scrollbar {
            let scrollbar_area = Rect {
                x: inner.x + inner.width.saturating_sub(1),
                y: inner.y,
                width: 1,
                height: inner.height,
            };
            let mut scrollbar_state = ScrollbarState::new(scrollbar_total)
                .position(self.scroll_offset)
                .viewport_content_length(tree_viewport);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
            frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
        }
    }

    pub fn render_in_leaf(
        &mut self,
        buf: &mut ratatui::buffer::Buffer,
        rects: &crate::quorp::tui::workbench::LeafRects,
        focused: bool,
        theme: &crate::quorp::tui::theme::Theme,
    ) {
        use ratatui::widgets::{Widget, StatefulWidget};

        let tabs = vec![crate::quorp::tui::chrome_v2::LeafTabVm {
            label: "Explorer".to_string(),
            active: true,
            icon: None,
        }];
        crate::quorp::tui::chrome_v2::render_leaf_tab_strip(buf, rects.tabs, &tabs, &theme.palette);

        crate::quorp::tui::paint::fill_rect(buf, rects.body, theme.palette.sidebar_bg);
        crate::quorp::tui::paint::fill_rect(buf, rects.scrollbar, theme.palette.sidebar_bg);

        if rects.body.height == 0 || rects.body.width == 0 {
            return;
        }

        self.viewport_height = rects.body.height as usize;

        if let Some(ref err) = self.root_error {
            let line = Line::from(format!("Error: {}", err));
            Paragraph::new(line).render(rects.body, buf);
            return;
        }

        let viewport = self.viewport_height;
        let error_reserve = usize::from(self.last_error.is_some());
        let tree_viewport = viewport.saturating_sub(error_reserve);
        self.ensure_scroll(tree_viewport);

        let total_rows = self.visible_rows.len();
        let show_scrollbar = total_rows > tree_viewport;
        let content_width = rects.body.width as usize;

        let start = self.scroll_offset;
        let end = (start + tree_viewport).min(total_rows);
        let mut lines: Vec<Line> = Vec::new();

        if let Some(msg) = &self.last_error {
            lines.push(Line::from(Span::styled(
                truncate_fit(&format!("! {}", msg), content_width),
                Style::default().fg(theme.palette.danger_orange),
            )));
        }

        for index in start..end {
            let Some(row) = self.visible_rows.get(index) else {
                continue;
            };
            let is_selected = focused && index == self.selected_index;
            let depth = row.depth * 2;
            let indent = " ".repeat(depth);
            let icon = if row.is_dir {
                if self.expanded.contains(&row.path) {
                    theme.glyphs.chevron_down
                } else {
                    theme.glyphs.chevron_right
                }
            } else {
                theme.glyphs.file_icon
            };

            let prefix = if row.path == self.root {
                String::new()
            } else {
                format!("{indent}{icon} ")
            };

            let prefix_width = UnicodeWidthStr::width(prefix.as_str());
            let name_budget = content_width.saturating_sub(prefix_width);
            let label_str = if row.is_dir { 
                row.display_label.trim_end_matches('/').to_string() 
            } else { 
                row.display_label.clone() 
            };
            let label = truncate_fit(&label_str, name_budget);

            let bg = if is_selected {
                theme.palette.row_selected_bg
            } else {
                theme.palette.sidebar_bg
            };

            let mut style = file_style(&row.path, row.is_dir, &theme.palette).bg(bg);
            let prefix_style = Style::default().fg(theme.palette.text_muted).bg(bg);

            if is_selected {
                style = style.add_modifier(Modifier::BOLD);
            }

            let text_width = prefix_width + UnicodeWidthStr::width(label.as_str());
            let padding_len = content_width.saturating_sub(text_width);
            let padding = " ".repeat(padding_len);

            lines.push(Line::from(vec![
                Span::styled(prefix, prefix_style),
                Span::styled(label, style),
                Span::styled(padding, Style::default().bg(bg)),
            ]));
        }

        while lines.len() < viewport {
            lines.push(Line::from(vec![Span::styled(
                " ".repeat(content_width),
                Style::default().bg(theme.palette.sidebar_bg),
            )]));
        }

        Paragraph::new(lines).render(rects.body, buf);

        if show_scrollbar && rects.scrollbar.width > 0 {
            let mut scrollbar_state = ScrollbarState::new(total_rows.max(1))
                .position(self.scroll_offset)
                .viewport_content_length(tree_viewport);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .track_symbol(Some(" "))
                .thumb_symbol(" ")
                .style(Style::default().fg(theme.palette.scrollbar_thumb).bg(theme.palette.scrollbar_track));
            
            // Render directly into the scrollbar rect
            StatefulWidget::render(scrollbar, rects.scrollbar, buf, &mut scrollbar_state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn read_children_depth_one_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let a = temp.path().join("a");
        fs::create_dir_all(a.join("b").join("c")).expect("mkdir");
        fs::write(a.join("leaf.txt"), "x").expect("write");
        let children = read_children(&a, temp.path()).expect("read");
        let names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"b"));
        assert!(names.contains(&"leaf.txt"));
        assert!(!names.contains(&"c"));
    }

    fn init_git_repo(root: &Path) {
        fs::create_dir(root.join(".git")).expect(".git");
    }

    #[test]
    fn read_children_standalone_lists_gitignored_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        init_git_repo(temp.path());
        fs::write(temp.path().join(".gitignore"), "ignored.txt\n").expect("gitignore");
        fs::write(temp.path().join("ignored.txt"), "").expect("ignored");
        fs::write(temp.path().join("visible.txt"), "").expect("visible");
        let children = read_children(temp.path(), temp.path()).expect("read");
        let names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"visible.txt"));
        assert!(
            names.contains(&"ignored.txt"),
            "standalone read_dir does not parse gitignore; use project bridge in production"
        );
    }

    #[test]
    fn visible_rows_expand_collapse() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("f.txt"), "").expect("write");
        fs::create_dir(temp.path().join("d")).expect("dir");
        let mut tree = FileTree::with_root(temp.path().to_path_buf());
        assert!(tree.visible_rows.len() >= 1);
        let root_idx = tree
            .visible_rows
            .iter()
            .position(|r| r.path == tree.root)
            .expect("root row");
        tree.selected_index = root_idx;
        tree.on_enter();
        assert!(!tree.expanded.contains(&tree.root));
        assert_eq!(tree.visible_rows.len(), 1);
        tree.selected_index = root_idx;
        tree.on_enter();
        assert!(tree.expanded.contains(&tree.root));
        assert!(tree.visible_rows.len() >= 3);
    }

    #[test]
    fn path_descendant_not_confused_with_name_prefix() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir_a = temp.path().join("a");
        let dir_ab = temp.path().join("ab");
        fs::create_dir_all(&dir_a).expect("mkdir");
        fs::create_dir_all(&dir_ab).expect("mkdir");
        assert!(!path_is_descendant(&dir_ab, &dir_a), "ab is not under a/");
        let file = dir_a.join("x.txt");
        fs::write(&file, "").expect("write");
        assert!(path_is_descendant(&file, &dir_a));
    }

    #[test]
    fn enter_on_file_sets_selected_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("note.txt"), "hi").expect("write");
        let mut tree = FileTree::with_root(temp.path().to_path_buf());
        let file_idx = tree
            .visible_rows
            .iter()
            .position(|r| r.path.file_name().is_some_and(|n| n == "note.txt"))
            .expect("file row");
        tree.selected_index = file_idx;
        assert!(tree.on_enter());
        assert!(
            tree.selected_file
                .as_ref()
                .is_some_and(|p| { p.file_name().is_some_and(|n| n == "note.txt") })
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_outside_root_not_listed_as_child() {
        let root = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        let target = outside.path().join("secret.txt");
        fs::write(&target, "x").expect("write");
        let link = root.path().join("outside_link");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let children = read_children(root.path(), root.path()).expect("read");
        assert!(!children.iter().any(|c| c.name == "outside_link"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_outside_resolves_outside_project_root() {
        let root = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        let target = outside.path().join("secret.txt");
        fs::write(&target, "x").expect("write");
        let link = root.path().join("outside_link");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert!(!path_within_project(&link, root.path()));
    }

    #[test]
    fn home_selects_first_entry() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("a.txt"), "").expect("write");
        fs::write(temp.path().join("b.txt"), "").expect("write");
        let mut tree = FileTree::with_root(temp.path().to_path_buf());
        tree.selected_index = 2;
        let key = KeyEvent::new(KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(
            tree.handle_key_event(&key),
            FileTreeKeyOutcome::Handled
        );
        assert_eq!(tree.selected_index, 0);
    }

    #[test]
    fn end_selects_last_entry() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("a.txt"), "").expect("write");
        fs::write(temp.path().join("b.txt"), "").expect("write");
        let mut tree = FileTree::with_root(temp.path().to_path_buf());
        tree.selected_index = 0;
        let key = KeyEvent::new(KeyCode::End, KeyModifiers::NONE);
        assert_eq!(
            tree.handle_key_event(&key),
            FileTreeKeyOutcome::Handled
        );
        assert!(tree.selected_index > 0);
        assert_eq!(tree.selected_index, tree.visible_rows.len() - 1);
    }

    #[test]
    fn left_on_file_jumps_to_parent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sub = temp.path().join("sub");
        fs::create_dir(&sub).expect("mkdir");
        fs::write(sub.join("child.txt"), "").expect("write");
        let mut tree = FileTree::with_root(temp.path().to_path_buf());
        let can_sub = sub.canonicalize().unwrap();
        tree.toggle_expand(&can_sub);
        let file_idx = tree
            .visible_rows
            .iter()
            .position(|r| r.path.file_name().is_some_and(|n| n == "child.txt"))
            .expect("child.txt");
        let parent_idx = tree
            .visible_rows
            .iter()
            .position(|r| r.path == can_sub)
            .expect("sub dir");
        tree.selected_index = file_idx;
        let key = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(
            tree.handle_key_event(&key),
            FileTreeKeyOutcome::Handled
        );
        assert_eq!(tree.selected_index, parent_idx);
    }

    #[test]
    fn file_style_rust_returns_orange() {
        let palette = crate::quorp::tui::theme::Theme::core_tui().palette;
        let style = file_style(Path::new("test.rs"), false, &palette);
        assert_eq!(style.fg, Some(palette.file_rust));
    }

    #[test]
    fn file_style_directory_returns_blue_bold() {
        let palette = crate::quorp::tui::theme::Theme::core_tui().palette;
        let style = file_style(Path::new("some_dir"), true, &palette);
        assert_eq!(style.fg, Some(palette.folder_blue));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }
}

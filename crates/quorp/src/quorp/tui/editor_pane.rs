#![allow(unused)]
//! Read-only file preview for the code pane (Phase 3b).
//!
//! **Integrated (`quorp` with bridge):** [`crate::quorp::tui::bridge`] opens the path with
//! [`project::Project::open_local_buffer`], subscribes to [`language::Buffer`] events, and converts
//! [`language::BufferSnapshot::chunks`] (Tree-sitter + syntax theme + diagnostics) into
//! [`ratatui::text::Line`]s for this pane.
//!
//! **Standalone (harnesses, `ui_lab`):** no bridge — UTF-8 text is read from disk with
//! [`read_file_capped`] (no separate syntect pipeline here; chat code fences still use syntect).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use crate::quorp::tui::path_guard::path_within_project;
use crate::quorp::tui::text_width::truncate_prefix_fit;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorRenderMode {
    Code,
    MarkdownPreview,
    EmptyState,
}

const MAX_READ_BYTES: usize = 2 * 1024 * 1024;
const TRUNCATION_BANNER: &str = "File truncated (first 2 MiB shown).";
#[cfg(not(test))]
const MAX_SCROLL_PATH_ENTRIES: usize = 256;
#[cfg(test)]
const MAX_SCROLL_PATH_ENTRIES: usize = 4;

fn gutter_style(focused: bool, theme: &crate::quorp::tui::theme::Theme) -> Style {
    let base = Style::default().fg(theme.palette.text_faint);
    if focused {
        base
    } else {
        base.add_modifier(Modifier::DIM)
    }
}

fn lines_from_plain_source(source: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = source
        .lines()
        .map(|line| {
            let line = line.trim_end_matches('\r');
            let expanded = line.replace('\t', "    ");
            Line::from(expanded)
        })
        .collect();
    if lines.is_empty() {
        lines.push(Line::default());
    }
    lines
}

fn truncate_line_to_width(line: Line<'static>, max_width: usize) -> Line<'static> {
    if line.width() <= max_width {
        return line;
    }
    let mut used = 0usize;
    let mut new_spans: Vec<Span<'static>> = Vec::new();
    for span in line.spans {
        let w = span.width();
        if used + w <= max_width {
            new_spans.push(span);
            used += w;
            continue;
        }
        let remaining = max_width.saturating_sub(used);
        if remaining == 0 {
            break;
        }
        let truncated = truncate_prefix_fit(span.content.as_ref(), remaining);
        new_spans.push(Span::styled(truncated, span.style));
        break;
    }
    Line::from(new_spans)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileTab {
    pub path: Option<PathBuf>,
}

pub struct EditorPane {
    tabs: Vec<FileTab>,
    active_tab: usize,
    /// Path whose content is currently in `highlighted` (`None` = welcome empty state).
    displayed_path: Option<PathBuf>,
    highlighted: Vec<Line<'static>>,
    vertical_scroll: usize,
    scroll_by_path: HashMap<PathBuf, usize>,
    scroll_path_order: VecDeque<PathBuf>,
    load_error: Option<String>,
    truncated: bool,
    viewport_height: usize,
    unified_bridge_tx: Option<
        futures::channel::mpsc::UnboundedSender<
            crate::quorp::tui::bridge::TuiToBackendRequest,
        >,
    >,
    bridge_load_pending: bool,
}

impl EditorPane {
    pub fn new() -> Self {
        Self::with_buffer_bridge(None)
    }

    pub(crate) fn with_buffer_bridge(
        unified_bridge_tx: Option<
            futures::channel::mpsc::UnboundedSender<
                crate::quorp::tui::bridge::TuiToBackendRequest,
            >,
        >,
    ) -> Self {
        Self {
            tabs: vec![FileTab { path: None }],
            active_tab: 0,
            displayed_path: None,
            highlighted: Vec::new(),
            vertical_scroll: 0,
            scroll_by_path: HashMap::new(),
            scroll_path_order: VecDeque::new(),
            load_error: None,
            truncated: false,
            viewport_height: 24,
            unified_bridge_tx,
            bridge_load_pending: false,
        }
    }

    pub(crate) fn apply_editor_pane_buffer_snapshot(
        &mut self,
        path: Option<PathBuf>,
        lines: Vec<Line<'static>>,
        error: Option<String>,
        truncated: bool,
    ) {
        let active = self.active_file_path();
        let applies = match (&path, active) {
            (None, None) => true,
            (Some(p), Some(a)) => paths_equal(Some(p.as_path()), Some(a)),
            _ => false,
        };
        if !applies {
            return;
        }
        self.bridge_load_pending = false;
        self.highlighted = lines;
        self.load_error = error;
        self.truncated = truncated;
    }

    /// Replace open file tabs for deterministic PNG regression (`new_for_prismforge_regression`).
    pub fn set_regression_file_tabs(&mut self, paths: Vec<PathBuf>, active: usize, project_root: &Path) {
        if paths.is_empty() {
            self.tabs = vec![FileTab { path: None }];
            self.active_tab = 0;
        } else {
            self.tabs = paths
                .into_iter()
                .map(|path| FileTab { path: Some(path) })
                .collect();
            self.active_tab = active.min(self.tabs.len().saturating_sub(1));
        }
        self.ensure_active_loaded(project_root);
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_tab_index(&self) -> usize {
        self.active_tab
    }

    fn active_file_path(&self) -> Option<&Path> {
        self.tabs
            .get(self.active_tab)
            .and_then(|t| t.path.as_deref())
    }

    /// Open the tree-selected file in a tab, or focus an existing tab for that path.
    pub fn sync_tree_selection(&mut self, selected: Option<&Path>, _project_root: &Path) {
        let Some(path) = selected else {
            return;
        };
        if let Some(index) = self
            .tabs
            .iter()
            .position(|t| t.path.as_deref() == Some(path))
        {
            self.active_tab = index;
            return;
        }
        self.tabs.push(FileTab {
            path: Some(path.to_path_buf()),
        });
        self.active_tab = self.tabs.len().saturating_sub(1);
    }

    pub fn ensure_active_loaded(&mut self, project_root: &Path) {
        let target = self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.path.clone());
        if paths_equal(self.displayed_path.as_deref(), target.as_deref()) {
            return;
        }
        if let Some(ref old) = self.displayed_path {
            self.remember_scroll_offset(old.clone(), self.vertical_scroll);
        }
        self.displayed_path = target.clone();
        self.load_error = None;
        self.truncated = false;
        self.highlighted.clear();
        self.bridge_load_pending = false;

        let Some(ref path) = target else {
            self.vertical_scroll = 0;
            if let Some(tx) = &self.unified_bridge_tx {
                let _ = tx.unbounded_send(
                    crate::quorp::tui::bridge::TuiToBackendRequest::CloseBuffer,
                );
            }
            return;
        };

        self.vertical_scroll = self.scroll_by_path.get(path).copied().unwrap_or(0);

        if !path_within_project(path, project_root) {
            self.load_error = Some("File path is outside the project root".to_string());
            return;
        }

        if let Some(tx) = &self.unified_bridge_tx {
            self.bridge_load_pending = true;
            if tx
                .unbounded_send(
                    crate::quorp::tui::bridge::TuiToBackendRequest::OpenBuffer(
                        path.clone(),
                    ),
                )
                .is_err()
            {
                self.bridge_load_pending = false;
                self.load_error = Some("Code preview bridge disconnected.".to_string());
            }
            return;
        }

        let (bytes, truncated_read) = match read_file_capped(path) {
            Ok(b) => b,
            Err(e) => {
                self.load_error = Some(e.to_string());
                return;
            }
        };

        if bytes.contains(&0) {
            self.load_error = Some("Binary file (contains NUL bytes)".to_string());
            return;
        }

        self.truncated = truncated_read;

        let source = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => {
                self.load_error = Some("File is not valid UTF-8".to_string());
                return;
            }
        };

        self.highlighted = lines_from_plain_source(&source);
    }

    pub fn leaf_tab_specs(&self, theme: &crate::quorp::tui::theme::Theme) -> Vec<crate::quorp::tui::chrome_v2::LeafTabSpec> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let label = tab
                    .path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("Welcome")
                    .to_string();
                let show_close = !(self.tabs.len() == 1 && tab.path.is_none());
                crate::quorp::tui::chrome_v2::LeafTabSpec {
                    label,
                    active: i == self.active_tab,
                    icon: Some(theme.glyphs.file_icon),
                    show_close,
                }
            })
            .collect()
    }

    pub fn draw_tab_strip(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        strip: ratatui::layout::Rect,
        theme: &crate::quorp::tui::theme::Theme,
    ) -> (Vec<crate::quorp::tui::chrome_v2::LeafTabLayoutCell>, usize) {
        let specs = self.leaf_tab_specs(theme);
        let (cells, overflow) = crate::quorp::tui::chrome_v2::layout_leaf_tabs(strip, &specs);
        crate::quorp::tui::chrome_v2::render_leaf_tabs_laid_out(
            buf,
            strip,
            &cells,
            &specs,
            &theme.palette,
            theme.glyphs.close_icon,
            theme.palette.editor_accent,
        );
        if overflow > 0 {
            crate::quorp::tui::chrome_v2::render_tab_overflow_hint(buf, strip, overflow, &theme.palette);
        }
        (cells, overflow)
    }

    pub fn activate_file_tab(&mut self, index: usize, project_root: &Path) -> bool {
        if index >= self.tabs.len() {
            return false;
        }
        self.active_tab = index;
        self.ensure_active_loaded(project_root);
        true
    }

    pub fn close_file_tab_at(&mut self, index: usize, project_root: &Path) -> bool {
        if index >= self.tabs.len() {
            return false;
        }
        if self.tabs.len() == 1 && self.tabs[0].path.is_none() {
            return false;
        }
        self.tabs.remove(index);
        let new_len = self.tabs.len();
        if self.active_tab > index {
            self.active_tab -= 1;
        } else if index == self.active_tab {
            self.active_tab = self.active_tab.min(new_len.saturating_sub(1));
        }
        self.ensure_active_loaded(project_root);
        true
    }

    pub fn close_all_file_tabs(&mut self, project_root: &Path) {
        self.tabs = vec![FileTab { path: None }];
        self.active_tab = 0;
        self.displayed_path = None;
        self.ensure_active_loaded(project_root);
    }

    pub fn cycle_file_tab(&mut self, delta: isize, project_root: &Path) {
        if self.tabs.is_empty() {
            return;
        }
        let len = self.tabs.len() as isize;
        self.active_tab = (self.active_tab as isize + delta).rem_euclid(len) as usize;
        self.ensure_active_loaded(project_root);
    }

    fn remember_scroll_offset(&mut self, path: PathBuf, offset: usize) {
        let cap = MAX_SCROLL_PATH_ENTRIES;
        if !self.scroll_by_path.contains_key(&path) {
            while self.scroll_by_path.len() >= cap {
                let Some(old) = self.scroll_path_order.pop_front() else {
                    break;
                };
                self.scroll_by_path.remove(&old);
            }
            self.scroll_path_order.push_back(path.clone());
        }
        self.scroll_by_path.insert(path, offset);
    }

    /// Updates tabs from the file tree selection and loads the active tab (tests and callers).
    pub fn sync_from_selected_file(&mut self, path: Option<&Path>, project_root: &Path) {
        self.sync_tree_selection(path, project_root);
        self.ensure_active_loaded(project_root);
    }

    fn line_count(&self) -> usize {
        if self.load_error.is_some() {
            return 0;
        }
        self.highlighted.len()
    }

    fn clamp_scroll(&mut self, viewport_lines: usize) {
        let total = self.line_count();
        if total == 0 {
            self.vertical_scroll = 0;
            return;
        }
        if viewport_lines == 0 {
            return;
        }
        let max_scroll = if viewport_lines >= total {
            0
        } else {
            total - viewport_lines
        };
        if self.vertical_scroll > max_scroll {
            self.vertical_scroll = max_scroll;
        }
    }

    fn persist_scroll(&mut self) {
        if let Some(ref path) = self.displayed_path {
            self.remember_scroll_offset(path.clone(), self.vertical_scroll);
        }
    }

    fn scroll_up(&mut self, delta: usize) {
        self.vertical_scroll = self.vertical_scroll.saturating_sub(delta);
        self.persist_scroll();
    }

    fn scroll_down(&mut self, delta: usize, viewport_lines: usize) {
        let total = self.line_count();
        let max_scroll = if total <= viewport_lines || total == 0 {
            0
        } else {
            total - viewport_lines
        };
        self.vertical_scroll = (self.vertical_scroll + delta).min(max_scroll);
        self.persist_scroll();
    }

    pub fn handle_key_event(&mut self, key: &KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT)
        {
            return false;
        }
        let viewport_lines = self.viewport_height;
        match key.code {
            KeyCode::Up => {
                self.scroll_up(1);
                true
            }
            KeyCode::Down => {
                self.scroll_down(1, viewport_lines);
                true
            }
            KeyCode::PageUp => {
                let step = viewport_lines.saturating_sub(1).max(1);
                self.scroll_up(step);
                true
            }
            KeyCode::PageDown => {
                let step = viewport_lines.saturating_sub(1).max(1);
                self.scroll_down(step, viewport_lines);
                true
            }
            KeyCode::Home => {
                self.vertical_scroll = 0;
                self.persist_scroll();
                true
            }
            KeyCode::End => {
                let total = self.line_count();
                let max_scroll = if total <= viewport_lines || total == 0 {
                    0
                } else {
                    total - viewport_lines
                };
                self.vertical_scroll = max_scroll;
                self.persist_scroll();
                true
            }
            _ => false,
        }
    }

    pub fn render(&mut self, frame: &mut Frame<'_>, inner: Rect, focused: bool, theme: &crate::quorp::tui::theme::Theme) {
        if inner.height == 0 || inner.width == 0 {
            return;
        }
        self.viewport_height = inner.height as usize;

        let banner_lines = usize::from(self.truncated);
        let code_viewport = self.viewport_height.saturating_sub(banner_lines);
        if code_viewport == 0 {
            if self.truncated {
                let banner = Paragraph::new(Line::from(Span::styled(
                    TRUNCATION_BANNER,
                    Style::default().fg(Color::Yellow),
                )));
                frame.render_widget(banner, inner);
            }
            return;
        }
        self.clamp_scroll(code_viewport);

        if let Some(ref err) = self.load_error {
            let line = Line::from(Span::styled(err.clone(), Style::default().fg(Color::Red)));
            frame.render_widget(Paragraph::new(line), inner);
            return;
        }

        if self.bridge_load_pending
            && self.highlighted.is_empty()
            && self.active_file_path().is_some()
        {
            let message = Paragraph::new(Line::from(Span::styled(
                "Loading…",
                Style::default().fg(theme.palette.text_muted),
            )));
            frame.render_widget(message, inner);
            return;
        }

        if self.active_file_path().is_none() {
            let message = Paragraph::new(Line::from(
                "Select a file in the tree (press Enter on a file).",
            ));
            frame.render_widget(message, inner);
            return;
        }

        let code_area = if self.truncated {
            let vertical = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(inner);
            let banner = Paragraph::new(Line::from(Span::styled(
                TRUNCATION_BANNER,
                Style::default().fg(Color::Yellow),
            )));
            frame.render_widget(banner, vertical[0]);
            vertical[1]
        } else {
            inner
        };

        if code_area.height == 0 || code_area.width == 0 {
            return;
        }

        let total_lines = self.highlighted.len();
        let line_digits = total_lines.max(1).to_string().len().max(4);
        let gutter_width = line_digits as u16;

        let show_scrollbar = total_lines > code_viewport && code_area.width > 1;
        let content_width = if show_scrollbar {
            code_area.width.saturating_sub(1)
        } else {
            code_area.width
        };

        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(gutter_width), Constraint::Min(1)])
            .split(Rect {
                x: code_area.x,
                y: code_area.y,
                width: content_width,
                height: code_area.height,
            });

        let gutter_area = horizontal[0];
        let text_area = horizontal[1];

        let code_column_width = text_area.width as usize;

        let start = self.vertical_scroll;
        let end = (start + code_viewport).min(total_lines);
        let mut code_lines: Vec<Line<'static>> = Vec::new();
        let mut gutter_text: Vec<Line> = Vec::new();

        for line_index in start..end {
            let Some(line) = self.highlighted.get(line_index) else {
                continue;
            };
            code_lines.push(truncate_line_to_width(line.clone(), code_column_width));
            let gutter_line = format!("{:>width$}", line_index + 1, width = line_digits);
            gutter_text.push(Line::from(Span::styled(gutter_line, gutter_style(focused, theme))));
        }

        while gutter_text.len() < code_viewport {
            gutter_text.push(Line::from(""));
        }
        while code_lines.len() < code_viewport {
            code_lines.push(Line::from(""));
        }

        frame.render_widget(Paragraph::new(gutter_text), gutter_area);
        frame.render_widget(Paragraph::new(code_lines), text_area);

        if show_scrollbar {
            let scrollbar_area = Rect {
                x: code_area.x + content_width,
                y: code_area.y,
                width: 1,
                height: code_area.height,
            };
            let mut scrollbar_state = ScrollbarState::new(total_lines.max(1))
                .position(self.vertical_scroll)
                .viewport_content_length(code_viewport);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None);
            frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
        }
    }

    pub fn render_in_leaf(
        &mut self,
        buf: &mut ratatui::buffer::Buffer,
        rects: &crate::quorp::tui::workbench::LeafRects,
        mode: EditorRenderMode,
        focused: bool,
        theme: &crate::quorp::tui::theme::Theme,
    ) {
        use ratatui::widgets::{Widget, StatefulWidget};

        crate::quorp::tui::paint::fill_rect(buf, rects.body, theme.palette.editor_bg);
        crate::quorp::tui::paint::fill_rect(buf, rects.scrollbar, theme.palette.editor_bg);

        if rects.body.height == 0 || rects.body.width == 0 {
            return;
        }
        self.viewport_height = rects.body.height as usize;

        if let Some(ref err) = self.load_error {
            let line = Line::from(Span::styled(err.clone(), Style::default().fg(theme.palette.danger_orange)));
            Paragraph::new(line).render(rects.body, buf);
            return;
        }

        if self.bridge_load_pending
            && self.highlighted.is_empty()
            && self.active_file_path().is_some()
        {
            let message = Paragraph::new(Line::from(Span::styled(
                "Loading…",
                Style::default().fg(theme.palette.text_muted),
            )));
            message.render(rects.body, buf);
            return;
        }

        if self.active_file_path().is_none() || mode == EditorRenderMode::EmptyState {
            let message = Paragraph::new(Line::from(
                "Select a file in the tree (press Enter on a file).",
            ));
            message.render(rects.body, buf);
            return;
        }

        let banner_lines = usize::from(self.truncated);
        let code_viewport = self.viewport_height.saturating_sub(banner_lines);
        if code_viewport == 0 {
            if self.truncated {
                let banner = Paragraph::new(Line::from(Span::styled(
                    TRUNCATION_BANNER,
                    Style::default().fg(theme.palette.warning_yellow),
                )));
                banner.render(rects.body, buf);
            }
            return;
        }

        self.clamp_scroll(code_viewport);

        let code_area = if self.truncated {
            let banner_rect = ratatui::layout::Rect {
                x: rects.body.x,
                y: rects.body.y,
                width: rects.body.width,
                height: 1,
            };
            let banner = Paragraph::new(Line::from(Span::styled(
                TRUNCATION_BANNER,
                Style::default().fg(theme.palette.warning_yellow),
            )));
            banner.render(banner_rect, buf);
            ratatui::layout::Rect {
                x: rects.body.x,
                y: rects.body.y + 1,
                width: rects.body.width,
                height: rects.body.height.saturating_sub(1),
            }
        } else {
            rects.body
        };

        if code_area.height == 0 || code_area.width == 0 {
            return;
        }

        let total_lines = self.highlighted.len();
        let show_scrollbar = total_lines > code_viewport;

        let start = self.vertical_scroll;
        let end = (start + code_viewport).min(total_lines);

        if mode == EditorRenderMode::MarkdownPreview {
            let mut md_lines: Vec<Line<'static>> = Vec::new();
            for line_index in start..end {
                let Some(line) = self.highlighted.get(line_index) else { continue };
                let mut raw = String::new();
                for span in &line.spans {
                    raw.push_str(&span.content);
                }
                
                let text = raw.trim_end_matches('\n');
                let l = if text.starts_with("# ") {
                    Line::from(Span::styled(text.to_string(), Style::default().fg(theme.palette.text).add_modifier(Modifier::BOLD)))
                } else if text.starts_with('>') || text.starts_with("---") {
                    Line::from(Span::styled(text.to_string(), Style::default().fg(theme.palette.text_muted)))
                } else if text.starts_with("- [ ]") || text.starts_with("- [x]") || text.starts_with("* [ ]") || text.starts_with("* [x]") {
                    let is_checked = text.contains("[x]");
                    let box_str = if is_checked { "[x]" } else { "[ ]" };
                    let rest = text.replacen(box_str, "", 1);
                    let color = if is_checked { theme.palette.success_green } else { theme.palette.text_muted };
                    Line::from(vec![
                        Span::styled(box_str.to_string(), Style::default().fg(color)),
                        Span::styled(rest, Style::default().fg(theme.palette.text)),
                    ])
                } else if text.starts_with("- ") || text.starts_with("* ") {
                    Line::from(Span::styled(format!("  {}", text), Style::default().fg(theme.palette.text)))
                } else {
                    Line::from(Span::styled(text.to_string(), Style::default().fg(theme.palette.text)))
                };
                md_lines.push(l);
            }
            Paragraph::new(md_lines).render(code_area, buf);
        } else {
            let line_digits = total_lines.max(1).to_string().len().max(4);
            let gutter_width = line_digits as u16;
            
            let gutter_area = ratatui::layout::Rect {
                x: code_area.x,
                y: code_area.y,
                width: gutter_width,
                height: code_area.height,
            };
            let text_area = ratatui::layout::Rect {
                x: code_area.x + gutter_width,
                y: code_area.y,
                width: code_area.width.saturating_sub(gutter_width),
                height: code_area.height,
            };

            let code_column_width = text_area.width as usize;

            let mut code_lines: Vec<Line<'static>> = Vec::new();
            let mut gutter_text: Vec<Line> = Vec::new();

            for line_index in start..end {
                let Some(line) = self.highlighted.get(line_index) else { continue };
                code_lines.push(truncate_line_to_width(line.clone(), code_column_width));
                let gutter_line = format!("{:>width$}", line_index + 1, width = line_digits);
                gutter_text.push(Line::from(Span::styled(gutter_line, gutter_style(focused, theme))));
            }

            Paragraph::new(gutter_text).render(gutter_area, buf);
            Paragraph::new(code_lines).render(text_area, buf);
        }

        if show_scrollbar && rects.scrollbar.width > 0 {
            let mut scrollbar_state = ScrollbarState::new(total_lines.max(1))
                .position(self.vertical_scroll)
                .viewport_content_length(code_viewport);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some(" "))
                .thumb_symbol(" ")
                .style(Style::default().fg(theme.palette.scrollbar_thumb).bg(theme.palette.scrollbar_track));
            StatefulWidget::render(scrollbar, rects.scrollbar, buf, &mut scrollbar_state);
        }
    }
}

fn paths_equal(active: Option<&Path>, path: Option<&Path>) -> bool {
    match (active, path) {
        (Some(a), Some(b)) => a == b,
        (None, None) => true,
        _ => false,
    }
}

fn read_file_capped(path: &Path) -> Result<(Vec<u8>, bool), std::io::Error> {
    use std::fs::File;
    use std::io::Read;

    let file = File::open(path)?;
    let file_len = file.metadata()?.len() as usize;
    let mut buf = Vec::new();
    file.take(MAX_READ_BYTES as u64).read_to_end(&mut buf)?;
    let truncated = file_len > MAX_READ_BYTES;
    Ok((buf, truncated))
}

#[cfg(test)]
impl EditorPane {
    pub(crate) fn vertical_scroll_for_test(&self) -> usize {
        self.vertical_scroll
    }

    pub(crate) fn scroll_path_count_for_test(&self) -> usize {
        self.scroll_by_path.len()
    }

    pub(crate) fn scroll_contains_path_for_test(&self, path: &Path) -> bool {
        self.scroll_by_path.contains_key(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_under_root_accepts_nested_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let file = root.join("deep").join("x.rs");
        std::fs::create_dir_all(file.parent().unwrap()).expect("mkdir");
        std::fs::write(&file, "fn main() {}\n").expect("write");
        assert!(path_within_project(&file, root));
    }

    #[test]
    fn path_under_root_accepts_file_at_root_depth() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let file = root.join("x.txt");
        std::fs::write(&file, "hi\n").expect("write");
        assert!(path_within_project(&file, root));
    }

    #[test]
    fn path_outside_root_rejected() {
        let root_dir = tempfile::tempdir().expect("tempdir");
        let other_dir = tempfile::tempdir().expect("tempdir");
        let outside = other_dir.path().join("outside.txt");
        std::fs::write(&outside, "x").expect("write");
        assert!(!path_within_project(outside.as_path(), root_dir.path()));
    }

    #[test]
    fn sync_rejects_path_outside_root() {
        let root_dir = tempfile::tempdir().expect("tempdir");
        let other_dir = tempfile::tempdir().expect("tempdir");
        let outside = other_dir.path().join("outside.rs");
        std::fs::write(&outside, "fn x() {}\n").expect("write");
        let mut preview = EditorPane::new();
        preview.sync_from_selected_file(Some(outside.as_path()), root_dir.path());
        assert!(preview.load_error.is_some());
        assert!(preview.highlighted.is_empty());
    }

    #[test]
    fn sync_rejects_binary_nul() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let file = root.join("bin.dat");
        std::fs::write(&file, [b'a', 0, b'b']).expect("write");
        let mut preview = EditorPane::new();
        preview.sync_from_selected_file(Some(file.as_path()), root);
        assert!(
            preview
                .load_error
                .as_ref()
                .is_some_and(|e: &String| e.contains("NUL"))
        );
    }

    #[test]
    fn sync_rejects_invalid_utf8() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let file = root.join("bad.txt");
        std::fs::write(&file, [0xff, 0xfe, 0xfd]).expect("write");
        let mut preview = EditorPane::new();
        preview.sync_from_selected_file(Some(file.as_path()), root);
        assert!(
            preview
                .load_error
                .as_ref()
                .is_some_and(|e: &String| e.contains("UTF-8"))
        );
    }

    #[test]
    fn sync_loads_json() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("data.json");
        std::fs::write(&path, "{\"a\": true}\n").expect("write");
        let mut preview = EditorPane::new();
        preview.sync_from_selected_file(Some(path.as_path()), temp.path());
        assert!(preview.load_error.is_none());
        assert!(!preview.highlighted.is_empty());
    }

    #[test]
    fn gutter_style_differs_when_unfocused() {
        let theme = crate::quorp::tui::theme::Theme::antigravity();
        assert_ne!(gutter_style(true, &theme), gutter_style(false, &theme));
        assert!(!gutter_style(true, &theme).add_modifier.contains(Modifier::DIM));
        assert!(gutter_style(false, &theme).add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn sync_loads_rust_plain_text() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("main.rs");
        std::fs::write(&path, "fn main() {\n    let x = 1;\n}\n").expect("write");
        let mut preview = EditorPane::new();
        preview.sync_from_selected_file(Some(path.as_path()), temp.path());
        assert!(preview.load_error.is_none());
        assert!(!preview.highlighted.is_empty());
        let mut joined = String::new();
        for line in &preview.highlighted {
            for span in &line.spans {
                joined.push_str(span.content.as_ref());
            }
            joined.push('\n');
        }
        assert!(joined.contains("fn main()"));
    }

    #[test]
    fn scroll_down_moves_offset() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("many.rs");
        let mut body = String::new();
        for i in 0..40 {
            body.push_str(&format!("let _{i} = {i};\n"));
        }
        std::fs::write(&path, body).expect("write");
        let mut preview = EditorPane::new();
        preview.sync_from_selected_file(Some(path.as_path()), temp.path());
        preview.viewport_height = 5;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert!(preview.handle_key_event(&key));
        assert!(preview.vertical_scroll_for_test() > 0);
    }

    #[test]
    fn scroll_restored_when_switching_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let a = root.join("a.rs");
        let b = root.join("b.rs");
        std::fs::write(&a, "a\n".repeat(30)).expect("write");
        std::fs::write(&b, "b\n".repeat(30)).expect("write");
        let mut preview = EditorPane::new();
        preview.sync_from_selected_file(Some(a.as_path()), root);
        preview.viewport_height = 5;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for _ in 0..3 {
            preview.handle_key_event(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        let scroll_a = preview.vertical_scroll_for_test();
        preview.sync_from_selected_file(Some(b.as_path()), root);
        preview.sync_from_selected_file(Some(a.as_path()), root);
        assert_eq!(preview.vertical_scroll_for_test(), scroll_a);
    }

    #[test]
    fn scroll_memory_evicts_at_cap() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let mut preview = EditorPane::new();
        preview.viewport_height = 3;
        for index in 0..=MAX_SCROLL_PATH_ENTRIES {
            let path = root.join(format!("f{index}.rs"));
            std::fs::write(&path, "x\n".repeat(20)).expect("write");
            preview.sync_from_selected_file(Some(path.as_path()), root);
            for _ in 0..2 {
                preview.handle_key_event(&crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Down,
                    crossterm::event::KeyModifiers::NONE,
                ));
            }
        }
        assert_eq!(
            preview.scroll_path_count_for_test(),
            MAX_SCROLL_PATH_ENTRIES
        );
        let first_path = root.join("f0.rs");
        assert!(
            !preview.scroll_contains_path_for_test(&first_path),
            "FIFO evicts oldest"
        );
    }

    #[test]
    fn home_scrolls_to_top() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let path = root.join("long.rs");
        let mut body = String::new();
        for i in 0..40 {
            body.push_str(&format!("let _{i} = {i};\n"));
        }
        std::fs::write(&path, body).expect("write");
        let mut preview = EditorPane::new();
        preview.sync_from_selected_file(Some(path.as_path()), root);
        preview.viewport_height = 5;
        for _ in 0..10 {
            preview.handle_key_event(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        assert!(preview.vertical_scroll_for_test() > 0);
        preview.handle_key_event(&KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(preview.vertical_scroll_for_test(), 0);
    }

    #[test]
    fn end_scrolls_to_bottom() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let path = root.join("long.rs");
        let mut body = String::new();
        for i in 0..40 {
            body.push_str(&format!("let _{i} = {i};\n"));
        }
        std::fs::write(&path, body).expect("write");
        let mut preview = EditorPane::new();
        preview.sync_from_selected_file(Some(path.as_path()), root);
        preview.viewport_height = 5;
        assert_eq!(preview.vertical_scroll_for_test(), 0);
        preview.handle_key_event(&KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert!(preview.vertical_scroll_for_test() > 0);
    }

    #[test]
    fn tree_selection_opens_distinct_file_tabs_and_reselect_focuses() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let a = root.join("one.rs");
        let b = root.join("two.rs");
        std::fs::write(&a, "//").expect("write");
        std::fs::write(&b, "//").expect("write");
        let mut preview = EditorPane::new();
        preview.sync_tree_selection(Some(a.as_path()), root);
        preview.sync_tree_selection(Some(b.as_path()), root);
        assert_eq!(preview.tab_count(), 3);
        preview.sync_tree_selection(Some(a.as_path()), root);
        assert_eq!(preview.tab_count(), 3);
        assert_eq!(preview.active_tab_index(), 1);
    }

    #[test]
    fn file_tab_close_and_activate() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let a = root.join("a.rs");
        let b = root.join("b.rs");
        std::fs::write(&a, "//").expect("write");
        std::fs::write(&b, "//").expect("write");
        let mut preview = EditorPane::new();
        preview.sync_tree_selection(Some(a.as_path()), root);
        preview.sync_tree_selection(Some(b.as_path()), root);
        assert!(preview.activate_file_tab(2, root));
        assert_eq!(preview.active_tab_index(), 2);
        assert!(preview.close_file_tab_at(1, root));
        assert_eq!(preview.tab_count(), 2);
    }
}

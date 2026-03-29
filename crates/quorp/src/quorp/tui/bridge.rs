#![allow(unused)]
use std::path::PathBuf;
use gpui::{AppContext, AsyncApp, EntityId, Entity, Task, FontStyle, FontWeight, HighlightStyle, Subscription};

use project::{EntryKind, Project};
use language::{Buffer, BufferEvent, BufferSnapshot, Chunk, DiagnosticSeverity};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use gpui::{Bounds, Keystroke, Modifiers, Point, Size, px};
use itertools::Itertools as _;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use terminal::{
    IndexedCell, Terminal, TerminalBounds, TerminalContent,
    alacritty_terminal::{
        term::cell::Flags,
        vte::ansi::Color as AnsiColor,
        vte::ansi::NamedColor,
    },
};
// Helper functions inlined to remove dependency on terminal_view and ui crates.
use unicode_width::UnicodeWidthChar as _;

use theme::{ActiveTheme as _, SyntaxTheme};
use std::mem;

const TUI_TERMINAL_MIN_CONTRAST: f32 = 3.0;

use std::str::FromStr as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use fs::Fs;
use language_model::{
    CompletionIntent, LanguageModelCompletionEvent, LanguageModelRegistry, LanguageModelRequest,
    LanguageModelRequestMessage, LanguageModelToolResult, LanguageModelToolResultContent,
    MessageContent, Role, SelectedModel,
};
use settings::{
    LanguageModelProviderSetting, LanguageModelSelection, update_settings_file,
};
use crate::quorp::tui::chat::ChatUiEvent;
use crate::quorp::tui::tui_backend::TuiBackend;
use crate::quorp::tui::tui_tool_runtime::execute_tui_tool_call;



fn hsla_to_ratatui_color(h: gpui::Hsla) -> ratatui::style::Color {
    let r: gpui::Rgba = h.into();
    ratatui::style::Color::Rgb(
        (r.r * 255.).clamp(0., 255.) as u8,
        (r.g * 255.).clamp(0., 255.) as u8,
        (r.b * 255.).clamp(0., 255.) as u8,
    )
}

pub fn is_blank(cell: &IndexedCell) -> bool {
    if cell.c != ' ' {
        return false;
    }

    if cell.bg != AnsiColor::Named(NamedColor::Background) {
        return false;
    }

    if cell.hyperlink().is_some() {
        return false;
    }

    if cell
        .flags
        .intersects(Flags::ALL_UNDERLINES | Flags::INVERSE | Flags::STRIKEOUT)
    {
        return false;
    }

    true
}

pub fn convert_color(fg: &AnsiColor, theme: &theme::Theme) -> gpui::Hsla {
    let colors = theme.colors();
    match fg {
        AnsiColor::Named(n) => match n {
            NamedColor::Black => colors.terminal_ansi_black,
            NamedColor::Red => colors.terminal_ansi_red,
            NamedColor::Green => colors.terminal_ansi_green,
            NamedColor::Yellow => colors.terminal_ansi_yellow,
            NamedColor::Blue => colors.terminal_ansi_blue,
            NamedColor::Magenta => colors.terminal_ansi_magenta,
            NamedColor::Cyan => colors.terminal_ansi_cyan,
            NamedColor::White => colors.terminal_ansi_white,
            NamedColor::BrightBlack => colors.terminal_ansi_bright_black,
            NamedColor::BrightRed => colors.terminal_ansi_bright_red,
            NamedColor::BrightGreen => colors.terminal_ansi_bright_green,
            NamedColor::BrightYellow => colors.terminal_ansi_bright_yellow,
            NamedColor::BrightBlue => colors.terminal_ansi_bright_blue,
            NamedColor::BrightMagenta => colors.terminal_ansi_bright_magenta,
            NamedColor::BrightCyan => colors.terminal_ansi_bright_cyan,
            NamedColor::BrightWhite => colors.terminal_ansi_bright_white,
            NamedColor::Foreground => colors.terminal_foreground,
            NamedColor::Background => colors.terminal_ansi_background,
            NamedColor::Cursor => theme.players().local().cursor,
            NamedColor::DimBlack => colors.terminal_ansi_dim_black,
            NamedColor::DimRed => colors.terminal_ansi_dim_red,
            NamedColor::DimGreen => colors.terminal_ansi_dim_green,
            NamedColor::DimYellow => colors.terminal_ansi_dim_yellow,
            NamedColor::DimBlue => colors.terminal_ansi_dim_blue,
            NamedColor::DimMagenta => colors.terminal_ansi_dim_magenta,
            NamedColor::DimCyan => colors.terminal_ansi_dim_cyan,
            NamedColor::DimWhite => colors.terminal_ansi_dim_white,
            NamedColor::BrightForeground => colors.terminal_bright_foreground,
            NamedColor::DimForeground => colors.terminal_dim_foreground,
        },
        AnsiColor::Spec(rgb) => terminal::rgba_color(rgb.r, rgb.g, rgb.b),
        AnsiColor::Indexed(i) => terminal::get_color_at_index(*i as usize, theme),
    }
}

// APCA (Accessible Perceptual Contrast Algorithm) constants
struct APCAConstants {
    main_trc: f32,
    s_rco: f32,
    s_gco: f32,
    s_bco: f32,
    norm_bg: f32,
    norm_txt: f32,
    rev_txt: f32,
    rev_bg: f32,
    blk_thrs: f32,
    blk_clmp: f32,
    scale_bow: f32,
    scale_wob: f32,
    lo_bow_offset: f32,
    lo_wob_offset: f32,
    delta_y_min: f32,
    lo_clip: f32,
}

impl Default for APCAConstants {
    fn default() -> Self {
        Self {
            main_trc: 2.4,
            s_rco: 0.2126729,
            s_gco: 0.7151522,
            s_bco: 0.0721750,
            norm_bg: 0.56,
            norm_txt: 0.57,
            rev_txt: 0.62,
            rev_bg: 0.65,
            blk_thrs: 0.022,
            blk_clmp: 1.414,
            scale_bow: 1.14,
            scale_wob: 1.14,
            lo_bow_offset: 0.027,
            lo_wob_offset: 0.027,
            delta_y_min: 0.0005,
            lo_clip: 0.1,
        }
    }
}

pub fn apca_contrast(text_color: Hsla, background_color: Hsla) -> f32 {
    let constants = APCAConstants::default();
    let text_y = srgb_to_y(text_color, &constants);
    let bg_y = srgb_to_y(background_color, &constants);
    let text_y_clamped = if text_y > constants.blk_thrs { text_y } else { text_y + (constants.blk_thrs - text_y).powf(constants.blk_clmp) };
    let bg_y_clamped = if bg_y > constants.blk_thrs { bg_y } else { bg_y + (constants.blk_thrs - bg_y).powf(constants.blk_clmp) };
    if (bg_y_clamped - text_y_clamped).abs() < constants.delta_y_min { return 0.0; }
    let sapc;
    let output_contrast;
    if bg_y_clamped > text_y_clamped {
        sapc = (bg_y_clamped.powf(constants.norm_bg) - text_y_clamped.powf(constants.norm_txt)) * constants.scale_bow;
        output_contrast = if sapc < constants.lo_clip { 0.0 } else { sapc - constants.lo_bow_offset };
    } else {
        sapc = (bg_y_clamped.powf(constants.rev_bg) - text_y_clamped.powf(constants.rev_txt)) * constants.scale_wob;
        output_contrast = if sapc > -constants.lo_clip { 0.0 } else { sapc + constants.lo_wob_offset };
    }
    output_contrast * 100.0
}

fn srgb_to_y(color: Hsla, constants: &APCAConstants) -> f32 {
    let rgba = color.to_rgb();
    let r_linear = (rgba.r).powf(constants.main_trc);
    let g_linear = (rgba.g).powf(constants.main_trc);
    let b_linear = (rgba.b).powf(constants.main_trc);
    constants.s_rco * r_linear + constants.s_gco * g_linear + constants.s_bco * b_linear
}

pub fn ensure_minimum_contrast(foreground: Hsla, background: Hsla, minimum_apca_contrast: f32) -> Hsla {
    if minimum_apca_contrast <= 0.0 { return foreground; }
    let current_contrast = apca_contrast(foreground, background).abs();
    if current_contrast >= minimum_apca_contrast { return foreground; }
    let adjusted = adjust_lightness_for_contrast(foreground, background, minimum_apca_contrast);
    if apca_contrast(adjusted, background).abs() >= minimum_apca_contrast { return adjusted; }
    let desaturated = adjust_lightness_and_saturation_for_contrast(foreground, background, minimum_apca_contrast);
    if apca_contrast(desaturated, background).abs() >= minimum_apca_contrast { return desaturated; }
    let black = Hsla { h: 0.0, s: 0.0, l: 0.0, a: foreground.a };
    let white = Hsla { h: 0.0, s: 0.0, l: 1.0, a: foreground.a };
    if apca_contrast(white, background).abs() > apca_contrast(black, background).abs() { white } else { black }
}

fn adjust_lightness_for_contrast(foreground: Hsla, background: Hsla, minimum_apca_contrast: f32) -> Hsla {
    let bg_luminance = srgb_to_y(background, &APCAConstants::default());
    let should_go_darker = bg_luminance > 0.5;
    let mut low = if should_go_darker { 0.0 } else { foreground.l };
    let mut high = if should_go_darker { foreground.l } else { 1.0 };
    let mut best_l = foreground.l;
    for _ in 0..20 {
        let mid = (low + high) / 2.0;
        let test_color = Hsla { h: foreground.h, s: foreground.s, l: mid, a: foreground.a };
        let contrast = apca_contrast(test_color, background).abs();
        if contrast >= minimum_apca_contrast {
            best_l = mid;
            if should_go_darker { low = mid; } else { high = mid; }
        } else if should_go_darker { high = mid; } else { low = mid; }
        if (contrast - minimum_apca_contrast).abs() < 1.0 { best_l = mid; break; }
    }
    Hsla { h: foreground.h, s: foreground.s, l: best_l, a: foreground.a }
}

fn adjust_lightness_and_saturation_for_contrast(foreground: Hsla, background: Hsla, minimum_apca_contrast: f32) -> Hsla {
    let saturation_steps = [1.0, 0.8, 0.6, 0.4, 0.2, 0.0];
    for &sat_multiplier in &saturation_steps {
        let test_color = Hsla { h: foreground.h, s: foreground.s * sat_multiplier, l: foreground.l, a: foreground.a };
        let adjusted = adjust_lightness_for_contrast(test_color, background, minimum_apca_contrast);
        if apca_contrast(adjusted, background).abs() >= minimum_apca_contrast { return adjusted; }
    }
    Hsla { h: foreground.h, s: 0.0, l: foreground.l, a: foreground.a }
}

fn hsla_to_ratatui_color(h: gpui::Hsla) -> ratatui::style::Color {
    let r: gpui::Rgba = h.into();
    ratatui::style::Color::Rgb(
        (r.r * 255.).clamp(0., 255.) as u8,
        (r.g * 255.).clamp(0., 255.) as u8,
        (r.b * 255.).clamp(0., 255.) as u8,
    )
}

fn chunk_to_ratatui_style(
    chunk: &Chunk<'_>,
    syntax_theme: &SyntaxTheme,
    default_fg: ratatui::style::Color,
) -> ratatui::style::Style {
    use ratatui::style::{Color, Modifier, Style};
    let hl = chunk
        .syntax_highlight_id
        .and_then(|id| syntax_theme.get(id))
        .copied()
        .unwrap_or(HighlightStyle {
            color: None,
            ..Default::default()
        });

    let mut style = Style::default();
    if let Some(c) = hl.color {
        style = style.fg(hsla_to_ratatui_color(c));
    } else {
        style = style.fg(default_fg);
    }
    if let Some(c) = hl.background_color {
        style = style.bg(hsla_to_ratatui_color(c));
    }
    if hl.font_weight == Some(FontWeight::BOLD) || hl.font_weight == Some(FontWeight::SEMIBOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if hl.font_style == Some(FontStyle::Italic) {
        style = style.add_modifier(Modifier::ITALIC);
    }

    if let Some(sev) = chunk.diagnostic_severity {
        style = match sev {
            DiagnosticSeverity::ERROR => style.fg(Color::Red),
            DiagnosticSeverity::WARNING => style.fg(Color::Yellow),
            DiagnosticSeverity::INFORMATION => style.fg(Color::Cyan),
            DiagnosticSeverity::HINT => style.fg(Color::DarkGray),
            _ => style,
        };
    }

    style
}

fn lines_from_snapshot(
    snapshot: &BufferSnapshot,
    cap: usize,
    syntax_theme: &std::sync::Arc<theme::SyntaxTheme>,
    default_fg: ratatui::style::Color,
) -> Vec<ratatui::text::Line<'static>> {
    let mut lines: Vec<ratatui::text::Line<'static>> = Vec::new();
    let mut current_spans: Vec<ratatui::text::Span<'static>> = Vec::new();

    let flush_span = |text: &str, style: ratatui::style::Style, out: &mut Vec<ratatui::text::Span<'static>>| {
        if text.is_empty() {
            return;
        }
        let expanded = text.replace('\t', "    ");
        out.push(ratatui::text::Span::styled(expanded, style));
    };

    for chunk in snapshot.chunks(0..cap, true) {
        let chunk_style = chunk_to_ratatui_style(&chunk, syntax_theme, default_fg);
        let mut rest = chunk.text;
        while !rest.is_empty() {
            if let Some(pos) = rest.find('\n') {
                let (before, after) = rest.split_at(pos);
                flush_span(before, chunk_style, &mut current_spans);
                lines.push(ratatui::text::Line::from(std::mem::take(&mut current_spans)));
                rest = after.strip_prefix('\n').unwrap_or("");
            } else {
                flush_span(rest, chunk_style, &mut current_spans);
                break;
            }
        }
    }

    if !current_spans.is_empty() {
        lines.push(ratatui::text::Line::from(current_spans));
    }
    if lines.is_empty() {
        lines.push(ratatui::text::Line::default());
    }
    lines
}

fn buffer_refresh_event(event: &BufferEvent) -> bool {
    matches!(
        event,
        BufferEvent::Edited { .. }
            | BufferEvent::Reparsed
            | BufferEvent::Reloaded
            | BufferEvent::LanguageChanged(_)
            | BufferEvent::DiagnosticsUpdated
            | BufferEvent::FileHandleChanged
    )
}

const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;

fn push_snapshot(
    buffer: &Entity<Buffer>,
    path: PathBuf,
    event_tx: &std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
    cx: &mut gpui::App,
) {
    let theme = cx.theme().clone();
    let snapshot = buffer.read(cx).snapshot();
    let total_len = snapshot.len();
    let cap = total_len.min(MAX_SNAPSHOT_BYTES);
    let truncated = total_len > MAX_SNAPSHOT_BYTES;
    
    let syntax_theme = theme.syntax().clone();
    let default_fg = hsla_to_ratatui_color(theme.colors().editor_foreground);
    let lines = lines_from_snapshot(&snapshot, cap, &syntax_theme, default_fg);
    if let Err(e) = event_tx.send(crate::quorp::tui::TuiEvent::UnifiedResponse(
        BackendToTuiResponse::BufferChunk {
            path: Some(path.clone()),
            lines,
            error: None,
            truncated,
        }
    )) {
        log::error!("unified bridge: UI channel closed: {e}");
    }
}

const MAX_TUI_TOOL_ROUNDS: usize = 8;
const TUI_TOOL_OUTPUT_PREVIEW_CHARS: usize = 1200;

/// Batches assistant deltas so the TUI thread is not notified on every token.
struct AssistantTextCoalescer {
    pending: String,
    max_without_flush: usize,
}

impl AssistantTextCoalescer {
    fn new() -> Self {
        Self {
            pending: String::new(),
            max_without_flush: 240,
        }
    }

    fn push_chunk(&mut self, chunk: &str, mut send_delta: impl FnMut(String)) {
        self.pending.push_str(chunk);
        loop {
            let flush_len = self.find_flush_point().unwrap_or(0);
            if flush_len == 0 {
                if self.pending.len() >= self.max_without_flush {
                    let mut end = self.max_without_flush.min(self.pending.len());
                    while end > 0 && !self.pending.is_char_boundary(end) {
                        end -= 1;
                    }
                    let piece = self.pending.drain(..end).collect::<String>();
                    if !piece.is_empty() {
                        send_delta(piece);
                    }
                }
                break;
            }
            let piece = self.pending.drain(..flush_len).collect::<String>();
            if !piece.is_empty() {
                send_delta(piece);
            }
        }
    }

    fn find_flush_point(&self) -> Option<usize> {
        if let Some(i) = self.pending.find('\n') {
            return Some(i + 1);
        }
        for pat in [". ", "! ", "? "] {
            if let Some(i) = self.pending.find(pat) {
                return Some(i + pat.len());
            }
        }
        if let Some(i) = self.pending.find("```") {
            return Some((i + 3).min(self.pending.len()));
        }
        None
    }

    fn flush_all(&mut self, mut send_delta: impl FnMut(String)) {
        if !self.pending.is_empty() {
            send_delta(std::mem::take(&mut self.pending));
        }
    }
}

fn chat_transcript_tool_result_preview(content: &LanguageModelToolResultContent) -> String {
    let body = content.to_str().unwrap_or("[non-text tool result]");
    if body.len() <= TUI_TOOL_OUTPUT_PREVIEW_CHARS {
        format!("\n← result:\n{body}\n")
    } else {
        let mut end = TUI_TOOL_OUTPUT_PREVIEW_CHARS.min(body.len());
        while end > 0 && !body.is_char_boundary(end) {
            end -= 1;
        }
        format!(
            "\n← result (truncated, {} chars total):\n{}…\n",
            body.len(),
            &body[..end]
        )
    }
}



pub fn list_children_sync(
    project: &Entity<Project>,
    parent_abs: &std::path::Path,
    cx: &gpui::App,
) -> Result<Vec<crate::quorp::tui::file_tree::TreeChild>, String> {
    let project_read = project.read(cx);
    let (worktree, parent_rel) = project_read
        .find_worktree(parent_abs, cx)
        .ok_or_else(|| format!("path not in project: {}", parent_abs.display()))?;
    let wt = worktree.read(cx);
    let entry = wt
        .entry_for_path(parent_rel.as_ref())
        .ok_or_else(|| format!("no worktree entry for {}", parent_abs.display()))?;
    if !entry.is_dir() {
        return Err(format!("not a directory: {}", parent_abs.display()));
    }
    let needs_expand = matches!(
        entry.kind,
        EntryKind::UnloadedDir | EntryKind::PendingDir
    );
    if needs_expand {
        return Err("directory not loaded yet".to_string());
    }

    let mut out = Vec::new();
    for child in wt.child_entries(parent_rel.as_ref()) {
        if child.is_ignored && !child.is_always_included {
            continue;
        }
        if child.path.file_name().is_none() {
            continue;
        }
        let abs_path = wt.absolutize(child.path.as_ref());
        let name = child
            .path
            .file_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "<invalid>".to_string());
        out.push(crate::quorp::tui::file_tree::TreeChild {
            path: abs_path,
            name,
            is_directory: child.is_dir(),
        });
    }
    out.sort_by(|a, b| match (a.is_directory, b.is_directory) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(out)
}

#[derive(Debug)]
pub enum TuiToBackendRequest {
    // Phase 2 placeholders for future unified implementations
    ListDirectory(PathBuf),
    OpenBuffer(PathBuf),
    CloseBuffer,

    PersistDefaultModel {
        registry_line: String,
    },
    StreamChat {
        request: LanguageModelRequest,
        preferred_model: Option<SelectedModel>,
        cancel: Arc<AtomicBool>,
        session_id: usize,
    },



    // Terminal / PTY
    TerminalKeystroke(Keystroke),
    TerminalInput(Vec<u8>),
    TerminalResize { cols: u16, rows: u16 },
    TerminalScrollPageUp,
    TerminalScrollPageDown,
    
    // Agent
    StartAgentAction(String),
}

pub struct UnifiedBridgeTuiBackend {
    request_tx: futures::channel::mpsc::UnboundedSender<TuiToBackendRequest>,
}

impl UnifiedBridgeTuiBackend {
    pub fn new(request_tx: futures::channel::mpsc::UnboundedSender<TuiToBackendRequest>) -> Self {
        Self { request_tx }
    }
}

impl TuiBackend for UnifiedBridgeTuiBackend {
    fn request_list_directory(&self, path: PathBuf) -> Result<(), String> {
        self.request_tx
            .unbounded_send(TuiToBackendRequest::ListDirectory(path))
            .map_err(|error| error.to_string())
    }

    fn request_open_buffer(&self, path: PathBuf) -> Result<(), String> {
        self.request_tx
            .unbounded_send(TuiToBackendRequest::OpenBuffer(path))
            .map_err(|error| error.to_string())
    }

    fn request_close_buffer(&self) -> Result<(), String> {
        self.request_tx
            .unbounded_send(TuiToBackendRequest::CloseBuffer)
            .map_err(|error| error.to_string())
    }
}

#[derive(Debug, Clone)]
pub enum BackendToTuiResponse {
    // Phase 2 placeholders
    DirectoryListed(PathBuf, Result<Vec<crate::quorp::tui::file_tree::TreeChild>, String>),
    BufferChunk {
        path: Option<PathBuf>,
        lines: Vec<ratatui::text::Line<'static>>,
        error: Option<String>,
        truncated: bool,
    },

    TerminalFrame(TerminalFrame),
    AgentStatusUpdate(String),
}

/// Spawns the main GPUI background task to listen to TuiToBackendRequests and
/// respond with BackendToTuiResponses.

fn send_chat_ui(tx: &std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>, event: ChatUiEvent) {
    if let Err(e) = tx.send(crate::quorp::tui::TuiEvent::Chat(event)) {
        log::error!("unified bridge: UI channel closed: {e}");
    }
}

pub fn spawn_unified_bridge_loop(
    project: gpui::Entity<project::Project>,
    terminal_entity: Option<Entity<Terminal>>,
    mut cx: AsyncApp,
    mut request_rx: futures::channel::mpsc::UnboundedReceiver<TuiToBackendRequest>,
    response_tx: std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
    fs: Arc<dyn Fs>,
) -> gpui::Task<()> {
    let theme = cx.update(|cx| cx.theme().clone());
    cx.spawn(async move |mut async_cx| {
        let observe_tx = response_tx.clone();
        let _ = async_cx.update(|cx| {
            cx.observe_global::<settings::SettingsStore>(move |_cx| {
                let _ = observe_tx.send(crate::quorp::tui::TuiEvent::ThemeReloaded);
            })
            .detach();
        });
        use futures::StreamExt;
        let mut _buffer_subscription: Option<Subscription> = None;

        if let Some(terminal) = &terminal_entity {
            let event_for_sub = response_tx.clone();
            let terminal_for_sub = terminal.clone();
            let _ = async_cx.update(|cx| {
                cx.subscribe(&terminal_for_sub, {
                    let event_tx = event_for_sub.clone();
                    move |entity, event: &terminal::Event, cx| match event {
                        terminal::Event::Wakeup => {
                            push_frame(&entity, &event_tx, cx);
                        }
                        terminal::Event::CloseTerminal => {
                            let _ = event_tx.try_send(crate::quorp::tui::TuiEvent::TerminalClosed);
                        }
                        _ => {}
                    }
                })
                .detach();
                push_frame(&terminal_for_sub, &event_for_sub, cx);
            });
        }

        while let Some(req) = request_rx.next().await {
            match req {
                TuiToBackendRequest::ListDirectory(parent_abs) => {
                    let mut listing =
                        async_cx.update(|cx| list_children_sync(&project, &parent_abs, cx));
                    
                    if listing.as_ref().err().map(|e| e.as_str()) == Some("directory not loaded yet") {
                        let expand_task: Option<Task<Result<(), anyhow::Error>>> =
                            async_cx.update(|cx| {
                                let (worktree, parent_rel) =
                                    project.read(cx).find_worktree(&parent_abs, cx)?;
                                let entry_id = {
                                    let wt = worktree.read(cx);
                                    let entry = wt.entry_for_path(parent_rel.as_ref())?;
                                    if !matches!(
                                        entry.kind,
                                        EntryKind::UnloadedDir | EntryKind::PendingDir
                                    ) {
                                        return None;
                                    }
                                    entry.id
                                };
                                worktree.update(cx, |wt, cx| wt.expand_entry(entry_id, cx))
                            });
                        
                        listing = if let Some(task) = expand_task {
                            match task.await {
                                Ok(()) => async_cx.update(|cx| {
                                    list_children_sync(&project, &parent_abs, cx)
                                }),
                                Err(e) => Err(format!("failed to expand directory: {e:#}")),
                            }
                        } else {
                            async_cx.update(|cx| list_children_sync(&project, &parent_abs, cx))
                        };
                    }

                    let _ = response_tx.send(crate::quorp::tui::TuiEvent::FileTreeListed {
                        parent: parent_abs,
                        result: listing,
                    });
                }
                TuiToBackendRequest::OpenBuffer(path) => {
                    _buffer_subscription.take();
                    let path_for_err = path.clone();
                    let open_task = async_cx.update(|cx| {
                        project.update(cx, |project, cx| project.open_local_buffer(&path, cx))
                    });

                    match open_task.await {
                        Ok(buffer) => {
                            let path_snap = path.clone();
                            let ev = response_tx.clone();
                            let theme_for_sub = theme.clone();
                            _buffer_subscription = Some(async_cx.update(|cx| {
                                let path_cb = path_snap.clone();
                                let ev_cb = ev.clone();
                                let sub = cx.subscribe(&buffer, move |buf, event: &BufferEvent, cx| {
                                    if buffer_refresh_event(event) {
                                        push_snapshot(&buf, path_cb.clone(), &ev_cb, cx);
                                    }
                                });
                                push_snapshot(&buffer, path_snap, &ev, cx);
                                sub
                            }));
                        }
                        Err(error) => {
                            let _ = response_tx.send(crate::quorp::tui::TuiEvent::UnifiedResponse(
                                BackendToTuiResponse::BufferChunk {
                                    path: Some(path_for_err),
                                    lines: Vec::new(),
                                    error: Some(format!("Failed to open file in project: {error:#}")),
                                    truncated: false,
                                }
                            ));
                        }
                    }
                }
                TuiToBackendRequest::CloseBuffer => {
                    _buffer_subscription.take();
                }


                TuiToBackendRequest::TerminalKeystroke(ks) => {
                    if let Some(term) = &terminal_entity {
                        let term = term.clone();
                        let _ = async_cx.update(|cx| {
                            let _ = term.update(cx, |t, _| {
                                let _ = t.try_keystroke(&ks, false);
                            });
                        });
                    }
                }
                TuiToBackendRequest::TerminalInput(bytes) => {
                    if let Some(term) = &terminal_entity {
                        let term = term.clone();
                        let _ = async_cx.update(|cx| {
                            let _ = term.update(cx, |t, _cx| t.input(bytes));
                        });
                    }
                }
                TuiToBackendRequest::TerminalResize { cols, rows } => {
                    if let Some(term) = &terminal_entity {
                        let term = term.clone();
                        let bounds = terminal_bounds_from_grid(cols, rows);
                        let _ = async_cx.update(|cx| {
                            let _ = term.update(cx, |t, _| t.set_size(bounds));
                        });
                    }
                }
                TuiToBackendRequest::TerminalScrollPageUp => {
                    if let Some(term) = &terminal_entity {
                        let term = term.clone();
                        let _ = async_cx.update(|cx| {
                            let _ = term.update(cx, |t, _| t.scroll_page_up());
                        });
                    }
                }
                TuiToBackendRequest::TerminalScrollPageDown => {
                    if let Some(term) = &terminal_entity {
                        let term = term.clone();
                        let _ = async_cx.update(|cx| {
                            let _ = term.update(cx, |t, _| t.scroll_page_down());
                        });
                    }
                }
                TuiToBackendRequest::StartAgentAction(action) => {
                    let _ = response_tx.send(crate::quorp::tui::TuiEvent::UnifiedResponse(
                        BackendToTuiResponse::AgentStatusUpdate(format!("Agent started: {}", action))
                    ));
                    // TODO: Connect this to actual MCP tool-calling workflows.
                }

                TuiToBackendRequest::PersistDefaultModel { registry_line } => {
                    let selected = match SelectedModel::from_str(&registry_line) {
                        Ok(s) => s,
                        Err(error) => {
                            log::warn!(
                                "chat bridge: skip persisting invalid model id `{registry_line}`: {error}"
                            );
                            continue;
                        }
                    };
                    let fs = fs.clone();
                    async_cx.update(|cx| {
                        let (enable_thinking, effort) =
                            LanguageModelRegistry::global(cx).update(cx, |registry, cx| {
                                registry
                                    .select_model(&selected, cx)
                                    .map(|configured| {
                                        (
                                            configured.model.supports_thinking(),
                                            configured.model.default_effort_level().map(
                                                |effort| effort.value.to_string(),
                                            ),
                                        )
                                    })
                                    .unwrap_or((false, None))
                            });

                        let provider_str = selected.provider.0.to_string();
                        let model_str = selected.model.0.to_string();
                        update_settings_file(fs, cx, move |settings, _| {
                            settings.agent.get_or_insert_default().set_model(
                                LanguageModelSelection {
                                    provider: LanguageModelProviderSetting(provider_str),
                                    model: model_str,
                                    enable_thinking,
                                    effort,
                                },
                            );
                        });

                        LanguageModelRegistry::global(cx).update(cx, |registry, cx| {
                            registry.select_default_model(Some(&selected), cx);
                        });
                    });
                }
                TuiToBackendRequest::StreamChat {
                    mut request,
                    preferred_model,
                    cancel,
                    session_id,
                } => {
                    let (configured, config_err) = async_cx.update(|cx| {
                        LanguageModelRegistry::global(cx).update(cx, |reg, cx| {
                            let configured = if let Some(ref sel) = preferred_model {
                                reg.select_model(sel, cx)
                            } else {
                                reg.default_model()
                            };
                            let err = reg.configuration_error(configured.clone(), cx);
                            (configured, err)
                        })
                    });

                    if let Some(err) = config_err {
                        send_chat_ui(
                            &response_tx,
                            ChatUiEvent::Error(session_id, err.to_string()),
                        );
                        send_chat_ui(&response_tx, ChatUiEvent::StreamFinished(session_id));
                        continue;
                    }

                    let Some(configured) = configured else {
                        send_chat_ui(
                            &response_tx,
                            ChatUiEvent::Error(
                                session_id,
                                "No language model selected. Configure a default model in Quorp."
                                    .to_string(),
                            ),
                        );
                        send_chat_ui(&response_tx, ChatUiEvent::StreamFinished(session_id));
                        continue;
                    };

                    let model = configured.model.clone();
                    let project = project.clone();
                    let fs = fs.clone();
                    let response_tx = response_tx.clone();

                    for round_index in 0..MAX_TUI_TOOL_ROUNDS {
                        if cancel.load(Ordering::Acquire) {
                            break;
                        }

                        if round_index > 0 {
                            request.intent = Some(CompletionIntent::ToolResults);
                        }

                        let stream_result = model.stream_completion(request.clone(), &async_cx).await;

                        let mut stream = match stream_result {
                            Ok(stream) => stream,
                            Err(error) => {
                                send_chat_ui(
                                    &response_tx,
                                    ChatUiEvent::Error(session_id, format!("stream start failed: {error}")),
                                );
                                send_chat_ui(&response_tx, ChatUiEvent::StreamFinished(session_id));
                                break;
                            }
                        };

                        let mut assistant_text = String::new();
                        let mut text_coalescer = AssistantTextCoalescer::new();
                        let mut tool_uses = Vec::new();
                        let mut stream_failed = false;

                        while let Some(item) = stream.next().await {
                            if cancel.load(Ordering::Acquire) {
                                break;
                            }
                            match item {
                                Ok(LanguageModelCompletionEvent::Text(text)) => {
                                    if !text.is_empty() {
                                        assistant_text.push_str(&text);
                                        let response_tx = response_tx.clone();
                                        text_coalescer.push_chunk(&text, |piece| {
                                            send_chat_ui(
                                                &response_tx,
                                                ChatUiEvent::AssistantDelta(session_id, piece),
                                            );
                                        });
                                    }
                                }
                                Ok(LanguageModelCompletionEvent::Thinking { text, .. }) => {
                                    if !text.is_empty() {
                                        assistant_text.push_str(&text);
                                        let response_tx = response_tx.clone();
                                        text_coalescer.push_chunk(&text, |piece| {
                                            send_chat_ui(
                                                &response_tx,
                                                ChatUiEvent::AssistantDelta(session_id, piece),
                                            );
                                        });
                                    }
                                }
                                Ok(LanguageModelCompletionEvent::ToolUse(tool_use)) => {
                                    if tool_use.is_input_complete {
                                        let response_tx_flush = response_tx.clone();
                                        text_coalescer.flush_all(|piece| {
                                            send_chat_ui(
                                                &response_tx_flush,
                                                ChatUiEvent::AssistantDelta(session_id, piece),
                                            );
                                        });
                                        send_chat_ui(
                                            &response_tx,
                                            ChatUiEvent::AssistantDelta(
                                                session_id,
                                                format!(
                                                    "\n▸ {} ({})\n",
                                                    tool_use.name, tool_use.id
                                                )
                                            ),
                                        );
                                        tool_uses.push(tool_use);
                                    }
                                }
                                Ok(LanguageModelCompletionEvent::ToolUseJsonParseError {
                                    tool_name,
                                    json_parse_error,
                                    ..
                                }) => {
                                    send_chat_ui(
                                        &response_tx,
                                        ChatUiEvent::Error(session_id, format!(
                                            "tool {tool_name}: invalid arguments ({json_parse_error})"
                                        )),
                                    );
                                }
                                Ok(
                                    LanguageModelCompletionEvent::Queued { .. }
                                    | LanguageModelCompletionEvent::Started
                                    | LanguageModelCompletionEvent::Stop(_)
                                    | LanguageModelCompletionEvent::RedactedThinking { .. }
                                    | LanguageModelCompletionEvent::StartMessage { .. }
                                    | LanguageModelCompletionEvent::ReasoningDetails(_)
                                    | LanguageModelCompletionEvent::UsageUpdate(_),
                                ) => {}
                                Err(error) => {
                                    stream_failed = true;
                                    send_chat_ui(
                                        &response_tx,
                                        ChatUiEvent::Error(session_id, format!("stream error: {error}")),
                                    );
                                    break;
                                }
                            }
                        }

                        let response_tx_flush = response_tx.clone();
                        text_coalescer.flush_all(|piece| {
                            send_chat_ui(&response_tx_flush, ChatUiEvent::AssistantDelta(session_id, piece));
                        });

                        if stream_failed {
                            break;
                        }

                        if tool_uses.is_empty() {
                            break;
                        }

                        let mut assistant_content = Vec::new();
                        if !assistant_text.trim().is_empty() {
                            assistant_content.push(MessageContent::Text(assistant_text));
                        }
                        for tool_use in &tool_uses {
                            assistant_content.push(MessageContent::ToolUse(tool_use.clone()));
                        }

                        request.messages.push(LanguageModelRequestMessage {
                            role: Role::Assistant,
                            content: assistant_content,
                            cache: false,
                            reasoning_details: None,
                        });

                        let mut result_blocks = Vec::new();
                        for tool_use in tool_uses {
                            let tool_name = tool_use.name.clone();
                            let tool_id = tool_use.id.clone();
                            match execute_tui_tool_call(
                                &tool_use,
                                &project,
                                fs.clone(),
                                &async_cx,
                            )
                            .await
                            {
                                Ok(content) => {
                                    send_chat_ui(
                                        &response_tx,
                                        ChatUiEvent::AssistantDelta(
                                            session_id,
                                            chat_transcript_tool_result_preview(&content),
                                        ),
                                    );
                                    result_blocks.push(MessageContent::ToolResult(
                                        LanguageModelToolResult {
                                            tool_use_id: tool_id,
                                            tool_name,
                                            is_error: false,
                                            content,
                                            output: None,
                                        },
                                    ));
                                }
                                Err(message) => {
                                    send_chat_ui(
                                        &response_tx,
                                        ChatUiEvent::AssistantDelta(session_id, format!(
                                            "\n← tool error: {message}\n"
                                        )),
                                    );
                                    result_blocks.push(MessageContent::ToolResult(
                                        LanguageModelToolResult {
                                            tool_use_id: tool_id,
                                            tool_name,
                                            is_error: true,
                                            content: language_model::LanguageModelToolResultContent::Text(
                                                message.into(),
                                            ),
                                            output: None,
                                        },
                                    ));
                                }
                            }
                        }

                        request.messages.push(LanguageModelRequestMessage {
                            role: Role::User,
                            content: result_blocks,
                            cache: false,
                            reasoning_details: None,
                        });
                    }

                    send_chat_ui(&response_tx, ChatUiEvent::StreamFinished(session_id));
                }
            }
        }
    })
}

pub fn crossterm_key_event_to_keystroke(key: &KeyEvent) -> Option<Keystroke> {
    if key.kind == KeyEventKind::Release {
        return None;
    }

    let mut modifiers = Modifiers::none();
    modifiers.control = key.modifiers.contains(KeyModifiers::CONTROL);
    modifiers.alt = key.modifiers.contains(KeyModifiers::ALT);
    modifiers.shift = key.modifiers.contains(KeyModifiers::SHIFT);

    let key_str: String = match key.code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => {
            if c.is_ascii_uppercase() {
                modifiers.shift = true;
            }
            c.to_ascii_lowercase().to_string()
        }
        KeyCode::Enter => "enter".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => {
            modifiers.shift = true;
            "tab".into()
        }
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Esc => "escape".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pageup".into(),
        KeyCode::PageDown => "pagedown".into(),
        KeyCode::Insert => "insert".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::F(n) => format!("f{n}"),
        _ => return None,
    };

    Some(Keystroke {
        modifiers,
        key: key_str,
        key_char: None,
    })
}

#[derive(Debug, Clone)]
pub struct TerminalFrame {
    pub lines: Vec<Line<'static>>,
}

/// Maps a crossterm key to a GPUI [`Keystroke`] for `Terminal::try_keystroke`.


pub fn terminal_bounds_from_grid(cols: u16, rows: u16) -> TerminalBounds {
    let cell_width = px(5.);
    let line_height = px(5.);
    TerminalBounds::new(
        line_height,
        cell_width,
        Bounds {
            origin: Point::default(),
            size: Size {
                width: cell_width * cols as f32,
                height: line_height * rows as f32,
            },
        },
    )
}



fn is_decorative_character(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2500..=0x257F
            | 0x2580..=0x259F
            | 0x25A0..=0x25FF
            | 0xE0B0..=0xE0B7
            | 0xE0B8..=0xE0BF
            | 0xE0C0..=0xE0CA
            | 0xE0CC..=0xE0D1
            | 0xE0D2..=0xE0D7
    )
}

fn cell_to_ratatui_style(indexed: &IndexedCell, theme: &theme::Theme) -> Style {
    let mut fg = indexed.fg;
    let mut bg = indexed.bg;
    if indexed.flags.contains(Flags::INVERSE) {
        mem::swap(&mut fg, &mut bg);
    }

    let mut fg_h = convert_color(&fg, theme);
    let bg_h = convert_color(&bg, theme);

    if !is_decorative_character(indexed.c) {
        fg_h = ensure_minimum_contrast(fg_h, bg_h, TUI_TERMINAL_MIN_CONTRAST);
    }

    if indexed.flags.intersects(Flags::DIM) {
        fg_h.a *= 0.7;
    }

    let mut style = Style::default()
        .fg(hsla_to_ratatui_color(fg_h))
        .bg(hsla_to_ratatui_color(bg_h));

    if indexed.flags.intersects(Flags::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if indexed.flags.intersects(Flags::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if indexed.flags.intersects(Flags::ALL_UNDERLINES) {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if indexed.flags.intersects(Flags::STRIKEOUT) {
        style = style.add_modifier(Modifier::CROSSED_OUT);
    }

    style
}

fn default_placeholder_style(theme: &theme::Theme) -> Style {
    let fg = convert_color(&AnsiColor::Named(NamedColor::Foreground), theme);
    let bg = convert_color(&AnsiColor::Named(NamedColor::Background), theme);
    Style::default()
        .fg(hsla_to_ratatui_color(fg))
        .bg(hsla_to_ratatui_color(bg))
}

fn terminal_content_to_frame(content: &TerminalContent, theme: &theme::Theme) -> TerminalFrame {
    let cols = content.terminal_bounds.num_columns();
    let rows = content.terminal_bounds.num_lines();
    if cols == 0 || rows == 0 {
        return TerminalFrame { lines: Vec::new() };
    }

    let default_style = default_placeholder_style(theme);

    let line_groups: Vec<Vec<&IndexedCell>> = content
        .cells
        .iter()
        .chunk_by(|c| c.point.line)
        .into_iter()
        .map(|(_, g)| g.collect())
        .collect();

    let mut lines = Vec::with_capacity(rows);
    for row_idx in 0..rows {
        let row_cells: Vec<&IndexedCell> = line_groups
            .get(row_idx)
            .map(|v| v.as_slice())
            .unwrap_or_default()
            .iter()
            .copied()
            .sorted_by_key(|c| c.point.column.0)
            .collect();

        let mut line_spans: Vec<Span<'static>> = Vec::new();
        let mut col: usize = 0;

        for cell in row_cells {
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }

            let target_col = cell.point.column.0 as usize;
            while col < target_col {
                line_spans.push(Span::styled(" ".to_string(), default_style));
                col += 1;
            }

            let ch = cell.c;
            let advance = ch.width().unwrap_or(1).max(1);

            if is_blank(cell) {
                col += advance;
                continue;
            }

            let style = cell_to_ratatui_style(cell, theme);
            line_spans.push(Span::styled(ch.to_string(), style));
            col += advance;
        }

        while col < cols {
            line_spans.push(Span::styled(" ".to_string(), default_style));
            col += 1;
        }

        lines.push(Line::from(line_spans));
    }

    TerminalFrame { lines }
}

fn push_frame(
    terminal: &Entity<Terminal>,
    event_tx: &std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
    cx: &mut gpui::App,
) {
    let theme = cx.theme().clone();
    let content = terminal.read(cx).last_content().clone();
    let frame = terminal_content_to_frame(&content, &theme);
    let _ = event_tx.try_send(crate::quorp::tui::TuiEvent::TerminalFrame(frame));
}

#[cfg(test)]
mod tests {
    use super::terminal_bounds_from_grid;

    #[test]
    fn terminal_bounds_from_grid_matches_tty_dimensions() {
        let bounds = terminal_bounds_from_grid(80, 24);
        assert_eq!(bounds.num_columns(), 80);
        assert_eq!(bounds.num_lines(), 24);
    }
}

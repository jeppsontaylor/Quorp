use std::collections::HashMap;
use std::hash::{Hash, Hasher};
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::json;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SyntectStyle, Theme as SyntectTheme, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::quorp::tui::reason_ledger::ReasonLedgerEntry;
use crate::quorp::tui::theme::Theme;
use quorp_agent_core::ReadFileRange;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssistantSegment {
    Text(String),
    Think(String),
    Code {
        language: String,
        body: String,
    },
    RunCommand {
        command: String,
        timeout_ms: u64,
    },
    ReadFile {
        path: String,
        range: Option<ReadFileRange>,
    },
    ListDirectory {
        path: String,
    },
    WriteFile {
        path: String,
        content: String,
    },
    ApplyPatch {
        path: String,
        patch: String,
    },
    ReplaceBlock {
        path: String,
        search_block: String,
        replace_block: String,
    },
    McpCallTool {
        server_name: String,
        tool_name: String,
        arguments: serde_json::Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptSurface {
    Chat,
    Shell,
}

impl TranscriptSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Shell => "shell",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    Text,
    Think,
    Code,
    RunCommand,
    ReadFile,
    ListDirectory,
    WriteFile,
    ApplyPatch,
    ReplaceBlock,
    McpCallTool,
}

impl SegmentKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Think => "reasoning",
            Self::Code => "code",
            Self::RunCommand => "command",
            Self::ReadFile => "read_file",
            Self::ListDirectory => "list_directory",
            Self::WriteFile => "write_file",
            Self::ApplyPatch => "apply_patch",
            Self::ReplaceBlock => "replace_block",
            Self::McpCallTool => "mcp_call_tool",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SegmentRenderOptions {
    pub surface: TranscriptSurface,
    pub left_pad: &'static str,
}

impl SegmentRenderOptions {
    pub fn chat() -> Self {
        Self {
            surface: TranscriptSurface::Chat,
            left_pad: "  ",
        }
    }

    pub fn shell() -> Self {
        Self {
            surface: TranscriptSurface::Shell,
            left_pad: "",
        }
    }
}

#[derive(Debug, Clone)]
struct HighlightToken {
    text: String,
    fg: Color,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct HighlightCacheKey {
    language: String,
    body: String,
}

type HighlightCache = HashMap<HighlightCacheKey, Vec<Vec<HighlightToken>>>;

#[derive(Debug)]
struct HighlightAssets {
    syntax_set: SyntaxSet,
    theme: SyntectTheme,
}

static HIGHLIGHT_ASSETS: OnceLock<HighlightAssets> = OnceLock::new();
static HIGHLIGHT_CACHE: OnceLock<Mutex<HighlightCache>> = OnceLock::new();
static SEGMENT_DIAGNOSTIC_KEYS: OnceLock<Mutex<std::collections::HashSet<String>>> =
    OnceLock::new();

#[cfg(test)]
static PARSE_COUNTER: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static HIGHLIGHT_COUNTER: AtomicU64 = AtomicU64::new(0);

fn highlight_assets() -> &'static HighlightAssets {
    HIGHLIGHT_ASSETS.get_or_init(|| {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme = ThemeSet::load_defaults().themes["base16-ocean.dark"].clone();
        HighlightAssets { syntax_set, theme }
    })
}

fn code_block_background(theme: &Theme, surface: TranscriptSurface) -> Color {
    if matches!(surface, TranscriptSurface::Shell) {
        return theme.palette.code_block_bg;
    }
    match theme.palette.code_block_bg {
        Color::Rgb(red, green, blue) => {
            let red = red.saturating_sub(14);
            let green = green.saturating_sub(12);
            let blue = blue.saturating_sub(8);
            Color::Rgb(red, green, blue)
        }
        other => other,
    }
}

fn high_contrast_code_color(theme: &Theme, color: Color) -> Color {
    fn lift(component: u8) -> u8 {
        match component {
            0..=40 => component.saturating_add(180),
            41..=80 => component.saturating_add(120),
            81..=140 => component.saturating_add(60),
            141..=230 => component.saturating_sub(20),
            _ => 255,
        }
    }

    match color {
        Color::Rgb(red, green, blue) => {
            let adjusted = Color::Rgb(lift(red), lift(green), lift(blue));
            if adjusted == Color::Rgb(0, 0, 0) {
                theme.palette.text
            } else {
                adjusted
            }
        }
        _ => theme.palette.text,
    }
}

fn highlight_cache() -> &'static Mutex<HashMap<HighlightCacheKey, Vec<Vec<HighlightToken>>>> {
    HIGHLIGHT_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn segment_kind(segment: &AssistantSegment) -> SegmentKind {
    match segment {
        AssistantSegment::Text(_) => SegmentKind::Text,
        AssistantSegment::Think(_) => SegmentKind::Think,
        AssistantSegment::Code { .. } => SegmentKind::Code,
        AssistantSegment::RunCommand { .. } => SegmentKind::RunCommand,
        AssistantSegment::ReadFile { .. } => SegmentKind::ReadFile,
        AssistantSegment::ListDirectory { .. } => SegmentKind::ListDirectory,
        AssistantSegment::WriteFile { .. } => SegmentKind::WriteFile,
        AssistantSegment::ApplyPatch { .. } => SegmentKind::ApplyPatch,
        AssistantSegment::ReplaceBlock { .. } => SegmentKind::ReplaceBlock,
        AssistantSegment::McpCallTool { .. } => SegmentKind::McpCallTool,
    }
}

fn theme_key(theme: &Theme) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!(
        "{:?}-{:?}-{:?}-{:?}-{:?}-{:?}",
        theme.palette.text,
        theme.palette.text_faint,
        theme.palette.success_green,
        theme.palette.subtle_border,
        theme.palette.code_block_bg,
        theme.palette.chat_accent
    )
    .hash(&mut hasher);
    hasher.finish()
}

pub fn parse_assistant_segments(
    text: &str,
    session_id: usize,
    message_index: usize,
    surface: TranscriptSurface,
) -> Vec<AssistantSegment> {
    #[cfg(test)]
    PARSE_COUNTER.fetch_add(1, Ordering::Relaxed);

    let mut segments = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        let run_cmd_pos = remaining.find("<run_command");
        let read_file_pos = remaining.find("<read_file");
        let list_dir_pos = remaining.find("<list_directory");
        let write_file_pos = remaining.find("<write_file");
        let apply_patch_pos = remaining.find("<apply_patch");
        let replace_block_pos = remaining.find("<replace_block");
        let mcp_call_tool_pos = remaining.find("<mcp_call_tool");
        let think_pos = remaining.find("<think>");
        let fence_pos = remaining.find("```");

        let next = [
            run_cmd_pos,
            read_file_pos,
            list_dir_pos,
            write_file_pos,
            apply_patch_pos,
            replace_block_pos,
            mcp_call_tool_pos,
            think_pos,
            fence_pos,
        ]
        .iter()
        .filter_map(|position| *position)
        .min();

        let Some(next_pos) = next else {
            segments.push(AssistantSegment::Text(remaining.to_string()));
            break;
        };

        if next_pos > 0 {
            segments.push(AssistantSegment::Text(remaining[..next_pos].to_string()));
            remaining = &remaining[next_pos..];
            continue;
        }

        if run_cmd_pos == Some(next_pos) {
            if let Some((segment, rest)) = parse_run_command_tag(remaining) {
                segments.push(segment);
                remaining = rest;
                continue;
            }
        } else if read_file_pos == Some(next_pos) {
            if let Some((segment, rest)) = parse_read_file_tag(remaining) {
                segments.push(segment);
                remaining = rest;
                continue;
            }
        } else if list_dir_pos == Some(next_pos) {
            if let Some((segment, rest)) = parse_list_directory_tag(remaining) {
                segments.push(segment);
                remaining = rest;
                continue;
            }
        } else if write_file_pos == Some(next_pos) {
            if let Some((segment, rest)) = parse_write_file_tag(remaining) {
                segments.push(segment);
                remaining = rest;
                continue;
            }
        } else if apply_patch_pos == Some(next_pos)
            && let Some((segment, rest)) = parse_apply_patch_tag(remaining)
        {
            segments.push(segment);
            remaining = rest;
            continue;
        } else if replace_block_pos == Some(next_pos)
            && let Some((segment, rest)) = parse_replace_block_tag(remaining)
        {
            segments.push(segment);
            remaining = rest;
            continue;
        } else if mcp_call_tool_pos == Some(next_pos)
            && let Some((segment, rest)) = parse_mcp_call_tool_tag(remaining)
        {
            segments.push(segment);
            remaining = rest;
            continue;
        }

        if think_pos == Some(next_pos) {
            let after_tag = &remaining[7..];
            let (content, rest) = if let Some(end) = after_tag.find("</think>") {
                (after_tag[..end].to_string(), &after_tag[end + 8..])
            } else {
                (after_tag.to_string(), "")
            };
            segments.push(AssistantSegment::Think(content));
            remaining = rest;
            continue;
        }

        if fence_pos == Some(next_pos) {
            let after_fence = &remaining[3..];
            let language_end = after_fence.find('\n').unwrap_or(after_fence.len());
            let language = after_fence[..language_end].trim().to_string();
            let body_start = if language_end < after_fence.len() {
                language_end + 1
            } else {
                language_end
            };
            let body_rest = &after_fence[body_start..];
            let (body, rest) = if let Some(end) = body_rest.find("```") {
                (body_rest[..end].to_string(), &body_rest[end + 3..])
            } else {
                (body_rest.to_string(), "")
            };
            segments.push(AssistantSegment::Code { language, body });
            remaining = rest;
            continue;
        }

        segments.push(AssistantSegment::Text(remaining.to_string()));
        break;
    }

    log_segment_diagnostics(&segments, session_id, message_index, surface);
    segments
}

fn parse_open_tag<'a>(remaining: &'a str, tag: &str) -> Option<(&'a str, usize)> {
    let open = format!("<{tag}");
    let after_open = remaining.strip_prefix(&open)?;
    let close = after_open.find('>')?;
    let first = after_open.as_bytes().first()?;
    if !matches!(first, b' ' | b'\t' | b'\r' | b'\n' | b'>') {
        return None;
    }
    let attrs = &after_open[..close];
    Some((attrs, close + open.len() + 1))
}

fn parse_run_command_tag(remaining: &str) -> Option<(AssistantSegment, &str)> {
    let (attrs, body_start) = parse_open_tag(remaining, "run_command")?;
    let timeout_ms = extract_attr(attrs, "timeout_ms")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30_000);
    let body = &remaining[body_start..];
    let end = body.find("</run_command>")?;
    let rest = &body[end + "</run_command>".len()..];
    Some((
        AssistantSegment::RunCommand {
            command: body[..end].trim().to_string(),
            timeout_ms,
        },
        rest,
    ))
}

fn parse_read_file_tag(remaining: &str) -> Option<(AssistantSegment, &str)> {
    let (attrs, body_start) = parse_open_tag(remaining, "read_file")?;
    let path = extract_attr(attrs, "path")?.to_string();
    let range = parse_read_file_range_attrs(attrs);
    let body = &remaining[body_start..];
    let end = body.find("</read_file>")?;
    if !body[..end].trim().is_empty() {
        return None;
    }
    let rest = &body[end + "</read_file>".len()..];
    Some((AssistantSegment::ReadFile { path, range }, rest))
}

fn parse_list_directory_tag(remaining: &str) -> Option<(AssistantSegment, &str)> {
    let (attrs, body_start) = parse_open_tag(remaining, "list_directory")?;
    let path = extract_attr(attrs, "path")?.to_string();
    let body = &remaining[body_start..];
    let end = body.find("</list_directory>")?;
    if !body[..end].trim().is_empty() {
        return None;
    }
    let rest = &body[end + "</list_directory>".len()..];
    Some((AssistantSegment::ListDirectory { path }, rest))
}

fn parse_read_file_range_attrs(attrs: &str) -> Option<ReadFileRange> {
    let start_line =
        extract_attr(attrs, "start_line").and_then(|value| value.parse::<usize>().ok());
    let end_line = extract_attr(attrs, "end_line").and_then(|value| value.parse::<usize>().ok());
    match (start_line, end_line) {
        (Some(start_line), Some(end_line)) => ReadFileRange {
            start_line,
            end_line,
        }
        .normalized(),
        _ => None,
    }
}

fn parse_write_file_tag(remaining: &str) -> Option<(AssistantSegment, &str)> {
    let (attrs, body_start) = parse_open_tag(remaining, "write_file")?;
    let path = extract_attr(attrs, "path")?.to_string();
    let body = &remaining[body_start..];
    let end = body.find("</write_file>")?;
    Some((
        AssistantSegment::WriteFile {
            path,
            content: body[..end].to_string(),
        },
        &body[end + "</write_file>".len()..],
    ))
}

fn parse_apply_patch_tag(remaining: &str) -> Option<(AssistantSegment, &str)> {
    let (attrs, body_start) = parse_open_tag(remaining, "apply_patch")?;
    let path = extract_attr(attrs, "path")?.to_string();
    let body = &remaining[body_start..];
    let end = body.find("</apply_patch>")?;
    Some((
        AssistantSegment::ApplyPatch {
            path,
            patch: body[..end].to_string(),
        },
        &body[end + "</apply_patch>".len()..],
    ))
}

fn parse_replace_block_tag(remaining: &str) -> Option<(AssistantSegment, &str)> {
    let (attrs, body_start) = parse_open_tag(remaining, "replace_block")?;
    let path = extract_attr(attrs, "path")?.to_string();
    let search_block = extract_attr(attrs, "search_block")?.to_string();
    let body = &remaining[body_start..];
    let end = body.find("</replace_block>")?;
    Some((
        AssistantSegment::ReplaceBlock {
            path,
            search_block,
            replace_block: body[..end].to_string(),
        },
        &body[end + "</replace_block>".len()..],
    ))
}

fn parse_mcp_call_tool_tag(remaining: &str) -> Option<(AssistantSegment, &str)> {
    let (attrs, body_start) = parse_open_tag(remaining, "mcp_call_tool")?;
    let server_name = extract_attr(attrs, "server_name")?.to_string();
    let tool_name = extract_attr(attrs, "tool_name")?.to_string();
    let body = &remaining[body_start..];
    let end = body.find("</mcp_call_tool>")?;
    let arguments = serde_json::from_str(body[..end].trim()).ok()?;
    Some((
        AssistantSegment::McpCallTool {
            server_name,
            tool_name,
            arguments,
        },
        &body[end + "</mcp_call_tool>".len()..],
    ))
}

pub fn render_assistant_segments(
    segments: &[AssistantSegment],
    theme: &Theme,
    options: SegmentRenderOptions,
) -> Vec<Line<'static>> {
    let _ = theme_key(theme);
    let mut lines = Vec::new();
    let mut emitted_anything = false;

    for segment in segments {
        match segment {
            AssistantSegment::Text(text) => {
                if text.is_empty() && !emitted_anything {
                    lines.push(Line::from(Span::styled(
                        options.left_pad.to_string(),
                        Style::default(),
                    )));
                    emitted_anything = true;
                    continue;
                }
                for line in text.lines() {
                    lines.push(Line::from(vec![
                        Span::styled(options.left_pad.to_string(), Style::default()),
                        Span::styled(line.to_string(), Style::default().fg(theme.palette.text)),
                    ]));
                    emitted_anything = true;
                }
            }
            AssistantSegment::Think(content) => {
                let ledger = ReasonLedgerEntry::parse_from_think(content);
                lines.extend(ledger.render(theme, options.left_pad));
                emitted_anything = true;
            }
            AssistantSegment::Code { language, body } => {
                let language_label = if language.is_empty() {
                    "code".to_string()
                } else {
                    language.clone()
                };
                let border_style = Style::default().fg(theme.palette.text_faint);
                let code_bg = code_block_background(theme, options.surface);
                let gutter_style = Style::default()
                    .fg(theme.palette.text_muted)
                    .bg(code_bg)
                    .add_modifier(Modifier::BOLD);
                let gutter_width = body.lines().count().max(1).to_string().len().max(2) + 3;
                lines.push(Line::from(vec![
                    Span::styled(options.left_pad.to_string(), Style::default()),
                    Span::styled(format!("┌── {language_label} "), border_style.bg(code_bg)),
                ]));
                emitted_anything = true;

                let highlighted = highlight_code(body, language);
                if highlighted.is_empty() {
                    lines.push(Line::from(vec![
                        Span::styled(options.left_pad.to_string(), Style::default()),
                        Span::styled(format!("{:>gutter_width$} ", 1), gutter_style.bg(code_bg)),
                        Span::styled(" ".to_string(), Style::default().bg(code_bg)),
                    ]));
                } else {
                    for (line_index, line_tokens) in highlighted.iter().enumerate() {
                        let line_number = line_index.saturating_add(1);
                        let gutter = format!(" {:>width$} ", line_number, width = gutter_width - 1);
                        let mut spans = vec![
                            Span::styled(options.left_pad.to_string(), Style::default()),
                            Span::styled("│".to_string(), Style::default().bg(code_bg)),
                            Span::styled(gutter, gutter_style),
                        ];
                        if line_tokens.is_empty() {
                            spans.push(Span::styled(" ".to_string(), Style::default().bg(code_bg)));
                        } else {
                            for token in line_tokens {
                                let fg = if token.fg == Color::Reset {
                                    theme.palette.text
                                } else {
                                    high_contrast_code_color(theme, token.fg)
                                };
                                spans.push(Span::styled(
                                    token.text.clone(),
                                    Style::default().fg(fg).bg(code_bg),
                                ));
                            }
                        }
                        lines.push(Line::from(spans));
                    }
                }
                lines.push(Line::from(vec![
                    Span::styled(options.left_pad.to_string(), Style::default()),
                    Span::styled("└────────────".to_string(), border_style.bg(code_bg)),
                ]));
            }
            AssistantSegment::RunCommand { command, .. } => {
                let command_style = Style::default().fg(theme.palette.success_green);
                lines.push(Line::from(Span::styled(
                    format!("{}┌── command", options.left_pad),
                    command_style,
                )));
                emitted_anything = true;
                for command_line in command.lines() {
                    lines.push(Line::from(vec![
                        Span::styled(format!("{}│ $ ", options.left_pad), command_style),
                        Span::styled(
                            command_line.to_string(),
                            Style::default()
                                .fg(theme.palette.text)
                                .bg(theme.palette.code_block_bg),
                        ),
                    ]));
                }
                lines.push(Line::from(Span::styled(
                    format!("{}└────────────", options.left_pad),
                    command_style,
                )));
            }
            AssistantSegment::ReadFile { path, range } => {
                let label_style = Style::default()
                    .fg(theme.palette.success_green)
                    .add_modifier(Modifier::BOLD);
                lines.push(Line::from(Span::styled(
                    format!("{}┌── read_file", options.left_pad),
                    label_style,
                )));
                lines.push(Line::from(vec![
                    Span::styled(format!("{}│ path: ", options.left_pad), Style::default()),
                    Span::styled(
                        path.to_string(),
                        Style::default().fg(theme.palette.text_faint),
                    ),
                ]));
                if let Some(range) = range.and_then(|value| value.normalized()) {
                    lines.push(Line::from(vec![
                        Span::styled(format!("{}│ lines: ", options.left_pad), Style::default()),
                        Span::styled(range.label(), Style::default().fg(theme.palette.text_faint)),
                    ]));
                }
                lines.push(Line::from(Span::styled(
                    format!("{}└────────────", options.left_pad),
                    label_style,
                )));
            }
            AssistantSegment::ListDirectory { path } => {
                let label_style = Style::default()
                    .fg(theme.palette.success_green)
                    .add_modifier(Modifier::BOLD);
                lines.push(Line::from(Span::styled(
                    format!("{}┌── list_directory", options.left_pad),
                    label_style,
                )));
                lines.push(Line::from(vec![
                    Span::styled(format!("{}│ path: ", options.left_pad), Style::default()),
                    Span::styled(
                        path.to_string(),
                        Style::default().fg(theme.palette.text_faint),
                    ),
                ]));
                lines.push(Line::from(Span::styled(
                    format!("{}└────────────", options.left_pad),
                    label_style,
                )));
            }
            AssistantSegment::WriteFile { path, content } => {
                let border_style = Style::default()
                    .fg(theme.palette.success_green)
                    .add_modifier(Modifier::BOLD);
                let body_style = Style::default()
                    .fg(theme.palette.text)
                    .bg(theme.palette.code_block_bg);
                lines.push(Line::from(Span::styled(
                    format!("{}┌── write_file", options.left_pad),
                    border_style,
                )));
                lines.push(Line::from(vec![
                    Span::styled(format!("{}│ path: ", options.left_pad), Style::default()),
                    Span::styled(
                        path.to_string(),
                        Style::default().fg(theme.palette.text_faint),
                    ),
                ]));
                for line in content.lines().take(10) {
                    lines.push(Line::from(vec![
                        Span::styled(format!("{}│ ", options.left_pad), body_style),
                        Span::styled(line.to_string(), body_style),
                    ]));
                }
                if content.lines().count() > 10 {
                    lines.push(Line::from(vec![Span::styled(
                        format!("{}│ ...", options.left_pad),
                        body_style,
                    )]));
                }
                lines.push(Line::from(Span::styled(
                    format!("{}└────────────", options.left_pad),
                    border_style,
                )));
            }
            AssistantSegment::ApplyPatch { path, patch } => {
                let border_style = Style::default()
                    .fg(theme.palette.success_green)
                    .add_modifier(Modifier::BOLD);
                let body_style = Style::default()
                    .fg(theme.palette.text)
                    .bg(theme.palette.code_block_bg);
                lines.push(Line::from(Span::styled(
                    format!("{}┌── apply_patch", options.left_pad),
                    border_style,
                )));
                lines.push(Line::from(vec![
                    Span::styled(format!("{}│ path: ", options.left_pad), Style::default()),
                    Span::styled(
                        path.to_string(),
                        Style::default().fg(theme.palette.text_faint),
                    ),
                ]));
                for line in patch.lines().take(10) {
                    lines.push(Line::from(vec![
                        Span::styled(format!("{}│ ", options.left_pad), body_style),
                        Span::styled(line.to_string(), body_style),
                    ]));
                }
                if patch.lines().count() > 10 {
                    lines.push(Line::from(vec![Span::styled(
                        format!("{}│ ...", options.left_pad),
                        body_style,
                    )]));
                }
                lines.push(Line::from(Span::styled(
                    format!("{}└────────────", options.left_pad),
                    border_style,
                )));
            }
            AssistantSegment::ReplaceBlock { path, .. } => {
                let border_style = Style::default()
                    .fg(theme.palette.success_green)
                    .add_modifier(Modifier::BOLD);
                lines.push(Line::from(Span::styled(
                    format!("{}┌── replace_block", options.left_pad),
                    border_style,
                )));
                lines.push(Line::from(vec![
                    Span::styled(format!("{}│ path: ", options.left_pad), Style::default()),
                    Span::styled(
                        path.to_string(),
                        Style::default().fg(theme.palette.text_faint),
                    ),
                ]));
                lines.push(Line::from(Span::styled(
                    format!("{}└────────────", options.left_pad),
                    border_style,
                )));
            }
            AssistantSegment::McpCallTool {
                server_name,
                tool_name,
                arguments,
            } => {
                let border_style = Style::default()
                    .fg(theme.palette.success_green)
                    .add_modifier(Modifier::BOLD);
                let body_style = Style::default()
                    .fg(theme.palette.text)
                    .bg(theme.palette.code_block_bg);
                let rendered_arguments = serde_json::to_string_pretty(arguments)
                    .unwrap_or_else(|_| arguments.to_string());
                lines.push(Line::from(Span::styled(
                    format!("{}┌── mcp_call_tool", options.left_pad),
                    border_style,
                )));
                lines.push(Line::from(vec![
                    Span::styled(format!("{}│ server: ", options.left_pad), Style::default()),
                    Span::styled(
                        server_name.to_string(),
                        Style::default().fg(theme.palette.text_faint),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled(format!("{}│ tool: ", options.left_pad), Style::default()),
                    Span::styled(
                        tool_name.to_string(),
                        Style::default().fg(theme.palette.text_faint),
                    ),
                ]));
                for line in rendered_arguments.lines().take(10) {
                    lines.push(Line::from(vec![
                        Span::styled(format!("{}│ ", options.left_pad), body_style),
                        Span::styled(line.to_string(), body_style),
                    ]));
                }
                if rendered_arguments.lines().count() > 10 {
                    lines.push(Line::from(vec![Span::styled(
                        format!("{}│ ...", options.left_pad),
                        body_style,
                    )]));
                }
                lines.push(Line::from(Span::styled(
                    format!("{}└────────────", options.left_pad),
                    border_style,
                )));
            }
        }
    }

    if !emitted_anything && lines.is_empty() {
        lines.push(Line::from(Span::styled(
            options.left_pad.to_string(),
            Style::default(),
        )));
    }

    lines
}

fn log_segment_diagnostics(
    segments: &[AssistantSegment],
    session_id: usize,
    message_index: usize,
    surface: TranscriptSurface,
) {
    let seen = SEGMENT_DIAGNOSTIC_KEYS.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
    for (segment_index, segment) in segments.iter().enumerate() {
        let kind = segment_kind(segment);
        let language = match segment {
            AssistantSegment::Code { language, .. } if !language.is_empty() => {
                Some(language.clone())
            }
            _ => None,
        };
        let key = format!(
            "{}:{session_id}:{message_index}:{segment_index}:{}:{}",
            surface.as_str(),
            kind.as_str(),
            language.as_deref().unwrap_or_default()
        );
        if let Ok(mut entries) = seen.lock()
            && !entries.insert(key)
        {
            continue;
        }
        crate::quorp::tui::diagnostics::log_event(
            "assistant.segment_classified",
            json!({
                "session_id": session_id,
                "message_index": message_index,
                "segment_index": segment_index,
                "surface": surface.as_str(),
                "segment_kind": kind.as_str(),
                "language": language,
                "rendered_as_code": matches!(kind, SegmentKind::Code),
            }),
        );
    }
}

fn extract_attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let pattern = format!("{name}=\"");
    let start = attrs.find(&pattern)?;
    let value_start = start + pattern.len();
    let end = attrs[value_start..].find('"')?;
    Some(&attrs[value_start..value_start + end])
}

fn highlight_code(code: &str, language: &str) -> Vec<Vec<HighlightToken>> {
    let key = HighlightCacheKey {
        language: language.to_string(),
        body: code.to_string(),
    };
    if let Ok(cache) = highlight_cache().lock()
        && let Some(cached) = cache.get(&key)
    {
        return cached.clone();
    }

    #[cfg(test)]
    HIGHLIGHT_COUNTER.fetch_add(1, Ordering::Relaxed);

    let assets = highlight_assets();
    let syntax = assets
        .syntax_set
        .find_syntax_by_token(language)
        .or_else(|| assets.syntax_set.find_syntax_by_extension(language))
        .unwrap_or_else(|| assets.syntax_set.find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, &assets.theme);
    let mut output = Vec::new();

    let lines = if code.is_empty() {
        vec![""]
    } else {
        code.split('\n').collect::<Vec<_>>()
    };

    for line in lines {
        let ranges: Vec<(SyntectStyle, &str)> = highlighter
            .highlight_line(line, &assets.syntax_set)
            .unwrap_or_default();
        let rendered = if ranges.is_empty() {
            vec![HighlightToken {
                text: line.to_string(),
                fg: Color::Reset,
            }]
        } else {
            ranges
                .into_iter()
                .map(|(style, text)| HighlightToken {
                    text: text.to_string(),
                    fg: Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b),
                })
                .collect()
        };
        output.push(rendered);
    }

    if let Ok(mut cache) = highlight_cache().lock() {
        cache.insert(key, output.clone());
    }
    output
}

#[cfg(test)]
pub fn reset_test_counters() {
    PARSE_COUNTER.store(0, Ordering::Relaxed);
    HIGHLIGHT_COUNTER.store(0, Ordering::Relaxed);
    if let Ok(mut cache) = highlight_cache().lock() {
        cache.clear();
    }
}

#[cfg(test)]
pub fn parse_count_for_test() -> u64 {
    PARSE_COUNTER.load(Ordering::Relaxed)
}

#[cfg(test)]
pub fn highlight_count_for_test() -> u64 {
    HIGHLIGHT_COUNTER.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fenced_python_becomes_code_segment() {
        let segments =
            parse_assistant_segments("```python\nprint('hi')\n```", 1, 0, TranscriptSurface::Chat);
        assert_eq!(
            segments,
            vec![AssistantSegment::Code {
                language: "python".to_string(),
                body: "print('hi')\n".to_string(),
            }]
        );
    }

    #[test]
    fn mixed_text_and_code_segments_are_preserved() {
        let segments = parse_assistant_segments(
            "before\n```python\nprint('hi')\n```\nafter",
            1,
            0,
            TranscriptSurface::Chat,
        );
        assert_eq!(segments.len(), 3);
        assert!(matches!(segments[0], AssistantSegment::Text(_)));
        assert!(matches!(segments[1], AssistantSegment::Code { .. }));
        assert!(matches!(segments[2], AssistantSegment::Text(_)));
    }

    #[test]
    fn incomplete_fence_stays_code() {
        let segments =
            parse_assistant_segments("```python\nprint('hi')", 1, 0, TranscriptSurface::Chat);
        assert_eq!(segments.len(), 1);
        assert!(matches!(segments[0], AssistantSegment::Code { .. }));
    }

    #[test]
    fn reasoning_and_code_both_classify() {
        let segments = parse_assistant_segments(
            "<think>step by step</think>\n```python\nprint('hi')\n```",
            1,
            0,
            TranscriptSurface::Chat,
        );
        assert!(matches!(segments[0], AssistantSegment::Think(_)));
        assert!(matches!(segments[1], AssistantSegment::Text(_)));
        assert!(matches!(segments[2], AssistantSegment::Code { .. }));
    }

    #[test]
    fn render_produces_code_box_without_fences() {
        let theme = Theme::core_tui();
        let lines = render_assistant_segments(
            &[AssistantSegment::Code {
                language: "python".to_string(),
                body: "print('hi')".to_string(),
            }],
            &theme,
            SegmentRenderOptions::shell(),
        );
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("python"));
        assert!(rendered.contains("print"));
        assert!(!rendered.contains("```"));
    }

    #[test]
    fn parse_read_file_tag() {
        let segments = parse_assistant_segments(
            "<read_file path=\"src/main.rs\"></read_file>",
            1,
            0,
            TranscriptSurface::Chat,
        );
        assert_eq!(
            segments,
            vec![AssistantSegment::ReadFile {
                path: "src/main.rs".to_string(),
                range: None,
            }]
        );
    }

    #[test]
    fn parse_read_file_tag_with_range() {
        let segments = parse_assistant_segments(
            "<read_file path=\"src/main.rs\" start_line=\"390\" end_line=\"450\"></read_file>",
            1,
            0,
            TranscriptSurface::Chat,
        );
        assert_eq!(
            segments,
            vec![AssistantSegment::ReadFile {
                path: "src/main.rs".to_string(),
                range: Some(ReadFileRange {
                    start_line: 390,
                    end_line: 450,
                }),
            }]
        );
    }

    #[test]
    fn parse_list_directory_tag() {
        let segments = parse_assistant_segments(
            "intro <list_directory path=\"src\"></list_directory> outro",
            1,
            0,
            TranscriptSurface::Chat,
        );
        assert_eq!(segments.len(), 3);
        assert!(matches!(segments[0], AssistantSegment::Text(_)));
        assert!(matches!(
            segments[1],
            AssistantSegment::ListDirectory { .. }
        ));
        assert!(matches!(segments[2], AssistantSegment::Text(_)));
    }

    #[test]
    fn parse_write_file_tag() {
        let segments = parse_assistant_segments(
            "<write_file path=\"notes.md\">hello\nworld</write_file>",
            1,
            0,
            TranscriptSurface::Chat,
        );
        assert_eq!(
            segments,
            vec![AssistantSegment::WriteFile {
                path: "notes.md".to_string(),
                content: "hello\nworld".to_string(),
            }]
        );
    }

    #[test]
    fn parse_apply_patch_tag() {
        let segments = parse_assistant_segments(
            "before<apply_patch path=\"notes.md\">diff replacement</apply_patch>after",
            1,
            0,
            TranscriptSurface::Chat,
        );
        assert_eq!(segments.len(), 3);
        assert!(matches!(segments[0], AssistantSegment::Text(_)));
        assert!(matches!(segments[1], AssistantSegment::ApplyPatch { .. }));
        assert!(matches!(segments[2], AssistantSegment::Text(_)));
        let patch = match &segments[1] {
            AssistantSegment::ApplyPatch { patch, .. } => patch.clone(),
            _ => String::new(),
        };
        assert_eq!(patch, "diff replacement");
    }

    #[test]
    fn parse_mcp_call_tool_tag() {
        let segments = parse_assistant_segments(
            r#"before<mcp_call_tool server_name="docs" tool_name="search">{"query":"validation"}</mcp_call_tool>after"#,
            1,
            0,
            TranscriptSurface::Chat,
        );
        assert_eq!(segments.len(), 3);
        assert!(matches!(segments[1], AssistantSegment::McpCallTool { .. }));
        match &segments[1] {
            AssistantSegment::McpCallTool {
                server_name,
                tool_name,
                arguments,
            } => {
                assert_eq!(server_name, "docs");
                assert_eq!(tool_name, "search");
                assert_eq!(arguments["query"], "validation");
            }
            _ => panic!("unexpected segment"),
        }
    }

    #[test]
    fn malformed_mcp_call_tool_tag_falls_back_to_text() {
        let segments = parse_assistant_segments(
            r#"<mcp_call_tool server_name="docs" tool_name="search">{not json}</mcp_call_tool>"#,
            1,
            0,
            TranscriptSurface::Chat,
        );
        assert_eq!(segments.len(), 1);
        assert_eq!(
            segments[0],
            AssistantSegment::Text(
                r#"<mcp_call_tool server_name="docs" tool_name="search">{not json}</mcp_call_tool>"#
                    .to_string()
            )
        );
    }

    #[test]
    fn malformed_tool_tags_fallback_to_text() {
        let segments = parse_assistant_segments(
            "bad <read_file path=\"src.rs\">oops<apply_patch></apply_patch>",
            1,
            0,
            TranscriptSurface::Chat,
        );
        assert!(!segments.is_empty());
        assert!(matches!(segments[0], AssistantSegment::Text(_)));
        let all_text: String = segments
            .iter()
            .filter_map(|segment| match segment {
                AssistantSegment::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            all_text,
            "bad <read_file path=\"src.rs\">oops<apply_patch></apply_patch>"
        );
    }

    #[test]
    fn malformed_tags_without_required_attributes_fallback_to_text() {
        let segments = parse_assistant_segments(
            "<read_file><write_file path=\"notes.md\">ignored</write_file>",
            1,
            0,
            TranscriptSurface::Chat,
        );
        assert_eq!(segments.len(), 1);
        assert!(matches!(segments[0], AssistantSegment::Text(_)));
        let all_text: String = segments
            .iter()
            .filter_map(|segment| match segment {
                AssistantSegment::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            all_text,
            "<read_file><write_file path=\"notes.md\">ignored</write_file>"
        );
    }

    #[test]
    fn malformed_missing_close_fallback_to_text() {
        let segments = parse_assistant_segments(
            "<list_directory path=\"src\">",
            1,
            0,
            TranscriptSurface::Chat,
        );
        assert_eq!(segments.len(), 1);
        assert!(matches!(segments[0], AssistantSegment::Text(_)));
        assert_eq!(
            segments[0],
            AssistantSegment::Text("<list_directory path=\"src\">".to_string())
        );
    }
}

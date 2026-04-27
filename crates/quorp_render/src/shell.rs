use unicode_width::UnicodeWidthStr;

use crate::caps::ColorCapability;
use crate::palette::{
    ACCENT_CYAN, ACCENT_GREEN, ACCENT_RED, ACCENT_VIOLET, ACCENT_YELLOW, BOLD, DIM, FG_TEXT, RESET,
    Rgb,
};
use crate::shimmer::{ShimmerStyle, render_shimmer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptItem {
    System {
        text: String,
    },
    User {
        text: String,
    },
    Assistant {
        text: String,
        streaming: bool,
    },
    Thinking {
        label: String,
    },
    Command {
        command: String,
        cwd: String,
        output_tail: Vec<String>,
        status: ToolStatus,
    },
    Error {
        title: String,
        detail: String,
    },
    Receipt {
        text: String,
        success: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    Queued,
    Running,
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTurn {
    pub label: String,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerView {
    pub prompt: String,
    pub buffer: String,
    pub blink_on: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    pub left: String,
    pub center: String,
    pub right: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellOverlay {
    SlashPalette {
        entries: Vec<PaletteRow>,
        selected: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteRow {
    pub value: String,
    pub detail: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellFrame {
    pub transcript: Vec<TranscriptItem>,
    pub live_turn: Option<LiveTurn>,
    pub composer: ComposerView,
    pub status: StatusLine,
    pub overlay: Option<ShellOverlay>,
}

pub fn render_shell_frame(frame: &ShellFrame, width: usize, color: ColorCapability) -> Vec<String> {
    let width = width.max(48);
    let mut lines = Vec::new();
    for item in &frame.transcript {
        render_item(&mut lines, item, width, color);
    }
    if let Some(live_turn) = &frame.live_turn {
        lines.push(render_live_turn(live_turn, color));
    }
    lines
}

pub fn render_shell_overlay(
    overlay: &Option<ShellOverlay>,
    width: usize,
    color: ColorCapability,
) -> Vec<String> {
    let Some(ShellOverlay::SlashPalette { entries, selected }) = overlay else {
        return Vec::new();
    };
    entries
        .iter()
        .take(8)
        .enumerate()
        .map(|(index, entry)| {
            let selector = if index == *selected { ">" } else { " " };
            let line = format!(
                "  {selector} {:<18} {:<12} {}",
                entry.value, entry.detail, entry.description
            );
            let line = truncate_to_width(&line, width);
            if matches!(color, ColorCapability::NoColor) {
                return line;
            }
            let selector_color = if index == *selected {
                ACCENT_YELLOW
            } else {
                ACCENT_CYAN
            };
            format!(
                "{}{}{} {}{:<18}{} {}{:<12}{} {}{}{}",
                selector_color.fg(),
                selector,
                RESET,
                ACCENT_CYAN.fg(),
                entry.value,
                RESET,
                ACCENT_VIOLET.fg(),
                entry.detail,
                RESET,
                FG_TEXT.fg(),
                truncate_to_width(&entry.description, width.saturating_sub(36)),
                RESET
            )
        })
        .collect()
}

pub fn render_composer(composer: &ComposerView, color: ColorCapability) -> String {
    if matches!(color, ColorCapability::NoColor) {
        return format!("{} {}", composer.prompt, composer.buffer);
    }
    let prompt = if composer.blink_on {
        format!("{}{}{}{}", ACCENT_CYAN.fg(), BOLD, composer.prompt, RESET)
    } else {
        format!("{}{}{}", DIM, composer.prompt, RESET)
    };
    format!("{prompt} {}", composer.buffer)
}

pub fn render_status_line(status: &StatusLine, width: usize, color: ColorCapability) -> String {
    let width = width.max(48);
    let right_width = UnicodeWidthStr::width(status.right.as_str());
    let left_center = if status.center.trim().is_empty() {
        status.left.clone()
    } else {
        format!("{}  {}", status.left, status.center)
    };
    let available = width.saturating_sub(right_width.saturating_add(2));
    let left_center = truncate_to_width(&left_center, available);
    let spacer =
        " ".repeat(width.saturating_sub(
            UnicodeWidthStr::width(left_center.as_str()).saturating_add(right_width),
        ));
    if matches!(color, ColorCapability::NoColor) {
        return format!("{left_center}{spacer}{}", status.right);
    }
    format!(
        "{}{}{}{}{}{}{}",
        DIM,
        left_center,
        RESET,
        spacer,
        ACCENT_VIOLET.fg(),
        status.right,
        RESET
    )
}

fn render_item(
    lines: &mut Vec<String>,
    item: &TranscriptItem,
    width: usize,
    color: ColorCapability,
) {
    match item {
        TranscriptItem::System { text } => {
            lines.push(paint(&format!("  {text}"), color, ACCENT_CYAN));
        }
        TranscriptItem::User { text } => {
            lines.push(paint(
                &format!("› {}", truncate_to_width(text, width - 2)),
                color,
                FG_TEXT,
            ));
        }
        TranscriptItem::Assistant { text, streaming } => {
            let glyph = if *streaming { "●" } else { " " };
            for (index, line) in wrap_lines(text, width.saturating_sub(4)).iter().enumerate() {
                let prefix = if index == 0 { glyph } else { " " };
                lines.push(format!(
                    "{} {}",
                    paint(prefix, color, ACCENT_CYAN),
                    paint(line, color, FG_TEXT)
                ));
            }
        }
        TranscriptItem::Thinking { label } => {
            lines.push(format!(
                "{} {}",
                paint("●", color, ACCENT_CYAN),
                paint(label, color, FG_TEXT)
            ));
        }
        TranscriptItem::Command {
            command,
            cwd,
            output_tail,
            status,
        } => {
            let (glyph, rgb) = match status {
                ToolStatus::Queued => ("◇", ACCENT_VIOLET),
                ToolStatus::Running => ("▶", ACCENT_CYAN),
                ToolStatus::Passed => ("✓", ACCENT_GREEN),
                ToolStatus::Failed => ("✕", ACCENT_RED),
            };
            lines.push(format!(
                "{} {}",
                paint(glyph, color, rgb),
                paint(
                    &truncate_to_width(command, width.saturating_sub(3)),
                    color,
                    FG_TEXT
                )
            ));
            if !cwd.trim().is_empty() {
                lines.push(format!(
                    "  {}",
                    paint(&format!("cwd {cwd}"), color, ACCENT_VIOLET)
                ));
            }
            for output in output_tail.iter().rev().take(4).rev() {
                lines.push(format!(
                    "  {}",
                    paint(
                        &truncate_to_width(output, width.saturating_sub(2)),
                        color,
                        FG_TEXT
                    )
                ));
            }
        }
        TranscriptItem::Error { title, detail } => {
            lines.push(format!(
                "{} {}",
                paint("✕", color, ACCENT_RED),
                paint(
                    &truncate_to_width(title, width.saturating_sub(3)),
                    color,
                    ACCENT_RED
                )
            ));
            if !detail.trim().is_empty() {
                lines.push(format!(
                    "  {}",
                    paint(
                        &truncate_to_width(detail, width.saturating_sub(2)),
                        color,
                        FG_TEXT
                    )
                ));
            }
        }
        TranscriptItem::Receipt { text, success } => {
            let (glyph, rgb) = if *success {
                ("✓", ACCENT_GREEN)
            } else {
                ("✕", ACCENT_RED)
            };
            lines.push(format!(
                "{} {}",
                paint(glyph, color, rgb),
                paint(
                    &truncate_to_width(text, width.saturating_sub(3)),
                    color,
                    FG_TEXT
                )
            ));
        }
    }
}

fn render_live_turn(live_turn: &LiveTurn, color: ColorCapability) -> String {
    let seconds = live_turn.elapsed_ms as f32 / 1000.0;
    if matches!(color, ColorCapability::NoColor) {
        return format!("● {} {:.1}s", live_turn.label, seconds);
    }
    format!(
        "{} {}",
        render_shimmer("●", seconds, ShimmerStyle::default(), color),
        paint(
            &format!("{} {:.1}s", live_turn.label, seconds),
            color,
            FG_TEXT
        )
    )
}

fn paint(text: &str, color: ColorCapability, rgb: Rgb) -> String {
    if matches!(color, ColorCapability::NoColor) {
        text.to_string()
    } else {
        format!("{}{}{}", rgb.fg(), text, RESET)
    }
}

fn wrap_lines(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for raw_line in text.lines() {
        let mut current = String::new();
        for word in raw_line.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if UnicodeWidthStr::width(candidate.as_str()) > width && !current.is_empty() {
                lines.push(current);
                current = word.to_string();
            } else {
                current = candidate;
            }
        }
        if current.is_empty() {
            lines.push(String::new());
        } else {
            lines.push(current);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for value in text.chars() {
        let char_width = unicode_width::UnicodeWidthChar::width(value).unwrap_or(0);
        if used + char_width >= width {
            break;
        }
        out.push(value);
        used += char_width;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composer_uses_chevron_not_quorp_prompt() {
        let rendered = render_composer(
            &ComposerView {
                prompt: ">".to_string(),
                buffer: "/plan".to_string(),
                blink_on: true,
            },
            ColorCapability::NoColor,
        );
        assert_eq!(rendered, "> /plan");
        assert!(!rendered.contains("quorp>"));
    }

    #[test]
    fn slash_overlay_renders_palette_rows() {
        let lines = render_shell_overlay(
            &Some(ShellOverlay::SlashPalette {
                selected: 0,
                entries: vec![PaletteRow {
                    value: "/plan".to_string(),
                    detail: "command".to_string(),
                    description: "Enter plan mode".to_string(),
                }],
            }),
            80,
            ColorCapability::NoColor,
        );
        assert_eq!(
            lines[0],
            "  > /plan              command      Enter plan mode"
        );
    }
}

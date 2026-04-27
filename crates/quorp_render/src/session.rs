//! Stream-first session scene rendering.
//!
//! These primitives are still scrollback-native: they produce committed
//! terminal text plus one active command frame that callers can repaint in
//! place while the command is running.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::caps::ColorCapability;
use crate::palette::{
    ACCENT_CYAN, ACCENT_GREEN, ACCENT_RED, ACCENT_VIOLET, ACCENT_YELLOW, BOLD, DIM, FG_TEXT, RESET,
    Rgb,
};
use crate::shimmer::{ShimmerStyle, render_shimmer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Pending,
    Active,
    Done,
    Warn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub label: String,
    pub state: TaskState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandState {
    Pending,
    Active { frame_time: f32 },
    Passed { exit_code: i32, duration: String },
    Failed { exit_code: i32, duration: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandCard {
    pub label: String,
    pub command: String,
    pub cwd: String,
    pub state: CommandState,
    pub output_summary: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionFrame {
    pub title: String,
    pub subtitle: String,
    pub tasks: Vec<TaskRow>,
    pub commands: Vec<CommandCard>,
    pub footer: String,
}

pub fn render_session_frame(frame: &SessionFrame, width: usize, color: ColorCapability) -> String {
    let width = width.max(48);
    let mut out = String::new();
    out.push_str(&render_brand_header(frame, width, color));
    if !frame.tasks.is_empty() {
        out.push('\n');
        out.push_str(&render_task_list(&frame.tasks, width, color));
    }
    for command in &frame.commands {
        out.push('\n');
        out.push_str(&render_command_card(command, width, color));
    }
    if !frame.footer.trim().is_empty() {
        out.push('\n');
        out.push_str(&render_footer(&frame.footer, width, color));
    }
    out
}

pub fn render_command_card(command: &CommandCard, width: usize, color: ColorCapability) -> String {
    let width = width.max(48);
    let inner_width = width.saturating_sub(4);
    let plain = matches!(color, ColorCapability::NoColor);
    let (top_left, top_right, bottom_left, bottom_right, horizontal, vertical) = if plain {
        ("+", "+", "+", "+", "-", "|")
    } else {
        ("╭", "╮", "╰", "╯", "─", "│")
    };
    let border_color = match command.state {
        CommandState::Pending => ACCENT_VIOLET,
        CommandState::Active { .. } => ACCENT_CYAN,
        CommandState::Passed { .. } => ACCENT_GREEN,
        CommandState::Failed { .. } => ACCENT_RED,
    };

    let mut out = String::new();
    out.push_str(&paint(
        &format!("{top_left}{}{top_right}", horizontal.repeat(width - 2)),
        color,
        border_color,
    ));
    out.push('\n');

    let state_label = render_command_state(&command.state, color);
    let header = format!("{}  {}", command.label, state_label);
    out.push_str(&card_line(
        &header,
        inner_width,
        vertical,
        color,
        border_color,
    ));
    out.push('\n');

    let command_text = match command.state {
        CommandState::Active { frame_time } if !plain => {
            render_shimmer(&command.command, frame_time, ShimmerStyle::default(), color)
        }
        _ => paint(&command.command, color, FG_TEXT),
    };
    out.push_str(&card_line_styled(
        "$ ",
        &command_text,
        inner_width,
        vertical,
        color,
        border_color,
    ));
    out.push('\n');

    let cwd = format!("cwd {}", command.cwd);
    out.push_str(&card_line(&cwd, inner_width, vertical, color, border_color));
    out.push('\n');

    if !command.output_summary.trim().is_empty() {
        out.push_str(&card_line(
            &command.output_summary,
            inner_width,
            vertical,
            color,
            border_color,
        ));
        out.push('\n');
    }

    out.push_str(&paint(
        &format!(
            "{bottom_left}{}{bottom_right}",
            horizontal.repeat(width - 2)
        ),
        color,
        border_color,
    ));
    out
}

fn render_brand_header(frame: &SessionFrame, width: usize, color: ColorCapability) -> String {
    let plain = matches!(color, ColorCapability::NoColor);
    let mut out = String::new();
    let wordmark = "QUORP";
    let title = if frame.title.trim().is_empty() {
        "terminal agent"
    } else {
        frame.title.as_str()
    };
    let subtitle = truncate_to_width(&frame.subtitle, width.saturating_sub(4));
    if plain {
        out.push_str(&format!("{wordmark} // {title}\n{subtitle}\n"));
        out.push_str(&"-".repeat(width));
        return out;
    }

    out.push_str(&ACCENT_YELLOW.fg());
    out.push_str(BOLD);
    out.push_str(wordmark);
    out.push_str(RESET);
    out.push_str(&ACCENT_CYAN.fg());
    out.push_str(" // ");
    out.push_str(RESET);
    out.push_str(&FG_TEXT.fg());
    out.push_str(title);
    out.push_str(RESET);
    out.push('\n');
    out.push_str(DIM);
    out.push_str(&subtitle);
    out.push_str(RESET);
    out.push('\n');
    out.push_str(&paint(&"━".repeat(width), color, ACCENT_VIOLET));
    out
}

fn render_task_list(tasks: &[TaskRow], width: usize, color: ColorCapability) -> String {
    let mut out = String::new();
    out.push_str(&paint("task list", color, ACCENT_CYAN));
    for task in tasks {
        out.push('\n');
        let (glyph, rgb) = match task.state {
            TaskState::Pending => ("·", ACCENT_VIOLET),
            TaskState::Active => ("*", ACCENT_CYAN),
            TaskState::Done => ("✓", ACCENT_GREEN),
            TaskState::Warn => ("!", ACCENT_YELLOW),
        };
        let label = truncate_to_width(&task.label, width.saturating_sub(6));
        if matches!(color, ColorCapability::NoColor) {
            out.push_str(&format!("  {glyph} {label}"));
        } else {
            out.push_str("  ");
            out.push_str(&paint(glyph, color, rgb));
            out.push(' ');
            out.push_str(&paint(&label, color, FG_TEXT));
        }
    }
    out
}

fn render_footer(footer: &str, width: usize, color: ColorCapability) -> String {
    let footer = truncate_to_width(footer, width);
    if matches!(color, ColorCapability::NoColor) {
        footer
    } else {
        format!("{}{}{}", ACCENT_VIOLET.fg(), footer, RESET)
    }
}

fn render_command_state(state: &CommandState, color: ColorCapability) -> String {
    match state {
        CommandState::Pending => paint("pending", color, ACCENT_VIOLET),
        CommandState::Active { frame_time } => {
            render_shimmer("running", *frame_time, ShimmerStyle::default(), color)
        }
        CommandState::Passed {
            exit_code,
            duration,
        } => paint(
            &format!("passed exit={exit_code} {duration}"),
            color,
            ACCENT_GREEN,
        ),
        CommandState::Failed {
            exit_code,
            duration,
        } => paint(
            &format!("failed exit={exit_code} {duration}"),
            color,
            ACCENT_RED,
        ),
    }
}

fn card_line(
    text: &str,
    inner_width: usize,
    vertical: &str,
    color: ColorCapability,
    border_color: Rgb,
) -> String {
    let text = paint(
        &truncate_to_width(text, inner_width.saturating_sub(2)),
        color,
        FG_TEXT,
    );
    card_line_styled("", &text, inner_width, vertical, color, border_color)
}

fn card_line_styled(
    prefix: &str,
    styled_text: &str,
    inner_width: usize,
    vertical: &str,
    color: ColorCapability,
    border_color: Rgb,
) -> String {
    let visible_width = prefix.width() + strip_ansi_width(styled_text);
    let padding = inner_width.saturating_sub(visible_width);
    let mut out = String::new();
    out.push_str(&paint(vertical, color, border_color));
    out.push(' ');
    out.push_str(prefix);
    out.push_str(styled_text);
    out.push_str(&" ".repeat(padding));
    out.push(' ');
    out.push_str(&paint(vertical, color, border_color));
    out
}

fn paint(text: &str, color: ColorCapability, rgb: Rgb) -> String {
    if matches!(color, ColorCapability::NoColor) {
        text.to_string()
    } else {
        format!("{}{}{}", rgb.fg(), text, RESET)
    }
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width <= 1 {
        return "…".to_string();
    }
    let target = max_width - 1;
    let mut out = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let char_width = ch.width().unwrap_or(0);
        if width + char_width > target {
            break;
        }
        out.push(ch);
        width += char_width;
    }
    out.push('…');
    out
}

fn strip_ansi_width(text: &str) -> usize {
    let mut width = 0;
    let mut in_escape = false;
    for ch in text.chars() {
        if in_escape {
            if ch == 'm' {
                in_escape = false;
            }
            continue;
        }
        if ch == '\x1b' {
            in_escape = true;
            continue;
        }
        width += ch.width().unwrap_or(0);
    }
    width
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_frame() -> SessionFrame {
        SessionFrame {
            title: "brilliant terminal coding".into(),
            subtitle: "agent-first Rust runtime · truecolor stream · sandboxed tools".into(),
            tasks: vec![
                TaskRow {
                    label: "plan verification gates".into(),
                    state: TaskState::Done,
                },
                TaskRow {
                    label: "run strict proof lane".into(),
                    state: TaskState::Active,
                },
            ],
            commands: vec![CommandCard {
                label: "verification".into(),
                command: "cargo test --workspace --lib".into(),
                cwd: "/repo".into(),
                state: CommandState::Active { frame_time: 0.0 },
                output_summary: "421 tests queued · first failures will pin exact spans".into(),
            }],
            footer: "qwen3-coder@nvidia · yolo sandbox · ctx 12.4k/64k".into(),
        }
    }

    #[test]
    fn no_color_session_frame_is_stable() {
        let rendered = render_session_frame(&sample_frame(), 64, ColorCapability::NoColor);
        assert_eq!(
            rendered,
            "QUORP // brilliant terminal coding\nagent-first Rust runtime · truecolor stream · sandboxed too…\n----------------------------------------------------------------\ntask list\n  ✓ plan verification gates\n  * run strict proof lane\n+--------------------------------------------------------------+\n| verification  ⠋ running                                      |\n| $ cargo test --workspace --lib                               |\n| cwd /repo                                                    |\n| 421 tests queued · first failures will pin exact spans       |\n+--------------------------------------------------------------+\nqwen3-coder@nvidia · yolo sandbox · ctx 12.4k/64k"
        );
    }

    #[test]
    fn truecolor_session_frame_contains_brand_and_shimmer() {
        let rendered = render_session_frame(&sample_frame(), 72, ColorCapability::TrueColor);
        assert!(rendered.contains("\x1b[38;2"));
        assert!(rendered.contains("QUORP"));
        assert!(rendered.contains("421 tests queued"));
        assert!(rendered.contains("421 tests queued"));
        assert!(rendered.ends_with("\x1b[0m"));
    }

    #[test]
    fn command_card_width_is_stable_across_state_changes() {
        let mut command = CommandCard {
            label: "build".into(),
            command: "cargo check --workspace".into(),
            cwd: "/repo".into(),
            state: CommandState::Active { frame_time: 0.0 },
            output_summary: "checking crates".into(),
        };
        let active = render_command_card(&command, 60, ColorCapability::NoColor);
        command.state = CommandState::Passed {
            exit_code: 0,
            duration: "2.7s".into(),
        };
        let passed = render_command_card(&command, 60, ColorCapability::NoColor);
        for line in active.lines().chain(passed.lines()) {
            assert_eq!(line.width(), 60);
        }
    }
}

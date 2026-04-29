//! Scrollback "promote" rendering — turning ephemeral live-region output
//! into committed scrollback lines (assistant prose, tool-call summaries,
//! repair sub-loops).

use crate::caps::ColorCapability;
use crate::palette::{ACCENT_CYAN, DIM, FG_TEXT, RESET};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptLine {
    UserPrompt(String),
    AssistantProse(String),
    ToolCallSummary {
        tool: String,
        target: String,
        sample_chars: u32,
    },
    RepairAttempt {
        attempt: u8,
        cap: u8,
        hypothesis: String,
    },
}

pub fn render_transcript_line(line: &TranscriptLine, color: ColorCapability) -> String {
    let plain = matches!(color, ColorCapability::NoColor);
    match line {
        TranscriptLine::UserPrompt(text) => {
            if plain {
                format!("> {text}")
            } else {
                format!(
                    "{cyan}❯{reset} {text}",
                    cyan = ACCENT_CYAN.fg(),
                    reset = RESET
                )
            }
        }
        TranscriptLine::AssistantProse(text) => {
            if plain {
                text.clone()
            } else {
                format!("{fg}{text}{reset}", fg = FG_TEXT.fg(), reset = RESET)
            }
        }
        TranscriptLine::ToolCallSummary {
            tool,
            target,
            sample_chars,
        } => {
            let core = format!("⌐ {tool} {target} ({sample_chars} chars)");
            if plain {
                core
            } else {
                format!("{dim}{core}{reset}", dim = DIM, reset = RESET)
            }
        }
        TranscriptLine::RepairAttempt {
            attempt,
            cap,
            hypothesis,
        } => {
            let core = format!("  ↳ repair attempt {attempt}/{cap} — {hypothesis}");
            if plain {
                core
            } else {
                format!("{dim}{core}{reset}", dim = DIM, reset = RESET)
            }
        }
    }
}
#[cfg(test)]
#[path = "../../../testing/quorp_render/transcript/tests.rs"]
mod tests;

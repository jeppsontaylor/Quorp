//! Permission modal — the impossible-to-miss approval card shown when a
//! tool action requires user confirmation.

use crate::caps::ColorCapability;
use crate::palette::{ACCENT_RED, ACCENT_YELLOW, FG_TEXT, RESET};

#[derive(Debug, Clone)]
pub struct PermissionPrompt {
    pub tool: String,
    pub command_repr: String,
    pub cwd: String,
    pub sandbox: String,
    pub rationale: String,
}

pub fn render_permission_modal(prompt: &PermissionPrompt, color: ColorCapability) -> String {
    let mut out = String::new();
    let header = "approve action";
    let line = "─".repeat(header.len() + 4);
    if matches!(color, ColorCapability::NoColor) {
        out.push_str(&format!("┌── {header} ──{line}\n"));
        out.push_str(&format!("│ {} {}\n", prompt.tool, prompt.command_repr));
        out.push_str(&format!(
            "│ cwd: {} · sandbox: {}\n",
            prompt.cwd, prompt.sandbox
        ));
        out.push_str(&format!("│ rationale: {}\n", prompt.rationale));
        out.push_str("│\n");
        out.push_str("│ [y] approve once   [a] always for command pattern\n");
        out.push_str("│ [t] always for tool   [n] deny   [e] edit\n");
        out.push_str(&format!("└──────────{line}\n"));
        return out;
    }

    out.push_str(&ACCENT_YELLOW.fg());
    out.push_str(&format!("┌── {header} ──{line}\n"));
    out.push_str("│ ");
    out.push_str(&FG_TEXT.fg());
    out.push_str(&format!("{} {}\n", prompt.tool, prompt.command_repr));
    out.push_str(&ACCENT_YELLOW.fg());
    out.push_str("│ ");
    out.push_str(&FG_TEXT.fg());
    out.push_str(&format!(
        "cwd: {} · sandbox: {}\n",
        prompt.cwd, prompt.sandbox
    ));
    out.push_str(&ACCENT_YELLOW.fg());
    out.push_str("│ ");
    out.push_str(&FG_TEXT.fg());
    out.push_str(&format!("rationale: {}\n", prompt.rationale));
    out.push_str(&ACCENT_YELLOW.fg());
    out.push_str("│\n");
    out.push_str("│ ");
    out.push_str(&FG_TEXT.fg());
    out.push_str("[y] approve once   [a] always for command pattern\n");
    out.push_str(&ACCENT_YELLOW.fg());
    out.push_str("│ ");
    out.push_str(&FG_TEXT.fg());
    out.push_str("[t] always for tool   ");
    out.push_str(&ACCENT_RED.fg());
    out.push_str("[n] deny");
    out.push_str(&FG_TEXT.fg());
    out.push_str("   [e] edit\n");
    out.push_str(&ACCENT_YELLOW.fg());
    out.push_str(&format!("└──────────{line}\n"));
    out.push_str(RESET);
    out
}
#[cfg(test)]
#[path = "../../../testing/quorp_render/permission_modal/tests.rs"]
mod tests;

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
mod tests {
    use super::*;

    fn sample() -> PermissionPrompt {
        PermissionPrompt {
            tool: "run_command".into(),
            command_repr: "cargo test -p quorp_term".into(),
            cwd: "crates/quorp_term".into(),
            sandbox: "tmp-copy".into(),
            rationale: "validate slash parser".into(),
        }
    }

    #[test]
    fn no_color_includes_options() {
        let s = render_permission_modal(&sample(), ColorCapability::NoColor);
        assert!(s.contains("[y] approve once"));
        assert!(s.contains("[n] deny"));
        assert!(s.contains("cargo test"));
    }

    #[test]
    fn truecolor_uses_red_for_deny() {
        let s = render_permission_modal(&sample(), ColorCapability::TrueColor);
        assert!(s.contains("\x1b[38;2"));
        assert!(s.contains("[n] deny"));
    }
}

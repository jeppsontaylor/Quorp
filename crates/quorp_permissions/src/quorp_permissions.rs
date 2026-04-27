//! Permission policy engine for Quorp.
//!
//! Five modes — read-only, ask, accept-edits, auto-safe, yolo-sandbox —
//! gate every mutating tool action through `Permissions::check`.

use std::collections::BTreeMap;

use globset::{Glob, GlobMatcher};
use quorp_core::PermissionMode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    ReadOnly,
    Ask,
    AcceptEdits,
    AutoSafe,
    YoloSandbox,
}

impl Mode {
    pub fn from_legacy(legacy: PermissionMode) -> Self {
        match legacy {
            PermissionMode::Ask => Mode::Ask,
            PermissionMode::FullAuto => Mode::AutoSafe,
            PermissionMode::FullPermissions => Mode::YoloSandbox,
        }
    }
}

/// What a tool action wants to do, classified for the permission engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Read,
    WriteFile,
    DeleteFile,
    RunCommand,
    Network,
    Mcp,
}

#[derive(Debug, Clone)]
pub struct Action {
    pub capability: Capability,
    pub tool_name: String,
    pub command_repr: Option<String>,
}

impl Action {
    pub fn new(
        capability: Capability,
        tool_name: impl Into<String>,
        command_repr: Option<String>,
    ) -> Self {
        Self {
            capability,
            tool_name: tool_name.into(),
            command_repr,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AllowPolicy {
    Once,
    AlwaysSession,
    AlwaysProject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowEntry {
    pub pattern: String,
    pub policy: AllowPolicy,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AllowList {
    /// Glob patterns matched against the rendered command string.
    pub commands: Vec<AllowEntry>,
    /// Tool names that are universally allowed (e.g. read_file).
    pub tools: BTreeMap<String, AllowPolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    PromptUser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionSurface {
    Host,
    Sandbox,
}

#[derive(Debug)]
pub struct Permissions {
    pub mode: Mode,
    pub allow: AllowList,
    compiled_command_globs: Vec<(GlobMatcher, AllowPolicy)>,
}

impl Permissions {
    pub fn new(mode: Mode, allow: AllowList) -> Self {
        let compiled_command_globs = allow
            .commands
            .iter()
            .filter_map(|entry| {
                Glob::new(&entry.pattern)
                    .ok()
                    .map(|glob| (glob.compile_matcher(), entry.policy.clone()))
            })
            .collect();
        Self {
            mode,
            allow,
            compiled_command_globs,
        }
    }

    pub fn check(&self, action: &Action) -> Decision {
        self.check_on_surface(action, ExecutionSurface::Sandbox)
    }

    pub fn check_on_surface(&self, action: &Action, surface: ExecutionSurface) -> Decision {
        match self.mode {
            Mode::YoloSandbox => {
                if surface == ExecutionSurface::Sandbox {
                    Decision::Allow
                } else {
                    Decision::Deny
                }
            }
            Mode::ReadOnly => match action.capability {
                Capability::Read => Decision::Allow,
                _ => Decision::Deny,
            },
            Mode::AutoSafe | Mode::AcceptEdits | Mode::Ask => {
                if matches!(action.capability, Capability::Read) {
                    return Decision::Allow;
                }
                if let Some(policy) = self.allow.tools.get(&action.tool_name)
                    && policy != &AllowPolicy::Once
                {
                    return Decision::Allow;
                }
                if let Some(cmd) = action.command_repr.as_deref()
                    && self
                        .compiled_command_globs
                        .iter()
                        .any(|(g, _)| g.is_match(cmd))
                {
                    return Decision::Allow;
                }
                if self.mode == Mode::AcceptEdits
                    && matches!(
                        action.capability,
                        Capability::WriteFile | Capability::DeleteFile
                    )
                {
                    return Decision::Allow;
                }
                if self.mode == Mode::AutoSafe && matches!(action.capability, Capability::WriteFile)
                {
                    return Decision::Allow;
                }
                Decision::PromptUser
            }
        }
    }
}

pub fn classify_tool_action(
    tool_name: &str,
    command_repr: Option<String>,
    path_hint: Option<&str>,
) -> Action {
    let normalized_tool = tool_name.trim().to_ascii_lowercase();
    let capability = match normalized_tool.as_str() {
        "read_file"
        | "list_directory"
        | "search_text"
        | "search_symbols"
        | "find_files"
        | "structural_search"
        | "cargo_diagnostics"
        | "get_repo_capsule"
        | "explain_validation_failure"
        | "suggest_implementation_targets"
        | "suggest_edit_anchors" => Capability::Read,
        "write_file"
        | "apply_patch"
        | "replace_block"
        | "replace_range"
        | "set_executable"
        | "preview_edit"
        | "apply_preview"
        | "modify_toml"
        | "structural_edit_preview" => Capability::WriteFile,
        "delete" | "delete_file" => Capability::DeleteFile,
        "run_command" | "run_validation" => classify_command_capability(command_repr.as_deref()),
        "mcp_call_tool" => Capability::Mcp,
        _ if path_hint.is_some() => Capability::WriteFile,
        _ => Capability::RunCommand,
    };
    Action::new(capability, normalized_tool, command_repr)
}

fn classify_command_capability(command: Option<&str>) -> Capability {
    let Some(command) = command.map(str::trim).filter(|command| !command.is_empty()) else {
        return Capability::RunCommand;
    };
    let first_word = command
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(|ch| matches!(ch, '\'' | '"'));
    match first_word {
        "curl" | "wget" | "ssh" | "scp" | "rsync" | "nc" | "ncat" | "telnet" => Capability::Network,
        _ => Capability::RunCommand,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_action() -> Action {
        Action {
            capability: Capability::Read,
            tool_name: "read_file".into(),
            command_repr: None,
        }
    }

    fn run_test_action() -> Action {
        Action::new(
            Capability::RunCommand,
            "run_command",
            Some("cargo test -p quorp_term".into()),
        )
    }

    #[test]
    fn read_only_allows_reads_and_blocks_writes() {
        let p = Permissions::new(Mode::ReadOnly, AllowList::default());
        assert_eq!(p.check(&read_action()), Decision::Allow);
        assert_eq!(p.check(&run_test_action()), Decision::Deny);
    }

    #[test]
    fn ask_prompts_for_unknown_command() {
        let p = Permissions::new(Mode::Ask, AllowList::default());
        assert_eq!(p.check(&run_test_action()), Decision::PromptUser);
    }

    #[test]
    fn allowlist_glob_skips_prompt() {
        let mut allow = AllowList::default();
        allow.commands.push(AllowEntry {
            pattern: "cargo test*".into(),
            policy: AllowPolicy::AlwaysSession,
        });
        let p = Permissions::new(Mode::Ask, allow);
        assert_eq!(p.check(&run_test_action()), Decision::Allow);
    }

    #[test]
    fn yolo_allows_everything() {
        let p = Permissions::new(Mode::YoloSandbox, AllowList::default());
        assert_eq!(p.check(&run_test_action()), Decision::Allow);
    }

    #[test]
    fn yolo_denies_host_surface() {
        let p = Permissions::new(Mode::YoloSandbox, AllowList::default());
        assert_eq!(
            p.check_on_surface(&run_test_action(), ExecutionSurface::Host),
            Decision::Deny
        );
        assert_eq!(
            p.check_on_surface(&run_test_action(), ExecutionSurface::Sandbox),
            Decision::Allow
        );
    }

    #[test]
    fn classifier_marks_network_commands() {
        let action = classify_tool_action(
            "run_command",
            Some("curl https://example.com".to_string()),
            None,
        );
        assert_eq!(action.capability, Capability::Network);
    }

    #[test]
    fn classifier_marks_read_and_write_tools() {
        assert_eq!(
            classify_tool_action("read_file", None, Some("src/main.rs")).capability,
            Capability::Read
        );
        assert_eq!(
            classify_tool_action("replace_block", None, Some("src/main.rs")).capability,
            Capability::WriteFile
        );
    }
}

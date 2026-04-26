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
        Self { mode, allow, compiled_command_globs }
    }

    pub fn check(&self, action: &Action) -> Decision {
        match self.mode {
            Mode::YoloSandbox => Decision::Allow,
            Mode::ReadOnly => match action.capability {
                Capability::Read => Decision::Allow,
                _ => Decision::Deny,
            },
            Mode::AutoSafe | Mode::AcceptEdits | Mode::Ask => {
                if matches!(action.capability, Capability::Read) {
                    return Decision::Allow;
                }
                if let Some(policy) = self.allow.tools.get(&action.tool_name) {
                    if policy != &AllowPolicy::Once {
                        return Decision::Allow;
                    }
                }
                if let Some(cmd) = action.command_repr.as_deref() {
                    if self.compiled_command_globs.iter().any(|(g, _)| g.is_match(cmd)) {
                        return Decision::Allow;
                    }
                }
                if self.mode == Mode::AcceptEdits
                    && matches!(action.capability, Capability::WriteFile | Capability::DeleteFile)
                {
                    return Decision::Allow;
                }
                if self.mode == Mode::AutoSafe
                    && matches!(action.capability, Capability::WriteFile)
                {
                    return Decision::Allow;
                }
                Decision::PromptUser
            }
        }
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
        Action {
            capability: Capability::RunCommand,
            tool_name: "run_command".into(),
            command_repr: Some("cargo test -p quorp_term".into()),
        }
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
}

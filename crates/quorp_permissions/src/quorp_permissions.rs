//! Permission policy engine for Quorp.
//!
//! Five modes — read-only, ask, accept-edits, auto-safe, yolo-sandbox —
//! gate every mutating tool action through `Permissions::check`.

use std::collections::BTreeMap;
use std::path::Path;

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
    Browser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityToken {
    ShellMeta,
    CompoundCommand,
    FilesystemWrite,
    Network,
    DependencyInstall,
    Docker,
    GitRemoteMutation,
    FindDelete,
    FindExec,
    SecretsRead,
    GeneratedExecutable,
    Mcp,
    Browser,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommandPolicyInput {
    pub argv: Vec<String>,
    pub shell_meta: Vec<String>,
    pub wrappers: Vec<String>,
    pub env_assignments: Vec<String>,
}

impl ParsedCommandPolicyInput {
    pub fn parse(command: &str) -> Self {
        let mut shell_meta = detect_shell_meta(command);
        let mut wrappers = Vec::new();
        let mut env_assignments = Vec::new();
        let mut argv = shlex::split(command).unwrap_or_default();

        if argv.is_empty() {
            return Self {
                argv,
                shell_meta,
                wrappers,
                env_assignments,
            };
        }

        while let Some(program) = argv.first().cloned() {
            match program.as_str() {
                "env" => {
                    wrappers.push("env".to_string());
                    argv.remove(0);
                    while argv.first().is_some_and(|arg| arg.contains('=')) {
                        let assignment = argv.remove(0);
                        env_assignments.push(assignment);
                    }
                }
                "sh" | "bash" | "zsh" | "fish" => {
                    if argv.get(1).is_some_and(|arg| arg == "-c") {
                        wrappers.push(format!("{program} -c"));
                        shell_meta.push(format!("{program} -c"));
                        let nested = argv.get(2).cloned().unwrap_or_default();
                        let mut nested_input = Self::parse(&nested);
                        wrappers.append(&mut nested_input.wrappers);
                        env_assignments.append(&mut nested_input.env_assignments);
                        shell_meta.append(&mut nested_input.shell_meta);
                        argv = nested_input.argv;
                    }
                    break;
                }
                "xargs" => {
                    wrappers.push("xargs".to_string());
                    break;
                }
                _ => break,
            }
        }

        Self {
            argv,
            shell_meta,
            wrappers,
            env_assignments,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Action {
    pub capability: Capability,
    pub tool_name: String,
    pub command_repr: Option<String>,
    pub command_input: Option<ParsedCommandPolicyInput>,
    pub tokens: Vec<CapabilityToken>,
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
            command_input: None,
            tokens: Vec::new(),
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
                        .any(|(glob, _)| glob.is_match(cmd))
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
                if self.mode == Mode::AutoSafe {
                    if matches!(action.capability, Capability::WriteFile) {
                        return Decision::Allow;
                    }
                    if matches!(action.capability, Capability::RunCommand)
                        && action.tokens.is_empty()
                    {
                        return Decision::Allow;
                    }
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
    let mut action = match normalized_tool.as_str() {
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
        | "suggest_edit_anchors" => Action::new(Capability::Read, normalized_tool, command_repr),
        "write_file"
        | "apply_patch"
        | "replace_block"
        | "replace_range"
        | "set_executable"
        | "preview_edit"
        | "apply_preview"
        | "modify_toml"
        | "structural_edit_preview" => {
            Action::new(Capability::WriteFile, normalized_tool, command_repr)
        }
        "delete" | "delete_file" => {
            Action::new(Capability::DeleteFile, normalized_tool, command_repr)
        }
        "mcp_call_tool" => {
            let mut action = Action::new(Capability::Mcp, normalized_tool, command_repr);
            action.tokens.push(CapabilityToken::Mcp);
            action
        }
        tool if tool.starts_with("browser_") => {
            let mut action = Action::new(Capability::Browser, normalized_tool, command_repr);
            action.tokens.push(CapabilityToken::Browser);
            action
        }
        _ if path_hint.is_some() => {
            Action::new(Capability::WriteFile, normalized_tool, command_repr)
        }
        _ => classify_command_action(normalized_tool, command_repr),
    };
    action.tokens.sort();
    action.tokens.dedup();
    action
}

fn classify_command_action(tool_name: String, command_repr: Option<String>) -> Action {
    let mut action = Action::new(Capability::RunCommand, tool_name, command_repr.clone());
    let Some(command) = command_repr else {
        return action;
    };
    let parsed = ParsedCommandPolicyInput::parse(&command);
    let mut tokens = classify_command_tokens(&parsed);
    if tokens.contains(&CapabilityToken::Network) {
        action.capability = Capability::Network;
    } else if tokens.contains(&CapabilityToken::Browser) {
        action.capability = Capability::Browser;
    }
    tokens.sort();
    tokens.dedup();
    action.command_input = Some(parsed);
    action.tokens = tokens;
    action
}

fn classify_command_tokens(parsed: &ParsedCommandPolicyInput) -> Vec<CapabilityToken> {
    let mut tokens = Vec::new();
    if !parsed.shell_meta.is_empty() {
        tokens.push(CapabilityToken::ShellMeta);
        if parsed.shell_meta.iter().any(|meta| is_compound_meta(meta)) {
            tokens.push(CapabilityToken::CompoundCommand);
        }
        if parsed
            .shell_meta
            .iter()
            .any(|meta| matches!(meta.as_str(), ">" | ">>" | "<" | "2>" | "2>>"))
        {
            tokens.push(CapabilityToken::FilesystemWrite);
        }
    }

    let Some(program) = parsed.argv.first().map(String::as_str) else {
        return tokens;
    };
    let args = &parsed.argv[1..];
    let argv_programs = parsed.argv.iter().map(String::as_str).collect::<Vec<_>>();

    if argv_programs
        .iter()
        .any(|program| is_network_program(program))
    {
        tokens.push(CapabilityToken::Network);
    }
    if is_browser_program(program, args) {
        tokens.push(CapabilityToken::Browser);
    }
    if matches!(program, "docker" | "podman") {
        tokens.push(CapabilityToken::Docker);
    }
    if is_dependency_install(program, args) {
        tokens.push(CapabilityToken::DependencyInstall);
        tokens.push(CapabilityToken::FilesystemWrite);
    }
    if is_filesystem_write_program(program, args) {
        tokens.push(CapabilityToken::FilesystemWrite);
    }
    if is_git_remote_mutation(program, args) {
        tokens.push(CapabilityToken::GitRemoteMutation);
        tokens.push(CapabilityToken::Network);
    }
    if program == "find" && args.iter().any(|arg| arg == "-delete") {
        tokens.push(CapabilityToken::FindDelete);
        tokens.push(CapabilityToken::FilesystemWrite);
    }
    if program == "find" && args.iter().any(|arg| arg == "-exec") {
        tokens.push(CapabilityToken::FindExec);
    }
    if is_secrets_read(program, args) {
        tokens.push(CapabilityToken::SecretsRead);
    }
    if is_generated_executable(program, args) {
        tokens.push(CapabilityToken::GeneratedExecutable);
    }

    tokens
}

fn detect_shell_meta(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = command.chars().collect();
    let mut index = 0usize;
    while index < chars.len() {
        match chars[index] {
            ';' => tokens.push(";".to_string()),
            '|' => {
                if chars.get(index + 1) == Some(&'|') {
                    tokens.push("||".to_string());
                    index += 1;
                } else {
                    tokens.push("|".to_string());
                }
            }
            '&' if chars.get(index + 1) == Some(&'&') => {
                tokens.push("&&".to_string());
                index += 1;
            }
            '>' => {
                if chars.get(index + 1) == Some(&'>') {
                    tokens.push(">>".to_string());
                    index += 1;
                } else {
                    tokens.push(">".to_string());
                }
            }
            '<' => tokens.push("<".to_string()),
            '$' if chars.get(index + 1) == Some(&'(') => {
                tokens.push("$(".to_string());
            }
            _ => {}
        }
        index += 1;
    }
    tokens
}

fn is_compound_meta(meta: &str) -> bool {
    matches!(meta, ";" | "&&" | "||" | "|" | "$(")
}

fn is_network_program(program: &str) -> bool {
    matches!(
        program,
        "curl" | "wget" | "ssh" | "scp" | "rsync" | "nc" | "ncat" | "telnet"
    )
}

fn is_dependency_install(program: &str, args: &[String]) -> bool {
    match program {
        "cargo" => args
            .first()
            .is_some_and(|subcommand| matches!(subcommand.as_str(), "add" | "install")),
        "npm" | "pnpm" => args
            .first()
            .is_some_and(|subcommand| matches!(subcommand.as_str(), "install" | "add" | "i")),
        "yarn" => args
            .first()
            .is_some_and(|subcommand| matches!(subcommand.as_str(), "add" | "install")),
        "pip" | "pip3" => args
            .first()
            .is_some_and(|subcommand| subcommand == "install"),
        "uv" => args.windows(2).any(|window| {
            window.first().is_some_and(|arg| arg == "pip")
                && window.get(1).is_some_and(|arg| arg == "install")
        }),
        "brew" | "apt" | "apt-get" => args
            .first()
            .is_some_and(|subcommand| subcommand == "install"),
        _ => false,
    }
}

fn is_filesystem_write_program(program: &str, args: &[String]) -> bool {
    matches!(
        program,
        "rm" | "mv" | "cp" | "tee" | "touch" | "mkdir" | "chmod" | "chown" | "ln" | "install"
    ) || (program == "git"
        && args.first().is_some_and(|subcommand| {
            matches!(subcommand.as_str(), "checkout" | "restore" | "apply")
        }))
}

fn is_git_remote_mutation(program: &str, args: &[String]) -> bool {
    if program != "git" {
        return false;
    }
    matches!(
        args,
        [subcommand, action, ..]
            if subcommand == "remote" && matches!(action.as_str(), "add" | "set-url" | "remove")
    ) || args.first().is_some_and(|subcommand| {
        matches!(subcommand.as_str(), "push" | "fetch" | "pull" | "clone")
    })
}

fn is_browser_program(program: &str, args: &[String]) -> bool {
    matches!(
        program,
        "open" | "xdg-open" | "playwright" | "chromium" | "google-chrome"
    ) || args.iter().any(|arg| {
        arg.starts_with("http://") || arg.starts_with("https://") || arg.starts_with("file://")
    })
}

fn is_secrets_read(program: &str, args: &[String]) -> bool {
    matches!(
        program,
        "cat" | "less" | "more" | "sed" | "awk" | "rg" | "grep"
    ) && args.iter().any(|arg| looks_like_secret_path(arg))
}

fn is_generated_executable(program: &str, args: &[String]) -> bool {
    if program.starts_with("./") || program.starts_with("/tmp/") || program.starts_with("target/") {
        return true;
    }
    matches!(program, "python" | "python3" | "node" | "bash" | "sh")
        && args
            .first()
            .is_some_and(|arg| looks_like_generated_script_path(arg))
}

fn looks_like_secret_path(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.ends_with(".env")
        || lower.contains("id_rsa")
        || lower.ends_with(".pem")
        || lower.ends_with(".p12")
        || lower.contains("credentials")
        || lower.contains("secret")
}

fn looks_like_generated_script_path(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    (lower.starts_with("/tmp/")
        || lower.starts_with("target/")
        || lower.starts_with("build/")
        || lower.starts_with("dist/")
        || lower.starts_with("./target/")
        || lower.starts_with("./build/"))
        && Path::new(value)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "sh" | "py" | "js" | "pl" | "rb"))
}
#[cfg(test)]
#[path = "../../../testing/quorp_permissions/quorp_permissions/tests.rs"]
mod tests;

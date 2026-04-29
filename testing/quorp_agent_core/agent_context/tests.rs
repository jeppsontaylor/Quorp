use super::*;
use crate::agent_protocol::AgentAction;
use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            match self.previous.as_ref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn base_config() -> AgentConfig {
    AgentConfig {
        validation: ValidationCommands {
            fmt_command: Some("cargo fmt --check".to_string()),
            clippy_command: Some("cargo clippy --all-targets --no-deps -- -D warnings".to_string()),
            workspace_test_command: Some("cargo test".to_string()),
            targeted_test_prefix: Some("cargo test ".to_string()),
        },
        ..AgentConfig::default()
    }
}

#[test]
fn mcp_approval_rule_can_scope_to_server_and_tool() {
    let mut config = base_config();
    config.approval_rules.push(ApprovalRule {
        action: "mcp_call_tool".to_string(),
        path_prefix: None,
        command_prefix: None,
        mcp_server_name: Some("docs".to_string()),
        mcp_tool_name: Some("search".to_string()),
        policy: ActionApprovalPolicy::AutoApproveReadOnly,
    });

    let matching = AgentAction::McpCallTool {
        server_name: "docs".to_string(),
        tool_name: "search".to_string(),
        arguments: serde_json::json!({"query":"validation"}),
    };
    let wrong_tool = AgentAction::McpCallTool {
        server_name: "docs".to_string(),
        tool_name: "fetch".to_string(),
        arguments: serde_json::json!({"id":1}),
    };

    assert_eq!(
        effective_approval_policy(&matching, &config),
        ActionApprovalPolicy::AutoApproveReadOnly
    );
    assert_eq!(
        effective_approval_policy(&wrong_tool, &config),
        ActionApprovalPolicy::RequireExplicitConfirmation
    );
}

#[test]
fn mcp_approval_rule_without_tool_matches_entire_server() {
    let mut config = base_config();
    config.approval_rules.push(ApprovalRule {
        action: "mcp_call_tool".to_string(),
        path_prefix: None,
        command_prefix: None,
        mcp_server_name: Some("filesystem".to_string()),
        mcp_tool_name: None,
        policy: ActionApprovalPolicy::AutoApproveReadOnly,
    });

    let matching = AgentAction::McpCallTool {
        server_name: "filesystem".to_string(),
        tool_name: "read_text_file".to_string(),
        arguments: serde_json::json!({"path":"README.md"}),
    };
    let other_server = AgentAction::McpCallTool {
        server_name: "docs".to_string(),
        tool_name: "read_text_file".to_string(),
        arguments: serde_json::json!({"path":"README.md"}),
    };

    assert_eq!(
        effective_approval_policy(&matching, &config),
        ActionApprovalPolicy::AutoApproveReadOnly
    );
    assert_eq!(
        effective_approval_policy(&other_server, &config),
        ActionApprovalPolicy::RequireExplicitConfirmation
    );
}

#[test]
fn validation_commands_do_not_prepend_prefix_to_full_commands() {
    let commands = validation_commands_for_plan(
        &base_config(),
        &ValidationPlan {
            fmt: false,
            clippy: false,
            workspace_tests: false,
            tests: vec!["cargo test -p toy-domain --quiet".to_string()],
            custom_commands: Vec::new(),
        },
    );
    assert_eq!(
        commands,
        vec!["cargo test -p toy-domain --quiet".to_string()]
    );
}

#[test]
fn validation_commands_still_prefix_targeted_selectors() {
    let commands = validation_commands_for_plan(
        &base_config(),
        &ValidationPlan {
            fmt: false,
            clippy: false,
            workspace_tests: false,
            tests: vec!["-p toy-domain --quiet".to_string()],
            custom_commands: Vec::new(),
        },
    );
    assert_eq!(
        commands,
        vec!["cargo test -p toy-domain --quiet".to_string()]
    );
}

#[test]
fn agent_tools_global_settings_parse_and_missing_file_defaults() {
    let _env_lock = env_lock();
    let temp_home = tempfile::tempdir().expect("home");
    let _home_guard = EnvVarGuard::set("HOME", temp_home.path());

    let missing = load_agent_config(temp_home.path());
    assert!(!missing.agent_tools.enabled);

    std::fs::create_dir_all(temp_home.path().join(".quorp")).expect("config dir");
    std::fs::write(
        temp_home.path().join(".quorp/settings.json"),
        r#"{
              "agent_tools": {
                "enabled": true,
                "tools": {
                  "fd": {"enabled": true, "command": "fd"},
                  "ast_grep": {
                    "enabled": true,
                    "command": "ast-grep",
                    "allow_rewrite_preview": true,
                    "allow_apply": false
                  },
                  "browser": {
                    "enabled": true,
                    "command": "node",
                    "args": ["-e"],
                    "max_runtime_seconds": 45,
                    "max_output_bytes": 4096
                  },
                  "cargo_diagnostics": {
                    "enabled": true,
                    "check_command": "cargo check --message-format=json"
                  }
                }
              }
            }"#,
    )
    .expect("settings");

    let parsed = load_agent_config(temp_home.path());
    assert!(parsed.agent_tools.enabled);
    assert_eq!(parsed.agent_tools.fd.command, "fd");
    assert!(parsed.agent_tools.ast_grep.allow_rewrite_preview);
    assert!(!parsed.agent_tools.ast_grep.allow_apply);
    assert!(parsed.agent_tools.browser.enabled);
    assert_eq!(parsed.agent_tools.browser.command, "node");
    assert_eq!(parsed.agent_tools.browser.args, vec!["-e".to_string()]);
    assert_eq!(
        parsed.agent_tools.cargo_diagnostics.check_command,
        "cargo check --message-format=json"
    );
}

#[test]
fn agent_tools_project_settings_can_narrow_global_tools() {
    let _env_lock = env_lock();
    let temp_home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let _home_guard = EnvVarGuard::set("HOME", temp_home.path());
    std::fs::create_dir_all(temp_home.path().join(".quorp")).expect("home config");
    std::fs::write(
        temp_home.path().join(".quorp/settings.json"),
        r#"{
              "agent_tools": {
                "enabled": true,
                "tools": {
                "ast_grep": {
                    "enabled": true,
                    "command": "ast-grep",
                    "allow_rewrite_preview": true,
                    "allow_apply": true
                  },
                  "browser": {
                    "enabled": false,
                    "command": "node"
                  }
                }
              }
            }"#,
    )
    .expect("settings");
    std::fs::create_dir_all(project.path().join(".quorp")).expect("project config");
    std::fs::write(
        project.path().join(".quorp/agent.toml"),
        r#"
[agent_tools]
enabled = true

[agent_tools.tools.ast_grep]
allow_rewrite_preview = false
allow_apply = true

[agent_tools.tools.browser]
enabled = true
command = "node"
args = ["-e"]
"#,
    )
    .expect("agent toml");

    let config = load_agent_config(project.path());
    assert!(config.agent_tools.enabled);
    assert!(!config.agent_tools.ast_grep.allow_rewrite_preview);
    assert!(config.agent_tools.ast_grep.allow_apply);
    assert!(config.agent_tools.browser.enabled);

    std::fs::write(
        project.path().join(".quorp/agent.toml"),
        r#"
[agent_tools]
enabled = false

[agent_tools.tools.browser]
enabled = false
"#,
    )
    .expect("agent toml");
    let narrowed = load_agent_config(project.path());
    assert!(!narrowed.agent_tools.enabled);
    assert!(!narrowed.agent_tools.browser.enabled);
}

#[test]
fn agent_tools_nextest_command_requires_cargo_subcommand_binary() {
    let _env_lock = env_lock();
    let temp_path = tempfile::tempdir().expect("path");
    let cargo = temp_path.path().join("cargo");
    std::fs::write(&cargo, "#!/bin/sh\nexit 0\n").expect("cargo");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let _path_guard = EnvVarGuard::set("PATH", temp_path.path());
    assert!(command_is_available("cargo check --message-format=json"));
    assert!(!command_is_available("cargo nextest run"));
    let cargo_nextest = temp_path.path().join("cargo-nextest");
    std::fs::write(&cargo_nextest, "#!/bin/sh\nexit 0\n").expect("cargo-nextest");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cargo_nextest, std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
    }
    assert!(command_is_available("cargo nextest run"));
}

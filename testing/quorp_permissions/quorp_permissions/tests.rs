use super::*;

fn read_action() -> Action {
    Action {
        capability: Capability::Read,
        tool_name: "read_file".into(),
        command_repr: None,
        command_input: None,
        tokens: Vec::new(),
    }
}

fn run_test_action() -> Action {
    classify_tool_action("run_command", Some("cargo test -p quorp_term".into()), None)
}

#[test]
fn read_only_allows_reads_and_blocks_writes() {
    let permissions = Permissions::new(Mode::ReadOnly, AllowList::default());
    assert_eq!(permissions.check(&read_action()), Decision::Allow);
    assert_eq!(permissions.check(&run_test_action()), Decision::Deny);
}

#[test]
fn ask_prompts_for_unknown_command() {
    let permissions = Permissions::new(Mode::Ask, AllowList::default());
    assert_eq!(permissions.check(&run_test_action()), Decision::PromptUser);
}

#[test]
fn allowlist_glob_skips_prompt() {
    let mut allow = AllowList::default();
    allow.commands.push(AllowEntry {
        pattern: "cargo test*".into(),
        policy: AllowPolicy::AlwaysSession,
    });
    let permissions = Permissions::new(Mode::Ask, allow);
    assert_eq!(permissions.check(&run_test_action()), Decision::Allow);
}

#[test]
fn yolo_allows_everything() {
    let permissions = Permissions::new(Mode::YoloSandbox, AllowList::default());
    assert_eq!(permissions.check(&run_test_action()), Decision::Allow);
}

#[test]
fn yolo_denies_host_surface() {
    let permissions = Permissions::new(Mode::YoloSandbox, AllowList::default());
    assert_eq!(
        permissions.check_on_surface(&run_test_action(), ExecutionSurface::Host),
        Decision::Deny
    );
    assert_eq!(
        permissions.check_on_surface(&run_test_action(), ExecutionSurface::Sandbox),
        Decision::Allow
    );
}

#[test]
fn auto_safe_allows_plain_test_command() {
    let permissions = Permissions::new(Mode::AutoSafe, AllowList::default());
    assert_eq!(permissions.check(&run_test_action()), Decision::Allow);
}

#[test]
fn classifier_marks_network_commands() {
    let action = classify_tool_action(
        "run_command",
        Some("curl https://example.com".to_string()),
        None,
    );
    assert_eq!(action.capability, Capability::Network);
    assert!(action.tokens.contains(&CapabilityToken::Network));
}

#[test]
fn classifier_marks_compound_and_network_tokens() {
    let action = classify_tool_action(
        "run_command",
        Some("cargo test && curl https://example.com".to_string()),
        None,
    );
    assert!(action.tokens.contains(&CapabilityToken::CompoundCommand));
    assert!(action.tokens.contains(&CapabilityToken::ShellMeta));
    assert!(action.tokens.contains(&CapabilityToken::Network));
}

#[test]
fn classifier_marks_find_delete_and_find_exec() {
    let delete_action =
        classify_tool_action("run_command", Some("find . -delete".to_string()), None);
    assert!(delete_action.tokens.contains(&CapabilityToken::FindDelete));

    let exec_action = classify_tool_action(
        "run_command",
        Some("find . -name '*.rs' -exec rm {} \\;".to_string()),
        None,
    );
    assert!(exec_action.tokens.contains(&CapabilityToken::FindExec));
}

#[test]
fn classifier_marks_git_remote_mutation_separately_from_git_status() {
    let safe = classify_tool_action("run_command", Some("git status".to_string()), None);
    assert!(safe.tokens.is_empty());

    let remote = classify_tool_action(
        "run_command",
        Some("git remote add origin https://example.com/repo.git".to_string()),
        None,
    );
    assert!(remote.tokens.contains(&CapabilityToken::GitRemoteMutation));
    assert!(remote.tokens.contains(&CapabilityToken::Network));
}

#[test]
fn classifier_marks_dependency_install_and_generated_executable() {
    let install = classify_tool_action("run_command", Some("cargo add anyhow".to_string()), None);
    assert!(install.tokens.contains(&CapabilityToken::DependencyInstall));

    let generated = classify_tool_action(
        "run_command",
        Some("python /tmp/quorp/generated.py".to_string()),
        None,
    );
    assert!(
        generated
            .tokens
            .contains(&CapabilityToken::GeneratedExecutable)
    );
}

#[test]
fn auto_safe_prompts_for_risky_commands() {
    let permissions = Permissions::new(Mode::AutoSafe, AllowList::default());
    let risky = classify_tool_action("run_command", Some("find . -delete".to_string()), None);
    assert_eq!(permissions.check(&risky), Decision::PromptUser);
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

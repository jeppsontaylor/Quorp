use super::*;

#[test]
fn parses_quorp_slash_commands() {
    assert_eq!(parse_slash_command("/plan"), Some(SlashCommand::Plan));
    assert_eq!(parse_slash_command("/help"), Some(SlashCommand::Help));
    assert_eq!(
        parse_slash_command("/sandbox tmp-copy"),
        Some(SlashCommand::Sandbox(Some(SandboxMode::TmpCopy)))
    );
}

#[test]
fn mode_commands_mutate_agent_state() {
    let mut run_mode = RunMode::Act;
    let mut permission_mode = PermissionMode::Ask;
    let mut sandbox = SandboxMode::Host;

    apply_mode_command(
        &SlashCommand::FullPermissions,
        &mut run_mode,
        &mut permission_mode,
        &mut sandbox,
    );
    apply_mode_command(
        &SlashCommand::Sandbox(Some(SandboxMode::TmpCopy)),
        &mut run_mode,
        &mut permission_mode,
        &mut sandbox,
    );

    assert_eq!(permission_mode, PermissionMode::FullPermissions);
    assert_eq!(sandbox, SandboxMode::TmpCopy);
}

#[test]
fn renders_validation_with_colorful_running_frames() {
    assert_eq!(
        render_validation("cargo test", ValidationStatus::Running, 2),
        "\x1b[36m| validating\x1b[0m cargo test"
    );
    assert_eq!(
        render_validation("cargo test", ValidationStatus::Passed, 0),
        "\x1b[32m+ validated\x1b[0m cargo test"
    );
}

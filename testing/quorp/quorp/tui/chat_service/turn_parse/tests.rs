use super::*;

#[test]
fn parses_lsp_definition_tool_call() {
    let action = parse_action_from_arguments(
        "lsp_definition",
        &serde_json::json!({
            "path": "src/lib.rs",
            "symbol": "Example",
            "line": 12,
            "character": 4
        }),
    )
    .expect("action");
    assert!(matches!(
        action,
        AgentAction::LspDefinition {
            path,
            symbol,
            line: Some(12),
            character: Some(4)
        } if path == "src/lib.rs" && symbol == "Example"
    ));
}

#[test]
fn parses_pseudo_lsp_hover_line() {
    let call = pseudo_tool_call_from_line("LspHover path: src/lib.rs line: 10 character: 3")
        .expect("tool call");
    let actions = parse_actions_from_tool_call(&call).expect("actions");
    assert!(matches!(
        actions.as_slice(),
        [AgentAction::LspHover {
            path,
            line: 10,
            character: 3
        }] if path == "src/lib.rs"
    ));
}

#[test]
fn parses_pseudo_process_start_line() {
    let call = pseudo_tool_call_from_line(
        "ProcessStart command: cargo args: [\"test\", \"--quiet\"] cwd: /tmp/project",
    )
    .expect("tool call");
    let actions = parse_actions_from_tool_call(&call).expect("actions");
    assert!(matches!(
        actions.as_slice(),
        [AgentAction::ProcessStart {
            command,
            args,
            cwd: Some(cwd)
        }] if command == "cargo"
            && matches!(args.as_slice(), [first, second] if first == "test" && second == "--quiet")
            && cwd == "/tmp/project"
    ));
}

#[test]
fn parses_pseudo_browser_open_line() {
    let call = pseudo_tool_call_from_line(
        "BrowserOpen url: https://example.com headless: true width: 1280 height: 720",
    )
    .expect("tool call");
    let actions = parse_actions_from_tool_call(&call).expect("actions");
    assert!(matches!(
        actions.as_slice(),
        [AgentAction::BrowserOpen {
            url,
            headless: true,
            width: Some(1280),
            height: Some(720)
        }] if url == "https://example.com"
    ));
}

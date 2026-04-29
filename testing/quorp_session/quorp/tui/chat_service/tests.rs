use super::*;

#[test]
fn build_system_prompt_includes_context_pack_section() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let workspace = temp_dir.path();
    std::fs::create_dir_all(workspace.join("agent")).expect("agent dir");
    std::fs::write(
        workspace.join("agent/owner-map.json"),
        r#"{
          "version": 1,
          "owners": [{
            "id": "session",
            "paths": ["src/**"],
            "responsibility": "session runtime"
          }]
        }"#,
    )
    .expect("owner map");

    let request = StreamRequest {
        request_id: 1,
        session_id: 1,
        model_id: "qwen/qwen3-coder-480b-a35b-instruct".to_string(),
        agent_mode: AgentMode::Act,
        latest_input: "improve the context compiler".to_string(),
        messages: vec![ChatServiceMessage {
            role: ChatServiceRole::User,
            content: "hello".to_string(),
        }],
        project_root: workspace.to_path_buf(),
        base_url_override: None,
        max_completion_tokens: Some(128),
        include_repo_capsule: false,
        disable_reasoning: true,
        native_tool_calls: true,
        watchdog: None,
        safety_mode_label: None,
        prompt_compaction_policy: None,
        capture_scope: None,
        capture_call_class: None,
    };

    let prompt = build_system_prompt(&request);

    assert!(prompt.contains("Compiled context pack:"));
    assert!(prompt.contains("pack_id="));
    assert!(prompt.contains("query \"improve the context compiler\""));
}

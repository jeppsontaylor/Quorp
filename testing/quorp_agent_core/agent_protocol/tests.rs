use super::{AgentAction, stable_content_hash};

#[test]
fn stable_content_hash_normalizes_line_endings() {
    assert_eq!(
        stable_content_hash("one\ntwo\n"),
        stable_content_hash("one\r\ntwo\r\n")
    );
    assert_eq!(
        stable_content_hash("one\ntwo\n"),
        stable_content_hash("one\rtwo\r")
    );
}

#[test]
fn new_actions_round_trip() {
    let actions = vec![
        AgentAction::ExpandContext {
            handle: "handle-1".to_string(),
        },
        AgentAction::RecallMemory {
            query: "token budget".to_string(),
            limit: 3,
        },
        AgentAction::ProposeRule {
            statement: "Prefer narrow repairs".to_string(),
            error_code: Some("E0001".to_string()),
            evidence: Some("validation output".to_string()),
        },
    ];

    for action in actions {
        let json = serde_json::to_string(&action).expect("serialize");
        let decoded: AgentAction = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.tool_name(), action.tool_name());
    }
}

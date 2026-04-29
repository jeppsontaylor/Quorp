use super::*;

#[test]
fn rule_round_trips() {
    let rule = Rule {
        id: RuleId::new("rb-001"),
        state: RuleState::Candidate,
        scope: Scope::Repo,
        statement: "do not borrow vec across loop body".into(),
        effect: RuleEffect::PromptRule,
        pattern: RulePattern {
            trigger: Trigger {
                error_code: Some("E0382".into()),
                symbol_path_prefix: None,
                message_skeleton: None,
                ast_kind: Some("loop_expression".into()),
            },
            min_cluster_count: 2,
            min_confidence: 0.6,
        },
        confidence: 0.7,
        created_at_unix: 0,
        updated_at_unix: 0,
        verified_for_runs: 0,
        false_positive_runs: 0,
    };
    let json = serde_json::to_string(&rule).unwrap();
    let back: Rule = serde_json::from_str(&json).unwrap();
    assert_eq!(back.id, rule.id);
}

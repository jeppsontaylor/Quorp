use super::{PromptMessage, PromptMessageRole, apply_prompt_compaction};

#[test]
fn current_default_prunes_long_tool_output() {
    let mut messages = vec![PromptMessage {
        role: PromptMessageRole::User,
        content: "goal".to_string(),
    }];
    messages.push(PromptMessage {
        role: PromptMessageRole::User,
        content: format!(
            "[Tool Output]\n{}",
            (0..40)
                .map(|index| format!("line {index}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    });
    for index in 0..9 {
        messages.push(PromptMessage {
            role: PromptMessageRole::Assistant,
            content: format!("message {index}"),
        });
    }

    let compacted = apply_prompt_compaction(&messages, None);
    assert_eq!(compacted.len(), messages.len());
    assert!(
        compacted[1]
            .content
            .contains("lines pruned for context length")
    );
}

#[test]
fn last6_policy_adds_user_ledger() {
    let messages = (0..8)
        .map(|index| PromptMessage {
            role: if index % 2 == 0 {
                PromptMessageRole::User
            } else {
                PromptMessageRole::Assistant
            },
            content: format!("message {index} {}", "x".repeat(1_500)),
        })
        .collect::<Vec<_>>();

    let compacted = apply_prompt_compaction(
        &messages,
        Some(quorp_agent_core::PromptCompactionPolicy::Last6Ledger768),
    );
    assert_eq!(compacted[0].role, PromptMessageRole::User);
    assert!(compacted[0].content.contains("Condensed prior context:"));
    assert!(
        compacted[0]
            .content
            .starts_with("[Compacted Prior Context]")
    );
    assert_eq!(compacted.len(), 7);
}

#[test]
fn benchmark_repair_minimal_keeps_patch_packet_without_full_history() {
    let messages = vec![
        PromptMessage {
            role: PromptMessageRole::User,
            content: "Goal: fix chrono\nFull objective text that should not repeat".to_string(),
        },
        PromptMessage {
            role: PromptMessageRole::Assistant,
            content: "I will inspect a lot of files".to_string(),
        },
        PromptMessage {
            role: PromptMessageRole::User,
            content: "[Patch Packet]\nOwner path: src/round.rs\nPatch target: Cargo.toml\nRepair write locked: true\nLast validation failure: failed\nMissing dependencies: chrono, uuid\nTarget dependency table: [dev-dependencies]\nObserved target content_hash: abc123\nRequired next action: modify_toml Cargo.toml [dev-dependencies]\nRecommended rerun command: cargo test --quiet\nMinimal JSON example: {}".to_string(),
        },
    ];

    let compacted = apply_prompt_compaction(
        &messages,
        Some(quorp_agent_core::PromptCompactionPolicy::BenchmarkRepairMinimal),
    );

    assert_eq!(compacted.len(), 1);
    assert!(compacted[0].content.contains("[Benchmark Repair Minimal]"));
    assert!(compacted[0].content.contains("Goal: fix chrono"));
    assert!(compacted[0].content.contains("Patch target: Cargo.toml"));
    assert!(
        compacted[0]
            .content
            .contains("Missing dependencies: chrono, uuid")
    );
    assert!(
        compacted[0]
            .content
            .contains("Observed target content_hash: abc123")
    );
    assert!(
        !compacted[0]
            .content
            .contains("I will inspect a lot of files")
    );
}

#[test]
fn benchmark_state_packet_keeps_required_state_and_caps_context() {
    let messages = vec![
        PromptMessage {
            role: PromptMessageRole::User,
            content: "Goal: fix chrono\nFull objective text that should not repeat".to_string(),
        },
        PromptMessage {
            role: PromptMessageRole::Assistant,
            content: "I will inspect a lot of files".to_string(),
        },
        PromptMessage {
            role: PromptMessageRole::User,
            content: format!(
                "[Patch Packet]\nOwner path: src/round.rs\nRequired next action: write_patch src/round.rs\nLast validation failure: failed\nAgent scorecard: parser_recovery=1 line_tools=1 injected_reads=1 redundant_reads=0 first_write=none repeated_edits=0\nAllowed actions: `ApplyPatch`, `WriteFile`, or ranged `ReplaceBlock`\nRecommended rerun command: cargo test --quiet\nImplementation slice:\n{}",
                (0..160)
                    .map(|index| format!("line {index}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        },
    ];

    let compacted = apply_prompt_compaction(
        &messages,
        Some(quorp_agent_core::PromptCompactionPolicy::BenchmarkStatePacket),
    );

    assert_eq!(compacted.len(), 1);
    assert!(compacted[0].content.contains("[Benchmark State Packet]"));
    assert!(
        compacted[0]
            .content
            .contains("Required next action: write_patch src/round.rs")
    );
    assert!(compacted[0].content.contains("Agent scorecard:"));
    assert!(
        !compacted[0]
            .content
            .contains("I will inspect a lot of files")
    );
    assert!(!compacted[0].content.contains("line 159"));
}

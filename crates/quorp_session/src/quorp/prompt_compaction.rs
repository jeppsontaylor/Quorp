use quorp_agent_core::PromptCompactionPolicy;

const DEFAULT_COMPACT_THRESHOLD_TOKENS: u64 = 2_000;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PromptMessageRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PromptMessage {
    pub role: PromptMessageRole,
    pub content: String,
}

pub fn apply_prompt_compaction(
    messages: &[PromptMessage],
    policy: Option<PromptCompactionPolicy>,
) -> Vec<PromptMessage> {
    match policy.unwrap_or(PromptCompactionPolicy::CurrentDefault) {
        PromptCompactionPolicy::CurrentDefault => compact_current_default(messages),
        PromptCompactionPolicy::Last8Ledger1024 => {
            compact_with_plaintext_ledger(messages, 8, 1_024)
        }
        PromptCompactionPolicy::Last6Ledger768 => compact_with_plaintext_ledger(messages, 6, 768),
        PromptCompactionPolicy::BenchmarkRepairMinimal => {
            compact_benchmark_repair_minimal(messages)
        }
        PromptCompactionPolicy::BenchmarkStatePacket => compact_benchmark_state_packet(messages),
        PromptCompactionPolicy::Off => messages.to_vec(),
    }
}

fn compact_current_default(messages: &[PromptMessage]) -> Vec<PromptMessage> {
    if messages.len() <= 10 {
        return messages.to_vec();
    }

    let mut compacted = Vec::with_capacity(messages.len());
    let total_messages = messages.len();
    let keep_tail = 6usize;

    for (index, message) in messages.iter().enumerate() {
        if index == 0 || index >= total_messages.saturating_sub(keep_tail) {
            compacted.push(message.clone());
            continue;
        }

        let mut content = message.content.clone();
        if message.role == PromptMessageRole::User
            && (content.starts_with("[Tool Output]")
                || content.starts_with("[Tool Success]")
                || content.starts_with("[Tool Error]")
                || content.starts_with("[Verifier]"))
        {
            let lines = content.lines().collect::<Vec<_>>();
            if lines.len() > 30 {
                let header = lines[..7].join("\n");
                let footer = lines[lines.len().saturating_sub(5)..].join("\n");
                content = format!(
                    "{}\n... [{} lines pruned for context length] ...\n{}",
                    header,
                    lines.len().saturating_sub(12),
                    footer
                );
            }
        }

        compacted.push(PromptMessage {
            role: message.role,
            content,
        });
    }

    compacted
}

fn compact_with_plaintext_ledger(
    messages: &[PromptMessage],
    recent_window: usize,
    ledger_cap_tokens: u64,
) -> Vec<PromptMessage> {
    if messages.len() <= recent_window
        || estimate_message_tokens(messages) < DEFAULT_COMPACT_THRESHOLD_TOKENS
    {
        return messages.to_vec();
    }

    let keep_from = messages.len().saturating_sub(recent_window);
    let older = &messages[..keep_from];
    let newer = &messages[keep_from..];
    let ledger = build_plaintext_ledger(older, ledger_cap_tokens);

    let mut compacted = Vec::with_capacity(newer.len() + 1);
    compacted.push(PromptMessage {
        role: PromptMessageRole::User,
        content: format!(
            "[Compacted Prior Context]\nTreat this as condensed transcript memory from earlier turns.\n{ledger}"
        ),
    });
    compacted.extend_from_slice(newer);
    compacted
}

fn compact_benchmark_repair_minimal(messages: &[PromptMessage]) -> Vec<PromptMessage> {
    let Some(repair_index) = messages.iter().rposition(|message| {
        message.role == PromptMessageRole::User
            && (message.content.contains("[Repair Phase]")
                || message.content.contains("[Patch Packet]"))
    }) else {
        return compact_current_default(messages);
    };

    let mut compacted = messages
        .iter()
        .take_while(|message| message.role == PromptMessageRole::System)
        .cloned()
        .collect::<Vec<_>>();
    compacted.push(PromptMessage {
        role: PromptMessageRole::User,
        content: render_minimal_repair_context(messages, repair_index),
    });
    compacted.extend_from_slice(&messages[repair_index + 1..]);
    compacted
}

fn compact_benchmark_state_packet(messages: &[PromptMessage]) -> Vec<PromptMessage> {
    let Some(repair_index) = messages.iter().rposition(|message| {
        message.role == PromptMessageRole::User
            && (message.content.contains("[Repair Phase]")
                || message.content.contains("[Patch Packet]"))
    }) else {
        return compact_current_default(messages);
    };

    let mut compacted = messages
        .iter()
        .take_while(|message| message.role == PromptMessageRole::System)
        .cloned()
        .collect::<Vec<_>>();
    compacted.push(PromptMessage {
        role: PromptMessageRole::User,
        content: render_benchmark_state_packet(messages, repair_index),
    });
    compacted.extend_from_slice(&messages[repair_index + 1..]);
    compacted
}

fn render_minimal_repair_context(messages: &[PromptMessage], repair_index: usize) -> String {
    let repair_packet = messages
        .get(repair_index)
        .map(|message| message.content.as_str())
        .unwrap_or_default();
    let mut lines = vec![
        "[Benchmark Repair Minimal]".to_string(),
        "Use this compact repair state instead of replaying the full objective/capsule."
            .to_string(),
    ];
    push_first_labeled_value(&mut lines, messages, "Goal:");
    push_last_labeled_value(&mut lines, messages, "Patch target:");
    push_last_labeled_value(&mut lines, messages, "Owner path:");
    push_last_labeled_value(&mut lines, messages, "Repair write locked:");
    push_last_labeled_value(&mut lines, messages, "Last validation failure:");
    push_last_labeled_value(&mut lines, messages, "Assertion excerpt:");
    push_last_labeled_value(&mut lines, messages, "Missing dependencies:");
    push_last_labeled_value(&mut lines, messages, "Target dependency table:");
    push_last_labeled_value(&mut lines, messages, "Observed target content_hash:");
    push_last_labeled_value(&mut lines, messages, "Required next action:");
    push_last_labeled_value(&mut lines, messages, "Primary failure test:");
    push_last_labeled_value(&mut lines, messages, "Primary failure location:");
    push_last_labeled_value(&mut lines, messages, "Recommended rerun command:");
    lines.push("Current repair packet:".to_string());
    lines.push(truncate_visible(repair_packet, 2_200));
    lines.join("\n")
}

fn render_benchmark_state_packet(messages: &[PromptMessage], repair_index: usize) -> String {
    let repair_packet = messages
        .get(repair_index)
        .map(|message| message.content.as_str())
        .unwrap_or_default();
    let mut lines = vec![
        "[Benchmark State Packet]".to_string(),
        "Use this typed agent state instead of replaying the full objective/capsule.".to_string(),
    ];
    push_first_labeled_value(&mut lines, messages, "Goal:");
    push_first_labeled_value(&mut lines, messages, "Owner path:");
    push_first_labeled_value(&mut lines, messages, "Repair target:");
    push_last_labeled_value(&mut lines, messages, "Required next action:");
    push_last_labeled_value(&mut lines, messages, "Last validation failure:");
    push_last_labeled_value(&mut lines, messages, "Primary failure test:");
    push_last_labeled_value(&mut lines, messages, "Primary failure location:");
    push_last_labeled_value(&mut lines, messages, "Assertion excerpt:");
    push_last_labeled_value(&mut lines, messages, "Honored implementation range:");
    push_last_labeled_value(&mut lines, messages, "Failed edit memory:");
    push_last_labeled_value(&mut lines, messages, "Agent scorecard:");
    push_last_labeled_value(&mut lines, messages, "Agent memory:");
    push_last_labeled_value(&mut lines, messages, "Allowed actions:");
    push_last_labeled_value(&mut lines, messages, "Recommended rerun command:");
    lines.push("Current repair packet:".to_string());
    lines.push(truncate_repair_packet(repair_packet, 6_400, 120));
    truncate_to_token_budget(lines.join("\n"), 1_600)
}

fn truncate_repair_packet(text: &str, max_chars: usize, max_lines: usize) -> String {
    let selected = text.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    truncate_visible(&selected, max_chars)
}

fn push_first_labeled_value(lines: &mut Vec<String>, messages: &[PromptMessage], label: &str) {
    if let Some(value) = messages
        .iter()
        .find_map(|message| extract_labeled_line(&message.content, label))
    {
        lines.push(format!("{label} {value}"));
    }
}

fn push_last_labeled_value(lines: &mut Vec<String>, messages: &[PromptMessage], label: &str) {
    if let Some(value) = messages
        .iter()
        .rev()
        .find_map(|message| extract_labeled_line(&message.content, label))
    {
        lines.push(format!("{label} {value}"));
    }
}

fn extract_labeled_line(content: &str, label: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix(label)
            .map(str::trim)
            .map(str::to_string)
            .filter(|value| !value.is_empty())
    })
}

fn truncate_visible(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let mut truncated = text.to_string();
    truncated.truncate(floor_char_boundary(&truncated, max_chars));
    truncated.push_str("\n... [truncated]");
    truncated
}

fn build_plaintext_ledger(messages: &[PromptMessage], ledger_cap_tokens: u64) -> String {
    let mut ledger = String::from("Condensed prior context:\n");
    for message in messages {
        let content = message
            .content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        if content.is_empty() {
            continue;
        }
        let role = match message.role {
            PromptMessageRole::System => "system",
            PromptMessageRole::User => "user",
            PromptMessageRole::Assistant => "assistant",
        };
        ledger.push_str(&format!("- {role}: {content}\n"));
        if estimate_text_tokens(&ledger) >= ledger_cap_tokens {
            break;
        }
    }
    truncate_to_token_budget(ledger.trim().to_string(), ledger_cap_tokens)
}

fn truncate_to_token_budget(mut text: String, ledger_cap_tokens: u64) -> String {
    while estimate_text_tokens(&text) > ledger_cap_tokens && !text.is_empty() {
        let truncate_to = text.len().saturating_sub(64);
        text.truncate(floor_char_boundary(&text, truncate_to));
    }
    text.trim().to_string()
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut boundary = index.min(text.len());
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn estimate_message_tokens(messages: &[PromptMessage]) -> u64 {
    messages
        .iter()
        .map(|message| estimate_text_tokens(&message.content))
        .sum()
}

fn estimate_text_tokens(text: &str) -> u64 {
    text.len().div_ceil(4) as u64
}

#[cfg(test)]
mod tests {
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
}

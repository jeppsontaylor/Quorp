use super::*;

#[test]
fn user_prompt_uses_chevron_in_color() {
    let line = TranscriptLine::UserPrompt("hi".into());
    let plain = render_transcript_line(&line, ColorCapability::NoColor);
    assert_eq!(plain, "> hi");
    let coloured = render_transcript_line(&line, ColorCapability::TrueColor);
    assert!(coloured.contains('❯'));
    assert!(coloured.contains("hi"));
}

#[test]
fn tool_call_summary_includes_chars() {
    let line = TranscriptLine::ToolCallSummary {
        tool: "read_file".into(),
        target: "src/main.rs:1-200".into(),
        sample_chars: 3100,
    };
    let plain = render_transcript_line(&line, ColorCapability::NoColor);
    assert!(plain.contains("read_file"));
    assert!(plain.contains("3100 chars"));
}

#[test]
fn repair_attempt_indents_with_arrow() {
    let line = TranscriptLine::RepairAttempt {
        attempt: 1,
        cap: 3,
        hypothesis: "missing semi".into(),
    };
    let plain = render_transcript_line(&line, ColorCapability::NoColor);
    assert!(plain.starts_with("  ↳"));
    assert!(plain.contains("1/3"));
}

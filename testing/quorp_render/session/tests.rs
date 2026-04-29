use super::*;

fn sample_frame() -> SessionFrame {
    SessionFrame {
        title: "brilliant terminal coding".into(),
        subtitle: "agent-first Rust runtime · truecolor stream · sandboxed tools".into(),
        tasks: vec![
            TaskRow {
                label: "plan verification gates".into(),
                state: TaskState::Done,
            },
            TaskRow {
                label: "run strict proof lane".into(),
                state: TaskState::Active,
            },
        ],
        commands: vec![CommandCard {
            label: "verification".into(),
            command: "cargo test --workspace --lib".into(),
            cwd: "/repo".into(),
            state: CommandState::Active { frame_time: 0.0 },
            output_summary: "421 tests queued · first failures will pin exact spans".into(),
        }],
        footer: "qwen3-coder@nvidia · yolo sandbox · ctx 12.4k/64k".into(),
    }
}

#[test]
fn no_color_session_frame_is_stable() {
    let rendered = render_session_frame(&sample_frame(), 64, ColorCapability::NoColor);
    assert_eq!(
        rendered,
        "QUORP // brilliant terminal coding\nagent-first Rust runtime · truecolor stream · sandboxed too…\n----------------------------------------------------------------\ntask list\n  ✓ plan verification gates\n  * run strict proof lane\n+--------------------------------------------------------------+\n| verification  ⠋ running                                      |\n| $ cargo test --workspace --lib                               |\n| cwd /repo                                                    |\n| 421 tests queued · first failures will pin exact spans       |\n+--------------------------------------------------------------+\nqwen3-coder@nvidia · yolo sandbox · ctx 12.4k/64k"
    );
}

#[test]
fn truecolor_session_frame_contains_brand_and_shimmer() {
    let rendered = render_session_frame(&sample_frame(), 72, ColorCapability::TrueColor);
    assert!(rendered.contains("\x1b[38;2"));
    assert!(rendered.contains("QUORP"));
    assert!(rendered.contains("421 tests queued"));
    assert!(rendered.contains("421 tests queued"));
    assert!(rendered.ends_with("\x1b[0m"));
}

#[test]
fn command_card_width_is_stable_across_state_changes() {
    let mut command = CommandCard {
        label: "build".into(),
        command: "cargo check --workspace".into(),
        cwd: "/repo".into(),
        state: CommandState::Active { frame_time: 0.0 },
        output_summary: "checking crates".into(),
    };
    let active = render_command_card(&command, 60, ColorCapability::NoColor);
    command.state = CommandState::Passed {
        exit_code: 0,
        duration: "2.7s".into(),
    };
    let passed = render_command_card(&command, 60, ColorCapability::NoColor);
    for line in active.lines().chain(passed.lines()) {
        assert_eq!(line.width(), 60);
    }
}

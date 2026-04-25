use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::Line;
use std::time::Duration;

use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::app::{Overlay, Pane};
use crate::quorp::tui::bridge::TerminalFrame;
use crate::quorp::tui::chat::ChatMessage;
use crate::quorp::tui::proof_rail::RailMode;
use crate::quorp::tui::rail_event::{
    AgentPhase, PlanStep, PlanStepStatus, RailEvent, RiskSeverity, ToolKind,
};
use crate::quorp::tui::shell::{ShellExperienceMode, ShellGeometry, ShellScene};
use crate::quorp::tui::terminal_surface::TerminalSnapshot;

use super::fixtures;
use super::harness::TuiTestHarness;
use super::scenario::TuiScenario;

fn seed_command_center_state(scenario: TuiScenario<'_>) -> TuiScenario<'_> {
    scenario
        .rail_event(RailEvent::PhaseChanged(AgentPhase::Editing))
        .rail_event(RailEvent::OneSecondStory {
            summary:
                "Patching planner ownership resolution, then proving it with focused Rust tests before widening scope."
                    .to_string(),
        })
        .rail_event(RailEvent::TopDoubtUpdated {
            doubt: "Planner ownership might still leak into public surface.".to_string(),
        })
        .rail_event(RailEvent::TimeToProofUpdated {
            eta_seconds: Some(95),
            confidence_target: Some(0.84),
        })
        .rail_event(RailEvent::ConfidenceUpdate {
            understanding: 0.86,
            merge_safety: 0.71,
            delta: 0.05,
        })
        .rail_event(RailEvent::PlanLocked {
            steps: vec![
                PlanStep {
                    label: "ground the contract".to_string(),
                    status: PlanStepStatus::Completed,
                },
                PlanStep {
                    label: "patch the narrow diff".to_string(),
                    status: PlanStepStatus::Active,
                },
                PlanStep {
                    label: "prove with targeted tests".to_string(),
                    status: PlanStepStatus::Pending,
                },
            ],
        })
        .rail_event(RailEvent::ToolStarted {
            tool_id: 12,
            name: "rg".to_string(),
            kind: ToolKind::Search,
            target: "planner ownership".to_string(),
            cwd: Some("/Users/bentaylor/Code/quorp".to_string()),
            expected_outcome: "ground the affected call sites".to_string(),
            validation_kind: None,
        })
        .rail_event(RailEvent::ToolProgress {
            tool_id: 12,
            state: crate::quorp::tui::rail_event::ToolState::Streaming,
            latest_output: Some("planner.rs, wrapper.rs".to_string()),
        })
        .rail_event(RailEvent::ToolStarted {
            tool_id: 14,
            name: "cargo_test".to_string(),
            kind: ToolKind::Test,
            target: "planner focused".to_string(),
            cwd: Some("/Users/bentaylor/Code/quorp".to_string()),
            expected_outcome: "focused proof turns green".to_string(),
            validation_kind: Some("cargo-test".to_string()),
        })
        .rail_event(RailEvent::ProofProgress {
            tests_passed: 6,
            tests_total: 9,
            coverage_delta: 0.03,
        })
        .rail_event(RailEvent::ToolCompleted {
            tool_id: 12,
            exit_code: Some(0),
            duration_ms: 420,
            files_changed: 0,
            confidence_delta: Some(0.02),
        })
        .rail_event(RailEvent::ToolCompleted {
            tool_id: 14,
            exit_code: Some(0),
            duration_ms: 1_820,
            files_changed: 1,
            confidence_delta: Some(0.05),
        })
        .rail_event(RailEvent::WatchpointAdded {
            label: "auth untouched".to_string(),
        })
        .rail_event(RailEvent::WatchpointTriggered {
            label: "auth untouched".to_string(),
            detail: "auth_guard.rs entered the candidate change radius".to_string(),
        })
        .rail_event(RailEvent::ReasoningStep {
            objective: "Narrow planner ownership resolution.".to_string(),
            evidence: vec!["Planner call sites all route through one helper.".to_string()],
            action: "Patch the helper before widening scope.".to_string(),
            expected_result: "Focused tests should prove the ownership change.".to_string(),
            rollback: Some("git checkout -- planner helper".to_string()),
            rejected_branch: Some("Skip proof and widen edit immediately.".to_string()),
        })
        .rail_event(RailEvent::SessionCheckpoint {
            label: "pre-proof".to_string(),
            commit_hash: Some("abc1234".to_string()),
        })
        .rail_event(RailEvent::ArtifactReady {
            label: "result_dir".to_string(),
            path: "/full-auto/demo-run".to_string(),
        })
        .rail_event(RailEvent::RollbackReadinessChanged {
            ready: true,
            summary: "checkpoint and diff recipe recorded".to_string(),
        })
        .rail_event(RailEvent::StopReasonSet {
            reason: "watching".to_string(),
        })
}

fn preview_fixture_harness() -> (tempfile::TempDir, TuiTestHarness) {
    let dir = fixtures::temp_project_with_files(&[
        (
            "crates/quorp/src/quorp/planner.rs",
            "pub fn plan_route() {\n    let narrowed = true;\n}\n",
        ),
        (
            "crates/quorp/src/quorp/auth_guard.rs",
            "pub fn auth_guard() {\n    let tripped = true;\n}\n",
        ),
        ("artifacts/run.log", "proof ready\n"),
    ]);
    let harness = TuiTestHarness::new_with_root(180, 50, dir.path().to_path_buf());
    (dir, harness)
}

fn open_preview_file(harness: &mut TuiTestHarness, relative_path: &str) {
    let path = harness.app.file_tree.root().join(relative_path);
    harness.app.file_tree.set_selected_file(Some(path.clone()));
    harness
        .app
        .editor_pane
        .open_preview_target(path.as_path(), harness.app.file_tree.root());
    harness
        .app
        .editor_pane
        .ensure_active_loaded(harness.app.file_tree.root());
    harness.app.focused = Pane::EditorPane;
}

#[test]
fn proof_rail_scenario_records_replay_and_renders_core_cards() {
    let mut harness = TuiTestHarness::new(180, 50);

    let artifacts = TuiScenario::new(&mut harness, "proof_rail_control_tower")
        .note("load a high-trust scenario")
        .resize(180, 50)
        .rail_event(RailEvent::PhaseChanged(AgentPhase::Planning))
        .rail_event(RailEvent::OneSecondStory {
            summary: "Patching planner ownership resolution, then proving it with focused Rust tests before widening scope.".to_string(),
        })
        .rail_event(RailEvent::ConfidenceUpdate {
            understanding: 0.86,
            merge_safety: 0.73,
            delta: 0.08,
        })
        .rail_event(RailEvent::PlanLocked {
            steps: vec![
                PlanStep {
                    label: "lock contract".to_string(),
                    status: PlanStepStatus::Completed,
                },
                PlanStep {
                    label: "run targeted proof".to_string(),
                    status: PlanStepStatus::Active,
                },
                PlanStep {
                    label: "widen scope only if needed".to_string(),
                    status: PlanStepStatus::Pending,
                },
            ],
        })
        .rail_event(RailEvent::RiskPromoted {
            description: "public API widened".to_string(),
            severity: RiskSeverity::Medium,
            blast_radius: 2,
        })
        .rail_event(RailEvent::WaitReason {
            explanation: "cargo test still compiling".to_string(),
        })
        .draw()
        .save_screenshot("proof_rail_control_tower_180x50")
        .assert_buffer_contains("WHY WAITING")
        .assert_buffer_contains("CONFIDENCE")
        .assert_buffer_contains("PLANNING")
        .assert_rail_mode(RailMode::WhyWaiting)
        .finish();

    assert!(artifacts.replay_path.exists(), "replay log was not written");
    assert!(
        artifacts.screenshot_path.is_some(),
        "screenshot path missing"
    );
}

#[test]
fn proof_rail_breakpoints_stay_visible_and_trustworthy() {
    for (cols, rows, expected_width) in [(180, 50, 58), (150, 50, 48), (120, 40, 38)] {
        let mut harness = TuiTestHarness::new(cols, rows);
        harness.apply_backend_event(crate::quorp::tui::TuiEvent::RailEvent(
            RailEvent::PhaseChanged(AgentPhase::Verifying),
        ));
        harness.draw();

        let state = harness
            .app
            .shell_state_snapshot(Rect::new(0, 0, cols, rows));
        let geometry = ShellGeometry::for_state(Rect::new(0, 0, cols, rows), &state);
        assert!(
            geometry.proof_rail.is_some(),
            "proof rail should be visible at {cols}x{rows}"
        );
        assert!(
            geometry
                .proof_rail
                .map(|rect| rect.width)
                .unwrap_or_default()
                >= expected_width,
            "proof rail too narrow at {cols}x{rows}"
        );
        harness.assert_buffer_contains("VERIFY RADAR");
        harness.assert_buffer_contains("PROOF STACK");
    }
}

#[test]
fn proof_rail_scenario_supports_command_and_keyboard_journeys() {
    let mut harness = TuiTestHarness::new(180, 50);
    harness.app.focused = Pane::Chat;

    let artifacts = TuiScenario::new(&mut harness, "proof_rail_keyboard_journey")
        .note("open and close help overlay")
        .resize(180, 50)
        .key_press(KeyCode::Char('?'), KeyModifiers::NONE)
        .assert_overlay(Overlay::Help)
        .key_press(KeyCode::Esc, KeyModifiers::NONE)
        .assert_overlay(Overlay::None)
        .rail_event(RailEvent::PhaseChanged(AgentPhase::Editing))
        .rail_event(RailEvent::ToolStarted {
            tool_id: 1,
            name: "cargo_test".to_string(),
            kind: ToolKind::Test,
            target: "quorp".to_string(),
            cwd: None,
            expected_outcome: "focused tests pass".to_string(),
            validation_kind: Some("cargo-test".to_string()),
        })
        .rail_event(RailEvent::ToolProgress {
            tool_id: 1,
            state: crate::quorp::tui::rail_event::ToolState::Streaming,
            latest_output: Some("running 14 tests".to_string()),
        })
        .rail_event(RailEvent::ToolStarted {
            tool_id: 2,
            name: "git_status".to_string(),
            kind: ToolKind::Git,
            target: ".".to_string(),
            cwd: None,
            expected_outcome: "worktree state captured".to_string(),
            validation_kind: None,
        })
        .rail_event(RailEvent::ProofProgress {
            tests_passed: 14,
            tests_total: 18,
            coverage_delta: 0.04,
        })
        .draw()
        .save_screenshot("proof_rail_keyboard_journey_180x50")
        .assert_buffer_contains("TOOL ORCHESTRA")
        .assert_buffer_contains("TOOL BUS")
        .finish();

    assert!(artifacts.replay_path.exists(), "replay log missing");
}

#[test]
fn shell_first_command_center_hides_legacy_sidebar() {
    let mut harness = TuiTestHarness::new(180, 50);
    harness.draw();

    let state = harness.app.shell_state_snapshot(Rect::new(0, 0, 180, 50));
    let geometry = ShellGeometry::for_state(Rect::new(0, 0, 180, 50), &state);
    assert_eq!(state.scene, ShellScene::Ready);
    assert_eq!(state.experience_mode, ShellExperienceMode::CommandCenter);
    assert_eq!(geometry.sidebar.width, 0);
    assert_eq!(geometry.proof_rail.expect("proof rail").width, 58);
}

#[test]
fn rail_boot_transition_combined_golden() {
    let mut harness = TuiTestHarness::new(180, 50);
    harness.app.force_bootstrap_for_test();
    harness.clear_replay_log();
    harness.record_replay_step("scenario", "rail_boot_transition");
    harness.draw();
    let bootstrap_state = harness.app.shell_state_snapshot(Rect::new(0, 0, 180, 50));
    assert_eq!(bootstrap_state.scene, ShellScene::Bootstrap);
    harness.record_replay_step("note", "bootstrap renders inside the future proof rail");
    harness.app.complete_bootstrap_for_test();
    harness.draw();
    let ready_state = harness.app.shell_state_snapshot(Rect::new(0, 0, 180, 50));
    assert_eq!(ready_state.scene, ShellScene::Ready);
    assert_eq!(
        ready_state.experience_mode,
        ShellExperienceMode::CommandCenter
    );
    harness.assert_screenshot_golden("rail_boot_transition");
    harness.assert_replay_golden("rail_boot_transition");
}

#[test]
fn control_deck_opens_and_can_arm_watchpoints() {
    let mut harness = TuiTestHarness::new(180, 50);
    harness.app.focused = Pane::EditorPane;

    seed_command_center_state(TuiScenario::new(&mut harness, "control_deck_watchpoint"))
        .draw()
        .key_press(KeyCode::Char('k'), KeyModifiers::CONTROL)
        .assert_overlay(Overlay::ActionDeck)
        .draw()
        .assert_buffer_contains("Control Deck")
        .key_press(KeyCode::Char('a'), KeyModifiers::NONE)
        .key_press(KeyCode::Char('u'), KeyModifiers::NONE)
        .key_press(KeyCode::Char('t'), KeyModifiers::NONE)
        .key_press(KeyCode::Char('h'), KeyModifiers::NONE)
        .draw()
        .assert_buffer_contains("Watchpoint: auth untouched")
        .key_press(KeyCode::Enter, KeyModifiers::NONE)
        .assert_overlay(Overlay::None)
        .draw()
        .assert_buffer_contains("WATCHPOINTS")
        .assert_buffer_contains("auth untouched")
        .finish();
}

#[test]
fn control_deck_can_follow_tool_target() {
    let (_dir, mut harness) = preview_fixture_harness();
    harness.app.focused = Pane::Chat;

    TuiScenario::new(&mut harness, "control_deck_follow_tool")
        .rail_event(RailEvent::ToolStarted {
            tool_id: 33,
            name: "read_file".to_string(),
            kind: ToolKind::Read,
            target: "crates/quorp/src/quorp/planner.rs".to_string(),
            cwd: None,
            expected_outcome: "review the planner helper in place".to_string(),
            validation_kind: None,
        })
        .draw()
        .key_press(KeyCode::Char('k'), KeyModifiers::CONTROL)
        .assert_overlay(Overlay::ActionDeck)
        .key_press(KeyCode::Char('f'), KeyModifiers::NONE)
        .key_press(KeyCode::Char('o'), KeyModifiers::NONE)
        .key_press(KeyCode::Char('l'), KeyModifiers::NONE)
        .key_press(KeyCode::Char('l'), KeyModifiers::NONE)
        .key_press(KeyCode::Char('o'), KeyModifiers::NONE)
        .key_press(KeyCode::Char('w'), KeyModifiers::NONE)
        .key_press(KeyCode::Char(' '), KeyModifiers::NONE)
        .key_press(KeyCode::Char('t'), KeyModifiers::NONE)
        .key_press(KeyCode::Char('o'), KeyModifiers::NONE)
        .key_press(KeyCode::Char('o'), KeyModifiers::NONE)
        .key_press(KeyCode::Char('l'), KeyModifiers::NONE)
        .key_press(KeyCode::Enter, KeyModifiers::NONE)
        .wait_for_buffer_contains_silent("pub fn plan_route()", Duration::from_millis(500))
        .draw()
        .assert_overlay(Overlay::None)
        .assert_focus(Pane::EditorPane)
        .assert_buffer_contains("pub fn plan_route()")
        .assert_combined_golden("follow_tool_focus")
        .finish();
}

#[test]
fn command_center_active_combined_golden() {
    let (_dir, mut harness) = preview_fixture_harness();
    open_preview_file(&mut harness, "crates/quorp/src/quorp/planner.rs");
    harness.app.proof_rail.set_user_mode(RailMode::ControlTower);

    seed_command_center_state(TuiScenario::new(&mut harness, "command_center_active"))
        .rail_event(RailEvent::BlastRadiusUpdate {
            files_touched: vec![
                "crates/quorp/src/quorp/planner.rs".to_string(),
                "crates/quorp/src/quorp/auth_guard.rs".to_string(),
            ],
            symbols_changed: 4,
            net_lines_delta: 28,
        })
        .draw()
        .assert_buffer_contains("COMMAND CENTER")
        .assert_buffer_contains("Preview")
        .assert_buffer_contains("pub fn plan_route()")
        .assert_buffer_contains("TOP DOUBT")
        .assert_buffer_contains("TIME TO PROOF")
        .assert_buffer_contains("WATCHPOINTS")
        .assert_combined_golden("command_center_active")
        .finish();
}

#[test]
fn diff_lens_active_combined_golden() {
    let (_dir, mut harness) = preview_fixture_harness();
    open_preview_file(&mut harness, "crates/quorp/src/quorp/planner.rs");

    seed_command_center_state(TuiScenario::new(&mut harness, "diff_lens_active"))
        .rail_event(RailEvent::BlastRadiusUpdate {
            files_touched: vec![
                "crates/quorp/src/quorp/planner.rs".to_string(),
                "crates/quorp/src/quorp/auth_guard.rs".to_string(),
            ],
            symbols_changed: 4,
            net_lines_delta: 28,
        })
        .key_press(KeyCode::Char('d'), KeyModifiers::NONE)
        .draw()
        .assert_rail_mode(RailMode::DiffReactor)
        .assert_buffer_contains("DIFF LENS")
        .assert_buffer_contains("pub fn plan_route()")
        .assert_combined_golden("diff_lens_active")
        .finish();
}

#[test]
fn verify_radar_active_combined_golden() {
    let (_dir, mut harness) = preview_fixture_harness();
    open_preview_file(&mut harness, "crates/quorp/src/quorp/planner.rs");

    seed_command_center_state(TuiScenario::new(&mut harness, "verify_radar_active"))
        .key_press(KeyCode::Char('v'), KeyModifiers::NONE)
        .draw()
        .assert_rail_mode(RailMode::VerifyRadar)
        .assert_buffer_contains("VERIFY RADAR")
        .assert_buffer_contains("pub fn plan_route()")
        .assert_combined_golden("verify_radar_active")
        .finish();
}

#[test]
fn trace_lens_active_combined_golden() {
    let mut harness = TuiTestHarness::new(180, 50);
    harness.app.focused = Pane::EditorPane;

    seed_command_center_state(TuiScenario::new(&mut harness, "trace_lens_active"))
        .key_press(KeyCode::Char('r'), KeyModifiers::NONE)
        .draw()
        .assert_rail_mode(RailMode::TraceLens)
        .assert_buffer_contains("TRACE LENS")
        .assert_buffer_contains("rejected")
        .assert_combined_golden("trace_lens_active")
        .finish();
}

#[test]
fn timeline_scrubber_active_combined_golden() {
    let mut harness = TuiTestHarness::new(180, 50);
    harness.app.focused = Pane::EditorPane;

    seed_command_center_state(TuiScenario::new(&mut harness, "timeline_scrubber_active"))
        .key_press(KeyCode::Char('t'), KeyModifiers::NONE)
        .draw()
        .assert_rail_mode(RailMode::TimelineScrubber)
        .assert_buffer_contains("TIMELINE")
        .assert_buffer_contains("result_dir")
        .assert_combined_golden("timeline_scrubber_active")
        .finish();
}

#[test]
fn assistant_feed_file_link_opens_center_preview() {
    let (_dir, mut harness) = preview_fixture_harness();
    harness.app.focused = Pane::Chat;
    harness.app.chat.seed_messages_for_test(vec![
        ChatMessage::User("show me the change".to_string()),
        ChatMessage::Assistant(
            "Start with crates/quorp/src/quorp/planner.rs:1 before widening scope.".to_string(),
        ),
    ]);

    TuiScenario::new(&mut harness, "assistant_feed_file_link_preview")
        .draw()
        .key_press(KeyCode::Down, KeyModifiers::ALT)
        .key_press(KeyCode::Enter, KeyModifiers::ALT)
        .draw()
        .assert_focus(Pane::EditorPane)
        .assert_buffer_contains("pub fn plan_route()")
        .finish();
}

#[test]
fn proof_rail_changed_file_opens_center_preview() {
    let (_dir, mut harness) = preview_fixture_harness();
    harness.app.focused = Pane::Chat;

    TuiScenario::new(&mut harness, "proof_rail_changed_file_preview")
        .rail_event(RailEvent::BlastRadiusUpdate {
            files_touched: vec!["crates/quorp/src/quorp/planner.rs".to_string()],
            symbols_changed: 1,
            net_lines_delta: 12,
        })
        .draw()
        .key_press(KeyCode::Down, KeyModifiers::ALT)
        .key_press(KeyCode::Enter, KeyModifiers::ALT)
        .draw()
        .assert_focus(Pane::EditorPane)
        .assert_buffer_contains("pub fn plan_route()")
        .finish();
}

#[test]
fn terminal_path_opens_center_preview() {
    let (_dir, mut harness) = preview_fixture_harness();
    harness.apply_backend_event(TuiEvent::TerminalFrame(TerminalFrame {
        snapshot: TerminalSnapshot::from_lines(&[Line::from(
            "rerun crates/quorp/src/quorp/auth_guard.rs:1 after the next patch",
        )]),
        cwd: Some(harness.app.file_tree.root().to_path_buf()),
        shell_label: Some("Terminal".to_string()),
        window_title: Some("engage-preview".to_string()),
    }));
    let targets = harness
        .app
        .shell_engage_target_keys_for_test(Rect::new(0, 0, 180, 50));
    assert!(
        targets
            .iter()
            .any(|(label, _)| label.ends_with("crates/quorp/src/quorp/auth_guard.rs:1")),
        "expected auth guard target, got {targets:?}"
    );

    TuiScenario::new(&mut harness, "terminal_path_jump")
        .draw()
        .key_press(KeyCode::Down, KeyModifiers::ALT)
        .key_press(KeyCode::Enter, KeyModifiers::ALT)
        .draw()
        .assert_focus(Pane::EditorPane)
        .assert_buffer_contains("pub fn auth_guard()")
        .assert_combined_golden("terminal_path_jump")
        .finish();
}

#[test]
fn preview_target_escalates_to_diff_lens_with_d() {
    let (_dir, mut harness) = preview_fixture_harness();
    open_preview_file(&mut harness, "crates/quorp/src/quorp/planner.rs");

    TuiScenario::new(&mut harness, "preview_to_diff_lens")
        .rail_event(RailEvent::BlastRadiusUpdate {
            files_touched: vec!["crates/quorp/src/quorp/planner.rs".to_string()],
            symbols_changed: 2,
            net_lines_delta: 18,
        })
        .draw()
        .key_press(KeyCode::Char('d'), KeyModifiers::NONE)
        .draw()
        .assert_focus(Pane::EditorPane)
        .assert_rail_mode(RailMode::DiffReactor)
        .assert_buffer_contains("pub fn plan_route()")
        .finish();
}

#[test]
fn artifact_row_opens_artifact_target_inside_quorp() {
    let (_dir, mut harness) = preview_fixture_harness();
    harness.app.focused = Pane::Chat;

    TuiScenario::new(&mut harness, "artifact_jump")
        .rail_event(RailEvent::ArtifactReady {
            label: "run log".to_string(),
            path: "artifacts/run.log".to_string(),
        })
        .draw()
        .key_press(KeyCode::Down, KeyModifiers::ALT)
        .key_press(KeyCode::Enter, KeyModifiers::ALT)
        .draw()
        .assert_focus(Pane::EditorPane)
        .assert_buffer_contains("proof ready")
        .assert_combined_golden("artifact_jump")
        .finish();
}

#[test]
fn keyboard_target_cycling_reaches_second_changed_file() {
    let (_dir, mut harness) = preview_fixture_harness();
    harness.app.focused = Pane::Chat;

    TuiScenario::new(&mut harness, "keyboard_target_cycling")
        .rail_event(RailEvent::BlastRadiusUpdate {
            files_touched: vec![
                "crates/quorp/src/quorp/planner.rs".to_string(),
                "crates/quorp/src/quorp/auth_guard.rs".to_string(),
            ],
            symbols_changed: 3,
            net_lines_delta: 22,
        })
        .draw()
        .key_press(KeyCode::Down, KeyModifiers::ALT)
        .key_press(KeyCode::Down, KeyModifiers::ALT)
        .key_press(KeyCode::Enter, KeyModifiers::ALT)
        .draw()
        .assert_focus(Pane::EditorPane)
        .assert_buffer_contains("pub fn auth_guard()")
        .finish();
}

#[test]
fn why_waiting_triggers_after_one_point_two_seconds_of_silence() {
    let mut harness = TuiTestHarness::new(180, 50);
    harness.app.focused = Pane::EditorPane;
    seed_command_center_state(TuiScenario::new(&mut harness, "why_waiting_signal"))
        .draw()
        .finish();
    harness.app.chat.set_streaming_for_test(true);
    harness.app.last_working_tick = Some(std::time::Instant::now() - Duration::from_millis(1300));
    harness.draw();
    let state = harness.app.shell_state_snapshot(Rect::new(0, 0, 180, 50));
    assert!(
        state.tool_orchestra.is_some(),
        "expected zero-dark tool orchestra"
    );
}

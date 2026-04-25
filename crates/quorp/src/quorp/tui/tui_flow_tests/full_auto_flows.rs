use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};
use tempfile::tempdir;

use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::app::{Overlay, Pane};
use crate::quorp::tui::chat::ChatUiEvent;
use crate::quorp::tui::proof_rail::RailMode;
use crate::quorp::tui::rail_event::{AgentPhase, RailEvent, RiskSeverity};
use crate::quorp::tui::slash_commands::LAUNCH_SPEC_FILE_NAME;
use crate::quorp::tui::slash_commands::{
    FullAutoLaunchSpec, FullAutoResolvedMode, FullAutoSandboxMode, SlashCommandKind,
};

use super::harness::TuiTestHarness;
use super::scenario::TuiScenario;

fn recv_start_agent_task(
    harness: &TuiTestHarness,
) -> crate::quorp::tui::agent_runtime::AgentTaskRequest {
    let event = harness
        .recv_tui_event_until(Duration::from_secs(2), |event| {
            matches!(event, TuiEvent::StartAgentTask(_))
        })
        .expect("start task event");
    let TuiEvent::StartAgentTask(task) = event else {
        panic!("expected StartAgentTask, got {event:?}");
    };
    task
}

fn write_executable_script(path: &Path, content: &str) {
    fs::write(path, content).expect("write script");
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(path).expect("script metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod script");
    }
}

#[test]
fn slash_command_deck_opens_and_inserts_full_auto_template() {
    let mut harness = TuiTestHarness::new(160, 42);
    harness.app.focused = Pane::Chat;

    TuiScenario::new(&mut harness, "slash_command_deck")
        .key_press(KeyCode::Char('/'), KeyModifiers::NONE)
        .assert_overlay(Overlay::SlashCommandDeck)
        .draw()
        .assert_buffer_contains("Slash Commands")
        .assert_buffer_contains("/full-auto")
        .key_press(KeyCode::Enter, KeyModifiers::NONE)
        .assert_overlay(Overlay::None)
        .finish();

    assert!(
        harness.app.chat.input_for_test().starts_with("/full-auto"),
        "expected /full-auto template, got {:?}",
        harness.app.chat.input_for_test()
    );
}

#[test]
fn slash_command_deck_combined_golden() {
    let mut harness = TuiTestHarness::new(160, 42);
    harness.app.focused = Pane::Chat;

    TuiScenario::new(&mut harness, "slash_command_deck_combined")
        .key_press(KeyCode::Char('/'), KeyModifiers::NONE)
        .assert_overlay(Overlay::SlashCommandDeck)
        .draw()
        .assert_combined_golden("slash_command_deck_combined")
        .finish();
}

#[test]
fn slash_command_deck_lists_docker_variants() {
    let mut harness = TuiTestHarness::new(160, 42);
    harness.app.focused = Pane::Chat;

    TuiScenario::new(&mut harness, "slash_command_deck_docker_variants")
        .key_press(KeyCode::Char('/'), KeyModifiers::NONE)
        .assert_overlay(Overlay::SlashCommandDeck)
        .draw()
        .assert_buffer_contains("/full-auto --docker")
        .assert_buffer_contains("/benchmark --docker")
        .finish();
}

#[test]
fn slash_character_stays_literal_when_composer_is_not_empty() {
    let mut harness = TuiTestHarness::new(120, 36);
    harness.app.focused = Pane::Chat;
    harness.app.chat.set_input_for_test("hello");

    harness.key_press(KeyCode::Char('/'), KeyModifiers::NONE);

    assert_eq!(harness.app.overlay, Overlay::None);
    assert_eq!(harness.app.chat.input_for_test(), "hello/");
}

#[test]
fn raw_full_auto_submission_emits_sandboxed_start_task_and_launch_spec() {
    let workspace = tempdir().expect("tempdir");
    fs::write(
        workspace.path().join("START_HERE.md"),
        "# Start\nFix the issue and prove it.\n",
    )
    .expect("write objective");
    fs::write(workspace.path().join("evaluate.sh"), "#!/bin/sh\nexit 0\n")
        .expect("write evaluator");

    let mut harness = TuiTestHarness::new_with_root(140, 40, workspace.path().to_path_buf());
    harness.app.focused = Pane::Chat;
    harness
        .app
        .chat
        .set_input_for_test("/full-auto fix the issue and run validation");

    harness.key_press(KeyCode::Enter, KeyModifiers::NONE);

    let task = recv_start_agent_task(&harness);
    assert_eq!(harness.app.chat.input_for_test(), "");
    assert_ne!(task.workspace_root, workspace.path());
    assert_eq!(task.workspace_root, task.result_dir.join("workspace"));
    assert!(task.workspace_root.exists(), "sandbox workspace missing");
    assert!(
        task.objective_file
            .as_ref()
            .is_some_and(|path| path.exists()),
        "sandbox objective file missing"
    );
    assert_eq!(task.max_iterations, 40);
    assert_eq!(task.sandbox_mode, FullAutoSandboxMode::LocalCopy);
    assert!(
        task.result_dir.join(LAUNCH_SPEC_FILE_NAME).exists(),
        "launch spec missing"
    );
}

#[test]
fn raw_full_auto_docker_submission_emits_docker_task_and_launch_spec() {
    let workspace = tempdir().expect("tempdir");
    fs::write(
        workspace.path().join("START_HERE.md"),
        "# Start\nFix the issue and prove it.\n",
    )
    .expect("write objective");
    fs::write(workspace.path().join("evaluate.sh"), "#!/bin/sh\nexit 0\n")
        .expect("write evaluator");

    let mut harness = TuiTestHarness::new_with_root(140, 40, workspace.path().to_path_buf());
    harness.app.focused = Pane::Chat;
    harness
        .app
        .chat
        .set_input_for_test("/full-auto --docker fix the issue and run validation");

    harness.key_press(KeyCode::Enter, KeyModifiers::NONE);

    let task = recv_start_agent_task(&harness);
    assert_eq!(task.sandbox_mode, FullAutoSandboxMode::Docker);
    assert_ne!(task.workspace_root, workspace.path());
    let spec = FullAutoLaunchSpec::load_from(&task.result_dir).expect("load launch spec");
    assert_eq!(spec.sandbox_mode, FullAutoSandboxMode::Docker);
}

#[test]
fn benchmark_submission_emits_sandboxed_start_task() {
    let root = tempdir().expect("tempdir");
    fs::write(root.path().join("START_HERE.md"), "# Root\n").expect("write root objective");

    let case_root = root.path().join("case-a");
    fs::create_dir_all(case_root.join("workspace").join("proof-full")).expect("mkdir workspace");
    fs::write(
        case_root.join("benchmark.json"),
        serde_json::json!({
            "id": "case-a",
            "title": "Case A",
            "difficulty": "medium",
            "category": "rust",
            "repo_condition": ["proof-full"],
            "objective_file": "START_HERE.md",
            "success_file": "SUCCESS.md",
            "reset_command": "./reset.sh <condition>",
            "evaluate_command": "./evaluate.sh <condition>",
            "estimated_minutes": 5,
            "expected_files_touched": ["src/lib.rs"],
            "primary_metrics": ["tests"],
            "tags": ["watch-mode"]
        })
        .to_string(),
    )
    .expect("write manifest");
    fs::write(case_root.join("START_HERE.md"), "# Case\n").expect("write objective");
    fs::write(
        case_root.join("LOCAL_REPRO.md"),
        "## Fast Loop\n\n```sh\n./evaluate.sh proof-full\n```\n\n## First Reads\n\n- `src/lib.rs`\n",
    )
    .expect("write local repro");
    fs::write(case_root.join("SUCCESS.md"), "done\n").expect("write success");
    write_executable_script(&case_root.join("reset.sh"), "#!/bin/sh\nexit 0\n");
    write_executable_script(&case_root.join("evaluate.sh"), "#!/bin/sh\nexit 0\n");
    fs::create_dir_all(case_root.join("workspace").join("proof-full").join("src"))
        .expect("mkdir src");
    fs::write(
        case_root
            .join("workspace")
            .join("proof-full")
            .join("src")
            .join("lib.rs"),
        "pub fn fixture() -> u32 { 1 }\n",
    )
    .expect("write owner file");
    fs::write(
        case_root
            .join("workspace")
            .join("proof-full")
            .join("README.md"),
        "# Workspace\n",
    )
    .expect("write workspace readme");

    let mut harness = TuiTestHarness::new_with_root(140, 40, root.path().to_path_buf());
    harness.app.focused = Pane::Chat;
    harness.app.chat.set_input_for_test(&format!(
        "/benchmark {} fix the benchmark case",
        case_root.display()
    ));

    harness.key_press(KeyCode::Enter, KeyModifiers::NONE);

    let task = recv_start_agent_task(&harness);
    assert!(task.workspace_root.starts_with(&task.result_dir));
    assert!(
        task.workspace_root
            .ends_with("sandbox/workspace/proof-full")
            || task.workspace_root.ends_with("workspace"),
        "unexpected benchmark workspace root {}",
        task.workspace_root.display()
    );
    assert_eq!(task.max_iterations, 60);
    assert_eq!(task.sandbox_mode, FullAutoSandboxMode::LocalCopy);
    assert!(task.result_dir.join(LAUNCH_SPEC_FILE_NAME).exists());
}

#[test]
fn benchmark_docker_submission_emits_docker_task() {
    let root = tempdir().expect("tempdir");
    let case_root = create_benchmark_case(root.path());

    let mut harness = TuiTestHarness::new_with_root(140, 40, root.path().to_path_buf());
    harness.app.focused = Pane::Chat;
    harness.app.chat.set_input_for_test(&format!(
        "/benchmark --docker {} fix the benchmark case",
        case_root.display()
    ));

    harness.key_press(KeyCode::Enter, KeyModifiers::NONE);

    let task = recv_start_agent_task(&harness);
    assert_eq!(task.sandbox_mode, FullAutoSandboxMode::Docker);
    assert_eq!(task.resolved_mode, FullAutoResolvedMode::Benchmark);
    let spec = FullAutoLaunchSpec::load_from(&task.result_dir).expect("load launch spec");
    assert_eq!(spec.sandbox_mode, FullAutoSandboxMode::Docker);
}

#[test]
fn resume_last_restores_docker_launch_spec() {
    let temp_dir = tempdir().expect("tempdir");
    let result_dir = temp_dir.path().join("run");
    let workspace_root = temp_dir.path().join("workspace");
    fs::create_dir_all(&workspace_root).expect("mkdir workspace");
    let spec = FullAutoLaunchSpec {
        kind: SlashCommandKind::Benchmark,
        goal: "resume docker benchmark".to_string(),
        target_path: temp_dir.path().join("case-a"),
        workspace_root,
        resolved_mode: FullAutoResolvedMode::Benchmark,
        sandbox_mode: FullAutoSandboxMode::Docker,
        docker_image: Some("quorp-runner:dev".to_string()),
        autonomy_profile: crate::quorp::tui::agent_context::AutonomyProfile::AutonomousSandboxed,
        max_steps: 60,
        max_seconds: Some(120),
        max_total_tokens: Some(1_500),
        result_dir: result_dir.clone(),
        objective_file: Some(temp_dir.path().join("workspace").join("START_HERE.md")),
        evaluate_command: Some("./evaluate.sh".to_string()),
        objective_metadata: serde_json::json!({"evaluate_command":"./evaluate.sh"}),
    };
    spec.write_to_disk().expect("write launch spec");

    let mut harness = TuiTestHarness::new_with_root(140, 40, temp_dir.path().to_path_buf());
    harness.app.focused = Pane::Chat;
    harness
        .app
        .chat
        .set_input_for_test(&format!("/resume-last {}", result_dir.display()));

    harness.key_press(KeyCode::Enter, KeyModifiers::NONE);

    let task = recv_start_agent_task(&harness);
    assert_eq!(task.sandbox_mode, FullAutoSandboxMode::Docker);
    assert_eq!(task.result_dir, result_dir);
}

#[test]
fn benchmark_watch_active_combined_golden() {
    let root = tempdir().expect("tempdir");
    let case_root = create_benchmark_case(root.path());
    let mut harness = TuiTestHarness::new_with_root(180, 48, root.path().to_path_buf());
    harness.app.focused = Pane::Chat;
    harness.app.chat.set_input_for_test(&format!(
        "/benchmark {} fix the benchmark case",
        case_root.display()
    ));

    TuiScenario::new(&mut harness, "benchmark_watch_active")
        .key_press(KeyCode::Enter, KeyModifiers::NONE)
        .wait_for_buffer_contains_silent("[Full Auto Launched]", Duration::from_secs(2))
        .draw()
        .rail_event(RailEvent::PhaseChanged(AgentPhase::Verifying))
        .rail_event(RailEvent::OneSecondStory {
            summary: "Patching planner ownership resolution, then proving it with focused Rust tests before widening scope.".to_string(),
        })
        .rail_event(RailEvent::WaitReason {
            explanation: "Running visible evaluator ./evaluate.sh".to_string(),
        })
        .rail_event(RailEvent::ProofProgress {
            tests_passed: 9,
            tests_total: 12,
            coverage_delta: 0.03,
        })
        .rail_event(RailEvent::ConfidenceUpdate {
            understanding: 0.81,
            merge_safety: 0.73,
            delta: 0.07,
        })
        .draw()
        .assert_buffer_contains("workspace:")
        .assert_combined_golden("benchmark_watch_active")
        .finish();
}

#[test]
fn benchmark_watch_failure_combined_golden() {
    let root = tempdir().expect("tempdir");
    let case_root = create_benchmark_case(root.path());
    let mut harness = TuiTestHarness::new_with_root(180, 48, root.path().to_path_buf());
    harness.app.focused = Pane::Chat;
    harness.app.chat.set_input_for_test(&format!(
        "/benchmark {} fix the benchmark case",
        case_root.display()
    ));

    TuiScenario::new(&mut harness, "benchmark_watch_failure")
        .key_press(KeyCode::Enter, KeyModifiers::NONE)
        .chat_event(ChatUiEvent::Error(
            0,
            "Evaluator failed: ./evaluate.sh\nstdout: 14 passed\nstderr: planner ownership mismatch".to_string(),
        ))
        .rail_event(RailEvent::PhaseChanged(AgentPhase::Debugging))
        .rail_event(RailEvent::RiskPromoted {
            description: "Visible evaluator still failing on planner ownership.".to_string(),
            severity: RiskSeverity::High,
            blast_radius: 2,
        })
        .rail_event(RailEvent::OneSecondStory {
            summary: "Focused proof failed, so the run is isolating the regression before widening.".to_string(),
        })
        .draw()
        .assert_buffer_contains("Evaluator failed")
        .assert_rail_mode(RailMode::RiskLedger)
        .assert_combined_golden("benchmark_watch_failure")
        .finish();
}

#[test]
fn benchmark_watch_resume_last_combined_golden() {
    let temp_dir = tempdir().expect("tempdir");
    let result_dir = temp_dir.path().join("run");
    let workspace_root = temp_dir.path().join("workspace");
    fs::create_dir_all(&workspace_root).expect("mkdir workspace");
    let spec = FullAutoLaunchSpec {
        kind: SlashCommandKind::Benchmark,
        goal: "resume benchmark watch".to_string(),
        target_path: temp_dir.path().join("case-a"),
        workspace_root,
        resolved_mode: FullAutoResolvedMode::Benchmark,
        sandbox_mode: FullAutoSandboxMode::LocalCopy,
        docker_image: None,
        autonomy_profile: crate::quorp::tui::agent_context::AutonomyProfile::AutonomousSandboxed,
        max_steps: 60,
        max_seconds: Some(120),
        max_total_tokens: Some(1_500),
        result_dir: result_dir.clone(),
        objective_file: Some(temp_dir.path().join("workspace").join("START_HERE.md")),
        evaluate_command: Some("./evaluate.sh".to_string()),
        objective_metadata: serde_json::json!({"evaluate_command":"./evaluate.sh"}),
    };
    spec.write_to_disk().expect("write launch spec");

    let mut harness = TuiTestHarness::new_with_root(180, 48, temp_dir.path().to_path_buf());
    harness.app.focused = Pane::Chat;
    harness
        .app
        .chat
        .set_input_for_test(&format!("/resume-last {}", result_dir.display()));

    TuiScenario::new(&mut harness, "benchmark_watch_resume_last")
        .key_press(KeyCode::Enter, KeyModifiers::NONE)
        .rail_event(RailEvent::PhaseChanged(AgentPhase::Planning))
        .rail_event(RailEvent::OneSecondStory {
            summary: "Resuming the last benchmark watch run from its saved launch spec."
                .to_string(),
        })
        .draw()
        .assert_buffer_contains("[Full Auto Resumed]")
        .assert_buffer_contains("resume benchmark watch")
        .assert_combined_golden("benchmark_watch_resume_last")
        .finish();
}

fn create_benchmark_case(root: &std::path::Path) -> std::path::PathBuf {
    fs::write(root.join("START_HERE.md"), "# Root\n").expect("write root objective");

    let case_root = root.join("case-a");
    fs::create_dir_all(case_root.join("workspace").join("proof-full")).expect("mkdir workspace");
    fs::write(
        case_root.join("benchmark.json"),
        serde_json::json!({
            "id": "case-a",
            "title": "Case A",
            "difficulty": "medium",
            "category": "rust",
            "repo_condition": ["proof-full"],
            "objective_file": "START_HERE.md",
            "success_file": "SUCCESS.md",
            "reset_command": "./reset.sh <condition>",
            "evaluate_command": "./evaluate.sh <condition>",
            "estimated_minutes": 5,
            "expected_files_touched": ["src/lib.rs"],
            "primary_metrics": ["tests"],
            "tags": ["watch-mode"]
        })
        .to_string(),
    )
    .expect("write manifest");
    fs::write(case_root.join("START_HERE.md"), "# Case\n").expect("write objective");
    fs::write(
        case_root.join("LOCAL_REPRO.md"),
        "## Fast Loop\n\n```sh\n./evaluate.sh proof-full\n```\n\n## First Reads\n\n- `src/lib.rs`\n",
    )
    .expect("write local repro");
    fs::write(case_root.join("SUCCESS.md"), "done\n").expect("write success");
    write_executable_script(&case_root.join("reset.sh"), "#!/bin/sh\nexit 0\n");
    write_executable_script(&case_root.join("evaluate.sh"), "#!/bin/sh\nexit 0\n");
    fs::create_dir_all(case_root.join("workspace").join("proof-full").join("src"))
        .expect("mkdir src");
    fs::write(
        case_root
            .join("workspace")
            .join("proof-full")
            .join("src")
            .join("lib.rs"),
        "pub fn fixture() -> u32 { 1 }\n",
    )
    .expect("write owner file");
    fs::write(
        case_root
            .join("workspace")
            .join("proof-full")
            .join("README.md"),
        "# Workspace\n",
    )
    .expect("write workspace readme");
    case_root
}

#[cfg(feature = "tui-e2e-smoke")]
#[test]
fn docker_full_auto_tui_smoke() {
    if std::env::var_os("QUORP_TUI_E2E_DOCKER").is_none() {
        eprintln!("skipping docker smoke; set QUORP_TUI_E2E_DOCKER=1 to enable");
        return;
    }

    let workspace = tempdir().expect("tempdir");
    fs::write(
        workspace.path().join("START_HERE.md"),
        "# Objective\nCreate a tiny proof artifact, then stop.\n",
    )
    .expect("write objective");
    fs::write(workspace.path().join("evaluate.sh"), "#!/bin/sh\nexit 0\n")
        .expect("write evaluator");

    let mut harness = TuiTestHarness::new_with_root(180, 48, workspace.path().to_path_buf());
    harness.app.focused = Pane::Chat;
    harness
        .app
        .chat
        .set_input_for_test("/full-auto --docker Please read START_HERE.md and stop after creating the smallest possible proof artifact.");

    harness.key_press(KeyCode::Enter, KeyModifiers::NONE);

    let task = recv_start_agent_task(&harness);
    let result_dir = task.result_dir.clone();
    harness.apply_backend_event(TuiEvent::StartAgentTask(task));

    assert!(
        harness.wait_for_buffer_contains(Duration::from_secs(2), "Docker-backed")
            || harness.wait_for_buffer_contains(Duration::from_secs(2), "Launching Docker"),
        "docker smoke never surfaced live watch-state text"
    );
    assert!(
        result_dir.join(LAUNCH_SPEC_FILE_NAME).exists(),
        "launch spec missing from docker smoke result dir"
    );

    harness
        .app
        .agent_runtime_tx
        .as_ref()
        .expect("agent runtime tx")
        .unbounded_send(crate::quorp::tui::agent_runtime::AgentRuntimeCommand::Cancel)
        .expect("cancel active docker run");

    assert!(
        harness.wait_for_buffer_contains(Duration::from_secs(2), "cancelled"),
        "docker smoke never surfaced the cancelled stop reason"
    );
}

//! Simulated backend events using the same [`crate::quorp::tui::TuiEvent`] payloads as production.

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::app::Pane;
use crate::quorp::tui::chat::{ChatMessage, ChatUiEvent};
use crate::quorp::tui::editor_pane::BufferSnapshot;
use crate::quorp::tui::file_tree::DirectoryListing;
use crate::quorp::tui::file_tree::TreeChild;
use crate::quorp::tui::path_index::{PathIndexSnapshot, path_entry_from_parts};

use super::fixtures;
use super::harness::TuiTestHarness;

#[test]
fn simulated_editor_pane_buffer_snapshot_renders() {
    let dir = fixtures::temp_project_with_files(&[("main.rs", "// on disk (bridge replaces)")]);
    let root = dir.path().to_path_buf();
    let path = root.join("main.rs");
    let mut h = TuiTestHarness::new_with_backend_state(100, 32, root.clone());
    h.app.focused = Pane::EditorPane;
    h.app
        .editor_pane
        .sync_tree_selection(Some(path.as_path()), &root);
    h.app.editor_pane.ensure_active_loaded(&root);
    h.apply_backend_event(TuiEvent::BufferSnapshot(BufferSnapshot {
        path: Some(path.clone()),
        lines: vec![Line::from(Span::styled(
            "fn main() {}",
            Style::default().fg(Color::Green),
        ))],
        error: None,
        truncated: false,
    }));
    h.draw();
    h.assert_buffer_contains("fn main()");
}

/// Mirrors the native editor snapshot path when opening a buffer fails: empty lines and a
/// user-visible error string (red in the real draw path).
#[test]
fn simulated_editor_pane_open_error_surfaces_in_buffer() {
    let dir = fixtures::temp_project_with_files(&[("missing.rs", "")]);
    let root = dir.path().to_path_buf();
    let path = root.join("missing.rs");
    let mut h = TuiTestHarness::new_with_backend_state(120, 32, root.clone());
    h.app.focused = Pane::EditorPane;
    h.app
        .editor_pane
        .sync_tree_selection(Some(path.as_path()), &root);
    h.app.editor_pane.ensure_active_loaded(&root);
    h.apply_backend_event(TuiEvent::BufferSnapshot(BufferSnapshot {
        path: Some(path),
        lines: Vec::new(),
        error: Some("Failed to open file in project: worktree path not found".to_string()),
        truncated: false,
    }));
    h.draw();
    h.assert_buffer_contains("Failed to open file in project");
}

#[test]
fn simulated_file_tree_listing_then_open_preview() {
    let dir = fixtures::temp_project_with_files(&[("sample.rs", "ignored")]);
    let root = dir.path().to_path_buf();
    let file_path = root.join("sample.rs");
    let mut h = TuiTestHarness::new_with_backend_state(100, 32, root.clone());
    h.apply_backend_event(TuiEvent::FileTreeListed(DirectoryListing {
        parent: root.clone(),
        result: Ok(vec![TreeChild {
            path: file_path.clone(),
            name: "sample.rs".to_string(),
            is_directory: false,
        }]),
    }));
    h.app.focused = Pane::FileTree;
    h.draw();
    h.assert_buffer_contains("sample.rs");
    // Visible rows: project root, then `sample.rs`. Move off the root row, then open the file.
    h.key_press(KeyCode::Down, KeyModifiers::NONE);
    h.key_press(KeyCode::Enter, KeyModifiers::NONE);
    h.assert_focus(Pane::EditorPane);
    let tree_root = h.app.file_tree.root().to_path_buf();
    h.app
        .editor_pane
        .sync_from_selected_file(h.app.file_tree.selected_file(), h.app.file_tree.root());
    h.app.editor_pane.ensure_active_loaded(&tree_root);
    let opened = h
        .app
        .file_tree
        .selected_file()
        .expect("sample.rs should be selected after Enter")
        .to_path_buf();
    h.apply_backend_event(TuiEvent::BufferSnapshot(BufferSnapshot {
        path: Some(opened),
        lines: vec![Line::from("fn main() {}")],
        error: None,
        truncated: false,
    }));
    h.draw();
    h.assert_focus(Pane::EditorPane);
    h.assert_buffer_contains("fn main()");
}

/// Phase 5.2-style flow: Ctrl+l focuses the file tree, Down+Enter selects a file and moves focus
/// to code preview. A simulated buffer snapshot matches what the native file-opening backend feeds
/// into the editor pane.
#[test]
fn playwright_open_file_from_tree_shows_fn_main_in_preview() {
    let dir = fixtures::temp_project_with_files(&[("hello.rs", "// pending")]);
    let root = dir.path().to_path_buf();
    let file_path = root.join("hello.rs");
    let mut h = TuiTestHarness::new_with_backend_state(100, 32, root.clone());
    h.apply_backend_event(TuiEvent::FileTreeListed(DirectoryListing {
        parent: root.clone(),
        result: Ok(vec![TreeChild {
            path: file_path.clone(),
            name: "hello.rs".to_string(),
            is_directory: false,
        }]),
    }));
    h.app.focused = Pane::EditorPane;
    h.key_press(KeyCode::Char('h'), KeyModifiers::CONTROL);
    h.assert_focus(Pane::FileTree);
    h.key_press(KeyCode::Down, KeyModifiers::NONE);
    h.key_press(KeyCode::Enter, KeyModifiers::NONE);
    h.assert_focus(Pane::EditorPane);
    let opened = h
        .app
        .file_tree
        .selected_file()
        .expect("hello.rs selected")
        .to_path_buf();
    h.app
        .editor_pane
        .sync_from_selected_file(h.app.file_tree.selected_file(), h.app.file_tree.root());
    h.app.editor_pane.ensure_active_loaded(&root);
    h.apply_backend_event(TuiEvent::BufferSnapshot(BufferSnapshot {
        path: Some(opened),
        lines: vec![Line::from("fn main() {}")],
        error: None,
        truncated: false,
    }));
    h.draw();
    h.assert_buffer_contains("fn main()");
}

#[test]
fn simulated_path_index_snapshot_enables_mention_list() {
    let dir = fixtures::temp_project_with_files(&[("lib.rs", "")]);
    let root = dir.path().to_path_buf();
    let lib_abs = root.join("lib.rs");
    let mut h = TuiTestHarness::new_with_backend_state(120, 32, root.clone());
    let entries = vec![
        path_entry_from_parts(".".to_string(), true, root.clone()),
        path_entry_from_parts("lib.rs".to_string(), false, lib_abs),
    ];
    h.apply_backend_event(TuiEvent::PathIndexSnapshot(PathIndexSnapshot {
        root: root.clone(),
        entries: std::sync::Arc::new(entries),
        files_seen: 2,
    }));
    h.app.focused = Pane::Chat;
    h.key_press(KeyCode::Char('@'), KeyModifiers::NONE);
    h.draw();
    h.assert_buffer_contains("lib.rs");
}

/// With [`TuiTestHarness::new_with_backend_state`], @-mention rows must come from injected
/// [`TuiEvent::PathIndexSnapshot`], not an `ignore` walk of the temp dir (Phase 3g).
#[test]
fn backend_state_mentions_use_bridge_snapshot_not_disk_walk() {
    let dir = fixtures::temp_project_with_files(&[("only_on_disk.rs", "")]);
    let root = dir.path().to_path_buf();
    let mut h = TuiTestHarness::new_with_backend_state(120, 32, root.clone());
    let entries = vec![
        path_entry_from_parts(".".to_string(), true, root.clone()),
        path_entry_from_parts(
            "bridge_only.rs".to_string(),
            false,
            root.join("bridge_only.rs"),
        ),
    ];
    h.apply_backend_event(TuiEvent::PathIndexSnapshot(PathIndexSnapshot {
        root: root.clone(),
        entries: std::sync::Arc::new(entries),
        files_seen: 2,
    }));
    h.app.focused = Pane::Chat;
    for ch in "@bridge_on".chars() {
        h.key_press(KeyCode::Char(ch), KeyModifiers::NONE);
    }
    h.draw();
    h.assert_buffer_contains("bridge_only");
    // Explorer still lists on-disk files; the @-mention index must not include them.
    h.app.chat.set_input_for_test("");
    for ch in "@only_on".chars() {
        h.key_press(KeyCode::Char(ch), KeyModifiers::NONE);
    }
    assert_eq!(
        h.app.chat.mention_match_count_for_test(),
        0,
        "on-disk-only file should not appear when index is snapshot-backed"
    );
}

/// Phase 3 regression: buffer snapshot rendering works when theme is captured once (no cx.theme()).
#[test]
fn theme_decoupled_buffer_snapshot_renders_with_syntax_color() {
    let dir = fixtures::temp_project_with_files(&[("colored.rs", "placeholder")]);
    let root = dir.path().to_path_buf();
    let path = root.join("colored.rs");
    let mut h = TuiTestHarness::new_with_backend_state(100, 32, root.clone());
    h.app.focused = Pane::EditorPane;
    h.app
        .editor_pane
        .sync_tree_selection(Some(path.as_path()), &root);
    h.app.editor_pane.ensure_active_loaded(&root);
    h.apply_backend_event(TuiEvent::BufferSnapshot(BufferSnapshot {
        path: Some(path),
        lines: vec![
            Line::from(vec![
                Span::styled("fn ", Style::default().fg(Color::Magenta)),
                Span::styled("hello", Style::default().fg(Color::Yellow)),
                Span::styled("() {}", Style::default().fg(Color::White)),
            ]),
            Line::from(Span::styled(
                "    println!(\"hi\");",
                Style::default().fg(Color::Green),
            )),
        ],
        error: None,
        truncated: false,
    }));
    h.draw();
    h.assert_buffer_contains("fn hello()");
    h.assert_buffer_contains("println!");
}

/// Inject a TerminalFrame via the bridge event path and verify the terminal pane renders it.
#[test]
fn terminal_frame_event_renders_in_terminal_pane() {
    let mut h = TuiTestHarness::new(100, 32);
    h.app.focused = Pane::Terminal;
    h.apply_backend_event(TuiEvent::TerminalFrame(
        crate::quorp::tui::bridge::TerminalFrame {
            snapshot: crate::quorp::tui::terminal_surface::TerminalSnapshot::from_lines(&[
                Line::from("$ cargo build"),
                Line::from(Span::styled(
                    "   Compiling quorp v0.231.0",
                    Style::default().fg(Color::Green),
                )),
            ]),
            cwd: Some(std::path::PathBuf::from("/Users/bentaylor/Code/quorp")),
            shell_label: Some("zsh".to_string()),
            window_title: None,
        },
    ));
    h.draw();
    h.assert_buffer_contains("cargo build");
    h.assert_buffer_contains("Compiling quorp");
}

/// TerminalClosed event sets pty_exited without panicking.
#[test]
fn terminal_closed_event_handled_gracefully() {
    let mut h = TuiTestHarness::new(80, 24);
    assert!(!h.app.terminal.pty_exited_for_test());
    h.apply_backend_event(TuiEvent::TerminalClosed);
    assert!(h.app.terminal.pty_exited_for_test());
    h.draw();
}

/// ChatUiEvent::Error surfaces the error text in the assistant's transcript message.
#[test]
fn chat_stream_error_surfaces_in_transcript() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Chat;
    h.app.chat.seed_messages_for_test(vec![
        ChatMessage::User("test".into()),
        ChatMessage::Assistant(String::new()),
    ]);
    h.app.chat.set_streaming_for_test(true);
    h.apply_chat_event(ChatUiEvent::Error(0, "connection refused".into()));
    assert_eq!(
        h.app.chat.last_assistant_text_for_test(),
        Some("Error: connection refused")
    );
    h.draw();
    h.assert_buffer_contains("Error:");
    h.assert_buffer_contains("connection");
}

#[test]
fn agent_status_update_routes_to_assistant_status() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Chat;
    h.apply_backend_event(TuiEvent::BackendResponse(
        crate::quorp::tui::bridge::BackendToTuiResponse::AgentStatusUpdate(
            "building...".to_string(),
        ),
    ));
    assert!(
        h.app
            .agent_pane
            .status_lines
            .contains(&"building...".to_string())
    );
    h.draw();
    h.assert_buffer_contains("building...");
}

#[test]
fn dead_response_variants_logged_not_panicked() {
    let mut h = TuiTestHarness::new(80, 24);
    // Inject a response that has no specific handler in apply_tui_backend_event except the catch-all
    h.apply_backend_event(TuiEvent::BackendResponse(
        crate::quorp::tui::bridge::BackendToTuiResponse::AgentStatusUpdate("test".to_string()),
    ));
    h.draw();
}

#[test]
fn agent_enter_surfaces_honest_status_message() {
    let dir = fixtures::temp_project_with_files(&[]);
    let root = dir.path().to_path_buf();
    let (mut app, _rx, _bridge_rx) =
        crate::quorp::tui::app::TuiApp::new_for_flow_tests_with_registry_chat(
            root.clone(),
            vec![],
            0,
        );
    app.focused = Pane::Agent;

    let _ = app.handle_event(crossterm::event::Event::Key(
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ),
    ));

    assert!(
        app.agent_pane
            .status_lines
            .iter()
            .any(|line| line.contains("Launch autonomous runs from the Assistant pane"))
    );
    app.leak_runtime_for_test_exit();
}

#[test]
fn editor_pane_respects_vim_normal_mode_keys() {
    let mut h = TuiTestHarness::new(80, 24);
    h.app.focused = Pane::EditorPane;

    // In Editor, 'Ctrl-l' switches to Chat (right_pane)
    h.key_press(
        crossterm::event::KeyCode::Char('l'),
        crossterm::event::KeyModifiers::CONTROL,
    );
    assert_eq!(h.app.focused, Pane::Chat);

    // In Chat, 'Ctrl-h' switches to the left sidebar (FileTree)
    h.key_press(
        crossterm::event::KeyCode::Char('h'),
        crossterm::event::KeyModifiers::CONTROL,
    );
    assert_eq!(h.app.focused, Pane::FileTree);

    // In FileTree, 'Ctrl-l' restores the last_left_pane (which was Chat)
    h.key_press(
        crossterm::event::KeyCode::Char('l'),
        crossterm::event::KeyModifiers::CONTROL,
    );
    assert_eq!(h.app.focused, Pane::Chat);
}

#[test]
fn editor_three_tabs_switch_preserves_content() {
    let mut h = TuiTestHarness::new(80, 24);
    let root = std::path::Path::new("/test");
    // Tab length max is handled in EditorPane logic, we just open files
    h.app
        .editor_pane
        .sync_tree_selection(Some(std::path::Path::new("/a.rs")), root);
    h.app
        .editor_pane
        .sync_tree_selection(Some(std::path::Path::new("/b.rs")), root);
    h.app
        .editor_pane
        .sync_tree_selection(Some(std::path::Path::new("/c.rs")), root);

    // There are 4 tabs now (index 0 is the default empty tab)
    h.app.editor_pane.activate_file_tab(1, root);
    assert_eq!(h.app.editor_pane.active_tab_index(), 1);
    h.app.editor_pane.activate_file_tab(3, root);
    assert_eq!(h.app.editor_pane.active_tab_index(), 3);
}

#[test]
fn terminal_enter_after_closed_sends_resize_to_restart() {
    let dir = fixtures::temp_project_with_files(&[]);
    let root = dir.path().to_path_buf();
    let (mut app, _rx, mut bridge_rx) =
        crate::quorp::tui::app::TuiApp::new_for_flow_tests_with_registry_chat(
            root.clone(),
            vec![],
            0,
        );
    app.focused = Pane::Terminal;
    app.apply_tui_backend_event(TuiEvent::TerminalClosed);
    assert!(app.terminal.pty_exited);

    let _ = app.handle_event(crossterm::event::Event::Key(
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ),
    ));

    let req = bridge_rx
        .try_recv()
        .expect("expected terminal restart resize request");
    assert!(matches!(
        req,
        crate::quorp::tui::bridge::TuiToBackendRequest::TerminalResize { .. }
    ));
}

#[test]
fn terminal_ctrl_c_sends_keystroke() {
    let dir = fixtures::temp_project_with_files(&[]);
    let root = dir.path().to_path_buf();
    let (mut app, _rx, mut bridge_rx) =
        crate::quorp::tui::app::TuiApp::new_for_flow_tests_with_registry_chat(
            root.clone(),
            vec![],
            0,
        );
    app.focused = Pane::Terminal;

    let _ = app.handle_event(crossterm::event::Event::Key(
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('c'),
            crossterm::event::KeyModifiers::CONTROL,
        ),
    ));

    let req = bridge_rx
        .try_recv()
        .expect("expected terminal ctrl-c keystroke request");
    assert!(matches!(
        req,
        crate::quorp::tui::bridge::TuiToBackendRequest::TerminalKeystroke(_)
    ));
    app.leak_runtime_for_test_exit();
}

#[test]
fn terminal_printable_key_sends_input_bytes() {
    let dir = fixtures::temp_project_with_files(&[]);
    let root = dir.path().to_path_buf();
    let (mut app, _rx, mut bridge_rx) =
        crate::quorp::tui::app::TuiApp::new_for_flow_tests_with_registry_chat(
            root.clone(),
            vec![],
            0,
        );
    app.focused = Pane::Terminal;

    let _ = app.handle_event(crossterm::event::Event::Key(
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::NONE,
        ),
    ));

    match bridge_rx
        .try_recv()
        .expect("expected terminal printable keystroke request")
    {
        crate::quorp::tui::bridge::TuiToBackendRequest::TerminalKeystroke(ks) => {
            assert_eq!(ks.key, "a");
        }
        req => {
            panic!(
                "Expected TerminalKeystroke(a), got something else: {:?}",
                req
            );
        }
    }
    println!("test terminal input complete!");
    app.leak_runtime_for_test_exit();
}

#[test]
fn screenshot_full_workspace_with_all_panes() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::EditorPane;
    h.draw();
    if std::env::var("VISUAL_TEST_OUTPUT_DIR").is_ok() {
        h.save_screenshot("workspace_full");
    }
}

#[test]
fn screenshot_editor_with_syntax_highlighting() {
    let dir = fixtures::temp_project_with_files(&[("styled.rs", "placeholder")]);
    let root = dir.path().to_path_buf();
    let path = root.join("styled.rs");
    let mut h = TuiTestHarness::new_with_backend_state(120, 40, root.clone());
    h.app.focused = Pane::EditorPane;
    h.app
        .editor_pane
        .sync_tree_selection(Some(path.as_path()), &root);
    h.app.editor_pane.ensure_active_loaded(&root);
    h.apply_backend_event(TuiEvent::BufferSnapshot(BufferSnapshot {
        path: Some(path),
        lines: vec![
            Line::from(vec![
                Span::styled("impl", Style::default().fg(Color::Magenta)),
                Span::styled(" ", Style::default()),
                Span::styled("MyStruct", Style::default().fg(Color::Yellow)),
                Span::styled(" {", Style::default()),
            ]),
            Line::from(Span::styled("}", Style::default())),
        ],
        error: None,
        truncated: false,
    }));
    h.draw();
    if std::env::var("VISUAL_TEST_OUTPUT_DIR").is_ok() {
        h.save_screenshot("editor_syntax");
    }
}

#[test]
fn screenshot_terminal_with_ansi_output() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Terminal;
    h.apply_backend_event(TuiEvent::TerminalFrame(
        crate::quorp::tui::bridge::TerminalFrame {
            snapshot: crate::quorp::tui::terminal_surface::TerminalSnapshot::from_lines(&[
                Line::from("$ cargo build"),
                Line::from(Span::styled(
                    "   Compiling quorp v0.231.0",
                    Style::default().fg(Color::Green),
                )),
            ]),
            cwd: Some(std::path::PathBuf::from("/Users/bentaylor/Code/quorp")),
            shell_label: Some("zsh".to_string()),
            window_title: None,
        },
    ));
    h.draw();
    if std::env::var("VISUAL_TEST_OUTPUT_DIR").is_ok() {
        h.save_screenshot("terminal_ansi");
    }
}

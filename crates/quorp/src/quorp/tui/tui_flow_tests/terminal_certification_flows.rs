use std::ops::ControlFlow;

use crossterm::event::{Event, KeyCode, KeyModifiers};
use futures::StreamExt as _;
use ratatui::layout::Rect;

use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::app::{Overlay, Pane};
use crate::quorp::tui::bridge::{TerminalFrame, TuiToBackendRequest};
use crate::quorp::tui::shell::ShellGeometry;
use crate::quorp::tui::terminal_surface::TerminalSnapshot;

use super::harness::TuiTestHarness;

fn next_terminal_request(
    bridge_rx: &mut futures::channel::mpsc::UnboundedReceiver<TuiToBackendRequest>,
) -> TuiToBackendRequest {
    loop {
        let request = futures::executor::block_on(bridge_rx.next()).expect("bridge request");
        if !matches!(
            request,
            TuiToBackendRequest::TerminalResize { .. }
                | TuiToBackendRequest::TerminalFocusChanged { .. }
        ) {
            return request;
        }
    }
}

#[test]
fn capture_mode_forwards_terminal_shortcuts_instead_of_opening_overlays() {
    let (bridge_tx, mut bridge_rx) = futures::channel::mpsc::unbounded();
    let mut h = TuiTestHarness::new_with_terminal_bridge(100, 32, bridge_tx);
    h.app.focused = Pane::Terminal;
    h.app.terminal.spawn_pty(100, 32).expect("spawn pty");

    let cases = [
        (KeyCode::Tab, KeyModifiers::NONE, "tab", false, false, false),
        (
            KeyCode::Esc,
            KeyModifiers::NONE,
            "escape",
            false,
            false,
            false,
        ),
        (
            KeyCode::Char('?'),
            KeyModifiers::NONE,
            "?",
            false,
            false,
            false,
        ),
        (
            KeyCode::Char('b'),
            KeyModifiers::CONTROL,
            "b",
            true,
            false,
            false,
        ),
        (
            KeyCode::Char('p'),
            KeyModifiers::CONTROL,
            "p",
            true,
            false,
            false,
        ),
        (
            KeyCode::Char('n'),
            KeyModifiers::CONTROL,
            "n",
            true,
            false,
            false,
        ),
        (
            KeyCode::Char('m'),
            KeyModifiers::CONTROL,
            "m",
            true,
            false,
            false,
        ),
        (
            KeyCode::Char('s'),
            KeyModifiers::CONTROL,
            "s",
            true,
            false,
            false,
        ),
        (
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            "c",
            true,
            false,
            false,
        ),
        (
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
            "d",
            true,
            false,
            false,
        ),
        (
            KeyCode::Char('l'),
            KeyModifiers::CONTROL,
            "l",
            true,
            false,
            false,
        ),
    ];

    for (code, modifiers, expected_key, expected_ctrl, expected_alt, expected_shift) in cases {
        let flow = h.key(code, modifiers);
        assert!(flow.is_continue(), "event {code:?} should continue");
        h.assert_focus(Pane::Terminal);
        h.assert_overlay(Overlay::None);
        match next_terminal_request(&mut bridge_rx) {
            TuiToBackendRequest::TerminalKeystroke(keystroke) => {
                assert_eq!(keystroke.key, expected_key);
                assert_eq!(keystroke.modifiers.control, expected_ctrl);
                assert_eq!(keystroke.modifiers.alt, expected_alt);
                assert_eq!(keystroke.modifiers.shift, expected_shift);
            }
            other => panic!("expected terminal keystroke for {code:?}, got {other:?}"),
        }
    }
}

#[test]
fn capture_mode_uses_escape_hatches_and_reenters_capture() {
    let mut h = TuiTestHarness::new(100, 32);
    h.app.focused = Pane::Terminal;
    assert!(h.app.terminal.in_capture_mode());

    let flow = h.key(KeyCode::Char('g'), KeyModifiers::CONTROL);
    assert!(flow.is_continue());
    assert!(!h.app.terminal.in_capture_mode());
    h.assert_focus(Pane::Terminal);

    let flow = h.key(KeyCode::Enter, KeyModifiers::NONE);
    assert!(flow.is_continue());
    assert!(h.app.terminal.in_capture_mode());

    let flow = h.key(KeyCode::Char('3'), KeyModifiers::ALT);
    assert!(flow.is_continue());
    h.assert_focus(Pane::Chat);
}

#[test]
fn terminal_paste_adds_bracketed_markers_when_terminal_requests_it() {
    let (bridge_tx, mut bridge_rx) = futures::channel::mpsc::unbounded();
    let mut h = TuiTestHarness::new_with_terminal_bridge(100, 32, bridge_tx);
    h.app.focused = Pane::Terminal;
    let mut snapshot = TerminalSnapshot::blank(24, 80);
    snapshot.bracketed_paste = true;
    h.apply_backend_event(TuiEvent::TerminalFrame(TerminalFrame {
        snapshot,
        cwd: None,
        shell_label: Some("zsh".to_string()),
        window_title: None,
    }));

    let flow = h
        .app
        .handle_event(Event::Paste("echo hi\nsecond line".to_string()));
    assert!(matches!(flow, ControlFlow::Continue(())));
    match next_terminal_request(&mut bridge_rx) {
        TuiToBackendRequest::TerminalInput(bytes) => {
            assert!(
                bytes.starts_with(b"\x1b[200~"),
                "missing bracketed paste prefix: {bytes:?}"
            );
            assert!(
                bytes.ends_with(b"\x1b[201~"),
                "missing bracketed paste suffix: {bytes:?}"
            );
        }
        other => panic!("expected terminal input paste, got {other:?}"),
    }
}

#[test]
fn terminal_paste_omits_bracketed_markers_when_terminal_does_not_request_it() {
    let (bridge_tx, mut bridge_rx) = futures::channel::mpsc::unbounded();
    let mut h = TuiTestHarness::new_with_terminal_bridge(100, 32, bridge_tx);
    h.app.focused = Pane::Terminal;
    h.apply_backend_event(TuiEvent::TerminalFrame(TerminalFrame {
        snapshot: TerminalSnapshot::blank(24, 80),
        cwd: None,
        shell_label: Some("zsh".to_string()),
        window_title: None,
    }));

    let flow = h.app.handle_event(Event::Paste("echo hi".to_string()));
    assert!(matches!(flow, ControlFlow::Continue(())));
    match next_terminal_request(&mut bridge_rx) {
        TuiToBackendRequest::TerminalInput(bytes) => {
            assert_eq!(bytes, b"echo hi");
        }
        other => panic!("expected terminal input paste, got {other:?}"),
    }
}

#[test]
fn shift_page_down_sends_scrollback_request_and_preserves_capture_mode() {
    let (bridge_tx, mut bridge_rx) = futures::channel::mpsc::unbounded();
    let mut h = TuiTestHarness::new_with_terminal_bridge(80, 24, bridge_tx);
    h.app.focused = Pane::Terminal;
    h.app.terminal.spawn_pty(80, 24).expect("spawn pty");

    let flow = h.key(KeyCode::PageDown, KeyModifiers::SHIFT);
    assert!(flow.is_continue());
    assert!(h.app.terminal.in_capture_mode());

    match next_terminal_request(&mut bridge_rx) {
        TuiToBackendRequest::TerminalScrollPageDown => {}
        other => panic!("expected scroll-page-down request, got {other:?}"),
    }
}

#[test]
fn shift_page_up_sends_scrollback_request_and_preserves_capture_mode() {
    let (bridge_tx, mut bridge_rx) = futures::channel::mpsc::unbounded();
    let mut h = TuiTestHarness::new_with_terminal_bridge(80, 24, bridge_tx);
    h.app.focused = Pane::Terminal;
    h.app.terminal.spawn_pty(80, 24).expect("spawn pty");

    let flow = h.key(KeyCode::PageUp, KeyModifiers::SHIFT);
    assert!(flow.is_continue());
    assert!(h.app.terminal.in_capture_mode());

    match next_terminal_request(&mut bridge_rx) {
        TuiToBackendRequest::TerminalScrollPageUp => {}
        other => panic!("expected scroll-page-up request, got {other:?}"),
    }
}

#[test]
fn alt_screen_fullscreen_keeps_footer_pinned_and_restores_layout() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Terminal;
    let mut snapshot = TerminalSnapshot::blank(18, 80);
    snapshot.alternate_screen = true;
    h.apply_backend_event(TuiEvent::TerminalFrame(TerminalFrame {
        snapshot,
        cwd: Some("/Users/bentaylor/Code/quorp".into()),
        shell_label: Some("zsh".to_string()),
        window_title: Some("vim".to_string()),
    }));

    let fullscreen_state = h.app.shell_state_snapshot(Rect::new(0, 0, 120, 40));
    let fullscreen_geometry = ShellGeometry::for_state(Rect::new(0, 0, 120, 40), &fullscreen_state);
    assert!(fullscreen_state.terminal.fullscreen);
    assert_eq!(
        fullscreen_geometry.footer.y + fullscreen_geometry.footer.height,
        40
    );
    assert_eq!(fullscreen_geometry.sidebar.width, 0);

    let mut restored = TerminalSnapshot::blank(18, 80);
    restored.alternate_screen = false;
    h.apply_backend_event(TuiEvent::TerminalFrame(TerminalFrame {
        snapshot: restored,
        cwd: Some("/Users/bentaylor/Code/quorp".into()),
        shell_label: Some("zsh".to_string()),
        window_title: None,
    }));
    let restored_state = h.app.shell_state_snapshot(Rect::new(0, 0, 120, 40));
    let restored_geometry = ShellGeometry::for_state(Rect::new(0, 0, 120, 40), &restored_state);
    assert!(!restored_state.terminal.fullscreen);
    assert_eq!(restored_geometry.sidebar.width, 0);
    assert!(restored_geometry.proof_rail.is_some());
    assert_eq!(restored_state.terminal.title, "Terminal");
}

#[test]
fn terminal_metadata_and_resize_follow_latest_frame() {
    let (bridge_tx, _bridge_rx) = futures::channel::mpsc::unbounded();
    let mut h = TuiTestHarness::new_with_terminal_bridge(80, 24, bridge_tx);
    h.app.focused = Pane::Terminal;
    h.apply_backend_event(TuiEvent::TerminalFrame(TerminalFrame {
        snapshot: TerminalSnapshot::blank(24, 80),
        cwd: Some("/tmp/quorp-a".into()),
        shell_label: Some("zsh".to_string()),
        window_title: Some("less".to_string()),
    }));

    let shell = h.app.shell_state_snapshot(Rect::new(0, 0, 80, 24));
    assert_eq!(shell.terminal.title, "less");
    assert_eq!(shell.terminal.detail_label.as_deref(), Some("/tmp/quorp-a"));

    h.apply_backend_event(TuiEvent::TerminalFrame(TerminalFrame {
        snapshot: TerminalSnapshot::blank(40, 120),
        cwd: Some("/tmp/quorp-b".into()),
        shell_label: Some("zsh".to_string()),
        window_title: None,
    }));
    let resized = h.app.shell_state_snapshot(Rect::new(0, 0, 120, 40));
    assert_eq!(resized.terminal.title, "Terminal");
    assert_eq!(
        resized.terminal.detail_label.as_deref(),
        Some("/tmp/quorp-b")
    );
}

#[test]
fn resize_while_terminal_focused_keeps_terminal_selected() {
    let mut h = TuiTestHarness::new(80, 24);
    h.app.focused = Pane::Terminal;
    h.resize(120, 40);
    h.draw();
    h.assert_focus(Pane::Terminal);
    assert!(
        h.app.terminal.in_capture_mode(),
        "terminal should remain in capture after resize"
    );
}

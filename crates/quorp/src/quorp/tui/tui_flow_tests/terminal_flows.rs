use crossterm::event::{KeyCode, KeyModifiers};
use futures::StreamExt as _;

use crate::quorp::tui::app::Pane;
use crate::quorp::tui::bridge::TuiToBackendRequest;

use super::harness::TuiTestHarness;

#[test]
fn shift_page_up_sends_scroll_request_when_terminal_focused() {
    let (btx, mut brx) = futures::channel::mpsc::unbounded();
    let mut h = TuiTestHarness::new_with_terminal_bridge(80, 24, btx);
    h.app.focused = Pane::Terminal;
    h.app
        .terminal
        .spawn_pty(80, 24)
        .expect("init terminal grid for test");
    let page_up = crossterm::event::KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT);
    let handled = h.app.terminal.try_handle_key(&page_up).expect("key");
    assert!(handled);
    let mut msg = futures::executor::block_on(brx.next());
    if matches!(msg, Some(TuiToBackendRequest::TerminalResize { .. })) {
        msg = futures::executor::block_on(brx.next());
    }
    assert!(
        matches!(msg, Some(TuiToBackendRequest::TerminalScrollPageUp)),
        "expected ScrollPageUp after optional Resize, got {msg:?}"
    );
}

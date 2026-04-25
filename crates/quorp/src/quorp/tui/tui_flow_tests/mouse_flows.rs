use crate::quorp::tui::app::{Pane, SplitterVisualState};
use crate::quorp::tui::shell::ShellGeometry;
use crate::quorp::tui::theme::Theme;
use crate::quorp::tui::workbench;

use super::harness::TuiTestHarness;

fn prism_vertical_split_center(h: &mut TuiTestHarness) -> (u16, u16) {
    let full = *h.buffer().area();
    let layout = h.app.workbench_layout_snapshot(full);
    // DFS order: root vertical (editor stack | chat) first, then horizontal (editor | terminal).
    let div = layout
        .splitters
        .first()
        .expect("expected PrismForge vertical splitter at index 0");
    (
        div.x,
        div.y.saturating_add(div.height.saturating_sub(1) / 2),
    )
}

#[test]
fn clicks_focus_code_terminal_and_chat_regions() {
    let mut h = TuiTestHarness::new(232, 64);
    h.app.terminal_dock_open = true;
    h.app.explorer_collapsed = false;
    h.draw();
    let full = *h.buffer().area();
    let state = h.app.shell_state_snapshot(full);
    let geometry = ShellGeometry::for_state(full, &state);
    let explorer = geometry.explorer.expect("explorer");
    let dock = geometry.dock.expect("terminal dock");
    let center = geometry.center;

    h.app.focused = Pane::Chat;
    h.mouse_left_down(explorer.x + 2, explorer.y + 2);
    h.assert_focus(Pane::FileTree);

    h.mouse_left_down(center.x + 2, center.y + 2);
    h.assert_focus(Pane::Chat);

    h.mouse_left_down(dock.x + 2, dock.y + 2);
    h.assert_focus(Pane::Terminal);
}

#[test]
fn splitter_hover_sets_visual_state_prismforge() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.theme = Theme::prism_forge();
    h.app.workspace = workbench::default_prismforge_tree();
    h.app.prismforge_dynamic_layout = true;
    h.draw();
    assert_eq!(h.app.splitter_visual_state, SplitterVisualState::Idle);
    let (cx, cy) = prism_vertical_split_center(&mut h);
    h.mouse_move_to(cx, cy);
    assert!(
        matches!(
            h.app.splitter_visual_state,
            SplitterVisualState::Hover { .. }
        ),
        "expected Hover over splitter, got {:?}",
        h.app.splitter_visual_state
    );
}

#[test]
fn splitter_drag_updates_vertical_ratio_prismforge() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.theme = Theme::prism_forge();
    h.app.workspace = workbench::default_prismforge_tree();
    h.app.prismforge_dynamic_layout = true;
    h.draw();
    let before = workbench::prismforge_ratios_from_tree(&h.app.workspace).0;
    let (cx, cy) = prism_vertical_split_center(&mut h);
    h.mouse_left_down(cx, cy);
    let drag_x = (cx + 22).min(h.buffer().area().width.saturating_sub(2));
    h.mouse_move_to(drag_x, cy);
    h.mouse_drag_left(drag_x, cy);
    h.mouse_left_up(drag_x, cy);
    h.draw();
    let after = workbench::prismforge_ratios_from_tree(&h.app.workspace).0;
    assert_ne!(
        before, after,
        "vertical ratio should change after splitter drag"
    );
}

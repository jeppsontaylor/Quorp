#![allow(unused)]
use std::collections::HashMap;

use ratatui::layout::Rect;

use crate::quorp::tui::theme::Metrics;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct LeafId(pub u16);

#[derive(Clone, Debug)]
pub enum WorkspaceNode {
    Split {
        axis: Axis,
        ratio_bp: u16,
        divider: u16,
        a: Box<WorkspaceNode>,
        b: Box<WorkspaceNode>,
    },
    Leaf(LeafId),
}

pub struct ShellRects {
    pub titlebar: Rect,
    pub activity: Rect,
    pub explorer_header: Rect,
    pub explorer_body: Rect,
    pub explorer_divider: Rect,
    pub workspace: Rect,
    pub statusbar: Rect,
}

pub struct LeafRects {
    pub tabs: Rect,
    pub body: Rect,
    pub scrollbar: Rect,
    pub banner: Option<Rect>,
    pub panel_tabs: Option<Rect>,
    pub composer: Option<Rect>,
}

pub struct WorkbenchLayout {
    pub leaves: HashMap<LeafId, LeafRects>,
    pub splitters: Vec<Rect>,
}

pub fn default_core_tui_tree() -> WorkspaceNode {
    WorkspaceNode::Split {
        axis: Axis::Vertical,
        ratio_bp: 3560,
        divider: 1,
        a: Box::new(WorkspaceNode::Split {
            axis: Axis::Horizontal,
            ratio_bp: 1700,
            divider: 0,
            a: Box::new(WorkspaceNode::Leaf(LeafId(1))),
            b: Box::new(WorkspaceNode::Leaf(LeafId(2))),
        }),
        b: Box::new(WorkspaceNode::Split {
            axis: Axis::Horizontal,
            ratio_bp: 7500,
            divider: 1,
            a: Box::new(WorkspaceNode::Leaf(LeafId(3))),
            b: Box::new(WorkspaceNode::Leaf(LeafId(4))),
        }),
    }
}

/// Mock1 explorer / chat / center constraints (`mock1.txt` lines 136–140).
pub const PRISMFORGE_EXPLORER_MIN: u16 = 24;
pub const PRISMFORGE_EXPLORER_MAX: u16 = 40;
pub const PRISMFORGE_CHAT_MIN: u16 = 38;
pub const PRISMFORGE_CHAT_MAX: u16 = 72;
pub const PRISMFORGE_CENTER_MIN: u16 = 60;

/// Clamps explorer width to Mock1 min/max; returns `(explorer_w, workspace_cols)` after activity + explorer + divider.
pub fn clamp_prismforge_horizontal_triple(
    full_width: u16,
    activity_w: u16,
    explorer_desired: u16,
    explorer_divider: u16,
) -> (u16, u16) {
    let explorer = explorer_desired.clamp(PRISMFORGE_EXPLORER_MIN, PRISMFORGE_EXPLORER_MAX);
    let before_workspace = activity_w.saturating_add(explorer).saturating_add(explorer_divider);
    let workspace_w = full_width.saturating_sub(before_workspace);
    (explorer, workspace_w)
}

fn ratio_bp_for_left_segment(usable: u16, left_cols: u16) -> u16 {
    if usable == 0 {
        return 5000;
    }
    let left = (left_cols.min(usable)) as u32;
    let u = usable as u32;
    let numerator = left.saturating_mul(10000).saturating_sub(5000).saturating_add(u - 1);
    (numerator / u).min(65535) as u16
}

/// Chat width for the vertical split inside `workspace`. Enforces center ≥ 60 only when the workspace is wide enough; on narrow terminals keeps desired chat up to `usable - 1` (Mock1 120×40: chat 48, center 36).
pub fn clamp_prismforge_chat_cols(workspace_cols: u16, chat_desired: u16) -> u16 {
    let split = 1u16;
    let usable = workspace_cols.saturating_sub(split);
    if usable == 0 {
        return PRISMFORGE_CHAT_MIN;
    }
    let desired = chat_desired.clamp(PRISMFORGE_CHAT_MIN, PRISMFORGE_CHAT_MAX);
    let min_usable_for_center_rule =
        PRISMFORGE_CENTER_MIN.saturating_add(PRISMFORGE_CHAT_MIN);
    if usable >= min_usable_for_center_rule {
        let mut chat = desired.min(usable.saturating_sub(1));
        let left = usable.saturating_sub(chat);
        if left < PRISMFORGE_CENTER_MIN {
            chat = usable
                .saturating_sub(PRISMFORGE_CENTER_MIN)
                .clamp(PRISMFORGE_CHAT_MIN, PRISMFORGE_CHAT_MAX);
        }
        chat
    } else {
        let cap = usable.saturating_sub(1);
        desired.min(cap).max(PRISMFORGE_CHAT_MIN.min(cap))
    }
}

/// Workspace tree from `workspace` rect and Mock1 metrics (dynamic `ratio_bp`).
pub fn prismforge_tree_for_workspace(workspace: Rect, metrics: &Metrics) -> WorkspaceNode {
    let usable_v = workspace.width.saturating_sub(1);
    let chat = clamp_prismforge_chat_cols(workspace.width, metrics.default_assistant_width);
    let left_w = usable_v.saturating_sub(chat);
    let vertical_ratio = ratio_bp_for_left_segment(usable_v, left_w);

    let h_div: u16 = 1;
    let usable_h = workspace.height.saturating_sub(h_div);
    let term = metrics
        .default_terminal_height
        .clamp(8u16, 20u16)
        .min(usable_h.saturating_sub(4));
    let top_h = usable_h.saturating_sub(term);
    let horizontal_ratio = ratio_bp_for_left_segment(usable_h, top_h);

    prismforge_tree_with_ratios(vertical_ratio, horizontal_ratio, h_div)
}

/// Same topology as [`prismforge_tree_for_workspace`] but with explicit `ratio_bp` values (500–9500).
pub fn prismforge_tree_with_ratios(
    vertical_ratio_bp: u16,
    horizontal_ratio_bp: u16,
    horizontal_divider: u16,
) -> WorkspaceNode {
    let h_div = horizontal_divider.max(1);
    WorkspaceNode::Split {
        axis: Axis::Vertical,
        ratio_bp: clamp_splitter_ratio_bp(vertical_ratio_bp),
        divider: 1,
        a: Box::new(WorkspaceNode::Split {
            axis: Axis::Horizontal,
            ratio_bp: clamp_splitter_ratio_bp(horizontal_ratio_bp),
            divider: h_div,
            a: Box::new(WorkspaceNode::Leaf(LeafId(1))),
            b: Box::new(WorkspaceNode::Leaf(LeafId(2))),
        }),
        b: Box::new(WorkspaceNode::Split {
            axis: Axis::Horizontal,
            ratio_bp: 7500,
            divider: h_div,
            a: Box::new(WorkspaceNode::Leaf(LeafId(3))),
            b: Box::new(WorkspaceNode::Leaf(LeafId(4))),
        }),
    }
}

/// Keeps both panes usable when dragging splitters.
pub const SPLITTER_RATIO_MIN: u16 = 500;
pub const SPLITTER_RATIO_MAX: u16 = 9500;

pub fn clamp_splitter_ratio_bp(ratio_bp: u16) -> u16 {
    ratio_bp.clamp(SPLITTER_RATIO_MIN, SPLITTER_RATIO_MAX)
}

/// Widen hit targets: vertical bars get 2 cols; horizontal bars get 2 rows (centered on `div`).
pub fn expand_splitter_hit_rect(div: Rect) -> Rect {
    if div.width == 0 || div.height == 0 {
        return div;
    }
    if div.width >= div.height {
        // Horizontal splitter (tall bar is small)
        if div.height <= 1 && div.width > 2 {
            let h = 2u16;
            let y = div.y.saturating_sub((h.saturating_sub(div.height)) / 2);
            return Rect::new(div.x, y, div.width, h);
        }
    } else {
        // Vertical splitter
        if div.width <= 1 && div.height > 2 {
            let w = 2u16;
            let x = div.x.saturating_sub((w.saturating_sub(div.width)) / 2);
            return Rect::new(x, div.y, w, div.height);
        }
    }
    div
}

/// DFS order matches [`collect_leaves`]: push divider, recurse `a`, recurse `b`.
pub fn set_splitter_ratio_bp(tree: &mut WorkspaceNode, target_index: usize, ratio_bp: u16) -> bool {
    let mut index = 0usize;
    set_splitter_ratio_bp_inner(tree, target_index, clamp_splitter_ratio_bp(ratio_bp), &mut index)
}

fn set_splitter_ratio_bp_inner(
    node: &mut WorkspaceNode,
    target_index: usize,
    ratio_bp: u16,
    index: &mut usize,
) -> bool {
    match node {
        WorkspaceNode::Leaf(_) => false,
        WorkspaceNode::Split {
            axis: _,
            ratio_bp: bp,
            divider,
            a,
            b,
        } => {
            if *divider > 0 {
                if *index == target_index {
                    *bp = ratio_bp;
                    return true;
                }
                *index += 1;
            }
            if set_splitter_ratio_bp_inner(a, target_index, ratio_bp, index) {
                return true;
            }
            set_splitter_ratio_bp_inner(b, target_index, ratio_bp, index)
        }
    }
}

pub fn get_splitter_ratio_bp(tree: &WorkspaceNode, target_index: usize) -> Option<u16> {
    let mut index = 0usize;
    get_splitter_ratio_bp_inner(tree, target_index, &mut index)
}

fn get_splitter_ratio_bp_inner(
    node: &WorkspaceNode,
    target_index: usize,
    index: &mut usize,
) -> Option<u16> {
    match node {
        WorkspaceNode::Leaf(_) => None,
        WorkspaceNode::Split {
            axis: _,
            ratio_bp,
            divider,
            a,
            b,
        } => {
            if *divider > 0 {
                if *index == target_index {
                    return Some(*ratio_bp);
                }
                *index += 1;
            }
            get_splitter_ratio_bp_inner(a, target_index, index)
                .or_else(|| get_splitter_ratio_bp_inner(b, target_index, index))
        }
    }
}

pub fn splitter_axis_at_index(tree: &WorkspaceNode, target_index: usize) -> Option<Axis> {
    let mut index = 0usize;
    splitter_axis_at_index_inner(tree, target_index, &mut index)
}

fn splitter_axis_at_index_inner(
    node: &WorkspaceNode,
    target_index: usize,
    index: &mut usize,
) -> Option<Axis> {
    match node {
        WorkspaceNode::Leaf(_) => None,
        WorkspaceNode::Split {
            axis,
            ratio_bp: _,
            divider,
            a,
            b,
        } => {
            if *divider > 0 {
                if *index == target_index {
                    return Some(*axis);
                }
                *index += 1;
            }
            splitter_axis_at_index_inner(a, target_index, index)
                .or_else(|| splitter_axis_at_index_inner(b, target_index, index))
        }
    }
}

/// `(vertical_ratio_bp, horizontal_ratio_bp)` for a PrismForge-shaped tree; defaults if shape differs.
pub fn prismforge_ratios_from_tree(node: &WorkspaceNode) -> (u16, u16) {
    match node {
        WorkspaceNode::Split {
            axis: Axis::Vertical,
            ratio_bp: v,
            a,
            ..
        } => match a.as_ref() {
            WorkspaceNode::Split {
                axis: Axis::Horizontal,
                ratio_bp: h,
                ..
            } => (*v, *h),
            _ => (*v, 5000),
        },
        _ => {
            let t = prismforge_tree_for_workspace(Rect::new(0, 0, 85, 38), &crate::quorp::tui::theme::Theme::prism_forge().metrics);
            prismforge_ratios_from_tree(&t)
        }
    }
}

/// Parent `Rect` passed into `split_axis` for the `target_index` splitter (same DFS order as [`collect_leaves`]).
pub fn split_parent_rect_for_index(
    workspace: Rect,
    tree: &WorkspaceNode,
    target_index: usize,
) -> Option<(Rect, Axis, u16)> {
    let mut index = 0usize;
    split_parent_rect_for_index_inner(workspace, tree, target_index, &mut index)
}

fn split_parent_rect_for_index_inner(
    rect: Rect,
    node: &WorkspaceNode,
    target_index: usize,
    index: &mut usize,
) -> Option<(Rect, Axis, u16)> {
    match node {
        WorkspaceNode::Leaf(_) => None,
        WorkspaceNode::Split {
            axis,
            ratio_bp,
            divider,
            a,
            b,
        } => {
            let (a_rect, _div, b_rect) = split_axis(rect, *axis, *ratio_bp, *divider);
            if *divider > 0 {
                if *index == target_index {
                    return Some((rect, *axis, *divider));
                }
                *index += 1;
            }
            split_parent_rect_for_index_inner(a_rect, a, target_index, index)
                .or_else(|| split_parent_rect_for_index_inner(b_rect, b, target_index, index))
        }
    }
}

/// Convert mouse column (vertical split) or row (horizontal split) to `ratio_bp` within `parent`.
pub fn ratio_bp_from_drag_position(
    parent: Rect,
    axis: Axis,
    primary_coord: u16,
    divider: u16,
) -> u16 {
    let out = match axis {
        Axis::Vertical => {
            let usable = parent.width.saturating_sub(divider);
            if usable == 0 {
                return clamp_splitter_ratio_bp(5000);
            }
            let rel = primary_coord.saturating_sub(parent.x);
            let max_a = usable.saturating_sub(10).max(1);
            let a_w = rel.min(max_a).max(10);
            ((a_w as u32 * 10000 + usable as u32 / 2) / usable as u32) as u16
        }
        Axis::Horizontal => {
            let usable = parent.height.saturating_sub(divider);
            if usable == 0 {
                return clamp_splitter_ratio_bp(5000);
            }
            let rel = primary_coord.saturating_sub(parent.y);
            let max_a = usable.saturating_sub(3).max(1);
            let a_h = rel.min(max_a).max(3);
            ((a_h as u32 * 10000 + usable as u32 / 2) / usable as u32) as u16
        }
    };
    clamp_splitter_ratio_bp(out)
}

/// PrismForge tree for a **120×40** shell (`workspace` **85×38** with `Theme::prism_forge()` metrics).
pub fn default_prismforge_tree() -> WorkspaceNode {
    prismforge_tree_for_workspace(Rect::new(0, 0, 85, 38), &crate::quorp::tui::theme::Theme::prism_forge().metrics)
}

pub fn compute_shell(full: Rect, metrics: &Metrics) -> ShellRects {
    let title_h = metrics.title_height;
    let status_h: u16 = 1;

    let body_h = full.height.saturating_sub(title_h + status_h);

    let titlebar = Rect::new(full.x, full.y, full.width, title_h);
    let statusbar = Rect::new(full.x, full.y + title_h + body_h, full.width, status_h);

    let body_y = full.y + title_h;
    let activity_w = metrics.activity_bar_width;
    let activity = Rect::new(full.x, body_y, activity_w, body_h);

    let after_activity = full.x + activity_w;
    let explorer_w = metrics.default_explorer_width;
    let explorer_header = Rect::new(after_activity, body_y, explorer_w, metrics.header_height);
    let explorer_body = Rect::new(
        after_activity,
        body_y + metrics.header_height,
        explorer_w,
        body_h.saturating_sub(metrics.header_height),
    );

    let divider_x = after_activity + explorer_w;
    let divider_w: u16 = 1;
    let explorer_divider = Rect::new(divider_x, body_y, divider_w, body_h);

    let workspace_x = divider_x + divider_w;
    let workspace_w = full
        .width
        .saturating_sub(activity_w + explorer_w + divider_w);
    let workspace = Rect::new(workspace_x, body_y, workspace_w, body_h);

    ShellRects {
        titlebar,
        activity,
        explorer_header,
        explorer_body,
        explorer_divider,
        workspace,
        statusbar,
    }
}

fn split_axis(rect: Rect, axis: Axis, ratio_bp: u16, divider: u16) -> (Rect, Rect, Rect) {
    match axis {
        Axis::Vertical => {
            let usable = rect.width.saturating_sub(divider);
            let a_w = ((usable as u32 * ratio_bp as u32) + 5000) / 10000;
            let a_w = a_w as u16;
            let b_w = usable.saturating_sub(a_w);
            let a = Rect::new(rect.x, rect.y, a_w, rect.height);
            let div = Rect::new(rect.x + a_w, rect.y, divider, rect.height);
            let b = Rect::new(rect.x + a_w + divider, rect.y, b_w, rect.height);
            (a, div, b)
        }
        Axis::Horizontal => {
            let usable = rect.height.saturating_sub(divider);
            let a_h = ((usable as u32 * ratio_bp as u32) + 5000) / 10000;
            let a_h = a_h as u16;
            let b_h = usable.saturating_sub(a_h);
            let a = Rect::new(rect.x, rect.y, rect.width, a_h);
            let div = Rect::new(rect.x, rect.y + a_h, rect.width, divider);
            let b = Rect::new(rect.x, rect.y + a_h + divider, rect.width, b_h);
            (a, div, b)
        }
    }
}

pub fn leaf_internal_rects(leaf_rect: Rect, leaf_id: LeafId, metrics: &Metrics) -> LeafRects {
    let tab_h = metrics.header_height;
    let scrollbar_w = metrics.scrollbar_width;

    let tabs = Rect::new(leaf_rect.x, leaf_rect.y, leaf_rect.width, tab_h);

    let content_y = leaf_rect.y + tab_h;
    let content_h = leaf_rect.height.saturating_sub(tab_h);
    let content_w = leaf_rect.width.saturating_sub(scrollbar_w);

    let scrollbar = Rect::new(
        leaf_rect.x + content_w,
        content_y,
        scrollbar_w,
        content_h,
    );

    match leaf_id {
        LeafId(3) => {
            let banner_h = metrics.banner_height;
            let composer_h = metrics.composer_height;
            let body_h = content_h.saturating_sub(banner_h + composer_h);

            let banner = Rect::new(leaf_rect.x, content_y, content_w, banner_h);
            let body = Rect::new(
                leaf_rect.x,
                content_y + banner_h,
                content_w,
                body_h,
            );
            let composer = Rect::new(
                leaf_rect.x,
                content_y + banner_h + body_h,
                content_w,
                composer_h,
            );
            LeafRects {
                tabs,
                body,
                scrollbar,
                banner: Some(banner),
                panel_tabs: None,
                composer: Some(composer),
            }
        }
        LeafId(2) => {
            let panel_h = metrics.header_height;
            let body = Rect::new(leaf_rect.x, content_y, content_w, content_h.saturating_sub(panel_h));
            let panel_tabs = Rect::new(
                leaf_rect.x,
                content_y + content_h.saturating_sub(panel_h),
                content_w,
                panel_h,
            );
            LeafRects {
                tabs: Rect::new(0, 0, 0, 0),
                body,
                scrollbar,
                banner: None,
                panel_tabs: Some(panel_tabs),
                composer: None,
            }
        }
        _ => {
            let body = Rect::new(leaf_rect.x, content_y, content_w, content_h);
            LeafRects {
                tabs,
                body,
                scrollbar,
                banner: None,
                panel_tabs: None,
                composer: None,
            }
        }
    }
}

pub fn compute_workbench(
    workspace: Rect,
    tree: &WorkspaceNode,
    metrics: &Metrics,
) -> WorkbenchLayout {
    let mut layout = WorkbenchLayout {
        leaves: HashMap::new(),
        splitters: Vec::new(),
    };
    collect_leaves(workspace, tree, metrics, &mut layout);
    layout
}

fn collect_leaves(
    rect: Rect,
    node: &WorkspaceNode,
    metrics: &Metrics,
    layout: &mut WorkbenchLayout,
) {
    match node {
        WorkspaceNode::Leaf(id) => {
            let leaf_rects = leaf_internal_rects(rect, *id, metrics);
            layout.leaves.insert(*id, leaf_rects);
        }
        WorkspaceNode::Split {
            axis,
            ratio_bp,
            divider,
            a,
            b,
        } => {
            let (a_rect, div_rect, b_rect) = split_axis(rect, *axis, *ratio_bp, *divider);
            if *divider > 0 {
                layout.splitters.push(div_rect);
            }
            collect_leaves(a_rect, a, metrics, layout);
            collect_leaves(b_rect, b, metrics, layout);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quorp::tui::theme::Theme;

    fn golden_metrics() -> Metrics {
        Theme::core_tui().metrics
    }

    fn golden_shell() -> (Rect, ShellRects) {
        let full = Rect::new(0, 0, 232, 64);
        let shell = compute_shell(full, &golden_metrics());
        (full, shell)
    }

    #[test]
    fn shell_geometry_correct() {
        let (_, shell) = golden_shell();
        assert_eq!(shell.titlebar.height, 2, "titlebar should be 2 rows");
        assert_eq!(shell.statusbar.height, 1, "statusbar should be 1 row");
        assert_eq!(shell.activity.width, 5, "activity rail should be 5 cols");
        assert_eq!(shell.explorer_header.width, 23, "explorer header should be 23 cols");
        assert_eq!(shell.explorer_body.width, 23, "explorer body should be 23 cols");
        assert_eq!(shell.explorer_divider.width, 1, "divider should be 1 col");
        assert_eq!(
            shell.workspace.width,
            232 - 5 - 23 - 1,
            "workspace = full - activity - explorer - divider"
        );
        assert_eq!(shell.workspace.width, 203);
    }

    #[test]
    fn core_tui_left_leaf_72_cols() {
        let (_, shell) = golden_shell();
        let tree = default_core_tui_tree();
        let layout = compute_workbench(shell.workspace, &tree, &golden_metrics());
        let left_editor = layout.leaves.get(&LeafId(1)).expect("leaf 1");
        let left_total_w = left_editor.tabs.width + left_editor.scrollbar.width;
        assert!(
            (71..=73).contains(&left_total_w),
            "left leaf total width should be ~72, got {left_total_w}"
        );
    }

    #[test]
    fn core_tui_right_leaf_130_cols() {
        let (_, shell) = golden_shell();
        let tree = default_core_tui_tree();
        let layout = compute_workbench(shell.workspace, &tree, &golden_metrics());
        let right = layout.leaves.get(&LeafId(3)).expect("leaf 3");
        let right_total_w = right.tabs.width;
        assert!(
            (129..=131).contains(&right_total_w),
            "right leaf should be ~130 cols, got {right_total_w}"
        );
    }

    #[test]
    fn split_no_off_by_one() {
        let rect = Rect::new(10, 5, 203, 61);
        let (a, div, b) = split_axis(rect, Axis::Vertical, 3560, 1);
        assert_eq!(
            a.width + div.width + b.width,
            rect.width,
            "a + div + b should equal total width"
        );
        assert_eq!(a.x, rect.x);
        assert_eq!(div.x, rect.x + a.width);
        assert_eq!(b.x, rect.x + a.width + div.width);
    }

    #[test]
    fn leaf_rects_terminal_has_panel_tabs() {
        let (_, shell) = golden_shell();
        let tree = default_core_tui_tree();
        let layout = compute_workbench(shell.workspace, &tree, &golden_metrics());
        let terminal = layout.leaves.get(&LeafId(2)).expect("leaf 2");
        assert!(terminal.panel_tabs.is_some(), "terminal leaf should have panel_tabs");
        assert!(terminal.composer.is_none(), "terminal leaf should not have composer");
        assert!(terminal.banner.is_none(), "terminal leaf should not have banner");
    }

    #[test]
    fn leaf_rects_agent_has_banner_and_composer() {
        let (_, shell) = golden_shell();
        let tree = default_core_tui_tree();
        let layout = compute_workbench(shell.workspace, &tree, &golden_metrics());
        let agent = layout.leaves.get(&LeafId(3)).expect("leaf 3");
        assert!(agent.banner.is_some(), "agent leaf should have banner");
        assert!(agent.composer.is_some(), "agent leaf should have composer");
        assert!(agent.panel_tabs.is_none(), "agent leaf should not have panel_tabs");
    }

    fn prism_metrics() -> Metrics {
        Theme::prism_forge().metrics
    }

    #[test]
    fn prismforge_shell_at_120x40() {
        let full = Rect::new(0, 0, 120, 40);
        let shell = compute_shell(full, &prism_metrics());
        assert_eq!(shell.titlebar.height, 1);
        assert_eq!(shell.activity.width, 4);
        assert_eq!(shell.explorer_header.width, 30);
        assert_eq!(shell.workspace.width, 120 - 4 - 30 - 1);
        assert_eq!(shell.workspace.height, 40 - 1 - 1);
    }

    #[test]
    fn prismforge_layout_at_120x40() {
        let full = Rect::new(0, 0, 120, 40);
        let m = prism_metrics();
        let shell = compute_shell(full, &m);
        let tree = prismforge_tree_for_workspace(shell.workspace, &m);
        let layout = compute_workbench(shell.workspace, &tree, &m);
        let editor = layout.leaves.get(&LeafId(1)).expect("leaf 1");
        let chat = layout.leaves.get(&LeafId(3)).expect("leaf 3");
        let terminal = layout.leaves.get(&LeafId(2)).expect("leaf 2");
        let editor_w = editor.tabs.width + editor.scrollbar.width;
        let chat_w = chat.tabs.width + chat.scrollbar.width;
        assert!(
            (35..=37).contains(&editor_w),
            "editor stack ~36 cols, got {editor_w}"
        );
        assert!((47..=49).contains(&chat_w), "chat ~48 cols, got {chat_w}");
        let panel_h = terminal.panel_tabs.expect("terminal panel").height;
        let inner_h = terminal.body.height.saturating_add(panel_h);
        assert!(
            (10..=14).contains(&inner_h),
            "terminal inner band ~12 rows, got {inner_h}"
        );
    }

    #[test]
    fn prismforge_default_tree_matches_workspace_85x38() {
        let m = prism_metrics();
        let a = default_prismforge_tree();
        let b = prismforge_tree_for_workspace(Rect::new(0, 0, 85, 38), &m);
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    #[test]
    fn clamp_prismforge_horizontal_triple_smoke() {
        let (ex, ws) = clamp_prismforge_horizontal_triple(120, 4, 30, 1);
        assert_eq!(ex, 30);
        assert_eq!(ws, 85);
    }

    #[test]
    fn prismforge_center_min_shrinks_chat_when_wide_enough_for_rule() {
        assert_eq!(clamp_prismforge_chat_cols(106, 48), 45);
    }

    #[test]
    fn prismforge_clamp_chat_respects_cap_when_narrow() {
        assert_eq!(clamp_prismforge_chat_cols(80, 48), 48);
        assert_eq!(clamp_prismforge_chat_cols(45, 48), 43);
    }
}

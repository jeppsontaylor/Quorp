//! Native macOS menu bar.
//!
//! Items dispatch through Tauri's standard menu-event channel; the
//! frontend receives `MenuEvent { id }` payloads via `tauri://menu`
//! emit and routes them to keymap handlers. We deliberately keep the
//! menu lean for v1 — the keyboard shortcuts in the frontend are the
//! authoritative source for accelerators.

use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Runtime, Wry};

pub fn build_menu(handle: &AppHandle<Wry>) -> tauri::Result<Menu<Wry>> {
    let app_submenu = SubmenuBuilder::new(handle, "Quorp")
        .item(
            &MenuItemBuilder::with_id("about", "About Quorp")
                .build(handle)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::with_id("settings", "Settings…")
                .accelerator("CmdOrCtrl+,")
                .build(handle)?,
        )
        .separator()
        .quit()
        .build()?;

    let file_submenu = SubmenuBuilder::new(handle, "File")
        .item(
            &MenuItemBuilder::with_id("new_session", "New Session")
                .accelerator("CmdOrCtrl+N")
                .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id("add_folder", "Add Folder…")
                .accelerator("CmdOrCtrl+O")
                .build(handle)?,
        )
        .separator()
        .close_window()
        .build()?;

    let view_submenu = SubmenuBuilder::new(handle, "View")
        .item(
            &MenuItemBuilder::with_id("toggle_left", "Toggle Left Panel")
                .accelerator("CmdOrCtrl+B")
                .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id("toggle_right", "Toggle Right Inspector")
                .accelerator("CmdOrCtrl+J")
                .build(handle)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::with_id("toggle_high_contrast", "High-Contrast Mode")
                .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id("toggle_no_color", "No-Color Mode")
                .build(handle)?,
        )
        .build()?;

    let agent_submenu = SubmenuBuilder::new(handle, "Agent")
        .item(
            &MenuItemBuilder::with_id("send", "Send")
                .accelerator("CmdOrCtrl+Return")
                .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id("cancel_run", "Cancel Run")
                .accelerator("CmdOrCtrl+.")
                .build(handle)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::with_id("open_palette", "Open Command Palette")
                .accelerator("CmdOrCtrl+K")
                .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id("replay_last", "Replay Last Run")
                .accelerator("CmdOrCtrl+Shift+R")
                .build(handle)?,
        )
        .build()?;

    let tools_submenu = SubmenuBuilder::new(handle, "Tools")
        .item(
            &MenuItemBuilder::with_id("doctor", "Open Doctor").build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id("benchmarks", "Open Benchmark Library").build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id("reveal_tmp", "Show /tmp/quorp folder…")
                .build(handle)?,
        )
        .build()?;

    let help_submenu = SubmenuBuilder::new(handle, "Help")
        .item(
            &MenuItemBuilder::with_id("docs", "Quorp Documentation")
                .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id("shortcuts", "Keyboard Shortcuts")
                .accelerator("CmdOrCtrl+/")
                .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id("report_issue", "Report an Issue")
                .build(handle)?,
        )
        .build()?;

    MenuBuilder::new(handle)
        .item(&app_submenu)
        .item(&file_submenu)
        .item(&view_submenu)
        .item(&agent_submenu)
        .item(&tools_submenu)
        .item(&help_submenu)
        .build()
}

/// Forward menu clicks to the frontend as a `menu://<id>` event so a
/// single React handler can route them to the matching keymap action.
pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: tauri::menu::MenuEvent) {
    let id = event.id().0.clone();
    let _ = app.emit("menu://event", id);
}

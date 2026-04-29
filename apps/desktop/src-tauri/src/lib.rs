//! Tauri 2 shell that hosts the Quorp desktop app.
//!
//! This crate is intentionally thin: every privileged operation lives
//! in `quorp_desktop_core` and is exposed here as a typed
//! `#[tauri::command]` wrapper. The capabilities allowlist
//! (`capabilities/default.json`) grants only `core:default`,
//! `dialog:default`, `opener:default`, `store:default`, and
//! `window-state:default`. We deliberately do not enable broad
//! `shell:*` or `fs:*` plugins — the frontend cannot run shell
//! commands or touch the filesystem outside the typed `quorp:*`
//! commands defined below.

pub mod commands;
pub mod menu;
pub mod state;

use crate::state::AppHandleState;

/// Entry point invoked from `main.rs`.
///
/// Builds a Tauri runtime, registers the desktop core's app state,
/// and wires every command in [`commands`] before launching the main
/// window.
pub fn run() {
    let _ = env_logger::Builder::from_default_env()
        .filter_module("quorp_desktop_app_lib", log::LevelFilter::Info)
        .filter_module("quorp_desktop_core", log::LevelFilter::Info)
        .try_init();

    let app_state = AppHandleState::new()
        .expect("failed to construct DesktopAppState (tokio runtime build failed)");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(app_state)
        .menu(menu::build_menu)
        .on_menu_event(menu::on_menu_event)
        .invoke_handler(tauri::generate_handler![
            // workspace
            commands::workspace::add_workspace,
            commands::workspace::list_workspaces,
            commands::workspace::trust_workspace,
            commands::workspace::remove_workspace,
            commands::workspace::open_terminal_at,
            // run
            commands::run::start_agent_run,
            commands::run::cancel_run,
            commands::run::get_run_status,
            commands::run::list_active_runs,
            // permission
            commands::permission::respond_to_permission,
            commands::permission::pending_permission_count,
            commands::permission::cancel_all_permissions,
            // artifacts
            commands::artifact::read_artifact,
            commands::artifact::list_run_artifacts,
            commands::artifact::read_event_window,
            commands::artifact::reveal_path,
            // benchmark
            commands::benchmark::list_benchmark_fixtures,
            commands::benchmark::start_benchmark_run,
            // replay
            commands::replay::replay_run,
            // checkpoint / verification / diff-apply
            commands::checkpoint::apply_run_diff,
            commands::checkpoint::verify_run_again,
            commands::checkpoint::rollback_to_checkpoint,
            // provider
            commands::provider::provider_info,
            commands::provider::set_nim_api_key,
            commands::provider::clear_nim_api_key,
            commands::provider::validate_nim_provider,
            // doctor
            commands::doctor::app_status,
            commands::doctor::doctor_report,
            // wire-version handshake
            commands::doctor::wire_version,
            // expansive (PR10)
            commands::expansive::check_for_updates,
            commands::expansive::apply_update,
            commands::expansive::new_window,
            commands::expansive::query_memory,
            commands::expansive::prune_memory,
            commands::expansive::list_rules,
            commands::expansive::update_rule_lifecycle,
            commands::expansive::list_agents_in_run,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

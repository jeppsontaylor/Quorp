//! Playwright-style user-flow tests: `TestBackend` + scripted `TuiApp::handle_event`.
//! See `docs/src/development/quorp-tui-testing.md`.

mod fixtures;
mod harness;
mod scenario;

mod backend_flows;
mod chat_flows;
mod chat_http_mock;
mod editor_pane_flows;
mod file_tree_flows;
mod full_auto_flows;
mod global_shortcuts;
mod mention_flows;
mod models_picker_flows;
mod mouse_flows;
mod navigation_flows;
mod proof_rail_flows;
mod rollback_flows;
mod rust_capture_flows;
mod screenshot_suite;
mod session_isolation_flows;
mod tab_strip_flows;
mod terminal_certification_flows;
mod terminal_flows;
mod vim_navigation_matrix;
mod visual_flows;

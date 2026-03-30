//! Playwright-style user-flow tests: `TestBackend` + scripted `TuiApp::handle_event`.
//! See `docs/src/development/quorp-tui-testing.md`.

mod fixtures;
mod harness;

mod backend_flows;
mod chat_http_mock;
mod chat_flows;
mod mention_flows;
mod editor_pane_flows;
mod file_tree_flows;
mod global_shortcuts;
mod models_picker_flows;
mod mouse_flows;
mod navigation_flows;
mod tab_strip_flows;
mod terminal_flows;
mod visual_flows;
mod session_isolation_flows;
mod screenshot_suite;
mod vim_navigation_matrix;
mod visual_regression;
mod rust_capture_flows;

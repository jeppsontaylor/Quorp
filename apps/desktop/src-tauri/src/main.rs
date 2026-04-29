// Prevents an additional console window from opening on Windows in
// release builds. Has no effect on macOS (our primary target).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    quorp_desktop_app_lib::run();
}

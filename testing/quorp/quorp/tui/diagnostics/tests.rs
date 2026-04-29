use super::*;

#[test]
fn log_event_flushes_via_background_writer() {
    clear_events_for_test();
    log_event("diagnostics.test_flush", json!({ "ok": true }));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if take_events_for_test().iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("diagnostics.test_flush")
        }) {
            return;
        }
        if let Ok(content) = std::fs::read_to_string(diagnostics_log_file())
            && content.contains("diagnostics.test_flush")
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("background diagnostics writer did not flush in time");
}

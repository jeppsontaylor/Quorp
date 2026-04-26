use serde_json::{Map, Value, json};
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static APP_RUN_ID: OnceLock<String> = OnceLock::new();
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);
#[cfg(not(test))]
static LOG_WRITER: OnceLock<std::sync::mpsc::Sender<String>> = OnceLock::new();
#[cfg(test)]
static TEST_LOG_EVENTS: OnceLock<Mutex<Vec<Value>>> = OnceLock::new();
#[cfg(test)]
static TEST_FLUSH_COUNT: AtomicU64 = AtomicU64::new(0);

pub fn app_run_id() -> &'static str {
    APP_RUN_ID.get_or_init(|| format!("{}-{}", std::process::id(), timestamp_ms()))
}

pub fn next_request_id() -> u64 {
    REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed)
}

pub fn log_event(event: &str, fields: Value) {
    let mut record = Map::new();
    record.insert("ts_ms".to_string(), json!(timestamp_ms()));
    record.insert("app_run_id".to_string(), json!(app_run_id()));
    record.insert("event".to_string(), json!(event));
    match fields {
        Value::Object(object) => {
            for (key, value) in object {
                record.insert(key, value);
            }
        }
        other => {
            record.insert("detail".to_string(), other);
        }
    }
    let line = Value::Object(record).to_string();
    log::info!("tui-diagnostics: {line}");
    #[cfg(test)]
    {
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            let sink = TEST_LOG_EVENTS.get_or_init(|| Mutex::new(Vec::new()));
            if let Ok(mut entries) = sink.lock() {
                entries.push(value);
            }
        }
        let log_path = diagnostics_log_file();
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
            let _ = writeln!(file, "{line}");
            TEST_FLUSH_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[cfg(not(test))]
    {
        let log_path = diagnostics_log_file();
        if let Ok(()) = writer_sender().send(format!("{line}\n")) {
            return;
        }
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
            let _ = writeln!(file, "{line}");
        }
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub fn take_events_for_test() -> Vec<Value> {
    let sink = TEST_LOG_EVENTS.get_or_init(|| Mutex::new(Vec::new()));
    sink.lock()
        .map(|mut entries| std::mem::take(&mut *entries))
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(dead_code)]
pub fn clear_events_for_test() {
    let sink = TEST_LOG_EVENTS.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut entries) = sink.lock() {
        entries.clear();
    }
}

pub fn diagnostics_log_file() -> PathBuf {
    static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();
    LOG_FILE
        .get_or_init(|| {
            ::paths::log_file()
                .parent()
                .unwrap_or_else(|| ::paths::log_file().as_path())
                .join("QuorpTuiDiagnostics.log")
        })
        .clone()
}

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

#[cfg(not(test))]
fn writer_sender() -> &'static std::sync::mpsc::Sender<String> {
    LOG_WRITER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let log_path = diagnostics_log_file();
            if let Some(parent) = log_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            while let Ok(line) = rx.recv() {
                if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
                    let _ = file.write_all(line.as_bytes());
                    #[cfg(test)]
                    TEST_FLUSH_COUNT.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
        tx
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_event_flushes_via_background_writer() {
        log_event("diagnostics.test_flush", json!({ "ok": true }));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if let Ok(content) = std::fs::read_to_string(diagnostics_log_file())
                && content.contains("diagnostics.test_flush")
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("background diagnostics writer did not flush in time");
    }
}

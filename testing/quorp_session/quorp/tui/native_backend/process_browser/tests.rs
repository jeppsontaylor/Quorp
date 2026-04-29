use super::*;
use crate::quorp::tui::ChatUiEvent;
use crate::quorp::tui::native_backend::native_backend_test_guard;
use std::sync::{Mutex, OnceLock};
use tempfile::tempdir;

static TEST_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(unix)]
fn write_script(path: &Path, content: &str) {
    std::fs::write(path, content).expect("write script");
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).expect("meta").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("chmod");
}

fn collect_events(event_rx: std::sync::mpsc::Receiver<TuiEvent>) -> Vec<TuiEvent> {
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match event_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => {
                let finished = matches!(event, TuiEvent::Chat(ChatUiEvent::CommandFinished(_, _)));
                events.push(event);
                if finished {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }
    events
}

#[cfg(unix)]
#[test]
fn process_session_supports_write_read_and_stop() {
    let _guard = native_backend_test_guard();
    let root = tempdir().expect("root");
    let script = root.path().join("process.sh");
    let transcript_file = root.path().join("transcript.txt");
    #[cfg(unix)]
    write_script(
        &script,
        r#"#!/bin/sh
echo ready
transcript_file="$1"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$transcript_file"
done
"#,
    );
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(8);
    spawn_process_start_task(
        event_tx.clone(),
        1,
        ProcessStartSpec {
            cwd: root.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            command: script.display().to_string(),
            args: vec![transcript_file.display().to_string()],
            requested_cwd: None,
        },
        None,
    );
    let events = collect_events(event_rx);
    let start_output = events
        .iter()
        .find_map(|event| match event {
            TuiEvent::Chat(ChatUiEvent::CommandFinished(
                _,
                ActionOutcome::Success { output, .. },
            )) => Some(output.clone()),
            _ => None,
        })
        .expect("start output");
    let process_id = start_output
        .lines()
        .find_map(|line| line.strip_prefix("process_id: "))
        .expect("process id")
        .to_string();

    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(8);
    spawn_process_write_task(
        event_tx.clone(),
        2,
        process_id.clone(),
        "hello\n".to_string(),
        None,
    );
    let _ = collect_events(event_rx);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if std::fs::read_to_string(&transcript_file)
            .unwrap_or_default()
            .contains("hello")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        std::fs::read_to_string(&transcript_file)
            .unwrap_or_default()
            .contains("hello")
    );

    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(8);
    spawn_process_read_task(event_tx.clone(), 3, process_id.clone(), 20, None);
    let events = collect_events(event_rx);
    let read_output = events
        .iter()
        .find_map(|event| match event {
            TuiEvent::Chat(ChatUiEvent::CommandFinished(
                _,
                ActionOutcome::Success { output, .. },
            )) => Some(output.clone()),
            _ => None,
        })
        .expect("read output");
    assert!(read_output.contains(&format!("process_id: {process_id}")));
    assert!(read_output.contains("running"));

    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(8);
    spawn_process_stop_task(event_tx, 4, process_id, None);
    let _ = collect_events(event_rx);
}

#[cfg(unix)]
#[test]
fn process_start_scrubs_secret_environment_values() {
    let _guard = native_backend_test_guard();
    let guard = TEST_ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("lock");
    let previous_secret = std::env::var_os("QUORP_TEST_SECRET");
    let previous_safe = std::env::var_os("QUORP_TEST_SAFE");
    unsafe {
        std::env::set_var("QUORP_TEST_SECRET", "top-secret");
        std::env::set_var("QUORP_TEST_SAFE", "visible");
    }

    let root = tempdir().expect("root");
    let script = root.path().join("process.sh");
    #[cfg(unix)]
    write_script(
        &script,
        r#"#!/bin/sh
echo "${QUORP_TEST_SECRET:-}|${QUORP_TEST_SAFE:-}"
"#,
    );
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(8);
    spawn_process_start_task(
        event_tx,
        1,
        ProcessStartSpec {
            cwd: root.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            command: script.display().to_string(),
            args: Vec::new(),
            requested_cwd: None,
        },
        None,
    );
    let events = collect_events(event_rx);
    let start_output = events
        .iter()
        .find_map(|event| match event {
            TuiEvent::Chat(ChatUiEvent::CommandFinished(
                _,
                ActionOutcome::Success { output, .. },
            )) => Some(output.clone()),
            _ => None,
        })
        .expect("start output");

    unsafe {
        match previous_secret {
            Some(value) => std::env::set_var("QUORP_TEST_SECRET", value),
            None => std::env::remove_var("QUORP_TEST_SECRET"),
        }
        match previous_safe {
            Some(value) => std::env::set_var("QUORP_TEST_SAFE", value),
            None => std::env::remove_var("QUORP_TEST_SAFE"),
        }
    }
    drop(guard);

    assert!(!start_output.contains("top-secret"));
}

#[cfg(unix)]
#[test]
fn process_stop_is_safe_after_process_exit() {
    let _guard = native_backend_test_guard();
    let root = tempdir().expect("root");
    let script = root.path().join("process.sh");
    #[cfg(unix)]
    write_script(
        &script,
        r#"#!/bin/sh
exit 0
"#,
    );
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(8);
    spawn_process_start_task(
        event_tx,
        1,
        ProcessStartSpec {
            cwd: root.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            command: script.display().to_string(),
            args: Vec::new(),
            requested_cwd: None,
        },
        None,
    );
    let events = collect_events(event_rx);
    let start_output = events
        .iter()
        .find_map(|event| match event {
            TuiEvent::Chat(ChatUiEvent::CommandFinished(
                _,
                ActionOutcome::Success { output, .. },
            )) => Some(output.clone()),
            _ => None,
        })
        .expect("start output");
    let process_id = start_output
        .lines()
        .find_map(|line| line.strip_prefix("process_id: "))
        .expect("process id")
        .to_string();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(8);
        spawn_process_read_task(event_tx, 2, process_id.clone(), 20, None);
        let events = collect_events(event_rx);
        let read_output = events.iter().find_map(|event| match event {
            TuiEvent::Chat(ChatUiEvent::CommandFinished(
                _,
                ActionOutcome::Success { output, .. },
            )) => Some(output.clone()),
            _ => None,
        });
        if read_output
            .as_deref()
            .is_some_and(|output| output.contains("exited(0)"))
        {
            break;
        }
        assert!(Instant::now() < deadline, "process did not exit in time");
        std::thread::sleep(Duration::from_millis(50));
    }
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(8);
    spawn_process_stop_task(event_tx, 2, process_id, None);
    let events = collect_events(event_rx);
    let stop_output = events
        .iter()
        .find_map(|event| match event {
            TuiEvent::Chat(ChatUiEvent::CommandFinished(
                _,
                ActionOutcome::Success { output, .. },
            )) => Some(output.clone()),
            TuiEvent::Chat(ChatUiEvent::CommandFinished(
                _,
                ActionOutcome::Failure { error, .. },
            )) => Some(error.clone()),
            _ => None,
        })
        .expect("stop output");
    assert!(stop_output.contains("exit_status: 0"));
}

#[cfg(unix)]
#[test]
fn browser_driver_protocol_parses_mock_responses() {
    let root = tempdir().expect("root");
    let script = root.path().join("browser.sh");
    #[cfg(unix)]
    write_script(
        &script,
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":[ ]*\([0-9][0-9]*\).*/\1/p')
  action=$(printf '%s' "$line" | sed -n 's/.*"action":"\([^"]*\)".*/\1/p')
  browser_id=$(printf '%s' "$line" | sed -n 's/.*"browser_id":"\([^"]*\)".*/\1/p')
  output_dir=$(printf '%s' "$line" | sed -n 's/.*"output_dir":"\([^"]*\)".*/\1/p')
  case "$action" in
    open)
      printf '{"id":%s,"ok":true,"result":{"browser_id":"%s","url":"https://example.com"}}\n' "$id" "$browser_id"
      ;;
    screenshot)
      mkdir -p "$output_dir"
      path="$output_dir/screenshot.png"
      printf 'shot' > "$path"
      printf '{"id":%s,"ok":true,"result":{"browser_id":"%s","screenshot_path":"%s","bytes":4}}\n' "$id" "$browser_id" "$path"
      ;;
    console_logs)
      printf '{"id":%s,"ok":true,"result":{"browser_id":"%s","logs":[{"type":"log","text":"hello"}]}}\n' "$id" "$browser_id"
      ;;
    network_errors)
      printf '{"id":%s,"ok":true,"result":{"browser_id":"%s","errors":[]}}\n' "$id" "$browser_id"
      ;;
    accessibility_snapshot)
      printf '{"id":%s,"ok":true,"result":{"browser_id":"%s","snapshot":{"role":"WebArea"}}}\n' "$id" "$browser_id"
      ;;
    close)
      printf '{"id":%s,"ok":true,"result":{"browser_id":"%s","closed":true}}\n' "$id" "$browser_id"
      exit 0
      ;;
    *)
      printf '{"id":%s,"ok":false,"error":"unknown action"}\n' "$id"
      ;;
  esac
done
"#,
    );
    let command = BrowserToolSettings {
        enabled: true,
        command: script.display().to_string(),
        args: Vec::new(),
        max_runtime_seconds: Some(30),
        max_output_bytes: Some(16 * 1024),
        url_policy: crate::quorp::tui::agent_context::BrowserUrlPolicy::AllowRemote,
    };
    let mut child = spawn_browser_driver(&command).expect("spawn");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    let stdin = child.stdin.take().expect("stdin");
    let stderr_log = Arc::new(Mutex::new(String::new()));
    spawn_browser_stderr_reader(stderr, Arc::clone(&stderr_log));
    let mut browser = ManagedBrowser {
        id: "browser-1".to_string(),
        child,
        stdin,
        stdout: BufReader::new(stdout),
        stderr_log,
        next_request_id: 1,
        tempdir: tempfile::tempdir().expect("tempdir"),
    };
    let output_dir = browser.tempdir.path().display().to_string();
    let open = browser
        .request(json!({
            "action": "open",
            "browser_id": "browser-1",
            "url": "https://example.com",
            "headless": true,
            "output_dir": output_dir,
        }))
        .expect("open");
    assert_eq!(
        open.result
            .as_ref()
            .and_then(|value| value.get("browser_id"))
            .and_then(serde_json::Value::as_str),
        Some("browser-1")
    );
    let screenshot_output_dir = browser.tempdir.path().display().to_string();
    let screenshot = browser
        .request(json!({
            "action": "screenshot",
            "browser_id": "browser-1",
            "output_dir": screenshot_output_dir,
        }))
        .expect("screenshot");
    assert!(
        screenshot
            .result
            .as_ref()
            .and_then(|value| value.get("screenshot_path"))
            .and_then(serde_json::Value::as_str)
            .is_some()
    );
    let close = browser
        .request(json!({
            "action": "close",
            "browser_id": "browser-1",
        }))
        .expect("close");
    assert_eq!(
        close
            .result
            .as_ref()
            .and_then(|value| value.get("closed"))
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
}

#[test]
fn browser_disabled_without_configuration() {
    let root = tempdir().expect("root");
    let error = start_managed_browser(&BrowserOpenSpec {
        project_root: root.path().to_path_buf(),
        url: "file:///tmp/example.html".to_string(),
        headless: true,
        width: None,
        height: None,
    })
    .expect_err("browser should be disabled by default");
    assert!(error.to_string().contains("disabled"));
}

#[test]
fn browser_local_only_policy_rejects_remote_urls() {
    let error = BrowserToolSettings {
        enabled: true,
        command: "node".to_string(),
        args: Vec::new(),
        max_runtime_seconds: Some(30),
        max_output_bytes: Some(16 * 1024),
        url_policy: crate::quorp::tui::agent_context::BrowserUrlPolicy::LocalOnly,
    }
    .url_policy
    .allows_url("https://example.com")
    .expect_err("remote URL should be rejected");
    assert!(error.to_string().contains("local-only"));
}

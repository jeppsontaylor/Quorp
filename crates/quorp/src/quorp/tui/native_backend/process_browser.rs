use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::agent_context::{BrowserToolSettings, load_agent_config};
use crate::quorp::tui::agent_protocol::{ActionOutcome, AgentAction};
use quorp_sandbox::{
    SandboxCommandSpec, build_command_plan, default_policy, sandbox_runtime_for_path,
};
use quorp_tools::patch::sanitize_project_path;

const PROCESS_OUTPUT_LINE_LIMIT: usize = 400;

static NEXT_MANAGED_ID: AtomicU64 = AtomicU64::new(1);
static PROCESS_REGISTRY: OnceLock<Mutex<HashMap<String, Arc<Mutex<ManagedProcess>>>>> =
    OnceLock::new();
static BROWSER_REGISTRY: OnceLock<Mutex<HashMap<String, Arc<Mutex<ManagedBrowser>>>>> =
    OnceLock::new();

#[derive(Debug, Clone)]
pub(crate) struct ProcessStartSpec {
    pub(crate) cwd: PathBuf,
    pub(crate) project_root: PathBuf,
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) requested_cwd: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct BrowserOpenSpec {
    pub(crate) project_root: PathBuf,
    pub(crate) url: String,
    pub(crate) headless: bool,
    pub(crate) width: Option<u32>,
    pub(crate) height: Option<u32>,
}

fn next_managed_id(prefix: &str) -> String {
    let id = NEXT_MANAGED_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{id}")
}

fn process_registry() -> &'static Mutex<HashMap<String, Arc<Mutex<ManagedProcess>>>> {
    PROCESS_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn browser_registry() -> &'static Mutex<HashMap<String, Arc<Mutex<ManagedBrowser>>>> {
    BROWSER_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug)]
struct ManagedProcess {
    id: String,
    command: String,
    cwd: PathBuf,
    child: Child,
    stdin: Option<ChildStdin>,
    output_lines: VecDeque<String>,
    exit_status: Option<i32>,
}

impl ManagedProcess {
    fn push_output(&mut self, line: impl Into<String>) {
        if self.output_lines.len() >= PROCESS_OUTPUT_LINE_LIMIT {
            self.output_lines.pop_front();
        }
        self.output_lines.push_back(line.into());
    }

    fn tail_output(&self, tail_lines: usize) -> String {
        if self.output_lines.is_empty() {
            return "[no output yet]".to_string();
        }
        self.output_lines
            .iter()
            .rev()
            .take(tail_lines.max(1))
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug)]
struct ManagedBrowser {
    id: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    stderr_log: Arc<Mutex<String>>,
    next_request_id: u64,
    tempdir: tempfile::TempDir,
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        if let Err(error) = terminate_child_process(&mut self.child, &self.id, "managed process") {
            log::warn!("failed to clean up managed process {}: {error:#}", self.id);
            return;
        }
        if let Err(error) = self.child.wait() {
            log::warn!(
                "failed to wait on managed process {} during drop: {error}",
                self.id
            );
        }
    }
}

impl Drop for ManagedBrowser {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        if let Err(error) = terminate_child_process(&mut self.child, &self.id, "browser driver") {
            log::warn!("failed to clean up managed browser {}: {error:#}", self.id);
            return;
        }
        if let Err(error) = self.child.wait() {
            log::warn!(
                "failed to wait on managed browser {} during drop: {error}",
                self.id
            );
        }
    }
}

#[derive(Debug, Deserialize)]
struct BrowserResponse {
    ok: bool,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
}

pub(crate) fn spawn_process_start_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    request: ProcessStartSpec,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    thread::spawn(move || {
        let action = AgentAction::ProcessStart {
            command: request.command.clone(),
            args: request.args.clone(),
            cwd: request.requested_cwd.clone(),
        };
        let result = start_managed_process(
            &request.project_root,
            &request.cwd,
            &request.command,
            &request.args,
            request.requested_cwd.as_deref(),
        )
        .and_then(|(process, stdout, stderr)| {
            let process_id = process.id.clone();
            let pid = process.child.id();
            let process_handle = Arc::new(Mutex::new(process));
            process_registry()
                .lock()
                .map_err(|_| anyhow::anyhow!("managed process registry is poisoned"))?
                .insert(process_id.clone(), Arc::clone(&process_handle));
            spawn_process_output_reader(Arc::clone(&process_handle), stdout, "stdout");
            spawn_process_output_reader(Arc::clone(&process_handle), stderr, "stderr");
            spawn_process_reaper(Arc::clone(&process_handle));
            Ok(format!(
                "[process_start]\nprocess_id: {process_id}\npid: {pid}\ncommand: {}\ncwd: {}",
                request.command.as_str(),
                request
                    .requested_cwd
                    .as_deref()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| request.project_root.display().to_string())
            ))
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "process_start",
            responder,
        );
    });
}

pub(crate) fn spawn_process_read_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    process_id: String,
    tail_lines: usize,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    thread::spawn(move || {
        let action = AgentAction::ProcessRead {
            process_id: process_id.clone(),
            tail_lines,
        };
        let result = with_process(&process_id, |process| {
            let status_label = process
                .exit_status
                .map(|code| format!("exited({code})"))
                .unwrap_or_else(|| "running".to_string());
            Ok(format!(
                "[process_read]\nprocess_id: {process_id}\nstatus: {status_label}\ncommand: {}\ncwd: {}\n{}",
                process.command,
                process.cwd.display(),
                process.tail_output(tail_lines)
            ))
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "process_read",
            responder,
        );
    });
}

pub(crate) fn spawn_process_write_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    process_id: String,
    stdin: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    thread::spawn(move || {
        let action = AgentAction::ProcessWrite {
            process_id: process_id.clone(),
            stdin: stdin.clone(),
        };
        let result = with_process_mut(&process_id, |process| {
            if let Some(stdin_handle) = process.stdin.as_mut() {
                stdin_handle
                    .write_all(stdin.as_bytes())
                    .map_err(|error| anyhow::anyhow!("failed to write stdin: {error}"))?;
                stdin_handle
                    .flush()
                    .map_err(|error| anyhow::anyhow!("failed to flush stdin: {error}"))?;
                Ok(format!(
                    "[process_write]\nprocess_id: {process_id}\nbytes_written: {}",
                    stdin.len()
                ))
            } else {
                Err(anyhow::anyhow!("process stdin is no longer available"))
            }
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "process_write",
            responder,
        );
    });
}

pub(crate) fn spawn_process_stop_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    process_id: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    thread::spawn(move || {
        let action = AgentAction::ProcessStop {
            process_id: process_id.clone(),
        };
        let result = stop_managed_process(&process_id);
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "process_stop",
            responder,
        );
    });
}

pub(crate) fn spawn_process_wait_for_port_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    process_id: String,
    host: String,
    port: u16,
    timeout_ms: u64,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    thread::spawn(move || {
        let action = AgentAction::ProcessWaitForPort {
            process_id: process_id.clone(),
            host: host.clone(),
            port,
            timeout_ms,
        };
        let result = wait_for_port(&process_id, &host, port, timeout_ms);
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result.map(|message| format!("[process_wait_for_port]\n{message}")),
            "process_wait_for_port",
            responder,
        );
    });
}

pub(crate) fn spawn_browser_open_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    request: BrowserOpenSpec,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    thread::spawn(move || {
        let action = AgentAction::BrowserOpen {
            url: request.url.clone(),
            headless: request.headless,
            width: request.width,
            height: request.height,
        };
        let result = start_managed_browser(&request).and_then(|browser| {
            let browser_id = browser.id.clone();
            let browser_handle = Arc::new(Mutex::new(browser));
            browser_registry()
                .lock()
                .map_err(|_| anyhow::anyhow!("managed browser registry is poisoned"))?
                .insert(browser_id.clone(), Arc::clone(&browser_handle));
            Ok(format!(
                "[browser_open]\nbrowser_id: {browser_id}\nurl: {}\nheadless: {}\nviewport: {:?}x{:?}",
                request.url.as_str(),
                request.headless,
                request.width,
                request.height
            ))
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "browser_open",
            responder,
        );
    });
}

pub(crate) fn spawn_browser_screenshot_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    browser_id: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    thread::spawn(move || {
        let action = AgentAction::BrowserScreenshot {
            browser_id: browser_id.clone(),
        };
        let result = with_browser_mut(&browser_id, |browser| {
            let request_id = browser.next_request_id;
            let browser_id = browser_id.clone();
            let output_dir = browser.tempdir.path().display().to_string();
            browser.request(json!({
                "id": request_id,
                "action": "screenshot",
                "browser_id": browser_id,
                "output_dir": output_dir,
            }))
        })
        .and_then(|response| render_browser_response(&browser_id, "browser_screenshot", response));
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "browser_screenshot",
            responder,
        );
    });
}

pub(crate) fn spawn_browser_console_logs_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    browser_id: String,
    limit: usize,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    thread::spawn(move || {
        let action = AgentAction::BrowserConsoleLogs {
            browser_id: browser_id.clone(),
            limit,
        };
        let result = with_browser_mut(&browser_id, |browser| {
            let request_id = browser.next_request_id;
            let browser_id = browser_id.clone();
            browser.request(json!({
                "id": request_id,
                "action": "console_logs",
                "browser_id": browser_id,
                "limit": limit,
            }))
        })
        .and_then(|response| {
            render_browser_response(&browser_id, "browser_console_logs", response)
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "browser_console_logs",
            responder,
        );
    });
}

pub(crate) fn spawn_browser_network_errors_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    browser_id: String,
    limit: usize,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    thread::spawn(move || {
        let action = AgentAction::BrowserNetworkErrors {
            browser_id: browser_id.clone(),
            limit,
        };
        let result = with_browser_mut(&browser_id, |browser| {
            let request_id = browser.next_request_id;
            let browser_id = browser_id.clone();
            browser.request(json!({
                "id": request_id,
                "action": "network_errors",
                "browser_id": browser_id,
                "limit": limit,
            }))
        })
        .and_then(|response| {
            render_browser_response(&browser_id, "browser_network_errors", response)
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "browser_network_errors",
            responder,
        );
    });
}

pub(crate) fn spawn_browser_accessibility_snapshot_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    browser_id: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    thread::spawn(move || {
        let action = AgentAction::BrowserAccessibilitySnapshot {
            browser_id: browser_id.clone(),
        };
        let result = with_browser_mut(&browser_id, |browser| {
            let request_id = browser.next_request_id;
            let browser_id = browser_id.clone();
            browser.request(json!({
                "id": request_id,
                "action": "accessibility_snapshot",
                "browser_id": browser_id,
            }))
        })
        .and_then(|response| {
            render_browser_response(&browser_id, "browser_accessibility_snapshot", response)
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "browser_accessibility_snapshot",
            responder,
        );
    });
}

pub(crate) fn spawn_browser_close_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    browser_id: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    thread::spawn(move || {
        let action = AgentAction::BrowserClose {
            browser_id: browser_id.clone(),
        };
        let result =
            close_managed_browser(&browser_id).map(|message| format!("[browser_close]\n{message}"));
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "browser_close",
            responder,
        );
    });
}

#[allow(clippy::disallowed_methods)]
fn start_managed_process(
    project_root: &Path,
    cwd: &Path,
    command: &str,
    args: &[String],
    requested_cwd: Option<&str>,
) -> anyhow::Result<(
    ManagedProcess,
    std::process::ChildStdout,
    std::process::ChildStderr,
)> {
    let resolved_cwd = requested_cwd
        .map(|requested| sanitize_project_path(project_root, cwd, requested))
        .transpose()?
        .unwrap_or_else(|| project_root.to_path_buf());
    let runtime = sandbox_runtime_for_path(project_root)?;
    let policy = default_policy();
    let arg_refs = args.iter().map(OsStr::new).collect::<Vec<_>>();
    let command_plan = build_command_plan(SandboxCommandSpec {
        program: OsStr::new(command),
        args: &arg_refs,
        current_dir: &resolved_cwd,
        runtime: &runtime,
        policy: &policy,
        extra_environment: &[],
        additional_mounts: &[],
        interactive: false,
    })?;
    let mut child = Command::new(&command_plan.program);
    command_plan.apply_to_command(&mut child);
    child
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    util::set_pre_exec_to_start_new_session(&mut child);
    let mut child = child
        .spawn()
        .map_err(|error| anyhow::anyhow!("failed to start managed process `{command}`: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("managed process stdout was not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("managed process stderr was not piped"))?;
    let stdin = child.stdin.take();
    let process_id = next_managed_id("process");
    let process = ManagedProcess {
        id: process_id,
        command: command.to_string(),
        cwd: resolved_cwd,
        child,
        stdin,
        output_lines: VecDeque::new(),
        exit_status: None,
    };
    Ok((process, stdout, stderr))
}

fn spawn_process_output_reader<R: std::io::Read + Send + 'static>(
    process_handle: Arc<Mutex<ManagedProcess>>,
    stream: R,
    label: &'static str,
) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            let read = reader.read_line(&mut line).unwrap_or(0);
            if read == 0 {
                break;
            }
            let text = line.trim_end_matches(['\n', '\r']).to_string();
            if let Ok(mut process) = process_handle.lock() {
                process.push_output(format!("[{label}] {text}"));
            } else {
                break;
            }
        }
    });
}

fn spawn_process_reaper(process_handle: Arc<Mutex<ManagedProcess>>) {
    thread::spawn(move || {
        loop {
            let should_stop = {
                let Ok(mut process) = process_handle.lock() else {
                    return;
                };
                match process.child.try_wait() {
                    Ok(Some(status)) => {
                        process.exit_status = Some(status.code().unwrap_or(-1));
                        true
                    }
                    Ok(None) => false,
                    Err(error) => {
                        process.push_output(format!("[stderr] process wait error: {error}"));
                        true
                    }
                }
            };
            if should_stop {
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
    });
}

fn with_process<T>(
    process_id: &str,
    operation: impl FnOnce(&ManagedProcess) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let process_handle = lookup_process(process_id)?;
    let process = process_handle
        .lock()
        .map_err(|_| anyhow::anyhow!("managed process `{process_id}` was poisoned"))?;
    operation(&process)
}

fn with_process_mut<T>(
    process_id: &str,
    operation: impl FnOnce(&mut ManagedProcess) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let process_handle = lookup_process(process_id)?;
    let mut process = process_handle
        .lock()
        .map_err(|_| anyhow::anyhow!("managed process `{process_id}` was poisoned"))?;
    operation(&mut process)
}

fn lookup_process(process_id: &str) -> anyhow::Result<Arc<Mutex<ManagedProcess>>> {
    process_registry()
        .lock()
        .map_err(|_| anyhow::anyhow!("managed process registry is poisoned"))?
        .get(process_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("managed process `{process_id}` not found"))
}

fn stop_managed_process(process_id: &str) -> anyhow::Result<String> {
    let process_handle = {
        let mut registry = process_registry()
            .lock()
            .map_err(|_| anyhow::anyhow!("managed process registry is poisoned"))?;
        registry
            .remove(process_id)
            .ok_or_else(|| anyhow::anyhow!("managed process `{process_id}` not found"))?
    };
    let mut process = process_handle
        .lock()
        .map_err(|_| anyhow::anyhow!("managed process `{process_id}` was poisoned"))?;
    if let Some(exit_status) = process.exit_status {
        return Ok(format!(
            "process_id: {process_id}\nexit_status: {exit_status}"
        ));
    }
    if let Some(status) = process.child.try_wait().map_err(|error| {
        anyhow::anyhow!("failed to check managed process `{process_id}`: {error}")
    })? {
        process.exit_status = Some(status.code().unwrap_or(-1));
        return Ok(format!(
            "process_id: {process_id}\nexit_status: {}",
            process.exit_status.unwrap_or(-1)
        ));
    }
    if let Some(stdin) = process.stdin.as_mut() {
        stdin.flush().map_err(|error| {
            anyhow::anyhow!("failed to flush managed process `{process_id}` stdin: {error}")
        })?;
    }
    terminate_child_process(&mut process.child, process_id, "managed process")?;
    let status = process.child.wait().map_err(|error| {
        anyhow::anyhow!("failed to wait on managed process `{process_id}`: {error}")
    })?;
    process.exit_status = Some(status.code().unwrap_or(-1));
    Ok(format!(
        "process_id: {process_id}\nexit_status: {}",
        process.exit_status.unwrap_or(-1)
    ))
}

fn terminate_child_process(
    child: &mut Child,
    process_id: &str,
    process_label: &str,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let process_group_id = child.id() as i32;
        let result = unsafe { libc::killpg(process_group_id, libc::SIGKILL) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(anyhow::anyhow!(
                    "failed to stop {process_label} `{process_id}`: {error}"
                ));
            }
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        child.kill().map_err(|error| {
            anyhow::anyhow!("failed to stop {process_label} `{process_id}`: {error}")
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        child.kill().map_err(|error| {
            anyhow::anyhow!("failed to stop {process_label} `{process_id}`: {error}")
        })
    }
}

fn wait_for_port(
    process_id: &str,
    host: &str,
    port: u16,
    timeout_ms: u64,
) -> anyhow::Result<String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut last_error = None;
    while Instant::now() < deadline {
        if let Some(status) = process_exit_status(process_id)? {
            return Err(anyhow::anyhow!(
                "managed process `{process_id}` exited before port {host}:{port} became ready ({status})"
            ));
        }
        let resolved = (host, port)
            .to_socket_addrs()
            .map_err(|error| anyhow::anyhow!("failed to resolve {host}:{port}: {error}"))?;
        for addr in resolved {
            match TcpStream::connect_timeout(&addr, Duration::from_millis(250)) {
                Ok(_) => {
                    return Ok(format!("process_id: {process_id}\nready: {host}:{port}"));
                }
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
    Err(anyhow::anyhow!(
        "managed process `{process_id}` did not expose {host}:{port} within {timeout_ms}ms{}",
        last_error
            .as_deref()
            .map(|error| format!(" ({error})"))
            .unwrap_or_default()
    ))
}

fn process_exit_status(process_id: &str) -> anyhow::Result<Option<i32>> {
    let process_handle = lookup_process(process_id)?;
    let mut process = process_handle
        .lock()
        .map_err(|_| anyhow::anyhow!("managed process `{process_id}` was poisoned"))?;
    if process.exit_status.is_some() {
        return Ok(process.exit_status);
    }
    match process.child.try_wait() {
        Ok(Some(status)) => {
            process.exit_status = Some(status.code().unwrap_or(-1));
            Ok(process.exit_status)
        }
        Ok(None) => Ok(None),
        Err(error) => Err(anyhow::anyhow!(
            "failed to poll managed process `{process_id}`: {error}"
        )),
    }
}

fn start_managed_browser(request: &BrowserOpenSpec) -> anyhow::Result<ManagedBrowser> {
    let config = load_agent_config(&request.project_root);
    let settings = config.agent_tools.browser;
    if !settings.enabled {
        return Err(anyhow::anyhow!("browser tooling is disabled in settings"));
    }
    settings.url_policy.allows_url(&request.url)?;
    let browser_id = next_managed_id("browser");
    let tempdir = tempfile::tempdir()
        .map_err(|error| anyhow::anyhow!("failed to create browser tempdir: {error}"))?;
    let mut child = spawn_browser_driver(&settings)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("browser driver stdout was not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("browser driver stderr was not piped"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("browser driver stdin was not piped"))?;
    let stderr_log = Arc::new(Mutex::new(String::new()));
    spawn_browser_stderr_reader(stderr, Arc::clone(&stderr_log));
    let mut browser = ManagedBrowser {
        id: browser_id,
        child,
        stdin,
        stdout: BufReader::new(stdout),
        stderr_log,
        next_request_id: 1,
        tempdir,
    };
    let request_id = browser.next_request_id;
    let browser_id = browser.id.clone();
    let output_dir = browser.tempdir.path().display().to_string();
    browser.request(json!({
        "id": request_id,
        "action": "open",
        "browser_id": browser_id,
        "url": request.url.as_str(),
        "headless": request.headless,
        "width": request.width,
        "height": request.height,
        "output_dir": output_dir,
    }))?;
    Ok(browser)
}

#[allow(clippy::disallowed_methods)]
fn spawn_browser_driver(settings: &BrowserToolSettings) -> anyhow::Result<Child> {
    let script = browser_driver_script();
    let mut command = Command::new(&settings.command);
    command.args(&settings.args).arg(script);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    util::set_pre_exec_to_start_new_session(&mut command);
    command.spawn().map_err(|error| {
        anyhow::anyhow!(
            "failed to start browser driver `{}`: {error}",
            settings.command
        )
    })
}

fn spawn_browser_stderr_reader(stderr: std::process::ChildStderr, log: Arc<Mutex<String>>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            let read = reader.read_line(&mut line).unwrap_or(0);
            if read == 0 {
                break;
            }
            if let Ok(mut log_buffer) = log.lock() {
                log_buffer.push_str(&line);
            } else {
                break;
            }
        }
    });
}

fn with_browser_mut<T>(
    browser_id: &str,
    operation: impl FnOnce(&mut ManagedBrowser) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let browser_handle = lookup_browser(browser_id)?;
    let mut browser = browser_handle
        .lock()
        .map_err(|_| anyhow::anyhow!("managed browser `{browser_id}` was poisoned"))?;
    operation(&mut browser)
}

fn lookup_browser(browser_id: &str) -> anyhow::Result<Arc<Mutex<ManagedBrowser>>> {
    browser_registry()
        .lock()
        .map_err(|_| anyhow::anyhow!("managed browser registry is poisoned"))?
        .get(browser_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("managed browser `{browser_id}` not found"))
}

fn close_managed_browser(browser_id: &str) -> anyhow::Result<String> {
    let browser_handle = {
        let mut registry = browser_registry()
            .lock()
            .map_err(|_| anyhow::anyhow!("managed browser registry is poisoned"))?;
        registry
            .remove(browser_id)
            .ok_or_else(|| anyhow::anyhow!("managed browser `{browser_id}` not found"))?
    };
    let mut browser = browser_handle
        .lock()
        .map_err(|_| anyhow::anyhow!("managed browser `{browser_id}` was poisoned"))?;
    if let Some(status) = browser.child.try_wait().map_err(|error| {
        anyhow::anyhow!("failed to check managed browser `{browser_id}`: {error}")
    })? {
        return Ok(format!(
            "browser_id: {browser_id}\nclosed: true\nexit_status: {}",
            status.code().unwrap_or(-1)
        ));
    }
    let request_id = browser.next_request_id;
    let browser_id = browser.id.clone();
    browser.request(json!({
        "id": request_id,
        "action": "close",
        "browser_id": browser_id,
    }))?;
    terminate_child_process(&mut browser.child, &browser_id, "browser driver")?;
    browser.child.wait().map_err(|error| {
        anyhow::anyhow!("failed to wait on managed browser `{browser_id}`: {error}")
    })?;
    Ok(format!("browser_id: {browser_id}\nclosed: true"))
}

fn render_browser_response(
    browser_id: &str,
    tool_name: &str,
    response: BrowserResponse,
) -> anyhow::Result<String> {
    if !response.ok {
        return Err(anyhow::anyhow!(
            "browser {browser_id}/{tool_name} failed: {}",
            response
                .error
                .unwrap_or_else(|| "unknown error".to_string())
        ));
    }
    let rendered = response
        .result
        .map(|value| serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()))
        .unwrap_or_else(|| "{}".to_string());
    Ok(format!("browser_id: {browser_id}\n{rendered}"))
}

impl ManagedBrowser {
    fn request(&mut self, request: serde_json::Value) -> anyhow::Result<BrowserResponse> {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        let mut request = request;
        if let Some(object) = request.as_object_mut() {
            object.insert("id".to_string(), serde_json::json!(request_id));
        }
        let line = serde_json::to_string(&request)?;
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()?;
        let mut response_line = String::new();
        let read = self.stdout.read_line(&mut response_line)?;
        if read == 0 {
            return Err(anyhow::anyhow!(
                "browser driver exited unexpectedly{}",
                self.stderr_log
                    .lock()
                    .ok()
                    .map(|log| if log.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", log.trim())
                    })
                    .unwrap_or_default()
            ));
        }
        let response: BrowserResponse = serde_json::from_str(response_line.trim())?;
        if response.ok {
            Ok(response)
        } else {
            Err(anyhow::anyhow!(
                "{}",
                response
                    .error
                    .unwrap_or_else(|| "browser driver returned an error".to_string())
            ))
        }
    }
}

fn browser_driver_script() -> String {
    r#"
const fs = require('fs');
const path = require('path');
const readline = require('readline');

let browser = null;
let context = null;
let page = null;
let logs = [];
let networkErrors = [];
let browserId = null;
let outputDir = null;

function respond(id, ok, result, error) {
  process.stdout.write(JSON.stringify({ id, ok, result, error }) + '\n');
}

function setupListeners() {
  if (!page) return;
  page.on('console', async (message) => {
    logs.push({
      type: message.type(),
      text: message.text(),
      location: message.location ? message.location() : undefined,
    });
  });
  page.on('pageerror', (error) => {
    networkErrors.push({ type: 'pageerror', message: String(error) });
  });
  page.on('requestfailed', (request) => {
    networkErrors.push({
      type: 'requestfailed',
      url: request.url(),
      failure: request.failure() ? request.failure().errorText : undefined,
    });
  });
}

async function openSession(message) {
  browserId = message.browser_id;
  outputDir = message.output_dir;
  if (!outputDir) throw new Error('missing output_dir');
  fs.mkdirSync(outputDir, { recursive: true });
  const { chromium } = require('playwright');
  browser = await chromium.launch({
    headless: message.headless !== false,
  });
  context = await browser.newContext({
    viewport: message.width && message.height ? { width: message.width, height: message.height } : undefined,
  });
  page = await context.newPage();
  setupListeners();
  await page.goto(message.url, { waitUntil: 'domcontentloaded' });
  return { browser_id: browserId, url: page.url() };
}

async function screenshot() {
  if (!page) throw new Error('browser not open');
  const screenshotPath = path.join(outputDir, `screenshot-${Date.now()}.png`);
  await page.screenshot({ path: screenshotPath, fullPage: true });
  const stats = fs.statSync(screenshotPath);
  return { browser_id: browserId, screenshot_path: screenshotPath, bytes: stats.size };
}

async function consoleLogs(message) {
  return { browser_id: browserId, logs: logs.slice(-Math.max(1, message.limit || 100)) };
}

async function networkErrors(message) {
  return { browser_id: browserId, errors: networkErrors.slice(-Math.max(1, message.limit || 100)) };
}

async function accessibilitySnapshot() {
  if (!page) throw new Error('browser not open');
  const snapshot = await page.accessibility.snapshot({ interestingOnly: false });
  return { browser_id: browserId, snapshot };
}

async function closeSession() {
  if (browser) {
    await browser.close();
  }
  browser = null;
  context = null;
  page = null;
  return { browser_id: browserId, closed: true };
}

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on('line', async (line) => {
  let message;
  try {
    message = JSON.parse(line);
  } catch (error) {
    respond(null, false, null, `invalid json: ${error.message}`);
    return;
  }
  try {
    let result;
    switch (message.action) {
      case 'open':
        result = await openSession(message);
        break;
      case 'screenshot':
        result = await screenshot(message);
        break;
      case 'console_logs':
        result = await consoleLogs(message);
        break;
      case 'network_errors':
        result = await networkErrors(message);
        break;
      case 'accessibility_snapshot':
        result = await accessibilitySnapshot(message);
        break;
      case 'close':
        result = await closeSession(message);
        respond(message.id, true, result, null);
        process.exit(0);
        return;
      default:
        throw new Error(`unknown action: ${message.action}`);
    }
    respond(message.id, true, result, null);
  } catch (error) {
    respond(message.id, false, null, String(error.stack || error.message || error));
  }
});
"#
    .to_string()
}

fn emit_tool_result(
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    action: AgentAction,
    result: anyhow::Result<String>,
    action_label: &str,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    super::emit_tool_result(
        event_tx,
        session_id,
        action,
        result,
        action_label,
        responder,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quorp::tui::ChatUiEvent;
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
                    let finished =
                        matches!(event, TuiEvent::Chat(ChatUiEvent::CommandFinished(_, _)));
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
}

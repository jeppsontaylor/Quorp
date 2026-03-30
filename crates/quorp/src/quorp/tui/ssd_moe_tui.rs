use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{BufRead as _, Write as _};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use paths::{ensure_user_models_dir, user_models_dir};

use crate::quorp::tui::model_registry::ModelSpec;
use crate::quorp::tui::ssd_moe_client::default_local_base_url;

#[cfg_attr(not(test), allow(dead_code))]
#[cfg_attr(test, allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub enum ModelStatus {
    NotDownloaded,
    Downloading {
        progress_pct: f32,
        downloaded_gb: f32,
        total_gb: f32,
    },
    Packing {
        layer: u32,
        total_layers: u32,
    },
    Ready,
    Starting,
    Running,
    Stopping,
    Failed(String),
}

impl ModelStatus {
    pub fn label(&self) -> String {
        match self {
            ModelStatus::NotDownloaded => "Not Downloaded".into(),
            ModelStatus::Downloading { progress_pct, .. } => {
                format!("Downloading {:.1}%", progress_pct)
            }
            ModelStatus::Packing {
                layer,
                total_layers,
            } => format!("Packing {layer}/{total_layers}"),
            ModelStatus::Ready => "Ready".into(),
            ModelStatus::Starting => "Starting...".into(),
            ModelStatus::Running => "Online".into(),
            ModelStatus::Stopping => "Stopping...".into(),
            ModelStatus::Failed(_) => "Offline".into(),
        }
    }

    pub fn indicator(&self) -> &str {
        match self {
            ModelStatus::Ready => "⬜",
            ModelStatus::Starting | ModelStatus::Stopping => "🟡",
            ModelStatus::Running => "🟢",
            ModelStatus::Failed(_) => "🔴",
            ModelStatus::NotDownloaded
            | ModelStatus::Downloading { .. }
            | ModelStatus::Packing { .. } => "⬜",
        }
    }
}

#[derive(Debug)]
struct RuntimeState {
    port: u16,
    status: ModelStatus,
    logs: VecDeque<String>,
    active_model: Option<ModelSpec>,
    child_process: Option<Child>,
    log_file_path: PathBuf,
}

impl RuntimeState {
    fn new(port: u16) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let log_file_path = PathBuf::from(home).join(".config/quorp/logs/ssd-moe.log");
        Self {
            port,
            status: ModelStatus::Ready,
            logs: VecDeque::with_capacity(500),
            active_model: None,
            child_process: None,
            log_file_path,
        }
    }
}

#[derive(Clone)]
pub struct SsdMoeRuntimeHandle {
    inner: Arc<Mutex<RuntimeState>>,
}

impl SsdMoeRuntimeHandle {
    pub fn shared_handle() -> Self {
        static SHARED_RUNTIME: OnceLock<Arc<Mutex<RuntimeState>>> = OnceLock::new();
        Self {
            inner: SHARED_RUNTIME
                .get_or_init(|| {
                    Arc::new(Mutex::new(RuntimeState::new(
                        flash_moe_defaults::DEFAULT_INFER_SERVE_PORT,
                    )))
                })
                .clone(),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn detached_for_test(port: u16) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RuntimeState::new(port))),
        }
    }

    pub fn port(&self) -> u16 {
        self.inner.lock().expect("ssd-moe runtime lock").port
    }

    pub fn status(&self) -> ModelStatus {
        self.inner
            .lock()
            .expect("ssd-moe runtime lock")
            .status
            .clone()
    }

    pub fn active_model(&self) -> Option<ModelSpec> {
        self.inner
            .lock()
            .expect("ssd-moe runtime lock")
            .active_model
            .clone()
    }

    pub fn base_url(&self) -> String {
        default_local_base_url(self.port())
    }

    fn append_log_line(&self, message: impl Into<String>) {
        let message = message.into();
        let log_file_path = {
            let mut state = self.inner.lock().expect("ssd-moe runtime lock");
            if state.logs.len() >= 500 {
                state.logs.pop_front();
            }
            state.logs.push_back(message.clone());
            state.log_file_path.clone()
        };
        if let Some(parent) = log_file_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file_path)
        {
            let _ = writeln!(file, "{message}");
        }
    }

    fn tcp_probe(&self) -> bool {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.port()));
        std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
    }

    pub fn resolve_server_path(project_root: &Path) -> Option<PathBuf> {
        let candidate = project_root.join("external/flash-moe/infer");
        candidate.is_file().then_some(candidate)
    }

    fn resolve_model_weights_path(model_dir_name: &str) -> Result<PathBuf, String> {
        if let Err(error) = ensure_user_models_dir() {
            let attempted = user_models_dir();
            return Err(format!(
                "Could not create user models directory at {:?}: {}",
                attempted, error
            ));
        }
        let path = if Path::new(model_dir_name).is_absolute() {
            PathBuf::from(model_dir_name)
        } else {
            user_models_dir().join(model_dir_name)
        };
        Ok(path.canonicalize().unwrap_or(path))
    }

    pub fn ensure_running(&self, project_root: &Path, model: &ModelSpec) {
        {
            let mut state = self.inner.lock().expect("ssd-moe runtime lock");
            state.active_model = Some(model.clone());
            if matches!(state.status, ModelStatus::Starting) {
                return;
            }
        }

        if self.tcp_probe() {
            {
                let mut state = self.inner.lock().expect("ssd-moe runtime lock");
                state.status = ModelStatus::Running;
                state.active_model = Some(model.clone());
            }
            self.append_log_line(format!(
                "Attached to existing SSD-MOE server on port {} for model {}",
                self.port(),
                model.id
            ));
            return;
        }

        let Some(infer_bin) = Self::resolve_server_path(project_root) else {
            let message = "flash-moe server binary not found".to_string();
            let mut state = self.inner.lock().expect("ssd-moe runtime lock");
            state.status = ModelStatus::Failed(message.clone());
            drop(state);
            self.append_log_line(message);
            return;
        };

        let model_weights_path = match Self::resolve_model_weights_path(model.model_dir_name) {
            Ok(path) => path,
            Err(error) => {
                let mut state = self.inner.lock().expect("ssd-moe runtime lock");
                state.status = ModelStatus::Failed(error.clone());
                drop(state);
                self.append_log_line(error);
                return;
            }
        };

        if !model_weights_path.join("packed_experts").exists() {
            let message = format!(
                "SSD-MOE model weights not found at {:?}. Expected packed_experts/ under the selected model directory.",
                model_weights_path
            );
            let mut state = self.inner.lock().expect("ssd-moe runtime lock");
            state.status = ModelStatus::Failed(message.clone());
            drop(state);
            self.append_log_line(message);
            return;
        }

        let Some(model_path) = model_weights_path.to_str() else {
            let message = "SSD-MOE model path is not valid UTF-8".to_string();
            let mut state = self.inner.lock().expect("ssd-moe runtime lock");
            state.status = ModelStatus::Failed(message.clone());
            drop(state);
            self.append_log_line(message);
            return;
        };

        let flash_moe_dir = infer_bin
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| project_root.to_path_buf());
        let port = self.port();

        {
            let mut state = self.inner.lock().expect("ssd-moe runtime lock");
            state.status = ModelStatus::Starting;
            state.logs.clear();
        }
        self.append_log_line(format!(
            "Starting SSD-MOE server on port {} with model {}",
            port, model.id
        ));

        let mut command = Command::new(&infer_bin);
        command
            .current_dir(&flash_moe_dir)
            .args([
                "--model",
                model_path,
                "--serve",
                &port.to_string(),
                "--k",
                &model.k_experts.to_string(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        match command.spawn() {
            Ok(mut child) => {
                if let Some(stdout) = child.stdout.take() {
                    let runtime = self.clone();
                    std::thread::spawn(move || {
                        let reader = std::io::BufReader::new(stdout);
                        for line in reader.lines().map_while(Result::ok) {
                            runtime.append_log_line(format!("[stdout] {line}"));
                        }
                    });
                }
                if let Some(stderr) = child.stderr.take() {
                    let runtime = self.clone();
                    std::thread::spawn(move || {
                        let reader = std::io::BufReader::new(stderr);
                        for line in reader.lines().map_while(Result::ok) {
                            runtime.append_log_line(format!("[stderr] {line}"));
                        }
                    });
                }
                let mut state = self.inner.lock().expect("ssd-moe runtime lock");
                state.child_process = Some(child);
            }
            Err(error) => {
                let message = format!("Failed to spawn infer: {error}");
                let mut state = self.inner.lock().expect("ssd-moe runtime lock");
                state.status = ModelStatus::Failed(message.clone());
                drop(state);
                self.append_log_line(message);
            }
        }
    }

    pub fn poll_health(&self) {
        let mut state = self.inner.lock().expect("ssd-moe runtime lock");
        match state.status {
            ModelStatus::Starting => {
                if self.tcp_probe() {
                    state.status = ModelStatus::Running;
                } else if let Some(child) = state.child_process.as_mut() {
                    if let Ok(Some(status)) = child.try_wait() {
                        state.status = ModelStatus::Failed(format!(
                            "SSD-MOE server exited before it started (status {:?})",
                            status.code()
                        ));
                        state.child_process = None;
                    }
                }
            }
            ModelStatus::Running => {
                if let Some(child) = state.child_process.as_mut() {
                    if let Ok(Some(status)) = child.try_wait() {
                        state.status = ModelStatus::Failed(format!(
                            "SSD-MOE server exited unexpectedly (status {:?})",
                            status.code()
                        ));
                        state.child_process = None;
                    }
                } else if !self.tcp_probe() {
                    state.status = ModelStatus::Ready;
                }
            }
            _ => {}
        }
    }

    pub fn wait_until_ready(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            self.poll_health();
            match self.status() {
                ModelStatus::Running => return Ok(()),
                ModelStatus::Failed(message) => return Err(message),
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "SSD-MOE server did not become ready on port {} within {:?}",
                    self.port(),
                    timeout
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn switch_model(&self, project_root: &Path, new_model: &ModelSpec) {
        crate::quorp::tui::model_registry::save_model(new_model.id);
        let already_active = self
            .active_model()
            .is_some_and(|current| current.id == new_model.id)
            && self.tcp_probe();
        if already_active {
            let mut state = self.inner.lock().expect("ssd-moe runtime lock");
            state.status = ModelStatus::Running;
            state.active_model = Some(new_model.clone());
            return;
        }
        self.stop();
        self.ensure_running(project_root, new_model);
    }

    pub fn stop(&self) {
        let mut state = self.inner.lock().expect("ssd-moe runtime lock");
        state.status = ModelStatus::Stopping;
        if let Some(mut child) = state.child_process.take() {
            if let Err(error) = child.kill() {
                log::warn!("tui: failed to stop SSD-MOE child: {error}");
            }
        }
        state.status = ModelStatus::Ready;
    }

    pub(crate) fn set_active_model_for_test(&self, model: Option<ModelSpec>) {
        self.inner.lock().expect("ssd-moe runtime lock").active_model = model;
    }
}

pub struct SsdMoeManager {
    runtime: SsdMoeRuntimeHandle,
    test_status_override: Option<ModelStatus>,
}

impl SsdMoeManager {
    pub fn new() -> Self {
        Self {
            runtime: SsdMoeRuntimeHandle::shared_handle(),
            test_status_override: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_detached_for_test(port: u16) -> Self {
        Self {
            runtime: SsdMoeRuntimeHandle::detached_for_test(port),
            test_status_override: None,
        }
    }

    pub fn status(&self) -> ModelStatus {
        self.test_status_override
            .clone()
            .unwrap_or_else(|| self.runtime.status())
    }

    pub fn active_model(&self) -> Option<ModelSpec> {
        self.runtime.active_model()
    }

    pub fn set_status_for_test(&mut self, status: ModelStatus) {
        self.test_status_override = Some(status);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn resolve_server_path(project_root: &Path) -> Option<PathBuf> {
        SsdMoeRuntimeHandle::resolve_server_path(project_root)
    }

    pub fn ensure_running(&mut self, project_root: &Path, model: &ModelSpec) {
        self.test_status_override = None;
        self.runtime.ensure_running(project_root, model);
    }

    pub fn poll_health(&mut self) {
        self.runtime.poll_health();
    }

    pub fn switch_model(&mut self, project_root: &Path, new_model: &ModelSpec) {
        self.test_status_override = None;
        self.runtime.switch_model(project_root, new_model);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn wait_until_ready(&self, timeout: Duration) -> Result<(), String> {
        self.runtime.wait_until_ready(timeout)
    }

    pub fn stop(&mut self) {
        self.test_status_override = None;
        self.runtime.stop();
    }

    pub(crate) fn set_active_model_for_test(&mut self, model: Option<ModelSpec>) {
        self.runtime.set_active_model_for_test(model);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_labels_are_correct() {
        assert_eq!(ModelStatus::Ready.label(), "Ready");
        assert_eq!(ModelStatus::Starting.label(), "Starting...");
        assert_eq!(ModelStatus::Running.label(), "Online");
        assert_eq!(ModelStatus::Failed("err".into()).label(), "Offline");
    }

    #[test]
    fn status_indicators_are_correct() {
        assert_eq!(ModelStatus::Ready.indicator(), "⬜");
        assert_eq!(ModelStatus::Starting.indicator(), "🟡");
        assert_eq!(ModelStatus::Running.indicator(), "🟢");
        assert_eq!(ModelStatus::Failed("err".into()).indicator(), "🔴");
    }

    #[test]
    fn resolve_missing_dir_returns_none() {
        let temp = tempfile::tempdir().expect("tempdir");
        assert!(SsdMoeManager::resolve_server_path(temp.path()).is_none());
    }

    #[test]
    fn resolve_existing_dir_returns_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let flash_dir = temp.path().join("external/flash-moe");
        std::fs::create_dir_all(&flash_dir).expect("mkdir");
        std::fs::write(flash_dir.join("infer"), "#!/bin/sh\n").expect("write");
        let result = SsdMoeManager::resolve_server_path(temp.path());
        assert!(result.is_some());
        assert!(result.expect("some").to_string_lossy().contains("flash-moe"));
    }

    #[test]
    fn ensure_running_fails_when_no_binary() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut server = SsdMoeManager::new_detached_for_test(59_997);
        server.ensure_running(
            temp.path(),
            &crate::quorp::tui::model_registry::local_moe_catalog()[1],
        );
        assert!(matches!(server.status(), ModelStatus::Failed(_)));
    }

    #[test]
    fn health_check_closed_port_returns_false() {
        let server = SsdMoeManager::new_detached_for_test(59_999);
        assert!(!server.runtime.tcp_probe());
    }

    #[test]
    fn poll_health_noop_when_not_starting() {
        let mut server = SsdMoeManager::new_detached_for_test(59_994);
        server.poll_health();
        assert_eq!(server.status(), ModelStatus::Ready);
    }

    #[test]
    fn wait_until_ready_times_out_when_unavailable() {
        let server = SsdMoeManager::new_detached_for_test(59_993);
        let error = server
            .wait_until_ready(Duration::from_millis(50))
            .expect_err("should time out");
        assert!(error.contains("did not become ready"));
    }
}

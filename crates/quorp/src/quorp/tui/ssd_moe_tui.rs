use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{BufRead as _, Read as _, Write as _};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use ::paths;
use ssd_moe_client::{
    AcquireManagedError, ClientConfig, ManagedLease, acquire_managed_blocking_detailed,
    broker_installation_status, load_runtime_managed_blocking,
};
use ssd_moe_contract::{
    AcquireDisposition, BrokerAcquirePolicy, BrokerAcquireRequest, BrokerConflictDetails,
    BrokerErrorResponse, BrokerHealth, BrokerInstanceRecord, BrokerStateFile, LaunchSpec,
    PreparedModelInspection, PreparedModelStatus, RequestedInstance, RuntimeLoadRequest,
};
use ssd_moe_launch::{
    RuntimeDiscoveryFailure, RuntimeDiscoveryRequest, RuntimeModelSource, broker_state_file,
    build_fingerprint, inspect_prepared_model_layout,
};

use crate::quorp::tui::local_model_program::LocalModelRole;
use crate::quorp::tui::model_registry::{self, LocalRuntimeBackend, ModelSpec};
use crate::quorp::tui::ssd_moe_client::default_local_base_url;
use serde_json::json;

#[cfg(test)]
static SSD_MOE_MODELS_ENV_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    SSD_MOE_MODELS_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
    WaitingForBroker,
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
            ModelStatus::WaitingForBroker => "Waiting...".into(),
            ModelStatus::Running => "Online".into(),
            ModelStatus::Stopping => "Stopping...".into(),
            ModelStatus::Failed(_) => "Offline".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SsdMoeStartupFailure {
    #[allow(dead_code)]
    ModelLayout(String),
    Setup(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdMoeBootstrapState {
    pub phase_label: String,
    pub detail: String,
    pub diagnostic: SsdMoeStartupDiagnostic,
    pub last_transition_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SsdMoeStartupCode {
    InferBinaryMissing,
    ModelLayoutMissing,
    BrokerUnavailable,
    BrokerBusy,
    WaitingForBroker,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdMoeLaunchProbeSummary {
    pub checked_infer_paths: Vec<PathBuf>,
    pub infer_bin: Option<PathBuf>,
    pub model_path: Option<PathBuf>,
    pub weights_path: Option<PathBuf>,
    pub manifest_path: Option<PathBuf>,
    pub model_source_label: Option<String>,
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdMoeBrokerInstanceSummary {
    pub instance_id: String,
    pub base_url: String,
    pub lease_count: usize,
    pub healthy: bool,
    pub infer_bin: PathBuf,
    pub model_path: PathBuf,
    pub weights_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdMoeBrokerProbeSummary {
    pub state_dir: PathBuf,
    pub expected_binary_path: PathBuf,
    pub installed_binary_exists: bool,
    pub broker_url: Option<String>,
    pub health: Option<BrokerHealth>,
    pub instances: Vec<SsdMoeBrokerInstanceSummary>,
    pub probe_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdMoeStartupDiagnostic {
    pub code: SsdMoeStartupCode,
    pub detail: String,
    pub launch_probe: SsdMoeLaunchProbeSummary,
    pub broker_probe: SsdMoeBrokerProbeSummary,
    pub active_instance: Option<SsdMoeBrokerInstanceSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdMoeAcquireMetadata {
    pub broker_url: String,
    pub base_url: String,
    pub instance_id: String,
    pub disposition: AcquireDisposition,
    pub lease_count: usize,
    pub model_path: PathBuf,
    pub weights_path: Option<PathBuf>,
    pub manifest_path: Option<PathBuf>,
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdMoeWaitMetadata {
    pub message: String,
    pub conflict: Option<BrokerConflictDetails>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdMoeTransitionRecord {
    pub previous_status: String,
    pub next_status: String,
    pub reason: String,
    pub model_id: Option<String>,
    pub port: u16,
    pub base_url: Option<String>,
    pub broker_url: Option<String>,
    pub instance_id: Option<String>,
    pub lease_count: Option<usize>,
    pub tcp_probe_ok: Option<bool>,
}

#[derive(Debug)]
struct RuntimeState {
    port: u16,
    status: ModelStatus,
    logs: VecDeque<String>,
    active_model: Option<ModelSpec>,
    lease: Option<ManagedLease>,
    base_url: Option<String>,
    runtime_metadata: Option<serde_json::Value>,
    acquire_metadata: Option<SsdMoeAcquireMetadata>,
    wait_metadata: Option<SsdMoeWaitMetadata>,
    next_retry_at: Option<Instant>,
    pending_retry_model: Option<ModelSpec>,
    runtime_process: Option<Child>,
    setup_process: Option<Child>,
    project_root: Option<PathBuf>,
    log_file_path: PathBuf,
    recent_transitions: VecDeque<SsdMoeTransitionRecord>,
    last_transition_reason: Option<String>,
}

enum TurboHeroAttachState {
    NotListening,
    AttachedRunning(String),
    AttachedStarting(String),
    WrongListener(String),
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
            lease: None,
            base_url: None,
            runtime_metadata: None,
            acquire_metadata: None,
            wait_metadata: None,
            next_retry_at: None,
            pending_retry_model: None,
            runtime_process: None,
            setup_process: None,
            project_root: None,
            log_file_path,
            recent_transitions: VecDeque::with_capacity(32),
            last_transition_reason: None,
        }
    }
}

#[derive(Clone)]
pub struct SsdMoeRuntimeHandle {
    inner: Arc<Mutex<RuntimeState>>,
}

impl SsdMoeRuntimeHandle {
    fn runtime_models_dir() -> PathBuf {
        paths::user_models_dir()
    }

    fn broker_state_dir() -> PathBuf {
        paths::ssd_moe_state_dir()
    }

    fn broker_client_config() -> ClientConfig {
        let models_dir = Self::runtime_models_dir();
        let state_dir = Self::broker_state_dir();
        ClientConfig {
            models_dir: models_dir.clone(),
            state_dir,
            ..ClientConfig::default()
        }
    }

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

    fn tcp_probe_port(port: u16) -> bool {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
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

    pub fn acquire_metadata(&self) -> Option<SsdMoeAcquireMetadata> {
        self.inner
            .lock()
            .expect("ssd-moe runtime lock")
            .acquire_metadata
            .clone()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    #[cfg_attr(not(test), allow(dead_code))]
    #[allow(dead_code)]
    pub fn recent_transitions(&self) -> Vec<SsdMoeTransitionRecord> {
        self.inner
            .lock()
            .expect("ssd-moe runtime lock")
            .recent_transitions
            .iter()
            .cloned()
            .collect()
    }

    pub fn last_transition_reason(&self) -> Option<String> {
        self.inner
            .lock()
            .expect("ssd-moe runtime lock")
            .last_transition_reason
            .clone()
    }

    pub fn wait_metadata(&self) -> Option<SsdMoeWaitMetadata> {
        self.inner
            .lock()
            .expect("ssd-moe runtime lock")
            .wait_metadata
            .clone()
    }

    fn push_transition(
        &self,
        state: &mut RuntimeState,
        next_status: &ModelStatus,
        reason: String,
        tcp_probe_ok: Option<bool>,
    ) {
        let previous_status = state.status.label();
        let next_status_label = next_status.label();
        let model_id = state
            .active_model
            .as_ref()
            .map(|model| model.id.to_string());
        let acquire_metadata = state.acquire_metadata.as_ref();
        let record = SsdMoeTransitionRecord {
            previous_status: previous_status.clone(),
            next_status: next_status_label.clone(),
            reason: reason.clone(),
            model_id: model_id.clone(),
            port: state.port,
            base_url: state.base_url.clone(),
            broker_url: acquire_metadata.map(|metadata| metadata.broker_url.clone()),
            instance_id: acquire_metadata.map(|metadata| metadata.instance_id.clone()),
            lease_count: acquire_metadata.map(|metadata| metadata.lease_count),
            tcp_probe_ok,
        };
        if state.recent_transitions.len() >= 32 {
            state.recent_transitions.pop_front();
        }
        state.recent_transitions.push_back(record.clone());
        state.last_transition_reason = Some(reason.clone());
        state.status = next_status.clone();

        crate::quorp::tui::diagnostics::log_event(
            "ssd.runtime_transition",
            json!({
                "previous_status": previous_status,
                "next_status": next_status_label,
                "reason": reason,
                "model_id": model_id,
                "port": record.port,
                "base_url": record.base_url,
                "broker_url": record.broker_url,
                "instance_id": record.instance_id,
                "lease_count": record.lease_count,
                "tcp_probe_ok": tcp_probe_ok,
            }),
        );
    }

    pub fn base_url(&self) -> String {
        self.inner
            .lock()
            .expect("ssd-moe runtime lock")
            .base_url
            .clone()
            .unwrap_or_else(|| default_local_base_url(self.port()))
    }

    pub fn runtime_metadata(&self) -> Option<serde_json::Value> {
        self.inner
            .lock()
            .expect("ssd-moe runtime lock")
            .runtime_metadata
            .clone()
    }

    pub fn bootstrap_state(&self) -> SsdMoeBootstrapState {
        let status = self.status();
        let active_model = self.active_model();
        let acquire_metadata = self.acquire_metadata();
        let wait_metadata = self.wait_metadata();
        let diagnostic = Self::startup_diagnostic(
            status.clone(),
            active_model.as_ref(),
            acquire_metadata.as_ref(),
            wait_metadata.as_ref(),
        );
        let phase_label = match status {
            ModelStatus::NotDownloaded => "Missing".to_string(),
            ModelStatus::Downloading { .. } => "Downloading".to_string(),
            ModelStatus::Packing { .. } => "Packing".to_string(),
            ModelStatus::Ready => "Prepared".to_string(),
            ModelStatus::Starting => "Starting".to_string(),
            ModelStatus::WaitingForBroker => "Waiting".to_string(),
            ModelStatus::Running => "Online".to_string(),
            ModelStatus::Stopping => "Stopping".to_string(),
            ModelStatus::Failed(_) => "Blocked".to_string(),
        };
        SsdMoeBootstrapState {
            phase_label,
            detail: diagnostic.detail.clone(),
            diagnostic,
            last_transition_reason: self.last_transition_reason(),
        }
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
        Self::tcp_probe_port(self.port())
    }

    pub fn resolve_server_path(_project_root: &Path) -> Option<PathBuf> {
        let model = crate::quorp::tui::model_registry::get_saved_model()?;
        if matches!(
            model.runtime_backend,
            LocalRuntimeBackend::TurboHeroOracle { .. }
        ) {
            let script = Self::turbohero_oracle_script_path();
            return script.is_file().then_some(script);
        }
        let request = Self::runtime_discovery_request(&model).ok()?;
        request.infer_bin_override.or_else(|| {
            request
                .infer_bin_candidates
                .into_iter()
                .find(|candidate| candidate.is_file())
        })
    }

    #[allow(dead_code)]
    fn resolve_setup_script_path() -> Option<PathBuf> {
        if let Ok(override_path) = std::env::var("QUORP_SSD_MOE_SETUP_SCRIPT") {
            let path = PathBuf::from(override_path);
            if path.is_file() {
                return Some(path);
            }
        }
        None
    }

    fn python_command() -> String {
        std::env::var("QUORP_PYTHON").unwrap_or_else(|_| "python3".to_string())
    }

    fn turbohero_root() -> PathBuf {
        std::env::var("QUORP_TURBOHERO_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/Users/bentaylor/Code/TurboHero"))
    }

    fn turbohero_python_command() -> PathBuf {
        if let Ok(path) = std::env::var("QUORP_TURBOHERO_PYTHON") {
            let path = PathBuf::from(path);
            if path.exists() {
                return path;
            }
        }
        let venv_python = Self::turbohero_root().join(".venv-py312/bin/python");
        if venv_python.exists() {
            return venv_python;
        }
        PathBuf::from(Self::python_command())
    }

    #[allow(clippy::disallowed_methods)]
    #[allow(dead_code)]
    fn configure_setup_command_stdio(command: &mut Command) {
        // The setup path is consumed through blocking stdio readers below, so this command must
        // stay on `std::process::Command` rather than being converted into an async process type.
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    }

    #[allow(clippy::disallowed_methods)]
    #[allow(dead_code)]
    fn spawn_setup_command(command: &mut Command) -> std::io::Result<Child> {
        command.spawn()
    }

    fn runtime_discovery_request(
        model: &ModelSpec,
    ) -> Result<RuntimeDiscoveryRequest, RuntimeDiscoveryFailure> {
        let Some(model_dir_name) = model.model_dir_name() else {
            return Err(RuntimeDiscoveryFailure::ModelLayoutMissing {
                hf_repo: model.hf_repo.to_string(),
                prepared_weights_root: Self::runtime_models_dir(),
                checked_paths: Vec::new(),
            });
        };
        let models_dir = Self::runtime_models_dir();
        let hf_home = std::env::var("HF_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| models_dir.join("huggingface"));
        let weights_dir = std::env::var("QUORP_FLASH_MOE_WEIGHTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| models_dir.join("flash-moe").join(model_dir_name));
        Ok(RuntimeDiscoveryRequest {
            infer_bin_override: std::env::var("QUORP_SSD_MOE_INFER_BIN")
                .ok()
                .map(PathBuf::from),
            infer_bin_candidates: vec![
                models_dir.join("bin/infer"),
                models_dir.join("flash-moe/infer"),
            ],
            model_path_override: std::env::var("QUORP_SSD_MOE_MODEL_PATH")
                .ok()
                .map(PathBuf::from),
            models_dir,
            hf_home,
            weights_dir,
            prepared_model_dir_name: model_dir_name.to_string(),
            hf_repo: model.hf_repo.to_string(),
        })
    }

    fn startup_diagnostic(
        status: ModelStatus,
        active_model: Option<&ModelSpec>,
        acquire_metadata: Option<&SsdMoeAcquireMetadata>,
        wait_metadata: Option<&SsdMoeWaitMetadata>,
    ) -> SsdMoeStartupDiagnostic {
        let launch_probe = active_model.map_or(
            SsdMoeLaunchProbeSummary {
                checked_infer_paths: Vec::new(),
                infer_bin: None,
                model_path: None,
                weights_path: None,
                manifest_path: None,
                model_source_label: None,
                failure: Some("no active local model selected".to_string()),
            },
            Self::probe_launch_inputs,
        );
        let broker_probe = Self::probe_broker_state();
        let active_instance = broker_probe
            .instances
            .iter()
            .find(|instance| instance.healthy)
            .cloned();
        let code = Self::classify_startup_code(
            &status,
            active_model,
            &launch_probe,
            &broker_probe,
            wait_metadata,
        );
        let detail = Self::startup_detail(
            &code,
            &status,
            active_model,
            &launch_probe,
            &broker_probe,
            active_instance.as_ref(),
            acquire_metadata,
            wait_metadata,
        );

        SsdMoeStartupDiagnostic {
            code,
            detail,
            launch_probe,
            broker_probe,
            active_instance,
        }
    }

    fn turbohero_oracle_script_path() -> PathBuf {
        Self::turbohero_root().join("turbohero/mlx_oracle_server.py")
    }

    fn turbohero_snapshot_path(model: &ModelSpec) -> Result<PathBuf, String> {
        let models_dir = Self::runtime_models_dir();
        let hf_home = std::env::var("HF_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| models_dir.join("huggingface"));
        let snapshot = hf_snapshot_roots_for_repo_local(&hf_home, model.hf_repo)
            .iter()
            .flat_map(|root| hf_snapshot_dirs_local(root))
            .find(|candidate| {
                candidate.join("config.json").is_file()
                    && candidate.join("tokenizer_config.json").is_file()
                    && candidate.join("tokenizer.json").is_file()
            })
            .ok_or_else(|| {
                format!(
                    "TurboHero snapshot missing for {} under {}",
                    model.hf_repo,
                    hf_home.display()
                )
            })?;
        Ok(snapshot)
    }

    fn runtime_health_url(port: u16) -> String {
        format!("http://127.0.0.1:{port}/health")
    }

    fn runtime_metadata_url(port: u16) -> String {
        format!("http://127.0.0.1:{port}/v1/runtime")
    }

    fn classify_startup_code(
        status: &ModelStatus,
        active_model: Option<&ModelSpec>,
        launch_probe: &SsdMoeLaunchProbeSummary,
        broker_probe: &SsdMoeBrokerProbeSummary,
        wait_metadata: Option<&SsdMoeWaitMetadata>,
    ) -> SsdMoeStartupCode {
        if active_model.is_some_and(|model| {
            matches!(
                model.runtime_backend,
                LocalRuntimeBackend::TurboHeroOracle { .. }
            )
        }) {
            return match status {
                ModelStatus::Running | ModelStatus::Starting | ModelStatus::Ready => {
                    if launch_probe.infer_bin.is_none() {
                        SsdMoeStartupCode::InferBinaryMissing
                    } else if launch_probe.model_path.is_none() {
                        SsdMoeStartupCode::ModelLayoutMissing
                    } else {
                        SsdMoeStartupCode::Ready
                    }
                }
                ModelStatus::Failed(_) => {
                    if launch_probe.infer_bin.is_none() {
                        SsdMoeStartupCode::InferBinaryMissing
                    } else if launch_probe.model_path.is_none() {
                        SsdMoeStartupCode::ModelLayoutMissing
                    } else {
                        SsdMoeStartupCode::Ready
                    }
                }
                _ if launch_probe.infer_bin.is_none() => SsdMoeStartupCode::InferBinaryMissing,
                _ if launch_probe.model_path.is_none() => SsdMoeStartupCode::ModelLayoutMissing,
                _ => SsdMoeStartupCode::Ready,
            };
        }
        match status {
            ModelStatus::Running => SsdMoeStartupCode::Ready,
            ModelStatus::WaitingForBroker => {
                if wait_metadata
                    .and_then(|metadata| metadata.conflict.as_ref())
                    .is_some()
                {
                    SsdMoeStartupCode::BrokerBusy
                } else {
                    SsdMoeStartupCode::WaitingForBroker
                }
            }
            _ if launch_probe.infer_bin.is_none() => SsdMoeStartupCode::InferBinaryMissing,
            _ if launch_probe.model_path.is_none() => SsdMoeStartupCode::ModelLayoutMissing,
            _ if broker_probe.health.is_none() => SsdMoeStartupCode::BrokerUnavailable,
            _ => SsdMoeStartupCode::Ready,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn startup_detail(
        code: &SsdMoeStartupCode,
        status: &ModelStatus,
        active_model: Option<&ModelSpec>,
        launch_probe: &SsdMoeLaunchProbeSummary,
        broker_probe: &SsdMoeBrokerProbeSummary,
        active_instance: Option<&SsdMoeBrokerInstanceSummary>,
        acquire_metadata: Option<&SsdMoeAcquireMetadata>,
        wait_metadata: Option<&SsdMoeWaitMetadata>,
    ) -> String {
        if active_model.is_some_and(|model| {
            matches!(
                model.runtime_backend,
                LocalRuntimeBackend::TurboHeroOracle { .. }
            )
        }) {
            return match code {
                SsdMoeStartupCode::Ready => match status {
                    ModelStatus::Running => {
                        let metadata_summary = launch_probe
                            .model_source_label
                            .as_ref()
                            .map(|label| format!(" via {label}"))
                            .unwrap_or_default();
                        format!(
                            "local TurboHero oracle online at {}{}",
                            default_local_base_url(
                                active_model.expect("checked above").default_port()
                            ),
                            metadata_summary
                        )
                    }
                    ModelStatus::Starting => format!(
                        "launching local TurboHero oracle at {}",
                        default_local_base_url(active_model.expect("checked above").default_port())
                    ),
                    ModelStatus::Ready => format!(
                        "local TurboHero oracle ready to launch at {}",
                        default_local_base_url(active_model.expect("checked above").default_port())
                    ),
                    ModelStatus::Stopping => "local TurboHero oracle is stopping".to_string(),
                    ModelStatus::Failed(message) => message.clone(),
                    _ => "local TurboHero oracle available".to_string(),
                },
                SsdMoeStartupCode::InferBinaryMissing => launch_probe
                    .failure
                    .clone()
                    .unwrap_or_else(|| "TurboHero Python launcher is unavailable".to_string()),
                SsdMoeStartupCode::ModelLayoutMissing => launch_probe
                    .failure
                    .clone()
                    .unwrap_or_else(|| "TurboHero model snapshot is incomplete".to_string()),
                _ => launch_probe
                    .failure
                    .clone()
                    .unwrap_or_else(|| "local TurboHero oracle is unavailable".to_string()),
            };
        }
        match code {
            SsdMoeStartupCode::Ready => match status {
                ModelStatus::Running => {
                    if let Some(metadata) = acquire_metadata {
                        format!(
                            "{} at {} · instance {} · {} lease(s)",
                            acquire_disposition_label(&metadata.disposition),
                            metadata.base_url,
                            metadata.instance_id,
                            metadata.lease_count
                        )
                    } else {
                        "local loopback runtime ready".to_string()
                    }
                }
                ModelStatus::Starting => "attach-or-spawn in progress".to_string(),
                ModelStatus::Ready => {
                    "valid model prepared; waiting for runtime health".to_string()
                }
                ModelStatus::Stopping => "runtime is stopping".to_string(),
                ModelStatus::NotDownloaded => "selected model is not downloaded".to_string(),
                ModelStatus::Downloading { progress_pct, .. } => {
                    format!("Downloading {:.1}%", progress_pct)
                }
                ModelStatus::Packing {
                    layer,
                    total_layers,
                } => format!("Packing {layer}/{total_layers}"),
                ModelStatus::Failed(message) => message.clone(),
                ModelStatus::WaitingForBroker => {
                    "waiting for a compatible shared runtime".to_string()
                }
            },
            SsdMoeStartupCode::InferBinaryMissing => {
                let checked = format_checked_paths(&launch_probe.checked_infer_paths);
                if let Some(instance) = active_instance {
                    format!(
                        "could not resolve an SSD-MOE infer binary for Quorp; checked {checked}. A healthy shared runtime exists at {} using {} with {} lease(s), but Quorp only spawns from its own models root or QUORP_SSD_MOE_INFER_BIN",
                        instance.base_url,
                        instance.infer_bin.display(),
                        instance.lease_count
                    )
                } else if broker_probe.health.is_some() {
                    format!(
                        "could not resolve an SSD-MOE infer binary for Quorp; checked {checked}. The shared broker is reachable at {}",
                        broker_probe
                            .broker_url
                            .clone()
                            .unwrap_or_else(|| "the configured loopback broker".to_string())
                    )
                } else {
                    format!(
                        "could not resolve an SSD-MOE infer binary for Quorp; checked {checked}. Install or point QUORP_SSD_MOE_INFER_BIN at a Quorp-owned infer binary"
                    )
                }
            }
            SsdMoeStartupCode::ModelLayoutMissing => launch_probe
                .failure
                .clone()
                .unwrap_or_else(|| "SSD-MOE model layout is incomplete".to_string()),
            SsdMoeStartupCode::BrokerUnavailable => {
                let broker_message = broker_probe
                    .probe_error
                    .clone()
                    .unwrap_or_else(|| "shared broker is unavailable".to_string());
                format!(
                    "{}; broker binary expected at {}",
                    broker_message,
                    broker_probe.expected_binary_path.display()
                )
            }
            SsdMoeStartupCode::BrokerBusy => {
                let base = wait_metadata
                    .map(|metadata| metadata.message.clone())
                    .unwrap_or_else(|| {
                        "another app is holding an incompatible shared runtime".to_string()
                    });
                if let Some(instance) = active_instance {
                    format!(
                        "{}; active instance {} at {} with {} lease(s)",
                        base, instance.instance_id, instance.base_url, instance.lease_count
                    )
                } else {
                    base
                }
            }
            SsdMoeStartupCode::WaitingForBroker => wait_metadata
                .map(|metadata| metadata.message.clone())
                .unwrap_or_else(|| "waiting for a compatible shared runtime".to_string()),
        }
    }

    fn probe_launch_inputs(model: &ModelSpec) -> SsdMoeLaunchProbeSummary {
        if let LocalRuntimeBackend::TurboHeroOracle {
            profile_key, mode, ..
        } = model.runtime_backend
        {
            let python = Self::turbohero_python_command();
            let script = Self::turbohero_oracle_script_path();
            let mut checked_infer_paths =
                vec![canonicalize_lenient(&python), canonicalize_lenient(&script)];
            let snapshot = Self::turbohero_snapshot_path(model).ok();
            if let Some(snapshot) = snapshot.as_ref() {
                checked_infer_paths.push(canonicalize_lenient(snapshot));
            }
            let failure = if !python.exists() {
                Some(format!(
                    "TurboHero Python interpreter is missing at {}",
                    python.display()
                ))
            } else if !script.is_file() {
                Some(format!(
                    "TurboHero oracle entrypoint is missing at {}",
                    script.display()
                ))
            } else if snapshot.is_none() {
                Some(format!(
                    "TurboHero snapshot missing for {} under {}",
                    model.hf_repo,
                    Self::runtime_models_dir().join("huggingface").display()
                ))
            } else {
                None
            };
            return SsdMoeLaunchProbeSummary {
                checked_infer_paths,
                infer_bin: python.exists().then_some(python),
                model_path: snapshot,
                weights_path: None,
                manifest_path: None,
                model_source_label: Some(format!("turbohero {profile_key} ({mode})")),
                failure,
            };
        }
        let request = match Self::runtime_discovery_request(model) {
            Ok(request) => request,
            Err(error) => {
                return SsdMoeLaunchProbeSummary {
                    checked_infer_paths: Vec::new(),
                    infer_bin: None,
                    model_path: None,
                    weights_path: None,
                    manifest_path: None,
                    model_source_label: None,
                    failure: Some(error.to_string()),
                };
            }
        };
        let checked_infer_paths = infer_checked_paths(&request);
        let infer_bin = checked_infer_paths
            .iter()
            .find(|candidate| candidate.is_file())
            .cloned();
        let (model_path, weights_path, manifest_path, model_source_label, failure) =
            match resolve_model_layout_summary(&request) {
                Ok(summary) => (
                    Some(summary.model_path),
                    summary.weights_path,
                    summary.manifest_path,
                    Some(runtime_model_source_label(&summary.model_source).to_string()),
                    None,
                ),
                Err(error) => (None, None, None, None, Some(error.to_string())),
            };

        SsdMoeLaunchProbeSummary {
            checked_infer_paths,
            infer_bin,
            model_path,
            weights_path,
            manifest_path,
            model_source_label,
            failure,
        }
    }

    fn probe_broker_state() -> SsdMoeBrokerProbeSummary {
        let config = Self::broker_client_config();
        let installation_status = broker_installation_status(&config);
        let broker_url = running_broker_url(&config);
        let mut summary = SsdMoeBrokerProbeSummary {
            state_dir: installation_status.state_dir.clone(),
            expected_binary_path: installation_status.expected_binary_path.clone(),
            installed_binary_exists: installation_status.installed_binary_exists,
            broker_url: broker_url.clone(),
            health: None,
            instances: Vec::new(),
            probe_error: None,
        };
        let Some(broker_url) = broker_url else {
            summary.probe_error = Some("shared broker is not running".to_string());
            return summary;
        };
        match http_probe_json::<BrokerHealth>(
            &format!("{broker_url}/v1/health"),
            Duration::from_millis(300),
        ) {
            Ok(health) => {
                summary.health = Some(health);
            }
            Err(error) => {
                summary.probe_error = Some(format!("probing broker health: {error}"));
                return summary;
            }
        }
        match http_probe_json::<Vec<BrokerInstanceRecord>>(
            &format!("{broker_url}/v1/instances"),
            Duration::from_millis(300),
        ) {
            Ok(instances) => {
                summary.instances = instances
                    .into_iter()
                    .map(SsdMoeBrokerInstanceSummary::from)
                    .collect();
            }
            Err(error) => {
                summary.probe_error = Some(format!("probing broker instances: {error}"));
            }
        }
        summary
    }

    #[allow(dead_code)]
    fn format_discovery_failure(error: &RuntimeDiscoveryFailure) -> String {
        error.to_string()
    }

    fn format_layout_failure(
        inspection: &PreparedModelInspection,
        failure: SsdMoeStartupFailure,
    ) -> String {
        match failure {
            SsdMoeStartupFailure::ModelLayout(prefix) => format!(
                "{prefix} at {:?}; missing {}",
                inspection.model_path,
                inspection.missing_paths.join(", ")
            ),
            SsdMoeStartupFailure::Setup(message) => message,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn latest_meaningful_server_error(logs: &VecDeque<String>) -> Option<String> {
        logs.iter().rev().find_map(|line| {
            let trimmed = line.trim();
            if let Some(message) = trimmed.strip_prefix("[stderr] FATAL: ") {
                return Some(message.to_string());
            }
            if let Some(message) = trimmed.strip_prefix("[stderr] ")
                && !message.is_empty()
            {
                return Some(message.to_string());
            }
            None
        })
    }

    fn record_waiting_for_broker(&self, model: &ModelSpec, error: &BrokerErrorResponse) {
        let mut state = self.inner.lock().expect("ssd-moe runtime lock");
        state.wait_metadata = Some(SsdMoeWaitMetadata {
            message: error.message.clone(),
            conflict: error.conflict.clone(),
        });
        state.next_retry_at = Some(Instant::now() + Duration::from_millis(500));
        state.pending_retry_model = Some(model.clone());
        self.push_transition(
            &mut state,
            &ModelStatus::WaitingForBroker,
            format!("broker conflict: {}", error.message),
            None,
        );
    }

    #[allow(dead_code)]
    fn acquire_runtime_launch(
        &self,
        model: &ModelSpec,
        resolved_launch: &ssd_moe_launch::ResolvedRuntimeLaunch,
    ) {
        let log_path = self
            .inner
            .lock()
            .expect("ssd-moe runtime lock")
            .log_file_path
            .clone();
        let launch = LaunchSpec {
            launch_kind: ssd_moe_contract::LaunchKind::FlashMoeInfer,
            infer_bin: resolved_launch.infer_bin.clone(),
            working_dir: Self::launch_working_dir(resolved_launch),
            command_args: Vec::new(),
            model_path: resolved_launch.model_layout.model_path.clone(),
            weights_path: resolved_launch.model_layout.weights_path.clone(),
            manifest_path: resolved_launch.model_layout.manifest_path.clone(),
            tokenizer_path: None,
            host: "127.0.0.1".to_string(),
            requested_port: None,
            k: model.k_experts().max(8),
            think_budget: model.think_budget(),
            kv_seq: model.kv_seq(),
            serve_mode: "openai".to_string(),
            api_version: "v1".to_string(),
            extra_env: BTreeMap::new(),
            log_path: Some(log_path),
        };
        let fingerprint = match build_fingerprint(&launch) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                let message = format!("Failed to fingerprint SSD-MOE launch: {error}");
                let mut state = self.inner.lock().expect("ssd-moe runtime lock");
                self.push_transition(
                    &mut state,
                    &ModelStatus::Failed(message.clone()),
                    message.clone(),
                    None,
                );
                drop(state);
                self.append_log_line(message);
                return;
            }
        };
        let request = BrokerAcquireRequest {
            requested: RequestedInstance {
                app_name: "quorp".to_string(),
                fingerprint,
                launch: launch.clone(),
                lease_ttl_secs: 45,
            },
            policy: BrokerAcquirePolicy::default(),
        };
        let config = ClientConfig {
            ..Self::broker_client_config()
        };
        {
            let mut state = self.inner.lock().expect("ssd-moe runtime lock");
            state.wait_metadata = None;
            state.next_retry_at = None;
            state.pending_retry_model = None;
            self.push_transition(
                &mut state,
                &ModelStatus::Starting,
                format!("acquire runtime requested for {}", model.id),
                None,
            );
        }
        self.append_log_line(format!(
            "Acquiring shared SSD-MOE runtime for {} via {}",
            model.id,
            runtime_model_source_label(&resolved_launch.model_source)
        ));
        match acquire_managed_blocking_detailed(request, &config) {
            Ok((response, lease)) => {
                if let Some(pid) = response.instance.pid {
                    crate::quorp::memory_fingerprint::register_managed_pid(pid);
                }
                let port = reqwest::Url::parse(&response.base_url)
                    .ok()
                    .and_then(|url| url.port())
                    .unwrap_or(self.port());
                let mut state = self.inner.lock().expect("ssd-moe runtime lock");
                state.port = port;
                state.base_url = Some(format!("{}/v1", response.base_url));
                state.acquire_metadata = Some(SsdMoeAcquireMetadata {
                    broker_url: response.broker_url.clone(),
                    base_url: response.base_url.clone(),
                    instance_id: response.instance.instance_id.clone(),
                    disposition: response.disposition.clone(),
                    lease_count: response.instance.lease_count,
                    model_path: response.runtime_metadata.model_path.clone(),
                    weights_path: response.runtime_metadata.weights_path.clone(),
                    manifest_path: response.runtime_metadata.manifest_path.clone(),
                    stale: false,
                });
                state.wait_metadata = None;
                state.lease = Some(lease);
                self.push_transition(
                    &mut state,
                    &ModelStatus::Running,
                    format!(
                        "broker attached runtime via {}",
                        acquire_disposition_label(&response.disposition)
                    ),
                    Some(true),
                );
                drop(state);
                self.append_log_line(format!(
                    "Broker attached SSD-MOE at {} ({})",
                    response.base_url,
                    acquire_disposition_label(&response.disposition)
                ));
            }
            Err(AcquireManagedError::BrokerConflict(error)) => {
                self.record_waiting_for_broker(model, &error);
                self.append_log_line(format!("Broker waiting: {}", error.message));
            }
            Err(AcquireManagedError::Other(message)) => {
                let full_message = format!("Failed to acquire SSD-MOE through broker: {message}");
                let mut state = self.inner.lock().expect("ssd-moe runtime lock");
                state.wait_metadata = None;
                state.next_retry_at = None;
                state.pending_retry_model = None;
                self.push_transition(
                    &mut state,
                    &ModelStatus::Failed(full_message.clone()),
                    full_message.clone(),
                    None,
                );
                drop(state);
                self.append_log_line(full_message);
            }
        }
    }

    fn launch_working_dir(resolved_launch: &ssd_moe_launch::ResolvedRuntimeLaunch) -> PathBuf {
        let inferred_working_dir = resolved_launch.working_dir.clone();
        if flash_moe_runtime_assets_ready(&inferred_working_dir) {
            return inferred_working_dir;
        }
        let models_dir = Self::runtime_models_dir();
        let shared_flash_moe_dir = models_dir.join("flash-moe");
        if canonicalize_lenient(&resolved_launch.infer_bin)
            == canonicalize_lenient(&models_dir.join("bin/infer"))
            && flash_moe_runtime_assets_ready(&shared_flash_moe_dir)
        {
            return shared_flash_moe_dir;
        }
        inferred_working_dir
    }

    #[allow(dead_code)]
    fn spawn_turbohero_runtime(
        &self,
        project_root: &Path,
        model: &ModelSpec,
    ) -> Result<(), String> {
        let snapshot_path = Self::turbohero_snapshot_path(model)?;
        let Some(profile_key) = model.turbohero_profile_key() else {
            return Err(format!("missing TurboHero profile for {}", model.id));
        };
        let Some(mode) = model.turbohero_mode() else {
            return Err(format!("missing TurboHero mode for {}", model.id));
        };
        let Some(served_model_id) = model.served_model_id() else {
            return Err(format!(
                "missing TurboHero served model id for {}",
                model.id
            ));
        };
        let python = Self::turbohero_python_command();
        let turbohero_root = Self::turbohero_root();
        let script_path = Self::turbohero_oracle_script_path();
        if !python.exists() {
            return Err(format!(
                "TurboHero Python interpreter is missing at {}",
                python.display()
            ));
        }
        if !script_path.is_file() {
            return Err(format!(
                "TurboHero oracle entrypoint is missing at {}",
                script_path.display()
            ));
        }

        let port = model.default_port();
        let base_url = default_local_base_url(port);
        let mut command = Command::new(&python);
        command
            .arg("-m")
            .arg("turbohero.mlx_oracle_server")
            .arg("--profile-key")
            .arg(profile_key)
            .arg("--mode")
            .arg(mode)
            .arg("--model-path")
            .arg(&snapshot_path)
            .arg("--served-model-id")
            .arg(served_model_id)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("--max-ctx")
            .arg(model.kv_seq().to_string())
            .arg("--max-output-tokens")
            .arg(model.max_output_tokens().unwrap_or(512).to_string());
        if model.trust_remote_code() {
            command.arg("--trust-remote-code");
        }
        command.current_dir(&turbohero_root);
        command.env("HF_HOME", Self::runtime_models_dir().join("huggingface"));
        command.env("PYTHONUNBUFFERED", "1");
        command.env("QUORP_LAUNCHED_BY", "quorp");
        Self::configure_setup_command_stdio(&mut command);

        {
            let mut state = self.inner.lock().expect("ssd-moe runtime lock");
            state.project_root = Some(project_root.to_path_buf());
            state.port = port;
            state.base_url = Some(base_url.clone());
            state.acquire_metadata = None;
            state.wait_metadata = None;
            state.next_retry_at = None;
            state.pending_retry_model = None;
            state.runtime_metadata = None;
            self.push_transition(
                &mut state,
                &ModelStatus::Starting,
                format!("launching TurboHero oracle for {}", model.id),
                None,
            );
            state.runtime_process = None;
        }

        self.append_log_line(format!(
            "Launching TurboHero oracle for {} via {} in {}",
            model.id,
            python.display(),
            turbohero_root.display()
        ));

        let mut child = Self::spawn_setup_command(&mut command)
            .map_err(|error| format!("Failed to spawn TurboHero oracle: {error}"))?;
        if let Some(stdout) = child.stdout.take() {
            let runtime = self.clone();
            std::thread::spawn(move || {
                let reader = std::io::BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    runtime.append_log_line(format!("[turbohero stdout] {line}"));
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            let runtime = self.clone();
            std::thread::spawn(move || {
                let reader = std::io::BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    runtime.append_log_line(format!("[turbohero stderr] {line}"));
                }
            });
        }
        let mut state = self.inner.lock().expect("ssd-moe runtime lock");
        state.runtime_process = Some(child);
        Ok(())
    }

    fn attach_turbohero_runtime_if_listening(
        &self,
        project_root: &Path,
        model: &ModelSpec,
    ) -> TurboHeroAttachState {
        let LocalRuntimeBackend::TurboHeroOracle { .. } = model.runtime_backend else {
            return TurboHeroAttachState::NotListening;
        };
        let probe = self.probe_turbohero_listener(model);
        let (healthy, detail, metadata) = match probe {
            TurboHeroAttachState::NotListening => return TurboHeroAttachState::NotListening,
            TurboHeroAttachState::WrongListener(message) => {
                return TurboHeroAttachState::WrongListener(message);
            }
            TurboHeroAttachState::AttachedRunning(detail) => (true, detail, None),
            TurboHeroAttachState::AttachedStarting(detail) => (false, detail, None),
        };
        let port = model.default_port();

        let mut state = self.inner.lock().expect("ssd-moe runtime lock");
        state.port = port;
        state.active_model = Some(model.clone());
        state.project_root = Some(project_root.to_path_buf());
        state.base_url = Some(default_local_base_url(port));
        state.acquire_metadata = None;
        state.wait_metadata = None;
        state.next_retry_at = None;
        state.pending_retry_model = None;
        state.setup_process = None;
        state.runtime_metadata = metadata.or_else(|| self.refresh_runtime_metadata(port));
        if let Some(pid) = state
            .runtime_metadata
            .as_ref()
            .and_then(|runtime| runtime.get("pid"))
            .and_then(|value| value.as_u64())
        {
            crate::quorp::memory_fingerprint::register_managed_pid(pid as u32);
        }
        let next_status = if healthy {
            ModelStatus::Running
        } else {
            ModelStatus::Starting
        };
        if state.status != next_status
            || state.last_transition_reason.as_deref() != Some(detail.as_str())
        {
            self.push_transition(&mut state, &next_status, detail.clone(), Some(true));
        } else {
            state.status = next_status;
            state.last_transition_reason = Some(detail.clone());
        }
        if healthy {
            TurboHeroAttachState::AttachedRunning(detail)
        } else {
            TurboHeroAttachState::AttachedStarting(detail)
        }
    }

    fn refresh_runtime_metadata(&self, port: u16) -> Option<serde_json::Value> {
        http_probe_json::<serde_json::Value>(
            &Self::runtime_metadata_url(port),
            Duration::from_millis(500),
        )
        .ok()
    }

    fn terminate_child_process(child: &mut Child) -> Result<(), String> {
        #[cfg(unix)]
        {
            let pid = child.id() as i32;
            let signal_result = unsafe { libc::kill(pid, libc::SIGTERM) };
            if signal_result != 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            child.kill().map_err(|error| error.to_string())
        }
    }

    fn stop_child_process(child: &mut Child) {
        let _ = Self::terminate_child_process(child);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() >= deadline => break,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(error) => {
                    log::warn!("tui: failed while waiting for child exit: {error}");
                    break;
                }
            }
        }
        if let Err(error) = child.kill() {
            log::warn!("tui: failed to force-stop child process: {error}");
        }
        let _ = child.wait();
    }

    pub fn ensure_running(&self, project_root: &Path, model: &ModelSpec) {
        let already_active = self
            .active_model()
            .is_some_and(|active| active.id == model.id)
            && matches!(self.status(), ModelStatus::Running)
            && self.tcp_probe();
        if already_active {
            return;
        }

        if matches!(
            model.runtime_backend,
            LocalRuntimeBackend::FlashMoePrepared { .. }
        ) && let Ok(request) = Self::runtime_discovery_request(model)
        {
            let inspection = inspect_prepared_model_layout(&request.weights_dir);
            if inspection.status != PreparedModelStatus::Ready
                && Self::resolve_setup_script_path().is_some()
            {
                self.start_model_setup(project_root, model, &inspection);
                return;
            }
        }

        if matches!(
            model.runtime_backend,
            LocalRuntimeBackend::TurboHeroOracle { .. }
        ) {
            match self.attach_turbohero_runtime_if_listening(project_root, model) {
                TurboHeroAttachState::AttachedRunning(_)
                | TurboHeroAttachState::AttachedStarting(_) => return,
                TurboHeroAttachState::WrongListener(message) => {
                    let mut state = self.inner.lock().expect("ssd-moe runtime lock");
                    state.active_model = Some(model.clone());
                    state.project_root = Some(project_root.to_path_buf());
                    state.wait_metadata = None;
                    state.acquire_metadata = None;
                    self.push_transition(
                        &mut state,
                        &ModelStatus::Failed(message.clone()),
                        message.clone(),
                        Some(true),
                    );
                    drop(state);
                    self.append_log_line(message);
                    return;
                }
                TurboHeroAttachState::NotListening => {}
            }

            self.stop();
            if let Err(message) = self.spawn_turbohero_runtime(project_root, model) {
                let mut state = self.inner.lock().expect("ssd-moe runtime lock");
                state.active_model = Some(model.clone());
                state.project_root = Some(project_root.to_path_buf());
                state.wait_metadata = None;
                state.acquire_metadata = None;
                self.push_transition(
                    &mut state,
                    &ModelStatus::Failed(message.clone()),
                    message.clone(),
                    Some(false),
                );
                drop(state);
                self.append_log_line(message);
            }
            return;
        }

        self.stop();
        {
            let mut state = self.inner.lock().expect("ssd-moe runtime lock");
            state.active_model = Some(model.clone());
            state.project_root = Some(project_root.to_path_buf());
            state.wait_metadata = None;
            self.push_transition(
                &mut state,
                &ModelStatus::Starting,
                format!("requesting broker runtime for {}", model.id),
                None,
            );
        }

        let request = RuntimeLoadRequest {
            model_id: model.id.to_string(),
            app_name: "quorp".to_string(),
            allow_experimental: false,
        };
        match load_runtime_managed_blocking(&request, &Self::broker_client_config()) {
            Ok((response, lease)) => {
                let mut state = self.inner.lock().expect("ssd-moe runtime lock");
                let port = reqwest::Url::parse(&response.acquire.base_url)
                    .ok()
                    .and_then(|url| url.port())
                    .unwrap_or(self.port());
                state.port = port;
                state.base_url = Some(format!("{}/v1", response.acquire.base_url));
                state.acquire_metadata = Some(SsdMoeAcquireMetadata {
                    broker_url: response.acquire.broker_url.clone(),
                    base_url: response.acquire.base_url.clone(),
                    instance_id: response.acquire.instance.instance_id.clone(),
                    disposition: response.acquire.disposition.clone(),
                    lease_count: response.acquire.instance.lease_count,
                    model_path: response.acquire.runtime_metadata.model_path.clone(),
                    weights_path: response.acquire.runtime_metadata.weights_path.clone(),
                    manifest_path: response.acquire.runtime_metadata.manifest_path.clone(),
                    stale: false,
                });
                state.lease = Some(lease);
                state.runtime_metadata = http_probe_json::<serde_json::Value>(
                    &format!("{}/v1/runtime", response.acquire.base_url),
                    Duration::from_secs(2),
                )
                .ok();
                self.push_transition(
                    &mut state,
                    &ModelStatus::Running,
                    format!(
                        "broker loaded {} via {}",
                        response.model_id,
                        acquire_disposition_label(&response.acquire.disposition)
                    ),
                    Some(true),
                );
            }
            Err(error) => {
                let message = format!("failed to load broker runtime for {}: {error}", model.id);
                let mut state = self.inner.lock().expect("ssd-moe runtime lock");
                self.push_transition(
                    &mut state,
                    &ModelStatus::Failed(message.clone()),
                    message.clone(),
                    None,
                );
                drop(state);
                self.append_log_line(message);
            }
        }
    }

    #[allow(dead_code)]
    fn start_model_setup(
        &self,
        _project_root: &Path,
        model: &ModelSpec,
        inspection: &PreparedModelInspection,
    ) {
        let model_weights_path = inspection.model_path.as_path();
        let Some(setup_script) = Self::resolve_setup_script_path() else {
            let message = Self::format_layout_failure(
                inspection,
                SsdMoeStartupFailure::Setup(
                    "SSD-MOE model directory is incomplete and setup script is missing".to_string(),
                ),
            );
            let mut state = self.inner.lock().expect("ssd-moe runtime lock");
            self.push_transition(
                &mut state,
                &ModelStatus::Failed(message.clone()),
                message.clone(),
                None,
            );
            drop(state);
            self.append_log_line(message);
            return;
        };

        let mut command = Command::new(Self::python_command());
        command
            .arg(&setup_script)
            .arg("--repo")
            .arg(model.hf_repo)
            .arg("--output")
            .arg(model_weights_path);
        Self::configure_setup_command_stdio(&mut command);

        {
            let mut state = self.inner.lock().expect("ssd-moe runtime lock");
            self.push_transition(
                &mut state,
                &ModelStatus::Downloading {
                    progress_pct: 0.0,
                    downloaded_gb: 0.0,
                    total_gb: model.estimated_disk_gb,
                },
                format!("starting automatic setup for {}", model.id),
                None,
            );
            state.setup_process = None;
            state.logs.clear();
        }
        self.append_log_line(format!(
            "Preparing {} at {:?}; status {:?}; missing {}",
            model.id,
            model_weights_path,
            inspection.status,
            inspection.missing_paths.join(", ")
        ));
        self.append_log_line(format!("Starting automatic setup from {}", model.hf_repo));

        match Self::spawn_setup_command(&mut command) {
            Ok(mut child) => {
                if let Some(stdout) = child.stdout.take() {
                    let runtime = self.clone();
                    let total_gb = model.estimated_disk_gb;
                    std::thread::spawn(move || {
                        let reader = std::io::BufReader::new(stdout);
                        for line in reader.lines().map_while(Result::ok) {
                            runtime.update_setup_progress_from_line(&line, total_gb);
                            runtime.append_log_line(format!("[setup stdout] {line}"));
                        }
                    });
                }
                if let Some(stderr) = child.stderr.take() {
                    let runtime = self.clone();
                    let total_gb = model.estimated_disk_gb;
                    std::thread::spawn(move || {
                        let reader = std::io::BufReader::new(stderr);
                        for line in reader.lines().map_while(Result::ok) {
                            runtime.update_setup_progress_from_line(&line, total_gb);
                            runtime.append_log_line(format!("[setup stderr] {line}"));
                        }
                    });
                }
                let mut state = self.inner.lock().expect("ssd-moe runtime lock");
                state.setup_process = Some(child);
            }
            Err(error) => {
                let message = Self::format_layout_failure(
                    inspection,
                    SsdMoeStartupFailure::Setup(format!(
                        "Failed to spawn SSD-MOE setup script: {error}"
                    )),
                );
                let mut state = self.inner.lock().expect("ssd-moe runtime lock");
                self.push_transition(
                    &mut state,
                    &ModelStatus::Failed(message.clone()),
                    message.clone(),
                    None,
                );
                drop(state);
                self.append_log_line(message);
            }
        }
    }

    #[allow(dead_code)]
    fn update_setup_progress_from_line(&self, line: &str, total_gb: f32) {
        let trimmed = line.trim();
        let mut state = self.inner.lock().expect("ssd-moe runtime lock");
        if let Some(progress_pct) = parse_extract_progress(trimmed) {
            self.push_transition(
                &mut state,
                &ModelStatus::Downloading {
                    progress_pct,
                    downloaded_gb: total_gb * (progress_pct / 100.0),
                    total_gb,
                },
                format!("setup progress {:.1}%", progress_pct),
                None,
            );
            return;
        }
        if let Some((layer, total_layers)) = parse_packing_progress(trimmed) {
            self.push_transition(
                &mut state,
                &ModelStatus::Packing {
                    layer,
                    total_layers,
                },
                format!("packing layer {layer}/{total_layers}"),
                None,
            );
            return;
        }
        if trimmed.starts_with("Downloading ") {
            self.push_transition(
                &mut state,
                &ModelStatus::Downloading {
                    progress_pct: 0.0,
                    downloaded_gb: 0.0,
                    total_gb,
                },
                trimmed.to_string(),
                None,
            );
        }
    }

    pub fn poll_health(&self) {
        let mut restart_model = None;
        let mut restart_root = None;
        let mut state = self.inner.lock().expect("ssd-moe runtime lock");
        let port = state.port;
        let turbohero_backend = state.active_model.as_ref().is_some_and(|model| {
            matches!(
                model.runtime_backend,
                LocalRuntimeBackend::TurboHeroOracle { .. }
            )
        });
        match state.status {
            ModelStatus::Downloading { .. } | ModelStatus::Packing { .. } => {
                if let Some(setup_process) = state.setup_process.as_mut()
                    && let Ok(Some(status)) = setup_process.try_wait()
                {
                    let exit_ok = status.success();
                    state.setup_process = None;
                    if exit_ok {
                        let inspection = state.active_model.as_ref().and_then(|model| {
                            Self::runtime_discovery_request(model)
                                .ok()
                                .map(|request| inspect_prepared_model_layout(&request.weights_dir))
                        });
                        if inspection.as_ref().is_some_and(|inspection| {
                            inspection.status == PreparedModelStatus::Ready
                        }) {
                            restart_model = state.active_model.clone();
                            restart_root = state.project_root.clone();
                            self.push_transition(
                                &mut state,
                                &ModelStatus::Ready,
                                "setup completed; waiting to reacquire runtime".to_string(),
                                None,
                            );
                        } else {
                            let message = inspection.map_or_else(
                                || "SSD-MOE model setup completed, but the prepared model directory could not be revalidated".to_string(),
                                |inspection| {
                                    Self::format_layout_failure(
                                        &inspection,
                                        SsdMoeStartupFailure::Setup(
                                            "SSD-MOE model setup completed, but the directory is still incomplete"
                                                .to_string(),
                                        ),
                                    )
                                },
                            );
                            self.push_transition(
                                &mut state,
                                &ModelStatus::Failed(message.clone()),
                                message,
                                None,
                            );
                        }
                    } else {
                        let message =
                            format!("SSD-MOE model setup failed (status {:?})", status.code());
                        self.push_transition(
                            &mut state,
                            &ModelStatus::Failed(message.clone()),
                            message,
                            None,
                        );
                    }
                }
            }
            ModelStatus::Starting => {
                if turbohero_backend {
                    if let Some(runtime_process) = state.runtime_process.as_mut()
                        && let Ok(Some(status)) = runtime_process.try_wait()
                    {
                        state.runtime_process = None;
                        state.base_url = None;
                        state.runtime_metadata = None;
                        let message = format!(
                            "TurboHero oracle exited before becoming healthy (status {:?})",
                            status.code()
                        );
                        self.push_transition(
                            &mut state,
                            &ModelStatus::Failed(message.clone()),
                            message,
                            Some(false),
                        );
                    } else if let Ok(health) = http_probe_json::<serde_json::Value>(
                        &Self::runtime_health_url(port),
                        Duration::from_millis(300),
                    ) && health.get("status").and_then(|value| value.as_str())
                        == Some("ok")
                    {
                        state.base_url = Some(default_local_base_url(port));
                        state.runtime_metadata = self.refresh_runtime_metadata(port);
                        if let Some(pid) = state
                            .runtime_metadata
                            .as_ref()
                            .and_then(|runtime| runtime.get("pid"))
                            .and_then(|value| value.as_u64())
                        {
                            crate::quorp::memory_fingerprint::register_managed_pid(pid as u32);
                        }
                        self.push_transition(
                            &mut state,
                            &ModelStatus::Running,
                            format!(
                                "TurboHero oracle healthy at {}",
                                default_local_base_url(port)
                            ),
                            Some(true),
                        );
                    }
                }
            }
            ModelStatus::WaitingForBroker => {
                if state
                    .next_retry_at
                    .is_some_and(|next_retry_at| Instant::now() >= next_retry_at)
                {
                    restart_model = state.pending_retry_model.clone();
                    restart_root = state.project_root.clone();
                    state.next_retry_at = None;
                }
            }
            ModelStatus::Running => {
                let tcp_probe_ok = Self::tcp_probe_port(port);
                if !tcp_probe_ok {
                    if turbohero_backend {
                        state.base_url = None;
                        state.runtime_metadata = None;
                        if let Some(runtime_process) = state.runtime_process.as_mut()
                            && let Ok(Some(_)) = runtime_process.try_wait()
                        {
                            state.runtime_process = None;
                        }
                        self.push_transition(
                            &mut state,
                            &ModelStatus::Ready,
                            format!("TurboHero oracle health probe failed on port {port}"),
                            Some(false),
                        );
                    } else {
                        if let Some(metadata) = state.acquire_metadata.as_mut() {
                            metadata.stale = true;
                        }
                        state.base_url = None;
                        state.wait_metadata = None;
                        state.lease = None;
                        self.push_transition(
                            &mut state,
                            &ModelStatus::Ready,
                            format!(
                                "runtime health probe failed on port {port}; preserving stale broker metadata for recovery"
                            ),
                            Some(false),
                        );
                    }
                } else if turbohero_backend {
                    state.runtime_metadata = self.refresh_runtime_metadata(port);
                }
            }
            _ => {}
        }
        drop(state);
        if let (Some(project_root), Some(model)) = (restart_root, restart_model) {
            self.ensure_running(project_root.as_path(), &model);
        }
    }

    pub fn wait_until_ready(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut latest_turbohero_detail = None;
        loop {
            let turbohero_context = {
                let state = self.inner.lock().expect("ssd-moe runtime lock");
                match (state.project_root.clone(), state.active_model.clone()) {
                    (Some(project_root), Some(model))
                        if matches!(
                            model.runtime_backend,
                            LocalRuntimeBackend::TurboHeroOracle { .. }
                        ) =>
                    {
                        Some((project_root, model))
                    }
                    _ => None,
                }
            };
            if let Some((project_root, model)) = turbohero_context {
                match self.attach_turbohero_runtime_if_listening(&project_root, &model) {
                    TurboHeroAttachState::AttachedRunning(detail)
                    | TurboHeroAttachState::AttachedStarting(detail) => {
                        latest_turbohero_detail = Some(detail);
                    }
                    TurboHeroAttachState::WrongListener(message) => return Err(message),
                    TurboHeroAttachState::NotListening => {}
                }
            }
            self.poll_health();
            match self.status() {
                ModelStatus::Running => return Ok(()),
                ModelStatus::Failed(message) => return Err(message),
                ModelStatus::WaitingForBroker => {
                    if Instant::now() >= deadline {
                        let message = self
                            .wait_metadata()
                            .map(|wait| wait.message)
                            .unwrap_or_else(|| {
                                "Timed out waiting for a compatible shared SSD-MOE runtime"
                                    .to_string()
                            });
                        return Err(format!("{message} (timeout after {:?})", timeout));
                    }
                }
                ModelStatus::Downloading { progress_pct, .. } => {
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "SSD-MOE model download still in progress ({progress_pct:.1}%)"
                        ));
                    }
                }
                ModelStatus::Packing {
                    layer,
                    total_layers,
                } => {
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "SSD-MOE model packing still in progress ({layer}/{total_layers})"
                        ));
                    }
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                if let Some(detail) = latest_turbohero_detail {
                    return Err(format!("{detail} (timeout after {:?})", timeout));
                }
                return Err(format!(
                    "SSD-MOE server did not become ready on port {} within {:?}",
                    self.port(),
                    timeout
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn probe_turbohero_listener(&self, model: &ModelSpec) -> TurboHeroAttachState {
        let port = model.default_port();
        if !Self::tcp_probe_port(port) {
            return TurboHeroAttachState::NotListening;
        }
        let expected_model_id = model.served_model_id().unwrap_or_default();
        let base_url = default_local_base_url(port);
        let health = http_probe_json::<serde_json::Value>(
            &Self::runtime_health_url(port),
            Duration::from_millis(500),
        )
        .ok();
        let runtime_metadata = self.refresh_runtime_metadata(port);
        let observed_model_id = runtime_metadata
            .as_ref()
            .and_then(Self::observed_turbohero_model_id)
            .or_else(|| health.as_ref().and_then(Self::observed_turbohero_model_id));
        if let Some(observed_model_id) = observed_model_id
            && observed_model_id != expected_model_id
        {
            return TurboHeroAttachState::WrongListener(format!(
                "port {port} is already serving `{observed_model_id}` instead of the requested `{expected_model_id}`"
            ));
        }
        let metadata = runtime_metadata.or(health);
        let healthy = metadata
            .as_ref()
            .and_then(|value| value.get("status"))
            .and_then(|value| value.as_str())
            == Some("ok");
        let detail = if healthy {
            format!("TurboHero oracle already healthy at {base_url}")
        } else if metadata.is_some() {
            format!(
                "TurboHero oracle is listening at {base_url} and serving `{expected_model_id}`, but runtime metadata is still warming"
            )
        } else {
            format!(
                "port {port} is accepting TCP connections, but `/health` and `/v1/runtime` are not ready yet; another process may be holding the port"
            )
        };
        if healthy {
            TurboHeroAttachState::AttachedRunning(detail)
        } else {
            TurboHeroAttachState::AttachedStarting(detail)
        }
    }

    fn observed_turbohero_model_id(value: &serde_json::Value) -> Option<&str> {
        ["served_model_id", "model_id", "model"]
            .iter()
            .find_map(|key| value.get(*key).and_then(|item| item.as_str()))
    }

    pub fn switch_model(&self, project_root: &Path, new_model: &ModelSpec) {
        if let Err(error) = model_registry::save_model(new_model.id) {
            log::error!(
                "tui: failed to persist switched SSD-MOE model {:?}: {}",
                new_model.id,
                error
            );
        }
        let already_active = self
            .active_model()
            .is_some_and(|current| current.id == new_model.id)
            && self.tcp_probe();
        if already_active {
            let mut state = self.inner.lock().expect("ssd-moe runtime lock");
            state.active_model = Some(new_model.clone());
            self.push_transition(
                &mut state,
                &ModelStatus::Running,
                format!("model {} already active", new_model.id),
                Some(true),
            );
            return;
        }
        self.stop();
        self.ensure_running(project_root, new_model);
    }

    #[allow(dead_code)]
    pub fn switch_role(&self, project_root: &Path, role: LocalModelRole) {
        let Some(model_id) = model_registry::preferred_local_model_id_for_role(role) else {
            log::error!(
                "tui: no preferred SSD-MOE model configured for role {:?}",
                role
            );
            return;
        };
        let Some(new_model) = model_registry::local_moe_spec_for_registry_id(&model_id) else {
            log::error!(
                "tui: preferred SSD-MOE model {:?} for role {:?} is not available in the registry",
                model_id,
                role
            );
            return;
        };
        if let Err(error) = model_registry::save_active_heavy_role(role) {
            log::error!(
                "tui: failed to persist active heavy role {:?}: {}",
                role,
                error
            );
        }
        self.switch_model(project_root, &new_model);
    }

    pub fn stop(&self) {
        let mut state = self.inner.lock().expect("ssd-moe runtime lock");
        self.push_transition(
            &mut state,
            &ModelStatus::Stopping,
            "stop requested".to_string(),
            None,
        );
        if let Some(lease) = state.lease.take() {
            lease.release();
        }
        if let Some(mut runtime_process) = state.runtime_process.take() {
            drop(state);
            Self::stop_child_process(&mut runtime_process);
            state = self.inner.lock().expect("ssd-moe runtime lock");
        }
        if let Some(mut setup_process) = state.setup_process.take() {
            drop(state);
            Self::stop_child_process(&mut setup_process);
            state = self.inner.lock().expect("ssd-moe runtime lock");
        }
        state.base_url = None;
        state.runtime_metadata = None;
        state.acquire_metadata = None;
        state.wait_metadata = None;
        state.next_retry_at = None;
        state.pending_retry_model = None;
        self.push_transition(
            &mut state,
            &ModelStatus::Ready,
            "stop completed".to_string(),
            None,
        );
    }

    pub(crate) fn set_active_model_for_test(&self, model: Option<ModelSpec>) {
        self.inner
            .lock()
            .expect("ssd-moe runtime lock")
            .active_model = model;
    }
}

#[derive(Debug, Clone)]
struct ModelLayoutSummary {
    model_path: PathBuf,
    weights_path: Option<PathBuf>,
    manifest_path: Option<PathBuf>,
    model_source: RuntimeModelSource,
}

impl From<BrokerInstanceRecord> for SsdMoeBrokerInstanceSummary {
    fn from(record: BrokerInstanceRecord) -> Self {
        Self {
            instance_id: record.instance_id,
            base_url: record.base_url,
            lease_count: record.lease_count,
            healthy: record.healthy,
            infer_bin: record.fingerprint.infer_bin,
            model_path: record.fingerprint.model_path,
            weights_path: record.fingerprint.weights_path,
        }
    }
}

fn canonicalize_lenient(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn infer_checked_paths(request: &RuntimeDiscoveryRequest) -> Vec<PathBuf> {
    let mut checked_paths = Vec::new();
    if let Some(path) = request.infer_bin_override.as_ref() {
        checked_paths.push(canonicalize_lenient(path));
    }
    checked_paths.extend(
        request
            .infer_bin_candidates
            .iter()
            .map(|candidate| canonicalize_lenient(candidate)),
    );
    checked_paths
}

fn resolve_model_layout_summary(
    request: &RuntimeDiscoveryRequest,
) -> Result<ModelLayoutSummary, RuntimeDiscoveryFailure> {
    let prepared_model_dir =
        canonicalize_lenient(&request.models_dir.join(&request.prepared_model_dir_name));
    let prepared_weights_root = canonicalize_lenient(&request.weights_dir);
    let mut checked_paths = Vec::new();

    if let Some(explicit_model_path) = request.model_path_override.as_ref() {
        let candidate = canonicalize_lenient(explicit_model_path);
        checked_paths.push(candidate.clone());
        if let Some(summary) = direct_model_layout_summary(&candidate, &prepared_weights_root) {
            return Ok(ModelLayoutSummary {
                model_source: RuntimeModelSource::ExplicitModelPath,
                ..summary
            });
        }
    }

    checked_paths.push(prepared_model_dir.clone());
    if inspect_prepared_model_layout(&prepared_model_dir).status == PreparedModelStatus::Ready {
        return Ok(ModelLayoutSummary {
            model_path: prepared_model_dir.clone(),
            weights_path: None,
            manifest_path: None,
            model_source: RuntimeModelSource::PreparedModelDir,
        });
    }

    let snapshot_roots = hf_snapshot_roots_for_repo_local(&request.hf_home, &request.hf_repo);
    let snapshot = snapshot_roots
        .iter()
        .flat_map(|root| hf_snapshot_dirs_local(root))
        .find(|candidate| candidate.join("config.json").is_file());
    checked_paths.extend(snapshot_roots.iter().cloned());
    if let Some(snapshot) = snapshot
        && split_weights_ready(&prepared_weights_root)
    {
        return Ok(ModelLayoutSummary {
            model_path: snapshot,
            weights_path: Some(prepared_weights_root.join("model_weights.bin")),
            manifest_path: Some(prepared_weights_root.join("model_weights.json")),
            model_source: RuntimeModelSource::HuggingFaceSnapshot,
        });
    }

    checked_paths.push(prepared_weights_root.clone());
    if inspect_prepared_model_layout(&prepared_weights_root).status == PreparedModelStatus::Ready {
        return Ok(ModelLayoutSummary {
            model_path: prepared_weights_root,
            weights_path: None,
            manifest_path: None,
            model_source: RuntimeModelSource::PreparedWeightsFallback,
        });
    }

    Err(RuntimeDiscoveryFailure::ModelLayoutMissing {
        hf_repo: request.hf_repo.clone(),
        prepared_weights_root,
        checked_paths,
    })
}

fn direct_model_layout_summary(
    candidate: &Path,
    prepared_weights_root: &Path,
) -> Option<ModelLayoutSummary> {
    if inspect_prepared_model_layout(candidate).status == PreparedModelStatus::Ready {
        return Some(ModelLayoutSummary {
            model_path: candidate.to_path_buf(),
            weights_path: None,
            manifest_path: None,
            model_source: RuntimeModelSource::ExplicitModelPath,
        });
    }

    if !candidate.join("config.json").is_file() {
        return None;
    }

    if split_weights_ready(prepared_weights_root) {
        return Some(ModelLayoutSummary {
            model_path: candidate.to_path_buf(),
            weights_path: Some(prepared_weights_root.join("model_weights.bin")),
            manifest_path: Some(prepared_weights_root.join("model_weights.json")),
            model_source: RuntimeModelSource::ExplicitModelPath,
        });
    }

    None
}

fn split_weights_ready(prepared_weights_root: &Path) -> bool {
    prepared_weights_root.join("model_weights.bin").is_file()
        && prepared_weights_root.join("model_weights.json").is_file()
}

fn hf_snapshot_roots_for_repo_local(hf_home: &Path, hf_repo: &str) -> Vec<PathBuf> {
    let relative_repo_dir = format!("models--{}", hf_repo.replace('/', "--"));
    let mut roots = vec![
        hf_home
            .join("hub")
            .join(&relative_repo_dir)
            .join("snapshots"),
        hf_home.join(&relative_repo_dir).join("snapshots"),
    ];
    let fallback_home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    roots.push(
        fallback_home
            .join(".cache/huggingface/hub")
            .join(relative_repo_dir)
            .join("snapshots"),
    );
    roots.push(
        fallback_home
            .join(".cache/huggingface")
            .join(format!("models--{}", hf_repo.replace('/', "--")))
            .join("snapshots"),
    );
    roots
}

fn hf_snapshot_dirs_local(root: &Path) -> Vec<PathBuf> {
    let mut snapshots = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                snapshots.push(path);
            }
        }
    }
    snapshots
}

fn flash_moe_runtime_assets_ready(root: &Path) -> bool {
    ["shaders.metal", "vocab.bin", "tokenizer.bin"]
        .iter()
        .all(|name| root.join(name).is_file())
}

fn running_broker_url(config: &ClientConfig) -> Option<String> {
    let state_path = broker_state_file(&config.state_dir);
    if let Ok(data) = std::fs::read(&state_path)
        && let Ok(state) = serde_json::from_slice::<BrokerStateFile>(&data)
    {
        return Some(format!("http://{}", state.bind));
    }
    Some(format!("http://{}", config.broker_bind))
}

fn format_checked_paths(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return "no infer paths".to_string();
    }
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn http_probe_json<T>(url: &str, timeout: Duration) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    let response = http_probe(url, timeout)?;
    if response.status_code / 100 != 2 {
        return Err(format!("endpoint returned HTTP {}", response.status_code));
    }
    serde_json::from_str(&response.body).map_err(|error| format!("parsing response: {error}"))
}

struct HttpProbeResponse {
    status_code: u16,
    body: String,
}

fn http_probe(url: &str, timeout: Duration) -> Result<HttpProbeResponse, String> {
    let url = reqwest::Url::parse(url).map_err(|error| format!("parse url: {error}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| "probe url missing host".to_string())?;
    let port = url.port_or_known_default().unwrap_or(80);
    let socket_addr = format!("{host}:{port}")
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], port)));
    let mut stream = std::net::TcpStream::connect_timeout(&socket_addr, timeout)
        .map_err(|error| format!("connect to {socket_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| format!("set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| format!("set write timeout: {error}"))?;

    let path = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    };
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n",
        if path.is_empty() { "/" } else { &path }
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("write request: {error}"))?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| format!("read response: {error}"))?;
    parse_http_probe_response(&response)
}

fn parse_http_probe_response(response: &[u8]) -> Result<HttpProbeResponse, String> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "probe response missing header terminator".to_string())?;
    let (header_bytes, body_bytes) = response.split_at(header_end);
    let header_text = String::from_utf8_lossy(header_bytes);
    let status_line = header_text
        .lines()
        .next()
        .ok_or_else(|| "probe response missing status line".to_string())?;
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "probe response missing status code".to_string())?
        .parse::<u16>()
        .map_err(|error| format!("parse status code: {error}"))?;
    let body = String::from_utf8_lossy(&body_bytes[4..]).into_owned();
    Ok(HttpProbeResponse { status_code, body })
}

fn format_doctor_report(
    model: &ModelSpec,
    diagnostic: &SsdMoeStartupDiagnostic,
    transitions: &[SsdMoeTransitionRecord],
) -> String {
    let launch = &diagnostic.launch_probe;
    let broker = &diagnostic.broker_probe;
    let mut lines = vec![
        format!("Model: {}", model.id),
        format!("Startup code: {:?}", diagnostic.code),
        format!("Detail: {}", diagnostic.detail),
        format!(
            "Models root: {}",
            SsdMoeRuntimeHandle::runtime_models_dir().display()
        ),
        format!(
            "Broker state root: {}",
            SsdMoeRuntimeHandle::broker_state_dir().display()
        ),
        format!(
            "Checked infer paths: {}",
            format_checked_paths(&launch.checked_infer_paths)
        ),
        format!(
            "Resolved infer: {}",
            launch
                .infer_bin
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<missing>".to_string())
        ),
        format!(
            "Resolved model path: {}",
            launch
                .model_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<missing>".to_string())
        ),
        format!(
            "Resolved weights: {}",
            launch
                .weights_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<none>".to_string())
        ),
        format!(
            "Resolved manifest: {}",
            launch
                .manifest_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<none>".to_string())
        ),
        format!(
            "Broker binary: {} ({})",
            broker.expected_binary_path.display(),
            if broker.installed_binary_exists {
                "installed"
            } else {
                "missing"
            }
        ),
        format!(
            "Broker url: {}",
            broker
                .broker_url
                .clone()
                .unwrap_or_else(|| "<unavailable>".to_string())
        ),
        format!(
            "Broker health: {}",
            broker
                .health
                .as_ref()
                .map(|health| {
                    format!(
                        "{} · {} instance(s) · {} lease(s)",
                        health.status, health.instance_count, health.lease_count
                    )
                })
                .unwrap_or_else(|| {
                    broker
                        .probe_error
                        .clone()
                        .unwrap_or_else(|| "unreachable".to_string())
                })
        ),
    ];
    if broker.instances.is_empty() {
        lines.push("Active broker instances: none".to_string());
    } else {
        lines.push(format!(
            "Active broker instances: {}",
            broker.instances.len()
        ));
        for instance in &broker.instances {
            lines.push(format!(
                "- {} {} healthy={} leases={} infer={} model={}",
                instance.instance_id,
                instance.base_url,
                instance.healthy,
                instance.lease_count,
                instance.infer_bin.display(),
                instance.model_path.display()
            ));
        }
    }
    if transitions.is_empty() {
        lines.push("Recent runtime transitions: none".to_string());
    } else {
        lines.push(format!("Recent runtime transitions: {}", transitions.len()));
        for transition in transitions.iter().rev().take(8) {
            lines.push(format!(
                "- {} -> {} · {} · port={} instance={} stale_base={}",
                transition.previous_status,
                transition.next_status,
                transition.reason,
                transition.port,
                transition
                    .instance_id
                    .clone()
                    .unwrap_or_else(|| "<none>".to_string()),
                transition
                    .base_url
                    .clone()
                    .unwrap_or_else(|| "<none>".to_string())
            ));
        }
    }
    lines.join("\n")
}

fn runtime_model_source_label(source: &RuntimeModelSource) -> &'static str {
    match source {
        RuntimeModelSource::ExplicitModelPath => "explicit model path",
        RuntimeModelSource::PreparedModelDir => "prepared model directory",
        RuntimeModelSource::HuggingFaceSnapshot => "huggingface snapshot",
        RuntimeModelSource::PreparedWeightsFallback => "prepared weights fallback",
    }
}

fn acquire_disposition_label(disposition: &AcquireDisposition) -> &'static str {
    match disposition {
        AcquireDisposition::Reused => "reused",
        AcquireDisposition::Spawned => "spawned",
        AcquireDisposition::Adopted => "adopted",
        AcquireDisposition::Busy => "busy",
    }
}

fn parse_extract_progress(line: &str) -> Option<f32> {
    let progress = line.strip_prefix("Extracting: ")?;
    let progress = progress.strip_suffix('%')?;
    progress.parse::<f32>().ok()
}

fn parse_packing_progress(line: &str) -> Option<(u32, u32)> {
    let progress = line.strip_prefix("Packing layer ")?;
    let (layer, total_layers) = progress.split_once('/')?;
    Some((layer.parse().ok()?, total_layers.parse().ok()?))
}

pub struct SsdMoeManager {
    runtime: SsdMoeRuntimeHandle,
    test_status_override: Option<ModelStatus>,
    #[cfg(test)]
    poll_health_calls: u64,
}

impl SsdMoeManager {
    pub fn new() -> Self {
        Self {
            runtime: SsdMoeRuntimeHandle::shared_handle(),
            test_status_override: None,
            #[cfg(test)]
            poll_health_calls: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_detached_for_test(port: u16) -> Self {
        Self {
            runtime: SsdMoeRuntimeHandle::detached_for_test(port),
            test_status_override: None,
            #[cfg(test)]
            poll_health_calls: 0,
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

    pub fn acquire_metadata(&self) -> Option<SsdMoeAcquireMetadata> {
        self.runtime.acquire_metadata()
    }

    pub fn wait_metadata(&self) -> Option<SsdMoeWaitMetadata> {
        self.runtime.wait_metadata()
    }

    #[allow(dead_code)]
    pub fn recent_transitions(&self) -> Vec<SsdMoeTransitionRecord> {
        self.runtime.recent_transitions()
    }

    pub fn last_transition_reason(&self) -> Option<String> {
        self.runtime.last_transition_reason()
    }

    pub fn bootstrap_state(&self) -> SsdMoeBootstrapState {
        self.runtime.bootstrap_state()
    }

    pub fn startup_diagnostic_for_model(model: &ModelSpec) -> SsdMoeStartupDiagnostic {
        SsdMoeRuntimeHandle::startup_diagnostic(ModelStatus::Ready, Some(model), None, None)
    }

    pub fn doctor_report(model: &ModelSpec) -> String {
        let diagnostic = Self::startup_diagnostic_for_model(model);
        format_doctor_report(
            model,
            &diagnostic,
            &SsdMoeRuntimeHandle::shared_handle().recent_transitions(),
        )
    }

    pub fn broker_installation_status(&self) -> ssd_moe_client::BrokerInstallationStatus {
        broker_installation_status(&SsdMoeRuntimeHandle::broker_client_config())
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
        #[cfg(test)]
        {
            self.poll_health_calls = self.poll_health_calls.saturating_add(1);
        }
        self.runtime.poll_health();
    }

    #[cfg(test)]
    pub fn poll_health_count_for_test(&self) -> u64 {
        self.poll_health_calls
    }

    pub fn switch_model(&mut self, project_root: &Path, new_model: &ModelSpec) {
        self.test_status_override = None;
        self.runtime.switch_model(project_root, new_model);
    }

    #[allow(dead_code)]
    pub fn switch_role(&mut self, project_root: &Path, role: LocalModelRole) {
        self.test_status_override = None;
        self.runtime.switch_role(project_root, role);
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    use ssd_moe_contract::{
        BrokerConflictKind, BrokerErrorCode, BrokerErrorResponse, BrokerInstanceRecord,
        InstanceSource,
    };

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        super::test_env_lock()
    }

    fn set_runtime_env(models_dir: &Path) {
        unsafe {
            std::env::set_var("GARY_MODELS_DIR", models_dir);
            std::env::set_var("SSD_MOE_MODELS_DIR", models_dir);
            std::env::set_var("QUORP_SSD_MOE_STATE_DIR", models_dir.join(".ssd-moe"));
            std::env::set_var("QUORP_TURBOHERO_ROOT", models_dir);
            std::env::set_var(
                "QUORP_TURBOHERO_PYTHON",
                models_dir.join(".venv-py312/bin/python"),
            );
            std::env::remove_var("QUORP_SSD_MOE_INFER_BIN");
            std::env::remove_var("QUORP_SSD_MOE_MODEL_PATH");
            std::env::remove_var("QUORP_FLASH_MOE_WEIGHTS_DIR");
            std::env::remove_var("QUORP_SSD_MOE_SETUP_SCRIPT");
            std::env::remove_var("HF_HOME");
            std::env::remove_var("SSD_MOE_STATE_DIR");
        }
    }

    fn clear_runtime_env() {
        unsafe {
            std::env::remove_var("GARY_MODELS_DIR");
            std::env::remove_var("SSD_MOE_MODELS_DIR");
            std::env::remove_var("QUORP_SSD_MOE_STATE_DIR");
            std::env::remove_var("QUORP_SSD_MOE_INFER_BIN");
            std::env::remove_var("QUORP_SSD_MOE_MODEL_PATH");
            std::env::remove_var("QUORP_FLASH_MOE_WEIGHTS_DIR");
            std::env::remove_var("QUORP_SSD_MOE_SETUP_SCRIPT");
            std::env::remove_var("QUORP_TURBOHERO_ROOT");
            std::env::remove_var("QUORP_TURBOHERO_PYTHON");
            std::env::remove_var("HF_HOME");
            std::env::remove_var("SSD_MOE_STATE_DIR");
        }
    }

    fn flash_moe_test_model() -> ModelSpec {
        crate::quorp::tui::model_registry::local_moe_spec_for_registry_id(
            "ssd_moe/qwen3-coder-30b-a3b",
        )
        .or_else(|| {
            crate::quorp::tui::model_registry::local_moe_spec_for_registry_id("qwen3-coder-30b-a3b")
        })
        .expect("qwen flash-moe test model")
    }

    fn unused_local_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .expect("bind unused port probe")
            .local_addr()
            .expect("unused port addr")
            .port()
    }

    fn write_fake_infer(root: &Path) -> PathBuf {
        let bin_dir = root.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("mkdir infer bin");
        let infer_path = bin_dir.join("infer");
        std::fs::write(&infer_path, "#!/bin/sh\nsleep 5\n").expect("write infer");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = std::fs::metadata(&infer_path)
                .expect("infer metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&infer_path, permissions).expect("chmod infer");
        }
        infer_path
    }

    fn write_fake_turbohero_python(root: &Path) -> PathBuf {
        let python_dir = root.join(".venv-py312").join("bin");
        std::fs::create_dir_all(&python_dir).expect("mkdir turbohero python dir");
        let python_path = python_dir.join("python");
        std::fs::write(&python_path, "#!/bin/sh\nexit 0\n").expect("write turbohero python");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = std::fs::metadata(&python_path)
                .expect("turbohero python metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&python_path, permissions).expect("chmod turbohero python");
        }
        python_path
    }

    fn write_fake_turbohero_script(root: &Path) -> PathBuf {
        let script_dir = root.join("turbohero");
        std::fs::create_dir_all(&script_dir).expect("mkdir turbohero script dir");
        let script_path = script_dir.join("mlx_oracle_server.py");
        std::fs::write(&script_path, "print('ok')\n").expect("write turbohero script");
        script_path
    }

    fn prepared_flash_moe_test_model() -> ModelSpec {
        let mut model = flash_moe_test_model();
        model.runtime_backend = LocalRuntimeBackend::FlashMoePrepared {
            model_dir_name: "qwen3-coder-30b-a3b-test",
            k_experts: model.k_experts(),
            think_budget: model.think_budget(),
            kv_seq: model.kv_seq(),
        };
        model
    }

    fn write_model_snapshot_and_weights(root: &Path, model: &ModelSpec) -> (PathBuf, PathBuf) {
        let snapshot_root = root
            .join("huggingface")
            .join("hub")
            .join(format!("models--{}", model.hf_repo.replace('/', "--")))
            .join("snapshots")
            .join("snapshot-1");
        std::fs::create_dir_all(&snapshot_root).expect("mkdir snapshot");
        std::fs::write(snapshot_root.join("config.json"), "{\"hidden_size\": 1}")
            .expect("write config");

        let weights_root = root
            .join("flash-moe")
            .join(model.model_dir_name().expect("flash-moe model dir"));
        std::fs::create_dir_all(&weights_root).expect("mkdir weights");
        std::fs::write(weights_root.join("model_weights.bin"), b"weights").expect("write weights");
        std::fs::write(weights_root.join("model_weights.json"), "{\"tensors\": {}}")
            .expect("write manifest");
        (snapshot_root, weights_root)
    }

    fn write_broker_state(root: &Path, bind: std::net::SocketAddr) {
        let state_dir = root.join(".ssd-moe");
        std::fs::create_dir_all(&state_dir).expect("mkdir broker state");
        let state = BrokerStateFile {
            pid: 42,
            bind: bind.to_string(),
            started_at_ms: 1,
            state_dir,
            broker_version: "0.1.0".to_string(),
            api_version: "v1".to_string(),
        };
        std::fs::write(
            broker_state_file(&root.join(".ssd-moe")),
            serde_json::to_vec(&state).expect("serialize broker state"),
        )
        .expect("write broker state");
    }

    fn start_fake_broker_server(
        instances: Vec<BrokerInstanceRecord>,
    ) -> (std::net::SocketAddr, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake broker");
        let address = listener.local_addr().expect("broker addr");
        let instances_body = serde_json::to_string(&instances).expect("serialize instances");
        let health_body = serde_json::to_string(&BrokerHealth {
            api_version: "v1".to_string(),
            status: "ok".to_string(),
            bind: address.to_string(),
            state_dir: PathBuf::from("/tmp/fake-broker"),
            instance_count: instances.len(),
            lease_count: instances.iter().map(|instance| instance.lease_count).sum(),
            broker_version: "0.1.0".to_string(),
        })
        .expect("serialize health");
        let handle = std::thread::spawn(move || {
            for _ in 0..2 {
                let Ok((mut stream, _peer)) = listener.accept() else {
                    break;
                };
                let mut buffer = [0u8; 2048];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let body = match path {
                    "/v1/health" => health_body.clone(),
                    "/v1/instances" => instances_body.clone(),
                    _ => "{}".to_string(),
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (address, handle)
    }

    fn start_fake_turbohero_server(
        served_model_id: &'static str,
        ready_after_requests: usize,
    ) -> (u16, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake turbohero");
        listener
            .set_nonblocking(true)
            .expect("set fake turbohero nonblocking");
        let port = listener.local_addr().expect("fake turbohero addr").port();
        let handle = std::thread::spawn(move || {
            let mut served_requests = 0usize;
            let mut last_request_at = Instant::now();
            loop {
                match listener.accept() {
                    Ok((mut stream, _peer)) => {
                        let mut buffer = [0u8; 4096];
                        let read = stream.read(&mut buffer).unwrap_or(0);
                        let request = String::from_utf8_lossy(&buffer[..read]);
                        let path = request
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or("/");
                        served_requests += 1;
                        last_request_at = Instant::now();
                        let status = if served_requests >= ready_after_requests {
                            "ok"
                        } else {
                            "warming"
                        };
                        let body = match path {
                            "/health" => serde_json::json!({
                                "status": status,
                                "model": served_model_id
                            })
                            .to_string(),
                            "/v1/runtime" => serde_json::json!({
                                "status": status,
                                "served_model_id": served_model_id,
                                "pid": 4242
                            })
                            .to_string(),
                            _ => "{}".to_string(),
                        };
                        let response = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if served_requests > 0
                            && last_request_at.elapsed() >= Duration::from_secs(2)
                        {
                            break;
                        }
                        if last_request_at.elapsed() >= Duration::from_secs(10) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
        });
        (port, handle)
    }

    fn test_turbohero_model(port: u16, served_model_id: &'static str) -> ModelSpec {
        let mut model = crate::quorp::tui::model_registry::local_moe_spec_for_registry_id(
            "ssd_moe/deepseek-coder-v2-lite-turbo",
        )
        .expect("deepseek turbo model");
        model.runtime_backend = LocalRuntimeBackend::TurboHeroOracle {
            profile_key: "deepseek_coder_v2_lite",
            mode: "turbo",
            served_model_id,
            default_port: port,
            max_ctx: 8192,
            max_output_tokens: 512,
            trust_remote_code: false,
        };
        model
    }

    fn write_flash_moe_runtime_assets(root: &Path) {
        let flash_moe_dir = root.join("flash-moe");
        std::fs::create_dir_all(&flash_moe_dir).expect("mkdir flash-moe");
        for name in ["shaders.metal", "vocab.bin", "tokenizer.bin"] {
            std::fs::write(flash_moe_dir.join(name), b"ok").expect("write flash-moe asset");
        }
    }

    #[test]
    fn status_labels_are_correct() {
        assert_eq!(ModelStatus::Ready.label(), "Ready");
        assert_eq!(ModelStatus::Starting.label(), "Starting...");
        assert_eq!(ModelStatus::Running.label(), "Online");
        assert_eq!(ModelStatus::Failed("err".into()).label(), "Offline");
    }

    #[test]
    fn resolve_missing_dir_returns_none() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = env_lock();
        set_runtime_env(temp.path());
        let _model_guard = crate::quorp::tui::model_registry::isolated_test_model_config_guard();
        crate::quorp::tui::model_registry::save_model(flash_moe_test_model().id)
            .expect("save flash-moe model");
        assert!(SsdMoeManager::resolve_server_path(temp.path()).is_none());
        clear_runtime_env();
    }

    #[test]
    fn resolve_existing_dir_returns_path() {
        let _guard = env_lock();
        let temp = tempfile::tempdir().expect("tempdir");
        set_runtime_env(temp.path());
        let _model_guard = crate::quorp::tui::model_registry::isolated_test_model_config_guard();
        crate::quorp::tui::model_registry::save_model(flash_moe_test_model().id)
            .expect("save flash-moe model");
        write_fake_turbohero_python(temp.path());
        let expected = write_fake_turbohero_script(temp.path());
        let result = SsdMoeManager::resolve_server_path(temp.path());
        assert!(result.is_some());
        assert_eq!(result.expect("some"), expected);
        clear_runtime_env();
    }

    #[test]
    fn ensure_running_fails_when_no_binary() {
        let _guard = env_lock();
        let temp = tempfile::tempdir().expect("tempdir");
        set_runtime_env(temp.path());
        let mut server = SsdMoeManager::new_detached_for_test(59_997);
        let model = prepared_flash_moe_test_model();
        server.ensure_running(temp.path(), &model);
        assert!(matches!(server.status(), ModelStatus::Failed(_)));
        clear_runtime_env();
    }

    #[test]
    fn launch_probe_checked_paths_are_quorp_owned_only() {
        let _guard = env_lock();
        let temp = tempfile::tempdir().expect("tempdir");
        set_runtime_env(temp.path());
        let model = prepared_flash_moe_test_model();
        write_model_snapshot_and_weights(temp.path(), &model);

        let diagnostic = SsdMoeManager::startup_diagnostic_for_model(&model);

        assert_eq!(diagnostic.code, SsdMoeStartupCode::InferBinaryMissing);
        assert_eq!(diagnostic.launch_probe.checked_infer_paths.len(), 2);
        assert!(
            diagnostic
                .launch_probe
                .checked_infer_paths
                .iter()
                .all(|path| path.starts_with(temp.path()))
        );
        assert!(
            diagnostic
                .launch_probe
                .checked_infer_paths
                .iter()
                .all(|path| !path.display().to_string().contains("GARY_ROOT"))
        );
        clear_runtime_env();
    }

    #[test]
    fn startup_diagnostic_reports_healthy_broker_instance_when_infer_is_missing() {
        let _guard = env_lock();
        let temp = tempfile::tempdir().expect("tempdir");
        set_runtime_env(temp.path());
        let model = prepared_flash_moe_test_model();
        let (snapshot_root, weights_root) = write_model_snapshot_and_weights(temp.path(), &model);
        let (bind, handle) = start_fake_broker_server(vec![BrokerInstanceRecord {
            instance_id: "instance-123".to_string(),
            fingerprint: ssd_moe_contract::InstanceFingerprint {
                model_path: snapshot_root.clone(),
                weights_path: Some(weights_root.join("model_weights.bin")),
                manifest_path: Some(weights_root.join("model_weights.json")),
                infer_bin: PathBuf::from(
                    "/Users/bentaylor/Code/GARY_ROOT/external/flash-moe/infer",
                ),
                infer_build_id: "build-id".to_string(),
                k: 8,
                think_budget: 512,
                kv_seq: 2048,
                serve_mode: "openai".to_string(),
                api_version: "v1".to_string(),
                tokenizer_path: None,
            },
            base_url: "http://127.0.0.1:18189".to_string(),
            health_url: "http://127.0.0.1:18189/health".to_string(),
            models_url: "http://127.0.0.1:18189/v1/models".to_string(),
            runtime_url: "http://127.0.0.1:18189/v1/runtime".to_string(),
            source: InstanceSource::BrokerSpawned,
            pid: Some(9001),
            started_at_ms: 1,
            last_seen_at_ms: 2,
            lease_count: 2,
            healthy: true,
            managed_by_broker: true,
        }]);
        write_broker_state(temp.path(), bind);

        let diagnostic = SsdMoeManager::startup_diagnostic_for_model(&model);

        assert_eq!(diagnostic.code, SsdMoeStartupCode::InferBinaryMissing);
        assert!(diagnostic.detail.contains("healthy shared runtime exists"));
        assert!(
            diagnostic
                .detail
                .contains("/Users/bentaylor/Code/GARY_ROOT/external/flash-moe/infer")
        );
        assert!(diagnostic.detail.contains("http://127.0.0.1:18189"));
        handle.join().expect("join fake broker");
        clear_runtime_env();
    }

    #[test]
    fn launch_working_dir_prefers_flash_moe_assets_when_infer_lives_in_bin() {
        let _guard = env_lock();
        let temp = tempfile::tempdir().expect("tempdir");
        set_runtime_env(temp.path());
        write_flash_moe_runtime_assets(temp.path());
        let infer_path = write_fake_infer(temp.path());
        let resolved_launch = ssd_moe_launch::ResolvedRuntimeLaunch {
            infer_bin: infer_path,
            working_dir: temp.path().join("bin"),
            model_layout: ssd_moe_launch::ResolvedModelLayout {
                model_path: temp.path().join("model"),
                prepared_model_root: temp.path().join("model"),
                weights_path: None,
                manifest_path: None,
            },
            model_source: RuntimeModelSource::PreparedModelDir,
        };

        let working_dir = SsdMoeRuntimeHandle::launch_working_dir(&resolved_launch);

        assert_eq!(working_dir, temp.path().join("flash-moe"));
        clear_runtime_env();
    }

    #[test]
    fn health_check_closed_port_returns_false() {
        let server = SsdMoeManager::new_detached_for_test(unused_local_port());
        assert!(!server.runtime.tcp_probe());
    }

    #[test]
    fn poll_health_running_drop_preserves_stale_metadata_and_transition() {
        let port = unused_local_port();
        let mut server = SsdMoeManager::new_detached_for_test(port);
        server
            .runtime
            .set_active_model_for_test(Some(prepared_flash_moe_test_model()));
        {
            let mut state = server.runtime.inner.lock().expect("ssd-moe runtime lock");
            state.port = port;
            state.base_url = Some(format!("http://127.0.0.1:{port}/v1"));
            state.acquire_metadata = Some(SsdMoeAcquireMetadata {
                broker_url: "http://127.0.0.1:39491".to_string(),
                base_url: format!("http://127.0.0.1:{port}"),
                instance_id: "instance-123".to_string(),
                disposition: AcquireDisposition::Reused,
                lease_count: 1,
                model_path: PathBuf::from("/tmp/model"),
                weights_path: Some(PathBuf::from("/tmp/model_weights.bin")),
                manifest_path: Some(PathBuf::from("/tmp/model_weights.json")),
                stale: false,
            });
            server.runtime.push_transition(
                &mut state,
                &ModelStatus::Running,
                "seed running runtime".to_string(),
                Some(true),
            );
        }

        server.poll_health();

        assert_eq!(server.status(), ModelStatus::Ready);
        let metadata = server.acquire_metadata().expect("retained metadata");
        assert!(metadata.stale);
        let transitions = server.recent_transitions();
        assert!(transitions.iter().any(|transition| {
            transition.previous_status == "Online"
                && transition.next_status == "Ready"
                && transition.reason.contains("runtime health probe failed")
                && transition.tcp_probe_ok == Some(false)
        }));
    }

    #[test]
    fn poll_health_noop_when_not_starting() {
        let mut server = SsdMoeManager::new_detached_for_test(59_994);
        server.poll_health();
        assert_eq!(server.status(), ModelStatus::Ready);
    }

    #[test]
    fn wait_until_ready_times_out_when_unavailable() {
        let server = SsdMoeManager::new_detached_for_test(unused_local_port());
        let error = server
            .wait_until_ready(Duration::from_millis(50))
            .expect_err("should time out");
        assert!(error.contains("did not become ready"));
    }

    #[test]
    fn ensure_running_attaches_to_healthy_turbohero_listener() {
        let _guard = env_lock();
        let temp = tempfile::tempdir().expect("tempdir");
        set_runtime_env(temp.path());
        let (port, handle) = start_fake_turbohero_server("deepseek-coder-v2-lite-turbo", 1);
        let mut server = SsdMoeManager::new_detached_for_test(port);
        let model = test_turbohero_model(port, "deepseek-coder-v2-lite-turbo");

        server.ensure_running(temp.path(), &model);

        assert_eq!(server.status(), ModelStatus::Running);
        let transitions = server.runtime.recent_transitions();
        assert!(transitions.iter().any(|transition| {
            transition.reason.contains("already healthy")
                && transition.next_status == ModelStatus::Running.label()
        }));
        handle.join().expect("join fake turbohero");
        clear_runtime_env();
    }

    #[test]
    fn wait_until_ready_allows_warming_turbohero_listener_to_become_ready() {
        let _guard = env_lock();
        let temp = tempfile::tempdir().expect("tempdir");
        set_runtime_env(temp.path());
        let (port, handle) = start_fake_turbohero_server("deepseek-coder-v2-lite-turbo", 3);
        let server = SsdMoeRuntimeHandle::detached_for_test(port);
        let model = test_turbohero_model(port, "deepseek-coder-v2-lite-turbo");
        server.ensure_running(temp.path(), &model);

        server
            .wait_until_ready(Duration::from_secs(2))
            .expect("warming listener should become ready");

        assert_eq!(server.status(), ModelStatus::Running);
        handle.join().expect("join fake turbohero");
        clear_runtime_env();
    }

    #[test]
    fn ensure_running_reports_wrong_model_on_turbohero_port() {
        let _guard = env_lock();
        let temp = tempfile::tempdir().expect("tempdir");
        set_runtime_env(temp.path());
        let (port, handle) = start_fake_turbohero_server("different-model", 1);
        let mut server = SsdMoeManager::new_detached_for_test(port);
        let model = test_turbohero_model(port, "deepseek-coder-v2-lite-turbo");

        server.ensure_running(temp.path(), &model);

        let status = server.status();
        assert!(
            matches!(status, ModelStatus::Failed(message) if message.contains("different-model"))
        );
        handle.join().expect("join fake turbohero");
        clear_runtime_env();
    }

    #[test]
    fn wait_until_ready_times_out_with_broker_conflict_detail() {
        let _guard = env_lock();
        let temp = tempfile::tempdir().expect("tempdir");
        set_runtime_env(temp.path());
        let server = SsdMoeRuntimeHandle::detached_for_test(59_992);
        let model = flash_moe_test_model();
        server.record_waiting_for_broker(
            &model,
            &BrokerErrorResponse {
                error_code: BrokerErrorCode::Busy,
                message: "another app is holding an incompatible runtime".to_string(),
                conflict: Some(BrokerConflictDetails {
                    kind: BrokerConflictKind::IncompatibleActive,
                    active_instance_id: Some("instance-123".to_string()),
                    active_base_url: Some("http://127.0.0.1:9000".to_string()),
                    active_lease_count: Some(2),
                    active_instance_healthy: Some(true),
                }),
            },
        );

        let error = server
            .wait_until_ready(Duration::from_millis(50))
            .expect_err("should time out");

        assert!(error.contains("another app is holding an incompatible runtime"));
        assert!(error.contains("timeout"));
        clear_runtime_env();
    }

    #[test]
    fn parse_extract_progress_reads_percent() {
        assert_eq!(parse_extract_progress("Extracting: 37%"), Some(37.0));
        assert_eq!(parse_extract_progress("Extracting: 100%"), Some(100.0));
        assert_eq!(parse_extract_progress("Packing layer 2/40"), None);
    }

    #[test]
    fn parse_packing_progress_reads_layer_counts() {
        assert_eq!(parse_packing_progress("Packing layer 2/40"), Some((2, 40)));
        assert_eq!(parse_packing_progress("Extracting: 15%"), None);
    }

    #[test]
    fn ensure_running_starts_model_setup_with_explicit_script() {
        let _guard = env_lock();
        let temp = tempfile::tempdir().expect("tempdir");
        set_runtime_env(temp.path());
        let infer_path = write_fake_infer(temp.path());
        unsafe {
            std::env::set_var("QUORP_SSD_MOE_INFER_BIN", &infer_path);
        }
        let script_dir = temp.path().join("scripts");
        std::fs::create_dir_all(&script_dir).expect("mkdir scripts");
        let setup_script = script_dir.join("setup_model.py");
        std::fs::write(
            &setup_script,
            r#"#!/usr/bin/env python3
import argparse
import pathlib
import time

parser = argparse.ArgumentParser()
parser.add_argument("--repo")
parser.add_argument("--output", required=True)
args = parser.parse_args()
output = pathlib.Path(args.output)
print("Downloading test/model...")
print("Extracting: 50%")
time.sleep(0.1)
(output / "packed_experts").mkdir(parents=True, exist_ok=True)
(output / "packed_experts" / "layer_00.bin").write_bytes(b"ok")
(output / "config.json").write_text("{\"hidden_size\": 1}")
(output / "model_weights.bin").write_bytes(b"weights")
(output / "model_weights.json").write_text("{\"tensors\": {}}")
print("Packing layer 1/1")
"#,
        )
        .expect("write setup script");
        unsafe {
            std::env::set_var("QUORP_SSD_MOE_SETUP_SCRIPT", &setup_script);
        }

        let mut server = SsdMoeManager::new_detached_for_test(59_991);
        let custom_model_dir = temp.path().join("out_test_auto_download");
        let custom_model_dir = Box::leak(
            custom_model_dir
                .to_string_lossy()
                .into_owned()
                .into_boxed_str(),
        );
        let mut model = flash_moe_test_model();
        model.runtime_backend = LocalRuntimeBackend::FlashMoePrepared {
            model_dir_name: custom_model_dir,
            k_experts: model.k_experts(),
            think_budget: model.think_budget(),
            kv_seq: model.kv_seq(),
        };

        server.ensure_running(temp.path(), &model);
        assert!(matches!(
            server.status(),
            ModelStatus::Downloading { .. } | ModelStatus::Packing { .. }
        ));
        let deadline = Instant::now() + Duration::from_secs(2);
        let weights_bin = Path::new(custom_model_dir).join("model_weights.bin");
        let weights_manifest = Path::new(custom_model_dir).join("model_weights.json");
        let first_layer = Path::new(custom_model_dir).join("packed_experts/layer_00.bin");
        while Instant::now() < deadline
            && !(weights_bin.is_file() && weights_manifest.is_file() && first_layer.is_file())
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(weights_bin.is_file());
        assert!(weights_manifest.is_file());
        assert!(first_layer.is_file());
        clear_runtime_env();
    }

    #[test]
    fn bootstrap_state_reports_waiting_message() {
        let server = SsdMoeRuntimeHandle::detached_for_test(59_990);
        let model = flash_moe_test_model();
        server.record_waiting_for_broker(
            &model,
            &BrokerErrorResponse {
                error_code: BrokerErrorCode::Busy,
                message: "waiting for the shared broker".to_string(),
                conflict: None,
            },
        );

        let state = server.bootstrap_state();

        assert_eq!(state.phase_label, "Waiting");
        assert_eq!(state.detail, "waiting for the shared broker");
    }

    #[test]
    fn inspect_model_layout_rejects_partial_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let model_root = temp.path().join("out_35b");
        std::fs::create_dir_all(model_root.join("packed_experts")).expect("mkdir packed");
        std::fs::write(model_root.join("packed_experts/layer_00.bin"), b"ok").expect("write layer");

        let inspection = inspect_prepared_model_layout(&model_root);

        assert_eq!(inspection.status, PreparedModelStatus::CorruptPartial);
        assert!(
            inspection
                .missing_paths
                .iter()
                .any(|path| path == "config.json")
        );
        assert!(
            inspection
                .missing_paths
                .iter()
                .any(|path| path == "model_weights.bin")
        );
        assert!(
            inspection
                .missing_paths
                .iter()
                .any(|path| path == "model_weights.json")
        );
    }

    #[test]
    fn latest_meaningful_server_error_prefers_fatal_stderr() {
        let logs = VecDeque::from(vec![
            "Starting SSD-MOE server".to_string(),
            "[stderr] FATAL: config.json not found in /tmp/model".to_string(),
        ]);

        let detail = SsdMoeRuntimeHandle::latest_meaningful_server_error(&logs);

        assert_eq!(
            detail.as_deref(),
            Some("config.json not found in /tmp/model")
        );
    }
}

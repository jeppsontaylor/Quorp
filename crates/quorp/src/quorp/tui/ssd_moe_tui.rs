#![allow(unused)]
use std::io::BufRead;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use language_models::{ssd_moe_shared_server, ServerStatus, SsdMoeServer};
use paths::user_models_dir;

use crate::quorp::tui::model_registry::ModelSpec;

const HEALTH_TIMEOUT_MS: u64 = 500;

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
            ModelStatus::Downloading {
                progress_pct, ..
            } => format!("Downloading {:.1}%", progress_pct),
            ModelStatus::Packing {
                layer,
                total_layers,
            } => format!("Packing {}/{}", layer, total_layers),
            ModelStatus::Ready => "Ready".into(),
            ModelStatus::Starting => "Starting...".into(),
            ModelStatus::Running => "Online".into(),
            ModelStatus::Stopping => "Stopping...".into(),
            ModelStatus::Failed(_) => "Offline".into(),
        }
    }

    pub fn indicator(&self) -> &str {
        match self {
            ModelStatus::NotDownloaded => "⬜",
            ModelStatus::Downloading { .. } => "📥",
            ModelStatus::Packing { .. } => "📦",
            ModelStatus::Ready => "⬜",
            ModelStatus::Starting => "🟡",
            ModelStatus::Running => "🟢",
            ModelStatus::Stopping => "🟡",
            ModelStatus::Failed(_) => "🔴",
        }
    }
}

/// TUI-side coordinator: shared [`SsdMoeServer`] when the SSD-MOE provider is registered, otherwise a private instance for tests.
pub struct SsdMoeManager {
    server: Arc<RwLock<SsdMoeServer>>,
    /// When false, the server is the process-wide singleton and must not be stopped on drop.
    stop_server_on_drop: bool,
    download_child: Option<Child>,
    download_logs: Arc<Mutex<Vec<String>>>,
    pub active_model: Option<ModelSpec>,
    ui_overlay_status: Option<ModelStatus>,
    test_status_override: Option<ModelStatus>,
}

impl SsdMoeManager {
    pub fn get_logs(&self) -> Vec<String> {
        let mut out = self.server.read().expect("ssd moe server lock").get_logs();
        out.extend(self.download_logs.lock().expect("download logs lock").clone());
        out
    }

    pub fn new() -> Self {
        let shared = ssd_moe_shared_server();
        let stop_server_on_drop = shared.is_none();
        let server = shared.unwrap_or_else(|| {
            Arc::new(RwLock::new(SsdMoeServer::new(
                flash_moe_defaults::DEFAULT_INFER_SERVE_PORT,
            )))
        });
        Self {
            server,
            stop_server_on_drop,
            download_child: None,
            download_logs: Arc::new(Mutex::new(Vec::new())),
            active_model: None,
            ui_overlay_status: None,
            test_status_override: None,
        }
    }

    /// Private [`SsdMoeServer`] on a chosen port (tests only). Does not use the process-wide singleton.
    #[cfg(test)]
    pub(crate) fn new_detached_for_test(port: u16) -> Self {
        Self {
            server: Arc::new(RwLock::new(SsdMoeServer::new(port))),
            stop_server_on_drop: true,
            download_child: None,
            download_logs: Arc::new(Mutex::new(Vec::new())),
            active_model: None,
            ui_overlay_status: None,
            test_status_override: None,
        }
    }

    pub fn status(&self) -> ModelStatus {
        if let Some(s) = &self.test_status_override {
            return s.clone();
        }
        if let Some(s) = &self.ui_overlay_status {
            return s.clone();
        }
        if self.download_child.is_some() {
            return self.download_status_from_logs();
        }
        match self.server.read().expect("ssd moe server lock").status() {
            ServerStatus::Stopped => ModelStatus::Ready,
            ServerStatus::Starting => ModelStatus::Starting,
            ServerStatus::Ready => ModelStatus::Running,
            ServerStatus::Failed(e) => ModelStatus::Failed(e),
        }
    }

    fn download_status_from_logs(&self) -> ModelStatus {
        let logs = self.download_logs.lock().expect("download logs lock");
        let Some(last) = logs.last() else {
            return ModelStatus::Downloading {
                progress_pct: 0.0,
                downloaded_gb: 0.0,
                total_gb: self
                    .active_model
                    .as_ref()
                    .map(|m| m.estimated_disk_gb)
                    .unwrap_or(1.0),
            };
        };
        let total_gb = self
            .active_model
            .as_ref()
            .map(|m| m.estimated_disk_gb)
            .unwrap_or(1.0);
        if last.starts_with("Downloading") {
            ModelStatus::Downloading {
                progress_pct: 5.0,
                downloaded_gb: total_gb * 0.05,
                total_gb,
            }
        } else if last.starts_with("Extracting: ") {
            let pct_str = last.replace("Extracting: ", "").replace('%', "");
            if let Ok(pct) = pct_str.parse::<f32>() {
                ModelStatus::Downloading {
                    progress_pct: 10.0 + (pct * 0.4),
                    downloaded_gb: total_gb * (0.1 + pct * 0.004),
                    total_gb,
                }
            } else {
                ModelStatus::Downloading {
                    progress_pct: 0.0,
                    downloaded_gb: 0.0,
                    total_gb,
                }
            }
        } else if last.starts_with("Packing layer ") {
            let replaced = last.replace("Packing layer ", "");
            let parts: Vec<&str> = replaced.split('/').collect();
            if parts.len() == 2 {
                if let (Ok(l), Ok(t)) = (parts[0].parse(), parts[1].parse()) {
                    return ModelStatus::Packing {
                        layer: l,
                        total_layers: t,
                    };
                }
            }
            ModelStatus::Downloading {
                progress_pct: 0.0,
                downloaded_gb: 0.0,
                total_gb,
            }
        } else if last == "Complete" {
            ModelStatus::Ready
        } else {
            ModelStatus::Downloading {
                progress_pct: 0.0,
                downloaded_gb: 0.0,
                total_gb,
            }
        }
    }

    pub fn set_status_for_test(&mut self, status: ModelStatus) {
        self.test_status_override = Some(status);
    }

    pub fn port(&self) -> u16 {
        self.server.read().expect("ssd moe server lock").port()
    }

    pub fn api_base(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port())
    }

    fn tcp_probe(&self) -> bool {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.port()));
        std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(HEALTH_TIMEOUT_MS)).is_ok()
    }

    pub fn resolve_server_path(project_root: &Path) -> Option<PathBuf> {
        SsdMoeServer::resolve_flash_moe_directory(None, Some(project_root))
    }

    pub fn ensure_running(&mut self, project_root: &Path, model: &ModelSpec) {
        self.active_model = Some(model.clone());
        self.test_status_override = None;

        let st = self.server.read().expect("ssd moe server lock").status();
        if matches!(st, ServerStatus::Starting | ServerStatus::Ready) {
            return;
        }

        if self
            .server
            .write()
            .expect("ssd moe server lock")
            .adopt_listener_if_present()
        {
            self.ui_overlay_status = None;
            return;
        }

        if SsdMoeServer::resolve_flash_moe_directory(None, Some(project_root))
            .or_else(|| SsdMoeServer::resolve_flash_moe_directory(None, None))
            .is_none()
        {
            self.ui_overlay_status = Some(ModelStatus::Failed(
                "infer binary not found in external/flash-moe/".into(),
            ));
            return;
        }

        let model_dir = match SsdMoeServer::resolve_model_weights_path(model.model_dir_name) {
            Ok(path) => path,
            Err(message) => {
                self.ui_overlay_status = Some(ModelStatus::Failed(message));
                return;
            }
        };
        if !model_dir.exists() || !model_dir.join("packed_experts").exists() {
            self.ui_overlay_status = Some(ModelStatus::NotDownloaded);
            return;
        }

        self.ui_overlay_status = None;

        self.server.write().expect("ssd moe server lock").start(
            None,
            self.port(),
            model.model_dir_name,
            model.k_experts,
            Some(project_root),
        );
    }

    pub fn download_model(&mut self, project_root: &Path, model: &ModelSpec) {
        self.stop();
        self.active_model = Some(model.clone());
        self.ui_overlay_status = None;
        self.test_status_override = None;

        self.download_logs.lock().expect("download logs lock").clear();

        let flash_moe_dir = match SsdMoeServer::resolve_flash_moe_directory(None, Some(project_root))
            .or_else(|| SsdMoeServer::resolve_flash_moe_directory(None, None))
        {
            Some(path) => path,
            None => {
                self.ui_overlay_status =
                    Some(ModelStatus::Failed("Could not resolve flash-moe directory".into()));
                return;
            }
        };

        let setup_script = flash_moe_dir.join("scripts/setup_model.py");
        let out_dir = match SsdMoeServer::resolve_model_weights_path(model.model_dir_name) {
            Ok(path) => path,
            Err(message) => {
                self.ui_overlay_status = Some(ModelStatus::Failed(message));
                return;
            }
        };

        let hub_cache = user_models_dir().join("huggingface_hub");
        let hf_home = user_models_dir().join("huggingface");
        if let Err(error) = std::fs::create_dir_all(&hub_cache) {
            log::warn!(
                "Could not create Hugging Face hub cache directory at {:?}: {}",
                hub_cache,
                error
            );
        }
        if let Err(error) = std::fs::create_dir_all(&hf_home) {
            log::warn!(
                "Could not create HF_HOME directory at {:?}: {}",
                hf_home,
                error
            );
        }

        let mut cmd = Command::new("python3");
        cmd.current_dir(&flash_moe_dir)
            .env("unbuffered", "1")
            .env("HUGGINGFACE_HUB_CACHE", hub_cache.as_os_str())
            .env("HF_HOME", hf_home.as_os_str())
            .args([
                "-u",
                setup_script.to_str().unwrap_or("scripts/setup_model.py"),
                "--repo",
                model.hf_repo,
                "--output",
                out_dir.to_str().unwrap_or(""),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        match cmd.spawn() {
            Ok(mut child) => {
                let stdout = child.stdout.take().unwrap();
                let stderr = child.stderr.take().unwrap();
                self.download_child = Some(child);

                let log_lines = self.download_logs.clone();
                std::thread::spawn(move || {
                    let reader = std::io::BufReader::new(stdout);
                    for line in reader.lines() {
                        if let Ok(l) = line {
                            log_lines.lock().expect("download logs lock").push(l);
                        }
                    }
                });

                let log_lines_err = self.download_logs.clone();
                std::thread::spawn(move || {
                    let reader = std::io::BufReader::new(stderr);
                    for line in reader.lines() {
                        if let Ok(l) = line {
                            log_lines_err.lock().expect("download logs lock").push(l);
                        }
                    }
                });
            }
            Err(error) => {
                let message = format!("Failed to spawn setup_model.py: {}", error);
                self.download_logs
                    .lock()
                    .expect("download logs lock")
                    .push(message.clone());
                self.ui_overlay_status = Some(ModelStatus::Failed(message));
            }
        }
    }

    pub fn poll_health(&mut self) {
        if let Some(child) = &mut self.download_child {
            if let Ok(Some(status)) = child.try_wait() {
                if !status.success() {
                    self.ui_overlay_status =
                        Some(ModelStatus::Failed(format!("Server/Script failed: {status}")));
                } else {
                    self.ui_overlay_status = Some(ModelStatus::Ready);
                }
                self.download_child = None;
                return;
            }
        }

        if let ModelStatus::Downloading { total_gb, .. } = self.download_status_from_logs() {
            let logs = self.download_logs.lock().expect("download logs lock").clone();
            if let Some(last) = logs.last() {
                if last.starts_with("Downloading") {
                    self.ui_overlay_status = Some(ModelStatus::Downloading {
                        progress_pct: 5.0,
                        downloaded_gb: total_gb * 0.05,
                        total_gb,
                    });
                } else if last.starts_with("Extracting: ") {
                    let pct_str = last.replace("Extracting: ", "").replace('%', "");
                    if let Ok(pct) = pct_str.parse::<f32>() {
                        self.ui_overlay_status = Some(ModelStatus::Downloading {
                            progress_pct: 10.0 + (pct * 0.4),
                            downloaded_gb: total_gb * (0.1 + pct * 0.004),
                            total_gb,
                        });
                    }
                } else if last.starts_with("Packing layer ") {
                    let replaced = last.replace("Packing layer ", "");
                    let parts: Vec<&str> = replaced.split('/').collect();
                    if parts.len() == 2 {
                        if let (Ok(l), Ok(t)) = (parts[0].parse(), parts[1].parse()) {
                            self.ui_overlay_status = Some(ModelStatus::Packing {
                                layer: l,
                                total_layers: t,
                            });
                        }
                    }
                } else if last == "Complete" {
                    self.ui_overlay_status = Some(ModelStatus::Ready);
                }
            }
        } else if let ModelStatus::Packing { .. } = self.download_status_from_logs() {
            let logs = self.download_logs.lock().expect("download logs lock").clone();
            if let Some(last) = logs.last() {
                if last.starts_with("Packing layer ") {
                    let replaced = last.replace("Packing layer ", "");
                    let parts: Vec<&str> = replaced.split('/').collect();
                    if parts.len() == 2 {
                        if let (Ok(l), Ok(t)) = (parts[0].parse(), parts[1].parse()) {
                            self.ui_overlay_status = Some(ModelStatus::Packing {
                                layer: l,
                                total_layers: t,
                            });
                        }
                    }
                } else if last == "Complete" {
                    self.ui_overlay_status = Some(ModelStatus::Ready);
                }
            }
        }

        self.server
            .write()
            .expect("ssd moe server lock")
            .poll_starting_until_listen();
    }

    pub fn switch_model(&mut self, project_root: &Path, new_model: &ModelSpec) {
        if let Some(active) = &self.active_model {
            if active.id == new_model.id {
                let st = self.server.read().expect("ssd moe server lock").status();
                if matches!(st, ServerStatus::Ready | ServerStatus::Starting) {
                    return;
                }
            }
        }

        if Self::resolve_server_path(project_root)
            .or_else(|| SsdMoeServer::resolve_flash_moe_directory(None, None))
            .is_none()
        {
            return;
        }

        let out_dir = match SsdMoeServer::resolve_model_weights_path(new_model.model_dir_name) {
            Ok(path) => path,
            Err(message) => {
                self.ui_overlay_status = Some(ModelStatus::Failed(message));
                return;
            }
        };

        if out_dir.join("model_weights.bin").exists() && out_dir.join("packed_experts").exists() {
            self.stop();
            self.ensure_running(project_root, new_model);
        } else {
            self.download_model(project_root, new_model);
        }
    }

    pub fn stop(&mut self) {
        println!("SsdMoeManager::stop entered");
        self.test_status_override = None;
        if let Some(mut child) = self.download_child.take() {
            println!("SsdMoeManager killing download child");
            let pid = child.id();
            let _ = std::process::Command::new("kill")
                .arg("-15")
                .arg(pid.to_string())
                .output();

            let mut exited = false;
            for _ in 0..50 {
                if child.try_wait().unwrap_or(None).is_some() {
                    exited = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if !exited {
                let _ = child.kill();
            }
            let _ = child.wait();
            println!("SsdMoeManager download child killed");
        }
        println!("SsdMoeManager waiting on self.server.write()");
        self.server.write().expect("ssd moe server lock").stop();
        println!("SsdMoeManager self.server.write().stop() finished");
        self.ui_overlay_status = None;
    }
}

impl Drop for SsdMoeManager {
    fn drop(&mut self) {
        if self.stop_server_on_drop {
            self.server.write().expect("ssd moe server lock").stop();
        }
        if let Some(mut child) = self.download_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_is_stopped() {
        let server = SsdMoeManager::new_detached_for_test(59_995);
        assert_eq!(server.status(), ModelStatus::Ready);
        assert!(server.download_child.is_none());
    }

    #[test]
    fn api_base_matches_server_port() {
        const PORT: u16 = 59_992;
        let server = SsdMoeManager::new_detached_for_test(PORT);
        assert_eq!(
            server.api_base(),
            format!("http://127.0.0.1:{PORT}/v1")
        );
        assert_eq!(server.port(), PORT);
    }

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
        assert!(
            result
                .expect("some")
                .to_string_lossy()
                .contains("flash-moe")
        );
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
        assert!(!server.tcp_probe());
    }

    #[test]
    fn poll_health_noop_when_not_starting() {
        let mut server = SsdMoeManager::new_detached_for_test(59_994);
        server.poll_health();
        assert_eq!(server.status(), ModelStatus::Ready);
    }
}

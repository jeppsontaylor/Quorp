use collections::VecDeque;
use paths::{ensure_user_models_dir, user_models_dir};
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, RwLock};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerStatus {
    Stopped,
    Starting,
    Ready,
    Failed(String),
}

pub struct SsdMoeServer {
    status: Arc<RwLock<ServerStatus>>,
    log_lines: Arc<RwLock<VecDeque<String>>>,
    child_process: Arc<RwLock<Option<Child>>>,
    port: u16,
    log_file_path: PathBuf,
}

impl SsdMoeServer {
    pub fn new(port: u16) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let log_file_path = PathBuf::from(home).join(".config/quorp/logs/ssd-moe.log");
        Self {
            status: Arc::new(RwLock::new(ServerStatus::Stopped)),
            log_lines: Arc::new(RwLock::new(VecDeque::with_capacity(500))),
            child_process: Arc::new(RwLock::new(None)),
            port,
            log_file_path,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn status(&self) -> ServerStatus {
        self.status.read().unwrap().clone()
    }

    pub fn get_logs(&self) -> Vec<String> {
        self.log_lines.read().unwrap().iter().cloned().collect()
    }

    pub fn log_file_path(&self) -> &Path {
        &self.log_file_path
    }

    /// Resolve the flash-moe checkout: explicit setting, then `project_root/external/flash-moe`, then paths near the executable.
    pub fn resolve_flash_moe_directory(
        configured_path: Option<&String>,
        project_root: Option<&Path>,
    ) -> Option<PathBuf> {
        if let Some(path) = configured_path {
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
        if let Some(root) = project_root {
            let candidate = root.join("external/flash-moe");
            if candidate.join("infer").exists() {
                return Some(candidate);
            }
        }
        if let Ok(exe) = env::current_exe() {
            if let Some(parent) = exe.parent() {
                let mut path = parent.join("../../external/flash-moe");
                if path.exists() {
                    return Some(path.canonicalize().unwrap_or(path));
                }

                path = parent.join("external/flash-moe");
                if path.exists() {
                    return Some(path.canonicalize().unwrap_or(path));
                }

                path = parent.join("../Resources/external/flash-moe");
                if path.exists() {
                    return Some(path.canonicalize().unwrap_or(path));
                }
            }
        }
        None
    }

    pub fn resolve_server_path(&self, configured_path: Option<&String>) -> Option<PathBuf> {
        Self::resolve_flash_moe_directory(configured_path, None)
    }

    /// Resolves SSD-MOE weight directory: relative names live under [`user_models_dir`]; absolute
    /// `model_dir` is used as-is. Ensures the user models root exists when using the default layout.
    pub fn resolve_model_weights_path(model_dir: &str) -> Result<PathBuf, String> {
        if let Err(error) = ensure_user_models_dir() {
            let attempted = user_models_dir();
            log::warn!(
                "Could not create user models directory at {:?}: {}",
                attempted,
                error
            );
            return Err(format!(
                "Could not create user models directory at {:?}: {}",
                attempted, error
            ));
        }

        let path = if Path::new(model_dir).is_absolute() {
            PathBuf::from(model_dir)
        } else {
            user_models_dir().join(model_dir)
        };

        Ok(path.canonicalize().unwrap_or(path))
    }

    pub fn start(
        &mut self,
        server_path: Option<String>,
        port: u16,
        model_dir: &str,
        k_experts: u32,
        project_root: Option<&Path>,
    ) {
        if *self.status.read().unwrap() == ServerStatus::Starting
            || *self.status.read().unwrap() == ServerStatus::Ready
        {
            return;
        }

        self.port = port;
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            log::info!("[SSD-MOE] Found existing server on port {}", port);
            *self.status.write().unwrap() = ServerStatus::Ready;
            return;
        }

        let flash_moe_dir = match Self::resolve_flash_moe_directory(
            server_path.as_ref(),
            project_root,
        ) {
            Some(dir) => dir,
            None => {
                let msg =
                    "Could not locate flash-moe directory. Please configure server_path.".to_string();
                log::error!("[SSD-MOE] {}", msg);
                *self.status.write().unwrap() = ServerStatus::Failed(msg);
                return;
            }
        };

        let infer_bin = flash_moe_dir.join("infer");
        if !infer_bin.exists() {
            let msg = format!(
                "'infer' binary not found at {:?}. Make sure to build it first.",
                infer_bin
            );
            log::error!("[SSD-MOE] {}", msg);
            *self.status.write().unwrap() = ServerStatus::Failed(msg);
            return;
        }

        let model_weights_path = match Self::resolve_model_weights_path(model_dir) {
            Ok(path) => path,
            Err(msg) => {
                log::error!("[SSD-MOE] {}", msg);
                *self.status.write().unwrap() = ServerStatus::Failed(msg);
                return;
            }
        };

        if !model_weights_path.join("packed_experts").exists() {
            let msg = format!(
                "SSD-MOE model weights not found at {:?}. Download or place packed weights there (expected packed_experts/).",
                model_weights_path
            );
            log::error!("[SSD-MOE] {}", msg);
            *self.status.write().unwrap() = ServerStatus::Failed(msg);
            return;
        }

        let Some(model_path_str) = model_weights_path.to_str() else {
            let msg = "SSD-MOE model path is not valid UTF-8".to_string();
            log::error!("[SSD-MOE] {}", msg);
            *self.status.write().unwrap() = ServerStatus::Failed(msg);
            return;
        };

        log::info!(
            "[SSD-MOE] Starting child process: {:?} locally at {:?} (model {:?}, k={})",
            infer_bin,
            flash_moe_dir,
            model_weights_path,
            k_experts
        );
        *self.status.write().unwrap() = ServerStatus::Starting;
        self.log_lines.write().unwrap().clear();
        self.append_log(&format!(
            "Starting SSD-MOE server at {:?} on port {}",
            infer_bin, port
        ));

        let mut cmd = Command::new(&infer_bin);
        cmd.current_dir(&flash_moe_dir)
            .args([
                "--model",
                model_path_str,
                "--serve",
                &port.to_string(),
                "--k",
                &k_experts.to_string(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        match cmd.spawn() {
            Ok(mut child) => {
                let stdout = child.stdout.take().unwrap();
                let stderr = child.stderr.take().unwrap();

                let log_lines_out = self.log_lines.clone();
                let log_file_out = self.log_file_path.clone();
                std::thread::spawn(move || {
                    let reader = std::io::BufReader::new(stdout);
                    use std::io::BufRead;
                    for line in reader.lines() {
                        if let Ok(line) = line {
                            append_log_line(
                                &log_lines_out,
                                &log_file_out,
                                format!("[stdout] {}", line),
                            );
                        }
                    }
                });

                let log_lines_err = self.log_lines.clone();
                let log_file_err = self.log_file_path.clone();
                std::thread::spawn(move || {
                    let reader = std::io::BufReader::new(stderr);
                    use std::io::BufRead;
                    for line in reader.lines() {
                        if let Ok(line) = line {
                            append_log_line(
                                &log_lines_err,
                                &log_file_err,
                                format!("[stderr] {}", line),
                            );
                        }
                    }
                });

                *self.child_process.write().unwrap() = Some(child);
            }
            Err(e) => {
                let msg = format!("Failed to spawn infer: {}", e);
                log::error!("[SSD-MOE] {}", msg);
                *self.status.write().unwrap() = ServerStatus::Failed(msg);
                return;
            }
        }

        let status = self.status.clone();
        std::thread::spawn(move || {
            let mut attempts = 0;
            loop {
                std::thread::sleep(Duration::from_secs(2));
                if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok() {
                    *status.write().unwrap() = ServerStatus::Ready;
                    log::info!("[SSD-MOE] Server is ready on port {}", port);
                    break;
                }
                attempts += 1;
                if attempts > 60 {
                    *status.write().unwrap() =
                        ServerStatus::Failed("Server failed to start within 2 minutes".into());
                    break;
                }
            }
        });
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.child_process.write().unwrap().take() {
            let _ = child.kill();
        }
        *self.status.write().unwrap() = ServerStatus::Stopped;
    }

    /// If something is already listening on this server's port, treat it as ready (no child spawned).
    pub fn adopt_listener_if_present(&mut self) -> bool {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.port));
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            *self.status.write().unwrap() = ServerStatus::Ready;
            true
        } else {
            false
        }
    }

    /// While status is `Starting`, flip to `Ready` as soon as the listen port accepts connections.
    pub fn poll_starting_until_listen(&mut self) {
        if *self.status.read().unwrap() != ServerStatus::Starting {
            return;
        }
        let addr = SocketAddr::from(([127, 0, 0, 1], self.port));
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok() {
            *self.status.write().unwrap() = ServerStatus::Ready;
            log::info!("[SSD-MOE] Server is ready on port {}", self.port);
        }
    }

    fn append_log(&self, msg: &str) {
        append_log_line(&self.log_lines, &self.log_file_path, msg.to_string());
    }
}

fn append_log_line(log_lines: &Arc<RwLock<VecDeque<String>>>, file_path: &Path, msg: String) {
    let mut logs = log_lines.write().unwrap();
    if logs.len() >= 500 {
        logs.pop_front();
    }
    logs.push_back(msg.clone());
    drop(logs);

    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(file_path)
    {
        let _ = writeln!(file, "{}", msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paths::QUORP_USER_MODELS_DIR_VAR;
    use std::sync::Mutex;

    static QUORP_MODELS_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_ssd_moe_server_initial_state() {
        let server = SsdMoeServer::new(8080);
        assert_eq!(server.status(), ServerStatus::Stopped);
        assert!(server.get_logs().is_empty());
        assert!(server.log_file_path().to_string_lossy().contains("ssd-moe.log"));
    }

    #[test]
    fn test_resolve_server_path_fallback() {
        let server = SsdMoeServer::new(8080);
        let resolved = server.resolve_server_path(None);
        if let Some(path) = resolved {
            assert!(path.to_string_lossy().contains("flash-moe"));
        }
    }

    #[test]
    fn test_resolve_server_path_configured() {
        let server = SsdMoeServer::new(8080);
        let configured = Some("/custom/path/to/flash-moe".to_string());
        let resolved = server.resolve_server_path(configured.as_ref());
        assert_eq!(resolved, Some(PathBuf::from("/custom/path/to/flash-moe")));
    }

    #[test]
    fn resolve_prefers_project_external_flash_moe() {
        let temp = tempfile::tempdir().expect("tempdir");
        let flash_dir = temp.path().join("external/flash-moe");
        std::fs::create_dir_all(&flash_dir).expect("mkdir");
        std::fs::write(flash_dir.join("infer"), "#!/bin/sh\n").expect("write");
        let resolved = SsdMoeServer::resolve_flash_moe_directory(None, Some(temp.path()));
        assert_eq!(resolved, Some(flash_dir));
    }

    #[test]
    fn resolve_model_weights_path_relative_uses_quorp_override() {
        let _guard = QUORP_MODELS_ENV_LOCK.lock().expect("env lock");
        let temp = tempfile::tempdir().expect("tempdir");
        // SAFETY: `QUORP_MODELS_ENV_LOCK` serializes env mutation for these tests.
        unsafe {
            std::env::set_var(QUORP_USER_MODELS_DIR_VAR, temp.path());
        }
        let resolved = SsdMoeServer::resolve_model_weights_path("out_35b").expect("resolve");
        assert_eq!(resolved, temp.path().join("out_35b"));
        unsafe {
            std::env::remove_var(QUORP_USER_MODELS_DIR_VAR);
        }
    }

    #[test]
    fn resolve_model_weights_path_absolute_bypasses_basename_join() {
        let _guard = QUORP_MODELS_ENV_LOCK.lock().expect("env lock");
        let temp = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var(QUORP_USER_MODELS_DIR_VAR, temp.path());
        }
        let custom = tempfile::tempdir().expect("tempdir2");
        let abs = custom.path().join("my_weights");
        std::fs::create_dir_all(&abs).expect("mkdir");
        let resolved =
            SsdMoeServer::resolve_model_weights_path(abs.to_str().expect("utf8")).expect("resolve");
        assert_eq!(
            resolved,
            abs.canonicalize().unwrap_or_else(|_| abs.clone())
        );
        unsafe {
            std::env::remove_var(QUORP_USER_MODELS_DIR_VAR);
        }
    }
}

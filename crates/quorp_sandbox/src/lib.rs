//! Sandbox backends for agent and benchmark runs.

#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;
use quorp_config::load_settings;
use quorp_core::{
    ContainerEnginePreference, SandboxMode, SandboxRuntimeProfile, SandboxRuntimeSettings,
};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use walkdir::{DirEntry, WalkDir};

#[derive(Debug, Clone)]
pub struct SandboxRequest {
    pub source_workspace: PathBuf,
    pub run_id: String,
    pub attempt: usize,
    pub mode: SandboxMode,
    pub keep_sandbox: bool,
}

#[derive(Debug)]
pub struct SandboxLease {
    workspace_path: PathBuf,
    sandbox_root: PathBuf,
    mode: SandboxMode,
    backend: SandboxBackend,
    source_workspace: PathBuf,
    keep_sandbox: bool,
    _temp_dir: Option<TempDir>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxBackend {
    Host,
    GitWorktree,
    TmpCopy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxCommandPlan {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub current_dir: PathBuf,
    pub host_environment: Vec<(OsString, OsString)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxMount {
    pub source: PathBuf,
    pub target: PathBuf,
    pub writable: bool,
}

impl SandboxMount {
    pub fn bind(source: impl Into<PathBuf>, target: impl Into<PathBuf>, writable: bool) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            writable,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SandboxCommandSpec<'a> {
    pub program: &'a OsStr,
    pub args: &'a [&'a OsStr],
    pub current_dir: &'a Path,
    pub runtime: &'a SandboxRuntimeSettings,
    pub policy: &'a SandboxPolicy,
    pub extra_environment: &'a [(&'a str, &'a OsStr)],
    pub additional_mounts: &'a [SandboxMount],
    pub interactive: bool,
}

impl SandboxCommandPlan {
    pub fn apply_to_command<C: SandboxCommandEnvironment>(&self, command: &mut C) {
        command.clear_env();
        for (key, value) in &self.host_environment {
            command.set_env(key, value);
        }
        for arg in &self.args {
            command.add_arg(arg);
        }
        command.set_current_dir(&self.current_dir);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxPolicy {
    pub allowed_environment_variables: Vec<String>,
    pub allowed_environment_prefixes: Vec<String>,
    pub denied_environment_variables: Vec<String>,
    pub default_shell: String,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            allowed_environment_variables: vec![
                "HOME".into(),
                "PATH".into(),
                "SHELL".into(),
                "TERM".into(),
                "TMPDIR".into(),
                "TMP".into(),
                "TEMP".into(),
                "USER".into(),
                "LOGNAME".into(),
                "LANG".into(),
                "LC_ALL".into(),
                "CARGO_HOME".into(),
                "RUSTUP_HOME".into(),
                "RUSTFLAGS".into(),
                "XDG_CACHE_HOME".into(),
                "XDG_CONFIG_HOME".into(),
                "XDG_DATA_HOME".into(),
                "XDG_RUNTIME_DIR".into(),
                "CI".into(),
                "PWD".into(),
            ],
            allowed_environment_prefixes: vec!["QUORP_".into()],
            denied_environment_variables: vec![
                "ALL_PROXY".into(),
                "HTTP_PROXY".into(),
                "HTTPS_PROXY".into(),
                "NO_PROXY".into(),
                "SSH_AUTH_SOCK".into(),
                "GIT_SSH_COMMAND".into(),
            ],
            default_shell: "/bin/sh".into(),
        }
    }
}

pub trait SandboxCommandEnvironment {
    fn clear_env(&mut self);
    fn set_env(&mut self, key: &OsStr, value: &OsStr);
    fn remove_env(&mut self, key: &OsStr);
    fn add_arg(&mut self, arg: &OsStr);
    fn set_current_dir(&mut self, path: &Path);
}

impl SandboxCommandEnvironment for std::process::Command {
    fn clear_env(&mut self) {
        self.env_clear();
    }

    fn set_env(&mut self, key: &OsStr, value: &OsStr) {
        self.env(key, value);
    }

    fn remove_env(&mut self, key: &OsStr) {
        self.env_remove(key);
    }

    fn add_arg(&mut self, arg: &OsStr) {
        self.arg(arg);
    }

    fn set_current_dir(&mut self, path: &Path) {
        self.current_dir(path);
    }
}

impl SandboxCommandEnvironment for portable_pty::CommandBuilder {
    fn clear_env(&mut self) {
        self.env_clear();
    }

    fn set_env(&mut self, key: &OsStr, value: &OsStr) {
        self.env(key, value);
    }

    fn remove_env(&mut self, key: &OsStr) {
        self.env_remove(key);
    }

    fn add_arg(&mut self, arg: &OsStr) {
        self.arg(arg);
    }

    fn set_current_dir(&mut self, path: &Path) {
        self.cwd(path);
    }
}

impl SandboxLease {
    pub fn workspace_path(&self) -> &Path {
        &self.workspace_path
    }

    pub fn sandbox_root(&self) -> &Path {
        &self.sandbox_root
    }

    pub fn mode(&self) -> SandboxMode {
        self.mode
    }

    pub fn backend(&self) -> SandboxBackend {
        self.backend
    }

    pub fn source_workspace(&self) -> &Path {
        &self.source_workspace
    }
}

impl Drop for SandboxLease {
    fn drop(&mut self) {
        if self.keep_sandbox || self.backend != SandboxBackend::GitWorktree {
            return;
        }
        if let Err(error) = remove_git_worktree(&self.source_workspace, &self.workspace_path) {
            log::warn!(
                "failed to remove git worktree sandbox {}: {error:#}",
                self.workspace_path.display()
            );
        }
    }
}

pub fn create_sandbox(request: SandboxRequest) -> anyhow::Result<SandboxLease> {
    match request.mode {
        SandboxMode::Host => Ok(SandboxLease {
            workspace_path: request.source_workspace.clone(),
            sandbox_root: request.source_workspace.clone(),
            mode: SandboxMode::Host,
            backend: SandboxBackend::Host,
            source_workspace: request.source_workspace,
            keep_sandbox: request.keep_sandbox,
            _temp_dir: None,
        }),
        SandboxMode::TmpCopy => create_isolated_sandbox(request),
    }
}

pub fn create_isolated_sandbox(request: SandboxRequest) -> anyhow::Result<SandboxLease> {
    if source_is_git_worktree(&request.source_workspace) {
        match create_git_worktree_sandbox(request.clone()) {
            Ok(lease) => return Ok(lease),
            Err(error) => {
                log_sandbox_fallback(&request.source_workspace, &error);
            }
        }
    }
    create_tmp_copy_sandbox(request)
}

pub fn create_git_worktree_sandbox(request: SandboxRequest) -> anyhow::Result<SandboxLease> {
    let temp_root = Path::new("/tmp").join("quorp");
    fs::create_dir_all(&temp_root)
        .with_context(|| format!("failed to create {}", temp_root.display()))?;
    let prefix = format!(
        "{}-attempt-{}-worktree-",
        sanitize_path_component(&request.run_id),
        request.attempt
    );
    let temp_dir = tempfile::Builder::new()
        .prefix(&prefix)
        .tempdir_in(&temp_root)
        .with_context(|| format!("failed to create sandbox under {}", temp_root.display()))?;
    let sandbox_root = temp_dir.path().to_path_buf();
    let workspace_path = sandbox_root.join("workspace");
    let branch_name = format!(
        "quorp/{}-attempt-{}",
        sanitize_path_component(&request.run_id),
        request.attempt
    );
    run_git(
        &request.source_workspace,
        &[
            "worktree",
            "add",
            "--detach",
            workspace_path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("non-utf8 sandbox path"))?,
            "HEAD",
        ],
    )
    .with_context(|| {
        format!(
            "failed to create git worktree sandbox `{branch_name}` from {}",
            request.source_workspace.display()
        )
    })?;
    let temp_dir = if request.keep_sandbox {
        let path = temp_dir.keep();
        return Ok(SandboxLease {
            workspace_path,
            sandbox_root: path,
            mode: SandboxMode::TmpCopy,
            backend: SandboxBackend::GitWorktree,
            source_workspace: request.source_workspace,
            keep_sandbox: true,
            _temp_dir: None,
        });
    } else {
        Some(temp_dir)
    };
    Ok(SandboxLease {
        workspace_path,
        sandbox_root,
        mode: SandboxMode::TmpCopy,
        backend: SandboxBackend::GitWorktree,
        source_workspace: request.source_workspace,
        keep_sandbox: request.keep_sandbox,
        _temp_dir: temp_dir,
    })
}

pub fn create_tmp_copy_sandbox(request: SandboxRequest) -> anyhow::Result<SandboxLease> {
    let temp_root = Path::new("/tmp").join("quorp");
    fs::create_dir_all(&temp_root)
        .with_context(|| format!("failed to create {}", temp_root.display()))?;
    let prefix = format!(
        "{}-attempt-{}-",
        sanitize_path_component(&request.run_id),
        request.attempt
    );
    let temp_dir = tempfile::Builder::new()
        .prefix(&prefix)
        .tempdir_in(&temp_root)
        .with_context(|| format!("failed to create sandbox under {}", temp_root.display()))?;
    let sandbox_root = temp_dir.path().to_path_buf();
    let workspace_path = sandbox_root.join("workspace");
    copy_workspace(&request.source_workspace, &workspace_path)?;
    let temp_dir = if request.keep_sandbox {
        let path = temp_dir.keep();
        return Ok(SandboxLease {
            workspace_path,
            sandbox_root: path,
            mode: SandboxMode::TmpCopy,
            backend: SandboxBackend::TmpCopy,
            source_workspace: request.source_workspace,
            keep_sandbox: true,
            _temp_dir: None,
        });
    } else {
        Some(temp_dir)
    };
    Ok(SandboxLease {
        workspace_path,
        sandbox_root,
        mode: SandboxMode::TmpCopy,
        backend: SandboxBackend::TmpCopy,
        source_workspace: request.source_workspace,
        keep_sandbox: request.keep_sandbox,
        _temp_dir: temp_dir,
    })
}

pub fn default_policy() -> SandboxPolicy {
    SandboxPolicy::default()
}

pub fn apply_sandbox_policy<C>(
    command: &mut C,
    policy: &SandboxPolicy,
    extra_environment: &[(&str, &OsStr)],
) where
    C: SandboxCommandEnvironment,
{
    let preserved_env = capture_allowed_environment(policy);
    command.clear_env();
    for (key, value) in preserved_env {
        command.set_env(OsStr::new(&key), OsStr::new(&value));
    }
    for (key, value) in extra_environment {
        if !is_denylisted_env_key(key, policy) {
            command.set_env(OsStr::new(key), value);
        }
    }
}

pub fn sandbox_runtime_for_path(path: &Path) -> anyhow::Result<SandboxRuntimeSettings> {
    for ancestor in path.ancestors() {
        if ancestor.join(".quorp").join("settings.json").exists() {
            return Ok(load_settings(ancestor)?.settings.sandbox.runtime);
        }
    }
    Ok(SandboxRuntimeSettings::default())
}

#[allow(clippy::too_many_arguments)]
pub fn build_command_plan(spec: SandboxCommandSpec<'_>) -> anyhow::Result<SandboxCommandPlan> {
    let current_dir =
        fs::canonicalize(spec.current_dir).unwrap_or_else(|_| spec.current_dir.to_path_buf());
    let mut host_environment = capture_allowed_environment(spec.policy);
    let extra_environment = capture_extra_environment(spec.policy, spec.extra_environment);
    match spec.runtime.profile {
        SandboxRuntimeProfile::Local => {
            host_environment.extend(extra_environment);
            Ok(SandboxCommandPlan {
                program: spec.program.to_os_string(),
                args: spec.args.iter().map(|arg| (*arg).to_os_string()).collect(),
                current_dir,
                host_environment: host_environment
                    .into_iter()
                    .map(|(key, value)| (OsString::from(key), OsString::from(value)))
                    .collect(),
            })
        }
        SandboxRuntimeProfile::Container => {
            let container_environment = container_environment_with_extras(
                spec.policy,
                current_dir.as_path(),
                &extra_environment,
            );
            let container_program =
                resolve_container_runtime_program(&spec.runtime.container.engine)?;
            Ok(build_container_command_plan(
                spec,
                container_program,
                current_dir,
                container_environment,
                host_environment,
            ))
        }
    }
}

fn build_container_command_plan(
    spec: SandboxCommandSpec<'_>,
    container_program: OsString,
    current_dir: PathBuf,
    container_environment: Vec<(String, String)>,
    host_environment: Vec<(String, String)>,
) -> SandboxCommandPlan {
    let mut container_args = Vec::new();
    container_args.push(OsString::from("run"));
    container_args.push(OsString::from("--rm"));
    container_args.push(OsString::from("--network=none"));
    container_args.push(OsString::from("--cap-drop=ALL"));
    container_args.push(OsString::from("--security-opt"));
    container_args.push(OsString::from("no-new-privileges"));
    container_args.push(OsString::from("--mount"));
    container_args.push(OsString::from(format!(
        "type=bind,source={},target={},rw",
        current_dir.display(),
        current_dir.display()
    )));
    for mount in spec.additional_mounts {
        container_args.push(OsString::from("--mount"));
        container_args.push(OsString::from(format!(
            "type=bind,source={},target={},{}",
            mount.source.display(),
            mount.target.display(),
            if mount.writable { "rw" } else { "ro" }
        )));
    }
    container_args.push(OsString::from("--workdir"));
    container_args.push(OsString::from(current_dir.as_os_str()));
    #[cfg(unix)]
    {
        if let Some(user) = container_user_argument() {
            container_args.push(OsString::from("--user"));
            container_args.push(OsString::from(user));
        }
    }
    if spec.interactive {
        container_args.push(OsString::from("-it"));
    } else {
        container_args.push(OsString::from("-i"));
    }
    for (key, value) in container_environment {
        container_args.push(OsString::from("--env"));
        container_args.push(OsString::from(format!("{key}={value}")));
    }
    container_args.push(OsString::from(&spec.runtime.container.image));
    container_args.push(spec.program.to_os_string());
    for arg in spec.args {
        container_args.push((*arg).to_os_string());
    }
    SandboxCommandPlan {
        program: container_program,
        args: container_args,
        current_dir,
        host_environment: host_environment
            .into_iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect(),
    }
}

fn capture_extra_environment(
    policy: &SandboxPolicy,
    extra_environment: &[(&str, &OsStr)],
) -> Vec<(String, String)> {
    let mut preserved = Vec::new();
    for (key, value) in extra_environment {
        if !is_denylisted_env_key(key, policy) {
            preserved.push(((*key).to_string(), value.to_string_lossy().into_owned()));
        }
    }
    preserved
}

fn container_environment(policy: &SandboxPolicy, current_dir: &Path) -> Vec<(String, String)> {
    let mut environment = BTreeMap::new();
    environment.insert(
        "PATH".to_string(),
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
    );
    environment.insert("HOME".to_string(), "/tmp/quorp-home".to_string());
    environment.insert("CARGO_HOME".to_string(), "/tmp/quorp-cargo".to_string());
    environment.insert("RUSTUP_HOME".to_string(), "/tmp/quorp-rustup".to_string());
    environment.insert("XDG_CACHE_HOME".to_string(), "/tmp/quorp-cache".to_string());
    environment.insert(
        "XDG_CONFIG_HOME".to_string(),
        "/tmp/quorp-config".to_string(),
    );
    environment.insert("XDG_DATA_HOME".to_string(), "/tmp/quorp-data".to_string());
    environment.insert(
        "XDG_RUNTIME_DIR".to_string(),
        "/tmp/quorp-runtime".to_string(),
    );
    environment.insert("TMPDIR".to_string(), "/tmp".to_string());
    environment.insert("TMP".to_string(), "/tmp".to_string());
    environment.insert("TEMP".to_string(), "/tmp".to_string());
    environment.insert("SHELL".to_string(), policy.default_shell.clone());
    environment.insert("PWD".to_string(), current_dir.display().to_string());
    if let Some(term) = std::env::var_os("TERM") {
        environment.insert("TERM".to_string(), term.to_string_lossy().into_owned());
    }
    for key in ["LANG", "LC_ALL", "CI", "USER", "LOGNAME"] {
        if let Some(value) = std::env::var_os(key)
            && !is_denylisted_env_key(key, policy)
        {
            environment.insert(key.to_string(), value.to_string_lossy().into_owned());
        }
    }
    for (key, value) in std::env::vars_os() {
        let key_string = key.to_string_lossy();
        if key_string.starts_with("QUORP_") && !is_denylisted_env_key(&key_string, policy) {
            environment.insert(
                key_string.into_owned(),
                value.to_string_lossy().into_owned(),
            );
        }
    }
    environment.into_iter().collect()
}

fn container_environment_with_extras(
    policy: &SandboxPolicy,
    current_dir: &Path,
    extra_environment: &[(String, String)],
) -> Vec<(String, String)> {
    let mut environment = container_environment(policy, current_dir);
    environment.extend(extra_environment.iter().cloned());
    environment
}

fn resolve_container_runtime_program(
    engine: &ContainerEnginePreference,
) -> anyhow::Result<OsString> {
    match engine {
        ContainerEnginePreference::Auto => {
            if is_container_runtime_available("podman") {
                Ok(OsString::from("podman"))
            } else if is_container_runtime_available("docker") {
                Ok(OsString::from("docker"))
            } else {
                Err(anyhow::anyhow!(
                    "container sandbox requested but neither `podman` nor `docker` is available"
                ))
            }
        }
        ContainerEnginePreference::Docker => {
            if is_container_runtime_available("docker") {
                Ok(OsString::from("docker"))
            } else {
                Err(anyhow::anyhow!(
                    "container sandbox requested but `docker` is not available"
                ))
            }
        }
        ContainerEnginePreference::Podman => {
            if is_container_runtime_available("podman") {
                Ok(OsString::from("podman"))
            } else {
                Err(anyhow::anyhow!(
                    "container sandbox requested but `podman` is not available"
                ))
            }
        }
    }
}

fn is_container_runtime_available(program: &str) -> bool {
    #[allow(clippy::disallowed_methods)]
    Command::new(program).arg("--version").output().is_ok()
}

#[cfg(unix)]
fn container_user_argument() -> Option<String> {
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    Some(format!("{uid}:{gid}"))
}

#[cfg(not(unix))]
fn container_user_argument() -> Option<String> {
    None
}

pub fn sanitize_env_name(name: &str) -> bool {
    util::redact::should_redact(name)
}

fn capture_allowed_environment(policy: &SandboxPolicy) -> Vec<(String, String)> {
    let mut preserved = Vec::new();
    for key in &policy.allowed_environment_variables {
        if let Some(value) = std::env::var_os(key) {
            preserved.push((key.clone(), value.to_string_lossy().into_owned()));
        }
    }
    for (key, value) in std::env::vars_os() {
        let key_string = key.to_string_lossy();
        if policy
            .allowed_environment_prefixes
            .iter()
            .any(|prefix| key_string.starts_with(prefix))
            && !is_denylisted_env_key(&key_string, policy)
        {
            preserved.push((
                key_string.into_owned(),
                value.to_string_lossy().into_owned(),
            ));
        }
    }
    if preserved.iter().all(|(key, _)| key.as_str() != "SHELL") {
        preserved.push(("SHELL".to_string(), policy.default_shell.clone()));
    }
    if preserved.iter().all(|(key, _)| key.as_str() != "PATH") {
        preserved.push((
            "PATH".to_string(),
            "/usr/bin:/bin:/usr/sbin:/sbin".to_string(),
        ));
    }
    preserved
}

fn is_denylisted_env_key(key: &str, policy: &SandboxPolicy) -> bool {
    policy
        .denied_environment_variables
        .iter()
        .any(|denied| denied.eq_ignore_ascii_case(key))
        || util::redact::should_redact(key)
}

fn remove_git_worktree(source_workspace: &Path, workspace_path: &Path) -> anyhow::Result<()> {
    run_git(
        source_workspace,
        &[
            "worktree",
            "remove",
            "--force",
            workspace_path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("non-utf8 sandbox path"))?,
        ],
    )
    .with_context(|| {
        format!(
            "failed to remove git worktree {} from {}",
            workspace_path.display(),
            source_workspace.display()
        )
    })?;
    run_git(source_workspace, &["worktree", "prune"])?;
    Ok(())
}

fn source_is_git_worktree(source: &Path) -> bool {
    source.join(".git").exists() || run_git(source, &["rev-parse", "--is-inside-work-tree"]).is_ok()
}

fn run_git(cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
    #[allow(clippy::disallowed_methods)]
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn log_sandbox_fallback(source: &Path, error: &anyhow::Error) {
    log::warn!(
        "falling back from git-worktree sandbox to tmp-copy for {}: {error:#}",
        source.display()
    );
}

fn copy_workspace(source: &Path, destination: &Path) -> anyhow::Result<()> {
    let source = fs::canonicalize(source)
        .with_context(|| format!("failed to canonicalize {}", source.display()))?;
    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    for entry in WalkDir::new(&source)
        .into_iter()
        .filter_entry(include_entry)
    {
        let entry = entry.with_context(|| format!("failed to walk {}", source.display()))?;
        let relative = entry
            .path()
            .strip_prefix(&source)
            .with_context(|| format!("failed to strip {}", source.display()))?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)
                .with_context(|| format!("failed to create {}", target.display()))?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            fs::copy(entry.path(), &target).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    entry.path().display(),
                    target.display()
                )
            })?;
        } else if entry.file_type().is_symlink() {
            copy_symlink(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn include_entry(entry: &DirEntry) -> bool {
    let Some(name) = entry.file_name().to_str() else {
        return true;
    };
    !matches!(
        name,
        ".git" | "target" | ".quorp-runs" | ".DS_Store" | "node_modules"
    )
}

#[cfg(unix)]
fn copy_symlink(source: &Path, target: &Path) -> anyhow::Result<()> {
    let link_target = fs::read_link(source)
        .with_context(|| format!("failed to read symlink {}", source.display()))?;
    std::os::unix::fs::symlink(link_target, target)
        .with_context(|| format!("failed to create symlink {}", target.display()))
}

#[cfg(not(unix))]
fn copy_symlink(source: &Path, target: &Path) -> anyhow::Result<()> {
    let metadata = fs::metadata(source)
        .with_context(|| format!("failed to inspect symlink target {}", source.display()))?;
    if metadata.is_dir() {
        fs::create_dir_all(target).with_context(|| format!("failed to create {}", target.display()))
    } else {
        fs::copy(source, target)
            .with_context(|| format!("failed to copy {}", source.display()))
            .map(|_| ())
    }
}

fn sanitize_path_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    static TEST_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    #[test]
    fn tmp_copy_sandbox_leaves_source_untouched() {
        let source = tempfile::tempdir().expect("source tempdir");
        let source_file = source.path().join("src.txt");
        fs::write(&source_file, "original").expect("write source");

        let lease = create_sandbox(SandboxRequest {
            source_workspace: source.path().to_path_buf(),
            run_id: "run/one".to_string(),
            attempt: 1,
            mode: SandboxMode::TmpCopy,
            keep_sandbox: false,
        })
        .expect("sandbox");

        fs::write(lease.workspace_path().join("src.txt"), "changed").expect("write sandbox");

        assert_eq!(
            fs::read_to_string(source_file).expect("read source"),
            "original"
        );
        assert_eq!(
            fs::read_to_string(lease.workspace_path().join("src.txt")).expect("read sandbox"),
            "changed"
        );
        assert_eq!(lease.backend(), SandboxBackend::TmpCopy);
    }

    #[test]
    fn git_worktree_sandbox_leaves_source_untouched() {
        #[allow(clippy::disallowed_methods)]
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let source = tempfile::tempdir().expect("source tempdir");
        fs::write(source.path().join("src.txt"), "original").expect("write source");
        run_git(source.path(), &["init"]).expect("git init");
        run_git(source.path(), &["config", "user.email", "test@example.com"]).expect("email");
        run_git(source.path(), &["config", "user.name", "Test User"]).expect("name");
        run_git(source.path(), &["add", "src.txt"]).expect("add");
        run_git(source.path(), &["commit", "-m", "initial"]).expect("commit");

        let lease = create_sandbox(SandboxRequest {
            source_workspace: source.path().to_path_buf(),
            run_id: "run/git".to_string(),
            attempt: 1,
            mode: SandboxMode::TmpCopy,
            keep_sandbox: false,
        })
        .expect("sandbox");

        fs::write(lease.workspace_path().join("src.txt"), "changed").expect("write sandbox");

        assert_eq!(lease.backend(), SandboxBackend::GitWorktree);
        assert_eq!(
            fs::read_to_string(source.path().join("src.txt")).expect("read source"),
            "original"
        );
        assert_eq!(
            fs::read_to_string(lease.workspace_path().join("src.txt")).expect("read sandbox"),
            "changed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn sandbox_policy_scrubs_secret_environment_values() {
        let guard = TEST_ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("lock");
        let previous = std::env::var_os("QUORP_TEST_SECRET");
        unsafe {
            std::env::set_var("QUORP_TEST_SECRET", "top-secret");
            std::env::set_var("QUORP_TEST_SAFE", "visible");
        }

        let mut command = Command::new("sh");
        command.arg("-lc").arg(
            "printf '%s|%s|%s' \"${QUORP_TEST_SECRET:-}\" \"${QUORP_TEST_SAFE:-}\" \"${PATH:-}\"",
        );
        apply_sandbox_policy(&mut command, &SandboxPolicy::default(), &[]);
        let output = command.output().expect("spawn");
        let rendered = String::from_utf8_lossy(&output.stdout).to_string();

        unsafe {
            match previous {
                Some(value) => std::env::set_var("QUORP_TEST_SECRET", value),
                None => std::env::remove_var("QUORP_TEST_SECRET"),
            }
            std::env::remove_var("QUORP_TEST_SAFE");
        }
        drop(guard);

        let segments = rendered.split('|').collect::<Vec<_>>();
        assert_eq!(segments[0], "");
        assert_eq!(segments[1], "visible");
        assert!(!segments[2].is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn dropping_git_worktree_sandbox_removes_registered_worktree() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let source = tempfile::tempdir().expect("source tempdir");
        fs::write(source.path().join("src.txt"), "original").expect("write source");
        run_git(source.path(), &["init"]).expect("git init");
        run_git(source.path(), &["config", "user.email", "test@example.com"]).expect("email");
        run_git(source.path(), &["config", "user.name", "Test User"]).expect("name");
        run_git(source.path(), &["add", "src.txt"]).expect("add");
        run_git(source.path(), &["commit", "-m", "initial"]).expect("commit");

        let workspace_path = {
            let lease = create_sandbox(SandboxRequest {
                source_workspace: source.path().to_path_buf(),
                run_id: "run/git-cleanup".to_string(),
                attempt: 1,
                mode: SandboxMode::TmpCopy,
                keep_sandbox: false,
            })
            .expect("sandbox");
            let workspace_path = lease.workspace_path().to_path_buf();
            assert_eq!(lease.backend(), SandboxBackend::GitWorktree);
            workspace_path
        };

        let worktree_list = run_git(source.path(), &["worktree", "list"]).expect("worktree list");
        assert!(!worktree_list.contains(&workspace_path.display().to_string()));
    }

    #[test]
    fn sandbox_runtime_is_loaded_from_workspace_settings() {
        let workspace = tempfile::tempdir().expect("workspace");
        let quorp_dir = workspace.path().join(".quorp");
        fs::create_dir_all(&quorp_dir).expect("quorp dir");
        fs::write(
            quorp_dir.join("settings.json"),
            r#"{
              "sandbox": {
                "mode": "tmp-copy",
                "keep_last_sandbox": false,
                "runtime": {
                  "profile": "container",
                  "container": {
                    "engine": "docker",
                    "image": "example/container:latest"
                  }
                }
              }
            }"#,
        )
        .expect("write settings");

        let runtime = sandbox_runtime_for_path(workspace.path()).expect("runtime");
        assert_eq!(runtime.profile, SandboxRuntimeProfile::Container);
        assert_eq!(runtime.container.engine, ContainerEnginePreference::Docker);
        assert_eq!(runtime.container.image, "example/container:latest");
    }

    #[test]
    fn container_command_plan_adds_network_and_mount_flags() {
        let workspace = tempfile::tempdir().expect("workspace");
        let policy = SandboxPolicy::default();
        let runtime = SandboxRuntimeSettings {
            profile: SandboxRuntimeProfile::Container,
            container: quorp_core::ContainerRuntimeSettings {
                engine: ContainerEnginePreference::Docker,
                image: "example/container:latest".to_string(),
            },
        };
        let host_environment = capture_allowed_environment(&policy);
        let container_environment = container_environment(&policy, workspace.path());
        let spec = SandboxCommandSpec {
            program: OsStr::new("/bin/sh"),
            args: &[OsStr::new("-lc"), OsStr::new("echo hi")],
            current_dir: workspace.path(),
            runtime: &runtime,
            policy: &policy,
            extra_environment: &[],
            additional_mounts: &[],
            interactive: false,
        };
        let plan = build_container_command_plan(
            spec,
            OsString::from("docker"),
            workspace.path().to_path_buf(),
            container_environment,
            host_environment,
        );

        assert_eq!(plan.program, OsString::from("docker"));
        let rendered_args = plan
            .args
            .iter()
            .map(|value| value.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(rendered_args.contains(&"--network=none".to_string()));
        assert!(rendered_args.contains(&"--cap-drop=ALL".to_string()));
        assert!(rendered_args.contains(&"example/container:latest".to_string()));
        assert!(rendered_args.contains(&"/bin/sh".to_string()));
        assert!(rendered_args.contains(&"-lc".to_string()));
        assert!(rendered_args.contains(&"echo hi".to_string()));
        assert!(
            rendered_args
                .iter()
                .any(|value| value.starts_with("type=bind,source="))
        );
    }

    #[test]
    fn container_environment_with_extras_preserves_additional_values() {
        let current_dir = tempfile::tempdir().expect("current dir");
        let policy = SandboxPolicy::default();

        let environment = container_environment_with_extras(
            &policy,
            current_dir.path(),
            &[(String::from("QUORP_TEST_TOKEN"), String::from("visible"))],
        );

        assert!(
            environment
                .iter()
                .any(|(key, value)| key == "QUORP_TEST_TOKEN" && value == "visible")
        );
    }
}

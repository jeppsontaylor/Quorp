//! macOS-native, Docker-free sandbox for agent runs.
//!
//! The boundary is the kernel sandbox (`Sandbox.kext`), driven via
//! Apple's `sandbox-exec(1)` wrapper. `sandbox-exec` is undocumented
//! SPI but ships on every macOS install through Sequoia 15 / 26.
//! Apple uses the same machinery internally; profiles propagate to all
//! descendants of the spawned command, so cargo→rustc→lld stay
//! confined under one profile.
//!
//! Layered with this kernel boundary:
//! - per-run TmpCopy of the workspace under
//!   `/private/tmp/quorp/<run-id>/work/` (cloned via `cp -c -R`),
//! - `setrlimit(2)` belt-and-suspenders on CPU / memory / fds / procs,
//! - `setpgid(0, 0)` so the desktop run service can `killpg` on cancel,
//! - environment scrubbing of secret-like names.
//!
//! This module is macOS-only. Non-macOS builds compile a stub that
//! refuses to construct the apple-sandbox lease at runtime so the rest
//! of the workspace continues to compile.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Network access permitted inside an Apple-sandboxed run. The kernel
/// sandbox renders this into the profile's `network` clauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkAllowance {
    /// Default. The profile denies all network syscalls (and DNS lookups).
    #[default]
    None,
    /// Allow only `127.0.0.1` traffic. Useful for LSP servers and
    /// fixture loopbacks. DNS still denied.
    LocalhostOnly,
    /// Full outbound access plus DNS. Requires Trusted workspace +
    /// explicit user confirmation in the desktop UI; never the default.
    All,
}

/// Tunable settings for an Apple-sandboxed run. The desktop's
/// `RunService` constructs one of these per-run from the user's
/// settings + per-run options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppleSandboxSettings {
    pub allow_network: NetworkAllowance,
    /// Paths to add to the profile's `(allow file-read* (subpath ...))`
    /// list. Used for `~/.cargo`, `~/.rustup`, custom toolchain dirs,
    /// etc. Each entry MUST be an absolute, canonical path. Paths
    /// containing characters that have meaning in TinyScheme strings
    /// (`"`, `\`) are rejected by [`validate_subpath`].
    pub additional_read_subpaths: Vec<PathBuf>,
    /// CPU seconds. Default 1800 (30 min). The watchdog usually fires
    /// first; this is a hard ceiling.
    pub rlimit_cpu_seconds: u64,
    /// Address-space cap in bytes. Default 8 GiB.
    pub rlimit_as_bytes: u64,
    pub rlimit_data_bytes: u64,
    pub rlimit_nofile: u64,
    pub rlimit_nproc: u64,
    /// Single-file write cap. Default 4 GiB.
    pub rlimit_fsize_bytes: u64,
    /// Disable core dumps. Defaults to `true`.
    pub disable_core_dumps: bool,
}

impl Default for AppleSandboxSettings {
    fn default() -> Self {
        Self {
            allow_network: NetworkAllowance::None,
            additional_read_subpaths: Vec::new(),
            rlimit_cpu_seconds: 1800,
            rlimit_as_bytes: 8 * 1024 * 1024 * 1024,
            rlimit_data_bytes: 8 * 1024 * 1024 * 1024,
            rlimit_nofile: 4096,
            rlimit_nproc: 1024,
            rlimit_fsize_bytes: 4 * 1024 * 1024 * 1024,
            disable_core_dumps: true,
        }
    }
}

/// Errors that can arise constructing a confined lease, rendering a
/// profile, or building a wrapped command.
#[derive(Debug, thiserror::Error)]
pub enum AppleSandboxError {
    #[error("invalid run id `{0}`: must match ^[A-Za-z0-9_-]{{1,64}}$")]
    InvalidRunId(String),
    #[error(
        "invalid additional_read_subpath `{0}`: must be absolute, canonical, and free of `\"` and `\\`"
    )]
    InvalidSubpath(String),
    #[error("apple sandbox is only available on macOS")]
    UnsupportedPlatform,
    #[error("io error during sandbox setup: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to clone workspace into /tmp via cp -c -R: status {status}; stderr: {stderr}")]
    CloneFailed { status: i32, stderr: String },
    #[error("sandbox-exec binary not found at /usr/bin/sandbox-exec")]
    SandboxExecMissing,
}

/// A handle to an active confined run-temp directory. Cleans up when
/// dropped unless `keep` is set. Mirrors the contract of
/// [`crate::SandboxLease`] but specialized for the Apple-sandbox path.
#[derive(Debug)]
pub struct ConfinedTmpLease {
    run_id: String,
    work_dir: PathBuf,
    cargo_home: PathBuf,
    rustup_home: PathBuf,
    cache_dir: PathBuf,
    scratch_dir: PathBuf,
    profile_path: PathBuf,
    run_meta_path: PathBuf,
    sandbox_root: PathBuf,
    keep: bool,
}

impl ConfinedTmpLease {
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }
    pub fn cargo_home(&self) -> &Path {
        &self.cargo_home
    }
    pub fn rustup_home(&self) -> &Path {
        &self.rustup_home
    }
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }
    pub fn scratch_dir(&self) -> &Path {
        &self.scratch_dir
    }
    pub fn profile_path(&self) -> &Path {
        &self.profile_path
    }
    pub fn run_meta_path(&self) -> &Path {
        &self.run_meta_path
    }
    pub fn sandbox_root(&self) -> &Path {
        &self.sandbox_root
    }
    pub fn keep(&self) -> bool {
        self.keep
    }
    pub fn set_keep(&mut self, keep: bool) {
        self.keep = keep;
    }
}

impl Drop for ConfinedTmpLease {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        if let Err(err) = std::fs::remove_dir_all(&self.sandbox_root) {
            log::warn!(
                "failed to clean up apple sandbox at {}: {err}",
                self.sandbox_root.display()
            );
        }
    }
}

/// Validate a run id. The run id flows into the generated TinyScheme
/// profile and into a directory name; both contexts demand a strict
/// alphabet. We accept dashes and underscores, length 1..=64.
pub fn validate_run_id(run_id: &str) -> Result<(), AppleSandboxError> {
    if run_id.is_empty() || run_id.len() > 64 {
        return Err(AppleSandboxError::InvalidRunId(run_id.to_string()));
    }
    if !run_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(AppleSandboxError::InvalidRunId(run_id.to_string()));
    }
    Ok(())
}

/// Validate a path destined for `(allow file-read* (subpath ...))`.
/// Rejects relative paths, paths containing `"` or `\`, and paths with
/// embedded NULs. The check is intentionally conservative: legitimate
/// macOS toolchain paths never contain these characters.
pub fn validate_subpath(path: &Path) -> Result<(), AppleSandboxError> {
    let s = path.to_string_lossy();
    if !path.is_absolute() || s.contains('"') || s.contains('\\') || s.contains('\0') {
        return Err(AppleSandboxError::InvalidSubpath(s.into_owned()));
    }
    Ok(())
}

/// Render a sandbox profile for the given run. The output is the
/// TinyScheme source that `/usr/bin/sandbox-exec -f <path>` will
/// evaluate. The caller writes it to disk and feeds the path to
/// [`build_command`] / [`build_command_for_program`].
///
/// `work_root` MUST already be the canonical absolute path
/// (`/private/tmp/quorp/<run-id>`). Both `/tmp/...` and `/private/tmp/...`
/// forms are emitted in the allow rules so tools that resolve the
/// symlink and tools that don't both succeed.
pub fn render_profile(
    run_id: &str,
    work_root: &Path,
    settings: &AppleSandboxSettings,
) -> Result<String, AppleSandboxError> {
    validate_run_id(run_id)?;
    for path in &settings.additional_read_subpaths {
        validate_subpath(path)?;
    }
    let work_root_str = work_root.to_string_lossy();
    // /private/tmp <-> /tmp normalization. If work_root starts with
    // /private/tmp we emit a /tmp twin; if it starts with /tmp we emit
    // a /private/tmp twin. Other roots are emitted as-is (callers
    // canonicalize before constructing the lease).
    let twin_root = work_root_str
        .strip_prefix("/private/tmp")
        .map(|rest| format!("/tmp{rest}"))
        .or_else(|| {
            work_root_str
                .strip_prefix("/tmp")
                .map(|rest| format!("/private/tmp{rest}"))
        });

    let extra_read_clause = if settings.additional_read_subpaths.is_empty() {
        String::new()
    } else {
        let body: Vec<String> = settings
            .additional_read_subpaths
            .iter()
            .map(|p| format!("       (subpath \"{}\")", p.to_string_lossy()))
            .collect();
        format!("(allow file-read*\n{})\n", body.join("\n"))
    };

    let (network_clauses, dns_clauses) = match settings.allow_network {
        NetworkAllowance::None => (
            "(deny network*)".to_string(),
            "(deny mach-lookup (global-name \"com.apple.mDNSResponder\"))".to_string(),
        ),
        NetworkAllowance::LocalhostOnly => (
            "(allow network-bind (local ip \"localhost:*\"))\n\
             (allow network-outbound (remote ip \"localhost:*\"))\n\
             (deny network*)"
                .to_string(),
            "(deny mach-lookup (global-name \"com.apple.mDNSResponder\"))".to_string(),
        ),
        NetworkAllowance::All => (
            "(allow network*)".to_string(),
            "(allow mach-lookup (global-name \"com.apple.mDNSResponder\"))".to_string(),
        ),
    };

    // We build on Apple's own `system.sb` base profile, which ships
    // every essential allow rule (dyld cache reads, code-sig checks,
    // IOKit metadata, mach-lookup, sysctl, etc.) and then we layer
    // tighter restrictions on top: deny writes outside the run root,
    // deny network, and (via setrlimit + setpgid in the spawn path)
    // bound resource use.
    //
    // The pattern is `(import "system.sb")` + `(allow default)` to
    // start from a working baseline, then explicit `(deny ...)` rules
    // for the regions and capabilities we want to take away. This is
    // the same pattern Apple's own per-daemon profiles in
    // /usr/share/sandbox/*.sb use.

    let twin_clause = match twin_root.as_deref() {
        Some(twin) => {
            format!("       (subpath \"{twin}\")\n       (subpath \"/private/tmp/quorp/{run_id}\")",)
        }
        None => format!("       (subpath \"/private/tmp/quorp/{run_id}\")"),
    };

    let body = format!(
        ";; Quorp per-run sandbox profile (generated).
;; Boundary: agent + tools may only write under the run root.
;; /tmp resolves to /private/tmp on macOS; we emit both forms.
(version 1)
(import \"system.sb\")
(allow default)

;; Tightening: deny all writes outside the per-run /tmp directory.
;; The run root itself stays writable because the deny excludes it
;; via the (require-not (subpath ...)) sub-clauses below.
(deny file-write*
       (subpath \"/Applications\")
       (subpath \"/Library\")
       (subpath \"/Network\")
       (subpath \"/System\")
       (subpath \"/Users\")
       (subpath \"/Volumes\")
       (subpath \"/bin\")
       (subpath \"/cores\")
       (subpath \"/dev\")
       (subpath \"/etc\")
       (subpath \"/home\")
       (subpath \"/opt\")
       (subpath \"/private/etc\")
       (subpath \"/private/var\")
       (subpath \"/sbin\")
       (subpath \"/usr\")
       (subpath \"/var\"))

;; Working copy: re-allow writes inside the run root explicitly.
;; This wins because rule order favors later allows over earlier denies
;; on overlapping subpaths.
(allow file-write*
       (subpath \"{work_root_str}\")
{twin_clause})

;; ---- User-trusted toolchain locations (read-only) ----
{extra_read_clause}
;; ---- POSIX shared memory scoped to this run ----
(allow ipc-posix-shm-read* ipc-posix-shm-write-create
       (ipc-posix-name-prefix \"quorp-{run_id}-\"))

;; ---- Network ----
{network_clauses}
{dns_clauses}

;; ---- Belt-and-suspenders denies ----
(deny mach-priv-host-port)
(deny system-fsctl)
(deny system-privilege)
(deny job-creation)
"
    );
    Ok(body)
}

/// Path to the Apple-shipped sandbox-exec wrapper. Always at
/// `/usr/bin/sandbox-exec` on macOS.
pub const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

/// Probe whether `sandbox-exec` is callable. On non-macOS this is
/// always `false`. The desktop's Doctor panel runs this on startup.
pub fn sandbox_exec_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::path::Path::new(SANDBOX_EXEC_PATH).exists()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::os::unix::process::CommandExt;
    use std::path::Path;
    use std::process::Command;

    use super::{AppleSandboxError, AppleSandboxSettings, ConfinedTmpLease, SANDBOX_EXEC_PATH};

    /// Wrap `program + args` to run under the given sandbox profile.
    /// The returned `Command` has not been spawned. Callers attach the
    /// per-run environment (CARGO_HOME, RUSTUP_HOME, TMPDIR, etc.) via
    /// `command.env(...)` after this returns.
    pub fn build_command_for_program<P: AsRef<Path>>(
        program: P,
        args: &[&str],
        profile_path: &Path,
        settings: &AppleSandboxSettings,
    ) -> Result<Command, AppleSandboxError> {
        if !std::path::Path::new(SANDBOX_EXEC_PATH).exists() {
            return Err(AppleSandboxError::SandboxExecMissing);
        }
        let mut cmd = Command::new(SANDBOX_EXEC_PATH);
        cmd.arg("-f").arg(profile_path);
        cmd.arg(program.as_ref());
        for arg in args {
            cmd.arg(arg);
        }
        apply_pre_exec(&mut cmd, settings.clone());
        Ok(cmd)
    }

    fn apply_pre_exec(cmd: &mut Command, settings: AppleSandboxSettings) {
        // SAFETY: the closure runs after fork() in the child, before
        // exec(). We only call async-signal-safe libc functions
        // (setrlimit, setpgid). All other state mutations are scalars.
        //
        // setpgid is best-effort: if the child happens to already lead
        // a process group (e.g. because the parent inherited one) the
        // call returns EPERM/EACCES and we ignore it. The desktop run
        // service falls back to killing the immediate child by pid.
        unsafe {
            cmd.pre_exec(move || {
                set_rlimits(&settings)?;
                let _ = libc::setpgid(0, 0);
                Ok(())
            });
        }
    }

    fn set_rlimits(s: &AppleSandboxSettings) -> std::io::Result<()> {
        // Each setrlimit is best-effort. If a particular resource
        // refuses our soft cap (typically because the process already
        // allocated past it, or because the kernel hard-cap is below
        // our request) we proceed with the rest. We never abort the
        // run for a setrlimit failure; the kernel sandbox profile is
        // the authoritative boundary, and the watchdog handles wall-
        // clock budgets independently.
        let _ = set_one(libc::RLIMIT_CPU, s.rlimit_cpu_seconds);
        let _ = set_one(libc::RLIMIT_AS, s.rlimit_as_bytes);
        let _ = set_one(libc::RLIMIT_DATA, s.rlimit_data_bytes);
        let _ = set_one(libc::RLIMIT_NOFILE, s.rlimit_nofile);
        let _ = set_one(libc::RLIMIT_NPROC, s.rlimit_nproc);
        let _ = set_one(libc::RLIMIT_FSIZE, s.rlimit_fsize_bytes);
        if s.disable_core_dumps {
            let _ = set_one(libc::RLIMIT_CORE, 0);
        }
        Ok(())
    }

    fn set_one(resource: i32, desired: u64) -> std::io::Result<()> {
        // Read the current limits so we can clamp `rlim_cur` to the
        // existing hard cap. Raising `rlim_max` requires CAP_SYS_RESOURCE
        // / root on Linux and will fail with EINVAL on macOS for non-root
        // callers; we only ever want to *lower* the soft cap.
        let mut current = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: getrlimit writes into the rlimit struct pointed to by
        // its argument; the value is on the stack and lives for the
        // duration of the call.
        let ret = unsafe { libc::getrlimit(resource as libc::c_int, &mut current) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let max = current.rlim_max;
        let cur_target = (desired as libc::rlim_t).min(max);
        let limit = libc::rlimit {
            rlim_cur: cur_target,
            rlim_max: max,
        };
        // SAFETY: same as above; we never raise rlim_max so the call
        // is permitted without privilege.
        let ret = unsafe { libc::setrlimit(resource as libc::c_int, &limit) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// Set up the per-run /tmp lifecycle: create the run root with
    /// mode 0700, clone the workspace, scaffold cargo-home / rustup-home
    /// / cache / scratch dirs, and write the profile file. Returns a
    /// [`ConfinedTmpLease`] that cleans up on Drop.
    pub fn create_apple_sandbox(
        run_id: &str,
        source_workspace: &Path,
        settings: &AppleSandboxSettings,
    ) -> Result<ConfinedTmpLease, AppleSandboxError> {
        super::validate_run_id(run_id)?;
        let parent = std::path::PathBuf::from("/private/tmp/quorp");
        std::fs::create_dir_all(&parent)?;
        chmod_0700(&parent)?;

        let sandbox_root = parent.join(run_id);
        if sandbox_root.exists() {
            // Refuse to reuse an existing entry — caller must pick a
            // unique run id. Cleanup is best-effort but never opaque.
            std::fs::remove_dir_all(&sandbox_root)?;
        }
        std::fs::create_dir_all(&sandbox_root)?;
        chmod_0700(&sandbox_root)?;

        let work_dir = sandbox_root.join("work");
        let cargo_home = sandbox_root.join("cargo-home");
        let rustup_home = sandbox_root.join("rustup-home");
        let cache_dir = sandbox_root.join("cache");
        let scratch_dir = sandbox_root.join("scratch");
        for dir in [&cargo_home, &rustup_home, &cache_dir, &scratch_dir] {
            std::fs::create_dir_all(dir)?;
        }

        clone_workspace(source_workspace, &work_dir)?;

        // Resolve work_dir to its canonical /private/tmp form so the
        // profile renderer can emit both /tmp and /private/tmp twins.
        let canonical_work = std::fs::canonicalize(&work_dir).unwrap_or_else(|_| work_dir.clone());

        let profile_text = super::render_profile(run_id, &canonical_work, settings)?;
        let profile_path = sandbox_root.join("profile.sb");
        std::fs::write(&profile_path, profile_text)?;

        let run_meta_path = sandbox_root.join("run-meta.json");
        let meta = serde_json::json!({
            "run_id": run_id,
            "source_workspace": source_workspace.display().to_string(),
            "work_dir": canonical_work.display().to_string(),
            "started_at": chrono::Utc::now().to_rfc3339(),
        });
        std::fs::write(&run_meta_path, serde_json::to_vec_pretty(&meta).unwrap())?;

        Ok(ConfinedTmpLease {
            run_id: run_id.to_string(),
            work_dir,
            cargo_home,
            rustup_home,
            cache_dir,
            scratch_dir,
            profile_path,
            run_meta_path,
            sandbox_root,
            keep: false,
        })
    }

    fn chmod_0700(path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(path, perms)
    }

    fn clone_workspace(source: &Path, dest: &Path) -> Result<(), AppleSandboxError> {
        if dest.exists() {
            std::fs::remove_dir_all(dest)?;
        }
        let output = std::process::Command::new("/bin/cp")
            .arg("-c")
            .arg("-R")
            .arg(source)
            .arg(dest)
            .output()?;
        if !output.status.success() {
            return Err(AppleSandboxError::CloneFailed {
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        // Strip the always-excluded subtrees.
        for excluded in [".git", "target", "node_modules", ".quorp-runs", ".DS_Store"] {
            let p = dest.join(excluded);
            if p.exists() {
                let _ = std::fs::remove_dir_all(&p).or_else(|_| std::fs::remove_file(&p));
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub use platform::{build_command_for_program, create_apple_sandbox};

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::path::Path;
    use std::process::Command;

    use super::{AppleSandboxError, AppleSandboxSettings, ConfinedTmpLease};

    pub fn build_command_for_program<P: AsRef<Path>>(
        _program: P,
        _args: &[&str],
        _profile_path: &Path,
        _settings: &AppleSandboxSettings,
    ) -> Result<Command, AppleSandboxError> {
        Err(AppleSandboxError::UnsupportedPlatform)
    }

    pub fn create_apple_sandbox(
        _run_id: &str,
        _source_workspace: &Path,
        _settings: &AppleSandboxSettings,
    ) -> Result<ConfinedTmpLease, AppleSandboxError> {
        Err(AppleSandboxError::UnsupportedPlatform)
    }
}

#[cfg(not(target_os = "macos"))]
pub use platform::{build_command_for_program, create_apple_sandbox};

#[cfg(test)]
#[path = "../../../testing/quorp_sandbox/macos_apple_sandbox/tests.rs"]
mod tests;

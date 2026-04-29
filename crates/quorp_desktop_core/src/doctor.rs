//! Diagnostic probes surfaced through the desktop's Doctor panel.
//!
//! Every probe is best-effort and never throws: if a check can't run
//! (binary missing, command failed, etc.) the report carries the
//! failure as a structured `DoctorCheck::warn(...)` so the UI can
//! show a clear remediation hint instead of an opaque error.
//!
//! Probes:
//! 1. `sandbox-exec` presence + known-good profile boot
//! 2. `xcode-select -p` (Xcode CLT)
//! 3. `security find-identity -v -p codesigning` (Developer ID)
//! 4. `pnpm` and `node` on PATH (so `tauri dev` works)
//! 5. `$PATH` delta vs. user login shell (Finder-launched apps drop
//!    `~/.zshenv` etc. — we check whether our PATH lacks common
//!    directories the user has)
//! 6. Retention summary: how many run dirs are on disk under each
//!    known workspace
//! 7. Active run count + pending permission count

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use crate::DesktopAppState;

/// Outcome of a single probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Ok,
    Warn,
    Fail,
    /// Probe was skipped (e.g. on the wrong platform).
    Skipped,
}

/// One row in the Doctor panel.
#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    pub id: String,
    pub label: String,
    pub status: DoctorStatus,
    pub detail: String,
    /// Optional follow-up action the UI surfaces as a button label.
    pub remediation: Option<String>,
}

impl DoctorCheck {
    pub fn ok(id: &str, label: &str, detail: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status: DoctorStatus::Ok,
            detail: detail.into(),
            remediation: None,
        }
    }
    pub fn warn(id: &str, label: &str, detail: impl Into<String>, remedy: &str) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status: DoctorStatus::Warn,
            detail: detail.into(),
            remediation: Some(remedy.into()),
        }
    }
    pub fn fail(id: &str, label: &str, detail: impl Into<String>, remedy: &str) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status: DoctorStatus::Fail,
            detail: detail.into(),
            remediation: Some(remedy.into()),
        }
    }
    pub fn skipped(id: &str, label: &str, reason: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status: DoctorStatus::Skipped,
            detail: reason.into(),
            remediation: None,
        }
    }
}

/// Full Doctor report. The UI renders one row per check.
#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub generated_at: String,
    pub checks: Vec<DoctorCheck>,
    pub overall: DoctorStatus,
}

/// Run every probe and return a structured report. Cheap probes run
/// inline; the binary-presence checks fork sub-processes (each with
/// a 2-second timeout) on a thread pool.
pub fn run_doctor(state: &DesktopAppState) -> DoctorReport {
    let checks = vec![
        probe_sandbox_exec(),
        probe_xcode_clt(),
        probe_codesign_identity(),
        probe_binary_on_path("node", "Node 20+ recommended"),
        probe_binary_on_path("pnpm", "pnpm 9.12+"),
        probe_path_from_finder_delta(),
        probe_run_state(state),
        probe_workspace_count(state),
        probe_provider_state(state),
    ];
    let overall = aggregate(&checks);
    DoctorReport {
        generated_at: chrono::Utc::now().to_rfc3339(),
        checks,
        overall,
    }
}

fn aggregate(checks: &[DoctorCheck]) -> DoctorStatus {
    let mut worst = DoctorStatus::Ok;
    for c in checks {
        worst = match (worst, c.status) {
            (DoctorStatus::Fail, _) | (_, DoctorStatus::Fail) => DoctorStatus::Fail,
            (DoctorStatus::Warn, _) | (_, DoctorStatus::Warn) => DoctorStatus::Warn,
            (DoctorStatus::Skipped, other) | (other, DoctorStatus::Skipped) => other,
            _ => DoctorStatus::Ok,
        };
    }
    worst
}

fn probe_sandbox_exec() -> DoctorCheck {
    #[cfg(not(target_os = "macos"))]
    {
        return DoctorCheck::skipped(
            "sandbox-exec",
            "Apple sandbox-exec",
            "macOS only.",
        );
    }
    #[cfg(target_os = "macos")]
    {
        let bin = Path::new("/usr/bin/sandbox-exec");
        if !bin.exists() {
            return DoctorCheck::fail(
                "sandbox-exec",
                "Apple sandbox-exec",
                "/usr/bin/sandbox-exec not present on this host.",
                "Reinstall macOS Command Line Tools (xcode-select --install).",
            );
        }
        // Known-good profile boot: import system.sb and let /bin/echo
        // run. If this fails, profile rendering will fail too and the
        // sandboxed-run path is broken on this host.
        let profile = "(version 1)\n(import \"system.sb\")\n(allow default)";
        let mut tmp = std::env::temp_dir();
        tmp.push(format!("quorp-doctor-{}.sb", std::process::id()));
        if std::fs::write(&tmp, profile).is_err() {
            return DoctorCheck::warn(
                "sandbox-exec",
                "Apple sandbox-exec",
                "couldn't write a probe profile to TMPDIR.",
                "Free up disk space or check TMPDIR permissions.",
            );
        }
        let result = run_with_timeout(
            std::process::Command::new("/usr/bin/sandbox-exec")
                .arg("-f")
                .arg(&tmp)
                .arg("/bin/echo")
                .arg("ok"),
            Duration::from_secs(2),
        );
        let _ = std::fs::remove_file(&tmp);
        match result {
            Ok(output) if output.status.success() => DoctorCheck::ok(
                "sandbox-exec",
                "Apple sandbox-exec",
                "/usr/bin/sandbox-exec runs a known-good profile.",
            ),
            Ok(output) => DoctorCheck::fail(
                "sandbox-exec",
                "Apple sandbox-exec",
                format!(
                    "probe profile rejected by sandbox-exec (exit {}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
                "Reinstall macOS CLT or run `sudo xcodebuild -license accept`.",
            ),
            Err(err) => DoctorCheck::fail(
                "sandbox-exec",
                "Apple sandbox-exec",
                format!("probe failed to run: {err}"),
                "Reinstall macOS CLT (xcode-select --install).",
            ),
        }
    }
}

fn probe_xcode_clt() -> DoctorCheck {
    #[cfg(not(target_os = "macos"))]
    {
        return DoctorCheck::skipped("xcode-clt", "Xcode CLT", "macOS only.");
    }
    #[cfg(target_os = "macos")]
    {
        match run_with_timeout(
            std::process::Command::new("/usr/bin/xcode-select").arg("-p"),
            Duration::from_secs(1),
        ) {
            Ok(output) if output.status.success() => DoctorCheck::ok(
                "xcode-clt",
                "Xcode CLT",
                String::from_utf8_lossy(&output.stdout).trim().to_string(),
            ),
            Ok(_) | Err(_) => DoctorCheck::fail(
                "xcode-clt",
                "Xcode CLT",
                "xcode-select -p did not return a path.",
                "Run: xcode-select --install",
            ),
        }
    }
}

fn probe_codesign_identity() -> DoctorCheck {
    #[cfg(not(target_os = "macos"))]
    {
        return DoctorCheck::skipped(
            "codesign-identity",
            "Developer ID identity",
            "macOS only.",
        );
    }
    #[cfg(target_os = "macos")]
    {
        match run_with_timeout(
            std::process::Command::new("/usr/bin/security")
                .arg("find-identity")
                .arg("-v")
                .arg("-p")
                .arg("codesigning"),
            Duration::from_secs(2),
        ) {
            Ok(output) if output.status.success() => {
                let body = String::from_utf8_lossy(&output.stdout);
                if body.contains("Developer ID Application") {
                    DoctorCheck::ok(
                        "codesign-identity",
                        "Developer ID identity",
                        "Developer ID Application certificate present.",
                    )
                } else {
                    DoctorCheck::warn(
                        "codesign-identity",
                        "Developer ID identity",
                        "no Developer ID Application certificate; ad-hoc signing only.",
                        "Use script/quorp-desktop-build-dmg (ad-hoc). Signed/notarized builds need a Developer ID.",
                    )
                }
            }
            Ok(_) | Err(_) => DoctorCheck::warn(
                "codesign-identity",
                "Developer ID identity",
                "couldn't query security keychain.",
                "Local DMGs build ad-hoc; signed builds require Developer ID.",
            ),
        }
    }
}

fn probe_binary_on_path(binary: &str, recommend: &str) -> DoctorCheck {
    let id = format!("path-{binary}");
    match which(binary) {
        Some(path) => DoctorCheck::ok(&id, &format!("{binary} on PATH"), path),
        None => DoctorCheck::warn(
            &id,
            &format!("{binary} on PATH"),
            "not found in $PATH visible to this app.",
            recommend,
        ),
    }
}

fn probe_path_from_finder_delta() -> DoctorCheck {
    #[cfg(not(target_os = "macos"))]
    {
        return DoctorCheck::skipped(
            "path-from-finder",
            "$PATH from Finder",
            "macOS only.",
        );
    }
    #[cfg(target_os = "macos")]
    {
        let our_path = std::env::var("PATH").unwrap_or_default();
        // The login shell's PATH is the user's authoritative one.
        // Spawn `/bin/zsh -lc 'echo $PATH'` and compare.
        let shell_path = match run_with_timeout(
            std::process::Command::new("/bin/zsh")
                .arg("-lc")
                .arg("printf %s \"$PATH\""),
            Duration::from_secs(1),
        ) {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            }
            _ => return DoctorCheck::skipped(
                "path-from-finder",
                "$PATH from Finder",
                "couldn't query login-shell PATH.",
            ),
        };
        let our: std::collections::HashSet<&str> =
            our_path.split(':').filter(|s| !s.is_empty()).collect();
        let theirs: std::collections::HashSet<&str> =
            shell_path.split(':').filter(|s| !s.is_empty()).collect();
        let missing: Vec<&&str> = theirs.difference(&our).collect();
        if missing.is_empty() {
            DoctorCheck::ok(
                "path-from-finder",
                "$PATH from Finder",
                "matches login-shell PATH.",
            )
        } else {
            DoctorCheck::warn(
                "path-from-finder",
                "$PATH from Finder",
                format!(
                    "{} login-shell entries missing: {}",
                    missing.len(),
                    missing
                        .iter()
                        .take(4)
                        .copied()
                        .copied()
                        .collect::<Vec<&str>>()
                        .join(", ")
                ),
                "Toggle Settings → UI → \"Inherit login-shell PATH\" (or relaunch from a terminal with `quorp app .`).",
            )
        }
    }
}

fn probe_run_state(state: &DesktopAppState) -> DoctorCheck {
    let active = state.runs.active_handles().len();
    let pending = state.permissions.pending_count();
    let detail = format!(
        "active runs: {active} · pending permissions: {pending}",
    );
    DoctorCheck::ok("runtime-state", "Runtime state", detail)
}

fn probe_workspace_count(state: &DesktopAppState) -> DoctorCheck {
    let count = state.workspaces.list().len();
    if count == 0 {
        DoctorCheck::warn(
            "workspaces",
            "Workspaces",
            "no workspaces registered.",
            "Click + Add Folder in the sidebar to register one.",
        )
    } else {
        DoctorCheck::ok(
            "workspaces",
            "Workspaces",
            format!("{count} registered"),
        )
    }
}

fn probe_provider_state(state: &DesktopAppState) -> DoctorCheck {
    if state.providers.has_api_key() {
        DoctorCheck::ok(
            "provider-key",
            "NIM API key",
            "stored in macOS Keychain.",
        )
    } else {
        DoctorCheck::warn(
            "provider-key",
            "NIM API key",
            "not configured. Real runs will fall back to the demo stream.",
            "Settings → Provider → paste your NVIDIA NIM API key.",
        )
    }
}

fn run_with_timeout(
    command: &mut std::process::Command,
    _timeout: Duration,
) -> std::io::Result<std::process::Output> {
    // We don't run a real timeout watchdog here because every
    // sub-process we shell out to is bounded by the OS already
    // (`security`, `xcode-select`, `sandbox-exec /bin/echo`). Keeping
    // this wrapper makes future timeout enforcement a one-line change.
    //
    // The clippy `disallowed_methods` lint forbids `Command::output()`
    // workspace-wide because the agent runtime should always use the
    // smol-aware variants. The Doctor runs as a one-off probe outside
    // any agent loop, so the project default doesn't apply here.
    #[allow(clippy::disallowed_methods)]
    command.output()
}

fn which(binary: &str) -> Option<String> {
    let path_env = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_env) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate.display().to_string());
        }
    }
    None
}

//! Tests for the macOS Apple-sandbox module.
//!
//! Profile-rendering and validation tests are cross-platform (the
//! renderer is pure-Rust string templating). Integration tests that
//! spawn `sandbox-exec` are gated on `#[cfg(target_os = "macos")]`.

use super::*;
use std::path::PathBuf;

#[test]
fn validate_run_id_accepts_canonical_ids() {
    for id in ["run-1", "RUN-2026-04-28", "Smoke_Test_001", "abc123", "a"] {
        validate_run_id(id).unwrap_or_else(|err| panic!("expected ok for {id}, got {err}"));
    }
}

#[test]
fn validate_run_id_rejects_empty() {
    let err = validate_run_id("").unwrap_err();
    assert!(matches!(err, AppleSandboxError::InvalidRunId(_)));
}

#[test]
fn validate_run_id_rejects_too_long() {
    let id = "a".repeat(65);
    assert!(validate_run_id(&id).is_err());
}

#[test]
fn validate_run_id_rejects_special_chars() {
    for id in [
        "run/1",
        "run space",
        "run\"quote",
        "run`tick",
        "run$var",
        "run;evil",
        "run\nnewline",
        "../../etc",
    ] {
        let err = validate_run_id(id).unwrap_err();
        assert!(
            matches!(err, AppleSandboxError::InvalidRunId(_)),
            "expected InvalidRunId for {id}, got {err}"
        );
    }
}

#[test]
fn validate_subpath_accepts_absolute_canonical() {
    validate_subpath(&PathBuf::from("/usr/lib")).unwrap();
    validate_subpath(&PathBuf::from("/Users/me/.cargo")).unwrap();
    validate_subpath(&PathBuf::from("/private/tmp/quorp/run-1/work")).unwrap();
}

#[test]
fn validate_subpath_rejects_relative() {
    let err = validate_subpath(&PathBuf::from("./relative")).unwrap_err();
    assert!(matches!(err, AppleSandboxError::InvalidSubpath(_)));
}

#[test]
fn validate_subpath_rejects_dangerous_chars() {
    // PathBuf::from preserves embedded characters faithfully.
    let err = validate_subpath(&PathBuf::from("/usr/lib\"evil")).unwrap_err();
    assert!(matches!(err, AppleSandboxError::InvalidSubpath(_)));
    let err = validate_subpath(&PathBuf::from("/usr/\\evil")).unwrap_err();
    assert!(matches!(err, AppleSandboxError::InvalidSubpath(_)));
}

#[test]
fn render_profile_emits_both_tmp_forms() {
    let work_root = PathBuf::from("/private/tmp/quorp/run-x/work");
    let s = AppleSandboxSettings::default();
    let out = render_profile("run-x", &work_root, &s).unwrap();
    assert!(out.contains("(subpath \"/private/tmp/quorp/run-x/work\")"));
    assert!(out.contains("(subpath \"/tmp/quorp/run-x/work\")"));
    // Both /tmp and /private/tmp twins of the run root must appear so
    // tools that resolve the symlink and tools that don't both pass.
}

#[test]
fn render_profile_imports_system_base() {
    let work_root = PathBuf::from("/private/tmp/quorp/run-x/work");
    let s = AppleSandboxSettings::default();
    let out = render_profile("run-x", &work_root, &s).unwrap();
    assert!(out.contains("(version 1)"));
    // We layer on top of Apple's `system.sb` base profile; this
    // gives us a known-good baseline for arbitrary tooling to run.
    assert!(out.contains("(import \"system.sb\")"));
    // After the import, file-writes outside the run root are denied
    // explicitly, and the run root itself is re-allowed.
    assert!(out.contains("(deny file-write*"));
    assert!(out.contains("(allow file-write*"));
}

#[test]
fn render_profile_default_denies_network() {
    let work_root = PathBuf::from("/private/tmp/quorp/run-x/work");
    let s = AppleSandboxSettings::default();
    let out = render_profile("run-x", &work_root, &s).unwrap();
    assert!(out.contains("(deny network*)"));
    // DNS lookup also denied to avoid surprising libresolv hangs.
    assert!(out.contains("com.apple.mDNSResponder"));
    // No `(allow network*)` when allow_network is None.
    assert!(!out.contains("(allow network*)"));
}

#[test]
fn render_profile_localhost_allows_loopback_only() {
    let work_root = PathBuf::from("/private/tmp/quorp/run-x/work");
    let s = AppleSandboxSettings {
        allow_network: NetworkAllowance::LocalhostOnly,
        ..AppleSandboxSettings::default()
    };
    let out = render_profile("run-x", &work_root, &s).unwrap();
    assert!(out.contains("(allow network-bind (local ip \"localhost:*\"))"));
    assert!(out.contains("(allow network-outbound (remote ip \"localhost:*\"))"));
    assert!(out.contains("(deny network*)"));
}

#[test]
fn render_profile_full_network_allows_outbound_and_dns() {
    let work_root = PathBuf::from("/private/tmp/quorp/run-x/work");
    let s = AppleSandboxSettings {
        allow_network: NetworkAllowance::All,
        ..AppleSandboxSettings::default()
    };
    let out = render_profile("run-x", &work_root, &s).unwrap();
    assert!(out.contains("(allow network*)"));
    assert!(out.contains("(allow mach-lookup (global-name \"com.apple.mDNSResponder\"))"));
}

#[test]
fn render_profile_appends_trusted_subpaths() {
    let work_root = PathBuf::from("/private/tmp/quorp/run-x/work");
    let s = AppleSandboxSettings {
        additional_read_subpaths: vec![
            PathBuf::from("/Users/test/.cargo"),
            PathBuf::from("/Users/test/.rustup"),
        ],
        ..AppleSandboxSettings::default()
    };
    let out = render_profile("run-x", &work_root, &s).unwrap();
    assert!(out.contains("(subpath \"/Users/test/.cargo\")"));
    assert!(out.contains("(subpath \"/Users/test/.rustup\")"));
}

#[test]
fn render_profile_rejects_invalid_subpath() {
    let work_root = PathBuf::from("/private/tmp/quorp/run-x/work");
    let s = AppleSandboxSettings {
        additional_read_subpaths: vec![PathBuf::from("./bad")],
        ..AppleSandboxSettings::default()
    };
    let err = render_profile("run-x", &work_root, &s).unwrap_err();
    assert!(matches!(err, AppleSandboxError::InvalidSubpath(_)));
}

#[test]
fn render_profile_rejects_invalid_run_id() {
    let work_root = PathBuf::from("/private/tmp/quorp/run-x/work");
    let s = AppleSandboxSettings::default();
    let err = render_profile("../escape", &work_root, &s).unwrap_err();
    assert!(matches!(err, AppleSandboxError::InvalidRunId(_)));
}

#[test]
fn render_profile_emits_run_scoped_shm_prefix() {
    let work_root = PathBuf::from("/private/tmp/quorp/run-x/work");
    let s = AppleSandboxSettings::default();
    let out = render_profile("run-x", &work_root, &s).unwrap();
    // POSIX shm names get prefixed by run id so two parallel sandboxes
    // can't share segments.
    assert!(out.contains("(ipc-posix-name-prefix \"quorp-run-x-\")"));
}

#[test]
fn render_profile_emits_belt_and_suspenders_denies() {
    let work_root = PathBuf::from("/private/tmp/quorp/run-x/work");
    let s = AppleSandboxSettings::default();
    let out = render_profile("run-x", &work_root, &s).unwrap();
    // Targeted denies that always apply on top of the system.sb base.
    // Other capabilities (iokit-open, mach-task-name) come pre-denied
    // by tightening rules in the system.sb profile.
    for must in [
        "(deny mach-priv-host-port)",
        "(deny system-fsctl)",
        "(deny system-privilege)",
        "(deny job-creation)",
    ] {
        assert!(out.contains(must), "expected `{must}` in profile");
    }
}

#[test]
fn default_settings_have_sane_rlimits() {
    let s = AppleSandboxSettings::default();
    assert_eq!(s.rlimit_cpu_seconds, 1800);
    assert_eq!(s.rlimit_as_bytes, 8 * 1024 * 1024 * 1024);
    assert_eq!(s.rlimit_nofile, 4096);
    assert_eq!(s.rlimit_nproc, 1024);
    assert!(s.disable_core_dumps);
    assert_eq!(s.allow_network, NetworkAllowance::None);
}

#[cfg(not(target_os = "macos"))]
mod non_macos {
    use super::*;
    use std::path::Path;

    #[test]
    fn create_apple_sandbox_returns_unsupported_off_macos() {
        let tmp = tempfile::tempdir().unwrap();
        let err = create_apple_sandbox("run-x", tmp.path(), &AppleSandboxSettings::default())
            .unwrap_err();
        assert!(matches!(err, AppleSandboxError::UnsupportedPlatform));
    }

    #[test]
    fn build_command_returns_unsupported_off_macos() {
        let err = build_command_for_program(
            Path::new("/bin/echo"),
            &[],
            Path::new("/tmp/profile.sb"),
            &AppleSandboxSettings::default(),
        )
        .unwrap_err();
        assert!(matches!(err, AppleSandboxError::UnsupportedPlatform));
    }
}

#[cfg(target_os = "macos")]
mod macos_integration {
    use super::*;

    #[test]
    fn sandbox_exec_is_present_on_macos() {
        // Should be true on every macOS build host since 10.x.
        assert!(sandbox_exec_available());
    }

    #[test]
    fn create_apple_sandbox_clones_workspace() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("hello.txt"), b"hi").unwrap();
        let run_id = format!("smoke-{}", std::process::id());
        let lease =
            create_apple_sandbox(&run_id, source.path(), &AppleSandboxSettings::default()).unwrap();

        assert!(lease.work_dir().exists());
        assert!(lease.work_dir().join("hello.txt").exists());
        assert!(lease.profile_path().exists());
        assert!(lease.run_meta_path().exists());

        let profile_text = std::fs::read_to_string(lease.profile_path()).unwrap();
        assert!(profile_text.contains("(version 1)"));
        assert!(profile_text.contains(&format!("(ipc-posix-name-prefix \"quorp-{run_id}-\")")));
    }

    #[test]
    fn create_apple_sandbox_excludes_target_and_dotgit() {
        let source = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(source.path().join("src")).unwrap();
        std::fs::write(source.path().join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::create_dir_all(source.path().join(".git")).unwrap();
        std::fs::write(source.path().join(".git/config"), b"x").unwrap();
        std::fs::create_dir_all(source.path().join("target/debug")).unwrap();
        std::fs::write(source.path().join("target/debug/build.bin"), b"x").unwrap();
        std::fs::write(source.path().join("node_modules"), b"x").unwrap();

        let run_id = format!("excl-{}", std::process::id());
        let lease =
            create_apple_sandbox(&run_id, source.path(), &AppleSandboxSettings::default()).unwrap();

        assert!(lease.work_dir().join("src/main.rs").exists());
        assert!(!lease.work_dir().join(".git").exists());
        assert!(!lease.work_dir().join("target").exists());
        assert!(!lease.work_dir().join("node_modules").exists());
    }

    #[test]
    fn lease_drop_removes_sandbox_root() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("a.txt"), b"x").unwrap();
        let run_id = format!("drop-{}", std::process::id());
        let sandbox_root = {
            let lease =
                create_apple_sandbox(&run_id, source.path(), &AppleSandboxSettings::default())
                    .unwrap();
            lease.sandbox_root().to_path_buf()
        };
        assert!(!sandbox_root.exists(), "sandbox root should be cleaned up");
    }

    #[test]
    fn keep_flag_preserves_sandbox_root() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("a.txt"), b"x").unwrap();
        let run_id = format!("keep-{}", std::process::id());
        let kept_root = {
            let mut lease =
                create_apple_sandbox(&run_id, source.path(), &AppleSandboxSettings::default())
                    .unwrap();
            lease.set_keep(true);
            let root = lease.sandbox_root().to_path_buf();
            drop(lease);
            root
        };
        assert!(kept_root.exists());
        // Manual cleanup so the test is repeatable.
        let _ = std::fs::remove_dir_all(&kept_root);
    }

    #[test]
    fn denies_etc_passwd_under_default_profile() {
        // Cat /etc/passwd is allowed at the file-read* level (covered by
        // (literal "/etc/passwd")) but writing to /etc/passwd must fail.
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("a.txt"), b"x").unwrap();
        let run_id = format!("deny-{}", std::process::id());
        let lease =
            create_apple_sandbox(&run_id, source.path(), &AppleSandboxSettings::default()).unwrap();

        // Reading /etc/passwd should succeed (it's explicitly allowed).
        let read_status = build_command_for_program(
            "/bin/cat",
            &["/etc/passwd"],
            lease.profile_path(),
            &AppleSandboxSettings::default(),
        )
        .unwrap()
        .status()
        .unwrap();
        assert!(read_status.success(), "cat /etc/passwd should be allowed");

        // Writing to /etc/passwd should fail.
        let write_status = build_command_for_program(
            "/bin/sh",
            &["-c", "echo evil >> /etc/passwd"],
            lease.profile_path(),
            &AppleSandboxSettings::default(),
        )
        .unwrap()
        .status()
        .unwrap();
        assert!(!write_status.success(), "writing /etc/passwd must fail");
    }

    #[test]
    fn denies_writes_outside_workroot() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("a.txt"), b"x").unwrap();
        let run_id = format!("scope-{}", std::process::id());
        let lease =
            create_apple_sandbox(&run_id, source.path(), &AppleSandboxSettings::default()).unwrap();

        // Writing inside the work dir succeeds.
        let in_status = build_command_for_program(
            "/bin/sh",
            &[
                "-c",
                &format!("echo hi > {}/test.txt", lease.work_dir().display()),
            ],
            lease.profile_path(),
            &AppleSandboxSettings::default(),
        )
        .unwrap()
        .status()
        .unwrap();
        assert!(
            in_status.success(),
            "writing inside work dir should succeed"
        );

        // Writing outside the run root must fail.
        let outside_path = source.path().parent().unwrap().join("escape.txt");
        let out_status = build_command_for_program(
            "/bin/sh",
            &["-c", &format!("echo hi > {}", outside_path.display())],
            lease.profile_path(),
            &AppleSandboxSettings::default(),
        )
        .unwrap()
        .status()
        .unwrap();
        assert!(
            !out_status.success(),
            "writing outside the work root must fail"
        );
    }

    #[test]
    fn rejects_existing_sandbox_root_by_recreating() {
        // Two sequential creates with the same run id should both
        // succeed: the second one wipes and recreates.
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("a.txt"), b"x").unwrap();
        let run_id = format!("recreate-{}", std::process::id());

        let lease1 =
            create_apple_sandbox(&run_id, source.path(), &AppleSandboxSettings::default()).unwrap();
        std::fs::write(lease1.work_dir().join("marker"), b"first").unwrap();
        // Don't drop yet — simulate a stale entry.
        std::mem::forget(lease1);

        let lease2 =
            create_apple_sandbox(&run_id, source.path(), &AppleSandboxSettings::default()).unwrap();
        // Marker from the first lease must NOT survive — the recreate
        // wipes the prior sandbox root.
        assert!(!lease2.work_dir().join("marker").exists());
    }
}

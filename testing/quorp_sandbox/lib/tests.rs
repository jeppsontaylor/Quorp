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
    command
        .arg("-lc")
        .arg("printf '%s|%s|%s' \"${QUORP_TEST_SECRET:-}\" \"${QUORP_TEST_SAFE:-}\" \"${PATH:-}\"");
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

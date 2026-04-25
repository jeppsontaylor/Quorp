use anyhow::Context as _;
use clap::{Args as ClapArgs, ValueEnum};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const CONTAINER_WORKSPACE_ROOT: &str = "/workspace";
pub const CONTAINER_RESULTS_ROOT: &str = "/quorp-results";
pub const CONTAINER_STATE_ROOT: &str = "/quorp-state";
const CONTAINER_DATA_ROOT: &str = "/quorp-state/data";
const CONTAINER_HOME_ROOT: &str = "/quorp-state/home";
const CONTAINER_CODEX_HOME_ROOT: &str = "/quorp-state/codex";
const DEFAULT_DOCKER_IMAGE: &str = "quorp-runner:dev";
const ENV_RUNTIME_MODE: &str = "QUORP_RUNTIME_MODE";
const ENV_IN_DOCKER: &str = "QUORP_IN_DOCKER";
const ENV_DOCKER_HOST_WORKSPACE_ROOT: &str = "QUORP_DOCKER_HOST_WORKSPACE_ROOT";
const ENV_DOCKER_CONTAINER_WORKSPACE_ROOT: &str = "QUORP_DOCKER_CONTAINER_WORKSPACE_ROOT";
const ENV_DOCKER_HOST_RESULT_DIR: &str = "QUORP_DOCKER_HOST_RESULT_DIR";
const ENV_DOCKER_IMAGE_USED: &str = "QUORP_DOCKER_IMAGE_USED";
const ENV_DOCKER_NETWORK_MODE: &str = "QUORP_DOCKER_NETWORK_MODE";
const ENV_DOCKER_ENDPOINT_REWRITTEN: &str = "QUORP_DOCKER_ENDPOINT_REWRITTEN";
const ENV_DOCKER_CONTAINER_NAME: &str = "QUORP_DOCKER_CONTAINER_NAME";

#[derive(Debug, Clone, ClapArgs, Default)]
pub struct DockerArgs {
    #[arg(long, default_value_t = false)]
    pub docker: bool,
    #[arg(long)]
    pub docker_image: Option<String>,
    #[arg(long)]
    pub docker_state_dir: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    pub docker_keep_container: bool,
}

#[derive(Debug, Clone)]
struct MountSpec {
    host: PathBuf,
    container: &'static str,
}

#[derive(Debug, Clone)]
struct DockerLaunchSpec {
    image: String,
    container_args: Vec<String>,
    mounts: Vec<MountSpec>,
    state_dir: PathBuf,
    pass_env: BTreeMap<String, String>,
    interactive: bool,
    keep_container: bool,
    container_name: Option<String>,
    host_workspace_root: Option<PathBuf>,
    host_result_dir: Option<PathBuf>,
    endpoint_rewritten: bool,
}

impl DockerArgs {
    pub fn enabled(&self) -> bool {
        self.docker
            || std::env::var("QUORP_SANDBOX")
                .ok()
                .is_some_and(|value| value.eq_ignore_ascii_case("docker"))
    }

    fn resolved_image(&self) -> String {
        self.docker_image
            .clone()
            .or_else(|| std::env::var("QUORP_DOCKER_IMAGE").ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_DOCKER_IMAGE.to_string())
    }
}

pub fn bootstrap_custom_data_dir_from_env() {
    if let Ok(dir) = std::env::var("QUORP_DATA_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            let _ = ::paths::set_custom_data_dir(trimmed);
        }
    }
}

pub fn runtime_metadata_json() -> serde_json::Value {
    let mode = std::env::var(ENV_RUNTIME_MODE).unwrap_or_else(|_| "host".to_string());
    let mut value = json!({
        "mode": mode,
    });
    if mode == "docker"
        && let Some(object) = value.as_object_mut()
    {
        object.insert(
            "docker_image".to_string(),
            std::env::var(ENV_DOCKER_IMAGE_USED)
                .ok()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
        );
        object.insert(
            "container_workspace".to_string(),
            std::env::var(ENV_DOCKER_CONTAINER_WORKSPACE_ROOT)
                .ok()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
        );
        object.insert(
            "network_mode".to_string(),
            std::env::var(ENV_DOCKER_NETWORK_MODE)
                .ok()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
        );
        object.insert(
            "endpoint_rewritten".to_string(),
            std::env::var(ENV_DOCKER_ENDPOINT_REWRITTEN)
                .ok()
                .and_then(|value| value.parse::<bool>().ok())
                .map(serde_json::Value::Bool)
                .unwrap_or(serde_json::Value::Null),
        );
        object.insert(
            "container_name".to_string(),
            std::env::var(ENV_DOCKER_CONTAINER_NAME)
                .ok()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
        );
    }
    value
}

pub fn runtime_metadata_summary_from_value(metadata: &serde_json::Value) -> Option<String> {
    let object = metadata.as_object()?;
    let mode = object.get("mode")?.as_str()?;
    if mode != "docker" {
        return Some("host".to_string());
    }
    let image = object
        .get("docker_image")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown-image");
    let workspace = object
        .get("container_workspace")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(CONTAINER_WORKSPACE_ROOT);
    Some(format!("docker image={image} workspace={workspace}"))
}

pub fn maybe_reexec_in_docker(args: &crate::CliArgs) -> anyhow::Result<Option<i32>> {
    if std::env::var(ENV_IN_DOCKER)
        .ok()
        .is_some_and(|value| value == "1")
    {
        return Ok(None);
    }
    let Some(spec) = build_launch_spec(args)? else {
        return Ok(None);
    };
    let exit_code = launch(spec)?;
    Ok(Some(exit_code))
}

fn build_launch_spec(args: &crate::CliArgs) -> anyhow::Result<Option<DockerLaunchSpec>> {
    match args.command.as_ref() {
        Some(crate::Command::Session(session_args)) if session_args.docker.enabled() => {
            build_session_spec(session_args).map(Some)
        }
        Some(crate::Command::Run(run_args))
            if run_args.command.is_none() && run_args.start.docker.enabled() =>
        {
            build_run_spec(run_args).map(Some)
        }
        Some(crate::Command::Agent {
            command: crate::AgentCommand::Run(agent_args),
        }) if agent_args.docker.enabled() => build_agent_run_spec(agent_args).map(Some),
        Some(crate::Command::Benchmark {
            command: crate::BenchmarkCommand::Run(benchmark_args),
        }) if benchmark_args.docker.enabled() => build_benchmark_run_spec(benchmark_args).map(Some),
        Some(crate::Command::Benchmark {
            command: crate::BenchmarkCommand::Batch(batch_args),
        }) if batch_args.docker.enabled() => build_benchmark_batch_spec(batch_args).map(Some),
        Some(crate::Command::Benchmark {
            command: crate::BenchmarkCommand::Resume(resume_args),
        }) if resume_args.docker.enabled() => build_benchmark_resume_spec(resume_args).map(Some),
        _ => Ok(None),
    }
}

fn build_session_spec(args: &crate::SessionArgs) -> anyhow::Result<DockerLaunchSpec> {
    let workspace = crate::SessionLaunchConfig::from_workspace(
        args.workspace
            .clone()
            .unwrap_or_else(crate::default_workspace_root),
        args.provider,
        args.model.clone(),
        args.prompt_compaction_policy
            .as_deref()
            .and_then(quorp_agent_core::PromptCompactionPolicy::parse),
    )
    .workspace_root;
    let state_dir = resolve_session_state_dir(&args.docker, &workspace)?;
    let mut container_args = vec![
        "session".to_string(),
        "--workspace".to_string(),
        CONTAINER_WORKSPACE_ROOT.to_string(),
    ];
    if let Some(provider) = args.provider {
        container_args.push("--provider".to_string());
        container_args.push(
            provider
                .to_possible_value()
                .expect("provider")
                .get_name()
                .to_string(),
        );
    }
    if let Some(model) = args.model.as_ref() {
        container_args.push("--model".to_string());
        container_args.push(model.clone());
    }
    if let Some(policy) = args.prompt_compaction_policy.as_ref() {
        container_args.push("--prompt-compaction-policy".to_string());
        container_args.push(policy.clone());
    }
    let image = args.docker.resolved_image();
    let (pass_env, endpoint_rewritten) = collect_pass_env(None)?;
    Ok(DockerLaunchSpec {
        image,
        container_args,
        mounts: vec![MountSpec {
            host: workspace.clone(),
            container: CONTAINER_WORKSPACE_ROOT,
        }],
        state_dir,
        pass_env,
        interactive: true,
        keep_container: args.docker.docker_keep_container,
        container_name: args
            .docker
            .docker_keep_container
            .then(|| default_container_name("session", &workspace)),
        host_workspace_root: Some(workspace),
        host_result_dir: None,
        endpoint_rewritten,
    })
}

fn build_run_spec(args: &crate::RunCliArgs) -> anyhow::Result<DockerLaunchSpec> {
    let start = &args.start;
    let workspace_arg = start
        .workspace
        .clone()
        .ok_or_else(|| anyhow::anyhow!("`quorp run --docker` requires --workspace <dir>"))?;
    let workspace = canonicalize_existing_path(&workspace_arg)?;
    let result_dir = start
        .result_dir
        .clone()
        .unwrap_or_else(|| crate::quorp::run_support::default_run_result_dir(&workspace, "run"));
    ensure_directory_exists(&result_dir)?;
    let state_dir = resolve_result_state_dir(&start.docker, &result_dir)?;
    let mut container_args = vec![
        "run".to_string(),
        "--workspace".to_string(),
        CONTAINER_WORKSPACE_ROOT.to_string(),
        "--result-dir".to_string(),
        CONTAINER_RESULTS_ROOT.to_string(),
        "--executor".to_string(),
        start
            .executor
            .to_possible_value()
            .expect("executor")
            .get_name()
            .to_string(),
        "--codex-session-mode".to_string(),
        start
            .codex_session_mode
            .to_possible_value()
            .expect("mode")
            .get_name()
            .to_string(),
        "--max-steps".to_string(),
        start.max_steps.to_string(),
        "--max-seconds".to_string(),
        start.max_seconds.to_string(),
        "--max-retries".to_string(),
        start.max_retries.to_string(),
        "--autonomy-profile".to_string(),
        start.autonomy_profile.clone(),
    ];
    if let Some(condition) = start.condition.as_ref() {
        container_args.push("--condition".to_string());
        container_args.push(condition.clone());
    }
    if let Some(objective_file) = start.objective_file.as_ref() {
        container_args.push("--objective-file".to_string());
        container_args.push(rewrite_optional_workspace_path(objective_file, &workspace)?);
    }
    if let Some(provider) = start.provider {
        container_args.push("--provider".to_string());
        container_args.push(
            provider
                .to_possible_value()
                .expect("provider")
                .get_name()
                .to_string(),
        );
    }
    if let Some(model) = start.model.as_ref() {
        container_args.push("--model".to_string());
        container_args.push(model.clone());
    }
    if let Some(base_url) = start.base_url.as_ref() {
        container_args.push("--base-url".to_string());
        container_args.push(rewrite_endpoint_if_needed(base_url).0);
    }
    if let Some(session_id) = start.codex_session_id.as_ref() {
        container_args.push("--codex-session-id".to_string());
        container_args.push(session_id.clone());
    }
    if let Some(max_total_tokens) = start.max_total_tokens {
        container_args.push("--max-total-tokens".to_string());
        container_args.push(max_total_tokens.to_string());
    }
    let image = start.docker.resolved_image();
    let (pass_env, endpoint_rewritten) = collect_pass_env(start.base_url.as_deref())?;
    Ok(DockerLaunchSpec {
        image,
        container_args,
        mounts: vec![
            MountSpec {
                host: workspace.clone(),
                container: CONTAINER_WORKSPACE_ROOT,
            },
            MountSpec {
                host: result_dir.clone(),
                container: CONTAINER_RESULTS_ROOT,
            },
        ],
        state_dir,
        pass_env,
        interactive: false,
        keep_container: start.docker.docker_keep_container,
        container_name: start
            .docker
            .docker_keep_container
            .then(|| default_container_name("run", &workspace)),
        host_workspace_root: Some(workspace),
        host_result_dir: Some(result_dir),
        endpoint_rewritten,
    })
}

fn build_agent_run_spec(args: &crate::AgentRunArgs) -> anyhow::Result<DockerLaunchSpec> {
    let workspace = canonicalize_existing_path(&args.workspace)?;
    let result_dir = absolutize_path(&args.result_dir)?;
    ensure_directory_exists(&result_dir)?;
    let state_dir = resolve_result_state_dir(&args.docker, &result_dir)?;
    let mut container_args = vec![
        "agent".to_string(),
        "run".to_string(),
        "--workspace".to_string(),
        CONTAINER_WORKSPACE_ROOT.to_string(),
        "--objective-file".to_string(),
        rewrite_optional_workspace_string(&args.objective_file, &workspace)?,
        "--model".to_string(),
        args.model.clone(),
        "--executor".to_string(),
        args.executor
            .to_possible_value()
            .expect("executor")
            .get_name()
            .to_string(),
        "--codex-session-mode".to_string(),
        args.codex_session_mode
            .to_possible_value()
            .expect("mode")
            .get_name()
            .to_string(),
        "--max-steps".to_string(),
        args.max_steps.to_string(),
        "--max-seconds".to_string(),
        args.max_seconds.to_string(),
        "--result-dir".to_string(),
        CONTAINER_RESULTS_ROOT.to_string(),
        "--autonomy-profile".to_string(),
        args.autonomy_profile.clone(),
    ];
    if let Some(base_url) = args.base_url.as_ref() {
        container_args.push("--base-url".to_string());
        container_args.push(rewrite_endpoint_if_needed(base_url).0);
    }
    if let Some(session_id) = args.codex_session_id.as_ref() {
        container_args.push("--codex-session-id".to_string());
        container_args.push(session_id.clone());
    }
    if let Some(max_total_tokens) = args.max_total_tokens {
        container_args.push("--max-total-tokens".to_string());
        container_args.push(max_total_tokens.to_string());
    }
    let image = args.docker.resolved_image();
    let (pass_env, endpoint_rewritten) = collect_pass_env(args.base_url.as_deref())?;
    Ok(DockerLaunchSpec {
        image,
        container_args,
        mounts: vec![
            MountSpec {
                host: workspace.clone(),
                container: CONTAINER_WORKSPACE_ROOT,
            },
            MountSpec {
                host: result_dir.clone(),
                container: CONTAINER_RESULTS_ROOT,
            },
        ],
        state_dir,
        pass_env,
        interactive: false,
        keep_container: args.docker.docker_keep_container,
        container_name: args
            .docker
            .docker_keep_container
            .then(|| default_container_name("agent-run", &workspace)),
        host_workspace_root: Some(workspace),
        host_result_dir: Some(result_dir),
        endpoint_rewritten,
    })
}

fn build_benchmark_run_spec(args: &crate::BenchmarkRunArgs) -> anyhow::Result<DockerLaunchSpec> {
    let source = canonicalize_existing_path(&args.path)?;
    let (mount_root, container_path) = rewrite_mountable_source(&source)?;
    let result_dir = absolutize_path(&args.result_dir)?;
    ensure_directory_exists(&result_dir)?;
    let state_dir = resolve_result_state_dir(&args.docker, &result_dir)?;
    let mut container_args = vec![
        "benchmark".to_string(),
        "run".to_string(),
        "--path".to_string(),
        container_path,
        "--result-dir".to_string(),
        CONTAINER_RESULTS_ROOT.to_string(),
        "--executor".to_string(),
        args.executor
            .to_possible_value()
            .expect("executor")
            .get_name()
            .to_string(),
        "--max-steps".to_string(),
        args.max_steps.to_string(),
        "--max-seconds".to_string(),
        args.max_seconds.to_string(),
        "--autonomy-profile".to_string(),
        args.autonomy_profile.clone(),
    ];
    if let Some(model) = args.model.as_ref() {
        container_args.push("--model".to_string());
        container_args.push(model.clone());
    }
    if let Some(base_url) = args.base_url.as_ref() {
        container_args.push("--base-url".to_string());
        container_args.push(rewrite_endpoint_if_needed(base_url).0);
    }
    if let Some(token_budget) = args.token_budget {
        container_args.push("--token-budget".to_string());
        container_args.push(token_budget.to_string());
    }
    if let Some(max_attempts) = args.max_attempts {
        container_args.push("--max-attempts".to_string());
        container_args.push(max_attempts.to_string());
    }
    if args.allow_heavy_local_model {
        container_args.push("--allow-heavy-local-model".to_string());
    }
    if let Some(condition) = args.condition.as_ref() {
        container_args.push("--condition".to_string());
        container_args.push(condition.clone());
    }
    if args.keep_sandbox {
        container_args.push("--keep-sandbox".to_string());
    }
    if let Some(log_file) = args.log_file.as_ref() {
        container_args.push("--log-file".to_string());
        container_args.push(rewrite_result_path(log_file, &result_dir)?);
    }
    let image = args.docker.resolved_image();
    let (pass_env, endpoint_rewritten) = collect_pass_env(args.base_url.as_deref())?;
    Ok(DockerLaunchSpec {
        image,
        container_args,
        mounts: vec![
            MountSpec {
                host: mount_root.clone(),
                container: CONTAINER_WORKSPACE_ROOT,
            },
            MountSpec {
                host: result_dir.clone(),
                container: CONTAINER_RESULTS_ROOT,
            },
        ],
        state_dir,
        pass_env,
        interactive: false,
        keep_container: args.docker.docker_keep_container,
        container_name: args
            .docker
            .docker_keep_container
            .then(|| default_container_name("benchmark-run", &mount_root)),
        host_workspace_root: Some(mount_root),
        host_result_dir: Some(result_dir),
        endpoint_rewritten,
    })
}

fn build_benchmark_batch_spec(
    args: &crate::BenchmarkBatchArgs,
) -> anyhow::Result<DockerLaunchSpec> {
    let source = canonicalize_existing_path(&args.cases_root)?;
    let result_dir = absolutize_path(&args.result_dir)?;
    ensure_directory_exists(&result_dir)?;
    let state_dir = resolve_result_state_dir(&args.docker, &result_dir)?;
    let mut container_args = vec![
        "benchmark".to_string(),
        "batch".to_string(),
        "--cases-root".to_string(),
        CONTAINER_WORKSPACE_ROOT.to_string(),
        "--result-dir".to_string(),
        CONTAINER_RESULTS_ROOT.to_string(),
        "--executor".to_string(),
        args.executor
            .to_possible_value()
            .expect("executor")
            .get_name()
            .to_string(),
        "--max-steps".to_string(),
        args.max_steps.to_string(),
        "--max-seconds".to_string(),
        args.max_seconds.to_string(),
        "--autonomy-profile".to_string(),
        args.autonomy_profile.clone(),
    ];
    if let Some(model) = args.model.as_ref() {
        container_args.push("--model".to_string());
        container_args.push(model.clone());
    }
    if let Some(base_url) = args.base_url.as_ref() {
        container_args.push("--base-url".to_string());
        container_args.push(rewrite_endpoint_if_needed(base_url).0);
    }
    if let Some(token_budget) = args.token_budget {
        container_args.push("--token-budget".to_string());
        container_args.push(token_budget.to_string());
    }
    if let Some(max_attempts) = args.max_attempts {
        container_args.push("--max-attempts".to_string());
        container_args.push(max_attempts.to_string());
    }
    if args.allow_heavy_local_model {
        container_args.push("--allow-heavy-local-model".to_string());
    }
    if let Some(condition) = args.condition.as_ref() {
        container_args.push("--condition".to_string());
        container_args.push(condition.clone());
    }
    if args.keep_sandbox {
        container_args.push("--keep-sandbox".to_string());
    }
    if let Some(log_dir) = args.log_dir.as_ref() {
        container_args.push("--log-dir".to_string());
        container_args.push(rewrite_result_path(log_dir, &result_dir)?);
    }
    let image = args.docker.resolved_image();
    let (pass_env, endpoint_rewritten) = collect_pass_env(args.base_url.as_deref())?;
    Ok(DockerLaunchSpec {
        image,
        container_args,
        mounts: vec![
            MountSpec {
                host: source.clone(),
                container: CONTAINER_WORKSPACE_ROOT,
            },
            MountSpec {
                host: result_dir.clone(),
                container: CONTAINER_RESULTS_ROOT,
            },
        ],
        state_dir,
        pass_env,
        interactive: false,
        keep_container: args.docker.docker_keep_container,
        container_name: args
            .docker
            .docker_keep_container
            .then(|| default_container_name("benchmark-batch", &source)),
        host_workspace_root: Some(source),
        host_result_dir: Some(result_dir),
        endpoint_rewritten,
    })
}

fn build_benchmark_resume_spec(
    args: &crate::BenchmarkResumeArgs,
) -> anyhow::Result<DockerLaunchSpec> {
    let result_dir = canonicalize_existing_path(&args.result_dir)?;
    let manifest_path = result_dir.join("benchmark-manifest.json");
    let manifest = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let manifest_json: serde_json::Value = serde_json::from_str(&manifest)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    let maybe_workspace_mount = benchmark_resume_workspace_mount(&manifest_json, &result_dir)?;
    let state_dir = resolve_result_state_dir(&args.docker, &result_dir)?;
    let image = args.docker.resolved_image();
    let pass_env = collect_pass_env(None)?.0;
    let mut mounts = vec![MountSpec {
        host: result_dir.clone(),
        container: CONTAINER_RESULTS_ROOT,
    }];
    if let Some(workspace_mount) = maybe_workspace_mount.as_ref() {
        mounts.push(MountSpec {
            host: workspace_mount.clone(),
            container: CONTAINER_WORKSPACE_ROOT,
        });
    }
    Ok(DockerLaunchSpec {
        image,
        container_args: vec![
            "benchmark".to_string(),
            "resume".to_string(),
            "--result-dir".to_string(),
            CONTAINER_RESULTS_ROOT.to_string(),
        ],
        mounts,
        state_dir,
        pass_env,
        interactive: false,
        keep_container: args.docker.docker_keep_container,
        container_name: args
            .docker
            .docker_keep_container
            .then(|| default_container_name("benchmark-resume", &result_dir)),
        host_workspace_root: maybe_workspace_mount,
        host_result_dir: Some(result_dir),
        endpoint_rewritten: false,
    })
}

#[allow(clippy::disallowed_methods)]
fn launch(spec: DockerLaunchSpec) -> anyhow::Result<i32> {
    ensure_directory_exists(&spec.state_dir)?;
    let mut command = Command::new("docker");
    command.arg("run");
    command.arg("--init");
    command.arg("--cap-drop").arg("ALL");
    command.arg("--security-opt").arg("no-new-privileges");
    if !spec.keep_container {
        command.arg("--rm");
    }
    #[cfg(unix)]
    {
        let user = format!("{}:{}", nix_like_uid(), nix_like_gid());
        command.arg("--user").arg(user);
    }
    if spec.interactive {
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            command.arg("-it");
        } else {
            command.arg("-i");
        }
    }
    if cfg!(target_os = "linux") {
        command
            .arg("--add-host")
            .arg("host.docker.internal:host-gateway");
    }
    if let Some(name) = spec.container_name.as_ref() {
        command.arg("--name").arg(name);
    }
    for mount in &spec.mounts {
        if !mount.host.exists() {
            anyhow::bail!(
                "docker mount source does not exist: {}",
                mount.host.display()
            );
        }
        command
            .arg("-v")
            .arg(format!("{}:{}", mount.host.display(), mount.container));
    }
    command.arg("-v").arg(format!(
        "{}:{}",
        spec.state_dir.display(),
        CONTAINER_STATE_ROOT
    ));
    command
        .arg("-w")
        .arg(if spec.host_workspace_root.is_some() {
            CONTAINER_WORKSPACE_ROOT
        } else {
            CONTAINER_RESULTS_ROOT
        });

    let mut pass_env = spec.pass_env;
    pass_env.insert(ENV_IN_DOCKER.to_string(), "1".to_string());
    pass_env.insert(ENV_RUNTIME_MODE.to_string(), "docker".to_string());
    pass_env.insert(
        "QUORP_DATA_DIR".to_string(),
        CONTAINER_DATA_ROOT.to_string(),
    );
    pass_env.insert("HOME".to_string(), CONTAINER_HOME_ROOT.to_string());
    pass_env.insert(
        "CODEX_HOME".to_string(),
        CONTAINER_CODEX_HOME_ROOT.to_string(),
    );
    pass_env.insert(ENV_DOCKER_IMAGE_USED.to_string(), spec.image.clone());
    pass_env.insert(ENV_DOCKER_NETWORK_MODE.to_string(), "default".to_string());
    pass_env.insert(
        ENV_DOCKER_ENDPOINT_REWRITTEN.to_string(),
        spec.endpoint_rewritten.to_string(),
    );
    if let Some(workspace_root) = spec.host_workspace_root.as_ref() {
        pass_env.insert(
            ENV_DOCKER_HOST_WORKSPACE_ROOT.to_string(),
            workspace_root.display().to_string(),
        );
        pass_env.insert(
            ENV_DOCKER_CONTAINER_WORKSPACE_ROOT.to_string(),
            CONTAINER_WORKSPACE_ROOT.to_string(),
        );
    }
    if let Some(result_dir) = spec.host_result_dir.as_ref() {
        pass_env.insert(
            ENV_DOCKER_HOST_RESULT_DIR.to_string(),
            result_dir.display().to_string(),
        );
    }
    if let Some(name) = spec.container_name.as_ref() {
        pass_env.insert(ENV_DOCKER_CONTAINER_NAME.to_string(), name.clone());
    }
    for (key, value) in pass_env {
        command.arg("-e").arg(format!("{key}={value}"));
    }

    command.arg(&spec.image);
    command.args(&spec.container_args);
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());

    let status = command.status().with_context(|| {
        "failed to launch Docker; is `docker` installed and running?".to_string()
    })?;
    Ok(status.code().unwrap_or(1))
}

fn collect_pass_env(
    cli_base_url: Option<&str>,
) -> anyhow::Result<(BTreeMap<String, String>, bool)> {
    let mut envs = BTreeMap::new();
    let mut endpoint_rewritten = false;
    for (key, value) in std::env::vars() {
        if should_passthrough_env(&key) {
            let rewritten = match key.as_str() {
                "QUORP_OLLAMA_HOST" | "QUORP_CHAT_BASE_URL" => {
                    let (value, changed) = rewrite_endpoint_if_needed(&value);
                    endpoint_rewritten |= changed;
                    value
                }
                _ => value,
            };
            envs.insert(key, rewritten);
        }
    }
    if let Some(base_url) = cli_base_url {
        endpoint_rewritten |= rewrite_endpoint_if_needed(base_url).1;
    }
    Ok((envs, endpoint_rewritten))
}

fn should_passthrough_env(key: &str) -> bool {
    if matches!(
        key,
        "QUORP_SANDBOX"
            | "QUORP_DOCKER_IMAGE"
            | "QUORP_DOCKER_STATE_DIR"
            | ENV_IN_DOCKER
            | ENV_RUNTIME_MODE
            | ENV_DOCKER_HOST_WORKSPACE_ROOT
            | ENV_DOCKER_CONTAINER_WORKSPACE_ROOT
            | ENV_DOCKER_HOST_RESULT_DIR
            | ENV_DOCKER_IMAGE_USED
            | ENV_DOCKER_NETWORK_MODE
            | ENV_DOCKER_ENDPOINT_REWRITTEN
            | ENV_DOCKER_CONTAINER_NAME
    ) {
        return false;
    }
    if key.starts_with("QUORP_") {
        return true;
    }
    matches!(
        key,
        "OPENAI_API_KEY"
            | "NVIDIA_API_KEY"
            | "ANTHROPIC_API_KEY"
            | "OPENROUTER_API_KEY"
            | "GOOGLE_API_KEY"
            | "XAI_API_KEY"
            | "MISTRAL_API_KEY"
            | "DEEPSEEK_API_KEY"
    )
}

fn rewrite_endpoint_if_needed(value: &str) -> (String, bool) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return (value.to_string(), false);
    }
    let Ok(mut url) = url::Url::parse(trimmed) else {
        return (value.to_string(), false);
    };
    let Some(host) = url.host_str() else {
        return (value.to_string(), false);
    };
    if host != "localhost" && host != "127.0.0.1" {
        return (value.to_string(), false);
    }
    if url.set_host(Some("host.docker.internal")).is_err() {
        return (value.to_string(), false);
    }
    (url.to_string(), true)
}

fn benchmark_resume_workspace_mount(
    manifest: &serde_json::Value,
    result_dir: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let challenge = manifest.get("challenge");
    if let Some(sandbox_root) = challenge
        .and_then(|value| value.get("sandbox_root"))
        .and_then(serde_json::Value::as_str)
    {
        let sandbox_root = PathBuf::from(sandbox_root);
        if sandbox_root.starts_with(result_dir) {
            return Ok(None);
        }
    }
    let benchmark_root = manifest
        .get("resolved")
        .and_then(|value| value.get("benchmark_root"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            anyhow::anyhow!("benchmark resume manifest is missing resolved.benchmark_root")
        })?;
    Ok(Some(canonicalize_existing_path(Path::new(benchmark_root))?))
}

fn rewrite_optional_workspace_path(path: &Path, workspace_root: &Path) -> anyhow::Result<String> {
    if !path.is_absolute() {
        return Ok(path.display().to_string());
    }
    rewrite_workspace_path(path, workspace_root)
}

fn rewrite_optional_workspace_string(raw: &str, workspace_root: &Path) -> anyhow::Result<String> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Ok(raw.to_string());
    }
    rewrite_workspace_path(path, workspace_root)
}

fn rewrite_workspace_path(path: &Path, workspace_root: &Path) -> anyhow::Result<String> {
    let absolute = canonicalize_existing_path(path)?;
    let workspace_root = canonicalize_existing_path(workspace_root)?;
    let relative = absolute.strip_prefix(&workspace_root).with_context(|| {
        format!(
            "{} is outside mounted workspace {}",
            absolute.display(),
            workspace_root.display()
        )
    })?;
    if relative.as_os_str().is_empty() {
        return Ok(CONTAINER_WORKSPACE_ROOT.to_string());
    }
    Ok(Path::new(CONTAINER_WORKSPACE_ROOT)
        .join(relative)
        .display()
        .to_string())
}

fn rewrite_result_path(path: &Path, result_dir: &Path) -> anyhow::Result<String> {
    let absolute = absolutize_path(path)?;
    let result_dir = absolutize_path(result_dir)?;
    let relative = absolute.strip_prefix(&result_dir).with_context(|| {
        format!(
            "{} is outside mounted result dir {}",
            absolute.display(),
            result_dir.display()
        )
    })?;
    if relative.as_os_str().is_empty() {
        return Ok(CONTAINER_RESULTS_ROOT.to_string());
    }
    Ok(Path::new(CONTAINER_RESULTS_ROOT)
        .join(relative)
        .display()
        .to_string())
}

fn resolve_session_state_dir(args: &DockerArgs, workspace_root: &Path) -> anyhow::Result<PathBuf> {
    if let Some(path) = args.docker_state_dir.clone().or_else(|| {
        std::env::var("QUORP_DOCKER_STATE_DIR")
            .ok()
            .map(PathBuf::from)
    }) {
        let path = absolutize_path(&path)?;
        ensure_directory_exists(&path)?;
        return Ok(path);
    }
    let name = workspace_root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("workspace");
    let path = ::paths::state_dir().join("docker-sessions").join(format!(
        "{}-{:016x}",
        sanitize_component(name),
        stable_hash(workspace_root)
    ));
    ensure_directory_exists(&path)?;
    Ok(path)
}

fn resolve_result_state_dir(args: &DockerArgs, result_dir: &Path) -> anyhow::Result<PathBuf> {
    if let Some(path) = args.docker_state_dir.clone().or_else(|| {
        std::env::var("QUORP_DOCKER_STATE_DIR")
            .ok()
            .map(PathBuf::from)
    }) {
        let path = absolutize_path(&path)?;
        ensure_directory_exists(&path)?;
        return Ok(path);
    }
    let path = result_dir.join("container-state");
    ensure_directory_exists(&path)?;
    Ok(path)
}

fn canonicalize_existing_path(path: &Path) -> anyhow::Result<PathBuf> {
    std::fs::canonicalize(path).with_context(|| format!("failed to resolve {}", path.display()))
}

fn rewrite_mountable_source(path: &Path) -> anyhow::Result<(PathBuf, String)> {
    if path.is_dir() {
        return Ok((path.to_path_buf(), CONTAINER_WORKSPACE_ROOT.to_string()));
    }
    let parent = path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "{} has no parent directory for Docker mounting",
            path.display()
        )
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{} does not have a valid UTF-8 file name for Docker mounting",
                path.display()
            )
        })?;
    Ok((
        parent.to_path_buf(),
        Path::new(CONTAINER_WORKSPACE_ROOT)
            .join(file_name)
            .display()
            .to_string(),
    ))
}

fn absolutize_path(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()
        .context("failed to determine current directory")?
        .join(path))
}

fn ensure_directory_exists(path: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))
}

fn default_container_name(scope: &str, path: &Path) -> String {
    format!(
        "quorp-{}-{}-{}",
        sanitize_component(scope),
        sanitize_component(
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("workspace")
        ),
        stable_hash(path)
    )
}

fn sanitize_component(value: &str) -> String {
    let mut rendered = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            rendered.push(character.to_ascii_lowercase());
        } else if character == '-' || character == '_' {
            rendered.push(character);
        } else {
            rendered.push('-');
        }
    }
    while rendered.contains("--") {
        rendered = rendered.replace("--", "-");
    }
    rendered.trim_matches('-').to_string()
}

fn stable_hash(path: &Path) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET;
    for byte in path.as_os_str().to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(unix)]
fn nix_like_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(unix)]
fn nix_like_gid() -> u32 {
    unsafe { libc::getgid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn rewrites_localhost_endpoints_for_docker() {
        let (rewritten, changed) = rewrite_endpoint_if_needed("http://127.0.0.1:11434/v1");
        assert!(changed);
        assert_eq!(rewritten, "http://host.docker.internal:11434/v1");
    }

    #[test]
    fn leaves_non_loopback_endpoints_unchanged() {
        let (rewritten, changed) = rewrite_endpoint_if_needed("https://api.openai.com/v1");
        assert!(!changed);
        assert_eq!(rewritten, "https://api.openai.com/v1");
    }

    #[test]
    fn runtime_metadata_defaults_to_host() {
        let _guard = TEST_ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::remove_var(ENV_RUNTIME_MODE);
        }
        assert_eq!(
            runtime_metadata_json()["mode"],
            serde_json::Value::String("host".into())
        );
    }

    #[test]
    fn docker_args_respect_env_fallback() {
        let _guard = TEST_ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::set_var("QUORP_SANDBOX", "docker");
        }
        assert!(DockerArgs::default().enabled());
        unsafe {
            std::env::remove_var("QUORP_SANDBOX");
        }
    }

    #[test]
    fn docker_passthrough_includes_nvidia_api_key() {
        assert!(should_passthrough_env("NVIDIA_API_KEY"));
    }

    #[test]
    fn rewrite_workspace_path_maps_inside_container() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let workspace = temp_dir.path().join("workspace");
        let nested = workspace.join("src").join("main.rs");
        std::fs::create_dir_all(nested.parent().expect("parent")).expect("mkdirs");
        std::fs::write(&nested, "fn main() {}\n").expect("write");
        let rewritten = rewrite_workspace_path(&nested, &workspace).expect("rewrite");
        assert_eq!(rewritten, "/workspace/src/main.rs");
    }

    #[test]
    fn result_state_dir_defaults_under_result_dir() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let result_dir = temp_dir.path().join("result");
        std::fs::create_dir_all(&result_dir).expect("mkdir");
        let state_dir =
            resolve_result_state_dir(&DockerArgs::default(), &result_dir).expect("state dir");
        assert_eq!(state_dir, result_dir.join("container-state"));
    }
}

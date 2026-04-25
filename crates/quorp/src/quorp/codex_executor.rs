use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use quorp_agent_core::{AgentRunOutcome, StopReason, TranscriptMessage, TranscriptRole};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};

use crate::quorp::executor::{CodexSessionMode, CodexSessionStrategy};

const DEFAULT_CODEX_MODEL_ID: &str = "gpt-5.3-codex-spark";
const DEFAULT_CODEX_REASONING_EFFORT: &str = "low";
const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub struct CodexRunOptions {
    pub workspace: PathBuf,
    pub objective_file: PathBuf,
    pub model_id: String,
    pub max_steps: usize,
    pub max_seconds: Option<u64>,
    pub max_total_tokens: Option<u64>,
    pub result_dir: PathBuf,
    pub session_strategy: CodexSessionStrategy,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodexPromptBundle {
    pub workspace: PathBuf,
    pub objective_file: PathBuf,
    pub prompt: String,
    pub prompt_fingerprint: String,
    pub prompt_token_estimate: u64,
}

pub type CodexProgressCallback = Arc<dyn Fn(CodexProgressEvent) + Send + Sync + 'static>;

#[derive(Debug, Clone)]
pub enum CodexProgressEvent {
    ThreadStarted {
        thread_id: Option<String>,
    },
    AssistantMessage {
        text: String,
    },
    CommandExecution {
        command: String,
        exit_code: Option<i32>,
        status: String,
    },
    TurnCompleted {
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
    },
}

#[derive(Clone)]
pub struct CodexTaskRunOptions {
    pub workspace: PathBuf,
    pub prompt: String,
    pub prompt_fingerprint: String,
    pub user_message: String,
    pub model_id: String,
    pub max_steps: usize,
    pub max_seconds: Option<u64>,
    pub max_total_tokens: Option<u64>,
    pub result_dir: PathBuf,
    pub session_strategy: CodexSessionStrategy,
    pub metadata: Value,
    pub progress_callback: Option<CodexProgressCallback>,
}

#[derive(Debug, Clone)]
pub struct CodexCompletionOptions {
    pub workspace: PathBuf,
    pub prompt: String,
    pub model_id: String,
    pub max_seconds: Option<u64>,
    pub artifact_dir: PathBuf,
    pub session_strategy: CodexSessionStrategy,
}

#[derive(Debug, Clone)]
pub struct CodexCompletionResult {
    pub content: String,
    pub raw_response: Value,
}

#[derive(Debug)]
struct ExecutorConfig {
    codex_binary: PathBuf,
    auth_json: PathBuf,
    codex_home: PathBuf,
    effort: String,
}

struct SessionRequest {
    workspace: PathBuf,
    model_id: String,
    prompt: String,
    max_seconds: Option<u64>,
    artifact_dir: PathBuf,
    session_strategy: CodexSessionStrategy,
    progress_callback: Option<CodexProgressCallback>,
}

#[derive(Debug)]
struct SessionResult {
    assistant_messages: Vec<String>,
    command_executions: Vec<CommandExecutionRecord>,
    usage: Option<CodexUsage>,
    model_requests: usize,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    last_message_path: PathBuf,
    prompt_path: PathBuf,
    session_path: PathBuf,
    status_code: Option<i32>,
    timed_out: bool,
    duration_ms: u64,
    error_message: Option<String>,
    session_metadata: CodexSessionMetadata,
}

#[derive(Debug, Clone, Serialize)]
struct CommandExecutionRecord {
    command: String,
    exit_code: Option<i32>,
    status: String,
}

#[derive(Debug, Clone, Serialize, Default)]
struct CodexUsage {
    input_tokens: u64,
    output_tokens: u64,
    total_billed_tokens: u64,
    reasoning_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodexSessionMetadata {
    pub requested_mode: String,
    pub resolved_mode: String,
    pub requested_session_id: Option<String>,
    pub resolved_session_id: Option<String>,
    pub resolved_thread_title: Option<String>,
    pub resolution_source: String,
    pub fallback_reason: Option<String>,
    pub cwd_match: bool,
    pub used_isolated_codex_home: bool,
    pub codex_home: PathBuf,
    pub status_code: Option<i32>,
    pub timed_out: bool,
}

#[derive(Debug)]
struct ResolvedSessionTarget {
    invocation: SessionInvocation,
    metadata: CodexSessionMetadata,
}

#[derive(Debug)]
enum SessionInvocation {
    Fresh,
    ResumeById(String),
    ResumeLast,
}

#[derive(Debug)]
struct ResolvedCodexThread {
    id: String,
    title: String,
}

struct SyntheticEventsSummary<'a> {
    model_id: &'a str,
    prompt_token_estimate: u64,
    usage: Option<&'a CodexUsage>,
    model_requests: usize,
    total_steps: usize,
    stop_reason: StopReason,
    total_billed_tokens: u64,
    duration_ms: u64,
    command_executions: &'a [CommandExecutionRecord],
}

pub fn default_model_id() -> String {
    std::env::var("QUORP_CODEX_MODEL").unwrap_or_else(|_| DEFAULT_CODEX_MODEL_ID.to_string())
}

pub fn fresh_session_strategy() -> CodexSessionStrategy {
    CodexSessionStrategy::fresh()
}

pub fn default_interactive_artifact_dir(workspace: &Path, scope: &str) -> PathBuf {
    let workspace_name = workspace
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("workspace");
    std::env::temp_dir()
        .join("quorp-codex")
        .join(workspace_name)
        .join(scope)
        .join(format!("{}-{}", timestamp_ms(), std::process::id()))
}

pub fn build_benchmark_prompt_bundle(
    workspace: &Path,
    objective_file: &Path,
    max_steps: usize,
    max_seconds: Option<u64>,
    max_total_tokens: Option<u64>,
) -> anyhow::Result<CodexPromptBundle> {
    let objective_path = resolve_objective_path(workspace, objective_file);
    let objective_text = fs::read_to_string(&objective_path)
        .with_context(|| format!("failed to read {}", objective_path.display()))?;
    let prompt = render_benchmark_prompt(
        workspace,
        &objective_path,
        &objective_text,
        max_steps,
        max_seconds,
        max_total_tokens,
    );
    let prompt_fingerprint = fingerprint_benchmark_prompt(&prompt, workspace, &objective_path);
    let prompt_token_estimate = estimate_token_count(&prompt);
    Ok(CodexPromptBundle {
        workspace: workspace.to_path_buf(),
        objective_file: objective_path,
        prompt,
        prompt_fingerprint,
        prompt_token_estimate,
    })
}

pub fn run_codex_agent(options: CodexRunOptions) -> anyhow::Result<AgentRunOutcome> {
    let prompt_bundle = build_benchmark_prompt_bundle(
        &options.workspace,
        &options.objective_file,
        options.max_steps,
        options.max_seconds,
        options.max_total_tokens,
    )?;
    let objective_path = prompt_bundle.objective_file.clone();
    let prompt_fingerprint = prompt_bundle.prompt_fingerprint.clone();
    run_codex_task(CodexTaskRunOptions {
        workspace: options.workspace,
        prompt: prompt_bundle.prompt,
        prompt_fingerprint: prompt_fingerprint.clone(),
        user_message: format!(
            "Run the benchmark objective from `{}` inside the real workspace and finish only after local validation is complete.",
            objective_path.display()
        ),
        model_id: options.model_id,
        max_steps: options.max_steps,
        max_seconds: options.max_seconds,
        max_total_tokens: options.max_total_tokens,
        result_dir: options.result_dir,
        session_strategy: options.session_strategy,
        metadata: json!({
            "objective_file": objective_path,
            "prompt_fingerprint": prompt_fingerprint,
        }),
        progress_callback: None,
    })
}

pub fn run_codex_task(options: CodexTaskRunOptions) -> anyhow::Result<AgentRunOutcome> {
    fs::create_dir_all(&options.result_dir)?;
    fs::create_dir_all(options.result_dir.join("artifacts"))?;
    let prompt_token_estimate = estimate_token_count(&options.prompt);
    write_json(
        &options.result_dir.join("request.json"),
        &json!({
            "executor": "codex",
            "workspace": options.workspace,
            "model_id": options.model_id,
            "max_steps": options.max_steps,
            "max_seconds": options.max_seconds,
            "max_total_tokens": options.max_total_tokens,
            "prompt_token_estimate": prompt_token_estimate,
            "prompt": options.prompt,
            "prompt_fingerprint": options.prompt_fingerprint,
            "session_strategy": {
                "mode": options.session_strategy.mode.label(),
                "session_id": options.session_strategy.session_id,
            },
            "metadata": options.metadata,
        }),
    )?;

    let session = match run_session(SessionRequest {
        workspace: options.workspace.clone(),
        model_id: options.model_id.clone(),
        prompt: options.prompt.clone(),
        max_seconds: options.max_seconds,
        artifact_dir: options.result_dir.join("artifacts").join("codex"),
        session_strategy: options.session_strategy.clone(),
        progress_callback: options.progress_callback.clone(),
    }) {
        Ok(session) => session,
        Err(error) => {
            return write_failed_task_artifacts(
                &options,
                prompt_token_estimate,
                vec![TranscriptMessage {
                    role: TranscriptRole::User,
                    content: options.user_message.clone(),
                }],
                error.to_string(),
            );
        }
    };

    let transcript = build_transcript(&options.user_message, &session.assistant_messages);
    let stop_reason = if session.timed_out {
        StopReason::TimeBudgetExhausted
    } else if session.error_message.is_some() {
        StopReason::FatalError
    } else {
        StopReason::Success
    };
    let usage = session.usage.clone();
    let total_billed_tokens = usage
        .as_ref()
        .map(|value| value.total_billed_tokens)
        .unwrap_or_default();
    let total_steps = session.model_requests.max(1);
    write_synthetic_events(
        &options.result_dir.join("events.jsonl"),
        SyntheticEventsSummary {
            model_id: &options.model_id,
            prompt_token_estimate,
            usage: session.usage.as_ref(),
            model_requests: session.model_requests,
            total_steps,
            stop_reason,
            total_billed_tokens,
            duration_ms: session.duration_ms,
            command_executions: &session.command_executions,
        },
    )?;
    write_json(
        &options.result_dir.join("summary.json"),
        &json!({
            "stop_reason": stop_reason,
            "total_steps": total_steps,
            "total_billed_tokens": total_billed_tokens,
            "duration_ms": session.duration_ms,
            "error_message": session.error_message,
            "usage": usage_summary_json(session.model_requests, usage.as_ref()),
        }),
    )?;
    write_json(&options.result_dir.join("transcript.json"), &transcript)?;
    write_json(
        &options.result_dir.join("metadata.json"),
        &json!({
            "executor": "codex",
            "workspace": options.workspace,
            "model_id": options.model_id,
            "prompt_token_estimate": prompt_token_estimate,
            "artifacts": {
                "prompt": session.prompt_path,
                "stdout_jsonl": session.stdout_path,
                "stderr_log": session.stderr_path,
                "last_message": session.last_message_path,
                "session": session.session_path,
            },
            "model_requests": session.model_requests,
            "duration_ms": session.duration_ms,
            "timed_out": session.timed_out,
            "status_code": session.status_code,
            "command_executions": session.command_executions,
            "session": session.session_metadata,
            "task_metadata": options.metadata,
        }),
    )?;
    write_final_diff(&options.workspace, &options.result_dir.join("final.diff"))?;

    Ok(AgentRunOutcome {
        stop_reason,
        total_steps,
        total_billed_tokens,
        duration_ms: session.duration_ms,
        transcript,
        error_message: session.error_message,
    })
}

pub fn request_codex_completion(
    options: CodexCompletionOptions,
) -> anyhow::Result<CodexCompletionResult> {
    let session = run_session(SessionRequest {
        workspace: options.workspace.clone(),
        model_id: options.model_id.clone(),
        prompt: options.prompt,
        max_seconds: options.max_seconds,
        artifact_dir: options.artifact_dir,
        session_strategy: options.session_strategy,
        progress_callback: None,
    })?;
    if session.timed_out {
        anyhow::bail!("Codex completion timed out after {}ms", session.duration_ms);
    }
    if let Some(error_message) = session.error_message.clone() {
        anyhow::bail!("{error_message}");
    }
    let Some(content) = session.assistant_messages.last().cloned() else {
        anyhow::bail!("Codex completion finished without an assistant message");
    };
    Ok(CodexCompletionResult {
        content,
        raw_response: json!({
            "executor": "codex",
            "model_requests": session.model_requests,
            "usage": session.usage,
            "artifacts": {
                "stdout_jsonl": session.stdout_path,
                "stderr_log": session.stderr_path,
                "last_message": session.last_message_path,
                "prompt": session.prompt_path,
                "session": session.session_path,
            },
            "session": session.session_metadata,
        }),
    })
}

fn usage_summary_json(model_requests: usize, usage: Option<&CodexUsage>) -> Value {
    json!({
        "model_requests": model_requests,
        "reported_billed_tokens": usage.map(|value| value.total_billed_tokens).unwrap_or_default(),
        "estimated_billed_tokens": 0,
        "total_billed_tokens": usage.map(|value| value.total_billed_tokens).unwrap_or_default(),
        "input_tokens": usage.map(|value| value.input_tokens).unwrap_or_default(),
        "output_tokens": usage.map(|value| value.output_tokens).unwrap_or_default(),
        "reasoning_tokens": usage.and_then(|value| value.reasoning_tokens).unwrap_or_default(),
        "cache_read_input_tokens": usage.and_then(|value| value.cache_read_input_tokens).unwrap_or_default(),
        "cache_write_input_tokens": usage.and_then(|value| value.cache_write_input_tokens).unwrap_or_default(),
    })
}

#[allow(clippy::disallowed_methods)]
fn run_session(request: SessionRequest) -> anyhow::Result<SessionResult> {
    let config = ExecutorConfig::from_env()?;
    fs::create_dir_all(&request.artifact_dir)?;
    let prompt_path = request.artifact_dir.join("prompt.txt");
    let stdout_path = request.artifact_dir.join("stdout.jsonl");
    let stderr_path = request.artifact_dir.join("stderr.log");
    let last_message_path = request.artifact_dir.join("last-message.txt");
    let session_path = request.artifact_dir.join("session.json");
    fs::write(&prompt_path, &request.prompt)
        .with_context(|| format!("failed to write {}", prompt_path.display()))?;

    let resolved_target =
        resolve_session_target(&config, &request.workspace, &request.session_strategy)?;
    let command_environment =
        prepare_command_environment(&request.artifact_dir, &config, &resolved_target)?;

    let stdout_file = File::create(&stdout_path)
        .with_context(|| format!("failed to create {}", stdout_path.display()))?;
    let stderr_file = File::create(&stderr_path)
        .with_context(|| format!("failed to create {}", stderr_path.display()))?;

    let started_at = Instant::now();
    let mut streamed_stdout_bytes = 0usize;
    let mut command = Command::new(&config.codex_binary);
    command.current_dir(&request.workspace);
    match &resolved_target.invocation {
        SessionInvocation::Fresh => {
            command
                .arg("exec")
                .arg("--skip-git-repo-check")
                .arg("-C")
                .arg(&request.workspace);
        }
        SessionInvocation::ResumeById(session_id) => {
            command.arg("exec").arg("resume").arg(session_id);
        }
        SessionInvocation::ResumeLast => {
            command.arg("exec").arg("resume").arg("--last");
        }
    }
    command
        .arg("--json")
        .arg("--output-last-message")
        .arg(&last_message_path)
        .arg("--model")
        .arg(&request.model_id)
        .arg("-c")
        .arg(format!("model_reasoning_effort={:?}", config.effort))
        .arg("-c")
        .arg("approval_policy=\"never\"")
        .arg("-c")
        .arg("sandbox_mode=\"danger-full-access\"");
    if matches!(resolved_target.invocation, SessionInvocation::Fresh) {
        command
            .arg("-c")
            .arg("use_memories=false")
            .arg("-c")
            .arg("generate_memories=false");
    }
    command
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    for (key, value) in command_environment {
        command.env(key, value);
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn `{}`", config.codex_binary.display()))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(request.prompt.as_bytes())
            .with_context(|| "failed to write Codex prompt".to_string())?;
    }

    let mut timed_out = false;
    let status_code = loop {
        stream_codex_progress(
            &stdout_path,
            &request.progress_callback,
            &mut streamed_stdout_bytes,
        )?;
        if let Some(status) = child
            .try_wait()
            .with_context(|| "failed to poll Codex child".to_string())?
        {
            break status.code();
        }
        if request
            .max_seconds
            .is_some_and(|value| started_at.elapsed() >= Duration::from_secs(value))
        {
            timed_out = true;
            child
                .kill()
                .with_context(|| "failed to terminate timed out Codex process".to_string())?;
            let status = child
                .wait()
                .with_context(|| "failed to wait for terminated Codex process".to_string())?;
            break status.code();
        }
        thread::sleep(POLL_INTERVAL);
    };
    stream_codex_progress(
        &stdout_path,
        &request.progress_callback,
        &mut streamed_stdout_bytes,
    )?;

    let stdout_text = fs::read_to_string(&stdout_path).unwrap_or_default();
    let stderr_text = fs::read_to_string(&stderr_path).unwrap_or_default();
    let last_message = fs::read_to_string(&last_message_path).unwrap_or_default();
    let parsed = parse_stdout(&stdout_text, &last_message)?;
    let mut session_metadata = resolved_target.metadata;
    if parsed.thread_id.is_some() {
        session_metadata.resolved_session_id = parsed.thread_id.clone();
    }
    session_metadata.status_code = status_code;
    session_metadata.timed_out = timed_out;
    write_json(&session_path, &session_metadata)?;

    let error_message = if timed_out {
        Some(format!(
            "Codex executor timed out after {} seconds",
            request.max_seconds.unwrap_or_default()
        ))
    } else if status_code.unwrap_or(-1) != 0 {
        Some(build_failure_message(
            status_code,
            &stderr_text,
            &parsed.assistant_messages,
        ))
    } else {
        None
    };

    Ok(SessionResult {
        assistant_messages: parsed.assistant_messages,
        command_executions: parsed.command_executions,
        usage: parsed.usage,
        model_requests: parsed.model_requests,
        stdout_path,
        stderr_path,
        last_message_path,
        prompt_path,
        session_path,
        status_code,
        timed_out,
        duration_ms: started_at.elapsed().as_millis() as u64,
        error_message,
        session_metadata,
    })
}

fn prepare_command_environment(
    artifact_dir: &Path,
    config: &ExecutorConfig,
    target: &ResolvedSessionTarget,
) -> anyhow::Result<Vec<(String, PathBuf)>> {
    if !target.metadata.used_isolated_codex_home {
        return Ok(Vec::new());
    }
    let home_dir = artifact_dir.join("home");
    let codex_home = home_dir.join(".codex");
    fs::create_dir_all(&codex_home)?;
    fs::create_dir_all(home_dir.join(".cargo"))?;
    fs::create_dir_all(home_dir.join(".rustup"))?;
    fs::copy(&config.auth_json, codex_home.join("auth.json")).with_context(|| {
        format!(
            "failed to copy Codex auth file from {}",
            config.auth_json.display()
        )
    })?;
    Ok(vec![
        ("HOME".to_string(), home_dir.clone()),
        ("CODEX_HOME".to_string(), codex_home),
        ("CARGO_HOME".to_string(), home_dir.join(".cargo")),
        ("RUSTUP_HOME".to_string(), home_dir.join(".rustup")),
    ])
}

fn resolve_session_target(
    config: &ExecutorConfig,
    workspace: &Path,
    strategy: &CodexSessionStrategy,
) -> anyhow::Result<ResolvedSessionTarget> {
    let requested_session_id = strategy.session_id.clone();
    let base_metadata =
        |resolved_mode: &str,
         resolution_source: &str,
         fallback_reason: Option<String>,
         cwd_match: bool,
         used_isolated_codex_home: bool,
         resolved_session_id: Option<String>,
         resolved_thread_title: Option<String>| CodexSessionMetadata {
            requested_mode: strategy.mode.label().to_string(),
            resolved_mode: resolved_mode.to_string(),
            requested_session_id: requested_session_id.clone(),
            resolved_session_id,
            resolved_thread_title,
            resolution_source: resolution_source.to_string(),
            fallback_reason,
            cwd_match,
            used_isolated_codex_home,
            codex_home: config.codex_home.clone(),
            status_code: None,
            timed_out: false,
        };

    match strategy.mode {
        CodexSessionMode::Fresh => Ok(ResolvedSessionTarget {
            invocation: SessionInvocation::Fresh,
            metadata: base_metadata("fresh", "fresh", None, false, true, None, None),
        }),
        CodexSessionMode::ResumeId => {
            let session_id = strategy
                .session_id
                .clone()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("Codex session mode `resume-id` requires a session id")
                })?;
            let thread = lookup_thread_by_id(&config.codex_home, &session_id)?;
            Ok(ResolvedSessionTarget {
                invocation: SessionInvocation::ResumeById(session_id.clone()),
                metadata: base_metadata(
                    "resume-id",
                    "explicit-id",
                    None,
                    thread.as_ref().is_some_and(|value| value.id == session_id),
                    false,
                    Some(session_id),
                    thread.map(|value| value.title),
                ),
            })
        }
        CodexSessionMode::ResumeLast => {
            if let Some(thread) = lookup_latest_thread(&config.codex_home)? {
                Ok(ResolvedSessionTarget {
                    invocation: SessionInvocation::ResumeById(thread.id.clone()),
                    metadata: base_metadata(
                        "resume-last",
                        "latest-thread",
                        None,
                        false,
                        false,
                        Some(thread.id),
                        Some(thread.title),
                    ),
                })
            } else {
                Ok(ResolvedSessionTarget {
                    invocation: SessionInvocation::ResumeLast,
                    metadata: base_metadata(
                        "resume-last",
                        "codex-cli-last",
                        None,
                        false,
                        false,
                        None,
                        None,
                    ),
                })
            }
        }
        CodexSessionMode::ResumeLastForCwd => {
            if let Some(thread) = lookup_latest_thread_for_cwd(&config.codex_home, workspace)? {
                Ok(ResolvedSessionTarget {
                    invocation: SessionInvocation::ResumeById(thread.id.clone()),
                    metadata: base_metadata(
                        "resume-last-for-cwd",
                        "latest-thread-for-cwd",
                        None,
                        true,
                        false,
                        Some(thread.id),
                        Some(thread.title),
                    ),
                })
            } else {
                Ok(ResolvedSessionTarget {
                    invocation: SessionInvocation::Fresh,
                    metadata: base_metadata(
                        "fresh",
                        "fresh-fallback",
                        Some(format!(
                            "No persisted Codex session matched workspace {}",
                            workspace.display()
                        )),
                        false,
                        true,
                        None,
                        None,
                    ),
                })
            }
        }
    }
}

fn open_thread_db(codex_home: &Path) -> anyhow::Result<Option<Connection>> {
    let Some(database_path) = latest_codex_state_db(codex_home)? else {
        return Ok(None);
    };
    let connection = Connection::open_with_flags(database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok(Some(connection))
}

fn latest_codex_state_db(codex_home: &Path) -> anyhow::Result<Option<PathBuf>> {
    if !codex_home.exists() {
        return Ok(None);
    }
    let mut candidates = Vec::new();
    for entry in fs::read_dir(codex_home)
        .with_context(|| format!("failed to read {}", codex_home.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !name.starts_with("state_") || !name.ends_with(".sqlite") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH);
        candidates.push((modified, path));
    }
    candidates.sort_by(|left, right| right.0.cmp(&left.0));
    Ok(candidates.into_iter().next().map(|(_, path)| path))
}

fn lookup_thread_by_id(
    codex_home: &Path,
    session_id: &str,
) -> anyhow::Result<Option<ResolvedCodexThread>> {
    let Some(connection) = open_thread_db(codex_home)? else {
        return Ok(None);
    };
    connection
        .query_row(
            "select id, title from threads where id = ?1 and archived = 0 limit 1",
            params![session_id],
            |row| {
                Ok(ResolvedCodexThread {
                    id: row.get(0)?,
                    title: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(anyhow::Error::from)
}

fn lookup_latest_thread(codex_home: &Path) -> anyhow::Result<Option<ResolvedCodexThread>> {
    let Some(connection) = open_thread_db(codex_home)? else {
        return Ok(None);
    };
    connection
        .query_row(
            "select id, title from threads where archived = 0 order by updated_at desc limit 1",
            [],
            |row| {
                Ok(ResolvedCodexThread {
                    id: row.get(0)?,
                    title: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(anyhow::Error::from)
}

fn lookup_latest_thread_for_cwd(
    codex_home: &Path,
    workspace: &Path,
) -> anyhow::Result<Option<ResolvedCodexThread>> {
    let Some(connection) = open_thread_db(codex_home)? else {
        return Ok(None);
    };
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf())
        .display()
        .to_string();
    connection
        .query_row(
            "select id, title from threads where archived = 0 and cwd = ?1 order by updated_at desc limit 1",
            params![workspace],
            |row| {
                Ok(ResolvedCodexThread {
                    id: row.get(0)?,
                    title: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(anyhow::Error::from)
}

fn resolve_objective_path(workspace: &Path, objective_file: &Path) -> PathBuf {
    if objective_file.is_absolute() {
        objective_file.to_path_buf()
    } else {
        workspace.join(objective_file)
    }
}

fn render_benchmark_prompt(
    workspace: &Path,
    objective_path: &Path,
    objective_text: &str,
    max_steps: usize,
    max_seconds: Option<u64>,
    max_total_tokens: Option<u64>,
) -> String {
    format!(
        "You are running a Quorp benchmark directly inside the real workspace at `{workspace}`.\n\
Work autonomously until the issue is fixed or you hit a hard stop.\n\
Read the benchmark brief below carefully, inspect the real files on disk, make the smallest correct changes, and run local validation before finishing.\n\
Prefer the owning crate and expected touch targets before widening.\n\
Treat `{workspace}` as the only editable workspace. If the brief references source paths outside this directory, translate them to the equivalent files under `{workspace}` before reading or editing.\n\
Quorp will run the final benchmark evaluators after you stop, so do your own local validation but do not wait for extra approval.\n\
Return a short final summary with files changed, validation commands run, and whether widening was required.\n\n\
## Soft Limits\n\
- Max agent steps hint: `{max_steps}`\n\
- Max seconds: `{max_seconds}`\n\
- Remaining token budget hint: `{token_budget}`\n\n\
## Benchmark Brief\n\
Source objective path: `{objective_path}`\n\n\
{objective_text}\n",
        workspace = workspace.display(),
        max_steps = max_steps,
        max_seconds = max_seconds
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string()),
        token_budget = max_total_tokens
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string()),
        objective_path = objective_path.display(),
    )
}

fn fingerprint_benchmark_prompt(prompt: &str, workspace: &Path, objective_path: &Path) -> String {
    let normalized = prompt
        .replace(&workspace.display().to_string(), "<workspace>")
        .replace(&objective_path.display().to_string(), "<objective_path>");
    stable_fingerprint(&normalized)
}

pub fn fingerprint_prompt_text(prompt: &str) -> String {
    stable_fingerprint(prompt)
}

fn stable_fingerprint(text: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn build_transcript(user_message: &str, assistant_messages: &[String]) -> Vec<TranscriptMessage> {
    let mut transcript = vec![TranscriptMessage {
        role: TranscriptRole::User,
        content: user_message.to_string(),
    }];
    transcript.extend(
        assistant_messages
            .iter()
            .cloned()
            .map(|content| TranscriptMessage {
                role: TranscriptRole::Assistant,
                content,
            }),
    );
    transcript
}

fn write_failed_task_artifacts(
    options: &CodexTaskRunOptions,
    prompt_token_estimate: u64,
    transcript: Vec<TranscriptMessage>,
    error_message: String,
) -> anyhow::Result<AgentRunOutcome> {
    write_synthetic_events(
        &options.result_dir.join("events.jsonl"),
        SyntheticEventsSummary {
            model_id: &options.model_id,
            prompt_token_estimate,
            usage: None,
            model_requests: 0,
            total_steps: 0,
            stop_reason: StopReason::FatalError,
            total_billed_tokens: 0,
            duration_ms: 0,
            command_executions: &[],
        },
    )?;
    write_json(
        &options.result_dir.join("summary.json"),
        &json!({
            "stop_reason": StopReason::FatalError,
            "total_steps": 0,
            "total_billed_tokens": 0,
            "duration_ms": 0,
            "error_message": error_message,
            "usage": usage_summary_json(0, None),
        }),
    )?;
    write_json(&options.result_dir.join("transcript.json"), &transcript)?;
    write_json(
        &options.result_dir.join("metadata.json"),
        &json!({
            "executor": "codex",
            "workspace": options.workspace,
            "model_id": options.model_id,
            "prompt": options.prompt,
            "prompt_fingerprint": options.prompt_fingerprint,
            "prompt_token_estimate": prompt_token_estimate,
            "error_message": error_message,
            "task_metadata": options.metadata,
        }),
    )?;
    write_final_diff(&options.workspace, &options.result_dir.join("final.diff"))?;
    Ok(AgentRunOutcome {
        stop_reason: StopReason::FatalError,
        total_steps: 0,
        total_billed_tokens: 0,
        duration_ms: 0,
        transcript,
        error_message: Some(error_message),
    })
}

fn write_synthetic_events(path: &Path, summary: SyntheticEventsSummary<'_>) -> anyhow::Result<()> {
    let mut lines = Vec::new();
    lines.push(json!({
        "ts_ms": 0,
        "payload": {
            "event": "run_started",
            "goal": "codex benchmark execution",
            "model_id": summary.model_id,
        }
    }));
    let request_count = summary.model_requests.max(1);
    for request_id in 0..request_count {
        lines.push(json!({
            "ts_ms": 0,
            "payload": {
                "event": "model_request_started",
                "step": request_id + 1,
                "request_id": request_id + 1,
                "message_count": 1,
                "prompt_token_estimate": summary.prompt_token_estimate,
                "completion_token_cap": Value::Null,
                "safety_mode": "codex",
            }
        }));
        for command in summary.command_executions {
            lines.push(json!({
                "ts_ms": 0,
                "payload": {
                    "event": "tool_call_started",
                    "step": request_id + 1,
                    "action": command.command,
                }
            }));
            if is_validation_command(&command.command) {
                lines.push(json!({
                    "ts_ms": 0,
                    "payload": {
                        "event": "validation_started",
                        "step": request_id + 1,
                        "summary": command.command,
                    }
                }));
                lines.push(json!({
                    "ts_ms": 0,
                    "payload": {
                        "event": "validation_finished",
                        "step": request_id + 1,
                        "summary": command.command,
                        "status": if command.exit_code.unwrap_or_default() == 0 { "success" } else { "failure" },
                    }
                }));
            }
            lines.push(json!({
                "ts_ms": 0,
                "payload": {
                    "event": "tool_call_finished",
                    "step": request_id + 1,
                    "action": command.command,
                    "status": command.status,
                }
            }));
        }
        lines.push(json!({
            "ts_ms": 0,
            "payload": {
                "event": "model_request_finished",
                "step": request_id + 1,
                "request_id": request_id + 1,
                "usage": summary.usage.map(|usage| {
                    json!({
                        "input_tokens": usage.input_tokens,
                        "output_tokens": usage.output_tokens,
                        "total_billed_tokens": usage.total_billed_tokens,
                        "reasoning_tokens": usage.reasoning_tokens,
                        "cache_read_input_tokens": usage.cache_read_input_tokens,
                        "cache_write_input_tokens": usage.cache_write_input_tokens,
                        "provider_request_id": Value::Null,
                        "latency_ms": 0,
                        "finish_reason": "stop",
                        "usage_source": "reported",
                    })
                }),
                "watchdog": Value::Null,
            }
        }));
    }
    lines.push(json!({
        "ts_ms": 0,
        "payload": {
            "event": "run_finished",
            "reason": summary.stop_reason,
            "total_steps": summary.total_steps,
            "total_billed_tokens": summary.total_billed_tokens,
            "duration_ms": summary.duration_ms,
        }
    }));

    let mut file =
        File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    for line in lines {
        writeln!(file, "{line}").with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(())
}

fn parse_stdout(stdout: &str, last_message: &str) -> anyhow::Result<ParsedStdout> {
    let mut assistant_messages = Vec::new();
    let mut command_executions = Vec::new();
    let mut usage = CodexUsage::default();
    let mut usage_seen = false;
    let mut model_requests = 0usize;
    let mut thread_id = None;

    for line in stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some(event) = parse_progress_event_line(line)? {
            apply_progress_event(
                &event,
                &mut assistant_messages,
                &mut command_executions,
                &mut usage,
                &mut usage_seen,
                &mut model_requests,
                &mut thread_id,
            );
        }
    }

    let trimmed_last_message = last_message.trim();
    if assistant_messages.is_empty() && !trimmed_last_message.is_empty() {
        assistant_messages.push(trimmed_last_message.to_string());
    }

    Ok(ParsedStdout {
        assistant_messages,
        command_executions,
        usage: usage_seen.then_some(usage),
        model_requests,
        thread_id,
    })
}

fn stream_codex_progress(
    stdout_path: &Path,
    progress_callback: &Option<CodexProgressCallback>,
    streamed_stdout_bytes: &mut usize,
) -> anyhow::Result<()> {
    let Some(progress_callback) = progress_callback.as_ref() else {
        return Ok(());
    };
    let stdout_text = match fs::read_to_string(stdout_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", stdout_path.display()));
        }
    };
    if stdout_text.len() <= *streamed_stdout_bytes {
        return Ok(());
    }
    let fresh = &stdout_text[*streamed_stdout_bytes..];
    if !fresh.ends_with('\n') {
        return Ok(());
    }
    for line in fresh.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(event) = parse_progress_event_line(line)? {
            progress_callback(event);
        }
    }
    *streamed_stdout_bytes = stdout_text.len();
    Ok(())
}

fn parse_progress_event_line(line: &str) -> anyhow::Result<Option<CodexProgressEvent>> {
    let value: Value = serde_json::from_str(line)
        .with_context(|| format!("failed to parse Codex JSONL line: {line}"))?;
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let event = match event_type {
        "thread.started" => Some(CodexProgressEvent::ThreadStarted {
            thread_id: value
                .get("thread_id")
                .and_then(Value::as_str)
                .map(|value| value.to_string()),
        }),
        "item.completed" => {
            let item = value.get("item").cloned().unwrap_or_else(|| json!({}));
            match item.get("type").and_then(Value::as_str) {
                Some("agent_message") => item.get("text").and_then(Value::as_str).map(|text| {
                    CodexProgressEvent::AssistantMessage {
                        text: text.to_string(),
                    }
                }),
                Some("command_execution") => Some(CodexProgressEvent::CommandExecution {
                    command: item
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    exit_code: item
                        .get("exit_code")
                        .and_then(Value::as_i64)
                        .map(|value| value as i32),
                    status: item
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("completed")
                        .to_string(),
                }),
                _ => None,
            }
        }
        "turn.completed" => Some(CodexProgressEvent::TurnCompleted {
            input_tokens: value
                .get("usage")
                .and_then(|usage| usage.get("input_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            output_tokens: value
                .get("usage")
                .and_then(|usage| usage.get("output_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            cached_input_tokens: value
                .get("usage")
                .and_then(|usage| usage.get("cached_input_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or_default(),
        }),
        _ => None,
    };
    Ok(event)
}

fn apply_progress_event(
    event: &CodexProgressEvent,
    assistant_messages: &mut Vec<String>,
    command_executions: &mut Vec<CommandExecutionRecord>,
    usage: &mut CodexUsage,
    usage_seen: &mut bool,
    model_requests: &mut usize,
    thread_id: &mut Option<String>,
) {
    match event {
        CodexProgressEvent::ThreadStarted { thread_id: value } => {
            *thread_id = value.clone();
        }
        CodexProgressEvent::AssistantMessage { text } => {
            assistant_messages.push(text.clone());
        }
        CodexProgressEvent::CommandExecution {
            command,
            exit_code,
            status,
        } => {
            command_executions.push(CommandExecutionRecord {
                command: command.clone(),
                exit_code: *exit_code,
                status: status.clone(),
            });
        }
        CodexProgressEvent::TurnCompleted {
            input_tokens,
            output_tokens,
            cached_input_tokens,
        } => {
            *model_requests += 1;
            usage.input_tokens = usage.input_tokens.saturating_add(*input_tokens);
            usage.output_tokens = usage.output_tokens.saturating_add(*output_tokens);
            usage.cache_read_input_tokens = Some(
                usage
                    .cache_read_input_tokens
                    .unwrap_or_default()
                    .saturating_add(*cached_input_tokens),
            );
            usage.total_billed_tokens = usage.input_tokens.saturating_add(usage.output_tokens);
            *usage_seen = true;
        }
    }
}

fn build_failure_message(
    status_code: Option<i32>,
    stderr_text: &str,
    assistant_messages: &[String],
) -> String {
    let stderr_excerpt = stderr_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join(" | ");
    let assistant_excerpt = assistant_messages
        .last()
        .map(|text| truncate_for_error(text))
        .unwrap_or_default();
    if !stderr_excerpt.is_empty() {
        format!(
            "Codex executor exited with status {}: {}",
            status_code.unwrap_or(-1),
            stderr_excerpt
        )
    } else if !assistant_excerpt.is_empty() {
        format!(
            "Codex executor exited with status {} after saying: {}",
            status_code.unwrap_or(-1),
            assistant_excerpt
        )
    } else {
        format!(
            "Codex executor exited with status {}",
            status_code.unwrap_or(-1)
        )
    }
}

fn truncate_for_error(text: &str) -> String {
    let mut truncated = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= 180 {
            truncated.push_str("...");
            break;
        }
        truncated.push(ch);
    }
    truncated.replace('\n', " ")
}

fn estimate_token_count(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4).max(1)
}

pub(crate) fn is_validation_command(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    normalized.contains("cargo test")
        || normalized.contains("cargo nextest")
        || normalized.contains("cargo clippy")
        || normalized.contains("cargo fmt")
        || normalized.contains("./evaluate")
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(path, bytes).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

#[allow(clippy::disallowed_methods)]
fn write_final_diff(workspace: &Path, output_path: &Path) -> anyhow::Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .arg("diff")
        .output();
    match output {
        Ok(output) if output.status.success() || !output.stdout.is_empty() => {
            fs::write(output_path, output.stdout)
                .with_context(|| format!("failed to write {}", output_path.display()))?;
        }
        _ => {
            fs::write(
                output_path,
                b"final diff unavailable for non-git workspace\n",
            )
            .with_context(|| format!("failed to write {}", output_path.display()))?;
        }
    }
    Ok(())
}

impl ExecutorConfig {
    fn from_env() -> anyhow::Result<Self> {
        let codex_binary =
            PathBuf::from(std::env::var("QUORP_CODEX_BIN").unwrap_or_else(|_| "codex".to_string()));
        let effort = std::env::var("QUORP_CODEX_EFFORT")
            .unwrap_or_else(|_| DEFAULT_CODEX_REASONING_EFFORT.to_string());
        let codex_home = resolve_codex_home_path();
        let auth_json = resolve_auth_json_path(&codex_home)?;
        Ok(Self {
            codex_binary,
            auth_json,
            codex_home,
            effort,
        })
    }
}

fn resolve_codex_home_path() -> PathBuf {
    if let Ok(path) = std::env::var("CODEX_HOME") {
        let path = PathBuf::from(path);
        if path.exists() {
            return path;
        }
    }
    paths::home_dir().join(".codex")
}

fn resolve_auth_json_path(codex_home: &Path) -> anyhow::Result<PathBuf> {
    if let Ok(path) = std::env::var("QUORP_CODEX_AUTH_JSON") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Ok(path);
        }
        anyhow::bail!(
            "QUORP_CODEX_AUTH_JSON points to missing file {}",
            path.display()
        );
    }
    let auth_path = codex_home.join("auth.json");
    if auth_path.exists() {
        return Ok(auth_path);
    }
    anyhow::bail!(
        "failed to locate Codex auth.json; set QUORP_CODEX_AUTH_JSON or ensure {}/auth.json exists",
        codex_home.display()
    )
}

#[derive(Debug, Default)]
struct ParsedStdout {
    assistant_messages: Vec<String>,
    command_executions: Vec<CommandExecutionRecord>,
    usage: Option<CodexUsage>,
    model_requests: usize,
    thread_id: Option<String>,
}

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stdout_collects_usage_messages_and_commands() {
        let stdout = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"123\"}\n",
            "{\"type\":\"turn.started\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"1\",\"type\":\"command_execution\",\"command\":\"cargo test -p demo --quiet\",\"aggregated_output\":\"ok\\n\",\"exit_code\":0,\"status\":\"completed\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"2\",\"type\":\"agent_message\",\"text\":\"done\"}}\n",
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":12,\"cached_input_tokens\":5,\"output_tokens\":7}}\n"
        );

        let parsed = parse_stdout(stdout, "").expect("parse stdout");
        assert_eq!(parsed.thread_id.as_deref(), Some("123"));
        assert_eq!(parsed.assistant_messages, vec!["done".to_string()]);
        assert_eq!(parsed.command_executions.len(), 1);
        assert_eq!(parsed.model_requests, 1);
        let usage = parsed.usage.expect("usage");
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.total_billed_tokens, 19);
        assert_eq!(usage.cache_read_input_tokens, Some(5));
    }

    #[test]
    fn parse_progress_event_line_maps_command_execution() {
        let event = parse_progress_event_line(
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\",\"command\":\"cargo test -p demo\",\"exit_code\":0,\"status\":\"completed\"}}",
        )
        .expect("parse line")
        .expect("event");
        assert!(matches!(
            event,
            CodexProgressEvent::CommandExecution {
                ref command,
                exit_code: Some(0),
                ref status
            } if command == "cargo test -p demo" && status == "completed"
        ));
    }

    #[test]
    fn parse_stdout_falls_back_to_last_message_file() {
        let parsed = parse_stdout(
            "{\"type\":\"turn.started\"}\n{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"cached_input_tokens\":0,\"output_tokens\":1}}\n",
            "final answer",
        )
        .expect("parse stdout");
        assert_eq!(parsed.assistant_messages, vec!["final answer".to_string()]);
    }

    #[test]
    fn validation_detection_matches_common_commands() {
        assert!(is_validation_command(
            "/bin/bash -lc cargo test -p demo --quiet"
        ));
        assert!(is_validation_command("bash ./evaluate.sh"));
        assert!(!is_validation_command(
            "/bin/bash -lc rg preview_change_reason"
        ));
    }

    #[test]
    fn benchmark_prompt_fingerprint_normalizes_absolute_paths() {
        let prompt_a = render_benchmark_prompt(
            Path::new("/tmp/workspace-a"),
            Path::new("/tmp/workspace-a/QUORP_BENCHMARK_OBJECTIVE.md"),
            "objective",
            100,
            Some(3600),
            Some(1200),
        );
        let prompt_b = render_benchmark_prompt(
            Path::new("/tmp/workspace-b"),
            Path::new("/tmp/workspace-b/QUORP_BENCHMARK_OBJECTIVE.md"),
            "objective",
            100,
            Some(3600),
            Some(1200),
        );
        assert_eq!(
            fingerprint_benchmark_prompt(
                &prompt_a,
                Path::new("/tmp/workspace-a"),
                Path::new("/tmp/workspace-a/QUORP_BENCHMARK_OBJECTIVE.md"),
            ),
            fingerprint_benchmark_prompt(
                &prompt_b,
                Path::new("/tmp/workspace-b"),
                Path::new("/tmp/workspace-b/QUORP_BENCHMARK_OBJECTIVE.md"),
            )
        );
    }

    #[test]
    fn resume_last_for_cwd_falls_back_to_fresh_without_state_db() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = ExecutorConfig {
            codex_binary: PathBuf::from("codex"),
            auth_json: temp_dir.path().join("auth.json"),
            codex_home: temp_dir.path().join(".codex"),
            effort: "low".to_string(),
        };
        let strategy = CodexSessionStrategy {
            mode: CodexSessionMode::ResumeLastForCwd,
            session_id: None,
        };
        let resolved =
            resolve_session_target(&config, Path::new("/tmp/project"), &strategy).expect("resolve");
        assert!(matches!(resolved.invocation, SessionInvocation::Fresh));
        assert_eq!(resolved.metadata.resolved_mode, "fresh");
        assert!(resolved.metadata.fallback_reason.is_some());
    }

    #[test]
    fn lookup_latest_thread_for_cwd_reads_state_db() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let codex_home = temp_dir.path().join(".codex");
        fs::create_dir_all(&codex_home).expect("mkdir");
        let db_path = codex_home.join("state_5.sqlite");
        let connection = Connection::open(&db_path).expect("db");
        connection
            .execute_batch(
                "create table threads (
                    id text primary key,
                    rollout_path text not null,
                    created_at integer not null,
                    updated_at integer not null,
                    source text not null,
                    model_provider text not null,
                    cwd text not null,
                    title text not null,
                    sandbox_policy text not null,
                    approval_mode text not null,
                    tokens_used integer not null default 0,
                    has_user_event integer not null default 0,
                    archived integer not null default 0,
                    archived_at integer,
                    git_sha text,
                    git_branch text,
                    git_origin_url text,
                    cli_version text not null default '',
                    first_user_message text not null default '',
                    agent_nickname text,
                    agent_role text,
                    memory_mode text not null default 'enabled',
                    model text,
                    reasoning_effort text,
                    agent_path text
                );",
            )
            .expect("schema");
        connection
            .execute(
                "insert into threads (id, rollout_path, created_at, updated_at, source, model_provider, cwd, title, sandbox_policy, approval_mode, archived)
                 values (?1, '', 1, 2, 'vscode', 'openai', ?2, 'Current thread', 'danger-full-access', 'never', 0)",
                params!["thread-1", "/tmp/project"],
            )
            .expect("insert");
        let thread =
            lookup_latest_thread_for_cwd(&codex_home, Path::new("/tmp/project")).expect("lookup");
        assert_eq!(thread.expect("thread").id, "thread-1");
    }
}

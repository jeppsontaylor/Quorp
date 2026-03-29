//! Native agent tool schemas and project-backed execution for the TUI chat bridge.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agent::{
    AgentTool as _, CommandOutputToolInput, ListDirectoryToolInput, ReadFileToolInput,
    TerminalTool, TerminalToolInput, ToolPermissionDecision, built_in_tools,
    decide_permission_from_settings, list_directory_headless, terminal_command_guardrail_rejection,
};
use agent_settings::AgentSettings;
use collections::HashMap as StdHashMap;
use fs::Fs;
use futures::future::Either;
use gpui::Entity;
use language_model::{LanguageModelRequestTool, LanguageModelToolResultContent, LanguageModelToolUse};
use project::Project;
use settings::Settings as _;
use task::{SaveStrategy, Shell, ShellBuilder, SpawnInTerminal, TaskId};
use terminal::Terminal;
use util::get_default_system_shell_preferring_bash;
use uuid::Uuid;

const COMMAND_OUTPUT_LIMIT: usize = 16 * 1024;

fn background_terminal_registry() -> &'static Mutex<HashMap<String, Entity<Terminal>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, Entity<Terminal>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::default()))
}

pub fn tui_chat_tools() -> Vec<LanguageModelRequestTool> {
    const NAMES: &[&str] = &["terminal", "command_output", "read_file", "list_directory"];
    built_in_tools()
        .filter(|t| NAMES.iter().any(|n| *n == t.name.as_str()))
        .collect()
}

pub async fn execute_tui_tool_call(
    tool_use: &LanguageModelToolUse,
    project: &Entity<Project>,
    fs: Arc<dyn Fs>,
    async_cx: &gpui::AsyncApp,
) -> Result<LanguageModelToolResultContent, String> {
    match tool_use.name.as_ref() {
        "terminal" => {
            let input: TerminalToolInput = serde_json::from_value(tool_use.input.clone())
                .map_err(|e| format!("terminal tool arguments: {e}"))?;
            run_terminal_tool(input, project, async_cx).await
        }
        "command_output" => {
            let input: CommandOutputToolInput = serde_json::from_value(tool_use.input.clone())
                .map_err(|e| format!("command_output arguments: {e}"))?;
            run_command_output_tool(input, async_cx).await
        }
        "read_file" => {
            let input: ReadFileToolInput = serde_json::from_value(tool_use.input.clone())
                .map_err(|e| format!("read_file arguments: {e}"))?;
            read_file_tool(input, project, fs, async_cx).await
        }
        "list_directory" => {
            let input: ListDirectoryToolInput = serde_json::from_value(tool_use.input.clone())
                .map_err(|e| format!("list_directory arguments: {e}"))?;
            list_directory_tool(input, project, async_cx).await
        }
        other => Err(format!(
            "Tool `{other}` is not enabled in the TUI (enabled: terminal, command_output, read_file, list_directory)."
        )),
    }
}

async fn run_command_output_tool(
    input: CommandOutputToolInput,
    async_cx: &gpui::AsyncApp,
) -> Result<LanguageModelToolResultContent, String> {
    let terminal_id = input.terminal_id.trim().to_string();
    if terminal_id.is_empty() {
        return Err("command_output: terminal_id is empty".to_string());
    }

    let output = async_cx
        .update(|cx| -> Result<String, String> {
            let map = background_terminal_registry()
                .lock()
                .map_err(|_| "terminal registry lock poisoned".to_string())?;
            let term = map.get(&terminal_id).ok_or_else(|| {
                format!(
                    "unknown terminal_id `{terminal_id}`; use the id returned by a background `terminal` tool run"
                )
            })?;
            Ok(term.read(cx).get_content())
        })
        .map_err(|e| e.to_string())?;

    let final_out = if output.len() > COMMAND_OUTPUT_LIMIT {
        let mut end = COMMAND_OUTPUT_LIMIT.min(output.len());
        while end > 0 && !output.is_char_boundary(end) {
            end -= 1;
        }
        output[..end].to_string()
    } else {
        output
    };

    Ok(LanguageModelToolResultContent::Text(final_out.into()))
}

async fn run_terminal_tool(
    input: TerminalToolInput,
    project: &Entity<Project>,
    async_cx: &gpui::AsyncApp,
) -> Result<LanguageModelToolResultContent, String> {
    if let Some(reason) = terminal_command_guardrail_rejection(&input.command) {
        return Err(format!(
            "Command rejected by security guardrails ({reason}). Ask the user for explicit permission or try a different approach."
        ));
    }

    let permission_result: Result<(), String> = async_cx.update(|cx| {
        let settings = AgentSettings::get_global(cx);
        match decide_permission_from_settings(
            TerminalTool::NAME,
            std::slice::from_ref(&input.command),
            settings,
        ) {
            ToolPermissionDecision::Allow => Ok(()),
            ToolPermissionDecision::Deny(reason) => Err(reason),
            ToolPermissionDecision::Confirm => Err(
                "This command requires approval. Add an allow rule for it in Quorp agent settings, or run it from the GUI agent where you can confirm."
                    .to_string(),
            ),
        }
    });

    permission_result?;

    let timeout = input
        .timeout_ms
        .map(Duration::from_millis)
        .filter(|duration| !duration.is_zero())
        .unwrap_or(Duration::from_secs(120));

    let cwd: std::path::PathBuf = async_cx
        .update(|cx| resolve_terminal_cwd(&input, project, cx))
        .map_err(|error| error.to_string())?;

    let task_id_str = format!("tui-tool-{}", Uuid::new_v4());
    let task_id = TaskId(task_id_str.clone());

    let spawn_task = async_cx.update(|cx| {
        project.update(cx, |project, cx| {
            let is_windows = project.path_style(cx).is_windows();
            let shell_str = project
                .remote_client()
                .and_then(|remote| remote.read(cx).default_system_shell())
                .unwrap_or_else(get_default_system_shell_preferring_bash);
            let (task_command, task_args) =
                ShellBuilder::new(&Shell::Program(shell_str), is_windows)
                    .redirect_stdin_to_dev_null()
                    .build(Some(input.command.clone()), &[]);

            let mut env = StdHashMap::default();
            env.insert("PAGER".into(), String::new());
            env.insert("GIT_PAGER".into(), "cat".into());

            let spawn = SpawnInTerminal {
                id: task_id,
                full_label: input.command.clone(),
                label: input.command.clone(),
                command: Some(task_command),
                args: task_args,
                command_label: input.command.clone(),
                cwd: Some(cwd),
                env,
                save: SaveStrategy::None,
                show_summary: false,
                show_command: false,
                ..Default::default()
            };
            project.create_terminal_task(spawn, cx)
        })
    });

    let terminal: Entity<Terminal> = spawn_task
        .await
        .map_err(|e| format!("spawn terminal task: {e:#}"))?;

    if input.run_in_background.unwrap_or(false) {
        async_cx
            .update(|_cx| -> Result<(), String> {
                let mut map = background_terminal_registry()
                    .lock()
                    .map_err(|_| "terminal registry lock poisoned".to_string())?;
                map.insert(task_id_str.clone(), terminal.clone());
                Ok(())
            })
            .map_err(|e| e.to_string())?;

        return Ok(LanguageModelToolResultContent::Text(
            format!(
                "Background job started. terminal_id: {task_id_str}\nUse the `command_output` tool with this exact terminal_id to read output while it runs or after it finishes."
            )
            .into(),
        ));
    }

    let wait_task = async_cx.update(|cx| terminal.read(cx).wait_for_completed_task(cx));

    let sleep = async_cx.background_executor().timer(timeout);
    futures::pin_mut!(wait_task);
    futures::pin_mut!(sleep);
    let (exit_status, timed_out) = match futures::future::select(wait_task, sleep).await {
        Either::Left((status, _)) => (status, false),
        Either::Right((_, pending_wait)) => {
            let _ = async_cx.update(|cx| {
                terminal.update(cx, |terminal: &mut Terminal, _cx| {
                    terminal.kill_active_task();
                });
            });
            (pending_wait.await, true)
        }
    };

    let output = async_cx.update(|cx| terminal.read(cx).get_content());

    let mut final_out = if output.len() > COMMAND_OUTPUT_LIMIT {
        let mut end = COMMAND_OUTPUT_LIMIT.min(output.len());
        while end > 0 && !output.is_char_boundary(end) {
            end -= 1;
        }
        output[..end].to_string()
    } else {
        output
    };

    if timed_out {
        final_out.push_str(&format!("\n[Command timed out after {:?}]", timeout));
    } else if let Some(status) = exit_status {
        final_out.push_str(&format!("\n[Exit code: {:?}]", status.code()));
    }

    Ok(LanguageModelToolResultContent::Text(final_out.into()))
}

fn resolve_terminal_cwd(
    input: &TerminalToolInput,
    project: &Entity<Project>,
    cx: &mut gpui::App,
) -> Result<std::path::PathBuf, anyhow::Error> {
    let project = project.read(cx);
    let cd = &input.cd;
    if cd == "." || cd.is_empty() {
        let mut worktrees = project.worktrees(cx);
        let Some(worktree) = worktrees.next() else {
            anyhow::bail!("no worktree");
        };
        anyhow::ensure!(
            worktrees.next().is_none(),
            "'.' is ambiguous in multi-root workspaces; specify a root directory in `cd`."
        );
        Ok(worktree.read(cx).abs_path().to_path_buf())
    } else {
        let input_path = Path::new(cd);
        if input_path.is_absolute() {
            if project.worktrees(cx).any(|worktree| {
                input_path.starts_with(worktree.read(cx).abs_path())
            }) {
                return Ok(input_path.into());
            }
        } else if let Some(worktree) = project.worktree_for_root_name(cd, cx) {
            return Ok(worktree.read(cx).abs_path().to_path_buf());
        }
        anyhow::bail!("`cd` directory {cd:?} was not in any of the project's worktrees.");
    }
}

async fn read_file_tool(
    input: ReadFileToolInput,
    project: &Entity<Project>,
    fs: Arc<dyn Fs>,
    async_cx: &gpui::AsyncApp,
) -> Result<LanguageModelToolResultContent, String> {
    let abs_path: std::path::PathBuf = async_cx.update(|cx| {
        let proj = project.read(cx);
        let project_path = proj
            .find_project_path(&input.path, cx)
            .ok_or_else(|| format!("path not in project: {}", input.path))?;
        proj.absolute_path(&project_path, cx)
            .ok_or_else(|| format!("could not resolve absolute path: {}", input.path))
    })?;

    let mut text = fs
        .load(&abs_path)
        .await
        .map_err(|e| format!("read {}: {e}", abs_path.display()))?;

    if let Some(start) = input.start_line {
        let lines: Vec<&str> = text.lines().collect();
        let start_idx = start.saturating_sub(1) as usize;
        let end_idx = input
            .end_line
            .map(|line| line as usize)
            .unwrap_or(lines.len())
            .min(lines.len());
        text = if start_idx < lines.len() {
            lines[start_idx..end_idx].join("\n")
        } else {
            String::new()
        };
    }

    Ok(LanguageModelToolResultContent::Text(text.into()))
}

async fn list_directory_tool(
    input: ListDirectoryToolInput,
    project: &Entity<Project>,
    async_cx: &gpui::AsyncApp,
) -> Result<LanguageModelToolResultContent, String> {
    let text = list_directory_headless(project, &input, async_cx)
        .await
        .map_err(|e| e.to_string())?;
    Ok(LanguageModelToolResultContent::Text(text.into()))
}

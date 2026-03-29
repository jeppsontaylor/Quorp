//! Runs `<run_command>` blocks via [`project::Project::create_terminal_task`] (Phase 3h — same stack
//! as the agent terminal tool: shell, cwd, permissions) and forwards output as [`ChatUiEvent`]s.
//!
//! Wired from `main` for the production TUI; see [`crate::quorp::tui::command_runner`] for the
//! non-integrated PTY fallback.

use std::path::PathBuf;
use std::process::ExitStatus;
use std::time::Duration;

use agent::{
    AgentTool as _, TerminalTool, ToolPermissionDecision, decide_permission_from_settings,
    terminal_command_guardrail_rejection,
};
use agent_settings::AgentSettings;
use collections::HashMap;
use futures::{StreamExt as _, future::Either};
use gpui::Entity;
use project::Project;
use settings::Settings;
use task::{SaveStrategy, Shell, ShellBuilder, SpawnInTerminal, TaskId};
use terminal::Terminal;
use util::get_default_system_shell_preferring_bash;
use uuid::Uuid;

use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::chat::ChatUiEvent;

const COMMAND_OUTPUT_LIMIT: usize = 16 * 1024;

#[derive(Debug)]
pub enum CommandBridgeRequest {
    Run {
        session_id: usize,
        command: String,
        cwd: PathBuf,
        timeout: Duration,
    },
}

fn send_chat_ui(tx: &std::sync::mpsc::SyncSender<TuiEvent>, event: ChatUiEvent) {
    if let Err(e) = tx.send(TuiEvent::Chat(event)) {
        log::error!("command bridge: UI channel closed: {e}");
    }
}

pub fn spawn_command_bridge_loop(
    project: Entity<Project>,
    async_app: gpui::AsyncApp,
    mut request_rx: futures::channel::mpsc::UnboundedReceiver<CommandBridgeRequest>,
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
) -> gpui::Task<()> {
    async_app.spawn(async move |async_cx| {
        while let Some(request) = request_rx.next().await {
            let CommandBridgeRequest::Run {
                session_id,
                command,
                cwd,
                timeout,
            } = request;

            if let Some(reason) = terminal_command_guardrail_rejection(&command) {
                let msg = format!(
                    "Command rejected by security guardrails ({reason}). Ask the user for explicit permission or try a different approach."
                );
                send_chat_ui(&event_tx, ChatUiEvent::CommandOutput(session_id, msg.clone()));
                send_chat_ui(&event_tx, ChatUiEvent::CommandFinished(session_id, msg));
                continue;
            }

            let permission_result: Result<(), String> = async_cx.update(|cx| {
                let settings = AgentSettings::get_global(cx);
                match decide_permission_from_settings(
                    TerminalTool::NAME,
                    std::slice::from_ref(&command),
                    settings,
                ) {
                    ToolPermissionDecision::Allow => Ok(()),
                    ToolPermissionDecision::Deny(reason) => Err(reason),
                    ToolPermissionDecision::Confirm => Err(
                        "This command requires approval. Add an allow rule in Quorp agent settings, or confirm it from the GUI agent."
                            .to_string(),
                    ),
                }
            });

            if let Err(reason) = permission_result {
                let msg = format!("Command denied by tool permissions: {reason}");
                send_chat_ui(&event_tx, ChatUiEvent::CommandOutput(session_id, msg.clone()));
                send_chat_ui(&event_tx, ChatUiEvent::CommandFinished(session_id, msg));
                continue;
            }

            let spawn_task = async_cx.update(|cx| {
                project.update(cx, |project, cx| {
                    let is_windows = project.path_style(cx).is_windows();
                    let shell_str = project
                        .remote_client()
                        .and_then(|r| r.read(cx).default_system_shell())
                        .unwrap_or_else(get_default_system_shell_preferring_bash);
                    let (task_command, task_args) =
                        ShellBuilder::new(&Shell::Program(shell_str), is_windows)
                            .redirect_stdin_to_dev_null()
                            .build(Some(command.clone()), &[]);

                    let mut env = HashMap::default();
                    env.insert("PAGER".into(), String::new());
                    env.insert("GIT_PAGER".into(), "cat".into());

                    let spawn = SpawnInTerminal {
                        id: TaskId(format!("tui-run-{}", Uuid::new_v4())),
                        full_label: command.clone(),
                        label: command.clone(),
                        command: Some(task_command),
                        args: task_args,
                        command_label: command.clone(),
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

            let terminal: Entity<Terminal> = match spawn_task.await {
                Ok(entity) => entity,
                Err(e) => {
                    let msg = format!("Failed to spawn command: {e:#}");
                    send_chat_ui(&event_tx, ChatUiEvent::CommandOutput(session_id, msg.clone()));
                    send_chat_ui(&event_tx, ChatUiEvent::CommandFinished(session_id, msg));
                    continue;
                }
            };

            let wait_task = async_cx.update(|cx| terminal.read(cx).wait_for_completed_task(cx));

            let (exit_status, timed_out): (Option<ExitStatus>, bool) = if timeout.is_zero() {
                (wait_task.await, false)
            } else {
                let sleep = async_cx.background_executor().timer(timeout);
                futures::pin_mut!(wait_task);
                futures::pin_mut!(sleep);
                match futures::future::select(wait_task, sleep).await {
                    Either::Left((status, _)) => (status, false),
                    Either::Right((_, pending_wait)) => {
                        let _ = async_cx.update(|cx| {
                            terminal.update(cx, |t: &mut Terminal, _cx| {
                                t.kill_active_task();
                            });
                        });
                        (pending_wait.await, true)
                    }
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
                final_out.push_str(&format!(
                    "\n[Command timed out after {}s]",
                    timeout.as_secs()
                ));
            } else if let Some(status) = exit_status {
                final_out.push_str(&format!("\n[Exit code: {:?}]", status.code()));
            }

            send_chat_ui(&event_tx, ChatUiEvent::CommandOutput(session_id, final_out.clone()));
            send_chat_ui(&event_tx, ChatUiEvent::CommandFinished(session_id, final_out));
        }
    })
}

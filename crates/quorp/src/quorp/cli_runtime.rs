use super::*;

pub(crate) fn run_inline_cli(launch: SessionLaunchConfig) -> anyhow::Result<()> {
    use std::io::{self, IsTerminal as _, Write};

    apply_session_env_overrides(&launch);
    let workspace_root = launch.workspace_root.clone();
    let model = launch.model.as_deref().unwrap_or("default remote model");
    let loaded = load_workspace_settings(&workspace_root)?;
    let mut run_mode = quorp_core::RunMode::Act;
    let mut permission_mode = loaded.settings.permissions.mode;
    let mut sandbox = loaded.settings.sandbox.mode;

    let color = quorp_render::RenderProfile::detect_from_env().color;
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let use_fullscreen =
        interactive && matches!(launch.tui_mode, CliTuiMode::Auto | CliTuiMode::Fullscreen);
    if use_fullscreen {
        return run_fullscreen_cli(launch, run_mode, permission_mode, sandbox);
    }
    if interactive {
        print_inline_startup_splash(&workspace_root, model, permission_mode, sandbox, color)?;
    }
    println!(
        "{}",
        quorp_render::render_session_frame(
            &quorp_render::SessionFrame {
                title: "ad hoc agent ready".to_string(),
                subtitle: format!("{model} · {}", workspace_root.display()),
                tasks: vec![
                    quorp_render::TaskRow {
                        label: "Ad hoc mode: type a task or use /plan, /act, /full-auto, /sandbox tmp-copy"
                            .to_string(),
                        state: quorp_render::TaskState::Active,
                    },
                    quorp_render::TaskRow {
                        label: format!(
                            "mode={run_mode:?} permissions={permission_mode:?} sandbox={sandbox:?}"
                        ),
                        state: quorp_render::TaskState::Done,
                    },
                ],
                commands: Vec::new(),
                footer: "NVIDIA/OpenAI-compatible provider · scrollback-native renderer"
                    .to_string(),
            },
            86,
            color,
        )
    );

    if let Some(prompt) = launch
        .initial_prompt
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        return run_inline_task(
            &workspace_root,
            launch.clone(),
            prompt.to_string(),
            run_mode,
            permission_mode,
            sandbox,
        );
    }

    if interactive {
        let mut composer =
            crate::quorp::inline_composer::TerminalComposer::new(quorp_slash::Registry::new());
        loop {
            let prompt = inline_prompt(color);
            let Some(input) = composer.read_line(&prompt, color)? else {
                break;
            };
            let input = input.trim();
            if input.is_empty() {
                continue;
            }
            if !handle_inline_input(
                input,
                &workspace_root,
                &launch,
                &mut run_mode,
                &mut permission_mode,
                &mut sandbox,
            )? {
                break;
            }
        }
        return Ok(());
    }

    let stdin = io::stdin();
    let mut line = String::new();
    loop {
        print!("{}", inline_prompt(color));
        io::stdout().flush()?;
        line.clear();
        if stdin.read_line(&mut line)? == 0 {
            break;
        }
        let input = line.trim();
        if !handle_inline_input(
            input,
            &workspace_root,
            &launch,
            &mut run_mode,
            &mut permission_mode,
            &mut sandbox,
        )? {
            break;
        }
    }
    Ok(())
}

fn print_inline_startup_splash(
    workspace_root: &Path,
    model: &str,
    permission_mode: quorp_core::PermissionMode,
    sandbox: quorp_core::SandboxMode,
    color: quorp_render::ColorCapability,
) -> anyhow::Result<()> {
    use quorp_render::splash::{SplashStatus, SplashStep, render_splash};
    use std::io::Write as _;

    print!(
        "{}",
        crate::quorp::inline_composer::render_quorp_loader("quorp · terminal runtime", color)
    );
    let steps = [
        SplashStep {
            name: "workspace".into(),
            detail: workspace_root.display().to_string(),
            status: SplashStatus::Done,
        },
        SplashStep {
            name: "provider".into(),
            detail: model.to_string(),
            status: SplashStatus::Done,
        },
        SplashStep {
            name: "sandbox".into(),
            detail: format!("{sandbox:?}"),
            status: SplashStatus::Done,
        },
        SplashStep {
            name: "permissions".into(),
            detail: format!("{permission_mode:?}"),
            status: SplashStatus::Done,
        },
        SplashStep {
            name: "slash".into(),
            detail: "live command palette armed".into(),
            status: SplashStatus::Done,
        },
    ];
    print!("{}", render_splash("boot checklist", &steps, color));
    println!();
    std::io::stdout().flush()?;
    Ok(())
}

fn inline_prompt(color: quorp_render::ColorCapability) -> String {
    if matches!(color, quorp_render::ColorCapability::NoColor) {
        return "> ".to_string();
    }
    format!(
        "{}>{} ",
        quorp_render::palette::ACCENT_YELLOW.fg(),
        quorp_render::palette::RESET
    )
}

fn handle_inline_input(
    input: &str,
    workspace_root: &Path,
    launch: &SessionLaunchConfig,
    run_mode: &mut quorp_core::RunMode,
    permission_mode: &mut quorp_core::PermissionMode,
    sandbox: &mut quorp_core::SandboxMode,
) -> anyhow::Result<bool> {
    if input.is_empty() {
        return Ok(true);
    }
    if matches!(input, "/quit" | "/exit") {
        return Ok(false);
    }
    if let Some(command) = quorp_term::parse_slash_command(input) {
        match command {
            quorp_term::SlashCommand::Doctor => crate::quorp::cli_demos::run_doctor_command()?,
            quorp_term::SlashCommand::Help => print_inline_help(),
            quorp_term::SlashCommand::Unknown(name) => {
                println!(
                    "{}",
                    quorp_term::render_card(&quorp_term::TranscriptCard::ApprovalWarning {
                        title: format!("unknown slash command /{name}"),
                        detail: "try /help for supported commands".to_string(),
                    })
                );
            }
            other => {
                quorp_term::apply_mode_command(&other, run_mode, permission_mode, sandbox);
                println!(
                    "{}",
                    quorp_term::render_card(&quorp_term::TranscriptCard::ToolCall {
                        name: "mode".to_string(),
                        detail: format!(
                            "run={run_mode:?} permissions={permission_mode:?} sandbox={sandbox:?}"
                        ),
                    })
                );
            }
        }
        return Ok(true);
    }
    run_inline_task(
        workspace_root,
        launch.clone(),
        input.to_string(),
        *run_mode,
        *permission_mode,
        *sandbox,
    )?;
    Ok(true)
}

fn run_inline_task(
    workspace_root: &Path,
    launch: SessionLaunchConfig,
    task: String,
    run_mode: quorp_core::RunMode,
    permission_mode: quorp_core::PermissionMode,
    sandbox: quorp_core::SandboxMode,
) -> anyhow::Result<()> {
    let color = quorp_render::RenderProfile::detect_from_env().color;
    let (terminal_width, _) = match terminal_size() {
        Ok((width, _)) => (usize::from(width), 0usize),
        Err(_) => (86usize, 0usize),
    };
    let autonomy_profile = match run_mode {
        quorp_core::RunMode::Plan => quorp_agent_core::AutonomyProfile::Interactive,
        quorp_core::RunMode::Act => {
            if matches!(sandbox, quorp_core::SandboxMode::TmpCopy) {
                quorp_agent_core::AutonomyProfile::AutonomousSandboxed
            } else {
                quorp_agent_core::AutonomyProfile::AutonomousHost
            }
        }
    };
    let mode_label = if matches!(run_mode, quorp_core::RunMode::Plan) {
        "Plan"
    } else {
        "Act"
    };
    let color_plan_indicator = if matches!(run_mode, quorp_core::RunMode::Plan) {
        format!(
            " {}Plan mode{} ",
            quorp_render::palette::ACCENT_VIOLET.fg(),
            quorp_render::palette::RESET
        )
    } else {
        String::new()
    };
    let result_dir = crate::quorp::run_support::default_run_result_dir(workspace_root, "inline");
    let result_dir_display = result_dir.display().to_string();
    std::fs::create_dir_all(&result_dir)?;
    let objective_file = result_dir.join("objective.md");
    std::fs::write(&objective_file, &task)?;

    let model = launch
        .model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("default model")
        .to_string();

    let (event_tx, event_rx) = std::sync::mpsc::sync_channel::<crate::quorp::tui::TuiEvent>(256);
    let workspace = workspace_root.to_path_buf();
    let options = crate::quorp::agent_runner::HeadlessRunOptions {
        workspace: workspace.clone(),
        objective_file: objective_file.clone(),
        model_id: model.clone(),
        base_url_override: launch.base_url.clone(),
        max_steps: 12,
        max_seconds: Some(3600),
        max_total_tokens: None,
        result_dir: result_dir.clone(),
        autonomy_profile,
        completion_policy: quorp_agent_core::CompletionPolicy::default(),
        objective_metadata: serde_json::json!({
            "origin": "inline",
            "run_mode": format!("{run_mode:?}"),
            "permission_mode": format!("{permission_mode:?}"),
            "sandbox": format!("{sandbox:?}"),
            "task": task.clone(),
        }),
        seed_context: Vec::new(),
    };

    let mut worker = Some(thread::spawn(move || {
        crate::quorp::agent_runner::run_headless_agent_with_progress(options, Some(event_tx))
    }));

    let mut command_state = quorp_render::CommandState::Active { frame_time: 0.0 };
    let mut output_buffer = VecDeque::<String>::new();
    let mut last_status = "starting inline agent run".to_string();
    let mut last_summary = "remote provider run initialized".to_string();
    let mut task_completed = false;
    let start_time = Instant::now();
    let mut last_render = Instant::now();
    let mut final_outcome: Option<quorp_agent_core::AgentRunOutcome> = None;
    let mut command_output_exit: Option<i32> = None;

    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, Hide, Clear(ClearType::All), MoveTo(0, 0));

    while !task_completed {
        let mut had_event = false;
        loop {
            match event_rx.recv_timeout(Duration::from_millis(75)) {
                Ok(crate::quorp::tui::TuiEvent::Chat(chat_event)) => {
                    had_event = true;
                    match chat_event {
                        crate::quorp::tui::ChatUiEvent::CommandOutput(_, line) => {
                            if !line.is_empty() {
                                if output_buffer.len() >= 5 {
                                    output_buffer.pop_front();
                                }
                                output_buffer.push_back(line);
                            }
                            if output_buffer.len() >= 3 {
                                last_summary = output_buffer
                                    .iter()
                                    .rev()
                                    .take(2)
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .into_iter()
                                    .rev()
                                    .collect::<Vec<_>>()
                                    .join(" · ");
                            }
                            last_status = "streaming command output".to_string();
                        }
                        crate::quorp::tui::ChatUiEvent::Error(_, message) => {
                            if !message.is_empty() {
                                if output_buffer.len() >= 5 {
                                    output_buffer.pop_front();
                                }
                                output_buffer.push_back(format!("error: {message}"));
                            }
                            last_status = "runtime error".to_string();
                        }
                        crate::quorp::tui::ChatUiEvent::CommandFinished(_, outcome) => {
                            match outcome {
                                quorp_agent_core::ActionOutcome::Success { action, .. } => {
                                    last_status = format!("completed: {:?}", action);
                                    command_state = quorp_render::CommandState::Passed {
                                        exit_code: 0,
                                        duration: format!("{:.2?}", start_time.elapsed()),
                                    };
                                    command_output_exit = Some(0);
                                }
                                quorp_agent_core::ActionOutcome::Failure { action, error } => {
                                    last_status = format!("failed: {:?} — {error}", action);
                                    command_state = quorp_render::CommandState::Failed {
                                        exit_code: 1,
                                        duration: format!("{:.2?}", start_time.elapsed()),
                                    };
                                    if output_buffer.len() >= 5 {
                                        output_buffer.pop_front();
                                    }
                                    output_buffer
                                        .push_back(format!("tool failure: {:?} · {error}", action));
                                    command_output_exit = Some(1);
                                }
                            }
                        }
                        crate::quorp::tui::ChatUiEvent::AssistantDelta(_, line) => {
                            if !line.trim().is_empty() {
                                last_status = format!("assistant: {line}");
                            }
                        }
                        crate::quorp::tui::ChatUiEvent::StreamFinished(_) => {
                            last_status = "assistant stream finished".to_string();
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        let worker_finished = worker
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished);
        if final_outcome.is_none() && worker_finished {
            let joined_worker = worker
                .take()
                .expect("worker handle should exist when finished");
            final_outcome = match joined_worker.join() {
                Ok(result) => Some(result?),
                Err(error) => {
                    let _ = execute!(stdout, Show);
                    return Err(anyhow::anyhow!("inline worker panicked: {:?}", error));
                }
            };
            while let Ok(crate::quorp::tui::TuiEvent::Chat(chat_event)) =
                event_rx.recv_timeout(Duration::from_millis(5))
            {
                match chat_event {
                    crate::quorp::tui::ChatUiEvent::CommandOutput(_, line) => {
                        if output_buffer.len() >= 5 {
                            output_buffer.pop_front();
                        }
                        output_buffer.push_back(line);
                    }
                    crate::quorp::tui::ChatUiEvent::Error(_, message) => {
                        if output_buffer.len() >= 5 {
                            output_buffer.pop_front();
                        }
                        output_buffer.push_back(format!("error: {message}"));
                    }
                    crate::quorp::tui::ChatUiEvent::CommandFinished(_, outcome) => match outcome {
                        quorp_agent_core::ActionOutcome::Success { action, .. } => {
                            command_state = quorp_render::CommandState::Passed {
                                exit_code: 0,
                                duration: format!("{:.2?}", start_time.elapsed()),
                            };
                            output_buffer.push_back(format!("completed: {:?}", action));
                            command_output_exit = Some(0);
                        }
                        quorp_agent_core::ActionOutcome::Failure { action, error } => {
                            command_state = quorp_render::CommandState::Failed {
                                exit_code: 1,
                                duration: format!("{:.2?}", start_time.elapsed()),
                            };
                            output_buffer.push_back(format!("failed: {:?}: {error}", action));
                            command_output_exit = Some(1);
                        }
                    },
                    _ => {}
                }
            }

            if !matches!(
                command_state,
                quorp_render::CommandState::Passed { .. }
                    | quorp_render::CommandState::Failed { .. }
            ) {
                command_state = quorp_render::CommandState::Failed {
                    exit_code: command_output_exit.unwrap_or(1),
                    duration: format!("{:.2?}", start_time.elapsed()),
                };
            }
            task_completed = true;
            had_event = true;
        }

        if had_event || last_render.elapsed() > Duration::from_millis(90) {
            let width = terminal_width.max(48);
            let output_summary = if output_buffer.is_empty() {
                last_summary.clone()
            } else {
                output_buffer
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            let frame = quorp_render::SessionFrame {
                title: "inline agent runtime".to_string(),
                subtitle: format!(
                    "task: {}",
                    truncate_for_frame(&task, width.saturating_sub(40))
                ),
                tasks: vec![
                    quorp_render::TaskRow {
                        label: format!("model: {model}"),
                        state: quorp_render::TaskState::Done,
                    },
                    quorp_render::TaskRow {
                        label: format!("status: {last_status}"),
                        state: if matches!(command_state, quorp_render::CommandState::Active { .. })
                        {
                            quorp_render::TaskState::Active
                        } else {
                            quorp_render::TaskState::Done
                        },
                    },
                ],
                commands: vec![quorp_render::CommandCard {
                    label: "agent run".to_string(),
                    command: format!("quorp exec --sandbox {sandbox:?}"),
                    cwd: workspace_root.display().to_string(),
                    state: match command_state {
                        quorp_render::CommandState::Active { .. } => {
                            quorp_render::CommandState::Active {
                                frame_time: start_time.elapsed().as_secs_f32(),
                            }
                        }
                        _ => command_state.clone(),
                    },
                    output_summary,
                }],
                footer: format!(
                    "model={model} · mode={mode_label}{color_plan_indicator} · cwd={}",
                    workspace_root.display()
                )
                .trim()
                .to_string(),
            };
            let rendered = quorp_render::render_session_frame(&frame, width, color);
            let _ = execute!(stdout, MoveTo(0, 0), Clear(ClearType::All));
            println!("{}", rendered);
            last_render = Instant::now();
        }
    }

    let _ = execute!(stdout, Show);
    let outcome = final_outcome.context("inline worker exited without outcome")?;
    if let Some(exit_code) = command_output_exit
        && exit_code == 0
    {
        println!(
            "{}",
            quorp_term::render_card(&quorp_term::TranscriptCard::Validation {
                label: "agent run".to_string(),
                status: quorp_term::ValidationStatus::Passed,
                frame: 0,
            })
        );
        println!(
            "{}",
            quorp_term::render_card(&quorp_term::TranscriptCard::ProofReceipt {
                path: format!("{result_dir_display}/metadata.json"),
                summary: format!(
                    "stop_reason={:?} · billed_tokens={} · runtime_ms={}",
                    outcome.stop_reason, outcome.total_billed_tokens, outcome.duration_ms
                ),
            })
        );
        return Ok(());
    }

    println!(
        "{}",
        quorp_term::render_card(&quorp_term::TranscriptCard::Validation {
            label: "agent run".to_string(),
            status: quorp_term::ValidationStatus::Failed,
            frame: 0,
        })
    );
    println!(
        "{}",
        quorp_term::render_card(&quorp_term::TranscriptCard::ProofReceipt {
            path: format!("{result_dir_display}/summary.json"),
            summary: format!(
                "stop_reason={:?} · error_message={:?} · total_billed_tokens={}",
                outcome.stop_reason, outcome.error_message, outcome.total_billed_tokens
            ),
        })
    );
    Err(anyhow::anyhow!(
        "inline run ended with non-success status: {:?}",
        outcome.stop_reason
    ))
}

fn truncate_for_frame(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        value.to_string()
    } else {
        let mut truncated = String::new();
        for ch in value.chars().take(max_len.saturating_sub(1)) {
            truncated.push(ch);
        }
        truncated.push('…');
        truncated
    }
}

fn run_fullscreen_cli(
    launch: SessionLaunchConfig,
    run_mode: quorp_core::RunMode,
    permission_mode: quorp_core::PermissionMode,
    sandbox: quorp_core::SandboxMode,
) -> anyhow::Result<()> {
    use std::io::{self, Write as _};

    let workspace_root = launch.workspace_root.clone();
    let _loaded = load_workspace_settings(&workspace_root)?;
    let profile = quorp_render::RenderProfile::detect_from_env();
    let mut shell = FullscreenShell::new(
        launch,
        workspace_root,
        profile,
        run_mode,
        permission_mode,
        sandbox,
    );

    let _terminal = FullscreenTerminalGuard::enter()?;

    if let Some(initial_prompt) = shell.launch.initial_prompt.clone()
        && !initial_prompt.trim().is_empty()
    {
        shell.start_prompt(initial_prompt)?;
    }

    loop {
        shell.drain_agent_events()?;
        shell.reap_finished_worker()?;

        let (width, height) = terminal_size().unwrap_or((120, 40));
        let render_output = shell.render(usize::from(width), usize::from(height));

        let mut stdout = io::stdout();
        execute!(stdout, MoveTo(0, 0), Clear(ClearType::All))?;
        for (row, line) in render_output.lines.iter().enumerate() {
            execute!(stdout, MoveTo(0, row as u16), Clear(ClearType::CurrentLine))?;
            write!(stdout, "{line}")?;
        }
        execute!(
            stdout,
            MoveTo(render_output.cursor_col, render_output.cursor_row)
        )?;
        stdout.flush()?;

        if shell.exit_requested {
            break;
        }

        if event::poll(Duration::from_millis(40))? {
            match event::read()? {
                Event::Key(key) => shell.handle_key(key)?,
                Event::Resize(_, _) => {}
                Event::Mouse(mouse_event) => shell.handle_mouse(mouse_event)?,
                _ => {}
            }
        }
    }

    Ok(())
}

struct FullscreenTerminalGuard;

impl FullscreenTerminalGuard {
    fn enter() -> anyhow::Result<Self> {
        let mut stdout = std::io::stdout();
        enable_raw_mode().context("failed to enable terminal raw mode")?;
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            Hide,
            Clear(ClearType::All),
            MoveTo(0, 0)
        )
        .context("failed to enter fullscreen terminal mode")?;
        Ok(Self)
    }
}

impl Drop for FullscreenTerminalGuard {
    fn drop(&mut self) {
        let mut stdout = std::io::stdout();
        let _ = execute!(
            stdout,
            Show,
            DisableMouseCapture,
            LeaveAlternateScreen,
            MoveTo(0, 0),
            Clear(ClearType::All)
        );
        if let Err(error) = crossterm::terminal::disable_raw_mode() {
            eprintln!("quorp: failed to restore terminal mode: {error}");
        }
    }
}

struct FullscreenShell {
    launch: SessionLaunchConfig,
    workspace_root: PathBuf,
    profile: quorp_render::RenderProfile,
    run_mode: quorp_core::RunMode,
    permission_mode: quorp_core::PermissionMode,
    sandbox: quorp_core::SandboxMode,
    registry: quorp_slash::Registry,
    composer: crate::quorp::inline_composer::ComposerState,
    transcript: VecDeque<quorp_render::TranscriptItem>,
    active_command_index: Option<usize>,
    running_worker: Option<RunningPromptSession>,
    queued_prompt: Option<String>,
    prompt_history: Vec<String>,
    history_cursor: Option<usize>,
    scroll_offset: usize,
    exit_requested: bool,
    model: String,
    provider_label: String,
    status_line: String,
    boot_started: Instant,
}

struct RunningPromptSession {
    worker: thread::JoinHandle<anyhow::Result<quorp_agent_core::AgentRunOutcome>>,
    event_rx: std::sync::mpsc::Receiver<crate::quorp::tui::TuiEvent>,
    start_time: Instant,
    command_output: VecDeque<String>,
}

struct ShellRenderOutput {
    lines: Vec<String>,
    cursor_row: u16,
    cursor_col: u16,
}

impl FullscreenShell {
    fn new(
        launch: SessionLaunchConfig,
        workspace_root: PathBuf,
        profile: quorp_render::RenderProfile,
        run_mode: quorp_core::RunMode,
        permission_mode: quorp_core::PermissionMode,
        sandbox: quorp_core::SandboxMode,
    ) -> Self {
        let model = launch
            .model
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("default remote model")
            .to_string();
        let provider_label = launch
            .provider
            .map(|provider| provider.label().to_string())
            .unwrap_or_else(|| "provider".to_string());
        let mut transcript = VecDeque::new();
        transcript.push_back(quorp_render::TranscriptItem::System {
            text: "type a task, or press / for commands".to_string(),
        });

        Self {
            launch,
            workspace_root,
            profile,
            run_mode,
            permission_mode,
            sandbox,
            registry: quorp_slash::Registry::new(),
            composer: crate::quorp::inline_composer::ComposerState::default(),
            transcript,
            active_command_index: None,
            running_worker: None,
            queued_prompt: None,
            prompt_history: Vec::new(),
            history_cursor: None,
            scroll_offset: 0,
            exit_requested: false,
            model,
            provider_label,
            status_line: "idle".to_string(),
            boot_started: Instant::now(),
        }
    }

    fn start_prompt(&mut self, prompt: String) -> anyhow::Result<()> {
        if prompt.trim().is_empty() {
            return Ok(());
        }
        if self.running_worker.is_some() {
            self.queued_prompt = Some(prompt.clone());
            self.status_line = "queued follow-up prompt".to_string();
            self.push_transcript(quorp_render::TranscriptItem::System {
                text: format!("queued follow-up: {}", truncate_for_frame(&prompt, 96)),
            });
            return Ok(());
        }
        self.prompt_history.push(prompt.clone());
        self.history_cursor = None;
        self.start_prompt_session(prompt)
    }

    fn start_prompt_session(&mut self, prompt: String) -> anyhow::Result<()> {
        let result_dir =
            crate::quorp::run_support::default_run_result_dir(&self.workspace_root, "fullscreen");
        std::fs::create_dir_all(&result_dir)?;
        let objective_file = result_dir.join("objective.md");
        std::fs::write(&objective_file, &prompt)?;
        let autonomy_profile = match self.run_mode {
            quorp_core::RunMode::Plan => quorp_agent_core::AutonomyProfile::Interactive,
            quorp_core::RunMode::Act => {
                if matches!(self.sandbox, quorp_core::SandboxMode::TmpCopy) {
                    quorp_agent_core::AutonomyProfile::AutonomousSandboxed
                } else {
                    quorp_agent_core::AutonomyProfile::AutonomousHost
                }
            }
        };
        let objective_metadata = serde_json::json!({
            "origin": "fullscreen",
            "run_mode": format!("{:?}", self.run_mode),
            "permission_mode": format!("{:?}", self.permission_mode),
            "sandbox": format!("{:?}", self.sandbox),
            "task": prompt.clone(),
        });
        let options = crate::quorp::agent_runner::HeadlessRunOptions {
            workspace: self.workspace_root.clone(),
            objective_file,
            model_id: self.model.clone(),
            base_url_override: self.launch.base_url.clone(),
            max_steps: 12,
            max_seconds: Some(3600),
            max_total_tokens: None,
            result_dir: result_dir.clone(),
            autonomy_profile,
            completion_policy: quorp_agent_core::CompletionPolicy::default(),
            objective_metadata,
            seed_context: Vec::new(),
        };
        let (event_tx, event_rx) =
            std::sync::mpsc::sync_channel::<crate::quorp::tui::TuiEvent>(256);
        let worker = thread::spawn(move || {
            crate::quorp::agent_runner::run_headless_agent_with_progress(options, Some(event_tx))
        });

        self.running_worker = Some(RunningPromptSession {
            worker,
            event_rx,
            start_time: Instant::now(),
            command_output: VecDeque::new(),
        });
        self.push_transcript(quorp_render::TranscriptItem::Thinking {
            label: "thinking".to_string(),
        });
        self.active_command_index = None;
        self.status_line = "running".to_string();
        Ok(())
    }

    fn submit_buffer(&mut self, input: String) -> anyhow::Result<()> {
        let input = input.trim().to_string();
        if input.is_empty() {
            return Ok(());
        }
        if let Some(command) = quorp_term::parse_slash_command(&input) {
            self.handle_slash_command(command)?;
            return Ok(());
        }
        self.push_transcript(quorp_render::TranscriptItem::User {
            text: input.clone(),
        });
        self.start_prompt(input)
    }

    fn handle_slash_command(&mut self, command: quorp_term::SlashCommand) -> anyhow::Result<()> {
        match command {
            quorp_term::SlashCommand::Plan
            | quorp_term::SlashCommand::Act
            | quorp_term::SlashCommand::Auto
            | quorp_term::SlashCommand::Manual
            | quorp_term::SlashCommand::FullAuto
            | quorp_term::SlashCommand::FullPermissions
            | quorp_term::SlashCommand::Permissions(_)
            | quorp_term::SlashCommand::Sandbox(_) => {
                quorp_term::apply_mode_command(
                    &command,
                    &mut self.run_mode,
                    &mut self.permission_mode,
                    &mut self.sandbox,
                );
                self.status_line = format!(
                    "mode updated · run={:?} permissions={:?} sandbox={:?}",
                    self.run_mode, self.permission_mode, self.sandbox
                );
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: self.status_line.clone(),
                });
            }
            quorp_term::SlashCommand::Clear => {
                self.transcript.clear();
                self.active_command_index = None;
                self.scroll_offset = 0;
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: "cleared transcript".to_string(),
                });
            }
            quorp_term::SlashCommand::Status => {
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: format!(
                        "status · model={} · cwd={} · run={:?} · permissions={:?} · sandbox={:?}",
                        self.model,
                        self.workspace_root.display(),
                        self.run_mode,
                        self.permission_mode,
                        self.sandbox
                    ),
                });
            }
            quorp_term::SlashCommand::Model(Some(model)) => {
                self.model = model;
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: format!("model switched to {}", self.model),
                });
            }
            quorp_term::SlashCommand::Model(None) => {
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: format!("model {}", self.model),
                });
            }
            quorp_term::SlashCommand::Provider(Some(provider)) => {
                self.provider_label = provider;
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: format!("provider switched to {}", self.provider_label),
                });
            }
            quorp_term::SlashCommand::Provider(None) => {
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: format!("provider {}", self.provider_label),
                });
            }
            quorp_term::SlashCommand::Help | quorp_term::SlashCommand::Unknown(_) => {
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: "try /plan, /act, /full-auto, /permissions, /sandbox, /status, /clear"
                        .to_string(),
                });
            }
            quorp_term::SlashCommand::Doctor => {
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: "run `quorp doctor` from a regular shell for full diagnostics"
                        .to_string(),
                });
            }
            quorp_term::SlashCommand::Tasks
            | quorp_term::SlashCommand::Checkpoint
            | quorp_term::SlashCommand::Rollback
            | quorp_term::SlashCommand::Theme
            | quorp_term::SlashCommand::Memory
            | quorp_term::SlashCommand::Rules
            | quorp_term::SlashCommand::Session(_)
            | quorp_term::SlashCommand::Init
            | quorp_term::SlashCommand::Edit(_)
            | quorp_term::SlashCommand::Undo
            | quorp_term::SlashCommand::Redo
            | quorp_term::SlashCommand::Files
            | quorp_term::SlashCommand::Hooks
            | quorp_term::SlashCommand::Mcp
            | quorp_term::SlashCommand::Diff
            | quorp_term::SlashCommand::Apply
            | quorp_term::SlashCommand::Revert
            | quorp_term::SlashCommand::Test
            | quorp_term::SlashCommand::Verify
            | quorp_term::SlashCommand::Save
            | quorp_term::SlashCommand::Load(_)
            | quorp_term::SlashCommand::Think
            | quorp_term::SlashCommand::Compact => {
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: "that command is not available in the fullscreen shell yet".to_string(),
                });
            }
        }
        self.history_cursor = None;
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> anyhow::Result<()> {
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.exit_requested = true;
                return Ok(());
            }
            (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                self.transcript.clear();
                self.active_command_index = None;
                self.scroll_offset = 0;
                self.push_transcript(quorp_render::TranscriptItem::System {
                    text: "cleared screen".to_string(),
                });
                return Ok(());
            }
            (KeyCode::PageUp, _) => {
                self.scroll_offset = self.scroll_offset.saturating_add(4);
                return Ok(());
            }
            (KeyCode::PageDown, _) => {
                self.scroll_offset = self.scroll_offset.saturating_sub(4);
                return Ok(());
            }
            (KeyCode::Home, _) => {
                self.scroll_offset = usize::MAX / 4;
                return Ok(());
            }
            (KeyCode::End, _) => {
                self.scroll_offset = 0;
                return Ok(());
            }
            (KeyCode::Up, _) | (KeyCode::Down, _) => {
                let command_palette_visible =
                    self.composer.suggestions_visible() && self.composer.buffer().starts_with('/');
                if !command_palette_visible && self.handle_history_navigation(key.code) {
                    return Ok(());
                }
            }
            _ => {}
        }

        match self.composer.handle_key(key, &self.registry) {
            crate::quorp::inline_composer::ComposerAction::Continue => {}
            crate::quorp::inline_composer::ComposerAction::Cancel => {
                if self.running_worker.is_some() {
                    self.exit_requested = true;
                }
            }
            crate::quorp::inline_composer::ComposerAction::Submit(input) => {
                self.composer.clear();
                self.submit_buffer(input)?;
            }
        }
        Ok(())
    }

    fn handle_mouse(&mut self, mouse_event: crossterm::event::MouseEvent) -> anyhow::Result<()> {
        match mouse_event.kind {
            crossterm::event::MouseEventKind::ScrollUp => {
                self.scroll_offset = self.scroll_offset.saturating_add(2);
            }
            crossterm::event::MouseEventKind::ScrollDown => {
                self.scroll_offset = self.scroll_offset.saturating_sub(2);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_history_navigation(&mut self, key_code: KeyCode) -> bool {
        let current_buffer_is_empty = self.composer.buffer().trim().is_empty();
        if self.prompt_history.is_empty() {
            return false;
        }
        match key_code {
            KeyCode::Up => {
                let next_index = match self.history_cursor {
                    Some(index) if index > 0 => index - 1,
                    Some(_) | None => self.prompt_history.len().saturating_sub(1),
                };
                self.history_cursor = Some(next_index);
                if let Some(value) = self.prompt_history.get(next_index) {
                    let mut composer =
                        crate::quorp::inline_composer::ComposerState::with_buffer(value);
                    composer.set_suggestions_visible(true);
                    self.composer = composer;
                }
                true
            }
            KeyCode::Down => {
                let Some(index) = self.history_cursor else {
                    return false;
                };
                if index + 1 >= self.prompt_history.len() {
                    self.history_cursor = None;
                    self.composer.clear();
                } else if let Some(value) = self.prompt_history.get(index + 1) {
                    self.history_cursor = Some(index + 1);
                    self.composer =
                        crate::quorp::inline_composer::ComposerState::with_buffer(value);
                }
                true
            }
            _ => current_buffer_is_empty,
        }
    }

    fn drain_agent_events(&mut self) -> anyhow::Result<()> {
        loop {
            let event = {
                let Some(running_worker) = self.running_worker.as_mut() else {
                    return Ok(());
                };
                running_worker.event_rx.try_recv()
            };
            match event {
                Ok(crate::quorp::tui::TuiEvent::Chat(chat_event)) => match chat_event {
                    crate::quorp::tui::ChatUiEvent::CommandOutput(_, line) => {
                        if !line.trim().is_empty() {
                            let summary = {
                                let Some(running_worker) = self.running_worker.as_mut() else {
                                    return Ok(());
                                };
                                running_worker.command_output.push_back(line.clone());
                                if running_worker.command_output.len() > 4 {
                                    running_worker.command_output.pop_front();
                                }
                                running_worker
                                    .command_output
                                    .iter()
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join(" · ")
                            };
                            self.update_command_tail("tool output".to_string(), summary);
                        }
                        self.status_line = "streaming command output".to_string();
                    }
                    crate::quorp::tui::ChatUiEvent::Error(_, message) => {
                        if !message.trim().is_empty() {
                            self.push_transcript(quorp_render::TranscriptItem::Error {
                                title: "runtime error".to_string(),
                                detail: truncate_for_frame(&message, 160),
                            });
                        }
                        self.status_line = "runtime error".to_string();
                    }
                    crate::quorp::tui::ChatUiEvent::CommandFinished(_, outcome) => match outcome {
                        quorp_agent_core::ActionOutcome::Success { action, .. } => {
                            self.status_line = format!("completed {:?}", action);
                            let summary = self
                                .running_worker
                                .as_ref()
                                .map(|running_worker| {
                                    running_worker
                                        .command_output
                                        .iter()
                                        .cloned()
                                        .chain(std::iter::once("success".to_string()))
                                        .collect::<Vec<_>>()
                                        .join(" · ")
                                })
                                .unwrap_or_else(|| "success".to_string());
                            self.finish_active_command(quorp_render::ToolStatus::Passed, summary);
                        }
                        quorp_agent_core::ActionOutcome::Failure { action, error } => {
                            self.status_line = format!("failed {:?} · {}", action, error);
                            self.finish_active_command(
                                quorp_render::ToolStatus::Failed,
                                format!("{action:?} · {error}"),
                            );
                        }
                    },
                    crate::quorp::tui::ChatUiEvent::AssistantDelta(_, line) => {
                        if !line.trim().is_empty() {
                            self.append_assistant_delta(&line);
                            self.status_line =
                                format!("assistant: {}", truncate_for_frame(&line, 96));
                        }
                    }
                    crate::quorp::tui::ChatUiEvent::StreamFinished(_) => {
                        self.status_line = "assistant stream finished".to_string();
                    }
                },
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        Ok(())
    }

    fn reap_finished_worker(&mut self) -> anyhow::Result<()> {
        let Some(running_worker) = self.running_worker.as_ref() else {
            return Ok(());
        };
        if !running_worker.worker.is_finished() {
            return Ok(());
        }
        let running_worker = self
            .running_worker
            .take()
            .expect("worker should exist when finished");
        let outcome = match running_worker.worker.join() {
            Ok(result) => result?,
            Err(error) => {
                self.push_transcript(quorp_render::TranscriptItem::Error {
                    title: "worker panicked".to_string(),
                    detail: format!("{error:?}"),
                });
                self.status_line = "worker panicked".to_string();
                return Ok(());
            }
        };
        self.status_line = format!(
            "run finished · {:?} · {} tokens",
            outcome.stop_reason, outcome.total_billed_tokens
        );
        self.push_transcript(quorp_render::TranscriptItem::Receipt {
            text: format!(
                "run finished · {:?} · {} tokens",
                outcome.stop_reason, outcome.total_billed_tokens
            ),
            success: outcome.stop_reason == quorp_agent_core::StopReason::Success,
        });
        if let Some(queued_prompt) = self.queued_prompt.take()
            && !queued_prompt.trim().is_empty()
        {
            self.start_prompt(queued_prompt)?;
        }
        Ok(())
    }

    fn render(&mut self, width: usize, height: usize) -> ShellRenderOutput {
        let buffer = self.composer.buffer().to_string();
        let overlay =
            if self.composer.suggestions_visible() && self.composer.buffer().starts_with('/') {
                Some(quorp_render::ShellOverlay::SlashPalette {
                    selected: self.composer.selected(),
                    entries: self
                        .composer
                        .suggestions(&self.registry)
                        .into_iter()
                        .map(|entry| quorp_render::shell::PaletteRow {
                            value: entry.value,
                            detail: entry.detail,
                            description: entry.description,
                        })
                        .collect(),
                })
            } else {
                None
            };
        let live_turn = self
            .running_worker
            .as_ref()
            .map(|running_worker| quorp_render::LiveTurn {
                label: "working".to_string(),
                elapsed_ms: running_worker.start_time.elapsed().as_millis() as u64,
            });
        let status = quorp_render::StatusLine {
            left: format!("{} · {}", self.model, self.provider_label),
            center: format!("{:?} · {:?}", self.permission_mode, self.sandbox),
            right: if matches!(self.run_mode, quorp_core::RunMode::Plan) {
                "Plan mode".to_string()
            } else {
                format!("{} · {}", self.workspace_root.display(), self.status_line)
            },
        };
        let composer = quorp_render::ComposerView {
            prompt: ">".to_string(),
            buffer: buffer.clone(),
            blink_on: self.boot_started.elapsed().as_millis() % 1000 < 500,
        };
        let frame = quorp_render::ShellFrame {
            transcript: self.transcript.iter().cloned().collect(),
            live_turn,
            composer,
            status,
            overlay,
        };
        let mut body = Vec::new();
        let show_boot = self.boot_started.elapsed() < Duration::from_millis(1200)
            || (self.transcript.len() <= 1 && self.running_worker.is_none());
        if show_boot {
            body.extend(
                quorp_render::logo::render_boot_card(
                    &self.workspace_root.display().to_string(),
                    &self.model,
                    &format!("{:?}", self.sandbox),
                    self.profile,
                )
                .lines()
                .map(|line| line.to_string()),
            );
            body.push(String::new());
        }
        body.extend(quorp_render::render_shell_frame(
            &frame,
            width,
            self.profile.color,
        ));
        let suggestions =
            quorp_render::shell::render_shell_overlay(&frame.overlay, width, self.profile.color);
        let footer =
            quorp_render::shell::render_status_line(&frame.status, width, self.profile.color);
        let footer_height = 1usize;
        let prompt_height = 1usize;
        let suggestion_height = suggestions.len().min(8).min(height.saturating_sub(2));
        let body_height = height.saturating_sub(footer_height + prompt_height + suggestion_height);
        let visible_body = if self.scroll_offset == 0 || body.len() <= body_height {
            body.split_off(body.len().saturating_sub(body_height))
        } else {
            let max_scroll = body.len().saturating_sub(body_height);
            let scroll_offset = self.scroll_offset.min(max_scroll);
            let end = body.len().saturating_sub(scroll_offset);
            let start = end.saturating_sub(body_height);
            body[start..end].to_vec()
        };

        let mut lines = Vec::new();
        if visible_body.len() < body_height {
            lines.extend(visible_body);
            while lines.len() < body_height {
                lines.push(String::new());
            }
        } else {
            lines.extend(visible_body);
        }
        lines.extend(suggestions);
        let cursor_row = lines.len() as u16;
        let cursor_col = (2usize + buffer_width_to_cursor(&buffer, self.composer.cursor()))
            .min(u16::MAX as usize) as u16;
        lines.push(quorp_render::shell::render_composer(
            &frame.composer,
            self.profile.color,
        ));
        lines.push(footer);

        ShellRenderOutput {
            lines,
            cursor_row,
            cursor_col,
        }
    }

    fn push_transcript(&mut self, item: quorp_render::TranscriptItem) {
        if self.transcript.len() >= 300 {
            self.transcript.pop_front();
        }
        self.transcript.push_back(item);
        if self.scroll_offset == 0 {
            self.scroll_offset = 0;
        }
    }

    fn append_assistant_delta(&mut self, delta: &str) {
        if let Some(quorp_render::TranscriptItem::Assistant { text, streaming }) =
            self.transcript.back_mut()
        {
            if !text.ends_with('\n') && !text.is_empty() {
                text.push(' ');
            }
            text.push_str(delta.trim());
            *streaming = true;
            return;
        }
        self.push_transcript(quorp_render::TranscriptItem::Assistant {
            text: delta.trim().to_string(),
            streaming: true,
        });
    }

    fn update_command_tail(&mut self, command: String, summary: String) {
        let output_tail = summary
            .split(" · ")
            .filter(|line| !line.trim().is_empty())
            .map(|line| truncate_for_frame(line, 120))
            .collect::<Vec<_>>();
        if let Some(index) = self.active_command_index
            && let Some(quorp_render::TranscriptItem::Command {
                output_tail: current_tail,
                status,
                ..
            }) = self.transcript.get_mut(index)
        {
            *current_tail = output_tail;
            *status = quorp_render::ToolStatus::Running;
            return;
        }
        let index = self.transcript.len();
        self.push_transcript(quorp_render::TranscriptItem::Command {
            command,
            cwd: self.workspace_root.display().to_string(),
            output_tail,
            status: quorp_render::ToolStatus::Running,
        });
        self.active_command_index = Some(index);
    }

    fn finish_active_command(&mut self, status: quorp_render::ToolStatus, summary: String) {
        let output_tail = summary
            .split(" · ")
            .filter(|line| !line.trim().is_empty())
            .map(|line| truncate_for_frame(line, 120))
            .collect::<Vec<_>>();
        if let Some(index) = self.active_command_index
            && let Some(quorp_render::TranscriptItem::Command {
                output_tail: current_tail,
                status: current_status,
                ..
            }) = self.transcript.get_mut(index)
        {
            *current_tail = output_tail;
            *current_status = status;
            self.active_command_index = None;
            return;
        }
        self.push_transcript(quorp_render::TranscriptItem::Command {
            command: "tool".to_string(),
            cwd: self.workspace_root.display().to_string(),
            output_tail,
            status,
        });
    }
}

fn buffer_width_to_cursor(buffer: &str, cursor: usize) -> usize {
    buffer[..cursor]
        .chars()
        .map(|value| value.width().unwrap_or(0))
        .sum()
}

fn print_inline_help() {
    println!(
        "{}",
        quorp_term::render_card(&quorp_term::TranscriptCard::Plan {
            title: "slash commands".to_string(),
            steps: vec![
                "/plan, /act, /full-auto, /full-permissions".to_string(),
                "/permissions <mode>, /sandbox <host|tmp-copy>".to_string(),
                "/hooks, /mcp, /diff, /apply, /revert, /compact, /doctor, /help".to_string(),
                "/exit or /quit".to_string(),
            ],
        })
    );
}

pub(crate) fn apply_session_env_overrides(launch: &SessionLaunchConfig) {
    if let Some(provider) = launch.provider {
        unsafe {
            std::env::set_var("QUORP_PROVIDER", provider.label());
        }
    }
    if let Some(model) = launch.model.as_deref() {
        unsafe {
            std::env::set_var("QUORP_MODEL", model);
        }
    }
    match (launch.provider, launch.base_url.as_deref()) {
        (Some(crate::quorp::executor::InteractiveProviderKind::Nvidia), Some(base_url)) => unsafe {
            std::env::set_var("QUORP_NVIDIA_BASE_URL", base_url);
            std::env::remove_var("QUORP_LOCAL_BASE_URL");
            std::env::remove_var("QUORP_BASE_URL");
            std::env::remove_var("QUORP_CHAT_BASE_URL");
        },
        (Some(crate::quorp::executor::InteractiveProviderKind::Local), Some(base_url)) => unsafe {
            std::env::set_var("QUORP_LOCAL_BASE_URL", base_url);
            std::env::remove_var("QUORP_NVIDIA_BASE_URL");
            std::env::remove_var("QUORP_BASE_URL");
            std::env::remove_var("QUORP_CHAT_BASE_URL");
        },
        _ => unsafe {
            std::env::remove_var("QUORP_LOCAL_BASE_URL");
            std::env::remove_var("QUORP_BASE_URL");
            std::env::remove_var("QUORP_CHAT_BASE_URL");
            std::env::remove_var("QUORP_NVIDIA_BASE_URL");
        },
    }
    match launch.prompt_compaction_policy {
        Some(policy) => unsafe {
            std::env::set_var("QUORP_PROMPT_COMPACTION_POLICY", policy.as_str());
        },
        None => unsafe {
            std::env::remove_var("QUORP_PROMPT_COMPACTION_POLICY");
        },
    }
}

pub(crate) fn run_mem_analyze() -> anyhow::Result<()> {
    let path = paths::memory_log_file();
    let summary = crate::quorp::memory_fingerprint::analyze_current_memory_log()
        .with_context(|| format!("resolved memory log path: {}", path.display()))?;
    println!(
        "{}",
        crate::quorp::memory_fingerprint::format_memory_summary(path, &summary,)
    );
    Ok(())
}

pub(crate) fn run_mem_log_path() -> anyhow::Result<()> {
    println!("{}", paths::memory_log_file().display());
    Ok(())
}

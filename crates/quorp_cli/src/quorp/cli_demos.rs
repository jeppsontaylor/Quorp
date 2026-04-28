//! CLI demo / introspection subcommands. Each handler exercises one of
//! the new application crates against real inputs and renders a
//! brilliant-CLI-friendly result to stdout.
//!
//! Lifted out of `main.rs` to keep the binary's root file under the
//! 2,000-LOC hard cap; the handlers' bodies are unchanged.

use std::path::PathBuf;

pub fn run_doctor_command() -> anyhow::Result<()> {
    use quorp_render::caps::RenderProfile;
    use quorp_render::splash::{SplashStatus, SplashStep, render_splash};

    let workspace = std::env::current_dir().unwrap_or_else(|_| paths::home_dir().clone());
    let loaded = quorp_config::load_settings(&workspace)?;
    let provider = quorp_provider::OpenAiCompatibleProvider::new(loaded.settings.provider.clone());
    let provider_url = provider.chat_completions_url()?;
    let api_key_present =
        crate::quorp::provider_config::env_value(&loaded.settings.provider.api_key_env)
            .is_some_and(|value| !value.trim().is_empty());
    let project_agent_toml = workspace.join(".quorp").join("agent.toml");
    let color = RenderProfile::detect_from_env().color;

    let mut steps: Vec<SplashStep> = Vec::new();
    steps.push(SplashStep {
        name: "workspace".into(),
        detail: workspace.display().to_string(),
        status: SplashStatus::Done,
    });

    let any_settings_loaded = loaded.sources.loaded_user || loaded.sources.loaded_project;
    let settings_detail = format!(
        "user={} project={}",
        loaded
            .sources
            .user_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| loaded.sources.user_path.display().to_string()),
        loaded
            .sources
            .project_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| loaded.sources.project_path.display().to_string()),
    );
    steps.push(SplashStep {
        name: "settings".into(),
        detail: if any_settings_loaded {
            settings_detail
        } else {
            format!("{settings_detail} (defaults — no settings file loaded)")
        },
        status: if any_settings_loaded {
            SplashStatus::Done
        } else {
            SplashStatus::Warn
        },
    });

    steps.push(SplashStep {
        name: "provider".into(),
        detail: format!(
            "{} model={}",
            loaded.settings.provider.name, loaded.settings.provider.model
        ),
        status: SplashStatus::Done,
    });

    steps.push(SplashStep {
        name: "endpoint".into(),
        detail: provider_url.to_string(),
        status: SplashStatus::Done,
    });

    steps.push(SplashStep {
        name: "api key".into(),
        detail: if api_key_present {
            format!("{} (present)", loaded.settings.provider.api_key_env)
        } else {
            format!("{} (missing)", loaded.settings.provider.api_key_env)
        },
        status: if api_key_present {
            SplashStatus::Done
        } else {
            SplashStatus::Warn
        },
    });

    steps.push(SplashStep {
        name: "sandbox".into(),
        detail: format!("{:?}", loaded.settings.sandbox.mode),
        status: SplashStatus::Done,
    });

    steps.push(SplashStep {
        name: "permissions".into(),
        detail: format!("{:?}", loaded.settings.permissions.mode),
        status: SplashStatus::Done,
    });

    let hooks = &loaded.settings.hooks;
    let hooks_total = hooks.before_tool.len() + hooks.after_tool.len() + hooks.stop.len();
    steps.push(SplashStep {
        name: "hooks".into(),
        detail: format!(
            "before={} after={} stop={}",
            hooks.before_tool.len(),
            hooks.after_tool.len(),
            hooks.stop.len()
        ),
        status: if hooks_total > 0 {
            SplashStatus::Done
        } else {
            SplashStatus::Warn
        },
    });

    steps.push(SplashStep {
        name: "legacy toml".into(),
        detail: if project_agent_toml.exists() {
            format!("found at {}", project_agent_toml.display())
        } else {
            "(none)".to_string()
        },
        status: if project_agent_toml.exists() {
            SplashStatus::Warn
        } else {
            SplashStatus::Done
        },
    });

    steps.push(SplashStep {
        name: "tmp-copy".into(),
        detail: "/tmp/quorp".into(),
        status: SplashStatus::Done,
    });

    print!("{}", render_splash("quorp · doctor", &steps, color));
    Ok(())
}

pub fn run_scan_command(workspace: Option<PathBuf>, harvest_symbols: bool) -> anyhow::Result<()> {
    use quorp_render::caps::RenderProfile;
    use quorp_render::splash::{SplashStatus, SplashStep, render_splash};
    use quorp_repo_scan::{Language, ScannedFile, harvest_rust_symbols, scan};

    let workspace = workspace
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| paths::home_dir().clone()));
    let workspace = std::fs::canonicalize(&workspace).unwrap_or(workspace);
    let color = RenderProfile::detect_from_env().color;

    let scan_started = std::time::Instant::now();
    let files: Vec<ScannedFile> = scan(&workspace);
    let scan_ms = scan_started.elapsed().as_millis();

    let mut counts: std::collections::BTreeMap<&str, (u64, u64)> =
        std::collections::BTreeMap::new();
    let mut total_bytes: u64 = 0;
    for file in &files {
        let label = match file.language {
            Language::Rust => "rust",
            Language::TypeScript => "typescript",
            Language::Python => "python",
            Language::Go => "go",
            Language::Toml => "toml",
            Language::Json => "json",
            Language::Markdown => "markdown",
            Language::Other => "other",
        };
        let entry = counts.entry(label).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += file.bytes;
        total_bytes += file.bytes;
    }

    let mut steps: Vec<SplashStep> = Vec::new();
    steps.push(SplashStep {
        name: "workspace".into(),
        detail: workspace.display().to_string(),
        status: SplashStatus::Done,
    });
    steps.push(SplashStep {
        name: "scanned".into(),
        detail: format!(
            "{} files · {} kB · {scan_ms} ms",
            files.len(),
            (total_bytes + 512) / 1024
        ),
        status: SplashStatus::Done,
    });
    for (label, (count, bytes)) in &counts {
        steps.push(SplashStep {
            name: (*label).to_string(),
            detail: format!("{count} files · {} kB", (bytes + 512) / 1024),
            status: SplashStatus::Done,
        });
    }

    if harvest_symbols {
        let symbols_started = std::time::Instant::now();
        let mut symbol_total = 0usize;
        for file in &files {
            if file.language != Language::Rust {
                continue;
            }
            if let Ok(contents) = std::fs::read_to_string(&file.path) {
                symbol_total += harvest_rust_symbols(file, &contents).len();
            }
        }
        let symbols_ms = symbols_started.elapsed().as_millis();
        steps.push(SplashStep {
            name: "symbols".into(),
            detail: format!("{symbol_total} top-level Rust symbols · {symbols_ms} ms"),
            status: SplashStatus::Done,
        });
    }

    print!("{}", render_splash("quorp · scan", &steps, color));
    Ok(())
}

pub fn run_commands_command(prefix: Option<String>) -> anyhow::Result<()> {
    use quorp_render::caps::RenderProfile;
    use quorp_render::palette::{ACCENT_CYAN, DIM, FG_TEXT, RESET};
    use quorp_slash::{Registry, SlashCommandSpec};

    let color = RenderProfile::detect_from_env().color;
    let plain = matches!(color, quorp_render::caps::ColorCapability::NoColor);
    let registry = Registry::new();

    let entries: Vec<&SlashCommandSpec> = if let Some(prefix) = prefix.as_deref() {
        registry
            .suggest(prefix)
            .into_iter()
            .map(|(spec, _)| spec)
            .collect()
    } else {
        registry.all().iter().collect()
    };

    if plain {
        for spec in entries {
            let aliases = if spec.aliases.is_empty() {
                String::new()
            } else {
                format!(" ({})", spec.aliases.join(", "))
            };
            println!("/{:<13} {} — {}", spec.name, aliases, spec.description);
        }
        return Ok(());
    }

    for spec in entries {
        let aliases = if spec.aliases.is_empty() {
            String::new()
        } else {
            format!(" ({})", spec.aliases.join(", "))
        };
        println!(
            "{cyan}/{:<13}{reset}{dim}{aliases}{reset} {fg}— {}{reset}",
            spec.name,
            spec.description,
            cyan = ACCENT_CYAN.fg(),
            dim = DIM,
            fg = FG_TEXT.fg(),
            reset = RESET,
            aliases = aliases,
        );
    }
    Ok(())
}

pub fn run_permissions_command(
    mode: quorp_permissions::Mode,
    tool: String,
    capability: Option<quorp_permissions::Capability>,
    command: Option<String>,
    allow_commands: Vec<String>,
) -> anyhow::Result<()> {
    use quorp_permissions::{
        AllowEntry, AllowList, AllowPolicy, Decision, Permissions, classify_tool_action,
    };
    use quorp_render::caps::RenderProfile;
    use quorp_render::permission_modal::{PermissionPrompt, render_permission_modal};
    use quorp_render::splash::{SplashStatus, SplashStep, render_splash};

    let color = RenderProfile::detect_from_env().color;

    let mut action = classify_tool_action(&tool, command.clone(), None);
    if let Some(capability) = capability {
        action.capability = capability;
    }

    let mut allow = AllowList::default();
    for pattern in &allow_commands {
        allow.commands.push(AllowEntry {
            pattern: pattern.clone(),
            policy: AllowPolicy::AlwaysSession,
        });
    }

    let permissions = Permissions::new(mode, allow);
    let decision = permissions.check(&action);

    let mode_label = format!("{:?}", mode);
    let cap_label = format!("{:?}", action.capability);
    let decision_label = match decision {
        Decision::Allow => "Allow",
        Decision::Deny => "Deny",
        Decision::PromptUser => "PromptUser",
    };

    let mut steps: Vec<SplashStep> = Vec::new();
    steps.push(SplashStep {
        name: "mode".into(),
        detail: mode_label.clone(),
        status: SplashStatus::Done,
    });
    steps.push(SplashStep {
        name: "tool".into(),
        detail: tool.clone(),
        status: SplashStatus::Done,
    });
    steps.push(SplashStep {
        name: "capability".into(),
        detail: cap_label,
        status: SplashStatus::Done,
    });
    if let Some(cmd) = command.as_deref() {
        steps.push(SplashStep {
            name: "command".into(),
            detail: cmd.to_string(),
            status: SplashStatus::Done,
        });
    }
    if !allow_commands.is_empty() {
        steps.push(SplashStep {
            name: "allowed".into(),
            detail: allow_commands.join(", "),
            status: SplashStatus::Done,
        });
    }
    steps.push(SplashStep {
        name: "decision".into(),
        detail: decision_label.to_string(),
        status: match decision {
            Decision::Allow => SplashStatus::Done,
            Decision::PromptUser | Decision::Deny => SplashStatus::Warn,
        },
    });

    print!("{}", render_splash("quorp · permissions", &steps, color));

    if matches!(decision, Decision::PromptUser) {
        let prompt = PermissionPrompt {
            tool: tool.clone(),
            command_repr: command.unwrap_or_else(|| "(no command supplied)".to_string()),
            cwd: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "(unknown)".to_string()),
            sandbox: "tmp-copy".to_string(),
            rationale: format!("approval required because mode={mode_label}"),
        };
        println!();
        print!("{}", render_permission_modal(&prompt, color));
    }
    Ok(())
}

pub fn run_render_demo() -> anyhow::Result<()> {
    use quorp_render::caps::{ColorCapability, RenderProfile};
    use quorp_render::permission_modal::{PermissionPrompt, render_permission_modal};
    use quorp_render::session::{
        CommandCard, CommandState, SessionFrame, TaskRow, TaskState, render_session_frame,
    };
    use quorp_render::shimmer::{ShimmerStyle, render_shimmer};
    use quorp_render::splash::{SplashStatus, SplashStep, render_splash};
    use quorp_render::status_footer::{StatusFooter, render_status_footer};
    use quorp_render::transcript::{TranscriptLine, render_transcript_line};
    use std::io::IsTerminal as _;

    let profile = RenderProfile::detect_from_env();
    let color = profile.color;

    let splash_steps = [
        SplashStep {
            name: "workspace".into(),
            detail: "~/Code/quorp".into(),
            status: SplashStatus::Done,
        },
        SplashStep {
            name: "settings".into(),
            detail: "user + project".into(),
            status: SplashStatus::Done,
        },
        SplashStep {
            name: "env".into(),
            detail: ".quorp/.env (4 vars)".into(),
            status: SplashStatus::Done,
        },
        SplashStep {
            name: "provider".into(),
            detail: "nvidia/qwen3-coder · 47ms".into(),
            status: SplashStatus::Done,
        },
        SplashStep {
            name: "repo capsule".into(),
            detail: "412 files, 64kb cached".into(),
            status: SplashStatus::Done,
        },
        SplashStep {
            name: "memory + rules".into(),
            detail: "3 active rules · 42 facts".into(),
            status: SplashStatus::Running,
        },
    ];
    let session_frame = SessionFrame {
        title: "brilliant terminal coding".into(),
        subtitle: "agent-first Rust runtime · truecolor stream · sandboxed tools".into(),
        tasks: vec![
            TaskRow {
                label: "Plan task with proof gates".into(),
                state: TaskState::Done,
            },
            TaskRow {
                label: "Run command with live chroma".into(),
                state: TaskState::Active,
            },
            TaskRow {
                label: "Compress proof into receipt".into(),
                state: TaskState::Pending,
            },
        ],
        commands: vec![
            CommandCard {
                label: "verify".into(),
                command: "./script/clippy".into(),
                cwd: "~/Code/quorp".into(),
                state: CommandState::Active { frame_time: 0.22 },
                output_summary: "strict lane running · raw log retained · first error pins span"
                    .into(),
            },
            CommandCard {
                label: "lib tests".into(),
                command: "cargo test --workspace --lib".into(),
                cwd: "~/Code/quorp".into(),
                state: CommandState::Passed {
                    exit_code: 0,
                    duration: "0.65s".into(),
                },
                output_summary: "421 passed across 39 suites".into(),
            },
        ],
        footer: "qwen3-coder@nvidia · --yolo sandbox · ctx 12.4k/64k · tasks 2/3".into(),
    };

    println!("{}", render_session_frame(&session_frame, 86, color));
    println!();

    print!(
        "{}",
        render_splash("quorp · boot checklist", &splash_steps, color)
    );
    println!();

    let frames = 18;
    let style = ShimmerStyle::default();
    let static_demo =
        std::env::var_os("QUORP_RENDER_DEMO_STATIC").is_some() || !std::io::stdout().is_terminal();
    if static_demo {
        println!(
            "  {} · ctx 12.4k/64k",
            render_shimmer("Cogitating", 0.0, style, color)
        );
    } else {
        print!("\x1b[?25l");
        for i in 0..frames {
            let t = i as f32 * 0.06;
            print!(
                "\r  {} · ctx 12.4k/64k",
                render_shimmer("Cogitating", t, style, color)
            );
            let _ = std::io::Write::flush(&mut std::io::stdout());
            std::thread::sleep(std::time::Duration::from_millis(55));
        }
        print!("\x1b[?25h\r\x1b[2K");
    }

    let transcript = [
        TranscriptLine::UserPrompt("refactor agent_runner.rs into smaller modules".into()),
        TranscriptLine::AssistantProse("I'll inspect the file and propose a 4-step plan.".into()),
        TranscriptLine::ToolCallSummary {
            tool: "read_file".into(),
            target: "crates/quorp/src/quorp/agent_runner.rs".into(),
            sample_chars: 31_842,
        },
        TranscriptLine::RepairAttempt {
            attempt: 1,
            cap: 3,
            hypothesis: "missing pub(super) on HeadlessEventRecorder".into(),
        },
    ];
    for line in &transcript {
        println!("{}", render_transcript_line(line, color));
    }
    println!();

    let footer = StatusFooter {
        model_provider: "qwen3-coder@nvidia".into(),
        mode_label: "Act".into(),
        phase_pill: "thinking".into(),
        usage_summary: "ctx 12.4k/64k · $0.024 · tasks 3/8 · 4.2s".into(),
    };
    println!("{}", render_status_footer(&footer, color));
    println!();

    let prompt = PermissionPrompt {
        tool: "run_command".into(),
        command_repr: "cargo test -p quorp_term".into(),
        cwd: "crates/quorp_term".into(),
        sandbox: "tmp-copy".into(),
        rationale: "validate the SlashCommand parser changes".into(),
    };
    print!("{}", render_permission_modal(&prompt, color));
    println!();

    let color_label = match color {
        ColorCapability::TrueColor => "truecolor",
        ColorCapability::Ansi256 => "ansi-256",
        ColorCapability::Ansi16 => "ansi-16",
        ColorCapability::NoColor => "no-color",
    };
    println!("(detected color profile: {color_label})");
    Ok(())
}

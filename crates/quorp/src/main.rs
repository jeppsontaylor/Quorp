// Disable command line from opening on release mode
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod reliability;
mod quorp;

use anyhow::{Context as _, Result};
use clap::Parser;
use client::{Client, UserStore};
use db::kvp::KeyValueStore;
use fs::{Fs, RealFs};
use gpui::{App, Application, AsyncApp};
use gpui_tokio::Tokio;
use language::LanguageRegistry;
use node_runtime::NodeRuntime;
use assets::Assets;
use project::{LocalProjectFlags, Project};
use release_channel::{AppCommitSha, AppVersion};
use session::{AppSession, Session};
use settings::{ProxySettings, watch_config_file};
use std::{
    env,
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use theme::ThemeRegistry;
use util::ResultExt;
use util::paths::{self, PathWithPosition};
use uuid::Uuid;
use quorp::{
    AppState, handle_keymap_file_changes, handle_settings_file_changes,
    eager_load_active_theme_and_icon_theme,
};

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let args = Args::parse();

    let tui_mode = args.tui || stdout_is_a_pty();

    zlog::init();

    if stdout_is_a_pty() && !tui_mode {
        zlog::init_output_stdout();
    } else {
        zlog::init_output_file(paths::log_file(), Some(paths::old_log_file())).ok();
    }
    ztracing::init();

    let version = option_env!("QUORP_BUILD_ID");
    let app_commit_sha =
        option_env!("QUORP_COMMIT_SHA").map(|commit_sha| AppCommitSha::new(commit_sha.to_string()));
    let app_version = AppVersion::load(env!("CARGO_PKG_VERSION"), version, app_commit_sha.clone());

    rayon::ThreadPoolBuilder::new()
        .num_threads(std::thread::available_parallelism().map_or(1, |n| n.get().div_ceil(2)))
        .stack_size(10 * 1024 * 1024)
        .thread_name(|ix| format!("RayonWorker{}", ix))
        .build_global()
        .unwrap();

    let app =
        Application::with_platform(gpui_platform::current_platform(tui_mode)).with_assets(Assets);

    let app_db = db::AppDatabase::new();
    let session_id = Uuid::new_v4().to_string();
    let session = app.background_executor().spawn(Session::new(
        session_id.clone(),
        KeyValueStore::from_app_db(&app_db),
    ));

    let fs = Arc::new(RealFs::new(None, app.background_executor()));
    let (user_settings_file_rx, user_settings_watcher) = watch_config_file(
        &app.background_executor(),
        fs.clone(),
        paths::settings_file().clone(),
    );
    let (global_settings_file_rx, global_settings_watcher) = watch_config_file(
        &app.background_executor(),
        fs.clone(),
        paths::global_settings_file().clone(),
    );
    let (user_keymap_file_rx, user_keymap_watcher) = watch_config_file(
        &app.background_executor(),
        fs.clone(),
        paths::keymap_file().clone(),
    );

    app.run(move |cx| {
        cx.set_global(app_db);
        gpui_tokio::init(cx);
        settings::init(cx);
        zlog_settings::init(cx);
        handle_settings_file_changes(
            user_settings_file_rx,
            user_settings_watcher,
            global_settings_file_rx,
            global_settings_watcher,
            cx,
        );
        handle_keymap_file_changes(user_keymap_file_rx, user_keymap_watcher, cx);

        <dyn Fs>::set_global(fs.clone(), cx);

        let client = Client::production(cx);
        let mut languages = LanguageRegistry::new(cx.background_executor().clone());
        languages.set_language_server_download_dir(paths::languages_dir().clone());
        let languages = Arc::new(languages);

        let user_store = cx.new(|cx| UserStore::new(client.clone(), cx));
        let node_runtime = NodeRuntime::new(client.http_client(), cx.background_executor().clone());
        let app_session = cx.new(|cx| AppSession::new(session, cx));

        let app_state = Arc::new(AppState {
            languages,
            client: client.clone(),
            user_store,
            fs: fs.clone(),
            node_runtime,
            session: app_session,
        });
        AppState::set_global(Arc::downgrade(&app_state), cx);

        reliability::init(client.clone(), cx);
        theme::init(theme::LoadThemes::All(Box::new(Assets)), cx);
        eager_load_active_theme_and_icon_theme(fs.clone(), cx);

        cx.spawn({
            let client = app_state.client.clone();
            async move |cx| authenticate(client, cx).await
        })
        .detach();

        // TUI Boot sequence
        let tui_workspace_root = tui_initial_workspace_root(&args);
        let tui_project = Project::local(
            app_state.client.clone(),
            app_state.node_runtime.clone(),
            app_state.user_store.clone(),
            app_state.languages.clone(),
            app_state.fs.clone(),
            None,
            LocalProjectFlags::default(),
            cx,
        );

        cx.spawn({
            let tui_project = tui_project.clone();
            let tui_workspace_root = tui_workspace_root.clone();
            let app_state = app_state.clone();
            async move |async_cx| {
                let _ = tui_project.update(&mut async_cx.clone(), |project, cx| {
                    project.find_or_create_worktree(&tui_workspace_root, true, cx)
                }).await;

                let (event_tx, event_rx) =
                    std::sync::mpsc::sync_channel::<crate::quorp::tui::TuiEvent>(
                        crate::quorp::tui::TUI_EVENT_QUEUE_CAPACITY,
                    );
                let chat_tx = event_tx.clone();
                let crossterm_tx = event_tx.clone();
                drop(event_tx);

                crate::quorp::tui::path_index_bridge::spawn_path_index_bridge_loop(
                    tui_project.clone(),
                    async_cx.clone(),
                    std::sync::Arc::new(std::sync::RwLock::new(tui_workspace_root.clone())),
                    chat_tx.clone(),
                ).detach();

                let terminal_entity_opt = async_cx.update(|cx| {
                    tui_project.update(cx, |project, cx| {
                        project.create_terminal_shell(Some(tui_workspace_root.clone()), cx)
                    })
                }).await.ok();

                let (unified_tx, unified_rx) = futures::channel::mpsc::unbounded();
                let (core_chat_models, core_chat_model_index) = async_cx.update(|cx| {
                    let registry = language_model::LanguageModelRegistry::read_global(cx);
                    let models: Vec<String> = registry.available_models(cx)
                        .map(|m| format!("{}/{}", m.provider_id().0, m.id().0)).collect();
                    let model_index = registry.default_model()
                        .and_then(|c| models.iter().position(|m| *m == format!("{}/{}", c.provider.id().0, c.model.id().0)))
                        .unwrap_or(0);
                    (models, model_index)
                });

                crate::quorp::tui::bridge::spawn_unified_bridge_loop(
                    tui_project.clone(),
                    terminal_entity_opt,
                    async_cx.clone(),
                    unified_rx,
                    chat_tx.clone(),
                    app_state.fs.clone(),
                ).detach();

                let (quit_tx, quit_rx) = futures::channel::oneshot::channel::<()>();
                std::thread::spawn(move || {
                    let _ = crate::quorp::tui::run(
                        app_state,
                        tui_workspace_root,
                        event_rx,
                        crossterm_tx,
                        chat_tx,
                        Some((unified_tx.clone(), core_chat_models, core_chat_model_index)),
                        None,
                        None,
                        Some(unified_tx),
                    );
                    let _ = quit_tx.send(());
                });

                let _ = quit_rx.await;
                async_cx.update(|cx| cx.quit());
            }
        }).detach();
    });
}

fn stdout_is_a_pty() -> bool {
    io::stdout().is_terminal()
}

fn tui_initial_workspace_root(args: &Args) -> PathBuf {
    let fallback = || std::env::current_dir().unwrap_or_else(|_| paths::home_dir().clone());
    let Some(first) = args.paths_or_urls.first() else {
        return fallback();
    };
    if first.contains("://") {
        return fallback();
    }
    let parsed = PathWithPosition::parse_str(first);
    let path = parsed.path;
    if path.as_os_str().is_empty() {
        return fallback();
    }
    match std::fs::metadata(&path) {
        Ok(m) if m.is_dir() => std::fs::canonicalize(&path).unwrap_or(path),
        Ok(m) if m.is_file() => path
            .parent()
            .map(|p| {
                if p.as_os_str().is_empty() {
                    fallback()
                } else {
                    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
                }
            })
            .unwrap_or_else(fallback),
        _ => path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()))
            .unwrap_or_else(fallback),
    }
}

async fn authenticate(client: Arc<Client>, cx: &AsyncApp) -> Result<()> {
    if stdout_is_a_pty() {
        if client::IMPERSONATE_LOGIN.is_some() {
            client.sign_in_with_optional_connect(false, cx).await?;
        } else if client.has_credentials(cx).await {
            client.sign_in_with_optional_connect(true, cx).await?;
        }
    } else if client.has_credentials(cx).await {
        client.sign_in_with_optional_connect(true, cx).await?;
    }

    Ok(())
}

#[derive(Parser, Debug)]
#[command(name = "quorp", version = env!("CARGO_PKG_VERSION"))]
struct Args {
    paths_or_urls: Vec<String>,
    #[arg(long)]
    tui: bool,
}

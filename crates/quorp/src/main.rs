// Disable command line from opening on release mode
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod quorp;

use clap::Parser;
use std::path::PathBuf;
use ::paths;
use util::paths::PathWithPosition;

fn main() {
    let args = Args::parse();

    zlog::init();
    zlog::init_output_file(paths::log_file(), Some(paths::old_log_file())).ok();
    ztracing::init();

    if let Err(error) = run_native_tui(args) {
        eprintln!("quorp: {error:#}");
        std::process::exit(1);
    }
}

fn run_native_tui(args: Args) -> anyhow::Result<()> {
    let workspace_root = initial_workspace_root(&args);
    let (event_tx, event_rx) =
        std::sync::mpsc::sync_channel::<crate::quorp::tui::TuiEvent>(
            crate::quorp::tui::TUI_EVENT_QUEUE_CAPACITY,
        );
    let chat_tx = event_tx.clone();
    let crossterm_tx = event_tx;

    crate::quorp::tui::run(
        workspace_root,
        event_rx,
        crossterm_tx,
        chat_tx,
        None,
        None,
        None,
        None,
    )
}

fn initial_workspace_root(args: &Args) -> PathBuf {
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
        Ok(metadata) if metadata.is_dir() => std::fs::canonicalize(&path).unwrap_or(path),
        Ok(metadata) if metadata.is_file() => path
            .parent()
            .map(|parent| {
                if parent.as_os_str().is_empty() {
                    fallback()
                } else {
                    std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf())
                }
            })
            .unwrap_or_else(fallback),
        _ => path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf()))
            .unwrap_or_else(fallback),
    }
}

#[derive(Parser, Debug)]
#[command(name = "quorp", version = env!("CARGO_PKG_VERSION"))]
struct Args {
    paths_or_urls: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(paths_or_urls: Vec<String>) -> Args {
        Args { paths_or_urls }
    }

    #[test]
    fn initial_workspace_root_defaults_to_current_dir_without_inputs() {
        let expected = std::env::current_dir().expect("current dir");
        assert_eq!(initial_workspace_root(&args(Vec::new())), expected);
    }

    #[test]
    fn initial_workspace_root_ignores_urls() {
        let expected = std::env::current_dir().expect("current dir");
        assert_eq!(
            initial_workspace_root(&args(vec!["https://example.com/repo".to_string()])),
            expected
        );
    }

    #[test]
    fn initial_workspace_root_uses_directory_argument() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let expected = std::fs::canonicalize(temp_dir.path()).expect("canonicalize");
        assert_eq!(
            initial_workspace_root(&args(vec![temp_dir.path().display().to_string()])),
            expected
        );
    }

    #[test]
    fn initial_workspace_root_uses_parent_for_file_argument() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let source_dir = temp_dir.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("mkdir");
        let file_path = source_dir.join("main.rs");
        std::fs::write(&file_path, "fn main() {}\n").expect("write");
        let expected = std::fs::canonicalize(&source_dir).expect("canonicalize");
        assert_eq!(
            initial_workspace_root(&args(vec![file_path.display().to_string()])),
            expected
        );
    }
}

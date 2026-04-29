//! Repo-scan adapter: walks the worktree, identifies source files,
//! populates `quorp_repo_graph`. Phase 6 ships the file walker and a
//! minimal symbol stub harvested with a regex pass — tree-sitter and
//! `notify` integration follow when the runtime starts using the graph.

use std::path::{Path, PathBuf};

use quorp_repo_graph::{FileId, GraphStats, LineRange, SymbolKind, SymbolNode, SymbolPath};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    Rust,
    TypeScript,
    Python,
    Go,
    Toml,
    Json,
    Markdown,
    Other,
}

impl Language {
    pub fn from_extension(ext: &str) -> Self {
        match ext {
            "rs" => Self::Rust,
            "ts" | "tsx" => Self::TypeScript,
            "py" => Self::Python,
            "go" => Self::Go,
            "toml" => Self::Toml,
            "json" => Self::Json,
            "md" => Self::Markdown,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub language: Language,
    pub bytes: u64,
}

/// Walk `root` (skipping target/, .git/, .quorp/) and emit one
/// `ScannedFile` per source file.
pub fn scan(root: &Path) -> Vec<ScannedFile> {
    WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !matches!(name.as_ref(), "target" | ".git" | ".quorp" | "node_modules")
        })
        .filter_map(|res| res.ok())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            let path = entry.path().to_path_buf();
            let language = path
                .extension()
                .and_then(|e| e.to_str())
                .map(Language::from_extension)
                .unwrap_or(Language::Other);
            let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            ScannedFile {
                path,
                language,
                bytes,
            }
        })
        .collect()
}

/// Crude Rust symbol harvest: regex-style scan of `fn name`, `struct Name`,
/// `enum Name`, `trait Name`. Replaced with tree-sitter in the wire-up.
pub fn harvest_rust_symbols(file: &ScannedFile, contents: &str) -> Vec<SymbolNode> {
    if file.language != Language::Rust {
        return Vec::new();
    }
    let mut symbols = Vec::new();
    for (idx, line) in contents.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed
            .strip_prefix("pub fn ")
            .or_else(|| trimmed.strip_prefix("fn "))
        {
            push_symbol(rest, SymbolKind::Function, idx as u32, file, &mut symbols);
        } else if let Some(rest) = trimmed
            .strip_prefix("pub struct ")
            .or_else(|| trimmed.strip_prefix("struct "))
        {
            push_symbol(rest, SymbolKind::Struct, idx as u32, file, &mut symbols);
        } else if let Some(rest) = trimmed
            .strip_prefix("pub enum ")
            .or_else(|| trimmed.strip_prefix("enum "))
        {
            push_symbol(rest, SymbolKind::Enum, idx as u32, file, &mut symbols);
        } else if let Some(rest) = trimmed
            .strip_prefix("pub trait ")
            .or_else(|| trimmed.strip_prefix("trait "))
        {
            push_symbol(rest, SymbolKind::Trait, idx as u32, file, &mut symbols);
        }
    }
    symbols
}

fn push_symbol(
    rest: &str,
    kind: SymbolKind,
    line: u32,
    file: &ScannedFile,
    out: &mut Vec<SymbolNode>,
) {
    let name = rest
        .split(['<', '(', ' ', '{'])
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(',')
        .to_string();
    if name.is_empty() {
        return;
    }
    out.push(SymbolNode {
        path: SymbolPath::new(name),
        kind,
        file: FileId(file.path.clone()),
        span: LineRange {
            start: line + 1,
            end: line + 1,
        },
    });
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct ScanReport {
    pub files: u32,
    pub rust_files: u32,
    pub symbols: u32,
}

impl ScanReport {
    pub fn into_graph_stats(self) -> GraphStats {
        GraphStats {
            files: self.files,
            symbols: self.symbols,
            edges: 0,
        }
    }
}
#[cfg(test)]
#[path = "../../../testing/quorp_repo_scan/quorp_repo_scan/tests.rs"]
mod tests;

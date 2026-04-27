#![allow(clippy::collapsible_if)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use regex::Regex;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

mod rust_language_server;
use rust_language_server::RustLanguageServerSession;

const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_FILES_SCANNED: usize = 50_000;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    pub fn from_path(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
        {
            "rs" => Self::Rust,
            "ts" | "tsx" | "js" | "jsx" => Self::TypeScript,
            "py" => Self::Python,
            "go" => Self::Go,
            "toml" => Self::Toml,
            "json" => Self::Json,
            "md" | "markdown" => Self::Markdown,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub path: String,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SymbolLocation {
    pub name: String,
    pub kind: String,
    pub signature: String,
    pub location: Location,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub path: String,
    pub severity: String,
    pub message: String,
    #[serde(default)]
    pub line: Option<usize>,
    #[serde(default)]
    pub column: Option<usize>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct HoverInfo {
    pub symbol: String,
    pub kind: String,
    pub signature: String,
    pub location: Location,
    pub reference_count: usize,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentSymbol {
    pub name: String,
    pub kind: String,
    pub signature: String,
    pub location: Location,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodeAction {
    pub title: String,
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RenamePreview {
    pub old_name: String,
    pub new_name: String,
    pub replacement_count: usize,
    pub locations: Vec<Location>,
}

#[derive(Debug, Clone)]
struct IndexedFile {
    path: String,
    _language: Language,
    content: String,
    symbols: Vec<SymbolLocation>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceSemanticIndex {
    root: PathBuf,
    files: Vec<IndexedFile>,
    symbol_index: BTreeMap<String, Vec<SymbolLocation>>,
    rust_language_server: Option<Arc<RustLanguageServerSession>>,
}

impl WorkspaceSemanticIndex {
    pub fn build(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::build_with_rust_language_server(root, None)
    }

    pub fn build_with_rust_language_server(
        root: impl AsRef<Path>,
        rust_language_server_command: Option<&str>,
    ) -> anyhow::Result<Self> {
        let root = root.as_ref().to_path_buf();
        let mut files = Vec::new();
        let mut symbol_index: BTreeMap<String, Vec<SymbolLocation>> = BTreeMap::new();

        for entry in WalkDir::new(&root)
            .into_iter()
            .filter_entry(|entry| {
                let name = entry.file_name().to_string_lossy();
                !matches!(name.as_ref(), "target" | ".git" | ".quorp" | "node_modules")
            })
            .filter_map(|result| result.ok())
            .filter(|entry| entry.file_type().is_file())
            .take(MAX_FILES_SCANNED)
        {
            let path = entry.path();
            let metadata = entry
                .metadata()
                .with_context(|| format!("failed to read metadata for {}", path.display()))?;
            if metadata.len() > MAX_FILE_BYTES {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(path) else {
                continue;
            };
            let language = Language::from_path(path);
            let relative_path = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            let symbols = extract_symbols(&relative_path, language, &content);
            let diagnostics = diagnostics_for_file(&relative_path, language, &content);
            for symbol in &symbols {
                symbol_index
                    .entry(symbol.name.clone())
                    .or_default()
                    .push(symbol.clone());
            }
            files.push(IndexedFile {
                path: relative_path,
                _language: language,
                content,
                symbols,
                diagnostics,
            });
        }

        let rust_language_server = match rust_language_server_command {
            Some(command) => RustLanguageServerSession::spawn(&root, command)?.map(Arc::new),
            None => None,
        };

        Ok(Self {
            root,
            files,
            symbol_index,
            rust_language_server,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn has_rust_language_server(&self) -> bool {
        self.rust_language_server.is_some()
    }

    pub fn diagnostics(&self, path: &str) -> Vec<Diagnostic> {
        let normalized_path = normalize_path(path);
        if let Some(file) = self.file_for_path(&normalized_path) {
            if file._language == Language::Rust
                && let Some(server) = &self.rust_language_server
                && let Ok(diagnostics) = server.diagnostics(&file.path, &file.content)
                && !diagnostics.is_empty()
            {
                return diagnostics;
            }
            return file.diagnostics.clone();
        }
        Vec::new()
    }

    pub fn document_symbols(&self, path: &str) -> Vec<DocumentSymbol> {
        let normalized_path = normalize_path(path);
        let Some(file) = self.file_for_path(&normalized_path) else {
            return Vec::new();
        };
        if file._language == Language::Rust
            && let Some(server) = &self.rust_language_server
            && let Ok(symbols) = server.document_symbols(&file.path, &file.content)
            && !symbols.is_empty()
        {
            return symbols;
        }
        file.symbols
            .iter()
            .map(|symbol| DocumentSymbol {
                name: symbol.name.clone(),
                kind: symbol.kind.clone(),
                signature: symbol.signature.clone(),
                location: symbol.location.clone(),
            })
            .collect()
    }

    pub fn workspace_symbols(&self, query: &str, limit: usize) -> Vec<SymbolLocation> {
        let query = query.trim().to_ascii_lowercase();
        let mut symbols = self
            .symbol_index
            .iter()
            .flat_map(|(name, entries)| {
                entries.iter().filter_map(|entry| {
                    if query.is_empty()
                        || name.to_ascii_lowercase().contains(&query)
                        || entry.location.path.to_ascii_lowercase().contains(&query)
                    {
                        Some(entry.clone())
                    } else {
                        None
                    }
                })
            })
            .collect::<Vec<_>>();
        if let Some(server) = &self.rust_language_server
            && let Ok(mut rust_symbols) = server.workspace_symbols(query.as_str(), limit.max(1))
        {
            symbols.append(&mut rust_symbols);
        }
        symbols = dedupe_symbols(symbols);
        symbols.sort_by_key(|entry| {
            (
                entry.location.path.clone(),
                entry.location.line,
                entry.location.column,
            )
        });
        symbols.truncate(limit);
        symbols
    }

    pub fn definition(&self, symbol: &str, path: Option<&str>) -> Option<SymbolLocation> {
        self.definition_at(path, symbol, None, None)
    }

    pub fn definition_at(
        &self,
        path: Option<&str>,
        symbol: &str,
        line: Option<usize>,
        character: Option<usize>,
    ) -> Option<SymbolLocation> {
        let candidate_path = path.map(normalize_path);
        if let Some(server) = &self.rust_language_server {
            let resolved_path = candidate_path.as_deref().map(str::to_string).or_else(|| {
                self.symbol_lookup_location(symbol, None)
                    .map(|(path, _, _)| path)
            });
            if let Some(lookup_path) = resolved_path
                && let Some(file) = self.file_for_path(&lookup_path)
                && file._language == Language::Rust
            {
                let lookup = match (line, character) {
                    (Some(line), Some(character)) => Some((line, character)),
                    _ => self
                        .symbol_lookup_location(symbol, Some(&lookup_path))
                        .map(|(_, line, column)| (line, column)),
                };
                if let Some((line, column)) = lookup
                    && let Ok(definition) =
                        server.definition(&file.path, line, column, &file.content)
                    && let Some(mut definition) = definition
                {
                    if definition.name.is_empty() {
                        definition.name = symbol.trim().to_string();
                    }
                    if definition.signature.is_empty() {
                        definition.signature = symbol.trim().to_string();
                    }
                    return Some(definition);
                }
            }
        }
        let matches = self
            .symbol_index
            .get(symbol.trim())
            .cloned()
            .unwrap_or_default();
        if let Some(path) = candidate_path {
            if let Some(exact) = matches.iter().find(|entry| entry.location.path == path) {
                return Some(exact.clone());
            }
        }
        matches.into_iter().next()
    }

    pub fn references(&self, symbol: &str, limit: usize) -> Vec<Location> {
        self.references_at(None, symbol, None, None, limit)
    }

    pub fn references_at(
        &self,
        path: Option<&str>,
        symbol: &str,
        line: Option<usize>,
        character: Option<usize>,
        limit: usize,
    ) -> Vec<Location> {
        let candidate_path = path.map(normalize_path);
        if let Some(server) = &self.rust_language_server {
            let resolved_path = candidate_path.as_deref().map(str::to_string).or_else(|| {
                self.symbol_lookup_location(symbol, None)
                    .map(|(path, _, _)| path)
            });
            if let Some(lookup_path) = resolved_path
                && let Some(file) = self.file_for_path(&lookup_path)
                && file._language == Language::Rust
            {
                let lookup = match (line, character) {
                    (Some(line), Some(character)) => Some((line, character)),
                    _ => self
                        .symbol_lookup_location(symbol, Some(&lookup_path))
                        .map(|(_, line, column)| (line, column)),
                };
                if let Some((line, column)) = lookup
                    && let Ok(references) =
                        server.references(&file.path, line, column, &file.content)
                    && !references.is_empty()
                {
                    let mut references = references;
                    references.truncate(limit);
                    return references;
                }
            }
        }
        let pattern = Regex::new(&format!(r"\b{}\b", regex::escape(symbol.trim())))
            .unwrap_or_else(|_| Regex::new("$^").expect("valid empty regex"));
        let mut locations = Vec::new();
        for file in &self.files {
            for (line_index, line) in file.content.lines().enumerate() {
                for matched in pattern.find_iter(line) {
                    locations.push(Location {
                        path: file.path.clone(),
                        line: line_index + 1,
                        column: matched.start() + 1,
                    });
                    if locations.len() >= limit {
                        return locations;
                    }
                }
            }
        }
        locations
    }

    pub fn hover(&self, path: &str, line: usize, column: usize) -> Option<HoverInfo> {
        let normalized_path = normalize_path(path);
        let file = self.file_for_path(&normalized_path)?;
        if file._language == Language::Rust
            && let Some(server) = &self.rust_language_server
            && let Ok(hover) = server.hover(&file.path, line, column, &file.content)
            && hover.is_some()
        {
            return hover;
        }
        let line_text = file.content.lines().nth(line.saturating_sub(1))?;
        let symbol = identifier_at(line_text, column).or_else(|| {
            file.symbols
                .iter()
                .find(|symbol| symbol.location.line == line)
                .map(|symbol| symbol.name.clone())
        })?;
        let definition = self.definition(&symbol, Some(&file.path)).or_else(|| {
            file.symbols
                .iter()
                .find(|symbol| symbol.location.line == line)
                .and_then(|symbol| self.definition(&symbol.name, Some(&file.path)))
        })?;
        let reference_count = self.references(&definition.name, usize::MAX).len();
        Some(HoverInfo {
            symbol: definition.name,
            kind: definition.kind,
            signature: definition.signature,
            location: definition.location,
            reference_count,
        })
    }

    pub fn code_actions(&self, path: &str, line: usize, column: usize) -> Vec<CodeAction> {
        let normalized_path = normalize_path(path);
        let Some(file) = self.file_for_path(&normalized_path) else {
            return Vec::new();
        };
        if file._language == Language::Rust
            && let Some(server) = &self.rust_language_server
            && let Ok(code_actions) = server.code_actions(&file.path, line, column, &file.content)
            && !code_actions.is_empty()
        {
            return code_actions;
        }
        let Some(line_text) = file.content.lines().nth(line.saturating_sub(1)) else {
            return Vec::new();
        };
        let Some(symbol) = identifier_at(line_text, column).or_else(|| {
            file.symbols
                .iter()
                .find(|symbol| symbol.location.line == line)
                .map(|symbol| symbol.name.clone())
        }) else {
            return Vec::new();
        };
        let symbol = self
            .definition(&symbol, Some(&file.path))
            .map(|definition| definition.name)
            .or_else(|| {
                file.symbols
                    .iter()
                    .find(|candidate| candidate.location.line == line)
                    .map(|candidate| candidate.name.clone())
            })
            .unwrap_or(symbol);
        let references = self.references(&symbol, 50);
        vec![
            CodeAction {
                title: format!("Rename `{symbol}`"),
                kind: "rename".to_string(),
                detail: format!("Preview replacements for {} reference(s)", references.len()),
            },
            CodeAction {
                title: format!("Find references for `{symbol}`"),
                kind: "references".to_string(),
                detail: "Use the semantic index to inspect all occurrences.".to_string(),
            },
            CodeAction {
                title: format!("Go to definition of `{symbol}`"),
                kind: "definition".to_string(),
                detail: "Open the declaration site from the semantic index.".to_string(),
            },
        ]
    }

    pub fn rename_preview(&self, old_name: &str, new_name: &str, limit: usize) -> RenamePreview {
        if let Some(server) = &self.rust_language_server
            && let Some((path, line, column)) = self.symbol_lookup_location(old_name, None)
            && let Some(file) = self.file_for_path(&path)
            && file._language == Language::Rust
            && let Ok(locations) =
                server.rename_preview(&file.path, line, column, new_name, &file.content)
            && !locations.is_empty()
        {
            return RenamePreview {
                old_name: old_name.to_string(),
                new_name: new_name.to_string(),
                replacement_count: locations.len(),
                locations,
            };
        }
        let locations = self.references(old_name, limit);
        RenamePreview {
            old_name: old_name.to_string(),
            new_name: new_name.to_string(),
            replacement_count: locations.len(),
            locations,
        }
    }

    pub fn render_locations(&self, locations: &[Location]) -> String {
        if locations.is_empty() {
            return "[no matches]".to_string();
        }
        let mut lines = vec![format!("matches: {}", locations.len())];
        lines.extend(
            locations.iter().map(|location| {
                format!("- {}:{}:{}", location.path, location.line, location.column)
            }),
        );
        lines.join("\n")
    }

    pub fn render_symbols(&self, symbols: &[SymbolLocation]) -> String {
        if symbols.is_empty() {
            return "[no matches]".to_string();
        }
        let mut lines = vec![format!("matches: {}", symbols.len())];
        lines.extend(symbols.iter().map(|symbol| {
            format!(
                "- {}:{}:{} {} {}",
                symbol.location.path,
                symbol.location.line,
                symbol.location.column,
                symbol.kind,
                symbol.name
            )
        }));
        lines.join("\n")
    }

    pub fn render_diagnostics(&self, diagnostics: &[Diagnostic]) -> String {
        if diagnostics.is_empty() {
            return "[no diagnostics]".to_string();
        }
        let mut lines = vec![format!("diagnostics: {}", diagnostics.len())];
        lines.extend(diagnostics.iter().map(|diagnostic| {
            let location = match (diagnostic.line, diagnostic.column) {
                (Some(line), Some(column)) => format!(":{}:{}", line, column),
                (Some(line), None) => format!(":{}", line),
                _ => String::new(),
            };
            format!(
                "- {} {}{} {}",
                diagnostic.severity, diagnostic.path, location, diagnostic.message
            )
        }));
        lines.join("\n")
    }

    pub fn render_hover(&self, hover: &HoverInfo) -> String {
        format!(
            "[hover]\nname: {}\nkind: {}\nlocation: {}:{}:{}\nreferences: {}\nsignature: {}",
            hover.symbol,
            hover.kind,
            hover.location.path,
            hover.location.line,
            hover.location.column,
            hover.reference_count,
            hover.signature
        )
    }

    pub fn render_rename_preview(&self, preview: &RenamePreview) -> String {
        let mut lines = vec![
            "[rename_preview]".to_string(),
            format!("old_name: {}", preview.old_name),
            format!("new_name: {}", preview.new_name),
            format!("replacement_count: {}", preview.replacement_count),
        ];
        if preview.locations.is_empty() {
            lines.push("[no replacements]".to_string());
        } else {
            lines.extend(preview.locations.iter().map(|location| {
                format!("- {}:{}:{}", location.path, location.line, location.column)
            }));
        }
        lines.join("\n")
    }

    fn file_for_path(&self, path: &str) -> Option<&IndexedFile> {
        self.files
            .iter()
            .find(|file| file.path == normalize_path(path))
    }

    fn symbol_lookup_location(
        &self,
        symbol: &str,
        preferred_path: Option<&str>,
    ) -> Option<(String, usize, usize)> {
        let symbol = symbol.trim();
        let entries = self.symbol_index.get(symbol)?;
        if let Some(preferred_path) = preferred_path
            && let Some(entry) = entries
                .iter()
                .find(|entry| entry.location.path == preferred_path)
        {
            return Some((
                entry.location.path.clone(),
                entry.location.line,
                entry.location.column,
            ));
        }
        entries.first().map(|entry| {
            (
                entry.location.path.clone(),
                entry.location.line,
                entry.location.column,
            )
        })
    }
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn dedupe_symbols(symbols: Vec<SymbolLocation>) -> Vec<SymbolLocation> {
    let mut seen = std::collections::BTreeSet::new();
    let mut deduped = Vec::new();
    for symbol in symbols {
        let key = (
            symbol.location.path.clone(),
            symbol.location.line,
            symbol.location.column,
            symbol.name.clone(),
        );
        if seen.insert(key) {
            deduped.push(symbol);
        }
    }
    deduped
}

fn extract_symbols(path: &str, language: Language, content: &str) -> Vec<SymbolLocation> {
    match language {
        Language::Rust => extract_line_based_symbols(
            path,
            content,
            &[
                ("pub fn ", "function"),
                ("fn ", "function"),
                ("pub struct ", "struct"),
                ("struct ", "struct"),
                ("pub enum ", "enum"),
                ("enum ", "enum"),
                ("pub trait ", "trait"),
                ("trait ", "trait"),
                ("pub mod ", "module"),
                ("mod ", "module"),
                ("pub type ", "type"),
                ("type ", "type"),
                ("impl ", "impl"),
            ],
        ),
        Language::TypeScript => extract_line_based_symbols(
            path,
            content,
            &[
                ("export function ", "function"),
                ("export class ", "class"),
                ("export interface ", "interface"),
                ("function ", "function"),
                ("class ", "class"),
                ("interface ", "interface"),
                ("type ", "type"),
            ],
        ),
        Language::Python => {
            extract_line_based_symbols(path, content, &[("def ", "function"), ("class ", "class")])
        }
        Language::Go => {
            extract_line_based_symbols(path, content, &[("func ", "function"), ("type ", "type")])
        }
        Language::Markdown => content
            .lines()
            .enumerate()
            .filter_map(|(line_index, line)| {
                let trimmed = line.trim_start();
                let heading = trimmed.strip_prefix('#')?;
                let name = heading.trim().trim_start_matches('#').trim();
                (!name.is_empty()).then(|| SymbolLocation {
                    name: name.to_string(),
                    kind: "heading".to_string(),
                    signature: trimmed.to_string(),
                    location: Location {
                        path: path.to_string(),
                        line: line_index + 1,
                        column: line.len().saturating_sub(trimmed.len()) + 1,
                    },
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn extract_line_based_symbols(
    path: &str,
    content: &str,
    prefixes: &[(&str, &str)],
) -> Vec<SymbolLocation> {
    let mut symbols = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        for (prefix, kind) in prefixes {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                let name = rest
                    .split(['(', '<', '{', ':', ' ', '\t'])
                    .next()
                    .unwrap_or("")
                    .trim_matches(|character: char| {
                        matches!(character, ':' | '{' | '(' | ';' | ',' | ')')
                    });
                if name.is_empty() {
                    continue;
                }
                let column = line.len().saturating_sub(trimmed.len()) + prefix.len() + 1;
                symbols.push(SymbolLocation {
                    name: name.to_string(),
                    kind: kind.to_string(),
                    signature: trimmed.to_string(),
                    location: Location {
                        path: path.to_string(),
                        line: line_index + 1,
                        column,
                    },
                });
                break;
            }
        }
    }
    symbols
}

fn diagnostics_for_file(path: &str, language: Language, content: &str) -> Vec<Diagnostic> {
    match language {
        Language::Rust => match syn::parse_file(content) {
            Ok(_) => Vec::new(),
            Err(error) => vec![Diagnostic {
                path: path.to_string(),
                severity: "error".to_string(),
                message: error.to_string(),
                line: None,
                column: None,
            }],
        },
        Language::Json => match serde_json::from_str::<serde_json::Value>(content) {
            Ok(_) => Vec::new(),
            Err(error) => vec![Diagnostic {
                path: path.to_string(),
                severity: "error".to_string(),
                message: error.to_string(),
                line: None,
                column: None,
            }],
        },
        Language::Toml => match content.parse::<toml::Value>() {
            Ok(_) => Vec::new(),
            Err(error) => vec![Diagnostic {
                path: path.to_string(),
                severity: "error".to_string(),
                message: error.to_string(),
                line: None,
                column: None,
            }],
        },
        _ => Vec::new(),
    }
}

fn identifier_at(line: &str, column: usize) -> Option<String> {
    if line.is_empty() {
        return None;
    }
    let cursor = column.saturating_sub(1).min(line.len());
    let bytes = line.as_bytes();
    let mut start = cursor;
    while start > 0 && is_identifier_char(bytes.get(start - 1).copied().unwrap_or_default() as char)
    {
        start -= 1;
    }
    let mut end = cursor;
    while end < bytes.len() && is_identifier_char(bytes[end] as char) {
        end += 1;
    }
    if start == end {
        return None;
    }
    Some(line[start..end].to_string())
}

fn is_identifier_char(character: char) -> bool {
    character == '_' || character.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn extracts_symbols_and_references() {
        let workspace = tempdir().expect("tempdir");
        let source = workspace.path().join("src/lib.rs");
        std::fs::create_dir_all(source.parent().expect("parent")).expect("create source dir");
        std::fs::write(&source, "pub fn alpha() {}\n\nfn beta() { alpha(); }\n")
            .expect("write source");
        let index = WorkspaceSemanticIndex::build(workspace.path()).expect("index");
        let symbols = index.workspace_symbols("alpha", 10);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "alpha");
        assert!(index.references("alpha", 10).len() >= 2);
    }

    #[test]
    fn produces_hover_and_rename_preview() {
        let workspace = tempdir().expect("tempdir");
        let source = workspace.path().join("src/main.rs");
        std::fs::create_dir_all(source.parent().expect("parent")).expect("create source dir");
        std::fs::write(
            &source,
            "pub struct Gamma;\nimpl Gamma { fn new() -> Self { Self } }\n",
        )
        .expect("write source");
        let index = WorkspaceSemanticIndex::build(workspace.path()).expect("index");
        let hover = index.hover("src/main.rs", 1, 5).expect("hover");
        assert_eq!(hover.symbol, "Gamma");
        let preview = index.rename_preview("Gamma", "Delta", 10);
        assert_eq!(preview.old_name, "Gamma");
        assert!(!preview.locations.is_empty());
    }

    #[test]
    fn parses_toml_diagnostics() {
        let workspace = tempdir().expect("tempdir");
        let source = workspace.path().join("Cargo.toml");
        std::fs::write(&source, "invalid = [").expect("write source");
        let index = WorkspaceSemanticIndex::build(workspace.path()).expect("index");
        assert!(!index.diagnostics("Cargo.toml").is_empty());
    }

    #[test]
    fn rust_language_server_results_override_scanner_for_rust_files() {
        let workspace = tempdir().expect("tempdir");
        let source = workspace.path().join("src/lib.rs");
        std::fs::create_dir_all(source.parent().expect("parent")).expect("create source dir");
        std::fs::write(&source, "pub fn demo_symbol() {}\n").expect("write source");

        let server_script = workspace.path().join("fake-rust-lsp.py");
        std::fs::write(
            &server_script,
            r#"#!/usr/bin/env python3
import json
import sys

def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        key, value = line.decode("utf-8").split(":", 1)
        headers[key.lower()] = value.strip()
    length = int(headers["content-length"])
    body = sys.stdin.buffer.read(length)
    return json.loads(body.decode("utf-8"))

def send_message(payload):
    body = json.dumps(payload).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

opened_uri = None
while True:
    message = read_message()
    if message is None:
        break
    method = message.get("method")
    if method == "initialize":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "definitionProvider": True,
                    "referencesProvider": True,
                    "hoverProvider": True,
                    "documentSymbolProvider": True,
                    "workspaceSymbolProvider": True,
                    "codeActionProvider": True,
                    "renameProvider": True
                },
                "serverInfo": {"name": "fake-rust-lsp", "version": "1.0.0"}
            }
        })
    elif method == "initialized":
        pass
    elif method == "textDocument/didOpen":
        opened_uri = message["params"]["textDocument"]["uri"]
        send_message({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": opened_uri,
                "diagnostics": [
                    {
                        "message": "fake diagnostic",
                        "severity": 2,
                        "range": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 0, "character": 3}
                        }
                    }
                ]
            }
        })
    elif method == "textDocument/hover":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "contents": {"kind": "markdown", "value": "hover from lsp"},
                "range": {
                    "start": {"line": 0, "character": 4},
                    "end": {"line": 0, "character": 8}
                }
            }
        })
    elif method == "textDocument/definition":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "uri": opened_uri,
                "range": {
                    "start": {"line": 0, "character": 4},
                    "end": {"line": 0, "character": 8}
                }
            }
        })
    elif method == "textDocument/references":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": [
                {
                    "uri": opened_uri,
                    "range": {
                        "start": {"line": 0, "character": 4},
                        "end": {"line": 0, "character": 8}
                    }
                },
                {
                    "uri": opened_uri,
                    "range": {
                        "start": {"line": 0, "character": 12},
                        "end": {"line": 0, "character": 16}
                    }
                }
            ]
        })
    elif method == "textDocument/documentSymbol":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": [
                {
                    "name": "demo_symbol",
                    "kind": 12,
                    "range": {
                        "start": {"line": 0, "character": 4},
                        "end": {"line": 0, "character": 16}
                    }
                }
            ]
        })
    elif method == "workspace/symbol":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": [
                {
                    "name": "demo_symbol",
                    "kind": 12,
                    "location": {
                        "uri": opened_uri,
                        "range": {
                            "start": {"line": 0, "character": 4},
                            "end": {"line": 0, "character": 16}
                        }
                    }
                }
            ]
        })
    elif method == "textDocument/codeAction":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": [
                {"title": "Rename demo_symbol", "kind": "refactor.rename"}
            ]
        })
    elif method == "textDocument/rename":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "changes": {
                    opened_uri: [
                        {
                            "range": {
                                "start": {"line": 0, "character": 4},
                                "end": {"line": 0, "character": 16}
                            },
                            "newText": "renamed_symbol"
                        }
                    ]
                }
            }
        })
    elif "id" in message:
        send_message({"jsonrpc": "2.0", "id": message["id"], "result": None})
"#,
        )
        .expect("write fake lsp");

        let command_line = format!("python3 {}", server_script.display());
        let index = WorkspaceSemanticIndex::build_with_rust_language_server(
            workspace.path(),
            Some(&command_line),
        )
        .expect("build index");
        assert!(index.has_rust_language_server());

        let diagnostics = index.diagnostics("src/lib.rs");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "fake diagnostic");

        let hover = index.hover("src/lib.rs", 1, 5).expect("hover");
        assert_eq!(hover.signature, "hover from lsp");
        assert_eq!(hover.reference_count, 0);

        let definition = index
            .definition("demo_symbol", Some("src/lib.rs"))
            .expect("definition");
        assert_eq!(definition.name, "demo_symbol");
        assert_eq!(definition.location.line, 1);

        let references = index.references("demo_symbol", 8);
        assert_eq!(references.len(), 2);

        let document_symbols = index.document_symbols("src/lib.rs");
        assert_eq!(document_symbols.len(), 1);
        assert_eq!(document_symbols[0].name, "demo_symbol");

        let workspace_symbols = index.workspace_symbols("demo", 8);
        assert!(
            workspace_symbols
                .iter()
                .any(|symbol| symbol.name == "demo_symbol")
        );

        let code_actions = index.code_actions("src/lib.rs", 1, 5);
        assert_eq!(code_actions.len(), 1);
        assert_eq!(code_actions[0].title, "Rename demo_symbol");

        let preview = index.rename_preview("demo_symbol", "renamed_symbol", 8);
        assert_eq!(preview.replacement_count, 1);
        assert_eq!(preview.locations.len(), 1);
    }
}

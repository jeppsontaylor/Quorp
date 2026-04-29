use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use quorp_context_model::{ContextItem, ItemMeta, Source, Trust};
use quorp_ids::{ChunkId, PackId};
use quorp_repo_graph::{LineRange, SymbolKind, SymbolPath};
use quorp_repo_scan::{Language, ScannedFile, harvest_rust_symbols, scan};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

const INDEX_RELATIVE_PATH: &str = ".quorp/index/index.sqlite";
const MAX_INDEXED_FILE_BYTES: u64 = 512 * 1024;
const DEFAULT_CHUNK_LINES: usize = 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexBuildReport {
    pub database_path: PathBuf,
    pub indexed_files: usize,
    pub changed_files: usize,
    pub skipped_files: usize,
    pub symbol_count: usize,
    pub lexical_chunk_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexStatus {
    pub database_path: PathBuf,
    pub exists: bool,
    pub indexed_files: usize,
    pub stale_files: usize,
    pub symbol_count: usize,
    pub lexical_chunk_count: usize,
    pub last_build_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolDefinition {
    pub path: PathBuf,
    pub kind: String,
    pub range: LineRange,
    pub definition_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolExplanation {
    pub symbol: String,
    pub definitions: Vec<SymbolDefinition>,
    pub references: usize,
    pub tests: Vec<String>,
}

#[derive(Debug, Clone)]
struct IndexedFileRecord {
    sha256: String,
    modified_unix_ms: i64,
}

#[derive(Debug, Clone)]
struct LexicalChunkRecord {
    path: PathBuf,
    range: LineRange,
    text: String,
    source_hash: String,
    score: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct IndexReader {
    workspace_root: PathBuf,
    database_path: PathBuf,
}

impl IndexReader {
    pub(crate) fn open_if_fresh(workspace_root: &Path) -> Result<Option<Self>> {
        let status = index_status(workspace_root)?;
        if !status.exists || status.stale_files != 0 {
            return Ok(None);
        }
        Ok(Some(Self {
            workspace_root: workspace_root.to_path_buf(),
            database_path: status.database_path,
        }))
    }

    fn open_connection(&self) -> Result<Connection> {
        open_index_database(&self.database_path)
    }

    pub(crate) fn lexical_excerpts(&self, query: &str, limit: usize) -> Result<Vec<ContextItem>> {
        let terms = crate::query_terms(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.open_connection()?;
        let mut statement = connection.prepare(
            "SELECT path, start_line, end_line, text, source_hash, lower_text
             FROM lexical_chunks",
        )?;
        let chunks = statement
            .query_map([], |row| {
                let lower_text: String = row.get(5)?;
                let score = terms
                    .iter()
                    .filter(|term| lower_text.contains(term.as_str()))
                    .count();
                Ok(LexicalChunkRecord {
                    path: PathBuf::from(row.get::<_, String>(0)?),
                    range: LineRange {
                        start: row.get::<_, u32>(1)?,
                        end: row.get::<_, u32>(2)?,
                    },
                    text: row.get(3)?,
                    source_hash: row.get(4)?,
                    score,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut ranked = chunks
            .into_iter()
            .filter(|chunk| chunk.score > 0)
            .collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.range.start.cmp(&right.range.start))
        });
        Ok(ranked
            .into_iter()
            .take(limit)
            .map(|chunk| {
                let cost = crate::estimate_tokens(&chunk.text);
                ContextItem::Excerpt {
                    chunk: ChunkId::new(format!(
                        "lexical:{}:{}-{}",
                        chunk.path.display(),
                        chunk.range.start,
                        chunk.range.end
                    )),
                    path: chunk.path,
                    range: chunk.range,
                    text: chunk.text,
                    meta: ItemMeta {
                        relevance: (0.55 + (chunk.score as f32 * 0.05)).min(0.9),
                        freshness_secs: 0,
                        trust: Trust::Derived,
                        cost_tokens: cost,
                        source: Source::Lexical,
                        source_hash: Some(chunk.source_hash),
                        selection_reason: Some(format!("indexed lexical hit for query `{query}`")),
                    },
                }
            })
            .collect())
    }

    pub(crate) fn symbol_excerpts(&self, symbol: &SymbolPath) -> Result<Vec<ContextItem>> {
        let connection = self.open_connection()?;
        let mut statement = connection.prepare(
            "SELECT path, kind, range_start, range_end, definition_hash
             FROM symbols
             WHERE name = ?1
             ORDER BY path ASC, range_start ASC",
        )?;
        let definitions = statement
            .query_map([symbol.as_str()], |row| {
                Ok(SymbolDefinition {
                    path: PathBuf::from(row.get::<_, String>(0)?),
                    kind: row.get(1)?,
                    range: LineRange {
                        start: row.get::<_, u32>(2)?,
                        end: row.get::<_, u32>(3)?,
                    },
                    definition_hash: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut items = Vec::new();
        for definition in definitions {
            let absolute = self.workspace_root.join(&definition.path);
            let text = match fs::read_to_string(&absolute) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let excerpt_range = LineRange {
                start: definition.range.start.saturating_sub(2).max(1),
                end: definition.range.end.saturating_add(2),
            };
            let excerpt = crate::slice_lines(&text, excerpt_range);
            let cost = crate::estimate_tokens(&excerpt);
            items.push(ContextItem::SymbolDef {
                chunk: ChunkId::new(format!(
                    "symbol:{}:{}-{}",
                    definition.path.display(),
                    definition.range.start,
                    definition.range.end
                )),
                path: symbol.clone(),
                signature: format!("{} {}", definition.kind, symbol.as_str()),
                body_excerpt: excerpt,
                meta: ItemMeta {
                    relevance: 0.97,
                    freshness_secs: 0,
                    trust: Trust::Derived,
                    cost_tokens: cost,
                    source: Source::Symbol,
                    source_hash: Some(definition.definition_hash),
                    selection_reason: Some(format!(
                        "indexed symbol definition for `{}`",
                        symbol.as_str()
                    )),
                },
            });
        }
        Ok(items)
    }

    pub(crate) fn record_context_pack(
        &self,
        pack_id: &PackId,
        items: &[ContextItem],
    ) -> Result<()> {
        let mut connection = self.open_connection()?;
        let transaction = connection.transaction()?;
        for item in items {
            let meta = item_meta(item);
            transaction.execute(
                "INSERT OR REPLACE INTO context_pack_items
                 (pack_id, item_key, source_hash, reason, trust_level, freshness_secs, budget_cost)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    pack_id.as_str(),
                    crate::item_key(item),
                    meta.source_hash,
                    meta.selection_reason,
                    format!("{:?}", meta.trust),
                    i64::try_from(meta.freshness_secs).unwrap_or(i64::MAX),
                    i64::from(meta.cost_tokens),
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
}

pub fn default_index_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(INDEX_RELATIVE_PATH)
}

pub fn build_index(workspace_root: &Path) -> Result<IndexBuildReport> {
    let database_path = default_index_path(workspace_root);
    let mut connection = open_index_database(&database_path)?;
    initialize_schema(&connection)?;

    let existing = load_existing_file_records(&connection)?;
    let scanned = scan(workspace_root);
    let mut seen_paths = BTreeSet::new();
    let mut indexed_files = 0usize;
    let mut changed_files = 0usize;
    let mut skipped_files = 0usize;

    let transaction = connection.transaction()?;
    for file in scanned {
        if !should_index_file(&file) {
            skipped_files += 1;
            continue;
        }
        let relative = relative_path(workspace_root, &file.path);
        seen_paths.insert(relative.clone());

        let bytes = fs::read(&file.path)?;
        let sha256 = hash_bytes(&bytes);
        let modified_unix_ms = modified_unix_ms(&file.path)?;
        let unchanged = existing
            .get(relative.to_string_lossy().as_ref())
            .is_some_and(|record| {
                record.sha256 == sha256 && record.modified_unix_ms == modified_unix_ms
            });
        if unchanged {
            indexed_files += 1;
            continue;
        }

        changed_files += 1;
        indexed_files += 1;
        delete_path_rows(&transaction, &relative)?;

        let text = String::from_utf8_lossy(&bytes).into_owned();
        transaction.execute(
            "INSERT OR REPLACE INTO files
             (path, sha256, bytes, modified_unix_ms, indexed_at_unix_ms, language)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                relative.to_string_lossy(),
                sha256,
                i64::try_from(bytes.len()).unwrap_or(i64::MAX),
                modified_unix_ms,
                now_unix_ms(),
                language_label(file.language),
            ],
        )?;
        insert_lexical_chunks(&transaction, &relative, &text)?;
        insert_symbols(&transaction, &relative, &file, &text)?;
        insert_imports(&transaction, &relative, &text)?;
        insert_tests(&transaction, &relative, &text)?;
    }

    for path in existing.keys() {
        if !seen_paths.contains(Path::new(path)) {
            delete_path_rows(&transaction, Path::new(path))?;
            transaction.execute("DELETE FROM files WHERE path = ?1", [path])?;
        }
    }

    transaction.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('last_build_unix_ms', ?1)",
        [now_unix_ms().to_string()],
    )?;
    transaction.commit()?;

    Ok(IndexBuildReport {
        symbol_count: count_rows(&connection, "symbols")?,
        lexical_chunk_count: count_rows(&connection, "lexical_chunks")?,
        database_path,
        indexed_files,
        changed_files,
        skipped_files,
    })
}

pub fn index_status(workspace_root: &Path) -> Result<IndexStatus> {
    let database_path = default_index_path(workspace_root);
    if !database_path.exists() {
        return Ok(IndexStatus {
            database_path,
            exists: false,
            indexed_files: 0,
            stale_files: 0,
            symbol_count: 0,
            lexical_chunk_count: 0,
            last_build_unix_ms: None,
        });
    }
    let connection = open_index_database(&database_path)?;
    initialize_schema(&connection)?;
    let existing = load_existing_file_records(&connection)?;
    let scanned = scan(workspace_root);
    let mut stale_files = 0usize;
    let mut seen = BTreeSet::new();
    for file in scanned {
        if !should_index_file(&file) {
            continue;
        }
        let relative = relative_path(workspace_root, &file.path);
        seen.insert(relative.clone());
        match existing.get(relative.to_string_lossy().as_ref()) {
            Some(record) if record.modified_unix_ms == modified_unix_ms(&file.path)? => {}
            _ => stale_files += 1,
        }
    }
    stale_files += existing
        .keys()
        .filter(|path| !seen.contains(Path::new(path.as_str())))
        .count();
    Ok(IndexStatus {
        indexed_files: existing.len(),
        stale_files,
        symbol_count: count_rows(&connection, "symbols")?,
        lexical_chunk_count: count_rows(&connection, "lexical_chunks")?,
        last_build_unix_ms: connection
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_build_unix_ms'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|value| value.parse::<i64>().ok()),
        database_path,
        exists: true,
    })
}

pub fn explain_symbol(workspace_root: &Path, symbol: &str) -> Result<SymbolExplanation> {
    let reader = IndexReader::open_if_fresh(workspace_root)?.ok_or_else(|| {
        anyhow::anyhow!(
            "context index is missing or stale; run `quorp index build --workspace {}` first",
            workspace_root.display()
        )
    })?;
    let connection = reader.open_connection()?;
    let mut definitions = connection
        .prepare(
            "SELECT path, kind, range_start, range_end, definition_hash
             FROM symbols
             WHERE name = ?1
             ORDER BY path ASC, range_start ASC",
        )?
        .query_map([symbol], |row| {
            Ok(SymbolDefinition {
                path: PathBuf::from(row.get::<_, String>(0)?),
                kind: row.get(1)?,
                range: LineRange {
                    start: row.get::<_, u32>(2)?,
                    end: row.get::<_, u32>(3)?,
                },
                definition_hash: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    definitions.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.range.start.cmp(&right.range.start))
    });
    let tests = connection
        .prepare(
            "SELECT command
             FROM tests
             WHERE name = ?1
             ORDER BY path ASC, name ASC",
        )?
        .query_map([symbol], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let references = count_rows_where(
        &connection,
        "symbol_references",
        "symbol_name = ?1",
        [symbol],
    )?;
    Ok(SymbolExplanation {
        symbol: symbol.to_string(),
        definitions,
        references,
        tests,
    })
}

pub fn context_pack_provenance_count(workspace_root: &Path) -> Result<usize> {
    let connection = open_index_database(&default_index_path(workspace_root))?;
    initialize_schema(&connection)?;
    count_rows(&connection, "context_pack_items")
}

fn open_index_database(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(Connection::open(path)?)
}

fn initialize_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS files (
            path TEXT PRIMARY KEY,
            sha256 TEXT NOT NULL,
            bytes INTEGER NOT NULL,
            modified_unix_ms INTEGER NOT NULL,
            indexed_at_unix_ms INTEGER NOT NULL,
            language TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS lexical_chunks (
            path TEXT NOT NULL,
            chunk_id TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            end_line INTEGER NOT NULL,
            text TEXT NOT NULL,
            lower_text TEXT NOT NULL,
            source_hash TEXT NOT NULL,
            PRIMARY KEY(path, chunk_id)
        );
        CREATE TABLE IF NOT EXISTS symbols (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            range_start INTEGER NOT NULL,
            range_end INTEGER NOT NULL,
            definition_hash TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS symbol_references (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            symbol_name TEXT NOT NULL,
            path TEXT NOT NULL,
            range_start INTEGER NOT NULL,
            range_end INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS diagnostics (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL,
            severity TEXT NOT NULL,
            code TEXT,
            message TEXT NOT NULL,
            range_start INTEGER NOT NULL,
            range_end INTEGER NOT NULL,
            source TEXT
        );
        CREATE TABLE IF NOT EXISTS imports (
            path TEXT NOT NULL,
            target TEXT NOT NULL,
            kind TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS tests (
            path TEXT NOT NULL,
            name TEXT NOT NULL,
            owner_path TEXT,
            command TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS proof_history (
            path TEXT NOT NULL,
            artifact TEXT NOT NULL,
            source_hash TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS context_pack_items (
            pack_id TEXT NOT NULL,
            item_key TEXT NOT NULL,
            source_hash TEXT,
            reason TEXT,
            trust_level TEXT NOT NULL,
            freshness_secs INTEGER NOT NULL,
            budget_cost INTEGER NOT NULL,
            PRIMARY KEY(pack_id, item_key)
        );
        ",
    )?;
    Ok(())
}

fn load_existing_file_records(
    connection: &Connection,
) -> Result<BTreeMap<String, IndexedFileRecord>> {
    let mut statement =
        connection.prepare("SELECT path, sha256, modified_unix_ms FROM files ORDER BY path ASC")?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                IndexedFileRecord {
                    sha256: row.get(1)?,
                    modified_unix_ms: row.get(2)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().collect())
}

fn delete_path_rows(connection: &Connection, path: &Path) -> Result<()> {
    let path = path.to_string_lossy();
    for table in [
        "lexical_chunks",
        "symbols",
        "imports",
        "tests",
        "diagnostics",
        "proof_history",
    ] {
        connection.execute(
            &format!("DELETE FROM {table} WHERE path = ?1"),
            [path.as_ref()],
        )?;
    }
    Ok(())
}

fn insert_lexical_chunks(connection: &Connection, path: &Path, text: &str) -> Result<()> {
    let lines = text.lines().collect::<Vec<_>>();
    for (chunk_index, chunk_lines) in lines.chunks(DEFAULT_CHUNK_LINES).enumerate() {
        let start = chunk_index * DEFAULT_CHUNK_LINES + 1;
        let end = start + chunk_lines.len().saturating_sub(1);
        let chunk_text = chunk_lines.join("\n");
        connection.execute(
            "INSERT OR REPLACE INTO lexical_chunks
             (path, chunk_id, start_line, end_line, text, lower_text, source_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                path.to_string_lossy(),
                format!("chunk-{chunk_index}"),
                i64::try_from(start).unwrap_or(i64::MAX),
                i64::try_from(end).unwrap_or(i64::MAX),
                chunk_text,
                chunk_lines.join("\n").to_ascii_lowercase(),
                hash_bytes(chunk_lines.join("\n").as_bytes()),
            ],
        )?;
    }
    Ok(())
}

fn insert_symbols(
    connection: &Connection,
    relative_path: &Path,
    file: &ScannedFile,
    text: &str,
) -> Result<()> {
    if file.language != Language::Rust {
        return Ok(());
    }
    for symbol in harvest_rust_symbols(file, text) {
        connection.execute(
            "INSERT INTO symbols
             (path, name, kind, range_start, range_end, definition_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                relative_path.to_string_lossy(),
                symbol.path.as_str(),
                symbol_kind_label(symbol.kind),
                i64::from(symbol.span.start),
                i64::from(symbol.span.end),
                hash_bytes(crate::slice_lines(text, symbol.span).as_bytes()),
            ],
        )?;
    }
    Ok(())
}

fn insert_imports(connection: &Connection, relative_path: &Path, text: &str) -> Result<()> {
    for line in text.lines() {
        let trimmed = line.trim();
        let target = if let Some(rest) = trimmed.strip_prefix("use ") {
            Some(("rust_use", rest.trim_end_matches(';').trim()))
        } else if let Some(rest) = trimmed.strip_prefix("mod ") {
            Some(("rust_mod", rest.trim_end_matches(';').trim()))
        } else if let Some(rest) = trimmed.strip_prefix("import ") {
            Some(("ts_import", rest.trim()))
        } else {
            None
        };
        if let Some((kind, target)) = target {
            connection.execute(
                "INSERT INTO imports (path, target, kind) VALUES (?1, ?2, ?3)",
                params![relative_path.to_string_lossy(), target, kind],
            )?;
        }
    }
    Ok(())
}

fn insert_tests(connection: &Connection, relative_path: &Path, text: &str) -> Result<()> {
    let lines = text.lines().collect::<Vec<_>>();
    for window in lines.windows(2) {
        let first = window[0].trim();
        let second = window[1].trim();
        if first == "#[test]"
            && let Some(name) = second
                .strip_prefix("fn ")
                .and_then(|rest| rest.split('(').next())
                .map(str::trim)
        {
            connection.execute(
                "INSERT INTO tests (path, name, owner_path, command) VALUES (?1, ?2, ?3, ?4)",
                params![
                    relative_path.to_string_lossy(),
                    name,
                    relative_path.to_string_lossy(),
                    format!("cargo test {name}"),
                ],
            )?;
        }
    }
    Ok(())
}

fn should_index_file(file: &ScannedFile) -> bool {
    matches!(
        file.language,
        Language::Rust
            | Language::TypeScript
            | Language::Python
            | Language::Go
            | Language::Toml
            | Language::Json
            | Language::Markdown
    ) && file.bytes <= MAX_INDEXED_FILE_BYTES
}

fn relative_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn modified_unix_ms(path: &Path) -> Result<i64> {
    Ok(fs::metadata(path)?
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default())
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn count_rows(connection: &Connection, table: &str) -> Result<usize> {
    Ok(
        connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })? as usize,
    )
}

fn count_rows_where<P>(
    connection: &Connection,
    table: &str,
    predicate: &str,
    params: P,
) -> Result<usize>
where
    P: rusqlite::Params,
{
    Ok(connection.query_row(
        &format!("SELECT COUNT(*) FROM {table} WHERE {predicate}"),
        params,
        |row| row.get::<_, i64>(0),
    )? as usize)
}

fn language_label(language: Language) -> &'static str {
    match language {
        Language::Rust => "rust",
        Language::TypeScript => "typescript",
        Language::Python => "python",
        Language::Go => "go",
        Language::Toml => "toml",
        Language::Json => "json",
        Language::Markdown => "markdown",
        Language::Other => "other",
    }
}

fn symbol_kind_label(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "function",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait => "trait",
        SymbolKind::Impl => "impl",
        SymbolKind::Module => "module",
        SymbolKind::Const => "const",
        SymbolKind::TypeAlias => "type_alias",
        SymbolKind::Method => "method",
        SymbolKind::Static => "static",
        SymbolKind::Macro => "macro",
        SymbolKind::Test => "test",
        SymbolKind::Benchmark => "benchmark",
    }
}

fn item_meta(item: &ContextItem) -> &ItemMeta {
    match item {
        ContextItem::Excerpt { meta, .. }
        | ContextItem::SymbolDef { meta, .. }
        | ContextItem::Memory { meta, .. }
        | ContextItem::Rule { meta, .. }
        | ContextItem::AgentContract { meta, .. } => meta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompileContext, CompileRequest};
    use quorp_context_model::TokenBudget;

    #[test]
    fn build_and_reopen_index_round_trips_status() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join("src")).expect("src");
        fs::write(
            root.path().join("src/lib.rs"),
            "pub fn hello() {}\n#[test]\nfn smoke() {}\n",
        )
        .expect("source");

        let report = build_index(root.path()).expect("build");
        assert_eq!(report.indexed_files, 1);
        let status = index_status(root.path()).expect("status");
        assert!(status.exists);
        assert_eq!(status.stale_files, 0);
        assert!(status.symbol_count >= 1);
    }

    #[test]
    fn build_detects_incremental_invalidation() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join("src")).expect("src");
        let source_path = root.path().join("src/lib.rs");
        fs::write(&source_path, "pub fn hello() {}\n").expect("source");
        build_index(root.path()).expect("build");

        fs::write(&source_path, "pub fn hello() {}\npub fn goodbye() {}\n").expect("source");
        let stale = index_status(root.path()).expect("status");
        assert_eq!(stale.stale_files, 1);

        let report = build_index(root.path()).expect("rebuild");
        assert_eq!(report.changed_files, 1);
        assert!(index_status(root.path()).expect("status").stale_files == 0);
    }

    #[test]
    fn explain_symbol_returns_deterministic_definition_order() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join("src")).expect("src");
        fs::write(root.path().join("src/a.rs"), "pub fn shared() {}\n").expect("a");
        fs::write(root.path().join("src/b.rs"), "pub fn shared() {}\n").expect("b");
        build_index(root.path()).expect("build");

        let explanation = explain_symbol(root.path(), "shared").expect("explain");
        let paths = explanation
            .definitions
            .iter()
            .map(|definition| definition.path.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![PathBuf::from("src/a.rs"), PathBuf::from("src/b.rs")]
        );
    }

    #[test]
    fn index_reader_records_context_pack_provenance() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join("src")).expect("src");
        fs::write(
            root.path().join("src/lib.rs"),
            "pub fn indexed_symbol() {}\n",
        )
        .expect("source");
        build_index(root.path()).expect("build");

        let compiler = crate::ContextCompiler::new();
        let pack = compiler
            .compile_workspace(
                root.path(),
                &CompileRequest {
                    anchors: vec![Anchor::Symbol(SymbolPath::new("indexed_symbol"))],
                    budget: TokenBudget {
                        total: 1200,
                        per_item_cap: 600,
                        reserve_for_output: 100,
                    },
                },
                &CompileContext {
                    git_sha: None,
                    generated_at_unix: 42,
                },
            )
            .expect("compile");
        assert!(!pack.items.is_empty());
        assert!(context_pack_provenance_count(root.path()).expect("count") >= 1);
    }
}

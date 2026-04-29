use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResultHandle {
    pub content_hash: String,
    pub label: String,
    pub path: Option<PathBuf>,
    pub byte_len: usize,
    pub line_count: usize,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolSynopsis {
    pub tool_name: String,
    pub summary: String,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct HandleSummary {
    pub handle: ResultHandle,
    pub synopsis: Option<ToolSynopsis>,
}

#[derive(Debug, Clone)]
pub struct HandleStore {
    root: PathBuf,
}

impl HandleStore {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            root: workspace_root.into(),
        }
    }

    fn handles_dir(&self) -> PathBuf {
        self.root.join(".quorp/handles")
    }

    fn index_path(&self) -> PathBuf {
        self.handles_dir().join("index.sqlite")
    }

    fn open_index(&self) -> Result<Connection> {
        let path = self.index_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.execute(
            "CREATE TABLE IF NOT EXISTS handles (
                content_hash TEXT PRIMARY KEY,
                label TEXT NOT NULL,
                path TEXT,
                byte_len INTEGER NOT NULL,
                line_count INTEGER NOT NULL,
                payload_path TEXT NOT NULL
            )",
            [],
        )?;
        Ok(connection)
    }

    pub fn store(&self, handle: &ResultHandle, payload: &str) -> Result<PathBuf> {
        let handles_dir = self.handles_dir();
        fs::create_dir_all(&handles_dir)?;
        let payload_path = handles_dir.join(format!("{}.json", handle.content_hash));
        fs::write(&payload_path, payload)?;
        let connection = self.open_index()?;
        connection.execute(
            "INSERT OR REPLACE INTO handles (content_hash, label, path, byte_len, line_count, payload_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                handle.content_hash,
                handle.label,
                handle.path.as_ref().map(|path| path.display().to_string()),
                i64::try_from(handle.byte_len).unwrap_or(i64::MAX),
                i64::try_from(handle.line_count).unwrap_or(i64::MAX),
                payload_path.display().to_string(),
            ],
        )?;
        Ok(payload_path)
    }

    pub fn load(&self, content_hash: &str) -> Result<Option<String>> {
        let path = self.handles_dir().join(format!("{}.json", content_hash));
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(fs::read_to_string(path)?))
    }
}

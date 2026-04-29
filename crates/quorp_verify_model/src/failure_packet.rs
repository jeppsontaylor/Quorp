use std::path::PathBuf;

use crate::Failure;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailurePacketKind {
    Compiler,
    Test,
    Security,
    Command,
    Parse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureSpan {
    pub file: PathBuf,
    pub line: u32,
    #[serde(default)]
    pub column: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailurePacket {
    pub kind: FailurePacketKind,
    pub command: String,
    pub summary: String,
    #[serde(default)]
    pub primary_span: Option<FailureSpan>,
    #[serde(default)]
    pub failures: Vec<Failure>,
    #[serde(default)]
    pub redacted: bool,
    #[serde(default)]
    pub truncated: bool,
}

impl FailurePacket {
    pub fn primary_path(&self) -> Option<&PathBuf> {
        self.primary_span.as_ref().map(|span| &span.file)
    }
}

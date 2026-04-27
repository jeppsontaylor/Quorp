//! Pure data model for Quorp's repository intelligence graph.
//!
//! Nodes: file, module, crate, function, trait, impl, struct/enum, test,
//! benchmark, dependency.
//! Edges: defines, calls, implements, bounds, tested_by, changed_with.
//!
//! This crate has zero I/O. Population happens in `quorp_repo_scan`.

#![allow(dead_code)]

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Dotted symbol path, e.g. `crate::module::Type::method`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolPath(pub String);

impl SymbolPath {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Inclusive line range within a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

/// Identity of a file in the graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileId(pub PathBuf);

/// Kind of symbol a node represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Enum,
    Trait,
    Impl,
    TypeAlias,
    Const,
    Static,
    Macro,
    Module,
    Test,
    Benchmark,
}

/// A node in the repo graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolNode {
    pub path: SymbolPath,
    pub kind: SymbolKind,
    pub file: FileId,
    pub span: LineRange,
}

/// An edge in the repo graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Edge {
    Defines {
        module: SymbolPath,
        symbol: SymbolPath,
    },
    Calls {
        caller: SymbolPath,
        callee: SymbolPath,
    },
    Implements {
        ty: SymbolPath,
        trait_path: SymbolPath,
    },
    TestedBy {
        target: SymbolPath,
        test: SymbolPath,
    },
    ImportsFrom {
        from: FileId,
        module: SymbolPath,
    },
}

/// Coarse counters used for capacity planning. Fully populated in
/// `quorp_repo_scan`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct GraphStats {
    pub files: u32,
    pub symbols: u32,
    pub edges: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_path_round_trips() {
        let p = SymbolPath::new("quorp_core::PermissionMode");
        let json = serde_json::to_string(&p).unwrap();
        let back: SymbolPath = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}

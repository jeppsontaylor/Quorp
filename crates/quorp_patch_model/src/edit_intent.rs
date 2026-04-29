use std::path::PathBuf;

use quorp_repo_graph::{LineRange, SymbolPath};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EditIntent {
    WholeFile {
        path: PathBuf,
    },
    TextRange {
        path: PathBuf,
        range: LineRange,
    },
    RustItemReplacement {
        path: PathBuf,
        item_path: SymbolPath,
        replacement_summary: Option<String>,
    },
    RustStructField {
        path: PathBuf,
        struct_path: SymbolPath,
        field_name: String,
        field_type: Option<String>,
    },
    RustEnumVariant {
        path: PathBuf,
        enum_path: SymbolPath,
        variant_name: String,
    },
    RustMatchArm {
        path: PathBuf,
        match_anchor: String,
    },
    TomlSet {
        path: PathBuf,
        table: String,
        key: String,
    },
    MarkdownSection {
        path: PathBuf,
        heading: String,
    },
}

impl EditIntent {
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::WholeFile { .. } => "whole_file",
            Self::TextRange { .. } => "text_range",
            Self::RustItemReplacement { .. } => "rust_item_replacement",
            Self::RustStructField { .. } => "rust_struct_field",
            Self::RustEnumVariant { .. } => "rust_enum_variant",
            Self::RustMatchArm { .. } => "rust_match_arm",
            Self::TomlSet { .. } => "toml_set",
            Self::MarkdownSection { .. } => "markdown_section",
        }
    }
}

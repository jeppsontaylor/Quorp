use serde::{Deserialize, Serialize};

use quorp_context::{ResultHandle, ToolSynopsis};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolPayload {
    Full {
        content: String,
    },
    Handle {
        handle: ResultHandle,
        synopsis: ToolSynopsis,
        slices: Vec<String>,
    },
}

use std::path::Path;

use quorp_patch_model::EditIntent;
use quorp_patch_vm::{WriteAmplification, WriteLease};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteContract {
    pub intent: EditIntent,
    pub amplification: WriteAmplification,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<WriteLease>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum WriteContractDecision {
    Allowed { contract: WriteContract },
    Denied { reason: String },
}

pub fn classify_write_file(
    path: &Path,
    content: &str,
    lease: Option<WriteLease>,
) -> WriteContractDecision {
    let amplification =
        WriteAmplification::from_content_change("write_file", None, content.as_bytes());
    let intent = EditIntent::WholeFile {
        path: path.to_path_buf(),
    };
    if amplification.is_broad_source_write() && lease.is_none() && !is_generated_path(path) {
        return WriteContractDecision::Denied {
            reason: format!(
                "source WriteFile for {} exceeds the 200-line limit; lower it to a semantic edit or attach an explicit lease",
                path.display()
            ),
        };
    }
    WriteContractDecision::Allowed {
        contract: WriteContract {
            intent,
            amplification,
            lease,
        },
    }
}

pub fn classify_semantic_write(
    path: &Path,
    operation_kind: &str,
    before_bytes: Option<&[u8]>,
    after_bytes: &[u8],
    lease: Option<WriteLease>,
) -> WriteContract {
    WriteContract {
        intent: EditIntent::WholeFile {
            path: path.to_path_buf(),
        },
        amplification: WriteAmplification::from_content_change(
            operation_kind,
            before_bytes,
            after_bytes,
        ),
        lease,
    }
}

pub fn is_generated_path(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            std::path::Component::Normal(part) if part == "target" || part == ".quorp-runs"
        )
    })
}
#[cfg(test)]
#[path = "../../../testing/quorp_tools/write_contract/tests.rs"]
mod tests;

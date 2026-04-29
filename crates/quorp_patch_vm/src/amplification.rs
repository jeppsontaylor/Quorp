use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteAmplification {
    pub before_bytes: usize,
    pub after_bytes: usize,
    pub before_lines: usize,
    pub after_lines: usize,
    pub changed_lines_estimate: usize,
    pub operation_kind: String,
}

impl WriteAmplification {
    pub fn from_content_change(
        operation_kind: impl Into<String>,
        before_content: Option<&[u8]>,
        after_content: &[u8],
    ) -> Self {
        let before_content = before_content.unwrap_or_default();
        let before_text = String::from_utf8_lossy(before_content);
        let after_text = String::from_utf8_lossy(after_content);
        let before_lines = before_text.lines().count();
        let after_lines = after_text.lines().count();
        Self {
            before_bytes: before_content.len(),
            after_bytes: after_content.len(),
            before_lines,
            after_lines,
            changed_lines_estimate: before_lines.abs_diff(after_lines).max(1),
            operation_kind: operation_kind.into(),
        }
    }

    pub fn is_broad_source_write(&self) -> bool {
        self.operation_kind == "write_file" && self.after_lines > 200
    }
}

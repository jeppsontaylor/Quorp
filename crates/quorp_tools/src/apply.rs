use std::path::Path;

use crate::edit::write_full_file;
use crate::patch::{
    PatchOperation, apply_resolved_file_patches, parse_multi_file_patch, perform_block_replacement,
    resolve_file_patches, sanitize_project_path, try_parse_search_replace_blocks,
};
use crate::preview::{
    normalize_single_file_hunk_patch, perform_line_replacement_shorthand,
    try_parse_line_replacement_shorthand,
};

pub fn apply_patch_edit(
    project_root: &Path,
    cwd: &Path,
    path: &str,
    patch: &str,
    mut stash_touched_path: impl FnMut(&Path),
) -> anyhow::Result<String> {
    let target = sanitize_project_path(project_root, cwd, path)?;
    if let Some(blocks) = try_parse_search_replace_blocks(patch) {
        let mut current_content = std::fs::read_to_string(&target)
            .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
        stash_touched_path(&target);
        for (search, replace) in blocks {
            current_content = perform_block_replacement(&current_content, &search, &replace, None)?;
        }
        write_full_file(&target, &current_content)?;
        return Ok(format!("Applied search/replace blocks to {path}"));
    }

    if let Some(line_replacement) = try_parse_line_replacement_shorthand(patch)? {
        let mut current_content = std::fs::read_to_string(&target)
            .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
        let line_number = perform_line_replacement_shorthand(
            &mut current_content,
            &line_replacement.search,
            &line_replacement.replace,
        )?;
        stash_touched_path(&target);
        write_full_file(&target, &current_content)?;
        return Ok(format!(
            "Applied single-line replacement shorthand to {path}: line {line_number}"
        ));
    }

    let (patch_input, normalized_single_file_hunk) = normalize_single_file_hunk_patch(path, patch)?;
    let file_patches = parse_multi_file_patch(patch_input.as_deref().unwrap_or(patch))?;
    if file_patches.is_empty() {
        return Err(anyhow::anyhow!(
            "apply_patch expects a unified diff patch or SEARCH/REPLACE blocks"
        ));
    }

    let resolved = resolve_file_patches(project_root, cwd, &file_patches)?;
    for patch in &resolved {
        stash_touched_path(&patch.source_path);
        if patch.target_path != patch.source_path {
            stash_touched_path(&patch.target_path);
        }
    }
    apply_resolved_file_patches(&resolved)?;

    let summary = resolved
        .iter()
        .map(|patch| match &patch.operation {
            PatchOperation::Add => format!("A {}", patch.display_path),
            PatchOperation::Update => format!("M {}", patch.display_path),
            PatchOperation::Delete => format!("D {}", patch.display_path),
            PatchOperation::Move { move_path } => {
                format!("R {} -> {}", patch.display_path, move_path)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    if normalized_single_file_hunk {
        Ok(format!("Applied single-file hunk patch:\n{summary}"))
    } else {
        Ok(format!("Applied unified diff patch:\n{summary}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_patch_edit_stashes_before_unified_diff_write() {
        let root = tempfile::tempdir().expect("tempdir");
        let file = root.path().join("target.txt");
        write_full_file(&file, "old\n").expect("bootstrap");
        let mut stashed = Vec::new();

        let summary = apply_patch_edit(
            root.path(),
            root.path(),
            "target.txt",
            "--- a/target.txt\n+++ b/target.txt\n@@ -1 +1 @@\n-old\n+new\n",
            |path| stashed.push(path.to_path_buf()),
        )
        .expect("apply");

        assert_eq!(std::fs::read_to_string(file).expect("read"), "new\n");
        assert_eq!(
            stashed,
            vec![
                root.path()
                    .join("target.txt")
                    .canonicalize()
                    .expect("canonical target")
            ]
        );
        assert!(summary.contains("Applied unified diff patch"));
        assert!(summary.contains("M target.txt"));
    }

    #[test]
    fn apply_patch_edit_rejects_malformed_hunks() {
        let root = tempfile::tempdir().expect("tempdir");
        let file = root.path().join("target.txt");
        write_full_file(&file, "old").expect("bootstrap");
        let mut stashed = Vec::new();

        let error = apply_patch_edit(
            root.path(),
            root.path(),
            "target.txt",
            "--- a/target.txt\n+++ b/target.txt\n@@ -1,2 +1,1 @@\n-old\n",
            |path| stashed.push(path.to_path_buf()),
        )
        .expect_err("malformed hunk");

        assert!(error.to_string().contains("Malformed hunk"));
        assert!(stashed.is_empty());
        assert_eq!(
            std::fs::read_to_string(root.path().join("target.txt")).expect("read"),
            "old"
        );
    }
}

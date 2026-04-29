use std::path::Path;

use crate::patch::{
    PatchOperation, apply_resolved_file_patches, parse_multi_file_patch, perform_block_replacement,
    resolve_file_patches, sanitize_project_path, try_parse_search_replace_blocks,
};
use crate::preview::{
    normalize_single_file_hunk_patch, perform_line_replacement_shorthand,
    try_parse_line_replacement_shorthand,
};
use quorp_ids::PatchId;
use quorp_patch_vm::{
    EditProvenance, FileChange, FileChangeKind, PatchApplyProof, PatchApplyReport, PatchReceipt,
    PatchReceiptV2, PatchVm, PatchVmPolicy, WriteAmplification, edit_provenance_intent_kind,
    hash_bytes, smallest_safe_edit,
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
        let before_content = std::fs::read_to_string(&target)
            .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
        let base_hash = hash_bytes(before_content.as_bytes());
        let mut current_content = before_content.clone();
        stash_touched_path(&target);
        for (search, replace) in blocks {
            current_content = perform_block_replacement(&current_content, &search, &replace, None)?;
        }
        let report = apply_single_file_patch_change(&target, path, base_hash, &current_content)?;
        let receipt = report.receipt(EditProvenance::ApplyPatch { path: path.into() });
        let receipt_v2 = patch_receipt_v2(&receipt, &before_content, &current_content);
        return Ok(format!(
            "Applied search/replace blocks to {path}\n{}\n{}",
            render_patch_vm_receipt(&receipt),
            render_patch_vm_receipt_v2(&receipt_v2)
        ));
    }

    if let Some(line_replacement) = try_parse_line_replacement_shorthand(patch)? {
        let before_content = std::fs::read_to_string(&target)
            .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
        let base_hash = hash_bytes(before_content.as_bytes());
        let mut current_content = before_content.clone();
        let line_number = perform_line_replacement_shorthand(
            &mut current_content,
            &line_replacement.search,
            &line_replacement.replace,
        )?;
        stash_touched_path(&target);
        let report = apply_single_file_patch_change(&target, path, base_hash, &current_content)?;
        let receipt = report.receipt(EditProvenance::ApplyPatch { path: path.into() });
        let receipt_v2 = patch_receipt_v2(&receipt, &before_content, &current_content);
        return Ok(format!(
            "Applied single-line replacement shorthand to {path}: line {line_number}\n{}\n{}",
            render_patch_vm_receipt(&receipt),
            render_patch_vm_receipt_v2(&receipt_v2)
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
    let report = apply_resolved_file_patches(&resolved)?;

    let mut summary = resolved
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
    if let Some(report) = report {
        summary.push('\n');
        summary.push_str(&render_patch_vm_receipt(
            &report.receipt(EditProvenance::ApplyPatch { path: path.into() }),
        ));
    }

    if normalized_single_file_hunk {
        Ok(format!("Applied single-file hunk patch:\n{summary}"))
    } else {
        Ok(format!("Applied unified diff patch:\n{summary}"))
    }
}

fn apply_single_file_patch_change(
    target: &Path,
    display_path: &str,
    base_hash: quorp_patch_vm::FileHash,
    updated_content: &str,
) -> anyhow::Result<PatchApplyReport> {
    let change = FileChange {
        path: target.to_path_buf(),
        display_path: display_path.to_string(),
        expected_hash: Some(base_hash),
        kind: FileChangeKind::Update {
            content: updated_content.as_bytes().to_vec(),
        },
    };
    let patch_id = PatchId::new(format!(
        "apply-patch-{}-{}",
        hash_bytes(display_path.as_bytes()).0,
        hash_bytes(match &change.kind {
            FileChangeKind::Update { content } => content,
            FileChangeKind::Add { .. } | FileChangeKind::Delete | FileChangeKind::Move { .. } => {
                unreachable!("single file patch change is always an update")
            }
        })
        .0
    ));
    let vm = PatchVm::new();
    vm.apply_file_changes(
        &patch_id,
        &[change],
        PatchApplyProof::HashesOnly,
        PatchVmPolicy {
            allow_full_file_rewrite: true,
            max_files: 1,
        },
    )
    .map_err(anyhow::Error::from)
}

fn render_patch_vm_receipt(receipt: &PatchReceipt) -> String {
    format!(
        "patch_vm_receipt:\npatch_id: {}\nprovenance: {:?}\npreview_id: {}\noutcome: {:?}\ntouched_paths: {}\nrollback_tokens: {}",
        receipt.patch_id,
        receipt.provenance,
        receipt.preview_id,
        receipt.outcome,
        receipt
            .touched_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
        receipt.rollback_tokens.len()
    )
}

fn render_patch_vm_receipt_v2(receipt: &PatchReceiptV2) -> String {
    format!(
        "patch_vm_receipt_v2:\nintent_kind: {}\nbefore_hash: {}\nafter_hash: {}\nnormalized_diff_hash: {}\nsmallest_safe_edit: {}\nverifier_packet_ids: {}",
        receipt.intent_kind,
        receipt.before_hash.0,
        receipt.after_hash.0,
        receipt.normalized_diff_hash,
        receipt.smallest_safe_edit,
        receipt.verifier_packet_ids.join(", ")
    )
}

fn patch_receipt_v2(
    receipt: &PatchReceipt,
    before_content: &str,
    after_content: &str,
) -> PatchReceiptV2 {
    let amplification = WriteAmplification::from_content_change(
        edit_provenance_intent_kind(&receipt.provenance),
        Some(before_content.as_bytes()),
        after_content.as_bytes(),
    );
    PatchReceiptV2::from_receipt(
        receipt,
        hash_bytes(before_content.as_bytes()),
        hash_bytes(after_content.as_bytes()),
        smallest_safe_edit(&amplification),
        Vec::new(),
    )
}
#[cfg(test)]
#[path = "../../../testing/quorp_tools/apply/tests.rs"]
mod tests;

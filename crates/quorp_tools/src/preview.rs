#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LineReplacementShorthand {
    pub search: String,
    pub replace: String,
}

pub fn try_parse_line_replacement_shorthand(
    patch_text: &str,
) -> anyhow::Result<Option<LineReplacementShorthand>> {
    let normalized = patch_text.replace("\r\n", "\n").replace('\r', "\n");
    let meaningful = normalized
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let [search_line, replace_line] = meaningful.as_slice() else {
        return Ok(None);
    };
    let search_line = search_line.trim_start();
    let replace_line = replace_line.trim_start();
    let Some(search) = search_line.strip_prefix('/') else {
        return Ok(None);
    };
    let Some(replace) = replace_line.strip_prefix('+') else {
        return Ok(None);
    };
    let search = search.trim();
    if search.is_empty() {
        return Err(anyhow::anyhow!(
            "apply_patch line replacement shorthand requires a non-empty search string"
        ));
    }

    Ok(Some(LineReplacementShorthand {
        search: search.to_string(),
        replace: replace.to_string(),
    }))
}

pub fn perform_line_replacement_shorthand(
    content: &mut String,
    search: &str,
    replace: &str,
) -> anyhow::Result<usize> {
    let mut matches = Vec::new();
    for (index, line) in content.lines().enumerate() {
        let occurrences = line.matches(search).count();
        if occurrences > 0 {
            matches.push((index + 1, occurrences));
        }
    }

    let [(line_number, occurrences)] = matches.as_slice() else {
        if matches.is_empty() {
            return Err(anyhow::anyhow!(
                "apply_patch line replacement shorthand found no lines containing `{search}`. Use a unified diff hunk, SEARCH/REPLACE block, ReplaceBlock with range, or SuggestEditAnchors."
            ));
        }
        let line_numbers = matches
            .iter()
            .map(|(line_number, _)| line_number.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(anyhow::anyhow!(
            "apply_patch line replacement shorthand is ambiguous; `{search}` appears on lines {line_numbers}. Use a unified diff hunk with context, SEARCH/REPLACE block, ReplaceBlock with range, or SuggestEditAnchors."
        ));
    };
    if *occurrences > 1 {
        return Err(anyhow::anyhow!(
            "apply_patch line replacement shorthand is ambiguous; `{search}` appears more than once on line {line_number}. Use a unified diff hunk with context, SEARCH/REPLACE block, ReplaceBlock with range, or SuggestEditAnchors."
        ));
    }

    let mut lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    let Some(line) = lines.get_mut(line_number.saturating_sub(1)) else {
        return Err(anyhow::anyhow!(
            "apply_patch line replacement shorthand resolved outside file"
        ));
    };
    if line.trim() == search && replace.chars().next().is_some_and(char::is_whitespace) {
        *line = replace.to_string();
    } else {
        *line = line.replacen(search, replace, 1);
    }
    let had_trailing_newline = content.ends_with('\n');
    *content = lines.join("\n");
    if had_trailing_newline {
        content.push('\n');
    }
    Ok(*line_number)
}

pub fn normalize_single_file_hunk_patch(
    request_path: &str,
    patch_text: &str,
) -> anyhow::Result<(Option<String>, bool)> {
    let normalized = patch_text.replace("\r\n", "\n").replace('\r', "\n");
    let hunk_body = normalized.trim_start();
    if !hunk_body.starts_with("@@") {
        return Ok((None, false));
    }

    let path = request_path.trim();
    if path.is_empty() {
        return Err(anyhow::anyhow!(
            "apply_patch hunk-only input requires an explicit path"
        ));
    }
    if path.contains('\n') || path.contains('\r') {
        return Err(anyhow::anyhow!(
            "apply_patch hunk-only path cannot contain newlines"
        ));
    }

    Ok((
        Some(format!("--- a/{path}\n+++ b/{path}\n{hunk_body}")),
        true,
    ))
}

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use quorp_agent_core::{PreviewEditPayload, ReadFileRange, stable_content_hash};

use crate::edit::{apply_toml_operations, perform_range_replacement};
use crate::patch::{
    PatchOperation, format_preview_match_lines, parse_multi_file_patch, perform_block_replacement,
    preview_block_replacement_matches, resolve_file_patches, sanitize_project_path,
    try_parse_search_replace_blocks,
};

type PreviewCache = VecDeque<PreviewRecord>;

static PREVIEW_CACHE: OnceLock<Mutex<PreviewCache>> = OnceLock::new();
const PREVIEW_CACHE_LIMIT: usize = 32;

#[derive(Debug, Clone)]
pub struct PreviewRecord {
    pub preview_id: String,
    pub path: String,
    pub target_path: PathBuf,
    pub base_hash: String,
    pub edit_kind: String,
    pub updated_content: String,
    pub syntax_status: String,
}

fn get_preview_cache() -> &'static Mutex<PreviewCache> {
    PREVIEW_CACHE.get_or_init(|| Mutex::new(PreviewCache::new()))
}

pub fn store_preview_record(mut record: PreviewRecord) -> anyhow::Result<String> {
    let seed = format!(
        "{}\n{}\n{}\n{}",
        record.path, record.base_hash, record.edit_kind, record.updated_content
    );
    record.preview_id = format!("pv_{}", &stable_content_hash(&seed)[..12]);
    let preview_id = record.preview_id.clone();
    let mut cache = get_preview_cache()
        .lock()
        .map_err(|_| anyhow::anyhow!("preview cache lock poisoned"))?;
    cache.retain(|existing| existing.preview_id != preview_id);
    cache.push_back(record);
    while cache.len() > PREVIEW_CACHE_LIMIT {
        cache.pop_front();
    }
    Ok(preview_id)
}

pub fn load_preview_record(preview_id: &str) -> anyhow::Result<PreviewRecord> {
    let cache = get_preview_cache()
        .lock()
        .map_err(|_| anyhow::anyhow!("preview cache lock poisoned"))?;
    cache
        .iter()
        .find(|record| record.preview_id == preview_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("preview_id `{preview_id}` was not found or has expired"))
}

pub fn render_preview_edit_result(
    project_root: &Path,
    cwd: &Path,
    path: &str,
    target: &Path,
    edit: &PreviewEditPayload,
) -> anyhow::Result<String> {
    match edit {
        PreviewEditPayload::ApplyPatch { patch } => Ok(
            match preview_apply_patch_edit(project_root, cwd, path, patch) {
                Ok(summary) => format!(
                    "[preview_edit]\npath: {path}\nedit_kind: apply_patch\nwould_apply: true\nsyntax_preflight: unavailable\nsyntax_diagnostic: apply_patch preview validated target resolution but did not materialize a complete scratch file\n{summary}"
                ),
                Err(error) => format!(
                    "[preview_edit]\npath: {path}\nedit_kind: apply_patch\nwould_apply: false\ndiagnostic: {error}\nnormalized_suggestion: Use a unified diff with unique context, SEARCH/REPLACE blocks, or PreviewEdit a smaller ReplaceBlock before writing."
                ),
            },
        ),
        PreviewEditPayload::ReplaceBlock {
            search_block,
            replace_block,
            range,
        } => {
            let current_content = std::fs::read_to_string(target)
                .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
            Ok(
                match perform_block_replacement(
                    &current_content,
                    search_block,
                    replace_block,
                    *range,
                ) {
                    Ok(updated_content) => {
                        let matches = preview_block_replacement_matches(
                            &current_content,
                            search_block,
                            *range,
                        );
                        let matching_lines = format_preview_match_lines(&matches);
                        let syntax_preflight = syntax_preflight_for_preview(path, &updated_content);
                        let range_note = (*range)
                        .and_then(ReadFileRange::normalized)
                        .map(|range| format!("\nnormalized_suggestion: Use ReplaceBlock with range {{\"start_line\":{},\"end_line\":{}}} or ApplyPatch with the same unique context.", range.start_line, range.end_line))
                        .unwrap_or_else(|| {
                            matches.as_slice().first().map(|candidate| {
                                format!(
                                    "\nnormalized_suggestion: Use ReplaceBlock with range {{\"start_line\":{},\"end_line\":{}}} to keep the write anchored.",
                                    candidate.start_line,
                                    candidate.end_line
                                )
                            }).unwrap_or_default()
                        });
                        format!(
                            "[preview_edit]\npath: {path}\nedit_kind: replace_block\nwould_apply: true\nmatching_line_numbers: {matching_lines}\n{syntax_preflight}{range_note}"
                        )
                    }
                    Err(error) => {
                        let matches = preview_block_replacement_matches(
                            &current_content,
                            search_block,
                            *range,
                        );
                        let matching_lines = format_preview_match_lines(&matches);
                        format!(
                            "[preview_edit]\npath: {path}\nedit_kind: replace_block\nwould_apply: false\nmatching_line_numbers: {matching_lines}\ndiagnostic: {error}\nnormalized_suggestion: Ask for SuggestEditAnchors on the focused range, then use ApplyPatch or ranged ReplaceBlock with exact visible context."
                        )
                    }
                },
            )
        }
        PreviewEditPayload::ReplaceRange {
            range,
            expected_hash,
            replacement,
        } => {
            let current_content = std::fs::read_to_string(target)
                .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
            Ok(
                match perform_range_replacement(
                    &current_content,
                    *range,
                    expected_hash,
                    replacement,
                ) {
                    Ok(updated_content) => render_successful_preview(
                        path,
                        target,
                        "replace_range",
                        &current_content,
                        updated_content,
                    )?,
                    Err(error) => format!(
                        "[preview_edit]\npath: {path}\nedit_kind: replace_range\nwould_apply: false\ndiagnostic: {error}\nnormalized_suggestion: Reread the exact range, copy its content_hash, then retry PreviewEdit with replace_range or ReplaceRange."
                    ),
                },
            )
        }
        PreviewEditPayload::ModifyToml {
            expected_hash,
            operations,
        } => {
            let current_content = std::fs::read_to_string(target)
                .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
            Ok(
                match apply_toml_operations(&current_content, expected_hash, operations) {
                    Ok(updated_content) => render_successful_preview(
                        path,
                        target,
                        "modify_toml",
                        &current_content,
                        updated_content,
                    )?,
                    Err(error) => format!(
                        "[preview_edit]\npath: {path}\nedit_kind: modify_toml\nwould_apply: false\ndiagnostic: {error}\nnormalized_suggestion: Read the full manifest first, then use ModifyToml with that full-file content_hash."
                    ),
                },
            )
        }
    }
}

fn render_successful_preview(
    path: &str,
    target: &Path,
    edit_kind: &str,
    current_content: &str,
    updated_content: String,
) -> anyhow::Result<String> {
    let syntax_preflight = syntax_preflight_for_preview(path, &updated_content);
    let syntax_status = syntax_preflight
        .lines()
        .find_map(|line| line.strip_prefix("syntax_preflight:"))
        .map(str::trim)
        .unwrap_or("unavailable")
        .to_string();
    let base_hash = stable_content_hash(current_content);
    let preview_id = store_preview_record(PreviewRecord {
        preview_id: String::new(),
        path: path.to_string(),
        target_path: target.to_path_buf(),
        base_hash: base_hash.clone(),
        edit_kind: edit_kind.to_string(),
        updated_content,
        syntax_status,
    })?;
    let apply_preview_example = serde_json::json!({
        "actions": [
            {
                "ApplyPreview": {
                    "preview_id": preview_id
                }
            }
        ],
        "assistant_message": "Applying the clean preview."
    });
    Ok(format!(
        "[preview_edit]\npath: {path}\nedit_kind: {edit_kind}\nwould_apply: true\npreview_id: {preview_id}\nbase_hash: {base_hash}\n{syntax_preflight}\napply_preview: {apply_preview_example}"
    ))
}

fn preview_apply_patch_edit(
    project_root: &Path,
    cwd: &Path,
    path: &str,
    patch: &str,
) -> anyhow::Result<String> {
    let target = sanitize_project_path(project_root, cwd, path)?;
    if let Some(blocks) = try_parse_search_replace_blocks(patch) {
        let mut current_content = std::fs::read_to_string(&target)
            .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
        let mut line_notes = Vec::new();
        for (search, replace) in blocks {
            let matches = preview_block_replacement_matches(&current_content, &search, None);
            line_notes.push(format_preview_match_lines(&matches));
            current_content = perform_block_replacement(&current_content, &search, &replace, None)?;
        }
        return Ok(format!(
            "patch_form: search_replace_blocks\nmatching_line_numbers: {}",
            line_notes.join("; ")
        ));
    }

    if let Some(line_replacement) = try_parse_line_replacement_shorthand(patch)? {
        let mut current_content = std::fs::read_to_string(&target)
            .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
        let line_number = perform_line_replacement_shorthand(
            &mut current_content,
            &line_replacement.search,
            &line_replacement.replace,
        )?;
        return Ok(format!(
            "patch_form: line_replacement_shorthand\nmatching_line_numbers: {line_number}"
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
        .join(", ");
    let patch_form = if normalized_single_file_hunk {
        "single_file_hunk"
    } else {
        "unified_diff"
    };
    Ok(format!(
        "patch_form: {patch_form}\nresolved_files: {summary}"
    ))
}

#[allow(clippy::disallowed_methods)]
pub fn syntax_preflight_for_preview(path: &str, updated_content: &str) -> String {
    if path.ends_with(".toml") {
        return match updated_content.parse::<toml_edit::DocumentMut>() {
            Ok(_) => {
                "syntax_preflight: passed\nsyntax_diagnostic: TOML parser accepted scratch content"
                    .to_string()
            }
            Err(error) => format!(
                "syntax_preflight: failed\nsyntax_diagnostic: {}",
                truncate_anchor_line(&error.to_string(), 320)
            ),
        };
    }
    if !path.ends_with(".rs") {
        return "syntax_preflight: unavailable\nsyntax_diagnostic: no cheap syntax preflight registered for this file type".to_string();
    }
    let scratch_path = std::env::temp_dir().join(format!(
        "quorp-preview-{}-{}.rs",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    if let Err(error) = std::fs::write(&scratch_path, updated_content) {
        return format!(
            "syntax_preflight: unavailable\nsyntax_diagnostic: failed to write scratch file: {error}"
        );
    }
    let result = match std::process::Command::new("rustfmt")
        .arg("--check")
        .arg(&scratch_path)
        .output()
    {
        Ok(output) if output.status.success() => {
            "syntax_preflight: passed\nsyntax_diagnostic: rustfmt accepted scratch file".to_string()
        }
        Ok(output) => {
            let diagnostic = String::from_utf8_lossy(&output.stderr);
            let diagnostic = if diagnostic.trim().is_empty() {
                String::from_utf8_lossy(&output.stdout).to_string()
            } else {
                diagnostic.to_string()
            };
            format!(
                "syntax_preflight: failed\nsyntax_diagnostic: {}",
                truncate_anchor_line(diagnostic.trim(), 320)
            )
        }
        Err(error) => format!(
            "syntax_preflight: unavailable\nsyntax_diagnostic: rustfmt unavailable: {error}"
        ),
    };
    if let Err(error) = std::fs::remove_file(&scratch_path) {
        return format!("{result}\ncleanup_diagnostic: failed to remove scratch file: {error}");
    }
    result
}

fn truncate_anchor_line(line: &str, max_chars: usize) -> String {
    let mut truncated = String::new();
    for (index, ch) in line.chars().enumerate() {
        if index >= max_chars {
            truncated.push_str("...");
            break;
        }
        truncated.push(ch);
    }
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_line_replacement_shorthand_accepts_simple_two_line_patch() {
        let patch = "/foo\n+bar\n";
        let parsed = try_parse_line_replacement_shorthand(patch).expect("parse");
        assert_eq!(
            parsed,
            Some(LineReplacementShorthand {
                search: "foo".to_string(),
                replace: "bar".to_string(),
            })
        );
    }

    #[test]
    fn normalize_single_file_hunk_patch_rewrites_hunk_only_input() {
        let (normalized, is_single_file) =
            normalize_single_file_hunk_patch("src/lib.rs", "@@ -1 +1 @@\n-old\n+new\n")
                .expect("normalize");
        assert!(is_single_file);
        assert_eq!(
            normalized.as_deref(),
            Some("--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n")
        );
    }

    #[test]
    fn normalize_single_file_hunk_patch_rejects_newline_paths() {
        let error = normalize_single_file_hunk_patch(
            "src/lib.rs\n+++ b/other.rs",
            "@@ -1 +1 @@\n-old\n+new\n",
        )
        .expect_err("reject newline path");

        assert!(error.to_string().contains("cannot contain newlines"));
    }

    #[test]
    fn syntax_preflight_for_toml_reports_parser_acceptance() {
        let output = syntax_preflight_for_preview("Cargo.toml", "[package]\nname = \"demo\"\n");
        assert!(output.contains("syntax_preflight: passed"));
        assert!(output.contains("TOML parser accepted scratch content"));
    }

    #[test]
    fn syntax_preflight_for_non_rust_files_is_unavailable() {
        let output = syntax_preflight_for_preview("notes.txt", "hello");
        assert!(output.contains("syntax_preflight: unavailable"));
    }

    #[test]
    fn line_replacement_shorthand_updates_exact_line() {
        let mut content = "alpha\nbeta\n".to_string();
        let line_number =
            perform_line_replacement_shorthand(&mut content, "beta", "gamma").expect("replace");
        assert_eq!(line_number, 2);
        assert_eq!(content, "alpha\ngamma\n");
    }

    #[test]
    fn line_replacement_shorthand_rejects_missing_search() {
        let mut content = "alpha\nbeta\n".to_string();
        let error =
            perform_line_replacement_shorthand(&mut content, "delta", "gamma").expect_err("reject");
        assert!(error.to_string().contains("found no lines"));
    }

    #[test]
    fn preview_record_round_trips_in_cache() {
        let record = PreviewRecord {
            preview_id: String::new(),
            path: "src/lib.rs".to_string(),
            target_path: PathBuf::from("/tmp/quorp-test/src/lib.rs"),
            base_hash: "hash".to_string(),
            edit_kind: "replace_range".to_string(),
            updated_content: "new".to_string(),
            syntax_status: "passed".to_string(),
        };
        let preview_id = store_preview_record(record).expect("store");
        let loaded = load_preview_record(&preview_id).expect("load");
        assert_eq!(loaded.preview_id, preview_id);
        assert_eq!(loaded.path, "src/lib.rs");
    }
}

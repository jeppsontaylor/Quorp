use std::path::{Component, Path, PathBuf};

use quorp_agent_core::ReadFileRange;
use quorp_ids::PatchId;
use quorp_patch_vm::{
    FileChange, FileChangeKind, PatchApplyProof, PatchRisk, PatchVm, PatchVmPolicy, hash_bytes,
};
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockReplacementMatch {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
}

pub fn sanitize_project_path(
    project_root: &Path,
    cwd: &Path,
    request_path: &str,
) -> anyhow::Result<PathBuf> {
    if request_path.trim().is_empty() {
        return Err(anyhow::anyhow!("Path cannot be empty"));
    }

    let project_root = if project_root.as_os_str().is_empty() {
        cwd
    } else {
        project_root
    };
    let requested = Path::new(request_path);
    let request_relative = if requested.is_absolute() {
        if let Ok(stripped) = requested.strip_prefix(project_root) {
            stripped.to_path_buf()
        } else {
            let canonical_root = project_root
                .canonicalize()
                .unwrap_or_else(|_| project_root.to_path_buf());
            let canonical_requested_parent = requested
                .parent()
                .and_then(|parent| parent.canonicalize().ok());
            if let Some(canonical_parent) = canonical_requested_parent {
                if canonical_parent.starts_with(&canonical_root) {
                    requested
                        .strip_prefix(&canonical_root)
                        .map(Path::to_path_buf)
                        .map_err(|_| anyhow::anyhow!("Absolute paths are not allowed"))?
                } else {
                    return Err(anyhow::anyhow!("Absolute paths are not allowed"));
                }
            } else {
                return Err(anyhow::anyhow!("Absolute paths are not allowed"));
            }
        }
    } else {
        requested.to_path_buf()
    };
    let mut candidate = PathBuf::new();
    for component in request_relative.components() {
        match component {
            Component::Normal(part) => candidate.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(anyhow::anyhow!("Parent directory traversal is not allowed"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(anyhow::anyhow!("Absolute-like paths are not allowed"));
            }
        }
    }

    let resolved = project_root.join(candidate);
    if !resolved.starts_with(project_root) {
        return Err(anyhow::anyhow!("Path resolved outside project root"));
    }

    if !resolved.exists() {
        return Ok(resolved);
    }

    let canonical_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let canonical_target = resolved
        .canonicalize()
        .map_err(|error| anyhow::anyhow!("Failed to resolve target path: {error}"))?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(anyhow::anyhow!("Path resolved outside project root"));
    }
    Ok(canonical_target)
}

pub fn try_parse_search_replace_blocks(patch: &str) -> Option<Vec<(String, String)>> {
    let mut blocks = Vec::new();
    let mut current_search = String::new();
    let mut current_replace = String::new();
    let mut state = 0;

    for line in patch.split_inclusive('\n') {
        if line.starts_with("<<<<") {
            state = 1;
            current_search.clear();
            current_replace.clear();
            continue;
        } else if line.starts_with("====") && state == 1 {
            state = 2;
            continue;
        } else if line.starts_with(">>>>") && state == 2 {
            let search_trim = current_search
                .strip_suffix('\n')
                .unwrap_or(&current_search)
                .to_string();
            let replace_trim = current_replace
                .strip_suffix('\n')
                .unwrap_or(&current_replace)
                .to_string();
            blocks.push((search_trim, replace_trim));
            state = 0;
            continue;
        }

        if state == 1 {
            current_search.push_str(line);
        } else if state == 2 {
            current_replace.push_str(line);
        }
    }

    if blocks.is_empty() {
        None
    } else {
        Some(blocks)
    }
}

pub fn perform_block_replacement(
    current_content: &str,
    search_block: &str,
    replace_block: &str,
    range: Option<ReadFileRange>,
) -> anyhow::Result<String> {
    let first_result =
        perform_block_replacement_inner(current_content, search_block, replace_block, range);
    if first_result.is_ok() || !should_retry_literal_newline_block(search_block) {
        return first_result;
    }

    let normalized_search_block = search_block.replace("\\n", "\n");
    let normalized_replace_block = if should_retry_literal_newline_block(replace_block) {
        replace_block.replace("\\n", "\n")
    } else {
        replace_block.to_string()
    };
    perform_block_replacement_inner(
        current_content,
        &normalized_search_block,
        &normalized_replace_block,
        range,
    )
    .map_err(|normalized_error| {
        anyhow::anyhow!(
            "Could not apply ReplaceBlock as written, and retrying literal \\n as line breaks also failed: {normalized_error}"
        )
    })
}

pub fn preview_block_replacement_matches(
    current_content: &str,
    search_block: &str,
    range: Option<ReadFileRange>,
) -> Vec<BlockReplacementMatch> {
    let normalized_range = range.and_then(ReadFileRange::normalized);
    let exact_matches = filter_matches_by_range(
        &exact_block_matches(current_content, search_block),
        normalized_range,
    );
    if !exact_matches.is_empty() {
        return exact_matches;
    }

    let search_lines = search_block.lines().collect::<Vec<_>>();
    let content_lines = current_content.lines().collect::<Vec<_>>();
    if search_lines.is_empty() || search_lines.len() > content_lines.len() {
        return Vec::new();
    }

    let line_spans = line_spans(current_content);
    let mut fuzzy_matches = Vec::new();
    for index in 0..=(content_lines.len() - search_lines.len()) {
        let matched = search_lines
            .iter()
            .enumerate()
            .all(|(search_index, line)| content_lines[index + search_index].trim() == line.trim());
        if matched {
            let end_index = index + search_lines.len().saturating_sub(1);
            if let (Some(start), Some(end)) = (line_spans.get(index), line_spans.get(end_index)) {
                fuzzy_matches.push(BlockReplacementMatch {
                    start_byte: start.0,
                    end_byte: end.1,
                    start_line: index + 1,
                    end_line: end_index + 1,
                });
            }
        }
    }
    filter_matches_by_range(&fuzzy_matches, normalized_range)
}

pub fn format_preview_match_lines(matches: &[BlockReplacementMatch]) -> String {
    if matches.is_empty() {
        return "none".to_string();
    }
    matches
        .iter()
        .map(|candidate| {
            if candidate.start_line == candidate.end_line {
                candidate.start_line.to_string()
            } else {
                format!("{}-{}", candidate.start_line, candidate.end_line)
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn perform_block_replacement_inner(
    current_content: &str,
    search_block: &str,
    replace_block: &str,
    range: Option<ReadFileRange>,
) -> anyhow::Result<String> {
    if current_content.is_empty() {
        if search_block.trim().is_empty() {
            return Ok(replace_block.to_string());
        }
        return Err(anyhow::anyhow!("File is empty but search block is not"));
    }

    let normalized_range = range.and_then(ReadFileRange::normalized);
    let exact_matches = exact_block_matches(current_content, search_block);
    let exact_candidates = filter_matches_by_range(&exact_matches, normalized_range);
    if let Some(candidate) = unique_replacement_candidate(
        "Search block",
        &exact_matches,
        &exact_candidates,
        normalized_range,
    )? {
        return Ok(replace_block_match(
            current_content,
            candidate,
            replace_block,
        ));
    }

    let search_lines: Vec<&str> = search_block.lines().collect();
    let content_lines: Vec<&str> = current_content.lines().collect();

    if search_lines.is_empty() {
        return Err(anyhow::anyhow!("Search block is empty"));
    }

    let mut fuzzy_matches = Vec::new();

    if search_lines.len() <= content_lines.len() {
        let line_spans = line_spans(current_content);
        for index in 0..=(content_lines.len() - search_lines.len()) {
            let mut matched = true;
            for search_index in 0..search_lines.len() {
                if content_lines[index + search_index].trim() != search_lines[search_index].trim() {
                    matched = false;
                    break;
                }
            }
            if matched {
                let end_index = index + search_lines.len().saturating_sub(1);
                if let (Some(start), Some(end)) = (line_spans.get(index), line_spans.get(end_index))
                {
                    fuzzy_matches.push(BlockReplacementMatch {
                        start_byte: start.0,
                        end_byte: end.1,
                        start_line: index + 1,
                        end_line: end_index + 1,
                    });
                }
            }
        }
    }
    let fuzzy_candidates = filter_matches_by_range(&fuzzy_matches, normalized_range);

    if let Some(candidate) = unique_replacement_candidate(
        "Search block",
        &fuzzy_matches,
        &fuzzy_candidates,
        normalized_range,
    )? {
        return Ok(replace_block_match(
            current_content,
            candidate,
            replace_block,
        ));
    }

    if fuzzy_matches.is_empty() {
        return Err(anyhow::anyhow!(
            "Could not find the search block in the file (even ignoring whitespace)"
        ));
    }
    Err(anyhow::anyhow!(
        "Search block is ambiguous; found {} matches at lines {}. Include enough surrounding context to make the search block unique, add a `range`, or use ApplyPatch/WriteFile.",
        fuzzy_matches.len(),
        format_match_lines(&fuzzy_matches)
    ))
}

fn should_retry_literal_newline_block(block: &str) -> bool {
    block.contains("\\n") && !block.contains('\n')
}

fn exact_block_matches(current_content: &str, search_block: &str) -> Vec<BlockReplacementMatch> {
    if search_block.is_empty() {
        return Vec::new();
    }
    let line_count = search_block.lines().count().max(1);
    current_content
        .match_indices(search_block)
        .map(|(start_byte, matched)| {
            let start_line = byte_line_number(current_content, start_byte);
            BlockReplacementMatch {
                start_byte,
                end_byte: start_byte + matched.len(),
                start_line,
                end_line: start_line + line_count.saturating_sub(1),
            }
        })
        .collect()
}

fn filter_matches_by_range(
    matches: &[BlockReplacementMatch],
    range: Option<ReadFileRange>,
) -> Vec<BlockReplacementMatch> {
    let Some(range) = range else {
        return matches.to_vec();
    };
    matches
        .iter()
        .copied()
        .filter(|candidate| {
            candidate.start_line >= range.start_line && candidate.end_line <= range.end_line
        })
        .collect()
}

fn unique_replacement_candidate(
    label: &str,
    all_matches: &[BlockReplacementMatch],
    candidates: &[BlockReplacementMatch],
    range: Option<ReadFileRange>,
) -> anyhow::Result<Option<BlockReplacementMatch>> {
    match candidates {
        [] => {
            if let Some(range) = range
                && !all_matches.is_empty()
            {
                return Err(anyhow::anyhow!(
                    "{label} has {} matches at lines {}, but none are fully inside requested range {}. Reread the file or provide a fresh range before patching.",
                    all_matches.len(),
                    format_match_lines(all_matches),
                    range.label()
                ));
            }
            Ok(None)
        }
        [candidate] => Ok(Some(*candidate)),
        _ => {
            let range_note = range
                .map(|range| format!(" inside requested range {}", range.label()))
                .unwrap_or_default();
            Err(anyhow::anyhow!(
                "{label} is ambiguous; found {} matches{} at lines {}. Include enough surrounding context to make the search block unique, use a narrower `range`, or use ApplyPatch/WriteFile.",
                candidates.len(),
                range_note,
                format_match_lines(candidates)
            ))
        }
    }
}

fn replace_block_match(
    current_content: &str,
    candidate: BlockReplacementMatch,
    replace_block: &str,
) -> String {
    let mut out = String::with_capacity(
        current_content
            .len()
            .saturating_sub(candidate.end_byte.saturating_sub(candidate.start_byte))
            .saturating_add(replace_block.len())
            .saturating_add(1),
    );
    out.push_str(&current_content[..candidate.start_byte]);
    out.push_str(replace_block);
    if !replace_block.ends_with('\n') && !replace_block.is_empty() {
        out.push('\n');
    }
    out.push_str(&current_content[candidate.end_byte..]);
    out
}

fn byte_line_number(text: &str, byte_index: usize) -> usize {
    text[..byte_index.min(text.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn line_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0usize;
    for line in text.split_inclusive('\n') {
        let end = start + line.len();
        spans.push((start, end));
        start = end;
    }
    if !text.ends_with('\n') && start < text.len() {
        spans.push((start, text.len()));
    }
    spans
}

fn format_match_lines(matches: &[BlockReplacementMatch]) -> String {
    let mut rendered = matches
        .iter()
        .take(8)
        .map(|candidate| candidate.start_line.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if matches.len() > 8 {
        rendered.push_str(", ...");
    }
    rendered
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum PatchOperation {
    Add,
    Update,
    Delete,
    Move { move_path: String },
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FilePatch {
    pub path: String,
    pub operation: PatchOperation,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Hunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub lines: Vec<PatchLine>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum PatchLine {
    Context(String),
    Remove(String),
    Add(String),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ResolvedFilePatch {
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub display_path: String,
    pub operation: PatchOperation,
    pub new_content: Option<String>,
}

pub fn parse_multi_file_patch(patch_text: &str) -> anyhow::Result<Vec<FilePatch>> {
    let normalized = patch_text.replace("\r\n", "\n").replace('\r', "\n");
    if normalized.trim().is_empty() {
        return Ok(Vec::new());
    }
    if normalized.trim().starts_with("*** Begin Patch") {
        return parse_model_patch(&normalized);
    }

    let file_header = Regex::new(r"^---\s+(?:a/)?(.+?)(?:\s+\d{4}-\d{2}-\d{2}.*)?$")
        .map_err(|error| anyhow::anyhow!("Failed to compile file header regex: {error}"))?;
    let new_file_header = Regex::new(r"^\+\+\+\s+(?:b/)?(.+?)(?:\s+\d{4}-\d{2}-\d{2}.*)?$")
        .map_err(|error| anyhow::anyhow!("Failed to compile new file header regex: {error}"))?;
    let hunk_header = Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")
        .map_err(|error| anyhow::anyhow!("Failed to compile hunk header regex: {error}"))?;
    let rename_from_re = Regex::new(r"^rename from (.+)$")
        .map_err(|error| anyhow::anyhow!("Failed to compile rename-from regex: {error}"))?;
    let rename_to_re = Regex::new(r"^rename to (.+)$")
        .map_err(|error| anyhow::anyhow!("Failed to compile rename-to regex: {error}"))?;

    let mut file_patches = Vec::new();
    let mut current_file: Option<FilePatch> = None;
    let mut current_hunk: Option<Hunk> = None;
    let mut old_path: Option<String> = None;
    let mut rename_from: Option<String> = None;
    let mut rename_to: Option<String> = None;

    for line in normalized.lines() {
        if line.starts_with("diff --git ") || line.starts_with("diff -") {
            if let Some(mut file) = current_file.take() {
                if let Some(hunk) = current_hunk.take() {
                    file.hunks.push(hunk);
                }
                if !file.hunks.is_empty() || matches!(file.operation, PatchOperation::Delete) {
                    file_patches.push(file);
                }
            }
            old_path = None;
            rename_from = None;
            rename_to = None;
            continue;
        }

        if let Some(caps) = rename_from_re.captures(line) {
            rename_from = caps.get(1).map(|value| value.as_str().to_string());
            continue;
        }
        if let Some(caps) = rename_to_re.captures(line) {
            rename_to = caps.get(1).map(|value| value.as_str().to_string());
            continue;
        }

        if let Some(caps) = file_header.captures(line) {
            if let Some(file) = current_file.as_mut()
                && let Some(hunk) = current_hunk.take()
            {
                file.hunks.push(hunk);
            }
            old_path = caps.get(1).map(|value| value.as_str().to_string());
            continue;
        }

        if let Some(caps) = new_file_header.captures(line) {
            if let Some(mut file) = current_file.take() {
                if let Some(hunk) = current_hunk.take() {
                    file.hunks.push(hunk);
                }
                if !file.hunks.is_empty() || matches!(file.operation, PatchOperation::Delete) {
                    file_patches.push(file);
                }
            }

            let new_path = caps
                .get(1)
                .map(|value| value.as_str().to_string())
                .unwrap_or_default();
            let (path, operation) = if let (Some(rename_from), Some(rename_to)) =
                (rename_from.clone(), rename_to.clone())
            {
                (
                    rename_from,
                    PatchOperation::Move {
                        move_path: rename_to,
                    },
                )
            } else if old_path.as_deref() == Some("/dev/null") {
                (new_path.clone(), PatchOperation::Add)
            } else if new_path == "/dev/null" {
                (old_path.clone().unwrap_or_default(), PatchOperation::Delete)
            } else {
                (
                    old_path.clone().unwrap_or_else(|| new_path.clone()),
                    PatchOperation::Update,
                )
            };

            if path.is_empty() || path == "/dev/null" {
                continue;
            }

            current_file = Some(FilePatch {
                path,
                operation,
                hunks: Vec::new(),
            });
            continue;
        }

        if let Some(caps) = hunk_header.captures(line) {
            let Some(file) = current_file.as_mut() else {
                return Err(anyhow::anyhow!(
                    "Found a hunk header before a file header in apply_patch input"
                ));
            };
            if let Some(hunk) = current_hunk.take() {
                file.hunks.push(hunk);
            }
            let old_start = caps
                .get(1)
                .and_then(|value| value.as_str().parse::<usize>().ok())
                .unwrap_or(1);
            let old_count = caps
                .get(2)
                .and_then(|value| value.as_str().parse::<usize>().ok())
                .unwrap_or(1);
            let new_start = caps
                .get(3)
                .and_then(|value| value.as_str().parse::<usize>().ok())
                .unwrap_or(1);
            let new_count = caps
                .get(4)
                .and_then(|value| value.as_str().parse::<usize>().ok())
                .unwrap_or(1);
            current_hunk = Some(Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
                lines: Vec::new(),
            });
            continue;
        }

        if let Some(hunk) = current_hunk.as_mut() {
            if let Some(stripped) = line.strip_prefix(' ') {
                hunk.lines.push(PatchLine::Context(stripped.to_string()));
            } else if let Some(stripped) = (!line.starts_with("--- "))
                .then(|| line.strip_prefix('-'))
                .flatten()
            {
                hunk.lines.push(PatchLine::Remove(stripped.to_string()));
            } else if let Some(stripped) = (!line.starts_with("+++ "))
                .then(|| line.strip_prefix('+'))
                .flatten()
            {
                hunk.lines.push(PatchLine::Add(stripped.to_string()));
            } else if line.starts_with('\\') {
                continue;
            }
        }
    }

    if let Some(mut file) = current_file {
        if let Some(hunk) = current_hunk {
            file.hunks.push(hunk);
        }
        if !file.hunks.is_empty() || matches!(file.operation, PatchOperation::Delete) {
            file_patches.push(file);
        }
    }

    Ok(file_patches)
}

pub fn parse_model_patch(patch_text: &str) -> anyhow::Result<Vec<FilePatch>> {
    let begin_file = Regex::new(r"^\*\*\*\s+Begin\s+File:\s+(.+)$")
        .map_err(|error| anyhow::anyhow!("Failed to compile model patch regex: {error}"))?;
    let delete_file = Regex::new(r"^\*\*\*\s+Delete\s+File:\s+(.+)$")
        .map_err(|error| anyhow::anyhow!("Failed to compile delete regex: {error}"))?;
    let end_patch = Regex::new(r"^\*\*\*\s+End\s+Patch$")
        .map_err(|error| anyhow::anyhow!("Failed to compile end regex: {error}"))?;
    let move_to = Regex::new(r"^\*\*\*\s+Move\s+To:\s+(.+)$")
        .map_err(|error| anyhow::anyhow!("Failed to compile move regex: {error}"))?;

    let mut file_patches = Vec::new();
    let mut current_file: Option<FilePatch> = None;
    let mut content_lines = Vec::new();

    for line in patch_text.lines() {
        if line == "*** Begin Patch" {
            continue;
        }
        if let Some(caps) = delete_file.captures(line) {
            let path = caps
                .get(1)
                .map(|value| value.as_str().trim().to_string())
                .unwrap_or_default();
            if !path.is_empty() {
                file_patches.push(FilePatch {
                    path,
                    operation: PatchOperation::Delete,
                    hunks: Vec::new(),
                });
            }
            current_file = None;
            content_lines.clear();
            continue;
        }
        if let Some(caps) = move_to.captures(line) {
            if let Some(file) = current_file.as_mut()
                && let Some(target) = caps.get(1).map(|value| value.as_str().trim())
            {
                file.operation = PatchOperation::Move {
                    move_path: target.to_string(),
                };
            }
            continue;
        }
        if let Some(caps) = begin_file.captures(line) {
            current_file = Some(FilePatch {
                path: caps
                    .get(1)
                    .map(|value| value.as_str().trim().to_string())
                    .unwrap_or_default(),
                operation: PatchOperation::Add,
                hunks: Vec::new(),
            });
            content_lines.clear();
            continue;
        }
        if line == "*** End File" {
            if let Some(mut file) = current_file.take() {
                let hunk = Hunk {
                    old_start: 1,
                    old_count: 0,
                    new_start: 1,
                    new_count: content_lines.len(),
                    lines: content_lines.drain(..).map(PatchLine::Add).collect(),
                };
                file.hunks.push(hunk);
                file_patches.push(file);
            }
            continue;
        }
        if end_patch.is_match(line) {
            break;
        }
        if current_file.is_some() {
            content_lines.push(line.to_string());
        }
    }

    Ok(file_patches)
}

pub fn resolve_file_patches(
    project_root: &Path,
    cwd: &Path,
    file_patches: &[FilePatch],
) -> anyhow::Result<Vec<ResolvedFilePatch>> {
    let mut resolved = Vec::with_capacity(file_patches.len());
    for file_patch in file_patches {
        let source_path = sanitize_project_path(project_root, cwd, &file_patch.path)?;
        let target_path = match &file_patch.operation {
            PatchOperation::Move { move_path } => {
                sanitize_project_path(project_root, cwd, move_path)?
            }
            _ => source_path.clone(),
        };
        let new_content = match &file_patch.operation {
            PatchOperation::Add => Some(render_added_file_content(&file_patch.hunks)),
            PatchOperation::Update | PatchOperation::Move { .. } => {
                let current_content = std::fs::read_to_string(&source_path)
                    .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
                Some(apply_hunks(&current_content, &file_patch.hunks)?)
            }
            PatchOperation::Delete => None,
        };
        resolved.push(ResolvedFilePatch {
            source_path,
            target_path,
            display_path: file_patch.path.clone(),
            operation: file_patch.operation.clone(),
            new_content,
        });
    }
    Ok(resolved)
}

pub fn apply_resolved_file_patches(resolved: &[ResolvedFilePatch]) -> anyhow::Result<()> {
    let changes = resolved_file_patches_to_vm_changes(resolved)?;
    if changes.is_empty() {
        return Ok(());
    }
    let vm = PatchVm::new();
    let patch_id = patch_id_for_changes(&changes);
    let policy = PatchVmPolicy::default();
    let preview = vm.preview_file_changes(&patch_id, &changes, policy)?;
    let proof = if preview.risk == PatchRisk::High {
        PatchApplyProof::PreviewId(&preview.preview_id)
    } else {
        PatchApplyProof::HashesOnly
    };
    vm.apply_file_changes(&patch_id, &changes, proof, policy)?;
    Ok(())
}

fn resolved_file_patches_to_vm_changes(
    resolved: &[ResolvedFilePatch],
) -> anyhow::Result<Vec<FileChange>> {
    let mut changes = Vec::with_capacity(resolved.len());
    for patch in resolved {
        match &patch.operation {
            PatchOperation::Add => {
                let content = patch
                    .new_content
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("apply_patch resolved add without content"))?;
                changes.push(FileChange {
                    path: patch.target_path.clone(),
                    display_path: patch.display_path.clone(),
                    expected_hash: None,
                    kind: FileChangeKind::Add {
                        content: content.as_bytes().to_vec(),
                    },
                });
            }
            PatchOperation::Update => {
                let content = patch.new_content.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("apply_patch resolved update without content")
                })?;
                let current_bytes = std::fs::read(&patch.source_path)
                    .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
                changes.push(FileChange {
                    path: patch.source_path.clone(),
                    display_path: patch.display_path.clone(),
                    expected_hash: Some(hash_bytes(&current_bytes)),
                    kind: FileChangeKind::Update {
                        content: content.as_bytes().to_vec(),
                    },
                });
            }
            PatchOperation::Delete => {
                if !patch.source_path.exists() {
                    continue;
                }
                let current_bytes = std::fs::read(&patch.source_path)
                    .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
                changes.push(FileChange {
                    path: patch.source_path.clone(),
                    display_path: patch.display_path.clone(),
                    expected_hash: Some(hash_bytes(&current_bytes)),
                    kind: FileChangeKind::Delete,
                });
            }
            PatchOperation::Move { .. } => {
                let content = patch.new_content.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("apply_patch resolved move without new content")
                })?;
                let current_bytes = std::fs::read(&patch.source_path)
                    .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
                changes.push(FileChange {
                    path: patch.source_path.clone(),
                    display_path: patch.display_path.clone(),
                    expected_hash: Some(hash_bytes(&current_bytes)),
                    kind: FileChangeKind::Move {
                        target: patch.target_path.clone(),
                        content: content.as_bytes().to_vec(),
                    },
                });
            }
        }
    }
    Ok(changes)
}

fn patch_id_for_changes(changes: &[FileChange]) -> PatchId {
    let mut bytes = Vec::new();
    for change in changes {
        bytes.extend_from_slice(change.display_path.as_bytes());
        bytes.push(0);
        match &change.kind {
            FileChangeKind::Add { content } | FileChangeKind::Update { content } => {
                bytes.extend_from_slice(hash_bytes(content).0.as_bytes());
            }
            FileChangeKind::Delete => bytes.extend_from_slice(b"delete"),
            FileChangeKind::Move { target, content } => {
                bytes.extend_from_slice(target.to_string_lossy().as_bytes());
                bytes.push(0);
                bytes.extend_from_slice(hash_bytes(content).0.as_bytes());
            }
        }
    }
    PatchId::new(format!("patch-vm-{}", hash_bytes(&bytes).0))
}

fn render_added_file_content(hunks: &[Hunk]) -> String {
    let lines = hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter_map(|line| match line {
            PatchLine::Add(text) => Some(text.clone()),
            PatchLine::Context(_) | PatchLine::Remove(_) => None,
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        String::new()
    } else {
        let mut content = lines.join("\n");
        content.push('\n');
        content
    }
}

fn apply_hunks(original: &str, hunks: &[Hunk]) -> anyhow::Result<String> {
    let had_trailing_newline = original.ends_with('\n');
    let mut lines = original.lines().map(str::to_string).collect::<Vec<_>>();
    let mut offset = 0isize;

    for hunk in hunks {
        let expected_old = hunk
            .lines
            .iter()
            .filter_map(|line| match line {
                PatchLine::Context(text) | PatchLine::Remove(text) => Some(text.clone()),
                PatchLine::Add(_) => None,
            })
            .collect::<Vec<_>>();
        let replacement = hunk
            .lines
            .iter()
            .filter_map(|line| match line {
                PatchLine::Context(text) | PatchLine::Add(text) => Some(text.clone()),
                PatchLine::Remove(_) => None,
            })
            .collect::<Vec<_>>();

        let expected_old_count = hunk
            .lines
            .iter()
            .filter(|line| matches!(line, PatchLine::Context(_) | PatchLine::Remove(_)))
            .count();
        let expected_new_count = hunk
            .lines
            .iter()
            .filter(|line| matches!(line, PatchLine::Context(_) | PatchLine::Add(_)))
            .count();
        if hunk.old_count > 0 && expected_old_count != hunk.old_count {
            return Err(anyhow::anyhow!(
                "Malformed hunk for line {}: expected {} old lines but found {}",
                hunk.old_start,
                hunk.old_count,
                expected_old_count
            ));
        }
        if hunk.new_count > 0 && expected_new_count != hunk.new_count {
            return Err(anyhow::anyhow!(
                "Malformed hunk for line {}: expected {} new lines but found {}",
                hunk.new_start,
                hunk.new_count,
                expected_new_count
            ));
        }

        let preferred_index = (hunk.old_start as isize + offset - 1).max(0) as usize;
        let start_index = if exact_line_match(&lines, preferred_index, &expected_old) {
            preferred_index
        } else if expected_old.is_empty() {
            preferred_index.min(lines.len())
        } else {
            let matches = find_exact_hunk_matches(&lines, &expected_old);
            match matches.as_slice() {
                [index] => *index,
                [] => {
                    return Err(anyhow::anyhow!(
                        "Could not locate hunk context for {}",
                        hunk.old_start
                    ));
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Patch hunk is ambiguous for {}",
                        hunk.old_start
                    ));
                }
            }
        };

        let range_end = start_index
            .saturating_add(expected_old.len())
            .min(lines.len());
        lines.splice(start_index..range_end, replacement.clone());
        offset += replacement.len() as isize - expected_old.len() as isize;
    }

    let mut rendered = lines.join("\n");
    if had_trailing_newline && !rendered.is_empty() {
        rendered.push('\n');
    }
    Ok(rendered)
}

fn exact_line_match(lines: &[String], start: usize, expected: &[String]) -> bool {
    if start > lines.len() || start.saturating_add(expected.len()) > lines.len() {
        return false;
    }
    lines[start..start + expected.len()]
        .iter()
        .zip(expected)
        .all(|(actual, expected)| actual == expected)
}

fn find_exact_hunk_matches(lines: &[String], expected: &[String]) -> Vec<usize> {
    if expected.is_empty() {
        return vec![lines.len()];
    }
    if expected.len() > lines.len() {
        return Vec::new();
    }
    (0..=lines.len() - expected.len())
        .filter(|index| exact_line_match(lines, *index, expected))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn sanitize_project_path_rejects_traversal_and_external_absolute() {
        let root = tempdir().expect("tempdir");
        let outside = tempdir().expect("outside");
        let file = outside.path().join("secret");
        std::fs::write(&file, "x").expect("write");

        assert!(sanitize_project_path(root.path(), root.path(), "../outside").is_err());
        assert!(sanitize_project_path(root.path(), root.path(), &file.to_string_lossy()).is_err());
    }

    #[test]
    fn sanitize_project_path_allows_relative_in_root() {
        let root = tempdir().expect("tempdir");
        let candidate =
            sanitize_project_path(root.path(), root.path(), "src/main.rs").expect("sanitized");
        assert_eq!(candidate, root.path().join("src/main.rs"));
    }

    #[test]
    fn sanitize_project_path_allows_absolute_paths_inside_root() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("src").join("main.rs");
        std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        std::fs::write(&file, "fn main() {}\n").expect("write");

        let candidate = sanitize_project_path(root.path(), root.path(), &file.to_string_lossy())
            .expect("sanitized");
        assert_eq!(candidate, file.canonicalize().expect("canonical"));
    }

    #[test]
    fn test_perform_block_replacement_exact_match() {
        let current = "line 1\nline 2\nline 3\nline 4\n";
        let search = "line 2\nline 3\n";
        let replace = "line 2 modified\nline 3 modified\n";
        let result = perform_block_replacement(current, search, replace, None).unwrap();
        assert_eq!(result, "line 1\nline 2 modified\nline 3 modified\nline 4\n");
    }

    #[test]
    fn test_perform_block_replacement_fuzzy_trailing_whitespace() {
        let current = "fn foo() {\n    let x = 1; \n    let y = 2;\n}\n";
        let search = "    let x = 1;\n    let y = 2;";
        let replace = "    let x = 100;\n    let y = 200;";
        let result = perform_block_replacement(current, search, replace, None).unwrap();
        assert_eq!(
            result,
            "fn foo() {\n    let x = 100;\n    let y = 200;\n}\n"
        );
    }

    #[test]
    fn test_perform_block_replacement_ambiguous() {
        let current = "a\nb\nc\nb\nd\n";
        let search = "b\n";
        let replace = "x\n";
        let err = perform_block_replacement(current, search, replace, None).unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
        assert!(err.to_string().contains("lines 2, 4"));
    }

    #[test]
    fn test_perform_block_replacement_not_found() {
        let current = "a\nb\nc\n";
        let search = "d\n";
        let replace = "x\n";
        let err = perform_block_replacement(current, search, replace, None).unwrap_err();
        assert!(err.to_string().contains("Could not find"));
    }

    #[test]
    fn test_try_parse_search_replace_blocks() {
        let patch = "\
Here is my patch!
<<<<
fn foo() {
====
fn foo(bar: i32) {
>>>>
Done.";
        let blocks = try_parse_search_replace_blocks(patch).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].0, "fn foo() {");
        assert_eq!(blocks[0].1, "fn foo(bar: i32) {");
    }

    #[test]
    fn test_perform_block_replacement_fuzzy_leading_whitespace() {
        let current = "fn foo() {\n    let x = 1;\n    let y = 2;\n}\n";
        let search = "let x = 1;\nlet y = 2;";
        let replace = "    let x = 100;\n    let y = 200;";
        let result = perform_block_replacement(current, search, replace, None).unwrap();
        assert_eq!(
            result,
            "fn foo() {\n    let x = 100;\n    let y = 200;\n}\n"
        );
    }

    #[test]
    fn test_perform_block_replacement_ranged_disambiguates() {
        let current = "a\nb\nc\nb\nd\n";
        let search = "b\n";
        let replace = "x\n";
        let result = perform_block_replacement(
            current,
            search,
            replace,
            Some(ReadFileRange {
                start_line: 4,
                end_line: 4,
            }),
        )
        .unwrap();
        assert_eq!(result, "a\nb\nc\nx\nd\n");
    }

    #[test]
    fn test_perform_block_replacement_ranged_stale_range_fails() {
        let current = "a\nb\nc\nb\nd\n";
        let search = "b\n";
        let replace = "x\n";
        let err = perform_block_replacement(
            current,
            search,
            replace,
            Some(ReadFileRange {
                start_line: 5,
                end_line: 5,
            }),
        )
        .unwrap_err();
        assert!(err.to_string().contains("none are fully inside"));
        assert!(err.to_string().contains("lines 2, 4"));
    }

    #[test]
    fn test_perform_block_replacement_ranged_still_ambiguous_fails() {
        let current = "a\nb\nc\nb\nd\n";
        let search = "b\n";
        let replace = "x\n";
        let err = perform_block_replacement(
            current,
            search,
            replace,
            Some(ReadFileRange {
                start_line: 1,
                end_line: 5,
            }),
        )
        .unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
        assert!(err.to_string().contains("requested range 1-5"));
    }

    #[test]
    fn test_perform_block_replacement_accepts_literal_newline_escape_fallback() {
        let current = "pub fn preview_change_reason(delayed_change: bool) -> &'static str {\n    if delayed_change {\n        \"immediate\"\n    } else {\n        \"immediate\"\n    }\n}\n";
        let search = "if delayed_change {\\n        \"immediate\"\\n    } else {\\n        \"immediate\"\\n    }";
        let replace = "if delayed_change {\\n        \"scheduled_at_period_end\"\\n    } else {\\n        \"immediate\"\\n    }";
        let result = perform_block_replacement(
            current,
            search,
            replace,
            Some(ReadFileRange {
                start_line: 1,
                end_line: 7,
            }),
        )
        .unwrap();

        assert!(result.contains("\"scheduled_at_period_end\""));
        assert!(!result.contains("\\n"));
    }

    #[test]
    fn resolve_file_patches_rejects_out_of_root_targets() {
        let root = tempdir().expect("tempdir");
        let file_patches = vec![FilePatch {
            path: "../escape.txt".to_string(),
            operation: PatchOperation::Update,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                lines: vec![PatchLine::Context("safe".to_string())],
            }],
        }];

        let error =
            resolve_file_patches(root.path(), root.path(), &file_patches).expect_err("reject");
        assert!(
            error
                .to_string()
                .contains("Parent directory traversal is not allowed")
        );
    }

    #[test]
    fn apply_hunks_rejects_ambiguous_matches() {
        let error = apply_hunks(
            "same\nline\nsame\nline",
            &[Hunk {
                old_start: 99,
                old_count: 2,
                new_start: 99,
                new_count: 2,
                lines: vec![
                    PatchLine::Context("same".to_string()),
                    PatchLine::Remove("line".to_string()),
                    PatchLine::Add("updated".to_string()),
                ],
            }],
        )
        .expect_err("ambiguous hunk");

        assert!(error.to_string().contains("Patch hunk is ambiguous"));
    }

    #[test]
    fn apply_hunks_rejects_malformed_line_counts() {
        let error = apply_hunks(
            "old",
            &[Hunk {
                old_start: 1,
                old_count: 2,
                new_start: 1,
                new_count: 1,
                lines: vec![PatchLine::Remove("old".to_string())],
            }],
        )
        .expect_err("malformed hunk");

        assert!(error.to_string().contains("Malformed hunk"));
    }
}

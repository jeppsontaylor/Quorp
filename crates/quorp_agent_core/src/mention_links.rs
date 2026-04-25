use std::path::{Path, PathBuf};

use url::Url;

use crate::path_guard::path_within_project;

const MAX_FILE_BYTES: usize = 512 * 1024;
const MAX_DIR_ENTRIES: usize = 200;

fn find_matching_bracket(text: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 0;
    for (index, character) in text.char_indices() {
        if character == open {
            depth += 1;
        } else if character == close {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

pub fn file_uri_to_project_path(uri: &str, project_root: &Path) -> Option<PathBuf> {
    let url = Url::parse(uri).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let path = url.to_file_path().ok()?;
    if path_within_project(&path, project_root) {
        Some(path)
    } else {
        None
    }
}

pub fn mention_link_for_path(abs_path: &Path, label: &str) -> Result<String, String> {
    let is_dir = abs_path.is_dir();
    let url = if is_dir {
        Url::from_directory_path(abs_path)
            .map_err(|_| "invalid directory path for URL".to_string())?
    } else {
        Url::from_file_path(abs_path).map_err(|_| "invalid file path for URL".to_string())?
    };
    let mut rendered = url.to_string();
    if is_dir && !rendered.ends_with('/') {
        rendered.push('/');
    }
    Ok(format!("[@{label}]({rendered})"))
}

pub fn collect_file_mention_uris(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut search_start = 0usize;
    while let Some(relative_start) = text[search_start..].find("[@") {
        let absolute_start = search_start + relative_start;
        let Some(name_end_rel) = find_matching_bracket(&text[absolute_start..], '[', ']') else {
            search_start = absolute_start + 2;
            continue;
        };
        let name_end = absolute_start + name_end_rel;
        if text.get(name_end + 1..name_end + 2) != Some("(") {
            search_start = name_end + 1;
            continue;
        }
        let uri_start = name_end + 2;
        let Some(uri_end_rel) = find_matching_bracket(&text[name_end + 1..], '(', ')') else {
            search_start = uri_start;
            continue;
        };
        let uri_end = name_end + 1 + uri_end_rel;
        let uri = text[uri_start..uri_end].to_string();
        if uri.starts_with("file:") {
            out.push(uri);
        }
        search_start = uri_end + 1;
    }
    out
}

pub fn strip_file_mention_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut search_start = 0usize;
    while search_start < text.len() {
        let Some(relative_start) = text[search_start..].find("[@") else {
            out.push_str(&text[search_start..]);
            break;
        };
        let absolute_start = search_start + relative_start;
        out.push_str(&text[search_start..absolute_start]);
        let Some(name_end_rel) = find_matching_bracket(&text[absolute_start..], '[', ']') else {
            out.push_str("[@");
            search_start = absolute_start + 2;
            continue;
        };
        let name_end = absolute_start + name_end_rel;
        if text.get(name_end + 1..name_end + 2) != Some("(") {
            out.push_str(&text[absolute_start..=name_end]);
            search_start = name_end + 1;
            continue;
        }
        let uri_start = name_end + 2;
        let Some(uri_end_rel) = find_matching_bracket(&text[name_end + 1..], '(', ')') else {
            out.push_str(&text[absolute_start..uri_start.min(text.len())]);
            search_start = uri_start;
            continue;
        };
        let uri_end = name_end + 1 + uri_end_rel;
        let uri = &text[uri_start..uri_end];
        if uri.starts_with("file:") {
            if !out.is_empty()
                && !out
                    .chars()
                    .last()
                    .is_some_and(|character| character.is_whitespace())
            {
                out.push(' ');
            }
            search_start = uri_end + 1;
        } else {
            out.push_str(&text[absolute_start..=uri_end]);
            search_start = uri_end + 1;
        }
    }
    out.trim().to_string()
}

fn read_file_limited(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let slice = if bytes.len() > MAX_FILE_BYTES {
        &bytes[..MAX_FILE_BYTES]
    } else {
        bytes.as_slice()
    };
    let mut rendered = String::from_utf8_lossy(slice).into_owned();
    if bytes.len() > MAX_FILE_BYTES {
        rendered.push_str("\n… [truncated: file exceeds mention limit]");
    }
    Ok(rendered)
}

fn list_directory_limited(path: &Path, project_root: &Path) -> Result<String, String> {
    let mut builder = ignore::WalkBuilder::new(path);
    builder.standard_filters(true);
    builder.max_depth(Some(2));
    let mut lines = Vec::new();
    for walk_result in builder.build().flatten() {
        let entry_path = walk_result.path();
        if entry_path == path {
            continue;
        }
        if !path_within_project(entry_path, project_root) {
            continue;
        }
        let relative = entry_path.strip_prefix(path).unwrap_or(entry_path);
        let label = if walk_result
            .file_type()
            .is_some_and(|file_type| file_type.is_dir())
        {
            format!("{}/", relative.display())
        } else {
            relative.display().to_string()
        };
        lines.push(label);
        if lines.len() >= MAX_DIR_ENTRIES {
            lines.push("… [truncated]".to_string());
            break;
        }
    }
    lines.sort();
    Ok(lines.join("\n"))
}

pub fn expand_mentions_for_api_message(trimmed_user_text: &str, project_root: &Path) -> String {
    let uris = collect_file_mention_uris(trimmed_user_text);
    if uris.is_empty() {
        return trimmed_user_text.to_string();
    }

    let mut attachments = String::new();
    let mut seen = std::collections::HashSet::<PathBuf>::new();
    for uri in uris {
        let Some(abs_path) = file_uri_to_project_path(&uri, project_root) else {
            continue;
        };
        let dedup_key = abs_path.canonicalize().unwrap_or_else(|_| abs_path.clone());
        if !seen.insert(dedup_key) {
            continue;
        }
        let display_path = abs_path.display().to_string();
        if abs_path.is_dir() {
            match list_directory_limited(&abs_path, project_root) {
                Ok(listing) => attachments.push_str(&format!(
                    "--- directory path: {display_path} ---\n{listing}\n\n"
                )),
                Err(error) => attachments.push_str(&format!(
                    "--- directory path: {display_path} (error: {error}) ---\n\n"
                )),
            }
        } else {
            match read_file_limited(&abs_path) {
                Ok(content) => attachments.push_str(&format!(
                    "--- source path: {display_path} ---\n{content}\n\n"
                )),
                Err(error) => attachments.push_str(&format!(
                    "--- source path: {display_path} (error: {error}) ---\n\n"
                )),
            }
        }
    }

    let stripped = strip_file_mention_links(trimmed_user_text);
    if attachments.is_empty() {
        return format!("User message:\n{stripped}");
    }
    format!(
        "The user attached the following from the project (paths are relative to the project root where applicable):\n\n{attachments}\nUser message:\n{stripped}"
    )
}

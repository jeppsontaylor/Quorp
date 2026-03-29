//! Parse `[@label](file://...)` mention links and expand attached paths into API message text.

use std::path::{Path, PathBuf};

use url::Url;

use crate::quorp::tui::path_guard::path_within_project;

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

/// Returns absolute path if `uri` is a `file:` URL inside `project_root`.
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

/// Build `[@label](file://...)` for an absolute path (trailing slash in URL for directories).
pub fn mention_link_for_path(abs_path: &Path, label: &str) -> Result<String, String> {
    let is_dir = abs_path.is_dir();
    let url = if is_dir {
        Url::from_directory_path(abs_path).map_err(|_| "invalid directory path for URL".to_string())?
    } else {
        Url::from_file_path(abs_path).map_err(|_| "invalid file path for URL".to_string())?
    };
    let mut s = url.to_string();
    if is_dir && !s.ends_with('/') {
        s.push('/');
    }
    Ok(format!("[@{label}]({s})"))
}

/// Extracts `(uri)` spans for each `[@...](...)` markdown link with `file` scheme.
pub fn collect_file_mention_uris(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut search_start = 0usize;
    while let Some(rel) = text[search_start..].find("[@") {
        let abs_start = search_start + rel;
        let Some(name_end_rel) = find_matching_bracket(&text[abs_start..], '[', ']') else {
            search_start = abs_start + 2;
            continue;
        };
        let name_end = abs_start + name_end_rel;
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

/// Strips `[@...](file:...)` links from text (collapses to single space where needed).
pub fn strip_file_mention_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut search_start = 0usize;
    while search_start < text.len() {
        let Some(rel) = text[search_start..].find("[@") else {
            out.push_str(&text[search_start..]);
            break;
        };
        let abs_start = search_start + rel;
        out.push_str(&text[search_start..abs_start]);
        let Some(name_end_rel) = find_matching_bracket(&text[abs_start..], '[', ']') else {
            out.push_str("[@");
            search_start = abs_start + 2;
            continue;
        };
        let name_end = abs_start + name_end_rel;
        if text.get(name_end + 1..name_end + 2) != Some("(") {
            out.push_str(&text[abs_start..=name_end]);
            search_start = name_end + 1;
            continue;
        }
        let uri_start = name_end + 2;
        let Some(uri_end_rel) = find_matching_bracket(&text[name_end + 1..], '(', ')') else {
            out.push_str(&text[abs_start..uri_start.min(text.len())]);
            search_start = uri_start;
            continue;
        };
        let uri_end = name_end + 1 + uri_end_rel;
        let uri = &text[uri_start..uri_end];
        if uri.starts_with("file:") {
            if !out.is_empty() && !out.chars().last().is_some_and(|c| c.is_whitespace()) {
                out.push(' ');
            }
            search_start = uri_end + 1;
        } else {
            out.push_str(&text[abs_start..=uri_end]);
            search_start = uri_end + 1;
        }
    }
    out.trim().to_string()
}

fn read_file_limited(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let slice = if bytes.len() > MAX_FILE_BYTES {
        &bytes[..MAX_FILE_BYTES]
    } else {
        bytes.as_slice()
    };
    let mut s = String::from_utf8_lossy(slice).into_owned();
    if bytes.len() > MAX_FILE_BYTES {
        s.push_str("\n… [truncated: file exceeds mention limit]");
    }
    Ok(s)
}

fn list_directory_limited(path: &Path, project_root: &Path) -> Result<String, String> {
    let mut builder = ignore::WalkBuilder::new(path);
    builder.standard_filters(true);
    builder.max_depth(Some(2));
    let mut lines = Vec::new();
    for walk_result in builder.build().flatten() {
        let p = walk_result.path();
        if p == path {
            continue;
        }
        if !path_within_project(p, project_root) {
            continue;
        }
        let rel = p.strip_prefix(path).unwrap_or(p);
        let label = if walk_result
            .file_type()
            .is_some_and(|ft| ft.is_dir())
        {
            format!("{}/", rel.display())
        } else {
            rel.display().to_string()
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

/// Prepends attachment context and returns the user-visible question without file links.
pub fn expand_mentions_for_api_message(trimmed_user_text: &str, project_root: &Path) -> String {
    let uris = collect_file_mention_uris(trimmed_user_text);
    if uris.is_empty() {
        return trimmed_user_text.to_string();
    }

    let mut attachments = String::new();
    let mut seen = std::collections::HashSet::<PathBuf>::new();
    for uri in uris {
        let Some(abs) = file_uri_to_project_path(&uri, project_root) else {
            continue;
        };
        let dedup_key = abs.canonicalize().unwrap_or_else(|_| abs.clone());
        if !seen.insert(dedup_key) {
            continue;
        }
        let is_dir = abs.is_dir();
        let path_line = abs.display().to_string();
        if is_dir {
            match list_directory_limited(&abs, project_root) {
                Ok(listing) => {
                    attachments.push_str(&format!(
                        "--- directory path: {path_line} ---\n{listing}\n\n"
                    ));
                }
                Err(e) => {
                    attachments.push_str(&format!(
                        "--- directory path: {path_line} (error: {e}) ---\n\n"
                    ));
                }
            }
        } else {
            match read_file_limited(&abs) {
                Ok(content) => {
                    attachments.push_str(&format!(
                        "--- source path: {path_line} ---\n{content}\n\n"
                    ));
                }
                Err(e) => {
                    attachments.push_str(&format!(
                        "--- source path: {path_line} (error: {e}) ---\n\n"
                    ));
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn collect_and_strip_roundtrip_empty() {
        let s = "hello no links";
        assert!(collect_file_mention_uris(s).is_empty());
        assert_eq!(strip_file_mention_links(s), s);
    }

    #[test]
    fn find_bracket_nested() {
        let t = "[@a [b]](file:///x)";
        let uris = collect_file_mention_uris(t);
        assert_eq!(uris.len(), 1);
        assert!(uris[0].starts_with("file:"));
    }

    #[test]
    fn expand_rejects_path_outside_root() {
        let dir = tempdir().expect("tempdir");
        let inner = dir.path().join("proj");
        fs::create_dir_all(&inner).expect("mkdir");
        let evil = tempdir().expect("tempdir2");
        let secret = evil.path().join("secret.txt");
        fs::write(&secret, "x").expect("write");
        let url = Url::from_file_path(&secret).unwrap().to_string();
        let msg = format!("see [@s]({url})");
        let out = expand_mentions_for_api_message(&msg, &inner);
        assert!(!out.contains("x"));
        assert!(out.contains("User message:"));
    }

    #[test]
    fn expand_includes_file_inside_root() {
        let dir = tempdir().expect("tempdir");
        let f = dir.path().join("a.rs");
        fs::write(&f, "// hi").expect("write");
        let url = Url::from_file_path(&f).unwrap().to_string();
        let msg = format!("ref [@a.rs]({url}) end");
        let out = expand_mentions_for_api_message(&msg, dir.path());
        assert!(out.contains("// hi"));
        assert!(out.contains("User message:"));
        assert!(!out.contains("file://"));
    }

    #[test]
    fn collect_multiple_file_uris_in_order() {
        let dir = tempdir().expect("tempdir");
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, "").expect("w");
        fs::write(&b, "").expect("w");
        let ua = Url::from_file_path(&a).unwrap().to_string();
        let ub = Url::from_file_path(&b).unwrap().to_string();
        let msg = format!("[@a]({ua}) then [@b]({ub})");
        let uris = collect_file_mention_uris(&msg);
        assert_eq!(uris.len(), 2);
        assert!(uris[0].contains("a.txt"));
        assert!(uris[1].contains("b.txt"));
    }

    #[test]
    fn strip_preserves_non_file_markdown_links() {
        let s = "x [@u](https://ex.com) y";
        assert!(collect_file_mention_uris(s).is_empty());
        assert_eq!(strip_file_mention_links(s), s);
    }

    #[test]
    fn expand_dedupes_same_file_twice() {
        let dir = tempdir().expect("tempdir");
        let f = dir.path().join("once.txt");
        fs::write(&f, "body").expect("w");
        let u = Url::from_file_path(&f).unwrap().to_string();
        let msg = format!("[@a]({u}) [@b]({u})");
        let out = expand_mentions_for_api_message(&msg, dir.path());
        assert_eq!(out.matches("body").count(), 1);
    }

    #[test]
    fn expand_truncates_large_file() {
        let dir = tempdir().expect("tempdir");
        let f = dir.path().join("big.bin");
        let big = vec![b'x'; MAX_FILE_BYTES + 1024];
        fs::write(&f, &big).expect("w");
        let u = Url::from_file_path(&f).unwrap().to_string();
        let msg = format!("[@b]({u})");
        let out = expand_mentions_for_api_message(&msg, dir.path());
        assert!(out.contains("truncated"));
    }

    #[test]
    fn expand_directory_lists_child_names() {
        let dir = tempdir().expect("tempdir");
        let sub = dir.path().join("childdir");
        fs::create_dir_all(&sub).expect("mkdir");
        fs::write(sub.join("inside.txt"), "").expect("w");
        let u = Url::from_directory_path(&sub).unwrap().to_string();
        let msg = format!("[@c]({u})");
        let out = expand_mentions_for_api_message(&msg, dir.path());
        assert!(out.contains("directory path"));
        assert!(out.contains("inside.txt"));
    }

    #[test]
    fn strip_malformed_open_bracket_does_not_panic() {
        let s = "[@only";
        assert!(collect_file_mention_uris(s).is_empty());
        let _ = strip_file_mention_links(s);
    }
}

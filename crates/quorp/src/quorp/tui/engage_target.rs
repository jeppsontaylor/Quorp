use std::path::{Path, PathBuf};

use url::Url;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngageTargetKind {
    FeedLink,
    ChangedFile,
    Artifact,
    TerminalPath,
    ToolTarget,
    Directory,
    File,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngageTarget {
    pub key: String,
    pub label: String,
    pub path: PathBuf,
    pub kind: EngageTargetKind,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub diff_capable: bool,
    pub source: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngageResolution {
    Local(EngageTarget),
    External(String),
}

pub fn resolve_target(
    target: &str,
    project_root: &Path,
    kind: EngageTargetKind,
    source: &'static str,
    diff_capable: bool,
) -> Option<EngageResolution> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Some(EngageResolution::External(trimmed.to_string()));
    }

    let (path_text, line, column) = split_line_and_column(trimmed);
    let path = normalize_target_path(path_text.as_str(), project_root)?;
    let label = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path_text.as_str())
        .to_string();
    let resolved_kind = if path.is_dir() {
        EngageTargetKind::Directory
    } else {
        match kind {
            EngageTargetKind::Artifact
            | EngageTargetKind::ChangedFile
            | EngageTargetKind::TerminalPath
            | EngageTargetKind::ToolTarget
            | EngageTargetKind::FeedLink
            | EngageTargetKind::Directory
            | EngageTargetKind::File => kind,
        }
    };
    let key = format!(
        "{}:{}:{}:{}",
        path.display(),
        line.unwrap_or(0),
        column.unwrap_or(0),
        resolved_kind as u8
    );

    Some(EngageResolution::Local(EngageTarget {
        key,
        label,
        path,
        kind: resolved_kind,
        line,
        column,
        diff_capable,
        source,
    }))
}

pub fn extract_openable_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut start = 0usize;
    let bytes = text.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if byte.is_ascii_whitespace() {
            if start < index {
                push_openable_candidate(&text[start..index], &mut tokens);
            }
            start = index + 1;
        }
    }
    if start < text.len() {
        push_openable_candidate(&text[start..], &mut tokens);
    }
    tokens
}

fn push_openable_candidate(candidate: &str, out: &mut Vec<String>) {
    let trimmed = trim_trailing_punctuation(candidate);
    if trimmed.is_empty() || !is_openable_candidate(trimmed) {
        return;
    }
    if out.iter().any(|existing| existing == trimmed) {
        return;
    }
    out.push(trimmed.to_string());
}

fn trim_trailing_punctuation(candidate: &str) -> &str {
    let mut end = candidate.len();
    while let Some(last) = candidate[..end].chars().last() {
        if matches!(last, '.' | ',' | ';' | ':' | ')' | ']' | '}' | '!' | '?') {
            end = end.saturating_sub(last.len_utf8());
            continue;
        }
        break;
    }
    &candidate[..end]
}

fn is_openable_candidate(candidate: &str) -> bool {
    if candidate == "/" {
        return false;
    }
    if candidate.starts_with("http://")
        || candidate.starts_with("https://")
        || candidate.starts_with("file://")
    {
        return true;
    }
    if candidate.starts_with('/')
        && candidate[1..]
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return false;
    }
    if candidate.starts_with('/')
        || candidate.starts_with("./")
        || candidate.starts_with("../")
        || candidate.starts_with('~')
        || candidate.contains('\\')
    {
        return true;
    }
    candidate.contains('/')
}

fn normalize_target_path(target: &str, project_root: &Path) -> Option<PathBuf> {
    if target.is_empty() {
        return None;
    }
    if target.starts_with("file://") {
        let url = Url::parse(target).ok()?;
        return url.to_file_path().ok();
    }
    if target.starts_with('~') {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok()?;
        if home.is_empty() {
            return None;
        }
        let suffix = target.trim_start_matches('~').trim_start_matches('/');
        return Some(PathBuf::from(home).join(suffix));
    }
    let raw = PathBuf::from(target);
    if raw.is_absolute() {
        return Some(raw);
    }
    Some(project_root.join(raw))
}

fn split_line_and_column(target: &str) -> (String, Option<usize>, Option<usize>) {
    if target.starts_with("file://") {
        return (target.to_string(), None, None);
    }
    let parts = target.rsplit(':').collect::<Vec<_>>();
    if parts.len() < 2 {
        return (target.to_string(), None, None);
    }

    let parse_usize = |value: &str| value.parse::<usize>().ok().filter(|number| *number > 0);
    let last = parts[0];
    let second = parts[1];
    if let (Some(column), Some(line)) = (parse_usize(last), parse_usize(second)) {
        let suffix_len = last.len() + second.len() + 2;
        if target.len() > suffix_len {
            let prefix = target[..target.len().saturating_sub(suffix_len)].to_string();
            if is_openable_candidate(prefix.as_str()) {
                return (prefix, Some(line), Some(column));
            }
        }
    }
    if let Some(line) = parse_usize(last) {
        let suffix_len = last.len() + 1;
        if target.len() > suffix_len {
            let prefix = target[..target.len().saturating_sub(suffix_len)].to_string();
            if is_openable_candidate(prefix.as_str()) {
                return (prefix, Some(line), None);
            }
        }
    }
    (target.to_string(), None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_tokens_finds_paths_and_strips_punctuation() {
        let tokens =
            extract_openable_tokens("see src/lib.rs:42, ../other/path.rs and /tmp/demo.txt.");
        assert!(tokens.contains(&"src/lib.rs:42".to_string()));
        assert!(tokens.contains(&"../other/path.rs".to_string()));
        assert!(tokens.contains(&"/tmp/demo.txt".to_string()));
    }

    #[test]
    fn extract_tokens_ignores_slash_commands_and_bare_root() {
        let tokens = extract_openable_tokens("Use /model or / to configure, then open src/lib.rs");
        assert!(!tokens.iter().any(|token| token == "/model"));
        assert!(!tokens.iter().any(|token| token == "/"));
        assert!(tokens.iter().any(|token| token == "src/lib.rs"));
    }

    #[test]
    fn resolve_relative_target_against_project_root() {
        let root = PathBuf::from("/tmp/project");
        let Some(EngageResolution::Local(target)) = resolve_target(
            "src/lib.rs:42:7",
            &root,
            EngageTargetKind::FeedLink,
            "feed",
            true,
        ) else {
            panic!("expected local target");
        };
        assert_eq!(target.path, root.join("src/lib.rs"));
        assert_eq!(target.line, Some(42));
        assert_eq!(target.column, Some(7));
        assert!(target.diff_capable);
    }

    #[test]
    fn resolve_http_target_stays_external() {
        let root = PathBuf::from("/tmp/project");
        let Some(EngageResolution::External(target)) = resolve_target(
            "https://example.com",
            &root,
            EngageTargetKind::FeedLink,
            "feed",
            false,
        ) else {
            panic!("expected external target");
        };
        assert_eq!(target, "https://example.com");
    }
}

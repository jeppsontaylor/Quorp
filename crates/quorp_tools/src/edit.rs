use std::io::Read;
use std::path::Path;

use quorp_agent_core::{ReadFileRange, TomlEditOperation, stable_content_hash};

pub const FILE_READ_LIMIT_BYTES: usize = 64 * 1024;
pub const FILE_READ_TRUNCATION_MARKER: &str = "\n[output truncated]";
pub const DIRECTORY_LIST_LIMIT: usize = 512;
pub const DIRECTORY_NAME_LIMIT: usize = 80;

pub fn read_file_contents(path: &Path, range: Option<ReadFileRange>) -> anyhow::Result<String> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(anyhow::anyhow!("Path is not a regular file"));
    }
    if metadata.len() > FILE_READ_LIMIT_BYTES as u64 {
        let file = std::fs::File::open(path)?;
        let mut bytes = Vec::with_capacity(FILE_READ_LIMIT_BYTES);
        file.take(FILE_READ_LIMIT_BYTES as u64)
            .read_to_end(&mut bytes)?;
        let mut text = String::from_utf8(bytes)
            .map_err(|error| anyhow::anyhow!("File is not valid UTF-8: {error}"))?;
        text.push_str(FILE_READ_TRUNCATION_MARKER);
        return slice_text_by_range(&text, range);
    }
    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8(bytes)
        .map_err(|error| anyhow::anyhow!("File is not valid UTF-8: {error}"))?;
    slice_text_by_range(&text, range)
}

pub fn slice_text_by_range(text: &str, range: Option<ReadFileRange>) -> anyhow::Result<String> {
    let Some(range) = range.and_then(ReadFileRange::normalized) else {
        return Ok(text.to_string());
    };
    let lines = text.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return Ok(String::new());
    }
    let start_index = range
        .start_line
        .saturating_sub(1)
        .min(lines.len().saturating_sub(1));
    let end_index = range.end_line.max(range.start_line).min(lines.len());
    Ok(lines[start_index..end_index].join("\n"))
}

pub fn perform_range_replacement(
    current_content: &str,
    range: ReadFileRange,
    expected_hash: &str,
    replacement: &str,
) -> anyhow::Result<String> {
    let range = range
        .normalized()
        .ok_or_else(|| anyhow::anyhow!("replace_range requires a valid 1-based line range"))?;
    let mut lines = current_content
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return Err(anyhow::anyhow!(
            "replace_range cannot target an empty file; use WriteFile if full-file creation is intended"
        ));
    }
    let start_index = range
        .start_line
        .saturating_sub(1)
        .min(lines.len().saturating_sub(1));
    let end_index = range.end_line.min(lines.len()).max(start_index + 1);
    let current_range_content = lines[start_index..end_index].join("\n");
    let current_hash = stable_content_hash(&current_range_content);
    if current_hash != expected_hash.trim() {
        return Err(anyhow::anyhow!(
            "replace_range hash mismatch for lines {}: expected_hash={} current content_hash={current_hash}. Reread the exact range before editing.",
            range.label(),
            expected_hash.trim()
        ));
    }
    let replacement_lines = replacement.lines().map(str::to_string).collect::<Vec<_>>();
    lines.splice(start_index..end_index, replacement_lines);
    let mut updated = lines.join("\n");
    if current_content.ends_with('\n') {
        updated.push('\n');
    }
    Ok(updated)
}

pub fn apply_toml_operations(
    current_content: &str,
    expected_hash: &str,
    operations: &[TomlEditOperation],
) -> anyhow::Result<String> {
    let current_hash = stable_content_hash(current_content);
    if current_hash != expected_hash.trim() {
        return Err(anyhow::anyhow!(
            "modify_toml hash mismatch: expected_hash={} current full-file content_hash={current_hash}. Read the full manifest first; partial range hashes are not accepted for TOML edits.",
            expected_hash.trim()
        ));
    }
    let mut document = current_content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| anyhow::anyhow!("current TOML did not parse: {error}"))?;
    for operation in operations {
        apply_toml_operation(&mut document, current_content, operation)?;
    }
    let updated = document.to_string();
    updated
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| anyhow::anyhow!("updated TOML did not parse: {error}"))?;
    Ok(updated)
}

pub fn apply_toml_operation(
    document: &mut toml_edit::DocumentMut,
    original_content: &str,
    operation: &TomlEditOperation,
) -> anyhow::Result<()> {
    match operation {
        TomlEditOperation::SetDependency {
            table,
            name,
            version,
            features,
            default_features,
            optional,
            package,
            path,
        } => {
            validate_dependency_table(document, original_content, table)?;
            ensure_dependency_table(document, table)?;
            let table_item = document
                .as_table_mut()
                .get_mut(table)
                .ok_or_else(|| anyhow::anyhow!("TOML table `{table}` was not available"))?;
            let table = table_item
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("TOML item `{table}` is not a table"))?;
            table.insert(
                name,
                dependency_item(
                    version,
                    features,
                    *default_features,
                    *optional,
                    package,
                    path,
                ),
            );
            Ok(())
        }
        TomlEditOperation::RemoveDependency { table, name } => {
            validate_dependency_table(document, original_content, table)?;
            if let Some(table_item) = document.as_table_mut().get_mut(table)
                && let Some(table) = table_item.as_table_mut()
            {
                table.remove(name);
            }
            Ok(())
        }
    }
}

pub fn validate_dependency_table(
    document: &toml_edit::DocumentMut,
    original_content: &str,
    table: &str,
) -> anyhow::Result<()> {
    match table {
        "dependencies" | "dev-dependencies" | "build-dependencies" => Ok(()),
        value if value.starts_with("target.") => {
            if toml_header_exists(original_content, value)
                && document.as_table().get(value).is_some()
            {
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "target-specific dependency table `{value}` must already exist as a directly editable table; use ReplaceRange for complex dotted/quoted target tables"
                ))
            }
        }
        other => Err(anyhow::anyhow!(
            "unsupported dependency table `{other}`. Use dependencies, dev-dependencies, build-dependencies, or an already-present target-specific table."
        )),
    }
}

pub fn toml_header_exists(content: &str, table: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .is_some_and(|header| header.trim() == table)
    })
}

pub fn ensure_dependency_table(
    document: &mut toml_edit::DocumentMut,
    table: &str,
) -> anyhow::Result<()> {
    if document.as_table().get(table).is_none() {
        document
            .as_table_mut()
            .insert(table, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    document
        .as_table_mut()
        .get_mut(table)
        .and_then(toml_edit::Item::as_table_mut)
        .map(|_| ())
        .ok_or_else(|| anyhow::anyhow!("TOML item `{table}` is not a table"))
}

pub fn dependency_item(
    version: &Option<String>,
    features: &[String],
    default_features: Option<bool>,
    optional: Option<bool>,
    package: &Option<String>,
    path: &Option<String>,
) -> toml_edit::Item {
    let needs_inline_table = !features.is_empty()
        || default_features.is_some()
        || optional.is_some()
        || package.is_some()
        || path.is_some();
    if !needs_inline_table {
        return toml_edit::value(version.as_deref().unwrap_or("*"));
    }

    let mut table = toml_edit::InlineTable::new();
    if let Some(version) = version.as_deref() {
        table.insert("version", toml_edit::Value::from(version));
    }
    if let Some(path) = path.as_deref() {
        table.insert("path", toml_edit::Value::from(path));
    }
    if let Some(package) = package.as_deref() {
        table.insert("package", toml_edit::Value::from(package));
    }
    if !features.is_empty() {
        let mut array = toml_edit::Array::new();
        for feature in features {
            array.push(feature.as_str());
        }
        table.insert("features", toml_edit::Value::Array(array));
    }
    if let Some(default_features) = default_features {
        table.insert("default-features", toml_edit::Value::from(default_features));
    }
    if let Some(optional) = optional {
        table.insert("optional", toml_edit::Value::from(optional));
    }
    table.fmt();
    toml_edit::Item::Value(toml_edit::Value::InlineTable(table))
}

pub fn count_file_lines(path: &Path) -> anyhow::Result<usize> {
    let text = read_file_contents(path, None)?;
    Ok(text.lines().count())
}

pub fn list_directory_entries(path: &Path) -> anyhow::Result<Vec<String>> {
    if !path.exists() {
        return Err(anyhow::anyhow!("Path does not exist"));
    }
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_dir() {
        return Err(anyhow::anyhow!("Path is not a directory"));
    }
    let mut names = std::fs::read_dir(path)?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let file_name = entry.file_name().into_string().ok()?;
            let metadata = entry.metadata().ok()?;
            let mut name = file_name;
            if metadata.is_dir() {
                name.push('/');
            }
            Some(name)
        })
        .collect::<Vec<_>>();
    names.sort();
    names.truncate(DIRECTORY_LIST_LIMIT);
    let mut output = Vec::new();
    for name in names {
        let mut line = name.clone();
        if name.len() > DIRECTORY_NAME_LIMIT {
            let truncated = DIRECTORY_NAME_LIMIT.saturating_sub(3);
            if truncated > 0 {
                if name.ends_with('/') {
                    line = format!(
                        "{}...",
                        &name[..truncated.min(name.len().saturating_sub(1))]
                    );
                    if !line.ends_with('/') {
                        line.push('/');
                    }
                } else {
                    line = format!("{}...", &name[..truncated]);
                }
            }
        }
        output.push(line);
    }
    Ok(output)
}

pub fn write_full_file(path: &Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            return Err(anyhow::anyhow!(
                "Parent directory does not exist: {parent:?}"
            ));
        }
        if !parent.is_dir() {
            return Err(anyhow::anyhow!("Parent path is not a directory"));
        }
    } else {
        return Err(anyhow::anyhow!("Invalid file path"));
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let filename = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Invalid target path"))?
        .to_string_lossy()
        .replace(['/', '\\'], "_");
    let tmp = path.with_file_name(format!(".{filename}.tmp.{nanos}"));
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn write_full_file_allow_create(path: &Path, content: &str) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid file path"))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| anyhow::anyhow!("Failed to create parent directory: {error}"))?;
    write_full_file(path, content)
}

#[cfg(unix)]
pub fn set_executable_bit(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(anyhow::anyhow!("Path is not a regular file"));
    }
    let mut permissions = metadata.permissions();
    let mode = permissions.mode();
    permissions.set_mode(mode | 0o111);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
pub fn set_executable_bit(_path: &Path) -> anyhow::Result<()> {
    Err(anyhow::anyhow!(
        "set_executable is only supported on unix-like systems"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use quorp_agent_core::ReadFileRange;
    use tempfile::tempdir;

    #[test]
    fn read_file_contents_rejects_binary_and_truncates() {
        let root = tempdir().expect("tempdir");
        let huge = root.path().join("huge.txt");
        let bytes = vec![b'a'; FILE_READ_LIMIT_BYTES + 123];
        std::fs::write(&huge, &bytes).expect("write");
        let output = read_file_contents(&huge, None).expect("read");
        assert!(output.ends_with(FILE_READ_TRUNCATION_MARKER));
        assert_eq!(
            output.len(),
            FILE_READ_LIMIT_BYTES + FILE_READ_TRUNCATION_MARKER.len()
        );

        let binary = root.path().join("binary.bin");
        std::fs::write(&binary, [0xff, 0x00]).expect("write");
        assert!(read_file_contents(&binary, None).is_err());
    }

    #[test]
    fn read_file_contents_honors_requested_range() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("sample.txt");
        std::fs::write(&file, "one\ntwo\nthree\nfour\n").expect("write");

        let output = read_file_contents(
            &file,
            Some(ReadFileRange {
                start_line: 2,
                end_line: 3,
            }),
        )
        .expect("read");

        assert_eq!(output, "two\nthree");
    }

    #[test]
    fn list_directory_entries_orders_and_truncates() {
        let root = tempdir().expect("tempdir");
        for index in 0..(DIRECTORY_LIST_LIMIT + 20) {
            let path = root.path().join(format!("file-{index:04}.txt"));
            std::fs::write(path, b"x").expect("write");
        }
        let entries = list_directory_entries(root.path()).expect("list");
        assert_eq!(entries.len(), DIRECTORY_LIST_LIMIT);
        assert!(entries.windows(2).all(|window| window[0] <= window[1]));

        let long_file = root.path().join("a".repeat(DIRECTORY_NAME_LIMIT + 20));
        std::fs::write(&long_file, b"x").expect("write long");
        let entries = list_directory_entries(root.path()).expect("list");
        assert!(
            entries
                .iter()
                .any(|entry| entry.len() <= DIRECTORY_NAME_LIMIT)
        );
    }

    #[test]
    fn write_full_file_replaces_content_and_requires_parent_dir() {
        let root = tempdir().expect("tempdir");
        let path = root.path().join("nested").join("file.txt");
        assert!(write_full_file(&path, "new").is_err());

        let file = root.path().join("existing.txt");
        write_full_file(&file, "before").expect("write");
        write_full_file(&file, "after").expect("rewrite");
        let content = std::fs::read_to_string(&file).expect("read");
        assert_eq!(content, "after");
    }

    #[cfg(unix)]
    #[test]
    fn set_executable_bit_marks_regular_file_executable() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().expect("tempdir");
        let file = root.path().join("script.sh");
        write_full_file(&file, "#!/bin/sh\necho hi\n").expect("write");
        set_executable_bit(&file).expect("chmod");
        let mode = std::fs::metadata(&file)
            .expect("metadata")
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0);
    }

    #[test]
    fn replace_range_uses_stable_hash_and_preserves_surrounding_content() {
        let current = "one\ntwo\nthree\n";
        let range = ReadFileRange {
            start_line: 2,
            end_line: 2,
        };
        let expected_hash = stable_content_hash("two");
        let updated =
            perform_range_replacement(current, range, &expected_hash, "TWO").expect("replace");
        assert_eq!(updated, "one\nTWO\nthree\n");
        let stale = perform_range_replacement(current, range, "0000000000000000", "TWO")
            .expect_err("stale hash");
        assert!(stale.to_string().contains("hash mismatch"));
    }

    #[test]
    fn modify_toml_sets_and_removes_dependency_with_full_file_hash() {
        let current = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n";
        let expected_hash = stable_content_hash(current);
        let updated = apply_toml_operations(
            current,
            &expected_hash,
            &[TomlEditOperation::SetDependency {
                table: "dependencies".to_string(),
                name: "chrono".to_string(),
                version: Some("0.4".to_string()),
                features: vec!["clock".to_string()],
                default_features: Some(false),
                optional: None,
                package: None,
                path: None,
            }],
        )
        .expect("set dependency");
        assert!(updated.contains("[dependencies]"));
        assert!(updated.contains("chrono"));
        assert!(updated.parse::<toml_edit::DocumentMut>().is_ok());

        let updated_hash = stable_content_hash(&updated);
        let removed = apply_toml_operations(
            &updated,
            &updated_hash,
            &[TomlEditOperation::RemoveDependency {
                table: "dependencies".to_string(),
                name: "chrono".to_string(),
            }],
        )
        .expect("remove dependency");
        assert!(!removed.contains("chrono"));
    }
}

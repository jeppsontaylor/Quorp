use serde::de::DeserializeOwned;

use super::{MemoryUpdate, TaskItem, TaskStatus, parse_task_status};

pub fn parse_optional_json_field<T>(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    warnings: &mut Vec<String>,
) -> Option<T>
where
    T: DeserializeOwned,
{
    let value = object.get(key)?;
    if value.is_null() {
        return None;
    }
    match serde_json::from_value::<T>(value.clone()) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            warnings.push(format!("Ignored malformed `{key}` field: {error}"));
            None
        }
    }
}

pub fn parse_task_updates(
    value: Option<&serde_json::Value>,
    warnings: &mut Vec<String>,
) -> Vec<TaskItem> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        warnings.push("Ignored non-array `task_updates` field.".to_string());
        return Vec::new();
    };

    let mut parsed = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match parse_task_item(item, warnings) {
            Some(task) => parsed.push(task),
            None => warnings.push(format!("Ignored malformed `task_updates[{index}]` entry.")),
        }
    }
    parsed
}

fn parse_task_item(value: &serde_json::Value, warnings: &mut Vec<String>) -> Option<TaskItem> {
    match value {
        serde_json::Value::String(title) => Some(TaskItem {
            title: title.trim().to_string(),
            status: TaskStatus::Pending,
        }),
        serde_json::Value::Object(object) => {
            let title = ["title", "progress", "summary", "message", "content"]
                .iter()
                .find_map(|key| object.get(*key).and_then(serde_json::Value::as_str))
                .unwrap_or_default()
                .trim()
                .to_string();

            let status = match object.get("status") {
                Some(serde_json::Value::String(raw_status)) => {
                    let parsed_status = parse_task_status(raw_status);
                    if parsed_status == TaskStatus::Pending
                        && !raw_status.trim().eq_ignore_ascii_case("pending")
                    {
                        warnings.push(format!(
                            "Coerced unsupported task status `{}` to `pending`.",
                            raw_status.trim()
                        ));
                    }
                    parsed_status
                }
                Some(serde_json::Value::Null) | None => TaskStatus::Pending,
                Some(_) => {
                    warnings.push("Coerced non-string task status to `pending`.".to_string());
                    TaskStatus::Pending
                }
            };

            if title.is_empty() {
                warnings.push(
                    "Task update was missing a `title`; using a generic placeholder.".to_string(),
                );
            }

            Some(TaskItem {
                title: if title.is_empty() {
                    "status updated".to_string()
                } else {
                    title
                },
                status,
            })
        }
        _ => None,
    }
}

pub fn parse_memory_updates(
    value: Option<&serde_json::Value>,
    warnings: &mut Vec<String>,
) -> Vec<MemoryUpdate> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        warnings.push("Ignored non-array `memory_updates` field.".to_string());
        return Vec::new();
    };

    let mut parsed = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match parse_memory_update(item, warnings) {
            Some(update) => parsed.push(update),
            None => warnings.push(format!(
                "Ignored malformed `memory_updates[{index}]` entry."
            )),
        }
    }
    parsed
}

fn parse_memory_update(
    value: &serde_json::Value,
    warnings: &mut Vec<String>,
) -> Option<MemoryUpdate> {
    match value {
        serde_json::Value::String(content) => Some(MemoryUpdate {
            kind: "note".to_string(),
            content: content.trim().to_string(),
            path: None,
        }),
        serde_json::Value::Object(object) => {
            let kind = object
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .or_else(|| object.get("type").and_then(serde_json::Value::as_str))
                .unwrap_or("note")
                .trim()
                .to_string();
            let content = object
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            let path = object
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string);

            if content.is_empty() {
                warnings.push("Ignored memory update without textual `content`.".to_string());
                return None;
            }

            Some(MemoryUpdate {
                kind,
                content,
                path,
            })
        }
        _ => None,
    }
}

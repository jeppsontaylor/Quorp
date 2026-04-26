use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::{ChallengeCapsule, ChallengeMetadata, ResolvedChallengeCase};

#[derive(Debug, Clone, Copy)]
pub struct RustSweCaseProfile {
    pub case_id: &'static str,
    pub fast_loop_commands: &'static [&'static str],
    pub final_eval_command: &'static str,
    pub likely_owner_files: &'static [&'static str],
    pub expected_touch_targets: &'static [&'static str],
}

pub fn compile_challenge_capsule(
    challenge: &ResolvedChallengeCase,
    sandbox_root: &Path,
) -> anyhow::Result<ChallengeCapsule> {
    let start_here = fs::read_to_string(&challenge.objective_source)
        .with_context(|| format!("failed to read {}", challenge.objective_source.display()))?;
    let repro_note_path = sandbox_root.join("LOCAL_REPRO.md");
    let repro_note = fs::read_to_string(&repro_note_path)
        .with_context(|| format!("failed to read {}", repro_note_path.display()))?;

    let start_fast_loop =
        extract_markdown_code_blocks(&extract_markdown_section(&start_here, "Fast Loop"));
    let repro_fast_loop =
        extract_markdown_code_blocks(&extract_markdown_section(&repro_note, "Fast Loop"));
    let owner_files =
        extract_path_like_items(&extract_markdown_section(&start_here, "Likely Owners"));
    let first_reads =
        extract_path_like_items(&extract_markdown_section(&repro_note, "First Reads"));
    let expected_touch_targets = challenge.manifest.expected_files_touched.clone();
    let companion_files_required = expected_touch_targets
        .iter()
        .filter(|path| is_companion_file(path))
        .cloned()
        .collect::<Vec<_>>();
    let strong_hints =
        extract_markdown_bullets(&extract_markdown_section(&start_here, "Strong Hints"));
    let watch_points =
        extract_markdown_bullets(&extract_markdown_section(&repro_note, "What To Watch"));
    let named_tests = watch_points
        .iter()
        .chain(strong_hints.iter())
        .flat_map(|item| extract_inline_code_spans(item))
        .filter(|item| !looks_like_path(item))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let case_class = classify_case_class(&expected_touch_targets, &companion_files_required);

    let capsule = ChallengeCapsule {
        case_class,
        owner_files,
        first_reads,
        fast_loop_commands: start_fast_loop
            .into_iter()
            .chain(repro_fast_loop)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        expected_touch_targets,
        companion_files_required,
        strong_hints,
        watch_points,
        named_tests,
    };
    Ok(apply_rust_swe_case_profile(capsule, &challenge.manifest.id))
}

pub fn build_challenge_objective(
    challenge: &ResolvedChallengeCase,
    metadata: &ChallengeMetadata,
) -> anyhow::Result<String> {
    let objective = fs::read_to_string(&challenge.objective_source)
        .with_context(|| format!("failed to read {}", challenge.objective_source.display()))?;
    let success = fs::read_to_string(&challenge.success_source)
        .with_context(|| format!("failed to read {}", challenge.success_source.display()))?;
    let objective_display =
        workspace_relative_display_path(&metadata.workspace_dir, &metadata.objective_file);
    let success_display =
        workspace_relative_display_path(&metadata.workspace_dir, &metadata.success_file);
    let mirrored_briefing_files = [
        Some(objective_display.clone()),
        Some(success_display.clone()),
        metadata
            .reference_file
            .as_ref()
            .map(|path| workspace_relative_display_path(&metadata.workspace_dir, path)),
        Some("benchmark.json".to_string()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ");
    let mut sections = vec![
        format!(
            "# Quorp Challenge Objective\n\nYou are running challenge `{}`: {}.\nKeep working until the case evaluator passes or you hit a budget stop.",
            challenge.manifest.id, challenge.manifest.title
        ),
        format!(
            "## Workspace\n- Editable workspace root: `.`\n- Condition: `{}`\n- Mirrored briefing files: {}\n- Do not modify files outside the workspace root.",
            metadata.condition, mirrored_briefing_files
        ),
        format!(
            "## Workspace Path Rules\n- All tool paths must be relative to the workspace root.\n- Do not use absolute paths in tool calls.\n- If you need orientation, start with `ListDirectory` on `.`.\n- Prefer the expected touch targets before top-level metadata files.\n- Avoid rereading `AGENTS.md`, `Cargo.lock`, `README.md`, or other root metadata unless the brief explicitly requires them.\n- Workspace root entries:\n{}",
            summarize_workspace_root(&metadata.workspace_dir)
        ),
        format!(
            "## Objective\n- File: `{}`\n- Inline summary:\n{}",
            objective_display,
            summarize_markdown_brief(&objective)
        ),
        format!(
            "## Success Criteria\n- File: `{}`\n- Inline summary:\n{}",
            success_display,
            summarize_markdown_brief(&success)
        ),
        format!(
            "## Commands\n- Reset: `{}`\n- Evaluate: `{}`\n- Stop when the evaluate command reports success.",
            substitute_condition(&challenge.manifest.reset_command, &challenge.condition),
            substitute_condition(&challenge.manifest.evaluate_command, &challenge.condition)
        ),
        format!(
            "## Expected Touch Targets\n{}",
            challenge
                .manifest
                .expected_files_touched
                .iter()
                .map(|path| format!("- `{}`", path))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        format!(
            "## Primary Metrics\n{}",
            challenge
                .manifest
                .primary_metrics
                .iter()
                .map(|metric| format!("- `{metric}`"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        format!(
            "## Challenge Capsule\n- Case class: `{}`\n- Primary owner files:\n{}\n- First reads:\n{}\n- Fast loop commands:\n{}\n- Companion files required:\n{}\n- Named tests/assertions to keep in view:\n{}\n- Strong hints:\n{}\n- Watch points:\n{}",
            metadata.capsule.case_class,
            render_bullet_list_or_none(&metadata.capsule.owner_files),
            render_bullet_list_or_none(&metadata.capsule.first_reads),
            render_bullet_list_or_none(&metadata.capsule.fast_loop_commands),
            render_bullet_list_or_none(&metadata.capsule.companion_files_required),
            render_bullet_list_or_none(&metadata.capsule.named_tests),
            render_bullet_list_or_none(&metadata.capsule.strong_hints),
            render_bullet_list_or_none(&metadata.capsule.watch_points)
        ),
        format!(
            "## Validation Ladder\n- First prove progress with the fast loop before full evaluation.\n{}\n- After any failed validation, summarize the failing test/assertion, patch or read the next owner file, and rerun the smallest relevant validation before widening.\n- Run `{}` only after the fast loop is green or the failure clearly requires broader validation.",
            metadata
                .capsule
                .fast_loop_commands
                .iter()
                .map(|command| format!("- Fast loop: `{command}`"))
                .collect::<Vec<_>>()
                .join("\n"),
            substitute_condition(&challenge.manifest.evaluate_command, &challenge.condition)
        ),
    ];
    if !metadata.capsule.companion_files_required.is_empty() {
        sections.push(format!(
            "## Companion File Sentinel\n- This case requires companion-file coverage in addition to code changes.\n{}\n- Do not stop before these surfaces are updated or deliberately ruled out by the brief and tests.",
            metadata
                .capsule
                .companion_files_required
                .iter()
                .map(|path| format!("- `{path}`"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if metadata.capsule.case_class == "narrow-owner-first" {
        sections.push(
            "## Narrow-Case Mode\n- Do not widen beyond the primary owner files and named tests until the fast loop proves the current hypothesis wrong."
                .to_string(),
        );
    }
    if !challenge.manifest.allowed_generated_files.is_empty() {
        sections.push(format!(
            "## Allowed Generated Files\n{}",
            challenge
                .manifest
                .allowed_generated_files
                .iter()
                .map(|path| format!("- `{}`", path))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if let Some(reference_file) = metadata.reference_file.as_ref() {
        let reference_display =
            workspace_relative_display_path(&metadata.workspace_dir, reference_file);
        let reference = fs::read_to_string(reference_file)
            .with_context(|| format!("failed to read {}", reference_file.display()))?;
        sections.push(format!(
            "## Reference\n- File: `{}`\n- Inline summary:\n{}",
            reference_display,
            summarize_markdown_brief(&reference)
        ));
    }
    if !challenge.manifest.tags.is_empty() {
        sections.push(format!(
            "## Tags\n{}",
            challenge
                .manifest
                .tags
                .iter()
                .map(|tag| format!("- `{tag}`"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    Ok(sections.join("\n\n"))
}

pub fn collect_challenge_context_files(
    metadata: &ChallengeMetadata,
) -> Vec<PathBuf> {
    vec![
        Some(metadata.workspace_dir.join("benchmark.json")),
        Some(metadata.objective_file.clone()),
        Some(metadata.success_file.clone()),
        metadata.reference_file.clone(),
        Some(metadata.capsule_file.clone()),
        Some(metadata.workspace_dir.join("AGENTS.md")),
        Some(metadata.workspace_dir.join("agent-map.json")),
        Some(metadata.workspace_dir.join("test-map.json")),
        Some(
            metadata
                .workspace_dir
                .join(".witness")
                .join("witness-graph.json"),
        ),
    ]
    .into_iter()
    .flatten()
    .filter(|path| path.exists())
    .collect()
}

pub fn summarize_markdown_brief(markdown: &str) -> String {
    markdown
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(12)
        .map(|line| {
            if line.starts_with('#') || line.starts_with('-') {
                format!("- {}", line.trim_start_matches('#').trim())
            } else {
                format!("- {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn summarize_workspace_root(workspace_dir: &Path) -> String {
    match fs::read_dir(workspace_dir) {
        Ok(entries) => {
            let mut names = entries
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let mut name = entry.file_name().into_string().ok()?;
                    let metadata = entry.metadata().ok()?;
                    if metadata.is_dir() {
                        name.push('/');
                    }
                    Some(name)
                })
                .collect::<Vec<_>>();
            names.sort();
            if names.is_empty() {
                "- [empty]".to_string()
            } else {
                names
                    .into_iter()
                    .take(12)
                    .map(|name| format!("- `{name}`"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        Err(_) => "- [unavailable]".to_string(),
    }
}

pub fn substitute_condition(command: &str, condition: &str) -> String {
    command.replace("<condition>", condition)
}

pub fn rust_swe_case_profile(case_id: &str) -> Option<RustSweCaseProfile> {
    const PROFILES: &[RustSweCaseProfile] = &[
        RustSweCaseProfile {
            case_id: "06-rust-swebench-bincode-serde-decoder-memory",
            fast_loop_commands: &["cargo test --quiet --features serde --test issues issue_474"],
            final_eval_command: "./evaluate.sh proof-full",
            likely_owner_files: &["src/features/serde/de_owned.rs"],
            expected_touch_targets: &["src/features/serde/de_owned.rs", "Cargo.toml"],
        },
        RustSweCaseProfile {
            case_id: "07-rust-swebench-chrono-epoch-truncation",
            fast_loop_commands: &["cargo test --quiet --lib round::tests::"],
            final_eval_command: "./evaluate.sh proof-full",
            likely_owner_files: &["src/round.rs"],
            expected_touch_targets: &["src/round.rs"],
        },
        RustSweCaseProfile {
            case_id: "08-rust-swebench-axum-fallback-merge",
            fast_loop_commands: &[
                "cargo test --quiet -p axum --lib --features headers routing::tests::",
            ],
            final_eval_command: "./evaluate.sh proof-full",
            likely_owner_files: &["axum/src/routing/mod.rs"],
            expected_touch_targets: &[
                "axum/src/routing/mod.rs",
                "axum/CHANGELOG.md",
                "axum/src/docs/routing/fallback.md",
                "axum/src/docs/routing/merge.md",
                "axum/src/docs/routing/nest.md",
            ],
        },
        RustSweCaseProfile {
            case_id: "09-rust-swebench-cargo-dist-create-release",
            fast_loop_commands: &[
                "cargo test --quiet -p cargo-dist --test integration-tests axolotlsay_edit_existing -- --exact",
            ],
            final_eval_command: "./evaluate.sh proof-full",
            likely_owner_files: &[
                "cargo-dist/src/backend/ci/github.rs",
                "cargo-dist/src/config.rs",
                "cargo-dist/src/init.rs",
                "cargo-dist/src/tasks.rs",
                "cargo-dist/templates/ci/github_ci.yml.j2",
            ],
            expected_touch_targets: &[
                "cargo-dist/src/backend/ci/github.rs",
                "cargo-dist/src/config.rs",
                "cargo-dist/src/init.rs",
                "cargo-dist/src/tasks.rs",
                "cargo-dist/templates/ci/github_ci.yml.j2",
            ],
        },
        RustSweCaseProfile {
            case_id: "10-rust-swebench-cc-rs-compile-intermediates",
            fast_loop_commands: &[
                "cargo test --quiet compile_intermediates",
                "cargo test --quiet gnu_smoke",
                "cargo test --quiet msvc_smoke",
            ],
            final_eval_command: "./evaluate.sh proof-full",
            likely_owner_files: &["src/lib.rs"],
            expected_touch_targets: &["src/lib.rs"],
        },
    ];
    PROFILES
        .iter()
        .find(|profile| profile.case_id == case_id)
        .copied()
}

fn apply_rust_swe_case_profile(mut capsule: ChallengeCapsule, case_id: &str) -> ChallengeCapsule {
    let Some(profile) = rust_swe_case_profile(case_id) else {
        return capsule;
    };
    extend_unique(
        &mut capsule.fast_loop_commands,
        profile
            .fast_loop_commands
            .iter()
            .map(|value| (*value).to_string()),
    );
    extend_unique(
        &mut capsule.owner_files,
        profile
            .likely_owner_files
            .iter()
            .map(|value| (*value).to_string()),
    );
    extend_unique(
        &mut capsule.expected_touch_targets,
        profile
            .expected_touch_targets
            .iter()
            .map(|value| (*value).to_string()),
    );
    capsule
        .strong_hints
        .push(format!("Final evaluator: `{}`", profile.final_eval_command));
    capsule
}

fn extend_unique(target: &mut Vec<String>, values: impl IntoIterator<Item = String>) {
    let mut seen = target.iter().cloned().collect::<BTreeSet<_>>();
    for value in values {
        if seen.insert(value.clone()) {
            target.push(value);
        }
    }
}

fn extract_markdown_section(markdown: &str, heading: &str) -> String {
    let mut capturing = false;
    let mut lines = Vec::new();
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            if trimmed.trim_start_matches("## ").trim() == heading {
                capturing = true;
                continue;
            }
            if capturing {
                break;
            }
        }
        if capturing {
            lines.push(line);
        }
    }
    lines.join("\n").trim().to_string()
}

fn extract_markdown_bullets(section: &str) -> Vec<String> {
    section
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("- ").map(str::trim))
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn extract_markdown_code_blocks(section: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut capturing = false;
    let mut current = Vec::new();
    for line in section.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if capturing {
                let block = current.join("\n").trim().to_string();
                if !block.is_empty() {
                    blocks.push(block);
                }
                current.clear();
                capturing = false;
            } else {
                capturing = true;
            }
            continue;
        }
        if capturing {
            current.push(trimmed.to_string());
        }
    }
    blocks
}

fn extract_path_like_items(section: &str) -> Vec<String> {
    let mut items = Vec::new();
    for bullet in extract_markdown_bullets(section) {
        let inline_paths = extract_inline_code_spans(&bullet)
            .into_iter()
            .filter(|item| looks_like_path(item))
            .collect::<Vec<_>>();
        if !inline_paths.is_empty() {
            items.extend(inline_paths);
            continue;
        }
        if looks_like_path(&bullet) {
            items.push(normalize_markdown_item(&bullet));
        }
    }
    items
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn extract_inline_code_spans(text: &str) -> Vec<String> {
    let mut spans = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if character == '`' {
            if let Some(open_index) = start.take() {
                let value = text[open_index + 1..index].trim();
                if !value.is_empty() {
                    spans.push(value.to_string());
                }
            } else {
                start = Some(index);
            }
        }
    }
    spans
}

fn looks_like_path(value: &str) -> bool {
    let trimmed = normalize_markdown_item(value);
    trimmed.contains('/')
        || trimmed.ends_with(".rs")
        || trimmed.ends_with(".md")
        || trimmed.ends_with(".toml")
        || trimmed.ends_with(".j2")
        || trimmed.ends_with(".json")
        || trimmed.ends_with(".yml")
}

fn normalize_markdown_item(value: &str) -> String {
    value
        .trim()
        .trim_matches('`')
        .trim_matches('.')
        .trim_matches(',')
        .trim()
        .to_string()
}

fn is_companion_file(path: &str) -> bool {
    path.contains("CHANGELOG")
        || path.contains("book/")
        || path.contains("docs/")
        || path.contains("templates/")
        || path.ends_with(".j2")
        || path.ends_with(".md")
}

fn classify_case_class(
    expected_touch_targets: &[String],
    companion_files_required: &[String],
) -> String {
    if expected_touch_targets.len() <= 2 && companion_files_required.is_empty() {
        "narrow-owner-first".to_string()
    } else if !companion_files_required.is_empty() && expected_touch_targets.len() >= 5 {
        "breadth-heavy-companion".to_string()
    } else if !companion_files_required.is_empty() {
        "companion-sensitive".to_string()
    } else {
        "multi-layer".to_string()
    }
}

fn render_bullet_list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "- [none]".to_string()
    } else {
        items
            .iter()
            .map(|item| format!("- `{}`", item))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn workspace_relative_display_path(workspace_dir: &Path, path: &Path) -> String {
    path.strip_prefix(workspace_dir)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

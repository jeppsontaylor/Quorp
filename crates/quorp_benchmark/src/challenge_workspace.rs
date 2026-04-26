use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::{copy_dir_all, looks_like_warpos_staged_workspace};

pub fn write_benchmark_sandbox_cargo_config(
    sandbox_root: &Path,
    condition: &str,
    cargo_cache_dir_name: &str,
) -> anyhow::Result<()> {
    let cargo_dir = sandbox_root.join(".cargo");
    fs::create_dir_all(&cargo_dir)?;
    fs::write(
        cargo_dir.join("config.toml"),
        format!("[build]\ntarget-dir = \"../{cargo_cache_dir_name}/{condition}\"\n"),
    )?;
    Ok(())
}

pub fn write_workspace_challenge_command_wrappers(workspace_dir: &Path) -> anyhow::Result<()> {
    for file_name in ["evaluate.sh", "reset.sh"] {
        let wrapper_path = workspace_dir.join(file_name);
        if wrapper_path.exists() {
            continue;
        }
        fs::write(
            &wrapper_path,
            format!(
                "#!/usr/bin/env bash\nset -euo pipefail\ncd \"$(dirname \"$0\")/../..\"\nexec ./{file_name} \"$@\"\n"
            ),
        )?;
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&wrapper_path)?.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&wrapper_path, permissions)?;
        }
    }
    Ok(())
}

pub fn resolve_challenge_workspace_dir(
    sandbox_root: &Path,
    condition: &str,
) -> anyhow::Result<PathBuf> {
    let legacy_workspace = sandbox_root.join("workspace").join(condition);
    if legacy_workspace.exists() {
        return Ok(legacy_workspace);
    }
    if looks_like_flat_challenge_workspace(sandbox_root)
        || looks_like_warpos_staged_workspace(sandbox_root)
    {
        return Ok(sandbox_root.to_path_buf());
    }
    anyhow::bail!(
        "failed to locate challenge workspace for condition `{condition}`; expected `{}` or a flat WarpOS challenge bundle at `{}`",
        legacy_workspace.display(),
        sandbox_root.display()
    )
}

pub fn maybe_materialize_flat_challenge_reset_script(
    result_dir: &Path,
    sandbox_root: &Path,
) -> anyhow::Result<()> {
    if sandbox_root.join("reset.sh").exists() || !looks_like_flat_challenge_workspace(sandbox_root)
    {
        return Ok(());
    }

    let baseline_root = result_dir.join(".quorp-flat-baseline");
    if baseline_root.exists() {
        fs::remove_dir_all(&baseline_root)
            .with_context(|| format!("failed to clear {}", baseline_root.display()))?;
    }
    let quoted_baseline = shell_single_quote(&baseline_root.display().to_string());
    let reset_script = format!(
        "#!/usr/bin/env bash\n\
         set -euo pipefail\n\
         baseline={quoted_baseline}\n\
         if [[ ! -d \"${{baseline}}\" ]]; then\n\
           echo \"missing flat challenge reset baseline: ${{baseline}}\" >&2\n\
           exit 1\n\
         fi\n\
         find . -mindepth 1 -maxdepth 1 -exec rm -rf -- {{}} +\n\
         cp -a \"${{baseline}}/.\" .\n"
    );
    let reset_path = sandbox_root.join("reset.sh");
    fs::write(&reset_path, reset_script)
        .with_context(|| format!("failed to write {}", reset_path.display()))?;
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(&reset_path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&reset_path, permissions)?;
    }
    copy_dir_all(sandbox_root, &baseline_root).with_context(|| {
        format!(
            "failed to snapshot flat challenge baseline {} -> {}",
            sandbox_root.display(),
            baseline_root.display()
        )
    })?;
    Ok(())
}

fn looks_like_flat_challenge_workspace(path: &Path) -> bool {
    path.join("benchmark.json").is_file()
        && path.join("evaluate.sh").is_file()
        && (path.join("START_HERE.md").is_file() || path.join("README.md").is_file())
        && (path.join("SUCCESS.md").is_file() || path.join("expected").exists())
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

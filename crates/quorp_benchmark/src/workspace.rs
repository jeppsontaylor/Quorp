use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;

use crate::ResolvedBenchmark;

pub fn rebase_attempt_path(
    resolved: &ResolvedBenchmark,
    workspace_dir: &Path,
    original_path: &Path,
) -> PathBuf {
    original_path
        .strip_prefix(&resolved.workspace_source)
        .map(|relative| workspace_dir.join(relative))
        .unwrap_or_else(|_| original_path.to_path_buf())
}

pub fn copy_dir_all(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if should_skip_copy_entry(&entry) {
            continue;
        }
        let destination = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &destination)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &destination)?;
            let permissions = fs::metadata(entry.path())?.permissions();
            fs::set_permissions(&destination, permissions)?;
        } else if file_type.is_symlink() {
            let target = fs::read_link(entry.path())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &destination)?;
        }
    }
    Ok(())
}

fn should_skip_copy_entry(entry: &fs::DirEntry) -> bool {
    matches!(
        entry.file_name().to_str(),
        Some("target")
            | Some(".quorp-cargo-target")
            | Some(".quorp-cargo-target-eval")
            | Some(".git")
            | Some("node_modules")
    )
}

pub fn copy_file_if_different(src: &Path, dst: &Path) -> anyhow::Result<()> {
    if src == dst {
        return Ok(());
    }
    if src.exists()
        && dst.exists()
        && let (Ok(src_canonical), Ok(dst_canonical)) =
            (fs::canonicalize(src), fs::canonicalize(dst))
        && src_canonical == dst_canonical
    {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::copy(src, dst)
        .with_context(|| format!("failed to copy {} to {}", src.display(), dst.display()))?;
    Ok(())
}

pub fn ensure_git_baseline(workspace_dir: &Path) -> anyhow::Result<()> {
    if workspace_dir.join(".git").exists() {
        return Ok(());
    }
    #[allow(clippy::disallowed_methods)]
    let init_status = Command::new("git")
        .arg("init")
        .current_dir(workspace_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !init_status.success() {
        anyhow::bail!("failed to initialize git in {}", workspace_dir.display());
    }
    #[allow(clippy::disallowed_methods)]
    let add_status = Command::new("git")
        .args(["add", "."])
        .current_dir(workspace_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !add_status.success() {
        anyhow::bail!(
            "failed to stage sandbox baseline in {}",
            workspace_dir.display()
        );
    }
    #[allow(clippy::disallowed_methods)]
    let commit_status = Command::new("git")
        .args([
            "-c",
            "user.name=quorp",
            "-c",
            "user.email=quorp@example.com",
            "commit",
            "-qm",
            "Benchmark baseline",
        ])
        .current_dir(workspace_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !commit_status.success() {
        anyhow::bail!(
            "failed to commit sandbox baseline in {}",
            workspace_dir.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_dir_all_skips_generated_build_directories() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let source = temp_dir.path().join("source");
        let destination = temp_dir.path().join("destination");

        fs::create_dir_all(source.join("nested")).expect("nested");
        fs::create_dir_all(source.join("target").join("debug")).expect("target");
        fs::create_dir_all(source.join(".quorp-cargo-target").join("debug")).expect("cache");
        fs::create_dir_all(source.join(".quorp-cargo-target-eval").join("debug"))
            .expect("eval cache");
        fs::create_dir_all(source.join(".git").join("objects")).expect("git");
        fs::create_dir_all(source.join("node_modules").join("pkg")).expect("node_modules");
        fs::write(source.join("nested").join("keep.txt"), "keep").expect("keep");
        fs::write(source.join("target").join("debug").join("drop.txt"), "drop").expect("drop");
        fs::write(
            source
                .join(".quorp-cargo-target")
                .join("debug")
                .join("drop.txt"),
            "drop",
        )
        .expect("cache drop");
        fs::write(
            source
                .join(".quorp-cargo-target-eval")
                .join("debug")
                .join("drop.txt"),
            "drop",
        )
        .expect("eval drop");
        fs::write(source.join(".git").join("objects").join("drop.txt"), "drop").expect("git drop");
        fs::write(
            source.join("node_modules").join("pkg").join("drop.txt"),
            "drop",
        )
        .expect("node_modules drop");

        copy_dir_all(&source, &destination).expect("copy");

        assert!(destination.join("nested").join("keep.txt").exists());
        assert!(!destination.join("target").exists());
        assert!(!destination.join(".quorp-cargo-target").exists());
        assert!(!destination.join(".quorp-cargo-target-eval").exists());
        assert!(!destination.join(".git").exists());
        assert!(!destination.join("node_modules").exists());
    }
}

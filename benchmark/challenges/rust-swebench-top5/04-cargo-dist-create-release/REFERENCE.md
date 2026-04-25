# Reference

- Dataset: `user2f86/rustbench`
- Split: `train`
- Instance: `axodotdev__cargo-dist-367`
- Repository: `axodotdev/cargo-dist`
- Base commit: `b3c413953f319841c755dc9247035122f26918f3`
- Dataset version: `4.0`
- Issue: https://github.com/axodotdev/cargo-dist/issues/366
- Pull request: https://github.com/axodotdev/cargo-dist/pull/367

## Problem Statement

See [`upstream/problem_statement.md`](upstream/problem_statement.md).

## Gold Patch Files

- `book/src/config.md`
- `cargo-dist/src/backend/ci/github.rs`
- `cargo-dist/src/config.rs`
- `cargo-dist/src/init.rs`
- `cargo-dist/src/tasks.rs`
- `cargo-dist/templates/ci/github_ci.yml.j2`

## Dataset Fail-to-Pass Tests

- `axolotlsay_edit_existing`

## Dataset Pass-to-Pass Tests

- `akaikatana_repo_with_dot_git`
- `axolotlsay_basic`
- `akaikatana_basic`
- `axolotlsay_no_homebrew_publish`
- `install_path_cargo_home`
- `env_path_invalid - should panic`
- `install_path_env_no_subdir`
- `install_path_env_subdir`
- `install_path_env_subdir_space`
- `install_path_env_subdir_space_deeper`
- `install_path_home_subdir_deeper`
- `install_path_home_subdir_min`
- `install_path_home_subdir_space`
- `install_path_home_subdir_space_deeper`
- `install_path_invalid - should panic`

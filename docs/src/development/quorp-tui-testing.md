# Quorp TUI Testing

`quorp` is validated as a native TUI product. The verification flow is:

1. `cargo check -p quorp --bin quorp`
2. seam/unit coverage for workspace-backed editor and file-tree behavior
3. focused TUI flow tests
4. Rust-native screenshot bundle export
5. visual regression checks against stable baselines

## Screenshot Workflow

Run the screenshot suite with:

```bash
./script/tui-screenshot-suite
```

Artifacts are written to:

```text
crates/quorp/target/tui_screenshots/
```

The bundle includes:

- startup/default workspace
- file tree plus preview
- terminal output
- chat composing
- chat streaming
- command running
- command finished
- model picker overlay

`INDEX.md` in that directory is the review manifest for design comparison and user-testing prep.

## Full Verification

Run the end-to-end gate with:

```bash
./script/tui-verify
```

That command now includes screenshot export and visual regression checks so the artifact path stays part of the standard release workflow.

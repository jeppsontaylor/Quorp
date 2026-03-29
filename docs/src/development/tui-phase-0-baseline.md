# TUI Phase 0 baseline (started March 29, 2026)

Phase 0 objective: freeze the current TUI behavior and define the acceptance bar before any additional seam extraction or runtime removal work.

## Acceptance checklist

### Startup and terminal restore

- [ ] TUI starts from `quorp -- --tui` without panicking.
- [ ] Leaving TUI restores terminal mode cleanly.

### File tree and preview

- [ ] File tree loads root entries.
- [ ] Selecting a file updates preview/editor content.
- [ ] Navigation between sibling and nested directories remains stable.

### Integrated terminal vertical

- [ ] PTY session can be created.
- [ ] Resize events update PTY dimensions.
- [ ] Keyboard input is delivered to the PTY process.
- [ ] Terminal output stream renders in TUI.

### Chat and model flow

- [ ] Chat request dispatch succeeds.
- [ ] Streaming response events are rendered incrementally.
- [ ] Model selection and command/tool routing remain functional.

### Verification gates

- [ ] `cargo check -p quorp --bin quorp` passes.
- [ ] `./script/tui-verify` passes all three stages.

## Phase 0 kickoff snapshot

Run these commands at the beginning of each migration batch:

1. `./script/stage-a-audit`
2. `./script/stage-a-next-blocker`
3. `./script/tui-verify`

Initial snapshot on March 29, 2026:

- `./script/stage-a-audit` reports a long tail of missing `=0.1.0` workspace crates without local path crates.
- `./script/stage-a-next-blocker` reports `collab_ui` as the next missing package.
- `./script/tui-verify` is expected to fail Stage 1 until the TUI-critical dependency subset compiles.

## Working agreement for Phase 0

- Resolve only blockers required for TUI runtime and backend behavior.
- Defer clearly GUI-only crates into the legacy track.
- Do not widen scope by adding placeholder crates unless they are required to unblock TUI-critical verification.

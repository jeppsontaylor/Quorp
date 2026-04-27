# Phase 1 Terminal UX Capture

Phase 1 makes the default session surface stream-first instead of dashboard-first. The renderer now has a single deterministic scene that combines the brand header, task list, active command card, completed command card, footer, splash, transcript lines, and permission prompt.

Regenerate the plain capture with:

```sh
NO_COLOR=1 QUORP_RENDER_DEMO_STATIC=1 cargo run -q -p quorp -- render-demo
```

Regenerate the truecolor terminal demo with:

```sh
QUORP_RENDER_DEMO_STATIC=1 cargo run -q -p quorp -- render-demo
```

Paper-friendly SVG capture:

- `docs/src/development/phase-1-terminal-ux.svg`

Plain capture:

```text
QUORP // brilliant terminal coding
agent-first Rust runtime · truecolor stream · sandboxed tools
--------------------------------------------------------------------------------------
task list
  ✓ Plan task with proof gates
  * Run command with live chroma
  · Compress proof into receipt
+------------------------------------------------------------------------------------+
| verify  ⠹ running                                                                  |
| $ ./script/clippy                                                                  |
| cwd ~/Code/quorp                                                                   |
| strict lane running · raw log retained · first error pins span                     |
+------------------------------------------------------------------------------------+
+------------------------------------------------------------------------------------+
| lib tests  passed exit=0 0.65s                                                     |
| $ cargo test --workspace --lib                                                     |
| cwd ~/Code/quorp                                                                   |
| 421 passed across 39 suites                                                        |
+------------------------------------------------------------------------------------+
qwen3-coder@nvidia · --yolo sandbox · ctx 12.4k/64k · tasks 2/3
```

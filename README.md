<p align="center">
  <img src="assets/images/quorp_video_header.gif" alt="QUORP banner" width="100%" />
</p>

<h1 align="center">Quorp</h1>

<p align="center">
  <strong>A high-performance, TUI-first text editor and agentic interface.</strong>
</p>

<p align="center">
  Native Rust foundations, terminal-native workflows, and a mascot with just enough menace.
</p>

Quorp takes the robust GPUI research foundation behind Zed and reworks it into a focused terminal-first environment for editing, orchestration, and AI-assisted workflows. The current workspace is centered on a Rust terminal binary with the CLI extracted into `quorp_cli`, the session/runtime path extracted into `quorp_session`, and smart-tooling support crates wired into the agent loop incrementally.

## Status

Quorp is mid-transition from the Zed backend into a standalone terminal-first agent runtime. The current tree already ships the extracted CLI/session crates, typed runtime actions, context expansion hooks, Patch VM-backed write receipts on the native backend, bounded runtime event fanout, verification reports with proof packets, and benchmark scoring gates.

The earlier handoff work has now been closed in this branch. Historical upgrade audits live under `tips/upgrade/v1`, and the recovered `tips/upgrade/v2` advisory set is present in this checkout as 24 text notes covering repo localization, precise edit semantics, task runtime, verification, memory, permissions, MCP, and worktree isolation.

## License

Quorp is open-source software licensed under the GNU General Public License v3.0 or later.

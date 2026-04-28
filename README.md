<p align="center">
  <img src="assets/images/quorp-mascot.png" alt="Quorp mascot" width="520" />
</p>

<h1 align="center">Quorp</h1>

<p align="center">
  <strong>A high-performance, TUI-first text editor and agentic interface.</strong>
</p>

<p align="center">
  <img src="assets/images/quorp-mascot-header.gif" alt="Animated Quorp mascot icon" width="96" />
</p>

<p align="center">
  Native Rust foundations, terminal-native workflows, and a mascot with just enough menace.
</p>

Quorp takes the robust GPUI research foundation behind Zed and reworks it into a focused terminal-first environment for editing, orchestration, and AI-assisted workflows. The current workspace is centered on a Rust terminal binary with the CLI extracted into `quorp_cli`, the session/runtime path extracted into `quorp_session`, and smart-tooling support crates wired into the agent loop incrementally.

## Status

Quorp is mid-transition from the Zed backend into a standalone terminal-first agent runtime. The current tree already ships the extracted CLI/session crates, typed runtime actions, context expansion hooks, Patch VM-backed write receipts on the native backend, bounded runtime event fanout, verification reports with proof packets, and benchmark scoring gates.

The remaining roadmap work is narrower than the earlier handoffs imply: durable subscriber workers, memory/rule policy, deeper verify execution, and broader replay/proof ergonomics are still being tightened. Historical upgrade audits currently live under `tips/upgrade/v1`; there is no tracked `tips/upgrade/v2` directory in this checkout, though v2-like notes were recovered from dangling git blobs.

## License

Quorp is open-source software licensed under the GNU General Public License v3.0 or later.

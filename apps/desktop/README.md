# Quorp Desktop (Tauri 2)

Tauri 2 + React 18 + TypeScript 5 + Vite 5 desktop shell over the
Rust runtime crates. The CLI binary at `crates/quorp` is unchanged;
this app is a sibling product.

## Prerequisites

- Node 20 LTS (`>= 20.10`)
- pnpm 9.12.x
- Rust 1.93+ (managed by `rust-toolchain.toml`)
- macOS 12+ for `pnpm tauri dev` and signed bundles

The Tauri shell crate at `apps/desktop/src-tauri` is intentionally
**excluded** from the root cargo workspace (`Cargo.toml` `[workspace]
exclude = ["apps/desktop/src-tauri"]`). Linux CI's
`cargo check --workspace` skips it automatically.

## Local development

First-time setup (verifies the toolchain, installs frontend deps,
sanity-checks the cargo workspace and the Tauri shell):

```bash
./script/quorp-desktop-bootstrap
```

Then launch with hot-reload:

```bash
./script/quorp-desktop-dev
# or directly:
cd apps/desktop && pnpm install --frozen-lockfile && pnpm tauri dev
```

The first `tauri dev` downloads Tauri/wry dependencies and takes a
few minutes. Subsequent runs are incremental.

## What's wired today

- ✅ Tauri shell + 35 typed IPC commands
- ✅ Workspace add/list/trust + Trust onboarding
- ✅ Settings → Provider with macOS Keychain storage and "Validate model"
- ✅ Live runs against NVIDIA NIM Qwen3-Coder when a key is set
      (synthetic demo stream when not, so the rest of the UI still
      exercises end-to-end)
- ✅ macOS Apple sandbox (sandbox-exec) + `cp -c -R` /tmp clonefile
      lifecycle for sandboxed runs
- ✅ Diff inspector with hand-rolled unified-diff renderer + Apply
      to source workspace (git-apply or in-process applier with
      atomic conflict reporting)
- ✅ Proof inspector (L0..L4 ladder placeholder)
- ✅ Permission modal with risk-tinted Allow/Deny + scope (Once /
      Session / Project)
- ✅ Benchmarks panel surfacing every fixture under
      `benchmark/challenges/rust-swebench-top5/`
- ✅ Doctor panel (sandbox-exec probe, Xcode CLT, Developer ID,
      `$PATH` from Finder, runtime state, …)
- ✅ Memory query (live `quorp_memory::Memory::recall` per workspace)
- ✅ Rules listing + lifecycle ledger persistence
- ✅ Theme switcher (Dark / High contrast / No-color, plus Light +
      System behind a feature flag)
- ✅ Multi-session sidebar (default-on)
- ✅ Manual update check
- ✅ Replay viewer transport bar
- 🟡 xterm.js terminal pane, multi-window, auto-updater feed,
      checkpoint rollback, verify-again, memory pruning — typed stubs
      ready; substantive Rust impls land in follow-up PRs without
      changing the wire surface.

For the full architectural plan, see
`/Users/bentaylor/.claude/plans/can-you-please-study-cuddly-creek.md`.

## Building a DMG

```bash
./script/quorp-desktop-build-dmg            # ad-hoc signed (local dev)
./script/quorp-desktop-build-dmg-signed     # Developer ID + notarization (CI)
```

Set the following env vars before invoking the signed build:
`APPLE_SIGNING_IDENTITY`, `APPLE_TEAM_ID`, `APPLE_API_KEY_ID`,
`APPLE_API_KEY_PATH`, `APPLE_API_ISSUER`.

## Capabilities allowlist

Edit `src-tauri/capabilities/default.json` carefully. Do **NOT** add
`shell:*` or `fs:*` plugins — every privileged operation is exposed as
a typed `quorp:*` Rust command in `src-tauri/src/commands/`.
The CI lint script at `script/quorp-desktop-capabilities-lint` greps
for those names and fails the build.

## Architecture pointers

- `src-tauri/src/commands/` — Tauri command wrappers; one file per
  domain (workspace, run, permission, benchmark, replay, provider,
  doctor).
- `src-tauri/src/menu.rs` — native macOS menu bar.
- `src/types/ipc.ts` — TypeScript mirrors of `quorp_desktop_ipc` DTOs.
- `src/lib/invoke.ts` — typed `invoke()` wrapper.
- `src/store/{view,workspace,run}Store.ts` — Zustand stores.
- `src/components/*.tsx` — layout shell + composer + timeline +
  inspector (PR5 surface).

For the full plan, see
`/Users/bentaylor/.claude/plans/can-you-please-study-cuddly-creek.md`.

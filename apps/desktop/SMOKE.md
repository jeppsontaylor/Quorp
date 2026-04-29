# Quorp Desktop — Manual Smoke Checklist

The CI workflow at `.github/workflows/desktop-release.yml` builds and
notarizes the DMG, but it cannot click buttons. Walk this checklist
before tagging a release.

Tested matrix: macOS 12 Monterey, 13 Ventura, 14 Sonoma, 15 Sequoia.
Run on each major version that's currently in market.

---

## 0 · Pre-flight (every release)

- [ ] `./script/quorp-desktop-doctor` exits 0 (or only warns about no
      Developer ID for local builds).
- [ ] `cargo test -p quorp_desktop_ipc -p quorp_desktop_core -p quorp_sandbox`
      reports 100+ tests passing.
- [ ] `./script/quorp-desktop-sandbox-smoke` passes all 5 checks.
- [ ] `./script/quorp-desktop-capabilities-lint` exits 0.
- [ ] `cargo check --workspace` is green.

## 1 · Install + first-launch

- [ ] Mount the DMG → drag Quorp.app into `/Applications`.
- [ ] Launch from `/Applications` (NOT from the DMG mount).
- [ ] Gatekeeper does NOT show "developer cannot be verified" on a
      notarized build. (Ad-hoc builds will, and that's expected; the
      release notes should warn users.)
- [ ] Window opens to the empty layout (LeftRail + LeftPanel + center
      Timeline + Composer + Inspector). Mission bar shows
      `Sandbox-exec ✓` chip.
- [ ] No `~/Library/Containers/ai.veox.quorp.desktop/` is created
      (App Sandbox is intentionally OFF).

## 2 · Workspace flow

- [ ] Click `+ Add Folder` → standard macOS picker appears (dialog
      plugin), not a save panel.
- [ ] Pick a folder → it appears in the LeftPanel list with a padlock
      icon (untrusted).
- [ ] Click the workspace → it becomes active in the mission bar.
- [ ] Hover the workspace row → "Trust" / "Open in CLI" buttons appear.
- [ ] Click "Open in CLI" → Terminal.app opens cd'd to the folder.
- [ ] Relaunch the app → workspace is still in the list (workspace
      registry survives — PR9 ships the in-memory registry; persistence
      lands in PR10).

## 3 · Settings → Provider

- [ ] `Cmd+,` opens the Settings dialog. Provider section is one of
      the tabs in the left nav.
- [ ] Paste a NIM API key → click `Save` → success message; key field
      clears.
- [ ] `security find-generic-password -s "ai.veox.quorp.desktop" -a "nvidia-nim"`
      from the terminal returns the entry (key is in the Keychain).
- [ ] Click `Validate model` → returns `{ok: true, latency_ms: …,
      model_id_echo: "qwen/qwen3-coder-480b-a35b-instruct"}`.
- [ ] Click `Remove key` → `has_key` flips to `no key configured`.
- [ ] Re-save a different key, verify with the security CLI again.
- [ ] Network panel inside the app stays at `(deny network*)` —
      Validate works because the parent process (not the sandboxed
      agent) makes the HTTP call.

## 4 · Demo run (no key configured)

- [ ] Settings → Provider → Remove key.
- [ ] Composer: type "test" → `Cmd+Enter`.
- [ ] Timeline lights up: `run_started` → 2× synthetic batches →
      `run_finished`.
- [ ] `Cmd+.` during the run cancels cleanly within ~250 ms.
- [ ] The Permissions inspector tab badge stays at 0.
- [ ] Status inspector populates with run id, model, steps.

## 5 · Live run (NIM key configured)

- [ ] Settings → Provider → Save NIM key.
- [ ] Composer: type a small task in a Trusted workspace → Send.
- [ ] Timeline streams real `RuntimeEvent` cards: `run_started`,
      `model_request_started/finished`, `tool_call_*`, etc.
- [ ] Mission bar tokens chip increases.
- [ ] If permission is set to `Ask`: PermissionModal auto-opens for
      mutating actions; click `Allow Once` → run continues.
- [ ] `Cmd+.` cancels cleanly; `events.jsonl` exists under
      `<workspace>/.quorp/runs/<run-id>/`.

## 6 · Benchmarks

- [ ] LeftRail → Benchmarks. Center pane shows fixture cards from
      `benchmark/challenges/rust-swebench-top5/`.
- [ ] Click `Run in sandbox` on `01-bincode-serde-decoder-memory`.
      Confirmation modal appears.
- [ ] Confirm → `start_benchmark_run` invoked. Surface flips to
      Sessions; Timeline streams events.
- [ ] `/private/tmp/quorp/<run-id>/` exists during the run.
      `ls /private/tmp/quorp/<run-id>/` shows `work/`, `cargo-home/`,
      `profile.sb`, `run-meta.json`.
- [ ] After the run finishes, the temp dir is removed (unless
      `keep_last_sandbox` is set in settings).

## 7 · Doctor

- [ ] LeftRail → Doctor. Page loads with overall badge.
- [ ] Every probe runs to completion; sandbox-exec, Xcode CLT, node,
      pnpm, $PATH delta, workspace count, provider key, runtime state.
- [ ] Click Refresh → probes re-run.
- [ ] Tools → Open Doctor (menu) → routes to the same surface.

## 8 · Theme + accessibility

- [ ] Settings → UI → switch to High contrast. Backgrounds darken,
      text contrast jumps. Diff colors stay legible.
- [ ] Switch to No-color. Diff colors flatten; `+` / `-` gutters
      stay; risk badges still readable.
- [ ] Switch back to Dark.
- [ ] Tab through the LeftRail and LeftPanel — focus rings visible.

## 9 · Replay

- [ ] Sessions → click a finished run (PR10 wires this; PR9 ships
      replay-via-IPC for ISSUE-01 invoked manually). Replay viewer
      loads `events.jsonl` with the transport bar.
- [ ] Pause / Play / pacing select work.
- [ ] Replay matches live timeline by snapshot (manual visual check).

## 10 · Capabilities + security

- [ ] `apps/desktop/src-tauri/capabilities/default.json` does NOT
      contain `shell:*` or `fs:*` — verified by
      `script/quorp-desktop-capabilities-lint`.
- [ ] DevTools (View → Reload page) is disabled in release.
- [ ] `lsof -p <pid>` on the running app shows no broad fs handles.

## 11 · Notarization (signed builds only)

- [ ] `spctl --assess --type open --context context:primary-signature -v Quorp.dmg`
      returns `accepted` `source=Notarized Developer ID`.
- [ ] `xcrun stapler validate Quorp.dmg` returns `The validate action
      worked!`.
- [ ] First-time launch from `/Applications` does not show the "from
      the internet" dialog twice (one is acceptable).

## 12 · Uninstall

- [ ] Drag Quorp.app to Trash.
- [ ] `~/Library/Application Support/Quorp/` survives (workspace +
      trust state) — by design; this is the user's data.
- [ ] `security find-generic-password -s "ai.veox.quorp.desktop"` still
      returns the key. Removal of the keychain entry is a separate
      action via Settings → Provider → Remove key.
- [ ] `/private/tmp/quorp/` is empty (all per-run temp dirs were
      cleaned up on Drop).

## 13 · Edge cases / regressions

- [ ] Cancel during model streaming — `Cmd+.` flips the cancellation
      flag mid-stream; agent loop exits with `Cancelled` stop reason.
- [ ] Permission timeout — start a run in `Ask` mode, walk away, come
      back after 2+ minutes. Modal disappears; broker logs `TimedOut`;
      next mutating action re-prompts.
- [ ] Run with network = LocalhostOnly — `cargo --version` works;
      `curl https://1.1.1.1` fails.
- [ ] Multiple workspaces, switch between them mid-run — active run
      doesn't change.
- [ ] Resize the window down to the 1200×760 minimum; layout doesn't
      collapse.

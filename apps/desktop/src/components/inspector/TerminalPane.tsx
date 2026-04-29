// Terminal pane (PR10). The xterm.js dependency is intentionally NOT
// added in this PR — wiring it requires a portable-pty backend on
// the Rust side, and we want the Expansive feature toggle to be
// reversible without forcing every user to download xterm.js's
// runtime. This pane renders a clear "coming soon" placeholder when
// the flag is on.

export function TerminalPane() {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center text-sm text-fg-muted">
      <p className="font-medium text-fg-primary">Terminal pane</p>
      <p className="max-w-md text-xs">
        xterm.js + a portable-pty Rust backend land in the next
        Expansive sprint. The toggle reserves the inspector tab and
        keymap routes; the runtime adapter opens a separate window
        once <code>portable-pty</code> is wired into{" "}
        <code>quorp_desktop_core</code>.
      </p>
    </div>
  );
}

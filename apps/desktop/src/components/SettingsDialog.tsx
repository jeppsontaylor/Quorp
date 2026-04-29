import { useEffect, useState } from "react";

import { ProviderSection } from "@/components/settings/ProviderSection";
import {
  FLAG_DESCRIPTIONS,
  FLAG_LABELS,
  useFeatureFlags,
} from "@/store/featureFlags";
import { useSettingsStore, type SettingsSection } from "@/store/settingsStore";
import { useViewStore, type Theme } from "@/store/viewStore";
import { cn } from "@/lib/utils";

const SECTIONS: { id: SettingsSection; label: string }[] = [
  { id: "general", label: "General" },
  { id: "workspaces", label: "Workspaces" },
  { id: "provider", label: "Provider" },
  { id: "models", label: "Models" },
  { id: "permissions", label: "Permissions" },
  { id: "sandbox", label: "Sandbox" },
  { id: "memory", label: "Memory" },
  { id: "rules", label: "Rules" },
  { id: "ui", label: "UI" },
  { id: "storage", label: "Storage" },
  { id: "updates", label: "Updates" },
  { id: "reset", label: "Reset" },
];

export function SettingsDialog() {
  const open = useSettingsStore((s) => s.open);
  const close = useSettingsStore((s) => s.closeSettings);
  const section = useSettingsStore((s) => s.section);
  const setSection = useSettingsStore((s) => s.setSection);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [close, open]);

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-bg-overlay backdrop-blur-sm"
      role="dialog"
      aria-modal="true"
      aria-label="Settings"
    >
      <div className="grid h-[600px] w-[920px] max-w-[95vw] grid-cols-[200px_1fr] overflow-hidden rounded-lg border border-border-subtle bg-bg-surface shadow-2xl">
        <nav
          aria-label="Settings sections"
          className="flex flex-col border-r border-border-subtle bg-bg-base py-2 text-sm"
        >
          {SECTIONS.map(({ id, label }) => (
            <button
              key={id}
              type="button"
              onClick={() => setSection(id)}
              className={cn(
                "px-4 py-1.5 text-left text-fg-secondary hover:bg-bg-elevated hover:text-fg-primary",
                section === id && "bg-bg-elevated text-fg-primary",
              )}
            >
              {label}
            </button>
          ))}
        </nav>
        <div className="flex flex-col overflow-hidden">
          <header className="flex items-center justify-between border-b border-border-subtle px-5 py-3">
            <h2 className="text-sm font-semibold uppercase tracking-wider text-fg-primary">
              Settings · {SECTIONS.find((s) => s.id === section)?.label}
            </h2>
            <button
              type="button"
              onClick={close}
              className="rounded-sm border border-border-subtle px-2 py-0.5 text-xs hover:border-ring-focus"
            >
              Close (Esc)
            </button>
          </header>
          <div className="flex-1 overflow-y-auto px-5 py-4">
            {section === "provider" && <ProviderSection />}
            {section === "general" && <GeneralSection />}
            {section === "models" && <ModelsSection />}
            {section === "permissions" && (
              <Placeholder>
                Per-workspace allowlist editor lands in PR9. Use the
                Permissions inspector tab on the active run for now.
              </Placeholder>
            )}
            {section === "sandbox" && (
              <Placeholder>
                Sandbox defaults (network policy, disk budget,
                keep-last) wire to <code>quorp_config</code> in PR9.
              </Placeholder>
            )}
            {section === "workspaces" && (
              <Placeholder>
                Add/remove workspaces from the left panel. Per-workspace
                defaults UI lands in PR9.
              </Placeholder>
            )}
            {section === "memory" && (
              <Placeholder>Memory tier controls land in PR10.</Placeholder>
            )}
            {section === "rules" && (
              <Placeholder>Rules editor lands in PR10.</Placeholder>
            )}
            {section === "ui" && <UiSection />}
            {section === "storage" && (
              <Placeholder>
                Storage retention controls land in PR9 alongside the
                run-ledger sweeper UI.
              </Placeholder>
            )}
            {section === "updates" && (
              <Placeholder>
                Manual check only in v1; auto-updates land in PR10.
              </Placeholder>
            )}
            {section === "reset" && <ResetSection />}
          </div>
        </div>
      </div>
    </div>
  );
}

function GeneralSection() {
  return (
    <Placeholder>
      Theme and density live under <strong>UI</strong>. Other general
      settings land in PR9.
    </Placeholder>
  );
}

function ModelsSection() {
  return (
    <div className="text-sm">
      <p className="text-fg-secondary">
        Quorp ships with a single model — NVIDIA NIM Qwen3-Coder
        (480B-A35B-Instruct). To configure access, open the{" "}
        <strong>Provider</strong> tab.
      </p>
      <p className="mt-3 text-xs text-fg-muted">
        Model ID: <code>qwen/qwen3-coder-480b-a35b-instruct</code>
      </p>
    </div>
  );
}

function UiSection() {
  const theme = useViewStore((s) => s.theme);
  const setTheme = useViewStore((s) => s.setTheme);
  const flags = useFeatureFlags((s) => s.flags);
  const setFlag = useFeatureFlags((s) => s.setFlag);

  const baseThemes: { id: Theme; label: string; description: string }[] = [
    { id: "quorp-dark", label: "Dark", description: "Default Quorp theme." },
    {
      id: "quorp-high-contrast",
      label: "High contrast",
      description: "Boosted contrast for legibility.",
    },
    {
      id: "quorp-no-color",
      label: "No-color",
      description: "Monochrome with symbol cues for color-blind safety.",
    },
  ];
  const lightThemes: { id: Theme; label: string; description: string }[] = [
    {
      id: "quorp-light",
      label: "Light",
      description: "Light surfaces; same diff fidelity as Dark.",
    },
    {
      id: "quorp-system",
      label: "System",
      description: "Tracks OS appearance via `prefers-color-scheme`.",
    },
  ];
  const themes = flags.light_theme
    ? [...baseThemes, ...lightThemes]
    : baseThemes;

  return (
    <div className="flex flex-col gap-4 text-sm">
      <header>
        <h3 className="text-base font-semibold text-fg-primary">Theme</h3>
        <p className="mt-1 text-xs text-fg-muted">
          Switching applies immediately and persists for this session.
        </p>
      </header>
      <ul className="flex flex-col gap-2">
        {themes.map((t) => (
          <li key={t.id}>
            <button
              type="button"
              onClick={() => setTheme(t.id)}
              className={cn(
                "flex w-full flex-col gap-1 rounded-md border bg-bg-base px-3 py-2 text-left",
                theme === t.id
                  ? "border-ring-focus"
                  : "border-border-subtle hover:border-border-strong",
              )}
            >
              <span className="font-medium text-fg-primary">{t.label}</span>
              <span className="text-xs text-fg-muted">{t.description}</span>
            </button>
          </li>
        ))}
      </ul>
      <hr className="border-border-subtle" />
      <header>
        <h3 className="text-base font-semibold text-fg-primary">
          Expansive features
        </h3>
        <p className="mt-1 text-xs text-fg-muted">
          Off-by-default surfaces. Toggling adds inspector tabs, side
          panels, or background services. Each is independently
          revertible.
        </p>
      </header>
      <ul className="flex flex-col gap-1.5">
        {(
          Object.entries(FLAG_LABELS) as [
            keyof typeof FLAG_LABELS,
            string,
          ][]
        ).map(([id, label]) => (
          <li
            key={id}
            className="flex items-center justify-between rounded-md border border-border-subtle bg-bg-base px-3 py-2"
          >
            <div className="flex flex-col">
              <span className="text-sm font-medium text-fg-primary">
                {label}
              </span>
              <span className="text-[11px] text-fg-muted">
                {FLAG_DESCRIPTIONS[id]}
              </span>
            </div>
            <ToggleSwitch
              checked={flags[id]}
              onChange={(v) => setFlag(id, v)}
              ariaLabel={label}
            />
          </li>
        ))}
      </ul>
    </div>
  );
}

function ToggleSwitch({
  checked,
  onChange,
  ariaLabel,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  ariaLabel: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      onClick={() => onChange(!checked)}
      className={cn(
        "relative inline-flex h-5 w-9 items-center rounded-full border transition-colors",
        checked
          ? "border-status-success/60 bg-status-success/30"
          : "border-border-subtle bg-bg-elevated",
      )}
    >
      <span
        className={cn(
          "inline-block h-3.5 w-3.5 rounded-full bg-fg-primary transition-transform",
          checked ? "translate-x-[18px]" : "translate-x-[3px]",
        )}
      />
    </button>
  );
}

function ResetSection() {
  return (
    <div className="flex flex-col gap-3 text-sm">
      <p className="text-fg-secondary">
        Destructive operations live here. Each requires double-confirmation
        (PR9). For now the buttons are advisory only.
      </p>
      <ul className="flex flex-col gap-2 text-xs">
        <ResetRow
          label="Clear caches"
          description="Drops the in-memory artifact LRU and any cached IPC reads."
        />
        <ResetRow
          label="Clear /tmp/quorp"
          description="Removes every per-run sandbox copy under /tmp/quorp/. Confirmed runs are unaffected."
        />
        <ResetRow
          label="Reset all settings"
          description="Forgets workspaces, trust state, and UI preferences. Keychain key is preserved."
        />
      </ul>
    </div>
  );
}

function ResetRow({
  label,
  description,
}: {
  label: string;
  description: string;
}) {
  return (
    <li className="flex items-center justify-between rounded-md border border-status-danger/30 bg-bg-base px-3 py-2">
      <div>
        <p className="font-medium text-fg-primary">{label}</p>
        <p className="text-[11px] text-fg-muted">{description}</p>
      </div>
      <button
        type="button"
        disabled
        className="rounded-sm border border-status-danger/40 px-2 py-0.5 text-[11px] text-status-danger opacity-50"
        title="Disabled until double-confirm flow lands"
      >
        Confirm
      </button>
    </li>
  );
}

function UpdatesSection() {
  const flags = useFeatureFlags((s) => s.flags);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<{
    current: string;
    latest: string;
    available: boolean;
    channel: string;
  } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const check = async () => {
    setBusy(true);
    setError(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const r = await invoke<{
        current_version: string;
        latest_known: string;
        update_available: boolean;
        channel: string;
      }>("check_for_updates");
      setStatus({
        current: r.current_version,
        latest: r.latest_known,
        available: r.update_available,
        channel: r.channel,
      });
    } catch (err) {
      setError(String((err as { message?: string })?.message ?? err));
    } finally {
      setBusy(false);
    }
  };

  if (!flags.auto_updater) {
    return (
      <Placeholder>
        Manual update checks live behind the
        <strong> Auto-updater </strong> Expansive flag (Settings → UI).
        Until then, distribute by replacing the .app bundle in
        /Applications.
      </Placeholder>
    );
  }

  return (
    <div className="flex flex-col gap-3 text-sm">
      <header>
        <h3 className="text-base font-semibold text-fg-primary">Updates</h3>
        <p className="mt-1 text-xs text-fg-muted">
          Manual check today. The Sparkle/Tauri-updater feed wiring
          ships once the release channel is published.
        </p>
      </header>
      <button
        type="button"
        onClick={check}
        disabled={busy}
        className="self-start rounded-md border border-border-subtle px-3 py-1.5 text-xs hover:border-ring-focus disabled:opacity-50"
      >
        Check for updates
      </button>
      {status && (
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 rounded-md border border-border-subtle bg-bg-base p-3 text-[11px]">
          <dt className="font-medium uppercase tracking-wider text-fg-muted">
            Current
          </dt>
          <dd className="font-mono text-fg-primary">{status.current}</dd>
          <dt className="font-medium uppercase tracking-wider text-fg-muted">
            Latest known
          </dt>
          <dd className="font-mono text-fg-primary">{status.latest}</dd>
          <dt className="font-medium uppercase tracking-wider text-fg-muted">
            Channel
          </dt>
          <dd className="font-mono text-fg-primary">{status.channel}</dd>
          <dt className="font-medium uppercase tracking-wider text-fg-muted">
            Available
          </dt>
          <dd
            className={
              status.available ? "text-status-info" : "text-status-success"
            }
          >
            {status.available ? "yes — apply via DMG" : "up to date"}
          </dd>
        </dl>
      )}
      {error && <p className="text-xs text-status-danger">{error}</p>}
    </div>
  );
}

function Placeholder({ children }: { children: React.ReactNode }) {
  return (
    <div className="rounded-md border border-dashed border-border-subtle bg-bg-base p-4 text-xs text-fg-muted">
      {children}
    </div>
  );
}

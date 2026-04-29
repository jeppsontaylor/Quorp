import { useEffect, useState } from "react";

import { ipc } from "@/lib/invoke";
import type { ProviderHealth, ProviderSummary } from "@/types/ipc";

/**
 * Settings → Provider. Single provider — NVIDIA NIM Qwen3-Coder.
 * The user pastes their NIM API key here; Rust stores it in the
 * macOS Keychain. The UI never reads the key back; it only
 * displays `has_key: bool` and lets the user clear or re-enter it.
 */
export function ProviderSection() {
  const [summary, setSummary] = useState<ProviderSummary | null>(null);
  const [health, setHealth] = useState<ProviderHealth | null>(null);
  const [keyDraft, setKeyDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const next = await ipc.providerInfo();
      setSummary(next);
    } catch (err) {
      setError(stringifyError(err));
    }
  };

  useEffect(() => {
    refresh().catch(() => {});
  }, []);

  const onSave = async () => {
    if (keyDraft.trim().length === 0) {
      setError("API key must not be empty.");
      return;
    }
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      await ipc.setNimApiKey(keyDraft);
      // Zero the draft string so the value doesn't sit in JS heap
      // any longer than necessary. Rust took ownership and dropped
      // the inbound copy.
      setKeyDraft("");
      setMessage("API key saved to macOS Keychain.");
      await refresh();
    } catch (err) {
      setError(stringifyError(err));
    } finally {
      setBusy(false);
    }
  };

  const onClear = async () => {
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      await ipc.clearNimApiKey();
      setMessage("API key removed from Keychain.");
      await refresh();
    } catch (err) {
      setError(stringifyError(err));
    } finally {
      setBusy(false);
    }
  };

  const onValidate = async () => {
    setBusy(true);
    setError(null);
    setMessage(null);
    setHealth(null);
    try {
      const result = await ipc.validateNimProvider();
      setHealth(result);
    } catch (err) {
      setError(stringifyError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-4 text-sm">
      <header>
        <h3 className="text-base font-semibold text-fg-primary">Provider</h3>
        <p className="mt-1 text-xs text-fg-muted">
          Quorp ships with a single provider. Bring your own NVIDIA NIM API
          key — it's stored in the macOS Keychain and never leaves Rust.
        </p>
      </header>
      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 rounded-md border border-border-subtle bg-bg-base p-3 text-[12px]">
        <dt className="font-medium uppercase tracking-wider text-fg-muted">
          Provider
        </dt>
        <dd className="font-mono text-fg-primary">
          {summary?.display_name ?? "—"}
        </dd>
        <dt className="font-medium uppercase tracking-wider text-fg-muted">
          Base URL
        </dt>
        <dd className="font-mono text-fg-primary break-all">
          {summary?.base_url ?? "—"}
        </dd>
        <dt className="font-medium uppercase tracking-wider text-fg-muted">
          Model
        </dt>
        <dd className="font-mono text-fg-primary">
          {summary?.default_model ?? "—"}
        </dd>
        <dt className="font-medium uppercase tracking-wider text-fg-muted">
          Key status
        </dt>
        <dd className="font-mono">
          {summary?.has_key ? (
            <span className="text-status-success">✓ stored in Keychain</span>
          ) : (
            <span className="text-status-warning">no key configured</span>
          )}
        </dd>
      </dl>
      <section>
        <label
          htmlFor="nim-api-key"
          className="block text-xs uppercase tracking-wider text-fg-muted"
        >
          NIM API key
        </label>
        <div className="mt-1 flex gap-2">
          <input
            id="nim-api-key"
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={keyDraft}
            placeholder={
              summary?.has_key
                ? "(leave blank to keep stored key)"
                : "nvapi-…"
            }
            onChange={(e) => setKeyDraft(e.target.value)}
            className="flex-1 rounded-md border border-border-subtle bg-bg-base px-2 py-1.5 font-mono text-sm text-fg-primary outline-none focus:border-ring-focus"
          />
          <button
            type="button"
            disabled={busy || keyDraft.trim().length === 0}
            onClick={onSave}
            className="rounded-md border border-border-subtle px-3 py-1.5 text-sm hover:border-ring-focus disabled:opacity-50"
          >
            Save
          </button>
        </div>
        <div className="mt-2 flex gap-2 text-xs">
          <button
            type="button"
            disabled={busy || !summary?.has_key}
            onClick={onValidate}
            className="rounded-md border border-border-subtle px-3 py-1 hover:border-ring-focus disabled:opacity-50"
          >
            Validate model
          </button>
          <button
            type="button"
            disabled={busy || !summary?.has_key}
            onClick={onClear}
            className="rounded-md border border-status-danger/40 px-3 py-1 text-status-danger hover:border-status-danger disabled:opacity-50"
          >
            Remove key
          </button>
        </div>
      </section>
      {health && (
        <section
          aria-label="Health-check result"
          className="rounded-md border border-border-subtle bg-bg-base p-3 text-xs"
        >
          <p className={health.ok ? "text-status-success" : "text-status-danger"}>
            {health.ok ? "✓ healthy" : "✗ unhealthy"} · latency{" "}
            {health.latency_ms} ms
          </p>
          {health.model_id_echo && (
            <p className="mt-1 font-mono text-[11px] text-fg-muted">
              model_id_echo: {health.model_id_echo}
            </p>
          )}
          {health.error && (
            <p className="mt-1 text-[11px] text-status-danger">
              {health.error}
            </p>
          )}
        </section>
      )}
      {message && (
        <p className="text-xs text-status-success">{message}</p>
      )}
      {error && <p className="text-xs text-status-danger">{error}</p>}
    </div>
  );
}

function stringifyError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}

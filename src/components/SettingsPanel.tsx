import {
  AlertTriangle,
  BellRing,
  Expand,
  Eye,
  EyeOff,
  Laptop,
  Moon,
  Pencil,
  Plus,
  Save,
  Shrink,
  Sun,
  Trash2,
  X
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { AppSettings, ProviderConfig, ProviderId } from "../types";
import { formatRelativeMinutes } from "../lib/format";
import { ProviderIcon } from "./ProviderIcon";

interface SettingsPanelProps {
  configs: ProviderConfig[];
  settings: AppSettings;
  selectedAccountId: number | null;
  onSelectAccount: (accountId: number | null) => void;
  onSaveConfig: (config: ProviderConfig) => Promise<void>;
  onDeleteConfig: (accountId: number) => Promise<void>;
  onTestAlert: (accountId: number, alertKind: "threshold" | "reset") => Promise<void>;
  onSaveSettings: (patch: Partial<AppSettings>) => Promise<void>;
  saving: boolean;
}

const labels: Record<ProviderId, string> = {
  openai: "OpenAI Codex",
  claude: "Claude",
  google: "Google Antigravity",
  minimax: "MiniMax",
  glm: "GLM"
};

const defaults: Record<ProviderId, Pick<ProviderConfig, "authMethod" | "baseUrl">> = {
  openai: { authMethod: "local_credential", baseUrl: "https://chatgpt.com/backend-api/wham/usage" },
  claude: { authMethod: "local_credential", baseUrl: "https://claude.ai/api/organizations/{org_id}/usage" },
  google: { authMethod: "local_credential", baseUrl: "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota" },
  minimax: { authMethod: "api_key", baseUrl: "https://www.minimax.io/v1/token_plan/remains" },
  glm: { authMethod: "api_key", baseUrl: "https://api.z.ai/api/monitor/usage/quota/limit" }
};

function newAccount(provider: ProviderId = "openai"): ProviderConfig {
  return {
    accountId: 0,
    provider,
    customName: "",
    authMethod: defaults[provider].authMethod,
    apiKey: "",
    baseUrl: defaults[provider].baseUrl,
    enabled: true,
    thresholdAlertEnabled: true,
    resetAlertEnabled: true,
    visible: true,
    sortOrder: 0
  };
}

export function SettingsPanel({
  configs,
  settings,
  selectedAccountId,
  onSelectAccount,
  onSaveConfig,
  onDeleteConfig,
  onTestAlert,
  onSaveSettings,
  saving
}: SettingsPanelProps) {
  const selectedStored = useMemo(
    () => configs.find((config) => config.accountId === selectedAccountId) ?? null,
    [configs, selectedAccountId]
  );
  const [draft, setDraft] = useState<ProviderConfig | null>(null);
  const [form, setForm] = useState<ProviderConfig>(() => newAccount());
  const [pendingDelete, setPendingDelete] = useState<ProviderConfig | null>(null);

  useEffect(() => {
    const next = draft ?? selectedStored;
    if (next) setForm(next);
  }, [draft, selectedStored]);

  useEffect(() => {
    if (!pendingDelete) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !saving) setPendingDelete(null);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [pendingDelete, saving]);

  const updateForm = (patch: Partial<ProviderConfig>) => {
    setForm((current) => ({ ...current, ...patch }));
    if (draft) setDraft((current) => current ? { ...current, ...patch } : current);
  };

  const closeEditor = () => {
    setDraft(null);
    onSelectAccount(null);
  };

  return (
    <section className="rounded-lg border border-border bg-panel p-5 shadow-soft">
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="m-0 text-lg font-semibold text-ink">Accounts and automation</h2>
          <p className="m-0 mt-1 text-sm text-slate-400">Credentials stay local and each card refreshes independently.</p>
        </div>
        <button
          type="button"
          onClick={() => {
            const next = newAccount();
            setDraft(next);
            setForm(next);
            onSelectAccount(null);
          }}
          className="action-button inline-flex items-center gap-2 rounded-md border px-3 py-2 text-sm font-medium"
        >
          <Plus className="h-4 w-4" />
          Add account
        </button>
      </div>

      <div className="grid gap-5 xl:grid-cols-[minmax(0,1fr)_300px]">
        <div className="space-y-4">
          <div className="grid gap-2 sm:grid-cols-2">
            {configs.map((config) => (
              <div
                key={config.accountId}
                className={`flex min-w-0 items-center gap-2 rounded-md border px-3 py-2.5 transition ${
                  !draft && selectedAccountId === config.accountId
                    ? "border-slate-500 bg-slate-800"
                    : "border-slate-800 bg-slate-950/40"
                }`}
              >
                <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-slate-700 bg-slate-950 text-white">
                  <div className="h-5 w-5 [&>img]:h-full [&>img]:w-full [&>img]:object-contain [&>svg]:h-full [&>svg]:w-full">
                    <ProviderIcon provider={config.provider} />
                  </div>
                </div>
                <div className="min-w-0 flex-1 text-sm text-slate-300">
                  <span className="block truncate font-medium text-ink">{labels[config.provider]}</span>
                  <span className="mt-0.5 block truncate text-xs text-slate-400">
                    {config.customName || "Main account"}
                  </span>
                </div>
                <button
                  type="button"
                  title="Edit account"
                  aria-label={`Edit ${labels[config.provider]} ${config.customName}`}
                  onClick={() => {
                    setDraft(null);
                    onSelectAccount(config.accountId);
                  }}
                  className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-slate-400 hover:bg-slate-800 hover:text-white"
                >
                  <Pencil className="h-4 w-4" />
                </button>
                <button
                  type="button"
                  title={config.visible ? "Hide card" : "Show card"}
                  aria-label={`${config.visible ? "Hide" : "Show"} ${labels[config.provider]} ${config.customName}`}
                  onClick={() => onSaveConfig({ ...config, visible: !config.visible })}
                  className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-slate-400 hover:bg-slate-800 hover:text-white"
                >
                  {config.visible ? <Eye className="h-4 w-4" /> : <EyeOff className="h-4 w-4" />}
                </button>
                <button
                  type="button"
                  title="Delete account"
                  aria-label={`Delete ${labels[config.provider]} ${config.customName}`}
                  disabled={saving}
                  onClick={() => setPendingDelete(config)}
                  className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-slate-400 hover:bg-rose-500/15 hover:text-rose-300 disabled:opacity-50"
                >
                  <Trash2 className="h-4 w-4" />
                </button>
              </div>
            ))}
          </div>

          {draft || selectedStored ? (
            <form
              className="space-y-4 rounded-lg border border-slate-800 bg-slate-950/40 p-4"
              onSubmit={async (event) => {
                event.preventDefault();
                await onSaveConfig(form);
                closeEditor();
              }}
            >
              <div className="flex items-center justify-between gap-3">
                <h3 className="m-0 text-sm font-semibold text-ink">
                  {form.accountId === 0 ? "Add account" : `Edit ${labels[form.provider]}`}
                </h3>
                <button
                  type="button"
                  title="Close editor"
                  aria-label="Close account editor"
                  onClick={closeEditor}
                  className="inline-flex h-8 w-8 items-center justify-center rounded-md text-slate-400 hover:bg-slate-800 hover:text-white"
                >
                  <X className="h-4 w-4" />
                </button>
              </div>
            <div>
              <label className="mb-2 block text-sm text-slate-300">Provider</label>
              <select
                value={form.provider}
                disabled={form.accountId !== 0}
                onChange={(event) => {
                  const provider = event.target.value as ProviderId;
                  updateForm({ provider, ...defaults[provider], apiKey: "" });
                }}
                className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-2.5 text-slate-100"
              >
                {Object.entries(labels).map(([provider, label]) => <option key={provider} value={provider}>{label}</option>)}
              </select>
            </div>
            <div>
              <label className="mb-2 block text-sm text-slate-300">Custom name</label>
              <input
                value={form.customName}
                onChange={(event) => updateForm({ customName: event.target.value })}
                placeholder="Work account"
                className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-2.5 text-slate-100 outline-none focus:border-slate-500"
              />
            </div>
            <div>
              <label className="mb-2 block text-sm text-slate-300">Authentication</label>
              <select
                value={form.authMethod}
                onChange={(event) => updateForm({ authMethod: event.target.value as ProviderConfig["authMethod"] })}
                className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-2.5 text-slate-100"
              >
                {form.provider === "minimax" || form.provider === "glm" ? (
                  <option value="api_key">API key</option>
                ) : (
                  <>
                    <option value="local_credential">Local application session</option>
                    <option value="oauth" disabled>OAuth login (next phase)</option>
                  </>
                )}
              </select>
            </div>
            <div>
              <label className="mb-2 block text-sm text-slate-300">
                {form.authMethod === "api_key" ? "API key" : "Credential source"}
              </label>
              <input
                value={form.apiKey}
                onChange={(event) => updateForm({ apiKey: event.target.value })}
                placeholder={form.authMethod === "api_key" ? "Enter key" : "Uses the local signed-in session"}
                disabled={form.authMethod !== "api_key"}
                className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-2.5 text-slate-100 outline-none focus:border-slate-500"
              />
            </div>
            <div>
              <label className="mb-2 block text-sm text-slate-300">Base URL</label>
              <input
                value={form.baseUrl}
                onChange={(event) => updateForm({ baseUrl: event.target.value })}
                className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-2.5 text-slate-100 outline-none focus:border-slate-500"
              />
            </div>
            <label className="flex items-center gap-3 rounded-md border border-slate-800 bg-slate-950 px-3 py-3 text-sm text-slate-300">
              <input type="checkbox" checked={form.enabled} onChange={(event) => updateForm({ enabled: event.target.checked })} className="h-4 w-4" />
              Enable auto refresh for this account
            </label>
            <AlertAccountSetting
              label="Enable quota threshold alerts"
              description={`Notify when a quota window reaches ${settings.notificationThreshold}% used`}
              checked={form.thresholdAlertEnabled}
              onChange={(thresholdAlertEnabled) => updateForm({ thresholdAlertEnabled })}
              onTest={() => onTestAlert(form.accountId, "threshold")}
              testDisabled={form.accountId === 0 || saving}
            />
            <AlertAccountSetting
              label="Enable quota reset alerts"
              description="Notify when usage drops from above 10% to below 5%"
              checked={form.resetAlertEnabled}
              onChange={(resetAlertEnabled) => updateForm({ resetAlertEnabled })}
              onTest={() => onTestAlert(form.accountId, "reset")}
              testDisabled={form.accountId === 0 || saving}
            />
            {form.provider === "google" ? (
              <label className="flex items-center justify-between gap-3 rounded-md border border-slate-800 bg-slate-950 px-3 py-3 text-sm text-slate-300">
                <span><span className="block font-medium">Two-column quota layout</span><span className="mt-1 block text-xs text-slate-500">Show two quota gauges per row</span></span>
                <input type="checkbox" checked={settings.antigravityTwoColumnQuota} onChange={(event) => onSaveSettings({ antigravityTwoColumnQuota: event.target.checked })} className="h-4 w-4" />
              </label>
            ) : null}
              <div className="flex flex-wrap items-center gap-2">
                <button type="submit" disabled={saving} className="action-button inline-flex items-center gap-2 rounded-md border px-3 py-2 text-sm font-medium disabled:opacity-60">
                  <Save className="h-4 w-4" />
                  {form.accountId === 0 ? "Add account" : "Save account"}
                </button>
                <button type="button" onClick={closeEditor} className="inline-flex items-center gap-2 rounded-md border border-slate-700 px-3 py-2 text-sm text-slate-300 hover:bg-slate-800 hover:text-white">
                  <X className="h-4 w-4" />
                  Cancel
                </button>
              </div>
            </form>
          ) : null}
        </div>

        <div className="space-y-5 rounded-lg border border-slate-800 bg-slate-950/40 p-4">
          <SegmentSetting label="Color theme" value={settings.colorTheme} options={[["system", "System", Laptop], ["dark", "Dark", Moon], ["light", "Light", Sun]]} onChange={(colorTheme) => onSaveSettings({ colorTheme })} />
          <SegmentSetting label="Interface size" value={settings.sizeTheme} options={[["compact", "Compact", Shrink], ["normal", "Normal", Laptop], ["large", "Large", Expand]]} onChange={(sizeTheme) => onSaveSettings({ sizeTheme })} />
          <ToggleRange label="Auto refresh" enabled={settings.autoRefreshEnabled} onToggle={() => onSaveSettings({ autoRefreshEnabled: !settings.autoRefreshEnabled })} valueLabel={`Every ${formatRelativeMinutes(settings.refreshIntervalMinutes)}`} value={settings.refreshIntervalMinutes} min={2} max={60} step={2} minLabel="2m" maxLabel="60m" onChange={(refreshIntervalMinutes) => onSaveSettings({ refreshIntervalMinutes })} />
          <ToggleRange label="Notifications" enabled={settings.notificationsEnabled} onToggle={() => onSaveSettings({ notificationsEnabled: !settings.notificationsEnabled })} valueLabel={`Alert at ${settings.notificationThreshold}% used`} value={settings.notificationThreshold} min={50} max={95} step={5} minLabel="50%" maxLabel="95%" onChange={(notificationThreshold) => onSaveSettings({ notificationThreshold })} />
        </div>
      </div>

      {pendingDelete ? (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/75 p-4 backdrop-blur-sm"
          role="presentation"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget && !saving) setPendingDelete(null);
          }}
        >
          <div
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="delete-account-title"
            aria-describedby="delete-account-description"
            className="w-full max-w-md rounded-lg border border-rose-500/30 bg-panel p-5 shadow-2xl"
          >
            <div className="flex items-start gap-3">
              <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-rose-500/15 text-rose-300">
                <AlertTriangle className="h-5 w-5" />
              </div>
              <div className="min-w-0 flex-1">
                <h3 id="delete-account-title" className="m-0 text-base font-semibold text-ink">
                  Delete this account?
                </h3>
                <p id="delete-account-description" className="m-0 mt-2 text-sm leading-6 text-slate-400">
                  <span className="font-medium text-slate-200">
                    {labels[pendingDelete.provider]} - {pendingDelete.customName || "Main account"}
                  </span>{" "}
                  and its saved quota history and local AI Bucket credential will be permanently removed.
                </p>
                <p className="m-0 mt-2 text-xs font-medium text-rose-300">This action cannot be undone.</p>
              </div>
            </div>
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                disabled={saving}
                onClick={() => setPendingDelete(null)}
                className="rounded-md border border-slate-700 px-3 py-2 text-sm text-slate-300 hover:bg-slate-800 hover:text-white disabled:opacity-50"
              >
                Cancel
              </button>
              <button
                type="button"
                disabled={saving}
                onClick={async () => {
                  await onDeleteConfig(pendingDelete.accountId);
                  setPendingDelete(null);
                }}
                className="inline-flex items-center gap-2 rounded-md border border-rose-500/40 bg-rose-500/15 px-3 py-2 text-sm font-medium text-rose-200 hover:bg-rose-500/25 disabled:opacity-50"
              >
                <Trash2 className="h-4 w-4" />
                Delete account
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </section>
  );
}

function SegmentSetting<T extends string>({ label, value, options, onChange }: { label: string; value: T; options: readonly (readonly [T, string, typeof Laptop])[]; onChange: (value: T) => void }) {
  return <div><label className="mb-2 block text-sm text-slate-300">{label}</label><div className="grid grid-cols-3 rounded-md border border-slate-700 bg-slate-950 p-1">{options.map(([option, text, Icon]) => <button key={option} type="button" onClick={() => onChange(option)} className={`inline-flex items-center justify-center gap-1.5 rounded px-2 py-2 text-xs ${value === option ? "bg-slate-700 text-white" : "text-slate-400 hover:bg-slate-800 hover:text-white"}`}><Icon className="h-3.5 w-3.5" />{text}</button>)}</div></div>;
}

function AlertAccountSetting({ label, description, checked, onChange, onTest, testDisabled }: { label: string; description: string; checked: boolean; onChange: (checked: boolean) => void; onTest: () => Promise<void>; testDisabled: boolean }) {
  return <div className="flex items-center gap-3 rounded-md border border-slate-800 bg-slate-950 px-3 py-3"><label className="flex min-w-0 flex-1 items-center gap-3 text-sm text-slate-300"><input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} className="h-4 w-4 shrink-0" /><span className="min-w-0"><span className="block font-medium">{label}</span><span className="mt-1 block text-xs text-slate-500">{description}</span></span></label><button type="button" title={testDisabled ? "Save this account before testing" : `Test ${label.toLowerCase()}`} aria-label={`Test ${label.toLowerCase()}`} disabled={testDisabled} onClick={() => void onTest()} className="action-button inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md border disabled:cursor-not-allowed disabled:opacity-40"><BellRing className="h-4 w-4" /></button></div>;
}

function ToggleRange({ label, enabled, onToggle, valueLabel, value, min, max, step, minLabel, maxLabel, onChange }: { label: string; enabled: boolean; onToggle: () => void; valueLabel: string; value: number; min: number; max: number; step: number; minLabel: string; maxLabel: string; onChange: (value: number) => void }) {
  return <div className="space-y-3 border-t border-slate-800 pt-4"><div className="flex items-center justify-between"><label className="text-sm font-medium text-slate-300">{label}</label><button type="button" onClick={onToggle} className={`rounded-md px-2.5 py-1.5 text-xs font-medium ${enabled ? "bg-emerald-500/20 text-emerald-300" : "bg-slate-800 text-slate-300"}`}>{enabled ? "On" : "Off"}</button></div><label className="block text-xs text-slate-400">{valueLabel}</label><input type="range" min={min} max={max} step={step} value={value} disabled={!enabled} onChange={(event) => onChange(Number(event.target.value))} className="range-control w-full" /><div className="flex justify-between text-xs text-slate-500"><span>{minLabel}</span><span>{maxLabel}</span></div></div>;
}

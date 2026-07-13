import { Bell, RefreshCw, ShieldCheck } from "lucide-react";
import { DndContext, DragEndEvent, PointerSensor, closestCenter, useSensor, useSensors } from "@dnd-kit/core";
import { SortableContext, rectSortingStrategy } from "@dnd-kit/sortable";
import { useEffect, useMemo, useState } from "react";
import { HistoryPanel } from "./components/HistoryPanel";
import { ProviderCard } from "./components/ProviderCard";
import { SettingsPanel } from "./components/SettingsPanel";
import mascotUrl from "./assets/ai-bucket-mascot.png";
import {
  getDashboardState,
  deleteProviderAccount,
  testProviderAlert,
  refreshAllProviders,
  refreshProvider,
  updateAppSettings,
  updateProviderConfig,
  reorderProviderAccounts
} from "./lib/tauri";
import type { DashboardState, ProviderId } from "./types";

function App() {
  const [state, setState] = useState<DashboardState | null>(null);
  const [refreshingProvider, setRefreshingProvider] = useState<number | "all" | null>(null);
  const [selectedAccountId, setSelectedAccountId] = useState<number | null>(null);
  const [savingConfig, setSavingConfig] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dragSensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 5 } }));

  useEffect(() => {
    void (async () => {
      try {
        setState(await getDashboardState());
      } catch (err) {
        setError(err instanceof Error ? err.message : "Failed to load dashboard");
      }
    })();
  }, []);

  useEffect(() => {
    if (!state) return;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const applyAppearance = () => {
      const colorTheme = state.settings.colorTheme ?? "system";
      const resolvedTheme = colorTheme === "system" ? (media.matches ? "dark" : "light") : colorTheme;
      document.documentElement.dataset.colorTheme = resolvedTheme;
      document.documentElement.dataset.themePreference = colorTheme;
      document.documentElement.dataset.sizeTheme = state.settings.sizeTheme ?? "normal";
    };
    applyAppearance();
    media.addEventListener("change", applyAppearance);
    return () => media.removeEventListener("change", applyAppearance);
  }, [state?.settings.colorTheme, state?.settings.sizeTheme]);

  useEffect(() => {
    if (!state?.settings.autoRefreshEnabled) {
      return;
    }

    const timer = window.setInterval(() => {
      void (async () => {
        try {
          setRefreshingProvider("all");
          const next = await refreshAllProviders();
          setState(next);
        } finally {
          setRefreshingProvider(null);
        }
      })();
    }, state.settings.refreshIntervalMinutes * 60_000);

    return () => window.clearInterval(timer);
  }, [state]);

  const alerts = useMemo(() => {
    if (!state) {
      return [];
    }

    return state.providers.filter((provider) => {
      return provider.limits.some(
        (limit) =>
          limit.total > 0 &&
          (limit.used / limit.total) * 100 >= state.settings.notificationThreshold
      );
    });
  }, [state]);

  if (!state) {
    return (
      <main className="flex min-h-screen items-center justify-center text-slate-300">
        Loading dashboard...
      </main>
    );
  }

  return (
    <main className="min-h-screen bg-slate-950 text-slate-100">
      <div className="mx-auto flex w-full max-w-7xl flex-col gap-6 px-4 py-6 sm:px-6 lg:px-8">
        <header className="relative rounded-lg border border-border bg-panel p-5 shadow-soft">
          <div className="flex items-center gap-4">
            <img
              src={mascotUrl}
              alt="AI Bucket mascot"
              className="h-20 w-20 shrink-0 object-contain sm:h-24 sm:w-24"
            />
            <div className="space-y-2">
              <div className="inline-flex items-center gap-2 rounded-full border border-slate-800 bg-slate-950/70 px-3 py-1 text-xs uppercase tracking-wide text-slate-400">
                <ShieldCheck className="h-3.5 w-3.5" />
                Windows quota monitor
              </div>
              <div>
                <h1 className="m-0 text-3xl font-semibold text-white">AI Bucket</h1>
                <p className="m-0 mt-2 max-w-2xl text-sm text-slate-400">
                  Quota viewer for Codex, Claude, Antigravity, MiniMax, and GLM.
                </p>
              </div>
            </div>
          </div>

          <div className="absolute right-3 top-3 flex items-center gap-1.5">
            <div
              className="inline-flex h-8 items-center gap-1.5 rounded-md border border-slate-800 bg-slate-950/60 px-2 text-xs text-slate-300"
              title={`${alerts.length} provider${alerts.length === 1 ? "" : "s"} near limit`}
            >
              <Bell className="h-4 w-4 text-amber-300" />
              <span>{alerts.length}</span>
            </div>
            <button
              title="Refresh all providers"
              aria-label="Refresh all providers"
              type="button"
              onClick={async () => {
                try {
                  setRefreshingProvider("all");
                  setState(await refreshAllProviders());
                } catch (err) {
                  setError(err instanceof Error ? err.message : "Refresh failed");
                } finally {
                  setRefreshingProvider(null);
                }
              }}
              disabled={refreshingProvider === "all"}
              className="action-button inline-flex h-8 w-8 items-center justify-center rounded-md border transition disabled:cursor-not-allowed disabled:opacity-70"
            >
              <RefreshCw
                className={`h-4 w-4 ${refreshingProvider === "all" ? "animate-spin" : ""}`}
              />
            </button>
          </div>
        </header>

        {error ? (
          <div className="rounded-lg border border-rose-500/40 bg-rose-500/10 px-4 py-3 text-sm text-rose-200">
            {error}
          </div>
        ) : null}

        <DndContext
          sensors={dragSensors}
          collisionDetection={closestCenter}
          onDragEnd={({ active, over }: DragEndEvent) => {
            if (!over || active.id === over.id) return;
            const visibleIds = state.providers.map((item) => item.accountId);
            const from = visibleIds.indexOf(Number(active.id));
            const to = visibleIds.indexOf(Number(over.id));
            if (from < 0 || to < 0) return;
            const reorderedVisible = [...visibleIds];
            const [moved] = reorderedVisible.splice(from, 1);
            reorderedVisible.splice(to, 0, moved);
            let visibleIndex = 0;
            const orderedIds = state.configs.map((config) =>
              config.visible ? reorderedVisible[visibleIndex++] : config.accountId
            );
            void reorderProviderAccounts(orderedIds).then(setState).catch((err) =>
              setError(err instanceof Error ? err.message : "Reorder failed")
            );
          }}
        >
          <SortableContext items={state.providers.map((provider) => provider.accountId)} strategy={rectSortingStrategy}>
          <section className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
          {state.providers.map((provider) => (
            <ProviderCard
              key={provider.accountId}
              provider={provider}
              twoColumnQuota={
                provider.provider === "google" && state.settings.antigravityTwoColumnQuota
              }
              refreshing={refreshingProvider === provider.accountId}
              onRefresh={async () => {
                try {
                  setRefreshingProvider(provider.accountId);
                  setState(await refreshProvider(provider.accountId));
                } catch (err) {
                  setError(err instanceof Error ? err.message : "Refresh failed");
                } finally {
                  setRefreshingProvider(null);
                }
              }}
            />
          ))}
          </section>
          </SortableContext>
        </DndContext>

        <SettingsPanel
          configs={state.configs}
          settings={state.settings}
          selectedAccountId={selectedAccountId}
          onSelectAccount={setSelectedAccountId}
          saving={savingConfig}
          onSaveConfig={async (config) => {
            try {
              setSavingConfig(true);
              const next = await updateProviderConfig(config);
              setState(next);
              if (config.accountId === 0) {
                const created = [...next.configs]
                  .reverse()
                  .find((item) => item.provider === config.provider && item.customName === config.customName);
                if (created) setSelectedAccountId(created.accountId);
              }
            } catch (err) {
              setError(err instanceof Error ? err.message : "Save failed");
            } finally {
              setSavingConfig(false);
            }
          }}
          onDeleteConfig={async (accountId) => {
            try {
              setSavingConfig(true);
              setState(await deleteProviderAccount(accountId));
              setSelectedAccountId(null);
            } catch (err) {
              setError(err instanceof Error ? err.message : "Delete failed");
            } finally {
              setSavingConfig(false);
            }
          }}
          onTestAlert={async (accountId, alertKind) => {
            try {
              await testProviderAlert(accountId, alertKind);
            } catch (err) {
              setError(err instanceof Error ? err.message : "Alert test failed");
            }
          }}
          onSaveSettings={async (patch) => {
            try {
              setState(await updateAppSettings(patch));
            } catch (err) {
              setError(err instanceof Error ? err.message : "Settings update failed");
            }
          }}
        />

        <HistoryPanel items={state.history} />
      </div>
    </main>
  );
}

export default App;

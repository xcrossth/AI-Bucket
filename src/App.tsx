import { Bell, Maximize2, PictureInPicture2, Power, RefreshCw, ShieldCheck } from "lucide-react";
import { DndContext, DragEndEvent, PointerSensor, closestCenter, useSensor, useSensors } from "@dnd-kit/core";
import { SortableContext, rectSortingStrategy } from "@dnd-kit/sortable";
import { useEffect, useMemo, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { HistoryPanel } from "./components/HistoryPanel";
import { ProviderCard } from "./components/ProviderCard";
import { SettingsPanel, WidgetAppearanceSettings } from "./components/SettingsPanel";
import mascotUrl from "./assets/ai-bucket-mascot.png";
import {
  getDashboardState,
  deleteProviderAccount,
  testProviderAlert,
  refreshAllProviders,
  refreshProvider,
  updateAppSettings,
  updateProviderConfig,
  reorderProviderAccounts,
  shutdownApp,
  startWindowDrag
} from "./lib/tauri";
import type { DashboardState } from "./types";

function App() {
  const [state, setState] = useState<DashboardState | null>(null);
  const [refreshingProvider, setRefreshingProvider] = useState<number | "all" | null>(null);
  const [selectedAccountId, setSelectedAccountId] = useState<number | null>(null);
  const [savingConfig, setSavingConfig] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmingShutdown, setConfirmingShutdown] = useState(false);
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
      const isWidget = state.settings.windowMode === "widget";
      const surface = isWidget ? Math.min(100, Math.max(40, state.settings.widgetOpacity)) : 100;
      const content = isWidget
        ? Math.min(100, surface + Math.min(30, Math.max(0, state.settings.foregroundOpacityBoost)))
        : 100;
      document.documentElement.dataset.windowMode = isWidget ? "widget" : "normal";
      document.documentElement.style.setProperty("--widget-surface-opacity", String(surface / 100));
      document.documentElement.style.setProperty("--widget-content-opacity", String(content / 100));
    };
    applyAppearance();
    media.addEventListener("change", applyAppearance);
    return () => media.removeEventListener("change", applyAppearance);
  }, [state?.settings.colorTheme, state?.settings.sizeTheme, state?.settings.windowMode, state?.settings.widgetOpacity, state?.settings.foregroundOpacityBoost]);

  useEffect(() => {
    if (!confirmingShutdown) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setConfirmingShutdown(false);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [confirmingShutdown]);

  useEffect(() => {
    let hideTimer: number | undefined;
    const showScrollbar = () => {
      document.documentElement.dataset.scrollbarVisible = "true";
      window.clearTimeout(hideTimer);
      hideTimer = window.setTimeout(() => {
        delete document.documentElement.dataset.scrollbarVisible;
      }, 1000);
    };
    const showScrollbarForPageKey = (event: KeyboardEvent) => {
      if (event.key === "PageUp" || event.key === "PageDown") showScrollbar();
    };
    window.addEventListener("wheel", showScrollbar, { passive: true });
    window.addEventListener("keydown", showScrollbarForPageKey);
    return () => {
      window.clearTimeout(hideTimer);
      window.removeEventListener("wheel", showScrollbar);
      window.removeEventListener("keydown", showScrollbarForPageKey);
      delete document.documentElement.dataset.scrollbarVisible;
    };
  }, []);

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

  const dragFromEmptySurface = (event: ReactMouseEvent<HTMLElement>) => {
    if (state?.settings.windowMode !== "widget" || event.button !== 0 || event.target !== event.currentTarget) return;
    void startWindowDrag();
  };

  const dragFromWidgetHeader = (event: ReactMouseEvent<HTMLElement>) => {
    if (state?.settings.windowMode !== "widget" || event.button !== 0) return;
    const target = event.target as HTMLElement;
    if (target.closest("button, input, select, textarea, a, [role='button']")) return;
    event.preventDefault();
    void startWindowDrag();
  };

  if (!state) {
    return (
      <main className="flex min-h-screen items-center justify-center text-slate-300">
        Loading dashboard...
      </main>
    );
  }

  const isWidget = state.settings.windowMode === "widget";
  const saveSettings = async (patch: Partial<DashboardState["settings"]>) => {
    try {
      setState(await updateAppSettings(patch));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Settings update failed");
    }
  };

  return (
    <main className={`app-shell min-h-screen text-slate-100 ${isWidget ? "widget-mode h-screen overflow-hidden" : "bg-slate-950"}`} onMouseDown={dragFromEmptySurface}>
      <div className={`app-layout mx-auto flex w-full max-w-7xl flex-col ${isWidget ? "h-screen min-h-0 gap-3 overflow-hidden p-3" : "gap-6 px-4 py-6 sm:px-6 lg:px-8"}`} onMouseDown={dragFromEmptySurface}>
        <header className={`widget-surface relative shrink-0 rounded-lg border border-border bg-panel shadow-soft ${isWidget ? "p-3" : "p-5"}`} onMouseDown={dragFromWidgetHeader}>
          <div className="flex items-center gap-4">
            <img
              src={mascotUrl}
              alt="AI Bucket mascot"
              className={`widget-content shrink-0 object-contain ${isWidget ? "h-12 w-12" : "h-20 w-20 sm:h-24 sm:w-24"}`}
            />
            <div className={isWidget ? "min-w-0" : "space-y-2"}>
              {!isWidget ? <div className="widget-content inline-flex items-center gap-2 rounded-full border border-slate-800 bg-slate-950/70 px-3 py-1 text-xs uppercase tracking-wide text-slate-400">
                <ShieldCheck className="h-3.5 w-3.5" />
                Windows quota monitor
              </div> : null}
              <div>
                <h1 className={`widget-content m-0 font-semibold text-white ${isWidget ? "text-xl" : "text-3xl"}`}>AI Bucket</h1>
                <p className={`widget-content m-0 max-w-2xl truncate text-slate-400 ${isWidget ? "mt-1 pr-28 text-xs" : "mt-2 text-sm"}`}>
                  Quota viewer for Codex, Claude, Antigravity, MiniMax, and GLM.
                </p>
              </div>
            </div>
          </div>

          <div className="absolute right-3 top-3 flex items-center gap-1.5">
            <div
              className="widget-control inline-flex h-8 items-center gap-1.5 rounded-md border border-slate-800 bg-slate-950/60 px-2 text-xs text-slate-300"
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
              className="widget-control action-button inline-flex h-8 w-8 items-center justify-center rounded-md border transition disabled:cursor-not-allowed disabled:opacity-70"
            >
              <RefreshCw
                className={`h-4 w-4 ${refreshingProvider === "all" ? "animate-spin" : ""}`}
              />
            </button>
            <button
              title={isWidget ? "Return to normal mode" : "Switch to Widget mode"}
              aria-label={isWidget ? "Return to normal mode" : "Switch to Widget mode"}
              type="button"
              onClick={() => void updateAppSettings({ windowMode: isWidget ? "normal" : "widget" }).then(setState).catch((err) => setError(err instanceof Error ? err.message : "Window mode update failed"))}
              className="widget-control action-button inline-flex h-8 w-8 items-center justify-center rounded-md border"
            >
              {isWidget ? <Maximize2 className="h-4 w-4" /> : <PictureInPicture2 className="h-4 w-4" />}
            </button>
            <button
              title="Exit AI Bucket"
              aria-label="Exit AI Bucket"
              type="button"
              onClick={() => setConfirmingShutdown(true)}
              className="widget-control action-button inline-flex h-8 w-8 items-center justify-center rounded-md border hover:text-rose-300"
            >
              <Power className="h-4 w-4" />
            </button>
          </div>
        </header>

        <div className={isWidget ? "widget-scroll-content flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto" : "contents"}>
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
          <section className={`provider-card-grid grid gap-4 ${isWidget ? "widget-card-grid" : "md:grid-cols-2 xl:grid-cols-3"}`}>
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

        {!isWidget ? <SettingsPanel
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
          onSaveSettings={saveSettings}
        /> : null}

        {!isWidget ? <HistoryPanel items={state.history} /> : null}

        {isWidget ? (
          <section className="widget-surface rounded-lg border border-border bg-panel p-4 shadow-soft">
            <WidgetAppearanceSettings settings={state.settings} onSaveSettings={saveSettings} />
          </section>
        ) : null}
        </div>
      </div>

      {confirmingShutdown ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/70 p-4 backdrop-blur-sm" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) setConfirmingShutdown(false); }}>
          <div role="alertdialog" aria-modal="true" aria-labelledby="shutdown-title" aria-describedby="shutdown-description" className="widget-surface w-full max-w-sm rounded-lg border border-border bg-panel p-5 shadow-2xl">
            <h2 id="shutdown-title" className="widget-content m-0 text-lg font-semibold text-ink">Exit AI Bucket?</h2>
            <p id="shutdown-description" className="widget-content m-0 mt-2 text-sm leading-6 text-slate-400">The current window position, size, and mode will be saved before the app closes.</p>
            <div className="mt-5 flex justify-end gap-2">
              <button type="button" onClick={() => setConfirmingShutdown(false)} className="widget-control rounded-md border border-slate-700 px-3 py-2 text-sm text-slate-300 hover:bg-slate-800 hover:text-white">Cancel</button>
              <button type="button" onClick={() => void shutdownApp()} className="widget-control inline-flex items-center gap-2 rounded-md border border-rose-500/40 bg-rose-500/15 px-3 py-2 text-sm font-medium text-rose-200 hover:bg-rose-500/25"><Power className="h-4 w-4" />Exit</button>
            </div>
          </div>
        </div>
      ) : null}
    </main>
  );
}

export default App;

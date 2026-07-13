import type {
  DashboardState,
  LimitMetric,
  ProviderConfig,
  ProviderId,
  ProviderSnapshot,
  ProviderStatus,
  UsageHistoryEntry
} from "../types";

const storageKey = "ai-bucket-dashboard-v4";

function isoPlusMinutes(minutes: number): string {
  return new Date(Date.now() + minutes * 60_000).toISOString();
}

function limit(
  id: string,
  label: string,
  used: number,
  total: number,
  resetMinutes: number
): LimitMetric {
  return { id, label, used, total, resetAt: isoPlusMinutes(resetMinutes) };
}

function defaultConfigs(): ProviderConfig[] {
  return [
    {
      accountId: 1,
      provider: "openai",
      customName: "",
      authMethod: "local_credential",
      apiKey: "",
      baseUrl: "https://chatgpt.com/backend-api/wham/usage",
      enabled: true, thresholdAlertEnabled: true, resetAlertEnabled: true, visible: true, sortOrder: 0
    },
    {
      accountId: 2,
      provider: "claude",
      customName: "",
      authMethod: "local_credential",
      apiKey: "",
      baseUrl: "https://claude.ai/api/organizations/{org_id}/usage",
      enabled: true, thresholdAlertEnabled: true, resetAlertEnabled: true, visible: true, sortOrder: 1
    },
    {
      accountId: 3,
      provider: "google",
      customName: "",
      authMethod: "local_credential",
      apiKey: "",
      baseUrl: "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota",
      enabled: true, thresholdAlertEnabled: true, resetAlertEnabled: true, visible: true, sortOrder: 2
    },
    {
      accountId: 4,
      provider: "minimax",
      customName: "",
      authMethod: "api_key",
      apiKey: "",
      baseUrl: "https://www.minimax.io/v1/token_plan/remains",
      enabled: true, thresholdAlertEnabled: true, resetAlertEnabled: true, visible: true, sortOrder: 3
    },
    {
      accountId: 5,
      provider: "glm",
      customName: "",
      authMethod: "api_key",
      apiKey: "",
      baseUrl: "https://api.z.ai/api/monitor/usage/quota/limit",
      enabled: true, thresholdAlertEnabled: true, resetAlertEnabled: true, visible: true, sortOrder: 4
    }
  ];
}

function defaultState(): DashboardState {
  const now = new Date().toISOString();
  return {
    providers: [
      {
        accountId: 1,
        provider: "openai",
        customName: "",
        providerName: "OpenAI Codex",
        icon: "OA",
        accentColor: "#10A37F",
        planName: "ChatGPT plan",
        status: "ready",
        fetchMode: "local_credential",
        limits: [
          limit("session", "session (5h)", 34, 100, 92),
          limit("weekly", "weekly (7d)", 61, 100, 620),
          limit("spark-session", "Codex Spark (5h)", 8, 100, 150)
        ],
        lastUpdated: now,
        notes: "Uses the local Codex CLI auth file in desktop mode.",
        configured: true
      },
      {
        accountId: 2,
        provider: "claude",
        customName: "",
        providerName: "Claude",
        icon: "C",
        accentColor: "#D97757",
        planName: "Claude subscription",
        status: "needs_auth",
        fetchMode: "local_credential",
        limits: [
          limit("session", "session (5h)", 19, 100, 180),
          limit("weekly", "weekly (7d)", 42, 100, 4_200)
        ],
        lastUpdated: now,
        notes: "Uses the local Claude Desktop session in desktop mode.",
        configured: false
      },
      {
        accountId: 3,
        provider: "google",
        customName: "",
        providerName: "Google Antigravity",
        icon: "AG",
        accentColor: "#4F8CFF",
        planName: "Antigravity",
        status: "needs_auth",
        fetchMode: "local_credential",
        limits: [],
        lastUpdated: now,
        notes: "Uses the local Antigravity OAuth session in desktop mode.",
        configured: true
      },
      {
        accountId: 4,
        provider: "minimax",
        customName: "",
        providerName: "MiniMax",
        icon: "MM",
        accentColor: "#FF6B35",
        planName: "Coding Plan",
        status: "placeholder",
        fetchMode: "api_key",
        limits: [
          limit("session", "session (5h)", 21, 100, 240),
          limit("weekly", "weekly (7d)", 20, 100, 5_400)
        ],
        lastUpdated: now,
        notes: "API-key quota refresh is available in desktop mode.",
        configured: false
      },
      {
        accountId: 5,
        provider: "glm",
        customName: "",
        providerName: "GLM / Z.AI",
        icon: "Z",
        accentColor: "#7C3AED",
        planName: "Coding Plan",
        status: "needs_config",
        fetchMode: "api_key",
        limits: [
          limit("session", "5 Hours Quota", 16, 100, 180),
          limit("weekly", "Weekly Quota", 18, 100, 4_200)
        ],
        lastUpdated: now,
        notes: "Add a coding-plan API key to activate upstream quota refresh.",
        configured: false
      }
    ],
    history: [],
    configs: defaultConfigs(),
    settings: {
      autoRefreshEnabled: true,
      refreshIntervalMinutes: 10,
      notificationThreshold: 80,
      notificationsEnabled: true,
      colorTheme: "system",
      sizeTheme: "normal",
      antigravityTwoColumnQuota: true
    }
  };
}

function historyForProvider(
  provider: ProviderSnapshot,
  startingId: number
): UsageHistoryEntry[] {
  return provider.limits.map((item, index) => ({
    id: startingId + index,
    provider: provider.provider,
    customName: provider.customName,
    limitId: item.id,
    limitLabel: item.label,
    usedValue: item.used,
    totalValue: item.total,
    percentage: item.total > 0 ? Math.round((item.used / item.total) * 100) : 0,
    recordedAt: provider.lastUpdated
  }));
}

function hydrateHistory(state: DashboardState): DashboardState {
  if (state.history.length > 0) return state;
  return {
    ...state,
    history: state.providers.flatMap((provider, index) =>
      historyForProvider(provider, index * 100 + 1)
    )
  };
}

export function loadMockState(): DashboardState {
  const raw = localStorage.getItem(storageKey);
  if (raw) {
    try {
      const parsed = JSON.parse(raw) as DashboardState;
      if (parsed.providers?.every((provider) => Array.isArray(provider.limits))) {
        return hydrateHistory({
          ...parsed,
          settings: { ...defaultState().settings, ...parsed.settings }
        });
      }
    } catch {
      // Recreate invalid or pre-v2 browser state below.
    }
  }

  const initial = hydrateHistory(defaultState());
  localStorage.setItem(storageKey, JSON.stringify(initial));
  return initial;
}

function saveMockState(state: DashboardState): DashboardState {
  localStorage.setItem(storageKey, JSON.stringify(state));
  return state;
}

function bumpSnapshot(snapshot: ProviderSnapshot): ProviderSnapshot {
  const now = new Date();
  const tick = (now.getMinutes() % 4) + 1;
  return {
    ...snapshot,
    limits: snapshot.limits.map((item, index) => ({
      ...item,
      used: Math.min(item.total, item.used + tick + index),
      resetAt: isoPlusMinutes(index === 0 ? 180 : 4_200)
    })),
    lastUpdated: now.toISOString()
  };
}

export async function mockRefreshProvider(accountId: number): Promise<DashboardState> {
  const state = loadMockState();
  const providers = state.providers.map((provider) =>
    provider.accountId === accountId ? bumpSnapshot(provider) : provider
  );
  const target = providers.find((provider) => provider.accountId === accountId)!;
  const history = [
    ...historyForProvider(target, Date.now()),
    ...state.history
  ].slice(0, 150);

  return saveMockState({ ...state, providers, history });
}

export async function mockRefreshAll(): Promise<DashboardState> {
  let state = loadMockState();
  for (const provider of state.providers) {
    state = await mockRefreshProvider(provider.accountId);
  }
  return state;
}

export async function mockUpdateConfig(config: ProviderConfig): Promise<DashboardState> {
  const state = loadMockState();
  const accountId = config.accountId || Math.max(0, ...state.configs.map((item) => item.accountId)) + 1;
  const saved = { ...config, accountId };
  const configs = config.accountId === 0
    ? [...state.configs, saved]
    : state.configs.map((item) => item.accountId === config.accountId ? saved : item);
  const providers = state.providers.map((provider) => {
    if (provider.accountId !== config.accountId) return provider;

    const localOrOauth = provider.provider === "openai" || provider.provider === "claude";
    const status: ProviderStatus = localOrOauth
      ? provider.status
      : config.apiKey
        ? "placeholder"
        : "needs_config";
    return {
      ...provider,
      configured: localOrOauth ? provider.configured : Boolean(config.apiKey),
      status
    };
  });
  return saveMockState({ ...state, configs, providers });
}

export async function mockDeleteAccount(accountId: number): Promise<DashboardState> {
  const state = loadMockState();
  const configs = state.configs
    .filter((config) => config.accountId !== accountId)
    .map((config, index) => ({ ...config, sortOrder: index }));
  return saveMockState({
    ...state,
    configs,
    providers: state.providers.filter((provider) => provider.accountId !== accountId),
    history: state.history.filter((entry) => {
      const deleted = state.configs.find((config) => config.accountId === accountId);
      return !deleted || entry.provider !== deleted.provider || entry.customName !== deleted.customName;
    })
  });
}

export async function mockUpdateSettings(
  patch: Partial<DashboardState["settings"]>
): Promise<DashboardState> {
  const state = loadMockState();
  return saveMockState({
    ...state,
    settings: { ...state.settings, ...patch }
  });
}

export async function mockReorderAccounts(accountIds: number[]): Promise<DashboardState> {
  const state = loadMockState();
  const order = new Map(accountIds.map((id, index) => [id, index]));
  return saveMockState({
    ...state,
    providers: [...state.providers].sort(
      (left, right) => (order.get(left.accountId) ?? 999) - (order.get(right.accountId) ?? 999)
    ),
    configs: [...state.configs]
      .map((config) => ({ ...config, sortOrder: order.get(config.accountId) ?? config.sortOrder }))
      .sort((left, right) => left.sortOrder - right.sortOrder)
  });
}

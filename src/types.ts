export type ProviderId = "openai" | "claude" | "google" | "minimax" | "glm";

export type ProviderStatus =
  | "ready"
  | "needs_auth"
  | "needs_config"
  | "error"
  | "placeholder";

export interface LimitMetric {
  id: string;
  label: string;
  resource?: string;
  used: number;
  total: number;
  resetAt?: string;
  windowSeconds?: number;
}

export interface ProviderSnapshot {
  accountId: number;
  provider: ProviderId;
  providerName: string;
  icon: string;
  accentColor: string;
  planName: string;
  status: ProviderStatus;
  fetchMode: "local_credential" | "oauth" | "api_key" | "placeholder";
  limits: LimitMetric[];
  lastUpdated: string;
  notes: string;
  configured: boolean;
  customName: string;
}

export interface UsageHistoryEntry {
  id: number;
  provider: ProviderId;
  customName: string;
  limitId: string;
  limitLabel: string;
  usedValue: number;
  totalValue: number;
  percentage: number;
  recordedAt: string;
}

export interface ProviderConfig {
  accountId: number;
  provider: ProviderId;
  customName: string;
  authMethod: "local_credential" | "oauth" | "api_key";
  apiKey: string;
  baseUrl: string;
  enabled: boolean;
  thresholdAlertEnabled: boolean;
  resetAlertEnabled: boolean;
  visible: boolean;
  sortOrder: number;
}

export interface AppSettings {
  autoRefreshEnabled: boolean;
  refreshIntervalMinutes: number;
  notificationThreshold: number;
  notificationsEnabled: boolean;
  colorTheme: "system" | "dark" | "light";
  sizeTheme: "compact" | "normal" | "large";
  antigravityTwoColumnQuota: boolean;
}

export interface DashboardState {
  providers: ProviderSnapshot[];
  history: UsageHistoryEntry[];
  configs: ProviderConfig[];
  settings: AppSettings;
}

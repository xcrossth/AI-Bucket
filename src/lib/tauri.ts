import { invoke } from "@tauri-apps/api/core";
import type { DashboardState, ProviderConfig, ProviderId } from "../types";
import {
  loadMockState,
  mockRefreshAll,
  mockRefreshProvider,
  mockDeleteAccount,
  mockReorderAccounts,
  mockUpdateConfig,
  mockUpdateSettings
} from "./mock";

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function getDashboardState(): Promise<DashboardState> {
  if (!isTauriRuntime()) {
    return loadMockState();
  }

  return invoke<DashboardState>("get_dashboard_state");
}

export async function refreshProvider(accountId: number): Promise<DashboardState> {
  if (!isTauriRuntime()) {
    return mockRefreshProvider(accountId);
  }

  return invoke<DashboardState>("refresh_provider", { accountId });
}

export async function reorderProviderAccounts(accountIds: number[]): Promise<DashboardState> {
  if (!isTauriRuntime()) {
    return mockReorderAccounts(accountIds);
  }
  return invoke<DashboardState>("reorder_provider_accounts", { accountIds });
}

export async function refreshAllProviders(): Promise<DashboardState> {
  if (!isTauriRuntime()) {
    return mockRefreshAll();
  }

  return invoke<DashboardState>("refresh_all_providers");
}

export async function updateProviderConfig(config: ProviderConfig): Promise<DashboardState> {
  if (!isTauriRuntime()) {
    return mockUpdateConfig(config);
  }

  return invoke<DashboardState>("save_provider_config", { config });
}

export async function deleteProviderAccount(accountId: number): Promise<DashboardState> {
  if (!isTauriRuntime()) {
    return mockDeleteAccount(accountId);
  }

  return invoke<DashboardState>("delete_provider_account", { accountId });
}

export async function testProviderAlert(
  accountId: number,
  alertKind: "threshold" | "reset"
): Promise<void> {
  if (!isTauriRuntime()) {
    return;
  }

  return invoke<void>("test_provider_alert", { accountId, alertKind });
}

export async function updateAppSettings(
  patch: Partial<DashboardState["settings"]>
): Promise<DashboardState> {
  if (!isTauriRuntime()) {
    return mockUpdateSettings(patch);
  }

  return invoke<DashboardState>("save_app_settings", { patch });
}

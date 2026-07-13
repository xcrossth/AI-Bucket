use serde::{Deserialize, Serialize};

pub const PROVIDER_IDS: [&str; 5] = ["openai", "claude", "google", "minimax", "glm"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitMetric {
    pub id: String,
    pub label: String,
    pub resource: Option<String>,
    pub used: f64,
    pub total: f64,
    #[serde(rename = "resetAt")]
    pub reset_at: Option<String>,
    #[serde(rename = "windowSeconds")]
    pub window_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSnapshot {
    #[serde(rename = "accountId")]
    pub account_id: i64,
    pub provider: String,
    #[serde(rename = "providerName")]
    pub provider_name: String,
    pub icon: String,
    #[serde(rename = "accentColor")]
    pub accent_color: String,
    #[serde(rename = "planName")]
    pub plan_name: String,
    pub status: String,
    #[serde(rename = "fetchMode")]
    pub fetch_mode: String,
    pub limits: Vec<LimitMetric>,
    #[serde(rename = "lastUpdated")]
    pub last_updated: String,
    pub notes: String,
    pub configured: bool,
    #[serde(rename = "customName")]
    pub custom_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageHistoryEntry {
    pub id: i64,
    pub provider: String,
    #[serde(rename = "customName")]
    pub custom_name: String,
    #[serde(rename = "limitId")]
    pub limit_id: String,
    #[serde(rename = "limitLabel")]
    pub limit_label: String,
    #[serde(rename = "usedValue")]
    pub used_value: f64,
    #[serde(rename = "totalValue")]
    pub total_value: f64,
    pub percentage: f64,
    #[serde(rename = "recordedAt")]
    pub recorded_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(rename = "accountId")]
    pub account_id: i64,
    pub provider: String,
    #[serde(rename = "customName")]
    pub custom_name: String,
    #[serde(rename = "authMethod")]
    pub auth_method: String,
    #[serde(rename = "apiKey")]
    pub api_key: String,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub enabled: bool,
    #[serde(rename = "thresholdAlertEnabled")]
    pub threshold_alert_enabled: bool,
    #[serde(rename = "resetAlertEnabled")]
    pub reset_alert_enabled: bool,
    pub visible: bool,
    #[serde(rename = "sortOrder")]
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(rename = "autoRefreshEnabled")]
    pub auto_refresh_enabled: bool,
    #[serde(rename = "refreshIntervalMinutes")]
    pub refresh_interval_minutes: i64,
    #[serde(rename = "notificationThreshold")]
    pub notification_threshold: i64,
    #[serde(rename = "notificationsEnabled")]
    pub notifications_enabled: bool,
    #[serde(rename = "colorTheme")]
    pub color_theme: String,
    #[serde(rename = "sizeTheme")]
    pub size_theme: String,
    #[serde(rename = "antigravityTwoColumnQuota")]
    pub antigravity_two_column_quota: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardState {
    pub providers: Vec<ProviderSnapshot>,
    pub history: Vec<UsageHistoryEntry>,
    pub configs: Vec<ProviderConfig>,
    pub settings: AppSettings,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SettingsPatch {
    #[serde(rename = "autoRefreshEnabled")]
    pub auto_refresh_enabled: Option<bool>,
    #[serde(rename = "refreshIntervalMinutes")]
    pub refresh_interval_minutes: Option<i64>,
    #[serde(rename = "notificationThreshold")]
    pub notification_threshold: Option<i64>,
    #[serde(rename = "notificationsEnabled")]
    pub notifications_enabled: Option<bool>,
    #[serde(rename = "colorTheme")]
    pub color_theme: Option<String>,
    #[serde(rename = "sizeTheme")]
    pub size_theme: Option<String>,
    #[serde(rename = "antigravityTwoColumnQuota")]
    pub antigravity_two_column_quota: Option<bool>,
}

pub fn provider_presentation(provider: &str) -> (&'static str, &'static str, &'static str) {
    match provider {
        "openai" => ("OpenAI Codex", "OA", "#10A37F"),
        "claude" => ("Claude", "C", "#D97757"),
        "google" => ("Google Antigravity", "AG", "#4F8CFF"),
        "minimax" => ("MiniMax", "MM", "#FF6B35"),
        "glm" => ("GLM / Z.AI", "Z", "#7C3AED"),
        _ => ("Unknown provider", "?", "#64748B"),
    }
}

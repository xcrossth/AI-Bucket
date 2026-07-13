use std::path::Path;

use rusqlite::{params, Connection};

use crate::{
    credentials,
    models::{
        provider_presentation, AppSettings, DashboardState, LimitMetric, ProviderConfig,
        ProviderSnapshot, UsageHistoryEntry,
    },
};

fn default_settings() -> AppSettings {
    AppSettings {
        auto_refresh_enabled: true,
        refresh_interval_minutes: 10,
        notification_threshold: 80,
        notifications_enabled: true,
        color_theme: "system".into(),
        size_theme: "normal".into(),
        antigravity_two_column_quota: true,
    }
}

fn default_configs() -> Vec<ProviderConfig> {
    vec![
        ProviderConfig {
            account_id: 0,
            provider: "openai".into(),
            custom_name: String::new(),
            auth_method: "local_credential".into(),
            api_key: String::new(),
            base_url: "https://chatgpt.com/backend-api/wham/usage".into(),
            enabled: true,
            threshold_alert_enabled: true,
            reset_alert_enabled: true,
            visible: true,
            sort_order: 0,
        },
        ProviderConfig {
            account_id: 0,
            provider: "claude".into(),
            custom_name: String::new(),
            auth_method: "local_credential".into(),
            api_key: String::new(),
            base_url: "https://claude.ai/api/organizations/{org_id}/usage".into(),
            enabled: true,
            threshold_alert_enabled: true,
            reset_alert_enabled: true,
            visible: true,
            sort_order: 1,
        },
        ProviderConfig {
            account_id: 0,
            provider: "google".into(),
            custom_name: String::new(),
            auth_method: "local_credential".into(),
            api_key: String::new(),
            base_url: "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota".into(),
            enabled: true,
            threshold_alert_enabled: true,
            reset_alert_enabled: true,
            visible: true,
            sort_order: 2,
        },
        ProviderConfig {
            account_id: 0,
            provider: "minimax".into(),
            custom_name: String::new(),
            auth_method: "api_key".into(),
            api_key: String::new(),
            base_url: "https://www.minimax.io/v1/token_plan/remains".into(),
            enabled: true,
            threshold_alert_enabled: true,
            reset_alert_enabled: true,
            visible: true,
            sort_order: 3,
        },
        ProviderConfig {
            account_id: 0,
            provider: "glm".into(),
            custom_name: String::new(),
            auth_method: "api_key".into(),
            api_key: String::new(),
            base_url: "https://api.z.ai/api/monitor/usage/quota/limit".into(),
            enabled: true,
            threshold_alert_enabled: true,
            reset_alert_enabled: true,
            visible: true,
            sort_order: 4,
        },
    ]
}

fn first_run_configs(detected_local_providers: &[String]) -> Vec<ProviderConfig> {
    let mut configs = default_configs();
    for config in &mut configs {
        if detected_local_providers.contains(&config.provider) {
            config.custom_name = "Local Config".into();
        }
    }
    configs.sort_by_key(|config| !detected_local_providers.contains(&config.provider));
    for (index, config) in configs.iter_mut().enumerate() {
        config.sort_order = index as i64;
    }
    configs
}

fn default_snapshot(
    account_id: i64,
    provider: &str,
    custom_name: &str,
    checked_at: &str,
) -> ProviderSnapshot {
    let (provider_name, icon, accent_color) = provider_presentation(provider);
    let (plan, status, fetch_mode, notes, configured) = match provider {
        "openai" => (
            "ChatGPT plan",
            "placeholder",
            "local_credential",
            "Refresh to load quota from the local Codex CLI auth file.",
            true,
        ),
        "claude" => (
            "Claude subscription",
            "needs_auth",
            "local_credential",
            "Refresh to load quota from the local Claude Desktop session.",
            false,
        ),
        "google" => (
            "Antigravity",
            "needs_auth",
            "local_credential",
            "Refresh to load quota from the local Antigravity OAuth session.",
            true,
        ),
        "minimax" => (
            "Coding Plan",
            "needs_config",
            "api_key",
            "Add a MiniMax Token/Coding Plan API key.",
            false,
        ),
        "glm" => (
            "Coding Plan",
            "needs_config",
            "api_key",
            "Add a GLM/Z.AI Coding Plan API key.",
            false,
        ),
        _ => (
            "Unknown",
            "error",
            "placeholder",
            "Unknown provider.",
            false,
        ),
    };
    ProviderSnapshot {
        account_id,
        provider: provider.into(),
        provider_name: provider_name.into(),
        icon: icon.into(),
        accent_color: accent_color.into(),
        plan_name: plan.into(),
        status: status.into(),
        fetch_mode: fetch_mode.into(),
        limits: Vec::new(),
        last_updated: checked_at.into(),
        notes: notes.into(),
        configured,
        custom_name: custom_name.into(),
    }
}

pub fn new_account_snapshot(
    account_id: i64,
    provider: &str,
    custom_name: &str,
    checked_at: &str,
) -> ProviderSnapshot {
    default_snapshot(account_id, provider, custom_name, checked_at)
}

pub fn init_db(
    conn: &Connection,
    credential_dir: &Path,
    now: &str,
    detected_local_providers: &[String],
) -> Result<(), String> {
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS provider_configs (
            provider TEXT PRIMARY KEY,
            api_key TEXT NOT NULL,
            base_url TEXT NOT NULL,
            enabled INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS provider_accounts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            provider TEXT NOT NULL,
            custom_name TEXT NOT NULL DEFAULT '',
            auth_method TEXT NOT NULL,
            credential_ref TEXT NOT NULL DEFAULT '',
            base_url TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            threshold_alert_enabled INTEGER NOT NULL DEFAULT 1,
            reset_alert_enabled INTEGER NOT NULL DEFAULT 1,
            visible INTEGER NOT NULL DEFAULT 1,
            sort_order INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS app_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            auto_refresh_enabled INTEGER NOT NULL,
            refresh_interval_minutes INTEGER NOT NULL,
            notification_threshold INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS provider_snapshots_v2 (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            provider TEXT NOT NULL,
            plan_name TEXT NOT NULL,
            status TEXT NOT NULL,
            fetch_mode TEXT NOT NULL,
            checked_at TEXT NOT NULL,
            notes TEXT NOT NULL,
            configured INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS quota_limits_v2 (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            snapshot_id INTEGER NOT NULL REFERENCES provider_snapshots_v2(id) ON DELETE CASCADE,
            limit_key TEXT NOT NULL,
            label TEXT NOT NULL,
            resource TEXT,
            used REAL NOT NULL,
            total REAL NOT NULL,
            reset_at TEXT,
            window_seconds INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_provider_snapshots_v2_provider
            ON provider_snapshots_v2(provider, id DESC);
        CREATE INDEX IF NOT EXISTS idx_quota_limits_v2_snapshot
            ON quota_limits_v2(snapshot_id);
        "#,
    )
    .map_err(|error| error.to_string())?;

    let _ = conn.execute(
        "ALTER TABLE app_settings ADD COLUMN color_theme TEXT NOT NULL DEFAULT 'system'",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE app_settings ADD COLUMN size_theme TEXT NOT NULL DEFAULT 'normal'",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE app_settings ADD COLUMN antigravity_two_column_quota INTEGER NOT NULL DEFAULT 1",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE app_settings ADD COLUMN notifications_enabled INTEGER NOT NULL DEFAULT 1",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE provider_snapshots_v2 ADD COLUMN account_id INTEGER",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE provider_accounts ADD COLUMN threshold_alert_enabled INTEGER NOT NULL DEFAULT 1",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE provider_accounts ADD COLUMN reset_alert_enabled INTEGER NOT NULL DEFAULT 1",
        [],
    );

    for config in default_configs() {
        conn.execute(
            "INSERT OR IGNORE INTO provider_configs (provider, api_key, base_url, enabled)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                config.provider,
                config.api_key,
                config.base_url,
                config.enabled as i64
            ],
        )
        .map_err(|error| error.to_string())?;
    }

    conn.execute(
        "UPDATE provider_configs
         SET base_url = 'https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota',
             api_key = ''
         WHERE provider = 'google'
           AND base_url = 'https://generativelanguage.googleapis.com/v1beta/openai'",
        [],
    )
    .map_err(|error| error.to_string())?;
    let settings = default_settings();
    conn.execute(
        "INSERT OR IGNORE INTO app_settings
         (id, auto_refresh_enabled, refresh_interval_minutes, notification_threshold,
          color_theme, size_theme, antigravity_two_column_quota, notifications_enabled)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            settings.auto_refresh_enabled as i64,
            settings.refresh_interval_minutes,
            settings.notification_threshold,
            settings.color_theme,
            settings.size_theme,
            settings.antigravity_two_column_quota as i64,
            settings.notifications_enabled as i64
        ],
    )
    .map_err(|error| error.to_string())?;

    let existing_account_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM provider_accounts", [], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    let account_defaults = if existing_account_count == 0 {
        first_run_configs(detected_local_providers)
    } else {
        default_configs()
    };

    for default in account_defaults {
        let legacy = conn
            .query_row(
                "SELECT api_key, base_url, enabled FROM provider_configs WHERE provider = ?1",
                [&default.provider],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)? != 0,
                    ))
                },
            )
            .map_err(|error| error.to_string())?;
        conn.execute(
            "INSERT INTO provider_accounts
             (provider, custom_name, auth_method, base_url, enabled,
              threshold_alert_enabled, reset_alert_enabled, visible, sort_order)
             SELECT ?1, ?2, ?3, ?4, ?5, 1, 1, 1, ?6
             WHERE NOT EXISTS (SELECT 1 FROM provider_accounts WHERE provider = ?1)",
            params![
                default.provider,
                default.custom_name,
                default.auth_method,
                legacy.1,
                legacy.2 as i64,
                default.sort_order
            ],
        )
        .map_err(|error| error.to_string())?;
        let account_id: i64 = conn
            .query_row(
                "SELECT id FROM provider_accounts WHERE provider = ?1 ORDER BY sort_order, id LIMIT 1",
                [&default.provider],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !legacy.0.is_empty() && credentials::read(credential_dir, account_id)?.is_empty() {
            credentials::write(credential_dir, account_id, &legacy.0)?;
        }
        conn.execute(
            "UPDATE provider_configs SET api_key = '' WHERE provider = ?1",
            [&default.provider],
        )
        .map_err(|error| error.to_string())?;
        conn.execute(
            "UPDATE provider_snapshots_v2 SET account_id = ?1
             WHERE provider = ?2 AND account_id IS NULL",
            params![account_id, default.provider],
        )
        .map_err(|error| error.to_string())?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM provider_snapshots_v2 WHERE account_id = ?1",
                [account_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if count == 0 {
            insert_snapshot(
                conn,
                &default_snapshot(account_id, &default.provider, "", now),
            )?;
        }
    }
    Ok(())
}

pub fn insert_snapshot(conn: &Connection, snapshot: &ProviderSnapshot) -> Result<(), String> {
    conn.execute(
        "INSERT INTO provider_snapshots_v2
         (account_id, provider, plan_name, status, fetch_mode, checked_at, notes, configured)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            snapshot.account_id,
            snapshot.provider,
            snapshot.plan_name,
            snapshot.status,
            snapshot.fetch_mode,
            snapshot.last_updated,
            snapshot.notes,
            snapshot.configured as i64
        ],
    )
    .map_err(|error| error.to_string())?;
    let snapshot_id = conn.last_insert_rowid();
    for limit in &snapshot.limits {
        conn.execute(
            "INSERT INTO quota_limits_v2
             (snapshot_id, limit_key, label, resource, used, total, reset_at, window_seconds)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                snapshot_id,
                limit.id,
                limit.label,
                limit.resource,
                limit.used,
                limit.total,
                limit.reset_at,
                limit.window_seconds
            ],
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn load_limits(conn: &Connection, snapshot_id: i64) -> Result<Vec<LimitMetric>, String> {
    let mut statement = conn
        .prepare(
            "SELECT limit_key, label, resource, used, total, reset_at, window_seconds
             FROM quota_limits_v2 WHERE snapshot_id = ?1 ORDER BY id",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([snapshot_id], |row| {
            Ok(LimitMetric {
                id: row.get(0)?,
                label: row.get(1)?,
                resource: row.get(2)?,
                used: row.get(3)?,
                total: row.get(4)?,
                reset_at: row.get(5)?,
                window_seconds: row.get(6)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

pub fn latest_snapshot(conn: &Connection, account_id: i64) -> Result<ProviderSnapshot, String> {
    let (provider, custom_name): (String, String) = conn
        .query_row(
            "SELECT provider, custom_name FROM provider_accounts WHERE id = ?1",
            [account_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?;
    let (provider_name, icon, accent_color) = provider_presentation(&provider);
    let (snapshot_id, plan_name, status, fetch_mode, checked_at, notes, configured) = conn
        .query_row(
            "SELECT id, plan_name, status, fetch_mode, checked_at, notes, configured
             FROM provider_snapshots_v2 WHERE account_id = ?1 ORDER BY id DESC LIMIT 1",
            [account_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)? != 0,
                ))
            },
        )
        .map_err(|error| error.to_string())?;
    Ok(ProviderSnapshot {
        account_id,
        provider,
        provider_name: provider_name.into(),
        icon: icon.into(),
        accent_color: accent_color.into(),
        plan_name,
        status,
        fetch_mode,
        limits: load_limits(conn, snapshot_id)?,
        last_updated: checked_at,
        notes,
        configured,
        custom_name,
    })
}

pub fn load_settings(conn: &Connection) -> Result<AppSettings, String> {
    conn.query_row(
        "SELECT auto_refresh_enabled, refresh_interval_minutes, notification_threshold,
                color_theme, size_theme, antigravity_two_column_quota, notifications_enabled
         FROM app_settings WHERE id = 1",
        [],
        |row| {
            Ok(AppSettings {
                auto_refresh_enabled: row.get::<_, i64>(0)? != 0,
                refresh_interval_minutes: row.get(1)?,
                notification_threshold: row.get(2)?,
                color_theme: row.get(3)?,
                size_theme: row.get(4)?,
                antigravity_two_column_quota: row.get::<_, i64>(5)? != 0,
                notifications_enabled: row.get::<_, i64>(6)? != 0,
            })
        },
    )
    .map_err(|error| error.to_string())
}

pub fn save_settings(conn: &Connection, settings: &AppSettings) -> Result<(), String> {
    conn.execute(
        "UPDATE app_settings SET auto_refresh_enabled = ?1,
         refresh_interval_minutes = ?2, notification_threshold = ?3,
         color_theme = ?4, size_theme = ?5,
         antigravity_two_column_quota = ?6, notifications_enabled = ?7 WHERE id = 1",
        params![
            settings.auto_refresh_enabled as i64,
            settings.refresh_interval_minutes,
            settings.notification_threshold,
            settings.color_theme,
            settings.size_theme,
            settings.antigravity_two_column_quota as i64,
            settings.notifications_enabled as i64
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn load_configs(
    conn: &Connection,
    credential_dir: &Path,
) -> Result<Vec<ProviderConfig>, String> {
    let mut statement = conn
        .prepare(
            "SELECT id, provider, custom_name, auth_method, base_url, enabled,
                    threshold_alert_enabled, reset_alert_enabled, visible, sort_order
             FROM provider_accounts ORDER BY sort_order, id",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(ProviderConfig {
                account_id: row.get(0)?,
                provider: row.get(1)?,
                custom_name: row.get(2)?,
                auth_method: row.get(3)?,
                api_key: String::new(),
                base_url: row.get(4)?,
                enabled: row.get::<_, i64>(5)? != 0,
                threshold_alert_enabled: row.get::<_, i64>(6)? != 0,
                reset_alert_enabled: row.get::<_, i64>(7)? != 0,
                visible: row.get::<_, i64>(8)? != 0,
                sort_order: row.get(9)?,
            })
        })
        .map_err(|error| error.to_string())?;
    let mut configs = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    for config in &mut configs {
        config.api_key = credentials::mask(&credentials::read(credential_dir, config.account_id)?);
    }
    Ok(configs)
}

pub fn config_for_account(
    conn: &Connection,
    credential_dir: &Path,
    account_id: i64,
) -> Result<ProviderConfig, String> {
    conn.query_row(
        "SELECT id, provider, custom_name, auth_method, base_url, enabled,
                threshold_alert_enabled, reset_alert_enabled, visible, sort_order
         FROM provider_accounts WHERE id = ?1",
        [account_id],
        |row| {
            Ok(ProviderConfig {
                account_id: row.get(0)?,
                provider: row.get(1)?,
                custom_name: row.get(2)?,
                auth_method: row.get(3)?,
                api_key: String::new(),
                base_url: row.get(4)?,
                enabled: row.get::<_, i64>(5)? != 0,
                threshold_alert_enabled: row.get::<_, i64>(6)? != 0,
                reset_alert_enabled: row.get::<_, i64>(7)? != 0,
                visible: row.get::<_, i64>(8)? != 0,
                sort_order: row.get(9)?,
            })
        },
    )
    .map_err(|error| error.to_string())
    .and_then(|mut config| {
        config.api_key = credentials::read(credential_dir, account_id)?;
        Ok(config)
    })
}

pub fn save_config(
    conn: &Connection,
    credential_dir: &Path,
    config: &ProviderConfig,
) -> Result<i64, String> {
    let account_id = if config.account_id == 0 {
        let sort_order: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM provider_accounts",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        conn.execute(
            "INSERT INTO provider_accounts
             (provider, custom_name, auth_method, base_url, enabled,
              threshold_alert_enabled, reset_alert_enabled, visible, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                config.provider,
                config.custom_name,
                config.auth_method,
                config.base_url,
                config.enabled as i64,
                config.threshold_alert_enabled as i64,
                config.reset_alert_enabled as i64,
                config.visible as i64,
                sort_order
            ],
        )
        .map_err(|error| error.to_string())?;
        conn.last_insert_rowid()
    } else {
        conn.execute(
            "UPDATE provider_accounts SET custom_name = ?1, auth_method = ?2, base_url = ?3,
             enabled = ?4, threshold_alert_enabled = ?5, reset_alert_enabled = ?6,
             visible = ?7 WHERE id = ?8",
            params![
                config.custom_name,
                config.auth_method,
                config.base_url,
                config.enabled as i64,
                config.threshold_alert_enabled as i64,
                config.reset_alert_enabled as i64,
                config.visible as i64,
                config.account_id
            ],
        )
        .map_err(|error| error.to_string())?;
        config.account_id
    };
    if !credentials::is_masked(&config.api_key) {
        credentials::write(credential_dir, account_id, config.api_key.trim())?;
        conn.execute(
            "UPDATE provider_accounts SET credential_ref = ?1 WHERE id = ?2",
            params![
                if config.api_key.trim().is_empty() {
                    String::new()
                } else {
                    format!("provider-{account_id}.credential")
                },
                account_id
            ],
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(account_id)
}

pub fn reorder_accounts(conn: &Connection, account_ids: &[i64]) -> Result<(), String> {
    for (index, account_id) in account_ids.iter().enumerate() {
        conn.execute(
            "UPDATE provider_accounts SET sort_order = ?1 WHERE id = ?2",
            params![index as i64, account_id],
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn delete_account(
    conn: &Connection,
    credential_dir: &Path,
    account_id: i64,
) -> Result<(), String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM provider_accounts WHERE id = ?1)",
            [account_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !exists {
        return Err("Account not found".into());
    }

    conn.execute(
        "DELETE FROM quota_limits_v2 WHERE snapshot_id IN
         (SELECT id FROM provider_snapshots_v2 WHERE account_id = ?1)",
        [account_id],
    )
    .map_err(|error| error.to_string())?;
    conn.execute(
        "DELETE FROM provider_snapshots_v2 WHERE account_id = ?1",
        [account_id],
    )
    .map_err(|error| error.to_string())?;
    conn.execute("DELETE FROM provider_accounts WHERE id = ?1", [account_id])
        .map_err(|error| error.to_string())?;
    credentials::write(credential_dir, account_id, "")?;

    let remaining_ids = conn
        .prepare("SELECT id FROM provider_accounts ORDER BY sort_order, id")
        .map_err(|error| error.to_string())?
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    reorder_accounts(conn, &remaining_ids)
}

fn load_history(conn: &Connection) -> Result<Vec<UsageHistoryEntry>, String> {
    let mut statement = conn
        .prepare(
            "SELECT l.id, s.provider, COALESCE(a.custom_name, ''), l.limit_key, l.label,
                    l.used, l.total, s.checked_at
             FROM quota_limits_v2 l
             JOIN provider_snapshots_v2 s ON s.id = l.snapshot_id
             LEFT JOIN provider_accounts a ON a.id = s.account_id
             ORDER BY l.id DESC LIMIT 150",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            let used: f64 = row.get(5)?;
            let total: f64 = row.get(6)?;
            Ok(UsageHistoryEntry {
                id: row.get(0)?,
                provider: row.get(1)?,
                custom_name: row.get(2)?,
                limit_id: row.get(3)?,
                limit_label: row.get(4)?,
                used_value: used,
                total_value: total,
                percentage: if total > 0.0 {
                    ((used / total) * 1000.0).round() / 10.0
                } else {
                    0.0
                },
                recorded_at: row.get(7)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

pub fn load_dashboard(conn: &Connection, credential_dir: &Path) -> Result<DashboardState, String> {
    let mut statement = conn
        .prepare("SELECT id FROM provider_accounts WHERE visible = 1 ORDER BY sort_order, id")
        .map_err(|error| error.to_string())?;
    let account_ids = statement
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let providers = account_ids
        .iter()
        .map(|account_id| latest_snapshot(conn, *account_id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DashboardState {
        providers,
        history: load_history(conn)?,
        configs: load_configs(conn, credential_dir)?,
        settings: load_settings(conn)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detected_local_configs_are_named_and_sorted_first() {
        let configs = first_run_configs(&["claude".into(), "google".into()]);
        assert_eq!(configs[0].provider, "claude");
        assert_eq!(configs[0].custom_name, "Local Config");
        assert_eq!(configs[1].provider, "google");
        assert_eq!(configs[1].custom_name, "Local Config");
        assert_eq!(configs[2].provider, "openai");
        assert_eq!(configs[2].sort_order, 2);
    }

    #[test]
    fn new_database_seeds_detected_accounts_before_other_providers() {
        let connection = Connection::open_in_memory().expect("in-memory database");
        let credential_dir =
            std::env::temp_dir().join(format!("ai-bucket-bootstrap-test-{}", std::process::id()));
        init_db(
            &connection,
            &credential_dir,
            "2026-07-13T00:00:00Z",
            &["claude".into()],
        )
        .expect("database bootstrap");

        let configs = load_configs(&connection, &credential_dir).expect("seeded configs");
        assert_eq!(configs.len(), 5);
        assert_eq!(configs[0].provider, "claude");
        assert_eq!(configs[0].custom_name, "Local Config");
        assert_eq!(configs[1].provider, "openai");
    }
}

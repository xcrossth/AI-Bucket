use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::{redirect::Policy, StatusCode};
use rusqlite::Connection;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use windows_sys::{
    Win32::Foundation::LocalFree,
    Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    },
};

use crate::models::LimitMetric;

const BOOTSTRAP_URL: &str = "https://claude.ai/api/bootstrap";
const MAX_RESPONSE_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeErrorKind {
    NeedsAuth,
    CredentialLocked,
    Network,
    Upstream,
    InvalidResponse,
}

#[derive(Debug, Clone)]
pub struct ClaudeError {
    pub kind: ClaudeErrorKind,
    pub message: String,
}

impl ClaudeError {
    fn new(kind: ClaudeErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClaudeUsage {
    pub plan: String,
    pub limits: Vec<LimitMetric>,
}

fn profile_dir() -> Result<PathBuf, ClaudeError> {
    let local = env::var_os("LOCALAPPDATA").ok_or_else(|| {
        ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "Windows local app data is unavailable",
        )
    })?;
    let packages = PathBuf::from(local).join("Packages");
    let entries = fs::read_dir(&packages).map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "Claude Desktop is not installed",
        )
    })?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.starts_with("claude_") {
            let profile = entry
                .path()
                .join("LocalCache")
                .join("Roaming")
                .join("Claude");
            if profile.join("Local State").is_file() {
                return Ok(profile);
            }
        }
    }
    Err(ClaudeError::new(
        ClaudeErrorKind::NeedsAuth,
        "Claude Desktop profile was not found. Install and sign in to Claude Desktop first.",
    ))
}

fn decrypt_dpapi(data: &[u8]) -> Result<Vec<u8>, ClaudeError> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let success = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if success == 0 || output.pbData.is_null() {
        return Err(ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "Windows could not unlock the Claude Desktop session for this user.",
        ));
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    let _ = unsafe { LocalFree(output.pbData.cast()) };
    Ok(result)
}

fn encryption_key(profile: &Path) -> Result<Vec<u8>, ClaudeError> {
    let raw = fs::read(profile.join("Local State")).map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "Claude Desktop encryption metadata was not found",
        )
    })?;
    let state: Value = serde_json::from_slice(&raw).map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Claude Desktop encryption metadata is invalid",
        )
    })?;
    let encoded = state
        .pointer("/os_crypt/encrypted_key")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ClaudeError::new(
                ClaudeErrorKind::NeedsAuth,
                "Claude Desktop encryption key is unavailable",
            )
        })?;
    let wrapped = STANDARD.decode(encoded).map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Claude Desktop encryption key is invalid",
        )
    })?;
    let payload = wrapped.strip_prefix(b"DPAPI").ok_or_else(|| {
        ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "Claude Desktop uses an unsupported credential format",
        )
    })?;
    decrypt_dpapi(payload)
}

fn decrypt_cookie(host: &str, encrypted: &[u8], key: &[u8]) -> Result<String, ClaudeError> {
    if !encrypted.starts_with(b"v10") || encrypted.len() < 3 + 12 + 16 {
        return Err(ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "Claude Desktop cookie format is unsupported",
        ));
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "Claude Desktop encryption key has an invalid size",
        )
    })?;
    let mut plain = cipher
        .decrypt(Nonce::from_slice(&encrypted[3..15]), &encrypted[15..])
        .map_err(|_| {
            ClaudeError::new(
                ClaudeErrorKind::NeedsAuth,
                "Claude Desktop session could not be decrypted",
            )
        })?;
    let host_hash = Sha256::digest(host.as_bytes());
    if plain.starts_with(host_hash.as_slice()) {
        plain.drain(..host_hash.len());
    }
    String::from_utf8(plain).map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Claude Desktop session contained invalid text",
        )
    })
}

fn read_cookie_header() -> Result<String, ClaudeError> {
    let profile = profile_dir()?;
    let key = encryption_key(&profile)?;
    let cookie_path = profile.join("Network").join("Cookies");
    let conn =
        Connection::open_with_flags(&cookie_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| {
                ClaudeError::new(
                ClaudeErrorKind::CredentialLocked,
                "Claude Desktop is still running. Quit it completely, then refresh Claude once.",
            )
            })?;
    let mut statement = conn
        .prepare(
            "SELECT host_key, name, encrypted_value FROM cookies
             WHERE host_key IN ('.claude.ai', 'claude.ai')
               AND name IN ('sessionKey', 'routingHint')",
        )
        .map_err(|_| {
            ClaudeError::new(
                ClaudeErrorKind::InvalidResponse,
                "Claude Desktop cookie database is incompatible",
            )
        })?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(|_| {
            ClaudeError::new(
                ClaudeErrorKind::InvalidResponse,
                "Claude Desktop cookies could not be read",
            )
        })?;
    let mut cookies = Vec::new();
    for row in rows.flatten() {
        let value = decrypt_cookie(&row.0, &row.2, &key)?;
        if !value.is_empty() {
            cookies.push(format!("{}={}", row.1, value));
        }
    }
    if !cookies
        .iter()
        .any(|cookie| cookie.starts_with("sessionKey="))
    {
        return Err(ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "Claude Desktop has no active claude.ai session. Sign in and try again.",
        ));
    }
    Ok(cookies.join("; "))
}

pub fn has_local_config() -> bool {
    if read_cookie_header().is_ok() {
        return true;
    }
    let Ok(profile) = profile_dir() else {
        return false;
    };
    let temporary = env::temp_dir().join(format!(
        "ai-bucket-claude-cookies-{}.sqlite",
        std::process::id()
    ));
    if fs::copy(profile.join("Network").join("Cookies"), &temporary).is_err() {
        return false;
    }
    let detected = Connection::open(&temporary)
        .and_then(|connection| {
            connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM cookies
                 WHERE host_key IN ('.claude.ai', 'claude.ai') AND name = 'sessionKey')",
                [],
                |row| row.get::<_, bool>(0),
            )
        })
        .unwrap_or(false);
    let _ = fs::remove_file(temporary);
    detected
}

async fn json_get(client: &reqwest::Client, url: &str, cookie: &str) -> Result<Value, ClaudeError> {
    let response = client
        .get(url)
        .header("Accept", "application/json")
        .header("Cookie", cookie)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AI-Bucket/0.1",
        )
        .send()
        .await
        .map_err(|error| {
            ClaudeError::new(
                ClaudeErrorKind::Network,
                format!("Claude quota request failed: {error}"),
            )
        })?;
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return Err(ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "Claude Desktop session expired. Sign in again.",
        ));
    }
    if !response.status().is_success() {
        return Err(ClaudeError::new(
            ClaudeErrorKind::Upstream,
            format!("Claude quota endpoint returned HTTP {}", response.status()),
        ));
    }
    if response.content_length().unwrap_or(0) > MAX_RESPONSE_BYTES {
        return Err(ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Claude quota response was unexpectedly large",
        ));
    }
    let body = response.bytes().await.map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Unable to read Claude quota response",
        )
    })?;
    if body.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Claude quota response was unexpectedly large",
        ));
    }
    serde_json::from_slice(&body).map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Claude quota endpoint returned invalid JSON",
        )
    })
}

fn active_organization(bootstrap: &Value) -> Option<(String, String)> {
    let memberships = bootstrap.pointer("/account/memberships")?.as_array()?;
    let selected = memberships
        .iter()
        .find(|item| {
            item.pointer("/organization/api_disabled_reason")
                .is_none_or(Value::is_null)
                && item
                    .pointer("/organization/billing_type")
                    .and_then(Value::as_str)
                    == Some("stripe_subscription")
        })
        .or_else(|| {
            memberships.iter().find(|item| {
                item.pointer("/organization/api_disabled_reason")
                    .is_none_or(Value::is_null)
            })
        })
        .or_else(|| memberships.first())?;
    let organization = selected.get("organization")?;
    let id = organization.get("uuid")?.as_str()?.to_owned();
    let plan = organization
        .get("rate_limit_tier")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Claude subscription")
        .to_owned();
    Some((id, plan))
}

fn append_window(limits: &mut Vec<LimitMetric>, id: &str, label: &str, value: Option<&Value>) {
    let Some(window) = value.and_then(Value::as_object) else {
        return;
    };
    let Some(used) = window.get("utilization").and_then(Value::as_f64) else {
        return;
    };
    limits.push(LimitMetric {
        id: id.to_owned(),
        label: label.to_owned(),
        resource: None,
        used: used.clamp(0.0, 100.0),
        total: 100.0,
        reset_at: window
            .get("resets_at")
            .and_then(Value::as_str)
            .map(str::to_owned),
        window_seconds: match id {
            "five-hour" => Some(18_000),
            _ if id.starts_with("seven-day") => Some(604_800),
            _ => None,
        },
    });
}

fn model_display_name(key: &str) -> String {
    let name = key.strip_prefix("seven_day_").unwrap_or(key);
    if name == "omelette" {
        return "Designer".to_string();
    }
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn append_modern_limits(limits: &mut Vec<LimitMetric>, root: &Map<String, Value>) {
    let Some(items) = root.get("limits").and_then(Value::as_array) else {
        return;
    };
    for (index, item) in items.iter().filter_map(Value::as_object).enumerate() {
        let Some(used) = item.get("percent").and_then(Value::as_f64) else {
            continue;
        };
        let kind = item.get("kind").and_then(Value::as_str).unwrap_or("quota");
        let (id, label, window_seconds) = match kind {
            "session" => (
                "session".to_string(),
                "session (5h)".to_string(),
                Some(18_000),
            ),
            "weekly_all" => (
                "weekly-all".to_string(),
                "weekly (all)".to_string(),
                Some(604_800),
            ),
            "weekly_scoped" => {
                let model = item
                    .get("scope")
                    .and_then(Value::as_object)
                    .and_then(|scope| scope.get("model"))
                    .and_then(Value::as_object)
                    .and_then(|model| model.get("display_name").or_else(|| model.get("id")))
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("model");
                let slug: String = model
                    .chars()
                    .map(|character| {
                        if character.is_ascii_alphanumeric() {
                            character.to_ascii_lowercase()
                        } else {
                            '-'
                        }
                    })
                    .collect();
                (
                    format!("weekly-{}", slug.trim_matches('-')),
                    format!("weekly {model}"),
                    Some(604_800),
                )
            }
            _ => (
                format!("claude-limit-{index}"),
                model_display_name(kind),
                None,
            ),
        };
        limits.push(LimitMetric {
            id,
            label,
            resource: None,
            used: used.clamp(0.0, 100.0),
            total: 100.0,
            reset_at: item
                .get("resets_at")
                .and_then(Value::as_str)
                .map(str::to_owned),
            window_seconds,
        });
    }
}

pub fn parse_usage(value: &Value, plan: String) -> Result<ClaudeUsage, ClaudeError> {
    let root = value.as_object().ok_or_else(|| {
        ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Claude quota response was not an object",
        )
    })?;
    let mut limits = Vec::new();
    append_modern_limits(&mut limits, root);
    if limits.is_empty() {
        append_window(
            &mut limits,
            "five-hour",
            "session (5h)",
            root.get("five_hour"),
        );
        append_window(
            &mut limits,
            "seven-day",
            "weekly (all)",
            root.get("seven_day"),
        );
        for (key, window) in root {
            if !key.starts_with("seven_day_") {
                continue;
            }
            let model = model_display_name(key);
            let id = key.replace('_', "-");
            append_window(&mut limits, &id, &format!("weekly {model}"), Some(window));
        }
    }
    let extra = root.get("extra_usage").and_then(Value::as_object);
    if extra
        .and_then(|item| item.get("is_enabled"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        let used = extra
            .and_then(|item| item.get("used_credits"))
            .and_then(Value::as_f64);
        let total = extra
            .and_then(|item| item.get("monthly_limit"))
            .and_then(Value::as_f64);
        if let (Some(used), Some(total)) = (used, total.filter(|value| *value > 0.0)) {
            limits.push(LimitMetric {
                id: "extra-usage".into(),
                label: "extra usage".into(),
                resource: None,
                used,
                total,
                reset_at: extra
                    .and_then(|item| item.get("resets_at"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                window_seconds: None,
            });
        }
    }
    if limits.is_empty() {
        return Err(ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Claude quota response contained no usage windows",
        ));
    }
    Ok(ClaudeUsage { plan, limits })
}

async fn collect_payload() -> Result<(Value, String), ClaudeError> {
    let cookie = read_cookie_header()?;
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|_| ClaudeError::new(ClaudeErrorKind::Network, "Unable to create HTTP client"))?;
    let bootstrap = json_get(&client, BOOTSTRAP_URL, &cookie).await?;
    let (organization_id, plan) = active_organization(&bootstrap).ok_or_else(|| {
        ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "Claude account has no active organization",
        )
    })?;
    let usage_url = format!("https://claude.ai/api/organizations/{organization_id}/usage");
    let usage = json_get(&client, &usage_url, &cookie).await?;
    Ok((usage, plan))
}

pub async fn collect() -> Result<ClaudeUsage, ClaudeError> {
    let (usage, plan) = collect_payload().await?;
    parse_usage(&usage, plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_usage_windows() {
        let value = serde_json::json!({
            "limits": [
                {"kind": "session", "group": "session", "is_active": true, "percent": 12.5, "resets_at": "2026-07-11T20:00:00Z", "scope": null},
                {"kind": "weekly_all", "group": "weekly", "is_active": true, "percent": 40, "resets_at": "2026-07-18T20:00:00Z", "scope": null},
                {"kind": "weekly_scoped", "group": "weekly", "is_active": true, "percent": 36, "resets_at": "2026-07-18T20:00:00Z", "scope": {"model": {"id": "fable", "display_name": "Fable"}}}
            ]
        });
        let usage = parse_usage(&value, "max".into()).expect("valid usage");
        assert_eq!(usage.plan, "max");
        assert_eq!(usage.limits.len(), 3);
        assert_eq!(usage.limits[0].used, 12.5);
        let fable = usage
            .limits
            .iter()
            .find(|limit| limit.id == "weekly-fable")
            .expect("Fable quota is preserved");
        assert_eq!(fable.label, "weekly Fable");
        assert_eq!(fable.used, 36.0);
    }

    #[test]
    #[ignore = "uses the local Claude Desktop session and calls live quota endpoints"]
    fn live_collector_returns_quota_windows() {
        let usage = tauri::async_runtime::block_on(collect()).expect("live Claude quota request");
        assert!(!usage.limits.is_empty());
        assert!(
            usage
                .limits
                .iter()
                .any(|limit| limit.label == "weekly Fable"),
            "live labels: {:?}",
            usage
                .limits
                .iter()
                .map(|limit| &limit.label)
                .collect::<Vec<_>>()
        );
    }
}

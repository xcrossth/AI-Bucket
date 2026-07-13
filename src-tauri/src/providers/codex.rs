use std::{env, fs, path::PathBuf, time::Duration};

use chrono::Utc;
use reqwest::{redirect::Policy, StatusCode};
use serde_json::{Map, Value};

use crate::models::LimitMetric;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const MAX_RESPONSE_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexErrorKind {
    NeedsAuth,
    Network,
    Upstream,
    InvalidResponse,
}

#[derive(Debug, Clone)]
pub struct CodexError {
    pub kind: CodexErrorKind,
    pub message: String,
}

impl CodexError {
    fn new(kind: CodexErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CodexUsage {
    pub plan: String,
    pub limits: Vec<LimitMetric>,
}

fn auth_path() -> Result<PathBuf, CodexError> {
    if let Some(path) = env::var_os("CODEX_AUTH_JSON") {
        return Ok(PathBuf::from(path));
    }
    if let Some(home) = env::var_os("CODEX_HOME") {
        return Ok(PathBuf::from(home).join("auth.json"));
    }
    let home = env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .ok_or_else(|| {
            CodexError::new(CodexErrorKind::NeedsAuth, "Home directory is unavailable")
        })?;
    Ok(PathBuf::from(home).join(".codex").join("auth.json"))
}

fn read_credentials() -> Result<(String, Option<String>), CodexError> {
    let path = auth_path()?;
    let raw = fs::read_to_string(&path).map_err(|_| {
        CodexError::new(
            CodexErrorKind::NeedsAuth,
            "Codex auth.json was not found. Sign in with Codex CLI first.",
        )
    })?;
    let document: Value = serde_json::from_str(&raw).map_err(|_| {
        CodexError::new(
            CodexErrorKind::NeedsAuth,
            "Codex auth.json is not valid JSON. Sign in again with Codex CLI.",
        )
    })?;
    let tokens = document.get("tokens").and_then(Value::as_object);
    let access_token = tokens
        .and_then(|value| value.get("access_token"))
        .or_else(|| document.get("access_token"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CodexError::new(
                CodexErrorKind::NeedsAuth,
                "Codex auth.json has no access token. Sign in again with Codex CLI.",
            )
        })?
        .to_owned();
    let account_id = tokens
        .and_then(|value| value.get("account_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned);
    Ok((access_token, account_id))
}

pub fn has_local_config() -> bool {
    read_credentials().is_ok()
}

pub async fn collect() -> Result<CodexUsage, CodexError> {
    let (access_token, account_id) = read_credentials()?;
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|_| CodexError::new(CodexErrorKind::Network, "Unable to create HTTP client"))?;

    let mut request = client
        .get(USAGE_URL)
        .bearer_auth(access_token)
        .header("Accept", "application/json");
    if let Some(account_id) = account_id {
        request = request.header("chatgpt-account-id", account_id);
    }

    let response = request.send().await.map_err(|error| {
        CodexError::new(
            CodexErrorKind::Network,
            format!("Codex quota request failed: {error}"),
        )
    })?;
    if response.status() == StatusCode::UNAUTHORIZED || response.status() == StatusCode::FORBIDDEN {
        return Err(CodexError::new(
            CodexErrorKind::NeedsAuth,
            "Codex session expired. Sign in again with Codex CLI.",
        ));
    }
    if !response.status().is_success() {
        return Err(CodexError::new(
            CodexErrorKind::Upstream,
            format!("Codex quota endpoint returned HTTP {}", response.status()),
        ));
    }
    if response.content_length().unwrap_or(0) > MAX_RESPONSE_BYTES {
        return Err(CodexError::new(
            CodexErrorKind::InvalidResponse,
            "Codex quota response was unexpectedly large",
        ));
    }
    let body = response.bytes().await.map_err(|_| {
        CodexError::new(
            CodexErrorKind::InvalidResponse,
            "Unable to read Codex quota response",
        )
    })?;
    if body.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(CodexError::new(
            CodexErrorKind::InvalidResponse,
            "Codex quota response was unexpectedly large",
        ));
    }
    let value: Value = serde_json::from_slice(&body).map_err(|_| {
        CodexError::new(
            CodexErrorKind::InvalidResponse,
            "Codex quota endpoint returned invalid JSON",
        )
    })?;
    parse_usage(&value)
}

fn object_field<'a>(object: &'a Map<String, Value>, snake: &str, camel: &str) -> Option<&'a Value> {
    object.get(snake).or_else(|| object.get(camel))
}

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(|item| {
        item.as_f64()
            .or_else(|| item.as_str().and_then(|text| text.parse::<f64>().ok()))
    })
}

fn integer(value: Option<&Value>) -> Option<i64> {
    number(value).map(|item| item.round() as i64)
}

fn reset_at(window: &Map<String, Value>) -> Option<String> {
    if let Some(timestamp) = integer(object_field(window, "reset_at", "resetAt")) {
        return chrono::DateTime::from_timestamp(timestamp, 0).map(|value| value.to_rfc3339());
    }
    integer(object_field(
        window,
        "reset_after_seconds",
        "resetAfterSeconds",
    ))
    .map(|seconds| (Utc::now() + chrono::Duration::seconds(seconds)).to_rfc3339())
}

fn window_label(prefix: &str, seconds: Option<i64>) -> String {
    let duration = match seconds {
        Some(value) if value % 604_800 == 0 => format!("{}d", value / 86_400),
        Some(value) if value % 3_600 == 0 => format!("{}h", value / 3_600),
        Some(value) if value % 60 == 0 => format!("{}m", value / 60),
        Some(value) => format!("{}s", value),
        None => "window".to_string(),
    };
    format!("{prefix} ({duration})")
}

fn slug(value: &str) -> String {
    let mut result = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character.to_ascii_lowercase());
        } else if !result.ends_with('-') {
            result.push('-');
        }
    }
    result.trim_matches('-').to_string()
}

fn parse_window(
    id: String,
    prefix: &str,
    resource: Option<String>,
    value: Option<&Value>,
) -> Option<LimitMetric> {
    let window = value?.as_object()?;
    let used = number(object_field(window, "used_percent", "usedPercent"))?;
    let window_seconds = integer(object_field(
        window,
        "limit_window_seconds",
        "limitWindowSeconds",
    ));
    Some(LimitMetric {
        id,
        label: window_label(prefix, window_seconds),
        resource,
        used: used.clamp(0.0, 100.0),
        total: 100.0,
        reset_at: reset_at(window),
        window_seconds,
    })
}

fn append_rate_limit(
    limits: &mut Vec<LimitMetric>,
    key_prefix: &str,
    label: &str,
    resource: Option<String>,
    rate_limit: &Map<String, Value>,
) {
    if let Some(limit) = parse_window(
        format!("{key_prefix}-primary"),
        label,
        resource.clone(),
        object_field(rate_limit, "primary_window", "primaryWindow"),
    ) {
        limits.push(limit);
    }
    if let Some(limit) = parse_window(
        format!("{key_prefix}-secondary"),
        label,
        resource,
        object_field(rate_limit, "secondary_window", "secondaryWindow"),
    ) {
        limits.push(limit);
    }
}

pub fn parse_usage(value: &Value) -> Result<CodexUsage, CodexError> {
    let root = value.as_object().ok_or_else(|| {
        CodexError::new(
            CodexErrorKind::InvalidResponse,
            "Codex quota response was not an object",
        )
    })?;
    let mut limits = Vec::new();
    if let Some(rate_limit) =
        object_field(root, "rate_limit", "rateLimit").and_then(Value::as_object)
    {
        if let Some(limit) = parse_window(
            "session".to_string(),
            "session",
            None,
            object_field(rate_limit, "primary_window", "primaryWindow"),
        ) {
            limits.push(limit);
        }
        if let Some(limit) = parse_window(
            "weekly".to_string(),
            "weekly",
            None,
            object_field(rate_limit, "secondary_window", "secondaryWindow"),
        ) {
            limits.push(limit);
        }
    }

    let additional = object_field(root, "additional_rate_limits", "additionalRateLimits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (index, entry) in additional.iter().enumerate() {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let name = [
            "limit_name",
            "limitName",
            "metered_feature",
            "meteredFeature",
            "model",
        ]
        .iter()
        .find_map(|key| entry.get(*key).and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("additional")
        .to_string();
        let Some(rate_limit) =
            object_field(entry, "rate_limit", "rateLimit").and_then(Value::as_object)
        else {
            continue;
        };
        let key = slug(&name);
        append_rate_limit(
            &mut limits,
            &format!(
                "{}-{}",
                if key.is_empty() { "additional" } else { &key },
                index
            ),
            &name,
            Some(name.clone()),
            rate_limit,
        );
    }

    if limits.is_empty() {
        return Err(CodexError::new(
            CodexErrorKind::InvalidResponse,
            "Codex quota response contained no usage windows",
        ));
    }
    let plan = object_field(root, "plan_type", "planType")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("ChatGPT plan")
        .to_string();
    Ok(CodexUsage { plan, limits })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_primary_secondary_and_additional_windows() {
        let payload = serde_json::json!({
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {"used_percent": 12.5, "limit_window_seconds": 18000, "reset_at": 1_800_000_000},
                "secondary_window": {"used_percent": 41, "limit_window_seconds": 604800, "reset_at": 1_800_100_000}
            },
            "additional_rate_limits": [{
                "limit_name": "GPT-5 Codex Spark",
                "rate_limit": {
                    "primary_window": {"used_percent": 3, "limit_window_seconds": 18000, "reset_at": 1_800_000_100},
                    "secondary_window": {"used_percent": 9, "limit_window_seconds": 604800, "reset_at": 1_800_100_100}
                }
            }]
        });
        let usage = parse_usage(&payload).expect("valid usage response");
        assert_eq!(usage.plan, "pro");
        assert_eq!(usage.limits.len(), 4);
        assert_eq!(usage.limits[0].label, "session (5h)");
        assert_eq!(usage.limits[1].label, "weekly (7d)");
        assert_eq!(
            usage.limits[2].resource.as_deref(),
            Some("GPT-5 Codex Spark")
        );
    }

    #[test]
    fn rejects_payload_without_windows() {
        let error = parse_usage(&serde_json::json!({"plan_type": "pro"}))
            .expect_err("missing windows should fail");
        assert_eq!(error.kind, CodexErrorKind::InvalidResponse);
    }

    #[test]
    #[ignore = "uses the local Codex session and calls the live quota endpoint"]
    fn live_collector_returns_quota_windows() {
        let usage = tauri::async_runtime::block_on(collect()).expect("live Codex quota request");
        assert!(!usage.limits.is_empty());
        assert!(usage.limits.iter().all(|limit| limit.total > 0.0));
    }

    #[test]
    #[ignore = "diagnoses repeated live Codex quota responses without printing credentials"]
    fn live_collector_is_consistent_across_repeated_requests() {
        let samples = tauri::async_runtime::block_on(async {
            let mut samples = Vec::new();
            for _ in 0..6 {
                let usage = collect().await.expect("live Codex quota request");
                samples.push(
                    usage
                        .limits
                        .iter()
                        .map(|limit| (limit.id.clone(), limit.used, limit.reset_at.clone()))
                        .collect::<Vec<_>>(),
                );
            }
            samples
        });
        for (index, sample) in samples.iter().enumerate() {
            eprintln!("sample {}: {sample:?}", index + 1);
        }
    }
}

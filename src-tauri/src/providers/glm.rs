use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use reqwest::{redirect::Policy, StatusCode, Url};
use serde_json::{Map, Value};

use crate::models::LimitMetric;

const INTERNATIONAL_URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";
const MAX_RESPONSE_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlmErrorKind {
    NeedsConfig,
    Auth,
    Network,
    Upstream,
    InvalidResponse,
}

#[derive(Debug, Clone)]
pub struct GlmError {
    pub kind: GlmErrorKind,
    pub message: String,
}

impl GlmError {
    fn new(kind: GlmErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GlmUsage {
    pub plan: String,
    pub limits: Vec<LimitMetric>,
}

pub async fn collect(api_key: &str, configured_url: &str) -> Result<GlmUsage, GlmError> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(GlmError::new(
            GlmErrorKind::NeedsConfig,
            "Add a GLM/Z.AI Coding Plan key before refreshing.",
        ));
    }
    let url = select_url(configured_url).ok_or_else(|| {
        GlmError::new(
            GlmErrorKind::NeedsConfig,
            "GLM quota URL must use the official Z.AI or BigModel HTTPS host.",
        )
    })?;
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|_| GlmError::new(GlmErrorKind::Network, "Unable to create HTTP client"))?;
    let response = client
        .get(&url)
        .bearer_auth(api_key)
        .header("Accept", "application/json")
        .header("Accept-Language", "en-US,en")
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|error| {
            GlmError::new(
                GlmErrorKind::Network,
                format!("GLM quota request failed: {error}"),
            )
        })?;
    let status = response.status();
    if response.content_length().unwrap_or(0) > MAX_RESPONSE_BYTES {
        return Err(GlmError::new(
            GlmErrorKind::InvalidResponse,
            "GLM quota response was unexpectedly large.",
        ));
    }
    let body = response.bytes().await.map_err(|_| {
        GlmError::new(
            GlmErrorKind::InvalidResponse,
            "Unable to read GLM quota response.",
        )
    })?;
    if body.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(GlmError::new(
            GlmErrorKind::InvalidResponse,
            "GLM quota response was unexpectedly large.",
        ));
    }
    let payload = serde_json::from_slice::<Value>(&body).ok();
    let (api_code, api_message) = payload
        .as_ref()
        .map(api_result)
        .unwrap_or((None, String::new()));
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
        || matches!(api_code, Some(1002 | 1004))
        || is_auth_message(&api_message)
    {
        return Err(GlmError::new(
            GlmErrorKind::Auth,
            "GLM/Z.AI rejected the key. Use an active Coding Plan key.",
        ));
    }
    if !status.is_success() {
        return Err(GlmError::new(
            GlmErrorKind::Upstream,
            format!("GLM quota endpoint returned HTTP {status}."),
        ));
    }
    let payload = payload.ok_or_else(|| {
        GlmError::new(
            GlmErrorKind::InvalidResponse,
            "GLM quota endpoint returned invalid JSON.",
        )
    })?;
    if api_code.is_some_and(|code| code != 0 && code != 200) {
        return Err(GlmError::new(
            GlmErrorKind::Upstream,
            if api_message.is_empty() {
                "GLM quota API returned an error.".to_string()
            } else {
                format!("GLM quota API: {}", compact_message(&api_message))
            },
        ));
    }
    parse_usage(&payload)
}

fn select_url(configured: &str) -> Option<String> {
    let configured = configured.trim();
    if !configured.is_empty() && is_allowed_url(configured) {
        return Some(configured.to_string());
    }
    if configured.is_empty() {
        return Some(INTERNATIONAL_URL.to_string());
    }
    None
}

fn is_allowed_url(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.host_str(), Some("api.z.ai" | "open.bigmodel.cn"))
        && url.path() == "/api/monitor/usage/quota/limit"
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

fn api_result(value: &Value) -> (Option<i64>, String) {
    let code = integer(value.get("code")).or_else(|| integer(value.pointer("/error/code")));
    let message = ["msg", "message"]
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .or_else(|| value.pointer("/error/message").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    (code, message)
}

fn compact_message(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(180)
        .collect()
}

fn is_auth_message(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "unauthorized",
        "authorization",
        "invalid token",
        "invalid key",
        "authentication",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

fn data_root(value: &Value) -> Option<&Map<String, Value>> {
    value
        .pointer("/data/data")
        .or_else(|| value.get("data"))
        .and_then(Value::as_object)
}

fn window_seconds(limit: &Map<String, Value>) -> Option<i64> {
    let count = integer(limit.get("number"))?;
    let unit = limit.get("unit")?.as_str()?.to_ascii_lowercase();
    match unit.as_str() {
        "minute" | "minutes" | "min" => Some(count * 60),
        "hour" | "hours" | "h" => Some(count * 3_600),
        "day" | "days" | "d" => Some(count * 86_400),
        "week" | "weeks" | "w" => Some(count * 604_800),
        "month" | "months" => Some(count * 2_592_000),
        _ => None,
    }
}

fn duration_label(seconds: Option<i64>) -> Option<String> {
    match seconds {
        Some(value) if value % 604_800 == 0 => Some(format!("{}w", value / 604_800)),
        Some(value) if value % 86_400 == 0 => Some(format!("{}d", value / 86_400)),
        Some(value) if value % 3_600 == 0 => Some(format!("{}h", value / 3_600)),
        Some(value) if value % 60 == 0 => Some(format!("{}m", value / 60)),
        _ => None,
    }
}

fn reset_at(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        if DateTime::parse_from_rfc3339(text).is_ok() {
            return Some(text.to_string());
        }
        if let Ok(number) = text.parse::<i64>() {
            return epoch_to_iso(number);
        }
    }
    integer(Some(value)).and_then(epoch_to_iso)
}

fn epoch_to_iso(value: i64) -> Option<String> {
    let millis = if value > 10_000_000_000 {
        value
    } else {
        value * 1_000
    };
    Utc.timestamp_millis_opt(millis)
        .single()
        .map(|date| date.to_rfc3339())
}

fn label(limit_type: &str, seconds: Option<i64>, index: usize) -> String {
    let base = match limit_type {
        "TOKENS_LIMIT" => "tokens",
        "TIME_LIMIT" => "tool usage",
        _ => "quota",
    };
    duration_label(seconds)
        .map(|duration| format!("{base} ({duration})"))
        .unwrap_or_else(|| {
            if index == 0 {
                base.to_string()
            } else {
                format!("{base} {}", index + 1)
            }
        })
}

fn slug(value: &str) -> String {
    let result: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    result
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

pub fn parse_usage(value: &Value) -> Result<GlmUsage, GlmError> {
    let root = data_root(value).ok_or_else(|| {
        GlmError::new(
            GlmErrorKind::InvalidResponse,
            "GLM quota response contained no data object.",
        )
    })?;
    let rows = root
        .get("limits")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            GlmError::new(
                GlmErrorKind::InvalidResponse,
                "GLM quota response contained no limits array.",
            )
        })?;
    let mut limits = Vec::new();
    for (index, row) in rows.iter().filter_map(Value::as_object).enumerate() {
        let limit_type = row.get("type").and_then(Value::as_str).unwrap_or("LIMIT");
        let Some(used) = number(row.get("percentage")) else {
            continue;
        };
        let seconds = window_seconds(row);
        let limit_label = label(limit_type, seconds, index);
        limits.push(LimitMetric {
            id: format!("{}-{index}", slug(limit_type)),
            label: limit_label,
            resource: row.get("name").and_then(Value::as_str).map(str::to_owned),
            used: used.clamp(0.0, 100.0),
            total: 100.0,
            reset_at: reset_at(row.get("nextResetTime")),
            window_seconds: seconds,
        });
    }
    if limits.is_empty() {
        return Err(GlmError::new(
            GlmErrorKind::InvalidResponse,
            "GLM quota response contained no percentage-based limits.",
        ));
    }
    let plan = ["level", "plan", "planName"]
        .iter()
        .find_map(|key| root.get(*key).and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("GLM Coding Plan")
        .to_string();
    Ok(GlmUsage { plan, limits })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_token_and_tool_limits() {
        let value = serde_json::json!({
            "code": 200,
            "data": {
                "level": "Max",
                "limits": [
                    {"type": "TOKENS_LIMIT", "percentage": 17, "unit": "HOUR", "number": 5, "nextResetTime": 1_800_000_000_000_i64},
                    {"type": "TIME_LIMIT", "percentage": 43, "unit": "DAY", "number": 30, "nextResetTime": "2030-01-01T00:00:00Z"}
                ]
            }
        });
        let usage = parse_usage(&value).expect("valid GLM usage");
        assert_eq!(usage.plan, "Max");
        assert_eq!(usage.limits[0].label, "tokens (5h)");
        assert_eq!(usage.limits[1].label, "tool usage (30d)");
    }

    #[test]
    fn rejects_non_zai_destination() {
        assert!(!is_allowed_url(
            "https://api.z.ai.example.com/api/monitor/usage/quota/limit"
        ));
        assert!(!is_allowed_url(
            "http://api.z.ai/api/monitor/usage/quota/limit"
        ));
        assert!(is_allowed_url(INTERNATIONAL_URL));
        assert!(is_allowed_url(
            "https://open.bigmodel.cn/api/monitor/usage/quota/limit"
        ));
    }
}

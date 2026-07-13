use std::{collections::HashSet, time::Duration};

use chrono::Utc;
use reqwest::{redirect::Policy, StatusCode};
use serde_json::{Map, Value};

use crate::models::LimitMetric;

const TOKEN_PLAN_URL: &str = "https://www.minimax.io/v1/token_plan/remains";
const CODING_PLAN_URL: &str = "https://api.minimax.io/v1/api/openplatform/coding_plan/remains";
const MAX_RESPONSE_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MiniMaxErrorKind {
    NeedsConfig,
    Auth,
    Network,
    Upstream,
    InvalidResponse,
}

#[derive(Debug, Clone)]
pub struct MiniMaxError {
    pub kind: MiniMaxErrorKind,
    pub message: String,
}

impl MiniMaxError {
    fn new(kind: MiniMaxErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MiniMaxUsage {
    pub plan: String,
    pub limits: Vec<LimitMetric>,
}

pub async fn collect(api_key: &str, configured_url: &str) -> Result<MiniMaxUsage, MiniMaxError> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(MiniMaxError::new(
            MiniMaxErrorKind::NeedsConfig,
            "Add a MiniMax Token/Coding Plan API key before refreshing.",
        ));
    }

    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|_| {
            MiniMaxError::new(MiniMaxErrorKind::Network, "Unable to create HTTP client")
        })?;
    let urls = candidate_urls(configured_url);
    let mut last_error = MiniMaxError::new(
        MiniMaxErrorKind::Network,
        "MiniMax quota endpoints were unavailable.",
    );

    for (index, url) in urls.iter().enumerate() {
        let can_fallback = index + 1 < urls.len();
        let response = match client
            .get(url)
            .bearer_auth(api_key)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) if can_fallback => {
                last_error = MiniMaxError::new(
                    MiniMaxErrorKind::Network,
                    "A MiniMax quota endpoint was unreachable.",
                );
                continue;
            }
            Err(_) => {
                return Err(MiniMaxError::new(
                    MiniMaxErrorKind::Network,
                    "MiniMax quota request failed.",
                ));
            }
        };

        let status = response.status();
        if response.content_length().unwrap_or(0) > MAX_RESPONSE_BYTES {
            return Err(MiniMaxError::new(
                MiniMaxErrorKind::InvalidResponse,
                "MiniMax quota response was unexpectedly large.",
            ));
        }
        let body = response.bytes().await.map_err(|_| {
            MiniMaxError::new(
                MiniMaxErrorKind::InvalidResponse,
                "Unable to read MiniMax quota response.",
            )
        })?;
        if body.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(MiniMaxError::new(
                MiniMaxErrorKind::InvalidResponse,
                "MiniMax quota response was unexpectedly large.",
            ));
        }
        let payload = serde_json::from_slice::<Value>(&body).ok();
        let (api_status, api_message) = payload
            .as_ref()
            .map(base_response)
            .unwrap_or((None, String::new()));

        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || api_status == Some(1004)
            || is_auth_message(&api_message)
        {
            return Err(MiniMaxError::new(
                MiniMaxErrorKind::Auth,
                "MiniMax rejected the key. Use an active Token Plan or Coding Plan key.",
            ));
        }

        if !status.is_success() {
            last_error = MiniMaxError::new(
                MiniMaxErrorKind::Upstream,
                format!("MiniMax quota endpoint returned HTTP {status}."),
            );
            if can_fallback
                && (status == StatusCode::NOT_FOUND
                    || status == StatusCode::METHOD_NOT_ALLOWED
                    || status.is_server_error())
            {
                continue;
            }
            return Err(last_error);
        }

        let payload = payload.ok_or_else(|| {
            MiniMaxError::new(
                MiniMaxErrorKind::InvalidResponse,
                "MiniMax quota endpoint returned invalid JSON.",
            )
        })?;
        if api_status.unwrap_or(0) != 0 {
            return Err(MiniMaxError::new(
                MiniMaxErrorKind::Upstream,
                if api_message.is_empty() {
                    "MiniMax quota API returned an error.".to_string()
                } else {
                    format!("MiniMax quota API: {}", compact_message(&api_message))
                },
            ));
        }

        match parse_usage(&payload, url.contains("/coding_plan/remains")) {
            Ok(usage) => return Ok(usage),
            Err(error) if can_fallback => {
                last_error = error;
                continue;
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error)
}

fn candidate_urls(configured_url: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    [configured_url.trim(), TOKEN_PLAN_URL, CODING_PLAN_URL]
        .into_iter()
        .filter(|url| !url.is_empty())
        .filter(|url| is_allowed_minimax_url(url))
        .filter(|url| seen.insert((*url).to_string()))
        .map(str::to_string)
        .collect()
}

fn is_allowed_minimax_url(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    if url.scheme() != "https" || url.username() != "" || url.password().is_some() {
        return false;
    }
    matches!(
        url.host_str(),
        Some(
            "www.minimax.io"
                | "api.minimax.io"
                | "platform.minimax.io"
                | "www.minimaxi.com"
                | "api.minimaxi.com"
                | "platform.minimaxi.com"
        )
    )
}

fn compact_message(value: &str) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(160).collect()
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

fn string_field(object: &Map<String, Value>, snake: &str, camel: &str) -> Option<String> {
    object_field(object, snake, camel)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn base_response(payload: &Value) -> (Option<i64>, String) {
    let Some(root) = payload.as_object() else {
        return (None, String::new());
    };
    let Some(base) = object_field(root, "base_resp", "baseResp").and_then(Value::as_object) else {
        return (None, String::new());
    };
    let status =
        number(object_field(base, "status_code", "statusCode")).map(|value| value.round() as i64);
    let message = string_field(base, "status_msg", "statusMsg").unwrap_or_default();
    (status, message)
}

fn is_auth_message(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    [
        "token plan",
        "coding plan",
        "invalid api key",
        "invalid key",
        "unauthorized",
        "inactive",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn is_text_model(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    normalized == "general"
        || normalized.starts_with("minimax-m")
        || normalized.starts_with("coding-plan")
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

fn timestamp(value: Option<&Value>) -> Option<String> {
    if let Some(raw) = number(value) {
        if raw > 0.0 {
            let milliseconds = if raw < 1_000_000_000_000.0 {
                raw * 1_000.0
            } else {
                raw
            };
            return chrono::DateTime::from_timestamp_millis(milliseconds.round() as i64)
                .map(|date| date.to_rfc3339());
        }
    }
    value
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn reset_at(
    model: &Map<String, Value>,
    remains_snake: &str,
    remains_camel: &str,
    end_snake: &str,
    end_camel: &str,
) -> Option<String> {
    if let Some(milliseconds) = number(object_field(model, remains_snake, remains_camel)) {
        if milliseconds > 0.0 {
            return Some(
                (Utc::now() + chrono::Duration::milliseconds(milliseconds.round() as i64))
                    .to_rfc3339(),
            );
        }
    }
    timestamp(object_field(model, end_snake, end_camel))
}

fn window_seconds(model: &Map<String, Value>, start: &str, end: &str) -> Option<i64> {
    let start = number(model.get(start))?;
    let end = number(model.get(end))?;
    let difference = end - start;
    if difference <= 0.0 {
        return None;
    }
    Some(if difference > 1_000_000.0 {
        (difference / 1_000.0).round() as i64
    } else {
        difference.round() as i64
    })
}

fn duration_label(seconds: Option<i64>, fallback: &str) -> String {
    match seconds {
        Some(value) if value >= 86_400 && value % 86_400 == 0 => {
            format!("{}d", value / 86_400)
        }
        Some(value) if value >= 3_600 && value % 3_600 == 0 => format!("{}h", value / 3_600),
        _ => fallback.to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_window(
    model: &Map<String, Value>,
    model_name: &str,
    id_suffix: &str,
    label_prefix: &str,
    total_snake: &str,
    total_camel: &str,
    count_snake: &str,
    count_camel: &str,
    percent_snake: &str,
    percent_camel: &str,
    remains_snake: &str,
    remains_camel: &str,
    end_snake: &str,
    end_camel: &str,
    seconds: Option<i64>,
    count_means_remaining: bool,
) -> Option<LimitMetric> {
    let total = number(object_field(model, total_snake, total_camel))
        .unwrap_or(0.0)
        .max(0.0);
    let (used, total) = if total > 0.0 {
        let count = number(object_field(model, count_snake, count_camel))
            .unwrap_or(0.0)
            .max(0.0);
        (
            if count_means_remaining {
                (total - count).max(0.0)
            } else {
                count.min(total)
            },
            total,
        )
    } else {
        let remaining =
            number(object_field(model, percent_snake, percent_camel))?.clamp(0.0, 100.0);
        (100.0 - remaining, 100.0)
    };
    let model_slug = slug(model_name);
    let compact_name = if model_name.eq_ignore_ascii_case("general") {
        String::new()
    } else {
        format!("{model_name} ")
    };
    Some(LimitMetric {
        id: format!(
            "{}-{id_suffix}",
            if model_slug.is_empty() {
                "general"
            } else {
                &model_slug
            }
        ),
        label: format!(
            "{compact_name}{label_prefix} ({})",
            duration_label(seconds, if id_suffix == "session" { "5h" } else { "7d" })
        ),
        resource: Some(model_name.to_string()),
        used,
        total,
        reset_at: reset_at(model, remains_snake, remains_camel, end_snake, end_camel),
        window_seconds: seconds,
    })
}

fn plan_label(root: &Map<String, Value>, models: &[&Map<String, Value>]) -> String {
    for (snake, camel) in [
        ("current_subscribe_title", "currentSubscribeTitle"),
        ("plan_name", "planName"),
        ("current_plan_title", "currentPlanTitle"),
        ("combo_title", "comboTitle"),
    ] {
        if let Some(value) = string_field(root, snake, camel) {
            let cleaned = value
                .replace("MiniMax", "")
                .replace("Coding Plan", "")
                .trim()
                .to_string();
            return if cleaned.is_empty() {
                "Coding Plan".into()
            } else {
                cleaned
            };
        }
    }
    let max_total = models
        .iter()
        .filter_map(|model| {
            number(object_field(
                model,
                "current_interval_total_count",
                "currentIntervalTotalCount",
            ))
        })
        .fold(0.0_f64, f64::max);
    if max_total >= 15_000.0 {
        "Max".into()
    } else if max_total >= 4_500.0 {
        "Plus".into()
    } else if max_total >= 1_500.0 {
        "Starter".into()
    } else {
        "Coding Plan".into()
    }
}

pub fn parse_usage(
    payload: &Value,
    count_means_remaining: bool,
) -> Result<MiniMaxUsage, MiniMaxError> {
    let root = payload.as_object().ok_or_else(|| {
        MiniMaxError::new(
            MiniMaxErrorKind::InvalidResponse,
            "MiniMax quota response was not an object.",
        )
    })?;
    let models = object_field(root, "model_remains", "modelRemains")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            MiniMaxError::new(
                MiniMaxErrorKind::InvalidResponse,
                "MiniMax quota response contained no model limits.",
            )
        })?;
    let text_models = models
        .iter()
        .filter_map(Value::as_object)
        .filter(|model| {
            string_field(model, "model_name", "modelName")
                .map(|name| is_text_model(&name))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    if text_models.is_empty() {
        return Err(MiniMaxError::new(
            MiniMaxErrorKind::InvalidResponse,
            "MiniMax returned no text/coding quota data.",
        ));
    }

    let mut limits = Vec::new();
    for model in &text_models {
        let model_name =
            string_field(model, "model_name", "modelName").unwrap_or_else(|| "general".into());
        let session_seconds = window_seconds(model, "start_time", "end_time");
        let weekly_seconds =
            window_seconds(model, "weekly_start_time", "weekly_end_time").or(Some(604_800));
        if let Some(limit) = build_window(
            model,
            &model_name,
            "session",
            "session",
            "current_interval_total_count",
            "currentIntervalTotalCount",
            "current_interval_usage_count",
            "currentIntervalUsageCount",
            "current_interval_remaining_percent",
            "currentIntervalRemainingPercent",
            "remains_time",
            "remainsTime",
            "end_time",
            "endTime",
            session_seconds.or(Some(18_000)),
            count_means_remaining,
        ) {
            limits.push(limit);
        }
        if let Some(limit) = build_window(
            model,
            &model_name,
            "weekly",
            "weekly",
            "current_weekly_total_count",
            "currentWeeklyTotalCount",
            "current_weekly_usage_count",
            "currentWeeklyUsageCount",
            "current_weekly_remaining_percent",
            "currentWeeklyRemainingPercent",
            "weekly_remains_time",
            "weeklyRemainsTime",
            "weekly_end_time",
            "weeklyEndTime",
            weekly_seconds,
            count_means_remaining,
        ) {
            limits.push(limit);
        }
    }
    if limits.is_empty() {
        return Err(MiniMaxError::new(
            MiniMaxErrorKind::InvalidResponse,
            "MiniMax quota response contained no usable windows.",
        ));
    }
    Ok(MiniMaxUsage {
        plan: plan_label(root, &text_models),
        limits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_percentage_based_coding_plan() {
        let payload = serde_json::json!({
            "base_resp": {"status_code": 0},
            "current_subscribe_title": "MiniMax Coding Plan Plus",
            "model_remains": [{
                "model_name": "general",
                "current_interval_total_count": 0,
                "current_interval_remaining_percent": 72,
                "current_weekly_total_count": 0,
                "current_weekly_remaining_percent": 64,
                "remains_time": 3_600_000,
                "weekly_remains_time": 86_400_000
            }]
        });
        let usage = parse_usage(&payload, true).expect("valid coding plan");
        assert_eq!(usage.plan, "Plus");
        assert_eq!(usage.limits.len(), 2);
        assert_eq!(usage.limits[0].used, 28.0);
        assert_eq!(usage.limits[1].used, 36.0);
    }

    #[test]
    fn parses_count_based_token_plan() {
        let payload = serde_json::json!({
            "base_resp": {"status_code": 0},
            "model_remains": [{
                "model_name": "MiniMax-M2",
                "current_interval_total_count": 1500,
                "current_interval_usage_count": 120,
                "current_weekly_total_count": 5000,
                "current_weekly_usage_count": 900
            }]
        });
        let usage = parse_usage(&payload, false).expect("valid token plan");
        assert_eq!(usage.plan, "Starter");
        assert_eq!(usage.limits[0].used, 120.0);
        assert_eq!(usage.limits[0].total, 1500.0);
        assert_eq!(usage.limits[1].used, 900.0);
    }

    #[test]
    fn rejects_non_minimax_credential_destinations() {
        assert!(is_allowed_minimax_url(TOKEN_PLAN_URL));
        assert!(is_allowed_minimax_url(CODING_PLAN_URL));
        assert!(!is_allowed_minimax_url(
            "http://api.minimax.io/v1/token_plan/remains"
        ));
        assert!(!is_allowed_minimax_url("https://example.com/collect"));
        assert!(!is_allowed_minimax_url(
            "https://api.minimax.io.example.com/collect"
        ));
    }
}

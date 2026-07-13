use std::{env, fs, path::PathBuf, sync::OnceLock, time::Duration};

use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::{redirect::Policy, StatusCode};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};

use crate::models::LimitMetric;

const TOKEN_KEY: &str = "antigravityUnifiedStateSync.oauthToken";
const LEGACY_TOKEN_KEY: &str = "jetskiStateSync.agentManagerInitState";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const CLIENT_ID_SUFFIX: &[u8] = b".apps.googleusercontent.com";
const CLIENT_SECRET_PREFIX: &[u8] = b"GOCSPX-";
const GOOGLE_CLIENT_SECRET_LENGTH: usize = 35;
const LOAD_URLS: [&str; 3] = [
    "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
    "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
    "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:loadCodeAssist",
];
const QUOTA_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";
const MAX_RESPONSE_BYTES: u64 = 2_097_152;
static OAUTH_CLIENTS: OnceLock<Result<Vec<OAuthClient>, String>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
struct OAuthClient {
    id: String,
    secret: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntigravityErrorKind {
    NeedsAuth,
    Network,
    Upstream,
    InvalidResponse,
}

#[derive(Debug, Clone)]
pub struct AntigravityError {
    pub kind: AntigravityErrorKind,
    pub message: String,
}

impl AntigravityError {
    fn new(kind: AntigravityErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AntigravityUsage {
    pub plan: String,
    pub limits: Vec<LimitMetric>,
}

fn state_db_path() -> Result<PathBuf, AntigravityError> {
    let roaming = env::var_os("APPDATA").ok_or_else(|| {
        AntigravityError::new(
            AntigravityErrorKind::NeedsAuth,
            "Windows app data is unavailable.",
        )
    })?;
    let path = PathBuf::from(roaming)
        .join("Antigravity")
        .join("User")
        .join("globalStorage")
        .join("state.vscdb");
    if !path.is_file() {
        return Err(AntigravityError::new(
            AntigravityErrorKind::NeedsAuth,
            "Antigravity session was not found. Install and sign in to Google Antigravity first.",
        ));
    }
    Ok(path)
}

fn language_server_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        paths.push(
            PathBuf::from(local)
                .join("Programs")
                .join("Antigravity")
                .join("resources")
                .join("bin")
                .join("language_server.exe"),
        );
    }
    if let Some(program_files) = env::var_os("ProgramFiles") {
        paths.push(
            PathBuf::from(program_files)
                .join("Antigravity")
                .join("resources")
                .join("bin")
                .join("language_server.exe"),
        );
    }
    paths
}

fn occurrences(bytes: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || bytes.len() < needle.len() {
        return Vec::new();
    }
    bytes
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, value)| (value == needle).then_some(index))
        .collect()
}

fn oauth_clients_from_binary(bytes: &[u8]) -> Vec<OAuthClient> {
    let mut ids = Vec::new();
    for suffix_at in occurrences(bytes, CLIENT_ID_SUFFIX) {
        let Some(hyphen_at) = bytes[..suffix_at].iter().rposition(|byte| *byte == b'-') else {
            continue;
        };
        let mut start = hyphen_at;
        while start > 0 && bytes[start - 1].is_ascii_digit() {
            start -= 1;
        }
        let hash = &bytes[hyphen_at + 1..suffix_at];
        if start == hyphen_at
            || !(20..=80).contains(&hash.len())
            || !hash
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            continue;
        }
        let end = suffix_at + CLIENT_ID_SUFFIX.len();
        if let Ok(id) = std::str::from_utf8(&bytes[start..end]) {
            if !ids.iter().any(|existing| existing == id) {
                ids.push(id.to_string());
            }
        }
    }

    let mut secrets = Vec::new();
    for start in occurrences(bytes, CLIENT_SECRET_PREFIX) {
        let Some(end) = start.checked_add(GOOGLE_CLIENT_SECRET_LENGTH) else {
            continue;
        };
        let Some(candidate) = bytes.get(start..end) else {
            continue;
        };
        if candidate
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            if let Ok(secret) = std::str::from_utf8(candidate) {
                if !secrets.iter().any(|existing| existing == secret) {
                    secrets.push(secret.to_string());
                }
            }
        }
    }

    ids.into_iter()
        .flat_map(|id| {
            secrets.iter().cloned().map(move |secret| OAuthClient {
                id: id.clone(),
                secret,
            })
        })
        .collect()
}

fn installed_oauth_clients() -> Result<&'static [OAuthClient], AntigravityError> {
    let result = OAUTH_CLIENTS.get_or_init(|| {
        for path in language_server_paths() {
            let Ok(bytes) = fs::read(path) else {
                continue;
            };
            let clients = oauth_clients_from_binary(&bytes);
            if !clients.is_empty() {
                return Ok(clients);
            }
        }
        Err("Antigravity OAuth metadata was not found in the installed app.".to_string())
    });
    result
        .as_deref()
        .map_err(|message| AntigravityError::new(AntigravityErrorKind::NeedsAuth, message.clone()))
}

fn read_state_value(connection: &Connection, key: &str) -> Option<String> {
    connection
        .query_row("SELECT value FROM ItemTable WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .ok()
}

fn varint(bytes: &[u8], offset: &mut usize) -> Option<u64> {
    let mut value = 0_u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*offset)?;
        *offset += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn field(bytes: &[u8], wanted: u64) -> Option<&[u8]> {
    let mut offset = 0;
    while offset < bytes.len() {
        let tag = varint(bytes, &mut offset)?;
        let number = tag >> 3;
        match tag & 7 {
            0 => {
                varint(bytes, &mut offset)?;
            }
            1 => offset = offset.checked_add(8)?,
            2 => {
                let length = usize::try_from(varint(bytes, &mut offset)?).ok()?;
                let end = offset.checked_add(length)?;
                let value = bytes.get(offset..end)?;
                if number == wanted {
                    return Some(value);
                }
                offset = end;
            }
            5 => offset = offset.checked_add(4)?,
            _ => return None,
        }
    }
    None
}

fn credential_message(encoded: &str, modern: bool) -> Option<Vec<u8>> {
    let decoded = STANDARD.decode(encoded.trim()).ok()?;
    if !modern {
        return field(&decoded, 6).map(ToOwned::to_owned);
    }
    let container = field(&decoded, 1)?;
    if field(container, 1)? != b"oauthTokenInfoSentinelKey" {
        return None;
    }
    let inner = field(container, 2)?;
    let info = std::str::from_utf8(field(inner, 1)?).ok()?;
    STANDARD.decode(info.trim()).ok()
}

fn local_refresh_token() -> Result<String, AntigravityError> {
    let connection =
        Connection::open_with_flags(state_db_path()?, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(
            |_| {
                AntigravityError::new(
                    AntigravityErrorKind::NeedsAuth,
                    "Antigravity session is currently locked. Close Antigravity and refresh again.",
                )
            },
        )?;
    let (encoded, modern) = read_state_value(&connection, TOKEN_KEY)
        .map(|value| (value, true))
        .or_else(|| read_state_value(&connection, LEGACY_TOKEN_KEY).map(|value| (value, false)))
        .ok_or_else(|| {
            AntigravityError::new(
                AntigravityErrorKind::NeedsAuth,
                "No signed-in Antigravity OAuth session was found.",
            )
        })?;
    let message = credential_message(&encoded, modern).ok_or_else(|| {
        AntigravityError::new(
            AntigravityErrorKind::InvalidResponse,
            "Antigravity OAuth state has an unsupported format.",
        )
    })?;
    let token = field(&message, 3)
        .and_then(|value| std::str::from_utf8(value).ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            AntigravityError::new(
                AntigravityErrorKind::NeedsAuth,
                "Antigravity OAuth session has no refresh token.",
            )
        })?;
    Ok(token.to_string())
}

pub fn has_local_config() -> bool {
    if local_refresh_token().is_ok() {
        return true;
    }
    let Ok(source) = state_db_path() else {
        return false;
    };
    let temporary = env::temp_dir().join(format!(
        "ai-bucket-antigravity-state-{}.sqlite",
        std::process::id()
    ));
    if fs::copy(source, &temporary).is_err() {
        return false;
    }
    let detected = Connection::open(&temporary)
        .and_then(|connection| {
            connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM ItemTable WHERE key IN (?1, ?2))",
                [TOKEN_KEY, LEGACY_TOKEN_KEY],
                |row| row.get::<_, bool>(0),
            )
        })
        .unwrap_or(false);
    let _ = fs::remove_file(temporary);
    detected
}

async fn response_json(response: reqwest::Response) -> Result<Value, AntigravityError> {
    if response.content_length().unwrap_or(0) > MAX_RESPONSE_BYTES {
        return Err(AntigravityError::new(
            AntigravityErrorKind::InvalidResponse,
            "Antigravity response was unexpectedly large.",
        ));
    }
    let bytes = response.bytes().await.map_err(|_| {
        AntigravityError::new(
            AntigravityErrorKind::InvalidResponse,
            "Unable to read Antigravity response.",
        )
    })?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(AntigravityError::new(
            AntigravityErrorKind::InvalidResponse,
            "Antigravity response was unexpectedly large.",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        AntigravityError::new(
            AntigravityErrorKind::InvalidResponse,
            "Antigravity returned invalid JSON.",
        )
    })
}

fn text_at<'a>(value: &'a Value, pointers: &[&str]) -> Option<&'a str> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn project_id(value: &Value) -> Option<String> {
    text_at(
        value,
        &["/cloudaicompanionProject", "/cloudaicompanionProject/id"],
    )
    .map(str::to_string)
}

fn plan_name(value: &Value) -> String {
    let tier = text_at(
        value,
        &["/paidTier/id", "/currentTier/id", "/onboardedUser/tier/id"],
    )
    .unwrap_or_default()
    .to_ascii_uppercase();
    let label = if tier.contains("ULTRA") {
        "Ultra"
    } else if tier.contains("PRO") || tier.contains("PREMIUM") || tier.contains("GOOGLE_ONE") {
        "Pro"
    } else if tier.contains("ENTERPRISE") {
        "Enterprise"
    } else if tier.contains("BUSINESS") || tier.contains("STANDARD") {
        "Business"
    } else if tier.contains("PLUS") {
        "Plus"
    } else if tier.contains("FREE") || tier.contains("INDIVIDUAL") {
        "Free"
    } else {
        "Antigravity"
    };
    label.to_string()
}

fn display_model(model: &str) -> String {
    model
        .replace(['_', '-'], " ")
        .split_whitespace()
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_quota(value: &Value) -> Result<Vec<LimitMetric>, AntigravityError> {
    let buckets = value
        .get("buckets")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            AntigravityError::new(
                AntigravityErrorKind::InvalidResponse,
                "Antigravity quota response has no buckets.",
            )
        })?;
    let mut limits = Vec::new();
    for bucket in buckets {
        let Some(model) = bucket
            .get("modelId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(remaining) = bucket.get("remainingFraction").and_then(Value::as_f64) else {
            continue;
        };
        limits.push(LimitMetric {
            id: format!(
                "model-{}",
                model
                    .to_ascii_lowercase()
                    .replace(|character: char| !character.is_ascii_alphanumeric(), "-")
            ),
            label: display_model(model),
            resource: Some(model.to_string()),
            used: ((1.0 - remaining.clamp(0.0, 1.0)) * 100.0).clamp(0.0, 100.0),
            total: 100.0,
            reset_at: bucket
                .get("resetTime")
                .and_then(Value::as_str)
                .map(str::to_string),
            window_seconds: None,
        });
    }
    if limits.is_empty() {
        return Err(AntigravityError::new(
            AntigravityErrorKind::InvalidResponse,
            "Antigravity returned no usable quota buckets.",
        ));
    }
    limits.sort_by(|left, right| left.label.cmp(&right.label));
    Ok(limits)
}

async fn oauth_access_token(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<String, AntigravityError> {
    let mut last_upstream_status = None;
    for oauth_client in installed_oauth_clients()? {
        let token_response = client
            .post(TOKEN_URL)
            .form(&[
                ("client_id", oauth_client.id.as_str()),
                ("client_secret", oauth_client.secret.as_str()),
                ("refresh_token", refresh_token),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|error| {
                AntigravityError::new(
                    AntigravityErrorKind::Network,
                    format!("Antigravity token refresh failed: {error}"),
                )
            })?;
        let token_status = token_response.status();
        let token_payload = response_json(token_response).await?;
        if token_status.is_success() {
            if let Some(access_token) = token_payload.get("access_token").and_then(Value::as_str) {
                return Ok(access_token.to_string());
            }
            return Err(AntigravityError::new(
                AntigravityErrorKind::InvalidResponse,
                "Google OAuth response has no access token.",
            ));
        }
        if !matches!(
            token_status,
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            last_upstream_status = Some(token_status);
        }
    }

    if let Some(status) = last_upstream_status {
        return Err(AntigravityError::new(
            AntigravityErrorKind::Upstream,
            format!("Google OAuth returned HTTP {status}."),
        ));
    }
    Err(AntigravityError::new(
        AntigravityErrorKind::NeedsAuth,
        "Antigravity session expired. Sign in to Antigravity again.",
    ))
}

pub async fn collect() -> Result<AntigravityUsage, AntigravityError> {
    let refresh_token = local_refresh_token()?;
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|_| {
            AntigravityError::new(
                AntigravityErrorKind::Network,
                "Unable to create HTTP client.",
            )
        })?;
    let access_token = oauth_access_token(&client, &refresh_token).await?;

    let mut bootstrap = None;
    for url in LOAD_URLS {
        let response = client
            .post(url)
            .bearer_auth(&access_token)
            .header("User-Agent", "vscode/1.X.X (Antigravity/1.0)")
            .json(&json!({"metadata": {"ideType": "ANTIGRAVITY"}}))
            .send()
            .await;
        let Ok(response) = response else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        let payload = response_json(response).await?;
        if project_id(&payload).is_some() {
            bootstrap = Some(payload);
            break;
        }
    }
    let bootstrap = bootstrap.ok_or_else(|| {
        AntigravityError::new(
            AntigravityErrorKind::Upstream,
            "Google Antigravity could not discover the account project.",
        )
    })?;
    let project = project_id(&bootstrap).expect("checked above");
    let response = client
        .post(QUOTA_URL)
        .bearer_auth(&access_token)
        .json(&json!({"project": project}))
        .send()
        .await
        .map_err(|error| {
            AntigravityError::new(
                AntigravityErrorKind::Network,
                format!("Antigravity quota request failed: {error}"),
            )
        })?;
    let status = response.status();
    let payload = response_json(response).await?;
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(AntigravityError::new(
            AntigravityErrorKind::NeedsAuth,
            "Google rejected the Antigravity session. Sign in again.",
        ));
    }
    if !status.is_success() {
        return Err(AntigravityError::new(
            AntigravityErrorKind::Upstream,
            format!("Antigravity quota endpoint returned HTTP {status}."),
        ));
    }
    Ok(AntigravityUsage {
        plan: plan_name(&bootstrap),
        limits: parse_quota(&payload)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quota_as_used_percentage() {
        let limits = parse_quota(&json!({"buckets": [{"modelId": "gemini-2.5-pro", "remainingFraction": 0.73, "resetTime": "2026-07-13T00:00:00Z"}]})).unwrap();
        assert!((limits[0].used - 27.0).abs() < 0.001);
        assert_eq!(limits[0].total, 100.0);
        assert_eq!(limits[0].reset_at.as_deref(), Some("2026-07-13T00:00:00Z"));
    }

    #[test]
    fn extracts_oauth_clients_without_hardcoded_credentials() {
        let mut fixture =
            b"prefix123456789012-client_hash_1234567890.apps.googleusercontent.commiddle".to_vec();
        fixture.extend_from_slice(CLIENT_SECRET_PREFIX);
        fixture.extend_from_slice(b"1234567890123456789012345678suffix");
        let clients = oauth_clients_from_binary(&fixture);
        assert_eq!(clients.len(), 1);
        assert_eq!(
            clients[0].id,
            "123456789012-client_hash_1234567890.apps.googleusercontent.com"
        );
        assert_eq!(
            clients[0].secret,
            format!(
                "{}1234567890123456789012345678",
                std::str::from_utf8(CLIENT_SECRET_PREFIX).unwrap()
            )
        );
    }
}

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use chrono::Utc;
use rand::{rngs::OsRng, RngCore};
use reqwest::{redirect::Policy, StatusCode};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
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
const OAUTH_AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const OAUTH_TOKEN_URL: &str = "https://api.anthropic.com/v1/oauth/token";
const OAUTH_BOOTSTRAP_URL: &str = "https://api.anthropic.com/api/claude_cli/bootstrap";
const OAUTH_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const CLAUDE_CODE_VERSION: &str = "2.1.207";
const OAUTH_SCOPES: &str =
    "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers";
const MAX_RESPONSE_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, Serialize)]
pub struct ClaudeOAuthStart {
    #[serde(rename = "authUrl")]
    pub auth_url: String,
}

#[derive(Debug, Clone)]
pub struct ClaudeOAuthPending {
    pub state: String,
    pub verifier: String,
    pub created_at: std::time::Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeOAuthCredential {
    pub version: u8,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    #[serde(default)]
    pub account_uuid: Option<String>,
    #[serde(default)]
    pub account_email: Option<String>,
    #[serde(default)]
    pub organization_uuid: Option<String>,
    #[serde(default)]
    pub organization_name: Option<String>,
    #[serde(default)]
    pub organization_rate_limit_tier: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: i64,
    #[serde(default)]
    scope: Option<String>,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeLocalCredentialCache {
    pub version: u8,
    pub kind: String,
    pub cookie_header: String,
    pub captured_at: String,
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

type CookieRow = (String, String, Vec<u8>);

fn read_cookie_rows(cookie_path: &Path) -> rusqlite::Result<Vec<CookieRow>> {
    let connection =
        Connection::open_with_flags(cookie_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = connection.prepare(
        "SELECT host_key, name, encrypted_value FROM cookies
         WHERE host_key IN ('.claude.ai', 'claude.ai')
           AND name IN ('sessionKey', 'routingHint')",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?
        .collect();
    rows
}

fn snapshot_cookie_database(cookie_path: &Path) -> Result<PathBuf, ClaudeError> {
    let mut nonce = [0_u8; 8];
    OsRng.fill_bytes(&mut nonce);
    let snapshot_dir = env::temp_dir().join(format!(
        "ai-bucket-claude-cookies-{}-{}",
        std::process::id(),
        u64::from_le_bytes(nonce)
    ));
    fs::create_dir(&snapshot_dir).map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::CredentialLocked,
            "Claude Desktop session could not be copied for validation. Retry refresh, or quit Claude once to initialize the encrypted cache.",
        )
    })?;

    for suffix in ["", "-wal", "-shm"] {
        let source = cookie_path.with_file_name(format!("Cookies{suffix}"));
        if suffix.is_empty() || source.exists() {
            let destination = snapshot_dir.join(format!("Cookies{suffix}"));
            if fs::copy(&source, destination).is_err() {
                let _ = fs::remove_dir_all(&snapshot_dir);
                return Err(ClaudeError::new(
                    ClaudeErrorKind::CredentialLocked,
                    "Claude Desktop session could not be copied while it was changing. Retry refresh, or quit Claude once to initialize the encrypted cache.",
                ));
            }
        }
    }
    Ok(snapshot_dir)
}

fn read_cookie_rows_with_snapshot(cookie_path: &Path) -> Result<Vec<CookieRow>, ClaudeError> {
    if let Ok(rows) = read_cookie_rows(cookie_path) {
        return Ok(rows);
    }

    let snapshot_dir = snapshot_cookie_database(cookie_path)?;
    let result = read_cookie_rows(&snapshot_dir.join("Cookies")).map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::CredentialLocked,
            "Claude Desktop session snapshot could not be validated. Retry refresh, or quit Claude once to initialize the encrypted cache.",
        )
    });
    let _ = fs::remove_dir_all(snapshot_dir);
    result
}

fn read_cookie_header() -> Result<String, ClaudeError> {
    let profile = profile_dir()?;
    let key = encryption_key(&profile)?;
    let cookie_path = profile.join("Network").join("Cookies");
    let rows = read_cookie_rows_with_snapshot(&cookie_path)?;
    let mut cookies = Vec::new();
    for row in rows {
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
    read_cookie_rows_with_snapshot(&profile.join("Network").join("Cookies"))
        .is_ok_and(|rows| rows.iter().any(|row| row.1 == "sessionKey"))
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

async fn collect_payload(cookie: &str) -> Result<(Value, String), ClaudeError> {
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|_| ClaudeError::new(ClaudeErrorKind::Network, "Unable to create HTTP client"))?;
    let bootstrap = json_get(&client, BOOTSTRAP_URL, cookie).await?;
    let (organization_id, plan) = active_organization(&bootstrap).ok_or_else(|| {
        ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "Claude account has no active organization",
        )
    })?;
    let usage_url = format!("https://claude.ai/api/organizations/{organization_id}/usage");
    let usage = json_get(&client, &usage_url, cookie).await?;
    Ok((usage, plan))
}

pub async fn collect_and_capture_local(
) -> Result<(ClaudeUsage, ClaudeLocalCredentialCache), ClaudeError> {
    let cookie_header = read_cookie_header()?;
    let (usage, plan) = collect_payload(&cookie_header).await?;
    let usage = parse_usage(&usage, plan)?;
    Ok((
        usage,
        ClaudeLocalCredentialCache {
            version: 1,
            kind: "claude_desktop_cookie".into(),
            cookie_header,
            captured_at: Utc::now().to_rfc3339(),
        },
    ))
}

pub async fn collect_cached_local(
    credential: &ClaudeLocalCredentialCache,
) -> Result<ClaudeUsage, ClaudeError> {
    if credential.kind != "claude_desktop_cookie" || credential.cookie_header.trim().is_empty() {
        return Err(ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "Stored Claude Desktop session cache is invalid.",
        ));
    }
    let (usage, plan) = collect_payload(&credential.cookie_header).await?;
    parse_usage(&usage, plan)
}

pub fn decode_local_credential_cache(
    value: &str,
) -> Result<ClaudeLocalCredentialCache, ClaudeError> {
    serde_json::from_str(value).map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "Stored Claude Desktop session cache is invalid.",
        )
    })
}

pub fn encode_local_credential_cache(
    credential: &ClaudeLocalCredentialCache,
) -> Result<String, ClaudeError> {
    serde_json::to_string(credential).map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Unable to store the Claude Desktop session cache.",
        )
    })
}

fn oauth_client() -> Result<reqwest::Client, ClaudeError> {
    reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| ClaudeError::new(ClaudeErrorKind::Network, "Unable to create OAuth client"))
}

fn random_url_safe(byte_count: usize) -> String {
    let mut bytes = vec![0_u8; byte_count];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn begin_oauth() -> Result<(ClaudeOAuthPending, ClaudeOAuthStart), ClaudeError> {
    let verifier = random_url_safe(48);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_url_safe(24);
    let mut url = url::Url::parse(OAUTH_AUTHORIZE_URL).map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Claude OAuth URL is invalid",
        )
    })?;
    url.query_pairs_mut()
        .append_pair("code", "true")
        .append_pair("client_id", OAUTH_CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", OAUTH_REDIRECT_URI)
        .append_pair("scope", OAUTH_SCOPES)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state)
        .append_pair("prompt", "login");
    Ok((
        ClaudeOAuthPending {
            state,
            verifier,
            created_at: std::time::Instant::now(),
        },
        ClaudeOAuthStart {
            auth_url: url.into(),
        },
    ))
}

pub fn parse_authorization_code(input: &str) -> Result<(String, String), ClaudeError> {
    let trimmed = input.trim();
    if let Ok(url) = url::Url::parse(trimmed) {
        let code = url
            .query_pairs()
            .find_map(|(key, value)| (key == "code").then(|| value.into_owned()));
        let state = url
            .query_pairs()
            .find_map(|(key, value)| (key == "state").then(|| value.into_owned()));
        if let (Some(code), Some(state)) = (code, state) {
            return Ok((code, state));
        }
    }
    let Some((code, state)) = trimmed.rsplit_once('#') else {
        return Err(ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Paste the complete Claude authorization code, including the #state suffix.",
        ));
    };
    if code.trim().is_empty() || state.trim().is_empty() {
        return Err(ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Claude returned an incomplete authorization code.",
        ));
    }
    Ok((code.trim().to_owned(), state.trim().to_owned()))
}

async fn oauth_json(response: reqwest::Response, context: &str) -> Result<Value, ClaudeError> {
    let status = response.status();
    let body = response.bytes().await.map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            format!("Unable to read {context}"),
        )
    })?;
    if body.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            format!("{context} was unexpectedly large"),
        ));
    }
    let value: Value = serde_json::from_slice(&body).map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            format!("{context} returned invalid JSON"),
        )
    })?;
    if !status.is_success() {
        let detail = value
            .pointer("/error/message")
            .or_else(|| value.get("error_description"))
            .or_else(|| value.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("Claude rejected the OAuth request");
        let kind = if matches!(
            status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::BAD_REQUEST
        ) {
            ClaudeErrorKind::NeedsAuth
        } else {
            ClaudeErrorKind::Upstream
        };
        return Err(ClaudeError::new(kind, format!("{detail} (HTTP {status})")));
    }
    Ok(value)
}

fn oauth_headers(request: reqwest::RequestBuilder, user_agent: &str) -> reqwest::RequestBuilder {
    request
        .header("Accept", "application/json")
        .header("User-Agent", user_agent)
        .header("anthropic-beta", OAUTH_BETA)
}

async fn oauth_bootstrap(client: &reqwest::Client, access_token: &str) -> Option<Value> {
    let response = oauth_headers(
        client.get(OAUTH_BOOTSTRAP_URL).bearer_auth(access_token),
        &format!("claude-cli/{CLAUDE_CODE_VERSION} (external, cli)"),
    )
    .send()
    .await
    .ok()?;
    oauth_json(response, "Claude account bootstrap").await.ok()
}

fn apply_bootstrap_identity(credential: &mut ClaudeOAuthCredential, bootstrap: Option<&Value>) {
    let Some(value) = bootstrap else { return };
    let account = value.pointer("/oauth_account").unwrap_or(value);
    credential.account_uuid = account
        .get("account_uuid")
        .or_else(|| account.pointer("/account/uuid"))
        .or_else(|| account.pointer("/account/id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    credential.account_email = account
        .get("account_email")
        .or_else(|| account.pointer("/account/email"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    credential.organization_uuid = account
        .get("organization_uuid")
        .or_else(|| account.pointer("/organization/uuid"))
        .or_else(|| account.pointer("/organization/id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    credential.organization_name = account
        .get("organization_name")
        .or_else(|| account.pointer("/organization/name"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    credential.organization_rate_limit_tier = account
        .get("organization_rate_limit_tier")
        .or_else(|| account.pointer("/organization/rate_limit_tier"))
        .or_else(|| account.get("subscription_type"))
        .and_then(Value::as_str)
        .map(str::to_owned);
}

pub async fn exchange_oauth_code(
    pending: &ClaudeOAuthPending,
    input: &str,
) -> Result<ClaudeOAuthCredential, ClaudeError> {
    if pending.created_at.elapsed() > Duration::from_secs(10 * 60) {
        return Err(ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "This Claude sign-in attempt expired. Start sign-in again.",
        ));
    }
    let (code, state) = parse_authorization_code(input)?;
    if state != pending.state {
        return Err(ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "Claude OAuth state did not match this sign-in attempt.",
        ));
    }
    let client = oauth_client()?;
    let response = client
        .post(OAUTH_TOKEN_URL)
        .header("anthropic-beta", OAUTH_BETA)
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "state": state,
            "client_id": OAUTH_CLIENT_ID,
            "redirect_uri": OAUTH_REDIRECT_URI,
            "code_verifier": pending.verifier,
        }))
        .send()
        .await
        .map_err(|error| {
            ClaudeError::new(
                ClaudeErrorKind::Network,
                format!("Claude sign-in failed: {error}"),
            )
        })?;
    let token: OAuthTokenResponse = serde_json::from_value(
        oauth_json(response, "Claude token exchange").await?,
    )
    .map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Claude returned an invalid token response",
        )
    })?;
    let refresh_token = token
        .refresh_token
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ClaudeError::new(
                ClaudeErrorKind::InvalidResponse,
                "Claude did not return a refresh token",
            )
        })?;
    let mut credential = ClaudeOAuthCredential {
        version: 1,
        access_token: token.access_token,
        refresh_token,
        expires_at: Utc::now().timestamp() + token.expires_in.max(60),
        account_uuid: None,
        account_email: None,
        organization_uuid: None,
        organization_name: None,
        organization_rate_limit_tier: None,
        scope: token.scope,
    };
    let bootstrap = oauth_bootstrap(&client, &credential.access_token).await;
    apply_bootstrap_identity(&mut credential, bootstrap.as_ref());
    Ok(credential)
}

pub fn decode_oauth_credential(value: &str) -> Result<ClaudeOAuthCredential, ClaudeError> {
    serde_json::from_str(value).map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::NeedsAuth,
            "Stored Claude OAuth credential is invalid. Sign in again.",
        )
    })
}

pub fn encode_oauth_credential(credential: &ClaudeOAuthCredential) -> Result<String, ClaudeError> {
    serde_json::to_string(credential).map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Unable to store Claude OAuth credential",
        )
    })
}

pub fn credential_needs_refresh(credential: &ClaudeOAuthCredential) -> bool {
    credential.expires_at <= Utc::now().timestamp() + 300
}

pub async fn refresh_oauth_credential(
    credential: &ClaudeOAuthCredential,
) -> Result<ClaudeOAuthCredential, ClaudeError> {
    let client = oauth_client()?;
    let response = client
        .post(OAUTH_TOKEN_URL)
        .header("anthropic-beta", OAUTH_BETA)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", credential.refresh_token.as_str()),
            ("client_id", OAUTH_CLIENT_ID),
        ])
        .send()
        .await
        .map_err(|error| {
            ClaudeError::new(
                ClaudeErrorKind::Network,
                format!("Claude token refresh failed: {error}"),
            )
        })?;
    let token: OAuthTokenResponse = serde_json::from_value(
        oauth_json(response, "Claude token refresh").await?,
    )
    .map_err(|_| {
        ClaudeError::new(
            ClaudeErrorKind::InvalidResponse,
            "Claude returned an invalid refresh response",
        )
    })?;
    let mut next = credential.clone();
    next.access_token = token.access_token;
    if let Some(refresh_token) = token.refresh_token.filter(|value| !value.is_empty()) {
        next.refresh_token = refresh_token;
    }
    next.expires_at = Utc::now().timestamp() + token.expires_in.max(60);
    if token.scope.is_some() {
        next.scope = token.scope;
    }
    let bootstrap = oauth_bootstrap(&client, &next.access_token).await;
    apply_bootstrap_identity(&mut next, bootstrap.as_ref());
    Ok(next)
}

pub async fn collect_oauth(credential: &ClaudeOAuthCredential) -> Result<ClaudeUsage, ClaudeError> {
    let client = oauth_client()?;
    let response = oauth_headers(
        client
            .get(OAUTH_USAGE_URL)
            .bearer_auth(&credential.access_token),
        &format!("claude-code/{CLAUDE_CODE_VERSION}"),
    )
    .send()
    .await
    .map_err(|error| {
        ClaudeError::new(
            ClaudeErrorKind::Network,
            format!("Claude quota request failed: {error}"),
        )
    })?;
    let usage = oauth_json(response, "Claude OAuth quota endpoint").await?;
    let plan = credential
        .organization_rate_limit_tier
        .clone()
        .or_else(|| {
            ["tier", "plan", "subscription_type", "rate_limit_tier"]
                .iter()
                .find_map(|key| {
                    usage
                        .get(*key)
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_owned)
                })
        })
        .unwrap_or_else(|| "Claude subscription".to_string());
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
    fn builds_pkce_authorization_url_with_required_scopes() {
        let (pending, start) = begin_oauth().expect("OAuth URL");
        let url = url::Url::parse(&start.auth_url).expect("valid URL");
        let query = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(url.as_str().split('?').next(), Some(OAUTH_AUTHORIZE_URL));
        assert_eq!(
            query.get("client_id").map(|value| value.as_ref()),
            Some(OAUTH_CLIENT_ID)
        );
        assert_eq!(
            query
                .get("code_challenge_method")
                .map(|value| value.as_ref()),
            Some("S256")
        );
        assert_eq!(
            query.get("prompt").map(|value| value.as_ref()),
            Some("login")
        );
        assert_eq!(
            query.get("state").map(|value| value.as_ref()),
            Some(pending.state.as_str())
        );
        assert!(query
            .get("scope")
            .is_some_and(|value| value.contains("user:profile")));
        assert!(pending.verifier.len() >= 43);
    }

    #[test]
    fn parses_manual_and_callback_authorization_codes() {
        assert_eq!(
            parse_authorization_code("code-value#state-value").expect("manual code"),
            ("code-value".into(), "state-value".into())
        );
        assert_eq!(
            parse_authorization_code(
                "https://platform.claude.com/oauth/code/callback?code=abc%2B123&state=xyz"
            )
            .expect("callback URL"),
            ("abc+123".into(), "xyz".into())
        );
        assert!(parse_authorization_code("missing-state").is_err());
    }

    #[test]
    fn oauth_credential_round_trips_without_losing_refresh_token() {
        let credential = ClaudeOAuthCredential {
            version: 1,
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at: 123,
            account_uuid: Some("account".into()),
            account_email: None,
            organization_uuid: None,
            organization_name: None,
            organization_rate_limit_tier: Some("max".into()),
            scope: Some("user:profile".into()),
        };
        let encoded = encode_oauth_credential(&credential).expect("encode");
        let decoded = decode_oauth_credential(&encoded).expect("decode");
        assert_eq!(decoded.refresh_token, "refresh");
        assert_eq!(decoded.account_uuid.as_deref(), Some("account"));
    }

    #[test]
    fn reads_identity_from_claude_cli_bootstrap_shape() {
        let mut credential = ClaudeOAuthCredential {
            version: 1,
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at: 123,
            account_uuid: None,
            account_email: None,
            organization_uuid: None,
            organization_name: None,
            organization_rate_limit_tier: None,
            scope: None,
        };
        let bootstrap = serde_json::json!({
            "oauth_account": {
                "account_uuid": "account-id",
                "account_email": "person@example.com",
                "organization_uuid": "org-id",
                "organization_name": "Example",
                "organization_rate_limit_tier": "default_claude_max_20x"
            }
        });
        apply_bootstrap_identity(&mut credential, Some(&bootstrap));
        assert_eq!(credential.account_uuid.as_deref(), Some("account-id"));
        assert_eq!(credential.organization_uuid.as_deref(), Some("org-id"));
        assert_eq!(
            credential.organization_rate_limit_tier.as_deref(),
            Some("default_claude_max_20x")
        );
    }

    #[test]
    fn local_session_cache_round_trips_and_is_distinct_from_oauth() {
        let cached = ClaudeLocalCredentialCache {
            version: 1,
            kind: "claude_desktop_cookie".into(),
            cookie_header: "sessionKey=secret; routingHint=route".into(),
            captured_at: "2026-07-16T00:00:00Z".into(),
        };
        let encoded = encode_local_credential_cache(&cached).expect("encode local cache");
        let decoded = decode_local_credential_cache(&encoded).expect("decode local cache");
        assert_eq!(decoded.cookie_header, cached.cookie_header);

        let oauth = ClaudeOAuthCredential {
            version: 1,
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at: 123,
            account_uuid: None,
            account_email: None,
            organization_uuid: None,
            organization_name: None,
            organization_rate_limit_tier: None,
            scope: None,
        };
        let oauth_encoded = encode_oauth_credential(&oauth).expect("encode OAuth");
        assert!(decode_local_credential_cache(&oauth_encoded).is_err());
    }

    #[test]
    fn snapshots_cookie_database_without_leaving_temporary_files() {
        let mut nonce = [0_u8; 8];
        OsRng.fill_bytes(&mut nonce);
        let source_dir = env::temp_dir().join(format!(
            "ai-bucket-claude-snapshot-test-{}-{}",
            std::process::id(),
            u64::from_le_bytes(nonce)
        ));
        fs::create_dir(&source_dir).expect("create source directory");
        let cookie_path = source_dir.join("Cookies");
        let connection = Connection::open(&cookie_path).expect("create cookie database");
        connection
            .execute_batch(
                "CREATE TABLE cookies (
                    host_key TEXT NOT NULL,
                    name TEXT NOT NULL,
                    encrypted_value BLOB NOT NULL
                );
                INSERT INTO cookies VALUES ('.claude.ai', 'sessionKey', X'010203');",
            )
            .expect("seed cookie database");
        drop(connection);

        let snapshot_dir = snapshot_cookie_database(&cookie_path).expect("snapshot database");
        let rows = read_cookie_rows(&snapshot_dir.join("Cookies")).expect("read snapshot");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "sessionKey");

        fs::remove_dir_all(&snapshot_dir).expect("remove snapshot directory");
        fs::remove_dir_all(&source_dir).expect("remove source directory");
        assert!(!snapshot_dir.exists());
    }

    #[test]
    #[ignore = "uses the local Claude Desktop session and calls live quota endpoints"]
    fn live_collector_returns_quota_windows() {
        let (usage, _) = tauri::async_runtime::block_on(collect_and_capture_local())
            .expect("live Claude quota request");
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

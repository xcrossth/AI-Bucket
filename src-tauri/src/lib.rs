mod credentials;
mod models;
mod providers;
mod store;

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, PhysicalPosition, PhysicalSize, Position, Size, State, WebviewWindow,
    WindowEvent,
};
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

#[cfg(not(windows))]
use tauri_plugin_notification::NotificationExt;
#[cfg(windows)]
use tauri_winrt_notification::{IconCrop, Toast};
#[cfg(windows)]
use windows_registry::CURRENT_USER;

#[cfg(windows)]
const WINDOWS_NOTIFICATION_APP_ID: &str = "com.local.ai-bucket.notification";
const TRAY_ID: &str = "ai-bucket-tray";

use models::{
    AppSettings, DashboardState, ProviderConfig, ProviderSnapshot, SettingsPatch, PROVIDER_IDS,
};
use providers::antigravity::{AntigravityErrorKind, AntigravityUsage};
use providers::claude::{ClaudeErrorKind, ClaudeUsage};
use providers::codex::{CodexErrorKind, CodexUsage};
use providers::glm::{GlmErrorKind, GlmUsage};
use providers::minimax::{MiniMaxErrorKind, MiniMaxUsage};

struct AppState {
    db_path: Mutex<PathBuf>,
    credential_dir: Mutex<PathBuf>,
    notified_usage: Mutex<HashMap<(i64, String), u32>>,
    layouts_path: PathBuf,
    window_mode: Arc<Mutex<String>>,
    minimize_to_tray: Arc<AtomicBool>,
    claude_oauth_sessions: Mutex<HashMap<i64, providers::claude::ClaudeOAuthPending>>,
    claude_oauth_locks: Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct WindowLayout {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    maximized: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct WindowLayouts {
    normal: Option<WindowLayout>,
    widget: Option<WindowLayout>,
}

fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

fn db_path(state: &State<'_, AppState>) -> Result<PathBuf, String> {
    state
        .db_path
        .lock()
        .map(|path| path.clone())
        .map_err(|_| "Failed to lock database path".to_string())
}

fn credential_dir(state: &State<'_, AppState>) -> Result<PathBuf, String> {
    state
        .credential_dir
        .lock()
        .map(|path| path.clone())
        .map_err(|_| "Failed to lock credential path".to_string())
}

fn connect(path: &PathBuf) -> Result<Connection, String> {
    Connection::open(path).map_err(|error| error.to_string())
}

fn window_state_flags() -> StateFlags {
    StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED | StateFlags::VISIBLE
}

fn window_exceeds_monitor(
    window_width: u32,
    window_height: u32,
    monitor_width: u32,
    monitor_height: u32,
) -> bool {
    window_width > monitor_width || window_height > monitor_height
}

fn read_window_layouts(path: &Path) -> WindowLayouts {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_window_layouts(path: &Path, layouts: &WindowLayouts) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(layouts).map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| error.to_string())
}

fn capture_window_layout(window: &WebviewWindow) -> Result<WindowLayout, String> {
    let position = window.outer_position().map_err(|error| error.to_string())?;
    let size = window.inner_size().map_err(|error| error.to_string())?;
    Ok(WindowLayout {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        maximized: window.is_maximized().map_err(|error| error.to_string())?,
    })
}

fn save_mode_layout(window: &WebviewWindow, path: &Path, mode: &str) -> Result<(), String> {
    let mut layouts = read_window_layouts(path);
    let layout = capture_window_layout(window)?;
    if mode == "widget" {
        layouts.widget = Some(layout);
    } else {
        layouts.normal = Some(layout);
    }
    write_window_layouts(path, &layouts)
}

fn clamped_layout(
    window: &WebviewWindow,
    mut layout: WindowLayout,
) -> Result<WindowLayout, String> {
    let monitor = window
        .current_monitor()
        .map_err(|error| error.to_string())?
        .or(window
            .primary_monitor()
            .map_err(|error| error.to_string())?);
    if let Some(monitor) = monitor {
        let origin = monitor.position();
        let size = monitor.size();
        layout.width = layout.width.min(size.width).max(360);
        layout.height = layout.height.min(size.height).max(320);
        let max_x = origin.x + size.width.saturating_sub(layout.width) as i32;
        let max_y = origin.y + size.height.saturating_sub(layout.height) as i32;
        layout.x = layout.x.clamp(origin.x, max_x);
        layout.y = layout.y.clamp(origin.y, max_y);
    }
    Ok(layout)
}

fn restore_mode_layout(window: &WebviewWindow, path: &Path, mode: &str) -> Result<(), String> {
    let layouts = read_window_layouts(path);
    let saved = if mode == "widget" {
        layouts.widget
    } else {
        layouts.normal
    };
    let fallback = if mode == "widget" {
        let position = window.outer_position().map_err(|error| error.to_string())?;
        WindowLayout {
            x: position.x,
            y: position.y,
            width: 560,
            height: 900,
            maximized: false,
        }
    } else {
        capture_window_layout(window)?
    };
    let layout = clamped_layout(window, saved.unwrap_or(fallback))?;
    window.unmaximize().map_err(|error| error.to_string())?;
    window
        .set_size(Size::Physical(PhysicalSize::new(
            layout.width,
            layout.height,
        )))
        .map_err(|error| error.to_string())?;
    window
        .set_position(Position::Physical(PhysicalPosition::new(
            layout.x, layout.y,
        )))
        .map_err(|error| error.to_string())?;
    if mode == "normal" && layout.maximized {
        window.maximize().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn apply_window_mode(
    window: &WebviewWindow,
    layouts_path: &Path,
    mode: &str,
    always_on_top: bool,
) -> Result<(), String> {
    window
        .set_decorations(mode != "widget")
        .map_err(|error| error.to_string())?;
    window
        .set_shadow(mode != "widget")
        .map_err(|error| error.to_string())?;
    window
        .set_always_on_top(mode == "widget" && always_on_top)
        .map_err(|error| error.to_string())?;
    restore_mode_layout(window, layouts_path, mode)
}

fn show_main_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window is unavailable".to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

fn toggle_main_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window is unavailable".to_string())?;
    let visible = window.is_visible().map_err(|error| error.to_string())?;
    let minimized = window.is_minimized().map_err(|error| error.to_string())?;
    if visible && !minimized {
        let state = app.state::<AppState>();
        persist_current_window(app, &state)?;
        window.hide().map_err(|error| error.to_string())
    } else {
        show_main_window(app)
    }
}

fn persist_current_window(app: &AppHandle, state: &AppState) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        let mode = state
            .window_mode
            .lock()
            .map_err(|_| "Failed to lock window mode".to_string())?
            .clone();
        save_mode_layout(&window, &state.layouts_path, &mode)?;
        let _ = app.save_window_state(window_state_flags());
    }
    Ok(())
}

fn start_window_state_saver(
    app: AppHandle,
    window: WebviewWindow,
    layouts_path: PathBuf,
    mode: Arc<Mutex<String>>,
) -> SyncSender<()> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        while receiver.recv().is_ok() {
            loop {
                match receiver.recv_timeout(Duration::from_millis(400)) {
                    Ok(()) => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let _ = app.save_window_state(window_state_flags());
                        let current_mode = mode
                            .lock()
                            .map(|value| value.clone())
                            .unwrap_or_else(|_| "normal".into());
                        let _ = save_mode_layout(&window, &layouts_path, &current_mode);
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        let _ = app.save_window_state(window_state_flags());
                        let current_mode = mode
                            .lock()
                            .map(|value| value.clone())
                            .unwrap_or_else(|_| "normal".into());
                        let _ = save_mode_layout(&window, &layouts_path, &current_mode);
                        return;
                    }
                }
            }
        }
    });
    sender
}

fn simulate_refresh(snapshot: &ProviderSnapshot) -> ProviderSnapshot {
    let mut next = snapshot.clone();
    let tick = (Utc::now().timestamp() % 4 + 1) as f64;
    for (index, limit) in next.limits.iter_mut().enumerate() {
        limit.used = (limit.used + tick + index as f64).min(limit.total);
    }
    next.last_updated = now_iso();
    next
}

fn apply_codex_usage(snapshot: &ProviderSnapshot, usage: CodexUsage) -> ProviderSnapshot {
    let mut next = snapshot.clone();
    next.plan_name = usage.plan;
    next.status = "ready".into();
    next.fetch_mode = "local_credential".into();
    next.limits = stabilize_codex_limits(&snapshot.limits, usage.limits);
    next.last_updated = now_iso();
    next.notes = "Quota loaded from the local Codex CLI session.".into();
    next.configured = true;
    next
}

fn stabilize_codex_limits(
    current: &[models::LimitMetric],
    mut incoming: Vec<models::LimitMetric>,
) -> Vec<models::LimitMetric> {
    let now = Utc::now();
    for next in &mut incoming {
        let Some(previous) = current.iter().find(|limit| limit.id == next.id) else {
            continue;
        };
        if previous.used <= next.used {
            continue;
        }
        let Some(previous_reset) = previous
            .reset_at
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        else {
            continue;
        };
        let Some(next_reset) = next
            .reset_at
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        else {
            continue;
        };
        let reset_delta = (previous_reset.timestamp() - next_reset.timestamp()).abs();
        if previous_reset > now && next_reset > now && reset_delta <= 15 * 60 {
            *next = previous.clone();
        }
    }
    incoming
}

fn apply_codex_error(
    snapshot: &ProviderSnapshot,
    kind: CodexErrorKind,
    message: String,
) -> ProviderSnapshot {
    let mut next = snapshot.clone();
    next.status = if kind == CodexErrorKind::NeedsAuth {
        "needs_auth".into()
    } else {
        "error".into()
    };
    next.last_updated = now_iso();
    next.notes = message;
    next
}

fn apply_claude_usage(
    snapshot: &ProviderSnapshot,
    usage: ClaudeUsage,
    fetch_mode: &str,
) -> ProviderSnapshot {
    let mut next = snapshot.clone();
    next.plan_name = usage.plan;
    next.status = "ready".into();
    next.fetch_mode = fetch_mode.into();
    next.limits = usage.limits;
    next.last_updated = now_iso();
    next.notes = if fetch_mode == "oauth" {
        "Quota loaded from this card's Claude OAuth session.".into()
    } else {
        "Quota loaded from the local Claude Desktop session.".into()
    };
    next.configured = true;
    next
}

fn apply_claude_error(
    snapshot: &ProviderSnapshot,
    kind: ClaudeErrorKind,
    message: String,
) -> ProviderSnapshot {
    let mut next = snapshot.clone();
    next.status = match kind {
        ClaudeErrorKind::NeedsAuth => "needs_auth".into(),
        ClaudeErrorKind::CredentialLocked => "credential_locked".into(),
        _ => "error".into(),
    };
    next.last_updated = now_iso();
    next.notes = message;
    next
}

fn apply_antigravity_usage(
    snapshot: &ProviderSnapshot,
    usage: AntigravityUsage,
) -> ProviderSnapshot {
    let mut next = snapshot.clone();
    let (provider_name, icon, accent_color) = models::provider_presentation("google");
    next.provider_name = provider_name.into();
    next.icon = icon.into();
    next.accent_color = accent_color.into();
    next.plan_name = usage.plan;
    next.status = "ready".into();
    next.fetch_mode = "local_credential".into();
    next.limits = usage.limits;
    next.last_updated = now_iso();
    next.notes = "Quota loaded from the local Google Antigravity OAuth session.".into();
    next.configured = true;
    next
}

fn apply_antigravity_error(
    snapshot: &ProviderSnapshot,
    kind: AntigravityErrorKind,
    message: String,
) -> ProviderSnapshot {
    let mut next = snapshot.clone();
    let (provider_name, icon, accent_color) = models::provider_presentation("google");
    next.provider_name = provider_name.into();
    next.icon = icon.into();
    next.accent_color = accent_color.into();
    next.status = if kind == AntigravityErrorKind::NeedsAuth {
        "needs_auth".into()
    } else {
        "error".into()
    };
    next.last_updated = now_iso();
    next.notes = message;
    next.configured = true;
    next
}

fn apply_minimax_usage(snapshot: &ProviderSnapshot, usage: MiniMaxUsage) -> ProviderSnapshot {
    let mut next = snapshot.clone();
    next.plan_name = usage.plan;
    next.status = "ready".into();
    next.fetch_mode = "api_key".into();
    next.limits = usage.limits;
    next.last_updated = now_iso();
    next.notes = "Quota loaded from the MiniMax Token/Coding Plan API.".into();
    next.configured = true;
    next
}

fn apply_minimax_error(
    snapshot: &ProviderSnapshot,
    kind: MiniMaxErrorKind,
    message: String,
    has_key: bool,
) -> ProviderSnapshot {
    let mut next = snapshot.clone();
    next.status = if matches!(kind, MiniMaxErrorKind::NeedsConfig | MiniMaxErrorKind::Auth) {
        "needs_config".into()
    } else {
        "error".into()
    };
    next.last_updated = now_iso();
    next.notes = message;
    next.configured = has_key;
    next
}

fn apply_glm_usage(snapshot: &ProviderSnapshot, usage: GlmUsage) -> ProviderSnapshot {
    let mut next = snapshot.clone();
    next.plan_name = usage.plan;
    next.status = "ready".into();
    next.fetch_mode = "api_key".into();
    next.limits = usage.limits;
    next.last_updated = now_iso();
    next.notes = "Quota loaded from the GLM/Z.AI Coding Plan API.".into();
    next.configured = true;
    next
}

fn apply_glm_error(
    snapshot: &ProviderSnapshot,
    kind: GlmErrorKind,
    message: String,
    has_key: bool,
) -> ProviderSnapshot {
    let mut next = snapshot.clone();
    next.status = if matches!(kind, GlmErrorKind::NeedsConfig | GlmErrorKind::Auth) {
        "needs_config".into()
    } else {
        "error".into()
    };
    next.last_updated = now_iso();
    next.notes = message;
    next.configured = has_key;
    next
}

fn mark_usage_for_notification(
    notified_usage: &mut HashMap<(i64, String), u32>,
    account_id: i64,
    limit_id: &str,
    highest: f64,
    threshold: i64,
) -> Option<u32> {
    let key = (account_id, limit_id.to_string());
    let percentage = highest.round().clamp(0.0, 100.0) as u32;
    let should_notify = highest >= threshold as f64
        && notified_usage
            .get(&key)
            .is_none_or(|previous| percentage > *previous);
    should_notify.then(|| {
        notified_usage.insert(key, percentage);
        percentage
    })
}

fn reset_quota_windows(
    previous: &ProviderSnapshot,
    current: &ProviderSnapshot,
) -> Vec<(String, String, u32)> {
    current
        .limits
        .iter()
        .filter_map(|limit| {
            let before = previous.limits.iter().find(|item| item.id == limit.id)?;
            if before.total <= 0.0 || limit.total <= 0.0 {
                return None;
            }
            let before_percentage = (before.used / before.total) * 100.0;
            let current_percentage = (limit.used / limit.total) * 100.0;
            (before_percentage > 10.0 && current_percentage < 5.0).then(|| {
                (
                    limit.id.clone(),
                    limit.label.clone(),
                    current_percentage.round().clamp(0.0, 100.0) as u32,
                )
            })
        })
        .collect()
}

fn notification_name(snapshot: &ProviderSnapshot) -> String {
    if snapshot.custom_name.trim().is_empty() {
        snapshot.provider_name.clone()
    } else {
        format!("{} - {}", snapshot.provider_name, snapshot.custom_name)
    }
}

fn provider_notification_icon(app: &AppHandle, provider: &str) -> Option<String> {
    let filename = match provider {
        "openai" => "openai.png",
        "claude" => "claude.png",
        "google" => "antigravity.png",
        "minimax" => "minimax.png",
        "glm" => "glm.png",
        _ => return None,
    };
    #[cfg(windows)]
    let staged = app
        .path()
        .app_data_dir()
        .ok()?
        .join("notification-icons")
        .join(filename);
    let bundled = app
        .path()
        .resource_dir()
        .ok()?
        .join("icons")
        .join("providers")
        .join(filename);
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("icons")
        .join("providers")
        .join(filename);
    #[cfg(windows)]
    if staged.is_file() {
        return Some(staged.to_string_lossy().into_owned());
    }
    bundled
        .is_file()
        .then_some(bundled)
        .or_else(|| source.is_file().then_some(source))
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn stage_windows_notification_icons(app: &AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("notification-icons");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let icons: [(&str, &[u8]); 6] = [
        ("app.png", include_bytes!("../icons/128x128.png")),
        (
            "openai.png",
            include_bytes!("../icons/providers/openai.png"),
        ),
        (
            "claude.png",
            include_bytes!("../icons/providers/claude.png"),
        ),
        (
            "antigravity.png",
            include_bytes!("../icons/providers/antigravity.png"),
        ),
        (
            "minimax.png",
            include_bytes!("../icons/providers/minimax.png"),
        ),
        ("glm.png", include_bytes!("../icons/providers/glm.png")),
    ];
    for (filename, bytes) in icons {
        std::fs::write(directory.join(filename), bytes).map_err(|error| error.to_string())?;
    }
    Ok(directory.join("app.png"))
}

#[cfg(windows)]
fn register_windows_notification_identity(app: &AppHandle) -> Result<(), String> {
    let icon = stage_windows_notification_icons(app)?;
    let key = CURRENT_USER
        .create(format!(
            r"SOFTWARE\Classes\AppUserModelId\{WINDOWS_NOTIFICATION_APP_ID}"
        ))
        .map_err(|error| error.to_string())?;
    key.set_string("DisplayName", "AI Bucket")
        .map_err(|error| error.to_string())?;
    key.set_string("IconBackgroundColor", "0")
        .map_err(|error| error.to_string())?;
    key.set_string("IconUri", icon.to_string_lossy())
        .map_err(|error| error.to_string())
}

fn show_provider_notification(
    app: &AppHandle,
    snapshot: &ProviderSnapshot,
    title: String,
    body: String,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        let mut toast = Toast::new(WINDOWS_NOTIFICATION_APP_ID)
            .title(&title)
            .text1(&body);
        if let Some(icon) = provider_notification_icon(app, &snapshot.provider) {
            toast = toast.icon(Path::new(&icon), IconCrop::Square, &snapshot.provider_name);
        }
        toast.show().map_err(|error| error.to_string())
    }

    #[cfg(not(windows))]
    {
        let mut builder = app.notification().builder().title(title).body(body);
        if let Some(icon) = provider_notification_icon(app, &snapshot.provider) {
            builder = builder.icon(icon);
        }
        builder.show().map_err(|error| error.to_string())
    }
}

fn maybe_notify(
    app: &AppHandle,
    settings: &AppSettings,
    config: &ProviderConfig,
    previous: &ProviderSnapshot,
    current: &ProviderSnapshot,
) -> Result<(), String> {
    let reset_windows = reset_quota_windows(previous, current);
    let app_state = app.state::<AppState>();
    let mut notified_usage = app_state
        .notified_usage
        .lock()
        .map_err(|_| "Failed to lock notification state".to_string())?;
    for (limit_id, label, percentage) in &reset_windows {
        notified_usage.remove(&(current.account_id, limit_id.clone()));
        if settings.notifications_enabled && config.reset_alert_enabled {
            show_provider_notification(
                app,
                current,
                notification_name(current),
                format!("Quota usage of '{label}' has been reset ({percentage}% used)."),
            )?;
        }
    }

    if !settings.notifications_enabled || !config.threshold_alert_enabled {
        return Ok(());
    }
    let Some(highest_limit) = current
        .limits
        .iter()
        .filter(|limit| limit.total > 0.0)
        .max_by(|left, right| {
            let left_percentage = left.used / left.total;
            let right_percentage = right.used / right.total;
            left_percentage.total_cmp(&right_percentage)
        })
    else {
        return Ok(());
    };
    let highest = (highest_limit.used / highest_limit.total) * 100.0;
    if let Some(percentage) = mark_usage_for_notification(
        &mut notified_usage,
        current.account_id,
        &highest_limit.id,
        highest,
        settings.notification_threshold,
    ) {
        show_provider_notification(
            app,
            current,
            notification_name(current),
            format!(
                "Quota usage of '{}' is above the threshold ({percentage}%).",
                highest_limit.label
            ),
        )?;
    }
    Ok(())
}

async fn import_and_store_claude_local_session(
    path: &PathBuf,
    credentials_path: &Path,
    account_id: i64,
) -> Result<ClaudeUsage, providers::claude::ClaudeError> {
    let (usage, credential) = providers::claude::collect_and_capture_local().await?;
    let encoded = providers::claude::encode_local_credential_cache(&credential)?;
    let conn = connect(path).map_err(|message| providers::claude::ClaudeError {
        kind: ClaudeErrorKind::InvalidResponse,
        message,
    })?;
    store::write_account_credential(&conn, credentials_path, account_id, &encoded).map_err(
        |message| providers::claude::ClaudeError {
            kind: ClaudeErrorKind::InvalidResponse,
            message,
        },
    )?;
    Ok(usage)
}

async fn collect_claude_local(
    path: &PathBuf,
    credentials_path: &Path,
    account_id: i64,
    stored_credential: &str,
) -> Result<(ClaudeUsage, bool), providers::claude::ClaudeError> {
    if let Ok(cached) = providers::claude::decode_local_credential_cache(stored_credential) {
        match providers::claude::collect_cached_local(&cached).await {
            Ok(usage) => return Ok((usage, true)),
            Err(error) if error.kind != ClaudeErrorKind::NeedsAuth => return Err(error),
            Err(_) => {}
        }
    }
    import_and_store_claude_local_session(path, credentials_path, account_id)
        .await
        .map(|usage| (usage, false))
}

async fn refresh_one(
    app: &AppHandle,
    path: &PathBuf,
    credentials_path: &Path,
    account_id: i64,
) -> Result<ProviderSnapshot, String> {
    let (current, settings, config) = {
        let conn = connect(path)?;
        (
            store::latest_snapshot(&conn, account_id)?,
            store::load_settings(&conn)?,
            store::config_for_account(&conn, credentials_path, account_id)?,
        )
    };
    let provider = config.provider.as_str();

    let next = if provider == "openai" {
        match providers::codex::collect(&config.local_config_path).await {
            Ok(usage) => apply_codex_usage(&current, usage),
            Err(error) => apply_codex_error(&current, error.kind, error.message),
        }
    } else if provider == "claude" {
        if config.auth_method == "oauth" {
            let lock = {
                let app_state = app.state::<AppState>();
                let mut locks = app_state
                    .claude_oauth_locks
                    .lock()
                    .map_err(|_| "Failed to lock Claude OAuth refresh state".to_string())?;
                locks
                    .entry(account_id)
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                    .clone()
            };
            let _guard = lock.lock().await;
            let result = async {
                let mut credential = providers::claude::decode_oauth_credential(&config.api_key)?;
                if providers::claude::credential_needs_refresh(&credential) {
                    credential = providers::claude::refresh_oauth_credential(&credential).await?;
                    let encoded = providers::claude::encode_oauth_credential(&credential)?;
                    let conn = connect(path).map_err(|message| providers::claude::ClaudeError {
                        kind: ClaudeErrorKind::InvalidResponse,
                        message,
                    })?;
                    store::write_account_credential(&conn, credentials_path, account_id, &encoded)
                        .map_err(|message| providers::claude::ClaudeError {
                            kind: ClaudeErrorKind::InvalidResponse,
                            message,
                        })?;
                }
                match providers::claude::collect_oauth(&credential).await {
                    Ok(usage) => Ok(usage),
                    Err(error) if error.kind == ClaudeErrorKind::NeedsAuth => {
                        let refreshed =
                            providers::claude::refresh_oauth_credential(&credential).await?;
                        let encoded = providers::claude::encode_oauth_credential(&refreshed)?;
                        let conn =
                            connect(path).map_err(|message| providers::claude::ClaudeError {
                                kind: ClaudeErrorKind::InvalidResponse,
                                message,
                            })?;
                        store::write_account_credential(
                            &conn,
                            credentials_path,
                            account_id,
                            &encoded,
                        )
                        .map_err(|message| {
                            providers::claude::ClaudeError {
                                kind: ClaudeErrorKind::InvalidResponse,
                                message,
                            }
                        })?;
                        providers::claude::collect_oauth(&refreshed).await
                    }
                    Err(error) => Err(error),
                }
            }
            .await;
            match result {
                Ok(usage) => apply_claude_usage(&current, usage, "oauth"),
                Err(error) => apply_claude_error(&current, error.kind, error.message),
            }
        } else {
            match collect_claude_local(path, credentials_path, account_id, &config.api_key).await {
                Ok((usage, used_cache)) => {
                    let mut next = apply_claude_usage(&current, usage, "local_credential");
                    next.notes = if used_cache {
                        "Quota loaded from the encrypted Claude Desktop session cache.".into()
                    } else {
                        "Claude Desktop session imported, verified, and encrypted for future refreshes."
                            .into()
                    };
                    next
                }
                Err(error) => apply_claude_error(&current, error.kind, error.message),
            }
        }
    } else if provider == "google" {
        match providers::antigravity::collect().await {
            Ok(usage) => apply_antigravity_usage(&current, usage),
            Err(error) => apply_antigravity_error(&current, error.kind, error.message),
        }
    } else if provider == "minimax" {
        match providers::minimax::collect(&config.api_key, &config.base_url).await {
            Ok(usage) => apply_minimax_usage(&current, usage),
            Err(error) => apply_minimax_error(
                &current,
                error.kind,
                error.message,
                !config.api_key.trim().is_empty(),
            ),
        }
    } else if provider == "glm" {
        match providers::glm::collect(&config.api_key, &config.base_url).await {
            Ok(usage) => apply_glm_usage(&current, usage),
            Err(error) => apply_glm_error(
                &current,
                error.kind,
                error.message,
                !config.api_key.trim().is_empty(),
            ),
        }
    } else {
        simulate_refresh(&current)
    };

    let conn = connect(path)?;
    store::insert_snapshot(&conn, &next)?;
    maybe_notify(app, &settings, &config, &current, &next)?;
    Ok(next)
}

#[tauri::command]
fn get_dashboard_state(state: State<AppState>) -> Result<DashboardState, String> {
    let path = db_path(&state)?;
    let credentials_path = credential_dir(&state)?;
    let conn = connect(&path)?;
    store::load_dashboard(&conn, &credentials_path)
}

#[tauri::command]
async fn refresh_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: i64,
) -> Result<DashboardState, String> {
    let path = db_path(&state)?;
    let credentials_path = credential_dir(&state)?;
    refresh_one(&app, &path, &credentials_path, account_id).await?;
    let conn = connect(&path)?;
    store::load_dashboard(&conn, &credentials_path)
}

#[tauri::command]
async fn refresh_all_providers(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<DashboardState, String> {
    let path = db_path(&state)?;
    let credentials_path = credential_dir(&state)?;
    let account_ids = {
        let conn = connect(&path)?;
        store::load_configs(&conn, &credentials_path)?
            .into_iter()
            .filter(|config| config.enabled)
            .map(|config| config.account_id)
            .collect::<Vec<_>>()
    };
    for account_id in account_ids {
        refresh_one(&app, &path, &credentials_path, account_id).await?;
    }
    let conn = connect(&path)?;
    store::load_dashboard(&conn, &credentials_path)
}

#[tauri::command]
fn save_provider_config(
    state: State<AppState>,
    mut config: ProviderConfig,
) -> Result<DashboardState, String> {
    if !PROVIDER_IDS.contains(&config.provider.as_str()) {
        return Err("Provider not found".into());
    }
    if config.custom_name.chars().count() > 80 {
        return Err("Custom account name must be 80 characters or fewer".into());
    }
    let supported_auth = match config.provider.as_str() {
        "openai" | "google" => config.auth_method == "local_credential",
        "claude" => matches!(config.auth_method.as_str(), "local_credential" | "oauth"),
        "minimax" | "glm" => config.auth_method == "api_key",
        _ => false,
    };
    if !supported_auth {
        return Err("This authentication method is not available for the provider yet".into());
    }
    if config.provider == "openai" && config.auth_method == "local_credential" {
        config.local_config_path = config.local_config_path.trim().to_string();
        providers::codex::validate_home(&config.local_config_path)
            .map_err(|error| error.message)?;
    } else {
        config.local_config_path.clear();
    }
    let path = db_path(&state)?;
    let credentials_path = credential_dir(&state)?;
    let conn = connect(&path)?;
    let account_id = store::save_config(&conn, &credentials_path, &config)?;
    let has_snapshot: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM provider_snapshots_v2 WHERE account_id = ?1)",
            [account_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !has_snapshot {
        store::insert_snapshot(
            &conn,
            &store::new_account_snapshot(
                account_id,
                &config.provider,
                &config.custom_name,
                &now_iso(),
            ),
        )?;
    }
    let mut snapshot = store::latest_snapshot(&conn, account_id)?;
    if config.provider != "openai" && config.provider != "claude" && config.provider != "google" {
        snapshot.configured = !config.api_key.trim().is_empty();
        snapshot.status = if snapshot.configured {
            "placeholder".into()
        } else {
            "needs_config".into()
        };
        snapshot.last_updated = now_iso();
        store::insert_snapshot(&conn, &snapshot)?;
    }
    store::load_dashboard(&conn, &credentials_path)
}

#[tauri::command]
fn begin_claude_oauth(
    state: State<AppState>,
    account_id: i64,
) -> Result<providers::claude::ClaudeOAuthStart, String> {
    let path = db_path(&state)?;
    let credentials_path = credential_dir(&state)?;
    let conn = connect(&path)?;
    let config = store::config_for_account(&conn, &credentials_path, account_id)?;
    if config.provider != "claude" || config.auth_method != "oauth" {
        return Err("Select Claude OAuth for this account before signing in.".into());
    }
    let (pending, start) = providers::claude::begin_oauth().map_err(|error| error.message)?;
    state
        .claude_oauth_sessions
        .lock()
        .map_err(|_| "Failed to lock Claude OAuth state".to_string())?
        .insert(account_id, pending);
    Ok(start)
}

#[tauri::command]
async fn complete_claude_oauth(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: i64,
    authorization_code: String,
) -> Result<DashboardState, String> {
    let pending = state
        .claude_oauth_sessions
        .lock()
        .map_err(|_| "Failed to lock Claude OAuth state".to_string())?
        .get(&account_id)
        .cloned()
        .ok_or_else(|| {
            "Start Claude sign-in again before entering the authorization code.".to_string()
        })?;
    let credential = providers::claude::exchange_oauth_code(&pending, &authorization_code)
        .await
        .map_err(|error| error.message)?;
    let path = db_path(&state)?;
    let credentials_path = credential_dir(&state)?;
    let conn = connect(&path)?;
    let config = store::config_for_account(&conn, &credentials_path, account_id)?;
    if config.provider != "claude" || config.auth_method != "oauth" {
        return Err("This account is no longer configured for Claude OAuth.".into());
    }
    if let Some(account_uuid) = credential.account_uuid.as_deref() {
        for other in store::load_configs(&conn, &credentials_path)? {
            if other.account_id == account_id
                || other.provider != "claude"
                || other.auth_method != "oauth"
            {
                continue;
            }
            let stored = store::config_for_account(&conn, &credentials_path, other.account_id)?;
            if providers::claude::decode_oauth_credential(&stored.api_key)
                .ok()
                .and_then(|item| item.account_uuid)
                .as_deref()
                == Some(account_uuid)
            {
                return Err(format!(
                    "This Claude account is already connected to '{}'.",
                    other.custom_name
                ));
            }
        }
    }
    let encoded =
        providers::claude::encode_oauth_credential(&credential).map_err(|error| error.message)?;
    store::write_account_credential(&conn, &credentials_path, account_id, &encoded)?;
    drop(conn);
    state
        .claude_oauth_sessions
        .lock()
        .map_err(|_| "Failed to lock Claude OAuth state".to_string())?
        .remove(&account_id);
    refresh_one(&app, &path, &credentials_path, account_id).await?;
    let conn = connect(&path)?;
    store::load_dashboard(&conn, &credentials_path)
}

#[tauri::command]
fn reorder_provider_accounts(
    state: State<AppState>,
    account_ids: Vec<i64>,
) -> Result<DashboardState, String> {
    let path = db_path(&state)?;
    let credentials_path = credential_dir(&state)?;
    let conn = connect(&path)?;
    store::reorder_accounts(&conn, &account_ids)?;
    store::load_dashboard(&conn, &credentials_path)
}

#[tauri::command]
fn delete_provider_account(
    state: State<AppState>,
    account_id: i64,
) -> Result<DashboardState, String> {
    let path = db_path(&state)?;
    let credentials_path = credential_dir(&state)?;
    let conn = connect(&path)?;
    store::delete_account(&conn, &credentials_path, account_id)?;
    store::load_dashboard(&conn, &credentials_path)
}

#[tauri::command]
fn test_provider_alert(
    app: AppHandle,
    state: State<AppState>,
    account_id: i64,
    alert_kind: String,
) -> Result<(), String> {
    let path = db_path(&state)?;
    let conn = connect(&path)?;
    let snapshot = store::latest_snapshot(&conn, account_id)?;
    let name = notification_name(&snapshot);
    match alert_kind.as_str() {
        "threshold" => {
            let (timeframe, percentage) = snapshot
                .limits
                .iter()
                .filter(|limit| limit.total > 0.0)
                .max_by(|left, right| {
                    (left.used / left.total).total_cmp(&(right.used / right.total))
                })
                .map(|limit| {
                    (
                        limit.label.as_str(),
                        ((limit.used / limit.total) * 100.0).round() as u32,
                    )
                })
                .unwrap_or(("quota window", 80));
            show_provider_notification(
                &app,
                &snapshot,
                format!("{name} - TEST"),
                format!("Quota usage of '{timeframe}' is above the threshold ({percentage}%)."),
            )
        }
        "reset" => {
            let timeframe = snapshot
                .limits
                .first()
                .map(|limit| limit.label.as_str())
                .unwrap_or("session (5h)");
            show_provider_notification(
                &app,
                &snapshot,
                format!("{name} - TEST"),
                format!("Quota usage of '{timeframe}' has been reset."),
            )
        }
        _ => Err("Unknown alert type".into()),
    }
}

#[tauri::command]
fn save_app_settings(
    app: AppHandle,
    state: State<AppState>,
    patch: SettingsPatch,
) -> Result<DashboardState, String> {
    let path = db_path(&state)?;
    let credentials_path = credential_dir(&state)?;
    let conn = connect(&path)?;
    let current = store::load_settings(&conn)?;
    let next = AppSettings {
        auto_refresh_enabled: patch
            .auto_refresh_enabled
            .unwrap_or(current.auto_refresh_enabled),
        refresh_interval_minutes: patch
            .refresh_interval_minutes
            .filter(|value| (2..=60).contains(value) && value % 2 == 0)
            .unwrap_or(current.refresh_interval_minutes),
        notification_threshold: patch
            .notification_threshold
            .unwrap_or(current.notification_threshold),
        notifications_enabled: patch
            .notifications_enabled
            .unwrap_or(current.notifications_enabled),
        color_theme: patch
            .color_theme
            .filter(|value| matches!(value.as_str(), "system" | "dark" | "light"))
            .unwrap_or(current.color_theme),
        size_theme: patch
            .size_theme
            .filter(|value| {
                matches!(
                    value.as_str(),
                    "very_compact" | "compact" | "normal" | "large"
                )
            })
            .unwrap_or(current.size_theme),
        antigravity_two_column_quota: patch
            .antigravity_two_column_quota
            .unwrap_or(current.antigravity_two_column_quota),
        window_mode: patch
            .window_mode
            .filter(|value| matches!(value.as_str(), "normal" | "widget"))
            .unwrap_or(current.window_mode),
        widget_opacity: patch
            .widget_opacity
            .map(|value| value.clamp(40, 100))
            .unwrap_or(current.widget_opacity),
        foreground_opacity_boost: patch
            .foreground_opacity_boost
            .map(|value| value.clamp(0, 30))
            .unwrap_or(current.foreground_opacity_boost),
        widget_always_on_top: patch
            .widget_always_on_top
            .unwrap_or(current.widget_always_on_top),
        minimize_to_tray: patch.minimize_to_tray.unwrap_or(current.minimize_to_tray),
    };
    store::save_settings(&conn, &next)?;
    state
        .minimize_to_tray
        .store(next.minimize_to_tray, Ordering::Relaxed);
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        tray.set_visible(next.minimize_to_tray)
            .map_err(|error| error.to_string())?;
    }
    if let Some(window) = app.get_webview_window("main") {
        let previous_mode = state
            .window_mode
            .lock()
            .map_err(|_| "Failed to lock window mode".to_string())?
            .clone();
        if previous_mode != next.window_mode {
            save_mode_layout(&window, &state.layouts_path, &previous_mode)?;
            apply_window_mode(
                &window,
                &state.layouts_path,
                &next.window_mode,
                next.widget_always_on_top,
            )?;
            *state
                .window_mode
                .lock()
                .map_err(|_| "Failed to lock window mode".to_string())? = next.window_mode.clone();
        } else {
            window
                .set_always_on_top(next.window_mode == "widget" && next.widget_always_on_top)
                .map_err(|error| error.to_string())?;
        }
    }
    store::load_dashboard(&conn, &credentials_path)
}

#[tauri::command]
fn start_window_drag(app: AppHandle) -> Result<(), String> {
    app.get_webview_window("main")
        .ok_or_else(|| "Main window is unavailable".to_string())?
        .start_dragging()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn minimize_window(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window is unavailable".to_string())?;
    persist_current_window(&app, &state)?;
    if state.minimize_to_tray.load(Ordering::Relaxed) {
        window.hide().map_err(|error| error.to_string())
    } else {
        window.minimize().map_err(|error| error.to_string())
    }
}

#[tauri::command]
fn shutdown_app(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    persist_current_window(&app, &state)?;
    app.exit(0);
    Ok(())
}

#[cfg(test)]
fn layered_opacity(surface: i64, boost: i64) -> (i64, i64) {
    let surface = surface.clamp(40, 100);
    (surface, (surface + boost.clamp(0, 30)).min(100))
}

pub fn run() {
    let window_state_flags = window_state_flags();
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(window_state_flags)
                .build(),
        )
        .setup(|app| {
            #[cfg(windows)]
            register_windows_notification_identity(app.handle())?;

            let app_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| error.to_string())?;
            std::fs::create_dir_all(&app_dir).map_err(|error| error.to_string())?;
            let db_path = app_dir.join("ai-bucket.sqlite");
            let credential_dir = app_dir.join("credentials");
            let detected_local_providers = if db_path.exists() {
                Vec::new()
            } else {
                [
                    ("openai", providers::codex::has_local_config()),
                    ("claude", providers::claude::has_local_config()),
                    ("google", providers::antigravity::has_local_config()),
                ]
                .into_iter()
                .filter(|(_, detected)| *detected)
                .map(|(provider, _)| provider.to_string())
                .collect()
            };
            let conn = Connection::open(&db_path).map_err(|error| error.to_string())?;
            store::init_db(
                &conn,
                &credential_dir,
                &now_iso(),
                &detected_local_providers,
            )?;
            let settings = store::load_settings(&conn)?;
            let layouts_path = app_dir.join("window-layouts.json");
            let window_mode = Arc::new(Mutex::new(settings.window_mode.clone()));
            let minimize_to_tray = Arc::new(AtomicBool::new(settings.minimize_to_tray));
            app.manage(AppState {
                db_path: Mutex::new(db_path),
                credential_dir: Mutex::new(credential_dir),
                notified_usage: Mutex::new(HashMap::new()),
                layouts_path: layouts_path.clone(),
                window_mode: window_mode.clone(),
                minimize_to_tray: minimize_to_tray.clone(),
                claude_oauth_sessions: Mutex::new(HashMap::new()),
                claude_oauth_locks: Mutex::new(HashMap::new()),
            });

            let window_icon =
                tauri::image::Image::from_bytes(include_bytes!("../icons/128x128@2x.png"))?;
            let show_item = MenuItem::with_id(app, "show", "Show AI Bucket", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit AI Bucket", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_item, &quit_item])?;
            let tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(window_icon.clone())
                .tooltip("AI Bucket")
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        let _ = show_main_window(app);
                    }
                    "quit" => {
                        let state = app.state::<AppState>();
                        let _ = persist_current_window(app, &state);
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    ) {
                        let _ = toggle_main_window(tray.app_handle());
                    }
                })
                .build(app)?;
            tray.set_visible(settings.minimize_to_tray)?;
            if let Some(window) = app.get_webview_window("main") {
                window.set_icon(window_icon)?;
                apply_window_mode(
                    &window,
                    &layouts_path,
                    &settings.window_mode,
                    settings.widget_always_on_top,
                )?;

                let save_window_state = start_window_state_saver(
                    app.handle().clone(),
                    window.clone(),
                    layouts_path,
                    window_mode,
                );
                let save_after_change = save_window_state.clone();
                let window_for_events = window.clone();
                let minimize_for_events = minimize_to_tray;
                window.on_window_event(move |event| {
                    let minimized = window_for_events.is_minimized().unwrap_or(false);
                    if minimized {
                        if minimize_for_events.load(Ordering::Relaxed) {
                            let _ = window_for_events.hide();
                        }
                        return;
                    }
                    if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_)) {
                        let _ = save_after_change.try_send(());
                    }
                });

                let size = window.inner_size()?;
                let monitor = window.current_monitor()?.or(app.primary_monitor()?);
                let exceeds_monitor = monitor.is_some_and(|monitor| {
                    let monitor_size = monitor.size();
                    window_exceeds_monitor(
                        size.width,
                        size.height,
                        monitor_size.width,
                        monitor_size.height,
                    )
                });
                if settings.window_mode == "normal" && exceeds_monitor {
                    window.maximize()?;
                    let _ = save_window_state.try_send(());
                }
                window.show()?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_dashboard_state,
            refresh_provider,
            refresh_all_providers,
            save_provider_config,
            begin_claude_oauth,
            complete_claude_oauth,
            reorder_provider_accounts,
            delete_provider_account,
            test_provider_alert,
            save_app_settings,
            start_window_drag,
            minimize_window,
            shutdown_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod live_tests {
    use super::*;

    #[test]
    fn layered_opacity_clamps_surface_boost_and_content() {
        assert_eq!(layered_opacity(40, 0), (40, 40));
        assert_eq!(layered_opacity(50, 15), (50, 65));
        assert_eq!(layered_opacity(90, 30), (90, 100));
        assert_eq!(layered_opacity(100, 30), (100, 100));
        assert_eq!(layered_opacity(20, -10), (40, 40));
    }

    #[test]
    fn codex_usage_does_not_regress_inside_the_same_reset_window() {
        let reset = (Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
        let current = vec![models::LimitMetric {
            id: "session".into(),
            label: "session (5h)".into(),
            resource: None,
            used: 68.0,
            total: 100.0,
            reset_at: Some(reset.clone()),
            window_seconds: Some(18_000),
        }];
        let incoming = vec![models::LimitMetric {
            id: "session".into(),
            label: "session (5h)".into(),
            resource: None,
            used: 9.0,
            total: 100.0,
            reset_at: Some(
                (DateTime::parse_from_rfc3339(&reset).unwrap() + chrono::Duration::minutes(5))
                    .to_rfc3339(),
            ),
            window_seconds: Some(18_000),
        }];
        assert_eq!(stabilize_codex_limits(&current, incoming)[0].used, 68.0);
    }

    #[test]
    fn quota_notification_only_advances_with_higher_usage_in_a_session() {
        let mut notified = HashMap::new();
        assert_eq!(
            mark_usage_for_notification(&mut notified, 7, "weekly", 80.0, 80),
            Some(80)
        );
        assert_eq!(
            mark_usage_for_notification(&mut notified, 7, "weekly", 80.0, 80),
            None
        );
        assert_eq!(
            mark_usage_for_notification(&mut notified, 7, "weekly", 79.0, 80),
            None
        );
        assert_eq!(
            mark_usage_for_notification(&mut notified, 7, "weekly", 90.0, 80),
            Some(90)
        );
        assert_eq!(
            mark_usage_for_notification(&mut notified, 7, "weekly", 90.0, 80),
            None
        );
        assert_eq!(
            mark_usage_for_notification(&mut notified, 7, "weekly", 100.0, 80),
            Some(100)
        );
        assert_eq!(
            mark_usage_for_notification(&mut notified, 8, "weekly", 80.0, 80),
            Some(80)
        );
        assert_eq!(
            mark_usage_for_notification(&mut notified, 7, "session", 80.0, 80),
            Some(80)
        );
    }

    #[test]
    fn quota_reset_detection_is_scoped_to_each_timeframe() {
        let limit = |id: &str, label: &str, used: f64| models::LimitMetric {
            id: id.into(),
            label: label.into(),
            resource: None,
            used,
            total: 100.0,
            reset_at: None,
            window_seconds: None,
        };
        let mut previous = store::new_account_snapshot(4, "openai", "Local", "before");
        previous.limits = vec![
            limit("session", "session (5h)", 45.0),
            limit("weekly", "weekly (7d)", 70.0),
        ];
        let mut current = previous.clone();
        current.limits = vec![
            limit("session", "session (5h)", 3.0),
            limit("weekly", "weekly (7d)", 68.0),
        ];

        let resets = reset_quota_windows(&previous, &current);
        assert_eq!(resets, vec![("session".into(), "session (5h)".into(), 3)]);
    }

    #[test]
    fn oversized_window_is_maximized_for_the_current_monitor() {
        assert!(window_exceeds_monitor(2560, 1200, 1920, 1080));
        assert!(window_exceeds_monitor(1600, 1400, 1920, 1080));
        assert!(!window_exceeds_monitor(1600, 900, 1920, 1080));
        assert!(!window_exceeds_monitor(1920, 1080, 1920, 1080));
    }

    #[test]
    #[ignore = "reads the locally saved MiniMax key and calls the live quota endpoint"]
    fn live_minimax_collector_returns_quota_windows() {
        let app_data = std::env::var_os("APPDATA").expect("APPDATA is available");
        let database = PathBuf::from(&app_data)
            .join("com.local.ai-bucket")
            .join("ai-bucket.sqlite");
        let conn = Connection::open(database).expect("AI Bucket database is available");
        let account_id: i64 = conn
            .query_row(
                "SELECT id FROM provider_accounts WHERE provider = 'minimax' ORDER BY id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("MiniMax account exists");
        let credential_dir = PathBuf::from(app_data)
            .join("com.local.ai-bucket")
            .join("credentials");
        let config = store::config_for_account(&conn, &credential_dir, account_id)
            .expect("MiniMax config exists");
        assert!(
            !config.api_key.trim().is_empty(),
            "MiniMax key is configured"
        );

        let usage = tauri::async_runtime::block_on(providers::minimax::collect(
            &config.api_key,
            &config.base_url,
        ))
        .expect("live MiniMax quota request");
        assert!(!usage.limits.is_empty());
        assert!(usage.limits.iter().all(|limit| limit.total > 0.0));
    }

    #[test]
    #[ignore = "reads the locally saved GLM key and calls the live quota endpoint"]
    fn live_glm_collector_returns_quota_windows() {
        let app_data = std::env::var_os("APPDATA").expect("APPDATA is available");
        let database = PathBuf::from(&app_data)
            .join("com.local.ai-bucket")
            .join("ai-bucket.sqlite");
        let conn = Connection::open(database).expect("AI Bucket database is available");
        let account_id: i64 = conn
            .query_row(
                "SELECT id FROM provider_accounts WHERE provider = 'glm' ORDER BY id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("GLM account exists");
        let credential_dir = PathBuf::from(app_data)
            .join("com.local.ai-bucket")
            .join("credentials");
        let config = store::config_for_account(&conn, &credential_dir, account_id)
            .expect("GLM config exists");
        assert!(!config.api_key.trim().is_empty(), "GLM key is configured");

        let usage = tauri::async_runtime::block_on(providers::glm::collect(
            &config.api_key,
            &config.base_url,
        ))
        .expect("live GLM quota request");
        assert!(!usage.limits.is_empty());
        assert!(usage.limits.iter().all(|limit| limit.total > 0.0));
    }

    #[test]
    #[ignore = "reads the local Antigravity OAuth session and calls the live quota endpoint"]
    fn live_antigravity_collector_returns_quota_buckets() {
        let usage = tauri::async_runtime::block_on(providers::antigravity::collect())
            .expect("live Antigravity quota request");
        assert!(!usage.limits.is_empty());
        assert!(usage.limits.iter().all(|limit| limit.total == 100.0));
    }
}

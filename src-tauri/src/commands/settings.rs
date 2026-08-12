use std::sync::{Arc, Mutex};
use tauri::State;
use sqlx::{SqlitePool, Row};
use serde::{Deserialize, Serialize};
use crate::error::AppResult;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub enum WorkMode {
    #[default]
    Light,
    Study,
    Focus,
}

impl WorkMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkMode::Light => "Light",
            WorkMode::Study => "Study",
            WorkMode::Focus => "Focus",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "Study" => WorkMode::Study,
            "Focus" => WorkMode::Focus,
            _ => WorkMode::Light,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppSettings {
    pub ai_provider: String,   // "none" | "local" | "openai" | "anthropic"
    pub openai_key: String,
    pub openai_model: String,
    pub anthropic_key: String,
    pub anthropic_model: String,
    pub idle_threshold_secs: u64,  // the idle threshold; applied after a restart
    pub log_interval_secs: u64,    // the tick interval of the activity loop
    pub work_mode: WorkMode,   // Light | Study | Focus
    pub onboarding_complete: bool,
    pub deadline_warn_hours: u64,    // how many hours before the deadline the first notification fires
    pub deadline_warn_minutes: u64,  // how many minutes before the deadline the second notification fires
    pub idle_notify_min_mins: u64,   // the minimum idle time in minutes for notify_return
    pub pomodoro_work_mins: u64,     // the length of a pomodoro work block
    pub pomodoro_break_mins: u64,    // the length of a pomodoro break
    pub nudge_after_mins: u64,       // a break reminder after N minutes of continuous work (0 = off)
    #[serde(default)]
    pub theme_mode: String,          // "light" | "dark" | "system"
    #[serde(default)]
    pub color_accent: String,        // colour overrides; empty means the CSS default
    #[serde(default)]
    pub color_accent_secondary: String, // the second accent (the .btn-primary gradient); empty means equal to color_accent
    #[serde(default)]
    pub color_bg: String,
    #[serde(default)]
    pub color_bg_secondary: String, // the sidebar and second-plane surfaces (--bg-secondary)
    #[serde(default)]
    pub color_bg_hover: String,     // the hover fill of rows and ghost buttons (--bg-hover)
    #[serde(default)]
    pub color_bg_card: String,      // cards, buttons, task rows (--bg-card)
    #[serde(default)]
    pub color_text_secondary: String, // captions, dates, counters (--text-secondary)
    #[serde(default)]
    pub color_text: String,
    #[serde(default)]
    pub color_border: String,
    #[serde(default)]
    pub quiet_until: String,         // notification pause: RFC3339; empty means off; QUIET_FOREVER means indefinite
    #[serde(default = "default_true")]
    pub context_notifications: bool, // contextual triggers (overdue items, returning from InProgress, skipped days)
    #[serde(default)]
    pub ai_fallback: bool,           // automatic AI provider switching on error or unavailability
    #[serde(default)]
    pub openai_in_keyring: bool,     // runtime-only: the key lives in the keyring
    #[serde(default)]
    pub anthropic_in_keyring: bool,  // runtime-only: the key lives in the keyring
    #[serde(default)]
    pub custom_theme_presets: String, // JSON [{name, colors:{color_*}}] — user-saved colour sets
    #[serde(default)]
    pub app_category_rules: String,  // JSON [{pattern, category}] — window classes to categories
    #[serde(default)]
    pub app_limits: String,          // JSON [{category, daily_mins}] — 0 or absence means no limit
    #[serde(default)]
    pub auto_backup_dir: String,     // empty means automatic backup is off
    #[serde(default = "default_seven")]
    pub auto_backup_keep: u64,       // how many copies to keep
    // Backend-owned, read-only for the frontend: written by the backup loop, never
    // by save_settings. Sending it back from the form would let a settings page
    // that was opened before a backup ran reset the timestamp on save.
    #[serde(default)]
    pub last_auto_backup: String,    // RFC3339 of the last successful automatic backup
    #[serde(default)]
    pub last_auto_backup_error: String, // "<rfc3339>\t<message>", empty once a run succeeds
    #[serde(default)]
    pub morning_digest_time: String, // "HH:MM", empty means off
    #[serde(default = "default_true")]
    pub show_subtasks_expanded: bool, // subtasks visible in the list without a click
    #[serde(default)]
    pub keybinds: String,             // JSON {action_id: combo}; a missing key means the action's default
    // The same for GLOBAL quick-capture hotkeys. Kept under a separate key
    // rather than alongside keybinds: they use a different mechanism (OS-level
    // registration versus a webview handler) and carry a different cost of
    // failure — a global combination may turn out to be taken by the system,
    // a local one cannot.
    #[serde(default)]
    pub global_keybinds: String,
    #[serde(default = "default_true")]
    pub focus_mode_auto: bool,        // automatically pause notifications during pomodoro work or a time block
    // Breaking browser time down by site. OFF by default: it requires parsing
    // window titles, which is a privacy matter, so it is only enabled
    // explicitly. The title itself never reaches the DB under any setting —
    // only the domain extracted from it is stored (see monitor/domain.rs).
    pub track_domains: bool,
    // Interface language ("ru" | "en"). An empty string means no explicit
    // choice and the frontend falls back to the system locale. The default is
    // empty rather than "ru", or a non-Russian user would get Russian on first
    // launch.
    pub language: String,
    #[serde(default)]
    pub history_cleanup_months: u64,  // completed items older than N months go to the Trash automatically; 0 = off
}

fn default_seven() -> u64 { 7 }

fn default_true() -> bool { true }

// The sentinel for an indefinite notification pause.
pub const QUIET_FOREVER: &str = "9999-12-31T00:00:00+00:00";

// Shape-only check for settings values the frontend owns the schema of. The
// backend deliberately does not model saved colour sets: it never reads their
// contents, and duplicating the schema here would mean two places to update
// whenever a colour key is added. Rejecting anything that is not a JSON array
// is enough to keep a malformed value out of the DB.
fn is_json_array(s: &str) -> bool {
    matches!(serde_json::from_str::<serde_json::Value>(s), Ok(serde_json::Value::Array(_)))
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            ai_provider: "local".into(),
            openai_key: String::new(),
            openai_model: "gpt-4o-mini".into(),
            anthropic_key: String::new(),
            anthropic_model: "claude-haiku-4-5-20251001".into(),
            idle_threshold_secs: 300,
            log_interval_secs: 60,
            work_mode: WorkMode::Light,
            onboarding_complete: false,
            deadline_warn_hours: 24,
            deadline_warn_minutes: 60,
            idle_notify_min_mins: 10,
            pomodoro_work_mins: 25,
            pomodoro_break_mins: 5,
            nudge_after_mins: 90,
            theme_mode: "system".into(),
            color_accent: String::new(),
            color_accent_secondary: String::new(),
            color_bg: String::new(),
            color_bg_secondary: String::new(),
            color_bg_hover: String::new(),
            color_bg_card: String::new(),
            color_text_secondary: String::new(),
            color_text: String::new(),
            color_border: String::new(),
            quiet_until: String::new(),
            context_notifications: true,
            ai_fallback: false,
            openai_in_keyring: false,
            anthropic_in_keyring: false,
            custom_theme_presets: String::new(),
            app_category_rules: String::new(),
            app_limits: String::new(),
            auto_backup_dir: String::new(),
            auto_backup_keep: 7,
            last_auto_backup: String::new(),
            last_auto_backup_error: String::new(),
            morning_digest_time: String::new(),
            show_subtasks_expanded: true,
            keybinds: String::new(),
            global_keybinds: String::new(),
            focus_mode_auto: true,
            track_domains: false,
            language: String::new(),
            history_cleanup_months: 0,
        }
    }
}

// API keys live in the system keyring (Secret Service / Windows Credential
// Manager) rather than in SQLite as plain text. If the keyring is unavailable
// (no daemon), we fall back to the settings table.
fn keyring_set(name: &str, value: &str) -> Result<(), keyring::Error> {
    let entry = keyring::Entry::new("kriptag", name)?;
    if value.is_empty() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e),
        }
    } else {
        entry.set_password(value)
    }
}

fn keyring_get(name: &str) -> Option<String> {
    keyring::Entry::new("kriptag", name).ok()?.get_password().ok()
}

pub(crate) async fn get_setting(pool: &SqlitePool, key: &str) -> Option<String> {
    sqlx::query("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .map(|r| r.get("value"))
}

// The single place a numeric setting is read from, used by the background loops
// (scheduler / pomodoro / activity) so copies of the same query do not multiply.
// A missing key or junk in the value yields the default.
pub async fn get_u64_setting(pool: &SqlitePool, key: &str, default: u64) -> u64 {
    get_setting(pool, key).await.and_then(|v| v.parse().ok()).unwrap_or(default)
}

// The single place a boolean setting is read from (same pattern as get_u64_setting).
pub async fn get_bool_setting(pool: &SqlitePool, key: &str, default: bool) -> bool {
    get_setting(pool, key).await.map(|v| v != "false").unwrap_or(default)
}

pub(crate) async fn set_setting(pool: &SqlitePool, key: &str, value: &str) -> AppResult<()> {
    sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
        .bind(key)
        .bind(value)
        .execute(pool)
        .await?;
    Ok(())
}

// For switching the mode from the tray: writes to the DB bypassing the full save_settings
pub async fn persist_work_mode(pool: &SqlitePool, mode: &WorkMode) -> AppResult<()> {
    set_setting(pool, "work_mode", mode.as_str()).await
}

// For pausing notifications from the tray: an empty string means the pause is lifted.
pub async fn persist_quiet_until(pool: &SqlitePool, value: &str) -> AppResult<()> {
    set_setting(pool, "quiet_until", value).await
}

// Which pause preset is selected (the tray item id: quiet_30/quiet_60/...), so
// the tray's timed-pause checkmark can be restored after a restart.
pub async fn persist_quiet_preset(pool: &SqlitePool, id: &str) -> AppResult<()> {
    set_setting(pool, "quiet_preset", id).await
}

pub async fn load_settings_raw(pool: &SqlitePool) -> AppResult<AppSettings> {
    let mut s = AppSettings::default();
    if let Some(v) = get_setting(pool, "ai_provider").await { s.ai_provider = v; }
    if let Some(v) = get_setting(pool, "openai_model").await { s.openai_model = v; }
    if let Some(v) = get_setting(pool, "anthropic_model").await { s.anthropic_model = v; }
    if let Some(v) = get_setting(pool, "idle_threshold_secs").await {
        if let Ok(n) = v.parse() { s.idle_threshold_secs = n; }
    }
    if let Some(v) = get_setting(pool, "log_interval_secs").await {
        if let Ok(n) = v.parse() { s.log_interval_secs = n; }
    }
    if let Some(v) = get_setting(pool, "work_mode").await {
        s.work_mode = WorkMode::from_str(&v);
    }
    if let Some(v) = get_setting(pool, "onboarding_complete").await {
        s.onboarding_complete = v == "true";
    }
    if let Some(v) = get_setting(pool, "deadline_warn_hours").await { if let Ok(n) = v.parse() { s.deadline_warn_hours = n; } }
    if let Some(v) = get_setting(pool, "deadline_warn_minutes").await { if let Ok(n) = v.parse() { s.deadline_warn_minutes = n; } }
    if let Some(v) = get_setting(pool, "idle_notify_min_mins").await { if let Ok(n) = v.parse() { s.idle_notify_min_mins = n; } }
    if let Some(v) = get_setting(pool, "pomodoro_work_mins").await { if let Ok(n) = v.parse() { s.pomodoro_work_mins = n; } }
    if let Some(v) = get_setting(pool, "pomodoro_break_mins").await { if let Ok(n) = v.parse() { s.pomodoro_break_mins = n; } }
    if let Some(v) = get_setting(pool, "nudge_after_mins").await { if let Ok(n) = v.parse() { s.nudge_after_mins = n; } }
    if let Some(v) = get_setting(pool, "theme_mode").await { s.theme_mode = v; }
    if let Some(v) = get_setting(pool, "color_accent").await { s.color_accent = v; }
    if let Some(v) = get_setting(pool, "color_accent_secondary").await { s.color_accent_secondary = v; }
    if let Some(v) = get_setting(pool, "color_bg").await { s.color_bg = v; }
    if let Some(v) = get_setting(pool, "color_bg_secondary").await { s.color_bg_secondary = v; }
    if let Some(v) = get_setting(pool, "color_bg_hover").await { s.color_bg_hover = v; }
    if let Some(v) = get_setting(pool, "color_bg_card").await { s.color_bg_card = v; }
    if let Some(v) = get_setting(pool, "color_text_secondary").await { s.color_text_secondary = v; }
    if let Some(v) = get_setting(pool, "color_text").await { s.color_text = v; }
    if let Some(v) = get_setting(pool, "color_border").await { s.color_border = v; }
    if let Some(v) = get_setting(pool, "quiet_until").await { s.quiet_until = v; }
    if let Some(v) = get_setting(pool, "context_notifications").await { s.context_notifications = v != "false"; }
    if let Some(v) = get_setting(pool, "ai_fallback").await { s.ai_fallback = v == "true"; }
    if let Some(v) = get_setting(pool, "custom_theme_presets").await { s.custom_theme_presets = v; }
    if let Some(v) = get_setting(pool, "app_category_rules").await { s.app_category_rules = v; }
    if let Some(v) = get_setting(pool, "app_limits").await { s.app_limits = v; }
    if let Some(v) = get_setting(pool, "auto_backup_dir").await { s.auto_backup_dir = v; }
    // Read-only on the way out; see the field comment on AppSettings.
    if let Some(v) = get_setting(pool, "last_auto_backup").await { s.last_auto_backup = v; }
    if let Some(v) = get_setting(pool, "last_auto_backup_error").await { s.last_auto_backup_error = v; }
    if let Some(v) = get_setting(pool, "auto_backup_keep").await {
        if let Ok(n) = v.parse() { s.auto_backup_keep = n; }
    }
    if let Some(v) = get_setting(pool, "morning_digest_time").await { s.morning_digest_time = v; }
    if let Some(v) = get_setting(pool, "show_subtasks_expanded").await { s.show_subtasks_expanded = v != "false"; }
    if let Some(v) = get_setting(pool, "keybinds").await { s.keybinds = v; }
    if let Some(v) = get_setting(pool, "global_keybinds").await { s.global_keybinds = v; }
    s.focus_mode_auto = get_bool_setting(pool, "focus_mode_auto", true).await;
    s.track_domains = get_bool_setting(pool, "track_domains", false).await;
    s.language = get_setting(pool, "language").await.unwrap_or_default();
    s.history_cleanup_months = get_u64_setting(pool, "history_cleanup_months", 0).await;
    // Keys: the keyring first, then the legacy value from the DB
    let openai_from_keyring = keyring_get("openai_key");
    let anthropic_from_keyring = keyring_get("anthropic_key");
    s.openai_in_keyring = openai_from_keyring.is_some();
    s.anthropic_in_keyring = anthropic_from_keyring.is_some();
    s.openai_key = openai_from_keyring
        .or(get_setting(pool, "openai_key").await)
        .unwrap_or_default();
    s.anthropic_key = anthropic_from_keyring
        .or(get_setting(pool, "anthropic_key").await)
        .unwrap_or_default();
    Ok(s)
}

#[tauri::command]
pub async fn get_settings(pool: State<'_, SqlitePool>) -> AppResult<AppSettings> {
    load_settings_raw(pool.inner()).await
}

#[tauri::command]
pub async fn save_settings(
    app: tauri::AppHandle,
    pool: State<'_, SqlitePool>,
    mode_state: State<'_, Arc<Mutex<WorkMode>>>,
    settings: AppSettings,
) -> AppResult<()> {
    set_setting(pool.inner(), "ai_provider", &settings.ai_provider).await?;
    set_setting(pool.inner(), "openai_model", &settings.openai_model).await?;
    set_setting(pool.inner(), "anthropic_model", &settings.anthropic_model).await?;
    // Minimums: values that would break tracking cannot be set
    set_setting(pool.inner(), "idle_threshold_secs", &settings.idle_threshold_secs.max(60).to_string()).await?;
    set_setting(pool.inner(), "log_interval_secs", &settings.log_interval_secs.clamp(10, 600).to_string()).await?;
    set_setting(pool.inner(), "work_mode", settings.work_mode.as_str()).await?;
    set_setting(pool.inner(), "onboarding_complete", if settings.onboarding_complete { "true" } else { "false" }).await?;
    set_setting(pool.inner(), "deadline_warn_hours", &settings.deadline_warn_hours.max(1).to_string()).await?;
    set_setting(pool.inner(), "deadline_warn_minutes", &settings.deadline_warn_minutes.clamp(1, 1440).to_string()).await?;
    set_setting(pool.inner(), "idle_notify_min_mins", &settings.idle_notify_min_mins.max(1).to_string()).await?;
    set_setting(pool.inner(), "pomodoro_work_mins", &settings.pomodoro_work_mins.clamp(1, 120).to_string()).await?;
    set_setting(pool.inner(), "pomodoro_break_mins", &settings.pomodoro_break_mins.clamp(1, 60).to_string()).await?;
    // 0 means off; otherwise at least 20 minutes so it does not spam
    set_setting(pool.inner(), "nudge_after_mins", &(if settings.nudge_after_mins == 0 { 0 } else { settings.nudge_after_mins.max(20) }).to_string()).await?;
    // Theme: mode plus colour overrides (an empty string means the CSS default)
    let theme_mode = match settings.theme_mode.as_str() { "light" | "dark" | "system" => settings.theme_mode.as_str(), _ => "system" };
    set_setting(pool.inner(), "theme_mode", theme_mode).await?;
    set_setting(pool.inner(), "color_accent", &settings.color_accent).await?;
    set_setting(pool.inner(), "color_accent_secondary", &settings.color_accent_secondary).await?;
    set_setting(pool.inner(), "color_bg", &settings.color_bg).await?;
    set_setting(pool.inner(), "color_bg_secondary", &settings.color_bg_secondary).await?;
    set_setting(pool.inner(), "color_bg_hover", &settings.color_bg_hover).await?;
    set_setting(pool.inner(), "color_bg_card", &settings.color_bg_card).await?;
    set_setting(pool.inner(), "color_text_secondary", &settings.color_text_secondary).await?;
    set_setting(pool.inner(), "color_text", &settings.color_text).await?;
    set_setting(pool.inner(), "color_border", &settings.color_border).await?;
    // Notification pause: empty means off; otherwise only valid RFC3339
    let quiet = if settings.quiet_until.is_empty()
        || chrono::DateTime::parse_from_rfc3339(&settings.quiet_until).is_ok()
    {
        settings.quiet_until.as_str()
    } else {
        ""
    };
    set_setting(pool.inner(), "quiet_until", quiet).await?;
    set_setting(pool.inner(), "context_notifications", if settings.context_notifications { "true" } else { "false" }).await?;
    set_setting(pool.inner(), "ai_fallback", if settings.ai_fallback { "true" } else { "false" }).await?;
    // App categorization rules: only a valid JSON array is stored
    let rules = if crate::commands::monitor::parse_category_rules(&settings.app_category_rules).is_empty()
        && !settings.app_category_rules.trim().is_empty()
    {
        "" // junk is not saved
    } else {
        settings.app_category_rules.as_str()
    };
    set_setting(pool.inner(), "app_category_rules", rules).await?;
    // Saved colour sets: same contract — only a valid JSON array is stored, so a
    // malformed value cannot make the settings screen unopenable next launch.
    let presets = if settings.custom_theme_presets.trim().is_empty()
        || is_json_array(&settings.custom_theme_presets)
    {
        settings.custom_theme_presets.as_str()
    } else {
        "" // junk is not saved
    };
    set_setting(pool.inner(), "custom_theme_presets", presets).await?;
    // Category limits: same logic — junk is not saved
    let limits = if crate::commands::monitor::parse_app_limits(&settings.app_limits).is_empty()
        && !settings.app_limits.trim().is_empty()
    {
        ""
    } else {
        settings.app_limits.as_str()
    };
    set_setting(pool.inner(), "app_limits", limits).await?;
    set_setting(pool.inner(), "auto_backup_dir", &settings.auto_backup_dir).await?;
    set_setting(pool.inner(), "auto_backup_keep", &settings.auto_backup_keep.max(1).to_string()).await?;
    set_setting(pool.inner(), "morning_digest_time", &settings.morning_digest_time).await?;
    set_setting(pool.inner(), "show_subtasks_expanded", if settings.show_subtasks_expanded { "true" } else { "false" }).await?;
    set_setting(pool.inner(), "keybinds", &settings.keybinds).await?;
    set_setting(pool.inner(), "global_keybinds", &settings.global_keybinds).await?;
    set_setting(pool.inner(), "focus_mode_auto", if settings.focus_mode_auto { "true" } else { "false" }).await?;
    set_setting(pool.inner(), "track_domains", if settings.track_domains { "true" } else { "false" }).await?;
    set_setting(pool.inner(), "language", &settings.language).await?;
    // 0 means off; otherwise at least 1 month (a fractional value must not silently become 0)
    set_setting(pool.inner(), "history_cleanup_months", &(if settings.history_cleanup_months == 0 { 0 } else { settings.history_cleanup_months.max(1) }).to_string()).await?;

    for (name, value) in [("openai_key", &settings.openai_key), ("anthropic_key", &settings.anthropic_key)] {
        match keyring_set(name, value) {
            Ok(()) => {
                // The key is in the keyring — clean up any legacy copy in the DB
                set_setting(pool.inner(), name, "").await?;
            }
            Err(_) => {
                // The keyring is unavailable — fall back to the DB as before
                set_setting(pool.inner(), name, value).await?;
            }
        }
    }

    *mode_state.lock().unwrap() = settings.work_mode.clone();
    crate::update_mode_checks(&app, &settings.work_mode);
    Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn work_mode_roundtrip() {
    for mode in [WorkMode::Light, WorkMode::Study, WorkMode::Focus] {
      assert_eq!(WorkMode::from_str(mode.as_str()), mode);
    }
  }

  #[test]
  fn work_mode_unknown_falls_back_to_light() {
    assert_eq!(WorkMode::from_str("abrakadabra"), WorkMode::Light);
    assert_eq!(WorkMode::from_str(""), WorkMode::Light);
  }
}
// Integration tests over in-memory SQLite: real migrations, real queries
#[cfg(test)]
mod db_tests {
    use super::*;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./src/db/migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn defaults_when_db_empty() {
        let pool = test_pool().await;
        let s = load_settings_raw(&pool).await.unwrap();
        assert_eq!(s.work_mode, WorkMode::Light);
        assert_eq!(s.idle_threshold_secs, 300);
        assert!(!s.onboarding_complete);
    }

    #[tokio::test]
    async fn set_get_roundtrip() {
        let pool = test_pool().await;
        set_setting(&pool, "ai_provider", "anthropic").await.unwrap();
        assert_eq!(get_setting(&pool, "ai_provider").await.unwrap(), "anthropic");

        // A repeated write overwrites rather than duplicating
        set_setting(&pool, "ai_provider", "openai").await.unwrap();
        assert_eq!(get_setting(&pool, "ai_provider").await.unwrap(), "openai");
    }

    #[tokio::test]
    async fn persist_work_mode_is_loaded_back() {
        let pool = test_pool().await;
        persist_work_mode(&pool, &WorkMode::Focus).await.unwrap();
        let s = load_settings_raw(&pool).await.unwrap();
        assert_eq!(s.work_mode, WorkMode::Focus);
    }

    #[tokio::test]
    async fn color_accent_secondary_defaults_empty_and_roundtrips() {
        let pool = test_pool().await;
        let s = load_settings_raw(&pool).await.unwrap();
        assert_eq!(s.color_accent_secondary, "");

        set_setting(&pool, "color_accent_secondary", "#f43f5e").await.unwrap();
        let s = load_settings_raw(&pool).await.unwrap();
        assert_eq!(s.color_accent_secondary, "#f43f5e");
    }

    #[tokio::test]
    async fn custom_theme_presets_default_empty_and_roundtrip() {
        let pool = test_pool().await;
        let s = load_settings_raw(&pool).await.unwrap();
        assert_eq!(s.custom_theme_presets, "", "без сохранённых наборов ключ пуст");

        let json = r##"[{"name":"Ночной","colors":{"color_accent":"#123456"}}]"##;
        set_setting(&pool, "custom_theme_presets", json).await.unwrap();
        let s = load_settings_raw(&pool).await.unwrap();
        assert_eq!(s.custom_theme_presets, json);
    }

    #[test]
    fn only_json_arrays_pass_the_presets_shape_check() {
        // The guard that keeps a malformed value out of the DB: anything that is
        // not an array would come back as an empty list next launch anyway, so
        // storing it only hides the loss until then.
        assert!(is_json_array("[]"));
        assert!(is_json_array(r#"[{"name":"a","colors":{}}]"#));
        assert!(!is_json_array("{}"), "объект — не массив наборов");
        assert!(!is_json_array("не json"));
        assert!(!is_json_array("null"));
        assert!(!is_json_array("\"строка\""));
    }

    #[tokio::test]
    async fn neutral_surface_colours_default_empty_and_roundtrip() {
        let pool = test_pool().await;
        let s = load_settings_raw(&pool).await.unwrap();
        assert_eq!(s.color_bg_secondary, "", "неустановленный фон сайдбара — пустая строка, а не цвет");
        assert_eq!(s.color_bg_hover, "", "неустановленный фон наведения — пустая строка, а не цвет");

        set_setting(&pool, "color_bg_secondary", "#f4f2f8").await.unwrap();
        set_setting(&pool, "color_bg_hover", "#eae7f2").await.unwrap();
        let s = load_settings_raw(&pool).await.unwrap();
        assert_eq!(s.color_bg_secondary, "#f4f2f8");
        assert_eq!(s.color_bg_hover, "#eae7f2");
    }

    #[tokio::test]
    async fn keybinds_defaults_empty_and_roundtrips() {
        let pool = test_pool().await;
        let s = load_settings_raw(&pool).await.unwrap();
        assert_eq!(s.keybinds, "");

        set_setting(&pool, "keybinds", r#"{"palette":"Ctrl+KeyJ"}"#).await.unwrap();
        let s = load_settings_raw(&pool).await.unwrap();
        assert_eq!(s.keybinds, r#"{"palette":"Ctrl+KeyJ"}"#);
    }

    // last_auto_backup is written by the backup loop and must reach the frontend,
    // which is exactly what was broken until v0.9.85: the key was written, but the
    // field existed in neither AppSettings nor load_settings_raw, so it never
    // crossed the boundary and Settings.svelte rendered a variable nobody assigned.
    #[tokio::test]
    async fn last_auto_backup_reaches_the_frontend() {
        let pool = test_pool().await;
        assert_eq!(load_settings_raw(&pool).await.unwrap().last_auto_backup, "");

        set_setting(&pool, "last_auto_backup", "2026-08-05T10:00:00+00:00").await.unwrap();
        assert_eq!(
            load_settings_raw(&pool).await.unwrap().last_auto_backup,
            "2026-08-05T10:00:00+00:00",
            "last_auto_backup не доходит до фронтенда"
        );
    }

    // save_settings takes State/AppHandle and cannot be called from a test, so the
    // guarantee is asserted on the source instead: the command must never write
    // last_auto_backup. If it did, a Settings page opened before a backup ran
    // would push the stale (empty) value back on save and erase the timestamp —
    // the field is backend-owned precisely to avoid that.
    #[test]
    fn save_settings_never_writes_the_backend_owned_timestamp() {
        let src = include_str!("settings.rs");
        let start = src
            .find("pub async fn save_settings(")
            .expect("save_settings not found");
        let end = src[start..]
            .find("\n}\n")
            .expect("save_settings is not closed") + start;
        let body = &src[start..end];

        // Prefix match on purpose: it covers last_auto_backup_error (v0.9.86) and
        // any future backend-owned key sharing the prefix.
        assert!(
            !body.contains("\"last_auto_backup"),
            "save_settings пишет last_auto_backup* — эти поля принадлежат бэкенду"
        );
        // Guard against the check silently passing on a body the parser lost.
        assert!(
            body.contains("\"auto_backup_dir\""),
            "разбор save_settings сломан: в теле нет даже auto_backup_dir"
        );
    }
}

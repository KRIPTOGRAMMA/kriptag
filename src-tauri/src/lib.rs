mod core;
mod db;
mod error;
mod commands;
mod notifier;
mod monitor;
mod ai;
mod voice;
mod status;
mod i18n;
mod stem;
mod mock_guard;
mod palette_guard;

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{Emitter, Manager};
use tauri::menu::{Menu, MenuItem, CheckMenuItem, Submenu};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};

// Plain (non-checkbox) menu items: some tray hosts (waybar, for one) render
// checkbox items (GtkCheckMenuItem) incorrectly, showing the status text instead
// of the name ("OFF" rather than "Light"/"Focus"/"Study"). The workaround is to
// mark the active mode with a "✓ " prefix inside the item's own text.
pub type ModeItems = Arc<Mutex<Vec<MenuItem<tauri::Wry>>>>;
pub type QuietItems = Arc<Mutex<Vec<CheckMenuItem<tauri::Wry>>>>;
// Items with fixed text: they must be relabelled once the chosen language is
// known, after the settings have been loaded. The key is the item's id.
pub type LabeledItems = Arc<Mutex<Vec<(&'static str, MenuItem<tauri::Wry>)>>>;
// The quick-capture window's initial mode: "task" | "note". It lives as managed
// state so QuickCapture can read it on mount (an emit-before-mount race).
pub type QuickMode = Arc<Mutex<String>>;
use crate::db::init_db;
use crate::ai::sidecar::{SharedSidecar, SidecarState};

#[tauri::command]
fn is_wayland() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok()
}

// The activity tracking mode: extended means system idle/resume via
// ext-idle-notify-v1 (Wayland), basic means only input inside the app window.
pub struct ExtendedTracking(pub bool);

#[tauri::command]
fn get_tracking_mode(mode: tauri::State<'_, ExtendedTracking>) -> &'static str {
    if mode.0 { "extended" } else { "basic" }
}

// The name of the active-window provider, if capability detection found a working one.
pub struct WindowTracking(pub Option<&'static str>);

#[tauri::command]
fn get_window_tracking(state: tauri::State<'_, WindowTracking>) -> Option<&'static str> {
    state.0
}

// Applies the saved global hotkeys without a restart. Returns the combinations
// that could not be registered (taken by the system, or unparseable) — the
// frontend shows them to the user, because a global hotkey that silently fails
// looks like a broken application.
#[tauri::command]
async fn apply_global_hotkeys(
    app: tauri::AppHandle,
    pool: tauri::State<'_, sqlx::SqlitePool>,
) -> crate::error::AppResult<Vec<String>> {
    let value = commands::settings::get_setting(pool.inner(), "global_keybinds")
        .await
        .unwrap_or_default();
    Ok(register_global_hotkeys(&app, &value))
}

fn normalize_quick_mode(mode: &str) -> &'static str {
    match mode {
        "note" => "note",
        // A note from the clipboard. A separate mode rather than a flag on
        // "note": the window must open with the text already in it, requiring no
        // manual paste, while still allowing an edit and a confirmation.
        "clipboard" => "clipboard",
        // Editing the pinned task or note. Not "create" but "open something that
        // already exists", hence a separate mode rather than a flag.
        "pinned" => "pinned",
        _ => "task",
    }
}

// (Re)registers the global hotkeys from the current settings.
//
// Called at startup and after the settings are saved, so it first unregisters
// everything previously registered: otherwise an old combination would keep
// working until a restart and the user would have two live at once.
//
// Each combination is registered by its own call rather than in a batch: a
// combination taken by the system then breaks only itself and the other three
// keep working. In a batch (`on_shortcuts`) a single error would cancel them all.
fn register_global_hotkeys(app: &tauri::AppHandle, global_keybinds: &str) -> Vec<String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

    let _ = app.global_shortcut().unregister_all();

    let overrides = commands::hotkeys::parse_overrides(global_keybinds);
    let mut failed = Vec::new();

    for (combo, mode) in commands::hotkeys::resolve_bindings(&overrides) {
        let shortcut = match combo.parse::<Shortcut>() {
            Ok(s) => s,
            Err(_) => {
                failed.push(combo);
                continue;
            }
        };
        let mode = mode.to_string();
        let res = app.global_shortcut().on_shortcut(shortcut, move |app, _, event| {
            if event.state == ShortcutState::Pressed {
                show_quick_capture(app, &mode);
            }
        });
        if res.is_err() {
            // Most often the combination is already taken by another application
            // or by the compositor. Not a startup error: we report it and carry on.
            failed.push(combo);
        }
    }
    failed
}

// The single path for opening the quick-capture window: record the mode, notify
// the window (if it is already mounted) and show it. Used both by the command
// from the frontend and by the global hotkeys.
fn show_quick_capture(app: &tauri::AppHandle, mode: &str) {
    let mode = normalize_quick_mode(mode);
    if let Some(state) = app.try_state::<QuickMode>() {
        *state.lock().unwrap() = mode.to_string();
    }
    let _ = app.emit_to("quick-task", "quick-mode", mode);
    if let Some(w) = app.get_webview_window("quick-task") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

#[tauri::command]
fn open_quick_capture(app: tauri::AppHandle, mode: String) {
    show_quick_capture(&app, &mode);
}

// The clipboard text for "clipboard" mode. An empty clipboard, or one holding an
// image or a file, is not an error but an empty string: the frontend then simply
// opens an ordinary empty note instead of showing an error.
#[tauri::command]
fn read_clipboard_text(app: tauri::AppHandle) -> String {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().read_text().unwrap_or_default()
}

/// Marks a launch that the operating system started at login rather than the
/// user starting it themselves.
///
/// Registered into the autostart entry, so it is present on exactly those
/// launches and on no others. The name is part of that entry once autostart has
/// been enabled: renaming it leaves existing installations passing the old flag,
/// which this build would no longer recognise, and they would go back to opening
/// a window at login.
const AUTOSTART_FLAG: &str = "--autostart";

/// Whether this launch came from the autostart entry.
fn is_autostart(args: &[String]) -> bool {
    args.iter().any(|a| a == AUTOSTART_FLAG)
}

// The quick-capture mode from the CLI arguments (--quick-note / --quick-task /
// -q). A shared parser for the first launch and for arguments forwarded by a
// second instance through single-instance (from WM binds on Wayland).
fn quick_mode_from_args(args: &[String]) -> Option<&'static str> {
    if args.iter().any(|a| a == "--quick-pinned") {
        Some("pinned")
    } else if args.iter().any(|a| a == "--quick-clip") {
        Some("clipboard")
    } else if args.iter().any(|a| a == "--quick-note") {
        Some("note")
    } else if args.iter().any(|a| a == "--quick-task" || a == "-q") {
        Some("task")
    } else {
        None
    }
}

#[tauri::command]
fn get_quick_mode(mode: tauri::State<'_, QuickMode>) -> String {
    mode.lock().unwrap().clone()
}


fn update_mode_checks(app: &tauri::AppHandle, mode: &commands::settings::WorkMode) {
    if let Some(items) = app.try_state::<ModeItems>() {
        let items = items.lock().unwrap();
        let active_id = format!("mode_{}", mode.as_str());
        for item in items.iter() {
            let id = item.id().as_ref().to_string();
            let name = id.strip_prefix("mode_").unwrap_or(&id);
            let label = if id == active_id { format!("✓ {name}") } else { name.to_string() };
            let _ = item.set_text(label);
        }
    }
}

// The checkmark on the active notification-pause item. active_id is a menu item
// id ("quiet_off" | "quiet_30" | ... | "quiet_inf"); an unrecognized id clears
// every checkmark.
fn update_quiet_checks(app: &tauri::AppHandle, active_id: &str) {
    if let Some(items) = app.try_state::<QuietItems>() {
        let items = items.lock().unwrap();
        for item in items.iter() {
            let _ = item.set_checked(item.id().as_ref() == active_id);
        }
    }
}

// Which pause menu item should be checked for a given quiet_until. preset is the
// stored preset id (quiet_preset in settings), which lets a timed pause restore
// its checkmark after a restart. A legacy value with no preset while a timed
// pause is active yields "quiet_timed" (no item is checked).
fn quiet_check_id(quiet_until: &str, preset: &str, now: chrono::DateTime<chrono::Utc>) -> &'static str {
    if quiet_until == commands::settings::QUIET_FOREVER {
        return "quiet_inf";
    }
    match chrono::DateTime::parse_from_rfc3339(quiet_until) {
        Ok(t) if now < t.with_timezone(&chrono::Utc) => match preset {
            "quiet_30" => "quiet_30",
            "quiet_60" => "quiet_60",
            "quiet_120" => "quiet_120",
            _ => "quiet_timed",
        },
        _ => "quiet_off",
    }
}

// The remaining minutes of an active timed pause (rounded up); None when the
// pause is inactive or indefinite.
fn quiet_remaining_mins(quiet_until: &str, now: chrono::DateTime<chrono::Utc>) -> Option<i64> {
    if quiet_until == commands::settings::QUIET_FOREVER {
        return None;
    }
    let t = chrono::DateTime::parse_from_rfc3339(quiet_until)
        .ok()?
        .with_timezone(&chrono::Utc);
    let secs = (t - now).num_seconds();
    if secs <= 0 {
        return None;
    }
    Some((secs + 59) / 60)
}

fn quiet_base_label(id: &str) -> &'static str {
    match id {
        "quiet_30" => "30 минут",
        "quiet_60" => "1 час",
        "quiet_120" => "2 часа",
        "quiet_inf" => "Бессрочно",
        _ => "Выкл",
    }
}

// Labels for the pause items: the active timed preset shows what remains ("1
// hour — 42 min left"), the rest show their base label.
// Relabels the tray items with fixed text to the chosen language. The submenus
// ("Mode", "Pause notifications", "Pomodoro") cannot be relabelled — Submenu does
// not expose set_text in this version of Tauri, so their titles stay in the OS
// locale's language. That is recorded in PLANS.md as a known limitation, not as
// an oversight.
fn relabel_tray(app: &tauri::AppHandle, lang: crate::i18n::Lang) {
    if let Some(items) = app.try_state::<LabeledItems>() {
        let items = items.lock().unwrap();
        for (key, item) in items.iter() {
            let _ = item.set_text(crate::i18n::tr(key, lang));
        }
    }
}

fn update_quiet_labels(app: &tauri::AppHandle, active_id: &str, remaining_mins: Option<i64>, lang: crate::i18n::Lang) {
    if let Some(items) = app.try_state::<QuietItems>() {
        let items = items.lock().unwrap();
        for item in items.iter() {
            let id = item.id().as_ref();
            let base = crate::i18n::tr(quiet_base_label(id), lang);
            let text = match remaining_mins {
                Some(m) if id == active_id => crate::i18n::tr_args(
                    "{base} — осталось {n} мин", lang,
                    &[("base", base.clone()), ("n", m.to_string())]),
                _ => base.clone(),
            };
            let _ = item.set_text(text);
        }
    }
}

/// What the main window remembers between launches.
///
/// Deliberately not `StateFlags::all()`, which is the plugin's default. That set
/// also carries DECORATIONS and VISIBLE, and both undo a decision made elsewhere:
/// the plugin calls `set_decorations(state.decorated)`, which would put the
/// system title bar back over the one this app draws itself, and it calls
/// `show()`, which would reveal a window whose config says `visible: false`.
///
/// One constant because these flags are used twice — once when registering the
/// plugin, once when saving on hide. Two lists would drift apart silently: the
/// saved state would carry fields the restore ignores, or the reverse.
const WINDOW_STATE_FLAGS: tauri_plugin_window_state::StateFlags = {
    use tauri_plugin_window_state::StateFlags;
    StateFlags::SIZE
        .union(StateFlags::POSITION)
        .union(StateFlags::MAXIMIZED)
        .union(StateFlags::FULLSCREEN)
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // `kriptag --status` is a short-lived CLI for status bars (waybar): it prints
    // JSON and exits without starting Tauri. Checked before anything else so it
    // does not disturb the single-instance of a running application.
    if std::env::args().any(|a| a == "--status") {
        status::print_status();
        return;
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let app = tauri::Builder::default()
                // Registered first: a second launch (say `kriptag --quick-task`
                // from a Hyprland bind) does not start a new instance but forwards
                // its arguments here — we open quick capture or the main window.
                .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
                    if let Some(mode) = quick_mode_from_args(&argv) {
                        show_quick_capture(app, mode);
                    } else if let Some(w) = app.get_webview_window("main") {
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }))
                // Without this the main window opens at the 800x600 from
                // tauri.conf.json on every launch, so a maximised window had to be
                // maximised again by hand each time — most visibly at login, where
                // autostart brings it up small.
                //
                // The flag set is deliberately not the default. `StateFlags::all()`
                // also restores DECORATIONS and VISIBLE, and both are wrong here:
                // the plugin calls `set_decorations(state.decorated)`, which would
                // put the system title bar back over the one this app draws itself
                // (decorations: false is a deliberate choice, see v0.9.40), and it
                // calls `show()`, which would reveal quick capture — a window whose
                // whole design is to stay hidden until a hotkey asks for it.
                //
                // Quick capture is denylisted for the same reason its config says
                // `center: true` and a fixed size: it is a popup, not a workspace.
                // Restoring its geometry would drop it wherever it was last used
                // instead of in front of the user.
                .plugin(
                    tauri_plugin_window_state::Builder::new()
                        .with_state_flags(WINDOW_STATE_FLAGS)
                        .with_denylist(&["quick-task"])
                        .build(),
                )
                .plugin(tauri_plugin_opener::init())
                .plugin(tauri_plugin_notification::init())
                // The flag is what makes a login launch distinguishable from one
                // the user asked for: it is written into the autostart entry and
                // is therefore absent when the app is started by hand. Starting
                // at login should put the app in the tray, not throw a window
                // over whatever the user is doing.
                //
                // A bare flag is safe on a Windows command line — no spaces, no
                // backslashes, nothing for an argument parser to split (unlike
                // the model path in v0.10.25).
                .plugin(tauri_plugin_autostart::init(
                    tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                    Some(vec![AUTOSTART_FLAG]),
                ))
                .plugin(tauri_plugin_global_shortcut::Builder::new().build())
                .plugin(tauri_plugin_shell::init())
                .plugin(tauri_plugin_dialog::init())
                .plugin(tauri_plugin_clipboard_manager::init())
                .invoke_handler(
                    tauri::generate_handler![
                        commands::tasks::create_task,
                        commands::tasks::get_tasks,
                        commands::tasks::delete_task,
                        commands::tasks::get_deleted_tasks,
                        commands::tasks::restore_task,
                        commands::tasks::purge_deleted_task,
                        commands::tasks::update_task,
                        commands::tasks::complete_task,
                        commands::tasks::search_tasks,
                        commands::tasks::search_tasks_snippet,
                        commands::tasks::reorder_tasks,
                        commands::projects::get_projects,
                        commands::projects::create_project,
                        commands::projects::update_project,
                        commands::projects::delete_project,
                        commands::projects::get_goal_history,
                        commands::categories::get_categories,
                        commands::categories::create_category,
                        commands::categories::update_category,
                        commands::categories::delete_category,
                        commands::statuses::get_statuses,
                        commands::statuses::create_status,
                        commands::statuses::update_status,
                        commands::statuses::delete_status,
                        apply_global_hotkeys,
                        commands::hotkeys::list_global_actions,
                        commands::hotkeys::validate_global_combo,
                        commands::pinned::get_pinned_item,
                        commands::pinned::set_pinned_item,
                        commands::pomodoro::get_pomodoro_state,
                        commands::pomodoro::pomodoro_toggle_pause,
                        commands::pomodoro::pomodoro_skip,
                        commands::pomodoro::pomodoro_start,
                        commands::pomodoro::pomodoro_stop,
                        commands::pomodoro::get_pomodoro_stats,
                        open_quick_capture,
                        read_clipboard_text,
                        get_quick_mode,
                        is_wayland,
                        get_tracking_mode,
                        get_window_tracking,
                        commands::monitor::record_input,
                        commands::monitor::get_session_stats,
                        commands::monitor::get_activity_state,
                        commands::monitor::get_activity_by_day,
                        commands::monitor::get_task_completions_by_day,
                        commands::monitor::get_app_usage,
                        commands::monitor::get_uncategorized_apps,
                        commands::ai::ai_suggest_app_rules,
                        commands::monitor::get_completions_for_day,
                        commands::monitor::get_hourly_activity,
                        commands::monitor::get_app_category_time,
                        commands::monitor::get_category_distribution,
                        commands::monitor::get_active_idle_ratio,
                        commands::monitor::get_block_idle,
                        commands::monitor::get_domain_usage,
                        commands::monitor::clear_domain_history,
                        commands::ai::ai_rewrite,
                        commands::ai::ai_subtasks,
                        commands::ai::ai_classify,
                        commands::ai::dashboard_insight,
                        commands::ai::summarize_day,
                        commands::ai::summarize_week,
                        commands::ai::ai_edit_selection,
                        commands::ai::ai_summarize_note,
                        commands::ai::ai_extract_tasks,
                        commands::rag::ai_ask_notes,
                        commands::planner::ai_plan_day,
                        commands::planner::ai_what_now,
                        commands::notes::get_notes,
                        commands::notes::create_note,
                        commands::notes::update_note,
                        commands::notes::delete_note,
                        commands::notes::get_deleted_notes,
                        commands::notes::restore_note,
                        commands::notes::purge_deleted_note,
                        commands::notes::search_notes,
                        commands::notes::search_notes_snippet,
                        commands::notes::rename_note_links,
                        commands::notes::get_note_revisions,
                        commands::notes::get_note_revision_content,
                        commands::notes::restore_note_revision,
                        commands::notes::export_notes_md,
                        commands::notes::import_notes_md,
                        commands::notes::export_note_html,
                        commands::note_links::ai_suggest_links,
                        commands::settings::get_settings,
                        commands::settings::save_settings,
                        commands::backup::export,
                        commands::backup::import,
                        commands::backup::preview_import,
                        commands::backup::do_auto_backup,
                        commands::model::list_model_options,
                        commands::model::model_status,
                        commands::model::model_path,
                        commands::model::download_model,
                        commands::voice::voice_available,
                        commands::voice::start_voice_recording,
                        commands::voice::stop_voice_recording,
                        commands::voice::cancel_voice_recording,
                        commands::subtasks::get_subtasks,
                        commands::subtasks::add_subtask,
                        commands::subtasks::toggle_subtask,
                        commands::subtasks::delete_subtask,
                        commands::subtasks::rename_subtask,
                        commands::dependencies::get_task_blockers,
                        commands::dependencies::add_task_dependency,
                        commands::dependencies::remove_task_dependency,
                        commands::routines::get_routines,
                        commands::routines::create_routine,
                        commands::routines::update_routine,
                        commands::routines::delete_routine,
                        commands::tracking::start_task_tracking,
                        commands::tracking::stop_task_tracking,
                        commands::tracking::get_active_session,
                        commands::tracking::get_task_seconds,
                        commands::tracking::get_project_seconds,
                        commands::images::save_note_image,
                        commands::images::get_images_dir,
                        commands::checklists::get_checklist_templates,
                        commands::checklists::create_checklist_template,
                        commands::checklists::delete_checklist_template,
                        commands::smart_lists::get_smart_lists,
                        commands::smart_lists::create_smart_list,
                        commands::smart_lists::delete_smart_list,
                        commands::notifications::get_notification_log,
                        commands::notifications::get_unread_notification_count,
                        commands::notifications::mark_notifications_read,
                        commands::notifications::clear_notification_log
                    ]
                )
                .setup(|app| {
                    // Tray
                    let open = MenuItem::with_id(app, "open", "Открыть", true, None::<&str>)?;
                    // Plain (non-checkbox) items — see the comment on ModeItems: the
                    // active mode is marked with a "✓ " prefix in the text, applied by
                    // update_mode_checks once the settings are loaded.
                    let mode_light = MenuItem::with_id(app, "mode_Light", "Light", true, None::<&str>)?;
                    let mode_focus = MenuItem::with_id(app, "mode_Focus", "Focus", true, None::<&str>)?;
                    let mode_study = MenuItem::with_id(app, "mode_Study", "Study", true, None::<&str>)?;
                    let mode_menu = Submenu::with_items(app, "Режим", true, &[&mode_light, &mode_focus, &mode_study])?;
                    // Notification pause: the checkmark is applied after the settings load
                    let quiet_30 = CheckMenuItem::with_id(app, "quiet_30", quiet_base_label("quiet_30"), true, false, None::<&str>)?;
                    let quiet_60 = CheckMenuItem::with_id(app, "quiet_60", quiet_base_label("quiet_60"), true, false, None::<&str>)?;
                    let quiet_120 = CheckMenuItem::with_id(app, "quiet_120", quiet_base_label("quiet_120"), true, false, None::<&str>)?;
                    let quiet_inf = CheckMenuItem::with_id(app, "quiet_inf", quiet_base_label("quiet_inf"), true, false, None::<&str>)?;
                    let quiet_off = CheckMenuItem::with_id(app, "quiet_off", quiet_base_label("quiet_off"), true, false, None::<&str>)?;
                    let quiet_menu = Submenu::with_items(
                        app,
                        "Пауза уведомлений",
                        true,
                        &[&quiet_30, &quiet_60, &quiet_120, &quiet_inf, &quiet_off],
                    )?;
                    let pomo_pause = MenuItem::with_id(app, "pomo_pause", "Помодоро: пауза/продолжить", true, None::<&str>)?;
                    let pomo_skip = MenuItem::with_id(app, "pomo_skip", "Помодоро: пропустить фазу", true, None::<&str>)?;
                    let pomo_menu = Submenu::with_items(app, "Помодоро", true, &[&pomo_pause, &pomo_skip])?;
                    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
                    let menu = Menu::with_items(app, &[&open, &mode_menu, &quiet_menu, &pomo_menu, &quit])?;

                    let labeled: LabeledItems = Arc::new(Mutex::new(vec![
                        ("Открыть", open.clone()),
                        ("Помодоро: пауза/продолжить", pomo_pause.clone()),
                        ("Помодоро: пропустить фазу", pomo_skip.clone()),
                        ("Выход", quit.clone()),
                    ]));
                    app.manage(labeled);

                    let mode_items: ModeItems = Arc::new(Mutex::new(vec![mode_light, mode_focus, mode_study]));
                    app.manage(mode_items);
                    let quiet_items: QuietItems =
                        Arc::new(Mutex::new(vec![quiet_30, quiet_60, quiet_120, quiet_inf, quiet_off]));
                    app.manage(quiet_items);

                    // The quick-capture window's initial mode (a task by default)
                    let quick_mode: QuickMode = Arc::new(Mutex::new("task".to_string()));
                    app.manage(quick_mode);

                    TrayIconBuilder::new()
                        .icon(app.default_window_icon().unwrap().clone())
                        .menu(&menu)
                        .on_menu_event(|app, event| match event.id.as_ref() {
                            "open" => {
                                if let Some(w) = app.get_webview_window("main") {
                                    let _ = w.show();
                                    let _ = w.set_focus();
                                }
                            }
                            "quit" => app.exit(0),
                            id if id.starts_with("quiet_") => {
                                use commands::settings::QUIET_FOREVER;
                                let value = match id {
                                    "quiet_off" => String::new(),
                                    "quiet_inf" => QUIET_FOREVER.to_string(),
                                    "quiet_30" => (chrono::Utc::now() + chrono::Duration::minutes(30)).to_rfc3339(),
                                    "quiet_60" => (chrono::Utc::now() + chrono::Duration::minutes(60)).to_rfc3339(),
                                    "quiet_120" => (chrono::Utc::now() + chrono::Duration::minutes(120)).to_rfc3339(),
                                    _ => return,
                                };
                                update_quiet_checks(app, id);
                                update_quiet_labels(app, id, quiet_remaining_mins(&value, chrono::Utc::now()),
                                    crate::i18n::lang_from_setting(""));
                                let pool = app.state::<sqlx::SqlitePool>().inner().clone();
                                let preset = id.to_string();
                                tauri::async_runtime::spawn(async move {
                                    let _ = commands::settings::persist_quiet_until(&pool, &value).await;
                                    let _ = commands::settings::persist_quiet_preset(&pool, &preset).await;
                                });
                            }
                            id if id.starts_with("mode_") => {
                                let mode = commands::settings::WorkMode::from_str(&id["mode_".len()..]);
                                *app.state::<Arc<Mutex<commands::settings::WorkMode>>>().lock().unwrap() = mode.clone();
                                // Refresh the tray checkmarks
                                update_mode_checks(app, &mode);
                                let pool = app.state::<sqlx::SqlitePool>().inner().clone();
                                tauri::async_runtime::spawn(async move {
                                    let _ = commands::settings::persist_work_mode(&pool, &mode).await;
                                });
                            }
                            // PomodoroCmdTx is managed later in run() (after the app is
                            // built), but by the time the tray is clicked the app is running.
                            "pomo_pause" => {
                                if let Some(tx) = app.try_state::<commands::pomodoro::PomodoroCmdTx>() {
                                    let _ = tx.0.send(notifier::pomodoro::PomodoroCmd::TogglePause);
                                }
                            }
                            "pomo_skip" => {
                                if let Some(tx) = app.try_state::<commands::pomodoro::PomodoroCmdTx>() {
                                    let _ = tx.0.send(notifier::pomodoro::PomodoroCmd::Skip);
                                }
                            }
                            _ => {}
                        })
                        .on_tray_icon_event(|tray, event| {
                            // A click on the icon opens the window
                            if let TrayIconEvent::Click { button: MouseButton::Left, .. } = event {
                                let app = tray.app_handle();
                                if let Some(w) = app.get_webview_window("main") {
                                    let _ = w.show();
                                    let _ = w.set_focus();
                                }
                            }
                        })
                        .build(app)?;

                    // Hide the main window instead of closing it (so the tray keeps working)
                    if let Some(main_win) = app.get_webview_window("main") {
                        let win = main_win.clone();
                        main_win.on_window_event(move |event| {
                            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                                api.prevent_close();

                                // Write the geometry to disk here, not only on exit.
                                //
                                // The window-state plugin flushes on RunEvent::Exit,
                                // and this app reaches that only through the tray's
                                // "Выход". Closing the window hides it and the
                                // process keeps running, so a session that ends any
                                // other way — logout, reboot, a crash — would lose
                                // the size the user had just chosen. Hiding is the
                                // moment they are done with the window, which makes
                                // it the honest moment to record how they left it.
                                //
                                // Saving before hide(): the plugin reads the live
                                // geometry, and a hidden window can report nothing
                                // useful.
                                use tauri_plugin_window_state::AppHandleExt;
                                let _ = win.app_handle().save_window_state(WINDOW_STATE_FLAGS);

                                let _ = win.hide();
                            }
                        });
                    }

                    // Hide the quick-task window instead of closing it (so the hotkeys keep working)
                    if let Some(quick_win) = app.get_webview_window("quick-task") {
                        let win = quick_win.clone();
                        quick_win.on_window_event(move |event| {
                            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                                api.prevent_close();
                                let _ = win.hide();
                            }
                        });
                    }

                    // Parsing the CLI arguments to support system hotkeys on Wayland
                    let args: Vec<String> = std::env::args().collect();
                    if let Some(mode) = quick_mode_from_args(&args) {
                        if let Some(main_win) = app.get_webview_window("main") {
                            let _ = main_win.hide();
                        }
                        show_quick_capture(&app.app_handle(), mode);
                    } else if is_autostart(&args) {
                        // Started at login: go straight to the tray. The window is
                        // created by the config as visible, so it has to be hidden
                        // rather than never shown — `visible: false` in the config
                        // is not an option, because a launch the user asked for
                        // must still open a window.
                        //
                        // Nothing is lost by hiding: the tray icon's left click and
                        // its "Открыть" item both show this window, and closing it
                        // has always hidden rather than quit, so a hidden main
                        // window is a state the app already lives in.
                        if let Some(main_win) = app.get_webview_window("main") {
                            let _ = main_win.hide();
                        }
                    }

                    // Global hotkey registration moved below, past the DB pool's
                    // initialization: the combinations became rebindable and are read
                    // from the settings, and at this point in setup there is no pool
                    // yet. The defaults and the action list live in
                    // commands/hotkeys.rs.

                    Ok(())
                })
                .build(tauri::generate_context!())
                .expect("error while building tauri application");

            let db_path = app.path()
                .app_data_dir()
                .expect("Failed to get app data dir");
            std::fs::create_dir_all(&db_path)
                .expect("Failed to create app data dir");
            let imported = commands::backup::apply_pending_import(&db_path);
            let db_url = format!("sqlite:{}?mode=rwc", db_path.join("data.db").display());
            let pool: sqlx::SqlitePool = init_db(&db_url).await.expect("Failed to init DB");

            // The imported database brings the backup clock of the copy with it,
            // which would trigger an immediate "overdue" backup and burn a
            // rotation slot. Must run before the backup loop starts below.
            if imported {
                commands::backup::note_import_as_fresh_backup(&pool).await;
            }

            // Before anything else touches settings: switches automatic backups on
            // for a DB that has never been configured (v0.9.85).
            let _ = commands::backup::ensure_default_backup_dir(&pool, &db_path).await;

            app.manage(pool.clone());
            app.manage(Mutex::new(SidecarState::new()) as SharedSidecar);
            app.manage(commands::voice::VoiceState::new());

            let tracker = Arc::new(monitor::activity::ActivityTracker::new());
            app.manage(tracker.clone());

            // Extended tracking: system idle/resume from the compositor. Where it
            // is unsupported (X11, an older compositor) we use the basic mode.
            #[cfg(target_os = "linux")]
            let extended = is_wayland() && monitor::wayland_idle::start(tracker.clone());
            // Elsewhere there is no compositor to ask, so tracking stays basic.
            #[cfg(not(target_os = "linux"))]
            let extended = false;
            app.manage(ExtendedTracking(extended));
            eprintln!("[monitor] режим трекинга: {}", if extended { "расширенный (ext-idle-notify)" } else { "базовый (окно в фокусе)" });

            // Per-app tracking via capability detection; with no provider the app column stays empty.
            let window_provider = monitor::window::detect_provider();
            app.manage(WindowTracking(window_provider.as_ref().map(|p| p.name())));
            eprintln!(
                "[monitor] трекинг приложений: {}",
                window_provider.as_ref().map(|p| p.name()).unwrap_or("недоступен")
            );
            let settings = commands::settings::load_settings_raw(&pool)
                .await
                .unwrap_or_default();
            // Apply the correct mode and pause checkmarks in the tray. The pause
            // preset is stored separately (quiet_preset), so after a restart a timed
            // pause restores both its checkmark and the label showing what remains.
            update_mode_checks(&app.app_handle(), &settings.work_mode);

            // The global hotkeys go here rather than in setup: the combinations live
            // in the settings and the pool only appears at this point.
            let failed = register_global_hotkeys(&app.app_handle(), &settings.global_keybinds);
            if !failed.is_empty() {
                eprintln!("[hotkeys] не удалось зарегистрировать: {}", failed.join(", "));
            }
            let quiet_preset = commands::settings::get_setting(&pool, "quiet_preset")
                .await
                .unwrap_or_default();
            let now = chrono::Utc::now();
            let quiet_id = quiet_check_id(&settings.quiet_until, &quiet_preset, now);
            update_quiet_checks(&app.app_handle(), quiet_id);
            // The tray's language. The menu is built in setup, where there is no
            // pool yet, so it is assembled from the OS locale there; by this point the
            // settings have been read and the labels are rebuilt for the chosen
            // language. Between those two points the tray is not yet visible to the user.
            let tray_lang = crate::i18n::lang_from_setting(&settings.language);
            relabel_tray(&app.app_handle(), tray_lang);
            update_quiet_labels(
                &app.app_handle(),
                quiet_id,
                quiet_remaining_mins(&settings.quiet_until, now),
                tray_lang,
            );

            // The pause watcher: when quiet_until passes we clear the preset's
            // checkmark and tick "Off"; while a timed pause is active we refresh the
            // remaining time in the item's label once a minute.
            {
                let app_handle = app.app_handle().clone();
                let pool_watch = pool.clone();
                let mut last_id = quiet_id;
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                        let value = commands::settings::get_setting(&pool_watch, "quiet_until")
                            .await
                            .unwrap_or_default();
                        let preset = commands::settings::get_setting(&pool_watch, "quiet_preset")
                            .await
                            .unwrap_or_default();
                        let now = chrono::Utc::now();
                        let id = quiet_check_id(&value, &preset, now);
                        if id != last_id && id != "quiet_timed" {
                            update_quiet_checks(&app_handle, id);
                        }
                        last_id = id;
                        let lang = crate::i18n::current_lang(&pool_watch).await;
                        update_quiet_labels(&app_handle, id, quiet_remaining_mins(&value, now), lang);
                    }
                });
            }
            // The work mode is live shared state: save_settings updates it
            // immediately, with no application restart.
            let work_mode = Arc::new(Mutex::new(settings.work_mode.clone()));
            app.manage(work_mode.clone());
            monitor::activity::start_activity_loop(
                app.app_handle().clone(),
                tracker,
                pool.clone(),
                settings.idle_threshold_secs,
                settings.log_interval_secs,
                work_mode.clone(),
                window_provider,
            );

            notifier::scheduler::start_scheduler(app.app_handle().clone(), pool.clone(), work_mode.clone());
            notifier::nudge::start_nudger(app.app_handle().clone(), pool.clone(), work_mode.clone());
            notifier::triggers::start_triggers(app.app_handle().clone(), pool.clone(), work_mode.clone());
            let pomodoro_tx = notifier::pomodoro::start_pomodoro(app.app_handle().clone(), work_mode, pool.clone());
            app.manage(commands::pomodoro::PomodoroCmdTx(pomodoro_tx));

            // Automatic backup: every 60s we check whether a copy is due
            {
                let app_handle = app.app_handle().clone();
                let pool_bk = pool.clone();
                tokio::spawn(async move {
                    loop {
                        if commands::backup::auto_backup_due(&pool_bk).await {
                            let data_dir = match app_handle.path().app_data_dir() {
                                Ok(d) => d,
                                Err(_) => { tokio::time::sleep(Duration::from_secs(60)).await; continue; }
                            };
                            // run_auto_backup, not auto_backup_impl: the failure has
                            // to be recorded, since nothing here can report it.
                            let _ = commands::backup::run_auto_backup(&pool_bk, &data_dir).await;
                        }
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                });
            }

            // Automatic history cleanup: every 60s we check the daily gate; the move
            // to the Trash itself happens at most once every 24h (history_cleanup_due).
            {
                let pool_cleanup = pool.clone();
                tokio::spawn(async move {
                    loop {
                        if commands::tasks::history_cleanup_due(&pool_cleanup).await {
                            let months = commands::settings::get_u64_setting(&pool_cleanup, "history_cleanup_months", 0).await;
                            let cutoff = chrono::Utc::now() - chrono::Duration::days(months as i64 * 30);
                            let _ = commands::tasks::cleanup_old_history_impl(&pool_cleanup, cutoff).await;
                            let _ = commands::settings::set_setting(&pool_cleanup, "last_history_cleanup", &chrono::Utc::now().to_rfc3339()).await;
                        }
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                });
            }
            app.run(|_, _| {});
        });
}
#[cfg(test)]
mod tests {
    use super::{is_autostart, normalize_quick_mode, quick_mode_from_args, quiet_check_id, quiet_remaining_mins};
    use chrono::{TimeZone, Utc};

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap()
    }

    /// The production half of this file.
    ///
    /// include_str! reads the tests too, so a guard looking for a literal finds
    /// its own text and passes on broken code. Same split, and same reason, as
    /// the one in ai/sidecar.rs.
    fn production_src() -> &'static str {
        const SRC: &str = include_str!("lib.rs");
        SRC.split(concat!("#[cfg(", "test)]")).next().unwrap_or("")
    }

    #[test]
    fn hiding_the_main_window_writes_its_geometry_to_disk() {
        // The plugin only flushes to disk on RunEvent::Exit, which this app
        // reaches through one path: "Выход" in the tray. Closing the window hides
        // it and the process lives on, so a session ending any other way — logout,
        // reboot, a crash — would lose the size just chosen. Verified live: after
        // the plugin was wired up but before this save, no state file existed.
        let src = production_src();
        let hide_arm = src
            .split("api.prevent_close();")
            .nth(1)
            .and_then(|rest| rest.split("win.hide()").next())
            .expect("не найден обработчик скрытия главного окна");
        assert!(
            hide_arm.contains("save_window_state"),
            "размер окна не сохраняется при скрытии — переживёт только выход \
             через трей, но не выключение компьютера"
        );
    }

    #[test]
    fn window_state_restores_geometry_but_not_decorations_or_visibility() {
        let src = production_src();

        // Geometry is the whole point: without it the main window reopens at the
        // 800x600 from tauri.conf.json every launch, which is what made a
        // maximised window have to be maximised again after every login.
        for flag in ["StateFlags::SIZE", "StateFlags::POSITION", "StateFlags::MAXIMIZED"] {
            assert!(
                src.contains(flag),
                "{flag} не восстанавливается — окно снова будет открываться \
                 размером из конфига, а не таким, каким его оставили"
            );
        }

        // The two the default `StateFlags::all()` would add, and both break a
        // deliberate decision elsewhere in this file: DECORATIONS puts the system
        // title bar back over the one the app draws itself, VISIBLE calls show()
        // on a window whose config says visible: false.
        for flag in ["StateFlags::DECORATIONS", "StateFlags::VISIBLE"] {
            assert!(
                !src.contains(flag),
                "{flag} восстанавливается: вернёт системную рамку поверх своей \
                 (decorations: false) либо покажет окно быстрого ввода, которое \
                 должно оставаться скрытым до горячей клавиши"
            );
        }

        // Quick capture is a popup: fixed size, centered, hidden until asked for.
        // Restoring its geometry would put it wherever it was last used.
        assert!(
            src.contains(r#".with_denylist(&["quick-task"])"#),
            "окно быстрого ввода не исключено — его положение начнёт сохраняться, \
             и всплывашка перестанет появляться по центру"
        );
    }

    #[test]
    fn autostart_is_recognised_only_by_its_own_flag() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_autostart(&args(&["kriptag", "--autostart"])));
        // A launch the user asked for must still open a window.
        assert!(!is_autostart(&args(&["kriptag"])));
        assert!(!is_autostart(&args(&["kriptag", "--quick-note"])));
        // Not a prefix match: an unrelated flag that merely starts the same way
        // must not put the app in the tray.
        assert!(!is_autostart(&args(&["kriptag", "--autostart-disable"])));
    }

    #[test]
    fn the_autostart_entry_carries_the_flag_the_app_looks_for() {
        // Two halves that must agree: the plugin writes this flag into the
        // autostart entry, and the startup code decides by it. A literal in one
        // place that drifted from the other would leave the app opening a window
        // at login again, silently and only on the user's machine.
        let src = production_src();
        assert!(
            src.contains("Some(vec![AUTOSTART_FLAG])"),
            "флаг автозапуска не передаётся плагину — запись автозапуска не будет \
             его содержать, и вход в систему снова начнётся с открытого окна"
        );
        assert!(
            src.contains("fn is_autostart"),
            "некому распознать флаг автозапуска"
        );
    }

    #[test]
    fn an_autostart_launch_hides_the_main_window() {
        // The window is created visible by tauri.conf.json, so starting in the
        // tray means hiding it. `visible: false` in the config is not the
        // alternative: a launch the user asked for must still open a window.
        let src = production_src();
        let arm = src
            .split("else if is_autostart(&args)")
            .nth(1)
            .expect("не найдена ветка автозапуска");
        assert!(
            arm.contains("main_win.hide()"),
            "при автозапуске главное окно не прячется — приложение откроется \
             поверх того, чем пользователь занят при входе в систему"
        );
    }

    #[test]
    fn quick_mode_parsing() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(quick_mode_from_args(&args(&["kriptag", "--quick-task"])), Some("task"));
        assert_eq!(quick_mode_from_args(&args(&["kriptag", "-q"])), Some("task"));
        assert_eq!(quick_mode_from_args(&args(&["kriptag", "--quick-note"])), Some("note"));
        // a note wins, as it did in the old startup code
        assert_eq!(quick_mode_from_args(&args(&["kriptag", "--quick-task", "--quick-note"])), Some("note"));
        assert_eq!(quick_mode_from_args(&args(&["kriptag"])), None);
        // the clipboard is the most specific mode and outranks both
        assert_eq!(quick_mode_from_args(&args(&["kriptag", "--quick-clip"])), Some("clipboard"));
        assert_eq!(
            quick_mode_from_args(&args(&["kriptag", "--quick-note", "--quick-clip"])),
            Some("clipboard")
        );
        // Editing the pinned item is the only mode that creates nothing, so it
        // outranks everything: if the slot was explicitly requested, a stray second
        // flag must not replace that with creating a new record.
        assert_eq!(quick_mode_from_args(&args(&["kriptag", "--quick-pinned"])), Some("pinned"));
        assert_eq!(
            quick_mode_from_args(&args(&["kriptag", "--quick-clip", "--quick-pinned"])),
            Some("pinned")
        );
    }

    #[test]
    fn normalize_quick_mode_keeps_known_modes_and_falls_back() {
        assert_eq!(normalize_quick_mode("note"), "note");
        assert_eq!(normalize_quick_mode("clipboard"), "clipboard");
        assert_eq!(normalize_quick_mode("pinned"), "pinned");
        assert_eq!(normalize_quick_mode("task"), "task");
        // an unknown mode yields neither a panic nor an empty string but a safe fallback
        assert_eq!(normalize_quick_mode("мусор"), "task");
        assert_eq!(normalize_quick_mode(""), "task");
    }

    #[test]
    fn check_id_forever_and_off() {
        assert_eq!(quiet_check_id(crate::commands::settings::QUIET_FOREVER, "", now()), "quiet_inf");
        assert_eq!(quiet_check_id("", "", now()), "quiet_off");
        assert_eq!(quiet_check_id("мусор", "quiet_30", now()), "quiet_off");
        // an expired pause means "Off", even if a preset is stored
        assert_eq!(quiet_check_id("2026-07-14T11:00:00+00:00", "quiet_60", now()), "quiet_off");
    }

    #[test]
    fn check_id_restores_preset_for_active_timed_pause() {
        let until = "2026-07-14T12:30:00+00:00";
        assert_eq!(quiet_check_id(until, "quiet_30", now()), "quiet_30");
        assert_eq!(quiet_check_id(until, "quiet_60", now()), "quiet_60");
        assert_eq!(quiet_check_id(until, "quiet_120", now()), "quiet_120");
        // a legacy value with no preset (or with junk) gets no checkmark
        assert_eq!(quiet_check_id(until, "", now()), "quiet_timed");
        assert_eq!(quiet_check_id(until, "quiet_off", now()), "quiet_timed");
    }

    #[test]
    fn remaining_mins_none_when_inactive() {
        assert_eq!(quiet_remaining_mins("", now()), None);
        assert_eq!(quiet_remaining_mins(crate::commands::settings::QUIET_FOREVER, now()), None);
        assert_eq!(quiet_remaining_mins("2026-07-14T11:59:00+00:00", now()), None);
        assert_eq!(quiet_remaining_mins("2026-07-14T12:00:00+00:00", now()), None);
    }

    #[test]
    fn remaining_mins_rounds_up() {
        // exactly 30 minutes: right after clicking the preset
        assert_eq!(quiet_remaining_mins("2026-07-14T12:30:00+00:00", now()), Some(30));
        // 90 seconds rounds up to 2 minutes
        assert_eq!(quiet_remaining_mins("2026-07-14T12:01:30+00:00", now()), Some(2));
        // the final minute
        assert_eq!(quiet_remaining_mins("2026-07-14T12:00:30+00:00", now()), Some(1));
    }
}

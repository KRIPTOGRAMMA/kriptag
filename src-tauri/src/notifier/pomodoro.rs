use std::sync::{Arc, Mutex};
use sqlx::SqlitePool;
use tokio::time::{sleep, Duration};
use crate::commands::settings::{WorkMode, get_u64_setting, get_bool_setting, set_setting};
use crate::notifier::scheduler::{send_pomodoro_notification, PomodoroMoment};

// A user command controlling the cycle (pause/resume/skip phase/manual
// start-stop outside Study).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PomodoroCmd {
    TogglePause,
    Skip,
    Start,
    Stop,
    /// Adds minutes to the current break ("five more minutes" on the notification
    /// that says the break is over). Only meaningful during a break: extending a
    /// work phase has no purpose, so the loop ignores it while working.
    ExtendBreak(u64),
}

// A row is written to pomodoro_log every time a work phase finishes (the
// work->break transition). task_id is the active task-tracking session, if one is
// running at that moment.
async fn log_completed_work(pool: &SqlitePool) {
    let task_id: Option<String> = sqlx::query_scalar(
        "SELECT task_id FROM task_sessions WHERE ended_at IS NULL LIMIT 1"
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let _ = sqlx::query(
        "INSERT INTO pomodoro_log (id, finished_at, task_id) VALUES (?, ?, ?)"
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(task_id)
    .execute(pool)
    .await;
}

// The command channel from the Tauri commands (the UI) into the loop. The
// managed state is a wrapper type so app.manage() does not clash with other
// Sender<T> instances.
pub struct PomodoroCmdTx(pub tokio::sync::mpsc::UnboundedSender<PomodoroCmd>);

// A persistent snapshot of the cycle, read by the frontend (polling) and by
// `kriptag --status`. phase is "work" | "break" | "paused" | "off"; until is the
// RFC3339 end of the current phase (unused by the frontend for "paused"/"off",
// but we still write the last meaningful value just in case).
async fn persist_state(pool: &SqlitePool, phase: &str, until: chrono::DateTime<chrono::Utc>) {
    let _ = set_setting(pool, "pomodoro_phase", phase).await;
    let _ = set_setting(pool, "pomodoro_until", &until.to_rfc3339()).await;
}

pub fn start_pomodoro(
    app: tauri::AppHandle,
    work_mode: Arc<Mutex<WorkMode>>,
    pool: SqlitePool,
) -> tokio::sync::mpsc::UnboundedSender<PomodoroCmd> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PomodoroCmd>();

    tokio::spawn(async move {
        let mut in_study = false;
        // A manual start is independent of Study: set by PomodoroCmd::Start and
        // cleared only by Stop (leaving Study does not touch it).
        let mut manual = false;
        let mut working = true;
        let mut paused = false;
        let mut remaining: u64 = 25 * 60;
        let mut work_secs: u64 = 25 * 60;
        let mut break_secs: u64 = 5 * 60;

        loop {
            tokio::select! {
                _ = sleep(Duration::from_secs(1)) => {}
                Some(cmd) = rx.recv() => {
                    match cmd {
                        PomodoroCmd::Start => {
                            if in_study || manual { continue; }
                            manual = true;
                            working = true;
                            paused = false;
                            work_secs = get_u64_setting(&pool, "pomodoro_work_mins", 25).await.max(1) * 60;
                            break_secs = get_u64_setting(&pool, "pomodoro_break_mins", 5).await.max(1) * 60;
                            remaining = work_secs;
                            let until = chrono::Utc::now() + chrono::Duration::seconds(remaining as i64);
                            persist_state(&pool, "work", until).await;
                            if get_bool_setting(&pool, "focus_mode_auto", true).await {
                                crate::notifier::mute::extend_quiet_until(&pool, until).await;
                            }
                        }
                        PomodoroCmd::Stop => {
                            if !in_study && !manual { continue; }
                            in_study = false;
                            manual = false;
                            paused = false;
                            persist_state(&pool, "off", chrono::Utc::now()).await;
                        }
                        PomodoroCmd::TogglePause => {
                            if !in_study && !manual { continue; }
                            paused = !paused;
                            let until = chrono::Utc::now() + chrono::Duration::seconds(remaining as i64);
                            persist_state(&pool, if paused { "paused" } else if working { "work" } else { "break" }, until).await;
                        }
                        PomodoroCmd::ExtendBreak(mins) => {
                            if !in_study && !manual { continue; }
                            // The "break is over" notification arrives when the loop
                            // has already switched to work, so "five more minutes"
                            // has to go back into a break rather than extend the
                            // current phase. Pressed during a break, it simply adds
                            // to what is left.
                            remaining = if working { mins * 60 } else { remaining + mins * 60 };
                            working = false;
                            let until = chrono::Utc::now() + chrono::Duration::seconds(remaining as i64);
                            persist_state(&pool, "break", until).await;
                        }
                        PomodoroCmd::Skip => {
                            if !in_study && !manual { continue; }
                            if working {
                                log_completed_work(&pool).await;
                            }
                            working = !working;
                            remaining = if working { work_secs } else { break_secs };
                            let until = chrono::Utc::now() + chrono::Duration::seconds(remaining as i64);
                            persist_state(&pool, if working { "work" } else { "break" }, until).await;
                            if working && get_bool_setting(&pool, "focus_mode_auto", true).await {
                                crate::notifier::mute::extend_quiet_until(&pool, until).await;
                            }
                        }
                    }
                    continue;
                }
            }

            let mode = work_mode.lock().unwrap().clone();
            if mode != WorkMode::Study {
                if in_study {
                    in_study = false;
                    if !manual {
                        paused = false;
                        persist_state(&pool, "off", chrono::Utc::now()).await;
                    }
                }
                if !manual { continue; }
            } else if !in_study && !manual {
                in_study = true;
                working = true;
                paused = false;
                // .max(1) guards against a 0 in the DB: otherwise remaining -= 1 underflows
                work_secs = get_u64_setting(&pool, "pomodoro_work_mins", 25).await.max(1) * 60;
                break_secs = get_u64_setting(&pool, "pomodoro_break_mins", 5).await.max(1) * 60;
                remaining = work_secs;
                let until = chrono::Utc::now() + chrono::Duration::seconds(remaining as i64);
                persist_state(&pool, "work", until).await;
                if get_bool_setting(&pool, "focus_mode_auto", true).await {
                    crate::notifier::mute::extend_quiet_until(&pool, until).await;
                }
                // Notification pause: the timer runs but stays silent. Checked only
                // at the moment of sending, so the DB is not hit every second.
                if !crate::notifier::mute::muted_now(&pool, &mode).await {
                    {
                        let lang = crate::i18n::current_lang(&pool).await;
                        let body = crate::i18n::tr_args("Помодоро запущено: {n} минут работы", lang, &[("n", (work_secs / 60).to_string())]);
                        send_pomodoro_notification(&app, &pool, "Study", &body, PomodoroMoment::WorkStarted).await;
                    }
                }
                continue;
            } else if !in_study && manual {
                // Study switched on over an already-running manual cycle: we treat it
                // as "in Study" for a consistent status, but the cycle continues
                // without a restart.
                in_study = true;
            }

            if paused {
                continue;
            }

            remaining -= 1;
            if remaining == 0 {
                let muted = crate::notifier::mute::muted_now(&pool, &mode).await;
                if working {
                    log_completed_work(&pool).await;
                    working = false;
                    remaining = break_secs;
                    if !muted {
                        let lang = crate::i18n::current_lang(&pool).await;
                        let body = crate::i18n::tr_args("Перерыв {n} минут — отдохни", lang, &[("n", (break_secs / 60).to_string())]);
                        send_pomodoro_notification(&app, &pool, "Study", &body, PomodoroMoment::BreakStarted).await;
                    }
                } else {
                    working = true;
                    remaining = work_secs;
                    if !muted {
                        let lang = crate::i18n::current_lang(&pool).await;
                        let body = crate::i18n::tr_args("Перерыв окончен: {n} минут работы", lang, &[("n", (work_secs / 60).to_string())]);
                        send_pomodoro_notification(&app, &pool, "Study", &body, PomodoroMoment::WorkStarted).await;
                    }
                }
                let until = chrono::Utc::now() + chrono::Duration::seconds(remaining as i64);
                persist_state(&pool, if working { "work" } else { "break" }, until).await;
                if working && get_bool_setting(&pool, "focus_mode_auto", true).await {
                    crate::notifier::mute::extend_quiet_until(&pool, until).await;
                }
            }
        }
    });

    tx
}

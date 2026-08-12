use chrono::Utc;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tokio::time::{interval, Duration};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum ActivityState {
    Active,
    Idle,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionStats {
    pub active_secs: u64,
    pub idle_secs: u64,
    pub session_start: String,
}

#[derive(Debug, Clone)]
pub struct ActivityTracker {
    pub state: Arc<Mutex<ActivityState>>,
    pub last_input: Arc<Mutex<chrono::DateTime<Utc>>>,
    pub session_start: chrono::DateTime<Utc>,
    pub active_secs: Arc<Mutex<u64>>,
    pub idle_secs: Arc<Mutex<u64>>,
}

impl ActivityTracker {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ActivityState::Active)),
            last_input: Arc::new(Mutex::new(Utc::now())),
            session_start: Utc::now(),
            active_secs: Arc::new(Mutex::new(0)),
            idle_secs: Arc::new(Mutex::new(0)),
        }
    }

    // Called from the frontend on mousemove/keydown
    pub fn record_input(&self) {
        let mut last = self.last_input.lock().unwrap();
        *last = Utc::now();
        let mut state = self.state.lock().unwrap();
        *state = ActivityState::Active;
    }

    pub fn get_stats(&self) -> SessionStats {
        SessionStats {
            active_secs: *self.active_secs.lock().unwrap(),
            idle_secs: *self.idle_secs.lock().unwrap(),
            session_start: self.session_start.to_rfc3339(),
        }
    }

    pub fn get_state(&self) -> ActivityState {
        self.state.lock().unwrap().clone()
    }
}

// The result of one tick of the idle state machine. Pure data — no DB, no
// notifications, no locks — so the logic can be covered by unit tests.
#[derive(Debug, Clone, PartialEq)]
pub struct IdleTick {
    pub state: ActivityState,
    pub idle_since: Option<chrono::DateTime<Utc>>,
    // Some(minutes) when this is an Idle->Active transition worth considering a notification for
    pub notify_return_mins: Option<i64>,
}

// The pure logic of one tick: from the previous state and the timing it derives
// the new state, when idleness began, and whether to notify about the return.
pub fn step_idle(
    prev_state: &ActivityState,
    idle_since: Option<chrono::DateTime<Utc>>,
    now: chrono::DateTime<Utc>,
    last_input: chrono::DateTime<Utc>,
    idle_threshold_secs: u64,
) -> IdleTick {
    let elapsed = (now - last_input).num_seconds().max(0) as u64;
    let new_state = if elapsed >= idle_threshold_secs {
        ActivityState::Idle
    } else {
        ActivityState::Active
    };

    let mut new_idle_since = idle_since;
    let mut notify_return_mins = None;

    if *prev_state == ActivityState::Active && new_state == ActivityState::Idle {
        new_idle_since = Some(last_input);
    }
    if *prev_state == ActivityState::Idle && new_state == ActivityState::Active {
        let away = new_idle_since.map(|t| (now - t).num_minutes()).unwrap_or(0);
        notify_return_mins = Some(away);
        new_idle_since = None;
    }

    IdleTick { state: new_state, idle_since: new_idle_since, notify_return_mins }
}

pub fn start_activity_loop(
    app: tauri::AppHandle,
    tracker: Arc<ActivityTracker>,
    pool: SqlitePool,
    idle_threshold_secs: u64,
    log_interval_secs: u64,
    work_mode: Arc<Mutex<crate::commands::settings::WorkMode>>,
    window_provider: Option<Arc<dyn super::window::WindowProvider>>,
) {
    tokio::spawn(async move {
        let mut tick = interval(Duration::from_secs(log_interval_secs));
        // A local prev_state: tracker.state is reset to Active immediately by
        // record_input, so an Idle->Active transition cannot be caught from it.
        let mut prev_state = ActivityState::Active;
        let mut idle_since: Option<chrono::DateTime<Utc>> = None;
        loop {
            tick.tick().await;

            let now = Utc::now();
            let last_input = *tracker.last_input.lock().unwrap();

            let step = step_idle(&prev_state, idle_since, now, last_input, idle_threshold_secs);
            let new_state = step.state.clone();
            idle_since = step.idle_since;

            {
                let mut state = tracker.state.lock().unwrap();
                *state = new_state.clone();
            }

            // An Idle->Active transition: notify about the return (except in Focus mode or while paused)
            if let Some(away_mins) = step.notify_return_mins {
                // Copy the mode into a local variable: a lock must not be held across .await
                let mode = work_mode.lock().unwrap().clone();
                if !crate::notifier::mute::muted_now(&pool, &mode).await {
                    notify_return(&app, &pool, away_mins).await;
                }
            }
            prev_state = new_state.clone();

            // Accumulate the statistics
            match new_state {
                ActivityState::Active => {
                    let mut secs = tracker.active_secs.lock().unwrap();
                    *secs += log_interval_secs;
                }
                ActivityState::Idle => {
                    let mut secs = tracker.idle_secs.lock().unwrap();
                    *secs += log_interval_secs;
                }
            }

            // Log to the DB on every tick. For an Active tick we record the class
            // of the focused window if a provider exists: a local socket, so the
            // cost is nil.
            let state_str = match new_state {
                ActivityState::Idle => "Idle",
                ActivityState::Active => "Active",
            };
            let window = match (&new_state, &window_provider) {
                (ActivityState::Active, Some(p)) => p.current_window(),
                _ => None,
            };

            // The domain is extracted only when the setting is explicitly on and
            // only for browsers. The title lives inside this function and goes no
            // further: either a domain or NULL reaches the DB. The setting is read
            // every tick rather than cached at startup so that turning it off takes
            // effect immediately, without restarting the app — for a privacy
            // checkbox that matters more than one saved query per minute.
            let domain = match &window {
                Some(w) if crate::monitor::domain::is_browser(&w.app) => {
                    if crate::commands::settings::get_bool_setting(&pool, "track_domains", false).await {
                        crate::monitor::domain::domain_from_title(&w.title)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            let focused_app = window.map(|w| w.app);

            let result = sqlx::query(
                "INSERT INTO activity_log (timestamp, state, app_focused, input_events, duration_secs, app, domain)
                 VALUES (?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(now.to_rfc3339())
            .bind(state_str)
            .bind(true)
            .bind(0i32)
            .bind(log_interval_secs as i64)
            .bind(focused_app)
            .bind(domain)
            .execute(&pool)
            .await;

            let _ = result;
        }
    });
}

// How long after one "welcome back" the next one stays silent.
pub const RETURN_COOLDOWN_MINS: i64 = 90;

// How much of a task title a notification body carries.
pub const TASK_TITLE_MAX: usize = 40;

// Cuts by characters, not bytes: a title is free text and cutting mid-codepoint
// would panic on the first Cyrillic one.
pub fn ellipsize(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

// Whether the same reminder is still fresh. Pure logic, split out from the query
// so the boundary can be tested without a DB.
pub fn is_within_cooldown(
    last_sent: Option<chrono::DateTime<Utc>>,
    now: chrono::DateTime<Utc>,
    cooldown_mins: i64,
) -> bool {
    match last_sent {
        // A clock that jumped backwards would otherwise mute the reminder for as
        // long as the jump lasted, so only a forward distance counts.
        Some(t) => (now - t).num_minutes() < cooldown_mins && now >= t,
        None => false,
    }
}

// The last activity_return entry is read from notification_log — the sending
// history is already there, so this needs neither a new table nor a setting.
async fn returned_recently(pool: &SqlitePool) -> bool {
    let last: Option<String> = sqlx::query_scalar(
        "SELECT created_at FROM notification_log
         WHERE kind = 'activity_return'
         ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let parsed = last
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|t| t.with_timezone(&Utc));

    is_within_cooldown(parsed, Utc::now(), RETURN_COOLDOWN_MINS)
}

// The notification shown on returning from idleness: the top task is the one
// with the nearest deadline, then the highest priority. Nothing is sent if the
// user was away only briefly.
async fn notify_return(app: &tauri::AppHandle, pool: &SqlitePool, away_mins: i64) {
    let min_mins = crate::commands::settings::get_u64_setting(pool, "idle_notify_min_mins", 10).await;
    if away_mins < min_mins as i64 {
        return;
    }
    // Every trip away from the desk crosses the threshold, so without a cooldown
    // a normal working day produces one of these every quarter of an hour, each
    // repeating the same task title. The reminder is only useful the first time.
    if returned_recently(pool).await {
        return;
    }

    let context_on = crate::commands::settings::get_setting(pool, "context_notifications")
        .await
        .as_deref()
        != Some("false");

    // A contextual trigger: the user was away for a long time and there is a task
    // in progress — any InProgress one, not necessarily the top by deadline.
    let in_progress = if context_on && away_mins >= CONTEXT_RETURN_MINS {
        nearest_task(pool, &["InProgress"]).await
    } else {
        None
    };

    let lang = crate::i18n::current_lang(pool).await;
    let mins = away_mins.to_string();
    // Task titles are free text and routinely run to a sentence or more. Pasted
    // whole into a notification body they push the actual message off screen and
    // read as a leak of something internal rather than as a reminder.
    let in_progress = in_progress.as_deref().map(|t| ellipsize(t, TASK_TITLE_MAX));
    let body = match in_progress {
        Some(title) => crate::i18n::tr_args(
            "Вы отсутствовали {n} мин. Продолжим задачу «{task}» или сделаем перерыв?",
            lang, &[("n", mins), ("task", title)]),
        None => match nearest_task(pool, &["Todo", "InProgress"]).await {
            Some(title) => crate::i18n::tr_args(
                "Вы отсутствовали {n} мин. Ближайшая задача: {task}",
                lang, &[("n", mins), ("task", ellipsize(&title, TASK_TITLE_MAX))]),
            None => crate::i18n::tr_args(
                "Вы отсутствовали {n} мин. С возвращением!", lang, &[("n", mins)]),
        },
    };

    crate::notifier::scheduler::send_notification(app, pool, "activity_return", "Kriptag", &body).await;
}

// The nearest visible task (by deadline, then by priority) in one of the given
// statuses. The statuses are fixed strings from the code, not user input.
pub async fn nearest_task(pool: &SqlitePool, statuses: &[&str]) -> Option<String> {
    use sqlx::Row;
    let placeholders = vec!["?"; statuses.len()].join(", ");
    let sql = format!(
        "SELECT title FROM tasks
         WHERE status IN ({placeholders}) AND hidden = 0 AND deleted_at IS NULL
         ORDER BY deadline IS NULL, deadline ASC,
                  CASE priority
                      WHEN 'Critical' THEN 0
                      WHEN 'High' THEN 1
                      WHEN 'Medium' THEN 2
                      ELSE 3
                  END
         LIMIT 1"
    );
    let mut query = sqlx::query(&sql);
    for s in statuses {
        query = query.bind(*s);
    }
    query
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .map(|row| row.get("title"))
}

// The threshold for a "long" absence before the contextual InProgress message.
const CONTEXT_RETURN_MINS: i64 = 40;

#[derive(serde::Serialize)]
pub struct ActivityDay {
    pub date: String,
    pub minutes: i64,
}

#[derive(serde::Serialize)]
pub struct TaskCompletion {
    pub date: String,
    pub completed: i64,
}

#[derive(serde::Serialize)]
pub struct CategoryCount {
    pub category: String,
    pub count: i64,
}

#[derive(serde::Serialize)]
pub struct ActiveIdleRatio {
    pub today_active: i64,
    pub today_idle: i64,
    pub week_active: i64,
    pub week_idle: i64,
}

// Idle time inside a planned time block. A block is a plan, while monitoring
// knows how much time was really worked: matching the two gives an honest
// plan-versus-actual instead of a schedule mistaken for a fact.
#[derive(Debug, serde::Serialize, PartialEq)]
pub struct BlockIdle {
    pub task_id: String,
    pub task_title: String,
    pub planned_mins: i64,
    pub idle_mins: i64,
    pub active_mins: i64,
}

// The intersection of two half-open intervals [a_start, a_end) and
// [b_start, b_end), in seconds. Extracted as a pure function because it is the
// only arithmetic in the whole feature and precisely where boundary mistakes are
// easy (a monitoring tick partly falls inside a block, starts before it, or ends
// after it).
pub fn overlap_secs(a_start: i64, a_end: i64, b_start: i64, b_end: i64) -> i64 {
    let start = a_start.max(b_start);
    let end = a_end.min(b_end);
    (end - start).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;

    // The boundaries are the one place where this feature can quietly lie: count
    // idleness that never happened, or lose idleness that did.
    #[test]
    fn overlap_handles_all_boundary_cases() {
        // The tick lies entirely inside the block
        assert_eq!(overlap_secs(100, 160, 0, 600), 60);
        // The block lies entirely inside the tick (a long idle tick swallows a short block)
        assert_eq!(overlap_secs(0, 600, 100, 160), 60);
        // Partial overlap on the left and on the right
        assert_eq!(overlap_secs(0, 120, 60, 600), 60);
        assert_eq!(overlap_secs(540, 660, 0, 600), 60);
        // Exactly coinciding boundaries
        assert_eq!(overlap_secs(0, 600, 0, 600), 600);

        // No overlap yields zero, NOT a negative number: otherwise summing over
        // ticks would subtract unrelated time.
        assert_eq!(overlap_secs(0, 60, 600, 660), 0);
        assert_eq!(overlap_secs(600, 660, 0, 60), 0);
        // Touching at a point: the intervals are half-open, so zero, not 1 second
        assert_eq!(overlap_secs(0, 600, 600, 660), 0);
        assert_eq!(overlap_secs(600, 660, 0, 600), 0);
        // An empty interval
        assert_eq!(overlap_secs(300, 300, 0, 600), 0);
    }

    fn at(now: chrono::DateTime<Utc>, secs_ago: i64) -> chrono::DateTime<Utc> {
        now - ChronoDuration::seconds(secs_ago)
    }

    #[test]
    fn active_stays_active_below_threshold() {
        let now = Utc::now();
        let step = step_idle(&ActivityState::Active, None, now, at(now, 100), 300);
        assert_eq!(step.state, ActivityState::Active);
        assert_eq!(step.idle_since, None);
        assert_eq!(step.notify_return_mins, None);
    }

    #[test]
    fn threshold_is_inclusive_boundary() {
        let now = Utc::now();
        // exactly at the threshold -> Idle (>=)
        let step = step_idle(&ActivityState::Active, None, now, at(now, 300), 300);
        assert_eq!(step.state, ActivityState::Idle);
        // one second below the threshold -> still Active
        let step = step_idle(&ActivityState::Active, None, now, at(now, 299), 300);
        assert_eq!(step.state, ActivityState::Active);
    }

    #[test]
    fn active_to_idle_records_idle_since_and_does_not_notify() {
        let now = Utc::now();
        let last_input = at(now, 400);
        let step = step_idle(&ActivityState::Active, None, now, last_input, 300);
        assert_eq!(step.state, ActivityState::Idle);
        assert_eq!(step.idle_since, Some(last_input));
        assert_eq!(step.notify_return_mins, None);
    }

    #[test]
    fn idle_stays_idle_keeps_idle_since() {
        let now = Utc::now();
        let idle_since = at(now, 600);
        let step = step_idle(&ActivityState::Idle, Some(idle_since), now, at(now, 500), 300);
        assert_eq!(step.state, ActivityState::Idle);
        assert_eq!(step.idle_since, Some(idle_since));
        assert_eq!(step.notify_return_mins, None);
    }

    #[test]
    fn idle_to_active_notifies_with_away_minutes_and_clears_idle_since() {
        let now = Utc::now();
        // went idle 30 minutes ago and has just returned (last_input = now)
        let idle_since = at(now, 30 * 60);
        let step = step_idle(&ActivityState::Idle, Some(idle_since), now, now, 300);
        assert_eq!(step.state, ActivityState::Active);
        assert_eq!(step.idle_since, None);
        assert_eq!(step.notify_return_mins, Some(30));
    }

    #[test]
    fn idle_to_active_without_idle_since_reports_zero() {
        let now = Utc::now();
        // idle_since is unset (an edge case): away = 0, but a notification is still considered
        let step = step_idle(&ActivityState::Idle, None, now, now, 300);
        assert_eq!(step.notify_return_mins, Some(0));
    }

    async fn test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./src/db/migrations").run(&pool).await.unwrap();
        pool
    }

    async fn insert_task(pool: &sqlx::SqlitePool, title: &str, status: &str, deadline: Option<&str>) {
        sqlx::query(
            "INSERT INTO tasks (id, title, status, priority, category, deadline, recurrence, tags, hidden, created_at, updated_at)
             VALUES (?, ?, ?, 'Medium', 'Work', ?, 'None', '[]', 0, '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')")
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(title).bind(status).bind(deadline)
            .execute(pool).await.unwrap();
    }

    // Regression: the contextual trigger must find a task in progress even when
    // the top task by deadline is Todo (only the top task's status used to be checked).
    #[tokio::test]
    async fn nearest_task_finds_in_progress_behind_todo_with_nearer_deadline() {
        let pool = test_pool().await;
        insert_task(&pool, "срочная todo", "Todo", Some("2026-07-15T00:00:00+00:00")).await;
        insert_task(&pool, "в работе без дедлайна", "InProgress", None).await;

        // the top task overall is a Todo with the nearest deadline
        assert_eq!(
            nearest_task(&pool, &["Todo", "InProgress"]).await.as_deref(),
            Some("срочная todo")
        );
        // but the InProgress task is found by a separate query
        assert_eq!(
            nearest_task(&pool, &["InProgress"]).await.as_deref(),
            Some("в работе без дедлайна")
        );
    }

    #[tokio::test]
    async fn nearest_task_none_when_no_matching_status() {
        let pool = test_pool().await;
        insert_task(&pool, "выполнена", "Done", None).await;
        assert_eq!(nearest_task(&pool, &["InProgress"]).await, None);
    }

    // Every trip away from the desk past the threshold produced a notification:
    // four of them in 45 minutes, all repeating the same task title.
    #[test]
    fn a_second_welcome_back_stays_silent_within_the_cooldown() {
        let now = Utc::now();
        let sent = |mins| Some(now - chrono::Duration::minutes(mins));

        assert!(is_within_cooldown(sent(15), now, RETURN_COOLDOWN_MINS),
            "через 15 минут после прошлого — молчим");
        assert!(is_within_cooldown(sent(RETURN_COOLDOWN_MINS - 1), now, RETURN_COOLDOWN_MINS),
            "за минуту до конца окна — ещё молчим");
        assert!(!is_within_cooldown(sent(RETURN_COOLDOWN_MINS), now, RETURN_COOLDOWN_MINS),
            "ровно на границе окно закончилось");
        assert!(!is_within_cooldown(sent(240), now, RETURN_COOLDOWN_MINS),
            "через четыре часа — снова уведомляем");
        assert!(!is_within_cooldown(None, now, RETURN_COOLDOWN_MINS),
            "первое уведомление за всё время");
    }

    #[test]
    fn a_clock_jumped_backwards_does_not_mute_the_reminder() {
        // A timestamp from the future (a clock change, a hand-edited row) would
        // otherwise mute the reminder for the whole length of the jump.
        let now = Utc::now();
        let future = Some(now + chrono::Duration::hours(5));
        assert!(!is_within_cooldown(future, now, RETURN_COOLDOWN_MINS));
    }

    #[test]
    fn a_long_task_title_is_cut_and_multibyte_safe() {
        // A real title from the user's DB: pasted whole it pushed the message
        // itself out of the notification.
        let title = "SMART-формат задачи: чёткая цель, измеримый результат, срок. \
                     Только результат, без пояснений";
        let cut = ellipsize(title, TASK_TITLE_MAX);
        assert_eq!(cut.chars().count(), TASK_TITLE_MAX + 1, "40 символов плюс многоточие");
        assert!(cut.ends_with('…'));

        // A short title is left alone, with no ellipsis appended.
        assert_eq!(ellipsize("Купить билеты", TASK_TITLE_MAX), "Купить билеты");
        // Exactly at the boundary — still no ellipsis.
        let exact: String = "я".repeat(TASK_TITLE_MAX);
        assert_eq!(ellipsize(&exact, TASK_TITLE_MAX), exact);
    }
}
use std::sync::{Arc, Mutex};
use sqlx::{SqlitePool, Row};
use tokio::time::{sleep, Duration};
use crate::commands::settings::{WorkMode, get_setting, set_setting};
use crate::notifier::scheduler::send_notification;

const CHECK_EVERY_SECS: u64 = 600; // once every 10 minutes
const OVERDUE_THRESHOLD: i64 = 3;
const MISSED_DAYS_THRESHOLD: u32 = 3;

// Contextual triggers: "overdue tasks have piled up". Notified at most once a
// day; the cooldown lives in settings (last_overdue_notify = YYYY-MM-DD) so it
// survives restarts and does not start spamming again.
pub fn start_triggers(app: tauri::AppHandle, pool: SqlitePool, work_mode: Arc<Mutex<WorkMode>>) {
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(CHECK_EVERY_SECS)).await;

            if get_setting(&pool, "context_notifications").await.as_deref() == Some("false") {
                continue;
            }
            let mode = work_mode.lock().unwrap().clone();
            if crate::notifier::mute::muted_now(&pool, &mode).await {
                continue;
            }

            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            if get_setting(&pool, "last_overdue_notify").await.as_deref() == Some(today.as_str()) {
                continue;
            }

            let count = overdue_count(&pool, &chrono::Utc::now().to_rfc3339()).await;
            if count >= OVERDUE_THRESHOLD {
                send_notification(
                    &app,
                    &pool,
                    "overdue",
                    "Kriptag",
                    &format!("Просроченных задач уже {}. Загляни в список и разбери завалы.", count),
                ).await;
                let _ = set_setting(&pool, "last_overdue_notify", &today).await;
            }

            // A run of "missed" days: consecutive past days that had tasks due and
            // still have them open. It has its own daily cooldown.
            if get_setting(&pool, "last_missed_notify").await.as_deref() != Some(today.as_str()) {
                let missed = missed_days(&pool).await;
                let streak =
                    consecutive_missed_days(chrono::Utc::now().date_naive(), &missed);
                if streak >= MISSED_DAYS_THRESHOLD {
                    send_notification(
                        &app,
                        &pool,
                        "missed_days",
                        "Kriptag",
                        &format!("Уже {} дн. подряд задачи остаются несделанными. Может, пересмотреть план?", streak),
                    ).await;
                    let _ = set_setting(&pool, "last_missed_notify", &today).await;
                }
            }
        }
    });
}

// Past days on which still-open tasks had their deadlines.
pub async fn missed_days(pool: &SqlitePool) -> std::collections::HashSet<chrono::NaiveDate> {
    let rows = sqlx::query(
        "SELECT DISTINCT date(deadline) as d FROM tasks
         WHERE deadline IS NOT NULL AND date(deadline) < date('now')
           AND status NOT IN ('Done', 'Archived') AND hidden = 0 AND deleted_at IS NULL",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    rows.iter()
        .filter_map(|r| r.get::<String, _>("d").parse().ok())
        .collect()
}

// The length of the run of missed days ending yesterday. A day with no deadlines
// (or where everything is closed) breaks the run. A pure function.
pub fn consecutive_missed_days(
    today: chrono::NaiveDate,
    missed: &std::collections::HashSet<chrono::NaiveDate>,
) -> u32 {
    let mut streak = 0;
    let mut day = today.pred_opt().unwrap();
    while missed.contains(&day) {
        streak += 1;
        day = day.pred_opt().unwrap();
    }
    streak
}

// How many tasks are overdue as of `now` (RFC3339). Hidden and closed ones do not count.
pub async fn overdue_count(pool: &SqlitePool, now: &str) -> i64 {
    sqlx::query(
        "SELECT COUNT(*) as c FROM tasks
         WHERE deadline IS NOT NULL AND deadline < ?
           AND status NOT IN ('Done', 'Archived') AND hidden = 0 AND deleted_at IS NULL",
    )
    .bind(now)
    .fetch_one(pool)
    .await
    .map(|r| r.get("c"))
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./src/db/migrations").run(&pool).await.unwrap();
        pool
    }

    async fn task(pool: &SqlitePool, id: &str, status: &str, deadline: Option<&str>, hidden: bool) {
        sqlx::query(
            "INSERT INTO tasks (id, title, status, priority, category, tags, recurrence, hidden, deadline, created_at, updated_at)
             VALUES (?, 't', ?, 'Medium', 'Work', '[]', 'None', ?, ?, '2026-07-01T00:00:00+00:00', '2026-07-01T00:00:00+00:00')")
            .bind(id).bind(status).bind(hidden).bind(deadline)
            .execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn counts_only_open_visible_overdue() {
        let pool = test_pool().await;
        let now = "2026-07-10T12:00:00+00:00";
        task(&pool, "a", "Todo", Some("2026-07-09T00:00:00+00:00"), false).await;       // overdue
        task(&pool, "b", "InProgress", Some("2026-07-08T00:00:00+00:00"), false).await; // overdue
        task(&pool, "c", "Done", Some("2026-07-01T00:00:00+00:00"), false).await;       // closed
        task(&pool, "d", "Todo", Some("2026-07-09T00:00:00+00:00"), true).await;        // hidden
        task(&pool, "e", "Todo", Some("2026-07-11T00:00:00+00:00"), false).await;       // not due yet
        task(&pool, "f", "Todo", None, false).await;                                    // no deadline

        assert_eq!(overdue_count(&pool, now).await, 2);
    }

    #[tokio::test]
    async fn empty_db_is_zero() {
        let pool = test_pool().await;
        assert_eq!(overdue_count(&pool, "2026-07-10T12:00:00+00:00").await, 0);
    }

    fn d(s: &str) -> chrono::NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn missed_streak_counts_back_from_yesterday() {
        let missed: std::collections::HashSet<_> =
            [d("2026-07-09"), d("2026-07-08"), d("2026-07-07")].into();
        assert_eq!(consecutive_missed_days(d("2026-07-10"), &missed), 3);
    }

    #[test]
    fn clean_day_breaks_streak() {
        // the 8th is clean: the run is yesterday only
        let missed: std::collections::HashSet<_> = [d("2026-07-09"), d("2026-07-07")].into();
        assert_eq!(consecutive_missed_days(d("2026-07-10"), &missed), 1);
    }

    #[test]
    fn no_miss_yesterday_is_zero() {
        let missed: std::collections::HashSet<_> = [d("2026-07-05")].into();
        assert_eq!(consecutive_missed_days(d("2026-07-10"), &missed), 0);
        assert_eq!(consecutive_missed_days(d("2026-07-10"), &Default::default()), 0);
    }

    #[tokio::test]
    async fn missed_days_only_past_open_visible() {
        let pool = test_pool().await;
        let yesterday = (chrono::Utc::now() - chrono::Duration::days(1))
            .to_rfc3339();
        let tomorrow = (chrono::Utc::now() + chrono::Duration::days(1))
            .to_rfc3339();
        task(&pool, "a", "Todo", Some(&yesterday), false).await;  // a miss
        task(&pool, "b", "Done", Some(&yesterday), false).await;  // closed, so not a miss
        task(&pool, "c", "Todo", Some(&tomorrow), false).await;   // in the future, so not a miss
        task(&pool, "d", "Todo", Some(&yesterday), true).await;   // hidden

        let missed = missed_days(&pool).await;
        assert_eq!(missed.len(), 1);
        assert!(missed.contains(&chrono::Utc::now().date_naive().pred_opt().unwrap()));
    }
}

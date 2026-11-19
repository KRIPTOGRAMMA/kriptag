use tauri::State;
use std::sync::Arc;
use sqlx::{SqlitePool, Row};
use crate::error::AppResult;
use crate::monitor::activity::{ActivityTracker, SessionStats, ActivityState, ActivityDay, TaskCompletion, CategoryCount, ActiveIdleRatio, BlockIdle};

#[tauri::command]
pub fn record_input(tracker: State<'_, Arc<ActivityTracker>>) {
    tracker.record_input();
}

#[tauri::command]
pub fn get_session_stats(tracker: State<'_, Arc<ActivityTracker>>) -> SessionStats {
    tracker.get_stats()
}

#[tauri::command]
pub fn get_activity_state(tracker: State<'_, Arc<ActivityTracker>>) -> String {
    match tracker.get_state() {
        ActivityState::Active => "Active".into(),
        ActivityState::Idle => "Idle".into(),
    }
}

#[tauri::command]
pub async fn get_activity_by_day(pool: State<'_, SqlitePool>) -> AppResult<Vec<ActivityDay>> {
    get_activity_by_day_impl(pool.inner()).await
}

pub async fn get_activity_by_day_impl(pool: &SqlitePool) -> AppResult<Vec<ActivityDay>> {
    // Bucketed by the middle of each tick's period, like get_hourly_activity: the
    // stored timestamp is the period's END, so a tick measured just before local
    // midnight would otherwise be credited to the following day.
    //
    // Local days, like get_task_completions_by_day: timestamps are stored in UTC,
    // but the user's day is local, and this date is shown to them verbatim as the
    // bar's label. Grouping by the UTC date put evening or early-morning work on
    // the neighbouring square, so the activity calendar and the completion
    // calendar disagreed about the same instant.
    let rows = sqlx::query(
        "SELECT date(datetime(timestamp, (-duration_secs / 2) || ' seconds'), 'localtime') as date,
                SUM(duration_secs) / 60 as minutes
         FROM activity_log
         WHERE state = 'Active'
         GROUP BY date(datetime(timestamp, (-duration_secs / 2) || ' seconds'), 'localtime')"
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().map(|row| ActivityDay {
        date: row.get("date"),
        minutes: row.get("minutes"),
    }).collect())
}

#[tauri::command]
pub async fn get_task_completions_by_day(pool: State<'_, SqlitePool>) -> AppResult<Vec<TaskCompletion>> {
    get_task_completions_by_day_impl(pool.inner()).await
}

pub async fn get_task_completions_by_day_impl(pool: &SqlitePool) -> AppResult<Vec<TaskCompletion>> {
    // Local days: completed_at is stored in UTC, but a "day" for the user is
    // local (otherwise evening tasks drift into tomorrow). The calendar squares
    // and get_completions_for_day group things the same way.
    let rows = sqlx::query(
      "SELECT date(completed_at, 'localtime') as date, COUNT(*) as completed
       FROM tasks
       WHERE completed_at IS NOT NULL
       GROUP BY date(completed_at, 'localtime')"
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().map(|row| TaskCompletion {
      date: row.get("date"),
      completed: row.get("completed"),
    }).collect())
}
#[tauri::command]
pub async fn get_category_distribution(pool: State<'_, SqlitePool>) -> AppResult<Vec<CategoryCount>> {
    get_category_distribution_impl(pool.inner()).await
}

pub async fn get_category_distribution_impl(pool: &SqlitePool) -> AppResult<Vec<CategoryCount>> {
    let rows = sqlx::query(
        "SELECT category, COUNT(*) as count
         FROM tasks
         WHERE completed_at IS NOT NULL
         GROUP BY category"
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().map(|row| CategoryCount {
        category: row.get("category"),
        count: row.get("count"),
    }).collect())
}

#[tauri::command]
pub async fn get_active_idle_ratio(pool: State<'_, SqlitePool>) -> AppResult<ActiveIdleRatio> {
    get_active_idle_ratio_impl(pool.inner()).await
}

pub async fn get_active_idle_ratio_impl(pool: &SqlitePool) -> AppResult<ActiveIdleRatio> {
    // "Today" and "this week" are local spans: at 02:00 in UTC+5 a UTC-based
    // "today" still reports yesterday's figures, which is precisely when the user
    // is looking at a fresh day and seeing stale numbers.
    let (today_active, today_idle) =
        state_sums(pool, "date(timestamp, 'localtime') = date('now', 'localtime')").await?;
    let (week_active, week_idle) = state_sums(
        pool,
        "date(timestamp, 'localtime') >= date('now', 'localtime', '-6 days')",
    )
    .await?;
    Ok(ActiveIdleRatio { today_active, today_idle, week_active, week_idle })
}

// Idle time inside a day's planned time blocks.
//
// A time block is a plan: "two hours on task X starting at 14:00". How much time
// was really worked is known to monitoring (activity_log). Intersecting the two
// gives an honest plan-versus-actual: before this, time blocks counted as done
// simply because their hour had arrived, whether or not the user was at the
// computer at all.
//
// Computed on the SQL side without window functions: there are few monitoring
// ticks per day (one per log_interval_secs, a minute by default) and even fewer
// blocks, so a plain join is cheaper than any optimization.
#[tauri::command]
pub async fn get_block_idle(pool: State<'_, SqlitePool>, date: String) -> AppResult<Vec<BlockIdle>> {
    get_block_idle_impl(pool.inner(), &date).await
}

pub async fn get_block_idle_impl(pool: &SqlitePool, date: &str) -> AppResult<Vec<BlockIdle>> {
    // This day's blocks. scheduled_mins may be NULL, so we default to an hour —
    // the same fallback the rest of the time-block code uses.
    let blocks = sqlx::query(
        "SELECT id, title, scheduled_at, COALESCE(scheduled_mins, 60) AS mins
         FROM tasks
         WHERE deleted_at IS NULL AND scheduled_at IS NOT NULL
           AND date(scheduled_at, 'localtime') = date(?)
         ORDER BY scheduled_at"
    )
    .bind(date)
    .fetch_all(pool)
    .await?;

    let ticks = sqlx::query(
        "SELECT timestamp, duration_secs, state FROM activity_log
         WHERE date(timestamp, 'localtime') = date(?)"
    )
    .bind(date)
    .fetch_all(pool)
    .await?;

    // Parse the ticks once rather than inside the loop over blocks.
    //
    // A row's timestamp is the END of the period it measured (see the INSERT in
    // monitor::activity), so the interval is [timestamp - duration, timestamp).
    let parsed: Vec<(i64, i64, bool)> = ticks.iter().filter_map(|r| {
        let ts: String = r.get("timestamp");
        let end = parse_ts(&ts)?;
        let dur: i64 = r.get("duration_secs");
        let state: String = r.get("state");
        Some((end - dur, end, state == "Idle"))
    }).collect();

    let mut out = Vec::new();
    for b in &blocks {
        let sched: String = b.get("scheduled_at");
        let Some(bs) = parse_ts(&sched) else { continue };
        let mins: i64 = b.get("mins");
        let be = bs + mins * 60;

        let (mut idle, mut active) = (0i64, 0i64);
        for &(ts, te, is_idle) in &parsed {
            let secs = crate::monitor::activity::overlap_secs(ts, te, bs, be);
            if secs == 0 { continue; }
            if is_idle { idle += secs; } else { active += secs; }
        }

        out.push(BlockIdle {
            task_id: b.get("id"),
            task_title: b.get("title"),
            planned_mins: mins,
            // Rounding down: reporting "idle for 9 minutes" at 9:59 is more
            // honest than rounding up to 10 and overstating the accusation.
            idle_mins: idle / 60,
            active_mins: active / 60,
        });
    }
    Ok(out)
}

// RFC3339 from the DB into unix seconds. An unparseable timestamp is skipped
// rather than failing the whole digest: one broken row must not cost the user a
// day of statistics.
fn parse_ts(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp())
}

async fn state_sums(pool: &SqlitePool, window: &str) -> AppResult<(i64, i64)> {
    let sql = format!(
        "SELECT state, SUM(duration_secs) as secs FROM activity_log WHERE {} GROUP BY state",
        window
    );
    let rows = sqlx::query(&sql).fetch_all(pool).await?;

    let (mut active, mut idle) = (0i64, 0i64);
    for row in &rows {
        let state: String = row.get("state");
        let secs: i64 = row.get("secs");
        match state.as_str() {
            "Active" => active = secs,
            "Idle" => idle = secs,
            _ => {}
        }
    }
    Ok((active, idle))
}

// ===== Per-application tracking =====

#[derive(Debug, serde::Serialize, PartialEq)]
pub struct AppMinutes {
    pub app: String,
    pub minutes: i64,
}

#[derive(Debug, serde::Serialize, PartialEq)]
pub struct DomainMinutes {
    pub domain: String,
    pub minutes: i64,
}

#[derive(Debug, serde::Serialize, PartialEq)]
pub struct CategoryMinutes {
    pub category: String,
    pub minutes: i64,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct CategoryRule {
    pub pattern: String,
    pub category: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct AppLimit {
    pub category: String,
    pub daily_mins: i64, // 0 or no rule means no limit
}

const KNOWN_CATEGORIES: [&str; 5] = ["Work", "Study", "Home", "Health", "Other"];

// Whether a category is one the dashboard knows. Exposed as a function rather than
// the constant itself so callers cannot accumulate their own copies of the list.
pub fn is_known_category(category: &str) -> bool {
    KNOWN_CATEGORIES.contains(&category)
}

// Time limits per app category: JSON in settings under the app_limits key,
// [{"category":"Other","daily_mins":60}, ...]. Junk or an empty string means no limits.
pub fn parse_app_limits(json: &str) -> Vec<AppLimit> {
    serde_json::from_str(json).unwrap_or_default()
}

// App categorization rules: JSON in settings under the app_category_rules key,
// [{"pattern":"kitty","category":"Work"}, ...]. Junk or an empty string simply
// means there are no rules.
pub fn parse_category_rules(json: &str) -> Vec<CategoryRule> {
    serde_json::from_str(json).unwrap_or_default()
}

// A glob with '*' (any substring), case-insensitive. Without '*' it is an exact match.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p = pattern.trim().to_lowercase();
    let t = text.to_lowercase();
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == t;
    }
    let mut pos = 0usize;
    let last = parts.len() - 1;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !t.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if i == last {
            return t.len() >= pos + part.len() && t[pos..].ends_with(part);
        } else {
            match t[pos..].find(part) {
                Some(idx) => pos += idx + part.len(),
                None => return false,
            }
        }
    }
    true
}

// The first matching rule wins; no match or an unknown category yields "Other"
// (the dashboard only knows the 5 palette categories).
pub fn categorize_app(app: &str, rules: &[CategoryRule]) -> String {
    for rule in rules {
        if glob_match(&rule.pattern, app) && KNOWN_CATEGORIES.contains(&rule.category.as_str()) {
            return rule.category.clone();
        }
    }
    "Other".into()
}

// Apps with no matching rule, ordered by time spent — the input for AI
// classification.
//
// Only unmatched apps are offered: anything already covered by a rule is the
// user's own decision, and re-proposing it would risk overwriting what they
// configured by hand. Ordering by minutes matters because the list is capped:
// the apps worth categorizing first are the ones actually eating the time,
// while a program opened once for a minute can stay in "Other" indefinitely.
pub async fn uncategorized_apps_impl(
    pool: &SqlitePool,
    days: i64,
    limit: usize,
) -> AppResult<Vec<AppMinutes>> {
    let rules_json = crate::commands::settings::get_setting(pool, "app_category_rules")
        .await
        .unwrap_or_default();
    let rules = parse_category_rules(&rules_json);

    let all = app_minutes_since(pool, days).await?;
    Ok(all
        .into_iter()
        .filter(|a| categorize_app(&a.app, &rules) == "Other")
        .take(limit)
        .collect())
}

#[tauri::command]
pub async fn get_uncategorized_apps(
    pool: State<'_, SqlitePool>,
    days: i64,
) -> AppResult<Vec<AppMinutes>> {
    uncategorized_apps_impl(pool.inner(), days, 15).await
}

async fn app_minutes_since(pool: &SqlitePool, days: i64) -> AppResult<Vec<AppMinutes>> {
    let since = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
    let rows = sqlx::query(
        "SELECT app, SUM(duration_secs) / 60 as minutes
         FROM activity_log
         WHERE state = 'Active' AND app IS NOT NULL AND timestamp >= ?
         GROUP BY app
         ORDER BY minutes DESC",
    )
    .bind(&since)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| AppMinutes { app: row.get("app"), minutes: row.get("minutes") })
        .collect())
}

#[tauri::command]
pub async fn get_app_usage(pool: State<'_, SqlitePool>, days: i64) -> AppResult<Vec<AppMinutes>> {
    get_app_usage_impl(pool.inner(), days).await
}

// The top 10 apps by active minutes over the last N days.
pub async fn get_app_usage_impl(pool: &SqlitePool, days: i64) -> AppResult<Vec<AppMinutes>> {
    let mut apps = app_minutes_since(pool, days.max(1)).await?;
    apps.truncate(10);
    Ok(apps)
}

// Time per site. An empty result is a normal state rather than an error: while
// track_domains is off (the default) the domain column is empty on every row,
// and the UI shows an explanation instead of a blank chart.
#[tauri::command]
pub async fn get_domain_usage(pool: State<'_, SqlitePool>, days: i64) -> AppResult<Vec<DomainMinutes>> {
    get_domain_usage_impl(pool.inner(), days).await
}

pub async fn get_domain_usage_impl(pool: &SqlitePool, days: i64) -> AppResult<Vec<DomainMinutes>> {
    let rows = sqlx::query(
        "SELECT domain, SUM(duration_secs) / 60 AS minutes
         FROM activity_log
         WHERE state = 'Active' AND domain IS NOT NULL AND domain != ''
           AND timestamp >= datetime('now', ?)
         GROUP BY domain
         HAVING minutes > 0
         ORDER BY minutes DESC
         LIMIT 10"
    )
    .bind(format!("-{} days", days.max(1)))
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().map(|r| DomainMinutes {
        domain: r.get("domain"),
        minutes: r.get("minutes"),
    }).collect())
}

// Forget all collected domain statistics. This belongs next to the checkbox:
// turning tracking off stops collection but does not by itself remove what has
// already accumulated — and a user unticking a privacy checkbox usually wants
// exactly that. A separate explicit button rather than a side effect of the
// toggle: silently deleting a user's data is a surprise too.
#[tauri::command]
pub async fn clear_domain_history(pool: State<'_, SqlitePool>) -> AppResult<u64> {
    clear_domain_history_impl(pool.inner()).await
}

pub async fn clear_domain_history_impl(pool: &SqlitePool) -> AppResult<u64> {
    let r = sqlx::query("UPDATE activity_log SET domain = NULL WHERE domain IS NOT NULL")
        .execute(pool)
        .await?;
    Ok(r.rows_affected())
}

#[tauri::command]
pub async fn get_app_category_time(
    pool: State<'_, SqlitePool>,
    days: i64,
) -> AppResult<Vec<CategoryMinutes>> {
    get_app_category_time_impl(pool.inner(), days).await
}

// Active minutes per category: apps from the log are run through the rules.
pub async fn get_app_category_time_impl(
    pool: &SqlitePool,
    days: i64,
) -> AppResult<Vec<CategoryMinutes>> {
    let rules_json = crate::commands::settings::get_setting(pool, "app_category_rules")
        .await
        .unwrap_or_default();
    let rules = parse_category_rules(&rules_json);

    let mut by_cat = std::collections::BTreeMap::<String, i64>::new();
    for row in app_minutes_since(pool, days.max(1)).await? {
        *by_cat.entry(categorize_app(&row.app, &rules)).or_default() += row.minutes;
    }

    let mut out: Vec<CategoryMinutes> = by_cat
        .into_iter()
        .map(|(category, minutes)| CategoryMinutes { category, minutes })
        .collect();
    out.sort_by(|a, b| b.minutes.cmp(&a.minutes));
    Ok(out)
}

// ===== Dashboard analytics =====

#[derive(Debug, serde::Serialize, PartialEq)]
pub struct DayCompletion {
    pub id: String,
    pub title: String,
}

// Tasks completed on a specific local day (for the calendar popup/tooltip).
#[tauri::command]
pub async fn get_completions_for_day(pool: State<'_, SqlitePool>, date: String) -> AppResult<Vec<DayCompletion>> {
    get_completions_for_day_impl(pool.inner(), date).await
}

pub async fn get_completions_for_day_impl(pool: &SqlitePool, date: String) -> AppResult<Vec<DayCompletion>> {
    let rows = sqlx::query(
        "SELECT id, title FROM tasks
         WHERE completed_at IS NOT NULL AND date(completed_at, 'localtime') = ?
         ORDER BY completed_at",
    )
    .bind(&date)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(|r| DayCompletion { id: r.get("id"), title: r.get("title") }).collect())
}

#[derive(Debug, serde::Serialize, PartialEq)]
pub struct HourCell {
    pub weekday: i64, // 0 = Sunday ... 6 = Saturday (strftime %w)
    pub hour: i64,    // 0-23, local time
    pub minutes: i64,
}

// An "hour x weekday" heatmap: active minutes over the last N days.
#[tauri::command]
pub async fn get_hourly_activity(pool: State<'_, SqlitePool>, days: i64) -> AppResult<Vec<HourCell>> {
    get_hourly_activity_impl(pool.inner(), days).await
}

pub async fn get_hourly_activity_impl(pool: &SqlitePool, days: i64) -> AppResult<Vec<HourCell>> {
    let since = format!("-{} days", days.max(1));
    let rows = sqlx::query(
        // Bucket by the MIDDLE of the period a tick measured, not by its
        // timestamp. The timestamp is the period's end (see the INSERT in
        // monitor::activity), so a tick finishing on the hour would otherwise be
        // credited entirely to the hour after the one actually worked. The
        // midpoint also handles a tick straddling a boundary: it lands in
        // whichever hour holds most of it, which is the best a single bucket can
        // do without splitting the row.
        "SELECT CAST(strftime('%w', datetime(timestamp, (-duration_secs / 2) || ' seconds'), 'localtime') AS INTEGER) AS weekday,
                CAST(strftime('%H', datetime(timestamp, (-duration_secs / 2) || ' seconds'), 'localtime') AS INTEGER) AS hour,
                SUM(duration_secs) / 60 AS minutes
         FROM activity_log
         WHERE state = 'Active'
           AND date(timestamp, 'localtime') >= date('now', 'localtime', ?)
         GROUP BY weekday, hour",
    )
    .bind(&since)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| HourCell { weekday: r.get("weekday"), hour: r.get("hour"), minutes: r.get("minutes") })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./src/db/migrations").run(&pool).await.unwrap();
        pool
    }

    // `ts` is the tick's END, matching what the activity loop writes: a tick
    // fires, then records the period that just elapsed. So a row (14:01, 60)
    // describes 14:00-14:01, not 14:01-14:02.
    async fn log(pool: &SqlitePool, ts: &str, state: &str, duration_secs: i64) {
        sqlx::query(
            "INSERT INTO activity_log (timestamp, state, app_focused, input_events, duration_secs)
             VALUES (?, ?, 1, 0, ?)")
            .bind(ts).bind(state).bind(duration_secs)
            .execute(pool).await.unwrap();
    }

    // time per site
    async fn log_domain(pool: &SqlitePool, ts: &str, app: &str, domain: Option<&str>, secs: i64) {
        sqlx::query(
            "INSERT INTO activity_log (timestamp, state, app_focused, input_events, duration_secs, app, domain)
             VALUES (?, 'Active', 1, 0, ?, ?, ?)")
            .bind(ts).bind(secs).bind(app).bind(domain)
            .execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn domain_usage_groups_and_sorts_by_minutes() {
        let pool = test_pool().await;
        let now = chrono::Utc::now();
        let ts = |m: i64| (now - chrono::Duration::minutes(m)).to_rfc3339();

        log_domain(&pool, &ts(10), "firefox", Some("github.com"), 600).await;
        log_domain(&pool, &ts(20), "firefox", Some("github.com"), 600).await;
        log_domain(&pool, &ts(30), "firefox", Some("youtube.com"), 300).await;

        let r = get_domain_usage_impl(&pool, 7).await.unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].domain, "github.com");
        assert_eq!(r[0].minutes, 20);
        assert_eq!(r[1].domain, "youtube.com");
    }

    // While track_domains is off the domain is NULL on every row, so the
    // statistics are empty — a normal state, not an error.
    #[tokio::test]
    async fn domain_usage_empty_when_nothing_tracked() {
        let pool = test_pool().await;
        let now = chrono::Utc::now().to_rfc3339();
        log_domain(&pool, &now, "firefox", None, 600).await;
        log_domain(&pool, &now, "kitty", None, 600).await;

        assert!(get_domain_usage_impl(&pool, 7).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn clear_domain_history_wipes_domains_but_keeps_activity() {
        let pool = test_pool().await;
        let now = chrono::Utc::now().to_rfc3339();
        log_domain(&pool, &now, "firefox", Some("github.com"), 600).await;

        let affected = clear_domain_history_impl(&pool).await.unwrap();
        assert_eq!(affected, 1);
        assert!(get_domain_usage_impl(&pool, 7).await.unwrap().is_empty());

        // The activity time itself is not lost, only the domains are erased
        let (active, _) = state_sums(&pool, "1=1").await.unwrap();
        assert_eq!(active, 600);
    }

    // idle time within time blocks
    async fn block(pool: &SqlitePool, id: &str, title: &str, at: &str, mins: i64) {
        sqlx::query(
            "INSERT INTO tasks (id, title, description, status, priority, category,
             deadline, tags, recurrence, hidden, created_at, updated_at, sort_order,
             scheduled_at, scheduled_mins)
             VALUES (?, ?, NULL, 'Todo', 'Medium', 'Other', NULL, '[]', 'None', 0,
             ?, ?, 1, ?, ?)")
            .bind(id).bind(title).bind(at).bind(at).bind(at).bind(mins)
            .execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn block_idle_splits_active_and_idle_within_block() {
        let pool = test_pool().await;
        // A 14:00-15:00 block (60 min)
        block(&pool, "t1", "работа", "2026-07-01T14:00:00+00:00", 60).await;

        // 20 minutes active, 10 minutes idle — inside the block
        // Ticks ENDING at 14:01..14:20 cover 14:00-14:20.
        for i in 1..=20 {
            log(&pool, &format!("2026-07-01T14:{:02}:00+00:00", i), "Active", 60).await;
        }
        // Ticks ENDING at 14:21..14:30 cover 14:20-14:30.
        for i in 21..=30 {
            log(&pool, &format!("2026-07-01T14:{:02}:00+00:00", i), "Idle", 60).await;
        }

        let r = get_block_idle_impl(&pool, "2026-07-01").await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].task_title, "работа");
        assert_eq!(r[0].planned_mins, 60);
        assert_eq!(r[0].active_mins, 20);
        assert_eq!(r[0].idle_mins, 10);
    }

    // The whole point of extracting overlap_secs: activity OUTSIDE a block must
    // not be credited to it, or the actual would exceed the plan.
    #[tokio::test]
    async fn block_idle_ignores_activity_outside_block() {
        let pool = test_pool().await;
        block(&pool, "t1", "встреча", "2026-07-01T14:00:00+00:00", 30).await;

        // Before and after the block — must not be counted. Ends chosen so the
        // periods themselves fall outside 14:00-14:30.
        log(&pool, "2026-07-01T13:00:00+00:00", "Active", 60).await;
        log(&pool, "2026-07-01T14:00:00+00:00", "Idle", 60).await;
        log(&pool, "2026-07-01T15:00:00+00:00", "Active", 60).await;
        log(&pool, "2026-07-01T16:00:00+00:00", "Idle", 60).await;
        // Inside the block — ticks ending 14:01..14:05 cover 14:00-14:05.
        for i in 1..=5 {
            log(&pool, &format!("2026-07-01T14:{:02}:00+00:00", i), "Idle", 60).await;
        }

        let r = get_block_idle_impl(&pool, "2026-07-01").await.unwrap();
        assert_eq!(r[0].idle_mins, 5);
        assert_eq!(r[0].active_mins, 0);
    }

    // The reader treated a row's timestamp as the START of its period, but the
    // activity loop writes it as the END: the tick fires, then records the time
    // that just elapsed. Every tick was therefore attributed one whole interval
    // into the future.
    //
    // This isolates the shift, which the other tests cannot: a tick aligned to a
    // block's boundary overlaps it under either convention. Here the tick lies
    // entirely BEFORE the block under the correct reading and entirely INSIDE it
    // under the wrong one, so the two differ by the whole tick.
    #[tokio::test]
    async fn a_tick_is_credited_to_the_period_it_measured_not_the_next_one() {
        let pool = test_pool().await;
        // A 14:00-15:00 block.
        block(&pool, "t1", "работа", "2026-07-01T14:00:00+00:00", 60).await;

        // A tick ending exactly when the block begins: it measured 13:00-14:00,
        // which is before the block and must count for nothing. Read as a start
        // it would have claimed 14:00-15:00 — the entire block, as idle.
        log(&pool, "2026-07-01T14:00:00+00:00", "Idle", 3600).await;

        let r = get_block_idle_impl(&pool, "2026-07-01").await.unwrap();
        assert_eq!(r.len(), 1, "блок должен найтись");
        assert_eq!(
            r[0].idle_mins, 0,
            "тик, измеривший час ДО блока, не должен засчитываться в блок"
        );

        // The control case: a tick that really did measure the block's first hour
        // ends when the block ends, and must count in full. Without this a reader
        // that credits nothing at all would pass the assertion above.
        log(&pool, "2026-07-01T15:00:00+00:00", "Idle", 3600).await;
        let r = get_block_idle_impl(&pool, "2026-07-01").await.unwrap();
        assert_eq!(r[0].idle_mins, 60, "тик, измеривший сам блок, засчитывается целиком");
    }

    #[tokio::test]
    async fn block_idle_counts_partial_tick_overlap() {
        let pool = test_pool().await;
        // A 14:00-14:10 block
        block(&pool, "t1", "короткий", "2026-07-01T14:00:00+00:00", 10).await;
        // A long idle tick covering 13:55-14:55 (so it ENDS at 14:55): only the
        // block's own 10 minutes fall inside it.
        log(&pool, "2026-07-01T14:55:00+00:00", "Idle", 3600).await;

        let r = get_block_idle_impl(&pool, "2026-07-01").await.unwrap();
        assert_eq!(r[0].idle_mins, 10);
    }

    #[tokio::test]
    async fn block_idle_returns_empty_without_blocks_and_skips_other_days() {
        let pool = test_pool().await;
        block(&pool, "t1", "вчерашний", "2026-06-30T14:00:00+00:00", 60).await;
        log(&pool, "2026-06-30T14:00:00+00:00", "Idle", 60).await;

        // A different day was requested: yesterday's block is not returned
        assert!(get_block_idle_impl(&pool, "2026-07-01").await.unwrap().is_empty());
        assert_eq!(get_block_idle_impl(&pool, "2026-06-30").await.unwrap().len(), 1);
    }

    // A task in the Trash must not show up in the day's digest.
    #[tokio::test]
    async fn block_idle_skips_deleted_tasks() {
        let pool = test_pool().await;
        block(&pool, "t1", "удалённая", "2026-07-01T14:00:00+00:00", 60).await;
        sqlx::query("UPDATE tasks SET deleted_at = ? WHERE id = 't1'")
            .bind("2026-07-01T15:00:00+00:00").execute(&pool).await.unwrap();
        log(&pool, "2026-07-01T14:00:00+00:00", "Idle", 60).await;

        assert!(get_block_idle_impl(&pool, "2026-07-01").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn activity_minutes_sum_durations_per_day() {
        let pool = test_pool().await;
        // Day 1: 3 active ticks of 60s each + idle (not counted)
        log(&pool, "2026-07-01T10:00:00+00:00", "Active", 60).await;
        log(&pool, "2026-07-01T10:01:00+00:00", "Active", 60).await;
        log(&pool, "2026-07-01T10:02:00+00:00", "Active", 60).await;
        log(&pool, "2026-07-01T10:03:00+00:00", "Idle", 60).await;
        // Day 2: ticks at a different interval (the setting changed) — 90s + 30s
        log(&pool, "2026-07-02T09:00:00+00:00", "Active", 90).await;
        log(&pool, "2026-07-02T09:02:00+00:00", "Active", 30).await;

        let days = get_activity_by_day_impl(&pool).await.unwrap();
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].date, "2026-07-01");
        assert_eq!(days[0].minutes, 3);   // 180s / 60, Idle excluded
        assert_eq!(days[1].date, "2026-07-02");
        assert_eq!(days[1].minutes, 2);   // (90+30)s / 60
    }

    #[tokio::test]
    async fn completions_grouped_by_day() {
        let pool = test_pool().await;
        for (id, day) in [("a", "01"), ("b", "01"), ("c", "02")] {
            sqlx::query(
                "INSERT INTO tasks (id, title, status, priority, category, tags, recurrence, hidden, created_at, updated_at, completed_at)
                 VALUES (?, 't', 'Done', 'Medium', 'Work', '[]', 'None', 1, '2026-07-01T00:00:00+00:00', '2026-07-01T00:00:00+00:00', ?)")
                .bind(id)
                .bind(format!("2026-07-{}T12:00:00+00:00", day))
                .execute(&pool).await.unwrap();
        }

        let days = get_task_completions_by_day_impl(&pool).await.unwrap();
        assert_eq!(days.len(), 2);
        assert_eq!((days[0].date.as_str(), days[0].completed), ("2026-07-01", 2));
        assert_eq!((days[1].date.as_str(), days[1].completed), ("2026-07-02", 1));
    }

    // The activity calendar and the completion calendar must agree about which
    // day an evening tick belongs to.
    //
    // Activity was grouped by the UTC date while completions use 'localtime', so
    // for anyone not on UTC the two squares disagreed: work logged at 02:00 local
    // in UTC+5 landed on the previous UTC day, and the dashboard showed minutes
    // on a day with no completed tasks while the task itself sat on the next one.
    // The date string here is also shown to the user verbatim as a day label.
    //
    // Built from the local midnight boundary so it fails wherever the machine's
    // offset is non-zero, rather than only in one timezone. On a UTC machine
    // local and UTC agree by definition and there is nothing to catch: the test
    // then only asserts the two commands match, which must hold everywhere.
    // get_block_idle is handed a LOCAL day key by the calendar (Calendar.svelte
    // builds it with localDateKey), but filtered both blocks and ticks by the UTC
    // date. A block scheduled just after local midnight was therefore invisible
    // on its own day, and the plan-versus-actual bar for it stayed empty.
    #[tokio::test]
    async fn block_idle_finds_a_block_scheduled_just_after_local_midnight() {
        use chrono::{Local, TimeZone};
        let pool = test_pool().await;

        let local_day = Local::now().date_naive();
        let at = Local
            .from_local_datetime(&local_day.and_hms_opt(0, 30, 0).unwrap())
            .single()
            .unwrap()
            .to_utc();

        sqlx::query(
            "INSERT INTO tasks (id, title, status, priority, category, tags, recurrence, hidden, created_at, updated_at, scheduled_at, scheduled_mins)
             VALUES ('b1', 'ночной блок', 'Todo', 'Medium', 'Work', '[]', 'None', 1, ?, ?, ?, 60)")
            .bind(at.to_rfc3339()).bind(at.to_rfc3339()).bind(at.to_rfc3339())
            .execute(&pool).await.unwrap();
        // Ten idle minutes inside that block: a tick ENDING ten minutes after the
        // block starts covers exactly the block's first ten minutes.
        let tick_end = at + chrono::Duration::minutes(10);
        log(&pool, &tick_end.to_rfc3339(), "Idle", 600).await;

        let key = local_day.format("%Y-%m-%d").to_string();
        let rows = get_block_idle_impl(&pool, &key).await.unwrap();
        assert_eq!(rows.len(), 1, "блок должен найтись по своему локальному дню");
        assert_eq!(rows[0].task_id, "b1");
        assert_eq!(rows[0].planned_mins, 60);
        assert_eq!(rows[0].idle_mins, 10, "простой внутри блока должен попасть в выборку");
    }

    #[tokio::test]
    async fn activity_days_use_the_same_local_day_as_completions() {
        use chrono::{Local, TimeZone, Timelike};
        let pool = test_pool().await;

        // 30 minutes past local midnight today: the instant most likely to sit on
        // a different UTC date than the local one.
        let local_key = Local::now().date_naive();
        let just_after_midnight = local_key.and_hms_opt(0, 30, 0).unwrap();
        let utc = Local
            .from_local_datetime(&just_after_midnight)
            .single()
            .unwrap()
            .to_utc();

        log(&pool, &utc.to_rfc3339(), "Active", 600).await;
        sqlx::query(
            "INSERT INTO tasks (id, title, status, priority, category, tags, recurrence, hidden, created_at, updated_at, completed_at)
             VALUES ('t1', 'сделана ночью', 'Done', 'Medium', 'Work', '[]', 'None', 1, ?, ?, ?)")
            .bind(utc.to_rfc3339()).bind(utc.to_rfc3339()).bind(utc.to_rfc3339())
            .execute(&pool).await.unwrap();

        let key = local_key.format("%Y-%m-%d").to_string();
        let activity = get_activity_by_day_impl(&pool).await.unwrap();
        let completions = get_task_completions_by_day_impl(&pool).await.unwrap();

        assert_eq!(activity.len(), 1, "одна запись — один день");
        assert_eq!(completions.len(), 1);
        // The substantive assertion: both calendars put the same instant on the
        // same square, and that square is the LOCAL day the user lived through.
        assert_eq!(
            activity[0].date, completions[0].date,
            "активность и выполнения должны попадать в один и тот же день"
        );
        assert_eq!(
            activity[0].date, key,
            "день должен быть локальным, а не UTC (смещение машины: {})",
            Local::now().offset()
        );
        // Sanity: the minutes themselves still arrive.
        assert_eq!(activity[0].minutes, 10);
        let _ = just_after_midnight.hour();
    }

    #[tokio::test]
    async fn completions_for_day_returns_titles_of_local_day() {
        use chrono::{Local, TimeZone, Duration};
        let pool = test_pool().await;

        let today_noon = Local::now().date_naive().and_hms_opt(12, 0, 0).unwrap();
        let today_utc = Local.from_local_datetime(&today_noon).single().unwrap().to_utc();
        for (id, title, at) in [
            ("d1", "сегодняшняя", today_utc),
            ("d2", "вчерашняя", today_utc - Duration::days(1)),
        ] {
            sqlx::query(
                "INSERT INTO tasks (id, title, status, priority, category, tags, recurrence, hidden, created_at, updated_at, completed_at)
                 VALUES (?, ?, 'Done', 'Medium', 'Work', '[]', 'None', 1, ?, ?, ?)")
                .bind(id).bind(title)
                .bind(at.to_rfc3339()).bind(at.to_rfc3339()).bind(at.to_rfc3339())
                .execute(&pool).await.unwrap();
        }

        let key = Local::now().date_naive().format("%Y-%m-%d").to_string();
        let completions = get_completions_for_day_impl(&pool, key).await.unwrap();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].id, "d1");
        assert_eq!(completions[0].title, "сегодняшняя");
        assert!(get_completions_for_day_impl(&pool, "1999-01-01".into()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn hourly_activity_groups_by_local_hour_and_window() {
        use chrono::{Datelike, Timelike, Duration, Local, Utc};
        let pool = test_pool().await;

        // A stable moment inside the hour: 3h before "now", at minute 10
        let t = (Utc::now() - Duration::hours(3))
            .with_minute(10).unwrap()
            .with_second(0).unwrap()
            .with_nanosecond(0).unwrap();
        log(&pool, &t.to_rfc3339(), "Active", 600).await; // 10 min
        log(&pool, &(t + Duration::minutes(5)).to_rfc3339(), "Active", 300).await; // +5 min, the same hour
        log(&pool, &t.to_rfc3339(), "Idle", 600).await; // not counted
        log(&pool, &(Utc::now() - Duration::days(100)).to_rfc3339(), "Active", 600).await; // outside the window

        let cells = get_hourly_activity_impl(&pool, 7).await.unwrap();
        let local = t.with_timezone(&Local);
        assert_eq!(cells, vec![HourCell {
            weekday: local.weekday().num_days_from_sunday() as i64,
            hour: local.hour() as i64,
            minutes: 15,
        }]);
    }

    // The same end-vs-start confusion as get_block_idle, in the hour heatmap:
    // a tick's timestamp is the END of what it measured, so a tick finishing
    // exactly on the hour belongs to the hour BEFORE it.
    //
    // The existing test above cannot catch this — it places the tick at minute 10,
    // where start and end share an hour. Here the tick measures a full hour and
    // ends on the boundary, so the correct bucket and the buggy one differ.
    #[tokio::test]
    async fn hourly_activity_credits_the_hour_the_work_happened_in() {
        use chrono::{Datelike, Duration, Local, Timelike, Utc};
        let pool = test_pool().await;

        // A tick ending exactly on an hour boundary, measuring the whole hour
        // before it. Three hours back so it stays inside the 7-day window.
        let end = (Utc::now() - Duration::hours(3))
            .with_minute(0).unwrap()
            .with_second(0).unwrap()
            .with_nanosecond(0).unwrap();
        log(&pool, &end.to_rfc3339(), "Active", 3600).await;

        let cells = get_hourly_activity_impl(&pool, 7).await.unwrap();
        // The work happened in the hour BEFORE the timestamp.
        let worked = (end - Duration::hours(1)).with_timezone(&Local);
        assert_eq!(cells.len(), 1, "один тик — одна ячейка");
        assert_eq!(
            (cells[0].weekday, cells[0].hour),
            (worked.weekday().num_days_from_sunday() as i64, worked.hour() as i64),
            "час должен быть тем, в котором шла работа, а не тем, в котором тик записался"
        );
        assert_eq!(cells[0].minutes, 60);
    }

    async fn insert_task(pool: &SqlitePool, id: &str, category: &str, completed_at: Option<&str>) {
        sqlx::query(
            "INSERT INTO tasks (id, title, status, priority, category, tags, recurrence, hidden, created_at, updated_at, completed_at)
             VALUES (?, 't', 'Done', 'Medium', ?, '[]', 'None', 0, '2026-07-01T00:00:00+00:00', '2026-07-01T00:00:00+00:00', ?)")
            .bind(id).bind(category).bind(completed_at)
            .execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn category_distribution_counts_only_completed() {
        let pool = test_pool().await;
        insert_task(&pool, "a", "Work", Some("2026-07-01T12:00:00+00:00")).await;
        insert_task(&pool, "b", "Work", Some("2026-07-02T12:00:00+00:00")).await;
        insert_task(&pool, "c", "Health", Some("2026-07-02T13:00:00+00:00")).await;
        insert_task(&pool, "d", "Study", None).await; // not completed, so not counted

        let cats = get_category_distribution_impl(&pool).await.unwrap();
        assert_eq!(cats.len(), 2);
        let get = |name: &str| cats.iter().find(|c| c.category == name).map(|c| c.count);
        assert_eq!(get("Work"), Some(2));
        assert_eq!(get("Health"), Some(1));
        assert_eq!(get("Study"), None);
    }

    #[tokio::test]
    async fn active_idle_ratio_splits_today_and_week() {
        let pool = test_pool().await;
        let now = chrono::Utc::now();
        let ts = |days_ago: i64| (now - chrono::Duration::days(days_ago)).to_rfc3339();

        // Today: 120s active + 60s idle
        log(&pool, &ts(0), "Active", 120).await;
        log(&pool, &ts(0), "Idle", 60).await;
        // 3 days ago: inside the week window but not today's
        log(&pool, &ts(3), "Active", 300).await;
        // 10 days ago: outside both windows
        log(&pool, &ts(10), "Active", 999).await;
        log(&pool, &ts(10), "Idle", 999).await;

        let r = get_active_idle_ratio_impl(&pool).await.unwrap();
        assert_eq!((r.today_active, r.today_idle), (120, 60));
        assert_eq!((r.week_active, r.week_idle), (420, 60));
    }

    #[test]
    fn glob_match_cases() {
        assert!(glob_match("kitty", "kitty"));
        assert!(glob_match("KiTTy", "kitty")); // case does not matter
        assert!(!glob_match("kitty", "kitty-extra")); // without '*' it is exact
        assert!(glob_match("kitty*", "kitty-extra"));
        assert!(glob_match("*fox", "firefox"));
        assert!(glob_match("*ire*", "firefox"));
        assert!(glob_match("jetbrains-*", "jetbrains-idea"));
        assert!(!glob_match("jetbrains-*", "idea-jetbrains"));
        assert!(glob_match("*", "что угодно"));
        assert!(!glob_match("a*b", "ba")); // the order of the parts matters
    }

    #[test]
    fn categorize_first_match_wins_and_unknown_is_other() {
        let rules = parse_category_rules(
            r#"[{"pattern":"jetbrains-*","category":"Work"},
                {"pattern":"*","category":"Study"},
                {"pattern":"zen","category":"Игры"}]"#,
        );
        assert_eq!(categorize_app("jetbrains-idea", &rules), "Work");
        assert_eq!(categorize_app("kitty", &rules), "Study"); // the wildcard rule
        // "Игры" is not in the palette: the rule is skipped (the wildcard catches it here)
        assert_eq!(categorize_app("zen", &rules), "Study");

        assert_eq!(categorize_app("anything", &[]), "Other");
        assert!(parse_category_rules("мусор").is_empty());
        assert!(parse_category_rules("").is_empty());
    }

    // Only apps with no matching rule reach the model, ordered by time. Anything a
    // rule already covers is the user's own decision and re-proposing it would risk
    // overwriting what they configured by hand.
    #[tokio::test]
    async fn uncategorized_apps_skips_apps_with_rules() {
        let pool = test_pool().await;
        let now = chrono::Utc::now();
        let ts = |m: i64| (now - chrono::Duration::minutes(m)).to_rfc3339();

        log_app(&pool, &ts(1), Some("firefox"), 6000).await;
        log_app(&pool, &ts(2), Some("jetbrains-idea"), 3000).await;
        log_app(&pool, &ts(3), Some("obscure-tool"), 600).await;

        crate::commands::settings::set_setting(
            &pool,
            "app_category_rules",
            r#"[{"pattern":"firefox","category":"Study"}]"#,
        ).await.unwrap();

        let out = uncategorized_apps_impl(&pool, 30, 15).await.unwrap();
        let names: Vec<&str> = out.iter().map(|a| a.app.as_str()).collect();
        assert_eq!(names, vec!["jetbrains-idea", "obscure-tool"],
            "the app with a rule is excluded, the rest keep their order by time");
    }

    // The list is capped, so the order decides what actually gets categorized: the
    // apps eating the time, not a program opened once for a minute.
    #[tokio::test]
    async fn uncategorized_apps_respects_limit_by_time() {
        let pool = test_pool().await;
        let now = chrono::Utc::now();
        let ts = |m: i64| (now - chrono::Duration::minutes(m)).to_rfc3339();

        log_app(&pool, &ts(1), Some("small"), 60).await;
        log_app(&pool, &ts(2), Some("big"), 6000).await;

        let out = uncategorized_apps_impl(&pool, 30, 1).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].app, "big");
    }

    async fn log_app(pool: &SqlitePool, ts: &str, app: Option<&str>, duration_secs: i64) {
        sqlx::query(
            "INSERT INTO activity_log (timestamp, state, app_focused, input_events, duration_secs, app)
             VALUES (?, 'Active', 1, 0, ?, ?)")
            .bind(ts).bind(duration_secs).bind(app)
            .execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn app_usage_sums_and_respects_window() {
        let pool = test_pool().await;
        let now = chrono::Utc::now();
        let ts = |days_ago: i64| (now - chrono::Duration::days(days_ago)).to_rfc3339();

        log_app(&pool, &ts(0), Some("kitty"), 600).await;
        log_app(&pool, &ts(0), Some("kitty"), 600).await;
        log_app(&pool, &ts(0), Some("zen"), 300).await;
        log_app(&pool, &ts(0), None, 999).await; // no app, so not counted
        log_app(&pool, &ts(30), Some("kitty"), 6000).await; // outside the window

        let usage = get_app_usage_impl(&pool, 7).await.unwrap();
        assert_eq!(usage[0], AppMinutes { app: "kitty".into(), minutes: 20 });
        assert_eq!(usage[1], AppMinutes { app: "zen".into(), minutes: 5 });
        assert_eq!(usage.len(), 2);
    }

    #[tokio::test]
    async fn category_time_applies_rules_from_settings() {
        let pool = test_pool().await;
        crate::commands::settings::set_setting(
            &pool,
            "app_category_rules",
            r#"[{"pattern":"kitty","category":"Work"}]"#,
        )
        .await
        .unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        log_app(&pool, &now, Some("kitty"), 600).await;
        log_app(&pool, &now, Some("zen"), 300).await; // no rule, so Other

        let cats = get_app_category_time_impl(&pool, 1).await.unwrap();
        assert_eq!(cats[0], CategoryMinutes { category: "Work".into(), minutes: 10 });
        assert_eq!(cats[1], CategoryMinutes { category: "Other".into(), minutes: 5 });
    }
}

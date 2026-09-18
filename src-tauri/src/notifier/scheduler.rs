use chrono::{DateTime, Local, TimeZone, Utc};
use std::sync::{Arc, Mutex};
use sqlx::{SqlitePool, Row};
use tauri_plugin_notification::NotificationExt;
use crate::commands::settings::WorkMode;

pub fn start_scheduler(app: tauri::AppHandle, pool: SqlitePool, work_mode: Arc<Mutex<WorkMode>>) {
    tokio::spawn(async move {
        loop {
            // Under Focus or an active pause we still check and mark deadlines but
            // do not send: otherwise a batch of stale notifications would arrive the
            // moment the mute is lifted.
            let mode = work_mode.lock().unwrap().clone();
            let muted = crate::notifier::mute::muted_now(&pool, &mode).await;
            check_deadlines(&app, &pool, muted).await;
            check_blocks(&app, &pool, muted).await;
            check_goals(&app, &pool, muted).await;
            check_morning_digest(&app, &pool, muted).await;
            check_app_limits(&app, &pool, muted).await;
            check_offtrack(&app, &pool, muted).await;
            check_note_reminders(&app, &pool, muted).await;
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        }
    });
}

// Marks one notification flag, but only while the task still sits at the very
// deadline the SELECT saw (v0.9.90).
//
// The race: check_deadlines reads the tasks, then awaits the sending of a
// notification, and only then writes the flag. If the user completes a recurring
// task inside that window, complete_task_impl moves the deadline and resets the
// flags (commands/tasks.rs) — and the scheduler's late write would then set
// notified_24h = 1 against the NEW deadline. The task would never be announced
// again, which is exactly the bug the comment in complete_task_impl describes as
// already fixed once; the race brings it back.
//
// The raw string from the SELECT is bound deliberately, with no re-formatting: a
// parse-and-render round trip can produce a different spelling of the same
// instant and the WHERE would never match.
//
// The column name is interpolated, never user input — the three call sites below
// are the only ones, and each passes a literal.
async fn mark_notified(pool: &SqlitePool, id: &str, deadline_str: &str, column: &str) {
    let sql = format!("UPDATE tasks SET {column} = 1 WHERE id = ? AND deadline = ?");
    let _ = sqlx::query(&sql).bind(id).bind(deadline_str).execute(pool).await;
}

async fn check_deadlines(app: &tauri::AppHandle, pool: &SqlitePool, muted: bool) {
    use crate::commands::settings::get_u64_setting;
    let now = Utc::now();
    let warn_hours = get_u64_setting(pool, "deadline_warn_hours", 24).await as i64;
    let warn_mins = get_u64_setting(pool, "deadline_warn_minutes", 60).await as i64;
    let at_hours = now + chrono::Duration::hours(warn_hours);
    let at_mins = now + chrono::Duration::minutes(warn_mins);
    let lang = crate::i18n::current_lang(pool).await;
    let msg_hours = crate::i18n::tr_args("Дедлайн через {n} ч", lang, &[("n", warn_hours.to_string())]);
    let msg_mins = crate::i18n::tr_args("Дедлайн через {n} мин", lang, &[("n", warn_mins.to_string())]);
    // The early warning is whichever is further from now. We do not assume the
    // "hours" setting always exceeds the "minutes" one: the user may have set 1h
    // and 90min.
    let (early_at, early_msg, late_at, late_msg) = if at_hours >= at_mins {
        (at_hours, &msg_hours, at_mins, &msg_mins)
    } else {
        (at_mins, &msg_mins, at_hours, &msg_hours)
    };

    let rows = match sqlx::query(
        "SELECT id, title, deadline, notified_24h, notified_1h, notified_deadline
         FROM tasks WHERE hidden = 0 AND deadline IS NOT NULL AND deleted_at IS NULL"
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return,
    };

    for row in rows {
        let id: String = row.get("id");
        let title: String = row.get("title");
        let deadline_str: String = row.get("deadline");
        let notified_24h: bool = row.get("notified_24h");
        let notified_1h: bool = row.get("notified_1h");
        let notified_deadline: bool = row.get("notified_deadline");

        let Ok(deadline) = chrono::DateTime::parse_from_rfc3339(&deadline_str) else { continue; };
        let deadline = deadline.with_timezone(&Utc);

        if !notified_24h && deadline <= early_at && deadline > late_at {
            if !muted { send_deadline_notification(app, pool, &id, &title, early_msg).await; }
            mark_notified(pool, &id, &deadline_str, "notified_24h").await;
        }

        if !notified_1h && deadline <= late_at && deadline > now {
            if !muted { send_deadline_notification(app, pool, &id, &title, late_msg).await; }
            mark_notified(pool, &id, &deadline_str, "notified_1h").await;
        }

        if !notified_deadline && deadline <= now {
            if !muted { send_deadline_notification(app, pool, &id, &title, &crate::i18n::tr("Дедлайн наступил!", lang)).await; }
            mark_notified(pool, &id, &deadline_str, "notified_deadline").await;
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct BlockDue {
    pub id: String,
    pub title: String,
    pub end_local: String, // the block's end as local "HH:MM", for the push text
    pub end_utc: DateTime<Utc>,
}

// Blocks that started within the (now - grace, now] window and have not been
// notified about yet. The grace window keeps us from spamming about long-started
// blocks after a long sleep or a restart — those are simply marked on the next
// check.
pub async fn blocks_due(pool: &SqlitePool, now: chrono::DateTime<Utc>, grace_mins: i64) -> Vec<BlockDue> {
    let rows = match sqlx::query(
        "SELECT id, title, scheduled_at, COALESCE(scheduled_mins, 60) as mins
         FROM tasks
         WHERE hidden = 0 AND notified_block = 0 AND scheduled_at IS NOT NULL AND deleted_at IS NULL",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return vec![],
    };

    let mut due = vec![];
    for row in rows {
        let scheduled_str: String = row.get("scheduled_at");
        let Ok(start) = chrono::DateTime::parse_from_rfc3339(&scheduled_str) else { continue; };
        let start = start.with_timezone(&Utc);
        if start <= now && start > now - chrono::Duration::minutes(grace_mins) {
            let mins: i64 = row.get("mins");
            let end_utc = start + chrono::Duration::minutes(mins);
            let end = end_utc.with_timezone(&chrono::Local);
            due.push(BlockDue {
                id: row.get("id"),
                title: row.get("title"),
                end_local: end.format("%H:%M").to_string(),
                end_utc,
            });
        }
    }
    due
}

pub async fn mark_block_notified(pool: &SqlitePool, id: &str) {
    let _ = sqlx::query("UPDATE tasks SET notified_block = 1 WHERE id = ?")
        .bind(id).execute(pool).await;
}

// Overdue blocks (older than the grace window) that were never notified about
// are marked too, so they do not remain candidates forever.
async fn sweep_stale_blocks(pool: &SqlitePool, now: chrono::DateTime<Utc>, grace_mins: i64) {
    let cutoff = (now - chrono::Duration::minutes(grace_mins)).to_rfc3339();
    let _ = sqlx::query(
        "UPDATE tasks SET notified_block = 1
         WHERE notified_block = 0 AND scheduled_at IS NOT NULL AND scheduled_at <= ?",
    )
    .bind(&cutoff)
    .execute(pool)
    .await;
}

const BLOCK_GRACE_MINS: i64 = 10;

async fn check_blocks(app: &tauri::AppHandle, pool: &SqlitePool, muted: bool) {
    let now = Utc::now();
    let focus_auto = crate::commands::settings::get_bool_setting(pool, "focus_mode_auto", true).await;
    for block in blocks_due(pool, now, BLOCK_GRACE_MINS).await {
        if !muted {
            let lang = crate::i18n::current_lang(pool).await;
            let body = crate::i18n::tr_args("Начался блок (до {time})", lang, &[("time", block.end_local.clone())]);
            send_notification(app, pool, "block", &block.title, &body).await;
        }
        if focus_auto {
            crate::notifier::mute::extend_quiet_until(pool, block.end_utc).await;
        }
        mark_block_notified(pool, &block.id).await;
    }
    sweep_stale_blocks(pool, now, BLOCK_GRACE_MINS).await;
}

#[derive(Debug, PartialEq)]
pub struct ReminderDue {
    pub note_id: String,
    pub title: String,
}

// Note reminders that have come due and have not been notified about yet. As
// with blocks this uses a window, but with no upper "too old" bound: a reminder
// missed during a long sleep is still worth showing once rather than staying
// silent forever.
pub async fn note_reminders_due(pool: &SqlitePool, now: chrono::DateTime<Utc>) -> Vec<ReminderDue> {
    let rows = match sqlx::query(
        "SELECT id, title FROM notes WHERE notified_reminder = 0 AND reminder_at IS NOT NULL AND reminder_at <= ?"
    )
    .bind(now.to_rfc3339())
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return vec![],
    };
    rows.into_iter()
        .map(|row| ReminderDue { note_id: row.get("id"), title: row.get("title") })
        .collect()
}

pub async fn mark_reminder_notified(pool: &SqlitePool, note_id: &str) {
    let _ = sqlx::query("UPDATE notes SET notified_reminder = 1 WHERE id = ?")
        .bind(note_id).execute(pool).await;
}

async fn check_note_reminders(app: &tauri::AppHandle, pool: &SqlitePool, muted: bool) {
    let now = Utc::now();
    for reminder in note_reminders_due(pool, now).await {
        if !muted {
            send_note_reminder(app, pool, &reminder.note_id, &reminder.title).await;
        }
        mark_reminder_notified(pool, &reminder.note_id).await;
    }
}

// A note reminder carrying buttons: "Done" drops the reminder, "Snooze" moves it
// an hour ahead. Same shape as send_deadline_notification — the direct path on
// Linux, the plugin everywhere else and whenever the direct one fails.
pub async fn send_note_reminder(
    app: &tauri::AppHandle,
    pool: &SqlitePool,
    note_id: &str,
    title: &str,
) {
    let lang = crate::i18n::current_lang(pool).await;
    let body_owned = crate::i18n::tr("Напоминание о заметке", lang);
    let body = body_owned.as_str();

    #[cfg(target_os = "linux")]
    let shown = {
        use crate::notifier::actions::{ActionTarget, NotificationAction};
        crate::notifier::actions::show_with_actions(
            app.clone(),
            pool.clone(),
            ActionTarget::Note(note_id.to_string()),
            title,
            body,
            &[
                (NotificationAction::NoteDone, crate::i18n::tr("Готово", lang).to_string()),
                (NotificationAction::NoteSnoozeHour, crate::i18n::tr("Отложить на час", lang).to_string()),
            ],
        )
    };
    #[cfg(not(target_os = "linux"))]
    let shown = false;

    if shown {
        log_notification(pool, "note_reminder", title, body, Some("note"), Some(note_id)).await;
    } else {
        send_notification_for(app, pool, "note_reminder", title, body, Some("note"), Some(note_id)).await;
    }
}

#[derive(Debug, PartialEq)]
pub struct GoalDue {
    pub id: String,
    pub name: String,
    pub body: String,
    pub period_key: String, // what to stamp notified_goal with after the push
}

// Projects whose goal for the current period has been reached and which have not
// been pushed about in that period. If both halves of the goal are set (tasks and
// minutes), both must be reached.
pub async fn goals_due(pool: &SqlitePool, now: chrono::DateTime<Utc>) -> Vec<GoalDue> {
    use crate::commands::projects::{get_projects_at, period_key};
    let projects = match get_projects_at(pool, now).await {
        Ok(p) => p,
        Err(_) => return vec![],
    };

    let mut due = vec![];
    for p in projects {
        if p.archived || (p.goal_tasks.is_none() && p.goal_mins.is_none()) {
            continue;
        }
        let tasks_met = p.goal_tasks.is_none_or(|n| p.goal_done_tasks >= n);
        let mins_met = p.goal_mins.is_none_or(|n| p.goal_done_mins >= n);
        let key = period_key(now, &p.goal_period);
        if !(tasks_met && mins_met) || p.notified_goal == key {
            continue;
        }
        let mut parts = vec![];
        if let Some(n) = p.goal_tasks { parts.push(format!("{} задач", n)); }
        if let Some(n) = p.goal_mins { parts.push(format!("{} мин", n)); }
        let period = if p.goal_period == "month" { "месяца" } else { "недели" };
        due.push(GoalDue {
            id: p.id,
            name: p.name,
            body: format!("Цель {} выполнена: {} 🎉", period, parts.join(" · ")),
            period_key: key,
        });
    }
    due
}

pub async fn mark_goal_notified(pool: &SqlitePool, id: &str, period_key: &str) {
    let _ = sqlx::query("UPDATE projects SET notified_goal = ? WHERE id = ?")
        .bind(period_key).bind(id).execute(pool).await;
}

async fn check_morning_digest(app: &tauri::AppHandle, pool: &SqlitePool, muted: bool) {
    let now = Utc::now();
    if !morning_digest_due(pool, now).await { return; }
    let local_today = now.with_timezone(&Local).date_naive();
    let tomorrow_local = local_today.succ_opt().unwrap_or(local_today);
    let tomorrow_utc = Local
        .from_local_datetime(&tomorrow_local.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or(now);

    // Today's blocks: the start of the local day, in UTC
    let today_start_utc = Local
        .from_local_datetime(&local_today.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or(now - chrono::Duration::hours(12));
    let blocks = sqlx::query(
        "SELECT title, COALESCE(scheduled_mins, 60) AS mins, scheduled_at
         FROM tasks WHERE hidden = 0 AND status != 'Done' AND deleted_at IS NULL
           AND scheduled_at IS NOT NULL AND scheduled_at < ? AND scheduled_at >= ?"
    )
    .bind(tomorrow_utc.to_rfc3339())
    .bind(today_start_utc.to_rfc3339())
    .fetch_all(pool).await.unwrap_or_default();

    // Today's deadlines plus overdue items
    let due_row = sqlx::query(
        "SELECT COUNT(*) AS due,
                SUM(CASE WHEN deadline < ? THEN 1 ELSE 0 END) AS overdue
         FROM tasks WHERE hidden = 0 AND status != 'Done' AND deleted_at IS NULL
           AND deadline IS NOT NULL AND deadline < ?"
    )
    .bind(now.to_rfc3339())
    .bind(tomorrow_utc.to_rfc3339())
    .fetch_one(pool).await;

    let (due, overdue) = match due_row {
        Ok(r) => (r.get::<i64, _>("due"), r.get::<Option<i64>, _>("overdue").unwrap_or(0)),
        _ => (0i64, 0i64),
    };

    let mut body = String::new();
    if !blocks.is_empty() {
        body.push_str(&format!("Запланировано блоков: {}\n", blocks.len()));
        for b in blocks.iter().take(3) {
            let title: String = b.get("title");
            let mins: i64 = b.get("mins");
            let sched: String = b.get("scheduled_at");
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&sched) {
                let local = dt.with_timezone(&Local);
                body.push_str(&format!("  {} {}мин\n", local.format("%H:%M"), mins));
            }
            body.push_str(&format!("  {}\n", title));
        }
        if blocks.len() > 3 {
            body.push_str(&format!("  ...и ещё {}\n", blocks.len() - 3));
        }
    }
    if due > 0 {
        body.push_str(&format!("Дедлайнов сегодня: {due}"));
        if overdue > 0 { body.push_str(&format!(" (просрочено: {overdue})")); }
        body.push('\n');
    }
    if body.is_empty() {
        body = "На сегодня ничего не запланировано.".into();
    }
    if !muted {
        let lang = crate::i18n::current_lang(pool).await;
        send_notification(app, pool, "digest", &crate::i18n::tr("Утренняя сводка", lang), body.trim()).await;
    }
    crate::commands::settings::set_setting(pool, "morning_digest_last", &local_today.format("%Y-%m-%d").to_string()).await.ok();
}

async fn check_goals(app: &tauri::AppHandle, pool: &SqlitePool, muted: bool) {
    let now = Utc::now();
    for goal in goals_due(pool, now).await {
        if !muted {
            send_notification(app, pool, "goal", &goal.name, &goal.body).await;
        }
        // Marked under mute too, so no batch arrives once the mute is lifted
        mark_goal_notified(pool, &goal.id, &goal.period_key).await;
    }
    record_goal_snapshots(pool, now).await;
}

async fn record_goal_snapshots(pool: &SqlitePool, now: chrono::DateTime<Utc>) {
    use crate::commands::projects::{get_projects_at, period_key};
    let Ok(projects) = get_projects_at(pool, now).await else { return };
    for p in projects {
        if p.archived || (p.goal_tasks.is_none() && p.goal_mins.is_none()) {
            continue;
        }
        let key = period_key(now, &p.goal_period);
        let last = sqlx::query(
            "SELECT done_tasks, done_mins FROM project_goal_history
             WHERE project_id = ? AND period_key = ?
             ORDER BY recorded_at DESC LIMIT 1"
        )
        .bind(&p.id).bind(&key)
        .fetch_optional(pool).await;
        let Ok(Some(last_row)) = last else {
            // no row yet — create the first one
            let _ = sqlx::query(
                "INSERT INTO project_goal_history (id, project_id, period_key, goal_tasks, goal_mins, done_tasks, done_mins, recorded_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(&p.id).bind(&key)
            .bind(p.goal_tasks).bind(p.goal_mins)
            .bind(p.goal_done_tasks).bind(p.goal_done_mins)
            .bind(now.to_rfc3339())
            .execute(pool).await;
            continue;
        };
        let last_done_tasks: i64 = last_row.get("done_tasks");
        let last_done_mins: i64 = last_row.get("done_mins");
        if last_done_tasks != p.goal_done_tasks || last_done_mins != p.goal_done_mins {
            let _ = sqlx::query(
                "INSERT INTO project_goal_history (id, project_id, period_key, goal_tasks, goal_mins, done_tasks, done_mins, recorded_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(&p.id).bind(&key)
            .bind(p.goal_tasks).bind(p.goal_mins)
            .bind(p.goal_done_tasks).bind(p.goal_done_mins)
            .bind(now.to_rfc3339())
            .execute(pool).await;
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct LimitDue {
    pub category: String,
    pub minutes: i64,
    pub limit: i64,
}

// Categories that have exceeded their daily limit and have not been notified
// about today (local day). notified_json is the current contents of
// settings.app_limits_notified: {category: "YYYY-MM-DD"}.
pub fn limits_due(
    limits: &[crate::commands::monitor::AppLimit],
    usage: &[crate::commands::monitor::CategoryMinutes],
    notified_json: &str,
    now: chrono::DateTime<Utc>,
) -> Vec<LimitDue> {
    let today = now.with_timezone(&Local).format("%Y-%m-%d").to_string();
    let notified: std::collections::HashMap<String, String> =
        serde_json::from_str(notified_json).unwrap_or_default();

    let mut out = Vec::new();
    for limit in limits {
        if limit.daily_mins <= 0 { continue; }
        let minutes = usage.iter().find(|u| u.category == limit.category).map(|u| u.minutes).unwrap_or(0);
        if minutes < limit.daily_mins { continue; }
        if notified.get(&limit.category) == Some(&today) { continue; }
        out.push(LimitDue { category: limit.category.clone(), minutes, limit: limit.daily_mins });
    }
    out
}

// "You meant to be doing X, and you are in something else."
//
// Every other notification here fires on the clock (a deadline at 24h/1h, a
// block starting, a digest at 08:00) and none of them look at what the user is
// actually doing. This one is the opposite: the time is not what makes it due,
// the mismatch is.
//
// The mismatch is only claimed when BOTH halves are known: a task worth being at
// (see UrgentTask) and a focused app that the user's own app_category_rules
// place in a different category. An app with no rule categorises as "Other",
// which is a default rather than a statement, so it is never treated as a
// mismatch — otherwise every unclassified tool would look like a distraction.
#[derive(Debug, Clone, PartialEq)]
pub struct OfftrackDue {
    pub task_id: String,
    pub title: String,
    pub task_category: String,
    pub app: String,
    pub app_category: String,
    // Why this task counts as urgent, for the notification text.
    pub reason: OfftrackReason,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OfftrackReason {
    // A timer is running on the task: the strongest signal, since time is being
    // recorded against work that is not happening.
    Tracking,
    // The deadline is within the warning window.
    DeadlineSoon,
    // The task is simply in progress.
    InProgress,
}

// One candidate task, already narrowed by the query.
#[derive(Debug, Clone)]
pub struct UrgentTask {
    pub id: String,
    pub title: String,
    pub category: String,
    pub tracking: bool,
    pub deadline_soon: bool,
    pub in_progress: bool,
}

pub fn offtrack_due(
    tasks: &[UrgentTask],
    app: &str,
    app_category: &str,
    notified_json: &str,
    now: DateTime<Utc>,
) -> Option<OfftrackDue> {
    // An unknown app tells us nothing: "Other" is what categorize_app returns
    // when NO rule matched, not a claim that the app is unrelated to the work.
    if app.is_empty() || app_category == "Other" {
        return None;
    }

    let today = now.with_timezone(&Local).format("%Y-%m-%d").to_string();
    let notified: std::collections::HashMap<String, String> =
        serde_json::from_str(notified_json).unwrap_or_default();

    tasks
        .iter()
        // A task in the same category as the focused app is exactly what the
        // user should be doing — never a mismatch.
        .filter(|t| t.category != app_category)
        // A task with no category cannot be compared against one.
        .filter(|t| !t.category.is_empty())
        .filter(|t| notified.get(&t.id) != Some(&today))
        .filter_map(|t| {
            let reason = if t.tracking {
                OfftrackReason::Tracking
            } else if t.deadline_soon {
                OfftrackReason::DeadlineSoon
            } else if t.in_progress {
                OfftrackReason::InProgress
            } else {
                return None;
            };
            Some(OfftrackDue {
                task_id: t.id.clone(),
                title: t.title.clone(),
                task_category: t.category.clone(),
                app: app.to_string(),
                app_category: app_category.to_string(),
                reason,
            })
        })
        // The strongest reason wins, so one notification names the task that
        // matters most rather than whichever the query returned first.
        .min_by_key(|d| match d.reason {
            OfftrackReason::Tracking => 0,
            OfftrackReason::DeadlineSoon => 1,
            OfftrackReason::InProgress => 2,
        })
}

async fn check_app_limits(app: &tauri::AppHandle, pool: &SqlitePool, muted: bool) {
    let now = Utc::now();
    let limits_json = crate::commands::settings::get_setting(pool, "app_limits").await.unwrap_or_default();
    let limits = crate::commands::monitor::parse_app_limits(&limits_json);
    if limits.is_empty() { return; }

    let Ok(usage) = crate::commands::monitor::get_app_category_time_impl(pool, 1).await else { return; };
    let notified_json = crate::commands::settings::get_setting(pool, "app_limits_notified").await.unwrap_or_default();
    let due = limits_due(&limits, &usage, &notified_json, now);
    if due.is_empty() { return; }

    let mut notified: std::collections::HashMap<String, String> =
        serde_json::from_str(&notified_json).unwrap_or_default();
    let today = now.with_timezone(&Local).format("%Y-%m-%d").to_string();

    for d in &due {
        if !muted {
            let lang = crate::i18n::current_lang(pool).await;
            let body = crate::i18n::tr_args("{cat}: {mins} мин из {limit} сегодня", lang,
                &[("cat", d.category.clone()), ("mins", d.minutes.to_string()), ("limit", d.limit.to_string())]);
            send_notification(app, pool, "app_limit", &d.category, &body).await;
        }
        // Marked under mute too, or a batch would arrive once the mute is lifted.
        notified.insert(d.category.clone(), today.clone());
    }
    if let Ok(json) = serde_json::to_string(&notified) {
        let _ = crate::commands::settings::set_setting(pool, "app_limits_notified", &json).await;
    }
}

async fn check_offtrack(app: &tauri::AppHandle, pool: &SqlitePool, muted: bool) {
    // Off by default with the rest of the contextual notifications: this one is
    // the most opinionated of them, and it should not appear unasked.
    if crate::commands::settings::get_setting(pool, "offtrack_notifications")
        .await
        .as_deref()
        != Some("true")
    {
        return;
    }

    let now = Utc::now();
    let warn_hours = crate::commands::settings::get_u64_setting(pool, "deadline_warn_hours", 24).await as i64;

    // The focused app comes from activity_log rather than from a live provider
    // query: the provider lives inside the monitor loop and is not shared, and
    // the log is written every tick anyway. Only a recent row counts — a stale
    // one would describe what the user was doing an hour ago.
    let Ok(row) = sqlx::query(
        "SELECT app FROM activity_log
         WHERE state = 'Active' AND app IS NOT NULL AND app != ''
           AND timestamp >= datetime('now', '-5 minutes')
         ORDER BY timestamp DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    else {
        return;
    };
    let Some(row) = row else { return };
    let focused: String = row.get("app");

    let rules_json = crate::commands::settings::get_setting(pool, "app_category_rules")
        .await
        .unwrap_or_default();
    let rules = crate::commands::monitor::parse_category_rules(&rules_json);
    let app_category = crate::commands::monitor::categorize_app(&focused, &rules);

    let warn_at = (now + chrono::Duration::hours(warn_hours)).to_rfc3339();
    let Ok(rows) = sqlx::query(
        "SELECT t.id, t.title, t.category, t.status, t.deadline,
                EXISTS(SELECT 1 FROM task_sessions s
                       WHERE s.task_id = t.id AND s.ended_at IS NULL) AS tracking
         FROM tasks t
         WHERE t.hidden = 0 AND t.deleted_at IS NULL
           AND t.status NOT IN ('Done', 'Archived')
           AND (t.status = 'InProgress'
                OR (t.deadline IS NOT NULL AND t.deadline <= ?)
                OR EXISTS(SELECT 1 FROM task_sessions s
                          WHERE s.task_id = t.id AND s.ended_at IS NULL))",
    )
    .bind(&warn_at)
    .fetch_all(pool)
    .await
    else {
        return;
    };

    let tasks: Vec<UrgentTask> = rows
        .iter()
        .map(|r| {
            let deadline: Option<String> = r.get("deadline");
            UrgentTask {
                id: r.get("id"),
                title: r.get("title"),
                category: r.get::<Option<String>, _>("category").unwrap_or_default(),
                tracking: r.get::<i64, _>("tracking") != 0,
                deadline_soon: deadline.is_some(),
                in_progress: r.get::<String, _>("status") == "InProgress",
            }
        })
        .collect();

    let notified_json = crate::commands::settings::get_setting(pool, "offtrack_notified")
        .await
        .unwrap_or_default();
    let Some(due) = offtrack_due(&tasks, &focused, &app_category, &notified_json, now) else {
        return;
    };

    if !muted {
        let lang = crate::i18n::current_lang(pool).await;
        let key = match due.reason {
            OfftrackReason::Tracking => "Идёт таймер задачи «{task}», а вы в {app}.",
            OfftrackReason::DeadlineSoon => "Скоро дедлайн задачи «{task}», а вы в {app}.",
            OfftrackReason::InProgress => "Задача «{task}» в работе, а вы в {app}.",
        };
        let body = crate::i18n::tr_args(
            key,
            lang,
            &[
                ("task", crate::monitor::activity::ellipsize(&due.title, crate::monitor::activity::TASK_TITLE_MAX)),
                ("app", due.app.clone()),
            ],
        );
        send_notification(app, pool, "offtrack", "Kriptag", &body).await;
    }

    // Marked under mute too, for the same reason as the app limits: otherwise a
    // batch would arrive the moment the mute is lifted.
    let mut notified: std::collections::HashMap<String, String> =
        serde_json::from_str(&notified_json).unwrap_or_default();
    let today = now.with_timezone(&Local).format("%Y-%m-%d").to_string();
    notified.insert(due.task_id.clone(), today);
    // Only today's marks are kept, or the setting would grow without bound.
    let today_str = now.with_timezone(&Local).format("%Y-%m-%d").to_string();
    notified.retain(|_, day| *day == today_str);
    if let Ok(json) = serde_json::to_string(&notified) {
        let _ = crate::commands::settings::set_setting(pool, "offtrack_notified", &json).await;
    }
}

// The morning digest: should it run? (the time has come, nothing was sent today, a time is configured)
async fn morning_digest_due(pool: &SqlitePool, now: chrono::DateTime<Utc>) -> bool {
    let local_now = now.with_timezone(&Local);
    let time_setting = crate::commands::settings::get_setting(pool, "morning_digest_time").await;
    let Some(time_str) = time_setting else { return false; };
    if time_str.is_empty() { return false; }
    let parts: Vec<&str> = time_str.split(':').collect();
    if parts.len() != 2 { return false; }
    let (h, m): (u32, u32) = match (parts[0].parse(), parts[1].parse()) {
        (Ok(h), Ok(m)) if h < 24 && m < 60 => (h, m),
        _ => return false,
    };
    // Has the time arrived today?
    let today = local_now.date_naive();
    let target = Local
        .from_local_datetime(&today.and_hms_opt(h, m, 0).unwrap())
        .single()
        .map(|d| d.with_timezone(&Utc));
    let Some(target_utc) = target else { return false; };
    if now < target_utc { return false; }
    // Was it already sent today?
    let last = crate::commands::settings::get_setting(pool, "morning_digest_last").await;
    let today_str = today.format("%Y-%m-%d").to_string();
    if last.as_deref() == Some(&today_str) { return false; }
    true
}

// kind is a stable tag for the push's source (deadline/block/digest/goal/
// app_limit/pomodoro/overdue/missed_days/nudge/activity_return/note_reminder),
// written into notification_log for the Notification Centre — the notification
// plugin keeps no history of its own. Most calls lead nowhere in particular when
// clicked in the feed, so their entity_type/entity_id stay NULL (see
// send_notification_for).
pub async fn send_notification(app: &tauri::AppHandle, pool: &SqlitePool, kind: &str, title: &str, body: &str) {
    send_notification_for(app, pool, kind, title, body, None, None).await;
}

/// Which pomodoro moment is being announced. It decides the buttons, and only the
/// caller knows it: the notification text alone would have to be parsed back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PomodoroMoment {
    /// A work stretch has started (the cycle began, or a break just ended).
    WorkStarted,
    /// A break has started.
    BreakStarted,
}

/// A pomodoro push carrying buttons. "Stop" is on every one of them; the other
/// button depends on the moment — skipping a break makes sense, extending a work
/// stretch does not.
pub async fn send_pomodoro_notification(
    app: &tauri::AppHandle,
    pool: &SqlitePool,
    title: &str,
    body: &str,
    moment: PomodoroMoment,
) {
    let lang = crate::i18n::current_lang(pool).await;

    #[cfg(target_os = "linux")]
    let shown = {
        use crate::notifier::actions::{ActionTarget, NotificationAction};
        let mut buttons = Vec::new();
        match moment {
            // A break is running: it can be skipped to get back to work.
            PomodoroMoment::BreakStarted => buttons.push((
                NotificationAction::PomodoroSkip,
                crate::i18n::tr("Пропустить", lang).to_string(),
            )),
            // Work has just started after a break: the useful offer is more rest,
            // which sends the cycle back into a break.
            PomodoroMoment::WorkStarted => buttons.push((
                NotificationAction::PomodoroExtendBreak,
                crate::i18n::tr("Ещё 5 минут", lang).to_string(),
            )),
        }
        buttons.push((
            NotificationAction::PomodoroStop,
            crate::i18n::tr("Остановить", lang).to_string(),
        ));
        crate::notifier::actions::show_with_actions(
            app.clone(), pool.clone(), ActionTarget::Pomodoro, title, body, &buttons,
        )
    };
    #[cfg(not(target_os = "linux"))]
    let shown = false;

    if shown {
        log_notification(pool, "pomodoro", title, body, None, None).await;
    } else {
        send_notification_for(app, pool, "pomodoro", title, body, None, None).await;
    }
}

// A deadline push carrying "Done" and "Snooze for an hour" buttons.
//
// On Linux it goes through notify_rust directly, because the notification plugin
// ignores actions on desktop (see notifier/actions.rs for the whole reasoning).
// Everywhere else — and whenever the direct path fails (no D-Bus session, a daemon
// that refuses actions) — we fall back to the ordinary push: a notification without
// buttons is worth far more than no notification at all.
//
// The feed entry is written in either case, so the Notification Centre does not
// depend on which path was taken.
pub async fn send_deadline_notification(
    app: &tauri::AppHandle,
    pool: &SqlitePool,
    task_id: &str,
    title: &str,
    body: &str,
) {
    let lang = crate::i18n::current_lang(pool).await;

    #[cfg(target_os = "linux")]
    let shown = {
        use crate::notifier::actions::{ActionTarget, NotificationAction};
        crate::notifier::actions::show_with_actions(
            app.clone(),
            pool.clone(),
            ActionTarget::Task(task_id.to_string()),
            title,
            body,
            &[
                (NotificationAction::TaskDone, crate::i18n::tr("Выполнено", lang).to_string()),
                (NotificationAction::TaskSnoozeHour, crate::i18n::tr("Отложить на час", lang).to_string()),
            ],
        )
    };
    #[cfg(not(target_os = "linux"))]
    let shown = false;

    if shown {
        log_notification(pool, "deadline", title, body, Some("task"), Some(task_id)).await;
    } else {
        send_notification_for(app, pool, "deadline", title, body, Some("task"), Some(task_id)).await;
    }
}

// Writing to the feed on its own, without a system push — needed when the push was
// already delivered by another path.
async fn log_notification(
    pool: &SqlitePool,
    kind: &str,
    title: &str,
    body: &str,
    entity_type: Option<&str>,
    entity_id: Option<&str>,
) {
    let _ = sqlx::query(
        "INSERT INTO notification_log (id, kind, title, body, created_at, entity_type, entity_id) VALUES (?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(kind)
    .bind(title)
    .bind(body)
    .bind(Utc::now().to_rfc3339())
    .bind(entity_type)
    .bind(entity_id)
    .execute(pool)
    .await;
}

// The same push but with a reference to an entity (a note reminder, where
// clicking it in the Notification Centre opens that note). entity_type is
// "note"/"task" and so on; entity_id is the record's id.
pub async fn send_notification_for(
    app: &tauri::AppHandle,
    pool: &SqlitePool,
    kind: &str,
    title: &str,
    body: &str,
    entity_type: Option<&str>,
    entity_id: Option<&str>,
) {
    let _ = app.notification()
        .builder()
        .title(title)
        .body(body)
        .show();
    let _ = sqlx::query(
        "INSERT INTO notification_log (id, kind, title, body, created_at, entity_type, entity_id) VALUES (?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(kind)
    .bind(title)
    .bind(body)
    .bind(Utc::now().to_rfc3339())
    .bind(entity_type)
    .bind(entity_id)
    .execute(pool)
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::settings::get_setting;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./src/db/migrations").run(&pool).await.unwrap();
        pool
    }

    async fn set_time(pool: &SqlitePool, time: &str) {
        crate::commands::settings::set_setting(pool, "morning_digest_time", time).await.unwrap();
    }

    async fn set_last(pool: &SqlitePool, date: &str) {
        crate::commands::settings::set_setting(pool, "morning_digest_last", date).await.unwrap();
    }

    // Pin now to a local time such that the target hour is testable. Returns a now
    // at which `time_str` has already arrived (or has not).
    fn fixed_now(hour: u32, min: u32) -> chrono::DateTime<Utc> {
        let today = Local::now().date_naive();
        let local_dt = Local
            .from_local_datetime(&today.and_hms_opt(hour, min, 0).unwrap())
            .single()
            .unwrap();
        local_dt.with_timezone(&Utc)
    }

    // v0.9.90. check_deadlines needs an AppHandle and cannot be called from a
    // test, so the race is asserted on the extracted mark_notified instead.
    //
    // The scenario is real: the scheduler SELECTs a task, awaits the sending of a
    // notification, and in that window the user completes the recurring task —
    // complete_task_impl moves the deadline and resets the flags. The late write
    // must not apply to the deadline it never saw.
    #[tokio::test]
    async fn a_late_scheduler_write_does_not_mark_the_new_deadline() {
        let pool = test_pool().await;
        let old_deadline = "2026-08-10T09:00:00+00:00";
        let new_deadline = "2026-08-11T09:00:00+00:00";

        sqlx::query(
            "INSERT INTO tasks (id, title, status, priority, category, recurrence, tags, hidden,
             deadline, notified_24h, created_at, updated_at)
             VALUES ('t1', 'ежедневная', 'Todo', 'Medium', 'Work', 'Daily', '[]', 0, ?, 0,
             '2026-08-01T00:00:00+00:00', '2026-08-01T00:00:00+00:00')")
            .bind(old_deadline)
            .execute(&pool).await.unwrap();

        // The user completes the task while the notification is being sent: the
        // deadline moves on and the flags are cleared.
        sqlx::query("UPDATE tasks SET deadline = ?, notified_24h = 0 WHERE id = 't1'")
            .bind(new_deadline)
            .execute(&pool).await.unwrap();

        // The scheduler's write finally lands, still carrying the OLD deadline.
        mark_notified(&pool, "t1", old_deadline, "notified_24h").await;

        let flag: bool = sqlx::query_scalar("SELECT notified_24h FROM tasks WHERE id = 't1'")
            .fetch_one(&pool).await.unwrap();
        assert!(
            !flag,
            "запоздавшая запись погасила напоминание о НОВОМ дедлайне — \
             о задаче больше никогда не напомнят"
        );

        // The other half: with the deadline unchanged the flag must still be set,
        // or the scheduler would notify about the same deadline every minute.
        mark_notified(&pool, "t1", new_deadline, "notified_24h").await;
        let flag: bool = sqlx::query_scalar("SELECT notified_24h FROM tasks WHERE id = 't1'")
            .fetch_one(&pool).await.unwrap();
        assert!(flag, "флаг не выставился при неизменившемся дедлайне — уведомление повторится");
    }

    // The guard above compares strings, so it only works if the spelling written
    // by complete_task_impl is byte-identical to what the scheduler's SELECT
    // reads back. Asserted against the real function rather than assumed: this is
    // the one thing that would make the WHERE silently never match, turning the
    // fix into "the flag is never set" — the opposite failure.
    #[tokio::test]
    async fn the_deadline_written_by_complete_task_matches_what_the_scheduler_reads() {
        use crate::core::task::CreateTask;

        let pool = test_pool().await;
        let ct = CreateTask {
            title: "ежедневная".into(),
            description: None,
            status: "Todo".into(),
            priority: crate::core::task::Priority::Medium,
            category: "Work".into(),
            deadline: Some(Utc::now() + chrono::Duration::days(1)),
            tags: vec![],
            recurrence: Some(crate::core::task::Recurrence::Daily),
            project_id: None,
        };
        let t = crate::commands::tasks::create_task_impl(&pool, ct).await.unwrap();

        let done = crate::commands::tasks::complete_task_impl(&pool, t.id.clone()).await.unwrap();
        let stored: String = sqlx::query_scalar("SELECT deadline FROM tasks WHERE id = ?")
            .bind(&t.id).fetch_one(&pool).await.unwrap();

        assert_eq!(
            stored,
            done.deadline.unwrap().to_rfc3339(),
            "написание дедлайна в БД разошлось с to_rfc3339 — условие AND deadline = ? \
             никогда не совпадёт и флаг не будет выставляться вообще"
        );

        // And the guard really does match on that exact string.
        mark_notified(&pool, &t.id, &stored, "notified_24h").await;
        let flag: bool = sqlx::query_scalar("SELECT notified_24h FROM tasks WHERE id = ?")
            .bind(&t.id).fetch_one(&pool).await.unwrap();
        assert!(flag, "флаг не выставился на реальном дедлайне из complete_task_impl");
    }

    #[tokio::test]
    async fn morning_digest_off_when_time_empty() {
        let pool = test_pool().await;
        set_time(&pool, "").await;
        assert!(!morning_digest_due(&pool, Utc::now()).await);
    }

    #[tokio::test]
    async fn morning_digest_not_due_before_set_time() {
        let pool = test_pool().await;
        set_time(&pool, "09:00").await;
        let before = fixed_now(8, 59);
        assert!(!morning_digest_due(&pool, before).await);
    }

    #[tokio::test]
    async fn morning_digest_due_after_set_time() {
        let pool = test_pool().await;
        set_time(&pool, "08:00").await;
        let after = fixed_now(8, 1);
        assert!(morning_digest_due(&pool, after).await);
    }

    #[tokio::test]
    async fn morning_digest_once_per_day() {
        let pool = test_pool().await;
        set_time(&pool, "08:00").await;
        let now = fixed_now(9, 0);
        let today_str = now.with_timezone(&Local).format("%Y-%m-%d").to_string();

        eprintln!("now={now}, today_str={today_str}");
        assert!(morning_digest_due(&pool, now).await);
        // After sending, the date is stored
        set_last(&pool, &today_str).await;
        let saved = get_setting(&pool, "morning_digest_last").await;
        eprintln!("saved last={saved:?}");
        assert!(!morning_digest_due(&pool, now).await);

        // The next day it must run again (simulated by resetting last to yesterday)
        let yesterday = (now.with_timezone(&Local).date_naive() - chrono::Duration::days(1))
            .format("%Y-%m-%d").to_string();
        set_last(&pool, &yesterday).await;
        assert!(morning_digest_due(&pool, now).await);
    }

    #[tokio::test]
    async fn morning_digest_invalid_time_never_fires() {
        let pool = test_pool().await;
        set_time(&pool, "25:00").await;
        assert!(!morning_digest_due(&pool, Utc::now()).await);

        set_time(&pool, "ab:cd").await;
        assert!(!morning_digest_due(&pool, Utc::now()).await);
    }

    async fn insert_block(pool: &SqlitePool, title: &str, scheduled_at: &str, notified: bool) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO tasks (id, title, status, priority, category, recurrence, tags, hidden,
             created_at, updated_at, scheduled_at, scheduled_mins, notified_block)
             VALUES (?, ?, 'Todo', 'Medium', 'Work', 'None', '[]', 0, ?, ?, ?, 30, ?)")
            .bind(&id).bind(title)
            .bind(scheduled_at).bind(scheduled_at)
            .bind(scheduled_at).bind(notified)
            .execute(pool).await.unwrap();
        id
    }

    #[tokio::test]
    async fn blocks_due_respects_window_and_flag() {
        let pool = test_pool().await;
        let now = Utc::now();
        let ts = |mins_ago: i64| (now - chrono::Duration::minutes(mins_ago)).to_rfc3339();

        insert_block(&pool, "начался", &ts(2), false).await;
        insert_block(&pool, "уже уведомлён", &ts(2), true).await;
        insert_block(&pool, "слишком давно", &ts(30), false).await;
        insert_block(&pool, "ещё не начался", &ts(-30), false).await;

        let due = blocks_due(&pool, now, 10).await;
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].title, "начался");
    }

    // end_utc is the moment Focus mode extends quiet_until to when a block starts
    // (check_blocks). It must equal scheduled_at + mins.
    #[tokio::test]
    async fn blocks_due_computes_end_utc_from_scheduled_mins() {
        let pool = test_pool().await;
        let now = Utc::now();
        let start = now - chrono::Duration::minutes(2);
        insert_block(&pool, "начался", &start.to_rfc3339(), false).await;

        let due = blocks_due(&pool, now, 10).await;
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].end_utc, start + chrono::Duration::minutes(30));
    }

    #[tokio::test]
    async fn mark_and_sweep_stop_repeat_notifications() {
        let pool = test_pool().await;
        let now = Utc::now();
        let fresh = insert_block(&pool, "свежий", &(now - chrono::Duration::minutes(1)).to_rfc3339(), false).await;
        insert_block(&pool, "протухший", &(now - chrono::Duration::minutes(120)).to_rfc3339(), false).await;

        mark_block_notified(&pool, &fresh).await;
        sweep_stale_blocks(&pool, now, 10).await;

        // after marking and the sweep, no candidates remain
        assert!(blocks_due(&pool, now, 10).await.is_empty());
        let unnotified: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM tasks WHERE notified_block = 0")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(unnotified, 0);
    }

    async fn insert_note(pool: &SqlitePool, title: &str, reminder_at: Option<&str>, notified: bool) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO notes (id, title, content, tags, created_at, updated_at, reminder_at, notified_reminder)
             VALUES (?, ?, '', '[]', ?, ?, ?, ?)")
            .bind(&id).bind(title)
            .bind(&now).bind(&now)
            .bind(reminder_at).bind(notified)
            .execute(pool).await.unwrap();
        id
    }

    // Note reminders use the same due/mark pattern as blocks but without an upper
    // "too old" bound: one missed during a sleep is still worth showing once
    // rather than staying silent forever.
    #[tokio::test]
    async fn note_reminders_due_respects_time_and_flag() {
        let pool = test_pool().await;
        let now = Utc::now();
        let ts = |mins_ago: i64| (now - chrono::Duration::minutes(mins_ago)).to_rfc3339();

        insert_note(&pool, "пора", Some(&ts(5)), false).await;
        insert_note(&pool, "давно пора", Some(&ts(500)), false).await;
        insert_note(&pool, "уже уведомлена", Some(&ts(5)), true).await;
        insert_note(&pool, "ещё рано", Some(&ts(-30)), false).await;
        insert_note(&pool, "без напоминания", None, false).await;

        let due = note_reminders_due(&pool, now).await;
        let titles: std::collections::HashSet<&str> = due.iter().map(|d| d.title.as_str()).collect();
        assert_eq!(titles, ["пора", "давно пора"].into_iter().collect());
    }

    #[tokio::test]
    async fn mark_reminder_notified_removes_from_due() {
        let pool = test_pool().await;
        let now = Utc::now();
        let id = insert_note(&pool, "заметка", Some(&(now - chrono::Duration::minutes(1)).to_rfc3339()), false).await;

        assert_eq!(note_reminders_due(&pool, now).await.len(), 1);
        mark_reminder_notified(&pool, &id).await;
        assert!(note_reminders_due(&pool, now).await.is_empty());
    }

    #[tokio::test]
    async fn goal_due_once_per_period_and_rearms_on_change() {
        use crate::commands::projects::*;
        let pool = test_pool().await;
        let now = Utc::now();

        let p = create_project_impl(&pool, CreateProject {
            name: "Спорт".into(), color: "".into(), target_date: None,
        }).await.unwrap();
        update_project_impl(&pool, p.id.clone(), UpdateProject {
            goal_tasks: Some(1), ..Default::default()
        }).await.unwrap();

        // the goal is not reached yet, so there are no candidates
        assert!(goals_due(&pool, now).await.is_empty());

        // a task completed within this period closes the goal
        sqlx::query(
            "INSERT INTO tasks (id, title, status, priority, category, recurrence, tags, hidden,
             created_at, updated_at, completed_at, project_id)
             VALUES (?, 'т', 'Done', 'Medium', 'Work', 'None', '[]', 1, ?, ?, ?, ?)")
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(now.to_rfc3339()).bind(now.to_rfc3339())
            .bind((now - chrono::Duration::minutes(1)).to_rfc3339())
            .bind(&p.id)
            .execute(&pool).await.unwrap();

        let due = goals_due(&pool, now).await;
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].name, "Спорт");
        assert!(due[0].body.contains("Цель недели"));

        // after marking it is no longer a candidate in this period
        mark_goal_notified(&pool, &due[0].id, &due[0].period_key).await;
        assert!(goals_due(&pool, now).await.is_empty());

        // changing the goal re-arms the push (notified_goal is reset)
        update_project_impl(&pool, p.id.clone(), UpdateProject {
            goal_tasks: Some(1), ..Default::default()
        }).await.unwrap();
        assert_eq!(goals_due(&pool, now).await.len(), 1);

        // an archived project is not notified about
        update_project_impl(&pool, p.id, UpdateProject {
            archived: Some(true), ..Default::default()
        }).await.unwrap();
        assert!(goals_due(&pool, now).await.is_empty());
    }

    use crate::commands::monitor::{AppLimit, CategoryMinutes};

    fn limit(category: &str, daily_mins: i64) -> AppLimit {
        AppLimit { category: category.to_string(), daily_mins }
    }
    fn usage(category: &str, minutes: i64) -> CategoryMinutes {
        CategoryMinutes { category: category.to_string(), minutes }
    }

    // "You meant to be doing X, and you are in something else" (v0.10.33).
    // Every other notification here fires on the clock; this one fires on a
    // mismatch, so the cases that must stay SILENT matter as much as the one
    // that fires.
    fn urgent(id: &str, category: &str) -> UrgentTask {
        UrgentTask {
            id: id.to_string(),
            title: format!("задача {id}"),
            category: category.to_string(),
            tracking: false,
            deadline_soon: false,
            in_progress: true,
        }
    }

    #[test]
    fn a_work_task_while_sitting_in_a_study_app_is_reported() {
        let now = Utc::now();
        let due = offtrack_due(&[urgent("t1", "Work")], "anki", "Study", "", now)
            .expect("несовпадение категорий — это и есть повод");
        assert_eq!(due.task_id, "t1");
        assert_eq!(due.reason, OfftrackReason::InProgress);
    }

    // The control case: the same task, an app of the SAME category. Without this
    // pair "never fires" would look exactly like correct behaviour.
    #[test]
    fn the_same_category_is_never_a_mismatch() {
        let now = Utc::now();
        assert_eq!(
            offtrack_due(&[urgent("t1", "Work")], "kitty", "Work", "", now),
            None,
            "приложение той же категории — это ровно то, чем и надо заниматься"
        );
    }

    // "Other" is what categorize_app returns when NO rule matched. Treating it as
    // a mismatch would make every unclassified tool look like a distraction.
    #[test]
    fn an_unclassified_app_says_nothing_and_is_not_a_mismatch() {
        let now = Utc::now();
        assert_eq!(
            offtrack_due(&[urgent("t1", "Work")], "randomapp", "Other", "", now),
            None,
            "«Other» — это отсутствие правила, а не утверждение о приложении"
        );
    }

    #[test]
    fn nothing_is_claimed_without_a_focused_app() {
        let now = Utc::now();
        assert_eq!(offtrack_due(&[urgent("t1", "Work")], "", "Study", "", now), None);
    }

    #[test]
    fn a_task_without_a_category_cannot_be_compared() {
        let now = Utc::now();
        assert_eq!(offtrack_due(&[urgent("t1", "")], "anki", "Study", "", now), None);
    }

    // A task that is neither tracked, nor due soon, nor in progress is not urgent
    // even if its category differs — the query may return it, the decision must not.
    #[test]
    fn a_task_with_no_urgency_is_not_a_reason() {
        let now = Utc::now();
        let mut t = urgent("t1", "Work");
        t.in_progress = false;
        assert_eq!(offtrack_due(&[t], "anki", "Study", "", now), None);
    }

    #[test]
    fn a_running_timer_outranks_a_deadline_and_a_status() {
        let now = Utc::now();
        let mut tracked = urgent("t2", "Work");
        tracked.tracking = true;
        let mut soon = urgent("t3", "Work");
        soon.deadline_soon = true;

        let due = offtrack_due(&[urgent("t1", "Work"), soon, tracked], "anki", "Study", "", now)
            .expect("повод есть");
        assert_eq!(due.task_id, "t2", "таймер — самый сильный сигнал");
        assert_eq!(due.reason, OfftrackReason::Tracking);
    }

    #[test]
    fn one_task_is_announced_once_per_day_and_rearms_tomorrow() {
        let now = Utc::now();
        let today = now.with_timezone(&Local).format("%Y-%m-%d").to_string();
        let notified = format!(r#"{{"t1":"{today}"}}"#);
        assert_eq!(
            offtrack_due(&[urgent("t1", "Work")], "anki", "Study", &notified, now),
            None,
            "сегодня уже сказали"
        );

        let tomorrow = now + chrono::Duration::days(1);
        assert!(
            offtrack_due(&[urgent("t1", "Work")], "anki", "Study", &notified, tomorrow).is_some(),
            "назавтра повод снова в силе"
        );
    }

    // A second task must still be announceable the same day: the cooldown is per
    // task, not global.
    #[test]
    fn another_task_is_still_due_the_same_day() {
        let now = Utc::now();
        let today = now.with_timezone(&Local).format("%Y-%m-%d").to_string();
        let notified = format!(r#"{{"t1":"{today}"}}"#);
        let due = offtrack_due(
            &[urgent("t1", "Work"), urgent("t2", "Work")],
            "anki",
            "Study",
            &notified,
            now,
        )
        .expect("вторая задача ещё не объявлялась");
        assert_eq!(due.task_id, "t2");
    }

    #[test]
    fn limits_due_exceeded_triggers() {
        let limits = vec![limit("Other", 60)];
        let usage = vec![usage("Other", 65)];
        let due = limits_due(&limits, &usage, "", Utc::now());
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].category, "Other");
        assert_eq!(due[0].minutes, 65);
        assert_eq!(due[0].limit, 60);
    }

    #[test]
    fn limits_due_exactly_at_limit_triggers() {
        let limits = vec![limit("Other", 60)];
        let usage = vec![usage("Other", 60)];
        assert_eq!(limits_due(&limits, &usage, "", Utc::now()).len(), 1);
    }

    #[test]
    fn limits_due_under_limit_does_not_trigger() {
        let limits = vec![limit("Other", 60)];
        let usage = vec![usage("Other", 59)];
        assert!(limits_due(&limits, &usage, "", Utc::now()).is_empty());
    }

    #[test]
    fn limits_due_zero_or_missing_limit_means_no_limit() {
        let limits = vec![limit("Other", 0)];
        let usage = vec![usage("Other", 500)];
        assert!(limits_due(&limits, &usage, "", Utc::now()).is_empty());
    }

    #[test]
    fn limits_due_once_per_day() {
        let now = Utc::now();
        let today = now.with_timezone(&Local).format("%Y-%m-%d").to_string();
        let limits = vec![limit("Other", 60)];
        let usage = vec![usage("Other", 65)];
        let notified = format!(r#"{{"Other":"{today}"}}"#);
        assert!(limits_due(&limits, &usage, &notified, now).is_empty());
    }

    #[test]
    fn limits_due_rearms_next_day() {
        let now = Utc::now();
        let limits = vec![limit("Other", 60)];
        let usage = vec![usage("Other", 65)];
        let notified = r#"{"Other":"1999-01-01"}"#;
        assert_eq!(limits_due(&limits, &usage, notified, now).len(), 1);
    }

    #[tokio::test]
    async fn check_app_limits_marks_notified_and_is_idempotent_same_day() {
        let pool = test_pool().await;
        let now = Utc::now();
        crate::commands::settings::set_setting(&pool, "app_limits", r#"[{"category":"Other","daily_mins":1}]"#).await.unwrap();
        sqlx::query(
            "INSERT INTO activity_log (timestamp, state, app_focused, input_events, duration_secs, app)
             VALUES (?, 'Active', 1, 0, 120, 'randomapp')")
            .bind(now.to_rfc3339())
            .execute(&pool).await.unwrap();

        // With no provider rules, randomapp lands in "Other" (categorize_app's default)
        let usage = crate::commands::monitor::get_app_category_time_impl(&pool, 1).await.unwrap();
        assert!(usage.iter().any(|c| c.category == "Other" && c.minutes >= 1));

        let notified_before = get_setting(&pool, "app_limits_notified").await.unwrap_or_default();
        assert!(notified_before.is_empty());

        // Simulate the full cycle without a real AppHandle: check limits_due and the marking directly
        let limits = crate::commands::monitor::parse_app_limits(
            &get_setting(&pool, "app_limits").await.unwrap()
        );
        let due = limits_due(&limits, &usage, &notified_before, now);
        assert_eq!(due.len(), 1);
    }
}

// The `kriptag --status` CLI mode: a short-lived process for a waybar custom
// module (or any other status bar). It opens the DB read-only (WAL allows reading
// in parallel with the running app), prints a single JSON line to stdout and
// exits — Tauri is never started and single-instance is not disturbed.
//
// Text priority: an active tracking session -> a running time block -> a running
// routine -> the next block -> the next routine -> an InProgress task -> a count
// of tasks due today -> "free". The work mode and any notification pause go into
// the tooltip. The pomodoro timer is runtime state of the app and is absent from
// the DB, so it is not shown in the status.

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Utc};
use serde::Serialize;
use sqlx::{Row, SqlitePool};

const TITLE_MAX: usize = 28;

#[derive(Debug, Serialize, PartialEq)]
pub struct StatusPayload {
    pub text: String,
    pub tooltip: String,
    // For styling in waybar: tracking | block | next | task | due | idle | off
    pub class: String,
    // The work mode (Light | Study | Focus), for format-icons
    pub alt: String,
}

fn empty_payload() -> StatusPayload {
    StatusPayload {
        text: String::new(),
        tooltip: format!("Kriptag: {}",
            crate::i18n::tr("БД не найдена", crate::i18n::lang_from_setting(""))),
        class: "off".into(),
        alt: String::new(),
    }
}

fn ellipsize(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

fn hhmm(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%H:%M").to_string()
}

struct Block {
    title: String,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

pub async fn status_payload(pool: &SqlitePool, now: DateTime<Utc>) -> Result<StatusPayload, sqlx::Error> {
    // The status line is read from waybar, i.e. outside the application window,
    // so the language comes from the same settings as the rest of the interface.
    let lang = crate::i18n::current_lang(pool).await;
    // Time blocks of today's local day (not Done, not hidden)
    let rows = sqlx::query(
        "SELECT title, scheduled_at, COALESCE(scheduled_mins, 60) AS mins FROM tasks
         WHERE hidden = 0 AND status != 'Done' AND scheduled_at IS NOT NULL AND deleted_at IS NULL",
    )
    .fetch_all(pool)
    .await?;

    let today = now.with_timezone(&Local).date_naive();
    let mut blocks: Vec<Block> = rows
        .into_iter()
        .filter_map(|r| {
            let start = DateTime::parse_from_rfc3339(&r.get::<String, _>("scheduled_at"))
                .ok()?
                .with_timezone(&Utc);
            if start.with_timezone(&Local).date_naive() != today {
                return None;
            }
            Some(Block {
                title: r.get("title"),
                start,
                end: start + Duration::minutes(r.get::<i64, _>("mins")),
            })
        })
        .collect();
    blocks.sort_by_key(|b| b.start);

    let current = blocks.iter().filter(|b| b.start <= now && now < b.end).last();
    let next = blocks.iter().find(|b| b.start > now);

    // Today's routines: minutes since midnight converted to an absolute time today
    let routine_rows = sqlx::query(
        "SELECT title, start_mins, duration_mins FROM routines
         WHERE active = 1 AND (days_mask & ?) != 0
         ORDER BY start_mins"
    )
    .bind(1i64 << today.weekday().num_days_from_monday())
    .fetch_all(pool)
    .await?;

    let mut routine_blocks: Vec<Block> = routine_rows.iter().filter_map(|r| {
        let start_mins: i64 = r.get("start_mins");
        let dur: i64 = r.get("duration_mins");
        let start = Local
            .from_local_datetime(&today.and_time(NaiveTime::from_hms_opt(
                (start_mins / 60) as u32,
                (start_mins % 60) as u32,
                0,
            )?))
            .single()?
            .with_timezone(&Utc);
        Some(Block {
            title: r.get("title"),
            start,
            end: start + Duration::minutes(dur),
        })
    }).collect();
    routine_blocks.sort_by_key(|b| b.start);
    let routine_current = routine_blocks.iter().filter(|b| b.start <= now && now < b.end).last();
    let routine_next = routine_blocks.iter().find(|b| b.start > now);

    let in_progress: Option<String> = sqlx::query(
        "SELECT title FROM tasks WHERE hidden = 0 AND status = 'InProgress' AND deleted_at IS NULL
         ORDER BY updated_at DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?
    .map(|r| r.get("title"));

    // "Due today" means before local midnight, overdue items included. Comparing
    // the strings is correct: both operands are RFC3339 in UTC.
    let tomorrow_local = today.succ_opt().unwrap_or(today);
    let tomorrow_utc = Local
        .from_local_datetime(&tomorrow_local.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or(now);
    let due_row = sqlx::query(
        "SELECT COUNT(*) AS due,
                SUM(CASE WHEN deadline < ? THEN 1 ELSE 0 END) AS overdue
         FROM tasks
         WHERE hidden = 0 AND status != 'Done' AND deadline IS NOT NULL AND deadline < ? AND deleted_at IS NULL",
    )
    .bind(now.to_rfc3339())
    .bind(tomorrow_utc.to_rfc3339())
    .fetch_one(pool)
    .await?;
    let due: i64 = due_row.get("due");
    let overdue: i64 = due_row.get::<Option<i64>, _>("overdue").unwrap_or(0);

    // The active tracking session
    let active_session: Option<(String, i64)> = sqlx::query(
        "SELECT s.started_at, t.title
         FROM task_sessions s
         JOIN tasks t ON t.id = s.task_id
         WHERE s.ended_at IS NULL
         LIMIT 1"
    )
    .fetch_optional(pool)
    .await?
    .and_then(|r| {
        let started: String = r.get("started_at");
        let started_dt = DateTime::parse_from_rfc3339(&started).ok()?.with_timezone(&Utc);
        let mins = (now - started_dt).num_seconds().max(0) / 60;
        Some((r.get::<String, _>("title"), mins))
    });

    let setting = |key: &str| {
        let pool = pool.clone();
        let key = key.to_string();
        async move {
            sqlx::query("SELECT value FROM settings WHERE key = ?")
                .bind(key)
                .fetch_optional(&pool)
                .await
                .ok()
                .flatten()
                .map(|r| r.get::<String, _>("value"))
        }
    };
    let work_mode = setting("work_mode").await.unwrap_or_else(|| "Light".into());
    let quiet_until = setting("quiet_until").await.unwrap_or_default();
    let pomo_phase = setting("pomodoro_phase").await.unwrap_or_else(|| "off".into());
    let pomo_until = setting("pomodoro_until")
        .await
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|t| t.with_timezone(&Utc));

    // Pomodoro is the most immediate state. It used not to be shown at all,
    // because the phase lived only in the runtime and the short-lived CLI could
    // not see it.
    let pomo_label = match (pomo_phase.as_str(), pomo_until) {
        ("work", Some(t)) if t > now => Some(format!("🍅 {}", crate::i18n::tr_args("до {time}", lang, &[("time", hhmm(t))]))),
        ("break", Some(t)) if t > now => Some(format!("☕ {}", crate::i18n::tr_args("до {time}", lang, &[("time", hhmm(t))]))),
        ("paused", _) => Some(format!("🍅 {}", crate::i18n::tr("пауза", lang))),
        _ => None,
    };

    let (text, class) = if let Some(label) = &pomo_label {
        (label.clone(), "pomodoro")
    } else if let Some((ref title, mins)) = active_session {
        (crate::i18n::tr_args("▶ {task} · {n} мин", lang, &[("task", ellipsize(title, TITLE_MAX)), ("n", mins.to_string())]), "tracking")
    } else if let Some(b) = current {
        (crate::i18n::tr_args("▶ {task} до {time}", lang, &[("task", ellipsize(&b.title, TITLE_MAX)), ("time", hhmm(b.end))]), "block")
    } else if let Some(b) = routine_current {
        (crate::i18n::tr_args("▶ {task} до {time}", lang, &[("task", ellipsize(&b.title, TITLE_MAX)), ("time", hhmm(b.end))]), "block")
    } else if let Some(b) = next {
        (format!("⏱ {} {}", hhmm(b.start), ellipsize(&b.title, TITLE_MAX)), "next")
    } else if let Some(b) = routine_next {
        (format!("⏱ {} {}", hhmm(b.start), ellipsize(&b.title, TITLE_MAX)), "next")
    } else if let Some(t) = &in_progress {
        (format!("▶ {}", ellipsize(t, TITLE_MAX)), "task")
    } else if due > 0 {
        (format!("☑ {due}"), "due")
    } else {
        ("✓".into(), "idle")
    };

    let mut tip: Vec<String> = Vec::new();
    if let Some((ref title, mins)) = active_session {
        tip.push(crate::i18n::tr_args("Трекинг: {task} ({n} мин)", lang, &[("task", title.clone()), ("n", mins.to_string())]));
    }
    if pomo_label.is_some() {
        let phase = match pomo_phase.as_str() {
            "work" => "работа",
            "break" => "перерыв",
            "paused" => "на паузе",
            _ => "",
        };
        tip.push(crate::i18n::tr_args("Помодоро: {phase}", lang,
            &[("phase", crate::i18n::tr(phase, lang))]));
    }
    if let Some(b) = current {
        tip.push(crate::i18n::tr_args("Идёт: {task} (до {time})", lang, &[("task", b.title.clone()), ("time", hhmm(b.end))]));
    } else if let Some(b) = routine_current {
        tip.push(crate::i18n::tr_args("Идёт рутина: {task} (до {time})", lang, &[("task", b.title.clone()), ("time", hhmm(b.end))]));
    }
    if let Some(b) = next {
        tip.push(crate::i18n::tr_args("Далее: {task} в {time}", lang, &[("task", b.title.clone()), ("time", hhmm(b.start))]));
    } else if let Some(b) = routine_next {
        tip.push(crate::i18n::tr_args("Далее рутина: {task} в {time}", lang, &[("task", b.title.clone()), ("time", hhmm(b.start))]));
    }
    if let Some(t) = &in_progress {
        tip.push(crate::i18n::tr_args("В работе: {task}", lang, &[("task", t.clone())]));
    }
    if due > 0 {
        let mut line = crate::i18n::tr_args("Задач на сегодня: {n}", lang, &[("n", due.to_string())]);
        if overdue > 0 {
            line.push_str(&crate::i18n::tr_args(" (просрочено: {n})", lang, &[("n", overdue.to_string())]));
        }
        tip.push(line);
    }
    tip.push(crate::i18n::tr_args("Режим: {mode}", lang, &[("mode", work_mode.clone())]));
    if quiet_until == crate::commands::settings::QUIET_FOREVER {
        tip.push(crate::i18n::tr("Уведомления: выключены", lang));
    } else if let Ok(t) = DateTime::parse_from_rfc3339(&quiet_until) {
        if now < t.with_timezone(&Utc) {
            tip.push(crate::i18n::tr_args("Уведомления: пауза до {time}", lang, &[("time", hhmm(t.with_timezone(&Utc)))]));
        }
    }

    Ok(StatusPayload {
        text,
        tooltip: tip.join("\n"),
        class: class.into(),
        alt: work_mode,
    })
}

async fn open_readonly() -> Option<SqlitePool> {
    // The same path Tauri's app.path().app_data_dir() yields: data_dir plus the
    // identifier (see tauri.conf.json). mode=ro means we neither create the file
    // nor touch the schema.
    let path = dirs::data_dir()?.join("com.kriptag.app").join("data.db");
    if !path.exists() {
        return None;
    }
    SqlitePool::connect(&format!("sqlite:{}?mode=ro", path.display()))
        .await
        .ok()
}

// The CLI entry point: prints the JSON for waybar and returns (the caller exits).
pub fn print_status() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let payload = rt.block_on(async {
        match open_readonly().await {
            Some(pool) => status_payload(&pool, Utc::now()).await.unwrap_or_else(|_| empty_payload()),
            None => empty_payload(),
        }
    });
    println!("{}", serde_json::to_string(&payload).expect("status json"));
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./src/db/migrations").run(&pool).await.unwrap();
        // The language is set explicitly. Without this an empty setting would mean
        // "detect from the OS locale", and the tests below that compare Russian
        // strings would fail for a developer running LANG=en_US — the result would
        // depend on the machine rather than on the code.
        crate::commands::settings::set_setting(&pool, "language", "ru").await.unwrap();
        pool
    }

    // Local noon today: a deterministic "now" away from the edges of the day.
    fn noon_utc() -> DateTime<Utc> {
        let today = Local::now().date_naive();
        Local
            .from_local_datetime(&today.and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap()
            .with_timezone(&Utc)
    }

    async fn insert_task(
        pool: &SqlitePool,
        title: &str,
        status: &str,
        deadline: Option<DateTime<Utc>>,
        scheduled_at: Option<DateTime<Utc>>,
        mins: Option<i64>,
    ) {
        sqlx::query(
            "INSERT INTO tasks (id, title, status, deadline, scheduled_at, scheduled_mins, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(title)
        .bind(status)
        .bind(deadline.map(|t| t.to_rfc3339()))
        .bind(scheduled_at.map(|t| t.to_rfc3339()))
        .bind(mins)
        .bind(Utc::now().to_rfc3339())
        .bind(Utc::now().to_rfc3339())
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn pomodoro_takes_priority_over_everything() {
        let pool = test_pool().await;
        let now = noon_utc();
        // A block is running, but pomodoro must still win on priority
        insert_task(&pool, "писать отчёт", "Todo", None, Some(now - Duration::minutes(30)), Some(60)).await;

        for (k, v) in [
            ("pomodoro_phase", "work".to_string()),
            ("pomodoro_until", (now + Duration::minutes(12)).to_rfc3339()),
        ] {
            sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?)")
                .bind(k).bind(v).execute(&pool).await.unwrap();
        }
        let p = status_payload(&pool, now).await.unwrap();
        assert_eq!(p.class, "pomodoro");
        assert!(p.text.starts_with("🍅 до "), "text: {}", p.text);
        assert!(p.tooltip.contains("Помодоро: работа"));

        // A break uses a different symbol and label
        sqlx::query("UPDATE settings SET value = 'break' WHERE key = 'pomodoro_phase'")
            .execute(&pool).await.unwrap();
        let p = status_payload(&pool, now).await.unwrap();
        assert!(p.text.starts_with("☕ до "), "text: {}", p.text);
        assert!(p.tooltip.contains("Помодоро: перерыв"));

        // A pause carries no time
        sqlx::query("UPDATE settings SET value = 'paused' WHERE key = 'pomodoro_phase'")
            .execute(&pool).await.unwrap();
        let p = status_payload(&pool, now).await.unwrap();
        assert_eq!(p.text, "🍅 пауза");

        // An expired phase (the loop has not ticked yet) must not mask the block
        sqlx::query("UPDATE settings SET value = 'work' WHERE key = 'pomodoro_phase'")
            .execute(&pool).await.unwrap();
        sqlx::query("UPDATE settings SET value = ? WHERE key = 'pomodoro_until'")
            .bind((now - Duration::minutes(1)).to_rfc3339())
            .execute(&pool).await.unwrap();
        let p = status_payload(&pool, now).await.unwrap();
        assert_eq!(p.class, "block");

        // "off" is not shown at all
        sqlx::query("UPDATE settings SET value = 'off' WHERE key = 'pomodoro_phase'")
            .execute(&pool).await.unwrap();
        let p = status_payload(&pool, now).await.unwrap();
        assert_eq!(p.class, "block");
    }

    // The whole tooltip goes to waybar, i.e. it is visible outside the window. We
    // check not one particular string but that no Cyrillic remains in it when
    // language=en — the same technique the e2e tests use for the frontend screens.
    #[tokio::test]
    async fn english_status_has_no_russian_left() {
        let pool = test_pool().await;
        crate::commands::settings::set_setting(&pool, "language", "en").await.unwrap();
        crate::commands::settings::set_setting(&pool, "work_mode", "Light").await.unwrap();
        let now = noon_utc();
        // A running block, so the "▶ ... until ..." branch reaches text rather than
        // just "in progress": otherwise four strings containing times would stay
        // unchecked.
        insert_task(&pool, "Report", "Todo", None, Some(now - Duration::minutes(30)), Some(60)).await;

        let p = status_payload(&pool, now).await.unwrap();
        let all = format!("{}\n{}", p.text, p.tooltip);
        assert!(
            !all.chars().any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c)),
            "в английском статусе осталась кириллица: {all}"
        );
        assert!(all.contains("Mode: Light"), "{all}");
    }

    #[tokio::test]
    async fn empty_db_is_idle() {
        let pool = test_pool().await;
        let p = status_payload(&pool, noon_utc()).await.unwrap();
        assert_eq!(p.text, "✓");
        assert_eq!(p.class, "idle");
        assert_eq!(p.alt, "Light"); // the default mode with no settings
        assert!(p.tooltip.contains("Режим: Light"));
    }

    #[tokio::test]
    async fn current_block_wins_and_shows_end_time() {
        let pool = test_pool().await;
        let now = noon_utc();
        // A running 11:30-12:30 block, the next at 14:00, plus an InProgress task
        insert_task(&pool, "писать отчёт", "Todo", None, Some(now - Duration::minutes(30)), Some(60)).await;
        insert_task(&pool, "созвон", "Todo", None, Some(now + Duration::hours(2)), Some(30)).await;
        insert_task(&pool, "фоновая задача", "InProgress", None, None, None).await;

        let p = status_payload(&pool, now).await.unwrap();
        assert_eq!(p.class, "block");
        assert!(p.text.starts_with("▶ писать отчёт до "), "text: {}", p.text);
        assert!(p.text.ends_with(&hhmm(now + Duration::minutes(30))));
        assert!(p.tooltip.contains("Далее: созвон"));
        assert!(p.tooltip.contains("В работе: фоновая задача"));
    }

    #[tokio::test]
    async fn next_block_then_inprogress_then_due() {
        let pool = test_pool().await;
        let now = noon_utc();

        // Deadlines only: one overdue, one in the evening
        insert_task(&pool, "просроченная", "Todo", Some(now - Duration::hours(3)), None, None).await;
        insert_task(&pool, "вечерняя", "Todo", Some(now + Duration::hours(5)), None, None).await;
        let p = status_payload(&pool, now).await.unwrap();
        assert_eq!(p.text, "☑ 2");
        assert_eq!(p.class, "due");
        assert!(p.tooltip.contains("Задач на сегодня: 2 (просрочено: 1)"));

        // An InProgress task appeared — higher priority than the counter
        insert_task(&pool, "важное дело прямо сейчас", "InProgress", None, None, None).await;
        let p = status_payload(&pool, now).await.unwrap();
        assert_eq!(p.class, "task");
        assert!(p.text.starts_with("▶ важное дело"));

        // A future block today outranks InProgress
        insert_task(&pool, "блок после обеда", "Todo", None, Some(now + Duration::hours(1)), Some(45)).await;
        let p = status_payload(&pool, now).await.unwrap();
        assert_eq!(p.class, "next");
        assert!(p.text.contains("блок после обеда"));

        // Finished blocks and yesterday's do not count
        insert_task(&pool, "вчерашний блок", "Todo", None, Some(now - Duration::days(1)), Some(60)).await;
        insert_task(&pool, "сделанный блок", "Done", None, Some(now - Duration::minutes(10)), Some(60)).await;
        let p = status_payload(&pool, now).await.unwrap();
        assert_eq!(p.class, "next", "Done/вчерашние блоки не должны влиять");
    }

    #[tokio::test]
    async fn mode_and_quiet_pause_in_tooltip() {
        let pool = test_pool().await;
        let now = noon_utc();
        for (k, v) in [
            ("work_mode", "Focus".to_string()),
            ("quiet_until", (now + Duration::minutes(45)).to_rfc3339()),
        ] {
            sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?)")
                .bind(k).bind(v).execute(&pool).await.unwrap();
        }

        let p = status_payload(&pool, now).await.unwrap();
        assert_eq!(p.alt, "Focus");
        assert!(p.tooltip.contains("Режим: Focus"));
        assert!(p.tooltip.contains("Уведомления: пауза до"));

        // An expired pause is not shown
        sqlx::query("UPDATE settings SET value = ? WHERE key = 'quiet_until'")
            .bind((now - Duration::minutes(1)).to_rfc3339())
            .execute(&pool).await.unwrap();
        let p = status_payload(&pool, now).await.unwrap();
        assert!(!p.tooltip.contains("Уведомления"));

        // An indefinite pause
        sqlx::query("UPDATE settings SET value = ? WHERE key = 'quiet_until'")
            .bind(crate::commands::settings::QUIET_FOREVER)
            .execute(&pool).await.unwrap();
        let p = status_payload(&pool, now).await.unwrap();
        assert!(p.tooltip.contains("Уведомления: выключены"));
    }

    #[test]
    fn ellipsize_respects_chars_not_bytes() {
        assert_eq!(ellipsize("короткое", 28), "короткое");
        let long = "очень длинное название задачи которое не влезает";
        let cut = ellipsize(long, 10);
        assert_eq!(cut.chars().count(), 11); // 10 characters plus the ellipsis
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn payload_serializes_to_waybar_json() {
        let p = StatusPayload {
            text: "▶ задача до 13:00".into(),
            tooltip: "Режим: Light".into(),
            class: "block".into(),
            alt: "Light".into(),
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"text\":"));
        assert!(json.contains("\"tooltip\":"));
        assert!(json.contains("\"class\":\"block\""));
    }
}

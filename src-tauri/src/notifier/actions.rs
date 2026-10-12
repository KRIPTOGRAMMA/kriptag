// Buttons on notifications — acting without opening the application.
//
// Three kinds carry them: a task deadline ("Done" / "Snooze for an hour"), a note
// reminder (the same pair, applied to the note) and pomodoro ("Skip" or "Five more
// minutes", plus "Stop" on every one).
//
// tauri-plugin-notification accepts actions in its builder but ignores them
// entirely on desktop: verified against the plugin's own code (2.3.3,
// desktop.rs::show() passes only title/body/icon/sound into notify_rust, never
// calls .action() and never subscribes to the D-Bus action signal). The lower
// layer supports the freedesktop protocol, so the buttons are only reachable by
// calling notify_rust directly.
//
// That makes this the first code in the project bypassing an official Tauri plugin
// rather than extending it, and the trade-off is deliberate: the buttons exist on
// Linux and nowhere else. Every other platform keeps the plugin path unchanged (see
// send_notification_for), so nothing regresses there — it is the same capability
// detection the project already uses for window tracking and ext-idle-notify.
//
// Waiting for a click is blocking (notify_rust::Notification::show() returns a
// handle whose wait_for_action consumes the current thread), so each notification
// with actions gets its own detached thread. The thread ends when the user clicks
// or the notification expires.

/// What the user pressed on a notification.
///
/// One enum for every kind of notification rather than one per kind: the daemon
/// hands back a bare action id with no hint of what it belonged to, so the id has
/// to identify the action on its own. Keeping them in a single list is what makes
/// a clash visible — "snooze" alone would have meant two different things for a
/// task and for a note.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum NotificationAction {
    /// Deadline: complete the task.
    TaskDone,
    /// Deadline: push it an hour forward.
    TaskSnoozeHour,
    /// Note reminder: drop the reminder, it has served its purpose.
    NoteDone,
    /// Note reminder: remind again in an hour.
    NoteSnoozeHour,
    /// Pomodoro: skip the phase being announced.
    PomodoroSkip,
    /// Pomodoro: stay on a break for another five minutes.
    PomodoroExtendBreak,
    /// Pomodoro: end the cycle entirely.
    PomodoroStop,
}

/// Applies the action to a task. Kept separate from the D-Bus plumbing so the
/// behaviour can be unit-tested: the plumbing needs a live session bus, this does
/// not.
///
/// "Snooze" shifts the deadline by an hour from now rather than from the old
/// deadline: the point is "remind me in an hour", and for an already-overdue task
/// shifting from the old value could land in the past again and fire immediately.
///
/// The notified_* flags are reset because they refer to the previous deadline —
/// the same reasoning as update_task, where without a reset the scheduler would
/// never notify about the new date.
pub async fn apply_deadline_action(
    pool: &sqlx::SqlitePool,
    task_id: &str,
    action: NotificationAction,
) -> Result<(), String> {
    match action {
        NotificationAction::TaskDone => {
            // tasks.rs returns AppError; this module keeps its String contract, so
            // the error is flattened here rather than migrating the notifier too.
            crate::commands::tasks::complete_task_impl(pool, task_id.to_string())
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        NotificationAction::TaskSnoozeHour => {
            let next = chrono::Utc::now() + chrono::Duration::hours(1);
            sqlx::query(
                "UPDATE tasks
                 SET deadline = ?, notified_24h = 0, notified_1h = 0, notified_deadline = 0
                 WHERE id = ?",
            )
            .bind(next.to_rfc3339())
            .bind(task_id)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
            Ok(())
        }
        // The other actions belong to other kinds of notification and never reach
        // this function; treating them as an error rather than silently doing
        // nothing means a wiring mistake shows up instead of hiding.
        other => Err(format!("действие {other:?} не относится к дедлайну")),
    }
}

/// Applies an action from a note reminder.
///
/// "Done" clears the reminder date instead of only marking it notified: the
/// reminder has done its job, and leaving the date would make it fire again after
/// any edit that resets the flag.
///
/// "Snooze" moves the date an hour ahead and clears the flag — the same reasoning
/// as for a deadline, and for the same reason it counts from now rather than from
/// the old date.
pub async fn apply_note_action(
    pool: &sqlx::SqlitePool,
    note_id: &str,
    action: NotificationAction,
) -> Result<(), String> {
    match action {
        NotificationAction::NoteDone => {
            sqlx::query("UPDATE notes SET reminder_at = NULL, notified_reminder = 0 WHERE id = ?")
                .bind(note_id)
                .execute(pool)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        NotificationAction::NoteSnoozeHour => {
            let next = chrono::Utc::now() + chrono::Duration::hours(1);
            sqlx::query("UPDATE notes SET reminder_at = ?, notified_reminder = 0 WHERE id = ?")
                .bind(next.to_rfc3339())
                .bind(note_id)
                .execute(pool)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        other => Err(format!("действие {other:?} не относится к напоминанию заметки")),
    }
}

/// Applies an action from a pomodoro notification.
///
/// These go through the cycle's command channel rather than the database: the
/// loop owns the phase and the remaining seconds, and writing the state from the
/// outside would be overwritten on its next tick.
pub fn apply_pomodoro_action(
    tx: &tokio::sync::mpsc::UnboundedSender<crate::notifier::pomodoro::PomodoroCmd>,
    action: NotificationAction,
) -> Result<(), String> {
    use crate::notifier::pomodoro::PomodoroCmd;
    let cmd = match action {
        NotificationAction::PomodoroSkip => PomodoroCmd::Skip,
        NotificationAction::PomodoroStop => PomodoroCmd::Stop,
        NotificationAction::PomodoroExtendBreak => PomodoroCmd::ExtendBreak(5),
        other => return Err(format!("действие {other:?} не относится к помодоро")),
    };
    tx.send(cmd).map_err(|e| e.to_string())
}

/// Maps the freedesktop action id back to an action. Unknown ids yield None:
/// the notification daemon may report "default" (the body was clicked) or
/// "__closed", and neither must be mistaken for a button press.
///
/// The ids carry their kind as a prefix. The daemon returns nothing but the id,
/// so a bare "snooze" would be ambiguous the moment a second kind of notification
/// grew a snooze button — which is exactly what happened here.
pub fn action_from_id(id: &str) -> Option<NotificationAction> {
    match id {
        "task_done" => Some(NotificationAction::TaskDone),
        "task_snooze" => Some(NotificationAction::TaskSnoozeHour),
        "note_done" => Some(NotificationAction::NoteDone),
        "note_snooze" => Some(NotificationAction::NoteSnoozeHour),
        "pomo_skip" => Some(NotificationAction::PomodoroSkip),
        "pomo_extend" => Some(NotificationAction::PomodoroExtendBreak),
        "pomo_stop" => Some(NotificationAction::PomodoroStop),
        _ => None,
    }
}

/// The id an action is published under. The inverse of action_from_id; a test
/// checks the round trip, so the two lists cannot drift apart.
pub fn id_for_action(action: NotificationAction) -> &'static str {
    match action {
        NotificationAction::TaskDone => "task_done",
        NotificationAction::TaskSnoozeHour => "task_snooze",
        NotificationAction::NoteDone => "note_done",
        NotificationAction::NoteSnoozeHour => "note_snooze",
        NotificationAction::PomodoroSkip => "pomo_skip",
        NotificationAction::PomodoroExtendBreak => "pomo_extend",
        NotificationAction::PomodoroStop => "pomo_stop",
    }
}

/// What a pressed action applies to: the row it changes, if any.
///
/// Pomodoro carries no id — the cycle is a single global thing, unlike a task or
/// a note, which are rows.
#[derive(Clone)]
pub enum ActionTarget {
    Task(String),
    Note(String),
    Pomodoro,
}

/// Shows a deadline notification carrying buttons and waits for a click in a
/// separate thread.
///
/// Returns false when the notification could not be shown (no D-Bus session, a
/// daemon that rejects actions), so the caller can fall back to the plugin instead
/// of leaving the user with no notification at all.
#[cfg(target_os = "linux")]
pub fn show_with_actions(
    app: tauri::AppHandle,
    pool: sqlx::SqlitePool,
    target: ActionTarget,
    title: &str,
    body: &str,
    buttons: &[(NotificationAction, String)],
    // Passed in rather than read here: this function is synchronous (it blocks
    // waiting for a click) and the setting lives behind an async query. The
    // caller already has the pool open.
    sound: crate::notifier::sound::NotifySound,
) -> bool {
    use tauri::Emitter;

    let mut notification = notify_rust::Notification::new();
    notification.summary(title).body(body);
    // This path talks to notify_rust directly, so unlike the plugin it can
    // honour "silent" — the hint the plugin never forwards.
    if sound.is_silent() {
        notification.hint(notify_rust::Hint::SuppressSound(true));
    } else if let Some(name) = sound.platform_name() {
        notification.hint(notify_rust::Hint::SoundName(name.to_string()));
    }
    for (action, label) in buttons {
        notification.action(id_for_action(*action), label);
    }

    let handle = match notification.show() {
        Ok(h) => h,
        Err(_) => return false,
    };

    std::thread::spawn(move || {
        handle.wait_for_action(|id| {
            let Some(action) = action_from_id(id) else { return };
            let target = target.clone();
            // The callback runs on the D-Bus thread with no async runtime, so the
            // work is handed to the app's runtime rather than blocking here.
            tauri::async_runtime::spawn(async move {
                match target {
                    ActionTarget::Task(id) => {
                        let _ = apply_deadline_action(&pool, &id, action).await;
                        // The lists are refreshed by the same event the quick-capture
                        // window uses: the section may not even be mounted, while the
                        // store is shared.
                        let _ = app.emit("task-created", ());
                    }
                    ActionTarget::Note(id) => {
                        let _ = apply_note_action(&pool, &id, action).await;
                        // "updated", not "created": the note already exists, only
                        // its reminder changed.
                        let _ = app.emit("note-updated", ());
                    }
                    ActionTarget::Pomodoro => {
                        // The cycle is driven through its channel, which lives in the
                        // app's managed state.
                        use tauri::Manager;
                        if let Some(tx) = app.try_state::<crate::commands::pomodoro::PomodoroCmdTx>() {
                            let _ = apply_pomodoro_action(&tx.0, action);
                        }
                    }
                }
            });
        });
    });

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./src/db/migrations").run(&pool).await.unwrap();
        pool
    }

    async fn task_with_deadline(pool: &sqlx::SqlitePool, deadline: chrono::DateTime<chrono::Utc>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO tasks (id, title, status, priority, category, deadline, tags, recurrence,
                                hidden, created_at, updated_at, sort_order,
                                notified_24h, notified_1h, notified_deadline)
             VALUES (?, 'дедлайн', 'Todo', 'Medium', 'Work', ?, '[]', 'None', 0, ?, ?, 1, 1, 1, 1)",
        )
        .bind(&id)
        .bind(deadline.to_rfc3339())
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(pool)
        .await
        .unwrap();
        id
    }

    // Only the two real buttons count. A daemon also reports "default" (the body was
    // clicked) and "__closed" — treating either as a press would complete a task the
    // user merely dismissed.
    #[test]
    fn only_known_action_ids_are_recognized() {
        assert!(matches!(action_from_id("task_done"), Some(NotificationAction::TaskDone)));
        assert!(matches!(action_from_id("task_snooze"), Some(NotificationAction::TaskSnoozeHour)));
        for id in ["default", "__closed", "", "Done", "снуз"] {
            assert!(action_from_id(id).is_none(), "id {id:?} must not be an action");
        }
    }

    #[tokio::test]
    async fn done_completes_the_task() {
        let pool = test_pool().await;
        let id = task_with_deadline(&pool, chrono::Utc::now()).await;

        apply_deadline_action(&pool, &id, NotificationAction::TaskDone).await.unwrap();

        let (status, hidden): (String, bool) =
            sqlx::query_as("SELECT status, hidden FROM tasks WHERE id = ?")
                .bind(&id).fetch_one(&pool).await.unwrap();
        assert_eq!(status, "Done");
        assert!(hidden, "a completed task leaves the active list");
    }

    // Snoozing shifts from NOW rather than from the old deadline: for an overdue task
    // shifting from the old value would land in the past again and fire immediately.
    #[tokio::test]
    async fn snooze_shifts_an_hour_from_now_even_when_overdue() {
        let pool = test_pool().await;
        let long_ago = chrono::Utc::now() - chrono::Duration::days(3);
        let id = task_with_deadline(&pool, long_ago).await;

        apply_deadline_action(&pool, &id, NotificationAction::TaskSnoozeHour).await.unwrap();

        let deadline: String = sqlx::query_scalar("SELECT deadline FROM tasks WHERE id = ?")
            .bind(&id).fetch_one(&pool).await.unwrap();
        let parsed = chrono::DateTime::parse_from_rfc3339(&deadline).unwrap().with_timezone(&chrono::Utc);
        let delta = (parsed - chrono::Utc::now()).num_minutes();
        assert!((55..=65).contains(&delta), "expected about an hour ahead, got {delta} minutes");
    }

    // The flags refer to the previous deadline. Without a reset the scheduler would
    // never notify about the new one — the very bug update_task had to fix.
    #[tokio::test]
    async fn snooze_rearms_the_notifications() {
        let pool = test_pool().await;
        let id = task_with_deadline(&pool, chrono::Utc::now()).await;

        apply_deadline_action(&pool, &id, NotificationAction::TaskSnoozeHour).await.unwrap();

        let (h24, h1, at): (bool, bool, bool) = sqlx::query_as(
            "SELECT notified_24h, notified_1h, notified_deadline FROM tasks WHERE id = ?",
        )
        .bind(&id).fetch_one(&pool).await.unwrap();
        assert!(!h24 && !h1 && !at, "every flag must be reset");
    }

    // A blocked task cannot be completed, and the button must not become a way around
    // that: the ban lives in complete_task_impl precisely because a task can also be
    // closed from the tray, the quick slot and the palette.
    #[tokio::test]
    async fn done_respects_the_dependency_ban() {
        let pool = test_pool().await;
        let blocker = task_with_deadline(&pool, chrono::Utc::now()).await;
        let blocked = task_with_deadline(&pool, chrono::Utc::now()).await;
        crate::commands::dependencies::add_task_dependency_impl(&pool, &blocked, &blocker)
            .await
            .unwrap();

        assert!(apply_deadline_action(&pool, &blocked, NotificationAction::TaskDone).await.is_err());

        let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = ?")
            .bind(&blocked).fetch_one(&pool).await.unwrap();
        assert_eq!(status, "Todo", "the blocked task stays open");
    }

    // --- v0.9.67: the other kinds of notification ---

    const ALL_ACTIONS: [NotificationAction; 7] = [
        NotificationAction::TaskDone,
        NotificationAction::TaskSnoozeHour,
        NotificationAction::NoteDone,
        NotificationAction::NoteSnoozeHour,
        NotificationAction::PomodoroSkip,
        NotificationAction::PomodoroExtendBreak,
        NotificationAction::PomodoroStop,
    ];

    // The daemon returns a bare id with no hint of its kind, so two actions sharing
    // one id would be indistinguishable — pressing "snooze" on a note would then
    // reschedule a task.
    #[test]
    fn action_ids_are_unique() {
        let ids: std::collections::HashSet<&str> =
            ALL_ACTIONS.iter().map(|a| id_for_action(*a)).collect();
        assert_eq!(ids.len(), ALL_ACTIONS.len(), "два действия делят один id");
    }

    // The two lists are written by hand; the round trip is what keeps them in step.
    #[test]
    fn every_action_survives_the_round_trip() {
        for action in ALL_ACTIONS {
            let id = id_for_action(action);
            assert_eq!(action_from_id(id), Some(action), "id {id:?} не вернулся в действие");
        }
    }

    // An action belonging to another kind must be refused rather than quietly
    // ignored: silence would hide a wiring mistake, and applying it would touch the
    // wrong row entirely.
    #[tokio::test]
    async fn actions_of_a_foreign_kind_are_refused() {
        let pool = test_pool().await;
        let id = task_with_deadline(&pool, chrono::Utc::now()).await;

        assert!(apply_deadline_action(&pool, &id, NotificationAction::NoteDone).await.is_err());
        assert!(apply_deadline_action(&pool, &id, NotificationAction::PomodoroStop).await.is_err());
        assert!(apply_note_action(&pool, &id, NotificationAction::TaskDone).await.is_err());

        // …and the task is untouched by any of it.
        let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = ?")
            .bind(&id).fetch_one(&pool).await.unwrap();
        assert_eq!(status, "Todo");
    }

    async fn note_with_reminder(pool: &sqlx::SqlitePool, at: chrono::DateTime<chrono::Utc>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO notes (id, title, content, tags, pinned, reminder_at, notified_reminder,
                                created_at, updated_at)
             VALUES (?, 'заметка', 'текст', '[]', 0, ?, 1, ?, ?)",
        )
        .bind(&id)
        .bind(at.to_rfc3339())
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(pool)
        .await
        .unwrap();
        id
    }

    // "Done" clears the date rather than only the flag: leaving the date would make
    // the reminder fire again the moment anything reset the flag.
    #[tokio::test]
    async fn note_done_clears_the_reminder_entirely() {
        let pool = test_pool().await;
        let id = note_with_reminder(&pool, chrono::Utc::now()).await;

        apply_note_action(&pool, &id, NotificationAction::NoteDone).await.unwrap();

        let (at, notified): (Option<String>, bool) =
            sqlx::query_as("SELECT reminder_at, notified_reminder FROM notes WHERE id = ?")
                .bind(&id).fetch_one(&pool).await.unwrap();
        assert!(at.is_none(), "дата напоминания должна быть снята");
        assert!(!notified, "флаг сброшен вместе с датой");
    }

    // Same reasoning as for a deadline: an hour from now, not from the old date,
    // or an overdue reminder would land in the past and fire immediately.
    #[tokio::test]
    async fn note_snooze_shifts_an_hour_from_now_and_rearms() {
        let pool = test_pool().await;
        let long_ago = chrono::Utc::now() - chrono::Duration::days(3);
        let id = note_with_reminder(&pool, long_ago).await;

        apply_note_action(&pool, &id, NotificationAction::NoteSnoozeHour).await.unwrap();

        let (at, notified): (Option<String>, bool) =
            sqlx::query_as("SELECT reminder_at, notified_reminder FROM notes WHERE id = ?")
                .bind(&id).fetch_one(&pool).await.unwrap();
        let parsed = chrono::DateTime::parse_from_rfc3339(&at.expect("дата на месте")).unwrap();
        let delta = (parsed.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_minutes();
        assert!((55..=65).contains(&delta), "ожидался примерно час, вышло {delta} мин");
        assert!(!notified, "иначе планировщик промолчит о новой дате");
    }

    // The pomodoro actions go through the cycle's channel: the loop owns the phase
    // and the remaining seconds, and a write from outside would be overwritten on
    // its next tick.
    #[test]
    fn pomodoro_actions_map_onto_cycle_commands() {
        use crate::notifier::pomodoro::PomodoroCmd;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PomodoroCmd>();

        apply_pomodoro_action(&tx, NotificationAction::PomodoroSkip).unwrap();
        apply_pomodoro_action(&tx, NotificationAction::PomodoroExtendBreak).unwrap();
        apply_pomodoro_action(&tx, NotificationAction::PomodoroStop).unwrap();

        assert_eq!(rx.try_recv().unwrap(), PomodoroCmd::Skip);
        assert_eq!(rx.try_recv().unwrap(), PomodoroCmd::ExtendBreak(5));
        assert_eq!(rx.try_recv().unwrap(), PomodoroCmd::Stop);
    }

    #[test]
    fn foreign_actions_never_reach_the_pomodoro_channel() {
        use crate::notifier::pomodoro::PomodoroCmd;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PomodoroCmd>();

        assert!(apply_pomodoro_action(&tx, NotificationAction::TaskDone).is_err());
        assert!(rx.try_recv().is_err(), "в канал ничего не ушло");
    }
}

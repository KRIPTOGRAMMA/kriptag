use tauri::State;
use sqlx::SqlitePool;
use uuid::Uuid;
use chrono::Utc;
use crate::core::task::{Subtask, Task};
use crate::error::{AppError, AppResult};

// Fills subtasks into already-loaded tasks with a single query.
pub async fn attach_subtasks(pool: &SqlitePool, tasks: &mut [Task]) -> AppResult<()> {
    if tasks.is_empty() {
        return Ok(());
    }
    let all = sqlx::query_as::<_, Subtask>(
        "SELECT id, task_id, title, done, position FROM subtasks ORDER BY position, created_at"
    )
    .fetch_all(pool)
    .await?;

    for task in tasks.iter_mut() {
        task.subtasks = all.iter().filter(|s| s.task_id == task.id).cloned().collect();
    }
    Ok(())
}

#[tauri::command]
pub async fn get_subtasks(pool: State<'_, SqlitePool>, task_id: String) -> AppResult<Vec<Subtask>> {
    get_subtasks_impl(pool.inner(), &task_id).await
}

pub async fn get_subtasks_impl(pool: &SqlitePool, task_id: &str) -> AppResult<Vec<Subtask>> {
    sqlx::query_as::<_, Subtask>(
        "SELECT id, task_id, title, done, position FROM subtasks
         WHERE task_id = ? ORDER BY position, created_at"
    )
    .bind(task_id)
    .fetch_all(pool)
    .await
    .map_err(AppError::from)
}

#[tauri::command]
pub async fn add_subtask(pool: State<'_, SqlitePool>, task_id: String, title: String) -> AppResult<Subtask> {
    add_subtask_impl(pool.inner(), &task_id, &title).await
}

pub async fn add_subtask_impl(pool: &SqlitePool, task_id: &str, title: &str) -> AppResult<Subtask> {
    let title = title.trim();
    if title.is_empty() {
        return Err(AppError::Other("Пустая подзадача".into()));
    }
    let id = Uuid::new_v4().to_string();
    // position = the end of the list
    let next_pos: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(position) + 1, 0) FROM subtasks WHERE task_id = ?")
        .bind(task_id)
        .fetch_one(pool)
        .await?;

    sqlx::query(
        "INSERT INTO subtasks (id, task_id, title, done, position, created_at)
         VALUES (?, ?, ?, 0, ?, ?)"
    )
    .bind(&id)
    .bind(task_id)
    .bind(title)
    .bind(next_pos)
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await?;

    Ok(Subtask { id, task_id: task_id.to_string(), title: title.to_string(), done: false, position: next_pos })
}

#[tauri::command]
pub async fn toggle_subtask(pool: State<'_, SqlitePool>, id: String) -> AppResult<()> {
    toggle_subtask_impl(pool.inner(), &id).await
}

pub async fn toggle_subtask_impl(pool: &SqlitePool, id: &str) -> AppResult<()> {
    let task_id: Option<String> = sqlx::query_scalar("SELECT task_id FROM subtasks WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;

    sqlx::query("UPDATE subtasks SET done = 1 - done WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    if let Some(task_id) = task_id {
        complete_if_all_subtasks_done(pool, &task_id).await?;
    }
    Ok(())
}

// Closes the task once its last subtask is ticked.
//
// This lives in the backend rather than in the checklist component because a
// subtask is written from several places — the modal, the panel in the list, the
// quick slot — and the rule has to hold for all of them, not only for the one the
// user happened to use. Same reasoning as the blocked-task check in v0.9.56.
//
// Deliberately silent about failure to complete: the task may be blocked by a
// dependency, and complete_task_impl rejects that. Ticking the last subtask is
// not the moment to interrupt with an error about something else — the checklist
// edit itself succeeded, and the block is already visible on the row.
pub async fn complete_if_all_subtasks_done(pool: &SqlitePool, task_id: &str) -> AppResult<()> {
    // A task with no subtasks is not "all done": there is nothing to finish, and
    // adding the first unticked subtask would otherwise look like a candidate.
    let (total, done): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(done), 0) FROM subtasks WHERE task_id = ?",
    )
    .bind(task_id)
    .fetch_one(pool)
    .await?;
    if total == 0 || done < total {
        return Ok(());
    }

    // Already finished, or in the Trash — completing again would move the deadline
    // of a recurring task a second time and stamp a fresh completed_at.
    let row: Option<(bool, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT hidden, deleted_at, recurrence FROM tasks WHERE id = ?",
    )
    .bind(task_id)
    .fetch_optional(pool)
    .await?;
    let Some((hidden, deleted_at, recurrence)) = row else { return Ok(()) };
    if hidden || deleted_at.is_some() {
        return Ok(());
    }
    let repeats = recurrence.as_deref().is_some_and(|r| !r.is_empty() && r != "None");

    // A recurring task is never hidden, so the guard above cannot stop it from
    // completing over and over: completing a repeat unticks the whole checklist,
    // so the next tick is again "the last one". Each round pushed the deadline
    // another day forward and cleared notified_24h/1h/deadline, putting the task
    // back in the notification queue — measured at five deadline shifts and five
    // re-armings from five tick/untick rounds. That is the spam.
    //
    // A checklist of a single item is the whole problem: with two or more, the
    // reset leaves the others unticked and the user has to genuinely redo them.
    // So auto-completion is limited to lists where finishing means something —
    // ticking one box cannot both close a run and immediately arm the next.
    if repeats && total < 2 {
        return Ok(());
    }

    let _ = crate::commands::tasks::complete_task_impl(pool, task_id.to_string()).await;
    Ok(())
}

// Inline title editing in the modal's checklist: an empty title is an error
// rather than a silent deletion — deleting is an explicit operation.
#[tauri::command]
pub async fn rename_subtask(pool: State<'_, SqlitePool>, id: String, title: String) -> AppResult<()> {
    rename_subtask_impl(pool.inner(), &id, &title).await
}

pub async fn rename_subtask_impl(pool: &SqlitePool, id: &str, title: &str) -> AppResult<()> {
    let title = title.trim();
    if title.is_empty() {
        return Err(AppError::Other("Пустая подзадача".into()));
    }
    sqlx::query("UPDATE subtasks SET title = ? WHERE id = ?")
        .bind(title)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn delete_subtask(pool: State<'_, SqlitePool>, id: String) -> AppResult<()> {
    delete_subtask_impl(pool.inner(), &id).await
}

// Deliberately does NOT auto-complete the task, unlike toggling. Deleting the last
// unticked subtask leaves an all-ticked list, but removing work is not finishing it
// — closing the task there would be a destructive surprise from an edit the user
// made for a different reason.
pub async fn delete_subtask_impl(pool: &SqlitePool, id: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM subtasks WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./src/db/migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn add_toggle_delete_roundtrip() {
        let pool = test_pool().await;
        let s = add_subtask_impl(&pool, "task-1", "  купить хлеб  ").await.unwrap();
        assert_eq!(s.title, "купить хлеб"); // trim
        assert!(!s.done);

        let list = get_subtasks_impl(&pool, "task-1").await.unwrap();
        assert_eq!(list.len(), 1);

        toggle_subtask_impl(&pool, &s.id).await.unwrap();
        assert!(get_subtasks_impl(&pool, "task-1").await.unwrap()[0].done);
        toggle_subtask_impl(&pool, &s.id).await.unwrap();
        assert!(!get_subtasks_impl(&pool, "task-1").await.unwrap()[0].done);

        delete_subtask_impl(&pool, &s.id).await.unwrap();
        assert!(get_subtasks_impl(&pool, "task-1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn empty_title_rejected() {
        let pool = test_pool().await;
        assert!(add_subtask_impl(&pool, "task-1", "   ").await.is_err());
    }

    #[tokio::test]
    async fn rename_updates_title_and_rejects_empty() {
        let pool = test_pool().await;
        let s = add_subtask_impl(&pool, "task-1", "старое").await.unwrap();

        rename_subtask_impl(&pool, &s.id, "  новое  ").await.unwrap();
        let list = get_subtasks_impl(&pool, "task-1").await.unwrap();
        assert_eq!(list[0].title, "новое"); // trim

        assert!(rename_subtask_impl(&pool, &s.id, "   ").await.is_err());
        // the title was not clobbered by the rejected rename
        assert_eq!(get_subtasks_impl(&pool, "task-1").await.unwrap()[0].title, "новое");
    }

    #[tokio::test]
    async fn position_increments_per_task() {
        let pool = test_pool().await;
        let a = add_subtask_impl(&pool, "t", "1").await.unwrap();
        let b = add_subtask_impl(&pool, "t", "2").await.unwrap();
        assert_eq!(a.position, 0);
        assert_eq!(b.position, 1);
    }

    // The point of the AppError migration (v0.9.83), asserted rather than assumed.
    //
    // These commands used to return Result<_, String> built by
    // `.map_err(|e| e.to_string())`, so a database failure reached the frontend
    // as bare sqlx text with no prefix. errorText.ts translates by matching a
    // closed set of prefixes and returns anything else untouched, so those
    // messages silently skipped localization — add_subtask is the most frequent
    // command that was affected.
    //
    // Dropping the table is the cheapest real sqlx failure; the assertion is
    // about the prefix, not about that particular SQL error.
    #[tokio::test]
    async fn db_failure_carries_the_translatable_prefix() {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE subtasks").execute(&pool).await.unwrap();

        let err = add_subtask_impl(&pool, "task-1", "заголовок").await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.starts_with("Ошибка базы данных: "),
            "ошибка БД пришла без префикса, errorText.ts её не переведёт: {msg}"
        );

        // The same for a read, which returns the error as a tail expression
        // (map_err(AppError::from)) rather than through `?` — a different path
        // through the same conversion.
        let err = get_subtasks_impl(&pool, "task-1").await.unwrap_err();
        assert!(
            err.to_string().starts_with("Ошибка базы данных: "),
            "чтение подзадач вернуло ошибку без префикса: {err}"
        );
    }

    // Domain messages must stay verbatim: errorText.ts splits on a known prefix,
    // and "Пустая подзадача" is not one — gluing a technical prefix onto it would
    // both mistranslate it and change what the user reads.
    #[tokio::test]
    async fn domain_error_stays_verbatim() {
        let pool = test_pool().await;
        let err = add_subtask_impl(&pool, "task-1", "   ").await.unwrap_err();
        assert_eq!(err.to_string(), "Пустая подзадача");
    }

    // A helper task to hang subtasks on. Auto-completion reads the tasks table, so
    // these cases need a real row rather than the bare "task-1" id the older tests
    // use — those only ever touched the subtasks table.
    async fn task_with(pool: &SqlitePool, recurrence: Option<crate::core::task::Recurrence>) -> String {
        let created = crate::commands::tasks::create_task_impl(
            pool,
            crate::core::task::CreateTask {
                title: "задача".into(),
                description: None,
                status: "Todo".into(),
                priority: crate::core::task::Priority::Medium,
                category: "Work".into(),
                deadline: Some(Utc::now() + chrono::Duration::days(1)),
                tags: vec![],
                recurrence,
                project_id: None,
            },
        )
        .await
        .unwrap();
        created.id
    }

    async fn task_row(pool: &SqlitePool, id: &str) -> (String, bool) {
        sqlx::query_as("SELECT status, hidden FROM tasks WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    // The rule the user asked for: ticking the LAST subtask finishes the task.
    #[tokio::test]
    async fn last_subtask_completes_the_task() {
        let pool = test_pool().await;
        let task_id = task_with(&pool, None).await;
        let a = add_subtask_impl(&pool, &task_id, "раз").await.unwrap();
        let b = add_subtask_impl(&pool, &task_id, "два").await.unwrap();

        toggle_subtask_impl(&pool, &a.id).await.unwrap();
        let (status, hidden) = task_row(&pool, &task_id).await;
        assert_eq!(status, "Todo", "задача закрылась на первой из двух подзадач");
        assert!(!hidden);

        toggle_subtask_impl(&pool, &b.id).await.unwrap();
        let (status, hidden) = task_row(&pool, &task_id).await;
        assert_eq!(status, "Done", "последняя подзадача не закрыла задачу");
        assert!(hidden, "закрытая задача должна уйти в историю");
    }

    // Unticking must not leave the task closed: the checklist is no longer complete.
    #[tokio::test]
    async fn unticking_does_not_complete() {
        let pool = test_pool().await;
        let task_id = task_with(&pool, None).await;
        let a = add_subtask_impl(&pool, &task_id, "раз").await.unwrap();
        toggle_subtask_impl(&pool, &a.id).await.unwrap();
        assert_eq!(task_row(&pool, &task_id).await.0, "Done");

        // The task is in history now; unticking edits the checklist but must not
        // itself reopen or re-close anything.
        toggle_subtask_impl(&pool, &a.id).await.unwrap();
        let subs = get_subtasks_impl(&pool, &task_id).await.unwrap();
        assert!(!subs[0].done, "галочка не снялась");
    }

    // A task with no checklist has nothing to finish — the rule must not fire on an
    // empty list, or every task without subtasks would be a candidate.
    #[tokio::test]
    async fn task_without_subtasks_is_untouched() {
        let pool = test_pool().await;
        let task_id = task_with(&pool, None).await;
        complete_if_all_subtasks_done(&pool, &task_id).await.unwrap();
        assert_eq!(task_row(&pool, &task_id).await.0, "Todo");
    }

    // Deleting is not finishing. Removing the last unticked subtask leaves an
    // all-ticked list, and closing the task there would be a destructive surprise.
    #[tokio::test]
    async fn deleting_the_last_undone_subtask_does_not_complete() {
        let pool = test_pool().await;
        let task_id = task_with(&pool, None).await;
        let a = add_subtask_impl(&pool, &task_id, "готова").await.unwrap();
        let b = add_subtask_impl(&pool, &task_id, "не готова").await.unwrap();
        toggle_subtask_impl(&pool, &a.id).await.unwrap();
        assert_eq!(task_row(&pool, &task_id).await.0, "Todo");

        delete_subtask_impl(&pool, &b.id).await.unwrap();
        assert_eq!(
            task_row(&pool, &task_id).await.0,
            "Todo",
            "удаление подзадачи закрыло задачу — удалять не значит выполнить"
        );
    }

    // A recurring task closes its run the same way as a manual completion: it does
    // not go to history but moves to the next deadline, and the checklist is
    // cleared as the plan for the NEXT run (v0.9.24 behaviour, reached from here).
    #[tokio::test]
    async fn recurring_task_moves_to_next_run() {
        let pool = test_pool().await;
        let task_id = task_with(&pool, Some(crate::core::task::Recurrence::Daily)).await;
        // Two items: a one-item recurring checklist is deliberately excluded from
        // auto-completion, see recurring_single_item_checklist_does_not_loop.
        let a = add_subtask_impl(&pool, &task_id, "раз").await.unwrap();
        let b = add_subtask_impl(&pool, &task_id, "два").await.unwrap();

        toggle_subtask_impl(&pool, &b.id).await.unwrap();
        toggle_subtask_impl(&pool, &a.id).await.unwrap();
        let (_, hidden) = task_row(&pool, &task_id).await;
        assert!(!hidden, "повторяющаяся задача не должна уходить в историю");

        let subs = get_subtasks_impl(&pool, &task_id).await.unwrap();
        assert!(
            subs.iter().all(|s| !s.done),
            "чек-лист повтора должен очиститься под следующий прогон"
        );
    }

    // A blocked task cannot be completed (v0.9.56). Ticking its last subtask must
    // still succeed as an edit — the block is about finishing, not about editing.
    #[tokio::test]
    async fn blocked_task_stays_open_but_the_tick_survives() {
        let pool = test_pool().await;
        let blocker = task_with(&pool, None).await;
        let task_id = task_with(&pool, None).await;
        crate::commands::dependencies::add_task_dependency_impl(&pool, &task_id, &blocker)
            .await
            .unwrap();

        let a = add_subtask_impl(&pool, &task_id, "раз").await.unwrap();
        toggle_subtask_impl(&pool, &a.id).await.unwrap();

        assert_eq!(
            task_row(&pool, &task_id).await.0,
            "Todo",
            "заблокированная задача закрылась в обход зависимости"
        );
        let subs = get_subtasks_impl(&pool, &task_id).await.unwrap();
        assert!(subs[0].done, "галочка потерялась из-за блокировки");
    }

    // The notification spam. Completing a repeat unticks the whole checklist, so on
    // a ONE-item list the next tick is again "the last one" — and every round moved
    // the deadline forward and cleared the notified_* flags, re-arming the
    // scheduler. Auto-completion therefore skips single-item recurring checklists.
    #[tokio::test]
    async fn recurring_single_item_checklist_does_not_loop() {
        let pool = test_pool().await;
        let task_id = task_with(&pool, Some(crate::core::task::Recurrence::Daily)).await;
        let a = add_subtask_impl(&pool, &task_id, "шаг").await.unwrap();

        let deadline_of = |pool: SqlitePool, id: String| async move {
            sqlx::query_scalar::<_, Option<String>>("SELECT deadline FROM tasks WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        };

        let before = deadline_of(pool.clone(), task_id.clone()).await;
        for _ in 0..5 {
            toggle_subtask_impl(&pool, &a.id).await.unwrap();
            toggle_subtask_impl(&pool, &a.id).await.unwrap();
        }
        let after = deadline_of(pool.clone(), task_id.clone()).await;
        assert_eq!(
            before, after,
            "дедлайн повтора уехал от щёлканья одной галочки — это и есть спам уведомлений"
        );

        let flags: i64 = sqlx::query_scalar(
            "SELECT notified_24h + notified_1h + notified_deadline FROM tasks WHERE id = ?",
        )
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(flags, 0, "флаги уведомлений сбрасывались по кругу");
    }

    // Two or more items still auto-complete: the reset unticks the others, so the
    // user has to genuinely redo the work before it can close again.
    #[tokio::test]
    async fn recurring_multi_item_checklist_still_completes() {
        let pool = test_pool().await;
        let task_id = task_with(&pool, Some(crate::core::task::Recurrence::Daily)).await;
        let a = add_subtask_impl(&pool, &task_id, "раз").await.unwrap();
        let b = add_subtask_impl(&pool, &task_id, "два").await.unwrap();

        let before: Option<String> = sqlx::query_scalar("SELECT deadline FROM tasks WHERE id = ?")
            .bind(&task_id).fetch_one(&pool).await.unwrap();
        toggle_subtask_impl(&pool, &a.id).await.unwrap();
        toggle_subtask_impl(&pool, &b.id).await.unwrap();
        let after: Option<String> = sqlx::query_scalar("SELECT deadline FROM tasks WHERE id = ?")
            .bind(&task_id).fetch_one(&pool).await.unwrap();

        assert_ne!(before, after, "повтор из двух пунктов не закрылся");
        let subs = get_subtasks_impl(&pool, &task_id).await.unwrap();
        assert!(subs.iter().all(|s| !s.done), "чек-лист не сброшен под следующий прогон");
    }
}

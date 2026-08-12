use tauri::State;
use sqlx::{SqlitePool, Row};
use serde::Serialize;
use chrono::{DateTime, Utc};
use crate::core::task::{CreateTask, Task, TaskRow, UpdateTask};
use crate::error::{AppError, AppResult};

#[tauri::command]
pub async fn create_task(
  pool: State<'_, SqlitePool>,
  task: CreateTask,
) -> AppResult<Task> {
  create_task_impl(pool.inner(), task).await
}

pub async fn create_task_impl(pool: &SqlitePool, task: CreateTask) -> AppResult<Task> {
  if task.title.trim().is_empty() {
    return Err(AppError::Other("Название задачи не может быть пустым".into()));
  }

  let mut new_task = task.into_task();
  // An unknown category/status silently falls back (the former enum semantics)
  new_task.category = crate::commands::categories::valid_or_fallback(pool, &new_task.category).await;
  new_task.status = crate::commands::statuses::valid_or_fallback(pool, &new_task.status).await;
  // A new task goes to the end of the list
  new_task.sort_order = sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(sort_order), 0) + 1 FROM tasks")
    .fetch_one(pool)
    .await?;

  sqlx::query(
    "INSERT INTO tasks (id, title, description, status, priority, category, deadline, tags, recurrence, hidden, created_at, updated_at, project_id, sort_order)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
  )
  .bind(&new_task.id)
  .bind(&new_task.title)
  .bind(&new_task.description)
  .bind(&new_task.status)
  .bind(format!("{:?}", new_task.priority))
  .bind(&new_task.category)
  .bind(new_task.deadline.map(|d| d.to_rfc3339()))
  .bind(serde_json::to_string(&new_task.tags).unwrap_or_else(|_| "[]".into()))
  .bind(new_task.recurrence.to_db()) 
  .bind(new_task.hidden)
  .bind(new_task.created_at.to_rfc3339())
  .bind(new_task.updated_at.to_rfc3339())
  .bind(&new_task.project_id)
  .bind(new_task.sort_order)
  .execute(pool)
  .await?;

  Ok(new_task)
}

#[tauri::command]
pub async fn get_tasks(
    pool: State<'_, SqlitePool>,
) -> AppResult<Vec<Task>> {
    get_tasks_impl(pool.inner()).await
}

pub async fn get_tasks_impl(pool: &SqlitePool) -> AppResult<Vec<Task>> {
    let rows = sqlx::query_as::<_, TaskRow>("SELECT * FROM tasks WHERE deleted_at IS NULL ORDER BY sort_order")
        .fetch_all(pool)
        .await?;

    let mut tasks: Vec<Task> = rows.into_iter().map(|r| r.into_task()).collect();
    crate::commands::subtasks::attach_subtasks(pool, &mut tasks).await?;
    crate::commands::dependencies::attach_blockers(pool, &mut tasks).await?;
    Ok(tasks)
}

#[tauri::command]
pub async fn delete_task(
  pool: State<'_, SqlitePool>,
  id: String,
) -> AppResult<()> {
  delete_task_impl(pool.inner(), id).await
}

// Soft delete ("Trash"): the task stays in the table with its subtasks and
// note links untouched, it just stops showing up in the active list/history —
// it is filtered out in get_tasks_impl. Real deletion goes through
// purge_deleted_task.
pub async fn delete_task_impl(pool: &SqlitePool, id: String) -> AppResult<()> {
  sqlx::query("UPDATE tasks SET deleted_at = ? WHERE id = ?")
    .bind(Utc::now().to_rfc3339())
    .bind(id)
    .execute(pool)
    .await?;

  Ok(())
}

#[tauri::command]
pub async fn get_deleted_tasks(pool: State<'_, SqlitePool>) -> AppResult<Vec<Task>> {
  get_deleted_tasks_impl(pool.inner()).await
}

pub async fn get_deleted_tasks_impl(pool: &SqlitePool) -> AppResult<Vec<Task>> {
  let rows = sqlx::query_as::<_, TaskRow>("SELECT * FROM tasks WHERE deleted_at IS NOT NULL ORDER BY deleted_at DESC")
    .fetch_all(pool)
    .await?;

  let mut tasks: Vec<Task> = rows.into_iter().map(|r| r.into_task()).collect();
  crate::commands::subtasks::attach_subtasks(pool, &mut tasks).await?;
  Ok(tasks)
}

#[tauri::command]
pub async fn restore_task(pool: State<'_, SqlitePool>, id: String) -> AppResult<()> {
  restore_task_impl(pool.inner(), id).await
}

pub async fn restore_task_impl(pool: &SqlitePool, id: String) -> AppResult<()> {
  sqlx::query("UPDATE tasks SET deleted_at = NULL WHERE id = ?")
    .bind(id)
    .execute(pool)
    .await?;

  Ok(())
}

// At most once every 24h — the same "is it due yet" logic as auto_backup_due
// (commands/backup.rs), where a matching last_* setting key holds the previous
// run. Disabled when history_cleanup_months = 0.
pub async fn history_cleanup_due(pool: &SqlitePool) -> bool {
    let months = crate::commands::settings::get_u64_setting(pool, "history_cleanup_months", 0).await;
    if months == 0 {
        return false;
    }
    match crate::commands::settings::get_setting(pool, "last_history_cleanup").await {
        Some(ts) => {
            let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&ts) else { return true; };
            let elapsed = Utc::now() - parsed.with_timezone(&Utc);
            elapsed >= chrono::Duration::hours(24)
        }
        None => true,
    }
}

// Automatic history cleanup: completed tasks older than the cutoff move to
// Trash through the same soft mechanism as manual deletion. completed_at is
// left alone, so dashboard streaks and the heatmap (which read
// tasks.completed_at directly, with no separate log) are not distorted after
// the fact. Returns how many tasks were moved (for the journal and tests).
pub async fn cleanup_old_history_impl(pool: &SqlitePool, cutoff: DateTime<Utc>) -> AppResult<u64> {
    let result = sqlx::query(
        "UPDATE tasks SET deleted_at = ?
         WHERE hidden = 1 AND deleted_at IS NULL AND completed_at IS NOT NULL AND completed_at < ?"
    )
    .bind(Utc::now().to_rfc3339())
    .bind(cutoff.to_rfc3339())
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

#[tauri::command]
pub async fn purge_deleted_task(pool: State<'_, SqlitePool>, id: String) -> AppResult<()> {
  purge_deleted_task_impl(pool.inner(), id).await
}

// Real removal of a row from the Trash — the same cleanup of subtasks and note
// links that delete_task_impl used to do back when deletion was hard.
//
// The task_dependencies delete is redundant today and kept deliberately.
// Migration 0031 declares ON DELETE CASCADE and that cascade really does fire —
// but only because sqlx enables `PRAGMA foreign_keys` by default in its own
// connect options; db/sqlite.rs never asks for it, and raw SQLite defaults it to
// OFF. Deleting the rows here means a change in that library default cannot
// quietly start orphaning dependencies. db/sqlite.rs::foreign_keys_are_enforced
// guards the assumption itself.
//
// Both columns are cleared: a purged task can be the blocked side of one link
// and the blocker of another.
//
// Wrapped in a transaction so the row and everything hanging off it disappear
// together — a failure between the statements would otherwise leave a task's
// subtasks behind (those have no FK at all, so nothing else would clean them).
pub async fn purge_deleted_task_impl(pool: &SqlitePool, id: String) -> AppResult<()> {
  let mut tx = pool.begin().await?;

  sqlx::query("DELETE FROM subtasks WHERE task_id = ?")
    .bind(&id)
    .execute(&mut *tx)
    .await?;

  sqlx::query("DELETE FROM task_dependencies WHERE task_id = ? OR blocker_id = ?")
    .bind(&id)
    .bind(&id)
    .execute(&mut *tx)
    .await?;

  sqlx::query("UPDATE notes SET linked_task_id = NULL WHERE linked_task_id = ?")
    .bind(&id)
    .execute(&mut *tx)
    .await?;

  sqlx::query("DELETE FROM tasks WHERE id = ?")
    .bind(&id)
    .execute(&mut *tx)
    .await?;

  tx.commit().await?;

  Ok(())
}

#[tauri::command]
pub async fn update_task(
    pool: State<'_, SqlitePool>,
    id: String,
    patch: UpdateTask,
) -> AppResult<Task> {
    update_task_impl(pool.inner(), id, patch).await
}

pub async fn update_task_impl(pool: &SqlitePool, id: String, patch: UpdateTask) -> AppResult<Task> {
    let row = sqlx::query_as::<_, TaskRow>("SELECT * FROM tasks WHERE id = ?")
        .bind(&id)
        .fetch_one(pool)
        .await?;

    let mut task = row.into_task();
    let old_deadline = task.deadline;

    if let Some(title) = patch.title {
        if title.trim().is_empty() {
            return Err(AppError::Other("Название задачи не может быть пустым".into()));
        }
        task.title = title;
    }
    if let Some(desc) = patch.description   { task.description = Some(desc); }
    if let Some(status) = patch.status {
        task.status = crate::commands::statuses::valid_or_fallback(pool, &status).await;
    }
    if let Some(priority) = patch.priority  { task.priority = priority; }
    if let Some(category) = patch.category {
        task.category = crate::commands::categories::valid_or_fallback(pool, &category).await;
    }
    if let Some(tags) = patch.tags          { task.tags = tags; }
    if let Some(recurrence) = patch.recurrence { task.recurrence = recurrence; }

    if let Some(dl) = patch.deadline {
        task.deadline = if dl.is_empty() {
            None
        } else {
            Some(DateTime::parse_from_rfc3339(&dl)
                .map_err(|e| e.to_string())?
                .with_timezone(&Utc))
        };
    }

    // Like deadline: an empty string detaches the task from its project
    if let Some(pid) = patch.project_id {
        task.project_id = if pid.is_empty() { None } else { Some(pid) };
    }

    // Time block: an empty string clears the block entirely (duration too)
    let old_scheduled = task.scheduled_at;
    if let Some(sa) = patch.scheduled_at {
        if sa.is_empty() {
            task.scheduled_at = None;
            task.scheduled_mins = None;
        } else {
            task.scheduled_at = Some(DateTime::parse_from_rfc3339(&sa)
                .map_err(|e| e.to_string())?
                .with_timezone(&Utc));
        }
    }
    if let Some(mins) = patch.scheduled_mins {
        task.scheduled_mins = Some(mins.clamp(15, 24 * 60));
    }

    task.updated_at = Utc::now();
    // If the deadline actually changed, the old notified_* flags no longer
    // describe the current deadline — without this reset the scheduler would
    // never notify about the new date (this used to be a bug: the flags were
    // left as they were).
    let deadline_changed = task.deadline != old_deadline;
    // Moving a block: reset notified_block so the push arrives at the new time
    let block_changed = task.scheduled_at != old_scheduled;

    sqlx::query(
        "UPDATE tasks SET title=?, description=?, status=?, priority=?,
         category=?, deadline=?, tags=?, recurrence=?, updated_at=?, project_id=?,
         scheduled_at=?, scheduled_mins=?,
         notified_24h = CASE WHEN ? THEN 0 ELSE notified_24h END,
         notified_1h = CASE WHEN ? THEN 0 ELSE notified_1h END,
         notified_deadline = CASE WHEN ? THEN 0 ELSE notified_deadline END,
         notified_block = CASE WHEN ? THEN 0 ELSE notified_block END
         WHERE id=?"
    )
    .bind(&task.title)
    .bind(&task.description)
    .bind(&task.status)
    .bind(format!("{:?}", task.priority))
    .bind(&task.category)
    .bind(task.deadline.map(|d| d.to_rfc3339()))
    .bind(serde_json::to_string(&task.tags).unwrap_or_else(|_| "[]".into()))
    .bind(task.recurrence.to_db())
    .bind(task.updated_at.to_rfc3339())
    .bind(&task.project_id)
    .bind(task.scheduled_at.map(|d| d.to_rfc3339()))
    .bind(task.scheduled_mins)
    .bind(deadline_changed)
    .bind(deadline_changed)
    .bind(deadline_changed)
    .bind(block_changed)
    .bind(&id)
    .execute(pool)
    .await?;

    Ok(task)
}

#[tauri::command]
pub async fn complete_task(
  pool: State<'_, SqlitePool>,
  id: String,
) -> AppResult<Task> {
  complete_task_impl(pool.inner(), id).await
}

pub async fn complete_task_impl(pool: &SqlitePool, id: String) -> AppResult<Task> {
  // A blocked task cannot be completed. The check lives here rather than in the
  // UI alone: a task can also be completed from the tray, the quick slot and the
  // command palette, and every one of those paths would otherwise bypass it.
  let blockers = crate::commands::dependencies::blockers_of(pool, &id).await?;
  if !blockers.is_empty() {
    let names = blockers.iter().map(|b| b.title.clone()).collect::<Vec<_>>().join(", ");
    let lang = crate::i18n::current_lang(pool).await;
    return Err(AppError::Other(crate::i18n::tr_args("Сначала выполните: {tasks}.", lang, &[("tasks", names)])));
  }

  let row = sqlx::query_as::<_, TaskRow>("SELECT * FROM tasks WHERE id = ?")
        .bind(&id)
        .fetch_one(pool)
        .await?;

  let mut task = row.into_task();
  let now = Utc::now();
  // A recurring task moves to a new deadline, so the old notified_* flags refer
  // to the PREVIOUS one. Without resetting them the scheduler would never notify
  // about this task again (this used to be a bug: the flags were never reset).
  let mut reset_notifications = false;

  // Cascade onto subtasks. For a one-off task "completed" means its whole
  // checklist is closed — otherwise the history holds a Done task with unchecked
  // items. For a recurring one it is the opposite: the task does not close but
  // moves to its next deadline, and the checklist is the plan for the NEXT run,
  // so it must be cleared rather than ticked (or the repeat arrives already
  // "completed").
  let subtasks_done: bool;

  match task.recurrence.next_occurrence(now) {
    None => {
      task.status = "Done".to_string();
      task.hidden = true;
      task.completed_at = Some(now);
      subtasks_done = true;
    }
    Some(next_deadline) => {
      // Shift scheduled_at by the same delta as the deadline. For Weekdays that
      // is not a fixed Duration (the interval depends on the current weekday), so
      // we compute it explicitly as next_deadline - now instead of to_duration.
      let delta = next_deadline - now;
      task.deadline = Some(next_deadline);
      if let Some(scheduled) = &task.scheduled_at {
        task.scheduled_at = Some(*scheduled + delta);
      }
      // The run is over, so the task returns to Todo. Without this a recurring
      // task left in InProgress (or in a custom status) keeps that status and its
      // place in the list after "complete": clicking ✓ visibly does nothing and
      // the task cannot be closed.
      task.status = "Todo".to_string();
      reset_notifications = true;
      subtasks_done = false;
    }
  }

  task.updated_at = now;

  // Both writes are one logical operation, especially for a recurring task:
  // clear the checklist AND move the deadline. A failure between them used to
  // leave the worst possible mixture — the checklist wiped while the task still
  // sits at its old deadline, i.e. the plan for the next run destroyed and the
  // run itself not advanced. Either everything or nothing (v0.9.89).
  let mut tx = pool.begin().await?;

  sqlx::query("UPDATE subtasks SET done = ? WHERE task_id = ?")
    .bind(subtasks_done)
    .bind(&id)
    .execute(&mut *tx)
    .await?;

  sqlx::query(
    "UPDATE tasks SET status=?, hidden=?, deadline=?, completed_at=?, updated_at=?, scheduled_at=?,
     notified_24h = CASE WHEN ? THEN 0 ELSE notified_24h END,
     notified_1h = CASE WHEN ? THEN 0 ELSE notified_1h END,
     notified_deadline = CASE WHEN ? THEN 0 ELSE notified_deadline END,
     notified_block = CASE WHEN ? THEN 0 ELSE notified_block END
     WHERE id=?"
  )
  .bind(&task.status)
  .bind(task.hidden)
  .bind(task.deadline.map(|d| d.to_rfc3339()))
  .bind(task.completed_at.map(|d| d.to_rfc3339()))
  .bind(task.updated_at.to_rfc3339())
  .bind(task.scheduled_at.map(|d| d.to_rfc3339()))
  .bind(reset_notifications)
  .bind(reset_notifications)
  .bind(reset_notifications)
  .bind(reset_notifications)
  .bind(&id)
  .execute(&mut *tx)
  .await?;

  tx.commit().await?;

  // Read after commit: into_task() does not load subtasks, and the returned task
  // must reflect the new checklist state rather than an empty list. It is a read
  // for the return value only, so it has no business inside the transaction.
  task.subtasks = crate::commands::subtasks::get_subtasks_impl(pool, &id).await?;

  // The notification is only sent if the task actually closed. A recurring task
  // keeps completed_at empty — it moved to its next deadline and still blocks,
  // so nothing was unblocked.
  if task.completed_at.is_some() {
    crate::commands::dependencies::notify_unblocked(pool, &id).await?;
  }

  Ok(task)
}

#[tauri::command]
pub async fn reorder_tasks(pool: State<'_, SqlitePool>, ids: Vec<String>) -> AppResult<()> {
    reorder_tasks_impl(pool.inner(), ids).await
}

// Manual ordering: the frontend sends the ids of the visible list in their new
// order. We reuse the very sort_order values those tasks already had and hand
// them out in the new order, so tasks outside the list do not shift and no
// collisions with other values arise.
pub async fn reorder_tasks_impl(pool: &SqlitePool, ids: Vec<String>) -> AppResult<()> {
    if ids.len() < 2 {
        return Ok(());
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!("SELECT id, sort_order FROM tasks WHERE id IN ({placeholders})");
    let mut q = sqlx::query_as::<_, (String, i64)>(&sql);
    for id in &ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;
    let existing: std::collections::HashSet<&str> = rows.iter().map(|(id, _)| id.as_str()).collect();
    let mut orders: Vec<i64> = rows.iter().map(|(_, o)| *o).collect();
    orders.sort_unstable();

    // Unknown ids (a race with deletion) are dropped BEFORE the zip, otherwise
    // the values would be handed out shifted, to the wrong tasks.
    let live_ids = ids.iter().filter(|id| existing.contains(id.as_str()));
    for (id, ord) in live_ids.zip(orders) {
        sqlx::query("UPDATE tasks SET sort_order = ? WHERE id = ?")
            .bind(ord)
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn search_tasks(
  pool: State<'_, SqlitePool>,
  query: String,
) -> AppResult<Vec<Task>> {
  search_tasks_impl(pool.inner(), query).await
}

pub async fn search_tasks_impl(pool: &SqlitePool, query: String) -> AppResult<Vec<Task>> {
  let trimmed = query.trim();
  if trimmed.is_empty() {
    return Ok(vec![]);
  }

  // Raw user input must not reach MATCH directly: characters such as
  // " - : ( ) and AND/OR/NOT are FTS5 syntax, not text. A hyphen in a title
  // ("buy bread-2") already failed with "no such column: 2". We wrap the input
  // as a quoted phrase prefix, which is safe for anything the user types.
  let escaped = trimmed.replace('"', "\"\"");
  let fts_query = format!("\"{}\"*", escaped);

  let rows = sqlx::query_as::<_, TaskRow>(
    "SELECT t.* FROM tasks t
     INNER JOIN tasks_fts ON tasks_fts.rowid = t.rowid
     WHERE tasks_fts MATCH ?
       AND t.hidden = 0
       AND t.deleted_at IS NULL
     ORDER BY rank"
  )
  .bind(fts_query)
  .fetch_all(pool)
  .await?;

  let mut tasks: Vec<Task> = rows.into_iter().map(|r| r.into_task()).collect();
  crate::commands::subtasks::attach_subtasks(pool, &mut tasks).await?;
  Ok(tasks)
}

#[derive(Debug, Serialize, Clone)]
pub struct TaskSnippet {
  pub item: Task,
  pub snippet: String,
}

#[tauri::command]
pub async fn search_tasks_snippet(pool: State<'_, SqlitePool>, query: String) -> AppResult<Vec<TaskSnippet>> {
  search_tasks_snippet_impl(pool.inner(), query).await
}

pub async fn search_tasks_snippet_impl(pool: &SqlitePool, query: String) -> AppResult<Vec<TaskSnippet>> {
  let trimmed = query.trim();
  if trimmed.is_empty() {
    return Ok(vec![]);
  }

  let escaped = trimmed.replace('"', "\"\"");
  let fts_query = format!("\"{}\"*", escaped);

  let rows = sqlx::query(
    "SELECT t.*,
            snippet(tasks_fts, 2, '<mark>', '</mark>', '…', 32) AS snippet
     FROM tasks t
     INNER JOIN tasks_fts ON tasks_fts.rowid = t.rowid
     WHERE tasks_fts MATCH ?
       AND t.hidden = 0
       AND t.deleted_at IS NULL
     ORDER BY rank"
  )
  .bind(fts_query)
  .fetch_all(pool)
  .await?;

  let mut snippets: Vec<TaskSnippet> = Vec::with_capacity(rows.len());
  for row in rows {
    let snippet: Option<String> = row.get("snippet");
    let snippet = snippet.unwrap_or_default();
    let task_row = TaskRow {
      id: row.get("id"),
      title: row.get("title"),
      description: row.get("description"),
      status: row.get("status"),
      priority: row.get("priority"),
      category: row.get("category"),
      deadline: row.get("deadline"),
      tags: row.get("tags"),
      created_at: row.get("created_at"),
      updated_at: row.get("updated_at"),
      completed_at: row.get("completed_at"),
      recurrence: row.get("recurrence"),
      hidden: row.get("hidden"),
      deleted_at: row.get("deleted_at"),
      project_id: row.get("project_id"),
      scheduled_at: row.get("scheduled_at"),
      scheduled_mins: row.get("scheduled_mins"),
      sort_order: row.get("sort_order"),
    };
    let mut task = task_row.into_task();
    task.subtasks = crate::commands::subtasks::get_subtasks_impl(pool, &task.id).await?;
    snippets.push(TaskSnippet { item: task, snippet });
  }
  Ok(snippets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::task::{Priority, Recurrence, RecurrenceUnit};

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./src/db/migrations").run(&pool).await.unwrap();
        pool
    }

    fn new_task(title: &str) -> CreateTask {
        CreateTask {
            title: title.into(),
            description: Some("desc".into()),
            status: "Todo".into(),
            priority: Priority::Medium,
            category: "Work".into(),
            deadline: Some(Utc::now() + chrono::Duration::days(3)),
            tags: vec!["a".into(), "b".into()],
            recurrence: None,
            project_id: None,
        }
    }

    #[tokio::test]
    async fn create_then_get_roundtrip() {
        let pool = test_pool().await;
        let created = create_task_impl(&pool, new_task("тестовая задача")).await.unwrap();

        let tasks = get_tasks_impl(&pool).await.unwrap();
        assert_eq!(tasks.len(), 1);
        let got = &tasks[0];
        assert_eq!(got.id, created.id);
        assert_eq!(got.title, "тестовая задача");
        assert_eq!(got.tags, vec!["a", "b"]);
        assert_eq!(got.status, "Todo");
        assert!(got.deadline.is_some());
    }

    #[tokio::test]
    async fn create_rejects_empty_title() {
        let pool = test_pool().await;
        assert!(create_task_impl(&pool, new_task("   ")).await.is_err());
    }

    #[tokio::test]
    async fn reorder_permutes_only_given_ids() {
        let pool = test_pool().await;
        let a = create_task_impl(&pool, new_task("а")).await.unwrap();
        let b = create_task_impl(&pool, new_task("б")).await.unwrap();
        let c = create_task_impl(&pool, new_task("в")).await.unwrap();
        let d = create_task_impl(&pool, new_task("г")).await.unwrap();

        // New tasks go to the end: а, б, в, г /* ru-ok: test data names */
        let titles = |tasks: &[Task]| tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(&get_tasks_impl(&pool).await.unwrap()), ["а", "б", "в", "г"]);

        // Reorder the first three: в, а, б — "г" is left alone /* ru-ok: test data names */
        reorder_tasks_impl(&pool, vec![c.id.clone(), a.id.clone(), b.id.clone()]).await.unwrap();
        assert_eq!(titles(&get_tasks_impl(&pool).await.unwrap()), ["в", "а", "б", "г"]);

        // The sort_order values are the same three as before (a permutation, not a renumbering)
        let orders: Vec<i64> = get_tasks_impl(&pool).await.unwrap().iter().map(|t| t.sort_order).collect();
        assert_eq!(orders, [1, 2, 3, 4]);

        // A vanished id does not break handing values out to the rest
        delete_task_impl(&pool, a.id.clone()).await.unwrap();
        reorder_tasks_impl(&pool, vec![b.id.clone(), a.id.clone(), c.id.clone()]).await.unwrap();
        assert_eq!(titles(&get_tasks_impl(&pool).await.unwrap()), ["б", "в", "г"]);

        // A single id is a no-op, not an error
        reorder_tasks_impl(&pool, vec![d.id.clone()]).await.unwrap();
    }

    #[tokio::test]
    async fn complete_non_recurring_marks_done_and_hides() {
        let pool = test_pool().await;
        let t = create_task_impl(&pool, new_task("разовая")).await.unwrap();

        let done = complete_task_impl(&pool, t.id).await.unwrap();
        assert_eq!(done.status, "Done");
        assert!(done.hidden);
        assert!(done.completed_at.is_some());
    }

    #[tokio::test]
    async fn complete_non_recurring_marks_subtasks_done() {
        use crate::commands::subtasks::{add_subtask_impl, get_subtasks_impl, toggle_subtask_impl};
        let pool = test_pool().await;
        let t = create_task_impl(&pool, new_task("с чеклистом")).await.unwrap();
        let a = add_subtask_impl(&pool, &t.id, "первый").await.unwrap();
        add_subtask_impl(&pool, &t.id, "второй").await.unwrap();
        // One is already ticked by hand — the cascade must not untick it
        toggle_subtask_impl(&pool, &a.id).await.unwrap();

        // An unrelated task nearby: the cascade must not touch its subtasks
        let other = create_task_impl(&pool, new_task("соседняя")).await.unwrap();
        add_subtask_impl(&pool, &other.id, "чужая").await.unwrap();

        let done = complete_task_impl(&pool, t.id.clone()).await.unwrap();

        assert!(get_subtasks_impl(&pool, &t.id).await.unwrap().iter().all(|s| s.done));
        // The returned task reflects the new state, not an empty list
        assert_eq!(done.subtasks.len(), 2);
        assert!(done.subtasks.iter().all(|s| s.done));
        assert!(!get_subtasks_impl(&pool, &other.id).await.unwrap()[0].done);
    }

    // A real bug from the production DB: a recurring task in the InProgress
    // status stayed InProgress after "complete" — clicking ✓ visibly did nothing
    // and the task could not be closed.
    #[tokio::test]
    async fn complete_recurring_returns_to_todo_from_any_status() {
        let pool = test_pool().await;
        let mut ct = new_task("ежедневная в работе");
        ct.recurrence = Some(Recurrence::Custom(1, RecurrenceUnit::Days));
        let t = create_task_impl(&pool, ct).await.unwrap();
        sqlx::query("UPDATE tasks SET status = 'InProgress' WHERE id = ?")
            .bind(&t.id).execute(&pool).await.unwrap();

        let done = complete_task_impl(&pool, t.id.clone()).await.unwrap();

        assert_eq!(done.status, "Todo");
        assert!(!done.hidden);
        // in the DB too, not only in the returned object
        let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = ?")
            .bind(&t.id).fetch_one(&pool).await.unwrap();
        assert_eq!(status, "Todo");
    }

    #[tokio::test]
    async fn complete_recurring_resets_subtasks_for_next_run() {
        use crate::commands::subtasks::{add_subtask_impl, get_subtasks_impl, toggle_subtask_impl};
        let pool = test_pool().await;
        let mut ct = new_task("ежедневная с чеклистом");
        ct.recurrence = Some(Recurrence::Custom(1, RecurrenceUnit::Days));
        let t = create_task_impl(&pool, ct).await.unwrap();
        // TWO subtasks, one ticked. With a single one, ticking it would itself
        // complete the task (subtasks.rs::complete_if_all_subtasks_done) and clear
        // the checklist, so the state this test wants to set up would be gone
        // before complete_task_impl below was ever called.
        let a = add_subtask_impl(&pool, &t.id, "пункт").await.unwrap();
        add_subtask_impl(&pool, &t.id, "второй пункт").await.unwrap();
        toggle_subtask_impl(&pool, &a.id).await.unwrap();
        assert!(get_subtasks_impl(&pool, &t.id).await.unwrap()[0].done);

        let done = complete_task_impl(&pool, t.id.clone()).await.unwrap();

        // The task moved to its next run, so the checklist must be clean or the
        // repeat arrives already "completed".
        assert_eq!(done.status, "Todo");
        assert!(get_subtasks_impl(&pool, &t.id).await.unwrap().iter().all(|s| !s.done));
        assert!(done.subtasks.iter().all(|s| !s.done));
    }

    #[tokio::test]
    async fn complete_recurring_moves_deadline_and_resets_notifications() {
        let pool = test_pool().await;
        let mut ct = new_task("ежедневная");
        ct.recurrence = Some(Recurrence::Custom(2, RecurrenceUnit::Days));
        let t = create_task_impl(&pool, ct).await.unwrap();

        // Simulate the scheduler having already notified about the old deadline
        sqlx::query("UPDATE tasks SET notified_24h = 1, notified_1h = 1 WHERE id = ?")
            .bind(&t.id).execute(&pool).await.unwrap();

        let before = Utc::now();
        let done = complete_task_impl(&pool, t.id.clone()).await.unwrap();

        // Not closed but moved by +2 days
        assert_eq!(done.status, "Todo");
        assert!(!done.hidden);
        assert!(done.completed_at.is_none());
        let dl = done.deadline.unwrap();
        assert!(dl >= before + chrono::Duration::days(2));
        assert!(dl <= Utc::now() + chrono::Duration::days(2));

        // The notification flags are reset, or nobody learns about the new deadline
        let row = sqlx::query_as::<_, (bool, bool)>(
            "SELECT notified_24h, notified_1h FROM tasks WHERE id = ?")
            .bind(&t.id).fetch_one(&pool).await.unwrap();
        assert_eq!(row, (false, false));
    }

    #[tokio::test]
    async fn complete_recurring_shifts_scheduled_block() {
        let pool = test_pool().await;
        let before = Utc::now() - chrono::Duration::minutes(1);
        let scheduled = before + chrono::Duration::hours(2);
        let mut ct = new_task("ежедневная с блоком");
        ct.recurrence = Some(Recurrence::Daily);
        let t = create_task_impl(&pool, ct).await.unwrap();
        sqlx::query("UPDATE tasks SET scheduled_at = ?, scheduled_mins = 30 WHERE id = ?")
            .bind(scheduled.to_rfc3339()).bind(&t.id)
            .execute(&pool).await.unwrap();

        let done = complete_task_impl(&pool, t.id.clone()).await.unwrap();

        // scheduled_at must shift by +1 day from its original value, not from now
        let expected = scheduled + chrono::Duration::days(1);
        let got = done.scheduled_at.unwrap();
        assert!((got - expected).num_seconds().abs() < 2,
            "expected {expected}, got {got}");
        assert_eq!(done.scheduled_mins, Some(30)); // untouched

        // notified_block is reset (recurring means reset_notifications=true)
        let (block_flag,): (bool,) = sqlx::query_as(
            "SELECT notified_block FROM tasks WHERE id = ?")
            .bind(&t.id).fetch_one(&pool).await.unwrap();
        assert!(!block_flag, "notified_block should be reset");
    }

    // v0.9.89. Completing a recurring task is one logical operation made of two
    // writes: clear the checklist and move the deadline. Until this version they
    // ran without a transaction, so a failure between them left the worst
    // possible mixture — the checklist for the next run wiped while the task
    // still sat at its old deadline.
    //
    // This half only covers the happy path; atomicity itself is asserted by
    // a_failed_second_write_leaves_the_checklist_alone below.
    #[tokio::test]
    async fn completing_a_recurring_task_moves_checklist_and_deadline_together() {
        let pool = test_pool().await;
        let mut ct = new_task("ежедневная с чек-листом");
        ct.recurrence = Some(Recurrence::Daily);
        let t = create_task_impl(&pool, ct).await.unwrap();

        let sub = crate::commands::subtasks::add_subtask_impl(&pool, &t.id, "шаг").await.unwrap();
        crate::commands::subtasks::toggle_subtask_impl(&pool, &sub.id).await.unwrap();

        let before_deadline = sqlx::query_scalar::<_, Option<String>>(
            "SELECT deadline FROM tasks WHERE id = ?")
            .bind(&t.id).fetch_one(&pool).await.unwrap();

        let done = complete_task_impl(&pool, t.id.clone()).await.unwrap();

        // The checklist was cleared for the next run...
        let still_done: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM subtasks WHERE task_id = ? AND done = 1")
            .bind(&t.id).fetch_one(&pool).await.unwrap();
        assert_eq!(still_done, 0, "чек-лист не сброшен для следующего запуска");

        // ...and the task really did move, rather than losing its marks in place.
        let after_deadline = sqlx::query_scalar::<_, Option<String>>(
            "SELECT deadline FROM tasks WHERE id = ?")
            .bind(&t.id).fetch_one(&pool).await.unwrap();
        assert_ne!(
            after_deadline, before_deadline,
            "отметки чек-листа стёрты, а дедлайн остался прежним — ровно та смесь, \
             которую предотвращает транзакция"
        );
        assert_eq!(done.status, "Todo");
        assert!(done.deadline.is_some());

        // The returned value is read after commit and must show the new state.
        assert_eq!(done.subtasks.len(), 1);
        assert!(!done.subtasks[0].done, "возвращённая задача показывает старый чек-лист");
    }

    // The actual point of the transaction, with a real injected failure.
    //
    // A BEFORE UPDATE trigger that aborts is the injection, and picking it took
    // three tries — the first two failed the wrong step, which is the recurring
    // trap of v0.9.86 and v0.9.87:
    //   - breaking the UPDATE's SQL in a scratchpad copy: complete_task_impl then
    //     returns Err at the .unwrap() of the happy-path test, before any
    //     assertion runs, so it is red with or without the transaction;
    //   - renaming a column the second write needs: `SELECT * FROM tasks` at the
    //     top of complete_task_impl maps into TaskRow and needs every column, so
    //     the function dies before the FIRST write and the checklist survives on
    //     its own, no transaction required.
    // A trigger on UPDATE OF tasks touches neither the SELECT nor the subtasks
    // write, so it fails exactly the step being protected.
    //
    // Without the transaction the marks would be gone for good while the task
    // kept its old deadline: the checklist for the next run destroyed and the run
    // itself not advanced — worse than either write failing alone.
    #[tokio::test]
    async fn a_failed_second_write_leaves_the_checklist_alone() {
        let pool = test_pool().await;
        let mut ct = new_task("ежедневная");
        ct.recurrence = Some(Recurrence::Daily);
        let t = create_task_impl(&pool, ct).await.unwrap();

        // TWO subtasks: ticking the only one would auto-complete the task here
        // (subtasks.rs::complete_if_all_subtasks_done) and clear the checklist
        // before the trigger below is even installed, so the failure this test
        // injects would have nothing left to roll back.
        let sub = crate::commands::subtasks::add_subtask_impl(&pool, &t.id, "шаг").await.unwrap();
        crate::commands::subtasks::add_subtask_impl(&pool, &t.id, "второй шаг").await.unwrap();
        crate::commands::subtasks::toggle_subtask_impl(&pool, &sub.id).await.unwrap();

        sqlx::query(
            "CREATE TRIGGER block_task_update BEFORE UPDATE ON tasks
             BEGIN SELECT RAISE(ABORT, 'injected failure'); END")
            .execute(&pool).await.unwrap();

        let result = complete_task_impl(&pool, t.id.clone()).await;
        assert!(result.is_err(), "подготовка теста не сработала: второй UPDATE не упал");

        let still_done: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM subtasks WHERE task_id = ? AND done = 1")
            .bind(&t.id).fetch_one(&pool).await.unwrap();
        assert_eq!(
            still_done, 1,
            "чек-лист сброшен, хотя задача не передвинулась — транзакция не откатила первую запись"
        );
    }

    #[tokio::test]
    async fn complete_weekdays_recurring_moves_to_next_matching_day_not_same_day() {
        use chrono::Datelike;
        let pool = test_pool().await;
        let mut ct = new_task("по будням");
        // The mask covers today's weekday only, which guarantees that a naive
        // "now + something fixed" would break by returning today's date again.
        let today_bit = 1u8 << Utc::now().weekday().num_days_from_monday();
        ct.recurrence = Some(Recurrence::Weekdays(today_bit));
        let t = create_task_impl(&pool, ct).await.unwrap();

        let before = Utc::now();
        let done = complete_task_impl(&pool, t.id.clone()).await.unwrap();

        assert_eq!(done.status, "Todo"); // not closed but moved
        assert!(!done.hidden);
        let dl = done.deadline.unwrap();
        // The next match of the same mask is exactly 7 days out, not "today again"
        assert!((dl - (before + chrono::Duration::days(7))).num_seconds().abs() < 5,
            "expected ~+7 days, got delta {:?}", dl - before);
    }

    #[tokio::test]
    async fn update_deadline_resets_notification_flags() {
        let pool = test_pool().await;
        let t = create_task_impl(&pool, new_task("с дедлайном")).await.unwrap();
        sqlx::query("UPDATE tasks SET notified_24h = 1 WHERE id = ?")
            .bind(&t.id).execute(&pool).await.unwrap();

        let patch = UpdateTask {
            title: None, description: None, status: None, priority: None,
            category: None, tags: None, recurrence: None, project_id: None,
            scheduled_at: None, scheduled_mins: None,
            deadline: Some((Utc::now() + chrono::Duration::days(10)).to_rfc3339()),
        };
        update_task_impl(&pool, t.id.clone(), patch).await.unwrap();

        let (notified,): (bool,) = sqlx::query_as(
            "SELECT notified_24h FROM tasks WHERE id = ?")
            .bind(&t.id).fetch_one(&pool).await.unwrap();
        assert!(!notified);
    }

    #[tokio::test]
    async fn search_finds_by_prefix_and_survives_hyphen() {
        let pool = test_pool().await;
        create_task_impl(&pool, new_task("купить хлеб-2")).await.unwrap();
        create_task_impl(&pool, new_task("помыть машину")).await.unwrap();

        let found = search_tasks_impl(&pool, "хлеб".into()).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "купить хлеб-2");

        // A hyphen is FTS5 syntax; this used to fail with "no such column"
        let found = search_tasks_impl(&pool, "хлеб-2".into()).await.unwrap();
        assert_eq!(found.len(), 1);

        // An empty query yields an empty result, not an error
        assert!(search_tasks_impl(&pool, "  ".into()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_is_soft_hides_from_active_but_keeps_row() {
        let pool = test_pool().await;
        let t = create_task_impl(&pool, new_task("на удаление")).await.unwrap();
        delete_task_impl(&pool, t.id.clone()).await.unwrap();

        // Not among the active ones...
        assert!(get_tasks_impl(&pool).await.unwrap().is_empty());
        // ...but the row is alive and visible in the Trash.
        let trash = get_deleted_tasks_impl(&pool).await.unwrap();
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].id, t.id);
        assert!(trash[0].deleted_at.is_some());
    }

    #[tokio::test]
    async fn restore_returns_task_to_active_list() {
        let pool = test_pool().await;
        let t = create_task_impl(&pool, new_task("восстановить")).await.unwrap();
        delete_task_impl(&pool, t.id.clone()).await.unwrap();
        assert!(get_tasks_impl(&pool).await.unwrap().is_empty());

        restore_task_impl(&pool, t.id.clone()).await.unwrap();

        let active = get_tasks_impl(&pool).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, t.id);
        assert_eq!(active[0].deleted_at, None);
        assert!(get_deleted_tasks_impl(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn purge_actually_removes_row_and_unlinks_notes() {
        use crate::commands::notes::{create_note_impl, get_notes_impl, CreateNote};
        let pool = test_pool().await;
        let t = create_task_impl(&pool, new_task("в корзину и навсегда")).await.unwrap();
        create_note_impl(&pool, CreateNote {
            title: "привязанная".into(),
            content: "x".into(),
            tags: vec![],
            linked_task_id: Some(t.id.clone()),
            project_id: None,
        }).await.unwrap();

        delete_task_impl(&pool, t.id.clone()).await.unwrap();
        purge_deleted_task_impl(&pool, t.id.clone()).await.unwrap();

        assert!(get_deleted_tasks_impl(&pool).await.unwrap().is_empty());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE id = ?")
            .bind(&t.id).fetch_one(&pool).await.unwrap();
        assert_eq!(count, 0);

        let notes = get_notes_impl(&pool).await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].linked_task_id, None);
    }

    // Regression, v0.9.81: purging a task used to leave its dependency rows
    // behind forever. Migration 0031 declares ON DELETE CASCADE, but SQLite
    // leaves foreign key enforcement OFF unless asked and nothing in the app
    // asks, so the schema never cleaned up and neither did the code.
    //
    // The purged task is deliberately on both sides of a link — blocked by one
    // task, blocking another — because the delete has to clear task_id and
    // blocker_id, and a one-sided fix would still pass a one-sided test.
    #[tokio::test]
    async fn purge_removes_dependencies_on_both_sides() {
        use crate::commands::dependencies::add_task_dependency_impl;
        let pool = test_pool().await;
        let blocker = create_task_impl(&pool, new_task("блокер")).await.unwrap();
        let middle = create_task_impl(&pool, new_task("в корзину")).await.unwrap();
        let blocked = create_task_impl(&pool, new_task("ждёт середину")).await.unwrap();

        // blocker -> middle -> blocked
        add_task_dependency_impl(&pool, &middle.id, &blocker.id).await.unwrap();
        add_task_dependency_impl(&pool, &blocked.id, &middle.id).await.unwrap();

        delete_task_impl(&pool, middle.id.clone()).await.unwrap();
        purge_deleted_task_impl(&pool, middle.id.clone()).await.unwrap();

        let orphans: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_dependencies WHERE task_id = ? OR blocker_id = ?",
        )
        .bind(&middle.id)
        .bind(&middle.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(orphans, 0, "dependency rows survived the purge of their task");

        // The untouched pair must stay: the cleanup is scoped to the purged id,
        // not a blunt sweep of the table.
        let other = create_task_impl(&pool, new_task("посторонняя")).await.unwrap();
        add_task_dependency_impl(&pool, &other.id, &blocker.id).await.unwrap();
        purge_deleted_task_impl(&pool, blocked.id.clone()).await.unwrap();
        let survivors: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_dependencies")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(survivors, 1, "the purge deleted links belonging to other tasks");
    }

    // The task-side counterpart of notes.rs::a_trashed_note_surfaces_from_no_read_path.
    //
    // Tasks got their Trash earlier than notes, so their readers were written
    // with deleted_at in mind — but there are far more of them: 22 filtered
    // queries across 7 files, including the waybar status line, the AI planner
    // and the dependency graph, none of which are exercised by the UI tests. A
    // new reader over `tasks` that forgets the filter is silent everywhere else.
    //
    // Covers the paths a user would notice: the active list, the status line
    // (three queries at once), planner context, and blockers. Search has its own
    // test (soft_deleted_task_excluded_from_search) and is not repeated here.
    #[tokio::test]
    async fn a_trashed_task_surfaces_from_no_read_path() {
        use crate::commands::dependencies::{add_task_dependency_impl, blockers_of};
        use crate::commands::planner::plan_day_context;

        let pool = test_pool().await;
        let now = Utc::now();

        let alive = create_task_impl(&pool, new_task("живая задача")).await.unwrap();
        let trashed = create_task_impl(&pool, new_task("выброшенная задача")).await.unwrap();

        // In progress, scheduled and due — so it would show up in every branch of
        // the status line if the filter were missing.
        sqlx::query(
            "UPDATE tasks SET status = 'InProgress', scheduled_at = ?, deadline = ? WHERE id = ?"
        )
        .bind(now.to_rfc3339())
        .bind((now - chrono::Duration::hours(1)).to_rfc3339())
        .bind(&trashed.id)
        .execute(&pool).await.unwrap();

        // The trashed task blocks a live one: a blocker in the Trash must not
        // hold anything back (v0.9.56).
        add_task_dependency_impl(&pool, &alive.id, &trashed.id).await.unwrap();

        delete_task_impl(&pool, trashed.id.clone()).await.unwrap();

        let active = get_tasks_impl(&pool).await.unwrap();
        assert_eq!(active.len(), 1, "активный список отдал задачу из Корзины");
        assert_eq!(active[0].id, alive.id);

        // The status line is read by waybar, outside the window — no UI test sees
        // it. One call covers its scheduled / in-progress / due queries.
        let status = crate::status::status_payload(&pool, now).await.unwrap();
        let rendered = format!("{status:?}");
        assert!(
            !rendered.contains("выброшенная"),
            "строка состояния показала задачу из Корзины: {rendered}"
        );

        // The planner feeds this text to the model: a trashed task here would be
        // planned into the user's day.
        // plan_day_context errors when the free window has closed (late in the
        // day), which is about the clock, not the filter — only assert when it
        // produced a context.
        if let Ok(ctx) = plan_day_context(&pool, now).await {
            assert!(
                !ctx.prompt.contains("выброшенная"),
                "планировщик подал задачу из Корзины в промпт модели: {}",
                ctx.prompt
            );
            assert!(
                !ctx.candidates.values().any(|t| t.contains("выброшенная")),
                "задача из Корзины попала в кандидаты планировщика"
            );
        }

        let blockers = blockers_of(&pool, &alive.id).await.unwrap();
        assert!(
            blockers.is_empty(),
            "задача из Корзины продолжает блокировать живую: {blockers:?}"
        );

        // The Trash itself is the one place it must appear.
        let trash = get_deleted_tasks_impl(&pool).await.unwrap();
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].id, trashed.id);
    }

    #[tokio::test]
    async fn soft_deleted_task_excluded_from_search() {
        let pool = test_pool().await;
        let t = create_task_impl(&pool, new_task("искомая задача про хлеб")).await.unwrap();
        assert_eq!(search_tasks_impl(&pool, "хлеб".into()).await.unwrap().len(), 1);

        delete_task_impl(&pool, t.id).await.unwrap();
        assert!(search_tasks_impl(&pool, "хлеб".into()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn schedule_block_set_move_and_clear() {
        let pool = test_pool().await;
        let t = create_task_impl(&pool, new_task("блок")).await.unwrap();
        let start = Utc::now() + chrono::Duration::hours(2);

        let patch = |sa: Option<String>, mins: Option<i64>| UpdateTask {
            title: None, description: None, status: None, priority: None,
            category: None, tags: None, recurrence: None, project_id: None,
            deadline: None, scheduled_at: sa, scheduled_mins: mins,
        };

        // assign a block
        let up = update_task_impl(&pool, t.id.clone(), patch(Some(start.to_rfc3339()), Some(45))).await.unwrap();
        assert_eq!(up.scheduled_mins, Some(45));
        assert!(up.scheduled_at.is_some());

        // moving it resets notified_block
        sqlx::query("UPDATE tasks SET notified_block = 1 WHERE id = ?")
            .bind(&t.id).execute(&pool).await.unwrap();
        update_task_impl(&pool, t.id.clone(), patch(Some((start + chrono::Duration::hours(1)).to_rfc3339()), None)).await.unwrap();
        let notified: bool = sqlx::query_scalar("SELECT notified_block FROM tasks WHERE id = ?")
            .bind(&t.id).fetch_one(&pool).await.unwrap();
        assert!(!notified);

        // the duration is clamped from below
        let up = update_task_impl(&pool, t.id.clone(), patch(None, Some(5))).await.unwrap();
        assert_eq!(up.scheduled_mins, Some(15));

        // an empty string clears the block entirely
        let up = update_task_impl(&pool, t.id.clone(), patch(Some(String::new()), None)).await.unwrap();
        assert_eq!(up.scheduled_at, None);
        assert_eq!(up.scheduled_mins, None);
    }

    #[tokio::test]
    async fn soft_delete_keeps_note_link_intact() {
        // Soft deletion does NOT touch note links or subtasks — that is what
        // separates it from purge_deleted_task (see
        // purge_actually_removes_row_and_unlinks_notes).
        use crate::commands::notes::{create_note_impl, get_notes_impl, CreateNote};
        let pool = test_pool().await;
        let t = create_task_impl(&pool, new_task("с заметкой")).await.unwrap();
        create_note_impl(&pool, CreateNote {
            title: "привязанная".into(),
            content: "x".into(),
            tags: vec![],
            linked_task_id: Some(t.id.clone()),
            project_id: None,
        }).await.unwrap();

        delete_task_impl(&pool, t.id.clone()).await.unwrap();

        let notes = get_notes_impl(&pool).await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].linked_task_id, Some(t.id));
    }

    async fn set_completed_at(pool: &SqlitePool, id: &str, completed_at: DateTime<Utc>) {
        sqlx::query("UPDATE tasks SET completed_at = ? WHERE id = ?")
            .bind(completed_at.to_rfc3339())
            .bind(id)
            .execute(pool).await.unwrap();
    }

    // Automatic history cleanup: old completed tasks move to the Trash by soft
    // deletion (deleted_at); completed_at is left alone.
    #[tokio::test]
    async fn cleanup_moves_only_old_completed_hidden_tasks_to_trash() {
        let pool = test_pool().await;
        let now = Utc::now();
        let cutoff = now - chrono::Duration::days(90);

        let old = complete_task_impl(&pool, create_task_impl(&pool, new_task("старая")).await.unwrap().id).await.unwrap();
        set_completed_at(&pool, &old.id, now - chrono::Duration::days(120)).await;

        let recent = complete_task_impl(&pool, create_task_impl(&pool, new_task("недавняя")).await.unwrap().id).await.unwrap();
        set_completed_at(&pool, &recent.id, now - chrono::Duration::days(10)).await;

        let active = create_task_impl(&pool, new_task("активная")).await.unwrap();

        let moved = cleanup_old_history_impl(&pool, cutoff).await.unwrap();
        assert_eq!(moved, 1);

        let trashed = get_deleted_tasks_impl(&pool).await.unwrap();
        assert_eq!(trashed.len(), 1);
        assert_eq!(trashed[0].id, old.id);
        // completed_at is untouched, so dashboard statistics stay accurate
        assert!(trashed[0].completed_at.is_some());

        let visible_ids: Vec<String> = get_tasks_impl(&pool).await.unwrap().into_iter().map(|t| t.id).collect();
        assert!(visible_ids.contains(&recent.id));
        assert!(visible_ids.contains(&active.id));
        assert!(!visible_ids.contains(&old.id));
    }

    #[tokio::test]
    async fn cleanup_is_idempotent_and_ignores_active_tasks() {
        let pool = test_pool().await;
        let now = Utc::now();
        let old = complete_task_impl(&pool, create_task_impl(&pool, new_task("старая")).await.unwrap().id).await.unwrap();
        set_completed_at(&pool, &old.id, now - chrono::Duration::days(200)).await;

        let cutoff = now - chrono::Duration::days(90);
        assert_eq!(cleanup_old_history_impl(&pool, cutoff).await.unwrap(), 1);
        // A second run: already in the Trash, so it is not counted again
        assert_eq!(cleanup_old_history_impl(&pool, cutoff).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn history_cleanup_due_respects_setting_and_24h_gate() {
        use crate::commands::settings::set_setting;
        let pool = test_pool().await;

        // Disabled by default (history_cleanup_months = 0)
        assert!(!history_cleanup_due(&pool).await);

        set_setting(&pool, "history_cleanup_months", "6").await.unwrap();
        assert!(history_cleanup_due(&pool).await, "включено и ни разу не запускалось — пора");

        let recent = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        set_setting(&pool, "last_history_cleanup", &recent).await.unwrap();
        assert!(!history_cleanup_due(&pool).await, "недавно запускалось — рано");

        let long_ago = (Utc::now() - chrono::Duration::hours(25)).to_rfc3339();
        set_setting(&pool, "last_history_cleanup", &long_ago).await.unwrap();
        assert!(history_cleanup_due(&pool).await, "прошло больше 24ч — пора снова");
    }

    // v0.9.71: the point of the migration. A DB failure used to arrive as bare
    // sqlx text ("no such table: tasks"), which says nothing to the user about
    // what class of problem it is. AppError::Db adds a prefix the frontend then
    // translates (src/lib/errorText.ts).
    #[tokio::test]
    async fn db_error_carries_a_prefix() {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE tasks").execute(&pool).await.unwrap();

        let err = get_tasks_impl(&pool).await.unwrap_err().to_string();
        assert!(
            err.starts_with("Ошибка базы данных: "),
            "сбой БД должен нести технический префикс, получено: {err}"
        );
        // The sqlx detail must survive: without it the message says nothing.
        assert!(err.contains("tasks"), "детали от sqlx потерялись: {err}");
    }

    // The other side of the same coin: a domain error is the message the code
    // wrote, verbatim. Wrapping it into AppError::Db would prepend a technical
    // prefix and the frontend would then translate a head that is not its own.
    #[tokio::test]
    async fn domain_errors_are_not_wrapped() {
        let pool = test_pool().await;
        let mut blank = new_task("");
        blank.title = "   ".into();

        let err = create_task_impl(&pool, blank).await.unwrap_err().to_string();
        assert_eq!(err, "Название задачи не может быть пустым");
    }
}


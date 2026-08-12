use sqlx::SqlitePool;
use sqlx::migrate::MigrateDatabase;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};

pub async fn init_db(db_path: &str) -> Result<SqlitePool, sqlx::Error> {
    if !sqlx::Sqlite::database_exists(db_path).await.unwrap_or(false) {
        sqlx::Sqlite::create_database(db_path).await?;
    }

    // Both pragmas set explicitly (v0.9.88). Checked against sqlx-sqlite 0.8.6
    // rather than assumed: in its default pragma map journal_mode and synchronous
    // are both `None` — sqlx sets neither. (foreign_keys is the exception, forced
    // to ON, which is what v0.9.81 established the hard way. busy_timeout is
    // already 5s by default and is deliberately left alone: it is not about
    // durability, it only turns an immediate SQLITE_BUSY into a delayed one.)
    //
    // So the live DB being in WAL today is not an inherited default at all — it
    // is written in the file from earlier runs. A freshly created database, or
    // one that arrived through Import, would come up in `delete` mode.
    //
    // WAL is load-bearing for three separate things: export_impl (the whole
    // reason VACUUM INTO is needed — see backup.rs), apply_pending_import (it
    // removes the stale -wal next to the imported file) and status.rs (the waybar
    // CLI opens the DB alongside the running app).
    //
    // synchronous=FULL is what makes a commit survive a power cut and not merely
    // a process crash. SQLite's compiled-in default is FULL, so this pins the
    // value rather than changing it.
    //
    // Via connect options, not `PRAGMA` on the pool: synchronous is per
    // connection, and a pool opens connections on demand — a one-off query would
    // configure exactly one of them.
    let options = db_path
        .parse::<SqliteConnectOptions>()?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full);
    let pool = SqlitePool::connect_with(options).await?;

    sqlx::migrate!("./src/db/migrations")
        .run(&pool)
        .await
        .map_err(|e: sqlx::migrate::MigrateError| sqlx::Error::Protocol(e.to_string()))?;

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Row;

    // init_db works with a file-backed DB (create_database/database_exists), so we
    // test against a temporary file rather than sqlite::memory:.
    fn temp_db_url() -> (String, std::path::PathBuf) {
        let path = std::env::temp_dir()
            .join(format!("kriptag-test-{}.db", uuid::Uuid::new_v4()));
        (format!("sqlite:{}?mode=rwc", path.display()), path)
    }

    fn cleanup(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[tokio::test]
    async fn init_db_creates_file_and_applies_all_migrations() {
        let (url, path) = temp_db_url();
        let pool = init_db(&url).await.expect("init_db failed");

        // Every key table from migrations 0001-0007 is present
        for table in ["tasks", "notes", "settings", "activity_log", "tasks_fts"] {
            let row = sqlx::query(
                "SELECT name FROM sqlite_master WHERE type IN ('table','view') AND name = ?"
            )
            .bind(table)
            .fetch_optional(&pool)
            .await
            .unwrap();
            assert!(row.is_some(), "таблица {table} не создана миграциями");
        }

        // A second init_db over an existing file does not fail (idempotence)
        drop(pool);
        let pool2 = init_db(&url).await.expect("повторный init_db упал");
        drop(pool2);
        cleanup(&path);
    }

    // v0.9.88, modelled on foreign_keys_are_enforced_so_cascades_actually_fire.
    //
    // The pragmas are not asserted on their own: a DB already in WAL from an
    // earlier run would satisfy that no matter what init_db does. The load-
    // bearing half is the second one — a database that arrives in `delete` mode
    // (an import, or a file made by another tool) must be switched over, and that
    // is the case sqlx's defaults do NOT cover: in sqlx-sqlite 0.8.6 the default
    // pragma map leaves journal_mode and synchronous unset entirely.
    #[tokio::test]
    async fn wal_and_full_sync_are_pinned_not_inherited() {
        let (url, path) = temp_db_url();
        let pool = init_db(&url).await.unwrap();

        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            mode.to_lowercase(),
            "wal",
            "journal_mode не WAL: от него зависят export_impl (VACUUM INTO), \
             apply_pending_import (удаление устаревшего -wal) и status.rs"
        );

        let sync: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sync, 2, "synchronous не FULL — коммит перестанет переживать отключение питания");

        // The consequence, not just the setting: writes really do go through a
        // -wal file next to the database.
        sqlx::query("INSERT INTO tasks (id, title, created_at, updated_at) VALUES ('wal-probe', 'x', '2026-01-01', '2026-01-01')")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            path.with_extension("db-wal").exists(),
            "рядом с базой нет -wal: запись идёт не через WAL"
        );

        drop(pool);
        cleanup(&path);

        // The half that sqlx cannot give us: a database handed over in `delete`
        // mode is switched to WAL by init_db.
        let (url2, path2) = temp_db_url();
        let legacy = SqlitePool::connect(&url2).await.unwrap();
        sqlx::query("PRAGMA journal_mode=DELETE").execute(&legacy).await.unwrap();
        let before: String = sqlx::query_scalar("PRAGMA journal_mode").fetch_one(&legacy).await.unwrap();
        assert_eq!(before.to_lowercase(), "delete", "подготовка теста не сработала");
        legacy.close().await;

        let pool2 = init_db(&url2).await.unwrap();
        let after: String = sqlx::query_scalar("PRAGMA journal_mode").fetch_one(&pool2).await.unwrap();
        assert_eq!(
            after.to_lowercase(),
            "wal",
            "база, пришедшая в режиме delete (импорт, чужой инструмент), осталась не в WAL"
        );

        drop(pool2);
        cleanup(&path2);
    }

    #[tokio::test]
    async fn fts_triggers_sync_on_insert_update_delete() {
        // Regression for the 0004 bug: the tasks_fts triggers must work by rowid,
        // otherwise the index diverges after an UPDATE and MATCH fails as
        // "malformed".
        let (url, path) = temp_db_url();
        let pool = init_db(&url).await.unwrap();

        sqlx::query(
            "INSERT INTO tasks (id, title, created_at, updated_at)
             VALUES (?, ?, ?, ?)"
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind("покормить кота")
        .bind("2026-07-09T10:00:00+00:00")
        .bind("2026-07-09T10:00:00+00:00")
        .execute(&pool).await.unwrap();

        let found: i64 = sqlx::query("SELECT COUNT(*) AS c FROM tasks_fts WHERE tasks_fts MATCH ?")
            .bind("кот*")
            .fetch_one(&pool).await.unwrap().get("c");
        assert_eq!(found, 1, "FTS не нашёл задачу после INSERT");

        // UPDATE: the old title no longer matches, the new one does, with no malformed error
        sqlx::query("UPDATE tasks SET title = ? WHERE title = ?")
            .bind("полить цветы").bind("покормить кота")
            .execute(&pool).await.unwrap();

        let old_gone: i64 = sqlx::query("SELECT COUNT(*) AS c FROM tasks_fts WHERE tasks_fts MATCH ?")
            .bind("кот*")
            .fetch_one(&pool).await.unwrap().get("c");
        assert_eq!(old_gone, 0, "FTS всё ещё находит старый заголовок после UPDATE");

        let new_found: i64 = sqlx::query("SELECT COUNT(*) AS c FROM tasks_fts WHERE tasks_fts MATCH ?")
            .bind("цвет*")
            .fetch_one(&pool).await.unwrap().get("c");
        assert_eq!(new_found, 1, "FTS не нашёл задачу по новому заголовку");

        // DELETE: the index is cleared
        sqlx::query("DELETE FROM tasks WHERE title = ?")
            .bind("полить цветы")
            .execute(&pool).await.unwrap();
        let after_delete: i64 = sqlx::query("SELECT COUNT(*) AS c FROM tasks_fts WHERE tasks_fts MATCH ?")
            .bind("цвет*")
            .fetch_one(&pool).await.unwrap().get("c");
        assert_eq!(after_delete, 0, "FTS не очистился после DELETE");

        drop(pool);
        cleanup(&path);
    }

    // Three migrations (0017, 0031 x2) rely on ON DELETE CASCADE, and that
    // cascade only runs when SQLite's foreign key enforcement is on. Nothing in
    // this file turns it on: it is on because sqlx sets `PRAGMA foreign_keys` in
    // its default connect options, while raw SQLite defaults it to OFF.
    //
    // So an inherited library default is load-bearing for data integrity. A sqlx
    // upgrade that changed it would not break the build and would not fail any
    // other test — it would silently start leaving orphaned rows behind on every
    // delete. This asserts the assumption directly, then proves the consequence
    // that actually matters.
    //
    // The version that added this test was originally written to fix "orphaned
    // dependencies on purge", on the strength of a sqlite3-CLI experiment showing
    // the pragma off. The CLI has its own default; it was never measuring this
    // connection. Hence: assert the pragma where the app opens its pool.
    #[tokio::test]
    async fn foreign_keys_are_enforced_so_cascades_actually_fire() {
        let (url, path) = temp_db_url();
        let pool = init_db(&url).await.unwrap();

        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(
            fk, 1,
            "PRAGMA foreign_keys выключен — ON DELETE CASCADE в 0017 и 0031 \
             перестанет срабатывать, и удаление начнёт молча оставлять сироты"
        );

        let blocked = uuid::Uuid::new_v4().to_string();
        let blocker = uuid::Uuid::new_v4().to_string();
        for id in [&blocked, &blocker] {
            sqlx::query("INSERT INTO tasks (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)")
                .bind(id)
                .bind("живая")
                .bind("2026-08-05T10:00:00+00:00")
                .bind("2026-08-05T10:00:00+00:00")
                .execute(&pool).await.unwrap();
        }
        sqlx::query(
            "INSERT INTO task_dependencies (task_id, blocker_id, created_at) VALUES (?, ?, ?)"
        )
        .bind(&blocked).bind(&blocker).bind("2026-08-05T10:00:00+00:00")
        .execute(&pool).await.unwrap();

        // A bare DELETE, without the manual cleanup purge_deleted_task_impl does
        // — this measures the schema, not the command.
        sqlx::query("DELETE FROM tasks WHERE id = ?")
            .bind(&blocker).execute(&pool).await.unwrap();

        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_dependencies")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(left, 0, "каскад из 0031 не сработал: связь пережила удаление блокера");

        drop(pool);
        cleanup(&path);
    }
}

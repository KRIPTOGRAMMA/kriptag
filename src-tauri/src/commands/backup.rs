use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zip::ZipWriter;
use zip::ZipArchive;
use zip::write::SimpleFileOptions;
use tauri::Manager;
use crate::error::{AppError, AppResult};
use crate::commands::settings::{get_setting, set_setting};

// Turns automatic backups on the first time the app runs, pointing them at a
// folder inside the app's own data dir.
//
// Until v0.9.85 auto_backup_dir defaulted to empty, auto_backup_due returned
// false for an empty dir, and nothing in the UI said so — the result was an app
// that had never once backed itself up while looking like it might have. Off by
// default is a defensible choice only when the user is told; it wasn't.
//
// The marker key is what separates "never configured" from "deliberately turned
// off". Without it, clearing the folder in Settings would be undone on the next
// launch and the setting would be impossible to switch off.
pub async fn ensure_default_backup_dir(pool: &sqlx::SqlitePool, data_dir: &Path) -> AppResult<()> {
    if get_setting(pool, "auto_backup_initialized").await.is_some() {
        return Ok(());
    }
    set_setting(pool, "auto_backup_initialized", "1").await?;

    // Respect a folder the user somehow already has (e.g. a DB from an older
    // build that predates the marker).
    if get_setting(pool, "auto_backup_dir").await.is_some_and(|d| !d.trim().is_empty()) {
        return Ok(());
    }

    let dir = data_dir.join("backups");
    fs::create_dir_all(&dir)?;
    set_setting(pool, "auto_backup_dir", &dir.to_string_lossy()).await?;
    Ok(())
}

// The DB runs in WAL mode: recent writes live in data.db-wal, not in data.db.
// Copying the file alone is not enough — the snapshot would be incomplete.
// VACUUM INTO writes a consistent copy of the whole DB (WAL included) to a
// separate file.
pub async fn export_impl(pool: &sqlx::SqlitePool, data_dir: &Path, path: &str) -> AppResult<()> {
    let snapshot_path = data_dir.join("data.db.export");

    let _ = std::fs::remove_file(&snapshot_path); // VACUUM INTO requires the file to be absent
    sqlx::query("VACUUM INTO ?")
        .bind(snapshot_path.to_string_lossy().as_ref())
        .execute(pool)
        .await?;

    let result: AppResult<()> = (|| {
        let zip_file = File::create(path)?;
        let mut zip = ZipWriter::new(zip_file);
        let options = SimpleFileOptions::default();

        zip.start_file("data.db", options)?;
        let mut db_file = File::open(&snapshot_path)?;
        let mut buf = Vec::new();
        db_file.read_to_end(&mut buf)?;
        zip.write_all(&buf)?;

        zip.finish()?;
        Ok(())
    })();

    let _ = std::fs::remove_file(&snapshot_path);
    result
}

#[tauri::command]
pub async fn export(
    app: tauri::AppHandle,
    pool: tauri::State<'_, sqlx::SqlitePool>,
    path: String,
) -> AppResult<()> {
    let data_dir = app.path().app_data_dir()?;
    export_impl(pool.inner(), &data_dir, &path).await
}

// data.db must not be overwritten while the pool is live: the activity loop
// writes to the DB every 60 seconds and would clobber the import. We drop a
// staging file and restart the app — apply_pending_import() picks it up before
// the pool is opened.
pub async fn import_impl(data_dir: &Path, path: &str) -> AppResult<()> {
    let staging_path = data_dir.join("data.db.import");

    let zip_file = File::open(path)?;
    let mut archive = ZipArchive::new(zip_file)?;

    let mut entry = archive.by_name("data.db")?;
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf)?;

    std::fs::write(&staging_path, &buf)?;

    // Validation belongs here and not in apply_pending_import: that one runs
    // before the pool and the window exist (lib.rs), so there is nobody to report
    // an error to. Here the user is standing in front of a dialog.
    //
    // Until v0.9.87 the only check was that the zip contained an entry named
    // data.db — the bytes were never looked at. A corrupt or foreign file went
    // straight to staging, apply_pending_import renamed it over the live database
    // on the next launch, and init_db then failed with .expect(): real data gone,
    // no window, no message.
    if let Err(e) = validate_import_db(&staging_path).await {
        let _ = std::fs::remove_file(&staging_path);
        return Err(e);
    }
    Ok(())
}

// What an archive holds, shown before the import is committed to (v0.9.92).
//
// Why: until now the only thing distinguishing two archives in the file dialog
// was the timestamp in the name, and the import is irreversible. In a single
// hour of testing the same mistake was made twice — an older snapshot picked,
// a note created after it silently rolled back. The name says when the copy was
// made; it does not say what is inside or what the current database would lose.
#[derive(Debug, serde::Serialize)]
pub struct ImportPreview {
    pub tasks: i64,
    pub notes: i64,
    // The same counts for the database being replaced, so the dialog can show
    // the difference rather than a number with nothing to compare it against.
    pub current_tasks: i64,
    pub current_notes: i64,
    // How current the copy is: the latest updated_at across tasks and notes,
    // not created_at. Editing an old note does not create a row, so created_at
    // dates the oldest thing the copy proves rather than the newest — on the
    // real database the two differ by two days, and "Copy from 1 Aug" for a
    // 6 Aug snapshot is exactly the confusion this whole version exists to end.
    // Empty when the archive holds neither tasks nor notes.
    pub newest: String,
    // How many rows in the LIVE database are newer than `newest` — exactly what
    // the import would discard. Zero means nothing is at stake.
    pub losing_tasks: i64,
    pub losing_notes: i64,
}

#[tauri::command]
pub async fn preview_import(
    pool: tauri::State<'_, sqlx::SqlitePool>,
    path: String,
) -> AppResult<ImportPreview> {
    // Read into memory and never touch data.db.import: merely looking at an
    // archive must not stage half an import.
    let mut archive = ZipArchive::new(File::open(&path)?)?;
    let mut buf = Vec::new();
    archive.by_name("data.db")?.read_to_end(&mut buf)?;

    let tmp = std::env::temp_dir().join(format!("kriptag-preview-{}.db", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, &buf)?;
    let result = preview_from_file(pool.inner(), &tmp).await;
    let _ = std::fs::remove_file(&tmp);
    result
}

async fn preview_from_file(live: &sqlx::SqlitePool, file: &Path) -> AppResult<ImportPreview> {
    validate_import_db(file).await?;

    let url = format!("sqlite:{}?mode=ro", file.display());
    let snap = sqlx::SqlitePool::connect(&url)
        .await
        .map_err(|_| AppError::Other("Файл не является базой данных Kriptag.".into()))?;

    let result = (async {
        let tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks").fetch_one(&snap).await?;
        let notes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes").fetch_one(&snap).await?;
        // created_at is compared as text: everything is written through
        // to_rfc3339 with a fixed layout, so lexicographic order matches
        // chronological order.
        // Shown to the user: how current the copy is.
        let newest: String = sqlx::query_scalar(
            "SELECT COALESCE(MAX(c), '') FROM (
                 SELECT MAX(updated_at) AS c FROM tasks
                 UNION ALL SELECT MAX(updated_at) FROM notes)",
        )
        .fetch_one(&snap)
        .await
        .unwrap_or_default();

        // The threshold for counting losses is a different question and stays on
        // created_at: "newer than the copy" must mean rows the copy never had.
        // Comparing updated_at instead would count an edit to an old note that
        // the archive does contain, and the warning would cry wolf.
        let created_cutoff: String = sqlx::query_scalar(
            "SELECT COALESCE(MAX(c), '') FROM (
                 SELECT MAX(created_at) AS c FROM tasks
                 UNION ALL SELECT MAX(created_at) FROM notes)",
        )
        .fetch_one(&snap)
        .await
        .unwrap_or_default();

        // The half that actually prevents the mistake: what the live database
        // holds beyond this snapshot.
        let (losing_tasks, losing_notes) = if created_cutoff.is_empty() {
            (0, 0)
        } else {
            (
                sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE created_at > ?")
                    .bind(&created_cutoff).fetch_one(live).await.unwrap_or(0),
                sqlx::query_scalar("SELECT COUNT(*) FROM notes WHERE created_at > ?")
                    .bind(&created_cutoff).fetch_one(live).await.unwrap_or(0),
            )
        };

        let current_tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
            .fetch_one(live).await.unwrap_or(0);
        let current_notes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes")
            .fetch_one(live).await.unwrap_or(0);

        Ok(ImportPreview {
            tasks, notes, current_tasks, current_notes, newest, losing_tasks, losing_notes,
        })
    })
    .await;

    snap.close().await;
    result
}

// Two questions, both cheap: is this a working SQLite file at all, and is it a
// database of *this* app. The second one is the realistic mistake — picking a
// backup belonging to some other program.
async fn validate_import_db(staging: &Path) -> AppResult<()> {
    let url = format!("sqlite:{}?mode=ro", staging.display());
    let pool = sqlx::SqlitePool::connect(&url)
        .await
        .map_err(|_| AppError::Other("Файл не является базой данных Kriptag.".into()))?;

    let result = (async {
        let check: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .map_err(|_| AppError::Other("Файл не является базой данных Kriptag.".into()))?;
        if check != "ok" {
            return Err(AppError::Other("Архив повреждён: база не проходит проверку целостности.".into()));
        }

        for table in ["tasks", "notes", "settings"] {
            let found: Option<String> = sqlx::query_scalar(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?",
            )
            .bind(table)
            .fetch_optional(&pool)
            .await?;
            if found.is_none() {
                return Err(AppError::Other(
                    "Это база данных другого приложения: в ней нет таблиц Kriptag.".into(),
                ));
            }
        }
        Ok(())
    })
    .await;

    pool.close().await;
    result
}

#[tauri::command]
pub async fn import(app: tauri::AppHandle, path: String) -> AppResult<()> {
    let data_dir = app.path().app_data_dir()?;
    import_impl(&data_dir, &path).await?;
    app.restart()
}

// Decides whether an automatic backup is due: at least 24h since the last one
// and a folder is configured.
pub async fn auto_backup_due(pool: &sqlx::SqlitePool) -> bool {
    let dir = get_setting(pool, "auto_backup_dir").await;
    let dir = match dir {
        Some(d) if !d.trim().is_empty() => d,
        _ => return false,
    };
    if !Path::new(&dir).is_dir() {
        return false;
    }
    let last = get_setting(pool, "last_auto_backup").await;
    match last {
        Some(ts) => {
            let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&ts) else {
                return true;
            };
            let elapsed = chrono::Utc::now() - parsed.with_timezone(&chrono::Utc);
            elapsed >= chrono::Duration::hours(24)
        }
        None => true, // never run before — it is due
    }
}

// Performs an automatic backup: export plus rotation. Returns the filename.
pub async fn auto_backup_impl(
    pool: &sqlx::SqlitePool,
    data_dir: &Path,
) -> AppResult<String> {
    let backup_dir = get_setting(pool, "auto_backup_dir").await.unwrap_or_default();
    let keep: usize = get_setting(pool, "auto_backup_keep").await
        .and_then(|v| v.parse().ok())
        .unwrap_or(7)
        .max(1);

    let dir = PathBuf::from(&backup_dir);
    let now = chrono::Local::now();
    let filename = format!("kriptag-backup-{}.zip", now.format("%Y-%m-%d-%H%M"));
    let path = dir.join(&filename);

    export_impl(pool, data_dir, path.to_str().unwrap()).await?;

    // Rotation: delete the oldest files beyond keep
    let mut entries: Vec<_> = fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with("kriptag-backup-")
                && e.file_name().to_string_lossy().ends_with(".zip")
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());
    while entries.len() > keep {
        if let Some(oldest) = entries.first() {
            let _ = fs::remove_file(oldest.path());
            entries.remove(0);
        }
    }

    set_setting(pool, "last_auto_backup", &now.to_rfc3339()).await?;
    Ok(filename)
}

// The failure path of the automatic backup, kept apart from auto_backup_impl so
// that one stays testable on its own.
//
// Why this exists (v0.9.86): the rotation step above used to be
// `read_dir(&dir).unwrap_or_else(|_| panic!(...))`. The loop that calls it runs
// inside tokio::spawn (lib.rs), and a panic in a spawned task does not bring the
// app down — it kills only that task. Backups would stop forever while the app
// went on looking perfectly healthy. That is the same shape of silent failure as
// the empty auto_backup_dir fixed in v0.9.85, so the error has to be recorded
// where the user can see it.
//
// No catch_unwind here on purpose: it would be armour around the one panic we
// just removed, and it would hide the next one just as well.
pub async fn run_auto_backup(pool: &sqlx::SqlitePool, data_dir: &Path) -> AppResult<String> {
    match auto_backup_impl(pool, data_dir).await {
        Ok(filename) => {
            // Cleared on success, otherwise a single bad run would leave a
            // warning in Settings for good.
            let _ = set_setting(pool, "last_auto_backup_error", "").await;
            Ok(filename)
        }
        Err(e) => {
            let record = format!("{}\t{}", chrono::Utc::now().to_rfc3339(), e);
            let _ = set_setting(pool, "last_auto_backup_error", &record).await;
            Err(e)
        }
    }
}

#[tauri::command]
pub async fn do_auto_backup(
    app: tauri::AppHandle,
    pool: tauri::State<'_, sqlx::SqlitePool>,
) -> Result<String, String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    run_auto_backup(pool.inner(), &data_dir)
        .await
        .map_err(|e| e.to_string())
}

// After an import the freshly installed database carries the last_auto_backup of
// the copy — usually old, sometimes absent. auto_backup_due then sees a backup
// that is more than 24h overdue and takes one within the minute (v0.9.92).
//
// That extra copy is not merely noise: it consumes a rotation slot, so with
// keep = 7 a few imports in a row can push out the very archive the user is
// trying to get back to. Observed on the real installation — the 19:08 archive
// holding a recovered note was rotated away while we were testing.
//
// The clock is therefore reset to "just now": the state on disk was replaced
// wholesale, and it came from a backup to begin with.
pub async fn note_import_as_fresh_backup(pool: &sqlx::SqlitePool) {
    let _ = set_setting(pool, "last_auto_backup", &chrono::Local::now().to_rfc3339()).await;
}

// Returns whether a staged import was actually applied, so the caller knows to
// reset the backup clock.
pub fn apply_pending_import(data_dir: &std::path::Path) -> bool {
    let staging = data_dir.join("data.db.import");
    if staging.exists() {
        let live = data_dir.join("data.db");

        // The rename below destroys the current database. Keep it aside first —
        // a rename, not a copy: same filesystem, atomic, no doubling of space.
        //
        // If the safety net itself fails, abort the whole import and leave both
        // files alone. A failed safety net must not end up worse than no safety
        // net: the staging file survives and the next launch tries again.
        if live.exists() {
            let _ = std::fs::remove_file(data_dir.join("data.db.pre-import"));
            if std::fs::rename(&live, data_dir.join("data.db.pre-import")).is_err() {
                return false;
            }
        }

        let _ = std::fs::rename(&staging, &live);
        // Otherwise WAL leftovers from the old DB would be replayed over the
        // imported one and silently roll the import back.
        let _ = std::fs::remove_file(data_dir.join("data.db-wal"));
        let _ = std::fs::remove_file(data_dir.join("data.db-shm"));
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kriptag-test-{}-{}", name, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // Not sqlite::memory: — with a pool over an in-memory DB every connection
    // sees its own empty database and VACUUM INTO may end up in the wrong place.
    // A file-backed DB matches production.
    async fn test_pool(dir: &Path) -> sqlx::SqlitePool {
        let url = format!("sqlite:{}?mode=rwc", dir.join("source.db").display());
        let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
        sqlx::migrate!("./src/db/migrations").run(&pool).await.unwrap();
        pool
    }

    async fn insert_task_at(pool: &sqlx::SqlitePool, title: &str, created_at: &str) {
        sqlx::query(
            "INSERT INTO tasks (id, title, status, priority, category, recurrence, tags, hidden, created_at, updated_at)
             VALUES (?, ?, 'Todo', 'Medium', 'Work', 'None', '[]', 0, ?, ?)")
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(title)
            .bind(created_at)
            .bind(created_at)
            .execute(pool).await.unwrap();
    }

    async fn insert_task(pool: &sqlx::SqlitePool, title: &str) {
        sqlx::query(
            "INSERT INTO tasks (id, title, status, priority, category, recurrence, tags, hidden, created_at, updated_at)
             VALUES (?, ?, 'Todo', 'Medium', 'Work', 'None', '[]', 0, '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')")
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(title)
            .execute(pool).await.unwrap();
    }

    // The full cycle: export to zip -> import into staging -> apply on "restart"
    // -> open the imported DB and check the data.
    #[tokio::test]
    async fn export_import_round_trip() {
        let dir = tmp_dir("roundtrip");
        let pool = test_pool(&dir).await;
        insert_task(&pool, "задача для бэкапа").await;

        let zip_path = dir.join("backup.zip");
        export_impl(&pool, &dir, zip_path.to_str().unwrap()).await.unwrap();
        assert!(zip_path.exists());
        // the temporary VACUUM INTO snapshot has been cleaned up
        assert!(!dir.join("data.db.export").exists());

        import_impl(&dir, zip_path.to_str().unwrap()).await.unwrap();
        assert!(dir.join("data.db.import").exists());

        // Simulate the pre-restart state: an old DB with WAL leftovers
        std::fs::write(dir.join("data.db"), b"old-db").unwrap();
        std::fs::write(dir.join("data.db-wal"), b"stale-wal").unwrap();
        std::fs::write(dir.join("data.db-shm"), b"stale-shm").unwrap();
        // The returned flag decides whether the backup clock is reset (v0.9.92).
        assert!(apply_pending_import(&dir), "импорт применён, но функция сказала «нет»");
        assert!(!dir.join("data.db.import").exists());
        assert!(!dir.join("data.db-wal").exists());
        assert!(!dir.join("data.db-shm").exists());

        // v0.9.87: the database being replaced is kept aside instead of being
        // destroyed. Until then a bad import left nothing to go back to.
        assert_eq!(
            std::fs::read(dir.join("data.db.pre-import")).unwrap(),
            b"old-db",
            "прежняя база не сохранена рядом как data.db.pre-import"
        );

        let imported = sqlx::SqlitePool::connect(&format!("sqlite:{}", dir.join("data.db").display()))
            .await
            .unwrap();
        let title: String = sqlx::query_scalar("SELECT title FROM tasks")
            .fetch_one(&imported)
            .await
            .unwrap();
        assert_eq!(title, "задача для бэкапа");

        imported.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn import_rejects_non_zip() {
        let dir = tmp_dir("badzip");
        let bad = dir.join("not-a-zip.zip");
        std::fs::write(&bad, b"garbage").unwrap();

        assert!(import_impl(&dir, bad.to_str().unwrap()).await.is_err());
        // no staging file must appear
        assert!(!dir.join("data.db.import").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Builds a zip containing a single "data.db" entry with the given bytes —
    // structurally a valid archive, which is all import_impl used to check.
    fn zip_with_db_bytes(path: &Path, bytes: &[u8]) {
        let mut zip = ZipWriter::new(File::create(path).unwrap());
        zip.start_file("data.db", SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
        zip.finish().unwrap();
    }

    // v0.9.92. The number that prevents the mistake is losing_*: how much of the
    // live database is newer than the snapshot and would be discarded.
    //
    // This is the exact situation observed on the real installation: a note was
    // created, then an OLDER archive was imported and the note silently
    // disappeared — twice within an hour, because the file dialog shows nothing
    // but a timestamp in the name.
    #[tokio::test]
    async fn preview_counts_what_the_import_would_discard() {
        let dir = tmp_dir("preview");
        let pool = test_pool(&dir).await;
        insert_task(&pool, "старая задача").await;

        // A snapshot of that state.
        let zip_path = dir.join("snap.zip");
        export_impl(&pool, &dir, zip_path.to_str().unwrap()).await.unwrap();

        let snapshot = dir.join("snap.db");
        {
            let mut a = ZipArchive::new(File::open(&zip_path).unwrap()).unwrap();
            let mut buf = Vec::new();
            a.by_name("data.db").unwrap().read_to_end(&mut buf).unwrap();
            std::fs::write(&snapshot, &buf).unwrap();
        }

        // Nothing newer yet: importing this loses nothing.
        let p = preview_from_file(&pool, &snapshot).await.unwrap();
        assert_eq!(p.tasks, 1);
        assert_eq!(p.losing_tasks, 0, "терять нечего, а предпросмотр утверждает обратное");
        assert!(!p.newest.is_empty(), "дата снимка не определена");
        // v0.9.92: both sides, or "39 tasks" has nothing to be compared against.
        assert_eq!(p.current_tasks, 1, "не отдано текущее число задач — сравнивать не с чем");
        assert_eq!(p.current_notes, 0);

        // Work done after the snapshot — precisely what the user would lose.
        sqlx::query(
            "INSERT INTO notes (id, title, content, tags, created_at, updated_at)
             VALUES ('n1', 'заметка после снимка', '', '[]', '2099-01-01T00:00:00+00:00', '2099-01-01T00:00:00+00:00')")
            .execute(&pool).await.unwrap();
        insert_task_at(&pool, "задача после снимка", "2099-01-01T00:00:00+00:00").await;

        let p = preview_from_file(&pool, &snapshot).await.unwrap();
        assert_eq!(p.tasks, 1, "содержимое архива не должно меняться от правок живой базы");
        assert_eq!(p.losing_notes, 1, "не посчитана заметка, которую импорт уничтожит");
        assert_eq!(p.losing_tasks, 1, "не посчитана задача, которую импорт уничтожит");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // `newest` (shown) and the loss threshold (counted) answer different
    // questions and must not be the same value.
    //
    // Found on the real installation: the dialog said "Copy from 1 Aug" for a
    // 6 Aug snapshot, because created_at dates the newest row the copy *created*,
    // while the notes in it had been edited two days later. So the caption moved
    // to updated_at — and the threshold deliberately did not: counting "newer
    // than the copy" by updated_at would flag an edit to a note the archive
    // already contains, and a warning that cries wolf stops being read.
    #[tokio::test]
    async fn editing_an_old_note_is_not_counted_as_a_loss() {
        let dir = tmp_dir("preview-edit");
        let pool = test_pool(&dir).await;
        sqlx::query(
            "INSERT INTO notes (id, title, content, tags, created_at, updated_at)
             VALUES ('n1', 'старая', '', '[]', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')")
            .execute(&pool).await.unwrap();

        let zip_path = dir.join("snap.zip");
        export_impl(&pool, &dir, zip_path.to_str().unwrap()).await.unwrap();
        let snapshot = dir.join("snap.db");
        {
            let mut a = ZipArchive::new(File::open(&zip_path).unwrap()).unwrap();
            let mut buf = Vec::new();
            a.by_name("data.db").unwrap().read_to_end(&mut buf).unwrap();
            std::fs::write(&snapshot, &buf).unwrap();
        }

        // The note is edited after the snapshot: nothing new was created, so
        // nothing would be lost that the archive does not already hold.
        sqlx::query("UPDATE notes SET content = 'правка', updated_at = '2099-01-01T00:00:00+00:00' WHERE id = 'n1'")
            .execute(&pool).await.unwrap();

        let p = preview_from_file(&pool, &snapshot).await.unwrap();
        assert_eq!(
            p.losing_notes, 0,
            "правка существующей заметки засчитана как потеря — предупреждение станет ложным"
        );
        // And the caption dates the copy by its last edit, not by its last
        // creation: both rows here were written at the same instant.
        assert_eq!(p.newest, "2026-01-01T00:00:00+00:00");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // v0.9.92. Reported from the real installation: every import was followed by
    // an extra backup a minute later. The imported database carries the
    // last_auto_backup of the copy, so auto_backup_due sees a run that is more
    // than 24h overdue. That copy is not just noise — it eats a rotation slot,
    // and with keep = 7 a few imports can push out the very archive being
    // restored (which is what happened to the 19:08 one during testing).
    #[tokio::test]
    async fn an_imported_db_does_not_trigger_an_immediate_backup() {
        let dir = tmp_dir("import-clock");
        let backup_dir = dir.join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();
        let pool = test_pool(&dir).await;
        set_setting(&pool, "auto_backup_dir", backup_dir.to_str().unwrap()).await.unwrap();

        // Exactly what an imported database looks like: a stale clock.
        let old = (Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        set_setting(&pool, "last_auto_backup", &old).await.unwrap();
        assert!(auto_backup_due(&pool).await, "подготовка теста не сработала");

        note_import_as_fresh_backup(&pool).await;
        assert!(
            !auto_backup_due(&pool).await,
            "сразу после импорта снимается лишняя копия — она вытесняет из ротации тот самый архив, к которому возвращались"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Looking at an archive must not stage anything: until the user confirms,
    // data.db.import has no business existing.
    #[tokio::test]
    async fn preview_does_not_stage_an_import() {
        let dir = tmp_dir("preview-nostage");
        let pool = test_pool(&dir).await;
        insert_task(&pool, "задача").await;
        let zip_path = dir.join("snap.zip");
        export_impl(&pool, &dir, zip_path.to_str().unwrap()).await.unwrap();

        let snapshot = dir.join("snap.db");
        {
            let mut a = ZipArchive::new(File::open(&zip_path).unwrap()).unwrap();
            let mut buf = Vec::new();
            a.by_name("data.db").unwrap().read_to_end(&mut buf).unwrap();
            std::fs::write(&snapshot, &buf).unwrap();
        }
        preview_from_file(&pool, &snapshot).await.unwrap();

        assert!(
            !dir.join("data.db.import").exists(),
            "предпросмотр создал staging — следующий запуск затрёт базу без подтверждения"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // A foreign or corrupt archive must be refused at preview time too, or the
    // dialog would show zeros and look like an empty but valid backup.
    #[tokio::test]
    async fn preview_refuses_a_foreign_database() {
        let dir = tmp_dir("preview-foreign");
        let pool = test_pool(&dir).await;

        let foreign = dir.join("foreign.db");
        let fp = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", foreign.display()))
            .await.unwrap();
        sqlx::query("CREATE TABLE bookmarks (id INTEGER PRIMARY KEY)").execute(&fp).await.unwrap();
        fp.close().await;

        assert!(preview_from_file(&pool, &foreign).await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // v0.9.87. The zip is well-formed and has a data.db entry, so every check
    // that existed before this version passes; the payload is not a database.
    // Before the fix this reached staging, apply_pending_import renamed it over
    // the live DB on the next launch and init_db died on .expect() — real data
    // destroyed, no window, no message.
    #[tokio::test]
    async fn import_rejects_a_zip_whose_payload_is_not_a_database() {
        let dir = tmp_dir("import-garbage");
        let zip_path = dir.join("backup.zip");
        zip_with_db_bytes(&zip_path, b"not a database at all");

        assert!(import_impl(&dir, zip_path.to_str().unwrap()).await.is_err());
        assert!(
            !dir.join("data.db.import").exists(),
            "негодный архив оставил staging-файл: перезапуск затрёт живую базу"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // The realistic mistake: a genuine SQLite file that belongs to some other
    // program. integrity_check says "ok" on it, so only the table check catches
    // this one — which is why both checks exist.
    #[tokio::test]
    async fn import_rejects_a_database_of_another_app() {
        let dir = tmp_dir("import-foreign");

        let foreign = dir.join("foreign.db");
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", foreign.display()))
            .await
            .unwrap();
        sqlx::query("CREATE TABLE bookmarks (id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;

        let zip_path = dir.join("backup.zip");
        zip_with_db_bytes(&zip_path, &std::fs::read(&foreign).unwrap());

        assert!(import_impl(&dir, zip_path.to_str().unwrap()).await.is_err());
        assert!(!dir.join("data.db.import").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // A failed safety net must not be worse than no safety net. If the current DB
    // cannot be moved aside, the import is abandoned whole: the live database
    // stays, the staging file survives and the next launch tries again.
    // Only the *keeping* step must fail here. Making the whole folder read-only
    // was the first attempt and proved nothing: it also blocks the final rename,
    // so the live DB survives on its own and the test passes with the guard
    // removed. Same trap as the v0.9.86 test — verified by trying it.
    //
    // A non-empty directory in place of data.db.pre-import fails exactly one
    // step: remove_file cannot delete a directory and rename cannot replace one,
    // while every other rename in the function still works.
    #[test]
    fn import_is_abandoned_when_the_old_db_cannot_be_kept() {
        let dir = tmp_dir("import-nosafety");
        std::fs::write(dir.join("data.db"), b"live-data").unwrap();
        std::fs::write(dir.join("data.db.import"), b"incoming").unwrap();

        let blocker = dir.join("data.db.pre-import");
        std::fs::create_dir(&blocker).unwrap();
        std::fs::write(blocker.join("occupied"), b"x").unwrap();

        apply_pending_import(&dir);

        assert_eq!(
            std::fs::read(dir.join("data.db")).unwrap(),
            b"live-data",
            "живая база затёрта, хотя сохранить её не удалось"
        );
        assert!(
            dir.join("data.db.import").exists(),
            "staging удалён — импорт потерян вместо повтора при следующем запуске"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_pending_import_is_noop_without_staging() {
        let dir = tmp_dir("noop");
        std::fs::write(dir.join("data.db"), b"current").unwrap();
        assert!(!apply_pending_import(&dir), "импорта не было, а функция сказала «да»");
        assert_eq!(std::fs::read(dir.join("data.db")).unwrap(), b"current");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn auto_backup_due_returns_false_when_dir_empty() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./src/db/migrations").run(&pool).await.unwrap();
        set_setting(&pool, "auto_backup_dir", "").await.unwrap();
        assert!(!auto_backup_due(&pool).await);
    }

    #[tokio::test]
    async fn auto_backup_due_returns_true_when_no_last_backup() {
        let dir = tmp_dir("due_no_last");
        let backup_dir = dir.join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./src/db/migrations").run(&pool).await.unwrap();
        set_setting(&pool, "auto_backup_dir", backup_dir.to_str().unwrap()).await.unwrap();
        assert!(auto_backup_due(&pool).await);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn auto_backup_due_respects_24h_interval() {
        let dir = tmp_dir("due_24h");
        let backup_dir = dir.join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./src/db/migrations").run(&pool).await.unwrap();
        set_setting(&pool, "auto_backup_dir", backup_dir.to_str().unwrap()).await.unwrap();

        // a recent backup — must not trigger
        let recent = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        set_setting(&pool, "last_auto_backup", &recent).await.unwrap();
        assert!(!auto_backup_due(&pool).await);

        // 25 hours ago — must trigger
        let old = (Utc::now() - chrono::Duration::hours(25)).to_rfc3339();
        set_setting(&pool, "last_auto_backup", &old).await.unwrap();
        assert!(auto_backup_due(&pool).await);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn auto_backup_rotation_keeps_only_keep_files() {
        let dir = tmp_dir("rotation");
        let backup_dir = dir.join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        // Unrelated files are left alone
        std::fs::write(backup_dir.join("note.txt"), b"hello").unwrap();

        // Create "old" backups via export_impl directly, not auto_backup_impl
        let pool = test_pool(&dir).await;
        set_setting(&pool, "auto_backup_dir", backup_dir.to_str().unwrap()).await.unwrap();
        set_setting(&pool, "auto_backup_keep", "3").await.unwrap();

        // Simulate 4 old backups
        for i in 1..=4 {
            let name = format!("kriptag-backup-2026-07-{:02}0-1200.zip", i);
            std::fs::write(backup_dir.join(&name), b"fake-zip").unwrap();
        }

        // Run the automatic backup: it creates a new one and prunes the old
        auto_backup_impl(&pool, &dir).await.unwrap();

        // 3 backups (keep) + 1 unrelated file = 4 files must remain
        let mut entries: Vec<_> = std::fs::read_dir(&backup_dir).unwrap()
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());

        let backup_count = entries.iter().filter(|e| {
            e.file_name().to_string_lossy().starts_with("kriptag-backup-")
        }).count();
        assert_eq!(backup_count, 3, "должно быть 3 бэкапа после ротации");

        // The unrelated file is untouched
        assert!(backup_dir.join("note.txt").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // v0.9.86. Until then the rotation step did
    // `read_dir(&dir).unwrap_or_else(|_| panic!(...))`, and the caller is a
    // tokio::spawn loop: the panic killed that task alone and automatic backups
    // stopped for good without a word anywhere.
    //
    // The failure has to land on read_dir specifically. A file where a directory
    // is expected — the obvious choice — is useless: export_impl fails first when
    // it cannot create the zip, so the test passes with the panic still in place
    // and proves nothing. Verified by trying exactly that.
    //
    // Write+execute without read is the one mode where the export succeeds and
    // the rotation cannot list the folder. Unix-only, hence the cfg.
    //
    // The assertion is literally that the call returns instead of panicking: a
    // panic would take the whole test binary down, which is the production
    // behaviour being fixed.
    #[cfg(unix)]
    #[tokio::test]
    async fn auto_backup_records_the_error_instead_of_panicking() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("backup-error");
        let pool = test_pool(&dir).await;

        let unreadable = dir.join("unreadable");
        std::fs::create_dir_all(&unreadable).unwrap();
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o300)).unwrap();
        set_setting(&pool, "auto_backup_dir", unreadable.to_str().unwrap()).await.unwrap();

        let result = run_auto_backup(&pool, &dir).await;
        assert!(result.is_err(), "сбойный бэкап вернул Ok");
        // Proof the failure is the rotation step and not something earlier: with
        // the folder made readable again, the exported zip is there. Without this
        // the test would pass with the panic still in place — exactly the trap
        // hit on the first attempt at this test.
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let written = std::fs::read_dir(&unreadable).unwrap().count();
        assert_eq!(
            written, 1,
            "экспорт не дошёл до ротации — тест бьёт не в ту строку"
        );

        let recorded = get_setting(&pool, "last_auto_backup_error").await.unwrap_or_default();
        assert!(!recorded.is_empty(), "ошибка бэкапа нигде не записана");
        assert!(
            recorded.contains('\t'),
            "ожидался формат <rfc3339>\\t<сообщение>, получено: {recorded}"
        );

        // The other half: one bad run must not leave a warning in Settings for
        // good, so a later success clears it.
        let good_dir = dir.join("backups");
        std::fs::create_dir_all(&good_dir).unwrap();
        set_setting(&pool, "auto_backup_dir", good_dir.to_str().unwrap()).await.unwrap();

        run_auto_backup(&pool, &dir).await.unwrap();
        assert_eq!(
            get_setting(&pool, "last_auto_backup_error").await.unwrap_or_default(),
            "",
            "успешный бэкап не сбросил прошлую ошибку"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // v0.9.85: automatic backups are on out of the box. Until then auto_backup_dir
    // defaulted to empty and auto_backup_due refused to run — the live database of
    // the app's only daily user had never been backed up once.
    #[tokio::test]
    async fn default_backup_dir_is_set_on_a_fresh_db() {
        let dir = tmp_dir("default-backup");
        let pool = test_pool(&dir).await;

        assert_eq!(get_setting(&pool, "auto_backup_dir").await, None);
        ensure_default_backup_dir(&pool, &dir).await.unwrap();

        let set = get_setting(&pool, "auto_backup_dir").await.unwrap();
        assert_eq!(set, dir.join("backups").to_string_lossy());
        assert!(dir.join("backups").is_dir(), "каталог бэкапов не создан");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // The other half, and the reason for the marker key: clearing the folder in
    // Settings is a deliberate "turn it off". Re-seeding it on the next launch
    // would make the setting impossible to switch off.
    #[tokio::test]
    async fn a_deliberately_cleared_folder_is_not_restored() {
        let dir = tmp_dir("cleared-backup");
        let pool = test_pool(&dir).await;

        ensure_default_backup_dir(&pool, &dir).await.unwrap();
        set_setting(&pool, "auto_backup_dir", "").await.unwrap();

        ensure_default_backup_dir(&pool, &dir).await.unwrap();
        assert_eq!(
            get_setting(&pool, "auto_backup_dir").await.unwrap(),
            "",
            "очищенная пользователем папка вернулась при следующем запуске"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}


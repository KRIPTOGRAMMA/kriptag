use tauri::State;
use sqlx::{SqlitePool, Row};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::error::{AppError, AppResult};
use crate::stem::stem_text;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub linked_task_id: Option<String>,
    pub project_id: Option<String>,
    pub pinned: bool,
    pub created_at: String,
    pub updated_at: String,
    pub reminder_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateNote {
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub linked_task_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateNote {
    pub title: Option<String>,
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
    // Some(Some(id)) links, Some(None) unlinks, None leaves it alone.
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub linked_task_id: Option<Option<String>>,
    // Likewise: Some(Some(id)) into a project, Some(None) out of it, None leaves it alone.
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub project_id: Option<Option<String>>,
    pub pinned: Option<bool>,
    // Some(Some(iso)) sets a reminder, Some(None) clears it, None leaves it alone.
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub reminder_at: Option<Option<String>>,
}

// We distinguish "field absent" from "field = null" in JSON so that unlinking
// (linked_task_id: null) can be told apart from "leave alone" (no field sent).
fn deserialize_optional_field<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<String>::deserialize(deserializer)?))
}

fn row_to_note(row: sqlx::sqlite::SqliteRow) -> Note {
    let tags_json: String = row.get("tags");
    let pinned: i64 = row.get("pinned");
    Note {
        id: row.get("id"),
        title: row.get("title"),
        content: row.get("content"),
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        linked_task_id: row.get("linked_task_id"),
        project_id: row.get("project_id"),
        pinned: pinned != 0,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        reminder_at: row.get("reminder_at"),
    }
}

const NOTE_COLUMNS: &str = "id, title, content, tags, linked_task_id, project_id, pinned, created_at, updated_at, reminder_at";

#[tauri::command]
pub async fn get_notes(pool: State<'_, SqlitePool>) -> AppResult<Vec<Note>> {
    get_notes_impl(pool.inner()).await
}

pub async fn get_notes_impl(pool: &SqlitePool) -> AppResult<Vec<Note>> {
    let rows = sqlx::query(&format!("SELECT {NOTE_COLUMNS} FROM notes WHERE deleted_at IS NULL ORDER BY updated_at DESC"))
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter().map(row_to_note).collect())
}

#[tauri::command]
pub async fn create_note(pool: State<'_, SqlitePool>, note: CreateNote) -> AppResult<Note> {
    create_note_impl(pool.inner(), note).await
}

pub async fn create_note_impl(pool: &SqlitePool, note: CreateNote) -> AppResult<Note> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    let title = if note.title.trim().is_empty() { "Без названия".to_string() } else { note.title };
    let tags_json = serde_json::to_string(&note.tags).unwrap_or_else(|_| "[]".into());

    sqlx::query(
        "INSERT INTO notes (id, title, content, tags, linked_task_id, project_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&id)
    .bind(&title)
    .bind(&note.content)
    .bind(&tags_json)
    .bind(&note.linked_task_id)
    .bind(&note.project_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;

    Ok(Note {
        id,
        title,
        content: note.content,
        tags: note.tags,
        linked_task_id: note.linked_task_id,
        project_id: note.project_id,
        pinned: false,
        created_at: now.clone(),
        updated_at: now,
        reminder_at: None,
    })
}

#[tauri::command]
pub async fn update_note(
    pool: State<'_, SqlitePool>,
    id: String,
    patch: UpdateNote,
) -> AppResult<Note> {
    update_note_impl(pool.inner(), id, patch).await
}

pub async fn update_note_impl(pool: &SqlitePool, id: String, patch: UpdateNote) -> AppResult<Note> {
    let now = Utc::now().to_rfc3339();

    // Checked BEFORE any write, not after. A debounced autosave (800ms) can arrive
    // once the note is already in the Trash; before the Trash existed the row was
    // gone and every UPDATE was a harmless no-op. Now the row survives, so writing
    // first and noticing afterwards would overwrite the text sitting in the Trash —
    // and a restore would hand back content the user never meant to keep.
    let alive: Option<i64> = sqlx::query_scalar("SELECT 1 FROM notes WHERE id = ? AND deleted_at IS NULL")
        .bind(&id)
        .fetch_optional(pool)
        .await?;
    if alive.is_none() {
        return Err(AppError::Other("__NOTE_DELETED__".into()));
    }

    if let Some(ref title) = patch.title {
        sqlx::query("UPDATE notes SET title = ?, updated_at = ? WHERE id = ?")
            .bind(title).bind(&now).bind(&id)
            .execute(pool).await?;
    }
    if let Some(ref content) = patch.content {
        snapshot_revision_if_due(pool, &id, &now).await?;
        sqlx::query("UPDATE notes SET content = ?, updated_at = ? WHERE id = ?")
            .bind(content).bind(&now).bind(&id)
            .execute(pool).await?;
    }
    if let Some(ref tags) = patch.tags {
        let tags_json = serde_json::to_string(tags).unwrap_or_else(|_| "[]".into());
        sqlx::query("UPDATE notes SET tags = ?, updated_at = ? WHERE id = ?")
            .bind(&tags_json).bind(&now).bind(&id)
            .execute(pool).await?;
    }
    if let Some(ref linked) = patch.linked_task_id {
        // linked: Some(id) links, None unlinks (linked is itself an Option<String>)
        sqlx::query("UPDATE notes SET linked_task_id = ?, updated_at = ? WHERE id = ?")
            .bind(linked).bind(&now).bind(&id)
            .execute(pool).await?;
    }
    if let Some(ref project) = patch.project_id {
        sqlx::query("UPDATE notes SET project_id = ?, updated_at = ? WHERE id = ?")
            .bind(project).bind(&now).bind(&id)
            .execute(pool).await?;
    }
    if let Some(pinned) = patch.pinned {
        sqlx::query("UPDATE notes SET pinned = ?, updated_at = ? WHERE id = ?")
            .bind(pinned).bind(&now).bind(&id)
            .execute(pool).await?;
    }
    if let Some(ref reminder) = patch.reminder_at {
        // The reminder actually changed (including being cleared), so reset
        // notified_reminder — the same pattern as notified_block/notified_24h on
        // tasks (tasks.rs). Otherwise a new or moved reminder never fires.
        sqlx::query("UPDATE notes SET reminder_at = ?, notified_reminder = 0, updated_at = ? WHERE id = ?")
            .bind(reminder).bind(&now).bind(&id)
            .execute(pool).await?;
    }

    // fetch_optional rather than fetch_one: the note may have been deleted in
    // parallel (autosave is debounced by 800ms, so the user can press "Delete"
    // before the pending save timer fires). This is not a save failure — the
    // note is gone, the update became a no-op, and there is nothing to roll back.
    //
    // `deleted_at IS NULL` is load-bearing here since the Trash arrived: the row
    // now survives deletion, so without it the late save would find the note,
    // report success and quietly write new content into a trashed note.
    let row = sqlx::query(&format!("SELECT {NOTE_COLUMNS} FROM notes WHERE id = ? AND deleted_at IS NULL"))
        .bind(&id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::Other("__NOTE_DELETED__".into()))?;

    Ok(row_to_note(row))
}

const REVISION_INTERVAL_MINS: i64 = 10;
const REVISION_KEEP: i64 = 20;

// A snapshot taken BEFORE the new content is written, but only if this note's
// latest revision is older than REVISION_INTERVAL_MINS (or there are no
// revisions at all — the first edit is captured too, so the original text can be
// restored). No snapshot is taken when the content has not changed in the DB, so
// frequent autosaves of identical text do not breed revisions; we compare
// against the note's current content.
async fn snapshot_revision_if_due(pool: &SqlitePool, note_id: &str, now: &str) -> AppResult<()> {
    let current: Option<String> = sqlx::query_scalar("SELECT content FROM notes WHERE id = ?")
        .bind(note_id)
        .fetch_optional(pool)
        .await?;
    let Some(current) = current else { return Ok(()) };

    let last_at: Option<String> = sqlx::query_scalar(
        "SELECT created_at FROM note_revisions WHERE note_id = ? ORDER BY created_at DESC LIMIT 1"
    )
    .bind(note_id)
    .fetch_optional(pool)
    .await?;

    let due = match &last_at {
        None => true,
        Some(last) => {
            let now_dt = chrono::DateTime::parse_from_rfc3339(now).map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now());
            let last_dt = chrono::DateTime::parse_from_rfc3339(last).map(|d| d.with_timezone(&Utc));
            match last_dt {
                Ok(last_dt) => (now_dt - last_dt).num_minutes() >= REVISION_INTERVAL_MINS,
                Err(_) => true,
            }
        }
    };
    if !due {
        return Ok(());
    }

    sqlx::query("INSERT INTO note_revisions (id, note_id, content, created_at) VALUES (?, ?, ?, ?)")
        .bind(Uuid::new_v4().to_string())
        .bind(note_id)
        .bind(&current)
        .bind(now)
        .execute(pool)
        .await?;

    rotate_revisions(pool, note_id).await
}

// Keep at most REVISION_KEEP revisions per note, deleting the oldest extras.
async fn rotate_revisions(pool: &SqlitePool, note_id: &str) -> AppResult<()> {
    sqlx::query(
        "DELETE FROM note_revisions WHERE note_id = ? AND id NOT IN (
            SELECT id FROM note_revisions WHERE note_id = ? ORDER BY created_at DESC LIMIT ?
        )"
    )
    .bind(note_id)
    .bind(note_id)
    .bind(REVISION_KEEP)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Serialize, Clone)]
pub struct NoteRevision {
    pub id: String,
    pub created_at: String,
    pub size: i64,
}

#[tauri::command]
pub async fn get_note_revisions(pool: State<'_, SqlitePool>, note_id: String) -> AppResult<Vec<NoteRevision>> {
    get_note_revisions_impl(pool.inner(), &note_id).await
}

pub async fn get_note_revisions_impl(pool: &SqlitePool, note_id: &str) -> AppResult<Vec<NoteRevision>> {
    let rows = sqlx::query(
        "SELECT id, created_at, length(content) as size FROM note_revisions WHERE note_id = ? ORDER BY created_at DESC"
    )
    .bind(note_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().map(|r| NoteRevision {
        id: r.get("id"),
        created_at: r.get("created_at"),
        size: r.get("size"),
    }).collect())
}

#[tauri::command]
pub async fn get_note_revision_content(pool: State<'_, SqlitePool>, revision_id: String) -> AppResult<String> {
    get_note_revision_content_impl(pool.inner(), &revision_id).await
}

pub async fn get_note_revision_content_impl(pool: &SqlitePool, revision_id: &str) -> AppResult<String> {
    sqlx::query_scalar("SELECT content FROM note_revisions WHERE id = ?")
        .bind(revision_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| crate::error::AppError::Other("Ревизия не найдена".into()))
}

// Rolling back to a revision: the note's current content is saved as a revision
// too (otherwise an edit unsaved at the moment of rollback vanishes without a
// trace), and then the note's content is replaced with the chosen revision.
#[tauri::command]
pub async fn restore_note_revision(pool: State<'_, SqlitePool>, revision_id: String) -> AppResult<Note> {
    restore_note_revision_impl(pool.inner(), &revision_id).await
}

pub async fn restore_note_revision_impl(pool: &SqlitePool, revision_id: &str) -> AppResult<Note> {
    let row = sqlx::query("SELECT note_id, content FROM note_revisions WHERE id = ?")
        .bind(revision_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| crate::error::AppError::Other("Ревизия не найдена".into()))?;
    let note_id: String = row.get("note_id");
    let revision_content: String = row.get("content");

    let now = Utc::now().to_rfc3339();
    // The current content goes into a revision as well, ignoring the 10-minute
    // interval: a rollback is a deliberate user action, not an autosave.
    let current: Option<String> = sqlx::query_scalar("SELECT content FROM notes WHERE id = ?")
        .bind(&note_id)
        .fetch_optional(pool)
        .await?;
    if let Some(current) = current {
        sqlx::query("INSERT INTO note_revisions (id, note_id, content, created_at) VALUES (?, ?, ?, ?)")
            .bind(Uuid::new_v4().to_string())
            .bind(&note_id)
            .bind(&current)
            .bind(&now)
            .execute(pool)
            .await?;
        rotate_revisions(pool, &note_id).await?;
    }

    sqlx::query("UPDATE notes SET content = ?, updated_at = ? WHERE id = ?")
        .bind(&revision_content)
        .bind(&now)
        .bind(&note_id)
        .execute(pool)
        .await?;

    let row = sqlx::query(&format!("SELECT {NOTE_COLUMNS} FROM notes WHERE id = ?"))
        .bind(&note_id)
        .fetch_one(pool)
        .await?;

    Ok(row_to_note(row))
}

// A wiki link to a renamed note: [[old]] or [[old|alias]] becomes [[new]] or
// [[new|alias]] — only the target changes, any alias stays as it was.
// Case-insensitive; the title inside [[...]] may contain any characters except
// '[', ']' and '|' (see WIKILINK_RE in src/lib/markdown.ts — we mirror that
// format).
fn rewrite_links(content: &str, old_title: &str, new_title: &str) -> (String, bool) {
    let old_lower = old_title.to_lowercase();
    let mut out = String::with_capacity(content.len());
    let mut changed = false;
    let mut rest = content;

    while let Some(start) = rest.find("[[") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else {
            // An unclosed link runs to the end of the line: copy the rest as is
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let inner = &after[..end];
        let (target, alias) = match inner.find('|') {
            Some(p) => (&inner[..p], Some(&inner[p + 1..])),
            None => (inner, None),
        };

        if target.trim().to_lowercase() == old_lower {
            changed = true;
            out.push_str("[[");
            out.push_str(new_title);
            if let Some(a) = alias {
                out.push('|');
                out.push_str(a);
            }
            out.push_str("]]");
        } else {
            out.push_str("[[");
            out.push_str(inner);
            out.push_str("]]");
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    (out, changed)
}

#[tauri::command]
pub async fn rename_note_links(
    pool: State<'_, SqlitePool>,
    old_title: String,
    new_title: String,
) -> AppResult<i64> {
    rename_note_links_impl(pool.inner(), old_title, new_title).await
}

// Rewrites [[old_title]] and [[old_title|alias]] across all notes to new_title.
// Returns how many notes were updated. An empty or unchanged old_title is a
// no-op (renaming "Untitled" to "Untitled" must not rewrite anything).
pub async fn rename_note_links_impl(pool: &SqlitePool, old_title: String, new_title: String) -> AppResult<i64> {
    let old_title = old_title.trim();
    let new_title = new_title.trim();
    // eq_ignore_ascii_case does not cover Cyrillic or other non-ASCII text, so we
    // compare via to_lowercase (Unicode case folding).
    if old_title.is_empty() || old_title.to_lowercase() == new_title.to_lowercase() {
        return Ok(0);
    }

    let rows = sqlx::query("SELECT id, content FROM notes WHERE deleted_at IS NULL")
        .fetch_all(pool)
        .await?;

    let mut updated = 0i64;
    let now = Utc::now().to_rfc3339();
    for row in rows {
        let id: String = row.get("id");
        let content: String = row.get("content");
        let (new_content, changed) = rewrite_links(&content, old_title, new_title);
        if changed {
            sqlx::query("UPDATE notes SET content = ?, updated_at = ? WHERE id = ?")
                .bind(&new_content)
                .bind(&now)
                .bind(&id)
                .execute(pool)
                .await?;
            updated += 1;
        }
    }
    Ok(updated)
}

/// Brings notes_stem_fts up to date with whatever the triggers marked dirty.
///
/// Called at the start of every search rather than after every write. Stemming
/// on write would pay the cost on the wiki-link rename, which rewrites many
/// notes in one go; doing it here means the work happens once, when the result
/// is actually about to be read, and a burst of edits collapses into a single
/// pass.
///
/// The trigger is what makes this safe. The note text is written from five
/// different places in this file, and a sixth added later would silently drop
/// out of the stemmed index if each write path had to remember to sync. Marking
/// is done by the database, so no write path can slip past it.
pub async fn reindex_stemmed_notes(pool: &SqlitePool) -> AppResult<u64> {
    let dirty = sqlx::query(
        "SELECT n.rowid AS rid, n.title, n.content, n.tags
         FROM notes_stem_dirty d
         INNER JOIN notes n ON n.rowid = d.rowid_ref"
    )
    .fetch_all(pool)
    .await?;

    let mut done = 0u64;
    for row in &dirty {
        let rid: i64 = row.get("rid");
        let title: String = row.get("title");
        let content: String = row.get("content");
        let tags: String = row.get("tags");

        // Delete first: FTS5 has no upsert, and re-inserting the same rowid
        // without this leaves the previous version in the index, so an edited
        // note would keep matching the words it no longer contains.
        sqlx::query("DELETE FROM notes_stem_fts WHERE rowid = ?")
            .bind(rid)
            .execute(pool)
            .await?;

        sqlx::query("INSERT INTO notes_stem_fts(rowid, title, content, tags) VALUES (?, ?, ?, ?)")
            .bind(rid)
            .bind(stem_text(&title))
            .bind(stem_text(&content))
            .bind(stem_text(&tags))
            .execute(pool)
            .await?;

        done += 1;
    }

    // Clearing after the loop, not inside it: if stemming fails halfway the rows
    // stay dirty and the next search retries them. Losing the mark would leave
    // the note permanently unindexed with nothing to show for it.
    if done > 0 {
        sqlx::query("DELETE FROM notes_stem_dirty").execute(pool).await?;
    }

    Ok(done)
}

#[tauri::command]
pub async fn search_notes(pool: State<'_, SqlitePool>, query: String) -> AppResult<Vec<Note>> {
    search_notes_impl(pool.inner(), query).await
}

/// Wraps raw user input as an FTS5 phrase prefix.
///
/// As in search_tasks: what the user types is not FTS5 syntax, so it is quoted
/// as a phrase and any quotes inside are doubled.
fn fts_phrase(text: &str) -> String {
    format!("\"{}\"*", text.replace('"', "\"\""))
}

pub async fn search_notes_impl(pool: &SqlitePool, query: String) -> AppResult<Vec<Note>> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(vec![]);
    }

    reindex_stemmed_notes(pool).await?;

    let rows = sqlx::query(
        "SELECT n.id, n.title, n.content, n.tags, n.linked_task_id, n.project_id, n.pinned, n.created_at, n.updated_at, n.reminder_at
         FROM notes n
         INNER JOIN notes_fts ON notes_fts.rowid = n.rowid
         WHERE notes_fts MATCH ?
           AND n.deleted_at IS NULL
         ORDER BY rank"
    )
    .bind(fts_phrase(trimmed))
    .fetch_all(pool)
    .await?;

    let mut notes: Vec<Note> = rows.into_iter().map(row_to_note).collect();

    // The stemmed half comes second and appends only what the exact half missed,
    // so an exact hit is never pushed below an approximate one. Both are one
    // list to the user — there is no "similar" section to reason about, search
    // simply finds more than it did.
    let seen: std::collections::HashSet<String> = notes.iter().map(|n| n.id.clone()).collect();

    let stem_rows = sqlx::query(
        "SELECT n.id, n.title, n.content, n.tags, n.linked_task_id, n.project_id, n.pinned, n.created_at, n.updated_at, n.reminder_at
         FROM notes n
         INNER JOIN notes_stem_fts ON notes_stem_fts.rowid = n.rowid
         WHERE notes_stem_fts MATCH ?
           AND n.deleted_at IS NULL
         ORDER BY rank"
    )
    .bind(fts_phrase(&stem_text(trimmed)))
    .fetch_all(pool)
    .await?;

    for row in stem_rows {
        let note = row_to_note(row);
        if !seen.contains(&note.id) {
            notes.push(note);
        }
    }

    Ok(notes)
}

#[derive(Debug, Serialize, Clone)]
pub struct NoteSnippet {
    pub item: Note,
    pub snippet: String,
}

#[tauri::command]
pub async fn search_notes_snippet(pool: State<'_, SqlitePool>, query: String) -> AppResult<Vec<NoteSnippet>> {
    search_notes_snippet_impl(pool.inner(), query).await
}

pub async fn search_notes_snippet_impl(pool: &SqlitePool, query: String) -> AppResult<Vec<NoteSnippet>> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(vec![]);
    }

    reindex_stemmed_notes(pool).await?;

    let rows = sqlx::query(
        "SELECT n.id, n.title, n.content, n.tags, n.linked_task_id, n.project_id, n.pinned, n.created_at, n.updated_at, n.reminder_at,
                snippet(notes_fts, 1, '<mark>', '</mark>', '…', 32) AS snippet
         FROM notes n
         INNER JOIN notes_fts ON notes_fts.rowid = n.rowid
         WHERE notes_fts MATCH ?
           AND n.deleted_at IS NULL
         ORDER BY rank"
    )
    .bind(fts_phrase(trimmed))
    .fetch_all(pool)
    .await?;

    let mut out: Vec<NoteSnippet> = rows.into_iter().map(|r| {
        let snippet: Option<String> = r.get("snippet");
        NoteSnippet { item: row_to_note(r), snippet: snippet.unwrap_or_default() }
    }).collect();

    let seen: std::collections::HashSet<String> = out.iter().map(|s| s.item.id.clone()).collect();

    // No snippet() on notes_stem_fts: it stores stems, so it would highlight
    // "покупк" instead of the word the note actually contains. The stemmed half
    // only answers "does this note match" — the excerpt is built from the note's
    // own text below.
    let stem_rows = sqlx::query(
        "SELECT n.id, n.title, n.content, n.tags, n.linked_task_id, n.project_id, n.pinned, n.created_at, n.updated_at, n.reminder_at
         FROM notes n
         INNER JOIN notes_stem_fts ON notes_stem_fts.rowid = n.rowid
         WHERE notes_stem_fts MATCH ?
           AND n.deleted_at IS NULL
         ORDER BY rank"
    )
    .bind(fts_phrase(&stem_text(trimmed)))
    .fetch_all(pool)
    .await?;

    for row in stem_rows {
        let note = row_to_note(row);
        if seen.contains(&note.id) {
            continue;
        }
        let snippet = lead_excerpt(&note.content);
        out.push(NoteSnippet { item: note, snippet });
    }

    Ok(out)
}

/// The opening of a note, for results that matched only on a stem.
///
/// Deliberately unmarked: the matching word is in some other form than the one
/// typed, so there is nothing in the text to put <mark> around without
/// re-deriving the position from the stems. Showing the start of the note is
/// honest and tells the user what it is about; a wrongly placed highlight would
/// not.
fn lead_excerpt(content: &str) -> String {
    const LIMIT: usize = 120;
    let flat = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= LIMIT {
        return flat;
    }
    let cut: String = flat.chars().take(LIMIT).collect();
    format!("{cut}…")
}

#[tauri::command]
pub async fn delete_note(pool: State<'_, SqlitePool>, id: String) -> AppResult<()> {
    delete_note_impl(pool.inner(), id).await
}

// Soft delete ("Trash"), mirroring delete_task_impl. The row stays in place with
// its revisions intact — it simply stops being returned by get_notes_impl and by
// search. Real deletion goes through purge_deleted_note.
//
// Revisions are deliberately NOT dropped here: they are the note's history, and
// restoring a note without it would be a silent loss.
pub async fn delete_note_impl(pool: &SqlitePool, id: String) -> AppResult<()> {
    sqlx::query("UPDATE notes SET deleted_at = ? WHERE id = ?")
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(&id)
        .execute(pool)
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn get_deleted_notes(pool: State<'_, SqlitePool>) -> AppResult<Vec<Note>> {
    get_deleted_notes_impl(pool.inner()).await
}

pub async fn get_deleted_notes_impl(pool: &SqlitePool) -> AppResult<Vec<Note>> {
    let rows = sqlx::query(&format!(
        "SELECT {NOTE_COLUMNS} FROM notes WHERE deleted_at IS NOT NULL ORDER BY deleted_at DESC"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_note).collect())
}

#[tauri::command]
pub async fn restore_note(pool: State<'_, SqlitePool>, id: String) -> AppResult<()> {
    restore_note_impl(pool.inner(), id).await
}

pub async fn restore_note_impl(pool: &SqlitePool, id: String) -> AppResult<()> {
    sqlx::query("UPDATE notes SET deleted_at = NULL WHERE id = ?")
        .bind(&id)
        .execute(pool)
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn purge_deleted_note(pool: State<'_, SqlitePool>, id: String) -> AppResult<()> {
    purge_deleted_note_impl(pool.inner(), id).await
}

// Real deletion, now the only place that removes the row and its revisions.
pub async fn purge_deleted_note_impl(pool: &SqlitePool, id: String) -> AppResult<()> {
    sqlx::query("DELETE FROM note_revisions WHERE note_id = ?")
        .bind(&id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM notes WHERE id = ?")
        .bind(&id)
        .execute(pool)
        .await?;
    Ok(())
}

// Characters that are invalid or troublesome in filenames on common filesystems
// (Windows included, since an export may be copied there) are replaced with "_".
fn sanitize_filename(title: &str) -> String {
    let cleaned: String = title
        .trim()
        .chars()
        .map(|c| if r#"/\:*?"<>|"#.contains(c) || c.is_control() { '_' } else { c })
        .collect();
    if cleaned.is_empty() { "Без названия".to_string() } else { cleaned }
}

#[tauri::command]
pub async fn export_notes_md(pool: State<'_, SqlitePool>, dir: String) -> AppResult<usize> {
    export_notes_md_impl(pool.inner(), std::path::Path::new(&dir)).await
}

// Each note becomes <sanitized name>.md with its content as is (wiki links are
// already Obsidian-compatible). Name collisions after sanitizing — including
// case differences between notes with the same title — get a "-2", "-3", ...
// suffix in order.
pub async fn export_notes_md_impl(pool: &SqlitePool, dir: &std::path::Path) -> AppResult<usize> {
    let notes = get_notes_impl(pool).await?;
    std::fs::create_dir_all(dir)?;

    let mut used: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut count = 0usize;
    for note in &notes {
        let base = sanitize_filename(&note.title);
        let key = base.to_lowercase();
        let n = used.entry(key).or_insert(0);
        *n += 1;
        let filename = if *n == 1 { format!("{base}.md") } else { format!("{base}-{n}.md") };
        std::fs::write(dir.join(&filename), &note.content)?;
        count += 1;
    }
    Ok(count)
}

// Exports a single note into a self-contained HTML file. Rendering markdown to
// HTML and embedding images as data: URIs is the frontend's job (renderMarkdown
// and DOMPurify already live there); this command only writes the finished
// string to disk — like export_notes_md, but without touching the DB.
#[tauri::command]
pub fn export_note_html(path: String, html: String) -> AppResult<()> {
    std::fs::write(&path, html)?;
    Ok(())
}

#[tauri::command]
pub async fn import_notes_md(pool: State<'_, SqlitePool>, dir: String) -> AppResult<usize> {
    import_notes_md_impl(pool.inner(), std::path::Path::new(&dir)).await
}

// Every *.md in a folder (non-recursively) becomes a new note: title = the
// filename without its extension, content = the file as is. A clash with an
// existing title is NOT merged — a separate new note is created. The user can
// sort duplicates out themselves; a silent merge would be more surprising.
pub async fn import_notes_md_impl(pool: &SqlitePool, dir: &std::path::Path) -> AppResult<usize> {
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut count = 0usize;
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Без названия").to_string();
        let content = std::fs::read_to_string(&path)?;
        create_note_impl(pool, CreateNote {
            title,
            content,
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await?;
        count += 1;
    }
    Ok(count)
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
    async fn create_get_update_delete_roundtrip() {
        let pool = test_pool().await;

        let note = create_note_impl(&pool, CreateNote {
            title: "заметка".into(),
            content: "текст".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();

        let all = get_notes_impl(&pool).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].title, "заметка");

        let updated = update_note_impl(&pool, note.id.clone(), UpdateNote {
            title: None,
            content: Some("новый текст".into()),
            tags: None,
            linked_task_id: None,
            project_id: None,
            pinned: None,
            reminder_at: None,
        }).await.unwrap();
        assert_eq!(updated.content, "новый текст");
        assert_eq!(updated.title, "заметка"); // untouched

        delete_note_impl(&pool, note.id).await.unwrap();
        assert!(get_notes_impl(&pool).await.unwrap().is_empty());
    }

    // The Trash, mirroring tasks. Until v0.9.76 a note was deleted outright and
    // there was nothing to restore.
    #[tokio::test]
    async fn delete_is_soft_and_restore_brings_the_note_back() {
        let pool = test_pool().await;
        let note = create_note_impl(&pool, CreateNote {
            title: "удаляемая".into(),
            content: "важный текст".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();

        delete_note_impl(&pool, note.id.clone()).await.unwrap();
        assert!(get_notes_impl(&pool).await.unwrap().is_empty(), "удалённая не в списке");

        let trashed = get_deleted_notes_impl(&pool).await.unwrap();
        assert_eq!(trashed.len(), 1, "удалённая должна быть в Корзине");
        assert_eq!(trashed[0].content, "важный текст", "текст пережил удаление");

        restore_note_impl(&pool, note.id.clone()).await.unwrap();
        let back = get_notes_impl(&pool).await.unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].content, "важный текст");
        assert!(get_deleted_notes_impl(&pool).await.unwrap().is_empty());
    }

    // Revisions are the note's history; dropping them on a soft delete would make
    // a restore a silent loss. Only purge removes them.
    #[tokio::test]
    async fn revisions_survive_the_trash_and_die_with_purge() {
        let pool = test_pool().await;
        let note = create_note_impl(&pool, CreateNote {
            title: "с историей".into(),
            content: "версия 1".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();
        update_note_impl(&pool, note.id.clone(), UpdateNote {
            title: None, content: Some("версия 2".into()), tags: None,
            linked_task_id: None, project_id: None, pinned: None, reminder_at: None,
        }).await.unwrap();

        let before = revision_count(&pool, &note.id).await;
        assert!(before > 0, "ревизия должна была появиться");

        delete_note_impl(&pool, note.id.clone()).await.unwrap();
        assert_eq!(revision_count(&pool, &note.id).await, before, "Корзина не трогает ревизии");

        purge_deleted_note_impl(&pool, note.id.clone()).await.unwrap();
        assert_eq!(revision_count(&pool, &note.id).await, 0, "очистка убирает ревизии");
        assert!(get_deleted_notes_impl(&pool).await.unwrap().is_empty());
    }

    // notes_fts is kept in sync by triggers on INSERT/UPDATE/DELETE, and a soft
    // delete is an UPDATE — so the row stays in the index and the query itself has
    // to exclude it. Without the filter a trashed note is still found by search
    // and by the command palette.
    #[tokio::test]
    async fn trashed_note_is_not_found_by_search() {
        let pool = test_pool().await;
        let note = create_note_impl(&pool, CreateNote {
            title: "уникальноеслово".into(),
            content: "текст".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();

        assert_eq!(search_notes_impl(&pool, "уникальноеслово".into()).await.unwrap().len(), 1);
        delete_note_impl(&pool, note.id).await.unwrap();
        assert!(
            search_notes_impl(&pool, "уникальноеслово".into()).await.unwrap().is_empty(),
            "заметка из Корзины не должна находиться поиском"
        );
    }

    #[tokio::test]
    async fn pinned_defaults_false_and_toggles_via_update() {
        let pool = test_pool().await;

        let note = create_note_impl(&pool, CreateNote {
            title: "закрепи меня".into(),
            content: "текст".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();
        assert!(!note.pinned);

        let pinned = update_note_impl(&pool, note.id.clone(), UpdateNote {
            title: None, content: None, tags: None, linked_task_id: None, project_id: None,
            pinned: Some(true), reminder_at: None,
        }).await.unwrap();
        assert!(pinned.pinned);

        // Survives a re-read from the DB, not just update's in-memory return.
        let all = get_notes_impl(&pool).await.unwrap();
        assert!(all.iter().find(|n| n.id == note.id).unwrap().pinned);

        let unpinned = update_note_impl(&pool, note.id.clone(), UpdateNote {
            title: None, content: None, tags: None, linked_task_id: None, project_id: None,
            pinned: Some(false), reminder_at: None,
        }).await.unwrap();
        assert!(!unpinned.pinned);
    }

    // A reminder can be set, moved or cleared; changing reminder_at resets
    // notified_reminder — the same principle as notified_block/notified_24h on
    // tasks when a deadline or block moves.
    #[tokio::test]
    async fn reminder_set_move_and_clear_resets_notified_flag() {
        let pool = test_pool().await;
        let note = create_note_impl(&pool, CreateNote {
            title: "напомни мне".into(), content: "".into(),
            tags: vec![], linked_task_id: None, project_id: None,
        }).await.unwrap();
        assert_eq!(note.reminder_at, None);

        let with_reminder = update_note_impl(&pool, note.id.clone(), UpdateNote {
            title: None, content: None, tags: None, linked_task_id: None, project_id: None,
            pinned: None, reminder_at: Some(Some("2026-08-01T10:00:00+00:00".into())),
        }).await.unwrap();
        assert_eq!(with_reminder.reminder_at.as_deref(), Some("2026-08-01T10:00:00+00:00"));

        // Simulate "already notified", then move the date — the flag must reset
        sqlx::query("UPDATE notes SET notified_reminder = 1 WHERE id = ?")
            .bind(&note.id).execute(&pool).await.unwrap();
        update_note_impl(&pool, note.id.clone(), UpdateNote {
            title: None, content: None, tags: None, linked_task_id: None, project_id: None,
            pinned: None, reminder_at: Some(Some("2026-08-02T10:00:00+00:00".into())),
        }).await.unwrap();
        let notified: i64 = sqlx::query_scalar("SELECT notified_reminder FROM notes WHERE id = ?")
            .bind(&note.id).fetch_one(&pool).await.unwrap();
        assert_eq!(notified, 0, "перенос напоминания должен сбросить notified_reminder");

        let cleared = update_note_impl(&pool, note.id.clone(), UpdateNote {
            title: None, content: None, tags: None, linked_task_id: None, project_id: None,
            pinned: None, reminder_at: Some(None),
        }).await.unwrap();
        assert_eq!(cleared.reminder_at, None);
    }

    // Autosave racing deletion: a debounced save can reach the backend after the
    // user has already deleted the note. An UPDATE against a missing row is a
    // harmless no-op, but the finalizing SELECT used to fail with RowNotFound and
    // surface as a visible error even though the deletion had genuinely gone
    // through — that is what was fixed here.
    #[tokio::test]
    async fn update_after_delete_is_soft_error_not_panic() {
        let pool = test_pool().await;
        let note = create_note_impl(&pool, CreateNote {
            title: "т".into(), content: "v1".into(),
            tags: vec![], linked_task_id: None, project_id: None,
        }).await.unwrap();

        delete_note_impl(&pool, note.id.clone()).await.unwrap();

        let r = update_note_impl(&pool, note.id.clone(), content_patch("v2")).await;
        assert!(r.is_err());
        // The note stays deleted: the UPDATE race neither revives nor duplicates it.
        assert!(get_notes_impl(&pool).await.unwrap().is_empty());

        // And, since the Trash arrived, the row still exists — so the late save
        // must not have overwritten what is sitting in it. Otherwise restoring
        // would return the note with content the user never meant to save.
        let trashed = get_deleted_notes_impl(&pool).await.unwrap();
        assert_eq!(trashed.len(), 1);
        assert_eq!(trashed[0].content, "v1", "опоздавшее автосохранение затёрло текст в Корзине");
    }

    #[tokio::test]
    async fn fts_search_finds_and_stays_in_sync() {
        let pool = test_pool().await;

        let note = create_note_impl(&pool, CreateNote {
            title: "Рецепт борща".into(),
            content: "свёкла, капуста".into(),
            tags: vec!["еда".into()],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();

        // By title, by content, by tag; prefix matching.
        assert_eq!(search_notes_impl(&pool, "борщ".into()).await.unwrap().len(), 1);
        assert_eq!(search_notes_impl(&pool, "капуст".into()).await.unwrap().len(), 1);
        assert_eq!(search_notes_impl(&pool, "еда".into()).await.unwrap().len(), 1);
        assert!(search_notes_impl(&pool, "плов".into()).await.unwrap().is_empty());

        // FTS5 special characters in the query do not break MATCH.
        assert!(search_notes_impl(&pool, "борщ-2 \"AND (x:y)".into()).await.unwrap().is_empty());
        assert!(search_notes_impl(&pool, "   ".into()).await.unwrap().is_empty());

        // After an UPDATE the index sees the new text and not the old.
        update_note_impl(&pool, note.id.clone(), UpdateNote {
            title: None,
            content: Some("теперь про плов".into()),
            tags: None,
            linked_task_id: None,
            project_id: None,
            pinned: None,
            reminder_at: None,
        }).await.unwrap();
        assert_eq!(search_notes_impl(&pool, "плов".into()).await.unwrap().len(), 1);
        assert!(search_notes_impl(&pool, "капуст".into()).await.unwrap().is_empty());

        // After a DELETE nothing is found.
        delete_note_impl(&pool, note.id).await.unwrap();
        assert!(search_notes_impl(&pool, "плов".into()).await.unwrap().is_empty());
        assert!(search_notes_impl(&pool, "борщ".into()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn fts_snippet_returns_markers() {
        let pool = test_pool().await;

        create_note_impl(&pool, CreateNote {
            title: "рецепт".into(),
            content: "свёкла и капуста для борща".into(),
            tags: vec!["еда".into()],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();

        let results = search_notes_snippet_impl(&pool, "капуста".into()).await.unwrap();
        assert_eq!(results.len(), 1, "snippet={:?}", results[0].snippet);
        assert!(results[0].snippet.contains("<mark>"), "snippet should contain <mark>, got: {:?}", results[0].snippet);
        assert!(results[0].snippet.contains("</mark>"), "snippet should contain </mark>");
        assert!(results[0].snippet.contains("капуста"), "snippet should contain query word");
        assert_eq!(results[0].item.content, "свёкла и капуста для борща");
    }

    #[test]
    fn rewrite_links_covers_alias_case_and_self_link() {
        // A plain link
        let (out, changed) = rewrite_links("см. [[Идея]] тут", "Идея", "Новая идея");
        assert_eq!(out, "см. [[Новая идея]] тут");
        assert!(changed);

        // The alias is kept, only the target changes
        let (out, changed) = rewrite_links("[[Идея|вот тут]]", "Идея", "Новая идея");
        assert_eq!(out, "[[Новая идея|вот тут]]");
        assert!(changed);

        // Case-insensitive
        let (out, changed) = rewrite_links("[[идея]]", "Идея", "Новая идея");
        assert_eq!(out, "[[Новая идея]]");
        assert!(changed);

        // Several links, only the matching ones are rewritten
        let (out, changed) = rewrite_links("[[Идея]] и [[Другая]] и снова [[Идея|та же]]", "Идея", "X");
        assert_eq!(out, "[[X]] и [[Другая]] и снова [[X|та же]]");
        assert!(changed);

        // No matches, nothing changed
        let (out, changed) = rewrite_links("[[Другая]] заметка", "Идея", "X");
        assert_eq!(out, "[[Другая]] заметка");
        assert!(!changed);

        // A self-link [[Идея]] -> [[X]] is rewritten like any other link
        let (out, changed) = rewrite_links("это [[Идея]] сама на себя", "Идея", "X");
        assert_eq!(out, "это [[X]] сама на себя");
        assert!(changed);

        // An unclosed link does not break parsing
        let (out, changed) = rewrite_links("текст [[Идея без закрытия", "Идея", "X");
        assert_eq!(out, "текст [[Идея без закрытия");
        assert!(!changed);
    }

    #[tokio::test]
    async fn rename_note_links_updates_across_notes_and_counts() {
        let pool = test_pool().await;

        let target = create_note_impl(&pool, CreateNote {
            title: "Идея".into(), content: "исходная".into(),
            tags: vec![], linked_task_id: None, project_id: None,
        }).await.unwrap();
        let referrer1 = create_note_impl(&pool, CreateNote {
            title: "Черновик".into(), content: "см. [[Идея]]".into(),
            tags: vec![], linked_task_id: None, project_id: None,
        }).await.unwrap();
        let referrer2 = create_note_impl(&pool, CreateNote {
            title: "Заметки".into(), content: "[[идея|та самая]] и [[Другая]]".into(),
            tags: vec![], linked_task_id: None, project_id: None,
        }).await.unwrap();
        let unrelated = create_note_impl(&pool, CreateNote {
            title: "Не связана".into(), content: "просто текст".into(),
            tags: vec![], linked_task_id: None, project_id: None,
        }).await.unwrap();

        let count = rename_note_links_impl(&pool, "Идея".into(), "Идея v2".into()).await.unwrap();
        assert_eq!(count, 2); // referrer1 and referrer2; target and unrelated are not counted

        let all = get_notes_impl(&pool).await.unwrap();
        let by_id = |id: &str| all.iter().find(|n| n.id == id).unwrap().content.clone();
        assert_eq!(by_id(&referrer1.id), "см. [[Идея v2]]");
        assert_eq!(by_id(&referrer2.id), "[[Идея v2|та самая]] и [[Другая]]");
        assert_eq!(by_id(&unrelated.id), "просто текст");
        assert_eq!(by_id(&target.id), "исходная"); // the target's content is not rewritten

        // An empty old_title, or one differing only in case, is a no-op
        assert_eq!(rename_note_links_impl(&pool, "".into(), "X".into()).await.unwrap(), 0);
        assert_eq!(rename_note_links_impl(&pool, "Идея v2".into(), "идея v2".into()).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn empty_title_becomes_placeholder() {
        let pool = test_pool().await;
        let note = create_note_impl(&pool, CreateNote {
            title: "   ".into(),
            content: "x".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();
        assert_eq!(note.title, "Без названия");
    }

    #[tokio::test]
    async fn tags_and_link_roundtrip() {
        let pool = test_pool().await;
        let note = create_note_impl(&pool, CreateNote {
            title: "с тегами".into(),
            content: "x".into(),
            tags: vec!["work".into(), "idea".into()],
            linked_task_id: Some("task-1".into()),
            project_id: Some("proj-1".into()),
        }).await.unwrap();
        assert_eq!(note.tags, vec!["work", "idea"]);
        assert_eq!(note.linked_task_id.as_deref(), Some("task-1"));

        // Re-read from the DB: tag serialization/parsing and the link survived
        let all = get_notes_impl(&pool).await.unwrap();
        assert_eq!(all[0].tags, vec!["work", "idea"]);
        assert_eq!(all[0].linked_task_id.as_deref(), Some("task-1"));

        // Updating tags and unlinking (Some(None))
        let updated = update_note_impl(&pool, note.id.clone(), UpdateNote {
            title: None,
            content: None,
            tags: Some(vec!["done".into()]),
            linked_task_id: Some(None),
            project_id: Some(None),
            pinned: None,
            reminder_at: None,
        }).await.unwrap();
        assert_eq!(updated.tags, vec!["done"]);
        assert_eq!(updated.linked_task_id, None);
        assert_eq!(updated.project_id, None);
    }

    fn content_patch(content: &str) -> UpdateNote {
        UpdateNote { title: None, content: Some(content.into()), tags: None, linked_task_id: None, project_id: None, pinned: None, reminder_at: None }
    }

    async fn revision_count(pool: &SqlitePool, note_id: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM note_revisions WHERE note_id = ?")
            .bind(note_id).fetch_one(pool).await.unwrap()
    }

    async fn set_last_revision_at(pool: &SqlitePool, note_id: &str, at: &str) {
        sqlx::query("UPDATE note_revisions SET created_at = ? WHERE note_id = ? AND id = (
            SELECT id FROM note_revisions WHERE note_id = ? ORDER BY created_at DESC LIMIT 1
        )")
        .bind(at).bind(note_id).bind(note_id)
        .execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn first_content_edit_snapshots_original() {
        let pool = test_pool().await;
        let note = create_note_impl(&pool, CreateNote {
            title: "т".into(), content: "исходный текст".into(),
            tags: vec![], linked_task_id: None, project_id: None,
        }).await.unwrap();

        update_note_impl(&pool, note.id.clone(), content_patch("новый текст")).await.unwrap();

        assert_eq!(revision_count(&pool, &note.id).await, 1);
        let revs = get_note_revisions_impl(&pool, &note.id).await.unwrap();
        assert_eq!(revs.len(), 1);
    }

    #[tokio::test]
    async fn second_edit_within_interval_does_not_snapshot_again() {
        let pool = test_pool().await;
        let note = create_note_impl(&pool, CreateNote {
            title: "т".into(), content: "v1".into(),
            tags: vec![], linked_task_id: None, project_id: None,
        }).await.unwrap();

        update_note_impl(&pool, note.id.clone(), content_patch("v2")).await.unwrap();
        assert_eq!(revision_count(&pool, &note.id).await, 1);

        // An edit right after: the interval (10 min) has not elapsed yet
        update_note_impl(&pool, note.id.clone(), content_patch("v3")).await.unwrap();
        assert_eq!(revision_count(&pool, &note.id).await, 1);
    }

    #[tokio::test]
    async fn edit_after_interval_snapshots_again() {
        let pool = test_pool().await;
        let note = create_note_impl(&pool, CreateNote {
            title: "т".into(), content: "v1".into(),
            tags: vec![], linked_task_id: None, project_id: None,
        }).await.unwrap();

        update_note_impl(&pool, note.id.clone(), content_patch("v2")).await.unwrap();
        assert_eq!(revision_count(&pool, &note.id).await, 1);

        // Push the latest revision 11 minutes back so the interval has elapsed
        let stale = (Utc::now() - chrono::Duration::minutes(11)).to_rfc3339();
        set_last_revision_at(&pool, &note.id, &stale).await;

        update_note_impl(&pool, note.id.clone(), content_patch("v3")).await.unwrap();
        assert_eq!(revision_count(&pool, &note.id).await, 2);
    }

    #[tokio::test]
    async fn rotation_keeps_at_most_twenty() {
        let pool = test_pool().await;
        let note = create_note_impl(&pool, CreateNote {
            title: "т".into(), content: "v0".into(),
            tags: vec![], linked_task_id: None, project_id: None,
        }).await.unwrap();

        // 25 edits, each ageing the previous revision past the interval so a snapshot happens
        for i in 1..=25 {
            update_note_impl(&pool, note.id.clone(), content_patch(&format!("v{i}"))).await.unwrap();
            let stale = (Utc::now() - chrono::Duration::minutes(11)).to_rfc3339();
            set_last_revision_at(&pool, &note.id, &stale).await;
        }

        assert_eq!(revision_count(&pool, &note.id).await, 20);
    }

    #[tokio::test]
    async fn restore_cycle_swaps_content_and_snapshots_current() {
        let pool = test_pool().await;
        let note = create_note_impl(&pool, CreateNote {
            title: "т".into(), content: "оригинал".into(),
            tags: vec![], linked_task_id: None, project_id: None,
        }).await.unwrap();

        update_note_impl(&pool, note.id.clone(), content_patch("изменённый")).await.unwrap();
        let revs = get_note_revisions_impl(&pool, &note.id).await.unwrap();
        assert_eq!(revs.len(), 1);
        let original_rev_id = revs[0].id.clone();

        let restored = restore_note_revision_impl(&pool, &original_rev_id).await.unwrap();
        assert_eq!(restored.content, "оригинал");

        // The current text landed in revisions too, so moving forward again is possible
        let revs_after = get_note_revisions_impl(&pool, &note.id).await.unwrap();
        assert_eq!(revs_after.len(), 2);
    }

    #[tokio::test]
    async fn restore_missing_revision_errors() {
        let pool = test_pool().await;
        let r = restore_note_revision_impl(&pool, "no-such-id").await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn delete_note_cascades_revisions() {
        let pool = test_pool().await;
        let note = create_note_impl(&pool, CreateNote {
            title: "т".into(), content: "v1".into(),
            tags: vec![], linked_task_id: None, project_id: None,
        }).await.unwrap();
        update_note_impl(&pool, note.id.clone(), content_patch("v2")).await.unwrap();
        assert_eq!(revision_count(&pool, &note.id).await, 1);

        // Since v0.9.76 delete is soft, so the cascade moved to purge: the Trash
        // must keep the history, or restoring a note would silently lose it.
        delete_note_impl(&pool, note.id.clone()).await.unwrap();
        assert_eq!(revision_count(&pool, &note.id).await, 1, "Корзина не трогает ревизии");

        purge_deleted_note_impl(&pool, note.id.clone()).await.unwrap();
        assert_eq!(revision_count(&pool, &note.id).await, 0);
    }

    fn temp_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("kriptag-md-test-{}", Uuid::new_v4()))
    }

    #[tokio::test]
    async fn export_roundtrip_recreates_notes_on_import() {
        let pool = test_pool().await;
        create_note_impl(&pool, CreateNote {
            title: "Первая заметка".into(), content: "текст один".into(),
            tags: vec![], linked_task_id: None, project_id: None,
        }).await.unwrap();
        create_note_impl(&pool, CreateNote {
            title: "Вторая заметка".into(), content: "[[Первая заметка]] текст два".into(),
            tags: vec![], linked_task_id: None, project_id: None,
        }).await.unwrap();

        let dir = temp_dir();
        let exported = export_notes_md_impl(&pool, &dir).await.unwrap();
        assert_eq!(exported, 2);
        assert!(dir.join("Первая заметка.md").exists());
        assert!(dir.join("Вторая заметка.md").exists());

        let pool2 = test_pool().await;
        let imported = import_notes_md_impl(&pool2, &dir).await.unwrap();
        assert_eq!(imported, 2);
        let notes = get_notes_impl(&pool2).await.unwrap();
        assert_eq!(notes.len(), 2);
        assert!(notes.iter().any(|n| n.title == "Первая заметка" && n.content == "текст один"));
        assert!(notes.iter().any(|n| n.title == "Вторая заметка" && n.content.contains("[[Первая заметка]]")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_filename_replaces_forbidden_chars() {
        assert_eq!(sanitize_filename("отчёт: план/факт"), "отчёт_ план_факт");
        assert_eq!(sanitize_filename("a<b>c|d?e*f\"g"), "a_b_c_d_e_f_g");
        assert_eq!(sanitize_filename("   "), "Без названия");
        assert_eq!(sanitize_filename(""), "Без названия");
    }

    #[tokio::test]
    async fn export_disambiguates_duplicate_titles() {
        let pool = test_pool().await;
        for _ in 0..3 {
            create_note_impl(&pool, CreateNote {
                title: "дубликат".into(), content: "x".into(),
                tags: vec![], linked_task_id: None, project_id: None,
            }).await.unwrap();
        }
        let dir = temp_dir();
        let exported = export_notes_md_impl(&pool, &dir).await.unwrap();
        assert_eq!(exported, 3);
        assert!(dir.join("дубликат.md").exists());
        assert!(dir.join("дубликат-2.md").exists());
        assert!(dir.join("дубликат-3.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn import_ignores_non_md_files() {
        let pool = test_pool().await;
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("заметка.md"), "содержимое").unwrap();
        std::fs::write(dir.join("картинка.png"), "не текст").unwrap();

        let imported = import_notes_md_impl(&pool, &dir).await.unwrap();
        assert_eq!(imported, 1);
        let notes = get_notes_impl(&pool).await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "заметка");
        assert_eq!(notes[0].content, "содержимое");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn import_of_missing_or_empty_dir_is_zero() {
        let pool = test_pool().await;
        let dir = temp_dir(); // not created, so it does not exist
        assert_eq!(import_notes_md_impl(&pool, &dir).await.unwrap(), 0);

        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(import_notes_md_impl(&pool, &dir).await.unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn import_duplicate_titles_create_separate_notes() {
        let pool = test_pool().await;
        create_note_impl(&pool, CreateNote {
            title: "уже есть".into(), content: "старое".into(),
            tags: vec![], linked_task_id: None, project_id: None,
        }).await.unwrap();

        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("уже есть.md"), "новое").unwrap();

        let imported = import_notes_md_impl(&pool, &dir).await.unwrap();
        assert_eq!(imported, 1);
        let notes = get_notes_impl(&pool).await.unwrap();
        assert_eq!(notes.len(), 2);
        assert!(notes.iter().any(|n| n.content == "старое"));
        assert!(notes.iter().any(|n| n.content == "новое"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // A sweep over every read path a trashed note could leak through.
    //
    // This is the test v0.9.76 was missing. That bug was not a mistyped filter:
    // notes.deleted_at did not exist until migration 0032, so every note query
    // written before it simply had no filter to get wrong, and the quick slot
    // (pinned.rs) kept serving a note out of the Trash. A shared SQL constant
    // would not have caught it either — a query written before the column would
    // not have used the constant.
    //
    // What catches that class is asking the question from outside the queries:
    // put one note in the Trash, run every reader, and require it to appear in
    // none of them. A new reader added without the filter fails here; so does a
    // new column with the same story as 0032, as soon as its reader is listed.
    //
    // Deliberately not table-driven: each call has a different signature and
    // return type, and spelling them out is what makes a missing entry visible.
    #[tokio::test]
    async fn a_trashed_note_surfaces_from_no_read_path() {
        use crate::commands::pinned::{get_pinned_impl, set_pinned_impl};

        let pool = test_pool().await;
        let alive = create_note_impl(&pool, CreateNote {
            title: "живая".into(),
            content: "общее слово барсук".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();
        let trashed = create_note_impl(&pool, CreateNote {
            title: "выброшенная".into(),
            content: "общее слово барсук и ссылка [[живая]]".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();

        // Pinned before deletion: the slot keeps an id in settings, so it must
        // notice the note left rather than serve a stale row.
        set_pinned_impl(&pool, Some("note".into()), Some(trashed.id.clone()))
            .await.unwrap();

        delete_note_impl(&pool, trashed.id.clone()).await.unwrap();

        let list = get_notes_impl(&pool).await.unwrap();
        assert_eq!(list.len(), 1, "список заметок отдал удалённую");
        assert_eq!(list[0].id, alive.id);

        // Both searches go through FTS, where a soft delete is an UPDATE: the row
        // stays in the index and only the query can exclude it.
        let found = search_notes_impl(&pool, "барсук".into()).await.unwrap();
        assert_eq!(found.len(), 1, "search_notes нашёл удалённую");
        assert_eq!(found[0].id, alive.id);

        let snippets = search_notes_snippet_impl(&pool, "барсук".into()).await.unwrap();
        assert_eq!(snippets.len(), 1, "search_notes_snippet нашёл удалённую");

        // The autosave race from v0.9.76: there is no standalone getter, so this
        // goes through update_note_impl. Two queries there filter on deleted_at
        // and only one is load-bearing — the `alive` probe before the writes
        // (notes.rs:148). Removing the filter from the read at the tail leaves
        // this green, because that read only shapes the return value; the writes
        // have already been refused. Hence the content assertion below: the
        // is_err() alone would pass on a version that writes first and reports
        // the failure afterwards, which is the bug v0.9.76 actually had.
        let late_save = update_note_impl(&pool, trashed.id.clone(), UpdateNote {
            title: None,
            content: Some("затёрто опоздавшим автосохранением".into()),
            tags: None,
            linked_task_id: None,
            project_id: None,
            pinned: None,
            reminder_at: None,
        }).await;
        assert!(late_save.is_err(), "опоздавшее автосохранение записало в Корзину");
        let still: String = sqlx::query_scalar("SELECT content FROM notes WHERE id = ?")
            .bind(&trashed.id).fetch_one(&pool).await.unwrap();
        assert!(
            still.contains("барсук"),
            "текст заметки в Корзине затёрт: {still}"
        );

        assert!(
            get_pinned_impl(&pool).await.unwrap().is_none(),
            "быстрый слот отдал заметку из Корзины — ровно баг v0.9.76"
        );

        // Renaming rewrites [[links]] across notes; the trashed one holds such a
        // link and must not be counted or touched.
        let renamed = rename_note_links_impl(&pool, "живая".into(), "переименованная".into())
            .await.unwrap();
        assert_eq!(renamed, 0, "переименование ссылок задело заметку из Корзины");

        // The Trash itself is the one place it must appear.
        let trash = get_deleted_notes_impl(&pool).await.unwrap();
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].id, trashed.id);
    }

    // --- Stemmed search (v0.10.19) ---

    #[tokio::test]
    async fn a_query_in_another_word_form_finds_the_note() {
        let pool = test_pool().await;
        create_note_impl(&pool, CreateNote {
            title: "хозяйство".into(),
            content: "сходить за покупками в субботу".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();

        // Plain FTS5 does not stem: "покупки" and "покупками" are different
        // tokens, and the trailing prefix star does not bridge them either.
        let found = search_notes_impl(&pool, "покупки".into()).await.unwrap();
        assert_eq!(found.len(), 1, "форма слова не нашлась — стемминг не работает");
        assert_eq!(found[0].title, "хозяйство");
    }

    #[tokio::test]
    async fn an_exact_match_outranks_a_stemmed_one() {
        let pool = test_pool().await;
        create_note_impl(&pool, CreateNote {
            title: "по форме".into(),
            content: "сходить за покупками".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();
        create_note_impl(&pool, CreateNote {
            title: "точное".into(),
            content: "мои покупки за месяц".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();

        let found = search_notes_impl(&pool, "покупки".into()).await.unwrap();
        assert_eq!(found.len(), 2, "нашлись не обе заметки");
        assert_eq!(
            found[0].title, "точное",
            "точное совпадение ушло ниже стеммированного — верх выдачи размывается"
        );
    }

    #[tokio::test]
    async fn a_note_is_never_returned_twice() {
        let pool = test_pool().await;
        create_note_impl(&pool, CreateNote {
            title: "одна".into(),
            content: "покупки и ещё раз покупки".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();

        // The word matches exactly AND survives stemming, so the note is in both
        // indexes at once — the merge has to drop the duplicate.
        let found = search_notes_impl(&pool, "покупки".into()).await.unwrap();
        assert_eq!(found.len(), 1, "заметка попала в выдачу дважды: {found:?}");
    }

    #[tokio::test]
    async fn every_write_path_reaches_the_stemmed_index() {
        // The guard for the one thing that cannot be guaranteed by construction.
        // Notes are written from five places in this file; each is exercised here
        // and afterwards every note has to be findable by its CURRENT text.
        //
        // Comparing row counts instead was the first version of this test, and it
        // was worthless: dropping the UPDATE trigger left the counts equal — the
        // row is created and counted, it just holds stale text.
        //
        // Searching once at the end was the second version, and it was worthless
        // for a subtler reason: the INSERT mark survives until the first search,
        // so a reindex that runs only after every edit reads the current text
        // anyway and the missing UPDATE trigger stays invisible. The index has to
        // be BUILT first, and only then edited — that is when a lost mark shows.
        // Both failures found by breaking the trigger, not by reasoning.
        let pool = test_pool().await;

        let a = create_note_impl(&pool, CreateNote {
            title: "первая".into(),
            content: "исходный текст".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();

        // Builds the index and consumes the INSERT marks.
        assert_eq!(
            search_notes_impl(&pool, "исходный".into()).await.unwrap().len(), 1,
            "заметка не нашлась до правок — индекс не построился"
        );

        update_note_impl(&pool, a.id.clone(), UpdateNote {
            title: Some("переименованная".into()),
            content: Some("совсем другие покупками слова".into()),
            tags: Some(vec!["метка".into()]),
            linked_task_id: None,
            project_id: None,
            pinned: None,
            reminder_at: None,
        }).await.unwrap();

        create_note_impl(&pool, CreateNote {
            title: "вторая".into(),
            content: "ссылка [[переименованная]] внутри".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();

        // Same again for the mass rewrite: index it, then rewrite it.
        assert_eq!(
            search_notes_impl(&pool, "ссылка".into()).await.unwrap().len(), 1,
            "вторая заметка не нашлась до переименования"
        );
        rename_note_links_impl(&pool, "переименованная".into(), "новое имя".into())
            .await.unwrap();

        reindex_stemmed_notes(&pool).await.unwrap();

        // Every note, by a distinctive word of the text it holds RIGHT NOW. Each
        // of these arrived through a different write path, and a path that did
        // not reach the index leaves its note unfindable here.
        for (word, expect_title) in [
            ("покупки", "переименованная"),   // update_note_impl (content)
            ("переименованная", "переименованная"), // update_note_impl (title)
            ("новое", "вторая"),              // rename_note_links_impl (mass rewrite)
        ] {
            let found = search_notes_impl(&pool, word.into()).await.unwrap();
            assert!(
                found.iter().any(|n| n.title == expect_title),
                "«{word}» не нашло заметку «{expect_title}» — путь записи прошёл мимо индекса; нашлось: {:?}",
                found.iter().map(|n| &n.title).collect::<Vec<_>>()
            );
        }

        // And nothing is findable by text that was replaced.
        assert!(
            search_notes_impl(&pool, "исходный".into()).await.unwrap().is_empty(),
            "заметка находится по затёртому тексту — старая версия осталась в индексе"
        );

        let dirty: i64 = sqlx::query_scalar("SELECT count(*) FROM notes_stem_dirty")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(dirty, 0, "после переиндексации остались помеченные строки");
    }

    #[tokio::test]
    async fn an_edited_note_stops_matching_its_old_words() {
        let pool = test_pool().await;
        let note = create_note_impl(&pool, CreateNote {
            title: "заметка".into(),
            content: "про покупками".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();
        assert_eq!(search_notes_impl(&pool, "покупки".into()).await.unwrap().len(), 1);

        update_note_impl(&pool, note.id.clone(), UpdateNote {
            title: None,
            content: Some("теперь про созвоны".into()),
            tags: None,
            linked_task_id: None,
            project_id: None,
            pinned: None,
            reminder_at: None,
        }).await.unwrap();

        // FTS5 has no upsert: without deleting the old row first, the note would
        // keep matching words it no longer contains.
        assert!(
            search_notes_impl(&pool, "покупки".into()).await.unwrap().is_empty(),
            "заметка находится по слову, которого в ней больше нет"
        );
        assert_eq!(search_notes_impl(&pool, "созвон".into()).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_trashed_note_stays_out_of_the_stemmed_half_too() {
        let pool = test_pool().await;
        let note = create_note_impl(&pool, CreateNote {
            title: "выброшенная".into(),
            content: "сходить за покупками".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();

        delete_note_impl(&pool, note.id).await.unwrap();

        assert!(
            search_notes_impl(&pool, "покупки".into()).await.unwrap().is_empty(),
            "заметка из Корзины нашлась через стеммированный индекс"
        );
    }

    #[tokio::test]
    async fn a_stemmed_hit_carries_a_readable_excerpt() {
        let pool = test_pool().await;
        create_note_impl(&pool, CreateNote {
            title: "заметка".into(),
            content: "сходить за покупками в субботу утром".into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap();

        let found = search_notes_snippet_impl(&pool, "покупки".into()).await.unwrap();
        assert_eq!(found.len(), 1);
        // The excerpt comes from the note's own text, never from the stemmed
        // index — a snippet() there would highlight "покупк" instead of a word.
        assert!(
            found[0].snippet.contains("покупками"),
            "в выдержке нет текста заметки: {:?}", found[0].snippet
        );
        assert!(
            !found[0].snippet.contains("покупк "),
            "в выдержку просочился стем: {:?}", found[0].snippet
        );
    }
}

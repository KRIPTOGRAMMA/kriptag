-- Stemmed search over notes, alongside notes_fts rather than instead of it.
--
-- notes_fts stays exactly as it is: it is an external-content table
-- (content='notes'), so FTS does not store the text but reads it back from
-- notes, and that is what makes snippet() highlight the real words. Putting
-- stems into it — a shadow column or a different tokenizer — would make
-- snippet() highlight stems instead. So the stems live in a table of their own
-- and the two searches are merged in Rust, exact matches first.
--
-- This one is NOT external content: it holds the stemmed text itself, which
-- exists nowhere else. Nothing ever calls snippet() on it — it only answers
-- "does this note match", and the snippet is taken from notes_fts.
CREATE VIRTUAL TABLE notes_stem_fts USING fts5(
    title,
    content,
    tags
);

-- The stems cannot be produced by a trigger: stemming lives in Rust and SQLite
-- cannot call into it. Writing the sync into every write path is what would go
-- wrong — the note text is written from five different places in notes.rs
-- (create, update.title, update.content, save_revision, and the wiki-link rename
-- that rewrites many notes at once), and a path added later would silently drop
-- out of the stemmed index with nothing to catch it.
--
-- So the trigger does the one thing a trigger can do and the database can
-- guarantee: it marks the note dirty. Rust re-stems whatever is marked. No write
-- path can slip past this, because it is the database itself doing the marking.
CREATE TABLE notes_stem_dirty (
    rowid_ref INTEGER PRIMARY KEY
);

CREATE TRIGGER notes_stem_ai AFTER INSERT ON notes BEGIN
    INSERT OR REPLACE INTO notes_stem_dirty(rowid_ref) VALUES (new.rowid);
END;

CREATE TRIGGER notes_stem_au AFTER UPDATE ON notes BEGIN
    INSERT OR REPLACE INTO notes_stem_dirty(rowid_ref) VALUES (new.rowid);
END;

CREATE TRIGGER notes_stem_ad AFTER DELETE ON notes BEGIN
    DELETE FROM notes_stem_fts WHERE rowid = old.rowid;
    DELETE FROM notes_stem_dirty WHERE rowid_ref = old.rowid;
END;

-- Every existing note starts dirty: the stemmed index is empty and has to be
-- built once. Doing it here rather than in Rust keeps the migration honest —
-- after it runs the invariant "dirty is what still needs stemming" already holds.
INSERT INTO notes_stem_dirty(rowid_ref) SELECT rowid FROM notes;

//! The `archive.db` SQLite database of archived posts.

pub mod import;
pub mod rows;

use std::time::Duration;

use anyhow::{Context as _, bail};
use camino::{Utf8Path, Utf8PathBuf};
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, named_params, params};

use rows::{Collection, PostRecord, PostRow, PostRows, RowSource, blog_names_match, build_rows};

pub const DB_FILE_NAME: &str = "archive.db";
pub const SCHEMA_VERSION: i32 = 1;

/// Long enough to ride out a `sqlite3` session holding the write lock briefly,
/// short enough that a wedged one surfaces as an error instead of a hang.
const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

const SCHEMA: &str = r"
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- keys: blog_name, created_at, updated_at (RFC 3339 UTC)

CREATE TABLE posts (
    id              INTEGER PRIMARY KEY,   -- Tumblr post id
    collection      TEXT NOT NULL,         -- 'posts' | 'drafts' | 'queue' | 'submissions' | 'likes'
    blog_name       TEXT NOT NULL,         -- blog holding the post (differs from the archive's blog for likes)
    timestamp       INTEGER NOT NULL,
    date            TEXT NOT NULL,         -- RFC 3339 UTC, e.g. 2026-10-28T16:50:30Z
    original_type   TEXT,                  -- legacy type: regular, photo, quote, ...
    state           TEXT,
    post_url        TEXT,
    short_url       TEXT,
    slug            TEXT,
    summary         TEXT,
    note_count      INTEGER,
    is_reblog       INTEGER NOT NULL,      -- reblogged_from_name IS NOT NULL (same rule as --no-reblogs)
    parent_blog     TEXT,                  -- reblogged_from_name
    parent_post_url TEXT,                  -- reblogged_from_url
    root_blog       TEXT,                  -- reblogged_root_name
    root_post_url   TEXT,                  -- reblogged_root_url
    body            TEXT NOT NULL,         -- plain text of the post's own content
    trail_text      TEXT NOT NULL,         -- plain text of all trail items, blank-line separated
    tags            TEXT NOT NULL,         -- newline-joined, for FTS
    authors         TEXT NOT NULL,         -- newline-joined distinct trail blog names, for FTS
    file_path       TEXT,                  -- archived file, relative to the database's directory
    source          TEXT NOT NULL,         -- 'api' | 'json-import'
    archived_at     INTEGER NOT NULL       -- when this row was last written
);
CREATE INDEX posts_timestamp ON posts (timestamp);
CREATE INDEX posts_collection ON posts (collection);

CREATE TABLE tags (
    post_id  INTEGER NOT NULL REFERENCES posts (id),
    position INTEGER NOT NULL,
    tag      TEXT NOT NULL,
    PRIMARY KEY (post_id, position)
);
CREATE INDEX tags_tag ON tags (tag COLLATE NOCASE);

CREATE TABLE trail (
    post_id        INTEGER NOT NULL REFERENCES posts (id),
    position       INTEGER NOT NULL,       -- trail order as returned (root first)
    blog           TEXT,
    source_post_id TEXT,
    is_root        INTEGER NOT NULL,
    text           TEXT NOT NULL,
    PRIMARY KEY (post_id, position)
);
CREATE INDEX trail_blog ON trail (blog);

CREATE TABLE media (
    post_id        INTEGER NOT NULL REFERENCES posts (id),
    position       INTEGER NOT NULL,       -- document order: own content, then trail items in order
    trail_position INTEGER,                -- NULL for the post's own content
    kind           TEXT NOT NULL,          -- 'image' | 'video' | 'audio'
    external       INTEGER NOT NULL,       -- 1 for third-party embeds (YouTube, ...)
    url            TEXT,                   -- original remote URL of the variant archivr selects
    path           TEXT,                   -- local copy, relative to the database's directory
    mime           TEXT,
    width          INTEGER,
    height         INTEGER,
    alt_text       TEXT,
    PRIMARY KEY (post_id, position)
);
CREATE INDEX media_kind ON media (kind);

CREATE TABLE notes (
    post_id   INTEGER NOT NULL REFERENCES posts (id),
    position  INTEGER NOT NULL,
    type      TEXT NOT NULL,               -- like, reblog, reply, posted, ...
    blog_name TEXT,
    timestamp INTEGER,
    text      TEXT,                        -- reply text or reblog commentary
    tags      TEXT,                        -- newline-joined reblog tags
    PRIMARY KEY (post_id, position)
);

CREATE VIRTUAL TABLE posts_fts USING fts5 (
    body, trail_text, tags, authors,
    content = 'posts', content_rowid = 'id',
    tokenize = 'porter unicode61 remove_diacritics 2',
    prefix = '2 3'
);

CREATE TRIGGER posts_ai AFTER INSERT ON posts BEGIN
    INSERT INTO posts_fts (rowid, body, trail_text, tags, authors)
    VALUES (new.id, new.body, new.trail_text, new.tags, new.authors);
END;

CREATE TRIGGER posts_ad AFTER DELETE ON posts BEGIN
    INSERT INTO posts_fts (posts_fts, rowid, body, trail_text, tags, authors)
    VALUES ('delete', old.id, old.body, old.trail_text, old.tags, old.authors);
END;

CREATE TRIGGER posts_au AFTER UPDATE ON posts BEGIN
    INSERT INTO posts_fts (posts_fts, rowid, body, trail_text, tags, authors)
    VALUES ('delete', old.id, old.body, old.trail_text, old.tags, old.authors);
    INSERT INTO posts_fts (rowid, body, trail_text, tags, authors)
    VALUES (new.id, new.body, new.trail_text, new.tags, new.authors);
END;
";

const UPSERT_POST: &str = "
INSERT INTO posts (
    id, collection, blog_name, timestamp, date, original_type, state, post_url,
    short_url, slug, summary, note_count, is_reblog, parent_blog, parent_post_url,
    root_blog, root_post_url, body, trail_text, tags, authors, file_path, source,
    archived_at
) VALUES (
    :id, :collection, :blog_name, :timestamp, :date, :original_type, :state, :post_url,
    :short_url, :slug, :summary, :note_count, :is_reblog, :parent_blog, :parent_post_url,
    :root_blog, :root_post_url, :body, :trail_text, :tags, :authors, :file_path, :source,
    :archived_at
)
ON CONFLICT(id) DO UPDATE SET
    collection = excluded.collection,
    blog_name = excluded.blog_name,
    timestamp = excluded.timestamp,
    date = excluded.date,
    original_type = excluded.original_type,
    state = excluded.state,
    post_url = excluded.post_url,
    short_url = excluded.short_url,
    slug = excluded.slug,
    summary = excluded.summary,
    note_count = excluded.note_count,
    is_reblog = excluded.is_reblog,
    parent_blog = excluded.parent_blog,
    parent_post_url = excluded.parent_post_url,
    root_blog = excluded.root_blog,
    root_post_url = excluded.root_post_url,
    body = excluded.body,
    trail_text = excluded.trail_text,
    tags = excluded.tags,
    authors = excluded.authors,
    file_path = excluded.file_path,
    source = excluded.source,
    archived_at = excluded.archived_at
";

/// What [`ArchiveDb::write_post`] did with a post.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Written,
    /// An import never replaces a row built from the API.
    SkippedApiRow,
    /// A draft, queued, submitted, or liked copy never replaces the published
    /// post's row.
    SkippedPublishedRow,
}

/// The database of one archive directory.
///
/// Writes accumulate in a transaction that [`commit`](Self::commit) ends.
/// Dropping the handle with one open discards it, so an interrupted run leaves
/// the database at its last commit, never mid-page.
#[derive(Debug)]
pub struct ArchiveDb {
    conn: Connection,
    path: Utf8PathBuf,
    created: bool,
}

impl ArchiveDb {
    pub fn path(dir: &Utf8Path) -> Utf8PathBuf {
        dir.join(DB_FILE_NAME)
    }

    pub fn exists(dir: &Utf8Path) -> bool {
        fs_err::exists(Self::path(dir)).unwrap_or(false)
    }

    /// The blog the database belongs to, or `None` when there is no database
    /// yet. Never creates the file.
    pub fn stored_blog_name(dir: &Utf8Path) -> anyhow::Result<Option<String>> {
        if !Self::exists(dir) {
            return Ok(None);
        }
        let path = Self::path(dir);
        let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("failed to open {path}"))?;
        read_blog_name(&conn).with_context(|| format!("failed to read {path}"))
    }

    /// Opens the database in `dir`, creating the file and schema when the
    /// directory has none yet.
    pub fn open(dir: &Utf8Path, blog_name: &str) -> anyhow::Result<Self> {
        let path = Self::path(dir);
        Self::open_at(path.clone(), blog_name).with_context(|| error_context(&path))
    }

    fn open_at(path: Utf8PathBuf, blog_name: &str) -> anyhow::Result<Self> {
        let mut conn = Connection::open(&path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;

        let version = user_version(&conn)?;
        if version > SCHEMA_VERSION {
            bail!(
                "was written by a newer archivr (schema version {version}, \
                 this build supports up to {SCHEMA_VERSION})"
            );
        }

        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;

        let created = version == 0;
        if created {
            create_schema(&mut conn, blog_name).context("failed to create the schema")?;
        } else {
            let stored = read_blog_name(&conn)?.context("has no blog_name recorded")?;
            if !blog_names_match(&stored, blog_name) {
                bail!("is for blog `{stored}`, but `{blog_name}` was requested");
            }
        }
        Ok(Self {
            conn,
            path,
            created,
        })
    }

    /// Whether this `open` created the schema, as opposed to finding one.
    pub fn created(&self) -> bool {
        self.created
    }

    /// Writes the post's rows, replacing everything previously stored for it,
    /// unless a row already there takes precedence.
    pub fn write_post(&mut self, rec: &PostRecord<'_>) -> anyhow::Result<WriteOutcome> {
        let rows = build_rows(rec)?;
        self.store_rows(rec, &rows)
            .with_context(|| error_context(&self.path))
    }

    fn store_rows(
        &mut self,
        rec: &PostRecord<'_>,
        rows: &PostRows,
    ) -> anyhow::Result<WriteOutcome> {
        let id = rows.post.id;

        if self.conn.is_autocommit() {
            // IMMEDIATE takes the write lock up front. A deferred transaction
            // reads before it writes, and in WAL mode a writer that committed
            // in between makes the upgrade fail with SQLITE_BUSY_SNAPSHOT,
            // which the busy timeout does not retry.
            self.conn.execute_batch("BEGIN IMMEDIATE")?;
        }

        let existing: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT source, collection FROM posts WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((source, collection)) = existing {
            let existing_published = collection == Collection::Posts.as_str();
            let incoming_published = rec.collection == Collection::Posts;
            if existing_published && !incoming_published {
                return Ok(WriteOutcome::SkippedPublishedRow);
            }
            // The published row wins over another collection's copy, whatever its source.
            let replaces_other_collection = incoming_published && !existing_published;
            if !replaces_other_collection
                && rec.source == RowSource::JsonImport
                && source == RowSource::Api.as_str()
            {
                return Ok(WriteOutcome::SkippedApiRow);
            }
        }

        // A savepoint, so a failure partway through a post's tables doesn't
        // leave half a post in the page's transaction.
        let sp = self.conn.savepoint()?;
        upsert_post(&sp, &rows.post)?;
        replace_child_rows(&sp, id, rows)?;
        sp.commit()?;
        Ok(WriteOutcome::Written)
    }

    /// Commits the open transaction, if any.
    pub fn commit(&mut self) -> anyhow::Result<()> {
        if self.conn.is_autocommit() {
            return Ok(());
        }
        self.commit_transaction()
            .with_context(|| error_context(&self.path))
    }

    fn commit_transaction(&mut self) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'updated_at'",
            [now_rfc3339()],
        )?;
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }

    pub fn post_count(&self) -> anyhow::Result<u64> {
        let count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM posts", [], |row| row.get(0))
            .with_context(|| error_context(&self.path))?;
        Ok(u64::try_from(count)?)
    }
}

/// Prefixes every error [`ArchiveDb`] reports: SQLite's own messages
/// ("database is locked") don't say which file they are about.
fn error_context(path: &Utf8Path) -> String {
    format!("{DB_FILE_NAME} at {path}")
}

/// Inserts the `posts` row, or overwrites every column of the existing one.
/// The update fires the FTS trigger that swaps the row's index entries.
fn upsert_post(conn: &Connection, post: &PostRow) -> anyhow::Result<()> {
    conn.prepare_cached(UPSERT_POST)?.execute(named_params! {
        ":id": post.id,
        ":collection": post.collection.as_str(),
        ":blog_name": post.blog_name,
        ":timestamp": post.timestamp,
        ":date": post.date,
        ":original_type": post.original_type,
        ":state": post.state,
        ":post_url": post.post_url,
        ":short_url": post.short_url,
        ":slug": post.slug,
        ":summary": post.summary,
        ":note_count": post.note_count,
        ":is_reblog": post.is_reblog,
        ":parent_blog": post.parent_blog,
        ":parent_post_url": post.parent_post_url,
        ":root_blog": post.root_blog,
        ":root_post_url": post.root_post_url,
        ":body": post.body,
        ":trail_text": post.trail_text,
        ":tags": post.tags,
        ":authors": post.authors,
        ":file_path": post.file_path,
        ":source": post.source.as_str(),
        ":archived_at": chrono::Utc::now().timestamp(),
    })?;
    Ok(())
}

/// Replaces the post's `tags`, `trail`, `media`, and `notes` rows wholesale,
/// since a re-archived post can have fewer of any of them than before.
fn replace_child_rows(conn: &Connection, id: i64, rows: &PostRows) -> anyhow::Result<()> {
    for table in ["tags", "trail", "media", "notes"] {
        conn.prepare_cached(&format!("DELETE FROM {table} WHERE post_id = ?1"))?
            .execute([id])?;
    }

    let mut insert_tag =
        conn.prepare_cached("INSERT INTO tags (post_id, position, tag) VALUES (?1, ?2, ?3)")?;
    for (position, tag) in (0_i64..).zip(&rows.tags) {
        insert_tag.execute(params![id, position, tag])?;
    }

    let mut insert_trail = conn.prepare_cached(
        "INSERT INTO trail (post_id, position, blog, source_post_id, is_root, text)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for row in &rows.trail {
        insert_trail.execute(params![
            id,
            row.position,
            row.blog,
            row.source_post_id,
            row.is_root,
            row.text,
        ])?;
    }

    let mut insert_media = conn.prepare_cached(
        "INSERT INTO media (post_id, position, trail_position, kind, external, url, path,
                            mime, width, height, alt_text)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?;
    for row in &rows.media {
        insert_media.execute(params![
            id,
            row.position,
            row.trail_position,
            row.kind,
            row.external,
            row.url,
            row.path,
            row.mime,
            row.width,
            row.height,
            row.alt_text,
        ])?;
    }

    let mut insert_note = conn.prepare_cached(
        "INSERT INTO notes (post_id, position, type, blog_name, timestamp, text, tags)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for row in &rows.notes {
        insert_note.execute(params![
            id,
            row.position,
            row.note_type,
            row.blog_name,
            row.timestamp,
            row.text,
            row.tags,
        ])?;
    }
    Ok(())
}

fn user_version(conn: &Connection) -> rusqlite::Result<i32> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
}

/// `None` for a file with no schema yet (`user_version` 0), whose `meta` table
/// doesn't exist.
fn read_blog_name(conn: &Connection) -> anyhow::Result<Option<String>> {
    if user_version(conn)? == 0 {
        return Ok(None);
    }
    Ok(conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'blog_name'",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

/// Creates the whole schema in one transaction, `user_version` included, so a
/// crash can't leave a file that looks created but is missing tables.
fn create_schema(conn: &mut Connection, blog_name: &str) -> anyhow::Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(SCHEMA)?;
    tx.execute(
        "INSERT INTO meta (key, value)
         VALUES ('blog_name', ?1), ('created_at', ?2), ('updated_at', ?2)",
        params![blog_name, now_rfc3339()],
    )?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::notes::PostNotes;
    use crate::test_support::post_json;
    use crabrave::handlers::blog::Post;
    use serde_json::{Value, json};
    use std::collections::HashMap;

    /// A scratch directory removed on drop, so a failed assertion leaves
    /// nothing behind. Pid-suffixed so concurrent test binaries sharing the
    /// system temp dir can't collide on fixed names.
    struct TempDir(Utf8PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let tmp = std::env::temp_dir();
            let dir = Utf8Path::from_path(&tmp)
                .unwrap()
                .join(format!("{}-{name}", std::process::id()));
            let _ = fs_err::remove_dir_all(&dir);
            fs_err::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Utf8Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs_err::remove_dir_all(&self.0);
        }
    }

    fn post_with(id: &str, edit: impl FnOnce(&mut Value)) -> Post {
        let mut value = post_json(id);
        edit(&mut value);
        serde_json::from_value(value).unwrap()
    }

    fn text_post(id: &str, body: &str, tags: &[&str]) -> Post {
        post_with(id, |v| {
            v["content"] = json!([{"type": "text", "text": body}]);
            v["tags"] = json!(tags);
        })
    }

    fn write(
        db: &mut ArchiveDb,
        post: &Post,
        collection: Collection,
        source: RowSource,
    ) -> WriteOutcome {
        db.write_post(&PostRecord {
            post,
            notes: None,
            collection,
            file_path: None,
            url_map: &HashMap::new(),
            source,
        })
        .unwrap()
    }

    fn write_api(db: &mut ArchiveDb, post: &Post) -> WriteOutcome {
        write(db, post, Collection::Posts, RowSource::Api)
    }

    /// An independent connection, standing in for `sqlite3` reading the file.
    fn reader(dir: &Utf8Path) -> Connection {
        Connection::open(ArchiveDb::path(dir)).unwrap()
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    fn matches(conn: &Connection, query: &str) -> i64 {
        conn.query_row(
            "SELECT count(*) FROM posts_fts WHERE posts_fts MATCH ?1",
            [query],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn meta(conn: &Connection, key: &str) -> String {
        conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn new_database_has_schema_version_and_meta() {
        let dir = TempDir::new("archivr-test-db-new");
        let db = ArchiveDb::open(dir.path(), "test").unwrap();
        assert!(db.created());
        assert!(ArchiveDb::exists(dir.path()));

        let conn = reader(dir.path());
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(meta(&conn, "blog_name"), "test");
        for key in ["created_at", "updated_at"] {
            chrono::DateTime::parse_from_rfc3339(&meta(&conn, key)).unwrap();
        }
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn reopening_is_not_created() {
        let dir = TempDir::new("archivr-test-db-reopen");
        assert!(ArchiveDb::open(dir.path(), "test").unwrap().created());
        assert!(!ArchiveDb::open(dir.path(), "test").unwrap().created());
    }

    #[test]
    fn stored_blog_name_reads_without_creating_the_file() {
        let dir = TempDir::new("archivr-test-db-stored-name");
        assert_eq!(ArchiveDb::stored_blog_name(dir.path()).unwrap(), None);
        assert!(!ArchiveDb::exists(dir.path()));

        ArchiveDb::open(dir.path(), "test").unwrap();
        assert_eq!(
            ArchiveDb::stored_blog_name(dir.path()).unwrap().as_deref(),
            Some("test")
        );
    }

    #[test]
    fn newer_schema_version_is_rejected() {
        let dir = TempDir::new("archivr-test-db-newer");
        ArchiveDb::open(dir.path(), "test").unwrap();
        reader(dir.path())
            .pragma_update(None, "user_version", 2)
            .unwrap();

        let err = ArchiveDb::open(dir.path(), "test").unwrap_err();
        assert!(format!("{err:#}").contains("newer archivr"), "{err:#}");
    }

    #[test]
    fn blog_name_mismatch_is_rejected() {
        let dir = TempDir::new("archivr-test-db-mismatch");
        ArchiveDb::open(dir.path(), "a").unwrap();
        let err = ArchiveDb::open(dir.path(), "b").unwrap_err();
        assert!(format!("{err:#}").contains("is for blog `a`"), "{err:#}");
    }

    #[test]
    fn errors_name_the_database_file() {
        let dir = TempDir::new("archivr-test-db-error-context");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        // Another connection holds the write lock; with no busy timeout the
        // write fails at once instead of waiting out the real one.
        db.conn.busy_timeout(Duration::ZERO).unwrap();
        let holder = reader(dir.path());
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();

        let err = db
            .write_post(&PostRecord {
                post: &text_post("1", "alpha", &[]),
                notes: None,
                collection: Collection::Posts,
                file_path: None,
                url_map: &HashMap::new(),
                source: RowSource::Api,
            })
            .unwrap_err();

        let message = format!("{err:#}");
        let named = format!("archive.db at {}", ArchiveDb::path(dir.path()));
        assert!(message.contains(&named), "{message}");
        assert!(message.contains("locked"), "{message}");
    }

    #[test]
    fn blog_name_forms_are_accepted() {
        let dir = TempDir::new("archivr-test-db-forms");
        ArchiveDb::open(dir.path(), "algorithmist").unwrap();
        ArchiveDb::open(dir.path(), "Algorithmist.tumblr.com").unwrap();
    }

    #[test]
    fn uncommitted_writes_are_invisible_until_commit() {
        let dir = TempDir::new("archivr-test-db-uncommitted");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        write_api(&mut db, &text_post("1", "alpha", &[]));

        let conn = reader(dir.path());
        assert_eq!(count(&conn, "SELECT count(*) FROM posts"), 0);
        db.commit().unwrap();
        assert_eq!(count(&conn, "SELECT count(*) FROM posts"), 1);
        assert_eq!(db.post_count().unwrap(), 1);
    }

    #[test]
    fn dropping_with_an_open_transaction_discards_it() {
        let dir = TempDir::new("archivr-test-db-dropped");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        write_api(&mut db, &text_post("1", "alpha", &[]));
        drop(db);

        let db = ArchiveDb::open(dir.path(), "test").unwrap();
        assert_eq!(db.post_count().unwrap(), 0);
    }

    #[test]
    fn commit_without_writes_is_a_no_op() {
        let dir = TempDir::new("archivr-test-db-empty-commit");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        db.commit().unwrap();
        assert_eq!(db.post_count().unwrap(), 0);
    }

    #[test]
    fn rewrite_replaces_rows_and_fts() {
        let dir = TempDir::new("archivr-test-db-rewrite");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        write_api(&mut db, &text_post("1", "alpha", &["x", "y"]));
        db.commit().unwrap();
        write_api(&mut db, &text_post("1", "beta", &["z"]));
        db.commit().unwrap();

        let conn = reader(dir.path());
        assert_eq!(count(&conn, "SELECT count(*) FROM posts"), 1);
        let tags: Vec<String> = conn
            .prepare("SELECT tag FROM tags WHERE post_id = 1 ORDER BY position")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(tags, ["z"]);
        assert_eq!(matches(&conn, "alpha"), 0);
        assert_eq!(matches(&conn, "beta"), 1);
        assert_eq!(matches(&conn, "bet*"), 1);
        conn.execute(
            "INSERT INTO posts_fts(posts_fts) VALUES('integrity-check')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn fts_covers_trail_tags_and_authors() {
        let dir = TempDir::new("archivr-test-db-fts-columns");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        let post = post_with("1", |v| {
            v["tags"] = json!(["wanderlust"]);
            v["trail"] = json!([{
                "content": [{"type": "text", "text": "marmalade"}],
                "blog": {"name": "quokka"},
                "post": {"id": "9"},
            }]);
        });
        write_api(&mut db, &post);
        db.commit().unwrap();

        let conn = reader(dir.path());
        assert_eq!(matches(&conn, "marmalade"), 1);
        assert_eq!(matches(&conn, "wanderlust"), 1);
        assert_eq!(matches(&conn, "quokka"), 1);
    }

    #[test]
    fn writes_trail_media_and_note_rows() {
        let dir = TempDir::new("archivr-test-db-child-rows");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        let post = post_with("7", |v| {
            v["tags"] = json!(["one", "two"]);
            v["content"] = json!([{
                "type": "image",
                "alt_text": "a cat",
                "media": [{
                    "url": "https://cdn.example/a.jpg",
                    "type": "image/jpeg",
                    "width": 640,
                    "height": 480,
                    "has_original_dimensions": true,
                }],
            }]);
            v["reblogged_root_id"] = json!("9");
            v["trail"] = json!([{
                "content": [{"type": "text", "text": "root words"}],
                "blog": {"name": "quokka"},
                "post": {"id": "9"},
            }]);
        });
        let notes = PostNotes {
            total_notes: 1,
            notes: vec![
                serde_json::from_value(json!({
                    "type": "reply",
                    "timestamp": 100,
                    "blog_name": "replier",
                    "reply_text": "nice",
                }))
                .unwrap(),
            ],
            rollup_notes: Vec::new(),
        };
        let url_map = HashMap::from([(
            "https://cdn.example/a.jpg".to_owned(),
            "../media/images/a.jpg".to_owned(),
        )]);
        db.write_post(&PostRecord {
            post: &post,
            notes: Some(&notes),
            collection: Collection::Posts,
            file_path: Some("7.html"),
            url_map: &url_map,
            source: RowSource::Api,
        })
        .unwrap();
        db.commit().unwrap();

        let conn = reader(dir.path());
        let (file_path, source, is_reblog, authors): (String, String, i64, String) = conn
            .query_row(
                "SELECT file_path, source, is_reblog, authors FROM posts WHERE id = 7",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (
                file_path.as_str(),
                source.as_str(),
                is_reblog,
                authors.as_str()
            ),
            ("7.html", "api", 1, "quokka")
        );
        assert_eq!(
            count(&conn, "SELECT count(*) FROM tags WHERE post_id = 7"),
            2
        );

        let (blog, root_post, is_root, text): (String, String, i64, String) = conn
            .query_row(
                "SELECT blog, source_post_id, is_root, text FROM trail WHERE post_id = 7",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (blog.as_str(), root_post.as_str(), is_root, text.as_str()),
            ("quokka", "9", 1, "root words")
        );

        let media = conn
            .query_row(
                "SELECT kind, external, url, path, mime, width, height, alt_text, trail_position
                 FROM media WHERE post_id = 7",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, Option<i64>>(8)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            media,
            (
                "image".to_owned(),
                0,
                "https://cdn.example/a.jpg".to_owned(),
                "media/images/a.jpg".to_owned(),
                "image/jpeg".to_owned(),
                640,
                480,
                "a cat".to_owned(),
                None
            )
        );

        let note: (String, String, i64, String, Option<String>) = conn
            .query_row(
                "SELECT type, blog_name, timestamp, text, tags FROM notes WHERE post_id = 7",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            note,
            (
                "reply".to_owned(),
                "replier".to_owned(),
                100,
                "nice".to_owned(),
                None
            )
        );
    }

    #[test]
    fn import_row_never_replaces_api_row() {
        let dir = TempDir::new("archivr-test-db-api-wins");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        write_api(&mut db, &text_post("1", "from the api", &[]));
        let outcome = write(
            &mut db,
            &text_post("1", "from the files", &[]),
            Collection::Posts,
            RowSource::JsonImport,
        );
        db.commit().unwrap();

        assert_eq!(outcome, WriteOutcome::SkippedApiRow);
        let conn = reader(dir.path());
        let body: String = conn
            .query_row("SELECT body FROM posts WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(body, "from the api");
    }

    #[test]
    fn api_row_replaces_import_row() {
        let dir = TempDir::new("archivr-test-db-api-replaces");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        write(
            &mut db,
            &text_post("1", "from the files", &[]),
            Collection::Posts,
            RowSource::JsonImport,
        );
        let outcome = write_api(&mut db, &text_post("1", "from the api", &[]));
        db.commit().unwrap();

        assert_eq!(outcome, WriteOutcome::Written);
        let source: String = reader(dir.path())
            .query_row("SELECT source FROM posts WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(source, "api");
    }

    #[test]
    fn import_row_refreshes_import_row() {
        let dir = TempDir::new("archivr-test-db-import-refresh");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        let import = |db: &mut ArchiveDb, body: &str| {
            write(
                db,
                &text_post("1", body, &[]),
                Collection::Posts,
                RowSource::JsonImport,
            )
        };
        assert_eq!(import(&mut db, "first"), WriteOutcome::Written);
        assert_eq!(import(&mut db, "second"), WriteOutcome::Written);
        db.commit().unwrap();

        let body: String = reader(dir.path())
            .query_row("SELECT body FROM posts WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(body, "second");
    }

    #[test]
    fn special_collection_never_replaces_published_post() {
        let dir = TempDir::new("archivr-test-db-published-wins");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        let published_first = text_post("1", "alpha", &[]);
        let queued_first = text_post("2", "beta", &[]);

        let outcome = write(&mut db, &published_first, Collection::Posts, RowSource::Api);
        assert_eq!(outcome, WriteOutcome::Written);
        let outcome = write(&mut db, &published_first, Collection::Queue, RowSource::Api);
        assert_eq!(outcome, WriteOutcome::SkippedPublishedRow);

        let outcome = write(&mut db, &queued_first, Collection::Queue, RowSource::Api);
        assert_eq!(outcome, WriteOutcome::Written);
        let outcome = write(&mut db, &queued_first, Collection::Posts, RowSource::Api);
        assert_eq!(outcome, WriteOutcome::Written);
        db.commit().unwrap();

        let conn = reader(dir.path());
        let collection = |id: i64| -> String {
            conn.query_row("SELECT collection FROM posts WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .unwrap()
        };
        assert_eq!(collection(1), "posts");
        assert_eq!(collection(2), "posts");
    }

    #[test]
    fn precedence_is_independent_of_write_order() {
        let dir = TempDir::new("archivr-test-db-order-independent");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        let queued = |id: &str| text_post(id, "queued", &[]);
        let published = |id: &str| text_post(id, "published", &[]);

        // The API's queue row arrives first, then the import's published row.
        write(&mut db, &queued("1"), Collection::Queue, RowSource::Api);
        let outcome = write(
            &mut db,
            &published("1"),
            Collection::Posts,
            RowSource::JsonImport,
        );
        assert_eq!(outcome, WriteOutcome::Written);

        // The same two rows in the opposite order.
        write(
            &mut db,
            &published("2"),
            Collection::Posts,
            RowSource::JsonImport,
        );
        let outcome = write(&mut db, &queued("2"), Collection::Queue, RowSource::Api);
        assert_eq!(outcome, WriteOutcome::SkippedPublishedRow);
        db.commit().unwrap();

        let conn = reader(dir.path());
        for id in [1, 2] {
            let row: (String, String, String) = conn
                .query_row(
                    "SELECT collection, source, body FROM posts WHERE id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(
                row,
                (
                    "posts".to_owned(),
                    "json-import".to_owned(),
                    "published".to_owned()
                ),
                "post {id}"
            );
        }
    }

    #[test]
    fn special_collections_replace_each_other() {
        let dir = TempDir::new("archivr-test-db-latest-special");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        let post = text_post("1", "alpha", &[]);
        write(&mut db, &post, Collection::Drafts, RowSource::Api);
        let outcome = write(&mut db, &post, Collection::Queue, RowSource::Api);
        db.commit().unwrap();

        assert_eq!(outcome, WriteOutcome::Written);
        let collection: String = reader(dir.path())
            .query_row("SELECT collection FROM posts WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(collection, "queue");
    }

    #[test]
    fn large_id_round_trips() {
        let dir = TempDir::new("archivr-test-db-large-id");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        write_api(&mut db, &text_post("831737016835325952", "alpha", &[]));
        db.commit().unwrap();

        let id: i64 = reader(dir.path())
            .query_row("SELECT id FROM posts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(id, 831_737_016_835_325_952);
    }

    #[test]
    fn commit_succeeds_while_reader_holds_snapshot() {
        let dir = TempDir::new("archivr-test-db-reader-snapshot");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();

        let conn = reader(dir.path());
        conn.execute_batch("BEGIN").unwrap();
        assert_eq!(count(&conn, "SELECT count(*) FROM posts"), 0);

        write_api(&mut db, &text_post("1", "alpha", &[]));
        db.commit().unwrap();

        // The reader keeps its snapshot until it ends its own transaction.
        assert_eq!(count(&conn, "SELECT count(*) FROM posts"), 0);
        conn.execute_batch("COMMIT").unwrap();
        assert_eq!(count(&conn, "SELECT count(*) FROM posts"), 1);
    }

    #[test]
    fn skipped_import_keeps_the_api_rows_children() {
        let dir = TempDir::new("archivr-test-db-skip-untouched");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        write_api(&mut db, &text_post("1", "alpha", &["x"]));
        write(
            &mut db,
            &text_post("1", "beta", &["y"]),
            Collection::Posts,
            RowSource::JsonImport,
        );
        db.commit().unwrap();

        let tag: String = reader(dir.path())
            .query_row("SELECT tag FROM tags WHERE post_id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(tag, "x");
    }
}

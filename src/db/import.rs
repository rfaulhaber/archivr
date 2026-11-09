//! Builds or updates `archive.db` from an existing `--json` archive, with no
//! network access.

use std::collections::HashMap;

use anyhow::{Context as _, bail};
use camino::{Utf8Path, Utf8PathBuf};
use crabrave::handlers::blog::Post;
use serde_json::Value;

use super::rows::{Collection, PostRecord, RowSource, blog_names_match};
use super::{ArchiveDb, WriteOutcome};
use crate::api;
use crate::notes::PostNotes;

/// Files processed between commits: few enough that an interrupted import
/// loses little, many enough that a commit per post isn't paid.
pub const IMPORT_BATCH: u64 = 1000;

/// How many failures [`ImportSummary::failures`] keeps; a systematic problem
/// would otherwise list every file of a large archive.
pub const MAX_REPORTED_FAILURES: usize = 10;

/// What an import did with each post file it found.
#[derive(Debug, Default)]
pub struct ImportSummary {
    pub imported: u64,
    /// Posts left alone because `archive.db` already holds them from the API.
    pub skipped_api: u64,
    /// Draft, queued, submitted, or liked copies of posts the database already
    /// holds as published.
    pub skipped_published: u64,
    /// Files that couldn't be loaded, or whose post belongs to another blog or
    /// has an id that isn't a number.
    pub failed: u64,
    /// The first [`MAX_REPORTED_FAILURES`] failures, with the reason.
    pub failures: Vec<(Utf8PathBuf, String)>,
}

impl ImportSummary {
    fn record_failure(&mut self, path: &Utf8Path, reason: String) {
        self.failed += 1;
        if self.failures.len() < MAX_REPORTED_FAILURES {
            self.failures.push((path.to_owned(), reason));
        }
    }
}

/// Loads a post file written by `--json`: the serialized [`Post`], plus the
/// captured notes under a `notes` key when `--save-notes` was active.
pub fn load_post_file(path: &Utf8Path) -> anyhow::Result<(Post, Option<PostNotes>)> {
    let text = fs_err::read_to_string(path)?;
    let mut value: Value = serde_json::from_str(&text).context("invalid JSON")?;
    let object = value.as_object_mut().context("not a JSON object")?;

    let notes = object
        .remove("notes")
        .map(serde_json::from_value::<PostNotes>)
        .transpose()
        .context("invalid notes")?;

    // `Post` serializes its id as `id` but deserializes it from `id_string`,
    // so a file it wrote can't be read back as-is.
    if !object.contains_key("id_string")
        && let Some(id) = object.get("id").cloned()
    {
        object.insert("id_string".to_owned(), id);
    }
    let post = serde_json::from_value(value).context("not a post")?;
    Ok((post, notes))
}

/// The post files of a `--json` archive: `<id>.json` or `<id>/index.json` at
/// the root and in each special-collection subdirectory, sorted by path.
/// Everything else (media, dotfiles, index pages) is ignored.
pub fn find_post_files(dir: &Utf8Path) -> anyhow::Result<Vec<(Collection, Utf8PathBuf)>> {
    let mut files = Vec::new();
    scan_dir(dir, Collection::Posts, &mut files)?;
    for collection in Collection::SPECIAL {
        let Some(name) = collection.dir_name() else {
            continue;
        };
        let subdir = dir.join(name);
        if subdir.is_dir() {
            scan_dir(&subdir, collection, &mut files)?;
        }
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(files)
}

fn scan_dir(
    dir: &Utf8Path,
    collection: Collection,
    files: &mut Vec<(Collection, Utf8PathBuf)>,
) -> anyhow::Result<()> {
    for entry in fs_err::read_dir(dir)? {
        let name = entry?.file_name();
        let Some(name) = name.to_str() else { continue };
        let path = match name.strip_suffix(".json") {
            Some(stem) if is_post_id(stem) => dir.join(name),
            Some(_) => continue,
            None if is_post_id(name) => dir.join(name).join("index.json"),
            None => continue,
        };
        if path.is_file() {
            files.push((collection, path));
        }
    }
    Ok(())
}

fn is_post_id(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit())
}

/// Builds or updates `archive.db` in `dir` from the `--json` post files there.
///
/// Files are loaded one at a time, since a real archive can hold over a
/// hundred thousand. A file that won't load, a post of another blog, or a post
/// whose id isn't a number is counted in the summary instead of aborting the
/// run. A database error does abort it: SQLite may have rolled back the open
/// transaction itself.
pub fn import_json_archive(dir: &Utf8Path, quiet: bool) -> anyhow::Result<ImportSummary> {
    let files = find_post_files(dir)?;
    if files.is_empty() {
        return Err(no_archive_error(dir));
    }
    let blog_name = match ArchiveDb::stored_blog_name(dir)? {
        Some(name) => name,
        None => root_blog_name(dir, &files)?,
    };

    let mut db = ArchiveDb::open(dir, &blog_name)?;
    let total = u64::try_from(files.len()).unwrap_or(u64::MAX);
    let mut summary = ImportSummary::default();

    for ((collection, path), done) in files.iter().zip(1_u64..) {
        match load_post_file(path) {
            // A like is another blog's post by definition.
            Ok((post, _))
                if *collection != Collection::Likes
                    && !blog_names_match(&post.blog_name, &blog_name) =>
            {
                summary.record_failure(
                    path,
                    format!(
                        "post belongs to blog `{}`, not `{blog_name}`",
                        post.blog_name
                    ),
                );
            }
            // `write_post` would fail on this id, and its errors abort the
            // import, so it is counted here with the other bad files.
            Ok((post, _)) if post.id.parse::<i64>().is_err() => {
                summary.record_failure(
                    path,
                    format!("post id `{}` is not a 64-bit integer", post.id),
                );
            }
            Ok((post, notes)) => {
                let relative = path
                    .strip_prefix(dir)
                    .with_context(|| format!("{path} is not inside {dir}"))?;
                let outcome = db.write_post(&PostRecord {
                    post: &post,
                    notes: notes.as_ref(),
                    collection: *collection,
                    file_path: Some(relative.as_str()),
                    url_map: &HashMap::new(),
                    source: RowSource::JsonImport,
                })?;
                match outcome {
                    WriteOutcome::Written => summary.imported += 1,
                    WriteOutcome::SkippedApiRow => summary.skipped_api += 1,
                    WriteOutcome::SkippedPublishedRow => summary.skipped_published += 1,
                }
            }
            Err(err) => summary.record_failure(path, format!("{err:#}")),
        }

        if done % IMPORT_BATCH == 0 || done == total {
            db.commit()?;
            if !quiet {
                api::say(format_args!("  {done} / {total} files processed"))?;
            }
        }
    }
    Ok(summary)
}

/// The `blog_name` of the first root-level post file that loads. Only root
/// files name the archive's own blog: a like is another blog's post.
fn root_blog_name(dir: &Utf8Path, files: &[(Collection, Utf8PathBuf)]) -> anyhow::Result<String> {
    let mut root_files = 0_usize;
    let mut first_failure = None;
    for (_, path) in files
        .iter()
        .filter(|(collection, _)| *collection == Collection::Posts)
    {
        root_files += 1;
        match load_post_file(path) {
            Ok((post, _)) => return Ok(post.blog_name),
            Err(err) if first_failure.is_none() => first_failure = Some((path, err)),
            Err(_) => {}
        }
    }
    match first_failure {
        Some((path, err)) => bail!(
            "found {root_files} post files at the root of {dir}, but none could be read; \
             the first failed with: {path}: {err:#}"
        ),
        None => bail!(
            "found {} post files in {dir}, but none at its root, which is where the blog \
             name comes from",
            files.len()
        ),
    }
}

fn no_archive_error(dir: &Utf8Path) -> anyhow::Error {
    anyhow::anyhow!("no --json post files found in {dir}; is it a --json archive?")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::db::rows::{RowSource, build_rows};
    use crate::test_support::post_json;
    use rusqlite::Connection;
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

    fn text_post(id: &str, body: &str) -> Post {
        post_with(id, |v| {
            v["content"] = json!([{"type": "text", "text": body}]);
        })
    }

    /// Writes a post the way `render_json` does: the serialized post, plus a
    /// `notes` key when notes were captured.
    fn write_file(dir: &Utf8Path, relative: &str, post: &Post, notes: Option<&PostNotes>) {
        let mut value = serde_json::to_value(post).unwrap();
        if let Some(notes) = notes {
            value
                .as_object_mut()
                .unwrap()
                .insert("notes".to_owned(), serde_json::to_value(notes).unwrap());
        }
        write_text(
            dir,
            relative,
            &serde_json::to_string_pretty(&value).unwrap(),
        );
    }

    fn write_text(dir: &Utf8Path, relative: &str, text: &str) {
        let path = dir.join(relative);
        fs_err::create_dir_all(path.parent().unwrap()).unwrap();
        fs_err::write(&path, text).unwrap();
    }

    fn reader(dir: &Utf8Path) -> Connection {
        Connection::open(ArchiveDb::path(dir)).unwrap()
    }

    fn query_string(conn: &Connection, sql: &str) -> String {
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn load_post_file_patches_id_string() {
        let dir = TempDir::new("archivr-test-import-load-id");
        write_file(dir.path(), "123.json", &text_post("123", "hello"), None);
        let text = fs_err::read_to_string(dir.path().join("123.json")).unwrap();
        assert!(!text.contains("id_string"), "{text}");

        let (post, notes) = load_post_file(&dir.path().join("123.json")).unwrap();
        assert_eq!(post.id, "123");
        assert!(notes.is_none());
    }

    #[test]
    fn load_post_file_splits_notes() {
        let dir = TempDir::new("archivr-test-import-load-notes");
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
        write_file(dir.path(), "5.json", &text_post("5", "hello"), Some(&notes));

        let (post, loaded) = load_post_file(&dir.path().join("5.json")).unwrap();
        assert_eq!(post.id, "5");
        let loaded = loaded.unwrap();
        assert_eq!(loaded.total_notes, 1);
        assert_eq!(loaded.notes.len(), 1);
        assert_eq!(loaded.notes[0].reply_text.as_deref(), Some("nice"));
    }

    #[test]
    fn find_post_files_covers_layouts_and_collections() {
        let dir = TempDir::new("archivr-test-import-find");
        for relative in [
            "1.json",
            "2/index.json",
            "drafts/3.json",
            "likes/4/index.json",
            "media/images/x.jpg",
            ".archivr-state.json",
            "notes.txt",
            "abc.json",
            "6/notes.json",
        ] {
            write_text(dir.path(), relative, "{}");
        }

        let found: Vec<(Collection, String)> = find_post_files(dir.path())
            .unwrap()
            .into_iter()
            .map(|(collection, path)| {
                (
                    collection,
                    path.strip_prefix(dir.path()).unwrap().as_str().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                (Collection::Posts, "1.json".to_owned()),
                (Collection::Posts, "2/index.json".to_owned()),
                (Collection::Drafts, "drafts/3.json".to_owned()),
                (Collection::Likes, "likes/4/index.json".to_owned()),
            ]
        );
    }

    #[test]
    fn import_writes_rows_marked_json_import() {
        let dir = TempDir::new("archivr-test-import-rows");
        write_file(dir.path(), "1.json", &text_post("1", "alpha"), None);
        write_file(dir.path(), "2/index.json", &text_post("2", "beta"), None);

        let summary = import_json_archive(dir.path(), true).unwrap();
        assert_eq!(summary.imported, 2);
        assert_eq!(summary.failed, 0);

        let conn = reader(dir.path());
        for (id, file_path, body) in [(1, "1.json", "alpha"), (2, "2/index.json", "beta")] {
            let row: (String, String, String) = conn
                .query_row(
                    "SELECT source, file_path, body FROM posts WHERE id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(
                row,
                (
                    "json-import".to_owned(),
                    file_path.to_owned(),
                    body.to_owned()
                )
            );
        }
        assert_eq!(
            query_string(&conn, "SELECT value FROM meta WHERE key = 'blog_name'"),
            "test"
        );
    }

    #[test]
    fn import_keeps_api_rows() {
        let dir = TempDir::new("archivr-test-import-api");
        let mut db = ArchiveDb::open(dir.path(), "test").unwrap();
        db.write_post(&PostRecord {
            post: &text_post("1", "from the api"),
            notes: None,
            collection: Collection::Posts,
            file_path: Some("1.json"),
            url_map: &HashMap::new(),
            source: RowSource::Api,
        })
        .unwrap();
        db.commit().unwrap();
        drop(db);
        write_file(dir.path(), "1.json", &text_post("1", "from the file"), None);

        let summary = import_json_archive(dir.path(), true).unwrap();
        assert_eq!(summary.skipped_api, 1);
        assert_eq!(summary.imported, 0);

        let conn = reader(dir.path());
        assert_eq!(
            query_string(&conn, "SELECT body FROM posts WHERE id = 1"),
            "from the api"
        );
        assert_eq!(
            query_string(&conn, "SELECT source FROM posts WHERE id = 1"),
            "api"
        );
    }

    #[test]
    fn import_keeps_the_published_copy_over_other_collections() {
        let dir = TempDir::new("archivr-test-import-published");
        write_file(dir.path(), "3.json", &text_post("3", "published"), None);
        write_file(dir.path(), "drafts/3.json", &text_post("3", "draft"), None);

        let summary = import_json_archive(dir.path(), true).unwrap();
        assert_eq!(summary.imported, 1);
        assert_eq!(summary.skipped_published, 1);

        let conn = reader(dir.path());
        assert_eq!(
            query_string(&conn, "SELECT body FROM posts WHERE id = 3"),
            "published"
        );
    }

    #[test]
    fn import_is_idempotent() {
        let dir = TempDir::new("archivr-test-import-idempotent");
        write_file(dir.path(), "1.json", &text_post("1", "alpha"), None);
        write_file(dir.path(), "2.json", &text_post("2", "beta"), None);

        assert_eq!(import_json_archive(dir.path(), true).unwrap().imported, 2);
        assert_eq!(import_json_archive(dir.path(), true).unwrap().imported, 2);

        let db = ArchiveDb::open(dir.path(), "test").unwrap();
        assert_eq!(db.post_count().unwrap(), 2);
    }

    #[test]
    fn import_counts_bad_files_and_continues() {
        let dir = TempDir::new("archivr-test-import-bad");
        write_text(dir.path(), "9.json", "not json at all");
        write_file(dir.path(), "1.json", &text_post("1", "alpha"), None);

        let summary = import_json_archive(dir.path(), true).unwrap();
        assert_eq!(summary.imported, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.failures.len(), 1);
        assert!(summary.failures[0].0.as_str().ends_with("9.json"));
        assert!(!summary.failures[0].1.is_empty());
    }

    #[test]
    fn import_counts_a_post_whose_id_is_not_a_number() {
        let dir = TempDir::new("archivr-test-import-bad-id");
        write_file(dir.path(), "1.json", &text_post("1", "alpha"), None);
        // The file name is a valid id; the post inside it is not.
        write_file(dir.path(), "2.json", &text_post("abc", "beta"), None);

        let summary = import_json_archive(dir.path(), true).unwrap();
        assert_eq!(summary.imported, 1);
        assert_eq!(summary.failed, 1);
        assert!(summary.failures[0].0.as_str().ends_with("2.json"));
        assert!(
            summary.failures[0].1.contains("abc"),
            "{:?}",
            summary.failures
        );
        assert_eq!(
            ArchiveDb::open(dir.path(), "test")
                .unwrap()
                .post_count()
                .unwrap(),
            1
        );
    }

    #[test]
    fn import_reports_only_the_first_failures() {
        let dir = TempDir::new("archivr-test-import-failure-cap");
        write_file(dir.path(), "1.json", &text_post("1", "alpha"), None);
        let bad = MAX_REPORTED_FAILURES + 3;
        for id in 100..100 + bad {
            write_text(dir.path(), &format!("{id}.json"), "{");
        }

        let summary = import_json_archive(dir.path(), true).unwrap();
        assert_eq!(summary.failed, u64::try_from(bad).unwrap());
        assert_eq!(summary.failures.len(), MAX_REPORTED_FAILURES);
    }

    #[test]
    fn import_rejects_off_blog_posts_outside_likes() {
        let dir = TempDir::new("archivr-test-import-off-blog");
        write_file(dir.path(), "1.json", &text_post("1", "mine"), None);
        let other = |id: &str| post_with(id, |v| v["blog_name"] = json!("other"));
        write_file(dir.path(), "2.json", &other("2"), None);
        write_file(dir.path(), "drafts/4.json", &other("4"), None);
        write_file(dir.path(), "likes/3.json", &other("3"), None);

        let summary = import_json_archive(dir.path(), true).unwrap();
        assert_eq!(summary.imported, 2);
        assert_eq!(summary.failed, 2);
        let failed: Vec<&str> = summary.failures.iter().map(|(p, _)| p.as_str()).collect();
        assert!(failed[0].ends_with("2.json"), "{failed:?}");
        assert!(failed[1].ends_with("drafts/4.json"), "{failed:?}");

        let conn = reader(dir.path());
        assert_eq!(
            query_string(&conn, "SELECT collection FROM posts WHERE id = 3"),
            "likes"
        );
        assert_eq!(
            query_string(&conn, "SELECT blog_name FROM posts WHERE id = 3"),
            "other"
        );
    }

    #[test]
    fn import_takes_the_blog_name_from_the_first_loadable_root_file() {
        let dir = TempDir::new("archivr-test-import-blog-name");
        write_text(dir.path(), "1.json", "garbage");
        write_file(dir.path(), "2.json", &text_post("2", "alpha"), None);

        import_json_archive(dir.path(), true).unwrap();
        assert_eq!(
            ArchiveDb::stored_blog_name(dir.path()).unwrap().as_deref(),
            Some("test")
        );
    }

    #[test]
    fn import_records_local_media_paths() {
        let dir = TempDir::new("archivr-test-import-media");
        let with_image = |id: &str, url: &str| {
            post_with(id, |v| {
                v["content"] = json!([{
                    "type": "image",
                    "media": [{
                        "url": url,
                        "type": "image/jpeg",
                        "width": 640,
                        "height": 480,
                        "has_original_dimensions": true,
                    }],
                }]);
            })
        };
        write_file(
            dir.path(),
            "1.json",
            &with_image("1", "media/images/h_x.jpg"),
            None,
        );
        write_file(
            dir.path(),
            "2/index.json",
            &with_image("2", "../media/images/h_y.jpg"),
            None,
        );

        import_json_archive(dir.path(), true).unwrap();

        let conn = reader(dir.path());
        for (id, path) in [(1, "media/images/h_x.jpg"), (2, "media/images/h_y.jpg")] {
            let (stored_path, url): (Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT path, url FROM media WHERE post_id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(stored_path.as_deref(), Some(path));
            assert_eq!(url, None);
        }
    }

    /// Guards against the serialized `Post` drifting from what it deserializes
    /// from: a field that doesn't survive the round trip through a `--json`
    /// file would make an imported row differ from the row the backup writes.
    #[test]
    fn import_builds_the_same_rows_as_the_backup_sink() {
        let dir = TempDir::new("archivr-test-import-parity");
        let image = |url: &str| {
            json!({
                "type": "image",
                "alt_text": "a cat",
                "media": [{
                    "url": url,
                    "type": "image/jpeg",
                    "width": 640,
                    "height": 480,
                    "has_original_dimensions": true,
                }],
            })
        };
        let (remote, trail_remote) = (
            "https://64.media.tumblr.com/a.jpg",
            "https://64.media.tumblr.com/t.jpg",
        );
        let original = post_with("1", |v| {
            v["content"] = json!([{"type": "text", "text": "hello"}, image(remote)]);
            v["tags"] = json!(["cats", "Art"]);
            v["note_count"] = json!(7);
            v["slug"] = json!("a-slug");
            v["summary"] = json!("a summary");
            v["state"] = json!("published");
            v["original_type"] = json!("regular");
            v["short_url"] = json!("https://tmblr.co/x");
            v["parent_post_url"] = json!("https://origin.tumblr.com/post/100");
            v["reblogged_from_name"] = json!("origin");
            v["reblogged_from_url"] = json!("https://origin.tumblr.com/post/100");
            v["reblogged_root_id"] = json!("90");
            v["reblogged_root_name"] = json!("root");
            v["reblogged_root_url"] = json!("https://root.tumblr.com/post/90");
            v["trail"] = json!([
                {
                    "blog": {"name": "root"},
                    "post": {"id": "90"},
                    "content": [{"type": "text", "text": "first"}],
                },
                {
                    "blog": {"name": "origin"},
                    "post": {"id": "100"},
                    "content": [{"type": "text", "text": "second"}, image(trail_remote)],
                },
            ]);
        });
        let notes = PostNotes {
            total_notes: 2,
            notes: vec![
                serde_json::from_value(json!({
                    "type": "reply",
                    "timestamp": 100,
                    "blog_name": "replier",
                    "reply_text": "nice",
                }))
                .unwrap(),
                serde_json::from_value(json!({
                    "type": "reblog",
                    "timestamp": 200,
                    "blog_name": "reblogger",
                    "added_text": "agreed",
                    "tags": ["x", "y"],
                }))
                .unwrap(),
            ],
            rollup_notes: Vec::new(),
        };
        let url_map = HashMap::from([
            (remote.to_owned(), "media/images/h_a.jpg".to_owned()),
            (trail_remote.to_owned(), "media/images/h_t.jpg".to_owned()),
        ]);
        let written = crate::media::rewrite_post_media_urls(&original, &url_map);
        write_file(dir.path(), "1.json", &written, Some(&notes));

        let (loaded, loaded_notes) = load_post_file(&dir.path().join("1.json")).unwrap();
        let mut from_sink = build_rows(&PostRecord {
            post: &original,
            notes: Some(&notes),
            collection: Collection::Posts,
            file_path: Some("1.json"),
            url_map: &url_map,
            source: RowSource::Api,
        })
        .unwrap();
        let from_file = build_rows(&PostRecord {
            post: &loaded,
            notes: loaded_notes.as_ref(),
            collection: Collection::Posts,
            file_path: Some("1.json"),
            url_map: &HashMap::new(),
            source: RowSource::JsonImport,
        })
        .unwrap();

        // The fixture must exercise each table, or equality proves little.
        assert_eq!(from_file.tags.len(), 2);
        assert_eq!(from_file.trail.len(), 2);
        assert_eq!(from_file.notes.len(), 2);
        let paths: Vec<Option<&str>> = from_file.media.iter().map(|m| m.path.as_deref()).collect();
        assert_eq!(
            paths,
            [Some("media/images/h_a.jpg"), Some("media/images/h_t.jpg")]
        );

        // A file holds rewritten paths, so the original URL is unknowable, and
        // the source column says which path built the row.
        for media in &mut from_sink.media {
            media.url = None;
        }
        from_sink.post.source = RowSource::JsonImport;
        assert_eq!(from_file, from_sink);
    }

    #[test]
    fn import_stores_notes() {
        let dir = TempDir::new("archivr-test-import-notes");
        let notes = PostNotes {
            total_notes: 1,
            notes: vec![
                serde_json::from_value(json!({
                    "type": "like",
                    "timestamp": 100,
                    "blog_name": "fan",
                }))
                .unwrap(),
            ],
            rollup_notes: Vec::new(),
        };
        write_file(dir.path(), "1.json", &text_post("1", "alpha"), Some(&notes));

        import_json_archive(dir.path(), true).unwrap();

        let conn = reader(dir.path());
        assert_eq!(
            query_string(&conn, "SELECT blog_name FROM notes WHERE post_id = 1"),
            "fan"
        );
    }

    #[test]
    fn import_commits_in_batches() {
        let dir = TempDir::new("archivr-test-import-batches");
        let total = IMPORT_BATCH + 5;
        for id in 1..=total {
            write_file(
                dir.path(),
                &format!("{id}.json"),
                &text_post(&id.to_string(), "alpha"),
                None,
            );
        }

        let summary = import_json_archive(dir.path(), true).unwrap();
        assert_eq!(summary.imported, total);
        let db = ArchiveDb::open(dir.path(), "test").unwrap();
        assert_eq!(db.post_count().unwrap(), total);
    }

    #[test]
    fn import_of_empty_dir_errors() {
        let dir = TempDir::new("archivr-test-import-empty");
        let err = import_json_archive(dir.path(), true).unwrap_err();
        assert!(
            format!("{err:#}").contains("no --json post files found"),
            "{err:#}"
        );
        assert!(!ArchiveDb::exists(dir.path()));
    }

    #[test]
    fn import_errors_when_no_root_file_loads() {
        let dir = TempDir::new("archivr-test-import-unloadable");
        write_text(dir.path(), "1.json", "garbage");
        write_file(dir.path(), "likes/2.json", &text_post("2", "alpha"), None);

        let message = format!("{:#}", import_json_archive(dir.path(), true).unwrap_err());
        assert!(
            message.contains("found 1 post files at the root of"),
            "{message}"
        );
        assert!(message.contains("none could be read"), "{message}");
        assert!(message.contains("1.json: invalid JSON"), "{message}");
        assert!(!ArchiveDb::exists(dir.path()));
    }

    #[test]
    fn import_errors_when_only_collections_hold_post_files() {
        let dir = TempDir::new("archivr-test-import-no-root");
        write_file(dir.path(), "drafts/3.json", &text_post("3", "alpha"), None);

        let message = format!("{:#}", import_json_archive(dir.path(), true).unwrap_err());
        assert!(message.contains("none at its root"), "{message}");
        assert!(!message.contains("no --json post files"), "{message}");
        assert!(!ArchiveDb::exists(dir.path()));
    }
}

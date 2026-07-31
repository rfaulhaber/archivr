use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::{PostTimestamp, ResolvedConfig};

pub const JOB_FILE_PATH: &str = ".archivr-job.json";

#[derive(Debug, Serialize, Deserialize)]
pub struct JobState {
    pub blog_name: String,
    pub started_at: PostTimestamp,
    /// Pagination cursor: the next page holds posts older than this timestamp.
    /// `None` marks a job that has not fetched its first page yet — or a job
    /// file from a pre-cursor archivr version (those carried an `offset` field,
    /// which serde ignores here), in which case the run re-scans the window
    /// from the top and reuses already-written files.
    #[serde(default)]
    pub before_cursor: Option<PostTimestamp>,
    /// Page offset *within* the current `before_cursor` window; only nonzero
    /// while walking a run of posts that share one second-granularity
    /// timestamp, where the cursor alone cannot advance.
    #[serde(default)]
    pub window_offset: u64,
    /// Ids of the most recently committed page. The next window re-includes
    /// the boundary second by design; on `--resume` these seed the seen-set so
    /// already-written files are not rewritten with a severed nav chain.
    #[serde(default)]
    pub last_page_ids: Vec<String>,
    /// Relative href of the last HTML post written, so the first post of a
    /// resumed run can still link "newer" across the process boundary.
    #[serde(default)]
    pub last_post_href: Option<String>,
    /// Output file of the last HTML post written. Its "older" nav slot still
    /// holds the placeholder on an interrupted run; a resumed run finalizes it
    /// against the first post it writes.
    #[serde(default)]
    pub last_post_file: Option<Utf8PathBuf>,
    #[serde(default)]
    pub before: Option<PostTimestamp>,
    #[serde(default)]
    pub after: Option<PostTimestamp>,
    #[serde(default)]
    pub json: bool,
    #[serde(default)]
    pub directories: bool,
    #[serde(default)]
    pub save_images: bool,
    #[serde(default)]
    pub save_notes: bool,
    #[serde(default)]
    pub notes_limit: Option<u64>,
    #[serde(default)]
    pub drafts: bool,
    #[serde(default)]
    pub queue: bool,
    #[serde(default)]
    pub submissions: bool,
    #[serde(default)]
    pub likes: bool,
    #[serde(default)]
    pub include_tags: Vec<String>,
    #[serde(default)]
    pub post_type: Option<String>,
    #[serde(default)]
    pub no_reblogs: bool,
    #[serde(default)]
    pub only_reblogs: bool,
    #[serde(default)]
    pub save_video: bool,
    #[serde(default)]
    pub save_audio: bool,
    #[serde(default)]
    pub save_video_external: bool,
    #[serde(default)]
    pub index_file: bool,
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default)]
    pub skip: Option<u64>,
    #[serde(default)]
    pub save_avatar: bool,
    #[serde(default)]
    pub media_list: bool,
    #[serde(default)]
    pub wayback: bool,
    #[serde(default)]
    pub no_embeds: bool,
    /// Posts archived by this job so far, committed in step with the cursor,
    /// so a resumed `--count` run knows how much of its budget is spent.
    #[serde(default)]
    pub archived_count: u64,
    /// Posts skipped by this job so far (see `--skip`), committed in step
    /// with the cursor.
    #[serde(default)]
    pub skipped_count: u64,
    #[serde(default)]
    pub template_path: Option<Utf8PathBuf>,
}

impl JobState {
    pub fn new(config: &ResolvedConfig) -> Self {
        Self {
            blog_name: config.blog_name.clone(),
            started_at: chrono::Utc::now().timestamp(),
            before_cursor: None,
            window_offset: 0,
            last_page_ids: Vec::new(),
            last_post_href: None,
            last_post_file: None,
            before: config.before,
            after: config.after,
            json: config.json,
            directories: config.directories,
            save_images: config.save_images,
            save_notes: config.save_notes,
            notes_limit: config.notes_limit,
            drafts: config.drafts,
            queue: config.queue,
            submissions: config.submissions,
            likes: config.likes,
            include_tags: config.include_tags.clone(),
            post_type: config.post_type.clone(),
            no_reblogs: config.no_reblogs,
            only_reblogs: config.only_reblogs,
            save_video: config.save_video,
            save_audio: config.save_audio,
            save_video_external: config.save_video_external,
            index_file: config.index_file,
            count: config.count,
            skip: config.skip,
            save_avatar: config.save_avatar,
            media_list: config.media_list,
            wayback: config.wayback,
            no_embeds: config.no_embeds,
            archived_count: 0,
            skipped_count: 0,
            template_path: config.template_path.clone(),
        }
    }

    pub fn load(path: &Utf8Path) -> anyhow::Result<Self> {
        let content = fs_err::read_to_string(path)?;
        let state: Self = serde_json::from_str(&content)?;
        Ok(state)
    }

    pub fn save(&self, path: &Utf8Path) -> anyhow::Result<()> {
        let content = serde_json::to_string(self)?;
        crate::fsutil::write_atomic(path, content.as_bytes())?;
        Ok(())
    }

    pub fn delete(path: &Utf8Path) -> anyhow::Result<()> {
        fs_err::remove_file(path)?;
        Ok(())
    }

    pub fn job_file_path(output_dir: &Utf8Path) -> Utf8PathBuf {
        output_dir.join(JOB_FILE_PATH)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn sample_job() -> JobState {
        JobState {
            blog_name: "test-blog".to_owned(),
            started_at: 1700000000,
            before_cursor: Some(1699990000),
            window_offset: 40,
            last_page_ids: vec!["101".to_owned(), "100".to_owned()],
            last_post_href: Some("100.html".to_owned()),
            last_post_file: Some(Utf8PathBuf::from("/tmp/out/100.html")),
            before: Some(1700001000),
            after: Some(1699999000),
            json: true,
            directories: false,
            save_images: true,
            save_notes: true,
            notes_limit: Some(100),
            drafts: true,
            queue: false,
            submissions: false,
            likes: true,
            include_tags: vec!["art".to_owned()],
            post_type: Some("video".to_owned()),
            no_reblogs: true,
            only_reblogs: false,
            save_video: true,
            save_audio: false,
            save_video_external: false,
            index_file: true,
            count: Some(50),
            skip: Some(5),
            save_avatar: true,
            media_list: true,
            wayback: false,
            no_embeds: true,
            archived_count: 12,
            skipped_count: 5,
            template_path: Some(Utf8PathBuf::from("my-template.html")),
        }
    }

    #[test]
    fn serialization_round_trip() {
        let job = sample_job();
        let json = serde_json::to_string(&job).unwrap();
        let deserialized: JobState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.blog_name, "test-blog");
        assert_eq!(deserialized.started_at, 1700000000);
        assert_eq!(deserialized.before_cursor, Some(1699990000));
        assert_eq!(deserialized.window_offset, 40);
        assert_eq!(deserialized.last_page_ids, vec!["101", "100"]);
        assert_eq!(deserialized.last_post_href.as_deref(), Some("100.html"));
        assert_eq!(
            deserialized.last_post_file,
            Some(Utf8PathBuf::from("/tmp/out/100.html"))
        );
        assert_eq!(deserialized.before, Some(1700001000));
        assert_eq!(deserialized.after, Some(1699999000));
        assert!(deserialized.json);
        assert!(!deserialized.directories);
        assert!(deserialized.save_images);
        assert!(deserialized.save_notes);
        assert_eq!(deserialized.notes_limit, Some(100));
        assert!(deserialized.drafts);
        assert!(!deserialized.queue);
        assert!(!deserialized.submissions);
        assert!(deserialized.likes);
        assert_eq!(deserialized.include_tags, vec!["art"]);
        assert_eq!(deserialized.post_type.as_deref(), Some("video"));
        assert!(deserialized.no_reblogs);
        assert!(!deserialized.only_reblogs);
        assert!(deserialized.save_video);
        assert!(!deserialized.save_audio);
        assert!(!deserialized.save_video_external);
        assert!(deserialized.index_file);
        assert_eq!(deserialized.count, Some(50));
        assert_eq!(deserialized.skip, Some(5));
        assert!(deserialized.save_avatar);
        assert!(deserialized.media_list);
        assert!(!deserialized.wayback);
        assert!(deserialized.no_embeds);
        assert_eq!(deserialized.archived_count, 12);
        assert_eq!(deserialized.skipped_count, 5);
        assert_eq!(
            deserialized.template_path,
            Some(Utf8PathBuf::from("my-template.html"))
        );
    }

    #[test]
    fn deserialization_defaults() {
        // Minimal JSON with only the required fields — optional fields should default
        let json = r#"{"blog_name":"b","started_at":100}"#;
        let job: JobState = serde_json::from_str(json).unwrap();
        assert_eq!(job.blog_name, "b");
        assert_eq!(job.before_cursor, None);
        assert_eq!(job.window_offset, 0);
        assert!(job.last_page_ids.is_empty());
        assert_eq!(job.last_post_href, None);
        assert_eq!(job.last_post_file, None);
        assert_eq!(job.before, None);
        assert_eq!(job.after, None);
        assert!(!job.json);
        assert!(!job.directories);
        assert!(!job.save_images);
        assert!(!job.save_notes);
        assert_eq!(job.notes_limit, None);
        assert!(!job.drafts);
        assert!(!job.queue);
        assert!(!job.submissions);
        assert!(!job.likes);
        assert!(job.include_tags.is_empty());
        assert_eq!(job.post_type, None);
        assert!(!job.no_reblogs);
        assert!(!job.only_reblogs);
        assert!(!job.save_video);
        assert!(!job.save_audio);
        assert!(!job.save_video_external);
        assert!(!job.index_file);
        assert_eq!(job.count, None);
        assert_eq!(job.skip, None);
        assert!(!job.save_avatar);
        assert!(!job.media_list);
        assert!(!job.wayback);
        assert!(!job.no_embeds);
        assert_eq!(job.archived_count, 0);
        assert_eq!(job.skipped_count, 0);
        assert_eq!(job.template_path, None);
    }

    #[test]
    fn deserializes_legacy_offset_job_file() {
        // A job file written by a pre-cursor archivr carries `offset`; it must
        // still load (offset ignored, cursor defaults to None → window rescan).
        let json = r#"{"blog_name":"b","offset":120,"started_at":100,"json":true}"#;
        let job: JobState = serde_json::from_str(json).unwrap();
        assert_eq!(job.blog_name, "b");
        assert_eq!(job.started_at, 100);
        assert_eq!(job.before_cursor, None);
        assert_eq!(job.window_offset, 0);
        assert!(job.json);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir();
        let dir = Utf8Path::from_path(&dir).unwrap();
        let path = dir.join(format!("archivr-test-job-{}.json", std::process::id()));

        let job = sample_job();
        job.save(&path).unwrap();

        let loaded = JobState::load(&path).unwrap();
        assert_eq!(loaded.blog_name, job.blog_name);
        assert_eq!(loaded.before_cursor, job.before_cursor);
        assert_eq!(loaded.window_offset, job.window_offset);
        assert_eq!(loaded.before, job.before);
        assert_eq!(loaded.save_images, job.save_images);

        JobState::delete(&path).unwrap();
        assert!(!fs_err::exists(&path).unwrap_or(true));
    }

    #[test]
    fn job_file_path_joins_correctly() {
        let path = JobState::job_file_path(Utf8Path::new("/tmp/output"));
        assert_eq!(path.as_str(), "/tmp/output/.archivr-job.json");
    }
}

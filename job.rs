use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::{PostTimestamp, ResolvedConfig};

pub const JOB_FILE_PATH: &str = ".archivr-job.json";

#[derive(Debug, Serialize, Deserialize)]
pub struct JobState {
    pub blog_name: String,
    pub offset: u64,
    pub started_at: PostTimestamp,
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
    pub template_path: Option<Utf8PathBuf>,
}

impl JobState {
    pub fn new(config: &ResolvedConfig) -> Self {
        Self {
            blog_name: config.blog_name.clone(),
            offset: 0,
            started_at: chrono::Utc::now().timestamp(),
            before: config.before,
            after: config.after,
            json: config.json,
            directories: config.directories,
            save_images: config.save_images,
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
        fs_err::write(path, content)?;
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
            offset: 42,
            started_at: 1700000000,
            before: Some(1700001000),
            after: Some(1699999000),
            json: true,
            directories: false,
            save_images: true,
            template_path: Some(Utf8PathBuf::from("my-template.html")),
        }
    }

    #[test]
    fn serialization_round_trip() {
        let job = sample_job();
        let json = serde_json::to_string(&job).unwrap();
        let deserialized: JobState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.blog_name, "test-blog");
        assert_eq!(deserialized.offset, 42);
        assert_eq!(deserialized.started_at, 1700000000);
        assert_eq!(deserialized.before, Some(1700001000));
        assert_eq!(deserialized.after, Some(1699999000));
        assert!(deserialized.json);
        assert!(!deserialized.directories);
        assert!(deserialized.save_images);
        assert_eq!(
            deserialized.template_path,
            Some(Utf8PathBuf::from("my-template.html"))
        );
    }

    #[test]
    fn deserialization_defaults() {
        // Minimal JSON with only the required fields — optional fields should default
        let json = r#"{"blog_name":"b","offset":0,"started_at":100}"#;
        let job: JobState = serde_json::from_str(json).unwrap();
        assert_eq!(job.blog_name, "b");
        assert_eq!(job.before, None);
        assert_eq!(job.after, None);
        assert!(!job.json);
        assert!(!job.directories);
        assert!(!job.save_images);
        assert_eq!(job.template_path, None);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir();
        let dir = Utf8Path::from_path(&dir).unwrap();
        let path = dir.join("archivr-test-job.json");

        let job = sample_job();
        job.save(&path).unwrap();

        let loaded = JobState::load(&path).unwrap();
        assert_eq!(loaded.blog_name, job.blog_name);
        assert_eq!(loaded.offset, job.offset);
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

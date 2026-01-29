use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct JobState {
    pub blog_name: String,
    pub offset: u64,
    pub total_posts: Option<u64>,
    pub started_at: i64,
}

impl JobState {
    pub fn new(blog_name: &str) -> Self {
        Self {
            blog_name: blog_name.to_owned(),
            offset: 0,
            total_posts: None,
            started_at: chrono::Utc::now().timestamp(),
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
        output_dir.join(".archivr-job.json")
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LastRun {
    pub blog_name: String,
    pub newest_post_timestamp: i64,
    pub completed_at: i64,
}

impl LastRun {
    pub fn new(blog_name: &str, newest_post_timestamp: i64) -> Self {
        Self {
            blog_name: blog_name.to_owned(),
            newest_post_timestamp,
            completed_at: chrono::Utc::now().timestamp(),
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

    pub fn marker_path(output_dir: &Utf8Path) -> Utf8PathBuf {
        output_dir.join(".archivr-last-run.json")
    }
}

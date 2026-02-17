use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::PostTimestamp;

pub const JOB_FILE_PATH: &str = ".archivr-job.json";

#[derive(Debug, Serialize, Deserialize)]
pub struct JobState {
    pub blog_name: String,
    pub offset: u64,
    pub started_at: PostTimestamp,
}

impl JobState {
    pub fn new(blog_name: &str) -> Self {
        Self {
            blog_name: blog_name.to_owned(),
            offset: 0,
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
        output_dir.join(JOB_FILE_PATH)
    }
}

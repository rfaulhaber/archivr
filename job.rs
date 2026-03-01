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

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::PostTimestamp;

pub const STATE_FILE_PATH: &str = ".archivr-state.json";

/// Persistent record of the most recent successful backup. Unlike `JobState`,
/// this file survives across runs and is used by `--incremental` to compute
/// the lower bound for the next fetch.
#[derive(Debug, Serialize, Deserialize)]
pub struct BackupState {
    pub blog_name: String,
    pub last_run_at: PostTimestamp,
    pub newest_post_timestamp: PostTimestamp,
}

impl BackupState {
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

    pub fn state_file_path(output_dir: &Utf8Path) -> Utf8PathBuf {
        output_dir.join(STATE_FILE_PATH)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn sample_state() -> BackupState {
        BackupState {
            blog_name: "test-blog".to_owned(),
            last_run_at: 1_700_000_500,
            newest_post_timestamp: 1_700_000_000,
        }
    }

    #[test]
    fn serialization_round_trip() {
        let state = sample_state();
        let json = serde_json::to_string(&state).unwrap();
        let deserialized: BackupState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.blog_name, "test-blog");
        assert_eq!(deserialized.last_run_at, 1_700_000_500);
        assert_eq!(deserialized.newest_post_timestamp, 1_700_000_000);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir();
        let dir = Utf8Path::from_path(&dir).unwrap();
        let path = dir.join("archivr-test-state.json");

        let state = sample_state();
        state.save(&path).unwrap();

        let loaded = BackupState::load(&path).unwrap();
        assert_eq!(loaded.blog_name, state.blog_name);
        assert_eq!(loaded.last_run_at, state.last_run_at);
        assert_eq!(loaded.newest_post_timestamp, state.newest_post_timestamp);

        fs_err::remove_file(&path).unwrap();
    }

    #[test]
    fn state_file_path_joins_correctly() {
        let path = BackupState::state_file_path(Utf8Path::new("/tmp/output"));
        assert_eq!(path.as_str(), "/tmp/output/.archivr-state.json");
    }
}

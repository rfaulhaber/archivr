use camino::{Utf8Path, Utf8PathBuf};

pub const LOCK_FILE_PATH: &str = ".archivr-lock";

/// Writes `contents` to `dest` atomically: write a `.part` sibling first, then
/// rename it into place. A crash mid-write leaves at worst a stale `.part`
/// file, never a truncated `dest`, so the existence of `dest` always means the
/// write completed (the image downloader relies on this to skip re-downloads).
pub fn write_atomic(dest: &Utf8Path, contents: &[u8]) -> std::io::Result<()> {
    let tmp = Utf8PathBuf::from(format!("{dest}.part"));
    let result = fs_err::write(&tmp, contents).and_then(|()| fs_err::rename(&tmp, dest));
    if result.is_err() {
        // Best-effort cleanup; the write/rename error is the one worth surfacing.
        let _ = fs_err::remove_file(&tmp);
    }
    result
}

/// Sets a file's modification time to a Unix timestamp, so archived post
/// files sort naturally by post date. Timestamps before the epoch (Tumblr
/// allows arbitrary post dates) are clamped to the epoch rather than rejected.
pub fn set_mtime(path: &Utf8Path, unix_secs: i64) -> std::io::Result<()> {
    let secs = u64::try_from(unix_secs).unwrap_or(0);
    let mtime = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    let file = fs_err::OpenOptions::new().write(true).open(path)?;
    file.file().set_modified(mtime)
}

/// Advisory lock on an output directory, so two concurrent runs can't
/// interleave writes to the same job/state files. The kernel releases the
/// lock when the process exits by any route (including `process::exit` on
/// Ctrl-C), so a leftover `.archivr-lock` file is never stale — only the
/// lock itself matters.
pub struct DirLock {
    _file: fs_err::File,
}

pub fn lock_output_dir(output_dir: &Utf8Path) -> anyhow::Result<DirLock> {
    let path = output_dir.join(LOCK_FILE_PATH);
    let file = fs_err::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&path)?;
    match file.file().try_lock() {
        Ok(()) => Ok(DirLock { _file: file }),
        Err(std::fs::TryLockError::WouldBlock) => Err(anyhow::anyhow!(
            "another archivr instance is already backing up into {output_dir} \
             (the lock on {path} is held); wait for it to finish or use a \
             different output directory"
        )),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // Pid-suffixed so concurrent test binaries sharing the system temp dir
    // (e.g. two checkouts on one host) can't collide on fixed names.
    fn temp_path(name: &str) -> Utf8PathBuf {
        let dir = std::env::temp_dir();
        Utf8Path::from_path(&dir)
            .unwrap()
            .join(format!("{}-{name}", std::process::id()))
    }

    #[test]
    fn writes_contents() {
        let path = temp_path("archivr-test-atomic-write.txt");
        write_atomic(&path, b"hello").unwrap();
        assert_eq!(fs_err::read(&path).unwrap(), b"hello");
        fs_err::remove_file(&path).unwrap();
    }

    #[test]
    fn overwrites_existing_file() {
        let path = temp_path("archivr-test-atomic-overwrite.txt");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(fs_err::read(&path).unwrap(), b"second");
        fs_err::remove_file(&path).unwrap();
    }

    #[test]
    fn leaves_no_part_file_on_success() {
        let path = temp_path("archivr-test-atomic-clean.txt");
        write_atomic(&path, b"data").unwrap();
        let part = Utf8PathBuf::from(format!("{path}.part"));
        assert!(!fs_err::exists(&part).unwrap());
        fs_err::remove_file(&path).unwrap();
    }

    #[test]
    fn fails_without_destination_directory() {
        let path = temp_path("archivr-test-atomic-missing-dir/nested/file.txt");
        assert!(write_atomic(&path, b"data").is_err());
    }

    #[test]
    fn set_mtime_stamps_file() {
        let path = temp_path("archivr-test-mtime.txt");
        write_atomic(&path, b"x").unwrap();
        set_mtime(&path, 1_500_000_000).unwrap();
        let modified = fs_err::metadata(&path).unwrap().modified().unwrap();
        let secs = modified
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(secs, 1_500_000_000);
        fs_err::remove_file(&path).unwrap();
    }

    #[test]
    fn set_mtime_clamps_pre_epoch_timestamps() {
        let path = temp_path("archivr-test-mtime-neg.txt");
        write_atomic(&path, b"x").unwrap();
        set_mtime(&path, -1000).unwrap();
        fs_err::remove_file(&path).unwrap();
    }

    #[test]
    fn dir_lock_excludes_second_locker() {
        let dir = temp_path("archivr-test-lockdir");
        fs_err::create_dir_all(&dir).unwrap();
        let first = lock_output_dir(&dir).unwrap();
        // Same-process relock via a second handle must be refused.
        assert!(lock_output_dir(&dir).is_err());
        drop(first);
        let relock = lock_output_dir(&dir);
        assert!(relock.is_ok());
        drop(relock);
        fs_err::remove_dir_all(&dir).unwrap();
    }
}

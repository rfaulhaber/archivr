use camino::{Utf8Path, Utf8PathBuf};

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
}

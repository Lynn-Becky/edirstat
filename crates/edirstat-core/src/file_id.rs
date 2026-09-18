use std::fs;
use std::path::Path;

/// Platform-specific unique file identifier (device, inode/file-index).
///
/// Used for hardlink detection and cycle/device boundary checks. Falls back
/// to `(0, 0)` on platforms without a native identifier (e.g. wasm).
#[cfg(unix)]
#[must_use]
pub fn get_file_id(_path: &Path, meta: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt as _;

    (meta.dev(), meta.ino())
}

/// Platform-specific unique file identifier (device, inode/file-index).
///
/// Used for hardlink detection and cycle/device boundary checks.
///
/// Stable `Metadata` does not expose a file index on Windows, so the entry is
/// queried by path via `GetFileInformationByHandle` (through the safe
/// `winapi-util` wrapper). Falls back to `(0, 0)` if the query fails.
#[cfg(windows)]
#[must_use]
pub fn get_file_id(path: &Path, _meta: &fs::Metadata) -> (u64, u64) {
    winapi_util::Handle::from_path_any(path)
        .and_then(|h| winapi_util::file::information(&h))
        .map_or((0, 0), |info| {
            (info.volume_serial_number(), info.file_index())
        })
}

/// Platform-specific unique file identifier (device, inode/file-index).
///
/// Used for hardlink detection and cycle/device boundary checks. Falls back
/// to `(0, 0)` on platforms without a native identifier (e.g. wasm).
#[cfg(not(any(unix, windows)))]
#[must_use]
pub const fn get_file_id(_path: &Path, _meta: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}

/// File identifier for a directory entry encountered during traversal.
///
/// On Windows every path lookup costs an extra file open, so only
/// directories get a real ID there — they are the entries cycle and
/// device-boundary checks apply to. Files keep the `(0, 0)` fallback.
#[cfg(windows)]
#[must_use]
pub fn get_entry_file_id(entry: &fs::DirEntry, meta: &fs::Metadata) -> (u64, u64) {
    if meta.is_dir() {
        get_file_id(&entry.path(), meta)
    } else {
        (0, 0)
    }
}

/// File identifier for a directory entry encountered during traversal.
///
/// On non-Windows platforms the ID is read straight from `Metadata`, at no
/// extra cost, for files and directories alike.
#[cfg(not(windows))]
#[must_use]
pub fn get_entry_file_id(_entry: &fs::DirEntry, meta: &fs::Metadata) -> (u64, u64) {
    get_file_id(Path::new(""), meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_fixture_dir(name: &str) -> std::io::Result<std::path::PathBuf> {
        let dir = std::env::current_dir()?.join("target").join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    #[test]
    fn test_same_file_same_id() -> Result<(), crate::EdirstatError> {
        let dir = fresh_fixture_dir("file_id_test_same")?;
        let path = dir.join("a.txt");
        fs::write(&path, b"x")?;

        let id1 = get_file_id(&path, &fs::metadata(&path)?);
        let id2 = get_file_id(&path, &fs::metadata(&path)?);
        assert_eq!(id1, id2);

        fs::remove_dir_all(&dir)?;
        Ok(())
    }

    // The fallback impl returns (0, 0) for every file; identity checks are
    // only meaningful on platforms with a real native identifier.
    #[cfg(any(unix, windows))]
    #[test]
    fn test_different_files_different_ids() -> Result<(), crate::EdirstatError> {
        let dir = fresh_fixture_dir("file_id_test_different")?;
        let first_path = dir.join("a.txt");
        let second_path = dir.join("b.txt");
        fs::write(&first_path, b"a")?;
        fs::write(&second_path, b"b")?;

        let first_id = get_file_id(&first_path, &fs::metadata(&first_path)?);
        let second_id = get_file_id(&second_path, &fs::metadata(&second_path)?);
        assert_ne!(first_id, second_id);

        fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn test_hardlink_shares_id() -> Result<(), crate::EdirstatError> {
        let dir = fresh_fixture_dir("file_id_test_hardlink")?;
        let original = dir.join("orig.txt");
        let link = dir.join("link.txt");
        fs::write(&original, b"data")?;
        fs::hard_link(&original, &link)?;

        let original_id = get_file_id(&original, &fs::metadata(&original)?);
        let link_id = get_file_id(&link, &fs::metadata(&link)?);
        assert_eq!(original_id, link_id);

        fs::remove_dir_all(&dir)?;
        Ok(())
    }

    // Walk-time IDs on Windows are resolved for directories only: each lookup
    // costs an extra file open, and files never need one (cycle and
    // device-boundary checks only apply to directories).
    #[cfg(windows)]
    #[test]
    fn test_entry_file_id_windows_dirs_only() -> Result<(), crate::EdirstatError> {
        let dir = fresh_fixture_dir("file_id_test_entry_dirs_only")?;
        fs::create_dir(dir.join("sub"))?;
        fs::write(dir.join("f.txt"), b"x")?;

        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let meta = entry.metadata()?;
            let id = get_entry_file_id(&entry, &meta);
            if meta.is_dir() {
                assert_ne!(id, (0, 0));
            } else {
                assert_eq!(id, (0, 0));
            }
        }

        fs::remove_dir_all(&dir)?;
        Ok(())
    }
}

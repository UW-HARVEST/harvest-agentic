use std::{
    fs::{create_dir, metadata, read_dir, remove_dir_all, remove_file},
    io::{self, ErrorKind::NotFound},
    path::Path,
};
use thiserror::Error;

/// Sets up an empty, writable directory at the given path. Used to create the output directory and
/// diagnostics directory (if specified on the command line). If the directory:
/// 1. Does not exist: will create the directory.
/// 2. Exists and is empty: will verify the directory is writable.
/// 3. Exists and is nonempty: if `delete_contents` is true, the contents will be deleted. If
///    `delete_contents` is false, will return `Err(EmptyDirError::NonEmpty)`.
pub fn empty_writable_dir<P: AsRef<Path>>(
    path: P,
    delete_contents: bool,
) -> Result<(), EmptyDirError> {
    let entries = match read_dir(&path) {
        // The directory does not exist, so create it. We can skip the metadata check in this case
        // so return early.
        Err(error) if error.kind() == NotFound => return create_dir(path).map_err(From::from),
        Err(error) => return Err(error.into()),
        Ok(entries) => entries,
    };
    // The directory already exists. Iterate through its contents to:
    // 1. Return an error if delete_contents == false, or
    // 2. Delete the contents if delete-contents == true.
    for dir_entry in entries {
        if !delete_contents {
            return Err(EmptyDirError::NonEmpty);
        }
        let dir_entry = dir_entry?;
        match dir_entry.file_type()?.is_dir() {
            false => remove_file(dir_entry.path())?,
            true => remove_dir_all(dir_entry.path())?,
        }
    }
    // We made it through the contents check/deletion; verify the directory is writable.
    if metadata(path)?.permissions().readonly() {
        return Err(EmptyDirError::NotWritable);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum EmptyDirError {
    #[error("I/O error: {0}")]
    IoError(#[from] io::Error),
    #[error("directory not empty")]
    NonEmpty,
    #[error("directory not writable")]
    NotWritable,
}

/// Returns the version string for this build of HARVEST.
/// These environment variables are set in build.rs.
pub fn get_version() -> &'static str {
    concat!(env!("GIT_SHA"), " (", env!("GIT_DATE"), ")")
}

/// Returns the git revision of the checkout containing `dir`, in the same
/// format as [`get_version`]'s SHA (`<sha>` with a trailing `*` when the
/// working tree is dirty), or `None` when `dir` is not in a git checkout.
pub fn git_revision<P: AsRef<Path>>(dir: P) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir.as_ref())
        .args(["describe", "--always", "--abbrev=0", "--dirty=*"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let revision = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!revision.is_empty()).then_some(revision)
}

#[cfg(test)]
mod tests {
    #[cfg(not(miri))]
    #[test]
    fn empty_writable_dir_test() {
        use super::*;
        use crate::test_util::tempdir;
        use std::{fs::File, path::PathBuf};

        let tempdir = tempdir().unwrap();
        // Returns $tempdir/$name as a PathBuf.
        let subdir_path = |name: &str| PathBuf::from_iter([tempdir.path(), name.as_ref()]);
        // Counts the number of entries in the given subdirectory of tempdir (used to verify
        // whether empty_writable_dir returned an empty directory).
        let entry_count = |name| read_dir(subdir_path(name)).unwrap().count();
        // Test with nonexistent directories.
        assert!(empty_writable_dir(subdir_path("a"), false).is_ok());
        assert_eq!(entry_count("a"), 0);
        assert!(empty_writable_dir(subdir_path("b"), true).is_ok());
        assert_eq!(entry_count("b"), 0);
        // Test with existing empty directories (this just reuses the previous test).
        assert!(empty_writable_dir(subdir_path("a"), false).is_ok());
        assert_eq!(entry_count("a"), 0);
        assert!(empty_writable_dir(subdir_path("b"), true).is_ok());
        assert_eq!(entry_count("b"), 0);
        // Test with an existing non-empty directory.
        let a_file = PathBuf::from_iter([&*subdir_path("a"), "file.txt".as_ref()]);
        File::create_new(&a_file).unwrap();
        match empty_writable_dir(subdir_path("a"), false) {
            Err(EmptyDirError::NonEmpty) => assert_eq!(entry_count("a"), 1),
            other => panic!("unexpected return value: {other:?}"),
        }
        assert!(empty_writable_dir(subdir_path("a"), true).is_ok());
        assert_eq!(entry_count("a"), 0);
        // Test with a non-writable directory (this should fail the writability checks).
        #[cfg(unix)]
        {
            use std::{fs::DirBuilder, os::unix::fs::DirBuilderExt};
            DirBuilder::new()
                .mode(0o400)
                .create(subdir_path("c"))
                .unwrap();
            match empty_writable_dir(subdir_path("c"), false) {
                Err(EmptyDirError::NotWritable) => {}
                other => panic!("unexpected return value: {other:?}"),
            }
            match empty_writable_dir(subdir_path("c"), true) {
                Err(EmptyDirError::NotWritable) => {}
                other => panic!("unexpected return value: {other:?}"),
            }
        }
        // Test with an existing file rather than a directory. This case should error, even if
        // delete_contents is true (because the user probably didn't intend to point the output to
        // that path).
        File::create_new(subdir_path("d")).unwrap();
        match empty_writable_dir(subdir_path("d"), false) {
            Err(EmptyDirError::IoError(_)) => {}
            other => panic!("unexpected return value: {other:?}"),
        }
        match empty_writable_dir(subdir_path("d"), true) {
            Err(EmptyDirError::IoError(_)) => {}
            other => panic!("unexpected return value: {other:?}"),
        }
    }
}

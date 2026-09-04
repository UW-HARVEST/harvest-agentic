//! Filesystem abstractions. Used to represent directory trees, such as the input project or
//! lowered Rust source.
//!
//! # Freezing
//!
//! These types are read-only. To create them:
//!
//! 1. Create the file/directory/symlink in the diagnostic directory and populate it as intended.
//! 2. "freeze" the file using `Reporter::freeze_path` or `Scratch::freeze`.
//!
//! Freezing the contents will:
//! 1. Make the on-disk structures read-only. This applies recursively, but does not follow
//!    symlinks.
//! 2. Construct a [DirEntry] representing the on-disk structure.
//!
//! After you have frozen a filesystem object, it (and everything else frozen with it, if it is a
//! directory) must be left unchanged in the diagnostic directory. This is to avoid the need to
//! store the contents of files in memory.

mod dir;
mod file;
mod freezer;

use crate::utils::{EmptyDirError, empty_writable_dir};
use std::collections::{BTreeMap, btree_map};
use std::ffi::{OsStr, OsString};
use std::fs::Permissions;
use std::fs::ReadDir;
use std::fs::canonicalize;
use std::fs::read_dir;
use std::fs::read_link;
use std::fs::remove_file;
use std::fs::set_permissions;
use std::io::{self, ErrorKind};
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tempfile::{TempDir, tempdir};
use thiserror::Error;

pub use dir::{Dir, DirEntry, ResolvedEntry};
pub use file::{File, TextFile};
pub(crate) use freezer::Freezer;

/// Hashes a directory tree deterministically: SHA-256 over each file's
/// directory-relative path and contents, visited in sorted order. Symlinks are
/// hashed by their target path. Used to detect whether an agent modified a
/// read-only reference directory (see [`ReferenceGuard`]).
pub fn hash_dir(root: &Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read as _;

    fn collect(
        root: &Path,
        dir: &Path,
        out: &mut std::collections::BTreeMap<PathBuf, PathBuf>,
    ) -> io::Result<()> {
        for entry in read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                collect(root, &path, out)?;
            } else {
                let rel = path
                    .strip_prefix(root)
                    .expect("walked path is under root")
                    .to_path_buf();
                out.insert(rel, path);
            }
        }
        Ok(())
    }

    let mut files = std::collections::BTreeMap::new();
    collect(root, root, &mut files)?;

    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    for (rel, path) in files {
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update([0u8]);
        if path.is_symlink() {
            hasher.update(read_link(&path)?.to_string_lossy().as_bytes());
        } else {
            let mut f = std::fs::File::open(&path)?;
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
        }
        hasher.update([0u8]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Tracks the read-only reference directories an agentic tool hands to an
/// agent — the C source (`c_src/`) and, for the conform stage, the external
/// test suite.
pub struct ReferenceGuard {
    /// (entry name, hash at the time it was handed to the agent)
    entries: Vec<(String, String)>,
}

impl ReferenceGuard {
    /// Fingerprints each of `names` under `work_dir`, right after they were
    /// materialized. Names that do not exist are ignored.
    pub fn capture(work_dir: &Path, names: &[&str]) -> io::Result<Self> {
        let mut entries = Vec::new();
        for name in names {
            let path = work_dir.join(name);
            if path.is_dir() {
                entries.push(((*name).to_owned(), hash_dir(&path)?));
            }
        }
        Ok(ReferenceGuard { entries })
    }

    /// Re-checks each reference directory and removes it from `work_dir` so it
    /// is not frozen into the IR. A directory the agent modified is first
    /// preserved under `rejected_dir` (when one is configured) and reported.
    ///
    /// Returns the names of the modified references, for the caller to record.
    pub fn strip(&self, work_dir: &Path, rejected_dir: Option<&Path>) -> io::Result<Vec<String>> {
        let mut modified = Vec::new();
        for (name, original) in &self.entries {
            let path = work_dir.join(name);
            if !path.is_dir() {
                tracing::warn!("agent deleted the read-only reference {name}/");
                modified.push(name.clone());
                continue;
            }
            if hash_dir(&path)? != *original {
                tracing::warn!(
                    "agent modified the read-only reference {name}/ — the change is discarded; \
                     preserving a copy for review"
                );
                modified.push(name.clone());
                if let Some(rejected) = rejected_dir {
                    let dest = rejected.join(name);
                    if dest.exists() {
                        std::fs::remove_dir_all(&dest)?;
                    }
                    std::fs::create_dir_all(rejected)?;
                    crate::cargo_utils::copy_directory_recursive(&path, &dest)
                        .map_err(|e| io::Error::other(e.to_string()))?;
                }
            }
            std::fs::remove_dir_all(&path)?;
        }
        Ok(modified)
    }
}

/// Removes hidden entries (names starting with `.`) under `dir`, including
/// nested ones, except the git work record: `.git` itself and the
/// framework-written `.gitignore` stay. Agentic tools call this before freezing an agent's
/// working directory into the IR, so runtime artifacts like `.opencode/`
/// never enter a [RawDir].
pub fn remove_hidden_entries(dir: &Path) -> io::Result<()> {
    let Ok(entries) = read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == ".git" || name == ".gitignore" {
            continue;
        }
        if name.to_string_lossy().starts_with('.') {
            if let Err(e) = std::fs::remove_dir_all(&path) {
                if path.is_dir() {
                    tracing::warn!("Failed to remove hidden directory {}: {e}", path.display());
                } else if let Err(e2) = remove_file(&path) {
                    tracing::warn!("Failed to remove hidden entry {}: {e2}", path.display());
                }
            }
        } else if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            remove_hidden_entries(&path)?;
        }
    }
    Ok(())
}

pub const WORKTREE_DIR: &str = "wt";

/// Removes the leftover worktree-checkout directory.
pub fn remove_worktree_dir(dir: &Path) -> io::Result<()> {
    let worktrees = dir.join(WORKTREE_DIR);
    if worktrees.exists() {
        std::fs::remove_dir_all(&worktrees)?;
    }
    Ok(())
}

/// Collects symlink paths under `dir`, for diagnostics before a freeze
/// ([RawDir] has no symlink representation; freezes skip them with a warning).
pub fn collect_symlinks(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Ok(metadata) = entry.metadata() {
            if metadata.file_type().is_symlink() {
                out.push(path.display().to_string());
            } else if metadata.is_dir() {
                out.extend(collect_symlinks(&path));
            }
        }
    }
    out
}

/// Utility to recursively delete a TempDir that contains read-only files and directories. Provided
/// to make it easier to delete the diagnostics directory (note that [DiagnosticsDir] automatically
/// deletes the diagnostic directory on drop if it is a TempDir).
pub fn delete_ro_tempdir(tempdir: TempDir) -> io::Result<()> {
    fn delete_contents(path: &mut PathBuf) -> io::Result<()> {
        set_permissions(&path, Permissions::from_mode(0o700))?;
        for entry in read_dir(&path)? {
            let entry = entry?;
            path.push(entry.file_name());
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                delete_contents(path)?;
            } else {
                if !file_type.is_symlink() {
                    set_permissions(&path, Permissions::from_mode(0o200))?;
                }
                remove_file(&path)?;
            }
            path.pop();
        }
        Ok(())
    }
    let mut path = tempdir.path().into();
    delete_contents(&mut path)?;
    tempdir.close()
}

/// Owns the diagnostics directory (if it is a temporary directory) and stores useful information
/// about it.
#[derive(Debug)]
pub struct DiagnosticsDir {
    path: PathBuf,
    // Owns the directory if it is temporary, otherwise is None.
    tempdir: Option<TempDir>,
}

impl Drop for DiagnosticsDir {
    fn drop(&mut self) {
        if let Some(tempdir) = self.tempdir.take() {
            let path = tempdir.path().to_owned();
            if let Err(error) = delete_ro_tempdir(tempdir) {
                panic!(
                    "Diagnostic directory cleanup failed. Path: {}. Error: {error}",
                    path.display()
                );
            };
        };
    }
}

impl DiagnosticsDir {
    /// Creates a new DiagnosticDir in a temporary directory.
    #[cfg(all(not(miri), test))]
    pub(crate) fn tempdir() -> Result<DiagnosticsDir, DiagnosticsDirNewError> {
        DiagnosticsDir::new(None, false)
    }

    /// Creates the DiagnosticDir instance. `path` is diagnostic_dir from the configuration, and
    /// `force` is force from the configuration.
    pub(crate) fn new(
        path: Option<&Path>,
        force: bool,
    ) -> Result<DiagnosticsDir, DiagnosticsDirNewError> {
        // We canonicalize the diagnostics path because it will be used to construct paths that are
        // passed as to external commands (as command-line arguments), and the canonicalized path
        // is probably the most compatible representation.
        let (path, tempdir) = match path {
            None => {
                let tempdir = tempdir()?;
                (canonicalize(tempdir.path())?, Some(tempdir))
            }
            Some(path) => {
                empty_writable_dir(path, force)?;
                (canonicalize(path)?, None)
            }
        };
        Ok(DiagnosticsDir { path, tempdir })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Converts the given path, which must be relative to the diagnostic directory, into an
    /// absolute path.
    pub fn to_absolute_path<P: AsRef<Path>>(&self, relative: P) -> io::Result<PathBuf> {
        let relative = relative.as_ref();
        if !relative.is_relative() {
            return Err(ErrorKind::InvalidInput.into());
        }
        Ok(PathBuf::from_iter([&self.path, relative]))
    }
}

/// Error type returned by DiagnosticsDir::new.
#[derive(Debug, Error)]
pub enum DiagnosticsDirNewError {
    #[error("empty directory error")]
    EmptyDir(#[from] EmptyDirError),
    #[error("I/O error")]
    IoError(#[from] io::Error),
}

/// A symlink that has been frozen. Note that the thing it points to is not frozen; in fact it may
/// not exist or may be entirely outside the diagnostics directory.
#[derive(Clone, Debug)]
pub struct Symlink {
    // The path contained by this symlink.
    contents: Arc<Path>,
}

impl Symlink {
    /// Creates a new Symlink representing the file at the given path. This is for internal use by
    /// the diagnostics system; tool code should use [Reporter::freeze] or Scratch::freeze to
    /// create a symlink.
    fn new<P: AsRef<Path>>(path: P) -> io::Result<Symlink> {
        // Symlink permissions cannot be changed, so just create the Symlink object.
        Ok(Symlink {
            contents: read_link(path)?.into(),
        })
    }

    /// Returns this symlink's target path.
    pub fn contents(&self) -> &Path {
        &self.contents
    }

    /// Writes this symlink into the filesystem at the given path.
    pub fn write_rw<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        symlink(&self.contents, path)
    }

    /// Used by Freezer::copy_ro. Writes a read-only copy of this Symlink into the given path.
    fn copy_ro(&self, absolute: &Path) -> io::Result<()> {
        symlink(&self.contents, absolute)
    }
}

// TODO: Remove; RawEntry is being replaced by DirEntry.
/// A representation of a file-system directory entry.
#[derive(Clone, Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum RawEntry {
    Dir(RawDir),
    File(Vec<u8>),
}

impl RawEntry {
    fn dir(&self) -> Option<&RawDir> {
        match self {
            RawEntry::Dir(raw_dir) => Some(raw_dir),
            _ => None,
        }
    }

    fn file(&self) -> Option<&Vec<u8>> {
        match self {
            RawEntry::File(file) => Some(file),
            _ => None,
        }
    }
}

/// A representation of a file-system directory tree.
// TODO: Removed; RawDir is being replaced by Dir.
#[derive(Clone, Debug, Default)]
#[cfg_attr(test, derive(PartialEq))]
pub struct RawDir(BTreeMap<OsString, RawEntry>);

impl RawDir {
    /// Create a [RawDir] from a local file system directory
    ///
    /// Returns the [RawDir], number of directories and number of
    /// files, as a tuple.
    ///
    /// # Examples
    ///
    /// ```
    /// # use harvest_core::fs::RawDir;
    /// # #[cfg(miri)] fn main() {}
    /// # #[cfg(not(miri))]
    /// # fn main() -> std::io::Result<()> {
    /// # let dir = tempfile::tempdir().unwrap();
    /// # let path = dir.path();
    /// let (raw_dir, num_dirs, num_files) = RawDir::populate_from(std::fs::read_dir(path)?)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn populate_from(read_dir: ReadDir) -> std::io::Result<(Self, usize, usize)> {
        RawDir::populate_from_filtered(read_dir, &|_| false)
    }

    /// Like [`RawDir::populate_from`], but skips any directory entry (at any
    /// depth) whose file name satisfies `skip_dir`. Used for source ingestion
    /// to keep build-artifact directories (`build/`, `target/`, ...) out of
    /// the IR: they bloat the representation, can go stale (a pre-cleanup `build/`
    /// carries object files and a shared library that no longer match the
    /// source), and frequently contain symlinks (plain `populate_from` skips
    /// symlinks anywhere, with a warning).
    pub fn populate_from_filtered(
        read_dir: ReadDir,
        skip_dir: &dyn Fn(&std::ffi::OsStr) -> bool,
    ) -> std::io::Result<(Self, usize, usize)> {
        let mut directories = 0;
        let mut files = 0;
        let mut result = BTreeMap::default();
        for entry in read_dir {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                if skip_dir(&entry.file_name()) {
                    continue;
                }
                let (subdir, dirs, fs) =
                    RawDir::populate_from_filtered(std::fs::read_dir(entry.path())?, skip_dir)?;
                directories += dirs + 1;
                files += fs;
                result.insert(entry.file_name(), RawEntry::Dir(subdir));
            } else if metadata.is_file() {
                let contents = std::fs::read(entry.path())?;
                result.insert(entry.file_name(), RawEntry::File(contents));
                files += 1;
            } else {
                // Symlinks (and other special files) have no RawDir
                // representation, warn and skip them.
                tracing::warn!(
                    "skipping symlink/special entry: {}",
                    entry.path().display()
                );
            }
        }
        Ok((RawDir(result), directories, files))
    }

    /// Like [`RawDir::populate_from_filtered`], but additionally includes only
    /// the top-level entries (files or directories) whose name satisfies
    /// `keep_top`. Used to reconstruct a frozen `CargoPackage` from a snapshot
    /// program directory whose top level mixes package entries with sidecars
    /// (c_src/, gtest_suite/, results.*, ...): the manifest's `package_entries`
    /// list drives `keep_top`, and `skip_dir` still applies at every depth.
    pub fn populate_from_toplevel_selected(
        read_dir: ReadDir,
        keep_top: &dyn Fn(&OsStr) -> bool,
        skip_dir: &dyn Fn(&OsStr) -> bool,
    ) -> std::io::Result<(Self, usize, usize)> {
        let mut directories = 0;
        let mut files = 0;
        let mut result = BTreeMap::default();
        for entry in read_dir {
            let entry = entry?;
            if !keep_top(&entry.file_name()) {
                continue;
            }
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                if skip_dir(&entry.file_name()) {
                    continue;
                }
                let (subdir, dirs, fs) =
                    RawDir::populate_from_filtered(std::fs::read_dir(entry.path())?, skip_dir)?;
                directories += dirs + 1;
                files += fs;
                result.insert(entry.file_name(), RawEntry::Dir(subdir));
            } else if metadata.is_file() {
                let contents = std::fs::read(entry.path())?;
                result.insert(entry.file_name(), RawEntry::File(contents));
                files += 1;
            } else {
                return Err(std::io::Error::other(format!(
                    "snapshot package entry {:?} is a symlink; snapshots must be symlink-free",
                    entry.file_name()
                )));
            }
        }
        Ok((RawDir(result), directories, files))
    }

    /// Returns the names of this directory's top-level entries.
    pub fn toplevel_entries(&self) -> impl Iterator<Item = &OsStr> {
        self.0.keys().map(OsString::as_os_str)
    }

    /// Print a representation of the directory to standard out.
    ///
    /// # Arguments
    ///
    /// * `level` - The level of this directory relative to the
    ///   root. Used to add padding to before entry names.
    pub fn display(&self, level: usize, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pad = "  ".repeat(level);
        for (name, entry) in self
            .0
            .iter()
            .filter_map(|(name, entry)| entry.dir().map(|e| (name, e)))
        {
            writeln!(f, "{pad}{}", name.to_string_lossy())?;
            entry.display(1, f)?;
        }

        for (name, entry) in self
            .0
            .iter()
            .filter_map(|(name, entry)| entry.file().map(|e| (name, e)))
        {
            writeln!(f, "{pad}{} ({}B)", name.to_string_lossy(), entry.len())?;
        }
        Ok(())
    }

    /// Returns the path and contents of the files in this directory and its subdirectories. Paths
    /// are relative to this directory.
    pub fn files_recursive(&self) -> Vec<(PathBuf, &[u8])> {
        fn recurse<'s>(path: &mut PathBuf, dir: &'s RawDir, out: &mut Vec<(PathBuf, &'s [u8])>) {
            for (name, entry) in dir.0.iter() {
                match entry {
                    RawEntry::Dir(entry) => {
                        path.push(name);
                        recurse(path, entry, out);
                        path.pop();
                    }
                    RawEntry::File(contents) => {
                        let segments: [&Path; 2] = [path.as_ref(), name.as_ref()];
                        out.push((segments.iter().collect(), contents));
                    }
                }
            }
        }
        let mut out = vec![];
        recurse(&mut PathBuf::new(), self, &mut out);
        out
    }

    /// Gets the contents of a file at the given path. The file must
    /// exist. On success, returns a reference to file's contents.
    ///
    /// `path` must be a relative path. `..` is resolved lexically: it
    /// just removes the previously-specified directory (in general
    /// this isn't correct in the presence of symlinks, but `RawDir`
    /// does not support symlinks).
    pub fn get_file_mut<P: AsRef<Path>>(&mut self, path: P) -> Result<&mut Vec<u8>, GetFileError> {
        let (segments, file_name) = resolve_file_path(path.as_ref())?;

        let mut cur_dir = self;
        for component in segments {
            if let RawEntry::Dir(rd) = cur_dir
                .0
                .get_mut(component)
                .ok_or(GetFileError::DoesNotExist)?
            {
                cur_dir = rd;
            } else {
                return Err(GetFileError::UnderFile);
            }
        }
        if let RawEntry::File(v) = cur_dir
            .0
            .get_mut(file_name)
            .ok_or(GetFileError::DoesNotExist)?
        {
            Ok(v)
        } else {
            Err(GetFileError::Directory)
        }
    }

    /// Gets the contents of a file at the given path. The file must
    /// exist. On success, returns a reference to file's contents.
    ///
    /// `path` must be a relative path. `..` is resolved lexically: it
    /// just removes the previously-specified directory (in general
    /// this isn't correct in the presence of symlinks, but `RawDir`
    /// does not support symlinks).
    pub fn get_file<P: AsRef<Path>>(&self, path: P) -> Result<&[u8], GetFileError> {
        let (segments, file_name) = resolve_file_path(path.as_ref())?;

        let mut cur_dir = self;
        for component in segments {
            if let RawEntry::Dir(rd) = cur_dir.0.get(component).ok_or(GetFileError::DoesNotExist)? {
                cur_dir = rd;
            } else {
                return Err(GetFileError::UnderFile);
            }
        }
        if let RawEntry::File(v) = cur_dir.0.get(file_name).ok_or(GetFileError::DoesNotExist)? {
            Ok(v)
        } else {
            Err(GetFileError::Directory)
        }
    }

    /// Creates a new file at the given path. The file must not already exist. On success, returns
    /// a reference to the newly-added file.
    ///
    /// `path` must be a relative path. `..` is resolved lexically: it just removes the
    /// previously-specified directory (in general this isn't correct in the presence of symlinks,
    /// but `RawDir` does not support symlinks).
    pub fn set_file<P: AsRef<Path>>(
        &mut self,
        path: P,
        contents: Vec<u8>,
    ) -> Result<&mut Vec<u8>, SetFileError> {
        // Determine which directories we need to descend into to reach the file (handling normal
        // directory names as well as . and ..), and split out the file name.
        let mut segments = vec![];
        // Whether the most-recently-processed entry can be a file.
        let mut last_can_be_file = true;
        for component in path.as_ref().components() {
            last_can_be_file = match component {
                Component::CurDir => false,
                Component::Normal(name) => {
                    segments.push(name);
                    true
                }
                Component::ParentDir => {
                    if segments.pop().is_none() {
                        return Err(SetFileError::OutsideDir);
                    }
                    false
                }
                Component::Prefix(_) | Component::RootDir => {
                    return Err(SetFileError::AbsolutePath);
                }
            };
        }
        if !last_can_be_file {
            return Err(SetFileError::Directory);
        }
        let filename = match segments.pop() {
            None => return Err(SetFileError::EmptyFileName),
            Some(empty) if empty.is_empty() => return Err(SetFileError::EmptyFileName),
            Some(name) => name.into(),
        };

        // Traverse through the directory tree to find the file entry.
        let mut cur_dir = self;
        for dir_name in segments {
            let RawDir(map) = cur_dir;
            let new_dir = map
                .entry(dir_name.into())
                .or_insert_with(|| RawEntry::Dir(RawDir::default()));
            let RawEntry::Dir(new_dir) = new_dir else {
                return Err(SetFileError::UnderFile);
            };
            cur_dir = new_dir;
        }
        let btree_map::Entry::Vacant(entry) = cur_dir.0.entry(filename) else {
            return Err(SetFileError::AlreadyExists);
        };
        let RawEntry::File(out) = entry.insert(RawEntry::File(contents)) else {
            panic!("RawEntry::File stopped being a file");
        };
        Ok(out)
    }

    /// Materializes the [RawDir] to the file system.
    ///
    /// `path` is a path to an empty or non-existent directory noting
    /// where the file system should be materialized to.
    pub fn materialize<P: AsRef<Path>>(&self, base_path: P) -> std::io::Result<()> {
        let base_path = base_path.as_ref();
        for (file_path, contents) in self.files_recursive().iter() {
            let dir_path = if let Some(parent) = file_path.parent() {
                base_path.join(parent)
            } else {
                base_path.into()
            };
            std::fs::create_dir_all(dir_path)?;
            std::fs::write(base_path.join(file_path), contents)?;
        }
        Ok(())
    }
}

/// Error type returned by [RawDir::set_file].
// TODO: Remove when no longer needed.
#[derive(Debug, Eq, Hash, PartialEq, thiserror::Error)]
pub enum SetFileError {
    #[error("tried to set file at absolute path")]
    AbsolutePath,
    #[error("file already exists")]
    AlreadyExists,
    #[error("tried to set file at directory path")]
    Directory,
    #[error("empty file name")]
    EmptyFileName,
    #[error("tried to write file outside this directory")]
    OutsideDir,
    #[error("tried to set a file that is under another file")]
    UnderFile,
}

/// Error type returned by [RawDir::get_file].
#[derive(Debug, Eq, Hash, PartialEq, thiserror::Error)]
// TODO: Remove when no longer needed.
pub enum GetFileError {
    #[error("tried to get file at absolute path")]
    AbsolutePath,
    #[error("tried to get file at directory path")]
    Directory,
    #[error("tried to get a file outside this directory")]
    OutsideDir,
    #[error("tried to get a file that is under another file")]
    UnderFile,
    #[error("tried to get a file that does not exist")]
    DoesNotExist,
}

#[cfg(all(not(miri), test))]
mod test_util {
    use super::Dir;
    use std::collections::HashSet;

    /// Checks whether a Dir has entries with the given names
    pub fn dir_has_entries(dir: &Dir, names: &[&str]) -> bool {
        let entries: Result<HashSet<_>, _> = dir.entries().map(|(n, _)| n.into_string()).collect();
        let Ok(entries) = entries else { return false };
        let expected: HashSet<_> = names.iter().map(|&n| ToOwned::to_owned(n)).collect();
        entries == expected
    }
}

#[cfg(all(not(miri), test))]
mod tests {
    use super::*;
    use std::fs::{create_dir, read, write};

    /// Warning: If this test fails, check your /tmp. There's probably a bunch of leftover
    /// temporary directories there that you will need to manually delete. Sorry.
    #[test]
    fn delete_ro_tempdir_test() {
        let dir = tempdir().unwrap();
        // To verify that delete_ro_tempdir does not follow symlinks out of the directory it is
        // deleting, we create a *second* temporary directory and create symlinks into it.
        let other_dir = tempdir().unwrap();
        let other_file = PathBuf::from_iter([other_dir.path(), "other_file".as_ref()]);
        let other_subdir = PathBuf::from_iter([other_dir.path(), "other_subdir".as_ref()]);
        let other_symlink = PathBuf::from_iter([other_dir.path(), "other_symlink".as_ref()]);
        write(&other_file, "other_file_contents").unwrap();
        create_dir(&other_subdir).unwrap();
        symlink("other_file", &other_symlink).unwrap();
        // Populate dir/
        let subdir = PathBuf::from_iter([dir.path(), "subdir".as_ref()]);
        let subdir_file = PathBuf::from_iter([dir.path(), "subdir_file".as_ref()]);
        let subdir_symlink = PathBuf::from_iter([dir.path(), "subdir_symlink".as_ref()]);
        let file_symlink = PathBuf::from_iter([dir.path(), "file_symlink".as_ref()]);
        let dir_symlink = PathBuf::from_iter([dir.path(), "dir_symlink".as_ref()]);
        create_dir(&subdir).unwrap();
        write(&subdir_file, "subdir_file_contents").unwrap();
        symlink(other_symlink.canonicalize().unwrap(), &subdir_symlink).unwrap();
        symlink(other_file.canonicalize().unwrap(), &file_symlink).unwrap();
        symlink(other_subdir.canonicalize().unwrap(), &dir_symlink).unwrap();
        // Wipe out the permissions of everything under dir/ (except the symlinks, as changing the
        // permissions of a symlink changes its target's permissions).
        set_permissions(subdir_file, Permissions::from_mode(0o000)).unwrap();
        set_permissions(subdir, Permissions::from_mode(0o000)).unwrap();
        set_permissions(&dir, Permissions::from_mode(0o000)).unwrap();
        // Perform the deletion.
        delete_ro_tempdir(dir).unwrap();
        // Verify other_dir was unchanged.
        assert_eq!(read(other_file).unwrap(), b"other_file_contents");
        assert_eq!(read_dir(other_subdir).unwrap().count(), 0);
        assert_eq!(
            read_link(other_symlink).unwrap(),
            AsRef::<Path>::as_ref("other_file")
        );
    }

    #[test]
    fn reference_guard_detects_and_quarantines_edits() {
        let work = tempdir().unwrap();
        let rejected = tempdir().unwrap();
        create_dir(work.path().join("c_src")).unwrap();
        write(work.path().join("c_src/a.c"), "int x;").unwrap();
        create_dir(work.path().join("gtest_suite")).unwrap();
        write(work.path().join("gtest_suite/t.cc"), "TEST(A,B){}").unwrap();

        let guard = ReferenceGuard::capture(work.path(), &["c_src", "gtest_suite"]).unwrap();
        // The agent "fixes" the oracle and leaves the suite alone.
        write(work.path().join("c_src/a.c"), "int x = 1;").unwrap();

        let modified = guard.strip(work.path(), Some(rejected.path())).unwrap();
        assert_eq!(modified, ["c_src"]);
        // Both references are gone from the working directory...
        assert!(!work.path().join("c_src").exists());
        assert!(!work.path().join("gtest_suite").exists());
        // ...and only the edited one is preserved, with the agent's content.
        assert_eq!(
            read(rejected.path().join("c_src/a.c")).unwrap(),
            b"int x = 1;"
        );
        assert!(!rejected.path().join("gtest_suite").exists());
    }

    #[test]
    fn reference_guard_reports_a_deleted_reference() {
        let work = tempdir().unwrap();
        create_dir(work.path().join("c_src")).unwrap();
        write(work.path().join("c_src/a.c"), "int x;").unwrap();
        let guard = ReferenceGuard::capture(work.path(), &["c_src"]).unwrap();
        std::fs::remove_dir_all(work.path().join("c_src")).unwrap();
        assert_eq!(guard.strip(work.path(), None).unwrap(), ["c_src"]);
    }

    #[test]
    fn reference_guard_is_quiet_when_nothing_changed() {
        let work = tempdir().unwrap();
        let rejected = tempdir().unwrap();
        create_dir(work.path().join("c_src")).unwrap();
        write(work.path().join("c_src/a.c"), "int x;").unwrap();
        let guard = ReferenceGuard::capture(work.path(), &["c_src"]).unwrap();
        assert!(
            guard
                .strip(work.path(), Some(rejected.path()))
                .unwrap()
                .is_empty()
        );
        assert!(!work.path().join("c_src").exists());
        assert_eq!(read_dir(rejected.path()).unwrap().count(), 0);
    }

    #[test]
    fn files_recursive() {
        #[rustfmt::skip]
        let dir = RawDir([
            ("dir1".into(), RawEntry::Dir(RawDir([
                ("dir2".into(), RawEntry::Dir(RawDir([
                    ("file2.txt".into(), RawEntry::File(b"B".into())),
                ].into_iter().collect()))),
                ("file3.txt".into(), RawEntry::File(b"C".into())),
            ].into_iter().collect()))),
            ("file1.txt".into(), RawEntry::File(b"A".into())),
        ].into_iter().collect());
        // TODO: This comparison is sensitive to the order that files_recursive outputs its files,
        // which is not specified. We should either specify files_recursive's iteration order or
        // make this test insensitive to order.
        assert_eq!(
            dir.files_recursive(),
            [
                (PathBuf::from("dir1/dir2/file2.txt"), b"B".as_slice()),
                (PathBuf::from("dir1/file3.txt"), b"C".as_slice()),
                (PathBuf::from("file1.txt"), b"A".as_slice())
            ]
        );
    }

    #[test]
    fn set_file() {
        let mut root = RawDir::default();
        assert!(root.set_file("file1.txt", b"A".into()).is_ok());
        assert!(root.set_file("dir1/dir2/file2.txt", b"B".into()).is_ok());
        assert!(root.set_file("dir1/file3.txt", b"C".into()).is_ok());
        assert_eq!(
            root.set_file("/etc/passwd", b"D".into()),
            Err(SetFileError::AbsolutePath)
        );
        assert_eq!(
            root.set_file("dir1/file3.txt", b"E".into()),
            Err(SetFileError::AlreadyExists)
        );
        assert_eq!(
            root.set_file(".", b"F".into()),
            Err(SetFileError::Directory)
        );
        assert_eq!(
            root.set_file("dir1/dir2/..", b"G".into()),
            Err(SetFileError::Directory)
        );
        assert_eq!(
            root.set_file("", b"H".into()),
            Err(SetFileError::EmptyFileName)
        );
        assert_eq!(
            root.set_file("../", b"I".into()),
            Err(SetFileError::OutsideDir)
        );
        assert_eq!(
            root.set_file("dir1/../../", b"J".into()),
            Err(SetFileError::OutsideDir)
        );
        assert_eq!(
            root.set_file("file1.txt/file4.txt", b"K".into()),
            Err(SetFileError::UnderFile)
        );
        #[rustfmt::skip]
        assert_eq!(root, RawDir([
            ("dir1".into(), RawEntry::Dir(RawDir([
                ("dir2".into(), RawEntry::Dir(RawDir([
                    ("file2.txt".into(), RawEntry::File(b"B".into())),
                ].into_iter().collect()))),
                ("file3.txt".into(), RawEntry::File(b"C".into())),
            ].into_iter().collect()))),
            ("file1.txt".into(), RawEntry::File(b"A".into())),
        ].into_iter().collect()));
    }

    #[test]
    fn to_absolute_path() {
        // Test with a hard-coded diagnostic directory path so that this test isn't just
        // duplicating to_absolute_path's logic.
        let diagnostics_dir = DiagnosticsDir {
            path: "/diagnostics/directory/path".into(),
            tempdir: None,
        };
        assert_eq!(
            diagnostics_dir.to_absolute_path("a/b/c").unwrap().as_path(),
            "/diagnostics/directory/path/a/b/c"
        );
        let diagnostics_dir = DiagnosticsDir::tempdir().unwrap();
        let result = diagnostics_dir.to_absolute_path("/already/absolute");
        assert_eq!(result.unwrap_err().kind(), ErrorKind::InvalidInput);
        let result = diagnostics_dir.to_absolute_path("relative/path");
        assert_eq!(
            result.unwrap(),
            PathBuf::from_iter([diagnostics_dir.path(), "relative/path".as_ref()])
        );
    }

    #[test]
    fn populate_from_skips_symlinks() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root.join("file.txt"), "contents").unwrap();
        create_dir(root.join("sub")).unwrap();
        write(root.join("sub/inner.txt"), "inner").unwrap();
        symlink("file.txt", root.join("link_to_file")).unwrap();
        symlink("sub", root.join("link_to_dir")).unwrap();
        symlink("/nonexistent/target", root.join("broken_link")).unwrap();

        let (raw_dir, directories, files) =
            RawDir::populate_from(read_dir(root).unwrap()).unwrap();

        assert_eq!(directories, 1);
        assert_eq!(files, 2);
        let mut names: Vec<String> = raw_dir
            .toplevel_entries()
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["file.txt".to_owned(), "sub".to_owned()]);
    }
}

// Helper function that deconstructs a [Path] to a vector of directory
// components and a file component.
fn resolve_file_path(path: &Path) -> Result<(Vec<&OsStr>, &OsStr), GetFileError> {
    // Determine which directories we need to descend into to reach the file (handling normal
    // directory names as well as . and ..), and split out the file name.
    let mut segments = vec![];
    // Whether the most-recently-processed entry can be a file.
    let mut last_can_be_file = true;
    for component in path.components() {
        last_can_be_file = match component {
            Component::CurDir => false,
            Component::Normal(name) => {
                segments.push(name);
                true
            }
            Component::ParentDir => {
                if segments.pop().is_none() {
                    return Err(GetFileError::OutsideDir);
                }
                false
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(GetFileError::AbsolutePath);
            }
        };
    }
    if !last_can_be_file {
        return Err(GetFileError::Directory);
    }
    let file_name = match segments.pop() {
        None => return Err(GetFileError::DoesNotExist),
        Some(empty) if empty.is_empty() => return Err(GetFileError::DoesNotExist),
        Some(name) => name,
    };
    Ok((segments, file_name))
}

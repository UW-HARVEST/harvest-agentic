//! The directory of one agentic stage run (`case_dir`):
//!
//! ```text
//! /var/tmp/.tmpXXXX/      case_dir (in `large_temp_dir()`): removed after a successful run,
//!                         kept after a failed one
//!   inputs/               inputs from the framework (C source, prompt files, skills, suites), read-only
//!   shared/               large files that agents share and that git must not record (tab2rust builds)
//!   workspace/            the agent's working directory: the crate and the reports. Frozen.
//!     inputs, shared      symbolic links to the two directories above (absolute targets). Git
//!                         tracks them, so every git worktree of a sub-agent has them, wherever it is.
//!     scratch/            private temporary output (C builds). Ignored by git. Removed before the freeze.
//!   tmp/                  `TMPDIR` of the agent and of every command it runs (mode 0700)
//! ```
//!
//! The prompt files name every file relative to the checkout (the workspace or a worktree):
//! `inputs/c_src/...`, `shared/...`, `scratch/...`. The framework also keeps its own files in
//! case_dir (`logs/`, `agent_env/`, `.claude/`, `tmp/`). They go with case_dir. `inputs/` is made
//! read-only before the agent starts, and any change to it is reported
//! (`harvest_core::fs::ReferenceGuard`). The links are not frozen. The git record keeps them,
//! and the next stage points them at its own case_dir.
//!
//! In a dry-run, case_dir is not temporary: it is [`DRY_RUN_CASE`] in the output of the run, and
//! it stays there (see `dry_run`).

use std::fs::{self, read_dir};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use harvest_core::config::Config;
use harvest_core::fs::{
    RawDir, ReferenceGuard, collect_symlinks, remove_hidden_entries, remove_worktree_dir,
};
use harvest_core::stage_manifest::{BUILD_DIR, C_SOURCE_DIR};
use tracing::{info, warn};

pub const WORKSPACE: &str = "workspace";
/// In case_dir, and as a link in the workspace.
pub const INPUTS: &str = "inputs";
/// In case_dir, and as a link in the workspace.
pub const SHARED: &str = "shared";
/// Inside the workspace.
pub const SCRATCH: &str = "scratch";
/// In case_dir.
pub const TMP: &str = "tmp";
/// case_dir of a dry-run, in the output of the run.
pub(crate) const DRY_RUN_CASE: &str = "run_dir";

/// The free space that [`large_temp_dir`] must have before a run starts. The C and test builds
/// of a large project and the agent's temporary files take some GB, and the file system is
/// usually shared with `/`.
const MIN_FREE_BYTES: u64 = 4 << 30;

/// One stage run's case_dir. [`CaseDir::freeze`] (or [`CaseDir::finish`]) removes it after a
/// successful run. When the run fails (an error return or a panic), dropping it keeps the
/// directory for debugging and prints its path to stderr. With the C builds in `scratch/` and the
/// temporary files in `tmp/`, a failed run of a large project can leave some GB in
/// [`large_temp_dir`], and a reboot does not remove them (`/var/tmp` is on disk). A run that a
/// signal ends (Ctrl+C) also leaves its directory, without the message.
pub struct CaseDir {
    temp: Option<tempfile::TempDir>,
    root: PathBuf,
    /// A dry-run: the directory that keeps what the agent would receive. case_dir is its
    /// [`DRY_RUN_CASE`].
    dry_run: Option<PathBuf>,
}

impl CaseDir {
    /// Creates case_dir in [`large_temp_dir`] with empty `workspace/`, `inputs/`, `shared/` and
    /// `tmp/`. Fails when [`large_temp_dir`] has less than [`MIN_FREE_BYTES`] free.
    pub fn new() -> io::Result<Self> {
        let base = large_temp_dir();
        let free = free_bytes(&base)?;
        if free < MIN_FREE_BYTES {
            return Err(io::Error::other(format!(
                "{} has only {:.1} GiB free. Free some space until you have {} GiB available, or set TMPDIR \
                 to a directory with more space.",
                base.display(),
                free as f64 / (1u64 << 30) as f64,
                MIN_FREE_BYTES >> 30
            )));
        }
        let temp = tempfile::Builder::new().tempdir_in(base)?;
        let root = temp.path().to_path_buf();
        create_areas(&root)?;
        Ok(Self {
            temp: Some(temp),
            root,
            dry_run: None,
        })
    }

    /// case_dir of a stage of the run with `config`: [`CaseDir::new`], or for a dry-run the
    /// directory [`DRY_RUN_CASE`] in the output of the run. That one is not removed.
    pub fn for_run(config: &Config) -> io::Result<Self> {
        if !config.dry_run {
            return Self::new();
        }
        // The files of a run name case_dir by its absolute path.
        let dir = std::path::absolute(&config.output)?;
        let root = dir.join(DRY_RUN_CASE);
        fs::create_dir_all(&dir)
            .and_then(|()| fs::create_dir(&root))
            .and_then(|()| create_areas(&root))
            .map_err(|e| io::Error::other(format!("cannot create {}: {e}", root.display())))?;
        Ok(Self {
            temp: None,
            root,
            dry_run: Some(dir),
        })
    }

    /// The directory of a dry-run that keeps what the agent would receive. `None` for a run.
    pub(crate) fn dry_run(&self) -> Option<&Path> {
        self.dry_run.as_deref()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn workspace(&self) -> PathBuf {
        self.root.join(WORKSPACE)
    }

    pub fn inputs(&self) -> PathBuf {
        self.root.join(INPUTS)
    }

    pub fn shared(&self) -> PathBuf {
        self.root.join(SHARED)
    }

    /// The scratch directory of the main workspace. A sub-agent in a git worktree has its own.
    pub fn scratch(&self) -> PathBuf {
        self.workspace().join(SCRATCH)
    }

    /// The `TMPDIR` of the agent.
    pub fn tmp(&self) -> PathBuf {
        self.root.join(TMP)
    }

    pub fn c_src(&self) -> PathBuf {
        self.inputs().join(C_SOURCE_DIR)
    }

    /// `inputs/prompts/`: the prompt files of the run, at the path by which the agents name them
    /// ([`PROMPTS_PATH`](crate::PROMPTS_PATH)).
    pub fn prompts(&self) -> PathBuf {
        self.root.join(crate::PROMPTS_PATH)
    }

    /// Writes prompt files of a stage into `inputs/prompts/<stage>/`, each as it is.
    pub fn write_prompts(&self, stage: &str, files: &[(&str, &str)]) -> io::Result<()> {
        let dir = self.prompts().join(stage);
        fs::create_dir_all(&dir)?;
        for (name, text) in files {
            fs::write(dir.join(name), text)?;
        }
        Ok(())
    }

    /// Writes the skill `name` (a directory of `agent_skills/`) into `inputs/agent_skills/<name>/`.
    pub fn write_skill(&self, name: &str) -> io::Result<()> {
        crate::skills::materialize(name, &self.inputs().join("agent_skills").join(name))
    }

    /// Call it after the framework has written its inputs and before the agent starts: creates
    /// `workspace/scratch/` and the links `workspace/inputs` and `workspace/shared` (replacing the
    /// links of an earlier stage), removes the write permission from everything in `inputs/`,
    /// and fingerprints `inputs/` for [`CaseDir::freeze`].
    pub fn lock_inputs(&self) -> io::Result<ReferenceGuard> {
        fs::create_dir_all(self.scratch())?;
        for (name, target) in [(INPUTS, self.inputs()), (SHARED, self.shared())] {
            let link = self.workspace().join(name);
            if fs::symlink_metadata(&link).is_ok() {
                fs::remove_file(&link)?;
            }
            std::os::unix::fs::symlink(target, link)?;
        }
        set_writable(&self.inputs(), false)?;
        ReferenceGuard::capture(&self.inputs())
    }

    /// Commits the links `inputs` and `shared` in the git record of the workspace when they
    /// changed, so that each git worktree that an agent makes has them. Call it after the git
    /// record exists (`ensure_workdir_git`). A failure is logged: the main agent still has the
    /// links.
    pub fn commit_links(&self) {
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(self.workspace())
                .output()
        };
        let staged = git(&["add", "--", INPUTS, SHARED])
            .and_then(|_| git(&["diff", "--cached", "--quiet", "--", INPUTS, SHARED]));
        match staged {
            // Exit code 1: the staged links differ from HEAD.
            Ok(out) if out.status.code() == Some(1) => {
                let message = "framework: link inputs/ and shared/ of this run";
                match git(&["commit", "-q", "-m", message, "--", INPUTS, SHARED]) {
                    Ok(out) if out.status.success() => {}
                    Ok(out) => warn!(
                        "git commit of the links failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    ),
                    Err(e) => warn!("git commit of the links failed: {e}"),
                }
            }
            Ok(_) => {} // unchanged, or no git record
            Err(e) => warn!("git add of the links failed: {e}"),
        }
    }

    /// Freezes the workspace after a successful run and removes case_dir. Before the freeze it
    /// removes the hidden entries of the agent runtime (except the git work record), the links
    /// `inputs` and `shared`, `target/`, `scratch/` and the git worktrees, and reports changes to
    /// `inputs/` (one file each in `audit_dir`, when given). Tool-specific cleanup comes before
    /// this call.
    pub fn freeze(
        self,
        guard: &ReferenceGuard,
        audit_dir: Option<&Path>,
    ) -> Result<RawDir, Box<dyn std::error::Error>> {
        let workspace = self.workspace();
        remove_hidden_entries(&workspace)?;
        guard.audit(&self.inputs(), audit_dir)?;
        for name in [INPUTS, SHARED] {
            let link = workspace.join(name);
            if fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink()) {
                fs::remove_file(&link)?;
            }
        }
        for name in [BUILD_DIR, SCRATCH] {
            let dir = workspace.join(name);
            if dir.exists()
                && let Err(e) = fs::remove_dir_all(&dir)
            {
                warn!("Failed to remove {}: {e}", dir.display());
            }
        }
        if let Err(e) = remove_worktree_dir(&workspace) {
            warn!("Failed to remove leftover worktree dir: {e}");
        }
        for entry in collect_symlinks(&workspace) {
            warn!("workspace contains symlink: {entry}");
        }
        let (dir, directories, files) = RawDir::populate_from(read_dir(&workspace)?)?;
        info!("Produced CargoPackage with {directories} directories and {files} files");
        self.finish();
        Ok(dir)
    }

    /// Removes case_dir after a successful run.
    pub fn finish(mut self) {
        if let Some(temp) = self.temp.take() {
            let _ = set_writable(&self.inputs(), true);
            drop(temp); // removes the directory
        }
    }
}

impl Drop for CaseDir {
    fn drop(&mut self) {
        if let Some(temp) = self.temp.take() {
            // Not finished: the run failed. Keep the directory, writable, so that it can be removed by hand.
            let _ = set_writable(&self.inputs(), true);
            eprintln!(
                "[agent_runner] the run failed; its directory is kept for debugging: {}",
                temp.keep().display()
            );
        } else if self.dry_run.is_some() {
            // The directory of a dry-run stays. Writable, so that it can be removed.
            let _ = set_writable(&self.inputs(), true);
        }
    }
}

/// Creates the empty areas of the case_dir `root`.
fn create_areas(root: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    for area in [WORKSPACE, INPUTS, SHARED] {
        fs::create_dir(root.join(area))?;
    }
    fs::DirBuilder::new().mode(0o700).create(root.join(TMP))
}

/// The directory for large temporary files: `$TMPDIR` when it names a directory, else
/// `/var/tmp`. This is systemd's "temporary-large" path (`systemd-path temporary-large`).
/// `/tmp` is often a small tmpfs in memory with a per-user quota; `/var/tmp` is on disk.
/// systemd-tmpfiles removes entries of `/var/tmp` that are unused for 30 days.
pub fn large_temp_dir() -> PathBuf {
    match std::env::var_os("TMPDIR").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() && dir.is_dir() => dir,
        _ => PathBuf::from("/var/tmp"),
    }
}

/// The space that an unprivileged user can use in the file system of `path`. A per-user quota
/// (for example on a tmpfs `/tmp`) is not taken into account.
fn free_bytes(path: &Path) -> io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c_path` is a NUL-terminated string and `stat` points to writable memory.
    if unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: statvfs succeeded, so it filled `stat`.
    let stat = unsafe { stat.assume_init() };
    Ok(stat.f_bavail * stat.f_frsize)
}

/// Adds or removes the write permission of `path` and of everything below it. Symbolic links are
/// not followed.
fn set_writable(path: &Path, writable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() && writable {
        // A directory must be writable before its entries can be changed or removed.
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(meta.permissions().mode() | 0o200),
        )?;
    }
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            set_writable(&entry?.path(), writable)?;
        }
    }
    let mode = fs::symlink_metadata(path)?.permissions().mode();
    let mode = if writable {
        mode | 0o200
    } else {
        mode & !0o222
    };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(test)]
mod tests {
    use super::*;
    use harvest_core::config::Profile;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn locks_inputs_and_freezes_only_the_crate() {
        let case = CaseDir::new().unwrap();
        fs::create_dir_all(case.c_src().join("src")).unwrap();
        fs::write(case.c_src().join("src/a.c"), "int a;").unwrap();
        let guard = case.lock_inputs().unwrap();
        assert!(fs::write(case.c_src().join("src/b.c"), "").is_err());
        assert!(fs::write(case.c_src().join("src/a.c"), "").is_err());
        // The links make the inputs and shared/ reachable from the workspace.
        let ws = case.workspace();
        assert_eq!(
            fs::read_to_string(ws.join("inputs/c_src/src/a.c")).unwrap(),
            "int a;"
        );
        fs::write(ws.join("shared/build.o"), "").unwrap();
        assert!(case.shared().join("build.o").exists());
        fs::write(case.scratch().join("x.o"), "").unwrap();
        for dir in ["src", "target", ".opencode"] {
            fs::create_dir_all(ws.join(dir)).unwrap();
        }
        fs::write(ws.join("src/lib.rs"), "").unwrap();
        fs::write(ws.join("PLAN.md"), "").unwrap();
        let root = case.root().to_path_buf();
        let report = tempfile::tempdir().unwrap();
        let dir = case.freeze(&guard, Some(report.path())).unwrap();
        let mut names: Vec<String> = dir
            .toplevel_entries()
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["PLAN.md", "src"]);
        assert_eq!(fs::read_dir(report.path()).unwrap().count(), 0);
        assert!(!root.exists());
    }

    /// Two stages in a row: the links are in git, so a worktree has them; the next stage
    /// points them at its own case_dir and commits that.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn links_reach_worktrees_across_stages() {
        let first = CaseDir::new().unwrap();
        fs::create_dir_all(first.c_src()).unwrap();
        fs::write(first.c_src().join("a.c"), "int a;").unwrap();
        first.lock_inputs().unwrap();
        crate::ensure_workdir_git(&first.workspace(), Profile::Harvest);
        first.commit_links(); // already in the baseline commit: nothing to do
        let ws = first.workspace();
        assert!(git(&ws, &["status", "--porcelain"]).is_empty());
        assert!(git(&ws, &["ls-tree", "HEAD", "inputs"]).starts_with("120000"));
        git(&ws, &["worktree", "add", "-q", "wt/t1", "-b", "t1"]);
        assert_eq!(
            fs::read_to_string(ws.join("wt/t1/inputs/c_src/a.c")).unwrap(),
            "int a;"
        );

        // The next stage gets the frozen crate with its git record, without the links.
        let second = CaseDir::new().unwrap();
        fs::create_dir_all(second.c_src()).unwrap();
        fs::write(second.c_src().join("a.c"), "int a;").unwrap();
        harvest_core::cargo_utils::copy_directory_recursive(
            &ws.join(".git"),
            &second.workspace().join(".git"),
        )
        .unwrap();
        fs::write(
            second.workspace().join(".gitignore"),
            fs::read(ws.join(".gitignore")).unwrap(),
        )
        .unwrap();
        first.finish();
        second.lock_inputs().unwrap();
        crate::ensure_workdir_git(&second.workspace(), Profile::Harvest);
        second.commit_links();
        let ws = second.workspace();
        assert!(git(&ws, &["status", "--porcelain"]).is_empty());
        let target = git(&ws, &["cat-file", "-p", "HEAD:inputs"]);
        assert_eq!(target, second.inputs().to_string_lossy());
        git(&ws, &["worktree", "add", "-q", "wt/t2", "-b", "t2"]);
        assert!(ws.join("wt/t2/inputs/c_src/a.c").exists());
        second.finish();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn keeps_the_directory_when_not_finished() {
        let case = CaseDir::new().unwrap();
        fs::write(case.inputs().join("f"), "").unwrap();
        case.lock_inputs().unwrap();
        let root = case.root().to_path_buf();
        drop(case);
        assert!(root.join("inputs/f").exists());
        fs::remove_dir_all(&root).unwrap(); // writable again
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn the_directory_of_a_dry_run_stays_in_the_output() {
        let out = tempfile::tempdir().unwrap();
        let mut config = Config::mock();
        config.dry_run = true;
        config.output = out.path().join("program");
        let case = CaseDir::for_run(&config).unwrap();
        assert_eq!(case.dry_run(), Some(&*config.output));
        assert_eq!(case.root(), config.output.join(DRY_RUN_CASE));
        fs::write(case.inputs().join("f"), "").unwrap();
        let _guard = case.lock_inputs().unwrap();
        let input = case.inputs().join("f");
        assert!(fs::write(&input, "x").is_err());
        drop(case);
        // The directory stays, and a person can remove it.
        fs::write(&input, "x").unwrap();
        // It is the directory of one dry-run.
        let err = CaseDir::for_run(&config).err().unwrap().to_string();
        assert!(err.starts_with("cannot create "), "{err}");
        // A run has a temporary directory.
        config.dry_run = false;
        let case = CaseDir::for_run(&config).unwrap();
        assert_eq!(case.dry_run(), None);
        assert!(!case.root().starts_with(out.path()));
        case.finish();
    }

    #[test]
    fn the_prompt_files_are_in_the_inputs() {
        assert_eq!(crate::PROMPTS_PATH, format!("{INPUTS}/prompts"));
    }

    #[test]
    #[cfg_attr(miri, ignore)] // no statvfs shim
    fn measures_free_space() {
        assert!(free_bytes(Path::new("/")).unwrap() > 0);
        assert!(free_bytes(Path::new("/no/such/dir")).is_err());
    }
}

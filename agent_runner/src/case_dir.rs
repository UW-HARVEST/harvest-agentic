//! The directory of one agentic stage run (`case_dir`):
//!
//! ```text
//! /tmp/.tmpXXXX/          case_dir: removed after a successful run, kept after a failed one
//!   inputs/               inputs from the framework (C source, task files, skills, suites), read-only
//!   shared/               large files that agents share and that git must not record (tab2rust builds)
//!   workspace/            the agent's working directory: the crate and the reports. Frozen.
//!     inputs, shared      symbolic links to the two directories above (absolute targets). Git
//!                         tracks them, so every git worktree of a sub-agent has them, wherever it is.
//!     scratch/            private temporary output (C builds). Ignored by git. Removed before the freeze.
//! ```
//!
//! The prompts name every file relative to the checkout (the workspace or a worktree):
//! `inputs/c_src/...`, `shared/...`, `scratch/...`. The framework also keeps its own files in
//! case_dir (`logs/`, `agent_env/`, `.claude/`). They go with case_dir. `inputs/` is made
//! read-only before the agent starts, and any change to it is reported
//! (`harvest_core::fs::ReferenceGuard`). The links are not frozen. The git record keeps them,
//! and the next stage points them at its own case_dir.

use std::fs::{self, read_dir};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

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

const LAYOUT: &str = include_str!("case_layout.md");

/// One stage run's case_dir. [`CaseDir::freeze`] (or [`CaseDir::finish`]) removes it after a
/// successful run. When the run fails (an error return or a panic), dropping it keeps the
/// directory for debugging and prints its path to stderr. A failed run of a large project can
/// leave some MB in `/tmp`.
pub struct CaseDir {
    temp: Option<tempfile::TempDir>,
    root: PathBuf,
}

impl CaseDir {
    /// Creates case_dir with empty `workspace/`, `inputs/` and `shared/`.
    pub fn new() -> io::Result<Self> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().to_path_buf();
        for area in [WORKSPACE, INPUTS, SHARED] {
            fs::create_dir(root.join(area))?;
        }
        Ok(Self {
            temp: Some(temp),
            root,
        })
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

    pub fn c_src(&self) -> PathBuf {
        self.inputs().join(C_SOURCE_DIR)
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

    /// The prompt section that tells the agent the areas and their rules. Every main prompt has
    /// it as `{CASE_LAYOUT}`, and the agent copies it into each sub-agent prompt.
    pub fn layout(&self) -> &'static str {
        LAYOUT.trim_end()
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
        }
    }
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
    fn links_reach_worktrees_across_stages() {
        let first = CaseDir::new().unwrap();
        fs::create_dir_all(first.c_src()).unwrap();
        fs::write(first.c_src().join("a.c"), "int a;").unwrap();
        first.lock_inputs().unwrap();
        crate::ensure_workdir_git(&first.workspace());
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
        crate::ensure_workdir_git(&second.workspace());
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
    fn layout_has_no_placeholders() {
        let case = CaseDir::new().unwrap();
        let text = case.layout();
        assert!(text.contains("`inputs/c_src/`"));
        assert!(!text.contains('{'), "{text}");
        case.finish();
    }
}

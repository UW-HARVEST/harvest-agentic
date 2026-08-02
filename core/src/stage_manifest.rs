//! The stage manifest: the metadata sidecar that makes a translated program
//! directory a self-describing pipeline snapshot.
//!
//! The pipeline's cross-process exchange format is the materialized output
//! directory of a run (`out_root/<prog>/`). The manifest, written as
//! `harvest_stage.json` inside that directory, records:
//!
//! - which stages produced the snapshot, and with what agent/model/prompt mode
//!   (provenance for experiments);
//! - which bench test case the snapshot was translated from, plus a content
//!   hash of its `test_case/` C source (so a later stage-resumed run can
//!   locate the bench case and detect drift);
//! - the top-level entries that belong to the frozen `CargoPackage`
//!   (`package_entries`), so a loader can reconstruct the package without
//!   guessing which sidecar files (c_src/, gtest_suite/, results.*, ...) to
//!   exclude.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{AgentKind, Stage};

/// File name of the manifest inside a snapshot program directory.
pub const STAGE_MANIFEST_FILE: &str = "harvest_stage.json";

/// Metadata describing a pipeline snapshot (a translated program directory).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StageManifest {
    /// Manifest schema version, bumped on incompatible changes.
    pub schema_version: u32,

    /// All agentic stages that have produced this snapshot, in pipeline
    /// order, accumulated across runs (a verify-from-snapshot run appends
    /// `verify` to the input snapshot's list).
    pub stages: Vec<Stage>,

    /// Agent backend used for the most recent stage.
    pub agent: AgentKind,

    /// Model used for the most recent stage, if one was set explicitly.
    pub model: Option<String>,

    /// Prompt mode of the most recent stage: "plan", "no_plan",
    /// "no_plan_file", or "workflow".
    pub prompt_mode: String,

    /// Harvest version string (from `get_version()`) of the most recent run.
    pub harvest_version: String,

    /// The bench program directory (the directory containing `test_case/`)
    /// this snapshot was produced from, as passed on that run's command line.
    pub bench_program_dir: PathBuf,

    /// SHA-256 over the bench case's `test_case/` tree (relative paths and
    /// file contents), to detect a changed or mismatched bench case when a
    /// later run resumes from this snapshot.
    pub test_case_hash: String,

    /// Top-level entries of the frozen `CargoPackage` as materialized into
    /// this directory. Everything else at the top level is a sidecar.
    pub package_entries: Vec<String>,

    /// Unix timestamp of the run that wrote this manifest.
    pub created_unix: u64,
}

impl StageManifest {
    /// Reads the manifest from a snapshot program directory.
    pub fn read_from_dir(program_dir: &Path) -> std::io::Result<Self> {
        let path = program_dir.join(STAGE_MANIFEST_FILE);
        let contents = fs::read_to_string(&path)?;
        serde_json::from_str(&contents)
            .map_err(|e| std::io::Error::other(format!("{}: {e}", path.display())))
    }

    /// Writes the manifest into a snapshot program directory.
    pub fn write_to_dir(&self, program_dir: &Path) -> std::io::Result<()> {
        let path = program_dir.join(STAGE_MANIFEST_FILE);
        let contents = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        fs::write(path, contents)
    }
}

/// Hashes a directory tree deterministically: SHA-256 over each file's
/// directory-relative path and contents, visited in sorted order. Symlinks
/// are hashed by their target path.
pub fn hash_dir(root: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};

    fn collect(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, PathBuf>) -> std::io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
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

    let mut files = BTreeMap::new();
    collect(root, root, &mut files)?;

    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    for (rel, path) in files {
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update([0u8]);
        if path.is_symlink() {
            hasher.update(fs::read_link(&path)?.to_string_lossy().as_bytes());
        } else {
            let mut f = fs::File::open(&path)?;
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

#[cfg(all(not(miri), test))]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = StageManifest {
            schema_version: 1,
            stages: vec![Stage::Translate, Stage::Verify],
            agent: AgentKind::Claude,
            model: Some("sonnet".to_owned()),
            prompt_mode: "plan".to_owned(),
            harvest_version: "test".to_owned(),
            bench_program_dir: PathBuf::from("/bench/lz4"),
            test_case_hash: "sha256:abc".to_owned(),
            package_entries: vec!["Cargo.toml".to_owned(), "src".to_owned()],
            created_unix: 0,
        };
        manifest.write_to_dir(dir.path()).unwrap();
        let read = StageManifest::read_from_dir(dir.path()).unwrap();
        assert_eq!(read.stages, manifest.stages);
        assert_eq!(read.package_entries, manifest.package_entries);
        assert_eq!(read.test_case_hash, manifest.test_case_hash);
    }

    #[test]
    fn hash_dir_deterministic_and_sensitive() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("a.c"), "int main(){}").unwrap();
        fs::write(dir.path().join("sub/b.h"), "#define B 1").unwrap();
        let h1 = hash_dir(dir.path()).unwrap();
        let h2 = hash_dir(dir.path()).unwrap();
        assert_eq!(h1, h2);
        fs::write(dir.path().join("sub/b.h"), "#define B 2").unwrap();
        let h3 = hash_dir(dir.path()).unwrap();
        assert_ne!(h1, h3);
    }
}

//! Loads a benchmark test case's external test suite into the IR as an
//! [`ExternalTestSuite`].
//!
//! The suite is read **pristine from the bench program directory**, not from a
//! translation snapshot: the snapshot's copy is whatever the grader staged on
//! some earlier run and can be stale, whereas the bench directory is the
//! single source of truth that the grader itself uses.
//!
//! Configured under `[tools.load_test_suite]`:
//!
//! ```toml
//! [tools.load_test_suite]
//! input_path = "/path/to/harvest-bench/tests/lz4"   # bench program dir
//! harness = "gtest"                                 # optional; auto-detect when absent
//! ```
//!
//! `input_path` names the bench *program* directory (the one holding
//! `test_case/` alongside `gtest_suite/` or `runner/` + `test_vectors/`) — not
//! the `test_case/` subdirectory that the rest of the pipeline takes as its
//! C-source input.

use full_source::{ExternalTestSuite, TestSuiteKind};
use harvest_core::config::unknown_field_warning;
use harvest_core::fs::RawDir;
use harvest_core::tools::{RunContext, Tool};
use harvest_core::{Id, Representation};
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::read_dir;
use std::path::{Path, PathBuf};
use tracing::info;

/// Build-artifact directories to skip: a suite directory that was configured
/// or built in place would otherwise drag CMake caches and symlinks into the
/// IR.
fn is_build_artifact_dir(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name == "build" || name == "target" || name.starts_with("build-")
}

/// Tool-specific configuration, read from `[tools.load_test_suite]`.
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Bench program directory holding the external test suite.
    pub input_path: Option<PathBuf>,

    /// Force a harness kind instead of detecting it from the directory layout.
    pub harness: Option<TestSuiteKind>,

    #[serde(flatten)]
    unknown: HashMap<String, serde_json::Value>,
}

impl Config {
    fn validate(&self) {
        unknown_field_warning("tools.load_test_suite", &self.unknown);
    }
}

/// Reads `tools.load_test_suite.input_path` from a run's configuration.
/// Exposed so the pipeline can decide whether the suite is loadable before
/// building its tool graph.
pub fn configured_input_path(config: &harvest_core::config::Config) -> Option<PathBuf> {
    config
        .tools
        .get("load_test_suite")
        .and_then(|v| v.get("input_path"))
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
}

/// True when `dir` holds an external test suite of any recognized kind.
pub fn has_suite(dir: &Path) -> bool {
    detect_kind(dir, None).is_ok()
}

/// Determines which kind of external suite `dir` carries, honoring an explicit
/// override. Detection order matches the benchmark grader's `auto` mode:
/// GoogleTest first, then the cando2 runner, then plain vectors.
pub fn detect_kind(
    dir: &Path,
    forced: Option<TestSuiteKind>,
) -> Result<TestSuiteKind, Box<dyn std::error::Error>> {
    let present = |kind: TestSuiteKind| kind.dirs().iter().all(|d| dir.join(d).is_dir());
    if let Some(kind) = forced {
        if !present(kind) {
            return Err(format!(
                "test suite harness {kind} requested, but {} does not contain {}",
                dir.display(),
                kind.dirs().join(" + ")
            )
            .into());
        }
        return Ok(kind);
    }
    TestSuiteKind::ALL
        .into_iter()
        .find(|&k| present(k))
    .ok_or_else(|| {
        format!(
            "no external test suite found in {} (looked for gtest_suite/, runner/ + test_vectors/, test_vectors/)",
            dir.display()
        )
        .into()
    })
}

pub struct LoadTestSuite;

impl Tool for LoadTestSuite {
    fn name(&self) -> &'static str {
        "load_test_suite"
    }

    fn run(
        self: Box<Self>,
        context: RunContext,
        _inputs: Vec<Id>,
    ) -> Result<Box<dyn Representation>, Box<dyn std::error::Error>> {
        let default_config = serde_json::Value::Object(Default::default());
        let config = Config::deserialize(
            context
                .config
                .tools
                .get("load_test_suite")
                .unwrap_or(&default_config),
        )?;
        config.validate();

        let input_path = config.input_path.ok_or(
            "tools.load_test_suite.input_path is not set; the conform stage needs the bench \
             program directory that holds the external test suite",
        )?;
        let kind = detect_kind(&input_path, config.harness)?;

        // Load only the suite directories, as top-level entries.
        let wanted: Vec<&str> = kind.dirs().to_vec();
        let (dir, directories, files) = RawDir::populate_from_toplevel_selected(
            read_dir(&input_path)?,
            &|name| wanted.iter().any(|d| OsStr::new(d) == name),
            &is_build_artifact_dir,
        )?;
        info!(
            "Loaded {kind} external test suite from {} ({directories} directories, {files} files)",
            input_path.display()
        );
        Ok(Box::new(ExternalTestSuite { dir, kind }))
    }
}

#[cfg(all(not(miri), test))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detects_gtest_before_vectors() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("gtest_suite")).unwrap();
        fs::create_dir(dir.path().join("test_vectors")).unwrap();
        assert_eq!(detect_kind(dir.path(), None).unwrap(), TestSuiteKind::Gtest);
    }

    #[test]
    fn detects_lib_when_runner_present() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("runner")).unwrap();
        fs::create_dir(dir.path().join("test_vectors")).unwrap();
        assert_eq!(detect_kind(dir.path(), None).unwrap(), TestSuiteKind::Lib);
    }

    #[test]
    fn forced_kind_must_be_present() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("test_vectors")).unwrap();
        assert_eq!(
            detect_kind(dir.path(), Some(TestSuiteKind::Bin)).unwrap(),
            TestSuiteKind::Bin
        );
        let err = detect_kind(dir.path(), Some(TestSuiteKind::Gtest))
            .unwrap_err()
            .to_string();
        assert!(err.contains("gtest_suite"), "{err}");
    }

    #[test]
    fn no_suite_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("test_case")).unwrap();
        assert!(detect_kind(dir.path(), None).is_err());
    }
}

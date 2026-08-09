use std::path::Path;

use harvest_core::{Representation, fs::RawDir};
use serde::{Deserialize, Serialize};

/// A raw C project passed as input.
pub struct RawSource {
    pub dir: RawDir,
}

impl std::fmt::Display for RawSource {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        writeln!(f, "Raw C source:")?;
        self.dir.display(0, f)
    }
}

impl Representation for RawSource {
    fn name(&self) -> &'static str {
        "raw_source"
    }

    fn materialize(&self, path: &Path) -> std::io::Result<()> {
        self.dir.materialize(path)
    }
}

/// A cargo project representation (Cargo.toml, src/, etc).
#[derive(Clone)]
pub struct CargoPackage {
    pub dir: RawDir,
}

impl std::fmt::Display for CargoPackage {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        writeln!(f, "Cargo package:")?;
        self.dir.display(0, f)
    }
}

impl Representation for CargoPackage {
    fn name(&self) -> &'static str {
        "cargo_package"
    }

    fn materialize(&self, path: &Path) -> std::io::Result<()> {
        self.dir.materialize(path)
    }
}

/// Which external test harness a test case's suite is built around. Mirrors
/// the benchmark grader's harness selection: whatever the grader runs, the
/// conform agent must be told to reproduce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TestSuiteKind {
    /// GoogleTest suite (`gtest_suite/`, built via CMake against the cdylib).
    Gtest,
    /// cando2 library validation (`runner/` + `test_vectors/`).
    Lib,
    /// Executable validation (`driver` binary against `test_vectors/`).
    Bin,
}

impl TestSuiteKind {
    /// Every suite kind, in the benchmark grader's `auto` detection order:
    /// GoogleTest first, then the cando2 runner, then plain vectors.
    pub const ALL: [TestSuiteKind; 3] =
        [TestSuiteKind::Gtest, TestSuiteKind::Lib, TestSuiteKind::Bin];

    /// The top-level directory names that constitute a suite of this kind.
    /// Each is reserved by [`harvest_core::stage_manifest::is_reserved_toplevel`].
    pub const fn dirs(self) -> &'static [&'static str] {
        match self {
            TestSuiteKind::Gtest => &["gtest_suite"],
            TestSuiteKind::Lib => &["runner", "test_vectors"],
            TestSuiteKind::Bin => &["test_vectors"],
        }
    }
}

impl std::fmt::Display for TestSuiteKind {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            TestSuiteKind::Gtest => write!(f, "gtest"),
            TestSuiteKind::Lib => write!(f, "lib"),
            TestSuiteKind::Bin => write!(f, "bin"),
        }
    }
}

/// The external test suite of a benchmark test case, loaded pristine from the
/// bench program directory.
///
/// This representation exists so the third-round conform stage can be given
/// the suite through the IR rather than through directory conventions. It is
/// deliberately *not* an input of the verify stage: the external tests are
/// held out from rounds 1 and 2, and making the suite a distinct
/// representation lets the scheduler's dependency graph enforce that instead
/// of relying on which files happen to sit in an agent's working directory.
pub struct ExternalTestSuite {
    /// The suite directories, as top-level entries (see [`TestSuiteKind::dirs`]).
    pub dir: RawDir,
    pub kind: TestSuiteKind,
}

impl std::fmt::Display for ExternalTestSuite {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        writeln!(f, "External test suite ({}):", self.kind)?;
        self.dir.display(0, f)
    }
}

impl Representation for ExternalTestSuite {
    fn name(&self) -> &'static str {
        "external_test_suite"
    }

    fn materialize(&self, path: &Path) -> std::io::Result<()> {
        self.dir.materialize(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `stage_manifest::SUITE_DIR_NAMES` is the flat list core uses to carve
    /// suite directories out of a snapshot; it must stay in lockstep with the
    /// per-kind directory sets defined here.
    #[test]
    fn suite_dir_names_match_kind_dirs() {
        let mut from_kinds: Vec<&str> = TestSuiteKind::ALL
            .iter()
            .flat_map(|k| k.dirs().iter().copied())
            .collect();
        from_kinds.sort();
        from_kinds.dedup();
        let mut from_core = harvest_core::stage_manifest::SUITE_DIR_NAMES.to_vec();
        from_core.sort();
        assert_eq!(from_kinds, from_core);
    }
}

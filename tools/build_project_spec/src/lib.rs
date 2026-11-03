use std::fmt::Display;
use std::path::Path;

use full_source::{ExternalTestSuite, RawSource, TestSuiteKind};
use harvest_bench::{Dimension, MANIFEST_FILE, Manifest, ProgramSpec};
use harvest_core::Id;
use harvest_core::Representation;
use harvest_core::tools::{RunContext, Tool};

/// What a translation must produce. Two levels, by input format:
/// - DARPA test cases: `Library`, `Executable` or `Configurable`, guessed from the C source's
///   `CMakeLists.txt`.
/// - harvest-bench test cases (those with a GoogleTest suite): `HarvestBench`, carrying the
///   suite's own program declaration. HARVEST does not subdivide it: which artifacts the
///   program has (a library, an executable, or both) is harvest-bench's declaration.
pub enum ProjectKind {
    Library,
    Executable,
    Configurable,
    HarvestBench(ProgramSpec),
}

impl Display for ProjectKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectKind::Library => write!(f, "Library"),
            ProjectKind::Executable => write!(f, "Executable"),
            ProjectKind::Configurable => write!(f, "Configurable"),
            ProjectKind::HarvestBench(program) => {
                let dims: Vec<_> = [Dimension::Lib, Dimension::Bin]
                    .into_iter()
                    .filter(|&d| program.has(d))
                    .map(Dimension::label)
                    .collect();
                write!(f, "HarvestBench({})", dims.join("+"))
            }
        }
    }
}

pub struct ProjectSpec {
    pub kind: ProjectKind,
}

impl Display for ProjectSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProjectSpec(kind={})", self.kind)
    }
}

impl Representation for ProjectSpec {
    fn name(&self) -> &'static str {
        "project_spec"
    }
}

/// The DARPA kind, guessed from the top-level `CMakeLists.txt` content:
///
/// Examines the top-level `CMakeLists.txt` content to classify the project:
/// - `add_executable(` at line start -> Executable
/// - `add_library(` at line start -> Library
/// - `add_subdirectory(` at line start (with neither of the above) -> Configurable
///   (e.g. sphincs, where the real targets live in sub-CMakeLists)
fn kind_from_cmakelists(cmakelists: &[u8]) -> Option<ProjectKind> {
    let text = String::from_utf8_lossy(cmakelists);
    if text.lines().any(|l| l.starts_with("add_executable(")) {
        return Some(ProjectKind::Executable);
    }
    if text.lines().any(|l| l.starts_with("add_library(")) {
        return Some(ProjectKind::Library);
    }
    if text.lines().any(|l| l.starts_with("add_subdirectory(")) {
        return Some(ProjectKind::Configurable);
    }
    None
}

/// The program that a harvest-bench suite's `gtest_suite/manifest.json` declares, parsed with
/// harvest-bench's own types: a manifest that the runner would reject fails here, before any
/// translation. No manifest, or no `program` block, declares a library-only program.
fn harvest_bench_program(manifest: Option<&[u8]>) -> Result<ProgramSpec, serde_json::Error> {
    Ok(match manifest {
        Some(manifest) => serde_json::from_slice::<Manifest>(manifest)?.program(),
        None => ProgramSpec::library_only(),
    })
}

pub struct BuildProjectSpec;

impl Tool for BuildProjectSpec {
    fn name(&self) -> &'static str {
        "build_project_spec"
    }

    fn run(
        self: Box<Self>,
        context: RunContext,
        inputs: Vec<Id>,
    ) -> Result<Box<dyn Representation>, Box<dyn std::error::Error>> {
        let repr = context
            .ir_snapshot
            .get::<RawSource>(inputs[0])
            .ok_or("No RawSource representation found in IR")?;

        // The optional second input is the external test suite. A GoogleTest suite makes the
        // input a harvest-bench test case: only its program declaration is read, and only the
        // resulting kind is passed on.
        if let Some(&suite) = inputs.get(1) {
            let suite = context
                .ir_snapshot
                .get::<ExternalTestSuite>(suite)
                .ok_or("No ExternalTestSuite representation found in IR")?;
            if suite.kind == TestSuiteKind::Gtest {
                let path = Path::new(TestSuiteKind::Gtest.dirs()[0]).join(MANIFEST_FILE);
                let program = harvest_bench_program(suite.dir.get_file(path).ok())
                    .map_err(|e| format!("malformed suite manifest.json: {e}"))?;
                return Ok(Box::new(ProjectSpec {
                    kind: ProjectKind::HarvestBench(program),
                }));
            }
        }

        let cmakelists = repr.dir.get_file("CMakeLists.txt").map_err(
            |_| "Could not identify project kind from CMakeLists.txt (or could not find it)",
        )?;
        let kind = kind_from_cmakelists(cmakelists).ok_or(
            "CMakeLists.txt found but contains no add_executable, add_library, or add_subdirectory",
        )?;

        Ok(Box::new(ProjectSpec { kind }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(manifest: Option<&str>) -> String {
        let program = harvest_bench_program(manifest.map(str::as_bytes)).unwrap();
        ProjectKind::HarvestBench(program).to_string()
    }

    #[test]
    fn harvest_bench_program_is_the_suite_declaration() {
        let both = r#"{"program":{"lib":{"c_target":"toy"},"bin":{"name":"toy"}}}"#;
        assert_eq!(kind(Some(both)), "HarvestBench(lib+bin)");
        assert_eq!(
            kind(Some(r#"{"program":{"bin":{"name":"toy"}}}"#)),
            "HarvestBench(bin)"
        );
        // No program block, or no manifest: harvest-bench's default, a library.
        assert_eq!(kind(Some(r#"{"invocations":[]}"#)), "HarvestBench(lib)");
        assert_eq!(kind(None), "HarvestBench(lib)");
        // Parsed strictly, as the runner does.
        assert!(harvest_bench_program(Some(br#"{"version":1}"#)).is_err());
    }
}

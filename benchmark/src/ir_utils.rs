use crate::error::HarvestResult;
use harvest_core::fs::RawDir;
use harvest_core::HarvestIR;

use full_source::{CargoPackage, ExternalTestSuite, RawSource};
use try_cargo_build::{Artifact, CargoBuildResult};

/// Extract the final CargoPackage representation from the IR.
///
/// When multiple tools in a pipeline each produce a `CargoPackage` (e.g. `translate_agentic`
/// followed by `verify_fix_agentic`), the IR holds one per tool. Because `HarvestIR` is backed by
/// a `BTreeMap<Id, ...>` and `Id`s are monotonically increasing, the last entry is always the most
/// recently produced.
pub fn raw_cargo_package(ir: &HarvestIR) -> HarvestResult<&RawDir> {
    ir.get_by_representation::<CargoPackage>()
        .last()
        .map(|(_, r)| &r.dir)
        .ok_or_else(|| "No CargoPackage representation found in IR".into())
}

/// Extract a single RawSource representation from the IR.
/// Returns an error if there are 0 or multiple RawSource representations.
pub fn raw_source(ir: &HarvestIR) -> HarvestResult<&RawDir> {
    let raw_sources: Vec<&RawDir> = ir
        .get_by_representation::<RawSource>()
        .map(|(_, r)| &r.dir)
        .collect();

    match raw_sources.len() {
        0 => Err("No RawSource representation found in IR".into()),
        1 => Ok(raw_sources[0]),
        n => Err(format!("Found {} RawSource representations, expected at most 1", n).into()),
    }
}

/// Extract the external test suite from the IR, if the run loaded one. Absent
/// for test cases that ship no external suite.
pub fn external_test_suite(ir: &HarvestIR) -> Option<&ExternalTestSuite> {
    ir.get_by_representation::<ExternalTestSuite>()
        .last()
        .map(|(_, r)| r)
}

/// Extract cargo build results from the IR.
/// Returns the build artifacts, or an error if the build failed or results are missing/ambiguous.
pub fn cargo_build_result(ir: &HarvestIR) -> Result<&Vec<Artifact>, String> {
    let build_results: Vec<_> = ir.get_by_representation::<CargoBuildResult>().collect();

    match build_results.len() {
        0 => Err("No artifacts built".into()),
        1 => {
            let (_, r) = build_results[0];
            if !r.success {
                Err(format!("cargo build failed:\n{}", r.err))
            } else {
                Ok(&r.artifacts)
            }
        }
        n => Err(format!("Found {} build results, expected at most 1", n)),
    }
}

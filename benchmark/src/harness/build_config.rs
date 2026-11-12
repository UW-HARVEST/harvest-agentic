//! Build configurations of a Test-Corpus test case: one per `test_vectors/<config>/` directory.
//!
//! `default` is the plain `cargo build --release`. Any other configuration names its build
//! parameters in `test_vectors/<config>/configuration.json` and is built with exactly the matching
//! Cargo features (`--no-default-features`), into a target directory of its own.

use crate::harness::TestCase;
use crate::HarvestResult;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The configuration built without features.
pub const DEFAULT: &str = "default";

/// The build parameters of a non-default configuration, beside its vectors.
pub const CONFIGURATION_FILE: &str = "configuration.json";

/// Builds the crate in `crate_dir` (release) in each configuration that `test_cases` use, with
/// the vectors in `vectors_dir`. Returns the `release` directory of each configuration.
pub fn build_all<'a>(
    crate_dir: &Path,
    vectors_dir: &Path,
    test_cases: &'a [TestCase],
) -> HarvestResult<BTreeMap<&'a str, PathBuf>> {
    let mut builds = BTreeMap::new();
    for config in test_cases.iter().map(TestCase::config) {
        if !builds.contains_key(config) {
            log::info!("Building configuration {config}...");
            builds.insert(config, build(crate_dir, vectors_dir, config)?);
        }
    }
    Ok(builds)
}

/// Builds the crate in configuration `config`, whose vectors are in `vectors_dir/<config>/`.
fn build(crate_dir: &Path, vectors_dir: &Path, config: &str) -> HarvestResult<PathBuf> {
    let mut cmd = Command::new("cargo");
    cmd.arg("build").arg("--release").current_dir(crate_dir);
    let target_dir = if config == DEFAULT {
        PathBuf::from("target")
    } else {
        let configuration = vectors_dir.join(config).join(CONFIGURATION_FILE);
        let features = features(&configurable_variables(&configuration)?);
        let target_dir = Path::new("target").join(format!("config-{config}"));
        cmd.arg("--no-default-features")
            .arg("--features")
            .arg(features.join(","))
            .arg("--target-dir")
            .arg(&target_dir);
        target_dir
    };
    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run cargo build ({config}): {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo build --release ({config}) failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(crate_dir.join(target_dir).join("release"))
}

/// Reads and parses the JSON file at `path`.
pub fn read_json(path: &Path) -> HarvestResult<serde_json::Value> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|e| format!("Failed to parse {}: {e}", path.display()).into())
}

/// The `configurable_variables` object of a `configuration.json`: the options and their
/// values (one value each beside a configuration's vectors, every legal value in `test_case/`).
pub fn configurable_variables(
    path: &Path,
) -> HarvestResult<serde_json::Map<String, serde_json::Value>> {
    read_json(path)?
        .get("configurable_variables")
        .and_then(|v| v.as_object())
        .cloned()
        .ok_or_else(|| format!("{} has no configurable_variables", path.display()).into())
}

/// The Cargo features of a configuration, by the Test-Corpus naming: a boolean parameter
/// `NAME` is the feature `NAME`; a string or integer parameter `NAME` with value `v` is the
/// feature `NAME_v`. The `cargo_features` agent skill names the features by the same rule (the
/// tests of both read `agent_skills/cargo_features/tests/naming_cases.json`).
pub fn features(parameters: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    parameters
        .iter()
        .map(|(name, value)| match value {
            // A boolean gives `NAME` also when it is false. This mirrors the official runner
            // (`_fmt_cargo_features` in Test-Corpus/tools/test_runner/runtests/orchestrator/
            // build.py). There it looks like a bug: a configuration that sets an option to false
            // enables its feature, while the C side gets `-DNAME=OFF`. Revisit when it changes.
            serde_json::Value::Bool(_) => name.clone(),
            serde_json::Value::String(v) => format!("{name}_{v}"),
            v => format!("{name}_{v}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_follow_the_shared_naming_cases() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../../../agent_skills/cargo_features/tests/naming_cases.json"
        ))
        .unwrap();
        for case in cases["cases"].as_array().unwrap() {
            let mut parameters = serde_json::Map::new();
            parameters.insert(
                case["option"].as_str().unwrap().to_owned(),
                case["value"].clone(),
            );
            assert_eq!(
                features(&parameters),
                [case["feature"].as_str().unwrap()],
                "{case}"
            );
        }
    }
}

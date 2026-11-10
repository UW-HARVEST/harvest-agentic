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
/// `NAME` set to true is the feature `NAME` (false adds none); a string or integer parameter
/// `NAME` with value `v` is the feature `NAME_v`.
pub fn features(parameters: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    parameters
        .iter()
        .filter_map(|(name, value)| match value {
            serde_json::Value::Bool(true) => Some(name.clone()),
            serde_json::Value::Bool(false) => None,
            serde_json::Value::String(v) => Some(format!("{name}_{v}")),
            v => Some(format!("{name}_{v}")),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_follow_the_test_corpus_naming() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIGURATION_FILE);
        fs::write(
            &path,
            r#"{"configurable_variables": {"MODE": "fast", "SIZE": 64, "EXTRA": true, "DEBUG": false}}"#,
        )
        .unwrap();
        let mut features = features(&configurable_variables(&path).unwrap());
        features.sort();
        assert_eq!(features, ["EXTRA", "MODE_fast", "SIZE_64"]);
    }
}

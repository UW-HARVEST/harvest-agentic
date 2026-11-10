//! The `default` configuration the grader tests, selected in the crate's default features.
//!
//! The Test-Corpus runner builds the C side of the `default` configuration with the `test`
//! preset of the program directory's `CMakePresets.json`, when there is one, and the Rust side
//! with plain `cargo build --release`. The translating agents do not get the preset: the
//! crate's `default` features select the CMake defaults. Before grading, the benchmark puts the
//! preset's configuration in `default`. The change stays in the snapshot, which is also the
//! delivered crate.

use crate::harness::build_config::{self, CONFIGURATION_FILE};
use crate::HarvestResult;
use harvest_core::cargo_utils::CargoToml;
use harvest_core::stage_manifest;
use load_test_suite::PRESETS_FILE;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;

/// The configure preset that the Test-Corpus runner builds the C side with.
const TEST_PRESET: &str = "test";

/// Sets the `default` features of the snapshot in `program_dir` to the configuration of the
/// `test` preset that its suite carries. For each option that the preset sets, the features of
/// that option leave `default` and the preset's feature goes in; other entries stay. A boolean
/// option is one whose values in the C source's `configuration.json` are booleans. Does nothing
/// when there is no preset or it sets no project option. Fails when the crate does not declare
/// a feature the preset selects.
pub fn select_graded_default(program_dir: &Path) -> HarvestResult<()> {
    let presets_path = stage_manifest::suite_dir(program_dir).join(PRESETS_FILE);
    if !presets_path.is_file() {
        return Ok(());
    }
    let preset = test_preset(&presets_path)?;
    if preset.is_empty() {
        return Ok(());
    }
    let options_path = stage_manifest::c_source_dir(program_dir).join(CONFIGURATION_FILE);
    let options = if options_path.is_file() {
        build_config::configurable_variables(&options_path)?
    } else {
        Map::new()
    };
    let wanted = build_config::features(&typed_values(&preset, &options));

    let mut cargo = CargoToml::open(&program_dir.join("Cargo.toml"))?;
    let declared = cargo.feature_names();
    let missing: Vec<&String> = wanted.iter().filter(|f| !declared.contains(f)).collect();
    if !missing.is_empty() {
        return Err(format!(
            "Cargo.toml has no feature {missing:?} for the default configuration \
             (the `{TEST_PRESET}` preset of {PRESETS_FILE} sets {preset:?})"
        )
        .into());
    }
    let selects_preset_option = |feature: &str| {
        preset.keys().any(|name| {
            feature == name
                || feature
                    .strip_prefix(name.as_str())
                    .is_some_and(|rest| rest.starts_with('_'))
        })
    };
    let old = cargo.default_features();
    let mut new: Vec<String> = old
        .iter()
        .filter(|f| !selects_preset_option(f))
        .cloned()
        .collect();
    new.extend(wanted);
    if new != old {
        log::info!("Default features {old:?} -> {new:?} (the `{TEST_PRESET}` preset)");
        cargo.set_default_features(&new);
        cargo.save()?;
    }
    Ok(())
}

/// The project options that the `test` configure preset at `path` sets, with its inherited
/// presets applied first. `CMAKE_*` variables are left out.
fn test_preset(path: &Path) -> HarvestResult<Map<String, Value>> {
    let json = build_config::read_json(path)?;
    let presets: HashMap<&str, &Value> = json
        .get("configurePresets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p.get("name")?.as_str()?, p)))
        .collect();
    let test = presets
        .get(TEST_PRESET)
        .ok_or_else(|| format!("{} has no `{TEST_PRESET}` configure preset", path.display()))?;
    let mut variables = Map::new();
    apply_preset(test, &presets, &mut variables);
    variables.retain(|name, _| !name.starts_with("CMAKE_"));
    Ok(variables)
}

/// Adds the cache variables of `preset` to `variables`, those of its inherited presets first.
fn apply_preset(
    preset: &Value,
    presets: &HashMap<&str, &Value>,
    variables: &mut Map<String, Value>,
) {
    let parents: Vec<&str> = match preset.get("inherits") {
        Some(Value::String(name)) => vec![name.as_str()],
        Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    for parent in parents {
        if let Some(parent) = presets.get(parent) {
            apply_preset(parent, presets, variables);
        }
    }
    if let Some(cache) = preset.get("cacheVariables").and_then(Value::as_object) {
        variables.extend(cache.clone());
    }
}

/// The preset's values, with a CMake truth value for each option whose legal values in
/// `options` (the C source's `configurable_variables`) are booleans.
fn typed_values(preset: &Map<String, Value>, options: &Map<String, Value>) -> Map<String, Value> {
    let is_boolean = |name: &str| {
        options
            .get(name)
            .and_then(Value::as_array)
            .is_some_and(|values| !values.is_empty() && values.iter().all(Value::is_boolean))
    };
    preset
        .iter()
        .map(|(name, value)| match value {
            Value::String(s) if is_boolean(name) => (
                name.clone(),
                Value::Bool(matches!(
                    s.to_ascii_uppercase().as_str(),
                    "1" | "ON" | "YES" | "TRUE" | "Y"
                )),
            ),
            _ => (name.clone(), value.clone()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const PRESETS: &str = r#"{
      "version": 6,
      "configurePresets": [
        { "name": "base", "hidden": true,
          "cacheVariables": { "CMAKE_BUILD_TYPE": "Release", "OP": "add" } },
        { "name": "test", "inherits": "base",
          "cacheVariables": { "OP": "mul", "REPEAT": "4" } }
      ]
    }"#;

    const CARGO: &str = "[package]\nname = \"p\"\n\n[features]\n\
        default = [\"OP_add\", \"REPEAT_5\", \"std\"]\n\
        std = []\nOP_add = []\nOP_mul = []\nREPEAT_4 = []\nREPEAT_5 = []\n";

    /// A snapshot program directory with `cargo` as its manifest.
    fn snapshot(presets: Option<&str>, options: Option<&str>, cargo: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let suite = stage_manifest::suite_dir(dir.path());
        let c_src = stage_manifest::c_source_dir(dir.path());
        fs::create_dir_all(&suite).unwrap();
        fs::create_dir_all(&c_src).unwrap();
        if let Some(presets) = presets {
            fs::write(suite.join(PRESETS_FILE), presets).unwrap();
        }
        if let Some(options) = options {
            fs::write(c_src.join(CONFIGURATION_FILE), options).unwrap();
        }
        fs::write(dir.path().join("Cargo.toml"), cargo).unwrap();
        dir
    }

    fn default_features(dir: &tempfile::TempDir) -> Vec<String> {
        CargoToml::open(&dir.path().join("Cargo.toml"))
            .unwrap()
            .default_features()
    }

    #[test]
    fn the_preset_replaces_the_option_features_of_default() {
        let dir = snapshot(Some(PRESETS), None, CARGO);
        select_graded_default(dir.path()).unwrap();
        assert_eq!(default_features(&dir), ["std", "OP_mul", "REPEAT_4"]);
    }

    #[test]
    fn options_the_preset_does_not_set_keep_their_default() {
        let presets =
            r#"{"configurePresets": [{"name": "test", "cacheVariables": {"OP": "mul"}}]}"#;
        let options = r#"{"configurable_variables": {"OP": ["add", "mul"], "REPEAT": [4, 5]}}"#;
        let dir = snapshot(Some(presets), Some(options), CARGO);
        select_graded_default(dir.path()).unwrap();
        assert_eq!(default_features(&dir), ["REPEAT_5", "std", "OP_mul"]);
    }

    #[test]
    fn no_presets_file_changes_nothing() {
        let dir = snapshot(None, None, CARGO);
        select_graded_default(dir.path()).unwrap();
        assert_eq!(default_features(&dir), ["OP_add", "REPEAT_5", "std"]);
    }

    #[test]
    fn a_preset_of_only_cmake_variables_changes_nothing() {
        let presets = r#"{"configurePresets": [
            {"name": "test", "cacheVariables": {"CMAKE_C_STANDARD": "99"}}]}"#;
        let dir = snapshot(Some(presets), None, CARGO);
        select_graded_default(dir.path()).unwrap();
        assert_eq!(default_features(&dir), ["OP_add", "REPEAT_5", "std"]);
    }

    #[test]
    fn a_missing_feature_is_an_error() {
        let cargo = "[package]\nname = \"p\"\n\n[features]\ndefault = []\nOP_mul = []\n";
        let dir = snapshot(Some(PRESETS), None, cargo);
        let err = select_graded_default(dir.path()).unwrap_err().to_string();
        assert!(err.contains("REPEAT_4"), "{err}");
        assert!(default_features(&dir).is_empty());
    }

    #[test]
    fn a_presets_file_without_the_test_preset_is_an_error() {
        let presets = r#"{"configurePresets": [{"name": "dev", "cacheVariables": {"OP": "mul"}}]}"#;
        let dir = snapshot(Some(presets), None, CARGO);
        assert!(select_graded_default(dir.path()).is_err());
    }

    #[test]
    fn boolean_options_follow_configuration_json() {
        let presets = r#"{"configurePresets": [{"name": "test", "cacheVariables": {
            "USE_COMPRESSION": "builtin", "USE_NSEC": "ON", "DEPRECATE_HARD": "OFF",
            "MODE": "on"}}]}"#;
        let options = r#"{"configurable_variables": {
            "USE_COMPRESSION": ["zlib", "builtin"], "USE_NSEC": [true, false],
            "DEPRECATE_HARD": [true, false], "MODE": ["on", "off"]}}"#;
        let cargo = "[package]\nname = \"p\"\n\n[features]\n\
            default = [\"USE_COMPRESSION_zlib\", \"DEPRECATE_HARD\"]\n\
            USE_COMPRESSION_zlib = []\nUSE_COMPRESSION_builtin = []\nUSE_NSEC = []\n\
            DEPRECATE_HARD = []\nMODE_on = []\n";
        let dir = snapshot(Some(presets), Some(options), cargo);
        select_graded_default(dir.path()).unwrap();
        assert_eq!(
            default_features(&dir),
            ["MODE_on", "USE_COMPRESSION_builtin", "USE_NSEC"]
        );
    }
}

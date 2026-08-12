//! Parser for `CMakePresets.json` test configurations.
//!
//! Test cases declare their required build configuration via a CMake preset
//! named `"test"` (or, failing that, the last non-hidden `configurePreset`):
//!
//! ```json
//! "cacheVariables": {
//!     "HASH_BACKEND": "blake",
//!     "SECPAR": "128f",
//!     "THASH": "simple"
//! }
//! ```
//!
//! The agent translates the C `#ifdef` switches into Cargo features whose names
//! match the cache-variable values exactly (e.g. `blake`, `128f`, `simple`).
//! Builtin CMake keys (those starting with `CMAKE_`) are filtered out — only
//! project-specific knobs survive into the Cargo feature list.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// One configuration the agent must verify: a set of CMake `-D` flags and the
/// matching Cargo feature list.
#[derive(Debug, Clone, Default)]
pub struct TestConfig {
    /// CMake build flags ready to splice into a command line, e.g.
    /// `-DHASH_BACKEND=blake -DSECPAR=128f -DTHASH=simple`. Empty when no
    /// project-specific cache variables are set.
    pub cmake_flags: String,

    /// Cargo feature names matching the cache-variable values, in sorted
    /// order. Empty when no project-specific cache variables are set.
    pub cargo_features: Vec<String>,
}

impl TestConfig {
    /// Whether this config carries any project-specific knobs.
    pub fn is_empty(&self) -> bool {
        self.cmake_flags.is_empty() && self.cargo_features.is_empty()
    }

    /// Markdown bullet describing this config, suitable to inject into a
    /// verify prompt under "Configurations to verify:".
    pub fn as_markdown_bullet(&self) -> String {
        format!(
            "- cmake flags: `{}`\n  cargo features: `{}`",
            self.cmake_flags,
            self.cargo_features.join(",")
        )
    }
}

/// Search for a `CMakePresets.json` in `start_dir` and a few common
/// neighbouring locations, then read it.
///
/// HARVEST tools see different "input" paths: `try_cargo_build` is given the
/// `test_case/` subdirectory, while `verify_fix_agentic` works in a tempdir
/// containing `translated_rust/c_src/`. The presets file lives at the test
/// case root in either case. This helper papers over those layout
/// differences by trying the obvious candidates in order.
pub fn find_test_config(start_dir: &Path) -> TestConfig {
    let candidates = [
        start_dir.join("CMakePresets.json"),
        start_dir.join("..").join("CMakePresets.json"),
        start_dir.join("translated_rust/c_src/CMakePresets.json"),
        start_dir.join("c_src/CMakePresets.json"),
    ];
    for c in &candidates {
        if c.exists() {
            let cfg = read_test_config(c);
            if !cfg.is_empty() {
                return cfg;
            }
        }
    }
    TestConfig::default()
}

/// Read the test config from a `CMakePresets.json` at `path`.
///
/// Returns `TestConfig::default()` (i.e. empty) when the file is missing,
/// unparseable, or contains no project-specific cache variables.
pub fn read_test_config(presets_path: &Path) -> TestConfig {
    let Ok(content) = std::fs::read_to_string(presets_path) else {
        return TestConfig::default();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        tracing::warn!(
            "Failed to parse {}; skipping test config",
            presets_path.display()
        );
        return TestConfig::default();
    };

    let presets = json
        .get("configurePresets")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let by_name: HashMap<String, &serde_json::Value> = presets
        .iter()
        .filter_map(|p| Some((p.get("name")?.as_str()?.to_string(), p)))
        .collect();

    let target = by_name.get("test").copied().or_else(|| {
        presets
            .iter()
            .rfind(|p| !p.get("hidden").and_then(|v| v.as_bool()).unwrap_or(false))
    });
    let Some(target) = target else {
        return TestConfig::default();
    };

    let mut merged: BTreeMap<String, String> = BTreeMap::new();
    walk_inherits(target, &by_name, &mut merged);

    let project_vars: BTreeMap<&String, &String> = merged
        .iter()
        .filter(|(k, _)| !k.starts_with("CMAKE_"))
        .collect();

    let cmake_flags = project_vars
        .iter()
        .map(|(k, v)| format!("-D{}={}", k, v))
        .collect::<Vec<_>>()
        .join(" ");
    let cargo_features = project_vars.values().map(|v| v.to_string()).collect();

    TestConfig {
        cmake_flags,
        cargo_features,
    }
}

fn walk_inherits(
    node: &serde_json::Value,
    by_name: &HashMap<String, &serde_json::Value>,
    merged: &mut BTreeMap<String, String>,
) {
    if let Some(inherits) = node.get("inherits") {
        let parents: Vec<&str> = match inherits {
            serde_json::Value::String(s) => vec![s.as_str()],
            serde_json::Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect(),
            _ => Vec::new(),
        };
        for p in parents {
            if let Some(parent) = by_name.get(p) {
                walk_inherits(parent, by_name, merged);
            }
        }
    }
    if let Some(vars) = node.get("cacheVariables").and_then(|v| v.as_object()) {
        for (k, v) in vars {
            if let Some(s) = v.as_str() {
                merged.insert(k.clone(), s.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, json: &str) -> std::path::PathBuf {
        let p = dir.join("CMakePresets.json");
        fs::write(&p, json).unwrap();
        p
    }

    #[test]
    fn extracts_features_and_flags_from_test_preset() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            r#"{
              "configurePresets": [
                {
                  "name": "base", "hidden": true,
                  "cacheVariables": { "CMAKE_BUILD_TYPE": "Release" }
                },
                {
                  "name": "test", "inherits": "base",
                  "cacheVariables": {
                    "HASH_BACKEND": "blake",
                    "SECPAR": "128f",
                    "THASH": "simple"
                  }
                }
              ]
            }"#,
        );
        let cfg = read_test_config(&path);
        assert_eq!(cfg.cargo_features, vec!["blake", "128f", "simple"]);
        // BTreeMap orders alphabetically by key (HASH_BACKEND, SECPAR, THASH)
        assert_eq!(
            cfg.cmake_flags,
            "-DHASH_BACKEND=blake -DSECPAR=128f -DTHASH=simple"
        );
    }

    #[test]
    fn missing_file_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = read_test_config(&tmp.path().join("nonexistent.json"));
        assert!(cfg.is_empty());
    }

    #[test]
    fn only_cmake_builtins_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            r#"{
              "configurePresets": [
                {
                  "name": "test",
                  "cacheVariables": { "CMAKE_C_STANDARD": "99" }
                }
              ]
            }"#,
        );
        assert!(read_test_config(&path).is_empty());
    }

    #[test]
    fn find_walks_up_one_level() {
        // Mirrors the benchmark layout: program_dir/CMakePresets.json,
        // and tools see program_dir/test_case/ as their input.
        let tmp = tempfile::tempdir().unwrap();
        let test_case = tmp.path().join("test_case");
        fs::create_dir_all(&test_case).unwrap();
        write(
            tmp.path(),
            r#"{
              "configurePresets": [
                { "name": "test",
                  "cacheVariables": { "HASH_BACKEND": "blake" } }
              ]
            }"#,
        );
        let cfg = find_test_config(&test_case);
        assert_eq!(cfg.cargo_features, vec!["blake"]);
    }

    #[test]
    fn find_returns_empty_when_no_presets_anywhere() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(find_test_config(tmp.path()).is_empty());
    }

    #[test]
    fn markdown_bullet_format() {
        let cfg = TestConfig {
            cmake_flags: "-DHASH_BACKEND=blake".into(),
            cargo_features: vec!["blake".into()],
        };
        assert_eq!(
            cfg.as_markdown_bullet(),
            "- cmake flags: `-DHASH_BACKEND=blake`\n  cargo features: `blake`"
        );
    }
}

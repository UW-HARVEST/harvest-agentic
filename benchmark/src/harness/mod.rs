/// This module `harness` is intended to contain code that is specific to a particular set of benchmarks,
/// for example, parsing code for benchmark-specific configs.
/// Currently, that is just the MITLL tractor benchmarks.
pub mod build_config;
pub mod default_config;
pub mod gtest;
pub mod library;

use crate::runner;
use crate::stats::ProgramEvalStats;
use crate::HarvestResult;
use regex::Regex;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Represents the expected stdout pattern in a test case
/// Used for tractor test cases
#[derive(Default, Debug, Deserialize, Clone)]
pub struct StdoutPattern {
    pub pattern: String,
    #[serde(default)]
    pub is_regex: bool,
}

/// Represents a test case with command arguments, input, and expected output
/// Used for tractor test cases
#[derive(Default, Debug, Deserialize, Clone)]
pub struct TestCase {
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub stdin: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub stdout: StdoutPattern,
    /// Present means the vector is skipped; its value (a note, a number or a bool) is not used.
    #[serde(default)]
    pub has_ub: Option<serde_json::Value>,
    /// The vector's path relative to `test_vectors/`: `<config>/<name>.json`, or
    /// `<config>/<name>/cando_vector.json` for a directory vector.
    #[serde(skip)] // Don't serialize/deserialize this field as it's not part of the JSON
    pub filename: String,
}

impl TestCase {
    /// The build configuration the vector runs against: the first component of `filename`.
    pub fn config(&self) -> &str {
        self.filename.split('/').next().unwrap_or_default()
    }
}

/// Parses a JSON file into a TestCase struct
pub fn parse_test_case_json<P: AsRef<Path>>(file_path: P) -> HarvestResult<TestCase> {
    let file_path = file_path.as_ref();

    // Read the JSON content from the file
    let json_str = fs::read_to_string(file_path).map_err(|e| {
        format!(
            "Failed to read test case file {}: {}",
            file_path.display(),
            e
        )
    })?;

    let mut test_case: TestCase = serde_json::from_str(&json_str).map_err(|e| {
        format!(
            "Failed to parse test case JSON from {}: {}",
            file_path.display(),
            e
        )
    })?;

    // Set the filename field to the file name
    test_case.filename = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_string();

    Ok(test_case)
}

/// Validate that required benchmark subdirectories exist.
/// Returns (test_case_dir, test_vectors_dir). Note: we return the `test_case`
/// root (not `test_case/src`) so downstream tools can see CMakeLists.txt if
/// present.
pub fn parse_benchmark_dir(input_dir: &Path) -> HarvestResult<(PathBuf, PathBuf)> {
    if !input_dir.exists() {
        return Err(format!("Input directory does not exist: {}", input_dir.display()).into());
    }
    if !input_dir.is_dir() {
        return Err(format!("Input path is not a directory: {}", input_dir.display()).into());
    }

    // Check for required subdirectories
    let test_case_dir = input_dir.join("test_case");
    let test_vectors_dir = input_dir.join("test_vectors");

    if !test_case_dir.exists() || !test_case_dir.is_dir() {
        return Err(format!(
            "Required test_case directory not found: {}",
            test_case_dir.display()
        )
        .into());
    }

    // test_case/ is a CMake project; its source layout is free (src/, app/,
    // sources at the top as in chibicc, or an upstream tree as in libsodium).
    if !test_case_dir.join("CMakeLists.txt").is_file() {
        return Err(format!(
            "Required test_case/CMakeLists.txt not found: {}",
            test_case_dir.display()
        )
        .into());
    }
    let has_gtest = matches!(
        load_test_suite::detect_kind(input_dir, None),
        Ok(full_source::TestSuiteKind::Gtest)
    );

    // JSON vectors are optional for test cases that ship a GoogleTest suite
    // instead (gtest_suite/); everything else still requires test_vectors/.
    if (!test_vectors_dir.exists() || !test_vectors_dir.is_dir()) && !has_gtest {
        return Err(format!(
            "Required test_vectors directory not found: {}",
            test_vectors_dir.display()
        )
        .into());
    }

    // Return the test_case root so downstream tools can find metadata files
    // (e.g., CMakeLists.txt) beside the sources.
    Ok((test_case_dir, test_vectors_dir))
}

/// The JSON of a directory vector (`test_vectors/<config>/<name>/`).
const CANDO_VECTOR_FILE: &str = "cando_vector.json";

/// Reads the test vectors of every build configuration. Each `test_vectors/<config>/` holds
/// `<name>.json` vectors and `<name>/cando_vector.json` directory vectors, and, for a
/// configuration other than `default`, its `configuration.json`.
pub fn parse_test_vectors<P: AsRef<Path>>(directory_path: P) -> HarvestResult<Vec<TestCase>> {
    let dir_path = directory_path.as_ref();
    let mut test_cases = Vec::new();
    for config in sorted_entries(dir_path)? {
        if !config.is_dir() {
            continue;
        }
        for entry in sorted_entries(&config)? {
            let file = if entry.is_dir() {
                entry.join(CANDO_VECTOR_FILE)
            } else {
                entry
            };
            let is_vector = file.extension().is_some_and(|ext| ext == "json")
                && file
                    .file_name()
                    .is_some_and(|n| n != build_config::CONFIGURATION_FILE)
                && file.is_file();
            if !is_vector {
                continue;
            }
            let mut test_case = parse_test_case_json(&file)?;
            test_case.filename = file
                .strip_prefix(dir_path)
                .unwrap_or(&file)
                .to_string_lossy()
                .into_owned();
            test_cases.push(test_case);
        }
    }
    Ok(test_cases)
}

fn sorted_entries(dir: &Path) -> HarvestResult<Vec<PathBuf>> {
    let mut entries = fs::read_dir(dir)
        .map_err(|e| format!("Failed to read directory {}: {}", dir.display(), e))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    Ok(entries)
}

/// Prepares a fresh working directory for one vector, as the Test-Corpus runner does: a
/// directory vector's `setup` script runs first, with `DEFAULT_RUN_DIR` (the fresh directory),
/// `WORKING_DIR_FILE` (where it may write another directory to run in) and, when the vector has
/// `resources/`, `TEST_RESOURCES`. Returns the fresh directory, to keep alive while the vector
/// runs, and the directory to run in.
pub fn prepare_run_dir(
    vectors_dir: &Path,
    test_case: &TestCase,
) -> HarvestResult<(tempfile::TempDir, PathBuf)> {
    let run_dir = tempfile::tempdir()?;
    let run_path = run_dir.path().to_path_buf();
    // The script runs in the fresh directory: a relative path would not resolve there.
    let vector = std::path::absolute(vectors_dir.join(&test_case.filename))?;
    let setup = vector.with_file_name("setup");
    if !test_case.filename.ends_with(CANDO_VECTOR_FILE) || !setup.is_file() {
        return Ok((run_dir, run_path));
    }
    // A suite copy does not keep the executable bit: run the script by its #! line.
    let script = fs::read_to_string(&setup)?;
    let mut interpreter = script
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("#!"))
        .ok_or_else(|| format!("{} has no #! line", setup.display()))?
        .split_whitespace();
    let working_dir_file = tempfile::NamedTempFile::new()?;
    let mut cmd = Command::new(interpreter.next().unwrap_or("/bin/sh"));
    cmd.args(interpreter)
        .arg(&setup)
        .current_dir(&run_path)
        .env("DEFAULT_RUN_DIR", &run_path)
        .env("WORKING_DIR_FILE", working_dir_file.path());
    let resources = vector.with_file_name("resources");
    if resources.is_dir() {
        cmd.env("TEST_RESOURCES", &resources);
    }
    let output = cmd.output()?;
    if !output.status.success() {
        return Err(format!(
            "{} failed: {}\n{}",
            setup.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let working_dir = fs::read_to_string(working_dir_file.path())?;
    let working_dir = working_dir.trim();
    let cwd = if working_dir.is_empty() {
        run_path
    } else {
        PathBuf::from(working_dir)
    };
    Ok((run_dir, cwd))
}

/// Clean up build artifacts from successfully translated Rust projects
pub fn cleanup_benchmarks(results: &[ProgramEvalStats], output_dir: &Path) {
    let mut cleaned_count = 0;
    let mut cleanup_errors = Vec::new();

    for result in results {
        if result.translation_success {
            let project_dir = output_dir.join(&result.program_name);

            // Check if Cargo.toml exists to confirm it's a Rust project
            if project_dir.join("Cargo.toml").exists() {
                match Command::new("cargo")
                    .arg("clean")
                    .current_dir(&project_dir)
                    .output()
                {
                    Ok(output) if output.status.success() => {
                        cleaned_count += 1;
                    }
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        let error_msg =
                            format!("Failed to clean {}: {}", result.program_name, stderr);
                        cleanup_errors.push(error_msg.clone());
                        log::info!("  ❌ {}", error_msg);
                    }
                    Err(e) => {
                        let error_msg = format!(
                            "Failed to execute cargo clean for {}: {}",
                            result.program_name, e
                        );
                        cleanup_errors.push(error_msg.clone());
                        log::info!("  ❌ {}", error_msg);
                    }
                }
            } else {
                log::info!("Skipping {}: No Cargo.toml found", result.program_name);
            }
        }
    }

    log::info!("\nCleanup Summary:");
    log::info!("  Successfully cleaned: {} projects", cleaned_count);
    if !cleanup_errors.is_empty() {
        log::info!("  Cleanup errors: {} projects", cleanup_errors.len());
        for error in &cleanup_errors {
            log::info!("    - {}", error);
        }
    }

    log::info!("\nDone!");
}

// TODO: need to skip test cases that have UB flag set.

/// Runs a binary with test case inputs and compares its output against expected values.
///
/// This function executes the binary with the command line arguments and stdin
/// from the provided test case, then compares the actual stdout and stderr
/// against the expected values in the test case.
pub fn validate_binary_output(
    binary_path: &Path,
    test_case: &TestCase,
    cwd: &Path,
    timeout_seconds: Option<u64>,
) -> HarvestResult<()> {
    let timeout = Duration::from_secs(timeout_seconds.unwrap_or(10));

    // Run the binary
    let output = runner::run_binary_with_timeout(binary_path, test_case, cwd, timeout)
        .map_err(|e| format!("Failed to run binary {}: {}", binary_path.display(), e))?;

    let actual_stdout = String::from_utf8_lossy(&output.stdout);
    let actual_stderr = String::from_utf8_lossy(&output.stderr);

    // Compare stdout against expected pattern
    let matches = if test_case.stdout.is_regex {
        // Use regex matching
        let regex = Regex::new(&test_case.stdout.pattern).map_err(|e| {
            format!(
                "Invalid regex pattern '{}': {}",
                test_case.stdout.pattern, e
            )
        })?;
        regex.is_match(&actual_stdout)
    } else {
        // Use simple equality matching
        actual_stdout.trim() == test_case.stdout.pattern.trim()
    };

    if matches {
        Ok(())
    } else {
        let pattern_type = if test_case.stdout.is_regex {
            "regex pattern"
        } else {
            "expected stdout"
        };
        Err(format!(
            "❌ Binary produced unexpected output:\n\n{}: {}\n\nActual stdout:\n{}\n\nActual stderr:\n{}",
            pattern_type.chars().next().unwrap().to_uppercase().collect::<String>() + &pattern_type[1..],
            test_case.stdout.pattern,
            actual_stdout,
            actual_stderr
        ).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a bench program directory with the given subdirectory paths.
    fn bench_dir(subdirs: &[&str]) -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("create tempdir");
        for sub in subdirs {
            fs::create_dir_all(root.path().join(sub)).expect("create subdir");
        }
        root
    }

    /// A bench program directory whose test_case/ is a CMake project.
    fn cmake_bench_dir(subdirs: &[&str]) -> tempfile::TempDir {
        let root = bench_dir(&[&["test_case"], subdirs].concat());
        fs::write(root.path().join("test_case/CMakeLists.txt"), "").unwrap();
        root
    }

    #[test]
    fn cmake_project_is_accepted() {
        // Any source layout: chibicc keeps its sources at the top of test_case/.
        let dir = cmake_bench_dir(&["test_vectors"]);
        assert!(parse_benchmark_dir(dir.path()).is_ok());
    }

    #[test]
    fn gtest_case_needs_no_test_vectors() {
        let dir = cmake_bench_dir(&["gtest_suite"]);
        assert!(parse_benchmark_dir(dir.path()).is_ok());
    }

    #[test]
    fn missing_cmakelists_is_rejected() {
        let dir = bench_dir(&["test_case/src", "test_vectors"]);
        let err = parse_benchmark_dir(dir.path()).unwrap_err().to_string();
        assert!(err.contains("CMakeLists.txt"), "unexpected error: {err}");
    }

    #[test]
    fn missing_test_case_is_rejected() {
        let dir = bench_dir(&["gtest_suite"]);
        let err = parse_benchmark_dir(dir.path()).unwrap_err().to_string();
        assert!(err.contains("test_case"), "unexpected error: {err}");
    }

    #[test]
    fn missing_vectors_without_gtest_is_rejected() {
        let dir = cmake_bench_dir(&[]);
        let err = parse_benchmark_dir(dir.path()).unwrap_err().to_string();
        assert!(err.contains("test_vectors"), "unexpected error: {err}");
    }

    #[test]
    fn vectors_are_read_per_build_configuration() {
        let dir = bench_dir(&["default/d1", "fast"]);
        let vectors = dir.path();
        fs::write(
            vectors.join("default/v1.json"),
            r#"{"rc": -11, "has_ub": 1}"#,
        )
        .unwrap();
        fs::write(vectors.join("default/d1/cando_vector.json"), "{}").unwrap();
        fs::write(vectors.join("fast/v2.json"), "{}").unwrap();
        fs::write(vectors.join("fast/configuration.json"), "{}").unwrap();
        let names: Vec<_> = parse_test_vectors(vectors)
            .unwrap()
            .into_iter()
            .map(|t| (t.filename.clone(), t.config().to_owned()))
            .collect();
        let expected = [
            ("default/d1/cando_vector.json", "default"),
            ("default/v1.json", "default"),
            ("fast/v2.json", "fast"),
        ];
        assert_eq!(names, expected.map(|(f, c)| (f.to_owned(), c.to_owned())));
    }

    #[test]
    fn setup_prepares_the_working_directory() {
        // A relative vectors path, as from a relative command-line path: the setup script runs
        // in another directory.
        let dir = tempfile::tempdir_in(".").unwrap();
        let cwd = std::env::current_dir().unwrap();
        let vectors = dir.path().strip_prefix(&cwd).unwrap();
        fs::create_dir_all(vectors.join("default/d1/resources")).unwrap();
        fs::write(vectors.join("default/d1/cando_vector.json"), "{}").unwrap();
        fs::write(vectors.join("default/d1/resources/r.txt"), "res").unwrap();
        fs::write(
            vectors.join("default/d1/setup"),
            "#!/bin/sh\nmkdir \"$DEFAULT_RUN_DIR/w\"\n\
             cp \"$TEST_RESOURCES/r.txt\" \"$DEFAULT_RUN_DIR/w/\"\n\
             echo \"$DEFAULT_RUN_DIR/w\" > \"$WORKING_DIR_FILE\"\n",
        )
        .unwrap();
        let test_case = TestCase {
            filename: "default/d1/cando_vector.json".to_owned(),
            ..TestCase::default()
        };
        let (run_dir, cwd) = prepare_run_dir(vectors, &test_case).unwrap();
        assert_eq!(cwd, run_dir.path().join("w"));
        assert_eq!(fs::read_to_string(cwd.join("r.txt")).unwrap(), "res");
    }
}

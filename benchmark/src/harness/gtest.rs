//! GoogleTest-based validation for translated shared libraries.
//!
//! A test case may ship a `gtest_suite/` directory: a CMake project containing
//! GoogleTest tests that call the library's exported C-ABI symbols directly.
//! Unlike the cando2 runner (single dispatch symbol, one call per vector),
//! a gtest suite can exercise multi-call API sequences and stateful setups.
//!
//! # Contract with the suite
//! - `gtest_suite/CMakeLists.txt` accepts `-DTEST_LIB_PATH=<abs .so>` (library
//!   under test) and is otherwise self-contained: it declares its own
//!   tag-pinned GoogleTest via FetchContent.
//! - The test executable target is named `harvest_gtest`.
//! - An optional `manifest.json` declares the process-level invocation plan
//!   (see [`GtestManifest`]); a legacy `budgets.json` is honored as an
//!   all-per-case plan.
//!
//! # Execution model
//! Tests are enumerated with `--gtest_list_tests`, then executed according to
//! the manifest:
//! - `mode: "case"` (and any test not matched by the manifest): one process
//!   per test via `--gtest_filter=<name>`. This keeps a crashing test (e.g. a
//!   segfault inside the translated library) from taking down the results of
//!   the remaining tests — gtest itself writes no report at all if the
//!   process dies mid-run.
//! - `mode: "suite"`: one process for a whole group of tests (aggregate
//!   suites whose sub-cases only read a shared in-memory record produced by a
//!   fork inside the suite — the fork, not the test process, is the crash
//!   container there). Per-case verdicts are ingested from the run's
//!   `--gtest_output=json:` report. If the batch process dies, no report
//!   exists and every planned test in the batch is recorded as failed.

use crate::error::HarvestResult;
use crate::harness::library;
use crate::stats::TestResult;
use harvest_core::cargo_utils::{self, CargoToml};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

/// Directory (inside a test case and inside the translated output) holding the suite
pub const GTEST_SUITE_DIR: &str = "gtest_suite";

/// Where the suite is built, relative to the translated output directory
const GTEST_BUILD_SUBDIR: &str = "target/gtest_build";

/// Required name of the suite's test executable target
const GTEST_BINARY_NAME: &str = "harvest_gtest";

/// Environment variable for shared library search paths
#[cfg(target_os = "macos")]
const LD_LIBRARY_PATH_ENV: &str = "DYLD_LIBRARY_PATH";
#[cfg(target_os = "windows")]
const LD_LIBRARY_PATH_ENV: &str = "PATH";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const LD_LIBRARY_PATH_ENV: &str = "LD_LIBRARY_PATH";

/// Grace the harness waits beyond an invocation's budget before SIGKILLing it.
const KILL_GRACE_SECS: u64 = 5;

/// Validates a translated Rust library by building and running the test case's
/// GoogleTest suite against the compiled cdylib.
///
/// # Arguments
/// * `program_name` - Name of the program being tested
/// * `input_dir` - Directory containing the original test case (source of
///   `gtest_suite/`; equal to `output_dir` in test-only reruns)
/// * `output_dir` - Directory containing the translated Rust project
/// * `timeout` - Fallback timeout in seconds for tests without a manifest entry
///
/// # Returns
/// Tuple of (test_results, error_messages). One `TestResult` per gtest test,
/// with `filename` set to the full `Suite.Test` name.
pub fn run_gtest_validation(
    program_name: &str,
    input_dir: &Path,
    output_dir: &Path,
    timeout: u64,
) -> HarvestResult<(Vec<TestResult>, Vec<String>)> {
    // Copy the suite from the original test case unless this is a test-only
    // rerun of an already-translated output directory.
    let suite_dir = output_dir.join(GTEST_SUITE_DIR);
    if input_dir != output_dir {
        cargo_utils::copy_directory_recursive(&input_dir.join(GTEST_SUITE_DIR), &suite_dir)
            .map_err(|e| format!("Failed to copy gtest_suite directory: {}", e))?;
    }
    if !suite_dir.is_dir() {
        return Err(format!("gtest_suite directory not found at {}", suite_dir.display()).into());
    }

    // Rebuild the translated project as a cdylib (same preparation as library
    // validation).
    let mut cargo = CargoToml::open(&output_dir.join("Cargo.toml"))?;
    cargo.add_workspace();
    cargo.ensure_cdylib();
    cargo.save()?;

    log::info!("Rebuilding project as cdylib...");
    let output = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .current_dir(output_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to run cargo build: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("cargo build --release failed: {}", stderr).into());
    }
    log::info!("✅ Cdylib build completed successfully");

    let lib_path = library::locate_compiled_library(output_dir, program_name)?;
    let lib_path = lib_path.canonicalize().unwrap_or(lib_path);
    log::info!("Located library at: {}", lib_path.display());

    let gtest_bin = build_gtest_suite(output_dir, &suite_dir, &lib_path)?;
    log::info!("GoogleTest suite built at: {}", gtest_bin.display());

    let ld_library_path = lib_path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    let test_names = list_gtest_tests(&gtest_bin, &ld_library_path)?;
    log::info!("Discovered {} GoogleTest test(s)", test_names.len());

    let manifest = load_gtest_manifest(&suite_dir);
    let plan = build_execution_plan(&test_names, manifest.as_ref(), timeout);
    log::info!(
        "Execution plan: {} batch invocation(s), {} per-case invocation(s)",
        plan.batches.len(),
        plan.singles.len()
    );

    execute_plan(&gtest_bin, &ld_library_path, &plan)
}

// ---------------------------------------------------------------------------
// Manifest

/// Name of the invocation-plan manifest inside `gtest_suite/`.
const MANIFEST_FILE: &str = "manifest.json";

/// Name of the legacy per-test budgets file (honored as an all-`case` plan).
const BUDGETS_FILE: &str = "budgets.json";

/// How a manifest entry is executed.
#[derive(serde::Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum InvocationMode {
    /// One process per matched test; the budget applies to each test.
    Case,
    /// One process for all matched tests together; the budget applies to the
    /// whole invocation and per-case verdicts come from the JSON report.
    Suite,
}

/// One planned process-level invocation.
#[derive(serde::Deserialize, Clone, Debug)]
pub struct InvocationSpec {
    pub mode: InvocationMode,
    /// gtest filter expression (`:`-separated positive patterns, `*`/`?`
    /// wildcards). Passed verbatim to `--gtest_filter` for `suite` entries.
    pub filter: String,
    /// Measured C-baseline seconds for this invocation. The granted timeout
    /// is `max(budget * default_factor, min_seconds)`.
    pub budget: f64,
    #[serde(default)]
    pub note: Option<String>,
}

/// The invocation-plan manifest shipped with a gtest suite (`manifest.json`).
///
/// Tests are assigned to the first entry whose `filter` matches (manifest
/// order); unmatched tests fall back to one process per test with the global
/// `--timeout`.
#[derive(serde::Deserialize)]
pub struct GtestManifest {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_factor")]
    pub default_factor: f64,
    #[serde(default = "default_min_seconds")]
    pub min_seconds: f64,
    #[serde(default)]
    pub invocations: Vec<InvocationSpec>,
}

fn default_version() -> u32 {
    1
}

fn default_factor() -> f64 {
    3.0
}

fn default_min_seconds() -> f64 {
    10.0
}

/// Legacy `budgets.json` shape: a flat map of test-name patterns to
/// C-baseline seconds, all executed per-case.
#[derive(serde::Deserialize)]
struct LegacyBudgets {
    #[serde(default = "default_factor")]
    default_factor: f64,
    #[serde(default = "default_min_seconds")]
    min_seconds: f64,
    #[serde(default)]
    baselines: std::collections::HashMap<String, f64>,
}

impl GtestManifest {
    /// Timeout in seconds granted to an invocation with the given C baseline.
    fn timeout_secs(&self, budget: f64) -> u64 {
        (budget * self.default_factor).max(self.min_seconds).ceil() as u64
    }
}

/// Loads the invocation plan: `manifest.json` if present, else a legacy
/// `budgets.json` converted to an all-`case` plan. Malformed files are logged
/// and ignored (everything falls back to the global timeout).
fn load_gtest_manifest(suite_dir: &Path) -> Option<GtestManifest> {
    let manifest_path = suite_dir.join(MANIFEST_FILE);
    if let Ok(raw) = fs::read_to_string(&manifest_path) {
        match serde_json::from_str::<GtestManifest>(&raw) {
            Ok(m) => {
                log::info!("Using invocation plan from {}", MANIFEST_FILE);
                return Some(m);
            }
            Err(e) => {
                log::warn!("Ignoring malformed {}: {}", manifest_path.display(), e);
                return None;
            }
        }
    }

    let budgets_path = suite_dir.join(BUDGETS_FILE);
    let raw = fs::read_to_string(&budgets_path).ok()?;
    match serde_json::from_str::<LegacyBudgets>(&raw) {
        Ok(b) => {
            log::info!("Using legacy per-test budgets from {}", BUDGETS_FILE);
            // Exact patterns win over wildcards in the legacy scheme; keep
            // that by ordering exact entries first.
            let mut entries: Vec<(String, f64)> = b.baselines.into_iter().collect();
            entries.sort_by_key(|(k, _)| (k.contains('*'), k.clone()));
            Some(GtestManifest {
                version: 1,
                default_factor: b.default_factor,
                min_seconds: b.min_seconds,
                invocations: entries
                    .into_iter()
                    .map(|(filter, budget)| InvocationSpec {
                        mode: InvocationMode::Case,
                        filter,
                        budget,
                        note: None,
                    })
                    .collect(),
            })
        }
        Err(e) => {
            log::warn!("Ignoring malformed {}: {}", budgets_path.display(), e);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Filter matching (gtest positive-filter subset: `:`-separated patterns,
// `*` = any run of characters, `?` = any single character)

/// Matches one glob pattern (with `*` and `?`) against a full test name.
fn glob_matches(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star_pi, mut star_ni) = (usize::MAX, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_pi = pi;
            star_ni = ni;
            pi += 1;
        } else if star_pi != usize::MAX {
            pi = star_pi + 1;
            star_ni += 1;
            ni = star_ni;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Matches a gtest filter expression (positive patterns only) against a name.
fn filter_matches(filter: &str, name: &str) -> bool {
    filter
        .split(':')
        .filter(|p| !p.is_empty())
        .any(|p| glob_matches(p, name))
}

// ---------------------------------------------------------------------------
// Execution planning

/// One `mode: "suite"` process invocation with its assigned tests.
struct Batch {
    filter: String,
    timeout_secs: u64,
    note: Option<String>,
    tests: Vec<String>,
}

struct ExecutionPlan {
    batches: Vec<Batch>,
    /// (test name, timeout seconds)
    singles: Vec<(String, u64)>,
}

/// Assigns every listed test to the first matching manifest entry
/// (manifest order); unmatched tests run per-case with the global timeout.
fn build_execution_plan(
    test_names: &[String],
    manifest: Option<&GtestManifest>,
    fallback_timeout: u64,
) -> ExecutionPlan {
    let mut plan = ExecutionPlan {
        batches: Vec::new(),
        singles: Vec::new(),
    };
    let Some(manifest) = manifest else {
        plan.singles = test_names
            .iter()
            .map(|n| (n.clone(), fallback_timeout))
            .collect();
        return plan;
    };

    if manifest.version != 1 {
        log::warn!(
            "manifest version {} is newer than supported (1); proceeding best-effort",
            manifest.version
        );
    }
    for spec in &manifest.invocations {
        if spec.filter.contains('-') && spec.filter.contains(':') {
            log::warn!(
                "manifest filter '{}' looks like it uses negative patterns; only positive patterns are supported for planning",
                spec.filter
            );
        }
    }

    // batch_slots[i] collects tests for manifest entry i when it is a Suite.
    let mut batch_slots: Vec<Vec<String>> = vec![Vec::new(); manifest.invocations.len()];
    for name in test_names {
        match manifest
            .invocations
            .iter()
            .position(|s| filter_matches(&s.filter, name))
        {
            Some(i) => {
                let spec = &manifest.invocations[i];
                match spec.mode {
                    InvocationMode::Suite => batch_slots[i].push(name.clone()),
                    InvocationMode::Case => plan
                        .singles
                        .push((name.clone(), manifest.timeout_secs(spec.budget))),
                }
            }
            None => plan.singles.push((name.clone(), fallback_timeout)),
        }
    }

    for (i, tests) in batch_slots.into_iter().enumerate() {
        if tests.is_empty() {
            continue;
        }
        let spec = &manifest.invocations[i];
        plan.batches.push(Batch {
            filter: spec.filter.clone(),
            timeout_secs: manifest.timeout_secs(spec.budget),
            note: spec.note.clone(),
            tests,
        });
    }
    plan
}

// ---------------------------------------------------------------------------
// Execution

fn execute_plan(
    gtest_bin: &Path,
    ld_library_path: &str,
    plan: &ExecutionPlan,
) -> HarvestResult<(Vec<TestResult>, Vec<String>)> {
    let mut test_results = Vec::new();
    let mut error_messages = Vec::new();

    log::info!("Validating library outputs against GoogleTest suite...");

    for (i, batch) in plan.batches.iter().enumerate() {
        log::info!(
            "Running gtest batch '{}' ({} tests, {} of {}, timeout {}s{})...",
            batch.filter,
            batch.tests.len(),
            i + 1,
            plan.batches.len(),
            batch.timeout_secs,
            batch
                .note
                .as_deref()
                .map(|n| format!("; {}", n))
                .unwrap_or_default()
        );
        let (mut results, mut errors) = run_gtest_batch(gtest_bin, ld_library_path, batch);
        // List every batched test's verdict, the same way the per-case path
        // does, so a batch is as legible as an individually-run set.
        for r in &results {
            if r.skipped {
                log::info!("  ⏭️  Test {} skipped (GTEST_SKIP)", r.filename);
            } else if r.passed {
                log::info!("  ✅ Test {} passed", r.filename);
            } else {
                log::info!("  ❌ Test {} failed", r.filename);
            }
        }
        let passed = results.iter().filter(|r| r.passed).count();
        let all_passed = passed == results.len();
        log::info!(
            "{} Batch '{}': {}/{} passed",
            if all_passed { "✅" } else { "❌" },
            batch.filter,
            passed,
            results.len()
        );
        test_results.append(&mut results);
        error_messages.append(&mut errors);
    }

    for (i, (name, timeout_secs)) in plan.singles.iter().enumerate() {
        let timeout_duration = Duration::from_secs(*timeout_secs);
        log::info!(
            "Running gtest {} ({} of {}, timeout {}s)...",
            name,
            i + 1,
            plan.singles.len(),
            timeout_secs
        );

        match run_single_gtest(gtest_bin, ld_library_path, name, timeout_duration) {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let skipped = stdout.contains("[  SKIPPED ]");
                if output.status.success() {
                    test_results.push(TestResult {
                        filename: name.clone(),
                        passed: true,
                        skipped,
                    });
                    if skipped {
                        log::info!("Skipping gtest {} (GTEST_SKIP)", name);
                    } else {
                        log::info!("✅ Test {} passed", name);
                    }
                } else {
                    test_results.push(TestResult {
                        filename: name.clone(),
                        passed: false,
                        skipped: false,
                    });
                    let error = format!(
                        "gtest {} failed: status {:?}\nstdout:\n{}\nstderr:\n{}",
                        name,
                        output.status.code(),
                        stdout,
                        String::from_utf8_lossy(&output.stderr)
                    );
                    error_messages.push(error.clone());
                    log::info!("❌ Test {} failed", name);
                }
            }
            Err(e) => {
                test_results.push(TestResult {
                    filename: name.clone(),
                    passed: false,
                    skipped: false,
                });
                let error = format!("gtest {} failed: {}", name, e);
                error_messages.push(error.clone());
                log::info!("❌ {}", error);
            }
        }
    }

    Ok((test_results, error_messages))
}

/// Verdict for one test parsed from a gtest JSON report.
struct JsonVerdict {
    passed: bool,
    skipped: bool,
    failure_text: String,
}

/// Runs one `mode: "suite"` batch invocation and ingests per-case verdicts
/// from its JSON report. If the process dies without writing a report (crash,
/// timeout), every planned test is recorded as failed — never skipped — so a
/// batch death cannot score better than individual failures.
fn run_gtest_batch(
    gtest_bin: &Path,
    ld_library_path: &str,
    batch: &Batch,
) -> (Vec<TestResult>, Vec<String>) {
    let json_path = gtest_bin
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("harvest_batch_report.json");
    let _ = fs::remove_file(&json_path);

    let run = run_gtest_process(
        gtest_bin,
        ld_library_path,
        &[
            format!("--gtest_filter={}", batch.filter),
            format!("--gtest_output=json:{}", json_path.display()),
        ],
        Duration::from_secs(batch.timeout_secs),
    );

    let mut results = Vec::new();
    let mut errors = Vec::new();

    let fail_all = |reason: &str, results: &mut Vec<TestResult>, errors: &mut Vec<String>| {
        for name in &batch.tests {
            results.push(TestResult {
                filename: name.clone(),
                passed: false,
                skipped: false,
            });
        }
        errors.push(format!(
            "gtest batch '{}' produced no per-test report: {} ({} tests recorded as failed)",
            batch.filter,
            reason,
            batch.tests.len()
        ));
    };

    let verdicts = match fs::read_to_string(&json_path) {
        Ok(raw) => match parse_gtest_json_report(&raw) {
            Ok(v) => v,
            Err(e) => {
                fail_all(&format!("unparseable JSON report: {}", e), &mut results, &mut errors);
                return (results, errors);
            }
        },
        Err(_) => {
            let reason = match &run {
                Ok(output) => format!(
                    "process exited with {:?} without writing a report (crash mid-run?)\nstderr tail:\n{}",
                    output.status.code(),
                    tail_of(&String::from_utf8_lossy(&output.stderr), 2000),
                ),
                Err(e) => e.to_string(),
            };
            fail_all(&reason, &mut results, &mut errors);
            return (results, errors);
        }
    };

    let mut seen = std::collections::HashSet::new();
    for name in &batch.tests {
        match verdicts.iter().find(|(n, _)| n == name) {
            Some((_, v)) => {
                seen.insert(name.clone());
                results.push(TestResult {
                    filename: name.clone(),
                    passed: v.passed,
                    skipped: v.skipped,
                });
                if !v.passed {
                    errors.push(format!("gtest {} failed:\n{}", name, v.failure_text));
                }
            }
            None => {
                results.push(TestResult {
                    filename: name.clone(),
                    passed: false,
                    skipped: false,
                });
                errors.push(format!(
                    "gtest {} has no verdict in the batch report (run ended early?)",
                    name
                ));
            }
        }
    }
    // Tests the report contains but the planner did not expect: count them
    // too (the binary's own filter matching is authoritative), with a warning.
    for (name, v) in &verdicts {
        if !batch.tests.contains(name) && !seen.contains(name) {
            log::warn!(
                "Batch '{}' reported unplanned test {} (filter matched more than planned)",
                batch.filter,
                name
            );
            results.push(TestResult {
                filename: name.clone(),
                passed: v.passed,
                skipped: v.skipped,
            });
            if !v.passed {
                errors.push(format!("gtest {} failed:\n{}", name, v.failure_text));
            }
        }
    }

    (results, errors)
}

fn tail_of(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("...{}", &s[s.len() - max..])
    }
}

/// Parses a gtest `--gtest_output=json:` report into (full test name, verdict)
/// pairs. Tolerates the three historical spellings of the failure text key
/// (`failure`, `failures`, `message`).
fn parse_gtest_json_report(raw: &str) -> Result<Vec<(String, JsonVerdict)>, String> {
    let root: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("invalid JSON: {}", e))?;
    let mut out = Vec::new();
    let suites = root
        .get("testsuites")
        .and_then(|v| v.as_array())
        .ok_or("missing testsuites array")?;
    for suite in suites {
        let suite_name = suite.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let Some(tests) = suite.get("testsuite").and_then(|v| v.as_array()) else {
            continue;
        };
        for t in tests {
            let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let full = format!("{}.{}", suite_name, name);
            let failures = t.get("failures").and_then(|v| v.as_array());
            let failed = failures.map(|a| !a.is_empty()).unwrap_or(false);
            let skipped = t.get("result").and_then(|v| v.as_str()) == Some("SKIPPED")
                || t.get("skipped")
                    .and_then(|v| v.as_array())
                    .map(|a| !a.is_empty())
                    .unwrap_or(false);
            let failure_text = failures
                .map(|a| {
                    a.iter()
                        .filter_map(|f| {
                            f.get("failure")
                                .or_else(|| f.get("message"))
                                .or_else(|| f.get("failures"))
                                .and_then(|v| v.as_str())
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            out.push((
                full,
                JsonVerdict {
                    passed: !failed,
                    skipped,
                    failure_text,
                },
            ));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Process plumbing

/// Configures and builds the gtest suite, returning the test binary path.
fn build_gtest_suite(
    output_dir: &Path,
    suite_dir: &Path,
    lib_path: &Path,
) -> HarvestResult<PathBuf> {
    let build_dir = output_dir.join(GTEST_BUILD_SUBDIR);
    fs::create_dir_all(&build_dir)?;

    log::info!("Configuring GoogleTest suite...");
    let output = Command::new("cmake")
        .arg("-S")
        .arg(suite_dir)
        .arg("-B")
        .arg(&build_dir)
        .arg("-DCMAKE_BUILD_TYPE=Release")
        .arg(format!("-DTEST_LIB_PATH={}", lib_path.display()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to run cmake configure: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "cmake configure failed for {}:\n{}",
            suite_dir.display(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    log::info!("Building GoogleTest suite...");
    let output = Command::new("cmake")
        .arg("--build")
        .arg(&build_dir)
        .arg("--parallel")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to run cmake build: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "GoogleTest suite build failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let gtest_bin = build_dir.join(GTEST_BINARY_NAME);
    if !gtest_bin.exists() {
        return Err(format!(
            "GoogleTest binary not found at {} (the suite must define an executable target named '{}')",
            gtest_bin.display(),
            GTEST_BINARY_NAME
        )
        .into());
    }
    Ok(gtest_bin.canonicalize().unwrap_or(gtest_bin))
}

/// Enumerates test names via `--gtest_list_tests`.
///
/// Listing format: suite lines start at column 0 and end with `.`; test lines
/// are indented. Both may carry trailing `# TypeParam/GetParam` comments.
fn list_gtest_tests(gtest_bin: &Path, ld_library_path: &str) -> HarvestResult<Vec<String>> {
    let output = Command::new(gtest_bin)
        .arg("--gtest_list_tests")
        .env(LD_LIBRARY_PATH_ENV, ld_library_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to list gtest tests: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "--gtest_list_tests failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut tests = Vec::new();
    let mut suite = String::new();
    for line in stdout.lines() {
        let entry = line.split('#').next().unwrap_or("");
        if entry.trim().is_empty() {
            continue;
        }
        if !entry.starts_with(' ') {
            // Suite lines end with '.'; skip any other preamble (e.g. the
            // "Running main() from gtest_main.cc" banner).
            if entry.trim_end().ends_with('.') {
                suite = entry.trim().to_string();
            }
        } else if !suite.is_empty() {
            tests.push(format!("{}{}", suite, entry.trim()));
        }
    }

    if tests.is_empty() {
        return Err("gtest suite lists no tests".to_string().into());
    }
    Ok(tests)
}

/// Spawns the gtest binary with the given extra args and a hard deadline.
///
/// stdout/stderr are drained on background threads WHILE waiting: a batch
/// invocation prints thousands of `[ RUN ]/[ OK ]` lines, far beyond the OS
/// pipe buffer, and a wait-then-read sequence would deadlock (the child
/// blocks on a full pipe, the parent "times out" a perfectly healthy run).
fn run_gtest_process(
    gtest_bin: &Path,
    ld_library_path: &str,
    args: &[String],
    timeout: Duration,
) -> HarvestResult<Output> {
    use std::io::Read;

    let mut child = Command::new(gtest_bin)
        .args(args)
        .env(LD_LIBRARY_PATH_ENV, ld_library_path)
        .env(
            "HARVEST_TEST_SOFT_TIMEOUT_SECS",
            timeout.as_secs().to_string(),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn gtest binary: {}", e))?;

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let stdout_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut p) = stdout_pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut p) = stderr_pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let wait_result = child.wait_timeout(timeout + Duration::from_secs(KILL_GRACE_SECS));
    let timed_out = matches!(wait_result, Ok(None));
    if timed_out {
        let _ = child.kill();
    }
    let status = child.wait();
    // The readers reach EOF once the child (and any stray descendants holding
    // the pipe) are gone; join AFTER the kill so they cannot hang us.
    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();

    if timed_out {
        return Err(format!(
            "Invocation exceeded its {}s budget (killed {}s later)",
            timeout.as_secs(),
            KILL_GRACE_SECS
        )
        .into());
    }
    match (wait_result, status) {
        (Ok(Some(_)), Ok(status)) | (_, Ok(status)) => Ok(Output {
            status,
            stdout,
            stderr,
        }),
        (_, Err(e)) => Err(format!("Error waiting for gtest: {}", e).into()),
    }
}

/// Runs a single test in its own process via `--gtest_filter`.
fn run_single_gtest(
    gtest_bin: &Path,
    ld_library_path: &str,
    test_name: &str,
    timeout: Duration,
) -> HarvestResult<Output> {
    run_gtest_process(
        gtest_bin,
        ld_library_path,
        &[format!("--gtest_filter={}", test_name)],
        timeout,
    )
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_basics() {
        assert!(glob_matches("Foo.Bar", "Foo.Bar"));
        assert!(!glob_matches("Foo.Bar", "Foo.Baz"));
        assert!(glob_matches("Foo.*", "Foo.Bar"));
        assert!(glob_matches("*", "anything"));
        assert!(glob_matches("Blocks/*.Matches/*", "Blocks/Test1.Matches/Block0001"));
        assert!(glob_matches("A?C", "ABC"));
        assert!(!glob_matches("A?C", "AC"));
        assert!(glob_matches("*tail", "long tail"));
        assert!(!glob_matches("*tail", "tail wags"));
    }

    #[test]
    fn filter_multi_pattern() {
        assert!(filter_matches("A.*:B.*", "B.test"));
        assert!(!filter_matches("A.*:B.*", "C.test"));
    }

    #[test]
    fn plan_assignment_first_match_wins() {
        let manifest = GtestManifest {
            version: 1,
            default_factor: 3.0,
            min_seconds: 10.0,
            invocations: vec![
                InvocationSpec {
                    mode: InvocationMode::Suite,
                    filter: "Agg.Rec:Blocks/*".to_string(),
                    budget: 2.0,
                    note: None,
                },
                InvocationSpec {
                    mode: InvocationMode::Case,
                    filter: "Heavy.Big".to_string(),
                    budget: 84.0,
                    note: None,
                },
            ],
        };
        let tests = vec![
            "Agg.Rec".to_string(),
            "Blocks/T.M/Block0000".to_string(),
            "Heavy.Big".to_string(),
            "Plain.Simple".to_string(),
        ];
        let plan = build_execution_plan(&tests, Some(&manifest), 42);
        assert_eq!(plan.batches.len(), 1);
        assert_eq!(plan.batches[0].tests.len(), 2);
        assert_eq!(plan.batches[0].timeout_secs, 10); // floor
        assert_eq!(plan.singles.len(), 2);
        assert_eq!(plan.singles[0], ("Heavy.Big".to_string(), 252)); // 84*3
        assert_eq!(plan.singles[1], ("Plain.Simple".to_string(), 42)); // fallback
    }

    #[test]
    fn json_report_parsing() {
        let raw = r#"{
          "tests": 3,
          "testsuites": [
            {
              "name": "Blocks/T",
              "testsuite": [
                {"name": "M/Block0000", "status": "RUN", "result": "COMPLETED"},
                {"name": "M/Block0001", "status": "RUN", "result": "COMPLETED",
                 "failures": [{"failure": "boom", "type": ""}]},
                {"name": "M/Block0002", "status": "RUN", "result": "SKIPPED"}
              ]
            }
          ]
        }"#;
        let v = parse_gtest_json_report(raw).unwrap();
        assert_eq!(v.len(), 3);
        assert!(v[0].1.passed && !v[0].1.skipped);
        assert!(!v[1].1.passed);
        assert_eq!(v[1].1.failure_text, "boom");
        assert!(v[2].1.passed && v[2].1.skipped);
    }
}

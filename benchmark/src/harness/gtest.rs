//! GoogleTest suite validation for translated projects.
//!
//! The engine — manifest planning, process-level invocations with
//! process-group containment and deadline-bounded output draining, gtest
//! JSON report ingestion — lives in the `harvest-bench` runner crate (the
//! same code that scores suites in the standalone benchmark repo), pulled in
//! as a git dependency pinned in Cargo.lock. Manifest semantics live
//! entirely in the runner; a malformed manifest is a hard error (the suite
//! is harness-owned, held-out content — a broken one means corrupt
//! infrastructure, and grading on with fallback timeouts would silently
//! distort scores).
//!
//! This module is the harvest-side adapter: build the translated crate as a
//! cdylib, hand the suite to the runner's streaming API, render its events
//! as harvest's log lines (the `✅/❌ Test … ` lines downstream tooling
//! greps out of output.log), and map the final report onto harvest's
//! `TestResult` rows.
//!
//! Local development: builds normally use the git rev pinned in Cargo.lock;
//! an untracked `.cargo/config.toml` at the repo root can `[patch]` the
//! dependency to the local `harvest-bench/` checkout so runner changes take
//! effect immediately, workspace-style.

use crate::error::HarvestResult;
use crate::harness::library;
use crate::stats::TestResult;
use harvest_bench::{run_suite_streaming, Event, InvocationMode, RunConfig, TestStatus};
use harvest_core::cargo_utils::CargoToml;
use std::path::Path;
use std::process::{Command, Stdio};

/// Directory (inside a test case and inside the translated output) holding the suite
pub const GTEST_SUITE_DIR: &str = "gtest_suite";

/// Where the suite is built, relative to the translated output directory
const GTEST_BUILD_SUBDIR: &str = "target/gtest_build";

/// Validates a translated Rust project by building and running a GoogleTest
/// suite against its compiled cdylib.
///
/// `suite_dir` is the suite the snapshot carries
/// (`.harvest/suite/gtest_suite/`) and is read in place: nothing is copied next
/// to the crate, and the CMake build tree goes under the project's `target/`.
/// Keeping the suite out of the crate directory is what stops a later stage
/// from picking it up as crate content — for a suite that rounds 1 and 2 hold
/// out, that would be a leak.
///
/// # Returns
/// Tuple of (test_results, error_messages). One `TestResult` per gtest test,
/// with `filename` set to the full `Suite.Test` name.
pub fn run_gtest_validation(
    program_name: &str,
    suite_dir: &Path,
    output_dir: &Path,
    timeout: u64,
) -> HarvestResult<(Vec<TestResult>, Vec<String>)> {
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

    let config = RunConfig {
        suite_dir: suite_dir.to_path_buf(),
        lib_path,
        build_dir: output_dir.join(GTEST_BUILD_SUBDIR),
        fallback_timeout: timeout,
    };
    let report = run_suite_streaming(&config, harvest_event_renderer())
        .map_err(|e| format!("GoogleTest validation failed: {}", e))?;

    let mut test_results = Vec::new();
    let mut error_messages = Vec::new();
    for v in report.verdicts {
        let (passed, skipped) = match v.status {
            TestStatus::Passed => (true, false),
            TestStatus::Skipped => (true, true),
            TestStatus::Failed(_) => (false, false),
        };
        if !passed {
            error_messages.push(format!("gtest {} failed:\n{}", v.name, v.failure));
        }
        test_results.push(TestResult {
            filename: v.name,
            passed,
            skipped,
        });
    }
    Ok((test_results, error_messages))
}

/// Renders runner events as harvest's log lines. The `✅/❌ Test … ` and
/// batch-summary formats are load-bearing: failure-set diffs and the trace
/// tooling grep them out of output.log, so they must not drift when the
/// engine underneath changes. Batches and per-case invocations keep their
/// historical separate numbering.
fn harvest_event_renderer() -> impl FnMut(&Event) {
    let mut batch_total = 0usize;
    let mut case_total = 0usize;
    let mut batch_i = 0usize;
    let mut case_i = 0usize;
    let mut in_batch = false;

    move |event: &Event| match event {
        Event::SuiteBuildStarted { .. } => {
            log::info!("Configuring and building GoogleTest suite...");
        }
        Event::SuiteBuilt { gtest_bin } => {
            log::info!("GoogleTest suite built at: {}", gtest_bin.display());
        }
        Event::TestsDiscovered { count } => {
            log::info!("Discovered {} GoogleTest test(s)", count);
        }
        Event::PlanReady {
            plan,
            from_manifest,
        } => {
            if *from_manifest {
                log::info!("Using invocation plan from manifest.json");
            }
            batch_total = plan
                .invocations
                .iter()
                .filter(|i| i.mode == InvocationMode::Suite)
                .count();
            case_total = plan.invocations.len() - batch_total;
            log::info!(
                "Execution plan: {} batch invocation(s), {} per-case invocation(s)",
                batch_total,
                case_total
            );
            log::info!("Validating library outputs against GoogleTest suite...");
        }
        Event::InvocationStarted { invocation, .. } => match invocation.mode {
            InvocationMode::Suite => {
                batch_i += 1;
                in_batch = true;
                log::info!(
                    "Running gtest batch '{}' ({} tests, {} of {}, timeout {}s{})...",
                    invocation.filter,
                    invocation.tests.len(),
                    batch_i,
                    batch_total,
                    invocation.timeout_secs,
                    invocation
                        .note
                        .as_deref()
                        .map(|n| format!("; {}", n))
                        .unwrap_or_default()
                );
            }
            InvocationMode::Case => {
                case_i += 1;
                in_batch = false;
                log::info!(
                    "Running gtest {} ({} of {}, timeout {}s)...",
                    invocation.filter,
                    case_i,
                    case_total,
                    invocation.timeout_secs
                );
            }
        },
        Event::TestFinished { verdict } => {
            // Batched tests are indented under their batch header; per-case
            // tests keep the historical unindented lines.
            if in_batch {
                match verdict.status {
                    TestStatus::Skipped => {
                        log::info!("  ⏭️  Test {} skipped (GTEST_SKIP)", verdict.name)
                    }
                    TestStatus::Passed => log::info!("  ✅ Test {} passed", verdict.name),
                    TestStatus::Failed(_) => log::info!("  ❌ Test {} failed", verdict.name),
                }
            } else {
                match verdict.status {
                    TestStatus::Skipped => {
                        log::info!("Skipping gtest {} (GTEST_SKIP)", verdict.name)
                    }
                    TestStatus::Passed => log::info!("✅ Test {} passed", verdict.name),
                    TestStatus::Failed(_) => log::info!("❌ Test {} failed", verdict.name),
                }
            }
        }
        Event::InvocationFinished {
            invocation,
            outcome,
            ..
        } => {
            if invocation.mode == InvocationMode::Suite {
                // Historically "passed" here included skips (they count as
                // non-failures for the batch verdict line).
                let passed = outcome.passed + outcome.skipped;
                log::info!(
                    "{} Batch '{}': {}/{} passed",
                    if passed == outcome.total { "✅" } else { "❌" },
                    invocation.filter,
                    passed,
                    outcome.total
                );
            }
        }
        _ => {}
    }
}

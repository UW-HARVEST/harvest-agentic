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
pub const GTEST_SUITE_DIR: &str = full_source::TestSuiteKind::Gtest.dirs()[0];

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
///
/// Per-case invocations always print their own line. Inside a batch only the
/// tests that need attention (failed or skipped) are printed, followed by a
/// count of the silent passes.
fn harvest_event_renderer_to(mut emit: impl FnMut(String)) -> impl FnMut(&Event) {
    let mut batch_total = 0usize;
    let mut case_total = 0usize;
    let mut batch_i = 0usize;
    let mut case_i = 0usize;
    let mut in_batch = false;
    // Tests of the current batch worth printing, held until the batch ends.
    let mut noteworthy: Vec<String> = Vec::new();

    move |event: &Event| match event {
        Event::SuiteBuildStarted { .. } => {
            emit("Configuring and building GoogleTest suite...".to_string());
        }
        Event::SuiteBuilt { gtest_bin } => {
            emit(format!("GoogleTest suite built at: {}", gtest_bin.display()));
        }
        Event::TestsDiscovered { count } => {
            emit(format!("Discovered {} GoogleTest test(s)", count));
        }
        Event::PlanReady {
            plan,
            from_manifest,
        } => {
            if *from_manifest {
                emit("Using invocation plan from manifest.json".to_string());
            }
            batch_total = plan
                .invocations
                .iter()
                .filter(|i| i.mode == InvocationMode::Suite)
                .count();
            case_total = plan.invocations.len() - batch_total;
            emit(format!(
                "Execution plan: {} batch invocation(s), {} per-case invocation(s)",
                batch_total, case_total
            ));
            emit("Validating library outputs against GoogleTest suite...".to_string());
        }
        Event::InvocationStarted { invocation, .. } => match invocation.mode {
            InvocationMode::Suite => {
                batch_i += 1;
                in_batch = true;
                noteworthy.clear();
                emit(format!(
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
                ));
            }
            InvocationMode::Case => {
                case_i += 1;
                in_batch = false;
                emit(format!(
                    "Running gtest {} ({} of {}, timeout {}s)...",
                    invocation.filter, case_i, case_total, invocation.timeout_secs
                ));
            }
        },
        Event::TestFinished { verdict } => {
            if in_batch {
                // Batched tests are indented under their batch header, and
                // only the ones worth attention are kept: the passes become a
                // single count when the batch ends.
                match verdict.status {
                    TestStatus::Skipped => noteworthy.push(format!(
                        "  \u{23ed}\u{fe0f}  Test {} skipped (GTEST_SKIP)",
                        verdict.name
                    )),
                    TestStatus::Passed => {}
                    TestStatus::Failed(_) => {
                        noteworthy.push(format!("  \u{274c} Test {} failed", verdict.name))
                    }
                }
            } else {
                // Per-case invocations keep the historical unindented lines,
                // one per test, whatever the verdict.
                match verdict.status {
                    TestStatus::Skipped => {
                        emit(format!("Skipping gtest {} (GTEST_SKIP)", verdict.name))
                    }
                    TestStatus::Passed => {
                        emit(format!("\u{2705} Test {} passed", verdict.name))
                    }
                    TestStatus::Failed(_) => {
                        emit(format!("\u{274c} Test {} failed", verdict.name))
                    }
                }
            }
        }
        Event::InvocationFinished {
            invocation,
            outcome,
            ..
        } => {
            if invocation.mode == InvocationMode::Suite {
                for line in noteworthy.drain(..) {
                    emit(line);
                }
                if outcome.passed > 0 && outcome.passed < outcome.total {
                    emit(format!("  \u{2705} {} other test(s) passed", outcome.passed));
                }
                // Historically "passed" here included skips (they count as
                // non-failures for the batch verdict line).
                let passed = outcome.passed + outcome.skipped;
                emit(format!(
                    "{} Batch '{}': {}/{} passed",
                    if passed == outcome.total {
                        "\u{2705}"
                    } else {
                        "\u{274c}"
                    },
                    invocation.filter,
                    passed,
                    outcome.total
                ));
            }
        }
        _ => {}
    }
}

/// The renderer used in production: every line goes to the log.
fn harvest_event_renderer() -> impl FnMut(&Event) {
    harvest_event_renderer_to(|line| log::info!("{}", line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use harvest_bench::{ExitKind, InvocationOutcome, PlannedInvocation, TestVerdict};

    fn invocation(mode: InvocationMode, filter: &str, tests: usize) -> PlannedInvocation {
        PlannedInvocation {
            mode,
            filter: filter.to_string(),
            tests: (0..tests).map(|i| format!("{}.T{}", filter, i)).collect(),
            timeout_secs: 10,
            note: None,
        }
    }

    fn verdict(name: &str, status: TestStatus) -> TestVerdict {
        TestVerdict {
            name: name.to_string(),
            status,
            failure: String::new(),
            time_secs: None,
            invocation: 0,
        }
    }

    fn outcome(passed: usize, skipped: usize, total: usize) -> InvocationOutcome {
        InvocationOutcome {
            exit: ExitKind::Clean { code: Some(0) },
            duration_secs: 0.0,
            stderr_tail: String::new(),
            passed,
            skipped,
            total,
        }
    }

    /// Drives the renderer over one invocation's events and returns its lines.
    fn render(inv: &PlannedInvocation, verdicts: &[TestVerdict], out: &InvocationOutcome) -> Vec<String> {
        let mut lines = Vec::new();
        {
            let mut render = harvest_event_renderer_to(|line| lines.push(line));
            render(&Event::InvocationStarted {
                index: 0,
                total: 1,
                invocation: inv,
            });
            for v in verdicts {
                render(&Event::TestFinished { verdict: v });
            }
            render(&Event::InvocationFinished {
                index: 0,
                total: 1,
                invocation: inv,
                outcome: out,
            });
        }
        lines
    }

    #[test]
    fn all_green_batch_prints_only_its_summary() {
        let inv = invocation(InvocationMode::Suite, "Suite.*", 3);
        let verdicts: Vec<_> = (0..3)
            .map(|i| verdict(&format!("Suite.T{}", i), TestStatus::Passed))
            .collect();
        let lines = render(&inv, &verdicts, &outcome(3, 0, 3));
        // Header plus one summary line: no per-test lines at all.
        assert_eq!(lines.len(), 2, "{:?}", lines);
        assert!(lines[0].starts_with("Running gtest batch"), "{:?}", lines);
        assert!(lines[1].contains("Batch 'Suite.*': 3/3 passed"), "{:?}", lines);
    }

    #[test]
    fn failing_batch_prints_the_failures_and_counts_the_rest() {
        let inv = invocation(InvocationMode::Suite, "Suite.*", 4);
        let verdicts = vec![
            verdict("Suite.T0", TestStatus::Passed),
            verdict("Suite.T1", TestStatus::Failed(harvest_bench::FailureCause::Assertion)),
            verdict("Suite.T2", TestStatus::Passed),
            verdict("Suite.T3", TestStatus::Skipped),
        ];
        let lines = render(&inv, &verdicts, &outcome(2, 1, 4));
        let joined = lines.join("\n");
        assert!(joined.contains("Test Suite.T1 failed"), "{}", joined);
        assert!(joined.contains("Test Suite.T3 skipped"), "{}", joined);
        assert!(joined.contains("2 other test(s) passed"), "{}", joined);
        // The passing tests are never named.
        assert!(!joined.contains("Suite.T0"), "{}", joined);
        assert!(!joined.contains("Suite.T2"), "{}", joined);
    }

    #[test]
    fn per_case_invocations_always_print_each_test() {
        let inv = invocation(InvocationMode::Case, "Solo.Test", 1);
        let verdicts = vec![verdict("Solo.Test", TestStatus::Passed)];
        let lines = render(&inv, &verdicts, &outcome(1, 0, 1));
        let joined = lines.join("\n");
        assert!(joined.contains("Test Solo.Test passed"), "{}", joined);
        // Per-case invocations get no batch summary line.
        assert!(!joined.contains("Batch"), "{}", joined);
    }

    #[test]
    fn batch_state_does_not_leak_into_the_next_invocation() {
        let mut lines = Vec::new();
        let failing = invocation(InvocationMode::Suite, "A.*", 1);
        let clean = invocation(InvocationMode::Suite, "B.*", 1);
        {
            let mut render = harvest_event_renderer_to(|line| lines.push(line));
            let bad = verdict("A.T0", TestStatus::Failed(harvest_bench::FailureCause::Assertion));
            let good = verdict("B.T0", TestStatus::Passed);
            let bad_out = outcome(0, 0, 1);
            let good_out = outcome(1, 0, 1);
            render(&Event::InvocationStarted { index: 0, total: 2, invocation: &failing });
            render(&Event::TestFinished { verdict: &bad });
            render(&Event::InvocationFinished { index: 0, total: 2, invocation: &failing, outcome: &bad_out });
            render(&Event::InvocationStarted { index: 1, total: 2, invocation: &clean });
            render(&Event::TestFinished { verdict: &good });
            render(&Event::InvocationFinished { index: 1, total: 2, invocation: &clean, outcome: &good_out });
        }
        // The failure belongs to the first batch only.
        let second = lines.iter().position(|l| l.contains("'B.*'")).unwrap();
        assert!(!lines[second..].iter().any(|l| l.contains("A.T0")), "{:?}", lines);
    }
}

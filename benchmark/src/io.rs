use crate::error::HarvestResult;
use crate::stats::{ProgramEvalStats, SummaryStats};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Record program directories
pub fn log_found_programs(program_dirs: &[PathBuf], input_dir: &Path) -> HarvestResult<()> {
    if program_dirs.is_empty() {
        log::info!("No program directories found in: {}", input_dir.display());
        return Ok(());
    }

    log::info!(
        "\nFound {} program directories to process:",
        program_dirs.len()
    );
    for dir in program_dirs {
        log::info!("  - {}", dir.file_name().unwrap().to_string_lossy());
    }

    Ok(())
}

/// Write CSV results to file
pub fn write_csv_results(file_path: &PathBuf, results: &[ProgramEvalStats]) -> HarvestResult<()> {
    let mut wtr = csv::Writer::from_path(file_path)?;

    // Write header
    wtr.write_record([
        "program_name",
        "translation_success",
        "rust_build_success",
        "total_tests",
        "passed_tests",
        "skipped_tests",
        "success_rate",
        "error_message",
    ])?;

    // Write data
    for result in results {
        wtr.write_record([
            &result.program_name,
            &result.translation_success.to_string(),
            &result.rust_build_success.to_string(),
            &result.total_tests.to_string(),
            &result.passed_tests.to_string(),
            &result.skipped_tests.to_string(),
            &format!("{:.2}", result.success_rate()),
            result.error_message.as_deref().unwrap_or(""),
        ])?;
    }

    wtr.flush()?;
    Ok(())
}

/// Check that the input directory exists
pub fn validate_input_directory(input_dir: &Path) -> HarvestResult<()> {
    if !input_dir.exists() || !input_dir.is_dir() {
        return Err(format!(
            "Input directory does not exist or is not a directory: {}",
            input_dir.display()
        )
        .into());
    }
    Ok(())
}

/// Create output directory if it doesn't exist
pub fn ensure_output_directory(output_dir: &PathBuf) -> HarvestResult<()> {
    if !output_dir.exists() {
        std::fs::create_dir_all(output_dir).map_err(|e| -> Box<dyn std::error::Error> {
            format!(
                "Failed to create output directory: {}. Error: {}",
                output_dir.display(),
                e
            )
            .into()
        })?;
    }
    Ok(())
}

/// Find all immediate child directories of the input directory that look
/// like test cases (contain a `test_case/` subdirectory). Other directories
/// (documentation, build trees, stray checkouts) are skipped with a log line
/// instead of aborting the whole run when their contract check fails.
pub fn collect_program_dirs(input_dir: &PathBuf) -> HarvestResult<Vec<PathBuf>> {
    let mut program_dirs = Vec::new();
    for entry in std::fs::read_dir(input_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("test_case").is_dir() {
            program_dirs.push(path);
        } else {
            log::info!("Skipping {} (no test_case/ subdirectory)", path.display());
        }
    }
    // Standardize order before returning
    program_dirs.sort();
    Ok(program_dirs)
}

/// Human readable logging of final summary statistics
pub fn log_summary_stats(summary: &SummaryStats) {
    // Print final summary
    log::info!("\n{}", "=".repeat(80));
    log::info!("FINAL SUMMARY - All Benchmark Processing Complete!");
    log::info!("{}", "=".repeat(80));

    log::info!("\nSummary Statistics:");
    log::info!("  Total programs processed: {}", summary.num_programs);
    log::info!(
        "  Successful translations: {} ({:.1}%)",
        summary.successful_translations,
        summary.translation_success_rate()
    );
    log::info!(
        "  Successful Rust builds: {} ({:.1}%)",
        summary.successful_rust_builds,
        summary.rust_build_success_rate()
    );

    // Headline first, and it is the granularity-invariant one. Suites in this
    // benchmark differ in granularity by orders of magnitude (libpng exposes
    // four coarse entry points; pcre2 hundreds of fine tests), so a pooled
    // per-test average is not a meaningful quantity to lead with.
    log::info!("\nTest Results:");
    log::info!(
        "  Programs passing their whole suite: {}/{} ({:.1}%)",
        summary.programs_all_tests_passed,
        summary.num_programs,
        summary.project_pass_rate()
    );

    log::info!("\n  Per-test totals (context only — see below):");
    log::info!("    Total test cases: {}", summary.total_tests);
    log::info!("    Skipped: {} ⏭️", summary.total_skipped_tests);
    log::info!("    Passed: {} ✅", summary.total_passed_tests);
    log::info!("    Failed: {} ❌", summary.failed_tests());
    log::info!(
        "    Pooled per-test rate: {:.1}% (granularity-sensitive)",
        summary.overall_success_rate()
    );
    if summary.max_evaluated_tests > 0 {
        log::info!(
            "    Evaluated tests per program ranged {}..{} — the pooled rate weights \
             each program by its suite's granularity, so quote the program figure above",
            summary.min_evaluated_tests,
            summary.max_evaluated_tests
        );
    }
}

/// Log programs that have issues (translation failures, build failures, or test failures)
pub fn log_failing_programs(results: &[ProgramEvalStats]) {
    let failed_examples: Vec<_> = results
        .iter()
        .filter(|r| !r.translation_success || !r.rust_build_success || r.failed_tests() > 0)
        .collect();

    if !failed_examples.is_empty() {
        log::info!("\n⚠️  Examples with issues:");
        for example in failed_examples {
            log::info!("  - {}: ", example.program_name);
            if !example.translation_success {
                log::info!("    ❌ Translation failed");
            }
            if !example.rust_build_success {
                log::info!("    ❌ Rust build failed");
            }
            if example.failed_tests() > 0 {
                log::info!(
                    "    ⚠️  Tests: {}/{} passed ({} skipped, {:.1}%)",
                    example.passed_tests,
                    example.evaluated_tests(),
                    example.skipped_tests,
                    example.success_rate()
                );
            }
        }
    }
}

/// Write error messages to a file
pub fn write_error_file(file_path: &PathBuf, error_messages: &[String]) -> HarvestResult<()> {
    let mut file = File::create(file_path)?;
    for error in error_messages {
        writeln!(file, "{}", error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_program_dirs_keeps_only_test_case_children() {
        let root = tempfile::tempdir().expect("create tempdir");
        std::fs::create_dir_all(root.path().join("prog_a/test_case")).unwrap();
        std::fs::create_dir_all(root.path().join("prog_b/test_case")).unwrap();
        std::fs::create_dir_all(root.path().join("docs")).unwrap();
        std::fs::write(root.path().join("README.md"), "x").unwrap();

        let dirs = collect_program_dirs(&root.path().to_path_buf()).unwrap();
        let names: Vec<_> = dirs
            .iter()
            .map(|d| d.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["prog_a", "prog_b"]);
    }
}

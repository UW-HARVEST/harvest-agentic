mod cli;
mod error;
mod harness;
mod io;
mod ir_utils;
mod logger;
mod runner;
mod stats;
use crate::cli::{Args, TestHarness};
use crate::error::HarvestResult;
use crate::harness::{
    cleanup_benchmarks, parse_benchmark_dir, parse_test_vectors, validate_binary_output,
};
use crate::io::{
    collect_program_dirs, ensure_output_directory, log_failing_programs, log_found_programs,
    log_summary_stats, validate_input_directory, write_csv_results, write_error_file,
};
use crate::ir_utils::{cargo_build_result, raw_cargo_package, raw_source};
use crate::logger::TeeLogger;
use crate::stats::{ProgramEvalStats, SummaryStats, TestResult};
use build_project_spec::{detect_project_kind, ProjectKind};
use clap::Parser;
use harvest_core::config::{AgentKind, Stage};
use harvest_core::stage_manifest::{hash_dir, StageManifest};
use harvest_core::utils::get_version;
use harvest_core::HarvestIR;
use harvest_translate::{transpile, util::set_user_only_umask};
use regex::Regex;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Encapsulate important results from transpilation
pub struct TranspilationResult {
    translation_success: bool,
    build_success: bool,
    rust_binary_path: Option<PathBuf>,
    build_error: Option<String>,
    /// Top-level entries of the final CargoPackage, recorded in the stage
    /// manifest so a later run can reconstruct the package from the output.
    package_entries: Vec<String>,
}

impl TranspilationResult {
    /// Extract relevant info from HarvestIR
    pub fn from_ir(ir: &HarvestIR) -> Self {
        let translation_success = raw_cargo_package(ir).is_ok();
        let package_entries = raw_cargo_package(ir)
            .map(|dir| {
                dir.toplevel_entries()
                    .map(|n| n.to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        let (build_success, rust_binary_path, build_error) = match cargo_build_result(ir) {
            Ok(artifacts) => {
                if artifacts.is_empty() {
                    // Empty artifacts list indicates build succeeded but produced no output
                    (
                        false,
                        None,
                        Some("Build succeeded but produced no artifacts".to_string()),
                    )
                } else {
                    // The framework's contract (set_bin_driver in cargo_utils) is that
                    // the entry-point binary is named "driver". Pick that artifact
                    // explicitly — taking the first executable is unsafe because the
                    // verify agent can leave debug binaries in src/bin/, which cargo
                    // builds and which can be ordered before driver in the artifact list.
                    let driver = artifacts.iter().find_map(|a| {
                        if a.target.name == "driver" {
                            a.executable.as_ref().map(|e| e.as_std_path().into())
                        } else {
                            None
                        }
                    });
                    (true, driver, None)
                }
            }
            Err(err) => (false, None, Some(err.clone())),
        };

        Self {
            translation_success,
            build_success,
            rust_binary_path,
            build_error,
            package_entries,
        }
    }
}

/// Translates a C source directory to a Rust Cargo project using harvest_translate
#[allow(clippy::too_many_arguments)]
pub fn translate_c_directory_to_rust_project(
    input_dir: &Path,
    output_dir: &Path,
    config_overrides: &[String],
    modular: bool,
    stages: &[Stage],
    stage_input: Option<&Path>,
    agentic_agent: Option<AgentKind>,
    agent_tools: bool,
) -> TranspilationResult {
    let args: Arc<harvest_translate::cli::Args> = harvest_translate::cli::Args {
        input: Some(input_dir.to_path_buf()),
        output: Some(output_dir.to_path_buf()),
        print_config_path: false,
        config: config_overrides.to_vec(),
        force: false,
        modular,
        agentic: (!stages.is_empty()).then(|| stages.to_vec()),
        stage_input: stage_input.map(Path::to_path_buf),
        agentic_agent,
        agent_tools,
    }
    .into();
    let mut config = harvest_translate::cli::initialize(args).expect("Failed to generate config");
    if config.log_filter.is_empty() {
        config.log_filter = "off".to_owned(); // Disable console logging in harvest_translate
    }
    /*
    TODO: This isn't general anyway, only logs a single tool's parameters

    let tool_config = &config.tools.raw_source_to_cargo_llm;
    log::info!(
        "Translating code using {}:{} with max tokens: {}",
        tool_config.backend,
        tool_config.model,
        tool_config.max_tokens
    );*/
    match transpile(config.into()) {
        Ok(ir) => {
            match raw_source(&ir) {
                Ok(raw_c_source) => {
                    if let Err(e) = raw_c_source.materialize(output_dir.join("c_src")) {
                        log::warn!("Failed to materialize C source: {}", e);
                    }
                }
                Err(e) => log::warn!("Failed to retrieve raw C source from IR: {}", e),
            }
            TranspilationResult::from_ir(&ir)
        }
        Err(e) => {
            log::error!("Failed to transpile (full error): {:#?}", e);
            TranspilationResult {
                translation_success: false,
                build_success: false,
                rust_binary_path: None,
                build_error: Some(format!("Failed to transpile: {}", e)),
                package_entries: Vec::new(),
            }
        }
    }
}

/// Options shared by every program in a benchmark run.
pub struct RunOptions {
    pub config_overrides: Vec<String>,
    pub timeout: u64,
    pub modular: bool,
    /// Agentic stages of this run, in pipeline order. Empty = non-agentic.
    pub stages: Vec<Stage>,
    pub agent: Option<AgentKind>,
    pub agent_tools: bool,
    pub model: Option<String>,
    pub no_plan: bool,
    pub no_plan_file: bool,
    pub workflow: bool,
    pub test_harness: TestHarness,
    pub verify_harness: crate::cli::VerifyHarness,
    pub fuzz: bool,
}

impl RunOptions {
    fn has(&self, stage: Stage) -> bool {
        self.stages.contains(&stage)
    }

    fn prompt_mode(&self) -> &'static str {
        if self.workflow {
            "workflow"
        } else if self.no_plan {
            "no_plan"
        } else if self.no_plan_file {
            "no_plan_file"
        } else {
            "plan"
        }
    }
}

/// One program's work item: the bench test case it is translated from and
/// graded against, and (when resuming from a snapshot) the snapshot program
/// directory the first stage loads instead of translating.
pub struct ProgramRun {
    pub bench_program_dir: PathBuf,
    pub stage_input: Option<PathBuf>,
}

/// Run all benchmarks for a list of programs
pub fn run_all_benchmarks(
    program_runs: &[ProgramRun],
    output_dir: &Path,
    opts: &RunOptions,
) -> HarvestResult<Vec<ProgramEvalStats>> {
    // Process all examples
    let mut results = Vec::new();
    let total_examples = program_runs.len();

    for (i, program_run) in program_runs.iter().enumerate() {
        log::error!("\n{}", "=".repeat(80));
        log::info!("Processing example {} of {}", i + 1, total_examples);
        log::info!("{}", "=".repeat(80));

        results.push(benchmark_single_program(program_run, output_dir, opts));
    }

    Ok(results)
}

/// Stamps the output program directory with a stage manifest, making it a
/// self-describing snapshot a later run can resume from (see
/// `harvest_core::stage_manifest`). Failures are logged, not fatal: the
/// grading result of this run is unaffected.
fn write_stage_manifest(
    output_dir: &Path,
    bench_program_dir: &Path,
    stage_input: Option<&Path>,
    opts: &RunOptions,
    package_entries: Vec<String>,
) {
    // Accumulate stages across runs: resuming appends to the input snapshot's
    // history.
    let mut stages = stage_input
        .and_then(|p| StageManifest::read_from_dir(p).ok())
        .map(|m| m.stages)
        .unwrap_or_default();
    stages.extend(opts.stages.iter().copied());
    stages.sort();
    stages.dedup();

    let bench_program_dir = std::path::absolute(bench_program_dir)
        .unwrap_or_else(|_| bench_program_dir.to_path_buf());
    let test_case_hash = match hash_dir(&bench_program_dir.join("test_case")) {
        Ok(h) => h,
        Err(e) => {
            log::warn!("Failed to hash test_case/ for the stage manifest: {e}");
            String::new()
        }
    };
    let manifest = StageManifest {
        schema_version: 1,
        stages,
        agent: opts.agent.unwrap_or_default(),
        model: opts.model.clone(),
        prompt_mode: opts.prompt_mode().to_owned(),
        harvest_version: get_version().to_owned(),
        bench_program_dir,
        test_case_hash,
        package_entries,
        created_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    match manifest.write_to_dir(output_dir) {
        Ok(()) => log::info!(
            "Stage manifest written to {}",
            output_dir
                .join(harvest_core::stage_manifest::STAGE_MANIFEST_FILE)
                .display()
        ),
        Err(e) => log::warn!("Failed to write stage manifest: {e}"),
    }
}

/// Run list of tests and output result/errors
fn run_test_validation(
    binary_path: &Path,
    test_cases: &[crate::harness::TestCase],
    timeout: u64,
    output_dir: &Path,
) -> (Vec<TestResult>, Vec<String>) {
    let mut test_results = Vec::new();
    let mut error_messages = Vec::new();

    log::info!("Validating Rust binary outputs against test cases...");

    for (i, test_case) in test_cases.iter().enumerate() {
        if test_case.has_ub.is_some() {
            log::info!(
                "Skipping test case {} ({} of {})",
                test_case.filename,
                i + 1,
                test_cases.len()
            );
            test_results.push(TestResult {
                filename: test_case.filename.clone(),
                passed: true,
                skipped: true,
            });
            continue;
        }
        log::info!(
            "Running test case {} ({} of {})...",
            test_case.filename,
            i + 1,
            test_cases.len()
        );

        log::info!(
            "Validating output for test case with args: {:?} stdin: {:?}",
            test_case.argv,
            test_case.stdin,
        );

        let timeout_opt = Some(timeout);
        match validate_binary_output(binary_path, test_case, timeout_opt) {
            Ok(()) => {
                test_results.push(TestResult {
                    filename: test_case.filename.clone(),
                    passed: true,
                    skipped: false,
                });
                log::info!("✅ Test case {} passed", test_case.filename);
            }
            Err(e) => {
                test_results.push(TestResult {
                    filename: test_case.filename.clone(),
                    passed: false,
                    skipped: false,
                });
                let error = format!("Test case {} failed: {}", test_case.filename, e);
                error_messages.push(error);
                log::info!("❌ Test case {} failed: {}", test_case.filename, e);
                test_case
                    .write_to_disk(output_dir)
                    .expect("failed to write test case to disk");
            }
        }
    }

    (test_results, error_messages)
}

/// Run all benchmarks for a single program
fn benchmark_single_program(
    program_run: &ProgramRun,
    output_root_dir: &Path,
    opts: &RunOptions,
) -> ProgramEvalStats {
    let program_dir = program_run.bench_program_dir.as_path();
    let stage_input = program_run.stage_input.as_deref();
    let timeout = opts.timeout;
    let test_harness = opts.test_harness;
    let agentic = !opts.stages.is_empty();

    let program_name = program_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let mut result = ProgramEvalStats::new(&program_name);

    log::info!("Processing program: {}", program_name);
    log::info!("Bench directory: {}", program_dir.display());
    if let Some(snapshot) = stage_input {
        log::info!("Stage input (snapshot): {}", snapshot.display());
    }

    // Get program output directory
    let output_dir = output_root_dir.join(&program_name);
    log::info!("Output directory: {}", output_dir.display());

    // Check for required subdirectories & log error if we don't find them
    // We use the test_case root (not src/) so translate can see CMakeLists.txt.
    let (test_case_dir, test_vectors_dir) = match parse_benchmark_dir(program_dir) {
        Ok(dirs) => dirs,
        Err(e) => {
            result.error_message = Some(e.to_string());
            return result;
        }
    };

    // Detect project kind from the C source root (same heuristic as build_project_spec).
    let project_kind = detect_project_kind(&test_case_dir);
    log::info!(
        "Detected project type: {}",
        project_kind
            .as_ref()
            .map(|k| k.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    );

    // Parse test vectors. gtest-only test cases (gtest_suite/ and no
    // test_vectors/) define their test set in the suite instead.
    let test_cases = if test_vectors_dir.is_dir() {
        match parse_test_vectors(&test_vectors_dir) {
            Ok(vectors) => vectors,
            Err(e) => {
                result.error_message = Some(e.to_string());
                return result;
            }
        }
    } else {
        Vec::new()
    };

    result.total_tests = test_cases.len();

    // Log test case parsing success
    if !test_cases.is_empty() {
        log::info!("✅ Successfully parsed {} test case(s)", test_cases.len());
    }

    // Inject per-stage tool config for the agentic stages that actually run
    // in this invocation. Both agents share the wishlist file; the verify
    // phase appends to whatever the translate phase wrote.
    // Ensure per-program output artifacts can be written before injecting paths.
    std::fs::create_dir_all(&output_dir).ok();

    let wishlist_path = output_dir.join("tool_wishlist.json");
    // Translate-phase PLAN.md is dumped under a dedicated name so a later verify
    // rewrite of the CargoPackage cannot overwrite or delete it.
    let plan_translate_path = output_dir.join("plan_translate.md");
    // Verify-phase HYPOTHESES.md captures the agent's hypothesis log across
    // compactions, for post-hoc analysis of how it approached debugging.
    let hypotheses_verify_path = output_dir.join("hypotheses_verify.md");
    // The output.log path lets each tool append the agent's full JSON trace
    // to the same log file as benchmark messages.
    let output_log_path = output_root_dir.join("output.log");
    let mut effective_overrides = opts.config_overrides.clone();
    // Stage-uniform flags apply to every agentic stage running in this
    // invocation; use -c tools.<tool>.<key>=... for per-stage overrides.
    let stage_overrides = |overrides: &mut Vec<String>, tool: &str| {
        overrides.push(format!(
            "tools.{tool}.wishlist_output_path={}",
            wishlist_path.display()
        ));
        overrides.push(format!(
            "tools.{tool}.output_log_path={}",
            output_log_path.display()
        ));
        if let Some(m) = &opts.model {
            overrides.push(format!("tools.{tool}.model={m}"));
        }
        if opts.no_plan {
            overrides.push(format!("tools.{tool}.no_plan=true"));
        }
        if opts.no_plan_file {
            overrides.push(format!("tools.{tool}.no_plan_file=true"));
        }
        if opts.workflow {
            overrides.push(format!("tools.{tool}.workflow=true"));
        }
    };
    if opts.has(Stage::Translate) {
        stage_overrides(&mut effective_overrides, "translate_agentic");
        effective_overrides.push(format!(
            "tools.translate_agentic.plan_output_path={}",
            plan_translate_path.display()
        ));
    }
    if opts.has(Stage::Verify) {
        stage_overrides(&mut effective_overrides, "verify_fix_agentic");
        effective_overrides.push(format!(
            "tools.verify_fix_agentic.hypotheses_output_path={}",
            hypotheses_verify_path.display()
        ));
        if opts.verify_harness == crate::cli::VerifyHarness::Gtest {
            effective_overrides.push("tools.verify_fix_agentic.verify_harness=gtest".to_owned());
        }
        if opts.fuzz {
            effective_overrides.push("tools.verify_fix_agentic.fuzz=true".to_owned());
        }
    }

    // Run the stage pipeline (translate and/or verify, or the non-agentic
    // translators), starting from the C source or the snapshot.
    let translation_result = translate_c_directory_to_rust_project(
        &test_case_dir,
        &output_dir,
        &effective_overrides,
        opts.modular,
        &opts.stages,
        stage_input,
        opts.agent,
        opts.agent_tools,
    );

    result.translation_success = translation_result.translation_success;
    result.rust_build_success = translation_result.build_success;

    if translation_result.translation_success {
        log::info!("✅ Translation completed successfully!");
    } else {
        let error = format!(
            "Failed to translate C project: {:?}",
            translation_result.build_error
        );
        result.error_message = Some(error.clone());
        log::info!("❌ Translation failed");
        return result;
    }

    // The output program directory now holds a materialized CargoPackage:
    // stamp it as a resumable snapshot. Build failure does not gate this —
    // a broken translate-only snapshot is a legitimate verify-stage input.
    if agentic {
        write_stage_manifest(
            &output_dir,
            program_dir,
            stage_input,
            opts,
            translation_result.package_entries.clone(),
        );
    }

    if translation_result.build_success {
        log::info!("✅ Rust build completed successfully!");
    } else {
        let error = format!(
            "Failed to build Rust project: {:?}",
            translation_result.build_error
        );
        result.error_message = Some(error.clone());
        log::info!("❌ Rust build failed");
        return result;
    }

    // Validation harness selection.
    // - gtest: a gtest_suite/ in the test case is preferred when present
    //   (auto), or forced via --test-harness gtest.
    // - Library projects always use cando2 (runner + test_vectors via FFI).
    // - Configurable projects can be tested either way; pick library validation only
    //   when an input `runner/` exists, otherwise fall back to running the driver
    //   binary against test_vectors (executable-style tests).
    // - Executable projects run the binary directly against test vectors.
    // - Unknown project kinds fall back to the binary path if available.
    let gtest_available = program_dir.join(harness::gtest::GTEST_SUITE_DIR).is_dir();
    if test_harness == TestHarness::Gtest && !gtest_available {
        let error = format!(
            "--test-harness gtest requested, but {} has no gtest_suite/ directory",
            program_dir.display()
        );
        log::error!("{}", error);
        result.error_message = Some(error);
        return result;
    }
    let use_gtest_validation = match test_harness {
        TestHarness::Gtest => true,
        TestHarness::Auto => gtest_available,
        TestHarness::Lib | TestHarness::Bin => false,
    };
    let runner_exists = program_dir.join("runner").is_dir();
    let use_library_validation = match test_harness {
        TestHarness::Lib => true,
        TestHarness::Bin => false,
        TestHarness::Auto | TestHarness::Gtest => match project_kind {
            Some(ProjectKind::Library) => true,
            Some(ProjectKind::Configurable) => runner_exists,
            _ => false,
        },
    };
    let (test_results, error_messages) = if use_gtest_validation {
        match harness::gtest::run_gtest_validation(&program_name, program_dir, &output_dir, timeout)
        {
            Ok(r) => {
                // gtest defines its own test set; the vector count no longer applies.
                result.total_tests = r.0.len();
                r
            }
            Err(e) => {
                let error_msg = format!("GoogleTest validation failed: {}", e);
                log::error!("{}", error_msg);
                result.error_message = Some(error_msg);
                return result;
            }
        }
    } else {
        match (use_library_validation, translation_result.rust_binary_path) {
            (true, _) => {
                match harness::library::run_library_validation(
                    &program_name,
                    program_dir,
                    &output_dir,
                    &test_cases,
                    timeout,
                ) {
                    Ok(r) => r,
                    Err(e) => {
                        let error_msg = format!("Library validation failed: {}", e);
                        log::error!("{}", error_msg);
                        result.error_message = Some(error_msg);
                        return result;
                    }
                }
            }
            (false, Some(binary_path)) if binary_path.exists() => {
                run_test_validation(&binary_path, &test_cases, timeout, &output_dir)
            }
            (_, binary_path) => {
                let error = format!(
                "Rust build reported success, but expected output artifact was not found at {:?}",
                binary_path
            );
                log::error!("{}", error);
                result.error_message = Some(error);
                return result;
            }
        }
    };

    result.passed_tests = test_results
        .iter()
        .filter(|t| t.passed && !t.skipped)
        .count();
    result.skipped_tests = test_results.iter().filter(|t| t.skipped).count();
    result.test_results = test_results;

    // Print summary for this example
    log::info!("\nResults for {}:", program_name);
    log::info!(
        "  Translation: {}",
        status_emoji(result.translation_success)
    );
    log::info!("  Rust Build: {}", status_emoji(result.rust_build_success));
    log::info!(
        "  Tests: {}/{} passed ({} skipped, {:.1}%)",
        result.passed_tests,
        result.evaluated_tests(),
        result.skipped_tests,
        result.success_rate()
    );

    // Write error messages to results.err file in the output directory if it was created
    if !error_messages.is_empty() {
        let error_file_path = output_dir.join("results.err");
        if let Err(e) = write_error_file(&error_file_path, &error_messages) {
            log::info!("Warning: Failed to write error file: {}", e);
        }
    }

    result
}

fn main() -> HarvestResult<()> {
    set_user_only_umask();
    let args = Args::parse();

    let log_root = args
        .test
        .as_ref()
        .or(args.output_dir.as_ref())
        .expect("clap requires either --test or output_dir");
    ensure_output_directory(log_root)?;
    let log_file = File::create(log_root.join("output.log"))?;
    TeeLogger::init(log::LevelFilter::Info, log_file)?;
    log::info!("Harvest version: {}", get_version());
    run(args)
}

fn apply_regex_filter(
    program_dirs: &mut Vec<PathBuf>,
    pattern: &str,
    keep_matches: bool,
    label: &str,
) -> HarvestResult<()> {
    let regex =
        Regex::new(pattern).map_err(|e| format!("Invalid regex pattern '{}': {}", pattern, e))?;
    let mut removed_names = Vec::new();
    program_dirs.retain(|path| {
        let matches = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| regex.is_match(name))
            .unwrap_or(false);
        let keep = if keep_matches { matches } else { !matches };
        if !keep {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                removed_names.push(name.to_string());
            }
        }
        keep
    });
    log::info!(
        "{} '{}' applied: {} programs remaining, {} removed",
        label,
        pattern,
        program_dirs.len(),
        removed_names.len(),
    );
    if !removed_names.is_empty() {
        let past_tense = match label {
            "Filter" => "Filtered",
            "Exclude" => "Excluded",
            _ => label,
        };
        log::info!("{}: {}", past_tense, removed_names.join(", "));
    }
    Ok(())
}

/// A translated program directory carries its test definition either as
/// cando2/stdio JSON vectors (`test_vectors/`) or as a GoogleTest suite
/// (`gtest_suite/`).
fn is_translated_program_dir(path: &Path) -> bool {
    path.join("Cargo.toml").exists()
        && (path.join("test_vectors").is_dir()
            || path.join(harness::gtest::GTEST_SUITE_DIR).is_dir())
}

fn translated_program_dirs(path: &Path) -> HarvestResult<Vec<PathBuf>> {
    if is_translated_program_dir(path) {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let path = entry?.path();
        if path.is_dir() && is_translated_program_dir(&path) {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn test_existing_program(
    program_dir: &Path,
    timeout: u64,
    test_harness: TestHarness,
) -> ProgramEvalStats {
    let program_name = program_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let mut result = ProgramEvalStats::new(&program_name);
    result.translation_success = true;

    log::info!("Testing translated program: {}", program_name);
    log::info!("Program directory: {}", program_dir.display());

    let gtest_available = program_dir.join(harness::gtest::GTEST_SUITE_DIR).is_dir();

    // gtest-only programs need no JSON vectors: the suite defines the test set.
    let test_vectors_dir = program_dir.join("test_vectors");
    let test_cases = if test_vectors_dir.is_dir() {
        match parse_test_vectors(&test_vectors_dir) {
            Ok(vectors) => vectors,
            Err(e) => {
                result.error_message = Some(e.to_string());
                return result;
            }
        }
    } else if gtest_available {
        Vec::new()
    } else {
        result.error_message = Some(format!(
            "Required test_vectors directory not found: {}",
            test_vectors_dir.display()
        ));
        return result;
    };
    result.total_tests = test_cases.len();

    if test_harness == TestHarness::Gtest && !gtest_available {
        result.error_message = Some(format!(
            "--test-harness gtest requested, but {} has no gtest_suite/ directory",
            program_dir.display()
        ));
        return result;
    }
    let use_gtest_validation = match test_harness {
        TestHarness::Gtest => true,
        TestHarness::Auto => gtest_available,
        TestHarness::Lib | TestHarness::Bin => false,
    };
    let use_library_validation = match test_harness {
        TestHarness::Lib => true,
        TestHarness::Bin => false,
        TestHarness::Auto | TestHarness::Gtest => program_dir.join("runner").is_dir(),
    };
    let (test_results, error_messages) = if use_gtest_validation {
        match harness::gtest::run_gtest_validation(&program_name, program_dir, program_dir, timeout)
        {
            Ok(r) => {
                result.rust_build_success = true;
                // gtest defines its own test set; the vector count no longer applies.
                result.total_tests = r.0.len();
                r
            }
            Err(e) => {
                result.rust_build_success = false;
                result.error_message = Some(format!("GoogleTest validation failed: {e}"));
                return result;
            }
        }
    } else if use_library_validation {
        match harness::library::run_library_validation(
            &program_name,
            program_dir,
            program_dir,
            &test_cases,
            timeout,
        ) {
            Ok(r) => {
                result.rust_build_success = true;
                r
            }
            Err(e) => {
                result.rust_build_success = false;
                result.error_message = Some(format!("Library validation failed: {e}"));
                return result;
            }
        }
    } else {
        let output = std::process::Command::new("cargo")
            .arg("build")
            .arg("--release")
            .current_dir(program_dir)
            .output();
        match output {
            Ok(output) if output.status.success() => {
                result.rust_build_success = true;
                let binary_path = program_dir.join("target/release/driver");
                if !binary_path.exists() {
                    result.error_message = Some(format!(
                        "Built executable project but driver binary was not found at {}",
                        binary_path.display()
                    ));
                    return result;
                }
                run_test_validation(&binary_path, &test_cases, timeout, program_dir)
            }
            Ok(output) => {
                result.rust_build_success = false;
                result.error_message = Some(format!(
                    "cargo build --release failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
                return result;
            }
            Err(e) => {
                result.rust_build_success = false;
                result.error_message = Some(format!("Failed to run cargo build: {e}"));
                return result;
            }
        }
    };

    result.passed_tests = test_results
        .iter()
        .filter(|t| t.passed && !t.skipped)
        .count();
    result.skipped_tests = test_results.iter().filter(|t| t.skipped).count();
    result.test_results = test_results;

    if !error_messages.is_empty() {
        let error_file_path = program_dir.join("results.err");
        if let Err(e) = write_error_file(&error_file_path, &error_messages) {
            log::info!("Warning: Failed to write error file: {}", e);
        }
    }

    result
}

fn run_test_only(
    test_path: &Path,
    timeout: u64,
    filter: Option<&str>,
    exclude: Option<&str>,
    test_harness: TestHarness,
) -> HarvestResult<Vec<ProgramEvalStats>> {
    validate_input_directory(test_path)?;
    let mut program_dirs = translated_program_dirs(test_path)?;
    if let Some(filter_pattern) = filter {
        apply_regex_filter(&mut program_dirs, filter_pattern, true, "Filter")?;
    }
    if let Some(exclude_pattern) = exclude {
        apply_regex_filter(&mut program_dirs, exclude_pattern, false, "Exclude")?;
    }
    log_found_programs(&program_dirs, test_path)?;
    Ok(program_dirs
        .iter()
        .map(|program_dir| test_existing_program(program_dir, timeout, test_harness))
        .collect())
}

/// Overall wall-clock budget for the conform agent (not per-test). Matches the
/// verify stage's default; the per-test grading timeout is the `--timeout` flag.
const CONFORM_AGENT_TIMEOUT_SECS: u64 = 36000;

/// Determines which external test harness a translated program directory
/// carries, and the directory names that constitute it, honoring an explicit
/// `--test-harness` override.
fn detect_conform_harness(
    program_dir: &Path,
    test_harness: TestHarness,
) -> HarvestResult<(conform_agentic::ConformHarness, Vec<String>)> {
    use conform_agentic::ConformHarness as H;
    let has_gtest = program_dir.join(harness::gtest::GTEST_SUITE_DIR).is_dir();
    let has_runner = program_dir.join("runner").is_dir();
    let has_vectors = program_dir.join("test_vectors").is_dir();
    let gtest = || (H::Gtest, vec!["gtest_suite".to_string()]);
    let lib = || {
        (
            H::Lib,
            vec!["runner".to_string(), "test_vectors".to_string()],
        )
    };
    let bin = || (H::Bin, vec!["test_vectors".to_string()]);
    match test_harness {
        TestHarness::Gtest => has_gtest.then(gtest).ok_or_else(|| {
            format!(
                "--test-harness gtest, but {} has no gtest_suite/",
                program_dir.display()
            )
            .into()
        }),
        TestHarness::Lib => has_runner.then(lib).ok_or_else(|| {
            format!(
                "--test-harness lib, but {} has no runner/",
                program_dir.display()
            )
            .into()
        }),
        TestHarness::Bin => has_vectors.then(bin).ok_or_else(|| {
            format!(
                "--test-harness bin, but {} has no test_vectors/",
                program_dir.display()
            )
            .into()
        }),
        TestHarness::Auto => {
            if has_gtest {
                Ok(gtest())
            } else if has_runner {
                Ok(lib())
            } else if has_vectors {
                Ok(bin())
            } else {
                Err(format!(
                    "no external test suite found in {} (looked for gtest_suite/, runner/, test_vectors/)",
                    program_dir.display()
                )
                .into())
            }
        }
    }
}

/// Third-stage conformance mode: refine each already-translated program in
/// `input_root` so its external tests pass, writing refined copies under
/// `output_root`. The agent runs tempdir-isolated (never touching the input);
/// grading afterward uses a pristine copy of the same external tests taken
/// from the untouched input, so editing the tests cannot help the agent.
fn run_conform(
    input_root: &Path,
    output_root: &Path,
    agent: harvest_core::config::AgentKind,
    model: Option<&str>,
    grading_timeout: u64,
    test_harness: TestHarness,
    output_log_path: &Path,
) -> HarvestResult<Vec<ProgramEvalStats>> {
    validate_input_directory(input_root)?;
    let program_dirs = translated_program_dirs(input_root)?;
    log_found_programs(&program_dirs, input_root)?;

    let env = std::collections::HashMap::new();
    let mut results = Vec::new();

    for in_prog in &program_dirs {
        let name = in_prog
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let out_prog = output_root.join(&name);
        let mut result = ProgramEvalStats::new(&name);

        log::info!("\n{}", "=".repeat(80));
        log::info!("Conform: refining {}", name);
        log::info!("{}", "=".repeat(80));

        let (harness_kind, test_dirs) = match detect_conform_harness(in_prog, test_harness) {
            Ok(hk) => hk,
            Err(e) => {
                result.error_message = Some(e.to_string());
                results.push(result);
                continue;
            }
        };

        // 1. Run the refinement agent (tempdir-isolated) into out_prog.
        if let Err(e) = conform_agentic::run(conform_agentic::ConformParams {
            input_project_dir: in_prog,
            output_project_dir: &out_prog,
            harness: harness_kind,
            test_dirs: &test_dirs,
            agent,
            model,
            timeout_secs: CONFORM_AGENT_TIMEOUT_SECS,
            env: &env,
            output_log_path: Some(output_log_path),
        }) {
            result.error_message = Some(format!("Conform agent failed: {e}"));
            results.push(result);
            continue;
        }

        // 2. Re-copy a pristine external test suite from the untouched input
        //    into the refined output, so the grade is against tests the agent
        //    could not have edited.
        let mut copy_failed = None;
        for d in &test_dirs {
            let src = in_prog.join(d);
            let dst = out_prog.join(d);
            if dst.exists() {
                let _ = std::fs::remove_dir_all(&dst);
            }
            if let Err(e) = harvest_core::cargo_utils::copy_directory_recursive(&src, &dst) {
                copy_failed = Some(format!("Failed to stage pristine {d}: {e}"));
                break;
            }
        }
        if let Some(e) = copy_failed {
            result.error_message = Some(e);
            results.push(result);
            continue;
        }

        // Propagate the stage manifest (appending conform to the stage
        // history) so the refined output stays a self-describing snapshot.
        match StageManifest::read_from_dir(in_prog) {
            Ok(mut manifest) => {
                if !manifest.stages.contains(&Stage::Conform) {
                    manifest.stages.push(Stage::Conform);
                }
                manifest.agent = agent;
                manifest.model = model.map(str::to_owned);
                manifest.prompt_mode = "conform".to_owned();
                manifest.harvest_version = get_version().to_owned();
                if let Err(e) = manifest.write_to_dir(&out_prog) {
                    log::warn!("Failed to propagate stage manifest to conform output: {e}");
                }
            }
            Err(_) => log::info!(
                "Input snapshot {} has no stage manifest; conform output not stamped",
                in_prog.display()
            ),
        }

        // 3. Grade independently, exactly like --test mode.
        results.push(test_existing_program(
            &out_prog,
            grading_timeout,
            test_harness,
        ));
    }

    Ok(results)
}

/// Resolves the bench program directory a snapshot grades against, when the
/// run resumes from a snapshot (first stage verify). The reference comes from
/// the snapshot's manifest, or from --test-case (which may point at a bench
/// root containing a same-named program subdirectory, or at one bench program
/// directory). The bench case's test_case/ content hash is checked against the
/// manifest: drift is fatal when using the manifest's own reference, and a
/// loud warning when the user explicitly overrode the location.
fn resolve_bench_reference(
    snapshot_prog_dir: &Path,
    test_case_override: Option<&Path>,
) -> HarvestResult<PathBuf> {
    let manifest = StageManifest::read_from_dir(snapshot_prog_dir).map_err(|e| {
        format!(
            "snapshot {} has no readable stage manifest ({e}); only outputs of a \
             stage-aware run can be resumed from",
            snapshot_prog_dir.display()
        )
    })?;
    let name = snapshot_prog_dir.file_name().unwrap_or_default();
    let bench_dir = match test_case_override {
        Some(tc) => {
            let candidate = tc.join(name);
            if parse_benchmark_dir(&candidate).is_ok() {
                candidate
            } else if parse_benchmark_dir(tc).is_ok() {
                tc.to_path_buf()
            } else {
                return Err(format!(
                    "--test-case {} is neither a bench program directory nor a bench root \
                     containing {:?}",
                    tc.display(),
                    name
                )
                .into());
            }
        }
        None => manifest.bench_program_dir.clone(),
    };
    parse_benchmark_dir(&bench_dir).map_err(|e| {
        format!(
            "bench reference {} for snapshot {} is not a valid bench program directory: {e}",
            bench_dir.display(),
            snapshot_prog_dir.display()
        )
    })?;

    if !manifest.test_case_hash.is_empty() {
        let current = hash_dir(&bench_dir.join("test_case"))?;
        if current != manifest.test_case_hash {
            let msg = format!(
                "test_case/ content of {} does not match the hash recorded when {} was \
                 produced (bench case changed, or wrong pairing)",
                bench_dir.display(),
                snapshot_prog_dir.display()
            );
            if test_case_override.is_some() {
                log::warn!("{msg} — proceeding because --test-case was given explicitly");
            } else {
                return Err(format!("{msg}; pass --test-case to override explicitly").into());
            }
        }
    }
    Ok(bench_dir)
}

fn run(args: Args) -> HarvestResult<()> {
    log::info!("Running Benchmarks");

    let stages = args.stages();
    args.validate_stages(&stages)?;

    if stages.contains(&Stage::Conform) {
        let input_dir = args
            .input_dir
            .as_ref()
            .expect("clap requires input_dir unless --test is used");
        let output_dir = args
            .output_dir
            .as_ref()
            .expect("clap requires output_dir unless --test is used");
        let agent = args
            .agent
            .ok_or("--agentic=conform requires --agent")?;
        validate_input_directory(input_dir)?;
        ensure_output_directory(output_dir)?;
        log::info!(
            "Conform mode: {} -> {}",
            input_dir.display(),
            output_dir.display()
        );
        let results = run_conform(
            input_dir,
            output_dir,
            agent,
            args.model.as_deref(),
            args.timeout,
            args.test_harness,
            &output_dir.join("output.log"),
        )?;
        let csv_output_path = output_dir.join("results.csv");
        write_csv_results(&csv_output_path, &results)?;
        let summary_stats = SummaryStats::from_results(&results);
        log_summary_stats(&summary_stats);
        log_failing_programs(&results);
        log::info!("\nConform processing complete.");
        return Ok(());
    }

    if let Some(test_path) = &args.test {
        log::info!("Test-only mode: {}", test_path.display());
        let results = run_test_only(
            test_path,
            args.timeout,
            args.filter.as_deref(),
            args.exclude.as_deref(),
            args.test_harness,
        )?;
        let csv_output_path = test_path.join("results.csv");
        write_csv_results(&csv_output_path, &results)?;
        let summary_stats = SummaryStats::from_results(&results);
        log_summary_stats(&summary_stats);
        log::info!("\nOutput Files:");
        log::info!("  Tested translated projects: {}", test_path.display());
        log::info!("  CSV results: {}", csv_output_path.display());
        log::info!("  Error logs: results.err files in each translated project directory");
        log_failing_programs(&results);
        log::info!("\nTest-only processing complete.");
        return Ok(());
    }

    let input_dir = args
        .input_dir
        .as_ref()
        .expect("clap requires input_dir unless --test is used");
    let output_dir = args
        .output_dir
        .as_ref()
        .expect("clap requires output_dir unless --test is used");

    validate_input_directory(input_dir)?;
    ensure_output_directory(output_dir)?;

    log::info!("Input directory: {}", input_dir.display());
    log::info!("Output directory: {}", output_dir.display());
    log::info!(
        "Using {} Translation",
        if args.modular {
            "Modular".to_owned()
        } else if !stages.is_empty() {
            format!(
                "Agentic ({})",
                stages
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        } else {
            "All-at-once".to_owned()
        }
    );

    // Build the per-program work list. INPUT_DIR is what the first stage
    // consumes: bench test cases for translate (and the non-agentic modes),
    // or a previous run's snapshot root when resuming from verify.
    let resume_from_snapshot = stages.first() == Some(&Stage::Verify);
    let mut program_dirs = if resume_from_snapshot {
        translated_program_dirs(input_dir)?
    } else if parse_benchmark_dir(input_dir).is_ok() {
        // The input itself is a single test case root: run just that.
        vec![input_dir.clone()]
    } else {
        collect_program_dirs(input_dir)?
    };

    if let Some(filter_pattern) = &args.filter {
        apply_regex_filter(&mut program_dirs, filter_pattern, true, "Filter")?;
    }

    if let Some(exclude_pattern) = &args.exclude {
        apply_regex_filter(&mut program_dirs, exclude_pattern, false, "Exclude")?;
    }

    log_found_programs(&program_dirs, input_dir)?;

    let program_runs: Vec<ProgramRun> = if resume_from_snapshot {
        let mut runs = Vec::new();
        for snapshot_dir in &program_dirs {
            let bench_program_dir =
                resolve_bench_reference(snapshot_dir, args.test_case.as_deref())?;
            log::info!(
                "Snapshot {} grades against bench case {}",
                snapshot_dir.display(),
                bench_program_dir.display()
            );
            runs.push(ProgramRun {
                bench_program_dir,
                stage_input: Some(snapshot_dir.clone()),
            });
        }
        runs
    } else {
        program_dirs
            .iter()
            .map(|dir| ProgramRun {
                bench_program_dir: dir.clone(),
                stage_input: None,
            })
            .collect()
    };

    let opts = RunOptions {
        config_overrides: args.config.clone(),
        timeout: args.timeout,
        modular: args.modular,
        stages,
        agent: args.agent,
        agent_tools: args.agent_tools,
        model: args.model.clone(),
        no_plan: args.no_plan,
        no_plan_file: args.no_plan_file,
        workflow: args.workflow,
        test_harness: args.test_harness,
        verify_harness: args.verify_harness.unwrap_or_default(),
        fuzz: args.fuzz,
    };
    let results = run_all_benchmarks(&program_runs, output_dir, &opts)?;
    let csv_output_path = output_dir.join("results.csv");
    write_csv_results(&csv_output_path, &results)?;

    let summary_stats = SummaryStats::from_results(&results);
    log_summary_stats(&summary_stats);

    log::info!("\nOutput Files:");
    log::info!("  Translated projects: {}", output_dir.display());
    log::info!("  CSV results: {}", csv_output_path.display());
    log::info!("  Error logs: results.err files in each translated project directory");

    // Print examples with issues
    log_failing_programs(&results);

    log::info!(
        "\nProcessing complete! Check the CSV file and individual project directories for detailed results."
    );

    cleanup_benchmarks(&results, output_dir);

    Ok(())
}

fn status_emoji(success: bool) -> &'static str {
    match success {
        true => "✅",
        false => "❌",
    }
}

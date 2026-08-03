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
use crate::ir_utils::{cargo_build_result, external_test_suite, raw_cargo_package, raw_source};
use crate::logger::TeeLogger;
use crate::stats::{ProgramEvalStats, SummaryStats, TestResult};
use build_project_spec::{detect_project_kind, ProjectKind};
use clap::Parser;
use harvest_core::config::{AgentKind, Stage};
use harvest_core::stage_manifest::{self, StageManifest};
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
}

impl TranspilationResult {
    /// Extract relevant info from HarvestIR
    pub fn from_ir(ir: &HarvestIR) -> Self {
        let translation_success = raw_cargo_package(ir).is_ok();
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
            // Re-emit the read-only reference inputs, so this output stands on
            // its own as the next stage's (and the grader's) input.
            write_snapshot_references(output_dir, &ir);
            TranspilationResult::from_ir(&ir)
        }
        Err(e) => {
            log::error!("Failed to transpile (full error): {:#?}", e);
            TranspilationResult {
                translation_success: false,
                build_success: false,
                rust_binary_path: None,
                build_error: Some(format!("Failed to transpile: {}", e)),
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
    pub force: bool,
}

impl RunOptions {
    fn has(&self, stage: Stage) -> bool {
        self.stages.contains(&stage)
    }

    /// How this run produces the crate, for the manifest: the agentic prompt
    /// mode, or which non-agentic translator ran.
    fn prompt_mode(&self) -> &'static str {
        if self.stages.is_empty() {
            return if self.modular { "modular" } else { "one_shot" };
        }
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

/// One program's work item, with every input already resolved to a concrete
/// location. A run that starts at translate reads them from the bench test
/// case; a run that resumes reads them from the snapshot, which carries its
/// own copies — that is what lets a snapshot be graded and refined after the
/// bench directory has moved or changed.
pub struct ProgramRun {
    pub name: String,
    /// C source root, handed to the pipeline as `config.input`: the bench
    /// `test_case/`, or the snapshot's `.harvest/c_src/`.
    pub c_source_dir: PathBuf,
    /// Directory whose children are the external test suite directories: the
    /// bench program directory, or the snapshot's `.harvest/suite/`.
    /// `None` when the test case ships no external suite.
    pub suite_root: Option<PathBuf>,
    /// The snapshot the first stage loads its `CargoPackage` from, if resuming.
    pub stage_input: Option<PathBuf>,
    /// Stage history carried over from the input snapshot.
    pub prior_stages: Vec<Stage>,
    /// Bench test case this lineage came from, and the revision of the bench
    /// checkout it was taken at. Provenance only, carried forward unchanged.
    pub bench_program: String,
    pub bench_revision: Option<String>,
    /// Test-Corpus checkout for the toolchain contract, from the input
    /// snapshot's manifest or detected now.
    pub test_corpus_root: Option<PathBuf>,
}

/// Makes `output_dir` ready to receive a snapshot: it must not already hold
/// one run's results when a second run writes into it.
fn prepare_output_dir(output_dir: &Path, force: bool) -> HarvestResult<()> {
    let occupied = std::fs::read_dir(output_dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    if occupied {
        if !force {
            return Err(format!(
                "output program directory {} is not empty; pass --force to overwrite it \
                 (reusing a populated directory mixes two runs' results)",
                output_dir.display()
            )
            .into());
        }
        log::warn!("--force: erasing existing {}", output_dir.display());
        std::fs::remove_dir_all(output_dir)?;
    }
    std::fs::create_dir_all(output_dir)?;
    Ok(())
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

/// Writes the read-only reference inputs the IR is carrying into the output
/// program directory, replacing whatever was there.
fn write_snapshot_references(output_dir: &Path, ir: &HarvestIR) {
    let replace = |dest: PathBuf, what: &str, write: &dyn Fn(&Path) -> std::io::Result<()>| {
        if dest.exists() {
            if let Err(e) = std::fs::remove_dir_all(&dest) {
                log::warn!("Failed to clear stale {what} at {}: {e}", dest.display());
                return;
            }
        }
        if let Err(e) = write(&dest) {
            log::warn!("Failed to write {what} to {}: {e}", dest.display());
        }
    };
    match raw_source(ir) {
        Ok(c_source) => replace(
            stage_manifest::c_source_dir(output_dir),
            "C source",
            &|dest| c_source.materialize(dest),
        ),
        Err(e) => log::warn!("Failed to retrieve C source from IR: {e}"),
    }
    if let Some(suite) = external_test_suite(ir) {
        replace(stage_manifest::suite_dir(output_dir), "test suite", &|dest| {
            suite.dir.materialize(dest)
        });
    }
}

/// Stamps the output program directory with a stage manifest. The manifest is
/// provenance, not protocol — the layout is what a later run reads — so a
/// failure here is logged rather than fatal.
fn write_stage_manifest(output_dir: &Path, run: &ProgramRun, opts: &RunOptions) {
    // Accumulate stages across runs: resuming appends to the input snapshot's
    // history.
    let mut stages = run.prior_stages.clone();
    stages.extend(opts.stages.iter().copied());
    stages.sort();
    stages.dedup();

    // Any reference directory an agent modified was quarantined here by the
    // stage that caught it.
    let reference_modified = std::fs::read_dir(
        stage_manifest::meta_dir(output_dir).join(stage_manifest::REJECTED_DIR),
    )
    .map(|entries| {
        let mut names: Vec<String> = entries
            .flatten()
            .flat_map(|stage| std::fs::read_dir(stage.path()).ok())
            .flat_map(|refs| refs.flatten())
            .map(|r| r.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names.dedup();
        names
    })
    .unwrap_or_default();
    if !reference_modified.is_empty() {
        log::warn!(
            "An agent modified read-only reference input(s): {}. The change was discarded; \
             copies are preserved under {}/{}/. Treat this run's results with suspicion.",
            reference_modified.join(", "),
            stage_manifest::HARVEST_META_DIR,
            stage_manifest::REJECTED_DIR
        );
    }

    let manifest = StageManifest {
        schema_version: 1,
        stages,
        agent: opts.agent,
        model: opts.model.clone(),
        prompt_mode: opts.prompt_mode().to_owned(),
        harvest_version: get_version().to_owned(),
        bench_program: run.bench_program.clone(),
        bench_revision: run.bench_revision.clone(),
        // Re-read rather than carried: the checkout can move under a resumed run.
        test_corpus_revision: run
            .test_corpus_root
            .as_ref()
            .and_then(harvest_core::utils::git_revision),
        test_corpus_root: run.test_corpus_root.clone(),
        reference_modified,
        created_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    match manifest.write_to_dir(output_dir) {
        Ok(()) => log::info!(
            "Stage manifest written to {}",
            stage_manifest::meta_dir(output_dir)
                .join(stage_manifest::STAGE_MANIFEST_FILE)
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
    let stage_input = program_run.stage_input.as_deref();
    let timeout = opts.timeout;
    let test_harness = opts.test_harness;

    let program_name = program_run.name.clone();
    let test_case_dir = program_run.c_source_dir.clone();
    let mut result = ProgramEvalStats::new(&program_name);

    log::info!("Processing program: {}", program_name);
    log::info!("C source: {}", test_case_dir.display());
    match &program_run.suite_root {
        Some(suite) => log::info!("External test suite: {}", suite.display()),
        None => log::info!("External test suite: none"),
    }
    if let Some(snapshot) = stage_input {
        log::info!("Stage input (snapshot): {}", snapshot.display());
    }

    // Get program output directory
    let output_dir = output_root_dir.join(&program_name);
    log::info!("Output directory: {}", output_dir.display());

    // Detect project kind from the C source root (same heuristic as build_project_spec).
    let project_kind = detect_project_kind(&test_case_dir);
    log::info!(
        "Detected project type: {}",
        project_kind
            .as_ref()
            .map(|k| k.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    );

    // Parse JSON test vectors, when the suite has them. gtest-only test cases
    // define their test set in the suite instead.
    let test_vectors_dir = program_run
        .suite_root
        .as_ref()
        .map(|root| root.join("test_vectors"))
        .filter(|dir| dir.is_dir());
    let test_cases = match &test_vectors_dir {
        Some(dir) => match parse_test_vectors(dir) {
            Ok(vectors) => vectors,
            Err(e) => {
                result.error_message = Some(e.to_string());
                return result;
            }
        },
        None => Vec::new(),
    };

    result.total_tests = test_cases.len();

    // Log test case parsing success
    if !test_cases.is_empty() {
        log::info!("✅ Successfully parsed {} test case(s)", test_cases.len());
    }

    // Guard the output: a snapshot is only self-contained if nothing from an
    // earlier run survives in it. Materializing into a populated directory
    // leaves stale files behind, which then travel forward as if they were
    // this run's output.
    if let Err(e) = prepare_output_dir(&output_dir, opts.force) {
        result.error_message = Some(e.to_string());
        log::error!("{e}");
        return result;
    }

    // Inject per-stage tool config for the agentic stages that actually run
    // in this invocation. Both agents share the wishlist file; the verify
    // phase appends to whatever the translate phase wrote. Everything the
    // framework owns lives under the snapshot's meta directory.
    let meta_dir = stage_manifest::meta_dir(&output_dir);
    std::fs::create_dir_all(&meta_dir).ok();

    let wishlist_path = meta_dir.join("tool_wishlist.json");
    // Translate-phase PLAN.md is dumped under a dedicated name so a later verify
    // rewrite of the CargoPackage cannot overwrite or delete it.
    let plan_translate_path = meta_dir.join("plan_translate.md");
    // Verify-phase HYPOTHESES.md captures the agent's hypothesis log across
    // compactions, for post-hoc analysis of how it approached debugging.
    let hypotheses_verify_path = meta_dir.join("hypotheses_verify.md");
    // The output.log path lets each tool append the agent's full JSON trace
    // to the same log file as benchmark messages.
    let output_log_path = output_root_dir.join("output.log");
    let rejected_dir = meta_dir.join(stage_manifest::REJECTED_DIR);
    let mut effective_overrides = opts.config_overrides.clone();
    // Stage-uniform settings apply to every agentic stage running in this
    // invocation; use -c tools.<tool>.<key>=... for per-stage overrides.
    let stage_overrides = |overrides: &mut Vec<String>, tool: &str, stage: Stage| {
        overrides.push(format!(
            "tools.{tool}.output_log_path={}",
            output_log_path.display()
        ));
        overrides.push(format!(
            "tools.{tool}.rejected_output_dir={}",
            rejected_dir.join(stage.to_string()).display()
        ));
        if let Some(root) = &program_run.test_corpus_root {
            overrides.push(format!("tools.{tool}.test_corpus_root={}", root.display()));
        }
        if let Some(m) = &opts.model {
            overrides.push(format!("tools.{tool}.model={m}"));
        }
    };
    // The suite is loaded on every run, whichever translator produces the
    // crate, so the output can re-emit it and stand on its own; only the
    // conform stage is given it as an input.
    if let Some(suite_root) = &program_run.suite_root {
        effective_overrides.push(format!(
            "tools.load_test_suite.input_path={}",
            suite_root.display()
        ));
        if let Some(kind) = opts.test_harness.suite_kind() {
            effective_overrides.push(format!("tools.load_test_suite.harness={kind}"));
        }
    }
    let prompt_mode_overrides = |overrides: &mut Vec<String>, tool: &str| {
        overrides.push(format!(
            "tools.{tool}.wishlist_output_path={}",
            wishlist_path.display()
        ));
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
        stage_overrides(&mut effective_overrides, "translate_agentic", Stage::Translate);
        prompt_mode_overrides(&mut effective_overrides, "translate_agentic");
        effective_overrides.push(format!(
            "tools.translate_agentic.plan_output_path={}",
            plan_translate_path.display()
        ));
    }
    if opts.has(Stage::Verify) {
        stage_overrides(&mut effective_overrides, "verify_fix_agentic", Stage::Verify);
        prompt_mode_overrides(&mut effective_overrides, "verify_fix_agentic");
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
    if opts.has(Stage::Conform) {
        stage_overrides(&mut effective_overrides, "conform_agentic", Stage::Conform);
        effective_overrides.push(format!(
            "tools.conform_agentic.notes_output_path={}",
            meta_dir.join("conform_notes.md").display()
        ));
        effective_overrides.push(format!(
            "tools.conform_agentic.report_output_path={}",
            meta_dir.join("conform_report.md").display()
        ));
    }

    // Run the stage pipeline (translate/verify/conform, or the non-agentic
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

    // The output program directory now holds a materialized CargoPackage plus
    // its reference inputs: stamp it as a resumable snapshot. This is the
    // framework's one output format, so it applies to the non-agentic
    // translators too — their results are gradeable and stage-resumable on the
    // same terms (a one-shot translation can be handed to the verify stage).
    // Build failure does not gate it either: a snapshot whose build is broken
    // is exactly the kind a verify run should be able to pick up.
    write_stage_manifest(&output_dir, program_run, opts);

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

    // Validation harness selection. The suite graded against is the one the
    // snapshot now carries under .harvest/suite/ — the same content the tools
    // saw, re-emitted from the IR, so grading never depends on the bench
    // directory still being in place.
    let graded_suite_root = stage_manifest::suite_dir(&output_dir);
    // - gtest: a gtest_suite/ in the suite is preferred when present (auto),
    //   or forced via --test-harness gtest.
    // - Library projects always use cando2 (runner + test_vectors via FFI).
    // - Configurable projects can be tested either way; pick library validation only
    //   when a `runner/` exists, otherwise fall back to running the driver
    //   binary against test_vectors (executable-style tests).
    // - Executable projects run the binary directly against test vectors.
    // - Unknown project kinds fall back to the binary path if available.
    let gtest_suite_dir = graded_suite_root.join(harness::gtest::GTEST_SUITE_DIR);
    let gtest_available = gtest_suite_dir.is_dir();
    if test_harness == TestHarness::Gtest && !gtest_available {
        let error = format!(
            "--test-harness gtest requested, but no gtest_suite/ was carried into {}",
            graded_suite_root.display()
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
    let runner_exists = graded_suite_root.join("runner").is_dir();
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
        match harness::gtest::run_gtest_validation(
            &program_name,
            &gtest_suite_dir,
            &output_dir,
            timeout,
        ) {
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
                    &graded_suite_root,
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

    // Write error messages to results.err in the snapshot's meta directory
    if !error_messages.is_empty() {
        let error_file_path = stage_manifest::meta_dir(&output_dir).join("results.err");
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

/// Recognizes a pipeline snapshot by its stage manifest. This is the only
/// marker that works for every role: a snapshot whose build failed never
/// reached grading, and one that was never graded carries no results — but
/// both carry their manifest, their C source, and their test suite, which is
/// all a resumed stage or a re-grade needs.
fn is_translated_program_dir(path: &Path) -> bool {
    stage_manifest::is_snapshot(path)
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

    // Everything graded against comes from the snapshot's own suite.
    let suite_root = stage_manifest::suite_dir(program_dir);
    let gtest_suite_dir = suite_root.join(harness::gtest::GTEST_SUITE_DIR);
    let gtest_available = gtest_suite_dir.is_dir();

    // gtest-only programs need no JSON vectors: the suite defines the test set.
    let test_vectors_dir = suite_root.join("test_vectors");
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
            "Snapshot {} carries no external test suite under {}",
            program_dir.display(),
            suite_root.display()
        ));
        return result;
    };
    result.total_tests = test_cases.len();

    if test_harness == TestHarness::Gtest && !gtest_available {
        result.error_message = Some(format!(
            "--test-harness gtest requested, but {} carries no gtest_suite/",
            suite_root.display()
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
        TestHarness::Auto | TestHarness::Gtest => suite_root.join("runner").is_dir(),
    };
    let (test_results, error_messages) = if use_gtest_validation {
        match harness::gtest::run_gtest_validation(
            &program_name,
            &gtest_suite_dir,
            program_dir,
            timeout,
        ) {
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
            &suite_root,
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
        let error_file_path = stage_manifest::meta_dir(program_dir).join("results.err");
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

/// Resolves a program's inputs for a run that starts from a bench test case:
/// the C source is the bench `test_case/`, and the external suite (if any)
/// sits beside it in the bench program directory.
fn start_from_bench(bench_program_dir: &Path, name: String) -> HarvestResult<ProgramRun> {
    let (c_source_dir, _) = parse_benchmark_dir(bench_program_dir)?;
    let suite_root =
        load_test_suite::has_suite(bench_program_dir).then(|| bench_program_dir.to_path_buf());
    if suite_root.is_none() {
        log::warn!(
            "bench case {} ships no external test suite",
            bench_program_dir.display()
        );
    }
    Ok(ProgramRun {
        // The toolchain contract is derived from the corpus the bench case
        // lives in; recording it now is what lets a later run find it once the
        // C source has moved inside a snapshot.
        test_corpus_root: agent_runner::locate_test_corpus(&c_source_dir),
        c_source_dir,
        suite_root,
        stage_input: None,
        prior_stages: Vec::new(),
        bench_revision: harvest_core::utils::git_revision(bench_program_dir),
        bench_program: name.clone(),
        name,
    })
}

/// Resolves a program's inputs for a run that resumes from a snapshot. Every
/// input comes from the snapshot itself; the bench directory is not consulted,
/// and does not have to still exist. `--test-case` overrides the suite with a
/// bench directory's current one, for deliberately re-running against an
/// updated test suite.
fn resume_from(
    snapshot_dir: &Path,
    name: String,
    suite_override: Option<&Path>,
) -> HarvestResult<ProgramRun> {
    let manifest = StageManifest::read_from_dir(snapshot_dir).map_err(|e| {
        format!(
            "snapshot {} has no readable stage manifest ({e}); only outputs of a \
             stage-aware run can be resumed from",
            snapshot_dir.display()
        )
    })?;
    let c_source_dir = stage_manifest::c_source_dir(snapshot_dir);
    if !c_source_dir.is_dir() {
        return Err(format!(
            "snapshot {} carries no C source at {}",
            snapshot_dir.display(),
            c_source_dir.display()
        )
        .into());
    }

    // Replacing the suite also replaces what `bench_revision` describes: it
    // records where the snapshot's reference material came from, so carrying
    // the origin's revision forward past a suite swap would misattribute the
    // tests this run was actually graded against.
    let mut bench_revision = manifest.bench_revision;
    let suite_root = match suite_override {
        Some(bench) => {
            let candidate = bench.join(&name);
            let chosen = if load_test_suite::has_suite(&candidate) {
                candidate
            } else if load_test_suite::has_suite(bench) {
                bench.to_path_buf()
            } else {
                return Err(format!(
                    "--test-case {} holds no external test suite (looked in it and in {:?})",
                    bench.display(),
                    name
                )
                .into());
            };
            log::info!(
                "Using the current suite from {} instead of the one {} carries",
                chosen.display(),
                snapshot_dir.display()
            );
            bench_revision = harvest_core::utils::git_revision(&chosen);
            Some(chosen)
        }
        None => {
            let carried = stage_manifest::suite_dir(snapshot_dir);
            load_test_suite::has_suite(&carried).then_some(carried)
        }
    };

    Ok(ProgramRun {
        name,
        c_source_dir,
        suite_root,
        stage_input: Some(snapshot_dir.to_path_buf()),
        prior_stages: manifest.stages,
        bench_program: manifest.bench_program,
        bench_revision,
        test_corpus_root: manifest.test_corpus_root,
    })
}

fn run(args: Args) -> HarvestResult<()> {
    log::info!("Running Benchmarks");

    let stages = args.stages();
    args.validate_stages(&stages)?;

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
    let resume_from_snapshot = matches!(stages.first(), Some(Stage::Verify | Stage::Conform));
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

    let mut program_runs: Vec<ProgramRun> = Vec::new();
    for dir in &program_dirs {
        let name = dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        program_runs.push(if resume_from_snapshot {
            resume_from(dir, name, args.test_case.as_deref())?
        } else {
            start_from_bench(dir, name)?
        });
    }

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
        force: args.force,
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

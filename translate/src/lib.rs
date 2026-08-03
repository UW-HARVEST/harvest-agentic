//! A framework for translating C code into Rust code. This is normally used through the
//! `translate` binary, but is exposed as a library crate as well.

pub mod cli;
mod runner;
mod scheduler;
pub mod util;

use build_project_spec::BuildProjectSpec;
use c_ast::ParseToAst;
use conform_agentic::ConformAgentic;
use harvest_core::config::{Config, Stage};
use harvest_core::utils::get_version;
use harvest_core::{HarvestIR, diagnostics};
use load_raw_source::LoadRawSource;
use load_test_suite::LoadTestSuite;
use load_translated_package::LoadTranslatedPackage;
use modular_translation_llm::ModularTranslationLlm;
use raw_source_to_cargo_llm::RawSourceToCargoLlm;
use runner::ToolRunner;
use scheduler::Scheduler;
use std::sync::Arc;
use tracing::{error, info};
use translate_agentic::TranslateAgentic;
use try_cargo_build::TryCargoBuild;
use verify_fix_agentic::VerifyFixAgentic;

/// Performs the complete transpilation process using the scheduler.
pub fn transpile(config: Arc<Config>) -> Result<HarvestIR, Box<dyn std::error::Error>> {
    // Basic tool setup
    let collector = diagnostics::Collector::initialize(&config)?;
    let mut ir = HarvestIR::default();
    let mut runner = ToolRunner::new(collector.reporter());
    let mut scheduler = Scheduler::default();

    info!("Harvest version: {}", get_version());
    info!("Transpiling with: {}", config.model_info().unwrap());

    // Setup a schedule for the transpilation. The C source is always loaded:
    // it is the translate input, and the verify stage's ground truth.
    let load_src = scheduler.queue(LoadRawSource::new(&config.input));

    // The external test suite is loaded for every run, whichever translator
    // produces the crate, so that the run can write it back out and its output
    // is a self-contained snapshot. Loading it is not the same as revealing it:
    // only the conform stage declares it as an input, which is what holds it
    // out of the other stages.
    let suite = load_test_suite::configured_input_path(&config)
        .filter(|path| load_test_suite::has_suite(path))
        .map(|_| scheduler.queue(LoadTestSuite));

    let translate = if !config.stages.is_empty() {
        // The initial CargoPackage comes from the translate stage when it
        // runs, otherwise from a previous run's snapshot.
        let package = if config.stages.contains(&Stage::Translate) {
            let project_spec = scheduler.queue_after(BuildProjectSpec, &[load_src]);
            scheduler.queue_after(TranslateAgentic, &[load_src, project_spec])
        } else {
            let snapshot = config.stage_input.as_deref().ok_or(
                "stages without `translate` require stage_input (a previous run's output directory)",
            )?;
            scheduler.queue(LoadTranslatedPackage::new(snapshot))
        };
        let package = if config.stages.contains(&Stage::Verify) {
            scheduler.queue_after(VerifyFixAgentic, &[package, load_src])
        } else {
            package
        };
        if config.stages.contains(&Stage::Conform) {
            let suite = suite.ok_or(
                "the conform stage needs an external test suite, but none was found at \
                 tools.load_test_suite.input_path",
            )?;
            scheduler.queue_after(ConformAgentic, &[package, load_src, suite])
        } else {
            package
        }
    } else if config.modular {
        let project_spec = scheduler.queue_after(BuildProjectSpec, &[load_src]);
        let parse_ast = scheduler.queue_after(ParseToAst, &[load_src]);
        scheduler.queue_after(ModularTranslationLlm, &[load_src, parse_ast, project_spec])
    } else {
        let project_spec = scheduler.queue_after(BuildProjectSpec, &[load_src]);
        scheduler.queue_after(RawSourceToCargoLlm, &[load_src, project_spec])
    };
    let _try_build = scheduler.queue_after(TryCargoBuild, &[translate]);

    // Run until all tasks are complete, respecting the dependencies declared in `queue_after`
    let result = scheduler.run_all(&mut runner, &mut ir, config);
    drop(scheduler);
    drop(runner);
    collector.diagnostics(); // TODO: Return this value (see issue 51)
    if let Err(e) = result {
        error!("Error during transpilation: {e}");
        return Err(e);
    }
    Ok(ir)
}

//! A framework for translating C code into Rust code. This is normally used through the
//! `translate` binary, but is exposed as a library crate as well.

pub mod cli;
mod runner;
mod scheduler;
pub mod util;

use build_project_spec::BuildProjectSpec;
use c_ast::ParseToAst;
use conform_agentic::ConformAgentic;
use harvest_core::config::{Config, Stage, ToolConfig};
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

/// The settings a run of [`transpile`] uses, for the stage manifest: the agent and the resolved
/// configuration of each agentic tool that runs, with the tool's own defaults filled in, or the
/// configuration section of the non-agentic translator as given. Secrets (`env`, `api_key`) are
/// left out. The tools are the ones [`transpile`] schedules for `config`.
pub fn effective_config(config: &Config) -> Result<serde_json::Value, serde_json::Error> {
    fn section<T: ToolConfig>(
        config: &Config,
        tools: &mut serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), serde_json::Error> {
        tools.insert(
            T::KEY.to_owned(),
            serde_json::to_value(config.tool::<T>()?)?,
        );
        Ok(())
    }
    let mut tools = serde_json::Map::new();
    if config.stages.is_empty() {
        let key = if config.modular {
            "modular_translation_llm"
        } else {
            "raw_source_to_cargo_llm"
        };
        let mut raw = config.tools.get(key).cloned().unwrap_or_default();
        if let Some(fields) = raw.as_object_mut() {
            fields.remove("api_key");
            fields.remove("env");
        }
        tools.insert(key.to_owned(), raw);
        return Ok(serde_json::json!({ "modular": config.modular, "tools": tools }));
    }
    for stage in &config.stages {
        match stage {
            Stage::Translate => section::<translate_agentic::Config>(config, &mut tools)?,
            Stage::Verify => section::<verify_fix_agentic::Config>(config, &mut tools)?,
            Stage::Conform => section::<conform_agentic::Config>(config, &mut tools)?,
        }
    }
    Ok(serde_json::json!({
        "agentic_agent": config.agentic_agent,
        "teamclaude": config.teamclaude,
        "tools": tools
    }))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_config_has_the_tools_of_the_stages() {
        let mut config = Config::mock();
        config.stages = vec![Stage::Translate, Stage::Verify];
        config.tools.insert(
            "verify_fix_agentic".to_owned(),
            serde_json::json!({"fuzz": false, "env": {"KEY": "secret"}}),
        );
        let value = effective_config(&config).unwrap();
        let tools = value["tools"].as_object().unwrap();
        assert_eq!(
            tools.keys().collect::<Vec<_>>(),
            ["translate_agentic", "verify_fix_agentic"]
        );
        // Defaults filled in, overrides kept, secrets left out.
        assert_eq!(tools["translate_agentic"]["tab2rust"], true);
        assert_eq!(tools["verify_fix_agentic"]["fuzz"], false);
        assert!(tools["verify_fix_agentic"].get("env").is_none());
        assert_eq!(value["agentic_agent"], "opencode");
        assert_eq!(value["teamclaude"], true);
    }

    #[test]
    fn effective_config_of_a_direct_translator_drops_secrets() {
        let mut config = Config::mock();
        config.tools.insert(
            "raw_source_to_cargo_llm".to_owned(),
            serde_json::json!({"model": "m", "api_key": "secret"}),
        );
        let value = effective_config(&config).unwrap();
        assert_eq!(
            value["tools"]["raw_source_to_cargo_llm"],
            serde_json::json!({"model": "m"})
        );
        assert_eq!(value["modular"], false);
    }
}

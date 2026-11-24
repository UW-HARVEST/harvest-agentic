//! Agentic conformance ("conform") tool — the third refinement stage.
//!
//! Like translate and verify, conform runs inside the IR pipeline: it consumes
//! the [`CargoPackage`] produced by an earlier stage (or loaded from a
//! snapshot), refines it, and produces a new `CargoPackage`.
//!
//! The distinguishing feature versus verify is its extra input: the
//! [`ExternalTestSuite`] is **revealed** to the agent, whose sole objective is
//! to make every external test pass. The research question this serves: if the
//! external tests are provided, can a third agent close the gap between
//! "passes its own internally generated tests" and "passes the external
//! tests"?
//!
//! Two properties are load-bearing and enforced here:
//!
//! - **The suite reaches only this stage.** It enters the pipeline as its own
//!   representation, and only this tool declares it as an input, so rounds 1
//!   and 2 cannot see the external tests by construction.
//! - **The agent cannot change the suite it is graded on.** The suite is a
//!   read-only input in case_dir's `inputs/`, outside the frozen workspace,
//!   so an edited suite can neither be graded nor leak into a later stage.
//!   [`ReferenceGuard`](harvest_core::fs::ReferenceGuard) reports an edit, so
//!   it leaves a trace in the snapshot.

use agent_runner::case_dir::INPUTS;
use agent_runner::{AgentInvocation, AgentPhase, CaseDir, StageFact};
use full_source::{CargoPackage, ExternalTestSuite, RawSource, TestSuiteKind};
use harvest_core::config::{ToolConfig, unknown_field_warning};
use harvest_core::stage_manifest::SUITE_DIR;
use harvest_core::tools::{RunContext, Tool};
use harvest_core::{Id, Representation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::{info, warn};

/// The prompt files of the stage that are the same in every run, by their name in
/// `inputs/prompts/conform/`: the main agent and the rules of the stage.
const STAGE_FILES: [(&str, &str); 2] = [
    ("main.md", include_str!("../prompts/main.md")),
    (
        "conform_rules.md",
        include_str!("../prompts/conform_rules.md"),
    ),
];
/// The prompt file that tells how to build and run the external suite: the file for the kind of
/// the suite, with the toolchain version of the run ([`test_instructions`]). The files name the
/// suite by its path in the checkout, `inputs/suite` (`INPUTS`/`SUITE_DIR`).
const TESTS_FILE: &str = "tests.md";
const TEST_GTEST: &str = include_str!("../prompts/test_gtest.md");
const TEST_LIB: &str = include_str!("../prompts/test_lib.md");
const TEST_BIN: &str = include_str!("../prompts/test_bin.md");
/// The state file of the main agent, which the framework writes into the workspace as
/// `CONFORM.md` before the agent starts: the format of a log entry. It holds no rules.
const CONFORM_MD: &str = include_str!("conform_md.md");
/// The section of `run.md` that lists the directories of the external suite. The prompt of the
/// main agent names it.
const SUITE_FACT: &str = "External test suite";

/// The agent's final gap-analysis report. It stays in the crate, like its notes
/// file `CONFORM.md`.
const CONFORM_REPORT: &str = "CONFORM_REPORT.md";
/// The log file of the agent, which the framework writes (see [`CONFORM_MD`]).
const CONFORM_LOG: &str = "CONFORM.md";

/// Tool-specific configuration, read from `[tools.conform_agentic]`.
#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    /// Agent timeout in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Agent model to use. If absent, no --model flag is passed and the CLI
    /// uses its default.
    pub model: Option<String>,

    /// Default model of the sub-agents that the main agent dispatches, in the
    /// same format as `model`. If absent, the sub-agents use the main model.
    pub subagent_model: Option<String>,

    /// Extra environment variables to inject into the agent process.
    #[serde(default, skip_serializing)]
    pub env: HashMap<String, String>,

    /// Destination path for the benchmark's output.log file, so the agent's
    /// full trace is appended to the same log as benchmark messages.
    pub output_log_path: Option<PathBuf>,

    /// Directory that records the read-only inputs the agent changed, one file each
    /// (`.harvest/reference_modified/<stage>/`). Injected by the benchmark.
    pub reference_audit_dir: Option<PathBuf>,

    /// Directory that keeps what the agents of this stage are given: the prompt of the main agent
    /// and a copy of the prompt files (`.harvest/prompts/<stage>/`). Injected by the benchmark.
    pub prompt_record_dir: Option<PathBuf>,

    /// The Test-Corpus / cando2 checkout defining the Rust toolchain contract.
    /// Injected from the stage manifest when resuming from a snapshot, whose
    /// C source no longer sits inside the corpus.
    pub test_corpus_root: Option<PathBuf>,

    #[serde(flatten)]
    unknown: HashMap<String, serde_json::Value>,
}

fn default_timeout_secs() -> u64 {
    agent_runner::DEFAULT_AGENT_TIMEOUT_SECS
}

impl ToolConfig for Config {
    const KEY: &'static str = "conform_agentic";
}

impl Config {
    fn validate(&self) {
        unknown_field_warning(&format!("tools.{}", Self::KEY), &self.unknown);
    }
}

pub struct ConformAgentic;

impl Tool for ConformAgentic {
    fn name(&self) -> &'static str {
        "conform_agentic"
    }

    fn run(
        self: Box<Self>,
        context: RunContext,
        inputs: Vec<Id>,
    ) -> Result<Box<dyn Representation>, Box<dyn std::error::Error>> {
        let config: Config = context.config.tool()?;
        config.validate();

        let cargo_package = context
            .ir_snapshot
            .get::<CargoPackage>(inputs[0])
            .ok_or("No CargoPackage representation found in IR")?;
        let raw_source = context
            .ir_snapshot
            .get::<RawSource>(inputs[1])
            .ok_or("No RawSource representation found in IR")?;
        let suite = context
            .ir_snapshot
            .get::<ExternalTestSuite>(inputs[2])
            .ok_or("No ExternalTestSuite representation found in IR")?;

        // The agent refines case_dir/workspace/, the materialized CargoPackage. The C source and
        // the external suite are read-only inputs in case_dir/inputs/ (see agent_runner::CaseDir).
        let case = CaseDir::new()?;
        let workspace = case.workspace();
        cargo_package.dir.materialize(&workspace)?;
        raw_source.dir.materialize(case.c_src())?;
        suite.dir.materialize(case.inputs().join(SUITE_DIR))?;
        info!(
            "Conform working directory: {} ({} suite revealed)",
            workspace.display(),
            suite.kind
        );

        let agent = context.config.agentic_agent;
        let rust_toolchain_context = agent_runner::detect_rust_toolchain_context(
            &context.config.input,
            config.test_corpus_root.as_deref(),
        )?;
        let prompt = agent_runner::main_prompt(AgentPhase::Conform);
        let facts = [suite_fact(suite.kind)];

        let prepared = agent_runner::prepare_agent(AgentInvocation {
            phase: AgentPhase::Conform,
            agent,
            case: &case,
            prompt: &prompt,
            timeout_secs: config.timeout_secs,
            model: config.model.as_deref(),
            subagent_model: config.subagent_model.as_deref(),
            no_plan: false,
            teamclaude: context.config.teamclaude,
            extra_env: &config.env,
            output_log_path: config.output_log_path.as_deref(),
            prompt_record_dir: config.prompt_record_dir.as_deref(),
            rust_toolchain: &rust_toolchain_context,
            additional_prompts: &[],
            stage_facts: &facts,
        })?;
        write_stage_files(
            &case,
            &test_instructions(suite.kind, &rust_toolchain_context.required_version),
        )?;
        // Guarding the suite is what keeps this stage honest: an agent that "passes" by editing
        // the tests is reported. The grade runs a pristine copy of the suite in any case.
        let reference_guard = case.lock_inputs()?;
        prepared.run()?;
        info!("Conformance refinement complete");

        if !workspace.join(CONFORM_REPORT).is_file() {
            warn!(
                "conform agent did not produce {CONFORM_REPORT} (internal-vs-external gap analysis missing)"
            );
        }

        let dir = case.freeze(&reference_guard, config.reference_audit_dir.as_deref())?;
        Ok(Box::new(CargoPackage { dir }))
    }
}

/// Writes the files of the stage. Into `inputs/prompts/conform/`: the prompt files of the stage
/// and `tests.md`. Into the workspace: `CONFORM.md`, the state file of the main agent.
fn write_stage_files(case: &CaseDir, tests: &str) -> std::io::Result<()> {
    let stage = AgentPhase::Conform.prompt_dir();
    case.write_prompts(stage, &STAGE_FILES)?;
    case.write_prompts(stage, &[(TESTS_FILE, tests)])?;
    fs::write(case.workspace().join(CONFORM_LOG), CONFORM_MD)
}

/// The fact of `run.md` about the external suite: its kind, and its directories as a
/// backtick-quoted list.
fn suite_fact(kind: TestSuiteKind) -> StageFact<'static> {
    let suite = format!("{INPUTS}/{SUITE_DIR}");
    let dirs = kind
        .dirs()
        .iter()
        .map(|d| format!("`{suite}/{d}/`"))
        .collect::<Vec<_>>()
        .join(", ");
    StageFact {
        heading: SUITE_FACT,
        text: format!("- Kind: {kind}\n- Directories: {dirs}"),
    }
}

/// The text of `tests.md`: how to build and run the external tests of this kind of suite, with
/// the toolchain version of the run. It mirrors what the benchmark grader runs, so that the agent
/// iterates against the same commands.
fn test_instructions(kind: TestSuiteKind, toolchain: &str) -> String {
    let text = match kind {
        TestSuiteKind::Gtest => TEST_GTEST,
        TestSuiteKind::Lib => TEST_LIB,
        TestSuiteKind::Bin => TEST_BIN,
    };
    text.replace("{TOOLCHAIN}", toolchain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn prompt_files_are_consistent_for_every_kind_of_suite() {
        let suite = format!("{INPUTS}/{SUITE_DIR}");
        for kind in [TestSuiteKind::Gtest, TestSuiteKind::Lib, TestSuiteKind::Bin] {
            let case = CaseDir::new().unwrap();
            let toolchain = agent_runner::RustToolchainContext::for_version("1.99.0");
            let env = HashMap::new();
            let facts = [suite_fact(kind)];
            agent_runner::prepare_agent(AgentInvocation {
                phase: AgentPhase::Conform,
                agent: harvest_core::config::AgentKind::OpenCode,
                case: &case,
                prompt: "",
                timeout_secs: 1,
                model: None,
                subagent_model: None,
                no_plan: false,
                teamclaude: true,
                extra_env: &env,
                output_log_path: None,
                prompt_record_dir: None,
                rust_toolchain: &toolchain,
                additional_prompts: &[],
                stage_facts: &facts,
            })
            .unwrap();
            write_stage_files(&case, &test_instructions(kind, "1.99.0")).unwrap();
            let prompt = agent_runner::main_prompt(AgentPhase::Conform);
            let name = format!("conform-{kind}");
            agent_runner::prompt_check::dump(&case, &name, &prompt, &[CONFORM_LOG]).unwrap();

            let inputs = case.inputs();
            assert_eq!(
                agent_runner::prompt_check::problems(&inputs).unwrap(),
                [""; 0],
                "{kind}"
            );
            let files = agent_runner::prompt_check::load(&inputs).unwrap();
            let read: Vec<String> =
                agent_runner::prompt_check::must_read(&files, "conform/main.md")
                    .into_iter()
                    .collect();
            assert_eq!(
                read,
                [
                    "cargo_features.md",
                    "common.md",
                    "conform/conform_rules.md",
                    "conform/main.md",
                    "conform/tests.md",
                    "git_main.md",
                    "main_agent.md",
                    "run.md",
                    "runtime.md"
                ],
                "{kind}"
            );
            // The static files name the suite by the path that the framework gives it, and
            // run.md lists its directories under the heading that the main prompt names.
            let main = &files["conform/main.md"];
            assert!(main.contains(&format!("`{suite}/`")) && main.contains(SUITE_FACT));
            assert!(files["conform/conform_rules.md"].contains(&format!("`{suite}/`")));
            for dir in kind.dirs() {
                assert!(
                    files["run.md"].contains(&format!("`{suite}/{dir}/`")),
                    "{dir}"
                );
            }
            let tests = &files["conform/tests.md"];
            assert!(tests.contains("RUSTUP_TOOLCHAIN=1.99.0"));
            // The files of the suite kinds write out the path of the suite in the checkout.
            assert_eq!(suite, "inputs/suite");
            assert!(tests.contains(&format!("`{suite}/")) && !tests.contains("{SUITE}"));
            assert!(files["run.md"].contains(&format!("- Kind: {kind}\n")));

            let log = fs::read_to_string(case.workspace().join(CONFORM_LOG)).unwrap();
            assert!(log.contains("## Log") && log.contains("### C<N>:"));
            assert!(!log.contains("Invariants") && !log.contains("inputs/prompts"));
            case.finish();
        }
    }

    #[test]
    fn main_prompt_only_names_the_prompt_file_of_the_stage() {
        let prompt = agent_runner::main_prompt(AgentPhase::Conform);
        assert_eq!(prompt.lines().count(), 2);
        assert!(prompt.contains("inputs/prompts/conform/main.md"));
    }
}

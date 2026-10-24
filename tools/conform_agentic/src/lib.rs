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
use agent_runner::{AgentInvocation, AgentPhase, CaseDir};
use full_source::{CargoPackage, ExternalTestSuite, RawSource, TestSuiteKind};
use harvest_core::config::{ToolConfig, unknown_field_warning};
use harvest_core::stage_manifest::SUITE_DIR;
use harvest_core::tools::{RunContext, Tool};
use harvest_core::{Id, Representation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{info, warn};

const PROMPT_CONFORM: &str = include_str!("prompt_conform.md");
/// How to build and run each kind of external suite (`{CONFORM_TEST_INSTRUCTIONS}`).
const TEST_GTEST: &str = include_str!("test_gtest.md");
const TEST_LIB: &str = include_str!("test_lib.md");
const TEST_BIN: &str = include_str!("test_bin.md");

/// The agent's final gap-analysis report. It stays in the crate, like its notes
/// file `CONFORM.md`.
const CONFORM_REPORT: &str = "CONFORM_REPORT.md";

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
    /// Not supported with Kiro.
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
        // Guarding the suite is what keeps this stage honest: an agent that "passes" by editing
        // the tests is reported. The grade runs a pristine copy of the suite in any case.
        let reference_guard = case.lock_inputs()?;
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
        let suite_path = format!("{INPUTS}/{SUITE_DIR}");
        let prompt = PROMPT_CONFORM
            .replace(
                "{CONFORM_TEST_INSTRUCTIONS}",
                &test_instructions(
                    suite.kind,
                    &suite_path,
                    &rust_toolchain_context.required_version,
                ),
            )
            .replace(
                "{EXTERNAL_TEST_DIRS}",
                &suite_dir_list(suite.kind, &suite_path),
            );

        agent_runner::invoke_agent(AgentInvocation {
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
            rust_toolchain: &rust_toolchain_context,
        })?;
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

/// Renders the suite's directories as the backtick-quoted list the prompt
/// splices into `{EXTERNAL_TEST_DIRS}`.
fn suite_dir_list(kind: TestSuiteKind, suite: &str) -> String {
    kind.dirs()
        .iter()
        .map(|d| format!("`{suite}/{d}/`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Builds the harness-specific "how to build and run the external tests" block
/// injected into the prompt. Mirrors exactly what the benchmark grader runs so
/// the agent iterates against the same commands.
fn test_instructions(kind: TestSuiteKind, suite: &str, toolchain: &str) -> String {
    let text = match kind {
        TestSuiteKind::Gtest => TEST_GTEST,
        TestSuiteKind::Lib => TEST_LIB,
        TestSuiteKind::Bin => TEST_BIN,
    };
    text.trim_end()
        .replace("{SUITE}", suite)
        .replace("{TOOLCHAIN}", toolchain)
}

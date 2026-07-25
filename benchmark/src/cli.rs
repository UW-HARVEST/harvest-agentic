use clap::{Parser, ValueEnum};
use std::path::PathBuf;

/// Which validation harness to run translated projects against.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestHarness {
    /// Prefer a gtest_suite/ when the test case provides one, otherwise fall
    /// back to library (cando2 runner) or executable (driver) validation.
    Auto,
    /// Force the GoogleTest suite (error if the test case has no gtest_suite/).
    Gtest,
    /// Force cando2 library validation (runner/ + test_vectors/).
    Lib,
    /// Force executable validation (driver binary against test_vectors/).
    Bin,
}

/// Which comparison mechanism the in-loop verification agent is given.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VerifyHarness {
    /// The agent gets a C++ GoogleTest environment with the C reference linked
    /// in and the Rust translation loaded via dlopen.
    #[default]
    Gtest,
    /// The agent dlopens the C `.so` from a Rust integration test.
    Libloading,
}

#[derive(Parser)]
#[command(name = "harvest-benchmark")]
#[command(
    about = "Runs all benchmarks by translating C projects to Rust and validating them with test vectors"
)]
pub struct Args {
    /// Input directory containing subdirectories with benchmarks
    #[arg(
        required_unless_present = "test",
        help = "Path to the directory containing example subdirectories (each with test_case/ and test_vectors/)"
    )]
    pub input_dir: Option<PathBuf>,

    /// Output directory where the translated Rust projects will be written
    #[arg(
        required_unless_present = "test",
        help = "Path to the output directory for all translated Rust projects"
    )]
    pub output_dir: Option<PathBuf>,

    /// Test an already-translated output directory without running translation.
    /// Accepts either an output root containing program subdirectories, or one
    /// translated program directory with Cargo.toml, runner/, and test_vectors/.
    #[arg(
        long,
        conflicts_with_all = [
            "modular",
            "agentic",
            "agentic_verify",
            "agentic_agent",
            "agentic_model",
            "no_plan",
            "no_plan_file",
            "workflow",
            "agent_tools",
            "config",
            "wait_until",
            "input_dir",
            "output_dir"
        ]
    )]
    pub test: Option<PathBuf>,

    /// Third-stage conformance refinement mode. Takes an already-translated
    /// (post verify) program directory as input_dir and writes a refined copy
    /// to output_dir. The external test suite (gtest_suite/ or the tractor
    /// runner+vectors already present in the input) is revealed to the agent,
    /// whose objective is to make every external test pass; the refined result
    /// is then graded independently against a pristine copy of the same tests.
    /// Decoupled from translate/verify. Requires --agentic-agent (and usually
    /// --agentic-model). Test selection follows --test-harness (default auto,
    /// gtest preferred).
    #[arg(
        long,
        conflicts_with_all = [
            "test",
            "modular",
            "agentic",
            "agentic_verify",
            "no_plan",
            "no_plan_file",
            "workflow",
            "agent_tools",
            "wait_until"
        ]
    )]
    pub conform: bool,

    /// Use modular translation rather than standard all-at-once translation.
    #[arg(long, conflicts_with = "agentic")]
    pub modular: bool,

    /// Use the agentic translation tool.
    #[arg(long, conflicts_with = "modular")]
    pub agentic: bool,

    /// Run the agentic verify-and-fix stage after translation (requires --agentic).
    #[arg(long, requires = "agentic")]
    pub agentic_verify: bool,

    /// Which agent to use: kiro, claude, or opencode. Used by --agentic and by
    /// --conform (both need an agent).
    #[arg(long)]
    pub agentic_agent: Option<String>,

    /// Agent model to use for agentic translation/verification/conformance.
    /// Claude accepts short aliases ("sonnet", "opus", "haiku") or full model IDs.
    /// OpenCode expects provider/model format (for example, "opencode-go/deepseek-v4-pro").
    #[arg(long)]
    pub agentic_model: Option<String>,

    /// Use the pre-883e2e2 prompts (no PLAN.md / HYPOTHESES.md / Invariants /
    /// sub-agent push) and skip the `--append-system-prompt` flag. For
    /// controlled experiments measuring the impact of the anti-compaction
    /// mechanism. Applies to both translator and verifier. Requires --agentic.
    #[arg(long, requires = "agentic")]
    pub no_plan: bool,

    /// Ablation mode: keep the sub-agent push and context-management guidance
    /// from the standard prompts, but never mention PLAN.md / HYPOTHESES.md or
    /// writing plans to disk (the agent may still do so spontaneously), and
    /// skip the `--append-system-prompt` compaction-recovery hint. Isolates
    /// the effect of plan-file persistence from sub-agent usage. Applies to
    /// both translator and verifier. Requires --agentic; mutually exclusive
    /// with --no-plan.
    #[arg(long, requires = "agentic", conflicts_with = "no_plan")]
    pub no_plan_file: bool,

    /// Inject a prompt hint encouraging the agent to use dynamic workflows
    /// (Claude Code's multi-agent orchestration feature). Only meaningful with
    /// --no-plan; requires --agentic and --agentic-agent claude.
    #[arg(long, requires = "no_plan")]
    pub workflow: bool,

    /// Provide the agent with pre-built analysis tools (c_sandbox, symbol_diff).
    /// Only meaningful when --agentic is set.
    #[arg(long, requires = "agentic")]
    pub agent_tools: bool,

    /// Comparison mechanism the in-loop verification agent is given.
    #[arg(long, value_enum, default_value_t = VerifyHarness::Gtest, requires = "agentic_verify")]
    pub verify_harness: VerifyHarness,

    /// With --verify-harness gtest, also describe FuzzTest
    /// to the agent and ship its scaffolding. No effect
    /// under the libloading harness.
    #[arg(long, requires = "agentic_verify")]
    pub fuzz: bool,

    /// Set a configuration value; format $NAME=$VALUE.
    #[arg(long, short)]
    pub config: Vec<String>,

    /// Validation harness selection. Defaults to auto: use the GoogleTest
    /// suite when the test case ships a gtest_suite/ directory, otherwise the
    /// existing library (cando2) or executable (driver) validation.
    #[arg(long, value_enum, default_value_t = TestHarness::Auto)]
    pub test_harness: TestHarness,

    /// Timeout in seconds for running test cases
    #[arg(long, default_value = "10")]
    pub timeout: u64,

    /// Filter benchmarks by regex pattern on directory names (keeps matching directories).
    /// Examples: ".*_lib$" (only libraries)
    /// Cannot be used together with --exclude.
    #[arg(long, conflicts_with = "exclude")]
    pub filter: Option<String>,

    /// Exclude benchmarks by regex pattern on directory names (removes matching directories).
    /// Examples: ".*_lib$" (exclude libraries)
    /// Cannot be used together with --filter.
    #[arg(long, conflicts_with = "filter")]
    pub exclude: Option<String>,

    /// Unix timestamp. If set with --agentic-verify, the verification agent
    /// will wait until this time before starting. Useful for aligning with
    /// the 5-hour free window reset. If the current time is already past the
    /// timestamp, verification starts immediately.
    #[arg(long, requires = "agentic_verify")]
    pub wait_until: Option<u64>,
}

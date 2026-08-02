use clap::{Parser, ValueEnum};
use harvest_core::config::{AgentKind, Stage};
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

fn parse_agent_kind(s: &str) -> Result<AgentKind, String> {
    match s.to_lowercase().as_str() {
        "kiro" => Ok(AgentKind::Kiro),
        "claude" => Ok(AgentKind::Claude),
        "opencode" | "oc" => Ok(AgentKind::OpenCode),
        other => Err(format!(
            "unknown agent kind: {other} (expected: kiro, claude, opencode)"
        )),
    }
}

#[derive(Parser)]
#[command(name = "harvest-benchmark")]
#[command(
    about = "Runs benchmarks by translating C projects to Rust and validating them with test suites.\n\
             The agentic pipeline is stage-composable: --agentic[=STAGES] selects which stages run,\n\
             and INPUT_DIR is whatever the first selected stage consumes (a bench test-case root for\n\
             translate; a previous run's output root for verify/conform)."
)]
pub struct Args {
    /// Input directory. Its meaning follows the first stage of the run:
    /// for translate (and the non-agentic modes) it is a bench test-case root;
    /// for verify/conform it is a previous run's output root (a snapshot
    /// carrying harvest_stage.json manifests).
    #[arg(required_unless_present = "test")]
    pub input_dir: Option<PathBuf>,

    /// Output directory where this run's products will be written.
    #[arg(required_unless_present = "test")]
    pub output_dir: Option<PathBuf>,

    /// Test an already-translated output directory without running any stage.
    /// Accepts either an output root containing program subdirectories, or one
    /// translated program directory.
    #[arg(
        long,
        conflicts_with_all = [
            "modular",
            "agentic",
            "agent",
            "model",
            "no_plan",
            "no_plan_file",
            "workflow",
            "agent_tools",
            "config",
            "test_case",
            "input_dir",
            "output_dir"
        ]
    )]
    pub test: Option<PathBuf>,

    /// Use modular translation rather than standard all-at-once translation.
    #[arg(long, conflicts_with = "agentic")]
    pub modular: bool,

    /// Run the agentic pipeline. The optional value selects the stages, in
    /// pipeline order (aliases: t, v, c): `--agentic=translate` freezes a
    /// translator snapshot, `--agentic=verify` resumes from one,
    /// `--agentic=conform` refines against the external suite. Bare
    /// `--agentic` means translate,verify. Conform currently runs alone.
    #[arg(
        long,
        conflicts_with = "modular",
        num_args(0..=1),
        require_equals = true,
        default_missing_values = ["translate", "verify"],
        value_delimiter = ',',
        value_parser = clap::builder::ValueParser::new(|s: &str| s.parse::<Stage>())
    )]
    pub agentic: Option<Vec<Stage>>,

    /// Which agent to use: kiro, claude, or opencode.
    #[arg(long, requires = "agentic", value_parser = parse_agent_kind)]
    pub agent: Option<AgentKind>,

    /// Agent model for the agentic stages of this run.
    /// Claude accepts short aliases ("sonnet", "opus", "haiku") or full model IDs.
    /// OpenCode expects provider/model format (for example, "opencode-go/deepseek-v4-pro").
    #[arg(long, requires = "agentic")]
    pub model: Option<String>,

    /// Override the bench test-case location when resuming from a snapshot
    /// (first stage verify/conform). By default the bench reference is read
    /// from each snapshot's harvest_stage.json. Accepts a bench root
    /// containing program subdirectories, or a single bench program directory.
    #[arg(long, requires = "agentic")]
    pub test_case: Option<PathBuf>,

    /// Use the pre-883e2e2 prompts (no PLAN.md / HYPOTHESES.md / Invariants /
    /// sub-agent push) and skip the `--append-system-prompt` flag. For
    /// controlled experiments measuring the impact of the anti-compaction
    /// mechanism. Applies to the translate/verify stages of this run.
    #[arg(long, requires = "agentic")]
    pub no_plan: bool,

    /// Ablation mode: keep the sub-agent push and context-management guidance
    /// from the standard prompts, but never mention PLAN.md / HYPOTHESES.md or
    /// writing plans to disk (the agent may still do so spontaneously), and
    /// skip the `--append-system-prompt` compaction-recovery hint. Isolates
    /// the effect of plan-file persistence from sub-agent usage. Applies to
    /// the translate/verify stages of this run.
    #[arg(long, requires = "agentic", conflicts_with = "no_plan")]
    pub no_plan_file: bool,

    /// Inject a prompt hint encouraging the agent to use dynamic workflows
    /// (Claude Code's multi-agent orchestration feature). Only meaningful with
    /// --no-plan; requires --agent claude.
    #[arg(long, requires = "no_plan")]
    pub workflow: bool,

    /// Provide the agent with pre-built analysis tools (c_sandbox, symbol_diff).
    #[arg(long, requires = "agentic")]
    pub agent_tools: bool,

    /// Comparison mechanism the in-loop verification agent is given
    /// (default: gtest). Requires the verify stage.
    #[arg(long, value_enum, requires = "agentic")]
    pub verify_harness: Option<VerifyHarness>,

    /// With the gtest verify harness, also describe FuzzTest to the agent and
    /// ship its scaffolding. Requires the verify stage.
    #[arg(long, requires = "agentic")]
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
}

impl Args {
    /// The agentic stages of this run, normalized to pipeline order and
    /// deduplicated. Empty when the run is not agentic.
    pub fn stages(&self) -> Vec<Stage> {
        let mut stages = self.agentic.clone().unwrap_or_default();
        stages.sort();
        stages.dedup();
        stages
    }

    /// Validates flag/stage combinations that clap cannot express (they
    /// depend on the *value* of --agentic, not its presence).
    pub fn validate_stages(&self, stages: &[Stage]) -> Result<(), String> {
        let has = |s: Stage| stages.contains(&s);
        let stages_str = || {
            stages
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        };
        if self.agentic.is_some() && stages.is_empty() {
            return Err("--agentic requires at least one stage".to_owned());
        }
        if has(Stage::Conform) && stages.len() > 1 {
            return Err(format!(
                "stage conform currently runs alone (got --agentic={})",
                stages_str()
            ));
        }
        let verify_only_flags = [
            (self.fuzz, "--fuzz"),
            (self.verify_harness.is_some(), "--verify-harness"),
        ];
        for (set, flag) in verify_only_flags {
            if set && !has(Stage::Verify) {
                return Err(format!(
                    "{flag} requires the verify stage (got --agentic={})",
                    stages_str()
                ));
            }
        }
        let translate_or_verify_flags = [
            (self.no_plan, "--no-plan"),
            (self.no_plan_file, "--no-plan-file"),
            (self.workflow, "--workflow"),
            (self.agent_tools, "--agent-tools"),
        ];
        for (set, flag) in translate_or_verify_flags {
            if set && !has(Stage::Translate) && !has(Stage::Verify) {
                return Err(format!(
                    "{flag} requires the translate or verify stage (got --agentic={})",
                    stages_str()
                ));
            }
        }
        if self.test_case.is_some() && stages.first() != Some(&Stage::Verify) {
            return Err(format!(
                "--test-case only applies when resuming from a snapshot with verify as \
                 the first stage (got --agentic={}); conform grades against the snapshot's \
                 own suite",
                stages_str()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> Result<Args, clap::Error> {
        Args::try_parse_from(argv)
    }

    #[test]
    fn bare_agentic_means_translate_verify() {
        let args = parse(&["bench", "--agentic", "in", "out"]).unwrap();
        assert_eq!(args.stages(), [Stage::Translate, Stage::Verify]);
        args.validate_stages(&args.stages()).unwrap();
    }

    #[test]
    fn stage_aliases_and_normalization() {
        let args = parse(&["bench", "--agentic=v,T,v", "in", "out"]).unwrap();
        assert_eq!(args.stages(), [Stage::Translate, Stage::Verify]);
        let args = parse(&["bench", "--agentic=c", "in", "out"]).unwrap();
        assert_eq!(args.stages(), [Stage::Conform]);
    }

    #[test]
    fn space_separated_stage_value_is_rejected() {
        // require_equals: `--agentic verify` must not swallow a positional.
        let args = parse(&["bench", "--agentic", "in", "out"]).unwrap();
        assert_eq!(args.input_dir.as_deref().unwrap().to_str(), Some("in"));
        assert!(parse(&["bench", "--agentic", "verify", "in", "out"]).is_err());
    }

    #[test]
    fn fuzz_requires_verify_stage() {
        let args = parse(&["bench", "--agentic=t", "--fuzz", "in", "out"]).unwrap();
        let err = args.validate_stages(&args.stages()).unwrap_err();
        assert!(err.contains("--fuzz"), "{err}");
    }

    #[test]
    fn conform_must_run_alone() {
        let args = parse(&["bench", "--agentic=v,c", "in", "out"]).unwrap();
        let err = args.validate_stages(&args.stages()).unwrap_err();
        assert!(err.contains("conform"), "{err}");
    }

    #[test]
    fn test_case_only_for_verify_first() {
        let args = parse(&["bench", "--agentic=t,v", "--test-case", "tc", "in", "out"]).unwrap();
        assert!(args.validate_stages(&args.stages()).is_err());
        let args = parse(&["bench", "--agentic=v", "--test-case", "tc", "in", "out"]).unwrap();
        args.validate_stages(&args.stages()).unwrap();
    }

    #[test]
    fn test_mode_conflicts_with_agentic() {
        assert!(parse(&["bench", "--test", "out", "--agentic"]).is_err());
        assert!(parse(&["bench", "--test", "out"]).is_ok());
    }
}

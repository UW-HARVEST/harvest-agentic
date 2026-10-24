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

impl TestHarness {
    /// The external-suite kind this selection forces on the conform stage, or
    /// `None` for `auto` (let the loader detect it from the bench directory).
    pub fn suite_kind(self) -> Option<full_source::TestSuiteKind> {
        match self {
            TestHarness::Auto => None,
            TestHarness::Gtest => Some(full_source::TestSuiteKind::Gtest),
            TestHarness::Lib => Some(full_source::TestSuiteKind::Lib),
            TestHarness::Bin => Some(full_source::TestSuiteKind::Bin),
        }
    }
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

/// One part of what `--test` grades.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestPart {
    /// Run the external test suites.
    #[value(alias = "c")]
    Correctness,
    /// Measure safety and idiomaticity.
    #[value(alias = "q")]
    Quality,
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
    /// for verify/conform it is a previous run's output root, whose program
    /// directories are self-contained snapshots (see .harvest/). With --test
    /// it is the output to re-grade: an output root, or one program snapshot.
    pub input_dir: PathBuf,

    /// Output directory where this run's products will be written. Not used
    /// with --test, which grades INPUT_DIR in place.
    #[arg(required_unless_present = "test")]
    pub output_dir: Option<PathBuf>,

    /// Re-grade an already-translated output (INPUT_DIR) without running any
    /// stage. The optional value selects what is graded (aliases: c, q):
    /// `correctness` runs the external test suites (results.csv), `quality`
    /// measures safety and idiomaticity (.harvest/quality.json, quality.csv).
    /// Bare `--test` means `--test=correctness,quality`.
    #[arg(
        long,
        value_name = "PARTS",
        num_args(0..=1),
        require_equals = true,
        default_missing_values = ["correctness", "quality"],
        value_delimiter = ',',
        value_enum,
        ignore_case = true,
        conflicts_with_all = [
            "modular",
            "agentic",
            "agent",
            "model",
            "subagent_model",
            "no_plan",
            "workflow",
            "config",
            "test_case",
            "output_dir"
        ]
    )]
    pub test: Option<Vec<TestPart>>,

    /// Use modular translation rather than standard all-at-once translation.
    #[arg(long, conflicts_with = "agentic")]
    pub modular: bool,

    /// Run the agentic pipeline. The optional value selects the stages, in
    /// pipeline order (aliases: t, v, c): `--agentic=translate` freezes a
    /// translator snapshot, `--agentic=verify` resumes from one,
    /// `--agentic=conform` refines against the external suite. Bare
    /// `--agentic` means translate,verify.
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

    /// Which agent to use: opencode (default), claude, or kiro.
    #[arg(long, requires = "agentic", value_parser = |s: &str| s.parse::<AgentKind>())]
    pub agent: Option<AgentKind>,

    /// Agent model for the agentic stages of this run.
    /// Claude accepts short aliases ("sonnet", "opus", "haiku") or full model IDs.
    /// OpenCode expects provider/model format (for example, "opencode-go/deepseek-v4-pro").
    #[arg(long, requires = "agentic")]
    pub model: Option<String>,

    /// Default model of the sub-agents (scouts included) that the main agent
    /// dispatches, in the same format as --model. Without it, the sub-agents
    /// use the main model. The main agent cannot override it per sub-agent.
    /// Applies to every agentic stage of the run.
    #[arg(long, requires = "agentic")]
    pub subagent_model: Option<String>,

    /// Start Claude Code directly, or through CCR for a `provider,model`
    /// model, also when the `teamclaude` command is on PATH. Without this
    /// flag, Claude Code runs through TeamClaude (`teamclaude run -- ...`)
    /// when it is installed, and a CCR model is then an error. No effect on
    /// the other agents.
    #[arg(long, requires = "agentic")]
    pub no_teamclaude: bool,

    /// Grade and refine against a bench directory's *current* external test
    /// suite instead of the one the snapshot carries. Only applies when
    /// resuming (first stage verify or conform). Accepts a bench root
    /// containing program subdirectories, or a single bench program directory.
    #[arg(long, requires = "agentic")]
    pub test_case: Option<PathBuf>,

    /// Use the pre-883e2e2 prompts (no PLAN.md / HYPOTHESES.md / Invariants /
    /// sub-agent push) and skip the `--append-system-prompt` flag. For
    /// controlled experiments measuring the impact of the anti-compaction
    /// mechanism. Applies to the translate/verify stages of this run.
    #[arg(long, requires = "agentic")]
    pub no_plan: bool,

    /// Inject a prompt hint encouraging the agent to use dynamic workflows
    /// (Claude Code's multi-agent orchestration feature). Only meaningful with
    /// --no-plan; requires --agent claude.
    #[arg(long, requires = "no_plan")]
    pub workflow: bool,

    /// Comparison mechanism the in-loop verification agent is given
    /// (default: gtest). Requires the verify stage.
    #[arg(long, value_enum, requires = "agentic")]
    pub verify_harness: Option<VerifyHarness>,

    /// Do not give the translate stage the tab2rust skill. By default the
    /// scouts find the C data tables with tab2rust and the translation
    /// sub-agents generate them. Requires the translate stage. Only the plan
    /// prompt of Claude and OpenCode uses tab2rust, so the translate stage
    /// stops with an error when the flag is given with --no-plan or Kiro.
    #[arg(long, requires = "agentic")]
    pub no_tab2rust: bool,

    /// Send a Safe API Wrapper Designer with the scouts of the translate
    /// stage. It designs the safe Rust boundary of the C library, and the
    /// translation follows the design. Requires the translate stage. Only the
    /// plan prompt of Claude and OpenCode sends scouts, so the translate stage
    /// stops with an error when the flag is given with --no-plan or Kiro.
    #[arg(long, requires = "agentic")]
    pub safe_wrapper: bool,

    /// Turn off FuzzTest in the verify stage. By default the gtest verify
    /// harness also describes FuzzTest to the agent and ships its
    /// scaffolding; with this flag the agent gets plain GoogleTest only.
    /// Requires the verify stage.
    #[arg(long, requires = "agentic")]
    pub no_fuzz: bool,

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

    /// Erase an output program directory that already holds results, instead
    /// of refusing to write into it. Without this, a populated directory is an
    /// error: materializing over one leaves the previous run's files in place
    /// and the mixture travels forward as if it were a single run's output.
    #[arg(long)]
    pub force: bool,
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
        let translate_only_flags = [
            (self.no_tab2rust, "--no-tab2rust"),
            (self.safe_wrapper, "--safe-wrapper"),
        ];
        for (set, flag) in translate_only_flags {
            if set && !has(Stage::Translate) {
                return Err(format!(
                    "{flag} requires the translate stage (got --agentic={})",
                    stages_str()
                ));
            }
        }
        // Every agentic stage checks the resolved config too (an agent set in a config file); this
        // reports a bad combination once, before the run, instead of once per program.
        agent_runner::effective_subagent_model(
            self.agent.unwrap_or_default(),
            self.model.as_deref(),
            self.subagent_model.as_deref(),
        )?;
        // The Claude backend checks the resolved config too; this reports a CCR model that cannot
        // run through TeamClaude once, before the run, instead of once per program.
        if self.agent == Some(AgentKind::Claude) {
            agent_runner::claude_uses_teamclaude(
                !self.no_teamclaude && self.config_bool("teamclaude") != Some(false),
                self.model.as_deref(),
                self.subagent_model.as_deref(),
            )?;
        }
        // The translate stage checks the resolved config too (an agent set in a config file).
        for (set, flag) in translate_only_flags {
            if set {
                translate_agentic::check_scout_option(
                    flag,
                    self.no_plan,
                    self.agent.unwrap_or_default(),
                )?;
            }
        }
        let verify_only_flags = [
            (self.no_fuzz, "--no-fuzz"),
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
        let translate_or_verify_flags =
            [(self.no_plan, "--no-plan"), (self.workflow, "--workflow")];
        for (set, flag) in translate_or_verify_flags {
            if set && !has(Stage::Translate) && !has(Stage::Verify) {
                return Err(format!(
                    "{flag} requires the translate or verify stage (got --agentic={})",
                    stages_str()
                ));
            }
        }
        if self.test_case.is_some() && matches!(stages.first(), None | Some(Stage::Translate)) {
            return Err(format!(
                "--test-case only applies when resuming from a snapshot, i.e. when the first \
                 stage is verify or conform (got --agentic={})",
                stages_str()
            ));
        }
        Ok(())
    }

    /// The last `-c key=value` of a top-level boolean config key.
    fn config_bool(&self, key: &str) -> Option<bool> {
        self.config.iter().rev().find_map(|entry| {
            let (name, value) = entry.split_once('=')?;
            (name == key).then(|| value.parse().ok()).flatten()
        })
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
        assert_eq!(args.input_dir, PathBuf::from("in"));
        assert!(parse(&["bench", "--agentic", "verify", "in", "out"]).is_err());
    }

    #[test]
    fn no_fuzz_requires_verify_stage() {
        let args = parse(&["bench", "--agentic=t", "--no-fuzz", "in", "out"]).unwrap();
        let err = args.validate_stages(&args.stages()).unwrap_err();
        assert!(err.contains("--no-fuzz"), "{err}");
    }

    #[test]
    fn scout_flags_require_translate_stage() {
        for flag in ["--no-tab2rust", "--safe-wrapper"] {
            let args = parse(&["bench", "--agentic=v", flag, "in", "out"]).unwrap();
            let err = args.validate_stages(&args.stages()).unwrap_err();
            assert!(err.contains(flag), "{err}");
            let args = parse(&["bench", "--agentic=t,v", flag, "in", "out"]).unwrap();
            args.validate_stages(&args.stages()).unwrap();
        }
    }

    #[test]
    fn subagent_model_is_checked_before_the_run() {
        // The combinations themselves are tested in agent_runner.
        let args = parse(&[
            "bench",
            "--agentic=t",
            "--agent",
            "kiro",
            "--subagent-model",
            "x",
            "in",
            "out",
        ])
        .unwrap();
        assert!(args.validate_stages(&args.stages()).is_err());
        assert!(parse(&["bench", "--subagent-model", "x", "in", "out"]).is_err());
    }

    #[test]
    fn no_teamclaude_allows_ccr_models() {
        // Whether TeamClaude is used without the flag depends on PATH; the combinations are
        // tested in agent_runner.
        for off in [&["--no-teamclaude"][..], &["-c", "teamclaude=false"]] {
            let mut argv = vec![
                "bench",
                "--agentic=t",
                "--agent",
                "claude",
                "--model",
                "openrouter,a/b",
            ];
            argv.extend_from_slice(off);
            argv.extend(["in", "out"]);
            let args = parse(&argv).unwrap();
            args.validate_stages(&args.stages()).unwrap();
        }
        assert!(parse(&["bench", "--no-teamclaude", "in", "out"]).is_err());
    }

    #[test]
    fn scout_flags_need_a_prompt_that_sends_scouts() {
        for flag in ["--no-tab2rust", "--safe-wrapper"] {
            for extra in [&["--no-plan"][..], &["--agent", "kiro"]] {
                let mut argv = vec!["bench", "--agentic=t", flag];
                argv.extend_from_slice(extra);
                argv.extend(["in", "out"]);
                let args = parse(&argv).unwrap();
                let err = args.validate_stages(&args.stages()).unwrap_err();
                assert!(err.starts_with(flag), "{err}");
            }
        }
    }

    #[test]
    fn conform_composes_with_the_other_stages() {
        for spec in ["--agentic=c", "--agentic=v,c", "--agentic=t,v,c"] {
            let args = parse(&["bench", spec, "in", "out"]).unwrap();
            let stages = args.stages();
            assert_eq!(stages.last(), Some(&Stage::Conform), "{spec}");
            args.validate_stages(&stages).unwrap();
        }
    }

    #[test]
    fn test_case_only_when_resuming_from_a_snapshot() {
        let args = parse(&["bench", "--agentic=t,v", "--test-case", "tc", "in", "out"]).unwrap();
        assert!(args.validate_stages(&args.stages()).is_err());
        for spec in ["--agentic=v", "--agentic=c"] {
            let args = parse(&["bench", spec, "--test-case", "tc", "in", "out"]).unwrap();
            args.validate_stages(&args.stages()).unwrap();
        }
    }

    #[test]
    fn test_parts_follow_the_agentic_value_syntax() {
        use TestPart::{Correctness, Quality};
        for (argv, parts) in [
            (vec!["bench", "--test", "out"], vec![Correctness, Quality]),
            (
                vec!["bench", "--test=c,q", "out"],
                vec![Correctness, Quality],
            ),
            (
                vec!["bench", "--test=quality,correctness", "out"],
                vec![Quality, Correctness],
            ),
            (vec!["bench", "--test=q", "out"], vec![Quality]),
            (vec!["bench", "--test=Q", "out"], vec![Quality]),
        ] {
            let args = parse(&argv).unwrap();
            assert_eq!(args.test, Some(parts), "{argv:?}");
            assert_eq!(args.input_dir, PathBuf::from("out"));
        }
        // Like --agentic, one-letter forms are not run together.
        assert!(parse(&["bench", "--test=cq", "out"]).is_err());
        assert!(parse(&["bench", "--test=x", "out"]).is_err());
        assert!(parse(&["bench", "--test=", "out"]).is_err());
        // The graded directory is the positional input; there is no output.
        assert!(parse(&["bench", "--test", "out", "other"]).is_err());
        assert!(parse(&["bench", "--test"]).is_err());
        assert!(
            parse(&["bench", "in"]).is_err(),
            "OUTPUT_DIR is required without --test"
        );
    }

    #[test]
    fn test_mode_conflicts_with_agentic() {
        assert!(parse(&["bench", "--test", "out", "--agentic"]).is_err());
        assert!(parse(&["bench", "--test", "out"]).is_ok());
    }
}

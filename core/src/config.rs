use std::fmt;
use std::str::FromStr;
use std::{collections::HashMap, path::PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which external agent to invoke for agentic translation and verification.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    #[default]
    OpenCode,
    Claude,
}

/// One agentic pipeline stage. Stages compose in pipeline order
/// (translate < verify < quality < conform); a run executes an ordered subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Translate,
    Verify,
    /// Safety and idiomaticity refactoring of a translated crate, guarded by the regression tests
    /// of the verify stage when the crate has them.
    Quality,
    Conform,
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Stage::Translate => write!(f, "translate"),
            Stage::Verify => write!(f, "verify"),
            Stage::Quality => write!(f, "quality"),
            Stage::Conform => write!(f, "conform"),
        }
    }
}

impl Stage {
    /// Refuses the stages of a dry-run that cannot be rendered. A dry-run renders one agentic
    /// stage: a later stage starts from the result of the stage before it, and a dry-run makes
    /// no result.
    pub fn check_dry_run(stages: &[Stage]) -> Result<(), String> {
        match stages {
            [_] => Ok(()),
            [] => Err("a dry-run renders an agentic stage: give --agentic=<stage>".to_owned()),
            _ => Err(format!(
                "a dry-run renders one stage, and this run has {}: {}. A later stage starts \
                 from the result of the stage before it, and a dry-run makes no result. Name \
                 one stage: --agentic=<stage>. A stage after translate takes a snapshot of an \
                 earlier run as its input",
                stages.len(),
                stages
                    .iter()
                    .map(Stage::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

impl FromStr for Stage {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "translate" | "t" => Ok(Stage::Translate),
            "verify" | "v" => Ok(Stage::Verify),
            "quality" | "q" => Ok(Stage::Quality),
            "conform" | "c" => Ok(Stage::Conform),
            other => Err(format!(
                "unknown stage: {other} (expected: translate/t, verify/v, quality/q, conform/c)"
            )),
        }
    }
}

/// What selects everything that the agents of a run are given: the HARVEST pipeline, or one
/// baseline arm of the ablation study. One value for the whole run. A baseline replaces the
/// prompts, the files, the skills and the backend extras of HARVEST with its own, and has only
/// the stages of [`Profile::stages`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// The HARVEST pipeline.
    #[default]
    Harvest,
    /// The agent with its native ability: one task sentence.
    Vanilla,
    /// The prompts of the ACTOR system.
    Actor,
    /// The transpiler c2rust, driven by the agent.
    C2rust,
}

impl Profile {
    /// The baseline arms.
    pub const BASELINES: [Profile; 3] = [Profile::Vanilla, Profile::Actor, Profile::C2rust];

    pub fn is_baseline(self) -> bool {
        self != Profile::Harvest
    }

    /// The stages that a run with this profile can have.
    pub fn stages(self) -> &'static [Stage] {
        match self {
            Profile::Harvest => &[
                Stage::Translate,
                Stage::Verify,
                Stage::Quality,
                Stage::Conform,
            ],
            Profile::Actor => &[Stage::Translate, Stage::Verify],
            Profile::Vanilla | Profile::C2rust => &[Stage::Translate],
        }
    }

    /// Refuses the stages of a run that this profile does not have. A baseline runs only as
    /// agentic stages.
    pub fn check_stages(self, stages: &[Stage]) -> Result<(), String> {
        if !self.is_baseline() {
            return Ok(());
        }
        let own = self
            .stages()
            .iter()
            .map(Stage::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        if stages.is_empty() {
            return Err(format!(
                "the {self} baseline runs only as agentic stages ({own}): give --agentic=<stages>"
            ));
        }
        match stages.iter().find(|stage| !self.stages().contains(stage)) {
            // The most likely cause is `--agentic` without a value.
            Some(stage) => Err(format!(
                "the {self} baseline has no {stage} stage (its stages: {own}). Name the stages: \
                 --agentic alone means translate,verify"
            )),
            None => Ok(()),
        }
    }

    /// Parses the value of `--baseline`: the name of a baseline arm.
    pub fn parse_baseline(name: &str) -> Result<Self, String> {
        match name.parse::<Profile>() {
            Ok(profile) if profile.is_baseline() => Ok(profile),
            _ => Err(format!(
                "unknown baseline: {name} (expected: vanilla, actor, c2rust)"
            )),
        }
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Profile::Harvest => "harvest",
            Profile::Vanilla => "vanilla",
            Profile::Actor => "actor",
            Profile::C2rust => "c2rust",
        })
    }
}

impl FromStr for Profile {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "harvest" => Ok(Profile::Harvest),
            "vanilla" => Ok(Profile::Vanilla),
            "actor" => Ok(Profile::Actor),
            "c2rust" => Ok(Profile::C2rust),
            other => Err(format!(
                "unknown profile: {other} (expected: harvest, vanilla, actor, c2rust)"
            )),
        }
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentKind::OpenCode => write!(f, "opencode"),
            AgentKind::Claude => write!(f, "claude"),
        }
    }
}

impl FromStr for AgentKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "opencode" | "oc" => Ok(AgentKind::OpenCode),
            "claude" => Ok(AgentKind::Claude),
            other => Err(format!(
                "unknown agent kind: {other} (expected: opencode/oc, claude)"
            )),
        }
    }
}

/// Configuration for this harvest-translate run. The sources of these configuration values (from
/// highest-precedence to lowest-precedence) are:
///
/// 1. Configurations passed using the `--config` command line flag.
/// 2. A user-specific configuration directory (e.g. `$HOME/.config/harvest/config.toml').
/// 3. Defaults specified in the code (using `#[serde(default)]`).
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Path to the directory containing the C code to translate.
    pub input: PathBuf,

    /// Path to output directory.
    pub output: PathBuf,

    /// Path to the diagnostics directory, if you want diagnostics output. If you do not specify a
    /// diagnostics path, a temporary directory will be created (so that working directories can be
    /// created for tools) and cleaned up when translate completes.
    pub diagnostics_dir: Option<PathBuf>,

    /// For both the output directory and diagnostics directory (if enabled):
    /// If true: if the directory exists and is nonempty, translate will delete the contents of the
    /// directory before running.
    /// If false: if the directory exists and is nonempty, translate will output an error and exit.
    pub force: bool,

    /// If true, use modular translation (translating one declaration at a time).
    // If false, use standard all-at-once translation.
    pub modular: bool,

    /// Which agentic stages to run, in pipeline order. Empty means the run is
    /// not agentic (direct LLM translation tools are used instead).
    #[serde(default)]
    pub stages: Vec<Stage>,

    /// When the first stage in `stages` is not `translate`, the path to the
    /// stage-input snapshot: an already-translated program directory
    /// (a previous run's output, carrying a `stage.json` manifest).
    #[serde(default)]
    pub stage_input: Option<PathBuf>,

    /// Which external agent to use for agentic translation (requires `agentic = true`).
    #[serde(default)]
    pub agentic_agent: AgentKind,

    /// Run Claude Code through TeamClaude (`teamclaude run -- ...`) when the
    /// `teamclaude` command is on PATH. No effect on the other agents.
    pub teamclaude: bool,

    /// What the agents of the agentic stages are given: the HARVEST pipeline, or a baseline arm.
    #[serde(default)]
    pub profile: Profile,

    /// A dry-run: the agentic stage of the run does everything up to the moment when its agent
    /// would start, keeps in `output` what the agent would receive, and starts no agent. The
    /// run has one stage (see [`Stage::check_dry_run`]) and no result. Not a configuration key:
    /// only the command line sets it, because the program that runs the pipeline must know it.
    #[serde(skip)]
    pub dry_run: bool,

    /// Filter describing which log messages should be output to stdout. This is in the
    /// `tracing_subscriber::filter::EnvFilter` format.
    pub log_filter: String,

    /// Sub-configuration for each tool.
    pub tools: HashMap<String, serde_json::Value>,

    // serde will place any unrecognized fields here. This will be passed to unknown_field_warning
    // after parsing to emit warnings on unrecognized config entries (we don't error on unknown
    // fields because that can be annoying to work with if you are switching back and forth between
    // commits that have different config options).
    #[serde(flatten)]
    pub unknown: HashMap<String, serde_json::Value>,
}

impl Config {
    /// The `[tools.<KEY>]` section as the tool's config type. A missing section is an empty
    /// table, so the defaults of the type apply.
    pub fn tool<T: ToolConfig>(&self) -> Result<T, serde_json::Error> {
        match self.tools.get(T::KEY) {
            Some(section) => T::deserialize(section),
            None => T::deserialize(Value::Object(Default::default())),
        }
    }

    /// Returns a mock config for testing.
    pub fn mock() -> Self {
        Self {
            input: PathBuf::from("mock_input"),
            output: PathBuf::from("mock_output"),
            diagnostics_dir: None,
            force: false,
            modular: false,
            stages: Vec::new(),
            stage_input: None,
            agentic_agent: AgentKind::default(),
            teamclaude: true,
            profile: Profile::default(),
            dry_run: false,
            log_filter: "off".to_owned(),
            tools: Default::default(),
            unknown: Default::default(),
        }
    }

    /// Returns formatted llm info.
    /// Printed at the start of translation and benchmarking runs to aid in reproduction of results.
    pub fn model_info(&self) -> Option<String> {
        if !self.stages.is_empty() {
            let stages = self
                .stages
                .iter()
                .map(Stage::to_string)
                .collect::<Vec<_>>()
                .join("+");
            let baseline = match self.profile {
                Profile::Harvest => String::new(),
                baseline => format!(" baseline={baseline}"),
            };
            return Some(format!(
                "agentic({}) stages={stages}{baseline}",
                self.agentic_agent
            ));
        }
        let tool_name = if self.modular {
            "modular_translation_llm"
        } else {
            "raw_source_to_cargo_llm"
        };

        self.tools.get(tool_name).map(|tool| {
            let backend = tool
                .get("backend")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let model = tool
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let max_tokens = tool
                .get("max_tokens")
                .map_or("<unknown>".to_owned(), |v| v.to_string());

            format!(
                "Backend={} Model={} Max Tokens={}",
                backend, model, max_tokens
            )
        })
    }
}

/// The config type of a tool: the `[tools.<KEY>]` section of the HARVEST config. It is
/// serialized for the stage manifest.
pub trait ToolConfig: DeserializeOwned + Serialize {
    /// The name of the config section.
    const KEY: &'static str;
}

/// Prints out a warning message for every field in `unknown`.
///
/// This is intended for use by config validation routines. `prefix` should be the path to this
/// entry (e.g. `tools::Config` should call this with a `prefix` of `tools`).
pub fn unknown_field_warning(prefix: &str, unknown: &HashMap<String, Value>) {
    let mut entries: Vec<_> = unknown.keys().collect();
    entries.sort_unstable();
    entries.into_iter().for_each(|name| match prefix {
        "" => eprintln!("Warning: unknown config key {name}"),
        p => eprintln!("Warning: unknown config key {p}.{name}"),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_kind_parses_names_and_alias() {
        assert_eq!("opencode".parse::<AgentKind>(), Ok(AgentKind::OpenCode));
        assert_eq!("OC".parse::<AgentKind>(), Ok(AgentKind::OpenCode));
        assert_eq!("claude".parse::<AgentKind>(), Ok(AgentKind::Claude));
        assert!(
            "gpt"
                .parse::<AgentKind>()
                .unwrap_err()
                .contains("opencode/oc")
        );
        assert_eq!(AgentKind::default(), AgentKind::OpenCode);
    }

    #[test]
    fn profile_names_and_stage_rules() {
        use Stage::{Conform, Quality, Translate, Verify};
        assert_eq!(Profile::default(), Profile::Harvest);
        for profile in Profile::BASELINES.into_iter().chain([Profile::Harvest]) {
            assert_eq!(profile.to_string().parse::<Profile>(), Ok(profile));
        }
        // `--baseline` names an arm, not the HARVEST pipeline.
        assert_eq!(Profile::parse_baseline("actor"), Ok(Profile::Actor));
        for name in ["harvest", "Vanilla", "plain"] {
            let err = Profile::parse_baseline(name).unwrap_err();
            assert!(err.contains("vanilla, actor, c2rust"), "{err}");
        }
        // HARVEST has every stage, and also the non-agentic translators.
        Profile::Harvest.check_stages(&[]).unwrap();
        Profile::Harvest
            .check_stages(&[Translate, Verify, Quality, Conform])
            .unwrap();
        Profile::Actor.check_stages(&[Translate, Verify]).unwrap();
        Profile::Actor.check_stages(&[Verify]).unwrap();
        for baseline in [Profile::Vanilla, Profile::C2rust] {
            baseline.check_stages(&[Translate]).unwrap();
            let err = baseline.check_stages(&[Translate, Verify]).unwrap_err();
            assert_eq!(
                err,
                format!(
                    "the {baseline} baseline has no verify stage (its stages: translate). Name \
                     the stages: --agentic alone means translate,verify"
                )
            );
        }
        for stage in [Quality, Conform] {
            assert!(Profile::Actor.check_stages(&[stage]).is_err());
        }
        let err = Profile::Vanilla.check_stages(&[]).unwrap_err();
        assert!(err.contains("--agentic"), "{err}");
    }

    #[test]
    fn a_dry_run_has_one_stage() {
        use Stage::{Translate, Verify};
        Stage::check_dry_run(&[Translate]).unwrap();
        Stage::check_dry_run(&[Verify]).unwrap();
        let err = Stage::check_dry_run(&[]).unwrap_err();
        assert!(err.contains("--agentic=<stage>"), "{err}");
        let err = Stage::check_dry_run(&[Translate, Verify]).unwrap_err();
        assert!(
            err.starts_with("a dry-run renders one stage, and this run has 2: translate, verify."),
            "{err}"
        );
    }

    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct Sample {
        #[serde(default = "five")]
        n: u32,
    }

    fn five() -> u32 {
        5
    }

    impl ToolConfig for Sample {
        const KEY: &'static str = "sample";
    }

    #[test]
    fn tool_section_defaults_when_missing() {
        let mut config = Config::mock();
        assert_eq!(config.tool::<Sample>().unwrap(), Sample { n: 5 });
        config
            .tools
            .insert("sample".to_owned(), serde_json::json!({"n": 7}));
        assert_eq!(config.tool::<Sample>().unwrap(), Sample { n: 7 });
    }
}
